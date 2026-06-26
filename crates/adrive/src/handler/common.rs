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
use serde_json::Value;
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::agent::output::{format_markdown, format_table, format_xml, OutputFormat};
use tos_core::infra::config::{Binary, ConfigFile, FieldSource, Profile};

use crate::domain::client::{Client as IdsClient, ClientOptions, Error as IdsError};

/// Build the effective runtime profile for ADrive commands.
///
/// Priority order (for every field, including credentials):
/// CLI flags > config file > environment variables > derived.
///
/// ADrive-specific rule: it never inherits shared-level (`[profile]`)
/// network settings or credentials from the config file, because those belong
/// to TOS. Only `[profile.adrive]` overrides, `ADRIVE_*` environment variables,
/// or explicit CLI flags provide ADrive runtime settings.
pub(crate) fn build_profile(global: &GlobalArgs) -> Result<Profile, CliError> {
    if global.profile.is_empty() {
        // [Review Fix #23] Runtime commands must not silently use env/default
        // ADrive credentials when the selected profile name is empty.
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }

    let config_path = global.existing_runtime_config_path()?;
    let config_dir = ConfigFile::config_dir_from_path(&config_path);
    let config = ConfigFile::load_from(&config_path)?;
    let config_profile = if config.profiles.is_empty() && global.profile == "default" {
        Profile::default()
    } else {
        let effective =
            config.get_effective_profile_in_dir(&global.profile, Binary::Adrive, &config_dir)?;
        let mut flat = effective.into_flat_profile();
        // [Review Fix #6] ADrive must not inherit shared TOS network settings.
        if effective.region.source == FieldSource::Shared {
            flat.region = None;
        }
        if effective.endpoint.source == FieldSource::Shared {
            flat.endpoint = None;
        }
        if effective.control_endpoint.source == FieldSource::Shared {
            flat.control_endpoint = None;
        }
        // ADrive must not inherit shared-level credentials (those belong to TOS).
        if effective.access_key_id.source == FieldSource::Shared {
            flat.access_key_id = None;
        }
        if effective.secret_access_key.source == FieldSource::Shared {
            flat.secret_access_key = None;
        }
        if effective.security_token.source == FieldSource::Shared {
            flat.security_token = None;
        }
        flat
    };

    let env_profile = Profile {
        region: std::env::var("ADRIVE_REGION").ok(),
        access_key_id: std::env::var("ADRIVE_ACCESS_KEY").ok(),
        secret_access_key: std::env::var("ADRIVE_SECRET_KEY").ok(),
        security_token: std::env::var("ADRIVE_SECURITY_TOKEN").ok(),
        endpoint: std::env::var("ADRIVE_ENDPOINT").ok(),
        psm: None,
        idc: None,
        cluster: None,
        addr_family: None,
        control_endpoint: None,
        account_id: std::env::var("ADRIVE_ACCOUNT_ID").ok(),
        checkpoint_dir: None,
        batch_report_dir: None,
        batch_report_format: None,
        progress_enabled: None,
        checkpoint_threshold: std::env::var("ADRIVE_CHECKPOINT_THRESHOLD").ok(),
        batch_concurrency: env_var_any(&["ADRIVE_BATCH_CONCURRENCY"])
            .and_then(|value| parse_positive_usize_env(&value)),
        list_concurrency: env_var_any(&["ADRIVE_LIST_CONCURRENCY"])
            .and_then(|value| parse_positive_usize_env(&value)),
        multipart_concurrency: env_var_any(&["ADRIVE_MULTIPART_CONCURRENCY"])
            .and_then(|value| parse_positive_usize_env(&value)),
        progress_granularity: std::env::var("ADRIVE_PROGRESS_GRANULARITY").ok(),
        overwrite_strategy: std::env::var("ADRIVE_OVERWRITE_STRATEGY").ok(),
        max_retry_count: env_var_any(&["ADRIVE_MAX_RETRY_COUNT"])
            .and_then(|value| parse_u32_env(&value)),
        requesttimeout: env_var_any(&["ADRIVE_REQUESTTIMEOUT", "ADRIVE_REQUEST_TIMEOUT"])
            .and_then(|value| parse_positive_u64_env(&value)),
        connecttimeout: env_var_any(&["ADRIVE_CONNECTTIMEOUT", "ADRIVE_CONNECT_TIMEOUT"])
            .and_then(|value| parse_positive_u64_env(&value)),
        maxconnections: env_var_any(&["ADRIVE_MAXCONNECTIONS", "ADRIVE_MAX_CONNECTIONS"])
            .and_then(|value| parse_positive_usize_env(&value)),
        tos: None,
        ve_tos: None,
        tosvector: None,
        tostable: None,
        adrive: None,
    };
    // global.region / global.endpoint / global.account_id are now pure CLI flags
    // (their TOS_* env bindings were removed), so they are safe to use here.
    let cli_profile = Profile {
        region: global.region.clone(),
        access_key_id: None,
        secret_access_key: None,
        security_token: None,
        endpoint: global.endpoint.clone(),
        psm: None,
        idc: None,
        cluster: None,
        addr_family: None,
        control_endpoint: None,
        account_id: global.account_id.clone(),
        checkpoint_dir: None,
        batch_report_dir: None,
        batch_report_format: None,
        progress_enabled: None,
        checkpoint_threshold: None,
        batch_concurrency: None,
        list_concurrency: None,
        multipart_concurrency: None,
        progress_granularity: None,
        overwrite_strategy: None,
        max_retry_count: None,
        requesttimeout: None,
        connecttimeout: None,
        maxconnections: None,
        tos: None,
        ve_tos: None,
        tosvector: None,
        tostable: None,
        adrive: None,
    };
    // Priority: CLI > Config > Env (applies uniformly to all fields).
    Ok(env_profile.merge(&config_profile).merge(&cli_profile))
}

