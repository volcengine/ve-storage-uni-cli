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

// [Review Fix #1] 自动解包 Envelope；保留向后兼容字段访问。
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
fn test_bucket_config_group_level_describe_works_for_first_batch() {
    let cases = [
        (vec!["ve-tos", "quota", "--describe"], "ve-tos quota"),
        (vec!["ve-tos", "policy", "--describe"], "ve-tos policy"),
        (
            vec!["ve-tos", "lifecycle", "--describe"],
            "ve-tos lifecycle",
        ),
        (
            vec!["ve-tos", "storageclass", "--describe"],
            "ve-tos storageclass",
        ),
        (vec!["ve-tos", "cors", "--describe"], "ve-tos cors"),
        (
            vec!["ve-tos", "versioning", "--describe"],
            "ve-tos versioning",
        ),
        (
            vec!["ve-tos", "replication", "--describe"],
            "ve-tos replication",
        ),
        (
            vec!["ve-tos", "encryption", "--describe"],
            "ve-tos encryption",
        ),
        (
            vec!["ve-tos", "custom-domain", "--describe"],
            "ve-tos custom-domain",
        ),
        (
            vec!["ve-tos", "notification", "--describe"],
            "ve-tos notification",
        ),
        (vec!["ve-tos", "website", "--describe"], "ve-tos website"),
        (vec!["ve-tos", "mirror", "--describe"], "ve-tos mirror"),
        (
            vec!["ve-tos", "inventory", "--describe"],
            "ve-tos inventory",
        ),
        (vec!["ve-tos", "tagging", "--describe"], "ve-tos tagging"),
        (vec!["ve-tos", "acl", "--describe"], "ve-tos acl"),
        (vec!["ve-tos", "rename", "--describe"], "ve-tos rename"),
        (
            vec!["ve-tos", "real-time-log", "--describe"],
            "ve-tos real-time-log",
        ),
        (
            vec!["ve-tos", "access-monitor", "--describe"],
            "ve-tos access-monitor",
        ),
        (vec!["ve-tos", "worm", "--describe"], "ve-tos worm"),
        (vec!["ve-tos", "trash", "--describe"], "ve-tos trash"),
        (vec!["ve-tos", "payment", "--describe"], "ve-tos payment"),
        (vec!["ve-tos", "logging", "--describe"], "ve-tos logging"),
        (
            vec!["ve-tos", "intelligent-tiering", "--describe"],
            "ve-tos intelligent-tiering",
        ),
        (
            vec!["ve-tos", "transfer-acceleration", "--describe"],
            "ve-tos transfer-acceleration",
        ),
        (
            vec!["ve-tos", "cdn-notification", "--describe"],
            "ve-tos cdn-notification",
        ),
        (
            vec!["ve-tos", "https-config", "--describe"],
            "ve-tos https-config",
        ),
        (
            vec!["ve-tos", "pay-by-traffic", "--describe"],
            "ve-tos pay-by-traffic",
        ),
        (vec!["ve-tos", "max-age", "--describe"], "ve-tos max-age"),
        (
            vec!["ve-tos", "redundancy-transition", "--describe"],
            "ve-tos redundancy-transition",
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
            unwrap_envelope(serde_json::from_str(&stdout).expect("describe json"));
        assert_eq!(json["command"], command);
        assert_eq!(json["supports_help"], true);
        assert_eq!(json["supports_describe"], true);
    }
}

#[test]
fn test_redundancy_transition_accepts_raw_task_id_query_alias() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "redundancy-transition",
        "get",
        "--bucket",
        "demo-bucket",
        "--x-tos-redundancy-transition-taskid",
        "task-1",
        "--dry-run",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_bucket_config_describe_includes_documented_parameters() {
    let cases = [
        (
            vec![
                "ve-tos",
                "quota",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Quota\":1024}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "policy",
                "delete",
                "--bucket",
                "demo-bucket",
                "--force",
                "--describe",
            ],
            "force",
        ),
        (
            vec![
                "ve-tos",
                "lifecycle",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Rules\":[]}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "storageclass",
                "set",
                "--bucket",
                "demo-bucket",
                "--storage-class",
                "STANDARD",
                "--describe",
            ],
            "x-tos-storage-class",
        ),
        (
            vec![
                "ve-tos",
                "cors",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"CORSRules\":[]}",
                "--content-md5",
                "abc",
                "--describe",
            ],
            "Content-MD5",
        ),
        (
            vec![
                "ve-tos",
                "versioning",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Status\":\"Enabled\"}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "replication",
                "get",
                "--bucket",
                "demo-bucket",
                "--rule-id",
                "rule-1",
                "--describe",
            ],
            "rule-id",
        ),
        (
            vec![
                "ve-tos",
                "encryption",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Rule\":{}}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "custom-domain",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Rule\":{\"Domain\":\"static.example.com\"}}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "custom-domain",
                "set-token",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Domain\":\"static.example.com\",\"Token\":\"token-1\"}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "notification",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Rules\":[]}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "website",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"IndexDocument\":{\"Suffix\":\"index.html\"}}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "mirror",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Rules\":[]}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "inventory",
                "set",
                "--bucket",
                "demo-bucket",
                "--id",
                "daily-report",
                "--config",
                "{\"Id\":\"daily-report\"}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "worm",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"ObjectLockEnabled\":\"Enabled\"}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "tagging",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"TagSet\":{\"Tags\":[]}}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "acl",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Owner\":{\"ID\":\"owner-1\"}}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "acl",
                "set",
                "--bucket",
                "demo-bucket",
                "--acl",
                "public-read",
                "--describe",
            ],
            "x-tos-acl",
        ),
        (
            vec![
                "ve-tos",
                "rename",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"RenameEnable\":true}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "access-monitor",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Status\":\"Enabled\"}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "trash",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Status\":\"Enabled\",\"Days\":7}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "payment",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Payer\":\"Requester\"}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "logging",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"BucketLoggingStatus\":{}}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "intelligent-tiering",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Status\":\"Enabled\"}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "transfer-acceleration",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Status\":\"Enabled\"}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "cdn-notification",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Events\":[]}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "https-config",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"TLS\":{\"Enable\":true}}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "pay-by-traffic",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"Status\":\"Enabled\"}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "max-age",
                "set",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"MaxAgeSeconds\":3600}",
                "--describe",
            ],
            "config(body)",
        ),
        (
            vec![
                "ve-tos",
                "redundancy-transition",
                "create",
                "--bucket",
                "demo-bucket",
                "--config",
                "{\"TargetRedundancy\":\"multi-az\"}",
                "--describe",
            ],
            "config(body)",
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
            unwrap_envelope(serde_json::from_str(&stdout).expect("describe json"));
        let params = json["parameters"].as_array().expect("parameters array");
        assert!(
            params.iter().any(|param| param["name"] == expected_param),
            "expected parameter `{}` in {}",
            expected_param,
            stdout
        );
    }
}

#[test]
fn test_bucket_config_delete_requires_force_for_execution() {
    let cases = [
        vec!["ve-tos", "policy", "delete", "--bucket", "demo-bucket"],
        vec!["ve-tos", "lifecycle", "delete", "--bucket", "demo-bucket"],
        vec!["ve-tos", "cors", "delete", "--bucket", "demo-bucket"],
        vec!["ve-tos", "replication", "delete", "--bucket", "demo-bucket"],
        vec!["ve-tos", "encryption", "delete", "--bucket", "demo-bucket"],
        vec![
            "ve-tos",
            "custom-domain",
            "delete",
            "--bucket",
            "demo-bucket",
            "--domain",
            "static.example.com",
        ],
        vec!["ve-tos", "mirror", "delete", "--bucket", "demo-bucket"],
        vec!["ve-tos", "website", "delete", "--bucket", "demo-bucket"],
        vec![
            "ve-tos",
            "inventory",
            "delete",
            "--bucket",
            "demo-bucket",
            "--id",
            "daily-report",
        ],
        vec!["ve-tos", "tagging", "delete", "--bucket", "demo-bucket"],
        vec!["ve-tos", "rename", "delete", "--bucket", "demo-bucket"],
        vec![
            "ve-tos",
            "real-time-log",
            "delete",
            "--bucket",
            "demo-bucket",
        ],
        vec![
            "ve-tos",
            "cdn-notification",
            "delete",
            "--bucket",
            "demo-bucket",
        ],
        vec!["ve-tos", "max-age", "delete", "--bucket", "demo-bucket"],
        vec![
            "ve-tos",
            "redundancy-transition",
            "delete",
            "--bucket",
            "demo-bucket",
        ],
    ];

    for args in cases {
        let output = cli(&args);
        assert!(
            !output.status.success(),
            "stdout={}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("requires --force"), "stderr={stderr}");
    }
}

#[test]
fn test_bucket_config_error_envelope_uses_leaf_command() {
    let output = cli(&["ve-tos", "lifecycle", "delete", "--bucket", "demo-bucket"]);
    assert!(
        !output.status.success(),
        "stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value = serde_json::from_str(stderr.trim()).expect("failed envelope");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["command"], "ve-tos lifecycle delete");
    assert_eq!(json["error"]["kind"], "validation_error");
}

#[test]
fn test_tos_describe_includes_new_bucket_config_groups() {
    let output = cli(&["ve-tos", "--describe"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        unwrap_envelope(serde_json::from_str(&stdout).expect("describe json"));
    let groups = json["groups"].as_array().expect("groups array");
    for expected in [
        "custom-domain",
        "notification",
        "website",
        "mirror",
        "inventory",
        "real-time-log",
        "worm",
        "trash",
        "intelligent-tiering",
        "transfer-acceleration",
        "cdn-notification",
        "pay-by-traffic",
        "max-age",
        "redundancy-transition",
    ] {
        assert!(
            groups.iter().any(|group| group["name"] == expected),
            "missing group `{expected}` in {stdout}"
        );
    }
}
