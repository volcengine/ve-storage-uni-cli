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

use super::describe::RiskLevel;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct CapabilitiesOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high_level: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low_level: Option<std::collections::HashMap<String, GroupInfo>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GroupInfo {
    pub commands: u32,
    pub description: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub query: String,
    pub results: Vec<SearchHit>,
    pub total: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchHit {
    pub command: String,
    pub layer: String,
    pub match_field: String,
    pub snippet: String,
    pub risk_level: RiskLevel,
}
