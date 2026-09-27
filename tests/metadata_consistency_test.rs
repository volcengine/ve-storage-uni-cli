//! User-visible help and describe contracts for shared command surfaces.

use std::process::Command;

fn output(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(args)
        .output()
        .expect("run CLI")
}

#[test]
fn byte_tos_config_and_doctor_help_only_describe_supported_surface() {
    for command in [
        vec!["tos", "config", "--help"],
        vec!["tos", "config", "init", "--help"],
        vec!["tos", "config", "show", "--help"],
        vec!["tos", "config", "set", "--help"],
        vec!["tos", "doctor", "--help"],
    ] {
        let result = output(&command);
        assert!(result.status.success(), "{command:?}");
        let help = String::from_utf8_lossy(&result.stdout);
        assert!(!help.contains("ve-tos"), "{command:?}: {help}");
        assert!(!help.contains("control_endpoint"), "{command:?}: {help}");
    }
}

#[test]
fn byte_tos_help_examples_follow_the_unified_invocation() {
    for command in [
        vec!["tos", "config", "set", "--help"],
        vec!["tos", "doctor", "--help"],
    ] {
        let result = output(&command);
        assert!(result.status.success(), "{command:?}");
        let help = String::from_utf8_lossy(&result.stdout);
        assert!(
            help.contains("ve-storage-uni-cli tos "),
            "{command:?}: {help}"
        );
        assert!(!help.contains("  tos-cli "), "{command:?}: {help}");
    }
}

#[test]
fn byte_tos_config_init_chinese_help_explains_psm_network_mode() {
    let result = output(&["tos", "config", "init", "--help", "--help-language", "zh"]);
    assert!(result.status.success());
    let help = String::from_utf8_lossy(&result.stdout);
    assert!(help.contains("配置 PSM 服务发现"), "{help}");
    assert!(!help.contains("PSM 服务发现和 region"), "{help}");
    assert!(!help.contains("Configure an endpoint"), "{help}");
}

#[test]
fn byte_tos_skill_does_not_require_region_for_psm() {
    let skill = include_str!("../skills/tos-cli/SKILL.md");
    assert!(skill.contains("PSM mode, configure PSM without an endpoint; region is optional"));
    assert!(!skill.contains("configure both region and PSM"));
}

#[test]
fn adrive_checkpoint_describe_matches_valueless_help_flag() {
    let help = output(&["ve-adrive", "cp", "--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--checkpoint"));

    let description = output(&["ve-adrive", "cp", "--describe", "--output", "json"]);
    assert!(description.status.success());
    let document: serde_json::Value = serde_json::from_slice(&description.stdout).unwrap();
    let checkpoint = document["data"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|parameter| parameter["name"] == "checkpoint")
        .expect("checkpoint parameter");
    assert_eq!(checkpoint["schema"]["type"], "boolean");
}

#[test]
fn byte_tos_doctor_describe_is_offline_command_metadata() {
    let result = output(&["tos", "doctor", "--describe", "--output", "json"]);
    assert!(result.status.success());
    let document: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(document["data"]["command"], "tos doctor");
    assert!(document["data"]["checks"].is_null());
    let parameters = document["data"]["parameters"].as_array().unwrap();
    let check = parameters
        .iter()
        .find(|parameter| parameter["name"] == "check")
        .expect("doctor check parameter");
    assert_eq!(check["schema"]["type"], "string");
    assert_eq!(
        check["schema"]["enum"],
        serde_json::json!([
            "auth",
            "config",
            "registry",
            "network",
            "endpoint",
            "mcp",
            "completion"
        ])
    );
    assert!(parameters
        .iter()
        .any(|parameter| parameter["name"] == "live-network"));
    assert!(!parameters
        .iter()
        .any(|parameter| parameter["name"] == "bucket"));
}

#[test]
fn byte_tos_config_set_describe_recovers_required_operands() {
    let result = output(&["tos", "config", "set", "--describe", "--output", "json"]);
    assert!(result.status.success());
    let document: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(document["data"]["command"], "tos config set");
    let parameters = document["data"]["parameters"].as_array().unwrap();
    for name in ["key", "value"] {
        assert!(
            parameters
                .iter()
                .any(|parameter| { parameter["name"] == name && parameter["required"] == true }),
            "missing required {name}: {parameters:?}"
        );
    }
}

#[test]
fn ve_tos_config_set_describe_keeps_its_existing_parameter_contract() {
    let result = output(&[
        "ve-tos",
        "config",
        "set",
        "region",
        "cn-beijing",
        "--describe",
        "--output",
        "json",
    ]);
    assert!(result.status.success());
    let document: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(document["data"]["command"], "ve-tos config set");
    assert!(document["data"]["parameters"].is_null());
}

#[test]
fn byte_tos_config_set_recovered_describe_honors_chinese_language() {
    let result = output(&[
        "tos",
        "config",
        "set",
        "--describe",
        "--language",
        "zh",
        "--output",
        "json",
    ]);
    assert!(result.status.success());
    let document: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert!(document["data"]["description"]
        .as_str()
        .unwrap()
        .starts_with("设置配置值。"));
    let key = document["data"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|parameter| parameter["name"] == "key")
        .unwrap();
    assert_eq!(key["description"], "要设置的配置键");
}

#[test]
fn byte_tos_doctor_skill_schema_matches_help_and_describe() {
    let result = output(&["tos", "skill", "list", "--output", "json"]);
    assert!(result.status.success());
    let document: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let doctor = document["data"]["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|skill| skill["name"] == "tos_doctor")
        .expect("tos doctor skill");
    let properties = &doctor["input_schema"]["properties"];
    assert_eq!(properties["live-network"]["type"], "boolean");
    assert_eq!(properties["network-timeout-ms"]["type"], "integer");
    assert_eq!(
        properties["check"]["enum"],
        serde_json::json!([
            "auth",
            "config",
            "registry",
            "network",
            "endpoint",
            "mcp",
            "completion"
        ])
    );
}

#[test]
fn byte_tos_skill_flag_types_match_clap_help() {
    let result = output(&["tos", "skill", "list", "--output", "json"]);
    assert!(result.status.success());
    let document: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let mut mismatches = Vec::new();
    for skill in document["data"]["skills"].as_array().unwrap() {
        let command = skill["command"].as_str().unwrap();
        let mut argv = command.split_whitespace().collect::<Vec<_>>();
        argv.push("--help");
        let help = output(&argv);
        assert!(help.status.success(), "{command}");
        let text = String::from_utf8_lossy(&help.stdout);
        for (name, schema) in skill["input_schema"]["properties"].as_object().unwrap() {
            let flag = format!("--{name}");
            let option = text.lines().find(|line| {
                line.trim_start()
                    .split_whitespace()
                    .any(|word| word.trim_end_matches(',') == flag)
            });
            let Some(option) = option else { continue };
            let is_value_flag = option.contains(&format!("{flag} <"));
            if (schema["type"] == "boolean") == is_value_flag {
                mismatches.push(format!("{command} {flag}: {}", schema["type"]));
            }
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
