//! Offline regressions for command-specific global help and side-effect-free previews.
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

const STORAGE_OPTIONS: &[&str] = &[
    "--profile",
    "--config-path",
    "--credentials-path",
    "--auth-mode",
    "--region",
    "--endpoint",
    "--psm",
    "--idc",
    "--cluster",
    "--addr-family",
    "--control-endpoint",
    "--account-id",
];

fn run(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn help_options(tool: &str, path: &[&str], language: &str, help_flag: &str) -> BTreeSet<String> {
    let language_flag = if path.first() == Some(&"skill") {
        "--help-language"
    } else {
        "--language"
    };
    let mut arguments = vec![tool];
    arguments.extend(path);
    arguments.extend([help_flag, language_flag, language]);
    let output = run(&arguments);
    assert!(
        output.status.success(),
        "{arguments:?}: {:?}",
        output.stderr
    );
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|line| line.trim_start().starts_with('-'))
        .filter_map(|line| {
            line.split_whitespace()
                .take(2)
                .find(|word| word.starts_with("--"))
        })
        .map(|word| word.trim_end_matches(',').to_string())
        .collect()
}

fn verify_local_help(tool: &str, path: &[&str]) {
    for language in ["en", "zh"] {
        for help_flag in ["-h", "--help"] {
            let options = help_options(tool, path, language, help_flag);
            for flag in STORAGE_OPTIONS.iter().copied().chain([
                "--yes",
                "--confirm",
                "--quiet",
                "--no-color",
            ]) {
                assert!(
                    !options.contains(flag),
                    "{tool} {path:?}: unexpected {flag}"
                );
            }
            for flag in ["--output", "--query", "--help"] {
                assert!(options.contains(flag), "{tool} {path:?}: missing {flag}");
            }
            assert_eq!(options.contains("--verbose"), tool != "ve-adrive");
            assert_eq!(
                options.contains("--dry-run"),
                path == ["skill"] || path == ["skill", "export"]
            );
        }
    }
}

#[test]
fn local_metadata_help_does_not_advertise_storage_or_confirmation_options() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for path in [
            vec!["capabilities"],
            vec!["completion"],
            vec!["skill"],
            vec!["skill", "list"],
            vec!["skill", "export"],
        ] {
            verify_local_help(tool, &path);
        }
    }
}

#[test]
fn configuration_help_preserves_stores_without_advertising_connection_overrides() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for action in ["init", "show", "set"] {
            let options = help_options(tool, &["config", action], "en", "--help");
            for flag in [
                "--profile",
                "--config-path",
                "--dry-run",
                "--describe",
                "--output",
                "--query",
            ] {
                assert!(
                    options.contains(flag),
                    "{tool} config {action}: missing {flag}"
                );
            }
            for flag in [
                "--region",
                "--endpoint",
                "--yes",
                "--confirm",
                "--quiet",
                "--no-color",
            ] {
                assert!(
                    !options.contains(flag),
                    "{tool} config {action}: unexpected {flag}"
                );
            }
            assert_eq!(
                options.contains("--auth-mode"),
                tool != "ve-adrive" && action == "show"
            );
        }
    }
}

#[test]
fn mcp_help_keeps_only_actual_child_defaults_and_startup_options() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let options = help_options(tool, &["serve"], "zh", "--help");
        for flag in [
            "--profile",
            "--config-path",
            "--credentials-path",
            "--region",
            "--endpoint",
            "--output",
            "--query",
            "--dry-run",
            "--describe",
            "--verbose",
            "--quiet",
        ] {
            assert!(options.contains(flag), "{tool} serve: missing {flag}");
        }
        for flag in ["--psm", "--idc", "--cluster", "--addr-family", "--confirm"] {
            assert!(!options.contains(flag), "{tool} serve: unexpected {flag}");
        }
        assert_eq!(options.contains("--auth-mode"), tool != "ve-adrive");
        assert_eq!(options.contains("--account-id"), tool == "ve-tos");
        assert_eq!(options.contains("--yes"), tool == "ve-adrive");
        assert_eq!(options.contains("--no-color"), tool == "ve-tos");
    }
}

#[test]
fn read_operations_hide_confirmation_flags_while_delete_operations_keep_them() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for command in ["ls", "stat", "du", "find", "cat"] {
            let options = help_options(tool, &[command], "en", "-h");
            assert!(!options.contains("--yes"), "{tool} {command}");
            assert!(!options.contains("--confirm"), "{tool} {command}");
            for flag in [
                "--auth-mode",
                "--profile",
                "--endpoint",
                "--dry-run",
                "--describe",
            ] {
                assert!(options.contains(flag), "{tool} {command}: missing {flag}");
            }
        }
        let options = help_options(tool, &["rm"], "en", "--help");
        for flag in ["--yes", "--confirm", "--quiet"] {
            assert!(options.contains(flag), "{tool} rm: missing {flag}");
        }
    }
}

