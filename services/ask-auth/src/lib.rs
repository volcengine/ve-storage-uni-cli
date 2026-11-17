//! Fail-closed standalone authorization-service bootstrap.
//!
//! The default router stays disabled; explicit trusted library injection enables the authorization lifecycle.
mod config;
mod error;
mod http;
mod logging;
mod provider;
mod validation;

pub use config::{Config, ConfigError};
pub use logging::request_log_subscriber;

use axum::Router;
use std::sync::Arc;

/// Build the standalone HTTP application with a disabled OAuth provider.
///
/// No parameters are required. Returns a router with health, readiness and the
/// frozen v1 business routes. Construction performs no I/O and cannot fail.
pub fn build_router() -> Router {
    http::router(Arc::new(provider::DisabledProvider))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use std::sync::atomic::Ordering;
    use tower::ServiceExt;

    #[tokio::test]
    async fn injected_mock_cannot_enable_lifecycle_or_receive_callback_secrets() {
        let provider = Arc::new(provider::MockProvider { calls: 0.into() });
        let application = http::router(provider.clone());
        let request = Request::builder().uri("/v1/oauth/doubao/callback?state=ERERERERcRGBERERERERESoqKioqKioqKioqKioqKioqKioqKioqKioqKioqKioq&code=MOCK_ISOLATION_SECRET")
            .body(Body::empty()).unwrap();
        let response = application.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(provider.calls.load(Ordering::Relaxed), 0);
    }
}

pub mod crypto;
pub mod storage;

pub mod login;

/// Build the explicitly injected login and authorization lifecycle application.
/// Requires a validated service; performs no I/O and returns a router. Other business routes stay unavailable.
pub fn build_enabled_router(service: Arc<login::LoginService>) -> Router {
    http::enabled_router(service)
}

pub mod cleanup;

pub mod runtime;

pub mod doubao;

pub mod json;

/// TCE host assembly and authenticated TCC configuration for this service.
pub mod platform;
