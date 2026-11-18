use super::{Batch, StatsStore, StoreError};
use rusqlite::{params, Connection, OptionalExtension};

impl StatsStore {
    pub fn meta(&self, key: &str) -> Result<Option<String>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::meta_on(&conn, key)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::set_meta_on(&conn, key, value)
    }

    fn meta_on(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    fn set_meta_on(conn: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
        conn.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
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
}

impl Batch<'_, '_> {
    pub fn reset_checkpoint(&self) -> Result<(), StoreError> {
        StatsStore::reset_checkpoint_on(self.tx)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<(), StoreError> {
        StatsStore::set_meta_on(self.tx, key, value)
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>, StoreError> {
        StatsStore::meta_on(self.tx, key)
    }
}
