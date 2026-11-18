phase: 05
name: Apply in one transaction; unified update; snapshots; tests T4-T7, T10
layer: usecase (apply) + initService (command wiring)
depends_on: [phase-01, phase-02, phase-03, phase-04]
plan: ./README.md

---
# Phase 05: Apply, unify, and close the contract

## Goal
Apply a validated plan in one transaction, route normal `update` through the same path, record superseded prefixes on truncation (copytruncate), and make T4, T5, T6, T7 and T10 pass.

## Apply (`crates/stats-cli/src/main.rs`)

The command flow for `update` and `backfill` becomes:

```text
1. acquire writer lock (phase 03)
2. open store; compute source_key and config_hash from the base config
3. store.with_batch(|b| {
       plan = plan_sources(b, files, source_key, config_hash)?   // reads only; may Err
       if plan.reset  { b.clear_aggregates()?; b.reset_checkpoint()?; }
       for file in plan.files {
           if let Some(prev) = &file.snapshot_before { b.snapshot_prefix(prev, now)?; }
           if let Import { skip_lines } = file.action {
               process_lines(b, decoded_lines[skip_lines..], ctx)?;   // existing per-line code
           }
           if let Some(old) = &file.replaces { b.delete_checkpoint(old)?; }
           b.set_input_checkpoint(&file.legacy_fields)?;   // path, size, lines, content_hash (legacy)
           b.set_prefix_state(&file.new_state)?;           // stable fields
       }
       b.set_meta(...)   // input_*, config_hash, source_key, as today
       Ok(())
   })?
4. regenerate report (outside the transaction; a failure here leaves the committed store intact, and a rerun regenerates it)
```

- `process_lines` is the existing per-line body (`main.rs:1000-1032` and the batch flush at `:1020-1025`), moved into a function that takes `&Batch` instead of `&mut StoreRouter`. The reverse-DNS enrichment and native-plugin calls are unchanged.
- `reset` and every checkpoint write happen inside the transaction, so a failure during apply rolls back the reset as well. This removes the risk that a failed backfill leaves the store cleared (see the review concern about partial state).
- `update` builds a plan with one file (the active path) and runs the same flow. Its per-file checkpoint and meta writes (`main.rs:966`, `:1035`, `:1062-1072`) move into the transaction. Decision E.
- `migrate` and partitioned stores: unchanged paths, see decision B (refusal below).

