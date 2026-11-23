//! Synthetic process tests: no real storage profiles, provider traffic or browser calls.
use serde_json::Value;
use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
};

fn run(tool: &str, arguments: &[&str], home: &PathBuf) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("STORAGE_CLI_ASK_HOME", home.join("ask"))
        // Keep invalid-config fixtures independent of publisher inputs in the test binary.
        .env("STORAGE_CLI_DOUBAO_APP_ID", "synthetic_incomplete_override")
        .args([tool])
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}
fn scratch() -> PathBuf {
    std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("ask-cli-synthetic-{}", uuid::Uuid::now_v7()))
}
fn envelope(output: &Output) -> Value {
    let bytes = if output.status.success() {
        assert!(output.stderr.is_empty());
        &output.stdout
    } else {
        assert!(output.stdout.is_empty());
        &output.stderr
    };
    serde_json::from_slice(bytes).unwrap_or_else(|_| {
        panic!(
            "invalid envelope: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

#[test]
fn configuration_errors_remain_visible_across_formats_and_queries() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for format in ["json", "yaml", "xml", "markdown", "table", "csv"] {
            verify_configuration_diagnostic(tool, format);
        }
    }
}

fn verify_configuration_diagnostic(tool: &str, format: &str) {
    let home = scratch();
    for extra in [
        vec![],
        vec!["--quiet"],
        vec!["--query", "data"],
        vec!["--query", "["],
    ] {
        let mut arguments = vec!["ask", "auth", "status", "--output", format];
        arguments.extend(extra);
        let output = run(tool, &arguments, &home);
        assert_eq!(output.status.code(), Some(3), "{tool} {arguments:?}");
        assert!(output.stdout.is_empty(), "{tool} {arguments:?}");
        let result: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(result["success"], false);
        assert_eq!(result["command"], format!("{tool} ask auth status"));
        assert_eq!(result["error"]["code"], "doubao_configuration");
        assert_eq!(result["error"]["exit_code"], 3);
        assert!(result["error"]["message"]
            .as_str()
            .unwrap()
            .contains("configuration"));
        assert!(result["error"]["suggested_action"]
            .as_str()
            .unwrap()
            .contains("publisher"));
        assert!(result["error"]["fix_command"].is_null());
        let request_id = uuid::Uuid::parse_str(result["request_id"].as_str().unwrap()).unwrap();
        assert_eq!(request_id.get_version_num(), 7);
        let diagnostic = String::from_utf8_lossy(&output.stderr);
        assert!(!diagnostic.contains("synthetic_incomplete_override"));
        assert!(!home.exists());
    }
}

#[test]
fn question_configuration_errors_do_not_render_an_empty_table() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        let output = run(tool, &["ask", "“测试”", "--output", "table"], &home);
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(envelope(&output)["error"]["code"], "doubao_configuration");
        assert!(!home.exists());
    }
}

#[test]
fn successful_usage_preserves_formatting_and_query_on_stdout() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for format in ["table", "csv"] {
            let output = run(
                tool,
                &["ask", "--output", format, "--query", "data.usage"],
                &home,
            );
            assert!(output.status.success());
            assert!(output.stderr.is_empty());
            let rendered = String::from_utf8_lossy(&output.stdout);
            assert!(rendered.contains(&format!("ve-storage-uni-cli {tool} ask 'question'")));
            assert!(!rendered.contains("request_id"));
            assert!(rendered.starts_with(if format == "table" { "+" } else { "value\n" }));
        }
        assert!(!home.exists());
    }
}

#[test]
fn previews_and_bare_ask_do_not_create_state_without_configuration() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for arguments in [
            vec!["ask"],
            vec!["ask", "--describe"],
            vec!["ask", "question", "--dry-run"],
            vec!["ask", "auth", "login", "--dry-run"],
        ] {
            let output = run(tool, &arguments, &home);
            assert!(
                output.status.success(),
                "{tool} {arguments:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let result = envelope(&output);
            assert_eq!(result["success"], true);
            assert_eq!(result["data"]["executed"], false);
            let id = uuid::Uuid::parse_str(result["request_id"].as_str().unwrap()).unwrap();
            assert_eq!(id.get_version_num(), 7);
            assert!(!home.exists(), "preview created local state");
        }
    }
}
#[test]
fn missing_auth_in_a_pipe_returns_doubao_recovery_without_browser() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        verify_missing_auth_diagnostic(tool, &["ask", "如何上传文件？"]);
        for format in ["json", "yaml", "xml", "markdown", "table", "csv"] {
            verify_missing_auth_diagnostic(
                tool,
                &[
                    "ask",
                    "如何上传文件？",
                    "--output",
                    format,
                    "--quiet",
                    "--query",
                    "data",
                ],
            );
        }
    }
}

