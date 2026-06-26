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

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crc64fast::Digest;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::client::Error;
use super::rate_limiter::RateLimiter;

#[derive(Debug, Clone, Default)]
pub struct ResponseInfo {
    pub request_id: String,
    pub status_code: u16,
    pub headers: HashMap<String, String>,
}

impl ResponseInfo {
    pub fn request_id(&self) -> &str {
        &self.request_id
    }

    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(key).map(String::as_str)
    }
}

pub enum Body {
    Bytes(Vec<u8>),
    File(std::path::PathBuf),
}

impl Body {
    pub fn from_bytes(data: impl Into<Vec<u8>>) -> Self {
        Self::Bytes(data.into())
    }

    pub fn from_file(path: impl Into<std::path::PathBuf>) -> Self {
        Self::File(path.into())
    }

    pub fn content_length(&self) -> Option<u64> {
        match self {
            Self::Bytes(bytes) => Some(bytes.len() as u64),
            Self::File(path) => std::fs::metadata(path).ok().map(|meta| meta.len()),
        }
    }

    pub async fn into_bytes(self, content_length: Option<u64>) -> Result<Vec<u8>, Error> {
        let bytes = match self {
            Self::Bytes(bytes) => bytes,
            Self::File(path) => tokio::fs::read(path).await.map_err(Error::HttpBody)?,
        };
        if let Some(expected_length) = content_length {
            let actual_length = bytes.len() as u64;
            // [Review Fix #1] Do not silently truncate upload bodies; mismatched lengths corrupt remote content.
            if actual_length != expected_length {
                return Err(Error::Client(format!(
                    "body length mismatch: expected {expected_length} bytes, got {actual_length} bytes"
                )));
            }
        }
        Ok(bytes)
    }
}

