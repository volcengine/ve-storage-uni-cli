use super::*;
use std::sync::Mutex;
#[tokio::test]
async fn equal_cleanup_refresh_keeps_worker_timer_and_changes_still_notify() {
    let initial = snapshot();
    let (publisher, _admission, _cleanup) = publisher(&initial, vec![Ok(snapshot())]);
    let mut subscription = publisher.controls.cleanup.subscribe();
    publisher.seed(&initial).await.unwrap();
    subscription.borrow_and_update();
    publisher.refresh().await.unwrap();
    assert!(
        !subscription.has_changed().unwrap(),
        "equal cleanup refresh restarted the worker timer"
    );
    let mut changed = snapshot();
    changed.cleanup.as_mut().unwrap().push(' ');
    publisher.seed(&changed).await.unwrap();
    assert!(
        !subscription.has_changed().unwrap(),
        "formatting alone changed typed cleanup settings"
    );
    changed.cleanup = Some(Zeroizing::new(
        changed
            .cleanup
            .as_ref()
            .unwrap()
            .replace("\"batch\":100", "\"batch\":101"),
    ));
    publisher.seed(&changed).await.unwrap();
    assert!(subscription.has_changed().unwrap());
    subscription.borrow_and_update();
    publisher.controls.clear();
    assert!(subscription.has_changed().unwrap());
    subscription.borrow_and_update();
    publisher.seed(&initial).await.unwrap();
    assert!(subscription.has_changed().unwrap());
}
#[tokio::test]
async fn database_clock_failure_revokes_policy_and_recovers_on_a_fresh_tick() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct ClockState(AtomicBool);
    impl PolicyClock for ClockState {
        fn now(&self) -> ClockFuture<'_> {
            Box::pin(async {
                if self.0.load(Ordering::SeqCst) {
                    Err(AppError::Database)
                } else {
                    Clock.now().await
                }
            })
        }
    }
    let initial = snapshot();
    let (mut publisher, admission, cleanup) =
        publisher(&initial, vec![Ok(snapshot()), Ok(snapshot())]);
    let clock = Arc::new(ClockState(AtomicBool::new(false)));
    publisher.clock = clock.clone();
    publisher.seed(&initial).await.unwrap();
    clock.0.store(true, Ordering::SeqCst);
    publisher.refresh().await.unwrap();
    assert!(admission.snapshot().await.is_err());
    assert!(!cleanup.is_enabled());
    clock.0.store(false, Ordering::SeqCst);
    publisher.refresh().await.unwrap();
    assert!(admission.snapshot().await.is_ok());
    assert!(cleanup.is_enabled());
}
#[test]
fn only_transient_sdk_errors_are_retryable() {
    for failure in [
        crate::platform::TccError::Identity,
        crate::platform::TccError::Unavailable,
        crate::platform::TccError::Timeout,
    ] {
        assert_eq!(source_error(failure), AppError::Source);
    }
    for failure in [
        crate::platform::TccError::InvalidLocator,
        crate::platform::TccError::Environment,
        crate::platform::TccError::InvalidSnapshot,
        crate::platform::TccError::MissingConfiguration,
    ] {
        assert_eq!(source_error(failure), AppError::Configuration);
    }
}

