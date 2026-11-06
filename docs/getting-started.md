# Getting started

## Requirements

Use a current stable Rust toolchain. The repository is a Cargo workspace with
12 packages. Optional GeoIP enrichment requires local MaxMind MMDB files; the
public fixture does not require them.

## Build and test

From the repository root:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

## Anonymized example

The generic configuration and anonymized access log are
`examples/awstats.example.test.conf` and `examples/access.log`.

```text
cargo run -p stats-cli -- update --config examples/awstats.example.test.conf --log-file examples/access.log --data-dir target/stats-data
cargo run -p stats-cli -- rebuild --config examples/awstats.example.test.conf --log-file examples/access.log --data-dir target/stats-data
cargo run -p stats-cli -- report --data-dir target/stats-data --output target/stats-report/index.html
cargo run -p stats-cli -- summary --data-dir target/stats-data --limit 10
cargo run -p stats-cli -- optimize --data-dir target/stats-data
cargo run -p stats-cli -- optimize --data-dir target/stats-data --vacuum
cargo run -p stats-cli -- storage --data-dir target/stats-data
cargo run -p stats-cli -- backup --data-dir target/stats-data --output-dir target/stats-backup
cargo run -p stats-cli -- validate --config examples/awstats.example.test.conf
cargo run -p stats-cli -- health --config examples/awstats.example.test.conf --data-dir target/stats-data
```

`update` is the normal cumulative operation. `rebuild` intentionally clears
derived aggregates and replays the selected input. `health` is read-only and
does not create a missing database.

## Other frontends

```text
cargo run -p stats-cli -- watch --config examples/awstats.example.test.conf --log-file examples/access.log --data-dir target/stats-data
cargo run -p stats-cli -- summary --data-dir target/stats-data --watch --interval 5
cargo run -p stats-cli -- live --config examples/awstats.example.test.conf --log-file examples/access.log --data-dir target/stats-data --limit 10 --interval 5
cargo run -p stats-cli -- tui --config examples/awstats.example.test.conf --log-file examples/access.log --data-dir target/stats-data --limit 10 --interval 5
cargo run -p stats-http -- --config examples/awstats.example.test.conf --data-dir target/stats-data --bind 127.0.0.1:3000
```

In the TUI, press `1` for Overview, `2` for Geography, `3` for Clients, `4`
for Performance, `r` to refresh, and `q`, `Esc`, or `Ctrl-C` to exit. The TUI
uses its dedicated terminal renderer and does not load report-export plugins.

With the HTTP process running:

```text
curl 'http://127.0.0.1:3000/report?output=html&limit=50'
curl 'http://127.0.0.1:3000/section/visitors?limit=50'
```

CGI smoke tests use the actual CGI environment:

```text
QUERY_STRING='section=visitors&limit=50' STATS_CONFIG=examples/awstats.example.test.conf STATS_DATA_DIR=target/stats-data cargo run -p stats-cgi
```

The generic configuration serves section-specific HTML. Full HTML, JSON, and
CSV output use external renderer plugins; configure `report_html`, `report_json`,
or `report_csv` before using those output modes. For example, with a configured
renderer file:

```text
cargo build -p stats-report-html -p stats-report-json -p stats-report-csv
cargo run -p stats-cli -- report --config <config-with-report-plugin> --data-dir target/stats-data --output target/stats-report.json --format json
QUERY_STRING='output=html&limit=50' STATS_CONFIG=<config-with-report-plugin> STATS_DATA_DIR=target/stats-data cargo run -p stats-cgi
```

See [Configuration](configuration.md), [Operations](operations.md), and
[Plugins](plugins.md) before adapting these commands to a deployment.
