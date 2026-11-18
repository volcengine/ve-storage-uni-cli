use crate::platform::{
    args::{BootstrapArgs, Command},
    bootstrap::{initial_snapshot, validate_snapshot},
    control::{self, ConfigSource},
    factory::{self, EnabledHost},
    lifecycle::{self, Signals},
    settings, AppError, TccSource,
};
use futures::FutureExt;
use std::{ffi::OsString, io::Write, panic::AssertUnwindSafe, sync::Arc};
use tokio::{net::TcpListener, sync::watch};

/// Run the internal host with owned operating-system arguments and no credential fallback.
/// Installs fixed panic output and the allowlisted logger before platform initialization.
/// Returns fixed AppError categories for argument, platform, configuration or lifecycle failures.
pub fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), AppError> {
    std::panic::set_hook(Box::new(|_| {
        let _ = writeln!(std::io::stderr(), "authorization host panicked");
    }));
    tracing::subscriber::set_global_default(crate::request_log_subscriber(
        tracing::level_filters::LevelFilter::INFO,
        std::io::stderr,
    ))
    .map_err(|_| AppError::Initialization)?;
    let command = crate::platform::args::parse(arguments)?;
    if matches!(command, Command::Help) {
        return std::io::stdout()
            .write_all(crate::platform::args::USAGE.as_bytes())
            .map_err(|_| AppError::Initialization);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|_| AppError::Initialization)?;
    runtime.block_on(async {
        AssertUnwindSafe(dispatch(command))
            .catch_unwind()
            .await
            .map_err(|_| AppError::Initialization)?
    })
}
async fn dispatch(command: Command) -> Result<(), AppError> {
    match command {
        Command::Help => Ok(()),
        Command::Migrate(path) => crate::platform::migration::run(&path).await,
        Command::Probe(arguments) => {
            let source = source(arguments.locator)?;
            let snapshot = initial_snapshot(&source).await?;
            validate_snapshot(&snapshot, arguments.environment)?;
            control::event(
                "validated",
                settings::runtime(&snapshot.runtime, arguments.environment)?.is_some(),
            );
            Ok(())
        }
        Command::Serve(arguments) => serve(arguments).await,
    }
}
fn source(locator: crate::platform::config::Locator) -> Result<Arc<dyn ConfigSource>, AppError> {
    TccSource::new(locator)
        .map(|source| Arc::new(source) as Arc<dyn ConfigSource>)
        .map_err(control::source_error)
}
enum Mode {
    Disabled(TcpListener),
    // [Review Fix #83] Keep the disabled startup state small without changing ownership of active resources.
    Enabled(TcpListener, Box<EnabledHost>),
}
async fn prepare(arguments: BootstrapArgs) -> Result<Mode, AppError> {
    let source = source(arguments.locator)?;
    let initial = initial_snapshot(&source).await?;
    let active = settings::runtime(&initial.runtime, arguments.environment)?;
    let enabled = match active {
        Some(active) => {
            Some(factory::build(active, &initial, source, arguments.environment).await?)
        }
        None => None,
    };
    let listener = TcpListener::bind(arguments.listen)
        .await
        .map_err(|_| AppError::Http)?;
    Ok(match enabled {
        Some(host) => Mode::Enabled(listener, Box::new(host)),
        None => Mode::Disabled(listener),
    })
}
async fn serve(arguments: BootstrapArgs) -> Result<(), AppError> {
    let signal = Signals::install()?.wait();
    tokio::pin!(signal);
    let mode = tokio::select! { biased; result=&mut signal => return result, mode=prepare(arguments) => mode? };
    match mode {
        Mode::Disabled(listener) => {
            control::event("disabled_restart_to_enable", false);
            crate::runtime::serve_disabled(listener, async {
                signal
                    .await
                    .map_err(|_| std::io::Error::other("shutdown signal failed"))
            })
            .await
            .map_err(|_| AppError::Runtime)
        }
        Mode::Enabled(listener, host) => {
            let host = *host;
            control::event("active", true);
            let (shutdown, receiver) = watch::channel(false);
            let runtime = host
                .runtime
                .serve(listener, lifecycle::runtime_signal(receiver.clone()));
            lifecycle::manage(
                async { runtime.await.map_err(|_| AppError::Runtime) },
                host.publisher.run(receiver),
                signal,
                shutdown,
                || host.publisher.controls.clear(),
            )
            .await
        }
    }
}
