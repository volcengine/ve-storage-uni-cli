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

//! Workspace-level agent scenario tests for current structured output formats.
//! [Review Fix #3] Align agent output tests with current Envelope/DryRunResult contracts.

use serde::Serialize;
use tos_core::agent::dryrun::{DryRunResult, Impact};
use tos_core::agent::envelope::{Envelope, ErrorDetail, ErrorKind, PaginationInfo};
use tos_core::agent::error::{AgentErrorCategory, ExitCode};
use tos_core::agent::output::{format_json, format_table};

#[derive(Debug, Clone, Serialize)]
struct AgentTestItem {
    id: String,
    status: String,
}

#[test]
fn test_agent_failed_envelope_contains_exit_code_and_error_kind() {
    let envelope = Envelope::<()>::failed(
        "tos object delete",
        ErrorDetail {
            status_code: Some(403),
            code: "AccessDenied".into(),
            message: "Bucket deletion requires admin role".into(),
            exit_code: ExitCode::PermissionDenied.as_i32(),
            kind: ErrorKind::PermissionDenied,
            category: AgentErrorCategory::AuthError,
            suggested_action: Some("refresh credentials and permissions".into()),
            fix_command: None,
            doctor_hint: Some("Check IAM policy".into()),
            docs_url: None,
        },
    );

    let parsed = serde_json::to_value(&envelope).unwrap();
    assert_eq!(parsed["success"], false);
    assert_eq!(parsed["status_code"], 403);
    assert_eq!(parsed["ec"], "AccessDenied");
    assert_eq!(parsed["status"], "failed");
    assert_eq!(parsed["error"]["exit_code"], 5);
    assert_eq!(parsed["error"]["kind"], "permission_denied");
    assert_eq!(parsed["error"]["doctor_hint"], "Check IAM policy");
}

#[test]
fn test_agent_dryrun_result_contains_risk_level_and_plan() {
    let result = DryRunResult {
        action: "tos object delete".into(),
        dry_run: true,
        impact: Impact {
            affected_objects: 1,
            affected_bytes: 0,
            risk_level: "critical".into(),
            estimated_duration: Some("< 1s".into()),
            scanned_count: None,
            preview_truncated: None,
        },
        plan: vec!["DELETE /bucket/key via data-plane endpoint".into()],
        warnings: vec!["destructive operation".into()],
        confirm_command: Some(
            "tos object delete tos://bucket/key --force --confirm tos://bucket/key".into(),
        ),
    };

    let parsed = serde_json::to_value(&result).unwrap();
    assert_eq!(parsed["impact"]["risk_level"], "critical");
    assert_eq!(parsed["plan"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["warnings"].as_array().unwrap().len(), 1);
}

#[test]
fn test_agent_json_output_is_parseable() {
    let items = vec![
        AgentTestItem {
            id: "item-1".into(),
            status: "active".into(),
        },
        AgentTestItem {
            id: "item-2".into(),
            status: "inactive".into(),
        },
    ];
    let output = format_json(&items).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 2);
    assert_eq!(parsed[0]["id"], "item-1");
}

#[test]
fn test_agent_table_output_contains_headers_and_rows() {
    let output = format_table(
        &["ID", "Status"],
        &[
            vec!["item-1".into(), "active".into()],
            vec!["item-2".into(), "inactive".into()],
        ],
    );
    assert!(output.contains("ID"));
    assert!(output.contains("Status"));
    assert!(output.contains("item-1"));
}

#[test]
fn test_agent_success_envelope_with_pagination_parseable() {
    let envelope = Envelope::success(
        "tos object list",
        serde_json::json!({ "objects": [{ "key": "a.txt" }] }),
    )
    .with_pagination(PaginationInfo {
        next_token: Some("cursor-abc".into()),
        next_marker: None,
        total_returned: 1,
    });

    let parsed = serde_json::to_value(&envelope).unwrap();
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["pagination"]["next_token"], "cursor-abc");
    assert_eq!(parsed["pagination"]["total_returned"], 1);
}

#[test]
fn test_agent_scenario_dryrun_then_error() {
    let dry_run = DryRunResult {
        action: "tos bucket delete".into(),
        dry_run: true,
        impact: Impact {
            affected_objects: 1,
            affected_bytes: 0,
            risk_level: "high".into(),
            estimated_duration: None,
            scanned_count: None,
            preview_truncated: None,
        },
        plan: vec!["DELETE /bucket".into()],
        warnings: vec!["requires --force".into()],
        confirm_command: None,
    };
    let dry_run_json = serde_json::to_value(&dry_run).unwrap();
    assert_eq!(dry_run_json["dry_run"], true);

    let error = Envelope::<()>::failed(
        "tos bucket delete",
        ErrorDetail {
            status_code: Some(403),
            code: "AccessDenied".into(),
            message: "Access denied".into(),
            exit_code: ExitCode::PermissionDenied.as_i32(),
            kind: ErrorKind::PermissionDenied,
            category: AgentErrorCategory::AuthError,
            suggested_action: Some("refresh credentials and permissions".into()),
            fix_command: None,
            doctor_hint: Some("Contact admin".into()),
            docs_url: None,
        },
    );
    let error_json = serde_json::to_value(&error).unwrap();
    assert_eq!(error_json["error"]["exit_code"], 5);
    assert_eq!(error_json["error"]["doctor_hint"], "Contact admin");
}
