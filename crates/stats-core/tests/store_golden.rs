use stats_core::{AccessEvent, AccessTimestamp, GeoInfo, StatsStore};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DATABASE: AtomicU64 = AtomicU64::new(0);

fn database() -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "stats-core-store-golden-{}-{}",
        std::process::id(),
        NEXT_DATABASE.fetch_add(1, Ordering::Relaxed)
    ));
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
    assert_eq!(store.top("statuses", 10).unwrap(), vec![("200".into(), 3)]);
    assert_eq!(
        store.top("status_classes", 10).unwrap(),
        vec![("2xx".into(), 3)]
    );
    assert_eq!(
        store.top("file_types", 10).unwrap(),
        vec![("(none)".into(), 2), (".css".into(), 1)]
    );
    assert_eq!(
        store.top("virtual_hosts", 10).unwrap(),
        vec![("www.example.test".into(), 3)]
    );
    assert_eq!(
        store.top("user_agents", 10).unwrap(),
        vec![("golden-agent".into(), 3)]
    );
    assert_eq!(
        store.top("referers", 10).unwrap(),
        vec![("https://example.test/".into(), 3)]
    );
    assert_eq!(
        store.top("referrer_domains", 10).unwrap(),
        vec![("example.test".into(), 3)]
    );
    assert_eq!(
        store.top("browsers", 10).unwrap(),
        vec![("Other".into(), 3)]
    );
    assert_eq!(store.top("oses", 10).unwrap(), vec![("Other".into(), 3)]);
    assert_eq!(
        store.top("request_times", 10).unwrap(),
        vec![("100-500ms".into(), 3)]
    );

    drop(store);
    let _ = fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn cardinality_limit_buckets_new_untrusted_keys() {
    let path = database();
    let store = StatsStore::open(&path).unwrap();
    let first = event("/one", 1);
    let mut second = event("/two", 1);
    second.user_agent = Some("second-agent".into());
    second.referer = Some("https://other.example/".into());
    store
        .record_with_cardinality_limit(&first, true, 1)
        .unwrap();
    store
        .record_with_cardinality_limit(&second, true, 1)
        .unwrap();

    assert_eq!(
        store.top("user_agents", 10).unwrap(),
        vec![("golden-agent".into(), 1), ("other".into(), 1)]
    );
    assert_eq!(
        store.top("referers", 10).unwrap(),
        vec![("https://example.test/".into(), 1), ("other".into(), 1)]
    );

    drop(store);
    let _ = fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn clearing_aggregates_resets_cardinality_counts_in_the_same_batch() {
    let path = database();
    let store = StatsStore::open(&path).unwrap();
    let first = event("/first", 1);
    let mut second = event("/second", 1);
    second.user_agent = Some("second-agent".into());
    store
        .with_batch(|batch| {
            batch.record_with_cardinality_limit(&first, true, 1)?;
            batch.clear_aggregates()?;
            batch.record_with_cardinality_limit(&second, true, 1)
        })
        .unwrap();
    assert_eq!(
        store.top("user_agents", 10).unwrap(),
        vec![("second-agent".into(), 1)]
    );
    drop(store);
    let _ = fs::remove_dir_all(path.parent().unwrap());
}

#[test]
#[ignore = "manual performance measurement; run with --ignored --nocapture"]
fn benchmark_cardinality_limit() {
    use std::time::Instant;

    const EVENTS: usize = 10_500;
    let run = |limit: usize, unique: bool| {
        let path = database();
        let store = StatsStore::open(&path).unwrap();
        let started = Instant::now();
        store
            .with_batch(|batch| {
                for index in 0..EVENTS {
                    let suffix = if unique {
                        index.to_string()
                    } else {
                        String::new()
                    };
                    let mut item = event(&format!("/bench/{suffix}"), 1);
                    item.user_agent = Some(format!("agent-{suffix}"));
                    item.referer = Some(format!("https://ref-{suffix}.example/"));
                    batch.record_with_cardinality_limit(&item, true, limit)?;
                }
                Ok::<_, stats_core::StoreError>(())
            })
            .unwrap();
        let elapsed = started.elapsed();
        drop(store);
        let _ = fs::remove_dir_all(path.parent().unwrap());
        elapsed
    };

    let capped = run(10_000, true);
    let uncapped = run(usize::MAX, true);
    let repeated_capped = run(10_000, false);
    let repeated_uncapped = run(usize::MAX, false);
    println!(
        "cardinality benchmark: {EVENTS} events; unique capped={capped:?}, unique uncapped={uncapped:?}, repeated capped={repeated_capped:?}, repeated uncapped={repeated_uncapped:?}"
    );
}

fn wide_event(index: usize) -> AccessEvent {
    let mut item = event(&format!("/wide/{index}"), 1);
    item.client_hostname = Some(format!("host-{index}.example.test"));
    item.referer_spam = Some(format!("spam-{index}.example"));
    item.scanner = Some(format!("scanner-{index}"));
    item.user = Some(format!("user-{index}"));
    item.user_agent = Some(format!("agent-{index}"));
    item.referer = Some(format!("https://ref-{index}.example/"));
    item.virtual_host = Some(format!("vhost-{index}.example.test"));
    item.geo = GeoInfo {
        country: Some(format!("C{}", index % 50)),
        region: Some(format!("R{index}")),
        city: Some(format!("City{index}")),
        latitude: None,
        longitude: None,
        asn: Some(index as u32),
        organization: Some(format!("Org{index}")),
    };
    item.status = 404;
    item
}

#[test]
#[ignore = "manual performance measurement; run with --ignored --nocapture"]
fn benchmark_cardinality_limit_wide_events() {
    use std::time::Instant;

    const EVENTS: usize = 10_500;
    let run = |limit: usize| {
        let path = database();
        let store = StatsStore::open(&path).unwrap();
        let started = Instant::now();
        store
            .with_batch(|batch| {
                for index in 0..EVENTS {
                    batch.record_with_cardinality_limit(&wide_event(index), true, limit)?;
                }
                Ok::<_, stats_core::StoreError>(())
            })
            .unwrap();
        let elapsed = started.elapsed();
        drop(store);
        let _ = fs::remove_dir_all(path.parent().unwrap());
        elapsed
    };
    let capped = run(10_000);
    let uncapped = run(usize::MAX);
    println!("wide cardinality benchmark: {EVENTS} unique events; capped={capped:?}, uncapped={uncapped:?}");
}
