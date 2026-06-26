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

use super::output::OutputFormat;
use clap::builder::BoolishValueParser;
use clap::Args;
use std::path::PathBuf;

use crate::agent::error::CliError;
use crate::infra::config::ConfigFile;

/// Summary of the global options reused in the `ve-tos-cli --help` grouped help text.
///
/// This text is used by the custom grouped help renderer and must stay in sync
/// with the clap definition on `GlobalArgs`.
pub const GROUPED_HELP_GLOBAL_OPTIONS: &str = r#"Global Options:
  -P, --profile <PROFILE>          Configuration profile name
      --config-path <PATH>         Path to the config TOML file (env: TOS_CONFIG_PATH)
  -r, --region <REGION>            Region
  -e, --endpoint <ENDPOINT>        Custom Data Plane endpoint
      --psm <PSM>                  ByteCloud TOS PSM service name (tos only)
      --idc <IDC>                  IDC used with --psm (tos only)
      --cluster <CLUSTER>          Cluster used with --psm (tos only)
      --addr-family <VALUE>        Address family used with --psm: v4, v6, or dual-stack (tos only)
      --control-endpoint <VALUE>   Custom Control Plane endpoint (used only by the TOS command surface)
      --account-id <ACCOUNT_ID>     Account ID for Control Plane endpoints
  -o, --output <FORMAT>            Output format (json, table, csv, yaml, markdown)
      --query <QUERY>              JMESPath filter expression
      --dry-run                    Preview the effect of the command without executing it
      --describe                   Print structured self-description of the command
  -y, --yes                        Auto-confirm destructive prompts in an interactive shell
      --confirm <RESOURCE>         Confirm critical delete operations with the exact tos:// or adrive:// target
      --no-color [<BOOL>]          Disable colored output (also honors `NO_COLOR=1`)
  -v, --verbose                    Include extra diagnostic output where supported
  -q, --quiet                      Disable prompts and progress output
"#;

