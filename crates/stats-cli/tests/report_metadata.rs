//! Unpartitioned update records the live log's prefix in report metadata.

use std::{fs, process::Command};

use stats_core::{ReportService, StatsStore};

#[test]
fn unpartitioned_update_populates_report_input_metadata() {
    let root =
        std::env::temp_dir().join(format!("stats-cli-report-metadata-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let logs = root.join("logs");
    fs::create_dir_all(&logs).unwrap();
    let log = logs.join("access.log");
    let line =
        "203.0.113.1 - - [27/Oct/2026:10:00:00 +0000] \"GET / HTTP/1.1\" 200 10 \"-\" \"agent\"\n";
    let content = line.repeat(3);
    fs::write(&log, &content).unwrap();
    let data = root.join("data");
    fs::create_dir_all(&data).unwrap();
    let config = root.join("stats.conf");
    fs::write(
        &config,
        format!(
            "LogFile=\"{}\"\nLogFormat=1\nSiteDomain=\"example.test\"\nHostAliases=\"localhost\"\nDirData=\"{}\"\nDNSLookup=0\nDatabasePeriod=none\n",
            log.display(),
            data.display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_stats-cli"))
        .args(["update", "-c", config.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let store = StatsStore::open_read_only(data.join("stats.sqlite3")).unwrap();
    let report = ReportService::new(&store).build(10).unwrap();
    assert_eq!(
        report.metadata.input_path.as_deref(),
        Some(log.to_str().unwrap())
    );
    assert_eq!(report.metadata.input_size, Some(content.len() as u64));
    assert_eq!(report.metadata.input_lines, Some(3));
    let _ = fs::remove_dir_all(&root);
}
