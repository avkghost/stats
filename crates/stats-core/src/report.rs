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
    pub method_anomalies: Vec<ReportRow>,
    pub urls: Vec<ReportRow>,
    pub bandwidth_urls: Vec<BandwidthRow>,
    pub daily: Vec<DailyRow>,
    pub users: Vec<ReportRow>,
    pub user_agents: Vec<ReportRow>,
    pub referers: Vec<ReportRow>,
    pub referrer_domains: Vec<ReportRow>,
    pub referer_spam: Vec<ReportRow>,
    pub scanners: Vec<ReportRow>,
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
            method_anomalies: self
                .store
                .top_if_exists("method_anomalies", limit)?
                .into_iter()
                .map(|(key, hits)| ReportRow { key, hits })
                .collect(),
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
            scanners: self
                .store
                .top_if_exists("scanners", limit)?
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
        apply_cluster_info_rows(&mut self.virtual_hosts, info);
    }
}

fn apply_cluster_info_rows(rows: &mut [ReportRow], info: &HashMap<String, String>) {
    for row in rows {
        if let Some(label) = info.get(&row.key) {
            row.key = format!("{} ({label})", row.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{apply_cluster_info_rows, BandwidthRow, Report, ReportMetadata, ReportRow};
    use std::collections::HashMap;

    #[test]
    fn cluster_info_decorates_matching_rows_only() {
        let mut rows = vec![
            ReportRow {
                key: "node-a".into(),
                hits: 3,
            },
            ReportRow {
                key: "node-b".into(),
                hits: 2,
            },
        ];
        let info = HashMap::from([(String::from("node-a"), String::from("Primary"))]);

        apply_cluster_info_rows(&mut rows, &info);

        assert_eq!(rows[0].key, "node-a (Primary)");
        assert_eq!(rows[1].key, "node-b");
        assert_eq!(rows[0].hits, 3);
    }

    #[test]
    fn url_aliases_decorate_all_display_url_dimensions() {
        let mut report = Report {
            metadata: ReportMetadata {
                schema_version: None,
                input_path: None,
                input_size: None,
                input_lines: None,
                config_path: None,
                config_hash: None,
                approximations: Vec::new(),
            },
            hits: 0,
            bytes: 0,
            pages: 0,
            files: 0,
            visits: 0,
            unique_visits: 0,
            countries: Vec::new(),
            regions: Vec::new(),
            cities: Vec::new(),
            asns: Vec::new(),
            organizations: Vec::new(),
            hostnames: Vec::new(),
            statuses: Vec::new(),
            status_classes: Vec::new(),
            errors: Vec::new(),
            methods: Vec::new(),
            method_anomalies: Vec::new(),
            urls: vec![ReportRow {
                key: "/docs".into(),
                hits: 4,
            }],
            bandwidth_urls: vec![BandwidthRow {
                key: "/docs".into(),
                hits: 4,
                bytes: 100,
            }],
            daily: Vec::new(),
            users: Vec::new(),
            user_agents: Vec::new(),
            referers: Vec::new(),
            referrer_domains: Vec::new(),
            referer_spam: Vec::new(),
            scanners: Vec::new(),
            file_types: Vec::new(),
            browsers: Vec::new(),
            oses: Vec::new(),
            robots: Vec::new(),
            request_times: Vec::new(),
            virtual_hosts: Vec::new(),
            entry_pages: vec![ReportRow {
                key: "/docs".into(),
                hits: 2,
            }],
            exit_pages: vec![ReportRow {
                key: "/docs".into(),
                hits: 1,
            }],
            search_engines: Vec::new(),
            keyphrases: Vec::new(),
            hours: Vec::new(),
            dates: Vec::new(),
            months: Vec::new(),
            weekdays: Vec::new(),
            diagnostics: Vec::new(),
        };
        report.apply_url_aliases(&HashMap::from([(
            String::from("/docs"),
            String::from("Docs"),
        )]));

        assert_eq!(report.urls[0].key, "Docs");
        assert_eq!(report.bandwidth_urls[0].key, "Docs");
        assert_eq!(report.entry_pages[0].key, "Docs");
        assert_eq!(report.exit_pages[0].key, "Docs");
    }
}
