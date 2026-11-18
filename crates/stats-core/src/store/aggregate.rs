use rusqlite::{params, Transaction};

pub(super) fn bump(tx: &Transaction<'_>, table: &str, key: &str) -> Result<(), rusqlite::Error> {
    let sql = format!(
        "INSERT INTO {table}(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1"
    );
    tx.prepare_cached(&sql)?.execute(params![key])?;
    Ok(())
}

pub(super) fn decrement(
    tx: &Transaction<'_>,
    table: &str,
    key: &str,
) -> Result<(), rusqlite::Error> {
    let sql = format!("UPDATE {table} SET hits=hits-1 WHERE key=?1 AND hits>1");
    let changed = tx.prepare_cached(&sql)?.execute(params![key])?;
    if changed == 0 {
        let delete = format!("DELETE FROM {table} WHERE key=?1");
        tx.prepare_cached(&delete)?.execute(params![key])?;
    }
    Ok(())
}
