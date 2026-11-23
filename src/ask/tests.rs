use super::*;
use clap::{CommandFactory, Parser};
use serde_json::json;

fn knowledge(tool: &str) -> knowledge::Knowledge {
    knowledge::Knowledge::new(crate::Cli::command(), tool).unwrap()
}

#[test]
fn public_knowledge_covers_commands_and_ranks_chinese_questions() {
    for (tool, question, expected) in [
        ("ve-tos", "如何创建存储桶？", "ve-tos mb"),
        ("ve-adrive", "如何上传文件？", "ve-adrive cp"),
        ("tos", "如何上传文件？", "tos cp"),
    ] {
        let catalog = knowledge(tool);
        let prompt = catalog.prompt(question).unwrap();
        assert!(prompt.len() <= 65536);
        assert!(
            prompt.contains(&format!("\"command_id\":\"{expected}\"")),
            "missing ranked details for {expected}"
        );
        assert!(catalog.detailed_ids(question).contains(&expected));
        assert!(!prompt.contains("TOS_SECRET_ACCESS_KEY="));
        assert!(!catalog
            .ids()
            .iter()
            .any(|id| id.contains(" ask") || (tool != "ve-adrive" && id.contains(" auth"))));
        assert!(catalog.ids().len() > 10);
    }
}

fn recommendation(id: &str, argv: &[&str]) -> String {
    json!({"result_type":"recommendation", "summary":"Review this command", "steps":[
        {"command_id":id, "argv":argv, "description":"Inspect syntax and resources"}
    ]})
    .to_string()
}

#[test]
fn recommendations_are_matched_to_the_exact_selected_command() {
    let catalog = knowledge("ve-tos");
    assert!(reply::validate(
        &catalog,
        &recommendation("ve-tos ls", &["ls"]),
        "ve-tos-cli"
    )
    .is_ok());
    for (id, argv) in [
        ("ve-tos made-up", vec!["made-up"]),
        ("ve-tos ls", vec!["mb", "tos://example"]),
        ("ve-tos ls", vec!["ls", "--made-up"]),
        ("ve-tos ls", vec!["--help"]),
        ("ve-adrive ls", vec!["ls"]),
        ("ve-tos ask", vec!["ask", "recurse"]),
    ] {
        assert!(reply::validate(&catalog, &recommendation(id, &argv), "ve-tos-cli").is_err());
    }
}

#[test]
fn model_text_and_structure_are_untrusted_and_bounded() {
    let catalog = knowledge("ve-tos");
    for bad in [
        "not json".to_string(),
        "[]".into(),
        json!({"result_type":"explanation","summary":"\u{1b}[31mbad","steps":[]}).to_string(),
        json!({"result_type":"explanation","summary":"\u{202e}bad","steps":[]}).to_string(),
        json!({"result_type":"explanation","summary":"fine","steps":[],"unknown":true}).to_string(),
        json!({"result_type":"explanation","summary":"x".repeat(5000),"steps":[]}).to_string(),
        recommendation("ve-tos ls", &["ls", "bad\npath"]),
    ] {
        assert!(reply::validate(&catalog, &bad, "ve-tos-cli").is_err());
    }
    for kind in ["explanation", "clarification", "out_of_scope"] {
        let body =
            json!({"result_type":kind,"summary":"Storage CLI scope only", "steps":[]}).to_string();
        assert!(reply::validate(&catalog, &body, "ve-tos-cli").is_ok());
    }
}

#[test]
fn metacharacters_remain_quoted_arguments_and_are_never_executed() {
    let catalog = knowledge("ve-tos");
    let body = recommendation(
        "ve-tos cp",
        &[
            "cp",
            "literal;$(touch /tmp/never-execute-ask)",
            "tos://example/a'b",
        ],
    );
    let result = reply::validate(&catalog, &body, "ve-tos-cli").unwrap();
    assert_eq!(
        result["steps"][0]["command"],
        "ve-tos-cli 'cp' 'literal;$(touch /tmp/never-execute-ask)' 'tos://example/a'\"'\"'b'"
    );
    assert_eq!(result["syntax_validated"], true);
}

#[test]
fn knowledge_scrubs_environment_and_default_values_before_validation() {
    let parser = crate::Cli::command().mut_arg("endpoint", |arg| {
        arg.default_value("synthetic-never-send-default")
    });
    let catalog = knowledge::Knowledge::new(parser, "ve-tos").unwrap();
    assert!(!catalog
        .prompt("list objects")
        .unwrap()
        .contains("synthetic-never-send-default"));
    let matches = catalog.parse(&["ls".into()]).unwrap();
    assert!(matches.get_one::<String>("endpoint").is_none());
}

