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

//! Integration tests for TOS config commands.
//!
//! Tests validate Agent-Native principles:
//! 1. Discovery (--describe, --help)
//! 2. Understanding (scenario_routing)
//! 3. Safe Execution (--dry-run)
//! 4. Controlled Output (Envelope, --output json/table)
//! 5. Deterministic Errors (structured exit codes, error envelope)
//! 6. Agent Ecosystem (machine-readable JSON, consistent schema)
//!
//! Updated for design doc 7.3 layered config model:
//!   [profile] shared + [profile.<binary>] override,
//!   secrets encrypted on disk as ENC:..., show annotates source section.

use std::process::Command;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
    time::Duration,
};

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(args)
        .output()
        .expect("Failed to execute ve-storage-uni-cli")
}

/// Run CLI with a temporary HOME so config writes are isolated.
fn cli_with_home(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    cli_with_home_and_env(home, args, &[])
}

fn cli_with_home_and_env(
    home: &std::path::Path,
    args: &[&str],
    envs: &[(&str, &std::ffi::OsStr)],
) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"));
    command.env("HOME", home.as_os_str());
    // [Review Fix #6] Keep config-file isolation tests independent from the
    // developer or CI shell's ADrive credentials and endpoint overrides.
    for key in [
        "TOS_REGION",
        "TOS_ENDPOINT",
        "TOS_CONFIG_PATH",
        "TOS_CREDENTIALS_PATH",
        "TOS_CONTROL_ENDPOINT",
        "TOS_ACCESS_KEY",
        "TOS_SECRET_KEY",
        "TOS_SECURITY_TOKEN",
        "TOS_ACCOUNT_ID",
        "TOS_CHECKPOINT_DIR",
        "TOS_BATCH_REPORT_DIR",
        "TOS_BATCH_REPORT_FORMAT",
        "TOS_PROGRESS_ENABLED",
        "TOS_MAX_RETRY_COUNT",
        "TOS_REQUESTTIMEOUT",
        "TOS_REQUEST_TIMEOUT",
        "TOS_CONNECTTIMEOUT",
        "TOS_CONNECT_TIMEOUT",
        "TOS_MAXCONNECTIONS",
        "TOS_MAX_CONNECTIONS",
        "BYTE_TOS_REGION",
        "BYTE_TOS_ENDPOINT",
        "BYTE_TOS_PSM",
        "BYTE_TOS_IDC",
        "BYTE_TOS_CLUSTER",
        "BYTE_TOS_ADDR_FAMILY",
        "BYTE_TOS_CONTROL_ENDPOINT",
        "BYTE_TOS_ACCESS_KEY",
        "BYTE_TOS_SECRET_KEY",
        "BYTE_TOS_SECURITY_TOKEN",
        "BYTE_TOS_ACCOUNT_ID",
        "BYTE_TOS_CHECKPOINT_DIR",
        "BYTE_TOS_BATCH_REPORT_DIR",
        "BYTE_TOS_BATCH_REPORT_FORMAT",
        "BYTE_TOS_PROGRESS_ENABLED",
        "BYTE_TOS_MAX_RETRY_COUNT",
        "BYTE_TOS_REQUESTTIMEOUT",
        "BYTE_TOS_REQUEST_TIMEOUT",
        "BYTE_TOS_CONNECTTIMEOUT",
        "BYTE_TOS_CONNECT_TIMEOUT",
        "BYTE_TOS_MAXCONNECTIONS",
        "BYTE_TOS_MAX_CONNECTIONS",
        "TEST_TOSAPI_ADDR",
        "TOS_FORCE_PSM",
        "ADRIVE_REGION",
        "ADRIVE_ENDPOINT",
        "ADRIVE_ACCESS_KEY",
        "ADRIVE_SECRET_KEY",
        "ADRIVE_SECURITY_TOKEN",
        "ADRIVE_ACCOUNT_ID",
        "ADRIVE_AUTH_MODE",
        "ADRIVE_AUTH_ENDPOINT",
        "ADRIVE_DEFAULT_INSTANCE",
        "ADRIVE_DEVICE_NAME",
        "ADRIVE_OAUTH_CLIENT_ID",
        "ADRIVE_ACCESS_TOKEN",
        "ADRIVE_REFRESH_TOKEN",
    ] {
        command.env_remove(key);
    }
    for (key, value) in envs {
        command.env(key, value);
    }
    command
        .args(args)
        .output()
        .expect("Failed to execute ve-storage-uni-cli")
}

fn parse_json(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        return v;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(stderr.trim()) {
        return v;
    }
    panic!("No valid JSON.\nstdout: {}\nstderr: {}", stdout, stderr);
}

fn parse_data_json(output: &std::process::Output) -> serde_json::Value {
    let json = parse_json(output);
    if json["status"] == "success" && json.get("data").is_some() {
        json["data"].clone()
    } else {
        json
    }
}

/// Pick the effective profile object by name from a config show JSON response.
fn find_profile<'a>(json: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    let arr = json["data"]["profiles"].as_array().expect("profiles array");
    arr.iter()
        .find(|p| p["profile_name"] == name)
        .unwrap_or_else(|| panic!("profile '{}' not found in: {}", name, json))
}

// ==========================================================================
// Principle 1: Discovery
// ==========================================================================

#[test]
fn test_config_help() {
    let output = cli(&["ve-tos", "config", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("init"));
    assert!(stdout.contains("show"));
    assert!(stdout.contains("set"));
    assert!(stdout.contains("ve-storage-uni-cli ve-tos config set control_endpoint"));
}

#[test]
fn test_config_init_help_explains_profile() {
    let output = cli(&["ve-tos", "config", "init", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--profile"));
    assert!(stdout.contains("shared section `[profile]`"));
    assert!(stdout.contains("`[profile.ve-tos]`"));
}

#[test]
fn test_config_show_help_explains_sources() {
    let output = cli(&["ve-tos", "config", "show", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("annotates where each value comes from"));
    assert!(stdout.contains("derived from endpoint"));
}

#[test]
fn test_config_set_help_lists_supported_keys() {
    let output = cli(&["ve-tos", "config", "set", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("control_endpoint"));
    assert!(stdout.contains("ve-tos only"));
    assert!(stdout.contains("account_id"));
    assert!(stdout.contains("[<profile>.ve-tos]"));
    assert!(stdout.contains("staging.control_endpoint"));
    assert!(stdout.contains("active TOS binary credentials"));
    assert!(stdout.contains("explicitly write shared credentials"));
}

#[test]
fn test_tos_config_set_help_only_lists_byte_tos_keys() {
    let output = cli(&["tos", "config", "set", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // [Review Fix #16] ByteTOS help should contain copyable supported examples.
    assert!(!stdout.contains("control_endpoint"));
    assert!(!stdout.contains("ve-tos"));
    assert!(stdout.contains("[active-profile.tos]"));
    assert!(stdout.contains("auth_mode (aksk or zti)"));
}

#[test]
fn test_adrive_config_set_help_explains_credential_section() {
    let output = cli(&["ve-adrive", "config", "set", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("[active-profile.adrive]"));
    assert!(stdout.contains("credentials.toml"));
}

#[test]
fn test_root_help_tos_config_set_lists_supported_keys() {
    let output = cli(&["help", "ve-tos", "config", "set"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Supported KEY values"));
    assert!(stdout.contains("control_endpoint"));
    assert!(stdout.contains("account_id"));
}

#[test]
fn test_config_init_describe() {
    let output = cli(&["--describe", "ve-tos", "config", "init"]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    assert_eq!(json["command"], "ve-tos config init");
    assert_eq!(json["layer"], "meta");
    assert_eq!(json["supports_dry_run"], true);
    assert!(json["scenario_routing"].is_object());
}

#[test]
fn test_config_show_describe() {
    let output = cli(&["--describe", "ve-tos", "config", "show"]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    assert_eq!(json["command"], "ve-tos config show");
    assert_eq!(json["supports_pipe"], true);
}

#[test]
fn test_config_set_describe() {
    let output = cli(&[
        "--describe",
        "ve-tos",
        "config",
        "set",
        "region",
        "cn-beijing",
    ]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    assert_eq!(json["command"], "ve-tos config set");
    assert_eq!(json["supports_dry_run"], true);
    assert!(json["description"]
        .as_str()
        .unwrap_or_default()
        .contains("active profile's ve-tos credentials"));
}

// ==========================================================================
// Principle 2: Understanding — scenario_routing
// ==========================================================================

#[test]
fn test_config_init_scenario_routing() {
    let output = cli(&["--describe", "ve-tos", "config", "init"]);
    let json = parse_data_json(&output);
    let routing = serde_json::to_string(&json["scenario_routing"]).unwrap();
    assert!(
        routing.contains("--profile"),
        "Should show named profile example"
    );
}

#[test]
fn test_config_set_scenario_routing() {
    let output = cli(&["--describe", "ve-tos", "config", "set", "k", "v"]);
    let json = parse_data_json(&output);
    let routing = serde_json::to_string(&json["scenario_routing"]).unwrap();
    // New schema documents all three key shapes; at least one named example expected.
    assert!(
        routing.contains("staging.") || routing.contains("default.ve-tos."),
        "Should show layered-section example in scenario_routing: {}",
        routing
    );
}

// ==========================================================================
// Principle 3: Safe Execution — --dry-run
// ==========================================================================

#[test]
fn test_config_init_dry_run() {
    let output = cli(&["--dry-run", "--output", "json", "ve-tos", "config", "init"]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["action"], "config init");
    assert!(json["plan"].is_array());
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(plan_str.contains("config.toml"));
}

#[test]
fn test_config_init_dry_run_named_profile() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "config",
        "init",
        "--profile",
        "staging",
    ]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(
        plan_str.contains("staging"),
        "Should mention the profile name"
    );
}

#[test]
fn test_config_init_dry_run_uses_global_profile_in_confirm_command() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "--profile",
        "dev",
        "ve-tos",
        "config",
        "init",
    ]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(plan_str.contains("[dev]"), "plan={plan_str}");
    assert_eq!(
        json["confirm_command"].as_str().unwrap_or_default(),
        "ve-storage-uni-cli ve-tos config init --profile dev"
    );
}

#[test]
fn test_config_set_dry_run() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "config",
        "set",
        "region",
        "cn-shanghai",
    ]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    assert_eq!(json["dry_run"], true);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(plan_str.contains("default"));
    assert!(plan_str.contains("region"));
    assert!(plan_str.contains("cn-shanghai"));
}

#[test]
fn test_config_set_dry_run_with_profile_prefix() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "config",
        "set",
        "staging.endpoint",
        "https://custom.com",
    ]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(plan_str.contains("staging"));
    assert!(plan_str.contains("endpoint"));
}

#[test]
fn test_config_set_dry_run_uses_global_profile_for_bare_key() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "config",
        "set",
        "endpoint",
        "https://custom.com",
        "--profile",
        "dev",
    ]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(plan_str.contains("[dev.ve-tos]"), "plan={plan_str}");
    assert!(
        json["confirm_command"]
            .as_str()
            .unwrap_or_default()
            .contains("--profile dev"),
        "json={json}"
    );
}

#[test]
fn test_config_set_dry_run_binary_override() {
    // New: layered key `default.ve-tos.endpoint`
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "config",
        "set",
        "default.ve-tos.endpoint",
        "tos-cn-beijing.volces.com",
    ]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(
        plan_str.contains("[default.ve-tos]") || plan_str.contains("default.ve-tos"),
        "Plan should reference [default.ve-tos] section: {}",
        plan_str
    );
}

#[test]
fn test_config_set_control_endpoint_dry_run() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "config",
        "set",
        "control_endpoint",
        "tos-control-cn-beijing.volces.com",
    ]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(
        plan_str.contains("[default.ve-tos]") || plan_str.contains("default.ve-tos"),
        "Plan should reference [default.ve-tos] section: {}",
        plan_str
    );
    assert!(plan_str.contains("control_endpoint"));
}

