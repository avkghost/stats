use crate::AccessEvent;
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use std::{path::Path, sync::Mutex, time::Duration};
use thiserror::Error;

mod aggregate;
mod checkpoint;
mod classify;
mod schema;
mod session;
use aggregate::{bump, decrement};
use checkpoint::{ensure_checkpoint_columns, prefix_state_row};
#[cfg(test)]
use classify::decode_query;
pub(crate) use classify::referrer_domain;
use classify::{
    browser, file_type, operating_system, request_time_bucket, robot, search_referrer, stable_key,
    visitor_label,
};
#[cfg(test)]
use schema::SERVICE_TABLES;
use schema::{AGGREGATE_SCHEMA, TOP_TABLES};

const SCHEMA_VERSION: &str = "13";
const ENTRY_EXIT_VERSION: &str = "1";

fn aggregate_tables() -> impl Iterator<Item = &'static str> {
    AGGREGATE_SCHEMA.iter().map(|(name, _)| *name)
}

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

/// One stored state of a stream's complete-line prefix (plan decision I).
///
/// For active logs, `size` is the position just after the last newline inside
/// the processed bound, so an unterminated fragment is never part of a state.
/// For archives, `size` is the full decoded length. `sha256` covers exactly
/// the first `size` decoded bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixState {
    pub source_key: String,
    pub config_hash: String,
    pub identity: String,
    pub path: String,
    pub size: u64,
    pub lines: u64,
    pub sha256: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotFoundAggregate {
    pub url: String,
    pub hits: u64,
    pub referrers: Vec<(String, u64)>,
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
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        for (name, columns) in AGGREGATE_SCHEMA {
            conn.execute(&format!("CREATE TABLE IF NOT EXISTS {name}({columns})"), [])?;
        }
        let has_last_url = conn
            .prepare("PRAGMA table_info(session_state)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .any(|name| name == "last_url");
        if !has_last_url {
            conn.execute("ALTER TABLE session_state ADD COLUMN last_url TEXT", [])?;
        }
        conn.execute_batch("CREATE TABLE IF NOT EXISTS input_checkpoints(identity TEXT PRIMARY KEY, path TEXT NOT NULL, config_hash TEXT NOT NULL, content_hash TEXT NOT NULL, size INTEGER NOT NULL, lines INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS input_checkpoints_content ON input_checkpoints(content_hash, config_hash);")?;
        ensure_checkpoint_columns(&conn)?;
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
            // A schema replay invalidates every value derived from input events. Prefix snapshots
            // are service state rather than aggregates, but they describe the old interpretation
            // of the input stream and must be removed with the checkpoints; retaining them could
            // make the next replay skip records under the new schema.
            Self::clear_aggregates_on(&conn)?;
            conn.execute_batch(
                "DELETE FROM input_checkpoints; DELETE FROM input_prefix_snapshots; DELETE FROM meta WHERE key LIKE 'input_%' OR key LIKE 'config_%';",
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
        self.with_batch(|batch| batch.record(event, is_page))
    }

    fn record_in(
        tx: &Transaction<'_>,
        event: &AccessEvent,
        is_page: bool,
    ) -> Result<(), StoreError> {
        tx.execute("INSERT INTO totals(key,hits,bytes) VALUES('all',1,?1) ON CONFLICT(key) DO UPDATE SET hits=hits+1, bytes=bytes+excluded.bytes", params![event.bytes as i64])?;
        let class = if is_page { "pages" } else { "files" };
        tx.execute("INSERT INTO totals(key,hits,bytes) VALUES(?1,1,?2) ON CONFLICT(key) DO UPDATE SET hits=hits+1, bytes=bytes+excluded.bytes", params![class, event.bytes as i64])?;
        tx.execute("INSERT INTO statuses(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![event.status])?;
        tx.execute("INSERT INTO status_classes(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![format!("{}xx", event.status / 100)])?;
        if event.status >= 400 {
            tx.execute("INSERT INTO errors(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1", params![event.status])?;
        }
        if event.status == 404 {
            if let Some(url) = event.url.as_deref().filter(|url| !url.is_empty()) {
                let referrer = event
                    .referer
                    .as_deref()
                    .filter(|value| !value.is_empty())
                    .unwrap_or("-");
                tx.execute(
                    "INSERT INTO not_found_urls(url,hits) VALUES(?1,1) ON CONFLICT(url) DO UPDATE SET hits=hits+1",
                    params![url],
                )?;
                tx.execute(
                    "INSERT INTO not_found_referrers(url,referrer,hits) VALUES(?1,?2,1) ON CONFLICT(url,referrer) DO UPDATE SET hits=hits+1",
                    params![url, referrer],
                )?;
            }
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
                bump(tx, "entry_pages", current_url)?;
            }
            if new_session || previous_url.is_none() {
                bump(tx, "exit_pages", current_url)?;
            } else if should_update_page && previous_url != Some(current_url) {
                if let Some(previous_url) = previous_url {
                    decrement(tx, "exit_pages", previous_url)?;
                }
                bump(tx, "exit_pages", current_url)?;
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

    pub fn diagnostic(&self, reason: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::diagnostic_on(&conn, reason)
    }

    fn diagnostic_on(conn: &Connection, reason: &str) -> Result<(), StoreError> {
        conn.execute("INSERT INTO diagnostics(reason,count) VALUES(?1,1) ON CONFLICT(reason) DO UPDATE SET count=count+1", params![reason])?;
        Ok(())
    }
    pub fn meta(&self, key: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
            .optional()?)
    }
    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::set_meta_on(&conn, key, value)
    }

    fn set_meta_on(conn: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
        conn.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])?;
        Ok(())
    }
    pub fn reset_checkpoint(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::reset_checkpoint_on(&conn)
    }

    fn reset_checkpoint_on(conn: &Connection) -> Result<(), StoreError> {
        conn.execute(
            "DELETE FROM meta WHERE key LIKE 'input_%' OR key LIKE 'config_%'",
            [],
        )?;
        conn.execute("DELETE FROM input_checkpoints", [])?;
        conn.execute("DELETE FROM input_prefix_snapshots", [])?;
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
        Self::set_input_checkpoint_on(&conn, checkpoint)
    }

    fn set_input_checkpoint_on(
        conn: &Connection,
        checkpoint: &InputCheckpoint,
    ) -> Result<(), StoreError> {
        conn.execute(
            "INSERT INTO input_checkpoints(identity,path,config_hash,content_hash,size,lines) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(identity) DO UPDATE SET path=excluded.path,config_hash=excluded.config_hash,content_hash=excluded.content_hash,size=excluded.size,lines=excluded.lines",
            params![checkpoint.identity, checkpoint.path, checkpoint.config_hash, checkpoint.content_hash, checkpoint.size as i64, checkpoint.lines as i64],
        )?;
        Ok(())
    }
    pub fn clear_aggregates(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::clear_aggregates_on(&conn)
    }

    fn clear_aggregates_on(conn: &Connection) -> Result<(), StoreError> {
        // Table names come exclusively from the static schema list above, so this dynamic SQL
        // cannot receive user input. Using one iterator here also makes clear-on-replay and
        // clear-on-config-change cover exactly the same set of tables.
        for table in aggregate_tables() {
            conn.execute(&format!("DELETE FROM {table}"), [])?;
        }
        Ok(())
    }
    pub fn totals(&self) -> Result<(u64, u64), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        // `optional()` distinguishes a missing aggregate row (a valid empty store) from a
        // missing/corrupt table (an operational error that callers must see).
        Ok(conn
            .query_row("SELECT hits,bytes FROM totals WHERE key='all'", [], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
            })
            .optional()?
            .unwrap_or((0, 0)))
    }
    pub fn metric(&self, key: &str) -> Result<(u64, u64), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row("SELECT hits,bytes FROM totals WHERE key=?1", [key], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
            })
            .optional()?
            .unwrap_or((0, 0)))
    }
    pub fn unique_visits(&self) -> Result<u64, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn.query_row("SELECT COUNT(*) FROM visit_keys", [], |r| {
            r.get::<_, i64>(0)
        })? as u64)
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
        if !TOP_TABLES.contains(&table) {
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

    pub fn not_found(&self, limit: usize) -> Result<Vec<NotFoundAggregate>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let exists = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='not_found_urls')",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            return Ok(Vec::new());
        }
        let mut urls =
            conn.prepare("SELECT url,hits FROM not_found_urls ORDER BY hits DESC,url LIMIT ?1")?;
        let rows = urls
            .query_map([limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut result = Vec::with_capacity(rows.len());
        for (url, hits) in rows {
            let mut refs = conn.prepare(
                "SELECT referrer,hits FROM not_found_referrers WHERE url=?1 ORDER BY hits DESC,referrer LIMIT 10",
            )?;
            let referrers = refs
                .query_map([&url], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            result.push(NotFoundAggregate {
                url,
                hits,
                referrers,
            });
        }
        Ok(result)
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

/// Writes and reads bound to one open transaction. Created only by
/// [`StatsStore::with_batch`]; nothing written through it is visible until the
/// batch commits.
pub struct Batch<'t, 'c> {
    tx: &'t Transaction<'c>,
}

impl Batch<'_, '_> {
    pub fn record(&self, event: &AccessEvent, is_page: bool) -> Result<(), StoreError> {
        StatsStore::record_in(self.tx, event, is_page)
    }

    pub fn set_input_checkpoint(&self, checkpoint: &InputCheckpoint) -> Result<(), StoreError> {
        StatsStore::set_input_checkpoint_on(self.tx, checkpoint)
    }

    pub fn set_prefix_state(&self, state: &PrefixState) -> Result<(), StoreError> {
        StatsStore::set_prefix_state_on(self.tx, state)
    }

    pub fn snapshot_prefix(&self, state: &PrefixState, captured_at: i64) -> Result<(), StoreError> {
        StatsStore::snapshot_prefix_on(self.tx, state, captured_at)
    }

    pub fn delete_checkpoint(&self, identity: &str) -> Result<(), StoreError> {
        StatsStore::delete_checkpoint_on(self.tx, identity)
    }

    pub fn prefix_states(
        &self,
        source_key: &str,
        config_hash: &str,
    ) -> Result<Vec<PrefixState>, StoreError> {
        StatsStore::prefix_states_on(self.tx, source_key, config_hash)
    }

    pub fn snapshot_states(
        &self,
        source_key: &str,
        config_hash: &str,
    ) -> Result<Vec<PrefixState>, StoreError> {
        StatsStore::snapshot_states_on(self.tx, source_key, config_hash)
    }

    pub fn legacy_checkpoint_count(
        &self,
        source_key: &str,
        config_hash: &str,
    ) -> Result<u64, StoreError> {
        StatsStore::legacy_checkpoint_count_on(self.tx, source_key, config_hash)
    }

    pub fn clear_aggregates(&self) -> Result<(), StoreError> {
        StatsStore::clear_aggregates_on(self.tx)
    }

    pub fn reset_checkpoint(&self) -> Result<(), StoreError> {
        StatsStore::reset_checkpoint_on(self.tx)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), StoreError> {
        StatsStore::set_meta_on(self.tx, key, value)
    }

    pub fn diagnostic(&self, reason: &str) -> Result<(), StoreError> {
        StatsStore::diagnostic_on(self.tx, reason)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, StoreError> {
        StatsStore::meta_on(self.tx, key)
    }

    pub fn input_checkpoint_by_identity(
        &self,
        identity: &str,
        config_hash: &str,
    ) -> Result<Option<InputCheckpoint>, StoreError> {
        StatsStore::input_checkpoint_by_identity_on(self.tx, identity, config_hash)
    }

    /// Renames the configuration hash on every checkpoint and snapshot row, keeping them.
    pub fn migrate_config_hash(&self, from: &str, to: &str) -> Result<(), StoreError> {
        self.tx.execute(
            "UPDATE input_checkpoints SET config_hash=?2 WHERE config_hash=?1",
            params![from, to],
        )?;
        self.tx.execute(
            "UPDATE input_prefix_snapshots SET config_hash=?2 WHERE config_hash=?1",
            params![from, to],
        )?;
        Ok(())
    }
}

impl StatsStore {
    /// Runs `f` inside one `BEGIN IMMEDIATE` transaction on this store.
    ///
    /// Writes made through the [`Batch`] commit together when `f` returns `Ok`
    /// and roll back together when it returns `Err`. `f` must not call other
    /// methods on this store: the connection mutex is held for the whole batch
    /// and is not re-entrant.
    pub fn with_batch<T, E: From<StoreError>>(
        &self,
        f: impl FnOnce(&Batch<'_, '_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| E::from(StoreError::Poisoned))?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| E::from(StoreError::from(error)))?;
        let value = f(&Batch { tx: &tx })?;
        tx.commit()
            .map_err(|error| E::from(StoreError::from(error)))?;
        Ok(value)
    }

    pub fn set_prefix_state(&self, state: &PrefixState) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::set_prefix_state_on(&conn, state)
    }

    pub fn prefix_states(
        &self,
        source_key: &str,
        config_hash: &str,
    ) -> Result<Vec<PrefixState>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::prefix_states_on(&conn, source_key, config_hash)
    }

    pub fn snapshot_states(
        &self,
        source_key: &str,
        config_hash: &str,
    ) -> Result<Vec<PrefixState>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::snapshot_states_on(&conn, source_key, config_hash)
    }

    pub fn legacy_checkpoint_count(
        &self,
        source_key: &str,
        config_hash: &str,
    ) -> Result<u64, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::legacy_checkpoint_count_on(&conn, source_key, config_hash)
    }

    pub fn snapshot_prefix(&self, state: &PrefixState, captured_at: i64) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::snapshot_prefix_on(&conn, state, captured_at)
    }

    pub fn delete_checkpoint(&self, identity: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::delete_checkpoint_on(&conn, identity)
    }

    fn set_prefix_state_on(conn: &Connection, state: &PrefixState) -> Result<(), StoreError> {
        // Inserted rows reuse the size and line count for the legacy columns, which
        // are NOT NULL. Existing rows keep their legacy content_hash, size, and lines.
        conn.execute(
            "INSERT INTO input_checkpoints(identity,path,config_hash,content_hash,size,lines,source_key,prefix_size,prefix_lines,prefix_sha256) VALUES(?1,?2,?3,'',?4,?5,?6,?4,?5,?7) ON CONFLICT(identity) DO UPDATE SET path=excluded.path,config_hash=excluded.config_hash,source_key=excluded.source_key,prefix_size=excluded.prefix_size,prefix_lines=excluded.prefix_lines,prefix_sha256=excluded.prefix_sha256",
            params![
                state.identity,
                state.path,
                state.config_hash,
                state.size as i64,
                state.lines as i64,
                state.source_key,
                state.sha256
            ],
        )?;
        Ok(())
    }

    fn prefix_states_on(
        conn: &Connection,
        source_key: &str,
        config_hash: &str,
    ) -> Result<Vec<PrefixState>, StoreError> {
        let mut statement = conn.prepare(
            "SELECT identity,path,config_hash,source_key,prefix_size,prefix_lines,prefix_sha256 FROM input_checkpoints WHERE source_key=?1 AND config_hash=?2 AND prefix_sha256 IS NOT NULL ORDER BY identity",
        )?;
        let rows = statement.query_map(params![source_key, config_hash], prefix_state_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn snapshot_states_on(
        conn: &Connection,
        source_key: &str,
        config_hash: &str,
    ) -> Result<Vec<PrefixState>, StoreError> {
        let mut statement = conn.prepare(
            "SELECT identity,path,config_hash,source_key,prefix_size,prefix_lines,prefix_sha256 FROM input_prefix_snapshots WHERE source_key=?1 AND config_hash=?2 ORDER BY captured_at, identity",
        )?;
        let rows = statement.query_map(params![source_key, config_hash], prefix_state_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Legacy rows have no stable prefix fields. Rows written before `source_key`
    /// existed carry an empty `source_key`, so they are counted for every source.
    fn legacy_checkpoint_count_on(
        conn: &Connection,
        source_key: &str,
        config_hash: &str,
    ) -> Result<u64, StoreError> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM input_checkpoints WHERE (source_key=?1 OR source_key='') AND config_hash=?2 AND prefix_sha256 IS NULL",
            params![source_key, config_hash],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    fn snapshot_prefix_on(
        conn: &Connection,
        state: &PrefixState,
        captured_at: i64,
    ) -> Result<(), StoreError> {
        conn.execute(
            "INSERT OR IGNORE INTO input_prefix_snapshots(source_key,config_hash,prefix_size,prefix_lines,prefix_sha256,identity,path,captured_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                state.source_key,
                state.config_hash,
                state.size as i64,
                state.lines as i64,
                state.sha256,
                state.identity,
                state.path,
                captured_at
            ],
        )?;
        Ok(())
    }

    fn meta_on(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    fn input_checkpoint_by_identity_on(
        conn: &Connection,
        identity: &str,
        config_hash: &str,
    ) -> Result<Option<InputCheckpoint>, StoreError> {
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

    fn delete_checkpoint_on(conn: &Connection, identity: &str) -> Result<(), StoreError> {
        conn.execute(
            "DELETE FROM input_checkpoints WHERE identity=?1",
            params![identity],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccessTimestamp, GeoInfo};

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
        assert_eq!(store.meta("schema_version").unwrap().as_deref(), Some("13"));
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
        assert_eq!(store.meta("schema_version").unwrap().as_deref(), Some("13"));
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
            (5, 500),
            "aggregates survive the upgrade"
        );
        assert_eq!(store.meta("schema_version").unwrap().as_deref(), Some("13"));
        assert_eq!(store.legacy_checkpoint_count("src", "cfg").unwrap(), 1);
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
