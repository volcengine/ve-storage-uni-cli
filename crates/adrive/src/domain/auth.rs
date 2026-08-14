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
use tos_core::infra::unified_credentials::UnifiedCredentialProvider;

use crate::domain::client::ClientOptions;
use crate::domain::token_manager::OAuthTokenManager;

/// Public Native Client identifier registered for ve-adrive-cli.
///
/// Internal integration environments can override it with the non-empty
/// `ADRIVE_OAUTH_CLIENT_ID` process environment variable.
const OAUTH_CLIENT_ID_PLACEHOLDER: &str = "ve-adrive-cli-public-client-placeholder";
pub const OAUTH_CLIENT_ID: &str = "global_74c584";

const OAUTH_CLIENT_ID_ENV: &str = "ADRIVE_OAUTH_CLIENT_ID";

/// Non-sensitive metadata used by local OAuth diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OAuthClientIdDiagnostics {
    pub(crate) source: &'static str,
    pub(crate) is_placeholder: bool,
}

/// Resolve the Public Client ID for the current process.
pub(crate) fn resolve_oauth_client_id() -> String {
    let environment_value = std::env::var(OAUTH_CLIENT_ID_ENV).ok();
    resolve_oauth_client_id_value(environment_value.as_deref())
}

/// Inspect the Client ID source without exposing its value.
pub(crate) fn oauth_client_id_diagnostics() -> OAuthClientIdDiagnostics {
    let environment_value = std::env::var(OAUTH_CLIENT_ID_ENV).ok();
    oauth_client_id_diagnostics_value(environment_value.as_deref(), OAUTH_CLIENT_ID)
}

fn resolve_oauth_client_id_value(environment_value: Option<&str>) -> String {
    environment_value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(OAUTH_CLIENT_ID)
        .to_string()
}

fn oauth_client_id_diagnostics_value(
    environment_value: Option<&str>,
    built_in_client_id: &str,
) -> OAuthClientIdDiagnostics {
    let environment_client_id = environment_value
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let resolved_client_id = environment_client_id.unwrap_or(built_in_client_id);
    OAuthClientIdDiagnostics {
        source: if environment_client_id.is_some() {
            "environment"
        } else {
            "built_in"
        },
        // [Review Fix #1] Keep placeholder detection stable after the built-in
        // Client ID is replaced with its registered production value.
        is_placeholder: resolved_client_id == OAUTH_CLIENT_ID_PLACEHOLDER,
    }
}

/// Scope shortcut requested by the first OAuth implementation.
pub const OAUTH_SCOPE: &str = "all";

/// Authentication strategy selected for an ADrive invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// Sign IDS requests with the existing access-key/secret-key mechanism.
    Aksk,
    /// Use OAuth credentials and Bearer-authenticated Resource requests.
    Oauth,
    /// Use credentials managed by the unified authentication integration.
    Unified,
}

