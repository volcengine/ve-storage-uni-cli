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

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::agent::output::{format_markdown, format_table, format_xml, OutputFormat};
use tos_core::infra::config::{merge_tos_runtime_profile, Binary, ConfigFile, Profile};

const TOS_CONFIG_BINARY_ENV: &str = "VE_STORAGE_UNI_TOS_CONFIG_BINARY";

/// Build the effective runtime profile for TOS commands.
///
/// Priority order (for every field, including credentials):
/// CLI flags > config file > environment variables > derived values.
///
/// Note: `global.region` / `global.endpoint` / `global.control_endpoint` /
/// `global.account_id` carry ONLY explicit CLI flags (their clap `env`
/// bindings were removed). Environment variables are sourced exclusively
/// through `Profile::from_env()` so they sit at the lowest precedence and
/// never get silently promoted above the config file.
pub(crate) fn build_profile(global: &GlobalArgs) -> Result<Profile, CliError> {
    if global.profile.is_empty() {
        // [Review Fix #22] Runtime commands must not silently fall back to env/default
        // credentials when the selected profile name is empty.
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }

    let config_path = global.existing_runtime_config_path()?;
    let config_dir = ConfigFile::config_dir_from_path(&config_path);
    let config = ConfigFile::load_from(&config_path)?;
    let active_binary = active_tos_config_binary();
    let env_profile = match active_binary {
        Binary::Tos => Profile::from_byte_tos_env(),
        _ => Profile::from_env(),
    };
    let config_profile = if config.profiles.is_empty() && global.profile == "default" {
        Profile::default()
    } else {
        match config.get_effective_profile_in_dir(&global.profile, active_binary, &config_dir) {
            Ok(effective) => effective.into_flat_profile(),
            Err(CliError::ConfigMissing(_)) if has_tos_env_profile_values(&env_profile) => {
                // [Review Fix #10] Keep runtime env-only profiles working for
                // the active surface: `ve-tos` consumes TOS_* while the new
                // ByteCloud `tos` consumes BYTE_TOS_*. Config-file namespaces
                // remain isolated; env fallback is deliberately per-surface.
                Profile::default()
            }
            Err(err) => return Err(err),
        }
    };

    let cli_profile = Profile {
        region: global.region.clone(),
        access_key_id: None,
        secret_access_key: None,
        security_token: None,
        endpoint: global.endpoint.clone(),
        psm: global.psm.clone(),
        idc: global.idc.clone(),
        cluster: global.cluster.clone(),
        addr_family: global.addr_family.clone(),
        control_endpoint: global.control_endpoint.clone(),
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
    // Priority: CLI > Config > Env. ByteCloud TOS also treats endpoint and
    // PSM as exclusive connection modes so lower-priority endpoint env cannot
    // suppress configured PSM discovery.
    let effective_profile = if active_binary == Binary::Tos {
        merge_tos_runtime_profile(env_profile, config_profile, cli_profile)
    } else {
        env_profile.merge(&config_profile).merge(&cli_profile)
    };
    validate_tos_psm_cli_modifiers(global, &effective_profile)?;
    Ok(effective_profile)
}

fn validate_tos_psm_cli_modifiers(global: &GlobalArgs, profile: &Profile) -> Result<(), CliError> {
    if active_tos_config_binary() != Binary::Tos {
        return Ok(());
    }
    let has_cli_modifier =
        global.idc.is_some() || global.cluster.is_some() || global.addr_family.is_some();
    // [Review Fix #2] Keep CLI validation aligned with resolver construction:
    // blank PSM is not a usable PSM value.
    let has_psm = profile
        .psm
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if has_cli_modifier && !has_psm {
        return Err(CliError::ValidationError(
            "--idc, --cluster, and --addr-family require --psm or a configured BYTE_TOS_PSM/[profile.tos].psm".to_string(),
        ));
    }
    Ok(())
}

fn has_tos_env_profile_values(profile: &Profile) -> bool {
    profile.region.is_some()
        || profile.access_key_id.is_some()
        || profile.secret_access_key.is_some()
        || profile.security_token.is_some()
        || profile.endpoint.is_some()
        || profile.psm.is_some()
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

pub(crate) fn active_tos_config_binary() -> Binary {
    std::env::var(TOS_CONFIG_BINARY_ENV)
        .ok()
        .as_deref()
        .and_then(Binary::parse)
        .unwrap_or(Binary::Tos)
}

/// Parse a bucket name from either `bucket` or `tos://bucket/...`.
pub(crate) fn parse_bucket_name(input: &str) -> String {
    if input.starts_with("tos://") {
        input
            .trim_start_matches("tos://")
            .split('/')
            .next()
            .unwrap_or(input)
            .to_string()
    } else {
        input.to_string()
    }
}

pub(crate) fn validate_bucket_flag_target(input: &str) -> Result<String, CliError> {
    let value = input.trim();
    if value.is_empty() {
        return Err(CliError::ValidationError(
            "--bucket expects a non-empty bucket name".to_string(),
        ));
    }
    if value.starts_with("tos://") || value.contains('/') {
        return Err(CliError::ValidationError(format!(
            "invalid --bucket '{}': expected a bucket name only; use positional tos://bucket/key for URI style",
            input
        )));
    }
    Ok(value.to_string())
}

/// Parse a `(bucket, key)` tuple from either `tos://bucket/key` or explicit flags.
pub(crate) fn parse_object_target(
    uri: Option<&str>,
    bucket: Option<&str>,
    key: Option<&str>,
) -> Result<(String, String), CliError> {
    if let Some(uri) = uri {
        if bucket.is_some() || key.is_some() {
            return Err(CliError::ValidationError(
                "positional tos://bucket/key cannot be combined with --bucket or --key".to_string(),
            ));
        }
        if !uri.starts_with("tos://") {
            return Err(CliError::ValidationError(format!(
                "invalid object uri '{}': expected tos://bucket/key",
                uri
            )));
        }
        let rest = uri.trim_start_matches("tos://");
        let mut parts = rest.splitn(2, '/');
        let bucket_name = parts.next().unwrap_or_default();
        let object_key = parts.next().unwrap_or_default();
        if bucket_name.is_empty() || object_key.is_empty() {
            return Err(CliError::ValidationError(format!(
                "invalid object uri '{}': expected tos://bucket/key",
                uri
            )));
        }
        return Ok((bucket_name.to_string(), object_key.to_string()));
    }

    let bucket_name = bucket.ok_or_else(|| {
        CliError::ValidationError(
            "missing object target: provide tos://bucket/key or --bucket".into(),
        )
    })?;
    let bucket_name = validate_bucket_flag_target(bucket_name)?;
    let object_key = key.ok_or_else(|| {
        CliError::ValidationError("missing object key: provide tos://bucket/key or --key".into())
    })?;
    if object_key.trim().is_empty() {
        return Err(CliError::ValidationError(
            "missing object key: provide tos://bucket/key or --key".into(),
        ));
    }
    Ok((bucket_name, object_key.to_string()))
}

/// Load request bytes from stdin, a file path, `file://path`, or inline text.
pub(crate) fn read_body_input(source: &str) -> Result<Vec<u8>, CliError> {
    if source == "-" {
        let mut buffer = Vec::new();
        std::io::stdin().read_to_end(&mut buffer)?;
        return Ok(buffer);
    }

    let candidate = source.strip_prefix("file://").unwrap_or(source);
    if Path::new(candidate).exists() {
        return Ok(fs::read(candidate)?);
    }

    Ok(source.as_bytes().to_vec())
}

/// [Review Fix #M4] Body input classification for upload-style handlers.
///
/// `FilePath` indicates the `--body` argument refers to an existing local
/// file on disk; the caller should stream the file via
/// `execute_object_streaming_request` instead of buffering the whole
/// payload into memory (Streaming I/O hard constraint).
///
/// `Inline` indicates the payload is small / already in memory (stdin
/// piped data or literal bytes); the caller can keep using
/// `execute_object_request` with a `Vec<u8>` body.
pub(crate) enum BodyInput {
    FilePath { path: PathBuf, len: u64 },
    Inline(Vec<u8>),
}

/// [Review Fix #M4] Classify a `--body` argument into `FilePath` (stream
/// directly from disk) or `Inline` (read into memory). Mirrors the
/// resolution rules of `read_body_input` but exposes the file path so
/// callers can compute payload SHA256/CRC64 incrementally and hand the
/// open file to `Body::wrap_stream`.
pub(crate) fn classify_body_input(source: &str) -> Result<BodyInput, CliError> {
    if source == "-" {
        let mut buffer = Vec::new();
        std::io::stdin().read_to_end(&mut buffer)?;
        return Ok(BodyInput::Inline(buffer));
    }

    let candidate = source.strip_prefix("file://").unwrap_or(source);
    let path = Path::new(candidate);
    if path.exists() && path.is_file() {
        let len = fs::metadata(path)?.len();
        return Ok(BodyInput::FilePath {
            path: path.to_path_buf(),
            len,
        });
    }

    Ok(BodyInput::Inline(source.as_bytes().to_vec()))
}

/// Load JSON from a string, `file://path`, or a local file path.
pub(crate) fn read_json_input(source: &str) -> Result<Value, CliError> {
    let candidate = source.strip_prefix("file://").unwrap_or(source);
    let raw = if Path::new(candidate).exists() {
        fs::read_to_string(candidate)?
    } else {
        source.to_string()
    };
    serde_json::from_str(&raw).map_err(|err| {
        CliError::ValidationError(format!("invalid JSON input '{}': {}", source, err))
    })
}

/// Parse a `k=v&k2=v2` string into a JSON object.
pub(crate) fn parse_kv_pairs(input: &str) -> Value {
    let mut map = Map::new();
    for pair in input.split('&').filter(|item| !item.is_empty()) {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or_default().trim();
        let value = parts.next().unwrap_or_default().trim();
        if !key.is_empty() {
            map.insert(key.to_string(), Value::String(value.to_string()));
        }
    }
    Value::Object(map)
}

/// Block destructive commands unless the caller explicitly confirms.
///
/// Confirmation gates (in priority order):
/// 1. Interactive TTY with `--force` → immediate pass
/// 2. Interactive TTY without `--force` → prompt user to type "yes"
/// 3. Non-interactive delete-class commands → require `--force` plus exact `--confirm`
/// 4. Other non-interactive destructive commands → require `--force`
pub(crate) fn ensure_force_for_destructive(
    global: &GlobalArgs,
    force: bool,
    command: &str,
    target: &str,
) -> Result<(), CliError> {
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let stderr_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let can_prompt = stdin_tty && stderr_tty && !global.quiet;
    let confirm_target = critical_confirm_target(command, target);

    if force {
        if requires_delete_confirm(command) && !can_prompt {
            return ensure_exact_confirm(global, command, &confirm_target);
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
            command, target, confirm_target
        )));
    }

    Err(CliError::ValidationError(format!(
        "destructive command '{}' for '{}' requires --force (or --yes in interactive shell)",
        command, target
    )))
}