/// Build a real IDS REST client for ADrive operations.
pub(crate) fn build_ids_client(global: &GlobalArgs) -> Result<IdsClient, CliError> {
    let profile = build_profile(global)?;
    let access_key = profile
        .access_key_id
        .ok_or_else(|| CliError::ConfigMissing("ADRIVE_ACCESS_KEY is required".to_string()))?;
    let secret_key = profile
        .secret_access_key
        .ok_or_else(|| CliError::ConfigMissing("ADRIVE_SECRET_KEY is required".to_string()))?;

    IdsClient::new(
        access_key,
        secret_key,
        profile.security_token,
        profile.endpoint,
        profile.region,
        ClientOptions {
            max_retry_count: profile.max_retry_count,
            requesttimeout: profile.requesttimeout,
            connecttimeout: profile.connecttimeout,
            maxconnections: profile.maxconnections,
        },
    )
    .map_err(|err| match err {
        IdsError::Client(message) if message.contains("ADRIVE_REGION") => {
            CliError::ConfigMissing(message)
        }
        other => CliError::Unknown(format!("failed to build IDS client: {other}")),
    })
}

pub(crate) fn map_ids_error(err: IdsError) -> CliError {
    match &err {
        IdsError::Server(server) => match server.status_code {
            Some(401) => CliError::AuthFailed(err.to_string()),
            Some(403) => CliError::PermissionDenied(err.to_string()),
            Some(404) => CliError::ResourceNotFound(err.to_string()),
            Some(409) | Some(412) => CliError::Conflict(err.to_string()),
            Some(429) | Some(503) => CliError::RateLimited(err.to_string()),
            Some(500..=599) | Some(408) => CliError::TransferFailed(err.to_string()),
            _ => CliError::Unknown(err.to_string()),
        },
        IdsError::Http(_) | IdsError::HttpBody(_) => CliError::TransferFailed(err.to_string()),
        IdsError::Json(_) | IdsError::Client(_) | IdsError::InvalidResponse(_) => {
            CliError::ValidationError(err.to_string())
        }
    }
}

fn env_var_any(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| std::env::var(key).ok())
}

fn parse_u32_env(value: &str) -> Option<u32> {
    value.trim().parse::<u32>().ok()
}

fn parse_positive_u64_env(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|parsed| *parsed > 0)
}

fn parse_positive_usize_env(value: &str) -> Option<usize> {
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|parsed| *parsed > 0)
}

