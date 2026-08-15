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

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn tempdir(name: &str) -> PathBuf {
    let thread = format!("{:?}", std::thread::current().id())
        .replace(|character: char| !character.is_alphanumeric(), "_");
    let path = std::env::temp_dir().join(format!(
        "ve-storage-credentials-{}-{name}-{thread}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create isolated home");
    path
}

fn cli(home: &Path, args: &[&str], envs: &[(&str, &OsStr)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"));
    command.env("HOME", home);
    for key in [
        "TOS_CONFIG_PATH",
        "TOS_CREDENTIALS_PATH",
        "TOS_ACCESS_KEY",
        "TOS_SECRET_KEY",
        "TOS_SECURITY_TOKEN",
        "BYTE_TOS_ACCESS_KEY",
        "BYTE_TOS_SECRET_KEY",
        "BYTE_TOS_SECURITY_TOKEN",
        "ADRIVE_ACCESS_KEY",
        "ADRIVE_SECRET_KEY",
        "ADRIVE_SECURITY_TOKEN",
        "ADRIVE_ACCESS_TOKEN",
        "ADRIVE_REFRESH_TOKEN",
        "ADRIVE_AUTH_MODE",
    ] {
        command.env_remove(key);
    }
    for (key, value) in envs {
        command.env(key, value);
    }
    command.args(args).output().expect("run CLI")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn parse_json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout)
        .or_else(|_| serde_json::from_slice(&output.stderr))
        .unwrap_or_else(|error| {
            panic!(
                "invalid JSON: {error}\nstdout={}\nstderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        })
}

#[test]
fn credential_write_accepts_explicit_credentials_path() {
    let home = tempdir("explicit-path");
    let credentials_path = home.join("secure").join("credentials.toml");
    let credentials_arg = credentials_path.to_string_lossy().into_owned();
    let output = cli(
        &home,
        &[
            "--output",
            "json",
            "--credentials-path",
            &credentials_arg,
            "ve-adrive",
            "config",
            "set",
            "access_key_id",
            "NEW_ADRIVE_AK",
        ],
        &[],
    );

    assert_success(&output);
    assert!(credentials_path.exists());
    assert!(!home.join(".tos").join("credentials.toml").exists());
    let content = std::fs::read_to_string(credentials_path).expect("read credentials");
    assert!(!content.contains("NEW_ADRIVE_AK"));
    assert!(content.contains("ENC:"));
}

#[test]
fn command_line_credentials_path_overrides_environment_path() {
    let home = tempdir("credentials-path-precedence");
    let environment_path = home.join("environment").join("credentials.toml");
    let command_line_path = home.join("command-line").join("credentials.toml");
    let command_line_arg = command_line_path.to_string_lossy().into_owned();
    let output = cli(
        &home,
        &[
            "--credentials-path",
            &command_line_arg,
            "ve-adrive",
            "config",
            "set",
            "access_key_id",
            "CLI_PATH_AK",
        ],
        &[(
            "TOS_CREDENTIALS_PATH",
            OsStr::new(environment_path.as_os_str()),
        )],
    );

    assert_success(&output);
    assert!(command_line_path.exists());
    assert!(!environment_path.exists());
}

#[test]
fn default_credentials_path_is_sibling_of_effective_config() {
    let home = tempdir("sibling-path");
    let config_path = home.join("custom").join("settings.toml");
    let config_arg = config_path.to_string_lossy().into_owned();
    let output = cli(
        &home,
        &[
            "--config-path",
            &config_arg,
            "ve-adrive",
            "config",
            "set",
            "secret_access_key",
            "NEW_ADRIVE_SK",
        ],
        &[],
    );

    assert_success(&output);
    assert!(home.join("custom").join("credentials.toml").exists());
    assert!(!home.join(".tos").join("credentials.toml").exists());
}

#[test]
fn credential_write_does_not_modify_existing_config() {
    let home = tempdir("no-dual-write");
    let config_dir = home.join(".tos");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    let config_path = config_dir.join("config.toml");
    let original = "[default.adrive]\nregion = \"cn-beijing\"\n";
    std::fs::write(&config_path, original).expect("write legacy config");

    let output = cli(
        &home,
        &[
            "ve-adrive",
            "config",
            "set",
            "access_key_id",
            "NEW_ADRIVE_AK",
        ],
        &[],
    );

    assert_success(&output);
    assert_eq!(
        std::fs::read_to_string(config_path).expect("read config"),
        original
    );
    assert!(config_dir.join("credentials.toml").exists());
}

#[test]
fn reading_legacy_config_credentials_does_not_migrate_them() {
    let home = tempdir("no-auto-migration");
    let config_dir = home.join(".tos");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "[default.adrive]\naccess_key_id = \"LEGACY_AK\"\nsecret_access_key = \"LEGACY_SK\"\n",
    )
    .unwrap();

    let doctor = cli(
        &home,
        &["--output", "json", "ve-adrive", "doctor", "--check", "auth"],
        &[],
    );
    assert_success(&doctor);
    assert_eq!(parse_json(&doctor)["data"]["checks"][0]["status"], "passed");
    assert!(!config_dir.join("credentials.toml").exists());
}

#[test]
fn three_cli_surfaces_write_isolated_credential_sections() {
    let home = tempdir("surface-isolation");
    for (surface, key, value) in [
        ("tos", "default.tos.access_key_id", "BYTE_AK"),
        ("ve-tos", "default.ve-tos.access_key_id", "VE_TOS_AK"),
        ("ve-adrive", "access_key_id", "ADRIVE_AK"),
    ] {
        let output = cli(&home, &[surface, "config", "set", key, value], &[]);
        assert_success(&output);
    }

    let content = std::fs::read_to_string(home.join(".tos").join("credentials.toml"))
        .expect("read credentials");
    for raw_secret in ["BYTE_AK", "VE_TOS_AK", "ADRIVE_AK"] {
        assert!(!content.contains(raw_secret), "content={content}");
    }
    assert!(content.contains("[default.tos]"));
    assert!(content.contains("[default.ve-tos]"));
    assert!(content.contains("[default.adrive]"));
    assert!(!content.contains("[profiles."));
    assert!(!content.contains(".aksk]"));
}

#[test]
fn bare_credential_keys_write_to_the_active_surface() {
    let home = tempdir("bare-surface-routing");
    for (surface, access_key, secret_key) in [
        ("tos", "BYTE_AK", "BYTE_SK"),
        ("ve-tos", "VE_TOS_AK", "VE_TOS_SK"),
        ("ve-adrive", "ADRIVE_AK", "ADRIVE_SK"),
    ] {
        assert_success(&cli(
            &home,
            &[surface, "config", "set", "access_key_id", access_key],
            &[],
        ));
        assert_success(&cli(
            &home,
            &[surface, "config", "set", "secret_access_key", secret_key],
            &[],
        ));
    }

    let content = std::fs::read_to_string(home.join(".tos").join("credentials.toml"))
        .expect("read credentials");
    assert!(content.contains("[default.tos]"), "content={content}");
    assert!(content.contains("[default.ve-tos]"), "content={content}");
    assert!(content.contains("[default.adrive]"), "content={content}");
    assert!(!content.contains("\n[default]\n"), "content={content}");

    assert_success(&cli(
        &home,
        &[
            "--profile",
            "staging",
            "tos",
            "config",
            "set",
            "security_token",
            "BYTE_TOKEN",
        ],
        &[],
    ));
    let content = std::fs::read_to_string(home.join(".tos").join("credentials.toml"))
        .expect("read credentials");
    assert!(content.contains("[staging.tos]"), "content={content}");
}

#[test]
fn explicit_profile_credential_key_still_writes_shared_credentials() {
    let home = tempdir("explicit-shared-routing");
    assert_success(&cli(
        &home,
        &["tos", "config", "set", "default.access_key_id", "SHARED_AK"],
        &[],
    ));

    let content = std::fs::read_to_string(home.join(".tos").join("credentials.toml"))
        .expect("read credentials");
    assert!(content.contains("\n[default]\n"), "content={content}");
    assert!(!content.contains("[default.tos]"), "content={content}");
    assert!(!content.contains("[default.ve-tos]"), "content={content}");
}

#[test]
fn runtime_auth_checks_use_credentials_file_for_all_surfaces() {
    let home = tempdir("runtime-resolution");
    for (surface, access_key, secret_key) in [
        (
            "tos",
            "default.tos.access_key_id",
            "default.tos.secret_access_key",
        ),
        (
            "ve-tos",
            "default.ve-tos.access_key_id",
            "default.ve-tos.secret_access_key",
        ),
        ("ve-adrive", "access_key_id", "secret_access_key"),
    ] {
        assert_success(&cli(
            &home,
            &[surface, "config", "set", access_key, "AK"],
            &[],
        ));
        assert_success(&cli(
            &home,
            &[surface, "config", "set", secret_key, "SK"],
            &[],
        ));
        let doctor = cli(
            &home,
            &["--output", "json", surface, "doctor", "--check", "auth"],
            &[],
        );
        assert_success(&doctor);
        let json = parse_json(&doctor);
        assert_eq!(
            json["data"]["checks"][0]["status"], "passed",
            "surface={surface}, json={json}"
        );
    }
}

#[test]
fn credential_set_dry_run_names_target_without_writing_files() {
    let home = tempdir("dry-run");
    let output = cli(
        &home,
        &[
            "--output",
            "json",
            "--dry-run",
            "ve-adrive",
            "config",
            "set",
            "access_key_id",
            "DRY_RUN_AK",
        ],
        &[],
    );
    assert_success(&output);
    let json = parse_json(&output);
    let plan = serde_json::to_string(&json["data"]["plan"]).unwrap();
    assert!(plan.contains("credentials.toml"), "json={json}");
    assert!(!plan.contains("DRY_RUN_AK"), "json={json}");
    assert!(!home.join(".tos").join("config.toml").exists());
    assert!(!home.join(".tos").join("credentials.toml").exists());
}

#[test]
fn non_sensitive_config_write_does_not_create_credentials_file() {
    let home = tempdir("non-sensitive");
    let output = cli(
        &home,
        &["ve-adrive", "config", "set", "region", "cn-beijing"],
        &[],
    );
    assert_success(&output);
    assert!(home.join(".tos").join("config.toml").exists());
    assert!(!home.join(".tos").join("credentials.toml").exists());
}

#[test]
fn config_show_merges_credentials_only_profiles_for_all_surfaces() {
    let home = tempdir("dual-file-show");
    for (surface, access_key, secret_key, raw_access_key) in [
        (
            "tos",
            "default.tos.access_key_id",
            "default.tos.secret_access_key",
            "BYTE_SHOW_AK",
        ),
        (
            "ve-tos",
            "default.ve-tos.access_key_id",
            "default.ve-tos.secret_access_key",
            "VE_SHOW_AK",
        ),
        (
            "ve-adrive",
            "access_key_id",
            "secret_access_key",
            "ADRIVE_SHOW_AK",
        ),
    ] {
        assert_success(&cli(
            &home,
            &[surface, "config", "set", access_key, raw_access_key],
            &[],
        ));
        assert_success(&cli(
            &home,
            &[surface, "config", "set", secret_key, "SHOW_SK"],
            &[],
        ));
        let show = cli(&home, &["--output", "json", surface, "config", "show"], &[]);
        assert_success(&show);
        let text = String::from_utf8_lossy(&show.stdout);
        assert!(!text.contains(raw_access_key), "surface={surface}, {text}");
        assert!(!text.contains("SHOW_SK"), "surface={surface}, {text}");
        let json = parse_json(&show);
        assert!(json["data"]["credentials_path"]
            .as_str()
            .unwrap_or_default()
            .ends_with("credentials.toml"));
        assert!(
            text.contains("credentials_file"),
            "surface={surface}, {text}"
        );
    }
}

#[test]
fn credentials_file_overlays_legacy_config_field_by_field() {
    let home = tempdir("legacy-overlay");
    let config_dir = home.join(".tos");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "[default.adrive]\naccess_key_id = \"LEGACY_AK\"\nsecret_access_key = \"LEGACY_SK\"\n",
    )
    .unwrap();
    assert_success(&cli(
        &home,
        &["ve-adrive", "config", "set", "access_key_id", "STORE_AK"],
        &[],
    ));

    let show = cli(
        &home,
        &["--output", "json", "ve-adrive", "config", "show"],
        &[],
    );
    assert_success(&show);
    let json = parse_json(&show);
    let profile = &json["data"]["profiles"][0];
    assert_eq!(profile["access_key_id"]["source"], "credentials_file");
    assert_eq!(profile["secret_access_key"]["source"], "BinaryOverride");
    let text = String::from_utf8_lossy(&show.stdout);
    for raw in ["LEGACY_AK", "LEGACY_SK", "STORE_AK"] {
        assert!(!text.contains(raw), "stdout={text}");
    }
}

#[test]
fn named_profile_can_use_credentials_without_config_profile() {
    let home = tempdir("named-profile");
    for (key, value) in [("access_key_id", "AK"), ("secret_access_key", "SK")] {
        assert_success(&cli(
            &home,
            &[
                "--profile",
                "staging",
                "ve-adrive",
                "config",
                "set",
                key,
                value,
            ],
            &[],
        ));
    }
    let doctor = cli(
        &home,
        &[
            "--output",
            "json",
            "--profile",
            "staging",
            "ve-adrive",
            "doctor",
            "--check",
            "auth",
        ],
        &[],
    );
    assert_success(&doctor);
    let json = parse_json(&doctor);
    assert_eq!(json["data"]["checks"][0]["status"], "passed", "json={json}");
}

#[test]
fn adrive_oauth_tokens_prefer_credentials_file_over_environment() {
    let home = tempdir("oauth-precedence");
    let config_dir = home.join(".tos");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("credentials.toml"),
        "schema_version = 1\n\n[default.adrive.oauth]\naccess_token = \"FILE_ACCESS_TOKEN\"\nrefresh_token = \"FILE_REFRESH_TOKEN\"\n",
    )
    .unwrap();
    let status = cli(
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
            ("ADRIVE_ACCESS_TOKEN", OsStr::new("ENV_ACCESS_TOKEN")),
            ("ADRIVE_REFRESH_TOKEN", OsStr::new("ENV_REFRESH_TOKEN")),
        ],
    );
    assert_success(&status);
    let json = parse_json(&status);
    assert_eq!(json["data"]["credential_source"], "credentials_file");
    let text = String::from_utf8_lossy(&status.stdout);
    for raw in [
        "FILE_ACCESS_TOKEN",
        "FILE_REFRESH_TOKEN",
        "ENV_ACCESS_TOKEN",
        "ENV_REFRESH_TOKEN",
    ] {
        assert!(!text.contains(raw), "stdout={text}");
    }
}

