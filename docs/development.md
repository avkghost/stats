# Development

## Local pre-commit checks

The repository includes a tracked pre-commit hook at `.githooks/pre-commit`.
Enable it once per checkout:

```bash
git config core.hooksPath .githooks
```

The hook checks staged whitespace, Rust formatting, workspace compilation,
Clippy, and all workspace tests with the locked dependency set. It is a fast
correctness gate; release packaging, checksum verification, and dependency
auditing remain part of the release checklist and CI.

To run the hook explicitly without creating a commit:

```bash
git hook run pre-commit
```
