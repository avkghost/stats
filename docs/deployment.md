# Deployment

## Installed layout

The supplied installer keeps application-owned files under `/opt/stats/`:

- `/opt/stats/bin/stats` — analyzer executable
- `/opt/stats/etc/awstats.example.test.conf` — expected configuration path
- `/opt/stats/var/lib` — SQLite data
- `/opt/stats/var/www` — generated reports
- `/opt/stats/lib/` — trusted native plugins

System-managed units remain under `/etc/systemd/system/` and logrotate remains
under `/etc/logrotate.d/`. Override application paths with
`STATS_BINARY_PATH`, `STATS_CONFIG_PATH`, `STATS_DATA_DIR`, and `STATS_OUTPUT_DIR`.

## Installer and systemd

Build a release binary, install the configuration, then run:

```text
sudo deploy/install.sh
systemctl start --wait stats-update.service
```

`deploy/install.sh` creates the `stats` system account and data/output
directories, installs the binary and `stats-update.service`/`stats-update.timer`,
reloads systemd, and enables the timer. It intentionally does not run an update
until configuration has been installed.

The service is restricted with `NoNewPrivileges`, `PrivateTmp`, `ProtectHome`,
`ProtectSystem=strict`, and explicit writable paths. The service account still
needs read access to the active log and GeoIP databases and write access to the
data and output directories.

If a configuration points at production paths such as
`/var/log/nginx/access.log` or `/var/lib/awstats`, run as the service account
with those permissions, or override both paths explicitly for a local run.

## Cron and logrotate

A safe logrotate `postrotate` hook can run:

```text
/opt/stats/bin/stats update --config /opt/stats/etc/awstats.example.test.conf
```

An equivalent timer-driven cron entry is:

```text
*/5 * * * * stats /opt/stats/bin/stats update --config /opt/stats/etc/awstats.example.test.conf >/var/log/stats-update.log 2>&1
```

The templates are `deploy/stats-update.service`, `deploy/stats-update.timer`,
and `deploy/stats-logrotate`. Install the service and timer under
`/etc/systemd/system/` and the logrotate file under `/etc/logrotate.d/stats`.

## Release bundles

Create a self-contained Linux bundle with:

```text
bash deploy/package-release.sh
```

The package script derives the version from `crates/stats-cli/Cargo.toml` unless
`STATS_VERSION` is supplied. A tag value such as `v0.1.2` is normalized to
`0.1.2`. `STATS_PLATFORM` and `STATS_RELEASE_DIR` are also supported. The bundle
contains the optimized binary, deployment templates, example configuration,
README, and changelog, plus a checksum manifest whose paths are valid from the
artifact directory.

## HTTP and CGI boundaries

For CGI and HTTP deployments, the effective `DirData` directory is the only
request-readable root. Requested database and `urlalias` files are canonicalized
before access, so traversal and symlink escapes are rejected. External renderer
libraries are trusted startup inputs from deployment configuration and are not
request-controlled.

Report failures return a generic HTTP 500 response or a generic CGI 500 response
with valid CGI headers. Detailed diagnostics remain server-side.

The HTTP frontend has a disabled-by-default `/raw-log` diagnostic endpoint; CGI
supports `output=raw-log`. Set a non-empty `STATS_RAW_LOG_AUTH_TOKEN` and have a
reverse proxy inject `Authorization: Bearer <token>`. Both paths return only
redacted records, limit requests to 10,000 lines, support optional line ranges,
and read the configured log only when it is inside `DirData`.

See [Operations](operations.md) for rotation, retention, backups, and migration.
