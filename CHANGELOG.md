# Changelog

## Unreleased

- Relative `DirOutput` paths are resolved from the configuration file directory.
- Installer smoke tests now run from the core release archive, and release validation reuses the CI workflow.
- Logrotate renders the configured `STATS_GROUP` instead of assuming the `stats` group.
- HTTP and CGI frontends return client, unsupported-feature, and server failures with distinct status codes while keeping response bodies generic.
- CSV exports neutralize spreadsheet formula prefixes and include host and referer-spam dimensions.
- Filter regular expressions are validated when configuration is loaded.
- Store internals now separate schema, classification, aggregate, checkpoint, and session concerns; aggregate updates reuse cached SQLite statements.
- Added regression coverage for TUI rendering through ratatui's test backend and retained full rotation-boundary coverage.

## 0.1.10

- Human-facing HTML and TUI reports now use compact, readable byte and count units while CLI, CSV, and JSON values remain exact.
- Human-readable values round at display precision before choosing a unit, so values near a boundary show as `1.00 MB` or `1.00M` instead of `1024 KB` or `1000K`.
- The systemd update service now has a five-minute startup deadline.
- The supplied logrotate template creates the active log with the `stats` reader group, so the scheduled service can read it after rotation.
- Deployment documentation describes the required log ownership and group access.
- The installer copies the bundled example configuration on a fresh install without overwriting an existing configuration.
- Quoted HTTP request lines now normalize methods such as `GET` and `POST` without retaining the opening quote.
- Nginx quoted referer and User-Agent fields are parsed as separate fields instead of absorbing surrounding quotes or trailing values.
- Fresh installs leave the periodic timer disabled until the reviewed configuration is explicitly enabled.
- The installed systemd service now uses the configured binary, config, data, and output paths instead of fixed example paths.

## 0.1.9

- Release archive checksum files now use relocatable archive names, so CI and users can verify them from any extracted artifact directory.

## 0.1.8

- Release bundles now include the native `stats-report-worker` beside every frontend that can render native plugins.
- Packaging produces separate core, HTTP, CGI, and plugin archives with SHA-256 sidecars and content verification.
- The installer deploys the worker together with the main `stats` binary, and CI validates release archive contents before publishing.
- Native worker integration tests locate the built worker reliably in clean workspace builds.
- Added a tracked pre-commit hook covering formatting, compilation, Clippy, and the complete workspace test suite.

## 0.1.7

- Native report plugins run in a separate `stats-report-worker` binary, which must be installed next to `stats-cgi`, `stats-http`, and `stats-cli`. Frontends no longer re-enter their own argument parsing as a worker, so a CGI request cannot reach worker mode through arguments.
- The plugin supervisor writes the request off its deadline path. A plugin worker that never reads stdin now times out after the configured limit instead of blocking the frontend.
- On Unix, native plugin workers run in their own process group, and a timeout or failed response kills the whole group. Descendants that inherit the worker's stdin can no longer hold the supervisor past its timeout. Processes that leave the group are not covered, and on other platforms only the direct worker is killed.

## 0.1.6

- Report metadata now records the processed active log: `input_path`, `input_size`, and `input_lines` are populated for unpartitioned stores again.
- Storage reads (`totals`, `metric`, `unique_visits`, `meta`) return errors on database failures instead of reporting zero or an absent value.
- `clear_aggregates` now also clears scanner classifications, and a schema replay clears stored prefix snapshots.

## 0.1.5

- Replaced unmaintained `chrono` and `chrono-tz` with maintained `jiff`, preserving fixed-offset and IANA timezone handling, including DST-aware conversion.
- Existing schema 13 databases keep their aggregates and checkpoints; older schemas retain the existing replay-on-open behavior.

## 0.1.4

- Updated bundled SQLite through `rusqlite` to the patched `libsqlite3-sys` release, addressing RUSTSEC-2026-0459.

## 0.1.3

- Rotations are counted correctly: a plain, gzip, bzip2, or xz archive that holds records already counted from the active log is no longer counted twice by `backfill`. Records written after the last update and before rotation (the unread tail) are counted once.
- Unpartitioned `update` and `backfill` are atomic: a failure leaves the database as it was. `backfill` checks the whole set before writing and rejects a set with identical or contained overlapping files, with a non-zero exit.
- copytruncate: the superseded content of a truncated active log is kept as a snapshot, so the copied archive is not counted again.
- Checkpoint config fingerprints are SHA-256. The earlier hash is renamed in place on the first update after upgrading, without clearing aggregates. Only a changed configuration clears them.
- Writers take an exclusive lock on `DirData/stats.lock`. A command that finds it held exits without writing. `watch`, `live`, and `tui` hold it per refresh.
- Only newline-terminated lines of the active log are counted. A partly written line is counted once, when it completes.
- Limits: `backfill` refuses partitioned stores (`migrate` still replays them). Partitioned `update` keeps the per-event path, without the new atomicity. Databases written before checkpoint support need the rebuild recovery described in `docs/operations.md` before compressed archives can be backfilled.

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
