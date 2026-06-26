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

use std::collections::{BTreeMap, HashMap};

use crate::cli::low_level::*;
use crate::domain::core::{self, DownloadResult, RawResponseData};
use crate::domain::object as object_domain;
use crate::handler::common::{
    build_profile, build_query, classify_body_input, ensure_force_for_destructive, marker_query,
    output_result, output_result_with_columns, parse_kv_pairs, parse_object_target,
    read_body_input, read_json_input, validate_bucket_flag_target, BodyInput,
};
use reqwest::Method;
use serde_json::{json, Value};
use tos_core::agent::describe::{
    CommandDescription, CommandLayer, CommandParameter, ParameterLocation, RiskLevel,
};
use tos_core::agent::dryrun::{DryRunResult, Impact};
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::client::TosClient;

/// Handle `ve-tos object ...` subcommands.
pub async fn handle_object_command(
    global: &GlobalArgs,
    action: &Option<ObjectAction>,
) -> Result<i32, CliError> {
    if global.describe {
        if let Some(action) = action {
            output_result(global, &describe_object_action(action))?;
        } else {
            output_result(global, &describe_object_group())?;
        }
        return Ok(0);
    }

    let Some(action) = action else {
        return Err(CliError::ValidationError(
            "`ve-tos object` requires a subcommand; use `ve-tos object --help` or `ve-tos object --describe`".to_string(),
        ));
    };

    if global.dry_run {
        output_result(global, &dry_run_object_action(action)?)?;
        return Ok(0);
    }

    // [Review Fix #1] Pre-flight registry guard so destructive object
    // commands fail with a deterministic ValidationError before we attempt
    // to build the runtime profile (which would otherwise mask the missing
    // --force with a ConfigMissing error).
    // [Review Fix #ForceGate] Pre-flight registry guard：添加 TTY 感知，
    // 交互式终端下放行至 handler 内部的 ensure_force_for_destructive 处理提示。
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let stderr_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let can_prompt = stdin_tty && stderr_tty && !global.quiet;
    if !can_prompt {
        match action {
            ObjectAction::Delete(args) => {
                let (bucket, key) = parse_object_target(
                    args.uri.as_deref(),
                    args.bucket.as_deref(),
                    args.key.as_deref(),
                )?;
                // [Review Fix #7] Validate non-interactive critical delete
                // confirmation before loading config or constructing a client.
                ensure_force_for_destructive(
                    global,
                    args.force,
                    "ve-tos object delete",
                    &format!("tos://{bucket}/{key}"),
                )?;
            }
            ObjectAction::BatchDelete(args) => {
                let bucket = args.bucket.require()?;
                // [Review Fix #7] Batch delete affects a bucket scope and must
                // use the public tos://bucket confirmation token in automation.
                ensure_force_for_destructive(
                    global,
                    args.force,
                    "ve-tos object batch-delete",
                    &format!("tos://{bucket}"),
                )?;
            }
            ObjectAction::DeleteTagging(args) => {
                let (bucket, key) = parse_object_target(
                    args.uri.as_deref(),
                    args.bucket.as_deref(),
                    args.key.as_deref(),
                )?;
                // [Review Fix #2] Object tag deletion is a delete-class
                // operation, so non-interactive callers must confirm the exact
                // public object URI before any network request is built.
                ensure_force_for_destructive(
                    global,
                    args.force,
                    "ve-tos object delete-tagging",
                    &format!("tos://{bucket}/{key}"),
                )?;
            }
            _ => {}
        }
    }
    let force_flag = match action {
        ObjectAction::Delete(args) => args.force,
        ObjectAction::BatchDelete(args) => args.force,
        ObjectAction::DeleteTagging(args) => args.force,
        _ => false,
    } || (global.yes && can_prompt)
        || can_prompt;
    if let Err(violation) = crate::registry::enforce_registry_guards(
        &format!("ve-tos object {}", object_action_name(action)),
        force_flag,
        stderr_tty,
    ) {
        return Err(CliError::ValidationError(format!(
            "{} requires --force (or run in an interactive terminal)",
            violation.command
        )));
    }

    // [Review Fix #5] Validate rename cross-bucket invariant before we
    // touch the runtime profile so that the deterministic ValidationError
    // surfaces independent of region/credential configuration.
    if let ObjectAction::Rename(args) = action {
        let (src_bucket, _) = parse_object_target(Some(&args.source), None, None)?;
        let (dst_bucket, _) = parse_object_target(Some(&args.destination), None, None)?;
        if src_bucket != dst_bucket {
            return Err(CliError::ValidationError(format!(
                "rename destination bucket must match source bucket: source={src_bucket}, destination={dst_bucket}"
            )));
        }
    }

    let profile = build_profile(global)?;
    let client = TosClient::new(&profile, "tos")?;

    match action {
        ObjectAction::Upload(args) => handle_upload(global, &client, args).await,
        ObjectAction::Download(args) => handle_download(global, &client, args).await,
        ObjectAction::FormUpload(args) => handle_form_upload(global, &client, args).await,
        ObjectAction::Copy(args) => handle_copy(global, &client, args).await,
        ObjectAction::Delete(args) => handle_delete(global, &client, args).await,
        ObjectAction::BatchDelete(args) => handle_batch_delete(global, &client, args).await,
        ObjectAction::List(args) => handle_list(global, &client, args).await,
        ObjectAction::ListVersions(args) => handle_list_versions(global, &client, args).await,
        ObjectAction::Head(args) => handle_head(global, &client, args).await,
        ObjectAction::Stat(args) => handle_stat(global, &client, args).await,
        ObjectAction::Status(args) => handle_status(global, &client, args).await,
        ObjectAction::SetMeta(args) => handle_set_meta(global, &client, args).await,
        ObjectAction::SetTime(args) => handle_set_time(global, &client, args).await,
        ObjectAction::SetExpires(args) => handle_set_expires(global, &client, args).await,
        ObjectAction::Append(args) => handle_append(global, &client, args).await,
        ObjectAction::SealAppend(args) => handle_seal_append(global, &client, args).await,
        ObjectAction::Modify(args) => handle_modify(global, &client, args).await,
        ObjectAction::Rename(args) => handle_rename(global, &client, args).await,
        ObjectAction::Restore(args) => handle_restore(global, &client, args).await,
        ObjectAction::GetAcl(args) => handle_get_acl(global, &client, args).await,
        ObjectAction::SetAcl(args) => handle_set_acl(global, &client, args).await,
        ObjectAction::GetTagging(args) => handle_get_tagging(global, &client, args).await,
        ObjectAction::SetTagging(args) => handle_set_tagging(global, &client, args).await,
        ObjectAction::DeleteTagging(args) => handle_delete_tagging(global, &client, args).await,
        ObjectAction::Link(args) => handle_link(global, &client, args).await,
        ObjectAction::GetSymlink(args) => handle_get_symlink(global, &client, args).await,
        ObjectAction::CreateSymlink(args) => handle_create_symlink(global, &client, args).await,
        ObjectAction::GetFetchTask(args) => handle_get_fetch_task(global, &client, args).await,
        ObjectAction::CreateFetchTask(args) => {
            handle_create_fetch_task(global, &client, args).await
        }
        ObjectAction::Fetch(args) => handle_fetch(global, &client, args).await,
        ObjectAction::SetRetention(args) => handle_set_retention(global, &client, args).await,
        ObjectAction::GetRetention(args) => handle_get_retention(global, &client, args).await,
    }?;

    Ok(0)
}

/// [Review Fix #1] Map an `ObjectAction` to the registry leaf name so the
/// pre-flight guard can look up the correct `EffectiveCapability`.
fn object_action_name(action: &ObjectAction) -> &'static str {
    match action {
        ObjectAction::Upload(_) => "upload",
        ObjectAction::Download(_) => "download",
        ObjectAction::FormUpload(_) => "form-upload",
        ObjectAction::Copy(_) => "copy",
        ObjectAction::Delete(_) => "delete",
        ObjectAction::BatchDelete(_) => "batch-delete",
        ObjectAction::List(_) => "list",
        ObjectAction::ListVersions(_) => "list-versions",
        ObjectAction::Head(_) => "head",
        ObjectAction::Stat(_) => "stat",
        ObjectAction::Status(_) => "status",
        ObjectAction::SetMeta(_) => "set-meta",
        ObjectAction::SetTime(_) => "set-time",
        ObjectAction::SetExpires(_) => "set-expires",
        ObjectAction::Append(_) => "append",
        ObjectAction::SealAppend(_) => "seal-append",
        ObjectAction::Modify(_) => "modify",
        ObjectAction::Rename(_) => "rename",
        ObjectAction::Restore(_) => "restore",
        ObjectAction::GetAcl(_) => "get-acl",
        ObjectAction::SetAcl(_) => "set-acl",
        ObjectAction::GetTagging(_) => "get-tagging",
        ObjectAction::SetTagging(_) => "set-tagging",
        ObjectAction::DeleteTagging(_) => "delete-tagging",
        ObjectAction::Link(_) => "link",
        ObjectAction::GetSymlink(_) => "get-symlink",
        ObjectAction::CreateSymlink(_) => "create-symlink",
        ObjectAction::GetFetchTask(_) => "get-fetch-task",
        ObjectAction::CreateFetchTask(_) => "create-fetch-task",
        ObjectAction::Fetch(_) => "fetch",
        ObjectAction::SetRetention(_) => "set-retention",
        ObjectAction::GetRetention(_) => "get-retention",
    }
}

async fn handle_upload(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectUploadArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let target_uri = format!("tos://{}/{}", bucket, key);
    crate::handler::high_level::ensure_tos_upload_storage_class_supported(
        "ve-tos object upload",
        None,
        &target_uri,
        args.storage_class.as_deref(),
    )?;
    let body_source = args.body.as_deref().ok_or_else(|| {
        CliError::ValidationError("`--body` is required for object upload".into())
    })?;
    // [Review Fix #M4] When `--body` resolves to a local file path, stream it
    // directly through `execute_object_streaming_request` instead of buffering
    // the entire payload (Streaming I/O hard constraint). stdin/inline bytes
    // remain on the buffered fast path because they are already in memory.
    let body_input = classify_body_input(body_source)?;
    let mut headers = object_write_headers(
        args.content_type.as_deref(),
        args.storage_class.as_deref(),
        args.meta.as_deref(),
        args.net_speed_test.as_deref(),
    );
    // ACL headers
    insert_acl_headers(
        &mut headers,
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
        args.tagging.clone(),
    );
    // WORM headers
    insert_optional_header(
        &mut headers,
        "x-object-lock-mode",
        args.object_lock_mode.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-object-lock-retain-until-date",
        args.object_lock_retain_until_date.clone(),
    );
    // Conditional write headers
    insert_optional_header(&mut headers, "if-none-match", args.if_none_match.clone());
    if args.forbid_overwrite {
        headers.insert("x-tos-forbid-overwrite".to_string(), "true".to_string());
    }
    // Integrity / traffic
    insert_optional_header(&mut headers, "Content-MD5", args.content_md5.clone());
    if let Some(limit) = args.traffic_limit {
        headers.insert("x-traffic-limit".to_string(), limit.to_string());
    }
    // Misc
    insert_optional_header(
        &mut headers,
        "x-persistent-headers",
        args.persistent_headers.clone(),
    );
    insert_optional_header(&mut headers, "x-etag-pattern", args.etag_pattern.clone());
    match body_input {
        BodyInput::FilePath { path, len } => {
            let path_str = path.to_string_lossy().to_string();
            // [Review Fix #1] PutObject rejects chunked streaming in this
            // path; send a fixed Content-Length just like append.
            headers.insert("content-length".to_string(), len.to_string());
            let payload_hash = crate::handler::high_level::file_sha256(&path_str)?;
            let body = crate::handler::high_level::file_stream_body(&path_str).await?;
            let result = core::execute_object_streaming_request(
                client,
                "ve-tos object upload",
                Method::PUT,
                &bucket,
                &key,
                BTreeMap::new(),
                headers,
                payload_hash,
                body,
            )
            .await?;
            output_result(global, &result)
        }
        BodyInput::Inline(bytes) => {
            let result = core::execute_object_request(
                client,
                "ve-tos object upload",
                Method::PUT,
                &bucket,
                &key,
                BTreeMap::new(),
                headers,
                Some(bytes),
            )
            .await?;
            output_result(global, &result)
        }
    }
}

