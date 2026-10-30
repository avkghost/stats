use anyhow::{bail, Context};
use bzip2::read::BzDecoder;
use clap::{Parser, Subcommand, ValueEnum};
use flate2::read::GzDecoder;
use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use stats_core::{
    accepts, hashfiles_backend, is_page, render_redacted_raw_log, Config, DatabasePeriod,
    GeoIpEnricher, InputCheckpoint, LogParser, MaxMindGeoIp, MethodAnomalyMatcher, ParseOutcome,
    PluginRegistry, RefererSpamMatcher, ReportService, ScannerMatcher, StatsStore,
};
use stats_plugin_api::{HostApi, PluginEvent, ABI_VERSION};
use stats_plugin_host::LoadedPlugin;
use stats_render::{
    builtin_renderer_registry, register_configured_renderer, render_report_stream, write_html_site,
    write_html_site_with_periods,
};
use stats_tui::run as run_tui;
use std::{
    collections::{hash_map::DefaultHasher, HashMap},
    ffi::CString,
    fs,
    hash::{Hash, Hasher},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command as ProcessCommand, Stdio},
    sync::mpsc,
};
use tokio::time::{self, Duration};
use xz2::read::XzDecoder;

mod reverse_dns;

#[derive(Parser)]
#[command(name = "stats", version, about = "AWStats-style Rust web log analyzer")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ReportFormat {
    Html,
    Json,
    Csv,
}

impl ReportFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Json => "json",
            Self::Csv => "csv",
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Append the explicitly supplied log file and regenerate the cumulative HTML site.
    Update {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        log_file: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Clear derived aggregates and replay the explicitly supplied log from scratch.
    Rebuild {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        log_file: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Append every matching rotated log in chronological order and regenerate the cumulative site once.
    Backfill {
        #[arg(short, long)]
        config: PathBuf,
        /// Directory containing the active log and its rotated variants.
        #[arg(long)]
        log_dir: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    Report {
        /// Configuration containing the requested external report renderer.
        #[arg(short, long)]
        config: Option<PathBuf>,
        #[arg(short, long)]
        data_dir: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long, value_enum, default_value_t = ReportFormat::Html)]
        format: ReportFormat,
        /// Restrict a partitioned report to one period such as 2026-10 or 2026.
        #[arg(long)]
        period: Option<String>,
    },
    /// Export a bounded, redacted slice of a configured log for local administration.
    RawLog {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        log_file: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, default_value_t = 0)]
        start: usize,
        #[arg(long, default_value_t = 1000)]
        lines: usize,
    },
    /// Update SQLite statistics for the control and all period databases.
    Optimize {
        #[arg(short, long)]
        data_dir: PathBuf,
        /// Rebuild SQLite databases to reclaim free space; this can be slow.
        #[arg(long)]
        vacuum: bool,
    },
    /// Show control and period database sizes.
    Storage {
        #[arg(short, long)]
        data_dir: PathBuf,
    },
    /// Create consistent snapshots of the control and all period databases.
    Backup {
        #[arg(short, long)]
        data_dir: PathBuf,
        #[arg(short, long)]
        output_dir: PathBuf,
    },
    /// List old partitions, or snapshot and optionally remove them.
    Retention {
        #[arg(short, long)]
        data_dir: PathBuf,
        /// Archive partitions strictly older than YYYY-MM or YYYY.
        #[arg(long)]
        before: String,
        /// Destination root for consistent archived database snapshots.
        #[arg(long)]
        archive_dir: Option<PathBuf>,
        /// Remove source partitions after every archive snapshot succeeds.
        #[arg(long)]
        remove_source: bool,
    },
    /// Back up existing data and replay all logs into the configured partition layout.
    Migrate {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        log_dir: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// New backup directory; it must not already exist.
        #[arg(long)]
        backup_dir: PathBuf,
    },
    /// Print a stable, terminal-friendly overview suitable for a future TUI.
    Summary {
        #[arg(short, long)]
        data_dir: PathBuf,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Refresh the summary until Ctrl-C is pressed.
        #[arg(long)]
        watch: bool,
        #[arg(long, default_value_t = 5)]
        interval: u64,
    },
    /// Incrementally update a log and redraw its terminal summary.
    Live {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        log_file: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long, default_value_t = 5)]
        interval: u64,
    },
    /// Run the interactive terminal dashboard.
    Tui {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        log_file: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 10)]
        limit: usize,
        #[arg(long, default_value_t = 5)]
        interval: u64,
    },
    Validate {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        log_file: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Verify configuration, GeoIP paths, and existing SQLite databases without mutation.
    Health {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Emit a machine-readable JSON result.
        #[arg(long)]
        json: bool,
    },
    Watch {
        #[arg(short, long)]
        config: PathBuf,
        #[arg(long)]
        log_file: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 5)]
        interval: u64,
    },
    #[command(name = "plugin-worker", hide = true)]
    PluginWorker {
        #[arg(long)]
        plugin: PathBuf,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Update {
            config,
            log_file,
            data_dir,
        } => tokio::task::spawn_blocking(move || update(config, log_file, data_dir)).await??,
        Command::Rebuild {
            config,
            log_file,
            data_dir,
        } => tokio::task::spawn_blocking(move || rebuild(config, log_file, data_dir)).await??,
        Command::Backfill {
            config,
            log_dir,
            data_dir,
        } => tokio::task::spawn_blocking(move || backfill(config, log_dir, data_dir)).await??,
        Command::RawLog {
            config,
            log_file,
            output,
            start,
            lines,
        } => {
            tokio::task::spawn_blocking(move || raw_log(config, log_file, output, start, lines))
                .await??
        }
        Command::Report {
            config,
            data_dir,
            output,
            limit,
            format,
            period,
        } => {
            tokio::task::spawn_blocking(move || {
                report(config, data_dir, output, limit, format, period)
            })
            .await??
        }
        Command::Optimize { data_dir, vacuum } => {
            tokio::task::spawn_blocking(move || optimize(data_dir, vacuum)).await??
        }
        Command::Storage { data_dir } => {
            tokio::task::spawn_blocking(move || storage(data_dir)).await??
        }
        Command::Backup {
            data_dir,
            output_dir,
        } => tokio::task::spawn_blocking(move || backup(data_dir, output_dir)).await??,
        Command::Retention {
            data_dir,
            before,
            archive_dir,
            remove_source,
        } => {
            tokio::task::spawn_blocking(move || {
                retention(data_dir, before, archive_dir, remove_source)
            })
            .await??
        }
        Command::Migrate {
            config,
            log_dir,
            data_dir,
            backup_dir,
        } => {
            tokio::task::spawn_blocking(move || migrate(config, log_dir, data_dir, backup_dir))
                .await??
        }
        Command::Summary {
            data_dir,
            limit,
            watch,
            interval,
        } => {
            if watch {
                summary_watch(data_dir, limit, interval).await?
            } else {
                tokio::task::spawn_blocking(move || summary(data_dir, limit)).await??
            }
        }
        Command::Live {
            config,
            log_file,
            data_dir,
            limit,
            interval,
        } => live(config, log_file, data_dir, limit, interval).await?,
        Command::Tui {
            config,
            log_file,
            data_dir,
            limit,
            interval,
        } => {
            tokio::task::spawn_blocking(move || tui(config, log_file, data_dir, limit, interval))
                .await??
        }
        Command::Validate {
            config,
            log_file,
            data_dir,
        } => validate(config, log_file, data_dir)?,
        Command::Health {
            config,
            data_dir,
            json,
        } => tokio::task::spawn_blocking(move || health(config, data_dir, json)).await??,
        Command::Watch {
            config,
            log_file,
            data_dir,
            interval,
        } => watch(config, log_file, data_dir, interval).await?,
        Command::PluginWorker { plugin } => plugin_worker(plugin)?,
    }
    Ok(())
}

async fn watch(
    config_path: PathBuf,
    log_override: Option<PathBuf>,
    data_override: Option<PathBuf>,
    interval_seconds: u64,
) -> anyhow::Result<()> {
    let effective = Config::from_path(&config_path)
        .with_context(|| format!("load configuration {}", config_path.display()))?
        .with_overrides(log_override.clone(), data_override.clone());
    tokio::task::spawn_blocking({
        let config_path = config_path.clone();
        let log_override = log_override.clone();
        let data_override = data_override.clone();
        move || update(config_path, log_override, data_override)
    })
    .await??;

    let parent = effective
        .log_file
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let (notify_tx, notify_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(notify_tx)?;
    watcher.watch(parent, RecursiveMode::NonRecursive)?;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(32);
    tokio::task::spawn_blocking(move || {
        while let Ok(event) = notify_rx.recv() {
            if event_tx.blocking_send(event).is_err() {
                break;
            }
        }
    });

    println!("watching {} (Ctrl-C to stop)", effective.log_file.display());
    let mut dirty = false;
    let mut ticker = time::interval(Duration::from_secs(interval_seconds.max(1)));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            Some(_event) = event_rx.recv() => dirty = true,
            _ = ticker.tick(), if dirty => {
                dirty = false;
                tokio::task::spawn_blocking({
                    let config_path = config_path.clone();
                    let log_override = log_override.clone();
                    let data_override = data_override.clone();
                    move || update(config_path, log_override, data_override)
                }).await??;
            }
        }
    }
    Ok(())
}

fn raw_log(
    config_path: PathBuf,
    log_override: Option<PathBuf>,
    output: Option<PathBuf>,
    start: usize,
    lines: usize,
) -> anyhow::Result<()> {
    let config = Config::from_path(&config_path)
        .with_context(|| format!("load configuration {}", config_path.display()))?;
    let path = log_override.unwrap_or(config.log_file);
    validate_raw_log_output(&path, output.as_deref())?;
    let body = render_redacted_raw_log(&path, start, lines)?;
    let mut writer: Box<dyn Write> = match output {
        Some(path) => {
            let mut options = fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options
                .open(&path)
                .with_context(|| format!("open redacted raw-log output {}", path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(0o600))
                    .with_context(|| format!("restrict raw-log output {}", path.display()))?;
            }
            Box::new(file)
        }
        None => Box::new(io::stdout().lock()),
    };
    writer.write_all(body.as_bytes())?;
    Ok(())
}

