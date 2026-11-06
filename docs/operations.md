# Operations

## Incremental updates

`update` is cumulative by default. It appends unseen events to the existing
database and regenerates the complete HTML site at the default
`output/index.html` sibling of the configured data directory. Source
identity and content hashes are checked before an append checkpoint is reused.
Rewrites, truncation, replacement, or changed prefixes trigger a safe aggregate
replay. Use `report --output <path>` when a custom report location is required.

The updater analyzes only the configured active file. It does not open
`access.log.1`, `access.log.2`, or compressed archives during a normal update.
Per-file checkpoints recognize replacement, truncation, and recreation at the
active path, so normal rotation preserves already-counted history.

Use `rebuild` only when intentionally clearing derived aggregates and replaying a
selected source. It is not the multi-file backfill operation.

## Rotation and backfill

For a conventional rotation directory, `backfill` discovers the configured
active filename plus numeric rotations such as `.1`, `.2.gz`, `.3.bz2`, and
`.4.xz`, processes them oldest-to-newest, and regenerates the site once at the
end. Do not call `backfill` from a recurring rotation hook; use it only for an
explicit historical import after confirming the archive set is complete.

Historical files can also be supplied explicitly, oldest to newest, without
using `rebuild` between files:

The `logs/...` paths below are illustrative placeholders. Replace them with
the actual active log and rotated archives for the deployment.

```text
cargo run -p stats-cli -- update --config logs/awstats.example.test.conf --log-file logs/access.log.2.gz --data-dir stats
cargo run -p stats-cli -- update --config logs/awstats.example.test.conf --log-file logs/access.log.1 --data-dir stats
cargo run -p stats-cli -- update --config logs/awstats.example.test.conf --log-file logs/access.log --data-dir stats
```

`.gz`, `.bz2`, and `.xz` files are decompressed for parsing. Repeated unchanged
invocations are recognized by per-file checkpoints and do not duplicate events.

Recommended rotation order is: update, generate the report, rotate the log,
create the new active file, and update again when new lines arrive. Existing
aggregates are not deleted during normal rotation handling.

## Partitioning and retention

For large retention periods, add `DatabasePeriod=monthly` or `DatabasePeriod=yearly`.
The control database remains at `DirData/stats.sqlite3`; event aggregates are
stored under `DirData/periods/YYYY-MM/stats.sqlite3` or
`DirData/periods/YYYY/stats.sqlite3`. Reports, summaries, and the TUI merge all
period databases automatically, while shared control session state preserves
continuity across boundaries. Late or out-of-order input remains approximate.

`retention --before YYYY-MM` or `--before YYYY` lists matching old partitions
without changing them. Add `--archive-dir` to create consistent SQLite snapshots
under `archive-dir/periods/...`; add `--remove-source` only after verifying the
archive. The archive directory must be separate from the live data directory.

## SQLite maintenance and backups

SQLite uses WAL mode, a five-second busy timeout, normal synchronous durability,
and in-memory temporary storage. Run `optimize` after large ingestion batches;
add `--vacuum` during a maintenance window when reclaiming deleted space is
worth the exclusive rewrite cost. `storage` includes SQLite `-wal` and `-shm`
sidecars in its totals.

Use `backup` for consistent snapshots of control and period databases. The
destination must not already exist; the command uses SQLite's online `VACUUM INTO`
snapshot mechanism and preserves the partition layout.

To migrate an existing single database, first set `DatabasePeriod=monthly` or
`DatabasePeriod=yearly`, then run `migrate`. It requires a new backup directory,
backs up current data, clears active aggregates, and replays every discovered
log. Keep the backup until new totals are verified. If migration is interrupted,
keep the backup and rerun with a different backup directory only after checking
the destination data; `migrate` refuses to overwrite an existing backup directory.

## Data interpretation

Reports include source/configuration fingerprints, schema metadata, entry/exit
page dimensions, and approximation notes so consumers can distinguish measured
values from heuristic visitor, session, and bot metrics. Schema changes invalidate
stale aggregate checkpoints and trigger a safe replay.

Visit and unique-visit keys are deterministic pseudonymous FNV-1a aggregation
identifiers. Raw client IP addresses are not stored, but these keys are not
cryptographic anonymization and should be protected as derived analytics data.
