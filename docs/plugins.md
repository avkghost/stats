# Plugins

## Plugin types and ABI

The plugin system includes an in-process Rust trait registry and a C ABI loader.
Plugins can provide event enrichers or report renderers. The existing native
plugin ABI and `LoadPlugin` configuration syntax are preserved as compatibility
references.

Build the supplied external renderers with:

```text
cargo build -p stats-report-html -p stats-report-json -p stats-report-csv
```

Install selected shared libraries in a trusted directory such as
`/opt/stats/lib/`, then reference them from `LoadPlugin`, for example:

```text
LoadPlugin="report_html /opt/stats/lib/libstats-report-html.so"
LoadPlugin="report_json /opt/stats/lib/libstats-report-json.so"
LoadPlugin="report_csv /opt/stats/lib/libstats-report-csv.so"
```

Event enrichers receive normalized client IP, method, URL, status, and byte
count and may update status or bytes. Report renderers receive a format and
UTF-8 JSON report, then return a media type, extension, and body through an
explicit release callback. HTML, JSON, and CSV stream renderers are loaded only
when requested and configured.

## Worker protocol and failure isolation

Native report renderers run in short-lived supervised child processes. A report
renderer crash, abort, timeout, non-zero exit, malformed response, or I/O
failure fails the request without crashing the CLI, HTTP, or CGI frontend.
Worker requests and responses are bounded to 8 MiB. The direct worker is reaped
after completion or termination; the supervisor does not automatically clean up
independently spawned descendants.

Event-enrichment callbacks use the loaded plugin directly in the analyzer
process. They are trusted native code and are not protected by the report-worker
crash boundary.

The CGI adapter keeps section-specific HTML available from static report
templates. Full HTML, JSON, and CSV output requires `STATS_CONFIG` with the
requested renderer plugin. The TUI uses its dedicated terminal renderer and
does not load report-export plugins.

## Security and trust model

Native plugins are trusted deployment code. They execute with the worker's
filesystem and operating-system privileges, are not OS-sandboxed, and can read
or modify accessible data or spawn processes. Process supervision is crash
containment for the direct worker, not a security sandbox.

External renderer libraries are startup configuration inputs, not request-
controlled paths. Keep plugin directories writable only by trusted deployers and
validate ABI compatibility before loading a library.

See [Deployment](deployment.md#http-and-cgi-boundaries) for HTTP/CGI filesystem
boundaries and [AWStats compatibility](compatibility.md) for compatibility scope.
