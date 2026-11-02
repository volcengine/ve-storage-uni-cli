use clap::Parser;
use ve_adrive_cli::{command_path, ADriveCommand};

#[derive(Parser)]
struct TestCli {
    #[command(subcommand)]
    command: ADriveCommand,
}

#[test]
fn share_cli_exposes_nine_leaf_commands() {
    for (action, arguments) in [
        ("create", vec!["adrive://inst/space/docs/"]),
        ("get", vec!["s", "--instance", "inst"]),
        ("list", vec!["--instance", "inst", "--creator", "u"]),
        (
            "update",
            vec!["s", "--instance", "inst", "--require-login", "false"],
        ),
        ("cancel", vec!["s", "--instance", "inst"]),
        ("inspect", vec!["https://example.test/share/s"]),
        ("files", vec!["https://example.test/share/s"]),
        ("view", vec!["https://example.test/share/s", "--raw"]),
        (
            "download",
            vec!["https://example.test/share/s", "./downloads"],
        ),
    ] {
        let mut argv = vec!["test", "share", action];
        argv.extend(arguments);
        let parsed = TestCli::try_parse_from(argv).expect("share action should parse");
        assert_eq!(
            command_path(&parsed.command),
            format!("ve-adrive share {action}")
        );
    }
}

#[test]
fn share_view_exposes_preview_union_and_raw_boolean() {
    let description = ve_adrive_cli::share_metadata::describe_share_command("ve-adrive share view")
        .expect("view command must have discovery metadata");
    let parameters = description["parameters"].as_array().unwrap();
    assert_eq!(
        parameters
            .iter()
            .find(|item| item["name"] == "raw")
            .unwrap()["schema"]["type"],
        "boolean"
    );
    assert!(description["semantics"]
        .to_string()
        .contains("PreviewLimit"));
    assert!(description["output_schema"]["properties"]
        .get("ResultType")
        .is_some());
    assert!(ve_adrive_cli::share_metadata::supports_password(
        "ve-adrive share view"
    ));
}

#[test]
fn management_share_uris_and_legacy_selectors_both_parse() {
    for action in ["get", "update", "cancel"] {
        for target in [vec!["adrive://inst/s"], vec!["s", "--instance", "inst"]] {
            let mut arguments = vec!["test", "share", action];
            arguments.extend(target);
            if action == "update" {
                arguments.extend(["--name", "New"]);
            }
            let parsed = TestCli::try_parse_from(arguments).unwrap();
            assert_eq!(
                command_path(&parsed.command),
                format!("ve-adrive share {action}")
            );
        }
    }
}

#[test]
fn instance_list_uri_and_legacy_flag_share_help_and_metadata() {
    for selector in [
        vec!["adrive://inst"],
        vec!["adrive://inst/"],
        vec!["--instance", "inst"],
    ] {
        let mut arguments = vec!["test", "share", "list"];
        arguments.extend(selector);
        let parsed = TestCli::try_parse_from(arguments).unwrap();
        assert_eq!(command_path(&parsed.command), "ve-adrive share list");
    }
    // [Review Fix #ShareList2] Extract Clap's help error without imposing Debug on parsed commands.
    let help = TestCli::try_parse_from(["test", "share", "list", "--help"])
        .err()
        .unwrap()
        .to_string();
    assert!(help.contains("[TARGET]"));
    assert!(help.contains("list accepts adrive://instance"));
    let mut description =
        ve_adrive_cli::share_metadata::describe_share_command("ve-adrive share list").unwrap();
    ve_adrive_cli::handler::meta::localize_adrive_auth_documentation_zh(&mut description);
    assert!(description["semantics"]
        .to_string()
        .contains("list 接受 adrive://instance"));
    assert_eq!(description["command"], "ve-adrive share list");
    let parameters = description["parameters"].as_array().unwrap();
    for key in ["target", "instance"] {
        assert_eq!(
            parameters
                .iter()
                .find(|parameter| parameter["name"] == key)
                .unwrap()["required"],
            false
        );
    }
}

#[test]
fn share_registry_has_typed_receiver_and_management_contract() {
    for action in [
        "create", "get", "list", "update", "cancel", "inspect", "files", "view", "download",
    ] {
        let command = format!("ve-adrive share {action}");
        assert!(
            ve_adrive_cli::registry::find_capability(&command).is_some(),
            "{command}"
        );
        let description =
            ve_adrive_cli::handler::meta::describe_adrive_command_metadata(&command).unwrap();
        assert_eq!(description["command"], command);
        if action == "cancel" {
            assert!(description["parameters"]
                .as_array()
                .unwrap()
                .iter()
                .all(|parameter| parameter["name"] != "share-endpoint"));
            assert!(TestCli::try_parse_from([
                "test",
                "share",
                "cancel",
                "s",
                "--instance",
                "inst",
                "--share-endpoint",
                "https://pages.test"
            ])
            .is_err());
        }
    }
}

