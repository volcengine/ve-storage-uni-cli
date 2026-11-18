use std::fmt;

/// Fixed host error categories; no underlying payload, path, argument or SDK error is retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppError {
    /// Unsupported or malformed bootstrap command; original values are discarded.
    Arguments,
    /// A typed configuration, credential or immutable binding is invalid.
    Configuration,
    /// Workload identity or a fresh configuration read is temporarily unavailable.
    Source,
    /// An explicitly configured database could not connect or close.
    Database,
    /// Operator configuration failed protected-file or JSON validation.
    MigrationFile,
    /// The separate one-off migration failed.
    Migration,
    /// Operating-system signal registration or delivery failed.
    Signal,
    /// The explicitly bound HTTP listener failed.
    Http,
    /// The owned HTTP/cleanup runtime failed or exceeded its drain budget.
    Runtime,
    /// The owned configuration publisher exited unexpectedly or panicked.
    Publisher,
    /// Immutable configuration changed and a rolling restart is required.
    RestartRequired,
    /// Host logging, Tokio or another bootstrap facility failed.
    Initialization,
}
impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Arguments => "invalid bootstrap arguments",
            Self::Configuration => "invalid authorization configuration",
            Self::Source => "authorization configuration unavailable",
            Self::Database => "authorization database unavailable",
            Self::MigrationFile => "invalid protected migration configuration",
            Self::Migration => "authorization migration failed",
            Self::Signal => "authorization shutdown signal failed",
            Self::Http => "authorization listener failed",
            Self::Runtime => "authorization runtime failed",
            Self::Publisher => "authorization configuration publisher failed",
            Self::RestartRequired => "authorization configuration requires restart",
            Self::Initialization => "authorization host initialization failed",
        })
    }
}
impl std::error::Error for AppError {}
