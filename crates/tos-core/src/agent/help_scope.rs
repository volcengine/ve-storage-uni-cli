//! Display only global options consumed by the selected command or its children.

const STORAGE_OPTIONS: &[&str] = &[
    "--auth-mode",
    "--profile",
    "--config-path",
    "--credentials-path",
    "--region",
    "--endpoint",
    "--psm",
    "--idc",
    "--cluster",
    "--addr-family",
    "--control-endpoint",
    "--account-id",
];
const NETWORK_OPTIONS: &[&str] = &[
    "--region",
    "--endpoint",
    "--psm",
    "--idc",
    "--cluster",
    "--addr-family",
    "--control-endpoint",
    "--account-id",
];
const CONFIRMATION_OPTIONS: &[&str] = &["--yes", "--confirm"];

/// Returns display-only exclusions for a canonical product and parsed command tokens.
/// Does not reject arguments, read configuration, or change command execution.
pub fn hidden_options(tool: &str, commands: &[&str]) -> Vec<&'static str> {
    let Some(&command) = commands.first() else {
        return Vec::new();
    };
    let action = commands.get(1).copied();
    let mut hidden = match command {
        "ask" => NETWORK_OPTIONS
            .iter()
            .copied()
            .chain(["--auth-mode"])
            .chain(CONFIRMATION_OPTIONS.iter().copied())
            .collect(),
        "capabilities" | "completion" | "skill" => local_options(command, action),
        "config" => config_options(tool, action),
        "serve" => serve_options(tool),
        "doctor" => vec!["--yes", "--confirm", "--quiet", "--account-id"],
        "api" if tool != "ve-tos" => local_options(command, action),
        "api" => vec!["--yes", "--confirm", "--quiet"],
        "auth" if tool == "ve-adrive" => auth_options(action),
        _ => Vec::new(),
    };
    if command != "serve" {
        hidden.push("--no-color");
    }
    if matches!(command, "ls" | "stat" | "du" | "find" | "cat" | "presign") {
        hidden.extend_from_slice(CONFIRMATION_OPTIONS);
    }
    if tool == "ve-adrive" {
        hide_adrive_options(command, action, &mut hidden);
    }
    hidden.sort_unstable();
    hidden.dedup();
    hidden
}

fn local_options(command: &str, action: Option<&str>) -> Vec<&'static str> {
    let mut hidden = STORAGE_OPTIONS.to_vec();
    hidden.extend(["--yes", "--confirm", "--quiet"]);
    if matches!(command, "capabilities" | "completion")
        || (command == "skill" && matches!(action, Some("list" | "ls")))
    {
        hidden.push("--dry-run");
    }
    hidden
}

fn config_options(tool: &str, action: Option<&str>) -> Vec<&'static str> {
    let mut hidden = NETWORK_OPTIONS.to_vec();
    hidden.extend(["--yes", "--confirm", "--quiet"]);
    if tool == "ve-adrive" || matches!(action, Some("init" | "set")) {
        hidden.push("--auth-mode");
    }
    if action == Some("init") {
        hidden.push("--credentials-path");
    }
    hidden
}

fn serve_options(tool: &str) -> Vec<&'static str> {
    // [Review Fix #17] MCP consumes some globals as child defaults; preserve only actual forwarding.
    let mut hidden = vec!["--psm", "--idc", "--cluster", "--addr-family", "--confirm"];
    // The VeTos raw API MCP tool executes in-process and consumes the parent's account ID.
    if tool != "ve-tos" {
        hidden.push("--account-id");
    }
    if tool != "ve-adrive" {
        hidden.push("--yes");
    }
    if tool != "ve-tos" {
        hidden.extend(["--control-endpoint", "--no-color"]);
    }
    if tool == "ve-adrive" {
        hidden.push("--auth-mode");
    }
    hidden
}

/// Whether a global CLI option applies to a canonical command's discovery schema.
/// Accepts hyphenated CLI names or underscore-separated MCP property names without `--`.
/// Returns a display decision only; existing argument acceptance remains unchanged.
pub fn supports_global_option(command: &str, name: &str) -> bool {
    let mut words = command.split_ascii_whitespace();
    let Some(tool) = words.next() else {
        return true;
    };
    let commands = words.collect::<Vec<_>>();
    let option = format!("--{}", name.replace('_', "-"));
    !hidden_options(tool, &commands).contains(&option.as_str())
}

fn auth_options(action: Option<&str>) -> Vec<&'static str> {
    let mut hidden = vec!["--region", "--yes", "--confirm", "--quiet"];
    if matches!(action, Some("status" | "logout")) {
        hidden.push("--endpoint");
    }
    if action == Some("status") {
        hidden.push("--dry-run");
    }
    hidden
}

fn hide_adrive_options(command: &str, action: Option<&str>, hidden: &mut Vec<&'static str>) {
    hidden.extend([
        "--psm",
        "--idc",
        "--cluster",
        "--addr-family",
        "--control-endpoint",
        "--account-id",
    ]);
    // Ask uses the shared TOS table formatter; ADrive's own formatter ignores verbose.
    if !matches!(command, "ask" | "du" | "serve") {
        hidden.push("--verbose");
    }
    if matches!(command, "cp" | "crt" | "put" | "mkdir" | "file" | "chunk") {
        hidden.extend_from_slice(CONFIRMATION_OPTIONS);
    }
    if matches!(
        command,
        "ls" | "stat" | "crt" | "cat" | "mkdir" | "file" | "chunk"
    ) {
        hidden.push("--quiet");
    }
    if command == "share" {
        hidden.push("--confirm");
        if action.is_some_and(|action| action != "cancel") {
            hidden.push("--yes");
        }
        if matches!(
            action,
            Some("create" | "get" | "update" | "list" | "inspect")
        ) {
            hidden.push("--quiet");
        }
    }
}
