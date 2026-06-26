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

use schemars::JsonSchema;
use serde::Serialize;

/// CLI 命令的元数据（用于 --help-json 输出）。
#[derive(Debug, Serialize, JsonSchema)]
pub struct CommandMeta {
    /// 命令全限定名称（如 "tos bucket create"）。
    pub name: String,
    /// 一行简短描述。
    pub summary: String,
    /// 命令的详细描述。
    pub description: String,
    /// 可用的子命令。
    pub subcommands: Vec<SubcommandMeta>,
    /// 该命令接受的参数。
    pub args: Vec<ArgMeta>,
    /// 用法示例。
    pub examples: Vec<Example>,
}

/// 子命令的元数据。
#[derive(Debug, Serialize, JsonSchema)]
pub struct SubcommandMeta {
    /// 子命令名称。
    pub name: String,
    /// 简短描述。
    pub summary: String,
    /// 子命令的参数。
    pub args: Vec<ArgMeta>,
    /// 用法示例。
    pub examples: Vec<Example>,
}

/// 单个 CLI 参数的元数据。
#[derive(Debug, Serialize, JsonSchema)]
pub struct ArgMeta {
    /// 参数名称（长格式，不含 --）。
    pub name: String,
    /// 短标志（单字符，如有）。
    pub short: Option<char>,
    /// 参数描述。
    pub description: String,
    /// 是否必需。
    pub required: bool,
    /// 默认值（如有）。
    pub default: Option<String>,
    /// 可设置该参数的环境变量。
    pub env_var: Option<String>,
    /// 类型提示（string、integer、boolean 等）。
    pub value_type: String,
    /// 可选值（用于枚举类型）。
    pub possible_values: Vec<String>,
}

/// 文档中的用法示例。
#[derive(Debug, Serialize, JsonSchema)]
pub struct Example {
    /// 示例的简短标题。
    pub title: String,
    /// 要运行的 CLI 命令。
    pub command: String,
    /// 命令功能说明。
    pub description: String,
}

/// 生成命令帮助元数据的 JSON 表示。
pub fn generate_help_json(meta: &CommandMeta) -> anyhow::Result<String> {
    let json = serde_json::to_string_pretty(meta)?;
    Ok(json)
}
