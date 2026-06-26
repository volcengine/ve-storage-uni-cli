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
use thiserror::Error;

/// Agent-facing error category for programmatic recovery decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentErrorCategory {
    Retryable,
    AuthError,
    NotFound,
    QuotaExceeded,
    InvalidParam,
    Unknown,
}

/// Agent-facing interpretation of a CLI or service error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentErrorSemantics {
    pub status_code: Option<u16>,
    pub code: String,
    pub message: String,
    pub request_id: Option<String>,
    pub category: AgentErrorCategory,
    pub suggested_action: String,
}

/// 结构化退出码
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum ExitCode {
    Success = 0,
    Unknown = 1,
    AuthFailed = 2,
    ConfigMissing = 3,
    ResourceNotFound = 4,
    PermissionDenied = 5,
    ValidationError = 6,
    RateLimited = 7,
    TransferFailed = 8,
    Conflict = 9,
}

impl ExitCode {
    pub fn as_i32(self) -> i32 {
        self as i32
    }
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("Unknown error: {0}")]
    Unknown(String),

    #[error("Authentication failed: {0}")]
    AuthFailed(String),

    #[error("Configuration missing: {0}")]
    ConfigMissing(String),

    #[error("Resource not found: {0}")]
    ResourceNotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Rate limited: {0}")]
    RateLimited(String),

    #[error("Transfer failed: {0}")]
    TransferFailed(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

impl CliError {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Unknown(_) => ExitCode::Unknown,
            Self::AuthFailed(_) => ExitCode::AuthFailed,
            Self::ConfigMissing(_) => ExitCode::ConfigMissing,
            Self::ResourceNotFound(_) => ExitCode::ResourceNotFound,
            Self::PermissionDenied(_) => ExitCode::PermissionDenied,
            Self::ValidationError(_) => ExitCode::ValidationError,
            Self::RateLimited(_) => ExitCode::RateLimited,
            Self::TransferFailed(_) => ExitCode::TransferFailed,
            Self::Conflict(_) => ExitCode::Conflict,
            // [Review Fix #4] HTTP 错误按状态码细分，不再坍缩为 Unknown
            Self::Http(err) => http_error_exit_code(err),
            // [Review Fix #4] IO 错误按 ErrorKind 细分
            Self::Io(err) => io_error_exit_code(err),
            // [Review Fix #4] JSON 解析失败属于输入校验错误
            Self::Json(_) => ExitCode::ValidationError,
        }
    }

    /// Convert this error into the stable Agent error contract.
    pub fn agent_semantics(&self) -> AgentErrorSemantics {
        let exit_code = self.exit_code();
        let raw_message = self.raw_message();
        let service = parse_service_error(raw_message);
        let code = service
            .as_ref()
            .and_then(|fields| fields.code.clone())
            .unwrap_or_else(|| default_error_code(exit_code).to_string());
        let status_code = service.as_ref().and_then(|fields| fields.status_code);
        let message = service
            .as_ref()
            .and_then(|fields| fields.message.clone())
            .unwrap_or_else(|| self.to_string());
        let request_id = service
            .as_ref()
            .and_then(|fields| fields.request_id.clone())
            .or_else(last_request_id);
        let category = classify_agent_error(exit_code, &code);
        AgentErrorSemantics {
            status_code,
            code,
            message,
            request_id,
            category,
            suggested_action: suggested_action(category).to_string(),
        }
    }

    fn raw_message(&self) -> &str {
        match self {
            Self::Unknown(message)
            | Self::AuthFailed(message)
            | Self::ConfigMissing(message)
            | Self::ResourceNotFound(message)
            | Self::PermissionDenied(message)
            | Self::ValidationError(message)
            | Self::RateLimited(message)
            | Self::TransferFailed(message)
            | Self::Conflict(message) => message,
            Self::Http(_) | Self::Io(_) | Self::Json(_) => "",
        }
    }
}

#[derive(Debug)]
struct ServiceErrorFields {
    status_code: Option<u16>,
    code: Option<String>,
    message: Option<String>,
    request_id: Option<String>,
}

fn parse_service_error(raw_message: &str) -> Option<ServiceErrorFields> {
    let status_code = extract_http_status_code(raw_message);
    let code = extract_bracketed_code(raw_message);
    let request_id = extract_request_id(raw_message);
    if status_code.is_none() && code.is_none() && request_id.is_none() {
        return None;
    }
    let message = extract_service_message(raw_message);
    Some(ServiceErrorFields {
        status_code,
        code,
        message,
        request_id,
    })
}

fn extract_http_status_code(message: &str) -> Option<u16> {
    let rest = message.strip_prefix("HTTP ")?;
    let code = rest.split_whitespace().next()?.trim();
    code.parse::<u16>().ok()
}

fn extract_bracketed_code(message: &str) -> Option<String> {
    let start = message.find('[')?;
    let rest = &message[(start + 1)..];
    let end = rest.find(']')?;
    let code = rest[..end].trim();
    (!code.is_empty()).then(|| code.to_string())
}

