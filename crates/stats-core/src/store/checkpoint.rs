use super::{PrefixState, StoreError};
use rusqlite::Connection;

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
