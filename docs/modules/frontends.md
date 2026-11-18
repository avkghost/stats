# Rendering and frontends

The workspace separates report data from presentation and transport. All
frontends query the `stats-core` report model; none should read SQLite tables
directly.

## Packages

| Package | Role | Boundary |
| --- | --- | --- |
| `stats-render` | HTML and native report renderer integration | Converts a `Report` into a site or plugin response |
| `stats-tui` | Ratatui dashboard | Reads report data and owns terminal interaction |
| `stats-http` | Standalone HTTP server | Serves reports and authenticated raw-log diagnostics |
| `stats-cgi` | CGI adapter | Maps CGI environment and query parameters to read-only output |
| `stats-cli` | Terminal commands and orchestration | Selects commands, output formats, and write paths |

## Rendering flow

The built-in HTML renderer is deterministic and offline. JSON and CSV are
external renderer plugins selected by configuration. The HTTP and CGI adapters
can expose configured report formats, while TUI and terminal summaries use
their dedicated renderers.

Raw-log output is a separate, bounded, redacted path. It is disabled by default
and must not be inferred from the presence of a renderer plugin. See
[Raw-log safety](../raw-log-safety.md).

## Failure boundary

Native report renderers run through supervised worker processes. The worker is the `stats-report-worker` binary, which must sit next to the frontend binary. A direct worker
crash, timeout, malformed response, or non-zero exit fails the request without
crashing the parent frontend. This is crash containment, not an OS sandbox;
event-enrichment callbacks and independently spawned descendants remain trusted
code. See the [plugin module guide](plugins.md#two-execution-paths).

## Request flows

### HTTP requests (D11)

```mermaid
sequenceDiagram
    actor Client
    participant Routes as stats-http routes (main.rs:96-103)
    participant Handler as report, section, raw_log handlers
    participant Blocking as spawn_blocking
    participant Service as ReportService, read-only store
    participant Overlay as overlays: urlalias, clusterinfo, userinfo
    participant Renderer as HTML, JSON, CSV renderer

    Client->>Routes: GET /report?output=...&limit=N
    Routes->>Handler: report, limit clamped to 1..10000, default 100
    Handler->>Blocking: build and render
    Blocking->>Service: open stats.sqlite3 read-only, build report
    Service-->>Blocking: Report
    opt HTML and config loaded
        Blocking->>Overlay: apply loaded overlays
    end
    Blocking->>Renderer: render selected format
    alt success
        Renderer-->>Client: 200 with the media type
    else any error
        Renderer-->>Client: 500 report request failed
    end
    Client->>Routes: GET /section/{section}
    Note over Routes,Renderer: HTML sections only
    Client->>Routes: GET /raw-log
    Note over Routes: 404 unless rawlog is loaded and a token is set, 401 with WWW-Authenticate on a bad bearer
    Client->>Routes: GET /healthz, GET /readyz
    Note over Routes: /healthz returns 200 ok, /readyz opens the store read-only
```

### CGI requests (D12)

```mermaid
sequenceDiagram
    actor Browser
    participant Server as web server (CGI)
    participant Cgi as stats-cgi (main.rs:238)
    participant Parse as parse_query (main.rs:55-94)
    participant Render as render_cgi (main.rs:126-236)
    participant Store as stats.sqlite3, read-only

    Browser->>Server: GET the CGI URL
    Server->>Cgi: run with QUERY_STRING, STATS_DATA_DIR, STATS_CONFIG, STATS_RAW_LOG_AUTH_TOKEN, HTTP_AUTHORIZATION
    Cgi->>Parse: allow-listed parameters only
    alt parameter rejected or any error
        Cgi-->>Server: Status 500, report request failed, exit 1
    else accepted
        Cgi->>Render: build the request
        opt output is raw-log
            Render->>Render: require rawlog plugin and matching bearer token
        end
        Render->>Store: read report
        Render-->>Cgi: body and media type
        Cgi-->>Server: Content-Type, Cache-Control no-store for raw-log, body
    end
    Server-->>Browser: response
```
