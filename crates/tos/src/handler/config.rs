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

//! `ve-tos-cli config` 子命令实现（对应设计文档 7.3）。
//!
//! 支持的操作：
//! - `ve-tos-cli config init [--profile NAME]` — 写入模板 profile（占位符不会被加密）
//! - `ve-tos-cli config show`                 — 展示生效配置（含来源 section 标注）
//! - `ve-tos-cli config set  <KEY> <VALUE>`    — 更新字段；支持三种 key 形式：
//!     * `region`                    → `[<active-profile>].region`
//!     * `endpoint`                  → 当前入口的 `[<active-profile>.<binary>]`
//!     * `control_endpoint`          → 当前入口的 `[<active-profile>.<binary>]`
//!     * `<profile>.<field>`         → `[<profile>].<field>`，但 TOS 专属字段特例写入当前入口的 binary section
//!     * `<profile>.<binary>.<field>`→ `[<profile>.<binary>].<field>`
//!
//! 敏感字段（access_key_id / secret_access_key / security_token）在保存时
//! 自动经 AES-256-GCM 加密为 `ENC:...`；展示时统一遮蔽为 `****` 或 `ENC:****`。

use std::collections::HashMap;

use crate::cli::meta::ConfigAction;
use crate::domain::auth::AuthMode;
use crate::handler::common::{
    active_tos_config_binary, output_result, resolve_auth_mode_from_config,
};
use tos_core::agent::describe::{CommandDescription, CommandLayer, RiskLevel};
use tos_core::agent::dryrun::{DryRunResult, Impact};
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::agent::output::OutputFormat;
use tos_core::infra::config::{
    has_only_sibling_tos_namespace, overlay_effective_credentials, redact_effective, Binary,
    ConfigFile, EffectiveProfile, Profile,
};
use tos_core::infra::credentials::{CredentialSection, CredentialsFile};

/// Handle `ve-tos config <action>`.
pub async fn handle_config_command(
    global: &GlobalArgs,
    action: &Option<ConfigAction>,
) -> Result<i32, CliError> {
    if global.describe {
        if let Some(action) = action {
            let mut desc = describe_config_action(action);
            desc.command = public_config_command(&desc.command);
            // [Review Fix #16] Route config describe through Envelope so Agents receive a stable schema.
            output_envelope(
                global,
                &Envelope::success(config_action_command(action), desc),
            )?;
        } else {
            let mut desc = describe_config_group();
            if let Some(object) = desc.as_object_mut() {
                object.insert(
                    "command".to_string(),
                    serde_json::json!(public_config_command("ve-tos config")),
                );
            }
            // [Review Fix #16] Route config group describe through the same Envelope contract.
            output_envelope(
                global,
                &Envelope::success(public_config_command("ve-tos config"), desc),
            )?;
        }
        return Ok(0);
    }

    let Some(action) = action else {
        return Err(CliError::ValidationError(
            "`ve-tos config` requires a subcommand; use `ve-tos config --help` or `ve-tos config --describe`".to_string(),
        ));
    };

    if global.dry_run {
        return handle_dry_run(global, action);
    }

    match action {
        ConfigAction::Init { profile } => handle_init(global, profile.as_deref()).await,
        ConfigAction::Show => handle_show(global).await,
        ConfigAction::Set { key, value } => handle_set(global, key, value).await,
    }
}

// =====================================================================
// --describe
// =====================================================================

fn describe_config_action(action: &ConfigAction) -> CommandDescription {
    match action {
        ConfigAction::Init { .. } => CommandDescription {
            command: "ve-tos config init".to_string(),
            layer: CommandLayer::Meta,
            api: None,
            description:
                "Initialize TOS CLI configuration with a template layered profile".to_string(),
            risk_level: RiskLevel::Low,
            supports_dry_run: true,
            supports_pipe: false,
            parameters: None,
            scenario_routing: Some(HashMap::from([
                (
                    "Initialize default profile".to_string(),
                    crate::registry::public_tos_command("ve-tos config init"),
                ),
                (
                    "Initialize named profile".to_string(),
                    crate::registry::public_tos_command("ve-tos config init --profile staging"),
                ),
            ])),
            related_commands: None,
            low_level_apis: None,
            ..Default::default()
        },
        ConfigAction::Show => CommandDescription {
            command: "ve-tos config show".to_string(),
            layer: CommandLayer::Meta,
            api: None,
            description: format!(
                "Show the effective configuration (shared + {} override) with source section annotations and redacted secrets",
                active_tos_config_binary().as_str()
            ),
            risk_level: RiskLevel::Low,
            supports_dry_run: false,
            supports_pipe: true,
            parameters: None,
            scenario_routing: Some(HashMap::from([
                (
                    "Show as table".to_string(),
                    crate::registry::public_tos_command("ve-tos config show --output table"),
                ),
                (
                    "Show as JSON".to_string(),
                    crate::registry::public_tos_command("ve-tos config show --output json"),
                ),
            ])),
            related_commands: None,
            low_level_apis: None,
            ..Default::default()
        },
        ConfigAction::Set { .. } => CommandDescription {
            command: "ve-tos config set".to_string(),
            layer: CommandLayer::Meta,
            api: None,
            description: config_set_description(),
            risk_level: RiskLevel::Low,
            supports_dry_run: true,
            supports_pipe: false,
            parameters: None,
            scenario_routing: Some(config_set_scenario_routing()),
            related_commands: None,
            low_level_apis: None,
            ..Default::default()
        },
    }
}

fn config_set_description() -> String {
    config_set_description_for(active_tos_config_binary())
}

// [Review Fix #2] Keep describe metadata testable without mutating the
// process-wide binary selector, which is shared by parallel tests.
fn config_set_description_for(binary: Binary) -> String {
    if binary == Binary::VeTos {
        "Set a configuration value. VeTos auth_mode supports aksk or unified; Unified selects the same-name external profile and uses `ve login`. Bare credential keys write to the active profile's ve-tos credentials; explicit key formats include staging.endpoint / default.ve-tos.control_endpoint".to_string()
    } else {
        "Set a configuration value. Bare credential keys write to the active profile's tos credentials; explicit key formats include staging.endpoint / default.tos.psm".to_string()
    }
}

