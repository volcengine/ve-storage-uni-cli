use ask_auth::{build_router, request_log_subscriber};
use axum::{body::Body, http::Request};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};
use tower::ServiceExt;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn logs_only_safe_route_metadata_and_server_generated_identity() {
    let capture = Capture::default();
    let subscriber =
        request_log_subscriber(tracing::level_filters::LevelFilter::TRACE, capture.clone());
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);
    tracing::error!(target: "foreign_dependency", "FOREIGN_TARGET_SECRET");
    for (method, path, body) in [
        (
            "GET",
            "/v1/auth/login-attempts/RAW_PATH_SECRET?x=QUERY_SECRET",
            "",
        ),
        (
            "GET",
            "/v1/oauth/doubao/callback?code=CALLBACK_CODE_SECRET&state=CALLBACK_STATE_SECRET",
            "",
        ),
        ("POST", "/v1/auth/login-attempts", "BODY_TOKEN_SECRET"),
        ("RAW_METHOD_SECRET", "/unknown/PATH_SECRET", ""),
    ] {
        let call = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", "Bearer AUTHORIZATION_SECRET")
            .header("x-request-id", "INBOUND_ID_SECRET")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        build_router().oneshot(call).await.unwrap();
    }
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.matches("request completed").count(), 4);
    assert_no_secrets(&logs);
    assert!(logs.contains("/v1/auth/login-attempts/{auth_id}"));
    assert!(logs.contains("<unmatched>"));
    for field in ["method=", "request_id=", "status=", "duration_ms="] {
        assert!(logs.contains(field));
    }
}

fn assert_no_secrets(logs: &str) {
    for secret in [
        "FOREIGN_TARGET_SECRET",
        "RAW_PATH_SECRET",
        "QUERY_SECRET",
        "CALLBACK_CODE_SECRET",
        "CALLBACK_STATE_SECRET",
        "BODY_TOKEN_SECRET",
        "RAW_METHOD_SECRET",
        "PATH_SECRET",
        "AUTHORIZATION_SECRET",
        "INBOUND_ID_SECRET",
    ] {
        assert!(
            !logs.contains(secret),
            "sensitive sentinel appeared in logs"
        );
    }
}

#[test]
fn cleanup_reports_admit_only_controlled_metrics_and_block_sqlx_traces() {
    use ask_auth::cleanup::{CleanupAlert, CleanupObserver, CleanupReport, TracingCleanupObserver};
    let capture = Capture::default();
    let subscriber =
        request_log_subscriber(tracing::level_filters::LevelFilter::TRACE, capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::trace!(target: "sqlx::query", "SQLX_CLEANUP_SECRET_SENTINEL");
    TracingCleanupObserver.observe(&CleanupReport {
        request_id: uuid::Uuid::now_v7(),
        duration: std::time::Duration::from_millis(10),
        transaction_duration: std::time::Duration::from_millis(2),
        affected_rows: 2,
        transactions: 1,
        lock_skips: 0,
        retries: 0,
        failures: 0,
        oldest_overdue: [None; 6],
        error: None,
        alert: CleanupAlert::None,
        is_paused: false,
    });
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("cleanup pass"));
    assert!(!logs.contains("SQLX_CLEANUP_SECRET_SENTINEL"));
    assert!(!logs.contains("auth_id="));
    drop(_guard);
    let subscriber =
        request_log_subscriber(tracing::level_filters::LevelFilter::TRACE, capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);
    for alert in [CleanupAlert::Integrity, CleanupAlert::BacklogWarning] {
        TracingCleanupObserver.observe(&CleanupReport {
            request_id: uuid::Uuid::now_v7(),
            duration: std::time::Duration::ZERO,
            transaction_duration: std::time::Duration::ZERO,
            affected_rows: 0,
            transactions: 0,
            lock_skips: 0,
            retries: 0,
            failures: 1,
            oldest_overdue: [None; 6],
            error: None,
            alert,
            is_paused: false,
        });
    }
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("ERROR") && logs.contains("WARN"));
}

#[test]
fn host_configuration_events_are_allowed_but_sdk_messages_are_filtered() {
    let capture = Capture::default();
    let subscriber =
        request_log_subscriber(tracing::level_filters::LevelFilter::TRACE, capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::info!(target: "ask_auth::config", request_id=%uuid::Uuid::now_v7(), status="unavailable", is_enabled=false, "configuration state");
    tracing::error!(target: "byted_bconfig", "SDK_RAW_SECRET_SENTINEL");
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("configuration state"));
    assert!(!logs.contains("SDK_RAW_SECRET_SENTINEL"));
}