#[tokio::test]
async fn preview_guards_run_before_configuration_or_state_access() {
    for suffix in [
        vec!["ask", "question", "--dry-run"],
        vec!["ask", "--describe"],
        vec!["ask", "auth", "status", "--dry-run"],
    ] {
        let mut argv = vec!["ve-storage-uni-cli", "ve-tos"];
        argv.extend(suffix);
        let cli = crate::Cli::try_parse_from(argv).unwrap();
        let route = Route::from_tool(&cli.tool).unwrap();
        let result = execute(&cli.global, &route, "ve-storage-uni-cli ve-tos")
            .await
            .unwrap();
        assert!(result.is_object());
        assert!(result["application_bundled"].is_boolean());
        assert!(result.get("configuration").is_none());
    }
}

#[test]
fn missing_application_configuration_points_to_the_publisher_without_login_recovery() {
    let detail = error_detail(
        &GlobalArgs::default(),
        &CliError::ConfigMissing("unconfigured build".into()),
        "cli",
    );
    assert!(detail.fix_command.is_none());
    let action = detail.suggested_action.unwrap();
    assert!(action.contains("official prebuilt"));
    assert!(!action.contains("APP_SECRET"));
}

#[test]
fn question_and_model_format_controls_are_rejected_before_any_flow() {
    let catalog = knowledge("ve-tos");
    for question in ["", "   ", "bad\u{1b}", &"q".repeat(8193)] {
        assert!(catalog.prompt(question).is_err());
    }
    for control in ['\u{206a}', '\u{2028}', '\u{2029}'] {
        let body =
            json!({"result_type":"explanation", "summary":format!("bad{control}text"),"steps":[]})
                .to_string();
        assert!(reply::validate(&catalog, &body, "ve-tos-cli").is_err());
    }
}

#[test]
fn command_parser_drops_synthetic_environment_credentials() {
    const SYNTHETIC_KEY: &str = "ASK_TEST_SYNTHETIC_CREDENTIAL_01";
    std::env::set_var(SYNTHETIC_KEY, "synthetic-never-send-secret");
    let parser = crate::Cli::command().mut_arg("endpoint", |arg| arg.env(SYNTHETIC_KEY));
    std::env::remove_var(SYNTHETIC_KEY);
    let catalog = knowledge::Knowledge::new(parser, "ve-tos").unwrap();
    assert!(!catalog
        .prompt("upload a file")
        .unwrap()
        .contains("synthetic-never-send-secret"));
    assert!(catalog
        .parse(&["ls".into()])
        .unwrap()
        .get_one::<String>("endpoint")
        .is_none());
}

#[test]
fn real_boolean_flags_and_legacy_adrive_auth_remain_recommendable() {
    let catalog = knowledge("ve-adrive");
    assert!(reply::validate(
        &catalog,
        &recommendation(
            "ve-adrive cp",
            &[
                "cp",
                "./folder",
                "adrive://instance/space/folder",
                "--recursive"
            ]
        ),
        "ve-adrive-cli"
    )
    .is_ok());
    assert!(reply::validate(
        &catalog,
        &recommendation(
            "ve-adrive auth login",
            &["auth", "login", "--instance", "instance"]
        ),
        "ve-adrive-cli"
    )
    .is_ok());
    assert!(reply::validate(
        &catalog,
        &recommendation(
            "ve-adrive auth login",
            &["auth", "login", "--provider", "doubao"]
        ),
        "ve-adrive-cli"
    )
    .is_err());
}

#[test]
fn permission_and_quota_failures_preserve_recovery_and_exit_codes() {
    for (failure, expected_exit, expected_kind) in [
        (tos_core::ask::SessionError::Unauthorized, 2, "auth_failed"),
        (
            tos_core::ask::SessionError::PermissionDenied,
            5,
            "permission_denied",
        ),
        (tos_core::ask::SessionError::RateLimited, 7, "rate_limited"),
    ] {
        let error = auth::session_error(failure);
        let detail = error_detail(&GlobalArgs::default(), &error, "ve-tos-cli");
        assert_eq!(detail.exit_code, expected_exit);
        assert_eq!(serde_json::to_value(&detail.kind).unwrap(), expected_kind);
        assert_eq!(detail.fix_command.is_some(), expected_exit == 2);
    }
}

#[test]
fn registry_discovery_preserves_existing_first_commands() {
    let catalogs = [
        ve_tos_cli::registry::COMMAND_GROUPS
            .iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>(),
        ve_tos_cli::registry::CAPABILITIES
            .iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>(),
        tos_cli::registry::CAPABILITIES
            .iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>(),
        ve_adrive_cli::registry::CAPABILITIES
            .iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>(),
    ];
    for commands in catalogs {
        assert!(commands[0].ends_with(" cp"));
        let ask_position = commands
            .iter()
            .position(|command| command.ends_with(" ask"))
            .unwrap();
        assert!(ask_position > 0);
    }
}

