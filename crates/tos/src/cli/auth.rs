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

use clap::Args;

use crate::domain::auth::AuthMode;

/// ve-tos-only authentication options for the current invocation.
#[derive(Clone, Copy, Debug, Default, Args)]
pub struct VeTosAuthArgs {
    /// Authentication mode for this invocation: aksk or unified
    #[arg(
        long,
        value_enum,
        global = true,
        value_name = "MODE",
        long_help = "Authentication mode for this invocation: aksk or unified. Precedence is --auth-mode <MODE>, profile auth_mode, TOS_AUTH_MODE, then aksk. Unified selects the same-name externally managed profile, ignores local AK/SK, and uses `ve login`."
    )]
    pub auth_mode: Option<AuthMode>,
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, Parser};

    use super::VeTosAuthArgs;
    use crate::domain::auth::AuthMode;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        auth: VeTosAuthArgs,
    }

    #[test]
    fn ve_tos_help_exposes_exact_unified_auth_contract() {
        let parsed = TestCli::try_parse_from(["ve-tos", "--auth-mode", "unified"]).unwrap();
        assert_eq!(parsed.auth.auth_mode, Some(AuthMode::Unified));

        let help = TestCli::command().render_long_help().to_string();
        for expected in [
            "--auth-mode <MODE>",
            "aksk or unified",
            "TOS_AUTH_MODE",
            "ve login",
        ] {
            assert!(help.contains(expected), "help missing {expected}");
        }
    }
}
