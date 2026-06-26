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

//! Dedicated integration tests for `ve-tos bucket` low-level API commands.
//!
//! These tests validate all six Agent-Native principles for bucket operations:
//! 1. Discovery (--describe, --help)
//! 2. Understanding (scenario_routing in describe output)
//! 3. Safe Execution (--dry-run, --force protection)
//! 4. Controlled Output (--output json/table/csv, Envelope structure)
//! 5. Deterministic Errors (structured exit codes, error envelope)
//! 6. Agent Ecosystem (machine-readable output, pipe-friendly)

use std::process::Command;

/// Helper: run the CLI binary with given args
fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(args)
        .output()
        .expect("Failed to execute ve-storage-uni-cli")
}

fn cli_without_runtime_config(args: &[&str]) -> std::process::Output {
    let temp_home = std::env::temp_dir().join(format!(
        "tos-bucket-test-home-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("unnamed")
    ));
    let _ = std::fs::remove_dir_all(&temp_home);
    std::fs::create_dir_all(&temp_home).expect("create temp home");

    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(args)
        .env("HOME", &temp_home)
        .env_remove("TOS_ACCESS_KEY")
        .env_remove("TOS_SECRET_KEY")
        .env_remove("TOS_ENDPOINT")
        .env_remove("TOS_CONTROL_ENDPOINT")
        .env_remove("TOS_REGION")
        .output()
        .expect("Failed to execute ve-storage-uni-cli");

    let _ = std::fs::remove_dir_all(&temp_home);
    output
}

/// Helper: unwrap Envelope to get inner data while preserving describe outputs.
fn unwrap_envelope(v: serde_json::Value) -> serde_json::Value {
    if let serde_json::Value::Object(ref map) = v {
        if map
            .get("status")
            .and_then(serde_json::Value::as_str)
            .is_some()
            && map.contains_key("command")
            && map.contains_key("data")
        {
            return map.get("data").cloned().unwrap_or(serde_json::Value::Null);
        }
    }
    v
}

/// Helper: parse JSON from stdout or stderr
fn parse_json(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        return unwrap_envelope(v);
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stderr.trim()) {
        return unwrap_envelope(v);
    }
    panic!(
        "No valid JSON found.\nstdout: {}\nstderr: {}",
        stdout, stderr
    );
}

fn parse_envelope(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|err| {
        panic!("stdout should be valid JSON envelope: {err}\nstdout: {stdout}")
    })
}

// ==========================================================================
// Principle 1: Discovery — --describe and --help
// ==========================================================================

#[test]
fn test_bucket_create_describe() {
    let output = cli(&[
        "--describe",
        "ve-tos",
        "bucket",
        "create",
        "tos://test-bucket",
    ]);
    assert!(output.status.success(), "describe should succeed");
    let json = parse_json(&output);
    assert_eq!(json["command"], "ve-tos bucket create");
    assert_eq!(json["layer"], "low_level");
    assert_eq!(json["api"], "CreateBucket");
    assert_eq!(json["risk_level"], "low");
    assert_eq!(json["supports_dry_run"], true);
}

