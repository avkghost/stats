use serde_json::json;
use stats_plugin_host::{supervise_report_worker, WorkerRequestError, MAX_WORKER_MESSAGE_BYTES};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stats_plugin_worker_fixture"))
}

fn run(mode: &str, timeout: Duration) -> Result<serde_json::Value, WorkerRequestError> {
    supervise_report_worker(
        fixture(),
        format!("fixture-mode://{mode}"),
        &json!({"operation":"metadata"}),
        timeout,
    )
}

#[test]
fn valid_response_is_accepted_and_following_request_succeeds() {
    let response = run("valid", Duration::from_secs(2)).expect("valid response");
    assert_eq!(response["ok"], true);
    assert!(run("valid", Duration::from_secs(2)).is_ok());
}

#[test]
fn raw_log_token_is_not_inherited_by_worker() {
    std::env::set_var("STATS_RAW_LOG_AUTH_TOKEN", "test-secret");
    let response = run("env-token", Duration::from_secs(2)).expect("worker response");
    std::env::remove_var("STATS_RAW_LOG_AUTH_TOKEN");
    assert_eq!(response["body"], r#"{"token_visible":false}"#);
}

#[test]
fn malformed_responses_are_rejected() {
    for mode in [
        "empty",
        "truncated",
        "extra",
        "diagnostic",
        "invalid-version",
        "invalid-type",
    ] {
        let result = run(mode, Duration::from_secs(2));
        assert!(result.is_err(), "fixture mode {mode} was accepted");
    }
}

#[test]
fn oversized_response_is_rejected_without_unbounded_reading() {
    let result = run("oversized", Duration::from_secs(2));
    assert!(matches!(result, Err(WorkerRequestError::MessageTooLarge)));
    assert_eq!(MAX_WORKER_MESSAGE_BYTES, 8 * 1024 * 1024);
    assert!(run("valid", Duration::from_secs(2)).is_ok());
}

#[test]
fn oversized_request_is_rejected_before_spawn() {
    let request = json!({
        "operation": "metadata",
        "padding": "x".repeat(MAX_WORKER_MESSAGE_BYTES),
    });
    let result = supervise_report_worker(
        fixture(),
        "fixture-mode://valid",
        &request,
        Duration::from_secs(2),
    );
    assert!(matches!(result, Err(WorkerRequestError::MessageTooLarge)));
}

#[test]
fn nonzero_exit_is_reported_and_worker_is_reaped() {
    let result = run("exit", Duration::from_secs(2));
    assert!(matches!(result, Err(WorkerRequestError::Exit(_))));
    assert!(run("valid", Duration::from_secs(2)).is_ok());
}

#[test]
fn abort_isolated_from_parent_process() {
    let result = run("abort", Duration::from_secs(2));
    assert!(result.is_err());
    assert!(run("valid", Duration::from_secs(2)).is_ok());
}

#[cfg(unix)]
#[test]
fn sigsegv_isolated_from_parent_process() {
    let result = run("segv", Duration::from_secs(2));
    match result {
        Err(WorkerRequestError::Exit(status)) => assert!(status.code().is_none()),
        other => panic!("expected signal termination, got {other:?}"),
    }
    assert!(run("valid", Duration::from_secs(2)).is_ok());
}

#[test]
fn timeout_kills_partial_and_silent_workers() {
    for mode in ["hang", "partial-hang"] {
        let result = run(mode, Duration::from_millis(150));
        assert!(matches!(result, Err(WorkerRequestError::Timeout(_))));
        assert!(run("valid", Duration::from_secs(2)).is_ok());
    }
}

#[test]
fn request_larger_than_pipe_buffer_still_times_out_for_non_reading_worker() {
    let request = json!({
        "operation": "metadata",
        "padding": "x".repeat(4 * 1024 * 1024),
    });
    let started = Instant::now();
    let result = supervise_report_worker(
        fixture(),
        "fixture-mode://hang",
        &request,
        Duration::from_millis(200),
    );
    assert!(matches!(result, Err(WorkerRequestError::Timeout(_))));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "supervisor blocked on stdin write for {:?}",
        started.elapsed()
    );
    assert!(run("valid", Duration::from_secs(2)).is_ok());
}

#[cfg(unix)]
#[test]
fn descendant_holding_stdin_cannot_block_supervisor_past_timeout() {
    let marker = std::env::temp_dir().join(format!("stats-fork-hold-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    std::env::set_var("STATS_FIXTURE_MARKER", &marker);
    let request = json!({
        "operation": "metadata",
        "padding": "x".repeat(4 * 1024 * 1024),
    });
    let started = Instant::now();
    let supervisor = std::thread::spawn(move || {
        supervise_report_worker(
            fixture(),
            "fixture-mode://fork-hold",
            &request,
            Duration::from_secs(3),
        )
    });
    // Wait until the worker's descendant exists, so the timeout fires while stdin is held open.
    let ready_deadline = Instant::now() + Duration::from_secs(2);
    while !marker.exists() && Instant::now() < ready_deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists(), "fixture did not start its descendant");
    let result = supervisor.join().expect("supervisor thread");
    std::env::remove_var("STATS_FIXTURE_MARKER");
    let _ = std::fs::remove_file(&marker);
    assert!(
        matches!(result, Err(WorkerRequestError::Timeout(_))),
        "{result:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "descendant kept stdin open and blocked the supervisor for {:?}",
        started.elapsed()
    );
    assert!(run("valid", Duration::from_secs(2)).is_ok());
}
