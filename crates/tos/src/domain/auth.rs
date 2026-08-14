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

/// Authentication strategy selected for a ve-tos invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    /// Sign requests with access-key and secret-key credentials.
    Aksk,
    /// Use credentials managed by the unified authentication integration.
    Unified,
}

impl AuthMode {
    /// Return the stable configuration and output representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aksk => "aksk",
            Self::Unified => "unified",
        }
    }

    /// Parse an authentication mode supplied by `source`.
    ///
    /// Leading and trailing whitespace and ASCII letter case are normalized.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::ValidationError`] when `value` is not `aksk` or
    /// `unified`.
    pub fn parse(value: &str, source: &str) -> Result<Self, CliError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "aksk" => Ok(Self::Aksk),
            "unified" => Ok(Self::Unified),
            _ => Err(CliError::ValidationError(format!(
                "invalid ve-tos auth mode '{}' from {}; expected aksk or unified",
                value, source
            ))),
        }
    }
}

/// Source that selected the effective ve-tos authentication strategy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthModeSource {
    /// `--auth-mode` on the current invocation.
    CommandLine,
    /// `[profile.ve-tos].auth_mode` in the configuration file.
    Config,
    /// `TOS_AUTH_MODE` in the current process environment.
    Environment,
    /// Backward-compatible default used when no strategy is configured.
    CompatibilityDefault,
}

impl AuthModeSource {
    /// Return the stable machine-readable source label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CommandLine => "command_line",
            Self::Config => "config",
            Self::Environment => "environment",
            Self::CompatibilityDefault => "compatibility_default",
        }
    }
}

/// Effective ve-tos authentication strategy and the layer that selected it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedAuthMode {
    /// Selected authentication mode.
    pub mode: AuthMode,
    /// Highest-precedence layer that supplied the mode.
    pub source: AuthModeSource,
}

#[cfg(test)]
mod tests {
    use super::{AuthMode, AuthModeSource, ResolvedAuthMode};

    #[test]
    fn auth_mode_parser_accepts_only_normalized_ve_tos_modes() {
        assert_eq!(
            AuthMode::parse("  AKSK  ", "profile config").unwrap(),
            AuthMode::Aksk
        );
        assert_eq!(
            AuthMode::parse("Unified", "VE_TOS_AUTH_MODE").unwrap(),
            AuthMode::Unified
        );

        for invalid in ["oauth", "", "oidc"] {
            let error = AuthMode::parse(invalid, "profile config").unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "Validation error: invalid ve-tos auth mode '{}' from profile config; expected aksk or unified",
                    invalid
                )
            );
        }
    }

    #[test]
    fn auth_mode_and_source_have_stable_output_values() {
        assert_eq!(AuthMode::Aksk.as_str(), "aksk");
        assert_eq!(AuthMode::Unified.as_str(), "unified");
        assert_eq!(AuthModeSource::CommandLine.as_str(), "command_line");
        assert_eq!(AuthModeSource::Config.as_str(), "config");
        assert_eq!(AuthModeSource::Environment.as_str(), "environment");
        assert_eq!(
            AuthModeSource::CompatibilityDefault.as_str(),
            "compatibility_default"
        );
        assert_eq!(
            serde_json::to_string(&AuthMode::Unified).unwrap(),
            "\"unified\""
        );
        assert_eq!(
            serde_json::to_string(&AuthModeSource::CommandLine).unwrap(),
            "\"command_line\""
        );
    }

    #[test]
    fn resolved_auth_mode_exposes_mode_and_source() {
        let resolved = ResolvedAuthMode {
            mode: AuthMode::Unified,
            source: AuthModeSource::Config,
        };

        assert_eq!(resolved.mode, AuthMode::Unified);
        assert_eq!(resolved.source, AuthModeSource::Config);
    }
}
