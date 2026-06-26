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

use std::collections::HashMap;

use crate::cli::low_level::*;
use crate::domain::bucket;
use crate::handler::common::{
    build_profile as build_runtime_profile, ensure_force_for_destructive,
    output_result as render_common_output, output_result_with_columns, parse_bucket_name,
};
use tos_core::agent::describe::{
    CommandDescription, CommandLayer, CommandParameter, ParameterLocation, RelatedCommands,
    RiskLevel,
};
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::client::TosClient;
use tos_core::infra::config::Profile;

/// 处理 bucket 子命令
pub async fn handle_bucket_command(
    global: &GlobalArgs,
    action: &Option<BucketAction>,
) -> Result<i32, CliError> {
    if global.describe {
        if let Some(action) = action {
            let desc = describe_bucket_action(action);
            render_common_output(global, &desc)?;
        } else {
            render_common_output(global, &describe_bucket_group())?;
        }
        return Ok(0);
    }

    let Some(action) = action else {
        return Err(CliError::ValidationError(
            "`ve-tos bucket` requires a subcommand; use `ve-tos bucket --help` or `ve-tos bucket --describe`".to_string(),
        ));
    };

    if global.dry_run {
        return handle_dry_run(global, action);
    }

    // [Review Fix #1] Pre-flight registry guard so destructive bucket
    // commands (e.g. `ve-tos bucket delete` without --force) fail with a stable
    // ValidationError BEFORE we try to build a runtime profile. Without this,
    // a missing region would mask the missing-force error.
    // [Review Fix #ForceGate] Pre-flight registry guard：添加 TTY 感知，
    // 交互式终端下放行至 handler 内部的 ensure_force_for_destructive 处理提示。
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let stderr_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let can_prompt = stdin_tty && stderr_tty && !global.quiet;
    if !can_prompt {
        if let BucketAction::Delete(args) = action {
            let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
            // [Review Fix #7] Non-interactive delete confirmation must be
            // validated before profile/client creation so config errors never
            // mask the critical safety gate.
            ensure_force_for_destructive(
                global,
                args.force || args.destroy,
                "ve-tos bucket delete",
                &format!("tos://{bucket_name}"),
            )?;
        }
    }
    let force_flag = match action {
        BucketAction::Delete(args) => args.force || args.destroy,
        _ => false,
    } || (global.yes && can_prompt)
        || can_prompt;
    if let Err(violation) = crate::registry::enforce_registry_guards(
        &format!("ve-tos bucket {}", bucket_action_name(action)),
        force_flag,
        stderr_tty,
    ) {
        return Err(CliError::ValidationError(format!(
            "{} requires --force (or run in an interactive terminal)",
            violation.command
        )));
    }

    // 构建 Profile（从全局参数 + 环境变量）并创建 Client
    let profile = build_runtime_profile(global)?;
    let client = TosClient::new(&profile, "tos")?;

    match action {
        BucketAction::Create(args) => handle_create(global, &client, args).await,
        BucketAction::Head(args) => handle_head(global, &client, args).await,
        BucketAction::Delete(args) => handle_delete(global, &client, args).await,
        BucketAction::List(args) => handle_list(global, &client, args).await,
        BucketAction::Stat(args) => handle_stat(global, &client, args).await,
        BucketAction::Info(args) => handle_info(global, &client, args).await,
        BucketAction::Location(args) => handle_location(global, &client, args).await,
    }
}

/// [Review Fix #1] Map a `BucketAction` to its registry command leaf so the
/// pre-flight guard can look up the correct `EffectiveCapability`.
fn bucket_action_name(action: &BucketAction) -> &'static str {
    match action {
        BucketAction::Create(_) => "create",
        BucketAction::Head(_) => "head",
        BucketAction::Delete(_) => "delete",
        BucketAction::List(_) => "list",
        BucketAction::Stat(_) => "stat",
        BucketAction::Info(_) => "info",
        BucketAction::Location(_) => "location",
    }
}

