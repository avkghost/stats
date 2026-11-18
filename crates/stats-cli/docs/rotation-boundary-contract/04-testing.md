---
parent: ./README.md
view: quality
---

# Testing Strategy: Rotation Boundary Contract

## Test Rules
- Integration test under `crates/stats-cli/tests/`, using `env!("CARGO_BIN_EXE_stats-cli")`, as in `native_report_failure.rs`.
- Deterministic generated records with fixed timestamps.
- Expected values are computed from `P`, `T`, `N` in the test, not hard-coded.
- Each test uses its own temporary directory, named with process id and test name, removed at start and end.
- Only observable aggregates are asserted. Checkpoint rows are internal and are not read by the test.

## Test Structure
```
crates/stats-cli/tests/
└── rotation_boundary.rs
    ├── fn generate(start, count) -> String            (record generator)
    ├── fn layout(name) -> Layout                       (temp dirs and config)
    ├── fn run_cli(args) -> Output                      (subprocess helper)
    ├── fn observe(db) -> Observation                   (comparison surface, below)
    ├── mod unix (#[cfg(unix)])
    │   ├── T1  plain_rotation_recovers_tail_and_matches_clean_import
    │   ├── T2  normal_update_after_rotation_adds_only_new_active
    │   ├── T3  repeated_backfill_changes_nothing
    │   ├── T4  compressed_rotation_before_backfill_counts_prefix_once   (gz, bz2, xz: three cases)
    │   ├── T5  ambiguous_compressed_match_aborts_backfill_without_writes
    │   ├── T5b unrelated_source_with_same_prefix_is_not_matched
    │   └── T6  upgrade_from_legacy_checkpoints_replays_without_double_count
    └── #[cfg(not(unix))] #[test] skipped_on_non_unix_platforms
```

## Comparison surface

The comparison covers every observable aggregate, not only totals:

| Aggregate | Access through `StatsStore` |
|---|---|
| Totals: hits, pages, bytes, visits | `totals()`, `metric(key)` for each key |
| URLs | `top("urls", ...)` |
| Status codes and status classes | `top("statuses", ...)`, `top("status_classes", ...)` |
| Methods | `top("methods", ...)` |
| Bandwidth per URL | `top_bandwidth(...)` |
| Daily | `daily(...)` |
| Not found | `not_found(...)` |
| Unique visits | `unique_visits()` |

Equality is exact. Row order is taken from the store's own ordering; if an ordering tie makes the comparison unstable, rows are compared as sorted sets.

## Coverage Mapping

### Acceptance criteria
| Criterion | Test |
|---|---|
| 1. Plain rename + backfill equals clean import; prefix counted once | T1 |
| 2. Normal update after rotation adds only new-active records; tail absent | T2 |
| 3. Second backfill changes nothing | T3 |
| 4. gz, bz2, xz rotations before backfill equal clean import | T4 (three cases; failing in Phase 1, green after Phase 2) |
| 5. Ambiguous compressed match: non-zero exit, diagnostic, no aggregate or checkpoint change | T5 |
| 5b. Same-prefix sources with different source_key never match each other | T5b |
| 6. Upgrade from pre-change checkpoints does not double count | T6 |
| 7. Unix-only scenarios; explicit notice elsewhere | module `#[cfg(unix)]` + `#[cfg(not(unix))]` test |
| 8. No state outside temp dir | Layout teardown in every test |

### Behaviour traced to tests
| Behaviour | Source | Test |
|---|---|---|
| Checkpoint kept across plain rename | `main.rs:873` | T1 (outcome only) |
| Prefix skipped by hash, uncompressed | `main.rs:938-943` | T1 |
| Normal update ignores rotations | `main.rs:812` | T2 |
| Backfill order oldest-first | `main.rs:544-581` | T1 |
| Unchanged file no-op | `main.rs:931-937` | T3 |
| Compressed file replay (current defect) | `main.rs:944-945` | T4 |
| Compressed prefix match (Phase 2) | D-9 | T4 green, T5 |
| Schema-change invalidation | `docs/operations.md:84-85` | T6 |