fn config_set_scenario_routing() -> HashMap<String, String> {
    config_set_scenario_routing_for(active_tos_config_binary())
}

fn config_set_scenario_routing_for(binary: Binary) -> HashMap<String, String> {
    let mut routing = HashMap::from([
        (
            "Set shared region".to_string(),
            crate::registry::public_tos_command("ve-tos config set region cn-beijing"),
        ),
        (
            "Set active profile tos endpoint".to_string(),
            crate::registry::public_tos_command(
                "ve-tos config set endpoint tos-cn-boe.volces.com --profile dev",
            ),
        ),
        (
            "Set staging access key".to_string(),
            crate::registry::public_tos_command("ve-tos config set staging.access_key_id AKxxx"),
        ),
        (
            "Set active surface access key".to_string(),
            crate::registry::public_tos_command("ve-tos config set access_key_id AKxxx"),
        ),
    ]);
    if binary == Binary::VeTos {
        routing.insert(
            "Select Unified authentication".to_string(),
            crate::registry::public_tos_command("ve-tos config set auth_mode unified"),
        );
        routing.insert(
            "Invoke with Unified authentication".to_string(),
            crate::registry::public_tos_command("ve-tos --profile default --auth-mode unified ls"),
        );
        routing.insert(
            "Set control endpoint".to_string(),
            crate::registry::public_tos_command(
                "ve-tos config set control_endpoint tos-control-cn-beijing.volces.com",
            ),
        );
    } else {
        routing.insert(
            "Set PSM service".to_string(),
            crate::registry::public_tos_command("ve-tos config set psm toutiao.tos.tosapi"),
        );
    }
    routing
}

pub fn describe_config_group() -> serde_json::Value {
    serde_json::json!({
        "command": "ve-tos config",
        "kind": "command_group",
        "layer": "meta",
        "description": "Configuration management",
        "supports_help": true,
        "supports_describe": true,
        "subcommands": [
            {"name": "init", "risk_level": "low", "description": "Initialize configuration"},
            {"name": "show", "risk_level": "low", "description": "Show effective configuration"},
            {"name": "set", "risk_level": "low", "description": "Set configuration value"}
        ]
    })
}

// =====================================================================
// --dry-run
// =====================================================================

fn handle_dry_run(global: &GlobalArgs, action: &ConfigAction) -> Result<i32, CliError> {
    let dry_run = match action {
        ConfigAction::Init { profile } => {
            let profile_name = effective_config_init_profile(global, profile.as_deref())?;
            let path = global.config_path();
            let active_binary = active_tos_config_binary();
            let mut plan = vec![format!(
                "CREATE template config file at '{}'",
                path.display()
            )];
            if active_binary == Binary::VeTos {
                plan.extend([
                    format!(
                        "WRITE [{}] with default region + AK/SK placeholders",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.{}] section with default endpoint",
                        profile_name,
                        active_binary.as_str()
                    ),
                ]);
                // [Review Fix #4] Only `ve-tos` has a control plane endpoint;
                // ByteCloud `tos` config init must not describe one.
                plan.push(format!(
                    "DERIVE [{}.{}].control_endpoint from endpoint unless explicitly configured",
                    profile_name,
                    active_binary.as_str()
                ));
            } else {
                plan.extend([
                    format!(
                        "WRITE [{}] with AK/SK placeholders; network settings omitted",
                        profile_name
                    ),
                    format!(
                        "CREATE [{}.{}] section",
                        profile_name,
                        active_binary.as_str()
                    ),
                ]);
            }
            plan.extend([
                format!(
                    "WRITE [{}.{}].checkpoint_dir default",
                    profile_name,
                    active_binary.as_str()
                ),
                format!(
                    "WRITE [{}.{}].batch_report_dir default",
                    profile_name,
                    active_tos_config_binary().as_str()
                ),
                format!(
                    "WRITE [{}.{}].batch_report_format default",
                    profile_name,
                    active_tos_config_binary().as_str()
                ),
                format!(
                    "WRITE [{}.{}].progress_enabled default",
                    profile_name,
                    active_tos_config_binary().as_str()
                ),
                format!(
                    "WRITE [{}.{}].max_retry_count default",
                    profile_name,
                    active_tos_config_binary().as_str()
                ),
                format!(
                    "WRITE [{}.{}].requesttimeout default",
                    profile_name,
                    active_tos_config_binary().as_str()
                ),
                format!(
                    "WRITE [{}.{}].connecttimeout default",
                    profile_name,
                    active_tos_config_binary().as_str()
                ),
                format!(
                    "WRITE [{}.{}].maxconnections default",
                    profile_name,
                    active_tos_config_binary().as_str()
                ),
            ]);
            DryRunResult {
                action: "config init".to_string(),
                dry_run: true,
                impact: Impact {
                    affected_objects: 0,
                    affected_bytes: 0,
                    risk_level: "low".to_string(),
                    estimated_duration: Some("< 1s".to_string()),
                    scanned_count: None,
                    preview_truncated: None,
                },
                plan,
                warnings: if path.exists() {
                    vec![format!(
                        "Config file already exists at '{}'; existing profiles are preserved",
                        path.display()
                    )]
                } else {
                    vec![]
                },
                confirm_command: Some(config_init_confirm_command(profile_name)),
            }
        }
        ConfigAction::Set { key, value } => {
            let segs = parse_key_path_for_tos(key, &global.profile)?;
            let segs_ref: Vec<&str> = segs.iter().map(|s| s.as_str()).collect();
            let mut validation_config = ConfigFile::default();
            validation_config.set_by_path(&segs_ref, value)?;
            // [Review Fix #17] Never echo config secrets in dry-run plans or confirm commands.
            let redacted_value = redact_config_value(key, value);
            let plan_line = if is_sensitive_key(key) {
                let section = credential_section_for_tos_path(&segs)?;
                format!(
                    "SET {}.{} in '{}' = '{}'",
                    credential_section_path(&segs[0], section),
                    segs.last().unwrap_or(key),
                    global.credentials_path().display(),
                    redacted_value
                )
            } else {
                match segs.len() {
                    2 => format!("SET [{}].{} = '{}'", segs[0], segs[1], redacted_value),
                    3 => format!(
                        "SET [{}.{}].{} = '{}'",
                        segs[0], segs[1], segs[2], redacted_value
                    ),
                    _ => format!("SET {} = '{}'", key, redacted_value),
                }
            };
            DryRunResult {
                action: "config set".to_string(),
                dry_run: true,
                impact: Impact {
                    affected_objects: 0,
                    affected_bytes: 0,
                    risk_level: "low".to_string(),
                    estimated_duration: Some("< 1s".to_string()),
                    scanned_count: None,
                    preview_truncated: None,
                },
                plan: vec![plan_line],
                warnings: if is_sensitive_key(key) {
                    vec![
                        "Secret values are encrypted with AES-256-GCM and stored as ENC:... on disk"
                            .to_string(),
                    ]
                } else {
                    vec![]
                },
                confirm_command: Some(config_set_confirm_command(global, key, redacted_value)),
            }
        }
        ConfigAction::Show => DryRunResult {
            action: "config show".to_string(),
            dry_run: true,
            impact: Impact {
                affected_objects: 0,
                affected_bytes: 0,
                risk_level: "low".to_string(),
                estimated_duration: Some("< 1s".to_string()),
                scanned_count: None,
                preview_truncated: None,
            },
            plan: vec![
                format!(
                    "READ config file and resolve effective profile (shared + {} override)",
                    active_tos_config_binary().as_str()
                ),
                "Display with source section annotations; secrets redacted".to_string(),
            ],
            warnings: vec![],
            confirm_command: Some(crate::registry::public_tos_command("ve-tos config show")),
        },
    };

    output_envelope(
        global,
        &Envelope::success(config_action_command(action), dry_run),
    )?;
    Ok(0)
}