/// Output a serializable result through the unified Envelope pipeline.
///
/// This is the single entry point for all ADrive command output. It:
/// 1. Auto-wraps in Envelope if not already envelope-shaped
/// 2. Injects a request_id (from env or generated ULID)
/// 3. Applies `--query` JMESPath filter if present
/// 4. Routes to the correct output format (json/yaml/xml/table/csv/markdown)
pub(crate) fn output_result<T: Serialize>(global: &GlobalArgs, data: &T) -> Result<(), CliError> {
    output_result_with_columns(global, data, None)
}

/// Output a result with declared table/csv columns through the unified pipeline.
pub(crate) fn output_result_with_columns<T: Serialize>(
    global: &GlobalArgs,
    data: &T,
    columns: Option<&'static [&'static str]>,
) -> Result<(), CliError> {
    let raw = serde_json::to_value(data)?;
    let mut enveloped = ensure_envelope(global, raw);
    publicize_adrive_output_value(&mut enveloped);
    let value = apply_query(global, enveloped)?;
    render_value(global, &value, columns)
}

/// [Review Fix #3] Kept for backward compatibility — routes through the unified pipeline.
pub(crate) fn output_envelope<T: Serialize>(
    global: &GlobalArgs,
    envelope: &Envelope<T>,
) -> Result<(), CliError> {
    output_result(global, envelope)
}

// ─── Envelope Wrapping ──────────────────────────────────────────────────────

/// Detect if the value is already Envelope-shaped, wrap if not, then inject request_id.
fn ensure_envelope(global: &GlobalArgs, value: Value) -> Value {
    let mut value = if is_envelope_shape(&value) {
        value
    } else {
        let command = describe_command(global);
        let envelope = Envelope::success(command, value);
        serde_json::to_value(envelope).unwrap_or(Value::Null)
    };
    inject_request_id(&mut value);
    value
}

fn is_envelope_shape(value: &Value) -> bool {
    matches!(value, Value::Object(map)
        if map.get("status").and_then(Value::as_str).is_some()
            && map.contains_key("command"))
}

fn describe_command(_global: &GlobalArgs) -> String {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(idx) = args.iter().position(|arg| arg.as_str() == "ve-adrive") else {
        return "ve-adrive".to_string();
    };
    let mut tokens = Vec::new();
    for arg in &args[(idx + 1)..] {
        if arg.starts_with('-') {
            break;
        }
        tokens.push(arg.as_str());
    }
    if tokens.is_empty() {
        "ve-adrive".to_string()
    } else {
        format!("ve-adrive {}", tokens.join(" "))
    }
}

/// Convert an ADrive command path into the public top-level command path
/// exposed by the unified CLI.
pub fn public_adrive_command_path(command: &str) -> String {
    // [Review Fix #24] `adrive` is no longer a public or compatibility command
    // prefix; only normalize the supported `ve-adrive` surface.
    if command == "ve-adrive" {
        return "ve-adrive".to_string();
    }
    command
        .strip_prefix("ve-adrive ")
        .map(|suffix| format!("ve-adrive {suffix}"))
        .unwrap_or_else(|| command.to_string())
}

/// Normalize ADrive user-facing JSON output to the supported public command
/// paths agents and users can actually execute.
pub fn publicize_adrive_output_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                publicize_adrive_output_field(key, child);
                publicize_adrive_output_value(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                publicize_adrive_output_value(item);
            }
        }
        Value::String(_) => {}
        _ => {}
    }
}

fn publicize_adrive_output_field(key: &str, value: &mut Value) {
    match (key, value) {
        ("tool", Value::String(tool)) if tool == "ve-adrive" => {
            *tool = "ve-adrive".to_string();
        }
        ("command", Value::String(command)) => {
            *command = public_adrive_command_path(command);
        }
        ("commands", Value::Array(commands)) => {
            for command in commands {
                if let Value::String(command) = command {
                    *command = public_adrive_command_path(command);
                }
            }
        }
        ("lines", Value::Array(lines)) => {
            for line in lines {
                if let Value::String(line) = line {
                    if line == "ve-adrive" {
                        *line = "ve-adrive".to_string();
                    } else if let Some(suffix) = line.strip_prefix("ve-adrive ") {
                        *line = format!("ve-adrive {suffix}");
                    }
                }
            }
        }
        _ => {}
    }
}

// ─── Request ID Injection ───────────────────────────────────────────────────

