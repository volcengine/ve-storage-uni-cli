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

pub mod meta;

use clap::{Args, Subcommand};
use ve_tos_cli::cli::high_level;

const TOS_CLI_EXAMPLE_PREFIX_ENV: &str = "VE_STORAGE_UNI_BYTED_TOS_EXAMPLE_PREFIX";

#[derive(Debug, Subcommand)]
pub enum TosCliCommand {
    // High-Level Commands
    /// Copy local files, TOS objects, or prefixes
    Cp(high_level::CpArgs),
    /// Move files or objects by copy plus source delete
    Mv(high_level::MvArgs),
    /// Synchronize source and destination incrementally
    Sync(high_level::SyncArgs),
    /// Create a folder marker
    Mkdir(high_level::MkdirArgs),
    /// Delete objects or prefixes
    Rm(RmArgs),
    /// List object prefixes or objects within a bucket
    Ls(high_level::LsArgs),
    /// Show bucket or object metadata
    Stat(high_level::StatArgs),
    /// Calculate object size statistics for a prefix
    Du(high_level::DuArgs),
    /// Find objects by name, size, or mtime
    Find(high_level::FindArgs),
    /// Stream object content
    Cat(high_level::CatArgs),
    /// Upload stdin to an object
    Put(high_level::PutArgs),
    /// Generate a presigned URL
    Presign(high_level::PresignArgs),

    // Utilities
    /// Discover CLI capabilities
    Capabilities(meta::CapabilitiesArgs),
    /// Guarded API metadata and dry-run planning utility
    Api(meta::ApiArgs),
    /// Configuration management
    Config(meta::ConfigCommand),
    /// Generate shell completion
    Completion(meta::CompletionArgs),
    /// Start or plan MCP serving
    Serve(meta::ServeArgs),
    /// Manage/export skill metadata
    Skill(meta::SkillCommand),
    /// Environment diagnostics
    Doctor(meta::DoctorArgs),
}

/// ByteCloud TOS delete arguments, excluding ve-tos-only HNS delete strategy flags.
#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  tos-cli rm tos://mybucket/file.txt --force --confirm tos://mybucket/file.txt\n  tos-cli rm tos://mybucket/prefix/ --recursive --force --confirm tos://mybucket/prefix/\n  tos-cli rm tos://mybucket/prefix/ --recursive --all-versions --force --confirm tos://mybucket/prefix/"
)]
pub struct RmArgs {
    /// Target path (tos://bucket/key or tos://bucket/prefix/)
    #[arg(value_name = "PATH", conflicts_with_all = ["bucket", "key"])]
    pub path: Option<String>,
    /// Bucket name (alternative to positional URI)
    #[arg(long)]
    pub bucket: Option<String>,
    /// Object key or prefix (used with --bucket)
    #[arg(long)]
    pub key: Option<String>,
    /// Recursive delete
    #[arg(long)]
    pub recursive: bool,
    /// Force delete without confirmation
    #[arg(long)]
    pub force: bool,
    /// Delete every object version and delete marker.
    #[arg(long)]
    pub all_versions: bool,
    /// Also abort incomplete multipart uploads matching the prefix
    #[arg(long)]
    pub include_uploads: bool,
    /// Write batch success/failure report to this path
    #[arg(long)]
    pub report_path: Option<String>,
    /// Write only failed items to the batch report
    #[arg(long)]
    pub report_failures_only: bool,
    /// Write planned delete manifest to this path
    #[arg(long)]
    pub manifest_path: Option<String>,
    /// Do not write a planned delete manifest
    #[arg(long, conflicts_with = "manifest_path")]
    pub no_manifest: bool,
    /// Maximum files/items running concurrently in this batch delete
    #[arg(long)]
    pub batch_concurrency: Option<usize>,
    /// Maximum prefixes listed concurrently when recursive listing uses delimiter="/"
    #[arg(long)]
    pub list_concurrency: Option<usize>,
    /// Recursive listing mode: auto, flat, or hierarchical
    #[arg(long, value_enum, requires = "recursive")]
    pub recursive_list_mode: Option<high_level::RecursiveListMode>,
    /// Include pattern
    #[arg(long)]
    pub include: Option<String>,
    /// Exclude pattern
    #[arg(long)]
    pub exclude: Option<String>,
    /// Enable listing-phase echo output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_list_echo")]
    pub list_echo: bool,
    /// Disable listing-phase echo output
    #[arg(long, conflicts_with = "list_echo")]
    pub no_list_echo: bool,
    /// Enable execution progress output even when stderr is not a TTY
    #[arg(long, conflicts_with = "no_progress")]
    pub progress: bool,
    /// Disable execution progress output
    #[arg(long, conflicts_with = "progress")]
    pub no_progress: bool,
}

