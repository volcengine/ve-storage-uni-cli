use crate::platform::{
    config::ExpectedEnvironment,
    control::{ConfigSource, HostSnapshot},
    settings::{self, AdmissionSettings, CleanupPolicy},
    AppError,
};
use std::{sync::Arc, time::Duration};
pub(crate) async fn initial_snapshot(
    source: &Arc<dyn ConfigSource>,
) -> Result<HostSnapshot, AppError> {
    // [Review Fix #77] Retry transient fresh reads only, within one startup deadline.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(AppError::Source);
            }
            let outcome = source.read().await;
            if tokio::time::Instant::now() >= deadline {
                return Err(AppError::Source);
            }
            match outcome {
                Err(AppError::Source) => tokio::time::sleep(Duration::from_secs(1)).await,
                outcome => return outcome,
            }
        }
    })
    .await
    .map_err(|_| AppError::Source)?
}
pub(crate) fn validate_snapshot(
    snapshot: &HostSnapshot,
    environment: ExpectedEnvironment,
) -> Result<(), AppError> {
    if let Some(active) = settings::runtime(&snapshot.runtime, environment)? {
        validate_controls(snapshot)?;
        // [Review Fix #86] Probe validates the same provider URL bounds, crypto and CA/DSN options before reporting success.
        crate::platform::factory::validate_active(active, snapshot)?;
    }
    Ok(())
}
pub(crate) fn validate_controls(snapshot: &HostSnapshot) -> Result<(), AppError> {
    AdmissionSettings::parse(settings::required(snapshot.admission.as_deref())?)?;
    CleanupPolicy::parse(settings::required(snapshot.cleanup.as_deref())?)?;
    Ok(())
}
#[cfg(test)]
mod tests;