impl AuthMode {
    /// Return the stable configuration and output representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aksk => "aksk",
            Self::Oauth => "oauth",
            Self::Unified => "unified",
        }
    }

    /// Parse an authentication mode from configuration or environment input.
    pub fn parse(value: &str, source: &str) -> Result<Self, CliError> {
        match value.trim().to_ascii_lowercase().as_str() {
            // [Review Fix #7] Config, environment, and CLI accept the same
            // stable values so precedence never changes validation semantics.
            "aksk" => Ok(Self::Aksk),
            "oauth" => Ok(Self::Oauth),
            "unified" => Ok(Self::Unified),
            _ => Err(CliError::ValidationError(format!(
                "invalid ADrive auth mode '{}' from {}; expected aksk, oauth, or unified",
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
    pub(crate) access_key_source: Option<&'static str>,
    pub(crate) secret_key_source: Option<&'static str>,
    pub(crate) security_token_source: Option<&'static str>,
    pub(crate) has_access_token: Option<bool>,
    pub(crate) has_refresh_token: Option<bool>,
    pub(crate) access_token_expiry: Option<String>,
    pub(crate) expires_at: Option<String>,
    pub(crate) scope: Option<Vec<String>>,
    pub(crate) instance_id: Option<String>,
    pub(crate) ready: bool,
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

    pub(crate) fn is_ready(&self) -> bool {
        self.ready
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

/// OAuth manager plus non-secret ADrive Resource Client settings.
pub struct OAuthAuthProvider {
    pub(crate) token_manager: OAuthTokenManager,
    pub(crate) endpoint: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) client_options: ClientOptions,
}

/// Unified credential resolver plus non-secret ADrive Resource Client settings.
pub struct UnifiedAuthProvider {
    pub(crate) credential_provider: UnifiedCredentialProvider,
    pub(crate) endpoint: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) client_options: ClientOptions,
}

/// OAuth credential presence used by offline status and doctor diagnostics.
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

    /// Select a persisted OAuth group or fall back to process-scoped values.
    pub fn from_stored(stored: StoredOAuthCredentials) -> Self {
        // A stored OAuth record is an indivisible credential group. Missing
        // fields never fall through to a different environment-issued group.
        if !stored.is_empty() {
            return Self {
                access_token: stored.access_token,
                refresh_token: stored.refresh_token,
            };
        }
        Self::from_environment()
    }

    /// Report whether the selected credential group has a usable Access Token.
    pub fn has_access_token(&self) -> bool {
        // [Review Fix #8] Empty or whitespace-only automation inputs are not
        // usable credentials and must not make status/doctor report readiness.
        self.access_token
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
    }

    /// Report whether the selected credential group has a usable Refresh Token.
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
    /// OAuth Access Token manager used by the Bearer request path.
    OAuth(OAuthAuthProvider),
    /// Unified-login SDK provider used for per-attempt HMAC signing.
    Unified(UnifiedAuthProvider),
}

#[cfg(test)]
mod tests {
    use super::{
        oauth_client_id_diagnostics_value, resolve_oauth_client_id_value, AuthMode, AuthModeSource,
        OAUTH_CLIENT_ID, OAUTH_CLIENT_ID_ENV, OAUTH_CLIENT_ID_PLACEHOLDER,
    };

    #[test]
    fn auth_mode_parser_accepts_all_exact_adrive_modes() {
        assert_eq!(AuthMode::parse("AKSK", "config").unwrap(), AuthMode::Aksk);
        assert_eq!(
            AuthMode::parse(" oauth ", "config").unwrap(),
            AuthMode::Oauth
        );
        assert_eq!(
            AuthMode::parse("Unified", "config").unwrap(),
            AuthMode::Unified
        );
        assert_eq!(AuthMode::Aksk.as_str(), "aksk");
        assert_eq!(AuthMode::Oauth.as_str(), "oauth");
        assert_eq!(AuthMode::Unified.as_str(), "unified");
    }

    #[test]
    fn auth_mode_parser_rejects_other_values_with_all_expected_modes() {
        let error = AuthMode::parse("oidc", "ADRIVE_AUTH_MODE").unwrap_err();

        assert_eq!(
            error.to_string(),
            "Validation error: invalid ADrive auth mode 'oidc' from ADRIVE_AUTH_MODE; expected aksk, oauth, or unified"
        );
    }

    #[test]
    fn auth_mode_source_has_stable_output_values() {
        assert_eq!(AuthModeSource::CommandLine.as_str(), "command_line");
        assert_eq!(AuthModeSource::Environment.as_str(), "environment");
        assert_eq!(AuthModeSource::Config.as_str(), "config");
        assert_eq!(
            AuthModeSource::CompatibilityDefault.as_str(),
            "compatibility_default"
        );
    }

    #[test]
    fn oauth_client_id_uses_environment_override_or_builtin_default() {
        assert_eq!(OAUTH_CLIENT_ID_ENV, "ADRIVE_OAUTH_CLIENT_ID");
        assert_eq!(OAUTH_CLIENT_ID, "global_74c584");
        assert_eq!(
            resolve_oauth_client_id_value(Some("test-client-id")),
            "test-client-id"
        );
        assert_eq!(resolve_oauth_client_id_value(None), OAUTH_CLIENT_ID);
        assert_eq!(resolve_oauth_client_id_value(Some("  ")), OAUTH_CLIENT_ID);
    }

    #[test]
    fn oauth_client_id_diagnostics_never_exposes_the_value() {
        let environment =
            oauth_client_id_diagnostics_value(Some("test-client-id"), OAUTH_CLIENT_ID);
        assert_eq!(environment.source, "environment");
        assert!(!environment.is_placeholder);

        let built_in = oauth_client_id_diagnostics_value(None, OAUTH_CLIENT_ID);
        assert_eq!(built_in.source, "built_in");
        assert!(!built_in.is_placeholder);

        let legacy_placeholder =
            oauth_client_id_diagnostics_value(None, OAUTH_CLIENT_ID_PLACEHOLDER);
        assert_eq!(legacy_placeholder.source, "built_in");
        assert!(legacy_placeholder.is_placeholder);
    }
}
