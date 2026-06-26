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
use crate::handler::common::{
    build_profile, classify_body_input, output_result, parse_object_target, BodyInput,
};
use reqwest::Method;
use tos_core::agent::describe::{
    CommandDescription, CommandLayer, CommandParameter, ParameterLocation, RiskLevel,
};
use tos_core::agent::dryrun::{DryRunResult, Impact};
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::client::TosClient;

/// Handle `ve-tos turbo ...` subcommands.
pub async fn handle_turbo_command(
    global: &GlobalArgs,
    action: &Option<TurboAction>,
) -> Result<i32, CliError> {
    if global.describe {
        if let Some(action) = action {
            output_result(global, &describe_turbo_action(action))?;
        } else {
            output_result(global, &describe_turbo_group())?;
        }
        return Ok(0);
    }

    let Some(action) = action else {
        return Err(CliError::ValidationError(
            "`ve-tos turbo` requires a subcommand; use `ve-tos turbo --help` or `ve-tos turbo --describe`"
                .to_string(),
        ));
    };

    if global.dry_run {
        output_result(global, &dry_run_turbo_action(action)?)?;
        return Ok(0);
    }

    let profile = build_profile(global)?;
    let client = TosClient::new(&profile, "tos")?;

    match action {
        TurboAction::Open(args) => handle_open(global, &client, args).await,
        TurboAction::Append(args) => handle_append(global, &client, args).await,
        TurboAction::List(args) => handle_list(global, &client, args).await,
        TurboAction::Close(args) => handle_close(global, &client, args).await,
    }?;

    Ok(0)
}

async fn handle_open(
    global: &GlobalArgs,
    client: &TosClient,
    args: &TurboOpenArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let headers = turbo_headers(
        args.content_type.clone(),
        args.content_md5.clone(),
        args.hash_crc64ecma.clone(),
        args.traffic_limit.map(|v| v.to_string()),
        args.if_match_guard_object.clone(),
        None,
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
    );
    execute_turbo_action(
        global,
        client,
        "ve-tos turbo open",
        &bucket,
        &key,
        build_turbo_open_query(args),
        headers,
        None,
    )
    .await
}

