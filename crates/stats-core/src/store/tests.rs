use super::*;

#[cfg(test)]
mod basic_tests {
    use super::*;
    use crate::{AccessTimestamp, GeoInfo};

    #[test]
    fn cardinality_table_list_is_unique_and_schema_backed() {
        let names = CARDINALITY_TABLES
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(names.len(), CARDINALITY_TABLES.len());
        for name in CARDINALITY_TABLES {
            assert!(AGGREGATE_SCHEMA.iter().any(|(table, _)| table == name));
        }
    }

    #[test]
    fn decodes_valid_utf8_query_keys_and_preserves_invalid_values() {
        assert_eq!(decode_query("caf%C3%A9+search"), "café search");
        assert_eq!(decode_query("bad%C3"), "bad%C3");
        assert_eq!(decode_query("bad%ZZ"), "bad%ZZ");
    }

    #[test]
    fn derives_awstats_dimensions() {
        assert_eq!(file_type(Some("/assets/app.CSS?v=1")), ".css");
        assert_eq!(file_type(Some("/")), "(none)");
        assert_eq!(browser("Mozilla/5.0 Chrome/120.0 Safari/537.36"), "Chrome");
        assert_eq!(browser("Mozilla/5.0 Edg/120.0 Chrome/120.0"), "Edge");
        assert_eq!(operating_system("Mozilla/5.0 (X11; Linux x86_64)"), "Linux");
        assert_eq!(
            operating_system("Mozilla/5.0 (iPhone; CPU iPhone OS 17_0)"),
            "iOS"
        );
        assert_eq!(
            robot("Mozilla/5.0 (compatible; GPTBot/1.0)"),
            Some("GPTBot")
        );
        assert_eq!(request_time_bucket(0.25), "100-500ms");
        assert_eq!(format!("{}xx", 404 / 100), "4xx");
        assert_eq!(
            search_referrer(Some(
                "https://www.google.com/search?q=rust+analytics%20tool"
            )),
            Some(("Google", "rust analytics tool".to_owned()))
        );
        assert_eq!(
            search_referrer(Some("https://www.bing.com/search?q=rust")),
            Some(("Bing", "rust".to_owned()))
        );
        assert_eq!(
            stable_key(&["client", "2026-10-28", "agent"]),
            stable_key(&["client", "2026-10-28", "agent"])
        );
        assert_ne!(
            stable_key(&["client", "2026-10-28", "agent"]),
            stable_key(&["client", "2026-10-29", "agent"])
        );
        assert_eq!(
            referrer_domain(Some("https://User:pass@WWW.Example.com:443/path")),
            Some("www.example.com".to_owned())
        );
        assert_eq!(referrer_domain(Some("/relative/path")), None);
        assert_eq!(referrer_domain(Some("-")), None);
    }

