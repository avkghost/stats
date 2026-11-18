use super::{StatsStore, StoreError};
use std::path::Path;

impl StatsStore {
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
}
