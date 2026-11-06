# Architecture

## Workspace boundaries

The workspace contains 12 packages covering core parsing and storage, CLI,
HTTP/CGI/TUI frontends, renderers, and plugins. The main execution path is:

1. A frontend reads configuration and selects the input source.
2. `stats-core` parses records into normalized events and applies GeoIP and
   plugin enrichment.
3. SQLite storage updates aggregates and append checkpoints.
4. The shared report query API reads aggregates.
5. HTML, JSON, CSV, terminal, HTTP, CGI, and TUI consumers render the result.

`stats-core` owns normalized events, parsing, GeoIP providers, plugin traits,
storage, and report queries. `stats-cli` owns orchestration and output. HTML,
terminal summaries, and the Ratatui dashboard consume the shared report query
API rather than accessing storage directly.

## Storage and processing

The default layout uses a single SQLite database. Optional monthly or yearly
partitioning keeps control/session state in `DirData/stats.sqlite3` and places
event aggregates under `DirData/periods/`. Reports federate period databases.
Append checkpoints use source identity and content hashes; changed prefixes,
replacement, or truncation trigger safe replay behavior.

SQLite uses WAL mode, a five-second busy timeout, normal synchronous durability,
and in-memory temporary storage. `backup`, `optimize`, `storage`, and `retention`
are explicit operational commands rather than hidden background services.

## Reporting and plugins

The default HTML path is a static multi-page report. External report renderers
are separate dynamic libraries and are loaded only when configured and requested.
Event-enrichment callbacks are invoked directly by the analyzer through the
loaded plugin and therefore run in the analyzer process. Native report-renderer
calls use short-lived supervised workers with bounded IPC. The worker boundary
protects the parent frontend from a direct report-plugin crash, but does not
provide OS sandboxing or descendant-process cleanup; it does not protect
in-process event callbacks.

See [Plugins](plugins.md), [Operations](operations.md), and
[AWStats compatibility](compatibility.md) for the operational and behavioral
details of these boundaries.
