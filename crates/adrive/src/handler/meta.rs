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

use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use tos_core::agent::dryrun::{DryRunResult, Impact};
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::agent::output::OutputFormat;
use tos_core::infra::client::storage_user_agent;
use tos_core::infra::config::{
    redact_effective, AdriveOverride, Binary, ConfigFile, EffectiveProfile, FieldSource,
    DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS, DEFAULT_HTTP_MAX_CONNECTIONS,
    DEFAULT_HTTP_MAX_RETRY_COUNT, DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS,
    DEFAULT_TOS_BATCH_REPORT_FORMAT, DEFAULT_TOS_PROGRESS_ENABLED,
};

use crate::cli::meta::{
    ApiArgs, CapabilitiesArgs, CompletionArgs, ConfigAction, ConfigCommand, DoctorArgs,
    DocumentationLanguage, ServeArgs, SkillAction, SkillCommand,
};
use crate::domain::client::resolve_endpoint_and_region;
use crate::handler::common::{
    build_profile, output_envelope, output_result, output_result_with_columns,
    public_adrive_command_path,
};
use crate::registry::{
    business_domain, business_domains, capabilities, command_domains, find_capability,
    CapabilityRow,
};
#[derive(Debug, Serialize)]
struct SkillList {
    language: &'static str,
    skills: Vec<SkillDefinition>,
}

#[derive(Debug, Clone, Serialize)]
struct SkillDefinition {
    schema_version: &'static str,
    name: String,
    domain: String,
    command: String,
    #[serde(skip)]
    internal_command: String,
    description: String,
    risk_level: String,
    input_schema: Value,
    examples: Vec<String>,
    usage: SkillUsage,
}

#[derive(Debug, Clone, Serialize)]
struct SkillUsage {
    format: &'static str,
    source: &'static str,
    mcp_tool_name: String,
    mcp_server: String,
    serve_reads_exported_files: bool,
    exported_file_use: &'static str,
    default_mcp_call: &'static str,
}

#[derive(Debug, Serialize)]
struct McpCommandExecution {
    command: String,
    argv: Vec<String>,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

const ADRIVE_EXAMPLE_PREFIX_ENV: &str = "VE_STORAGE_UNI_ADRIVE_EXAMPLE_PREFIX";
const ADRIVE_DEFAULT_CHECKPOINT_DIR: &str = "~/.tos/checkpoints/ve-adrive";
const ADRIVE_DEFAULT_BATCH_REPORT_DIR: &str = "~/.tos/reports/ve-adrive";

const ADRIVE_HIGH_LEVEL_SEMANTICS: &[(&str, &[&str])] = &[
    (
        "cp",
        &[
            "local path -> adrive://instance/space/path uploads with put_file or multipart upload",
            "adrive://instance/space/path -> local path downloads with get_file and atomic local persist",
            "adrive://instance/space/path -> adrive://instance/space/path copies with copy_file where service-side copy is available",
            "recursive transfers enumerate folders first and honor include/exclude filters",
        ],
    ),
    (
        "mv",
        &[
            "same instance and same space uses rename_file or rename_folder",
            "cross-space or local/remote moves run copy first, then delete the source after destination success",
            "critical source delete requires --force plus exact --confirm <source> in non-interactive shells",
        ],
    ),
    (
        "sync",
        &[
            "builds a source/destination diff from list_files, size, and mtime where available",
            "--delete removes extraneous destination files/folders and upgrades the command to critical risk",
            "transfer phases reuse cp overwrite, checkpoint, and report behavior",
        ],
    ),
    (
        "crt",
        &[
            "adrive://instance -> create_instance",
            "adrive://instance/space -> create_space",
        ],
    ),
    (
        "del",
        &[
            "adrive://instance -> delete_instance",
            "adrive://instance/space -> delete_space",
            "critical deletes require --force plus exact --confirm <target> in non-interactive shells",
        ],
    ),
    (
        "rm",
        &[
            "adrive://instance/space/path -> delete_file or delete_folder",
            "--recursive plans a folder traversal before deletion unless direct recursive mode is selected",
            "critical deletes require --force plus exact --confirm <target> in non-interactive shells",
        ],
    ),
    (
        "ls",
        &[
            "no target -> list_instances",
            "--instance or adrive://instance -> list_spaces",
            "--instance + --space or adrive://instance/space[/folder] -> list_files",
        ],
    ),
    (
        "stat",
        &["adrive://instance/space/path -> head_file metadata for a file or folder"],
    ),
    (
        "du",
        &["adrive://instance/space/folder -> read-only list_files traversal with size, histogram, and optional cost summaries"],
    ),
    (
        "find",
        &["adrive://instance/space/folder -> read-only list_files traversal filtered by name, size, and mtime"],
    ),
    ("cat", &["adrive://instance/space/file -> get_file body streamed to stdout"]),
    ("put", &["stdin -> adrive://instance/space/file upload; multipart is used above the configured threshold"]),
    (
        "mkdir",
        &["adrive://instance/space/folder -> create_folder; --parents creates missing parent folders as needed"],
    ),
];

const CAPABILITY_GROUP_TABLE_COLUMNS: &[&str] = &[
    "name",
    "group",
    "command",
    "layer",
    "description",
    "implemented",
    "command_count",
];

const CAPABILITY_ROW_TABLE_COLUMNS: &[&str] = &[
    "command",
    "group",
    "layer",
    "description",
    "risk_level",
    "destructive",
    "supports_dry_run",
    "supports_force",
];

/// Handle ADrive capabilities.
pub async fn handle_capabilities_command(
    global: &GlobalArgs,
    args: &CapabilitiesArgs,
) -> Result<i32, CliError> {
    let rows = filtered_capabilities(args);
    let view = if args.view == "tree" {
        "full"
    } else {
        args.view.as_str()
    };
    let groups = capability_group_rows(&rows);
    let search_scores = capability_search_scores(args, &rows);
    let commands = capability_command_rows(&rows);
    let payload = match args.view.as_str() {
        "groups" => json!({
            "tool": "ve-adrive",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "ids",
            "view": view,
            "groups": groups,
            "capabilities": [],
            "commands": [],
            "search_scores": search_scores,
            "uri_format": "adrive://instance/space/folder/file",
            "high_level_semantics": high_level_semantics(),
        }),
        "text" => json!({
            "tool": "ve-adrive",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "ids",
            "view": view,
            "groups": groups,
            "capabilities": [],
            "commands": [],
            "lines": rows.iter().map(|row| {
                format!("{} [{}] - {}", row.command, row.risk_level, row.description)
            }).collect::<Vec<_>>(),
            "search_scores": search_scores,
        }),
        "compact" => json!({
            "tool": "ve-adrive",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "ids",
            "view": view,
            "groups": groups,
            "capabilities": rows.iter().map(compact_capability).collect::<Vec<_>>(),
            "commands": commands,
            "search_scores": search_scores,
        }),
        "full" | "tree" => json!({
            "tool": "ve-adrive",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "ids",
            "view": view,
            "groups": groups,
            "capabilities": rows.iter().map(public_capability_row).collect::<Vec<_>>(),
            "commands": commands,
            "search_scores": search_scores,
            "uri_format": "adrive://instance/space/folder/file",
            "parameters": {
                "instance": "A-Drive instance identifier",
                "space": "Space within the instance",
                "folder": "Folder path within the space",
                "file": "File name within the folder"
            },
            "high_level_semantics": high_level_semantics(),
        }),
        other => {
            return Err(CliError::ValidationError(format!(
                "unsupported capabilities view '{}': expected groups, text, compact, full, or tree",
                other
            )));
        }
    };
    output_result_with_columns(
        global,
        &Envelope::success("ve-adrive capabilities", payload),
        capabilities_table_columns(global, args.view.as_str()),
    )?;
    Ok(0)
}

fn filtered_capabilities(args: &CapabilitiesArgs) -> Vec<CapabilityRow> {
    let facet_filtered: Vec<&CapabilityRow> = capabilities()
        .iter()
        .filter(|row| {
            args.group
                .as_deref()
                .map(|group| capability_matches_group(row, group))
                .unwrap_or(true)
        })
        .filter(|row| {
            args.layer
                .as_deref()
                .map(|layer| normalize_facet(row.layer) == normalize_facet(layer))
                .unwrap_or(true)
        })
        .collect();

    let Some(term) = args.search.as_deref() else {
        return facet_filtered.into_iter().cloned().collect();
    };

    let mut scored: Vec<(CapabilityRow, f64)> = facet_filtered
        .into_iter()
        .filter_map(|row| {
            let candidates: Vec<&str> = std::iter::once(row.command)
                .chain(std::iter::once(row.description))
                .chain(row.api_actions.iter().copied())
                .collect();
            let score = rank_best(term, &candidates);
            if score >= 0.85 {
                Some((row.clone(), score))
            } else {
                None
            }
        })
        .collect();

    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(row, _)| row).collect()
}

/// Compute the best fuzzy match score for `term` against a set of candidate
/// strings. Uses Jaro-Winkler similarity with boosted scores for exact,
/// substring, and prefix matches — aligned with the TOS capabilities search.
fn rank_best(term: &str, candidates: &[&str]) -> f64 {
    let t_lower = term.to_lowercase();
    candidates
        .iter()
        .map(|c| {
            let c_lower = c.to_lowercase();
            if c_lower == t_lower {
                return 1.0;
            }
            if c_lower.contains(&t_lower) {
                return 0.92;
            }
            if c_lower.starts_with(&t_lower) {
                return 0.9;
            }
            strsim::jaro_winkler(&t_lower, &c_lower) as f64
        })
        .fold(0.0_f64, f64::max)
}