impl From<Vec<u8>> for Body {
    fn from(data: Vec<u8>) -> Self {
        Self::Bytes(data)
    }
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InstanceInfo {
    #[serde(rename = "InstanceID", default)]
    pub instance_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub run_state: String,
    #[serde(default)]
    pub space_count: i64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetInstanceInput {
    #[serde(skip_serializing)]
    pub instance: String,
}

impl GetInstanceInput {
    pub fn new(instance: impl Into<String>) -> Self {
        Self {
            instance: instance.into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetInstanceByNameInput {
    #[serde(skip_serializing)]
    pub name: String,
}

impl GetInstanceByNameInput {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }

    pub fn to_query_pairs(&self) -> Vec<(String, String)> {
        vec![("name".to_string(), self.name.clone())]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetInstanceOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    pub instance: InstanceInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct CreateInstanceInput {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub meta: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateInstanceOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    pub instance: InstanceInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListInstancesInput {
    #[serde(skip_serializing)]
    pub limit: Option<i32>,
    #[serde(skip_serializing)]
    pub marker: Option<String>,
}

impl ListInstancesInput {
    pub fn new() -> Self {
        Self {
            limit: None,
            marker: None,
        }
    }

    pub fn with_limit(mut self, limit: i32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn to_query_pairs(&self) -> Vec<(String, String)> {
        pagination_query(self.limit, self.marker.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListInstancesOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(default)]
    pub instances: Vec<InstanceInfo>,
    #[serde(default)]
    pub next_marker: String,
    #[serde(default)]
    pub is_truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteInstanceInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
}

impl DeleteInstanceInput {
    pub fn new(instance_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeleteInstanceOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SpaceInfo {
    #[serde(rename = "InstanceID", default)]
    pub instance_id: String,
    #[serde(rename = "SpaceID", default)]
    pub space_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub owner_type: String,
    #[serde(default, rename = "OwnerId")]
    pub owner_id: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetSpaceInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space: String,
}

impl GetSpaceInput {
    pub fn new(instance_id: impl Into<String>, space: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            space: space.into(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GetSpaceByNameInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub instance_name: String,
    #[serde(skip_serializing)]
    pub space_name: String,
}

impl GetSpaceByNameInput {
    pub fn new_with_instance_id(
        instance_id: impl Into<String>,
        space_name: impl Into<String>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            instance_name: String::new(),
            space_name: space_name.into(),
        }
    }

    pub fn new_with_instance_name(
        instance_name: impl Into<String>,
        space_name: impl Into<String>,
    ) -> Self {
        Self {
            instance_id: String::new(),
            instance_name: instance_name.into(),
            space_name: space_name.into(),
        }
    }

    pub fn to_query_pairs(&self) -> Vec<(String, String)> {
        let mut query = vec![("spaceName".to_string(), self.space_name.clone())];
        if !self.instance_id.is_empty() {
            query.push(("instanceId".to_string(), self.instance_id.clone()));
        } else if !self.instance_name.is_empty() {
            query.push(("instanceName".to_string(), self.instance_name.clone()));
        }
        query
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetSpaceOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    pub space: SpaceInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct CreateSpaceInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    pub space_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub index_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateSpaceOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    pub space: SpaceInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSpacesInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub limit: Option<i32>,
    #[serde(skip_serializing)]
    pub marker: Option<String>,
}

impl ListSpacesInput {
    pub fn new(instance_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            limit: None,
            marker: None,
        }
    }

    pub fn with_limit(mut self, limit: i32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn to_query_pairs(&self) -> Vec<(String, String)> {
        pagination_query(self.limit, self.marker.as_deref())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListSpacesOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(default)]
    pub spaces: Vec<SpaceInfo>,
    #[serde(default)]
    pub next_marker: String,
    #[serde(default)]
    pub is_truncated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteSpaceInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space_id: String,
}

impl DeleteSpaceInput {
    pub fn new(instance_id: impl Into<String>, space_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            space_id: space_id.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeleteSpaceOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct FileInfo {
    #[serde(
        rename = "InstanceID",
        alias = "InstanceId",
        alias = "instance_id",
        alias = "instanceId",
        default
    )]
    pub instance_id: String,
    #[serde(
        rename = "SpaceID",
        alias = "SpaceId",
        alias = "space_id",
        alias = "spaceId",
        default
    )]
    pub space_id: String,
    #[serde(
        alias = "file_path",
        alias = "filePath",
        alias = "Path",
        alias = "path",
        default
    )]
    pub file_path: String,
    #[serde(
        rename = "FileType",
        alias = "file_type",
        alias = "fileType",
        alias = "Type",
        alias = "type",
        default
    )]
    pub file_type: String,
    #[serde(alias = "storage_class", alias = "storageClass", default)]
    pub storage_class: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub meta: HashMap<String, String>,
    #[serde(
        alias = "HashCRC64ECMA",
        alias = "hash_crc64_ecma",
        alias = "hashCRC64ECMA",
        alias = "hashCrc64Ecma",
        default
    )]
    pub hash_crc64_ecma: u64,
    #[serde(
        alias = "size",
        alias = "FileSize",
        alias = "file_size",
        alias = "fileSize",
        alias = "ContentLength",
        alias = "content_length",
        alias = "contentLength",
        default
    )]
    pub size: i64,
    #[serde(alias = "ETag", alias = "etag", default)]
    pub etag: String,
    #[serde(alias = "created_at", alias = "createdAt", default)]
    pub created_at: i64,
    #[serde(alias = "updated_at", alias = "updatedAt", default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct FolderInfo {
    #[serde(
        alias = "folder",
        alias = "FilePath",
        alias = "file_path",
        alias = "filePath",
        default
    )]
    pub folder: String,
    #[serde(alias = "updated_at", alias = "updatedAt", default)]
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListFilesInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space_id: String,
    #[serde(skip_serializing)]
    pub prefix: Option<String>,
    #[serde(skip_serializing)]
    pub delimiter: Option<String>,
    #[serde(skip_serializing)]
    pub limit: Option<i32>,
    #[serde(skip_serializing)]
    pub marker: Option<String>,
}

impl ListFilesInput {
    pub fn new(instance_id: impl Into<String>, space_id: impl Into<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            space_id: space_id.into(),
            prefix: None,
            delimiter: None,
            limit: None,
            marker: None,
        }
    }

    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    pub fn with_delimiter(mut self, delimiter: impl Into<String>) -> Self {
        self.delimiter = Some(delimiter.into());
        self
    }