#[test]
fn config_show_does_not_expose_sibling_surface_credentials() {
    let adrive_home = tempdir("show-adrive-isolation");
    assert_success(&cli(
        &adrive_home,
        &[
            "ve-adrive",
            "config",
            "set",
            "access_key_id",
            "ADRIVE_ONLY_AK",
        ],
        &[],
    ));
    let tos_show = cli(
        &adrive_home,
        &["--output", "json", "tos", "config", "show"],
        &[],
    );
    assert_success(&tos_show);
    assert!(parse_json(&tos_show)["data"]["profiles"]
        .as_array()
        .expect("profiles")
        .is_empty());

    let tos_home = tempdir("show-tos-isolation");
    assert_success(&cli(
        &tos_home,
        &[
            "tos",
            "config",
            "set",
            "default.tos.access_key_id",
            "TOS_ONLY_AK",
        ],
        &[],
    ));
    let adrive_show = cli(
        &tos_home,
        &["--output", "json", "ve-adrive", "config", "show"],
        &[],
    );
    assert_success(&adrive_show);
    assert!(parse_json(&adrive_show)["data"]["profiles"]
        .as_array()
        .expect("profiles")
        .is_empty());
}

#[test]
fn config_show_rejects_missing_explicit_credentials_path() {
    let home = tempdir("missing-explicit-show");
    assert_success(&cli(
        &home,
        &["ve-adrive", "config", "set", "region", "cn-beijing"],
        &[],
    ));
    let missing = home.join("missing").join("credentials.toml");
    let missing_arg = missing.to_string_lossy().into_owned();
    let show = cli(
        &home,
        &[
            "--credentials-path",
            &missing_arg,
            "ve-adrive",
            "config",
            "show",
        ],
        &[],
    );
    assert!(!show.status.success());
    assert!(String::from_utf8_lossy(&show.stderr).contains("No credentials file found"));
}

