use stats_core::{AccessEvent, AccessTimestamp, GeoInfo, StatsStore};
use std::{fs, path::PathBuf};

fn database() -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("stats-core-store-golden-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory.join("stats.sqlite3")
}

fn event(url: &str, bytes: u64) -> AccessEvent {
    AccessEvent {
        client_ip: "203.0.113.10".into(),
        client_hostname: None,
        referer_spam: None,
        referer_excluded: false,
        scanner: None,
        method_anomaly: None,
        identity: None,
        user: None,
        timestamp: AccessTimestamp::parse_rfc3339("2026-01-02T03:04:05+00:00").unwrap(),
        request: format!("GET {url} HTTP/1.1"),
        method: Some("GET".into()),
        url: Some(url.into()),
        status: 200,
        bytes,
        referer: Some("https://example.test/".into()),
        user_agent: Some("golden-agent".into()),
        duration_seconds: Some(0.25),
        virtual_host: Some("www.example.test".into()),
        geo: GeoInfo::default(),
    }
}

#[test]
fn fixed_fixture_has_stable_store_projection() {
    let path = database();
    let store = StatsStore::open(&path).unwrap();
    store.record(&event("/", 100), true).unwrap();
    store.record(&event("/docs", 50), true).unwrap();
    store.record(&event("/asset.css", 25), false).unwrap();

    assert_eq!(store.totals().unwrap(), (3, 175));
    assert_eq!(store.metric("pages").unwrap(), (2, 150));
    assert_eq!(store.metric("files").unwrap(), (1, 25));
    assert_eq!(
        store.top("urls", 10).unwrap(),
        vec![
            ("/".into(), 1),
            ("/asset.css".into(), 1),
            ("/docs".into(), 1)
        ]
    );
    assert_eq!(store.top("methods", 10).unwrap(), vec![("GET".into(), 3)]);
    assert_eq!(
        store.top("file_types", 10).unwrap(),
        vec![("(none)".into(), 2), (".css".into(), 1)]
    );
    assert_eq!(
        store.top("virtual_hosts", 10).unwrap(),
        vec![("www.example.test".into(), 3)]
    );

    drop(store);
    let _ = fs::remove_dir_all(path.parent().unwrap());
}