    pub fn with_limit(mut self, limit: i32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn with_marker(mut self, marker: impl Into<String>) -> Self {
        self.marker = Some(marker.into());
        self
    }

    pub fn to_query_pairs(&self) -> Vec<(String, String)> {
        let mut query = pagination_query(self.limit, self.marker.as_deref());
        if let Some(prefix) = &self.prefix {
            query.push(("prefix".to_string(), prefix.clone()));
        }
        if let Some(delimiter) = &self.delimiter {
            query.push(("delimiter".to_string(), delimiter.clone()));
        }
        query
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ListFilesOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(alias = "next_marker", alias = "nextMarker", default)]
    pub next_marker: String,
    #[serde(alias = "is_truncated", alias = "isTruncated", default)]
    pub is_truncated: bool,
    #[serde(alias = "folders", default)]
    pub folders: Vec<FolderInfo>,
    #[serde(alias = "files", default)]
    pub files: Vec<FileInfo>,
}

pub struct PutFileInput {
    pub instance_id: String,
    pub space_id: String,
    pub file_path: String,
    pub body: Body,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub auto_index: Option<bool>,
    pub meta: Option<HashMap<String, String>>,
    pub rate_limiter: Option<Arc<RateLimiter>>,
}

impl PutFileInput {
    pub fn new(
        instance_id: impl Into<String>,
        space_id: impl Into<String>,
        file_path: impl Into<String>,
        body: impl Into<Body>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            space_id: space_id.into(),
            file_path: file_path.into(),
            body: body.into(),
            content_type: None,
            content_length: None,
            auto_index: None,
            meta: None,
            rate_limiter: None,
        }
    }

    pub fn with_content_length(mut self, content_length: u64) -> Self {
        self.content_length = Some(content_length);
        self
    }

    pub fn with_rate_limiter(mut self, rate_limiter: impl Into<Arc<RateLimiter>>) -> Self {
        self.rate_limiter = Some(rate_limiter.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PutFileOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(rename = "InstanceID", default)]
    pub instance_id: String,
    #[serde(rename = "SpaceID", default)]
    pub space_id: String,
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub etag: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub meta: HashMap<String, String>,
    #[serde(default)]
    pub hash_crc64_ecma: u64,
    #[serde(default)]
    pub version_id: String,
}

#[derive(Debug, Clone, Default)]
pub struct GetFileInput {
    pub instance_id: String,
    pub space_id: String,
    pub file_path: String,
    pub range_raw: Option<String>,
    pub if_match: Option<String>,
    pub rate_limiter: Option<Arc<RateLimiter>>,
}

impl GetFileInput {
    pub fn new(
        instance_id: impl Into<String>,
        space_id: impl Into<String>,
        file_path: impl Into<String>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            space_id: space_id.into(),
            file_path: file_path.into(),
            range_raw: None,
            if_match: None,
            rate_limiter: None,
        }
    }

    pub fn with_range_raw(mut self, raw: impl Into<String>) -> Self {
        self.range_raw = Some(raw.into());
        self
    }
}

pub struct GetFileOutput {
    pub response_info: ResponseInfo,
    pub content_length: i64,
    pub content_type: String,
    pub content_range: Option<String>,
    pub etag: String,
    pub hash_crc64_ecma: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub file_type: String,
    pub storage_class: String,
    pub meta: HashMap<String, String>,
    pub is_folder: bool,
    content: Pin<Box<dyn Stream<Item = std::io::Result<Vec<u8>>> + Send>>,
    crc_state: Option<Digest>,
}