/// --dry-run 处理：不需要真实凭证，本地预览操作
fn handle_dry_run(global: &GlobalArgs, action: &BucketAction) -> Result<i32, CliError> {
    let (command, dry_run) = match action {
        BucketAction::Create(args) => {
            validate_bucket_create_args(args)?;
            let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
            let effective_region = args
                .region
                .clone()
                .or_else(|| global.region.clone())
                .unwrap_or_else(|| "<runtime profile region>".to_string());
            let mut plan = vec![
                format!("CREATE bucket '{}'", bucket_name),
                format!("Target region: {}", effective_region),
            ];
            push_plan_kv(
                &mut plan,
                "Storage class",
                Some(args.storage_class.as_str()),
            );
            push_plan_kv(&mut plan, "Bucket type", args.bucket_type.as_deref());
            push_plan_kv(&mut plan, "Project name", args.project_name.as_deref());
            if args.bucket_object_lock_enabled {
                plan.push("Enable bucket object lock".to_string());
            }
            push_plan_kv(&mut plan, "ACL", args.acl.as_deref());
            push_plan_kv(
                &mut plan,
                "Grant full control",
                args.grant_full_control.as_deref(),
            );
            push_plan_kv(&mut plan, "Grant read", args.grant_read.as_deref());
            push_plan_kv(
                &mut plan,
                "Grant read non-list",
                args.grant_read_non_list.as_deref(),
            );
            push_plan_kv(&mut plan, "Grant read ACP", args.grant_read_acp.as_deref());
            push_plan_kv(&mut plan, "Grant write", args.grant_write.as_deref());
            push_plan_kv(
                &mut plan,
                "Grant write ACP",
                args.grant_write_acp.as_deref(),
            );
            push_plan_kv(&mut plan, "AZ redundancy", args.az_redundancy.as_deref());
            push_plan_kv(&mut plan, "Tagging", args.tagging.as_deref());
            (
                "ve-tos bucket create",
                tos_core::agent::dryrun::DryRunResult {
                    action: "bucket create".to_string(),
                    dry_run: true,
                    impact: tos_core::agent::dryrun::Impact {
                        affected_objects: 0,
                        affected_bytes: 0,
                        risk_level: "low".to_string(),
                        estimated_duration: Some("< 1s".to_string()),
                        scanned_count: None,
                        preview_truncated: None,
                    },
                    plan,
                    warnings: vec![],
                    confirm_command: Some(build_bucket_create_confirm_command(args, &bucket_name)),
                },
            )
        }
        BucketAction::Delete(args) => {
            let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
            let mut plan = vec![format!("DELETE bucket '{}'", bucket_name)];
            if args.force || args.destroy {
                plan.push("Safety gate: confirmed via --force/--destroy".to_string());
            }
            (
                "ve-tos bucket delete",
                tos_core::agent::dryrun::DryRunResult {
                    action: "bucket delete".to_string(),
                    dry_run: true,
                    impact: tos_core::agent::dryrun::Impact {
                        affected_objects: 0,
                        affected_bytes: 0,
                        risk_level: "high".to_string(),
                        estimated_duration: Some("< 1s".to_string()),
                        scanned_count: None,
                        preview_truncated: None,
                    },
                    plan,
                    warnings: vec!["Bucket must be empty before deletion".to_string()],
                    confirm_command: Some(format!(
                        "ve-tos bucket delete tos://{} --force --confirm tos://{}",
                        bucket_name, bucket_name
                    )),
                },
            )
        }
        BucketAction::Head(args) => {
            let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
            (
                "ve-tos bucket head",
                tos_core::agent::dryrun::DryRunResult {
                    action: "bucket head".to_string(),
                    dry_run: true,
                    impact: tos_core::agent::dryrun::Impact {
                        affected_objects: 0,
                        affected_bytes: 0,
                        risk_level: "low".to_string(),
                        estimated_duration: Some("< 1s".to_string()),
                        scanned_count: None,
                        preview_truncated: None,
                    },
                    plan: vec![format!("HEAD bucket '{}' (read-only)", bucket_name)],
                    warnings: vec![],
                    confirm_command: Some(format!("ve-tos bucket head tos://{}", bucket_name)),
                },
            )
        }
        BucketAction::List(args) => {
            validate_bucket_list_args(args)?;
            let mut plan = vec!["LIST all buckets owned by current user (read-only)".to_string()];
            push_plan_kv(&mut plan, "Project filter", args.project_name.as_deref());
            push_plan_kv(&mut plan, "Bucket type filter", args.bucket_type.as_deref());
            (
                "ve-tos bucket list",
                tos_core::agent::dryrun::DryRunResult {
                    action: "bucket list".to_string(),
                    dry_run: true,
                    impact: tos_core::agent::dryrun::Impact {
                        affected_objects: 0,
                        affected_bytes: 0,
                        risk_level: "low".to_string(),
                        estimated_duration: Some("< 1s".to_string()),
                        scanned_count: None,
                        preview_truncated: None,
                    },
                    plan,
                    warnings: vec![],
                    confirm_command: Some(build_bucket_list_confirm_command(args)),
                },
            )
        }
        BucketAction::Stat(args) => {
            let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
            (
                "ve-tos bucket stat",
                tos_core::agent::dryrun::DryRunResult {
                    action: "bucket stat".to_string(),
                    dry_run: true,
                    impact: tos_core::agent::dryrun::Impact {
                        affected_objects: 0,
                        affected_bytes: 0,
                        risk_level: "low".to_string(),
                        estimated_duration: Some("< 1s".to_string()),
                        scanned_count: None,
                        preview_truncated: None,
                    },
                    plan: vec![format!(
                        "GET statistics for bucket '{}' (read-only)",
                        bucket_name
                    )],
                    warnings: vec![],
                    confirm_command: Some(format!("ve-tos bucket stat tos://{}", bucket_name)),
                },
            )
        }
        BucketAction::Info(args) => {
            let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
            (
                "ve-tos bucket info",
                tos_core::agent::dryrun::DryRunResult {
                    action: "bucket info".to_string(),
                    dry_run: true,
                    impact: tos_core::agent::dryrun::Impact {
                        affected_objects: 0,
                        affected_bytes: 0,
                        risk_level: "low".to_string(),
                        estimated_duration: Some("< 1s".to_string()),
                        scanned_count: None,
                        preview_truncated: None,
                    },
                    plan: vec![format!(
                        "GET detailed info for bucket '{}' (read-only)",
                        bucket_name
                    )],
                    warnings: vec![],
                    confirm_command: Some(format!("ve-tos bucket info tos://{}", bucket_name)),
                },
            )
        }
        BucketAction::Location(args) => {
            let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
            (
                "ve-tos bucket location",
                tos_core::agent::dryrun::DryRunResult {
                    action: "bucket location".to_string(),
                    dry_run: true,
                    impact: tos_core::agent::dryrun::Impact {
                        affected_objects: 0,
                        affected_bytes: 0,
                        risk_level: "low".to_string(),
                        estimated_duration: Some("< 1s".to_string()),
                        scanned_count: None,
                        preview_truncated: None,
                    },
                    plan: vec![format!(
                        "GET location for bucket '{}' (read-only)",
                        bucket_name
                    )],
                    warnings: vec![],
                    confirm_command: Some(format!("ve-tos bucket location tos://{}", bucket_name)),
                },
            )
        }
    };

    let envelope = Envelope::success(command, dry_run);
    output_result(global, &envelope)?;
    Ok(0)
}

