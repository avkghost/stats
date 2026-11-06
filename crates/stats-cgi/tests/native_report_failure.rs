use std::{fs, path::PathBuf, process::Command};

fn plugin_library() -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        })
        .join("deps");
    fs::read_dir(directory)
        .expect("target dependency directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("libstats_report_json"))
                && path.extension().is_some_and(|extension| {
                    matches!(extension.to_str(), Some("so" | "dylib" | "dll"))
                })
        })
        .expect("test report plugin shared library")
}

fn setup() -> (PathBuf, PathBuf, PathBuf) {
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
