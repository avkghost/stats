use anyhow::{bail, Context};
use stats_core::{Config, FileSandbox, ReportService, StatsStore};
use stats_render::{
    builtin_renderer_registry, register_configured_renderer, render_html_section,
    render_report_stream,
};
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
    section: Option<String>,
}

fn parse_query(query: &str) -> anyhow::Result<CgiRequest> {
    let mut output = OutputFormat::Html;
    let mut limit = DEFAULT_LIMIT;
    let mut section = None;
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
            "section" => {
                if value.is_empty() || value.contains('/') || value.contains('\\') {
                    bail!("invalid CGI section {value:?}");
                }
                section = Some(value);
            }
            _ => bail!("unsupported CGI parameter {key:?}"),
        }
    }
    if section.is_some() && output != OutputFormat::Html {
        bail!("CGI section selection is supported only for HTML output");
    }
    Ok(CgiRequest {
        output,
        limit,
        section,
    })
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

fn render_cgi(
    data_dir: PathBuf,
    request: &CgiRequest,
    config: Option<&Config>,
) -> anyhow::Result<(String, String)> {
    let sandbox = FileSandbox::for_data_dir(&data_dir).with_context(|| {
        format!(
            "initialize CGI filesystem sandbox for {}",
            data_dir.display()
        )
    })?;
    let database = sandbox.check_existing(&data_dir.join("stats.sqlite3"))?;
    let store = StatsStore::open_read_only(&database)
        .with_context(|| format!("open CGI database {}", database.display()))?;
    let mut report = ReportService::new(&store).build(request.limit)?;
    if let Some(config) = config {
        let config = config.clone().with_overrides(None, Some(data_dir.clone()));
        if config
            .plugins
            .iter()
            .any(|plugin| plugin.name == "urlalias")
        {
            sandbox.check_existing(&config.url_alias_path())?;
        }
        report.apply_url_aliases(&config.url_aliases()?);
    }
    let registry = builtin_renderer_registry()
        .map_err(|error| anyhow::anyhow!("register CGI renderers: {error}"))?;
    let mut registry = registry;
    let external_html = config.is_some_and(|config| {
        config
            .plugins
            .iter()
            .any(|plugin| stats_render::report_plugin_format(plugin).as_deref() == Some("html"))
    });
    if request.output != OutputFormat::Html || external_html {
        let config = config.ok_or_else(|| {
            anyhow::anyhow!(
                "CGI output {} is an external plugin; set STATS_CONFIG to a configuration with LoadPlugin=report_{} <library>",
                request.output.renderer_name(),
                request.output.renderer_name()
            )
        })?;
        register_configured_renderer(&mut registry, config, request.output.renderer_name())?;
    }
    if request.section.is_none() && !external_html && request.output == OutputFormat::Html {
        bail!(
            "CGI HTML rendering is an external plugin; set STATS_CONFIG with LoadPlugin=report_html <library>"
        );
    }
    let body = match request.section.as_deref() {
        Some(section) if external_html => {
            stats_render::render_report_section_stream(&registry, &report, "html", section)?
        }
        Some(section) => render_html_section(&report, section)
            .map_err(|error| anyhow::anyhow!("render CGI HTML section: {error}"))?,
        None => render_report_stream(&registry, &report, request.output.renderer_name())?,
    };
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
    let config = env::var_os("STATS_CONFIG")
        .map(Config::from_path)
        .transpose()?;
    let (media_type, body) = render_cgi(data_dir, &request, config.as_ref())?;
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
                section: None,
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
                section: None,
            }
        );
    }

    #[test]
    fn rejects_unknown_and_unsafe_parameters() {
        assert!(parse_query("path=../stats.sqlite3").is_err());
        assert!(parse_query("output=../../etc/passwd").is_err());
        assert!(parse_query("limit=10001").is_err());
        assert!(parse_query("section=../visitors").is_err());
        assert!(parse_query("output=json&section=visitors").is_err());
    }

    #[test]
    fn accepts_allowlisted_html_sections() {
        assert_eq!(
            parse_query("section=visitors").unwrap(),
            CgiRequest {
                output: OutputFormat::Html,
                limit: DEFAULT_LIMIT,
                section: Some("visitors".into()),
            }
        );
    }
}
