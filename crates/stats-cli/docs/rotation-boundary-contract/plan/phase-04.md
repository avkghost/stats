phase: 04
name: Matching algorithm and preflight plan
layer: domain (matching, pure) + usecase (preflight)
depends_on: [phase-01, phase-02, phase-03]
plan: ./README.md

---
# Phase 04: Matching and preflight

## Goal
Decide, for every file in a backfill (or the active file in a normal update), what to skip, what to import, what to supersede, and when to abort, **before any write**.

## Context
Current decision logic in `update_inner` (`crates/stats-cli/src/main.rs:871-945`):
1. Identity match (`dev:ino` + config hash), `main.rs:873`.
2. Raw content hash, path match, `main.rs:875-880`.
3. Logical (decoded) content hash, path match, `main.rs:882-887`.
4. Legacy meta match, uncompressed only, `main.rs:889-909`.
5. Legacy prefix match by raw bytes, uncompressed only, `main.rs:910-930`.
6. `skip_lines` (`main.rs:931-945`): all lines if logical hash equals, prefix lines if an uncompressed prefix hash matches, otherwise 0.

Compressed candidates never reach a prefix match, so `skip_lines` is 0 (the T4 defect).

## Matching algorithm

### Inputs
- `source_key`, `config_hash` (both from phase 03 and the base config).
- Candidate file: `identity`, `path`, `compressed` flag, decoded bytes `D` (length `d`).
- Stored states for `(source_key, config_hash)`: current rows with stable fields, plus snapshot rows (phase 01).

### Rule set (evaluated in this order)

**R0 - exact current match (unchanged behaviour).** If the identity row exists and its logical content hash equals the candidate's, the file is fully counted: `Skip(all)`. Same for the raw-content and logical-content lookups (`main.rs:875-887`). These keep today's semantics for repeated unchanged invocations.

**R1 - stable prefix match.** For each stable state `S` (current or snapshot) with `S.size <= d`, compute `sha256(D[..S.size])` (memoized per distinct size) and compare with `S.sha256`. A match means the candidate begins with the state, i.e. the state's content was already counted.
- `S.size == d` is a full match: the candidate equals a retained state exactly. Outcome `Skip(all)`. This is how a copytruncate archive matches its snapshot (see phase 05).
- `S.size < d`: outcome `Skip(S.lines)` and import `D[lines_of_S..]`.

**R2 - resolution.** Let `M` be the set of matching states.
- `|M| == 0`:
  - If the candidate is **compressed** and the source has legacy rows (`legacy_checkpoint_count > 0`): `Abort(Unresolvable)`. Overlap with those legacy rows cannot be excluded (decision D).
  - Otherwise: `New`, import all lines (skip 0). This is today's behaviour for a new file.
- `|M| == 1`: `Skip(S.lines)` (or `Skip(all)` for the full-match case).
- `|M| >= 2`: `Abort(Ambiguous)`.

**R3 - legacy fallback, plain files only.** Before R2 resolves a plain candidate with `|M| == 0`, apply the existing legacy branches (steps 4-6 above) unchanged. A plain candidate that matches a legacy row keeps today's skip count and is upgraded when it is written (phase 05 writes stable fields for every processed file).

**R4 - supersession.** When the candidate's identity already has a current row `R` that R1 did not match (the file was replaced or truncated under the same inode), the plan records `snapshot_before = R` (see phase 05). The candidate is then `New` for its own content.

### Within-run checks (preflight only)
- Candidates in one backfill must not overlap each other. Two candidates overlap if one's full decoded content is a prefix of the other's, using the same `sha256` comparison. Overlap means the same stream appears twice (for example `access.log.1.gz` and `access.log.2.gz` are copies). Outcome: `Abort(AmbiguousWithinRun)`.
- Candidates are processed oldest to newest. Preflight evaluates every candidate against the **pre-run** state only. Because overlap within the run aborts, no decision depends on another file's outcome in the same run.

### Pure function (`crates/stats-core/src/checkpoint_match.rs`, new)

```text
pub struct StateRef { pub identity: String, pub size: u64, pub lines: u64, pub sha256: String, pub kind: StateKind }  // Current | Snapshot
pub enum Decision { Skip { lines: u64, full: bool, matched: Vec<StateRef> }, New, Abort(Reason) }
pub enum Reason { Ambiguous { candidates: Vec<StateRef> }, Unresolvable, AmbiguousWithinRun { other: String } }

pub fn decide(candidate_len: u64, legacy_rows_present: bool, compressed: bool,
              states: &[StateRef], prefix_hash: impl FnMut(u64) -> String) -> Decision
```
No I/O. The closure computes the prefix hash for a size, so the function is testable with synthetic data.

## Preflight (`crates/stats-cli/src/main.rs`, new function `plan_sources`)

