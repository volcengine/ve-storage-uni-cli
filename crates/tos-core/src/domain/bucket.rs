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

/// Bucket information returned by list/head operations.
#[derive(Debug, Clone, Serialize)]
pub struct BucketInfo {
    /// Bucket name.
    pub name: String,
    /// Region where the bucket is located.
    pub region: String,
    /// Storage class (STANDARD, IA, ARCHIVE, etc.).
    pub storage_class: String,
    /// Creation time (ISO 8601).
    pub created_at: String,
    /// Bucket ACL (private, public-read, etc.).
    pub acl: String,
}

impl Outputable for BucketInfo {
    fn headers() -> Vec<String> {
        vec![
            "Name".into(),
            "Region".into(),
            "Storage Class".into(),
            "Created".into(),
            "ACL".into(),
        ]
    }

    fn row(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.region.clone(),
            self.storage_class.clone(),
            self.created_at.clone(),
            self.acl.clone(),
        ]
    }
}
