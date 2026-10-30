use crate::{store::referrer_domain, AccessEvent};
use std::{fs, path::Path};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RefererSpamRuleError {
    #[error("read referer-spam rules {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid referer-spam rule at {path}:{line}: {message}")]
    Invalid {
        path: String,
        line: usize,
        message: String,
    },
}

#[derive(Debug, Clone, Copy)]
enum MatchMode {
    Exact,
    Suffix,
}

#[derive(Debug, Clone)]
enum Action {
    Exclude,
    Classify(String),
}

#[derive(Debug, Clone)]
struct Rule {
    mode: MatchMode,
    pattern: String,
    action: Action,
}

#[derive(Debug, Default, Clone)]
pub struct RefererSpamMatcher {
    rules: Vec<Rule>,
}

impl RefererSpamMatcher {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, RefererSpamRuleError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|source| RefererSpamRuleError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse(&text, &path.display().to_string())
    }

    pub fn parse(text: &str, path: &str) -> Result<Self, RefererSpamRuleError> {
        let mut rules = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields = line.splitn(3, '\t').collect::<Vec<_>>();
            if fields.len() != 3 {
                return Err(RefererSpamRuleError::Invalid {
                    path: path.into(),
                    line: index + 1,
                    message: "expected action<TAB>pattern<TAB>label".into(),
                });
            }
            let (action_name, pattern, label) = (fields[0], fields[1], fields[2].trim());
            let (action, mode) = match action_name {
                "exclude_exact" => (Action::Exclude, MatchMode::Exact),
                "exclude_suffix" => (Action::Exclude, MatchMode::Suffix),
                "classify_exact" => (Action::Classify(label.to_owned()), MatchMode::Exact),
                "classify_suffix" => (Action::Classify(label.to_owned()), MatchMode::Suffix),
                _ => {
                    return Err(RefererSpamRuleError::Invalid {
                        path: path.into(),
                        line: index + 1,
                        message: format!("unsupported action {action_name:?}"),
                    })
                }
            };
            let pattern = pattern
                .trim()
                .trim_start_matches('.')
                .trim_end_matches('.')
                .to_ascii_lowercase();
            if pattern.is_empty() || label.is_empty() {
                return Err(RefererSpamRuleError::Invalid {
                    path: path.into(),
                    line: index + 1,
                    message: "pattern and label must not be empty".into(),
                });
            }
            rules.push(Rule {
                mode,
                pattern,
                action,
            });
        }
        Ok(Self { rules })
    }

    pub fn apply(&self, event: &mut AccessEvent) {
        let Some(host) = referrer_domain(event.referer.as_deref()) else {
            return;
        };
        let Some(rule) = self.rules.iter().find(|rule| rule.matches(&host)) else {
            return;
        };
        match &rule.action {
            Action::Exclude => event.referer_excluded = true,
            Action::Classify(label) => event.referer_spam = Some(label.clone()),
        }
    }
}

impl Rule {
    fn matches(&self, host: &str) -> bool {
        match self.mode {
            MatchMode::Exact => host == self.pattern,
            MatchMode::Suffix => {
                host == self.pattern || host.ends_with(&format!(".{}", self.pattern))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RefererSpamMatcher;
    use crate::{AccessEvent, GeoInfo};
    use chrono::{DateTime, FixedOffset};

    fn event(referer: &str) -> AccessEvent {
        AccessEvent {
            client_ip: "203.0.113.10".into(),
            client_hostname: None,
            identity: None,
            user: None,
            timestamp: DateTime::<FixedOffset>::parse_from_rfc3339("2026-01-01T00:00:00+00:00")
                .unwrap(),
            request: "GET / HTTP/1.1".into(),
            method: Some("GET".into()),
            url: Some("/".into()),
            status: 200,
            bytes: 1,
            referer: Some(referer.into()),
            user_agent: None,
            duration_seconds: None,
            virtual_host: None,
            referer_spam: None,
            referer_excluded: false,
            geo: GeoInfo::default(),
        }
    }

    #[test]
    fn first_matching_rule_wins_and_normalizes_host() {
        let matcher = RefererSpamMatcher::parse(
            "classify_suffix\t.example.test\tspam\nexclude_exact\tbad.example.test\tignored\n",
            "rules",
        )
        .unwrap();
        let mut event = event("https://BAD.example.test/path?q=1");
        matcher.apply(&mut event);
        assert_eq!(event.referer_spam.as_deref(), Some("spam"));
        assert!(!event.referer_excluded);
    }

    #[test]
    fn exclusion_marks_only_referer_dimensions() {
        let matcher =
            RefererSpamMatcher::parse("exclude_suffix\tspam.test\tignored", "rules").unwrap();
        let mut event = event("https://sub.spam.test/");
        matcher.apply(&mut event);
        assert!(event.referer_excluded);
        assert!(event.referer_spam.is_none());
    }
}