fn validate_raw_log_output(source: &Path, output: Option<&Path>) -> anyhow::Result<()> {
    let Some(output) = output else {
        return Ok(());
    };
    if fs::symlink_metadata(output)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        bail!("refusing symlink raw-log output {}", output.display());
    }
    if output.exists() && fs::canonicalize(output)? == fs::canonicalize(source)? {
        bail!(
            "raw-log output must differ from source {}",
            source.display()
        );
    }
    Ok(())
}

fn update(
    config_path: PathBuf,
    log_override: Option<PathBuf>,
    data_override: Option<PathBuf>,
) -> anyhow::Result<()> {
    update_inner(
        config_path.clone(),
        log_override.clone(),
        data_override.clone(),
        false,
    )?;
    regenerate_default_report(&config_path, log_override, data_override, false)
}

fn backfill(
    config_path: PathBuf,
    log_dir_override: Option<PathBuf>,
    data_override: Option<PathBuf>,
) -> anyhow::Result<()> {
    let base_config = Config::from_path(&config_path)
        .with_context(|| format!("load configuration {}", config_path.display()))?;
    let active_path = base_config
        .log_file
        .file_name()
        .map(PathBuf::from)
        .context("configured LogFile must include a file name")?;
    let log_dir = log_dir_override.unwrap_or_else(|| {
        base_config
            .log_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    });
    let files = discover_backfill_files(&log_dir, &active_path)?;
    if files.is_empty() {
        bail!(
            "no log files matching {} found in {}",
            active_path.display(),
            log_dir.display()
        );
    }
    println!(
        "backfilling {} log files from {}",
        files.len(),
        log_dir.display()
    );
    for path in &files {
        println!("processing {}", path.display());
        update_inner(
            config_path.clone(),
            Some(path.clone()),
            data_override.clone(),
            false,
        )?;
    }
    regenerate_default_report(&config_path, None, data_override, false)
}

fn discover_backfill_files(log_dir: &Path, active_name: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let active_name = active_name
        .to_str()
        .context("configured LogFile name is not valid UTF-8")?;
    let mut files = Vec::new();
    for entry in fs::read_dir(log_dir)
        .with_context(|| format!("read log directory {}", log_dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("read entry in log directory {}", log_dir.display()))?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let rank = if name == active_name {
            Some(0u64)
        } else {
            let uncompressed = name
                .strip_suffix(".gz")
                .or_else(|| name.strip_suffix(".bz2"))
                .or_else(|| name.strip_suffix(".xz"))
                .unwrap_or(&name);
            uncompressed
                .strip_prefix(&format!("{active_name}."))
                .and_then(|suffix| suffix.parse::<u64>().ok())
        };
        if let Some(rank) = rank {
            files.push((rank, entry.path()));
        }
    }
    files.sort_by(|(left_rank, left_path), (right_rank, right_path)| {
        right_rank
            .cmp(left_rank)
            .then_with(|| left_path.cmp(right_path))
    });
    Ok(files.into_iter().map(|(_, path)| path).collect())
}

fn regenerate_default_report(
    config_path: &Path,
    log_override: Option<PathBuf>,
    data_override: Option<PathBuf>,
    quiet: bool,
) -> anyhow::Result<()> {
    let config = Config::from_path(config_path)
        .with_context(|| format!("load configuration {}", config_path.display()))?
        .with_overrides(log_override, data_override);
    let mut report = build_report_for_data_dir(&config.data_dir, 50)?;
    report.apply_url_aliases(&config.url_aliases()?);
    report.apply_cluster_info(&config.cluster_info()?);
    let output = config
        .data_dir
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("output/index.html");
    let external_html = config
        .plugins
        .iter()
        .any(|plugin| stats_render::report_plugin_format(plugin).as_deref() == Some("html"));
    if external_html {
        let mut registry = builtin_renderer_registry()
            .map_err(|error| anyhow::anyhow!("register report renderers: {error}"))?;
        register_configured_renderer(&mut registry, &config, "html")
            .with_context(|| format!("load HTML renderer from {}", config_path.display()))?;
        let body = render_report_stream(&registry, &report, "html")?;
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create report directory {}", parent.display()))?;
        }
        fs::write(&output, body)
            .with_context(|| format!("write external HTML report {}", output.display()))?;
    } else {
        write_report_site(&config.data_dir, &output, &report, 50)?;
    }
    if !quiet {
        if external_html {
            println!("wrote external HTML report {}", output.display());
        } else {
            println!("wrote cumulative report site {}", output.display());
        }
    }
    Ok(())
}

struct StoreRouter {
    control: StatsStore,
    partitions: HashMap<String, StatsStore>,
    data_dir: PathBuf,
    period: DatabasePeriod,
}

impl StoreRouter {
    fn open(data_dir: &Path, period: DatabasePeriod) -> anyhow::Result<Self> {
        fs::create_dir_all(data_dir)
            .with_context(|| format!("create data directory {}", data_dir.display()))?;
        let db_path = data_dir.join("stats.sqlite3");
        let control = StatsStore::open(&db_path)
            .with_context(|| format!("open statistics database {}", db_path.display()))?;
        Ok(Self {
            control,
            partitions: HashMap::new(),
            data_dir: data_dir.to_path_buf(),
            period,
        })
    }

    fn partition_key(&self, event: &stats_core::AccessEvent) -> Option<String> {
        match self.period {
            DatabasePeriod::None => None,
            DatabasePeriod::Month => Some(event.timestamp.format("%Y-%m").to_string()),
            DatabasePeriod::Year => Some(event.timestamp.format("%Y").to_string()),
        }
    }

    fn partition_store(&mut self, key: &str) -> anyhow::Result<&mut StatsStore> {
        if !self.partitions.contains_key(key) {
            let dir = self.data_dir.join("periods").join(key);
            fs::create_dir_all(&dir)
                .with_context(|| format!("create period data directory {}", dir.display()))?;
            let db_path = dir.join("stats.sqlite3");
            let store = StatsStore::open(&db_path)
                .with_context(|| format!("open period database {}", db_path.display()))?;
            self.partitions.insert(key.to_owned(), store);
        }
        self.partitions
            .get_mut(key)
            .ok_or_else(|| anyhow::anyhow!("period store {} was not inserted", key))
    }

    fn record(&mut self, event: &stats_core::AccessEvent, page: bool) -> anyhow::Result<()> {
        if let Some(key) = self.partition_key(event) {
            let shared_state = self.control.session_state_for_event(event)?;
            {
                let partition = self.partition_store(&key)?;
                if let Some(state) = &shared_state {
                    partition.seed_session_state(event, state)?;
                }
                partition.record(event, page)?;
                if let Some(state) = partition.session_state_for_event(event)? {
                    self.control.merge_session_state(event, &state)?;
                }
            }
        } else {
            self.control.record(event, page)?;
        }
        Ok(())
    }

    fn diagnostic(&self, message: &str) -> anyhow::Result<()> {
        self.control.diagnostic(message)?;
        Ok(())
    }

    fn clear_aggregates(&mut self) -> anyhow::Result<()> {
        self.control.clear_aggregates()?;
        if self.period != DatabasePeriod::None {
            let periods = self.data_dir.join("periods");
            if periods.is_dir() {
                for entry in fs::read_dir(&periods)? {
                    let entry = entry?;
                    let db_path = entry.path().join("stats.sqlite3");
                    if db_path.is_file() {
                        StatsStore::open(&db_path)?.clear_aggregates()?;
                    }
                }
            }
            self.partitions.clear();
        }
        Ok(())
    }

    fn meta(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self.control.meta(key)?)
    }
    fn reset_checkpoint(&self) -> anyhow::Result<()> {
        Ok(self.control.reset_checkpoint()?)
    }
    fn input_checkpoint_by_identity(
        &self,
        identity: &str,
        config_hash: &str,
    ) -> anyhow::Result<Option<InputCheckpoint>> {
        Ok(self
            .control
            .input_checkpoint_by_identity(identity, config_hash)?)
    }
    fn input_checkpoint_by_content_and_path(
        &self,
        content_hash: &str,
        config_hash: &str,
        path: &str,
    ) -> anyhow::Result<Option<InputCheckpoint>> {
        Ok(self
            .control
            .input_checkpoint_by_content_and_path(content_hash, config_hash, path)?)
    }
    fn set_input_checkpoint(&self, checkpoint: &InputCheckpoint) -> anyhow::Result<()> {
        Ok(self.control.set_input_checkpoint(checkpoint)?)
    }
    fn set_meta(&self, key: &str, value: &str) -> anyhow::Result<()> {
        Ok(self.control.set_meta(key, value)?)
    }
}

fn record_event_batch(
    events: &mut Vec<stats_core::AccessEvent>,
    reverse_dns: Option<&mut reverse_dns::ReverseDnsEnricher>,
    store: &mut StoreRouter,
    filters: &stats_core::FilterConfig,
) -> anyhow::Result<u64> {
    if let Some(enricher) = reverse_dns {
        let _ = enricher.enrich_batch(events);
    }
    let count = events.len() as u64;
    for event in events.drain(..) {
        store.record(&event, is_page(&event, filters))?;
    }
    Ok(count)
}

