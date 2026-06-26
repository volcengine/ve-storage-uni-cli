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

use clap::{Args, Subcommand, ValueEnum};

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli capabilities\n  ve-tos-cli capabilities --view full\n  ve-tos-cli capabilities --group high_level\n  ve-tos-cli capabilities --search \"list\""
)]
pub struct CapabilitiesArgs {
    /// View: groups (default — group summary with command counts), text
    /// (one-line summaries: `<command>\t<description>`), compact (capability
    /// rows without parameters), full (capability rows + parameters + command
    /// tree). `tree` is accepted as a legacy alias for `compact`.
    #[arg(long, default_value = "groups")]
    pub view: String,
    /// Filter by command group
    #[arg(long)]
    pub group: Option<String>,
    /// Search keywords
    #[arg(long)]
    pub search: Option<String>,
    /// Filter by layer
    #[arg(long)]
    pub layer: Option<String>,
}

#[derive(Debug, Args)]
// [Review Fix #1] Keep the metadata example on a registered command so copied help examples succeed.
#[command(
    long_about = "Raw API passthrough.\n\nPass request data with --request as inline JSON or file://path. The JSON fields are: method, endpoint_rule (or endpoint_kind), bucket, key, path, query, headers, body. Put request query parameters, HTTP headers, and JSON body inside that object.",
    after_help = "Examples:\n  ve-tos-cli api object list --describe\n  ve-tos-cli api bucket lifecycle --dry-run --request '{\"method\":\"GET\",\"endpoint_rule\":\"bucket\",\"bucket\":\"mybucket\",\"query\":{\"lifecycle\":\"\"}}'\n  ve-tos-cli api object put --dry-run --request '{\"method\":\"PUT\",\"endpoint_rule\":\"object\",\"bucket\":\"mybucket\",\"key\":\"hello.json\",\"headers\":{\"content-type\":\"application/json\"},\"body\":{\"hello\":\"world\"}}'\n  ve-tos-cli api object delete --force --request '{\"method\":\"DELETE\",\"endpoint_rule\":\"object\",\"bucket\":\"mybucket\",\"key\":\"old.txt\"}'"
)]
pub struct ApiArgs {
    /// API group
    pub group: String,
    /// API action
    pub action: String,
    /// Raw request contract (inline JSON or file://path)
    #[arg(long)]
    pub request: Option<String>,
    /// Show API description
    #[arg(long)]
    pub describe: bool,
    /// Allow non-idempotent raw API execution
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
#[command(
    about = "Inspect and modify TOS CLI configuration",
    long_about = "Inspect and modify TOS CLI configuration stored in ~/.tos/config.toml.",
    after_help = "Examples:\n  ve-tos-cli config init\n  ve-tos-cli config init --profile staging\n  ve-tos-cli config show\n  ve-tos-cli config set region cn-beijing\n  ve-tos-cli config set endpoint https://tos-cn-beijing.volces.com\n  ve-tos-cli config set endpoint https://tos-cn-boe.volces.com --profile dev\n  ve-tos-cli config set control_endpoint https://tos-control-cn-beijing.volces.com\n  ve-tos-cli config set max_retry_count 3\n  ve-tos-cli config set requesttimeout 60"
)]
pub struct ConfigCommand {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Interactive initialization
    #[command(
        after_help = "Examples:\n  ve-tos-cli config init\n  ve-tos-cli config init --profile staging\n\nThis command ensures both the shared profile section `[profile]` and the ve-tos-specific override section `[profile.ve-tos]` exist."
    )]
    Init {
        #[arg(
            long,
            help = "Profile name to initialize (defaults to `default`)",
            long_help = "Profile name to initialize (defaults to `default`).\n\nThe command ensures both the shared section `[profile]` and the ve-tos-specific section `[profile.ve-tos]` exist."
        )]
        profile: Option<String>,
    },
    /// Show current configuration (redacted)
    #[command(
        after_help = "Examples:\n  ve-tos-cli config show\n  ve-tos-cli config show --output json\n\nOutput redacts secrets and annotates where each value comes from, for example `[default]`, `[default.ve-tos]`, env, cli, or derived from endpoint."
    )]
    Show,
    /// Set a configuration value
    #[command(
        after_help = "Supported KEY values:\n  region                      -> write to [active-profile]\n  endpoint                    -> write to [active-profile.ve-tos] or [active-profile.tos]\n  psm / idc / cluster / addr_family (tos only) -> write to [active-profile.tos]\n  control_endpoint (ve-tos only) -> write to [active-profile.ve-tos]\n  account_id                  -> write to [active-profile.ve-tos]\n  checkpoint_dir / progress_enabled -> write to the active TOS binary section\n  max_retry_count / requesttimeout / connecttimeout / maxconnections -> write to the active TOS binary section\n  <profile>.region            -> write to [<profile>]\n  <profile>.endpoint          -> write to the active TOS binary section\n  <profile>.psm (tos only)    -> write to [<profile>.tos]\n  <profile>.control_endpoint (ve-tos only) -> write to [<profile>.ve-tos]\n  <profile>.account_id        -> write to [<profile>.ve-tos]\n  <profile>.ve-tos.endpoint   -> write to [<profile>.ve-tos]\n  <profile>.tos.psm           -> write to [<profile>.tos]\n  <profile>.access_key_id / secret_access_key -> write to [<profile>]\n\nExamples:\n  ve-tos-cli config set region cn-beijing\n  ve-tos-cli config set endpoint https://tos-cn-beijing.volces.com\n  ve-tos-cli config set endpoint https://tos-cn-boe.volces.com --profile dev\n  ve-tos-cli config set control_endpoint https://tos-control-cn-beijing.volces.com\n  ve-tos-cli config set account_id 2100000001\n  ve-tos-cli config set max_retry_count 3\n  ve-tos-cli config set requesttimeout 60\n  ve-tos-cli config set staging.region cn-shanghai\n  ve-tos-cli config set staging.control_endpoint https://tos-control-cn-shanghai.volces.com"
    )]
    Set {
        #[arg(
            value_name = "KEY",
            help = "Configuration key, e.g. `region`, `endpoint`, `account_id`, or `staging.endpoint`",
            long_help = "Configuration key to set.\n\nCommon keys include:\n  - region\n  - endpoint\n  - psm / idc / cluster / addr_family (tos only)\n  - control_endpoint (ve-tos only)\n  - account_id\n  - access_key_id\n  - secret_access_key\n  - max_retry_count\n  - requesttimeout\n  - connecttimeout\n  - maxconnections\n\nThree path forms are supported:\n  - active profile key: `region` (uses `--profile`, defaulting to `default`)\n  - named profile: `staging.region`\n  - binary override: `staging.ve-tos.endpoint` or `staging.tos.psm`\n\nFor `ve-tos`, an `endpoint` / `control_endpoint` / `account_id` / HTTP tuning key without an explicit binary qualifier is written to `[active-profile.ve-tos]` by default. For `tos`, `endpoint` and PSM keys are written to `[active-profile.tos]`; the `tos` entry rejects `control_endpoint` because only `ve-tos` has a control plane endpoint."
        )]
        key: String,
        #[arg(
            value_name = "VALUE",
            help = "Configuration value",
            long_help = "Value to write into the configuration file. Credential fields are encrypted at rest and automatically redacted by `show`."
        )]
        value: String,
    },
}

