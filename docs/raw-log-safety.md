# Raw-log output safety variants

Raw access-log output is disabled by default. It can contain IP addresses,
identities, authenticated usernames, URLs with secrets, cookies, and user-agent
data. The following deployment variants define the intended security boundary.

## Variant A: disabled by default

Only aggregated reports are available. This is the default for CLI, CGI, TUI,
and HTTP output and is suitable for public dashboards.

## Variant B: local administrator export (implemented)

`stats-cli raw-log` exports a bounded slice of the configured source for a local
operator. It always redacts client IPs, query strings, and token-shaped values,
and output files are created with restrictive permissions. It is not exposed
through the HTTP server.

## Variant C: authenticated diagnostic endpoint (HTTP implemented)

The standalone HTTP frontend exposes `/raw-log` only when the non-empty
`STATS_RAW_LOG_AUTH_TOKEN` environment variable is configured. A reverse proxy
should inject `Authorization: Bearer <token>` after authenticating the operator.
The endpoint supports bounded line ranges, reads only a configured log beneath
`DirData`, and returns redacted records. It remains disabled when the token is
absent. CGI supports the equivalent `output=raw-log` request and checks
`HTTP_AUTHORIZATION` against the same token.

## Variant D: renderer input

This project does not expose raw access-log lines to report renderers. External
renderers receive the structured report model, not the raw event stream, and
the raw-log redaction path is not a renderer extension point. Therefore Variant
D is not an implemented raw-log export mode and must not be inferred from the
presence of a report plugin.

The supported baseline is Variant A, with local Variant B and explicitly
authenticated Variants C where configured. All raw-log paths use the shared
bounded redaction implementation for client identity, query strings, and
secret-shaped headers or parameters.