async fn handle_create(
    global: &GlobalArgs,
    client: &TosClient,
    args: &BucketCreateArgs,
) -> Result<i32, CliError> {
    validate_bucket_create_args(args)?;
    let profile = build_bucket_create_profile(global, args)?;
    let override_client;
    let effective_client = if args.region.is_some() {
        // [Review Fix #7] `ve-tos bucket create --region` 必须真正影响签名地域和目标服务端点，
        // 不能只出现在 help 中却被运行时忽略。
        override_client = Some(TosClient::new(&profile, "tos")?);
        override_client.as_ref().unwrap()
    } else {
        client
    };

    let req = bucket::CreateBucketRequest {
        bucket: parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?,
        storage_class: if args.storage_class == "STANDARD" {
            None
        } else {
            Some(args.storage_class.clone())
        },
        acl: args.acl.clone(),
        grant_full_control: args.grant_full_control.clone(),
        grant_read: args.grant_read.clone(),
        grant_read_non_list: args.grant_read_non_list.clone(),
        grant_read_acp: args.grant_read_acp.clone(),
        grant_write: args.grant_write.clone(),
        grant_write_acp: args.grant_write_acp.clone(),
        az_redundancy: args.az_redundancy.clone(),
        bucket_type: args.bucket_type.clone(),
        bucket_object_lock_enabled: args.bucket_object_lock_enabled.then_some(true),
        tagging: args.tagging.clone(),
        project_name: args.project_name.clone(),
    };

    let result = bucket::create_bucket(effective_client, &req).await?;
    output_result(global, &result)?;
    Ok(0)
}

async fn handle_head(
    global: &GlobalArgs,
    client: &TosClient,
    args: &BucketHeadArgs,
) -> Result<i32, CliError> {
    let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
    let result = bucket::head_bucket(client, &bucket_name).await?;
    // [Review Fix #FmtUni] 单对象详情走统一路径，table/csv 复用 render_value 中的
    // `field/value` 模板（snake_case，与 JSON 一致）。原先手写的 `Field/Value`
    // 列头与全局 snake_case 约束冲突，移除。
    render_common_output(global, &result)?;
    Ok(0)
}

