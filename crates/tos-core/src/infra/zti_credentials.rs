/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::{fmt, future::Future, pin::Pin, sync::Arc};

use reqwest::header::HeaderValue;

use crate::agent::error::CliError;

type TokenFuture = Pin<Box<dyn Future<Output = Result<String, ZtiTokenError>> + Send>>;
type TokenResolver = dyn Fn() -> TokenFuture + Send + Sync + 'static;

/// Resolves ZTI tokens through an asynchronous source.
///
/// Clones share the resolver. Tokens are fetched for every request without an
/// additional cache here; discovery, refresh, and deadlines belong to the source.
#[derive(Clone)]
pub struct ZtiTokenProvider {
    resolver: Arc<TokenResolver>,
    // [Review Fix #7] Provenance travels with clones so MCP can reject caller
    // providers that its subprocess cannot inherit.
    is_builtin_source: bool,
}

/// Closed, secret-safe failure categories returned by a ZTI token source.
///
/// Sources must classify errors without forwarding raw error text or tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZtiTokenError {
    /// The adapter cannot supply a token.
    Unavailable,
    /// The token expired and could not be refreshed.
    Expired,
    /// The adapter reached its token-resolution deadline.
    Timeout,
}

impl ZtiTokenProvider {
    /// Creates a lazy, cloneable provider without invoking the resolver.
    ///
    /// # Parameters
    ///
    /// * `resolver` - Asynchronous token supplier responsible for source discovery,
    ///   refresh, and bounded I/O. Return only the closed [`ZtiTokenError`] categories.
    ///
    /// # Returns
    ///
    /// A provider that invokes the supplied resolver on each token request.
    ///
    /// # Errors
    ///
    /// Construction is infallible; resolution errors are reported by [`Self::get_header_value`].
    pub fn new<F, Fut>(resolver: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String, ZtiTokenError>> + Send + 'static,
    {
        Self {
            resolver: Arc::new(move || Box::pin(resolver())),
            is_builtin_source: false,
        }
    }

    pub(crate) fn from_builtin_source<F, Fut>(resolver: F) -> Self
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<String, ZtiTokenError>> + Send + 'static,
    {
        let mut provider = Self::new(resolver);
        provider.is_builtin_source = true;
        provider
    }

    /// Returns whether the caller supplied this resolver rather than built-in discovery.
    pub fn is_caller_supplied(&self) -> bool {
        !self.is_builtin_source
    }

    /// Resolves the current token and marks its validated HTTP header as sensitive.
    ///
    /// # Parameters
    ///
    /// Uses this provider's shared resolver; no additional arguments are required.
    ///
    /// # Returns
    ///
    /// A sensitive header preserving the token exactly as returned by the resolver.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::AuthFailed`] for unavailable or expired tokens,
    /// [`CliError::TransferFailed`] for resolver timeouts, or
    /// [`CliError::ValidationError`] for blank or invalid HTTP header values.
    /// Messages never include token contents or raw source errors.
    pub async fn get_header_value(&self) -> Result<HeaderValue, CliError> {
        let token = (self.resolver)().await.map_err(map_token_error)?;
        if token.trim().is_empty() {
            return Err(invalid_token_error());
        }
        let mut header = HeaderValue::from_str(&token).map_err(|_| invalid_token_error())?;
        header.set_sensitive(true);
        Ok(header)
    }
}

impl fmt::Debug for ZtiTokenProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ZtiTokenProvider { resolver: [REDACTED] }")
    }
}

fn invalid_token_error() -> CliError {
    CliError::ValidationError(
        "[ZtiTokenInvalid] ZTI token is empty or invalid for an HTTP header".to_string(),
    )
}

