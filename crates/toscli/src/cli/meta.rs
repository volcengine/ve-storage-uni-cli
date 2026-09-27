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

// Keep most utility argument contracts aligned with ve-tos while the new
// top-level command owns the registry and dispatch surface.
pub use ve_tos_cli::cli::meta::{ApiArgs, CapabilitiesArgs, DocumentationLanguage};

// [Review Fix #16] ByteTOS has its own config contract. Sharing ve-tos Clap
// help made unsupported control-plane keys appear in copyable tos examples.
// [Review Fix #23] The help renderer rewrites ve-tos-cli to the active ByteTOS
// entrypoint, including the unified CLI's "ve-storage-uni-cli tos" prefix.
/// ByteTOS configuration command and its optional action.
#[derive(Debug, Args)]
#[command(
    about = "Inspect and modify ByteTOS CLI configuration",
    long_about = "Inspect and modify ByteTOS CLI configuration stored in ~/.tos/config.toml.",
    after_help = "Examples:\n  ve-tos-cli config init\n  ve-tos-cli config show\n  ve-tos-cli config set region cn-beijing\n  ve-tos-cli config set endpoint https://tos.example.com\n  ve-tos-cli config set auth_mode zti"
)]
pub struct ConfigCommand {
    #[command(subcommand)]
    pub action: Option<ConfigAction>,
}

/// ByteTOS configuration actions with examples valid for this command surface.
#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Initialize the selected profile without setting network defaults
    #[command(
        after_help = "Examples:\n  ve-tos-cli config init\n  ve-tos-cli config init --profile staging\n\nCreates shared and [profile.tos] sections. Configure an endpoint (and region if it cannot be inferred), or PSM service discovery, before making requests."
    )]
    Init {
        /// Profile name to initialize (defaults to default)
        #[arg(long)]
        profile: Option<String>,
    },
    /// Show current configuration with secrets redacted
    #[command(
        after_help = "Examples:\n  ve-tos-cli config show\n  ve-tos-cli config show --output json\n\nShows effective values with source annotations such as [default], [default.tos], env, and cli."
    )]
    Show,
    /// Set a ByteTOS configuration value
    #[command(
        after_help = "Common KEY values:\n  region                         -> [active-profile]\n  endpoint / psm / idc / cluster / addr_family -> [active-profile.tos]\n  auth_mode (aksk or zti)        -> [active-profile.tos]\n  access_key_id / secret_access_key / security_token -> active tos credentials\n  checkpoint_dir / progress_enabled / max_retry_count -> [active-profile.tos]\n\nKEY may also name a profile, for example staging.region or staging.tos.psm.\n\nExamples:\n  ve-tos-cli config set region cn-beijing\n  ve-tos-cli config set endpoint https://tos.example.com\n  ve-tos-cli config set psm toutiao.tos.tosapi\n  ve-tos-cli config set auth_mode zti"
    )]
    Set {
        /// Configuration key, for example region, endpoint, or staging.tos.psm
        #[arg(value_name = "KEY")]
        key: String,
        /// Configuration value
        #[arg(value_name = "VALUE")]
        value: String,
    },
}

