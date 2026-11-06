# Stats

[![CI](https://github.com/avkghost/stats/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/avkghost/stats/actions/workflows/ci.yml)
[![Release Build](https://github.com/avkghost/stats/actions/workflows/release.yml/badge.svg)](https://github.com/avkghost/stats/actions/workflows/release.yml)
[![Latest Release](https://img.shields.io/github/v/release/avkghost/stats)](https://github.com/avkghost/stats/releases)
[![License](https://img.shields.io/github/license/avkghost/stats)](https://github.com/avkghost/stats/blob/master/LICENSE)

Stats is an experimental Rust-based, AWStats-style web log analyzer. It was
developed as an independent alternative for selected AWStats-oriented workflows
after AWStats's deprecation notice. It is not a drop-in AWStats replacement and
does not promise complete statistical parity.

The upstream project is [eldy/awstats](https://github.com/eldy/awstats), created
and maintained by Laurent Destailleur (`eldy`). AWStats's notice recommends
[Matomo Log Analytics](https://matomo.org/log-analytics/). Stats is independent
and is not affiliated with or endorsed by AWStats, Laurent Destailleur, or Matomo.

## Contents

- [Implemented capabilities](#implemented-capabilities)
- [Quick start](#quick-start)
- [AWStats compatibility](#awstats-compatibility)
- [Documentation](#documentation)
- [License and acknowledgments](#license-and-acknowledgments)

## Implemented capabilities

- AWStats-style `Key=Value` configuration with a documented compatibility subset.
- Apache/Nginx custom formats, common LogFormat 4, JSON-line records, Unix timestamps, and AWStats time formats 1–6.
- Optional IANA or fixed-offset timezone normalization and MaxMind Country, City, and ASN enrichment.
- SQLite aggregate storage with append checkpoints and optional monthly/yearly partitioning.
- Static multi-page HTML reports, terminal summaries, a Ratatui dashboard, and HTTP/CGI frontends.
- AWStats-style hits, pages, files, visits, traffic, status, referrer, search, geography, browser, OS, robot, and related report dimensions.
- Event-enrichment plugins and external HTML, JSON, and CSV report renderers.
- Supervised native report workers with bounded 8 MiB IPC messages and controlled crash, timeout, and malformed-response handling.

Native plugins are trusted deployment code. Event-enrichment callbacks execute
in-process in the analyzer, while native report renderers execute in supervised
workers. Plugins are not OS-sandboxed. Supervised report workers protect the
frontend from a direct report-plugin crash, but independently spawned
descendant processes are not automatically cleaned up. See
[Plugin safety](docs/plugins.md#security-and-trust-model).

## Quick start

Requires a current stable Rust toolchain. From the repository root:

```text
cargo run -p stats-cli -- update --config examples/awstats.example.test.conf --log-file examples/access.log --data-dir target/stats-data
cargo run -p stats-cli -- report --data-dir target/stats-data --output target/stats-report/index.html
cargo run -p stats-cli -- summary --data-dir target/stats-data --limit 10
```

The fixture is anonymized and runs without GeoIP databases. By default, an
update generates the static site at the `output/index.html` sibling of the
configured data directory (for `DirData="stats"`, this is `output/index.html`);
`report --output <path>` selects a custom output path. Build and test the
workspace with the commands in
[Getting started](docs/getting-started.md).

## AWStats compatibility

Compatibility is limited to implemented behavior and is reported with evidence.
Configuration names, plugin names, and selected log-format conventions are
compatibility references; original AWStats historical databases are not used.
Unknown directives are reported as diagnostics, and complete upstream directive
or report parity is unsupported. Read the [compatibility matrix](docs/compatibility.md)
before treating any output as equivalent.

## Documentation

- [Getting started](docs/getting-started.md) — installation, build, fixtures, and frontend smoke tests.
- [Configuration](docs/configuration.md) — directives, formats, timezone, GeoIP, and validation.
- [Operations](docs/operations.md) — incremental updates, rotation, backfill, partitions, backups, and migration.
- [Deployment](docs/deployment.md) — release bundles, `/opt/stats` installation, systemd, cron, logrotate, HTTP, and CGI.
- [Plugins](docs/plugins.md) — ABI, renderers, worker protocol, failure isolation, and trust boundaries.
- [Architecture](docs/architecture.md) — workspace boundaries, processing, storage, and reporting.
- [AWStats compatibility](docs/compatibility.md) — evidence-based compatibility matrix and limitations.
- [Changelog](CHANGELOG.md) — release history and verified changes.

## License and acknowledgments

The project is released under [GPL-3.0-or-later](LICENSE). AWStats names and
interfaces are acknowledged as compatibility references. See the compatibility
document for provenance notes and the limits of that relationship.
