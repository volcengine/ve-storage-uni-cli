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

use serde::Serialize;
use crate::agent::output::Outputable;

/// Object information returned by list/head operations.
#[derive(Debug, Clone, Serialize)]
pub struct ObjectInfo {
    /// Object key.
    pub key: String,
    /// Object size in bytes.
    pub size: u64,
    /// Last modified time (ISO 8601).
    pub last_modified: String,
    /// ETag (content hash).
    pub etag: String,
    /// Storage class.
    pub storage_class: String,
    /// Content type (MIME).
    pub content_type: Option<String>,
    /// Version ID (if versioning enabled).
    pub version_id: Option<String>,
}

impl Outputable for ObjectInfo {
    fn headers() -> Vec<String> {
        vec![
            "Key".into(),
            "Size".into(),
            "Last Modified".into(),
            "ETag".into(),
            "Storage Class".into(),
        ]
    }

    fn row(&self) -> Vec<String> {
        vec![
            self.key.clone(),
            self.size.to_string(),
            self.last_modified.clone(),
            self.etag.clone(),
            self.storage_class.clone(),
        ]
    }
}