#[test]
fn adrive_config_set_preserves_explicit_tos_credential_routing() {
    let home = tempdir("adrive-explicit-routing");
    let output = cli(
        &home,
        &[
            "ve-adrive",
            "config",
            "set",
            "default.tos.access_key_id",
            "TOS_AK_FROM_EXPLICIT_PATH",
        ],
        &[],
    );
    assert_success(&output);
    let content = std::fs::read_to_string(home.join(".tos").join("credentials.toml")).unwrap();
    assert!(content.contains("[default.tos]"));
    assert!(!content.contains("[default.adrive]"));
}

#[test]
fn adrive_config_show_includes_masked_oauth_credentials() {
    const OAUTH_USER_ID_MUST_NOT_LEAK: &str = "UNIQUE_OAUTH_USER_ID_MUST_NOT_LEAK";
    let home = tempdir("oauth-config-show");
    let config_dir = home.join(".tos");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("credentials.toml"),
        &format!(
            "schema_version = 1\n[default.adrive.oauth]\naccess_token = \"OAUTH_ACCESS_RAW\"\nrefresh_token = \"OAUTH_REFRESH_RAW\"\nuser_id = \"{OAUTH_USER_ID_MUST_NOT_LEAK}\"\n"
        ),
    )
    .unwrap();
    let show = cli(
        &home,
        &["--output", "json", "ve-adrive", "config", "show"],
        &[],
    );
    assert_success(&show);
    let json = parse_json(&show);
    let profile = &json["data"]["profiles"][0];
    assert_eq!(profile["access_token"]["source"], "credentials_file");
    assert_eq!(profile["refresh_token"]["source"], "credentials_file");
    let text = String::from_utf8_lossy(&show.stdout);
    assert!(!text.contains("OAUTH_ACCESS_RAW"), "stdout={text}");
    assert!(!text.contains("OAUTH_REFRESH_RAW"), "stdout={text}");
    // [Review Fix #2] Preserve config-show's privacy contract for identity metadata.
    assert!(!text.contains(OAUTH_USER_ID_MUST_NOT_LEAK), "stdout={text}");
    let serialized = serde_json::to_string(&json).unwrap();
    assert!(!serialized.contains(OAUTH_USER_ID_MUST_NOT_LEAK));
}

#[test]
// [Review Fix #6] Both surfaces reject an explicit ADrive namespace, but tos
// now points callers to the supported VeTos aksk/unified auth_mode contract.
fn auth_mode_config_is_rejected_by_non_adrive_surfaces() {
    for surface in ["tos", "ve-tos"] {
        let home = tempdir(&format!("auth-mode-scope-{surface}"));
        let output = cli(
            &home,
            &[
                surface,
                "config",
                "set",
                "default.adrive.auth_mode",
                "oauth",
            ],
            &[],
        );
        assert!(!output.status.success(), "surface={surface}");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let expected = if surface == "tos" {
            "auth_mode is not supported by tos"
        } else {
            "only supported by ve-adrive"
        };
        assert!(
            stderr.contains(expected),
            "surface={surface}, stderr={stderr}"
        );
        assert!(
            stderr.contains("unified"),
            "surface={surface}, stderr={stderr}"
        );
    }
}
