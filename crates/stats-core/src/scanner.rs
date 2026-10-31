use crate::AccessEvent;
use std::{fs, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ScannerRuleError {
    #[error("read scanner rules {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid scanner rule at {path}:{line}: {message}")]
    Invalid {
        path: String,
        line: usize,
        message: String,
    },
}

#[derive(Debug, Clone, Copy)]
enum MatchMode {
    Exact,
    Prefix,
    Suffix,
}

#[derive(Debug, Clone)]
struct Rule {
    mode: MatchMode,
    pattern: String,
    label: String,
}

#[derive(Debug, Default, Clone)]
pub struct ScannerMatcher {
    rules: Vec<Rule>,
}

impl ScannerMatcher {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ScannerRuleError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| ScannerRuleError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&text, &path.display().to_string())
    }

    pub fn parse(text: &str, path: &str) -> Result<Self, ScannerRuleError> {
        let mut rules = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields = line.splitn(3, '\t').collect::<Vec<_>>();
            if fields.len() != 3 {
                return Err(ScannerRuleError::Invalid {
                    path: path.into(),
                    line: index + 1,
                    message: "expected mode<TAB>path<TAB>label".into(),
                });
            }
            let mode = match fields[0] {
                "path_exact" => MatchMode::Exact,
                "path_prefix" => MatchMode::Prefix,
                "path_suffix" => MatchMode::Suffix,
                mode => {
                    return Err(ScannerRuleError::Invalid {
                        path: path.into(),
                        line: index + 1,
                        message: format!("unsupported mode {mode:?}"),
                    })
                }
            };
            let pattern = fields[1].trim().to_owned();
            let label = fields[2].trim().to_owned();
            if pattern.is_empty() || label.is_empty() {
                return Err(ScannerRuleError::Invalid {
                    path: path.into(),
                    line: index + 1,
                    message: "path and label must not be empty".into(),
                });
            }
            rules.push(Rule {
                mode,
                pattern,
                label,
            });
        }
        Ok(Self { rules })
    }

    pub fn apply(&self, event: &mut AccessEvent) {
        let Some(url) = event.url.as_deref() else {
            return;
        };
        if let Some(rule) = self.rules.iter().find(|rule| rule.matches(url)) {
            event.scanner = Some(rule.label.clone());
        }
    }

    pub fn apply_heuristic(&self, event: &mut AccessEvent) {
        if event.scanner.is_some() || event.status != 404 {
            return;
        }
        let Some(url) = event.url.as_deref() else {
            return;
        };
        let path = url.split('?').next().unwrap_or(url).to_ascii_lowercase();
        let label = if path == "/.env" || path.starts_with("/.env.") || path.contains("/.env/") {
            Some("environment-probe")
        } else if path == "/.git" || path.starts_with("/.git/") {
            Some("git-probe")
        } else if path == "/.aws" || path.starts_with("/.aws/") {
            Some("cloud-credentials-probe")
        } else if path.starts_with("/wp-")
            || path.starts_with("/wordpress")
            || path.starts_with("/wp/")
        {
            Some("wordpress-probe")
        } else if path.contains("phpmyadmin")
            || path == "/phpinfo.php"
            || path.contains("/vendor/phpunit/")
        {
            Some("runtime-probe")
        } else if path.starts_with("/actuator/")
            || path.starts_with("/server-status")
            || path.starts_with("/cgi-bin/")
        {
            Some("service-probe")
        } else {
            None
        };
        if let Some(label) = label {
            event.scanner = Some(label.into());
        }
    }
}

impl Rule {
    fn matches(&self, url: &str) -> bool {
        match self.mode {
            MatchMode::Exact => url == self.pattern,
            MatchMode::Prefix => url.starts_with(&self.pattern),
            MatchMode::Suffix => url.ends_with(&self.pattern),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ScannerMatcher;
    use crate::{AccessEvent, GeoInfo};
    use chrono::{DateTime, FixedOffset};

    fn event(url: &str) -> AccessEvent {
        AccessEvent {
            client_ip: "203.0.113.10".into(),
            client_hostname: None,
            referer_spam: None,
            referer_excluded: false,
            scanner: None,
            method_anomaly: None,
            identity: None,
            user: None,
            timestamp: DateTime::<FixedOffset>::parse_from_rfc3339("2026-01-01T00:00:00+00:00")
                .unwrap(),
            request: format!("GET {url} HTTP/1.1"),
            method: Some("GET".into()),
            url: Some(url.into()),
            status: 404,
            bytes: 1,
            referer: None,
            user_agent: None,
            duration_seconds: None,
            virtual_host: None,
            geo: GeoInfo::default(),
        }
    }

    #[test]
    fn first_matching_path_rule_wins() {
        let matcher = ScannerMatcher::parse(
            "path_prefix\t/.env\tenvironment-probe\npath_suffix\t/.git/config\tgit-probe\n",
            "rules",
        )
        .unwrap();
        let mut first_event = event("/.env.production");
        matcher.apply(&mut first_event);
        assert_eq!(first_event.scanner.as_deref(), Some("environment-probe"));

        let mut second_event = event("/.git/config");
        matcher.apply(&mut second_event);
        assert_eq!(second_event.scanner.as_deref(), Some("git-probe"));
    }

    #[test]
    fn leaves_unmatched_paths_unclassified() {
        let matcher = ScannerMatcher::parse("path_exact\t/\troot-probe", "rules").unwrap();
        let mut event = event("/about/");
        matcher.apply(&mut event);
        assert!(event.scanner.is_none());
    }

    #[test]
    fn classifies_high_confidence_404_probe_paths_only() {
        let matcher = ScannerMatcher::default();
        let mut probe = event("/.env.production");
        matcher.apply_heuristic(&mut probe);
        assert_eq!(probe.scanner.as_deref(), Some("environment-probe"));

        let mut asset = event("/favicon.ico");
        matcher.apply_heuristic(&mut asset);
        assert!(asset.scanner.is_none());
    }

    #[test]
    fn explicit_rule_label_wins_over_heuristic_label() {
        let matcher =
            ScannerMatcher::parse("path_prefix\t/.env\tcustom-environment", "rules").unwrap();
        let mut probe = event("/.env.production");
        matcher.apply(&mut probe);
        matcher.apply_heuristic(&mut probe);
        assert_eq!(probe.scanner.as_deref(), Some("custom-environment"));
    }
}
