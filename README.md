# Stats

Linux-first modular web log analyzer in Rust, with AWStats-style configuration and local MaxMind GeoIP enrichment.

See [CHANGELOG.md](CHANGELOG.md) for the current `0.1.1` release scope and verification summary.

## Current implementation

- Cargo workspace with `stats-core` and `stats-cli`.
- AWStats-style `Key=Value` configuration parsing.
- Apache/Nginx custom format support, common LogFormat 4, JSON-line records, Unix timestamps, and AWStats time formats 1–6. Apache aliases include request tokens (`%r`, `%m`, `%U`, `%q`, `%H`), client/vhost fields (`%a`, `%{c}a`, `%v`, `%{Host}i`), epoch timestamps (`%{sec}t`, `%{msec}t`, `%{usec}t`), duration tokens (`%D`, `%T`, `%{ms}T`), and arbitrary quoted headers.
- Optional `TimeZone=<hour-offset>` normalizes parsed event timestamps before aggregation; leaving it unset preserves the timestamp offset from each record.
- MaxMind Country, City, and ASN MMDB providers.
- GeoIP lookup safeguards for private, loopback, unspecified, multicast, and scope-suffixed IPv6 addresses.
- SQLite aggregate store with append checkpointing.
- Optional monthly/yearly SQLite partitioning with federated reports across all period databases.
- Static multi-page HTML or machine-readable JSON reports with AWStats-style hits/pages/files/visits totals, daily traffic detail, calendar dimensions, top bandwidth URLs, virtual-host reports, authenticated users, HTTP errors and status classes, request-time buckets, referrer URLs and referring sites, search engines/keyphrases, incremental session entry/exit pages, and status, country, region, city, ASN, network organization, file-type, browser, OS, and robot tables.
- Rust plugin traits for event enrichment plus a C ABI and dynamic host loader.
- Tokio CLI runtime with blocking analysis work isolated from the runtime.
- Ratatui interactive dashboard backed by the shared report query API.
- Stable terminal summaries with one-shot and refreshable views.
- Live terminal analysis that incrementally updates the log and redraws the summary.
- Read-only `stats-cgi` adapter for AWStats-style CGI hosting; it renders an existing database and never updates logs or checkpoints.
- Standalone Tokio/Axum HTTP frontend with HTML reports, HTML section routes, and configured JSON/CSV renderer plugins.
- AWStats-compatible `urlalias` display plugin support using `DirData/urlalias.<SiteDomain>.txt` or `DirData/urlalias.txt`; stored and machine-readable URL keys remain canonical.

## Run against the example fixture

The generic example configuration is at `examples/awstats.example.test.conf`. Copy it and adjust `LogFile`, `DirData`, and any local GeoIP plugin paths for a deployment.

    cargo run -p stats-cli -- update --config logs/awstats.example.test.conf --log-file logs/access.log --data-dir target/stats-data
    cargo run -p stats-cli -- backfill --config logs/awstats.example.test.conf --data-dir target/stats-data
    cargo run -p stats-cli -- rebuild --config logs/awstats.example.test.conf --log-file logs/access.log --data-dir target/stats-data
    cargo run -p stats-cli -- report --data-dir target/stats-data --output target/stats-report/index.html
    cargo run -p stats-cli -- report --data-dir target/stats-data --period 2026-10 --output target/stats-october/index.html
    cargo run -p stats-cli -- optimize --data-dir target/stats-data
    cargo run -p stats-cli -- optimize --data-dir target/stats-data --vacuum
    cargo run -p stats-cli -- storage --data-dir target/stats-data
    cargo run -p stats-cli -- backup --data-dir target/stats-data --output-dir target/stats-backup
    cargo run -p stats-cli -- retention --data-dir stats --before 2025-01
    cargo run -p stats-cli -- retention --data-dir stats --before 2025-01 --archive-dir target/stats-archive --remove-source
    cargo run -p stats-cli -- migrate --config logs/awstats.example.test.conf --log-dir logs --data-dir stats --backup-dir target/pre-partition-backup
    cargo run -p stats-cli -- report --config logs/awstats.example.test.conf --data-dir target/stats-data --output target/stats-report.json --format json
    cargo run -p stats-cli -- report --config logs/awstats.example.test.conf --data-dir target/stats-data --output target/stats-report.csv --format csv
    cargo run -p stats-cli -- report --config logs/awstats.example.test.conf --data-dir target/stats-data --output - --format json | jq '.hits, .pages'
    cargo run -p stats-cli -- watch --config logs/awstats.example.test.conf --log-file logs/access.log --data-dir target/stats-data
    cargo run -p stats-cli -- validate --config logs/awstats.example.test.conf
    cargo run -p stats-cli -- health --config logs/awstats.example.test.conf --data-dir stats
    cargo run -p stats-cli -- summary --data-dir target/stats-data --limit 10
    cargo run -p stats-cli -- summary --data-dir target/stats-data --watch --interval 5
    cargo run -p stats-cli -- live --config logs/awstats.example.test.conf --limit 10 --interval 5
    cargo run -p stats-cli -- tui --config logs/awstats.example.test.conf --limit 10 --interval 5
    cargo run -p stats-http -- --data-dir target/stats-data --bind 127.0.0.1:3000
    curl 'http://127.0.0.1:3000/report?output=html&limit=50'
    curl 'http://127.0.0.1:3000/section/visitors?limit=50'
    curl 'http://127.0.0.1:3000/report?output=json&limit=50' # requires LoadPlugin=report_json
    QUERY_STRING='output=json&limit=50' STATS_CONFIG=logs/awstats.example.test.conf STATS_DATA_DIR=stats cargo run -p stats-cgi
    QUERY_STRING='section=visitors&limit=50' STATS_DATA_DIR=stats cargo run -p stats-cgi

