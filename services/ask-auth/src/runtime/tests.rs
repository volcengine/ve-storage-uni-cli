use super::*;
mod binding;
mod http;
use std::{
    future::{pending, ready},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

struct DropProbe(Arc<AtomicBool>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

async fn until_shutdown(mut shutdown: watch::Receiver<bool>) -> Result<(), RuntimeError> {
    if !*shutdown.borrow() {
        shutdown.changed().await.unwrap();
    }
    Ok(())
}

#[tokio::test]
async fn graceful_idle_shutdown_signals_both_components_and_closes() {
    let (shutdown, receiver) = watch::channel(false);
    let is_closed = Arc::new(AtomicBool::new(false));
    let closed = is_closed.clone();
    let outcome = supervise(
        until_shutdown(receiver.clone()),
        until_shutdown(receiver),
        ready(Ok(())),
        shutdown,
        async move {
            closed.store(true, Ordering::SeqCst);
        },
    )
    .await;
    assert_eq!(outcome, Ok(()));
    assert!(is_closed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn unexpected_cleanup_exit_stops_http_and_closes() {
    let (shutdown, receiver) = watch::channel(false);
    let outcome = supervise(
        until_shutdown(receiver),
        ready(Ok(())),
        pending(),
        shutdown,
        ready(()),
    )
    .await;
    assert_eq!(outcome, Err(RuntimeError::CleanupExited));
}

#[tokio::test]
async fn cleanup_panic_is_sanitized_and_http_drains() {
    let (shutdown, receiver) = watch::channel(false);
    let worker = async {
        panic!("PRIVATE_WORKER_PANIC");
    };
    let outcome = supervise(
        until_shutdown(receiver),
        worker,
        pending(),
        shutdown,
        ready(()),
    )
    .await;
    assert_eq!(outcome, Err(RuntimeError::CleanupPanicked));
    assert!(!format!("{outcome:?}").contains("PRIVATE_WORKER_PANIC"));
}

#[tokio::test]
async fn http_failure_signals_cleanup_and_preserves_fixed_error() {
    let (shutdown, receiver) = watch::channel(false);
    let outcome = supervise(
        ready(Err(RuntimeError::Http)),
        until_shutdown(receiver),
        pending(),
        shutdown,
        ready(()),
    )
    .await;
    assert_eq!(outcome, Err(RuntimeError::Http));
}

#[tokio::test]
async fn http_early_success_is_unexpected_without_a_signal() {
    let (shutdown, receiver) = watch::channel(false);
    let outcome = supervise(
        ready(Ok(())),
        until_shutdown(receiver),
        pending(),
        shutdown,
        ready(()),
    )
    .await;
    assert_eq!(outcome, Err(RuntimeError::HttpExited));
}

#[tokio::test]
async fn simultaneous_clean_completion_prefers_shutdown_signal() {
    let (shutdown, _) = watch::channel(false);
    let outcome = supervise(
        ready(Ok(())),
        ready(Ok(())),
        ready(Ok(())),
        shutdown,
        ready(()),
    )
    .await;
    assert_eq!(outcome, Ok(()));
}

#[tokio::test]
async fn simultaneous_component_failure_prefers_cleanup_before_http() {
    let (shutdown, _) = watch::channel(false);
    let outcome = supervise(
        ready(Err(RuntimeError::Http)),
        ready(Ok(())),
        pending(),
        shutdown,
        ready(()),
    )
    .await;
    assert_eq!(outcome, Err(RuntimeError::CleanupExited));
}

#[tokio::test]
async fn signal_failure_is_sanitized() {
    let (shutdown, receiver) = watch::channel(false);
    let signal = ready(Err(std::io::Error::other("PRIVATE_SIGNAL_ERROR")));
    let outcome = supervise(
        until_shutdown(receiver.clone()),
        until_shutdown(receiver),
        signal,
        shutdown,
        ready(()),
    )
    .await;
    assert_eq!(outcome, Err(RuntimeError::Signal));
    assert_eq!(
        outcome.unwrap_err().to_string(),
        "authorization runtime signal failed"
    );
}

#[tokio::test(start_paused = true)]
async fn parallel_drain_and_pool_close_have_separate_bounded_budgets() {
    let (shutdown, _) = watch::channel(false);
    let started = tokio::time::Instant::now();
    let outcome = supervise(pending(), pending(), ready(Ok(())), shutdown, pending()).await;
    assert_eq!(outcome, Err(RuntimeError::DrainTimeout));
    assert_eq!(started.elapsed(), DRAIN_BUDGET + CLOSE_BUDGET);
}

#[tokio::test(start_paused = true)]
async fn close_timeout_is_reported_after_successful_parallel_drain() {
    let (shutdown, receiver) = watch::channel(false);
    let started = tokio::time::Instant::now();
    let outcome = supervise(
        until_shutdown(receiver.clone()),
        until_shutdown(receiver),
        ready(Ok(())),
        shutdown,
        pending(),
    )
    .await;
    assert_eq!(outcome, Err(RuntimeError::CloseTimeout));
    assert_eq!(started.elapsed(), CLOSE_BUDGET);
}

#[tokio::test(start_paused = true)]
async fn cleanup_panic_during_drain_is_preserved_when_http_exceeds_the_deadline() {
    let (shutdown, _) = watch::channel(false);
    let worker = async {
        panic!("PRIVATE_DRAIN_PANIC");
    };
    let outcome = supervise(pending(), worker, ready(Ok(())), shutdown, ready(())).await;
    assert_eq!(outcome, Err(RuntimeError::CleanupPanicked));
}

#[tokio::test]
async fn cancelling_serve_drops_components_without_detached_work() {
    let (shutdown, _) = watch::channel(false);
    let is_http_dropped = Arc::new(AtomicBool::new(false));
    let is_worker_dropped = Arc::new(AtomicBool::new(false));
    let http_probe = DropProbe(is_http_dropped.clone());
    let worker_probe = DropProbe(is_worker_dropped.clone());
    let http = async move {
        let _probe = http_probe;
        pending::<Result<(), RuntimeError>>().await
    };
    let worker = async move {
        let _probe = worker_probe;
        pending::<Result<(), RuntimeError>>().await
    };
    let mut serving = Box::pin(supervise(http, worker, pending(), shutdown, ready(())));
    assert!(futures::poll!(&mut serving).is_pending());
    drop(serving);
    assert!(is_http_dropped.load(Ordering::SeqCst));
    assert!(is_worker_dropped.load(Ordering::SeqCst));
}
