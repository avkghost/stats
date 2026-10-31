# AWStats plugin parity

This project treats the AWStats 8.0 source tree as the compatibility reference. The upstream plugin inventory is maintained under [`wwwroot/cgi-bin/plugins`](https://github.com/eldy/AWStats/tree/develop/wwwroot/cgi-bin/plugins). The table below is an implementation audit, not a claim of complete parity: parity is complete only when every inventory entry has a Rust implementation or a tested compatibility adapter.

The current upstream inventory contains 22 entries: `clusterinfo`, `decodeutfkeys`,
`detectrefererspam`, `example`, `export_to_csv`, `geoip`, `geoip2_city`,
`geoip2_country`, `geoip2_region`, `geoipfree`, `geoip6`, `graphapplet`,
`graphgooglechartapi`, `hashfiles`, `hostinfo`, `ipv6`, `rawlog`, `timehires`,
`timezone`, `tooltips`, `urlalias`, and `userinfo`.

## Compatibility map

| Upstream plugin family | Current status | Decision |
| --- | --- | --- |
| `geoip2_*`, `geoip_*`, `geoipfree`, `geoip6` | Core MaxMind Country, City, ASN, region, and organization enrichment | Keep as core providers; do not duplicate Perl plugin loading |
| `export_to_csv` | External CSV report plugin | Keep as a native report renderer |
| `graphapplet`, `graphgooglechartapi` | Replaced by local HTML/CSS charts | Keep local and offline; do not depend on Java or third-party chart services |
| `timezone` | Core `TimeZone` configuration with fixed-offset and IANA-zone conversion | Keep deterministic, checkpoint-aware, and DST-aware |
| `userinfo` | Core authenticated-user aggregation with display decoration from `userinfo[.<SiteDomain>].txt` or `userinfo.txt` | Keep stored user keys canonical; decorate only report rows |
| `clusterinfo` | Display-only cluster-info adapter loads `clusterinfo[.<SiteDomain>].txt` | Keep labels out of canonical stored dimensions |
| `hashfiles` | SQLite-backed compatibility adapter records the selected backend; no incompatible Perl `.hash` files are created | Keep SQLite authoritative and document the format boundary |
| `tooltips`, `decodeutfkeys` | Referrer UTF-8 key normalization covered; native HTML chart-row tooltips implemented | Keep tooltips presentation-only and offline |
| `urlalias` | Candidate parity feature | Add a report-decoration hook without changing stored URL keys |
| `detectrefererspam` | Minimal explicit rule engine implemented and documented | Extend only with configured rules; do not invent a universal heuristic |
| `rawlog` | CLI, authenticated HTTP, and authenticated CGI exports use bounded redacted output | Keep endpoints disabled unless `STATS_RAW_LOG_AUTH_TOKEN` and reverse-proxy bearer authentication are configured |
| `hostinfo`, `ipv6` | Optional bounded reverse-DNS enrichment implemented behind `DNSLookup=1` | Never perform reverse-DNS lookups during normal updates unless explicitly enabled |
| `timehires` | Update summaries expose millisecond elapsed timing when enabled | Keep timing diagnostics outside report data |

## Implementation order

0. Required gate: inventory every upstream plugin and record one of `implemented`,
`compatibility adapter`, or `not started`; do not report AWStats parity while any
entry is `not started`.

1. Completed: report-decoration hooks for URL aliases affect only display labels; aggregation, checkpoints, and JSON keys remain canonical URLs.
2. Completed: referer-spam classification is an explicit rule dimension and exclusions do not change traffic totals.
   The rules file is only loaded when `LoadPlugin=detectrefererspam` is present.
3. Completed: referrer query keys decode valid UTF-8 and preserve the original value when decoding fails.
4. Completed: raw-log access is available only through bounded redacted CLI, authenticated HTTP, and authenticated CGI paths.
5. Completed: `userinfo` loads the AWStats-compatible site-specific/fallback mapping and decorates authenticated-user report rows without changing aggregation keys.
6. Completed: `timezone` accepts numeric offsets and named IANA zones with daylight-saving conversion.

The current external renderer ABI is intentionally format-oriented. These features should be implemented as shared report transformations or narrowly scoped plugin capabilities, not by embedding Perl-compatible plugin execution in the Rust process.

The upstream `detectrefererspam` source currently only registers its scan hook and logs a debug call. It does not define a domain list, regular expressions, or a discard policy, so Rust should not claim parity by adding an undocumented heuristic.