async fn handle_delete(
    global: &GlobalArgs,
    client: &TosClient,
    args: &BucketDeleteArgs,
) -> Result<i32, CliError> {
    let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
    ensure_force_for_destructive(
        global,
        args.force || args.destroy,
        "ve-tos bucket delete",
        &bucket_name,
    )?;
    let result = bucket::delete_bucket(client, &bucket_name).await?;
    output_result(global, &result)?;
    Ok(0)
}

async fn handle_list(
    global: &GlobalArgs,
    client: &TosClient,
    args: &BucketListArgs,
) -> Result<i32, CliError> {
    let result = bucket::list_buckets(
        client,
        args.project_name.as_deref(),
        args.bucket_type.as_deref(),
    )
    .await?;
    // [Review Fix #FmtUni] list-like 命令通过声明列接入统一渲染：
    //   - JSON/YAML/XML/Markdown 完全保持 Envelope schema 不变
    //   - Table/CSV 列头 snake_case，列顺序声明式（name/location/bucket_type/creation_date）
    //   - footer 由 render_value 从 Envelope.pagination 自动衍生为 `Total: N`
    output_result_with_columns(global, &result, Some(BUCKET_LIST_TABLE_COLUMNS))?;
    Ok(0)
}

const BUCKET_LIST_TABLE_COLUMNS: &[&str] = &["name", "location", "bucket_type", "creation_date"];

async fn handle_stat(
    global: &GlobalArgs,
    client: &TosClient,
    args: &BucketStatArgs,
) -> Result<i32, CliError> {
    let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
    let result = bucket::get_bucket_stat(client, &bucket_name).await?;
    output_result(global, &result)?;
    Ok(0)
}

async fn handle_info(
    global: &GlobalArgs,
    client: &TosClient,
    args: &BucketInfoArgs,
) -> Result<i32, CliError> {
    let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
    let result = bucket::get_bucket_info(client, &bucket_name).await?;
    output_result(global, &result)?;
    Ok(0)
}

async fn handle_location(
    global: &GlobalArgs,
    client: &TosClient,
    args: &BucketLocationArgs,
) -> Result<i32, CliError> {
    let bucket_name = parse_bucket_arg(args.uri.as_deref(), args.bucket_name.as_deref())?;
    let result = bucket::get_bucket_location(client, &bucket_name).await?;
    output_result(global, &result)?;
    Ok(0)
}

// ===== --describe 支持 =====

