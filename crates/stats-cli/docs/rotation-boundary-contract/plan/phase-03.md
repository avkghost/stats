phase: 03
name: source_key derivation and writer lock
layer: usecase / initService (stats-cli orchestration)
depends_on: [phase-01, phase-02]
plan: ./README.md

---
# Phase 03: source_key and writer lock

## Goal
1. Give every checkpoint a stable `source_key` derived from the configured active log, so matching can be restricted to one log family (decision: source scoping from review).
2. Serialize writers (update, backfill, rebuild, migrate) with an advisory lock, so two processes cannot read the same checkpoint and both import the same tail (F-4).

## Context
- `update_inner` builds `config` with `with_overrides(log_override, data_override)` (`crates/stats-cli/src/main.rs:785-788`). The override replaces `config.log_file`. The source key must therefore be computed from the configured path **before** the override; otherwise each archive in a backfill would get its own source.
- `resolve_log_file(config_path, configured)` resolves a relative `LogFile` against the config directory (`main.rs:1404`). Reuse it.
- Config changes already clear all checkpoints (`main.rs:52-59`, the `config_hash` comparison). A changed `LogFile` changes the config file bytes and therefore `config_hash`, so within a single database a source change already clears state today. `source_key` is defense in depth, and the design says so; it does not change the current behaviour for a single config.

## Source key derivation (`crates/stats-core/src/source_key.rs`, new)

```text
pub fn source_key(configured_log_file: &Path) -> Result<String, SourceKeyError>
```
1. `parent = configured_log_file.parent()` (default `.`), `name = configured_log_file.file_name()`.
2. `canonical_parent = fs::canonicalize(parent)`. If it fails (directory missing), return an error: "LogFile directory must exist to derive source identity: {path}".
3. `key = format!("{}/{}", canonical_parent.display(), name)`.
4. Rotation family: `name` is the configured active name, which is by definition the family base. Rotated names (`access.log.1`, `access.log.2.gz`) are never passed in, because they are only candidates. Document this.

Pure-path tests use temp directories: relative and absolute configured paths give the same key; a symlinked directory resolves to the same key as its target; a different file name in the same directory gives a different key.

Stored in `input_checkpoints.source_key` (phase 01) and in `input_prefix_snapshots.source_key`. Not hashed, so diagnostics can show it. Length is bounded by path length.

## Writer lock (`crates/stats-cli/src/main.rs`)

- Path: `config.data_dir.join("stats.lock")`. Created if missing; the data directory is created before the lock is taken.
- Acquire with `flock(fd, LOCK_EX | LOCK_NB)` through `libc` (already a dependency of `stats-cli`, `Cargo.toml`). Hold the `File` for the whole command.
- On `EWOULDBLOCK`, return an error: `"another stats process holds {path}; wait for it to finish and retry"`, non-zero exit, no writes.
- **Single acquisition in the dispatcher** (decision F). `main()` (`main.rs:237`) acquires the lock before dispatching `update`, `backfill`, `rebuild`, `migrate`, and `retention --remove-source`. Acquisition happens before any store is opened, so a refused run cannot open or migrate the store.
- **`watch` is the exception:** it acquires the lock per iteration, around one `update_unlocked()` call, and releases it before waiting for the next change. A process-lifetime lock would block manual `update`, `backfill`, and `retention` indefinitely.
- Internal functions are split so that none of them locks:
  - `update` → `update_unlocked`; `backfill` → `backfill_unlocked`
  - `rebuild` → `rebuild_unlocked` → `update_unlocked` (today `rebuild` calls the public `update`, `main.rs:1118-1138`)
  - `migrate` → `migrate_unlocked` → `backfill_partitioned_unlocked` (replaces the call to `backfill` at `main.rs:1899`)
  - `watch` takes the lock around each `update_unlocked` call (`main.rs:369-421`)
- The lock helper carries a test-only acquisition counter. A test asserts that each writing command acquires exactly once per process. A second acquisition in the same process fails `flock`, so nested locking is refused at runtime as well as caught by the test.
- Released when the `File` drops, including on error paths.

Note: `flock` is advisory and Unix-only. On non-Unix the lock is a no-op and the non-Unix build keeps the existing unlocked behaviour; say so in the code comment and in the skip test (T-non-unix).

## Files to Modify
- `crates/stats-core/src/source_key.rs` (new), `crates/stats-core/src/lib.rs` (export)
- `crates/stats-cli/src/main.rs`: `update`, `backfill`, `rebuild`, `migrate` take the lock; `update_inner` receives the source key as a parameter computed from the base config.

## Tests

| Test | Verifies |
|---|---|
| `source_key_is_stable_for_relative_and_absolute_paths` | Same file via `./logs/access.log` and absolute path gives the same key. |
| `source_key_follows_symlinked_directories` | Symlinked parent resolves to the target's key. |
| `source_key_differs_by_file_name` | `access.log` vs `other.log` in one directory differ. |
| `source_key_requires_existing_directory` | Missing parent is an error, not a silent key. |
| `T9_concurrent_writer_is_refused` (integration, `rotation_boundary.rs`) | The test takes `flock` on `data/stats.lock` through `libc`, then runs `backfill`. Expected: non-zero exit, stderr contains "another stats process", and the store's totals and checkpoint count are unchanged. |
| `lock_released_after_command` (integration) | After a successful `update`, a second `update` acquires the lock. |

## Verification
- `cargo test -p stats-core source_key` and `cargo test -p stats-cli --test rotation_boundary t9 lock_released`.
- Existing T1-T3 still pass (the lock does not change single-writer behaviour).
- No matching change yet: T4-* still fail, as expected for this phase.
