---
date: 2026-11-18
feature: rotation-boundary-contract
service: crates/stats-cli
status: complete (implemented in c119d87; documentation in Phase 3 done)
research: ../../../../.thoughts/research/2026-11-18-rotation-unread-tail.md
---

# Rotation Boundary Contract - Design Documents

## Business Context

The updater deliberately reads only the configured active log during a normal `update`. Records written to the active file after the last checkpoint and before a rotation therefore sit in the renamed file, and only the documented `backfill` procedure reads them. Today this boundary is described in prose (`docs/operations.md:12-15`, `:20-26`, `:43-45`) and verified only by a unit test of filename discovery. Administrators must infer from the prose whether a given rotation setup preserves the unread tail without recounting the prefix.

The scratch run (research document) shows the documented procedure is correct for uncompressed rotations and double-counts the prefix when the rotated file is compressed. Operators whose logrotate setup compresses before backfill get inflated `pages` and `bytes`, with no error.

This feature makes the boundary an executable contract and then closes the gap.

## Normative Contract

After rotation to a plain, gzip, bzip2, or xz archive, followed by the documented `backfill`, aggregates equal a clean import of the same records, with the prefix counted exactly once.

## Scope

Three phases (details in `03-decisions.md`):
1. **Fixture:** T1-T3 pass. T4 for gz, bz2, and xz is added and fails, pinning the current defect. Kept on a feature branch while red.
2. **Production fix:** compressed-prefix matching in checkpoint lookup (`crates/stats-cli/src/main.rs:871-945`, checkpoint schema in `crates/stats-core/src/store.rs`), per D-9.
3. **Merge and document:** T4 green, T5 and T6 added, `docs/operations.md` updated.

Out of scope: whether the web server keeps writing to the renamed file through its open descriptor (D-10). The contract covers the analyzer only.

## Acceptance Criteria
1. After `update`, a plain rename, creation of a new active file, a normal `update`, and `backfill --log-dir` produce aggregates identical to a clean import, with the prefix counted once.
2. Between the rename and `backfill`, a normal `update` adds only the new active file's records. The unread tail is absent and the prefix is not counted again.
3. A second `backfill` over unchanged files changes no aggregate.
4. After a gzip, bzip2, or xz rotation and backfill, aggregates equal a clean import (P+T+N, counted once). Fails in Phase 1, passes after Phase 2.
5. An ambiguous compressed match is diagnosed and does not import silently.
6. Upgrading from pre-change checkpoints does not double count.
7. Unix runs the scenarios. A `cfg(not(unix))` test prints an explicit skip notice, because file identity is not inode-based there (`crates/stats-cli/src/main.rs:1170-1177`).
8. The fixture leaves no state outside its temporary directory.

## Documents

| File | View | Description |
|---|---|---|
| [01-architecture.md](./01-architecture.md) | Logical | C4 L1-L3 for the fixture, the binary, and the data it touches |
| [02-behavior.md](./02-behavior.md) | Process | Data flow and sequence per scenario, including the Phase 2 target flow |
| [03-decisions.md](./03-decisions.md) | Decision | Normative contract, phases, decisions D-1 to D-10, matching algorithm, risks, open questions |
| [04-testing.md](./04-testing.md) | Quality | Comparison surface, test cases T1-T6, acceptance coverage |

Conditional files 05-08 are not created. The feature has no domain events, no repository model, no REST endpoints, and no new persistence beyond the checkpoint columns in Phase 2. The `/prompts/` standards referenced by the template do not exist in this repository, so the standards matrix (07) is not applicable.

Research: [`.thoughts/research/2026-11-18-rotation-unread-tail.md`](../../../../.thoughts/research/2026-11-18-rotation-unread-tail.md) (includes the scratch experiment results).
