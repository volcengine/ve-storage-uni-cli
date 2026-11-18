use crate::platform::{
    config::ExpectedEnvironment,
    settings::{self, AdmissionSettings, CleanupPolicy},
    AppError, PayloadKey, Snapshot, TccSource,
};
use crate::{
    cleanup::CleanupSettings,
    login::{AdmissionError, AdmissionFuture, AdmissionProvider, AdmissionSnapshot},
    storage::CleanupStore,
};
use chrono::NaiveDateTime;
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;
use zeroize::Zeroizing;

pub(crate) type SourceFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HostSnapshot, AppError>> + Send + 'a>>;
pub(crate) trait ConfigSource: Send + Sync {
    fn read(&self) -> SourceFuture<'_>;
}
impl ConfigSource for TccSource {
    fn read(&self) -> SourceFuture<'_> {
        Box::pin(async move {
            self.read_snapshot()
                .await
                .map(HostSnapshot::from)
                .map_err(source_error)
        })
    }
}
pub(crate) fn source_error(error: crate::platform::TccError) -> AppError {
    // [Review Fix #76] Malformed snapshots and environment mismatches must never enter transient startup retries.
    match error {
        crate::platform::TccError::Identity
        | crate::platform::TccError::Unavailable
        | crate::platform::TccError::Timeout => AppError::Source,
        _ => AppError::Configuration,
    }
}
pub(crate) struct HostSnapshot {
    pub(crate) runtime: Zeroizing<String>,
    pub(crate) secrets: Option<Zeroizing<String>>,
    pub(crate) admission: Option<Zeroizing<String>>,
    pub(crate) cleanup: Option<Zeroizing<String>>,
    pub(crate) version: String,
    pub(crate) completed: Instant,
}
impl From<Snapshot> for HostSnapshot {
    fn from(snapshot: Snapshot) -> Self {
        let copy = |key| {
            snapshot
                .required(key)
                .ok()
                .map(|text| Zeroizing::new(text.to_owned()))
        };
        Self {
            runtime: copy(PayloadKey::Runtime).unwrap_or_default(),
            secrets: copy(PayloadKey::Secrets),
            admission: copy(PayloadKey::Admission),
            cleanup: copy(PayloadKey::Cleanup),
            version: snapshot.version().into(),
            completed: snapshot.read_completed_at(),
        }
    }
}
pub(crate) struct Fingerprint(Zeroizing<[u8; 32]>);
impl Fingerprint {
    pub(crate) fn capture(snapshot: &HostSnapshot) -> Self {
        let mut digest = Sha256::new();
        for payload in [
            Some(snapshot.runtime.as_str()),
            snapshot.secrets.as_deref().map(String::as_str),
        ] {
            digest.update([u8::from(payload.is_some())]);
            let bytes = payload.unwrap_or_default().as_bytes();
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
        }
        Self(Zeroizing::new(digest.finalize().into()))
    }
    pub(crate) fn matches(&self, snapshot: &HostSnapshot) -> bool {
        *self.0 == *Self::capture(snapshot).0
    }
}
#[derive(Clone)]
pub(crate) struct LiveAdmission {
    receiver: watch::Receiver<Option<AdmissionSnapshot>>,
}
impl LiveAdmission {
    pub(crate) fn channel() -> (Arc<Self>, watch::Sender<Option<AdmissionSnapshot>>) {
        let (sender, receiver) = watch::channel(None);
        (Arc::new(Self { receiver }), sender)
    }
}
impl AdmissionProvider for LiveAdmission {
    fn snapshot(&self) -> AdmissionFuture<'_> {
        Box::pin(async move {
            self.receiver.has_changed().map_err(|_| AdmissionError)?;
            let snapshot = self
                .receiver
                .borrow()
                .as_ref()
                .map(copy_policy)
                .ok_or(AdmissionError)?;
            self.receiver.has_changed().map_err(|_| AdmissionError)?;
            Ok(snapshot)
        })
    }
}
fn copy_policy(snapshot: &AdmissionSnapshot) -> AdmissionSnapshot {
    AdmissionSnapshot {
        is_enabled: snapshot.is_enabled,
        version: snapshot.version.clone(),
        fetched_at: snapshot.fetched_at,
        allowed_users: snapshot.allowed_users.clone(),
        allowed_tenants: snapshot.allowed_tenants.clone(),
        denied_users: snapshot.denied_users.clone(),
        denied_tenants: snapshot.denied_tenants.clone(),
    }
}
type ClockFuture<'a> = Pin<Box<dyn Future<Output = Result<NaiveDateTime, AppError>> + Send + 'a>>;
pub(crate) trait PolicyClock: Send + Sync {
    fn now(&self) -> ClockFuture<'_>;
}
impl PolicyClock for CleanupStore {
    fn now(&self) -> ClockFuture<'_> {
        Box::pin(async { self.cutoff().await.map_err(|_| AppError::Database) })
    }
}
pub(crate) struct Controls {
    pub(crate) admission: watch::Sender<Option<AdmissionSnapshot>>,
    pub(crate) cleanup: watch::Sender<Option<CleanupSettings>>,
}
impl Controls {
    pub(crate) fn clear(&self) {
        self.admission.send_replace(None);
        self.cleanup.send_replace(None);
    }
    async fn publish(
        &self,
        snapshot: &HostSnapshot,
        clock: &dyn PolicyClock,
    ) -> Result<(), AppError> {
        let admission =
            AdmissionSettings::parse(settings::required(snapshot.admission.as_deref())?)?;
        let cleanup = CleanupPolicy::parse(settings::required(snapshot.cleanup.as_deref())?)?;
        let now = clock.now().await?;
        let fetched_at = policy_timestamp(now, snapshot.completed)?;
        if !settings::text(&snapshot.version, 256) {
            return Err(AppError::Configuration);
        }
        self.admission.send_replace(Some(
            admission.snapshot(snapshot.version.clone(), fetched_at),
        ));
        // [Review Fix #88] Equal refreshes must not reset the cleanup worker's interval; failures still clear unconditionally.
        self.cleanup.send_if_modified(|current| {
            if *current == cleanup {
                false
            } else {
                *current = cleanup;
                true
            }
        });
        Ok(())
    }
}
impl Drop for Controls {
    fn drop(&mut self) {
        self.clear();
    }
}
fn policy_timestamp(now: NaiveDateTime, completed: Instant) -> Result<NaiveDateTime, AppError> {
    let elapsed = Instant::now()
        .checked_duration_since(completed)
        .ok_or(AppError::Configuration)?;
    if elapsed > Duration::from_secs(30) {
        return Err(AppError::Configuration);
    }
    // [Review Fix #73] Subtract rounded-up monotonic age from DB UTC; host NTP cannot grant future freshness.
    let millis =
        i64::try_from(elapsed.as_millis() + u128::from(elapsed.subsec_nanos() % 1_000_000 != 0))
            .map_err(|_| AppError::Configuration)?;
    let now = chrono::DateTime::from_timestamp_millis(now.and_utc().timestamp_millis())
        .ok_or(AppError::Configuration)?
        .naive_utc();
    now.checked_sub_signed(chrono::Duration::milliseconds(millis))
        .ok_or(AppError::Configuration)
}
pub(crate) struct Publisher {
    pub(crate) source: Arc<dyn ConfigSource>,
    pub(crate) clock: Arc<dyn PolicyClock>,
    pub(crate) controls: Controls,
    pub(crate) fingerprint: Fingerprint,
    pub(crate) environment: ExpectedEnvironment,
}
impl Publisher {
    pub(crate) async fn seed(&self, snapshot: &HostSnapshot) -> Result<(), AppError> {
        let outcome = self.controls.publish(snapshot, self.clock.as_ref()).await;
        if outcome.is_err() {
            self.controls.clear();
        }
        outcome
    }
    pub(crate) async fn refresh(&self) -> Result<(), AppError> {
        let result = self.read_and_publish().await;
        if let Err(error) = result {
            self.controls.clear();
            event(
                if error == AppError::RestartRequired {
                    "restart_required"
                } else {
                    "unavailable"
                },
                false,
            );
            if error == AppError::RestartRequired {
                return Err(error);
            }
        } else {
            event("current", true);
        }
        Ok(())
    }
    async fn read_and_publish(&self) -> Result<(), AppError> {
        let snapshot = self.source.read().await?;
        if !self.fingerprint.matches(&snapshot) {
            return Err(AppError::RestartRequired);
        }
        if settings::runtime(&snapshot.runtime, self.environment)?.is_none() {
            return Err(AppError::RestartRequired);
        }
        self.controls.publish(&snapshot, self.clock.as_ref()).await
    }
    pub(crate) async fn run(&self, mut shutdown: watch::Receiver<bool>) -> Result<(), AppError> {
        loop {
            tokio::select! { biased;
                _=stopped(&mut shutdown) => return Ok(()),
                _=tokio::time::sleep(Duration::from_secs(15)) => {}
            }
            tokio::select! { biased;
                _=stopped(&mut shutdown) => return Ok(()),
                result=self.refresh() => result?,
            }
        }
    }
}
pub(crate) async fn stopped(shutdown: &mut watch::Receiver<bool>) {
    while !*shutdown.borrow_and_update() {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}
pub(crate) fn event(status: &'static str, is_enabled: bool) {
    // [Review Fix #106] Keep the fixed allowlisted log target unchanged when moving Rust module paths.
    tracing::info!(target:"ask_auth::config",request_id=%uuid::Uuid::now_v7(),status,is_enabled,"configuration state");
}
#[cfg(test)]
pub(crate) mod tests;