// =====================================================================
// config init
// =====================================================================

async fn handle_init(global: &GlobalArgs, profile: Option<&str>) -> Result<i32, CliError> {
    use tos_core::infra::config::{
        TosOverride, DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS, DEFAULT_HTTP_MAX_CONNECTIONS,
        DEFAULT_HTTP_MAX_RETRY_COUNT, DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS,
        DEFAULT_TOS_BATCH_REPORT_DIR, DEFAULT_TOS_BATCH_REPORT_FORMAT, DEFAULT_TOS_CHECKPOINT_DIR,
        DEFAULT_TOS_PROGRESS_ENABLED,
    };

    let profile_name = effective_config_init_profile(global, profile)?;
    let path = global.config_path();

    let mut config = ConfigFile::load_from(&path)?;
    let created = !config.profiles.contains_key(profile_name);
    let active_binary = active_tos_config_binary();
    {
        let p = config.get_or_insert_profile(profile_name);
        // Only ve-tos has a stable production default pair. ByteTOS deployments
        // must select endpoint or PSM explicitly.
        if active_binary == Binary::VeTos && p.region.is_none() {
            p.region = Some("cn-beijing".to_string());
        }
        // 明显的占位符以 `<...>` 包裹，save() 时不会被加密
        if p.access_key_id.is_none() {
            p.access_key_id = Some("<YOUR_ACCESS_KEY_ID>".to_string());
        }
        if p.secret_access_key.is_none() {
            p.secret_access_key = Some("<YOUR_SECRET_ACCESS_KEY>".to_string());
        }
        let tos_override = match active_binary {
            Binary::VeTos => p.ve_tos.get_or_insert_with(TosOverride::default),
            _ => p.tos.get_or_insert_with(TosOverride::default),
        };
        if active_binary == Binary::VeTos && tos_override.endpoint.is_none() {
            tos_override.endpoint = Some("tos-cn-beijing.volces.com".to_string());
        }
        if tos_override.checkpoint_dir.is_none() {
            tos_override.checkpoint_dir = Some(DEFAULT_TOS_CHECKPOINT_DIR.to_string());
        }
        if tos_override.batch_report_dir.is_none() {
            tos_override.batch_report_dir = Some(DEFAULT_TOS_BATCH_REPORT_DIR.to_string());
        }
        if tos_override.batch_report_format.is_none() {
            tos_override.batch_report_format = Some(DEFAULT_TOS_BATCH_REPORT_FORMAT.to_string());
        }
        if tos_override.progress_enabled.is_none() {
            tos_override.progress_enabled = Some(DEFAULT_TOS_PROGRESS_ENABLED);
        }
        if tos_override.max_retry_count.is_none() {
            tos_override.max_retry_count = Some(DEFAULT_HTTP_MAX_RETRY_COUNT);
        }
        if tos_override.requesttimeout.is_none() {
            tos_override.requesttimeout = Some(DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS);
        }
        if tos_override.connecttimeout.is_none() {
            tos_override.connecttimeout = Some(DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS);
        }
        if tos_override.maxconnections.is_none() {
            tos_override.maxconnections = Some(DEFAULT_HTTP_MAX_CONNECTIONS);
        }
    }
    config.save_to_path(&path)?;

    let data = serde_json::json!({
        "config_path": path.display().to_string(),
        "profile": profile_name,
        "created": created,
        "layout": {
            "shared_section": format!("[{}]", profile_name),
            "binary_overrides": [
                format!("[{}.{}]", profile_name, active_tos_config_binary().as_str()),
                format!("[{}.tosvector]", profile_name),
                format!("[{}.tostable]", profile_name),
                format!("[{}.adrive]", profile_name),
            ],
        },
        "message": format!(
            "Config file written to {}. Edit it or run '{}' to fill in credentials, then verify with '{}'.",
            path.display(),
            public_config_command("ve-tos config set <KEY> <VALUE>"),
            public_config_command("ve-tos config show")
        ),
    });

    let envelope = Envelope::success(public_config_command("ve-tos config init"), data);
    output_envelope(global, &envelope)?;
    Ok(0)
}

// =====================================================================
// config show
// =====================================================================

#[derive(serde::Serialize)]
struct ConfigShowData {
    config_path: String,
    credentials_path: String,
    profiles: Vec<EffectiveProfile>,
}

