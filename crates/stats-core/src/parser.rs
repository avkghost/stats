use chrono::{DateTime, Datelike, FixedOffset, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use regex::Regex;
use thiserror::Error;

const MAX_LINE_LENGTH: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct AccessEvent {
    pub client_ip: String,
    pub client_hostname: Option<String>,
    pub referer_spam: Option<String>,
    pub referer_excluded: bool,
    pub scanner: Option<String>,
    pub method_anomaly: Option<String>,
    pub identity: Option<String>,
    pub user: Option<String>,
    pub timestamp: DateTime<FixedOffset>,
    pub request: String,
    pub method: Option<String>,
    pub url: Option<String>,
    pub status: u16,
    pub bytes: u64,
    pub referer: Option<String>,
    pub user_agent: Option<String>,
    pub duration_seconds: Option<f64>,
    pub virtual_host: Option<String>,
    pub geo: crate::GeoInfo,
}

impl AccessEvent {
    pub fn apply_timezone_offset(&mut self, hours: i32) -> bool {
        let Some(offset) = FixedOffset::east_opt(hours.saturating_mul(60 * 60)) else {
            return false;
        };
        self.timestamp = self.timestamp.with_timezone(&offset);
        true
    }

    pub fn apply_timezone(&mut self, value: &str) -> bool {
        if let Ok(hours) = value.parse::<i32>() {
            return self.apply_timezone_offset(hours);
        }
        let Ok(time_zone) = value.parse::<Tz>() else {
            return false;
        };
        self.timestamp = self.timestamp.with_timezone(&time_zone).fixed_offset();
        true
    }
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("line exceeds maximum supported length of {MAX_LINE_LENGTH} bytes")]
    LineTooLong,
    #[error("line does not match configured format")]
    Format,
    #[error("invalid timestamp: {0}")]
    Timestamp(String),
    #[error("invalid numeric field: {0}")]
    Number(String),
    #[error("invalid JSON log record: {0}")]
    Json(String),
}

#[derive(Debug)]
pub enum ParseOutcome {
    Event(Box<AccessEvent>),
    Diagnostic(ParseError),
}

#[derive(Debug, Clone)]
pub struct LogParser {
    regex: Regex,
    time_format: TimeFormat,
    json: bool,
}

#[derive(Debug, Clone, Copy)]
enum TimeFormat {
    Time1,
    Time1b,
    Time2,
    Time3,
    Time4,
    Time5,
    Time6,
    Time7,
    Time8,
}

const TIME_FORMAT_RULES: &[(&str, TimeFormat)] = &[
    ("%time1b", TimeFormat::Time1b),
    ("%time1", TimeFormat::Time1),
    ("%time2", TimeFormat::Time2),
    ("%time3", TimeFormat::Time3),
    ("%time4", TimeFormat::Time4),
    ("%time5", TimeFormat::Time5),
    ("%time6", TimeFormat::Time6),
    ("%time7", TimeFormat::Time7),
    ("%time8", TimeFormat::Time8),
];

fn detect_time_format(format: &str) -> TimeFormat {
    if format == "1" {
        return TimeFormat::Time1;
    }
    TIME_FORMAT_RULES
        .iter()
        .find_map(|(token, time_format)| {
            contains_format_token(format, token).then_some(*time_format)
        })
        .unwrap_or(TimeFormat::Time1)
}

fn contains_format_token(format: &str, token: &str) -> bool {
    format.match_indices(token).any(|(index, matched)| {
        format[index + matched.len()..]
            .chars()
            .next()
            .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_')
    })
}

impl LogParser {
    pub fn new(format: impl Into<String>) -> Result<Self, regex::Error> {
        let format = format.into();
        let format = match format.trim() {
            "2" => {
                "%time2 %host %logname %method %url %code %bytesd %other %ua %referer".to_owned()
            }
            _ => normalize_apache_format(&format),
        };
        let regex = compile_format(&format)?;
        let json = format.trim().eq_ignore_ascii_case("json");
        let time_format = detect_time_format(&format);
        Ok(Self {
            regex,
            time_format,
            json,
        })
    }

