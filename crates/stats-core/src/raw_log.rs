use std::{fs, io::Read, path::Path};

use anyhow::{Context, Result};
use regex::Regex;

pub const MAX_RAW_LOG_LINES: usize = 10_000;

pub fn bearer_token_matches(expected: &str, authorization: &str) -> bool {
    let Some(candidate) = authorization.strip_prefix("Bearer ") else {
        return false;
    };
    if candidate.len() != expected.len() {
        return false;
    }
    candidate
        .as_bytes()
        .iter()
        .zip(expected.as_bytes())
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

pub fn render_redacted(path: &Path, start: usize, lines: usize) -> Result<String> {
    let raw = fs::read(path).with_context(|| format!("read raw log {}", path.display()))?;
    let decoded = decode(path, &raw)?;
    let text = String::from_utf8_lossy(&decoded);
    let query = Regex::new(r#"\?[^\s\"]*"#)?;
    let secret =
        Regex::new(r#"(?i)(authorization|cookie|token|secret|password|api[_-]?key)=([^&\s\"]+)"#)?;
    let header_secret = Regex::new(r#"(?i)(authorization|cookie)\s*:\s*[^\s\"]+"#)?;
    let output = text
        .lines()
        .skip(start)
        .take(lines.min(MAX_RAW_LOG_LINES))
        .map(|line| redact_line(line, &query, &secret, &header_secret))
        .collect::<Vec<_>>();
    if output.is_empty() {
        Ok(String::new())
    } else {
        Ok(output.join("\n") + "\n")
    }
}

fn decode(path: &Path, raw: &[u8]) -> Result<Vec<u8>> {
    let extension = path.extension().and_then(|value| value.to_str());
    let mut decoded = Vec::new();
    match extension {
        Some("gz") => flate2::read::GzDecoder::new(raw).read_to_end(&mut decoded)?,
        Some("bz2") => bzip2::read::BzDecoder::new(raw).read_to_end(&mut decoded)?,
        Some("xz") => xz2::read::XzDecoder::new(raw).read_to_end(&mut decoded)?,
        _ => return Ok(raw.to_vec()),
    };
    Ok(decoded)
}

fn redact_line(line: &str, query: &Regex, secret: &Regex, header_secret: &Regex) -> String {
    let mut fields = line.splitn(2, char::is_whitespace);
    let _client = fields.next().unwrap_or_default();
    let rest = fields.next().unwrap_or_default();
    let body = query.replace_all(rest, "?<redacted>");
    let body = header_secret.replace_all(&body, "$1: <redacted>");
    let body = secret.replace_all(&body, "$1=<redacted>");
    format!("<client> {}", body.trim_start())
}

#[cfg(test)]
mod tests {
    use super::{bearer_token_matches, render_redacted};
    use std::fs;

    #[test]
    fn redacts_identity_queries_and_headers() {
        let path = std::env::temp_dir().join(format!(
            "stats-core-raw-log-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(
            &path,
            "203.0.113.10 - - \"GET /?token=secret HTTP/1.1\" Cookie: session\n",
        )
        .unwrap();

        let output = render_redacted(&path, 0, 10).unwrap();
        assert!(output.starts_with("<client> "));
        assert!(!output.contains("203.0.113.10"));
        assert!(!output.contains("secret"));
        assert!(!output.contains("session"));
        assert!(output.contains("?<redacted>"));

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn matches_only_the_expected_bearer_token() {
        assert!(bearer_token_matches("secret", "Bearer secret"));
        assert!(!bearer_token_matches("secret", "Bearer wrong"));
        assert!(!bearer_token_matches("secret", "Basic secret"));
        assert!(!bearer_token_matches("secret", "Bearer "));
    }
}
