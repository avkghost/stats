use super::{DailyAggregate, StatsStore, StoreError, TOP_TABLES};
use rusqlite::{params, Connection, OptionalExtension};

impl StatsStore {
    pub fn diagnostic(&self, reason: &str) -> Result<(), StoreError> {
        self.diagnostic_with_cardinality_limit(reason, super::DEFAULT_CARDINALITY_LIMIT)
    }

    pub fn diagnostic_with_cardinality_limit(
        &self,
        reason: &str,
        limit: usize,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::diagnostic_on(&conn, reason, limit)
    }

    fn diagnostic_on(conn: &Connection, reason: &str, limit: usize) -> Result<(), StoreError> {
        let key = super::capped_key_connection(conn, "diagnostics", "reason", reason, limit)?;
        conn.execute("INSERT INTO diagnostics(reason,count) VALUES(?1,1) ON CONFLICT(reason) DO UPDATE SET count=count+1", params![key])?;
        Ok(())
    }

    pub fn totals(&self) -> Result<(u64, u64), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row("SELECT hits,bytes FROM totals WHERE key='all'", [], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64))
            })
            .optional()?
            .unwrap_or((0, 0)))
    }

    pub fn metric(&self, key: &str) -> Result<(u64, u64), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(conn
            .query_row("SELECT hits,bytes FROM totals WHERE key=?1", [key], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64))
            })
            .optional()?
            .unwrap_or((0, 0)))
    }

    pub fn unique_visits(&self) -> Result<u64, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Ok(
            conn.query_row("SELECT COUNT(*) FROM visit_keys", [], |row| {
                row.get::<_, i64>(0)
            })? as u64,
        )
    }

    pub fn diagnostics(&self, limit: usize) -> Result<Vec<(String, u64)>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let mut stmt = conn
            .prepare("SELECT reason,count FROM diagnostics ORDER BY count DESC,reason LIMIT ?1")?;
        let rows = stmt
            .query_map([limit as i64], |row| {
                Ok((row.get(0)?, row.get::<_, i64>(1).map(|value| value as u64)?))
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
            .query_map([limit as i64], |row| {
                Ok((row.get(0)?, row.get::<_, i64>(1).map(|value| value as u64)?))
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
