use super::*;
use crate::platform::control::SourceFuture;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
struct Source {
    responses: Mutex<Vec<Result<HostSnapshot, AppError>>>,
    reads: AtomicUsize,
}
impl ConfigSource for Source {
    fn read(&self) -> SourceFuture<'_> {
        Box::pin(async {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.responses.lock().unwrap().remove(0)
        })
    }
}
#[tokio::test(start_paused = true)]
async fn startup_read_retries_transient_failure_without_accepting_cached_success() {
    let concrete = Arc::new(Source {
        responses: Mutex::new(vec![
            Err(AppError::Source),
            Ok(crate::platform::control::tests::snapshot()),
        ]),
        reads: AtomicUsize::new(0),
    });
    let source: Arc<dyn ConfigSource> = concrete.clone();
    assert!(initial_snapshot(&source).await.is_ok());
    assert_eq!(concrete.reads.load(Ordering::SeqCst), 2);
}
#[tokio::test(start_paused = true)]
async fn startup_invalid_snapshot_is_not_retried() {
    let concrete = Arc::new(Source {
        responses: Mutex::new(vec![Err(AppError::Configuration)]),
        reads: AtomicUsize::new(0),
    });
    let source: Arc<dyn ConfigSource> = concrete.clone();
    assert!(matches!(
        initial_snapshot(&source).await,
        Err(AppError::Configuration)
    ));
    assert_eq!(concrete.reads.load(Ordering::SeqCst), 1);
}
struct Unavailable(AtomicUsize);
impl ConfigSource for Unavailable {
    fn read(&self) -> SourceFuture<'_> {
        Box::pin(async {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(AppError::Source)
        })
    }
}
#[tokio::test(start_paused = true)]
async fn startup_transient_failure_exhausts_one_total_budget() {
    let concrete = Arc::new(Unavailable(AtomicUsize::new(0)));
    let source: Arc<dyn ConfigSource> = concrete.clone();
    let started = tokio::time::Instant::now();
    assert!(matches!(
        initial_snapshot(&source).await,
        Err(AppError::Source)
    ));
    assert_eq!(started.elapsed(), Duration::from_secs(30));
    assert!(concrete.0.load(Ordering::SeqCst) <= 30);
}
#[test]
fn probe_disabled_validates_runtime_alone_and_active_rejects_missing_policy() {
    let mut initial = crate::platform::control::tests::snapshot();
    initial.runtime = zeroize::Zeroizing::new(r#"{"schema_version":1,"is_enabled":false}"#.into());
    initial.secrets = None;
    initial.admission = None;
    initial.cleanup = None;
    assert!(validate_snapshot(&initial, crate::platform::config::ExpectedEnvironment::Boe).is_ok());
    let mut active = crate::platform::control::tests::snapshot();
    active.admission = None;
    assert!(matches!(
        validate_snapshot(&active, crate::platform::config::ExpectedEnvironment::Boe),
        Err(AppError::Configuration)
    ));
}
#[test]
fn active_probe_applies_provider_url_and_database_option_validation_without_io() {
    let ca = std::env::temp_dir().join(format!("ask-auth-probe-ca-{}", uuid::Uuid::now_v7()));
    std::fs::write(&ca, b"synthetic CA fixture; never connected").unwrap();
    let mut initial = crate::platform::control::tests::snapshot();
    let mut runtime: serde_json::Value = serde_json::from_str(&initial.runtime).unwrap();
    runtime["database_ca_path"] = ca.to_string_lossy().as_ref().into();
    initial.runtime = zeroize::Zeroizing::new(runtime.to_string());
    assert!(validate_snapshot(&initial, crate::platform::config::ExpectedEnvironment::Boe).is_ok());
    let mut scopes = vec!["offline_access".to_string()];
    scopes.extend((0..15).map(|index| format!("{index}{}", "[".repeat(248))));
    runtime["required_scopes"] = serde_json::json!(scopes);
    initial.runtime = zeroize::Zeroizing::new(runtime.to_string());
    assert!(settings::runtime(
        &initial.runtime,
        crate::platform::config::ExpectedEnvironment::Boe
    )
    .is_ok());
    assert_eq!(
        validate_snapshot(&initial, crate::platform::config::ExpectedEnvironment::Boe),
        Err(AppError::Configuration)
    );
    runtime["required_scopes"] = serde_json::json!(["offline_access"]);
    std::fs::remove_file(ca).unwrap();
    initial.runtime = zeroize::Zeroizing::new(runtime.to_string());
    assert_eq!(
        validate_snapshot(&initial, crate::platform::config::ExpectedEnvironment::Boe),
        Err(AppError::Configuration)
    );
}
