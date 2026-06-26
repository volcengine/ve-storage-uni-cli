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
pub use ve_tos_cli::cli::meta::{
    ApiArgs, CapabilitiesArgs, ConfigAction, ConfigCommand, DoctorArgs, DocumentationLanguage,
};

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
    long_about = "Start the ByteTOS MCP server from the same registry-backed skill definitions used by `skill list`.\n\n`stdio` is the default MCP transport for clients that spawn the CLI as a child process. `sse` starts a local HTTP/SSE listener on 127.0.0.1:<port> with rmcp's standard `/sse` and `/message` endpoints. `--dry-run` and `--describe` report the startup plan without launching a long-lived server.",
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
    #[arg(long, default_value = "8080")]
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
        #[arg(long)]
        name: Option<String>,
        /// Output directory
        #[arg(long, default_value = "./tos-skills")]
        dir: String,
        /// Documentation language: en or zh
        #[arg(long, value_enum, default_value = "en")]
        language: DocumentationLanguage,
    },
}
