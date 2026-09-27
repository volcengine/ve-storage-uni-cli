//! Entry-level contracts for ByteTOS authentication selection.
use std::path::Path;
use std::process::{Command, Output};

fn invoke(home: &Path, args: &[&str], mode: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"));
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin");
    if let Some(mode) = mode {
        command.env("BYTETOS_AUTH_MODE", mode);
    }
    command.args(args).output().expect("run isolated CLI")
}

struct TestHome(std::path::PathBuf);
impl TestHome {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture(config: &str) -> TestHome {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let home = TestHome(
        std::env::temp_dir().join(format!("tos-zti-entry-{}-{suffix}", std::process::id())),
    );
    let config_dir = home.path().join(".tos");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), config).unwrap();
    std::fs::write(config_dir.join("credentials.toml"), "invalid = [").unwrap();
    home
}

fn json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn describe_exposes_scoped_auth_schema_without_credentials() {
    let home = fixture("");
    for args in [
        vec!["tos", "ls", "tos://bucket/", "--describe"],
        vec!["tos", "cp", "--describe"],
    ] {
        let result = json(&invoke(home.path(), &args, Some("zti")));
        let parameter = result["data"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|parameter| parameter["name"] == "auth_mode")
            .unwrap();
        assert_eq!(
            parameter["schema"]["enum"],
            serde_json::json!(["aksk", "zti"])
        );
        assert_eq!(parameter["schema"]["default"], "aksk");
    }
}

#[test]
fn discovery_describes_public_zti_and_presign_boundary() {
    let home = fixture("");
    let capabilities = json(&invoke(
        home.path(),
        &["tos", "capabilities", "--view", "full"],
        None,
    ));
    assert_eq!(
        capabilities["data"]["authentication"]["modes"],
        serde_json::json!(["aksk", "zti"])
    );
    assert_eq!(capabilities["data"]["authentication"]["zti_built_in"], true);
    let presign = capabilities["data"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["command"] == "tos presign")
        .unwrap();
    assert_eq!(presign["auth_modes"], serde_json::json!(["aksk"]));
    let description = json(&invoke(
        home.path(),
        &["tos", "presign", "--describe"],
        None,
    ));
    assert!(description["data"]["description"]
        .as_str()
        .unwrap()
        .contains("ZTI"));
    let auth_parameter = description["data"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|parameter| parameter["name"] == "auth_mode")
        .unwrap();
    assert!(auth_parameter["description"]
        .as_str()
        .unwrap()
        .contains("ZTI is not supported"));

    let help = invoke(home.path(), &["tos", "presign", "--help"], None);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("AK/SK only; ZTI is not supported"));
}

#[test]
fn doctor_zti_is_offline_and_skips_broken_aksk_credentials() {
    let home = fixture("[default.tos]\nauth_mode = 'zti'\nregion = 'test-region'\nendpoint = 'http://127.0.0.1:1'\n");
    let token = "SECRET_INVALID_ZTI_TOKEN";
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .env("SEC_TOKEN_STRING", token)
        .args(["tos", "doctor", "--check", "auth"])
        .output()
        .unwrap();
    let result = json(&output);
    let check = &result["data"]["checks"][0];
    assert_eq!(check["status"], "passed");
    assert_eq!(check["details"]["mode"], "zti");
    assert_eq!(check["details"]["source"], "environment_string");
    assert_eq!(check["details"]["remote_verified"], false);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(token));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(token));
}

#[test]
fn full_doctor_in_zti_mode_skips_broken_aksk_credentials() {
    let home = fixture(
        "[default.tos]\nauth_mode = 'zti'\nregion = 'test-region'\npsm = 'example.tos.api'\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env_clear()
        .env("HOME", home.path())
        .env("PATH", "/usr/bin:/bin")
        .env("SEC_TOKEN_STRING", "SECRET_INVALID_ZTI_TOKEN")
        .args(["tos", "doctor"])
        .output()
        .unwrap();
    let result = json(&output);
    assert_eq!(result["data"]["summary"]["failed"], 0);
    let auth = result["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["name"] == "auth")
        .unwrap();
    assert_eq!(auth["details"]["mode"], "zti");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("SECRET_INVALID_ZTI_TOKEN"));
}

#[test]
fn completions_offer_only_byte_tos_auth_values() {
    let home = fixture("");
    for shell in ["bash", "zsh", "fish", "powershell"] {
        let completion = json(&invoke(home.path(), &["tos", "completion", shell], None));
        let script = completion["data"]["script"].as_str().unwrap();
        let option = if shell == "fish" {
            "-l auth-mode"
        } else {
            "--auth-mode"
        };
        for expected in [option, "aksk", "zti"] {
            assert!(script.contains(expected), "{shell} missing {expected}");
        }
        assert!(!script.contains("unified"), "{shell} offered ve-tos mode");
        assert!(!script.contains("oauth"), "{shell} offered adrive mode");
    }
}

#[test]
fn zti_rejects_missing_token_source_without_reading_credentials() {
    // [Review Fix #4] Pass network configuration so this checks token discovery.
    let home = fixture("[default.tos]\nauth_mode = 'zti'\nregion = 'test-region'\nendpoint = 'http://127.0.0.1:1'\n");
    let output = invoke(home.path(), &["tos", "ls", "tos://bucket/"], None);
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("ZtiTokenUnavailable"), "{error}");
    assert!(!error.contains("parse credentials"), "{error}");
}

