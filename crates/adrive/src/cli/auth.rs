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
    /// Authentication mode for this invocation: aksk or oauth
    // [Review Fix #1] Keep the override ADrive-scoped while allowing scripts
    // to place it before or after an ADrive leaf command.
    #[arg(long, value_enum, global = true)]
    pub auth_mode: Option<AuthMode>,
}

/// Inspect or manage the selected ADrive authentication strategy.
#[derive(Debug, Args)]
pub struct AuthCommand {
    // [Review Fix #2] A bare registry-backed `auth` invocation is a useful,
    // executable status check instead of an invalid generated Agent command.
    #[command(subcommand)]
    pub action: Option<AuthAction>,
}

/// Authentication framework actions.
#[derive(Clone, Copy, Debug, Subcommand)]
pub enum AuthAction {
    /// Show the effective mode and credential availability without exposing secrets
    Status,
    /// Start OAuth login (server integration is not implemented yet)
    Login,
    /// Clear OAuth login state (persistent token storage is not implemented yet)
    Logout,
}