fn verify_missing_auth_diagnostic(tool: &str, arguments: &[&str]) {
    let home = scratch();
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env_clear()
        .env("STORAGE_CLI_ASK_HOME", home.join("ask"))
        .env("TOS_CONFIG_PATH", home.join("config.toml"))
        .env("STORAGE_CLI_DOUBAO_APP_ID", "synthetic_app")
        .env("STORAGE_CLI_DOUBAO_SCOPES", "synthetic_confirmed_scope")
        .arg(tool)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let result = envelope(&output);
    assert_eq!(result["error"]["code"], "doubao_login_required");
    assert_eq!(result["error"]["exit_code"], 2);
    assert_eq!(result["command"], format!("{tool} ask"));
    assert_eq!(
        result["error"]["fix_command"],
        format!(
            "ve-storage-uni-cli {tool} ask auth login --no-browser --config-path {}",
            shell_quote(home.join("config.toml").to_str().unwrap())
        )
    );
    assert!(!output.stderr.windows(4).any(|bytes| bytes == b"AKSK"));
    assert!(!home.join("ask/tokens").exists());
    std::fs::remove_dir_all(&home).unwrap();
}
#[test]
fn removed_provider_syntax_fails_before_configuration_or_state() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for arguments in [
            vec!["auth", "login", "--provider", "doubao", "--no-browser"],
            vec!["ask", "auth", "login", "--provider", "doubao"],
            vec!["ask", "auth", "login", "--provider", "doubao", "--describe"],
            vec!["auth", "login", "--provider", "doubao", "--describe"],
            vec!["ask", "auth", "--provider", "doubao", "--describe"],
            vec!["ask", "auth", "--no-browser", "--dry-run"],
            vec!["ask", "auth", "unknown-action", "--describe"],
        ] {
            let output = run(tool, &arguments, &home);
            assert!(!output.status.success());
            assert!(!home.exists());
        }
    }
}
#[test]
fn assistant_help_is_available_without_configuration() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for arguments in [
            vec!["ask", "--help"],
            vec!["ask", "auth", "--help"],
            vec!["ask", "auth", "login", "--help"],
        ] {
            let output = run(tool, &arguments, &home);
            assert!(output.status.success());
            assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
            assert!(!home.exists());
        }
    }
}

#[test]
fn assistant_help_demonstrates_direct_questions_before_login_management() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for language in ["en", "zh"] {
            for help_flag in ["-h", "--help"] {
                verify_direct_question_help(tool, language, help_flag);
            }
        }
    }
}

fn verify_direct_question_help(tool: &str, language: &str, help_flag: &str) {
    let home = scratch();
    let prefix = format!("ve-storage-uni-cli {tool}");
    for mut arguments in [
        vec!["ask"],
        vec!["ask", "How do I upload a file?"],
        vec!["ask", "--question", "auth"],
    ] {
        arguments.extend([help_flag, "--language", language]);
        let output = run(tool, &arguments, &home);
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let help = String::from_utf8_lossy(&output.stdout);
        assert_direct_question_examples(&help, &prefix, language, help_flag);
        assert!(!home.exists());
    }
}

