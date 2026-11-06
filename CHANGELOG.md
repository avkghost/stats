# Changelog

## 0.1.2

- Added the GPLv3 license text and explicit AWStats attribution/non-affiliation language.
- Added a compatibility matrix, sanitized runnable fixture, and clearer publication limitations.
- Added all-target workspace checks and `cargo audit` to CI.
- Upgraded Hickory DNS dependencies to 0.26.3 and adapted reverse-DNS lookup handling.
- Corrected README commands so the default fixture workflow runs without private local files.

## 0.1.1

Patch release containing the corrected Linux release bundle contents. Installed bundles no longer include the source-only packaging script.

## 0.1.0

Initial modular AWStats-style Rust analyzer release.

### Included

- AWStats-style configuration and custom, predefined, and JSON log formats.
- SQLite aggregation with content-verified incremental replay and safe schema migration.
- MaxMind GeoIP Country, City, ASN, region, and network-organization enrichment.
- HTTP status, status-class, error, method, URL, bandwidth, request-time, calendar, referrer, search, browser, OS, robot, virtual-host, and authenticated-user dimensions.
- Daily traffic detail with hits, bytes, pages, files, and visits.
- Approximate visits and unique visits with deterministic pseudonymous session keys.
- Entry-page and exit-page session dimensions.
- Multi-page HTML, JSON, stdout-pipeable, and terminal reports.
- Fast HTML overview with Visitors, Pages, Referrers, Geography, and Performance detail pages plus shared CSS/JavaScript assets.
- AWStats-inspired HTML presentation with blue navigation chrome, section bars, responsive layout, and data tables.
- Interactive HTML tabs for Summary, Visitors, Pages, Referrers, Geography, and Performance views.
- Daily traffic bar visualization above the detailed daily table.
- Status-class distribution bars above the status-class table.
- Bandwidth-by-URL bars scaled by response bytes.
- Tokio live analysis and refreshable summaries.
- Ratatui dashboard with Overview, Geography, Clients, and Performance views.
- Rust plugin traits, versioned C ABI, dynamic loading, and supervised native plugin workers; report-renderer callbacks are isolated in short-lived child processes.
- Added frontend regression coverage for controlled CLI, HTTP, and CGI worker failures; HTTP and CGI error responses no longer expose internal rendering details.
- Configuration validation with path-aware errors, LogFormat compilation checks, and first-record parsing.
- Active-path-only log-rotation handling with per-file identity/content checkpoints and legacy checkpoint migration.
- Normal updates now append cumulative data and regenerate the complete multi-page HTML site automatically.
- Explicit `update --log-file` backfill accepts multiple uncompressed, gzip, bzip2, or xz files, checkpoints each file independently, and avoids duplicate reprocessing.
- Added `backfill` to discover numeric plain/gzip/bzip2/xz rotations and process them oldest-to-newest with one final report regeneration.
- Added opt-in monthly/yearly database partitioning through `DatabasePeriod`, with federated HTML, JSON, summary, and TUI reports.
- Added SQLite large-database tuning and an `optimize` command with optional `--vacuum` across all partitions.
- Added read-only `storage` inventory output for control/period database sizes.
- Added non-destructive `backup` snapshots for control and period databases.
- Added explicit `migrate` workflow for backup-protected replay into monthly/yearly partitions.
- Added period-scoped reports with `report --period YYYY-MM` or `report --period YYYY`.

### Verification

- Workspace formatting and Clippy with warnings denied pass.
- 16 workspace tests pass.
- Debug and release workspace builds pass.
- Workspace crate packaging succeeds.
- Supplied access log and GeoIP configuration replay successfully with schema version 11.
