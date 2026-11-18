# GeoIP test fixtures

These small MMDB files are the official MaxMind-DB test databases:

<https://github.com/maxmind/MaxMind-DB/tree/main/test-data>

They are test data only and are distributed under the Apache-2.0 or MIT
license, at the user's option, as stated by the upstream repository. They do
not contain production GeoLite data or require a MaxMind account.

The files are used by the positive GeoIP unit test in
`crates/stats-core/src/geoip.rs`.
