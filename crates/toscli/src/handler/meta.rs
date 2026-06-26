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
use tos_core::agent::describe::{CommandDescription, CommandLayer, RiskLevel};
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::client::storage_user_agent;
use tos_core::infra::config::{merge_tos_runtime_profile, Binary, ConfigFile, Profile};

use crate::cli::meta::{
    ApiArgs, CapabilitiesArgs, CompletionArgs, ConfigCommand, DoctorArgs, DocumentationLanguage,
    ServeArgs, SkillAction, SkillCommand,
};
use crate::registry::{
    business_domain, capabilities, command_domains, find_capability, public_tos_command,
    CapabilityRow,
};

const TOS_CONFIG_BINARY_ENV: &str = "VE_STORAGE_UNI_TOS_CONFIG_BINARY";

struct EnvGuard {
    previous: Option<String>,
}

impl EnvGuard {
    fn tos_namespace() -> Self {
        let previous = std::env::var(TOS_CONFIG_BINARY_ENV).ok();
        std::env::set_var(TOS_CONFIG_BINARY_ENV, "tos");
        Self { previous }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(value) = self.previous.take() {
            std::env::set_var(TOS_CONFIG_BINARY_ENV, value);
        } else {
            std::env::remove_var(TOS_CONFIG_BINARY_ENV);
        }
    }
}

#[derive(Debug, Serialize)]
struct SkillDefinition {
    schema_version: &'static str,
    name: String,
    domain: String,
    command: String,
    description: String,
    risk_level: String,
    input_schema: Value,
    examples: Vec<String>,
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: &'static str,
    status: &'static str,
    message: String,
    details: Value,
}

