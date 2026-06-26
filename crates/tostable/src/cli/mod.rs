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

pub mod low_level;
pub mod meta;

use self::low_level::{MaintenanceAction, NamespaceAction, TBucketAction, TableAction};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum TosTableCommand {
    // === Low-Level API (17 接口) ===
    /// Table bucket management
    Bucket(low_level::TBucketCommand),
    /// Namespace management
    Namespace(low_level::NamespaceCommand),
    /// Table management
    Table(low_level::TableCommand),
    /// Maintenance configuration
    Maintenance(low_level::MaintenanceCommand),

    // === Meta Commands ===
    Capabilities(meta::CapabilitiesArgs),
    Api(meta::ApiArgs),
    Config(meta::ConfigCommand),
    Completion(meta::CompletionArgs),
}

/// [Spec §4/§5] See `tos_cli::command_path`. Stable, debug-syntax-free command
/// path used by error envelopes.
pub fn command_path(command: &TosTableCommand) -> String {
    let suffix = match command {
        TosTableCommand::Bucket(cmd) => format!("bucket {}", tbucket_action_name(&cmd.action)),
        TosTableCommand::Namespace(cmd) => {
            format!("namespace {}", namespace_action_name(&cmd.action))
        }
        TosTableCommand::Table(cmd) => format!("table {}", table_action_name(&cmd.action)),
        TosTableCommand::Maintenance(cmd) => {
            format!("maintenance {}", maintenance_action_name(&cmd.action))
        }
        TosTableCommand::Capabilities(_) => "capabilities".to_string(),
        TosTableCommand::Api(_) => "api".to_string(),
        TosTableCommand::Config(_) => "config".to_string(),
        TosTableCommand::Completion(_) => "completion".to_string(),
    };
    format!("tostable {suffix}")
}

fn tbucket_action_name(action: &TBucketAction) -> &'static str {
    match action {
        TBucketAction::Create(_) => "create",
        TBucketAction::Get(_) => "get",
        TBucketAction::Delete(_) => "delete",
        TBucketAction::List => "list",
    }
}

fn namespace_action_name(action: &NamespaceAction) -> &'static str {
    match action {
        NamespaceAction::Create(_) => "create",
        NamespaceAction::Get(_) => "get",
        NamespaceAction::Delete(_) => "delete",
        NamespaceAction::List(_) => "list",
    }
}

fn table_action_name(action: &TableAction) -> &'static str {
    match action {
        TableAction::Create(_) => "create",
        TableAction::Get(_) => "get",
        TableAction::Delete(_) => "delete",
        TableAction::List(_) => "list",
        TableAction::Rename(_) => "rename",
        TableAction::Metadata(_) => "metadata",
    }
}

fn maintenance_action_name(action: &MaintenanceAction) -> &'static str {
    match action {
        MaintenanceAction::Get(_) => "get",
        MaintenanceAction::Set(_) => "set",
    }
}
