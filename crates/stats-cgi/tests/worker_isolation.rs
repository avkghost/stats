use std::process::Command;

#[test]
fn worker_marker_argument_does_not_enter_worker_mode() {
    let output = Command::new(env!("CARGO_BIN_EXE_stats-cgi"))
        .arg("__stats_plugin_render_worker")
        .arg("/nonexistent/plugin.so")
        .env("QUERY_STRING", "output=json")
        .env(
            "STATS_DATA_DIR",
            std::env::temp_dir().join("stats-cgi-worker-isolation-missing"),
        )
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.starts_with("Status: ") || stdout.starts_with("Content-Type: "),
        "expected CGI response, got {stdout:?}"
    );
    assert!(!stdout.contains("protocol_version"));
}