#[test]
fn test_adrive_config_init_dry_run_does_not_write_file() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "config",
            "init",
            "--profile",
            "dev",
        ],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_data_json(&output);
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["action"], "config init");
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(plan_str.contains("[dev.adrive]"), "plan={plan_str}");
    assert!(
        !tmp.join(".tos").join("config.toml").exists(),
        "ve-adrive config init --dry-run must not create config.toml"
    );
}

#[test]
fn test_adrive_config_set_dry_run_routes_bare_key_and_redacts_secret() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--dry-run",
            "--output",
            "json",
            "--profile",
            "dev",
            "ve-adrive",
            "config",
            "set",
            "secret_access_key",
            "RAW_SECRET_SHOULD_NOT_LEAK",
        ],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("RAW_SECRET_SHOULD_NOT_LEAK"),
        "dry-run output must redact secret: {stdout}"
    );
    let json = parse_data_json(&output);
    assert_eq!(json["dry_run"], true);
    let plan_str = serde_json::to_string(&json["plan"]).unwrap();
    assert!(plan_str.contains("[dev.adrive]"), "plan={plan_str}");
    assert!(plan_str.contains("credentials.toml"), "plan={plan_str}");
    assert!(plan_str.contains("****"), "plan={plan_str}");
    assert!(
        !tmp.join(".tos").join("config.toml").exists(),
        "ve-adrive config set --dry-run must not create config.toml"
    );
}

// ==========================================================================
// Principle 4: Controlled Output — Envelope, init/show/set E2E
// ==========================================================================

#[test]
fn test_config_init_creates_file() {
    let tmp = tempdir();
    let output = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "init"]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["status"], "success");
    assert_eq!(json["command"], "ve-tos config init");
    assert!(json["data"]["config_path"].is_string());
    assert_eq!(json["data"]["profile"], "default");
    let config_path = tmp.join(".tos").join("config.toml");
    assert!(config_path.exists(), "Config file should be created");
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("[default]"));
    assert!(content.contains("region = \"cn-beijing\""));
    // Layered: ve-tos init should also write a [default.ve-tos] section
    assert!(
        content.contains("[default.ve-tos]"),
        "Layered config should have [default.ve-tos] section: {}",
        content
    );
    assert!(
        content.contains("endpoint = \"tos-cn-beijing.volces.com\""),
        "ve-tos init must keep the production endpoint default: {content}"
    );
}

#[test]
fn test_tos_config_init_omits_network_defaults() {
    let tmp = tempdir();
    let output = cli_with_home(&tmp, &["--output", "json", "tos", "config", "init"]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let content = std::fs::read_to_string(tmp.join(".tos").join("config.toml")).unwrap();
    assert!(content.contains("[default.tos]"), "content={content}");
    for field in ["region", "endpoint", "psm"] {
        assert!(
            !content
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("{field} ="))),
            "tos init must not write {field}: {content}"
        );
    }
}

#[test]
fn test_tos_config_init_dry_run_omits_network_defaults() {
    let output = cli(&["--dry-run", "--output", "json", "tos", "config", "init"]);
    assert!(output.status.success());
    let json = parse_data_json(&output);
    let plan = serde_json::to_string(&json["plan"]).unwrap();

    assert!(!plan.contains("placeholder region"), "plan={plan}");
    assert!(!plan.contains("default endpoint"), "plan={plan}");
    assert!(plan.contains("network settings omitted"), "plan={plan}");
}

#[test]
fn test_config_path_global_option_overrides_default_config_file() {
    let tmp = tempdir();
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).expect("create isolated home");
    let config_path = tmp.join("custom").join("tos-config.toml");
    let config_path_arg = config_path.to_string_lossy().into_owned();
    let default_config_path = home.join(".tos").join("config.toml");

    let init_output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "--config-path",
            config_path_arg.as_str(),
            "ve-tos",
            "config",
            "init",
        ],
    );
    assert!(
        init_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&init_output.stderr)
    );
    let init_json = parse_json(&init_output);
    assert_eq!(init_json["data"]["config_path"], config_path_arg);
    assert!(config_path.exists(), "custom config file should be created");
    assert!(
        !default_config_path.exists(),
        "global --config-path must not write the default config file"
    );

    let set_output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "--config-path",
            config_path_arg.as_str(),
            "ve-tos",
            "config",
            "set",
            "region",
            "cn-shanghai",
        ],
    );
    assert!(
        set_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&set_output.stderr)
    );
    let set_json = parse_json(&set_output);
    assert_eq!(set_json["data"]["config_path"], config_path_arg);

    let show_output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "--config-path",
            config_path_arg.as_str(),
            "ve-tos",
            "config",
            "show",
        ],
    );
    assert!(
        show_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&show_output.stderr)
    );
    let show_json = parse_json(&show_output);
    assert_eq!(show_json["data"]["config_path"], config_path_arg);
    let default = find_profile(&show_json, "default");
    assert_eq!(default["region"]["value"], "cn-shanghai");
}

#[test]
fn test_config_path_env_overrides_default_config_file() {
    let tmp = tempdir();
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).expect("create isolated home");
    let config_path = tmp.join("env").join("tos-config.toml");
    let config_path_arg = config_path.to_string_lossy().into_owned();
    let default_config_path = home.join(".tos").join("config.toml");

    let output = cli_with_home_and_env(
        &home,
        &["--output", "json", "ve-tos", "config", "init"],
        &[("TOS_CONFIG_PATH", config_path.as_os_str())],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_json(&output);
    assert_eq!(json["data"]["config_path"], config_path_arg);
    assert!(config_path.exists(), "env config file should be created");
    assert!(
        !default_config_path.exists(),
        "TOS_CONFIG_PATH must not write the default config file"
    );
}

#[test]
fn test_config_path_uses_custom_key_dir_for_encrypted_values() {
    let tmp = tempdir();
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).expect("create isolated home");
    let config_path = tmp.join("secure").join("tos-config.toml");
    let config_path_arg = config_path.to_string_lossy().into_owned();

    let set_output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "--config-path",
            config_path_arg.as_str(),
            "ve-tos",
            "config",
            "set",
            "secret_access_key",
            "CUSTOM_SECRET",
        ],
    );
    assert!(
        set_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&set_output.stderr)
    );

    let show_output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "--config-path",
            config_path_arg.as_str(),
            "ve-tos",
            "config",
            "show",
        ],
    );
    assert!(
        show_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&show_output.stderr)
    );
    let show_json = parse_json(&show_output);
    let default = find_profile(&show_json, "default");
    assert_eq!(default["secret_access_key"]["value"], "****CRET");
}

#[test]
fn test_high_level_ls_config_path_missing_does_not_fallback_to_default_config() {
    let tmp = tempdir();
    let home = tmp.join("home");
    let default_config_dir = home.join(".tos");
    std::fs::create_dir_all(&default_config_dir).expect("create default config dir");
    std::fs::write(
        default_config_dir.join("config.toml"),
        r#"
[default]
region = "cn-beijing"
access_key_id = "ak"
secret_access_key = "sk"

[default.ve-tos]
endpoint = "http://127.0.0.1:9"
requesttimeout = 1
connecttimeout = 1
"#,
    )
    .expect("write default config");
    let missing_config_path = tmp.join("missing").join("config.toml");
    let missing_config_arg = missing_config_path.to_string_lossy().into_owned();

    let output = cli_with_home(
        &home,
        &[
            "ve-tos",
            "ls",
            "--config-path",
            missing_config_arg.as_str(),
            "--output",
            "json",
        ],
    );
    assert!(
        !output.status.success(),
        "ls should fail before using the default config"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("No config file found at") && stderr.contains(&missing_config_arg),
        "ls should fail with the missing custom config path instead of continuing with fallback config sources: {stderr}"
    );
    assert!(
        !stderr.contains("127.0.0.1"),
        "ls appears to have used endpoint from the default config: {stderr}"
    );
}

#[test]
fn test_config_init_named_profile() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "init",
            "--profile",
            "prod",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["profile"], "prod");
    let config_path = tmp.join(".tos").join("config.toml");
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(content.contains("[prod]"));
    assert!(content.contains("[prod.ve-tos]"));
}

#[test]
fn test_tos_config_init_uses_global_profile_when_local_profile_missing() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "--profile",
            "dev",
            "ve-tos",
            "config",
            "init",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["profile"], "dev");
    let content = std::fs::read_to_string(tmp.join(".tos").join("config.toml")).unwrap();
    assert!(content.contains("[dev]"), "content={content}");
    assert!(content.contains("[dev.ve-tos]"), "content={content}");
    assert!(
        !content.contains("[default.ve-tos]"),
        "global --profile dev should not initialize default.ve-tos: {content}"
    );
}

#[test]
fn test_config_show_json_envelope() {
    let tmp = tempdir();
    cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "init"]);
    let output = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["status"], "success");
    assert_eq!(json["command"], "ve-tos config show");
    // New schema: data.profiles is an array of EffectiveProfile entries
    assert!(json["data"]["profiles"].is_array());
    let default = find_profile(&json, "default");
    assert_eq!(default["binary"], "ve-tos");
    // Each field carries {value, source}
    assert!(default["region"]["value"].is_string());
    assert!(default["region"]["source"].is_string());
    assert!(default["control_endpoint"]["value"].is_string());
    assert_eq!(default["control_endpoint"]["source"], "Derived");
}

#[test]
fn test_tos_and_ve_tos_config_namespaces_are_isolated() {
    let tmp = tempdir();
    let byted_set = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "tos",
            "config",
            "set",
            "endpoint",
            "tos-cn-north.byted.org",
        ],
    );
    assert!(
        byted_set.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&byted_set.stderr)
    );
    let ve_set = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "endpoint",
            "tos-cn-beijing.volces.com",
        ],
    );
    assert!(
        ve_set.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&ve_set.stderr)
    );

    let content = std::fs::read_to_string(tmp.join(".tos").join("config.toml")).unwrap();
    assert!(content.contains("[default.tos]"), "content={content}");
    assert!(content.contains("[default.ve-tos]"), "content={content}");

    let byted_show = cli_with_home(&tmp, &["--output", "json", "tos", "config", "show"]);
    assert!(byted_show.status.success());
    let byted_json = parse_json(&byted_show);
    let byted_default = find_profile(&byted_json, "default");
    assert_eq!(byted_default["binary"], "tos");
    assert_eq!(byted_default["endpoint"]["value"], "tos-cn-north.byted.org");
    assert!(
        byted_default.get("control_endpoint").is_none(),
        "tos config show must not expose ve-tos-only control_endpoint: {byted_default}"
    );

    let ve_show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    assert!(ve_show.status.success());
    let ve_json = parse_json(&ve_show);
    let ve_default = find_profile(&ve_json, "default");
    assert_eq!(ve_default["binary"], "ve-tos");
    assert_eq!(ve_default["endpoint"]["value"], "tos-cn-beijing.volces.com");
    assert_eq!(
        ve_default["control_endpoint"]["value"],
        "tos-control-cn-beijing.volces.com"
    );
}