async fn handle_show(global: &GlobalArgs) -> Result<i32, CliError> {
    let path = global.config_path();
    let config_dir = ConfigFile::config_dir_from_path(&path);
    let config = ConfigFile::load_from(&path)?;
    let binary = active_tos_config_binary();
    let selected_mode = if binary == Binary::VeTos {
        resolve_auth_mode_from_config(global, &config)?.mode
    } else {
        AuthMode::Aksk
    };
    let credentials_path = global.credentials_path();
    let effective = match selected_mode {
        // [Review Fix #6] A unified inspection is selected-profile-only and
        // never validates, reads, or decrypts the local credentials store.
        AuthMode::Unified => {
            if config.profiles.is_empty() {
                return Err(config_show_missing_error(&path));
            }
            build_unified_config_show_profiles(global, &config, &config_dir, binary)?
        }
        AuthMode::Aksk => {
            let existing_path = global.existing_runtime_credentials_path()?;
            let credentials = CredentialsFile::load_from(&existing_path)?;
            if config.profiles.is_empty() && credentials.profile_names().is_empty() {
                return Err(config_show_missing_error(&path));
            }
            build_aksk_config_show_profiles(AkskConfigShowContext {
                config: &config,
                config_dir: &config_dir,
                binary,
                credentials_path: &credentials_path,
                credentials: &credentials,
                selected_profile: &global.profile,
            })?
        }
    };

    let format = global.output.unwrap_or_else(OutputFormat::auto_detect);
    match format {
        OutputFormat::Table => {
            println!(
                "Config file: {}\nCredentials file: {}\n",
                path.display(),
                credentials_path.display()
            );
            let headers = &["PROFILE", "FIELD", "VALUE", "SOURCE"];
            let mut rows: Vec<Vec<String>> = Vec::new();
            for eff in &effective {
                push_traced_row(&mut rows, eff, "region", &eff.region);
                push_traced_row(&mut rows, eff, "endpoint", &eff.endpoint);
                if binary == Binary::Tos {
                    push_traced_row(&mut rows, eff, "psm", &eff.psm);
                    push_traced_row(&mut rows, eff, "idc", &eff.idc);
                    push_traced_row(&mut rows, eff, "cluster", &eff.cluster);
                    push_traced_row(&mut rows, eff, "addr_family", &eff.addr_family);
                }
                if binary == Binary::VeTos {
                    // [Review Fix #1] `control_endpoint` is only meaningful on
                    // the `ve-tos` control plane; the ByteCloud `tos` view
                    // should not render a placeholder row for it.
                    push_traced_row(&mut rows, eff, "control_endpoint", &eff.control_endpoint);
                }
                push_traced_row(&mut rows, eff, "checkpoint_dir", &eff.checkpoint_dir);
                push_traced_row(&mut rows, eff, "batch_report_dir", &eff.batch_report_dir);
                push_traced_row(
                    &mut rows,
                    eff,
                    "batch_report_format",
                    &eff.batch_report_format,
                );
                push_traced_bool_row(&mut rows, eff, "progress_enabled", &eff.progress_enabled);
                push_traced_value_row(&mut rows, eff, "max_retry_count", &eff.max_retry_count);
                push_traced_value_row(&mut rows, eff, "requesttimeout", &eff.requesttimeout);
                push_traced_value_row(&mut rows, eff, "connecttimeout", &eff.connecttimeout);
                push_traced_value_row(&mut rows, eff, "maxconnections", &eff.maxconnections);
                push_traced_row(&mut rows, eff, "access_key_id", &eff.access_key_id);
                push_traced_row(&mut rows, eff, "secret_access_key", &eff.secret_access_key);
                push_traced_row(&mut rows, eff, "security_token", &eff.security_token);
            }
            println!("{}", tos_core::agent::output::format_table(headers, &rows));
        }
        _ => {
            let show_data = ConfigShowData {
                config_path: path.display().to_string(),
                credentials_path: credentials_path.display().to_string(),
                profiles: effective,
            };
            let envelope =
                Envelope::success(public_config_command("ve-tos config show"), show_data);
            output_result(global, &envelope)?;
        }
    }
    Ok(0)
}

fn config_show_missing_error(path: &std::path::Path) -> CliError {
    CliError::ConfigMissing(format!(
        "No config file found at {}. Run '{}' to create one.",
        path.display(),
        crate::registry::public_tos_command("ve-tos config init")
    ))
}

fn build_unified_config_show_profiles(
    global: &GlobalArgs,
    config: &ConfigFile,
    config_dir: &std::path::Path,
    binary: Binary,
) -> Result<Vec<EffectiveProfile>, CliError> {
    let effective = config.get_effective_profile_without_credentials_in_dir(
        &global.profile,
        binary,
        config_dir,
    )?;
    Ok(vec![redact_effective(effective)])
}

struct AkskConfigShowContext<'a> {
    config: &'a ConfigFile,
    config_dir: &'a std::path::Path,
    binary: Binary,
    credentials_path: &'a std::path::Path,
    credentials: &'a CredentialsFile,
    selected_profile: &'a str,
}

fn build_aksk_config_show_profiles(
    context: AkskConfigShowContext<'_>,
) -> Result<Vec<EffectiveProfile>, CliError> {
    let mut display_config = context.config.clone();
    for profile_name in context.credentials.profile_names() {
        display_config.get_or_insert_profile(&profile_name);
    }
    let mut effective = Vec::new();
    for (profile_name, profile) in &display_config.profiles {
        let is_unified_sibling = profile_name != context.selected_profile
            && configured_unified_profile(profile, context.binary)?;
        let stored = if is_unified_sibling {
            Default::default()
        } else {
            context.credentials.effective_aksk(
                profile_name,
                credential_section_for_binary(context.binary),
                context.credentials_path,
            )?
        };
        if !context.config.profiles.contains_key(profile_name) && stored.is_empty() {
            continue;
        }
        if has_only_sibling_tos_namespace(profile, context.binary) && stored.is_empty() {
            continue;
        }
        let mut profile = if is_unified_sibling {
            display_config.get_effective_profile_without_credentials_in_dir(
                profile_name,
                context.binary,
                context.config_dir,
            )?
        } else {
            display_config.get_effective_profile_in_dir(
                profile_name,
                context.binary,
                context.config_dir,
            )?
        };
        if !is_unified_sibling {
            overlay_effective_credentials(&mut profile, &stored);
        }
        effective.push(redact_effective(profile));
    }
    Ok(effective)
}

