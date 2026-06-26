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

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
};

fn isolated_home(test_name: &str) -> PathBuf {
    let thread = format!("{:?}", std::thread::current().id());
    let safe_thread = thread.replace(|c: char| !c.is_ascii_alphanumeric(), "_");
    let dir = std::env::temp_dir().join(format!(
        "ve-storage-cli-basic-{test_name}-{}-{safe_thread}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create isolated HOME");
    dir
}

fn cli_with_empty_home(test_name: &str, args: &[&str]) -> std::process::Output {
    let home = isolated_home(test_name);
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env("HOME", home)
        .args(args)
        .output()
        .expect("Failed to execute")
}

fn assert_parameter_schema_contract(parameters: &[serde_json::Value], context: &str) {
    for parameter in parameters {
        let name = parameter["name"].as_str().unwrap_or("<unnamed>");
        // [Review Fix #RebaseTest] The rebase kept schema assertions but lost
        // their helper, so keep the contract explicit in this test file.
        let schema = parameter["schema"].as_object().unwrap_or_else(|| {
            panic!("{context} parameter {name} missing schema object: {parameter:?}")
        });
        assert!(
            schema
                .get("type")
                .and_then(|value| value.as_str())
                .is_some(),
            "{context} parameter {name} schema missing type: {parameter:?}"
        );
    }
}

#[test]
fn test_help_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .arg("--help")
        .output()
        .expect("Failed to execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ve-storage-uni-cli"));
    assert!(stdout.contains("tos"));
    assert!(stdout.contains("ve-tos"));
    // [Help-Cleanup #1] tosvector / tostable are intentionally hidden from the
    // primary root help while still remaining directly invokable parser surfaces.
    // ve-adrive is now implemented and must be visible.
    assert!(
        !stdout.contains("tosvector"),
        "tosvector must stay hidden from root --help"
    );
    assert!(
        !stdout.contains("tostable"),
        "tostable must stay hidden from root --help"
    );
    assert!(
        stdout.contains("ve-adrive"),
        "ve-adrive is implemented and must be listed in --help"
    );
    assert!(stdout.contains("Language:"), "stdout={stdout}");
    assert!(
        stdout.contains("--language <en|zh>"),
        "root help must document help language selection: stdout={stdout}"
    );
    assert!(
        stdout.contains("Help output language"),
        "root help must scope --language to help output: stdout={stdout}"
    );
    // Top-level help must not advertise raw API counts (e.g. "250+ APIs").
    assert!(
        !stdout.contains("APIs"),
        "top-level --help must not include API-count strings"
    );
}

#[test]
fn test_version_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .arg("--version")
        .output()
        .expect("Failed to execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ve-storage-uni-cli"));
}

#[test]
fn test_tos_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "--help"])
        .output()
        .expect("Failed to execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // High-level commands
    assert!(stdout.contains("cp"));
    assert!(stdout.contains("mv"));
    assert!(stdout.contains("sync"));
    assert!(stdout.contains("mb"));
    assert!(stdout.contains("rb"));
    assert!(stdout.contains("mkdir"));
    assert!(stdout.contains("rm"));
    assert!(stdout.contains("ls"));
    assert!(stdout.contains("stat"));
    assert!(stdout.contains("restore"));
    assert!(stdout.contains("Capabilities / Utilities"));
    assert!(stdout.contains("TOS Target Syntax"));
    assert!(stdout.contains("Examples:"));
    assert!(stdout.contains("General:"));
    assert!(stdout.contains("Run 've-storage-uni-cli ve-tos <command> --help'"));
    // Core operations
    assert!(stdout.contains("bucket"));
    assert!(stdout.contains("object"));
    assert!(stdout.contains("multipart"));
    assert!(stdout.contains("turbo"));
    // Bucket config
    assert!(stdout.contains("policy"));
    assert!(stdout.contains("lifecycle"));
    assert!(stdout.contains("encryption"));
    // Advanced features
    assert!(stdout.contains("data-process"));
    assert!(stdout.contains("accelerator"));
    assert!(stdout.contains("mrap"));
    assert!(stdout.contains("dataset"));
    assert!(stdout.contains("control"));
    // Utilities
    assert!(stdout.contains("config"));
    assert!(stdout.contains("capabilities"));
    assert!(stdout.contains("serve"));
    assert!(stdout.contains("doctor"));
    // Global options stay in sync with current parser
    assert!(stdout.contains("--profile"));
    assert!(stdout.contains("--region"));
    assert!(stdout.contains("--control-endpoint"));
    assert!(stdout.contains("--dry-run"));
    assert!(stdout.contains("Language:"));
    assert!(stdout.contains("--language <en|zh>"));
    assert!(stdout.contains("Help output language"));
    assert!(stdout.contains("--help --language zh"));
    assert!(stdout.contains("Include extra diagnostic output where supported"));
    assert!(stdout.contains("Disable prompts and progress output"));
    assert!(!stdout.contains("--trace-dir"));
    assert!(!stdout.contains("--trace-redact"));
    // Section headers
    assert!(stdout.contains("High-Level Commands"));
    assert!(stdout.contains("Low-Level API"));
    assert!(stdout.contains("Utilities"));
    assert!(stdout.contains("Global Options"));
}

#[test]
fn test_byted_tos_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("High-Level Commands"));
    assert!(stdout.contains("Capabilities / Utilities"));
    assert!(stdout.contains("TOS Target Syntax"));
    assert!(stdout.contains("delimiter=\"/\""));
    assert!(stdout.contains("Run 've-storage-uni-cli tos <command> --help'"));
    assert!(stdout.contains("Language:"));
    assert!(stdout.contains("--language <en|zh>"));
    assert!(stdout.contains("Help output language"));
    assert!(stdout.contains("--help --language zh"));
    assert!(!stdout.contains("Low-Level API"));
    assert!(!stdout.contains("  mb            "));
    assert!(!stdout.contains("  rb            "));
    assert!(!stdout.contains("List buckets"));
    assert!(!stdout.contains("multipart"));
    assert!(!stdout.contains("bucket config"));
    assert!(!stdout.contains("--control-endpoint"));
    assert!(!stdout.contains("--account-id"));
    assert!(!stdout.contains("restore"));
}

#[test]
fn test_byted_tos_help_lists_psm_globals() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--psm"), "stdout={stdout}");
    assert!(stdout.contains("--idc"), "stdout={stdout}");
    assert!(stdout.contains("--cluster"), "stdout={stdout}");
    assert!(stdout.contains("--addr-family"), "stdout={stdout}");
}

#[test]
fn test_byted_tos_chinese_help_translates_psm_globals() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "--help", "--language", "zh"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for expected in [
        "--psm <PSM>",
        "PSM 服务名",
        "--idc <IDC>",
        "与 --psm 配合使用的 IDC",
        "--cluster <CLUSTER>",
        "与 --psm 配合使用的集群",
        "--addr-family <VALUE>",
        "与 --psm 配合使用的地址族",
    ] {
        assert!(
            stdout.contains(expected),
            "expected={expected}, stdout={stdout}"
        );
    }
}

#[test]
fn test_byted_tos_chinese_leaf_help_translates_psm_globals() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "ls", "--help", "--language", "zh"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for unexpected in [
        "ByteCloud TOS PSM service name",
        "CLI flag only. Supported by the `tos` command surface",
        "IDC used with PSM service discovery",
        "Cluster used with PSM service discovery",
        "Address family used with PSM service discovery",
    ] {
        assert!(
            !stdout.contains(unexpected),
            "unexpected={unexpected}, stdout={stdout}"
        );
    }
    for expected in [
        "PSM 服务名",
        "仅 CLI 参数。仅 tos 命令支持",
        "与 PSM 服务发现配合使用的 IDC",
        "与 PSM 服务发现配合使用的集群",
        "与 PSM 服务发现配合使用的地址族：v4、v6 或 dual-stack",
    ] {
        assert!(
            stdout.contains(expected),
            "expected={expected}, stdout={stdout}"
        );
    }
}

#[test]
fn test_ve_tos_rejects_psm_globals() {
    for flag in ["--psm", "--idc", "--cluster", "--addr-family"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args([flag, "value", "ve-tos", "ls", "tos://bucket/prefix/"])
            .output()
            .expect("Failed to execute");
        assert!(
            !output.status.success(),
            "ve-tos must reject {flag}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("ve-tos does not support {flag}")),
            "stderr={stderr}"
        );
    }
}

#[test]
fn test_ve_adrive_rejects_psm_globals() {
    for flag in ["--psm", "--idc", "--cluster", "--addr-family"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args([flag, "value", "ve-adrive", "ls", "adrive://inst/space/path"])
            .output()
            .expect("Failed to execute");
        assert!(
            !output.status.success(),
            "ve-adrive must reject {flag}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("ve-adrive does not support {flag}")),
            "stderr={stderr}"
        );
    }
}

