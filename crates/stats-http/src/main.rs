use anyhow::{Context, Result};
use axum::{
    extract::{Path as RequestPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use clap::Parser;
use serde::Deserialize;
use stats_core::{
    render_redacted_raw_log, Config, FileSandbox, ReportService, StatsStore, MAX_RAW_LOG_LINES,
};
use stats_render::{
    builtin_renderer_registry, register_configured_renderer, render_html_report,
    render_report_section_stream, render_report_stream,
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

#[derive(Debug, Parser)]
#[command(
    name = "stats-http",
    about = "Standalone HTTP frontend for stats reports"
)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:3000")]
    bind: SocketAddr,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[derive(Clone)]
struct AppState {
    data_dir: PathBuf,
    config: Option<Config>,
    sandbox: FileSandbox,
    raw_log_token: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ReportQuery {
    output: Option<String>,
    limit: Option<usize>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config = args
        .config
        .as_ref()
        .map(|path| {
            Config::from_path(path)
                .with_context(|| format!("load configuration {}", path.display()))
        })
        .transpose()?;
    let config = config.map(|config| config.with_overrides(None, args.data_dir.clone()));
    let data_dir = config
        .as_ref()
        .map(|config| config.data_dir.clone())
        .or(args.data_dir)
        .unwrap_or_else(|| PathBuf::from("stats"));
    let sandbox = FileSandbox::for_data_dir(&data_dir).with_context(|| {
        format!(
            "initialize HTTP filesystem sandbox for {}",
            data_dir.display()
        )
    })?;
    let state = Arc::new(AppState {
        data_dir,
        config,
        sandbox,
        raw_log_token: std::env::var("STATS_RAW_LOG_AUTH_TOKEN")
            .ok()
            .filter(|token| !token.is_empty()),
    });
    let app = Router::new()
        .route("/", get(report))
        .route("/report", get(report))
        .route("/section/{section}", get(section))
        .route("/raw-log", get(raw_log))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("bind HTTP listener on {}", args.bind))?;
    axum::serve(listener, app)
        .await
        .context("serve HTTP requests")?;
    Ok(())
}

async fn healthz() -> Response {
    (StatusCode::OK, "ok\n").into_response()
}

async fn readyz(State(state): State<Arc<AppState>>) -> Response {
    let result = tokio::task::spawn_blocking(move || {
        let database = state
            .sandbox
            .check_existing(&state.data_dir.join("stats.sqlite3"))?;
        StatsStore::open_read_only(&database)
            .with_context(|| format!("open readiness database {}", database.display()))
            .map(|_| ())
    })
    .await;
    match result {
        Ok(Ok(())) => (StatusCode::OK, "ready\n").into_response(),
        Ok(Err(error)) => (StatusCode::SERVICE_UNAVAILABLE, error.to_string()).into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            format!("readiness worker failed: {error}"),
        )
            .into_response(),
    }
}

async fn report(State(state): State<Arc<AppState>>, Query(query): Query<ReportQuery>) -> Response {
    let limit = query.limit.unwrap_or(100).clamp(1, 10_000);
    let output = query.output.unwrap_or_else(|| "html".into());
    match render_in_blocking(state, output, limit, None).await {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

async fn section(
    State(state): State<Arc<AppState>>,
    RequestPath(section): RequestPath<String>,
    Query(query): Query<ReportQuery>,
) -> Response {
    let limit = query.limit.unwrap_or(100).clamp(1, 10_000);
    match render_in_blocking(state, "html".into(), limit, Some(section)).await {
        Ok(response) => response,
        Err(error) => error_response(error),
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawLogQuery {
    start: Option<usize>,
    lines: Option<usize>,
}

async fn raw_log(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<RawLogQuery>,
) -> Response {
    let Some(expected) = state.raw_log_token.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|value| value == expected);
    if !authorized {
        eprintln!("raw-log authorization failed");
        return (
            StatusCode::UNAUTHORIZED,
            [
                (header::WWW_AUTHENTICATE, "Bearer"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            "raw-log authorization required\n",
        )
            .into_response();
    }
    let start = query.start.unwrap_or(0);
    let lines = query.lines.unwrap_or(100).clamp(1, MAX_RAW_LOG_LINES);
    eprintln!("raw-log access granted start={start} lines={lines}");
    let result = tokio::task::spawn_blocking(move || render_raw_log(&state, start, lines)).await;
    match result {
        Ok(Ok(body)) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            body,
        )
            .into_response(),
        Ok(Err(error)) => error_response(error),
        Err(error) => error_response(anyhow::anyhow!("raw-log worker failed: {error}")),
    }
}

fn render_raw_log(state: &AppState, start: usize, lines: usize) -> Result<String> {
    let config = state
        .config
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("raw-log requires --config"))?;
    let path = state.sandbox.check_existing(&config.log_file)?;
    render_redacted_raw_log(&path, start, lines)
}

async fn render_in_blocking(
    state: Arc<AppState>,
    output: String,
    limit: usize,
    section: Option<String>,
) -> Result<Response> {
    tokio::task::spawn_blocking(move || render_report(&state, &output, limit, section))
        .await
        .context("join HTTP report worker")?
}

fn render_report(
    state: &AppState,
    output: &str,
    limit: usize,
    section: Option<String>,
) -> Result<Response> {
    let database = state
        .sandbox
        .check_existing(&state.data_dir.join("stats.sqlite3"))?;
    let store = StatsStore::open_read_only(&database)
        .with_context(|| format!("open HTTP database {}", database.display()))?;
    let mut report = ReportService::new(&store).build(limit)?;
    if let ("html", Some(config)) = (output, state.config.as_ref()) {
        let aliases = if config
            .plugins
            .iter()
            .any(|plugin| plugin.name == "urlalias")
        {
            let path = state.sandbox.check_existing(&config.url_alias_path())?;
            config.url_aliases_from_path(&path)?
        } else {
            std::collections::HashMap::new()
        };
        report.apply_url_aliases(&aliases);
    }
    let mut registry = builtin_renderer_registry()
        .map_err(|error| anyhow::anyhow!("register HTTP renderers: {error}"))?;
    let external = state.config.as_ref().is_some_and(|config| {
        config
            .plugins
            .iter()
            .any(|plugin| stats_render::report_plugin_format(plugin).as_deref() == Some(output))
    });
    if external && section.is_some() {
        let config = state
            .config
            .as_ref()
            .expect("external renderer implies configured state");
        register_configured_renderer(&mut registry, config, output)?;
    }
    let body = match (output, section.as_deref(), external) {
        ("html", Some(section), true) => {
            render_report_section_stream(&registry, &report, "html", section)?
        }
        ("html", Some(section), false) => stats_render::render_html_section(&report, section)
            .map_err(|error| anyhow::anyhow!("render HTML section: {error}"))?,
        ("html", None, false) => render_html_report(&report),
        (_, Some(_), _) => anyhow::bail!("sections are supported only for HTML output"),
        (output, None, _) => {
            let config = state.config.as_ref().ok_or_else(|| {
                anyhow::anyhow!("output {output} requires --config with LoadPlugin=report_{output}")
            })?;
            register_configured_renderer(&mut registry, config, output)?;
            render_report_stream(&registry, &report, output)?
        }
    };
    let media_type = match output {
        "html" => "text/html; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        _ => anyhow::bail!("unsupported output format {output:?}; use html, json, or csv"),
    };
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, media_type)], body).into_response())
}

fn error_response(error: anyhow::Error) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
}