#[derive(Debug, Args)]
#[command(
    long_about = "Generate shell completion scripts for the dedicated TOS command names (`ve-tos-cli` and `ve-tos`) plus the unified entrypoint.\n\nThe command returns a structured CLI Envelope. Use `--output json` plus a JSON extractor such as `jq -r '.data.script'` when installing the raw script.",
    after_help = "Examples:\n  ve-tos-cli completion bash\n  ve-tos-cli completion zsh\n  ve-tos-cli completion fish\n\nInstall examples:\n  ve-tos-cli completion bash --output json | jq -r '.data.script' > ~/.ve-tos-completion.bash\n  echo 'source ~/.ve-tos-completion.bash' >> ~/.bashrc\n  mkdir -p ~/.zfunc\n  ve-tos-cli completion zsh --output json | jq -r '.data.script' > ~/.zfunc/_ve-tos\n  echo 'fpath=(~/.zfunc $fpath); autoload -Uz compinit && compinit' >> ~/.zshrc\n  mkdir -p ~/.config/fish/completions && ve-tos-cli completion fish --output json | jq -r '.data.script' > ~/.config/fish/completions/ve-tos.fish\n  ve-tos-cli completion powershell --output json | jq -r '.data.script' >> $PROFILE"
)]
pub struct CompletionArgs {
    /// Shell type
    pub shell: String,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Start the TOS MCP server from the same registry-backed skill definitions used by `skill list`.\n\n`stdio` is the default MCP transport for clients that spawn the CLI as a child process. `sse` starts a local HTTP/SSE listener on 127.0.0.1:<port> with rmcp's standard `/sse` and `/message` endpoints. `--dry-run` and `--describe` report the startup plan without launching a long-lived server.",
    after_help = "Examples:\n  ve-tos-cli serve --mcp\n  ve-tos-cli serve --mcp --transport sse --port 9090\n  ve-tos-cli serve --mcp --dry-run --output json\n\nMCP usage:\n  Tool names come from skills, e.g. `ve_tos_ls` for `ve-tos ls` and `ve_tos_bucket_create` for `ve-tos bucket create`.\n  `tools/call` plans by default; pass argument `execute: true` to run the underlying CLI command."
)]
pub struct ServeArgs {
    /// Enable MCP server
    #[arg(long)]
    pub mcp: bool,
    /// Transport: stdio or sse
    #[arg(long, default_value = "stdio", value_parser = ["stdio", "sse"])]
    pub transport: String,
    /// Port for SSE transport
    #[arg(long, default_value = "8080")]
    pub port: u16,
}

