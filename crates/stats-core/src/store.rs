use crate::AccessEvent;
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use std::{cell::RefCell, collections::HashMap};
use std::{path::Path, sync::Mutex, time::Duration};
use thiserror::Error;

mod aggregate;
mod checkpoint;
mod classify;
mod maintenance;
mod metadata;
mod report;
mod schema;
mod session;
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
use schema::{AGGREGATE_SCHEMA, CARDINALITY_TABLES, TOP_TABLES};

const SCHEMA_VERSION: &str = "14";
const ENTRY_EXIT_VERSION: &str = "1";
pub const DEFAULT_CARDINALITY_LIMIT: usize = 10_000;
pub const PREPARED_STATEMENT_CACHE_CAPACITY: usize = if CARDINALITY_TABLES.len() * 2 > 128 {
    CARDINALITY_TABLES.len() * 2
} else {
    128
};
const _: () = assert!(PREPARED_STATEMENT_CACHE_CAPACITY >= CARDINALITY_TABLES.len() * 2);

fn aggregate_tables() -> impl Iterator<Item = &'static str> {
    AGGREGATE_SCHEMA.iter().map(|(name, _)| *name)
}

fn capped_key(
    tx: &Transaction<'_>,
    counts: &RefCell<HashMap<String, usize>>,
    table: &str,
    column: &str,
    key: &str,
    limit: usize,
) -> Result<String, rusqlite::Error> {
    if limit == usize::MAX {
        return Ok(key.to_owned());
    }
    let exists_sql = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {column}=?1)");
    if tx
        .prepare_cached(&exists_sql)?
        .query_row([key], |row| row.get::<_, bool>(0))?
    {
        return Ok(key.to_owned());
    }
    let cached_count = { counts.borrow().get(table).copied() };
    let count = match cached_count {
        Some(count) => count,
        None => {
            let count_sql = format!("SELECT COUNT(*) FROM {table}");
            let count = tx
                .prepare_cached(&count_sql)?
                .query_row([], |row| row.get::<_, i64>(0))? as usize;
            counts.borrow_mut().insert(table.to_owned(), count);
            count
        }
    };
    if count < limit.max(1) {
        counts
            .borrow_mut()
            .entry(table.to_owned())
            .and_modify(|count| *count += 1);
    }
    Ok(if count >= limit.max(1) {
        "other".to_owned()
    } else {
        key.to_owned()
    })
}

fn capped_key_connection(
    conn: &Connection,
    table: &str,
    column: &str,
    key: &str,
    limit: usize,
) -> Result<String, rusqlite::Error> {
    if limit == usize::MAX {
        return Ok(key.to_owned());
    }
    let exists_sql = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE {column}=?1)");
    if conn
        .prepare_cached(&exists_sql)?
        .query_row([key], |row| row.get::<_, bool>(0))?
    {
        return Ok(key.to_owned());
    }
    let count_sql = format!("SELECT COUNT(*) FROM {table}");
    let count = conn
        .prepare_cached(&count_sql)?
        .query_row([], |row| row.get::<_, i64>(0))? as usize;
    Ok(if count >= limit.max(1) {
        "other".to_owned()
    } else {
        key.to_owned()
    })
}

fn capped_key_transaction(
    tx: &Transaction<'_>,
    counts: &RefCell<HashMap<String, usize>>,
    table: &str,
    column: &str,
    key: &str,
    limit: usize,
) -> Result<String, rusqlite::Error> {
    capped_key(tx, counts, table, column, key, limit)
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
        // Capped dimensions use one existence and one count statement each, so the cache must hold
        // all of them plus the batch statements; the rusqlite default is too small for that.
        conn.set_prepared_statement_cache_capacity(PREPARED_STATEMENT_CACHE_CAPACITY);
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
}

/// Writes and reads bound to one open transaction. Created only by
/// [`StatsStore::with_batch`]; nothing written through it is visible until the
/// batch commits.
pub struct Batch<'t, 'c> {
    tx: &'t Transaction<'c>,
    cardinality_counts: RefCell<HashMap<String, usize>>,
}

impl Batch<'_, '_> {
    pub fn clear_aggregates(&self) -> Result<(), StoreError> {
        StatsStore::clear_aggregates_on(self.tx)?;
        self.cardinality_counts.borrow_mut().clear();
        Ok(())
    }

    pub fn diagnostic(&self, reason: &str) -> Result<(), StoreError> {
        self.diagnostic_with_cardinality_limit(reason, DEFAULT_CARDINALITY_LIMIT)
    }

    pub fn diagnostic_with_cardinality_limit(
        &self,
        reason: &str,
        limit: usize,
    ) -> Result<(), StoreError> {
        let key = capped_key_transaction(
            self.tx,
            &self.cardinality_counts,
            "diagnostics",
            "reason",
            reason,
            limit,
        )?;
        self.tx.execute("INSERT INTO diagnostics(reason,count) VALUES(?1,1) ON CONFLICT(reason) DO UPDATE SET count=count+1", params![key])?;
        Ok(())
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
        let value = f(&Batch {
            tx: &tx,
            cardinality_counts: RefCell::new(HashMap::new()),
        })?;
        tx.commit()
            .map_err(|error| E::from(StoreError::from(error)))?;
        Ok(value)
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
mod tests;
