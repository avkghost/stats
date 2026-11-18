//! Transactional update for unpartitioned stores (plan phases 04 and 05).
//!
//! One run is one `BEGIN IMMEDIATE` transaction on the store. Planning (which
//! only reads files and checkpoints) and applying (parse, record, checkpoint)
//! happen inside it. Any error returns from the transaction closure, so the whole
//! run is rolled back and no file is ever half-applied.
//!
//! Each file is read as a stream. Digests are taken at known sizes, so memory
//! stays bounded by the decoder buffer, not by file size. The active file is read
//! only up to the size it had when the run started, and only complete lines are
//! counted (decisions I and J).

use crate::{
    configured_method_anomaly, configured_referer_spam, configured_scanner,
    enrich_with_native_plugins, hash_bytes, load_native_plugins, plugin_paths, reverse_dns,
    reverse_dns_enabled, update_summary, WorkerPlugin,
};
use anyhow::{bail, Context};
use bzip2::read::BzDecoder;
use flate2::read::GzDecoder;
use stats_core::{
    accepts, decide, is_page, AbortReason, AccessEvent, Batch, Config, Decision, FilterConfig,
    GeoIpEnricher, InputCheckpoint, LogParser, MaxMindGeoIp, ParseOutcome, PluginRegistry,
    PrefixDigest, PrefixState, StateKind, StateRef, StatsStore,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use xz2::read::XzDecoder;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    Plain,
    Gzip,
    Bzip2,
    Xz,
}

fn codec_for(path: &Path) -> Codec {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("gz") => Codec::Gzip,
        Some("bz2") => Codec::Bzip2,
        Some("xz") => Codec::Xz,
        _ => Codec::Plain,
    }
}

/// Result of one streaming pass over a file.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Scan {
    /// Bytes consumed from the (decoded) stream, within the bound.
    consumed: u64,
    consumed_sha: String,
    /// Position after the last complete line. For archives this is the full length.
    complete_size: u64,
    complete_lines: u64,
    complete_sha: String,
    /// Digests at the requested sizes that fall on a line boundary within the scan.
    marks: BTreeMap<u64, String>,
    invalid_utf8: bool,
}

/// Streams `path` once.
///
/// `active_bound` limits the read to the active file's size at the start of the
/// run. Archives pass `None` and are read to end of file, where an unterminated
/// final line counts as complete. `on_line` receives every complete line in order,
/// with its zero-based index and its bytes without the line terminator.
fn scan(
    path: &Path,
    active_bound: Option<u64>,
    marks: &BTreeSet<u64>,
    mut on_line: impl FnMut(u64, &[u8]) -> anyhow::Result<()>,
) -> anyhow::Result<Scan> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let raw: Box<dyn Read> = match active_bound {
        Some(bound) => Box::new(file.take(bound)),
        None => Box::new(file),
    };
    let decoded: Box<dyn Read> = match codec_for(path) {
        Codec::Plain => raw,
        Codec::Gzip => Box::new(GzDecoder::new(raw)),
        Codec::Bzip2 => Box::new(BzDecoder::new(raw)),
        Codec::Xz => Box::new(XzDecoder::new(raw)),
    };
    let mut reader = BufReader::new(decoded);

    let mut consumed_digest = PrefixDigest::new();
    let mut complete_digest = PrefixDigest::new();
    let mut complete_size = 0u64;
    let mut complete_lines = 0u64;
    let mut consumed = 0u64;
    let mut line_index = 0u64;
    let mut marks_out = BTreeMap::new();
    let mut pending = marks.iter().copied().filter(|mark| *mark > 0).peekable();
    let mut buffer = Vec::new();
    let mut invalid_utf8 = false;

    loop {
        buffer.clear();
        let read = reader
            .read_until(b'\n', &mut buffer)
            .with_context(|| format!("read {}", path.display()))?;
        if read == 0 {
            break;
        }
        let terminated = buffer.last() == Some(&b'\n');
        consumed_digest.update(&buffer);
        consumed += read as u64;

        while let Some(&mark) = pending.peek() {
            if mark > consumed {
                break;
            }
            if mark == consumed {
                marks_out.insert(mark, consumed_digest.clone().finish());
            }
            pending.next();
        }

        if terminated {
            complete_digest = consumed_digest.clone();
            complete_size = consumed;
            complete_lines = line_index + 1;
        }
        if terminated || active_bound.is_none() {
            let mut body: &[u8] = &buffer;
            if let Some(stripped) = body.strip_suffix(b"\n") {
                body = stripped.strip_suffix(b"\r").unwrap_or(stripped);
            }
            if std::str::from_utf8(body).is_err() {
                invalid_utf8 = true;
            }
            on_line(line_index, body)?;
        }
        line_index += 1;
    }

    if active_bound.is_none() {
        // Archives are complete files: an unterminated final line is a full line.
        complete_digest = consumed_digest.clone();
        complete_size = consumed;
        complete_lines = line_index;
    }

    Ok(Scan {
        consumed,
        consumed_sha: consumed_digest.finish(),
        complete_size,
        complete_lines,
        complete_sha: complete_digest.finish(),
        marks: marks_out,
        invalid_utf8,
    })
}

