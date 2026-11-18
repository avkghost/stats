use super::{Batch, InputCheckpoint, PrefixState, StatsStore, StoreError};
use rusqlite::{params, Connection, OptionalExtension};

impl StatsStore {
    pub fn set_input_checkpoint(&self, checkpoint: &InputCheckpoint) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        Self::set_input_checkpoint_on(&conn, checkpoint)
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
                checkpoint_row,
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
                checkpoint_row,
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
                checkpoint_row,
            )
            .optional()?)
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
}

impl Batch<'_, '_> {
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
}

impl StatsStore {
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
}

fn checkpoint_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<InputCheckpoint> {
    Ok(InputCheckpoint {
        identity: row.get(0)?,
        path: row.get(1)?,
        config_hash: row.get(2)?,
        content_hash: row.get(3)?,
        size: row.get::<_, i64>(4)? as u64,
        lines: row.get::<_, i64>(5)? as u64,
    })
}

pub(super) fn prefix_state_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PrefixState> {
    Ok(PrefixState {
        identity: row.get(0)?,
        path: row.get(1)?,
        config_hash: row.get(2)?,
        source_key: row.get(3)?,
        size: row.get::<_, i64>(4)? as u64,
        lines: row.get::<_, i64>(5)? as u64,
        sha256: row.get(6)?,
    })
}

/// Adds the prefix columns and snapshot table to a pre-existing database.
///
/// Additive only: existing aggregates and checkpoints remain valid while older
/// stores acquire the current checkpoint metadata columns.
pub(super) fn ensure_checkpoint_columns(conn: &Connection) -> Result<(), StoreError> {
    let existing = conn
        .prepare("PRAGMA table_info(input_checkpoints)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    for (name, declaration) in [
        ("source_key", "TEXT NOT NULL DEFAULT ''"),
        ("prefix_size", "INTEGER"),
        ("prefix_lines", "INTEGER"),
        ("prefix_sha256", "TEXT"),
    ] {
        if !existing.iter().any(|column| column.as_str() == name) {
            conn.execute(
                &format!("ALTER TABLE input_checkpoints ADD COLUMN {name} {declaration}"),
                [],
            )?;
        }
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS input_prefix_snapshots(source_key TEXT NOT NULL, config_hash TEXT NOT NULL, prefix_size INTEGER NOT NULL, prefix_lines INTEGER NOT NULL, prefix_sha256 TEXT NOT NULL, identity TEXT NOT NULL, path TEXT NOT NULL, captured_at INTEGER NOT NULL, PRIMARY KEY(source_key, config_hash, prefix_sha256)); CREATE INDEX IF NOT EXISTS input_prefix_snapshots_lookup ON input_prefix_snapshots(source_key, config_hash, captured_at);",
    )?;
    Ok(())
}