#[test]
fn test_tos_config_set_and_show_psm_fields() {
    let tmp = tempdir();
    for (key, value) in [
        ("psm", "toutiao.tos.tosapi"),
        ("idc", "lf"),
        ("cluster", "default"),
        ("addr_family", "v4"),
    ] {
        let output = cli_with_home(
            &tmp,
            &["--output", "json", "tos", "config", "set", key, value],
        );
        assert!(
            output.status.success(),
            "tos config set {key} failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let content = std::fs::read_to_string(tmp.join(".tos").join("config.toml")).unwrap();
    assert!(content.contains("[default.tos]"), "content={content}");
    assert!(
        content.contains("psm = \"toutiao.tos.tosapi\""),
        "content={content}"
    );
    assert!(content.contains("idc = \"lf\""), "content={content}");
    assert!(
        content.contains("cluster = \"default\""),
        "content={content}"
    );
    assert!(
        content.contains("addr_family = \"v4\""),
        "content={content}"
    );

    let show = cli_with_home(&tmp, &["--output", "json", "tos", "config", "show"]);
    assert!(show.status.success());
    let json = parse_json(&show);
    let default = find_profile(&json, "default");
    assert_eq!(default["psm"]["value"], "toutiao.tos.tosapi");
    assert_eq!(default["idc"]["value"], "lf");
    assert_eq!(default["cluster"]["value"], "default");
    assert_eq!(default["addr_family"]["value"], "v4");

    let ve_show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    assert!(ve_show.status.success());
    let ve_json = parse_json(&ve_show);
    assert!(
        ve_json["data"]["profiles"]
            .as_array()
            .expect("profiles")
            .iter()
            .all(|profile| profile["profile_name"] != "default"),
        "ve-tos must skip tos-only PSM profile: {ve_json}"
    );
}

#[test]
fn test_ve_tos_config_set_rejects_psm_fields() {
    let tmp = tempdir();
    for key in [
        "psm",
        "idc",
        "cluster",
        "addr_family",
        "default.tos.psm",
        "default.tos.addr_family",
    ] {
        let output = cli_with_home(
            &tmp,
            &["--output", "json", "ve-tos", "config", "set", key, "value"],
        );
        assert!(
            !output.status.success(),
            "ve-tos config set {key} must fail"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("PSM config fields are only supported by tos"),
            "stderr={stderr}"
        );
    }
}

#[test]
fn test_ve_tos_config_show_skips_unrelated_tos_namespace_profiles() {
    let tmp = tempdir();
    let config_dir = tmp.join(".tos");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[default]
region = "cn-beijing"
access_key_id = "ak"
secret_access_key = "sk"

[default.ve-tos]
endpoint = "tos-cn-beijing.volces.com"

[dev]
region = "cn-beijing"
access_key_id = "ak"
secret_access_key = "sk"

[dev.tos]
endpoint = "tos-cn-north.byted.org"
"#,
    )
    .expect("write config");

    let output = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_json(&output);
    let default = find_profile(&json, "default");
    assert_eq!(default["binary"], "ve-tos");
    assert_eq!(default["endpoint"]["value"], "tos-cn-beijing.volces.com");
    assert!(
        json["data"]["profiles"]
            .as_array()
            .expect("profiles")
            .iter()
            .all(|profile| profile["profile_name"] != "dev"),
        "tos-only dev profile must not block or appear in ve-tos config show: {json}"
    );
}

#[test]
fn test_ve_tos_rejects_profile_with_only_tos_namespace() {
    let tmp = tempdir();
    let config_dir = tmp.join(".tos");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[dev]
access_key_id = "ak"
secret_access_key = "sk"

[dev.tos]
endpoint = "tos-cn-north.byted.org"
"#,
    )
    .expect("write config");

    let ve_show = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "--profile",
            "dev",
            "ve-tos",
            "presign",
            "tos://bucket/key",
        ],
    );
    assert!(!ve_show.status.success());
    let stderr = String::from_utf8_lossy(&ve_show.stderr);
    assert!(
        stderr.contains("will not consume the `tos` namespace"),
        "stderr={stderr}"
    );

    let byted_show = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "--profile",
            "dev",
            "tos",
            "config",
            "show",
        ],
    );
    assert!(
        byted_show.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&byted_show.stderr)
    );
    let byted_json = parse_json(&byted_show);
    let dev = find_profile(&byted_json, "dev");
    assert_eq!(dev["binary"], "tos");
    assert_eq!(dev["endpoint"]["value"], "tos-cn-north.byted.org");
}

#[test]
fn test_ve_tos_runtime_uses_tos_env_even_when_tos_namespace_exists() {
    let tmp = tempdir();
    let config_dir = tmp.join(".tos");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[default]
access_key_id = "ak"
secret_access_key = "sk"

[default.tos]
endpoint = "tos-cn-north.byted.org"
"#,
    )
    .expect("write config");

    let mut ve_command = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"));
    ve_command
        .env("HOME", &tmp)
        .env("TOS_REGION", "cn-boe")
        .env("TOS_ENDPOINT", "tos-cn-boe.volces.com")
        .env("TOS_ACCESS_KEY", "env-ak")
        .env("TOS_SECRET_KEY", "env-sk")
        .args(["--dry-run", "--output", "json", "ve-tos", "ls"]);
    let ve_output = ve_command.output().expect("run ve-tos ls");
    assert!(
        ve_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&ve_output.stderr)
    );
}

#[test]
fn test_byted_tos_runtime_uses_byte_tos_env_not_tos_env() {
    let tmp = tempdir();

    let mut tos_env_only = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"));
    tos_env_only
        .env("HOME", &tmp)
        .env("TOS_REGION", "cn-boe")
        .env("TOS_ENDPOINT", "tos-cn-boe.volces.com")
        .env("TOS_ACCESS_KEY", "env-ak")
        .env("TOS_SECRET_KEY", "env-sk")
        .env_remove("BYTE_TOS_REGION")
        .env_remove("BYTE_TOS_ENDPOINT")
        .env_remove("BYTE_TOS_ACCESS_KEY")
        .env_remove("BYTE_TOS_SECRET_KEY")
        .args(["--output", "json", "tos", "presign", "tos://bucket/key"]);
    let tos_env_output = tos_env_only.output().expect("run tos presign");
    assert!(!tos_env_output.status.success());
    let stderr = String::from_utf8_lossy(&tos_env_output.stderr);
    assert!(
        stderr.contains("endpoint or psm is required")
            || stderr.contains("region is required")
            || stderr.contains("access_key_id is required"),
        "stderr={stderr}"
    );
    let error = parse_json(&tos_env_output);
    assert_eq!(
        error["error"]["fix_command"],
        "tos config set endpoint <endpoint>"
    );

    let mut byte_env = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"));
    byte_env
        .env("HOME", &tmp)
        .env("TOS_REGION", "cn-wrong")
        .env("TOS_ENDPOINT", "tos-wrong.volces.com")
        .env("TOS_ACCESS_KEY", "wrong-ak")
        .env("TOS_SECRET_KEY", "wrong-sk")
        .env("BYTE_TOS_REGION", "cn-boe")
        .env("BYTE_TOS_ENDPOINT", "tos-cn-boe.volces.com")
        .env("BYTE_TOS_ACCESS_KEY", "byte-ak")
        .env("BYTE_TOS_SECRET_KEY", "byte-sk")
        .args(["--output", "json", "tos", "presign", "tos://bucket/key"]);
    let byte_output = byte_env.output().expect("run tos presign with byte env");
    assert!(
        byte_output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&byte_output.stderr)
    );
    let byte_json = parse_json(&byte_output);
    let url = byte_json["data"]["url"].as_str().expect("presigned url");
    assert!(url.contains("tos-algorithm=TOS-HMAC-SHA256"), "url={url}");
    assert!(url.contains("tos-signature="), "url={url}");
    assert!(
        !url.contains("X-Tos-Algorithm=TOS4-HMAC-SHA256"),
        "url={url}"
    );
}

#[test]
fn test_byted_tos_doctor_prefers_psm_over_byte_tos_endpoint_env() {
    let tmp = tempdir();
    let output = cli_with_home_and_env(
        &tmp,
        &["--output", "json", "tos", "doctor", "--check", "endpoint"],
        &[
            ("BYTE_TOS_REGION", std::ffi::OsStr::new("cn-beijing")),
            (
                "BYTE_TOS_ENDPOINT",
                std::ffi::OsStr::new("tos-cn-north-boe.byted.org"),
            ),
            ("BYTE_TOS_PSM", std::ffi::OsStr::new("toutiao.tos.tosapi")),
            ("BYTE_TOS_ADDR_FAMILY", std::ffi::OsStr::new("dual-stack")),
        ],
    );

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed = parse_json(&output);
    let details = &parsed["data"]["checks"][0]["details"];
    assert_eq!(details["endpoint"], serde_json::Value::Null);
    assert_eq!(details["endpoint_mode_active"], false);
    assert_eq!(details["has_explicit_endpoint"], false);
    assert_eq!(details["has_psm"], true);
    assert_eq!(details["psm"], "toutiao.tos.tosapi");
    assert_eq!(details["addr_family"], "dual-stack");
}

#[test]
fn test_byted_tos_doctor_accepts_env_only_psm_without_config() {
    let tmp = tempdir();
    let output = cli_with_home_and_env(
        &tmp,
        &["--output", "json", "tos", "doctor", "--check", "endpoint"],
        &[("BYTE_TOS_PSM", std::ffi::OsStr::new("toutiao.tos.tosapi"))],
    );

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed = parse_json(&output);
    let check = &parsed["data"]["checks"][0];
    assert_eq!(check["status"], "passed");
    assert_eq!(check["details"]["has_psm"], true);
    assert_eq!(check["details"]["has_region"], false);

    let config_output = cli_with_home_and_env(
        &tmp,
        &["--output", "json", "tos", "doctor", "--check", "config"],
        &[("BYTE_TOS_PSM", std::ffi::OsStr::new("toutiao.tos.tosapi"))],
    );
    assert!(config_output.status.success());
    let config_check = &parse_json(&config_output)["data"]["checks"][0];
    assert_eq!(config_check["status"], "passed");
    assert_eq!(config_check["details"]["has_region"], false);
}

