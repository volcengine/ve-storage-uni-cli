//! Direct Doubao OAuth, independent of product credentials and hosted auth services.
//! Local credentials reuse the shared cross-platform cipher and file writer in an independent store.
mod application;
mod application_values;
pub mod callback;
pub mod cli;
pub mod client;
pub mod config;
pub mod session;
pub mod store;

pub use callback::{AuthorizationCode, PendingLogin};
pub use client::{Client, TokenSet};
pub use config::Config;
use serde::{Deserialize, Serialize};
pub use session::{Reply, SessionClient, SessionError};
pub use store::{Status, Store};
use zeroize::Zeroize;

/// An owned secret, deliberately lacking `Debug` and `Display` implementations.
#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);
impl Secret {
    /// Own a sensitive value. The allocation is zeroized on drop.
    pub fn new(value: String) -> Self {
        Self(value)
    }
    /// Borrow sensitive text solely for an authorized protocol operation.
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Redacted failures: provider bodies, submitted values and parser errors are never included.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid Doubao configuration")]
    Configuration,
    #[error("unsafe or unavailable Doubao credential storage")]
    Storage,
    #[error("Doubao operation timed out")]
    Timeout,
    #[error("Doubao callback was rejected")]
    Callback,
    #[error("Doubao authorization was denied")]
    Denied,
    #[error("Doubao authorization request failed")]
    Provider,
    #[error("Doubao response was invalid or insufficiently scoped")]
    Response,
    #[error("Doubao login is required")]
    LoginRequired,
    #[error("Doubao credential store is busy")]
    Busy,
}
/// Result with errors suitable for safe CLI display.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
