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

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use tos_core::agent::envelope::{Envelope, PaginationInfo};
use tos_core::agent::error::CliError;
use tos_core::infra::client::TosClient;

// ===== Request / Response 结构体 =====

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateBucketRequest {
    pub bucket: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_full_control: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_read: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_read_non_list: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_read_acp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_write: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_write_acp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub az_redundancy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket_object_lock_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tagging: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateBucketResponse {
    pub bucket: String,
    pub location: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HeadBucketResponse {
    pub bucket: String,
    pub region: String,
    pub storage_class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub az_redundancy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
}

// [Review Fix #FmtUni-C1] PascalCase 仅用于反序列化（兼容 TOS 服务端 JSON），
// 序列化（CLI 输出）必须遵循 project_memory 的 snake_case Naming Convention，
// 否则统一渲染管道用 snake_case 列声明取值会全部命中 Null，table 视图沦为空表。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct ListBucketsResponse {
    pub buckets: Vec<BucketInfo>,
    pub owner: BucketOwner,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct BucketInfo {
    pub name: String,
    pub location: String,
    pub creation_date: String,
    #[serde(default)]
    pub extranet_endpoint: String,
    #[serde(default)]
    pub intranet_endpoint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all(deserialize = "PascalCase"))]
pub struct BucketOwner {
    // [Review Fix #FmtUni-C1] CLI 输出 `id`（snake_case），仅反序列化时识别 TOS 的 `ID`。
    #[serde(rename(deserialize = "ID"))]
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeleteBucketResponse {
    pub bucket: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetBucketLocationResponse {
    pub region: String,
    pub extranet_endpoint: String,
    pub intranet_endpoint: String,
}

// ===== API 实现 =====

/// CreateBucket - 创建存储桶
pub async fn create_bucket(
    client: &TosClient,
    req: &CreateBucketRequest,
) -> Result<Envelope<CreateBucketResponse>, CliError> {
    let url = client.bucket_endpoint(&req.bucket)?;
    let path = client.bucket_request_path(&req.bucket)?;
    let mut headers = BTreeMap::new();

    if let Some(ref sc) = req.storage_class {
        // [Review Fix #BucketHeaderCanonical] Request parameter headers use
        // the canonical x-tos-* form; do not send duplicate x-* aliases.
        headers.insert("x-tos-storage-class".to_string(), sc.clone());
    }
    if let Some(ref acl) = req.acl {
        headers.insert("x-tos-acl".to_string(), acl.clone());
    }
    if let Some(ref value) = req.grant_full_control {
        headers.insert("x-tos-grant-full-control".to_string(), value.clone());
    }
    if let Some(ref value) = req.grant_read {
        headers.insert("x-tos-grant-read".to_string(), value.clone());
    }
    if let Some(ref value) = req.grant_read_non_list {
        headers.insert("x-tos-grant-read-non-list".to_string(), value.clone());
    }
    if let Some(ref value) = req.grant_read_acp {
        headers.insert("x-tos-grant-read-acp".to_string(), value.clone());
    }
    if let Some(ref value) = req.grant_write {
        headers.insert("x-tos-grant-write".to_string(), value.clone());
    }
    if let Some(ref value) = req.grant_write_acp {
        headers.insert("x-tos-grant-write-acp".to_string(), value.clone());
    }
    if let Some(ref az) = req.az_redundancy {
        headers.insert("x-tos-az-redundancy".to_string(), az.clone());
    }
    if let Some(ref bt) = req.bucket_type {
        headers.insert("x-tos-bucket-type".to_string(), bt.clone());
    }
    if let Some(true) = req.bucket_object_lock_enabled {
        headers.insert(
            "x-tos-bucket-object-lock-enabled".to_string(),
            "true".to_string(),
        );
    }
    if let Some(ref value) = req.tagging {
        headers.insert("x-tos-tagging".to_string(), value.clone());
    }
    if let Some(ref pn) = req.project_name {
        headers.insert("x-tos-project-name".to_string(), pn.clone());
    }

    let resp = client
        .send_request(Method::PUT, &url, &path, BTreeMap::new(), headers, None)
        .await?;

    let location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let request_id = extract_request_id(&resp);
    let _resp = client.check_response(resp).await?;

    let data = CreateBucketResponse {
        bucket: req.bucket.clone(),
        location,
    };

    Ok(Envelope::success("ve-tos bucket create", data).with_request_id(request_id))
}

/// HeadBucket - 查询桶元信息
pub async fn head_bucket(
    client: &TosClient,
    bucket: &str,
) -> Result<Envelope<HeadBucketResponse>, CliError> {
    let url = client.bucket_endpoint(bucket)?;
    let path = client.bucket_request_path(bucket)?;
    let resp = client
        .send_request(
            Method::HEAD,
            &url,
            &path,
            BTreeMap::new(),
            BTreeMap::new(),
            None,
        )
        .await?;

    let request_id = extract_request_id(&resp);

    // [Review Fix #1] HeadBucket 响应头按文档优先读取 `x-bucket-region/x-storage-class/...`，同时兼容旧实现 `x-tos-*`。
    let region =
        get_header_value(&resp, &["x-bucket-region", "x-tos-bucket-region"]).unwrap_or_default();

    let storage_class = get_header_value(&resp, &["x-storage-class", "x-tos-storage-class"])
        .unwrap_or_else(|| "STANDARD".to_string());

    let az_redundancy = get_header_value(&resp, &["x-az-redundancy", "x-tos-az-redundancy"]);

    let bucket_type = get_header_value(&resp, &["x-bucket-type", "x-tos-bucket-type"]);

    let project_name = get_header_value(&resp, &["x-project-name", "x-tos-project-name"]);

    let _resp = client.check_response(resp).await?;

    let data = HeadBucketResponse {
        bucket: bucket.to_string(),
        region,
        storage_class,
        az_redundancy,
        bucket_type,
        project_name,
    };

    Ok(Envelope::success("ve-tos bucket head", data).with_request_id(request_id))
}

/// DeleteBucket - 删除桶
pub async fn delete_bucket(
    client: &TosClient,
    bucket: &str,
) -> Result<Envelope<DeleteBucketResponse>, CliError> {
    let url = client.bucket_endpoint(bucket)?;
    let path = client.bucket_request_path(bucket)?;
    let resp = client
        .send_request(
            Method::DELETE,
            &url,
            &path,
            BTreeMap::new(),
            BTreeMap::new(),
            None,
        )
        .await?;

    let request_id = extract_request_id(&resp);
    let _resp = client.check_response(resp).await?;

    let data = DeleteBucketResponse {
        bucket: bucket.to_string(),
        message: "Bucket deleted successfully".to_string(),
    };

    Ok(Envelope::success("ve-tos bucket delete", data).with_request_id(request_id))
}

/// ListBuckets - 列举所有桶
pub async fn list_buckets(
    client: &TosClient,
    project_name: Option<&str>,
    bucket_type: Option<&str>,
) -> Result<Envelope<ListBucketsResponse>, CliError> {
    let url = client.service_endpoint();
    let mut headers = BTreeMap::new();

    if let Some(pn) = project_name {
        headers.insert("x-tos-project-name".to_string(), pn.to_string());
    }
    if let Some(bt) = bucket_type {
        headers.insert("x-tos-bucket-type".to_string(), bt.to_string());
    }

    let resp = client
        .send_request(Method::GET, &url, "/", BTreeMap::new(), headers, None)
        .await?;

    let request_id = extract_request_id(&resp);
    let resp = client.check_response(resp).await?;
    let body = resp.text().await.map_err(CliError::Http)?;

    let data: ListBucketsResponse = serde_json::from_str(&body).map_err(|e| {
        CliError::Unknown(format!(
            "Failed to parse ListBuckets response: {} -- body: {}",
            e,
            &body[..200.min(body.len())]
        ))
    })?;

    let total = data.buckets.len() as u64;
    Ok(Envelope::success("ve-tos bucket list", data)
        .with_request_id(request_id)
        .with_pagination(PaginationInfo {
            next_token: None,
            next_marker: None,
            total_returned: total,
        }))
}

/// GetBucketLocation - 获取桶地域信息
pub async fn get_bucket_location(
    client: &TosClient,
    bucket: &str,
) -> Result<Envelope<GetBucketLocationResponse>, CliError> {
    let url = client.bucket_endpoint(bucket)?;
    let path = client.bucket_request_path(bucket)?;
    let mut query = BTreeMap::new();
    query.insert("location".to_string(), String::new());

    let resp = client
        .send_request(Method::GET, &url, &path, query, BTreeMap::new(), None)
        .await?;

    let request_id = extract_request_id(&resp);
    let resp = client.check_response(resp).await?;
    let body = resp.text().await.map_err(CliError::Http)?;

    let data: GetBucketLocationResponse = serde_json::from_str(&body).map_err(|e| {
        CliError::Unknown(format!("Failed to parse GetBucketLocation response: {}", e))
    })?;

    Ok(Envelope::success("ve-tos bucket location", data).with_request_id(request_id))
}

/// GetBucketStat - 获取桶统计信息
pub async fn get_bucket_stat(
    client: &TosClient,
    bucket: &str,
) -> Result<Envelope<Value>, CliError> {
    get_bucket_json(client, bucket, "stat", "ve-tos bucket stat").await
}

/// GetBucketInfo - 获取桶详细信息
pub async fn get_bucket_info(
    client: &TosClient,
    bucket: &str,
) -> Result<Envelope<Value>, CliError> {
    get_bucket_json(client, bucket, "bucketInfo", "ve-tos bucket info").await
}

async fn get_bucket_json(
    client: &TosClient,
    bucket: &str,
    query_flag: &str,
    command: &str,
) -> Result<Envelope<Value>, CliError> {
    let url = client.bucket_endpoint(bucket)?;
    let path = client.bucket_request_path(bucket)?;
    let mut query = BTreeMap::new();
    query.insert(query_flag.to_string(), String::new());

    let resp = client
        .send_request(Method::GET, &url, &path, query, BTreeMap::new(), None)
        .await?;

    let request_id = extract_request_id(&resp);
    let resp = client.check_response(resp).await?;
    let body = resp.text().await.map_err(CliError::Http)?;
    let data = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| Value::String(body));

    Ok(Envelope::success(command, data).with_request_id(request_id))
}

// 辅助函数
fn extract_request_id(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("x-tos-request-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

fn get_header_value(resp: &reqwest::Response, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = resp.headers().get(*key).and_then(|v| v.to_str().ok()) {
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{GetBucketLocationResponse, ListBucketsResponse};

    #[test]
    fn list_buckets_response_preserves_extranet_endpoint() {
        let body = r#"{
            "Buckets": [
                {
                    "Name": "demo",
                    "Location": "cn-beijing",
                    "CreationDate": "2021-08-19T09:16:05.000Z",
                    "ExtranetEndpoint": "tos-cn-beijing.volces.com",
                    "IntranetEndpoint": "tos-cn-beijing.ivolces.com",
                    "ProjectName": "default",
                    "BucketType": "hns"
                }
            ],
            "Owner": {
                "ID": "account-id"
            }
        }"#;
        let parsed: ListBucketsResponse = serde_json::from_str(body).expect("parse list buckets");
        assert_eq!(
            parsed.buckets[0].extranet_endpoint,
            "tos-cn-beijing.volces.com"
        );
        assert_eq!(
            parsed.buckets[0].intranet_endpoint,
            "tos-cn-beijing.ivolces.com"
        );
        assert_eq!(parsed.buckets[0].project_name.as_deref(), Some("default"));
        assert_eq!(parsed.buckets[0].bucket_type.as_deref(), Some("hns"));

        let serialized = serde_json::to_value(&parsed).expect("serialize list buckets");
        assert_eq!(serialized["buckets"][0]["project_name"], "default");
        assert_eq!(serialized["buckets"][0]["bucket_type"], "hns");
        assert!(serialized["buckets"][0].get("ProjectName").is_none());
        assert!(serialized["buckets"][0].get("BucketType").is_none());
    }

    #[test]
    fn get_bucket_location_response_preserves_extranet_endpoint() {
        let body = r#"{
            "Region": "cn-beijing",
            "ExtranetEndpoint": "tos-cn-beijing.volces.com",
            "IntranetEndpoint": "tos-cn-beijing.ivolces.com"
        }"#;
        let parsed: GetBucketLocationResponse =
            serde_json::from_str(body).expect("parse bucket location");
        assert_eq!(parsed.extranet_endpoint, "tos-cn-beijing.volces.com");
        assert_eq!(parsed.intranet_endpoint, "tos-cn-beijing.ivolces.com");
    }
}
