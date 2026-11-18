//! Rotation boundary contract.
//!
//! Normative contract: after the active log is rotated to a plain, gzip, bzip2,
//! or xz archive and the documented backfill runs, aggregates equal a clean
//! import of the same records, with the prefix counted exactly once.
//!
//! The fixture covers plain and compressed rotations, unread tails, and the
//! documented backfill recovery contract.

#[cfg(unix)]
mod unix {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        ops::RangeInclusive,
        path::{Path, PathBuf},
        process::{Command, Output},
    };

    use stats_core::StatsStore;

    /// Records counted before rotation (checkpointed by the first update).
    const P: usize = 100;
    /// Records appended after the checkpoint and before rename (unread tail).
    const T: usize = 50;
    /// Records written to the new active file after rename.
    const N: usize = 30;
    /// Records in a new active file written after a later rotation (T6).
    const M: usize = 10;

    const TABLES: &[&str] = &[
        "urls",
        "statuses",
        "status_classes",
        "methods",
        "user_agents",
        "referers",
        "hours",
        "dates",
    ];
    const METRICS: &[&str] = &["pages", "files", "visits"];

    #[derive(Debug, PartialEq)]
    struct Observation {
        totals: (u64, u64),
        metrics: Vec<(String, (u64, u64))>,
        unique_visits: u64,
        tables: Vec<(String, Vec<(String, u64)>)>,
        bandwidth: Vec<(String, u64, u64)>,
        daily: String,
        not_found: String,
    }

    #[derive(Clone, Copy)]
    enum Codec {
        Gz,
        Bz2,
        Xz,
    }

    impl Codec {
        fn extension(self) -> &'static str {
            match self {
                Codec::Gz => "gz",
                Codec::Bz2 => "bz2",
                Codec::Xz => "xz",
            }
        }

        fn compress(self, source: &Path, destination: &Path) {
            let data = fs::read(source).expect("read rotated log");
            let output = fs::File::create(destination).expect("create compressed archive");
            match self {
                Codec::Gz => {
                    let mut encoder =
                        flate2::write::GzEncoder::new(output, flate2::Compression::default());
                    encoder.write_all(&data).unwrap();
                    encoder.finish().unwrap();
                }
                Codec::Bz2 => {
                    let mut encoder =
                        bzip2::write::BzEncoder::new(output, bzip2::Compression::default());
                    encoder.write_all(&data).unwrap();
                    encoder.finish().unwrap();
                }
                Codec::Xz => {
                    let mut encoder = xz2::write::XzEncoder::new(output, 6);
                    encoder.write_all(&data).unwrap();
                    encoder.finish().unwrap();
                }
            }
        }
    }

    /// Deterministic combined-format record for index `i` (1-based).
    fn record(i: usize) -> String {
        let status = if i.is_multiple_of(7) { 404 } else { 200 };
        let method = if i.is_multiple_of(3) { "POST" } else { "GET" };
        format!(
            "203.0.113.{} - - [27/Oct/2026:10:{:02}:{:02} +0000] \"{} /page{}.html HTTP/1.1\" {} {} \"-\" \"agent-{}\"\n",
            i % 200 + 1,
            (i / 60) % 60,
            i % 60,
            method,
            i % 37,
            status,
            100 + i % 50,
            i % 4,
        )
    }

    fn lines(range: RangeInclusive<usize>) -> String {
        range.map(record).collect()
    }

    /// A temporary layout with its own config and data directory. Removed on drop.
    struct Layout {
        root: PathBuf,
    }

    impl Layout {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "stats-rotation-{}-{}",
                std::process::id(),
                name
            ));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(root.join("logs")).unwrap();
            fs::create_dir_all(root.join("data")).unwrap();
            fs::write(
                root.join("stats.conf"),
                format!(
                    "LogFile=\"{}\"\nLogFormat=1\nSiteDomain=\"example.test\"\nHostAliases=\"localhost\"\nDirData=\"{}\"\nDNSLookup=0\nDatabasePeriod=none\n",
                    root.join("logs/access.log").display(),
                    root.join("data").display(),
                ),
            )
            .unwrap();
            Layout { root }
        }

        fn logs(&self) -> PathBuf {
            self.root.join("logs")
        }

        fn config(&self) -> PathBuf {
            self.root.join("stats.conf")
        }

        fn data(&self) -> PathBuf {
            self.root.join("data")
        }
    }

    impl Drop for Layout {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn run_cli(args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stats-cli"))
            .args(args)
            .output()
            .expect("run stats-cli")
    }

    fn expect_success(output: Output, what: &str) {
        assert!(
            output.status.success(),
            "{what} failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    fn observe(data: &Path) -> Observation {
        let store = StatsStore::open_read_only(data.join("stats.sqlite3")).expect("open store");
        Observation {
            totals: store.totals().unwrap(),
            metrics: METRICS
                .iter()
                .map(|key| (key.to_string(), store.metric(key).unwrap()))
                .collect(),
            unique_visits: store.unique_visits().unwrap(),
            tables: TABLES
                .iter()
                .map(|table| (table.to_string(), store.top(table, 10_000).unwrap()))
                .collect(),
            bandwidth: store.top_bandwidth(10_000).unwrap(),
            daily: format!("{:?}", store.daily(10_000).unwrap()),
            not_found: format!("{:?}", store.not_found(10_000).unwrap()),
        }
    }

    fn hits(observation: &Observation) -> u64 {
        observation.totals.0
    }

    /// One complete rotation scenario and its clean-import reference.
    struct Run {
        scenario: Layout,
        // Held only so its temporary directory is removed on drop.
        _clean: Layout,
        after_normal: Observation,
        after_backfill: Observation,
        clean_import: Observation,
    }

    impl Run {
        fn execute(name: &str, codec: Option<Codec>) -> Self {
            let scenario = Layout::new(name);
            let clean = Layout::new(&format!("{name}-clean"));
            let config = scenario.config();
            let config_str = config.to_str().unwrap();
            let active = scenario.logs().join("access.log");
            let rotated = scenario.logs().join("access.log.1");

            // 1. Prefix P, checkpointed by a normal update.
            fs::write(&active, lines(1..=P)).unwrap();
            expect_success(run_cli(&["update", "-c", config_str]), "first update");

            // 2. Unread tail T appended after the checkpoint.
            let mut file = OpenOptions::new().append(true).open(&active).unwrap();
            file.write_all(lines(P + 1..=P + T).as_bytes()).unwrap();
            drop(file);

            // 3. Rename into rotation (same inode), then new active file with N records.
            fs::rename(&active, &rotated).unwrap();
            fs::write(&active, lines(P + T + 1..=P + T + N)).unwrap();

            // 4. Normal update: reads only the active path.
            expect_success(run_cli(&["update", "-c", config_str]), "normal update");
            let after_normal = observe(&scenario.data());

            // 5. Optional compression before backfill.
            if let Some(codec) = codec {
                let archive = scenario
                    .logs()
                    .join(format!("access.log.1.{}", codec.extension()));
                codec.compress(&rotated, &archive);
                fs::remove_file(&rotated).unwrap();
            }

            // 6. Documented backfill.
            let logs = scenario.logs();
            expect_success(
                run_cli(&[
                    "backfill",
                    "-c",
                    config_str,
                    "--log-dir",
                    logs.to_str().unwrap(),
                ]),
                "backfill",
            );
            let after_backfill = observe(&scenario.data());

            // Clean import of the same records: P+T as rotation, then N as active.
            let clean_config = clean.config();
            let clean_config_str = clean_config.to_str().unwrap();
            fs::write(clean.logs().join("access.log.1"), lines(1..=P + T)).unwrap();
            fs::write(
                clean.logs().join("access.log"),
                lines(P + T + 1..=P + T + N),
            )
            .unwrap();
            expect_success(
                run_cli(&[
                    "update",
                    "-c",
                    clean_config_str,
                    "--log-file",
                    clean.logs().join("access.log.1").to_str().unwrap(),
                ]),
                "clean import of rotation",
            );
            expect_success(
                run_cli(&[
                    "update",
                    "-c",
                    clean_config_str,
                    "--log-file",
                    clean.logs().join("access.log").to_str().unwrap(),
                ]),
                "clean import of active",
            );
            let clean_import = observe(&clean.data());

            Run {
                scenario,
                _clean: clean,
                after_normal,
                after_backfill,
                clean_import,
            }
        }

        fn second_backfill(&self) -> Observation {
            let logs = self.scenario.logs();
            expect_success(
                run_cli(&[
                    "backfill",
                    "-c",
                    self.scenario.config().to_str().unwrap(),
                    "--log-dir",
                    logs.to_str().unwrap(),
                ]),
                "second backfill",
            );
            observe(&self.scenario.data())
        }
    }

    fn assert_matches_clean_import(run: &Run, label: &str) {
        assert_eq!(
            hits(&run.after_backfill),
            (P + T + N) as u64,
            "{label}: total hits must equal P+T+N counted once"
        );
        assert_eq!(
            run.after_backfill, run.clean_import,
            "{label}: aggregates must equal a clean import of the same records"
        );
    }

    // T1
    #[test]
    fn t1_plain_rotation_recovers_tail_and_matches_clean_import() {
        let run = Run::execute("t1", None);
        assert_matches_clean_import(&run, "plain rotation");
    }

    // T2
    #[test]
    fn t2_normal_update_after_rotation_adds_only_new_active() {
        let run = Run::execute("t2", None);
        assert_eq!(
            hits(&run.after_normal),
            (P + N) as u64,
            "after normal update, tail T must be absent and prefix P counted once"
        );
    }

    // T3
    #[test]
    fn t3_repeated_backfill_changes_nothing() {
        let run = Run::execute("t3", None);
        let again = run.second_backfill();
        assert_eq!(
            again, run.after_backfill,
            "second backfill over unchanged files must not change any aggregate"
        );
    }

    // T4: compressed rotations recover the unread tail without replaying the prefix.
    #[test]
    fn t4_gz_rotation_before_backfill_counts_prefix_once() {
        let run = Run::execute("t4-gz", Some(Codec::Gz));
        assert_matches_clean_import(&run, "gzip rotation");
    }

    #[test]
    fn t4_bz2_rotation_before_backfill_counts_prefix_once() {
        let run = Run::execute("t4-bz2", Some(Codec::Bz2));
        assert_matches_clean_import(&run, "bzip2 rotation");
    }

    #[test]
    fn t4_xz_rotation_before_backfill_counts_prefix_once() {
        let run = Run::execute("t4-xz", Some(Codec::Xz));
        assert_matches_clean_import(&run, "xz rotation");
    }
    // T9: a concurrent writer is refused before any write.
    #[test]
    fn t9_concurrent_writer_is_refused_without_writes() {
        use std::os::unix::io::AsRawFd;

        let run = Run::execute("t9", None);
        let scenario = &run.scenario;
        let before = observe(&scenario.data());

        // Hold the writer lock from outside, as a running update or backfill would.
        let holder = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(scenario.data().join("stats.lock"))
            .expect("lock file exists after a command");
        let held = unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(held, 0, "test could not take the lock");

        let logs = scenario.logs();
        let output = run_cli(&[
            "backfill",
            "-c",
            scenario.config().to_str().unwrap(),
            "--log-dir",
            logs.to_str().unwrap(),
        ]);
        assert!(
            !output.status.success(),
            "backfill must refuse under a held lock"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("another stats process holds"),
            "stderr must explain the refusal: {stderr}"
        );
        assert_eq!(
            observe(&scenario.data()),
            before,
            "a refused command must not change any aggregate"
        );
        drop(holder);
    }

    // The lock is released when a command finishes.
    #[test]
    fn lock_is_released_after_a_successful_command() {
        use std::os::unix::io::AsRawFd;

        let run = Run::execute("t9-release", None);
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(run.scenario.data().join("stats.lock"))
            .unwrap();
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        assert_eq!(result, 0, "lock must be free once the command has exited");
    }
    fn append(path: &Path, text: &str) {
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn config_arg(layout: &Layout) -> String {
        layout.config().to_str().unwrap().to_owned()
    }

    fn cli_update(layout: &Layout) {
        expect_success(
            run_cli(&["update", "-c", &config_arg(layout)]),
            "normal update",
        );
    }

    fn cli_backfill(layout: &Layout) -> Output {
        let logs = layout.logs();
        run_cli(&[
            "backfill",
            "-c",
            &config_arg(layout),
            "--log-dir",
            logs.to_str().unwrap(),
        ])
    }

    fn stderr_of(output: &Output) -> String {
        String::from_utf8_lossy(&output.stderr).into_owned()
    }

    /// Rotation scenario before any backfill: P checkpointed, T appended, rename, new active N.
    fn rotated_scenario(name: &str) -> Layout {
        let layout = Layout::new(name);
        let active = layout.logs().join("access.log");
        fs::write(&active, lines(1..=P)).unwrap();
        cli_update(&layout);
        append(&active, &lines(P + 1..=P + T));
        fs::rename(&active, layout.logs().join("access.log.1")).unwrap();
        fs::write(&active, lines(P + T + 1..=P + T + N)).unwrap();
        cli_update(&layout);
        layout
    }

    /// The clean reference for `rotated_scenario`: P+T as the rotation, N as the active file.
    fn clean_reference(
        name: &str,
        rotation: RangeInclusive<usize>,
        active: RangeInclusive<usize>,
    ) -> Observation {
        let clean = Layout::new(name);
        fs::write(clean.logs().join("access.log.1"), lines(rotation)).unwrap();
        fs::write(clean.logs().join("access.log"), lines(active)).unwrap();
        expect_success(
            run_cli(&[
                "update",
                "-c",
                &config_arg(&clean),
                "--log-file",
                clean.logs().join("access.log.1").to_str().unwrap(),
            ]),
            "clean import of rotation",
        );
        expect_success(
            run_cli(&[
                "update",
                "-c",
                &config_arg(&clean),
                "--log-file",
                clean.logs().join("access.log").to_str().unwrap(),
            ]),
            "clean import of active",
        );
        observe(&clean.data())
    }

    fn identity_of(path: &Path) -> String {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(path).unwrap();
        format!("{}:{}", metadata.dev(), metadata.ino())
    }

    /// Same hash the updater stores in legacy rows (`DefaultHasher` over the bytes).
    fn legacy_hash(bytes: &[u8]) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    // T5: two identical archives are refused as a unit, and nothing is written.
    #[test]
    fn t5_identical_archives_abort_without_writes_then_recover() {
        let layout = rotated_scenario("t5");
        let rotated = layout.logs().join("access.log.1");
        Codec::Gz.compress(&rotated, &layout.logs().join("access.log.1.gz"));
        Codec::Gz.compress(&rotated, &layout.logs().join("access.log.2.gz"));
        fs::remove_file(&rotated).unwrap();
        let before = observe(&layout.data());

        let output = cli_backfill(&layout);
        assert!(!output.status.success(), "duplicate archives must abort");
        assert!(
            stderr_of(&output).contains("contain the same records"),
            "{}",
            stderr_of(&output)
        );
        assert_eq!(observe(&layout.data()), before, "abort must write nothing");

        fs::remove_file(layout.logs().join("access.log.2.gz")).unwrap();
        expect_success(
            cli_backfill(&layout),
            "backfill after removing the duplicate",
        );
        assert_eq!(
            observe(&layout.data()),
            clean_reference("t5-clean", 1..=P + T, P + T + 1..=P + T + N)
        );
    }

    // T7: copytruncate. The truncated prefix is snapshotted, and the copied archive counts once.
    #[test]
    fn t7_copytruncate_archive_matches_snapshot_and_counts_once() {
        let layout = Layout::new("t7");
        let active = layout.logs().join("access.log");
        fs::write(&active, lines(1..=P)).unwrap();
        cli_update(&layout);

        // Copy first (new identity), then truncate the active file in place (same identity).
        fs::copy(&active, layout.logs().join("access.log.1")).unwrap();
        fs::write(&active, lines(P + 1..=P + N)).unwrap();
        cli_update(&layout);
        assert_eq!(hits(&observe(&layout.data())), (P + N) as u64);

        expect_success(
            cli_backfill(&layout),
            "backfill of the copytruncate archive",
        );
        let after = observe(&layout.data());
        assert_eq!(
            hits(&after),
            (P + N) as u64,
            "copied prefix must not be counted again"
        );
        assert_eq!(after, clean_reference("t7-clean", 1..=P, P + 1..=P + N));
        expect_success(cli_backfill(&layout), "repeated backfill");
        assert_eq!(observe(&layout.data()), after);
    }

    // T8: partitioned stores refuse the public backfill.
    #[test]
    fn t8_backfill_refuses_partitioned_stores() {
        let layout = Layout::new("t8");
        let config = fs::read_to_string(layout.config())
            .unwrap()
            .replace("DatabasePeriod=none", "DatabasePeriod=monthly");
        fs::write(layout.config(), config).unwrap();
        fs::write(layout.logs().join("access.log"), lines(1..=10)).unwrap();

        let output = cli_backfill(&layout);
        assert!(!output.status.success());
        assert!(
            stderr_of(&output).contains("not supported when DatabasePeriod is set"),
            "{}",
            stderr_of(&output)
        );
        assert!(
            !layout.data().join("periods").exists(),
            "no partition may be created"
        );
    }

    // T10: a corrupt archive aborts before any write.
    #[test]
    fn t10_corrupt_archive_aborts_before_any_write() {
        let layout = rotated_scenario("t10");
        let rotated = layout.logs().join("access.log.1");
        let archive = layout.logs().join("access.log.1.gz");
        Codec::Gz.compress(&rotated, &archive);
        fs::remove_file(&rotated).unwrap();
        let bytes = fs::read(&archive).unwrap();
        fs::write(&archive, &bytes[..bytes.len() / 2]).unwrap();
        let before = observe(&layout.data());

        let output = cli_backfill(&layout);
        assert!(!output.status.success(), "corrupt archive must abort");
        assert!(
            stderr_of(&output).contains("access.log.1.gz"),
            "{}",
            stderr_of(&output)
        );
        assert_eq!(observe(&layout.data()), before);
    }

    // T11: a partially written trailing line is counted exactly once, when it completes.
    #[test]
    fn t11_partial_trailing_line_is_counted_once() {
        let layout = Layout::new("t11");
        let active = layout.logs().join("access.log");
        let record_101 = lines(101..=101);
        let fragment = 60;
        fs::write(
            &active,
            format!("{}{}", lines(1..=100), &record_101[..fragment]),
        )
        .unwrap();
        cli_update(&layout);
        assert_eq!(
            hits(&observe(&layout.data())),
            100,
            "fragment must not be counted yet"
        );

        append(
            &active,
            &format!("{}{}", &record_101[fragment..], lines(102..=110)),
        );
        cli_update(&layout);
        let after = observe(&layout.data());
        assert_eq!(hits(&after), 110, "completed fragment counted once");

        let clean = Layout::new("t11-clean");
        fs::write(clean.logs().join("access.log"), lines(1..=110)).unwrap();
        cli_update(&clean);
        assert_eq!(after, observe(&clean.data()));
    }

    // T6: legacy upgrade keeps counts, refuses an unresolvable compressed archive, and recovers.
    #[test]
    fn t6_legacy_upgrade_keeps_counts_and_recovers_from_unresolvable_archive() {
        let layout = Layout::new("t6");
        let active = layout.logs().join("access.log");
        let db = layout.data().join("stats.sqlite3");
        fs::write(&active, lines(1..=P)).unwrap();
        cli_update(&layout);
        assert_eq!(hits(&observe(&layout.data())), P as u64);

        // Rewrite the active identity's checkpoint as a pre-digest legacy row, and add an
        // unrelated legacy row that no present file can resolve.
        let config_hash = StatsStore::open_read_only(&db)
            .unwrap()
            .meta("config_hash")
            .unwrap()
            .expect("config hash stored");
        let active_bytes = fs::read(&active).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE input_checkpoints SET source_key='', prefix_size=NULL, prefix_lines=NULL, prefix_sha256=NULL, content_hash=?1, size=?2, lines=?3 WHERE identity=?4",
            rusqlite::params![
                legacy_hash(&active_bytes),
                active_bytes.len() as i64,
                P as i64,
                identity_of(&active)
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO input_checkpoints(identity,path,config_hash,content_hash,size,lines,source_key) VALUES('0:0','/gone/access.log',?1,'x',1,1,'')",
            rusqlite::params![config_hash],
        )
        .unwrap();
        drop(conn);

        // Legacy compatibility: appended records are counted once, with no re-import.
        append(&active, &lines(P + 1..=P + N));
        cli_update(&layout);
        assert_eq!(hits(&observe(&layout.data())), (P + N) as u64);

        // Rotate to a compressed archive, start a new active file, then add an archive
        // whose content nothing stored resembles.
        fs::rename(&active, layout.logs().join("access.log.1")).unwrap();
        Codec::Gz.compress(
            &layout.logs().join("access.log.1"),
            &layout.logs().join("access.log.1.gz"),
        );
        fs::remove_file(layout.logs().join("access.log.1")).unwrap();
        fs::write(&active, lines(P + N + 1..=P + N + M)).unwrap();
        cli_update(&layout);
        let stored_total = hits(&observe(&layout.data()));
        assert_eq!(stored_total, (P + N + M) as u64);

        let unrelated = layout.root.join("unrelated.log");
        fs::write(&unrelated, lines(9001..=9020)).unwrap();
        Codec::Gz.compress(&unrelated, &layout.logs().join("access.log.2.gz"));
        fs::remove_file(&unrelated).unwrap();
        let before = observe(&layout.data());
        let refused = cli_backfill(&layout);
        assert!(!refused.status.success(), "unresolvable archive must abort");
        assert!(
            stderr_of(&refused).contains("cannot be checked"),
            "{}",
            stderr_of(&refused)
        );
        assert_eq!(
            observe(&layout.data()),
            before,
            "refusal must write nothing"
        );

        // Documented recovery: rebuild from the oldest archive, then update in order.
        let cfg = config_arg(&layout);
        expect_success(
            run_cli(&[
                "rebuild",
                "-c",
                &cfg,
                "--log-file",
                layout.logs().join("access.log.2.gz").to_str().unwrap(),
            ]),
            "rebuild from the oldest archive",
        );
        expect_success(
            run_cli(&[
                "update",
                "-c",
                &cfg,
                "--log-file",
                layout.logs().join("access.log.1.gz").to_str().unwrap(),
            ]),
            "update from the rotation",
        );
        cli_update(&layout);
        expect_success(cli_backfill(&layout), "backfill after recovery");

        let recovered = observe(&layout.data());
        let reference = {
            let clean = Layout::new("t6-clean");
            fs::write(clean.logs().join("q.log"), lines(9001..=9020)).unwrap();
            fs::write(clean.logs().join("pn.log"), lines(1..=P + N)).unwrap();
            fs::write(
                clean.logs().join("access.log"),
                lines(P + N + 1..=P + N + M),
            )
            .unwrap();
            for name in ["q.log", "pn.log"] {
                expect_success(
                    run_cli(&[
                        "update",
                        "-c",
                        &config_arg(&clean),
                        "--log-file",
                        clean.logs().join(name).to_str().unwrap(),
                    ]),
                    "clean import",
                );
            }
            expect_success(
                run_cli(&[
                    "update",
                    "-c",
                    &config_arg(&clean),
                    "--log-file",
                    clean.logs().join("access.log").to_str().unwrap(),
                ]),
                "clean import of active",
            );
            observe(&clean.data())
        };
        assert_eq!(recovered, reference);
    }

    fn checkpoint_count(layout: &Layout) -> usize {
        let store = StatsStore::open_read_only(layout.data().join("stats.sqlite3")).unwrap();
        let config = store
            .meta("config_hash")
            .unwrap()
            .expect("config hash stored");
        let source = stats_core::source_key(&layout.logs().join("access.log")).unwrap();
        store.prefix_states(&source, &config).unwrap().len()
            + store.snapshot_states(&source, &config).unwrap().len()
    }

    fn write_archive(layout: &Layout, name: &str, range: RangeInclusive<usize>) {
        let plain = layout.root.join(format!("{name}.plain"));
        fs::write(&plain, lines(range)).unwrap();
        Codec::Gz.compress(&plain, &layout.logs().join(name));
        fs::remove_file(&plain).unwrap();
    }

    // T5c: one archive that begins with another is refused as a unit, and nothing is written.
    #[test]
    fn t5c_contained_overlap_aborts_without_writes() {
        let layout = Layout::new("t5c");
        fs::write(
            layout.logs().join("access.log"),
            lines(P + T + 1..=P + T + N),
        )
        .unwrap();
        cli_update(&layout);
        write_archive(&layout, "access.log.2.gz", 1..=P);
        write_archive(&layout, "access.log.1.gz", 1..=P + T);
        let before = observe(&layout.data());
        let checkpoints_before = checkpoint_count(&layout);

        let output = cli_backfill(&layout);
        assert!(!output.status.success(), "contained overlap must abort");
        assert!(
            stderr_of(&output).contains("counted twice"),
            "{}",
            stderr_of(&output)
        );
        assert_eq!(observe(&layout.data()), before, "abort must write nothing");
        assert_eq!(checkpoint_count(&layout), checkpoints_before);
    }

    // T12: a config hash written by an earlier version is migrated, and archive counts survive.
    #[test]
    fn t12_legacy_config_hash_is_migrated_without_replay() {
        let layout = Layout::new("t12");
        let active = layout.logs().join("access.log");
        fs::write(&active, lines(1..=P)).unwrap();
        cli_update(&layout);
        fs::rename(&active, layout.logs().join("access.log.1")).unwrap();
        fs::write(&active, lines(P + 1..=P + N)).unwrap();
        cli_update(&layout);
        expect_success(cli_backfill(&layout), "backfill of the rotation");
        assert_eq!(hits(&observe(&layout.data())), (P + N) as u64);

        // Store what an earlier version wrote: the legacy hash on meta and on the rows.
        let db = layout.data().join("stats.sqlite3");
        let config_bytes = fs::read(layout.config()).unwrap();
        let legacy = legacy_hash(&config_bytes);
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute(
            "UPDATE meta SET value=?1 WHERE key='config_hash'",
            rusqlite::params![legacy],
        )
        .unwrap();
        conn.execute(
            "UPDATE input_checkpoints SET config_hash=?1",
            rusqlite::params![legacy],
        )
        .unwrap();
        drop(conn);

        // The upgraded binary keeps the archive's counts and renames the hash.
        cli_update(&layout);
        let store = StatsStore::open_read_only(&db).unwrap();
        assert_eq!(
            store.meta("config_hash").unwrap().as_deref(),
            Some(stats_core::prefix_sha256(&config_bytes).as_str())
        );
        drop(store);
        assert_eq!(
            hits(&observe(&layout.data())),
            (P + N) as u64,
            "archive counts must survive"
        );
        assert_eq!(checkpoint_count(&layout), 2, "rows migrated, none dropped");

        append(&active, &lines(P + N + 1..=P + N + M));
        cli_update(&layout);
        assert_eq!(hits(&observe(&layout.data())), (P + N + M) as u64);
    }

    // T12b: a genuinely different configuration clears and replays, without double counting.
    #[test]
    fn t12b_changed_configuration_clears_and_replays_once() {
        let layout = Layout::new("t12b");
        fs::write(layout.logs().join("access.log"), lines(1..=P)).unwrap();
        cli_update(&layout);
        assert_eq!(hits(&observe(&layout.data())), P as u64);

        let config = fs::read_to_string(layout.config()).unwrap();
        fs::write(layout.config(), format!("{config}# changed\n")).unwrap();
        cli_update(&layout);
        assert_eq!(
            hits(&observe(&layout.data())),
            P as u64,
            "a changed configuration replays the active file once"
        );
    }
}

#[cfg(not(unix))]
#[test]
fn skipped_on_non_unix_platforms() {
    eprintln!("rotation_boundary: skipped, inode identity unavailable on this platform");
}
