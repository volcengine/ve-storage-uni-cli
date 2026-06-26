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

use std::process::Command;

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(args)
        .output()
        .expect("failed to execute ve-storage-uni-cli")
}

fn assert_help_contains(command: &[&str], expected: &[&str], unexpected: &[&str]) {
    let output = cli(command);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for flag in expected {
        assert!(stdout.contains(flag), "expected {flag} in {stdout}");
    }
    for flag in unexpected {
        assert!(!stdout.contains(flag), "unexpected {flag} in {stdout}");
    }
}

#[test]
fn test_body_commands_expose_config_json_instead_of_body_fields() {
    let cases: &[(&[&str], &[&str], &[&str])] = &[
        (
            &["ve-tos", "quota", "set", "--help"],
            &["--bucket", "--config"],
            &["--quota"],
        ),
        (
            &["ve-tos", "policy", "set", "--help"],
            &["--bucket", "--config"],
            &["--policy"],
        ),
        (
            &["ve-tos", "lifecycle", "set", "--help"],
            &["--bucket", "--config"],
            &["--status", "--expiration"],
        ),
        (
            &["ve-tos", "cors", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--allowed-origins"],
        ),
        (
            &["ve-tos", "versioning", "set", "--help"],
            &["--bucket", "--config"],
            &["--status"],
        ),
        (
            &["ve-tos", "replication", "set", "--help"],
            &["--bucket", "--config"],
            &["--destination-bucket"],
        ),
        (
            &["ve-tos", "encryption", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--sse-algorithm"],
        ),
        (
            &["ve-tos", "custom-domain", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--certificate-id"],
        ),
        (
            &["ve-tos", "custom-domain", "set-token", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--token"],
        ),
        (
            &["ve-tos", "notification", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--kafka-instance-id"],
        ),
        (
            &["ve-tos", "website", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--index-document-suffix"],
        ),
        (
            &["ve-tos", "mirror", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--condition-key-prefix"],
        ),
        (
            &["ve-tos", "inventory", "set", "--help"],
            &["--bucket", "--id", "--config", "--content-md5"],
            &["--is-enabled"],
        ),
        (
            &["ve-tos", "tagging", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--tags"],
        ),
        (
            &["ve-tos", "rename", "set", "--help"],
            &["--bucket", "--config"],
            &["--enabled"],
        ),
        (
            &["ve-tos", "worm", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--default-retention-days"],
        ),
        (
            &["ve-tos", "real-time-log", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--tls-project-id"],
        ),
        (
            &["ve-tos", "access-monitor", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--status"],
        ),
        (
            &["ve-tos", "trash", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--days"],
        ),
        (
            &["ve-tos", "payment", "set", "--help"],
            &["--bucket", "--config"],
            &["--payer"],
        ),
        (
            &["ve-tos", "logging", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--target-bucket"],
        ),
        (
            &["ve-tos", "intelligent-tiering", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--access-tier"],
        ),
        (
            &["ve-tos", "transfer-acceleration", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--status"],
        ),
        (
            &["ve-tos", "cdn-notification", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--events"],
        ),
        (
            &["ve-tos", "https-config", "set", "--help"],
            &["--bucket", "--config"],
            &["--min-tls-version"],
        ),
        (
            &["ve-tos", "pay-by-traffic", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--status"],
        ),
        (
            &["ve-tos", "max-age", "set", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--max-age-seconds"],
        ),
        (
            &["ve-tos", "redundancy-transition", "create", "--help"],
            &["--bucket", "--config", "--content-md5"],
            &["--target-redundancy"],
        ),
    ];
    for (command, expected, unexpected) in cases {
        assert_help_contains(command, expected, unexpected);
    }
}

#[test]
fn test_header_query_commands_keep_explicit_cli_parameters() {
    assert_help_contains(
        &["ve-tos", "storageclass", "set", "--help"],
        &["--bucket", "--storage-class"],
        &[],
    );
    assert_help_contains(
        &["ve-tos", "acl", "set", "--help"],
        &[
            "--bucket",
            "--config",
            "--acl",
            "--grant-read-non-list",
            "--grant-write-acp",
        ],
        &["--owner-id", "--permission"],
    );
    assert_help_contains(
        &["ve-tos", "replication", "get", "--help"],
        &["--bucket", "--rule-id"],
        &[],
    );
    assert_help_contains(
        &["ve-tos", "inventory", "list", "--help"],
        &["--bucket", "--continuation-token"],
        &[],
    );
    assert_help_contains(
        &["ve-tos", "redundancy-transition", "delete", "--help"],
        &["--bucket", "--task-id", "--force"],
        &[],
    );
}