fn requires_delete_confirm(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.contains(" delete")
        || normalized.contains(" batch-delete")
        || normalized.ends_with(" mv")
        || normalized.ends_with(" rm")
        || normalized.ends_with(" rb")
        || normalized.contains(" --delete")
}

fn critical_confirm_target(command: &str, target: &str) -> String {
    if target.contains("://") {
        return target.to_string();
    }
    if command.starts_with("ve-tos ") && !target.contains(" -> ") {
        return format!("tos://{}", target.trim_start_matches('/'));
    }
    target.to_string()
}

fn ensure_exact_confirm(
    global: &GlobalArgs,
    command: &str,
    expected: &str,
) -> Result<(), CliError> {
    // [Review Fix #6] Delete-class commands are critical in non-interactive
    // execution: --force confirms intent, while --confirm must name the exact
    // resource path using the public URI form.
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

/// Render low-level output according to `--output`.
///
/// [Review Fix #1] 成功路径统一 Envelope：
/// - 若 `result` 序列化后已是 `{status, command, ...}` 形态（即调用方显式构造了 Envelope），
///   则原样输出，避免双重包装。
/// - 否则按 `Envelope<Value>::success` 自动包装，确保 Agent 拿到一致 schema。
///
/// [Review Fix #2] 同时在序列化后应用 `--query` JMESPath 过滤。
pub fn output_result<T: Serialize>(global: &GlobalArgs, result: &T) -> Result<(), CliError> {
    output_result_with_columns(global, result, None)
}

/// [Review Fix #FmtUni] 声明式列入口：list-like 命令可在不破坏 Envelope/JSON 路径的
/// 前提下，为 table / csv 视图声明优先列。`columns` 仅作用于这两种视图，其它格式
/// 完全无视。`--query` 优先级高于声明列；当 `--query` 显式提取了非数组 / 非对象数组
/// 时，自动回落到通用反射式渲染。
pub fn output_result_with_columns<T: Serialize>(
    global: &GlobalArgs,
    result: &T,
    columns: Option<&'static [&'static str]>,
) -> Result<(), CliError> {
    let raw = serde_json::to_value(result)?;
    let enveloped = ensure_envelope(global, raw);
    let value = apply_query(global, enveloped)?;
    render_value(global, &value, columns)
}

