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

## Variant C: authenticated diagnostic endpoint

An HTTP endpoint may be enabled only behind deployment authentication and an
allowlist of source files. It must support bounded time/line ranges, emit an
audit event, and return redacted records by default. The endpoint should be
disabled when no authentication integration is configured.

## Variant D: redacted plugin output

An external renderer can receive structured events after redaction. The default
redaction policy should hash or remove client IPs, identities, query strings,
authorization-like headers, cookies, and token-shaped values. Raw values must
never be passed to a renderer merely because it is configured.

The implementation should begin with Variant A plus local Variant B. Variants C
and D require an explicit authorization and redaction policy in configuration;
they should not be inferred from the presence of a report plugin.