    pub fn parse(&self, line: &str) -> ParseOutcome {
        if line.len() > MAX_LINE_LENGTH {
            return ParseOutcome::Diagnostic(ParseError::LineTooLong);
        }
        if self.json {
            return self.parse_json(line);
        }
        let Some(caps) = self.regex.captures(line) else {
            return ParseOutcome::Diagnostic(ParseError::Format);
        };
        let get = |name: &str| caps.name(name).map(|m| m.as_str().to_string());
        let timestamp =
            match get("time").and_then(|value| parse_timestamp(&value, self.time_format)) {
                Some(v) => v,
                None => {
                    return ParseOutcome::Diagnostic(ParseError::Timestamp(
                        get("time").unwrap_or_default(),
                    ))
                }
            };
        let status = match get("status").and_then(|v| v.parse().ok()) {
            Some(v) => v,
            None => return ParseOutcome::Diagnostic(ParseError::Number("status".into())),
        };
        let bytes = match get("bytes").as_deref() {
            Some("-") => 0,
            Some(value) => match value.parse() {
                Ok(value) => value,
                Err(_) => return ParseOutcome::Diagnostic(ParseError::Number("bytes".into())),
            },
            None => return ParseOutcome::Diagnostic(ParseError::Number("bytes".into())),
        };
        let query = none_dash(get("query"));
        let request = get("request").unwrap_or_else(|| {
            let method = get("method").unwrap_or_default();
            let mut url = get("url").unwrap_or_default();
            if let Some(query) = &query {
                url.push_str(query);
            }
            let protocol = get("protocol").unwrap_or_default();
            format_request(&method, &url, &protocol)
        });
        let (request_method, request_url) = split_request(&request);
        let method = get("method").or(request_method);
        let url = get("url")
            .map(|value| decode_unreserved_url(&format!("{value}{}", query.unwrap_or_default())))
            .or(request_url);
        let duration_seconds = get("extra1")
            .and_then(|v| v.parse().ok())
            .or_else(|| get("duration").and_then(|v| v.parse().ok()))
            .or_else(|| {
                get("duration_ms")
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|v| v / 1_000.0)
            })
            .or_else(|| {
                get("duration_us")
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|v| v / 1_000_000.0)
            });
        ParseOutcome::Event(Box::new(AccessEvent {
            client_ip: get("host").unwrap_or_default(),
            client_hostname: None,
            referer_spam: None,
            referer_excluded: false,
            scanner: None,
            method_anomaly: None,
            identity: none_dash(get("logname")),
            user: none_dash(get("user")),
            timestamp,
            request,
            method,
            url,
            status,
            bytes,
            referer: none_dash(get("referer")),
            user_agent: none_dash(get("ua")),
            duration_seconds,
            virtual_host: none_dash(get("virtual_host")),
            geo: crate::GeoInfo::default(),
        }))
    }

    fn parse_json(&self, line: &str) -> ParseOutcome {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => return ParseOutcome::Diagnostic(ParseError::Json(error.to_string())),
        };
        let get = |keys: &[&str]| keys.iter().find_map(|key| value.get(*key)).cloned();
        let string = |keys: &[&str]| get(keys).and_then(|value| value.as_str().map(str::to_owned));
        let timestamp_value = get(&["time", "timestamp", "@timestamp"]);
        let timestamp = timestamp_value
            .as_ref()
            .and_then(json_timestamp)
            .or_else(|| {
                timestamp_value
                    .as_ref()
                    .and_then(|value| value.as_i64())
                    .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
                    .map(|timestamp| timestamp.fixed_offset())
            });
        let timestamp = match timestamp {
            Some(value) => value,
            None => {
                return ParseOutcome::Diagnostic(ParseError::Timestamp(
                    timestamp_value
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
                ))
            }
        };
        let request = string(&["request", "request_line"]);
        let method = string(&["method", "http_method"]);
        let url = string(&["url", "path", "request_uri"]);
        let request = request
            .or_else(|| match (&method, &url) {
                (Some(method), Some(url)) => Some(format!("{method} {url}")),
                _ => None,
            })
            .unwrap_or_default();
        let (request_method, request_url) = split_request(&request);
        let status = get(&["status", "status_code", "response_status"])
            .and_then(|value| value.as_u64())
            .map(|value| value as u16);
        let bytes =
            get(&["bytes", "body_bytes", "response_bytes"]).and_then(|value| value.as_u64());
        let (status, bytes) = match (status, bytes) {
            (Some(status), Some(bytes)) => (status, bytes),
            (None, _) => return ParseOutcome::Diagnostic(ParseError::Number("status".into())),
            (_, None) => return ParseOutcome::Diagnostic(ParseError::Number("bytes".into())),
        };
        ParseOutcome::Event(Box::new(AccessEvent {
            client_ip: string(&["host", "client_ip", "remote_addr", "ip"]).unwrap_or_default(),
            client_hostname: None,
            referer_spam: None,
            referer_excluded: false,
            scanner: None,
            method_anomaly: None,
            identity: none_dash(string(&["logname", "identity"])),
            user: none_dash(string(&["user", "authenticated_user"])),
            timestamp,
            request,
            method: method.or(request_method),
            url: url.or(request_url),
            status,
            bytes,
            referer: none_dash(string(&["referer", "http_referer"])),
            user_agent: none_dash(string(&["ua", "user_agent", "http_user_agent"])),
            duration_seconds: get(&["duration", "request_time", "request_time_seconds"])
                .and_then(|value| value.as_f64()),
            virtual_host: none_dash(string(&["virtual_host", "server_name", "vhost"])),
            geo: crate::GeoInfo::default(),
        }))
    }
}

