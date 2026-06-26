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

use super::error::AgentErrorCategory;

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope<T: Serialize> {
    pub success: bool,
    pub status: Status,
    pub command: String,
    pub request_id: Option<String>,
    pub status_code: Option<u16>,
    pub ec: Option<String>,
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pagination: Option<PaginationInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorDetail>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Success,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaginationInfo {
    /// Token-based pagination cursor used by TOS APIs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_token: Option<String>,
    /// Marker-based pagination cursor used by ADrive/IDS APIs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_marker: Option<String>,
    /// Number of entries returned in the current response.
    pub total_returned: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorDetail {
    #[serde(skip_serializing, default)]
    pub status_code: Option<u16>,
    pub code: String,
    pub message: String,
    pub exit_code: i32,
    pub kind: ErrorKind,
    pub category: AgentErrorCategory,
    pub suggested_action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doctor_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Unknown,
    AuthFailed,
    ConfigMissing,
    ResourceNotFound,
    PermissionDenied,
    ValidationError,
    RateLimited,
    TransferFailed,
    Conflict,
}

impl<T: Serialize> Envelope<T> {
    pub fn success(command: impl Into<String>, data: T) -> Self {
        Self {
            success: true,
            status: Status::Success,
            command: command.into(),
            // [Review Fix #3] Even direct Envelope serialization should carry
            // a stable request_id; service responses overwrite this with the
            // upstream x-tos-request-id via with_request_id().
            request_id: Some(ulid::Ulid::new().to_string()),
            status_code: None,
            ec: None,
            data: Some(data),
            pagination: None,
            error: None,
        }
    }

    pub fn with_request_id(mut self, id: impl Into<String>) -> Self {
        self.request_id = Some(id.into());
        self
    }

    /// Mark an envelope as having no single upstream request ID.
    ///
    /// Use this for client-side aggregate commands that may issue zero or many
    /// service requests, where a generated fallback ID would be misleading.
    pub fn without_request_id(mut self) -> Self {
        self.request_id = None;
        self
    }

    pub fn with_pagination(mut self, pagination: PaginationInfo) -> Self {
        self.pagination = Some(pagination);
        self
    }
}

impl Envelope<()> {
    pub fn failed(command: impl Into<String>, error: ErrorDetail) -> Self {
        Self {
            success: false,
            status: Status::Failed,
            command: command.into(),
            request_id: Some(ulid::Ulid::new().to_string()),
            status_code: error.status_code,
            ec: Some(error.code.clone()),
            data: None,
            pagination: None,
            error: Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_success_envelope_serialization() {
        let env = Envelope::success("tos bucket list", serde_json::json!({"buckets": []}));
        let json = serde_json::to_string(&env).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["status"], "success");
        assert_eq!(parsed["command"], "tos bucket list");
        assert!(parsed["request_id"].is_string());
        assert!(parsed["status_code"].is_null());
        assert!(parsed["ec"].is_null());
        assert!(parsed["data"].is_object());
        assert!(parsed.get("error").is_none());
    }

    #[test]
    fn test_failed_envelope_serialization() {
        let error = ErrorDetail {
            status_code: Some(404),
            code: "NoSuchBucket".to_string(),
            message: "Bucket not found".to_string(),
            exit_code: 4,
            kind: ErrorKind::ResourceNotFound,
            category: AgentErrorCategory::NotFound,
            suggested_action: Some("verify the bucket name or create the bucket".to_string()),
            fix_command: Some("tos bucket create --bucket my-bucket".to_string()),
            doctor_hint: None,
            docs_url: None,
        };
        let env = Envelope::<()>::failed("tos object upload", error);
        let json = serde_json::to_string(&env).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["success"], false);
        assert_eq!(parsed["status"], "failed");
        assert_eq!(parsed["status_code"], 404);
        assert_eq!(parsed["ec"], "NoSuchBucket");
        assert_eq!(parsed["error"]["code"], "NoSuchBucket");
        assert_eq!(parsed["error"]["exit_code"], 4);
        assert_eq!(parsed["error"]["kind"], "resource_not_found");
    }

    #[test]
    fn test_envelope_with_pagination() {
        let env = Envelope::success("tos object list", serde_json::json!({"objects": []}))
            .with_pagination(PaginationInfo {
                next_token: Some("abc123".to_string()),
                next_marker: None,
                total_returned: 100,
            });
        let json = serde_json::to_string(&env).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["pagination"]["next_token"], "abc123");
        assert!(parsed["pagination"].get("next_marker").is_none());
        assert_eq!(parsed["pagination"]["total_returned"], 100);
    }

    #[test]
    fn test_envelope_with_marker_pagination() {
        let env = Envelope::success("adrive ls", serde_json::json!({"files": []})).with_pagination(
            PaginationInfo {
                next_token: None,
                next_marker: Some("marker-abc".to_string()),
                total_returned: 50,
            },
        );
        let json = serde_json::to_string(&env).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["pagination"].get("next_token").is_none());
        assert_eq!(parsed["pagination"]["next_marker"], "marker-abc");
        assert_eq!(parsed["pagination"]["total_returned"], 50);
    }

    #[test]
    fn test_envelope_with_request_id() {
        let env = Envelope::success("test", serde_json::json!({})).with_request_id("req-12345");
        let json = serde_json::to_string(&env).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["request_id"], "req-12345");
    }
}
