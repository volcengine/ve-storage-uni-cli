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

// [Review Fix #FmtUni-Phase2] Typed multipart list/list-parts handlers.
//
// Mirror `domain::bucket::list_buckets` and `domain::object::list_objects`:
// XML deserialized via `quick_xml::de`, output keys snake_case, pagination
// driven by `IsTruncated` + next-marker fields, repeated child elements
// collapse into JSON arrays so the table renderer gets a stable Schema.

use std::collections::BTreeMap;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use tos_core::agent::envelope::{Envelope, PaginationInfo};
use tos_core::agent::error::CliError;
use tos_core::infra::client::TosClient;

use crate::domain::core::extract_request_id;
use crate::domain::object::{CommonPrefix, ObjectOwner};

// ===== ListMultipartUploads =====

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct ListMultipartUploadsResponse {
    pub bucket: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub delimiter: String,
    #[serde(default)]
    pub max_uploads: u32,
    #[serde(default)]
    pub is_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_marker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_id_marker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_key_marker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_upload_id_marker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding_type: Option<String>,
    #[serde(default, alias = "Upload", alias = "Uploads", alias = "uploads")]
    pub uploads: Vec<MultipartUploadInfo>,
    #[serde(default, alias = "CommonPrefixes", alias = "common_prefixes")]
    pub common_prefixes: Vec<CommonPrefix>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct MultipartUploadInfo {
    pub key: String,
    pub upload_id: String,
    #[serde(default)]
    pub initiated: String,
    #[serde(default)]
    pub storage_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ObjectOwner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiator: Option<ObjectOwner>,
}

// ===== ListParts =====

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct ListPartsResponse {
    pub bucket: String,
    pub key: String,
    pub upload_id: String,
    #[serde(default)]
    pub storage_class: String,
    #[serde(default)]
    pub max_parts: u32,
    #[serde(default)]
    pub is_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part_number_marker: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_part_number_marker: Option<u32>,
    #[serde(default, alias = "Part", alias = "Parts", alias = "parts")]
    pub parts: Vec<PartInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<ObjectOwner>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct PartInfo {
    pub part_number: u32,
    #[serde(default)]
    pub last_modified: String,
    // [Review Fix #ETag-Casing] TOS XML 用 <ETag>（双大写），serde 的 PascalCase
    // 算法只能映射到 "Etag"，会丢字段。显式 rename 兼容 XML/JSON 双形态。
    #[serde(default, rename(deserialize = "ETag"), alias = "etag")]
    pub etag: String,
    #[serde(default)]
    pub size: u64,
}

// ===== API impls =====

/// ListMultipartUploads - 列举正在进行的分片上传
pub async fn list_multipart_uploads(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    delimiter: Option<&str>,
    key_marker: Option<&str>,
    upload_id_marker: Option<&str>,
    max_uploads: Option<u32>,
    encoding_type: Option<&str>,
    fetch_from_kv: bool,
) -> Result<Envelope<ListMultipartUploadsResponse>, CliError> {
    let url = client.bucket_endpoint(bucket)?;
    let path = client.bucket_request_path(bucket)?;

    let mut query: BTreeMap<String, String> = BTreeMap::new();
    query.insert("uploads".to_string(), String::new());
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
    if let Some(km) = key_marker {
        if !km.is_empty() {
            query.insert("key-marker".to_string(), km.to_string());
        }
    }
    if let Some(uim) = upload_id_marker {
        if !uim.is_empty() {
            query.insert("upload-id-marker".to_string(), uim.to_string());
        }
    }
    if let Some(mu) = max_uploads {
        query.insert("max-uploads".to_string(), mu.to_string());
    }
    if let Some(et) = encoding_type {
        if !et.is_empty() {
            query.insert("encoding-type".to_string(), et.to_string());
        }
    }
    if fetch_from_kv {
        query.insert("fetch-from-kv".to_string(), "true".to_string());
    }

    let resp = client
        .send_request(Method::GET, &url, &path, query, BTreeMap::new(), None)
        .await?;

    let request_id = extract_request_id(&resp);
    let resp = client.check_response(resp).await?;
    let body = resp.text().await.map_err(CliError::Http)?;

    let data: ListMultipartUploadsResponse = parse_list_response(&body, "ListMultipartUploads")?;

    let next_token = if data.is_truncated {
        data.next_key_marker.clone()
    } else {
        None
    };
    let total = data.uploads.len() as u64;

    Ok(Envelope::success("ve-tos multipart list", data)
        .with_request_id(request_id)
        .with_pagination(PaginationInfo {
            next_token,
            next_marker: None,
            total_returned: total,
        }))
}

/// ListParts - 列举某次分片上传已上传的分片
pub async fn list_parts(
    client: &TosClient,
    bucket: &str,
    key: &str,
    upload_id: &str,
    part_number_marker: Option<u32>,
    max_parts: Option<u32>,
    fetch_from_kv: bool,
) -> Result<Envelope<ListPartsResponse>, CliError> {
    let url = client.object_endpoint(bucket, key)?;
    let path = client.object_request_path(bucket, key)?;

    let mut query: BTreeMap<String, String> = BTreeMap::new();
    query.insert("uploadId".to_string(), upload_id.to_string());
    if let Some(m) = part_number_marker {
        query.insert("part-number-marker".to_string(), m.to_string());
    }
    if let Some(m) = max_parts {
        query.insert("max-parts".to_string(), m.to_string());
    }
    if fetch_from_kv {
        query.insert("fetch-from-kv".to_string(), "true".to_string());
    }

    let resp = client
        .send_request(Method::GET, &url, &path, query, BTreeMap::new(), None)
        .await?;

    let request_id = extract_request_id(&resp);
    let resp = client.check_response(resp).await?;
    let body = resp.text().await.map_err(CliError::Http)?;

    let data: ListPartsResponse = parse_list_response(&body, "ListParts")?;

    let next_token = if data.is_truncated {
        data.next_part_number_marker.map(|n| n.to_string())
    } else {
        None
    };
    let total = data.parts.len() as u64;

    Ok(Envelope::success("ve-tos multipart list-parts", data)
        .with_request_id(request_id)
        .with_pagination(PaginationInfo {
            next_token,
            next_marker: None,
            total_returned: total,
        }))
}

/// [Review Fix #FmtUni-Phase2] Content-aware deserializer (XML or JSON).
/// See `domain::object::parse_list_response` for design rationale.
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
    fn list_multipart_uploads_parses_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListMultipartUploadsResult>
    <Bucket>my-bucket</Bucket>
    <MaxUploads>1000</MaxUploads>
    <IsTruncated>false</IsTruncated>
    <Upload>
        <Key>a.txt</Key>
        <UploadId>up-1</UploadId>
        <Initiated>2024-01-01T00:00:00.000Z</Initiated>
        <StorageClass>STANDARD</StorageClass>
    </Upload>
    <Upload>
        <Key>b.txt</Key>
        <UploadId>up-2</UploadId>
        <Initiated>2024-01-02T00:00:00.000Z</Initiated>
        <StorageClass>IA</StorageClass>
    </Upload>
</ListMultipartUploadsResult>"#;
        let parsed: ListMultipartUploadsResponse =
            quick_xml::de::from_str(xml).expect("parse multipart uploads");
        assert_eq!(parsed.uploads.len(), 2);
        assert_eq!(parsed.uploads[0].upload_id, "up-1");
    }

    #[test]
    fn list_parts_parses_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListPartsResult>
    <Bucket>my-bucket</Bucket>
    <Key>a.txt</Key>
    <UploadId>up-1</UploadId>
    <StorageClass>STANDARD</StorageClass>
    <MaxParts>1000</MaxParts>
    <IsTruncated>false</IsTruncated>
    <Part>
        <PartNumber>1</PartNumber>
        <LastModified>2024-01-01T00:00:00.000Z</LastModified>
        <ETag>"e1"</ETag>
        <Size>1024</Size>
    </Part>
    <Part>
        <PartNumber>2</PartNumber>
        <LastModified>2024-01-01T00:01:00.000Z</LastModified>
        <ETag>"e2"</ETag>
        <Size>2048</Size>
    </Part>
</ListPartsResult>"#;
        let parsed: ListPartsResponse = quick_xml::de::from_str(xml).expect("parse parts");
        assert_eq!(parsed.parts.len(), 2);
        assert_eq!(parsed.parts[1].size, 2048);
        // [Review Fix #ETag-Casing] 锁定 PartInfo <ETag> 不丢字段。
        assert_eq!(parsed.parts[0].etag, "\"e1\"");
        assert_eq!(parsed.parts[1].etag, "\"e2\"");
    }

    #[test]
    fn list_parts_response_serializes_snake_case() {
        let resp = ListPartsResponse {
            bucket: "b".into(),
            key: "k".into(),
            upload_id: "u".into(),
            storage_class: "STANDARD".into(),
            max_parts: 1,
            is_truncated: false,
            part_number_marker: None,
            next_part_number_marker: None,
            parts: vec![PartInfo {
                part_number: 1,
                last_modified: "t".into(),
                etag: "e".into(),
                size: 1,
            }],
            owner: None,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("upload_id").is_some());
        assert!(json.get("max_parts").is_some());
        assert!(json["parts"][0].get("part_number").is_some());
    }
}