fn json_timestamp(value: &serde_json::Value) -> Option<DateTime<FixedOffset>> {
    let value = value.as_str()?;
    DateTime::parse_from_rfc3339(value)
        .ok()
        .or_else(|| parse_timestamp(value, TimeFormat::Time1))
        .or_else(|| naive_fixed(value, "%Y-%m-%d %H:%M:%S"))
}

fn parse_timestamp(value: &str, format: TimeFormat) -> Option<DateTime<FixedOffset>> {
    match format {
        TimeFormat::Time1 => DateTime::parse_from_str(
            value.trim().trim_matches(['[', ']']),
            "%d/%b/%Y:%H:%M:%S %z",
        )
        .ok(),
        TimeFormat::Time1b => {
            naive_fixed(value.trim().trim_matches(['[', ']']), "%d/%b/%Y:%H:%M:%S")
        }
        TimeFormat::Time2 => naive_fixed(value, "%Y-%m-%d %H:%M:%S"),
        TimeFormat::Time3 => {
            let year = Utc::now().year();
            naive_fixed(&format!("{year} {value}"), "%Y %b %d %H:%M:%S")
        }
        TimeFormat::Time4 => value
            .parse::<i64>()
            .ok()
            .and_then(|seconds| DateTime::<Utc>::from_timestamp(seconds, 0))
            .map(|timestamp| timestamp.fixed_offset()),
        TimeFormat::Time5 => DateTime::parse_from_rfc3339(value)
            .ok()
            .or_else(|| naive_fixed(value, "%Y-%m-%dT%H:%M:%S")),
        TimeFormat::Time6 => naive_fixed(value, "%d/%m/%Y, %H:%M:%S"),
        TimeFormat::Time7 => epoch_fractional(value, 1_000),
        TimeFormat::Time8 => epoch_fractional(value, 1_000_000),
    }
}

fn epoch_fractional(value: &str, units_per_second: i64) -> Option<DateTime<FixedOffset>> {
    let value = value.parse::<i64>().ok()?;
    let seconds = value.div_euclid(units_per_second);
    let remainder = value.rem_euclid(units_per_second);
    let nanos = remainder * (1_000_000_000 / units_per_second);
    DateTime::<Utc>::from_timestamp(seconds, nanos as u32).map(|timestamp| timestamp.fixed_offset())
}

fn naive_fixed(value: &str, format: &str) -> Option<DateTime<FixedOffset>> {
    let naive = NaiveDateTime::parse_from_str(value, format).ok()?;
    FixedOffset::east_opt(0)?
        .from_local_datetime(&naive)
        .single()
}

fn none_dash(value: Option<String>) -> Option<String> {
    value.filter(|v| v != "-")
}

fn format_request(method: &str, url: &str, protocol: &str) -> String {
    match (method.is_empty(), url.is_empty(), protocol.is_empty()) {
        (false, false, false) => format!("{method} {url} {protocol}"),
        (false, false, true) => format!("{method} {url}"),
        _ => String::new(),
    }
}

fn split_request(request: &str) -> (Option<String>, Option<String>) {
    let mut parts = request.split_whitespace();
    let method = parts.next().map(str::to_owned);
    let url = parts.next().map(decode_unreserved_url);
    if method.as_deref().is_some_and(|m| m.contains('\\')) {
        (Some("UNKNOWN".into()), Some(request.to_owned()))
    } else {
        (method, url)
    }
}

fn decode_unreserved_url(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = (bytes[index + 1] as char).to_digit(16);
            let low = (bytes[index + 2] as char).to_digit(16);
            if let (Some(high), Some(low)) = (high, low) {
                let byte = (high * 16 + low) as u8;
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                    decoded.push(byte as char);
                    index += 3;
                    continue;
                }
            }
        }
        let character = value[index..].chars().next().unwrap_or_default();
        decoded.push(character);
        index += character.len_utf8();
    }
    decoded
}