#[test]
fn test_byted_tos_doctor_requires_region_for_endpoint_override_of_psm() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--endpoint",
            "https://private.example.com",
            "--output",
            "json",
            "tos",
            "doctor",
            "--check",
            "config",
        ],
        &[("BYTE_TOS_PSM", std::ffi::OsStr::new("toutiao.tos.tosapi"))],
    );
    assert!(output.status.success());
    let check = &parse_json(&output)["data"]["checks"][0];
    assert_eq!(check["status"], "warning");
    assert_eq!(check["details"]["has_endpoint"], true);
    assert_eq!(check["details"]["has_psm"], false);
    assert_eq!(check["details"]["has_region"], false);
}

#[test]
fn test_byted_tos_runtime_prefers_psm_over_byte_tos_endpoint_env() {
    let tmp = tempdir();
    let output = cli_with_home_and_env(
        &tmp,
        &["--output", "json", "tos", "ls", "tos://dms-agent-boe"],
        &[
            ("BYTE_TOS_REGION", std::ffi::OsStr::new("cn-beijing")),
            (
                "BYTE_TOS_ENDPOINT",
                std::ffi::OsStr::new("tos-cn-north-boe.byted.org"),
            ),
            ("BYTE_TOS_PSM", std::ffi::OsStr::new("toutiao.tos.tosapi")),
            ("BYTE_TOS_ACCESS_KEY", std::ffi::OsStr::new("ak")),
            ("BYTE_TOS_SECRET_KEY", std::ffi::OsStr::new("sk")),
            ("BYTE_TOS_MAX_RETRY_COUNT", std::ffi::OsStr::new("0")),
            ("TEST_TOSAPI_ADDR", std::ffi::OsStr::new("127.0.0.1:1")),
        ],
    );

    assert!(!output.status.success(), "stdout should not succeed");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("http://127.0.0.1:1/dms-agent-boe"),
        "stderr={stderr}"
    );
    assert!(
        !stderr.contains("tos-cn-north-boe.byted.org"),
        "stderr={stderr}"
    );
}

fn capture_first_consul_lookup(listener: TcpListener) -> String {
    listener
        .set_nonblocking(true)
        .expect("make Consul mock nonblocking");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "Consul lookup timed out"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept Consul lookup: {error}"),
        }
    };
    // [Review Fix #18] TCP may split the query across reads; capture the complete headers.
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set Consul mock read timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let length = stream.read(&mut buffer).expect("read Consul lookup");
        assert!(length > 0, "Consul lookup ended before request headers");
        request.extend_from_slice(&buffer[..length]);
        assert!(
            request.len() <= 16 * 1024,
            "Consul lookup headers too large"
        );
    }
    // An empty service list lets the CLI exit after its first lookup.
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]")
        .expect("reply to Consul lookup");
    String::from_utf8_lossy(&request).to_string()
}

fn first_consul_lookup_with_force_psm(force_psm_value: Option<&str>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local Consul mock");
    let address = listener
        .local_addr()
        .expect("Consul mock address")
        .to_string();
    let server = thread::spawn(move || capture_first_consul_lookup(listener));
    let home = tempdir();
    let mut environment = vec![
        ("BYTE_TOS_REGION", std::ffi::OsStr::new("cn-beijing")),
        ("BYTE_TOS_ACCESS_KEY", std::ffi::OsStr::new("test-ak")),
        ("BYTE_TOS_SECRET_KEY", std::ffi::OsStr::new("test-sk")),
        ("BYTE_TOS_MAX_RETRY_COUNT", std::ffi::OsStr::new("0")),
        ("CONSUL_HTTP_ADDR", std::ffi::OsStr::new(&address)),
    ];
    if let Some(value) = force_psm_value {
        environment.push(("TOS_FORCE_PSM", std::ffi::OsStr::new(value)));
    }
    let output = cli_with_home_and_env(
        &home,
        &[
            "tos",
            "--psm",
            "forced.example.psm",
            "ls",
            "tos://example-bucket",
        ],
        &environment,
    );
    assert!(
        !output.status.success(),
        "mock returned no service endpoints"
    );
    server.join().expect("capture Consul lookup")
}

#[test]
fn force_psm_queries_selected_service_without_bns() {
    let forced_request = first_consul_lookup_with_force_psm(Some("true"));
    assert!(
        forced_request.contains("name=forced.example.psm"),
        "{forced_request}"
    );
    let default_request = first_consul_lookup_with_force_psm(None);
    assert!(
        default_request.contains("name=tos.access.bns"),
        "{default_request}"
    );
    let disabled_request = first_consul_lookup_with_force_psm(Some("false"));
    assert!(
        disabled_request.contains("name=tos.access.bns"),
        "{disabled_request}"
    );
}

#[test]
fn byte_tos_psm_help_documents_force_mode() {
    for command in [
        vec!["tos", "--help"],
        vec!["tos", "ls", "--help"],
        vec!["tos", "--help", "--help-language", "zh"],
    ] {
        let output = cli(&command);
        assert!(output.status.success(), "{command:?}");
        let help = String::from_utf8_lossy(&output.stdout);
        assert!(help.contains("TOS_FORCE_PSM=true"), "{command:?}: {help}");
    }
}

#[test]
fn test_adrive_config_show_does_not_inherit_shared_settings() {
    let tmp = tempdir();
    let config_dir = tmp.join(".tos");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[default]
region = "cn-beijing"
endpoint = "tos-cn-beijing.volces.com"
access_key_id = "AK_FOR_TOS_ONLY"
secret_access_key = "SK_FOR_TOS_ONLY"
security_token = "TOKEN_FOR_TOS_ONLY"
"#,
    )
    .expect("write config");

    let output = cli_with_home(&tmp, &["--output", "json", "ve-adrive", "config", "show"]);
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["status"], "success");
    assert_eq!(json["command"], "ve-adrive config show");
    let default = find_profile(&json, "default");
    assert_eq!(default["binary"], "adrive");
    assert_eq!(default["region"]["source"], "Unset");
    assert_eq!(default["region"]["value"], serde_json::Value::Null);
    assert_eq!(default["endpoint"]["source"], "Unset");
    assert_eq!(default["endpoint"]["value"], serde_json::Value::Null);
    assert_eq!(default["access_key_id"]["source"], "Unset");
    assert_eq!(default["access_key_id"]["value"], serde_json::Value::Null);
    assert_eq!(default["secret_access_key"]["source"], "Unset");
    assert_eq!(
        default["secret_access_key"]["value"],
        serde_json::Value::Null
    );
    assert_eq!(default["security_token"]["source"], "Unset");
    assert_eq!(default["security_token"]["value"], serde_json::Value::Null);
}

#[test]
fn test_ve_tos_custom_endpoint_without_region_recommends_region() {
    let tmp = tempdir();
    let output = cli_with_home_and_env(
        &tmp,
        &["--output", "json", "ve-tos", "presign", "tos://bucket/key"],
        &[
            ("TOS_ENDPOINT", std::ffi::OsStr::new("storage.example.com")),
            ("TOS_ACCESS_KEY", std::ffi::OsStr::new("ak")),
            ("TOS_SECRET_KEY", std::ffi::OsStr::new("sk")),
        ],
    );

    assert!(!output.status.success());
    let error = parse_json(&output);
    assert!(error["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("region is required"));
    assert_eq!(
        error["error"]["fix_command"],
        "ve-tos config set region <region>"
    );
}

#[test]
fn test_adrive_ls_does_not_use_shared_tos_region_as_ids_endpoint() {
    let tmp = tempdir();
    let config_dir = tmp.join(".tos");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[default]
region = "cn-beijing"

[default.adrive]
access_key_id = "AK_FOR_ADRIVE"
secret_access_key = "SK_FOR_ADRIVE"
"#,
    )
    .expect("write config");

    let output = cli_with_home(&tmp, &["--output", "json", "ve-adrive", "ls"]);
    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"]["kind"], "config_missing");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("ADRIVE_ENDPOINT is required"));
    assert!(!json["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("ids-cn-beijing.volces.com"));
}

#[test]
fn test_tos_config_set_http_tuning_keys_and_show() {
    let tmp = tempdir();
    for (key, value) in [
        ("max_retry_count", "5"),
        ("requesttimeout", "45"),
        ("connect_timeout", "7"),
        ("max_connections", "32"),
    ] {
        let output = cli_with_home(
            &tmp,
            &["--output", "json", "ve-tos", "config", "set", key, value],
        );
        assert!(
            output.status.success(),
            "ve-tos config set {key} failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    assert!(show.status.success());
    let show_json = parse_json(&show);
    let default = find_profile(&show_json, "default");
    assert_eq!(default["max_retry_count"]["value"], 5);
    assert_eq!(default["max_retry_count"]["source"], "BinaryOverride");
    assert_eq!(default["requesttimeout"]["value"], 45);
    assert_eq!(default["requesttimeout"]["source"], "BinaryOverride");
    assert_eq!(default["connecttimeout"]["value"], 7);
    assert_eq!(default["connecttimeout"]["source"], "BinaryOverride");
    assert_eq!(default["maxconnections"]["value"], 32);
    assert_eq!(default["maxconnections"]["source"], "BinaryOverride");

    let content = std::fs::read_to_string(tmp.join(".tos").join("config.toml")).unwrap();
    assert!(content.contains("requesttimeout = 45"), "content={content}");
    assert!(content.contains("connecttimeout = 7"), "content={content}");
    assert!(content.contains("maxconnections = 32"), "content={content}");
}

#[test]
fn test_adrive_config_init_uses_global_profile_when_local_profile_missing() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "--profile",
            "dev",
            "ve-adrive",
            "config",
            "init",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["profile"], "dev");
    let content = std::fs::read_to_string(tmp.join(".tos").join("config.toml")).unwrap();
    assert!(content.contains("[dev.adrive]"), "content={content}");
    assert!(
        content.contains(r#"checkpoint_dir = "~/.tos/checkpoints/ve-adrive""#),
        "ve-adrive config init must use a surface-scoped checkpoint dir: {content}"
    );
    assert!(
        content.contains(r#"batch_report_dir = "~/.tos/reports/ve-adrive""#),
        "ve-adrive config init must use a surface-scoped report dir: {content}"
    );
    assert!(
        !content.contains("[default.adrive]"),
        "global --profile dev should not initialize default.adrive: {content}"
    );
}

#[test]
fn test_adrive_config_init_custom_path_rejects_invalid_existing_config() {
    let tmp = tempdir();
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).expect("create isolated home");
    let config_path = tmp.join("adrive").join("config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).expect("create config dir");
    std::fs::write(&config_path, "not valid = [").expect("write invalid config");
    let config_path_arg = config_path.to_string_lossy().into_owned();

    let output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "--config-path",
            config_path_arg.as_str(),
            "ve-adrive",
            "config",
            "init",
        ],
    );
    assert!(
        !output.status.success(),
        "ve-adrive config init should reject invalid existing config"
    );
    let content = std::fs::read_to_string(&config_path).expect("read config");
    assert_eq!(content, "not valid = [");
}

#[test]
fn test_adrive_config_set_high_level_and_http_tuning_keys() {
    let tmp = tempdir();
    for (key, value) in [
        ("checkpoint_dir", "/tmp/adrive-checkpoints"),
        ("progress_enabled", "false"),
        ("max_retry_count", "4"),
        ("request_timeout", "50"),
        ("connecttimeout", "9"),
        ("maxconnections", "16"),
    ] {
        let output = cli_with_home(
            &tmp,
            &["--output", "json", "ve-adrive", "config", "set", key, value],
        );
        assert!(
            output.status.success(),
            "ve-adrive config set {key} failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let show = cli_with_home(&tmp, &["--output", "json", "ve-adrive", "config", "show"]);
    assert!(show.status.success());
    let show_json = parse_json(&show);
    let default = find_profile(&show_json, "default");
    assert_eq!(default["binary"], "adrive");
    assert_eq!(default["region"]["source"], "Unset");
    assert_eq!(
        default["checkpoint_dir"]["value"],
        "/tmp/adrive-checkpoints"
    );
    assert_eq!(default["checkpoint_dir"]["source"], "BinaryOverride");
    assert_eq!(default["progress_enabled"]["value"], false);
    assert_eq!(default["progress_enabled"]["source"], "BinaryOverride");
    assert_eq!(default["max_retry_count"]["value"], 4);
    assert_eq!(default["max_retry_count"]["source"], "BinaryOverride");
    assert_eq!(default["requesttimeout"]["value"], 50);
    assert_eq!(default["requesttimeout"]["source"], "BinaryOverride");
    assert_eq!(default["connecttimeout"]["value"], 9);
    assert_eq!(default["connecttimeout"]["source"], "BinaryOverride");
    assert_eq!(default["maxconnections"]["value"], 16);
    assert_eq!(default["maxconnections"]["source"], "BinaryOverride");
}

#[test]
fn test_adrive_config_set_uses_global_profile_for_bare_key() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "endpoint",
            "https://ids-dev.volces.com",
            "--profile",
            "dev",
        ],
    );
    assert!(output.status.success());

    let show = cli_with_home(&tmp, &["--output", "json", "ve-adrive", "config", "show"]);
    assert!(show.status.success());
    let show_json = parse_json(&show);
    let dev = find_profile(&show_json, "dev");
    assert_eq!(dev["endpoint"]["value"], "https://ids-dev.volces.com");
    assert_eq!(dev["endpoint"]["source"], "BinaryOverride");

    let content = std::fs::read_to_string(tmp.join(".tos").join("config.toml")).unwrap();
    assert!(content.contains("[dev.adrive]"), "content={content}");
    assert!(
        !content.contains("[default.adrive]"),
        "bare key with --profile must not write default.adrive: {content}"
    );
}

#[test]
fn test_config_show_table_format() {
    let tmp = tempdir();
    cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "init"]);
    // Set a real-looking secret so we can observe redaction
    cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "default.secret_access_key",
            "verysecretvalue1234",
        ],
    );
    let output = cli_with_home(&tmp, &["--output", "table", "ve-tos", "config", "show"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // `config show --output table` uses a dedicated vertical renderer with
    // PROFILE/FIELD/VALUE/SOURCE columns (not the generic snake_case table),
    // so each config field appears as its own row. Real secrets stay redacted.
    assert!(
        stdout.contains("PROFILE") && stdout.contains("FIELD") && stdout.contains("SOURCE"),
        "table header must contain PROFILE/FIELD/SOURCE columns: {}",
        stdout
    );
    assert!(
        stdout.contains("region"),
        "table must contain 'region' row: {}",
        stdout
    );
    assert!(
        stdout.contains("default"),
        "table must list the default profile name: {}",
        stdout
    );
    assert!(
        stdout.contains("****"),
        "Table must show redacted secrets (AK/SK): {}",
        stdout
    );
}

#[test]
fn test_tos_config_show_table_hides_control_endpoint() {
    let tmp = tempdir();
    cli_with_home(&tmp, &["--output", "json", "tos", "config", "init"]);
    cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "tos",
            "config",
            "set",
            "endpoint",
            "tos-cn-north.byted.org",
        ],
    );

    let output = cli_with_home(&tmp, &["--output", "table", "tos", "config", "show"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("control_endpoint"),
        "tos config show table must hide ve-tos-only control_endpoint: {stdout}"
    );
}