fn describe_bucket_action(action: &BucketAction) -> CommandDescription {
    match action {
        BucketAction::Create(_) => CommandDescription {
            command: "ve-tos bucket create".to_string(),
            layer: CommandLayer::LowLevel,
            api: Some("CreateBucket".to_string()),
            description: "Create a new TOS bucket".to_string(),
            risk_level: RiskLevel::Low,
            supports_dry_run: true,
            supports_pipe: false,
            parameters: Some(vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter("x-tos-storage-class", ParameterLocation::Header, false, "Storage class. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE"),
                parameter("x-tos-bucket-type", ParameterLocation::Header, false, "Bucket type"),
                parameter("x-tos-project-name", ParameterLocation::Header, false, "Project name"),
                parameter("x-tos-bucket-object-lock-enabled", ParameterLocation::Header, false, "Enable bucket object lock"),
                parameter("x-tos-acl", ParameterLocation::Header, false, "Bucket ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control"),
                parameter("x-tos-grant-full-control", ParameterLocation::Header, false, "Grant full control"),
                parameter("x-tos-grant-read", ParameterLocation::Header, false, "Grant read permission"),
                parameter("x-tos-grant-read-non-list", ParameterLocation::Header, false, "Grant read without list permission"),
                parameter("x-tos-grant-read-acp", ParameterLocation::Header, false, "Grant read ACP permission"),
                parameter("x-tos-grant-write", ParameterLocation::Header, false, "Grant write permission"),
                parameter("x-tos-grant-write-acp", ParameterLocation::Header, false, "Grant write ACP permission"),
                parameter("x-tos-az-redundancy", ParameterLocation::Header, false, "AZ redundancy. Allowed: single-az, multi-az"),
                parameter("x-tos-tagging", ParameterLocation::Header, false, "Object tags"),
            ]),
            scenario_routing: Some(HashMap::from([
                (
                    "Create bucket (URI)".to_string(),
                    "ve-tos bucket create tos://my-bucket --storage-class STANDARD".to_string(),
                ),
                (
                    "Create bucket (flags)".to_string(),
                    "ve-tos bucket create --bucket my-bucket --region cn-beijing".to_string(),
                ),
                (
                    "Create archive bucket".to_string(),
                    "ve-tos bucket create tos://archive-bucket --storage-class ARCHIVE".to_string(),
                ),
            ])),
            related_commands: None,
            low_level_apis: None,
            ..Default::default()
        },
        BucketAction::Head(_) => CommandDescription {
            command: "ve-tos bucket head".to_string(),
            layer: CommandLayer::LowLevel,
            api: Some("HeadBucket".to_string()),
            description: "Get bucket metadata (region, storage class, etc.)".to_string(),
            risk_level: RiskLevel::Low,
            supports_dry_run: true,
            supports_pipe: false,
            parameters: Some(vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )]),
            scenario_routing: Some(HashMap::from([(
                "Inspect bucket".to_string(),
                "ve-tos bucket head tos://my-bucket".to_string(),
            )])),
            related_commands: None,
            low_level_apis: None,
            ..Default::default()
        },
        BucketAction::Delete(_) => CommandDescription {
            command: "ve-tos bucket delete".to_string(),
            layer: CommandLayer::LowLevel,
            api: Some("DeleteBucket".to_string()),
            description: "Delete a bucket (must be empty; requires --force for execution)".to_string(),
            risk_level: RiskLevel::High,
            supports_dry_run: true,
            supports_pipe: false,
            parameters: Some(vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter("force", ParameterLocation::Flag, false, "Required for destructive execution"),
                parameter("destroy", ParameterLocation::Flag, false, "Alias for --force (permanent deletion intent)"),
            ]),
            scenario_routing: Some(HashMap::from([
                (
                    "Confirm delete (requires --force + --confirm)".to_string(),
                    "ve-tos bucket delete tos://my-bucket --force --confirm tos://my-bucket"
                        .to_string(),
                ),
                (
                    "Preview delete".to_string(),
                    "ve-tos bucket delete tos://my-bucket --dry-run".to_string(),
                ),
            ])),
            related_commands: Some(RelatedCommands {
                high_level: Some(
                    "ve-tos rm tos://bucket/ --recursive --force --confirm tos://bucket/ (empty contents first, then delete bucket)".to_string(),
                ),
                low_level: None,
            }),
            low_level_apis: None,
            ..Default::default()
        },
        BucketAction::List(_) => CommandDescription {
            command: "ve-tos bucket list".to_string(),
            layer: CommandLayer::LowLevel,
            api: Some("ListBuckets".to_string()),
            description: "List all buckets owned by the current user".to_string(),
            risk_level: RiskLevel::Low,
            supports_dry_run: true,
            supports_pipe: true,
            parameters: Some(vec![
                parameter("x-tos-project-name", ParameterLocation::Header, false, "Filter by project name"),
                parameter("x-tos-bucket-type", ParameterLocation::Header, false, "Filter by bucket type"),
            ]),
            scenario_routing: Some(HashMap::from([
                (
                    "List all buckets".to_string(),
                    "ve-tos bucket list".to_string(),
                ),
                (
                    "JSON output".to_string(),
                    "ve-tos bucket list --output json".to_string(),
                ),
            ])),
            related_commands: Some(RelatedCommands {
                high_level: Some("ve-tos ls (list buckets or objects within a bucket)".to_string()),
                low_level: None,
            }),
            low_level_apis: None,
            ..Default::default()
        },
        BucketAction::Stat(_) => CommandDescription {
            command: "ve-tos bucket stat".to_string(),
            layer: CommandLayer::LowLevel,
            api: Some("GetBucketStat".to_string()),
            description: "Get bucket statistics (object count, storage usage)".to_string(),
            risk_level: RiskLevel::Low,
            supports_dry_run: true,
            supports_pipe: false,
            parameters: Some(vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter("stat", ParameterLocation::Query, true, "Fixed marker query"),
            ]),
            scenario_routing: Some(HashMap::from([(
                "Inspect bucket statistics".to_string(),
                "ve-tos bucket stat tos://my-bucket".to_string(),
            )])),
            related_commands: None,
            low_level_apis: None,
            ..Default::default()
        },
        BucketAction::Info(_) => CommandDescription {
            command: "ve-tos bucket info".to_string(),
            layer: CommandLayer::LowLevel,
            api: Some("GetBucketInfo".to_string()),
            description: "Get detailed bucket information".to_string(),
            risk_level: RiskLevel::Low,
            supports_dry_run: true,
            supports_pipe: false,
            parameters: Some(vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter("bucketInfo", ParameterLocation::Query, true, "Fixed marker query"),
            ]),
            scenario_routing: Some(HashMap::from([(
                "Inspect bucket details".to_string(),
                "ve-tos bucket info tos://my-bucket".to_string(),
            )])),
            related_commands: None,
            low_level_apis: None,
            ..Default::default()
        },
        BucketAction::Location(_) => CommandDescription {
            command: "ve-tos bucket location".to_string(),
            layer: CommandLayer::LowLevel,
            api: Some("GetBucketLocation".to_string()),
            description: "Get bucket location (region and endpoint)".to_string(),
            risk_level: RiskLevel::Low,
            supports_dry_run: true,
            supports_pipe: false,
            parameters: Some(vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter("location", ParameterLocation::Query, true, "Fixed marker query"),
            ]),
            scenario_routing: Some(HashMap::from([(
                "Inspect bucket location".to_string(),
                "ve-tos bucket location tos://my-bucket".to_string(),
            )])),
            related_commands: None,
            low_level_apis: None,
            ..Default::default()
        },
    }
}