fn capability_group_rows(rows: &[CapabilityRow]) -> Vec<Value> {
    let mut groups = Vec::new();
    for (name, layer, command, description) in [
        (
            "High-Level",
            "high-level",
            "ve-adrive",
            "File management operations with adrive:// URI support",
        ),
        (
            "Capabilities / Utilities",
            "utility",
            "ve-adrive capabilities",
            "CLI configuration and introspection utilities",
        ),
    ] {
        let commands = rows
            .iter()
            .filter(|row| row.layer == layer || row.group == name)
            .map(|row| row.command)
            .collect::<Vec<_>>();
        if commands.is_empty() {
            continue;
        }
        let category = if layer == "utility" {
            "utilities".to_string()
        } else {
            layer.replace('-', "_")
        };
        groups.push(json!({
            "name": name,
            "group": category,
            "command": command,
            "layer": layer.replace('-', "_"),
            "category": category,
            "description": description,
            "implemented": true,
            "command_count": commands.len(),
            "commands": commands,
        }));
    }
    groups
}

fn capability_matches_group(row: &CapabilityRow, group: &str) -> bool {
    let requested = normalize_facet(group);
    let row_group = normalize_facet(row.group);
    let row_layer = normalize_facet(row.layer);
    let row_domain = normalize_facet(row.domain);
    let category = if row.layer == "utility" {
        "utilities".to_string()
    } else {
        row_layer.clone()
    };
    requested == row_group
        || requested == row_layer
        || requested == row_domain
        || requested == category
        || (requested == "capabilities_utilities" && row.layer == "utility")
}

fn normalize_facet(value: &str) -> String {
    let mut normalized = String::new();
    let mut last_was_sep = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_sep = false;
        } else if !last_was_sep {
            normalized.push('_');
            last_was_sep = true;
        }
    }
    normalized.trim_matches('_').to_string()
}

fn capability_command_rows(rows: &[CapabilityRow]) -> Vec<Value> {
    rows.iter()
        .map(|row| {
            json!({
                "name": row.domain,
                "command": row.command,
                "layer": row.layer.replace('-', "_"),
                "category": if row.layer == "utility" { "utilities" } else { row.layer },
                "description": row.description,
                "supports_help": true,
                "supports_describe": true,
                "implemented": true,
                "parameters": row.parameters,
                "subcommands": [],
            })
        })
        .collect()
}

fn capability_search_scores(args: &CapabilitiesArgs, rows: &[CapabilityRow]) -> Vec<Value> {
    let Some(term) = args.search.as_deref() else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let candidates: Vec<&str> = std::iter::once(row.command)
                .chain(std::iter::once(row.description))
                .chain(row.api_actions.iter().copied())
                .collect();
            let score = rank_best(term, &candidates);
            (score >= 0.85).then(|| {
                json!({
                    "kind": "capability",
                    "command": row.command,
                    "score": score,
                    "matched_field": "command_or_api",
                })
            })
        })
        .collect()
}

fn compact_capability(row: &CapabilityRow) -> Value {
    json!({
        "command": row.command,
        "domain": row.domain,
        "group": row.group,
        "layer": row.layer,
        "description": row.description,
        "risk_level": row.risk_level,
        "destructive": row.destructive,
        "supports_force": row.supports_force,
        "supports_dry_run": row.supports_dry_run,
        "api_actions": row.api_actions,
    })
}

fn high_level_semantics() -> Value {
    let mut semantics = serde_json::Map::new();
    for (command, lines) in ADRIVE_HIGH_LEVEL_SEMANTICS {
        semantics.insert((*command).to_string(), json!(*lines));
    }
    Value::Object(semantics)
}

fn capabilities_table_columns(global: &GlobalArgs, view: &str) -> Option<&'static [&'static str]> {
    if global.query.is_some() {
        // [Review Fix #4] Explicit JMESPath selection owns the table shape.
        return None;
    }
    match view {
        "groups" => Some(CAPABILITY_GROUP_TABLE_COLUMNS),
        "compact" | "full" | "tree" => Some(CAPABILITY_ROW_TABLE_COLUMNS),
        _ => None,
    }
}

/// Handle ADrive API metadata.
pub async fn handle_api_command(global: &GlobalArgs, args: &ApiArgs) -> Result<i32, CliError> {
    let command = format!("ve-adrive api {} {}", args.group, args.action);
    if args.describe {
        let capability = find_capability("ve-adrive api")
            .map(compact_capability)
            .unwrap_or_else(
                || json!({"command": "ve-adrive api", "mode": "guarded_utility_passthrough"}),
            );
        let desc = json!({
            "command": command,
            "description": format!(
                "Guarded ADrive utility API planning for {}.{}; direct raw execution is not implemented yet",
                args.group, args.action
            ),
            "service": "ids",
            "capability": capability,
            "mode": "guarded_utility_passthrough",
            "layer": "meta",
            "raw_api_execution_implemented": false,
            "supports_dry_run": true,
            "supports_force": false,
        });
        output_result(global, &Envelope::success(command, desc))?;
        return Ok(0);
    }

    let request = parse_optional_request(args.request.as_deref())?;
    if !global.dry_run {
        return Err(CliError::ValidationError(
            "ADrive raw API execution is not implemented yet; use --dry-run to inspect the planned request or --describe for metadata".to_string(),
        ));
    }
    let payload = json!({
        "group": &args.group,
        "action": &args.action,
        "request": request,
        "status": "planned_not_executed",
        "mode": "guarded_utility_passthrough",
        "raw_api_execution_implemented": false,
        "message": "ADrive raw API execution is not implemented; this utility returns dry-run metadata only",
    });
    let envelope = Envelope::success(
        format!("ve-adrive api {} {}", args.group, args.action),
        payload,
    );
    output_envelope(global, &envelope)?;
    Ok(0)
}

fn parse_optional_request(request: Option<&str>) -> Result<Value, CliError> {
    let Some(request) = request else {
        return Ok(Value::Null);
    };
    let candidate = request.strip_prefix("file://").unwrap_or(request);
    let payload = if Path::new(candidate).exists() {
        fs::read_to_string(candidate)?
    } else {
        request.to_string()
    };
    serde_json::from_str(&payload)
        .map_err(|err| CliError::ValidationError(format!("invalid --request JSON: {err}")))
}

pub async fn handle_skill_command(
    global: &GlobalArgs,
    cmd: &SkillCommand,
) -> Result<i32, CliError> {
    if global.describe {
        // [Review Fix #ADrive-SkillDescribe] Describe must stay read-only even
        // for `skill export`, otherwise Agents can accidentally create files
        // while only asking for metadata.
        let command_path = match &cmd.action {
            SkillAction::List { .. } => "ve-adrive skill list",
            SkillAction::Export { .. } => "ve-adrive skill export",
        };
        let description = describe_adrive_command_metadata(command_path).ok_or_else(|| {
            CliError::ValidationError(format!("no metadata registered for {command_path}"))
        })?;
        output_result(global, &Envelope::success(command_path, description))?;
        return Ok(0);
    }
    match &cmd.action {
        SkillAction::List { language } => {
            output_result(
                global,
                &Envelope::success(
                    "ve-adrive skill list",
                    SkillList {
                        language: language.code(),
                        skills: skill_definitions_for_language(*language),
                    },
                ),
            )?;
            Ok(0)
        }
        SkillAction::Export {
            name,
            dir,
            language,
        } => {
            let export_plan = skill_markdown_export_plan(name.as_deref(), dir)?;
            if global.dry_run {
                output_result(
                    global,
                    &Envelope::success(
                        "ve-adrive skill export",
                        plan_skill_markdown_export(&export_plan, dir, *language),
                    ),
                )?;
                return Ok(0);
            }
            let exported = export_markdown_skills(export_plan, dir, *language)?;
            output_result(
                global,
                &Envelope::success("ve-adrive skill export", exported),
            )?;
            Ok(0)
        }
    }
}

fn skill_definitions() -> Vec<SkillDefinition> {
    capabilities()
        .iter()
        .map(|row| {
            let internal_command = row.command.to_string();
            let public_command = public_adrive_command_path(row.command);
            let name = public_command.replace(' ', "_").replace('-', "_");
            SkillDefinition {
                schema_version: "adrive-skill-v1",
                name: name.clone(),
                domain: business_domain(row.command).to_string(),
                command: public_command,
                internal_command,
                description: row.description.to_string(),
                risk_level: row.risk_level.to_string(),
                input_schema: skill_input_schema(row),
                examples: row
                    .examples
                    .iter()
                    .map(|example| public_adrive_example(example))
                    .collect(),
                usage: skill_usage(name),
            }
        })
        .collect()
}

fn skill_definitions_for_language(language: DocumentationLanguage) -> Vec<SkillDefinition> {
    let mut definitions = skill_definitions();
    if matches!(language, DocumentationLanguage::Zh) {
        for definition in &mut definitions {
            definition.description = localized_skill_description_zh(definition);
            definition.input_schema = localized_input_schema(&definition.input_schema, language);
        }
    }
    definitions
}

fn localized_skill_description_zh(skill: &SkillDefinition) -> String {
    format!(
        "用于调用 `{}`。原始英文说明：{}",
        public_adrive_command(&skill.command),
        skill.description
    )
}

fn localized_input_schema(schema: &Value, language: DocumentationLanguage) -> Value {
    match language {
        DocumentationLanguage::En => schema.clone(),
        DocumentationLanguage::Zh => localize_schema_descriptions_zh(schema),
    }
}

fn localize_schema_descriptions_zh(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut localized = serde_json::Map::new();
            for (key, child) in map {
                if key == "description" {
                    if let Some(description) = child.as_str() {
                        // [Review Fix #ZhDocs1] 中文 skill 文档不能只翻译章节标题；
                        // schema 参数说明也包装成中文，保留原文避免误译命令契约。
                        localized.insert(
                            key.clone(),
                            Value::String(format!("参数说明：{description}")),
                        );
                        continue;
                    }
                }
                localized.insert(key.clone(), localize_schema_descriptions_zh(child));
            }
            Value::Object(localized)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(localize_schema_descriptions_zh)
                .collect::<Vec<_>>(),
        ),
        _ => value.clone(),
    }
}