#[test]
fn test_tos_config_set_rejects_control_endpoint() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "tos",
            "config",
            "set",
            "control_endpoint",
            "tos-control-cn-north.byted.org",
        ],
    );
    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["status"], "failed");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("only supported by ve-tos"),
        "json={json}"
    );

    let dry_run = cli_with_home(
        &tmp,
        &[
            "--dry-run",
            "--output",
            "json",
            "tos",
            "config",
            "set",
            "control_endpoint",
            "tos-control-cn-north.byted.org",
        ],
    );
    assert!(!dry_run.status.success());
}

#[test]
fn test_config_set_and_verify() {
    let tmp = tempdir();
    cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "init"]);
    // Set shared region
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "region",
            "ap-southeast-1",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["status"], "success");
    assert_eq!(json["data"]["field"], "region");
    assert_eq!(json["data"]["value"], "ap-southeast-1");
    assert_eq!(json["data"]["section"], "[default]");
    // Verify via show
    let show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    let show_json = parse_json(&show);
    let default = find_profile(&show_json, "default");
    assert_eq!(default["region"]["value"], "ap-southeast-1");
    assert_eq!(default["region"]["source"], "Shared");
}

#[test]
fn test_config_set_named_profile() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "staging.region",
            "cn-guangzhou",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["section"], "[staging]");
    assert_eq!(json["data"]["field"], "region");
    let show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    let show_json = parse_json(&show);
    let staging = find_profile(&show_json, "staging");
    assert_eq!(staging["region"]["value"], "cn-guangzhou");
}

#[test]
fn test_config_set_global_profile_routes_bare_tos_key_to_profile_override() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "endpoint",
            "http://tos-cn-boe.volces.com",
            "--profile",
            "dev",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["section"], "[dev.ve-tos]");
    assert_eq!(json["data"]["field"], "endpoint");

    let show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    let show_json = parse_json(&show);
    let dev = find_profile(&show_json, "dev");
    assert_eq!(dev["endpoint"]["value"], "http://tos-cn-boe.volces.com");
    assert_eq!(dev["endpoint"]["source"], "BinaryOverride");

    let content = std::fs::read_to_string(tmp.join(".tos").join("config.toml")).unwrap();
    assert!(content.contains("[dev.ve-tos]"), "content={content}");
    assert!(
        !content.contains("[default.ve-tos]"),
        "bare key with --profile must not write default.ve-tos: {content}"
    );
}

#[test]
fn test_tos_doctor_config_uses_global_profile() {
    let tmp = tempdir();
    let default_set = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "endpoint",
            "http://tos-default.volces.com",
        ],
    );
    assert!(default_set.status.success());
    let dev_set = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "endpoint",
            "http://tos-dev.volces.com",
            "--profile",
            "dev",
        ],
    );
    assert!(dev_set.status.success());

    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "--profile",
            "dev",
            "ve-tos",
            "doctor",
            "--check",
            "config",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["profile"], "dev");
    assert_eq!(
        json["data"]["checks"][0]["details"]["endpoint"],
        "http://tos-dev.volces.com"
    );
}

#[test]
fn test_tos_runtime_rejects_partial_profile_name() {
    let tmp = tempdir();
    let config_dir = tmp.join(".tos");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[dev]
region = "cn-beijing"

[dev.ve-tos]
endpoint = "tos-cn-beijing.volces.com"
"#,
    )
    .expect("write config");

    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "--profile",
            "de",
            "ve-tos",
            "doctor",
            "--check",
            "config",
        ],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["summary"]["failed"], 1);
    assert_eq!(json["data"]["checks"][0]["status"], "failed");
    assert!(json["data"]["checks"][0]["message"]
        .as_str()
        .expect("message")
        .contains("Profile 'de' not found"));
}

#[test]
fn test_adrive_doctor_network_uses_global_profile() {
    let tmp = tempdir();
    let default_set = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "endpoint",
            "https://ids-default.volces.com",
        ],
    );
    assert!(default_set.status.success());
    let dev_set = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "endpoint",
            "https://ids-dev.volces.com",
            "--profile",
            "dev",
        ],
    );
    assert!(dev_set.status.success());

    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "--profile",
            "dev",
            "ve-adrive",
            "doctor",
            "--check",
            "network",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["profile"], "dev");
    assert_eq!(
        json["data"]["checks"][0]["details"]["endpoint"],
        "https://ids-dev.volces.com"
    );
}

#[test]
fn test_adrive_doctor_region_only_recommends_endpoint() {
    let tmp = tempdir();
    let set = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "region",
            "cn-beijing",
        ],
    );
    assert!(set.status.success());

    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-adrive",
            "doctor",
            "--check",
            "network",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    let check = &json["data"]["checks"][0];
    assert_eq!(check["status"], "warning");
    assert_eq!(
        check["fix_command"],
        "ve-adrive config set endpoint <endpoint>"
    );
}

#[test]
fn test_adrive_runtime_rejects_partial_profile_name() {
    let tmp = tempdir();
    let config_dir = tmp.join(".tos");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"
[dev]
region = "cn-beijing"

[dev.adrive]
endpoint = "https://ids-dev.volces.com"
"#,
    )
    .expect("write config");

    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "--profile",
            "de",
            "ve-adrive",
            "doctor",
            "--check",
            "config",
        ],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["summary"]["failed"], 1);
    assert_eq!(json["data"]["checks"][0]["status"], "failed");
    assert!(json["data"]["checks"][0]["message"]
        .as_str()
        .expect("message")
        .contains("Profile 'de' not found"));
}

#[test]
fn test_config_set_table_output_renders_fields() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "table",
            "ve-tos",
            "config",
            "set",
            "endpoint",
            "http://tos-cn-boe.volces.com",
            "--profile",
            "dev",
        ],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("field"), "stdout={stdout}");
    assert!(stdout.contains("endpoint"), "stdout={stdout}");
    assert!(stdout.contains("[dev.ve-tos]"), "stdout={stdout}");
    assert!(
        !stdout.trim_start().starts_with('{'),
        "table output should not be JSON: {stdout}"
    );
}

#[test]
fn test_config_set_binary_override_and_source() {
    let tmp = tempdir();
    // Shared endpoint → should be Shared source
    cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "region",
            "cn-beijing",
        ],
    );
    cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "endpoint",
            "shared-endpoint",
        ],
    );
    // ve-tos override endpoint -> ve-tos view should be BinaryOverride
    cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "default.ve-tos.endpoint",
            "tos-cn-beijing.volces.com",
        ],
    );
    let show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    let show_json = parse_json(&show);
    let default = find_profile(&show_json, "default");
    assert_eq!(default["region"]["value"], "cn-beijing");
    assert_eq!(default["region"]["source"], "Shared");
    assert_eq!(default["endpoint"]["value"], "tos-cn-beijing.volces.com");
    assert_eq!(default["endpoint"]["source"], "BinaryOverride");
    assert_eq!(
        default["control_endpoint"]["value"],
        "tos-control-cn-beijing.volces.com"
    );
    assert_eq!(default["control_endpoint"]["source"], "Derived");
}

