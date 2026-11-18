//! Internal dependency seam only; no platform HTTP format is asserted here.
// These operations are reserved for a future Store-gated lifecycle implementation.
#![allow(dead_code)]

use std::{fmt, future::Future, pin::Pin};

type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderTokens, ProviderError>> + Send + 'a>>;

pub(crate) trait OAuthProvider: Send + Sync {
    fn exchange(&self, code: &str) -> ProviderFuture<'_>;
}

pub(crate) struct ProviderTokens {
    access_token: String,
    refresh_token: String,
}

impl fmt::Debug for ProviderTokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderTokens([REDACTED])")
    }
}

#[derive(Debug)]
pub(crate) struct ProviderError;

pub(crate) struct DisabledProvider;

impl OAuthProvider for DisabledProvider {
    fn exchange(&self, _code: &str) -> ProviderFuture<'_> {
        Box::pin(async { Err(ProviderError) })
    }
}

#[cfg(test)]
pub(crate) struct MockProvider {
    pub(crate) calls: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl OAuthProvider for MockProvider {
    fn exchange(&self, _code: &str) -> ProviderFuture<'_> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Box::pin(async {
            Ok(ProviderTokens {
                access_token: "mock-only-access".into(),
                refresh_token: "mock-only-refresh".into(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_provider_model_has_safe_debug_output() {
        let tokens = ProviderTokens {
            access_token: "ACCESS_SECRET_SENTINEL".into(),
            refresh_token: "REFRESH_SECRET_SENTINEL".into(),
        };
        assert_eq!(format!("{tokens:?}"), "ProviderTokens([REDACTED])");
    }
}
