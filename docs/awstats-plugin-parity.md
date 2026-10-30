# AWStats plugin parity

This project treats the AWStats 8.0 source tree as the compatibility reference. The upstream plugin inventory is maintained under [`wwwroot/cgi-bin/plugins`](https://github.com/eldy/AWStats/tree/develop/wwwroot/cgi-bin/plugins).

## Compatibility map

| Upstream plugin family | Current status | Decision |
| --- | --- | --- |
| `geoip2_*`, `geoip_*`, `geoipfree`, `geoip6` | Core MaxMind Country, City, ASN, region, and organization enrichment | Keep as core providers; do not duplicate Perl plugin loading |
| `export_to_csv` | External CSV report plugin | Keep as a native report renderer |
| `graphapplet`, `graphgooglechartapi` | Replaced by local HTML/CSS charts | Keep local and offline; do not depend on Java or third-party chart services |
| `timezone` | Core `TimeZone` configuration | Keep deterministic and checkpoint-aware |
| `userinfo` | Core authenticated-user aggregation | Keep in the shared report model |
| `clusterinfo`, `hashfiles` | Not applicable to the current SQLite/checkpoint design | Revisit only if distributed ingestion is introduced |
| `tooltips`, `decodeutfkeys` | Presentation/encoding concerns | Add only when a concrete AWStats report compatibility case requires them |
| `urlalias` | Candidate parity feature | Add a report-decoration hook without changing stored URL keys |
| `detectrefererspam` | Minimal explicit rule engine implemented and documented | Extend only with configured rules; do not invent a universal heuristic |
| `rawlog` | Local CLI export implemented with bounded redacted output; HTTP/CGI remain disabled | Keep network raw-log exposure disabled until authentication and redaction policy are integrated |
| `hostinfo`, `ipv6` | Optional bounded reverse-DNS enrichment implemented behind `DNSLookup=1` | Never perform reverse-DNS lookups during normal updates unless explicitly enabled |
| `timehires` | Internal diagnostic concern | Keep timing diagnostics outside report data |

## Implementation order

1. Add report-decoration hooks for URL aliases. The alias file must affect only display labels; aggregation, checkpoints, and JSON keys remain canonical URLs.
2. Add referer-spam classification as an explicit filter dimension. It must be observable in diagnostics and never silently discard records unless configured.
3. Add optional UTF-8 key normalization for referrer-derived values, preserving the original value when decoding fails.
4. Re-evaluate raw-log access only after CGI/HTTP authorization and redaction are defined.

The current external renderer ABI is intentionally format-oriented. These features should be implemented as shared report transformations or narrowly scoped plugin capabilities, not by embedding Perl-compatible plugin execution in the Rust process.

The upstream `detectrefererspam` source currently only registers its scan hook and logs a debug call. It does not define a domain list, regular expressions, or a discard policy, so Rust should not claim parity by adding an undocumented heuristic.
