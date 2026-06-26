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
        .expect("Failed to execute ve-storage-uni-cli")
}

// [Review Fix #1] 自动解包成功 Envelope 到原始 data，保留既有断言。
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
fn test_help_alias_matches_help_flag_for_group_and_leaf() {
    let group = cli(&["ve-tos", "help", "bucket"]);
    assert!(
        group.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&group.stderr)
    );
    let group_stdout = String::from_utf8_lossy(&group.stdout);
    assert!(group_stdout.contains("Bucket core APIs"));
    assert!(group_stdout.contains("create"));
    assert!(group_stdout.contains("delete"));

    let leaf = cli(&["ve-tos", "help", "bucket", "create"]);
    assert!(
        leaf.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&leaf.stderr)
    );
    let leaf_stdout = String::from_utf8_lossy(&leaf.stdout);
    assert!(leaf_stdout.contains("Bucket name"));
    assert!(leaf_stdout.contains("--region"));
}

#[test]
fn test_group_level_describe_works_for_tos_core_layers() {
    let cases = [
        (vec!["ve-tos", "--describe"], "ve-tos"),
        (vec!["ve-tos", "bucket", "--describe"], "ve-tos bucket"),
        (vec!["ve-tos", "object", "--describe"], "ve-tos object"),
        (
            vec!["ve-tos", "multipart", "--describe"],
            "ve-tos multipart",
        ),
        (vec!["ve-tos", "turbo", "--describe"], "ve-tos turbo"),
        (vec!["ve-tos", "config", "--describe"], "ve-tos config"),
    ];

    for (args, command) in cases {
        let output = cli(&args);
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value = unwrap_envelope(
            serde_json::from_str(&stdout).expect("group describe output should be json"),
        );
        assert_eq!(json["command"], command);
        assert_eq!(json["supports_help"], true);
        assert_eq!(json["supports_describe"], true);
    }
}

#[test]
fn test_describe_includes_documented_parameters_for_completed_commands() {
    let cases = [
        (
            vec![
                "ve-tos",
                "bucket",
                "create",
                "tos://demo-bucket",
                "--describe",
            ],
            "x-tos-bucket-type",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "upload",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--body",
                "payload",
                "--describe",
            ],
            "x-tos-net-speed-test",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "link",
                "--bucket",
                "demo-bucket",
                "--key",
                "link.txt",
                "--source-key",
                "target.txt",
                "--describe",
            ],
            "x-link-target",
        ),
        (
            vec![
                "ve-tos",
                "multipart",
                "list",
                "--bucket",
                "demo-bucket",
                "--describe",
            ],
            "upload-id-marker",
        ),
        (
            vec![
                "ve-tos",
                "multipart",
                "list-parts",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--upload-id",
                "upload-123",
                "--describe",
            ],
            "part-number-marker",
        ),
        (
            vec![
                "ve-tos",
                "multipart",
                "copy",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--upload-id",
                "upload-123",
                "--part-number",
                "1",
                "--copy-source",
                "/src-bucket/src-key",
                "--describe",
            ],
            "x-tos-copy-source",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "create-fetch-task",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--source-url",
                "https://example.com/demo.txt",
                "--describe",
            ],
            "x-tos-grant-read",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "copy",
                "tos://src-bucket/src-key",
                "tos://dst-bucket/dst-key",
                "--describe",
            ],
            "x-traffic-limit",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "append",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--body",
                "inline",
                "--offset",
                "0",
                "--describe",
            ],
            "append-last-time",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "delete",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--describe",
            ],
            "x-lifecycle-directly-delete-versions",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "rename",
                "tos://demo-bucket/src.txt",
                "tos://demo-bucket/dst.txt",
                "--describe",
            ],
            "x-forbid-overwrite",
        ),
        (
            vec![
                "ve-tos",
                "turbo",
                "list",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--describe",
            ],
            "listopenedturbo",
        ),
        (
            vec![
                "ve-tos",
                "turbo",
                "open",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--describe",
            ],
            "x-tos-grant-write-acp",
        ),
    ];

    for (args, expected_param) in cases {
        let output = cli(&args);
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            unwrap_envelope(serde_json::from_str(&stdout).expect("describe output should be json"));
        let params = json["parameters"]
            .as_array()
            .expect("parameters should exist");
        assert!(
            params.iter().any(|param| param["name"] == expected_param),
            "expected parameter `{}` in describe output: {}",
            expected_param,
            stdout
        );
    }
}

