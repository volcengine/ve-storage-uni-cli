use ask_auth::cleanup::{CleanupControl, CleanupSettings};
use std::time::Duration;

#[test]
fn settings_enforce_hard_batch_rate_and_deadline_limits() {
    assert!(CleanupSettings::new(
        Duration::from_secs(10),
        100,
        Duration::from_secs(5),
        Duration::from_secs(1),
        100
    )
    .is_ok());
    assert!(CleanupSettings::new(
        Duration::from_secs(10),
        501,
        Duration::from_secs(5),
        Duration::from_secs(1),
        100
    )
    .is_err());
    assert!(CleanupSettings::new(
        Duration::from_secs(10),
        100,
        Duration::from_secs(6),
        Duration::from_secs(1),
        100
    )
    .is_err());
    assert!(CleanupSettings::new(
        Duration::from_secs(10),
        100,
        Duration::from_secs(5),
        Duration::from_secs(2),
        100
    )
    .is_err());
    assert!(CleanupSettings::new(
        Duration::from_secs(10),
        100,
        Duration::from_secs(5),
        Duration::from_secs(1),
        101
    )
    .is_err());
}
#[test]
fn cleanup_control_is_initially_disabled_and_failure_closes_it() {
    let (control, source) = CleanupControl::channel();
    assert!(!control.is_enabled());
    source.send(Some(CleanupSettings::default())).unwrap();
    assert!(control.is_enabled());
    source.send(None).unwrap();
    assert!(!control.is_enabled());
    source.send(Some(CleanupSettings::default())).unwrap();
    drop(source);
    assert!(!control.is_enabled());
}