#[derive(Debug, Args)]
#[command(
    about = "List or export TOS skill metadata",
    long_about = "List built-in TOS skills or export them as Markdown SKILL.md directories for Codex/Agent runtimes.\n\nExported files are portable skill-pack artifacts. They are not required to run `serve`; `serve` rebuilds tools from the in-process registry.",
    after_help = "Examples:\n  ve-tos-cli skill list\n  ve-tos-cli skill list --language zh\n  ve-tos-cli skill export\n  ve-tos-cli skill export --name cp --dir ./ve-tos-skills\n  ve-tos-cli skill export --language zh --dir ./ve-tos-skills-zh\n  ve-tos-cli skill export --name \"ve-tos bucket create\" --dir ./ve-tos-skills\n\nNotes:\n  Export writes dir/SKILL.md plus dir/{domain}/{skill_name}/SKILL.md and refuses to overwrite existing files.\n  Use --language zh to generate Chinese Markdown skill docs.\n  Use --dry-run to preview target paths and conflicts without creating files."
)]
pub struct SkillCommand {
    #[command(subcommand)]
    pub action: SkillAction,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
pub enum DocumentationLanguage {
    En,
    Zh,
}

impl DocumentationLanguage {
    pub fn code(self) -> &'static str {
        match self {
            DocumentationLanguage::En => "en",
            DocumentationLanguage::Zh => "zh",
        }
    }
}

#[derive(Debug, Subcommand)]
pub enum SkillAction {
    /// List all built-in skills
    #[command(alias = "ls")]
    List {
        /// Documentation language: en or zh
        #[arg(long, value_enum, default_value = "en")]
        language: DocumentationLanguage,
    },
    /// Export skills to local directory
    Export {
        #[arg(long)]
        name: Option<String>,
        /// Output directory
        #[arg(long, default_value = "./ve-tos-skills")]
        dir: String,
        /// Documentation language: en or zh
        #[arg(long, value_enum, default_value = "en")]
        language: DocumentationLanguage,
    },
}

/// Environment diagnostics
#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli doctor\n  ve-tos-cli doctor --check auth\n  ve-tos-cli doctor --check permissions --bucket mybucket\n  ve-tos-cli doctor --check principles"
)]
pub struct DoctorArgs {
    /// Check a specific module: auth, config, registry, permissions, region, network, version, mcp, principles, completion
    #[arg(long)]
    pub check: Option<String>,
    /// Bucket name (used with --check permissions)
    #[arg(long)]
    pub bucket: Option<String>,
    /// [G6] Probe the configured TOS endpoint with a real HTTPS request and
    /// record latency. Off by default to keep `ve-tos doctor` fully offline-safe.
    #[arg(long, default_value_t = false)]
    pub live_network: bool,
    /// [G6] Timeout (milliseconds) for the live network probe. Only used when
    /// --live-network is set.
    #[arg(long, default_value_t = 3000)]
    pub network_timeout_ms: u64,
}