#[test]
fn zti_dry_run_and_config_show_skip_broken_credentials() {
    let home = fixture("[default.tos]\nauth_mode = 'zti'\nendpoint = 'http://127.0.0.1:1'\naccess_key_id = 'ENC:broken'\nsecret_access_key = 'ENC:broken'\n");
    json(&invoke(
        home.path(),
        &["tos", "ls", "tos://bucket/", "--dry-run"],
        None,
    ));
    let result = json(&invoke(home.path(), &["tos", "config", "show"], None));
    let text = result.to_string();
    assert!(text.contains("zti"), "{text}");
    assert!(!text.contains("ENC:broken"), "{text}");
}

#[test]
fn tos_flag_remains_scoped_and_ve_entries_reject_zti() {
    let home = fixture("");
    for args in [
        vec!["--auth-mode", "zti", "tos", "ls"],
        vec!["ve-tos", "--auth-mode", "zti", "ls"],
        vec!["ve-adrive", "--auth-mode", "zti", "config", "show"],
    ] {
        let output = invoke(home.path(), &args, None);
        assert!(!output.status.success(), "{args:?}");
    }
}

#[test]
fn configuration_and_environment_selection_report_provenance() {
    let home = fixture("[default.tos]\nendpoint = 'http://127.0.0.1:1'\n");
    for (extra, environment, expected_source) in [
        (vec![], Some("zti"), "environment"),
        (vec!["--auth-mode", "zti"], Some("aksk"), "command_line"),
    ] {
        let mut args = vec!["tos", "config", "show"];
        args.extend(extra);
        let result = json(&invoke(home.path(), &args, environment));
        let mode = &result["data"]["profiles"][0]["auth_mode"];
        assert_eq!(mode["value"], "zti");
        assert_eq!(mode["source"], expected_source);
    }
    json(&invoke(
        home.path(),
        &["tos", "config", "set", "auth_mode", "zti"],
        None,
    ));
    let result = json(&invoke(
        home.path(),
        &["tos", "config", "show"],
        Some("aksk"),
    ));
    assert_eq!(
        result["data"]["profiles"][0]["auth_mode"]["source"],
        "BinaryOverride"
    );
    assert!(
        std::fs::read_to_string(home.path().join(".tos/config.toml"))
            .unwrap()
            .contains("auth_mode = \"zti\"")
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_auth_mode_environment_is_rejected_without_aksk_fallback() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let home = fixture("[default.tos]\nendpoint = 'http://127.0.0.1:1'\n");
    std::fs::remove_file(home.path().join(".tos/credentials.toml")).unwrap();
    let invalid_mode = OsString::from_vec(vec![0xff]);
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", "/usr/bin:/bin")
            .env("BYTETOS_AUTH_MODE", &invalid_mode)
            .args(args)
            .output()
            .unwrap()
    };

    let rejected = run(&["tos", "config", "show"]);
    assert!(!rejected.status.success());
    let error = String::from_utf8_lossy(&rejected.stderr);
    assert!(error.contains("BYTETOS_AUTH_MODE"), "{error}");
    assert!(error.contains("UTF-8"), "{error}");

    let overridden = json(&run(&["tos", "config", "show", "--auth-mode", "zti"]));
    assert_eq!(
        overridden["data"]["profiles"][0]["auth_mode"]["value"],
        "zti"
    );
}