fn configured_unified_profile(profile: &Profile, binary: Binary) -> Result<bool, CliError> {
    if binary != Binary::VeTos {
        return Ok(false);
    }
    // [Review Fix #6] A unified sibling remains non-secret even while the
    // selected invocation uses AKSK and displays other credential profiles.
    profile
        .ve_tos
        .as_ref()
        .and_then(|settings| settings.auth_mode.as_deref())
        .map(|value| AuthMode::parse(value, "profile config").map(|mode| mode == AuthMode::Unified))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn credential_section_for_binary(binary: Binary) -> CredentialSection {
    match binary {
        Binary::Tos => CredentialSection::Tos,
        Binary::VeTos => CredentialSection::VeTos,
        Binary::Adrive => CredentialSection::ADrive,
        Binary::TosVector | Binary::TosTable => CredentialSection::Shared,
    }
}

fn push_traced_row(
    rows: &mut Vec<Vec<String>>,
    eff: &EffectiveProfile,
    field: &str,
    tf: &tos_core::infra::config::TracedField<String>,
) {
    let source = tf.source.label(&eff.profile_name, &eff.binary);
    rows.push(vec![
        eff.profile_name.clone(),
        field.to_string(),
        tf.value.clone().unwrap_or_else(|| "-".to_string()),
        source,
    ]);
}

fn push_traced_bool_row(
    rows: &mut Vec<Vec<String>>,
    eff: &EffectiveProfile,
    field: &str,
    tf: &tos_core::infra::config::TracedField<bool>,
) {
    let source = tf.source.label(&eff.profile_name, &eff.binary);
    rows.push(vec![
        eff.profile_name.clone(),
        field.to_string(),
        tf.value
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".to_string()),
        source,
    ]);
}

fn push_traced_value_row<T>(
    rows: &mut Vec<Vec<String>>,
    eff: &EffectiveProfile,
    field: &str,
    tf: &tos_core::infra::config::TracedField<T>,
) where
    T: Clone + serde::Serialize + ToString,
{
    let source = tf.source.label(&eff.profile_name, &eff.binary);
    rows.push(vec![
        eff.profile_name.clone(),
        field.to_string(),
        tf.value
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "-".to_string()),
        source,
    ]);
}

// =====================================================================
// config set
// =====================================================================

async fn handle_set(global: &GlobalArgs, key: &str, value: &str) -> Result<i32, CliError> {
    let segs = parse_key_path_for_tos(key, &global.profile)?;
    if is_sensitive_key(key) {
        return handle_credential_set(global, value, &segs);
    }
    let segs_ref: Vec<&str> = segs.iter().map(|s| s.as_str()).collect();

    let path = global.config_path();
    let mut config = ConfigFile::load_from(&path)?;
    config.set_by_path(&segs_ref, value)?;
    config.save_to_path(&path)?;

    let section = match segs.len() {
        2 => format!("[{}]", segs[0]),
        3 => format!("[{}.{}]", segs[0], segs[1]),
        _ => "?".to_string(),
    };
    let field = segs.last().map(|s| s.as_str()).unwrap_or("?");

    let encrypted = is_sensitive_key(key);
    let displayed_value = if encrypted { "ENC:****" } else { value };

    let data = serde_json::json!({
        "section": section,
        "field": field,
        "value": displayed_value,
        "encrypted": encrypted,
        "config_path": path.display().to_string(),
        "message": format!(
            "Set {}.{} = '{}'",
            section, field, displayed_value
        ),
    });

    let envelope = Envelope::success(public_config_command("ve-tos config set"), data);
    output_envelope(global, &envelope)?;
    Ok(0)
}

fn handle_credential_set(
    global: &GlobalArgs,
    value: &str,
    segments: &[String],
) -> Result<i32, CliError> {
    let profile_name = segments
        .first()
        .ok_or_else(|| CliError::ValidationError("missing credential profile".to_string()))?;
    let field = segments
        .last()
        .ok_or_else(|| CliError::ValidationError("missing credential field".to_string()))?;
    let section = credential_section_for_tos_path(segments)?;
    let credentials_path = global.credentials_path();
    let mut credentials = CredentialsFile::load_from(&credentials_path)?;
    credentials.set_aksk_field(profile_name, section, field, value)?;
    credentials.save_to_path(&credentials_path)?;

    let data = serde_json::json!({
        "section": credential_section_path(profile_name, section),
        "field": field,
        "value": "ENC:****",
        "encrypted": true,
        "config_path": global.config_path().display().to_string(),
        "credentials_path": credentials_path.display().to_string(),
        "message": format!("Saved {field} to credentials"),
    });
    let envelope = Envelope::success(public_config_command("ve-tos config set"), data);
    output_envelope(global, &envelope)?;
    Ok(0)
}

fn credential_section_for_tos_path(path: &[String]) -> Result<CredentialSection, CliError> {
    match path {
        [_profile, _field] => Ok(CredentialSection::Shared),
        [_profile, binary, _field] => match Binary::parse(binary) {
            Some(Binary::Tos) => Ok(CredentialSection::Tos),
            Some(Binary::VeTos) => Ok(CredentialSection::VeTos),
            Some(Binary::Adrive) => Ok(CredentialSection::ADrive),
            _ => Err(CliError::ValidationError(format!(
                "unsupported credential namespace '{binary}'"
            ))),
        },
        _ => Err(CliError::ValidationError(
            "invalid credential key path".to_string(),
        )),
    }
}

fn credential_section_path(profile_name: &str, section: CredentialSection) -> String {
    match section {
        CredentialSection::Shared => format!("[{profile_name}]"),
        CredentialSection::Tos => format!("[{profile_name}.tos]"),
        CredentialSection::VeTos => format!("[{profile_name}.ve-tos]"),
        CredentialSection::ADrive => format!("[{profile_name}.adrive]"),
    }
}

