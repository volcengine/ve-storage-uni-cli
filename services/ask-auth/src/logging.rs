use tracing::level_filters::LevelFilter;
use tracing_subscriber::{filter::filter_fn, fmt::MakeWriter, layer::SubscriberExt, Layer};

/// Build a subscriber for sanitized HTTP request and cleanup metric events.
///
/// `level` is the maximum event level and `writer` receives formatted records.
/// Returns an uninstalled subscriber; callers choose local or global installation.
/// Construction performs no I/O and cannot fail.
pub fn request_log_subscriber<W>(
    level: LevelFilter,
    writer: W,
) -> impl tracing::Subscriber + Send + Sync
where
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    // [Review Fix #1] Dependency debug/trace logs may contain credentials; admit only our sanitized event target.
    let safe_events = filter_fn(move |metadata| {
        matches!(
            metadata.target(),
            "ask_auth::request" | "ask_auth::cleanup" | "ask_auth::config"
        ) && level >= *metadata.level()
    });
    tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(writer)
            .with_filter(safe_events),
    )
}