    #[test]
    fn tracks_entry_and_exit_pages_per_session() {
        let path = std::env::temp_dir().join(format!(
            "stats-entry-exit-{}-{}.sqlite3",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let store = StatsStore::open(&path).expect("open store");
        let event = |url: &str, timestamp: &str| AccessEvent {
            client_ip: "203.0.113.10".into(),
            client_hostname: None,
            referer_spam: None,
            referer_excluded: false,
            scanner: None,
            method_anomaly: None,
            identity: None,
            user: Some("alice".into()),
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
            geo: GeoInfo {
                region: Some("CA".into()),
                organization: Some("Example ISP".into()),
                ..Default::default()
            },
        };
        store
            .record(&event("/start", "2026-01-01T00:00:00+00:00"), true)
            .expect("record first page");
        store
            .record(&event("/next", "2026-01-01T00:01:00+00:00"), true)
            .expect("record second page");
        store
            .record(&event("/new", "2026-01-01T00:32:00+00:00"), true)
            .expect("record new session page");

        assert_eq!(
            store.top("entry_pages", 10).unwrap(),
            vec![("/new".into(), 1), ("/start".into(), 1)]
        );
        assert_eq!(
            store.top("exit_pages", 10).unwrap(),
            vec![("/new".into(), 1), ("/next".into(), 1)]
        );
        assert_eq!(store.top("users", 10).unwrap(), vec![("alice".into(), 3)]);
        assert_eq!(
            store.top_bandwidth(10).unwrap(),
            vec![
                ("/new".into(), 1, 10),
                ("/next".into(), 1, 10),
                ("/start".into(), 1, 10),
            ]
        );
        assert_eq!(
            store.daily(10).unwrap(),
            vec![DailyAggregate {
                date: "2026-01-01".into(),
                hits: 3,
                bytes: 30,
                pages: 3,
                files: 0,
                visits: 2,
            }]
        );
        assert_eq!(
            store.top("status_classes", 10).unwrap(),
            vec![("2xx".into(), 3)]
        );
        assert_eq!(store.top("regions", 10).unwrap(), vec![("CA".into(), 3)]);
        assert_eq!(
            store.top("organizations", 10).unwrap(),
            vec![("Example ISP".into(), 3)]
        );
        let mut anomalous = event("/method", "2026-01-01T00:33:00+00:00");
        anomalous.method_anomaly = Some("PROPFIND".into());
        store
            .record(&anomalous, true)
            .expect("record method anomaly");
        assert_eq!(
            store.top_if_exists("method_anomalies", 10).unwrap(),
            vec![("PROPFIND".into(), 1)]
        );
        let mut missing = event("/.env", "2026-01-01T00:34:00+00:00");
        missing.status = 404;
        missing.referer = Some("https://example.test/scan".into());
        store.record(&missing, false).expect("record missing URL");
        assert_eq!(
            store.not_found(10).unwrap(),
            vec![NotFoundAggregate {
                url: "/.env".into(),
                hits: 1,
                referrers: vec![("https://example.test/scan".into(), 1)],
            }]
        );
        assert_eq!(store.metric("visits").unwrap().0, 2);
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn migrates_old_schema_and_invalidates_checkpoint() {
        let path = std::env::temp_dir().join(format!(
            "stats-schema-migration-{}-{}.sqlite3",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let store = StatsStore::open(&path).expect("open store");
        store
            .set_meta("schema_version", "3")
            .expect("set old schema");
        store
            .set_meta("input_content_hash", "stale")
            .expect("set checkpoint");
        drop(store);

        let conn = Connection::open(&path).expect("open raw database");
        conn.execute("INSERT INTO totals(key,hits,bytes) VALUES('all',99,99)", [])
            .expect("seed old aggregate");
        drop(conn);

        let store = StatsStore::open(&path).expect("migrate store");
        assert_eq!(store.meta("schema_version").unwrap().as_deref(), Some("14"));
        assert_eq!(store.meta("input_content_hash").unwrap(), None);
        assert_eq!(store.totals().unwrap(), (0, 0));
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preserves_current_schema_database_and_checkpoints() {
        let path = std::env::temp_dir().join(format!(
            "stats-schema-current-{}-{}.sqlite3",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let store = StatsStore::open(&path).expect("open store");
        store
            .conn
            .lock()
            .expect("lock store")
            .execute("INSERT INTO totals(key,hits,bytes) VALUES('all',7,8)", [])
            .expect("seed aggregate");
        store
            .set_input_checkpoint(&InputCheckpoint {
                identity: "dev:ino".into(),
                path: "access.log".into(),
                config_hash: "config".into(),
                content_hash: "content".into(),
                size: 10,
                lines: 1,
            })
            .expect("set checkpoint");
        drop(store);

        let store = StatsStore::open(&path).expect("reopen current schema");
        assert_eq!(store.meta("schema_version").unwrap().as_deref(), Some("14"));
        assert_eq!(store.totals().unwrap(), (7, 8));
        assert_eq!(
            store
                .input_checkpoint_by_identity("dev:ino", "config")
                .unwrap()
                .unwrap()
                .lines,
            1
        );
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn finds_identical_content_checkpoint_by_path() {
        let path = std::env::temp_dir().join(format!(
            "stats-checkpoint-path-{}-{}.sqlite3",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let store = StatsStore::open(&path).expect("open store");
        for (identity, log_path) in [("one", "access.log.1"), ("two", "access.log.2.gz")] {
            store
                .set_input_checkpoint(&InputCheckpoint {
                    identity: identity.into(),
                    path: log_path.into(),
                    config_hash: "config".into(),
                    content_hash: "same-content".into(),
                    size: 10,
                    lines: 1,
                })
                .expect("set checkpoint");
        }
        assert_eq!(
            store
                .input_checkpoint_by_content_and_path("same-content", "config", "access.log.2.gz")
                .unwrap()
                .unwrap()
                .identity,
            "two"
        );
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn clears_aggregates_without_touching_schema() {
        let path = std::env::temp_dir().join(format!(
            "stats-clear-{}-{}.sqlite3",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let store = StatsStore::open(&path).expect("open store");
        store
            .set_meta("input_content_hash", "checkpoint")
            .expect("set checkpoint");
        store
            .conn
            .lock()
            .expect("lock store")
            .execute("INSERT INTO totals(key,hits,bytes) VALUES('all',7,8)", [])
            .expect("seed aggregate");
        store.clear_aggregates().expect("clear aggregates");
        assert_eq!(store.totals().unwrap(), (0, 0));
        assert_eq!(store.meta("schema_version").unwrap().as_deref(), Some("14"));
        assert_eq!(
            store.meta("input_content_hash").unwrap().as_deref(),
            Some("checkpoint")
        );
        drop(store);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn optimizes_and_vacuums_database() {
        let path = std::env::temp_dir().join(format!(
            "stats-optimize-{}-{}.sqlite3",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let store = StatsStore::open(&path).expect("open store");
        store.optimize(false).expect("optimize store");
        store.optimize(true).expect("vacuum store");
        let backup = path.with_extension("backup.sqlite3");
        store.backup_to(&backup).expect("backup store");
        assert!(backup.is_file());
        drop(store);
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(backup);
    }

    #[test]
    fn formats_visitors_with_hostname_and_ip_fallback() {
        assert_eq!(
            visitor_label("203.0.113.10", Some("crawler.example")),
            "203.0.113.10 (crawler.example)"
        );
        assert_eq!(visitor_label("203.0.113.10", None), "203.0.113.10");
        assert_eq!(visitor_label("203.0.113.10", Some("-")), "203.0.113.10");
    }
}

#[cfg(test)]
mod table_category_tests {
    use super::*;
    use rusqlite::Connection;

    fn temp_store(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "stats-core-store-categories-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("stats.sqlite3")
    }

    fn table_names(path: &std::path::Path) -> Vec<String> {
        let conn = Connection::open(path).unwrap();
        let mut statement = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap();
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        names
    }

    #[test]
    fn every_table_is_exactly_one_of_aggregate_or_service() {
        let path = temp_store("categories");
        drop(StatsStore::open(&path).unwrap());
        let names = table_names(&path);
        for name in &names {
            let aggregate = aggregate_tables().any(|table| table == name);
            let service = SERVICE_TABLES.contains(&name.as_str());
            assert!(
                aggregate ^ service,
                "table {name} must be in exactly one category"
            );
        }
        for listed in aggregate_tables().chain(SERVICE_TABLES.iter().copied()) {
            assert!(
                names.iter().any(|name| name == listed),
                "listed table {listed} must exist"
            );
        }
    }

    #[test]
    fn top_tables_are_aggregates_with_key_and_hits_columns() {
        let path = temp_store("top-tables");
        drop(StatsStore::open(&path).unwrap());
        let conn = Connection::open(&path).unwrap();
        for table in TOP_TABLES {
            assert!(
                aggregate_tables().any(|aggregate| aggregate == *table),
                "{table} must be an aggregate"
            );
            let columns: Vec<String> = conn
                .prepare(&format!("PRAGMA table_info({table})"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            assert!(
                columns.iter().any(|c| c == "key") && columns.iter().any(|c| c == "hits"),
                "{table}"
            );
        }
    }

    #[test]
    fn fresh_store_reads_default_without_error() {
        let path = temp_store("fresh");
        let store = StatsStore::open(&path).unwrap();
        assert_eq!(store.totals().unwrap(), (0, 0));
        assert_eq!(store.metric("pages").unwrap(), (0, 0));
        assert_eq!(store.unique_visits().unwrap(), 0);
        assert_eq!(store.meta("missing").unwrap(), None);
    }

    #[test]
    fn missing_table_is_an_error_not_a_default() {
        let path = temp_store("missing");
        let store = StatsStore::open(&path).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("DROP TABLE totals; DROP TABLE visit_keys; DROP TABLE meta;")
            .unwrap();
        assert!(store.totals().is_err());
        assert!(store.metric("pages").is_err());
        assert!(store.unique_visits().is_err());
        assert!(store.meta("schema_version").is_err());
    }

    #[test]
    fn clear_aggregates_clears_scanners() {
        let path = temp_store("scanners");
        let store = StatsStore::open(&path).unwrap();
        let conn = Connection::open(&path).unwrap();
        conn.execute("INSERT INTO scanners(key,hits) VALUES('probe',3)", [])
            .unwrap();
        store.clear_aggregates().unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM scanners", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 0);
    }

    #[test]
    fn schema_replay_clears_prefix_snapshots() {
        let path = temp_store("replay-snapshots");
        let store = StatsStore::open(&path).unwrap();
        store
            .snapshot_prefix(
                &PrefixState {
                    source_key: "src".into(),
                    config_hash: "cfg".into(),
                    identity: "1:10".into(),
                    path: "/logs/access.log".into(),
                    size: 100,
                    lines: 10,
                    sha256: "abc".into(),
                },
                1,
            )
            .unwrap();
        assert_eq!(store.snapshot_states("src", "cfg").unwrap().len(), 1);
        drop(store);
        let conn = Connection::open(&path).unwrap();
        conn.execute("UPDATE meta SET value='3' WHERE key='schema_version'", [])
            .unwrap();
        drop(conn);
        let reopened = StatsStore::open(&path).unwrap();
        assert!(reopened.snapshot_states("src", "cfg").unwrap().is_empty());
    }

    #[test]
    fn legacy_pre_prefix_schema_opens_into_the_same_table_set() {
        let fresh_path = temp_store("legacy-fresh");
        drop(StatsStore::open(&fresh_path).unwrap());
        let fresh = table_names(&fresh_path);

        let legacy_path = temp_store("legacy-old");
        {
            let conn = Connection::open(&legacy_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE totals(key TEXT PRIMARY KEY, hits INTEGER NOT NULL, bytes INTEGER NOT NULL);
                 CREATE TABLE input_checkpoints(identity TEXT PRIMARY KEY, path TEXT NOT NULL, config_hash TEXT NOT NULL, content_hash TEXT NOT NULL, size INTEGER NOT NULL, lines INTEGER NOT NULL);
                 INSERT INTO meta VALUES('schema_version','13'),('entry_exit_dimensions','1');
                 INSERT INTO totals VALUES('all',5,500),('pages',5,500);
                 INSERT INTO input_checkpoints VALUES('1:2','/logs/access.log','cfg','abc',100,10);",
            )
            .unwrap();
        }

        let store = StatsStore::open(&legacy_path).unwrap();
        assert_eq!(
            store.totals().unwrap(),
            (0, 0),
            "schema 13 aggregates are replayed"
        );
        assert_eq!(store.meta("schema_version").unwrap().as_deref(), Some("14"));
        assert_eq!(store.legacy_checkpoint_count("src", "cfg").unwrap(), 0);
        drop(store);

        let mut legacy = table_names(&legacy_path);
        let mut expected = fresh.clone();
        legacy.sort();
        expected.sort();
        assert_eq!(
            legacy, expected,
            "upgraded database must have the same tables as a fresh one"
        );
        let conn = Connection::open(&legacy_path).unwrap();
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(input_checkpoints)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            columns.iter().any(|c| c == "prefix_sha256"),
            "prefix columns added"
        );
    }
}

#[cfg(test)]
mod capped_call_site_tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Splits the argument list that starts at `text[0] == '('`, honouring nested brackets and
    /// string literals. Returns `None` if the call is not closed in `text`.
    fn call_arguments(text: &str) -> Option<Vec<&str>> {
        let mut args = Vec::new();
        let mut depth = 0usize;
        let mut start = 1usize;
        let mut in_string = false;
        for (index, character) in text.char_indices() {
            if in_string {
                if character == '"' {
                    in_string = false;
                }
                continue;
            }
            match character {
                '"' => in_string = true,
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    depth -= 1;
                    if depth == 0 {
                        args.push(&text[start..index]);
                        return Some(args);
                    }
                }
                ',' if depth == 1 => {
                    args.push(&text[start..index]);
                    start = index + 1;
                }
                _ => {}
            }
        }
        None
    }

    /// Returns the table literal of every `capped_key*` call in the production source. Calls that
    /// pass the table through a variable (the wrapper) carry no literal and are skipped.
    fn literal_capped_tables(production: &str) -> BTreeSet<String> {
        let mut tables = BTreeSet::new();
        let mut rest = production;
        while let Some(position) = rest.find("capped_key") {
            let before = rest[..position].trim_end();
            let mut after = &rest[position + "capped_key".len()..];
            if let Some(tail) = after.strip_prefix("_connection") {
                after = tail;
            } else if let Some(tail) = after.strip_prefix("_transaction") {
                after = tail;
            }
            if after.starts_with('(') && !before.ends_with("fn") {
                if let Some(args) = call_arguments(after) {
                    let literal = args
                        .iter()
                        .map(|arg| arg.trim())
                        .find(|arg| arg.len() >= 2 && arg.starts_with('"') && arg.ends_with('"'));
                    if let Some(literal) = literal {
                        tables.insert(literal[1..literal.len() - 1].to_owned());
                    }
                }
            }
            rest = &rest[position + "capped_key".len()..];
        }
        tables
    }

    /// Source-level guard: the capped existence and count queries must go through the statement
    /// cache. A formatted SQL string passed to `query_row` is parsed again on every call, which
    /// was the measured cost before the cache was introduced. This does not observe evictions,
    /// which rusqlite does not expose; the capacity is checked against the table count instead.
    #[test]
    fn capped_queries_use_the_prepared_statement_cache() {
        let source = include_str!("../store.rs");
        let production = &source[..source.find("\n#[cfg(test)]\nmod ").expect("test marker")];
        for table in ["exists_sql", "count_sql"] {
            assert!(
                production.contains(&format!("prepare_cached(&{table})")),
                "{table} must be prepared through the statement cache"
            );
            assert!(
                !production.contains(&format!("query_row(&{table}")),
                "{table} must not be run with an uncached query_row"
            );
        }
        assert!(PREPARED_STATEMENT_CACHE_CAPACITY >= 2 * CARDINALITY_TABLES.len());
    }

    #[test]
    fn every_capped_call_site_targets_a_listed_table_and_every_listed_table_has_a_call() {
        let source = include_str!("../store.rs");
        let aggregate_source = include_str!("aggregate.rs");
        let marker = "\n#[cfg(test)]\nmod ";
        let production = format!(
            "{}\n{}",
            &source[..source.find(marker).expect("first test module marker")],
            aggregate_source
        );
        let called = literal_capped_tables(&production);
        let listed: BTreeSet<String> = CARDINALITY_TABLES
            .iter()
            .map(|table| (*table).to_owned())
            .collect();

        assert!(!called.is_empty(), "no capped_key call sites were found");
        let unlisted: Vec<_> = called.difference(&listed).collect();
        assert!(
            unlisted.is_empty(),
            "capped_key targets missing from CARDINALITY_TABLES: {unlisted:?}"
        );
        let uncalled: Vec<_> = listed.difference(&called).collect();
        assert!(
            uncalled.is_empty(),
            "CARDINALITY_TABLES entries with no capped_key call: {uncalled:?}"
        );
    }
}
