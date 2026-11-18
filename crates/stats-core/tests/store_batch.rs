//! Store foundation (plan phase 01) and batch transactions (plan phase 02).

use rusqlite::Connection;
use stats_core::{
    AccessEvent, AccessTimestamp, GeoInfo, InputCheckpoint, PrefixState, StatsStore, StoreError,
};
use std::{fs, path::PathBuf};

fn temp_database(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "stats-core-store-batch-{}-{}",
        std::process::id(),
        name
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory.join("stats.sqlite3")
}

fn cleanup(path: &std::path::Path) {
    if let Some(directory) = path.parent() {
        let _ = fs::remove_dir_all(directory);
    }
}

fn event(url: &str, timestamp: &str) -> AccessEvent {
    AccessEvent {
        client_ip: "203.0.113.10".into(),
        client_hostname: None,
        referer_spam: None,
        referer_excluded: false,
        scanner: None,
        method_anomaly: None,
        identity: None,
        user: None,
        timestamp: AccessTimestamp::parse_rfc3339(timestamp).expect("timestamp"),
        request: format!("GET {url} HTTP/1.1"),
        method: Some("GET".into()),
        url: Some(url.into()),
        status: 200,
        bytes: 10,
        referer: None,
        user_agent: Some("test-agent".into()),
        duration_seconds: None,
        virtual_host: None,
        geo: GeoInfo::default(),
    }
}

fn state(identity: &str, size: u64) -> PrefixState {
    PrefixState {
        source_key: "src".into(),
        config_hash: "cfg".into(),
        identity: identity.into(),
        path: "/logs/access.log".into(),
        size,
        lines: size / 10,
        sha256: format!("sha-{identity}-{size}"),
    }
}

// Phase 01

#[test]
fn legacy_database_gains_columns_without_replay() {
    let path = temp_database("legacy");
    {
        // Schema as written before the prefix fields existed (schema version 13).
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE totals(key TEXT PRIMARY KEY, hits INTEGER NOT NULL, bytes INTEGER NOT NULL);
             CREATE TABLE input_checkpoints(identity TEXT PRIMARY KEY, path TEXT NOT NULL, config_hash TEXT NOT NULL, content_hash TEXT NOT NULL, size INTEGER NOT NULL, lines INTEGER NOT NULL);
             CREATE INDEX input_checkpoints_content ON input_checkpoints(content_hash, config_hash);
             INSERT INTO meta VALUES('schema_version','13'),('entry_exit_dimensions','1');
             INSERT INTO totals VALUES('all',5,500),('pages',5,500);
             INSERT INTO input_checkpoints VALUES('1:2','/logs/access.log','cfg','abc',100,10);",
        )
        .unwrap();
    }

    let store = StatsStore::open(&path).unwrap();
    assert_eq!(store.totals().unwrap(), (5, 500), "aggregates must be kept");
    assert_eq!(
        store.meta("schema_version").unwrap().as_deref(),
        Some("13"),
        "schema version must not change"
    );
    assert_eq!(store.legacy_checkpoint_count("src", "cfg").unwrap(), 1);
    assert!(store.prefix_states("src", "cfg").unwrap().is_empty());
    let legacy = store
        .input_checkpoint_by_identity("1:2", "cfg")
        .unwrap()
        .expect("legacy checkpoint kept");
    assert_eq!(legacy.content_hash, "abc");
    drop(store);
    cleanup(&path);
}

#[test]
fn stable_state_round_trips_and_is_scoped_by_source_and_config() {
    let path = temp_database("round-trip");
    let store = StatsStore::open(&path).unwrap();
    store.set_prefix_state(&state("1:10", 100)).unwrap();

    assert_eq!(
        store.prefix_states("src", "cfg").unwrap(),
        vec![state("1:10", 100)]
    );
    assert!(store
        .prefix_states("src", "other-config")
        .unwrap()
        .is_empty());
    assert!(store
        .prefix_states("other-source", "cfg")
        .unwrap()
        .is_empty());
    assert_eq!(store.legacy_checkpoint_count("src", "cfg").unwrap(), 0);
    drop(store);
    cleanup(&path);
}

