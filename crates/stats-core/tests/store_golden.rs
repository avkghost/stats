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

#[test]
fn wide_fixture_projects_every_dimension_as_recorded() {
    let path = database();
    let store = StatsStore::open(&path).unwrap();
    let mut events = Vec::new();
    for index in 0..4 {
        events.push(wide_event(index));
    }
    let mut second = event("/docs/start", 300);
    second.status = 500;
    second.method = Some("POST".into());
    second.request = "POST /docs/start HTTP/1.1".into();
    second.duration_seconds = Some(2.5);
    events.push(second);
    for item in &events {
        store.record(item, true).unwrap();
    }
    let tables = [
        "countries",
        "regions",
        "cities",
        "asns",
        "organizations",
        "hostnames",
        "referer_spam",
        "scanners",
        "statuses",
        "status_classes",
        "errors",
        "methods",
        "method_anomalies",
        "urls",
        "users",
        "user_agents",
        "referers",
        "referrer_domains",
        "search_engines",
        "keyphrases",
        "file_types",
        "browsers",
        "oses",
        "robots",
        "request_times",
        "virtual_hosts",
        "entry_pages",
        "exit_pages",
        "hours",
        "dates",
        "months",
        "weekdays",
    ];
    let mut dump = String::new();
    for table in tables {
        dump.push_str(&format!("{table}: {:?}\n", store.top(table, 100).unwrap()));
    }
    dump.push_str(&format!(
        "totals: {:?} pages: {:?} visits: {:?}\n",
        store.totals().unwrap(),
        store.metric("pages").unwrap(),
        store.unique_visits().unwrap()
    ));
    let expected = r#"
countries: [("C0", 1), ("C1", 1), ("C2", 1), ("C3", 1)]
regions: [("R0, C0", 1), ("R1, C1", 1), ("R2, C2", 1), ("R3, C3", 1)]
cities: [("City0", 1), ("City1", 1), ("City2", 1), ("City3", 1)]
asns: [("0", 1), ("1", 1), ("2", 1), ("3", 1)]
organizations: [("Org0", 1), ("Org1", 1), ("Org2", 1), ("Org3", 1)]
hostnames: [("203.0.113.10", 1), ("203.0.113.10 (host-0.example.test)", 1), ("203.0.113.10 (host-1.example.test)", 1), ("203.0.113.10 (host-2.example.test)", 1), ("203.0.113.10 (host-3.example.test)", 1)]
referer_spam: [("spam-0.example", 1), ("spam-1.example", 1), ("spam-2.example", 1), ("spam-3.example", 1)]
scanners: [("scanner-0", 1), ("scanner-1", 1), ("scanner-2", 1), ("scanner-3", 1)]
statuses: [("404", 4), ("500", 1)]
status_classes: [("4xx", 4), ("5xx", 1)]
errors: [("404", 4), ("500", 1)]
methods: [("GET", 4), ("POST", 1)]
method_anomalies: []
urls: [("/docs/start", 1), ("/wide/0", 1), ("/wide/1", 1), ("/wide/2", 1), ("/wide/3", 1)]
users: [("user-0", 1), ("user-1", 1), ("user-2", 1), ("user-3", 1)]
user_agents: [("agent-0", 1), ("agent-1", 1), ("agent-2", 1), ("agent-3", 1), ("golden-agent", 1)]
referers: [("https://example.test/", 1), ("https://ref-0.example/", 1), ("https://ref-1.example/", 1), ("https://ref-2.example/", 1), ("https://ref-3.example/", 1)]
referrer_domains: [("example.test", 1), ("ref-0.example", 1), ("ref-1.example", 1), ("ref-2.example", 1), ("ref-3.example", 1)]
search_engines: []
keyphrases: []
file_types: [("(none)", 5)]
browsers: [("Other", 5)]
oses: [("Other", 5)]
robots: []
request_times: [("100-500ms", 4), ("1-5s", 1)]
virtual_hosts: [("vhost-0.example.test", 1), ("vhost-1.example.test", 1), ("vhost-2.example.test", 1), ("vhost-3.example.test", 1), ("www.example.test", 1)]
entry_pages: [("/docs/start", 1), ("/wide/0", 1), ("/wide/1", 1), ("/wide/2", 1), ("/wide/3", 1)]
exit_pages: [("/docs/start", 1), ("/wide/0", 1), ("/wide/1", 1), ("/wide/2", 1), ("/wide/3", 1)]
hours: [("03", 5)]
dates: [("2026-01-02", 5)]
months: [("2026-01", 5)]
weekdays: [("Friday", 5)]
totals: (5, 304) pages: (5, 304) visits: 5
"#;
    assert_eq!(dump, expected.trim_start_matches('\n'));
    drop(store);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