#[test]
fn test_config_set_control_endpoint_and_show_source() {
    let tmp = tempdir();
    cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "init"]);
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "control_endpoint",
            "tos-control-cn-shanghai.volces.com",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["section"], "[default.ve-tos]");
    assert_eq!(json["data"]["field"], "control_endpoint");
    assert_eq!(json["data"]["value"], "tos-control-cn-shanghai.volces.com");

    let show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    let show_json = parse_json(&show);
    let default = find_profile(&show_json, "default");
    assert_eq!(
        default["control_endpoint"]["value"],
        "tos-control-cn-shanghai.volces.com"
    );
    assert_eq!(default["control_endpoint"]["source"], "BinaryOverride");
}

#[test]
fn test_config_secret_is_encrypted_on_disk() {
    let tmp = tempdir();
    cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "init"]);
    let out = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "access_key_id",
            "AKTPREALSECRET1234",
        ],
    );
    assert!(out.status.success());
    let config_content = std::fs::read_to_string(tmp.join(".tos").join("config.toml")).unwrap();
    let content = std::fs::read_to_string(tmp.join(".tos").join("credentials.toml")).unwrap();
    // Raw disk must not contain plaintext AK; must contain ENC:
    assert!(
        !content.contains("AKTPREALSECRET1234"),
        "Plaintext AK leaked to disk: {}",
        content
    );
    assert!(
        content.contains("ENC:"),
        "Expected ENC: prefix in credentials file: {}",
        content
    );
    assert!(
        !config_content.contains("AKTPREALSECRET1234"),
        "config.toml must not receive new AK/SK writes: {config_content}"
    );
    // Master key file exists with 0600 on unix
    let key_path = tmp.join(".tos").join(".key");
    assert!(key_path.exists(), "Master key should be auto-generated");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "Master key must be 0600");
    }
    // show should decrypt and redact
    let show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    let show_json = parse_json(&show);
    let default = find_profile(&show_json, "default");
    let masked = default["access_key_id"]["value"].as_str().unwrap();
    assert!(
        masked.starts_with("****"),
        "AK should be redacted in show: {}",
        masked
    );
    // Redacted tail should match the last 4 chars of the plaintext
    assert!(
        masked.ends_with("1234"),
        "Redacted AK should end with last 4 of plaintext: {}",
        masked
    );
}

#[test]
fn test_config_init_preserves_existing() {
    let tmp = tempdir();
    cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "region",
            "cn-shanghai",
        ],
    );
    cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "init"]);
    let show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    let show_json = parse_json(&show);
    let default = find_profile(&show_json, "default");
    assert_eq!(
        default["region"]["value"], "cn-shanghai",
        "init should not overwrite existing profile"
    );
}

// ==========================================================================
// Principle 5: Deterministic Errors
// ==========================================================================

#[test]
fn test_config_show_no_config_error() {
    let tmp = tempdir();
    let output = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("Error should be valid JSON envelope");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"]["kind"], "config_missing");
    let exit_code = output.status.code().unwrap_or(-1);
    assert!(
        (0..=9).contains(&exit_code),
        "Exit code {} should be in range 0-9",
        exit_code
    );
}

#[test]
fn test_config_set_invalid_key_error() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "not_a_real_field",
            "value",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("Error should be valid JSON envelope");
    assert_eq!(json["status"], "failed");
    assert_eq!(json["error"]["kind"], "validation_error");
    let msg = json["error"]["message"].as_str().unwrap();
    assert!(
        msg.to_lowercase().contains("unknown") || msg.contains("not_a_real_field"),
        "Should reject unknown field: {}",
        msg
    );
    assert!(msg.contains("region"), "Should hint valid keys: {}", msg);
}

#[test]
fn test_config_set_rejects_empty_global_profile_for_bare_key() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "endpoint",
            "x",
            "--profile",
            "",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("Error should be valid JSON envelope");
    assert_eq!(json["error"]["kind"], "validation_error");
    let msg = json["error"]["message"].as_str().unwrap();
    assert!(msg.contains("profile"), "msg={msg}");
}

#[test]
fn test_adrive_config_set_rejects_empty_global_profile_for_bare_key() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "endpoint",
            "x",
            "--profile",
            "",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("Error should be valid JSON envelope");
    assert_eq!(json["error"]["kind"], "validation_error");
    let msg = json["error"]["message"].as_str().unwrap();
    assert!(msg.contains("profile"), "msg={msg}");
}

#[test]
fn test_tos_config_set_accepts_account_id_config_keys() {
    let tmp = tempdir();
    for key in [
        "account_id",
        "default.account_id",
        "default.ve-tos.account_id",
    ] {
        let output = cli_with_home(
            &tmp,
            &["--output", "json", "ve-tos", "config", "set", key, "123456"],
        );
        assert!(
            output.status.success(),
            "ve-tos config set {key} should be accepted: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json = parse_json(&output);
        assert_eq!(json["status"], "success");
        assert_eq!(json["command"], "ve-tos config set");
    }
    let content = std::fs::read_to_string(tmp.join(".tos").join("config.toml")).unwrap();
    assert!(content.contains("[default.ve-tos]"), "content={content}");
    assert!(content.contains("account_id"), "content={content}");
    assert!(content.contains("123456"), "content={content}");

    let show = cli_with_home(&tmp, &["--output", "json", "ve-tos", "config", "show"]);
    assert!(show.status.success());
    let show_json = parse_json(&show);
    let default = find_profile(&show_json, "default");
    assert_eq!(default["account_id"]["value"], "123456");
    assert_eq!(default["account_id"]["source"], "BinaryOverride");
}

#[test]
fn test_config_set_invalid_binary_error() {
    let tmp = tempdir();
    let output = cli_with_home(
        &tmp,
        &[
            "--output",
            "json",
            "ve-tos",
            "config",
            "set",
            "default.unknownbin.endpoint",
            "x",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let json: serde_json::Value =
        serde_json::from_str(stderr.trim()).expect("Error should be valid JSON envelope");
    assert_eq!(json["error"]["kind"], "validation_error");
    let msg = json["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("binary") || msg.to_lowercase().contains("unknown"),
        "Should reject unknown binary: {}",
        msg
    );
}

// ==========================================================================
// Principle 6: Agent Ecosystem — consistent schema
// ==========================================================================

#[test]
fn test_config_describe_consistent_schema() {
    let actions: Vec<Vec<&str>> = vec![
        vec!["--describe", "ve-tos", "config", "init"],
        vec!["--describe", "ve-tos", "config", "show"],
        vec!["--describe", "ve-tos", "config", "set", "k", "v"],
    ];
    for args in &actions {
        let output = cli(args);
        assert!(output.status.success(), "{:?} should succeed", args);
        let json = parse_data_json(&output);
        assert!(json["command"].is_string(), "{:?} missing command", args);
        assert_eq!(json["layer"], "meta", "{:?} should be meta layer", args);
        assert!(
            json["description"].is_string(),
            "{:?} missing description",
            args
        );
        assert!(
            json["risk_level"].is_string(),
            "{:?} missing risk_level",
            args
        );
        assert!(
            json["supports_dry_run"].is_boolean(),
            "{:?} missing supports_dry_run",
            args
        );
        assert!(
            json["supports_pipe"].is_boolean(),
            "{:?} missing supports_pipe",
            args
        );
    }
}

#[test]
fn test_config_set_dry_run_redacts_secret_value() {
    let output = cli(&[
        "--dry-run",
        "--output",
        "json",
        "ve-tos",
        "config",
        "set",
        "secret_access_key",
        "SUPERSECRET",
    ]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("SUPERSECRET"),
        "dry-run output must not contain raw secret: {stdout}"
    );
    let json = parse_json(&output);
    assert_eq!(json["status"], "success");
    let data = &json["data"];
    assert_eq!(data["dry_run"], true);
    assert!(
        serde_json::to_string(data)
            .unwrap()
            .contains("***REDACTED***"),
        "dry-run should show a redaction placeholder: {data}"
    );
}

#[test]
fn test_adrive_auth_mode_can_be_configured_and_inspected() {
    let home = tempdir();
    let set = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "auth_mode",
            "oauth",
        ],
    );
    assert!(
        set.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&set.stderr)
    );

    let status = cli_with_home(&home, &["--output", "json", "ve-adrive", "auth", "status"]);
    assert!(
        status.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&status.stderr)
    );
    let json = parse_json(&status);
    assert_eq!(json["data"]["mode"], "oauth");
    assert_eq!(json["data"]["source"], "config");
}

#[test]
fn test_adrive_auth_mode_precedence_is_cli_then_config_then_env() {
    let home = tempdir();
    let set = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "auth_mode",
            "oauth",
        ],
    );
    assert!(set.status.success());

    let from_env = cli_with_home_and_env(
        &home,
        &["--output", "json", "ve-adrive", "auth", "status"],
        &[("ADRIVE_AUTH_MODE", std::ffi::OsStr::new("aksk"))],
    );
    assert!(from_env.status.success());
    let env_json = parse_json(&from_env);
    assert_eq!(env_json["data"]["mode"], "oauth");
    assert_eq!(env_json["data"]["source"], "config");

    let from_cli = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "aksk",
            "auth",
            "status",
        ],
        &[("ADRIVE_AUTH_MODE", std::ffi::OsStr::new("aksk"))],
    );
    assert!(
        from_cli.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&from_cli.stderr)
    );
    let cli_json = parse_json(&from_cli);
    assert_eq!(cli_json["data"]["mode"], "aksk");
    assert_eq!(cli_json["data"]["source"], "command_line");
}

#[test]
// [Review Fix #5] VeTos now owns aksk/unified while ByteTOS remains isolated;
// validate the supported value set separately from tool scoping.
fn test_auth_mode_option_is_scoped_to_supported_tools() {
    let unsupported_ve_tos_value = cli(&[
        "--output",
        "json",
        "ve-tos",
        "--auth-mode",
        "oauth",
        "doctor",
    ]);
    assert!(!unsupported_ve_tos_value.status.success());
    let stderr = String::from_utf8_lossy(&unsupported_ve_tos_value.stderr);
    assert!(
        stderr.contains("possible values: aksk, unified"),
        "{stderr}"
    );

    let unsupported_tos_surface = cli(&["--output", "json", "tos", "--auth-mode", "unified", "ls"]);
    assert!(!unsupported_tos_surface.status.success());
    let stderr = String::from_utf8_lossy(&unsupported_tos_surface.stderr);
    assert!(stderr.contains("possible values: aksk, zti"), "{stderr}");
}

#[test]
fn test_oauth_mode_does_not_fall_back_to_configured_aksk() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "ls",
        ],
        &[
            ("ADRIVE_REGION", std::ffi::OsStr::new("cn-beijing")),
            ("ADRIVE_ACCESS_KEY", std::ffi::OsStr::new("ak")),
            ("ADRIVE_SECRET_KEY", std::ffi::OsStr::new("sk")),
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("login_required"), "stderr={stderr}");
    let json: serde_json::Value = serde_json::from_str(stderr.trim()).expect("valid json");
    assert_eq!(json["error"]["fix_command"], "ve-adrive auth login");
    assert_eq!(
        json["error"]["doctor_hint"],
        "ve-adrive doctor --check auth"
    );
}

