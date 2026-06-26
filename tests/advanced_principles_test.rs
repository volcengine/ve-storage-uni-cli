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

use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(args)
        .output()
        .expect("failed to execute ve-storage-uni-cli")
}

fn cli_with_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"));
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command
        .output()
        .expect("failed to execute ve-storage-uni-cli")
}

fn json_stdout(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|err| {
        panic!("stdout is not json: {err}; stdout={stdout}");
    });
    // [Review Fix #1] 成功路径已统一 Envelope。测试历史上直接断言裸字段，
    // 这里在 helper 里把 Envelope 自动解包到 data，保留所有断言不变。
    unwrap_envelope(raw)
}

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

#[test]
fn test_advanced_group_describe_exposes_subcommands_and_endpoint_rule() {
    for (group, expected_count) in [
        ("data-process", 35),
        ("object-set", 21),
        ("accelerator", 21),
        ("mrap", 16),
        ("ap", 10),
        ("cap", 9),
        ("dataset", 11),
        ("control", 21),
    ] {
        let output = cli(&["ve-tos", group, "--describe"]);
        assert!(
            output.status.success(),
            "group={group}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json = json_stdout(&output);
        assert_eq!(json["command"], format!("ve-tos {group}"));
        assert_eq!(json["supports_help"], true);
        assert_eq!(json["supports_describe"], true);
        let subcommands = json["subcommands"].as_array().expect("subcommands");
        assert_eq!(subcommands.len(), expected_count, "group={group}");
        assert!(json["endpoint_rule"]["data_plane"].is_string());
        assert!(json["endpoint_rule"]["control_plane"].is_string());
    }
}

#[test]
fn test_advanced_leaf_describe_does_not_require_business_args() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "accelerator",
        "create-prefetch-job",
        "--describe",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(raw["command"], "ve-tos accelerator create-prefetch-job");
    assert_eq!(
        raw["data"]["command"],
        "ve-tos accelerator create-prefetch-job"
    );
    assert_eq!(raw["data"]["api"], "PutAcceleratorPrefetchJob");
}

#[test]
fn test_accelerator_create_dry_run_includes_region_query() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "accelerator",
        "create",
        "--name",
        "acc-demo",
        "--region",
        "cn-beijing",
        "--config",
        "{}",
        "--dry-run",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    let plan = raw["data"]["plan"].as_array().expect("plan array");
    let query_line = plan
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find(|line| line.starts_with("query="))
        .expect("query plan line");
    let query: serde_json::Value =
        serde_json::from_str(query_line.trim_start_matches("query=")).expect("query json");
    assert_eq!(query["name"], "acc-demo");
    assert_eq!(query["region"], "cn-beijing");
}

#[test]
fn test_accelerator_create_region_query_falls_back_to_env_profile() {
    let output = cli_with_env(
        &[
            "--output",
            "json",
            "--profile",
            "__codex_test_env_only__",
            "ve-tos",
            "accelerator",
            "create",
            "--name",
            "acc-demo",
            "--config",
            "{}",
            "--dry-run",
        ],
        &[("TOS_REGION", "cn-guilin-boe")],
    );

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    let plan = raw["data"]["plan"].as_array().expect("plan array");
    let query_line = plan
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find(|line| line.starts_with("query="))
        .expect("query plan line");
    let query: serde_json::Value =
        serde_json::from_str(query_line.trim_start_matches("query=")).expect("query json");
    assert_eq!(query["region"], "cn-guilin-boe");
}

#[test]
fn test_capabilities_search_matches_real_api_names() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "capabilities",
        "--search",
        "PutQosPolicy",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = json_stdout(&output);
    let capabilities = json["capabilities"].as_array().expect("capabilities");
    let row = capabilities
        .iter()
        .find(|row| row["command"] == "ve-tos control set-qos-policy")
        .expect("set-qos-policy row");
    assert!(row["apis"].to_string().contains("PutQosPolicy"));
    assert_eq!(row["method"], "PUT");
    assert_eq!(row["risk_level"], "medium");
}

#[test]
fn test_advanced_help_exposes_common_low_level_parameters() {
    let output = cli(&["ve-tos", "data-process", "set-image-style", "--help"]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--bucket"), "stdout={stdout}");
    assert!(stdout.contains("--style-name"), "stdout={stdout}");
    assert!(stdout.contains("--config"), "stdout={stdout}");
    assert!(stdout.contains("--content-md5"), "stdout={stdout}");
    assert!(stdout.contains("--force"), "stdout={stdout}");
}

#[test]
fn test_advanced_dry_run_uses_documented_endpoint_kinds() {
    let cases = [
        (
            &[
                "--output",
                "json",
                "ve-tos",
                "data-process",
                "get-image-style",
                "--bucket",
                "demo-bucket",
                "--style-name",
                "small",
                "--dry-run",
            ][..],
            "data-plane",
            "GetBucketImageStyle",
        ),
        (
            &[
                "--output",
                "json",
                "ve-tos",
                "accelerator",
                "get",
                "--id",
                "acc-1",
                "--dry-run",
            ][..],
            "control-plane",
            "GetAccelerator",
        ),
        (
            &[
                "--output",
                "json",
                "ve-tos",
                "control",
                "list-resource-tags",
                "--bucket",
                "demo-bucket",
                "--resource-trn",
                "trn:tos:::bucket/demo-bucket",
                "--dry-run",
            ][..],
            "data-plane",
            "ListTagsForResource",
        ),
        (
            &[
                "--output",
                "json",
                "ve-tos",
                "control",
                "get-subscribe",
                "--dry-run",
            ][..],
            "control-plane",
            "GetSubscribeConfiguration",
        ),
    ];

    for (args, endpoint_kind, api) in cases {
        let output = cli(args);
        assert!(
            output.status.success(),
            "args={args:?}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json = json_stdout(&output);
        let plan = json["plan"]
            .as_array()
            .expect("dry-run plan")
            .iter()
            .map(|value| value.as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plan.contains(endpoint_kind), "plan={plan}");
        assert!(plan.contains(api), "plan={plan}");
    }
}

#[test]
fn test_advanced_body_commands_require_config_for_execution_paths() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "accelerator",
        "create",
        "--name",
        "acc-1",
        "--dry-run",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("requires --config"), "stderr={stderr}");
}

#[test]
fn test_advanced_delete_allows_dry_run_without_force() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "data-process",
        "delete-image-style",
        "--bucket",
        "demo-bucket",
        "--style-name",
        "small",
        "--dry-run",
    ]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"dry_run\""), "stdout={stdout}");
}

#[test]
fn test_advanced_describe_includes_config_body_and_endpoint_kind() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "control",
        "set-resource-tag",
        "--bucket",
        "demo-bucket",
        "--resource-trn",
        "trn:tos:::bucket/demo-bucket",
        "--describe",
    ]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = json_stdout(&output);
    assert_eq!(json["api"], "TagResource");
    assert_eq!(json["scenario_routing"]["endpoint_kind"], "DataPlane");
    assert!(json["parameters"].to_string().contains("config"));
    assert!(json["parameters"].to_string().contains("resource-trn"));
}