#[test]
fn test_non_tos_help_hides_psm_globals() {
    for args in [
        ["ve-tos", "find", "--help"],
        ["ve-adrive", "find", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(args)
            .output()
            .expect("Failed to execute");
        assert!(output.status.success(), "args={args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        for flag in ["--psm", "--idc", "--cluster", "--addr-family"] {
            assert!(
                !stdout.contains(flag),
                "args={args:?}, flag={flag}, stdout={stdout}"
            );
        }
    }
}

#[test]
fn test_find_accepts_space_separated_negative_filters() {
    for args in [
        [
            "tos",
            "find",
            "tos://bucket/prefix",
            "--name",
            "cli",
            "--mtime",
            "-7d",
            "--dry-run",
        ],
        [
            "ve-tos",
            "find",
            "tos://bucket/prefix",
            "--name",
            "cli",
            "--mtime",
            "-7d",
            "--dry-run",
        ],
        [
            "ve-adrive",
            "find",
            "adrive://inst/space/prefix",
            "--name",
            "cli",
            "--mtime",
            "-7d",
            "--dry-run",
        ],
        [
            "tos",
            "find",
            "tos://bucket/prefix",
            "--name",
            "cli",
            "--size",
            "-1",
            "--dry-run",
        ],
        [
            "ve-tos",
            "find",
            "tos://bucket/prefix",
            "--name",
            "cli",
            "--size",
            "-1",
            "--dry-run",
        ],
        [
            "ve-adrive",
            "find",
            "adrive://inst/space/prefix",
            "--name",
            "cli",
            "--size",
            "-1",
            "--dry-run",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(args)
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "args={args:?}, stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_tos_find_accepts_bare_mtime_filter() {
    for args in [
        [
            "tos",
            "find",
            "tos://bucket/prefix",
            "--name",
            "cli",
            "--mtime",
            "7d",
            "--dry-run",
        ],
        [
            "ve-tos",
            "find",
            "tos://bucket/prefix",
            "--name",
            "cli",
            "--mtime",
            "7d",
            "--dry-run",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(args)
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "args={args:?}, stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn test_byted_tos_rejects_storage_class_transfer_option() {
    for (command, args) in [
        (
            "tos cp",
            vec![
                "--dry-run",
                "--output",
                "json",
                "tos",
                "cp",
                "tos://bucket/source",
                "tos://bucket/destination",
                "--storage-class",
                "STANDARD",
            ],
        ),
        (
            "tos find",
            vec![
                "--dry-run",
                "--output",
                "json",
                "tos",
                "find",
                "tos://bucket/",
                "--storage-class",
                "STANDARD",
            ],
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(args)
            .output()
            .expect("Failed to execute");

        assert!(!output.status.success(), "{command} should fail");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let parsed: serde_json::Value =
            serde_json::from_str(stderr.trim()).expect("failed envelope");
        assert_eq!(parsed["status"], "failed");
        assert_eq!(parsed["error"]["kind"], "validation_error");
        assert!(parsed["error"]["message"]
            .as_str()
            .unwrap()
            .contains(&format!("{command} does not support --storage-class")));
    }
}

#[test]
fn test_byted_tos_help_hides_storage_class_transfer_option() {
    for command in ["cp", "mv", "sync", "put", "find"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(["tos", command, "--help"])
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "{} --help stderr={}",
            command,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("--storage-class"),
            "tos {command} help should not document --storage-class"
        );
        assert!(
            !stdout.contains("Storage class") && !stdout.contains("ARCHIVE_FR"),
            "tos {command} help should not leave storage-class details behind"
        );
    }
}

#[test]
fn test_byted_tos_restore_is_not_supported() {
    for args in [
        vec![
            "--dry-run",
            "--output",
            "json",
            "tos",
            "restore",
            "tos://bucket/archived.txt",
        ],
        vec!["--output", "json", "tos", "restore", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(args)
            .output()
            .expect("Failed to execute");

        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        let parsed: serde_json::Value =
            serde_json::from_str(stderr.trim()).expect("failed envelope");
        assert_eq!(parsed["status"], "failed");
        assert_eq!(parsed["error"]["kind"], "validation_error");
        assert!(parsed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unrecognized subcommand"));
    }
}

#[test]
fn test_byted_tos_subcommand_help_uses_byte_tos_env_names() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "skill", "export", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[env: BYTE_TOS_PROFILE=]"),
        "stdout={stdout}"
    );
    assert!(
        stdout.contains("[env: BYTE_TOS_OUTPUT=]"),
        "stdout={stdout}"
    );
    assert!(!stdout.contains("[env: TOS_PROFILE=]"), "stdout={stdout}");
    assert!(!stdout.contains("[env: TOS_OUTPUT=]"), "stdout={stdout}");
    assert!(!stdout.contains("--control-endpoint"), "stdout={stdout}");
    assert!(!stdout.contains("--account-id"), "stdout={stdout}");
}

#[test]
fn test_byted_tos_rejects_control_plane_global_flags() {
    for (flag, value) in [
        ("--control-endpoint", "https://tos-control.example.com"),
        ("--account-id", "2100000001"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(["--output", "json", flag, value, "tos", "doctor"])
            .output()
            .expect("Failed to execute");
        assert!(!output.status.success(), "flag={flag}");
        assert_eq!(output.status.code(), Some(6), "flag={flag}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(flag), "stderr={stderr}");
        assert!(stderr.contains("does not support"), "stderr={stderr}");
        let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
        assert_eq!(parsed["success"], false);
        assert_eq!(parsed["error"]["kind"], "validation_error");
    }
}

#[test]
fn test_unified_tos_scope_hides_control_plane_global_flags() {
    let help = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(!stdout.contains("--control-endpoint"));
    assert!(!stdout.contains("--account-id"));
}

#[test]
fn test_unified_ve_tos_scope_keeps_control_plane_global_flags() {
    let help = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("--control-endpoint"));
    assert!(stdout.contains("--account-id"));
}

#[test]
fn test_tos_and_ve_tos_default_checkpoint_and_report_paths_are_surface_scoped() {
    let home = isolated_home("tos-surface-artifact-paths");
    let run = |tool: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"));
        command
            .env("HOME", &home)
            .env_remove("TOS_CHECKPOINT_DIR")
            .env_remove("TOS_BATCH_REPORT_DIR")
            .env_remove("BYTE_TOS_CHECKPOINT_DIR")
            .env_remove("BYTE_TOS_BATCH_REPORT_DIR")
            .args([
                "--dry-run",
                "--output",
                "json",
                tool,
                "cp",
                "./local",
                "tos://bucket/key",
                "--checkpoint",
            ]);
        command.output().expect("run dry-run cp")
    };

    let byted_tos = run("tos");
    assert!(
        byted_tos.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&byted_tos.stderr)
    );
    let byted_json: serde_json::Value =
        serde_json::from_slice(&byted_tos.stdout).expect("valid tos json");
    assert_eq!(
        byted_json["data"]["checkpoint"]["directory"],
        "~/.tos/checkpoints/tos"
    );
    assert_eq!(
        byted_json["data"]["report"]["path"],
        "~/.tos/reports/tos/cp.csv"
    );

    let ve_tos = run("ve-tos");
    assert!(
        ve_tos.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&ve_tos.stderr)
    );
    let ve_json: serde_json::Value =
        serde_json::from_slice(&ve_tos.stdout).expect("valid ve-tos json");
    assert_eq!(
        ve_json["data"]["checkpoint"]["directory"],
        "~/.tos/checkpoints/ve-tos"
    );
    assert_eq!(
        ve_json["data"]["report"]["path"],
        "~/.tos/reports/ve-tos/cp.csv"
    );
}

#[test]
fn test_byted_tos_high_level_dry_run_uses_tos_command_names() {
    let output = cli_with_empty_home(
        "tos-high-level-public-command",
        &[
            "--dry-run",
            "--output",
            "json",
            "tos",
            "cp",
            "Cargo.toml",
            "tos://bucket/key",
        ],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid dry-run json");
    assert_eq!(parsed["command"], "tos cp");
    assert_eq!(parsed["data"]["command"], "tos cp");
    assert_eq!(parsed["data"]["samples"][0]["operation"], "tos cp");
}

#[test]
fn test_byted_tos_config_set_uses_tos_command_name() {
    let output = cli_with_empty_home(
        "tos-config-set-public-command",
        &[
            "--output",
            "json",
            "tos",
            "config",
            "set",
            "region",
            "cn-beijing",
        ],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid config set json");
    assert_eq!(parsed["command"], "tos config set");
    assert_eq!(parsed["data"]["field"], "region");
    assert_eq!(parsed["data"]["value"], "cn-beijing");
    assert_eq!(parsed["data"]["encrypted"], false);
}

#[test]
fn test_ve_adrive_default_checkpoint_path_is_surface_scoped() {
    let output = cli_with_empty_home(
        "ve-adrive-surface-checkpoint-path",
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "cp",
            "./local",
            "adrive://inst/space/docs/file.txt",
            "--checkpoint",
        ],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid ve-adrive dry-run json");
    assert_eq!(
        json["data"]["checkpoint"]["directory"],
        "~/.tos/checkpoints/ve-adrive"
    );
}

#[test]
fn test_byted_tos_capabilities_are_high_level_only() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "capabilities", "--view", "full", "--output", "json"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid capabilities json");
    let payload = parsed.get("data").unwrap_or(&parsed);
    let layers = payload["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .map(|row| row["layer"].as_str().unwrap_or_default())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(layers.contains("high_level"));
    assert!(layers.contains("utilities"));
    assert!(!layers.contains("low_level"));
    let commands = serde_json::to_string(&payload["capabilities"]).unwrap();
    assert!(commands.contains("tos ls"));
    assert!(!commands.contains("tos restore"));
    assert!(!commands.contains("tos mb"));
    assert!(!commands.contains("tos rb"));
    assert!(!commands.contains("ListBuckets"));
    assert!(commands.contains("ListObjectsType2"));
    assert!(!commands.contains("tos bucket"));
    assert!(!commands.contains("tos multipart"));
}

#[test]
fn test_byted_tos_doctor_summary_matches_shared_shape() {
    let output = cli_with_empty_home(
        "byted-tos-doctor-shared-shape",
        &["--output", "json", "tos", "doctor"],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid doctor json");
    let summary = &parsed["data"]["summary"];
    assert!(summary["total"].is_number());
    assert!(summary["passed"].is_number());
    assert!(summary["warnings"].is_number());
    assert!(summary["failed"].is_number());
    assert!(summary.get("status").is_none());

    let checks = parsed["data"]["checks"].as_array().expect("checks");
    for name in ["config", "auth", "registry", "network", "mcp", "completion"] {
        assert!(
            checks.iter().any(|check| check["name"] == name),
            "tos doctor should include {name}: {checks:?}"
        );
    }
    let mcp = checks
        .iter()
        .find(|check| check["name"] == "mcp")
        .expect("mcp check");
    assert_eq!(mcp["details"]["runtime"], "available");
    assert_eq!(mcp["details"]["stdio_status"], "available");
    assert_eq!(mcp["details"]["sse_status"], "available");
}

#[test]
fn test_byted_tos_doctor_endpoint_alias_uses_network_check() {
    let output = cli_with_empty_home(
        "byted-tos-doctor-endpoint-alias",
        &["--output", "json", "tos", "doctor", "--check", "endpoint"],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid doctor json");
    assert_eq!(parsed["data"]["summary"]["total"], 1);
    assert_eq!(parsed["data"]["checks"][0]["name"], "network");
    assert_eq!(
        parsed["data"]["checks"][0]["details"]["psm_supported"],
        true
    );
    assert_eq!(parsed["data"]["checks"][0]["details"]["has_psm"], false);
}

#[test]
fn test_byted_tos_ls_requires_object_listing_target() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "--dry-run", "tos", "ls"])
        .output()
        .expect("Failed to execute");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("tos-cli ls only supports object listing"),
        "stderr={stderr}"
    );
    assert!(stderr.contains("tos://bucket/prefix"), "stderr={stderr}");
}

#[test]
fn test_byted_tos_api_is_guarded_utility_only() {
    let dry_run = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "tos",
            "api",
            "object",
            "list",
            "--request",
            r#"{"method":"GET","bucket":"bucket"}"#,
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        dry_run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&dry_run.stdout).expect("valid api dry-run json");
    assert_eq!(parsed["data"]["raw_api_execution_implemented"], false);
    assert_eq!(parsed["data"]["status"], "planned_not_executed");

    let execute = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "tos",
            "api",
            "object",
            "list",
            "--request",
            r#"{"method":"GET","bucket":"bucket"}"#,
        ])
        .output()
        .expect("Failed to execute");
    assert!(!execute.status.success());
    let stderr = String::from_utf8_lossy(&execute.stderr);
    assert!(
        stderr.contains("raw API execution is not implemented"),
        "stderr={stderr}"
    );
}

#[test]
fn test_byted_tos_rejects_unsupported_recursive_modes_before_network() {
    let help = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "rm", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(help.status.success());
    let help_stdout = String::from_utf8_lossy(&help.stdout);
    assert!(
        !help_stdout.contains("--recursive-delete-mode"),
        "tos rm help must not document ve-tos-only recursive delete mode"
    );

    let list_mode = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "tos",
            "rm",
            "tos://bucket/prefix/",
            "--recursive",
            "--recursive-list-mode",
            "flat",
            "--force",
            "--confirm",
            "tos://bucket/prefix/",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!list_mode.status.success());
    let list_stderr = String::from_utf8_lossy(&list_mode.stderr);
    assert!(
        list_stderr.contains("recursive listing only supports delimiter"),
        "stderr={list_stderr}"
    );

    let delete_mode = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "tos",
            "rm",
            "tos://bucket/prefix/",
            "--recursive",
            "--recursive-delete-mode",
            "direct",
            "--force",
            "--confirm",
            "tos://bucket/prefix/",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!delete_mode.status.success());
    let delete_stderr = String::from_utf8_lossy(&delete_mode.stderr);
    assert!(
        delete_stderr.contains("unexpected argument '--recursive-delete-mode'"),
        "stderr={delete_stderr}"
    );

    let dry_run = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "tos",
            "rm",
            "tos://bucket/",
            "--recursive",
            "--force",
            "--confirm",
            "tos://bucket/",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        dry_run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let dry_run_stdout = String::from_utf8_lossy(&dry_run.stdout);
    assert!(dry_run_stdout.contains("planned object deletes"));
    assert!(!dry_run_stdout.contains("direct recursion"));
}

#[test]
fn test_byted_tos_serve_and_skill_schema_are_registry_backed() {
    let serve = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--dry-run", "--output", "json", "tos", "serve", "--mcp"])
        .output()
        .expect("Failed to execute");
    assert!(
        serve.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&serve.stderr)
    );
    let serve_json: serde_json::Value =
        serde_json::from_slice(&serve.stdout).expect("valid serve dry-run json");
    assert_eq!(serve_json["data"]["status"], "planned_not_started");
    assert_eq!(serve_json["data"]["mode"], "mcp");
    assert!(serve_json["data"]["call_semantics"]
        .as_str()
        .unwrap_or_default()
        .contains("execute=true"));

    let skill = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "tos", "skill", "list"])
        .output()
        .expect("Failed to execute");
    assert!(
        skill.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&skill.stderr)
    );
    let skill_json: serde_json::Value =
        serde_json::from_slice(&skill.stdout).expect("valid skill list json");
    let skills = skill_json["data"]["skills"].as_array().expect("skills");
    let cp_skill = skills
        .iter()
        .find(|skill| skill["name"] == "tos_cp")
        .expect("tos_cp skill");
    assert!(cp_skill["input_schema"]["properties"]["source"].is_object());
    assert!(cp_skill["input_schema"]["properties"]["destination"].is_object());

    let zh_skill = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "tos",
            "skill",
            "list",
            "--language",
            "zh",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        zh_skill.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&zh_skill.stderr)
    );
    let zh_skill_json: serde_json::Value =
        serde_json::from_slice(&zh_skill.stdout).expect("valid zh skill list json");
    assert_eq!(zh_skill_json["data"]["language"], "zh");
    assert!(zh_skill_json["data"]["skills"]
        .to_string()
        .contains("原始英文说明"));

    let export_dir = isolated_home("tos-skill-export-md").join("skills");
    let export_dir_arg = export_dir.to_string_lossy().to_string();
    let dry_run = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "tos",
            "skill",
            "export",
            "--name",
            "tos_ls",
            "--dir",
            &export_dir_arg,
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        dry_run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let dry_run_json: serde_json::Value =
        serde_json::from_slice(&dry_run.stdout).expect("valid skill export dry-run json");
    assert_eq!(dry_run_json["data"]["format"], "markdown_skill");
    let dry_run_paths = dry_run_json["data"]["paths"].as_array().expect("paths");
    assert!(dry_run_paths
        .iter()
        .any(|path| path.as_str().unwrap_or_default().ends_with("SKILL.md")));
    assert!(dry_run_paths.iter().any(|path| path
        .as_str()
        .unwrap_or_default()
        .ends_with("tos-transfer/tos_ls/SKILL.md")));

    let export = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "tos",
            "skill",
            "export",
            "--name",
            "tos_ls",
            "--dir",
            &export_dir_arg,
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        export.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&export.stderr)
    );
    assert!(export_dir.join("SKILL.md").exists());
    let skill_path = export_dir
        .join("tos-transfer")
        .join("tos_ls")
        .join("SKILL.md");
    let markdown = fs::read_to_string(&skill_path).expect("read exported skill");
    assert!(markdown.starts_with("# tos_ls"), "markdown={markdown}");
    assert!(
        markdown.contains("Use this skill when"),
        "markdown={markdown}"
    );
    assert!(markdown.contains("```json"), "markdown={markdown}");

    let zh_export_dir = isolated_home("tos-skill-export-md-zh").join("skills");
    let zh_export_dir_arg = zh_export_dir.to_string_lossy().to_string();
    let zh_export = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "tos",
            "skill",
            "export",
            "--name",
            "tos_ls",
            "--dir",
            &zh_export_dir_arg,
            "--language",
            "zh",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        zh_export.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&zh_export.stderr)
    );
    let zh_export_json: serde_json::Value =
        serde_json::from_slice(&zh_export.stdout).expect("valid zh skill export json");
    assert_eq!(zh_export_json["data"]["language"], "zh");
    let zh_markdown = fs::read_to_string(
        zh_export_dir
            .join("tos-transfer")
            .join("tos_ls")
            .join("SKILL.md"),
    )
    .expect("read zh exported skill");
    assert!(zh_markdown.contains("## 说明"), "markdown={zh_markdown}");
    assert!(zh_markdown.contains("执行建议"), "markdown={zh_markdown}");
    assert!(zh_markdown.contains("参数说明"), "markdown={zh_markdown}");
}