/// Offline-first diagnostics for the ByteTOS command surface.
#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  ve-tos-cli doctor\n  ve-tos-cli doctor --check auth\n  ve-tos-cli doctor --check network\n  ve-tos-cli doctor --check completion"
)]
pub struct DoctorArgs {
    /// Check one module: auth, config, registry, network (or endpoint), mcp, or completion
    #[arg(long)]
    pub check: Option<String>,
    /// Kept for compatibility with existing invocations; ByteTOS doctor has no permission probe.
    #[arg(long, hide = true)]
    pub bucket: Option<String>,
    /// Probe the configured TOS endpoint; off by default to keep doctor offline
    #[arg(long, default_value_t = false)]
    pub live_network: bool,
    /// Timeout in milliseconds for --live-network
    #[arg(long, default_value_t = 3000)]
    pub network_timeout_ms: u64,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Generate shell completion scripts for the dedicated ByteTOS command names (`tos-cli` and `tos`) plus the unified entrypoint.\n\nThe command returns a structured CLI Envelope. Use `--output json` plus a JSON extractor such as `jq -r '.data.script'` when installing the raw script.",
    after_help = "Examples:\n  tos-cli completion bash\n  tos-cli completion zsh\n  tos-cli completion fish\n\nInstall examples:\n  tos-cli completion bash --output json | jq -r '.data.script' > ~/.tos-completion.bash\n  echo 'source ~/.tos-completion.bash' >> ~/.bashrc\n  mkdir -p ~/.zfunc\n  tos-cli completion zsh --output json | jq -r '.data.script' > ~/.zfunc/_tos\n  echo 'fpath=(~/.zfunc $fpath); autoload -Uz compinit && compinit' >> ~/.zshrc\n  mkdir -p ~/.config/fish/completions && tos-cli completion fish --output json | jq -r '.data.script' > ~/.config/fish/completions/tos.fish\n  tos-cli completion powershell --output json | jq -r '.data.script' >> $PROFILE"
)]
pub struct CompletionArgs {
    /// Shell type
    pub shell: String,
}

#[derive(Debug, Args)]
#[command(
    long_about = "Start the ByteTOS MCP server from the same registry-backed skill definitions used by `skill list`.\n\n`stdio` is the default MCP transport for clients that spawn the CLI as a child process. `sse` is same-host only and listens on 127.0.0.1:<port>. After binding, it prints a fresh Bearer token once to stderr. Every `/sse` and `/message` request must send it in the Authorization header; URL/query credentials are rejected. The Host header must be exact `127.0.0.1:<port>` or `localhost:<port>`. Native clients may omit Origin; when the Origin header is present, it must be the matching HTTP loopback origin on the same port. `--dry-run` and `--describe` report this startup contract without generating a token or launching a server.",
    after_help = "Examples:\n  tos serve --mcp\n  tos serve --mcp --transport sse --port 9090\n  tos serve --mcp --dry-run --output json\n\nMCP usage:\n  Tool names come from skills, e.g. `tos_ls` for `tos ls` and `tos_cp` for `tos cp`.\n  `tools/call` plans by default; pass argument `execute: true` to run the underlying CLI command."
)]
pub struct ServeArgs {
    /// Enable MCP server
    #[arg(long)]
    pub mcp: bool,
    /// Transport: stdio or sse
    #[arg(long, default_value = "stdio", value_parser = ["stdio", "sse"])]
    pub transport: String,
    /// Port for SSE transport
    // [Review Fix #7] Port zero cannot describe the OS-selected listener port consistently.
    #[arg(
        long,
        default_value = "8080",
        value_parser = clap::value_parser!(u16).range(1..)
    )]
    pub port: u16,
}

#[derive(Debug, Args)]
#[command(
    about = "List or export TOS skills",
    long_about = "List built-in TOS skills or export them as Markdown SKILL.md directories for Codex/Agent runtimes.",
    after_help = "Examples:\n  tos-cli skill list\n  tos-cli skill list --language zh\n  tos-cli skill export --name tos_ls --dir ./tos-skills\n  tos-cli skill export --language zh --dir ./tos-skills-zh\n  tos-cli skill export --dir ./tos-skills --dry-run --output json\n\nNotes:\n  Export writes dir/SKILL.md plus dir/{domain}/{skill_name}/SKILL.md and refuses to overwrite existing files.\n  Use --language zh to generate Chinese Markdown skill docs.\n  Use --dry-run to preview target paths and conflicts without creating files."
)]
pub struct SkillCommand {
    #[command(subcommand)]
    pub action: SkillAction,
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
    /// Export skills as Markdown SKILL.md directories
    Export {
        #[arg(
            long,
            help = "Exact skill name or canonical command, e.g. tos_cp or \"tos cp\""
        )]
        name: Option<String>,
        /// Output directory
        #[arg(long, default_value = "./tos-skills")]
        dir: String,
        /// Documentation language: en or zh
        #[arg(long, value_enum, default_value = "en")]
        language: DocumentationLanguage,
    },
}