fn assert_direct_question_examples(help: &str, prefix: &str, language: &str, help_flag: &str) {
    let question_position = help
        .find(&format!("  {prefix} ask '"))
        .unwrap_or_else(|| panic!("direct question example missing: {help}"));
    let login_position = help
        .find(&format!("  {prefix} ask auth login"))
        .unwrap_or_else(|| panic!("login example missing: {help}"));
    assert!(question_position < login_position);
    assert_standard_help_layout(help, language);
    assert!(
        help.contains(&format!("{prefix} ask '<QUESTION>' [OPTIONS]")),
        "{help}"
    );
    assert!(
        help.contains(&format!("{prefix} ask auth <COMMAND> [OPTIONS]")),
        "{help}"
    );
    assert!(!help.contains("ask [OPTIONS] [QUESTION] [COMMAND]"));
    assert!(help.contains(&format!("{prefix} ask --question 'auth'")));
    assert_eq!(
        help.matches(if language == "zh" {
            "示例:"
        } else {
            "Examples:"
        })
        .count(),
        1
    );
    if help_flag == "--help" {
        assert!(
            help.contains(if language == "zh" {
                "在 ask 后直接填写用引号括起的问题"
            } else {
                "Put the quoted question directly after ask"
            }),
            "{help}"
        );
    }
}

fn assert_standard_help_layout(help: &str, language: &str) {
    let headings = if language == "zh" {
        ["用法:", "选项:", "示例:", "语言:"]
    } else {
        ["Usage:", "Options:", "Examples:", "Language:"]
    };
    let positions = headings.map(|heading| {
        help.lines()
            .position(|line| line.starts_with(heading))
            .unwrap_or_else(|| panic!("missing {heading}: {help}"))
    });
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{help}");
    let mut previous_position = positions[0];
    for heading in ["Commands:", "Arguments:", "命令:", "参数:"] {
        if let Some(position) = help.lines().position(|line| line.starts_with(heading)) {
            assert!(
                previous_position < position && position < positions[1],
                "{help}"
            );
            previous_position = position;
        }
    }
}

#[test]
fn question_and_storage_help_share_the_same_section_order() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for language in ["en", "zh"] {
            for help_flag in ["-h", "--help"] {
                verify_help_section_order(tool, language, help_flag);
            }
        }
    }
}

fn verify_help_section_order(tool: &str, language: &str, help_flag: &str) {
    let home = scratch();
    for command in ["ask", "cp", "completion"] {
        let output = run(tool, &[command, help_flag, "--language", language], &home);
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert_standard_help_layout(&String::from_utf8_lossy(&output.stdout), language);
        assert!(!home.exists());
    }
}

#[test]
fn assistant_help_only_advertises_effective_options() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for language in ["en", "zh"] {
            for help_flag in ["-h", "--help"] {
                verify_assistant_help(tool, language, help_flag);
            }
        }
    }
}

fn verify_assistant_help(tool: &str, language: &str, help_flag: &str) {
    let home = scratch();
    for suffix in [
        vec![],
        vec!["auth"],
        vec!["auth", "login"],
        vec!["auth", "status"],
        vec!["auth", "logout"],
    ] {
        let mut arguments = vec!["ask"];
        arguments.extend(&suffix);
        arguments.extend([help_flag, "--language", language]);
        let output = run(tool, &arguments, &home);
        assert!(output.status.success(), "{tool} {arguments:?}");
        assert!(output.stderr.is_empty());
        let help = String::from_utf8_lossy(&output.stdout);
        assert_assistant_help_options(&help, suffix.is_empty(), suffix.last() == Some(&"login"));
        assert!(help.contains(&format!("ve-storage-uni-cli {tool} ask")));
        assert!(!home.exists());
    }
}

fn assert_assistant_help_options(help: &str, is_question: bool, is_login: bool) {
    for flag in [
        "--auth-mode",
        "--region",
        "--endpoint",
        "--psm",
        "--idc",
        "--cluster",
        "--addr-family",
        "--control-endpoint",
        "--account-id",
        "--yes",
        "--confirm",
        "--no-color",
    ] {
        assert!(!help.contains(flag), "irrelevant {flag}: {help}");
    }
    for flag in [
        "--profile",
        "--config-path",
        "--credentials-path",
        "--output",
        "--query",
        "--dry-run",
        "--describe",
        "--verbose",
        "--quiet",
        "--language",
        "--help",
    ] {
        assert!(help.contains(flag), "missing {flag}: {help}");
    }
    for (flag, is_expected) in [("--question", is_question), ("--no-browser", is_login)] {
        let is_advertised = help.lines().any(|line| line.trim_start().starts_with(flag));
        assert_eq!(is_advertised, is_expected, "{flag}: {help}");
    }
}