pub fn describe_bucket_group() -> serde_json::Value {
    serde_json::json!({
        "command": "ve-tos bucket",
        "kind": "command_group",
        "layer": "low_level",
        "description": "Bucket Core APIs",
        "supports_help": true,
        "supports_describe": true,
        "subcommands": [
            {"name": "create", "api": "CreateBucket", "risk_level": "low", "description": "Create a new bucket"},
            {"name": "head", "api": "HeadBucket", "risk_level": "low", "description": "Get bucket metadata"},
            {"name": "delete", "api": "DeleteBucket", "risk_level": "high", "description": "Delete a bucket"},
            {"name": "list", "api": "ListBuckets", "risk_level": "low", "description": "List all buckets"},
            {"name": "stat", "api": "GetBucketStat", "risk_level": "low", "description": "Get bucket statistics"},
            {"name": "info", "api": "GetBucketInfo", "risk_level": "low", "description": "Get bucket detailed information"},
            {"name": "location", "api": "GetBucketLocation", "risk_level": "low", "description": "Get bucket location"}
        ]
    })
}

// ===== 辅助函数 =====

fn build_bucket_create_profile(
    global: &GlobalArgs,
    args: &BucketCreateArgs,
) -> Result<Profile, CliError> {
    let mut profile = build_runtime_profile(global)?;
    if let Some(region) = &args.region {
        profile.region = Some(region.clone());
    }
    Ok(profile)
}

fn parse_bucket_arg(uri: Option<&str>, bucket_name: Option<&str>) -> Result<String, CliError> {
    let raw_bucket = uri.or(bucket_name).ok_or_else(|| {
        CliError::ValidationError("missing bucket target: provide tos://bucket or --bucket".into())
    })?;
    // [Review Fix #2] Keep the documented dual spelling in one parser so CLI, dry-run, and execution agree.
    Ok(parse_bucket_name(raw_bucket))
}

fn validate_bucket_create_args(args: &BucketCreateArgs) -> Result<(), CliError> {
    validate_storage_class(&args.storage_class)?;
    validate_optional_bucket_type(args.bucket_type.as_deref())?;
    validate_optional_az_redundancy(args.az_redundancy.as_deref())?;
    validate_optional_non_empty("project-name", args.project_name.as_deref())
}

fn validate_bucket_list_args(args: &BucketListArgs) -> Result<(), CliError> {
    validate_optional_bucket_type(args.bucket_type.as_deref())
}

fn validate_storage_class(value: &str) -> Result<(), CliError> {
    validate_allowed_value(
        "storage-class",
        value,
        &[
            "STANDARD",
            "IA",
            "ARCHIVE_FR",
            "INTELLIGENT_TIERING",
            "COLD_ARCHIVE",
            "ARCHIVE",
            "DEEP_COLD_ARCHIVE",
        ],
    )
}

fn validate_optional_bucket_type(value: Option<&str>) -> Result<(), CliError> {
    if let Some(value) = value {
        validate_allowed_value("bucket-type", value, &["fns", "hns"])?;
    }
    Ok(())
}

fn validate_optional_az_redundancy(value: Option<&str>) -> Result<(), CliError> {
    if let Some(value) = value {
        validate_allowed_value("az-redundancy", value, &["single-az", "multi-az"])?;
    }
    Ok(())
}

fn validate_optional_non_empty(name: &str, value: Option<&str>) -> Result<(), CliError> {
    if let Some(value) = value {
        if value.trim().is_empty() {
            return Err(CliError::ValidationError(format!(
                "--{} must not be empty",
                name
            )));
        }
    }
    Ok(())
}

fn validate_allowed_value(name: &str, value: &str, allowed: &[&str]) -> Result<(), CliError> {
    if allowed.iter().any(|candidate| candidate == &value) {
        return Ok(());
    }
    Err(CliError::ValidationError(format!(
        "invalid --{} '{}'; allowed values: {}",
        name,
        value,
        allowed.join(", ")
    )))
}

