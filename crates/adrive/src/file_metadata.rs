//! 文件资源 Low-Level 命令的能力发现、Describe 和 MCP 类型契约。
use crate::registry::{CapabilityRow, RegistryParameter};
use serde_json::{json, Value};
use tos_core::agent::error::CliError;

const PATH: RegistryParameter = RegistryParameter {
    name: "path",
    required: true,
    description: "Exact file URI: adrive://instance/space/path",
};

const META: RegistryParameter = RegistryParameter {
    name: "meta",
    required: true,
    description: crate::cli::high_level::USERMETA_HELP,
};

const BY_NAME: RegistryParameter = RegistryParameter {
    name: "by-name",
    required: false,
    description: "Treat ADrive instance/space target segments as names and resolve them to IDs",
};

const SET_META_TARGET_ROUTING: &str = "requires one adrive://instance/space/file URI; --by-name resolves instance/space names to IDs before execution";
const SET_META_UPDATE_ROUTING: &str = "incremental Meta patch: omitted keys remain, empty strings retain keys, file content is unchanged";

pub(crate) const CAPABILITIES: &[CapabilityRow] = &[CapabilityRow {
    command: "ve-adrive file set-meta",
    domain: "file",
    group: "Low-Level",
    layer: "low-level",
    description: "Incrementally update file user metadata",
    risk_level: "medium",
    destructive: false,
    supports_force: false,
    supports_dry_run: true,
    api_actions: &["update_file_meta"],
    parameters: &[PATH, META, BY_NAME],
    examples: &[
        "ve-adrive-cli file set-meta adrive://inst/space/docs/a.txt --meta 'owner=xsh#env=test'",
    ],
}];

pub fn describe(command: &str) -> Option<Value> {
    if command == "ve-adrive file" {
        return Some(json!({
            "command": command,
            "kind": "command_group",
            "layer": "low-level",
            "description": "File core APIs",
            "supports_describe": true,
            "supports_help": true,
            "subcommands": CAPABILITIES
                .iter()
                .map(|row| json!({"name": row.command.rsplit(' ').next(), "description": row.description}))
                .collect::<Vec<_>>(),
        }));
    }
    if !command.starts_with("ve-adrive file ") {
        return None;
    }
    let row = crate::registry::find_capability(command)?;
    Some(json!({
        "command": command,
        "layer": row.layer,
        "api": "update_file_meta",
        "description": row.description,
        "risk_level": row.risk_level,
        "supports_dry_run": row.supports_dry_run,
        "supports_pipe": false,
        "parameters": row.parameters.iter().map(|parameter| json!({
            "name": parameter.name,
            "location": if parameter.name == "path" { "path" } else { "flag" },
            "required": parameter.required,
            "description": parameter.description,
            "schema": {"type": parameter_schema_type(parameter.name)},
        })).collect::<Vec<_>>(),
        "scenario_routing": {
            "target_resolution": SET_META_TARGET_ROUTING,
            "metadata_update": SET_META_UPDATE_ROUTING,
            "dry_run": "returns a deterministic plan without mutating local files or ADrive resources",
            "output": "success and failure paths use Envelope plus --query and multi-format rendering",
        },
        "related_commands": {"low_level": row.api_actions},
        "low_level_apis": row.api_actions,
        "wraps_apis": row.api_actions,
        "examples": row.examples,
        "output_filter_examples": [
            format!("{} --output json | jq '.data'", command),
            format!("{} --output json --query 'data'", command),
        ],
        "shell_quoting_tips": [
            "Quote paths and JSON/JMESPath expressions that contain shell metacharacters.",
            "The command returns an Envelope; extract payload fields from data.*."
        ],
    }))
}

fn parameter_schema_type(name: &str) -> &'static str {
    if name == "by-name" {
        "boolean"
    } else {
        "string"
    }
}

/// 校验 set-meta 目标必须是已存在的文件 URI，目录目标在离线阶段拒绝。
pub(crate) fn validate_set_meta_target(path: &str) -> Result<(), CliError> {
    let target = crate::handler::common::parse_adrive_uri(path, false)?;
    if target.path.is_empty() || target.path.ends_with('/') {
        return Err(CliError::ValidationError(format!(
            "target must be an ADrive file path: {path}"
        )));
    }
    Ok(())
}

/// 在 MCP 规划阶段复用 CLI 的离线校验，避免生成缺参或非法键的“成功计划”。
pub(crate) fn validate_mcp_arguments(
    command: &str,
    arguments: &serde_json::Map<String, Value>,
) -> Result<(), CliError> {
    if command != "ve-adrive file set-meta" {
        return Ok(());
    }
    // --describe 只描述契约，不要求执行参数。
    if arguments.get("describe").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    let path = required_string(command, arguments, "path")?;
    let meta = required_string(command, arguments, "meta")?;
    validate_set_meta_target(path)?;
    crate::domain::metadata::parse_user_metadata(meta).map_err(CliError::ValidationError)?;
    Ok(())
}

fn required_string<'a>(
    command: &str,
    arguments: &'a serde_json::Map<String, Value>,
    name: &str,
) -> Result<&'a str, CliError> {
    match arguments.get(name) {
        Some(Value::String(value)) => Ok(value.as_str()),
        Some(_) => Err(CliError::ValidationError(format!(
            "argument '{name}' for MCP tool '{command}' must be a string"
        ))),
        None => Err(CliError::ValidationError(format!(
            "missing required argument '{name}' for MCP tool '{command}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_metadata_is_a_low_level_command() {
        assert_eq!(describe("ve-adrive file").unwrap()["layer"], "low-level");
        for row in CAPABILITIES {
            assert_eq!(row.layer, "low-level");
            assert_eq!(describe(row.command).unwrap()["layer"], row.layer);
            assert_eq!(crate::registry::business_domain(row.command), "adrive-file");
        }
    }

    #[test]
    fn mcp_planning_reuses_offline_validation() {
        let arguments = |value: Value| value.as_object().unwrap().clone();
        for invalid in [
            json!({"path": "adrive://i/s/a.txt"}),
            json!({"path": "adrive://i/s/a.txt", "meta": "IDS-key=value"}),
            json!({"path": "adrive://i/s/", "meta": "a=b"}),
            json!({"path": "adrive://i/s/a.txt", "meta": "bad.key=value"}),
        ] {
            assert!(
                validate_mcp_arguments("ve-adrive file set-meta", &arguments(invalid.clone()))
                    .is_err(),
                "{invalid}"
            );
        }
        assert!(validate_mcp_arguments(
            "ve-adrive file set-meta",
            &arguments(json!({"path": "adrive://i/s/a.txt", "meta": "a=b"})),
        )
        .is_ok());
        assert!(validate_mcp_arguments(
            "ve-adrive file set-meta",
            &arguments(json!({"describe": true})),
        )
        .is_ok());
    }
}
