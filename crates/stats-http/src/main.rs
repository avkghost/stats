use anyhow::{Context, Result};
use axum::{
    extract::{Path as RequestPath, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use clap::Parser;
use serde::Deserialize;
use stats_core::{Config, ReportService, StatsStore};
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
    let state = Arc::new(AppState { data_dir, config });
    let app = Router::new()
        .route("/", get(report))
        .route("/report", get(report))
        .route("/section/{section}", get(section))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("bind HTTP listener on {}", args.bind))?;
    axum::serve(listener, app)
        .await
        .context("serve HTTP requests")?;
    Ok(())
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
    let database = state.data_dir.join("stats.sqlite3");
    let store = StatsStore::open_read_only(&database)
        .with_context(|| format!("open HTTP database {}", database.display()))?;
    let report = ReportService::new(&store).build(limit)?;
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