#[derive(Debug, Serialize)]
struct McpCommandExecution {
    command: String,
    argv: Vec<String>,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

pub async fn handle_capabilities_command(
    global: &GlobalArgs,
    args: &CapabilitiesArgs,
) -> Result<i32, CliError> {
    let mut rows = capabilities()
        .iter()
        .filter(|row| {
            args.group
                .as_deref()
                .map(|group| row.group == group || row.domain == group)
                .unwrap_or(true)
        })
        .filter(|row| {
            args.layer
                .as_deref()
                .map(|layer| row.layer == layer)
                .unwrap_or(true)
        })
        .filter(|row| {
            args.search
                .as_deref()
                .map(|needle| row_matches_search(row, needle))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| left.command.cmp(right.command));

    let payload = match args.view.as_str() {
        "groups" => json!({
            "tool": "tos",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "byted_tos",
            "view": "groups",
            "uri_format": "tos://bucket/key",
            "implemented_layers": ["high_level", "utilities"],
            "unimplemented_layers": ["low_level"],
            "listing_semantics": {
                "delimiter": "/",
                "recursive": "expand common prefixes with delimiter=\"/\""
            },
            "groups": group_rows(&rows),
            "capabilities": [],
            "commands": [],
        }),
        "text" => json!({
            "tool": "tos",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "byted_tos",
            "view": "text",
            "lines": rows.iter().map(|row| format!("{}\t{}", row.command, row.description)).collect::<Vec<_>>(),
        }),
        "compact" | "tree" => json!({
            "tool": "tos",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "byted_tos",
            "view": if args.view == "tree" { "compact" } else { args.view.as_str() },
            "capabilities": rows.iter().map(compact_row).collect::<Vec<_>>(),
            "commands": command_domains(),
        }),
        "full" => json!({
            "tool": "tos",
            "version": env!("CARGO_PKG_VERSION"),
            "service_name": "byted_tos",
            "view": "full",
            "uri_format": "tos://bucket/key",
            "capabilities": rows.iter().map(public_row).collect::<Vec<_>>(),
            "commands": command_domains(),
            "high_level_semantics": {
                "list": "All object/prefix listing scenarios use delimiter=\"/\".",
                "delete": "Recursive deletes use planned object deletes; bottom-up directory delete ordering is not required."
            },
        }),
        other => {
            return Err(CliError::ValidationError(format!(
                "unsupported capabilities view '{}': expected groups, text, compact, full, or tree",
                other
            )))
        }
    };
    ve_tos_cli::handler::common::output_result(
        global,
        &Envelope::success("tos capabilities", payload),
    )?;
    Ok(0)
}

pub async fn handle_api_command(global: &GlobalArgs, args: &ApiArgs) -> Result<i32, CliError> {
    let command = format!("tos api {} {}", args.group, args.action);
    if args.describe || global.describe {
        let capability = find_capability("tos api")
            .map(|row| compact_row(&row))
            .unwrap_or_else(|| json!({"command": "tos api", "mode": "guarded_utility"}));
        let desc = json!({
            "command": command,
            "description": format!(
                "Guarded TOS utility API metadata for {}.{}; tos-cli executes high-level workflows only",
                args.group, args.action
            ),
            "service": "tos",
            "capability": capability,
            "mode": "guarded_utility",
            "layer": "meta",
            "raw_api_execution_implemented": false,
            "supports_dry_run": true,
            "supports_force": false,
            "endpoint_mode_only": true,
        });
        ve_tos_cli::handler::common::output_result(global, &Envelope::success(command, desc))?;
        return Ok(0);
    }

    let request = parse_optional_request(args.request.as_deref())?;
    if !global.dry_run {
        return Err(CliError::ValidationError(
            "tos-cli raw API execution is not implemented; use --dry-run to inspect the planned request or --describe for metadata"
                .to_string(),
        ));
    }

    let payload = json!({
        "group": &args.group,
        "action": &args.action,
        "request": request,
        "status": "planned_not_executed",
        "mode": "guarded_utility",
        "raw_api_execution_implemented": false,
        "endpoint_mode_only": true,
        "message": "tos-cli exposes high-level TOS workflows and utilities; raw API execution is intentionally disabled",
    });
    ve_tos_cli::handler::common::output_result(global, &Envelope::success(command, payload))?;
    Ok(0)
}

pub async fn handle_config_command(
    global: &GlobalArgs,
    command: &ConfigCommand,
) -> Result<i32, CliError> {
    let _guard = EnvGuard::tos_namespace();
    ve_tos_cli::handler::config::handle_config_command(global, &command.action).await
}

pub async fn handle_completion_command(
    global: &GlobalArgs,
    args: &CompletionArgs,
) -> Result<i32, CliError> {
    if global.describe {
        let desc = describe_tos_command_metadata("tos completion").ok_or_else(|| {
            CliError::ValidationError("no metadata registered for tos completion".into())
        })?;
        ve_tos_cli::handler::common::output_result(
            global,
            &Envelope::success("tos completion", desc),
        )?;
        return Ok(0);
    }
    let script = completion_script(&args.shell)?;
    ve_tos_cli::handler::common::output_result(
        global,
        &Envelope::success(
            "tos completion",
            json!({
                "shell": args.shell,
                "script": script,
                "command_count": capabilities().len(),
                "status": "generated",
            }),
        ),
    )?;
    Ok(0)
}

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
                )))
            }
        }
        return Ok(0);
    }
    ve_tos_cli::handler::common::output_result(
        global,
        &Envelope::success("tos serve", serve_plan(args)?),
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
        "transport": args.transport,
        "port": is_sse.then_some(args.port),
        "protocol": "MCP standard protocol via rmcp",
        "tcp_listener": is_sse,
        "bind": is_sse.then(|| format!("127.0.0.1:{}", args.port)),
        "endpoints": if is_sse { vec!["/sse", "/message"] } else { Vec::new() },
        "tool_source": "In-process tos-cli high-level skill registry; exported Markdown skills are not read by serve.",
        "call_semantics": "tools/call plans by default; include execute=true to run the underlying CLI command.",
        "capabilities": capabilities().len(),
        "skill_domains": command_domains(),
        "status": "planned_not_started",
        "message": "tos serve exposes registry-backed high-level TOS MCP tools; long-running startup is intentionally deferred for dry-run/describe",
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

    struct TosCliDispatcher {
        global: GlobalArgs,
    }

    impl ToolDispatcher for TosCliDispatcher {
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

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(TosCliDispatcher {
        global: global.clone(),
    });
    Ok(TosMcpServer::new(
        "tos-cli",
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
    let argv = build_mcp_typed_argv(global, &skill.command, object)?;
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
    if skill.command == "tos serve" {
        return Err(CliError::ValidationError(
            "tos_serve MCP tool only supports planning; omit execute=true and use dry_run/describe"
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
    push_mcp_global_args(global, arguments, &mut argv);
    push_mcp_public_command_path(command, &mut argv);
    push_mcp_command_args(row, arguments, &mut argv)?;
    Ok(argv)
}

fn push_mcp_public_command_path(command: &str, argv: &mut Vec<String>) {
    let mut parts = command.split_whitespace();
    let Some(first_part) = parts.next() else {
        return;
    };
    let is_direct_tos_exe = std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_string)
        })
        .map(|stem| matches!(stem.as_str(), "tos" | "tos-cli"))
        .unwrap_or(false);
    if !(is_direct_tos_exe && first_part == "tos") {
        argv.push(first_part.to_string());
    }
    argv.extend(parts.map(ToString::to_string));
}

fn push_mcp_global_args(
    global: &GlobalArgs,
    arguments: &serde_json::Map<String, Value>,
    argv: &mut Vec<String>,
) {
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
        ("tos cp" | "tos mv" | "tos sync", "source" | "destination")
            | (
                "tos ls"
                    | "tos mkdir"
                    | "tos rm"
                    | "tos stat"
                    | "tos du"
                    | "tos find"
                    | "tos cat"
                    | "tos put"
                    | "tos presign",
                "path"
            )
            | ("tos api", "group" | "action")
            | ("tos completion", "shell")
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

pub async fn handle_skill_command(
    global: &GlobalArgs,
    command: &SkillCommand,
) -> Result<i32, CliError> {
    match &command.action {
        SkillAction::List { language } => {
            ve_tos_cli::handler::common::output_result(
                global,
                &Envelope::success(
                    "tos skill list",
                    json!({
                        "language": language.code(),
                        "skills": skill_definitions_for_language(*language),
                    }),
                ),
            )?;
        }
        SkillAction::Export {
            name,
            dir,
            language,
        } => {
            let export_plan = skill_markdown_export_plan(name.as_deref(), dir)?;
            let payload = if global.dry_run {
                plan_skill_markdown_export(&export_plan, dir, *language)
            } else {
                export_markdown_skills(export_plan, dir, *language)?
            };
            ve_tos_cli::handler::common::output_result(
                global,
                &Envelope::success("tos skill export", payload),
            )?;
        }
    }
    Ok(0)
}

pub async fn handle_doctor_command(
    global: &GlobalArgs,
    args: &DoctorArgs,
) -> Result<i32, CliError> {
    let selected = args.check.as_deref();
    let mut checks = Vec::new();
    maybe_push_result(&mut checks, selected, "config", || config_check(global));
    maybe_push_result(&mut checks, selected, "auth", || auth_check(global));
    maybe_push(&mut checks, selected, "registry", registry_check);
    if selected_matches(selected, "network") {
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
    maybe_push(&mut checks, selected, "mcp", mcp_check);
    maybe_push(&mut checks, selected, "completion", completion_check);
    if checks.is_empty() {
        return Err(CliError::ValidationError(format!(
            "unknown doctor check '{}': expected auth, config, registry, network, endpoint, mcp, or completion",
            selected.unwrap_or_default()
        )));
    }
    let failed = checks
        .iter()
        .filter(|check| check.status == "failed")
        .count();
    let warnings = checks
        .iter()
        .filter(|check| check.status == "warning")
        .count();
    let passed = checks
        .iter()
        .filter(|check| check.status == "passed")
        .count();
    ve_tos_cli::handler::common::output_result(
        global,
        &Envelope::success(
            "tos doctor",
            json!({
                "profile": global.profile,
                "checks": checks,
                "summary": {
                    "total": checks.len(),
                    "passed": passed,
                    "warnings": warnings,
                    "failed": failed
                }
            }),
        ),
    )?;
    Ok(0)
}

pub fn describe_tos_command_metadata(command: &str) -> Option<CommandDescription> {
    let row = find_capability(command)?;
    Some(CommandDescription {
        command: row.command.to_string(),
        layer: if row.layer == "high_level" {
            CommandLayer::HighLevel
        } else {
            CommandLayer::Meta
        },
        description: row.description.to_string(),
        risk_level: match row.risk_level {
            "critical" => RiskLevel::Critical,
            "high" => RiskLevel::High,
            "medium" => RiskLevel::Medium,
            _ => RiskLevel::Low,
        },
        supports_dry_run: row.supports_dry_run,
        supports_pipe: matches!(row.domain, "cat"),
        low_level_apis: Some(row.api_actions.iter().map(|api| api.to_string()).collect()),
        wraps_apis: Some(row.api_actions.iter().map(|api| api.to_string()).collect()),
        ..Default::default()
    })
}

fn row_matches_search(row: &CapabilityRow, needle: &str) -> bool {
    let needle = needle.to_ascii_lowercase();
    row.command.to_ascii_lowercase().contains(&needle)
        || row.description.to_ascii_lowercase().contains(&needle)
        || row.domain.to_ascii_lowercase().contains(&needle)
}

fn group_rows(rows: &[&CapabilityRow]) -> Vec<Value> {
    ["high_level", "utilities"]
        .into_iter()
        .map(|group| {
            let group_rows = rows
                .iter()
                .filter(|row| row.group == group)
                .collect::<Vec<_>>();
            json!({
                "name": group,
                "group": group,
                "command": if group == "high_level" { "tos" } else { "tos capabilities" },
                "layer": group,
                "description": if group == "high_level" {
                    "High-level object and bucket workflows"
                } else {
                    "Discovery, configuration, diagnostics, completion, skill, API passthrough, and serve utilities"
                },
                "implemented": true,
                "command_count": group_rows.len(),
            })
        })
        .collect()
}

fn compact_row(row: &&CapabilityRow) -> Value {
    json!({
        "command": row.command,
        "group": row.group,
        "layer": row.layer,
        "description": row.description,
        "risk_level": row.risk_level,
    })
}

fn public_row(row: &&CapabilityRow) -> Value {
    json!({
        "command": row.command,
        "group": row.group,
        "layer": row.layer,
        "description": row.description,
        "risk_level": row.risk_level,
        "destructive": row.destructive,
        "supports_force": row.supports_force,
        "supports_dry_run": row.supports_dry_run,
        "api_actions": row.api_actions,
        "parameters": row.parameters,
        "examples": row.examples.iter().map(|example| public_tos_command(example)).collect::<Vec<_>>(),
    })
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

fn completion_script(shell: &str) -> Result<String, CliError> {
    let commands = completion_words().join(" ");
    match shell {
        "bash" => Ok(format!(
            "_tos_complete() {{\n  local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n  if [[ \"${{COMP_WORDS[0]}}\" == \"ve-storage-uni-cli\" ]]; then\n    if [[ \"$COMP_CWORD\" -eq 1 ]]; then\n      COMPREPLY=( $(compgen -W \"tos\" -- \"$cur\") )\n      return\n    fi\n    [[ \"${{COMP_WORDS[1]}}\" == \"tos\" ]] || return\n  fi\n  COMPREPLY=( $(compgen -W \"{commands}\" -- \"$cur\") )\n}}\ncomplete -F _tos_complete tos\ncomplete -F _tos_complete tos-cli\ncomplete -F _tos_complete ve-storage-uni-cli"
        )),
        "zsh" => Ok(format!(
            "#compdef tos tos-cli ve-storage-uni-cli\n_arguments '1:command:(tos {commands})'"
        )),
        "fish" => Ok(commands
            .split_whitespace()
            .flat_map(|cmd| {
                [
                    format!("complete -c tos -f -a {cmd}"),
                    format!("complete -c tos-cli -f -a {cmd}"),
                    format!("complete -c ve-storage-uni-cli -n '__fish_seen_subcommand_from tos' -f -a {cmd}"),
                ]
            })
            .chain(["complete -c ve-storage-uni-cli -f -a tos".to_string()])
            .collect::<Vec<_>>()
            .join("\n")),
        "powershell" => Ok(format!(
            "Register-ArgumentCompleter -Native -CommandName tos,tos-cli,ve-storage-uni-cli -ScriptBlock {{\n  param($wordToComplete, $commandAst, $cursorPosition)\n  @('tos',{cmds}) | Where-Object {{ $_ -like \"$wordToComplete*\" }} | ForEach-Object {{ [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }}\n}}\n",
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
        .filter_map(|row| row.command.strip_prefix("tos "))
        .filter_map(|suffix| suffix.split_whitespace().next())
        .collect::<Vec<_>>();
    words.sort_unstable();
    words.dedup();
    words
}

fn skill_definitions() -> Vec<SkillDefinition> {
    capabilities()
        .iter()
        .map(|row| SkillDefinition {
            schema_version: "tos.skill.v1",
            name: row.command.replace("tos ", "tos_").replace(' ', "_"),
            domain: business_domain(row.command).to_string(),
            command: row.command.to_string(),
            description: row.description.to_string(),
            risk_level: row.risk_level.to_string(),
            input_schema: skill_input_schema(row),
            examples: row
                .examples
                .iter()
                .map(|example| public_tos_command(example))
                .collect(),
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
        public_tos_command(&skill.command),
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
        "port" | "max_keys" | "batch_concurrency" | "list_concurrency"
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
        "recursive"
            | "force"
            | "include_parent"
            | "no_clobber"
            | "no_manifest"
            | "report_failures_only"
            | "progress"
            | "no_progress"
            | "list_echo"
            | "no_list_echo"
            | "delete"
            | "human_readable"
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
    let Some(name) = name else {
        return Ok(skills);
    };
    let selected = skills
        .into_iter()
        .filter(|skill| skill.name == name || skill.command == name)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(CliError::ValidationError(format!(
            "unknown tos skill '{}'",
            name
        )));
    }
    Ok(selected)
}

fn skill_markdown_export_plan(
    name: Option<&str>,
    dir: &str,
) -> Result<Vec<(SkillDefinition, PathBuf)>, CliError> {
    let base_dir = Path::new(dir);
    Ok(selected_skills(name)?
        .into_iter()
        .map(|skill| {
            let path = base_dir
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
    // [Review Fix #SkillExportAlign] Keep tos-cli dry-run output aligned with
    // ve-tos and ve-adrive so skill pack callers can parse one schema.
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
        "skill_count": export_plan.len(),
        "entries": entries,
        "status": "planned_not_written",
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
        .map(|(skill, _)| skill)
        .collect::<Vec<_>>();
    let root_path = skill_root_path(Path::new(dir));
    if let Some(parent) = root_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&root_path, skill_index_markdown("tos", &skills, language))?;
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
    skills: &[&SkillDefinition],
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

Use this skill when the user wants to run `{command}` with the ByteCloud TOS CLI.

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

Prefer `tos {suffix} --describe` or `tos {suffix} --dry-run --output json` before executing a command that writes or deletes data. Destructive commands must include the required `--force` and exact `--confirm` target.
"#,
            name = skill.name,
            command = skill.command,
            description = skill.description,
            risk_level = skill.risk_level,
            schema = schema,
            examples = examples,
            suffix = skill
                .command
                .strip_prefix("tos ")
                .unwrap_or(skill.command.as_str()),
        ),
        DocumentationLanguage::Zh => format!(
            r#"# {name}

当用户需要通过 ByteCloud TOS CLI 运行 `{command}` 时使用此 Skill。

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

执行会写入或删除数据的命令前，优先运行 `tos {suffix} --describe` 或 `tos {suffix} --dry-run --output json`。破坏性命令必须包含必需的 `--force` 和精确匹配目标的 `--confirm`。
"#,
            name = skill.name,
            command = skill.command,
            description = localized_skill_description_zh(skill),
            risk_level = skill.risk_level,
            schema = schema,
            examples = examples,
            suffix = skill
                .command
                .strip_prefix("tos ")
                .unwrap_or(skill.command.as_str()),
        ),
    }
}

fn maybe_push<F>(
    checks: &mut Vec<DoctorCheck>,
    selected: Option<&str>,
    name: &'static str,
    build: F,
) where
    F: FnOnce() -> DoctorCheck,
{
    if selected_matches(selected, name) {
        checks.push(build());
    }
}

fn maybe_push_result<F>(
    checks: &mut Vec<DoctorCheck>,
    selected: Option<&str>,
    name: &'static str,
    build: F,
) where
    F: FnOnce() -> Result<DoctorCheck, CliError>,
{
    if !selected_matches(selected, name) {
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

fn selected_matches(selected: Option<&str>, name: &str) -> bool {
    selected
        .map(|selected_name| {
            selected_name == name || (selected_name == "endpoint" && name == "network")
        })
        .unwrap_or(true)
}

fn config_check(global: &GlobalArgs) -> Result<DoctorCheck, CliError> {
    let path = global.config_path();
    let profile = effective_tos_profile(global)?;
    let has_psm = has_non_empty_value(profile.psm.as_deref());
    Ok(DoctorCheck {
        name: "config",
        status: if profile.endpoint.is_some() || (has_psm && profile.region.is_some()) {
            "passed"
        } else {
            "warning"
        },
        message: "tos-cli configuration is read from the tos profile namespace and BYTE_TOS_* environment variables".to_string(),
        details: json!({
            "profile": global.profile,
            "config_path": path.display().to_string(),
            "config_exists": path.exists(),
            "has_endpoint": profile.endpoint.is_some(),
            "has_psm": has_psm,
            "has_region": profile.region.is_some(),
            "endpoint": profile.endpoint,
            "psm": profile.psm,
            "idc": profile.idc,
            "cluster": profile.cluster,
            "addr_family": profile.addr_family,
            "region": profile.region,
            "section": format!("{}.tos", global.profile),
            "env_prefix": "BYTE_TOS",
        }),
    })
}

fn auth_check(global: &GlobalArgs) -> Result<DoctorCheck, CliError> {
    let profile = effective_tos_profile(global)?.redacted();
    let has_access_key = profile.access_key_id.is_some();
    let has_secret_key = profile.secret_access_key.is_some();
    let has_security_token = profile.security_token.is_some();
    Ok(DoctorCheck {
        name: "auth",
        status: if has_access_key && has_secret_key {
            "passed"
        } else {
            "warning"
        },
        message: if has_access_key && has_secret_key {
            "tos-cli credentials are configured".to_string()
        } else {
            "tos-cli credentials are incomplete (check BYTE_TOS_* or [profile.tos])".to_string()
        },
        details: json!({
            "has_access_key": has_access_key,
            "has_secret_key": has_secret_key,
            "has_security_token": has_security_token,
            "access_key_id": profile.access_key_id,
            "secret_access_key": profile.secret_access_key,
            "security_token": profile.security_token,
            "env_prefix": "BYTE_TOS",
        }),
    })
}

fn registry_check() -> DoctorCheck {
    DoctorCheck {
        name: "registry",
        status: "passed",
        message: "tos-cli registry exposes high-level commands and utilities only".to_string(),
        details: json!({
            "capabilities": capabilities().len(),
            "layers": ["high_level", "utilities"],
        }),
    }
}

async fn network_check(global: &GlobalArgs, args: &DoctorArgs) -> Result<DoctorCheck, CliError> {
    let profile = effective_tos_profile(global)?;
    let has_psm = has_non_empty_value(profile.psm.as_deref());
    let Some(target) = profile.endpoint.clone() else {
        let has_region = profile.region.is_some();
        let (status, message, hint) = if has_psm && has_region {
            (
                "passed",
                "tos-cli will use PSM service discovery; live endpoint probe is skipped without --endpoint",
                "run a bucket command with --psm or configure [profile.tos].psm to validate service discovery",
            )
        } else if has_psm {
            (
                "warning",
                "tos-cli PSM service discovery is configured, but region is required for signing",
                "configure region via --region, BYTE_TOS_REGION, or [profile].region",
            )
        } else {
            (
                "warning",
                "tos-cli requires either an endpoint or PSM service discovery for network access",
                "configure endpoint via --endpoint/BYTE_TOS_ENDPOINT/[profile.tos].endpoint or PSM via --psm/BYTE_TOS_PSM/[profile.tos].psm",
            )
        };
        return Ok(DoctorCheck {
            name: "network",
            status,
            message: message.to_string(),
            details: json!({
                "endpoint_cli_override": global.endpoint,
                "endpoint": profile.endpoint,
                "psm": profile.psm,
                "idc": profile.idc,
                "cluster": profile.cluster,
                "addr_family": profile.addr_family,
                "region": profile.region,
                "has_explicit_endpoint": false,
                "has_psm": has_psm,
                "has_region": has_region,
                "psm_supported": true,
                "endpoint_mode_active": false,
                "live_check": args.live_network,
                "skipped": true,
                "hint": hint,
            }),
        });
    };

    if args.live_network {
        return live_network_check(global, args, profile, target).await;
    }

    Ok(DoctorCheck {
        name: "network",
        status: "passed",
        message: if has_psm {
            "tos-cli endpoint is configured; endpoint mode takes precedence over PSM".to_string()
        } else {
            "tos-cli endpoint is configured".to_string()
        },
        details: json!({
            "endpoint_cli_override": global.endpoint,
            "endpoint": profile.endpoint,
            "psm": profile.psm,
            "idc": profile.idc,
            "cluster": profile.cluster,
            "addr_family": profile.addr_family,
            "region": profile.region,
            "has_explicit_endpoint": profile.endpoint.is_some(),
            "has_psm": has_psm,
            "has_region": profile.region.is_some(),
            "psm_supported": true,
            "endpoint_mode_active": true,
            "live_check": false,
            "hint": "endpoint mode is active; PSM is used only when endpoint is absent",
        }),
    })
}

fn has_non_empty_value(value: Option<&str>) -> bool {
    value.map(str::trim).is_some_and(|value| !value.is_empty())
}

async fn live_network_check(
    global: &GlobalArgs,
    args: &DoctorArgs,
    profile: Profile,
    target: String,
) -> Result<DoctorCheck, CliError> {
    let url = if target.starts_with("http://") || target.starts_with("https://") {
        target
    } else {
        format!("https://{}", target)
    };
    let timeout = std::time::Duration::from_millis(args.network_timeout_ms);
    let client = reqwest::Client::builder()
        .user_agent(storage_user_agent())
        .timeout(timeout)
        .build()
        .map_err(|err| CliError::Unknown(format!("failed to build HTTP client: {err}")))?;

    let started = std::time::Instant::now();
    let probe = client.head(&url).send().await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match probe {
        Ok(response) => {
            let status = response.status();
            let outcome = if status.is_server_error() {
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
                    status.as_u16()
                ),
                details: json!({
                    "endpoint_cli_override": global.endpoint,
                    "endpoint": profile.endpoint,
                    "region": profile.region,
                    "has_explicit_endpoint": true,
                    "has_region": profile.region.is_some(),
                    "psm_supported": false,
                    "endpoint_mode_only": true,
                    "live_check": true,
                    "url": url,
                    "http_status": status.as_u16(),
                    "latency_ms": latency_ms,
                }),
            })
        }
        Err(err) => Ok(DoctorCheck {
            name: "network",
            status: "failed",
            message: format!("probe failed after {}ms: {}", latency_ms, err),
            details: json!({
                "endpoint_cli_override": global.endpoint,
                "endpoint": profile.endpoint,
                "region": profile.region,
                "has_explicit_endpoint": true,
                "has_region": profile.region.is_some(),
                "psm_supported": false,
                "endpoint_mode_only": true,
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

fn effective_tos_profile(global: &GlobalArgs) -> Result<Profile, CliError> {
    if global.profile.is_empty() {
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }

    let config_path = global.existing_runtime_config_path()?;
    let config_dir = ConfigFile::config_dir_from_path(&config_path);
    let config = ConfigFile::load_from(&config_path)?;
    let env_profile = Profile::from_byte_tos_env();
    let config_profile = if config.profiles.is_empty() && global.profile == "default" {
        Profile::default()
    } else {
        match config.get_effective_profile_in_dir(&global.profile, Binary::Tos, &config_dir) {
            Ok(effective) => effective.into_flat_profile(),
            Err(CliError::ConfigMissing(_)) if has_profile_values(&env_profile) => {
                Profile::default()
            }
            Err(err) => return Err(err),
        }
    };

    let mut cli_profile = Profile::default();
    cli_profile.region = global.region.clone();
    cli_profile.endpoint = global.endpoint.clone();
    cli_profile.control_endpoint = global.control_endpoint.clone();
    cli_profile.account_id = global.account_id.clone();
    Ok(merge_tos_runtime_profile(
        env_profile,
        config_profile,
        cli_profile,
    ))
}

fn has_profile_values(profile: &Profile) -> bool {
    profile.region.is_some()
        || profile.access_key_id.is_some()
        || profile.secret_access_key.is_some()
        || profile.security_token.is_some()
        || profile.endpoint.is_some()
        || profile.psm.is_some()
        || profile.idc.is_some()
        || profile.cluster.is_some()
        || profile.addr_family.is_some()
        || profile.control_endpoint.is_some()
        || profile.account_id.is_some()
        || profile.checkpoint_dir.is_some()
        || profile.batch_report_dir.is_some()
        || profile.batch_report_format.is_some()
        || profile.progress_enabled.is_some()
        || profile.max_retry_count.is_some()
        || profile.requesttimeout.is_some()
        || profile.connecttimeout.is_some()
        || profile.maxconnections.is_some()
}

fn string_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn bool_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key).and_then(Value::as_bool)
}
