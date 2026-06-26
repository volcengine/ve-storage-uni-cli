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

use self::low_level::{DataAction, IndexAction, VBucketAction, VPolicyAction};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum TosVectorCommand {
    // === Low-Level API (17 接口) ===
    /// Vector bucket management
    Bucket(low_level::VBucketCommand),
    /// Bucket policy management
    Policy(low_level::VPolicyCommand),
    /// Vector index management
    Index(low_level::IndexCommand),
    /// Vector data operations
    Data(low_level::DataCommand),

    // === Meta Commands ===
    /// Discover CLI capabilities
    Capabilities(meta::CapabilitiesArgs),
    /// Raw API passthrough
    Api(meta::ApiArgs),
    /// Configuration management
    Config(meta::ConfigCommand),
    /// Generate shell completion
    Completion(meta::CompletionArgs),
}

/// [Spec §4/§5 — Controlled Output / Deterministic Errors] Map a parsed
/// `TosVectorCommand` to a stable, copy-paste-runnable command path. See the
/// rationale in `tos_cli::command_path`. Keep this in lock-step with the
/// canonical `tos` mapping so every binary's error envelopes look identical.
pub fn command_path(command: &TosVectorCommand) -> String {
    let suffix = match command {
        TosVectorCommand::Bucket(cmd) => format!("bucket {}", vbucket_action_name(&cmd.action)),
        TosVectorCommand::Policy(cmd) => format!("policy {}", vpolicy_action_name(&cmd.action)),
        TosVectorCommand::Index(cmd) => format!("index {}", index_action_name(&cmd.action)),
        TosVectorCommand::Data(cmd) => format!("data {}", data_action_name(&cmd.action)),
        TosVectorCommand::Capabilities(_) => "capabilities".to_string(),
        TosVectorCommand::Api(_) => "api".to_string(),
        TosVectorCommand::Config(_) => "config".to_string(),
        TosVectorCommand::Completion(_) => "completion".to_string(),
    };
    format!("tosvector {suffix}")
}

fn vbucket_action_name(action: &VBucketAction) -> &'static str {
    match action {
        VBucketAction::Create(_) => "create",
        VBucketAction::Get(_) => "get",
        VBucketAction::Delete(_) => "delete",
        VBucketAction::List => "list",
    }
}

fn vpolicy_action_name(action: &VPolicyAction) -> &'static str {
    match action {
        VPolicyAction::Get(_) => "get",
        VPolicyAction::Set(_) => "set",
        VPolicyAction::Delete(_) => "delete",
    }
}

fn index_action_name(action: &IndexAction) -> &'static str {
    match action {
        IndexAction::Create(_) => "create",
        IndexAction::Get(_) => "get",
        IndexAction::Delete(_) => "delete",
        IndexAction::List(_) => "list",
    }
}

fn data_action_name(action: &DataAction) -> &'static str {
    match action {
        DataAction::Upsert(_) => "upsert",
        DataAction::Get(_) => "get",
        DataAction::Delete(_) => "delete",
        DataAction::Search(_) => "search",
        DataAction::List(_) => "list",
    }
}