impl GetFileOutput {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        response_info: ResponseInfo,
        content_length: i64,
        content_type: String,
        content_range: Option<String>,
        etag: String,
        hash_crc64_ecma: u64,
        created_at: i64,
        updated_at: i64,
        file_type: String,
        storage_class: String,
        meta: HashMap<String, String>,
        is_folder: bool,
        content: Pin<Box<dyn Stream<Item = std::io::Result<Vec<u8>>> + Send>>,
    ) -> Self {
        // [Review Fix #5] Preserve SDK behavior: verify CRC64 for full downloads when IDS provides it.
        let crc_state = if content_range.is_none() && hash_crc64_ecma != 0 {
            Some(Digest::new())
        } else {
            None
        };

        Self {
            response_info,
            content_length,
            content_type,
            content_range,
            etag,
            hash_crc64_ecma,
            created_at,
            updated_at,
            file_type,
            storage_class,
            meta,
            is_folder,
            content,
            crc_state,
        }
    }

    pub async fn read_all(mut self) -> Result<Vec<u8>, Error> {
        let mut buffer = if self.content_length > 0 {
            Vec::with_capacity(self.content_length as usize)
        } else {
            Vec::new()
        };
        while let Some(chunk) = futures::StreamExt::next(&mut self).await {
            buffer.extend_from_slice(&chunk.map_err(Error::HttpBody)?);
        }
        Ok(buffer)
    }
}

impl Stream for GetFileOutput {
    type Item = std::io::Result<Vec<u8>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.content.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if let Some(state) = &mut self.crc_state {
                    let _ = state.write(&chunk);
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(None) => {
                if let Some(state) = self.crc_state.take() {
                    let calculated = state.sum64();
                    if calculated != self.hash_crc64_ecma {
                        return Poll::Ready(Some(Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "CRC64 checksum mismatch: expected {}, got {}",
                                self.hash_crc64_ecma, calculated
                            ),
                        ))));
                    }
                }
                Poll::Ready(None)
            }
            other => other,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HeadFileInput {
    pub instance_id: String,
    pub space_id: String,
    pub file_path: String,
}

impl HeadFileInput {
    pub fn new(
        instance_id: impl Into<String>,
        space_id: impl Into<String>,
        file_path: impl Into<String>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            space_id: space_id.into(),
            file_path: file_path.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HeadFileOutput {
    pub response_info: ResponseInfo,
    pub content_length: i64,
    pub content_type: String,
    pub etag: String,
    pub hash_crc64_ecma: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub file_type: String,
    pub storage_class: String,
    pub meta: HashMap<String, String>,
    pub is_folder: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DeleteFileInput {
    pub instance_id: String,
    pub space_id: String,
    pub file_path: String,
    pub if_match: Option<String>,
}

impl DeleteFileInput {
    pub fn new(
        instance_id: impl Into<String>,
        space_id: impl Into<String>,
        file_path: impl Into<String>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            space_id: space_id.into(),
            file_path: file_path.into(),
            if_match: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DeleteFileOutput {
    pub response_info: ResponseInfo,
    pub version_id: String,
    pub delete_marker: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RenameFileInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space_id: String,
    #[serde(skip_serializing)]
    pub file_path: String,
    pub new_file_path: String,
    #[serde(default)]
    pub forbid_overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RenameFileOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(default)]
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CopyFileInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space_id: String,
    #[serde(skip_serializing)]
    pub file_path: String,
    pub copy_to_space_id: String,
    pub copy_to_path: String,
    #[serde(skip_serializing)]
    pub copy_source_if_match: Option<String>,
    #[serde(default)]
    pub auto_rename: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CopyFileOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(default)]
    pub copy_to_file_path: String,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateFolderInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space_id: String,
    pub folder_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateFolderOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(rename = "InstanceID", default)]
    pub instance_id: String,
    #[serde(rename = "SpaceID", default)]
    pub space_id: String,
    #[serde(default)]
    pub folder_path: String,
    #[serde(default)]
    pub created_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct DeleteFolderInput {
    pub instance_id: String,
    pub space_id: String,
    pub folder_path: String,
}

impl DeleteFolderInput {
    pub fn new(
        instance_id: impl Into<String>,
        space_id: impl Into<String>,
        folder_path: impl Into<String>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            space_id: space_id.into(),
            folder_path: folder_path.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeleteFolderOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(default)]
    pub folder_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RenameFolderInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space_id: String,
    #[serde(skip_serializing)]
    pub folder_path: String,
    pub new_folder_path: String,
    #[serde(default)]
    pub forbid_overwrite: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RenameFolderOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(default)]
    pub folder_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InitiateMultipartUploadInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space_id: String,
    #[serde(skip_serializing)]
    pub file_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InitiateMultipartUploadOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(rename = "InstanceID", default)]
    pub instance_id: String,
    #[serde(rename = "SpaceID", default)]
    pub space_id: String,
    #[serde(default)]
    pub file_path: String,
    pub upload_id: String,
}

pub struct UploadPartInput {
    pub instance_id: String,
    pub space_id: String,
    pub file_path: String,
    pub upload_id: String,
    pub part_number: i32,
    pub body: Body,
    pub content_length: Option<u64>,
    pub rate_limiter: Option<Arc<RateLimiter>>,
}

impl UploadPartInput {
    pub fn new(
        instance_id: impl Into<String>,
        space_id: impl Into<String>,
        file_path: impl Into<String>,
        upload_id: impl Into<String>,
        part_number: i32,
        body: impl Into<Body>,
    ) -> Self {
        Self {
            instance_id: instance_id.into(),
            space_id: space_id.into(),
            file_path: file_path.into(),
            upload_id: upload_id.into(),
            part_number,
            body: body.into(),
            content_length: None,
            rate_limiter: None,
        }
    }