The configured GeoIP plugin paths are read from `LoadPlugin` entries. CLI path overrides are intended for local fixtures and tests.

For large retention periods, add `DatabasePeriod=monthly` or `DatabasePeriod=yearly` to the configuration. The control database remains at `DirData/stats.sqlite3`; event aggregates are stored under `DirData/periods/YYYY-MM/stats.sqlite3` or `DirData/periods/YYYY/stats.sqlite3`. Reports, summaries, and the TUI merge all period databases automatically, while shared control session state preserves continuity across boundaries. Late or out-of-order input remains approximate. The default is `DatabasePeriod=none`, which preserves the single-database layout.

SQLite uses WAL mode, a five-second busy timeout, normal synchronous durability, and in-memory temporary storage. Run `optimize` after large ingestion batches; add `--vacuum` during maintenance windows when reclaiming deleted space is worth the exclusive rewrite cost.

Partition retention is explicit and safe by default. `retention --before YYYY-MM` or `--before YYYY` lists matching old partitions without changing them. Add `--archive-dir` to create consistent SQLite snapshots under `archive-dir/periods/...`; add `--remove-source` only after verifying the archive, which removes the original partition directories after all snapshots succeed. The archive directory must be separate from the live data directory.

Use `backup` for consistent snapshots of the control and period databases. The destination must not already exist; the command uses SQLite's online `VACUUM INTO` snapshot mechanism and preserves the partition directory layout. `storage` includes SQLite `-wal` and `-shm` sidecars in its totals.

To migrate an existing single database, first add `DatabasePeriod=monthly` or `DatabasePeriod=yearly` to the configuration, then run `migrate`. It requires a new backup directory, backs up the current data, clears active aggregates, and replays every discovered log. Keep the backup until the new totals are verified.

Updates verify the source file identity and content hash before using the append checkpoint. Rewrites, truncation, replacement, or changed prefixes trigger a safe aggregate replay. Use `rebuild` to explicitly clear derived aggregates and replay the source from scratch. `validate` reports directives that are currently ignored.

The updater analyzes only the configured active file. It does not open `access.log.1`, `access.log.2`, or compressed archives. Per-file identity/content checkpoints recognize when the active file is renamed, replaced, truncated, or recreated, so normal rotation preserves already-counted history and processes only new lines from the active path.

Historical files are supported when supplied explicitly. Run `update` once per file, oldest to newest, without using `rebuild` between files; each invocation appends unseen events to the same database and regenerates the complete cumulative site. `.gz`, `.bz2`, and `.xz` files are decompressed for parsing. For example:

    cargo run -p stats-cli -- update --config logs/awstats.example.test.conf --log-file logs/access.log.2.gz --data-dir stats
    cargo run -p stats-cli -- update --config logs/awstats.example.test.conf --log-file logs/access.log.1 --data-dir stats
    cargo run -p stats-cli -- update --config logs/awstats.example.test.conf --log-file logs/access.log --data-dir stats

