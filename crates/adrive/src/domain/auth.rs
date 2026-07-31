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

use clap::ValueEnum;
use serde::Serialize;
use tos_core::agent::error::CliError;
use tos_core::infra::credentials::StoredOAuthCredentials;

use crate::domain::client::ClientOptions;

/// Authentication strategy selected for an ADrive invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// Sign IDS requests with the existing access-key/secret-key mechanism.
    Aksk,
    /// Use OAuth credentials. Resource request integration is intentionally deferred.
    Oauth,
}

impl AuthMode {
    /// Return the stable configuration and output representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aksk => "aksk",
            Self::Oauth => "oauth",
        }
    }

    /// Parse an authentication mode from configuration or environment input.
    pub fn parse(value: &str, source: &str) -> Result<Self, CliError> {
        match value.trim().to_ascii_lowercase().as_str() {
            // [Review Fix #7] Config, environment, and CLI accept the same two
            // stable values so precedence never changes validation semantics.
            "aksk" => Ok(Self::Aksk),
            "oauth" => Ok(Self::Oauth),
            _ => Err(CliError::ValidationError(format!(
                "invalid ADrive auth mode '{}' from {}; expected aksk or oauth",
                value, source
            ))),
        }
    }
}

/// Source that selected the effective ADrive authentication strategy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthModeSource {
    /// `--auth-mode` on the current invocation.
    CommandLine,
    /// `ADRIVE_AUTH_MODE` in the current process environment.
    Environment,
    /// `[profile.adrive].auth_mode` in the configuration file.
    Config,
    /// Backward-compatible default used when no strategy is configured.
    CompatibilityDefault,
}

impl AuthModeSource {
    /// Return the stable machine-readable source label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommandLine => "command_line",
            Self::Environment => "environment",
            Self::Config => "config",
            Self::CompatibilityDefault => "compatibility_default",
        }
    }
}

/// Effective authentication strategy and the layer that selected it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedAuthMode {
    /// Selected authentication mode.
    pub mode: AuthMode,
    /// Highest-precedence layer that supplied the mode.
    pub source: AuthModeSource,
}

/// Credential presence for the selected mode; unselected-mode fields stay unset.
pub(crate) struct CredentialAvailability {
    pub(crate) has_access_key: Option<bool>,
    pub(crate) has_secret_key: Option<bool>,
    pub(crate) has_security_token: Option<bool>,
    pub(crate) has_access_token: Option<bool>,
    pub(crate) has_refresh_token: Option<bool>,
    pub(crate) oauth_service_integration: &'static str,
    pub(crate) credential_source: &'static str,
}

impl CredentialAvailability {
    pub(crate) fn has_complete_aksk(&self) -> bool {
        self.has_access_key == Some(true) && self.has_secret_key == Some(true)
    }

    pub(crate) fn has_oauth_token(&self) -> bool {
        self.has_access_token == Some(true) || self.has_refresh_token == Some(true)
    }
}

/// Existing AK/SK provider inputs used to construct the IDS HMAC client.
pub struct AkskAuthProvider {
    pub(crate) access_key: String,
    pub(crate) secret_key: String,
    pub(crate) security_token: Option<String>,
    pub(crate) endpoint: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) client_options: ClientOptions,
}

/// OAuth credentials reserved for the future IDS OAuth request implementation.
pub struct OAuthCredentials {
    access_token: Option<String>,
    refresh_token: Option<String>,
}

impl OAuthCredentials {
    /// Build credentials from process-scoped automation inputs without persisting them.
    pub fn from_environment() -> Self {
        Self {
            access_token: std::env::var("ADRIVE_ACCESS_TOKEN").ok(),
            refresh_token: std::env::var("ADRIVE_REFRESH_TOKEN").ok(),
        }
    }

    /// Merge persisted OAuth tokens over process-scoped fallback values.
    pub fn from_stored(stored: StoredOAuthCredentials) -> Self {
        let environment = Self::from_environment();
        Self {
            access_token: stored.access_token.or(environment.access_token),
            refresh_token: stored.refresh_token.or(environment.refresh_token),
        }
    }

    /// Report whether a process-scoped access token is available.
    pub fn has_access_token(&self) -> bool {
        // [Review Fix #8] Empty or whitespace-only automation inputs are not
        // usable credentials and must not make status/doctor report readiness.
        self.access_token
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    }

    /// Report whether a process-scoped refresh token is available.
    pub fn has_refresh_token(&self) -> bool {
        self.refresh_token
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    }
}

/// Credentials resolved for the selected ADrive authentication mode.
pub enum AuthProvider {
    /// Existing HMAC request authentication.
    Aksk(AkskAuthProvider),
    /// OAuth placeholder; server and resource request integration comes later.
    OAuth(OAuthCredentials),
}