fn update_inner(
    config_path: PathBuf,
    log_override: Option<PathBuf>,
    data_override: Option<PathBuf>,
    quiet: bool,
) -> anyhow::Result<()> {
    let config = Config::from_path(&config_path)
        .with_context(|| format!("load configuration {}", config_path.display()))?
        .with_overrides(log_override, data_override);
    let timehires_enabled = config
        .plugins
        .iter()
        .any(|plugin| plugin.name == "timehires");
    let update_started = std::time::Instant::now();
    let config_hash = hash_bytes(
        &fs::read(&config_path)
            .with_context(|| format!("read configuration {}", config_path.display()))?,
    );
    if !quiet {
        for warning in &config.warnings {
            eprintln!("warning: {}: {warning}", config_path.display());
        }
    }
    let db_path = config.data_dir.join("stats.sqlite3");
    let mut store = StoreRouter::open(&config.data_dir, config.database_period)?;
    if hashfiles_backend(&config.plugins).is_some() {
        store.set_meta("hashfiles_backend", "sqlite")?;
    }
    let log_files = vec![resolve_log_file(&config_path, config.log_file.clone())];
    let parser = LogParser::new(&config.log_format).context("compile LogFormat")?;
    let (country, city, asn) = plugin_paths(&config.plugins);
    let geoip = MaxMindGeoIp::open(country.as_deref(), city.as_deref(), asn.as_deref())
        .context("initialize configured GeoIP databases")?;
    let mut registry = PluginRegistry::new();
    registry.add(GeoIpEnricher {
        provider: Box::new(geoip),
    });
    let mut reverse_dns = if config.dns_lookup {
        Some(
            reverse_dns::ReverseDnsEnricher::new(
                config.dns_lookup_timeout_ms,
                config.dns_lookup_cache_size,
            )
            .context("initialize reverse DNS enrichment")?,
        )
    } else {
        None
    };
    let referer_spam = configured_referer_spam(&config_path, &config)?;
    let scanner = configured_scanner(&config_path, &config)?;
    let method_anomaly = configured_method_anomaly(&config);
    let previous_config_hash = store.meta("config_hash")?;
    if previous_config_hash
        .as_deref()
        .is_some_and(|hash| hash != config_hash)
    {
        store.clear_aggregates()?;
        store.reset_checkpoint()?;
    }
    let legacy_hash = store.meta("input_content_hash")?;
    let legacy_path = store.meta("input_path")?;
    let legacy_size = store
        .meta("input_size")?
        .and_then(|value| value.parse::<u64>().ok());
    let legacy_lines = store
        .meta("input_lines")?
        .and_then(|value| value.parse::<u64>().ok());
    let mut native_plugins = load_native_plugins(&config.plugins, quiet);
    let mut processed = 0u64;
    let mut events = 0u64;
    let mut diagnostics = 0u64;
    let mut filtered = 0u64;
    let mut active_checkpoint = None;
    for path in log_files {
        let metadata =
            fs::metadata(&path).with_context(|| format!("inspect log file {}", path.display()))?;
        let compression = path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        let compressed = matches!(compression.as_deref(), Some("gz" | "bz2" | "xz"));
        let raw = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let content_hash = hash_bytes(&raw);
        let parsed = decode_log_bytes(&path, &raw)?;
        let logical_hash = hash_bytes(&parsed);
        let text = String::from_utf8_lossy(&parsed);
        let lines: Vec<&str> = text.lines().collect();
        let identity = file_identity(&metadata);
        let path_string = path.display().to_string();
        let mut checkpoint = store.input_checkpoint_by_identity(&identity, &config_hash)?;
        if checkpoint.is_none() {
            checkpoint = store.input_checkpoint_by_content_and_path(
                &content_hash,
                &config_hash,
                &path_string,
            )?;
        }
        if checkpoint.is_none() {
            checkpoint = store.input_checkpoint_by_content_and_path(
                &logical_hash,
                &config_hash,
                &path_string,
            )?;
        }

        if checkpoint.is_none()
            && !compressed
            && legacy_path
                .as_deref()
                .is_none_or(|previous| previous == path_string)
            && legacy_hash.as_deref() == Some(&logical_hash)
            && legacy_size == Some(raw.len() as u64)
            && legacy_lines == Some(lines.len() as u64)
        {
            checkpoint = Some(InputCheckpoint {
                identity: identity.clone(),
                path: path.display().to_string(),
                config_hash: config_hash.clone(),
                content_hash: logical_hash.clone(),
                size: raw.len() as u64,
                lines: lines.len() as u64,
            });
        }
        if checkpoint.is_none()
            && !compressed
            && legacy_path
                .as_deref()
                .is_none_or(|previous| previous == path_string)
        {
            if let (Some(size), Some(content_hash), Some(line_count)) =
                (legacy_size, legacy_hash.as_deref(), legacy_lines)
            {
                let prefix_matches = raw.len() as u64 >= size
                    && lines.len() as u64 >= line_count
                    && hash_bytes(&raw[..size as usize]) == content_hash;
                if prefix_matches {
                    checkpoint = Some(InputCheckpoint {
                        identity: identity.clone(),
                        path: path.display().to_string(),
                        config_hash: config_hash.clone(),
                        content_hash: content_hash.to_owned(),
                        size,
                        lines: line_count,
                    });
                }
            }
        }
        let skip_lines = checkpoint
            .as_ref()
            .map(|previous| {
                if previous.content_hash == logical_hash {
                    lines.len()
                } else if !compressed
                    && raw.len() as u64 >= previous.size
                    && lines.len() as u64 >= previous.lines
                    && hash_bytes(&raw[..previous.size as usize]) == previous.content_hash
                {
                    previous.lines as usize
                } else {
                    0
                }
            })
            .unwrap_or(0);
        let is_active = path == config.log_file;
        if is_active {
            active_checkpoint = Some((
                path.clone(),
                metadata.clone(),
                raw.clone(),
                logical_hash.clone(),
                lines.len(),
            ));
        }
        let current_checkpoint = InputCheckpoint {
            identity: identity.clone(),
            path: path.display().to_string(),
            config_hash: config_hash.clone(),
            content_hash: logical_hash.clone(),
            size: raw.len() as u64,
            lines: lines.len() as u64,
        };
        if skip_lines == lines.len() {
            store.set_input_checkpoint(&current_checkpoint)?;
            continue;
        }
        if std::str::from_utf8(&parsed).is_err() && skip_lines == 0 {
            store.diagnostic(&format!(
                "{}: input contains invalid UTF-8; replacement characters were used",
                path.display()
            ))?;
            diagnostics += 1;
        }
        let mut event_batch = Vec::with_capacity(256);
        for line in lines.iter().skip(skip_lines) {
            processed += 1;
            match parser.parse(line) {
                ParseOutcome::Event(mut event) => {
                    if let Some(time_zone) = config.time_zone {
                        event.apply_timezone_offset(time_zone);
                    }
                    if !accepts(&event, &config.filters) {
                        filtered += 1;
                        continue;
                    }
                    registry.enrich(&mut event);
                    if let Some(matcher) = scanner.as_ref() {
                        matcher.apply(&mut event);
                    }
                    if let Some(matcher) = referer_spam.as_ref() {
                        matcher.apply(&mut event);
                    }
                    if let Some(matcher) = method_anomaly.as_ref() {
                        matcher.apply(&mut event);
                    }
                    if let Err(error) = enrich_with_native_plugins(&mut event, &mut native_plugins)
                    {
                        store.diagnostic(&format!("native plugin enrichment disabled: {error}"))?;
                    }
                    if !config.url_referrer_with_query {
                        event.referer = event.referer.take().map(|referer| {
                            referer.split('?').next().unwrap_or_default().to_owned()
                        });
                    }
                    event_batch.push(*event);
                    if event_batch.len() >= 256 {
                        events += record_event_batch(
                            &mut event_batch,
                            reverse_dns.as_mut(),
                            &mut store,
                            &config.filters,
                        )?;
                    }
                }
                ParseOutcome::Diagnostic(error) => {
                    store.diagnostic(&format!("{}: {error}", path.display()))?;
                    diagnostics += 1;
                }
            }
        }
        events += record_event_batch(
            &mut event_batch,
            reverse_dns.as_mut(),
            &mut store,
            &config.filters,
        )?;
        store.set_input_checkpoint(&current_checkpoint)?;
    }
    let Some((active_path, active_metadata, active_raw, active_hash, active_lines)) =
        active_checkpoint
    else {
        bail!(
            "active log file {} was not discovered",
            config.log_file.display()
        );
    };
    let modified = active_metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_secs())
        .unwrap_or_default();
    let fingerprint = format!(
        "{}:{}:{}:{}:{}:{}",
        active_path.display(),
        config_hash,
        file_identity(&active_metadata),
        active_metadata.len(),
        modified,
        active_metadata
            .modified()
            .ok()
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.subsec_nanos())
            .unwrap_or_default()
    );
    store.set_meta("input_fingerprint", &fingerprint)?;
    store.set_meta("input_content_hash", &active_hash)?;
    store.set_meta("input_path", &active_path.display().to_string())?;
    store.set_meta("config_path", &config_path.display().to_string())?;
    store.set_meta("config_hash", &config_hash)?;
    store.set_meta("input_size", &(active_raw.len() as u64).to_string())?;
    store.set_meta("input_lines", &active_lines.to_string())?;
    if !quiet {
        if processed == 0 && events == 0 && diagnostics == 0 && filtered == 0 {
            if timehires_enabled {
                println!(
                    "unchanged input; no update required (elapsed_ms={})",
                    update_started.elapsed().as_millis()
                );
            } else {
                println!("unchanged input; no update required");
            }
        } else {
            if timehires_enabled {
                println!("processed={processed} events={events} diagnostics={diagnostics} filtered={filtered} elapsed_ms={} database={}", update_started.elapsed().as_millis(), db_path.display());
            } else {
                println!("processed={processed} events={events} diagnostics={diagnostics} filtered={filtered} database={}", db_path.display());
            }
        }
    }
    Ok(())
}

fn rebuild(
    config_path: PathBuf,
    log_override: Option<PathBuf>,
    data_override: Option<PathBuf>,
) -> anyhow::Result<()> {
    let config = Config::from_path(&config_path)
        .with_context(|| format!("load configuration {}", config_path.display()))?
        .with_overrides(log_override.clone(), data_override.clone());
    let db_path = config.data_dir.join("stats.sqlite3");
    let mut store = StoreRouter::open(&config.data_dir, config.database_period)?;
    store.clear_aggregates()?;
    if store.control.totals()?.0 != 0 {
        bail!(
            "rebuild could not clear aggregates in {}",
            db_path.display()
        );
    }
    store.reset_checkpoint()?;
    drop(store);
    update(config_path, log_override, data_override)
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn decode_log_bytes(path: &Path, raw: &[u8]) -> anyhow::Result<Vec<u8>> {
    let compression = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let Some(compression) = compression.as_deref() else {
        return Ok(raw.to_vec());
    };
    let mut decoded = Vec::new();
    match compression {
        "gz" => GzDecoder::new(raw)
            .read_to_end(&mut decoded)
            .with_context(|| format!("decompress gzip log {}", path.display()))?,
        "bz2" => BzDecoder::new(raw)
            .read_to_end(&mut decoded)
            .with_context(|| format!("decompress bzip2 log {}", path.display()))?,
        "xz" => XzDecoder::new(raw)
            .read_to_end(&mut decoded)
            .with_context(|| format!("decompress xz log {}", path.display()))?,
        _ => return Ok(raw.to_vec()),
    };
    Ok(decoded)
}

fn file_identity(metadata: &fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}", metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        "portable".to_owned()
    }
}

