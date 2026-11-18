use std::{fs, path::PathBuf, process::Command};

fn plugin_library() -> PathBuf {
    // Cargo's artifact layout differs by version: deps/ may be absent, and the fixture build
    // may sit only under target/<profile>/build. Search every candidate directory and take the
    // library built with the test fixture, identified by the mode variable it reads.
    let profile = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        });
    let mut directories = vec![profile.join("deps"), profile.clone()];
    if let Ok(builds) = fs::read_dir(profile.join("build").join("stats-report-json")) {
        directories.extend(
            builds
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("out")),
        );
    }
    directories
        .iter()
        .filter_map(|directory| fs::read_dir(directory).ok())
        .flat_map(|entries| entries.filter_map(Result::ok).map(|entry| entry.path()))
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("libstats_report_json"))
                && path.extension().is_some_and(|extension| {
                    matches!(extension.to_str(), Some("so" | "dylib" | "dll"))
                })
                && fs::read(path)
                    .map(|bytes| {
                        bytes
                            .windows(b"STATS_TEST_REPORT_PLUGIN_MODE_FILE".len())
                            .any(|window| window == b"STATS_TEST_REPORT_PLUGIN_MODE_FILE")
                    })
                    .unwrap_or(false)
        })
        .expect("test-fixture report plugin shared library")
}

/// The native worker is a binary of `stats-render`, which cargo does not build for another
/// package's tests. Build it here with the same profile as this test so the frontend can find it.
fn ensure_report_worker() {
    let mut command = std::process::Command::new(env!("CARGO"));
    command.args([
        "build",
        "--quiet",
        "-p",
        "stats-render",
        "--bin",
        "stats-report-worker",
    ]);
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    let status = command
        .status()
        .expect("run cargo to build stats-report-worker");
    assert!(status.success(), "building stats-report-worker failed");

    // Integration-test executables live in target/<profile>/deps, while `cargo build --bin`
    // places this worker in target/<profile>. The production contract is still “next to the
    // frontend”; mirror that layout beside the test executable so the test exercises the same
    // lookup without depending on stale artifacts from a previous build.
    let profile = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        });
    let source = profile.join(format!(
        "stats-report-worker{}",
        std::env::consts::EXE_SUFFIX
    ));
    let destination = std::env::current_exe()
        .expect("locate CGI integration test executable")
        .parent()
        .expect("integration test executable directory")
        .join(source.file_name().expect("worker file name"));
    fs::copy(&source, &destination).expect("place stats-report-worker beside test executable");
}

fn setup() -> (PathBuf, PathBuf, PathBuf) {
    ensure_report_worker();
    let root =
        std::env::temp_dir().join(format!("stats-cgi-native-failure-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    let log = root.join("access.log");
    fs::write(&log, "").unwrap();
    let mode = root.join("mode");
    fs::write(&mode, "abort\n").unwrap();
    let config = root.join("stats.conf");
    fs::write(
        &config,
        format!(
            "LogFile=\"{}\"\nLogFormat=1\nSiteDomain=\"example.test\"\nDirData=\"{}\"\nLoadPlugin=\"report_json {}\"\n",
            log.display(),
            data.display(),
            plugin_library().display()
        ),
    )
    .unwrap();
    stats_core::StatsStore::open(data.join("stats.sqlite3")).unwrap();
    (root, config, mode)
}

fn run(config: &PathBuf, data: &PathBuf, mode: &PathBuf) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_stats-cgi"))
        .env("QUERY_STRING", "output=json")
        .env("STATS_CONFIG", config)
        .env("STATS_DATA_DIR", data)
        .env("STATS_TEST_REPORT_PLUGIN_MODE_FILE", mode)
        .env("STATS_TEST_SECRET", "cgi-native-worker-secret")
        .output()
        .unwrap()
}

#[test]
fn cgi_worker_failure_has_controlled_response_and_next_run_succeeds() {
    let (root, config, mode) = setup();
    let data = root.join("data");
    let failed = run(&config, &data, &mode);
    assert!(!failed.status.success());
    let stdout = String::from_utf8_lossy(&failed.stdout);
    assert!(
        stdout.starts_with("Status: 500 Internal Server Error\r\n"),
        "status={:?} stdout={stdout:?} stderr={:?}",
        failed.status,
        failed.stderr
    );
    assert!(stdout.contains("Content-Type: text/plain; charset=utf-8"));
    assert!(stdout.contains("report request failed"));
    assert!(!stdout.contains(config.to_str().unwrap()));
    assert!(!stdout.contains("cgi-native-worker-secret"));
    assert!(!stdout.contains("panicked at"));

    fs::write(&mode, "valid\n").unwrap();
    let succeeded = run(&config, &data, &mode);
    assert!(succeeded.status.success());
    let stdout = String::from_utf8_lossy(&succeeded.stdout);
    assert!(stdout.starts_with("Content-Type: application/json; charset=utf-8\r\n"));
    assert!(stdout.len() > "Content-Type: application/json; charset=utf-8\r\n\r\n".len());
    let _ = fs::remove_dir_all(root);
}