fn public_capability_row(row: &CapabilityRow) -> Value {
    let mut value = serde_json::to_value(row).unwrap_or_else(|_| json!({}));
    value["examples"] = json!(row
        .examples
        .iter()
        .map(|example| public_adrive_example(example))
        .collect::<Vec<_>>());
    value
}

fn public_adrive_command(command: &str) -> String {
    let prefix = adrive_example_prefix();
    command
        .strip_prefix("ve-adrive ")
        .or_else(|| command.strip_prefix("ve-adrive-cli "))
        .or_else(|| command.strip_prefix("ve-storage-uni-cli ve-adrive "))
        .map(|suffix| format!("{prefix} {suffix}"))
        .unwrap_or_else(|| command.to_string())
}

fn public_adrive_example(example: &str) -> String {
    let prefix = adrive_example_prefix();
    let with_public_pipeline = example
        .replace(" | ve-adrive ", &format!(" | {prefix} "))
        .replace("ve-adrive-cli ", &format!("{prefix} "))
        .replace("ve-storage-uni-cli ve-adrive ", &format!("{prefix} "));
    public_adrive_command(&with_public_pipeline)
}

fn adrive_example_prefix() -> String {
    std::env::var(ADRIVE_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "ve-adrive-cli".to_string())
}

fn skill_usage(name: String) -> SkillUsage {
    SkillUsage {
        format: "Markdown SKILL.md",
        source: "Derived from the live ADrive CLI capability registry.",
        mcp_tool_name: name,
        mcp_server: public_adrive_command("ve-adrive serve --mcp"),
        serve_reads_exported_files: false,
        exported_file_use: "Portable Markdown skill pack for external agents, documentation generators, prompts, or adapters. The built-in MCP server rebuilds tools from the in-process registry instead of reading exported files.",
        default_mcp_call: "tools/call returns a plan by default; include argument execute=true to run the underlying CLI command.",
    }
}

fn skill_input_schema(row: &CapabilityRow) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in row.parameters {
        properties.insert(
            parameter.name.to_string(),
            json!({
                "type": parameter_schema_type(parameter.name),
                "description": parameter.description,
            }),
        );
        if parameter.required {
            required.push(parameter.name);
        }
    }
    for (name, schema) in mcp_common_schema_properties() {
        properties.entry(name.to_string()).or_insert(schema);
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn parameter_schema_type(name: &str) -> &'static str {
    if is_boolean_parameter(name) {
        "boolean"
    } else if matches!(
        name,
        "port"
            | "max-keys"
            | "max-depth"
            | "top-k"
            | "batch-concurrency"
            | "list-concurrency"
            | "multipart-concurrency"
    ) {
        "integer"
    } else {
        "string"
    }
}