struct WorkerPlugin {
    name: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    disabled: bool,
}

impl WorkerPlugin {
    fn start(name: &str, path: &str) -> anyhow::Result<Self> {
        let mut child = ProcessCommand::new(std::env::current_exe()?)
            .arg("plugin-worker")
            .arg("--plugin")
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawn native plugin worker {name} from {path}"))?;
        let stdin = child
            .stdin
            .take()
            .context("native plugin worker stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("native plugin worker stdout unavailable")?;
        Ok(Self {
            name: name.to_owned(),
            child,
            stdin,
            stdout: BufReader::new(stdout),
            disabled: false,
        })
    }

    fn enrich(&mut self, event: &mut stats_core::AccessEvent) -> anyhow::Result<()> {
        if self.disabled {
            return Ok(());
        }
        let request = serde_json::json!({
            "client_ip": event.client_ip,
            "method": event.method.as_deref().unwrap_or("UNKNOWN"),
            "url": event.url.as_deref().unwrap_or("UNKNOWN"),
            "status": event.status,
            "bytes": event.bytes,
        });
        writeln!(self.stdin, "{}", request)?;
        self.stdin.flush()?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mut pollfd = libc::pollfd {
                fd: self.stdout.get_ref().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let result = unsafe { libc::poll(&mut pollfd, 1, 500) };
            if result == 0 {
                self.disabled = true;
                let _ = self.child.kill();
                return Err(anyhow::anyhow!("native plugin worker timed out"));
            }
            if result < 0 {
                self.disabled = true;
                let _ = self.child.kill();
                return Err(anyhow::anyhow!(
                    "poll native plugin worker: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            self.disabled = true;
            return Err(anyhow::anyhow!(
                "native plugin worker exited without a response"
            ));
        }
        let response: serde_json::Value =
            serde_json::from_str(&line).context("decode native plugin worker response")?;
        if let Some(error) = response.get("error").and_then(|value| value.as_str()) {
            return Err(anyhow::anyhow!("{error}"));
        }
        event.status = response
            .get("status")
            .and_then(|value| value.as_u64())
            .context("native plugin response missing status")? as u16;
        event.bytes = response
            .get("bytes")
            .and_then(|value| value.as_u64())
            .context("native plugin response missing bytes")?;
        Ok(())
    }
}

impl Drop for WorkerPlugin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn load_native_plugins(plugins: &[stats_core::PluginConfig], quiet: bool) -> Vec<WorkerPlugin> {
    let mut loaded = Vec::new();
    for plugin in plugins {
        if stats_render::report_plugin_format(plugin).is_some() {
            continue;
        }
        let Some(path) = plugin
            .args
            .iter()
            .find(|arg| arg.ends_with(std::env::consts::DLL_SUFFIX))
        else {
            continue;
        };
        match WorkerPlugin::start(&plugin.name, path) {
            Ok(native) => {
                if !quiet {
                    println!("started native plugin worker {} from {}", plugin.name, path);
                }
                loaded.push(native);
            }
            Err(error) if !quiet => eprintln!(
                "warning: cannot start native plugin {}: {error:#}",
                plugin.name
            ),
            Err(_) => {}
        }
    }
    loaded
}

fn enrich_with_native_plugins(
    event: &mut stats_core::AccessEvent,
    plugins: &mut [WorkerPlugin],
) -> anyhow::Result<()> {
    if plugins.is_empty() {
        return Ok(());
    }
    for plugin in plugins {
        if let Err(error) = plugin.enrich(event) {
            plugin.disabled = true;
            return Err(anyhow::anyhow!("plugin {}: {error:#}", plugin.name));
        }
    }
    Ok(())
}

fn plugin_worker(path: PathBuf) -> anyhow::Result<()> {
    let host_api = HostApi {
        abi_version: ABI_VERSION,
        log: None,
    };
    let plugin = unsafe { LoadedPlugin::load(&path, &host_api) }
        .with_context(|| format!("load native plugin worker library {}", path.display()))?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::BufWriter::new(std::io::stdout());
    for line in stdin.lock().lines() {
        let line = line?;
        let request: serde_json::Value =
            serde_json::from_str(&line).context("decode native plugin worker request")?;
        let client_ip = CString::new(
            request
                .get("client_ip")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN"),
        )?;
        let method = CString::new(
            request
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN"),
        )?;
        let url = CString::new(
            request
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN"),
        )?;
        let mut event = PluginEvent {
            client_ip: client_ip.as_ptr(),
            method: method.as_ptr(),
            url: url.as_ptr(),
            status: request.get("status").and_then(|v| v.as_u64()).unwrap_or(0) as u16,
            bytes: request.get("bytes").and_then(|v| v.as_u64()).unwrap_or(0),
        };
        let response = match plugin.enrich(&mut event) {
            Ok(()) => serde_json::json!({"status": event.status, "bytes": event.bytes}),
            Err(error) => serde_json::json!({"error": error.to_string()}),
        };
        writeln!(stdout, "{}", response)?;
        stdout.flush()?;
    }
    Ok(())
}

fn plugin_paths(
    plugins: &[stats_core::PluginConfig],
) -> (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) {
    let mut country = None;
    let mut city = None;
    let mut asn = None;
    for plugin in plugins {
        let path = plugin.args.last().map(PathBuf::from);
        match plugin.name.as_str() {
            "geoip2_country" | "geoip" | "geoipfree" | "geoip6" => country = path,
            "geoip2_city" | "geoip2_region" => city = path,
            name if name.contains("asn") => asn = path,
            _ => {}
        }
    }
    (country, city, asn)
}

fn resolve_log_file(config_path: &Path, configured: PathBuf) -> PathBuf {
    if configured.is_absolute() || configured.exists() {
        return configured;
    }
    config_path
        .parent()
        .map(|parent| parent.join(&configured))
        .filter(|candidate| candidate.exists())
        .unwrap_or(configured)
}

fn configured_referer_spam(
    config_path: &Path,
    config: &Config,
) -> anyhow::Result<Option<RefererSpamMatcher>> {
    if !config
        .plugins
        .iter()
        .any(|plugin| plugin.name == "detectrefererspam")
    {
        return Ok(None);
    }
    config
        .referer_spam_rules
        .as_ref()
        .map(|path| {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                config_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(path)
            };
            RefererSpamMatcher::from_path(path)
        })
        .transpose()
        .context("load referer-spam rules")
}

fn configured_scanner(
    config_path: &Path,
    config: &Config,
) -> anyhow::Result<Option<ScannerMatcher>> {
    if !config
        .plugins
        .iter()
        .any(|plugin| plugin.name == "detectscanner")
    {
        return Ok(None);
    }
    config
        .scanner_rules
        .as_ref()
        .map(|path| {
            let path = if path.is_absolute() {
                path.clone()
            } else {
                config_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(path)
            };
            ScannerMatcher::from_path(path)
        })
        .transpose()
        .context("load scanner rules")
}

fn configured_method_anomaly(config: &Config) -> Option<MethodAnomalyMatcher> {
    config
        .plugins
        .iter()
        .any(|plugin| plugin.name == "detecthttpmethodanomaly")
        .then(|| MethodAnomalyMatcher::new(config.http_method_anomaly_allowed.clone()))
}

fn report(
    config_path: Option<PathBuf>,
    data_dir: PathBuf,
    output: PathBuf,
    limit: usize,
    format: ReportFormat,
    period: Option<String>,
) -> anyhow::Result<()> {
    let mut report = build_report_for_period(&data_dir, limit, period.as_deref())?;
    let mut renderer_registry = builtin_renderer_registry()
        .map_err(|error| anyhow::anyhow!("register built-in report renderers: {error}"))?;
    let config = config_path
        .as_deref()
        .map(Config::from_path)
        .transpose()
        .map_err(|error| anyhow::anyhow!("load renderer configuration: {error}"))?;
    if matches!(format, ReportFormat::Html) {
        if let Some(config) = config.as_ref() {
            let config = config.clone().with_overrides(None, Some(data_dir.clone()));
            report.apply_url_aliases(&config.url_aliases()?);
            report.apply_cluster_info(&config.cluster_info()?);
        }
    }
    let external_renderer_configured = config.as_ref().is_some_and(|config| {
        config.plugins.iter().any(|plugin| {
            stats_render::report_plugin_format(plugin).as_deref() == Some(format.as_str())
        })
    });
    if let Some(config) = config.as_ref() {
        if external_renderer_configured {
            register_configured_renderer(&mut renderer_registry, config, format.as_str())?;
        }
    }
    if matches!(format, ReportFormat::Html)
        && output != Path::new("-")
        && !external_renderer_configured
    {
        if period.is_some() {
            write_html_site(&report, &output)?;
        } else {
            write_report_site(&data_dir, &output, &report, limit)?;
        }
        println!("wrote HTML report site {}", output.display());
        return Ok(());
    }
    if matches!(format, ReportFormat::Html) && !external_renderer_configured {
        bail!(
            "HTML stream rendering is an external plugin; provide --config with LoadPlugin=report_html <library>"
        );
    }
    if !matches!(format, ReportFormat::Html) {
        let config = config.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "report format {} is an external plugin; provide --config with a matching LoadPlugin entry",
                format.as_str()
            )
        })?;
        if !external_renderer_configured {
            bail!(
                "no external report renderer for {} is configured (expected LoadPlugin=report_{} <library>)",
                format.as_str(),
                format.as_str()
            );
        }
        let _ = config;
    }
    let content = render_report_stream(&renderer_registry, &report, format.as_str())?;
    if output == Path::new("-") {
        print!("{}", content);
    } else {
        fs::write(&output, content)
            .with_context(|| format!("write report {}", output.display()))?;
        println!("wrote {}", output.display());
    }
    Ok(())
}

fn write_report_site(
    data_dir: &Path,
    output: &Path,
    report: &stats_core::Report,
    limit: usize,
) -> anyhow::Result<()> {
    let periods = period_names(data_dir)?;
    let links = periods
        .iter()
        .map(|period| (period.clone(), format!("periods/{period}/index.html")))
        .collect::<Vec<_>>();
    write_html_site_with_periods(report, output, &links)?;
    for period in periods {
        let period_report = build_report_for_period(data_dir, limit, Some(&period))?;
        let period_output = output
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("periods")
            .join(&period)
            .join("index.html");
        write_html_site(&period_report, &period_output)?;
    }
    Ok(())
}