The per-file checkpoints make a repeated invocation unchanged and prevent duplicate ingestion. Use `rebuild` only when intentionally resetting the database and replaying one selected file; it is not the multi-file backfill command.

For a conventional rotation directory, `backfill` discovers the configured active filename plus numeric rotations such as `.1`, `.2.gz`, `.3.bz2`, and `.4.xz`, processes them oldest-to-newest, and regenerates the site once at the end. Normal `update` still processes only the configured active file.

Recommended cron/logrotate order is: run `stats update` first, generate the report, rotate the log, create the new active log, and run `stats update` again when new lines arrive. The first update checkpoints the pre-rotation active file; the later update sees the replacement at the configured active path and processes only new active-log lines. Existing aggregates are not deleted during normal rotation handling.

A safe `logrotate` `postrotate` hook can run the analyzer after rotation without scanning archives:

    /usr/local/bin/stats update --config /etc/stats/awstats.example.test.conf

For a timer-driven deployment, run the same command before rotation and let the next timer invocation process the replacement active file:

    # /etc/cron.d/stats-update
    */5 * * * * stats /usr/local/bin/stats update --config /etc/stats/awstats.example.test.conf >/var/log/stats-update.log 2>&1

Do not call `backfill` from the recurring rotation hook: it intentionally reopens numeric archives. Use `backfill` only for an explicit historical import after confirming that the archive set is complete. If a migration is interrupted, keep the backup and rerun `migrate` with a different backup directory only after checking the destination data; the command refuses to overwrite an existing backup directory.

Deployment templates are in `deploy/`: `stats-update.service`, `stats-update.timer`, and `stats-logrotate`. Install the service and timer under `/etc/systemd/system/`, install the logrotate file under `/etc/logrotate.d/stats`, then adjust `/usr/local/bin/stats`, `/etc/stats/awstats.example.test.conf`, `/var/lib/stats`, `/var/www/stats`, and the log owner/group to match the host. The `stats` service account needs read access to the active log and GeoIP databases and write access to the data/output directories.

After building a release, `sudo deploy/install.sh` performs the repeatable installation. It creates the system account and data directories when needed, installs the release binary and service files, reloads systemd, and enables the timer. It intentionally does not run an update until the configuration has been installed. Override paths with `STATS_BINARY_PATH`, `STATS_CONFIG_PATH`, `STATS_DATA_DIR`, and `STATS_OUTPUT_DIR` when the host layout differs.

Create a self-contained Linux release bundle with `deploy/package-release.sh`. It produces `target/release-artifacts/stats-0.1.1-linux-x86_64.tar.gz` and a `.sha256` checksum containing the optimized binary, deployment templates, README, and changelog. Set `STATS_VERSION`, `STATS_PLATFORM`, or `STATS_RELEASE_DIR` to customize the artifact name or destination.

The release workflow publishes the same bundle when a `v*` tag is pushed. For the first release, verify CI is green, create the tag, push it, then verify the attached checksum before installation:

    git tag v0.1.1
    git push origin v0.1.1

Typical installation and upgrade sequence:

    install -m 0755 target/release/stats-cli /usr/local/bin/stats
    install -d -o stats -g stats /etc/stats /var/lib/stats /var/www/stats
    install -m 0644 deploy/stats-update.service deploy/stats-update.timer /etc/systemd/system/
    install -m 0644 deploy/stats-logrotate /etc/logrotate.d/stats
    systemctl daemon-reload
    systemctl enable --now stats-update.timer
    systemctl start --wait stats-update.service

Before replacing a binary or changing partition settings, run `stats backup --data-dir /var/lib/stats --output-dir /var/backups/stats/<timestamp>` and retain the backup. To roll back, stop the timer, restore the binary/configuration, validate with `stats validate`, restore the database snapshot into a separate recovery directory first, and switch it into place only after checking `stats summary`; then start the timer again. Never use `migrate` as a rollback operation because it clears active aggregates after creating its backup.

Trusted native enrichment plugins can be configured with a `.so` path, for example `LoadPlugin="my_enricher /opt/stats/libmy_enricher.so"`. They receive the normalized client IP, method, URL, status, and byte count and may update status or bytes. Native plugins run in supervised worker processes with a bounded response wait; failures disable the plugin for the current update and are recorded as diagnostics.