fn build_bucket_create_confirm_command(args: &BucketCreateArgs, bucket_name: &str) -> String {
    let mut parts = vec![
        "tos".to_string(),
        "bucket".to_string(),
        "create".to_string(),
        format!("tos://{}", bucket_name),
    ];
    push_optional_arg(&mut parts, "--region", args.region.as_deref());
    push_optional_arg(
        &mut parts,
        "--storage-class",
        Some(args.storage_class.as_str()),
    );
    push_optional_arg(&mut parts, "--bucket-type", args.bucket_type.as_deref());
    push_optional_arg(&mut parts, "--project-name", args.project_name.as_deref());
    if args.bucket_object_lock_enabled {
        parts.push("--bucket-object-lock-enabled".to_string());
    }
    push_optional_arg(&mut parts, "--acl", args.acl.as_deref());
    push_optional_arg(
        &mut parts,
        "--grant-full-control",
        args.grant_full_control.as_deref(),
    );
    push_optional_arg(&mut parts, "--grant-read", args.grant_read.as_deref());
    push_optional_arg(
        &mut parts,
        "--grant-read-non-list",
        args.grant_read_non_list.as_deref(),
    );
    push_optional_arg(
        &mut parts,
        "--grant-read-acp",
        args.grant_read_acp.as_deref(),
    );
    push_optional_arg(&mut parts, "--grant-write", args.grant_write.as_deref());
    push_optional_arg(
        &mut parts,
        "--grant-write-acp",
        args.grant_write_acp.as_deref(),
    );
    push_optional_arg(&mut parts, "--az-redundancy", args.az_redundancy.as_deref());
    push_optional_arg(&mut parts, "--tagging", args.tagging.as_deref());
    parts.join(" ")
}

fn build_bucket_list_confirm_command(args: &BucketListArgs) -> String {
    let mut parts = vec![
        "ve-tos".to_string(),
        "bucket".to_string(),
        "list".to_string(),
    ];
    push_optional_arg(&mut parts, "--project-name", args.project_name.as_deref());
    push_optional_arg(&mut parts, "--bucket-type", args.bucket_type.as_deref());
    parts.join(" ")
}

fn push_plan_kv(plan: &mut Vec<String>, label: &str, value: Option<&str>) {
    if let Some(value) = value {
        plan.push(format!("{label}: {value}"));
    }
}

fn push_optional_arg(parts: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        parts.push(flag.to_string());
        parts.push(value.to_string());
    }
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