/// Ensure the Envelope carries a request_id.
/// Priority: existing non-empty request_id > explicit null > TOS_LAST_REQUEST_ID env > generated ULID.
fn inject_request_id(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };
    let needs_id = match map.get("request_id") {
        Some(Value::Null) => false,
        Some(value) => value.as_str().map(str::is_empty).unwrap_or(true),
        None => true,
    };
    if !needs_id {
        return;
    }
    let id = std::env::var("TOS_LAST_REQUEST_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ulid::Ulid::new().to_string());
    map.insert("request_id".to_string(), Value::String(id));
}

// ─── JMESPath Query ─────────────────────────────────────────────────────────

/// Apply a `--query` JMESPath filter expression.
/// Returns the original value unchanged when no --query is specified.
fn apply_query(global: &GlobalArgs, value: Value) -> Result<Value, CliError> {
    let Some(expr) = global.query.as_deref() else {
        return Ok(value);
    };
    let expr = expr.trim();
    if expr.is_empty() {
        return Ok(value);
    }
    let compiled = jmespath::compile(expr).map_err(|err| {
        CliError::ValidationError(format!("invalid --query expression '{}': {}", expr, err))
    })?;
    let var = jmespath::Variable::from_serializable(&value).map_err(|err| {
        CliError::ValidationError(format!(
            "failed to convert response into JMESPath input: {}",
            err
        ))
    })?;
    let result = compiled.search(var).map_err(|err| {
        CliError::ValidationError(format!("--query evaluation failed for '{}': {}", expr, err))
    })?;
    let json_str = serde_json::to_string(&*result).map_err(CliError::Json)?;
    let value: Value = serde_json::from_str(&json_str).map_err(CliError::Json)?;
    Ok(value)
}

// ─── Render Value ───────────────────────────────────────────────────────────

fn render_value(
    global: &GlobalArgs,
    value: &Value,
    columns: Option<&'static [&'static str]>,
) -> Result<(), CliError> {
    match global.output.unwrap_or_else(OutputFormat::auto_detect) {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).map_err(CliError::Json)?
            );
        }
        OutputFormat::Yaml => {
            println!(
                "{}",
                serde_yaml::to_string(value).map_err(|err| CliError::Unknown(err.to_string()))?
            );
        }
        OutputFormat::Xml => {
            println!("{}", format_xml(value).map_err(CliError::Json)?);
        }
        OutputFormat::Table => {
            let payload = unwrap_envelope_data(value);
            println!("{}", render_table(payload, columns));
            if let Some(footer) = envelope_footer(value) {
                println!("{}", footer);
            }
        }
        OutputFormat::Csv => {
            let payload = unwrap_envelope_data(value);
            println!("{}", render_csv(payload, columns));
        }
        OutputFormat::Markdown => {
            println!("{}", format_markdown(value).map_err(CliError::Json)?);
        }
    }
    Ok(())
}

// ─── Envelope Helpers ───────────────────────────────────────────────────────

fn unwrap_envelope_data(value: &Value) -> &Value {
    if is_envelope_shape(value) {
        match value {
            Value::Object(map) => map.get("data").unwrap_or(value),
            _ => value,
        }
    } else {
        value
    }
}

