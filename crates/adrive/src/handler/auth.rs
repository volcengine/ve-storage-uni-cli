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

use serde_json::json;
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;

use crate::cli::auth::{AuthAction, AuthCommand};
use crate::cli::ADriveAuthArgs;
use crate::domain::auth::{AuthMode, ResolvedAuthMode};
use crate::handler::common::{inspect_selected_credentials, output_envelope, resolve_auth_mode};

/// Handle ADrive authentication framework commands without calling OAuth services.
pub async fn handle_auth_command(
    global: &GlobalArgs,
    auth_args: &ADriveAuthArgs,
    command: &AuthCommand,
) -> Result<i32, CliError> {
    let resolved = resolve_auth_mode(global, auth_args.auth_mode)?;
    let action = command.action.unwrap_or(AuthAction::Status);
    match action {
        // [Review Fix #5] Keep the public dispatcher focused on routing; each
        // action owns its output and mode-specific behavior in a small helper.
        AuthAction::Status => handle_status(global, command, resolved),
        AuthAction::Login => handle_login(resolved.mode),
        AuthAction::Logout => handle_logout(global, resolved.mode),
    }
}

fn handle_status(
    global: &GlobalArgs,
    command: &AuthCommand,
    resolved: ResolvedAuthMode,
) -> Result<i32, CliError> {
    let credentials = inspect_selected_credentials(global, resolved.mode)?;
    let command_path = if command.action.is_some() {
        "ve-adrive auth status"
    } else {
        "ve-adrive auth"
    };
    output_envelope(
        global,
        &Envelope::success(
            command_path,
            json!({
                "mode": resolved.mode.as_str(),
                "source": resolved.source.as_str(),
                "has_access_key": credentials.has_access_key,
                "has_secret_key": credentials.has_secret_key,
                "has_security_token": credentials.has_security_token,
                "has_access_token": credentials.has_access_token,
                "has_refresh_token": credentials.has_refresh_token,
                "oauth_service_integration": credentials.oauth_service_integration,
                "credential_source": credentials.credential_source
            }),
        ),
    )?;
    Ok(0)
}

fn handle_login(mode: AuthMode) -> Result<i32, CliError> {
    if mode == AuthMode::Aksk {
        return Err(CliError::ValidationError(
            "auth login requires OAuth mode; select it with --auth-mode oauth, ADRIVE_AUTH_MODE=oauth, or profile auth_mode=oauth".to_string(),
        ));
    }
    Err(CliError::ValidationError(
        "OAuth login service integration is not implemented yet".to_string(),
    ))
}

fn handle_logout(global: &GlobalArgs, mode: AuthMode) -> Result<i32, CliError> {
    if mode == AuthMode::Oauth {
        return Err(CliError::ValidationError(
            "OAuth token storage is not implemented yet".to_string(),
        ));
    }
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive auth logout",
            json!({
                "mode": "aksk",
                "status": "not_applicable",
                "message": "AK/SK credentials were not changed"
            }),
        ),
    )?;
    Ok(0)
}
