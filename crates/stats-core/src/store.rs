use crate::AccessEvent;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Transaction};
use std::{path::Path, sync::Mutex, time::Duration};
use thiserror::Error;

const SCHEMA_VERSION: &str = "13";
const ENTRY_EXIT_VERSION: &str = "1";

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("store lock poisoned")]
    Poisoned,
}

pub struct StatsStore {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputCheckpoint {
    pub identity: String,
    pub path: String,
    pub config_hash: String,
    pub content_hash: String,
    pub size: u64,
    pub lines: u64,
}

#[derive(Debug, Default, Clone)]
pub struct UpdateStats {
    pub lines: u64,
    pub events: u64,
    pub diagnostics: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyAggregate {
    pub date: String,
    pub hits: u64,
    pub bytes: u64,
    pub pages: u64,
    pub files: u64,
    pub visits: u64,
}

impl StatsStore {
    /// Opens an existing database without schema setup or write access.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA temp_store=MEMORY;")?;
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS totals(key TEXT PRIMARY KEY, hits INTEGER NOT NULL, bytes INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS countries(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS regions(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS cities(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS asns(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS organizations(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS statuses(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS status_classes(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS errors(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS methods(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS urls(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS url_bandwidth(key TEXT PRIMARY KEY, hits INTEGER NOT NULL, bytes INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS users(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS user_agents(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS referers(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS referrer_domains(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS search_engines(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS keyphrases(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS file_types(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS browsers(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS oses(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS robots(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS request_times(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS virtual_hosts(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS entry_pages(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS exit_pages(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS hours(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS visit_keys(key TEXT PRIMARY KEY); CREATE TABLE IF NOT EXISTS session_state(key TEXT PRIMARY KEY, last_seen INTEGER NOT NULL, last_url TEXT); CREATE TABLE IF NOT EXISTS diagnostics(reason TEXT PRIMARY KEY, count INTEGER NOT NULL);")?;
        let has_last_url = conn
            .prepare("PRAGMA table_info(session_state)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "last_url");
        if !has_last_url {
            conn.execute("ALTER TABLE session_state ADD COLUMN last_url TEXT", [])?;
        }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS dates(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS daily(key TEXT PRIMARY KEY, hits INTEGER NOT NULL, bytes INTEGER NOT NULL, pages INTEGER NOT NULL, files INTEGER NOT NULL, visits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS months(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS weekdays(key TEXT PRIMARY KEY, hits INTEGER NOT NULL);")?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS input_checkpoints(identity TEXT PRIMARY KEY, path TEXT NOT NULL, config_hash TEXT NOT NULL, content_hash TEXT NOT NULL, size INTEGER NOT NULL, lines INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS input_checkpoints_content ON input_checkpoints(content_hash, config_hash);")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS hostnames(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS referer_spam(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS scanners(key TEXT PRIMARY KEY, hits INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS method_anomalies(key TEXT PRIMARY KEY, hits INTEGER NOT NULL);",
        )?;
        let previous_schema = conn
            .query_row(
                "SELECT value FROM meta WHERE key='schema_version'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let entry_exit_marker = conn
            .query_row(
                "SELECT value FROM meta WHERE key='entry_exit_dimensions'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let page_count = conn
            .query_row("SELECT hits FROM totals WHERE key='pages'", [], |row| {
                row.get::<_, i64>(0)
            })
            .optional()?
            .unwrap_or_default();
        let needs_replay = previous_schema.as_deref() != Some(SCHEMA_VERSION)
            || (previous_schema.as_deref() == Some(SCHEMA_VERSION)
                && entry_exit_marker.is_none()
                && page_count > 0);
        if needs_replay {
            conn.execute_batch("DELETE FROM totals; DELETE FROM countries; DELETE FROM regions; DELETE FROM cities; DELETE FROM asns; DELETE FROM organizations; DELETE FROM statuses; DELETE FROM status_classes; DELETE FROM errors; DELETE FROM methods; DELETE FROM urls; DELETE FROM url_bandwidth; DELETE FROM users; DELETE FROM user_agents; DELETE FROM referers; DELETE FROM referrer_domains; DELETE FROM search_engines; DELETE FROM keyphrases; DELETE FROM file_types; DELETE FROM browsers; DELETE FROM oses; DELETE FROM robots; DELETE FROM request_times; DELETE FROM virtual_hosts; DELETE FROM entry_pages; DELETE FROM exit_pages; DELETE FROM hours; DELETE FROM dates; DELETE FROM daily; DELETE FROM months; DELETE FROM weekdays; DELETE FROM visit_keys; DELETE FROM session_state; DELETE FROM diagnostics; DELETE FROM input_checkpoints; DELETE FROM meta WHERE key LIKE 'input_%' OR key LIKE 'config_%';")?;
        }
        if needs_replay {
            conn.execute_batch(
                "DELETE FROM hostnames; DELETE FROM referer_spam; DELETE FROM scanners; DELETE FROM method_anomalies;",
            )?;
        }
        conn.execute(
            "INSERT INTO meta(key,value) VALUES('entry_exit_dimensions',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [ENTRY_EXIT_VERSION],
        )?;
        conn.execute(
            "INSERT INTO meta(key,value) VALUES('schema_version',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [SCHEMA_VERSION],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }
    pub fn record(&self, event: &AccessEvent, is_page: bool) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let tx = conn.unchecked_transaction()?;
        tx.execute("INSERT INTO totals(key,hits,bytes) VALUES('all',1,?1) ON CONFLICT(key) DO UPDATE SET hits=hits+1, bytes=bytes+excluded.bytes", params![event.bytes as i64])?;
        let class = if is_page { "pages" } else { "files" };
        tx.execute("INSERT INTO totals(key,hits,bytes) VALUES(?1,1,?2) ON CONFLICT(key) DO UPDATE SET hits=hits+1, bytes=bytes+excluded.bytes", params![class, event.bytes as i64])?;
        tx.execute("INSERT INTO statuses(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![event.status])?;
        tx.execute("INSERT INTO status_classes(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![format!("{}xx", event.status / 100)])?;
        if event.status >= 400 {
            tx.execute("INSERT INTO errors(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![event.status])?;
        }
        tx.execute(
            "INSERT INTO methods(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![event.method.as_deref().unwrap_or("UNKNOWN")],
        )?;
        if let Some(anomaly) = &event.method_anomaly {
            tx.execute(
                "INSERT INTO method_anomalies(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
                params![anomaly],
            )?;
        }
        if let Some(user) = event
            .user
            .as_deref()
            .filter(|user| !user.is_empty() && *user != "-")
        {
            tx.execute(
                "INSERT INTO users(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
                params![user],
            )?;
        }
        tx.execute(
            "INSERT INTO urls(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![event.url.as_deref().unwrap_or("UNKNOWN")],
        )?;
        tx.execute(
            "INSERT INTO url_bandwidth(key,hits,bytes) VALUES(?1,1,?2) ON CONFLICT(key) DO UPDATE SET hits=hits+1, bytes=bytes+excluded.bytes",
            params![event.url.as_deref().unwrap_or("UNKNOWN"), event.bytes as i64],
        )?;
        tx.execute("INSERT INTO user_agents(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![event.user_agent.as_deref().unwrap_or("UNKNOWN")])?;
        if !event.referer_excluded {
            tx.execute("INSERT INTO referers(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![event.referer.as_deref().unwrap_or("UNKNOWN")])?;
            if let Some(domain) = referrer_domain(event.referer.as_deref()) {
                tx.execute("INSERT INTO referrer_domains(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![domain])?;
            }
            if let Some((engine, phrase)) = search_referrer(event.referer.as_deref()) {
                tx.execute("INSERT INTO search_engines(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![engine])?;
                if !phrase.is_empty() {
                    tx.execute("INSERT INTO keyphrases(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![phrase])?;
                }
            }
        }
        if let Some(label) = &event.referer_spam {
            tx.execute("INSERT INTO referer_spam(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![label])?;
        }
        if let Some(label) = &event.scanner {
            tx.execute(
                "INSERT INTO scanners(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
                params![label],
            )?;
        }
        tx.execute("INSERT INTO file_types(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![file_type(event.url.as_deref())])?;
        let user_agent = event.user_agent.as_deref().unwrap_or("UNKNOWN");
        tx.execute("INSERT INTO browsers(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![browser(user_agent)])?;
        tx.execute(
            "INSERT INTO oses(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![operating_system(user_agent)],
        )?;
        if let Some(robot) = robot(user_agent) {
            tx.execute("INSERT INTO robots(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![robot])?;
        }
        if let Some(duration) = event
            .duration_seconds
            .filter(|value| value.is_finite() && *value >= 0.0)
        {
            tx.execute("INSERT INTO request_times(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![request_time_bucket(duration)])?;
        }
        if let Some(virtual_host) = &event.virtual_host {
            tx.execute("INSERT INTO virtual_hosts(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![virtual_host])?;
        }
        tx.execute(
            "INSERT INTO hours(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![event.timestamp.format("%H").to_string()],
        )?;
        tx.execute(
            "INSERT INTO dates(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![event.timestamp.format("%Y-%m-%d").to_string()],
        )?;
        tx.execute(
            "INSERT INTO months(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![event.timestamp.format("%Y-%m").to_string()],
        )?;
        tx.execute("INSERT INTO weekdays(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![event.timestamp.format("%A").to_string()])?;
        let date = event.timestamp.format("%Y-%m-%d").to_string();
        tx.execute(
            "INSERT INTO daily(key,hits,bytes,pages,files,visits) VALUES(?1,1,?2,?3,?4,0) ON CONFLICT(key) DO UPDATE SET hits=hits+1, bytes=bytes+excluded.bytes, pages=pages+excluded.pages, files=files+excluded.files",
            params![date, event.bytes as i64, is_page as i64, (!is_page) as i64],
        )?;
        let visit_key = stable_key(&[
            &event.client_ip,
            &date,
            event.user_agent.as_deref().unwrap_or(""),
        ]);
        tx.execute(
            "INSERT OR IGNORE INTO visit_keys(key) VALUES(?1)",
            params![visit_key],
        )?;
        let session_key =
            stable_key(&[&event.client_ip, event.user_agent.as_deref().unwrap_or("")]);
        let timestamp = event.timestamp.timestamp();
        let previous = tx
            .query_row(
                "SELECT last_seen,last_url FROM session_state WHERE key=?1",
                params![session_key],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        let new_session = previous
            .as_ref()
            .is_none_or(|(last_seen, _)| timestamp > *last_seen + 1800);
        if new_session {
            tx.execute("INSERT INTO totals(key,hits,bytes) VALUES('visits',1,0) ON CONFLICT(key) DO UPDATE SET hits=hits+1", [])?;
            tx.execute(
                "UPDATE daily SET visits=visits+1 WHERE key=?1",
                params![date],
            )?;
        }
        if is_page {
            let current_url = event.url.as_deref().unwrap_or("UNKNOWN");
            let previous_url = previous.as_ref().and_then(|(_, url)| url.as_deref());
            let should_update_page = previous
                .as_ref()
                .is_none_or(|(last_seen, _)| timestamp >= *last_seen);
            if new_session || previous_url.is_none() {
                bump(&tx, "entry_pages", current_url)?;
            }
            if new_session || previous_url.is_none() {
                bump(&tx, "exit_pages", current_url)?;
            } else if should_update_page && previous_url != Some(current_url) {
                if let Some(previous_url) = previous_url {
                    decrement(&tx, "exit_pages", previous_url)?;
                }
                bump(&tx, "exit_pages", current_url)?;
            }
        }
        match previous {
            None => {
                tx.execute(
                    "INSERT INTO session_state(key,last_seen,last_url) VALUES(?1,?2,?3)",
                    params![
                        session_key,
                        timestamp,
                        is_page.then(|| event.url.as_deref().unwrap_or("UNKNOWN"))
                    ],
                )?;
            }
            Some((_, _)) if new_session => {
                tx.execute(
                    "UPDATE session_state SET last_seen=?2,last_url=?3 WHERE key=?1",
                    params![
                        session_key,
                        timestamp,
                        is_page.then(|| event.url.as_deref().unwrap_or("UNKNOWN"))
                    ],
                )?;
            }
            Some((last_seen, _)) if timestamp >= last_seen => {
                if is_page {
                    tx.execute(
                        "UPDATE session_state SET last_seen=?2,last_url=?3 WHERE key=?1",
                        params![
                            session_key,
                            timestamp,
                            event.url.as_deref().unwrap_or("UNKNOWN")
                        ],
                    )?;
                } else {
                    tx.execute(
                        "UPDATE session_state SET last_seen=?2 WHERE key=?1",
                        params![session_key, timestamp],
                    )?;
                }
            }
            Some(_) => {}
        }
        if let Some(country) = &event.geo.country {
            tx.execute("INSERT INTO countries(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![country])?;
        }
        if let Some(region) = &event.geo.region {
            tx.execute("INSERT INTO regions(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![region])?;
        }
        if let Some(city) = &event.geo.city {
            tx.execute("INSERT INTO cities(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![city])?;
        }
        if let Some(asn) = event.geo.asn {
            tx.execute("INSERT INTO asns(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![asn])?;
        }
        if let Some(organization) = &event.geo.organization {
            tx.execute("INSERT INTO organizations(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![organization])?;
        }
        let visitor = visitor_label(&event.client_ip, event.client_hostname.as_deref());
        tx.execute(
            "INSERT INTO hostnames(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![visitor],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn optimize(&self, vacuum: bool) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        conn.execute_batch("PRAGMA optimize;")?;
        if vacuum {
            conn.execute_batch("VACUUM;")?;
        }
        Ok(())
    }

    pub fn backup_to(&self, path: impl AsRef<Path>) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        conn.execute("VACUUM INTO ?1", [path.as_ref().to_string_lossy().as_ref()])?;
        Ok(())
    }

    pub fn session_state_for_event(
        &self,
        event: &AccessEvent,
    ) -> Result<Option<(i64, Option<String>)>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let key = stable_key(&[&event.client_ip, event.user_agent.as_deref().unwrap_or("")]);
        Ok(conn
            .query_row(
                "SELECT last_seen,last_url FROM session_state WHERE key=?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    pub fn seed_session_state(
        &self,
        event: &AccessEvent,
        state: &(i64, Option<String>),
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let key = stable_key(&[&event.client_ip, event.user_agent.as_deref().unwrap_or("")]);
        conn.execute(
            "INSERT OR IGNORE INTO session_state(key,last_seen,last_url) VALUES(?1,?2,?3)",
            params![key, state.0, state.1],
        )?;
        Ok(())
    }

    pub fn merge_session_state(
        &self,
        event: &AccessEvent,
        state: &(i64, Option<String>),
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let key = stable_key(&[&event.client_ip, event.user_agent.as_deref().unwrap_or("")]);
        conn.execute(
            "INSERT INTO session_state(key,last_seen,last_url) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET last_seen=MAX(last_seen,excluded.last_seen), last_url=CASE WHEN excluded.last_seen >= last_seen THEN excluded.last_url ELSE last_url END",
            params![key, state.0, state.1],
        )?;
        Ok(())
    }
    pub fn diagnostic(&self, reason: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        conn.execute("INSERT INTO diagnostics(reason,count) VALUES(?1,1) ON CONFLICT(reason) DO UPDATE SET count=count+1", params![reason])?;
        Ok(())
    }
    pub fn meta(&self, key: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .ok())
    }
    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        conn.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])?;
        Ok(())
    }
    pub fn reset_checkpoint(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        conn.execute(
            "DELETE FROM meta WHERE key LIKE 'input_%' OR key LIKE 'config_%'",
            [],
        )?;
        conn.execute("DELETE FROM input_checkpoints", [])?;
        Ok(())
    }
    pub fn input_checkpoint_by_identity(
        &self,
        identity: &str,
        config_hash: &str,
    ) -> Result<Option<InputCheckpoint>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row(
                "SELECT identity,path,config_hash,content_hash,size,lines FROM input_checkpoints WHERE identity=?1 AND config_hash=?2",
                params![identity, config_hash],
                |row| {
                    Ok(InputCheckpoint {
                        identity: row.get(0)?,
                        path: row.get(1)?,
                        config_hash: row.get(2)?,
                        content_hash: row.get(3)?,
                        size: row.get::<_, i64>(4)? as u64,
                        lines: row.get::<_, i64>(5)? as u64,
                    })
                },
            )
            .optional()?)
    }
    pub fn input_checkpoint_by_content(
        &self,
        content_hash: &str,
        config_hash: &str,
    ) -> Result<Option<InputCheckpoint>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row(
                "SELECT identity,path,config_hash,content_hash,size,lines FROM input_checkpoints WHERE content_hash=?1 AND config_hash=?2 LIMIT 1",
                params![content_hash, config_hash],
                |row| {
                    Ok(InputCheckpoint {
                        identity: row.get(0)?,
                        path: row.get(1)?,
                        config_hash: row.get(2)?,
                        content_hash: row.get(3)?,
                        size: row.get::<_, i64>(4)? as u64,
                        lines: row.get::<_, i64>(5)? as u64,
                    })
                },
            )
            .optional()?)
    }

    pub fn input_checkpoint_by_content_and_path(
        &self,
        content_hash: &str,
        config_hash: &str,
        path: &str,
    ) -> Result<Option<InputCheckpoint>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row(
                "SELECT identity,path,config_hash,content_hash,size,lines FROM input_checkpoints WHERE content_hash=?1 AND config_hash=?2 AND path=?3 LIMIT 1",
                params![content_hash, config_hash, path],
                |row| {
                    Ok(InputCheckpoint {
                        identity: row.get(0)?,
                        path: row.get(1)?,
                        config_hash: row.get(2)?,
                        content_hash: row.get(3)?,
                        size: row.get::<_, i64>(4)? as u64,
                        lines: row.get::<_, i64>(5)? as u64,
                    })
                },
            )
            .optional()?)
    }
    pub fn set_input_checkpoint(&self, checkpoint: &InputCheckpoint) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        conn.execute(
            "INSERT INTO input_checkpoints(identity,path,config_hash,content_hash,size,lines) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(identity) DO UPDATE SET path=excluded.path,config_hash=excluded.config_hash,content_hash=excluded.content_hash,size=excluded.size,lines=excluded.lines",
            params![checkpoint.identity, checkpoint.path, checkpoint.config_hash, checkpoint.content_hash, checkpoint.size as i64, checkpoint.lines as i64],
        )?;
        Ok(())
    }
    pub fn clear_aggregates(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        conn.execute_batch("DELETE FROM totals; DELETE FROM countries; DELETE FROM regions; DELETE FROM cities; DELETE FROM asns; DELETE FROM organizations; DELETE FROM statuses; DELETE FROM status_classes; DELETE FROM errors; DELETE FROM methods; DELETE FROM urls; DELETE FROM url_bandwidth; DELETE FROM users; DELETE FROM user_agents; DELETE FROM referers; DELETE FROM referrer_domains; DELETE FROM search_engines; DELETE FROM keyphrases; DELETE FROM file_types; DELETE FROM browsers; DELETE FROM oses; DELETE FROM robots; DELETE FROM request_times; DELETE FROM virtual_hosts; DELETE FROM entry_pages; DELETE FROM exit_pages; DELETE FROM hours; DELETE FROM visit_keys; DELETE FROM session_state; DELETE FROM diagnostics; DELETE FROM daily;")?;
        conn.execute_batch("DELETE FROM hostnames; DELETE FROM method_anomalies;")?;
        conn.execute_batch("DELETE FROM referer_spam;")?;
        conn.execute_batch("DELETE FROM dates; DELETE FROM months; DELETE FROM weekdays;")?;
        Ok(())
    }
    pub fn totals(&self) -> Result<(u64, u64), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row("SELECT hits,bytes FROM totals WHERE key='all'", [], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
            })
            .unwrap_or((0, 0)))
    }
    pub fn metric(&self, key: &str) -> Result<(u64, u64), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row("SELECT hits,bytes FROM totals WHERE key=?1", [key], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
            })
            .unwrap_or((0, 0)))
    }
    pub fn unique_visits(&self) -> Result<u64, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row("SELECT COUNT(*) FROM visit_keys", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap_or_default() as u64)
    }
    pub fn diagnostics(&self, limit: usize) -> Result<Vec<(String, u64)>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let mut stmt = conn
            .prepare("SELECT reason,count FROM diagnostics ORDER BY count DESC,reason LIMIT ?1")?;
        let rows = stmt
            .query_map([limit as i64], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1).map(|v| v as u64)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
    pub fn top(&self, table: &str, limit: usize) -> Result<Vec<(String, u64)>, StoreError> {
        let allowed = [
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
        if !allowed.contains(&table) {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let sql = format!("SELECT key,hits FROM {table} ORDER BY hits DESC,key LIMIT ?1");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt
            .query_map([limit as i64], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1).map(|v| v as u64)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn top_if_exists(
        &self,
        table: &str,
        limit: usize,
    ) -> Result<Vec<(String, u64)>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let exists = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            params![table],
            |row| row.get::<_, bool>(0),
        )?;
        drop(conn);
        if exists {
            self.top(table, limit)
        } else {
            Ok(Vec::new())
        }
    }

    pub fn top_bandwidth(&self, limit: usize) -> Result<Vec<(String, u64, u64)>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let mut stmt = conn
            .prepare("SELECT key,hits,bytes FROM url_bandwidth ORDER BY bytes DESC,key LIMIT ?1")?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok((
                    row.get(0)?,
                    row.get::<_, i64>(1).map(|value| value as u64)?,
                    row.get::<_, i64>(2).map(|value| value as u64)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn daily(&self, limit: usize) -> Result<Vec<DailyAggregate>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let mut stmt = conn
            .prepare("SELECT key,hits,bytes,pages,files,visits FROM daily ORDER BY key LIMIT ?1")?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok(DailyAggregate {
                    date: row.get(0)?,
                    hits: row.get::<_, i64>(1).map(|value| value as u64)?,
                    bytes: row.get::<_, i64>(2).map(|value| value as u64)?,
                    pages: row.get::<_, i64>(3).map(|value| value as u64)?,
                    files: row.get::<_, i64>(4).map(|value| value as u64)?,
                    visits: row.get::<_, i64>(5).map(|value| value as u64)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn bump(tx: &Transaction<'_>, table: &str, key: &str) -> Result<(), rusqlite::Error> {
    let sql = format!(
        "INSERT INTO {table}(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1"
    );
    tx.execute(&sql, params![key])?;
    Ok(())
}

fn decrement(tx: &Transaction<'_>, table: &str, key: &str) -> Result<(), rusqlite::Error> {
    let sql = format!("UPDATE {table} SET hits=hits-1 WHERE key=?1 AND hits>1");
    let changed = tx.execute(&sql, params![key])?;
    if changed == 0 {
        let delete = format!("DELETE FROM {table} WHERE key=?1");
        tx.execute(&delete, params![key])?;
    }
    Ok(())
}

pub(crate) fn referrer_domain(referer: Option<&str>) -> Option<String> {
    let referer = referer?.trim();
    let (_, authority) = referer.split_once("://")?;
    let authority = authority.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if authority.starts_with('[') {
        authority.strip_prefix('[')?.split(']').next()?
    } else if authority.matches(':').count() == 1 {
        authority.split(':').next()?
    } else {
        authority
    };
    let host = host.trim().to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

fn stable_key(parts: &[&str]) -> String {
    // FNV-1a is used only for deterministic pseudonymous aggregation keys;
    // these values are not intended to provide cryptographic protection.
    let mut hash = 0xcbf29ce484222325u64;
    for part in parts {
        for byte in part.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn visitor_label(client_ip: &str, client_hostname: Option<&str>) -> String {
    let ip = client_ip.trim();
    match client_hostname
        .map(str::trim)
        .filter(|hostname| !hostname.is_empty() && *hostname != "-")
    {
        Some(hostname) => format!("{ip} ({hostname})"),
        None => ip.to_owned(),
    }
}

fn file_type(url: Option<&str>) -> String {
    let path = url
        .unwrap_or("UNKNOWN")
        .split('?')
        .next()
        .unwrap_or("UNKNOWN");
    let Some(extension) = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, ext)| ext)
    else {
        return "(none)".to_owned();
    };
    if extension.is_empty()
        || extension.len() > 16
        || !extension.chars().all(|c| c.is_ascii_alphanumeric())
    {
        "(none)".to_owned()
    } else {
        format!(".{}", extension.to_ascii_lowercase())
    }
}

fn search_referrer(referer: Option<&str>) -> Option<(&'static str, String)> {
    let referer = referer?;
    let (_, rest) = referer.split_once("://")?;
    let authority = rest.split(['/', '?']).next()?.to_ascii_lowercase();
    let engine = [
        ("google.", "Google"),
        ("bing.com", "Bing"),
        ("duckduckgo.com", "DuckDuckGo"),
        ("yandex.", "Yandex"),
        ("search.brave.com", "Brave"),
    ]
    .iter()
    .find_map(|(needle, label)| authority.contains(needle).then_some(*label))?;
    let query = referer.split_once('?')?.1;
    let phrase = query
        .split('&')
        .filter_map(|part| part.split_once('='))
        .find(|(key, _)| matches!(*key, "q" | "query" | "p" | "text"))
        .map(|(_, value)| decode_query(value))
        .unwrap_or_default();
    Some((engine, phrase.chars().take(256).collect()))
}

fn decode_query(value: &str) -> String {
    let mut decoded = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                if let (Some(high), Some(low)) = (high, low) {
                    decoded.push((high * 16 + low) as u8);
                    index += 3;
                } else {
                    return value.to_owned();
                }
            }
            b'%' => return value.to_owned(),
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).unwrap_or_else(|_| value.to_owned())
}

fn browser(user_agent: &str) -> &'static str {
    let ua = user_agent.to_ascii_lowercase();
    [
        ("edg/", "Edge"),
        ("opr/", "Opera"),
        ("opera", "Opera"),
        ("firefox/", "Firefox"),
        ("chrome/", "Chrome"),
        ("chromium/", "Chrome"),
        ("safari/", "Safari"),
        ("curl/", "curl"),
        ("wget/", "Wget"),
    ]
    .iter()
    .find_map(|(needle, label)| ua.contains(needle).then_some(*label))
    .unwrap_or("Other")
}

fn operating_system(user_agent: &str) -> &'static str {
    let ua = user_agent.to_ascii_lowercase();
    [
        ("windows", "Windows"),
        ("android", "Android"),
        ("iphone", "iOS"),
        ("ipad", "iOS"),
        ("ios", "iOS"),
        ("mac os", "macOS"),
        ("macintosh", "macOS"),
        ("linux", "Linux"),
    ]
    .iter()
    .find_map(|(needle, label)| ua.contains(needle).then_some(*label))
    .unwrap_or("Other")
}

fn robot(user_agent: &str) -> Option<&'static str> {
    let ua = user_agent.to_ascii_lowercase();
    [
        ("googlebot", "Googlebot"),
        ("bingbot", "Bingbot"),
        ("gptbot", "GPTBot"),
        ("crawler", "Crawler"),
        ("spider", "Spider"),
        ("bot", "Bot"),
        ("curl/", "curl"),
        ("wget/", "Wget"),
    ]
    .iter()
    .find_map(|(needle, label)| ua.contains(needle).then_some(*label))
}

fn request_time_bucket(seconds: f64) -> &'static str {
    if seconds < 0.01 {
        "<10ms"
    } else if seconds < 0.1 {
        "10-100ms"
    } else if seconds < 0.5 {
        "100-500ms"
    } else if seconds < 1.0 {
        "500ms-1s"
    } else if seconds < 5.0 {
        "1-5s"
    } else {
        ">=5s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GeoInfo;
    use chrono::DateTime;

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
            timestamp: DateTime::parse_from_rfc3339(timestamp).expect("timestamp"),
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
        anomalous.method_anomaly = Some("unrecognized:PROPFIND".into());
        store
            .record(&anomalous, true)
            .expect("record method anomaly");
        assert_eq!(
            store.top_if_exists("method_anomalies", 10).unwrap(),
            vec![("unrecognized:PROPFIND".into(), 1)]
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
        assert_eq!(store.meta("schema_version").unwrap().as_deref(), Some("13"));
        assert_eq!(store.meta("input_content_hash").unwrap(), None);
        assert_eq!(store.totals().unwrap(), (0, 0));
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
        assert_eq!(store.meta("schema_version").unwrap().as_deref(), Some("13"));
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