#[test]
fn test_bucket_head_describe() {
    let output = cli(&[
        "--describe",
        "ve-tos",
        "bucket",
        "head",
        "tos://test-bucket",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["command"], "ve-tos bucket head");
    assert_eq!(json["api"], "HeadBucket");
    assert_eq!(json["supports_dry_run"], true);
}

#[test]
fn test_bucket_delete_describe() {
    let output = cli(&[
        "--describe",
        "ve-tos",
        "bucket",
        "delete",
        "tos://test-bucket",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["command"], "ve-tos bucket delete");
    assert_eq!(json["api"], "DeleteBucket");
    assert_eq!(json["risk_level"], "high");
    assert_eq!(json["supports_dry_run"], true);
    assert!(json["related_commands"]["high_level"].is_string());
}

#[test]
fn test_bucket_list_describe() {
    let output = cli(&["--describe", "ve-tos", "bucket", "list"]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["command"], "ve-tos bucket list");
    assert_eq!(json["api"], "ListBuckets");
    assert_eq!(json["supports_pipe"], true);
    assert!(json["scenario_routing"].is_object());
}

#[test]
fn test_bucket_stat_describe() {
    let output = cli(&[
        "--describe",
        "ve-tos",
        "bucket",
        "stat",
        "tos://test-bucket",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["command"], "ve-tos bucket stat");
    assert_eq!(json["api"], "GetBucketStat");
}

#[test]
fn test_bucket_info_describe() {
    let output = cli(&[
        "--describe",
        "ve-tos",
        "bucket",
        "info",
        "tos://test-bucket",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["command"], "ve-tos bucket info");
    assert_eq!(json["api"], "GetBucketInfo");
}

#[test]
fn test_bucket_location_describe() {
    let output = cli(&[
        "--describe",
        "ve-tos",
        "bucket",
        "location",
        "tos://test-bucket",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["command"], "ve-tos bucket location");
    assert_eq!(json["api"], "GetBucketLocation");
}

#[test]
fn test_bucket_help_output() {
    let output = cli(&["ve-tos", "bucket", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("create"), "should list create subcommand");
    assert!(stdout.contains("head"), "should list head subcommand");
    assert!(stdout.contains("delete"), "should list delete subcommand");
    assert!(stdout.contains("list"), "should list list subcommand");
    assert!(stdout.contains("stat"), "should list stat subcommand");
    assert!(stdout.contains("info"), "should list info subcommand");
    assert!(
        stdout.contains("location"),
        "should list location subcommand"
    );
}

// ==========================================================================
// Principle 2: Understanding — scenario_routing in describe
// ==========================================================================

#[test]
fn test_bucket_create_scenario_routing() {
    let output = cli(&[
        "--describe",
        "ve-tos",
        "bucket",
        "create",
        "tos://test-bucket",
    ]);
    let json = parse_json(&output);
    let routing = &json["scenario_routing"];
    assert!(routing.is_object(), "Should have scenario routing");
    let routing_str = serde_json::to_string(routing).unwrap();
    assert!(
        routing_str.contains("tos://"),
        "Should contain URI-style example"
    );
}

#[test]
fn test_bucket_list_scenario_routing() {
    let output = cli(&["--describe", "ve-tos", "bucket", "list"]);
    let json = parse_json(&output);
    let routing = &json["scenario_routing"];
    assert!(routing.is_object(), "Should have scenario routing");
    let routing_str = serde_json::to_string(routing).unwrap();
    assert!(
        routing_str.contains("--output json"),
        "List should show JSON output example"
    );
}

// ==========================================================================
// Principle 3: Safe Execution — --dry-run and --force protection
// ==========================================================================

#[test]
fn test_bucket_create_dry_run() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "create",
        "tos://my-test-bucket",
    ]);
    assert!(output.status.success(), "dry-run should not fail");
    let json = parse_json(&output);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["action"], "bucket create");
    assert!(json["plan"].is_array());
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(plan_str.contains("my-test-bucket"));
    assert!(json["confirm_command"].is_string());
}

#[test]
fn test_bucket_create_dry_run_accepts_bucket_flag() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "create",
        "--bucket",
        "my-test-bucket",
    ]);
    assert!(output.status.success(), "dry-run should accept --bucket");
    let json = parse_json(&output);
    assert_eq!(json["dry_run"], true);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(plan_str.contains("my-test-bucket"));
}

#[test]
fn test_bucket_create_dry_run_preserves_project_and_headers_in_plan() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "create",
        "tos://my-test-bucket",
        "--region",
        "cn-beijing",
        "--project-name",
        "default",
        "--bucket-type",
        "hns",
        "--az-redundancy",
        "multi-az",
    ]);
    assert!(output.status.success(), "dry-run should not fail");
    let json = parse_json(&output);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(plan_str.contains("Target region: cn-beijing"));
    assert!(plan_str.contains("Project name: default"));
    assert!(plan_str.contains("Bucket type: hns"));
    assert!(plan_str.contains("AZ redundancy: multi-az"));
    let confirm = json["confirm_command"].as_str().unwrap_or_default();
    assert!(confirm.contains("--project-name default"));
}

#[test]
fn test_bucket_delete_dry_run() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "delete",
        "tos://my-test-bucket",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["action"], "bucket delete");
    assert!(json["warnings"].is_array());
    let warnings = json["warnings"].as_array().unwrap();
    assert!(!warnings.is_empty(), "Delete dry-run should have warnings");
    assert_eq!(json["impact"]["risk_level"], "high");
}

#[test]
fn test_bucket_delete_requires_force_for_execution() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "delete",
        "tos://my-test-bucket",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("requires --force"), "stderr={stderr}");
}

#[test]
fn test_bucket_create_dry_run_with_storage_class() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "create",
        "tos://archive-bucket",
        "--storage-class",
        "ARCHIVE",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["dry_run"], true);
    let confirm = json["confirm_command"].as_str().unwrap();
    assert!(
        confirm.contains("ARCHIVE"),
        "Confirm command should preserve storage class"
    );
}

#[test]
fn test_bucket_create_dry_run_accepts_intelligent_tiering_storage_class() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "create",
        "tos://tiering-bucket",
        "--storage-class",
        "INTELLIGENT_TIERING",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["dry_run"], true);
    let confirm = json["confirm_command"].as_str().unwrap();
    assert!(confirm.contains("INTELLIGENT_TIERING"));
}