#[test]
fn stable_fields_keep_existing_legacy_fields() {
    let path = temp_database("keep-legacy");
    let store = StatsStore::open(&path).unwrap();
    store
        .set_input_checkpoint(&InputCheckpoint {
            identity: "1:10".into(),
            path: "/logs/access.log".into(),
            config_hash: "cfg".into(),
            content_hash: "legacyhash".into(),
            size: 50,
            lines: 5,
        })
        .unwrap();
    assert_eq!(
        store.legacy_checkpoint_count("src", "cfg").unwrap(),
        1,
        "row without stable fields is legacy"
    );

    store.set_prefix_state(&state("1:10", 100)).unwrap();

    assert_eq!(store.legacy_checkpoint_count("src", "cfg").unwrap(), 0);
    let kept = store
        .input_checkpoint_by_identity("1:10", "cfg")
        .unwrap()
        .expect("row kept");
    assert_eq!(kept.content_hash, "legacyhash");
    assert_eq!(kept.size, 50);
    assert_eq!(kept.lines, 5);
    assert_eq!(
        store.prefix_states("src", "cfg").unwrap(),
        vec![state("1:10", 100)]
    );
    drop(store);
    cleanup(&path);
}

#[test]
fn snapshot_insert_is_idempotent_and_reset_clears_snapshots() {
    let path = temp_database("snapshots");
    let store = StatsStore::open(&path).unwrap();
    store.snapshot_prefix(&state("1:10", 100), 1).unwrap();
    store.snapshot_prefix(&state("1:10", 100), 2).unwrap();
    assert_eq!(store.snapshot_states("src", "cfg").unwrap().len(), 1);

    store.reset_checkpoint().unwrap();
    assert!(store.snapshot_states("src", "cfg").unwrap().is_empty());
    drop(store);
    cleanup(&path);
}

// Phase 02

#[test]
fn batch_commits_all_writes_on_ok() {
    let path = temp_database("batch-ok");
    let store = StatsStore::open(&path).unwrap();
    store
        .with_batch(|batch| -> Result<(), StoreError> {
            batch.record(&event("/a", "2026-01-01T00:00:00+00:00"), true)?;
            batch.set_prefix_state(&state("1:10", 100))?;
            batch.set_meta("config_hash", "cfg")?;
            Ok(())
        })
        .unwrap();

    assert_eq!(store.totals().unwrap(), (1, 10));
    assert_eq!(
        store.prefix_states("src", "cfg").unwrap(),
        vec![state("1:10", 100)]
    );
    assert_eq!(store.meta("config_hash").unwrap().as_deref(), Some("cfg"));
    drop(store);
    cleanup(&path);
}

#[test]
fn batch_rolls_back_every_write_on_err() {
    let path = temp_database("batch-err");
    let store = StatsStore::open(&path).unwrap();
    let result: Result<(), StoreError> = store.with_batch(|batch| {
        batch.record(&event("/a", "2026-01-01T00:00:00+00:00"), true)?;
        batch.set_prefix_state(&state("1:10", 100))?;
        batch.set_meta("config_hash", "cfg")?;
        batch.diagnostic("injected failure")?;
        // Any error value will do; the batch must not commit it.
        Err(StoreError::Poisoned)
    });

    assert!(result.is_err());
    assert_eq!(
        store.totals().unwrap(),
        (0, 0),
        "record must be rolled back"
    );
    assert!(store.prefix_states("src", "cfg").unwrap().is_empty());
    assert_eq!(store.meta("config_hash").unwrap(), None);
    assert!(store.diagnostics(10).unwrap().is_empty());
    drop(store);
    cleanup(&path);
}

// T5b: a state from one source never matches a candidate of another source.
#[test]
fn prefix_states_are_scoped_by_source_key() {
    let path = temp_database("source-scope");
    let store = StatsStore::open(&path).unwrap();
    // Identity is the primary key, so the other source needs its own identity.
    let mut other = state("1:11", 100);
    other.source_key = "other-src".into();
    store.set_prefix_state(&state("1:10", 100)).unwrap();
    store.set_prefix_state(&other).unwrap();

    assert_eq!(
        store.prefix_states("src", "cfg").unwrap(),
        vec![state("1:10", 100)]
    );
    assert_eq!(
        store.prefix_states("other-src", "cfg").unwrap(),
        vec![other]
    );
    drop(store);
    cleanup(&path);
}

#[test]
fn single_record_api_still_commits() {
    let path = temp_database("single-record");
    let store = StatsStore::open(&path).unwrap();
    store
        .record(&event("/a", "2026-01-01T00:00:00+00:00"), true)
        .unwrap();
    assert_eq!(store.totals().unwrap(), (1, 10));
    drop(store);
    cleanup(&path);
}
