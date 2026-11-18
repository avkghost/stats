use super::{AccessEvent, Batch, StatsStore, StoreError, DEFAULT_CARDINALITY_LIMIT};
use rusqlite::{params, Transaction};

impl StatsStore {
    pub fn record(&self, event: &AccessEvent, is_page: bool) -> Result<(), StoreError> {
        self.record_with_cardinality_limit(event, is_page, DEFAULT_CARDINALITY_LIMIT)
    }

    pub fn record_with_cardinality_limit(
        &self,
        event: &AccessEvent,
        is_page: bool,
        limit: usize,
    ) -> Result<(), StoreError> {
        self.with_batch(|batch| batch.record_with_cardinality_limit(event, is_page, limit))
    }
}

impl Batch<'_, '_> {
    pub fn record(&self, event: &AccessEvent, is_page: bool) -> Result<(), StoreError> {
        self.record_with_cardinality_limit(event, is_page, DEFAULT_CARDINALITY_LIMIT)
    }

    pub fn record_with_cardinality_limit(
        &self,
        event: &AccessEvent,
        is_page: bool,
        limit: usize,
    ) -> Result<(), StoreError> {
        StatsStore::record_in(self, event, is_page, limit)
    }
}

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
