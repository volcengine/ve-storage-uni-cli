use crate::platform::{control::stopped, AppError};
use futures::FutureExt;
use std::{future::Future, panic::AssertUnwindSafe, time::Duration};
use tokio::sync::watch;

pub(crate) async fn manage(
    runtime: impl Future<Output = Result<(), AppError>>,
    publisher: impl Future<Output = Result<(), AppError>>,
    signal: impl Future<Output = Result<(), AppError>>,
    shutdown: watch::Sender<bool>,
    clear: impl Fn(),
) -> Result<(), AppError> {
    let runtime = catch(runtime, AppError::Runtime);
    let publisher = catch(publisher, AppError::Publisher);
    tokio::pin!(runtime, publisher, signal);
    let (is_runtime_done, is_publisher_done, outcome) = tokio::select! { biased;
        result=&mut signal => (false,false,result),
        result=&mut publisher => (false,true,result.and(Err(AppError::Publisher))),
        result=&mut runtime => (true,false,result.and(Err(AppError::Runtime))),
    };
    // [Review Fix #79] Invalidate admission before runtime draining, even when the publisher panics or exits.
    clear();
    shutdown.send_replace(true);
    let (mut runtime_failure, mut publisher_failure) = (None, None);
    let drain = async {
        tokio::join!(
            finish(runtime, is_runtime_done, &mut runtime_failure),
            finish(publisher, is_publisher_done, &mut publisher_failure)
        )
    };
    let drained = tokio::time::timeout(Duration::from_secs(9), drain).await;
    // [Review Fix #85] A bounded drain must preserve the original failure and every observed secondary failure.
    let failure = publisher_failure
        .or(runtime_failure)
        .or_else(|| drained.err().map(|_| AppError::Runtime));
    outcome.and(failure.map_or(Ok(()), Err))
}
async fn finish(
    component: impl Future<Output = Result<(), AppError>>,
    is_done: bool,
    failure: &mut Option<AppError>,
) {
    if !is_done {
        *failure = component.await.err();
    }
}
async fn catch(
    component: impl Future<Output = Result<(), AppError>>,
    class: AppError,
) -> Result<(), AppError> {
    AssertUnwindSafe(component)
        .catch_unwind()
        .await
        .map_err(|_| class)?
}
pub(crate) async fn runtime_signal(mut receiver: watch::Receiver<bool>) -> std::io::Result<()> {
    stopped(&mut receiver).await;
    Ok(())
}
#[cfg(unix)]
pub(crate) struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}
#[cfg(unix)]
impl Signals {
    pub(crate) fn install() -> Result<Self, AppError> {
        use tokio::signal::unix::{signal, SignalKind};
        let interrupt = signal(SignalKind::interrupt()).map_err(|_| AppError::Signal)?;
        let terminate = signal(SignalKind::terminate()).map_err(|_| AppError::Signal)?;
        Ok(Self {
            interrupt,
            terminate,
        })
    }
    pub(crate) async fn wait(mut self) -> Result<(), AppError> {
        tokio::select! {
            value=self.interrupt.recv() => value.ok_or(AppError::Signal),
            value=self.terminate.recv() => value.ok_or(AppError::Signal),
        }
    }
}
#[cfg(not(unix))]
pub(crate) struct Signals;
#[cfg(not(unix))]
impl Signals {
    pub(crate) fn install() -> Result<Self, AppError> {
        Ok(Self)
    }
    pub(crate) async fn wait(self) -> Result<(), AppError> {
        tokio::signal::ctrl_c().await.map_err(|_| AppError::Signal)
    }
}
#[cfg(test)]
mod tests;
