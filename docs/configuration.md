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
private, loopback, unspecified, multicast, and scope-suffixed IPv6 addresses.
Configured GeoIP and plugin resource paths are deployment inputs and must be
readable by the process account.

## Report and display settings

Page totals exclude common static-file extensions by default. Set `NotPageList`
to replace that list explicitly. `urlalias` display mappings are read from
`DirData/urlalias.<SiteDomain>.txt` or `DirData/urlalias.txt`; stored and
machine-readable URL keys remain canonical.

See [AWStats compatibility](compatibility.md) for the directive-level evidence
matrix and known differences from upstream AWStats.
