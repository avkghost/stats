use crate::{AccessEvent, FilterConfig};
use regex::Regex;

pub fn accepts(event: &AccessEvent, filters: &FilterConfig) -> bool {
    let host = &event.client_ip;
    let url = event.url.as_deref().unwrap_or("UNKNOWN");
    let user_agent = event.user_agent.as_deref().unwrap_or("UNKNOWN");
    if let Some(virtual_host) = event.virtual_host.as_deref() {
        if !filters.allowed_virtual_hosts.is_empty()
            && !matches_any(virtual_host, &filters.allowed_virtual_hosts)
        {
            return false;
        }
    }
    if matches_any(host, &filters.skip_hosts)
        || matches_any(url, &filters.skip_files)
        || matches_any(user_agent, &filters.skip_user_agents)
    {
        return false;
    }
    if !filters.only_hosts.is_empty() && !matches_any(host, &filters.only_hosts) {
        return false;
    }
    if !filters.only_files.is_empty() && !matches_any(url, &filters.only_files) {
        return false;
    }
    if !filters.only_user_agents.is_empty() && !matches_any(user_agent, &filters.only_user_agents) {
        return false;
    }
    true
}

pub fn is_page(event: &AccessEvent, filters: &FilterConfig) -> bool {
    !matches_any(
        event.url.as_deref().unwrap_or("UNKNOWN"),
        &filters.not_page_files,
    )
}

fn matches_any(value: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| {
        if let Some(regex) = pattern
            .strip_prefix("REGEX[")
            .and_then(|v| v.strip_suffix(']'))
        {
            Regex::new(regex)
                .map(|re| re.is_match(value))
                .unwrap_or(false)
        } else {
            value.contains(pattern)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;

    fn event(url: &str, agent: &str) -> AccessEvent {
        AccessEvent {
            client_ip: "192.0.2.1".into(),
            identity: None,
            user: None,
            timestamp: DateTime::parse_from_rfc3339("2026-10-28T00:00:00+00:00").unwrap(),
            request: format!("GET {url} HTTP/1.1"),
            method: Some("GET".into()),
            url: Some(url.into()),
            status: 200,
            bytes: 1,
            referer: None,
            user_agent: Some(agent.into()),
            duration_seconds: None,
            virtual_host: None,
            geo: Default::default(),
        }
    }

    #[test]
    fn applies_skip_and_only_filters() {
        let filters = FilterConfig {
            skip_files: vec!["/private".into()],
            only_user_agents: vec!["Mozilla".into()],
            ..Default::default()
        };
        assert!(!accepts(&event("/private", "Mozilla"), &filters));
        assert!(!accepts(&event("/public", "curl"), &filters));
        assert!(accepts(&event("/public", "Mozilla"), &filters));
    }

    #[test]
    fn filters_virtual_hosts_when_configured() {
        let mut allowed = event("/public", "Mozilla");
        allowed.virtual_host = Some("www.example.test".into());
        let filters = FilterConfig {
            allowed_virtual_hosts: vec!["example.test".into(), "www.example.test".into()],
            ..Default::default()
        };
        assert!(accepts(&allowed, &filters));
        allowed.virtual_host = Some("other.test".into());
        assert!(!accepts(&allowed, &filters));
    }
}