fn period_names(data_dir: &Path) -> anyhow::Result<Vec<String>> {
    let periods_dir = data_dir.join("periods");
    if !periods_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut periods = Vec::new();
    for entry in fs::read_dir(&periods_dir)
        .with_context(|| format!("read period directory {}", periods_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let period = entry.file_name().to_string_lossy().into_owned();
        if validate_period_name(&period).is_ok() && entry.path().join("stats.sqlite3").is_file() {
            periods.push(period);
        }
    }
    periods.sort();
    Ok(periods)
}

fn optimize(data_dir: PathBuf, vacuum: bool) -> anyhow::Result<()> {
    fs::create_dir_all(&data_dir)
        .with_context(|| format!("create data directory {}", data_dir.display()))?;
    let databases = database_paths(&data_dir)?;
    for db_path in &databases {
        let store = StatsStore::open(db_path)
            .with_context(|| format!("open statistics database {}", db_path.display()))?;
        store.optimize(vacuum)?;
        println!("optimized {}", db_path.display());
    }
    Ok(())
}

fn storage(data_dir: PathBuf) -> anyhow::Result<()> {
    let databases = database_paths(&data_dir)?;
    let mut total_bytes = 0u64;
    println!("Statistics storage: {}", data_dir.display());
    for db_path in &databases {
        let main_size = match fs::metadata(db_path) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect database {}", db_path.display()))
            }
        };
        let wal_path = PathBuf::from(format!("{}-wal", db_path.display()));
        let shm_path = PathBuf::from(format!("{}-shm", db_path.display()));
        let wal_size = fs::metadata(&wal_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let shm_size = fs::metadata(&shm_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let size = main_size + wal_size + shm_size;
        total_bytes += size;
        println!("{:>12} bytes  {}", size, db_path.display());
        if wal_size > 0 {
            println!("{:>12} bytes    {}", wal_size, wal_path.display());
        }
        if shm_size > 0 {
            println!("{:>12} bytes    {}", shm_size, shm_path.display());
        }
    }
    println!("{:>12} bytes  total", total_bytes);
    Ok(())
}

fn backup(data_dir: PathBuf, output_dir: PathBuf) -> anyhow::Result<()> {
    if output_dir.exists() {
        bail!(
            "backup output directory already exists: {}",
            output_dir.display()
        );
    }
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("create backup directory {}", output_dir.display()))?;
    let databases = database_paths(&data_dir)?;
    let mut copied = 0usize;
    for db_path in databases {
        if !db_path.is_file() {
            continue;
        }
        let relative = db_path
            .strip_prefix(&data_dir)
            .with_context(|| format!("map database {} into backup", db_path.display()))?;
        let destination = output_dir.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create backup directory {}", parent.display()))?;
        }
        let store = StatsStore::open(&db_path)
            .with_context(|| format!("open statistics database {}", db_path.display()))?;
        store.backup_to(&destination)?;
        println!(
            "backed up {} to {}",
            db_path.display(),
            destination.display()
        );
        copied += 1;
    }
    println!("backed up {} database(s)", copied);
    Ok(())
}

fn retention(
    data_dir: PathBuf,
    before: String,
    archive_dir: Option<PathBuf>,
    remove_source: bool,
) -> anyhow::Result<()> {
    validate_period_name(&before)?;
    if remove_source && archive_dir.is_none() {
        bail!("--remove-source requires --archive-dir");
    }
    let periods_dir = data_dir.join("periods");
    let mut candidates = Vec::new();
    if periods_dir.is_dir() {
        for entry in fs::read_dir(&periods_dir)
            .with_context(|| format!("read period directory {}", periods_dir.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.len() == before.len() && name < before && validate_period_name(&name).is_ok() {
                let database = entry.path().join("stats.sqlite3");
                if database.is_file() {
                    candidates.push((name, database, entry.path()));
                }
            }
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    if candidates.is_empty() {
        println!("no partitions older than {}", before);
        return Ok(());
    }

    println!("{} partition(s) older than {}", candidates.len(), before);
    for (period, database, _) in &candidates {
        println!("  {}", database.display());
        if let Some(archive_root) = &archive_dir {
            let destination = archive_root
                .join("periods")
                .join(period)
                .join("stats.sqlite3");
            if destination.exists() {
                bail!(
                    "archive destination already exists: {}",
                    destination.display()
                );
            }
        }
    }
    let Some(archive_root) = archive_dir else {
        if remove_source {
            unreachable!("validated above");
        }
        println!("dry run: no archive directory supplied; sources were unchanged");
        return Ok(());
    };

    if archive_root.starts_with(&data_dir) || data_dir.starts_with(&archive_root) {
        bail!("archive directory must be separate from data directory");
    }
    fs::create_dir_all(&archive_root)
        .with_context(|| format!("create archive directory {}", archive_root.display()))?;
    for (period, database, _) in &candidates {
        let destination = archive_root
            .join("periods")
            .join(period)
            .join("stats.sqlite3");
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create archive directory {}", parent.display()))?;
        }
        StatsStore::open(database)
            .with_context(|| format!("open partition database {}", database.display()))?
            .backup_to(&destination)
            .with_context(|| format!("archive partition {}", database.display()))?;
        println!(
            "archived {} to {}",
            database.display(),
            destination.display()
        );
    }
    if remove_source {
        for (_, _, source_dir) in candidates {
            fs::remove_dir_all(&source_dir)
                .with_context(|| format!("remove archived partition {}", source_dir.display()))?;
        }
        println!("removed archived source partitions");
    }
    Ok(())
}

fn migrate(
    config_path: PathBuf,
    log_dir: Option<PathBuf>,
    data_override: Option<PathBuf>,
    backup_dir: PathBuf,
) -> anyhow::Result<()> {
    let config = Config::from_path(&config_path)
        .with_context(|| format!("load configuration {}", config_path.display()))?
        .with_overrides(None, data_override.clone());
    if config.database_period == DatabasePeriod::None {
        bail!(
            "migration requires DatabasePeriod=monthly or DatabasePeriod=yearly in {}",
            config_path.display()
        );
    }
    if backup_dir.exists() {
        bail!(
            "migration backup directory already exists: {}",
            backup_dir.display()
        );
    }
    let data_dir = config.data_dir.clone();
    if data_dir.exists() {
        backup(data_dir.clone(), backup_dir)?;
    } else {
        fs::create_dir_all(&backup_dir).with_context(|| {
            format!("create migration backup directory {}", backup_dir.display())
        })?;
        println!(
            "no existing database found; created empty backup {}",
            backup_dir.display()
        );
    }
    let mut store = StoreRouter::open(&data_dir, config.database_period)?;
    store.clear_aggregates()?;
    store.reset_checkpoint()?;
    drop(store);
    println!(
        "replaying logs into {:?} partitions",
        config.database_period
    );
    backfill(config_path, log_dir, data_override)
}

fn database_paths(data_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let periods_dir = data_dir.join("periods");
    let mut databases = Vec::new();
    let control_db = data_dir.join("stats.sqlite3");
    if control_db.is_file() || !periods_dir.is_dir() {
        databases.push(control_db);
    }
    if periods_dir.is_dir() {
        for entry in fs::read_dir(&periods_dir)
            .with_context(|| format!("read period directory {}", periods_dir.display()))?
        {
            let entry = entry?;
            let db_path = entry.path().join("stats.sqlite3");
            if db_path.is_file() {
                databases.push(db_path);
            }
        }
    }
    databases.sort();
    Ok(databases)
}

fn build_report_for_data_dir(data_dir: &Path, limit: usize) -> anyhow::Result<stats_core::Report> {
    build_report_for_period(data_dir, limit, None)
}

fn build_report_for_period(
    data_dir: &Path,
    limit: usize,
    period: Option<&str>,
) -> anyhow::Result<stats_core::Report> {
    let databases = if let Some(period) = period {
        validate_period_name(period)?;
        let path = data_dir.join("periods").join(period).join("stats.sqlite3");
        if !path.is_file() {
            bail!("period database does not exist: {}", path.display());
        }
        vec![path]
    } else {
        let mut paths = database_paths(data_dir)?;
        let periods_dir = data_dir.join("periods");
        if periods_dir.is_dir() && paths.len() > 1 {
            paths.retain(|path| path != &data_dir.join("stats.sqlite3"));
        }
        paths
    };
    // Top-N rows from each partition are sufficient to compute the global top-N.
    let query_limit = limit.max(1);
    let mut reports = Vec::with_capacity(databases.len());
    for db_path in &databases {
        let store = StatsStore::open(db_path)
            .with_context(|| format!("open statistics database {}", db_path.display()))?;
        reports.push(ReportService::new(&store).build(query_limit)?);
    }
    let Some(mut merged) = reports.pop() else {
        bail!("no statistics database found in {}", data_dir.display());
    };
    for report in reports {
        merge_reports(&mut merged, report, limit.max(1));
    }
    if databases.len() > 1 {
        merged.metadata.input_path = Some(format!("{} database partitions", databases.len()));
        merged.metadata.approximations.push(
            "federated session metrics use shared control state; late or out-of-order input remains approximate".into(),
        );
    }
    Ok(merged)
}

fn validate_period_name(period: &str) -> anyhow::Result<()> {
    let valid_year = period.len() == 4 && period.bytes().all(|byte| byte.is_ascii_digit());
    let valid_month = period.len() == 7
        && period.as_bytes().get(4) == Some(&b'-')
        && period[..4].bytes().all(|byte| byte.is_ascii_digit())
        && period[5..].bytes().all(|byte| byte.is_ascii_digit())
        && (1..=12).contains(&period[5..].parse::<u8>().unwrap_or_default());
    if valid_year || valid_month {
        Ok(())
    } else {
        bail!("invalid period {period}; expected YYYY or YYYY-MM")
    }
}

fn merge_reports(target: &mut stats_core::Report, source: stats_core::Report, limit: usize) {
    target.hits += source.hits;
    target.bytes += source.bytes;
    target.pages += source.pages;
    target.files += source.files;
    target.visits += source.visits;
    target.unique_visits += source.unique_visits;
    merge_rows(&mut target.countries, source.countries, limit);
    merge_rows(&mut target.regions, source.regions, limit);
    merge_rows(&mut target.cities, source.cities, limit);
    merge_rows(&mut target.asns, source.asns, limit);
    merge_rows(&mut target.organizations, source.organizations, limit);
    merge_rows(&mut target.statuses, source.statuses, limit);
    merge_rows(&mut target.status_classes, source.status_classes, limit);
    merge_rows(&mut target.errors, source.errors, limit);
    merge_rows(&mut target.methods, source.methods, limit);
    merge_rows(&mut target.method_anomalies, source.method_anomalies, limit);
    merge_rows(&mut target.urls, source.urls, limit);
    merge_bandwidth(&mut target.bandwidth_urls, source.bandwidth_urls, limit);
    merge_daily(&mut target.daily, source.daily, limit);
    merge_rows(&mut target.users, source.users, limit);
    merge_rows(&mut target.user_agents, source.user_agents, limit);
    merge_rows(&mut target.referers, source.referers, limit);
    merge_rows(&mut target.referrer_domains, source.referrer_domains, limit);
    merge_rows(&mut target.file_types, source.file_types, limit);
    merge_rows(&mut target.browsers, source.browsers, limit);
    merge_rows(&mut target.oses, source.oses, limit);
    merge_rows(&mut target.robots, source.robots, limit);
    merge_rows(&mut target.request_times, source.request_times, limit);
    merge_rows(&mut target.virtual_hosts, source.virtual_hosts, limit);
    merge_rows(&mut target.entry_pages, source.entry_pages, limit);
    merge_rows(&mut target.exit_pages, source.exit_pages, limit);
    merge_rows(&mut target.search_engines, source.search_engines, limit);
    merge_rows(&mut target.keyphrases, source.keyphrases, limit);
    merge_rows(&mut target.hours, source.hours, limit);
    merge_rows(&mut target.dates, source.dates, limit);
    merge_rows(&mut target.months, source.months, limit);
    merge_rows(&mut target.weekdays, source.weekdays, limit);
    merge_rows(&mut target.diagnostics, source.diagnostics, limit);
}

fn merge_rows(
    target: &mut Vec<stats_core::ReportRow>,
    source: Vec<stats_core::ReportRow>,
    limit: usize,
) {
    let mut values = HashMap::<String, u64>::new();
    for row in target.drain(..).chain(source) {
        *values.entry(row.key).or_default() += row.hits;
    }
    target.extend(
        values
            .into_iter()
            .map(|(key, hits)| stats_core::ReportRow { key, hits }),
    );
    target.sort_by(|left, right| {
        right
            .hits
            .cmp(&left.hits)
            .then_with(|| left.key.cmp(&right.key))
    });
    target.truncate(limit);
}

fn merge_bandwidth(
    target: &mut Vec<stats_core::BandwidthRow>,
    source: Vec<stats_core::BandwidthRow>,
    limit: usize,
) {
    let mut values = HashMap::<String, (u64, u64)>::new();
    for row in target.drain(..).chain(source) {
        let entry = values.entry(row.key).or_default();
        entry.0 += row.hits;
        entry.1 += row.bytes;
    }
    target.extend(
        values
            .into_iter()
            .map(|(key, (hits, bytes))| stats_core::BandwidthRow { key, hits, bytes }),
    );
    target.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.key.cmp(&right.key))
    });
    target.truncate(limit);
}