/// Everything decided about one file before any write.
struct FilePlan {
    path: PathBuf,
    path_string: String,
    identity: String,
    /// Size of the active file at run start; `None` for archives. Apply reads exactly this range.
    bound: Option<u64>,
    scan: Scan,
    /// Lines already counted; processing starts at this index.
    skip_lines: u64,
    /// Identity of a current checkpoint this file supersedes (its content is now counted
    /// through this file).
    replaces: Option<String>,
    /// State to keep as a snapshot before this identity's checkpoint is overwritten.
    snapshot_before: Option<PrefixState>,
    /// False when the file is a historical copy of a snapshot. Its content is already
    /// recorded there, so no new checkpoint is written for it.
    write_checkpoint: bool,
}

/// Runs one update over `files` as a single transaction.
///
/// `active` is the configured live log. Only that file is read up to its size at run
/// start; every other file is treated as a complete archive. `files` is processed in
/// the given order, which must be oldest to newest.
pub(crate) fn run(
    config_path: &Path,
    config: &Config,
    active: &Path,
    files: &[PathBuf],
    quiet: bool,
) -> anyhow::Result<()> {
    let update_started = Instant::now();
    let config_bytes = fs::read(config_path)
        .with_context(|| format!("read configuration {}", config_path.display()))?;
    // Stable fingerprint for new databases. Rows written by earlier versions carry the
    // frozen legacy hash of the same bytes; see the migration below.
    let config_hash = stats_core::prefix_sha256(&config_bytes);
    let legacy_config_hash = hash_bytes(&config_bytes);
    let source = stats_core::source_key(active)
        .context("derive source identity from the configured LogFile")?;
    if !quiet {
        for warning in &config.warnings {
            eprintln!("warning: {}: {warning}", config_path.display());
        }
    }
    let db_path = config.data_dir.join("stats.sqlite3");
    fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("create data directory {}", config.data_dir.display()))?;
    let store = StatsStore::open(&db_path)?;
    let timehires_enabled = config
        .plugins
        .iter()
        .any(|plugin| plugin.name == "timehires");

    let parser = LogParser::new(&config.log_format).context("compile LogFormat")?;
    let (country, city, asn) = plugin_paths(&config.plugins);
    let geoip = MaxMindGeoIp::open(country.as_deref(), city.as_deref(), asn.as_deref())
        .context("initialize configured GeoIP databases")?;
    let mut registry = PluginRegistry::new();
    registry.add(GeoIpEnricher {
        provider: Box::new(geoip),
    });
    let mut reverse_dns = if reverse_dns_enabled(config) {
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
    let referer_spam = configured_referer_spam(config_path, config)?;
    let scanner = configured_scanner(config_path, config)?;
    let method_anomaly = configured_method_anomaly(config);
    let mut native_plugins: Vec<WorkerPlugin> = load_native_plugins(&config.plugins, quiet);

    let mut counters = Counters::default();
    store.with_batch(|batch| -> anyhow::Result<()> {
        match batch.meta("config_hash")? {
            Some(previous) if previous == config_hash => {}
            // The same configuration under the earlier hash: keep every aggregate and
            // checkpoint, and only rename the hash.
            Some(previous) if previous == legacy_config_hash => {
                batch.migrate_config_hash(&legacy_config_hash, &config_hash)?;
            }
            // A genuinely different configuration: its checkpoints no longer apply.
            Some(_) => {
                batch.clear_aggregates()?;
                batch.reset_checkpoint()?;
            }
            None => {}
        }
        let plans = plan(batch, &source, &config_hash, active, files)?;
        for plan in &plans {
            apply_file(
                batch,
                plan,
                &source,
                &config_hash,
                config,
                &parser,
                &mut registry,
                scanner.as_ref(),
                referer_spam.as_ref(),
                method_anomaly.as_ref(),
                &mut native_plugins,
                &mut reverse_dns,
                &mut counters,
            )?;
        }
        batch.set_meta("config_hash", &config_hash)?;
        batch.set_meta("config_path", &config_path.display().to_string())?;
        batch.set_meta("source_key", &source)?;
        if stats_core::hashfiles_backend(&config.plugins).is_some() {
            batch.set_meta("hashfiles_backend", "sqlite")?;
        }
        Ok(())
    })?;

    if !quiet {
        println!(
            "{}",
            update_summary(
                counters.processed,
                counters.events,
                counters.diagnostics,
                counters.filtered,
                timehires_enabled,
                update_started.elapsed().as_millis(),
                &db_path,
            )
        );
    }
    Ok(())
}

