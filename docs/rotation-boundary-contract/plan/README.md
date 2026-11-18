---
date: 2026-11-18
feature: rotation-boundary-contract
service: crates/stats-cli
design: ../README.md
status: complete. Phases 1-5 implemented and committed in c119d87 (fixture in accd8be). Phase 3 documentation done. Known pre-existing environmental failures: the three native_report_failure tests (see README "Findings").
---

# Code Plan: Rotation Boundary Contract - Phase 2

## Overview

Phase 2 makes backfill correct for compressed rotations (the defect pinned by T4-gz, T4-bz2, T4-xz), makes every unpartitioned write all-or-nothing, and covers copytruncate (T7). It also prevents a silent loss found while planning: a partially written trailing line in the active log (T11).

Production files changed: `../../../crates/stats-core/src/store.rs`, `crates/stats-cli/src/main.rs`, plus new modules in `stats-core`. The schema version does not change, so no existing database is cleared on upgrade.

## Decisions (final, after review)

| # | Decision | Final wording |
|---|---|---|
| A | Stable hash | Use `sha2::Sha256` for all new prefix fields. Stability across Rust toolchains is the reason. |
| B | Partitioned stores | **Split the paths.** Public `backfill` refuses when `DatabasePeriod != none` (T8). `migrate` does not call the public command; it calls `backfill_partitioned_unlocked`, an internal function that keeps today's per-period semantics. Partitioned `update` keeps its current path. |
| C | copytruncate | Included (T7). The truncated active file's superseded prefix is snapshotted; a later archive copy matches the snapshot. |
| D | Legacy compressed archive without a resolvable prefix | Abort with a recovery instruction. No override flag. |
| E | Transactional path scope | Applies to unpartitioned `update` and unpartitioned `backfill`. Partitioned `update` stays on the old path until it has its own design. |
| F | Lock wiring | **Acquired exactly once, in the command dispatcher** (`main()`), for every writing command. Internal functions are named `*_unlocked` and never take the lock. See "Lock wiring". |
| G | copy-without-truncate | Documented limitation; out of scope. |
| H | Failure policy | Non-zero exit and no writes. |
| I | Partial trailing line (new, from F-7) | The updater consumes only newline-terminated lines of the active file. The checkpoint `size` and `lines` cover only those complete lines. Archives are complete files, so their final unterminated line is consumed at end of file. See "Complete-line rule". |
| J | Preflight input and TOCTOU (new, from review) | Preflight streams each file through a hasher; it does not keep decoded content in memory. For the active file, preflight and apply use the same byte limit `S` (the size at preflight start). Apply verifies the digest of `[0, S)` only; bytes after `S` are ignored and handled by the next run. For archives, apply verifies the whole file. Any mismatch inside the verified range aborts and rolls back the transaction. See "Preflight input strategy". |
| K | Replacement policy | Additive, confirmed. A changed prefix adds the new records to the existing aggregates. This is not a replay and does not delete earlier statistics. `../../operations.md` must say this (Phase 3). |

## Lock wiring (decision F)

The lock is taken once, in the dispatcher, before any store is opened. Every command that can write is listed; the internal functions below never lock.

| Command (dispatcher) | Lock | Calls (all unlocked) |
|---|---|---|
| `update` | acquire | `update_unlocked()` then report regeneration |
| `backfill` | acquire | `backfill_unlocked()` (refuses partitioned stores, B) |
| `rebuild` | acquire | `rebuild_unlocked()` → `update_unlocked()` (today `rebuild` calls the public `update`, `main.rs:1118-1138`; it must call the unlocked form) |
| `migrate` | acquire | `migrate_unlocked()` → `backfill_partitioned_unlocked()` (replaces the call at `main.rs:1899`) |
| `live` | **acquire per refresh** (correction: it runs `update` every interval, so it is a writer) | lock around each `update_unlocked()` call; `summary` is read-only and runs unlocked |
| `tui` | **acquire per refresh** (correction: the initial and each refresh call `update_inner`) | lock around each `update_inner` call |
| `watch` | **acquire per iteration, not for the process lifetime** | loop: wait for change, acquire lock, `update_unlocked()`, release. Manual `update`, `backfill`, and `retention` can run between iterations (`main.rs:369-421`) |
| `retention` with `--remove-source` | acquire | removes partitions; takes the lock because it deletes data |
| `backup`, `optimize`, `storage`, `summary`, `report`, `tui`, `live`, `validate`, `health` | none | read-only or self-contained; to verify during implementation that none of them calls `update` or `backfill` |