## Test Cases

### T1 - plain_rotation_recovers_tail_and_matches_clean_import
1. Generate P lines into `access.log`; `update`.
2. Append T lines to `access.log`.
3. Rename `access.log` to `access.log.1`.
4. Write N lines to a new `access.log`; `update`.
5. `backfill --log-dir`.
6. Build a clean store: `update --log-file access.log.1` (P+T), then `update --log-file access.log` (N).
7. Assert every aggregate in the comparison surface equals the clean store's, and total hits equals `P+T+N`.

### T2 - normal_update_after_rotation_adds_only_new_active
Steps 1-4 of T1, with no backfill. Assert total hits equals `P + N`. The tail `T` is absent. The prefix `P` is not counted again.

### T3 - repeated_backfill_changes_nothing
T1 through step 5, then run `backfill` again. Assert every aggregate is unchanged.

### T4 - compressed_rotation_before_backfill_counts_prefix_once
Cases `T4-gz`, `T4-bz2`, `T4-xz`. For each:
1. Steps 1-4 of T1.
2. Compress `access.log.1` with the matching codec.
3. `backfill --log-dir`.
4. Assert every aggregate equals the clean import (`P+T+N`, counted once).

**Phase 1 status:** all three cases fail with a mismatch on hits, pages, and bytes. The failure message must show the actual and expected totals. These tests stay on the feature branch until Phase 2 is merged.

### T5 - ambiguous_compressed_match_aborts_backfill_without_writes
1. Steps 1-4 of T1.
2. Create two compressed copies of the same decoded content (`access.log.1.gz` and `access.log.2.gz`), both matching the stored prefix, alongside the existing `access.log.1` family.
3. Snapshot every aggregate (comparison surface) and the checkpoint row count.
4. `backfill --log-dir`.
5. Assert all of:
   - exit status is non-zero;
   - stderr names the ambiguous file and both competing candidates, and identifies the source (`source_key`);
   - every aggregate and the checkpoint row count equal the snapshot taken in step 3 (preflight writes nothing, so no partial state remains).
6. Control case in the same test: a second backfill over an unambiguous set (duplicate removed) then succeeds, showing the failure was the ambiguity, not a broken store.

### T5b - unrelated_source_with_same_prefix_is_not_matched
Two different log families in separate config directories (different `source_key`) whose first records are identical. Backfill for one must not skip lines using the other's checkpoint. Assert that the aggregates equal the clean import for each family independently.

### T6 - upgrade_from_legacy_checkpoints_replays_without_double_count
1. Build a store with the pre-change checkpoint schema (fixture: a copy of the current format).
2. Run the upgraded binary `update`.
3. Assert the schema change invalidates checkpoints and triggers a replay that clears aggregates first, so the result equals a clean import of the same log, not the sum.

### T-non-unix - skipped_on_non_unix_platforms
`#[cfg(not(unix))]` test that prints `rotation_boundary: skipped, inode identity unavailable on this platform` and passes. Exists so the skip is visible in test output.

## Deferred (not in this feature)
- Operational test with a real web server that reopens its log after rotation (D-10).
- Missing new active file at normal update (error path).
- Prefix mismatch after an in-place edit of the active file.
- Multi-period (`DatabasePeriod`) stores.

## Test Count Summary

| Module | Tests | Phase 1 status | Phase 2 status |
|---|---|---|---|
| rotation_boundary.rs (unix) | T1, T2, T3 | pass | pass |
| | T4-gz, T4-bz2, T4-xz | fail (pins defect) | pass |
| | T5, T5b, T6 | not written (Phase 2) | pass |
| rotation_boundary.rs (non-unix) | skip notice | pass | pass |
| **TOTAL** | **10** | | |
