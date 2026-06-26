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
use crate::domain::core;
use crate::domain::multipart as multipart_domain;
use crate::handler::common::{
    build_profile, build_query, classify_body_input, ensure_force_for_destructive, marker_query,
    output_result, output_result_with_columns, parse_object_target, read_json_input, BodyInput,
};
use reqwest::Method;
use tos_core::agent::describe::{
    CommandDescription, CommandLayer, CommandParameter, ParameterLocation, RiskLevel,
};
use tos_core::agent::dryrun::{DryRunResult, Impact};
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::client::TosClient;

/// Handle `ve-tos multipart ...` subcommands.
pub async fn handle_multipart_command(
    global: &GlobalArgs,
    action: &Option<MultipartAction>,
) -> Result<i32, CliError> {
    if global.describe {
        if let Some(action) = action {
            output_result(global, &describe_multipart_action(action))?;
        } else {
            output_result(global, &describe_multipart_group())?;
        }
        return Ok(0);
    }

    let Some(action) = action else {
        return Err(CliError::ValidationError(
            "`ve-tos multipart` requires a subcommand; use `ve-tos multipart --help` or `ve-tos multipart --describe`".to_string(),
        ));
    };

    if global.dry_run {
        output_result(global, &dry_run_multipart_action(action)?)?;
        return Ok(0);
    }

    // [Review Fix #1] Pre-flight registry guard so destructive multipart
    // commands (e.g. abort) fail with a deterministic ValidationError before
    // we attempt to build the runtime profile, which would otherwise mask
    // the missing --force with a ConfigMissing error.
    // [Review Fix #ForceGate] Pre-flight registry guard：添加 TTY 感知，
    // 交互式终端下放行至 handler 内部的 ensure_force_for_destructive 处理提示。
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let stderr_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let can_prompt = stdin_tty && stderr_tty && !global.quiet;
    let force_flag = match action {
        MultipartAction::Abort(args) => args.force,
        _ => false,
    } || (global.yes && can_prompt)
        || can_prompt;
    if let Err(violation) = crate::registry::enforce_registry_guards(
        &format!("ve-tos multipart {}", multipart_action_name(action)),
        force_flag,
        stderr_tty,
    ) {
        return Err(CliError::ValidationError(format!(
            "{} requires --force (or run in an interactive terminal)",
            violation.command
        )));
    }

    let profile = build_profile(global)?;
    let client = TosClient::new(&profile, "tos")?;

    match action {
        MultipartAction::Create(args) => handle_create(global, &client, args).await,
        MultipartAction::Upload(args) => handle_upload(global, &client, args).await,
        MultipartAction::Complete(args) => handle_complete(global, &client, args).await,
        MultipartAction::Abort(args) => handle_abort(global, &client, args).await,
        MultipartAction::Copy(args) => handle_copy(global, &client, args).await,
        MultipartAction::ListParts(args) => handle_list_parts(global, &client, args).await,
        MultipartAction::List(args) => handle_list(global, &client, args).await,
    }?;

    Ok(0)
}

fn multipart_action_name(action: &MultipartAction) -> &'static str {
    match action {
        MultipartAction::Create(_) => "create",
        MultipartAction::Upload(_) => "upload",
        MultipartAction::Complete(_) => "complete",
        MultipartAction::Abort(_) => "abort",
        MultipartAction::Copy(_) => "copy",
        MultipartAction::ListParts(_) => "list-parts",
        MultipartAction::List(_) => "list",
    }
}