/// [Review Fix #1] 检测是否已是 Envelope，不是则自动包装。
fn ensure_envelope(global: &GlobalArgs, value: Value) -> Value {
    let was_envelope = is_envelope_shape(&value);
    let mut value = if was_envelope {
        value
    } else {
        let command = describe_command(global);
        let envelope = Envelope::success(command, value);
        serde_json::to_value(envelope).unwrap_or(Value::Null)
    };
    // [Review Fix #1] Raw values pass through Envelope::success first, which
    // generates a fallback ULID; prefer the upstream request id when present.
    if !was_envelope {
        if let Some(id) = last_request_id_from_env() {
            if let Value::Object(map) = &mut value {
                map.insert("request_id".to_string(), Value::String(id));
            }
        }
    }
    // [G8] Auto-inject a request_id if the caller did not set one. Priority:
    //   1. Existing request_id on the Envelope (set by handlers that already
    //      surfaced an upstream X-Tos-Request-Id header).
    //   1.5 Explicit null request_id, used by aggregate commands with no
    //       single upstream request ID.
    //   2. The TOS_LAST_REQUEST_ID env var (set by infra::client when an
    //      HTTP response carried X-Tos-Request-Id) — lets us correlate even
    //      for handlers that haven't been refactored.
    //   3. Generated ULID, so every Agent invocation has a stable handle.
    inject_request_id(&mut value);
    normalize_envelope_command(&mut value);
    value
}

fn normalize_envelope_command(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };
    let Some(command) = map.get("command").and_then(Value::as_str) else {
        return;
    };
    let public_command = public_envelope_command_for_binary(active_tos_config_binary(), command);
    if public_command != command {
        map.insert("command".to_string(), Value::String(public_command));
    }
}

fn public_envelope_command_for_binary(binary: Binary, command: &str) -> String {
    // [Review Fix #8] The shared ve-tos handlers are also used by the public
    // `tos` surface, so normalize at the output boundary for every Envelope.
    if binary == Binary::Tos {
        command
            .strip_prefix("ve-tos ")
            .map(|suffix| format!("tos {suffix}"))
            .unwrap_or_else(|| command.to_string())
    } else {
        command.to_string()
    }
}

/// [G8] Ensure the Envelope carries a request_id. Called from ensure_envelope.
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
    let id = last_request_id_from_env().unwrap_or_else(|| ulid::Ulid::new().to_string());
    map.insert("request_id".to_string(), Value::String(id));
}

fn last_request_id_from_env() -> Option<String> {
    std::env::var("TOS_LAST_REQUEST_ID")
        .ok()
        .filter(|s| !s.is_empty())
}

fn is_envelope_shape(value: &Value) -> bool {
    matches!(value, Value::Object(map)
        if map.get("status").and_then(Value::as_str).is_some()
            && map.contains_key("command"))
}

fn describe_command(_global: &GlobalArgs) -> String {
    let args = std::env::args().collect::<Vec<_>>();
    let binary_stem = args
        .first()
        .and_then(|arg| std::path::Path::new(arg).file_stem())
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let start = if matches!(binary_stem, "ve-tos" | "ve-tos-cli") {
        1
    } else {
        // [Review Fix #27] `tos` is now a separate public command, so ve-tos
        // command recovery only recognizes the ve-tos surface.
        let Some(tos_idx) = args.iter().position(|arg| arg.as_str() == "ve-tos") else {
            return "ve-tos".to_string();
        };
        tos_idx + 1
    };
    let mut tokens = Vec::new();
    for arg in &args[start..] {
        if arg.starts_with('-') {
            break;
        }
        tokens.push(arg.as_str());
    }
    if tokens.is_empty() {
        return "ve-tos".to_string();
    }
    for len in (1..=tokens.len()).rev() {
        let candidate = format!("ve-tos {}", tokens[..len].join(" "));
        if crate::registry::capability_row_for_command(&candidate, false).is_some()
            || crate::registry::find_command_tree_entry(&candidate).is_some()
            || tokens
                .first()
                .and_then(|group| crate::registry::find_group(group))
                .map(|group| group.command == candidate)
                .unwrap_or(false)
        {
            return candidate;
        }
    }
    format!("ve-tos {}", tokens.join(" "))
}

/// [G4] Apply a `--query` JMESPath filter using the full `jmespath` crate.
///
/// Supersedes the previous handwritten subset (path/index/wildcard only).
/// Now supports the entire JMESPath grammar: pipes, multi-select, slicing,
/// comparison filters (`[?Size > \`10\`]`), built-in functions (`length`,
/// `keys`, `sort_by`, ...), and projections.
///
/// On evaluation failure we surface a `ValidationError` (exit 6) so the
/// Agent gets a deterministic error code. An expression that legitimately
/// matches nothing returns `Value::Null` rather than failing — this matches
/// upstream `aws --query` semantics and avoids breaking pipelines that
/// optionally select fields.
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