async fn handle_download(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectDownloadArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mut headers = BTreeMap::new();
    if let Some(range) = &args.range {
        headers.insert("range".to_string(), range.clone());
    }
    insert_optional_header(
        &mut headers,
        "If-Modified-Since",
        args.if_modified_since.clone(),
    );
    insert_optional_header(
        &mut headers,
        "If-Unmodified-Since",
        args.if_unmodified_since.clone(),
    );
    insert_optional_header(&mut headers, "If-Match", args.if_match.clone());
    insert_optional_header(&mut headers, "If-None-Match", args.if_none_match.clone());
    if let Some(limit) = args.traffic_limit {
        headers.insert("x-traffic-limit".to_string(), limit.to_string());
    }
    insert_optional_header(
        &mut headers,
        "X-Replicated-From",
        args.replicated_from.clone(),
    );
    insert_optional_header(&mut headers, "X-From-Modular", args.from_modular.clone());
    let query = build_query(&[
        ("versionId", args.version_id.clone()),
        ("response-content-type", args.response_content_type.clone()),
        (
            "response-content-disposition",
            args.response_content_disposition.clone(),
        ),
        (
            "response-cache-control",
            args.response_cache_control.clone(),
        ),
        ("response-expires", args.response_expires.clone()),
        ("partNumber", args.part_number.map(|n| n.to_string())),
    ]);
    let resp =
        core::send_object_request(client, Method::GET, &bucket, &key, query, headers, None).await?;
    let request_id = core::extract_request_id(&resp);
    let response_headers = core::extract_headers(&resp);
    let mut resp = client.check_response(resp).await?;

    // [Review Fix #M1] Stream the response body instead of buffering it all
    // into memory; this keeps `ve-tos object download` aligned with the
    // "Streaming I/O" hard constraint and behaves identically to high-level
    // `cp` for arbitrarily large objects.
    if let Some(output) = &args.body {
        if output == "-" {
            crate::handler::high_level::stream_response_to_stdout(&mut resp).await?;
            return Ok(());
        }

        // [Review Fix #M1] Write to `<dest>.tos-partial-<pid>` first, then
        // atomically rename. Partial files are removed on error so we never
        // leave the destination in a half-written state.
        let dest_path = std::path::Path::new(output);
        let temp_path = crate::handler::high_level::partial_path(dest_path);
        let bytes_written =
            match crate::handler::high_level::write_response_stream(&mut resp, &temp_path).await {
                Ok(n) => n,
                Err(err) => {
                    let _ = std::fs::remove_file(&temp_path);
                    return Err(err);
                }
            };
        if let Err(err) = std::fs::rename(&temp_path, dest_path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(CliError::Io(err));
        }

        let envelope = tos_core::agent::envelope::Envelope::success(
            "ve-tos object download",
            DownloadResult {
                bucket,
                key,
                output: output.clone(),
                bytes_written,
                headers: response_headers,
            },
        )
        .with_request_id(request_id);
        return output_result(global, &envelope);
    }

    // [Review Fix #m1] No `--body` provided: previously this path called
    // `String::from_utf8_lossy(&bytes)` which silently corrupts binary
    // payloads and lies in `body_format: "text"`. We now classify the
    // response by Content-Type and return base64 for binary bodies, while
    // still streaming the body into memory in bounded chunks. Operators that
    // want zero-copy behavior MUST pass `--body <path>` or `--body -`.
    let content_type = response_headers
        .get("content-type")
        .map(String::as_str)
        .unwrap_or("");
    let is_textual = is_textual_content_type(content_type);
    let mut buffer: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(CliError::Http)? {
        buffer.extend_from_slice(&chunk);
    }

    let (body_format, body_value) = if is_textual {
        match String::from_utf8(buffer.clone()) {
            Ok(text) => ("text".to_string(), json!({ "raw": text })),
            // Even when the Content-Type claims to be textual, fall back to
            // base64 if the bytes are not valid UTF-8 — better safe than
            // silently corrupting.
            Err(_) => (
                "base64".to_string(),
                json!({ "base64": base64_encode(&buffer) }),
            ),
        }
    } else {
        (
            "base64".to_string(),
            json!({ "base64": base64_encode(&buffer) }),
        )
    };

    let envelope = tos_core::agent::envelope::Envelope::success(
        "ve-tos object download",
        RawResponseData {
            status_code: 200,
            headers: response_headers,
            body_format: Some(body_format),
            body: Some(body_value),
        },
    )
    .with_request_id(request_id);
    output_result(global, &envelope)
}

/// [Review Fix #m1] Heuristic to decide whether a response body is safe to
/// surface as a UTF-8 string. We treat the standard `text/*` family plus the
/// well-known JSON / XML / YAML / form-encoded / JS / SQL / shell types as
/// textual. Anything else (octet-stream, image/*, video/*, archives, ...) is
/// surfaced as base64.
fn is_textual_content_type(content_type: &str) -> bool {
    let lowered = content_type.to_ascii_lowercase();
    let media_type = lowered.split(';').next().unwrap_or("").trim();
    if media_type.is_empty() {
        return false;
    }
    if media_type.starts_with("text/") {
        return true;
    }
    matches!(
        media_type,
        "application/json"
            | "application/xml"
            | "application/yaml"
            | "application/x-yaml"
            | "application/javascript"
            | "application/x-www-form-urlencoded"
            | "application/x-sh"
            | "application/sql"
    )
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine;
    BASE64_STANDARD.encode(bytes)
}

async fn handle_form_upload(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectFormUploadArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let body_source = args
        .body
        .as_deref()
        .ok_or_else(|| CliError::ValidationError("`--body` is required for form upload".into()))?;
    let file_content = read_body_input(body_source)?;
    let filename = std::path::Path::new(body_source)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("upload");

    // [Review Fix] PostObject requires form-based signing (not header-based V4).
    // Step 1: Prepare credential/date/algorithm for policy construction
    let prep = client.form_prepare();

    // Step 2: Build policy JSON with expiration and conditions
    let expiration = (chrono::Utc::now() + chrono::Duration::minutes(5))
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();

    let mut conditions: Vec<serde_json::Value> = vec![
        json!({"bucket": bucket}),
        json!(["starts-with", "$key", ""]),
        json!({"x-tos-algorithm": &prep.algorithm}),
        json!({"x-tos-credential": &prep.credential}),
        json!({"x-tos-date": &prep.date}),
    ];
    if args.content_type.is_some() {
        conditions.push(json!(["starts-with", "$Content-Type", ""]));
    }
    if args.storage_class.is_some() {
        conditions.push(json!(["starts-with", "$x-tos-storage-class", ""]));
    }
    if args.meta.is_some() {
        conditions.push(json!(["starts-with", "$x-tos-meta-", ""]));
    }
    if let Some(ref token) = prep.security_token {
        conditions.push(json!({"x-tos-security-token": token}));
    }

    let policy_json = json!({
        "expiration": expiration,
        "conditions": conditions,
    });
    let policy_base64 = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(policy_json.to_string().as_bytes())
    };

    // Step 3: Compute the real signature over the base64 policy
    let signature = client.form_sign(&prep.date_short, &policy_base64);

    // Step 4: Build multipart/form-data body
    let boundary = format!(
        "TosCliFormBoundary{:016x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let mut form_body: Vec<u8> = Vec::new();

    write_form_field(&mut form_body, &boundary, "key", &key);
    write_form_field(
        &mut form_body,
        &boundary,
        "x-tos-algorithm",
        &prep.algorithm,
    );
    write_form_field(
        &mut form_body,
        &boundary,
        "x-tos-credential",
        &prep.credential,
    );
    write_form_field(&mut form_body, &boundary, "x-tos-date", &prep.date);
    write_form_field(&mut form_body, &boundary, "policy", &policy_base64);
    write_form_field(&mut form_body, &boundary, "x-tos-signature", &signature);
    if let Some(ref token) = prep.security_token {
        write_form_field(&mut form_body, &boundary, "x-tos-security-token", token);
    }
    if let Some(ref ct) = args.content_type {
        write_form_field(&mut form_body, &boundary, "Content-Type", ct);
    }
    if let Some(ref sc) = args.storage_class {
        write_form_field(&mut form_body, &boundary, "x-tos-storage-class", sc);
    }
    if let Some(ref meta) = args.meta {
        for pair in meta.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                write_form_field(&mut form_body, &boundary, &format!("x-tos-meta-{}", k), v);
            }
        }
    }

    // File field (MUST be last per PostObject spec)
    form_body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    form_body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n",
            filename
        )
        .as_bytes(),
    );
    form_body.extend_from_slice(&file_content);
    form_body.extend_from_slice(b"\r\n");
    form_body.extend_from_slice(format!("--{}--\r\n", boundary).as_bytes());

    // Step 5: Send request without Authorization header
    let url = format!("{}/", client.bucket_endpoint(&bucket)?);
    let content_type_header = format!("multipart/form-data; boundary={}", boundary);
    let response = client
        .send_form_post(&url, &content_type_header, form_body)
        .await?;

    // Step 6: Check response through the standard error-handling path
    let resp = client.check_response(response).await?;
    let response_body = resp.text().await.unwrap_or_default();
    let envelope = tos_core::agent::envelope::Envelope::success(
        "ve-tos object form-upload",
        if response_body.is_empty() {
            json!({"bucket": bucket, "key": key})
        } else {
            serde_json::from_str(&response_body).unwrap_or(json!({"raw": response_body}))
        },
    );
    output_result(global, &envelope)
}

/// Write a single form field for multipart/form-data encoding.
fn write_form_field(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(format!("--{}\r\n", boundary).as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"{}\"\r\n\r\n", name).as_bytes(),
    );
    body.extend_from_slice(value.as_bytes());
    body.extend_from_slice(b"\r\n");
}

