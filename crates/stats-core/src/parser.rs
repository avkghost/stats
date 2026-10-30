use chrono::{DateTime, Datelike, FixedOffset, NaiveDateTime, TimeZone, Utc};
use regex::Regex;
use std::sync::OnceLock;
use thiserror::Error;

const MAX_LINE_LENGTH: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct AccessEvent {
    pub client_ip: String,
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
    Time2,
    Time3,
    Time4,
    Time5,
    Time6,
}

impl LogParser {
    pub fn new(format: impl Into<String>) -> Result<Self, regex::Error> {
        let format = normalize_apache_format(&format.into());
        let regex = compile_format(&format)?;
        let json = format.trim().eq_ignore_ascii_case("json");
        let time_format = if format == "1" || format.contains("%time1") {
            TimeFormat::Time1
        } else if format.contains("%time2") {
            TimeFormat::Time2
        } else if format.contains("%time3") {
            TimeFormat::Time3
        } else if format.contains("%time4") {
            TimeFormat::Time4
        } else if format.contains("%time5") {
            TimeFormat::Time5
        } else if format.contains("%time6") {
            TimeFormat::Time6
        } else {
            TimeFormat::Time1
        };
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
        let bytes = match get("bytes").and_then(|v| v.parse().ok()) {
            Some(v) => v,
            None => return ParseOutcome::Diagnostic(ParseError::Number("bytes".into())),
        };
        let request = get("request").unwrap_or_default();
        let (method, url) = split_request(&request);
        let duration_seconds = get("extra1")
            .and_then(|v| v.parse().ok())
            .or_else(|| get("duration").and_then(|v| v.parse().ok()));
        ParseOutcome::Event(Box::new(AccessEvent {
            client_ip: get("host").unwrap_or_default(),
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
        TimeFormat::Time1 => {
            DateTime::parse_from_str(value.trim_matches(['[', ']']), "%d/%b/%Y:%H:%M:%S %z").ok()
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
    }
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
        let long_token = if remaining.starts_with("%{User-Agent}i")
            || remaining.starts_with("%{user-agent}i")
            || remaining.starts_with("%{User-agent}i")
        {
            Some(("%{User-Agent}i", "%uaquot"))
        } else if remaining.starts_with("%{Referer}i") || remaining.starts_with("%{referer}i") {
            Some(("%{Referer}i", "%refererquot"))
        } else if remaining.starts_with("%{Host}i") || remaining.starts_with("%{host}i") {
            Some(("%{Host}i", "%virtualname"))
        } else if remaining.starts_with("%{X-Forwarded-For}i")
            || remaining.starts_with("%{x-forwarded-for}i")
        {
            Some(("%{X-Forwarded-For}i", "%host"))
        } else if remaining.starts_with("%{remote}p") {
            Some(("%{remote}p", "%other2"))
        } else if remaining.starts_with("%{status}o") {
            Some(("%{status}o", "%code"))
        } else if remaining.starts_with("%>s") {
            Some(("%>s", "%code"))
        } else {
            None
        };
        if let Some((apache, awstats)) = long_token {
            normalized.push_str(awstats);
            index += apache.len();
            continue;
        }

        let replacement = match bytes.get(index + 1).copied() {
            Some(b'v') => "%virtualname",
            Some(b'a') | Some(b'h') => "%host",
            Some(b'l') => "%logname",
            Some(b'u') => "%user",
            Some(b't') => "%time1bracket",
            Some(b'r') => "%methodurl",
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

fn compile_format(format: &str) -> Result<Regex, regex::Error> {
    static STANDARD: OnceLock<Regex> = OnceLock::new();
    if format == "1" {
        return Ok(STANDARD.get_or_init(|| Regex::new(r#"^(?P<host>\S+)\s+\S+\s+(?P<logname>\S+)\s+\[(?P<time>[^]]+)\]\s+\"(?P<request>.*?)\"\s+(?P<status>\d{3})\s+(?P<bytes>\d+)\s+\"(?P<referer>.*?)\"\s+\"(?P<ua>.*?)\"$"#).unwrap()).clone());
    }
    if format == "4" {
        return Regex::new(
            r#"^(?P<host>\S+)\s+\S+\s+(?P<logname>\S+)\s+\[(?P<time>[^]]+)\]\s+\"(?P<request>.*?)\"\s+(?P<status>\d{3})\s+(?P<bytes>\d+)$"#,
        );
    }
    if format.contains("%host %logname %user %time1 %methodurl %code %bytesd %referer %ua %extra1")
    {
        return Regex::new(
            r#"^(?P<host>\S+)\s+(?P<logname>\S+)\s+(?P<user>\S+)\s+\[(?P<time>[^]]+)\]\s+\"(?P<request>.*?)\"\s+(?P<status>\d{3})\s+(?P<bytes>\d+)\s+\"(?P<referer>.*?)\"\s+\"(?P<ua>.*?)\"\s+(?P<extra1>\S+)$"#,
        );
    }
    let mut pattern = String::from("^");
    let mut ignored_field = 0usize;
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
                "logname" => ("logname".into(), r"\S+"),
                "user" => ("user".into(), r"\S+"),
                "time1" => ("time".into(), r"[^]]+"),
                "time1bracket" => ("time".into(), r"\[[^]]+\]"),
                "time2" => ("time".into(), r"\S+\s+\S+"),
                "time3" => ("time".into(), r"\S+\s+\S+\s+\S+"),
                "time4" => ("time".into(), r"\d+"),
                "time5" => ("time".into(), r"\S+"),
                "time6" => ("time".into(), r"\S+\s+\S+"),
                "methodurl" | "methodurlnoprot" => ("request".into(), r#".*?"#),
                "code" => ("status".into(), r"\d{3}"),
                "bytesd" => ("bytes".into(), r"\d+"),
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
}
