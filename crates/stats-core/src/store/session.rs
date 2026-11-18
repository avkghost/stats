use super::{stable_key, StatsStore, StoreError};
use crate::AccessEvent;
use rusqlite::{params, OptionalExtension};

impl StatsStore {
    pub fn session_state_for_event(
        &self,
        event: &AccessEvent,
    ) -> Result<Option<(i64, Option<String>)>, StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let key = stable_key(&[&event.client_ip, event.user_agent.as_deref().unwrap_or("")]);
        Ok(conn
            .query_row(
                "SELECT last_seen,last_url FROM session_state WHERE key=?1",
                params![key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    pub fn seed_session_state(
        &self,
        event: &AccessEvent,
        state: &(i64, Option<String>),
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let key = stable_key(&[&event.client_ip, event.user_agent.as_deref().unwrap_or("")]);
        conn.execute(
            "INSERT OR IGNORE INTO session_state(key,last_seen,last_url) VALUES(?1,?2,?3)",
            params![key, state.0, state.1],
        )?;
        Ok(())
    }

    pub fn merge_session_state(
        &self,
        event: &AccessEvent,
        state: &(i64, Option<String>),
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|_| StoreError::Poisoned)?;
        let key = stable_key(&[&event.client_ip, event.user_agent.as_deref().unwrap_or("")]);
        conn.execute(
            "INSERT INTO session_state(key,last_seen,last_url) VALUES(?1,?2,?3) ON CONFLICT(key) DO UPDATE SET last_seen=MAX(last_seen,excluded.last_seen), last_url=CASE WHEN excluded.last_seen >= last_seen THEN excluded.last_url ELSE last_url END",
            params![key, state.0, state.1],
        )?;
        Ok(())
    }
}
