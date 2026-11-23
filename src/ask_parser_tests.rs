use super::Cli;
use clap::Parser;

#[test]
fn ask_all_tools_parse_bare_and_one_question() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for suffix in [vec!["ask"], vec!["ask", "如何上传文件？"]] {
            let mut arguments = vec!["ve-storage-uni-cli", tool];
            arguments.extend(suffix);
            assert!(
                Cli::try_parse_from(arguments).is_ok(),
                "ask missing for {tool}"
            );
        }
    }
}

#[test]
fn doubao_auth_routes_are_nested_under_ask() {
    for tool in ["tos", "ve-tos", "ve-adrive"] {
        for action in ["login", "status", "logout"] {
            assert!(
                Cli::try_parse_from(["ve-storage-uni-cli", tool, "ask", "auth", action]).is_ok(),
                "{tool} auth {action}"
            );
        }
        assert!(Cli::try_parse_from([
            "ve-storage-uni-cli",
            tool,
            "ask",
            "auth",
            "login",
            "--no-browser"
        ])
        .is_ok());
        for flag in ["--login", "--session-id"] {
            assert!(Cli::try_parse_from(["ve-storage-uni-cli", tool, "ask", flag]).is_err());
        }
    }
}

#[test]
fn existing_adrive_auth_still_parses() {
    assert!(Cli::try_parse_from([
        "ve-storage-uni-cli",
        "ve-adrive",
        "auth",
        "login",
        "--instance",
        "inst-1"
    ])
    .is_ok());
    assert!(Cli::try_parse_from(["ve-storage-uni-cli", "ve-adrive", "auth", "status"]).is_ok());
}