fn merge_daily(
    target: &mut Vec<stats_core::DailyRow>,
    source: Vec<stats_core::DailyRow>,
    limit: usize,
) {
    let mut values = HashMap::<String, (u64, u64, u64, u64, u64)>::new();
    for row in target.drain(..).chain(source) {
        let entry = values.entry(row.date).or_default();
        entry.0 += row.hits;
        entry.1 += row.bytes;
        entry.2 += row.pages;
        entry.3 += row.files;
        entry.4 += row.visits;
    }
    target.extend(
        values
            .into_iter()
            .map(
                |(date, (hits, bytes, pages, files, visits))| stats_core::DailyRow {
                    date,
                    hits,
                    bytes,
                    pages,
                    files,
                    visits,
                },
            ),
    );
    target.sort_by(|left, right| right.date.cmp(&left.date));
    target.truncate(limit);
}

fn summary(data_dir: PathBuf, limit: usize) -> anyhow::Result<()> {
    let report = build_report_for_data_dir(&data_dir, limit.max(1))?;

    println!("Stats summary");
    println!("=============");
    println!("Hits:          {}", report.hits);
    println!("Bytes:         {}", report.bytes);
    println!("Pages:         {}", report.pages);
    println!("Files:         {}", report.files);
    println!("Visits:        {} (approximate)", report.visits);
    println!("Unique visits: {} (approximate)", report.unique_visits);
    println!(
        "Input:         {}",
        report.metadata.input_path.as_deref().unwrap_or("unknown")
    );
    println!();

    print_summary_section("Top URLs", &report.urls);
    print_bandwidth_section(&report.bandwidth_urls);
    print_daily_section(&report.daily);
    print_summary_section("Authenticated users", &report.users);
    print_summary_section("HTTP errors", &report.errors);
    print_summary_section("Status classes", &report.status_classes);
    print_summary_section("Countries", &report.countries);
    print_summary_section("Regions", &report.regions);
    print_summary_section("Network organizations", &report.organizations);
    print_summary_section("Virtual hosts", &report.virtual_hosts);
    print_summary_section("Request times", &report.request_times);
    print_summary_section("Referring sites", &report.referrer_domains);
    if !report.diagnostics.is_empty() {
        print_summary_section("Diagnostics", &report.diagnostics);
    }
    Ok(())
}

async fn summary_watch(
    data_dir: PathBuf,
    limit: usize,
    interval_seconds: u64,
) -> anyhow::Result<()> {
    let interval = Duration::from_secs(interval_seconds.max(1));
    loop {
        print!("\x1b[2J\x1b[H");
        std::io::stdout().flush()?;
        let refresh_dir = data_dir.clone();
        tokio::task::spawn_blocking(move || summary(refresh_dir, limit)).await??;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = time::sleep(interval) => {}
        }
    }
    Ok(())
}

async fn live(
    config_path: PathBuf,
    log_override: Option<PathBuf>,
    data_override: Option<PathBuf>,
    limit: usize,
    interval_seconds: u64,
) -> anyhow::Result<()> {
    let interval = Duration::from_secs(interval_seconds.max(1));
    loop {
        print!("\x1b[2J\x1b[H");
        std::io::stdout().flush()?;
        let update_config = config_path.clone();
        let update_log = log_override.clone();
        let update_data = data_override.clone();
        tokio::task::spawn_blocking(move || update(update_config, update_log, update_data))
            .await??;
        let data_dir = Config::from_path(&config_path)
            .with_context(|| format!("load configuration {}", config_path.display()))?
            .with_overrides(log_override.clone(), data_override.clone())
            .data_dir;
        let summary_dir = data_dir.clone();
        tokio::task::spawn_blocking(move || summary(summary_dir, limit)).await??;
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = time::sleep(interval) => {}
        }
    }
    Ok(())
}

fn tui(
    config_path: PathBuf,
    log_override: Option<PathBuf>,
    data_override: Option<PathBuf>,
    limit: usize,
    interval_seconds: u64,
) -> anyhow::Result<()> {
    update_inner(
        config_path.clone(),
        log_override.clone(),
        data_override.clone(),
        true,
    )?;
    let config = Config::from_path(&config_path)
        .with_context(|| format!("load configuration {}", config_path.display()))?
        .with_overrides(log_override, data_override);
    let refresh = Duration::from_secs(interval_seconds.max(1));
    let report = build_report(&config.data_dir, limit)?;
    run_tui(report, &config.log_file, refresh, || {
        update_inner(
            config_path.clone(),
            Some(config.log_file.clone()),
            Some(config.data_dir.clone()),
            true,
        )?;
        build_report(&config.data_dir, limit)
    })
}

fn build_report(data_dir: &Path, limit: usize) -> anyhow::Result<stats_core::Report> {
    build_report_for_data_dir(data_dir, limit.max(1))
}

fn print_summary_section(title: &str, rows: &[stats_core::ReportRow]) {
    println!("{title}");
    println!("{}", "-".repeat(title.len()));
    if rows.is_empty() {
        println!("(none)");
    } else {
        for row in rows {
            println!("{:>8}  {}", row.hits, row.key);
        }
    }
    println!();
}

fn print_bandwidth_section(rows: &[stats_core::BandwidthRow]) {
    println!("Bandwidth by URL");
    println!("----------------");
    if rows.is_empty() {
        println!("(none)");
    } else {
        for row in rows {
            println!("{:>12} bytes  {:>8} hits  {}", row.bytes, row.hits, row.key);
        }
    }
    println!();
}

fn print_daily_section(rows: &[stats_core::DailyRow]) {
    println!("Daily traffic");
    println!("-------------");
    if rows.is_empty() {
        println!("(none)");
    } else {
        for row in rows {
            println!(
                "{}  {:>8} hits  {:>8} bytes  {:>6} pages  {:>6} files  {:>6} visits",
                row.date, row.hits, row.bytes, row.pages, row.files, row.visits
            );
        }
    }
    println!();
}