/// Global arguments shared by every command.
#[derive(Debug, Args, Clone)]
pub struct GlobalArgs {
    /// Configuration profile name
    #[arg(
        long,
        short = 'P',
        env = "TOS_PROFILE",
        default_value = "default",
        global = true
    )]
    pub profile: String,

    /// Path to the config TOML file.
    ///
    /// Defaults to `$HOME/.tos/config.toml` when omitted. The parent directory
    /// is also used for the local encryption key that protects stored secrets.
    #[arg(long, env = "TOS_CONFIG_PATH", value_name = "PATH", global = true)]
    pub config_path: Option<PathBuf>,

    /// Region
    ///
    /// CLI flag only. Environment region variables are read by each tool's
    /// profile loader so they stay at the lowest precedence
    /// (CLI > Config > Env).
    #[arg(long, short = 'r', global = true)]
    pub region: Option<String>,

    /// Custom service endpoint
    ///
    /// CLI flag only. Environment endpoint variables are read by each tool's
    /// profile loader so they stay at the lowest precedence.
    #[arg(long, short = 'e', global = true)]
    pub endpoint: Option<String>,

    /// ByteCloud TOS PSM service name.
    ///
    /// CLI flag only. Supported by the `tos` command surface. When omitted,
    /// `--idc`, `--cluster`, and `--addr-family` do not enable PSM mode by
    /// themselves.
    #[arg(long, global = true)]
    pub psm: Option<String>,

    /// IDC used with PSM service discovery.
    #[arg(long, global = true)]
    pub idc: Option<String>,

    /// Cluster used with PSM service discovery.
    #[arg(long, global = true)]
    pub cluster: Option<String>,

    /// Address family used with PSM service discovery: v4, v6, or dual-stack.
    #[arg(long = "addr-family", alias = "addr_family", global = true)]
    pub addr_family: Option<String>,

    /// Custom control-plane endpoint
    ///
    /// CLI flag only. Tool-specific control endpoint variables are read by the
    /// profile loader when that tool supports control-plane operations.
    #[arg(long, global = true)]
    pub control_endpoint: Option<String>,

    /// Account ID for control-plane endpoints
    ///
    /// CLI flag only. Tool-specific account ID variables are read by the
    /// profile loader when that tool supports control-plane operations.
    #[arg(long, global = true)]
    pub account_id: Option<String>,

    /// Output format: json / xml / table / csv / yaml / markdown
    #[arg(long, short = 'o', env = "TOS_OUTPUT", value_enum, global = true)]
    pub output: Option<OutputFormat>,

    /// JMESPath filter expression
    #[arg(long, global = true)]
    pub query: Option<String>,

    /// Preview the effect of the command without executing it
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Print structured self-description of the command
    #[arg(long, global = true)]
    pub describe: bool,

    /// Auto-confirm destructive prompts in an interactive shell. In pipe
    /// (non-TTY) contexts delete-class critical operations still require
    /// `--force` plus an exact `--confirm <RESOURCE>` match.
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,

    /// Confirm critical delete operations by typing the exact public resource
    /// URI (for example `tos://bucket/prefix` or `adrive://inst/space/path`).
    /// Required with `--force` in pipe (non-TTY) contexts.
    #[arg(long, global = true, value_name = "RESOURCE")]
    pub confirm: Option<String>,

    /// Disable colored output.
    ///
    /// `NO_COLOR` is commonly set to `1` in many environments, so this flag
    /// accepts the usual boolean spellings: `1/0/true/false/on/off`.
    // [Review Fix #1] Accept `NO_COLOR=1` so clap does not bail out during parsing.
    #[arg(
        long,
        env = "NO_COLOR",
        global = true,
        num_args = 0..=1,
        default_missing_value = "true",
        value_parser = BoolishValueParser::new()
    )]
    pub no_color: bool,

    /// Include extra diagnostic output where supported.
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,

    /// Disable prompts and progress output.
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,

    /// Directory for trace diagnostics output.
    #[arg(long, global = true, hide = true)]
    pub trace_dir: Option<String>,

    /// Trace redaction level (strict / relaxed / off).
    #[arg(long, default_value = "strict", global = true, hide = true)]
    pub trace_redact: String,
}

// [Review Fix] Provide a Default impl that matches the clap defaults for ergonomic
// unit-test construction.
impl Default for GlobalArgs {
    fn default() -> Self {
        Self {
            profile: "default".to_string(),
            config_path: None,
            region: None,
            endpoint: None,
            psm: None,
            idc: None,
            cluster: None,
            addr_family: None,
            control_endpoint: None,
            account_id: None,
            output: None,
            query: None,
            dry_run: false,
            describe: false,
            yes: false,
            confirm: None,
            no_color: false,
            verbose: false,
            quiet: false,
            trace_dir: None,
            trace_redact: "strict".to_string(),
        }
    }
}

impl GlobalArgs {
    /// Return the effective config file path for this invocation.
    ///
    /// Uses `--config-path` / `TOS_CONFIG_PATH` when present; otherwise returns
    /// the default `$HOME/.tos/config.toml` path.
    pub fn config_path(&self) -> PathBuf {
        ConfigFile::config_path_from(self.config_path.as_deref())
    }

    /// Return the effective config path for runtime commands.
    ///
    /// When the path was provided explicitly, runtime commands require the file
    /// to exist so missing custom paths cannot silently continue with fallback
    /// configuration sources.
    pub fn existing_runtime_config_path(&self) -> Result<PathBuf, CliError> {
        let path = self.config_path();
        // [Review Fix #3] Runtime commands must fail on a missing explicit
        // config path instead of continuing with env/default fallback sources.
        if self.config_path.is_some() && !path.exists() {
            return Err(CliError::ConfigMissing(format!(
                "No config file found at {}",
                path.display()
            )));
        }
        Ok(path)
    }
}