fn render_value(
    global: &GlobalArgs,
    value: &Value,
    columns: Option<&'static [&'static str]>,
) -> Result<(), CliError> {
    // [Review Fix #5] Default output stays context-aware: interactive TTY gets
    // table for humans, while non-TTY/pipe execution gets JSON for Agents.
    match global.output.unwrap_or_else(OutputFormat::auto_detect) {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).map_err(CliError::Json)?
            );
        }
        OutputFormat::Yaml => {
            println!(
                "{}",
                serde_yaml::to_string(&value).map_err(|err| CliError::Unknown(err.to_string()))?
            );
        }
        OutputFormat::Xml => {
            println!("{}", format_xml(&value).map_err(CliError::Json)?);
        }
        OutputFormat::Table => {
            // [Review Fix #FmtUni] --verbose 时把 Envelope 头部摘要打在表格之上，
            // 让人和 Agent 都能看到 status / command / request_id；payload 只渲染 data。
            if global.verbose {
                if let Some(meta) = format_envelope_meta_line(value) {
                    println!("{}", meta);
                }
            }
            let payload = unwrap_envelope_data(value);
            println!("{}", render_table(payload, columns));
            // [Review Fix #FmtUni] footer 标准化：当 Envelope 携带 pagination 时
            // 自动追加 `Total: N`（含 next_token 时附带），任何 list-like 命令免维护。
            if let Some(footer) = envelope_footer(value) {
                println!("{}", footer);
            }
        }
        OutputFormat::Csv => {
            let payload = unwrap_envelope_data(value);
            println!("{}", render_csv(payload, columns));
        }
        // [G9] Markdown view keeps the full Envelope so the heading carries
        // command + status + request_id alongside the payload.
        OutputFormat::Markdown => {
            println!("{}", format_markdown(&value).map_err(CliError::Json)?);
        }
    }
    Ok(())
}

/// [Review Fix #FmtUni] 在 --verbose 下把 Envelope 头部信息渲染为单行摘要，
/// 让 table 视图也能看到 status / command / request_id，而不需要切换到 JSON。
fn format_envelope_meta_line(value: &Value) -> Option<String> {
    if !is_envelope_shape(value) {
        return None;
    }
    let map = value.as_object()?;
    let status = map.get("status").and_then(Value::as_str).unwrap_or("");
    let command = map.get("command").and_then(Value::as_str).unwrap_or("");
    let request_id = map.get("request_id").and_then(Value::as_str).unwrap_or("");
    Some(format!(
        "[{}] {} (request_id={})",
        status, command, request_id
    ))
}

/// [Review Fix #FmtUni] 当 Envelope 携带 pagination 时输出 `Total: N` 行
/// （含 next_token 时附带），让 list-like 命令免维护底栏。
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
    match (total, next_token) {
        (Some(n), Some(t)) => Some(format!("Total: {} (next_token={})", n, t)),
        (Some(n), None) => Some(format!("Total: {}", n)),
        (None, Some(t)) => Some(format!("next_token={}", t)),
        (None, None) => None,
    }
}

/// Returns the inner `data` payload when `value` matches the Envelope shape; otherwise returns the
/// original value untouched. Used to keep table/csv views focused on business fields.
///
/// [Review Fix #FmtUni-C2] Low-level handlers wrap responses in `RawResponseData`
/// (`{status_code, headers, body_format, body}`); for table/csv we further unwrap
/// `body` so list-like commands like `ve-tos object list` render the actual XML/JSON
/// payload (Buckets/Contents/...) rather than collapsing into a 4-row meta table.
/// JSON/YAML/XML/Markdown remain untouched — they keep the full Envelope schema.
fn unwrap_envelope_data(value: &Value) -> &Value {
    let data = if is_envelope_shape(value) {
        match value {
            Value::Object(map) => map.get("data").unwrap_or(value),
            _ => value,
        }
    } else {
        value
    };
    if is_raw_response_shape(data) {
        if let Value::Object(map) = data {
            if let Some(body) = map.get("body") {
                if !body.is_null() {
                    return body;
                }
            }
        }
    }
    data
}

/// [Review Fix #FmtUni-C2] Detect the `RawResponseData` carrier produced by
/// `domain::core::execute_*_request`. Used by `unwrap_envelope_data` to drill
/// down to `body` for table/csv views without affecting JSON/YAML/XML/Markdown.
fn is_raw_response_shape(value: &Value) -> bool {
    matches!(value, Value::Object(map)
        if map.contains_key("status_code")
            && map.contains_key("headers")
            && map.contains_key("body"))
}

fn render_table(value: &Value, columns: Option<&'static [&'static str]>) -> String {
    // [Review Fix #FmtUni] payload 是裸数据；list-like 命令的 data 通常是对象，
    // 内部含一个数组字段（buckets / objects / contents），需要先尝试抽出来。
    // [Review Fix #FmtUni-Phase2] 当声明了 columns 时，多数组 payload 优先匹配声明列
    // （e.g. ListObjectsResponse 同时含 contents/common_prefixes，按 OBJECT_LIST_TABLE_COLUMNS
    //  选 contents 而不是退化到 field/value 视图）。
    let array_view = pick_array_payload_for_columns(value, columns);
    match (array_view, value) {
        (Some(items), _) if items.iter().all(|item| matches!(item, Value::Object(_))) => {
            let headers = resolve_object_headers(items, columns);
            let header_refs: Vec<&str> = headers.iter().map(String::as_str).collect();
            let rows = items
                .iter()
                .map(|item| match item {
                    Value::Object(map) => headers
                        .iter()
                        .map(|key| cell_value(map.get(key).unwrap_or(&Value::Null)))
                        .collect(),
                    _ => vec![],
                })
                .collect::<Vec<Vec<String>>>();
            format_table(&header_refs, &rows)
        }
        (Some(items), _) => {
            // [Review Fix #FmtUni] 标量数组：列头与 JSON 字段一致使用 snake_case。
            let rows = items
                .iter()
                .enumerate()
                .map(|(idx, item)| vec![idx.to_string(), cell_value(item)])
                .collect::<Vec<Vec<String>>>();
            format_table(&["index", "value"], &rows)
        }
        (None, Value::Object(_)) => {
            // [Review Fix #FlattenDetail] 单对象详情递归展平嵌套对象为 dot-notation。
            let rows = flatten_object_to_rows("", value);
            format_table(&["field", "value"], &rows)
        }
        (None, _) => format_table(&["value"], &[vec![cell_value(value)]]),
    }
}