impl RmArgs {
    /// Convert ByteCloud TOS delete arguments into the shared ve-tos transfer engine shape.
    pub fn into_ve_tos_args(self) -> high_level::RmArgs {
        high_level::RmArgs {
            path: self.path,
            bucket: self.bucket,
            key: self.key,
            recursive: self.recursive,
            // [Review Fix #9] ByteCloud tos always uses planned FNS-style
            // deletes; the ve-tos HNS recursive delete strategy is not part
            // of this parser surface.
            recursive_delete_mode: None,
            force: self.force,
            all_versions: self.all_versions,
            include_uploads: self.include_uploads,
            report_path: self.report_path,
            report_failures_only: self.report_failures_only,
            manifest_path: self.manifest_path,
            no_manifest: self.no_manifest,
            batch_concurrency: self.batch_concurrency,
            list_concurrency: self.list_concurrency,
            recursive_list_mode: self.recursive_list_mode,
            include: self.include,
            exclude: self.exclude,
            list_echo: self.list_echo,
            no_list_echo: self.no_list_echo,
            progress: self.progress,
            no_progress: self.no_progress,
        }
    }
}

/// Stable command path used by envelopes and parse errors.
pub fn command_path(command: &TosCliCommand) -> String {
    match command {
        TosCliCommand::Cp(_) => "tos cp",
        TosCliCommand::Mv(_) => "tos mv",
        TosCliCommand::Sync(_) => "tos sync",
        TosCliCommand::Mkdir(_) => "tos mkdir",
        TosCliCommand::Rm(_) => "tos rm",
        TosCliCommand::Ls(_) => "tos ls",
        TosCliCommand::Stat(_) => "tos stat",
        TosCliCommand::Du(_) => "tos du",
        TosCliCommand::Find(_) => "tos find",
        TosCliCommand::Cat(_) => "tos cat",
        TosCliCommand::Put(_) => "tos put",
        TosCliCommand::Presign(_) => "tos presign",
        TosCliCommand::Capabilities(_) => "tos capabilities",
        TosCliCommand::Api(args) => return format!("tos api {} {}", args.group, args.action),
        TosCliCommand::Config(cmd) => return config_command_path(cmd),
        TosCliCommand::Completion(_) => "tos completion",
        TosCliCommand::Serve(_) => "tos serve",
        TosCliCommand::Skill(cmd) => return skill_command_path(cmd),
        TosCliCommand::Doctor(_) => "tos doctor",
    }
    .to_string()
}

fn config_command_path(cmd: &meta::ConfigCommand) -> String {
    match &cmd.action {
        Some(meta::ConfigAction::Init { .. }) => "tos config init".to_string(),
        Some(meta::ConfigAction::Show) => "tos config show".to_string(),
        Some(meta::ConfigAction::Set { .. }) => "tos config set".to_string(),
        None => "tos config".to_string(),
    }
}