    pub fn with_content_length(mut self, content_length: u64) -> Self {
        self.content_length = Some(content_length);
        self
    }

    pub fn with_rate_limiter(mut self, rate_limiter: impl Into<Arc<RateLimiter>>) -> Self {
        self.rate_limiter = Some(rate_limiter.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UploadPartOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(default)]
    pub file_path: String,
    pub part_number: i32,
    #[serde(rename = "ETag")]
    pub etag: String,
    #[serde(default)]
    pub hash_crc64_ecma: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PartInfo {
    pub part_number: i32,
    #[serde(rename = "ETag")]
    pub etag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CompleteMultipartUploadInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space_id: String,
    #[serde(skip_serializing)]
    pub file_path: String,
    pub upload_id: String,
    pub parts: Vec<PartInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AbortMultipartUploadInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space_id: String,
    #[serde(skip_serializing)]
    pub file_path: String,
    #[serde(skip_serializing)]
    pub upload_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CompleteMultipartUploadOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(rename = "InstanceID", default)]
    pub instance_id: String,
    #[serde(rename = "SpaceID", default)]
    pub space_id: String,
    #[serde(default)]
    pub file_path: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub etag: String,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub meta: HashMap<String, String>,
    #[serde(default)]
    pub hash_crc64_ecma: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SearchFilesInput {
    #[serde(skip_serializing)]
    pub instance_id: String,
    #[serde(skip_serializing)]
    pub space_id: String,
    pub query: String,
    pub top_k: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<HashMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalize: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchResultItem {
    pub file_path: String,
    #[serde(default)]
    pub distance: f64,
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub snippet: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub meta: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchFilesOutput {
    #[serde(skip)]
    pub response_info: ResponseInfo,
    #[serde(default)]
    pub results: Vec<SearchResultItem>,
}

fn pagination_query(limit: Option<i32>, marker: Option<&str>) -> Vec<(String, String)> {
    let mut query = Vec::new();
    if let Some(limit) = limit {
        query.push(("limit".to_string(), limit.to_string()));
    }
    if let Some(marker) = marker {
        query.push(("marker".to_string(), marker.to_string()));
    }
    query
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crc64(bytes: &[u8]) -> u64 {
        let mut digest = Digest::new();
        let _ = digest.write(bytes);
        digest.sum64()
    }

    fn output_with_crc(bytes: Vec<u8>, expected_crc: u64) -> GetFileOutput {
        GetFileOutput::new(
            ResponseInfo::default(),
            bytes.len() as i64,
            "application/octet-stream".to_string(),
            None,
            String::new(),
            expected_crc,
            0,
            0,
            String::new(),
            String::new(),
            HashMap::new(),
            false,
            Box::pin(futures::stream::iter(vec![Ok::<Vec<u8>, std::io::Error>(
                bytes,
            )])),
        )
    }

    #[test]
    fn file_info_accepts_file_type_aliases() {
        for (field, expected) in [
            ("FileType", "text"),
            ("file_type", "image"),
            ("fileType", "video"),
            ("Type", "document"),
            ("type", "archive"),
        ] {
            let mut payload = serde_json::Map::new();
            payload.insert("FilePath".to_string(), serde_json::json!("docs/a.txt"));
            payload.insert(field.to_string(), serde_json::json!(expected));
            let file: FileInfo = serde_json::from_value(serde_json::Value::Object(payload))
                .expect("parse file info");

            assert_eq!(file.file_type, expected, "field={field}");
        }
    }

    #[test]
    fn file_info_accepts_common_service_field_aliases() {
        let file: FileInfo = serde_json::from_value(serde_json::json!({
            "instance_id": "inst",
            "space_id": "space",
            "file_path": "docs/a.txt",
            "fileType": "file",
            "storageClass": "standard",
            "hashCRC64ECMA": 12345,
            "fileSize": 678,
            "etag": "etag-1",
            "createdAt": 111,
            "updatedAt": 222
        }))
        .expect("parse file info");

        assert_eq!(file.instance_id, "inst");
        assert_eq!(file.space_id, "space");
        assert_eq!(file.file_path, "docs/a.txt");
        assert_eq!(file.file_type, "file");
        assert_eq!(file.storage_class, "standard");
        assert_eq!(file.hash_crc64_ecma, 12345);
        assert_eq!(file.size, 678);
        assert_eq!(file.etag, "etag-1");
        assert_eq!(file.created_at, 111);
        assert_eq!(file.updated_at, 222);
    }

    #[test]
    fn list_files_output_accepts_lowercase_service_fields() {
        let output: ListFilesOutput = serde_json::from_value(serde_json::json!({
            "next_marker": "next-1",
            "is_truncated": true,
            "folders": [{"folder": "docs/", "updated_at": 333}],
            "files": [{
                "file_path": "docs/a.txt",
                "file_type": "file",
                "size": 5,
                "etag": "etag-2"
            }]
        }))
        .expect("parse list files output");

        assert_eq!(output.next_marker, "next-1");
        assert!(output.is_truncated);
        assert_eq!(output.folders[0].folder, "docs/");
        assert_eq!(output.folders[0].updated_at, 333);
        assert_eq!(output.files[0].file_path, "docs/a.txt");
        assert_eq!(output.files[0].size, 5);
        assert_eq!(output.files[0].etag, "etag-2");
    }

    #[test]
    fn by_name_inputs_use_sdk_query_keys() {
        assert_eq!(
            GetInstanceByNameInput::new("inst-name").to_query_pairs(),
            vec![("name".to_string(), "inst-name".to_string())]
        );
        assert_eq!(
            GetSpaceByNameInput::new_with_instance_id("inst-id", "space-name").to_query_pairs(),
            vec![
                ("spaceName".to_string(), "space-name".to_string()),
                ("instanceId".to_string(), "inst-id".to_string())
            ]
        );
        assert_eq!(
            GetSpaceByNameInput::new_with_instance_name("inst-name", "space-name").to_query_pairs(),
            vec![
                ("spaceName".to_string(), "space-name".to_string()),
                ("instanceName".to_string(), "inst-name".to_string())
            ]
        );
    }

    #[tokio::test]
    async fn get_file_output_accepts_matching_crc() {
        let bytes = b"hello ids".to_vec();
        let output = output_with_crc(bytes.clone(), crc64(&bytes));

        assert_eq!(output.read_all().await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn get_file_output_rejects_crc_mismatch() {
        let output = output_with_crc(b"hello ids".to_vec(), 1);
        let err = output.read_all().await.unwrap_err();

        assert!(matches!(
            err,
            Error::HttpBody(io_err) if io_err.kind() == std::io::ErrorKind::InvalidData
        ));
    }
}
