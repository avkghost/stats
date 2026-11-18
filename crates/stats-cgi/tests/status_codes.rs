use std::{
    fs,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT_DATABASE: AtomicUsize = AtomicUsize::new(0);

fn run(query: &str, data: &std::path::Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_stats-cgi"))
        .env("QUERY_STRING", query)
        .env("STATS_DATA_DIR", data)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

fn database() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "stats-cgi-status-codes-{}-{}",
        std::process::id(),
        NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    stats_core::StatsStore::open(root.join("stats.sqlite3")).unwrap();
    root
}

#[test]
fn unsupported_format_and_unknown_section_are_client_errors() {
    let data = database();
    assert_eq!(run("output=xml", &data), "Status: 400 Bad Request");
    assert_eq!(run("section=unknown", &data), "Status: 404 Not Found");
    assert_eq!(
        run("section=overview", &data),
        "Content-Type: text/html; charset=utf-8"
    );
    let _ = fs::remove_dir_all(data);
}