#[test]
fn accepted_ask_auth_namespace_parses_for_every_product() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for action in ["login", "status", "logout"] {
            for suffix in [
                vec!["ask", "auth", action, "--dry-run"],
                vec!["--dry-run", "ask", "auth", action],
                vec!["ask", "--dry-run", "auth", action],
                vec!["ask", "auth", "--dry-run", action],
            ] {
                let arguments = [vec!["ve-storage-uni-cli", tool], suffix].concat();
                let cli = crate::Cli::try_parse_from(arguments).expect("accepted ask auth syntax");
                let route = Route::from_tool(&cli.tool).expect("assistant route");
                assert_eq!(route.command(), format!("{tool} ask auth {action}"));
            }
        }
    }
}

#[test]
fn ask_question_forms_and_auth_management_are_exclusive() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for question in ["login", "status", "logout", "help"] {
            let cli = crate::Cli::try_parse_from(["cli", tool, "ask", question]).unwrap();
            assert!(
                matches!(Route::from_tool(&cli.tool).unwrap().action, Action::Ask(Some(value)) if value == question)
            );
        }
        let cli = crate::Cli::try_parse_from(["cli", tool, "ask", "--question", "auth"]).unwrap();
        assert!(matches!(
            Route::from_tool(&cli.tool).unwrap().action,
            Action::Ask(Some("auth"))
        ));
        for suffix in [
            vec!["ask", "question", "--question", "other"],
            vec!["ask", "--question", "one", "--question", "two"],
            vec!["ask", "question", "other"],
            vec!["ask", "auth", "status", "--question", "question"],
            vec!["ask", "auth", "login", "--provider", "doubao"],
            vec!["auth", "login", "--provider", "doubao"],
        ] {
            assert!(crate::Cli::try_parse_from([vec!["cli", tool], suffix].concat()).is_err());
        }
    }
}

#[test]
fn assistant_discovery_uses_the_accepted_auth_namespace() {
    let catalogs = [
        ve_tos_cli::registry::CAPABILITIES
            .iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>(),
        tos_cli::registry::CAPABILITIES
            .iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>(),
        ve_adrive_cli::registry::CAPABILITIES
            .iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>(),
    ];
    for (tool, commands) in ["ve-tos", "tos", "ve-adrive"].into_iter().zip(catalogs) {
        assert!(commands.contains(&format!("{tool} ask auth").as_str()));
        if tool != "ve-adrive" {
            assert!(!commands.contains(&format!("{tool} auth").as_str()));
        }
        assert!(reply::validate(
            &knowledge(tool),
            &recommendation(
                &format!("{tool} ask auth status"),
                &["ask", "auth", "status"]
            ),
            tool
        )
        .is_err());
    }
}

#[test]
fn nested_assistant_metadata_has_no_invented_storage_api() {
    for action in ["login", "status", "logout"] {
        let command = format!("ve-tos ask auth {action}");
        let row = ve_tos_cli::registry::capability_row_for_command(&command, true).unwrap();
        assert!(row.apis.is_empty(), "invented storage API: {:?}", row.apis);
        assert!(row.method.is_none());
        assert!(row.endpoint_rule.is_none());
    }
}

#[tokio::test]
async fn mixed_questions_and_auth_are_rejected_even_in_previews() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for suffix in [
            vec!["ask", "question", "auth", "status"],
            vec!["ask", "--question", "question", "auth", "status"],
            vec!["ask", "--dry-run", "question", "auth", "status"],
            vec![
                "ask",
                "--describe",
                "--question",
                "question",
                "auth",
                "login",
            ],
        ] {
            let cli = crate::Cli::try_parse_from([vec!["cli", tool], suffix].concat()).unwrap();
            let route = Route::from_tool(&cli.tool).unwrap();
            let error = execute(&cli.global, &route, tool).await.unwrap_err();
            assert!(matches!(error, CliError::ValidationError(_)));
        }
    }
}

#[test]
fn auth_error_recovery_preserves_selected_profile_and_explicit_stores() {
    let global = GlobalArgs {
        profile: "staging '$(opaque)".into(),
        config_path: Some("/synthetic/config '$(opaque).toml".into()),
        credentials_path: Some("/synthetic/credentials '$(opaque).toml".into()),
        ..Default::default()
    };
    let detail = error_detail(
        &global,
        &CliError::AuthFailed("login required".into()),
        "tos-cli",
    );
    assert_eq!(detail.fix_command.unwrap(), "tos-cli ask auth login --no-browser --profile 'staging '\"'\"'$(opaque)' --config-path '/synthetic/config '\"'\"'$(opaque).toml' --credentials-path '/synthetic/credentials '\"'\"'$(opaque).toml'");
    assert_eq!(
        auth::fix_command(&GlobalArgs::default(), "tos-cli"),
        "tos-cli ask auth login --no-browser"
    );
}

#[test]
fn auth_recovery_keeps_leading_hyphens_inside_option_values() {
    let global = GlobalArgs {
        profile: "--describe".into(),
        config_path: Some("--dry-run".into()),
        credentials_path: Some("--quiet".into()),
        ..Default::default()
    };
    assert_eq!(auth::fix_command(&global, "tos-cli"), "tos-cli ask auth login --no-browser --profile='--describe' --config-path='--dry-run' --credentials-path='--quiet'");
}