fn mcp_common_schema_properties() -> [(&'static str, Value); 9] {
    [
        (
            "execute",
            json!({"type": "boolean", "description": "Execute the CLI command; false returns a plan only"}),
        ),
        (
            "dry_run",
            json!({"type": "boolean", "description": "Pass global --dry-run to the CLI command"}),
        ),
        (
            "describe",
            json!({"type": "boolean", "description": "Pass global --describe to the CLI command"}),
        ),
        (
            "profile",
            json!({"type": "string", "description": "Configuration profile name"}),
        ),
        (
            "output",
            json!({"type": "string", "description": "Output format, defaults to json"}),
        ),
        (
            "region",
            json!({"type": "string", "description": "Optional global region override"}),
        ),
        (
            "endpoint",
            json!({"type": "string", "description": "Optional global endpoint override"}),
        ),
        (
            "verbose",
            json!({"type": "boolean", "description": "Include extra diagnostic output where supported"}),
        ),
        (
            "quiet",
            json!({"type": "boolean", "description": "Disable prompts and progress output"}),
        ),
    ]
}

fn is_boolean_parameter(name: &str) -> bool {
    matches!(
        name,
        "force"
            | "by-name"
            | "recursive"
            | "include-parent"
            | "include-uploads"
            | "index-enabled"
            | "parents"
            | "no-clobber"
            | "no-manifest"
            | "report-failures-only"
            | "progress"
            | "no-progress"
            | "list-echo"
            | "no-list-echo"
            | "delete"
            | "size-only"
            | "exact-timestamps"
            | "human-readable"
            | "cost"
            | "mcp"
            | "dry_run"
            | "describe"
            | "execute"
            | "verbose"
            | "quiet"
    )
}

fn selected_skills(name: Option<&str>) -> Result<Vec<SkillDefinition>, CliError> {
    let skills = skill_definitions();
    let selected = skills
        .into_iter()
        .filter(|skill| {
            name.map(|name| {
                skill.name == name
                    || skill.domain == name
                    || skill.command == name
                    || skill.command == format!("ve-adrive {name}")
                    || skill.internal_command == name
                    || skill.internal_command == format!("ve-adrive {name}")
                    || legacy_skill_name(&skill.internal_command) == name
            })
            .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(CliError::ValidationError(format!(
            "no ve-adrive skill matches '{}'",
            name.unwrap_or_default()
        )));
    }
    Ok(selected)
}

fn legacy_skill_name(command: &str) -> String {
    command.replace(' ', "_").replace('-', "_")
}

fn skill_markdown_export_plan(
    name: Option<&str>,
    dir: &str,
) -> Result<Vec<(SkillDefinition, PathBuf)>, CliError> {
    Ok(selected_skills(name)?
        .into_iter()
        .map(|skill| {
            let path = Path::new(dir)
                .join(&skill.domain)
                .join(&skill.name)
                .join("SKILL.md");
            (skill, path)
        })
        .collect())
}

fn plan_skill_markdown_export(
    export_plan: &[(SkillDefinition, PathBuf)],
    dir: &str,
    language: DocumentationLanguage,
) -> Value {
    // [Review Fix #SkillExportAlign] Expose the same path fields as tos-cli
    // and ve-tos so dry-run consumers do not need per-command branching.
    let entries = export_plan
        .iter()
        .map(|(skill, path)| {
            json!({
                "skill": skill.name,
                "domain": skill.domain,
                "command": skill.command,
                "path": path.display().to_string(),
                "conflict": path.exists(),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "dry_run": true,
        "format": "markdown_skill",
        "language": language.code(),
        "dir": dir,
        "selected": export_plan.len(),
        "root_file": skill_root_path(Path::new(dir)).display().to_string(),
        "paths": export_paths(export_plan, Path::new(dir))
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "skill_paths": export_plan
            .iter()
            .map(|(_, path)| path.display().to_string())
            .collect::<Vec<_>>(),
        "status": "planned_not_written",
        "skill_count": export_plan.len(),
        "entries": entries,
    })
}

fn export_markdown_skills(
    export_plan: Vec<(SkillDefinition, PathBuf)>,
    dir: &str,
    language: DocumentationLanguage,
) -> Result<Value, CliError> {
    for path in export_paths(&export_plan, Path::new(dir)) {
        if path.exists() {
            return Err(CliError::Conflict(format!(
                "skill export target '{}' already exists",
                path.display()
            )));
        }
    }

    let mut files = Vec::new();
    let skills = export_plan
        .iter()
        .map(|(skill, _)| skill.clone())
        .collect::<Vec<_>>();
    let root_path = skill_root_path(Path::new(dir));
    if let Some(parent) = root_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &root_path,
        skill_index_markdown("ve-adrive", &skills, language),
    )?;
    files.push(root_path.display().to_string());
    for (skill, path) in export_plan {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, skill_markdown(&skill, language))?;
        files.push(path.display().to_string());
    }

    Ok(json!({
        "dry_run": false,
        "format": "markdown_skill",
        "language": language.code(),
        "dir": dir,
        "selected": files.len().saturating_sub(1),
        "root_file": files.first().cloned(),
        "files": files,
    }))
}

fn skill_root_path(dir: &Path) -> PathBuf {
    dir.join("SKILL.md")
}

fn export_paths(export_plan: &[(SkillDefinition, PathBuf)], dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![skill_root_path(dir)];
    paths.extend(export_plan.iter().map(|(_, path)| path.clone()));
    paths
}

fn skill_index_markdown(
    surface: &str,
    skills: &[SkillDefinition],
    language: DocumentationLanguage,
) -> String {
    let mut domains = std::collections::BTreeMap::<&str, Vec<&SkillDefinition>>::new();
    for skill in skills {
        domains.entry(&skill.domain).or_default().push(skill);
    }
    let mut body = match language {
        DocumentationLanguage::En => format!(
            "# {surface} skills\n\nUse this skill pack when the user wants to operate `{surface}` commands. Select a domain below, then use the nested command skill.\n\n"
        ),
        DocumentationLanguage::Zh => format!(
            "# {surface} Skills\n\n当用户需要操作 `{surface}` 命令时使用此 Skill 包。先按领域选择，再进入对应的命令 Skill。\n\n"
        ),
    };
    for (domain, skills) in domains {
        body.push_str(&format!("## {domain}\n\n"));
        for skill in skills {
            let description = match language {
                DocumentationLanguage::En => skill.description.clone(),
                DocumentationLanguage::Zh => localized_skill_description_zh(skill),
            };
            body.push_str(&format!(
                "- [{}](./{}/{}/SKILL.md): `{}` - {}\n",
                skill.name, skill.domain, skill.name, skill.command, description
            ));
        }
        body.push('\n');
    }
    body
}

fn skill_markdown(skill: &SkillDefinition, language: DocumentationLanguage) -> String {
    let examples = if skill.examples.is_empty() {
        match language {
            DocumentationLanguage::En => {
                "- Run with `--describe` first to inspect the command contract.".to_string()
            }
            DocumentationLanguage::Zh => {
                "- 先运行 `--describe` 检查命令契约，再决定是否执行。".to_string()
            }
        }
    } else {
        skill
            .examples
            .iter()
            .map(|example| format!("- `{example}`"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let input_schema = localized_input_schema(&skill.input_schema, language);
    let schema = serde_json::to_string_pretty(&input_schema).unwrap_or_else(|_| "{}".to_string());
    match language {
        DocumentationLanguage::En => format!(
            r#"# {name}

Use this skill when the user wants to run `{command}` with the Volcano Engine ADrive CLI.

## Description

{description}

## Command

`{command}`

Risk level: `{risk_level}`

## Inputs

```json
{schema}
```

## Examples

{examples}

## Execution

Prefer `{public_command} --describe` or `{public_command} --dry-run --output json` before executing a command that writes or deletes data. Destructive commands must include the required `--force` and exact `--confirm` target.
"#,
            name = skill.name,
            command = skill.command,
            description = skill.description,
            risk_level = skill.risk_level,
            schema = schema,
            examples = examples,
            public_command = public_adrive_command(&skill.command),
        ),
        DocumentationLanguage::Zh => format!(
            r#"# {name}

当用户需要通过火山引擎 ADrive CLI 运行 `{command}` 时使用此 Skill。

## 说明

{description}

## 命令

`{command}`

风险等级：`{risk_level}`

## 输入

```json
{schema}
```

## 示例

{examples}

## 执行建议

执行会写入或删除数据的命令前，优先运行 `{public_command} --describe` 或 `{public_command} --dry-run --output json`。破坏性命令必须包含必需的 `--force` 和精确匹配目标的 `--confirm`。
"#,
            name = skill.name,
            command = skill.command,
            description = localized_skill_description_zh(skill),
            risk_level = skill.risk_level,
            schema = schema,
            examples = examples,
            public_command = public_adrive_command(&skill.command),
        ),
    }
}

/// Handle ADrive config.
pub async fn handle_config_command(
    global: &GlobalArgs,
    cmd: &ConfigCommand,
) -> Result<i32, CliError> {
    if global.describe {
        // [Review Fix #ADrive-ConfigDescribe] Capabilities advertise describe
        // support for this utility group, so group-level describe must not
        // require a leaf subcommand.
        output_result(
            global,
            &Envelope::success("ve-adrive config", describe_config_group()),
        )?;
        return Ok(0);
    }

    let Some(action) = &cmd.action else {
        return Err(CliError::ValidationError(
            "`ve-adrive config` requires a subcommand; use `ve-adrive config --help`".to_string(),
        ));
    };

    if global.dry_run {
        // [Review Fix #ADrive-ConfigDryRun] Global --dry-run must stay side-effect
        // free for config init/set, matching the TOS config handler contract.
        return handle_config_dry_run(global, action);
    }

    match action {
        ConfigAction::Init { profile } => {
            let profile_name = effective_config_init_profile(global, profile.as_deref())?;
            let config_path = global.config_path();
            // [Review Fix #2] Preserve unreadable or malformed existing config
            // files instead of replacing them with a fresh default profile.
            let mut config = ConfigFile::load_from(&config_path)?;
            let adrive_override = config
                .get_or_insert_profile(profile_name)
                .adrive
                .get_or_insert_with(AdriveOverride::default);
            if adrive_override.checkpoint_dir.is_none() {
                adrive_override.checkpoint_dir = Some(ADRIVE_DEFAULT_CHECKPOINT_DIR.to_string());
            }
            if adrive_override.batch_report_dir.is_none() {
                adrive_override.batch_report_dir =
                    Some(ADRIVE_DEFAULT_BATCH_REPORT_DIR.to_string());
            }
            if adrive_override.batch_report_format.is_none() {
                adrive_override.batch_report_format =
                    Some(DEFAULT_TOS_BATCH_REPORT_FORMAT.to_string());
            }
            if adrive_override.progress_enabled.is_none() {
                adrive_override.progress_enabled = Some(DEFAULT_TOS_PROGRESS_ENABLED);
            }
            if adrive_override.max_retry_count.is_none() {
                adrive_override.max_retry_count = Some(DEFAULT_HTTP_MAX_RETRY_COUNT);
            }
            if adrive_override.requesttimeout.is_none() {
                adrive_override.requesttimeout = Some(DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS);
            }
            if adrive_override.connecttimeout.is_none() {
                adrive_override.connecttimeout = Some(DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS);
            }
            if adrive_override.maxconnections.is_none() {
                adrive_override.maxconnections = Some(DEFAULT_HTTP_MAX_CONNECTIONS);
            }
            config.save_to_path(&config_path)?;
            let envelope = Envelope::success(
                "ve-adrive config init",
                json!({
                    "profile": profile_name,
                    "status": "initialized",
                    "config_path": config_path.display().to_string(),
                    "message": format!("Profile '{}' initialized with A-Drive defaults", profile_name),
                }),
            );
            output_envelope(global, &envelope)?;
            Ok(0)
        }
        ConfigAction::Show => {
            let config_path = global.config_path();
            let config_dir = ConfigFile::config_dir_from_path(&config_path);
            let config = ConfigFile::load_from(&config_path)?;

            // 与 tos config show 对齐：列出所有 profiles
            let binary = Binary::Adrive;
            let mut effective: Vec<EffectiveProfile> = Vec::new();
            for profile_name in config.profiles.keys() {
                let eff = config.get_effective_profile_in_dir(profile_name, binary, &config_dir)?;
                effective.push(redact_adrive_effective(eff));
            }

            let format = global.output.unwrap_or_else(OutputFormat::auto_detect);
            match format {
                OutputFormat::Table => {
                    println!("Config file: {}\n", config_path.display());
                    let headers = &["PROFILE", "FIELD", "VALUE", "SOURCE"];
                    let mut rows: Vec<Vec<String>> = Vec::new();
                    for eff in &effective {
                        push_traced_row(&mut rows, eff, "region", &eff.region);
                        push_traced_row(&mut rows, eff, "endpoint", &eff.endpoint);
                        push_traced_row(&mut rows, eff, "checkpoint_dir", &eff.checkpoint_dir);
                        push_traced_row(&mut rows, eff, "batch_report_dir", &eff.batch_report_dir);
                        push_traced_row(
                            &mut rows,
                            eff,
                            "batch_report_format",
                            &eff.batch_report_format,
                        );
                        push_traced_bool_row(
                            &mut rows,
                            eff,
                            "progress_enabled",
                            &eff.progress_enabled,
                        );
                        push_traced_value_row(
                            &mut rows,
                            eff,
                            "max_retry_count",
                            &eff.max_retry_count,
                        );
                        push_traced_value_row(
                            &mut rows,
                            eff,
                            "requesttimeout",
                            &eff.requesttimeout,
                        );
                        push_traced_value_row(
                            &mut rows,
                            eff,
                            "connecttimeout",
                            &eff.connecttimeout,
                        );
                        push_traced_value_row(
                            &mut rows,
                            eff,
                            "maxconnections",
                            &eff.maxconnections,
                        );
                        push_traced_row(&mut rows, eff, "access_key_id", &eff.access_key_id);
                        push_traced_row(
                            &mut rows,
                            eff,
                            "secret_access_key",
                            &eff.secret_access_key,
                        );
                        push_traced_row(&mut rows, eff, "security_token", &eff.security_token);
                        if let Some(ref f) = eff.account_id {
                            push_traced_row(&mut rows, eff, "account_id", f);
                        }
                        if let Some(ref f) = eff.default_instance {
                            push_traced_row(&mut rows, eff, "default_instance", f);
                        }
                        if let Some(ref f) = eff.default_space {
                            push_traced_row(&mut rows, eff, "default_space", f);
                        }
                    }
                    use tos_core::agent::output::format_table;
                    println!("{}", format_table(headers, &rows));
                }
                _ => {
                    let envelope = Envelope::success(
                        "ve-adrive config show",
                        json!({
                            "config_path": config_path.display().to_string(),
                            "profiles": effective,
                        }),
                    );
                    output_envelope(global, &envelope)?;
                }
            }
            Ok(0)
        }
        ConfigAction::Set { key, value } => {
            let config_path = global.config_path();
            let mut config = ConfigFile::load_from(&config_path)?;
            // [Review Fix #ADrive-ConfigDryRun] Real execution and dry-run use the
            // same key routing so previewed ADrive writes cannot drift from saved writes.
            let segments = adrive_config_key_segments(global, key)?;
            let segment_refs = segments.iter().map(String::as_str).collect::<Vec<_>>();
            config.set_by_path(&segment_refs, value)?;
            config.save_to_path(&config_path)?;
            let envelope = Envelope::success(
                "ve-adrive config set",
                adrive_config_set_output(key, value, &segments, &config_path),
            );
            output_envelope(global, &envelope)?;
            Ok(0)
        }
    }
}

fn adrive_config_set_output(
    key: &str,
    value: &str,
    segments: &[String],
    config_path: &std::path::Path,
) -> Value {
    // [Review Fix #6] Only redact sensitive keys; non-sensitive routing values
    // such as region/endpoint must stay visible for troubleshooting.
    let section = match segments {
        [profile, _field] => format!("[{profile}]"),
        [profile, service, _field] => format!("[{profile}.{service}]"),
        _ => key.to_string(),
    };
    let field = segments.last().cloned().unwrap_or_else(|| key.to_string());
    let encrypted = is_sensitive_adrive_config_key(key);
    json!({
        "key": key,
        "section": section,
        "field": field,
        "value": redact_adrive_config_value(key, value),
        "encrypted": encrypted,
        "status": "saved",
        "config_path": config_path.display().to_string(),
        "message": format!("Saved {} to config", key),
    })
}

fn handle_config_dry_run(global: &GlobalArgs, action: &ConfigAction) -> Result<i32, CliError> {
    let dry_run = match action {
        ConfigAction::Init { profile } => {
            let profile_name = effective_config_init_profile(global, profile.as_deref())?;
            let path = global.config_path();
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
                plan: vec![
                    format!(
                        "CREATE or UPDATE template config file at '{}'",
                        path.display()
                    ),
                    format!("ENSURE [{}.adrive] section exists", profile_name),
                    format!(
                        "WRITE [{}.adrive].checkpoint_dir default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].batch_report_dir default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].batch_report_format default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].progress_enabled default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].max_retry_count default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].requesttimeout default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].connecttimeout default if missing",
                        profile_name
                    ),
                    format!(
                        "WRITE [{}.adrive].maxconnections default if missing",
                        profile_name
                    ),
                ],
                warnings: if path.exists() {
                    vec![format!(
                        "Config file already exists at '{}'; existing profiles are preserved",
                        path.display()
                    )]
                } else {
                    vec![]
                },
                confirm_command: Some(format!(
                    "ve-adrive-cli config init --profile {}",
                    profile_name
                )),
            }
        }
        ConfigAction::Set { key, value } => {
            let segments = adrive_config_key_segments(global, key)?;
            let segment_refs = segments.iter().map(String::as_str).collect::<Vec<_>>();
            let mut validation_config = ConfigFile::default();
            validation_config.set_by_path(&segment_refs, value)?;
            let redacted_value = redact_adrive_config_value(key, value);
            let plan_line = match segments.len() {
                2 => format!(
                    "SET [{}].{} = '{}'",
                    segments[0], segments[1], redacted_value
                ),
                3 => format!(
                    "SET [{}.{}].{} = '{}'",
                    segments[0], segments[1], segments[2], redacted_value
                ),
                _ => format!("SET {} = '{}'", key, redacted_value),
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
                warnings: if is_sensitive_adrive_config_key(key) {
                    vec!["Sensitive value redacted in dry-run output".to_string()]
                } else {
                    vec![]
                },
                confirm_command: Some(format!(
                    "ve-adrive-cli --profile {} config set {} {}",
                    global.profile,
                    key,
                    redact_adrive_config_value(key, value)
                )),
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
            plan: vec!["READ config file and render redacted effective profiles".to_string()],
            warnings: vec![],
            confirm_command: None,
        },
    };
    output_envelope(
        global,
        &Envelope::success(config_action_command(action), dry_run),
    )?;
    Ok(0)
}

fn adrive_config_key_segments(global: &GlobalArgs, key: &str) -> Result<Vec<String>, CliError> {
    if global.profile.is_empty() {
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }
    if key.contains('.') {
        return Ok(key.split('.').map(ToString::to_string).collect());
    }
    Ok(vec![
        global.profile.clone(),
        "adrive".to_string(),
        key.to_string(),
    ])
}

fn is_sensitive_adrive_config_key(key: &str) -> bool {
    let leaf = key.rsplit('.').next().unwrap_or(key);
    matches!(
        leaf,
        "access_key_id" | "secret_access_key" | "security_token"
    )
}

fn redact_adrive_config_value(key: &str, value: &str) -> String {
    if is_sensitive_adrive_config_key(key) {
        "****".to_string()
    } else {
        value.to_string()
    }
}

fn config_action_command(action: &ConfigAction) -> &'static str {
    match action {
        ConfigAction::Init { .. } => "ve-adrive config init",
        ConfigAction::Show => "ve-adrive config show",
        ConfigAction::Set { .. } => "ve-adrive config set",
    }
}

fn effective_config_init_profile<'a>(
    global: &'a GlobalArgs,
    profile: Option<&'a str>,
) -> Result<&'a str, CliError> {
    let profile_name = profile.unwrap_or(global.profile.as_str());
    if profile_name.is_empty() {
        // [Review Fix #21] ADrive config init must honor global --profile and reject empty names.
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }
    Ok(profile_name)
}

fn describe_config_group() -> Value {
    json!({
        "command": "ve-adrive config",
        "description": "Configuration management",
        "kind": "command_group",
        "layer": "meta",
        "subcommands": [
            {
                "name": "init",
                "description": "Initialize configuration",
                "risk_level": "low",
            },
            {
                "name": "show",
                "description": "Show effective configuration",
                "risk_level": "low",
            },
            {
                "name": "set",
                "description": "Set configuration value",
                "risk_level": "low",
            },
        ],
        "supports_describe": true,
        "supports_help": true,
    })
}

/// Build registry-backed describe metadata for ADrive meta commands.
///
/// Returns `None` when `command_path` is not an ADrive meta command handled by
/// this module.
pub fn describe_adrive_command_metadata(command_path: &str) -> Option<Value> {
    let registry_command = match command_path {
        "ve-adrive skill list" | "ve-adrive skill export" => "ve-adrive skill",
        other => other,
    };
    let row = find_capability(registry_command)?;
    let parameters = describe_meta_parameters(command_path, row);
    Some(json!({
        "command": command_path,
        "layer": "meta",
        "description": describe_meta_command_description(command_path, row.description),
        "risk_level": row.risk_level,
        "supports_dry_run": matches!(command_path, "ve-adrive serve" | "ve-adrive skill export"),
        "supports_pipe": false,
        "parameters": parameters,
        "scenario_routing": describe_meta_scenario_routing(command_path),
        "related_commands": {
            "low_level": row.api_actions,
        },
        "low_level_apis": row.api_actions,
        "wraps_apis": row.api_actions,
        "examples": describe_meta_examples(command_path),
        "output_filter_examples": [
            format!("{} --output json | jq '.data'", public_adrive_command(command_path)),
            format!("{} --output json --query 'data'", public_adrive_command(command_path)),
        ],
        "shell_quoting_tips": [
            "Quote paths and JSON/JMESPath expressions that contain shell metacharacters.",
            "The command returns an Envelope; extract payload fields from data.*."
        ],
    }))
}

fn describe_meta_command_description(command_path: &str, fallback: &str) -> String {
    match command_path {
        "ve-adrive completion" => {
            "Generate shell completion scripts and installation snippets for ADrive CLI names."
        }
        "ve-adrive serve" => "Start the registry-backed ADrive MCP server.",
        "ve-adrive skill list" => "List ADrive skill metadata from the live registry.",
        "ve-adrive skill export" => "Export ADrive Markdown SKILL.md files for external consumers.",
        _ => fallback,
    }
    .to_string()
}

fn describe_meta_parameters(command_path: &str, row: &CapabilityRow) -> Vec<Value> {
    match command_path {
        "ve-adrive skill list" => vec![describe_meta_parameter(
            "language",
            false,
            "Documentation language for generated skill metadata: en (default) or zh",
            "flag",
        )],
        "ve-adrive skill export" => vec![
            describe_meta_parameter(
                "name",
                false,
                "Optional skill name, command path, or business domain filter",
                "flag",
            ),
            describe_meta_parameter(
                "dir",
                false,
                "Output directory for exported Markdown skill files",
                "flag",
            ),
            describe_meta_parameter(
                "language",
                false,
                "Documentation language for generated SKILL.md files: en (default) or zh",
                "flag",
            ),
        ],
        _ => row
            .parameters
            .iter()
            .map(|parameter| {
                describe_meta_parameter(
                    parameter.name,
                    parameter.required,
                    parameter.description,
                    if parameter.name == "shell" {
                        "path"
                    } else {
                        "flag"
                    },
                )
            })
            .collect(),
    }
}

fn describe_meta_parameter(name: &str, required: bool, description: &str, location: &str) -> Value {
    json!({
        "name": name,
        "location": location,
        "required": required,
        "description": description,
        "schema": { "type": parameter_schema_type(name) },
    })
}

fn describe_meta_scenario_routing(command_path: &str) -> Value {
    let mut routing = base_meta_scenario_routing();
    match command_path {
        "ve-adrive completion" => insert_completion_routing(&mut routing),
        "ve-adrive serve" => insert_serve_routing(&mut routing),
        "ve-adrive skill list" | "ve-adrive skill export" => insert_skill_routing(&mut routing),
        _ => {}
    }
    Value::Object(routing)
}

fn base_meta_scenario_routing() -> serde_json::Map<String, Value> {
    let mut routing = serde_json::Map::new();
    routing.insert(
        "dry_run".to_string(),
        json!("returns a deterministic plan without mutating local files or ADrive resources"),
    );
    routing.insert(
        "output".to_string(),
        json!("success and failure paths use Envelope plus --query and multi-format rendering"),
    );
    routing
}

fn insert_completion_routing(routing: &mut serde_json::Map<String, Value>) {
    routing.insert(
        "install_flow".to_string(),
        json!("the command returns an Envelope; install by extracting data.script, then source bash output, add ~/.zfunc to zsh fpath and run compinit, write fish output under ~/.config/fish/completions, or append PowerShell output to $PROFILE"),
    );
    routing.insert(
        "registered_command_names".to_string(),
        json!("generated scripts register ve-adrive-cli and ve-adrive"),
    );
}

fn insert_serve_routing(routing: &mut serde_json::Map<String, Value>) {
    routing.insert(
        "transport_matrix".to_string(),
        json!("stdio uses stdin/stdout and opens no TCP listener; sse starts a local rmcp HTTP/SSE listener on 127.0.0.1:<port>"),
    );
    routing.insert(
        "tool_source".to_string(),
        json!("MCP tools are rebuilt from the in-process skill registry; exported Markdown skill files are not read by serve"),
    );
    routing.insert(
        "call_semantics".to_string(),
        json!(
            "tools/call plans by default; include execute=true to run the underlying CLI command"
        ),
    );
}

fn insert_skill_routing(routing: &mut serde_json::Map<String, Value>) {
    routing.insert(
        "format".to_string(),
        json!("Markdown SKILL.md pack with root index plus per-domain command skills"),
    );
    routing.insert(
        "consumers".to_string(),
        json!("external Agent catalogs, prompt context, documentation generators, adapters, and MCP tool advertisement"),
    );
    routing.insert(
        "serve_relationship".to_string(),
        json!(
            "serve uses the same live registry data but does not read the exported Markdown skill directory"
        ),
    );
}

fn describe_meta_examples(command_path: &str) -> Vec<String> {
    match command_path {
        "ve-adrive completion" => vec![
            public_adrive_command("ve-adrive completion bash --output json"),
            format!(
                "{} | jq -r '.data.script' > ~/.ve-adrive-completion.bash",
                public_adrive_command("ve-adrive completion bash --output json")
            ),
            format!(
                "{} | jq -r '.data.script' > ~/.zfunc/_ve-adrive",
                public_adrive_command("ve-adrive completion zsh --output json")
            ),
        ],
        "ve-adrive serve" => vec![
            public_adrive_command("ve-adrive serve --mcp"),
            public_adrive_command("ve-adrive serve --mcp --transport sse --port 9090"),
            public_adrive_command("ve-adrive serve --mcp --dry-run --output json"),
        ],
        "ve-adrive skill list" => vec![
            public_adrive_command("ve-adrive skill list"),
            public_adrive_command("ve-adrive skill list --language zh"),
        ],
        "ve-adrive skill export" => vec![
            public_adrive_command("ve-adrive skill export --dir ./ve-adrive-skills"),
            public_adrive_command(
                "ve-adrive skill export --name ve_adrive_ls --dir ./ve-adrive-skills --dry-run --output json",
            ),
            public_adrive_command("ve-adrive skill export --language zh --dir ./ve-adrive-skills-zh"),
        ],
        _ => vec![public_adrive_command(command_path)],
    }
}

fn redact_adrive_effective(effective: EffectiveProfile) -> EffectiveProfile {
    let mut redacted = redact_effective(effective);
    // Keep ADrive config show aligned with runtime profile loading:
    // ADrive does not inherit shared TOS network settings or credentials.
    if redacted.region.source == FieldSource::Shared {
        redacted.region.value = None;
        redacted.region.source = FieldSource::Unset;
    }
    if redacted.endpoint.source == FieldSource::Shared {
        redacted.endpoint.value = None;
        redacted.endpoint.source = FieldSource::Unset;
    }
    if redacted.control_endpoint.source == FieldSource::Shared {
        redacted.control_endpoint.value = None;
        redacted.control_endpoint.source = FieldSource::Unset;
    }
    if redacted.access_key_id.source == FieldSource::Shared {
        redacted.access_key_id.value = None;
        redacted.access_key_id.source = FieldSource::Unset;
    }
    if redacted.secret_access_key.source == FieldSource::Shared {
        redacted.secret_access_key.value = None;
        redacted.secret_access_key.source = FieldSource::Unset;
    }
    if redacted.security_token.source == FieldSource::Shared {
        redacted.security_token.value = None;
        redacted.security_token.source = FieldSource::Unset;
    }
    redacted
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
            .map(|value| value.to_string())
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

/// Handle ADrive completion.
pub async fn handle_completion_command(
    global: &GlobalArgs,
    args: &CompletionArgs,
) -> Result<i32, CliError> {
    if global.describe {
        let description =
            describe_adrive_command_metadata("ve-adrive completion").ok_or_else(|| {
                CliError::ValidationError(
                    "no metadata registered for ve-adrive completion".to_string(),
                )
            })?;
        output_result(
            global,
            &Envelope::success("ve-adrive completion", description),
        )?;
        return Ok(0);
    }
    let script = completion_script(&args.shell)?;
    let envelope = Envelope::success(
        "ve-adrive completion",
        json!({
            "shell": &args.shell,
            "script": script,
            "command_count": capabilities().len(),
            "status": "generated",
            "message": format!("Shell completion for {} generated", args.shell),
        }),
    );
    output_envelope(global, &envelope)?;
    Ok(0)
}

/// Handle ADrive MCP serving.
pub async fn handle_serve_command(global: &GlobalArgs, args: &ServeArgs) -> Result<i32, CliError> {
    let transport = args.transport.as_str();
    if args.mcp && !global.dry_run && !global.describe {
        match transport {
            "stdio" => run_mcp_stdio(global).await?,
            "sse" => run_mcp_sse(global, args.port).await?,
            other => {
                return Err(CliError::ValidationError(format!(
                    "unsupported serve transport '{}': expected stdio or sse",
                    other
                )));
            }
        }
        return Ok(0);
    }
    output_envelope(
        global,
        &Envelope::success("ve-adrive serve", serve_plan(args)?),
    )?;
    Ok(0)
}

fn serve_plan(args: &ServeArgs) -> Result<Value, CliError> {
    if !matches!(args.transport.as_str(), "stdio" | "sse") {
        return Err(CliError::ValidationError(format!(
            "unsupported serve transport '{}': expected stdio or sse",
            args.transport
        )));
    }
    let is_sse = args.transport == "sse";
    Ok(json!({
        "mode": if args.mcp { "mcp" } else { "registry" },
        "transport": &args.transport,
        "port": is_sse.then_some(args.port),
        "protocol": "MCP standard protocol via rmcp",
        "tcp_listener": is_sse,
        "bind": is_sse.then(|| format!("127.0.0.1:{}", args.port)),
        "endpoints": if is_sse { vec!["/sse", "/message"] } else { Vec::new() },
        "tool_source": "In-process ADrive skill registry; exported Markdown skill files are not read by serve.",
        "call_semantics": "tools/call plans by default; include execute=true to run the underlying CLI command.",
        "capabilities": capabilities().len(),
        "skill_domains": command_domains(),
        "status": "planned_not_started",
        "message": "ADrive serve exposes registry-backed MCP tools; long-running startup is intentionally deferred for dry-run/describe",
    }))
}

async fn run_mcp_stdio(global: &GlobalArgs) -> Result<(), CliError> {
    build_mcp_server(global)?
        .run_stdio()
        .await
        .map_err(CliError::Io)?;
    Ok(())
}

async fn run_mcp_sse(global: &GlobalArgs, port: u16) -> Result<(), CliError> {
    let bind: SocketAddr = ([127, 0, 0, 1], port).into();
    build_mcp_server(global)?
        .run_sse(bind)
        .await
        .map_err(CliError::Io)?;
    Ok(())
}

fn build_mcp_server(global: &GlobalArgs) -> Result<tos_core::mcp::TosMcpServer, CliError> {
    use std::sync::Arc;
    use tos_core::mcp::{
        ToolDispatcher, ToolEntry, ToolInvocation, ToolInvocationResult, TosMcpServer,
    };

    let entries = skill_definitions()
        .into_iter()
        .map(|skill| {
            ToolEntry::from_parts(
                skill.name,
                skill.description,
                skill.input_schema,
                matches!(skill.risk_level.as_str(), "high" | "critical"),
            )
        })
        .collect::<Vec<_>>();

    struct ADriveDispatcher {
        global: GlobalArgs,
    }

    impl ToolDispatcher for ADriveDispatcher {
        fn dispatch<'a>(
            &'a self,
            invocation: ToolInvocation,
        ) -> tos_core::mcp::server::DispatchFuture<'a> {
            Box::pin(async move {
                match mcp_invoke_tool(&self.global, invocation.name, invocation.arguments).await {
                    Ok((payload, is_error)) => Ok(ToolInvocationResult { payload, is_error }),
                    Err(err) => Err(err.to_string()),
                }
            })
        }
    }

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(ADriveDispatcher {
        global: global.clone(),
    });
    Ok(TosMcpServer::new(
        "adrive-uni-cli",
        env!("CARGO_PKG_VERSION"),
        entries,
        dispatcher,
    ))
}

async fn mcp_invoke_tool(
    global: &GlobalArgs,
    name: String,
    arguments: Value,
) -> Result<(Value, bool), CliError> {
    let skill = skill_definitions()
        .into_iter()
        .find(|skill| skill.name == name)
        .ok_or_else(|| CliError::ValidationError(format!("unknown MCP tool '{}'", name)))?;
    mcp_execute_typed_command(global, &skill, &arguments).await
}

async fn mcp_execute_typed_command(
    global: &GlobalArgs,
    skill: &SkillDefinition,
    arguments: &Value,
) -> Result<(Value, bool), CliError> {
    let object = arguments.as_object().ok_or_else(|| {
        CliError::ValidationError(format!("{} arguments must be a JSON object", skill.name))
    })?;
    let execute = bool_field(object, "execute").unwrap_or(false);
    let argv = build_mcp_typed_argv(global, &skill.internal_command, object)?;
    if !execute {
        return Ok((
            json!({
                "command": skill.command,
                "argv": argv,
                "execution_status": "planned_not_executed",
            }),
            false,
        ));
    }
    if skill.internal_command == "ve-adrive serve" {
        // [Review Fix #1] Do not allow a tool call to start another long-running MCP server inside the active MCP request.
        return Err(CliError::ValidationError(
            "ve_adrive_serve MCP tool only supports planning; omit execute=true and use dry_run/describe"
                .to_string(),
        ));
    }
    let result = run_mcp_typed_argv(&skill.command, argv).await?;
    let is_error = result.exit_code.unwrap_or(1) != 0;
    let payload = serde_json::to_value(result).map_err(CliError::Json)?;
    Ok((payload, is_error))
}

fn build_mcp_typed_argv(
    global: &GlobalArgs,
    command: &str,
    arguments: &serde_json::Map<String, Value>,
) -> Result<Vec<String>, CliError> {
    let row = find_capability(command).ok_or_else(|| {
        CliError::ValidationError(format!("unknown typed MCP command '{}'", command))
    })?;
    let mut argv = Vec::new();
    push_mcp_global_args(global, arguments, &mut argv)?;
    push_mcp_public_command_path(command, &mut argv);
    push_mcp_command_args(row, arguments, &mut argv)?;
    Ok(argv)
}

fn push_mcp_public_command_path(command: &str, argv: &mut Vec<String>) {
    let mut parts = command.split_whitespace();
    let Some(first_part) = parts.next() else {
        return;
    };
    // [Review Fix #26] MCP subprocess execution uses the canonical public
    // top-level command directly; old `adrive` command paths are unsupported.
    argv.push(first_part.to_string());
    argv.extend(parts.map(ToString::to_string));
}

fn push_mcp_global_args(
    global: &GlobalArgs,
    arguments: &serde_json::Map<String, Value>,
    argv: &mut Vec<String>,
) -> Result<(), CliError> {
    argv.push("--output".to_string());
    argv.push(
        string_field(arguments, "output")
            .unwrap_or("json")
            .to_string(),
    );
    argv.push("--profile".to_string());
    argv.push(
        string_field(arguments, "profile")
            .unwrap_or(&global.profile)
            .to_string(),
    );
    for (field, flag, fallback) in [
        ("region", "--region", global.region.as_deref()),
        ("endpoint", "--endpoint", global.endpoint.as_deref()),
    ] {
        if let Some(value) = string_field(arguments, field).or(fallback) {
            argv.push(flag.to_string());
            argv.push(value.to_string());
        }
    }
    for (field, flag, fallback) in [
        ("dry_run", "--dry-run", global.dry_run),
        ("describe", "--describe", global.describe),
        ("verbose", "--verbose", global.verbose),
        ("quiet", "--quiet", global.quiet),
    ] {
        if bool_field(arguments, field).unwrap_or(fallback) {
            argv.push(flag.to_string());
        }
    }
    Ok(())
}

fn push_mcp_command_args(
    row: &CapabilityRow,
    arguments: &serde_json::Map<String, Value>,
    argv: &mut Vec<String>,
) -> Result<(), CliError> {
    let reserved = [
        "execute", "output", "profile", "region", "endpoint", "dry_run", "describe", "verbose",
        "quiet",
    ];
    for key in arguments.keys() {
        if reserved.contains(&key.as_str()) {
            continue;
        }
        if !row.parameters.iter().any(|param| param.name == key) {
            return Err(CliError::ValidationError(format!(
                "unknown argument '{}' for MCP tool '{}'",
                key, row.command
            )));
        }
    }
    for parameter in row
        .parameters
        .iter()
        .filter(|parameter| is_positional_parameter(row.command, parameter.name))
    {
        if let Some(value) = arguments.get(parameter.name) {
            push_mcp_argument_value(argv, None, value)?;
        } else if parameter.required {
            return Err(CliError::ValidationError(format!(
                "missing required argument '{}' for MCP tool '{}'",
                parameter.name, row.command
            )));
        }
    }
    for parameter in row
        .parameters
        .iter()
        .filter(|parameter| !is_positional_parameter(row.command, parameter.name))
    {
        let Some(value) = arguments.get(parameter.name) else {
            continue;
        };
        let flag = format!("--{}", parameter.name.replace('_', "-"));
        if is_boolean_parameter(parameter.name) {
            if value.as_bool().unwrap_or(false) {
                argv.push(flag);
            }
            continue;
        }
        push_mcp_argument_value(argv, Some(&flag), value)?;
    }
    Ok(())
}

fn is_positional_parameter(command: &str, name: &str) -> bool {
    matches!(
        (command, name),
        (
            "ve-adrive cp" | "ve-adrive mv" | "ve-adrive sync",
            "source" | "destination"
        ) | (
            "ve-adrive ls" | "ve-adrive crt" | "ve-adrive del" | "ve-adrive rm",
            "path"
        ) | ("ve-adrive api", "group" | "action")
            | ("ve-adrive completion", "shell")
    )
}

fn push_mcp_argument_value(
    argv: &mut Vec<String>,
    flag: Option<&str>,
    value: &Value,
) -> Result<(), CliError> {
    match value {
        Value::Null => Ok(()),
        Value::Array(values) => {
            for item in values {
                push_mcp_argument_value(argv, flag, item)?;
            }
            Ok(())
        }
        Value::String(_) | Value::Bool(_) | Value::Number(_) => {
            if let Some(flag) = flag {
                argv.push(flag.to_string());
            }
            argv.push(value_to_cli_string(value)?);
            Ok(())
        }
        Value::Object(_) => Err(CliError::ValidationError(
            "MCP typed command arguments must be scalar values or arrays".to_string(),
        )),
    }
}

fn value_to_cli_string(value: &Value) -> Result<String, CliError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Null => Ok(String::new()),
        Value::Array(_) | Value::Object(_) => Err(CliError::ValidationError(
            "MCP typed command argument cannot be converted to a CLI scalar".to_string(),
        )),
    }
}