#[test]
fn test_bucket_info_dry_run() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "info",
        "tos://my-bucket",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["action"], "bucket info");
}

#[test]
fn test_bucket_location_dry_run() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "location",
        "tos://my-bucket",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["action"], "bucket location");
}

#[test]
fn test_bucket_dry_run_envelope_uses_stable_command_name() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "location",
        "tos://my-bucket",
    ]);
    assert!(output.status.success());
    let envelope = parse_envelope(&output);
    assert_eq!(envelope["command"], "ve-tos bucket location");
    assert_eq!(envelope["status"], "success");
}

// ==========================================================================
// Principle 4: Controlled Output — Envelope structure, output formats
// ==========================================================================

#[test]
fn test_bucket_list_error_envelope_structure() {
    let output = cli(&["--output", "json", "ve-tos", "bucket", "list"]);

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let envelope: serde_json::Value = serde_json::from_str(stdout.trim())
            .expect("Success output should be valid JSON envelope");
        assert_eq!(envelope["status"], "success");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let json: serde_json::Value =
            serde_json::from_str(stderr.trim()).expect("Error should be valid JSON envelope");
        assert_eq!(json["status"], "failed");
        assert_eq!(json["command"], "ve-tos bucket list");
        assert!(json["error"].is_object());
        assert!(json["error"]["code"].is_string());
        assert!(json["error"]["message"].is_string());
        assert!(json["error"]["exit_code"].is_number());
        assert!(json["error"]["kind"].is_string());
    }
}

#[test]
fn test_bucket_head_error_envelope_has_guidance() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "head",
        "nonexistent",
    ]);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let json: serde_json::Value =
            serde_json::from_str(stderr.trim()).expect("Error should be valid JSON envelope");
        assert_eq!(json["status"], "failed");
        let has_guidance = json["error"]["doctor_hint"].is_string()
            || json["error"]["docs_url"].is_string()
            || json["error"]["fix_command"].is_string();
        assert!(has_guidance, "Error envelope should provide agent guidance");
    }
}

#[test]
fn test_bucket_create_error_exit_code_range() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "create",
        "test-bucket-no-creds",
    ]);
    if !output.status.success() {
        let exit_code = output.status.code().unwrap_or(-1);
        assert!(
            (0..=9).contains(&exit_code),
            "Exit code {} should be in range 0-9",
            exit_code
        );
    }
}

// ==========================================================================
// Principle 5: Deterministic Errors — exit codes, error kinds
// ==========================================================================

#[test]
fn test_bucket_delete_error_envelope() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "delete",
        "nonexistent-bucket",
        "--force",
    ]);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let json: serde_json::Value =
            serde_json::from_str(stderr.trim()).expect("Error should be valid JSON envelope");
        assert_eq!(json["status"], "failed");

        let kind = json["error"]["kind"].as_str().unwrap();
        let known_kinds = [
            "unknown",
            "auth_failed",
            "config_missing",
            "resource_not_found",
            "permission_denied",
            "validation_error",
            "rate_limited",
            "transfer_failed",
            "conflict",
        ];
        assert!(
            known_kinds.contains(&kind),
            "Error kind '{}' should be a known ErrorKind variant",
            kind
        );
    }
}

#[test]
fn test_bucket_stat_deterministic_exit_code() {
    let output = cli_without_runtime_config(&[
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "stat",
        "no-such-bucket",
    ]);
    if !output.status.success() {
        let exit_code = output.status.code().unwrap_or(-1);
        assert!(
            (0..=9).contains(&exit_code),
            "Exit code {} should be deterministic (0-9)",
            exit_code
        );
        let output2 = cli_without_runtime_config(&[
            "--output",
            "json",
            "ve-tos",
            "bucket",
            "stat",
            "no-such-bucket",
        ]);
        assert_eq!(
            output.status.code(),
            output2.status.code(),
            "Same input should produce same exit code"
        );
    }
}

// ==========================================================================
// Principle 6: Agent Ecosystem — machine-readable, pipe-friendly
// ==========================================================================

#[test]
fn test_all_bucket_describe_outputs_valid_json() {
    let actions: Vec<Vec<&str>> = vec![
        vec!["--describe", "ve-tos", "bucket", "create", "b"],
        vec!["--describe", "ve-tos", "bucket", "head", "b"],
        vec!["--describe", "ve-tos", "bucket", "delete", "b"],
        vec!["--describe", "ve-tos", "bucket", "list"],
        vec!["--describe", "ve-tos", "bucket", "stat", "b"],
        vec!["--describe", "ve-tos", "bucket", "info", "b"],
        vec!["--describe", "ve-tos", "bucket", "location", "b"],
    ];
    for args in &actions {
        let output = cli(args);
        assert!(
            output.status.success(),
            "describe for {:?} should succeed",
            args
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let _parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "describe for {:?} should be valid JSON: {}\nstdout: {}",
                args, e, stdout
            )
        });
    }
}