#[test]
fn adrive_auth_and_utility_api_help_preserve_real_authentication_boundaries() {
    for action in ["status", "login", "logout"] {
        let options = help_options("ve-adrive", &["auth", action], "zh", "--help");
        for flag in [
            "--profile",
            "--config-path",
            "--credentials-path",
            "--auth-mode",
            "--describe",
        ] {
            assert!(options.contains(flag), "auth {action}: missing {flag}");
        }
        for flag in [
            "--region",
            "--quiet",
            "--verbose",
            "--yes",
            "--confirm",
            "--no-color",
        ] {
            assert!(!options.contains(flag), "auth {action}: unexpected {flag}");
        }
        assert_eq!(options.contains("--endpoint"), action == "login");
        assert_eq!(options.contains("--dry-run"), action != "status");
    }
    for tool in ["tos", "ve-adrive"] {
        let options = help_options(tool, &["api"], "en", "--help");
        for flag in STORAGE_OPTIONS {
            assert!(!options.contains(*flag), "{tool} api: unexpected {flag}");
        }
        assert!(options.contains("--dry-run"));
    }
    let options = help_options("ve-tos", &["api"], "en", "--help");
    assert!(options.contains("--auth-mode"));
    assert!(options.contains("--endpoint"));
}

fn scratch() -> PathBuf {
    std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("help-scope-{}", uuid::Uuid::now_v7()))
}

#[test]
fn skill_export_describe_never_creates_files() {
    let directory = scratch();
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let target = directory.join(tool);
        let output = run(&[
            tool,
            "skill",
            "export",
            "--dir",
            target.to_str().unwrap(),
            "--describe",
            "--output",
            "json",
        ]);
        let has_created_files = directory.exists();
        if has_created_files {
            std::fs::remove_dir_all(&directory).unwrap();
        }
        assert!(output.status.success(), "{tool}: {:?}", output.stderr);
        assert!(
            !has_created_files,
            "{tool} --describe wrote exported skills"
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["data"]["command"], format!("{tool} skill export"));
        let names = result["data"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|parameter| parameter["name"].as_str().unwrap())
            .collect::<BTreeSet<_>>();
        assert_eq!(names, ["name", "dir", "language"].into_iter().collect());
    }
}

#[test]
fn skill_discovery_does_not_advertise_ignored_global_properties() {
    for tool in ["tos", "ve-adrive"] {
        let output = run(&[tool, "skill", "list", "--output", "json"]);
        assert!(output.status.success(), "{tool}: {:?}", output.stderr);
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        let skills = result["data"]["skills"].as_array().unwrap();
        for command in ["completion", "capabilities"] {
            let skill = skills
                .iter()
                .find(|skill| skill["command"] == format!("{tool} {command}"))
                .unwrap_or_else(|| panic!("{tool} {command} missing from discovery"));
            let properties = skill["input_schema"]["properties"].as_object().unwrap();
            for name in [
                "profile",
                "auth_mode",
                "region",
                "endpoint",
                "config_path",
                "credentials_path",
                "yes",
                "quiet",
                "dry_run",
            ] {
                assert!(
                    !properties.contains_key(name),
                    "{tool} {command} advertised {name}"
                );
            }
            assert!(properties.contains_key("execute"));
            assert!(properties.contains_key("output"));
        }
    }
}

#[test]
fn capabilities_describe_returns_the_command_contract() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let output = run(&[tool, "capabilities", "--describe", "--output", "json"]);
        assert!(output.status.success());
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["data"]["command"], format!("{tool} capabilities"));
        assert_eq!(result["data"]["supports_dry_run"], false);
    }
}

#[test]
fn doctor_previews_do_not_read_configuration_or_run_live_probes() {
    let directory = scratch();
    std::fs::create_dir(&directory).unwrap();
    let config = directory.join("invalid-config.toml");
    std::fs::write(&config, "invalid configuration [[[\n").unwrap();
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for preview in ["--describe", "--dry-run"] {
            let output = run(&[
                tool,
                "doctor",
                preview,
                "--live-network",
                "--config-path",
                config.to_str().unwrap(),
                "--output",
                "json",
            ]);
            assert!(
                output.status.success(),
                "{tool} {preview}: {:?}",
                output.stderr
            );
            assert!(output.stderr.is_empty());
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(result["data"]["command"], format!("{tool} doctor"));
            assert!(result["data"].get("checks").is_none());
            if preview == "--dry-run" {
                assert_eq!(result["data"]["dry_run"], true);
                assert_eq!(result["data"]["executed"], false);
                assert_eq!(result["data"]["network_requests"], 0);
            }
        }
    }
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "invalid configuration [[[\n"
    );
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn assistant_discovery_keeps_profile_and_hides_unrelated_storage_overrides() {
    for tool in ["tos", "ve-adrive"] {
        let output = run(&[tool, "skill", "list", "--output", "json"]);
        assert!(output.status.success(), "{tool}: {:?}", output.stderr);
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        for suffix in ["ask", "ask auth"] {
            let skill = result["data"]["skills"]
                .as_array()
                .unwrap()
                .iter()
                .find(|skill| skill["command"] == format!("{tool} {suffix}"))
                .unwrap_or_else(|| panic!("{tool} {suffix} missing"));
            let properties = skill["input_schema"]["properties"].as_object().unwrap();
            assert!(properties.contains_key("profile"), "{tool} {suffix}");
            for name in ["auth_mode", "region", "endpoint", "yes"] {
                assert!(!properties.contains_key(name), "{tool} {suffix}: {name}");
            }
        }
    }
}
