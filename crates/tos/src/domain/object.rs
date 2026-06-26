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

// [Review Fix #FmtUni-Phase2] Typed list/list-versions handlers for `ve-tos object`.
//
// These mirror `domain::bucket::list_buckets` so the four "list-like" raw-API
// commands (object list, object list-versions, multipart list, multipart
// list-parts) all share the same shape:
//   - typed struct with `#[serde(rename_all(deserialize = "PascalCase"))]`
//     so the TOS XML is accepted while CLI output stays in snake_case.
//   - `Envelope::success(...).with_pagination(PaginationInfo { next_token, total_returned })`
//     so the unified renderer can derive the standard `Total: N (next_token=...)`
//     footer for table/csv views.
//   - returns `Result<Envelope<TypedResponse>, CliError>` so handlers can call
//     `output_result_with_columns(global, &result, Some(COLUMNS))` for declarative
//     column ordering.

use std::collections::BTreeMap;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use tos_core::agent::envelope::{Envelope, PaginationInfo};
use tos_core::agent::error::CliError;
use tos_core::infra::client::TosClient;

use crate::domain::core::extract_request_id;

// ===== ListObjects (V2) =====

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct ListObjectsResponse {
    pub name: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub delimiter: String,
    #[serde(default)]
    pub max_keys: u32,
    #[serde(default)]
    pub key_count: u32,
    #[serde(default)]
    pub is_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_continuation_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_type: Option<String>,
    #[serde(default, alias = "Contents", alias = "contents")]
    pub contents: Vec<ObjectInfo>,
    #[serde(default, alias = "CommonPrefixes", alias = "common_prefixes")]
    pub common_prefixes: Vec<CommonPrefix>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct ObjectInfo {
    pub key: String,
    #[serde(default)]
    pub last_modified: String,
    // [Review Fix #ETag-Casing] TOS/S3 XML 用 <ETag>（双大写），serde 的 PascalCase
    // 算法会把 `etag` 映射为 "Etag" 而非 "ETag"，导致反序列化丢字段、列值为空。
    // 显式 rename + alias 兼容 XML/JSON/已转 snake_case 三种载荷形态。
    #[serde(default, rename(deserialize = "ETag"), alias = "etag")]
    pub etag: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub storage_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_crc64ecma: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub object_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ObjectOwner>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct CommonPrefix {
    pub prefix: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct ObjectOwner {
    #[serde(default, rename(deserialize = "ID"))]
    pub id: String,
    #[serde(default)]
    pub display_name: String,
}

// ===== ListVersions =====

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct ListVersionsResponse {
    pub name: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub delimiter: String,
    #[serde(default)]
    pub max_keys: u32,
    #[serde(default)]
    pub is_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_marker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id_marker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_key_marker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_version_id_marker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_type: Option<String>,
    #[serde(default, alias = "Version", alias = "Versions", alias = "versions")]
    pub versions: Vec<ObjectVersion>,
    #[serde(
        default,
        alias = "DeleteMarker",
        alias = "DeleteMarkers",
        alias = "delete_markers"
    )]
    pub delete_markers: Vec<DeleteMarker>,
    #[serde(default, alias = "CommonPrefixes", alias = "common_prefixes")]
    pub common_prefixes: Vec<CommonPrefix>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct ObjectVersion {
    pub key: String,
    #[serde(default)]
    pub version_id: String,
    #[serde(default)]
    pub is_latest: bool,
    #[serde(default)]
    pub last_modified: String,
    // [Review Fix #ETag-Casing] 同 ObjectInfo.etag：TOS XML 用 <ETag>（双大写）。
    #[serde(default, rename(deserialize = "ETag"), alias = "etag")]
    pub etag: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub storage_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_crc64ecma: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct DeleteMarker {
    pub key: String,
    #[serde(default)]
    pub version_id: String,
    #[serde(default)]
    pub is_latest: bool,
    #[serde(default)]
    pub last_modified: String,
}

// ===== API impls =====

/// ListObjectsV2 - 列举对象（XML 响应反序列化为 typed schema）
pub async fn list_objects(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    delimiter: Option<&str>,
    max_keys: u32,
    continuation_token: Option<&str>,
) -> Result<Envelope<ListObjectsResponse>, CliError> {
    let url = client.bucket_endpoint(bucket)?;
    let path = client.bucket_request_path(bucket)?;

    let mut query: BTreeMap<String, String> = BTreeMap::new();
    query.insert("list-type".to_string(), "2".to_string());
    if let Some(p) = prefix {
        if !p.is_empty() {
            query.insert("prefix".to_string(), p.to_string());
        }
    }
    if let Some(d) = delimiter {
        if !d.is_empty() {
            query.insert("delimiter".to_string(), d.to_string());
        }
    }
    query.insert("max-keys".to_string(), max_keys.to_string());
    if let Some(t) = continuation_token {
        if !t.is_empty() {
            query.insert("continuation-token".to_string(), t.to_string());
        }
    }

    let resp = client
        .send_request(Method::GET, &url, &path, query, BTreeMap::new(), None)
        .await?;

    let request_id = extract_request_id(&resp);
    let resp = client.check_response(resp).await?;
    let body = resp.text().await.map_err(CliError::Http)?;

    let data: ListObjectsResponse = parse_list_response(&body, "ListObjectsV2")?;

    let next_token = if data.is_truncated {
        data.next_continuation_token.clone()
    } else {
        None
    };
    let total = data.contents.len() as u64;

    Ok(Envelope::success("ve-tos object list", data)
        .with_request_id(request_id)
        .with_pagination(PaginationInfo {
            next_token,
            next_marker: None,
            total_returned: total,
        }))
}

/// ListObjectVersions - 列举对象版本（含 delete-marker）
pub async fn list_object_versions(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
) -> Result<Envelope<ListVersionsResponse>, CliError> {
    let url = client.bucket_endpoint(bucket)?;
    let path = client.bucket_request_path(bucket)?;

    let mut query: BTreeMap<String, String> = BTreeMap::new();
    query.insert("versions".to_string(), String::new());
    if let Some(p) = prefix {
        if !p.is_empty() {
            query.insert("prefix".to_string(), p.to_string());
        }
    }

    let resp = client
        .send_request(Method::GET, &url, &path, query, BTreeMap::new(), None)
        .await?;

    let request_id = extract_request_id(&resp);
    let resp = client.check_response(resp).await?;
    let body = resp.text().await.map_err(CliError::Http)?;

    let data: ListVersionsResponse = parse_list_response(&body, "ListObjectVersions")?;

    let next_token = if data.is_truncated {
        data.next_key_marker.clone()
    } else {
        None
    };
    let total = (data.versions.len() + data.delete_markers.len()) as u64;

    Ok(Envelope::success("ve-tos object list-versions", data)
        .with_request_id(request_id)
        .with_pagination(PaginationInfo {
            next_token,
            next_marker: None,
            total_returned: total,
        }))
}

/// [Review Fix #FmtUni-Phase2] Content-aware deserializer.
///
/// TOS list-style endpoints return either XML (S3-compatible signing path) or
/// JSON (internal/v2 path) depending on bucket type and SDK version. We sniff
/// the first non-whitespace byte to dispatch:
///   - `<` → XML via `quick_xml::de`
///   - otherwise → JSON via `serde_json` (covers `{`, `[`, and even raw text)
///
/// Both branches feed the same typed struct because the struct uses
/// `#[serde(rename_all(deserialize = "PascalCase"))]` plus explicit aliases
/// for repeated/array fields whose XML element name differs from the JSON
/// array name (e.g. XML `<Contents>...</Contents>` vs JSON `"Contents":[...]`).
fn parse_list_response<T: for<'de> Deserialize<'de>>(body: &str, api: &str) -> Result<T, CliError> {
    let trimmed = body.trim_start();
    let result = if trimmed.starts_with('<') {
        quick_xml::de::from_str::<T>(body).map_err(|e| format!("xml: {}", e))
    } else {
        serde_json::from_str::<T>(body).map_err(|e| format!("json: {}", e))
    };
    result.map_err(|e| {
        let preview = &body[..200.min(body.len())];
        CliError::Unknown(format!(
            "Failed to parse {} response: {} -- body: {}",
            api, e, preview
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_objects_parses_xml_with_repeated_contents() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
    <Name>my-bucket</Name>
    <Prefix></Prefix>
    <KeyCount>2</KeyCount>
    <MaxKeys>1000</MaxKeys>
    <IsTruncated>false</IsTruncated>
    <Contents>
        <Key>a.txt</Key>
        <LastModified>2024-01-01T00:00:00.000Z</LastModified>
        <ETag>"etag-a"</ETag>
        <Size>10</Size>
        <StorageClass>STANDARD</StorageClass>
    </Contents>
    <Contents>
        <Key>b.txt</Key>
        <LastModified>2024-01-02T00:00:00.000Z</LastModified>
        <ETag>"etag-b"</ETag>
        <Size>20</Size>
        <StorageClass>IA</StorageClass>
    </Contents>
</ListBucketResult>"#;
        let parsed: ListObjectsResponse =
            quick_xml::de::from_str(xml).expect("parse ListObjectsResponse");
        assert_eq!(parsed.name, "my-bucket");
        assert_eq!(parsed.contents.len(), 2);
        assert_eq!(parsed.contents[0].key, "a.txt");
        assert_eq!(parsed.contents[1].size, 20);
        assert!(!parsed.is_truncated);
        // [Review Fix #ETag-Casing] 锁定 <ETag> 双大写元素能映射到 etag 字段。
        assert_eq!(parsed.contents[0].etag, "\"etag-a\"");
        assert_eq!(parsed.contents[1].etag, "\"etag-b\"");
    }

    #[test]
    fn list_objects_response_serializes_snake_case() {
        let resp = ListObjectsResponse {
            name: "b".into(),
            prefix: String::new(),
            delimiter: String::new(),
            max_keys: 1,
            key_count: 1,
            is_truncated: false,
            continuation_token: None,
            next_continuation_token: None,
            encoding_type: None,
            contents: vec![ObjectInfo {
                key: "k".into(),
                last_modified: "t".into(),
                etag: "e".into(),
                size: 1,
                storage_class: "STANDARD".into(),
                hash_crc64ecma: None,
                object_type: None,
                owner: None,
            }],
            common_prefixes: vec![],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("max_keys").is_some(), "expected snake_case key");
        assert!(json.get("is_truncated").is_some());
        let item = &json["contents"][0];
        assert!(item.get("storage_class").is_some());
        assert!(item.get("last_modified").is_some());
    }

    #[test]
    fn list_versions_parses_xml_with_versions_and_delete_markers() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListVersionsResult>
    <Name>my-bucket</Name>
    <MaxKeys>1000</MaxKeys>
    <IsTruncated>false</IsTruncated>
    <Version>
        <Key>a.txt</Key>
        <VersionId>v1</VersionId>
        <IsLatest>true</IsLatest>
        <LastModified>2024-01-01T00:00:00.000Z</LastModified>
        <ETag>"e1"</ETag>
        <Size>10</Size>
        <StorageClass>STANDARD</StorageClass>
    </Version>
    <DeleteMarker>
        <Key>b.txt</Key>
        <VersionId>v2</VersionId>
        <IsLatest>true</IsLatest>
        <LastModified>2024-01-02T00:00:00.000Z</LastModified>
    </DeleteMarker>
</ListVersionsResult>"#;
        let parsed: ListVersionsResponse =
            quick_xml::de::from_str(xml).expect("parse ListVersionsResponse");
        assert_eq!(parsed.versions.len(), 1);
        assert_eq!(parsed.delete_markers.len(), 1);
        assert_eq!(parsed.versions[0].version_id, "v1");
        assert_eq!(parsed.delete_markers[0].key, "b.txt");
        // [Review Fix #ETag-Casing] 锁定 ObjectVersion <ETag> 不丢字段。
        assert_eq!(parsed.versions[0].etag, "\"e1\"");
    }
}