#[test]
fn test_tos_mb_help_documents_value_ranges() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "mb", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("private"));
    assert!(stdout.contains("public-read-write"));
    assert!(stdout.contains("bucket-owner-full-control"));
    assert!(stdout.contains("ARCHIVE_FR"));
    assert!(stdout.contains("INTELLIGENT_TIERING"));
    assert!(stdout.contains("DEEP_COLD_ARCHIVE"));
    assert!(stdout.contains("single-az"));
    assert!(stdout.contains("multi-az"));
    assert!(stdout.contains("--bucket-type"));
    assert!(stdout.contains("fns"));
    assert!(stdout.contains("hns"));
}

#[test]
fn test_skill_help_documents_language_option() {
    for args in [
        ["ve-tos", "skill", "--help"],
        ["ve-adrive", "skill", "--help"],
        ["tos", "skill", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(args)
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "args={args:?}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("--language zh"), "stdout={stdout}");
        assert!(
            stdout.contains("--help-language <en|zh>"),
            "stdout={stdout}"
        );
    }
}

#[test]
fn test_help_supports_chinese_language() {
    let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (
            vec!["--help", "--language", "zh"],
            vec!["用法:", "工具:", "--language <en|zh>"],
        ),
        (
            vec!["help", "--language", "zh"],
            vec!["用法:", "工具:", "--language <en|zh>"],
        ),
        (
            vec!["tos", "--help", "--language", "zh"],
            vec!["高阶命令", "TOS 目标语法", "查看命令详情"],
        ),
        (
            vec!["help", "tos", "--language", "zh"],
            vec!["高阶命令", "TOS 目标语法", "查看命令详情"],
        ),
        (
            vec!["help", "tos", "ls", "--language", "zh"],
            vec!["说明:", "列出 Bucket 内的对象前缀或对象", "Bucket 名称"],
        ),
        (
            vec!["ve-tos", "--help", "--language", "zh"],
            vec!["高阶命令", "TOS 目标语法", "查看命令详情"],
        ),
        (
            vec!["ve-adrive", "--help", "--language", "zh"],
            vec![
                "高阶命令",
                "复制本地文件、ADrive 文件或文件夹",
                "能力 / 工具",
            ],
        ),
        (
            vec!["ve-tos", "cp", "--help", "--language", "zh"],
            vec!["说明:", "参数:", "选项:", "--recursive", "--no-progress"],
        ),
        (
            vec!["ve-tos", "--help", "--language", "zh", "ls"],
            vec![
                "说明:",
                "列出 Bucket 或对象",
                "Bucket 名称",
                "禁用提示和进度输出",
            ],
        ),
        (
            vec!["ve-adrive", "sync", "--language", "zh", "--help"],
            vec!["说明:", "参数:", "选项:", "--list-echo", "--no-progress"],
        ),
        (
            vec!["tos", "skill", "export", "--help", "--language", "zh"],
            vec!["说明:", "选项:", "--language", "SKILL.md"],
        ),
    ];

    for (args, expected_parts) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(&args)
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "args={args:?}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        for expected in expected_parts {
            assert!(
                stdout.contains(expected),
                "args={args:?}, expected={expected}, stdout={stdout}"
            );
        }
    }
}

#[test]
fn test_chinese_root_help_translates_command_descriptions() {
    let tos_output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "--help", "--language", "zh"])
        .output()
        .expect("Failed to execute");
    assert!(tos_output.status.success());
    let tos_stdout = String::from_utf8_lossy(&tos_output.stdout);
    for unexpected in [
        "Bucket Core APIs",
        "Turbo append upload APIs",
        "Advanced data processing APIs",
        "Inspect API metadata",
        "生成预签名 URLs",
        "恢复归档对象s",
    ] {
        assert!(
            !tos_stdout.contains(unexpected),
            "unexpected={unexpected}, stdout={tos_stdout}"
        );
    }
    for expected in [
        "Bucket 核心 API",
        "Turbo 追加上传 API",
        "高级数据处理 API",
        "查看 API 元数据",
    ] {
        assert!(
            tos_stdout.contains(expected),
            "expected={expected}, stdout={tos_stdout}"
        );
    }

    let adrive_output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-adrive", "--help", "--language", "zh"])
        .output()
        .expect("Failed to execute");
    assert!(adrive_output.status.success());
    let adrive_stdout = String::from_utf8_lossy(&adrive_output.stdout);
    assert!(
        adrive_stdout.contains("复制本地文件、ADrive 文件或文件夹"),
        "stdout={adrive_stdout}"
    );
    assert!(
        adrive_stdout.contains("发现 CLI 能力"),
        "stdout={adrive_stdout}"
    );
}

#[test]
fn test_chinese_help_translates_common_leaf_text() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "--help", "--language", "zh", "ls"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for unexpected in [
        "List buckets or objects",
        "Path to list",
        "Bucket name (alternative to positional URI)",
        "Configuration profile name",
        "Custom service endpoint",
        "Disable prompts and progress output",
    ] {
        assert!(
            !stdout.contains(unexpected),
            "unexpected={unexpected}, stdout={stdout}"
        );
    }
    for expected in [
        "列出 Bucket 或对象",
        "要列出的路径",
        "Bucket 名称（位置 URI 的替代写法）",
        "配置 profile 名称",
        "自定义服务 endpoint",
        "禁用提示和进度输出",
    ] {
        assert!(
            stdout.contains(expected),
            "expected={expected}, stdout={stdout}"
        );
    }
}

#[test]
fn test_chinese_help_translates_common_batch_and_skill_text() {
    let cases: Vec<(Vec<&str>, Vec<&str>, Vec<&str>)> = vec![
        (
            vec!["ve-tos", "cp", "--help", "--language", "zh"],
            vec!["源路径", "目标路径", "递归复制", "目标覆盖策略"],
            vec![
                "Source path",
                "Destination path",
                "Recursive copy",
                "Destination overwrite strategy",
            ],
        ),
        (
            vec!["ve-tos", "rm", "--help", "--language", "zh"],
            vec!["目标路径", "HNS Bucket 的递归删除策略", "计划删除 manifest"],
            vec![
                "Target path",
                "Recursive delete strategy",
                "递归删除 strategy",
            ],
        ),
        (
            vec!["ve-adrive", "sync", "--help", "--language", "zh"],
            vec!["源路径", "删除目标端多余", "目标覆盖策略"],
            vec![
                "Source path",
                "Delete extraneous",
                "Destination overwrite strategy",
            ],
        ),
        (
            vec!["tos", "skill", "export", "--help", "--language", "zh"],
            vec!["将 Skill 导出为 Markdown", "输出目录", "文档语言：en 或 zh"],
            vec![
                "Export skills as Markdown",
                "Output directory",
                "Documentation language: en or zh",
            ],
        ),
    ];

    for (args, expected_parts, unexpected_parts) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(&args)
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "args={args:?}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        for expected in expected_parts {
            assert!(
                stdout.contains(expected),
                "args={args:?}, expected={expected}, stdout={stdout}"
            );
        }
        for unexpected in unexpected_parts {
            assert!(
                !stdout.contains(unexpected),
                "args={args:?}, unexpected={unexpected}, stdout={stdout}"
            );
        }
    }
}

#[test]
fn test_chinese_help_translates_low_level_and_capability_text() {
    let cases: Vec<(Vec<&str>, Vec<&str>, Vec<&str>)> = vec![
        (
            vec!["ve-tos", "bucket", "--help", "--language", "zh"],
            vec!["创建新 Bucket", "获取 Bucket 元数据", "显示此消息"],
            vec![
                "Create a new bucket",
                "Get bucket metadata",
                "Print this message",
            ],
        ),
        (
            vec!["ve-tos", "object", "list", "--help", "--language", "zh"],
            vec!["列出对象", "对象列举 URI", "单次响应最大 key 数"],
            vec![
                "List objects",
                "Object list URI",
                "Maximum keys per response",
            ],
        ),
        (
            vec!["tos", "capabilities", "--help", "--language", "zh"],
            vec!["视图：groups", "按命令分组过滤", "搜索关键词"],
            vec!["View: groups", "Filter by command group", "Search keywords"],
        ),
    ];

    for (args, expected_parts, unexpected_parts) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(&args)
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "args={args:?}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        for expected in expected_parts {
            assert!(
                stdout.contains(expected),
                "args={args:?}, expected={expected}, stdout={stdout}"
            );
        }
        for unexpected in unexpected_parts {
            assert!(
                !stdout.contains(unexpected),
                "args={args:?}, unexpected={unexpected}, stdout={stdout}"
            );
        }
    }
}

#[test]
fn test_help_flag_before_subcommand_uses_subcommand_help() {
    let leading = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "--help", "ls"])
        .output()
        .expect("Failed to execute");
    let trailing = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "ls", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(leading.status.success());
    assert!(trailing.status.success());
    assert_eq!(
        String::from_utf8_lossy(&leading.stdout),
        String::from_utf8_lossy(&trailing.stdout)
    );
}

#[test]
fn test_chinese_help_uses_same_leaf_options_as_english_help() {
    let english = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "ls", "--help"])
        .output()
        .expect("Failed to execute");
    let chinese = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "ls", "--help", "--language", "zh"])
        .output()
        .expect("Failed to execute");
    assert!(english.status.success());
    assert!(chinese.status.success());
    let english_stdout = String::from_utf8_lossy(&english.stdout);
    let chinese_stdout = String::from_utf8_lossy(&chinese.stdout);
    for flag in [
        "--bucket",
        "--key",
        "--max-keys",
        "--continuation-token",
        "--human-readable",
        "--columns",
        "--manifest-path",
        "--help",
    ] {
        assert!(
            english_stdout.contains(flag) && chinese_stdout.contains(flag),
            "flag={flag}, english={english_stdout}, chinese={chinese_stdout}"
        );
    }
    assert!(chinese_stdout.contains("用法:"));
    assert!(chinese_stdout.contains("选项:"));
}

#[test]
fn test_leaf_help_documents_help_language_option() {
    let english = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "cp", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(
        english.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&english.stderr)
    );
    let english_stdout = String::from_utf8_lossy(&english.stdout);
    assert!(
        english_stdout.contains("Language:"),
        "stdout={english_stdout}"
    );
    assert!(
        english_stdout.contains("--language <en|zh>"),
        "stdout={english_stdout}"
    );
    assert!(
        english_stdout.contains("--help --language zh"),
        "stdout={english_stdout}"
    );

    let chinese = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "cp", "--help", "--language", "zh"])
        .output()
        .expect("Failed to execute");
    assert!(
        chinese.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&chinese.stderr)
    );
    let chinese_stdout = String::from_utf8_lossy(&chinese.stdout);
    assert!(chinese_stdout.contains("语言:"), "stdout={chinese_stdout}");
    assert!(
        chinese_stdout.contains("--language <en|zh>"),
        "stdout={chinese_stdout}"
    );
    assert!(
        chinese_stdout.contains("--help --language zh"),
        "stdout={chinese_stdout}"
    );
}

#[test]
fn test_help_language_en_uses_existing_english_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "cp", "--language", "en", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("Copy local files, TOS objects, or prefixes"));
    assert!(!stdout.contains("说明:"));
}

#[test]
fn test_help_language_rejects_invalid_requests() {
    let bad_language = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--help", "--language", "ja"])
        .output()
        .expect("Failed to execute");
    assert!(!bad_language.status.success());
    let stderr = String::from_utf8_lossy(&bad_language.stderr);
    assert!(stderr.contains("expected en or zh"), "stderr={stderr}");

    let unknown_command = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "not-a-command", "--help", "--language", "zh"])
        .output()
        .expect("Failed to execute");
    assert!(!unknown_command.status.success());
    let stderr = String::from_utf8_lossy(&unknown_command.stderr);
    assert!(stderr.contains("未知命令"), "stderr={stderr}");

    let unknown_top_level = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--help", "not-a-tool", "--language", "zh"])
        .output()
        .expect("Failed to execute");
    assert!(!unknown_top_level.status.success());
    let stderr = String::from_utf8_lossy(&unknown_top_level.stderr);
    assert!(stderr.contains("not-a-tool"), "stderr={stderr}");
}

#[test]
fn test_tos_put_help_documents_stdin_eof() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "put", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("EOF"));
    assert!(stdout.contains("Ctrl+D"));
    assert!(stdout.contains("Ctrl+Z"));
    assert!(stdout.contains("Ctrl+C cancels"));
}

#[test]
fn test_adrive_put_help_documents_stdin_eof() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-adrive", "put", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("EOF"));
    assert!(stdout.contains("Ctrl+D"));
    assert!(stdout.contains("Ctrl+Z"));
    assert!(stdout.contains("Ctrl+C cancels"));
}

