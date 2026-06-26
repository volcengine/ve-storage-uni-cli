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

//! Integration tests for current CliError, ExitCode, and envelope error details.
//! [Review Fix #3] Align error tests with current CliError/ExitCode and Envelope APIs.

use tos_core::agent::envelope::{Envelope, ErrorDetail, ErrorKind, Status};
use tos_core::agent::error::{AgentErrorCategory, CliError, ExitCode};

#[test]
fn test_exit_code_numeric_values_are_stable() {
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
fn test_cli_error_maps_to_expected_exit_codes() {
    let cases = [
        (CliError::Unknown("x".into()), ExitCode::Unknown),
        (CliError::AuthFailed("x".into()), ExitCode::AuthFailed),
        (CliError::ConfigMissing("x".into()), ExitCode::ConfigMissing),
        (
            CliError::ResourceNotFound("x".into()),
            ExitCode::ResourceNotFound,
        ),
        (
            CliError::PermissionDenied("x".into()),
            ExitCode::PermissionDenied,
        ),
        (
            CliError::ValidationError("x".into()),
            ExitCode::ValidationError,
        ),
        (CliError::RateLimited("x".into()), ExitCode::RateLimited),
        (
            CliError::TransferFailed("x".into()),
            ExitCode::TransferFailed,
        ),
        (CliError::Conflict("x".into()), ExitCode::Conflict),
    ];

    for (err, expected) in cases {
        assert_eq!(err.exit_code(), expected, "err={err}");
    }
}

#[test]
fn test_cli_error_display_contains_context() {
    let err = CliError::ValidationError("missing --bucket".into());
    assert_eq!(err.to_string(), "Validation error: missing --bucket");

    let err = CliError::RateLimited("retry after 60s".into());
    assert!(err.to_string().contains("retry after 60s"));
}

#[test]
fn test_failed_envelope_serializes_structured_error_detail() {
    let error = ErrorDetail {
        status_code: Some(404),
        code: "NoSuchBucket".into(),
        message: "bucket not found".into(),
        exit_code: ExitCode::ResourceNotFound.as_i32(),
        kind: ErrorKind::ResourceNotFound,
        category: AgentErrorCategory::NotFound,
        suggested_action: Some("verify bucket/key/resource name and region".into()),
        fix_command: Some("tos bucket create --bucket demo".into()),
        doctor_hint: Some("tos doctor".into()),
        docs_url: Some("https://www.volcengine.com/docs/6349".into()),
    };
    let envelope = Envelope::<()>::failed("tos object list", error);
    let json = serde_json::to_value(&envelope).unwrap();

    assert_eq!(json["status"], "failed");
    assert_eq!(json["success"], false);
    assert_eq!(json["command"], "tos object list");
    assert_eq!(json["status_code"], 404);
    assert_eq!(json["ec"], "NoSuchBucket");
    assert_eq!(json["error"]["code"], "NoSuchBucket");
    assert_eq!(json["error"]["kind"], "resource_not_found");
    assert_eq!(json["error"]["exit_code"], 4);
    assert_eq!(
        json["error"]["fix_command"],
        "tos bucket create --bucket demo"
    );
}

#[test]
fn test_agent_semantics_prefers_tos_error_code() {
    let err = CliError::PermissionDenied(
        "HTTP 403 [SignatureDoesNotMatch] bad signature (RequestId: req-123)".into(),
    );
    let semantics = err.agent_semantics();

    assert_eq!(semantics.status_code, Some(403));
    assert_eq!(semantics.code, "SignatureDoesNotMatch");
    assert_eq!(semantics.message, "bad signature");
    assert_eq!(semantics.request_id.as_deref(), Some("req-123"));
    assert_eq!(semantics.category, AgentErrorCategory::AuthError);
}

#[test]
fn test_agent_semantics_maps_common_tos_classes() {
    let cases = [
        (
            CliError::ResourceNotFound("[NoSuchBucket] missing (RequestId: r1)".into()),
            AgentErrorCategory::NotFound,
        ),
        (
            CliError::RateLimited("[TooManyRequests] busy (RequestId: r2)".into()),
            AgentErrorCategory::QuotaExceeded,
        ),
        (
            CliError::TransferFailed("[RequestTimeout] retry (RequestId: r3)".into()),
            AgentErrorCategory::Retryable,
        ),
        (
            CliError::ValidationError("[InvalidArgument] bad arg (RequestId: r4)".into()),
            AgentErrorCategory::InvalidParam,
        ),
    ];

    for (err, expected) in cases {
        assert_eq!(err.agent_semantics().category, expected, "err={err}");
    }
}

#[test]
fn test_success_envelope_has_success_status_and_data() {
    let envelope = Envelope::success("tos bucket list", serde_json::json!({"buckets": []}));
    assert!(matches!(envelope.status, Status::Success));
    assert_eq!(envelope.command, "tos bucket list");
    assert!(envelope.data.is_some());
    assert!(envelope.error.is_none());
}
