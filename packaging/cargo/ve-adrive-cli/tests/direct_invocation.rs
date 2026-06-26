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

    let dry_run = Command::new(env!("CARGO_BIN_EXE_ve-adrive-cli"))
        .args(["ls", "--dry-run", "--output", "json"])
        .output()
        .expect("Failed to execute ve-adrive-cli dry-run");
    assert!(dry_run.status.success());
    let stdout = String::from_utf8_lossy(&dry_run.stdout);
    assert!(stdout.contains("\"status\": \"success\""));
    assert!(stdout.contains("\"command\": \"ve-adrive ls\""));
    assert!(stdout.contains("\"dry_run\": true"));
}

#[test]
fn dedicated_adrive_cli_version_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-adrive-cli"))
        .arg("--version")
        .output()
        .expect("Failed to execute ve-adrive-cli --version");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("ve-adrive-cli"));
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