/// When the Envelope carries a `pagination` field, produce a footer like `Total: N`.
fn envelope_footer(value: &Value) -> Option<String> {
    if !is_envelope_shape(value) {
        return None;
    }
    let map = value.as_object()?;
    let pagination = map.get("pagination")?.as_object()?;
    let total = pagination.get("total_returned").and_then(Value::as_u64);
    let next_token = pagination
        .get("next_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let next_marker = pagination
        .get("next_marker")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let next_label = next_marker
        .map(|marker| ("next_marker", marker))
        .or_else(|| next_token.map(|token| ("next_token", token)));
    match (total, next_label) {
        (Some(n), Some((label, value))) => Some(format!("Total: {} ({}={})", n, label, value)),
        (Some(n), None) => Some(format!("Total: {}", n)),
        (None, Some((label, value))) => Some(format!("{}={}", label, value)),
        (None, None) => None,
    }
}

// ─── Table / CSV Rendering ──────────────────────────────────────────────────

fn render_table(value: &Value, columns: Option<&'static [&'static str]>) -> String {
    let array = pick_array_payload(value, columns);
    match (array, value) {
        (Some(items), _) if items.iter().all(|item| item.is_object()) => {
            let headers = resolve_headers(items, columns);
            let rows = items
                .iter()
                .map(|item| {
                    let map = item.as_object().expect("checked object");
                    headers
                        .iter()
                        .map(|key| cell_value(map.get(key).unwrap_or(&Value::Null)))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let header_refs = headers.iter().map(String::as_str).collect::<Vec<_>>();
            format_table(&header_refs, &rows)
        }
        (Some(items), _) => {
            let rows = items
                .iter()
                .enumerate()
                .map(|(idx, item)| vec![idx.to_string(), cell_value(item)])
                .collect::<Vec<_>>();
            format_table(&["index", "value"], &rows)
        }
        (None, Value::Object(_)) => {
            // [Review Fix #DescribeTable] Match TOS object-detail rendering so
            // nested describe metadata is visible as field/value rows.
            let rows = flatten_object_to_rows("", value);
            format_table(&["field", "value"], &rows)
        }
        (None, _) => format_table(&["value"], &[vec![cell_value(value)]]),
    }
}

fn render_csv(value: &Value, columns: Option<&'static [&'static str]>) -> String {
    let array = pick_array_payload(value, columns);
    match (array, value) {
        (Some(items), _) if items.iter().all(|item| item.is_object()) => {
            let headers = resolve_headers(items, columns);
            let mut lines = vec![headers.join(",")];
            for item in items {
                if let Value::Object(map) = item {
                    lines.push(
                        headers
                            .iter()
                            .map(|key| {
                                csv_escape(&cell_value(map.get(key).unwrap_or(&Value::Null)))
                            })
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                }
            }
            lines.join("\n")
        }
        (Some(items), _) => {
            let mut lines = vec!["index,value".to_string()];
            for (idx, item) in items.iter().enumerate() {
                lines.push(format!("{},{}", idx, csv_escape(&cell_value(item))));
            }
            lines.join("\n")
        }
        (None, Value::Object(_)) => {
            // [Review Fix #DescribeTable] Keep CSV object details aligned with
            // table output instead of returning one opaque JSON blob.
            let rows = flatten_object_to_rows("", value);
            let mut lines = vec!["field,value".to_string()];
            for row in rows {
                lines.push(format!(
                    "{},{}",
                    csv_escape(row.first().map(String::as_str).unwrap_or_default()),
                    csv_escape(row.get(1).map(String::as_str).unwrap_or_default())
                ));
            }
            lines.join("\n")
        }
        (None, _) => format!("value\n{}", csv_escape(&cell_value(value))),
    }
}

fn pick_array_payload<'a>(
    value: &'a Value,
    columns: Option<&'static [&'static str]>,
) -> Option<&'a Vec<Value>> {
    if let Value::Array(items) = value {
        return Some(items);
    }
    let Value::Object(map) = value else {
        return None;
    };
    if let Some(columns) = columns {
        let mut empty_candidate = None;
        for candidate in map.values().filter_map(Value::as_array) {
            if candidate.iter().any(|item| {
                item.as_object()
                    .map(|obj| columns.iter().any(|column| obj.contains_key(*column)))
                    .unwrap_or(false)
            }) {
                return Some(candidate);
            }
            if candidate.is_empty() && empty_candidate.is_none() {
                empty_candidate = Some(candidate);
            }
        }
        if empty_candidate.is_some() {
            return empty_candidate;
        }
    }
    let mut found = None;
    for candidate in map.values().filter_map(Value::as_array) {
        if found.is_some() {
            return None;
        }
        found = Some(candidate);
    }
    found
}

fn resolve_headers(items: &[Value], columns: Option<&'static [&'static str]>) -> Vec<String> {
    if let Some(columns) = columns {
        return columns.iter().map(|value| value.to_string()).collect();
    }
    let mut headers = Vec::new();
    for item in items {
        if let Some(map) = item.as_object() {
            for key in map.keys() {
                if !headers.contains(key) {
                    headers.push(key.clone());
                }
            }
        }
    }
    headers
}

