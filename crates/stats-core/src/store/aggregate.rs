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

pub(super) fn record_http_aggregates(
    batch: &Batch<'_, '_>,
    event: &AccessEvent,
    is_page: bool,
    limit: usize,
) -> Result<(), StoreError> {
    let tx = batch.tx;
    tx.execute(
        "INSERT INTO totals(key,hits,bytes) VALUES('all',1,?1) ON CONFLICT(key) DO UPDATE SET hits=hits+1, bytes=bytes+excluded.bytes",
        params![event.bytes as i64],
    )?;
    let class = if is_page { "pages" } else { "files" };
    tx.execute(
        "INSERT INTO totals(key,hits,bytes) VALUES(?1,1,?2) ON CONFLICT(key) DO UPDATE SET hits=hits+1, bytes=bytes+excluded.bytes",
        params![class, event.bytes as i64],
    )?;
    tx.execute(
        "INSERT INTO statuses(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
        params![event.status],
    )?;
    tx.execute(
        "INSERT INTO status_classes(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
        params![format!("{}xx", event.status / 100)],
    )?;
    if event.status >= 400 {
        tx.execute(
            "INSERT INTO errors(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![event.status],
        )?;
    }
    if event.status == 404 {
        if let Some(url) = event.url.as_deref().filter(|url| !url.is_empty()) {
            let url = super::capped_key(
                tx,
                &batch.cardinality_counts,
                "not_found_urls",
                "url",
                url,
                limit,
            )?;
            let referrer = event
                .referer
                .as_deref()
                .filter(|value| !value.is_empty())
                .unwrap_or("-");
            tx.execute(
                "INSERT INTO not_found_urls(url,hits) VALUES(?1,1) ON CONFLICT(url) DO UPDATE SET hits=hits+1",
                params![url],
            )?;
            tx.execute(
                "INSERT INTO not_found_referrers(url,referrer,hits) VALUES(?1,?2,1) ON CONFLICT(url,referrer) DO UPDATE SET hits=hits+1",
                params![url, referrer],
            )?;
        }
    }
    tx.execute(
        "INSERT INTO methods(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
        params![event.method.as_deref().unwrap_or("UNKNOWN")],
    )?;
    if let Some(anomaly) = &event.method_anomaly {
        tx.execute(
            "INSERT INTO method_anomalies(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![anomaly],
        )?;
    }
    Ok(())
}

pub(super) fn record_user_url_aggregates(
    batch: &Batch<'_, '_>,
    event: &AccessEvent,
    limit: usize,
) -> Result<(), StoreError> {
    let tx = batch.tx;
    if let Some(user) = event
        .user
        .as_deref()
        .filter(|user| !user.is_empty() && *user != "-")
    {
        let user = super::capped_key(tx, &batch.cardinality_counts, "users", "key", user, limit)?;
        tx.execute(
            "INSERT INTO users(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![user],
        )?;
    }
    let url_key = super::capped_key(
        tx,
        &batch.cardinality_counts,
        "urls",
        "key",
        event.url.as_deref().unwrap_or("UNKNOWN"),
        limit,
    )?;
    tx.execute(
        "INSERT INTO urls(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
        params![url_key],
    )?;
    let bandwidth_key = super::capped_key(
        tx,
        &batch.cardinality_counts,
        "url_bandwidth",
        "key",
        event.url.as_deref().unwrap_or("UNKNOWN"),
        limit,
    )?;
    tx.execute(
        "INSERT INTO url_bandwidth(key,hits,bytes) VALUES(?1,1,?2) ON CONFLICT(key) DO UPDATE SET hits=hits+1, bytes=bytes+excluded.bytes",
        params![bandwidth_key, event.bytes as i64],
    )?;
    let user_agent_key = super::capped_key(
        tx,
        &batch.cardinality_counts,
        "user_agents",
        "key",
        event.user_agent.as_deref().unwrap_or("UNKNOWN"),
        limit,
    )?;
    tx.execute(
        "INSERT INTO user_agents(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
        params![user_agent_key],
    )?;
    Ok(())
}

pub(super) fn record_referer_aggregates(
    batch: &Batch<'_, '_>,
    event: &AccessEvent,
    limit: usize,
) -> Result<(), StoreError> {
    if event.referer_excluded {
        return Ok(());
    }
    let tx = batch.tx;
    let referer_key = super::capped_key(
        tx,
        &batch.cardinality_counts,
        "referers",
        "key",
        event.referer.as_deref().unwrap_or("UNKNOWN"),
        limit,
    )?;
    tx.execute(
        "INSERT INTO referers(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
        params![referer_key],
    )?;
    if let Some(domain) = super::referrer_domain(event.referer.as_deref()) {
        let domain = super::capped_key(
            tx,
            &batch.cardinality_counts,
            "referrer_domains",
            "key",
            &domain,
            limit,
        )?;
        tx.execute(
            "INSERT INTO referrer_domains(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![domain],
        )?;
    }
    if let Some((engine, phrase)) = super::search_referrer(event.referer.as_deref()) {
        tx.execute(
            "INSERT INTO search_engines(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![engine],
        )?;
        if !phrase.is_empty() {
            let phrase = super::capped_key(
                tx,
                &batch.cardinality_counts,
                "keyphrases",
                "key",
                &phrase,
                limit,
            )?;
            tx.execute(
                "INSERT INTO keyphrases(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
                params![phrase],
            )?;
        }
    }
    Ok(())
}

pub(super) fn record_security_aggregates(
    batch: &Batch<'_, '_>,
    event: &AccessEvent,
    limit: usize,
) -> Result<(), StoreError> {
    let tx = batch.tx;
    if let Some(label) = &event.referer_spam {
        let label = super::capped_key(
            tx,
            &batch.cardinality_counts,
            "referer_spam",
            "key",
            label,
            limit,
        )?;
        tx.execute(
            "INSERT INTO referer_spam(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![label],
        )?;
    }
    if let Some(label) = &event.scanner {
        let label = super::capped_key(
            tx,
            &batch.cardinality_counts,
            "scanners",
            "key",
            label,
            limit,
        )?;
        tx.execute(
            "INSERT INTO scanners(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![label],
        )?;
    }
    Ok(())
}

pub(super) fn record_client_aggregates(
    batch: &Batch<'_, '_>,
    event: &AccessEvent,
    file_type_name: String,
    browser_name: &str,
    operating_system_name: &str,
    robot_name: Option<&str>,
    limit: usize,
) -> Result<(), StoreError> {
    let tx = batch.tx;
    tx.execute(
        "INSERT INTO file_types(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
        params![file_type_name],
    )?;
    tx.execute(
        "INSERT INTO browsers(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
        params![browser_name],
    )?;
    tx.execute(
        "INSERT INTO oses(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
        params![operating_system_name],
    )?;
    if let Some(robot) = robot_name {
        tx.execute(
            "INSERT INTO robots(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![robot],
        )?;
    }
    if let Some(duration) = event
        .duration_seconds
        .filter(|value| value.is_finite() && *value >= 0.0)
    {
        tx.execute(
            "INSERT INTO request_times(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![super::request_time_bucket(duration)],
        )?;
    }
    if let Some(virtual_host) = &event.virtual_host {
        let virtual_host = super::capped_key(
            tx,
            &batch.cardinality_counts,
            "virtual_hosts",
            "key",
            virtual_host,
            limit,
        )?;
        tx.execute(
            "INSERT INTO virtual_hosts(key,hits) VALUES(?1,1) ON CONFLICT(key) DO UPDATE SET hits=hits+1",
            params![virtual_host],
        )?;
    }
    Ok(())
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