// =====================================================================
// Helpers
// =====================================================================

/// 将 `ve-tos-cli config set` 的 key 解析为 1~3 段，并应用当前 TOS 入口的专属 section 默认路由规则。
///
/// - `region`               → ["<active-profile>", "region"]
/// - `access_key_id`        → ["<active-profile>", "<active-binary>", "access_key_id"]
/// - `endpoint`             → ["<active-profile>", "<active-binary>", "endpoint"]
/// - `control_endpoint`     → ["<active-profile>", "<active-binary>", "control_endpoint"]
/// - `account_id`           → ["<active-profile>", "<active-binary>", "account_id"]
/// - `checkpoint_dir`       → ["<active-profile>", "<active-binary>", "checkpoint_dir"]
/// - `progress_enabled`     → ["<active-profile>", "<active-binary>", "progress_enabled"]
/// - `max_retry_count`      → ["<active-profile>", "<active-binary>", "max_retry_count"]
/// - `requesttimeout`       → ["<active-profile>", "<active-binary>", "requesttimeout"]
/// - `connecttimeout`       → ["<active-profile>", "<active-binary>", "connecttimeout"]
/// - `maxconnections`       → ["<active-profile>", "<active-binary>", "maxconnections"]
/// - `auth_mode`            → ["<active-profile>", "<active-binary>", "auth_mode"]
/// - `staging.region`       → ["staging", "region"]
/// - `staging.endpoint`     → ["staging", "<active-binary>", "endpoint"]
/// - `staging.control_endpoint` → ["staging", "<active-binary>", "control_endpoint"]
/// - `staging.account_id`   → ["staging", "<active-binary>", "account_id"]
/// - `default.tos.endpoint` → ["default", "tos", "endpoint"]（显式指定）
fn parse_key_path_for_tos(key: &str, active_profile: &str) -> Result<Vec<String>, CliError> {
    parse_key_path_for_tos_binary(key, active_profile, active_tos_config_binary())
}

fn parse_key_path_for_tos_binary(
    key: &str,
    active_profile: &str,
    active_binary: Binary,
) -> Result<Vec<String>, CliError> {
    // [Review Fix #2] Keep validation separate from binary routing so each
    // helper remains focused and under the project function-size limit.
    let parts = parse_config_key_parts(key, active_profile)?;
    // [Review Fix #5] 技术方案 7.3 规定 TOS 专属字段默认写入当前入口的
    // binary override，而不是 shared `[profile]`。
    let routed = match parts.as_slice() {
        [field] if is_aksk_credential_key(field) => {
            vec![
                active_profile.to_string(),
                active_binary.as_str().to_string(),
                field.clone(),
            ]
        }
        [field] if is_default_tos_override_key(field) => {
            vec![
                active_profile.to_string(),
                active_binary.as_str().to_string(),
                field.clone(),
            ]
        }
        [field] => vec![active_profile.to_string(), field.clone()],
        [profile, field] if is_default_tos_override_key(field) => {
            vec![
                profile.clone(),
                active_binary.as_str().to_string(),
                field.clone(),
            ]
        }
        _ => parts,
    };
    reject_unsupported_tos_control_endpoint(&routed)?;
    reject_unsupported_psm_config_fields(&routed, active_binary)?;
    validate_auth_mode_route(&routed, active_binary)?;
    Ok(routed)
}

fn parse_config_key_parts(key: &str, active_profile: &str) -> Result<Vec<String>, CliError> {
    if active_profile.is_empty() {
        // [Review Fix #18] Bare config keys need a concrete destination profile.
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }

    let parts: Vec<String> = key.split('.').map(|s| s.to_string()).collect();
    if parts.is_empty() || parts.iter().any(|p| p.is_empty()) {
        return Err(CliError::ValidationError(format!(
            "Invalid config key '{}'. Expected: <field> or <profile>.<field> or <profile>.<binary>.<field>",
            key
        )));
    }
    if parts.len() > 3 {
        return Err(CliError::ValidationError(format!(
            "Invalid config key '{}': too many segments (max 3)",
            key
        )));
    }
    Ok(parts)
}

fn is_aksk_credential_key(field: &str) -> bool {
    matches!(
        field,
        "access_key_id" | "secret_access_key" | "security_token"
    )
}

fn validate_auth_mode_route(path: &[String], active_binary: Binary) -> Result<(), CliError> {
    if path.last().map(String::as_str) != Some("auth_mode") {
        return Ok(());
    }
    if active_binary == Binary::Tos {
        return Err(CliError::ValidationError(
            "auth_mode is not supported by tos; use `ve-tos config set auth_mode <aksk|unified>`"
                .to_string(),
        ));
    }
    if path.len() == 3 && Binary::parse(&path[1]) == Some(Binary::VeTos) {
        return Ok(());
    }
    if path.len() == 3 && Binary::parse(&path[1]) == Some(Binary::Adrive) {
        return Err(CliError::ValidationError(
            "auth_mode for adrive is only supported by ve-adrive; use `ve-adrive config set auth_mode <aksk|oauth|unified>`".to_string(),
        ));
    }
    let namespace = path.get(1).map(String::as_str).unwrap_or("shared");
    Err(CliError::ValidationError(format!(
        "auth_mode is not supported in the {namespace} config namespace"
    )))
}

fn reject_unsupported_tos_control_endpoint(path: &[String]) -> Result<(), CliError> {
    if path.len() == 3 && path[1] == Binary::Tos.as_str() && path[2] == "control_endpoint" {
        // [Review Fix #3] `control_endpoint` belongs to the `ve-tos` control
        // plane. Reject new ByteCloud `tos` writes instead of creating a
        // hidden config value that `ve-tos config show` intentionally omits.
        return Err(CliError::ValidationError(
            "control_endpoint is only supported by ve-tos; use `ve-tos config set control_endpoint <value>`"
                .to_string(),
        ));
    }
    Ok(())
}