#[derive(Default)]
struct Counters {
    processed: u64,
    events: u64,
    diagnostics: u64,
    filtered: u64,
}

fn describe_abort(reason: &AbortReason, file: &Path, source: &str) -> anyhow::Error {
    match reason {
        AbortReason::Ambiguous { candidates } => anyhow::anyhow!(
            "backfill aborted: {} matches {} stored states for source {source} ({}); no changes were written",
            file.display(),
            candidates.len(),
            candidates.join(", ")
        ),
        AbortReason::Unresolvable => anyhow::anyhow!(
            "backfill aborted: compressed archive {} cannot be checked against legacy checkpoints for source {source}; run the recovery procedure in docs/operations.md; no changes were written",
            file.display()
        ),
    }
}

/// Builds the plan for every file before any write. Only reads happen here.
fn plan(
    batch: &Batch<'_, '_>,
    source: &str,
    config_hash: &str,
    active: &Path,
    files: &[PathBuf],
) -> anyhow::Result<Vec<FilePlan>> {
    let mut states: Vec<StateRef> = batch
        .prefix_states(source, config_hash)?
        .into_iter()
        .map(|state| StateRef {
            identity: state.identity,
            kind: StateKind::Current,
            size: state.size,
            lines: state.lines,
            sha256: state.sha256,
        })
        .collect();
    states.extend(
        batch
            .snapshot_states(source, config_hash)?
            .into_iter()
            .map(|state| StateRef {
                identity: state.identity,
                kind: StateKind::Snapshot,
                size: state.size,
                lines: state.lines,
                sha256: state.sha256,
            }),
    );
    let legacy_rows_present = batch.legacy_checkpoint_count(source, config_hash)? > 0;
    let state_sizes: BTreeSet<u64> = states.iter().map(|state| state.size).collect();

    let mut plans: Vec<FilePlan> = Vec::with_capacity(files.len());
    for path in files {
        let metadata =
            fs::metadata(path).with_context(|| format!("inspect log file {}", path.display()))?;
        let is_active = path == active;
        let active_bound = is_active.then_some(metadata.len());
        let identity = crate::file_identity(&metadata);
        let compressed = codec_for(path) != Codec::Plain;
        let scanned = scan(path, active_bound, &state_sizes, |_, _| Ok(()))?;
        let own_identity_row = batch.input_checkpoint_by_identity(&identity, config_hash)?;
        let own_stable_state = states
            .iter()
            .find(|state| state.identity == identity && state.kind == StateKind::Current)
            .cloned();

        let decision = decide(&states, &scanned.marks, compressed, legacy_rows_present);
        let (skip_lines, replaces, write_checkpoint) = match decision {
            Decision::Abort(reason) => return Err(describe_abort(&reason, path, source)),
            Decision::New => {
                // A plain file whose identity still has a legacy row keeps the legacy
                // comparison, so an upgrade does not re-import what was already counted.
                let legacy = match own_identity_row {
                    Some(ref row) if own_stable_state.is_none() && !compressed => {
                        legacy_skip_lines(path, row, &scanned)?
                    }
                    _ => None,
                };
                (legacy.unwrap_or(0), None, true)
            }
            Decision::Skip { lines, matched } => {
                let replaces = match matched.kind {
                    StateKind::Current if matched.identity != identity => {
                        Some(matched.identity.clone())
                    }
                    _ => None,
                };
                let write = matched.kind == StateKind::Current;
                (lines, replaces, write)
            }
        };

        // The identity's live state is overwritten by this file. Keep it as a snapshot
        // unless this file extends it (normal growth), so a later archive copy of that
        // content is still recognised.
        let snapshot_before = own_stable_state
            .as_ref()
            .filter(|current| scanned.marks.get(&current.size) != Some(&current.sha256))
            .map(|current| PrefixState {
                source_key: source.to_owned(),
                config_hash: config_hash.to_owned(),
                identity: current.identity.clone(),
                path: path.display().to_string(),
                size: current.size,
                lines: current.lines,
                sha256: current.sha256.clone(),
            });

        plans.push(FilePlan {
            path: path.clone(),
            path_string: path.display().to_string(),
            identity,
            bound: active_bound,
            scan: scanned,
            skip_lines,
            replaces,
            snapshot_before,
            write_checkpoint,
        });
    }

    // Two files whose complete streams overlap at the start count the shared records
    // twice, even when one is shorter. Each file is rescanned for the other files' complete
    // sizes, so the check compares digests and needs no pairwise state.
    if plans.len() > 1 {
        for (index, left) in plans.iter().enumerate() {
            let others: BTreeSet<u64> = plans
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(_, plan)| plan.scan.complete_size)
                .filter(|size| *size > 0)
                .collect();
            let marks = scan(&left.path, left.bound, &others, |_, _| Ok(()))?.marks;
            for (other, right) in plans.iter().enumerate() {
                if other == index || right.scan.complete_size == 0 {
                    continue;
                }
                if marks.get(&right.scan.complete_size) != Some(&right.scan.complete_sha) {
                    continue;
                }
                if right.scan.complete_size == left.scan.complete_size {
                    bail!(
                        "backfill aborted: {} and {} contain the same records for source {source}; remove one of them; no changes were written",
                        left.path.display(),
                        right.path.display()
                    );
                }
                bail!(
                    "backfill aborted: {} begins with all of {} for source {source}, so the shared records would be counted twice; remove one of them; no changes were written",
                    left.path.display(),
                    right.path.display()
                );
            }
        }
    }
    Ok(plans)
}