#[test]
fn storage_help_keeps_its_authentication_and_profile_options() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for arguments in [
            vec!["ls", "--help"],
            vec!["cp", "ask", "--help"],
            vec!["--profile", "ask", "cp", "--help"],
        ] {
            let output = run(tool, &arguments, &home);
            assert!(output.status.success());
            let help = String::from_utf8_lossy(&output.stdout);
            for flag in ["--auth-mode", "--profile", "--region", "--endpoint"] {
                assert!(help.contains(flag), "{tool} {arguments:?} missing {flag}");
            }
        }
        assert!(!home.exists());
    }
}

#[cfg(unix)]
#[test]
fn dedicated_assistant_help_uses_the_same_option_boundary() {
    for binary_name in ["tos-cli", "ve-tos-cli", "ve-adrive-cli"] {
        let directory = scratch();
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join(binary_name);
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ve-storage-uni-cli"), &executable).unwrap();
        for language in ["en", "zh"] {
            for help_flag in ["-h", "--help"] {
                for arguments in [
                    vec!["ask"],
                    vec!["ask", "auth"],
                    vec!["ask", "auth", "login"],
                    vec!["ask", "auth", "status"],
                    vec!["ask", "auth", "logout"],
                ] {
                    verify_dedicated_assistant_help(
                        &executable,
                        binary_name,
                        &arguments,
                        language,
                        help_flag,
                    );
                }
            }
        }
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

fn verify_dedicated_assistant_help(
    executable: &std::path::Path,
    binary_name: &str,
    arguments: &[&str],
    language: &str,
    help_flag: &str,
) {
    let output = Command::new(executable)
        .env_clear()
        .args(arguments)
        .args([help_flag, "--language", language])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let help = String::from_utf8_lossy(&output.stdout);
    assert_assistant_help_options(
        &help,
        arguments.len() == 1,
        arguments.last() == Some(&"login"),
    );
    assert!(help.contains(&format!("{binary_name} ask")));
    if arguments.len() == 1 {
        assert_direct_question_examples(&help, binary_name, language, help_flag);
    }
}

#[cfg(unix)]
#[test]
fn dedicated_binary_names_select_their_own_usage_prefix() {
    for binary_name in ["tos-cli", "ve-tos-cli", "ve-adrive-cli"] {
        let directory = scratch();
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join(binary_name);
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_ve-storage-uni-cli"), &executable).unwrap();
        let home = directory.join("home");
        let output = Command::new(&executable)
            .env_clear()
            .env("STORAGE_CLI_ASK_HOME", home.join("ask"))
            .args(["ask", "--dry-run"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            envelope(&output)["data"]["usage"],
            format!("{binary_name} ask 'question'")
        );
        assert!(!home.exists());
        for action in [None, Some("login"), Some("status"), Some("logout")] {
            let mut arguments = vec!["ask", "auth"];
            arguments.extend(action);
            arguments.push("--dry-run");
            let output = Command::new(&executable)
                .env_clear()
                .env("STORAGE_CLI_ASK_HOME", home.join("ask"))
                .args(arguments)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                envelope(&output)["data"]["login"],
                format!("{binary_name} ask auth login --no-browser")
            );
            if action.is_none() {
                assert_eq!(
                    envelope(&output)["data"]["usage"],
                    format!("{binary_name} ask auth <login|status|logout>")
                );
            }
            assert!(!home.exists());
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn status_and_logout_need_no_app_secret_file() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for action in ["status", "logout", "status"] {
            let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
                .env_clear()
                .env("STORAGE_CLI_ASK_HOME", home.join("ask"))
                .env("TOS_CONFIG_PATH", home.join("config.toml"))
                .env("STORAGE_CLI_DOUBAO_APP_ID", "synthetic_app")
                .env("STORAGE_CLI_DOUBAO_SCOPES", "synthetic_confirmed_scope")
                .args([tool, "ask", "auth", action])
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert_eq!(envelope(&output)["data"]["status"], "logged_out");
        }
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[test]
fn assistant_success_and_failure_keep_the_selected_product_identity() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for (arguments, is_success) in [
            (vec!["ask"], true),
            (vec!["ask", "--describe"], true),
            (vec!["ask", "question"], false),
        ] {
            let output = run(tool, &arguments, &home);
            let result = envelope(&output);
            assert_eq!(result["success"], is_success);
            assert_eq!(result["command"], format!("{tool} ask"));
            assert!(!home.exists());
        }
    }
}

fn verify_localized_preview(tool: &str, language: &str, action: Option<&str>) {
    let home = scratch();
    let mut arguments = vec!["ask"];
    if let Some(action) = action {
        arguments.extend(["auth", action]);
    }
    arguments.extend(["--describe", "--language", language]);
    let output = run(tool, &arguments, &home);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = envelope(&output);
    let suffix = action
        .map(|action| format!("ask auth {action}"))
        .unwrap_or("ask".into());
    assert_eq!(result["command"], format!("{tool} {suffix}"));
    assert_eq!(result["data"]["command"], result["command"]);
    let prefix = "ve-storage-uni-cli";
    let expected_usage = match action {
        Some("login") => format!("{prefix} {tool} ask auth login [--no-browser]"),
        Some(action) => format!("{prefix} {tool} ask auth {action}"),
        None => format!("{prefix} {tool} ask 'question'"),
    };
    assert_eq!(result["data"]["usage"], expected_usage);
    assert_eq!(result["data"]["experimental"], true);
    assert_eq!(result["data"]["provider"], "doubao");
    assert_eq!(result["data"]["executed"], false);
    assert!(result["data"]["application_bundled"].is_boolean());
    assert!(result["data"].get("configuration").is_none());
    let notice = result["data"]["security_notice"].as_str().unwrap();
    assert!(notice.contains(if language == "zh" {
        "敏感"
    } else {
        "sensitive"
    }));
    let request_id = uuid::Uuid::parse_str(result["request_id"].as_str().unwrap()).unwrap();
    assert_eq!(request_id.get_version_num(), 7);
    assert!(!home.exists());
}

#[test]
fn localized_assistant_describe_uses_the_typed_preview_and_uuid_v7() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for language in ["en", "zh"] {
            for action in [None, Some("login"), Some("status"), Some("logout")] {
                verify_localized_preview(tool, language, action);
            }
        }
    }
}

#[test]
fn every_auth_action_preview_and_help_avoids_state() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for action in ["login", "status", "logout"] {
            for flag in ["--help", "--dry-run"] {
                let output = run(tool, &["ask", "auth", action, flag], &home);
                assert!(output.status.success(), "{tool} {action} {flag}");
                assert!(!home.exists());
            }
        }
    }
}