#[test]
fn test_configured_oauth_without_credentials_prioritizes_login_remediation() {
    let home = tempdir();
    let set = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "auth_mode",
            "oauth",
        ],
    );
    assert!(set.status.success());

    let output = cli_with_home(&home, &["--output", "json", "ve-adrive", "ls"]);

    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["error"]["fix_command"], "ve-adrive auth login");
    assert_eq!(
        json["error"]["doctor_hint"],
        "ve-adrive doctor --check auth"
    );
}

#[test]
fn test_oauth_with_access_token_reports_resource_configuration_remediation() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "ls",
        ],
        &[(
            "ADRIVE_ACCESS_TOKEN",
            std::ffi::OsStr::new("PROCESS_SCOPED_TOKEN"),
        )],
    );

    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(
        json["error"]["fix_command"],
        "ve-adrive config set endpoint <endpoint>"
    );
    assert_eq!(
        json["error"]["doctor_hint"],
        "ve-adrive doctor --check config"
    );
}

#[test]
fn test_oauth_missing_profile_recommends_profile_initialization() {
    let home = tempdir();
    let set = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "--profile",
            "existing",
            "ve-adrive",
            "config",
            "set",
            "auth_mode",
            "oauth",
        ],
    );
    assert!(set.status.success());

    let output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "--profile",
            "missing",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "ls",
        ],
    );

    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["error"]["fix_command"], "ve-adrive config init");
    assert_eq!(
        json["error"]["doctor_hint"],
        "ve-adrive doctor --check config"
    );
}

#[test]
fn test_adrive_auth_mode_defaults_to_legacy_aksk() {
    let home = tempdir();
    let status = cli_with_home(&home, &["--output", "json", "ve-adrive", "auth", "status"]);
    assert!(
        status.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&status.stderr)
    );
    let json = parse_json(&status);
    assert_eq!(json["command"], "ve-adrive auth status");
    assert_eq!(json["data"]["mode"], "aksk");
    assert_eq!(json["data"]["source"], "compatibility_default");
}

#[test]
fn test_adrive_config_rejects_invalid_auth_mode() {
    let home = tempdir();
    let output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "auth_mode",
            "automatic",
        ],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected aksk, oauth, or unified"),
        "stderr={stderr}"
    );
}

#[test]
fn test_adrive_rejects_invalid_auth_mode_from_environment() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &["--output", "json", "ve-adrive", "auth", "status"],
        &[("ADRIVE_AUTH_MODE", std::ffi::OsStr::new("automatic"))],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ADRIVE_AUTH_MODE"), "stderr={stderr}");
    assert!(
        stderr.contains("expected aksk, oauth, or unified"),
        "stderr={stderr}"
    );
}

#[test]
fn test_adrive_auth_mode_uses_the_same_values_in_every_source() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &["--output", "json", "ve-adrive", "auth", "status"],
        &[("ADRIVE_AUTH_MODE", std::ffi::OsStr::new("ak/sk"))],
    );
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("expected aksk, oauth, or unified"),
        "stderr={stderr}"
    );
}

#[test]
fn test_adrive_auth_login_is_mode_strict_and_dry_run_stays_offline() {
    let home = tempdir();
    let aksk = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "aksk",
            "auth",
            "login",
        ],
    );
    assert!(!aksk.status.success());
    let aksk_json = parse_json(&aksk);
    assert_eq!(aksk_json["command"], "ve-adrive auth login");
    assert!(aksk_json["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("requires OAuth mode"));

    let oauth = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "--dry-run",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "auth",
            "login",
            "--instance",
            "inst-1",
            "--auth-endpoint",
            "https://auth.example.com",
        ],
    );
    assert!(oauth.status.success());
    let oauth_json = parse_json(&oauth);
    assert_eq!(oauth_json["command"], "ve-adrive auth login");
    assert_eq!(oauth_json["data"]["dry_run"], true);
    assert_eq!(oauth_json["data"]["instance_id"], "inst-1");
    assert_eq!(oauth_json["data"]["scope"], "all");
    assert_eq!(oauth_json["data"]["network_requests"], 0);
    assert_eq!(oauth_json["data"]["credentials_written"], false);
}

#[test]
fn test_adrive_oauth_login_without_instance_returns_actionable_command() {
    let home = tempdir();
    let output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "auth",
            "login",
        ],
    );

    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["error"]["code"], "oauth_instance_required");
    assert_eq!(
        json["error"]["fix_command"],
        "ve-adrive auth login --instance <instance_id>"
    );
    assert_eq!(
        json["error"]["doctor_hint"],
        "ve-adrive doctor --check config"
    );
}

#[test]
fn test_adrive_oauth_login_without_auth_endpoint_returns_actionable_command() {
    let home = tempdir();
    let output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "auth",
            "login",
            "--instance",
            "inst-1",
        ],
    );

    assert!(!output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["error"]["code"], "oauth_auth_endpoint_required");
    assert_eq!(
        json["error"]["fix_command"],
        "ve-adrive config set auth_endpoint <url>"
    );
    assert_eq!(
        json["error"]["doctor_hint"],
        "ve-adrive doctor --check config"
    );
}

#[test]
fn test_adrive_auth_status_reports_tokens_without_exposing_them() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "auth",
            "status",
        ],
        &[
            (
                "ADRIVE_ACCESS_TOKEN",
                std::ffi::OsStr::new("ACCESS_TOKEN_MUST_NOT_LEAK"),
            ),
            (
                "ADRIVE_REFRESH_TOKEN",
                std::ffi::OsStr::new("REFRESH_TOKEN_MUST_NOT_LEAK"),
            ),
            ("ADRIVE_ACCESS_KEY", std::ffi::OsStr::new("IGNORED_AK")),
            ("ADRIVE_SECRET_KEY", std::ffi::OsStr::new("IGNORED_SK")),
        ],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("ACCESS_TOKEN_MUST_NOT_LEAK"),
        "stdout={stdout}"
    );
    assert!(
        !stdout.contains("REFRESH_TOKEN_MUST_NOT_LEAK"),
        "stdout={stdout}"
    );
    let json = parse_json(&output);
    assert_eq!(json["data"]["has_access_key"], serde_json::Value::Null);
    assert_eq!(json["data"]["has_secret_key"], serde_json::Value::Null);
    assert_eq!(json["data"]["has_access_token"], true);
    assert_eq!(json["data"]["has_refresh_token"], true);
    assert_eq!(json["data"]["access_token_expiry"], "unknown");
    assert_eq!(json["data"]["scope"], serde_json::Value::Null);
    assert_eq!(json["data"]["instance_id"], serde_json::Value::Null);
    assert_eq!(json["data"]["ready"], true);
    assert_eq!(json["data"]["oauth_service_integration"], "enabled");
}

#[test]
fn test_adrive_oauth_blank_tokens_are_not_reported_as_credentials() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "auth",
            "status",
        ],
        &[
            ("ADRIVE_ACCESS_TOKEN", std::ffi::OsStr::new("   ")),
            ("ADRIVE_REFRESH_TOKEN", std::ffi::OsStr::new("")),
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["has_access_token"], false);
    assert_eq!(json["data"]["has_refresh_token"], false);
}

#[test]
fn test_adrive_capabilities_registers_auth_framework() {
    let home = tempdir();
    let output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "capabilities",
            "--view",
            "full",
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    let capabilities = json["data"]["capabilities"]
        .as_array()
        .expect("capabilities array");
    assert!(
        capabilities
            .iter()
            .any(|row| row["command"] == "ve-adrive auth"),
        "json={json}"
    );
}

#[test]
fn test_adrive_auth_mode_can_follow_the_leaf_command() {
    let home = tempdir();
    let output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "auth",
            "status",
            "--auth-mode",
            "oauth",
        ],
    );
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_json(&output);
    assert_eq!(json["data"]["mode"], "oauth");
    assert_eq!(json["data"]["source"], "command_line");
}

#[test]
fn test_adrive_auth_without_action_defaults_to_status() {
    let home = tempdir();
    let output = cli_with_home(&home, &["--output", "json", "ve-adrive", "auth"]);
    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = parse_json(&output);
    assert_eq!(json["command"], "ve-adrive auth");
    assert_eq!(json["data"]["mode"], "aksk");
    assert_eq!(json["data"]["credential_source"], "resolved_profile");
}

#[test]
fn test_adrive_doctor_reports_available_oauth_as_ready() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "doctor",
            "--check",
            "auth",
        ],
        &[
            (
                "ADRIVE_ACCESS_TOKEN",
                std::ffi::OsStr::new("PROCESS_SCOPED_TOKEN"),
            ),
            (
                "ADRIVE_OAUTH_CLIENT_ID",
                std::ffi::OsStr::new("test-client-id"),
            ),
            ("ADRIVE_ACCESS_KEY", std::ffi::OsStr::new("IGNORED_AK")),
            ("ADRIVE_SECRET_KEY", std::ffi::OsStr::new("IGNORED_SK")),
        ],
    );
    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["checks"][0]["status"], "passed");
    assert_eq!(
        json["data"]["checks"][0]["details"]["has_access_key"],
        serde_json::Value::Null
    );
    assert_eq!(
        json["data"]["checks"][0]["details"]["oauth_service_integration"],
        "enabled"
    );
    assert_eq!(
        json["data"]["checks"][0]["details"]["credential_state"],
        "access_available"
    );
    assert_eq!(
        json["data"]["checks"][0]["details"]["oauth_client_id_source"],
        "environment"
    );
    assert_eq!(
        json["data"]["checks"][0]["details"]["oauth_client_id_placeholder"],
        false
    );
    assert!(json["data"]["checks"][0]["details"]
        .get("access_key_source")
        .is_none());
    assert!(json["data"]["checks"][0].get("fix_command").is_none());
    assert_eq!(json["data"]["summary"]["passed"], 1);
}

#[test]
fn test_adrive_doctor_uses_registered_builtin_oauth_client_id() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "doctor",
            "--check",
            "auth",
        ],
        &[(
            "ADRIVE_AUTH_ENDPOINT",
            std::ffi::OsStr::new("https://auth.example.com"),
        )],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    let auth_check = &json["data"]["checks"][0];
    assert_eq!(auth_check["status"], "warning");
    assert!(auth_check["message"]
        .as_str()
        .unwrap()
        .contains("credentials are not configured"));
    assert_eq!(auth_check["details"]["oauth_client_id_source"], "built_in");
    assert_eq!(auth_check["details"]["oauth_client_id_placeholder"], false);
    assert_eq!(auth_check["fix_command"], "ve-adrive auth login");
}

#[test]
fn test_adrive_doctor_process_access_token_does_not_require_client_id() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "doctor",
            "--check",
            "auth",
        ],
        &[(
            "ADRIVE_ACCESS_TOKEN",
            std::ffi::OsStr::new("PROCESS_SCOPED_TOKEN"),
        )],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    let auth_check = &json["data"]["checks"][0];
    assert_eq!(auth_check["status"], "passed");
    assert_eq!(auth_check["details"]["credential_source"], "environment");
    assert_eq!(auth_check["details"]["oauth_client_id_placeholder"], false);
}