#[test]
fn missing_config_suggests_init_for_selected_surface() {
    let home = fixture("");
    std::fs::remove_file(home.path().join(".tos/config.toml")).unwrap();
    std::fs::remove_file(home.path().join(".tos/credentials.toml")).unwrap();
    for (surface, expected) in [
        ("tos", "ve-storage-uni-cli tos config init"),
        ("ve-tos", "ve-storage-uni-cli ve-tos config init"),
    ] {
        let output = invoke(home.path(), &[surface, "config", "show"], None);
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(expected), "{surface}: {error}");
    }
}

#[test]
fn describe_uses_tos_entrypoint_and_tos_routing() {
    let home = fixture("");
    let listing_without_target = json(&invoke(home.path(), &["tos", "ls", "--describe"], None));
    assert_eq!(listing_without_target["data"]["command"], "tos ls");
    let invalid_execution = invoke(home.path(), &["tos", "ls"], None);
    assert!(!invalid_execution.status.success());
    for command in [
        "cp", "mv", "sync", "mkdir", "rm", "stat", "du", "find", "cat", "put", "presign",
    ] {
        let description = json(&invoke(home.path(), &["tos", command, "--describe"], None));
        let details = &description["data"];
        assert_eq!(details["command"], format!("tos {command}"));
        if let Some(examples) = details["output_filter_examples"].as_array() {
            for example in examples {
                let example = example.as_str().unwrap();
                assert!(
                    example.starts_with(&format!("ve-storage-uni-cli tos {command} ")),
                    "{command}: {example}"
                );
            }
        }
        assert!(
            details["related_commands"]["low_level"].is_null(),
            "{command}: {details}"
        );
    }

    let listing = json(&invoke(
        home.path(),
        &["tos", "ls", "tos://bucket/", "--describe"],
        None,
    ));
    let listing = &listing["data"];
    assert_eq!(listing["command"], "tos ls");
    assert!(!listing["scenario_routing"]["target_matrix"]
        .as_str()
        .unwrap()
        .contains("ListBuckets"));
    assert!(!listing["scenario_routing"]["output_shapes"]
        .as_str()
        .unwrap()
        .contains("ve-tos"));

    let copy = json(&invoke(
        home.path(),
        &[
            "tos",
            "cp",
            "tos://bucket/source",
            "tos://bucket/target",
            "--describe",
        ],
        None,
    ));
    let copy_examples = copy["data"]["output_filter_examples"].as_array().unwrap();
    assert!(copy_examples.iter().all(|example| example
        .as_str()
        .unwrap()
        .starts_with("ve-storage-uni-cli tos cp ")));
    let copy_parameters = copy["data"]["parameters"].as_array().unwrap();
    assert!(!copy_parameters
        .iter()
        .any(|parameter| parameter["name"] == "storage-class"));
    let copy_list_mode = copy_parameters
        .iter()
        .find(|parameter| parameter["name"] == "recursive-list-mode")
        .unwrap();
    assert_eq!(
        copy_list_mode["schema"]["enum"],
        serde_json::json!(["hierarchical"])
    );

    let removed = json(&invoke(home.path(), &["tos", "rm", "--describe"], None));
    assert!(removed["data"]["scenario_routing"]["target_scope"]
        .as_str()
        .unwrap()
        .contains("unavailable in tos"));
    let parameter_names = removed["data"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|parameter| parameter["name"].as_str())
        .collect::<Vec<_>>();
    assert!(!parameter_names.contains(&"recursive-delete-mode"));
    assert!(parameter_names.contains(&"all-versions"));
    assert!(parameter_names.contains(&"include-uploads"));
    let rm_list_mode = removed["data"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|parameter| parameter["name"] == "recursive-list-mode")
        .unwrap();
    assert_eq!(
        rm_list_mode["schema"]["enum"],
        serde_json::json!(["hierarchical"])
    );

    let config = json(&invoke(
        home.path(),
        &["tos", "config", "show", "--describe"],
        None,
    ));
    let scenarios = config["data"]["scenario_routing"].to_string();
    assert!(
        scenarios.contains("ve-storage-uni-cli tos config show"),
        "{scenarios}"
    );
    assert!(!scenarios.contains("ve-tos"), "{scenarios}");

    for action in ["init", "set"] {
        let args = if action == "set" {
            vec![
                "tos",
                "config",
                "set",
                "endpoint",
                "example.test",
                "--describe",
            ]
        } else {
            vec!["tos", "config", "init", "--describe"]
        };
        let description = json(&invoke(home.path(), &args, None));
        let scenarios = description["data"]["scenario_routing"].to_string();
        assert!(
            scenarios.contains(&format!("ve-storage-uni-cli tos config {action}")),
            "{scenarios}"
        );
        assert!(!scenarios.contains("ve-tos"), "{scenarios}");
    }
}