#[test]
fn test_tos_put_help_documents_object_write_options() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "put", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--storage-class"));
    assert!(stdout.contains("ARCHIVE_FR"));
    assert!(stdout.contains("DEEP_COLD_ARCHIVE"));
    assert!(stdout.contains("--acl"));
    assert!(stdout.contains("bucket-owner-entrusted"));
    assert!(stdout.contains("--meta"));
    assert!(stdout.contains("key=value#key2=value2"));
}

#[test]
fn test_tos_transfer_help_documents_object_write_options() {
    for command in ["cp", "mv", "sync"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(["ve-tos", command, "--help"])
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "{} --help stderr={}",
            command,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("--storage-class"),
            "{} help should document --storage-class",
            command
        );
        assert!(
            stdout.contains("bucket-owner-entrusted"),
            "{} help should document object ACL values",
            command
        );
        assert!(
            stdout.contains("key=value#key2=value2"),
            "{} help should document metadata syntax",
            command
        );
    }
}

#[test]
fn test_tos_mkdir_describe_documents_fixed_directory_content_type() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "mkdir", "--describe"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("content-type"));
    assert!(stdout.contains("application/x-directory"));
}

#[test]
fn test_tos_api_help_documents_raw_request_format() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "api", "--help"])
        .output()
        .expect("Failed to execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--request"));
    assert!(stdout.contains("endpoint_rule"));
    assert!(stdout.contains("query"));
    assert!(stdout.contains("headers"));
    assert!(stdout.contains("body"));
    assert!(stdout.contains(r#""query":{"lifecycle":""}"#));
    assert!(stdout.contains(r#""headers":{"content-type":"application/json"}"#));
}

#[test]
fn test_tos_api_help_metadata_example_is_registered() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "api", "object", "list", "--describe"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ve-tos object list"));
}

// [Review Fix #4] Keep this describe alignment case registered as a test after
// moving direct binary coverage into the dedicated Cargo entry crates.
#[test]
fn test_high_level_describe_without_business_args_is_aligned() {
    let cases: &[(&[&str], &str, &[&str])] = &[
        (
            &["ve-tos"],
            "ve-tos",
            &[
                "cp", "mv", "sync", "mb", "rb", "mkdir", "rm", "ls", "stat", "du", "find", "cat",
                "presign", "restore",
            ],
        ),
        (
            &["ve-adrive"],
            "ve-adrive",
            &[
                "cp", "mv", "sync", "crt", "del", "rm", "ls", "stat", "du", "find", "cat", "mkdir",
            ],
        ),
    ];

    for (tool_args, expected_prefix, commands) in cases {
        for command in *commands {
            let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
                .args(*tool_args)
                .args([*command, "--describe", "--output", "json"])
                .output()
                .expect("Failed to execute high-level describe");
            assert!(
                output.status.success(),
                "{} {} --describe failed: stderr={}",
                expected_prefix,
                command,
                String::from_utf8_lossy(&output.stderr)
            );
            let parsed: serde_json::Value =
                serde_json::from_slice(&output.stdout).expect("valid describe json");
            let payload = &parsed["data"];
            assert_eq!(
                payload["command"],
                format!("{expected_prefix} {command}"),
                "unexpected describe command payload"
            );
            assert!(payload["supports_dry_run"].as_bool().is_some());
            assert!(payload["supports_pipe"].as_bool().is_some());
            let routing = payload["scenario_routing"].as_object().expect("routing");
            for routing_key in [
                "target_resolution",
                "dry_run",
                "output",
                "progress",
                "checkpoint",
                "low_level_boundary",
            ] {
                assert!(
                    routing
                        .get(routing_key)
                        .and_then(|value| value.as_str())
                        .is_some(),
                    "{} {} describe missing {routing_key} routing: {payload:?}",
                    expected_prefix,
                    command
                );
            }
            if *command == "ls" {
                for routing_key in ["target_matrix", "output_shapes"] {
                    assert!(
                        routing
                            .get(routing_key)
                            .and_then(|value| value.as_str())
                            .is_some(),
                        "{} ls describe missing {routing_key} routing: {payload:?}",
                        expected_prefix
                    );
                }
                let examples = payload["output_filter_examples"]
                    .as_array()
                    .expect("output filter examples");
                assert!(
                    examples
                        .iter()
                        .any(|example| example.as_str().unwrap_or_default().contains("[*]")),
                    "{} ls describe missing concrete list query example: {payload:?}",
                    expected_prefix
                );
            }
            if *command == "rm" {
                for routing_key in ["target_scope", "destructive_guard", "recursive_delete"] {
                    assert!(
                        routing
                            .get(routing_key)
                            .and_then(|value| value.as_str())
                            .is_some(),
                        "{} rm describe missing {routing_key} routing: {payload:?}",
                        expected_prefix
                    );
                }
                let examples = payload["output_filter_examples"]
                    .as_array()
                    .expect("output filter examples");
                assert!(
                    examples.iter().any(|example| example
                        .as_str()
                        .unwrap_or_default()
                        .contains("data.impact")),
                    "{} rm describe missing dry-run impact query example: {payload:?}",
                    expected_prefix
                );
            }
            let parameters = payload["parameters"].as_array().expect("parameters");
            assert_parameter_schema_contract(
                parameters,
                &format!("{expected_prefix} {command} describe"),
            );
        }
    }
}

#[test]
fn test_high_level_describe_table_output_does_not_collapse_to_api_array() {
    for (surface, command) in [
        ("ve-adrive", "rm"),
        ("ve-tos", "rm"),
        ("ve-adrive", "ls"),
        ("ve-tos", "ls"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args([surface, command, "--describe", "--output", "table"])
            .output()
            .expect("Failed to execute high-level describe table");
        assert!(
            output.status.success(),
            "{surface} {command} --describe --output table failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("field") && stdout.contains("value"),
            "{surface} {command} describe table should render metadata field/value rows:\n{stdout}"
        );
        assert!(
            stdout.contains("parameters") || stdout.contains("scenario_routing"),
            "{surface} {command} describe table lost rich metadata:\n{stdout}"
        );
        assert!(
            stdout.contains("scenario_routing.target_resolution"),
            "{surface} {command} describe table should flatten nested routing metadata:\n{stdout}"
        );
        assert!(
            !stdout.lines().any(|line| line.contains("| index | value")),
            "{surface} {command} describe table collapsed to a raw array:\n{stdout}"
        );
    }
}

#[test]
fn test_high_level_describe_csv_output_uses_field_value_metadata_rows() {
    for (surface, command) in [
        ("ve-adrive", "rm"),
        ("ve-tos", "rm"),
        ("ve-adrive", "ls"),
        ("ve-tos", "ls"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args([surface, command, "--describe", "--output", "csv"])
            .output()
            .expect("Failed to execute high-level describe csv");
        assert!(
            output.status.success(),
            "{surface} {command} --describe --output csv failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.lines().next() == Some("field,value"),
            "{surface} {command} describe csv should render metadata field/value rows:\n{stdout}"
        );
        assert!(
            stdout.contains("scenario_routing.target_resolution"),
            "{surface} {command} describe csv should flatten nested routing metadata:\n{stdout}"
        );
        assert!(
            !stdout
                .lines()
                .next()
                .unwrap_or_default()
                .starts_with("index,value"),
            "{surface} {command} describe csv collapsed to a raw array:\n{stdout}"
        );
    }
}

#[test]
fn test_only_public_unified_top_level_commands_are_accepted() {
    let tos_output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(
        tos_output.status.success(),
        "tos is now a public unified top-level command: stderr={}",
        String::from_utf8_lossy(&tos_output.stderr)
    );

    for tool in ["adrive", "tosvector", "tostable"] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args([tool, "--help"])
            .output()
            .expect("Failed to execute");
        assert!(
            !output.status.success(),
            "{tool} must not be accepted as a unified top-level command"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("unrecognized subcommand '{tool}'")),
            "{tool} should fail as an unknown top-level command: {stderr}"
        );
    }
}

#[test]
fn test_adrive_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-adrive", "--help"])
        .output()
        .expect("Failed to execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cp"));
    assert!(stdout.contains("crt"));
    assert!(stdout.contains("del"));
    assert!(stdout.contains("sync"));
    assert!(stdout.contains("file"));
    assert!(stdout.contains("instance"));
    assert!(stdout.contains("Capabilities / Utilities"));
    assert!(stdout.contains("ADrive Target Syntax"));
    assert!(stdout.contains("--query <QUERY>"));
    assert!(stdout.contains("--confirm <RESOURCE>"));
    assert!(stdout.contains("Language:"));
    assert!(stdout.contains("--language <en|zh>"));
    assert!(stdout.contains("Help output language"));
    assert!(stdout.contains("--help --language zh"));
    assert!(stdout.contains("Include extra diagnostic output where supported"));
    assert!(stdout.contains("Disable prompts and progress output"));
    assert!(!stdout.contains("--trace-dir"));
    assert!(!stdout.contains("--trace-redact"));
    assert!(!stdout.contains("--control-endpoint"));
    assert!(!stdout.contains("--account-id"));
    assert!(stdout.contains("Examples:"));
    assert!(stdout.contains("General:"));
    assert!(stdout.contains("Run 've-storage-uni-cli ve-adrive <command> --help'"));

    let subcommand_output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-adrive", "mkdir", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(subcommand_output.status.success());
    let subcommand_stdout = String::from_utf8_lossy(&subcommand_output.stdout);
    assert!(!subcommand_stdout.contains("--control-endpoint"));
    assert!(!subcommand_stdout.contains("--account-id"));
}

#[test]
fn test_adrive_rejects_control_plane_global_flags() {
    for (flag, value) in [
        ("--control-endpoint", "https://tos-control.example.com"),
        ("--account-id", "2100000001"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(["--output", "json", flag, value, "ve-adrive", "doctor"])
            .output()
            .expect("Failed to execute");
        assert!(!output.status.success(), "flag={flag}");
        assert_eq!(output.status.code(), Some(6), "flag={flag}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(flag), "stderr={stderr}");
        assert!(stderr.contains("does not support"), "stderr={stderr}");
        let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
        assert_eq!(parsed["success"], false);
        assert_eq!(parsed["error"]["kind"], "validation_error");
    }
}

#[test]
fn test_unified_ve_adrive_scope_hides_control_plane_global_flags() {
    let help = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-adrive", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(!stdout.contains("--control-endpoint"));
    assert!(!stdout.contains("--account-id"));
}

#[test]
fn test_tos_bucket_list_json_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-tos", "bucket", "list"])
        .output()
        .expect("Failed to execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("Output should be valid JSON");
        assert_eq!(parsed["success"], true);
        assert_eq!(parsed["status"], "success");
        assert!(parsed["data"].is_object());
        assert!(parsed["request_id"].is_string());
        assert!(parsed["status_code"].is_null());
        assert!(parsed["ec"].is_null());
    } else {
        let parsed: serde_json::Value = serde_json::from_str(stderr.trim())
            .expect("Error output should be valid JSON envelope");
        assert_eq!(parsed["success"], false);
        assert_eq!(parsed["status"], "failed");
        assert!(parsed["request_id"].is_string());
        assert!(parsed["ec"].is_string());
        assert!(parsed["error"]["code"].is_string());
        assert!(parsed["error"]["message"].is_string());
    }
}

#[test]
fn test_tos_describe_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--describe", "ve-tos", "bucket", "list"])
        .output()
        .expect("Failed to execute");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("Describe output should be valid JSON");
    assert_eq!(parsed["success"], true);
    assert!(parsed["data"].is_object());
    assert!(parsed["request_id"].is_string());
    assert!(
        parsed.get("tool").is_some() || parsed.get("command").is_some(),
        "Describe output should have 'tool' or 'command' field"
    );
}

#[test]
fn test_tos_group_describe_consumes_registry() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "--describe"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid describe json");
    // [Review Fix #1] 顶层 --describe 现在统一走 Envelope；原始 describe 内容下沉到 data。
    let payload = parsed.get("data").unwrap_or(&parsed);
    let groups = payload["groups"].as_array().expect("groups array");
    for expected in [
        "capabilities",
        "api",
        "completion",
        "serve",
        "skill",
        "doctor",
    ] {
        assert!(
            groups.iter().any(|group| group["name"] == expected
                && group["category"] == "utilities"
                && group["layer"] == "meta"),
            "missing registry-backed utility group {expected}"
        );
    }
}

#[test]
fn test_tos_cp_command_parse() {
    let output = cli_with_empty_home(
        "tos-cp-command-parse",
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "cp",
            "./local",
            "tos://bucket/key",
        ],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    // [Review Fix #1] 成功路径已统一 Envelope；原始 plan 字段下沉到 data。
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert_eq!(payload["command"], "ve-tos cp");
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["execution_status"], "planned_not_executed");
}