async fn run_mcp_typed_argv(
    command: &str,
    argv: Vec<String>,
) -> Result<McpCommandExecution, CliError> {
    let exe = std::env::current_exe()?;
    let output = timeout(
        Duration::from_secs(300),
        TokioCommand::new(exe).args(&argv).output(),
    )
    .await
    .map_err(|_| {
        CliError::ValidationError(format!(
            "MCP typed command '{}' timed out after 300 seconds",
            command
        ))
    })??;
    Ok(McpCommandExecution {
        command: command.to_string(),
        argv,
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn string_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn bool_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key).and_then(Value::as_bool)
}

// ---------------------------------------------------------------------------
// Doctor types (local, aligned with TOS doctor)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: &'static str,
    status: &'static str,
    message: String,
    details: Value,
}

#[derive(Debug, Serialize)]
struct DoctorSummary {
    total: usize,
    passed: usize,
    warnings: usize,
    failed: usize,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    profile: String,
    checks: Vec<DoctorCheck>,
    summary: DoctorSummary,
}

/// Handle ADrive doctor.
pub async fn handle_doctor_command(
    global: &GlobalArgs,
    args: &DoctorArgs,
) -> Result<i32, CliError> {
    let report = doctor_report(global, args).await?;
    let envelope = Envelope::success("ve-adrive doctor", serde_json::to_value(&report).unwrap());
    output_envelope(global, &envelope)?;
    Ok(0)
}