#[test]
fn test_bucket_describe_consistent_schema() {
    let actions: Vec<Vec<&str>> = vec![
        vec!["--describe", "ve-tos", "bucket", "create", "b"],
        vec!["--describe", "ve-tos", "bucket", "head", "b"],
        vec!["--describe", "ve-tos", "bucket", "delete", "b"],
        vec!["--describe", "ve-tos", "bucket", "list"],
        vec!["--describe", "ve-tos", "bucket", "stat", "b"],
        vec!["--describe", "ve-tos", "bucket", "info", "b"],
        vec!["--describe", "ve-tos", "bucket", "location", "b"],
    ];

    for args in &actions {
        let output = cli(args);
        let json = parse_json(&output);

        assert!(json["command"].is_string(), "{:?} missing command", args);
        assert!(json["layer"].is_string(), "{:?} missing layer", args);
        assert!(
            json["description"].is_string(),
            "{:?} missing description",
            args
        );
        assert!(
            json["risk_level"].is_string(),
            "{:?} missing risk_level",
            args
        );
        assert!(
            json["supports_dry_run"].is_boolean(),
            "{:?} missing supports_dry_run",
            args
        );
        assert!(
            json["supports_pipe"].is_boolean(),
            "{:?} missing supports_pipe",
            args
        );
    }
}

#[test]
fn test_bucket_dry_run_impact_schema() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "create",
        "tos://schema-test-bucket",
    ]);
    let json = parse_json(&output);
    assert_eq!(json["dry_run"], true);
    assert!(json["impact"].is_object(), "Should have impact block");
    assert!(
        json["impact"]["affected_objects"].is_number(),
        "Impact should have affected_objects"
    );
    assert!(
        json["impact"]["affected_bytes"].is_number(),
        "Impact should have affected_bytes"
    );
    assert!(
        json["impact"]["risk_level"].is_string(),
        "Impact should have risk_level"
    );
}

#[test]
fn test_bucket_create_invalid_storage_class_is_structured_validation_error() {
    let output = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-tos",
        "bucket",
        "create",
        "tos://schema-test-bucket",
        "--storage-class",
        "NOT_A_REAL_CLASS",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("error output should be valid JSON envelope");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"]["kind"], "validation_error");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("allowed values"));
}

#[test]
fn test_bucket_list_invalid_bucket_type_is_structured_validation_error() {
    let output = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-tos",
        "bucket",
        "list",
        "--bucket-type",
        "invalid-type",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("error output should be valid JSON envelope");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"]["kind"], "validation_error");
}

// ==========================================================================
// URI Parsing — tos:// protocol support
// ==========================================================================

#[test]
fn test_bucket_create_with_tos_uri() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "create",
        "tos://my-uri-bucket",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(
        plan_str.contains("my-uri-bucket"),
        "Should parse bucket name from tos:// URI"
    );
    assert!(
        !plan_str.contains("tos://"),
        "Parsed bucket name should not include tos:// prefix"
    );
}

#[test]
fn test_bucket_delete_with_tos_uri_dry_run() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "delete",
        "tos://uri-delete-bucket",
        "--force",
    ]);
    assert!(output.status.success());
    let json = parse_json(&output);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(
        plan_str.contains("uri-delete-bucket"),
        "Should parse bucket name from tos:// URI for delete"
    );
}

// ==========================================================================
// Edge Cases
// ==========================================================================

#[test]
fn test_bucket_create_missing_bucket_name() {
    let output = cli(&["ve-tos", "bucket", "create"]);
    assert!(
        !output.status.success(),
        "create without bucket name should fail"
    );
}

#[test]
fn test_bucket_head_missing_bucket_name() {
    let output = cli(&["ve-tos", "bucket", "head"]);
    assert!(!output.status.success());
}

#[test]
fn test_bucket_delete_missing_bucket_name() {
    let output = cli(&["ve-tos", "bucket", "delete"]);
    assert!(!output.status.success());
}

#[test]
fn test_bucket_stat_missing_bucket_name() {
    let output = cli(&["ve-tos", "bucket", "stat"]);
    assert!(!output.status.success());
}

#[test]
fn test_bucket_invalid_subcommand() {
    let output = cli(&["ve-tos", "bucket", "nonexistent"]);
    assert!(
        !output.status.success(),
        "Invalid bucket subcommand should fail"
    );
}