#[test]
fn share_page_override_uses_canonical_endpoint_name() {
    for arguments in [
        vec!["create", "adrive://inst/space/file.txt"],
        vec!["get", "adrive://inst/s"],
        vec!["list", "adrive://inst"],
        vec!["update", "adrive://inst/s", "--name", "New"],
    ] {
        let command = format!("ve-adrive share {}", arguments[0]);
        let mut input = vec!["test", "share"];
        input.extend(arguments);
        input.extend(["--share-endpoint", "http://pages.example.test:8080"]);
        assert!(TestCli::try_parse_from(input).is_ok(), "{command}");
        let description =
            ve_adrive_cli::handler::meta::describe_adrive_command_metadata(&command).unwrap();
        let parameters = description["parameters"].as_array().unwrap();
        assert!(parameters
            .iter()
            .any(|item| item["name"] == "share-endpoint"));
        assert!(parameters
            .iter()
            .all(|item| item["name"] != "share-base-url"));
    }
}

#[test]
fn share_endpoint_help_hides_legacy_alias() {
    let help = TestCli::try_parse_from(["test", "share", "create", "--help"])
        .err()
        .unwrap()
        .to_string();
    assert!(help.contains("--share-endpoint"));
    assert!(!help.contains("--share-base-url"));
    assert!(TestCli::try_parse_from([
        "test",
        "share",
        "get",
        "adrive://inst/s",
        "--share-base-url",
        "http://pages.test",
    ])
    .is_ok());
}

#[test]
fn share_cli_rejects_negative_limits_and_resume_without_checkpoint() {
    assert!(
        TestCli::try_parse_from(["test", "share", "list", "--instance", "i", "--limit", "0"])
            .is_err()
    );
    assert!(TestCli::try_parse_from([
        "test",
        "share",
        "download",
        "https://x.test/share/s",
        "out",
        "--resume"
    ])
    .is_err());
    assert!(TestCli::try_parse_from([
        "test",
        "share",
        "create",
        "adrive://i/s/a",
        "--preview-limit",
        "-1"
    ])
    .is_err());
}

#[test]
fn receiver_debug_never_contains_url_password() {
    let parsed = TestCli::parse_from([
        "test",
        "share",
        "files",
        "https://x.test/share/s?pwd=must-not-appear",
    ]);
    assert!(!format!("{:?}", parsed.command).contains("must-not-appear"));
}

#[test]
fn sharing_help_and_describe_explain_connection_and_local_root_rules() {
    for action in ["create", "inspect", "download"] {
        let help = TestCli::try_parse_from(["test", "share", action, "--help"])
            .err()
            .unwrap()
            .to_string();
        assert!(help.contains("idsapp-boe"), "{help}");
        assert!(help.contains("--endpoint"), "{help}");
        let command = format!("ve-adrive share {action}");
        let described = ve_adrive_cli::share_metadata::describe_share_command(&command).unwrap();
        assert!(described["semantics"].to_string().contains("idsapp-boe"));
        if action == "download" {
            assert!(help.contains("OS resolves"), "{help}");
            assert!(help.contains("403"), "{help}");
        }
    }
}

#[test]
fn receiver_help_and_describe_explain_optional_url_slash_and_directory_selection() {
    for action in ["inspect", "files", "download"] {
        let help = TestCli::try_parse_from(["test", "share", action, "--help"])
            .err()
            .unwrap()
            .to_string();
        assert!(help.contains("/share/<id>/") && help.contains("one trailing slash"));
        assert!(help.contains("Select directory prefixes with --path"));
        let mut described = ve_adrive_cli::share_metadata::describe_share_command(&format!(
            "ve-adrive share {action}"
        ))
        .unwrap();
        assert!(described["semantics"]
            .to_string()
            .contains("one trailing slash"));
        ve_adrive_cli::handler::meta::localize_adrive_auth_documentation_zh(&mut described);
        assert!(described["semantics"].to_string().contains("末尾一个斜杠"));
        assert!(described["parameters"]
            .to_string()
            .contains("末尾可带一个斜杠"));
    }
}

