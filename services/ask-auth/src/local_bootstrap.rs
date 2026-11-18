use ask_auth::{build_router, request_log_subscriber, Config};
use std::{future::IntoFuture, sync::Arc, time::Duration};

const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) async fn run() -> Result<(), &'static str> {
    let config =
        Config::from_environment().map_err(|_| "invalid authorization service configuration")?;
    tracing::subscriber::set_global_default(request_log_subscriber(
        config.log_level,
        std::io::stderr,
    ))
    .map_err(|_| "authorization service logging setup failed")?;
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|_| "authorization service could not bind listener")?;
    serve_until_shutdown(listener).await
}

async fn serve_until_shutdown(listener: tokio::net::TcpListener) -> Result<(), &'static str> {
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let shutdown_listener = shutdown.clone();
    let server = axum::serve(listener, build_router())
        .with_graceful_shutdown(async move { shutdown_listener.notified().await })
        .into_future();
    tokio::pin!(server);
    tokio::select! {
        outcome = &mut server => outcome.map_err(|_| "authorization service listener failed"),
        signal_result = wait_for_shutdown() => {
            shutdown.notify_one();
            // [Review Fix #4] Incomplete headers never enter Router middleware; bound the entire drain after shutdown.
            let outcome = tokio::time::timeout(SHUTDOWN_DRAIN_TIMEOUT, server).await
                .map_err(|_| "authorization service shutdown drain deadline exceeded")?;
            outcome.map_err(|_| "authorization service listener failed")?;
            signal_result.map_err(|_| "authorization service shutdown signal setup failed")
        }
    }
}

#[cfg(unix)]
async fn wait_for_shutdown() -> std::io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}
