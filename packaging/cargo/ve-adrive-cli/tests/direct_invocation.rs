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

// [Review Fix #Rebase2] Keep master's ADrive auth-mode boundary alongside share redaction coverage.
#[test]
fn direct_entry_rejects_bytetos_zti_mode() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-adrive-cli"))
        .args([
            "--auth-mode",
            "zti",
            "doctor",
            "--check",
            "auth",
            "--output",
            "json",
        ])
        .output()
        .expect("run ve-adrive-cli");
    assert!(!output.status.success());
    let response: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    let message = response["error"]["message"].as_str().unwrap();
    assert!(message.contains("invalid value 'zti'"));
    assert!(message.contains("aksk, oauth, unified"));
}

#[test]
fn share_parse_errors_keep_canonical_commands_and_hide_url_passwords() {
    for args in [
        vec![
            "share",
            "https://x.test/share/s?pwd=fixture-leak",
            "--output",
            "json",
        ],
        vec![
            "share",
            "files",
            "https://x.test/share/s",
            "--unknown=https://x.test/share/s?pwd=fixture-leak",
            "--output",
            "json",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-adrive-cli"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("fixture-leak"), "{stderr}");
        assert!(stderr.contains("ValidationError"));
    }
}

#[test]
fn dedicated_adrive_cli_direct_invocation() {
    let help = Command::new(env!("CARGO_BIN_EXE_ve-adrive-cli"))
        .arg("--help")
        .output()
        .expect("Failed to execute ve-adrive-cli");
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("ADrive CLI"));
    assert!(stdout.contains("ve-adrive-cli <command>"));
    // [Review Fix #1] Direct-entry control-plane help coverage lives in the
    // packaging crate after removing root-package dedicated binaries.
    assert!(!stdout.contains("--control-endpoint"));
    assert!(!stdout.contains("--account-id"));

    // [Review Fix #ShareTestIsolation2] Standalone dry-run must not read the developer machine's config.
    let config_path =
        std::env::temp_dir().join(format!("adrive-direct-test-{}.toml", std::process::id()));
    std::fs::write(&config_path, "").unwrap();
    let dry_run = Command::new(env!("CARGO_BIN_EXE_ve-adrive-cli"))
        .arg("--config-path")
        .arg(&config_path)
        .args(["ls", "--dry-run", "--output", "json"])
        .output()
        .expect("Failed to execute ve-adrive-cli dry-run");
    assert!(dry_run.status.success());
    let stdout = String::from_utf8_lossy(&dry_run.stdout);
    assert!(stdout.contains("\"status\": \"success\""));
    assert!(stdout.contains("\"command\": \"ve-adrive ls\""));
    assert!(stdout.contains("\"dry_run\": true"));
    std::fs::remove_file(config_path).unwrap();
}

#[test]
fn dedicated_adrive_cli_version_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-adrive-cli"))
        .arg("--version")
        .output()
        .expect("Failed to execute ve-adrive-cli --version");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        format!("ve-adrive-cli {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn dedicated_adrive_cli_rejects_control_plane_global_flags() {
    for (flag, value) in [
        ("--control-endpoint", "https://tos-control.example.com"),
        ("--account-id", "2100000001"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-adrive-cli"))
            .args(["--output", "json", flag, value, "doctor"])
            .output()
            .expect("Failed to execute ve-adrive-cli");
        assert!(!output.status.success(), "flag={flag}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(flag), "stderr={stderr}");
        assert!(stderr.contains("does not support"), "stderr={stderr}");
    }
}

#[test]
fn dedicated_share_leaf_describe_and_help_are_available() {
    for action in [
        "create", "get", "list", "update", "cancel", "inspect", "files", "download",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ve-adrive-cli"))
            .args(["share", action, "--describe", "--output", "json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains(&format!("ve-adrive share {action}")));
    }
}

#[test]
fn dedicated_bare_share_help_is_chinese_and_contextual() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-adrive-cli"))
        .args(["share", "--language", "zh"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("取消分享链接"));
    assert!(stdout.contains("ve-adrive-cli share"));
    assert!(!stdout.contains("Cancel a share link"));
}