#[test]
fn bare_ask_auth_shows_usage_without_entering_question_flow() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        let output = run(tool, &["ask", "auth"], &home);
        assert!(output.status.success());
        assert_eq!(envelope(&output)["data"]["executed"], false);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("doubao_configuration"));
        assert!(!home.exists());
    }
}

#[test]
fn mixed_question_auth_inputs_fail_before_previews_or_configuration() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for arguments in [
            vec!["ask", "question", "auth", "status"],
            vec!["ask", "--question", "auth", "auth", "login"],
            vec!["ask", "--dry-run", "question", "auth", "logout"],
            vec!["ask", "question", "auth", "--dry-run"],
            vec!["ask", "--describe", "--question", "auth", "auth"],
            vec!["ask", "--describe", "--question", "auth", "auth", "status"],
            vec![
                "ask",
                "auth",
                "status",
                "--question",
                "question",
                "--describe",
            ],
        ] {
            let output = run(tool, &arguments, &home);
            assert_eq!(output.status.code(), Some(6));
            assert_eq!(envelope(&output)["error"]["kind"], "validation_error");
            assert!(!home.exists());
        }
    }
}

#[test]
fn storage_describe_recovery_preserves_ordinary_ask_argument_values() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for arguments in [
            vec!["cp", "ask", "--describe"],
            vec!["--profile", "ask", "cp", "--describe"],
        ] {
            let output = run(tool, &arguments, &home);
            assert!(output.status.success(), "{tool} {arguments:?}");
            assert!(!home.exists());
        }
    }
}