fn cell_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn flatten_object_to_rows(prefix: &str, value: &Value) -> Vec<Vec<String>> {
    match value {
        Value::Object(map) => {
            let mut rows = Vec::new();
            for (key, val) in map {
                let full_key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                match val {
                    Value::Object(_) => {
                        rows.extend(flatten_object_to_rows(&full_key, val));
                    }
                    Value::Array(items)
                        if items.iter().all(|item| {
                            item.is_string() || item.is_number() || item.is_boolean()
                        }) =>
                    {
                        let joined = items.iter().map(cell_value).collect::<Vec<_>>().join(", ");
                        rows.push(vec![full_key, joined]);
                    }
                    _ => {
                        rows.push(vec![full_key, cell_value(val)]);
                    }
                }
            }
            rows
        }
        _ => vec![vec![prefix.to_string(), cell_value(value)]],
    }
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn table_output_uses_declared_columns_for_empty_payloads() {
        let rendered = render_table(
            &json!({
                "files": [],
                "folders": [],
                "next_marker": "",
                "is_truncated": false,
            }),
            Some(&["file_path", "size", "file_type"]),
        );

        assert!(rendered.contains("file_path"));
        assert!(rendered.contains("file_type"));
        assert!(!rendered.contains("field"));
    }

    #[test]
    fn csv_output_uses_declared_columns_for_empty_payloads() {
        let rendered = render_csv(
            &json!({
                "instances": [],
                "next_marker": "",
                "is_truncated": false,
            }),
            Some(&["instance_id", "name"]),
        );

        assert_eq!(rendered, "instance_id,name");
    }

    #[test]
    fn envelope_footer_prefers_next_marker_for_adrive_pagination() {
        let footer = envelope_footer(&json!({
            "success": true,
            "status": "success",
            "command": "ve-adrive ls",
            "request_id": "req",
            "status_code": null,
            "ec": null,
            "data": {"files": []},
            "pagination": {
                "next_marker": "marker-1",
                "total_returned": 2
            }
        }));

        assert_eq!(footer, Some("Total: 2 (next_marker=marker-1)".to_string()));
    }

    #[test]
    fn publicizer_does_not_translate_legacy_adrive_command_paths() {
        assert_eq!(public_adrive_command_path("ve-adrive ls"), "ve-adrive ls");
        assert_eq!(public_adrive_command_path("adrive ls"), "adrive ls");

        let mut output = json!({
            "tool": "adrive",
            "command": "adrive ls",
            "commands": ["adrive cp", "ve-adrive rm"],
            "lines": ["adrive", "adrive ls", "ve-adrive ls"],
            "message": "run adrive ls for old command data"
        });

        publicize_adrive_output_value(&mut output);

        assert_eq!(output["tool"], "adrive");
        assert_eq!(output["command"], "adrive ls");
        assert_eq!(output["commands"], json!(["adrive cp", "ve-adrive rm"]));
        assert_eq!(
            output["lines"],
            json!(["adrive", "adrive ls", "ve-adrive ls"])
        );
        assert_eq!(output["message"], "run adrive ls for old command data");
    }
}

/// Parse an adrive:// URI into its components.
///
/// Format: `adrive://instance/space/folder_path.../file`
///
/// Returns `(instance, space, path_remainder)` where `path_remainder` is
/// everything after `space/` (could be a folder path, file path, or empty).
#[derive(Debug, Clone)]
pub(crate) struct ParsedADriveUri {
    pub instance: String,
    pub space: String,
    /// The remaining path after instance/space (folder/file or just folder/).
    /// Empty string if only instance/space are present.
    pub path: String,
}

impl ParsedADriveUri {
    /// Extract the file name (last segment, only if not ending with '/').
    pub fn file(&self) -> Option<&str> {
        if self.path.is_empty() || self.path.ends_with('/') {
            return None;
        }
        self.path.rsplit('/').next()
    }
}