#[test]
fn parameter_free_describe_retains_high_level_command_contract() {
    let home = fixture("");
    for command in ["cp", "mv", "sync"] {
        let description = json(&invoke(home.path(), &["tos", command, "--describe"], None));
        let rich_description = json(&invoke(
            home.path(),
            &[
                "tos",
                command,
                "./source",
                "tos://bucket/dest",
                "--describe",
            ],
            None,
        ));
        let parameters = description["data"]["parameters"].as_array().unwrap();
        for positional in ["source", "destination"] {
            assert!(
                parameters.iter().any(|parameter| {
                    parameter["name"] == positional && parameter["required"] == true
                }),
                "{command} missing {positional}: {parameters:?}"
            );
        }
        for field in [
            "api",
            "low_level_apis",
            "wraps_apis",
            "scenario_routing",
            "output_filter_examples",
        ] {
            assert_eq!(
                description["data"][field], rich_description["data"][field],
                "tos {command} {field}"
            );
        }
    }
}

#[test]
fn tos_describe_does_not_advertise_unavailable_operations() {
    let home = fixture("");
    let move_description = json(&invoke(
        home.path(),
        &[
            "tos",
            "mv",
            "tos://bucket/source",
            "tos://bucket/target",
            "--describe",
        ],
        None,
    ));
    assert!(!move_description["data"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|parameter| parameter["name"] == "checkpoint"));

    let mkdir_description = json(&invoke(
        home.path(),
        &["tos", "mkdir", "tos://bucket/folder/", "--describe"],
        None,
    ));
    assert!(!mkdir_description["data"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .any(|parameter| parameter["name"] == "content-type"));

    let list_description = json(&invoke(
        home.path(),
        &["tos", "ls", "tos://bucket/", "--describe"],
        None,
    ));
    let list_path = list_description["data"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|parameter| parameter["name"] == "path")
        .unwrap();
    assert!(list_path["description"]
        .as_str()
        .unwrap()
        .contains("optional --key"));
    let cat_description = json(&invoke(
        home.path(),
        &["tos", "cat", "tos://bucket/key", "--describe"],
        None,
    ));
    let cat_path = cat_description["data"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|parameter| parameter["name"] == "path")
        .unwrap();
    assert!(cat_path["description"]
        .as_str()
        .unwrap()
        .contains("--bucket and --key"));
    for field in ["api", "low_level_apis", "wraps_apis"] {
        assert!(
            !list_description["data"][field]
                .to_string()
                .contains("ListBuckets"),
            "{field}: {}",
            list_description["data"][field]
        );
        assert!(
            list_description["data"][field]
                .to_string()
                .contains("ListObjectsType2"),
            "{field}: {}",
            list_description["data"][field]
        );
    }

    let remove_description = json(&invoke(
        home.path(),
        &["tos", "rm", "tos://bucket/key", "--describe"],
        None,
    ));
    assert!(!remove_description["data"]["scenario_routing"]
        .to_string()
        .contains("HNS"));
    assert!(
        remove_description["data"]["scenario_routing"]["recursive_delete"]
            .as_str()
            .unwrap()
            .contains("object versions")
    );
}

#[test]
fn tos_recursive_list_help_only_offers_hierarchical() {
    let home = fixture("");
    for command in ["cp", "mv", "sync", "rm"] {
        let output = invoke(home.path(), &["tos", command, "--help"], None);
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        let list_mode = help.split("--recursive-list-mode").nth(1).unwrap();
        let list_mode = list_mode.split("\n      --").next().unwrap();
        assert!(list_mode.contains("hierarchical"), "{command}: {list_mode}");
        assert!(!list_mode.contains("- auto:"), "{command}: {list_mode}");
        assert!(!list_mode.contains("- flat:"), "{command}: {list_mode}");
    }
    let chinese = invoke(
        home.path(),
        &["tos", "cp", "--help", "--language", "zh"],
        None,
    );
    assert!(chinese.status.success());
    let help = String::from_utf8(chinese.stdout).unwrap();
    assert!(help.contains("递归列举模式：仅支持 hierarchical"), "{help}");
    for args in [
        vec!["tos", "cp", "-h"],
        vec!["tos", "cp", "-h", "--language", "zh"],
    ] {
        let output = invoke(home.path(), &args, None);
        assert!(output.status.success());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(!help.contains("auto, flat, hierarchical"), "{help}");
    }
}

#[test]
fn rich_describe_parameters_match_tos_capabilities() {
    let home = fixture("");
    let capabilities = json(&invoke(
        home.path(),
        &["tos", "capabilities", "--view", "full"],
        None,
    ));
    let rows = capabilities["data"]["capabilities"].as_array().unwrap();
    for (command, arguments) in [
        ("cp", vec!["./source", "tos://bucket/dest"]),
        ("mv", vec!["./source", "tos://bucket/dest"]),
        ("sync", vec!["./source", "tos://bucket/dest"]),
        ("mkdir", vec!["tos://bucket/folder/"]),
        ("rm", vec!["tos://bucket/key"]),
        ("ls", vec!["tos://bucket/"]),
        ("stat", vec!["tos://bucket/key"]),
        ("du", vec!["tos://bucket/"]),
        ("find", vec!["tos://bucket/"]),
        ("cat", vec!["tos://bucket/key"]),
        ("put", vec!["tos://bucket/key"]),
        ("presign", vec!["tos://bucket/key"]),
    ] {
        let mut args = vec!["tos", command];
        args.extend(arguments);
        args.push("--describe");
        let description = json(&invoke(home.path(), &args, None));
        let names = description["data"]["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|parameter| parameter["name"].as_str())
            .filter(|name| *name != "auth_mode")
            .collect::<std::collections::BTreeSet<_>>();
        let row = rows
            .iter()
            .find(|row| row["command"] == format!("tos {command}"))
            .unwrap();
        if matches!(command, "cp" | "mv" | "sync" | "rm" | "du") {
            assert!(
                row["api_actions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|action| action == "HeadBucket"),
                "tos {command} performs a bucket type lookup"
            );
        }
        assert_eq!(
            description["data"]["description"], row["description"],
            "tos {command} description"
        );
        let expected = row["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|parameter| parameter["name"].as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names, expected, "tos {command}");
        for expected_parameter in row["parameters"].as_array().unwrap() {
            let actual = description["data"]["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .find(|parameter| parameter["name"] == expected_parameter["name"])
                .unwrap();
            assert_eq!(
                actual["required"], expected_parameter["required"],
                "tos {command} {} required",
                expected_parameter["name"]
            );
        }
        assert_eq!(
            description["data"]["low_level_apis"], row["api_actions"],
            "tos {command} API actions"
        );
        assert_eq!(
            description["data"]["wraps_apis"], row["api_actions"],
            "tos {command} wrapped API actions"
        );
    }
}

#[test]
fn tos_filter_examples_have_real_operands_and_cat_uses_raw_bytes() {
    let home = fixture("");
    for (command, arguments) in [
        ("cp", vec!["./source", "tos://bucket/dest"]),
        ("mv", vec!["./source", "tos://bucket/dest"]),
        ("sync", vec!["./source", "tos://bucket/dest"]),
        ("rm", vec!["tos://bucket/key"]),
        ("ls", vec!["tos://bucket/"]),
        ("cat", vec!["tos://bucket/key"]),
    ] {
        let mut args = vec!["tos", command];
        args.extend(arguments);
        args.push("--describe");
        let description = json(&invoke(home.path(), &args, None));
        let examples = description["data"]["output_filter_examples"]
            .as_array()
            .unwrap();
        assert!(!examples.is_empty(), "tos {command}");
        for example in examples {
            let example = example.as_str().unwrap();
            assert!(!example.contains("..."), "tos {command}: {example}");
            if command == "cat" {
                assert!(!example.contains("--query"), "{example}");
                assert!(!example.contains("--output json"), "{example}");
            }
        }
    }
}

#[test]
fn recovered_utility_describe_marks_required_operands_as_paths() {
    let home = fixture("");
    for (command, operands) in [
        ("api", vec!["group", "action"]),
        ("completion", vec!["shell"]),
    ] {
        let description = json(&invoke(home.path(), &["tos", command, "--describe"], None));
        let parameters = description["data"]["parameters"].as_array().unwrap();
        for operand in operands {
            let parameter = parameters
                .iter()
                .find(|parameter| parameter["name"] == operand)
                .unwrap();
            assert_eq!(parameter["location"], "path", "tos {command} {operand}");
        }
    }
}
