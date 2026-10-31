# AWStats 8.0 plugin parity manifest

This manifest is the acceptance checklist for complete AWStats 8.0 plugin
parity. A feature that is merely similar is not marked complete; each entry
needs a tested Rust implementation or a compatibility adapter with equivalent
observable behavior.

| Plugin | Status | Required work |
| --- | --- | --- |
| `clusterinfo` | compatibility adapter | Load `clusterinfo[.<SiteDomain>].txt` and decorate display virtual-host labels |
| `decodeutfkeys` | implemented | Preserve valid UTF-8 query-key decoding and invalid-value fallback |
| `detectrefererspam` | compatibility adapter | Configured rule engine and report dimension |
| `example` | implemented | Loadable no-op native extension template with documented lifecycle |
| `export_to_csv` | compatibility adapter | External CSV renderer with equivalent export coverage |
| `geoip` | compatibility adapter | Core MaxMind provider mapping |
| `geoip2_city` | compatibility adapter | Core MaxMind City provider mapping |
| `geoip2_country` | compatibility adapter | Core MaxMind Country provider mapping |
| `geoip_asn_maxmind` | compatibility adapter | Core MaxMind ASN mapping |
| `geoip_city_maxmind` | compatibility adapter | Core MaxMind City mapping |
| `geoipfree` | compatibility adapter | Core local MaxMind fallback behavior |
| `geoip6` | compatibility adapter | IPv6 lookup through the core provider |
| `geoip_isp_maxmind` | compatibility adapter | Core MaxMind network-organization mapping |
| `geoip_org_maxmind` | compatibility adapter | Core MaxMind organization mapping |
| `geoip_region_maxmind` | compatibility adapter | Core MaxMind region mapping |
| `graphapplet` | compatibility adapter | Offline HTML/CSS chart equivalent |
| `graphgooglechartapi` | compatibility adapter | Offline chart equivalent without remote service calls |
| `hashfiles` | compatibility adapter | Explicitly selects SQLite as the authoritative replacement for Perl `Storable` hash files |
| `hostinfo` | compatibility adapter | Opt-in bounded reverse-DNS enrichment |
| `ipv6` | compatibility adapter | IPv6 parsing, filtering, and enrichment |
| `rawlog` | compatibility adapter | Bounded redacted CLI, HTTP, and CGI exports |
| `timehires` | compatibility adapter | Millisecond elapsed timing in update summaries |
| `timezone` | compatibility adapter | Fixed offsets and named IANA zones with daylight-saving conversion |
| `tooltips` | compatibility adapter | Escaped HTML chart and table tooltips |
| `urlalias` | compatibility adapter | Display-only URL alias transformation |
| `userinfo` | compatibility adapter | Load site-specific or fallback tab-separated user labels and decorate authenticated-user rows |

AWStats parity must not be reported as complete while any entry is `not
started` or lacks plugin-specific compatibility tests.