async fn doctor_report(global: &GlobalArgs, args: &DoctorArgs) -> Result<DoctorReport, CliError> {
    let checks = build_doctor_checks(global, args).await?;
    let passed = checks.iter().filter(|c| c.status == "passed").count();
    let warnings = checks.iter().filter(|c| c.status == "warning").count();
    let failed = checks.iter().filter(|c| c.status == "failed").count();
    Ok(DoctorReport {
        profile: global.profile.clone(),
        summary: DoctorSummary {
            total: checks.len(),
            passed,
            warnings,
            failed,
        },
        checks,
    })
}

fn completion_script(shell: &str) -> Result<String, CliError> {
    let commands = completion_words().join(" ");
    match shell {
        "bash" => Ok(format!(
            "_adrive_complete() {{\n  local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n  if [[ \"${{COMP_WORDS[0]}}\" == \"ve-storage-uni-cli\" ]]; then\n    if [[ \"$COMP_CWORD\" -eq 1 ]]; then\n      COMPREPLY=( $(compgen -W \"ve-adrive\" -- \"$cur\") )\n      return\n    fi\n    [[ \"${{COMP_WORDS[1]}}\" == \"ve-adrive\" ]] || return\n  fi\n  COMPREPLY=( $(compgen -W \"{commands}\" -- \"$cur\") )\n}}\ncomplete -F _adrive_complete ve-adrive\ncomplete -F _adrive_complete ve-adrive-cli\ncomplete -F _adrive_complete ve-storage-uni-cli"
        )),
        "zsh" => Ok(format!(
            "#compdef ve-adrive ve-adrive-cli ve-storage-uni-cli\n_arguments '1:command:(ve-adrive {commands})'"
        )),
        "fish" => Ok(commands
            .split_whitespace()
            .flat_map(|cmd| {
                [
                    format!("complete -c ve-adrive -f -a {cmd}"),
                    format!("complete -c ve-adrive-cli -f -a {cmd}"),
                    format!("complete -c ve-storage-uni-cli -n '__fish_seen_subcommand_from ve-adrive' -f -a {cmd}"),
                ]
            })
            .chain(["complete -c ve-storage-uni-cli -f -a ve-adrive".to_string()])
            .collect::<Vec<_>>()
            .join("\n")),
        "powershell" | "pwsh" => Ok(format!(
            "Register-ArgumentCompleter -Native -CommandName ve-adrive,ve-adrive-cli,ve-storage-uni-cli -ScriptBlock {{\n  param($wordToComplete, $commandAst, $cursorPosition)\n  @('ve-adrive',{cmds}) | Where-Object {{ $_ -like \"$wordToComplete*\" }} | ForEach-Object {{ [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }}\n}}\n",
            cmds = commands
                .split_whitespace()
                .map(|command| format!("'{}'", command.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(",")
        )),
        other => Err(CliError::ValidationError(format!(
            "unsupported completion shell '{}': expected bash, zsh, fish, or powershell",
            other
        ))),
    }
}

fn completion_words() -> Vec<&'static str> {
    let mut words = capabilities()
        .iter()
        .filter_map(|row| row.command.strip_prefix("ve-adrive "))
        .filter_map(|suffix| suffix.split_whitespace().next())
        .collect::<Vec<_>>();
    words.sort_unstable();
    words.dedup();
    words
}

async fn build_doctor_checks(
    global: &GlobalArgs,
    args: &DoctorArgs,
) -> Result<Vec<DoctorCheck>, CliError> {
    let selected = args.check.as_deref();
    let mut checks = Vec::new();
    maybe_push_check_result(&mut checks, selected, "config", || config_check(global));
    maybe_push_check_result(&mut checks, selected, "auth", || auth_check(global));
    maybe_push_check(&mut checks, selected, "registry", registry_check);
    // network_check is async because --live-network performs a real HTTPS probe.
    let is_network_selected = selected.map(|name| name == "network").unwrap_or(true);
    if is_network_selected {
        match network_check(global, args).await {
            Ok(check) => checks.push(check),
            Err(err) => checks.push(DoctorCheck {
                name: "network",
                status: "failed",
                message: err.to_string(),
                details: json!({ "recoverable": true }),
            }),
        }
    }
    maybe_push_check(&mut checks, selected, "mcp", mcp_check);
    maybe_push_check(&mut checks, selected, "completion", completion_check);
    if selected == Some("principles") {
        // [Review Fix #DoctorLazy] Keep ordinary doctor output fast; run the
        // cross-surface invariant check only when explicitly requested.
        checks.push(principles_check());
    }
    if checks.is_empty() {
        return Err(CliError::ValidationError(format!(
            "unknown doctor check '{}': expected auth, config, registry, network, mcp, principles, or completion",
            selected.unwrap_or_default()
        )));
    }
    Ok(checks)
}

fn maybe_push_check<F>(
    checks: &mut Vec<DoctorCheck>,
    selected: Option<&str>,
    name: &'static str,
    build: F,
) where
    F: FnOnce() -> DoctorCheck,
{
    if selected
        .map(|selected_name| selected_name == name)
        .unwrap_or(true)
    {
        checks.push(build());
    }
}

fn maybe_push_check_result<F>(
    checks: &mut Vec<DoctorCheck>,
    selected: Option<&str>,
    name: &'static str,
    build: F,
) where
    F: FnOnce() -> Result<DoctorCheck, CliError>,
{
    let is_selected = selected.map(|n| n == name).unwrap_or(true);
    if !is_selected {
        return;
    }
    match build() {
        Ok(check) => checks.push(check),
        Err(err) => checks.push(DoctorCheck {
            name,
            status: "failed",
            message: err.to_string(),
            details: json!({ "recoverable": true }),
        }),
    }
}

fn config_check(global: &GlobalArgs) -> Result<DoctorCheck, CliError> {
    let path = global.config_path();
    let profile = build_profile(global)?;
    Ok(DoctorCheck {
        name: "config",
        status: if profile.endpoint.is_some() || profile.region.is_some() {
            "passed"
        } else {
            "warning"
        },
        message: "effective ADrive profile loaded".to_string(),
        details: json!({
            "config_exists": path.exists(),
            "config_path": path.display().to_string(),
            "has_endpoint": profile.endpoint.is_some(),
            "has_region": profile.region.is_some(),
        }),
    })
}

fn auth_check(global: &GlobalArgs) -> Result<DoctorCheck, CliError> {
    let profile = build_profile(global)?;
    let has_access_key = profile.access_key_id.is_some();
    let has_secret_key = profile.secret_access_key.is_some();
    let status = if has_access_key && has_secret_key {
        "passed"
    } else {
        "warning"
    };
    let message = if has_access_key && has_secret_key {
        "ADrive credentials are configured".to_string()
    } else {
        "ADrive credentials are incomplete (check ADRIVE_ACCESS_KEY / ADRIVE_SECRET_KEY or config file)"
            .to_string()
    };
    Ok(DoctorCheck {
        name: "auth",
        status,
        message,
        details: json!({
            "has_access_key": has_access_key,
            "has_secret_key": has_secret_key,
            "has_security_token": profile.security_token.is_some(),
        }),
    })
}

async fn network_check(global: &GlobalArgs, args: &DoctorArgs) -> Result<DoctorCheck, CliError> {
    let profile = build_profile(global)?;
    let resolved = resolve_endpoint_and_region(profile.endpoint.clone(), profile.region.clone());
    let endpoint = resolved.as_ref().ok().map(|(endpoint, _)| endpoint.clone());

    // Without --live-network, retain offline-safe behavior so `ve-adrive-cli doctor`
    // works in air-gapped environments.
    if !args.live_network {
        return Ok(DoctorCheck {
            name: "network",
            status: if resolved.is_ok() {
                "passed"
            } else {
                "warning"
            },
            message: resolved
                .as_ref()
                .map(|_| "network endpoint is explicit or derived from ADRIVE_REGION".to_string())
                .unwrap_or_else(|err| err.to_string()),
            details: json!({
                "endpoint": endpoint,
                "has_explicit_endpoint": profile.endpoint.is_some(),
                "has_region": profile.region.is_some(),
                "live_check": false,
                "hint": "pass --live-network to perform a real probe",
            }),
        });
    }

    // Live probe: HTTPS HEAD against the configured endpoint with a tight
    // timeout. Even a 403 proves the host is reachable.
    let Some(target) = endpoint else {
        return Ok(DoctorCheck {
            name: "network",
            status: "warning",
            message: resolved
                .err()
                .map(|err| err.to_string())
                .unwrap_or_else(|| "no endpoint configured; cannot probe".to_string()),
            details: json!({ "live_check": true, "skipped": true }),
        });
    };

    let url = if target.starts_with("http://") || target.starts_with("https://") {
        target.clone()
    } else {
        format!("https://{}", target)
    };
    let timeout_dur = std::time::Duration::from_millis(args.network_timeout_ms);
    let client = match reqwest::Client::builder()
        .user_agent(storage_user_agent())
        .timeout(timeout_dur)
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            return Ok(DoctorCheck {
                name: "network",
                status: "failed",
                message: format!("failed to build HTTP client: {err}"),
                details: json!({ "live_check": true, "url": url }),
            });
        }
    };

    let started = std::time::Instant::now();
    let probe = client.head(&url).send().await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match probe {
        Ok(resp) => {
            let status_code = resp.status();
            let outcome = if status_code.is_server_error() {
                "warning"
            } else {
                "passed"
            };
            Ok(DoctorCheck {
                name: "network",
                status: outcome,
                message: format!(
                    "reached {} in {}ms (HTTP {})",
                    url,
                    latency_ms,
                    status_code.as_u16()
                ),
                details: json!({
                    "live_check": true,
                    "url": url,
                    "http_status": status_code.as_u16(),
                    "latency_ms": latency_ms,
                }),
            })
        }
        Err(err) => Ok(DoctorCheck {
            name: "network",
            status: "failed",
            message: format!("probe failed after {}ms: {}", latency_ms, err),
            details: json!({
                "live_check": true,
                "url": url,
                "latency_ms": latency_ms,
                "error": err.to_string(),
                "is_timeout": err.is_timeout(),
                "is_connect": err.is_connect(),
            }),
        }),
    }
}