#[test]
fn test_tos_cp_dry_run_supports_xml_output() {
    let output = cli_with_empty_home(
        "tos-cp-dry-run-xml",
        &[
            "--dry-run",
            "--output",
            "xml",
            "ve-tos",
            "cp",
            "./local",
            "tos://bucket/key",
        ],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(stdout.contains("<command>ve-tos cp</command>"));
    assert!(stdout.contains("<dry_run>true</dry_run>"));
}

#[test]
fn test_single_file_remote_directory_destination_uses_source_file_name() {
    let tos_cp = cli_with_empty_home(
        "single-file-tos-cp",
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "cp",
            "folder/1.txt",
            "tos://bucket",
        ],
    );
    assert!(tos_cp.status.success());
    let stdout = String::from_utf8_lossy(&tos_cp.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert_eq!(payload["destination"], "tos://bucket/1.txt");

    let tos_download = cli_with_empty_home(
        "single-file-tos-download-local-dir",
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "cp",
            "tos://bucket/folder/1.txt",
            "./temp/",
        ],
    );
    assert!(tos_download.status.success());
    let stdout = String::from_utf8_lossy(&tos_download.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert_eq!(
        payload["destination"],
        format!("./temp{}1.txt", std::path::MAIN_SEPARATOR)
    );

    let tos_mv = cli_with_empty_home(
        "single-file-tos-mv",
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "mv",
            "tos://source-bucket/folder/1.txt",
            "tos://bucket/",
        ],
    );
    assert!(tos_mv.status.success());
    let stdout = String::from_utf8_lossy(&tos_mv.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert_eq!(payload["destination"], "tos://bucket/1.txt");

    let tos_same_object = cli_with_empty_home(
        "single-file-tos-same-object",
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "mv",
            "tos://bucket/folder/1.txt",
            "tos://bucket/folder/",
        ],
    );
    assert!(!tos_same_object.status.success());
    let stderr = String::from_utf8_lossy(&tos_same_object.stderr);
    assert!(
        stderr.contains("source and destination resolve to the same TOS object"),
        "stderr={stderr}"
    );

    let adrive_cp = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "cp",
            "folder/1.txt",
            "adrive://inst/space",
        ])
        .output()
        .expect("Failed to execute");
    assert!(adrive_cp.status.success());
    let stdout = String::from_utf8_lossy(&adrive_cp.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert!(payload["description"]
        .as_str()
        .unwrap_or_default()
        .contains("adrive://inst/space/1.txt"));

    let adrive_mv = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "mv",
            "adrive://inst/space/folder/1.txt",
            "adrive://inst/space/docs/",
        ])
        .output()
        .expect("Failed to execute");
    assert!(adrive_mv.status.success());
    let stdout = String::from_utf8_lossy(&adrive_mv.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert!(payload["description"]
        .as_str()
        .unwrap_or_default()
        .contains("adrive://inst/space/docs/1.txt"));

    let adrive_same_file = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "mv",
            "adrive://inst/space/docs/1.txt",
            "adrive://inst/space/docs/",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!adrive_same_file.status.success());
    let stderr = String::from_utf8_lossy(&adrive_same_file.stderr);
    assert!(
        stderr.contains("source and destination resolve to the same ADrive file"),
        "stderr={stderr}"
    );
}

#[test]
fn test_tos_path_traversal_requires_force_and_confirm() {
    let dry_run = cli_with_empty_home(
        "tos-path-traversal-dry-run",
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "cp",
            "tos://bucket/root/../escape.txt",
            "./out/",
        ],
    );
    assert!(dry_run.status.success());
    let stdout = String::from_utf8_lossy(&dry_run.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let payload = parsed.get("data").unwrap_or(&parsed);
    let warnings = payload["warnings"]
        .as_array()
        .expect("warnings array")
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        warnings.contains("path traversal"),
        "warnings should mention path traversal: {warnings}"
    );

    let no_force = cli_with_empty_home(
        "tos-path-traversal-no-force",
        &["ve-tos", "cp", "tos://bucket/root/../escape.txt", "./out/"],
    );
    assert!(!no_force.status.success());
    let stderr = String::from_utf8_lossy(&no_force.stderr);
    assert!(stderr.contains("path traversal"), "stderr={stderr}");
    assert!(stderr.contains("--force"), "stderr={stderr}");
    assert!(stderr.contains("--confirm"), "stderr={stderr}");

    let force_without_confirm = cli_with_empty_home(
        "tos-path-traversal-force-no-confirm",
        &[
            "ve-tos",
            "cp",
            "tos://bucket/root/../escape.txt",
            "./out/",
            "--force",
        ],
    );
    assert!(!force_without_confirm.status.success());
    let stderr = String::from_utf8_lossy(&force_without_confirm.stderr);
    assert!(stderr.contains("path traversal"), "stderr={stderr}");
    assert!(stderr.contains("--confirm"), "stderr={stderr}");
}

#[test]
fn test_tos_path_traversal_force_confirm_allows_local_copy() {
    let root = std::env::temp_dir().join(format!(
        "ve-storage-uni-cli-tos-path-traversal-confirm-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    let home = root.join("home");
    let work = root.join("work");
    let out = work.join("out");
    fs::create_dir_all(&home).expect("create home");
    fs::create_dir_all(&out).expect("create output directory");
    fs::write(root.join("source.txt"), "ok").expect("write source file");

    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env("HOME", &home)
        .current_dir(&work)
        .args([
            "--confirm",
            "../source.txt",
            "ve-tos",
            "cp",
            "../source.txt",
            "./out/",
            "--force",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(out.join("source.txt")).expect("read copied file"),
        "ok"
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn test_tos_added_high_level_command_skeletons_parse() {
    let cases: &[&[&str]] = &[
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "mv",
            "tos://bucket/source",
            "tos://bucket/destination",
            "--checkpoint-dir",
            "/tmp/tos-checkpoints",
            "--report-path",
            "/tmp/tos-mv-report.csv",
            "--no-progress",
            "--force",
        ],
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "mb",
            "tos://bucket",
            "--storage-class",
            "STANDARD",
            "--bucket-type",
            "hns",
        ],
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "rb",
            "tos://bucket",
            "--force",
        ],
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "mkdir",
            "tos://bucket/folder",
        ],
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "stat",
            "tos://bucket/key",
            "--version-id",
            "v1",
        ],
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "restore",
            "tos://bucket/key",
            "--recursive",
            "--days",
            "7",
            "--tier",
            "Standard",
            "--report-path",
            "/tmp/tos-restore-report.csv",
            "--force",
        ],
    ];

    for args in cases {
        let output = cli_with_empty_home("tos-high-level-skeletons", args);
        assert!(
            output.status.success(),
            "args={args:?}\nstderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("Should be valid JSON");
        // [Review Fix #1] 成功路径统一 Envelope，dry-run 字段下沉到 data。
        let payload = parsed.get("data").unwrap_or(&parsed);
        assert_eq!(payload["dry_run"], true);
        assert_eq!(payload["execution_status"], "planned_not_executed");
    }
}

#[test]
fn test_high_level_manifest_and_report_filter_flags_are_exposed() {
    for args in [
        ["ve-tos", "cp", "--help"],
        ["ve-tos", "rm", "--help"],
        ["ve-adrive", "cp", "--help"],
        ["ve-adrive", "rm", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(&args)
            .output()
            .expect("Failed to execute");
        assert!(output.status.success(), "args={args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("--manifest-path"), "args={args:?}");
        assert!(stdout.contains("--no-manifest"), "args={args:?}");
        assert!(stdout.contains("--report-failures-only"), "args={args:?}");
    }

    for args in [
        ["ve-tos", "ls", "--help"],
        ["ve-tos", "du", "--help"],
        ["ve-tos", "find", "--help"],
        ["ve-adrive", "ls", "--help"],
        ["ve-adrive", "du", "--help"],
        ["ve-adrive", "find", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(&args)
            .output()
            .expect("Failed to execute");
        assert!(output.status.success(), "args={args:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("--manifest-path"), "args={args:?}");
        assert!(!stdout.contains("--report-failures-only"), "args={args:?}");
        assert!(!stdout.contains("--no-manifest"), "args={args:?}");
    }
}

#[test]
fn test_no_manifest_conflicts_with_manifest_path() {
    for args in vec![
        vec![
            "ve-tos",
            "cp",
            "./local",
            "tos://bucket/key",
            "--manifest-path",
            "/tmp/manifest.csv",
            "--no-manifest",
        ],
        vec![
            "ve-adrive",
            "cp",
            "./local",
            "adrive://inst/space/key",
            "--manifest-path",
            "/tmp/manifest.csv",
            "--no-manifest",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(&args)
            .output()
            .expect("Failed to execute");
        assert!(!output.status.success(), "args={args:?}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("--manifest-path"), "stderr={stderr}");
        assert!(stderr.contains("--no-manifest"), "stderr={stderr}");
    }
}

#[test]
fn test_tos_mkdir_dry_run_normalizes_folder_target() {
    let output = cli_with_empty_home(
        "tos-mkdir-dry-run",
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "mkdir",
            "--bucket",
            "bucket",
            "--key",
            "folder/subfolder",
            "--parents",
        ],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert_eq!(payload["command"], "ve-tos mkdir");
    assert_eq!(payload["target"], "tos://bucket/folder/subfolder/");
    assert_eq!(payload["batch"]["enabled"], true);
    assert_eq!(payload["dry_run"], true);
}

#[test]
fn test_tos_object_upload_command_parse() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "object",
            "upload",
            "--bucket",
            "my-bucket",
            "--key",
            "test.txt",
            "--body",
            "./test.txt",
        ])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert_eq!(payload["dry_run"], true);
}

#[test]
fn test_tos_object_list_accepts_uri_or_bucket_flag_only() {
    let cases: &[&[&str]] = &[
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "object",
            "list",
            "tos://my-bucket/prefix/",
        ],
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "object",
            "list",
            "--bucket",
            "my-bucket",
            "--prefix",
            "prefix/",
        ],
    ];

    for args in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(*args)
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "args={args:?}\nstderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value =
            serde_json::from_str(&stdout).expect("Should be valid JSON");
        let payload = parsed.get("data").unwrap_or(&parsed);
        assert_eq!(payload["dry_run"], true);
    }
}

#[test]
fn test_tos_object_list_rejects_uri_prefix_flag_mix() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "object",
            "list",
            "tos://my-bucket",
            "--prefix",
            "prefix/",
        ])
        .output()
        .expect("Failed to execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cannot be used with"), "stderr={stderr}");
    assert!(stderr.contains("--prefix"), "stderr={stderr}");
}

#[test]
fn test_tos_object_list_rejects_bare_positional_bucket() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "object",
            "list",
            "my-bucket",
        ])
        .output()
        .expect("Failed to execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("tos://bucket"), "stderr={stderr}");
    assert!(stderr.contains("--bucket <bucket>"), "stderr={stderr}");
}

#[test]
fn test_tos_object_list_rejects_uri_bucket_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "object",
            "list",
            "--bucket",
            "tos://my-bucket/prefix/",
        ])
        .output()
        .expect("Failed to execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--bucket expects a bucket name only"),
        "stderr={stderr}"
    );
    assert!(
        stderr.contains("tos://bucket for URI style"),
        "stderr={stderr}"
    );
}

#[test]
fn test_tosvector_is_not_a_public_top_level_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "tosvector",
            "data",
            "search",
            "--bucket",
            "vbucket",
            "--index-name",
            "idx1",
            "--vector",
            "[1.0, 2.0, 3.0]",
            "--top-k",
            "5",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unrecognized subcommand 'tosvector'"),
        "stderr={stderr}"
    );
}

#[test]
fn test_adrive_mkdir_parse() {
    // Low-level `file list` was removed; verify a high-level command parses and
    // honors --dry-run instead.
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "mkdir",
            "--instance",
            "inst1",
            "--space",
            "sp1",
            "--folder",
            "/foo",
            "--parents",
        ])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["summary"]["recursive"], true);
}

#[test]
fn test_adrive_capabilities_schema_matches_agent_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-adrive",
            "capabilities",
            "--view",
            "full",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let data = &parsed["data"];
    assert!(data["groups"].as_array().expect("groups").len() >= 2);
    assert!(data["capabilities"].as_array().expect("capabilities").len() >= 19);
    assert!(data["commands"].as_array().expect("commands").len() >= 19);
    assert!(data["search_scores"].as_array().is_some());
    assert_eq!(data["uri_format"], "adrive://instance/space/folder/file");
    // [Review Fix #2] Capture capability rows before deriving semantics coverage.
    let capabilities = data["capabilities"].as_array().expect("capabilities");
    let semantics = data["high_level_semantics"]
        .as_object()
        .expect("high_level_semantics");
    // [Review Fix #1] Derive the expected semantics keys from capability rows
    // so future high-level commands cannot be added without discovery text.
    for command in capabilities
        .iter()
        .filter(|capability| capability["layer"] == "high-level")
        .filter_map(|capability| capability["domain"].as_str())
    {
        assert!(
            semantics
                .get(command)
                .and_then(|value| value.as_array())
                .is_some_and(|entries| !entries.is_empty()),
            "missing high-level semantics for ve-adrive {command}"
        );
    }
    assert!(capabilities
        .iter()
        .any(|capability| capability["command"] == "ve-adrive crt"));
    assert!(capabilities
        .iter()
        .any(|capability| capability["command"] == "ve-adrive del"));
    assert!(
        capabilities.iter().all(|capability| !capability["command"]
            .as_str()
            .unwrap_or_default()
            .starts_with("adrive ")),
        "ve-adrive capabilities must expose public ve-adrive command paths"
    );
    // [Review Fix #1] ADrive capabilities must expose the same execution-safety
    // metadata that ADrive describe exposes for agents.
    assert!(capabilities.iter().all(|capability| capability
        .get("supports_dry_run")
        .and_then(|value| value.as_bool())
        .is_some()));
    assert!(capabilities
        .iter()
        .filter(|capability| capability["destructive"] == true)
        .all(|capability| capability["supports_force"] == true
            && capability["supports_dry_run"] == true));
    // [Review Fix #3] Critical ADrive examples are copied by agents; keep them
    // executable in non-interactive shells by showing the exact confirmation
    // gate alongside --force.
    for capability in capabilities.iter().filter(|capability| {
        capability["command"] == "ve-adrive del" || capability["command"] == "ve-adrive rm"
    }) {
        let examples = capability["examples"].as_array().expect("examples");
        assert!(
            examples.iter().all(|example| example
                .as_str()
                .unwrap_or_default()
                .contains("--confirm adrive://")),
            "critical capability examples must include --confirm: {capability:?}"
        );
    }
    assert!(!data["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .any(|group| group["name"] == "Low-Level API" || group["layer"] == "low_level"));
    assert!(!data["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .any(|cap| cap["layer"] == "low-level"
            || cap["command"]
                .as_str()
                .unwrap_or_default()
                .starts_with("ve-adrive api instance")));
}

#[test]
fn test_capabilities_table_views_use_aligned_rows() {
    for args in [
        ["ve-tos", "capabilities", "--view", "groups"].as_slice(),
        ["ve-adrive", "capabilities", "--view", "groups"].as_slice(),
        ["ve-tos", "capabilities", "--view", "full", "--group", "cp"].as_slice(),
        [
            "ve-adrive",
            "capabilities",
            "--view",
            "full",
            "--group",
            "cp",
        ]
        .as_slice(),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(["--output", "table"])
            .args(args)
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        // [Review Fix #3] Capabilities table output should select the row set
        // for the requested view, not flatten helper metadata into field/value rows.
        assert!(!stdout.contains("| field"));
        assert!(!stdout.contains("high_level_semantics"));
        assert!(stdout.contains("command"));
        assert!(stdout.contains("group"));
        assert!(stdout.contains("description"));
        if args == ["ve-tos", "capabilities", "--view", "groups"].as_slice()
            || args == ["ve-adrive", "capabilities", "--view", "groups"].as_slice()
        {
            assert!(!stdout.contains("category"));
        }
    }
}

#[test]
fn test_capabilities_table_query_owns_shape() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "table",
            "--query",
            "data.groups",
            "ve-tos",
            "capabilities",
            "--view",
            "full",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("command_count"));
    assert!(!stdout.contains("risk_level"));
}

