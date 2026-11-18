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
    bearer_token_matches, render_redacted_raw_log, Config, FileSandbox, ReportService, StatsStore,
    MAX_RAW_LOG_LINES,
};
use stats_render::{
    builtin_renderer_registry, register_configured_renderer, render_html_report,
    render_report_section_stream, render_report_stream,
};
use std::{error::Error, fmt};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

#[derive(Debug)]
enum RequestError {
    BadRequest(String),
    NotImplemented(String),
}

impl fmt::Display for RequestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(message) | Self::NotImplemented(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl Error for RequestError {}

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
    raw_log_enabled: bool,
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
    let raw_log_enabled = config
        .as_ref()
        .is_some_and(|config| config.plugin_enabled("rawlog"));
    let state = Arc::new(AppState {
        data_dir,
        config,
        sandbox,
        raw_log_token: std::env::var("STATS_RAW_LOG_AUTH_TOKEN")
            .ok()
            .filter(|token| !token.is_empty()),
        raw_log_enabled,
    });
    let app = app(state);
    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("bind HTTP listener on {}", args.bind))?;
    axum::serve(listener, app)
        .await
        .context("serve HTTP requests")?;
    Ok(())
}

fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(report))
        .route("/report", get(report))
        .route("/section/{section}", get(section))
        .route("/raw-log", get(raw_log))
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state)
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
    if !state.raw_log_enabled {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(expected) = state.raw_log_token.as_deref() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| bearer_token_matches(expected, value));
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
        let cluster_info = if config
            .plugins
            .iter()
            .any(|plugin| plugin.name == "clusterinfo")
        {
            let path = state.sandbox.check_existing(&config.cluster_info_path())?;
            config.cluster_info_from_path(&path)?
        } else {
            std::collections::HashMap::new()
        };
        report.apply_cluster_info(&cluster_info);
        let user_info = if config
            .plugins
            .iter()
            .any(|plugin| plugin.name == "userinfo")
        {
            let path = state.sandbox.check_existing(&config.user_info_path())?;
            config.user_info_from_path(&path)?
        } else {
            std::collections::HashMap::new()
        };
        report.apply_user_info(&user_info);
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
        (_, Some(_), _) => {
            return Err(anyhow::Error::new(RequestError::BadRequest(
                "sections are supported only for HTML output".into(),
            )))
        }
        (output, None, _) => {
            let config = state.config.as_ref().ok_or_else(|| {
                anyhow::Error::new(RequestError::NotImplemented(format!(
                    "output {output} requires --config with LoadPlugin=report_{output}"
                )))
            })?;
            register_configured_renderer(&mut registry, config, output)?;
            render_report_stream(&registry, &report, output)?
        }
    };
    let media_type = match output {
        "html" => "text/html; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        _ => {
            return Err(anyhow::Error::new(RequestError::BadRequest(format!(
                "unsupported output format {output:?}; use html, json, or csv"
            ))))
        }
    };
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, media_type)], body).into_response())
}

fn error_response(error: anyhow::Error) -> Response {
    eprintln!("HTTP report request failed: {error:#}");
    let status = match error.downcast_ref::<RequestError>() {
        Some(RequestError::BadRequest(_)) => StatusCode::BAD_REQUEST,
        Some(RequestError::NotImplemented(_)) => StatusCode::NOT_IMPLEMENTED,
        None => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, "report request failed\n").into_response()
}

#[cfg(test)]
mod tests {
    use super::{app, error_response, AppState, RequestError};
    use axum::{body::to_bytes, body::Body, http::Request};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_state() -> Arc<AppState> {
        // Tests run in parallel inside one process. A timestamp alone can repeat, so each state
        // gets a unique directory; otherwise one test's database changes another test's result.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let data_dir = std::env::temp_dir().join(format!(
            "stats-http-routes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&data_dir);
        std::fs::create_dir_all(&data_dir).unwrap();
        Arc::new(AppState {
            sandbox: stats_core::FileSandbox::for_data_dir(&data_dir).unwrap(),
            data_dir,
            config: None,
            raw_log_token: None,
            raw_log_enabled: false,
        })
    }

    #[tokio::test]
    async fn public_routes_have_stable_statuses() {
        let state = test_state();
        let response = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let response = app(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/raw-log")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);

        let response = app(state)
            .oneshot(
                Request::builder()
                    .uri("/readyz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        );
    }

    async fn get(state: Arc<AppState>, uri: &str) -> axum::response::Response {
        app(state)
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn database_for(state: &AppState) {
        stats_core::StatsStore::open(state.data_dir.join("stats.sqlite3")).unwrap();
    }

    #[tokio::test]
    async fn report_without_a_database_is_a_server_error() {
        let response = get(test_state(), "/report").await;
        assert_eq!(
            response.status(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[tokio::test]
    async fn report_html_with_a_database_returns_html() {
        let state = test_state();
        database_for(&state);
        let response = get(state, "/report").await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn report_json_without_configuration_is_not_implemented() {
        let state = test_state();
        database_for(&state);
        let response = get(state, "/report?output=json").await;
        assert_eq!(response.status(), axum::http::StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn section_overview_with_a_database_returns_an_html_section() {
        let state = test_state();
        database_for(&state);
        let response = get(state, "/section/overview").await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
    }

    #[tokio::test]
    async fn maps_client_and_missing_plugin_errors_without_leaking_details() {
        let response = error_response(anyhow::Error::new(RequestError::BadRequest(
            "sections are supported only for HTML output".into(),
        )));
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"report request failed\n");

        let response = error_response(anyhow::Error::new(RequestError::NotImplemented(
            "output csv requires --config".into(),
        )));
        assert_eq!(response.status(), axum::http::StatusCode::NOT_IMPLEMENTED);
    }
}
