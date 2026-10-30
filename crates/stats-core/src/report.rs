use crate::StatsStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize, Serialize)]
pub struct Report {
    pub metadata: ReportMetadata,
    pub hits: u64,
    pub bytes: u64,
    pub pages: u64,
    pub files: u64,
    pub visits: u64,
    pub unique_visits: u64,
    pub countries: Vec<ReportRow>,
    pub regions: Vec<ReportRow>,
    pub cities: Vec<ReportRow>,
    pub asns: Vec<ReportRow>,
    pub organizations: Vec<ReportRow>,
    pub hostnames: Vec<ReportRow>,
    pub statuses: Vec<ReportRow>,
    pub status_classes: Vec<ReportRow>,
    pub errors: Vec<ReportRow>,
    pub methods: Vec<ReportRow>,
    pub urls: Vec<ReportRow>,
    pub bandwidth_urls: Vec<BandwidthRow>,
    pub daily: Vec<DailyRow>,
    pub users: Vec<ReportRow>,
    pub user_agents: Vec<ReportRow>,
    pub referers: Vec<ReportRow>,
    pub referrer_domains: Vec<ReportRow>,
    pub referer_spam: Vec<ReportRow>,
    pub file_types: Vec<ReportRow>,
    pub browsers: Vec<ReportRow>,
    pub oses: Vec<ReportRow>,
    pub robots: Vec<ReportRow>,
    pub request_times: Vec<ReportRow>,
    pub virtual_hosts: Vec<ReportRow>,
    pub entry_pages: Vec<ReportRow>,
    pub exit_pages: Vec<ReportRow>,
    pub search_engines: Vec<ReportRow>,
    pub keyphrases: Vec<ReportRow>,
    pub hours: Vec<ReportRow>,
    pub dates: Vec<ReportRow>,
    pub months: Vec<ReportRow>,
    pub weekdays: Vec<ReportRow>,
    pub diagnostics: Vec<ReportRow>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReportMetadata {
    pub schema_version: Option<String>,
    pub input_path: Option<String>,
    pub input_size: Option<u64>,
    pub input_lines: Option<u64>,
    pub config_path: Option<String>,
    pub config_hash: Option<String>,
    pub approximations: Vec<String>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct ReportRow {
    pub key: String,
    pub hits: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BandwidthRow {
    pub key: String,
    pub hits: u64,
    pub bytes: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DailyRow {
    pub date: String,
    pub hits: u64,
    pub bytes: u64,
    pub pages: u64,
    pub files: u64,
    pub visits: u64,
}

pub struct ReportService<'a> {
    store: &'a StatsStore,
}
impl<'a> ReportService<'a> {
    pub fn new(store: &'a StatsStore) -> Self {
        Self { store }
    }
    pub fn build(&self, limit: usize) -> anyhow::Result<Report> {
        let (hits, bytes) = self.store.totals()?;
        let (pages, _) = self.store.metric("pages")?;
        let (files, _) = self.store.metric("files")?;
        let (visits, _) = self.store.metric("visits")?;
        let metadata = ReportMetadata {
            schema_version: self.store.meta("schema_version")?,
            input_path: self.store.meta("input_path")?,
            input_size: self
                .store
                .meta("input_size")?
                .and_then(|value| value.parse().ok()),
            input_lines: self
                .store
                .meta("input_lines")?
                .and_then(|value| value.parse().ok()),
            config_path: self.store.meta("config_path")?,
            config_hash: self.store.meta("config_hash")?,
            approximations: vec![
                "unique_visits are anonymized IP/user-agent/day estimates".into(),
                "visits use a 30-minute inactivity heuristic".into(),
                "robot and browser classifications use user-agent heuristics".into(),
            ],
        };
        let rows = |table| {
            self.store.top(table, limit).map(|v| {
                v.into_iter()
                    .map(|(key, hits)| ReportRow { key, hits })
                    .collect()
            })
        };
        Ok(Report {
            metadata,
            hits,
            bytes,
            pages,
            files,
            visits,
            unique_visits: self.store.unique_visits()?,
            countries: rows("countries")?,
            regions: rows("regions")?,
            cities: rows("cities")?,
            asns: rows("asns")?,
            organizations: rows("organizations")?,
            hostnames: self
                .store
                .top_if_exists("hostnames", limit)?
                .into_iter()
                .map(|(key, hits)| ReportRow { key, hits })
                .collect(),
            statuses: rows("statuses")?,
            status_classes: rows("status_classes")?,
            errors: rows("errors")?,
            methods: rows("methods")?,
            urls: rows("urls")?,
            bandwidth_urls: self
                .store
                .top_bandwidth(limit)?
                .into_iter()
                .map(|(key, hits, bytes)| BandwidthRow { key, hits, bytes })
                .collect(),
            daily: self
                .store
                .daily(limit)?
                .into_iter()
                .map(|row| DailyRow {
                    date: row.date,
                    hits: row.hits,
                    bytes: row.bytes,
                    pages: row.pages,
                    files: row.files,
                    visits: row.visits,
                })
                .collect(),
            users: rows("users")?,
            user_agents: rows("user_agents")?,
            referers: rows("referers")?,
            referrer_domains: rows("referrer_domains")?,
            referer_spam: self
                .store
                .top_if_exists("referer_spam", limit)?
                .into_iter()
                .map(|(key, hits)| ReportRow { key, hits })
                .collect(),
            file_types: rows("file_types")?,
            browsers: rows("browsers")?,
            oses: rows("oses")?,
            robots: rows("robots")?,
            request_times: rows("request_times")?,
            virtual_hosts: rows("virtual_hosts")?,
            entry_pages: rows("entry_pages")?,
            exit_pages: rows("exit_pages")?,
            search_engines: rows("search_engines")?,
            keyphrases: rows("keyphrases")?,
            hours: rows("hours")?,
            dates: rows("dates")?,
            months: rows("months")?,
            weekdays: rows("weekdays")?,
            diagnostics: self
                .store
                .diagnostics(limit)?
                .into_iter()
                .map(|(key, hits)| ReportRow { key, hits })
                .collect(),
        })
    }
}

impl Report {
    pub fn apply_url_aliases(&mut self, aliases: &HashMap<String, String>) {
        for row in self
            .urls
            .iter_mut()
            .chain(self.entry_pages.iter_mut())
            .chain(self.exit_pages.iter_mut())
        {
            if let Some(alias) = aliases.get(&row.key) {
                row.key.clone_from(alias);
            }
        }
        for row in &mut self.bandwidth_urls {
            if let Some(alias) = aliases.get(&row.key) {
                row.key.clone_from(alias);
            }
        }
    }

    pub fn apply_cluster_info(&mut self, info: &HashMap<String, String>) {
        for row in &mut self.virtual_hosts {
            if let Some(label) = info.get(&row.key) {
                row.key = format!("{} ({label})", row.key);
            }
        }
    }
}
