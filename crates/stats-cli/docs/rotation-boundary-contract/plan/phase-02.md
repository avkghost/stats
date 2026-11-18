phase: 02
name: Batch transactions in the store
layer: adapter (stats-core store)
depends_on: [phase-01]
plan: ./README.md

---
# Phase 02: Batch transactions

## Goal
Let the CLI write a whole backfill (aggregates, checkpoints, meta, diagnostics) inside one SQLite transaction, so that a failure at any point leaves the store unchanged (F-5).

## Context
- Today `StatsStore::record` opens its own transaction per event (`crates/stats-core/src/store.rs:132-133`, `tx.commit()` at the end of the method). `set_input_checkpoint` (`:511`) and `set_meta` run as autocommit statements.
- `update_inner` writes checkpoints after each file (`main.rs:966`, `:1035`) and meta at the end (`main.rs:1062-1072`). `backfill` calls `update_inner` once per file (`main.rs:534`), so today a failure in file k leaves files 1..k-1 committed.
- Per-event commits are also a performance cost; a single transaction per backfill is cheaper, but that is secondary.

## Files to Modify

### `crates/stats-core/src/store.rs`

**New API:**
```text
impl StatsStore {
    pub fn with_batch<T>(&self, f: impl FnOnce(&Batch<'_>) -> Result<T, StoreError>) -> Result<T, StoreError>
}
pub struct Batch<'a> { tx: &'a Transaction<'a> }
```
- `with_batch` locks the connection, starts `transaction_with_behavior(TransactionBehavior::Immediate)` (takes the write lock up front, so a concurrent writer fails fast instead of interleaving), runs `f`, commits on `Ok`, and drops (rolls back) on `Err`.
- `Batch` exposes, all bound to the same transaction:
  - `record(&AccessEvent, is_page: bool)`
  - `set_input_checkpoint(&InputCheckpoint)`
  - `set_prefix_state(&PrefixState)`
  - `snapshot_prefix(&PrefixState, captured_at)`
  - `delete_checkpoint(identity)`
  - `clear_aggregates()` and `reset_checkpoint()` (same statements as the existing methods, `store.rs:519-525` and `:431-438`)
  - `set_meta(key, value)`, `diagnostic(message)`
  - reads needed by the plan: `input_checkpoint_by_identity`, `input_checkpoint_by_content_and_path`, `prefix_states`, `legacy_checkpoint_count`, `meta`

**Refactor `record`:**
- Move the body of `StatsStore::record` (`store.rs:132-~357`) into `fn record_in(tx: &Transaction<'_>, event: &AccessEvent, is_page: bool) -> Result<(), StoreError>`. Remove its `unchecked_transaction` and `commit`.
- `StatsStore::record` (public, unchanged signature) becomes `self.with_batch(|b| b.record(event, is_page))`, so existing callers and tests keep working.
- **Verify during implementation:** if the body calls any `self` method that locks `conn`, that call must become a `tx`-based helper, because the mutex is not re-entrant. Known candidates are the session-state helpers used by `StoreRouter` (`session_state_for_event`, `seed_session_state`, `merge_session_state`, `store.rs:373-412`), which are used only in partitioned mode. Confirm this when moving the code; do not assume.

**Helper functions `bump`, `decrement` (`store.rs:703-718`)** already take `&Transaction`, so they need no change.

## Key Decisions
- `BEGIN IMMEDIATE` for every batch. Readers in WAL mode are not blocked by it; a second writer gets `SQLITE_BUSY` after the 5-second timeout (`store.rs:71`).
- `Batch` is not `Clone` and cannot escape the closure, so no batch can commit after an error path.
- Diagnostics written inside a batch are rolled back with it on failure. The CLI prints the error to stderr, which is the operator-visible record of an aborted backfill.

## Tests (stats-core unit tests)

| Test | Verifies |
|---|---|
| `batch_commits_all_writes_on_ok` | Record two events and set a checkpoint inside `with_batch`; afterwards totals and checkpoint are present. |
| `batch_rolls_back_everything_on_err` | Inside `with_batch`: record one event, set a checkpoint, then return `Err`. Afterwards totals are zero, checkpoint absent, diagnostics absent. |
| `single_record_api_still_commits` | Public `StatsStore::record` still produces the same totals as before (regression against the refactor). |
| `batch_takes_write_lock_up_front` | Hold a `with_batch` in one thread that sleeps, a second `StatsStore` connection on the same file attempts a write and gets busy (or completes after release). Keep this test simple; mark timing-sensitive parts with generous bounds. |

## Verification
- `cargo test -p stats-core` passes, including the existing store tests (`store.rs:916-1195`).
- Totals produced by the existing rotation tests (`crates/stats-cli/tests/rotation_boundary.rs`, T1-T3) are unchanged, because the behaviour of a single-event `record` is unchanged.
- `cargo test -p stats-cli --test rotation_boundary` still shows T1-T3 passing and T4-* failing (the defect is not yet fixed in this phase).