#[test]
fn share_files_describe_and_help_explain_ls_columns_and_structured_metadata() {
    let help = TestCli::try_parse_from(["test", "share", "files", "--help"])
        .err()
        .unwrap()
        .to_string();
    assert!(
        help.contains("same file_path, size, file_type, updated_at and is_folder columns as ls")
    );
    let mut described =
        ve_adrive_cli::share_metadata::describe_share_command("ve-adrive share files").unwrap();
    assert!(described["semantics"]
        .to_string()
        .contains("JSON and explicit --query"));
    ve_adrive_cli::handler::meta::localize_adrive_auth_documentation_zh(&mut described);
    assert!(described["semantics"]
        .to_string()
        .contains("与 ls 使用相同"));
    assert!(described["semantics"]
        .to_string()
        .contains("JSON 和显式 --query"));
}

#[test]
fn share_files_help_and_localized_describe_explain_combined_result_cap() {
    let help = TestCli::try_parse_from(["test", "share", "files", "--help"])
        .err()
        .unwrap()
        .to_string();
    assert!(help.contains("--max-keys") && help.contains("default 1000"));
    assert!(help.contains("files and folders together"));
    assert!(help.contains("service pages are fetched automatically"));
    let mut described =
        ve_adrive_cli::share_metadata::describe_share_command("ve-adrive share files").unwrap();
    ve_adrive_cli::handler::meta::localize_adrive_auth_documentation_zh(&mut described);
    let semantics = described["semantics"].to_string();
    for phrase in [
        "--max-keys",
        "默认 1000",
        "合计条目数",
        "自动获取服务分页",
        "聚合且规范化",
    ] {
        assert!(
            semantics.contains(phrase),
            "missing localized guidance: {phrase}"
        );
    }
    assert!(!semantics.contains("原始数组"));
}

#[test]
fn share_download_help_does_not_advertise_listing_pagination_flags() {
    let help = TestCli::try_parse_from(["test", "share", "download", "--help"])
        .err()
        .unwrap()
        .to_string();
    for flag in ["--max-keys", "--marker", "--limit"] {
        assert!(!help.contains(flag), "download help advertises {flag}");
    }
    assert!(help.contains("EOF"));
    assert!(help.contains("default to anonymous"));
    assert!(help.contains("Password precedence is stdin > URL pwd"));
    assert!(help.contains("Downloads enumerate all selected service pages automatically"));
}

#[test]
fn share_download_describe_explains_root_selection_denial_and_summary() {
    let mut described =
        ve_adrive_cli::share_metadata::describe_share_command("ve-adrive share download").unwrap();
    let semantics = described["semantics"].to_string();
    assert!(semantics.contains("No --path selects all shared roots"));
    assert!(semantics.contains("cp-style summary") && semantics.contains("file-count progress"));
    assert!(semantics.contains("succeeded=Completed-Skipped"));
    assert!(semantics.contains("Summary.Skipped") && semantics.contains("--no-progress"));
    assert!(semantics.contains("Forbidden.ShareDownloadDisabled"));
    ve_adrive_cli::handler::meta::localize_adrive_auth_documentation_zh(&mut described);
    assert!(described["semantics"]
        .to_string()
        .contains("单文件自动选中"));
    assert!(described["semantics"].to_string().contains("按文件计数"));
    assert!(described["semantics"]
        .to_string()
        .contains("下载自动枚举全部选中服务分页"));
    assert!(described["semantics"]
        .to_string()
        .contains("密码优先级为 stdin > URL pwd"));
}

