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

#[derive(Debug, Serialize, Deserialize)]
pub struct DryRunResult {
    pub action: String,
    pub dry_run: bool,
    pub impact: Impact,
    pub plan: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm_command: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Impact {
    pub affected_objects: u64,
    pub affected_bytes: u64,
    pub risk_level: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_duration: Option<String>,
    /// [Review Fix #m3] Total objects scanned during the dry-run preview
    /// (i.e. the number of `ListObjects`/`ListVersions` entries actually
    /// inspected). When `preview_truncated` is `true`, this is the
    /// `MAX_PREVIEW_OBJECTS` cap and the real impact may be larger.
    /// Skipped in serialization when not provided to keep older payloads
    /// byte-identical for read-only / non-listing commands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scanned_count: Option<u64>,
    /// [Review Fix #m3] `true` when the listing was truncated at
    /// `MAX_PREVIEW_OBJECTS`; Agents must treat `affected_objects` and
    /// `affected_bytes` as lower bounds in that case.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_truncated: Option<bool>,
}
