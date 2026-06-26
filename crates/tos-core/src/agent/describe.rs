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

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// 命令自描述信息
#[derive(Debug, Serialize, Deserialize)]
pub struct CommandDescription {
    pub command: String,
    pub layer: CommandLayer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    pub description: String,
    pub risk_level: RiskLevel,
    pub supports_dry_run: bool,
    pub supports_pipe: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<CommandParameter>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scenario_routing: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub related_commands: Option<RelatedCommands>,
    /// [Review Fix #6] 该命令实际依赖的底层 OpenAPI 列表，供 Agent 做能力推理；
    /// 元数据由 registry.rs 集中维护，handler 仅做转写。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low_level_apis: Option<Vec<String>>,
    /// [G5] Agent-friendly examples for `--query` (JMESPath) and `--output` filters.
    /// Each entry is a copy-pasteable shell snippet that demonstrates how to extract
    /// the most useful field(s) from this command's response. Optional.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_filter_examples: Option<Vec<String>>,
    /// [G5] Quoting/escaping reminders specific to this command's typical inputs
    /// (e.g. object keys with spaces, JMESPath backticks in `--query`). Returned
    /// as bullet-style hints, one per element.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shell_quoting_tips: Option<Vec<String>>,
    /// [G5] Alias of `low_level_apis` exposed under the spec-mandated key
    /// `wraps_apis`. Both fields are populated to preserve backwards-compat with
    /// existing Agent prompts that reference `low_level_apis`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wraps_apis: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandParameter {
    pub name: String,
    pub location: ParameterLocation,
    pub required: bool,
    pub description: String,
    /// [G5] Optional JSON Schema fragment describing the accepted shape/type of
    /// this parameter. Lets Agents validate input before invoking the CLI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParameterLocation {
    Path,
    Query,
    Header,
    Body,
    Flag,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandLayer {
    HighLevel,
    LowLevel,
    Meta,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RelatedCommands {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low_level: Option<Vec<String>>,
}

/// Trait：可自描述的命令
pub trait Describable {
    fn describe(&self) -> CommandDescription;
}

// [G5] Default impls let new optional fields land without churning every
// existing struct literal across the handler crates.
impl Default for CommandDescription {
    fn default() -> Self {
        Self {
            command: String::new(),
            layer: CommandLayer::HighLevel,
            api: None,
            description: String::new(),
            risk_level: RiskLevel::Low,
            supports_dry_run: false,
            supports_pipe: false,
            parameters: None,
            scenario_routing: None,
            related_commands: None,
            low_level_apis: None,
            output_filter_examples: None,
            shell_quoting_tips: None,
            wraps_apis: None,
        }
    }
}

impl Default for CommandParameter {
    fn default() -> Self {
        Self {
            name: String::new(),
            location: ParameterLocation::Flag,
            required: false,
            description: String::new(),
            schema: None,
        }
    }
}

impl CommandDescription {
    /// [G5] After populating `low_level_apis`, mirror it into the spec-mandated
    /// `wraps_apis` alias. Call this from registry/handler code instead of
    /// duplicating the list manually.
    pub fn mirror_apis(mut self) -> Self {
        if self.wraps_apis.is_none() {
            self.wraps_apis = self.low_level_apis.clone();
        }
        self
    }
}
