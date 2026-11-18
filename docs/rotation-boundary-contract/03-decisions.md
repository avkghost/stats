---
parent: ./README.md
view: decision
---

# Design Decisions: Rotation Boundary Contract

## Normative contract (approved)

After a rotation of the active log to a plain, gzip, bzip2, or xz archive, followed by `backfill`, the aggregates must equal a clean import of the same records: prefix `P` + tail `T` + new active `N`, counted exactly once. This contract is implemented and covered by the rotation fixture.

## Phases (approved)

| Phase | Content | Production code | Exit condition |
|---|---|---|---|
| 1 | Fixture T1-T3 established the plain boundary; T4-gz, T4-bz2, T4-xz pinned the compressed defect. | Test fixture | Completed |
| 2 | Source-scoped decoded-prefix matching, preflight/apply atomicity, snapshots, and writer locking. | `stats-core` and `stats-cli` ingestion paths | Completed |
| 3 | Merge and document the contract and reopen requirement. | Documentation and template comments | Completed |

Phase 2 has its own code plan and approval gate before implementation.

## Decisions

| # | Decision | Choice | Alternatives considered | Rationale |
|---|---|---|---|---|
| D-1 | Test location | `../../crates/stats-cli/tests/rotation_boundary.rs` | Unit test in `main.rs`; `stats-core` test | Matches `../../crates/stats-cli/tests/native_report_failure.rs`. Orchestration lives in `main.rs` and is not reachable from `stats-core`. |
| D-2 | Driver | Subprocess invocation of `CARGO_BIN_EXE_stats-cli` | Call `update_inner` directly | `update_inner` and `backfill` are private (`main.rs:784`, `:500`). The subprocess also covers config loading and report regeneration. |
| D-3 | Comparison surface | Observable aggregates only (`04-testing.md`, "Comparison surface") | Rendered HTML; whole SQLite file; checkpoint rows | HTML is not deterministic across regeneration. Checkpoint rows are internal and legitimately differ. Identity is exercised indirectly through the aggregate outcome. |
| D-4 | Compressed rotation before backfill | **Normative:** P+T+N, counted once, for gz, bz2, xz | Assert the defect (rejected); `#[ignore]` (rejected); omit (rejected) | Operators need the documented procedure to be correct regardless of compression. |
| D-5 | Production change | **Checkpoint matching and backfill orchestration are production code.** The fixture is the regression guard. | Fixture-only change (rejected) | The contract must hold for operator workflows, not only in tests. |
| D-6 | Fixture data | Deterministic generator, fixed timestamps, `LogFormat=1` | Captured real logs; random data | Reproducibility. The scratch run used this format and confirmed the expected counts. |
| D-7 | Platform gating | Unix scenarios use the inode-aware path; a `#[cfg(not(unix))] #[test]` prints a skip notice | One `cfg(unix)` gate only (rejected: silent) | Inode-based scenarios do not apply on non-Unix platforms, and the skip must remain visible. |
| D-8 | Documentation | `../operations.md` and the deployment template document the verified contract | Publish an unverified claim (rejected) | Current docs describe behaviour already covered by the fixture and code. |
| D-9 | Compressed-prefix matching (Phase 2) | Source-scoped decoded-prefix matching; preflight-then-apply backfill. See "Matching algorithm" and "Backfill atomicity" below. | Config-scoped matching (rejected: false matches across sources); partial import with per-file commit (rejected: leaves partial state) | Required for the contract; the two refinements were requested in review. |
| D-10 | logrotate reopen | **Documentation requirement, not enforced by the analyzer.** `../operations.md` requires that the web server reopens its log after rotation and before the next update. `deploy/stats-logrotate` gets a deployment-specific placeholder comment, not a hard-coded `systemctl` reload. No real web-server test. | Hard-coded `systemctl reload nginx` (rejected: template is not nginx-specific); real web-server test (deferred until a concrete supported deployment exists) | The analyzer cannot observe which descriptor the web server writes to. The requirement belongs in the operator contract. |

## Matching algorithm (D-9, Phase 2)

### Source scoping
Before the fix, a file matched by identity, raw content hash, decoded full-content hash with the same path, and legacy metadata. Compressed files had no usable prefix match, so their skip count was 0. The current implementation uses the algorithm below for plain and compressed files.

Proposed: every checkpoint carries a **`source_key`**, and every prefix lookup is restricted to checkpoints with the same `source_key`. The `source_key` is:
- the canonical absolute path of the configured `LogFile`, with the file name reduced to the rotation family base (for `access.log`, `access.log.1`, `access.log.2.gz` all map to `access.log`);
- combined with the config hash, as today.

Uniqueness is evaluated **only among checkpoints with the same `source_key`**, never across the whole configuration. Two unrelated logs with identical opening bytes are therefore never confused.

### Prefix matching
1. Each checkpoint stores the decoded prefix's **byte length, hash, and line count** at processing time (for an active file, the whole decoded content at that time).
2. For a candidate (compressed or plain), decode it, then compare its first `decoded_size` bytes against each stored decoded-prefix hash with the same `source_key`, where the candidate has at least the stored line count.
3. **Exactly one** match is accepted. Zero matches for a candidate that is not new content, or more than one match, is an error (see Backfill atomicity).
4. After a match, skip the stored line count, process the remainder, and write a checkpoint for the new identity (`dev:ino`) and path, carrying the same `source_key`.
5. Plain files use the same mechanism, so there is one code path.

### Backfill atomicity (preflight then apply)
Backfill runs in two phases over the whole ordered file list:
1. **Preflight (read-only):** decode every candidate, resolve its checkpoint match (or confirm it is new and unambiguous), and compute the line range each file would contribute. No aggregate or checkpoint table is written. Any ambiguous or unresolvable file aborts here with a diagnostic naming the file and the competing checkpoints.
2. **Apply:** only if preflight passed for every file, process the files oldest to newest in one transaction, updating aggregates and checkpoints together, then regenerate the report.

If apply fails midway, the transaction rolls back, so the store is unchanged. Checkpoint, snapshot, aggregate, and metadata writes are all inside that transaction.

Schema impact: additive checkpoint columns (`source_key`, decoded-prefix size,
hash, and line count) are introduced without bumping the schema version. Legacy
rows remain readable; compressed archives that cannot be matched to them require
the documented rebuild recovery. Configuration fingerprint migration is tested
separately by T12 and T12b.

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| Two unrelated logs share a prefix and match each other | High: wrong skip | `source_key` scoping; uniqueness only within source (D-9) |
| Ambiguous match leaves partial aggregates after a multi-file backfill | High | Preflight over all files before any write; single transaction for apply (D-9). T5 asserts no aggregate change. |
| Upgrade replay double counts | High | Replay clears aggregates; T6 asserts this |
| Inode reuse matches an unrelated checkpoint | Medium | Compressed matches are decided by decoded-prefix hash within `source_key`, not by inode alone |
| Temp directory collisions in parallel tests | Medium | Name directories with process id plus test name; remove at start and end |
| Expected counts drift with parser changes | Low | Expected values come from `P`, `T`, `N`; the clean import is asserted to equal the generated count before comparison |
| Red T4-* merged to master by mistake | Medium | Phase 1 stays on a feature branch until Phase 2 merges |
| Web server keeps writing to the renamed file after rotation | High, operational | Documentation requirement (D-10); not testable by the analyzer |

## Resolved questions
- [x] **`source_key` derivation:** canonical active path plus configured rotation-family base, derived from `LogFile`; no config change is required.
- [ ] **Exit code:** the approved policy is non-zero exit with no import. Confirm the specific non-zero code is not special-cased by any caller (none is known in this repo).