Verification: a unit test asserts that `update`, `backfill`, `rebuild`, and `migrate` each acquire exactly once (a counter in the lock helper, test-only). Nested acquisition in one process would fail the `flock` call on the second attempt, so a regression shows up as a refusal in T9-style tests.

## Complete-line rule (decision I, verified as F-7)

- Lines are split on `\n` only. For the active file, a trailing fragment without `\n` is not consumed and is not counted in `lines`.
- Define, for a byte bound `B` (the active file's size `S` for active files; the decoded length for archives):
  - `complete_prefix_size` = position just after the last `\n` within `[0, B)`; 0 if there is none;
  - `prefix_lines` = number of lines through that newline;
  - `prefix_sha256` = SHA-256 of `decoded[0..complete_prefix_size]`.
- The checkpoint stores `complete_prefix_size`, `prefix_lines`, and `prefix_sha256`. Archives: the final unterminated line is consumed at EOF, so their `complete_prefix_size` is the full decoded length.
- Consequence: a fragment is neither checkpointed nor imported. When its newline arrives, the next run imports it exactly once.
- Test: T11 (in phase 05). The verified baseline is 109 hits against the clean import's 110 with the current code.

## Preflight input strategy (decision J)

Problems: a multi-day archive can be large, and the active file is appended while it is read.

Strategy:
1. **Bounded snapshot of the active file.** Preflight records the active file's size at the start (`S`) and reads only the first `S` bytes. Apply uses the same `S`. Bytes appended after `S` are ignored and handled by the next run, so a normal write between preflight and apply is not an error. The complete-line rule (decision I) is applied within `[0, S)`.
2. **Streaming preflight.** Decode each file (`GzDecoder` / `BzDecoder` / `XzDecoder`, or plain read) through a `Read` adaptor that feeds `Sha256` and records:
   - decoded length `d`;
   - SHA-256 of the decoded prefix at each state size needed by matching (sizes come from the stored states, which are known before streaming);
   - SHA-256 of the full decoded content (`full_digest`);
   - complete-line count.
   Memory is bounded by the decoder buffer, not by file size.
3. **Plan records** for each file: identity, raw size and raw SHA-256 (for archives), decoded length, `full_digest`, and the action.
4. **Apply streams again** and verifies, before `COMMIT`, that decoded length and `full_digest` equal the plan's values. Any difference (file replaced, truncated, or changed) is an error. Because the whole apply is one transaction, the error rolls back every earlier file's aggregates and checkpoints.
5. **Parse while streaming.** Apply parses lines as they stream; it does not hold the file in memory.

Why this is TOCTOU-safe: a change between preflight and apply cannot produce a commit, because the digest check precedes `COMMIT`. A change after `COMMIT` is the next run's business and is handled by the normal rules.

Trade-off: each file is decoded twice (preflight and apply). Accepted for bounded memory and atomicity.

## Phases

| # | File | Scope | Production files | Exit condition |
|---|---|---|---|---|
| 01 | [phase-01.md](phase-01.md) | Store foundation: additive columns, snapshot table, SHA-256 prefix helper | `store.rs`, new hash helper, `../../../Cargo.toml` | Legacy DB opens without replay; migration unit tests pass |
| 02 | [phase-02.md](phase-02.md) | Batch transactions | `store.rs` | Rollback unit test passes |
| 03 | [phase-03.md](phase-03.md) | `source_key`, dispatcher lock wiring | `main.rs`, `source_key.rs` | T9 passes; lock acquisition counted once per command |
| 04 | [phase-04.md](phase-04.md) | Matching, complete-line rule, streaming preflight | `checkpoint_match.rs`, `main.rs` | Matching and preflight unit tests pass |
| 05 | [phase-05.md](phase-05.md) | Apply in one transaction, unified update/backfill, migrate split, snapshots, T4-T11 | `main.rs` | All rotation tests green; `cargo test --workspace` green |

## File map

### New files
- `../../../crates/stats-core/src/prefix_hash.rs` - SHA-256 helper (phase 01)
- `../../../crates/stats-core/src/checkpoint_match.rs` - pure decision logic (phase 04)
- `../../../crates/stats-core/src/source_key.rs` - source key derivation (phase 03)
- `../../../crates/stats-cli/tests/rotation_boundary.rs` - exists (Phase 1); extended in phase 05

### Modified files
- `../../../crates/stats-core/src/store.rs` - schema additions (`:84`, migration in `open()` `:69-131`), `record` → `record_in` (`:132-357`), `with_batch` / `Batch`, prefix state methods, `reset_checkpoint` (`:431-438`)
- `../../../crates/stats-core/src/lib.rs` - exports
- `../../../crates/stats-core/Cargo.toml` - `sha2`
- `../../../crates/stats-cli/src/main.rs` - dispatcher lock (`:237-365`), `update` (`:486`), `backfill` (`:500`), `rebuild` (`:1118`), `migrate` (`:1858`, call at `:1899`), `update_inner` (`:784-1088`) split into `plan_sources`, `apply_plan`, `process_lines`, and the `*_unlocked` variants

## Error codes
Not applicable (anyhow messages). Message formats are specified in phase 04.

## Success criteria
- [ ] Phases 01-05 implemented in order; each phase's unit tests pass
- [ ] `cargo test -p stats-cli --test rotation_boundary` passes T1, T2, T3, T4-gz, T4-bz2, T4-xz, T5, T6, T7, T8, T9, T10, T11 (T5b is a stats-core unit test)
- [ ] `cargo test --workspace` passes, including `migrates_old_schema_and_invalidates_checkpoint` (`store.rs:1072`)
- [ ] Lock acquisition counted once per command (phase 03 test)
- [ ] `migrate` still replays into partitions (regression test: existing `migration_backs_up_old_store_before_replaying_into_partitions`, `main.rs:2840`)
- [ ] `cargo clippy --workspace` clean if CI runs it (check `../../../.github/workflows/ci.yml`)
- [ ] `../../operations.md` corrected in design Phase 3 (decision K wording: additive, analyzer cannot remove events without event-level storage)

## Findings from planning

- **F-1 (verified):** copytruncate then backfill double counts: 250 vs clean 150. Mechanism: the truncated identity's checkpoint is overwritten; the copied archive has a new identity and no path match. Addressed by decision C.
- **F-2 (resolved in code; documented):** `DefaultHasher` is not specified across Rust releases. `hash_bytes` now uses a frozen SipHash-1-3 implementation (`../../../crates/stats-cli/src/legacy_hash.rs`) that matches `DefaultHasher` on the toolchain that wrote existing data (unit-tested). The new config fingerprint is SHA-256. A stored legacy config hash is migrated in place, keeping aggregates and checkpoints (test T12); only a genuinely different config clears (T12b).
- **Limitation: legacy rows.** Rows written before checkpoint support (no stable prefix fields) cannot be matched against compressed archives. Such a backfill aborts with a message and needs the documented rebuild recovery (test T6). This is a permanent requirement for those databases, not a bug to fix.
- **Limitation: lock cost.** The writer lock is held through preflight. Each archive is decoded twice (preflight and apply, plus one extra pass when a backfill has several files for overlap detection). Large archives block other writers for that time.
- **Limitation: containment.** Overlap detection in one run covers both identical files and one file that begins with another. An archive that ends mid-line and is then continued by another file is not detected. Copy-without-truncate (decision G) is out of scope.
- **F-3 (verified):** an in-place rewrite that keeps 100 records and appends 10 gives 110 (correct). Truncate-and-replace with 40 different records adds them on top (150). The docs claim a "safe aggregate replay" (`docs/operations.md:9-10`), which is incorrect. Decision K confirms the additive behaviour; the docs will be corrected.
- **F-4 (from code; not reproduced):** no writer lock existed. Addressed by decisions F and the dispatcher wiring.
- **F-5 (verified by reading `store.rs:132-133`):** each event opens its own transaction; a multi-file backfill is not atomic. Addressed by phase 02 and 05.
- **F-6 (verified by reading `store.rs:108-115`):** a schema bump clears aggregates and checkpoints on open. Phase 2 does not bump the schema.
- **F-7 (verified, scratch run `partial2`):** a partially written trailing line in the active file is counted in the checkpoint's line total but rejected by the parser. When the line completes, the next update skips it. Result: 109 hits against the clean import's 110. Addressed by decision I. Live web-server logs make this reachable in normal operation.
- **F-8 (verified, scratch run `copytrunc`):** the same copytruncate double count as F-1 (listed separately for the record).