#[test]
fn test_adrive_parse_error_uses_failed_envelope() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-adrive", "cp"])
        .output()
        .expect("Failed to execute");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
    assert_eq!(parsed["status"], "failed");
    assert_eq!(parsed["command"], "ve-adrive cp");
    assert_eq!(parsed["error"]["kind"], "validation_error");
    assert_eq!(parsed["error"]["exit_code"], 6);
    assert_eq!(parsed["error"]["fix_command"], "ve-adrive <command> --help");
}

#[test]
fn test_adrive_runtime_errors_use_public_command_names() {
    let cases = [
        (
            ["ve-adrive", "crt", "adrive://inst/space/path"].as_slice(),
            "ve-adrive crt",
            "ve-adrive mkdir",
        ),
        (
            ["ve-adrive", "rm", "adrive://inst/space"].as_slice(),
            "ve-adrive rm",
            "ve-adrive del",
        ),
    ];

    for (args, expected_command, expected_hint) in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .arg("--output")
            .arg("json")
            .args(args)
            .output()
            .expect("Failed to execute");

        assert!(!output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let json_text = if stdout.trim().is_empty() {
            stderr.trim()
        } else {
            stdout.trim()
        };
        let parsed: serde_json::Value = serde_json::from_str(json_text).expect("valid json");
        let message = parsed["error"]["message"].as_str().expect("message");
        assert_eq!(parsed["command"], expected_command);
        assert!(message.contains(expected_command), "message={message}");
        assert!(message.contains(expected_hint), "message={message}");
        // [Review Fix #4] Public error text must not suggest the internal
        // ADrive registry command after the unified top-level command changed.
        assert!(
            !message.contains("Validation error: adrive"),
            "message={message}"
        );
        assert!(!message.contains("use adrive"), "message={message}");
    }
}

#[test]
fn test_adrive_high_level_describe_and_dry_run_have_stable_envelope() {
    let describe = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--describe",
            "ve-adrive",
            "ls",
            "adrive://inst/space/",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        describe.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&describe.stderr)
    );
    let stdout = String::from_utf8_lossy(&describe.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["command"], "ve-adrive ls");
    assert_eq!(parsed["data"]["command"], "ve-adrive ls");
    assert_eq!(parsed["data"]["supports_dry_run"], true);
    assert!(parsed["data"]["low_level_apis"].as_array().unwrap().len() >= 1);
    assert!(parsed["data"]["wraps_apis"].as_array().unwrap().len() >= 1);
    assert!(parsed["data"]["output_filter_examples"]
        .as_array()
        .unwrap()
        .iter()
        .any(|example| example.as_str().unwrap_or_default().contains("--query")));

    let dry_run = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "ls",
            "adrive://inst/space/",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        dry_run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let stdout = String::from_utf8_lossy(&dry_run.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["command"], "ve-adrive ls");
    assert_eq!(parsed["data"]["command"], "ve-adrive ls");
    assert_eq!(parsed["data"]["dry_run"], true);
}

#[test]
fn test_adrive_recursive_transfer_help_exposes_include_parent() {
    for args in [
        ["ve-adrive", "cp", "--help"],
        ["ve-adrive", "mv", "--help"],
        ["ve-adrive", "sync", "--help"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(args)
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("--include-parent"));
    }
}

#[test]
fn test_adrive_critical_delete_requires_exact_confirm_after_force() {
    let target = "adrive://inst/space/docs/a.txt";

    let dry_run = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--dry-run", "--output", "json", "ve-adrive", "rm", target])
        .output()
        .expect("Failed to execute");
    assert!(
        dry_run.status.success(),
        "dry-run should not require force/confirm: stderr={}",
        String::from_utf8_lossy(&dry_run.stderr)
    );

    let forced = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-adrive", "rm", target, "--force"])
        .output()
        .expect("Failed to execute");
    assert!(!forced.status.success());
    let stderr = String::from_utf8_lossy(&forced.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
    assert_eq!(parsed["error"]["kind"], "validation_error");
    assert!(parsed["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("critical delete command"));
    assert!(parsed["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("--confirm adrive://inst/space/docs/a.txt"));

    // [Review Fix #2] Wrong confirmation must fail before credentials or network are touched.
    let wrong_confirm = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--confirm",
            "adrive://inst/space/other.txt",
            "ve-adrive",
            "rm",
            target,
            "--force",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!wrong_confirm.status.success());
    let stderr = String::from_utf8_lossy(&wrong_confirm.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
    assert!(parsed["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("does not match the critical resource"));
}

#[test]
fn test_adrive_mv_requires_source_confirm_after_force() {
    let source = "adrive://inst/space/docs/source.txt";
    let destination = "adrive://inst/space/docs/destination.txt";

    let forced = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-adrive",
            "mv",
            source,
            destination,
            "--force",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!forced.status.success());
    let stderr = String::from_utf8_lossy(&forced.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
    let message = parsed["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("--confirm adrive://inst/space/docs/source.txt"));

    let wrong_confirm = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--confirm",
            destination,
            "ve-adrive",
            "mv",
            source,
            destination,
            "--force",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!wrong_confirm.status.success());
    let stderr = String::from_utf8_lossy(&wrong_confirm.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
    let message = parsed["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("does not match"));
    assert!(message.contains(source));
}

#[test]
fn test_adrive_rm_recursive_delete_mode_is_described_and_planned() {
    let help = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-adrive", "rm", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(help.status.success());
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("--recursive-delete-mode"));
    assert!(stdout.contains("--include-uploads"));
    assert!(stdout.contains("--checkpoint-dir"));

    let dry_run = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "rm",
            "adrive://inst/space/docs/",
            "--recursive",
            "--include-uploads",
            "--checkpoint-dir",
            "/tmp/adrive-upload-checkpoints",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        dry_run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&dry_run.stderr)
    );
    let stdout = String::from_utf8_lossy(&dry_run.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert_eq!(payload["summary"]["recursive"], true);
    assert_eq!(payload["summary"]["recursive_delete_mode"], "bottom-up");
    assert!(payload["request_plan"]
        .to_string()
        .contains("delete_file leaf entries"));
    assert!(payload["request_plan"]
        .to_string()
        .contains("delete_folder bottom-up"));
    assert!(payload["request_plan"]
        .to_string()
        .contains("abort_multipart_upload"));
    assert_eq!(payload["checkpoint"]["enabled"], true);
    assert_eq!(
        payload["checkpoint"]["directory"],
        "/tmp/adrive-upload-checkpoints"
    );

    let root_dry_run = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "rm",
            "adrive://inst/space",
            "--recursive",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        root_dry_run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&root_dry_run.stderr)
    );
    let stdout = String::from_utf8_lossy(&root_dry_run.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert_eq!(payload["description"], "Delete adrive://inst/space");
    assert_eq!(payload["summary"]["recursive"], true);

    let non_recursive_root = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "rm",
            "adrive://inst/space",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!non_recursive_root.status.success());
    let stderr = String::from_utf8_lossy(&non_recursive_root.stderr);
    assert!(stderr.contains("add --recursive to clear a space"));

    let direct_root = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "rm",
            "adrive://inst/space",
            "--recursive",
            "--recursive-delete-mode",
            "direct",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!direct_root.status.success());
    let stderr = String::from_utf8_lossy(&direct_root.stderr);
    assert!(stderr.contains("cannot target a space root"));
}

#[test]
fn test_adrive_sync_delete_requires_exact_confirm_after_force() {
    let root = std::env::temp_dir().join(format!(
        "ve-storage-uni-cli-adrive-sync-confirm-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    let source = root.join("source");
    fs::create_dir_all(&source).expect("create source directory");
    fs::write(source.join("keep.txt"), "keep").expect("write source file");
    let destination = "adrive://inst/space/backup";

    let forced = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-adrive",
            "sync",
            source.to_str().expect("source path"),
            destination,
            "--delete",
            "--force",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!forced.status.success());
    let stderr = String::from_utf8_lossy(&forced.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
    let message = parsed["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("--confirm adrive://inst/space/backup"),
        "message={message}"
    );

    let wrong_confirm = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--confirm",
            "adrive://inst/space/other",
            "ve-adrive",
            "sync",
            source.to_str().expect("source path"),
            destination,
            "--delete",
            "--force",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!wrong_confirm.status.success());
    let stderr = String::from_utf8_lossy(&wrong_confirm.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
    assert!(parsed["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("does not match the critical resource"));
}

#[test]
fn test_adrive_describe_without_required_positionals_recovers() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "--describe", "ve-adrive", "cp"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["command"], "ve-adrive cp");
    assert_eq!(parsed["data"]["command"], "ve-adrive cp");
    assert_eq!(parsed["data"]["supports_dry_run"], true);
}

#[test]
fn test_adrive_config_group_describe_matches_capabilities_contract() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "--describe", "ve-adrive", "config"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["command"], "ve-adrive config");
    assert_eq!(parsed["data"]["kind"], "command_group");
    // [Review Fix #ADrive-ConfigDescribe-Test] Capabilities marks this group
    // as describable; the handler must return subcommand metadata directly.
    assert_eq!(parsed["data"]["supports_describe"], true);
    assert!(parsed["data"]["subcommands"].as_array().unwrap().len() >= 3);
}

#[test]
fn test_adrive_config_set_echoes_non_sensitive_values() {
    let output = cli_with_empty_home(
        "adrive-config-set-nonsensitive",
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "region",
            "cn-beijing",
        ],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid config set json");
    assert_eq!(parsed["command"], "ve-adrive config set");
    assert_eq!(parsed["data"]["section"], "[default.adrive]");
    assert_eq!(parsed["data"]["field"], "region");
    assert_eq!(parsed["data"]["value"], "cn-beijing");
    assert_eq!(parsed["data"]["encrypted"], false);
    assert!(parsed["data"]["config_path"].is_string());
}

#[test]
fn test_adrive_completion_help_documents_installation() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-adrive", "completion", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Install examples"), "stdout={stdout}");
    assert!(stdout.contains("data.script"), "stdout={stdout}");
    assert!(stdout.contains("jq -r"), "stdout={stdout}");
}

#[test]
fn test_adrive_completion_registers_unified_and_direct_entries() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-adrive", "completion", "bash"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid completion json");
    let script = parsed["data"]["script"].as_str().expect("script");
    assert!(script.contains("ve-adrive"), "script={script}");
    assert!(script.contains("ve-adrive-cli"), "script={script}");
    assert!(script.contains("ve-storage-uni-cli"), "script={script}");
    assert!(script.contains("cp"), "script={script}");
    assert!(script.contains("du"), "script={script}");
}

#[test]
fn test_adrive_skill_describe_explains_export_and_serve_relationship() {
    let export_dir =
        std::env::temp_dir().join(format!("adrive-skill-describe-{}", std::process::id()));
    if export_dir.exists() {
        let _ = std::fs::remove_dir_all(&export_dir);
    }
    let export_dir_arg = export_dir.to_string_lossy().to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-adrive",
            "skill",
            "export",
            "--dir",
            &export_dir_arg,
            "--describe",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !export_dir.exists(),
        "describe must not create export directory {}",
        export_dir.display()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["command"], "ve-adrive skill export");
    assert!(parsed["data"]["scenario_routing"]["format"]
        .as_str()
        .unwrap()
        .contains("Markdown SKILL.md"));
    assert!(parsed["data"]["scenario_routing"]["serve_relationship"]
        .as_str()
        .unwrap()
        .contains("does not read"));
}

