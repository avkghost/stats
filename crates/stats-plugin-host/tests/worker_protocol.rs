use serde_json::json;
use stats_plugin_host::{supervise_report_worker, WorkerRequestError, MAX_WORKER_MESSAGE_BYTES};
use std::{path::PathBuf, time::Duration};

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
