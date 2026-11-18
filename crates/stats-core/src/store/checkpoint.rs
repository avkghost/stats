use super::PrefixState;

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
