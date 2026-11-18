# Documentation

This directory is the source of truth for the project documentation. Start with
the page that matches the question, then use the module pages to connect the
behaviour to the implementation.

## Architecture and modules

- [Architecture](architecture.md) — system boundaries, data flow, storage, and
  Mermaid diagrams.
- [Core module](modules/core.md) — parsing, enrichment, aggregation, reports,
  and SQLite persistence in `stats-core`.
- [CLI module](modules/cli.md) — commands, orchestration, incremental ingestion,
  rotation, partitioning, and locks in `stats-cli`.
- [Rendering and frontends](modules/frontends.md) — HTML, terminal, TUI, HTTP,
  and CGI responsibilities.
- [Plugin system](modules/plugins.md) — plugin API, loader, worker protocol,
  renderer plugins, and trust boundaries.
- Rotation research is kept outside the repository and is not part of the
  published documentation tree.

## User and operator guides

- [Getting started](getting-started.md)
- [Configuration](configuration.md)
- [Operations](operations.md)
- [Deployment](deployment.md)
- [Raw-log safety](raw-log-safety.md)

## Compatibility and feature references

- [AWStats compatibility](compatibility.md)
- [AWStats plugin parity](awstats-plugin-parity.md)
- [AWStats plugin manifest](awstats-plugin-manifest.md)
- [HTTP method anomaly detection](http-method-anomaly.md)
- [Scanner detection](scanner-detection.md)
- [Referer-spam rules](referer-spam-rules.md)
- [Rotation boundary contract](rotation-boundary-contract/README.md)

## Documentation conventions

Behavioural guarantees belong in the user or operations guides. Module pages
explain ownership and boundaries and should link to those guarantees instead of
duplicating them. Architecture diagrams describe runtime relationships; they are
not substitutes for command or configuration reference documentation.

## Design records

- [Rotation boundary contract](rotation-boundary-contract/README.md) — historical design for the rotation and backfill behaviour.