fn extract_request_id(message: &str) -> Option<String> {
    let marker = "(RequestId:";
    let start = message.find(marker)? + marker.len();
    let end = message[start..].find(')')? + start;
    let request_id = message[start..end].trim();
    (!request_id.is_empty()).then(|| request_id.to_string())
}

fn extract_service_message(message: &str) -> Option<String> {
    let code_end = message.find(']')?;
    let after_code = message[(code_end + 1)..].trim();
    let without_request_id = after_code
        .split("(RequestId:")
        .next()
        .unwrap_or(after_code)
        .trim();
    (!without_request_id.is_empty()).then(|| without_request_id.to_string())
}

fn last_request_id() -> Option<String> {
    std::env::var("TOS_LAST_REQUEST_ID")
        .ok()
        .filter(|request_id| !request_id.is_empty())
}

fn default_error_code(exit_code: ExitCode) -> &'static str {
    match exit_code {
        ExitCode::Success => "Success",
        ExitCode::Unknown => "Unknown",
        ExitCode::AuthFailed => "AuthError",
        ExitCode::ConfigMissing => "ConfigMissing",
        ExitCode::ResourceNotFound => "NotFound",
        ExitCode::PermissionDenied => "PermissionDenied",
        ExitCode::ValidationError => "InvalidParam",
        ExitCode::RateLimited => "QuotaExceeded",
        ExitCode::TransferFailed => "Retryable",
        ExitCode::Conflict => "Conflict",
    }
}

fn classify_agent_error(exit_code: ExitCode, code: &str) -> AgentErrorCategory {
    let normalized = normalize_error_code(code);
    if is_retryable_code(&normalized) {
        return AgentErrorCategory::Retryable;
    }
    if is_auth_code(&normalized) {
        return AgentErrorCategory::AuthError;
    }
    if is_not_found_code(&normalized) {
        return AgentErrorCategory::NotFound;
    }
    if is_quota_code(&normalized) {
        return AgentErrorCategory::QuotaExceeded;
    }
    if is_invalid_param_code(&normalized) {
        return AgentErrorCategory::InvalidParam;
    }
    category_from_exit_code(exit_code)
}

fn normalize_error_code(code: &str) -> String {
    code.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_retryable_code(code: &str) -> bool {
    matches!(
        code,
        "requesttimeout" | "internalerror" | "serviceunavailable" | "serverbusy"
    ) || code.contains("temporarilyunavailable")
        || code.contains("network")
        || code.contains("timeout")
}

fn is_auth_code(code: &str) -> bool {
    matches!(
        code,
        "accessdenied"
            | "authenticationfailed"
            | "authfailed"
            | "invalidaccesskeyid"
            | "invalidcredential"
            | "invalidsecretkey"
            | "invalidsecuritytoken"
            | "permissiondenied"
            | "signaturenotmatch"
            | "unauthorized"
    )
}

fn is_not_found_code(code: &str) -> bool {
    code.contains("notfound") || code.starts_with("nosuch")
}

fn is_quota_code(code: &str) -> bool {
    code.contains("quota")
        || code.contains("limitexceeded")
        || code.contains("ratelimit")
        || code.contains("throttl")
        || code == "toomanyrequests"
        || code == "slowdown"
}

fn is_invalid_param_code(code: &str) -> bool {
    code.starts_with("invalid")
        || code.starts_with("missing")
        || code.starts_with("malformed")
        || code == "badrequest"
        || code.starts_with("unsupported")
}

fn category_from_exit_code(exit_code: ExitCode) -> AgentErrorCategory {
    match exit_code {
        ExitCode::AuthFailed | ExitCode::ConfigMissing | ExitCode::PermissionDenied => {
            AgentErrorCategory::AuthError
        }
        ExitCode::ResourceNotFound => AgentErrorCategory::NotFound,
        ExitCode::ValidationError | ExitCode::Conflict => AgentErrorCategory::InvalidParam,
        ExitCode::RateLimited => AgentErrorCategory::QuotaExceeded,
        ExitCode::TransferFailed => AgentErrorCategory::Retryable,
        ExitCode::Success | ExitCode::Unknown => AgentErrorCategory::Unknown,
    }
}

fn suggested_action(category: AgentErrorCategory) -> &'static str {
    match category {
        AgentErrorCategory::Retryable => {
            "retry with exponential backoff; verify endpoint and network if retries keep failing"
        }
        AgentErrorCategory::AuthError => {
            "refresh credentials, security token, profile, and IAM/TOS permissions"
        }
        AgentErrorCategory::NotFound => {
            "verify bucket/key/resource name and region; create the resource if it should exist"
        }
        AgentErrorCategory::QuotaExceeded => {
            "reduce request rate or concurrency, retry later, or request a quota increase"
        }
        AgentErrorCategory::InvalidParam => {
            "validate command arguments and JSON body; run the command with --describe or --help"
        }
        AgentErrorCategory::Unknown => {
            "inspect status_code, ec, and request_id; run ve-tos doctor if needed"
        }
    }
}