pub(crate) fn snapshot() -> HostSnapshot {
    HostSnapshot { runtime:Zeroizing::new(settings::tests::runtime()),secrets:Some(Zeroizing::new(settings::tests::secrets())),
        admission:Some(Zeroizing::new(r#"{"schema_version":1,"is_enabled":true,"allowed_users":["verified-open-id"],"allowed_tenants":[],"denied_users":[],"denied_tenants":[]}"#.into())),
        cleanup:Some(Zeroizing::new(r#"{"schema_version":1,"is_enabled":true,"interval_seconds":10,"batch":100,"pass_budget_ms":5000,"transaction_budget_ms":1000,"rows_per_second":100}"#.into())),
        version:"synthetic-version".into(),completed:Instant::now() }
}
struct Source(Mutex<Vec<Result<HostSnapshot, AppError>>>);
impl ConfigSource for Source {
    fn read(&self) -> SourceFuture<'_> {
        Box::pin(async { self.0.lock().unwrap().remove(0) })
    }
}
struct Clock;
impl PolicyClock for Clock {
    fn now(&self) -> ClockFuture<'_> {
        Box::pin(async {
            Ok(chrono::DateTime::from_timestamp_millis(1_800_000_000_000)
                .unwrap()
                .naive_utc())
        })
    }
}
fn publisher(
    initial: &HostSnapshot,
    reads: Vec<Result<HostSnapshot, AppError>>,
) -> (
    Publisher,
    Arc<LiveAdmission>,
    crate::cleanup::CleanupControl,
) {
    let (admission, sender) = LiveAdmission::channel();
    let (cleanup, cleanup_sender) = crate::cleanup::CleanupControl::channel();
    (
        Publisher {
            source: Arc::new(Source(Mutex::new(reads))),
            clock: Arc::new(Clock),
            controls: Controls {
                admission: sender,
                cleanup: cleanup_sender,
            },
            fingerprint: Fingerprint::capture(initial),
            environment: ExpectedEnvironment::Boe,
        },
        admission,
        cleanup,
    )
}
#[tokio::test]
async fn live_controls_fail_closed_until_valid_publication() {
    let initial = snapshot();
    let (publisher, admission, cleanup) = publisher(&initial, vec![]);
    assert!(admission.snapshot().await.is_err());
    assert!(!cleanup.is_enabled());
    publisher.seed(&initial).await.unwrap();
    let policy = admission.snapshot().await.unwrap();
    assert!(policy.allowed_users.contains("verified-open-id"));
    assert!(cleanup.is_enabled());
    assert!(policy.fetched_at <= Clock.now().await.unwrap());
    assert!(!policy.is_fresh(policy.fetched_at + chrono::Duration::minutes(6)));
    drop(publisher);
    assert!(admission.snapshot().await.is_err());
    assert!(!cleanup.is_enabled());
}
#[tokio::test]
async fn failed_source_clears_both_controls_and_next_snapshot_recovers() {
    let initial = snapshot();
    let (publisher, admission, cleanup) =
        publisher(&initial, vec![Err(AppError::Source), Ok(snapshot())]);
    publisher.seed(&initial).await.unwrap();
    publisher.refresh().await.unwrap();
    assert!(admission.snapshot().await.is_err());
    assert!(!cleanup.is_enabled());
    publisher.refresh().await.unwrap();
    assert!(admission.snapshot().await.is_ok());
    assert!(cleanup.is_enabled());
}
#[tokio::test]
async fn immutable_raw_change_demands_restart_and_clears_controls() {
    let initial = snapshot();
    let mut changed = snapshot();
    changed.runtime.push(' ');
    let (publisher, admission, cleanup) = publisher(&initial, vec![Ok(changed)]);
    publisher.seed(&initial).await.unwrap();
    assert_eq!(publisher.refresh().await, Err(AppError::RestartRequired));
    assert!(admission.snapshot().await.is_err());
    assert!(!cleanup.is_enabled());
}
#[tokio::test]
async fn invalid_policy_and_stale_snapshot_cannot_publish() {
    let initial = snapshot();
    let mut changed = snapshot();
    changed.completed -= Duration::from_secs(31);
    let (publisher, admission, cleanup) = publisher(&initial, vec![Ok(changed)]);
    publisher.seed(&initial).await.unwrap();
    publisher.refresh().await.unwrap();
    assert!(admission.snapshot().await.is_err());
    assert!(!cleanup.is_enabled());
    let mut invalid = snapshot();
    invalid.admission = Some(Zeroizing::new("[1,true,[],[],[],[]]".into()));
    assert!(publisher.seed(&invalid).await.is_err());
}
#[test]
fn policy_time_uses_database_clock_and_checks_monotonic_age() {
    let now = chrono::DateTime::from_timestamp_millis(1_800_000_000_000)
        .unwrap()
        .naive_utc();
    let value = policy_timestamp(now, Instant::now() - Duration::from_millis(250)).unwrap();
    assert!(value <= now - chrono::Duration::milliseconds(250));
    assert!(value
        .and_utc()
        .timestamp_subsec_nanos()
        .is_multiple_of(1_000_000));
    assert!(policy_timestamp(now, Instant::now() + Duration::from_secs(1)).is_err());
}