`update` is cumulative by default: it appends new active-log events to the existing database and regenerates the complete HTML site at the data directory's sibling `output/index.html` (for `DirData="stats"`, that is `output/index.html`). `report` can generate the site at a custom output path such as `target/stats-report/index.html`; the overview is intentionally compact so it loads quickly, while detail pages contain the complete tables and visualizations. The static multi-page site remains the default HTML path. Stream HTML, JSON, and CSV are external report-renderer plugins and are loaded only when requested and configured with `--config`.

`summary` prints a script-friendly terminal overview. Add `--watch` to refresh an existing database without rereading the log. `live` combines incremental updates with the refreshable summary and exits cleanly on Ctrl-C. `tui` provides the interactive Ratatui dashboard: press `1` for Overview, `2` for Geography, `3` for Clients, `4` for Performance, `r` to refresh immediately, and `q`, `Esc`, or `Ctrl-C` to exit. The TUI uses its dedicated terminal renderer and does not load report-export plugins. Both `live` and `tui` accept `--log-file` and `--data-dir` overrides for deployments where the config paths are not directly usable.

`health` is a read-only deployment check. It verifies the configured active log, plugin resource paths, and every existing control/period database, then prints database totals. It never creates a missing database; a fresh deployment should use `update` first. Add `--json` for monitoring integrations; the command still exits nonzero when any check fails.

Reports include source/configuration fingerprints, schema metadata, entry/exit page dimensions, and explicit approximation notes so downstream consumers can distinguish measured values from heuristic visitor, session, and bot metrics. Schema changes invalidate stale aggregate checkpoints and trigger a safe replay.

Visit and unique-visit keys are deterministic pseudonymous FNV-1a aggregation identifiers. Raw client IP addresses are not stored, but these keys are not cryptographic anonymization and should be protected as derived analytics data.

Page totals exclude common static-file extensions by default. Set `NotPageList` in the AWStats configuration to replace that list explicitly.

If the AWStats config points at production paths such as `/var/log/nginx/access.log` and `/var/lib/awstats`, run as the service account with those permissions, or override both paths explicitly for a local run.

## Architecture direction

`stats-core` owns normalized events, parsing, GeoIP providers, plugin traits, storage, and report queries. `stats-cli` owns orchestration and output. HTML, terminal summaries, and the Ratatui dashboard consume the same report query API rather than accessing storage directly.

The current plugin implementation includes an in-process Rust trait registry and a C ABI loader used by the supervised worker process. Plugins can contribute event enrichers or report renderers. A configured `LoadPlugin="report_html /opt/stats/libstats-report-html.so"`, `LoadPlugin="report_json /opt/stats/libstats-report-json.so"`, or `LoadPlugin="report_csv /opt/stats/libstats-report-csv.so"` entry loads only the requested stream renderer. Native report plugins receive a format and UTF-8 JSON report, then return a media type, extension, and body through an explicit release callback. Dynamic libraries remain trusted code from a policy perspective, but a plugin crash is isolated from the analyzer process.

The CGI adapter uses the same rule: section-specific HTML remains available from the static report templates, while full HTML, JSON, and CSV output requires `STATS_CONFIG` with the requested renderer plugin. The TUI uses its own terminal renderer and never loads report-export plugins.

For CGI and HTTP deployments, the frontend applies an implicit `open_basedir`-style read boundary: the effective `DirData` directory is the only request-readable root. Requested database and `urlalias` files are canonicalized before access, so traversal and symlink escapes are rejected. External renderer libraries are trusted startup inputs from deployment configuration and are not request-controlled.

The standalone HTTP frontend also provides a disabled-by-default diagnostic endpoint at `/raw-log`; CGI supports the equivalent `output=raw-log` request. Set a non-empty `STATS_RAW_LOG_AUTH_TOKEN` in the service environment and have the reverse proxy inject `Authorization: Bearer <token>`. Both paths return only redacted records, bound requests to 10,000 lines, support optional line ranges, and read the configured log only when it is inside `DirData`.

Build the supplied external renderers separately with `cargo build -p stats-report-html -p stats-report-json -p stats-report-csv`. Install the resulting shared libraries in a trusted plugin directory, then reference the selected library from `LoadPlugin`; the main analyzer binaries do not contain the stream renderers.