fn registry_check() -> DoctorCheck {
    DoctorCheck {
        name: "registry",
        status: "passed",
        message: "capability registry is available".to_string(),
        details: json!({
            "capabilities": capabilities().len(),
            "domains": command_domains(),
        }),
    }
}

fn principles_check() -> DoctorCheck {
    let rows = capabilities();
    let missing_domain: Vec<&str> = rows
        .iter()
        .filter(|row| row.domain.is_empty())
        .map(|row| row.command)
        .collect();
    let destructive_without_force: Vec<&str> = rows
        .iter()
        .filter(|row| row.destructive && !row.supports_force)
        .map(|row| row.command)
        .collect();
    let skill_domains = skill_definitions()
        .iter()
        .map(|skill| skill.domain.clone())
        .collect::<std::collections::BTreeSet<_>>();
    // Skill domains use the business taxonomy (adrive-transfer/-shared/-admin),
    // so compare coverage against `business_domains()` rather than command roots.
    let uncovered_skill_domains: Vec<&str> = business_domains()
        .into_iter()
        .filter(|domain| !skill_domains.contains(*domain))
        .collect();
    let exposed_low_level: Vec<&str> = rows
        .iter()
        .filter(|row| normalize_facet(row.layer) == "low_level")
        .map(|row| row.command)
        .collect();
    let passed = missing_domain.is_empty()
        && destructive_without_force.is_empty()
        && uncovered_skill_domains.is_empty()
        && exposed_low_level.is_empty();
    DoctorCheck {
        name: "principles",
        status: if passed { "passed" } else { "failed" },
        message: if passed {
            "six-principle invariants are upheld by the ADrive registry".to_string()
        } else {
            "six-principle invariants failed".to_string()
        },
        details: json!({
            "capabilities": rows.len(),
            "skill_definitions": skill_domains.len(),
            "missing_domain": missing_domain,
            "destructive_force_violations": destructive_without_force,
            "exposed_unimplemented_low_level": exposed_low_level,
            "skill_domains": skill_domains.into_iter().collect::<Vec<_>>(),
            "uncovered_skill_domains": uncovered_skill_domains,
            "principle_keys": [
                "discovery",
                "understanding",
                "safe_execution",
                "controlled_output",
                "deterministic_errors",
                "agent_ecosystem"
            ],
        }),
    }
}

fn mcp_check() -> DoctorCheck {
    DoctorCheck {
        name: "mcp",
        status: "passed",
        message: "MCP runtime is available for stdio and SSE transports".to_string(),
        details: json!({
            "capabilities": capabilities().len(),
            "runtime": "available",
            "stdio_status": "available",
            "sse_status": "available",
            "default_bind": "127.0.0.1",
        }),
    }
}

fn completion_check() -> DoctorCheck {
    DoctorCheck {
        name: "completion",
        status: "passed",
        message: "completion generation is registry-backed".to_string(),
        details: json!({ "shells": ["bash", "zsh", "fish", "powershell"] }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mcp_serve_execute_is_rejected() {
        let global = GlobalArgs::default();
        let result = mcp_invoke_tool(
            &global,
            "ve_adrive_serve".to_string(),
            json!({"execute": true, "mcp": true}),
        )
        .await;

        assert!(matches!(
            result,
            Err(CliError::ValidationError(message))
                if message.contains("only supports planning")
        ));
    }
}
