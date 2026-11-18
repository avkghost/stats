# Plugin system

The plugin system has two distinct paths: event enrichment and report
rendering. They share configuration vocabulary but have different execution and
failure boundaries.

## Packages and roles

| Package | Role |
| --- | --- |
| `stats-plugin-api` | Versioned C ABI types, descriptors, events, and renderer callbacks |
| `stats-plugin-host` | Dynamic loading, ABI checks, worker supervision, and response validation |
| `stats-core` | In-process Rust traits for event enrichers and report renderers |
| `stats-render` | Built-in renderers and adapters for configured native renderers |
| `plugins/report-*` | External HTML, JSON, and CSV renderer libraries |
| `plugins/example` | Minimal native plugin example |

## Two execution paths

Event enrichers receive normalized events during analysis and execute in the
analyzer process. They can affect event fields and are trusted deployment code.

Report renderers receive a bounded JSON report request in a short-lived worker
and return a media type, extension, and body through the ABI release callback.
The supervisor limits messages to 8 MiB. On Unix the worker leads its own process
group; on timeout, a malformed or failed response, or an unsuccessful exit, the
whole group receives SIGKILL before the request writer is joined. Processes that
leave the group (for example with `setsid`) are not reached. On other platforms
only the direct worker is killed, so descendants may outlive the request. The
supervisor provides no OS sandboxing.

## Configuration and deployment

Plugin paths are startup configuration, not request-controlled paths. Install
libraries in a trusted directory, validate ABI compatibility, and keep the
directory writable only by trusted deployers. See [Plugins](../plugins.md) for
the protocol details and [Deployment](../deployment.md) for service boundaries.

AWStats plugin names are compatibility aliases or adapter concepts, not a claim
that Perl plugins can be executed by this project. The status of each supported
family is recorded in the [parity manifest](../awstats-plugin-manifest.md).

## Worker and ABI flows

### Renderer worker supervision (D13)

```mermaid
sequenceDiagram
    participant Renderer as NativeReportRenderer (stats-render native.rs)
    participant Sup as supervise_report_worker (stats-plugin-host)
    participant Child as worker: stats-report-worker <plugin-path>
    participant Plugin as native plugin library

    Renderer->>Sup: request JSON, timeout 10 s
    Sup->>Sup: reject a request over 8 MiB before spawning
    Sup->>Child: spawn with the plugin path, stdin and stdout piped, stderr discarded
    Sup->>Child: write JSON and a newline, close stdin
    Child->>Plugin: load and check ABI
    Child->>Plugin: metadata or render
    Plugin-->>Child: body, or error
    Child-->>Sup: one JSON line on stdout
    loop while the child runs
        Sup->>Sup: reader thread caps stdout at 8 MiB, poll every 5 ms
    end
    alt deadline passed
        Sup->>Child: kill and reap
        Sup-->>Renderer: Timeout
    else non-zero exit
        Sup-->>Renderer: Exit status
    else response valid
        Sup-->>Renderer: JSON object with protocol_version 1
    end
```

### Plugin ABI: load and render (D14)

```mermaid
sequenceDiagram
    participant Host as LoadedPlugin::load (stats-plugin-host)
    participant Lib as shared library (libloading)
    participant Plugin as stats_plugin_init
    participant API as PluginApi table

    Host->>Lib: open the library path
    Host->>Lib: get symbol stats_plugin_init
    Host->>Plugin: init with HostApi, ABI version 1
    Plugin-->>Host: pointer to PluginApi, or null
    Host->>Host: null is a NullApi error
    Host->>API: read descriptor
    Host->>Host: abi_version must equal 1, else Abi error
    Host->>Host: name and version must be non-null UTF-8
    Host->>API: render needs CAPABILITY_REPORT and both function pointers
    Host->>API: render_report(request)
    API-->>Host: status and RenderedReport
    alt status non-zero
        Host->>API: release_report, then ReportCallback error
    else status zero
        Host->>Host: read media type, extension, body as C strings
        Host->>API: release_report
    end
    Note over Host: Drop calls shutdown when the plugin provides it
```
