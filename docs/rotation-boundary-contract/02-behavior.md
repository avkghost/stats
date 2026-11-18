---
parent: ./README.md
view: process
---

# Behavior: Rotation Boundary Contract

Record sets: `P` = 100 prefix records (already checkpointed), `T` = 50 tail records (appended after the checkpoint, before rename), `N` = 30 new-active records. The clean reference imports `P+T` records as the rotation, then `N` as the active file.

## Data Flow Diagrams

### DFD 1: Plain rotation, then backfill (scenario S1)

```mermaid
flowchart LR
    Gen([Generator]) -->|P lines| Active[(access.log)]
    Active -->|update| CP1[(checkpoint: identity X, P lines)]
    Gen -->|T lines appended| Active
    Active -->|rename, same inode X| Rot[(access.log.1)]
    Gen -->|N lines| NewActive[(new access.log, inode Y)]
    NewActive -->|update| Store[(stats.sqlite3)]
    Rot -->|backfill: identity X matches, skip P, read T| Store
    NewActive -->|backfill: already checkpointed| Store
    Store -->|totals/top/daily| Compare{{equal to clean import?}}
```

### DFD 2: Clean import (reference)

```mermaid
flowchart LR
    Gen([Generator]) -->|P+T lines| RefRot[(access.log.1, fresh)]
    Gen -->|N lines| RefActive[(access.log, fresh)]
    RefRot -->|update --log-file| RefStore[(clean stats.sqlite3)]
    RefActive -->|update --log-file| RefStore
```

### DFD 3: Gzip before backfill (scenario S2)

```mermaid
flowchart LR
    Rot[(access.log.1, inode X)] -->|gzip| Gz[(access.log.1.gz, inode Z)]
    Gz -->|backfill: no identity, no content, no path match| Replay["full replay of P+T lines"]
    Replay --> Store2[(stats.sqlite3)]
```

---

## Sequence Diagrams

### Use Case 1: Plain rotation recovers the tail (S1)

```mermaid
sequenceDiagram
    participant F as Fixture
    participant CLI as stats-cli
    participant FS as Log dir
    participant DB as stats.sqlite3

    F->>FS: write P lines to access.log
    F->>CLI: update -c conf
    CLI->>DB: checkpoint(identity X, P lines)
    F->>FS: append T lines to access.log
    F->>FS: rename access.log -> access.log.1 (inode X kept)
    F->>FS: write N lines to new access.log (inode Y)
    F->>CLI: update -c conf
    CLI->>FS: reads access.log only
    CLI->>DB: N lines counted, checkpoint(Y)
    F->>CLI: backfill -c conf --log-dir logs
    CLI->>FS: reads access.log.1 (X)
    CLI->>DB: identity X matches, prefix P hashes equal, skip P, count T
    CLI->>FS: access.log (Y) already checkpointed, no new lines
    F->>DB: read totals/top/daily
    F->>F: compare with clean import (DFD 2)
```

**Assertions:**
- After the normal update (before backfill): totals equal `P + N` (130 in the scratch run). The prefix `P` was counted by the first update; the tail `T` is absent because it is unread. (Acceptance criterion 2.)
- After backfill: totals equal `P+T+N` and equal the clean import. (Criterion 1.)

**Error and edge cases:**
| Condition | Expected behaviour | Covered by |
|---|---|---|
| New active file missing at normal update | `update` fails with `inspect log file` context | Not in initial scope; see 04-testing.md "Deferred" |
| Second backfill, nothing changed | No aggregate change ("unchanged input") | T3 |
| Prefix hash mismatch (file edited in place) | Replay path; not exercised by this feature | Deferred |

### Use Case 2: Clean import reference

```mermaid
sequenceDiagram
    participant F as Fixture
    participant CLI as stats-cli
    participant DB as clean stats.sqlite3

    F->>CLI: update -c conf --log-file logs/access.log.1
    CLI->>DB: P+T lines counted
    F->>CLI: update -c conf --log-file logs/access.log
    CLI->>DB: N lines counted
```

The documented manual sequence (`docs/operations.md:34-38`, oldest to newest) is used here, which is the reference the fixture compares against.

### Use Case 3: Compressed rotation before backfill (S2) - normative outcome P+T+N, counted once, current defect fixed in Phase 2

```mermaid
sequenceDiagram
    participant F as Fixture
    participant CLI as stats-cli
    participant FS as Log dir
    participant DB as stats.sqlite3

    Note over F,DB: Steps 1-5 identical to S1 through the normal update
    F->>FS: gzip access.log.1 -> access.log.1.gz (inode Z)
    F->>CLI: backfill -c conf --log-dir logs
    CLI->>FS: reads access.log.1.gz (Z)
    CLI->>DB: no identity, content, or path match, skip = 0
    CLI->>DB: P+T lines counted again, pages and bytes double for P
    F->>DB: read totals
```

Observed in the scratch run for `.gz` (research document): 280 pages and 34,590 bytes against 180 and 22,140 for the clean import, with `visits` unchanged at 180. `.bz2` and `.xz` use the same decode-then-match path (`main.rs:409-431`), so they are expected to show the same defect; T4-bz2 and T4-xz confirm this in Phase 1.

**Phase 2 target flow:** after the rotated file is decoded, its first `decoded_size` bytes are hashed and matched against the stored decoded-prefix checkpoint for the config hash. A unique match skips the stored line count and writes a checkpoint for the new identity. No match or an ambiguous match is diagnosed. See D-9 in `03-decisions.md`.

## Additional Scenarios

### Repeated backfill
- **Trigger:** backfill run twice with no file changes.
- **Behaviour:** identity and content match; `skip_lines` equals the full line count (`crates/stats-cli/src/main.rs:931-937`). No change.

### Non-Unix platforms
- **Trigger:** test run on a non-Unix target.
- **Behaviour:** `file_identity` returns `"portable"` for every file (`main.rs:1170-1177`), so identity matching is not inode-based and the S1 expectations do not hold. The fixture is `#[cfg(unix)]` and is skipped elsewhere with a message.
