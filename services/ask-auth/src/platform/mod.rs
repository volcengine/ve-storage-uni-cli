//! TCE host assembly with native authenticated TCC reads and fail-closed controls.
//! The host installs an allowlisted logger and fixed panic output before initialization.
pub mod config;
pub mod tcc;
pub use tcc::{PayloadKey, Snapshot, TccError, TccSource};

mod error;
mod json;
mod settings;
pub use error::AppError;
mod args;
mod bootstrap;
mod control;
mod factory;
mod host;
mod lifecycle;
mod migration;
pub use host::run;
