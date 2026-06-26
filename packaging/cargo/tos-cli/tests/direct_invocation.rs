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
fn direct_help_exposes_tos_cli_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_tos-cli"))
        .arg("--help")
        .output()
        .expect("run tos-cli");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("High-Level Commands"));
    assert!(stdout.contains("Capabilities / Utilities"));
    assert!(!stdout.contains("Low-Level API"));
    assert!(!stdout.contains("  mb            "));
    assert!(!stdout.contains("  rb            "));
    assert!(!stdout.contains("List buckets"));
    // [Review Fix #1] Keep direct-entry coverage here now that the root package
    // no longer builds a `tos-cli` binary for integration tests.
    assert!(!stdout.contains("--control-endpoint"));
    assert!(!stdout.contains("--account-id"));
}

#[test]
fn direct_tos_cli_rejects_control_plane_global_flags() {
    for (flag, value) in [
        ("--control-endpoint", "https://tos-control.example.com"),
        ("--account-id", "2100000001"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_tos-cli"))
            .args(["--output", "json", flag, value, "doctor"])
            .output()
            .expect("run tos-cli");
        assert!(!output.status.success(), "flag={flag}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(flag), "stderr={stderr}");
        assert!(stderr.contains("does not support"), "stderr={stderr}");
    }
}
