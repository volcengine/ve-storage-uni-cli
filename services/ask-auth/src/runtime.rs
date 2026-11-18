//! Explicitly injected HTTP/1 runtime with owned, bounded cleanup and connection supervision.
use crate::{
    cleanup::CleanupWorker,
    login::LoginService,
    storage::{CleanupStore, Store},
};
use axum::Router;
use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
use hyper::server::conn::http1;
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    service::TowerToHyperService,
};
use std::{future::Future, io, panic::AssertUnwindSafe, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
};

const DRAIN_BUDGET: Duration = Duration::from_secs(5);
const CLOSE_BUDGET: Duration = Duration::from_secs(3);
const MAX_CONNECTIONS: usize = 256;
const MAX_HEADERS: usize = 100;
const MAX_HEADER_BUFFER: usize = 32 * 1024;

/// Sanitized terminal failure. No source error, panic payload or request value is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeError {
    /// Injected components do not share the exact validated app/deployment/crypto binding.
    InvalidBinding,
    /// The HTTP listener failed.
    Http,
    /// HTTP serving terminated before a shutdown request.
    HttpExited,
    /// An HTTP serving future panicked.
    HttpPanicked,
    /// Cleanup returned before a shutdown request.
    CleanupExited,
    /// Cleanup panicked.
    CleanupPanicked,
    /// The injected shutdown-signal future failed.
    Signal,
    /// HTTP and cleanup did not both drain within five seconds.
    DrainTimeout,
    /// The exact API and cleanup pools did not both close within three additional seconds.
    CloseTimeout,
}
impl std::fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidBinding => "authorization runtime binding invalid",
            Self::Http => "authorization runtime HTTP failed",
            Self::HttpExited => "authorization runtime HTTP exited unexpectedly",
            Self::HttpPanicked => "authorization runtime HTTP panicked",
            Self::CleanupExited => "authorization runtime cleanup exited unexpectedly",
            Self::CleanupPanicked => "authorization runtime cleanup panicked",
            Self::Signal => "authorization runtime signal failed",
            Self::DrainTimeout => "authorization runtime drain deadline exceeded",
            Self::CloseTimeout => "authorization runtime pool close deadline exceeded",
        })
    }
}
impl std::error::Error for RuntimeError {}

/// Owns one explicitly enabled HTTP application and its dedicated cleanup worker.
/// No environment, platform SDK, signal registration or database connection is created here.
pub struct AuthorizationRuntime {
    service: Arc<LoginService>,
    worker: CleanupWorker,
    resources: Resources,
}
impl AuthorizationRuntime {
    /// Validate injected components and take ownership without I/O or starting work.
    /// Returns InvalidBinding unless cleanup uses the exact LoginService crypto capability;
    /// LoginService already validates its app, deployment, key and API Store binding.
    /// Rejection permanently closes both component pools, including retained external handles.
    /// Dropping a successfully constructed runtime also closes both pools without starting work.
    pub fn new(service: LoginService, worker: CleanupWorker) -> Result<Self, RuntimeError> {
        // [Review Fix #53] Own the close guard before serving, including unused runtimes and unpolled futures.
        let resources = Resources {
            api: service.runtime_store(),
            cleanup: worker.runtime_store(),
        };
        if !resources.cleanup.matches_crypto(service.runtime_crypto()) {
            // [Review Fix #51] Rejection drops the same guard even when external Arc owners retain either store.
            return Err(RuntimeError::InvalidBinding);
        }
        Ok(Self {
            service: Arc::new(service),
            worker,
            resources,
        })
    }
    /// Serve a caller-bound HTTP/1 TCP listener until the injected signal or a component exits.
    /// The signal returns io::Result<()>; failures are reduced to Signal. TLS terminates upstream.
    /// Stop acceptance, signal cleanup, drain both in parallel for 5s, then drop unfinished work
    /// and close the exact owned pools concurrently within 3s. Errors contain fixed classes only.
    /// Cancellation immediately drops connections/handlers/cleanup and permanently closes pools;
    /// this includes dropping the returned future before its first poll.
    /// asynchronous connection-close acknowledgements require awaiting this method to completion.
    pub async fn serve(
        self,
        listener: TcpListener,
        signal: impl Future<Output = io::Result<()>>,
    ) -> Result<(), RuntimeError> {
        let Self {
            service,
            mut worker,
            resources,
        } = self;
        let (shutdown, receiver) = watch::channel(false);
        let http = serve_http(
            listener,
            crate::build_enabled_router(service),
            receiver.clone(),
        );
        let cleanup = async move {
            worker.run(receiver).await;
            Ok(())
        };
        supervise(http, cleanup, signal, shutdown, resources.close()).await
    }
}

