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
