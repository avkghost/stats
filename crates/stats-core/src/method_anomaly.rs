use crate::AccessEvent;
use std::collections::HashSet;

const DEFAULT_ALLOWED_METHODS: &[&str] = &[
    "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "CONNECT", "TRACE",
];

/// Classifies methods outside the configured HTTP method allow-list.
#[derive(Debug, Clone)]
pub struct MethodAnomalyMatcher {
    allowed: HashSet<String>,
}

impl MethodAnomalyMatcher {
    pub fn new(allowed: impl IntoIterator<Item = String>) -> Self {
        let mut methods = allowed
            .into_iter()
            .map(|method| method.to_ascii_uppercase())
            .filter(|method| !method.is_empty())
            .collect::<HashSet<_>>();
        if methods.is_empty() {
            methods.extend(
                DEFAULT_ALLOWED_METHODS
                    .iter()
                    .map(|method| (*method).into()),
            );
        }
        Self { allowed: methods }
    }

    pub fn apply(&self, event: &mut AccessEvent) {
        let Some(method) = event.method.as_deref() else {
            return;
        };
        let normalized = method.trim().to_ascii_uppercase();
        if !normalized.is_empty() && !self.allowed.contains(&normalized) {
            event.method_anomaly = Some(normalized);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MethodAnomalyMatcher;
    use crate::{AccessEvent, GeoInfo};
    use chrono::{DateTime, FixedOffset};

    fn event(method: Option<&str>) -> AccessEvent {
        AccessEvent {
            client_ip: "192.0.2.1".into(),
            client_hostname: None,
            referer_spam: None,
            referer_excluded: false,
            scanner: None,
            method_anomaly: None,
            identity: None,
            user: None,
            timestamp: DateTime::<FixedOffset>::parse_from_rfc3339("1970-01-01T00:00:00+00:00")
                .unwrap(),
            request: String::new(),
            method: method.map(str::to_owned),
            url: Some("/".into()),
            status: 200,
            bytes: 0,
            referer: None,
            user_agent: None,
            duration_seconds: None,
            virtual_host: None,
            geo: GeoInfo::default(),
        }
    }

    #[test]
    fn classifies_methods_outside_the_allow_list() {
        let matcher = MethodAnomalyMatcher::new(["GET".into(), "POST".into()]);
        let mut normal = event(Some("get"));
        matcher.apply(&mut normal);
        assert!(normal.method_anomaly.is_none());

        let mut unusual = event(Some("PROPFIND"));
        matcher.apply(&mut unusual);
        assert_eq!(unusual.method_anomaly.as_deref(), Some("PROPFIND"));
    }
}
