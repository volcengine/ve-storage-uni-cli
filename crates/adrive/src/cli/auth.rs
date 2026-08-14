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

use clap::{Args, Subcommand};

use crate::domain::auth::AuthMode;

/// ADrive-only options that control authentication for the current invocation.
#[derive(Clone, Copy, Debug, Default, Args)]
pub struct ADriveAuthArgs {
    // [Review Fix #6] Keep generated option help synchronized with the ValueEnum.
    /// Authentication mode for this invocation: aksk, oauth, or unified
    // [Review Fix #1] Keep the override ADrive-scoped while allowing scripts
    // to place it before or after an ADrive leaf command.
    #[arg(
        long,
        value_enum,
        global = true,
        value_name = "MODE",
        long_help = "Authentication mode for this invocation: aksk, oauth, or unified. Precedence is --auth-mode <MODE>, profile auth_mode, ADRIVE_AUTH_MODE, then aksk. Unified selects the same-name externally managed profile, ignores local AK/SK and OAuth credentials, and uses `ve login` / `ve logout`."
    )]
    pub auth_mode: Option<AuthMode>,
}

/// Inspect or manage the selected ADrive authentication strategy.
#[derive(Debug, Args)]
#[command(
    long_about = "Inspect the selected ADrive authentication strategy. Select aksk, oauth, or unified with --auth-mode <MODE> or ADRIVE_AUTH_MODE. Unified uses the same-name externally managed profile and never reads local AK/SK or OAuth credentials.",
    after_help = "Examples:\n  ve-adrive-cli auth status\n  ve-adrive-cli --auth-mode oauth auth login --instance inst-1\n  ve-adrive-cli --profile default --auth-mode unified auth status\n  ve login\n\nUnified login and logout are owned by the external framework; use `ve login` or `ve logout`."
)]
pub struct AuthCommand {
    // [Review Fix #2] A bare registry-backed `auth` invocation is a useful,
    // executable status check instead of an invalid generated Agent command.
    #[command(subcommand)]
    pub action: Option<AuthAction>,
}

/// Inputs required to start a Device Authorization login.
#[derive(Clone, Debug, Args)]
pub struct LoginArgs {
    /// ADrive Instance ID to authorize. Overrides profile default_instance and
    /// ADRIVE_DEFAULT_INSTANCE; required if neither fallback is configured
    #[arg(long, value_name = "INSTANCE_ID")]
    pub instance: Option<String>,

    /// OAuth Authorization Server base URL. Required unless configured in the
    /// selected profile or ADRIVE_AUTH_ENDPOINT
    #[arg(long, value_name = "URL")]
    pub auth_endpoint: Option<String>,

    /// Human-readable device name shown during authorization
    #[arg(long, value_name = "NAME")]
    pub device_name: Option<String>,
}

/// Authentication framework actions.
#[derive(Clone, Debug, Subcommand)]
pub enum AuthAction {
    /// Show the effective mode and credential availability without exposing secrets
    Status,
    /// Start OAuth Device Authorization login
    Login(LoginArgs),
    /// Clear the current profile's locally persisted OAuth state; --dry-run only previews it
    Logout,
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::{ADriveAuthArgs, AuthAction, LoginArgs};
    use crate::domain::auth::AuthMode;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(subcommand)]
        action: AuthAction,
    }

    #[derive(Debug, Parser)]
    struct TestAuthArgsCli {
        #[command(flatten)]
        auth: ADriveAuthArgs,
    }

    #[test]
    fn auth_mode_parser_and_help_include_unified() {
        let cli = TestAuthArgsCli::try_parse_from(["ve-adrive-auth", "--auth-mode", "unified"])
            .expect("unified auth mode should parse");
        assert_eq!(cli.auth.auth_mode, Some(AuthMode::Unified));

        let command = TestAuthArgsCli::command();
        let auth_mode = command
            .get_arguments()
            .find(|argument| argument.get_id() == "auth_mode")
            .expect("--auth-mode argument should exist");
        assert_eq!(
            auth_mode.get_help().map(ToString::to_string).as_deref(),
            Some("Authentication mode for this invocation: aksk, oauth, or unified")
        );
        assert_eq!(
            auth_mode
                .get_value_names()
                .map(|names| { names.iter().map(ToString::to_string).collect::<Vec<_>>() }),
            Some(vec!["MODE".to_string()])
        );
        let long_help = auth_mode
            .get_long_help()
            .map(ToString::to_string)
            .unwrap_or_default();
        for expected in ["ADRIVE_AUTH_MODE", "ve login", "ve logout"] {
            assert!(long_help.contains(expected), "long help missing {expected}");
        }
    }

    #[test]
    fn login_accepts_device_authorization_inputs() {
        let cli = TestCli::try_parse_from([
            "ve-adrive-auth",
            "login",
            "--instance",
            "instance-a",
            "--auth-endpoint",
            "https://auth.example.com",
            "--device-name",
            "ci-runner",
        ])
        .expect("login arguments should parse");

        let AuthAction::Login(LoginArgs {
            instance,
            auth_endpoint,
            device_name,
        }) = cli.action
        else {
            panic!("expected login action");
        };
        assert_eq!(instance.as_deref(), Some("instance-a"));
        assert_eq!(auth_endpoint.as_deref(), Some("https://auth.example.com"));
        assert_eq!(device_name.as_deref(), Some("ci-runner"));
    }

    #[test]
    fn login_instance_help_explains_resolution_and_requirement() {
        let command = TestCli::command();
        let login = command
            .find_subcommand("login")
            .expect("login subcommand should exist");
        let instance = login
            .get_arguments()
            .find(|argument| argument.get_id() == "instance")
            .expect("--instance argument should exist");

        assert_eq!(
            instance.get_help().map(ToString::to_string).as_deref(),
            Some(
                "ADrive Instance ID to authorize. Overrides profile default_instance and \
ADRIVE_DEFAULT_INSTANCE; required if neither fallback is configured"
            )
        );
    }

    #[test]
    fn login_auth_endpoint_help_explains_explicit_sources() {
        let command = TestCli::command();
        let login = command
            .find_subcommand("login")
            .expect("login subcommand should exist");
        let auth_endpoint = login
            .get_arguments()
            .find(|argument| argument.get_id() == "auth_endpoint")
            .expect("--auth-endpoint argument should exist");
        let help = auth_endpoint
            .get_help()
            .map(ToString::to_string)
            .unwrap_or_default();

        assert!(help.contains("Required unless configured"));
        assert!(help.contains("ADRIVE_AUTH_ENDPOINT"));
    }
}