/// Legacy comparison for a plain file whose identity has a pre-digest checkpoint row.
/// Returns the number of lines already counted, or `None` when nothing verifies.
fn legacy_skip_lines(
    path: &Path,
    row: &InputCheckpoint,
    scanned: &Scan,
) -> anyhow::Result<Option<u64>> {
    let raw = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if hash_bytes(&raw) == row.content_hash {
        return Ok(Some(scanned.complete_lines));
    }
    let prefix_matches =
        raw.len() as u64 >= row.size && hash_bytes(&raw[..row.size as usize]) == row.content_hash;
    Ok(prefix_matches.then_some(row.lines))
}

#[allow(clippy::too_many_arguments)]
fn apply_file(
    batch: &Batch<'_, '_>,
    plan: &FilePlan,
    source: &str,
    config_hash: &str,
    config: &Config,
    parser: &LogParser,
    registry: &mut PluginRegistry,
    scanner: Option<&stats_core::ScannerMatcher>,
    referer_spam: Option<&stats_core::RefererSpamMatcher>,
    method_anomaly: Option<&stats_core::MethodAnomalyMatcher>,
    native_plugins: &mut [WorkerPlugin],
    reverse_dns: &mut Option<reverse_dns::ReverseDnsEnricher>,
    counters: &mut Counters,
) -> anyhow::Result<()> {
    if let Some(previous) = &plan.snapshot_before {
        batch.snapshot_prefix(previous, unix_seconds())?;
    }

    let bound = plan.bound;
    let mut pending: Vec<AccessEvent> = Vec::with_capacity(256);
    let mut invalid_reported = false;
    let mut processed = 0u64;
    let mut events = 0u64;
    let mut diagnostics = 0u64;
    let mut filtered = 0u64;
    let skip = plan.skip_lines;
    let replay = scan(&plan.path, bound, &BTreeSet::new(), |index, line| {
        if index < skip {
            return Ok(());
        }
        processed += 1;
        let text = String::from_utf8_lossy(line);
        if !invalid_reported && std::str::from_utf8(line).is_err() {
            invalid_reported = true;
            batch.diagnostic(&format!(
                "{}: input contains invalid UTF-8; replacement characters were used",
                plan.path.display()
            ))?;
            diagnostics += 1;
        }
        let text: &str = &text;
        if text.trim().is_empty() || text.trim_start().starts_with('#') {
            return Ok(());
        }
        match parser.parse(text) {
            ParseOutcome::Event(mut event) => {
                if let Some(time_zone) = config.time_zone.as_deref() {
                    event.apply_timezone(time_zone);
                }
                if !accepts(&event, &config.filters) {
                    filtered += 1;
                    return Ok(());
                }
                registry.enrich(&mut event);
                if let Some(matcher) = scanner {
                    matcher.apply(&mut event);
                    if config.scanner_heuristics {
                        matcher.apply_heuristic(&mut event);
                    }
                }
                if let Some(matcher) = referer_spam {
                    matcher.apply(&mut event);
                }
                if let Some(matcher) = method_anomaly {
                    matcher.apply(&mut event);
                }
                if let Err(error) = enrich_with_native_plugins(&mut event, native_plugins) {
                    batch.diagnostic(&format!("native plugin enrichment disabled: {error}"))?;
                }
                if !config.url_referrer_with_query {
                    event.referer = event
                        .referer
                        .take()
                        .map(|referer| referer.split('?').next().unwrap_or_default().to_owned());
                }
                pending.push(*event);
                if pending.len() >= 256 {
                    events += flush(batch, &mut pending, reverse_dns.as_mut(), &config.filters)?;
                }
            }
            ParseOutcome::Diagnostic(error) => {
                batch.diagnostic(&format!("{}: {error}", plan.path.display()))?;
                diagnostics += 1;
            }
        }
        Ok(())
    })?;
    events += flush(batch, &mut pending, reverse_dns.as_mut(), &config.filters)?;

    // Verification before commit: the bytes we parsed must be the bytes we planned
    // against. A change inside the bound aborts the whole transaction.
    if replay.consumed != plan.scan.consumed
        || replay.consumed_sha != plan.scan.consumed_sha
        || replay.complete_size != plan.scan.complete_size
        || replay.complete_sha != plan.scan.complete_sha
    {
        bail!(
            "{} changed while the update was running; no changes were written",
            plan.path.display()
        );
    }

    counters.processed += processed;
    counters.events += events;
    counters.diagnostics += diagnostics;
    counters.filtered += filtered;

    if plan.write_checkpoint {
        if let Some(replaced) = &plan.replaces {
            batch.delete_checkpoint(replaced)?;
        }
        batch.set_input_checkpoint(&InputCheckpoint {
            identity: plan.identity.clone(),
            path: plan.path_string.clone(),
            config_hash: config_hash.to_owned(),
            // Legacy comparison field: never equal to a real hash, so stable rows are
            // matched by digest only.
            content_hash: String::new(),
            size: plan.scan.complete_size,
            lines: plan.scan.complete_lines,
        })?;
        batch.set_prefix_state(&PrefixState {
            source_key: source.to_owned(),
            config_hash: config_hash.to_owned(),
            identity: plan.identity.clone(),
            path: plan.path_string.clone(),
            size: plan.scan.complete_size,
            lines: plan.scan.complete_lines,
            sha256: plan.scan.complete_sha.clone(),
        })?;
    }
    Ok(())
}

fn flush(
    batch: &Batch<'_, '_>,
    events: &mut Vec<AccessEvent>,
    reverse_dns: Option<&mut reverse_dns::ReverseDnsEnricher>,
    filters: &FilterConfig,
) -> anyhow::Result<u64> {
    if let Some(enricher) = reverse_dns {
        let _ = enricher.enrich_batch(events);
    }
    let count = events.len() as u64;
    for event in events.drain(..) {
        batch.record(&event, is_page(&event, filters))?;
    }
    Ok(count)
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}