#[test]
fn test_tos_bucket_targets_use_strict_uri_or_flag_style() {
    let bare_positional = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "head",
        "demo-bucket",
    ]);
    assert!(!bare_positional.status.success());
    let bare_output = format!(
        "{}{}",
        String::from_utf8_lossy(&bare_positional.stdout),
        String::from_utf8_lossy(&bare_positional.stderr)
    );
    assert!(bare_output.contains("positional targets must use tos://bucket"));

    let uri_positional = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "head",
        "tos://demo-bucket",
    ]);
    assert!(
        uri_positional.status.success(),
        "stdout={}",
        String::from_utf8_lossy(&uri_positional.stdout)
    );

    let uri_flag = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "bucket",
        "head",
        "--bucket",
        "tos://demo-bucket",
    ]);
    assert!(!uri_flag.status.success());
    let flag_output = format!(
        "{}{}",
        String::from_utf8_lossy(&uri_flag.stdout),
        String::from_utf8_lossy(&uri_flag.stderr)
    );
    assert!(flag_output.contains("--bucket expects a bucket name only"));
}

#[test]
fn test_tos_object_bucket_flag_rejects_uri_style() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "object",
        "head",
        "--bucket",
        "tos://demo-bucket",
        "--key",
        "demo.txt",
    ]);
    assert!(!output.status.success());
    let stdout = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("expected a bucket name only"));
    assert!(!stdout.contains("tos://tos://"));
}

#[test]
fn test_object_describe_covers_remaining_leaf_commands() {
    let cases = [
        (
            vec![
                "ve-tos",
                "object",
                "list",
                "--bucket",
                "demo-bucket",
                "--describe",
            ],
            "continuation-token",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "get-tagging",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--describe",
            ],
            "tagging",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "fetch",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--source-url",
                "https://example.com/demo.txt",
                "--describe",
            ],
            "source_url(body)",
        ),
    ];

    for (args, expected_param) in cases {
        let output = cli(&args);
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            unwrap_envelope(serde_json::from_str(&stdout).expect("describe output should be json"));
        let params = json["parameters"]
            .as_array()
            .expect("parameters should exist");
        assert!(
            params.iter().any(|param| param["name"] == expected_param),
            "expected parameter `{}` in describe output: {}",
            expected_param,
            stdout
        );
    }
}

#[test]
fn test_object_rename_rejects_cross_bucket_destination() {
    let output = cli(&[
        "ve-tos",
        "object",
        "rename",
        "tos://src-bucket/src.txt",
        "tos://dst-bucket/dst.txt",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("destination bucket must match source bucket"));
}

#[test]
fn test_describe_works_after_subcommand_name() {
    let output = cli(&["ve-tos", "bucket", "create", "demo-bucket", "--describe"]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        unwrap_envelope(serde_json::from_str(&stdout).expect("describe output should be json"));
    assert_eq!(json["command"], "ve-tos bucket create");
    assert_eq!(json["api"], "CreateBucket");
    assert_eq!(json["supports_dry_run"], true);
}

#[test]
fn test_dry_run_works_after_subcommand_name() {
    let output = cli(&[
        "ve-tos",
        "object",
        "upload",
        "--bucket",
        "demo-bucket",
        "--key",
        "demo.txt",
        "--body",
        "payload",
        "--dry-run",
    ]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        unwrap_envelope(serde_json::from_str(&stdout).expect("dry-run output should be json"));
    assert_eq!(json["action"], "ve-tos object upload");
    assert_eq!(json["dry_run"], true);
}

#[test]
fn test_core_output_supports_yaml_table_and_csv() {
    let yaml = cli(&[
        "--output",
        "yaml",
        "ve-tos",
        "object",
        "upload",
        "--bucket",
        "demo-bucket",
        "--key",
        "demo.txt",
        "--body",
        "payload",
        "--dry-run",
    ]);
    assert!(
        yaml.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&yaml.stderr)
    );
    let yaml_stdout = String::from_utf8_lossy(&yaml.stdout);
    assert!(yaml_stdout.contains("action: ve-tos object upload"));
    assert!(yaml_stdout.contains("dry_run: true"));

    let table = cli(&[
        "--output",
        "table",
        "ve-tos",
        "object",
        "upload",
        "--bucket",
        "demo-bucket",
        "--key",
        "demo.txt",
        "--body",
        "payload",
        "--dry-run",
    ]);
    assert!(
        table.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&table.stderr)
    );
    let table_stdout = String::from_utf8_lossy(&table.stdout);
    // [Review Fix #FmtUni] Headers are snake_case to align with JSON keys (Q2 contract).
    assert!(table_stdout.contains("field"));
    assert!(table_stdout.contains("value"));
    assert!(table_stdout.contains("action"));

    let csv = cli(&[
        "--output",
        "csv",
        "ve-tos",
        "object",
        "upload",
        "--bucket",
        "demo-bucket",
        "--key",
        "demo.txt",
        "--body",
        "payload",
        "--dry-run",
    ]);
    assert!(
        csv.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&csv.stderr)
    );
    let csv_stdout = String::from_utf8_lossy(&csv.stdout);
    // Single-object dry-run payloads render as a flattened field/value view, so
    // the CSV header is `field,value` and `action` appears as a data row's key
    // (e.g. `action,ve-tos object upload`) rather than as a header column.
    assert!(csv_stdout
        .lines()
        .next()
        .unwrap_or_default()
        .contains("field"));
    assert!(csv_stdout.contains("action"));
    assert!(csv_stdout.contains("ve-tos object upload"));
}

