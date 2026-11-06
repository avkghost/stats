use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command},
    thread,
    time::Duration,
};

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

fn setup() -> (PathBuf, PathBuf, PathBuf, u16) {
    let root =
        std::env::temp_dir().join(format!("stats-http-native-failure-{}", std::process::id()));
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
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    (root, config, mode, port)
}

fn request(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn wait_for_server(child: &mut Child, port: u16) {
    for _ in 0..100 {
        if let Ok(response) = TcpStream::connect(("127.0.0.1", port)) {
            drop(response);
            return;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "HTTP server exited early"
        );
        thread::sleep(Duration::from_millis(20));
    }
    panic!("HTTP server did not start");
}

#[test]
fn http_worker_failure_returns_safely_and_next_request_succeeds() {
    let (root, config, mode, port) = setup();
    let secret = "http-native-worker-secret";
    let mut server = Command::new(env!("CARGO_BIN_EXE_stats-http"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "--data-dir",
            config.parent().unwrap().join("data").to_str().unwrap(),
            "--bind",
            &format!("127.0.0.1:{port}"),
        ])
        .env("STATS_TEST_REPORT_PLUGIN_MODE_FILE", &mode)
        .env("STATS_TEST_SECRET", secret)
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_for_server(&mut server, port);

    let failed = request(port, "/report?output=json");
    assert!(failed.starts_with("HTTP/1.1 500"), "{failed}");
    assert!(failed.contains("report request failed"));
    assert!(!failed.contains(secret));
    assert!(!failed.contains(config.to_str().unwrap()));
    assert!(!failed.contains("panicked at"));
    assert!(request(port, "/healthz").starts_with("HTTP/1.1 200"));

    fs::write(&mode, "valid\n").unwrap();
    let succeeded = request(port, "/report?output=json");
    assert!(succeeded.starts_with("HTTP/1.1 200"), "{succeeded}");
    assert!(succeeded.contains("application/json"));

    server.kill().unwrap();
    server.wait().unwrap();
    let _ = fs::remove_dir_all(root);
}