#[test]
fn test_adrive_skill_schema_documents_usage_and_execute() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-adrive", "skill", "list"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let adrive_ls = parsed["data"]["skills"]
        .as_array()
        .expect("skills")
        .iter()
        .find(|skill| skill["name"] == "ve_adrive_ls")
        .expect("ve_adrive_ls skill");
    let adrive_cp = parsed["data"]["skills"]
        .as_array()
        .expect("skills")
        .iter()
        .find(|skill| skill["name"] == "ve_adrive_cp")
        .expect("ve_adrive_cp skill");
    let properties = adrive_ls["input_schema"]["properties"]
        .as_object()
        .expect("input properties");
    let cp_properties = adrive_cp["input_schema"]["properties"]
        .as_object()
        .expect("cp input properties");
    assert_eq!(adrive_ls["schema_version"], "adrive-skill-v1");
    assert_eq!(adrive_ls["usage"]["format"], "Markdown SKILL.md");
    assert_eq!(adrive_ls["usage"]["serve_reads_exported_files"], false);
    assert_eq!(
        adrive_ls["usage"]["mcp_server"],
        "ve-storage-uni-cli ve-adrive serve --mcp"
    );
    assert!(properties.contains_key("execute"));
    assert!(properties.contains_key("path"));
    assert_eq!(cp_properties["include-parent"]["type"], "boolean");
    let examples = adrive_ls["examples"].as_array().expect("examples");
    assert!(examples.iter().any(|example| example
        .as_str()
        .unwrap_or_default()
        .starts_with("ve-storage-uni-cli ve-adrive ls ")));
}

#[test]
fn test_adrive_skill_export_writes_markdown_skill_pack() {
    let dir = std::env::temp_dir().join(format!(
        "ve-storage-uni-cli-adrive-skill-export-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);

    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-adrive",
            "skill",
            "export",
            "--name",
            "ve_adrive_ls",
            "--dir",
            dir.to_str().expect("temp dir"),
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid export json");
    assert_eq!(parsed["data"]["format"], "markdown_skill");
    assert!(dir.join("SKILL.md").exists());
    let skill_path = dir
        .join("adrive-transfer")
        .join("ve_adrive_ls")
        .join("SKILL.md");
    let markdown = fs::read_to_string(&skill_path).expect("read exported ve-adrive skill");
    assert!(
        markdown.starts_with("# ve_adrive_ls"),
        "markdown={markdown}"
    );
    assert!(markdown.contains("`ve-adrive ls`"), "markdown={markdown}");
    assert!(markdown.contains("```json"), "markdown={markdown}");
    let zh_dir = std::env::temp_dir().join(format!(
        "ve-storage-uni-cli-adrive-skill-export-zh-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&zh_dir);
    let zh_output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-adrive",
            "skill",
            "export",
            "--name",
            "ve_adrive_ls",
            "--dir",
            zh_dir.to_str().expect("temp dir"),
            "--language",
            "zh",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        zh_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&zh_output.stderr)
    );
    let zh_parsed: serde_json::Value =
        serde_json::from_slice(&zh_output.stdout).expect("valid zh export json");
    assert_eq!(zh_parsed["data"]["language"], "zh");
    let zh_markdown = fs::read_to_string(
        zh_dir
            .join("adrive-transfer")
            .join("ve_adrive_ls")
            .join("SKILL.md"),
    )
    .expect("read exported zh ve-adrive skill");
    assert!(zh_markdown.contains("## 说明"), "markdown={zh_markdown}");
    assert!(zh_markdown.contains("参数说明"), "markdown={zh_markdown}");
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&zh_dir);
}

#[test]
fn test_adrive_serve_describe_documents_mcp_transport_details() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--describe",
            "ve-adrive",
            "serve",
            "--mcp",
            "--transport",
            "sse",
            "--port",
            "19091",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["command"], "ve-adrive serve");
    assert_eq!(parsed["data"]["protocol"], "MCP standard protocol via rmcp");
    assert_eq!(parsed["data"]["tcp_listener"], true);
    assert_eq!(parsed["data"]["bind"], "127.0.0.1:19091");
    assert!(parsed["data"]["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .any(|endpoint| endpoint == "/sse"));
    assert!(parsed["data"]["tool_source"]
        .as_str()
        .unwrap()
        .contains("exported Markdown skill files are not read"));
}

#[test]
fn test_adrive_capabilities_filters_normalize_group_and_layer() {
    for args in [
        [
            "ve-adrive",
            "capabilities",
            "--layer",
            "high_level",
            "--view",
            "full",
        ],
        [
            "ve-adrive",
            "capabilities",
            "--group",
            "high_level",
            "--view",
            "full",
        ],
        [
            "ve-adrive",
            "capabilities",
            "--group",
            "utilities",
            "--view",
            "full",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(["--output", "json"])
            .args(args)
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "args={args:?}\nstderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
        assert!(
            !parsed["data"]["capabilities"]
                .as_array()
                .expect("capabilities")
                .is_empty(),
            "args={args:?}"
        );
    }
}

#[test]
fn test_adrive_help_examples_use_adrive_uri_only() {
    for command in [
        "cp", "mv", "sync", "crt", "del", "rm", "ls", "stat", "du", "find", "cat", "mkdir",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(["ve-adrive", command, "--help"])
            .output()
            .expect("Failed to execute");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Examples:"), "command={command}");
        assert!(stdout.contains("adrive://"), "command={command}");
        assert!(!stdout.contains("ids://"), "command={command}");
        if matches!(command, "del" | "rm") {
            assert!(
                stdout.contains("--confirm adrive://"),
                "critical examples must include exact confirmation: command={command}"
            );
        }
    }
}

#[test]
fn test_adrive_high_level_help_exposes_by_name() {
    for command in [
        "cp", "mv", "sync", "crt", "del", "rm", "ls", "stat", "du", "find", "cat", "put", "mkdir",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(["ve-adrive", command, "--help"])
            .output()
            .expect("Failed to execute");
        assert!(
            output.status.success(),
            "command={command}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("--by-name"),
            "high-level command must expose --by-name: command={command}"
        );
    }
}

#[test]
fn test_adrive_api_describe_marks_raw_execution_unimplemented() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-adrive",
            "api",
            "instance",
            "create",
            "--describe",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["command"], "ve-adrive api instance create");
    assert_eq!(parsed["data"]["mode"], "guarded_utility_passthrough");
    assert_eq!(parsed["data"]["layer"], "meta");
    assert_eq!(parsed["data"]["raw_api_execution_implemented"], false);
    assert_eq!(parsed["data"]["supports_force"], false);
    assert_eq!(parsed["data"]["capability"]["command"], "ve-adrive api");
}

#[test]
fn test_adrive_api_force_execution_is_unimplemented() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-adrive",
            "api",
            "instance",
            "create",
            "--request",
            r#"{"Name":"my-inst"}"#,
            "--force",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
    assert_eq!(parsed["status"], "failed");
    assert_eq!(parsed["command"], "ve-adrive api instance create");
    assert_eq!(parsed["error"]["kind"], "validation_error");
    assert!(parsed["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("not implemented"));
    assert!(parsed["error"]["fix_command"]
        .as_str()
        .unwrap_or_default()
        .contains("--dry-run"));
}

#[test]
fn test_tos_high_level_describe_filter_examples_do_not_double_prefix() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--describe",
            "ve-tos",
            "ls",
            "tos://bucket/",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let examples = parsed["data"]["output_filter_examples"]
        .as_array()
        .expect("examples")
        .iter()
        .map(|value| value.as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert!(examples
        .iter()
        .any(|example| example.starts_with("ve-storage-uni-cli ve-tos ls ")));
    assert!(!examples
        .iter()
        .any(|example| example.contains("ve-storage-uni-cli ve-tos ve-storage-uni-cli ve-tos")));
}

#[test]
fn test_invalid_command_fails() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .arg("nonexistent")
        .output()
        .expect("Failed to execute");
    assert!(!output.status.success());
}

// ==========================================================================
// New command groups — verify parsing works
// ==========================================================================

#[test]
fn test_tos_turbo_command_parse() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "turbo",
            "append",
            "--bucket",
            "b",
            "--key",
            "k",
            "--body",
            "payload",
        ])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");
    let payload = parsed.get("data").unwrap_or(&parsed);
    assert_eq!(payload["dry_run"], true);
}

#[test]
fn test_tos_data_process_command_parse() {
    let output = cli_with_empty_home(
        "tos-data-process-parse",
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "data-process",
            "list-jobs",
            "--bucket",
            "b",
        ],
    );
    assert!(output.status.success());
}

#[test]
fn test_tos_mrap_command_parse() {
    // [Review Fix #4] Advanced commands now run real handlers, so parse tests must stay dry-run.
    let output = cli_with_empty_home(
        "tos-mrap-parse",
        &["--dry-run", "--output", "json", "ve-tos", "mrap", "list"],
    );
    assert!(output.status.success());
}

#[test]
fn test_tos_dataset_command_parse() {
    // [Review Fix #4] Advanced commands now run real handlers, so parse tests must stay dry-run.
    let output = cli_with_empty_home(
        "tos-dataset-parse",
        &["--dry-run", "--output", "json", "ve-tos", "dataset", "list"],
    );
    assert!(output.status.success());
}

#[test]
fn test_tos_control_command_parse() {
    // [Review Fix #4] Advanced commands now run real handlers, so parse tests must stay dry-run.
    let output = cli_with_empty_home(
        "tos-control-parse",
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "control",
            "list-batch-jobs",
        ],
    );
    assert!(output.status.success());
}

#[test]
fn test_tos_worm_command_parse() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "worm",
            "get",
            "--bucket",
            "demo-bucket",
            "--describe",
        ])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
}

#[test]
fn test_tos_mirror_command_parse() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-tos",
            "mirror",
            "get",
            "--bucket",
            "demo-bucket",
            "--describe",
        ])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
}

#[test]
fn test_tos_logging_command_parse() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-tos",
            "logging",
            "get",
            "--bucket",
            "demo-bucket",
            "--describe",
        ])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
}

#[test]
fn test_tos_redundancy_transition_command_parse() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--dry-run",
            "--output",
            "json",
            "ve-tos",
            "redundancy-transition",
            "list",
            "--bucket",
            "b",
        ])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
}

#[test]
fn test_tos_doctor_command_parse() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-tos", "doctor"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid doctor json");
    assert_eq!(parsed["status"], "success");
    assert!(parsed["data"]["summary"]["total"].as_u64().unwrap() > 0);
    assert!(parsed["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["name"] == "registry"));
    assert!(!parsed["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["name"] == "principles"));
    let checks = parsed["data"]["checks"].as_array().unwrap();
    let config = checks
        .iter()
        .find(|check| check["name"] == "config")
        .expect("config check");
    assert!(config["details"]["config_exists"].is_boolean());
    assert!(config["details"]["config_path"].is_string());
    assert!(config["details"]["has_endpoint"].is_boolean());
    assert!(config["details"]["has_region"].is_boolean());
    let auth = checks
        .iter()
        .find(|check| check["name"] == "auth")
        .expect("auth check");
    assert!(auth["details"]["has_access_key"].is_boolean());
    assert!(auth["details"]["has_secret_key"].is_boolean());
    assert!(auth["details"]["has_security_token"].is_boolean());
    let network = checks
        .iter()
        .find(|check| check["name"] == "network")
        .expect("network check");
    assert!(network["details"]["has_explicit_endpoint"].is_boolean());
    assert!(network["details"]["has_region"].is_boolean());
}

#[test]
fn test_tos_doctor_with_check_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-tos", "doctor", "--check", "auth"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid doctor json");
    assert_eq!(parsed["data"]["checks"][0]["name"], "auth");
}

#[test]
fn test_tos_doctor_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "doctor", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--check"));
    assert!(stdout.contains("--bucket"));
}

#[test]
fn test_tos_capabilities_consumes_registry() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-tos",
            "capabilities",
            "--view",
            "full",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    let capabilities = parsed["data"]["capabilities"]
        .as_array()
        .expect("capabilities");
    assert_eq!(capabilities[0]["command"], "ve-tos cp");
    assert_eq!(capabilities[0]["group"], "cp");
    assert_eq!(parsed["data"]["uri_format"], "tos://bucket/key");
    let semantics = parsed["data"]["high_level_semantics"]
        .as_object()
        .expect("high_level_semantics");
    // [Review Fix #1] Keep TOS capabilities and high-level semantics in lockstep.
    for command in capabilities
        .iter()
        .filter(|capability| capability["layer"] == "high_level")
        .filter_map(|capability| capability["group"].as_str())
    {
        assert!(
            semantics
                .get(command)
                .and_then(|value| value.as_array())
                .is_some_and(|entries| !entries.is_empty()),
            "missing high-level semantics for tos {command}"
        );
    }
}

#[test]
fn test_tos_api_describe_consumes_registry() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-tos",
            "api",
            "config",
            "show",
            "--describe",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["command"], "ve-tos config show");
    assert_eq!(parsed["data"]["layer"], "meta");
}

#[test]
fn test_tos_api_uses_command_tree_for_low_level_actions() {
    // [Review Fix #s1] `object upload` is now curated, so we exercise the
    // command-tree fallback against a still-derived leaf (`object head`).
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-tos", "api", "object", "head"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["mode"], "command_metadata");
    assert_eq!(parsed["data"]["command"], "ve-tos object head");
    assert!(parsed["data"]["capability_row"]["risk_level"].is_string());
    assert_eq!(parsed["data"]["capability_row"]["endpoint_rule"], "data");
}

