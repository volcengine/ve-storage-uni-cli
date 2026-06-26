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

/// A CLI command registered as an MCP tool.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct RegisteredCommand {
    /// Tool name (e.g., "tos_bucket_create").
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Input schema as JSON Schema.
    pub input_schema: serde_json::Value,
    /// The CLI command pattern (e.g., "tos bucket create").
    pub cli_command: String,
    /// Whether this tool performs destructive operations.
    pub destructive: bool,
    /// Required permissions/scopes.
    pub required_scopes: Vec<String>,
}

impl RegisteredCommand {
    /// Create a new registered command.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        cli_command: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            cli_command: cli_command.into(),
            destructive: false,
            required_scopes: Vec::new(),
        }
    }

    /// Mark this command as destructive.
    pub fn set_destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Set the input schema.
    pub fn with_schema(mut self, schema: serde_json::Value) -> Self {
        self.input_schema = schema;
        self
    }

    /// Add a required scope.
    pub fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.required_scopes.push(scope.into());
        self
    }
}