#[test]
fn test_dry_run_validation_error_is_structured_and_deterministic() {
    let args = [
        "--output",
        "json",
        "ve-tos",
        "object",
        "upload",
        "--bucket",
        "demo-bucket",
        "--dry-run",
    ];
    let output1 = cli(&args);
    let output2 = cli(&args);

    assert!(!output1.status.success());
    assert_eq!(output1.status.code(), output2.status.code());

    let stderr = String::from_utf8_lossy(&output1.stderr);
    let json: serde_json::Value =
        serde_json::from_str(&stderr).expect("error output should be structured json");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"]["kind"], "validation_error");
    assert!(json["error"]["exit_code"].is_number());
}

#[test]
fn test_destructive_object_delete_requires_force_for_execution() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "object",
        "delete",
        "--bucket",
        "demo-bucket",
        "--key",
        "demo.txt",
    ]);
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(&stderr).expect("error output should be structured json");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"]["kind"], "validation_error");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("--force"));
}

#[test]
fn test_destructive_object_delete_force_requires_tos_uri_confirm() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "object",
        "delete",
        "tos://demo-bucket/demo.txt",
        "--force",
    ]);
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(&stderr).expect("error output should be structured json");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"]["kind"], "validation_error");
    let message = json["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("--confirm tos://demo-bucket/demo.txt"),
        "message={message}"
    );
    assert!(
        !message.contains("HTTP error"),
        "confirm guard must fail before network: {message}"
    );
}

#[test]
fn test_destructive_bucket_delete_rejects_bare_confirm() {
    let output = cli(&[
        "--output",
        "json",
        "--confirm",
        "demo-bucket",
        "ve-tos",
        "bucket",
        "delete",
        "tos://demo-bucket",
        "--force",
    ]);
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(&stderr).expect("error output should be structured json");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"]["kind"], "validation_error");
    let message = json["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("does not match") && message.contains("tos://demo-bucket"),
        "message={message}"
    );
}

#[test]
fn test_destructive_multipart_abort_requires_force_for_execution() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "multipart",
        "abort",
        "--bucket",
        "demo-bucket",
        "--key",
        "demo.txt",
        "--upload-id",
        "upload-123",
    ]);
    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(&stderr).expect("error output should be structured json");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"]["kind"], "validation_error");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("--force"));
}

#[test]
fn test_describe_mentions_force_for_destructive_commands() {
    let cases = [
        (
            vec![
                "ve-tos",
                "bucket",
                "delete",
                "tos://demo-bucket",
                "--describe",
            ],
            "ve-tos bucket delete",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "delete",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--describe",
            ],
            "ve-tos object delete",
        ),
        (
            vec![
                "ve-tos",
                "object",
                "batch-delete",
                "--bucket",
                "demo-bucket",
                "--keys",
                "a.txt,b.txt",
                "--describe",
            ],
            "ve-tos object batch-delete",
        ),
        (
            vec![
                "ve-tos",
                "multipart",
                "abort",
                "--bucket",
                "demo-bucket",
                "--key",
                "demo.txt",
                "--upload-id",
                "upload-123",
                "--describe",
            ],
            "ve-tos multipart abort",
        ),
    ];

    for (args, command) in cases {
        let output = cli(&args);
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let json: serde_json::Value =
            unwrap_envelope(serde_json::from_str(&stdout).expect("describe output should be json"));
        assert_eq!(json["command"], command);
        let description = json["description"].as_str().unwrap_or_default();
        let routing = serde_json::to_string(&json["scenario_routing"]).unwrap_or_default();
        assert!(
            description.contains("--force") || routing.contains("--force"),
            "{} should mention --force in describe output",
            command
        );
    }
}