/// 通用输出函数
fn output_result<T: serde::Serialize>(global: &GlobalArgs, result: &T) -> Result<(), CliError> {
    render_common_output(global, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use tos_core::infra::config::ConfigFile;
    use tos_core::infra::config::TosOverride;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn temp_home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tos-bucket-profile-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set_env_var(key: &str, value: impl AsRef<std::ffi::OsStr>) {
        // [Review Fix #6] 通过测试锁串行化环境变量修改，避免并发测试相互污染。
        unsafe { std::env::set_var(key, value) }
    }

    fn remove_env_var(key: &str) {
        unsafe { std::env::remove_var(key) }
    }

    fn restore_env_var(key: &str, original: Option<OsString>) {
        match original {
            Some(value) => set_env_var(key, value),
            None => remove_env_var(key),
        }
    }

    #[test]
    fn build_profile_prefers_cli_then_config_then_env() {
        let _guard = env_lock().lock().unwrap();
        let home = temp_home();

        let original_home = std::env::var_os("HOME");
        let original_region = std::env::var_os("TOS_REGION");
        let original_endpoint = std::env::var_os("TOS_ENDPOINT");
        let original_control_endpoint = std::env::var_os("TOS_CONTROL_ENDPOINT");
        let original_access_key = std::env::var_os("TOS_ACCESS_KEY");
        let original_secret_key = std::env::var_os("TOS_SECRET_KEY");

        set_env_var("HOME", &home);
        set_env_var("TOS_REGION", "env-region");
        set_env_var("TOS_ENDPOINT", "env-endpoint");
        set_env_var("TOS_CONTROL_ENDPOINT", "env-control-endpoint");
        remove_env_var("TOS_ACCESS_KEY");
        remove_env_var("TOS_SECRET_KEY");

        let mut config = ConfigFile::default();
        let profile = config.get_or_insert_profile("staging");
        profile.region = Some("config-region".to_string());
        profile.access_key_id = Some("config-ak".to_string());
        profile.secret_access_key = Some("config-sk".to_string());
        profile.tos = Some(TosOverride {
            endpoint: Some("config-endpoint".to_string()),
            control_endpoint: Some("config-control-endpoint".to_string()),
            ..Default::default()
        });
        config.save().unwrap();

        let global = GlobalArgs {
            profile: "staging".to_string(),
            config_path: None,
            region: Some("cli-region".to_string()),
            endpoint: None,
            psm: None,
            idc: None,
            cluster: None,
            addr_family: None,
            control_endpoint: Some("cli-control-endpoint".to_string()),
            account_id: None,
            output: None,
            query: None,
            dry_run: false,
            describe: false,
            no_color: false,
            verbose: false,
            quiet: false,
            trace_dir: None,
            trace_redact: "strict".to_string(),
            yes: false,
            confirm: None,
        };

        let merged = build_runtime_profile(&global).unwrap();

        assert_eq!(merged.region.as_deref(), Some("cli-region"));
        // Config endpoint (from `[staging.tos]`) wins over the env-supplied
        // endpoint because the priority is CLI > Config > Env. The CLI did not
        // pass --endpoint here, so the config value is used.
        assert_eq!(merged.endpoint.as_deref(), Some("config-endpoint"));
        assert_eq!(
            merged.control_endpoint.as_deref(),
            Some("cli-control-endpoint")
        );
        assert_eq!(merged.access_key_id.as_deref(), Some("config-ak"));
        assert_eq!(merged.secret_access_key.as_deref(), Some("config-sk"));

        restore_env_var("HOME", original_home);
        restore_env_var("TOS_REGION", original_region);
        restore_env_var("TOS_ENDPOINT", original_endpoint);
        restore_env_var("TOS_CONTROL_ENDPOINT", original_control_endpoint);
        restore_env_var("TOS_ACCESS_KEY", original_access_key);
        restore_env_var("TOS_SECRET_KEY", original_secret_key);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn build_bucket_create_profile_prefers_subcommand_region_override() {
        let _guard = env_lock().lock().unwrap();
        let home = temp_home();
        let original_home = std::env::var_os("HOME");
        let original_region = std::env::var_os("TOS_REGION");
        let original_endpoint = std::env::var_os("TOS_ENDPOINT");
        let original_control_endpoint = std::env::var_os("TOS_CONTROL_ENDPOINT");
        let original_access_key_id = std::env::var_os("TOS_ACCESS_KEY");
        let original_secret_access_key = std::env::var_os("TOS_SECRET_KEY");

        // [Review Fix #7] Isolate profile merge test from the developer's encrypted local config.
        set_env_var("HOME", &home);
        remove_env_var("TOS_REGION");
        remove_env_var("TOS_ENDPOINT");
        remove_env_var("TOS_CONTROL_ENDPOINT");
        remove_env_var("TOS_ACCESS_KEY");
        remove_env_var("TOS_SECRET_KEY");

        let global = GlobalArgs {
            profile: "default".to_string(),
            config_path: None,
            region: Some("cli-region".to_string()),
            endpoint: Some("https://tos-cn-beijing.volces.com".to_string()),
            psm: None,
            idc: None,
            cluster: None,
            addr_family: None,
            control_endpoint: None,
            account_id: None,
            output: None,
            query: None,
            dry_run: false,
            describe: false,
            no_color: false,
            verbose: false,
            quiet: false,
            trace_dir: None,
            trace_redact: "strict".to_string(),
            yes: false,
            confirm: None,
        };
        let args = BucketCreateArgs {
            uri: Some("demo-bucket".to_string()),
            bucket_name: None,
            region: Some("cmd-region".to_string()),
            storage_class: "STANDARD".to_string(),
            bucket_type: None,
            project_name: None,
            bucket_object_lock_enabled: false,
            acl: None,
            grant_full_control: None,
            grant_read: None,
            grant_read_non_list: None,
            grant_read_acp: None,
            grant_write: None,
            grant_write_acp: None,
            az_redundancy: None,
            tagging: None,
        };

        let merged = build_bucket_create_profile(&global, &args).unwrap();

        assert_eq!(merged.region.as_deref(), Some("cmd-region"));
        assert_eq!(
            merged.endpoint.as_deref(),
            Some("https://tos-cn-beijing.volces.com")
        );

        restore_env_var("HOME", original_home);
        restore_env_var("TOS_REGION", original_region);
        restore_env_var("TOS_ENDPOINT", original_endpoint);
        restore_env_var("TOS_CONTROL_ENDPOINT", original_control_endpoint);
        restore_env_var("TOS_ACCESS_KEY", original_access_key_id);
        restore_env_var("TOS_SECRET_KEY", original_secret_access_key);
        let _ = std::fs::remove_dir_all(home);
    }
}