/// Serve the disabled router through the same owned, bounded HTTP/1 supervisor.
/// The listener is caller-bound; signal errors are sanitized. No database or provider is created.
/// Shutdown drains for at most five seconds; cancellation drops all owned connections.
pub async fn serve_disabled(
    listener: TcpListener,
    signal: impl Future<Output = io::Result<()>>,
) -> Result<(), RuntimeError> {
    let (shutdown, receiver) = watch::channel(false);
    let mut idle_shutdown = receiver.clone();
    let idle = async move {
        stopped(&mut idle_shutdown).await;
        Ok(())
    };
    supervise(
        serve_http(listener, crate::build_router(), receiver),
        idle,
        signal,
        shutdown,
        std::future::ready(()),
    )
    .await
}

struct Resources {
    api: Arc<Store>,
    cleanup: Arc<CleanupStore>,
}
impl Resources {
    async fn close(&self) {
        tokio::join!(self.api.close(), self.cleanup.close());
    }
}
impl Drop for Resources {
    fn drop(&mut self) {
        // SQLx marks closed synchronously; cancellation must reject acquisitions without spawning.
        drop(self.api.close());
        drop(self.cleanup.close());
    }
}

async fn supervise(
    http: impl Future<Output = Result<(), RuntimeError>>,
    worker: impl Future<Output = Result<(), RuntimeError>>,
    signal: impl Future<Output = io::Result<()>>,
    shutdown: watch::Sender<bool>,
    close: impl Future<Output = ()>,
) -> Result<(), RuntimeError> {
    let outcome = run_components(http, worker, signal, shutdown).await;
    let closure = tokio::time::timeout(CLOSE_BUDGET, close)
        .await
        .map_err(|_| RuntimeError::CloseTimeout);
    outcome.and(closure)
}

async fn run_components(
    http: impl Future<Output = Result<(), RuntimeError>>,
    worker: impl Future<Output = Result<(), RuntimeError>>,
    signal: impl Future<Output = io::Result<()>>,
    shutdown: watch::Sender<bool>,
) -> Result<(), RuntimeError> {
    let http = catch_component(http, RuntimeError::HttpPanicked);
    let worker = catch_component(worker, RuntimeError::CleanupPanicked);
    tokio::pin!(http, worker, signal);
    // Stable precedence for simultaneous readiness: signal, cleanup, HTTP.
    let (is_http_done, is_worker_done, outcome) = tokio::select! {
        biased;
        result = &mut signal => (false, false, result.map_err(|_| RuntimeError::Signal)),
        result = &mut worker => (false, true, result.and(Err(RuntimeError::CleanupExited))),
        result = &mut http => (true, false, result.and(Err(RuntimeError::HttpExited))),
    };
    shutdown.send_replace(true);
    let (mut http_failure, mut worker_failure) = (None, None);
    let drain = async {
        tokio::join!(
            drain_component(http, is_http_done, &mut http_failure),
            drain_component(worker, is_worker_done, &mut worker_failure),
        );
    };
    let drained = tokio::time::timeout(DRAIN_BUDGET, drain).await;
    // [Review Fix #52] Preserve observed component failures even if the other component exceeds its drain.
    let failure = worker_failure
        .or(http_failure)
        .or_else(|| drained.err().map(|_| RuntimeError::DrainTimeout));
    outcome.and(failure.map_or(Ok(()), Err))
}

async fn drain_component(
    component: impl Future<Output = Result<(), RuntimeError>>,
    is_done: bool,
    failure: &mut Option<RuntimeError>,
) {
    if !is_done {
        *failure = component.await.err();
    }
}

async fn catch_component(
    component: impl Future<Output = Result<(), RuntimeError>>,
    panic_class: RuntimeError,
) -> Result<(), RuntimeError> {
    AssertUnwindSafe(component)
        .catch_unwind()
        .await
        .map_err(|_| panic_class)?
}

async fn stopped(shutdown: &mut watch::Receiver<bool>) {
    while !*shutdown.borrow_and_update() {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

async fn serve_http(
    listener: TcpListener,
    router: Router,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), RuntimeError> {
    let mut connections = FuturesUnordered::new();
    loop {
        tokio::select! {
            biased;
            _ = stopped(&mut shutdown) => break,
            outcome = connections.next(), if !connections.is_empty() => {
                if let Some(outcome) = outcome { outcome?; }
            }
            accepted = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                let (stream, _) = accepted.map_err(|_| RuntimeError::Http)?;
                connections.push(catch_component(serve_connection(stream, router.clone(), shutdown.clone()), RuntimeError::HttpPanicked));
            }
        }
    }
    drop(listener);
    while let Some(outcome) = connections.next().await {
        outcome?;
    }
    Ok(())
}

async fn serve_connection(
    stream: TcpStream,
    router: Router,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), RuntimeError> {
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(DRAIN_BUDGET)
        .max_headers(MAX_HEADERS)
        .max_buf_size(MAX_HEADER_BUFFER);
    let connection =
        builder.serve_connection(TokioIo::new(stream), TowerToHyperService::new(router));
    tokio::pin!(connection);
    tokio::select! {
        biased;
        _ = stopped(&mut shutdown) => {
            connection.as_mut().graceful_shutdown();
            // Client disconnects/parser failures are connection-local, never runtime outages.
            let _ = connection.await;
        }
        _ = &mut connection => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests;