#[test]
fn test_tos_api_curated_low_level_returns_capability_metadata() {
    // [Review Fix #s1] Newly curated `ve-tos object upload` must surface as
    // capability_metadata with the registry-defined risk/endpoint/method.
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-tos", "api", "object", "upload"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["mode"], "capability_metadata");
    assert_eq!(parsed["data"]["command"], "ve-tos object upload");
    assert_eq!(parsed["data"]["capability_row"]["risk_level"], "medium");
    assert_eq!(
        parsed["data"]["capability_row"]["endpoint_rule"],
        "DataPlane"
    );
    assert_eq!(parsed["data"]["capability_row"]["method"], "PUT");
}

#[test]
fn test_tos_api_storageclass_alias_returns_registry_metadata() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-tos", "api", "storageclass", "set"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["data"]["command"], "ve-tos storageclass set");
    assert_eq!(parsed["data"]["capability_row"]["group"], "storageclass");
    assert_eq!(parsed["data"]["capability_row"]["risk_level"], "medium");
}

#[test]
fn test_tos_api_raw_request_dry_run_plan() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--dry-run",
            "ve-tos",
            "api",
            "unknown",
            "action",
            "--request",
            r#"{"method":"GET","endpoint_kind":"data","path":"/?not_allowed=false"}"#,
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        !output.status.success(),
        "query strings must be rejected from path to keep request signing deterministic"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--dry-run",
            "ve-tos",
            "api",
            "unknown",
            "action",
            "--request",
            r#"{"method":"GET","endpoint_kind":"data","path":"/","query":{"list-type":2}}"#,
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["mode"], "unregistered_raw_passthrough_plan");
    assert_eq!(parsed["data"]["request"]["query"]["list-type"], 2);
}

#[test]
fn test_tos_api_raw_request_dry_run_redacts_body_secrets() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--dry-run",
            "ve-tos",
            "api",
            "unknown",
            "action",
            "--request",
            r#"{"method":"GET","endpoint_kind":"data","path":"/","body":{"secret":"SUPERSECRET","nested":{"security_token":"TOKENVALUE"},"items":[{"access_key_id":"AKID"}]}}"#,
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("SUPERSECRET"), "stdout={stdout}");
    assert!(!stdout.contains("TOKENVALUE"), "stdout={stdout}");
    assert!(!stdout.contains("AKID"), "stdout={stdout}");
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(
        parsed["data"]["request"]["body"]["secret"],
        "***REDACTED***"
    );
    assert_eq!(
        parsed["data"]["request"]["body"]["nested"]["security_token"],
        "***REDACTED***"
    );
    assert_eq!(
        parsed["data"]["request"]["body"]["items"][0]["access_key_id"],
        "***REDACTED***"
    );
}

#[test]
fn test_tos_api_raw_request_requires_force_for_mutation() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--dry-run",
            "ve-tos",
            "api",
            "unknown",
            "action",
            "--request",
            r#"{"method":"DELETE","endpoint_kind":"data","path":"/bucket"}"#,
        ])
        .output()
        .expect("Failed to execute");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("requires --force"), "stderr={stderr}");
}

#[test]
fn test_tos_capabilities_group_utilities_returns_utility_capabilities() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-tos",
            "capabilities",
            "--group",
            "utilities",
            "--view",
            "full",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert!(parsed["data"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .all(|group| group["group"] == "utilities"
            && group["category"] == "utilities"
            && group["implemented"] == true));
    assert!(parsed["data"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability["command"] == "ve-tos skill export"));
    assert!(parsed["data"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| command["command"] == "ve-tos config"));
}

#[test]
fn test_tos_capabilities_unknown_group_is_deterministic_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-tos",
            "capabilities",
            "--group",
            "not-a-real-group",
            "--view",
            "full",
        ])
        .output()
        .expect("Failed to execute");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("failed envelope");
    assert_eq!(parsed["status"], "failed");
    assert_eq!(parsed["error"]["kind"], "validation_error");
    assert!(parsed["error"]["message"]
        .as_str()
        .unwrap()
        .contains("unknown capabilities group"));
}

#[test]
fn test_tos_storageclass_alias_parses() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--dry-run",
            "ve-tos",
            "storageclass",
            "set",
            "--bucket",
            "demo-bucket",
            "--storage-class",
            "STANDARD",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["data"]["action"], "ve-tos storageclass set");
    assert_eq!(parsed["data"]["dry_run"], true);
}

#[test]
fn test_tos_capabilities_tree_view_contains_low_level_actions() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-tos",
            "capabilities",
            "--view",
            "tree",
            "--search",
            "upload",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert!(parsed["data"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| command["command"] == "ve-tos object upload"));
}

#[test]
fn test_tos_skill_list_and_export_consume_registry() {
    let list_output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-tos", "skill", "list"])
        .output()
        .expect("Failed to execute");
    assert!(
        list_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&list_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["skills"][0]["name"], "ve_tos_cp");
    assert!(parsed["data"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|skill| skill["command"] == "ve-tos object upload"));
    assert!(parsed["data"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .any(|skill| skill["command"] == "ve-tos api"));

    let dir = std::env::temp_dir().join(format!(
        "ve-storage-uni-cli-skill-export-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    let export_output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-tos",
            "skill",
            "export",
            "--name",
            "cp",
            "--dir",
            dir.to_str().expect("temp dir"),
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        export_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&export_output.stderr)
    );
    assert!(dir.join("SKILL.md").exists());
    let skill_path = dir.join("tos-transfer").join("ve_tos_cp").join("SKILL.md");
    assert!(skill_path.exists());
    let exported = fs::read_to_string(skill_path).expect("exported skill");
    assert!(exported.starts_with("# ve_tos_cp"), "exported={exported}");
    assert!(exported.contains("```json"), "exported={exported}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_tos_completion_consumes_registry() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-tos", "completion", "zsh"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["data"]["shell"], "zsh");
    assert!(parsed["data"]["script"]
        .as_str()
        .unwrap()
        .contains("capabilities"));
}

#[test]
fn test_byted_tos_completion_lists_real_commands_and_unified_entry() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "tos", "completion", "bash"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid completion json");
    let script = parsed["data"]["script"].as_str().expect("script");
    assert!(script.contains("cp"), "script={script}");
    assert!(script.contains("config"), "script={script}");
    assert!(script.contains("completion"), "script={script}");
    assert!(script.contains("tos"), "script={script}");
    assert!(script.contains("tos-cli"), "script={script}");
    assert!(script.contains("ve-storage-uni-cli"), "script={script}");
    assert!(!script.contains("tos-transfer"), "script={script}");
}

#[test]
fn test_tos_completion_help_documents_installation() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["ve-tos", "completion", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Install examples"), "stdout={stdout}");
    assert!(stdout.contains("data.script"), "stdout={stdout}");
    assert!(stdout.contains("jq -r"), "stdout={stdout}");
}

#[test]
fn test_byted_tos_completion_help_uses_tos_install_paths() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "completion", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("~/.tos-completion.bash"), "stdout={stdout}");
    assert!(stdout.contains(".zfunc/_tos"), "stdout={stdout}");
    assert!(stdout.contains("completions/tos.fish"), "stdout={stdout}");
    assert!(
        !stdout.contains("~/.ve-tos-completion.bash"),
        "stdout={stdout}"
    );
    assert!(!stdout.contains(".zfunc/_ve-tos"), "stdout={stdout}");
}

#[test]
fn test_tos_skill_describe_explains_export_and_serve_relationship() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--describe",
            "ve-tos",
            "skill",
            "export",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["command"], "ve-tos skill export");
    assert!(parsed["data"]["scenario_routing"]["format"]
        .as_str()
        .unwrap()
        .contains("Markdown SKILL.md"));
    assert!(parsed["data"]["scenario_routing"]["serve_relationship"]
        .as_str()
        .unwrap()
        .contains("does not read"));
}

#[test]
fn test_tos_skill_schema_documents_execute_and_bucket_create_targets() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["--output", "json", "ve-tos", "skill", "list"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    let bucket_create = parsed["data"]["skills"]
        .as_array()
        .expect("skills")
        .iter()
        .find(|skill| skill["name"] == "ve_tos_bucket_create")
        .expect("ve_tos_bucket_create skill");
    let properties = bucket_create["input_schema"]["properties"]
        .as_object()
        .expect("input properties");
    assert_eq!(bucket_create["schema_version"], "tos-skill-v1");
    assert_eq!(bucket_create["usage"]["serve_reads_exported_files"], false);
    assert!(properties.contains_key("execute"));
    assert!(properties.contains_key("uri"));
    assert!(properties.contains_key("bucket_name"));
    assert!(properties["bucket_name"]["description"]
        .as_str()
        .unwrap()
        .contains("--bucket"));
    let examples = bucket_create["examples"].as_array().expect("examples");
    assert!(examples.iter().any(|example| example
        .as_str()
        .unwrap_or_default()
        .starts_with("ve-storage-uni-cli ve-tos bucket create ")));
}

#[test]
fn test_tos_serve_sse_dry_run_reports_plan() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--dry-run",
            "ve-tos",
            "serve",
            "--mcp",
            "--transport",
            "sse",
            "--port",
            "18080",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).expect("valid json");
    assert_eq!(parsed["status"], "success");
    assert_eq!(parsed["command"], "ve-tos serve");
    assert_eq!(parsed["data"]["mode"], "mcp");
    assert_eq!(parsed["data"]["transport"], "sse");
    assert_eq!(parsed["data"]["port"], 18080);
    assert_eq!(parsed["data"]["status"], "planned_not_started");
}

#[test]
fn test_tos_serve_describe_documents_mcp_transport_details() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "--describe",
            "ve-tos",
            "serve",
            "--mcp",
            "--transport",
            "sse",
            "--port",
            "19090",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["data"]["protocol"], "MCP standard protocol via rmcp");
    assert_eq!(parsed["data"]["tcp_listener"], true);
    assert_eq!(parsed["data"]["bind"], "127.0.0.1:19090");
    assert!(parsed["data"]["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .any(|endpoint| endpoint == "/sse"));
    assert!(parsed["data"]["tool_source"]
        .as_str()
        .unwrap()
        .contains("exported Markdown skill files are not read"));
}

#[test]
fn test_tos_parse_error_uses_failed_envelope() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args([
            "--output",
            "json",
            "ve-tos",
            "object",
            "upload",
            "--unknown-flag",
        ])
        .output()
        .expect("Failed to execute");
    assert!(
        !output.status.success(),
        "stdout={}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(output.status.code(), Some(6));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
    assert_eq!(parsed["success"], false);
    assert_eq!(parsed["status"], "failed");
    assert_eq!(parsed["command"], "ve-tos object upload");
    assert!(parsed["status_code"].is_null());
    assert_eq!(parsed["ec"], "ValidationError");
    assert!(parsed["request_id"].is_string());
    assert_eq!(parsed["error"]["kind"], "validation_error");
    assert_eq!(parsed["error"]["exit_code"], 6);
    assert!(parsed["error"]["fix_command"]
        .as_str()
        .unwrap()
        .contains("--help"));
}

#[test]
fn test_byted_tos_serve_help_uses_tos_tool_names() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(["tos", "serve", "--help"])
        .output()
        .expect("Failed to execute");
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("tos_ls"), "stdout={stdout}");
    assert!(stdout.contains("tos serve --mcp"), "stdout={stdout}");
    assert!(!stdout.contains("ve_tos_ls"), "stdout={stdout}");
    assert!(!stdout.contains("ve_tos_bucket_create"), "stdout={stdout}");
}

#[test]
fn test_tos_serve_mcp_stdio_runtime_lists_tools() {
    let mut session = StdioMcpSession::spawn();
    let initialized = session.initialize();
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        "ve-storage-uni-cli"
    );
    let tools = session.request(
        br#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
"#,
    );
    assert!(tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["name"] == "ve_tos_api"));
}

#[test]
fn test_tos_serve_mcp_stdio_runtime_executes_typed_command() {
    let mut session = StdioMcpSession::spawn();
    session.initialize();
    let response = session.request(
        br#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ve_tos_capabilities","arguments":{"view":"groups","execute":true}}}
"#,
    );
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    let execution: serde_json::Value = serde_json::from_str(text).expect("execution json");
    assert_eq!(execution["command"], "ve-tos capabilities");
    assert_eq!(execution["exit_code"], 0);
    let command_stdout: serde_json::Value =
        serde_json::from_str(execution["stdout"].as_str().unwrap()).expect("typed command stdout");
    assert_eq!(command_stdout["status"], "success");
}

struct StdioMcpSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

impl StdioMcpSession {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .args(["ve-tos", "serve", "--mcp", "--transport", "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn mcp stdio");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn initialize(&mut self) -> serde_json::Value {
        let response = self.request(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"cli-basic-test","version":"0.0.0"}}}
"#,
        );
        // [Review Fix #22] rmcp follows the MCP lifecycle and only accepts tool
        // requests after the client sends the initialized notification.
        self.stdin
            .write_all(
                br#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
"#,
            )
            .expect("write initialized notification");
        self.stdin.flush().expect("flush initialized notification");
        response
    }

    fn request(&mut self, request: &[u8]) -> serde_json::Value {
        self.stdin.write_all(request).expect("write mcp request");
        self.stdin.flush().expect("flush mcp request");
        let mut line = String::new();
        self.stdout.read_line(&mut line).expect("read mcp response");
        assert!(!line.is_empty(), "mcp response must not be empty");
        serde_json::from_str(line.trim()).expect("mcp response json")
    }
}

impl Drop for StdioMcpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
