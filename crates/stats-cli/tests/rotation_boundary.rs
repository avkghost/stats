//! Rotation boundary contract.
//!
//! Normative contract: after the active log is rotated to a plain, gzip, bzip2,
//! or xz archive and the documented backfill runs, aggregates equal a clean
//! import of the same records, with the prefix counted exactly once.
//!
//! Phase 1: T1-T3 pass; T4-* fail and pin the current defect for compressed
//! rotations (see docs/rotation-boundary-contract/03-decisions.md).

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
        let status = if i % 7 == 0 { 404 } else { 200 };
        let method = if i % 3 == 0 { "POST" } else { "GET" };
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

    // T4 (Phase 1: expected to FAIL until Phase 2 matching fix)
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
}

#[cfg(not(unix))]
#[test]
fn skipped_on_non_unix_platforms() {
    eprintln!("rotation_boundary: skipped, inode identity unavailable on this platform");
}