fn reject_unsupported_psm_config_fields(
    path: &[String],
    active_binary: Binary,
) -> Result<(), CliError> {
    let Some(field) = path.last() else {
        return Ok(());
    };
    // [Review Fix #3] The ve-tos entry must not write PSM fields even through
    // explicit sibling paths such as `default.tos.psm`.
    if active_binary != Binary::Tos && is_psm_config_field(field) {
        return Err(CliError::ValidationError(
            "PSM config fields are only supported by tos".to_string(),
        ));
    }
    if path.len() == 3 && path[1] == Binary::VeTos.as_str() && is_psm_config_field(field) {
        return Err(CliError::ValidationError(
            "PSM config fields are only supported by tos".to_string(),
        ));
    }
    Ok(())
}

fn is_psm_config_field(field: &str) -> bool {
    matches!(
        field,
        "psm" | "idc" | "cluster" | "addr_family" | "addr-family"
    )
}

fn is_default_tos_override_key(field: &str) -> bool {
    matches!(
        field,
        "endpoint"
            | "psm"
            | "idc"
            | "cluster"
            | "addr_family"
            | "addr-family"
            | "control_endpoint"
            | "account_id"
            | "checkpoint_dir"
            | "batch_report_dir"
            | "batch_report_format"
            | "progress_enabled"
            | "max_retry_count"
            | "requesttimeout"
            | "request_timeout"
            | "connecttimeout"
            | "connect_timeout"
            | "maxconnections"
            | "max_connections"
            | "auth_mode"
    )
}

/// 判断 key 末段是否是敏感字段（决定是否加密/遮蔽）。
fn is_sensitive_key(key: &str) -> bool {
    let last = key.rsplit('.').next().unwrap_or("");
    tos_core::infra::config::is_sensitive_field(last)
}

fn redact_config_value<'a>(key: &str, value: &'a str) -> &'a str {
    if is_sensitive_key(key) {
        "***REDACTED***"
    } else {
        value
    }
}

fn config_set_confirm_command(global: &GlobalArgs, key: &str, redacted_value: &str) -> String {
    let profile_arg = if key.contains('.') || global.profile == "default" {
        String::new()
    } else {
        format!(" --profile {}", global.profile)
    };
    format!(
        "{} {} {}{}",
        crate::registry::public_tos_command("ve-tos config set"),
        key,
        redacted_value,
        profile_arg
    )
}

fn config_init_confirm_command(profile_name: &str) -> String {
    if profile_name == "default" {
        crate::registry::public_tos_command("ve-tos config init")
    } else {
        format!(
            "{} --profile {}",
            crate::registry::public_tos_command("ve-tos config init"),
            profile_name
        )
    }
}

fn effective_config_init_profile<'a>(
    global: &'a GlobalArgs,
    profile: Option<&'a str>,
) -> Result<&'a str, CliError> {
    let profile_name = profile.unwrap_or(global.profile.as_str());
    if profile_name.is_empty() {
        // [Review Fix #19] Global --profile must select the init target when no
        // command-local profile is supplied, and an empty profile must not
        // create an empty TOML section.
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }
    Ok(profile_name)
}

fn public_config_command(command: &str) -> String {
    // [Review Fix #7] Shared config handlers must render the active public
    // surface (`tos` vs `ve-tos`) instead of leaking the implementation prefix.
    if active_tos_config_binary() == Binary::Tos {
        command
            .strip_prefix("ve-tos ")
            .map(|suffix| format!("tos {suffix}"))
            .unwrap_or_else(|| command.to_string())
    } else {
        command.to_string()
    }
}

fn config_action_command(action: &ConfigAction) -> String {
    let command = match action {
        ConfigAction::Init { .. } => "ve-tos config init",
        ConfigAction::Show => "ve-tos config show",
        ConfigAction::Set { .. } => "ve-tos config set",
    };
    public_config_command(command)
}

