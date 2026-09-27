/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0
 * Unless required by law or agreed in writing, software is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND.
 */

use clap::Args;
use tos_core::infra::byte_tos_auth::ByteTosAuthMode;

/// Authentication selection scoped to the ByteTOS command surface.
#[derive(Clone, Copy, Debug, Default, Args)]
pub struct ByteTosAuthArgs {
    /// Authentication mode for this invocation: aksk or zti
    #[arg(
        long,
        value_enum,
        global = true,
        value_name = "MODE",
        long_help = "Authentication mode for this invocation: aksk or zti. Precedence is --auth-mode <MODE>, [profile.tos].auth_mode, BYTETOS_AUTH_MODE, then aksk. ZTI uses SEC_TOKEN_STRING, a local Agent, or SEC_TOKEN_PATH, ignores local AK/SK, and never falls back to AK/SK."
    )]
    pub auth_mode: Option<ByteTosAuthMode>,
}