fn render_csv(value: &Value, columns: Option<&'static [&'static str]>) -> String {
    let array_view = pick_array_payload_for_columns(value, columns);
    match (array_view, value) {
        (Some(items), _) if items.iter().all(|item| matches!(item, Value::Object(_))) => {
            let headers = resolve_object_headers(items, columns);
            let mut lines = vec![headers.join(",")];
            for item in items {
                if let Value::Object(map) = item {
                    let row = headers
                        .iter()
                        .map(|key| csv_escape(&cell_value(map.get(key).unwrap_or(&Value::Null))))
                        .collect::<Vec<String>>();
                    lines.push(row.join(","));
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
            // [Review Fix #FlattenDetail] CSV 单对象视图与 table 对齐，递归展平。
            let rows = flatten_object_to_rows("", value);
            let mut lines = vec!["field,value".to_string()];
            for row in &rows {
                lines.push(format!(
                    "{},{}",
                    csv_escape(row.first().map(|s| s.as_str()).unwrap_or_default()),
                    csv_escape(row.get(1).map(|s| s.as_str()).unwrap_or_default())
                ));
            }
            lines.join("\n")
        }
        (None, _) => format!("value\n{}", csv_escape(&cell_value(value))),
    }
}

/// [Review Fix #FmtUni] 当 payload 形如 `{"<list_key>": [...]}` 时返回内部数组，
/// 让 list-like 命令的渲染统一走"对象数组 → 表格"路径。`--query` 已显式提取出
/// 数组时会直接命中 Value::Array 分支，无需此处处理。
fn pick_array_payload(value: &Value) -> Option<&[Value]> {
    match value {
        Value::Array(items) => Some(items.as_slice()),
        Value::Object(map) => {
            let mut found: Option<&[Value]> = None;
            for v in map.values() {
                if let Value::Array(items) = v {
                    if found.is_some() {
                        return None;
                    }
                    found = Some(items.as_slice());
                }
            }
            found
        }
        _ => None,
    }
}

/// [Review Fix #FmtUni-Phase2] Column-aware payload picker. When a list-like
/// command declares its preferred columns, a payload with multiple sibling
/// arrays (e.g. `ListObjectsResponse { contents, common_prefixes }`) should
/// pick the array whose first object covers the most declared columns. Falls
/// back to the column-agnostic `pick_array_payload` when:
///   - no columns are declared,
///   - the payload itself is an array,
///   - exactly one sibling array exists,
///   - or no candidate array's keys overlap with the declared columns.
fn pick_array_payload_for_columns<'a>(
    value: &'a Value,
    columns: Option<&'static [&'static str]>,
) -> Option<&'a [Value]> {
    if let Some(cols) = columns {
        if let Value::Object(map) = value {
            let mut best: Option<(&[Value], i64)> = None;
            for v in map.values() {
                let Value::Array(items) = v else {
                    continue;
                };
                // Score: # of declared columns present in the first object.
                // Empty arrays score as 0 but are still candidates so that
                // empty-bucket responses still produce a stable column header
                // row instead of falling back to the field/value view.
                let score: i64 = items
                    .iter()
                    .find_map(|item| {
                        if let Value::Object(obj) = item {
                            Some(cols.iter().filter(|c| obj.contains_key(**c)).count() as i64)
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0);
                match best {
                    Some((_, prev)) if score <= prev => {}
                    _ => best = Some((items.as_slice(), score)),
                }
            }
            if let Some((items, _score)) = best {
                return Some(items);
            }
        }
    }
    pick_array_payload(value)
}

/// [Review Fix #FmtUni] 列头解析：声明列优先（Q6 选项 A），保留命令规定的顺序与裁剪；
/// 未声明则反射所有 JSON key（snake_case，与 Q2 一致）。
fn resolve_object_headers(
    items: &[Value],
    columns: Option<&'static [&'static str]>,
) -> Vec<String> {
    if let Some(cols) = columns {
        return cols.iter().map(|s| (*s).to_string()).collect();
    }
    let mut headers = Vec::new();
    for item in items {
        if let Value::Object(map) = item {
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
        Value::Bool(v) => v.to_string(),
        Value::Number(v) => v.to_string(),
        Value::String(v) => v.clone(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "<invalid-json>".to_string()),
    }
}

/// [Review Fix #FlattenDetail] 递归展平嵌套对象为 dot-notation 路径的 field/value 行。
/// - 嵌套 Object → 递归展开（`owner.id`）
/// - 纯标量数组 → 逗号拼接（`tags: a, b, c`）
/// - 复杂数组/其他 → 降级为 JSON 字符串
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
                        if items
                            .iter()
                            .all(|i| i.is_string() || i.is_number() || i.is_boolean()) =>
                    {
                        let joined = items
                            .iter()
                            .map(|i| cell_value(i))
                            .collect::<Vec<_>>()
                            .join(", ");
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
    if value.chars().any(|ch| matches!(ch, ',' | '"' | '\n')) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Build a query map from optional key/value pairs.
pub(crate) fn build_query(entries: &[(&str, Option<String>)]) -> BTreeMap<String, String> {
    let mut query = BTreeMap::new();
    for (key, value) in entries {
        if let Some(value) = value {
            query.insert((*key).to_string(), value.clone());
        }
    }
    query
}

/// Build a marker query map for APIs that use `?flag`.
pub(crate) fn marker_query(flags: &[&str]) -> BTreeMap<String, String> {
    let mut query = BTreeMap::new();
    for flag in flags {
        query.insert((*flag).to_string(), String::new());
    }
    query
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    /// [G8] Tests that read or write the `TOS_LAST_REQUEST_ID` env var must
    /// hold this lock for the duration of their interaction with the variable;
    /// `cargo test` runs unit tests in parallel by default and the env is
    /// process-global.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn global() -> GlobalArgs {
        GlobalArgs::default()
    }

    // [Review Fix #1] 已是 Envelope 形态时不应被双重包装
    #[test]
    fn ensure_envelope_passes_through_existing_envelope() {
        let env = json!({
            "status": "success",
            "command": "ve-tos object list",
            "request_id": "01HEXISTING",
            "data": {"objects": []},
        });
        let out = ensure_envelope(&global(), env);
        assert_eq!(out["command"], "tos object list");
        assert_eq!(out["request_id"], "01HEXISTING");
        assert_eq!(out["data"], json!({"objects": []}));
    }

    #[test]
    fn public_envelope_command_uses_tos_surface_for_ve_tos_commands() {
        assert_eq!(
            public_envelope_command_for_binary(Binary::Tos, "ve-tos ls"),
            "tos ls"
        );
        assert_eq!(
            public_envelope_command_for_binary(Binary::VeTos, "ve-tos ls"),
            "ve-tos ls"
        );
    }

    // [Review Fix #1] 裸数据自动被包装为 Envelope
    #[test]
    fn ensure_envelope_wraps_raw_value() {
        let raw = json!({"foo": "bar"});
        let out = ensure_envelope(&global(), raw.clone());
        assert_eq!(out["status"], "success");
        assert_eq!(out["data"], raw);
        assert!(out.get("command").is_some());
        // [G8] every Envelope must carry a non-empty request_id.
        assert!(out["request_id"].as_str().map_or(false, |s| !s.is_empty()));
    }

    // [G8] When the Envelope already has a request_id, do not overwrite it.
    #[test]
    fn ensure_envelope_preserves_existing_request_id() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("TOS_LAST_REQUEST_ID", "from-env");
        let env = json!({
            "status": "success",
            "command": "ve-tos object list",
            "request_id": "explicit-id",
            "data": {},
        });
        let out = ensure_envelope(&global(), env);
        assert_eq!(out["request_id"], "explicit-id");
        std::env::remove_var("TOS_LAST_REQUEST_ID");
    }

    #[test]
    fn ensure_envelope_preserves_explicit_null_request_id() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("TOS_LAST_REQUEST_ID", "from-env");
        let env = json!({
            "status": "success",
            "command": "ve-tos du",
            "request_id": null,
            "data": {},
        });
        let out = ensure_envelope(&global(), env);
        assert!(out["request_id"].is_null());
        std::env::remove_var("TOS_LAST_REQUEST_ID");
    }

    // [G8] When no request_id is present, prefer TOS_LAST_REQUEST_ID over a
    // freshly generated ULID so the Envelope carries the upstream TOS RequestId.
    #[test]
    fn ensure_envelope_uses_env_request_id_when_present() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("TOS_LAST_REQUEST_ID", "tos-upstream-id");
        let raw = json!({"foo": "bar"});
        let out = ensure_envelope(&global(), raw);
        assert_eq!(out["request_id"], "tos-upstream-id");
        std::env::remove_var("TOS_LAST_REQUEST_ID");
    }

    // [G8] Without env var, the injector must fall back to a generated ULID
    // so Agents always have a stable handle.
    #[test]
    fn ensure_envelope_generates_ulid_fallback() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var("TOS_LAST_REQUEST_ID");
        let raw = json!({"foo": "bar"});
        let out = ensure_envelope(&global(), raw);
        let id = out["request_id"].as_str().expect("request_id present");
        assert_eq!(id.len(), 26, "ULID must be 26 chars, got {:?}", id);
    }

    // [Review Fix #2] 路径表达式：标量字段
    // [G4] JMESPath integration tests — exercise the real grammar instead of
    // the previous handwritten subset.

    fn run_query(value: Value, expr: &str) -> Result<Value, CliError> {
        let mut g = GlobalArgs::default();
        g.query = Some(expr.to_string());
        apply_query(&g, value)
    }

    #[test]
    fn query_picks_scalar_field() {
        let v = json!({"data": {"crc64": "abc", "size": 1024}});
        assert_eq!(run_query(v, "data.crc64").unwrap(), json!("abc"));
    }

    #[test]
    fn query_indexes_array() {
        let v = json!({"data": [{"key": "a"}, {"key": "b"}]});
        assert_eq!(run_query(v, "data[1].key").unwrap(), json!("b"));
    }

    #[test]
    fn query_collects_array_with_wildcard() {
        let v = json!({"data": [{"key": "a"}, {"key": "b"}]});
        assert_eq!(run_query(v, "data[*].key").unwrap(), json!(["a", "b"]));
    }

    #[test]
    fn query_collects_object_with_wildcard() {
        let v = json!({"items": {"a": 1, "b": 2}});
        // JMESPath wildcard order is insertion-defined; just check membership.
        let out = run_query(v, "items.*").unwrap();
        let arr = out.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert!(arr.contains(&json!(1)));
        assert!(arr.contains(&json!(2)));
    }

    #[test]
    fn query_returns_null_when_path_missing() {
        // aws --query semantics: missing path is null, not an error.
        let v = json!({"a": 1});
        assert_eq!(run_query(v, "x.y.z").unwrap(), Value::Null);
    }

    #[test]
    fn query_supports_filter_expression() {
        let v = json!({"Contents": [
            {"Key": "a", "Size": 100},
            {"Key": "b", "Size": 5000},
            {"Key": "c", "Size": 200},
        ]});
        // Real JMESPath filter: pick objects whose Size > 1000.
        let out = run_query(v, "Contents[?Size > `1000`].Key").unwrap();
        assert_eq!(out, json!(["b"]));
    }

    #[test]
    fn query_invalid_expression_is_validation_error() {
        let v = json!({"a": 1});
        let err = run_query(v, "data[?").unwrap_err();
        assert!(matches!(err, CliError::ValidationError(_)));
    }

    // [Review Fix #2] apply_query 与 GlobalArgs.query 集成
    #[test]
    fn apply_query_returns_input_when_query_absent() {
        let v = json!({"a": 1});
        let out = apply_query(&global(), v.clone()).unwrap();
        assert_eq!(out, v);
    }

    #[test]
    fn apply_query_filters_using_global_query() {
        let mut g = global();
        g.query = Some("data.crc64".into());
        let v = json!({
            "status": "success",
            "command": "ve-tos cp",
            "data": {"crc64": "deadbeef", "etag": "x"}
        });
        let out = apply_query(&g, v).unwrap();
        assert_eq!(out, json!("deadbeef"));
    }

    #[test]
    fn apply_query_returns_null_for_missing_path() {
        // [G4] Match `aws --query` semantics: missing path → null (not error).
        let mut g = global();
        g.query = Some("nope.path".into());
        let v = json!({"a": 1});
        let out = apply_query(&g, v).unwrap();
        assert_eq!(out, Value::Null);
    }

    #[test]
    fn is_envelope_shape_recognizes_envelope() {
        assert!(is_envelope_shape(
            &json!({"status": "success", "command": "x"})
        ));
        assert!(!is_envelope_shape(&json!({"status": "success"})));
        assert!(!is_envelope_shape(&json!({"command": "x"})));
        assert!(!is_envelope_shape(&json!([1, 2, 3])));
    }

    #[test]
    fn unwrap_envelope_data_returns_inner_payload() {
        let envelope = json!({
            "status": "success",
            "command": "ve-tos object upload",
            "data": {"action": "ve-tos object upload", "dry_run": true},
        });
        let inner = unwrap_envelope_data(&envelope);
        assert_eq!(inner["action"], "ve-tos object upload");
        assert_eq!(inner["dry_run"], true);
    }

    #[test]
    fn unwrap_envelope_data_returns_value_when_not_envelope() {
        let raw = json!({"foo": "bar"});
        let inner = unwrap_envelope_data(&raw);
        assert_eq!(inner["foo"], "bar");
    }

    // [Review Fix #FmtUni] Format Uniformity 不变量测试 ---------------------------
    // 这些用例锁住设计决策（Q1-Q9）：snake_case 列头、声明列、自动数组提取、
    // 标准化 footer、--verbose meta 头。任意一项被破坏即测试失败。

    #[test]
    fn render_table_uses_snake_case_field_value_for_object() {
        let payload = json!({"name": "demo", "size": 1024});
        let out = render_table(&payload, None);
        assert!(
            out.contains("field"),
            "table header must be lowercase 'field': {}",
            out
        );
        assert!(
            out.contains("value"),
            "table header must be lowercase 'value': {}",
            out
        );
        assert!(
            !out.contains("FIELD"),
            "header must not be SCREAMING-CASE: {}",
            out
        );
    }

    #[test]
    fn render_table_pulls_array_out_of_payload() {
        let payload = json!({"buckets": [
            {"name": "a", "location": "cn-beijing", "creation_date": "2024-01"},
            {"name": "b", "location": "cn-shanghai", "creation_date": "2024-02"},
        ]});
        let out = render_table(&payload, Some(&["name", "location", "creation_date"]));
        assert!(out.contains("name"));
        assert!(out.contains("location"));
        assert!(out.contains("creation_date"));
        assert!(out.contains("cn-beijing"));
    }

    #[test]
    fn render_table_respects_declared_column_order() {
        let payload = json!([
            {"a": 1, "b": 2, "c": 3},
            {"a": 4, "b": 5, "c": 6},
        ]);
        let out = render_table(&payload, Some(&["c", "a"]));
        let header_line = out.lines().nth(1).unwrap_or_default();
        let c_pos = header_line.find('c').unwrap_or(usize::MAX);
        let a_pos = header_line.find('a').unwrap_or(usize::MAX);
        assert!(
            c_pos < a_pos,
            "declared 'c' must precede 'a' in header: {}",
            header_line
        );
        assert!(
            !header_line.contains('b'),
            "undeclared column 'b' must be absent: {}",
            header_line
        );
    }

    #[test]
    fn envelope_footer_emits_total_from_pagination() {
        let env = json!({
            "status": "success",
            "command": "ve-tos bucket list",
            "data": {"buckets": []},
            "pagination": {"total_returned": 7},
        });
        assert_eq!(envelope_footer(&env), Some("Total: 7".to_string()));
    }

    #[test]
    fn envelope_footer_emits_next_token_when_present() {
        let env = json!({
            "status": "success",
            "command": "ve-tos object list",
            "data": {"objects": []},
            "pagination": {"total_returned": 100, "next_token": "abc"},
        });
        assert_eq!(
            envelope_footer(&env),
            Some("Total: 100 (next_token=abc)".to_string())
        );
    }

    #[test]
    fn envelope_footer_returns_none_without_pagination() {
        let env = json!({
            "status": "success",
            "command": "ve-tos object head",
            "data": {"size": 10},
        });
        assert_eq!(envelope_footer(&env), None);
    }

    #[test]
    fn meta_line_only_under_verbose() {
        let env = json!({
            "status": "success",
            "command": "ve-tos bucket list",
            "request_id": "01H...",
            "data": {"buckets": []},
        });
        let meta = format_envelope_meta_line(&env).expect("meta line");
        assert!(meta.contains("ve-tos bucket list"));
        assert!(meta.contains("request_id=01H..."));
        // Non-Envelope value yields no meta line.
        assert!(format_envelope_meta_line(&json!({"foo": "bar"})).is_none());
    }

    #[test]
    fn render_csv_uses_declared_columns_and_snake_case() {
        let payload = json!({"buckets": [
            {"name": "a", "location": "cn-beijing", "creation_date": "2024-01"},
            {"name": "b", "location": "cn-shanghai", "creation_date": "2024-02"},
        ]});
        let out = render_csv(&payload, Some(&["name", "location", "creation_date"]));
        let header = out.lines().next().unwrap_or_default();
        assert_eq!(header, "name,location,creation_date");
        assert!(
            !header.contains("Name"),
            "csv header must be snake_case: {}",
            header
        );
    }

    #[test]
    fn pick_array_payload_returns_inner_array_for_single_array_object() {
        let v = json!({"buckets": [{"x": 1}]});
        assert!(pick_array_payload(&v).is_some());
    }

    #[test]
    fn pick_array_payload_none_when_two_arrays_present() {
        // Ambiguous payload: pick neither, fall back to generic object rendering.
        let v = json!({"buckets": [], "regions": []});
        assert!(pick_array_payload(&v).is_none());
    }

    // [Review Fix #FmtUni-C1] BucketInfo 序列化必须是 snake_case，否则 table 全空
    #[test]
    fn bucket_info_serializes_snake_case_for_unified_renderer() {
        use crate::domain::bucket::BucketInfo;
        let bi = BucketInfo {
            name: "demo".into(),
            location: "cn-beijing".into(),
            creation_date: "2026-01-01T00:00:00Z".into(),
            extranet_endpoint: "tos-cn-beijing.volces.com".into(),
            intranet_endpoint: "tos-cn-beijing.ivolces.com".into(),
            project_name: Some("default".into()),
            bucket_type: Some("hns".into()),
        };
        let v = serde_json::to_value(&bi).expect("serialize");
        assert_eq!(v["name"], "demo");
        assert_eq!(v["location"], "cn-beijing");
        assert_eq!(v["creation_date"], "2026-01-01T00:00:00Z");
        assert_eq!(v["project_name"], "default");
        assert_eq!(v["bucket_type"], "hns");
        assert!(
            v.get("Name").is_none(),
            "PascalCase must not leak to CLI output"
        );
        assert!(v.get("CreationDate").is_none());
    }

    // [Review Fix #FmtUni-C1] 反序列化仍兼容 TOS 服务端 PascalCase JSON
    #[test]
    fn bucket_info_deserializes_pascalcase_from_tos_service() {
        use crate::domain::bucket::ListBucketsResponse;
        let body = r#"{"Buckets":[{"Name":"demo","Location":"cn-beijing","CreationDate":"2026-01-01T00:00:00Z","ExtranetEndpoint":"e","IntranetEndpoint":"i","ProjectName":"default","BucketType":"hns"}],"Owner":{"ID":"acc-1"}}"#;
        let parsed: ListBucketsResponse = serde_json::from_str(body).expect("parse pascal");
        assert_eq!(parsed.buckets[0].name, "demo");
        assert_eq!(parsed.buckets[0].project_name.as_deref(), Some("default"));
        assert_eq!(parsed.buckets[0].bucket_type.as_deref(), Some("hns"));
        assert_eq!(parsed.owner.id, "acc-1");
    }

    // [Review Fix #FmtUni-C1] bucket list 端到端：列声明能取到值
    #[test]
    fn bucket_list_envelope_renders_non_empty_table() {
        // 模拟 handle_list 的 Envelope.data 形态（snake_case）
        let envelope = json!({
            "status": "success",
            "command": "ve-tos bucket list",
            "request_id": "01HBUCKET",
            "data": {
                "buckets": [
                    {"name": "demo-1", "location": "cn-beijing", "bucket_type": "hns", "creation_date": "2026-01-01T00:00:00Z"},
                    {"name": "demo-2", "location": "cn-shanghai", "bucket_type": "fns", "creation_date": "2026-01-02T00:00:00Z"},
                ],
                "owner": {"id": "acc-1"}
            }
        });
        let payload = unwrap_envelope_data(&envelope);
        let out = render_table(
            payload,
            Some(&["name", "location", "bucket_type", "creation_date"]),
        );
        assert!(
            out.contains("demo-1"),
            "rendered table missing data:\n{}",
            out
        );
        assert!(
            out.contains("cn-beijing"),
            "rendered table missing data:\n{}",
            out
        );
        assert!(out.contains("hns"), "bucket_type missing:\n{}", out);
        assert!(
            out.contains("bucket_type"),
            "bucket_type header missing:\n{}",
            out
        );
        assert!(out.contains("name"), "snake_case header missing:\n{}", out);
    }

    // [Review Fix #FmtUni-C2] RawResponseData 透视：object list 的 body 才能被 table 渲染
    #[test]
    fn unwrap_envelope_data_drills_into_raw_response_body() {
        let envelope = json!({
            "status": "success",
            "command": "ve-tos object list",
            "request_id": "01HOBJECT",
            "data": {
                "status_code": 200,
                "headers": {"x-tos-request-id": "rid"},
                "body_format": "xml",
                "body": {
                    "Contents": [
                        {"Key": "a.txt", "Size": "10"},
                        {"Key": "b.txt", "Size": "20"},
                    ]
                }
            }
        });
        let unwrapped = unwrap_envelope_data(&envelope);
        // 应该已经是 body，不是 RawResponseData 包装
        assert!(
            unwrapped.get("Contents").is_some(),
            "body must be unwrapped: {}",
            unwrapped
        );
        assert!(unwrapped.get("status_code").is_none());
    }

    // [Review Fix #FmtUni-C2] body 缺失时不能误把外壳吞掉
    #[test]
    fn unwrap_envelope_data_keeps_raw_response_when_body_null() {
        let envelope = json!({
            "status": "success",
            "command": "ve-tos head",
            "request_id": "01HHEAD",
            "data": {
                "status_code": 200,
                "headers": {"etag": "x"},
                "body_format": null,
                "body": null,
            }
        });
        let unwrapped = unwrap_envelope_data(&envelope);
        assert!(
            unwrapped.get("status_code").is_some(),
            "should keep raw response shell when body is null"
        );
    }

    #[test]
    fn force_for_non_delete_destructive_passes_with_force_flag() {
        let g = global();
        assert!(
            ensure_force_for_destructive(&g, true, "ve-tos multipart abort", "upload-id").is_ok()
        );
    }

    #[test]
    fn force_for_delete_requires_confirm_after_force_in_non_tty() {
        let g = global();
        let result = ensure_force_for_destructive(&g, true, "ve-tos bucket delete", "my-bucket");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("--confirm tos://my-bucket"),
            "delete force should require URI confirm: {}",
            msg
        );
    }

    #[test]
    fn force_for_delete_accepts_exact_uri_confirm() {
        let mut g = global();
        g.confirm = Some("tos://my-bucket".to_string());
        assert!(
            ensure_force_for_destructive(&g, true, "ve-tos bucket delete", "my-bucket").is_ok()
        );
    }

    #[test]
    fn force_for_delete_rejects_bare_confirm() {
        let mut g = global();
        g.confirm = Some("my-bucket".to_string());
        let result = ensure_force_for_destructive(&g, true, "ve-tos bucket delete", "my-bucket");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("tos://my-bucket"),
            "delete confirm mismatch should show URI target: {}",
            msg
        );
    }

    #[test]
    fn force_for_destructive_rejects_yes_in_non_tty() {
        let mut g = global();
        g.yes = true;
        let result = ensure_force_for_destructive(&g, false, "ve-tos multipart abort", "upload-id");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("--force"),
            "--yes must not replace --force outside an interactive prompt: {}",
            msg
        );
    }

    #[test]
    fn force_for_destructive_rejects_without_force_in_non_tty() {
        let g = global();
        let result = ensure_force_for_destructive(&g, false, "ve-tos bucket delete", "my-bucket");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("--force"),
            "error should hint --force: {}",
            msg
        );
    }

    #[test]
    fn force_for_destructive_rejects_with_quiet_even_in_tty() {
        let mut g = global();
        g.quiet = true;
        let result = ensure_force_for_destructive(&g, false, "ve-tos object delete", "tos://b/k");
        assert!(result.is_err());
    }

    #[test]
    fn force_for_destructive_error_message_mentions_target() {
        let g = global();
        let result =
            ensure_force_for_destructive(&g, false, "ve-tos multipart abort", "upload-id-123");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("upload-id-123"),
            "error must mention target: {}",
            msg
        );
        assert!(
            msg.contains("ve-tos multipart abort"),
            "error must mention command: {}",
            msg
        );
    }

    #[test]
    fn force_for_delete_yes_does_not_replace_confirm_in_non_tty() {
        let mut g = global();
        g.yes = true;
        g.quiet = true;
        let result = ensure_force_for_destructive(&g, false, "ve-tos bucket delete", "test-bucket");
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("--confirm tos://test-bucket"), "{}", msg);
    }
}
