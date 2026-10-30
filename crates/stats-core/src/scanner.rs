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
}
