phase: 01
name: Store foundation - additive schema, prefix states, stable hash
layer: adapter (stats-core store)
depends_on: none
plan: ./README.md

---
# Phase 01: Store foundation

## Goal
Add the columns and table that the matching algorithm needs, without changing the schema version and without clearing any existing data. An existing database must open, keep its aggregates and checkpoints, and gain the new fields as empty.

## Context
Current schema, `crates/stats-core/src/store.rs:84`:
`input_checkpoints(identity PRIMARY KEY, path, config_hash, content_hash, size, lines)` with index `input_checkpoints_content(content_hash, config_hash)`.

`SCHEMA_VERSION = "13"` (`store.rs:6`). `open()` replays (clears aggregates and checkpoints) when the stored version differs or when the entry-exit marker is missing (`store.rs:108-115`). This phase must not change either constant, otherwise the replay path clears archive-derived counts (F-6).

The existing additive pattern is the `last_url` column migration inside `open()` (`store.rs:~83-86`, `PRAGMA table_info` then `ALTER TABLE ... ADD COLUMN`). Reuse it.

---

## Files to Modify

### `crates/stats-core/src/store.rs`

**Additive migration (inside `open()`, after the `input_checkpoints` CREATE at `:84`):**
- `PRAGMA table_info(input_checkpoints)`; for each missing column run `ALTER TABLE input_checkpoints ADD COLUMN ...`:
  - `source_key TEXT NOT NULL DEFAULT ''`
  - `prefix_size INTEGER`
  - `prefix_lines INTEGER`
  - `prefix_sha256 TEXT`
- `CREATE INDEX IF NOT EXISTS input_checkpoints_source ON input_checkpoints(source_key, config_hash)`.
- `CREATE TABLE IF NOT EXISTS input_prefix_snapshots(source_key TEXT NOT NULL, config_hash TEXT NOT NULL, prefix_size INTEGER NOT NULL, prefix_lines INTEGER NOT NULL, prefix_sha256 TEXT NOT NULL, identity TEXT NOT NULL, path TEXT NOT NULL, captured_at INTEGER NOT NULL, PRIMARY KEY(source_key, config_hash, prefix_size, prefix_sha256))`.
- Rows created before this change have `source_key = ''` and NULL prefix fields. Call them **legacy rows**. They are never treated as stable states (see phase-04).

**Migration must not touch `needs_replay`**. Keep `SCHEMA_VERSION` at `"13"`.

**New types (store.rs, near `InputCheckpoint` at `:22`):**
```text
PrefixState { source_key, config_hash, identity, path, size: u64, lines: u64, sha256: String }
```
Do not add fields to `InputCheckpoint`. Its struct literal is constructed in existing tests (`store.rs:1101-1118`) and in `main.rs`; changing it would widen this phase for no gain. Stable fields are written through a separate method.

**New store methods (single-statement, used outside batches for tests and by phase 02 batch wrappers):**
- `set_prefix_state(&self, state: &PrefixState) -> Result<(), StoreError>`: updates `source_key`, `prefix_size`, `prefix_lines`, `prefix_sha256` on the row with `identity`, inserting the row if absent (path, config_hash, content_hash may be set to empty string for inserted rows; the legacy `content_hash` column is NOT NULL, so an insert must set it, using the empty string).
- `prefix_states(&self, source_key: &str, config_hash: &str) -> Result<Vec<PrefixState>, StoreError>`: current rows with `prefix_sha256 IS NOT NULL`, plus snapshot rows (phase 05 uses the snapshot part).
- `legacy_checkpoint_count(&self, source_key: &str, config_hash: &str) -> Result<u64, StoreError>`: rows with `prefix_sha256 IS NULL` for that source.
- `snapshot_prefix(&self, state: &PrefixState, captured_at: i64) -> Result<(), StoreError>`: `INSERT OR IGNORE` into `input_prefix_snapshots`.

**Stable hash helper** (new `crates/stats-core/src/prefix_hash.rs`, decision A):
- `pub fn prefix_sha256(bytes: &[u8]) -> String` (lowercase hex of `sha2::Sha256`).
- Also `pub struct PrefixDigest` with `update(&[u8])` and `finish()`, so preflight can stream a decoder through the hasher without holding the content (decision J).
- Unit test with the standard vectors for empty input and `"abc"`, so the algorithm is pinned and any change fails loudly.

**Meaning of the new fields (decision I, complete-line rule):**
- `prefix_size` = bytes through the last `\n` processed (for archives, the full decoded length, since archives are complete).
- `prefix_lines` = complete lines in that range.
- `prefix_sha256` = SHA-256 of exactly those `prefix_size` bytes.

**`reset_checkpoint` (`store.rs:431-438`)**: also clears `input_prefix_snapshots`. Snapshots describe checkpointed history, so they are reset together with checkpoints.

### `crates/stats-core/src/lib.rs`
Export `PrefixState` and `prefix_sha256`.

### `crates/stats-core/Cargo.toml`
Add `sha2` (decision A, approved), pinned to the current major version at implementation time. Check the license against the workspace and confirm the crate is not already transitively present in `Cargo.lock`, so the diff shows the exact new dependency.

---

## Key Decisions
- The legacy `content_hash` column stays NOT NULL. Stable rows write an empty string there. Existing exact-match lookups (`store.rs:463-510`) do not match empty strings, so new rows cannot be mistaken for legacy ones.
- `prefix_sha256` is the only field used for compressed matching. `content_hash` keeps its legacy meaning for plain-file compatibility.
- No schema version bump: avoids F-6.

## Tests (stats-core unit tests in `store.rs` test module)

| Test | Verifies |
|---|---|
| `legacy_database_gains_columns_without_replay` | Create the pre-change `input_checkpoints` table with raw rusqlite and a legacy row (`content_hash` set, no new columns), set `schema_version = "13"`, insert a `totals` row. Open with `StatsStore::open`. Assert: `totals` unchanged; legacy row present; `prefix_sha256` is NULL; `schema_version` still `"13"`. |
| `stable_state_round_trips` | `set_prefix_state` then `prefix_states` returns the same size, lines, sha256, identity. |
| `legacy_rows_are_counted_separately` | `legacy_checkpoint_count` returns 1 for the legacy row and 0 after a stable state is written for that identity. |
| `snapshot_insert_is_idempotent` | Same snapshot twice produces one row. |
| `reset_checkpoint_clears_snapshots` | Snapshot inserted, `reset_checkpoint` run, snapshot count is 0. |
| `prefix_sha256_known_vectors` | Empty and `"abc"` vectors (SHA-256 standard values if approved). |

## Verification
- `cargo test -p stats-core` passes, including the existing `migrates_old_schema_and_invalidates_checkpoint` (`store.rs:1072`) which still expects version `13` and cleared checkpoints after a version `3` open.
- No change to `main.rs` in this phase.