#[test]
fn share_download_progress_flags_are_boolean_and_mutually_exclusive() {
    let described =
        ve_adrive_cli::share_metadata::describe_share_command("ve-adrive share download").unwrap();
    for name in ["progress", "no-progress"] {
        let parameter = described["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == name)
            .unwrap();
        assert_eq!(parameter["schema"]["type"], "boolean");
        assert_eq!(parameter["required"], false);
    }
    let arguments = [
        "ve-adrive-cli",
        "share",
        "download",
        "http://localhost/share/s",
        "./out",
        "--progress",
        "--no-progress",
    ];
    assert!(TestCli::try_parse_from(arguments).is_err());
}

#[test]
fn chinese_describe_and_discovery_localize_semantics_without_changing_machine_fields() {
    let mut described =
        ve_adrive_cli::share_metadata::describe_share_command("ve-adrive share download").unwrap();
    ve_adrive_cli::handler::meta::localize_adrive_auth_documentation_zh(&mut described);
    let semantics = described["semantics"].as_array().unwrap();
    let connections = semantics
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find(|text| text.contains("接收端标准 IDS"))
        .unwrap();
    assert!(connections.contains("默认使用服务端 ShareURL"));
    assert!(connections.contains("接收端标准 IDS"));
    assert!(semantics
        .iter()
        .filter_map(serde_json::Value::as_str)
        .any(|text| text.contains("用户指定")
            && text.contains("根目录")
            && text.contains("Conflict")));
    assert_eq!(described["command"], "ve-adrive share download");
    let english =
        ve_adrive_cli::share_metadata::describe_share_command("ve-adrive share download").unwrap();
    let mut discovery = serde_json::json!({"high_level_semantics":{"share":english["semantics"]},"path":"ve-adrive share download"});
    ve_adrive_cli::handler::meta::localize_adrive_auth_documentation_zh(&mut discovery);
    assert!(discovery["high_level_semantics"]["share"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .any(|text| text.contains("默认使用服务端 ShareURL")));
    assert_eq!(discovery["path"], "ve-adrive share download");
}

#[test]
fn management_describe_exposes_optional_service_url_and_warning_in_both_languages() {
    for action in ["create", "get", "update", "list"] {
        let command = format!("ve-adrive share {action}");
        let english = ve_adrive_cli::share_metadata::describe_share_command(&command).unwrap();
        let mut chinese = english.clone();
        ve_adrive_cli::handler::meta::localize_adrive_auth_documentation_zh(&mut chinese);
        for described in [&english, &chinese] {
            let schema = if action == "list" {
                &described["output_schema"]["properties"]["Items"]["items"]
            } else {
                &described["output_schema"]
            };
            assert_eq!(schema["properties"]["ShareUrl"]["type"], "string");
            assert_eq!(schema["properties"]["ShareUrlWarning"]["type"], "string");
            assert!(schema.get("required").is_none());
            assert!(described["semantics"].to_string().contains("ShareURL"));
            assert!(described["semantics"]
                .to_string()
                .contains("ShareUrlWarning"));
        }
    }
}

#[test]
fn whole_space_create_help_and_describe_explain_wire_scope() {
    let help = TestCli::try_parse_from(["test", "share", "create", "--help"])
        .err()
        .unwrap()
        .to_string();
    for phrase in ["whole Space", "ShareAllFiles=true", "FilePathList=[]"] {
        assert!(help.contains(phrase), "missing create guidance: {phrase}");
    }
    let mut description =
        ve_adrive_cli::share_metadata::describe_share_command("ve-adrive share create").unwrap();
    assert!(description["semantics"].to_string().contains("whole Space"));
    assert!(description["examples"]
        .to_string()
        .contains("adrive://inst/space --"));
    ve_adrive_cli::handler::meta::localize_adrive_auth_documentation_zh(&mut description);
    assert!(description["semantics"].to_string().contains("整个 Space"));
    assert!(description["parameters"].to_string().contains("Space 根"));
}

#[test]
fn whole_space_receiver_help_and_describe_preserve_root_layout_rules() {
    for action in ["files", "download"] {
        let help = TestCli::try_parse_from(["test", "share", action, "--help"])
            .err()
            .unwrap()
            .to_string();
        assert!(help.contains("whole-Space share"), "{action}");
        assert!(help.contains("--path /"), "{action}");
        if action == "download" {
            assert!(help.contains("full Space-relative hierarchy"));
            assert!(help.contains("root has no basename"));
        }
        let mut description = ve_adrive_cli::share_metadata::describe_share_command(&format!(
            "ve-adrive share {action}"
        ))
        .unwrap();
        ve_adrive_cli::handler::meta::localize_adrive_auth_documentation_zh(&mut description);
        assert!(description["semantics"]
            .to_string()
            .contains("整 Space 分享"));
        assert!(description["semantics"].to_string().contains("--path /"));
        if action == "download" {
            assert!(description["semantics"]
                .to_string()
                .contains("完整空间相对层级"));
            assert!(description["semantics"]
                .to_string()
                .contains("根没有 basename"));
        }
    }
}

#[test]
fn management_output_schema_preserves_unknown_whole_space_scope() {
    for action in ["create", "get", "list", "update"] {
        let description = ve_adrive_cli::share_metadata::describe_share_command(&format!(
            "ve-adrive share {action}"
        ))
        .unwrap();
        let schema = if action == "list" {
            &description["output_schema"]["properties"]["Items"]["items"]
        } else {
            &description["output_schema"]
        };
        assert_eq!(schema["properties"]["ShareAllFiles"]["type"], "boolean");
        assert!(schema.get("required").is_none());
        assert!(schema["properties"]["ShareAllFiles"]
            .get("default")
            .is_none());
    }
}
