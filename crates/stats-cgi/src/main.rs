use anyhow::{bail, Context};
use stats_core::{ReportService, StatsStore};
use stats_render::{builtin_renderer_registry, render_report_stream};
use std::{
    env,
    io::{self, Write},
    path::PathBuf,
};

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Html,
    Json,
    Csv,
}

impl OutputFormat {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "html" => Ok(Self::Html),
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            _ => bail!("unsupported CGI output format {value:?}"),
        }
    }

    fn renderer_name(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Json => "json",
            Self::Csv => "csv",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CgiRequest {
    output: OutputFormat,
    limit: usize,
}

fn parse_query(query: &str) -> anyhow::Result<CgiRequest> {
    let mut output = OutputFormat::Html;
    let mut limit = DEFAULT_LIMIT;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, raw_value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("invalid CGI parameter {pair:?}"))?;
        let value = percent_decode(raw_value)?;
        match key {
            "output" => output = OutputFormat::parse(&value)?,
            "limit" => {
                limit = value
                    .parse::<usize>()
                    .with_context(|| format!("invalid CGI limit {value:?}"))?;
                if limit == 0 || limit > MAX_LIMIT {
                    bail!("CGI limit must be between 1 and {MAX_LIMIT}");
                }
            }
            _ => bail!("unsupported CGI parameter {key:?}"),
        }
    }
    Ok(CgiRequest { output, limit })
}

fn percent_decode(value: &str) -> anyhow::Result<String> {
    let bytes = value.as_bytes();
    let mut decoded = String::with_capacity(value.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                decoded.push(' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                let (Some(high), Some(low)) = (high, low) else {
                    bail!("invalid percent escape in CGI parameter");
                };
                decoded.push((high * 16 + low) as u8 as char);
                index += 3;
            }
            b'%' => bail!("truncated percent escape in CGI parameter"),
            _ => {
                let character = value[index..].chars().next().unwrap_or_default();
                decoded.push(character);
                index += character.len_utf8();
            }
        }
    }
    Ok(decoded)
}

fn render_cgi(data_dir: PathBuf, request: &CgiRequest) -> anyhow::Result<(String, String)> {
    let database = data_dir.join("stats.sqlite3");
    let store = StatsStore::open_read_only(&database)
        .with_context(|| format!("open CGI database {}", database.display()))?;
    let report = ReportService::new(&store).build(request.limit)?;
    let registry = builtin_renderer_registry()
        .map_err(|error| anyhow::anyhow!("register CGI renderers: {error}"))?;
    let body = render_report_stream(&registry, &report, request.output.renderer_name())?;
    let media_type = match request.output {
        OutputFormat::Html => "text/html; charset=utf-8",
        OutputFormat::Json => "application/json; charset=utf-8",
        OutputFormat::Csv => "text/csv; charset=utf-8",
    };
    Ok((media_type.to_owned(), body))
}

fn main() -> anyhow::Result<()> {
    let request = parse_query(&env::var("QUERY_STRING").unwrap_or_default())?;
    let data_dir = PathBuf::from(env::var("STATS_DATA_DIR").unwrap_or_else(|_| "stats".into()));
    let (media_type, body) = render_cgi(data_dir, &request)?;
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    writeln!(stdout, "Content-Type: {media_type}\r")?;
    writeln!(stdout)?;
    stdout.write_all(body.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_read_only_html_request() {
        assert_eq!(
            parse_query("").unwrap(),
            CgiRequest {
                output: OutputFormat::Html,
                limit: DEFAULT_LIMIT,
            }
        );
    }

    #[test]
    fn accepts_allowlisted_parameters() {
        assert_eq!(
            parse_query("output=json&limit=25").unwrap(),
            CgiRequest {
                output: OutputFormat::Json,
                limit: 25,
            }
        );
    }

    #[test]
    fn rejects_unknown_and_unsafe_parameters() {
        assert!(parse_query("path=../stats.sqlite3").is_err());
        assert!(parse_query("output=../../etc/passwd").is_err());
        assert!(parse_query("limit=10001").is_err());
    }
}