/// [Review Fix #4] 把 reqwest::Error 映射成结构化退出码。
///
/// 映射策略对齐方案 §4.5：
/// - 401 → AuthFailed(2)
/// - 403 → PermissionDenied(5)
/// - 404 → ResourceNotFound(4)
/// - 408 / timeout / connect → TransferFailed(8)（可重试）
/// - 409 / 412 → Conflict(9)
/// - 429 / 503 → RateLimited(7)
/// - 5xx → TransferFailed(8)
/// - 其它 → Unknown(1)
fn http_error_exit_code(err: &reqwest::Error) -> ExitCode {
    if err.is_timeout() || err.is_connect() {
        return ExitCode::TransferFailed;
    }
    if let Some(status) = err.status() {
        return match status.as_u16() {
            401 => ExitCode::AuthFailed,
            403 => ExitCode::PermissionDenied,
            404 => ExitCode::ResourceNotFound,
            408 => ExitCode::TransferFailed,
            409 | 412 => ExitCode::Conflict,
            429 => ExitCode::RateLimited,
            500..=599 => ExitCode::TransferFailed,
            _ => ExitCode::Unknown,
        };
    }
    ExitCode::Unknown
}

/// [Review Fix #4] 把 std::io::Error 按 ErrorKind 映射。
fn io_error_exit_code(err: &std::io::Error) -> ExitCode {
    use std::io::ErrorKind::*;
    match err.kind() {
        NotFound => ExitCode::ResourceNotFound,
        PermissionDenied => ExitCode::PermissionDenied,
        AlreadyExists => ExitCode::Conflict,
        InvalidInput | InvalidData => ExitCode::ValidationError,
        TimedOut | Interrupted | UnexpectedEof | WriteZero | BrokenPipe => ExitCode::TransferFailed,
        ConnectionAborted | ConnectionRefused | ConnectionReset | NotConnected => {
            ExitCode::TransferFailed
        }
        _ => ExitCode::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit_codes() {
        assert_eq!(ExitCode::Success.as_i32(), 0);
        assert_eq!(ExitCode::Unknown.as_i32(), 1);
        assert_eq!(ExitCode::AuthFailed.as_i32(), 2);
        assert_eq!(ExitCode::ConfigMissing.as_i32(), 3);
        assert_eq!(ExitCode::ResourceNotFound.as_i32(), 4);
        assert_eq!(ExitCode::PermissionDenied.as_i32(), 5);
        assert_eq!(ExitCode::ValidationError.as_i32(), 6);
        assert_eq!(ExitCode::RateLimited.as_i32(), 7);
        assert_eq!(ExitCode::TransferFailed.as_i32(), 8);
        assert_eq!(ExitCode::Conflict.as_i32(), 9);
    }

    #[test]
    fn test_cli_error_exit_code_mapping() {
        let err = CliError::AuthFailed("bad key".to_string());
        assert_eq!(err.exit_code(), ExitCode::AuthFailed);

        let err = CliError::ResourceNotFound("bucket not found".to_string());
        assert_eq!(err.exit_code(), ExitCode::ResourceNotFound);

        let err = CliError::ValidationError("invalid param".to_string());
        assert_eq!(err.exit_code(), ExitCode::ValidationError);
    }

    #[test]
    fn test_cli_error_display() {
        let err = CliError::AuthFailed("invalid credentials".to_string());
        assert_eq!(
            err.to_string(),
            "Authentication failed: invalid credentials"
        );
    }

    // [Review Fix #4] 验证 IO/JSON 错误粒度细化
    #[test]
    fn test_io_error_exit_code_granularity() {
        use std::io;
        let cases: Vec<(io::ErrorKind, ExitCode)> = vec![
            (io::ErrorKind::NotFound, ExitCode::ResourceNotFound),
            (io::ErrorKind::PermissionDenied, ExitCode::PermissionDenied),
            (io::ErrorKind::AlreadyExists, ExitCode::Conflict),
            (io::ErrorKind::InvalidInput, ExitCode::ValidationError),
            (io::ErrorKind::InvalidData, ExitCode::ValidationError),
            (io::ErrorKind::TimedOut, ExitCode::TransferFailed),
            (io::ErrorKind::BrokenPipe, ExitCode::TransferFailed),
            (io::ErrorKind::ConnectionRefused, ExitCode::TransferFailed),
        ];
        for (kind, expected) in cases {
            let err = CliError::Io(io::Error::new(kind, "x"));
            assert_eq!(err.exit_code(), expected, "kind = {:?}", kind);
        }
    }

    #[test]
    fn test_json_error_is_validation() {
        let err: CliError = serde_json::from_str::<serde_json::Value>("{not json")
            .unwrap_err()
            .into();
        assert_eq!(err.exit_code(), ExitCode::ValidationError);
    }
}
