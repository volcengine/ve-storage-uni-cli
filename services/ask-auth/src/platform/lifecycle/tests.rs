use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[tokio::test(start_paused = true)]
async fn primary_restart_failure_survives_a_runtime_drain_timeout() {
    let (shutdown, _) = watch::channel(false);
    assert_eq!(
        manage(
            std::future::pending(),
            async { Err(AppError::RestartRequired) },
            std::future::pending(),
            shutdown,
            || {}
        )
        .await,
        Err(AppError::RestartRequired)
    );
}
struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
fn held(flag: Arc<AtomicBool>) -> impl Future<Output = Result<(), AppError>> {
    let guard = Dropped(flag);
    async move {
        let _guard = guard;
        std::future::pending().await
    }
}
#[tokio::test]
async fn publisher_panic_clears_policy_and_signals_owned_runtime() {
    let (shutdown, receiver) = watch::channel(false);
    let cleared = AtomicBool::new(false);
    let runtime = async {
        runtime_signal(receiver).await.unwrap();
        Ok(())
    };
    let publisher = async { panic!("synthetic private panic") };
    assert_eq!(
        manage(runtime, publisher, std::future::pending(), shutdown, || {
            cleared.store(true, Ordering::SeqCst);
        })
        .await,
        Err(AppError::Publisher)
    );
    assert!(cleared.load(Ordering::SeqCst));
}
#[tokio::test(start_paused = true)]
async fn shutdown_is_bounded_and_drops_both_unfinished_components() {
    let runtime_drop = Arc::new(AtomicBool::new(false));
    let publisher_drop = Arc::new(AtomicBool::new(false));
    let (shutdown, _) = watch::channel(false);
    assert_eq!(
        manage(
            held(runtime_drop.clone()),
            held(publisher_drop.clone()),
            async { Ok(()) },
            shutdown,
            || {}
        )
        .await,
        Err(AppError::Runtime)
    );
    assert!(runtime_drop.load(Ordering::SeqCst));
    assert!(publisher_drop.load(Ordering::SeqCst));
}
#[tokio::test]
async fn unpolled_manager_cancellation_drops_components_and_runtime_failures_propagate() {
    let runtime_drop = Arc::new(AtomicBool::new(false));
    let publisher_drop = Arc::new(AtomicBool::new(false));
    let (shutdown, _) = watch::channel(false);
    drop(manage(
        held(runtime_drop.clone()),
        held(publisher_drop.clone()),
        std::future::pending(),
        shutdown,
        || {},
    ));
    assert!(runtime_drop.load(Ordering::SeqCst));
    assert!(publisher_drop.load(Ordering::SeqCst));
    let (shutdown, mut receiver) = watch::channel(false);
    let publisher = async {
        stopped(&mut receiver).await;
        Ok(())
    };
    assert_eq!(
        manage(
            async { Err(AppError::Http) },
            publisher,
            std::future::pending(),
            shutdown,
            || {}
        )
        .await,
        Err(AppError::Http)
    );
}
#[tokio::test]
async fn signal_failure_still_drains_and_unexpected_worker_exit_fails() {
    let (shutdown, runtime_receiver) = watch::channel(false);
    let mut publisher_receiver = runtime_receiver.clone();
    let runtime = async {
        runtime_signal(runtime_receiver).await.unwrap();
        Ok(())
    };
    let publisher = async {
        stopped(&mut publisher_receiver).await;
        Ok(())
    };
    assert_eq!(
        manage(
            runtime,
            publisher,
            async { Err(AppError::Signal) },
            shutdown,
            || {}
        )
        .await,
        Err(AppError::Signal)
    );
    let (shutdown, runtime_receiver) = watch::channel(false);
    let runtime = async {
        runtime_signal(runtime_receiver).await.unwrap();
        Ok(())
    };
    assert_eq!(
        manage(
            runtime,
            async { Ok(()) },
            std::future::pending(),
            shutdown,
            || {}
        )
        .await,
        Err(AppError::Publisher)
    );
}
