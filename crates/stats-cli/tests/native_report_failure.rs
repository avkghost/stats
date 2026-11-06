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

fn setup() -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let root =
        std::env::temp_dir().join(format!("stats-cli-native-failure-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
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
    (root, config, data, mode)
}

#[test]
fn cli_worker_failure_is_controlled_and_next_render_succeeds() {
    let (root, config, data, mode) = setup();
    let output = root.join("report.json");
    let secret = "native-worker-test-secret";
    let failed = Command::new(env!("CARGO_BIN_EXE_stats-cli"))
        .args([
            "report",
            "--config",
            config.to_str().unwrap(),
            "--data-dir",
            data.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--format",
            "json",
        ])
        .env("STATS_TEST_REPORT_PLUGIN_MODE_FILE", &mode)
        .env("STATS_TEST_SECRET", secret)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    let stderr = String::from_utf8_lossy(&failed.stderr);
    assert!(stderr.contains("worker") || stderr.contains("report"));
    assert!(!stderr.contains(secret));
    assert!(!stderr.contains("panicked at"));

    fs::write(&mode, "valid\n").unwrap();
    let succeeded = Command::new(env!("CARGO_BIN_EXE_stats-cli"))
        .args([
            "report",
            "--config",
            config.to_str().unwrap(),
            "--data-dir",
            data.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--format",
            "json",
        ])
        .env("STATS_TEST_REPORT_PLUGIN_MODE_FILE", &mode)
        .output()
        .unwrap();
    assert!(succeeded.status.success(), "{succeeded:?}");
    assert!(fs::metadata(output).unwrap().len() > 0);
    let _ = fs::remove_dir_all(root);
}