/// Parse an `adrive://instance/space[/path...]` URI.
///
/// If `require_space` is true, the URI must contain at least instance and space.
/// If `allow_instance_only` is true, `adrive://instance` is valid.
pub(crate) fn parse_adrive_uri(
    uri: &str,
    allow_instance_only: bool,
) -> Result<ParsedADriveUri, CliError> {
    if !uri.starts_with("adrive://") {
        return Err(CliError::ValidationError(format!(
            "invalid A-Drive URI '{}': expected adrive://instance/space[/path]",
            uri
        )));
    }
    let rest = uri.trim_start_matches("adrive://");
    let parts: Vec<&str> = rest.splitn(3, '/').collect();

    let instance = parts.first().filter(|s| !s.is_empty()).ok_or_else(|| {
        CliError::ValidationError(format!("invalid A-Drive URI '{}': missing instance", uri))
    })?;

    if parts.len() < 2 || parts[1].is_empty() {
        if allow_instance_only {
            return Ok(ParsedADriveUri {
                instance: instance.to_string(),
                space: String::new(),
                path: String::new(),
            });
        }
        return Err(CliError::ValidationError(format!(
            "invalid A-Drive URI '{}': expected adrive://instance/space[/path]",
            uri
        )));
    }

    let space = parts[1];
    let path = if parts.len() > 2 { parts[2] } else { "" };

    Ok(ParsedADriveUri {
        instance: instance.to_string(),
        space: space.to_string(),
        path: path.to_string(),
    })
}

/// Resolve target from either positional URI or explicit flags.
pub(crate) fn resolve_target(
    uri: Option<&str>,
    instance: Option<&str>,
    space: Option<&str>,
    folder: Option<&str>,
    file: Option<&str>,
) -> Result<ParsedADriveUri, CliError> {
    if let Some(uri) = uri {
        return parse_adrive_uri(uri, false);
    }

    let instance = instance.ok_or_else(|| {
        CliError::ValidationError(
            "missing target: provide adrive://instance/space/path or --instance".into(),
        )
    })?;
    let space = space.ok_or_else(|| {
        CliError::ValidationError("missing --space: required with --instance".into())
    })?;

    let path = match (folder, file) {
        (Some(f), Some(name)) => {
            let f = f.trim_end_matches('/');
            format!("{f}/{name}")
        }
        (Some(f), None) => {
            let f = f.trim_end_matches('/');
            format!("{f}/")
        }
        (None, Some(name)) => name.to_string(),
        (None, None) => String::new(),
    };

    Ok(ParsedADriveUri {
        instance: instance.to_string(),
        space: space.to_string(),
        path,
    })
}

/// Block destructive commands unless the caller explicitly confirms.
pub(crate) fn ensure_force_for_destructive(
    global: &GlobalArgs,
    force: bool,
    command: &str,
    target: &str,
) -> Result<(), CliError> {
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let stderr_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let can_prompt = stdin_tty && stderr_tty && !global.quiet;

    if force {
        if requires_delete_confirm(command) && !can_prompt {
            return ensure_exact_confirm(global, command, target);
        }
        return Ok(());
    }

    if global.yes && can_prompt {
        return Ok(());
    }

    if can_prompt {
        eprint!(
            "⚠ destructive command '{}' targeting '{}'\n  Type 'yes' to proceed: ",
            command, target
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).map_err(|e| {
            CliError::ValidationError(format!("failed to read confirmation input: {}", e))
        })?;
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("yes") || trimmed.eq_ignore_ascii_case("y") {
            return Ok(());
        }
        return Err(CliError::ValidationError(format!(
            "operation cancelled by user (received '{}')",
            trimmed
        )));
    }

    if requires_delete_confirm(command) {
        return Err(CliError::ValidationError(format!(
            "critical delete command '{}' for '{}' requires --force and --confirm {} in non-interactive execution",
            command, target, target
        )));
    }

    Err(CliError::ValidationError(format!(
        "destructive command '{}' for '{}' requires --force (or --yes in interactive shell)",
        command, target
    )))
}

fn requires_delete_confirm(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.contains(" del")
        || normalized.contains(" mv")
        || normalized.contains(" rm")
        || normalized.contains(" delete")
        || normalized.contains(" --delete")
}

fn ensure_exact_confirm(
    global: &GlobalArgs,
    command: &str,
    expected: &str,
) -> Result<(), CliError> {
    // [Review Fix #6] ADrive delete-class commands are critical in
    // non-interactive execution and must echo the public adrive:// target.
    match global.confirm.as_deref() {
        Some(provided) if provided == expected => Ok(()),
        Some(provided) => Err(CliError::ValidationError(format!(
            "--confirm '{}' does not match the critical resource '{}' for {}",
            provided, expected, command
        ))),
        None => Err(CliError::ValidationError(format!(
            "critical delete command '{}' requires --confirm {} in non-interactive execution",
            command, expected
        ))),
    }
}
