use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read config {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config line {line}: {message}")]
    Invalid { line: usize, message: String },
}

#[derive(Debug, Clone)]
pub struct PluginConfig {
    pub name: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FilterConfig {
    pub allowed_virtual_hosts: Vec<String>,
    pub skip_hosts: Vec<String>,
    pub only_hosts: Vec<String>,
    pub skip_files: Vec<String>,
    pub only_files: Vec<String>,
    pub skip_user_agents: Vec<String>,
    pub only_user_agents: Vec<String>,
    pub not_page_files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabasePeriod {
    None,
    Month,
    Year,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            allowed_virtual_hosts: Vec::new(),
            skip_hosts: Vec::new(),
            only_hosts: Vec::new(),
            skip_files: Vec::new(),
            only_files: Vec::new(),
            skip_user_agents: Vec::new(),
            only_user_agents: Vec::new(),
            not_page_files: [
                ".avif", ".css", ".gif", ".gz", ".ico", ".jpeg", ".jpg", ".js", ".json", ".map",
                ".mp3", ".mp4", ".pdf", ".png", ".svg", ".ttf", ".txt", ".webm", ".webp", ".woff",
                ".woff2", ".xml", ".zip",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub log_file: PathBuf,
    pub log_format: String,
    pub site_domain: String,
    pub host_aliases: Vec<String>,
    pub data_dir: PathBuf,
    pub database_period: DatabasePeriod,
    pub time_zone: Option<i32>,
    pub dns_lookup: bool,
    pub plugins: Vec<PluginConfig>,
    pub filters: FilterConfig,
    pub url_referrer_with_query: bool,
    pub warnings: Vec<String>,
}

impl Config {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref().to_path_buf();
        let text =
            fs::read_to_string(&path).map_err(|source| ConfigError::Read { path, source })?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let mut values = HashMap::<String, String>::new();
        let mut plugins = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(ConfigError::Invalid {
                    line: line_no,
                    message: "expected Key=Value".into(),
                });
            };
            let key = key.trim();
            let value = value.trim().trim_matches('"').to_string();
            match key {
                "LoadPlugin" => {
                    let mut parts = value.split_whitespace();
                    let name = parts.next().unwrap_or_default().to_string();
                    if !name.is_empty() {
                        plugins.push(PluginConfig {
                            name,
                            args: parts.map(str::to_owned).collect(),
                        });
                    }
                }
                _ => {
                    values.insert(key.to_string(), value);
                }
            }
        }
        let required = |key: &str| {
            values
                .get(key)
                .cloned()
                .ok_or_else(|| ConfigError::Invalid {
                    line: 0,
                    message: format!("missing {key}"),
                })
        };
        let list = |key: &str| {
            values
                .get(key)
                .map(|v| v.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default()
        };
        let log_format = required("LogFormat")?;
        let known = [
            "LogFile",
            "LogFormat",
            "SiteDomain",
            "HostAliases",
            "DirData",
            "DatabasePeriod",
            "TimeZone",
            "DNSLookup",
            "SkipHosts",
            "OnlyHosts",
            "SkipFiles",
            "OnlyFiles",
            "SkipUserAgents",
            "OnlyUserAgents",
            "NotPageList",
            "URLReferrerWithQuery",
            "FirstDayOfWeek",
            // AWStats presentation-only directives accepted as no-ops.
            "UseFramesWhenCGI",
            "MenuWidth",
        ];
        let warnings = values
            .keys()
            .filter(|key| {
                !known.contains(&key.as_str())
                    && !key.starts_with("Show")
                    && !key.starts_with("MinHit")
            })
            .map(|key| format!("unsupported directive ignored: {key}"))
            .collect();
        let site_domain = values.get("SiteDomain").cloned().unwrap_or_default();
        let host_aliases = values
            .get("HostAliases")
            .map(|v| v.split_whitespace().map(str::to_owned).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut allowed_virtual_hosts = host_aliases.clone();
        if !site_domain.is_empty() {
            allowed_virtual_hosts.push(site_domain.clone());
        }
        Ok(Self {
            log_file: PathBuf::from(required("LogFile")?),
            log_format,
            site_domain,
            host_aliases,
            data_dir: PathBuf::from(
                values
                    .get("DirData")
                    .cloned()
                    .unwrap_or_else(|| "./data".into()),
            ),
            database_period: match values
                .get("DatabasePeriod")
                .map(|value| value.to_ascii_lowercase())
                .as_deref()
            {
                None | Some("none") | Some("single") => DatabasePeriod::None,
                Some("month") | Some("monthly") => DatabasePeriod::Month,
                Some("year") | Some("yearly") => DatabasePeriod::Year,
                Some(value) => {
                    return Err(ConfigError::Invalid {
                        line: 0,
                        message: format!(
                            "invalid DatabasePeriod {value}; expected none, monthly, or yearly"
                        ),
                    })
                }
            },
            time_zone: values
                .get("TimeZone")
                .map(|value| {
                    let offset = value.parse::<i32>().map_err(|_| ConfigError::Invalid {
                        line: 0,
                        message: format!(
                            "invalid TimeZone {value}; expected an integer hour offset"
                        ),
                    })?;
                    if !(-24..=24).contains(&offset) {
                        return Err(ConfigError::Invalid {
                            line: 0,
                            message: format!(
                                "invalid TimeZone {value}; expected an offset from -24 to 24"
                            ),
                        });
                    }
                    Ok(offset)
                })
                .transpose()?,
            dns_lookup: values.get("DNSLookup").map(|v| v != "0").unwrap_or(false),
            plugins,
            filters: FilterConfig {
                allowed_virtual_hosts,
                skip_hosts: list("SkipHosts"),
                only_hosts: list("OnlyHosts"),
                skip_files: list("SkipFiles"),
                only_files: list("OnlyFiles"),
                skip_user_agents: list("SkipUserAgents"),
                only_user_agents: list("OnlyUserAgents"),
                not_page_files: values
                    .get("NotPageList")
                    .map(|_| list("NotPageList"))
                    .unwrap_or_else(|| FilterConfig::default().not_page_files),
            },
            url_referrer_with_query: values
                .get("URLReferrerWithQuery")
                .map(|value| value != "0")
                .unwrap_or(false),
            warnings,
        })
    }

    pub fn with_overrides(mut self, log_file: Option<PathBuf>, data_dir: Option<PathBuf>) -> Self {
        if let Some(path) = log_file {
            self.log_file = path;
        }
        if let Some(path) = data_dir {
            self.data_dir = path;
        }
        self
    }

    pub fn url_aliases(&self) -> Result<HashMap<String, String>, ConfigError> {
        if !self.plugins.iter().any(|plugin| plugin.name == "urlalias") {
            return Ok(HashMap::new());
        }
        let site_file = (!self.site_domain.is_empty()).then(|| {
            self.data_dir
                .join(format!("urlalias.{}.txt", self.site_domain))
        });
        let fallback = self.data_dir.join("urlalias.txt");
        let path = site_file.filter(|path| path.is_file()).unwrap_or(fallback);
        let text = fs::read_to_string(&path).map_err(|source| ConfigError::Read {
            path: path.clone(),
            source,
        })?;
        let mut aliases = HashMap::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((url, alias)) = line.split_once('\t') else {
                return Err(ConfigError::Invalid {
                    line: index + 1,
                    message: format!("invalid URL alias entry in {}", path.display()),
                });
            };
            let url = url.trim();
            let alias = alias.trim();
            if url.is_empty() || alias.is_empty() {
                return Err(ConfigError::Invalid {
                    line: index + 1,
                    message: format!("invalid URL alias entry in {}", path.display()),
                });
            }
            aliases.insert(url.to_owned(), alias.to_owned());
        }
        Ok(aliases)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_awstats_custom_config_and_plugins() {
        let config = Config::parse("LogFile=access.log\nLogFormat=\"%host %time1\"\nSiteDomain=example.test\nLoadPlugin=\"geoip2_city /var/lib/GeoIP/GeoLite2-City.mmdb\"\n").unwrap();
        assert_eq!(config.log_file, PathBuf::from("access.log"));
        assert_eq!(config.site_domain, "example.test");
        assert_eq!(config.plugins[0].name, "geoip2_city");
        assert!(config.filters.not_page_files.iter().any(|v| v == ".css"));
    }

    #[test]
    fn accepts_awstats_presentation_directives_without_warnings() {
        let config = Config::parse(
            "LogFile=access.log\nLogFormat=\"%host %time1\"\nUseFramesWhenCGI=1\nMenuWidth=150\n",
        )
        .unwrap();
        assert!(config.warnings.is_empty());
    }

    #[test]
    fn parses_database_period() {
        let monthly = Config::parse(
            "LogFile=access.log\nLogFormat=\"%host %time1\"\nDatabasePeriod=monthly\n",
        )
        .unwrap();
        assert_eq!(monthly.database_period, DatabasePeriod::Month);

        let yearly = Config::parse(
            "LogFile=access.log\nLogFormat=\"%host %time1\"\nDatabasePeriod=yearly\n",
        )
        .unwrap();
        assert_eq!(yearly.database_period, DatabasePeriod::Year);
    }

    #[test]
    fn parses_optional_timezone_offset() {
        let config = Config::parse("LogFile=access.log\nLogFormat=1\nTimeZone=2\n").unwrap();
        assert_eq!(config.time_zone, Some(2));

        let error = Config::parse("LogFile=access.log\nLogFormat=1\nTimeZone=25\n");
        assert!(matches!(error, Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn loads_url_aliases_only_when_plugin_is_enabled() {
        let root = std::env::temp_dir().join(format!("stats-urlalias-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("urlalias.example.test.txt"),
            "/\tHome\n# ignored\n",
        )
        .unwrap();
        let mut config = Config::parse(
            "LogFile=access.log\nLogFormat=1\nSiteDomain=example.test\nLoadPlugin=urlalias\n",
        )
        .unwrap()
        .with_overrides(None, Some(root.clone()));
        assert_eq!(config.url_aliases().unwrap().get("/"), Some(&"Home".into()));
        config.plugins.clear();
        assert!(config.url_aliases().unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
