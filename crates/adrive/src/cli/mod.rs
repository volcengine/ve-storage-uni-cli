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

pub mod high_level;
pub mod meta;

use clap::Subcommand;

const ADRIVE_EXAMPLE_PREFIX_ENV: &str = "VE_STORAGE_UNI_ADRIVE_EXAMPLE_PREFIX";

#[derive(Debug, Subcommand)]
pub enum ADriveCommand {
    // ─── High-Level Commands ───────────────────────────────
    /// Copy local files, ADrive files, or folders
    Cp(high_level::CpArgs),
    /// Move files or folders by copy plus source delete
    Mv(high_level::MvArgs),
    /// Synchronize source and destination incrementally
    Sync(high_level::SyncArgs),
    /// Create an instance or space
    Crt(high_level::CreateArgs),
    /// Delete an instance or space
    Del(high_level::DeleteArgs),
    /// Delete a file or folder
    Rm(high_level::RmArgs),
    /// List instances, spaces, files, or folders
    Ls(high_level::LsArgs),
    /// Show instance, space, file, or folder metadata
    Stat(high_level::StatArgs),
    /// Calculate file size statistics for a folder
    Du(high_level::DuArgs),
    /// Find files by name, size, or mtime
    Find(high_level::FindArgs),
    /// Stream file content
    Cat(high_level::CatArgs),
    /// Upload stdin to a file
    Put(high_level::PutArgs),
    /// Create a folder
    Mkdir(high_level::MkdirArgs),

    // ─── Meta / Utilities ───────────────────────────────────
    /// Discover CLI capabilities
    Capabilities(meta::CapabilitiesArgs),
    /// Inspect API metadata
    Api(meta::ApiArgs),
    /// Configuration management
    Config(meta::ConfigCommand),
    /// Generate shell completion
    Completion(meta::CompletionArgs),
    /// Start MCP server
    Serve(meta::ServeArgs),
    /// Manage/export skill metadata
    Skill(meta::SkillCommand),
    /// Environment diagnostics
    Doctor(meta::DoctorArgs),
}

/// Stable, debug-syntax-free command path used by error envelopes.
pub fn command_path(command: &ADriveCommand) -> String {
    let suffix: &str = match command {
        // High-level
        ADriveCommand::Cp(_) => "cp",
        ADriveCommand::Mv(_) => "mv",
        ADriveCommand::Sync(_) => "sync",
        ADriveCommand::Crt(_) => "crt",
        ADriveCommand::Del(_) => "del",
        ADriveCommand::Rm(_) => "rm",
        ADriveCommand::Ls(_) => "ls",
        ADriveCommand::Stat(_) => "stat",
        ADriveCommand::Du(_) => "du",
        ADriveCommand::Find(_) => "find",
        ADriveCommand::Cat(_) => "cat",
        ADriveCommand::Put(_) => "put",
        ADriveCommand::Mkdir(_) => "mkdir",
        // Meta
        ADriveCommand::Capabilities(_) => "capabilities",
        ADriveCommand::Api(args) => return format!("ve-adrive api {} {}", args.group, args.action),
        ADriveCommand::Config(cmd) => return config_command_path(cmd),
        ADriveCommand::Completion(_) => "completion",
        ADriveCommand::Serve(_) => "serve",
        ADriveCommand::Skill(cmd) => return skill_command_path(cmd),
        ADriveCommand::Doctor(_) => "doctor",
    };
    // [Review Fix #11] `command_path` feeds envelopes and describe recovery,
    // so it must use the actual public top-level command directly.
    format!("ve-adrive {suffix}")
}

fn skill_command_path(cmd: &meta::SkillCommand) -> String {
    match &cmd.action {
        meta::SkillAction::List { .. } => "ve-adrive skill list".to_string(),
        meta::SkillAction::Export { .. } => "ve-adrive skill export".to_string(),
    }
}

fn config_command_path(cmd: &meta::ConfigCommand) -> String {
    match &cmd.action {
        Some(meta::ConfigAction::Init { .. }) => "ve-adrive config init".to_string(),
        Some(meta::ConfigAction::Show) => "ve-adrive config show".to_string(),
        Some(meta::ConfigAction::Set { .. }) => "ve-adrive config set".to_string(),
        None => "ve-adrive config".to_string(),
    }
}