Inputs: the ordered file list (for backfill: `discover_backfill_files` output; for update: the single active file), the open `Batch` from phase 02 (reads only), the source key, the config hash.

For each file, oldest to newest (decision J: streaming, bounded memory):
1. **Bound the input.** For the active file, record its current size `S` and read only the first `S` bytes; later appends belong to the next run. For archives, the whole file.
2. **Stream and digest.** Decode through `GzDecoder` / `BzDecoder` / `XzDecoder` (or plain bytes), feeding a `PrefixDigest`. Record: decoded length `d`, the digest of the prefix at each state size needed by matching (sizes come from the stored states, read before streaming), `full_digest`, and the complete-line count (decision I: for the active file, only bytes through the last `\n`; for archives, EOF terminates the last line).
3. **Any decode error or I/O error aborts the whole plan** (a corrupt archive fails before any write; today it fails after earlier files were committed).
4. Compute the decision through `decide` using the digests from step 2.
5. Record the `FilePlan`: identity, path, the byte bound (`S` for the active file, the full decoded length for archives), `verified_digest` (SHA-256 of `[0, bound)`), action (`Skip { lines }` or `Import { skip_lines }`), `replaces: Option<identity>`, `snapshot_before: Option<PrefixState>`, and `new_state` with the complete-line fields:
   - `prefix_size` = `complete_prefix_size` (position after the last `\n` within the bound; the full length for archives, per the EOF rule);
   - `prefix_lines` = lines through that position;
   - `prefix_sha256` = SHA-256 of `decoded[0..prefix_size]`.
   A trailing fragment inside the bound is therefore never in the checkpoint.
6. Check the within-run overlap rule against files already in the plan.

**Apply re-verifies (decision J).** Apply streams each file again, reading only `[0, bound)`, and before `COMMIT` checks that the digest of that range equals `verified_digest`. For the active file the bound is `S`, so bytes appended after `S` are not part of the check and are not an error. For archives the bound is the whole file. A difference inside the range (the file was replaced, truncated, or altered) returns an error inside the transaction, so all earlier files roll back with it. Verification happens before commit, so a change between preflight and apply cannot produce a partial commit.

Also compute `reset: bool`: true when the stored `config_hash` differs from the current one (`main.rs:52-59`). `reset` is applied inside the apply transaction, not here.

Output: `Plan { reset, files: Vec<FilePlan> }`, or an error with a diagnostic per abort reason.

## Diagnostics and exit (decision H)

Each abort produces one error that names the source, the file, and the reason. Examples:
- Ambiguous: `backfill aborted: {file} matches {n} stored states for source {source_key} (identities {a}, {b}); no changes were written`
- Unresolvable: `backfill aborted: compressed archive {file} cannot be checked against legacy checkpoints for source {source_key}; see the recovery procedure in docs/operations.md; no changes were written`
- Within run: `backfill aborted: {file} and {other} contain the same records; remove one of them; no changes were written`

The command exits non-zero (`anyhow` error). Nothing is written, because the plan is built before the apply transaction starts (phase 05).

## Files to Modify
- `crates/stats-core/src/checkpoint_match.rs` (new), `crates/stats-core/src/lib.rs` (export)
- `crates/stats-cli/src/main.rs`: new `plan_sources`; old decision block (`:871-945`) is replaced by a call to `decide` for stable cases and retained as the R3 legacy branch

## Tests

| Test | Verifies |
|---|---|
| `decide_new_when_no_states_and_no_legacy` | `New`, skip 0. |
| `decide_unique_prefix_match_skips_state_lines` | One state matches; `Skip(lines)`, not full. |
| `decide_full_match_skips_everything` | Candidate equal to a state (`size == d`); `Skip(all)`. |
| `decide_ambiguous_when_two_states_match` | Two states with the same sha256 and size (duplicate snapshots from different identities) gives `Ambiguous`. |
| `decide_compressed_with_legacy_rows_is_unresolvable` | `compressed = true`, `legacy_rows_present = true`, no match gives `Unresolvable`. |
| `decide_plain_with_legacy_rows_falls_through_to_new` | `compressed = false`, no match, legacy rows gives `New` (R3 handles legacy). |
| `decide_ignores_states_longer_than_candidate` | State larger than candidate does not match (no false containment). |
| `decide_hash_is_keyed_by_size` | Same sha256 at a different size does not match. |
| `within_run_overlap_detected` | Two candidates where one is a prefix of the other give `AmbiguousWithinRun`. |
| `plan_aborts_before_any_write_on_corrupt_archive` (integration, phase 05 test file) | Corrupt `.gz` in the file list aborts; store unchanged (see T10 in phase-05). |

## Verification
- `cargo test -p stats-core checkpoint_match` passes all decision-table tests.
- Existing T1-T3 still pass.
- T4-* may begin passing at this point only if apply (phase 05) is also in place; at the end of phase 04 alone they still fail, because `plan_sources` is not yet applied.
