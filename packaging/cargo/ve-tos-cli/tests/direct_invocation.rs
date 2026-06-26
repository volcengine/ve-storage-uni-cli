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
fn dedicated_tos_cli_direct_invocation() {
    let help = Command::new(env!("CARGO_BIN_EXE_ve-tos-cli"))
        .arg("--help")
        .output()
        .expect("Failed to execute ve-tos-cli");
    let stdout = String::from_utf8_lossy(&help.stdout);
    assert!(stdout.contains("TOS Object Storage CLI"));
    assert!(stdout.contains("ve-tos-cli <command>"));
    // [Review Fix #1] Direct-entry control-plane help coverage lives in the
    // packaging crate after removing root-package dedicated binaries.
    assert!(stdout.contains("--control-endpoint"));
    assert!(stdout.contains("--account-id"));

    let describe = Command::new(env!("CARGO_BIN_EXE_ve-tos-cli"))
        .args(["cp", "--describe", "--output", "json"])
        .output()
        .expect("Failed to execute ve-tos-cli describe");
    assert!(describe.status.success());
    let stdout = String::from_utf8_lossy(&describe.stdout);
    assert!(stdout.contains("\"status\": \"success\""));
    assert!(stdout.contains("\"command\": \"ve-tos cp\""));
}

#[test]
fn dedicated_tos_cli_version_output() {
    let output = Command::new(env!("CARGO_BIN_EXE_ve-tos-cli"))
        .arg("--version")
        .output()
        .expect("Failed to execute ve-tos-cli --version");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("ve-tos-cli"));
}
