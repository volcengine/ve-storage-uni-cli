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

use std::collections::{BTreeMap, BTreeSet};
use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(args)
        .output()
        .expect("failed to execute ve-storage-uni-cli")
}

fn stdout_json(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|err| {
        panic!("stdout is not json: {err}; stdout={stdout}");
    })
}

fn data(output: &Output) -> serde_json::Value {
    let json = stdout_json(output);
    assert_eq!(
        json["status"], "success",
        "expected success envelope: {json}"
    );
    json["data"].clone()
}

fn functional_roots(capabilities: &serde_json::Value) -> BTreeSet<String> {
    capabilities["groups"]
        .as_array()
        .expect("groups")
        .iter()
        .filter(|group| group["category"] != "utilities")
        .map(|group| group["name"].as_str().expect("group name").to_string())
        .collect()
}

fn collect_leaf_commands(entry: &serde_json::Value, out: &mut Vec<String>) {
    let subcommands = entry["subcommands"].as_array().expect("subcommands");
    if subcommands.is_empty() {
        out.push(entry["command"].as_str().expect("command").to_string());
        return;
    }
    for child in subcommands {
        collect_leaf_commands(child, out);
    }
}

#[test]
fn all_functional_groups_have_discoverable_capability_rows() {
    let output = cli(&[
        "ve-tos",
        "capabilities",
        "--view",
        "full",
        "--output",
        "json",
    ]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let caps = data(&output);
    let roots = functional_roots(&caps);

    let groups = caps["groups"].as_array().expect("groups");
    for group in groups
        .iter()
        .filter(|group| group["category"] != "utilities")
    {
        assert!(
            group["command_count"].as_u64().unwrap_or_default() > 0,
            "functional group {} has no capability rows",
            group["command"]
        );
    }

    let capability_commands = caps["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .map(|capability| {
            (
                capability["command"]
                    .as_str()
                    .expect("cap command")
                    .to_string(),
                capability,
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut leaves = Vec::new();
    for command in caps["commands"].as_array().expect("commands") {
        collect_leaf_commands(command, &mut leaves);
    }

    for command in leaves {
        let root = command
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .to_string();
        if !roots.contains(&root) {
            continue;
        }
        let capability = capability_commands
            .get(&command)
            .unwrap_or_else(|| panic!("missing capability row for functional command {command}"));
        assert!(
            matches!(capability["risk_level"].as_str(), Some(risk) if risk != "unknown"),
            "functional command {command} has unknown risk"
        );
        assert!(
            capability["supports_describe"].as_bool().unwrap_or(false),
            "functional command {command} must support describe"
        );
        assert!(
            capability["supports_dry_run"].as_bool().unwrap_or(false),
            "functional command {command} must support dry-run"
        );
    }
}

#[test]
fn all_functional_skills_have_deterministic_risk() {
    let output = cli(&["ve-tos", "skill", "list", "--output", "json"]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let skills = data(&output);
    let utilities = BTreeSet::from([
        "api",
        "capabilities",
        "completion",
        "config",
        "doctor",
        "serve",
        "skill",
    ]);
    for skill in skills["skills"].as_array().expect("skills") {
        let command = skill["command"].as_str().expect("skill command");
        let root = command.split_whitespace().nth(1).unwrap_or_default();
        if utilities.contains(root) {
            continue;
        }
        assert_ne!(
            skill["risk_level"], "unknown",
            "functional skill {command} must not have unknown risk"
        );
        assert!(
            skill["input_schema"].is_object(),
            "functional skill {command} must expose input schema"
        );
    }
}

#[test]
fn functional_leaf_describe_uses_success_envelope() {
    let output = cli(&[
        "ve-tos",
        "bucket",
        "create",
        "--bucket",
        "demo-bucket",
        "--describe",
        "--output",
        "json",
    ]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    assert_eq!(json["status"], "success");
    assert_eq!(json["data"]["command"], "ve-tos bucket create");
    assert_eq!(json["data"]["api"], "CreateBucket");
}

#[test]
fn low_level_layer_filter_keeps_functional_capabilities() {
    let output = cli(&[
        "ve-tos",
        "capabilities",
        "--layer",
        "low_level",
        "--view",
        "full",
        "--output",
        "json",
    ]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let caps = data(&output);
    assert!(
        caps["capabilities"].as_array().expect("capabilities").len() > 200,
        "low_level filter should retain derived functional capabilities"
    );
    for group in caps["groups"].as_array().expect("groups") {
        assert!(
            group["command_count"].as_u64().unwrap_or_default() > 0,
            "low-level group {} has no capability rows",
            group["command"]
        );
    }
}

#[test]
fn share_parse_errors_never_echo_query_passwords() {
    let output = cli(&[
        "ve-adrive",
        "share",
        "files",
        "https://x.test/share/s",
        "https://x.test/share/s?pwd=never-echo",
        "--output",
        "json",
    ]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("never-echo"), "{stderr}");
    let error: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(error["error"]["code"], "ValidationError");
    assert!(error["error"]["suggested_action"]
        .as_str()
        .unwrap()
        .contains("--help"));
}

#[test]
fn share_unknown_action_and_flag_errors_have_canonical_secret_free_commands() {
    for args in [
        vec![
            "ve-adrive",
            "share",
            "https://x.test/share/s?pwd=fixture-leak",
            "--output",
            "json",
        ],
        vec![
            "ve-adrive",
            "share",
            "files",
            "https://x.test/share/s",
            "--unknown=https://x.test/share/s?pwd=fixture-leak",
            "--output",
            "json",
        ],
    ] {
        let output = cli(&args);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("fixture-leak"), "{stderr}");
        let error: serde_json::Value = serde_json::from_str(&stderr).unwrap();
        assert!(matches!(
            error["command"].as_str(),
            Some("ve-adrive share" | "ve-adrive share files")
        ));
        assert_eq!(error["error"]["code"], "ValidationError");
    }
}

#[cfg(unix)]
#[test]
fn share_sigint_restores_real_terminal_echo_during_and_after_password() {
    let output = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/share_pty_regression.py"
        ))
        .arg(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn share_describe_recovers_incomplete_positionals_without_io() {
    for action in [
        "create", "get", "list", "update", "cancel", "inspect", "files", "view", "download",
    ] {
        let output = cli(&[
            "ve-adrive",
            "share",
            action,
            "--describe",
            "--output",
            "json",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            data(&output)["command"],
            format!("ve-adrive share {action}")
        );
    }
}

#[test]
fn share_group_describe_is_structured_and_unauthenticated() {
    let output = cli(&["ve-adrive", "share", "--describe", "--output", "json"]);
    assert!(output.status.success());
    assert_eq!(data(&output)["kind"], "command_group");
    assert_eq!(data(&output)["subcommands"].as_array().unwrap().len(), 9);
}

#[test]
fn adrive_share_help_lists_all_actions_in_both_languages() {
    // [Review Fix #Baseline2] Root help lists namespaces by layer; verify
    // action discovery in the share namespace's own help instead.
    for language in ["en", "zh"] {
        let output = cli(&["ve-adrive", "share", "--help", "--language", language]);
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        for action in [
            "create", "get", "list", "update", "cancel", "inspect", "files", "view", "download",
        ] {
            assert!(
                stdout
                    .lines()
                    .any(|line| line.split_whitespace().next() == Some(action)),
                "language={language}, missing {action}"
            );
        }
    }
}

#[test]
fn bare_share_group_help_uses_requested_language_and_unified_prefix() {
    let output = cli(&["ve-adrive", "share", "--language", "zh"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("取消分享链接"));
    assert!(stdout.contains("ve-storage-uni-cli ve-adrive share"));
    assert!(!stdout.contains("Cancel a share link"));
}