#[test]
fn auth_group_advertised_previews_and_usage_are_side_effect_free() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for arguments in [
            vec!["ask", "auth"],
            vec!["ask", "auth", "--dry-run"],
            vec!["ask", "--dry-run", "auth"],
            vec!["--dry-run", "ask", "auth"],
            vec!["ask", "auth", "--describe", "--language", "en"],
            vec!["ask", "auth", "--describe", "--language", "zh"],
        ] {
            let output = run(tool, &arguments, &home);
            assert!(output.status.success(), "{tool} {arguments:?}");
            let result = envelope(&output);
            assert_eq!(result["command"], format!("{tool} ask auth"));
            assert_eq!(result["data"]["executed"], false);
            assert_eq!(
                result["data"]["usage"],
                format!("ve-storage-uni-cli {tool} ask auth <login|status|logout>")
            );
            if arguments.contains(&"--describe") {
                let security_notice = result["data"]["security_notice"].as_str().unwrap();
                let expected_notice = arguments
                    .contains(&"zh")
                    .then_some("敏感")
                    .unwrap_or("sensitive");
                assert!(security_notice.contains(expected_notice));
            }
            let request_id = uuid::Uuid::parse_str(result["request_id"].as_str().unwrap()).unwrap();
            assert_eq!(request_id.get_version_num(), 7);
            assert!(!home.exists());
        }
    }
}

#[test]
fn leading_help_preserves_the_explicit_question_value() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for language in ["en", "zh"] {
            let home = scratch();
            let leading = run(
                "--help",
                &[tool, "ask", "--question", "auth", "--language", language],
                &home,
            );
            let trailing = run(
                tool,
                &[
                    "ask",
                    "--question",
                    "auth",
                    "--help",
                    "--language",
                    language,
                ],
                &home,
            );
            assert!(leading.status.success());
            assert!(trailing.status.success());
            let leading_help = String::from_utf8_lossy(&leading.stdout);
            let trailing_help = String::from_utf8_lossy(&trailing.stdout);
            assert!(leading_help.contains("--question"), "{tool} {language}");
            assert_eq!(leading_help, trailing_help, "{tool} {language}");
            assert!(!home.exists());
        }
    }
}

#[test]
fn incomplete_application_override_has_publisher_recovery_without_a_login_fix() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let home = scratch();
        for arguments in [
            vec!["ask", "question"],
            vec!["ask", "auth", "login"],
            vec!["ask", "auth", "login", "--no-browser"],
            vec!["ask", "auth", "status"],
            vec!["ask", "auth", "logout"],
        ] {
            let output = run(tool, &arguments, &home);
            assert_eq!(output.status.code(), Some(3));
            let result = envelope(&output);
            assert_eq!(result["error"]["code"], "doubao_configuration");
            assert!(result["error"]["fix_command"].is_null());
            assert!(result["error"]["suggested_action"]
                .as_str()
                .unwrap()
                .contains("official prebuilt"));
            let rendered = String::from_utf8_lossy(&output.stderr);
            assert!(!rendered.contains("APP_SECRET"));
            assert!(!rendered.contains("synthetic_incomplete_override"));
            assert!(!home.exists());
        }
    }
}

#[test]
fn previews_preserve_profile_and_store_recovery_without_touching_invalid_files() {
    let directory = scratch();
    std::fs::create_dir(&directory).unwrap();
    let config = directory.join("config '$(opaque).toml");
    let credentials = directory.join("credentials '$(opaque).toml");
    std::fs::write(&config, "invalid configuration [[[\n").unwrap();
    std::fs::write(&credentials, "invalid credentials [[[\n").unwrap();
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for suffix in [
            vec![],
            vec!["auth"],
            vec!["auth", "login"],
            vec!["auth", "status"],
            vec!["auth", "logout"],
        ] {
            for preview in ["--describe", "--dry-run"] {
                verify_scoped_preview(tool, &suffix, preview, &config, &credentials);
            }
        }
    }
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "invalid configuration [[[\n"
    );
    assert_eq!(
        std::fs::read_to_string(&credentials).unwrap(),
        "invalid credentials [[[\n"
    );
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
    std::fs::remove_dir_all(directory).unwrap();
}