fn validate(
    config_path: PathBuf,
    log_override: Option<PathBuf>,
    data_override: Option<PathBuf>,
) -> anyhow::Result<()> {
    let config = Config::from_path(&config_path)
        .with_context(|| format!("load configuration {}", config_path.display()))?
        .with_overrides(log_override, data_override);

    let mut failed = false;
    println!("configuration: {}", config_path.display());
    for warning in &config.warnings {
        println!("  [warning] {warning}");
    }
    println!("log format: {}", config.log_format);
    let parser = match LogParser::new(&config.log_format) {
        Ok(parser) => {
            println!("  [ok] parser compiles configured LogFormat");
            Some(parser)
        }
        Err(error) => {
            failed = true;
            println!("  [error] cannot compile configured LogFormat: {}", error);
            println!(
                "  [hint] check LogFormat syntax in {}",
                config_path.display()
            );
            None
        }
    };
    println!("log file: {}", config.log_file.display());
    match fs::File::open(&config.log_file) {
        Ok(_) => println!("  [ok] readable log file"),
        Err(error) => {
            failed = true;
            println!(
                "  [error] cannot read log file {}: {}",
                config.log_file.display(),
                error
            );
        }
    }
    if let Some(parser) = parser.as_ref() {
        match fs::read(&config.log_file) {
            Ok(raw) => {
                let sample = raw
                    .split(|byte| *byte == b'\n')
                    .map(String::from_utf8_lossy)
                    .map(|line| line.trim().to_owned())
                    .find(|line| !line.is_empty());
                match sample {
                    Some(line) => match parser.parse(&line) {
                        ParseOutcome::Event(_) => {
                            println!("  [ok] first non-empty log record parses")
                        }
                        ParseOutcome::Diagnostic(error) => {
                            failed = true;
                            println!("  [error] first non-empty log record rejected: {error}");
                            println!(
                                "  [hint] compare the record with LogFormat in {}",
                                config_path.display()
                            );
                        }
                    },
                    None => {
                        println!("  [warning] log file contains no non-empty records to sample")
                    }
                }
            }
            Err(error) => println!(
                "  [warning] cannot sample log file {}: {}",
                config.log_file.display(),
                error
            ),
        }
    }

    println!("data directory: {}", config.data_dir.display());
    match fs::metadata(&config.data_dir) {
        Ok(metadata) if metadata.is_dir() => println!("  [ok] directory exists"),
        Ok(_) => println!("  [error] path exists but is not a directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = config
                .data_dir
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            if parent.is_dir() {
                println!(
                    "  [ok] directory is absent but can be created under {}",
                    parent.display()
                );
            } else {
                failed = true;
                println!("  [error] directory and parent are unavailable: {}", error);
                println!("  [hint] create the parent or override --data-dir");
            }
        }
        Err(error) => {
            failed = true;
            println!(
                "  [error] cannot inspect directory {}: {}",
                config.data_dir.display(),
                error
            );
            println!("  [hint] check ownership and permissions or override --data-dir");
        }
    }

    for plugin in &config.plugins {
        let Some(path) = plugin.args.last() else {
            failed = true;
            println!("plugin {}: [error] missing database path", plugin.name);
            continue;
        };
        let path = PathBuf::from(path);
        match fs::File::open(&path) {
            Ok(_) => println!("plugin {}: [ok] readable {}", plugin.name, path.display()),
            Err(error) => {
                failed = true;
                println!(
                    "plugin {}: [error] cannot read {}: {}",
                    plugin.name,
                    path.display(),
                    error
                );
            }
        }
    }
    if failed {
        bail!("validation failed; resolve the paths marked [error]");
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct HealthResource {
    path: String,
    readable: bool,
}

#[derive(Debug, Serialize)]
struct HealthDatabase {
    path: String,
    readable: bool,
    hits: Option<u64>,
    bytes: Option<u64>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct HealthReport {
    ok: bool,
    log_file: String,
    log_readable: bool,
    plugin_resources: Vec<HealthResource>,
    databases: Vec<HealthDatabase>,
    warnings: Vec<String>,
}

fn health(config_path: PathBuf, data_override: Option<PathBuf>, json: bool) -> anyhow::Result<()> {
    let config = Config::from_path(&config_path)
        .with_context(|| format!("load configuration {}", config_path.display()))?
        .with_overrides(None, data_override);
    let mut failed = false;
    let log_readable = config.log_file.is_file();
    if !log_readable {
        if !json {
            println!(
                "[error] log file is not readable: {}",
                config.log_file.display()
            );
        }
        failed = true;
    } else if !json {
        println!("[ok] log file: {}", config.log_file.display());
    }
    if !json {
        for warning in &config.warnings {
            println!("[warning] {}", warning);
        }
    }
    let mut plugin_resources = Vec::new();
    for plugin in &config.plugins {
        for path in plugin.args.iter().filter(|arg| arg.starts_with('/')) {
            let path = Path::new(path);
            let readable = path.is_file();
            plugin_resources.push(HealthResource {
                path: path.display().to_string(),
                readable,
            });
            if readable {
                if !json {
                    println!("[ok] plugin resource: {}", path.display());
                }
            } else {
                if !json {
                    println!(
                        "[error] plugin resource is not readable: {}",
                        path.display()
                    );
                }
                failed = true;
            }
        }
    }
    let databases = database_paths(&config.data_dir)?;
    let existing = databases
        .iter()
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    let mut database_reports = Vec::new();
    if existing.is_empty() {
        if !json {
            println!(
                "[error] no statistics database found under {}",
                config.data_dir.display()
            );
        }
        failed = true;
    }
    for path in existing {
        match StatsStore::open_read_only(path).and_then(|store| store.totals()) {
            Ok((hits, bytes)) => {
                database_reports.push(HealthDatabase {
                    path: path.display().to_string(),
                    readable: true,
                    hits: Some(hits),
                    bytes: Some(bytes),
                    error: None,
                });
                if !json {
                    println!(
                        "[ok] database: {} (hits={}, bytes={})",
                        path.display(),
                        hits,
                        bytes
                    );
                }
            }
            Err(error) => {
                database_reports.push(HealthDatabase {
                    path: path.display().to_string(),
                    readable: false,
                    hits: None,
                    bytes: None,
                    error: Some(error.to_string()),
                });
                if !json {
                    println!("[error] database {}: {}", path.display(), error);
                }
                failed = true;
            }
        }
    }
    if json {
        let report = HealthReport {
            ok: !failed,
            log_file: config.log_file.display().to_string(),
            log_readable,
            plugin_resources,
            databases: database_reports,
            warnings: config.warnings.clone(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    if failed {
        bail!("health check failed");
    }
    if !json {
        println!("health check passed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        backup, build_report_for_data_dir, build_report_for_period, configured_referer_spam,
        configured_scanner, database_paths, health, migrate, plugin_paths, resolve_log_file,
        retention, StoreRouter,
    };
    use super::{decode_log_bytes, discover_backfill_files, validate_raw_log_output};
    use chrono::DateTime;
    use stats_core::{AccessEvent, Config, DatabasePeriod, GeoInfo, StatsStore};
    use std::fs;
    use std::io::Write;
    use std::path::{Path, PathBuf};

    #[test]
    fn rejects_raw_log_output_that_is_the_source() {
        let directory = std::env::temp_dir().join(format!(
            "stats-raw-log-safety-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("access.log");
        fs::write(&source, b"log").unwrap();

        let error = validate_raw_log_output(&source, Some(&source)).unwrap_err();
        assert!(error.to_string().contains("must differ from source"));

        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rejects_raw_log_output_symlink() {
        use std::os::unix::fs::symlink;

        let directory = std::env::temp_dir().join(format!(
            "stats-raw-log-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("access.log");
        let output = directory.join("export.log");
        fs::write(&source, b"log").unwrap();
        symlink(&source, &output).unwrap();

        let error = validate_raw_log_output(&source, Some(&output)).unwrap_err();
        assert!(error.to_string().contains("refusing symlink"));

        fs::remove_dir_all(directory).unwrap();
    }

    fn compressed(extension: &str, input: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        match extension {
            "gz" => {
                let mut encoder =
                    flate2::write::GzEncoder::new(&mut output, flate2::Compression::default());
                encoder.write_all(input).unwrap();
                encoder.finish().unwrap();
            }
            "bz2" => {
                let mut encoder =
                    bzip2::write::BzEncoder::new(&mut output, bzip2::Compression::default());
                encoder.write_all(input).unwrap();
                encoder.finish().unwrap();
            }
            "xz" => {
                let mut encoder = xz2::write::XzEncoder::new(&mut output, 6);
                encoder.write_all(input).unwrap();
                encoder.finish().unwrap();
            }
            _ => unreachable!(),
        }
        output
    }

    #[test]
    fn decodes_supported_log_compressions() {
        let input = b"one log line\ntwo log lines\n";
        for extension in ["gz", "bz2", "xz"] {
            let path = std::path::PathBuf::from(format!("access.log.{extension}"));
            assert_eq!(
                decode_log_bytes(&path, &compressed(extension, input)).unwrap(),
                input
            );
        }
    }

    #[test]
    fn leaves_uncompressed_and_unknown_extensions_unchanged() {
        let input = b"plain log data";
        for path in ["access.log", "access.log.zip"] {
            assert_eq!(decode_log_bytes(path.as_ref(), input).unwrap(), input);
        }
    }

    #[test]
    fn discovers_rotations_oldest_to_newest() {
        let directory = std::env::temp_dir().join(format!(
            "stats-backfill-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker")
        ));
        fs::create_dir_all(&directory).unwrap();
        for name in [
            "access.log",
            "access.log.1",
            "access.log.2.gz",
            "access.log.3.bz2",
            "access.log.4.xz",
            "access.log.tmp",
            "other.log.1.gz",
        ] {
            fs::write(directory.join(name), b"test").unwrap();
        }

        let files =
            discover_backfill_files(&directory, std::path::Path::new("access.log")).unwrap();
        let names = files
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "access.log.4.xz",
                "access.log.3.bz2",
                "access.log.2.gz",
                "access.log.1",
                "access.log"
            ]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn partitions_events_and_federates_reports() {
        let directory = std::env::temp_dir().join(format!(
            "stats-partition-test-{}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker"),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&directory).unwrap();
        let mut router = StoreRouter::open(&directory, DatabasePeriod::Month).unwrap();
        for timestamp in ["2026-01-31T23:59:00+00:00", "2026-02-01T00:01:00+00:00"] {
            let event = AccessEvent {
                client_ip: "203.0.113.10".into(),
                client_hostname: None,
                referer_spam: None,
                referer_excluded: false,
                scanner: None,
                method_anomaly: None,
                identity: None,
                user: None,
                timestamp: DateTime::parse_from_rfc3339(timestamp).unwrap(),
                request: "GET / HTTP/1.1".into(),
                method: Some("GET".into()),
                url: Some("/".into()),
                status: 200,
                bytes: 10,
                referer: None,
                user_agent: Some("test-agent".into()),
                duration_seconds: None,
                virtual_host: None,
                geo: GeoInfo::default(),
            };
            router.record(&event, true).unwrap();
        }
        drop(router);

        assert!(directory.join("periods/2026-01/stats.sqlite3").is_file());
        assert!(directory.join("periods/2026-02/stats.sqlite3").is_file());
        let maintenance_databases = database_paths(&directory).unwrap();
        assert_eq!(maintenance_databases.len(), 3);
        assert!(maintenance_databases.contains(&directory.join("stats.sqlite3")));
        assert_eq!(
            StatsStore::open(directory.join("periods/2026-01/stats.sqlite3"))
                .unwrap()
                .totals()
                .unwrap()
                .0,
            1
        );
        assert_eq!(
            StatsStore::open(directory.join("periods/2026-02/stats.sqlite3"))
                .unwrap()
                .totals()
                .unwrap()
                .0,
            1
        );
        let backup_dir = directory.with_file_name(format!(
            "{}-backup",
            directory.file_name().unwrap().to_string_lossy()
        ));
        backup(directory.clone(), backup_dir.clone()).unwrap();
        assert!(backup_dir.join("stats.sqlite3").is_file());
        assert!(backup_dir.join("periods/2026-01/stats.sqlite3").is_file());
        assert!(backup_dir.join("periods/2026-02/stats.sqlite3").is_file());
        fs::remove_dir_all(&backup_dir).unwrap();
        let report = build_report_for_data_dir(&directory, 50).unwrap();
        assert_eq!(report.hits, 2);
        assert_eq!(report.bytes, 20);
        assert_eq!(report.visits, 1);
        assert_eq!(report.daily.len(), 2);
        assert!(report
            .metadata
            .approximations
            .iter()
            .any(|note| note.contains("shared control state")));
        let output = directory.with_file_name(format!(
            "{}-report",
            directory.file_name().unwrap().to_string_lossy()
        ));
        super::write_report_site(&directory, &output.join("index.html"), &report, 50).unwrap();
        let overview = fs::read_to_string(output.join("index.html")).unwrap();
        assert!(overview.contains("periods/2026-01/index.html"));
        assert!(overview.contains("periods/2026-02/index.html"));
        assert!(output.join("periods/2026-01/index.html").is_file());
        assert!(output.join("periods/2026-02/index.html").is_file());
        let limited = build_report_for_data_dir(&directory, 1).unwrap();
        assert_eq!(limited.urls.len(), 1);
        assert_eq!(limited.daily.len(), 1);
        let january = build_report_for_period(&directory, 50, Some("2026-01")).unwrap();
        assert_eq!(january.hits, 1);
        assert_eq!(january.daily.len(), 1);
        assert!(build_report_for_period(&directory, 50, Some("../2026-01")).is_err());
        assert_eq!(
            StatsStore::open(directory.join("stats.sqlite3"))
                .unwrap()
                .totals()
                .unwrap(),
            (0, 0)
        );
        fs::remove_dir_all(&output).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn migration_backs_up_old_store_before_replaying_into_partitions() {
        let root = std::env::temp_dir().join(format!(
            "stats-migrate-test-{}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker"),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log_dir = root.join("logs");
        let data_dir = root.join("data");
        let backup_dir = root.join("backup");
        fs::create_dir_all(&log_dir).unwrap();
        fs::create_dir_all(&data_dir).unwrap();

        let config_path = root.join("stats.conf");
        fs::write(
            &config_path,
            format!(
                "LogFile=\"access.log\"\nDirData=\"{}\"\nLogFormat=1\nDatabasePeriod=monthly\n",
                data_dir.display()
            ),
        )
        .unwrap();
        fs::write(
            log_dir.join("access.log"),
            "203.0.113.10 - - [27/Oct/2026:23:59:00 +0000] \"GET / HTTP/1.1\" 200 10 \"-\" \"test-agent\"\n",
        )
        .unwrap();

        let old_event = AccessEvent {
            client_ip: "198.51.100.5".into(),
            client_hostname: None,
            referer_spam: None,
            referer_excluded: false,
            scanner: None,
            method_anomaly: None,
            identity: None,
            user: None,
            timestamp: DateTime::parse_from_rfc3339("2026-01-01T00:00:00+00:00").unwrap(),
            request: "GET /old HTTP/1.1".into(),
            method: Some("GET".into()),
            url: Some("/old".into()),
            status: 200,
            bytes: 5,
            referer: None,
            user_agent: Some("old-agent".into()),
            duration_seconds: None,
            virtual_host: None,
            geo: GeoInfo::default(),
        };
        let old_store = StatsStore::open(data_dir.join("stats.sqlite3")).unwrap();
        old_store.record(&old_event, true).unwrap();
        drop(old_store);

        migrate(
            config_path.clone(),
            Some(log_dir.clone()),
            Some(data_dir.clone()),
            backup_dir.clone(),
        )
        .unwrap();

        assert!(backup_dir.join("stats.sqlite3").is_file());
        assert!(data_dir.join("periods/2026-10/stats.sqlite3").is_file());
        let report = build_report_for_data_dir(&data_dir, 50).unwrap();
        assert_eq!(report.hits, 1);
        assert_eq!(report.bytes, 10);
        assert_eq!(
            StatsStore::open(backup_dir.join("stats.sqlite3"))
                .unwrap()
                .totals()
                .unwrap()
                .0,
            1
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retention_archives_old_partitions_before_removing_sources() {
        let root = std::env::temp_dir().join(format!(
            "stats-retention-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let data_dir = root.join("data");
        let archive_dir = root.join("archive");
        fs::create_dir_all(&data_dir).unwrap();
        let mut router = StoreRouter::open(&data_dir, DatabasePeriod::Month).unwrap();
        for timestamp in ["2024-12-31T23:59:00+00:00", "2025-01-01T00:01:00+00:00"] {
            let event = AccessEvent {
                client_ip: "203.0.113.10".into(),
                client_hostname: None,
                referer_spam: None,
                referer_excluded: false,
                scanner: None,
                method_anomaly: None,
                identity: None,
                user: None,
                timestamp: DateTime::parse_from_rfc3339(timestamp).unwrap(),
                request: "GET / HTTP/1.1".into(),
                method: Some("GET".into()),
                url: Some("/".into()),
                status: 200,
                bytes: 10,
                referer: None,
                user_agent: Some("test-agent".into()),
                duration_seconds: None,
                virtual_host: None,
                geo: GeoInfo::default(),
            };
            router.record(&event, true).unwrap();
        }
        drop(router);

        retention(
            data_dir.clone(),
            "2025-01".into(),
            Some(archive_dir.clone()),
            true,
        )
        .unwrap();
        assert!(!data_dir.join("periods/2024-12").exists());
        assert!(data_dir.join("periods/2025-01/stats.sqlite3").is_file());
        assert!(archive_dir.join("periods/2024-12/stats.sqlite3").is_file());
        assert_eq!(
            StatsStore::open(archive_dir.join("periods/2024-12/stats.sqlite3"))
                .unwrap()
                .totals()
                .unwrap()
                .0,
            1
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn health_check_does_not_create_a_missing_database() {
        let root = std::env::temp_dir().join(format!(
            "stats-health-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log_path = root.join("access.log");
        let data_dir = root.join("data");
        let config_path = root.join("stats.conf");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(&log_path, "log\n").unwrap();
        fs::write(
            &config_path,
            format!(
                "LogFile=\"{}\"\nDirData=\"{}\"\nLogFormat=1\nSiteDomain=example.test\n",
                log_path.display(),
                data_dir.display()
            ),
        )
        .unwrap();

        assert!(health(config_path.clone(), None, false).is_err());
        assert!(!data_dir.join("stats.sqlite3").exists());

        StatsStore::open(data_dir.join("stats.sqlite3")).unwrap();
        health(config_path, None, false).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn maps_awstats_geoip_plugin_aliases_to_database_roles() {
        let plugins = [
            stats_core::PluginConfig {
                name: "geoip2_region".into(),
                args: vec!["region.mmdb".into()],
            },
            stats_core::PluginConfig {
                name: "geoip2_city".into(),
                args: vec!["city.mmdb".into()],
            },
            stats_core::PluginConfig {
                name: "geoip6".into(),
                args: vec!["country.mmdb".into()],
            },
            stats_core::PluginConfig {
                name: "geoip2_asn".into(),
                args: vec!["asn.mmdb".into()],
            },
        ];

        assert_eq!(
            plugin_paths(&plugins),
            (
                Some(PathBuf::from("country.mmdb")),
                Some(PathBuf::from("city.mmdb")),
                Some(PathBuf::from("asn.mmdb"))
            )
        );
    }

    #[test]
    fn resolves_relative_log_file_from_config_directory_when_needed() {
        let root = std::env::temp_dir().join(format!(
            "stats-log-path-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let config_dir = root.join("logs");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_dir.join("access.log"), "line\n").unwrap();
        let config_path = config_dir.join("awstats.site.conf");

        assert_eq!(
            resolve_log_file(&config_path, PathBuf::from("access.log")),
            config_dir.join("access.log")
        );
        assert_eq!(
            resolve_log_file(&config_path, PathBuf::from("missing.log")),
            PathBuf::from("missing.log")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn referer_spam_rules_require_the_matching_plugin() {
        let config =
            Config::parse("LogFile=access.log\nLogFormat=1\nRefererSpamRules=missing.rules\n")
                .unwrap();
        assert!(
            configured_referer_spam(Path::new("/tmp/stats.conf"), &config)
                .unwrap()
                .is_none()
        );

        let enabled = Config::parse(
            "LogFile=access.log\nLogFormat=1\nRefererSpamRules=missing.rules\nLoadPlugin=detectrefererspam\n",
        )
        .unwrap();
        assert!(configured_referer_spam(Path::new("/tmp/stats.conf"), &enabled).is_err());
    }

    #[test]
    fn scanner_rules_require_the_matching_plugin() {
        let root = std::env::temp_dir().join(format!(
            "stats-scanner-rules-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("scanner.rules"),
            "path_prefix\t/.env\tenv-probe\n",
        )
        .unwrap();
        let config_path = root.join("stats.conf");
        let config =
            Config::parse("LogFile=access.log\nLogFormat=1\nScannerRules=scanner.rules\n").unwrap();
        assert!(configured_scanner(&config_path, &config).unwrap().is_none());

        let enabled = Config::parse(
            "LogFile=access.log\nLogFormat=1\nScannerRules=scanner.rules\nLoadPlugin=detectscanner\n",
        )
        .unwrap();
        assert!(configured_scanner(&config_path, &enabled)
            .unwrap()
            .is_some());
        fs::remove_dir_all(root).unwrap();
    }
}