async fn handle_append(
    global: &GlobalArgs,
    client: &TosClient,
    args: &TurboAppendArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    // [Review Fix #M4] Stream file-path bodies through the V4 streaming
    // pipeline instead of buffering into memory.
    let body_input = classify_body_input(&args.body)?;
    let mut headers = turbo_headers(
        Some("application/octet-stream".to_string()),
        args.content_md5.clone(),
        args.hash_crc64ecma.clone(),
        args.traffic_limit.map(|v| v.to_string()),
        args.if_match_guard_object.clone(),
        args.turbo_token.clone(),
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
    );
    let query = BTreeMap::from([("appendturbo".to_string(), String::new())]);
    match body_input {
        BodyInput::FilePath { path, len } => {
            let path_str = path.to_string_lossy().to_string();
            // [Review Fix] TOS requires Content-Length for streaming turbo append bodies
            headers.insert("content-length".to_string(), len.to_string());
            let payload_hash = crate::handler::high_level::file_sha256(&path_str)?;
            let body = crate::handler::high_level::file_stream_body(&path_str).await?;
            let result = core::execute_object_streaming_request(
                client,
                "ve-tos turbo append",
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
                "ve-tos turbo append",
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

async fn handle_list(
    global: &GlobalArgs,
    client: &TosClient,
    args: &TurboListArgs,
) -> Result<(), CliError> {
    let key = args.key.clone().ok_or_else(|| {
        CliError::ValidationError("`--key` is required for turbo list".to_string())
    })?;
    let bucket = args.bucket.require()?;
    let result = core::execute_object_request(
        client,
        "ve-tos turbo list",
        Method::GET,
        &bucket,
        &key,
        build_turbo_list_query(args),
        BTreeMap::new(),
        None,
    )
    .await?;
    output_result(global, &result)
}

async fn handle_close(
    global: &GlobalArgs,
    client: &TosClient,
    args: &TurboCloseArgs,
) -> Result<(), CliError> {
    let (bucket, key) = parse_object_target(
        args.uri.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
    )?;
    let headers = turbo_headers(
        None,
        None,
        None,
        args.traffic_limit.map(|v| v.to_string()),
        args.if_match_guard_object.clone(),
        args.turbo_token.clone(),
        args.acl.clone(),
        args.grant_full_control.clone(),
        args.grant_read.clone(),
        args.grant_read_non_list.clone(),
        args.grant_read_acp.clone(),
        args.grant_write.clone(),
        args.grant_write_acp.clone(),
    );
    execute_turbo_action(
        global,
        client,
        "ve-tos turbo close",
        &bucket,
        &key,
        build_turbo_close_query(),
        headers,
        None,
    )
    .await
}

async fn execute_turbo_action(
    global: &GlobalArgs,
    client: &TosClient,
    command: &str,
    bucket: &str,
    key: &str,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Option<serde_json::Value>,
) -> Result<(), CliError> {
    let encoded_body = match body {
        Some(v) => Some(serde_json::to_vec(&v).map_err(CliError::Json)?),
        None => None,
    };
    let result = core::execute_object_request(
        client,
        command,
        Method::POST,
        bucket,
        key,
        query,
        headers,
        encoded_body,
    )
    .await?;
    output_result(global, &result)
}

fn turbo_headers(
    content_type: Option<String>,
    content_md5: Option<String>,
    hash_crc64ecma: Option<String>,
    traffic_limit: Option<String>,
    if_match_guard_object: Option<String>,
    turbo_token: Option<String>,
    acl: Option<String>,
    grant_full_control: Option<String>,
    grant_read: Option<String>,
    grant_read_non_list: Option<String>,
    grant_read_acp: Option<String>,
    grant_write: Option<String>,
    grant_write_acp: Option<String>,
) -> BTreeMap<String, String> {
    // [Review Fix #TurboContentType] Turbo requests without a body (notably
    // CloseTurbo) must not inherit a synthetic JSON content type.
    let mut headers = BTreeMap::new();
    insert_optional_header(&mut headers, "content-type", content_type);
    insert_optional_header(&mut headers, "content-md5", content_md5);
    insert_optional_header(&mut headers, "x-hash-crc64ecma", hash_crc64ecma);
    insert_optional_header(&mut headers, "x-traffic-limit", traffic_limit);
    insert_optional_header(&mut headers, "if-match-guard-object", if_match_guard_object);
    // [Review Fix #TurboTokenHeader] AppendTurbo expects the canonical
    // `x-tos-turbo-token` header returned by OpenTurbo.
    insert_optional_header(&mut headers, "x-tos-turbo-token", turbo_token);
    if let Some(acl) = acl {
        // [Review Fix #TurboHeaderCanonical] TOS header parameters use the
        // canonical `x-tos-*` form; sending both legacy and canonical ACL
        // headers can make server-side validation ambiguous.
        headers.insert("x-tos-acl".to_string(), acl);
    }
    insert_optional_header(&mut headers, "x-tos-grant-full-control", grant_full_control);
    insert_optional_header(&mut headers, "x-tos-grant-read", grant_read);
    insert_optional_header(
        &mut headers,
        "x-tos-grant-read-non-list",
        grant_read_non_list,
    );
    insert_optional_header(&mut headers, "x-tos-grant-read-acp", grant_read_acp);
    insert_optional_header(&mut headers, "x-tos-grant-write", grant_write);
    insert_optional_header(&mut headers, "x-tos-grant-write-acp", grant_write_acp);
    headers
}

fn build_turbo_open_query(args: &TurboOpenArgs) -> BTreeMap<String, String> {
    // [Review Fix #TurboOpenMode] OpenTurbo requires `mode`: 0 creates a new
    // open session, 1 reopens in write mode.
    BTreeMap::from([
        ("openturbo".to_string(), String::new()),
        ("mode".to_string(), args.mode.to_string()),
    ])
}

fn build_turbo_close_query() -> BTreeMap<String, String> {
    // [Review Fix #TurboCloseMode] CloseTurbo validates the write-session mode
    // explicitly; mode=1 matches the server-side write-open/close path.
    BTreeMap::from([
        ("closeturbo".to_string(), String::new()),
        ("mode".to_string(), "1".to_string()),
    ])
}

fn build_turbo_list_query(args: &TurboListArgs) -> BTreeMap<String, String> {
    let mut query = BTreeMap::from([("listopenedturbo".to_string(), String::new())]);
    insert_optional_query(&mut query, "marker", args.marker.clone());
    insert_optional_query(&mut query, "max-keys", args.max_keys.map(|v| v.to_string()));
    insert_optional_query(&mut query, "prefix", args.prefix.clone());
    insert_optional_query(&mut query, "encoding-type", args.encoding_type.clone());
    query
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

fn insert_optional_query(query: &mut BTreeMap<String, String>, key: &str, value: Option<String>) {
    if let Some(value) = value {
        query.insert(key.to_string(), value);
    }
}

fn dry_run_turbo_action(action: &TurboAction) -> Result<DryRunResult, CliError> {
    let (command, method, target, risk_level, has_body) = turbo_dry_run_meta(action)?;
    Ok(DryRunResult {
        action: command.to_string(),
        dry_run: true,
        impact: Impact {
            affected_objects: 1,
            affected_bytes: 0,
            risk_level: risk_level.to_string(),
            estimated_duration: Some("< 1s".to_string()),
            scanned_count: None,
            preview_truncated: None,
        },
        plan: vec![format!("{} {}", method, target)],
        warnings: if has_body {
            vec!["Turbo request body is omitted from dry-run output; validate payload before execution.".to_string()]
        } else {
            vec![]
        },
        confirm_command: None,
    })
}

fn turbo_dry_run_meta(
    action: &TurboAction,
) -> Result<(&'static str, &'static str, String, &'static str, bool), CliError> {
    Ok(match action {
        TurboAction::Open(args) => (
            "ve-tos turbo open",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        TurboAction::Append(args) => (
            "ve-tos turbo append",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
        ),
        TurboAction::List(args) => (
            "ve-tos turbo list",
            "GET",
            format!(
                "tos://{}/{}",
                args.bucket.require()?,
                args.key.clone().unwrap_or_default()
            ),
            "low",
            false,
        ),
        TurboAction::Close(args) => (
            "ve-tos turbo close",
            "POST",
            object_target_preview(
                args.uri.as_deref(),
                args.bucket.as_deref(),
                args.key.as_deref(),
            )?,
            "medium",
            true,
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

fn turbo_acl_parameters() -> Vec<CommandParameter> {
    vec![
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
    ]
}

fn describe_turbo_action(action: &TurboAction) -> CommandDescription {
    let (command, api, description, risk) = match action {
        TurboAction::Open(_) => (
            "ve-tos turbo open",
            "OpenTurbo",
            "Open a turbo write session",
            RiskLevel::Medium,
        ),
        TurboAction::Append(_) => (
            "ve-tos turbo append",
            "AppendTurbo",
            "Append data through turbo",
            RiskLevel::Medium,
        ),
        TurboAction::List(_) => (
            "ve-tos turbo list",
            "ListOpenedTurbo",
            "List opened turbo sessions",
            RiskLevel::Low,
        ),
        TurboAction::Close(_) => (
            "ve-tos turbo close",
            "CloseTurbo",
            "Close a turbo write session",
            RiskLevel::Medium,
        ),
    };

    let parameters = match action {
        TurboAction::Open(_) => {
            let mut params = vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter("object", ParameterLocation::Path, true, "Object key"),
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
                    "x-hash-crc64ecma",
                    ParameterLocation::Header,
                    false,
                    "CRC64 checksum",
                ),
                parameter(
                    "x-traffic-limit",
                    ParameterLocation::Header,
                    false,
                    "Traffic limit",
                ),
                parameter(
                    "if-match-guard-object",
                    ParameterLocation::Header,
                    false,
                    "Guard object match condition",
                ),
                parameter(
                    "mode",
                    ParameterLocation::Query,
                    true,
                    "Open mode: 0=create open, 1=write open",
                ),
            ];
            params.extend(turbo_acl_parameters());
            Some(params)
        }
        TurboAction::Close(_) => {
            let mut params = vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter("object", ParameterLocation::Path, true, "Object key"),
                parameter(
                    "x-traffic-limit",
                    ParameterLocation::Header,
                    false,
                    "Traffic limit",
                ),
                parameter(
                    "if-match-guard-object",
                    ParameterLocation::Header,
                    false,
                    "Guard object match condition",
                ),
                parameter(
                    "x-tos-turbo-token",
                    ParameterLocation::Header,
                    false,
                    "Turbo session token",
                ),
            ];
            params.extend(turbo_acl_parameters());
            Some(params)
        }
        TurboAction::Append(_) => {
            let mut params = vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter("object", ParameterLocation::Path, true, "Object key"),
                parameter("body", ParameterLocation::Body, true, "Append payload"),
                parameter(
                    "x-tos-turbo-token",
                    ParameterLocation::Header,
                    false,
                    "Turbo session token",
                ),
                parameter(
                    "Content-MD5",
                    ParameterLocation::Header,
                    false,
                    "Content-MD5 checksum",
                ),
                parameter(
                    "x-hash-crc64ecma",
                    ParameterLocation::Header,
                    false,
                    "CRC64 checksum",
                ),
                parameter(
                    "x-traffic-limit",
                    ParameterLocation::Header,
                    false,
                    "Traffic limit",
                ),
                parameter(
                    "if-match-guard-object",
                    ParameterLocation::Header,
                    false,
                    "Guard object match condition",
                ),
            ];
            params.extend(turbo_acl_parameters());
            Some(params)
        }
        TurboAction::List(_) => Some(vec![
            parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
            parameter("object", ParameterLocation::Path, true, "Object key"),
            parameter(
                "listopenedturbo",
                ParameterLocation::Query,
                true,
                "Fixed marker query",
            ),
            parameter(
                "marker",
                ParameterLocation::Query,
                false,
                "Pagination marker",
            ),
            parameter(
                "max-keys",
                ParameterLocation::Query,
                false,
                "Maximum keys per response",
            ),
            parameter("prefix", ParameterLocation::Query, false, "Prefix filter"),
            parameter(
                "encoding-type",
                ParameterLocation::Query,
                false,
                "Encoding type",
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
        scenario_routing: Some(HashMap::from([
            ("English Example".to_string(), format!("{} --help", command)),
            (
                "Describe Example".to_string(),
                format!("{} --describe", command),
            ),
        ])),
        related_commands: None,
        low_level_apis: None,
        ..Default::default()
    }
}

pub fn describe_turbo_group() -> serde_json::Value {
    serde_json::json!({
        "command": "ve-tos turbo",
        "kind": "command_group",
        "layer": "low_level",
        "description": "Turbo Core APIs",
        "supports_help": true,
        "supports_describe": true,
        "subcommands": [
            {"name": "open", "api": "OpenTurbo", "risk_level": "medium"},
            {"name": "append", "api": "AppendTurbo", "risk_level": "medium"},
            {"name": "list", "api": "ListOpenedTurbo", "risk_level": "low"},
            {"name": "close", "api": "CloseTurbo", "risk_level": "medium"}
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turbo_open_args_with_mode(mode: u8) -> TurboOpenArgs {
        TurboOpenArgs {
            uri: None,
            bucket: Some("bucket".to_string()),
            key: Some("key".to_string()),
            content_type: None,
            content_md5: None,
            hash_crc64ecma: None,
            traffic_limit: None,
            if_match_guard_object: None,
            mode,
            acl: None,
            grant_full_control: None,
            grant_read: None,
            grant_read_non_list: None,
            grant_read_acp: None,
            grant_write: None,
            grant_write_acp: None,
        }
    }

    #[test]
    fn build_turbo_open_query_includes_mode() {
        let query = build_turbo_open_query(&turbo_open_args_with_mode(1));

        assert_eq!(query.get("openturbo").map(String::as_str), Some(""));
        assert_eq!(query.get("mode").map(String::as_str), Some("1"));
    }

    #[test]
    fn build_turbo_close_query_uses_write_mode() {
        let query = build_turbo_close_query();

        assert_eq!(query.get("closeturbo").map(String::as_str), Some(""));
        assert_eq!(query.get("mode").map(String::as_str), Some("1"));
    }

    #[test]
    fn turbo_headers_use_canonical_acl_names() {
        let headers = turbo_headers(
            None,
            None,
            None,
            None,
            None,
            Some("session-token".to_string()),
            Some("private".to_string()),
            Some("id=owner".to_string()),
            Some("id=reader".to_string()),
            None,
            None,
            None,
            Some("id=writer".to_string()),
        );

        assert_eq!(
            headers.get("x-tos-acl").map(String::as_str),
            Some("private")
        );
        assert_eq!(
            headers.get("x-tos-turbo-token").map(String::as_str),
            Some("session-token")
        );
        assert_eq!(
            headers.get("x-tos-grant-full-control").map(String::as_str),
            Some("id=owner")
        );
        assert_eq!(
            headers.get("x-tos-grant-read").map(String::as_str),
            Some("id=reader")
        );
        assert_eq!(
            headers.get("x-tos-grant-write-acp").map(String::as_str),
            Some("id=writer")
        );
        assert!(!headers.contains_key("x-acl"));
        assert!(!headers.contains_key("x-grant-full-control"));
        assert!(!headers.contains_key("x-turbo-token"));
    }
}
