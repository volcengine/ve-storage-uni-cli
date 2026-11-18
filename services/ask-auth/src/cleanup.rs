//! Injected, fail-closed cluster cleanup. No environment, provider or TCC SDK access.
use crate::storage::{CleanupCandidate, CleanupCursor, CleanupQueue, CleanupStore, StorageError};
use aes_gcm::aead::{rand_core::RngCore, OsRng};
use std::{collections::VecDeque, sync::Arc, time::Duration};
use tokio::{sync::watch, time::Instant};
use uuid::Uuid;

/// Validated operational bounds. Defaults require deployment load testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CleanupSettings {
    interval: Duration,
    batch: u16,
    pass_budget: Duration,
    transaction_budget: Duration,
    rows_per_second: u16,
}
impl CleanupSettings {
    /// Validate interval 1..300s, batch 1..500, pass <=5s, transaction <=1s,
    /// and affected business-row rate 2..100/s. Invalid bounds return Invalid.
    pub fn new(
        interval: Duration,
        batch: u16,
        pass_budget: Duration,
        transaction_budget: Duration,
        rows_per_second: u16,
    ) -> Result<Self, StorageError> {
        if !(Duration::from_secs(1)..=Duration::from_secs(300)).contains(&interval)
            || !(1..=500).contains(&batch)
            || pass_budget.is_zero()
            || pass_budget > Duration::from_secs(5)
            || transaction_budget.is_zero()
            || transaction_budget > Duration::from_secs(1)
            || transaction_budget > pass_budget
            || !(2..=100).contains(&rows_per_second)
        {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            interval,
            batch,
            pass_budget,
            transaction_budget,
            rows_per_second,
        })
    }
}
impl Default for CleanupSettings {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(10),
            batch: 100,
            pass_budget: Duration::from_secs(5),
            transaction_budget: Duration::from_secs(1),
            rows_per_second: 100,
        }
    }
}
/// Trusted configuration subscription. None, source failure or source closure pauses work.
/// The future TCC adapter must publish None on source failure; no stale fallback is allowed.
#[derive(Clone)]
pub struct CleanupControl {
    receiver: watch::Receiver<Option<CleanupSettings>>,
}
impl CleanupControl {
    /// Create an initially disabled control and its trusted publisher capability.
    /// Dropping the publisher disables work even when the last value was enabled.
    pub fn channel() -> (Self, watch::Sender<Option<CleanupSettings>>) {
        let (sender, receiver) = watch::channel(None);
        (Self { receiver }, sender)
    }
    /// Return whether a live trusted publisher currently enables validated settings.
    pub fn is_enabled(&self) -> bool {
        self.settings().is_some()
    }
    fn settings(&self) -> Option<CleanupSettings> {
        if self.receiver.has_changed().is_err() {
            None
        } else {
            *self.receiver.borrow()
        }
    }
}
/// Alert intent; delivery to TCE/metrics is owned by the injected observer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupAlert {
    None,
    BacklogWarning,
    BacklogPage,
    NoSuccessfulPass,
    Integrity,
}
/// Safe measured pass summary. No row IDs, secrets or user identity labels.
#[derive(Debug)]
pub struct CleanupReport {
    pub request_id: Uuid,
    pub duration: Duration,
    pub transaction_duration: Duration,
    pub affected_rows: u64,
    pub transactions: u64,
    pub lock_skips: u64,
    pub retries: u64,
    pub failures: u64,
    pub oldest_overdue: [Option<Duration>; 6],
    pub error: Option<StorageError>,
    pub alert: CleanupAlert,
    pub is_paused: bool,
}
impl CleanupReport {
    fn new() -> Self {
        Self {
            request_id: Uuid::now_v7(),
            duration: Duration::ZERO,
            transaction_duration: Duration::ZERO,
            affected_rows: 0,
            transactions: 0,
            lock_skips: 0,
            retries: 0,
            failures: 0,
            oldest_overdue: [None; 6],
            error: None,
            alert: CleanupAlert::None,
            is_paused: false,
        }
    }
}
/// Nonblocking trusted observer for sanitized reports; implementations must not perform blocking I/O.
pub trait CleanupObserver: Send + Sync {
    /// Receive measured results and alert intents. The observer must return promptly and not panic.
    fn observe(&self, report: &CleanupReport);
}
/// Default sanitized tracing observer; actual alert delivery remains deployment work.
pub struct TracingCleanupObserver;
impl CleanupObserver for TracingCleanupObserver {
    fn observe(&self, report: &CleanupReport) {
        // [Review Fix #48] Integrity/page intents must be visible at error level; backlog warnings use warn.
        match report.alert {
            CleanupAlert::Integrity
            | CleanupAlert::BacklogPage
            | CleanupAlert::NoSuccessfulPass => {
                tracing::error!(target: "ask_auth::cleanup", request_id=%report.request_id, alert=?report.alert, failures=report.failures, "cleanup alert");
            }
            CleanupAlert::BacklogWarning => {
                tracing::warn!(target: "ask_auth::cleanup", request_id=%report.request_id, alert=?report.alert, "cleanup alert");
            }
            CleanupAlert::None => {}
        }
        tracing::info!(target: "ask_auth::cleanup", request_id=%report.request_id, duration_ms=report.duration.as_millis() as u64,
            transaction_duration_ms=report.transaction_duration.as_millis() as u64, affected_rows=report.affected_rows,
            transactions=report.transactions, lock_skips=report.lock_skips, retries=report.retries,
            failures=report.failures, error=?report.error, alert=?report.alert, is_paused=report.is_paused, "cleanup pass");
        for (queue, age) in CleanupQueue::ALL.into_iter().zip(report.oldest_overdue) {
            tracing::info!(target: "ask_auth::cleanup", request_id=%report.request_id, queue=?queue, oldest_overdue_ms=age.map(|duration| duration.as_millis() as u64), "cleanup queue");
        }
    }
}
/// Serial worker, constructed only after a dedicated CleanupStore pool validates successfully.
/// Each replica creates one instance. It has no leader and never calls a provider.
pub struct CleanupWorker {
    store: Arc<CleanupStore>,
    control: CleanupControl,
    observer: Arc<dyn CleanupObserver>,
    cursors: [Option<CleanupCursor>; 6],
    scan_cycles: [u8; 6],
    next_queue: usize,
    rate: RowBudget,
    backlog: Backlog,
    failure_streak: u32,
    pending_retries: Vec<PendingRetry>,
}
impl CleanupWorker {
    pub(crate) fn runtime_store(&self) -> Arc<CleanupStore> {
        self.store.clone()
    }
    /// Inject validated storage, trusted live controls and a nonblocking report observer.
    /// Construction does no I/O; the dedicated writer pool is already validated.
    pub fn new(
        store: Arc<CleanupStore>,
        control: CleanupControl,
        observer: Arc<dyn CleanupObserver>,
    ) -> Self {
        Self {
            store,
            control,
            observer,
            cursors: std::array::from_fn(|_| None),
            scan_cycles: [0; 6],
            next_queue: 0,
            rate: RowBudget::default(),
            backlog: Backlog::new(),
            failure_streak: 0,
            pending_retries: Vec::new(),
        }
    }
    /// Run serial delayed passes with independent 0..3s cryptographic startup/tick jitter.
    /// A true or closed shutdown subscription stops new work and bounds current rollback/finish.
    /// Errors are sanitized and observed; closed configuration terminates the worker.
    pub async fn run(&mut self, mut shutdown: watch::Receiver<bool>) {
        while !is_shutdown(&shutdown) {
            let Some(settings) = self.control.settings() else {
                if !self.await_enabled(&mut shutdown).await {
                    return;
                }
                continue;
            };
            let jitter = match random_delay(3000) {
                Ok(delay) => delay,
                Err(error) => {
                    self.report_failure(error);
                    return;
                }
            };
            let backoff =
                Duration::from_secs((1u64 << self.failure_streak.min(5)).saturating_sub(1));
            if !self
                .wait(settings.interval + jitter + backoff, &mut shutdown)
                .await
            {
                continue;
            }
            let report = self.run_once(&mut shutdown).await;
            self.failure_streak = if report.error.is_some() {
                self.failure_streak.saturating_add(1)
            } else {
                0
            };
        }
    }
    /// Execute at most one bounded pass when control is enabled. Preserves rate reservations
    /// across calls. Shutdown/control changes are observed before each new transaction.
    /// Reports errors rather than replaying uncertain writes; no overlapping pass is possible.
    pub async fn run_once(&mut self, shutdown: &mut watch::Receiver<bool>) -> CleanupReport {
        let started = Instant::now();
        self.rate.finish(started);
        self.pending_retries.clear();
        let mut report = CleanupReport::new();
        let Some(settings) = self.control.settings() else {
            report.is_paused = true;
            self.observer.observe(&report);
            return report;
        };
        let deadline = started + settings.pass_budget;
        match tokio::time::timeout_at(
            deadline,
            self.execute_pass((settings, deadline), shutdown, &mut report),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                report.error = Some(error);
                report.failures += 1;
            }
            Err(_) => {
                // [Review Fix #44] A cancelled pass cannot count as a successful DB pass or free uncertain writes.
                self.cursors = std::array::from_fn(|_| None);
                report.error = Some(StorageError::Timeout);
                report.failures += 1;
            }
        }
        self.rate.finish(Instant::now());
        report.duration = started.elapsed();
        self.backlog.evaluate(&mut report);
        self.observer.observe(&report);
        report
    }
    async fn execute_pass(
        &mut self,
        limits: (CleanupSettings, Instant),
        shutdown: &mut watch::Receiver<bool>,
        report: &mut CleanupReport,
    ) -> Result<(), StorageError> {
        let (settings, deadline) = limits;
        if !self.can_continue(settings, shutdown) {
            report.is_paused = true;
            return Ok(());
        }
        let cutoff = self.store.cutoff().await?;
        self.probe_queues(cutoff, report).await?;
        let mut exhausted = [false; 6];
        while Instant::now() < deadline && exhausted.iter().any(|is_done| !is_done) {
            if !self.can_continue(settings, shutdown) {
                report.is_paused = true;
                break;
            }
            let index = self.next_queue;
            self.next_queue = (index + 1) % 6;
            if exhausted[index] {
                continue;
            }
            let queue = CleanupQueue::ALL[index];
            let batch = self
                .store
                .discover(queue, cutoff, self.cursors[index].as_ref(), settings.batch)
                .await?;
            if batch.is_empty() {
                self.cursors[index] = None;
                exhausted[index] = true;
                continue;
            }
            let has_budget = self
                .process_batch(batch, (index, settings, deadline), shutdown, report)
                .await?;
            self.scan_cycles[index] += 1;
            // [Review Fix #45] Continuous arrivals cannot postpone revisiting skipped index heads forever.
            if self.scan_cycles[index] >= 16 {
                self.cursors[index] = None;
                self.scan_cycles[index] = 0;
                exhausted[index] = true;
            }
            if !has_budget {
                break;
            }
        }
        Ok(())
    }
    async fn probe_queues(
        &self,
        cutoff: chrono::NaiveDateTime,
        report: &mut CleanupReport,
    ) -> Result<(), StorageError> {
        for (index, queue) in CleanupQueue::ALL.into_iter().enumerate() {
            report.oldest_overdue[index] = self.store.oldest_overdue(queue, cutoff).await?;
        }
        Ok(())
    }
    async fn process_batch(
        &mut self,
        batch: Vec<CleanupCandidate>,
        limits: (usize, CleanupSettings, Instant),
        shutdown: &mut watch::Receiver<bool>,
        report: &mut CleanupReport,
    ) -> Result<bool, StorageError> {
        let (index, settings, deadline) = limits;
        for candidate in batch {
            if !self.can_continue(settings, shutdown) || Instant::now() >= deadline {
                report.is_paused = true;
                return Ok(false);
            }
            if !self.permit((settings, deadline), shutdown).await {
                return Ok(false);
            }
            self.cursors[index] = Some(candidate.cursor());
            let result = self
                .process_candidate(&candidate, (settings.transaction_budget, deadline), report)
                .await;
            match result {
                Ok(outcome) => {
                    report.affected_rows += outcome.affected_rows;
                    report.lock_skips += outcome.lock_skips;
                    if outcome.lock_skips == 0 {
                        self.recover_retry(index, &candidate.cursor(), report);
                    }
                }
                Err(StorageError::Conflict | StorageError::NotFound) => {}
                Err(error @ (StorageError::Timeout | StorageError::Retryable))
                    if report.retries < 2 =>
                {
                    // [Review Fix #47] Failed retry attempts remain visible even when a later rediscovery succeeds.
                    report.failures += 1;
                    report.retries += 1;
                    // [Review Fix #50] Budget/control exits cannot disguise an unresolved failed transaction as success.
                    self.pending_retries.push(PendingRetry {
                        index,
                        cursor: candidate.cursor(),
                        error,
                    });
                    report.error = Some(error);
                    self.cursors[index] = None;
                    self.wait(random_delay(100)?, shutdown).await;
                    break; // Rediscover current metadata; never replay cached DML after uncertainty.
                }
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
    async fn process_candidate(
        &mut self,
        candidate: &CleanupCandidate,
        limits: (Duration, Instant),
        report: &mut CleanupReport,
    ) -> Result<crate::storage::CleanupOutcome, StorageError> {
        let started = Instant::now();
        let budget = limits.0.min(limits.1.saturating_duration_since(started));
        let result = self.store.process(candidate, budget).await;
        self.rate.finish(Instant::now());
        report.transactions += 1;
        report.transaction_duration += started.elapsed();
        result
    }
    fn recover_retry(&mut self, index: usize, cursor: &CleanupCursor, report: &mut CleanupReport) {
        self.pending_retries
            .retain(|pending| pending.index != index || pending.cursor != *cursor);
        report.error = self.pending_retries.first().map(|pending| pending.error);
    }
    fn can_continue(&self, settings: CleanupSettings, shutdown: &watch::Receiver<bool>) -> bool {
        !is_shutdown(shutdown) && self.control.settings() == Some(settings)
    }
    async fn permit(
        &mut self,
        limits: (CleanupSettings, Instant),
        shutdown: &mut watch::Receiver<bool>,
    ) -> bool {
        let (settings, deadline) = limits;
        loop {
            if !self.can_continue(settings, shutdown) || Instant::now() >= deadline {
                return false;
            }
            match self.rate.reserve(Instant::now(), settings.rows_per_second) {
                None => return true,
                Some(next) if next >= deadline => return false,
                Some(next) => {
                    if !self
                        .wait(next.saturating_duration_since(Instant::now()), shutdown)
                        .await
                    {
                        return false;
                    }
                }
            }
        }
    }
    async fn wait(&mut self, duration: Duration, shutdown: &mut watch::Receiver<bool>) -> bool {
        tokio::select! {
            biased;
            _ = shutdown.changed() => false,
            _ = self.control.receiver.changed() => false,
            _ = tokio::time::sleep(duration) => !is_shutdown(shutdown),
        }
    }
    async fn await_enabled(&mut self, shutdown: &mut watch::Receiver<bool>) -> bool {
        tokio::select! { biased; _ = shutdown.changed() => false, result = self.control.receiver.changed() => result.is_ok() }
    }
    fn report_failure(&self, error: StorageError) {
        let mut report = CleanupReport::new();
        report.error = Some(error);
        report.failures = 1;
        self.observer.observe(&report);
    }
}
struct PendingRetry {
    index: usize,
    cursor: CleanupCursor,
    error: StorageError,
}
#[derive(Default)]
struct RowBudget {
    reservations: VecDeque<Instant>,
    has_pending: bool,
}
impl RowBudget {
    fn finish(&mut self, now: Instant) {
        if self.has_pending {
            // [Review Fix #42] Commit/cancellation time anchors the rolling window, including unknown acknowledgements.
            self.reservations.pop_back();
            self.reservations.push_back(now);
            self.has_pending = false;
        }
    }
    fn reserve(&mut self, now: Instant, limit: u16) -> Option<Instant> {
        while self
            .reservations
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(1))
        {
            self.reservations.pop_front();
        }
        // [Review Fix #41] Reserve the maximum two business writes before opening any transaction.
        if (self.reservations.len() + 1) * 2 > usize::from(limit) {
            return self
                .reservations
                .front()
                .map(|at| *at + Duration::from_secs(1));
        }
        self.reservations.push_back(now);
        self.has_pending = true;
        None
    }
}
struct Backlog {
    overdue_since: [Option<Instant>; 6],
    success_anchor: Instant,
    has_known_work: bool,
}
impl Backlog {
    fn new() -> Self {
        Self {
            overdue_since: [None; 6],
            success_anchor: Instant::now(),
            has_known_work: false,
        }
    }
    fn evaluate(&mut self, report: &mut CleanupReport) {
        let now = Instant::now();
        if report.oldest_overdue.iter().any(Option::is_some) {
            self.has_known_work = true;
        }
        if report.error.is_none() && !report.is_paused {
            self.success_anchor = now;
            self.has_known_work = report.oldest_overdue.iter().any(Option::is_some);
        }
        for (index, age) in report.oldest_overdue.into_iter().enumerate() {
            if age.is_some_and(|age| age > Duration::from_secs(300)) {
                report.alert = CleanupAlert::BacklogPage;
            }
            if age.is_some_and(|age| age > Duration::from_secs(60)) {
                let since = self.overdue_since[index].get_or_insert(now);
                if now.duration_since(*since) >= Duration::from_secs(300)
                    && report.alert == CleanupAlert::None
                {
                    report.alert = CleanupAlert::BacklogWarning;
                }
            } else if report.error.is_none() {
                self.overdue_since[index] = None;
            }
        }
        if self.has_known_work
            && now.duration_since(self.success_anchor) >= Duration::from_secs(120)
        {
            report.alert = CleanupAlert::NoSuccessfulPass;
        }
        if report.error == Some(StorageError::Integrity) {
            report.alert = CleanupAlert::Integrity;
        }
    }
}
fn is_shutdown(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow() || shutdown.has_changed().is_err()
}
fn random_delay(maximum_millis: u32) -> Result<Duration, StorageError> {
    let bound = maximum_millis + 1;
    let ceiling = u32::MAX - u32::MAX % bound;
    loop {
        let mut bytes = [0; 4];
        OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(|_| StorageError::Unavailable)?;
        let sample = u32::from_le_bytes(bytes);
        if sample < ceiling {
            return Ok(Duration::from_millis(u64::from(sample % bound)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rate_window_retains_two_row_reservation_until_transaction_completion() {
        let started = Instant::now();
        let mut rate = RowBudget::default();
        assert_eq!(rate.reserve(started, 2), None);
        rate.finish(started + Duration::from_millis(900));
        assert_eq!(
            rate.reserve(started + Duration::from_secs(1), 2),
            Some(started + Duration::from_millis(1900))
        );
        assert_eq!(rate.reserve(started + Duration::from_millis(1900), 2), None);
    }
    #[test]
    fn rate_reserves_two_rows_for_unknown_commit_and_across_passes() {
        let started = Instant::now();
        let mut rate = RowBudget::default();
        for _ in 0..50 {
            assert_eq!(rate.reserve(started, 100), None);
            rate.finish(started);
        }
        assert_eq!(
            rate.reserve(started, 100),
            Some(started + Duration::from_secs(1))
        );
        assert_eq!(
            rate.reserve(started + Duration::from_millis(999), 100),
            Some(started + Duration::from_secs(1))
        );
    }
    #[tokio::test(start_paused = true)]
    async fn backlog_warn_page_and_integrity_thresholds_use_measured_time() {
        let mut backlog = Backlog::new();
        let mut report = CleanupReport::new();
        report.oldest_overdue[0] = Some(Duration::from_secs(61));
        backlog.evaluate(&mut report);
        assert_eq!(report.alert, CleanupAlert::None);
        tokio::time::advance(Duration::from_secs(300)).await;
        backlog.evaluate(&mut report);
        assert_eq!(report.alert, CleanupAlert::BacklogWarning);
        report.oldest_overdue[0] = Some(Duration::from_secs(301));
        backlog.evaluate(&mut report);
        assert_eq!(report.alert, CleanupAlert::BacklogPage);
        report.error = Some(StorageError::Integrity);
        backlog.evaluate(&mut report);
        assert_eq!(report.alert, CleanupAlert::Integrity);
    }
    #[tokio::test(start_paused = true)]
    async fn failed_pass_with_known_work_pages_after_two_minutes() {
        let mut backlog = Backlog::new();
        let mut report = CleanupReport::new();
        report.oldest_overdue[0] = Some(Duration::from_secs(1));
        backlog.evaluate(&mut report);
        report.error = Some(StorageError::Unavailable);
        tokio::time::advance(Duration::from_secs(120)).await;
        backlog.evaluate(&mut report);
        assert_eq!(report.alert, CleanupAlert::NoSuccessfulPass);
    }
}