fn map_token_error(error: ZtiTokenError) -> CliError {
    match error {
        ZtiTokenError::Unavailable => {
            CliError::AuthFailed("[ZtiTokenUnavailable] ZTI token is unavailable".to_string())
        }
        ZtiTokenError::Expired => {
            CliError::AuthFailed("[ZtiTokenExpired] ZTI token has expired".to_string())
        }
        ZtiTokenError::Timeout => {
            CliError::TransferFailed("[ZtiTokenTimeout] ZTI token resolution timed out".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[tokio::test]
    async fn resolves_current_token_on_every_call_and_clone() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed_calls = Arc::clone(&calls);
        let provider = ZtiTokenProvider::new(move || {
            let sequence = observed_calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(format!("token-{sequence}")) }
        });

        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.get_header_value().await.unwrap(), "token-0");
        assert_eq!(
            provider.clone().get_header_value().await.unwrap(),
            "token-1"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn construction_clone_and_debug_do_not_evaluate_resolver() {
        let provider = ZtiTokenProvider::new(|| async {
            panic!("resolver must not run during construction or debug");
        });
        let output = format!("{:?}", provider.clone());
        assert_eq!(output, "ZtiTokenProvider { resolver: [REDACTED] }");
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&provider);
        assert!(provider.is_caller_supplied());
    }

    #[tokio::test]
    async fn concurrent_clones_preserve_each_resolution_and_sensitive_header() {
        const REQUEST_COUNT: usize = 4;
        let calls = Arc::new(AtomicUsize::new(0));
        let observed_calls = Arc::clone(&calls);
        let barrier = Arc::new(tokio::sync::Barrier::new(REQUEST_COUNT));
        let provider = ZtiTokenProvider::new(move || {
            let sequence = observed_calls.fetch_add(1, Ordering::SeqCst);
            let barrier = Arc::clone(&barrier);
            async move {
                barrier.wait().await;
                tokio::task::yield_now().await;
                Ok(format!("token-{sequence}"))
            }
        });
        let mut requests = tokio::task::JoinSet::new();
        for _ in 0..REQUEST_COUNT {
            let provider = provider.clone();
            requests.spawn(async move { provider.get_header_value().await });
        }

        let headers = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let mut tokens = Vec::new();
            while let Some(result) = requests.join_next().await {
                let header = result.unwrap().unwrap();
                assert!(header.is_sensitive());
                tokens.push(header.to_str().unwrap().to_string());
            }
            tokens
        })
        .await
        .expect("concurrent token resolutions should complete");
        let mut actual_tokens = headers;
        actual_tokens.sort();
        assert_eq!(actual_tokens, ["token-0", "token-1", "token-2", "token-3"]);
        assert_eq!(calls.load(Ordering::SeqCst), REQUEST_COUNT);
    }

    #[tokio::test]
    async fn successful_headers_are_sensitive_and_debug_redacted() {
        let provider = ZtiTokenProvider::new(|| async { Ok("SECRET_TOKEN".to_string()) });
        let header = provider.get_header_value().await.unwrap();

        assert_eq!(header, "SECRET_TOKEN");
        assert!(header.is_sensitive());
        assert!(!format!("{header:?}").contains("SECRET_TOKEN"));
    }

    #[tokio::test]
    async fn rejects_empty_whitespace_and_invalid_headers_without_echoing_token() {
        for token in [
            "",
            "   ",
            "\t",
            "SECRET\r\nInjected: yes",
            "SECRET\0",
            "SECRET\x7f",
        ] {
            let provider = ZtiTokenProvider::new(move || async { Ok(token.to_string()) });
            let error = provider.get_header_value().await.unwrap_err();

            assert!(matches!(error, CliError::ValidationError(_)));
            assert_eq!(error.to_string(), "Validation error: [ZtiTokenInvalid] ZTI token is empty or invalid for an HTTP header");
            assert!(!format!("{error:?}").contains("SECRET"));
        }
    }

    #[tokio::test]
    async fn preserves_valid_header_contents_without_normalization() {
        let provider = ZtiTokenProvider::new(|| async { Ok(" token with spaces ".to_string()) });
        assert_eq!(
            provider.get_header_value().await.unwrap(),
            " token with spaces "
        );
    }

    #[tokio::test]
    async fn resolver_errors_have_fixed_secret_safe_categories_and_messages() {
        let cases = [
            (
                ZtiTokenError::Unavailable,
                "Authentication failed: [ZtiTokenUnavailable] ZTI token is unavailable",
            ),
            (
                ZtiTokenError::Expired,
                "Authentication failed: [ZtiTokenExpired] ZTI token has expired",
            ),
            (
                ZtiTokenError::Timeout,
                "Transfer failed: [ZtiTokenTimeout] ZTI token resolution timed out",
            ),
        ];
        for (failure, expected_message) in cases {
            let provider = ZtiTokenProvider::new(move || async move { Err(failure) });
            let error = provider.get_header_value().await.unwrap_err();

            assert_eq!(error.to_string(), expected_message);
            match failure {
                ZtiTokenError::Unavailable | ZtiTokenError::Expired => {
                    assert!(matches!(error, CliError::AuthFailed(_)))
                }
                ZtiTokenError::Timeout => assert!(matches!(error, CliError::TransferFailed(_))),
            }
        }
    }
}