/// Print grouped help output for the `adrive` tool.
pub fn print_grouped_help() {
    const HELP: &str = r#"ADrive CLI — Agent-Native

Usage:
  ve-adrive-cli <command> [options]
  ve-storage-uni-cli ve-adrive <command> [options]

High-Level Commands:
  cp            Copy local files, ADrive files, or folders
  mv            Move files or folders by copy plus source delete
  sync          Synchronize source and destination incrementally
  crt           Create an instance or space
  del           Delete an instance or space
  mkdir         Create a folder
  rm            Delete a file or folder
  ls            List instances, spaces, files, or folders
  stat          Show instance, space, file, or folder metadata
  du            Calculate file size statistics for a folder
  find          Find files by name, size, or mtime
  cat           Stream file content
  put           Upload stdin to a file

Capabilities / Utilities:
  capabilities  Discover CLI capabilities
  api           Inspect API metadata
  config        Configuration management
  completion    Generate shell completion
  serve         Start MCP server
  skill         Manage/export skill metadata
  doctor        Environment diagnostics

ADrive Target Syntax:
  URI:     adrive://<instance>/<space>/<folder>/<file>
  Flags:   --instance <ID> --space <ID> --folder <PATH> --file <NAME>
  Names:   add --by-name when <instance> and <space> are names instead of IDs

Global Options:
  -P, --profile <PROFILE>          Configuration profile name
  -r, --region <REGION>            Region
  -e, --endpoint <ENDPOINT>        Custom ADrive endpoint
  -o, --output <FORMAT>            Output format (json, table, csv, yaml, markdown)
      --query <QUERY>              JMESPath filter expression
      --dry-run                    Preview the effect of the command without executing it
      --describe                   Print structured self-description of the command
  -y, --yes                        Auto-confirm destructive prompts in an interactive shell
      --confirm <RESOURCE>         Confirm critical deletes with the exact adrive:// target in non-interactive shells
      --no-color [<BOOL>]          Disable colored output (also honors `NO_COLOR=1`)
  -v, --verbose                    Include extra diagnostic output where supported
  -q, --quiet                      Disable prompts and progress output

Examples:
  ve-adrive-cli crt adrive://inst-1
  ve-adrive-cli crt adrive://inst-1/space-1
  ve-adrive-cli ls adrive://inst-name/space-name/docs/ --by-name
  ve-adrive-cli ls adrive://inst-1/space-1/docs/
  ve-adrive-cli cp ./a.txt adrive://inst-1/space-1/docs/a.txt
  ve-adrive-cli cat --instance inst-1 --space space-1 --folder docs --file a.txt
  ve-adrive-cli cat adrive://inst-1/space-1/docs/a.txt | gzip | ve-adrive-cli put adrive://inst-1/space-1/docs/a.txt.gz
  ve-adrive-cli rm adrive://inst-1/space-1/docs/a.txt --force --confirm adrive://inst-1/space-1/docs/a.txt
  ve-adrive-cli del adrive://inst-1/space-1 --force --confirm adrive://inst-1/space-1

General:
  -h, --help                        Print help
  -V, --version                     Print version

Language:
  --language <en|zh>                Help output language, e.g. --help --language zh

Run 've-adrive-cli <command> --help' for details on a specific command.
Run 've-adrive-cli capabilities --view groups' for machine-readable command listing.
Run 've-adrive-cli doctor' for environment diagnostics.
"#;
    print!("{}", contextualized_grouped_help(HELP));
}

fn contextualized_grouped_help(help: &str) -> String {
    let prefix =
        std::env::var(ADRIVE_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "ve-adrive-cli".to_string());
    if prefix == "ve-adrive-cli" {
        return help.to_string();
    }

    help.replace(
        "  ve-adrive-cli <command> [options]\n  ve-storage-uni-cli ve-adrive <command> [options]",
        "  __ADRIVE_PRIMARY__ <command> [options]\n  __ADRIVE_SECONDARY__ <command> [options]",
    )
    .replace("ve-adrive-cli ", &format!("{prefix} "))
    .replace("__ADRIVE_PRIMARY__", &prefix)
    .replace("__ADRIVE_SECONDARY__", "ve-adrive-cli")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_path_uses_public_ve_adrive_prefix() {
        let command = ADriveCommand::Capabilities(meta::CapabilitiesArgs {
            view: "groups".to_string(),
            group: None,
            search: None,
            layer: None,
        });

        assert_eq!(command_path(&command), "ve-adrive capabilities");
    }
}
