# AWStats compatibility scope

This project is an AWStats-style analyzer implemented in Rust. It is not currently
claimed to be a drop-in replacement for the Perl AWStats application.

## Upstream context

The [official AWStats repository](https://github.com/eldy/awstats) says that AWStats
has been maintained for 25 years, identifies version 8.0 as the final version
maintained by the original author, and includes a November 2025 deprecation notice.
The [AWStats 8.0 release](https://github.com/eldy/awstats/releases/tag/AWSTATS_8_0)
is dated 26 August 2025. The upstream notice recommends [Matomo Log
Analytics](https://matomo.org/log-analytics/) as a maintained alternative.

Those facts explain the motivation for this project; they do not establish
compatibility between the two implementations.

## Attribution and provenance

The project uses AWStats configuration names, plugin names, and compatible log
format conventions as interoperability references. The Rust implementation and
plugin mechanism were developed separately. No original AWStats historical
database, Perl implementation fragment, HTML template, or static asset is part
of this repository. AWStats documentation and public behavior were consulted as
references; this document does not make a clean-room claim or imply affiliation
with AWStats or its original author.

## Evidence-based matrix

| Area | Status | Evidence and boundary |
| --- | --- | --- |
| AWStats-style `Key=Value` configuration | Supported and tested | `crates/stats-core/src/config.rs` parses required `LogFile`, `LogFormat`, `SiteDomain`, `HostAliases`, `DirData`, selected filters, timezone, period, and plugin settings; unit tests cover representative configurations. |
| Arbitrary AWStats configuration compatibility | Unsupported | Unknown directives are reported as ignored diagnostics; presentation-only `Show*` settings are accepted as no-ops. The implementation does not claim to accept every upstream directive. |
| Apache/Nginx custom log formats | Supported and tested | `crates/stats-core/src/parser.rs` implements custom tokens and quoted fields; parser tests cover request, host/proxy, timestamp, status, bytes, referrer, user-agent, and duration variants. |
| AWStats time formats 1–6 and common format 4 | Implemented but unverified | The parser contains the format mappings and repository history includes fixes for `time1`, `time1b`, and related fixture parsing. A complete differential comparison with AWStats 8.0 has not been run. |
| JSON-line input and Unix/epoch timestamps | Implemented but unverified | JSON fields and epoch timestamp tokens are implemented in `parser.rs`; no upstream differential result is recorded here. |
| IIS and Lotus log formats | Implemented but unverified | Recent commits explicitly add IIS and Lotus support; dedicated compatibility fixtures are not established as a complete conformance suite. |
| Timezone normalization | Supported and tested | `TimeZone` accepts numeric offsets and IANA zones in `config.rs`/`parser.rs`; tests cover valid, invalid, and named-zone inputs. |
| IPv4/IPv6 client handling | Partially supported | The parser and reverse-DNS safeguards handle IPv4, IPv6, and non-routable addresses, but full AWStats hostname/client semantics have not been differentially validated. |
| Requests, hits, pages, files, visits, bandwidth | Supported and tested | Aggregation and report structures in `crates/stats-core/src/store.rs` and `report.rs`, with renderer/CLI tests and generated report paths. Metric definitions may differ from AWStats. |
| Status classes, HTTP errors, request-time buckets | Supported and tested | Report aggregation and HTML rendering cover status/error and request-time data; see `crates/stats-core/src/report.rs` and `crates/stats-render/src/html.rs`. |
| Referrers, search engines, keyphrases, robots | Partially supported | Report fields and enrichment logic exist, including referer-spam and scanner rules, but parity with AWStats databases and classification tables is not established. |
| Browser, OS, country, region, city, ASN, network organization | Partially supported | Report fields and optional MaxMind providers exist; results depend on configured external MMDB files and have not been compared against AWStats output. |
| Incremental updates and duplicate prevention | Supported and tested | SQLite checkpoints, file identity/content checks, replay/rebuild, rotation handling, and partition tests are implemented in the CLI/store paths. |
| Historical import and compressed logs | Partially supported | Explicit chronological updates and `.gz`, `.bz2`, and `.xz` input are documented and implemented; this is not an AWStats history-file migration format. |
| SQLite persistence and period partitioning | Supported and tested | `stats-core` provides SQLite storage; CLI tests cover monthly/yearly partition routing, federation, migration, retention, and backup behavior. |
| Static HTML reports | Supported and tested | `stats-render` produces a multi-page HTML site and has escaping/rendering tests. |
| JSON and CSV reports | Supported and tested | External renderer crates and plugin-host tests cover configured stream renderers. |
| CGI, HTTP, TUI, and terminal frontends | Implemented but unverified | `stats-cgi`, `stats-http`, `stats-tui`, and CLI commands exist and compile; this audit did not perform a full deployed-service or browser compatibility test. |
| Native plugins | Supported and tested | Rust event-enrichment traits and a C ABI/dynamic loader exist. Report-renderer metadata and callbacks run in supervised child processes; configured JSON and CSV renderers were exercised through the CLI worker path. This is not differential verification against AWStats. |
| AWStats drop-in replacement | Unknown / not claimed | No complete command-line, configuration, historical database, report, or statistical differential suite against AWStats 8.0 is present. |

> **Security warning:** Native plugins are trusted deployment code, not a sandbox.
> Report-renderer crashes are isolated to their worker process, but a plugin still
> runs with the worker's filesystem and operating-system privileges and may read,
> modify, or delete accessible data or spawn processes. Event-enrichment plugins
> also run as trusted worker processes. Load only plugins from sources and paths
> controlled by the operator.
>
> The supervisor bounds each request/response message at 8 MiB and reaps the
> direct worker process after completion, failure, or timeout. It does not
> implement operating-system process-group or descendant cleanup; a plugin that
> deliberately spawns independent descendants remains outside this isolation
> guarantee.

## Known behavioral differences and limitations

- The configuration parser intentionally supports a subset of AWStats directives and
  reports ignored directives instead of silently implementing the full AWStats model.
- The storage format is SQLite, not AWStats's Perl history-file format; AWStats
  historical files are not automatically interchangeable.
- Visit and unique-visitor calculations depend on this project's session/checkpoint
  model and may differ from AWStats for the same records.
- GeoIP, reverse DNS, browser/OS, robot, and search-engine results depend on local
  databases and classification rules.
- A complete differential test against the upstream AWStats executable and a shared
  corpus remains future work.