fn verify_scoped_preview(
    tool: &str,
    suffix: &[&str],
    preview: &str,
    config: &std::path::Path,
    credentials: &std::path::Path,
) {
    let profile = "auth '$(opaque)";
    let arguments = [
        vec!["ask"],
        suffix.to_vec(),
        vec![
            preview,
            "--profile",
            profile,
            "--config-path",
            config.to_str().unwrap(),
            "--credentials-path",
            credentials.to_str().unwrap(),
        ],
    ]
    .concat();
    let output = run(tool, &arguments, &config.parent().unwrap().to_path_buf());
    assert!(
        output.status.success(),
        "{tool} {arguments:?}: {:?}",
        output.stderr
    );
    let expected = format!("ve-storage-uni-cli {tool} ask auth login --no-browser --profile {} --config-path {} --credentials-path {}", shell_quote(profile), shell_quote(config.to_str().unwrap()), shell_quote(credentials.to_str().unwrap()));
    assert_eq!(envelope(&output)["data"]["login"], expected);
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn save_synthetic_login(credentials: &std::path::Path, profile: &str) {
    let config = tos_core::ask::Config::new(
        "synthetic_app".into(),
        "http://localhost:8080/callback",
        vec!["synthetic_confirmed_scope".into()],
        credentials.parent().unwrap().to_path_buf(),
    )
    .unwrap()
    .with_credentials_store(credentials.to_path_buf(), profile.into())
    .unwrap();
    let tokens: tos_core::ask::TokenSet = serde_json::from_value(serde_json::json!({
        "access_token":"synthetic-profile-access", "refresh_token":"synthetic-profile-refresh",
        "access_expires_at":4102444800_u64, "refresh_expires_at":4102444800_u64,
        "scope":"synthetic_confirmed_scope offline_access"
    }))
    .unwrap();
    tos_core::ask::Store::open(&config)
        .unwrap()
        .save_login(&tokens)
        .unwrap();
}

fn configured_auth(tool: &str, arguments: &[&str]) -> Value {
    let config_index = arguments
        .iter()
        .position(|argument| *argument == "--config-path")
        .unwrap()
        + 1;
    let home = std::path::Path::new(arguments[config_index])
        .parent()
        .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env_clear()
        .env("STORAGE_CLI_DOUBAO_APP_ID", "synthetic_app")
        .env("STORAGE_CLI_DOUBAO_SCOPES", "synthetic_confirmed_scope")
        .env("STORAGE_CLI_ASK_HOME", home.join("legacy-ask"))
        .arg(tool)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{tool} {arguments:?}: {:?}",
        output.stderr
    );
    let result = envelope(&output);
    assert_eq!(result["data"]["provider"], "doubao");
    result
}

fn auth_state(tool: &str, profile: &str, action: &str, config: &std::path::Path) -> Value {
    configured_auth(
        tool,
        &[
            "ask",
            "auth",
            action,
            "--profile",
            profile,
            "--config-path",
            config.to_str().unwrap(),
        ],
    )["data"]["status"]
        .clone()
}

#[test]
fn auth_status_and_logout_follow_profile_and_selected_credentials_path() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        let directory = scratch();
        let credentials = directory.join("credentials.toml");
        let alternate = directory.join("alternate.toml");
        let config = directory.join("config.toml");
        save_synthetic_login(&credentials, "staging");
        save_synthetic_login(&alternate, "staging");
        let key = std::fs::read(directory.join(".key")).unwrap();
        let config_flag = ["--config-path", config.to_str().unwrap()];
        for (profile, action, expected) in [
            ("default", "status", "logged_out"),
            ("staging", "status", "ready"),
            ("default", "logout", "logged_out"),
            ("staging", "status", "ready"),
            ("staging", "logout", "logged_out"),
            ("staging", "status", "logged_out"),
        ] {
            assert_eq!(auth_state(tool, profile, action, &config), expected);
        }
        let arguments = [
            vec![
                "ask",
                "auth",
                "status",
                "--profile",
                "staging",
                "--credentials-path",
                alternate.to_str().unwrap(),
            ],
            config_flag.to_vec(),
        ]
        .concat();
        assert_eq!(configured_auth(tool, &arguments)["data"]["status"], "ready");
        assert_eq!(std::fs::read(directory.join(".key")).unwrap(), key);
        assert!(!config.exists());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
