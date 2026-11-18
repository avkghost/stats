# Configuration

Stats reads an AWStats-style `Key=Value` configuration. It implements a defined
subset of the upstream syntax; it does not accept every AWStats directive.
`validate` reports directives that are currently ignored.

The public example is [examples/awstats.example.test.conf](../examples/awstats.example.test.conf).

## Main settings

The implemented configuration includes the required log source and identity
settings such as `LogFile`, `LogFormat`, `SiteDomain`, `HostAliases`, and
`DirData`, together with selected filters, timezone, period, and plugin settings.
Presentation-only `Show*` settings are accepted as no-ops. Unknown directives
are reported as ignored diagnostics.

`DatabasePeriod=none` keeps the default single-database layout. `monthly` and
`yearly` enable partitioned storage; see [Operations](operations.md#partitioning-and-retention).

`CardinalityLimit` controls the maximum number of distinct keys retained for
high-cardinality dimensions such as URLs, user agents, referers, hostnames,
and diagnostics. New keys beyond the limit are counted under `other`. The
default is `10000`; the `other` bucket occupies one of those rows, so at most
`CardinalityLimit - 1` named keys are retained. Set a smaller value when
untrusted traffic must not be allowed to grow the database through unique
dimension values. The limit applies to new records only: existing rows are not
pruned or merged when the setting is lowered. The fingerprint is computed from
the raw configuration-file bytes, so changing a comment, whitespace, or any
directive causes the normal configuration-fingerprint replay.

The repository includes a manual benchmark for this path. Run
`cargo test -p stats-core --test store_golden benchmark_cardinality_limit
-- --ignored --nocapture` to compare the capped path with the internal
uncapped control on 10,500 unique and repeated events. The unique-key result
isolates the cost of admitting new keys; the repeated-key result primarily
shows the existence-check overhead. The benchmark is intentionally ignored by
the normal suite because timings are machine-dependent.

## Log formats

Supported inputs include Apache/Nginx custom formats, common LogFormat 4,
JSON-line records, Unix timestamps, and AWStats time formats 1–6. Apache aliases
include request tokens `%r`, `%m`, `%U`, `%q`, and `%H`; client and virtual-host
fields `%a`, `%{c}a`, `%v`, and `%{Host}i`; epoch timestamps `%{sec}t`,
`%{msec}t`, and `%{usec}t`; duration tokens `%D`, `%T`, and `%{ms}T`; and
arbitrary quoted headers.

Malformed records are rejected or skipped according to the parser path and are
reported through the analyzer diagnostics. Always validate a representative
sample before relying on aggregate totals.

## Timezone and GeoIP

`TimeZone=<hour-offset|IANA/Zone>` normalizes parsed timestamps before
aggregation, including daylight-saving transitions for named IANA zones. If it
is unset, the timestamp offset from each record is preserved.

MaxMind Country, City, and ASN MMDB providers are optional. GeoIP lookup guards
private, loopback, unspecified, multicast, link-local, unique-local, and
scope-suffixed addresses before consulting an MMDB file.
Configured GeoIP and plugin resource paths are deployment inputs and must be
readable by the process account.

## Report and display settings

Page totals exclude common static-file extensions by default. Set `NotPageList`
to replace that list explicitly. `urlalias` display mappings are read from
`DirData/urlalias.<SiteDomain>.txt` or `DirData/urlalias.txt`; stored and
machine-readable URL keys remain canonical.

See [AWStats compatibility](compatibility.md) for the directive-level evidence
matrix and known differences from upstream AWStats.