/// Output an Envelope respecting the --output format.
fn output_envelope<T: serde::Serialize>(
    global: &GlobalArgs,
    envelope: &Envelope<T>,
) -> Result<(), CliError> {
    output_result(global, envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static CONFIG_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn config_describe_documents_unified_only_for_ve_tos() {
        let ve_tos_description = config_set_description_for(Binary::VeTos);
        let ve_tos_routing = config_set_scenario_routing_for(Binary::VeTos);
        assert!(ve_tos_description.contains("aksk or unified"));
        assert!(ve_tos_description.contains("ve login"));
        assert!(ve_tos_routing
            .values()
            .any(|command| command.contains("auth_mode unified")));

        assert!(!config_set_description_for(Binary::Tos).contains("unified"));
        assert!(!config_set_scenario_routing_for(Binary::Tos)
            .values()
            .any(|command| command.contains("unified")));
    }

    #[tokio::test]
    async fn unified_config_show_ignores_malformed_credentials_and_encrypted_config_secrets() {
        let _lock = CONFIG_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let original_binary = std::env::var_os("VE_STORAGE_UNI_TOS_CONFIG_BINARY");
        std::env::set_var("VE_STORAGE_UNI_TOS_CONFIG_BINARY", "ve-tos");
        let directory = std::env::temp_dir().join(format!(
            "ve-tos-unified-config-show-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        let credentials_path = directory.join("malformed-credentials.toml");
        std::fs::write(
            &config_path,
            concat!(
                "[selected]\n",
                "region = \"cn-beijing\"\n",
                "access_key_id = \"ENC:not-valid\"\n",
                "secret_access_key = \"ENC:not-valid\"\n",
                "security_token = \"ENC:not-valid\"\n",
                "[selected.ve-tos]\n",
                "auth_mode = \"unified\"\n",
                "endpoint = \"https://tos.example.com\"\n",
                "[unselected]\n",
                "access_key_id = \"ENC:not-valid\"\n",
                "secret_access_key = \"ENC:not-valid\"\n",
                "[unselected.ve-tos]\n",
                "auth_mode = \"aksk\"\n",
            ),
        )
        .unwrap();
        std::fs::write(&credentials_path, "[this is not valid TOML").unwrap();
        let mut global = GlobalArgs::default();
        global.profile = "selected".to_string();
        global.config_path = Some(config_path);
        global.credentials_path = Some(credentials_path.clone());
        global.ve_tos_auth_mode = Some("unified".to_string());
        global.output = Some(OutputFormat::Json);

        let result = handle_show(&global).await;

        assert!(result.is_ok(), "result={result:?}");
        assert_eq!(
            std::fs::read_to_string(&credentials_path).unwrap(),
            "[this is not valid TOML"
        );
        assert!(!directory.join(".key").exists());
        match original_binary {
            Some(value) => std::env::set_var("VE_STORAGE_UNI_TOS_CONFIG_BINARY", value),
            None => std::env::remove_var("VE_STORAGE_UNI_TOS_CONFIG_BINARY"),
        }
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn unified_config_show_profiles_only_include_selected_profile() {
        let directory = std::env::temp_dir().join(format!(
            "ve-tos-unified-selected-show-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let mut config = ConfigFile::default();
        let selected = config.get_or_insert_profile("selected");
        selected.region = Some("cn-beijing".to_string());
        selected.access_key_id = Some("ENC:not-valid".to_string());
        selected.secret_access_key = Some("ENC:not-valid".to_string());
        selected
            .ve_tos
            .get_or_insert_with(Default::default)
            .auth_mode = Some("unified".to_string());
        let unselected = config.get_or_insert_profile("unselected");
        unselected.access_key_id = Some("ENC:not-valid".to_string());
        unselected.secret_access_key = Some("ENC:not-valid".to_string());

        let mut global = GlobalArgs::default();
        global.profile = "selected".to_string();
        let profiles =
            build_unified_config_show_profiles(&global, &config, &directory, Binary::VeTos)
                .unwrap();

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].profile_name, "selected");
        assert_eq!(profiles[0].region.value.as_deref(), Some("cn-beijing"));
        assert!(profiles[0].access_key_id.value.is_none());
        assert!(profiles[0].secret_access_key.value.is_none());
        assert!(profiles[0].security_token.value.is_none());
        assert!(!directory.join(".key").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn aksk_config_show_keeps_selected_credentials_and_isolates_unified_sibling() {
        let directory = std::env::temp_dir().join(format!(
            "ve-tos-mixed-auth-show-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let credentials_path = directory.join("credentials.toml");
        let mut config = ConfigFile::default();
        config
            .get_or_insert_profile("selected")
            .ve_tos
            .get_or_insert_with(Default::default)
            .auth_mode = Some("unified".to_string());
        let unified = config.get_or_insert_profile("unified");
        unified.access_key_id = Some("ENC:not-valid".to_string());
        unified.secret_access_key = Some("ENC:not-valid".to_string());
        unified
            .ve_tos
            .get_or_insert_with(Default::default)
            .auth_mode = Some("unified".to_string());

        let mut credentials = CredentialsFile::default();
        for (profile_name, access_key, secret_key) in [
            ("selected", "selected-ak", "selected-sk"),
            ("unified", "unified-ak", "unified-sk"),
        ] {
            credentials
                .set_aksk_field(
                    profile_name,
                    CredentialSection::VeTos,
                    "access_key_id",
                    access_key,
                )
                .unwrap();
            credentials
                .set_aksk_field(
                    profile_name,
                    CredentialSection::VeTos,
                    "secret_access_key",
                    secret_key,
                )
                .unwrap();
        }
        credentials.save_to_path(&credentials_path).unwrap();
        let loaded = CredentialsFile::load_from(&credentials_path).unwrap();

        let profiles = build_aksk_config_show_profiles(AkskConfigShowContext {
            config: &config,
            config_dir: &directory,
            binary: Binary::VeTos,
            credentials_path: &credentials_path,
            credentials: &loaded,
            selected_profile: "selected",
        })
        .unwrap();
        let selected = profiles
            .iter()
            .find(|profile| profile.profile_name == "selected")
            .unwrap();
        let unified = profiles
            .iter()
            .find(|profile| profile.profile_name == "unified")
            .unwrap();

        assert!(selected.access_key_id.value.is_some());
        assert_eq!(
            selected.access_key_id.source,
            tos_core::infra::config::FieldSource::CredentialsFile
        );
        assert!(selected.secret_access_key.value.is_some());
        assert!(unified.access_key_id.value.is_none());
        assert!(unified.secret_access_key.value.is_none());
        assert!(unified.security_token.value.is_none());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn bare_ve_tos_auth_mode_routes_to_active_profile_override() {
        assert_eq!(
            parse_key_path_for_tos_binary("auth_mode", "staging", Binary::VeTos).unwrap(),
            ["staging", "ve-tos", "auth_mode"]
        );
        assert_eq!(
            parse_key_path_for_tos_binary("dev.auth_mode", "default", Binary::VeTos).unwrap(),
            ["dev", "ve-tos", "auth_mode"]
        );
    }

    #[test]
    fn tos_surface_rejects_auth_mode_for_bare_and_explicit_namespaces() {
        for key in [
            "auth_mode",
            "default.auth_mode",
            "default.tos.auth_mode",
            "default.ve-tos.auth_mode",
        ] {
            let error = parse_key_path_for_tos_binary(key, "default", Binary::Tos).unwrap_err();
            assert_eq!(
                error.to_string(),
                "Validation error: auth_mode is not supported by tos; use `ve-tos config set auth_mode <aksk|unified>`",
                "key={key}"
            );
        }
    }

    #[test]
    fn ve_tos_surface_rejects_auth_mode_for_other_service_namespaces() {
        let adrive_error =
            parse_key_path_for_tos_binary("default.adrive.auth_mode", "default", Binary::VeTos)
                .unwrap_err();
        assert_eq!(
            adrive_error.to_string(),
            "Validation error: auth_mode for adrive is only supported by ve-adrive; use `ve-adrive config set auth_mode <aksk|oauth|unified>`"
        );

        for namespace in ["tos", "tosvector", "tostable"] {
            let key = format!("default.{namespace}.auth_mode");
            let error = parse_key_path_for_tos_binary(&key, "default", Binary::VeTos).unwrap_err();
            assert_eq!(
                error.to_string(),
                format!(
                    "Validation error: auth_mode is not supported in the {namespace} config namespace"
                )
            );
        }
    }
}