### Partitioned stores (`DatabasePeriod` monthly or yearly), decision B (split)
- **Public `backfill`** refuses with a non-zero exit when `DatabasePeriod != none`: `backfill is not supported when DatabasePeriod is set; see docs/operations.md`. T8 checks this.
- **`migrate`** does not call public `backfill`. It calls `backfill_partitioned_unlocked`, an internal function that runs the current per-file, per-period replay semantics (`main.rs:1899`'s call moves to this function). Its behaviour is unchanged, and the existing migration test (`main.rs:2840`) is the regression guard.
- **Partitioned `update`** keeps the current per-event path. It does not get the Phase 2 atomicity or the copytruncate snapshots. Documented as a limitation; a separate design is needed (decision E).

## Snapshot on truncation (decision C, copytruncate)

- Trigger: a candidate with an existing current row for the same identity, where R1 did not match. Typical cause: `copytruncate` (same inode truncated) or replacement in place.
- Action: before the row is overwritten, `snapshot_prefix(prev)` stores the superseded state. The superseded content was real traffic, so its aggregates stay counted; the snapshot exists only so that a later archive copy of that content is recognised.
- Consequence for the copied archive: when `backfill` later reads the copy (new identity, same bytes as the snapshot), R1 gives a full match and `Skip(all)`. Nothing is imported twice.

## Replacement policy (decision K, confirmed)
A changed prefix adds the new records to the existing aggregates. This is not a replay and does not delete the earlier statistics. The analyzer cannot remove already-aggregated events, because the store keeps aggregates, not event rows. `docs/operations.md:9-10` currently says "trigger a safe aggregate replay", which is wrong; design phase 3 replaces that sentence with the additive description above.

## Files to Modify
- `crates/stats-cli/src/main.rs`: `update` (`:486-498`), `backfill` (`:500-542`), `update_inner` (`:784-1088`) split into `plan_sources`, `apply_plan`, `process_lines`
- `crates/stats-core/src/store.rs`: none beyond phases 01-02 (apply uses the `Batch` API)
- `crates/stats-cli/tests/rotation_boundary.rs`: add T5, T6, T7, T8, T10; T4-* expectations unchanged

## Test cases

### T4-gz, T4-bz2, T4-xz - expected GREEN
Same as Phase 1: `P+T+N` counted once, aggregates equal the clean import. Mechanism: R1 gives `Skip(100)` for the rotated archive, which matches the stored active state of 100 records.

### T5 - ambiguous compressed match aborts without writes
Setup: steps 1-4 of T1, then `access.log.1.gz` and `access.log.2.gz` with identical decoded content (both contain the first `P+T` records).
Expected:
- Snapshot the aggregate comparison surface and the row count of `input_checkpoints` and `input_prefix_snapshots` before the run.
- `backfill`: non-zero exit; stderr contains `ambiguous` or `contain the same records`, and both file names and the source key.
- After the run, every aggregate and both row counts equal the snapshot.
- Control: delete `access.log.2.gz`, rerun `backfill`: exit 0; aggregates equal the clean import (`P+T+N`).

### T5b - source scoping (store unit test, not CLI)
Two states with the same `prefix_sha256` and size, stored under two `source_key` values with the same `config_hash`. `prefix_states(source_a, config)` returns only source A's states; `decide` for a candidate of source B against source A's states gives `New`, not a match. Lives in `stats-core` unit tests (`store.rs` and `checkpoint_match.rs`), because the CLI cannot produce two sources under one config hash (a changed `LogFile` changes `config_hash`).

### T6 - legacy upgrade without replay
Setup: a database created with the pre-change schema (created with raw rusqlite in the test, using the Phase 1 legacy fixture): `schema_version = "13"`, aggregates for P records, one legacy checkpoint row for the active file, no new columns.
Steps:
1. Open the database with the new binary and run a normal `update` with no new lines. Expected: totals unchanged (P); no replay; `schema_version` still `"13"`.
2. Append N records, normal `update`. Expected: totals `P+N`; the active file's row now has stable fields.
3. Rotate: compress the **previous** active file, which was checkpointed only by the legacy row, into `access.log.1.gz`, then `backfill`. Expected: non-zero exit, `Unresolvable` message, totals unchanged.
4. Recovery (the documented procedure, to be written in phase 3): `rebuild --log-file access.log.1.gz` then `update --log-file access.log`. Expected: totals equal the clean import; a further `backfill` is a no-op.

Note on step 3: the legacy row here is for a plain file whose identity no longer exists after compression. Plain legacy rows whose file still exists would be resolved by R3 instead; add a second variant if you want that path covered.

### T7 - copytruncate (decision C)
Setup: P records, normal `update`. Then `cp access.log access.log.1` (new identity, plain), truncate `access.log` in place (same identity), write N new records, normal `update`.
Steps:
1. After the truncation update, expected totals `P+N` (the truncated content is new; the superseded P is snapshotted, not removed).
2. `backfill --log-dir`: the copy matches the snapshot exactly (R1, `size == d`), so `Skip(all)`. Expected totals `P+N`.
3. Compare with the clean import of `access.log.1` (P) plus `access.log` (N): totals `P+N` equal.

Corrected expectation: the research run gave 250 for this sequence before the fix; after the fix it must equal `P+N`, i.e. 150 with P=100 and N=50 as in the research run. Record the exact numbers in the test, computed from `P` and `N`.

### T8 - partitioned stores refuse backfill
Setup: `DatabasePeriod=monthly`, any archive. Expected: `backfill` exits non-zero with the partitioned-store message; no `periods/` writes; totals unchanged.

### T9 - writer lock (phase 03)
As in phase 03.

### T10 - corrupt archive aborts before any write
Setup: steps 1-4 of T1, plus `access.log.1.gz` containing a truncated gzip stream.
Expected: `backfill` non-zero exit with the decode error naming the file; aggregates and checkpoint counts unchanged. (This is the atomicity guarantee at the CLI level. The store-level rollback test is in phase 02.)

## Verification
- `cargo test -p stats-cli --test rotation_boundary`: T1-T10 pass (T9 from phase 03). T4-* are green.
- `cargo test --workspace`: existing unit and integration tests pass, including `migrates_old_schema_and_invalidates_checkpoint`.
- Manual check with the research scripts (`.../scratchpad/rot`, `.../scratchpad/copytrunc`): totals now match the clean import in the scenarios that previously gave 280 and 250.

### T11 - partial trailing line in the active file (decision I)
Setup: 100 complete records in `access.log`, then the first 60 bytes of record 101 (no newline).
Steps:
1. Normal `update`. Expected: totals 100 (the fragment is not consumed; checkpoint `size` ends at the last newline).
2. Append the remaining bytes of record 101 with its newline, then 9 more complete records. Normal `update`. Expected: totals 110.
3. Clean import of the final file: totals 110. Equal to step 2.
Baseline with the current code: 100, then 109 (verified, F-7). T11 is red until phase 05.

## Open questions (phase 05)
None outstanding from the review. Documentation edits (`docs/operations.md`, `deploy/stats-logrotate`) remain in design Phase 3, after these tests pass.
