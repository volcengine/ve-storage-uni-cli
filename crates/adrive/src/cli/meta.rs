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
    after_help = "Examples:\n  ve-adrive-cli capabilities\n  ve-adrive-cli capabilities --view full\n  ve-adrive-cli capabilities --group high_level\n  ve-adrive-cli capabilities --search \"sync\""
)]
pub struct CapabilitiesArgs {
    /// View: groups (default — group summary with command counts), text
    /// (one-line summaries), compact (capability rows without parameters),
    /// full (capability rows + parameters + command tree).
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
#[command(
    after_help = "Examples:\n  ve-adrive-cli api file list --describe\n  ve-adrive-cli api instance create --dry-run --request '{\"Name\":\"my-inst\"}'\n  ve-adrive-cli api space delete --dry-run --request '{\"InstanceId\":\"inst-1\",\"SpaceId\":\"sp\"}'"
)]
pub struct ApiArgs {
    /// API group
    pub group: String,
    /// API action
    pub action: String,
    /// Request body (JSON or file://path)
    #[arg(long)]
    pub request: Option<String>,
    /// Show API description
    #[arg(long)]
    pub describe: bool,
    /// Reserved for future raw API execution; currently unimplemented
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
#[command(
    about = "List or export ADrive skill metadata",
    long_about = "List built-in ADrive skills or export them as Markdown SKILL.md directories for Codex/Agent runtimes.\n\nExported files are portable skill-pack artifacts. They are not required to run `serve`; `serve` rebuilds MCP tools from the in-process registry.",
    after_help = "Examples:\n  ve-adrive-cli skill list\n  ve-adrive-cli skill list --language zh\n  ve-adrive-cli skill export --dir ./ve-adrive-skills\n  ve-adrive-cli skill export --language zh --dir ./ve-adrive-skills-zh\n  ve-adrive-cli skill export --name ve_adrive_ls --dir ./ve-adrive-skills --dry-run --output json\n\nNotes:\n  Export writes dir/SKILL.md plus dir/{domain}/{skill_name}/SKILL.md and refuses to overwrite existing files.\n  Use --language zh to generate Chinese Markdown skill docs.\n  MCP tool names match exported skill names, e.g. `ve_adrive_ls` for `ve-adrive ls`."
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
    /// List available skills
    #[command(alias = "ls")]
    List {
        /// Documentation language: en or zh
        #[arg(long, value_enum, default_value = "en")]
        language: DocumentationLanguage,
    },
    /// Export skills as Markdown SKILL.md directories
    Export {
        /// Optional skill name or domain filter
        #[arg(long)]
        name: Option<String>,
        /// Output directory
        #[arg(long, default_value = "./ve-adrive-skills")]
        dir: String,
        /// Documentation language: en or zh
        #[arg(long, value_enum, default_value = "en")]
        language: DocumentationLanguage,
    },
}

#[derive(Debug, Args)]
#[command(
    about = "Inspect and modify A-Drive CLI configuration",
    long_about = "Inspect and modify A-Drive CLI configuration stored in ~/.tos/config.toml.",
    after_help = "Examples:\n  ve-adrive-cli config init\n  ve-adrive-cli config show\n  ve-adrive-cli config set region cn-beijing\n  ve-adrive-cli config set default.adrive.endpoint https://ids-cn-beijing.volces.com\n  ve-adrive-cli config set max_retry_count 3\n  ve-adrive-cli config set requesttimeout 60"
)]
pub struct ConfigCommand {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Interactive initialization
    Init {
        #[arg(long)]
        profile: Option<String>,
    },
    /// Show current configuration (redacted)
    Show,
    /// Set a configuration value
    #[command(
        after_help = "Supported KEY values:\n  region / endpoint / access_key_id / secret_access_key / security_token\n  account_id / default_instance / default_space\n  checkpoint_dir / batch_report_dir / batch_report_format / progress_enabled\n  max_retry_count / requesttimeout / connecttimeout / maxconnections\n\nExamples:\n  ve-adrive-cli config set region cn-beijing\n  ve-adrive-cli config set endpoint https://ids-cn-beijing.volces.com\n  ve-adrive-cli config set max_retry_count 3\n  ve-adrive-cli config set requesttimeout 60"
    )]
    Set { key: String, value: String },
}

#[derive(Debug, Args)]
#[command(
    long_about = "Generate shell completion scripts for the dedicated ADrive command names (`ve-adrive-cli` and `ve-adrive`) plus the unified entrypoint.\n\nThe command returns a structured CLI Envelope. Use `--output json` plus a JSON extractor such as `jq -r '.data.script'` when installing the raw script.",
    after_help = "Examples:\n  ve-adrive-cli completion bash\n  ve-adrive-cli completion zsh\n  ve-adrive-cli completion fish\n\nInstall examples:\n  ve-adrive-cli completion bash --output json | jq -r '.data.script' > ~/.ve-adrive-completion.bash\n  echo 'source ~/.ve-adrive-completion.bash' >> ~/.bashrc\n  mkdir -p ~/.zfunc\n  ve-adrive-cli completion zsh --output json | jq -r '.data.script' > ~/.zfunc/_ve-adrive\n  echo 'fpath=(~/.zfunc $fpath); autoload -Uz compinit && compinit' >> ~/.zshrc\n  mkdir -p ~/.config/fish/completions && ve-adrive-cli completion fish --output json | jq -r '.data.script' > ~/.config/fish/completions/ve-adrive.fish\n  ve-adrive-cli completion powershell --output json | jq -r '.data.script' >> $PROFILE"
)]
pub struct CompletionArgs {
    /// Shell name (bash, zsh, fish, powershell)
    pub shell: String,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Start the ADrive MCP server from the same registry-backed skill definitions used by `skill list`.\n\n`stdio` is the default MCP transport for clients that spawn the CLI as a child process. `sse` starts a local HTTP/SSE listener on 127.0.0.1:<port> with rmcp's standard `/sse` and `/message` endpoints. `--dry-run` and `--describe` report the startup plan without launching a long-lived server.",
    after_help = "Examples:\n  ve-adrive-cli serve --mcp\n  ve-adrive-cli serve --mcp --transport sse --port 9090\n  ve-adrive-cli serve --mcp --dry-run --output json\n\nMCP usage:\n  Tool names come from skills, e.g. `ve_adrive_ls` for `ve-adrive ls` and `ve_adrive_cp` for `ve-adrive cp`.\n  `tools/call` plans by default; pass argument `execute: true` to run the underlying CLI command."
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
    after_help = "Examples:\n  ve-adrive-cli doctor\n  ve-adrive-cli doctor --check auth\n  ve-adrive-cli doctor --check network --live-network\n  ve-adrive-cli doctor --check principles"
)]
pub struct DoctorArgs {
    /// Check a specific module: auth, config, registry, network, principles, mcp, completion
    #[arg(long)]
    pub check: Option<String>,
    /// Probe the configured ADrive endpoint with a real HTTPS request and
    /// record latency. Off by default to keep `ve-adrive-cli doctor` fully offline-safe.
    #[arg(long, default_value_t = false)]
    pub live_network: bool,
    /// Timeout (milliseconds) for the live network probe. Only used when
    /// --live-network is set.
    #[arg(long, default_value_t = 5000)]
    pub network_timeout_ms: u64,
}