fn skill_command_path(cmd: &meta::SkillCommand) -> String {
    match &cmd.action {
        meta::SkillAction::List { .. } => "tos skill list".to_string(),
        meta::SkillAction::Export { .. } => "tos skill export".to_string(),
    }
}

/// Print grouped help for the ByteCloud TOS tool.
pub fn print_grouped_help() {
    const HELP: &str = r#"TOS CLI — Agent-Native

Usage:
  tos-cli <command> [options]
  ve-storage-uni-cli tos <command> [options]

High-Level Commands:
  cp            Copy local files, TOS objects, or prefixes
  mv            Move files or objects by copy plus source delete
  sync          Synchronize source and destination incrementally
  mkdir         Create a folder marker
  rm            Delete objects or prefixes
  ls            List object prefixes or objects within a bucket
  stat          Show bucket or object metadata
  du            Calculate object size statistics for a prefix
  find          Find objects by name, size, or mtime
  cat           Stream object content
  put           Upload stdin to an object
  presign       Generate a presigned URL

Capabilities / Utilities:
  capabilities  Discover CLI capabilities
  api           Guarded API metadata and dry-run planning utility
  config        Configuration management
  completion    Generate shell completion
  serve         Start or plan MCP serving
  skill         Manage/export skill metadata
  doctor        Environment diagnostics

TOS Target Syntax:
  URI:     tos://<bucket>/<key>
  Flags:   --bucket <BUCKET> --key <KEY>
  Listing: object/prefix listing requires a bucket and uses delimiter="/"; recursive commands expand common prefixes.

Global Options:
  -P, --profile <PROFILE>          Configuration profile name
  -r, --region <REGION>            Region
  -e, --endpoint <ENDPOINT>        Custom endpoint
      --psm <PSM>                  PSM service name for BNS discovery
      --idc <IDC>                  IDC used with --psm
      --cluster <CLUSTER>          Cluster used with --psm
      --addr-family <VALUE>        Address family used with --psm: v4, v6, or dual-stack
  -o, --output <FORMAT>            Output format (json, table, csv, yaml, markdown)
      --query <QUERY>              JMESPath filter expression
      --dry-run                    Preview the command without executing it
      --describe                   Print structured self-description
  -y, --yes                        Auto-confirm destructive prompts
      --confirm <RESOURCE>         Confirm critical deletes with the exact target
      --no-color [<BOOL>]          Disable colored output
  -v, --verbose                    Include extra diagnostic output where supported
  -q, --quiet                      Disable prompts and progress output

Examples:
  tos-cli ls tos://mybucket/
  tos-cli cp ./a.txt tos://mybucket/docs/a.txt
  tos-cli rm tos://mybucket/docs/ --recursive --force --confirm tos://mybucket/docs/
  tos-cli config set endpoint tos-cn-north.byted.org
  tos-cli --psm toutiao.tos.tosapi --idc lf ls tos://mybucket/docs/
  tos-cli capabilities --view full

General:
  -h, --help                        Print help
  -V, --version                     Print version

Language:
  --language <en|zh>                Help output language, e.g. --help --language zh

Run 'tos-cli <command> --help' for details on a specific command.
Run 'tos-cli capabilities --view groups' for machine-readable command listing.
Run 'tos-cli doctor' for environment diagnostics.
"#;
    print!("{}", contextualized_grouped_help(HELP));
}

fn contextualized_grouped_help(help: &str) -> String {
    let prefix =
        std::env::var(TOS_CLI_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "tos-cli".to_string());
    if prefix == "tos-cli" {
        return help.to_string();
    }
    help.replace(
        "  tos-cli <command> [options]\n  ve-storage-uni-cli tos <command> [options]",
        "  __TOS_PRIMARY__ <command> [options]\n  __TOS_SECONDARY__ <command> [options]",
    )
    .replace("tos-cli ", &format!("{prefix} "))
    .replace("__TOS_PRIMARY__", &prefix)
    .replace("__TOS_SECONDARY__", "tos-cli")
}