fn normalize_apache_format(format: &str) -> String {
    if format.eq_ignore_ascii_case("json") || !format.contains('%') {
        return format.to_owned();
    }
    let bytes = format.as_bytes();
    let mut normalized = String::with_capacity(format.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            let character = format[index..].chars().next().unwrap_or_default();
            normalized.push(character);
            index += character.len_utf8();
            continue;
        }

        let remaining = &format[index..];
        let long_token = match_apache_long_token(remaining);
        if let Some((apache, awstats)) = long_token {
            normalized.push_str(awstats);
            index += apache.len();
            continue;
        }
        if remaining.starts_with("%{") {
            if let Some(close) = remaining.find('}') {
                let token_length = close + 2;
                let suffix = remaining.as_bytes().get(close + 1).copied();
                if matches!(suffix, Some(b'i' | b'o' | b'e' | b't' | b'P' | b'C' | b'n')) {
                    let quoted = normalized.ends_with('"')
                        && format[index + token_length..].starts_with('"');
                    normalized.push_str(if quoted { "%otherquot" } else { "%other1" });
                    index += token_length;
                    continue;
                }
            }
        }

        let replacement = match bytes.get(index + 1).copied() {
            Some(b'v') => "%virtualname",
            Some(b'a') | Some(b'h') => "%host",
            Some(b'l') => "%logname",
            Some(b'u') => "%user",
            Some(b't') => "%time1bracket",
            Some(b'r') => "%methodurl",
            Some(b'm') => "%method",
            Some(b'U') => "%url",
            Some(b'q') => "%query",
            Some(b'H') => "%protocol",
            Some(b'D') => "%duration_us",
            Some(b'T') => "%duration",
            Some(b'O' | b'B' | b'b') => "%bytesd",
            Some(b'p') => "%other1",
            _ => "",
        };
        let is_boundary = bytes
            .get(index + 2)
            .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_');
        if !replacement.is_empty() && is_boundary {
            normalized.push_str(replacement);
            index += 2;
        } else {
            normalized.push('%');
            index += 1;
        }
    }
    normalized
}

const APACHE_LONG_TOKEN_RULES: &[(&[&str], &str)] = &[
    (
        &["%{User-Agent}i", "%{user-agent}i", "%{User-agent}i"],
        "%uaquot",
    ),
    (&["%{Referer}i", "%{referer}i"], "%refererquot"),
    (&["%{Host}i", "%{host}i"], "%virtualname"),
    (&["%{X-Forwarded-For}i", "%{x-forwarded-for}i"], "%host"),
    (&["%{c}a"], "%host"),
    (&["%{canonical}v"], "%virtualname"),
    (&["%{s}v"], "%virtualname"),
    (&["%{local}p"], "%other1"),
    (&["%{sec}t", "%{SEC}t"], "%time4"),
    (&["%{begin:sec}t", "%{end:sec}t"], "%time4"),
    (&["%{msec}t", "%{MSEC}t"], "%time7"),
    (&["%{begin:msec}t", "%{end:msec}t"], "%time7"),
    (&["%{usec}t", "%{USEC}t"], "%time8"),
    (&["%{begin:usec}t", "%{end:usec}t"], "%time8"),
    (&["%{ms}T", "%{MS}T"], "%duration_ms"),
    (&["%{us}T", "%{US}T"], "%duration_us"),
    (&["%{remote}p"], "%other2"),
    (&["%{status}o"], "%code"),
    (&["%>s"], "%code"),
];

fn match_apache_long_token(remaining: &str) -> Option<(&'static str, &'static str)> {
    APACHE_LONG_TOKEN_RULES
        .iter()
        .find_map(|(patterns, awstats)| {
            patterns
                .iter()
                .find(|pattern| remaining.starts_with(**pattern))
                .map(|pattern| (*pattern, *awstats))
        })
}