async fn handle_copy(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectCopyArgs,
) -> Result<(), CliError> {
    let (src_bucket, src_key) = parse_object_target(Some(&args.source), None, None)?;
    let (dst_bucket, dst_key) = parse_object_target(Some(&args.destination), None, None)?;
    let mut headers = BTreeMap::new();
    headers.insert(
        // [Review Fix #M3] Use TOS-native copy headers (x-tos-*).
        "x-tos-copy-source".to_string(),
        format!("/{}/{}", src_bucket, src_key),
    );
    insert_optional_header(&mut headers, "range", args.range.clone());
    insert_optional_header(
        &mut headers,
        "x-tos-copy-source-if-modified-since",
        args.copy_source_if_modified_since.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-tos-copy-source-if-unmodified-since",
        args.copy_source_if_unmodified_since.clone(),
    );
    insert_optional_header(&mut headers, "x-etag-pattern", args.etag_pattern.clone());
    insert_optional_header(&mut headers, "x-unique-tag", args.unique_tag.clone());
    insert_optional_header(
        &mut headers,
        "x-tos-copy-source-last-modified",
        args.copy_source_last_modified.clone(),
    );
    insert_optional_header(&mut headers, "x-data-id", args.data_id.clone());
    insert_optional_header(&mut headers, "x-finger-print", args.finger_print.clone());
    insert_optional_header(
        &mut headers,
        "x-internal-metadata-directive",
        args.internal_metadata_directive.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-crr-source-timestamp-nsec",
        args.crr_source_timestamp_nsec.clone(),
    );
    insert_optional_header(&mut headers, "x-crr-proxy", args.crr_proxy.clone());
    insert_optional_header(
        &mut headers,
        "x-crr-source-bucket-version-status",
        args.crr_source_bucket_version_status.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-traffic-limit",
        args.traffic_limit.map(|value| value.to_string()),
    );
    insert_optional_header(
        &mut headers,
        "x-object-lock-mode",
        args.object_lock_mode.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-object-lock-retain-until-date",
        args.object_lock_retain_until_date.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-if-unmodified-since",
        args.if_unmodified_since.clone(),
    );
    insert_optional_header(&mut headers, "if-none-match", args.if_none_match.clone());
    insert_optional_header(&mut headers, "if-match", args.if_match.clone());
    insert_optional_header(
        &mut headers,
        "x-persistent-headers",
        args.persistent_headers.clone(),
    );
    insert_object_copy_metadata_directive_header(&mut headers, args.metadata_directive.clone());
    insert_optional_header(
        &mut headers,
        "x-tagging-directive",
        args.tagging_directive.clone(),
    );
    insert_acl_headers(
        &mut headers,
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
        args.tagging.clone(),
    );
    let result = core::execute_object_request(
        client,
        "ve-tos object copy",
        Method::PUT,
        &dst_bucket,
        &dst_key,
        BTreeMap::new(),
        headers,
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_delete(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectDeleteArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    ensure_force_for_destructive(
        global,
        args.force,
        "ve-tos object delete",
        &format!("tos://{bucket}/{key}"),
    )?;
    let query = build_query(&[("versionId", args.version_id.clone())]);
    let mut headers = BTreeMap::new();
    insert_optional_header(&mut headers, "X-From-Modular", args.from_modular.clone());
    insert_optional_header(
        &mut headers,
        "X-If-Match-Expires",
        args.if_match_expires.clone(),
    );
    insert_optional_header(&mut headers, "Last-Modified", args.last_modified.clone());
    insert_optional_header(
        &mut headers,
        "X-If-Match-CreateTime",
        args.if_match_create_time.clone(),
    );
    insert_optional_header(&mut headers, "If-Match", args.if_match.clone());
    insert_optional_header(&mut headers, "X-If-Match-Tags", args.if_match_tags.clone());
    insert_optional_header(
        &mut headers,
        "X-If-Match-AccessTime",
        args.if_match_access_time.clone(),
    );
    if args.lifecycle_directly_delete_versions {
        headers.insert(
            "x-lifecycle-directly-delete-versions".to_string(),
            "true".to_string(),
        );
    }
    insert_optional_header(
        &mut headers,
        "x-if-match-inode-id",
        args.if_match_inode_id.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-parent-inode-id",
        args.parent_inode_id.clone(),
    );
    if args.only_put_delete_marker {
        headers.insert("x-only-put-delete-marker".to_string(), "true".to_string());
    }
    insert_optional_header(
        &mut headers,
        "X-Inner-Properties-TimeStamp",
        args.inner_properties_timestamp.clone(),
    );
    insert_optional_header(
        &mut headers,
        "X-Inner-Properties-TimeStampNsec",
        args.inner_properties_timestamp_nsec.clone(),
    );
    let result = core::execute_object_request(
        client,
        "ve-tos object delete",
        Method::DELETE,
        &bucket,
        &key,
        query,
        headers,
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_batch_delete(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectBatchDeleteArgs,
) -> Result<(), CliError> {
    let bucket = args.bucket.require()?;
    ensure_force_for_destructive(
        global,
        args.force,
        "ve-tos object batch-delete",
        &format!("tos://{}", bucket),
    )?;
    // [Review Fix #6] TOS DeleteMultiObjects API 要求 PascalCase: {"Objects":[{"Key":"..."}], "Quiet":false}
    let body = if args.keys.trim_start().starts_with('[') || args.keys.trim_start().starts_with('{')
    {
        read_json_input(&args.keys)?
    } else {
        json!({ "Objects": args.keys.split(',').map(|item| item.trim()).filter(|item| !item.is_empty()).map(|item| json!({"Key": item})).collect::<Vec<_>>(), "Quiet": false })
    };
    let result = core::execute_bucket_request(
        client,
        "ve-tos object batch-delete",
        Method::POST,
        &bucket,
        build_query(&[
            ("delete", Some(String::new())),
            (
                "queryRecursive",
                args.recursive.then_some("true".to_string()),
            ),
            (
                "querySkipTrash",
                args.skip_trash.then_some("true".to_string()),
            ),
        ]),
        {
            let mut headers = json_headers();
            insert_optional_header(&mut headers, "Content-MD5", args.content_md5.clone());
            headers
        },
        Some(serde_json::to_vec(&body).map_err(CliError::Json)?),
    )
    .await?;
    output_result(global, &result)
}

async fn handle_list(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectListArgs,
) -> Result<(), CliError> {
    let bucket = object_list_bucket(args)?;
    let prefix = object_list_prefix(args)?;
    // [Review Fix #FmtUni-Phase2] Typed handler 替换原 raw API 路径，
    // 与 `ve-tos bucket list` 同构：snake_case 列声明 + Envelope.pagination footer。
    let result = object_domain::list_objects(
        client,
        &bucket,
        prefix.as_deref(),
        args.delimiter.as_deref(),
        args.max_keys,
        args.continuation_token.as_deref(),
    )
    .await?;
    output_result_with_columns(global, &result, Some(OBJECT_LIST_TABLE_COLUMNS))
}

const OBJECT_LIST_TABLE_COLUMNS: &[&str] =
    &["key", "size", "last_modified", "storage_class", "etag"];

async fn handle_list_versions(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectListVersionsArgs,
) -> Result<(), CliError> {
    let bucket = args.bucket.require()?;
    let result =
        object_domain::list_object_versions(client, &bucket, args.prefix.as_deref()).await?;
    output_result_with_columns(global, &result, Some(OBJECT_LIST_VERSIONS_TABLE_COLUMNS))
}

const OBJECT_LIST_VERSIONS_TABLE_COLUMNS: &[&str] = &[
    "key",
    "version_id",
    "size",
    "last_modified",
    "is_latest",
    "storage_class",
];

async fn handle_head(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectHeadArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let query = build_query(&[("versionId", args.version_id.clone())]);
    let mut headers = BTreeMap::new();
    insert_optional_header(
        &mut headers,
        "If-Modified-Since",
        args.if_modified_since.clone(),
    );
    insert_optional_header(
        &mut headers,
        "If-Unmodified-Since",
        args.if_unmodified_since.clone(),
    );
    insert_optional_header(&mut headers, "If-Match", args.if_match.clone());
    insert_optional_header(&mut headers, "If-None-Match", args.if_none_match.clone());
    if let Some(range) = &args.range {
        headers.insert("range".to_string(), range.clone());
    }
    insert_optional_header(
        &mut headers,
        "X-Replicated-From",
        args.replicated_from.clone(),
    );
    insert_optional_header(&mut headers, "X-From-Modular", args.from_modular.clone());
    let result = core::execute_object_request(
        client,
        "ve-tos object head",
        Method::HEAD,
        &bucket,
        &key,
        query,
        headers,
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_stat(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectStatArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let result = core::execute_object_request(
        client,
        "ve-tos object stat",
        Method::GET,
        &bucket,
        &key,
        marker_query(&["stat"]),
        BTreeMap::new(),
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_status(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectStatusArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let result = core::execute_object_request(
        client,
        "ve-tos object status",
        Method::GET,
        &bucket,
        &key,
        marker_query(&["status"]),
        BTreeMap::new(),
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_set_meta(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectSetMetaArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let body = parse_json_or_kv(&args.meta);
    let mut headers = json_headers();
    insert_optional_header(&mut headers, "x-unique-tag", args.unique_tag.clone());
    insert_optional_header(&mut headers, "Content-Type", args.content_type.clone());
    let result = core::execute_object_request(
        client,
        "ve-tos object set-meta",
        Method::POST,
        &bucket,
        &key,
        build_query(&[
            ("metadata", Some(String::new())),
            ("queryVersionID", args.version_id.clone()),
        ]),
        headers,
        Some(serde_json::to_vec(&body).map_err(CliError::Json)?),
    )
    .await?;
    output_result(global, &result)
}

async fn handle_set_time(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectSetTimeArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mut headers = json_headers();
    insert_optional_header(
        &mut headers,
        "x-modify-timestamp",
        args.modify_timestamp.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-modify-timestamp-ns",
        args.modify_timestamp_ns.clone(),
    );
    let result = core::execute_object_request(
        client,
        "ve-tos object set-time",
        Method::POST,
        &bucket,
        &key,
        marker_query(&["time"]),
        headers,
        Some(serde_json::to_vec(&json!({ "time": args.time })).map_err(CliError::Json)?),
    )
    .await?;
    output_result(global, &result)
}

async fn handle_set_expires(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectSetExpiresArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let result = core::execute_object_request(
        client,
        "ve-tos object set-expires",
        Method::POST,
        &bucket,
        &key,
        build_query(&[
            ("objectExpires", Some(String::new())),
            ("queryVersionID", args.version_id.clone()),
        ]),
        json_headers(),
        Some(serde_json::to_vec(&json!({ "expires": args.expires })).map_err(CliError::Json)?),
    )
    .await?;
    output_result(global, &result)
}

async fn handle_append(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectAppendArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    // [Review Fix #M4] Stream file-path bodies through the V4 streaming
    // pipeline; stdin/inline bytes stay on the buffered fast path.
    let body_input = classify_body_input(&args.body)?;
    let mut query = marker_query(&["append"]);
    query.insert("offset".to_string(), args.offset.to_string());
    insert_optional_query(
        &mut query,
        "append-last-time",
        args.append_last_time.clone(),
    );
    insert_optional_query(&mut query, "queryVersionID", args.version_id.clone());
    let mut headers = BTreeMap::new();
    insert_optional_header(&mut headers, "Content-Type", args.content_type.clone());
    insert_optional_header(&mut headers, "Content-MD5", args.content_md5.clone());
    insert_optional_header(
        &mut headers,
        "x-content-sha256",
        args.content_sha256.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-decoded-content-length",
        args.decoded_content_length.map(|value| value.to_string()),
    );
    insert_optional_header(
        &mut headers,
        "x-object-lock-mode",
        args.object_lock_mode.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-object-lock-retain-until-date",
        args.object_lock_retain_until_date.clone(),
    );
    insert_acl_headers(
        &mut headers,
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
        None,
    );
    insert_optional_header(
        &mut headers,
        "x-persistent-headers",
        args.persistent_headers.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-traffic-limit",
        args.traffic_limit.map(|value| value.to_string()),
    );
    insert_optional_header(&mut headers, "if-none-match", args.if_none_match.clone());
    insert_optional_header(&mut headers, "x-if-match", args.if_match.clone());
    insert_optional_header(&mut headers, "if-match", args.if_match.clone());
    match body_input {
        BodyInput::FilePath { path, len } => {
            let path_str = path.to_string_lossy().to_string();
            // [Review Fix] TOS requires Content-Length for streaming append bodies
            headers.insert("content-length".to_string(), len.to_string());
            let payload_hash = crate::handler::high_level::file_sha256(&path_str)?;
            let body = crate::handler::high_level::file_stream_body(&path_str).await?;
            let result = core::execute_object_streaming_request(
                client,
                "ve-tos object append",
                Method::POST,
                &bucket,
                &key,
                query,
                headers,
                payload_hash,
                body,
            )
            .await?;
            output_result(global, &result)
        }
        BodyInput::Inline(bytes) => {
            let result = core::execute_object_request(
                client,
                "ve-tos object append",
                Method::POST,
                &bucket,
                &key,
                query,
                headers,
                Some(bytes),
            )
            .await?;
            output_result(global, &result)
        }
    }
}

async fn handle_seal_append(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectSealAppendArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mut query = marker_query(&["seal"]);
    insert_optional_query(
        &mut query,
        "offset",
        args.offset.map(|value| value.to_string()),
    );
    insert_optional_query(&mut query, "queryVersionID", args.version_id.clone());
    let mut headers = BTreeMap::new();
    insert_acl_headers(
        &mut headers,
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
        None,
    );
    insert_optional_header(&mut headers, "if-none-match", args.if_none_match.clone());
    insert_optional_header(&mut headers, "x-if-match", args.if_match.clone());
    insert_optional_header(&mut headers, "if-match", args.if_match.clone());
    let result = core::execute_object_request(
        client,
        "ve-tos object seal-append",
        Method::POST,
        &bucket,
        &key,
        query,
        headers,
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_modify(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectModifyArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    // [Review Fix #M4] Stream file-path bodies through the V4 streaming
    // pipeline; stdin/inline bytes stay on the buffered fast path.
    let body_input = classify_body_input(&args.body)?;
    let mut query = marker_query(&["modify"]);
    query.insert("offset".to_string(), args.offset.to_string());
    insert_optional_query(&mut query, "queryVersionID", args.version_id.clone());
    let mut headers = BTreeMap::new();
    insert_optional_header(&mut headers, "Content-Type", args.content_type.clone());
    insert_optional_header(&mut headers, "Content-MD5", args.content_md5.clone());
    insert_optional_header(
        &mut headers,
        "x-traffic-limit",
        args.traffic_limit.map(|value| value.to_string()),
    );
    insert_optional_header(&mut headers, "if-none-match", args.if_none_match.clone());
    insert_optional_header(&mut headers, "x-if-match", args.if_match.clone());
    insert_optional_header(&mut headers, "if-match", args.if_match.clone());
    match body_input {
        BodyInput::FilePath { path, len } => {
            let path_str = path.to_string_lossy().to_string();
            // [Review Fix #HNS-MODIFY-1] ModifyObject rejects chunked streaming;
            // send the same fixed Content-Length used by append/upload streams.
            headers.insert("content-length".to_string(), len.to_string());
            let payload_hash = crate::handler::high_level::file_sha256(&path_str)?;
            let body = crate::handler::high_level::file_stream_body(&path_str).await?;
            let result = core::execute_object_streaming_request(
                client,
                "ve-tos object modify",
                Method::POST,
                &bucket,
                &key,
                query,
                headers,
                payload_hash,
                body,
            )
            .await?;
            output_result(global, &result)
        }
        BodyInput::Inline(bytes) => {
            let result = core::execute_object_request(
                client,
                "ve-tos object modify",
                Method::POST,
                &bucket,
                &key,
                query,
                headers,
                Some(bytes),
            )
            .await?;
            output_result(global, &result)
        }
    }
}

async fn handle_rename(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectRenameArgs,
) -> Result<(), CliError> {
    let (src_bucket, src_key) = parse_object_target(Some(&args.source), None, None)?;
    let (dst_bucket, dst_key) = parse_object_target(Some(&args.destination), None, None)?;
    if src_bucket != dst_bucket {
        // [Review Fix #5] RenameObject 仅支持桶内重命名，目标桶与源桶不一致时必须显式拒绝，避免静默忽略目标桶。
        return Err(CliError::ValidationError(format!(
            "rename destination bucket must match source bucket: source={src_bucket}, destination={dst_bucket}"
        )));
    }
    let mut headers = BTreeMap::new();
    if args.recursive_mkdir {
        headers.insert("x-tos-recursive-mkdir".to_string(), "true".to_string());
    }
    if args.not_update_timestamp {
        headers.insert("x-not-update-timestamp".to_string(), "true".to_string());
    }
    if args.forbid_overwrite {
        headers.insert("x-tos-forbid-overwrite".to_string(), "true".to_string());
    }
    insert_optional_header(&mut headers, "X-Tracer-Traceid", args.trace_id.clone());
    let mut query = BTreeMap::new();
    query.insert("rename".to_string(), String::new());
    query.insert("name".to_string(), dst_key.to_string());
    let result = core::execute_object_request(
        client,
        "ve-tos object rename",
        Method::PUT,
        &src_bucket,
        &src_key,
        query,
        headers,
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_restore(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectRestoreArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mut headers = json_headers();
    insert_optional_header(&mut headers, "Content-MD5", args.content_md5.clone());
    let result = core::execute_object_request(
        client,
        "ve-tos object restore",
        Method::POST,
        &bucket,
        &key,
        build_query(&[
            ("restore", Some(String::new())),
            ("queryVersionID", args.version_id.clone()),
        ]),
        headers,
        Some(serde_json::to_vec(&json!({ "days": args.days })).map_err(CliError::Json)?),
    )
    .await?;
    output_result(global, &result)
}

async fn handle_get_acl(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectGetAclArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mut query = marker_query(&["acl"]);
    insert_optional_query(&mut query, "queryVersionID", args.version_id.clone());
    let result = core::execute_object_request(
        client,
        "ve-tos object get-acl",
        Method::GET,
        &bucket,
        &key,
        query,
        BTreeMap::new(),
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_set_acl(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectSetAclArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mut headers = BTreeMap::new();
    headers.insert("x-tos-acl".to_string(), args.acl.clone());
    let result = core::execute_object_request(
        client,
        "ve-tos object set-acl",
        Method::PUT,
        &bucket,
        &key,
        build_query(&[
            ("acl", Some(String::new())),
            ("queryVersionID", args.version_id.clone()),
        ]),
        headers,
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_get_tagging(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectGetTaggingArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mut query = marker_query(&["tagging"]);
    insert_optional_query(&mut query, "queryVersionID", args.version_id.clone());
    let result = core::execute_object_request(
        client,
        "ve-tos object get-tagging",
        Method::GET,
        &bucket,
        &key,
        query,
        BTreeMap::new(),
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_set_tagging(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectSetTaggingArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    execute_json_object_action(
        global,
        client,
        "ve-tos object set-tagging",
        Method::PUT,
        &bucket,
        &key,
        marker_query(&["tagging"]),
        parse_json_or_kv(&args.tags),
    )
    .await
}

async fn handle_delete_tagging(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectDeleteTaggingArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    // [Review Fix #2] Keep interactive prompting and non-interactive exact
    // confirmation behavior aligned with `ve-tos object delete`.
    ensure_force_for_destructive(
        global,
        args.force,
        "ve-tos object delete-tagging",
        &format!("tos://{bucket}/{key}"),
    )?;
    let result = core::execute_object_request(
        client,
        "ve-tos object delete-tagging",
        Method::DELETE,
        &bucket,
        &key,
        marker_query(&["tagging"]),
        BTreeMap::new(),
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_link(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectLinkArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mut headers = BTreeMap::new();
    headers.insert("x-link-target".to_string(), args.source_key.clone());
    insert_acl_headers(
        &mut headers,
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
        args.tagging.clone(),
    );
    let result = core::execute_object_request(
        client,
        "ve-tos object link",
        Method::PUT,
        &bucket,
        &key,
        marker_query(&["link"]),
        headers,
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_get_symlink(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectGetSymlinkArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let result = core::execute_object_request(
        client,
        "ve-tos object get-symlink",
        Method::GET,
        &bucket,
        &key,
        build_query(&[
            ("symlink", Some(String::new())),
            ("queryVersionID", args.version_id.clone()),
        ]),
        BTreeMap::new(),
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_create_symlink(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectCreateSymlinkArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(None, args.bucket.as_deref(), args.key.as_deref())?;
    let mut headers = BTreeMap::new();
    headers.insert("x-symlink-target".to_string(), args.target_key.clone());
    insert_optional_header(&mut headers, "x-symlink-bucket", args.target_bucket.clone());
    insert_acl_headers(
        &mut headers,
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
        args.tagging.clone(),
    );
    let result = core::execute_object_request(
        client,
        "ve-tos object create-symlink",
        Method::PUT,
        &bucket,
        &key,
        marker_query(&["symlink"]),
        headers,
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_get_fetch_task(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectGetFetchTaskArgs,
) -> Result<(), CliError> {
    let bucket = args.bucket.require()?;
    let result = core::execute_bucket_request(
        client,
        "ve-tos object get-fetch-task",
        Method::GET,
        &bucket,
        build_query(&[
            ("fetchTask", Some(String::new())),
            ("taskId", args.task_id.clone()),
        ]),
        BTreeMap::new(),
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_create_fetch_task(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectCreateFetchTaskArgs,
) -> Result<(), CliError> {
    let body = json!({
        "URL": args.source_url,
        "Object": args.key,
    });
    let mut headers = json_headers();
    insert_optional_header(&mut headers, "x-etag-pattern", args.etag_pattern.clone());
    insert_optional_header(
        &mut headers,
        "x-traffic-limit",
        args.traffic_limit.map(|v| v.to_string()),
    );
    insert_optional_header(
        &mut headers,
        "x-if-unmodified-since",
        args.if_unmodified_since.clone(),
    );
    insert_optional_header(&mut headers, "if-none-match", args.if_none_match.clone());
    insert_optional_header(&mut headers, "if-match", args.if_match.clone());
    insert_optional_header(
        &mut headers,
        "x-object-lock-mode",
        args.object_lock_mode.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-object-lock-retain-until-date",
        args.object_lock_retain_until_date.clone(),
    );
    insert_acl_headers(
        &mut headers,
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
        None,
    );
    let result = core::execute_bucket_request(
        client,
        "ve-tos object create-fetch-task",
        Method::POST,
        &args.bucket.require()?,
        marker_query(&["fetchTask"]),
        headers,
        Some(serde_json::to_vec(&body).map_err(CliError::Json)?),
    )
    .await?;
    output_result(global, &result)
}

async fn handle_fetch(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectFetchArgs,
) -> Result<(), CliError> {
    // [Review Fix #8] FetchObject body 需要包含可选的 StorageClass 和 Meta 字段
    let mut body = serde_json::Map::new();
    body.insert("URL".to_string(), json!(args.source_url));
    if let Some(ref sc) = args.storage_class {
        body.insert("StorageClass".to_string(), json!(sc));
    }
    if let Some(ref meta) = args.meta {
        body.insert("Meta".to_string(), json!(meta));
    }
    execute_json_object_action(
        global,
        client,
        "ve-tos object fetch",
        Method::POST,
        &args.bucket.require()?,
        &args.key,
        marker_query(&["fetch"]),
        serde_json::Value::Object(body),
    )
    .await
}

async fn handle_set_retention(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectSetRetentionArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mode = normalize_object_retention_mode(&args.mode)?;
    let body = build_object_retention_body(&mode, &args.retain_until_date)?;
    let mut headers =
        BTreeMap::from([("Content-Type".to_string(), "application/json".to_string())]);
    headers.insert(
        "Content-MD5".to_string(),
        args.content_md5
            .clone()
            .unwrap_or_else(|| content_md5_base64(&body)),
    );
    let result = core::execute_object_request(
        client,
        "ve-tos object set-retention",
        Method::PUT,
        &bucket,
        &key,
        build_query(&[
            ("retention", Some(String::new())),
            ("versionId", args.version_id.clone()),
        ]),
        headers,
        Some(body),
    )
    .await?;
    output_result(global, &result)
}

fn build_object_retention_body(mode: &str, retain_until_date: &str) -> Result<Vec<u8>, CliError> {
    // [Review Fix #RetentionBody] The TOS PutObjectRetention API documents a
    // top-level JSON body. The service also requires a matching Content-MD5,
    // so callers build the body first and derive the checksum from these bytes.
    serde_json::to_vec(&json!({
        "Mode": mode,
        "RetainUntilDate": retain_until_date,
    }))
    .map_err(CliError::Json)
}

fn normalize_object_retention_mode(mode: &str) -> Result<String, CliError> {
    let normalized = mode.trim().to_ascii_uppercase();
    if normalized == "COMPLIANCE" {
        return Ok(normalized);
    }
    Err(CliError::ValidationError(
        // [Review Fix #RetentionMode] The TOS PutObjectRetention API only
        // documents COMPLIANCE; rejecting other modes locally avoids opaque
        // MalformedBody responses from the service.
        "ve-tos object set-retention only supports --mode COMPLIANCE".to_string(),
    ))
}

fn content_md5_base64(body: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine;

    let digest = md5::compute(body);
    BASE64_STANDARD.encode(digest.0)
}

async fn handle_get_retention(
    global: &GlobalArgs,
    client: &TosClient,
    args: &ObjectGetRetentionArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let result = core::execute_object_request(
        client,
        "ve-tos object get-retention",
        Method::GET,
        &bucket,
        &key,
        build_query(&[
            ("retention", Some(String::new())),
            ("versionId", args.version_id.clone()),
        ]),
        BTreeMap::new(),
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn execute_json_object_action(
    global: &GlobalArgs,
    client: &TosClient,
    command: &str,
    method: Method,
    bucket: &str,
    key: &str,
    query: BTreeMap<String, String>,
    body: Value,
) -> Result<(), CliError> {
    let result = core::execute_object_request(
        client,
        command,
        method,
        bucket,
        key,
        query,
        json_headers(),
        Some(serde_json::to_vec(&body).map_err(CliError::Json)?),
    )
    .await?;
    output_result(global, &result)
}

fn object_list_bucket(args: &ObjectListArgs) -> Result<String, CliError> {
    if let Some(uri) = &args.uri {
        if args.bucket.is_some() || args.prefix.is_some() {
            return Err(CliError::ValidationError(
                "object list target styles cannot be mixed: use either tos://bucket[/prefix] or --bucket <bucket> [--prefix <prefix>]".into(),
            ));
        }
        let (bucket, _) = parse_object_list_uri(uri)?;
        return Ok(bucket);
    }
    let bucket = args.bucket.clone().ok_or_else(|| {
        CliError::ValidationError(
            "missing bucket for object list: provide tos://bucket[/prefix] or --bucket".into(),
        )
    })?;
    validate_bucket_flag_target(&bucket)
}

fn object_list_prefix(args: &ObjectListArgs) -> Result<Option<String>, CliError> {
    if let Some(uri) = &args.uri {
        let (_, prefix) = parse_object_list_uri(uri)?;
        return Ok(prefix);
    }
    Ok(args.prefix.clone())
}

fn object_list_target(args: &ObjectListArgs) -> Result<String, CliError> {
    let bucket = object_list_bucket(args)?;
    if let Some(prefix) = object_list_prefix(args)? {
        if !prefix.is_empty() {
            return Ok(format!(
                "tos://{}/{}",
                bucket,
                prefix.trim_start_matches('/')
            ));
        }
    }
    Ok(format!("tos://{}", bucket))
}

fn parse_object_list_uri(uri: &str) -> Result<(String, Option<String>), CliError> {
    let value = uri.trim();
    let Some(rest) = value.strip_prefix("tos://") else {
        return Err(CliError::ValidationError(format!(
            "invalid object list URI '{}': expected tos://bucket[/prefix]",
            uri
        )));
    };
    let (bucket, prefix) = match rest.split_once('/') {
        Some((bucket, prefix)) => (bucket, (!prefix.is_empty()).then(|| prefix.to_string())),
        None => (rest, None),
    };
    if bucket.is_empty() {
        return Err(CliError::ValidationError(format!(
            "invalid object list URI '{}': missing bucket name",
            uri
        )));
    }
    Ok((bucket.to_string(), prefix))
}

fn object_write_headers(
    content_type: Option<&str>,
    storage_class: Option<&str>,
    meta: Option<&str>,
    net_speed_test: Option<&str>,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    if let Some(content_type) = content_type {
        headers.insert("content-type".to_string(), content_type.to_string());
    }
    if let Some(storage_class) = storage_class {
        headers.insert("x-tos-storage-class".to_string(), storage_class.to_string());
    }
    if let Some(meta) = meta {
        if let Value::Object(map) = parse_kv_pairs(meta) {
            for (key, value) in map {
                headers.insert(
                    format!("x-tos-meta-{}", key),
                    value.as_str().unwrap_or_default().to_string(),
                );
            }
        }
    }
    if let Some(net_speed_test) = net_speed_test {
        headers.insert(
            "x-tos-net-speed-test".to_string(),
            net_speed_test.to_string(),
        );
    }
    headers
}

fn insert_optional_header(
    headers: &mut BTreeMap<String, String>,
    key: &str,
    value: Option<String>,
) {
    if let Some(value) = value {
        headers.insert(key.to_string(), value);
    }
}

fn insert_object_copy_metadata_directive_header(
    headers: &mut BTreeMap<String, String>,
    value: Option<String>,
) {
    // [Review Fix #4] Both tos-rust-sdk and ve-tos-rust-sdk define
    // CopyObject metadata directive as x-tos-metadata-directive.
    insert_optional_header(headers, "x-tos-metadata-directive", value);
}

fn insert_optional_query(query: &mut BTreeMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        query.insert(key.to_string(), value);
    }
}

fn insert_acl_headers(
    headers: &mut BTreeMap<String, String>,
    acl: Option<String>,
    grant_full_control: Option<String>,
    grant_read: Option<String>,
    grant_read_non_list: Option<String>,
    grant_read_acp: Option<String>,
    grant_write: Option<String>,
    grant_write_acp: Option<String>,
    tagging: Option<String>,
) {
    if let Some(acl) = acl {
        // [Review Fix #ObjectHeaderCanonical] ACL-related object headers use
        // canonical `x-tos-*` names instead of sending legacy duplicates.
        headers.insert("x-tos-acl".to_string(), acl);
    }
    insert_optional_header(headers, "x-tos-grant-full-control", grant_full_control);
    insert_optional_header(headers, "x-tos-grant-read", grant_read);
    insert_optional_header(headers, "x-tos-grant-read-non-list", grant_read_non_list);
    insert_optional_header(headers, "x-tos-grant-read-acp", grant_read_acp);
    insert_optional_header(headers, "x-tos-grant-write", grant_write);
    insert_optional_header(headers, "x-tos-grant-write-acp", grant_write_acp);
    insert_optional_header(headers, "x-tagging", tagging);
}

fn parameter(
    name: &str,
    location: ParameterLocation,
    required: bool,
    description: &str,
) -> CommandParameter {
    CommandParameter {
        name: name.to_string(),
        location,
        required,
        description: description.to_string(),
        ..Default::default()
    }
}

fn json_headers() -> BTreeMap<String, String> {
    BTreeMap::from([("content-type".to_string(), "application/json".to_string())])
}

fn parse_json_or_kv(input: &str) -> Value {
    read_json_input(input).unwrap_or_else(|_| parse_kv_pairs(input))
}

fn dry_run_object_action(action: &ObjectAction) -> Result<DryRunResult, CliError> {
    if let ObjectAction::Upload(args) = action {
        let target = object_target_preview(
            args.uri.as_deref(),
            args.bucket.as_deref(),
            args.key.as_deref(),
        )?;
        crate::handler::high_level::ensure_tos_upload_storage_class_supported(
            "ve-tos object upload",
            None,
            &target,
            args.storage_class.as_deref(),
        )?;
    }
    let (command, method, target, risk_level, has_body) = object_dry_run_meta(action)?;
    Ok(DryRunResult {
        action: command.to_string(),
        dry_run: true,
        impact: Impact {
            // [Review Fix #3] Critical object mutations affect an object too;
            // keep dry-run impact consistent with high-risk operations.
            affected_objects: if matches!(risk_level, "high" | "critical") {
                1
            } else {
                0
            },
            affected_bytes: 0,
            risk_level: risk_level.to_string(),
            estimated_duration: Some("< 1s".to_string()),
            scanned_count: None,
            preview_truncated: None,
        },
        plan: vec![format!("{} {}", method, target)],
        warnings: if has_body {
            vec!["Request body is omitted from dry-run output; validate your input before execution.".to_string()]
        } else {
            vec![]
        },
        confirm_command: None,
    })
}

fn object_dry_run_meta(
    action: &ObjectAction,
) -> Result<(&'static str, &'static str, String, &'static str, bool), CliError> {
    Ok(match action {
        ObjectAction::Upload(args) => (
            "ve-tos object upload",
            "PUT",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        ObjectAction::Download(args) => (
            "ve-tos object download",
            "GET",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "low",
            false,
        ),
        ObjectAction::FormUpload(args) => (
            "ve-tos object form-upload",
            "POST",
            // [Review Fix #TOS-FormUploadDryRun] form-upload targets an object,
            // so dry-run must accept tos://bucket/key just like execution.
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        ObjectAction::Copy(args) => (
            "ve-tos object copy",
            "PUT",
            args.destination.clone(),
            "medium",
            false,
        ),
        ObjectAction::Delete(args) => (
            "ve-tos object delete",
            "DELETE",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "high",
            false,
        ),
        ObjectAction::BatchDelete(args) => (
            "ve-tos object batch-delete",
            "POST",
            format!("tos://{}", args.bucket.require()?),
            "high",
            true,
        ),
        ObjectAction::List(args) => (
            "ve-tos object list",
            "GET",
            object_list_target(args)?,
            "low",
            false,
        ),
        ObjectAction::ListVersions(args) => (
            "ve-tos object list-versions",
            "GET",
            format!("tos://{}", args.bucket.require()?),
            "low",
            false,
        ),
        ObjectAction::Head(args) => (
            "ve-tos object head",
            "HEAD",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "low",
            false,
        ),
        ObjectAction::Stat(args) => (
            "ve-tos object stat",
            "GET",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "low",
            false,
        ),
        ObjectAction::Status(args) => (
            "ve-tos object status",
            "GET",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "low",
            false,
        ),
        ObjectAction::SetMeta(args) => (
            "ve-tos object set-meta",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        ObjectAction::SetTime(args) => (
            "ve-tos object set-time",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        ObjectAction::SetExpires(args) => (
            "ve-tos object set-expires",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        ObjectAction::Append(args) => (
            "ve-tos object append",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        ObjectAction::SealAppend(args) => (
            "ve-tos object seal-append",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            false,
        ),
        ObjectAction::Modify(args) => (
            "ve-tos object modify",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        ObjectAction::Rename(args) => (
            "ve-tos object rename",
            "PUT",
            args.source.clone(),
            "medium",
            true,
        ),
        ObjectAction::Restore(args) => (
            "ve-tos object restore",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        ObjectAction::GetAcl(args) => (
            "ve-tos object get-acl",
            "GET",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "low",
            false,
        ),
        ObjectAction::SetAcl(args) => (
            "ve-tos object set-acl",
            "PUT",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        ObjectAction::GetTagging(args) => (
            "ve-tos object get-tagging",
            "GET",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "low",
            false,
        ),
        ObjectAction::SetTagging(args) => (
            "ve-tos object set-tagging",
            "PUT",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        ObjectAction::DeleteTagging(args) => (
            "ve-tos object delete-tagging",
            "DELETE",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "critical",
            false,
        ),
        ObjectAction::Link(args) => (
            "ve-tos object link",
            "PUT",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            false,
        ),
        ObjectAction::GetSymlink(args) => (
            "ve-tos object get-symlink",
            "GET",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "low",
            false,
        ),
        ObjectAction::CreateSymlink(args) => (
            "ve-tos object create-symlink",
            "PUT",
            object_target_preview(None, args.bucket.as_deref(), args.key.as_deref())?,
            "medium",
            false,
        ),
        ObjectAction::GetFetchTask(args) => (
            "ve-tos object get-fetch-task",
            "GET",
            format!("tos://{}", args.bucket.require()?),
            "low",
            false,
        ),
        ObjectAction::CreateFetchTask(args) => (
            "ve-tos object create-fetch-task",
            "POST",
            format!("tos://{}", args.bucket.require()?),
            "medium",
            true,
        ),
        ObjectAction::Fetch(args) => (
            "ve-tos object fetch",
            "POST",
            format!("tos://{}/{}", args.bucket.require()?, args.key),
            "medium",
            true,
        ),
        ObjectAction::SetRetention(args) => (
            "ve-tos object set-retention",
            "PUT",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "high",
            true,
        ),
        ObjectAction::GetRetention(args) => (
            "ve-tos object get-retention",
            "GET",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "low",
            false,
        ),
    })
}

fn object_target_preview(
    uri: Option<&str>,
    bucket: Option<&str>,
    key: Option<&str>,
) -> Result<String, CliError> {
    let (bucket, key) = parse_object_target(uri, bucket, key)?;
    Ok(format!("tos://{}/{}", bucket, key))
}

fn describe_object_action(action: &ObjectAction) -> CommandDescription {
    let (command, api, description, risk, pipe) = match action {
        ObjectAction::Upload(_) => (
            "ve-tos object upload",
            "PutObject",
            "Upload a single object",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::Download(_) => (
            "ve-tos object download",
            "GetObject",
            "Download a single object",
            RiskLevel::Low,
            true,
        ),
        ObjectAction::FormUpload(_) => (
            "ve-tos object form-upload",
            "PostObject",
            "Upload an object with POST semantics",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::Copy(_) => (
            "ve-tos object copy",
            "CopyObject",
            "Server-side object copy",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::Delete(_) => (
            "ve-tos object delete",
            "DeleteObject",
            "Delete one object or a specific version (requires --force for execution)",
            RiskLevel::High,
            false,
        ),
        ObjectAction::BatchDelete(_) => (
            "ve-tos object batch-delete",
            "DeleteObjects",
            "Delete multiple objects in one request (requires --force for execution)",
            RiskLevel::High,
            false,
        ),
        ObjectAction::List(_) => (
            "ve-tos object list",
            "ListBucket",
            "List objects in a bucket",
            RiskLevel::Low,
            true,
        ),
        ObjectAction::ListVersions(_) => (
            "ve-tos object list-versions",
            "ListBucketVersions",
            "List object versions",
            RiskLevel::Low,
            true,
        ),
        ObjectAction::Head(_) => (
            "ve-tos object head",
            "HeadObject",
            "Read object headers only",
            RiskLevel::Low,
            false,
        ),
        ObjectAction::Stat(_) => (
            "ve-tos object stat",
            "GetFileStatus",
            "Get object file status",
            RiskLevel::Low,
            false,
        ),
        ObjectAction::SetMeta(_) => (
            "ve-tos object set-meta",
            "SetObjectMeta",
            "Set object metadata",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::SetTime(_) => (
            "ve-tos object set-time",
            "SetObjectTime",
            "Set object time attributes",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::SetExpires(_) => (
            "ve-tos object set-expires",
            "SetObjectExpires",
            "Set object expiration",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::Append(_) => (
            "ve-tos object append",
            "AppendObject",
            "Append data to an appendable object",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::SealAppend(_) => (
            "ve-tos object seal-append",
            "SealAppendObject",
            "Seal an appendable object",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::Modify(_) => (
            "ve-tos object modify",
            "ModifyObject",
            "Modify object content at an offset",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::Rename(_) => (
            "ve-tos object rename",
            "RenameObject",
            "Rename an object",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::Restore(_) => (
            "ve-tos object restore",
            "RestoreObject",
            "Restore an archived object",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::Status(_) => (
            "ve-tos object status",
            "GetFileStatus",
            "Get object processing status alias",
            RiskLevel::Low,
            false,
        ),
        ObjectAction::GetAcl(_) => (
            "ve-tos object get-acl",
            "GetObjectAcl",
            "Get object ACL",
            RiskLevel::Low,
            false,
        ),
        ObjectAction::SetAcl(_) => (
            "ve-tos object set-acl",
            "PutObjectAcl",
            "Set object ACL",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::GetTagging(_) => (
            "ve-tos object get-tagging",
            "GetObjectTagging",
            "Get object tagging",
            RiskLevel::Low,
            false,
        ),
        ObjectAction::SetTagging(_) => (
            "ve-tos object set-tagging",
            "PutObjectTagging",
            "Set object tagging",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::DeleteTagging(_) => (
            "ve-tos object delete-tagging",
            "DeleteObjectTagging",
            "Delete object tagging",
            RiskLevel::Critical,
            true,
        ),
        ObjectAction::Link(_) => (
            "ve-tos object link",
            "PutLink",
            "Create a hard link-like object",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::GetSymlink(_) => (
            "ve-tos object get-symlink",
            "GetSymlink",
            "Get symlink target",
            RiskLevel::Low,
            false,
        ),
        ObjectAction::CreateSymlink(_) => (
            "ve-tos object create-symlink",
            "PutSymlink",
            "Create a symbolic link",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::GetFetchTask(_) => (
            "ve-tos object get-fetch-task",
            "GetFetchTask",
            "Get fetch task details",
            RiskLevel::Low,
            false,
        ),
        ObjectAction::CreateFetchTask(_) => (
            "ve-tos object create-fetch-task",
            "PutFetchTask",
            "Create a fetch task",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::Fetch(_) => (
            "ve-tos object fetch",
            "FetchObject",
            "Fetch an external object into TOS",
            RiskLevel::Medium,
            false,
        ),
        ObjectAction::SetRetention(_) => (
            "ve-tos object set-retention",
            "PutObjectRetention",
            "Set object retention",
            RiskLevel::High,
            false,
        ),
        ObjectAction::GetRetention(_) => (
            "ve-tos object get-retention",
            "GetObjectRetention",
            "Get object retention",
            RiskLevel::Low,
            false,
        ),
    };

    let scenario_routing = match action {
        ObjectAction::Delete(_) => HashMap::from([
            (
                "Confirm delete (requires --force + --confirm)".to_string(),
                "ve-tos object delete tos://bucket/key --force --confirm tos://bucket/key"
                    .to_string(),
            ),
            (
                "Preview delete".to_string(),
                "ve-tos object delete tos://bucket/key --dry-run".to_string(),
            ),
        ]),
        ObjectAction::BatchDelete(_) => HashMap::from([
            (
                "Confirm batch delete (requires --force)".to_string(),
                "ve-tos object batch-delete --bucket my-bucket --keys a.txt,b.txt --force"
                    .to_string(),
            ),
            (
                "Preview batch delete".to_string(),
                "ve-tos object batch-delete --bucket my-bucket --keys a.txt,b.txt --dry-run"
                    .to_string(),
            ),
        ]),
        ObjectAction::DeleteTagging(_) => HashMap::from([
            (
                "Confirm delete tagging (requires --force + --confirm)".to_string(),
                "ve-tos object delete-tagging tos://bucket/key --force --confirm tos://bucket/key"
                    .to_string(),
            ),
            (
                "Preview delete tagging".to_string(),
                "ve-tos object delete-tagging tos://bucket/key --dry-run".to_string(),
            ),
        ]),
        _ => HashMap::from([
            ("English Example".to_string(), format!("{} --help", command)),
            (
                "Describe Example".to_string(),
                format!("{} --describe", command),
            ),
        ]),
    };

    let parameters = match action {
        ObjectAction::Upload(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter("body", ParameterLocation::Body, true, "Upload body source"),
            parameter(
                "content-type",
                ParameterLocation::Header,
                false,
                "Content type",
            ),
            parameter(
                "x-tos-storage-class",
                ParameterLocation::Header,
                false,
                "Storage class for ve-tos uploads; ByteTOS PutObject upload rejects this override",
            ),
            parameter(
                "x-tos-meta-*",
                ParameterLocation::Header,
                false,
                "Custom metadata headers",
            ),
            parameter(
                "x-tos-net-speed-test",
                ParameterLocation::Header,
                false,
                "Net speed test marker",
            ),
        ]),
        ObjectAction::FormUpload(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter(
                "x-tos-object-key",
                ParameterLocation::Header,
                true,
                "Uploaded object key",
            ),
            parameter("body", ParameterLocation::Body, true, "Form upload body"),
            parameter(
                "content-type",
                ParameterLocation::Header,
                false,
                "Content type",
            ),
            parameter(
                "x-tos-storage-class",
                ParameterLocation::Header,
                false,
                "Storage class",
            ),
            parameter(
                "x-tos-meta-*",
                ParameterLocation::Header,
                false,
                "Custom metadata headers",
            ),
        ]),
        ObjectAction::Copy(_) => Some(vec![
            parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Destination bucket",
            ),
            parameter(
                "object",
                ParameterLocation::Path,
                true,
                "Destination object key",
            ),
            parameter(
                "x-tos-copy-source",
                ParameterLocation::Header,
                true,
                "Source object path /{sourceBucket}/{sourceObject}",
            ),
            parameter("range", ParameterLocation::Header, false, "Copy range"),
            parameter(
                "x-tos-copy-source-if-modified-since",
                ParameterLocation::Header,
                false,
                "Source modified-since condition",
            ),
            parameter(
                "x-tos-copy-source-if-unmodified-since",
                ParameterLocation::Header,
                false,
                "Source unmodified-since condition",
            ),
            parameter(
                "x-etag-pattern",
                ParameterLocation::Header,
                false,
                "ETag pattern hint",
            ),
            parameter(
                "x-unique-tag",
                ParameterLocation::Header,
                false,
                "Unique tag",
            ),
            parameter(
                "x-tos-copy-source-last-modified",
                ParameterLocation::Header,
                false,
                "Copy source last modified",
            ),
            parameter("x-data-id", ParameterLocation::Header, false, "Data ID"),
            parameter(
                "x-finger-print",
                ParameterLocation::Header,
                false,
                "Fingerprint",
            ),
            parameter(
                "x-internal-metadata-directive",
                ParameterLocation::Header,
                false,
                "Internal metadata directive",
            ),
            parameter(
                "x-crr-source-timestamp-nsec",
                ParameterLocation::Header,
                false,
                "CRR source timestamp (ns)",
            ),
            parameter("x-crr-proxy", ParameterLocation::Header, false, "CRR proxy"),
            parameter(
                "x-crr-source-bucket-version-status",
                ParameterLocation::Header,
                false,
                "CRR source bucket version status",
            ),
            parameter(
                "x-tos-metadata-directive",
                ParameterLocation::Header,
                false,
                "Metadata directive",
            ),
            parameter(
                "x-tagging-directive",
                ParameterLocation::Header,
                false,
                "Tagging directive",
            ),
            parameter(
                "x-traffic-limit",
                ParameterLocation::Header,
                false,
                "Traffic limit",
            ),
            parameter(
                "x-object-lock-mode",
                ParameterLocation::Header,
                false,
                "Object lock mode",
            ),
            parameter(
                "x-object-lock-retain-until-date",
                ParameterLocation::Header,
                false,
                "Object lock retain-until date",
            ),
            parameter(
                "x-if-unmodified-since",
                ParameterLocation::Header,
                false,
                "Unmodified-since condition",
            ),
            parameter(
                "if-none-match",
                ParameterLocation::Header,
                false,
                "If-None-Match condition",
            ),
            parameter(
                "if-match",
                ParameterLocation::Header,
                false,
                "If-Match condition",
            ),
            parameter(
                "x-persistent-headers",
                ParameterLocation::Header,
                false,
                "Persistent headers list",
            ),
            parameter("x-tagging", ParameterLocation::Header, false, "Object tags"),
            parameter("x-tos-acl", ParameterLocation::Header, false, "ACL value"),
            parameter(
                "x-tos-grant-full-control",
                ParameterLocation::Header,
                false,
                "Grant full control",
            ),
            parameter(
                "x-tos-grant-read",
                ParameterLocation::Header,
                false,
                "Grant read permission",
            ),
            parameter(
                "x-tos-grant-read-non-list",
                ParameterLocation::Header,
                false,
                "Grant read without list permission",
            ),
            parameter(
                "x-tos-grant-read-acp",
                ParameterLocation::Header,
                false,
                "Grant read ACP permission",
            ),
            parameter(
                "x-tos-grant-write",
                ParameterLocation::Header,
                false,
                "Grant write permission",
            ),
            parameter(
                "x-tos-grant-write-acp",
                ParameterLocation::Header,
                false,
                "Grant write ACP permission",
            ),
        ]),
        ObjectAction::Download(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "versionId",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
            parameter("range", ParameterLocation::Header, false, "Range header"),
            parameter(
                "If-Modified-Since",
                ParameterLocation::Header,
                false,
                "If-Modified-Since",
            ),
            parameter(
                "If-Unmodified-Since",
                ParameterLocation::Header,
                false,
                "If-Unmodified-Since",
            ),
            parameter(
                "X-Replicated-From",
                ParameterLocation::Header,
                false,
                "X-Replicated-From",
            ),
            parameter(
                "X-From-Modular",
                ParameterLocation::Header,
                false,
                "X-From-Modular",
            ),
        ]),
        ObjectAction::Head(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "versionId",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
            parameter(
                "If-Modified-Since",
                ParameterLocation::Header,
                false,
                "If-Modified-Since",
            ),
            parameter(
                "If-Unmodified-Since",
                ParameterLocation::Header,
                false,
                "If-Unmodified-Since",
            ),
            parameter(
                "X-Replicated-From",
                ParameterLocation::Header,
                false,
                "X-Replicated-From",
            ),
            parameter(
                "X-From-Modular",
                ParameterLocation::Header,
                false,
                "X-From-Modular",
            ),
        ]),
        ObjectAction::List(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("prefix", ParameterLocation::Query, false, "Prefix filter"),
            parameter("delimiter", ParameterLocation::Query, false, "Delimiter"),
            parameter(
                "max-keys",
                ParameterLocation::Query,
                true,
                "Maximum keys per response",
            ),
            parameter(
                "continuation-token",
                ParameterLocation::Query,
                false,
                "Continuation token",
            ),
        ]),
        ObjectAction::ListVersions(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter(
                "versions",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter("prefix", ParameterLocation::Query, false, "Prefix filter"),
        ]),
        ObjectAction::Stat(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter("stat", ParameterLocation::Query, true, "Fixed marker query"),
        ]),
        ObjectAction::Delete(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "versionId",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
            parameter(
                "force",
                ParameterLocation::Flag,
                false,
                "Required for destructive execution",
            ),
            parameter(
                "X-From-Modular",
                ParameterLocation::Header,
                false,
                "From modular marker",
            ),
            parameter(
                "X-If-Match-Expires",
                ParameterLocation::Header,
                false,
                "If-match expires condition",
            ),
            parameter(
                "Last-Modified",
                ParameterLocation::Header,
                false,
                "Last modified header",
            ),
            parameter(
                "X-If-Match-CreateTime",
                ParameterLocation::Header,
                false,
                "If-match create time condition",
            ),
            parameter(
                "If-Match",
                ParameterLocation::Header,
                false,
                "If-Match condition",
            ),
            parameter(
                "X-If-Match-Tags",
                ParameterLocation::Header,
                false,
                "If-match tags condition",
            ),
            parameter(
                "X-If-Match-AccessTime",
                ParameterLocation::Header,
                false,
                "If-match access time condition",
            ),
            parameter(
                "x-lifecycle-directly-delete-versions",
                ParameterLocation::Header,
                false,
                "Lifecycle directly delete versions",
            ),
            parameter(
                "x-if-match-inode-id",
                ParameterLocation::Header,
                false,
                "If-match inode ID",
            ),
            parameter(
                "x-parent-inode-id",
                ParameterLocation::Header,
                false,
                "Parent inode ID",
            ),
            parameter(
                "x-only-put-delete-marker",
                ParameterLocation::Header,
                false,
                "Only put delete marker",
            ),
            parameter(
                "X-Inner-Properties-TimeStamp",
                ParameterLocation::Header,
                false,
                "Inner properties timestamp",
            ),
            parameter(
                "X-Inner-Properties-TimeStampNsec",
                ParameterLocation::Header,
                false,
                "Inner properties timestamp nsec",
            ),
        ]),
        ObjectAction::BatchDelete(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter(
                "delete",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "queryRecursive",
                ParameterLocation::Query,
                false,
                "Recursive delete",
            ),
            parameter(
                "querySkipTrash",
                ParameterLocation::Query,
                false,
                "Skip trash",
            ),
            parameter(
                "Content-MD5",
                ParameterLocation::Header,
                false,
                "Content-MD5 checksum",
            ),
            parameter(
                "keys(body)",
                ParameterLocation::Body,
                true,
                "Delete objects payload",
            ),
            parameter(
                "force",
                ParameterLocation::Flag,
                false,
                "Required for destructive execution",
            ),
        ]),
        ObjectAction::SetMeta(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "metadata",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "queryVersionID",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
            parameter(
                "x-unique-tag",
                ParameterLocation::Header,
                false,
                "Unique tag",
            ),
            parameter(
                "Content-Type",
                ParameterLocation::Header,
                false,
                "Content type",
            ),
            parameter(
                "meta(body)",
                ParameterLocation::Body,
                true,
                "Metadata payload",
            ),
        ]),
        ObjectAction::SetTime(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter("time", ParameterLocation::Query, true, "Fixed marker query"),
            parameter(
                "x-modify-timestamp",
                ParameterLocation::Header,
                false,
                "Modify timestamp",
            ),
            parameter(
                "x-modify-timestamp-ns",
                ParameterLocation::Header,
                false,
                "Modify timestamp ns",
            ),
        ]),
        ObjectAction::SetExpires(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "objectExpires",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "queryVersionID",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
        ]),
        ObjectAction::Append(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "append",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter("offset", ParameterLocation::Query, true, "Append offset"),
            parameter(
                "append-last-time",
                ParameterLocation::Query,
                false,
                "Append last time",
            ),
            parameter(
                "queryVersionID",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
            parameter(
                "Content-Type",
                ParameterLocation::Header,
                false,
                "Content type",
            ),
            parameter(
                "Content-MD5",
                ParameterLocation::Header,
                false,
                "Content-MD5 checksum",
            ),
            parameter(
                "x-content-sha256",
                ParameterLocation::Header,
                false,
                "SHA256 checksum",
            ),
            parameter(
                "x-decoded-content-length",
                ParameterLocation::Header,
                false,
                "Decoded content length",
            ),
            parameter(
                "x-object-lock-mode",
                ParameterLocation::Header,
                false,
                "Object lock mode",
            ),
            parameter(
                "x-object-lock-retain-until-date",
                ParameterLocation::Header,
                false,
                "Object lock retain-until date",
            ),
            parameter("x-tos-acl", ParameterLocation::Header, false, "ACL value"),
            parameter(
                "x-tos-grant-full-control",
                ParameterLocation::Header,
                false,
                "Grant full control",
            ),
            parameter(
                "x-tos-grant-read",
                ParameterLocation::Header,
                false,
                "Grant read permission",
            ),
            parameter(
                "x-tos-grant-read-non-list",
                ParameterLocation::Header,
                false,
                "Grant read without list permission",
            ),
            parameter(
                "x-tos-grant-read-acp",
                ParameterLocation::Header,
                false,
                "Grant read ACP permission",
            ),
            parameter(
                "x-tos-grant-write",
                ParameterLocation::Header,
                false,
                "Grant write permission",
            ),
            parameter(
                "x-tos-grant-write-acp",
                ParameterLocation::Header,
                false,
                "Grant write ACP permission",
            ),
            parameter(
                "x-persistent-headers",
                ParameterLocation::Header,
                false,
                "Persistent headers list",
            ),
            parameter(
                "x-traffic-limit",
                ParameterLocation::Header,
                false,
                "Traffic limit",
            ),
            parameter(
                "if-none-match",
                ParameterLocation::Header,
                false,
                "If-None-Match condition",
            ),
            parameter(
                "x-if-match",
                ParameterLocation::Header,
                false,
                "If-Match condition",
            ),
            parameter(
                "if-match",
                ParameterLocation::Header,
                false,
                "If-Match condition",
            ),
            parameter("body", ParameterLocation::Body, true, "Append body"),
        ]),
        ObjectAction::SealAppend(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter("seal", ParameterLocation::Query, true, "Fixed marker query"),
            parameter("offset", ParameterLocation::Query, false, "Append offset"),
            parameter(
                "queryVersionID",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
            parameter("x-tos-acl", ParameterLocation::Header, false, "ACL value"),
            parameter(
                "x-tos-grant-full-control",
                ParameterLocation::Header,
                false,
                "Grant full control",
            ),
            parameter(
                "x-tos-grant-read",
                ParameterLocation::Header,
                false,
                "Grant read permission",
            ),
            parameter(
                "x-tos-grant-read-non-list",
                ParameterLocation::Header,
                false,
                "Grant read without list permission",
            ),
            parameter(
                "x-tos-grant-read-acp",
                ParameterLocation::Header,
                false,
                "Grant read ACP permission",
            ),
            parameter(
                "x-tos-grant-write",
                ParameterLocation::Header,
                false,
                "Grant write permission",
            ),
            parameter(
                "x-tos-grant-write-acp",
                ParameterLocation::Header,
                false,
                "Grant write ACP permission",
            ),
            parameter(
                "if-none-match",
                ParameterLocation::Header,
                false,
                "If-None-Match condition",
            ),
            parameter(
                "x-if-match",
                ParameterLocation::Header,
                false,
                "If-Match condition",
            ),
            parameter(
                "if-match",
                ParameterLocation::Header,
                false,
                "If-Match condition",
            ),
        ]),
        ObjectAction::Modify(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "modify",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter("offset", ParameterLocation::Query, true, "Modify offset"),
            parameter(
                "queryVersionID",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
            parameter(
                "Content-Type",
                ParameterLocation::Header,
                false,
                "Content type",
            ),
            parameter(
                "Content-MD5",
                ParameterLocation::Header,
                false,
                "Content-MD5 checksum",
            ),
            parameter(
                "x-traffic-limit",
                ParameterLocation::Header,
                false,
                "Traffic limit",
            ),
            parameter(
                "if-none-match",
                ParameterLocation::Header,
                false,
                "If-None-Match condition",
            ),
            parameter(
                "x-if-match",
                ParameterLocation::Header,
                false,
                "If-Match condition",
            ),
            parameter(
                "if-match",
                ParameterLocation::Header,
                false,
                "If-Match condition",
            ),
            parameter("body", ParameterLocation::Body, true, "Modify body"),
        ]),
        ObjectAction::Rename(_) => Some(vec![
            parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Source bucket name",
            ),
            parameter("object", ParameterLocation::Path, true, "Source object key"),
            parameter(
                "rename",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "X-Tracer-Traceid",
                ParameterLocation::Header,
                false,
                "Trace ID",
            ),
            parameter(
                "x-recursive-mkdir",
                ParameterLocation::Header,
                false,
                "Recursive mkdir",
            ),
            parameter(
                "x-not-update-timestamp",
                ParameterLocation::Header,
                false,
                "Do not update timestamp",
            ),
            parameter(
                "x-forbid-overwrite",
                ParameterLocation::Header,
                false,
                "Forbid overwrite",
            ),
            parameter(
                "destination(body)",
                ParameterLocation::Body,
                true,
                "Destination object key",
            ),
        ]),
        ObjectAction::Restore(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "restore",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "queryVersionID",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
            parameter(
                "Content-MD5",
                ParameterLocation::Header,
                false,
                "Content-MD5 checksum",
            ),
            parameter("days(body)", ParameterLocation::Body, true, "Restore days"),
        ]),
        ObjectAction::Status(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "status",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
        ]),
        ObjectAction::SetAcl(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter("acl", ParameterLocation::Query, true, "Fixed marker query"),
            parameter(
                "queryVersionID",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
            parameter("acl(body)", ParameterLocation::Body, true, "ACL payload"),
        ]),
        ObjectAction::GetAcl(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter("acl", ParameterLocation::Query, true, "Fixed marker query"),
            parameter(
                "queryVersionID",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
        ]),
        ObjectAction::GetTagging(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "tagging",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "queryVersionID",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
        ]),
        ObjectAction::SetTagging(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "tagging",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "tags(body)",
                ParameterLocation::Body,
                true,
                "Tagging payload",
            ),
        ]),
        ObjectAction::DeleteTagging(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "tagging",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "force",
                ParameterLocation::Flag,
                false,
                "Required for real delete-tagging execution",
            ),
        ]),
        ObjectAction::Link(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter("link", ParameterLocation::Query, true, "Fixed marker query"),
            parameter(
                "x-link-target",
                ParameterLocation::Header,
                true,
                "Link target object key",
            ),
            parameter("x-tos-acl", ParameterLocation::Header, false, "ACL value"),
            parameter(
                "x-tos-grant-full-control",
                ParameterLocation::Header,
                false,
                "Grant full control",
            ),
            parameter(
                "x-tos-grant-read",
                ParameterLocation::Header,
                false,
                "Grant read permission",
            ),
            parameter(
                "x-tos-grant-read-non-list",
                ParameterLocation::Header,
                false,
                "Grant read without list permission",
            ),
            parameter(
                "x-tos-grant-read-acp",
                ParameterLocation::Header,
                false,
                "Grant read ACP permission",
            ),
            parameter(
                "x-tos-grant-write",
                ParameterLocation::Header,
                false,
                "Grant write permission",
            ),
            parameter(
                "x-tos-grant-write-acp",
                ParameterLocation::Header,
                false,
                "Grant write ACP permission",
            ),
            parameter("x-tagging", ParameterLocation::Header, false, "Object tags"),
        ]),
        ObjectAction::GetSymlink(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "queryVersionID",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
        ]),
        ObjectAction::CreateSymlink(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "symlink",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "x-symlink-target",
                ParameterLocation::Header,
                true,
                "Symlink target object key",
            ),
            parameter(
                "x-symlink-bucket",
                ParameterLocation::Header,
                false,
                "Symlink target bucket",
            ),
            parameter("x-tos-acl", ParameterLocation::Header, false, "ACL value"),
            parameter(
                "x-tos-grant-full-control",
                ParameterLocation::Header,
                false,
                "Grant full control",
            ),
            parameter(
                "x-tos-grant-read",
                ParameterLocation::Header,
                false,
                "Grant read permission",
            ),
            parameter(
                "x-tos-grant-read-non-list",
                ParameterLocation::Header,
                false,
                "Grant read without list permission",
            ),
            parameter(
                "x-tos-grant-read-acp",
                ParameterLocation::Header,
                false,
                "Grant read ACP permission",
            ),
            parameter(
                "x-tos-grant-write",
                ParameterLocation::Header,
                false,
                "Grant write permission",
            ),
            parameter(
                "x-tos-grant-write-acp",
                ParameterLocation::Header,
                false,
                "Grant write ACP permission",
            ),
            parameter("x-tagging", ParameterLocation::Header, false, "Object tags"),
        ]),
        ObjectAction::GetFetchTask(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter(
                "fetchTask",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter("taskId", ParameterLocation::Query, false, "Fetch task ID"),
        ]),
        ObjectAction::CreateFetchTask(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter(
                "fetchTask",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "x-etag-pattern",
                ParameterLocation::Header,
                false,
                "ETag pattern hint",
            ),
            parameter(
                "x-traffic-limit",
                ParameterLocation::Header,
                false,
                "Traffic limit",
            ),
            parameter(
                "x-if-unmodified-since",
                ParameterLocation::Header,
                false,
                "Unmodified-since condition",
            ),
            parameter(
                "if-none-match",
                ParameterLocation::Header,
                false,
                "If-None-Match condition",
            ),
            parameter(
                "if-match",
                ParameterLocation::Header,
                false,
                "If-Match condition",
            ),
            parameter(
                "x-object-lock-mode",
                ParameterLocation::Header,
                false,
                "Object lock mode",
            ),
            parameter(
                "x-object-lock-retain-until-date",
                ParameterLocation::Header,
                false,
                "Object lock retain-until date",
            ),
            parameter("x-tos-acl", ParameterLocation::Header, false, "ACL value"),
            parameter(
                "x-tos-grant-full-control",
                ParameterLocation::Header,
                false,
                "Grant full control",
            ),
            parameter(
                "x-tos-grant-read",
                ParameterLocation::Header,
                false,
                "Grant read permission",
            ),
            parameter(
                "x-tos-grant-read-non-list",
                ParameterLocation::Header,
                false,
                "Grant read without list permission",
            ),
            parameter(
                "x-tos-grant-read-acp",
                ParameterLocation::Header,
                false,
                "Grant read ACP permission",
            ),
            parameter(
                "x-tos-grant-write",
                ParameterLocation::Header,
                false,
                "Grant write permission",
            ),
            parameter(
                "x-tos-grant-write-acp",
                ParameterLocation::Header,
                false,
                "Grant write ACP permission",
            ),
        ]),
        ObjectAction::Fetch(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "fetch",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "source_url(body)",
                ParameterLocation::Body,
                true,
                "Source URL",
            ),
            parameter(
                "storage_class(body)",
                ParameterLocation::Body,
                false,
                "Storage class",
            ),
            parameter(
                "meta(body)",
                ParameterLocation::Body,
                false,
                "Custom metadata",
            ),
        ]),
        ObjectAction::SetRetention(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "retention",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "versionId",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
            parameter(
                "Mode(body)",
                ParameterLocation::Body,
                true,
                "Retention mode (COMPLIANCE)",
            ),
            parameter(
                "RetainUntilDate(body)",
                ParameterLocation::Body,
                true,
                "Retention expiration timestamp",
            ),
            parameter(
                "Content-MD5",
                ParameterLocation::Header,
                false,
                "Content-MD5 checksum (auto-computed when omitted)",
            ),
        ]),
        ObjectAction::GetRetention(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "retention",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "versionId",
                ParameterLocation::Query,
                false,
                "Target object version",
            ),
        ]),
    };

    CommandDescription {
        command: command.to_string(),
        layer: CommandLayer::LowLevel,
        api: Some(api.to_string()),
        description: description.to_string(),
        risk_level: risk,
        supports_dry_run: true,
        supports_pipe: pipe,
        parameters,
        scenario_routing: Some(scenario_routing),
        related_commands: None,
        low_level_apis: None,
        ..Default::default()
    }
}

pub fn describe_object_group() -> serde_json::Value {
    serde_json::json!({
        "command": "ve-tos object",
        "kind": "command_group",
        "layer": "low_level",
        "description": "Object Core APIs",
        "supports_help": true,
        "supports_describe": true,
        "subcommands": [
            {"name": "upload", "api": "PutObject", "risk_level": "medium"},
            {"name": "download", "api": "GetObject", "risk_level": "low"},
            {"name": "copy", "api": "CopyObject", "risk_level": "medium"},
            {"name": "delete", "api": "DeleteObject", "risk_level": "high"},
            {"name": "batch-delete", "api": "DeleteObjects", "risk_level": "high"},
            {"name": "list", "api": "ListBucket", "risk_level": "low"},
            {"name": "head", "api": "HeadObject", "risk_level": "low"},
            {"name": "set-retention", "api": "PutObjectRetention", "risk_level": "high"}
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_object_retention_body_uses_documented_json_schema() {
        let body = build_object_retention_body("COMPLIANCE", "2026-06-05T12:00:00Z")
            .expect("build retention body");

        assert_eq!(
            serde_json::from_slice::<Value>(&body).expect("retention body is json"),
            json!({
                "Mode": "COMPLIANCE",
                "RetainUntilDate": "2026-06-05T12:00:00Z",
            })
        );
    }

    #[test]
    fn content_md5_base64_matches_retention_body() {
        let body = build_object_retention_body("COMPLIANCE", "2026-06-05T12:00:00Z")
            .expect("build retention body");

        assert_eq!(content_md5_base64(&body), "OP7DOGpIoMkGaCjT3086KQ==");
    }

    #[test]
    fn normalize_object_retention_mode_rejects_unsupported_modes() {
        assert_eq!(
            normalize_object_retention_mode("compliance").expect("normalize compliance"),
            "COMPLIANCE"
        );
        assert!(normalize_object_retention_mode("GOVERNANCE").is_err());
    }

    #[test]
    fn object_copy_metadata_directive_header_matches_tos_sdks() {
        let mut headers = BTreeMap::new();

        insert_object_copy_metadata_directive_header(&mut headers, Some("REPLACE_NEW".to_string()));

        assert_eq!(
            headers.get("x-tos-metadata-directive").map(String::as_str),
            Some("REPLACE_NEW")
        );
        assert!(!headers.contains_key("x-metadata-directive"));
    }

    #[test]
    fn object_upload_dry_run_rejects_bytetos_storage_class_override() {
        let action = ObjectAction::Upload(ObjectUploadArgs {
            uri: Some("tos://bucket/object.bin".to_string()),
            bucket: None,
            key: None,
            body: Some("/tmp/object.bin".to_string()),
            content_type: None,
            storage_class: Some("ARCHIVE".to_string()),
            meta: None,
            net_speed_test: None,
            acl: None,
            grant_full_control: None,
            grant_read: None,
            grant_read_non_list: None,
            grant_read_acp: None,
            grant_write: None,
            grant_write_acp: None,
            tagging: None,
            object_lock_mode: None,
            object_lock_retain_until_date: None,
            if_none_match: None,
            forbid_overwrite: false,
            content_md5: None,
            traffic_limit: None,
            persistent_headers: None,
            etag_pattern: None,
        });

        let err = dry_run_object_action(&action).expect_err("storage class override is rejected");

        assert!(err
            .to_string()
            .contains("ByteTOS upload does not support --storage-class"));
    }
}
