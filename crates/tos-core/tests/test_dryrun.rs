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

//! Integration tests for the current DryRunResult contract.
//! [Review Fix #3] Align dry-run tests with the current plan/impact schema.

use tos_core::agent::dryrun::{DryRunResult, Impact};

fn sample_dry_run() -> DryRunResult {
    DryRunResult {
        action: "tos bucket delete".into(),
        dry_run: true,
        impact: Impact {
            affected_objects: 1,
            affected_bytes: 0,
            risk_level: "critical".into(),
            estimated_duration: Some("< 1s".into()),
            scanned_count: None,
            preview_truncated: None,
        },
        plan: vec!["DELETE /bucket via data-plane endpoint".into()],
        warnings: vec!["requires --force and --confirm tos://demo".into()],
        confirm_command: Some("tos bucket delete tos://demo --force --confirm tos://demo".into()),
    }
}

#[test]
fn test_dryrun_result_serializes_action_and_flag() {
    let parsed = serde_json::to_value(sample_dry_run()).unwrap();
    assert_eq!(parsed["action"], "tos bucket delete");
    assert_eq!(parsed["dry_run"], true);
}

#[test]
fn test_dryrun_result_contains_impact_summary() {
    let parsed = serde_json::to_value(sample_dry_run()).unwrap();
    assert_eq!(parsed["impact"]["affected_objects"], 1);
    assert_eq!(parsed["impact"]["affected_bytes"], 0);
    assert_eq!(parsed["impact"]["risk_level"], "critical");
    assert_eq!(parsed["impact"]["estimated_duration"], "< 1s");
}

#[test]
fn test_dryrun_result_contains_plan_warnings_and_confirmation() {
    let parsed = serde_json::to_value(sample_dry_run()).unwrap();
    assert_eq!(parsed["plan"].as_array().unwrap().len(), 1);
    assert_eq!(parsed["warnings"].as_array().unwrap().len(), 1);
    assert_eq!(
        parsed["confirm_command"],
        "tos bucket delete tos://demo --force --confirm tos://demo"
    );
}

#[test]
fn test_dryrun_result_omits_empty_warnings_and_absent_confirmation() {
    let result = DryRunResult {
        action: "tos bucket list".into(),
        dry_run: true,
        impact: Impact {
            affected_objects: 0,
            affected_bytes: 0,
            risk_level: "low".into(),
            estimated_duration: None,
            scanned_count: None,
            preview_truncated: None,
        },
        plan: vec!["GET /".into()],
        warnings: vec![],
        confirm_command: None,
    };

    let parsed = serde_json::to_value(result).unwrap();
    assert!(parsed.get("warnings").is_none());
    assert!(parsed.get("confirm_command").is_none());
    assert!(parsed["impact"].get("estimated_duration").is_none());
}