fn compile_format(format: &str) -> Result<Regex, regex::Error> {
    let predefined = match format {
        "1" => Some(
            r#"^(?P<host>\S+)\s+\S+\s+(?P<logname>\S+)\s+\[(?P<time>[^]]+)\]\s+\"(?P<request>.*?)\"\s+(?P<status>\d{3})\s+(?P<bytes>\d+)\s+\"(?P<referer>.*?)\"\s+\"(?P<ua>.*?)\"$"#,
        ),
        "4" => Some(
            r#"^(?P<host>\S+)\s+\S+\s+(?P<logname>\S+)\s+\[(?P<time>[^]]+)\]\s+\"(?P<request>.*?)\"\s+(?P<status>\d{3})\s+(?P<bytes>\d+)$"#,
        ),
        _ => None,
    };
    if let Some(pattern) = predefined {
        return Regex::new(pattern);
    }
    if format.contains("%host %logname %user %time1 %methodurl %code %bytesd %referer %ua %extra1")
    {
        return Regex::new(
            r#"^(?P<host>\S+)\s+(?P<logname>\S+)\s+(?P<user>\S+)\s+\[(?P<time>[^]]+)\]\s+\"(?P<request>.*?)\"\s+(?P<status>\d{3})\s+(?P<bytes>\d+)\s+\"(?P<referer>.*?)\"\s+\"(?P<ua>.*?)\"\s+(?P<extra1>\S+)$"#,
        );
    }
    let mut pattern = String::from("^");
    let mut ignored_field = 0usize;
    let bracketed_time1 = format.contains("[%time1]");
    let mut chars = format.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%' {
            let mut token = String::new();
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                token.push(chars.next().unwrap());
            }
            let (name, expression): (String, &str) = match token.as_str() {
                "host" => ("host".into(), r"\S+"),
                "logname" => ("logname".into(), r".*?"),
                "user" => ("user".into(), r"\S+"),
                "time1" => (
                    "time".into(),
                    if bracketed_time1 {
                        r"[^]]+"
                    } else {
                        r"\[[^]]+\]"
                    },
                ),
                "time1b" => ("time".into(), r"\[[^]]+\]"),
                "time1bracket" => ("time".into(), r"\[[^]]+\]"),
                "time2" => ("time".into(), r"\S+\s+\S+"),
                "time3" => ("time".into(), r"\S+\s+\S+\s+\S+"),
                "time4" => ("time".into(), r"\d+"),
                "time5" => ("time".into(), r"\S+"),
                "time6" => ("time".into(), r"\S+\s+\S+"),
                "time7" | "time8" => ("time".into(), r"\d+"),
                "methodurl" | "methodurlnoprot" => ("request".into(), r#".*?"#),
                "method" => ("method".into(), r"\S+"),
                "url" => ("url".into(), r"\S+"),
                "query" => ("query".into(), r"\S*"),
                "protocol" => ("protocol".into(), r"\S+"),
                "duration" => ("duration".into(), r"\d+(?:\.\d+)?"),
                "duration_ms" | "duration_us" => (token.clone(), r"\d+(?:\.\d+)?"),
                "code" => ("status".into(), r"\d{3}"),
                "bytesd" => ("bytes".into(), r"(?:\d+|-)"),
                "referer" | "refererquot" => ("referer".into(), r#".*?"#),
                "ua" | "uaquot" | "uabracket" => ("ua".into(), r#".*?"#),
                "extra1" => ("extra1".into(), r"\S+"),
                "rqtime" => ("duration".into(), r"\S+"),
                "virtualname" => ("virtual_host".into(), r"\S+"),
                "other1" | "other2" => {
                    let name = format!("ignored{ignored_field}");
                    ignored_field += 1;
                    (name, r"\S+")
                }
                "otherquot" => {
                    let name = format!("ignored{ignored_field}");
                    ignored_field += 1;
                    (name, r#".*?"#)
                }
                _ => {
                    let name = format!("ignored{ignored_field}");
                    ignored_field += 1;
                    (name, r"\S+")
                }
            };
            pattern.push_str(&format!("(?P<{name}>{expression})"));
        } else if ch.is_whitespace() {
            pattern.push_str(r"\s+");
        } else {
            pattern.push_str(&regex::escape(&ch.to_string()));
        }
    }
    pattern.push('$');
    Regex::new(&pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_time_formats_in_rule_order() {
        assert!(matches!(detect_time_format("1"), TimeFormat::Time1));
        assert!(matches!(detect_time_format("%time1b"), TimeFormat::Time1b));
        assert!(matches!(
            detect_time_format("%host %time4"),
            TimeFormat::Time4
        ));
        assert!(matches!(
            detect_time_format("%time7 %time4"),
            TimeFormat::Time4
        ));
        assert!(matches!(detect_time_format("custom"), TimeFormat::Time1));
    }

    #[test]
    fn parses_supplied_nginx_custom_shape() {
        let parser = LogParser::new(
            "%host %logname %user %time1 %methodurl %code %bytesd %referer %ua %extra1",
        )
        .unwrap();
        let outcome = parser.parse(r#"203.0.113.11 - - [27/Oct/2026:00:14:17 +0000] "GET / HTTP/1.1" 200 1318 "-" "Mozilla/5.0" 0.000"#);
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected event");
        };
        assert_eq!(event.status, 200);
        assert_eq!(event.url.as_deref(), Some("/"));
        assert_eq!(event.bytes, 1318);
        assert_eq!(event.duration_seconds, Some(0.0));
    }

    #[test]
    fn preserves_ipv6_client_addresses_for_ipv6_plugin_compatibility() {
        let parser = LogParser::new("%host %time4 %methodurl %code %bytesd").unwrap();
        let outcome = parser.parse("2001:db8::42 1793059200 GET /health 204 0");
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected IPv6 event");
        };
        assert_eq!(event.client_ip, "2001:db8::42");
        assert_eq!(event.status, 204);
    }

    #[test]
    fn classifies_binary_like_request_as_unknown_method() {
        let parser = LogParser::new(
            "%host %logname %user %time1 %methodurl %code %bytesd %referer %ua %extra1",
        )
        .unwrap();
        let outcome = parser.parse(r#"203.0.113.12 - - [27/Oct/2026:00:04:09 +0000] "\x03\x00\x00\x13" 400 150 "-" "-" 0.150"#);
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected event");
        };
        assert_eq!(event.method.as_deref(), Some("UNKNOWN"));
    }

    #[test]
    fn rejects_oversized_lines_with_a_diagnostic() {
        let parser = LogParser::new("%host %time1 %methodurl %code %bytesd").unwrap();
        let line = "x".repeat(MAX_LINE_LENGTH + 1);
        assert!(matches!(
            parser.parse(&line),
            ParseOutcome::Diagnostic(ParseError::LineTooLong)
        ));
    }

    #[test]
    fn parses_unix_timestamps() {
        let parser =
            LogParser::new("%host %logname %user %time4 %methodurl %code %bytesd").unwrap();
        let outcome = parser.parse("192.0.2.1 - - 1793059200 GET /health 204 0");
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected event");
        };
        assert_eq!(event.status, 204);
        assert_eq!(event.url.as_deref(), Some("/health"));
    }

    #[test]
    fn supports_predefined_common_log_format_four() {
        let parser = LogParser::new("4").unwrap();
        let outcome = parser
            .parse("192.0.2.1 - - [27/Oct/2026:00:14:17 +0000] \"GET /health HTTP/1.1\" 200 2");
        assert!(matches!(outcome, ParseOutcome::Event(_)));
    }

    #[test]
    fn parses_json_access_records() {
        let parser = LogParser::new("json").unwrap();
        let outcome = parser.parse(r#"{"remote_addr":"192.0.2.9","server_name":"api.example.test","time":"2026-10-27T00:14:17Z","method":"GET","url":"/api","status":200,"bytes":42,"user_agent":"curl/8.0","request_time":0.25}"#);
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected JSON event");
        };
        assert_eq!(event.client_ip, "192.0.2.9");
        assert_eq!(event.url.as_deref(), Some("/api"));
        assert_eq!(event.status, 200);
        assert_eq!(event.duration_seconds, Some(0.25));
        assert_eq!(event.virtual_host.as_deref(), Some("api.example.test"));
    }

    #[test]
    fn parses_common_apache_log_format_tokens() {
        let parser = LogParser::new(
            r#"%v:%p [%a]:%{remote}p %l %u %t "%r" %>s %O "%{Referer}i" "%{User-Agent}i""#,
        )
        .unwrap();
        let outcome = parser.parse(
            r#"www.example.test:443 [192.0.2.10]:53144 - alice [27/Oct/2026:00:14:17 +0000] "GET /%66oo HTTP/1.1" 200 42 "https://search.example/?q=rust" "Mozilla/5.0""#,
        );
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected Apache-format event");
        };
        assert_eq!(event.client_ip, "192.0.2.10");
        assert_eq!(event.virtual_host.as_deref(), Some("www.example.test"));
        assert_eq!(event.url.as_deref(), Some("/foo"));
        assert_eq!(event.status, 200);
        assert_eq!(event.bytes, 42);
    }

    #[test]
    fn parses_awstats_custom_time1_format_with_bracketed_log_timestamp() {
        let parser = LogParser::new(
            "%host %other %logname %time1 %methodurl %code %bytesd %refererquot %uaquot",
        )
        .unwrap();
        let outcome = parser.parse(
            r#"127.0.0.1 - - [01/Jan/2001:16:00:00 +0100] "GET / HTTP/1.1" 200 42 "-" "test-agent""#,
        );
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected AWStats custom-format event");
        };
        assert_eq!(event.status, 200);
        assert_eq!(event.url.as_deref(), Some("/"));
        assert_eq!(event.bytes, 42);
    }

    #[test]
    fn parses_awstats_time5_iso8601_timestamp_with_fraction_and_offset() {
        let parser = LogParser::new(
            "%host %other %logname %time5 %methodurl %code %bytesd %refererquot %uaquot",
        )
        .unwrap();
        let ParseOutcome::Event(event) = parser.parse(
            r#"80.8.55.10 - - 2001-01-01T23:55:10.147454Z "GET / HTTP/1.1" 200 70476 "-" "test-agent""#,
        ) else {
            panic!("expected AWStats time5 event");
        };
        assert_eq!(event.status, 200);
        assert_eq!(event.url.as_deref(), Some("/"));
        assert_eq!(event.bytes, 70476);
        assert_eq!(
            event.timestamp.to_rfc3339(),
            "2001-01-01T23:55:10.147454+00:00"
        );
    }

    #[test]
    fn accepts_spaced_logname_and_dash_bytes_in_awstats_format() {
        let parser = LogParser::new(
            "%host %other %logname %time1 %methodurl %code %bytesd %refererquot %uaquot",
        )
        .unwrap();
        let ParseOutcome::Event(event) = parser.parse(
            r#"80.8.55.7 - John Begood [01/Jan/2001:13:01:00 +0100] "SEARCH / -" 411 - "-" "-""#,
        ) else {
            panic!("expected AWStats event with optional fields");
        };
        assert_eq!(event.identity.as_deref(), Some("John Begood"));
        assert_eq!(event.status, 411);
        assert_eq!(event.bytes, 0);
    }

    #[test]
    fn parses_awstats_time1b_timestamp_without_timezone() {
        let parser = LogParser::new("%host %time1b %methodurl %code %bytesd").unwrap();
        let ParseOutcome::Event(event) =
            parser.parse(r#"192.0.2.1 [01/Jan/2001:13:01:00] "GET / HTTP/1.1" 200 1"#)
        else {
            panic!("expected AWStats time1b event");
        };
        assert_eq!(event.timestamp.to_rfc3339(), "2001-01-01T13:01:00+00:00");
    }

    #[test]
    fn parses_awstats_numeric_iis_format_two() {
        let parser = LogParser::new("2").unwrap();
        let ParseOutcome::Event(event) = parser.parse(
            "2001-01-01 13:01:00 192.0.2.1 - GET /index.html 200 123 HTTP/1.1 Mozilla/5.0 -",
        ) else {
            panic!("expected AWStats IIS format event");
        };
        assert_eq!(event.client_ip, "192.0.2.1");
        assert_eq!(event.method.as_deref(), Some("GET"));
        assert_eq!(event.url.as_deref(), Some("/index.html"));
        assert_eq!(event.status, 200);
        assert_eq!(event.bytes, 123);
    }

    #[test]
    fn decodes_only_unreserved_url_escapes() {
        let parser = LogParser::new("%host [%time1] %methodurl %code %bytesd").unwrap();
        let outcome = parser
            .parse(r#"192.0.2.1 [27/Oct/2026:00:14:17 +0000] GET /%66oo/%2F/%7E HTTP/1.1 200 1"#);
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected event");
        };
        assert_eq!(event.url.as_deref(), Some("/foo/%2F/~"));
    }

    #[test]
    fn parses_apache_forwarded_and_host_headers() {
        let parser = LogParser::new(r#"%{X-Forwarded-For}i "%{Host}i" %t "%r" %>s %O"#).unwrap();
        let outcome = parser.parse(
            r#"198.51.100.7 "www.example.test" [27/Oct/2026:00:14:17 +0000] "GET / HTTP/1.1" 200 12"#,
        );
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected Apache header event");
        };
        assert_eq!(event.client_ip, "198.51.100.7");
        assert_eq!(event.virtual_host.as_deref(), Some("www.example.test"));
        assert_eq!(event.status, 200);
        assert_eq!(event.bytes, 12);
    }

    #[test]
    fn parses_apache_split_request_tokens() {
        let parser = LogParser::new("%a %t \"%m %U%q %H\" %>s %O").unwrap();
        let outcome = parser.parse(
            r#"192.0.2.21 [27/Oct/2026:00:14:17 +0000] "GET /search?q=rust HTTP/1.1" 200 128"#,
        );
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected split-request event");
        };
        assert_eq!(event.method.as_deref(), Some("GET"));
        assert_eq!(event.url.as_deref(), Some("/search?q=rust"));
        assert_eq!(event.request, "GET /search?q=rust HTTP/1.1");
        assert_eq!(event.bytes, 128);
    }

    #[test]
    fn parses_apache_duration_tokens() {
        let parser = LogParser::new("%a %t \"%r\" %>s %O %D").unwrap();
        let outcome = parser
            .parse(r#"192.0.2.22 [27/Oct/2026:00:14:17 +0000] "GET / HTTP/1.1" 200 128 250000"#);
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected duration event");
        };
        assert_eq!(event.duration_seconds, Some(0.25));

        let parser = LogParser::new("%a %t \"%r\" %>s %O %{ms}T").unwrap();
        let outcome =
            parser.parse(r#"192.0.2.22 [27/Oct/2026:00:14:17 +0000] "GET / HTTP/1.1" 200 128 125"#);
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected millisecond duration event");
        };
        assert_eq!(event.duration_seconds, Some(0.125));
    }

    #[test]
    fn accepts_arbitrary_apache_header_tokens() {
        let parser = LogParser::new(r#"%a %t "%r" %>s %O "%{Cookie}i" %{Request-Id}o"#).unwrap();
        let outcome = parser.parse(
            r#"192.0.2.23 [27/Oct/2026:00:14:17 +0000] "GET / HTTP/1.1" 200 128 "session=abc 123" req-42"#,
        );
        assert!(matches!(outcome, ParseOutcome::Event(_)));
    }

    #[test]
    fn applies_configured_timezone_offset() {
        let parser = LogParser::new("%host [%time1] %methodurl %code %bytesd").unwrap();
        let ParseOutcome::Event(mut event) =
            parser.parse("192.0.2.24 [27/Oct/2026:23:30:00 +0000] GET / HTTP/1.1 200 1")
        else {
            panic!("expected timezone event");
        };
        assert!(event.apply_timezone_offset(2));
        assert_eq!(
            event.timestamp.format("%Y-%m-%d %H:%M").to_string(),
            "2026-10-28 01:30"
        );
    }

    #[test]
    fn applies_named_timezone_with_daylight_saving() {
        let parser = LogParser::new("%host [%time1] %methodurl %code %bytesd").unwrap();
        let ParseOutcome::Event(mut event) =
            parser.parse("192.0.2.24 [01/Jul/2026:23:30:00 +0000] GET / HTTP/1.1 200 1")
        else {
            panic!("expected timezone event");
        };
        assert!(event.apply_timezone("Europe/Warsaw"));
        assert_eq!(
            event.timestamp.format("%Y-%m-%d %H:%M %:z").to_string(),
            "2026-07-02 01:30 +02:00"
        );
        assert!(!event.apply_timezone("Not/AZone"));
    }

    #[test]
    fn parses_apache_connection_and_vhost_aliases() {
        let parser = LogParser::new(r#"%{c}a %t "%{canonical}v" "%r" %>s %O %{local}p"#).unwrap();
        let outcome = parser.parse(
            r#"192.0.2.25 [27/Oct/2026:00:14:17 +0000] "www.example.test" "GET / HTTP/1.1" 200 64 443"#,
        );
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected alias event");
        };
        assert_eq!(event.client_ip, "192.0.2.25");
        assert_eq!(event.virtual_host.as_deref(), Some("www.example.test"));
        assert_eq!(event.status, 200);
    }

    #[test]
    fn parses_apache_epoch_timestamp_token() {
        let parser = LogParser::new(r#"%a %{sec}t "%r" %>s %O"#).unwrap();
        let outcome = parser.parse(r#"192.0.2.26 1793059200 "GET / HTTP/1.1" 200 64"#);
        let ParseOutcome::Event(event) = outcome else {
            panic!("expected epoch timestamp event");
        };
        assert_eq!(event.timestamp.timestamp(), 1793059200);
        assert_eq!(event.status, 200);
    }

    #[test]
    fn parses_high_resolution_apache_epoch_timestamp_tokens() {
        let parser = LogParser::new(r#"%a %{msec}t "%r" %>s %O"#).unwrap();
        let ParseOutcome::Event(event) =
            parser.parse(r#"192.0.2.27 1793059200123 "GET / HTTP/1.1" 200 64"#)
        else {
            panic!("expected millisecond epoch event");
        };
        assert_eq!(event.timestamp.timestamp(), 1793059200);
        assert_eq!(event.timestamp.timestamp_subsec_millis(), 123);

        let parser = LogParser::new(r#"%a %{usec}t "%r" %>s %O"#).unwrap();
        let ParseOutcome::Event(event) =
            parser.parse(r#"192.0.2.27 1793059200123456 "GET / HTTP/1.1" 200 64"#)
        else {
            panic!("expected microsecond epoch event");
        };
        assert_eq!(event.timestamp.timestamp_subsec_micros(), 123_456);
    }

    #[test]
    fn parses_apache_begin_end_epoch_timestamp_aliases() {
        let parser = LogParser::new(r#"%a %{begin:msec}t "%r" %>s %O"#).unwrap();
        let ParseOutcome::Event(event) =
            parser.parse(r#"192.0.2.28 1793059200456 "GET / HTTP/1.1" 200 64"#)
        else {
            panic!("expected begin timestamp event");
        };
        assert_eq!(event.timestamp.timestamp(), 1793059200);
        assert_eq!(event.timestamp.timestamp_subsec_millis(), 456);

        let parser = LogParser::new(r#"%a %{end:sec}t "%r" %>s %O"#).unwrap();
        let ParseOutcome::Event(event) =
            parser.parse(r#"192.0.2.28 1793059200 "GET / HTTP/1.1" 200 64"#)
        else {
            panic!("expected end timestamp event");
        };
        assert_eq!(event.timestamp.timestamp(), 1793059200);
    }

    #[test]
    fn accepts_apache_fractional_timestamp_modifiers() {
        let parser = LogParser::new(r#"%a %t %{msec_frac}t %{usec_frac}t "%r" %>s %O"#).unwrap();
        let outcome = parser.parse(
            r#"192.0.2.29 [27/Oct/2026:00:14:17 +0000] .123 123456 "GET / HTTP/1.1" 200 64"#,
        );
        assert!(matches!(outcome, ParseOutcome::Event(_)));
    }

    #[test]
    fn accepts_apache_auxiliary_modifier_fields() {
        let parser = LogParser::new(r#"%a %t "%r" %>s %O %{pid}P %{cookie}C %{note}n"#).unwrap();
        let outcome = parser.parse(
            r#"192.0.2.30 [27/Oct/2026:00:14:17 +0000] "GET / HTTP/1.1" 200 64 1234 "sid=abc" note-value"#,
        );
        assert!(matches!(outcome, ParseOutcome::Event(_)));
    }
}