async fn handle_create(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MultipartCreateArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mut headers = BTreeMap::new();
    if args.forbid_overwrite {
        headers.insert("x-forbid-overwrite".to_string(), "true".to_string());
    }
    insert_optional_header(&mut headers, "x-etag-pattern", args.etag_pattern.clone());
    insert_acl_headers(
        &mut headers,
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
    );
    insert_optional_header(&mut headers, "x-tagging", args.tagging.clone());
    insert_optional_header(
        &mut headers,
        "x-persistent-headers",
        args.persistent_headers.clone(),
    );
    // [Review Fix #3] 补齐 CreateMultipartUpload 文档中的复制/来源相关 header（可选）。
    insert_optional_header(
        &mut headers,
        "x-replicated-from",
        args.replicated_from.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-crr-source-versionId",
        args.crr_source_version_id.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-crr-source-last-modify-time",
        args.crr_source_last_modify_time.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-crr-source-timestamp-nsec",
        args.crr_source_timestamp_nsec.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-crr-source-bucket-version-status",
        args.crr_source_bucket_version_status.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-crr-source-uploadId",
        args.crr_source_upload_id.clone(),
    );
    insert_optional_header(&mut headers, "X-From-Modular", args.from_modular.clone());
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
    let result = core::execute_object_request(
        client,
        "ve-tos multipart create",
        Method::POST,
        &bucket,
        &key,
        marker_query(&["uploads"]),
        headers,
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_upload(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MultipartUploadArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    // [Review Fix #M4] Stream file-path bodies through the V4 streaming
    // pipeline so per-part uploads do not buffer the entire payload.
    let body_input = classify_body_input(&args.body)?;
    let query = build_query(&[
        ("partNumber", Some(args.part_number.to_string())),
        ("uploadId", Some(args.upload_id.clone())),
    ]);
    let mut headers = BTreeMap::new();
    insert_optional_header(&mut headers, "content-md5", args.content_md5.clone());
    insert_optional_header(
        &mut headers,
        "x-content-sha256",
        args.content_sha256.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-hash-crc64ecma",
        args.hash_crc64ecma.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-decoded-content-length",
        args.decoded_content_length.map(|v| v.to_string()),
    );
    insert_optional_header(
        &mut headers,
        "x-traffic-limit",
        args.traffic_limit.map(|v| v.to_string()),
    );
    match body_input {
        BodyInput::FilePath { path, len } => {
            let path_str = path.to_string_lossy().to_string();
            // [Review Fix] TOS requires Content-Length for streaming multipart upload bodies
            headers.insert("content-length".to_string(), len.to_string());
            let payload_hash = crate::handler::high_level::file_sha256(&path_str)?;
            let body = crate::handler::high_level::file_stream_body(&path_str).await?;
            let result = core::execute_object_streaming_request(
                client,
                "ve-tos multipart upload",
                Method::PUT,
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
                "ve-tos multipart upload",
                Method::PUT,
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

async fn handle_complete(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MultipartCompleteArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    // [Review Fix #7] TOS CompleteMultipartUpload 要求 {"Parts": [...]} 包装;
    // 当用户传入裸数组时自动包装。
    let raw = read_json_input(&args.parts)?;
    let body = if raw.is_array() {
        serde_json::json!({ "Parts": raw })
    } else {
        raw
    };
    let mut query = build_query(&[("uploadId", Some(args.upload_id.clone()))]);
    if args.complete_all {
        query.insert("x-complete-all".to_string(), "true".to_string());
    }
    let mut headers =
        BTreeMap::from([("content-type".to_string(), "application/json".to_string())]);
    if args.complete_all {
        headers.insert("x-complete-all".to_string(), "true".to_string());
    }
    insert_optional_header(
        &mut headers,
        "x-if-unmodified-since",
        args.if_unmodified_since.clone(),
    );
    insert_optional_header(&mut headers, "if-none-match", args.if_none_match.clone());
    insert_optional_header(&mut headers, "if-match", args.if_match.clone());
    insert_optional_header(
        &mut headers,
        "x-server-side-encryption",
        args.server_side_encryption.clone(),
    );
    insert_optional_header(&mut headers, "X-From-Modular", args.from_modular.clone());
    let result = core::execute_object_request(
        client,
        "ve-tos multipart complete",
        Method::POST,
        &bucket,
        &key,
        query,
        headers,
        Some(serde_json::to_vec(&body).map_err(CliError::Json)?),
    )
    .await?;
    output_result(global, &result)
}

async fn handle_abort(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MultipartAbortArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    ensure_force_for_destructive(
        global,
        args.force,
        "ve-tos multipart abort",
        &format!("tos://{bucket}/{key}"),
    )?;
    let result = core::execute_object_request(
        client,
        "ve-tos multipart abort",
        Method::DELETE,
        &bucket,
        &key,
        build_query(&[("uploadId", Some(args.upload_id.clone()))]),
        {
            let mut headers = BTreeMap::new();
            insert_optional_header(&mut headers, "X-From-Modular", args.from_modular.clone());
            headers
        },
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_copy(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MultipartCopyArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let mut headers = BTreeMap::new();
    // [Review Fix #M3] Use TOS-native copy headers (x-tos-*).
    headers.insert("x-tos-copy-source".to_string(), args.copy_source.clone());
    insert_optional_header(
        &mut headers,
        "x-tos-copy-source-range",
        args.copy_source_range.clone(),
    );
    insert_optional_header(
        &mut headers,
        "x-tos-copy-source-part-number",
        args.copy_source_part_number.map(|v| v.to_string()),
    );
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
    insert_optional_header(
        &mut headers,
        "x-traffic-limit",
        args.traffic_limit.map(|v| v.to_string()),
    );
    let query = build_query(&[
        ("partNumber", Some(args.part_number.to_string())),
        ("uploadId", Some(args.upload_id.clone())),
    ]);
    let result = core::execute_object_request(
        client,
        "ve-tos multipart copy",
        Method::PUT,
        &bucket,
        &key,
        query,
        headers,
        None,
    )
    .await?;
    output_result(global, &result)
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

fn insert_acl_headers(
    headers: &mut BTreeMap<String, String>,
    acl: Option<String>,
    grant_full_control: Option<String>,
    grant_read: Option<String>,
    grant_read_non_list: Option<String>,
    grant_read_acp: Option<String>,
    grant_write: Option<String>,
    grant_write_acp: Option<String>,
) {
    if let Some(acl) = acl {
        // [Review Fix #MultipartHeaderCanonical] ACL-related multipart
        // headers use canonical `x-tos-*` names instead of legacy duplicates.
        headers.insert("x-tos-acl".to_string(), acl);
    }
    insert_optional_header(headers, "x-tos-grant-full-control", grant_full_control);
    insert_optional_header(headers, "x-tos-grant-read", grant_read);
    insert_optional_header(headers, "x-tos-grant-read-non-list", grant_read_non_list);
    insert_optional_header(headers, "x-tos-grant-read-acp", grant_read_acp);
    insert_optional_header(headers, "x-tos-grant-write", grant_write);
    insert_optional_header(headers, "x-tos-grant-write-acp", grant_write_acp);
}

async fn handle_list_parts(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MultipartListPartsArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    // [Review Fix #FmtUni-Phase2] Typed handler，与 `bucket list`/`object list` 同构。
    let result = multipart_domain::list_parts(
        client,
        &bucket,
        &key,
        &args.upload_id,
        args.part_number_marker,
        args.max_parts,
        args.fetch_from_kv,
    )
    .await?;
    output_result_with_columns(global, &result, Some(MULTIPART_LIST_PARTS_TABLE_COLUMNS))
}

const MULTIPART_LIST_PARTS_TABLE_COLUMNS: &[&str] =
    &["part_number", "size", "etag", "last_modified"];

async fn handle_list(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MultipartListArgs,
) -> Result<(), CliError> {
    // [Review Fix #FmtUni-Phase2] Typed handler 替换原 raw API 路径。
    let bucket = args.bucket.require()?;
    let result = multipart_domain::list_multipart_uploads(
        client,
        &bucket,
        args.prefix.as_deref(),
        args.delimiter.as_deref(),
        args.key_marker.as_deref(),
        args.upload_id_marker.as_deref(),
        args.max_uploads,
        args.encoding_type.as_deref(),
        args.fetch_from_kv,
    )
    .await?;
    output_result_with_columns(global, &result, Some(MULTIPART_LIST_TABLE_COLUMNS))
}

const MULTIPART_LIST_TABLE_COLUMNS: &[&str] = &["key", "upload_id", "initiated", "storage_class"];

fn dry_run_multipart_action(action: &MultipartAction) -> Result<DryRunResult, CliError> {
    let (command, method, target, risk_level, has_body) = multipart_dry_run_meta(action)?;
    Ok(DryRunResult {
        action: command.to_string(),
        dry_run: true,
        impact: Impact {
            affected_objects: if matches!(risk_level, "high") { 1 } else { 0 },
            affected_bytes: 0,
            risk_level: risk_level.to_string(),
            estimated_duration: Some("< 1s".to_string()),
            scanned_count: None,
            preview_truncated: None,
        },
        plan: vec![format!("{} {}", method, target)],
        warnings: if has_body {
            vec!["Request body is omitted from dry-run output; validate your part list/body before execution.".to_string()]
        } else {
            vec![]
        },
        confirm_command: None,
    })
}

fn multipart_dry_run_meta(
    action: &MultipartAction,
) -> Result<(&'static str, &'static str, String, &'static str, bool), CliError> {
    Ok(match action {
        MultipartAction::Create(args) => (
            "ve-tos multipart create",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            false,
        ),
        MultipartAction::Upload(args) => (
            "ve-tos multipart upload",
            "PUT",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        MultipartAction::Complete(args) => (
            "ve-tos multipart complete",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        MultipartAction::Abort(args) => (
            "ve-tos multipart abort",
            "DELETE",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "high",
            false,
        ),
        MultipartAction::Copy(args) => (
            "ve-tos multipart copy",
            "PUT",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            false,
        ),
        MultipartAction::ListParts(args) => (
            "ve-tos multipart list-parts",
            "GET",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "low",
            false,
        ),
        MultipartAction::List(args) => (
            "ve-tos multipart list",
            "GET",
            format!("tos://{}", args.bucket.require()?),
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

fn describe_multipart_action(action: &MultipartAction) -> CommandDescription {
    let (command, api, description, risk) = match action {
        MultipartAction::Create(_) => (
            "ve-tos multipart create",
            "CreateMultipartUpload",
            "Create a multipart upload session",
            RiskLevel::Medium,
        ),
        MultipartAction::Upload(_) => (
            "ve-tos multipart upload",
            "UploadPart",
            "Upload one multipart part",
            RiskLevel::Medium,
        ),
        MultipartAction::Complete(_) => (
            "ve-tos multipart complete",
            "CompleteMultipartUpload",
            "Complete a multipart upload",
            RiskLevel::Medium,
        ),
        MultipartAction::Abort(_) => (
            "ve-tos multipart abort",
            "AbortMultipartUpload",
            "Abort a multipart upload (requires --force for execution)",
            RiskLevel::High,
        ),
        MultipartAction::Copy(_) => (
            "ve-tos multipart copy",
            "UploadPartCopy",
            "Upload a part by copy",
            RiskLevel::Medium,
        ),
        MultipartAction::ListParts(_) => (
            "ve-tos multipart list-parts",
            "ListMultipartUploadParts",
            "List uploaded parts",
            RiskLevel::Low,
        ),
        MultipartAction::List(_) => (
            "ve-tos multipart list",
            "ListMultipartUploads",
            "List multipart uploads in a bucket",
            RiskLevel::Low,
        ),
    };

    let scenario_routing = match action {
        MultipartAction::Abort(_) => HashMap::from([
            (
                "Confirm abort (requires --force)".to_string(),
                "ve-tos multipart abort tos://bucket/key --upload-id <id> --force".to_string(),
            ),
            (
                "Preview abort".to_string(),
                "ve-tos multipart abort tos://bucket/key --upload-id <id> --dry-run".to_string(),
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
        MultipartAction::Create(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "uploads",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "x-forbid-overwrite",
                ParameterLocation::Header,
                false,
                "Forbid overwrite existing object",
            ),
            parameter(
                "x-etag-pattern",
                ParameterLocation::Header,
                false,
                "ETag pattern hint",
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
            parameter(
                "x-replicated-from",
                ParameterLocation::Header,
                false,
                "Replicated-from marker",
            ),
            parameter(
                "x-crr-source-versionId",
                ParameterLocation::Header,
                false,
                "CRR source versionId",
            ),
            parameter(
                "x-crr-source-last-modify-time",
                ParameterLocation::Header,
                false,
                "CRR source last modify time",
            ),
            parameter(
                "x-crr-source-timestamp-nsec",
                ParameterLocation::Header,
                false,
                "CRR source timestamp (ns)",
            ),
            parameter(
                "x-crr-source-bucket-version-status",
                ParameterLocation::Header,
                false,
                "CRR source bucket version status",
            ),
            parameter(
                "x-crr-source-uploadId",
                ParameterLocation::Header,
                false,
                "CRR source uploadId",
            ),
            parameter(
                "X-From-Modular",
                ParameterLocation::Header,
                false,
                "From modular marker",
            ),
            parameter(
                "x-persistent-headers",
                ParameterLocation::Header,
                false,
                "Persistent headers list",
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
        ]),
        MultipartAction::Upload(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "partNumber",
                ParameterLocation::Query,
                true,
                "Multipart part number",
            ),
            parameter(
                "uploadId",
                ParameterLocation::Query,
                true,
                "Multipart upload ID",
            ),
            parameter("body", ParameterLocation::Body, true, "Part body source"),
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
                "x-hash-crc64ecma",
                ParameterLocation::Header,
                false,
                "CRC64 checksum",
            ),
            parameter(
                "x-decoded-content-length",
                ParameterLocation::Header,
                false,
                "Decoded content length",
            ),
            parameter(
                "x-traffic-limit",
                ParameterLocation::Header,
                false,
                "Traffic limit",
            ),
        ]),
        MultipartAction::Complete(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "uploadId",
                ParameterLocation::Query,
                true,
                "Multipart upload ID",
            ),
            parameter(
                "x-complete-all",
                ParameterLocation::Query,
                false,
                "Complete all parts server-side",
            ),
            parameter(
                "parts",
                ParameterLocation::Body,
                true,
                "Completed parts JSON",
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
        ]),
        MultipartAction::Copy(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "partNumber",
                ParameterLocation::Query,
                true,
                "Multipart part number",
            ),
            parameter(
                "uploadId",
                ParameterLocation::Query,
                true,
                "Multipart upload ID",
            ),
            parameter(
                "x-tos-copy-source",
                ParameterLocation::Header,
                true,
                "Source object path /{sourceBucket}/{sourceObject}",
            ),
            parameter(
                "x-tos-copy-source-range",
                ParameterLocation::Header,
                false,
                "Source byte range",
            ),
            parameter(
                "x-tos-copy-source-part-number",
                ParameterLocation::Header,
                false,
                "Source part number",
            ),
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
                "x-traffic-limit",
                ParameterLocation::Header,
                false,
                "Traffic limit",
            ),
        ]),
        MultipartAction::ListParts(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "uploadId",
                ParameterLocation::Query,
                true,
                "Multipart upload ID",
            ),
            parameter(
                "part-number-marker",
                ParameterLocation::Query,
                false,
                "Part number marker",
            ),
            parameter(
                "max-parts",
                ParameterLocation::Query,
                false,
                "Maximum parts per response",
            ),
            parameter(
                "fetch-from-kv",
                ParameterLocation::Query,
                false,
                "Fetch from KV",
            ),
        ]),
        MultipartAction::Abort(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "uploadId",
                ParameterLocation::Query,
                true,
                "Multipart upload ID",
            ),
            parameter(
                "force",
                ParameterLocation::Flag,
                false,
                "Required for destructive execution",
            ),
        ]),
        MultipartAction::List(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter(
                "uploads",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter("prefix", ParameterLocation::Query, false, "Prefix filter"),
            parameter("delimiter", ParameterLocation::Query, false, "Delimiter"),
            parameter(
                "key-marker",
                ParameterLocation::Query,
                false,
                "Object key marker",
            ),
            parameter(
                "upload-id-marker",
                ParameterLocation::Query,
                false,
                "Upload ID marker",
            ),
            parameter(
                "max-uploads",
                ParameterLocation::Query,
                false,
                "Maximum uploads per response",
            ),
            parameter(
                "encoding-type",
                ParameterLocation::Query,
                false,
                "Encoding type",
            ),
            parameter(
                "fetch-from-kv",
                ParameterLocation::Query,
                false,
                "Fetch from KV",
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
        supports_pipe: false,
        parameters,
        scenario_routing: Some(scenario_routing),
        related_commands: None,
        low_level_apis: None,
        ..Default::default()
    }
}

pub fn describe_multipart_group() -> serde_json::Value {
    serde_json::json!({
        "command": "ve-tos multipart",
        "kind": "command_group",
        "layer": "low_level",
        "description": "Multipart Core APIs",
        "supports_help": true,
        "supports_describe": true,
        "subcommands": [
            {"name": "create", "api": "CreateMultipartUpload", "risk_level": "low"},
            {"name": "upload", "api": "UploadPart", "risk_level": "medium"},
            {"name": "complete", "api": "CompleteMultipartUpload", "risk_level": "medium"},
            {"name": "abort", "api": "AbortMultipartUpload", "risk_level": "high"},
            {"name": "copy", "api": "UploadPartCopy", "risk_level": "medium"},
            {"name": "list-parts", "api": "ListParts", "risk_level": "low"},
            {"name": "list", "api": "ListMultipartUploads", "risk_level": "low"}
        ]
    })
}
