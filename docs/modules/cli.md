# `stats-cli`

`stats-cli` is the orchestration layer and the primary operator interface. It
loads configuration, selects input files, coordinates `stats-core`, and invokes
renderers or frontend loops.

## Command groups

| Group | Commands | Purpose |
| --- | --- | --- |
| Ingestion | `update`, `rebuild`, `backfill`, `migrate` | Read logs and update single or partitioned stores |
| Reports | `report`, `summary`, `health`, `storage` | Query, render, and inspect stored data |
| Runtime | `watch`, `live`, `tui` | Refresh reports or dashboards |
| Maintenance | `backup`, `optimize`, `retention` | Protect, compact, and remove historical data |
| Diagnostics | `validate`, `raw-log` | Check configuration and produce bounded redacted output |
| Plugin | `plugin-worker` | Run a renderer worker process |

## Ingestion ownership

`main.rs` owns command dispatch and compatibility-oriented CLI behaviour.
`atomic_update.rs` owns planning, preflight, overlap checks, apply, and commit
for unpartitioned writes. `writer_lock.rs` protects writers on Unix. The
`reverse_dns.rs` module is opt-in enrichment and is not part of the core parser.

Normal `update` reads only the configured active log. `backfill` discovers the
rotation family and processes it oldest-first. The complete-line rule, compressed
prefix matching, copytruncate handling, and recovery procedure are normative in
[Operations](../operations.md#rotation-and-backfill).

## Write safety

- The dispatcher acquires the advisory writer lock once for a writing command.
- Unpartitioned update and backfill plan and apply inside one transaction.
- Preflight verifies input digests before commit; failures leave no partial
  aggregate or checkpoint changes.
- Partitioned `backfill` is refused. `migrate` uses its dedicated partitioned
  replay path.
- `watch`, `live`, and `tui` acquire the lock per refresh so they do not block
  manual commands for their entire process lifetime.

See [Operations](../operations.md#locking) for lock scope and cost, and the
[rotation contract](../rotation-boundary-contract/README.md) for the fixture
and acceptance tests.

## Configuration and health checks

The unpartitioned atomic path fingerprints the configuration bytes with
SHA-256. A legacy fingerprint is migrated in place; a genuinely different
configuration clears aggregates and checkpoints inside the same transaction
before replay. This logic is in `atomic_update.rs` and is covered by the
configuration migration tests.

`health` is read-only. It checks the configured log file, absolute plugin
resource paths, and every existing control or period database without creating a
missing database. It emits human-readable diagnostics by default or a JSON
`HealthReport`; any missing or unreadable resource returns a non-zero result.

## Runtime flows

### Multi-file backfill (D7)

```mermaid
sequenceDiagram
    actor Operator
    participant Dispatch as dispatcher (main.rs)
    participant Lock as writer_lock
    participant Backfill as backfill (main.rs:563)
    participant Run as atomic_update::run
    participant Store as StatsStore::with_batch
    participant Apply as apply_file
    participant Report as regenerate_default_report

    Operator->>Dispatch: backfill --config --log-dir
    Dispatch->>Lock: acquire exclusive, non-blocking
    Dispatch->>Backfill: run with the lock held
    Backfill->>Backfill: discover rotations, oldest first
    Backfill->>Run: run(config, active, files)
    Run->>Store: BEGIN IMMEDIATE
    Run->>Store: config hash: same, migrated, or cleared
    loop each file, plan
        Run->>Run: scan digests at known sizes and the complete-line boundary
        Run->>Run: decide: skip, new, or abort
    end
    Run->>Run: overlap check across files
    alt any file aborts, or files overlap
        Run->>Store: rollback, nothing written
        Run-->>Operator: error, non-zero exit
    else plan accepted
        loop each file, apply
            Run->>Apply: apply_file
            Apply->>Apply: replay lines after the skip, parse, enrich, record
            Apply->>Apply: verify digests of the planned bound
            Apply->>Store: checkpoint and prefix state
        end
        Run->>Store: COMMIT
        Run->>Report: write the site once
    end
```

### Partitioned update (D8)

```mermaid
flowchart TB
    Update["update (main.rs:547)"] --> Dispatch["update_inner (main.rs:891)"]
    Dispatch --> Period{"DatabasePeriod"}
    Period -->|none| Atomic["atomic_update::run: one transaction"]
    Period -->|monthly or yearly| Legacy["update_inner_partitioned (main.rs:908)"]
    Legacy --> Router["StoreRouter.open (main.rs:753)"]
    Router --> Lookup["per-file checkpoint lookup: identity, content, logical hash, legacy meta"]
    Lookup --> Parse["parse and enrich each line"]
    Parse --> Batch["record_event_batch, every 256 events"]
    Batch --> Key{"partition key"}
    Key -->|Month or Year| Period1["periods/KEY/stats.sqlite3, session state merged to control"]
    Key -->|no partition| Control["control stats.sqlite3"]
    Legacy --> Ckpt["checkpoint written per file; no verify step; no single transaction"]
```

### Refresh loops (D9)

`watch`: a file event marks the store dirty, and the next interval tick runs an update.

```mermaid
sequenceDiagram
    actor Operator
    participant Watch as watch (main.rs:420)
    participant Lock as writer_lock
    participant Update as update
    participant FS as log directory (notify)

    Operator->>Watch: stats-cli watch
    Watch->>Lock: acquire
    Watch->>Update: initial update
    Watch->>Lock: release
    Watch->>FS: watch the parent directory of LogFile
    loop until Ctrl-C
        FS-->>Watch: file event sets dirty
        Note over Watch: next interval tick, if dirty
        Watch->>Lock: acquire
        Watch->>Update: update
        Watch->>Lock: release
    end
```

`live`: each interval runs an update under the lock, then prints the read-only summary.

```mermaid
sequenceDiagram
    actor Operator
    participant Live as live (main.rs:2284)
    participant Lock as writer_lock
    participant Update as update
    participant Summary as summary

    Operator->>Live: stats-cli live
    loop each interval until Ctrl-C
        Live->>Lock: acquire
        Live->>Update: update
        Live->>Lock: release
        Live->>Summary: print summary, read-only
    end
```

`tui`: the initial update and each refresh run under the lock; the dashboard loop stays unlocked.

```mermaid
sequenceDiagram
    actor Operator
    participant Tui as tui (main.rs:2318)
    participant Lock as writer_lock
    participant Update as update_inner (quiet)
    participant Report as build_report
    participant Dash as stats-tui run loop

    Operator->>Tui: stats-cli tui
    Tui->>Lock: acquire
    Tui->>Update: initial update
    Tui->>Lock: release
    Tui->>Report: build report
    Tui->>Dash: run with refresh callback
    loop each refresh, or key r
        Dash->>Lock: acquire, inside the callback
        Dash->>Update: update_inner
        Dash->>Lock: release
        Dash->>Report: build report
    end
```

### Writer lock by command (D10)

```mermaid
flowchart LR
    subgraph Writers["writer lock, once per command"]
        update["update"]
        rebuild["rebuild"]
        backfill["backfill"]
        migrate["migrate"]
    end
    subgraph DataLock["data-directory lock"]
        optimize["optimize"]
        retention_remove["retention --remove-source"]
    end
    subgraph PerRefresh["writer lock per refresh, released between refreshes"]
        watch["watch"]
        live["live"]
        tui["tui"]
    end
    subgraph NoLock["no lock: read-only or self-contained"]
        report["report"]
        summary["summary"]
        health["health"]
        storage["storage"]
        backup["backup"]
        validate["validate"]
        rawlog["raw-log"]
        retention_list["retention (list only)"]
    end
    rebuild -->|calls the unlocked update, so the lock is not taken twice| update
```