#[test]
fn test_adrive_doctor_oauth_without_tokens_recommends_login() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "doctor",
            "--check",
            "auth",
        ],
        &[
            (
                "ADRIVE_AUTH_ENDPOINT",
                std::ffi::OsStr::new("https://auth.example.com"),
            ),
            (
                "ADRIVE_OAUTH_CLIENT_ID",
                std::ffi::OsStr::new("test-client-id"),
            ),
        ],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    let auth_check = &json["data"]["checks"][0];
    assert_eq!(auth_check["status"], "warning");
    assert_eq!(auth_check["details"]["credential_state"], "logged_out");
    assert_eq!(auth_check["fix_command"], "ve-adrive auth login");
}

#[test]
fn test_adrive_doctor_reports_aksk_environment_sources() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "aksk",
            "doctor",
            "--check",
            "auth",
        ],
        &[
            ("ADRIVE_ACCESS_KEY", std::ffi::OsStr::new("ENV_AK")),
            ("ADRIVE_SECRET_KEY", std::ffi::OsStr::new("ENV_SK")),
        ],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    let auth_check = &json["data"]["checks"][0];
    assert_eq!(auth_check["status"], "passed");
    assert_eq!(auth_check["details"]["access_key_source"], "environment");
    assert_eq!(auth_check["details"]["secret_key_source"], "environment");
    assert_eq!(auth_check["details"]["security_token_source"], "none");
    assert_eq!(auth_check["details"]["credential_source"], "environment");
}

#[test]
fn test_adrive_doctor_reports_legacy_config_aksk_sources() {
    let home = tempdir();
    let config_dir = home.join(".tos");
    std::fs::create_dir_all(&config_dir).expect("create config directory");
    std::fs::write(
        config_dir.join("config.toml"),
        "[default.adrive]\naccess_key_id = \"LEGACY_AK\"\nsecret_access_key = \"LEGACY_SK\"\n",
    )
    .expect("write legacy config");

    let output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "aksk",
            "doctor",
            "--check",
            "auth",
        ],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    let details = &json["data"]["checks"][0]["details"];
    assert_eq!(details["access_key_source"], "config_file");
    assert_eq!(details["secret_key_source"], "config_file");
    assert_eq!(details["credential_source"], "config_file");
}

#[test]
fn test_adrive_doctor_reports_mixed_aksk_sources_without_warning() {
    let home = tempdir();
    let set_access_key = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "access_key_id",
            "STORED_AK",
        ],
    );
    assert!(set_access_key.status.success());
    std::fs::write(
        home.join(".tos").join("config.toml"),
        "[default.adrive]\naccess_key_id = \"LOWER_PRIORITY_AK\"\nsecret_access_key = \"LEGACY_SK\"\n",
    )
    .expect("write legacy config");

    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "aksk",
            "doctor",
            "--check",
            "auth",
        ],
        &[
            (
                "ADRIVE_ACCESS_KEY",
                std::ffi::OsStr::new("LOWEST_PRIORITY_AK"),
            ),
            (
                "ADRIVE_SECRET_KEY",
                std::ffi::OsStr::new("LOWER_PRIORITY_SK"),
            ),
            (
                "ADRIVE_SECURITY_TOKEN",
                std::ffi::OsStr::new("ENV_SESSION_TOKEN"),
            ),
        ],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    let auth_check = &json["data"]["checks"][0];
    let details = &auth_check["details"];
    assert_eq!(auth_check["status"], "passed");
    assert_eq!(details["access_key_source"], "credentials_file");
    assert_eq!(details["secret_key_source"], "config_file");
    assert_eq!(details["security_token_source"], "environment");
    assert_eq!(details["credential_source"], "mixed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    for credential in [
        "STORED_AK",
        "LOWER_PRIORITY_AK",
        "LOWEST_PRIORITY_AK",
        "LEGACY_SK",
        "LOWER_PRIORITY_SK",
        "ENV_SESSION_TOKEN",
    ] {
        assert!(
            !stdout.contains(credential),
            "credential leaked: {credential}"
        );
    }
}

#[test]
fn test_adrive_doctor_recommends_the_missing_aksk_field() {
    let cases = [
        (
            "missing-access-key",
            "ADRIVE_SECRET_KEY",
            "ONLY_SK",
            "Access Key",
            "ve-adrive config set access_key_id <access_key_id>",
        ),
        (
            "missing-secret-key",
            "ADRIVE_ACCESS_KEY",
            "ONLY_AK",
            "Secret Key",
            "ve-adrive config set secret_access_key <secret_access_key>",
        ),
    ];

    for (case_name, environment_key, value, message_fragment, fix_command) in cases {
        let home = tempdir().join(case_name);
        std::fs::create_dir_all(&home).expect("create test home");
        let output = cli_with_home_and_env(
            &home,
            &[
                "--output",
                "json",
                "ve-adrive",
                "--auth-mode",
                "aksk",
                "doctor",
                "--check",
                "auth",
            ],
            &[(environment_key, std::ffi::OsStr::new(value))],
        );

        assert!(output.status.success(), "case={case_name}");
        let json = parse_json(&output);
        let auth_check = &json["data"]["checks"][0];
        assert_eq!(auth_check["status"], "warning", "case={case_name}");
        assert!(
            auth_check["message"]
                .as_str()
                .is_some_and(|message| message.contains(message_fragment)),
            "case={case_name}, json={json}"
        );
        assert_eq!(auth_check["fix_command"], fix_command, "case={case_name}");
    }
}

#[test]
fn test_adrive_doctor_without_aksk_recommends_config_init() {
    let home = tempdir();
    let output = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "aksk",
            "doctor",
            "--check",
            "auth",
        ],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    let auth_check = &json["data"]["checks"][0];
    assert_eq!(auth_check["status"], "warning");
    assert_eq!(auth_check["fix_command"], "ve-adrive config init");
    assert_eq!(auth_check["details"]["credential_source"], "none");
}

#[test]
fn test_adrive_oauth_doctor_requires_login_auth_endpoint() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "doctor",
            "--check",
            "auth",
        ],
        &[(
            "ADRIVE_OAUTH_CLIENT_ID",
            std::ffi::OsStr::new("test-client-id"),
        )],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    let check = &json["data"]["checks"][0];
    assert_eq!(check["status"], "warning");
    assert_eq!(
        check["fix_command"],
        "ve-adrive config set auth_endpoint <url>"
    );
}

#[test]
fn test_adrive_doctor_rejects_environment_refresh_token_without_access_token() {
    let home = tempdir();
    let output = cli_with_home_and_env(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "doctor",
            "--check",
            "auth",
        ],
        &[
            // [Review Fix #4] Keep this case focused on the unsupported
            // process Refresh Token source instead of the placeholder warning.
            (
                "ADRIVE_AUTH_ENDPOINT",
                std::ffi::OsStr::new("https://auth.example.com"),
            ),
            (
                "ADRIVE_REFRESH_TOKEN",
                std::ffi::OsStr::new("UNUSABLE_PROCESS_REFRESH_TOKEN"),
            ),
            (
                "ADRIVE_OAUTH_CLIENT_ID",
                std::ffi::OsStr::new("test-client-id"),
            ),
        ],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("UNUSABLE_PROCESS_REFRESH_TOKEN"));
    let json = parse_json(&output);
    assert_eq!(json["data"]["checks"][0]["status"], "warning");
    assert_eq!(json["data"]["checks"][0]["details"]["ready"], false);
    assert_eq!(
        json["data"]["checks"][0]["details"]["credential_state"],
        "unsupported_refresh_source"
    );
    assert_eq!(
        json["data"]["checks"][0]["fix_command"],
        "ve-adrive auth login"
    );
    assert_eq!(json["data"]["summary"]["warnings"], 1);
}

#[test]
fn test_adrive_config_show_includes_auth_endpoint() {
    let home = tempdir();
    let set = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "config",
            "set",
            "auth_endpoint",
            "https://auth.example.com",
        ],
    );
    assert!(set.status.success());

    let show = cli_with_home(&home, &["--output", "json", "ve-adrive", "config", "show"]);

    assert!(show.status.success());
    let json = parse_json(&show);
    assert_eq!(
        json["data"]["profiles"][0]["auth_endpoint"]["value"],
        "https://auth.example.com"
    );
}

#[test]
fn test_adrive_auth_logout_is_mode_strict_and_offline() {
    let home = tempdir();
    let aksk = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "aksk",
            "auth",
            "logout",
        ],
    );
    assert!(!aksk.status.success());
    let aksk_json = parse_json(&aksk);
    assert!(aksk_json["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("requires OAuth mode"));

    let oauth = cli_with_home(
        &home,
        &[
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "auth",
            "logout",
        ],
    );
    assert!(oauth.status.success());
    let oauth_json = parse_json(&oauth);
    assert_eq!(oauth_json["command"], "ve-adrive auth logout");
    assert_eq!(oauth_json["data"]["status"], "already_logged_out");
    assert_eq!(oauth_json["data"]["scope"], "local_credentials_file_only");
}

#[test]
fn test_adrive_auth_logout_dry_run_preserves_credentials() {
    let home = tempdir();
    let credentials_dir = home.join(".tos");
    std::fs::create_dir_all(&credentials_dir).expect("create credentials directory");
    let credentials_path = credentials_dir.join("credentials.toml");
    let original = "schema_version = 1\n\n[default.adrive.oauth]\naccess_token = \"access\"\nrefresh_token = \"refresh\"\n";
    std::fs::write(&credentials_path, original).expect("write credentials");

    let output = cli_with_home(
        &home,
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "auth",
            "logout",
        ],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["dry_run"], true);
    assert_eq!(json["data"]["would_clear"], true);
    assert_eq!(json["data"]["credentials_written"], false);
    assert_eq!(json["data"]["status"], "would_log_out");
    assert_eq!(
        std::fs::read_to_string(credentials_path).expect("read credentials"),
        original
    );
    assert!(!credentials_dir.join(".key").exists());
}

#[test]
fn test_adrive_auth_logout_dry_run_does_not_create_credentials() {
    let home = tempdir();
    let output = cli_with_home(
        &home,
        &[
            "--dry-run",
            "--output",
            "json",
            "ve-adrive",
            "--auth-mode",
            "oauth",
            "auth",
            "logout",
        ],
    );

    assert!(output.status.success());
    let json = parse_json(&output);
    assert_eq!(json["data"]["status"], "already_logged_out");
    assert_eq!(json["data"]["would_clear"], false);
    assert_eq!(json["data"]["credentials_written"], false);
    assert!(!home.join(".tos").exists());
}

// ==========================================================================
// Helper
// ==========================================================================

fn tempdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ve-tos-cli-test-{}", std::process::id()));
    let tid = format!("{:?}", std::thread::current().id());
    let dir = dir.join(tid.replace(|c: char| !c.is_alphanumeric(), "_"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}
