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
    build_profile, build_query, ensure_force_for_destructive, output_result, parse_kv_pairs,
    read_json_input,
};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use reqwest::Method;
use serde_json::{json, Value};
use tos_core::agent::describe::{
    CommandDescription, CommandLayer, CommandParameter, ParameterLocation, RiskLevel,
};
use tos_core::agent::dryrun::{DryRunResult, Impact};
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::client::TosClient;

#[derive(Debug)]
struct BucketConfigOperation {
    command: &'static str,
    api: &'static str,
    description: &'static str,
    method: Method,
    bucket: String,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
    risk: RiskLevel,
    parameters: Vec<CommandParameter>,
    supports_pipe: bool,
    destructive: bool,
    force: bool,
}

pub async fn handle_quota_command(
    global: &GlobalArgs,
    action: &Option<QuotaAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos quota",
        describe_group(
            "ve-tos quota",
            "Bucket storage quota",
            &[("get", "Get bucket quota"), ("set", "Set bucket quota")],
        ),
        quota_operation,
    )
    .await
}

pub async fn handle_policy_command(
    global: &GlobalArgs,
    action: &Option<PolicyAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos policy",
        describe_group(
            "ve-tos policy",
            "Bucket policy management",
            &[
                ("get", "Get bucket policy"),
                ("set", "Set bucket policy"),
                ("delete", "Delete bucket policy"),
            ],
        ),
        policy_operation,
    )
    .await
}

pub async fn handle_lifecycle_command(
    global: &GlobalArgs,
    action: &Option<LifecycleAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos lifecycle",
        describe_group(
            "ve-tos lifecycle",
            "Lifecycle rule management",
            &[
                ("get", "Get lifecycle rules"),
                ("set", "Set lifecycle rules"),
                ("delete", "Delete lifecycle rules"),
            ],
        ),
        lifecycle_operation,
    )
    .await
}

pub async fn handle_storageclass_command(
    global: &GlobalArgs,
    action: &Option<StorageclassAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos storageclass",
        describe_group(
            "ve-tos storageclass",
            "Bucket default storage class",
            &[("set", "Set bucket default storage class")],
        ),
        storageclass_operation,
    )
    .await
}

pub async fn handle_cors_command(
    global: &GlobalArgs,
    action: &Option<CorsAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos cors",
        describe_group(
            "ve-tos cors",
            "Bucket CORS configuration",
            &[
                ("get", "Get CORS configuration"),
                ("set", "Set CORS configuration"),
                ("delete", "Delete CORS configuration"),
            ],
        ),
        cors_operation,
    )
    .await
}

pub async fn handle_versioning_command(
    global: &GlobalArgs,
    action: &Option<VersioningAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos versioning",
        describe_group(
            "ve-tos versioning",
            "Bucket versioning configuration",
            &[
                ("get", "Get versioning status"),
                ("set", "Set versioning status"),
            ],
        ),
        versioning_operation,
    )
    .await
}

pub async fn handle_replication_command(
    global: &GlobalArgs,
    action: &Option<ReplicationAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos replication",
        describe_group(
            "ve-tos replication",
            "Cross-region replication",
            &[
                ("get", "Get replication configuration"),
                ("set", "Set replication configuration"),
                ("delete", "Delete replication configuration"),
            ],
        ),
        replication_operation,
    )
    .await
}

pub async fn handle_encryption_command(
    global: &GlobalArgs,
    action: &Option<EncryptionAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos encryption",
        describe_group(
            "ve-tos encryption",
            "Bucket encryption configuration",
            &[
                ("get", "Get bucket encryption"),
                ("set", "Set bucket encryption"),
                ("delete", "Delete bucket encryption"),
            ],
        ),
        encryption_operation,
    )
    .await
}

pub async fn handle_tagging_command(
    global: &GlobalArgs,
    action: &Option<TaggingAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos tagging",
        describe_group(
            "ve-tos tagging",
            "Bucket tagging management",
            &[
                ("get", "Get bucket tagging"),
                ("set", "Set bucket tagging"),
                ("delete", "Delete bucket tagging"),
            ],
        ),
        tagging_operation,
    )
    .await
}

pub async fn handle_acl_command(
    global: &GlobalArgs,
    action: &Option<AclAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos acl",
        describe_group(
            "ve-tos acl",
            "Bucket ACL management",
            &[("get", "Get bucket ACL"), ("set", "Set bucket ACL")],
        ),
        acl_operation,
    )
    .await
}

pub async fn handle_rename_command(
    global: &GlobalArgs,
    action: &Option<RenameAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos rename",
        describe_group(
            "ve-tos rename",
            "Bucket RenameObject configuration",
            &[
                ("get", "Get RenameObject configuration"),
                ("set", "Enable RenameObject"),
                ("delete", "Disable RenameObject"),
            ],
        ),
        rename_operation,
    )
    .await
}

pub async fn handle_access_monitor_command(
    global: &GlobalArgs,
    action: &Option<AccessMonitorAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos access-monitor",
        describe_group(
            "ve-tos access-monitor",
            "Bucket access monitor configuration",
            &[
                ("get", "Get access monitor configuration"),
                ("set", "Set access monitor configuration"),
            ],
        ),
        access_monitor_operation,
    )
    .await
}

pub async fn handle_payment_command(
    global: &GlobalArgs,
    action: &Option<PaymentAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos payment",
        describe_group(
            "ve-tos payment",
            "Bucket requester pays configuration",
            &[
                ("get", "Get requester pays configuration"),
                ("set", "Set requester pays configuration"),
            ],
        ),
        payment_operation,
    )
    .await
}

pub async fn handle_trash_command(
    global: &GlobalArgs,
    action: &Option<TrashAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos trash",
        describe_group(
            "ve-tos trash",
            "Bucket trash configuration",
            &[
                ("get", "Get trash configuration"),
                ("set", "Set trash configuration"),
            ],
        ),
        trash_operation,
    )
    .await
}

pub async fn handle_logging_command(
    global: &GlobalArgs,
    action: &Option<LoggingAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos logging",
        describe_group(
            "ve-tos logging",
            "Bucket access logging configuration",
            &[
                ("get", "Get bucket access logging configuration"),
                ("set", "Set or disable bucket access logging configuration"),
            ],
        ),
        logging_operation,
    )
    .await
}

pub async fn handle_intelligent_tiering_command(
    global: &GlobalArgs,
    action: &Option<IntelligentTieringAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos intelligent-tiering",
        describe_group(
            "ve-tos intelligent-tiering",
            "Bucket intelligent tiering configuration",
            &[
                ("get", "Get intelligent tiering configuration"),
                ("set", "Set intelligent tiering configuration"),
            ],
        ),
        intelligent_tiering_operation,
    )
    .await
}

pub async fn handle_transfer_acceleration_command(
    global: &GlobalArgs,
    action: &Option<TransferAccelerationAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos transfer-acceleration",
        describe_group(
            "ve-tos transfer-acceleration",
            "Bucket transfer acceleration configuration",
            &[
                ("get", "Get transfer acceleration configuration"),
                ("set", "Set transfer acceleration configuration"),
            ],
        ),
        transfer_acceleration_operation,
    )
    .await
}

pub async fn handle_cdn_notification_command(
    global: &GlobalArgs,
    action: &Option<CdnNotificationAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos cdn-notification",
        describe_group(
            "ve-tos cdn-notification",
            "Bucket CDN notification configuration",
            &[
                ("get", "Get CDN notification configuration"),
                ("set", "Set CDN notification configuration"),
                ("delete", "Delete CDN notification configuration"),
            ],
        ),
        cdn_notification_operation,
    )
    .await
}

pub async fn handle_https_config_command(
    global: &GlobalArgs,
    action: &Option<HttpsConfigAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos https-config",
        describe_group(
            "ve-tos https-config",
            "Bucket HTTPS/TLS configuration",
            &[
                ("get", "Get HTTPS/TLS configuration"),
                ("set", "Set HTTPS/TLS configuration"),
            ],
        ),
        https_config_operation,
    )
    .await
}

pub async fn handle_pay_by_traffic_command(
    global: &GlobalArgs,
    action: &Option<PayByTrafficAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos pay-by-traffic",
        describe_group(
            "ve-tos pay-by-traffic",
            "Bucket pay-by-traffic configuration",
            &[
                ("get", "Get pay-by-traffic configuration"),
                ("set", "Set pay-by-traffic configuration"),
            ],
        ),
        pay_by_traffic_operation,
    )
    .await
}

pub async fn handle_max_age_command(
    global: &GlobalArgs,
    action: &Option<MaxAgeAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos max-age",
        describe_group(
            "ve-tos max-age",
            "Bucket max-age cache configuration",
            &[
                ("get", "Get max-age configuration"),
                ("set", "Set max-age configuration"),
                ("delete", "Delete max-age configuration"),
            ],
        ),
        max_age_operation,
    )
    .await
}

pub async fn handle_redundancy_transition_command(
    global: &GlobalArgs,
    action: &Option<RedundancyTransitionAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos redundancy-transition",
        describe_group(
            "ve-tos redundancy-transition",
            "Bucket data redundancy transition",
            &[
                ("create", "Create redundancy transition task"),
                ("delete", "Delete redundancy transition task"),
                ("get", "Get redundancy transition task"),
                ("list", "List redundancy transition tasks"),
                ("get-remaining-time", "Get estimated remaining time"),
            ],
        ),
        redundancy_transition_operation,
    )
    .await
}

pub async fn handle_custom_domain_command(
    global: &GlobalArgs,
    action: &Option<CustomDomainAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos custom-domain",
        describe_group(
            "ve-tos custom-domain",
            "Bucket custom domain binding",
            &[
                ("set", "Set custom domain binding"),
                ("delete", "Delete custom domain binding"),
                ("list", "List custom domain bindings"),
            ],
        ),
        custom_domain_operation,
    )
    .await
}

pub async fn handle_notification_command(
    global: &GlobalArgs,
    action: &Option<NotificationAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos notification",
        describe_group(
            "ve-tos notification",
            "Bucket event notification configuration (notification_v2)",
            &[
                (
                    "get",
                    "Get event notification configuration (notification_v2)",
                ),
                (
                    "set",
                    "Set event notification configuration (notification_v2)",
                ),
            ],
        ),
        notification_operation,
    )
    .await
}

pub async fn handle_website_command(
    global: &GlobalArgs,
    action: &Option<WebsiteAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos website",
        describe_group(
            "ve-tos website",
            "Bucket static website hosting",
            &[
                ("get", "Get website configuration"),
                ("set", "Set website configuration"),
                ("delete", "Delete website configuration"),
            ],
        ),
        website_operation,
    )
    .await
}

pub async fn handle_mirror_command(
    global: &GlobalArgs,
    action: &Option<MirrorAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos mirror",
        describe_group(
            "ve-tos mirror",
            "Bucket mirror back-to-source rules",
            &[
                ("get", "Get mirror back-to-source rules"),
                ("set", "Set mirror back-to-source rules"),
                ("delete", "Delete mirror back-to-source rules"),
            ],
        ),
        mirror_operation,
    )
    .await
}

pub async fn handle_inventory_command(
    global: &GlobalArgs,
    action: &Option<InventoryAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos inventory",
        describe_group(
            "ve-tos inventory",
            "Bucket inventory configuration",
            &[
                ("get", "Get inventory configuration"),
                ("set", "Set inventory configuration"),
                ("delete", "Delete inventory configuration"),
                ("list", "List inventory configurations"),
            ],
        ),
        inventory_operation,
    )
    .await
}

pub async fn handle_real_time_log_command(
    global: &GlobalArgs,
    action: &Option<RealTimeLogAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos real-time-log",
        describe_group(
            "ve-tos real-time-log",
            "Bucket real-time log configuration",
            &[
                ("get", "Get real-time log configuration"),
                ("set", "Set real-time log configuration"),
                ("delete", "Delete real-time log configuration"),
            ],
        ),
        real_time_log_operation,
    )
    .await
}

pub async fn handle_worm_command(
    global: &GlobalArgs,
    action: &Option<WormAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        action,
        "ve-tos worm",
        describe_group(
            "ve-tos worm",
            "Bucket object lock configuration",
            &[
                ("get", "Get object lock configuration"),
                ("set", "Set object lock configuration"),
            ],
        ),
        worm_operation,
    )
    .await
}

async fn handle_group<A, F>(
    global: &GlobalArgs,
    action: &Option<A>,
    group_name: &str,
    group_description: Value,
    op_builder: F,
) -> Result<i32, CliError>
where
    F: Fn(&A) -> Result<BucketConfigOperation, CliError>,
{
    if global.describe {
        if let Some(action) = action {
            match op_builder(action) {
                Ok(op) => {
                    output_result(global, &Envelope::success(op.command, describe_action(&op)))?;
                }
                Err(err) => {
                    let command = current_bucket_config_command_path(group_name);
                    if let Some(desc) = crate::registry::describe_command_metadata(&command) {
                        output_result(global, &Envelope::success(command, desc))?;
                    } else {
                        return Err(err);
                    }
                }
            }
        } else {
            output_result(global, &group_description)?;
        }
        return Ok(0);
    }

    let Some(action) = action else {
        return Err(CliError::ValidationError(format!(
            "`{group_name}` requires a subcommand; use `{group_name} --help` or `{group_name} --describe`"
        )));
    };

    let op = op_builder(action)?;
    if global.dry_run {
        output_result(global, &dry_run(&op))?;
        return Ok(0);
    }
    // [Review Fix #3] Dry-run must remain available for risk review before the caller adds --force.
    if op.destructive {
        ensure_force_for_destructive(global, op.force, op.command, &op.bucket)?;
    }

    let profile = build_profile(global)?;
    let client = TosClient::new(&profile, "tos")?;
    let result = core::execute_bucket_request(
        &client, op.command, op.method, &op.bucket, op.query, op.headers, op.body,
    )
    .await?;
    output_result(global, &result)?;
    Ok(0)
}

fn current_bucket_config_command_path(group_name: &str) -> String {
    let args = std::env::args().collect::<Vec<_>>();
    let binary_stem = args
        .first()
        .and_then(|arg| std::path::Path::new(arg).file_stem())
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let start = if matches!(binary_stem, "ve-tos" | "ve-tos-cli") {
        1
    } else {
        // [Review Fix #27] Recognize only the canonical unified `ve-tos`
        // entrypoint when reconstructing leaf describe metadata.
        let Some(tos_idx) = args.iter().position(|arg| arg.as_str() == "ve-tos") else {
            return group_name.to_string();
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
    for len in (1..=tokens.len()).rev() {
        let candidate = format!("ve-tos {}", tokens[..len].join(" "));
        if crate::registry::capability_row_for_command(&candidate, false).is_some()
            || crate::registry::find_command_tree_entry(&candidate).is_some()
        {
            return candidate;
        }
    }
    group_name.to_string()
}

fn quota_operation(action: &QuotaAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        QuotaAction::Get(args) => op(
            "ve-tos quota get",
            "GetBucketQuota",
            "Get bucket quota",
            Method::GET,
            args.bucket.require()?,
            query_flag("quota"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        QuotaAction::Set(args) => op(
            "ve-tos quota set",
            "PutBucketQuota",
            "Set bucket quota",
            Method::PUT,
            args.bucket.require()?,
            query_flag("quota"),
            Some(quota_body(args)?),
            RiskLevel::Medium,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "Quota(body)",
                    ParameterLocation::Body,
                    true,
                    "Bucket quota value in bytes",
                ),
                parameter(
                    "config(body)",
                    ParameterLocation::Body,
                    false,
                    "Full quota payload via --config",
                ),
            ],
            false,
            false,
        ),
    })
}

fn policy_operation(action: &PolicyAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        PolicyAction::Get(args) => op(
            "ve-tos policy get",
            "GetBucketPolicy",
            "Get bucket policy",
            Method::GET,
            args.bucket.require()?,
            query_flag("policy"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        PolicyAction::Set(args) => op(
            "ve-tos policy set",
            "PutBucketPolicy",
            "Set bucket policy",
            Method::PUT,
            args.bucket.require()?,
            query_flag("policy"),
            Some(required_config_body(&args.config, "ve-tos policy set")?),
            RiskLevel::Medium,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter("policy(body)", ParameterLocation::Body, true, "Policy JSON"),
            ],
            false,
            false,
        ),
        PolicyAction::Delete(args) => op(
            "ve-tos policy delete",
            "DeleteBucketPolicy",
            "Delete bucket policy (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("policy"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn lifecycle_operation(action: &LifecycleAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        LifecycleAction::Get(args) => op(
            "ve-tos lifecycle get",
            "GetBucketLifecycle",
            "Get lifecycle rules",
            Method::GET,
            args.bucket.require()?,
            query_flag("lifecycle"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        LifecycleAction::Set(args) => op(
            "ve-tos lifecycle set",
            "PutBucketLifecycle",
            "Set lifecycle rules",
            Method::PUT,
            args.bucket.require()?,
            query_flag("lifecycle"),
            Some(required_config_body(&args.config, "ve-tos lifecycle set")?),
            RiskLevel::Medium,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "Rules(body)",
                    ParameterLocation::Body,
                    false,
                    "Full lifecycle payload via --rules",
                ),
                parameter(
                    "ID(body)",
                    ParameterLocation::Body,
                    false,
                    "Lifecycle rule ID",
                ),
                parameter(
                    "Prefix(body)",
                    ParameterLocation::Body,
                    false,
                    "Lifecycle prefix selector",
                ),
                parameter(
                    "Status(body)",
                    ParameterLocation::Body,
                    false,
                    "Lifecycle rule status",
                ),
                parameter(
                    "Tags(body)",
                    ParameterLocation::Body,
                    false,
                    "Lifecycle tags",
                ),
                parameter(
                    "Filter(body)",
                    ParameterLocation::Body,
                    false,
                    "Lifecycle filter",
                ),
                parameter(
                    "Expiration(body)",
                    ParameterLocation::Body,
                    false,
                    "Expiration definition",
                ),
                parameter(
                    "NoncurrentVersionExpiration(body)",
                    ParameterLocation::Body,
                    false,
                    "Noncurrent version expiration",
                ),
                parameter(
                    "AbortIncompleteMultipartUpload(body)",
                    ParameterLocation::Body,
                    false,
                    "Abort incomplete multipart upload definition",
                ),
                parameter(
                    "Transitions(body)",
                    ParameterLocation::Body,
                    false,
                    "Storage class transitions",
                ),
                parameter(
                    "NoncurrentVersionTransitions(body)",
                    ParameterLocation::Body,
                    false,
                    "Noncurrent version transitions",
                ),
                parameter(
                    "AccessTimeTransitions(body)",
                    ParameterLocation::Body,
                    false,
                    "Access-time transitions",
                ),
                parameter(
                    "NoncurrentVersionAccessTimeTransitions(body)",
                    ParameterLocation::Body,
                    false,
                    "Noncurrent version access-time transitions",
                ),
            ],
            false,
            false,
        ),
        LifecycleAction::Delete(args) => op(
            "ve-tos lifecycle delete",
            "DeleteBucketLifecycle",
            "Delete lifecycle rules (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("lifecycle"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn storageclass_operation(action: &StorageclassAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        StorageclassAction::Set(args) => {
            // [Review Fix #1] PutBucketStoragePolicy 按文档走 storageClass query + x-tos-storage-class header，不能发送 JSON body。
            let headers = BTreeMap::from([(
                "x-tos-storage-class".to_string(),
                args.storage_class.clone(),
            )]);
            op_with_headers(
                "ve-tos storageclass set",
                "PutBucketStoragePolicy",
                "Set bucket default storage class",
                Method::PUT,
                args.bucket.require()?,
                query_flag("storageClass"),
                headers,
                None,
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "x-tos-storage-class",
                        ParameterLocation::Header,
                        true,
                        "Bucket default storage class header",
                    ),
                ],
                false,
                false,
            )
        }
    })
}

fn cors_operation(action: &CorsAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        CorsAction::Get(args) => op(
            "ve-tos cors get",
            "GetBucketCORS",
            "Get CORS configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("cors"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        CorsAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos cors set")?;
            let mut headers = BTreeMap::new();
            // [Review Fix #2] PutBucketCors 的 Content-MD5 是独立 header，需要在 CLI/help/describe 中显式暴露。
            insert_optional_header(&mut headers, "Content-MD5", args.content_md5.clone());
            op_with_headers(
                "ve-tos cors set",
                "PutBucketCORS",
                "Set CORS configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("cors"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "CORSRules(body)",
                        ParameterLocation::Body,
                        false,
                        "Full CORS payload via --rules",
                    ),
                    parameter(
                        "AllowedOrigins(body)",
                        ParameterLocation::Body,
                        false,
                        "Allowed origins",
                    ),
                    parameter(
                        "AllowedMethods(body)",
                        ParameterLocation::Body,
                        false,
                        "Allowed methods",
                    ),
                    parameter(
                        "AllowedHeaders(body)",
                        ParameterLocation::Body,
                        false,
                        "Allowed headers",
                    ),
                    parameter(
                        "ExposeHeaders(body)",
                        ParameterLocation::Body,
                        false,
                        "Expose headers",
                    ),
                    parameter(
                        "MaxAgeSeconds(body)",
                        ParameterLocation::Body,
                        false,
                        "CORS cache max age",
                    ),
                    parameter(
                        "ResponseVary(body)",
                        ParameterLocation::Body,
                        false,
                        "Whether to emit Vary: Origin",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Optional Content-MD5 header",
                    ),
                ],
                false,
                false,
            )
        }
        CorsAction::Delete(args) => op(
            "ve-tos cors delete",
            "DeleteBucketCors",
            "Delete CORS configuration (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("cors"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn versioning_operation(action: &VersioningAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        VersioningAction::Get(args) => op(
            "ve-tos versioning get",
            "GetBucketVersioning",
            "Get bucket versioning status",
            Method::GET,
            args.bucket.require()?,
            query_flag("versioning"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        VersioningAction::Set(args) => op(
            "ve-tos versioning set",
            "PutBucketVersioning",
            "Set bucket versioning status",
            Method::PUT,
            args.bucket.require()?,
            query_flag("versioning"),
            Some(required_config_body(&args.config, "ve-tos versioning set")?),
            RiskLevel::Medium,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "status(body)",
                    ParameterLocation::Body,
                    true,
                    "Versioning status",
                ),
            ],
            false,
            false,
        ),
    })
}

fn replication_operation(action: &ReplicationAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        ReplicationAction::Get(args) => {
            let mut query = query_flag("replication");
            if let Some(rule_id) = &args.rule_id {
                query.insert("rule-id".to_string(), rule_id.clone());
            }
            op(
                "ve-tos replication get",
                "GetBucketReplication",
                "Get replication configuration",
                Method::GET,
                args.bucket.require()?,
                query,
                None,
                RiskLevel::Low,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "rule-id",
                        ParameterLocation::Query,
                        false,
                        "Replication rule ID",
                    ),
                ],
                false,
                false,
            )
        }
        ReplicationAction::Set(args) => op(
            "ve-tos replication set",
            "PutBucketReplication",
            "Set replication configuration",
            Method::PUT,
            args.bucket.require()?,
            query_flag("replication"),
            Some(required_config_body(
                &args.config,
                "ve-tos replication set",
            )?),
            RiskLevel::Medium,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "Role(body)",
                    ParameterLocation::Body,
                    false,
                    "Replication role",
                ),
                parameter(
                    "Rules(body)",
                    ParameterLocation::Body,
                    false,
                    "Full replication payload via --rules",
                ),
                parameter(
                    "ID(body)",
                    ParameterLocation::Body,
                    false,
                    "Replication rule ID",
                ),
                parameter(
                    "Status(body)",
                    ParameterLocation::Body,
                    false,
                    "Replication status",
                ),
                parameter(
                    "PrefixSet(body)",
                    ParameterLocation::Body,
                    false,
                    "Replication prefix set",
                ),
                parameter(
                    "Tags(body)",
                    ParameterLocation::Body,
                    false,
                    "Replication tags",
                ),
                parameter(
                    "Destination.Bucket(body)",
                    ParameterLocation::Body,
                    false,
                    "Destination bucket ARN",
                ),
                parameter(
                    "Destination.Location(body)",
                    ParameterLocation::Body,
                    false,
                    "Destination location",
                ),
                parameter(
                    "Destination.StorageClass(body)",
                    ParameterLocation::Body,
                    false,
                    "Destination storage class",
                ),
                parameter(
                    "Destination.StorageClassInheritDirective(body)",
                    ParameterLocation::Body,
                    false,
                    "Storage class inherit directive",
                ),
                parameter(
                    "HistoricalObjectReplication(body)",
                    ParameterLocation::Body,
                    false,
                    "Historical object replication status",
                ),
                parameter(
                    "TransferType(body)",
                    ParameterLocation::Body,
                    false,
                    "Transfer type",
                ),
                parameter(
                    "AccessControlTranslation.Owner(body)",
                    ParameterLocation::Body,
                    false,
                    "Access control translation owner",
                ),
            ],
            false,
            false,
        ),
        ReplicationAction::Delete(args) => op(
            "ve-tos replication delete",
            "DeleteBucketReplication",
            "Delete replication configuration (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("replication"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn encryption_operation(action: &EncryptionAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        EncryptionAction::Get(args) => op(
            "ve-tos encryption get",
            "GetBucketEncryption",
            "Get bucket encryption configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("encryption"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        EncryptionAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos encryption set")?;
            let mut headers = BTreeMap::new();
            // [Review Fix #3] PutBucketEncryption 需要文档规定的嵌套 schema，并在省略时自动补齐 Content-MD5。
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos encryption set",
                "PutBucketEncryption",
                "Set bucket encryption configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("encryption"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "SSEAlgorithm(body)",
                        ParameterLocation::Body,
                        true,
                        "Server-side encryption algorithm",
                    ),
                    parameter(
                        "KMSDataEncryption(body)",
                        ParameterLocation::Body,
                        false,
                        "KMS data encryption mode",
                    ),
                    parameter(
                        "KMSMasterKeyID(body)",
                        ParameterLocation::Body,
                        false,
                        "KMS master key ID",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        EncryptionAction::Delete(args) => op(
            "ve-tos encryption delete",
            "DeleteBucketEncryption",
            "Delete bucket encryption configuration (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("encryption"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn tagging_operation(action: &TaggingAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        TaggingAction::Get(args) => op(
            "ve-tos tagging get",
            "GetBucketTagging",
            "Get bucket tagging",
            Method::GET,
            args.bucket.require()?,
            query_flag("tagging"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        TaggingAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos tagging set")?;
            let mut headers = BTreeMap::new();
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos tagging set",
                "PutBucketTagging",
                "Set bucket tagging",
                Method::PUT,
                args.bucket.require()?,
                query_flag("tagging"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "TagSet.Tags(body)",
                        ParameterLocation::Body,
                        true,
                        "Bucket tag list",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        TaggingAction::Delete(args) => op(
            "ve-tos tagging delete",
            "DeleteBucketTagging",
            "Delete bucket tagging (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("tagging"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn acl_operation(action: &AclAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        AclAction::Get(args) => op(
            "ve-tos acl get",
            "GetBucketAcl",
            "Get bucket ACL",
            Method::GET,
            args.bucket.require()?,
            query_flag("acl"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        AclAction::Set(args) => {
            let header_mode = [
                args.acl.is_some(),
                args.grant_full_control.is_some(),
                args.grant_read.is_some(),
                args.grant_read_non_list.is_some(),
                args.grant_read_acp.is_some(),
                args.grant_write.is_some(),
                args.grant_write_acp.is_some(),
            ]
            .into_iter()
            .any(|present| present);
            let body_mode = args.config.is_some();
            if !header_mode && !body_mode {
                return Err(CliError::ValidationError(
                    "ve-tos acl set requires either ACL headers or ACL body fields".into(),
                ));
            }
            if header_mode && body_mode {
                return Err(CliError::ValidationError(
                    "ve-tos acl set cannot mix ACL header mode with ACL body mode".into(),
                ));
            }

            if header_mode {
                if args.acl.is_some()
                    && [
                        args.grant_full_control.is_some(),
                        args.grant_read.is_some(),
                        args.grant_read_non_list.is_some(),
                        args.grant_read_acp.is_some(),
                        args.grant_write.is_some(),
                        args.grant_write_acp.is_some(),
                    ]
                    .into_iter()
                    .any(|present| present)
                {
                    return Err(CliError::ValidationError(
                        "ve-tos acl set cannot combine --acl with explicit x-tos-grant-* headers"
                            .into(),
                    ));
                }
                if args.grant_read_non_list.is_some() && args.grant_read_acp.is_none() {
                    return Err(CliError::ValidationError(
                        "ve-tos acl set requires --grant-read-acp when --grant-read-non-list is used"
                            .into(),
                    ));
                }
                let mut headers = BTreeMap::new();
                insert_optional_header(&mut headers, "x-tos-acl", args.acl.clone());
                insert_optional_header(
                    &mut headers,
                    "x-tos-grant-full-control",
                    args.grant_full_control.clone(),
                );
                insert_optional_header(&mut headers, "x-tos-grant-read", args.grant_read.clone());
                insert_optional_header(
                    &mut headers,
                    "x-tos-grant-read-non-list",
                    args.grant_read_non_list.clone(),
                );
                insert_optional_header(
                    &mut headers,
                    "x-tos-grant-read-acp",
                    args.grant_read_acp.clone(),
                );
                insert_optional_header(&mut headers, "x-tos-grant-write", args.grant_write.clone());
                insert_optional_header(
                    &mut headers,
                    "x-tos-grant-write-acp",
                    args.grant_write_acp.clone(),
                );
                op_with_headers(
                    "ve-tos acl set",
                    "PutBucketAcl",
                    "Set bucket ACL",
                    Method::PUT,
                    args.bucket.require()?,
                    query_flag("acl"),
                    headers,
                    None,
                    RiskLevel::High,
                    vec![
                        parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                        parameter(
                            "x-tos-acl",
                            ParameterLocation::Header,
                            false,
                            "Canned ACL header",
                        ),
                        parameter(
                            "x-tos-grant-full-control",
                            ParameterLocation::Header,
                            false,
                            "Grant full control header",
                        ),
                        parameter(
                            "x-tos-grant-read",
                            ParameterLocation::Header,
                            false,
                            "Grant read header",
                        ),
                        parameter(
                            "x-tos-grant-read-non-list",
                            ParameterLocation::Header,
                            false,
                            "Grant read without list header",
                        ),
                        parameter(
                            "x-tos-grant-read-acp",
                            ParameterLocation::Header,
                            false,
                            "Grant read ACP header",
                        ),
                        parameter(
                            "x-tos-grant-write",
                            ParameterLocation::Header,
                            false,
                            "Grant write header",
                        ),
                        parameter(
                            "x-tos-grant-write-acp",
                            ParameterLocation::Header,
                            false,
                            "Grant write ACP header",
                        ),
                        parameter(
                            "config(body)",
                            ParameterLocation::Body,
                            false,
                            "Full ACL JSON body",
                        ),
                    ],
                    false,
                    false,
                )
            } else {
                op(
                    "ve-tos acl set",
                    "PutBucketAcl",
                    "Set bucket ACL",
                    Method::PUT,
                    args.bucket.require()?,
                    query_flag("acl"),
                    Some(required_config_body(&args.config, "ve-tos acl set")?),
                    RiskLevel::High,
                    vec![
                        parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                        parameter(
                            "x-tos-acl",
                            ParameterLocation::Header,
                            false,
                            "Canned ACL header",
                        ),
                        parameter(
                            "x-tos-grant-full-control",
                            ParameterLocation::Header,
                            false,
                            "Grant full control header",
                        ),
                        parameter(
                            "x-tos-grant-read",
                            ParameterLocation::Header,
                            false,
                            "Grant read header",
                        ),
                        parameter(
                            "x-tos-grant-read-non-list",
                            ParameterLocation::Header,
                            false,
                            "Grant read without list header",
                        ),
                        parameter(
                            "x-tos-grant-read-acp",
                            ParameterLocation::Header,
                            false,
                            "Grant read ACP header",
                        ),
                        parameter(
                            "x-tos-grant-write",
                            ParameterLocation::Header,
                            false,
                            "Grant write header",
                        ),
                        parameter(
                            "x-tos-grant-write-acp",
                            ParameterLocation::Header,
                            false,
                            "Grant write ACP header",
                        ),
                        parameter(
                            "config(body)",
                            ParameterLocation::Body,
                            true,
                            "Full ACL JSON body",
                        ),
                    ],
                    false,
                    false,
                )
            }
        }
    })
}

fn rename_operation(action: &RenameAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        RenameAction::Get(args) => op(
            "ve-tos rename get",
            "GetBucketRename",
            "Get RenameObject configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("rename"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        RenameAction::Set(args) => op(
            "ve-tos rename set",
            "PutBucketRename",
            "Enable RenameObject for the bucket",
            Method::PUT,
            args.bucket.require()?,
            query_flag("rename"),
            Some(required_config_body(&args.config, "ve-tos rename set")?),
            RiskLevel::Medium,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "config(body)",
                    ParameterLocation::Body,
                    true,
                    "Full Rename JSON body",
                ),
            ],
            false,
            false,
        ),
        RenameAction::Delete(args) => op(
            "ve-tos rename delete",
            "DeleteBucketRename",
            "Disable RenameObject (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("rename"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn access_monitor_operation(
    action: &AccessMonitorAction,
) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        AccessMonitorAction::Get(args) => op(
            "ve-tos access-monitor get",
            "GetBucketAccessMonitor",
            "Get access monitor configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("accessmonitor"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        AccessMonitorAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos access-monitor set")?;
            let mut headers = BTreeMap::new();
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos access-monitor set",
                "PutBucketAccessMonitor",
                "Set access monitor configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("accessmonitor"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "Status(body)",
                        ParameterLocation::Body,
                        true,
                        "Access monitor status",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
    })
}

fn payment_operation(action: &PaymentAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        PaymentAction::Get(args) => op(
            "ve-tos payment get",
            "GetBucketRequestPayment",
            "Get requester pays configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("requestPayment"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        PaymentAction::Set(args) => op(
            "ve-tos payment set",
            "PutBucketRequestPayment",
            "Set requester pays configuration",
            Method::PUT,
            args.bucket.require()?,
            query_flag("requestPayment"),
            Some(required_config_body(&args.config, "ve-tos payment set")?),
            RiskLevel::Medium,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "Payer(body)",
                    ParameterLocation::Body,
                    true,
                    "Bucket payment mode",
                ),
            ],
            false,
            false,
        ),
    })
}

fn trash_operation(action: &TrashAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        TrashAction::Get(args) => op(
            "ve-tos trash get",
            "GetBucketTrash",
            "Get trash configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("trash"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        TrashAction::Set(args) => {
            let body = trash_body(args)?;
            let headers = md5_headers(&body, args.content_md5.clone());
            op_with_headers(
                "ve-tos trash set",
                "PutBucketTrash",
                "Set trash configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("trash"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "config(body)",
                        ParameterLocation::Body,
                        false,
                        "Full trash payload via --config",
                    ),
                    parameter(
                        "Status(body)",
                        ParameterLocation::Body,
                        false,
                        "Trash status",
                    ),
                    parameter(
                        "Days(body)",
                        ParameterLocation::Body,
                        false,
                        "Trash retention days",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
    })
}

fn logging_operation(action: &LoggingAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        LoggingAction::Get(args) => op(
            "ve-tos logging get",
            "GetBucketLogging",
            "Get bucket access logging configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("logging"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        LoggingAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos logging set")?;
            let mut headers = BTreeMap::new();
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos logging set",
                "PutBucketLogging",
                "Set bucket access logging configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("logging"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter(
                        "bucket",
                        ParameterLocation::Path,
                        true,
                        "Bucket name",
                    ),
                    parameter(
                        "BucketLoggingStatus.LoggingEnabled.TargetBucket(body)",
                        ParameterLocation::Body,
                        false,
                        "Target bucket for delivered logs; omit together with TargetPrefix to disable logging",
                    ),
                    parameter(
                        "BucketLoggingStatus.LoggingEnabled.TargetPrefix(body)",
                        ParameterLocation::Body,
                        false,
                        "Target object key prefix; omit together with TargetBucket to disable logging",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
    })
}

fn https_config_operation(action: &HttpsConfigAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        HttpsConfigAction::Get(args) => op(
            "ve-tos https-config get",
            "GetBucketHttpsConfig",
            "Get HTTPS/TLS configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("httpsConfig"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        HttpsConfigAction::Set(args) => op(
            "ve-tos https-config set",
            "PutBucketHttpsConfig",
            "Set HTTPS/TLS configuration",
            Method::PUT,
            args.bucket.require()?,
            query_flag("httpsConfig"),
            Some(required_config_body(
                &args.config,
                "ve-tos https-config set",
            )?),
            RiskLevel::Medium,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "config(body)",
                    ParameterLocation::Body,
                    true,
                    "Full HTTPS config JSON body",
                ),
            ],
            false,
            false,
        ),
    })
}

fn intelligent_tiering_operation(
    action: &IntelligentTieringAction,
) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        IntelligentTieringAction::Get(args) => op(
            "ve-tos intelligent-tiering get",
            "GetBucketIntelligentConfiguration",
            "Get intelligent tiering configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("intelligenttiering"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        IntelligentTieringAction::Set(args) => {
            let body = intelligent_tiering_body(args)?;
            let headers = md5_headers(&body, args.content_md5.clone());
            op_with_headers(
                "ve-tos intelligent-tiering set",
                "PutBucketIntelligentConfiguration",
                "Set intelligent tiering configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("intelligenttiering"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "config(body)",
                        ParameterLocation::Body,
                        false,
                        "Full intelligent tiering payload via --config",
                    ),
                    parameter(
                        "Status(body)",
                        ParameterLocation::Body,
                        false,
                        "Intelligent tiering status",
                    ),
                    parameter(
                        "Tiering.AccessTier(body)",
                        ParameterLocation::Body,
                        false,
                        "Tiering access tier",
                    ),
                    parameter(
                        "Tiering.Days(body)",
                        ParameterLocation::Body,
                        false,
                        "Tiering transition days",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
    })
}

fn transfer_acceleration_operation(
    action: &TransferAccelerationAction,
) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        TransferAccelerationAction::Get(args) => op(
            "ve-tos transfer-acceleration get",
            "GetBucketTransferAcceleration",
            "Get transfer acceleration configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("transferAcceleration"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        TransferAccelerationAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos transfer-acceleration set")?;
            let headers = md5_headers(&body, args.content_md5.clone());
            op_with_headers(
                "ve-tos transfer-acceleration set",
                "PutBucketTransferAcceleration",
                "Set transfer acceleration configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("transferAcceleration"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "Status(body)",
                        ParameterLocation::Body,
                        true,
                        "Transfer acceleration status",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
    })
}

fn cdn_notification_operation(
    action: &CdnNotificationAction,
) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        CdnNotificationAction::Get(args) => op(
            "ve-tos cdn-notification get",
            "GetBucketCdnNotification",
            "Get CDN notification configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("cdn_notification"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        CdnNotificationAction::Set(args) => {
            let body = cdn_notification_body(args)?;
            let headers = md5_headers(&body, args.content_md5.clone());
            op_with_headers(
                "ve-tos cdn-notification set",
                "PutBucketCdnNotification",
                "Set CDN notification configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("cdn_notification"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "config(body)",
                        ParameterLocation::Body,
                        false,
                        "Full CDN notification payload via --config",
                    ),
                    parameter(
                        "Rules.Events(body)",
                        ParameterLocation::Body,
                        false,
                        "Notification events",
                    ),
                    parameter(
                        "Rules.Filter.TOSKey.FilterRules(body)",
                        ParameterLocation::Body,
                        false,
                        "Filter rules",
                    ),
                    parameter(
                        "Role(body)",
                        ParameterLocation::Body,
                        false,
                        "CDN notification role",
                    ),
                    parameter(
                        "Rules.CustomDomain(body)",
                        ParameterLocation::Body,
                        false,
                        "CDN custom domain",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        CdnNotificationAction::Delete(args) => op(
            "ve-tos cdn-notification delete",
            "DeleteBucketCdnNotification",
            "Delete CDN notification configuration (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("cdn_notification"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn pay_by_traffic_operation(
    action: &PayByTrafficAction,
) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        PayByTrafficAction::Get(args) => op(
            "ve-tos pay-by-traffic get",
            "GetBucketPayByTraffic",
            "Get pay-by-traffic configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("payByTraffic"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        PayByTrafficAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos pay-by-traffic set")?;
            let headers = md5_headers(&body, args.content_md5.clone());
            op_with_headers(
                "ve-tos pay-by-traffic set",
                "PutBucketPayByTraffic",
                "Set pay-by-traffic configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("payByTraffic"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "Status(body)",
                        ParameterLocation::Body,
                        true,
                        "Pay-by-traffic status",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
    })
}

fn max_age_operation(action: &MaxAgeAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        MaxAgeAction::Get(args) => op(
            "ve-tos max-age get",
            "GetBucketMaxAge",
            "Get max-age configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("max-age"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        MaxAgeAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos max-age set")?;
            let headers = md5_headers(&body, args.content_md5.clone());
            op_with_headers(
                "ve-tos max-age set",
                "PutBucketMaxAge",
                "Set max-age configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("max-age"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "MaxAgeSeconds(body)",
                        ParameterLocation::Body,
                        true,
                        "Max-Age cache seconds",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        MaxAgeAction::Delete(args) => op(
            "ve-tos max-age delete",
            "DeleteBucketMaxAge",
            "Delete max-age configuration (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("max-age"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn redundancy_transition_operation(
    action: &RedundancyTransitionAction,
) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        RedundancyTransitionAction::Create(args) => {
            let (body, target_redundancy) = redundancy_transition_body(args)?;
            let headers = md5_headers(&body, args.content_md5.clone());
            op_with_headers(
                "ve-tos redundancy-transition create",
                "CreateBucketDataRedundancyTransition",
                "Create redundancy transition task",
                Method::POST,
                args.bucket.require()?,
                redundancy_transition_create_query(target_redundancy.as_deref()),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "config(body)",
                        ParameterLocation::Body,
                        false,
                        "Full redundancy transition payload via --config",
                    ),
                    parameter(
                        "x-tos-target-redundancy-type",
                        ParameterLocation::Query,
                        false,
                        "Target redundancy type",
                    ),
                    parameter(
                        "Prefix(body)",
                        ParameterLocation::Body,
                        false,
                        "Object prefix scope",
                    ),
                    parameter(
                        "StorageClass(body)",
                        ParameterLocation::Body,
                        false,
                        "Storage class filter",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        RedundancyTransitionAction::Delete(args) => op(
            "ve-tos redundancy-transition delete",
            "DeleteBucketDataRedundancyTransition",
            "Delete redundancy transition task (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            redundancy_transition_query(args.task_id.as_deref(), None),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "x-tos-redundancy-transition-taskid",
                    ParameterLocation::Query,
                    false,
                    "Redundancy transition task ID",
                ),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
        RedundancyTransitionAction::Get(args) => op(
            "ve-tos redundancy-transition get",
            "GetBucketDataRedundancyTransition",
            "Get redundancy transition task",
            Method::GET,
            args.bucket.require()?,
            redundancy_transition_query(Some(&args.task_id), None),
            None,
            RiskLevel::Low,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "x-tos-redundancy-transition-taskid",
                    ParameterLocation::Query,
                    true,
                    "Redundancy transition task ID",
                ),
            ],
            false,
            false,
        ),
        RedundancyTransitionAction::List(args) => op(
            "ve-tos redundancy-transition list",
            "ListBucketDataRedundancyTransition",
            "List redundancy transition tasks",
            Method::GET,
            args.bucket.require()?,
            redundancy_transition_query(None, args.continuation_token.as_deref()),
            None,
            RiskLevel::Low,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "continuation-token",
                    ParameterLocation::Query,
                    false,
                    "Continuation token",
                ),
            ],
            false,
            false,
        ),
        RedundancyTransitionAction::GetRemainingTime(args) => op(
            "ve-tos redundancy-transition get-remaining-time",
            "GetBucketTransitionEstimatedRemainingTime",
            "Get estimated remaining time",
            Method::GET,
            args.bucket.require()?,
            estimated_remaining_time_query(args.task_id.as_deref()),
            None,
            RiskLevel::Low,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "estimatedRemainingTime",
                    ParameterLocation::Query,
                    true,
                    "Estimated remaining time query flag",
                ),
                parameter(
                    "x-tos-redundancy-transition-taskid",
                    ParameterLocation::Query,
                    false,
                    "Redundancy transition task ID",
                ),
            ],
            false,
            false,
        ),
    })
}

fn quota_body(args: &QuotaSetArgs) -> Result<Vec<u8>, CliError> {
    if let Some(config) = &args.config {
        return json_bytes_from_input(config);
    }

    Err(CliError::ValidationError(
        "ve-tos quota set requires --config JSON body".into(),
    ))
}

fn required_config_body(config: &Option<String>, command: &str) -> Result<Vec<u8>, CliError> {
    json_bytes_from_input(config.as_deref().ok_or_else(|| {
        CliError::ValidationError(format!("{command} requires --config JSON body"))
    })?)
}

const TRASH_DEFAULT_CLEAN_INTERVAL: u64 = 7;
const TRASH_DEFAULT_FORBIDDEN_OVER_WRITE: &str = "Disabled";
const TRASH_DEFAULT_PATH: &str = ".Trash/";

fn trash_body(args: &TrashSetArgs) -> Result<Vec<u8>, CliError> {
    let structured = args.status.is_some() || args.days.is_some();
    ensure_payload_mode("trash", args.config.is_some(), structured)?;
    if let Some(config) = &args.config {
        // [Review Fix #HNS-TRASH-1] PutBucketTrash requires a wrapped `Trash`
        // payload with service-required defaults, while CLI users often pass
        // the same short status shape used by other bucket config commands.
        let body = normalize_trash_payload(read_json_input(config)?)?;
        return serde_json::to_vec(&body).map_err(CliError::Json);
    }

    let status = normalize_enum_value(
        "ve-tos trash set",
        "status",
        args.status.as_deref().unwrap_or_default(),
        &["Enabled", "Disabled"],
    )?;
    let body = build_trash_payload(status, args.days.map(u64::from), None, None, None);
    serde_json::to_vec(&body).map_err(CliError::Json)
}

fn normalize_trash_payload(value: Value) -> Result<Value, CliError> {
    let mut top_level = match value {
        Value::Object(map) => map,
        other => {
            return Err(CliError::ValidationError(format!(
                "ve-tos trash set requires a JSON object payload, got {other}"
            )));
        }
    };

    let inner = take_json_alias(&mut top_level, &["Trash", "trash"])
        .unwrap_or_else(|| Value::Object(top_level));
    let mut trash = match inner {
        Value::Object(map) => map,
        other => {
            return Err(CliError::ValidationError(format!(
                "`Trash` must be a JSON object, got {other}"
            )));
        }
    };

    let status = take_json_alias(&mut trash, &["Status", "status"])
        .ok_or_else(|| CliError::ValidationError("ve-tos trash set requires `Status`".into()))?;
    let status = trash_string_field(status, "Status")?;
    let status = normalize_enum_value(
        "ve-tos trash set",
        "Status",
        &status,
        &["Enabled", "Disabled"],
    )?;
    let clean_interval = take_json_alias(
        &mut trash,
        &["CleanInterval", "clean_interval", "Days", "days"],
    )
    .map(|value| trash_u64_field(value, "CleanInterval"))
    .transpose()?;
    let forbidden_over_write =
        take_json_alias(&mut trash, &["ForbiddenOverWrite", "forbidden_over_write"])
            .map(|value| trash_string_field(value, "ForbiddenOverWrite"))
            .transpose()?;
    let forbidden_over_write = forbidden_over_write
        .map(|value| {
            normalize_enum_value(
                "ve-tos trash set",
                "ForbiddenOverWrite",
                &value,
                &["Enabled", "Disabled"],
            )
        })
        .transpose()?;
    let trash_path = take_json_alias(&mut trash, &["TrashPath", "trash_path"])
        .map(|value| trash_string_field(value, "TrashPath"))
        .transpose()?;
    let prefix_match_rules =
        take_json_alias(&mut trash, &["PrefixMatchRules", "prefix_match_rules"]);

    let mut normalized = build_trash_inner(
        status,
        clean_interval,
        forbidden_over_write,
        trash_path,
        prefix_match_rules,
    );
    for (key, value) in trash {
        normalized.insert(key, value);
    }
    Ok(Value::Object(serde_json::Map::from_iter([(
        "Trash".to_string(),
        Value::Object(normalized),
    )])))
}

fn build_trash_payload(
    status: String,
    clean_interval: Option<u64>,
    forbidden_over_write: Option<String>,
    trash_path: Option<String>,
    prefix_match_rules: Option<Value>,
) -> Value {
    Value::Object(serde_json::Map::from_iter([(
        "Trash".to_string(),
        Value::Object(build_trash_inner(
            status,
            clean_interval,
            forbidden_over_write,
            trash_path,
            prefix_match_rules,
        )),
    )]))
}

fn build_trash_inner(
    status: String,
    clean_interval: Option<u64>,
    forbidden_over_write: Option<String>,
    trash_path: Option<String>,
    prefix_match_rules: Option<Value>,
) -> serde_json::Map<String, Value> {
    let mut body = serde_json::Map::new();
    body.insert("Status".to_string(), Value::String(status));
    body.insert(
        "CleanInterval".to_string(),
        json!(clean_interval.unwrap_or(TRASH_DEFAULT_CLEAN_INTERVAL)),
    );
    body.insert(
        "ForbiddenOverWrite".to_string(),
        Value::String(
            forbidden_over_write.unwrap_or_else(|| TRASH_DEFAULT_FORBIDDEN_OVER_WRITE.to_string()),
        ),
    );
    body.insert(
        "TrashPath".to_string(),
        Value::String(trash_path.unwrap_or_else(|| TRASH_DEFAULT_PATH.to_string())),
    );
    if let Some(value) = prefix_match_rules {
        if !value.is_null() {
            body.insert("PrefixMatchRules".to_string(), value);
        }
    }
    body
}

fn take_json_alias(map: &mut serde_json::Map<String, Value>, aliases: &[&str]) -> Option<Value> {
    aliases.iter().find_map(|alias| map.remove(*alias))
}

fn trash_string_field(value: Value, field: &str) -> Result<String, CliError> {
    match value {
        Value::String(value) => Ok(value),
        other => Err(CliError::ValidationError(format!(
            "`{field}` must be a string, got {other}"
        ))),
    }
}

fn trash_u64_field(value: Value, field: &str) -> Result<u64, CliError> {
    match value {
        Value::Number(number) => number.as_u64().ok_or_else(|| {
            CliError::ValidationError(format!("`{field}` must be a non-negative integer"))
        }),
        other => Err(CliError::ValidationError(format!(
            "`{field}` must be a non-negative integer, got {other}"
        ))),
    }
}

fn intelligent_tiering_body(args: &IntelligentTieringSetArgs) -> Result<Vec<u8>, CliError> {
    let structured = args.status.is_some() || args.access_tier.is_some() || args.days.is_some();
    ensure_payload_mode("intelligent-tiering", args.config.is_some(), structured)?;
    if let Some(config) = &args.config {
        return json_bytes_from_input(config);
    }

    let status = normalize_enum_value(
        "ve-tos intelligent-tiering set",
        "status",
        args.status.as_deref().unwrap_or_default(),
        &["Enabled", "Disabled"],
    )?;
    let mut body = serde_json::Map::new();
    body.insert("Status".to_string(), Value::String(status));
    // [Review Fix #9] TOS PutBucketIntelligentConf API 使用 "Transitions" (复数数组)，非 "Tiering" (单数对象)
    if args.access_tier.is_some() || args.days.is_some() {
        let mut transition = serde_json::Map::new();
        insert_optional_string(&mut transition, "AccessTier", args.access_tier.clone());
        if let Some(days) = args.days {
            transition.insert("Days".to_string(), json!(days));
        }
        body.insert(
            "Transitions".to_string(),
            Value::Array(vec![Value::Object(transition)]),
        );
    }
    serde_json::to_vec(&Value::Object(body)).map_err(CliError::Json)
}

// [Review Fix #10] TOS PutBucketTransferAcceleration API 使用
// {"TransferAccelerationConfiguration": {"Enabled": "true/false"}} 包装
#[cfg(test)]
fn transfer_acceleration_body(args: &TransferAccelerationSetArgs) -> Result<Vec<u8>, CliError> {
    if args.enabled.is_some() && args.status.is_some() {
        return Err(CliError::ValidationError(
            "ve-tos transfer-acceleration set cannot mix --enabled with --status".into(),
        ));
    }
    let enabled_str = if let Some(enabled) = args.enabled {
        if enabled { "true" } else { "false" }.to_string()
    } else {
        let status = normalize_enum_value(
            "ve-tos transfer-acceleration set",
            "status",
            args.status.as_deref().unwrap_or_default(),
            &["Enabled", "Suspended"],
        )?;
        if status == "Enabled" { "true" } else { "false" }.to_string()
    };
    serde_json::to_vec(&json!({
        "TransferAccelerationConfiguration": {
            "Enabled": enabled_str
        }
    }))
    .map_err(CliError::Json)
}

fn cdn_notification_body(args: &CdnNotificationSetArgs) -> Result<Vec<u8>, CliError> {
    let structured = args.events.is_some()
        || args.filter_rules.is_some()
        || args.role.is_some()
        || args.endpoint.is_some();
    ensure_payload_mode("cdn-notification", args.config.is_some(), structured)?;
    if let Some(config) = &args.config {
        return cdn_notification_config_body(config);
    }

    let events = parse_string_list(args.events.as_deref().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos cdn-notification set requires --events when using field-level flags".into(),
        )
    })?)?;
    let role = args.role.clone().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos cdn-notification set requires --role when using field-level flags".into(),
        )
    })?;
    let custom_domain = args.endpoint.clone().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos cdn-notification set requires --endpoint when using field-level flags".into(),
        )
    })?;
    let filter = json!({
        "TOSKey": {
            "FilterRules": args
                .filter_rules
                .as_deref()
                .map(parse_filter_rules)
                .transpose()?
                .unwrap_or_else(|| Value::Array(Vec::new()))
        }
    });
    let mut body = serde_json::Map::new();
    body.insert("Role".to_string(), Value::String(role));
    body.insert(
        "Rules".to_string(),
        json!([{
            "RuleId": "default",
            "CustomDomain": custom_domain,
            "Events": events,
            "Filter": filter,
        }]),
    );
    serde_json::to_vec(&Value::Object(body)).map_err(CliError::Json)
}

fn cdn_notification_config_body(input: &str) -> Result<Vec<u8>, CliError> {
    let value = read_json_input(input)?;
    serde_json::to_vec(&normalize_cdn_notification_value(value)?).map_err(CliError::Json)
}

fn normalize_cdn_notification_value(value: Value) -> Result<Value, CliError> {
    let Value::Object(mut map) = value else {
        return Err(CliError::ValidationError(
            "ve-tos cdn-notification set request body must be a JSON object".into(),
        ));
    };
    if map.contains_key("Rules") {
        validate_cdn_notification_canonical(&map)?;
        return Ok(Value::Object(map));
    }
    // [Review Fix #CdnNotificationSchema] The service schema is Role + Rules[].
    // Convert the older flat Events/Role/Endpoint/Filter payload so existing
    // config files fail less surprisingly while the outgoing body is canonical.
    let role = take_required_string(&mut map, "Role", "ve-tos cdn-notification set")?;
    let events = map.remove("Events").ok_or_else(|| {
        CliError::ValidationError("ve-tos cdn-notification set requires Events".into())
    })?;
    validate_string_array_value(&events, "Events", "ve-tos cdn-notification set")?;
    let custom_domain = take_optional_string(&mut map, "CustomDomain")
        .or_else(|| take_optional_string(&mut map, "Endpoint"))
        .ok_or_else(|| {
            CliError::ValidationError(
                "ve-tos cdn-notification set requires CustomDomain in each rule".into(),
            )
        })?;
    let rule_id = take_optional_string(&mut map, "RuleId").unwrap_or_else(|| "default".to_string());
    let filter = map.remove("Filter").unwrap_or_else(|| {
        json!({
            "TOSKey": {
                "FilterRules": []
            }
        })
    });
    let body = json!({
        "Role": role,
        "Rules": [{
            "RuleId": rule_id,
            "CustomDomain": custom_domain,
            "Events": events,
            "Filter": filter,
        }]
    });
    let Value::Object(body_map) = &body else {
        unreachable!("json! object")
    };
    validate_cdn_notification_canonical(body_map)?;
    Ok(body)
}

fn validate_cdn_notification_canonical(
    map: &serde_json::Map<String, Value>,
) -> Result<(), CliError> {
    validate_required_string(map, "Role", "ve-tos cdn-notification set")?;
    let rules = map.get("Rules").ok_or_else(|| {
        CliError::ValidationError("ve-tos cdn-notification set requires Rules".into())
    })?;
    let Value::Array(rules) = rules else {
        return Err(CliError::ValidationError(
            "ve-tos cdn-notification set requires Rules to be an array".into(),
        ));
    };
    if rules.is_empty() {
        return Err(CliError::ValidationError(
            "ve-tos cdn-notification set requires at least one rule".into(),
        ));
    }
    for rule in rules {
        let Value::Object(rule_map) = rule else {
            return Err(CliError::ValidationError(
                "ve-tos cdn-notification set requires each rule to be an object".into(),
            ));
        };
        validate_required_string(rule_map, "RuleId", "ve-tos cdn-notification set")?;
        validate_required_string(rule_map, "CustomDomain", "ve-tos cdn-notification set")?;
        let events = rule_map.get("Events").ok_or_else(|| {
            CliError::ValidationError("ve-tos cdn-notification set requires rule Events".into())
        })?;
        validate_string_array_value(events, "Rules.Events", "ve-tos cdn-notification set")?;
        let filter = rule_map.get("Filter").ok_or_else(|| {
            CliError::ValidationError("ve-tos cdn-notification set requires rule Filter".into())
        })?;
        validate_cdn_filter(filter)?;
    }
    Ok(())
}

fn validate_cdn_filter(value: &Value) -> Result<(), CliError> {
    let Some(filter_rules) = value
        .get("TOSKey")
        .and_then(|tos_key| tos_key.get("FilterRules"))
    else {
        return Err(CliError::ValidationError(
            "ve-tos cdn-notification set requires Filter.TOSKey.FilterRules".into(),
        ));
    };
    let Value::Array(rules) = filter_rules else {
        return Err(CliError::ValidationError(
            "ve-tos cdn-notification set requires FilterRules to be an array".into(),
        ));
    };
    for rule in rules {
        let Value::Object(rule_map) = rule else {
            return Err(CliError::ValidationError(
                "ve-tos cdn-notification set requires each FilterRule to be an object".into(),
            ));
        };
        validate_required_string(rule_map, "Name", "ve-tos cdn-notification set")?;
        validate_required_string(rule_map, "Value", "ve-tos cdn-notification set")?;
    }
    Ok(())
}

fn validate_required_string(
    map: &serde_json::Map<String, Value>,
    field: &str,
    command: &str,
) -> Result<(), CliError> {
    match map.get(field).and_then(Value::as_str) {
        Some(value) if !value.trim().is_empty() => Ok(()),
        _ => Err(CliError::ValidationError(format!(
            "{command} requires {field} to be a non-empty string"
        ))),
    }
}

fn take_required_string(
    map: &mut serde_json::Map<String, Value>,
    field: &str,
    command: &str,
) -> Result<String, CliError> {
    let value = map
        .remove(field)
        .ok_or_else(|| CliError::ValidationError(format!("{command} requires {field}")))?;
    value
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.to_string())
        .ok_or_else(|| {
            CliError::ValidationError(format!(
                "{command} requires {field} to be a non-empty string"
            ))
        })
}

fn take_optional_string(map: &mut serde_json::Map<String, Value>, field: &str) -> Option<String> {
    map.remove(field)
        .and_then(|value| value.as_str().map(|text| text.to_string()))
        .filter(|value| !value.trim().is_empty())
}

fn validate_string_array_value(value: &Value, field: &str, command: &str) -> Result<(), CliError> {
    let Value::Array(items) = value else {
        return Err(CliError::ValidationError(format!(
            "{command} requires {field} to be an array of strings"
        )));
    };
    if items.is_empty()
        || items.iter().any(|item| {
            item.as_str()
                .map(str::trim)
                .map(str::is_empty)
                .unwrap_or(true)
        })
    {
        return Err(CliError::ValidationError(format!(
            "{command} requires {field} to contain non-empty string values"
        )));
    }
    Ok(())
}

fn redundancy_transition_body(
    args: &RedundancyTransitionCreateArgs,
) -> Result<(Vec<u8>, Option<String>), CliError> {
    let structured =
        args.target_redundancy.is_some() || args.prefix.is_some() || args.storage_class.is_some();
    ensure_payload_mode("redundancy-transition", args.config.is_some(), structured)?;

    let mut target_redundancy = args.target_redundancy.clone();
    if let Some(config) = &args.config {
        let mut value = read_json_input(config)?;
        if let Value::Object(map) = &mut value {
            if let Some(raw_target_redundancy) = map.remove("TargetRedundancy") {
                let extracted = raw_target_redundancy.as_str().ok_or_else(|| {
                    CliError::ValidationError(
                        "ve-tos redundancy-transition create requires TargetRedundancy to be a string"
                            .into(),
                    )
                })?;
                target_redundancy = Some(extracted.to_string());
            }
        }
        let body = serde_json::to_vec(&value).map_err(CliError::Json)?;
        return Ok((body, target_redundancy));
    }

    let mut body = serde_json::Map::new();
    insert_optional_string(&mut body, "Prefix", args.prefix.clone());
    insert_optional_string(&mut body, "StorageClass", args.storage_class.clone());
    let body = serde_json::to_vec(&Value::Object(body)).map_err(CliError::Json)?;
    Ok((body, target_redundancy))
}

fn md5_headers(body: &[u8], content_md5: Option<String>) -> BTreeMap<String, String> {
    BTreeMap::from([(
        "Content-MD5".to_string(),
        content_md5.unwrap_or_else(|| content_md5_base64(body)),
    )])
}

fn redundancy_transition_create_query(target_redundancy: Option<&str>) -> BTreeMap<String, String> {
    // [Review Fix #2] CreateBucketDataRedundancyTransition expects target
    // redundancy as an operation query parameter, not inside the JSON body.
    build_query(&[
        ("redundancyTransition", Some(String::new())),
        (
            "x-tos-target-redundancy-type",
            target_redundancy.map(ToString::to_string),
        ),
    ])
}

fn redundancy_transition_query(
    task_id: Option<&str>,
    continuation_token: Option<&str>,
) -> BTreeMap<String, String> {
    build_query(&[
        ("redundancyTransition", Some(String::new())),
        (
            "x-tos-redundancy-transition-taskid",
            task_id.map(ToString::to_string),
        ),
        (
            "continuation-token",
            continuation_token.map(ToString::to_string),
        ),
    ])
}

fn estimated_remaining_time_query(task_id: Option<&str>) -> BTreeMap<String, String> {
    build_query(&[
        ("estimatedRemainingTime", Some(String::new())),
        (
            "x-tos-redundancy-transition-taskid",
            task_id.map(ToString::to_string),
        ),
    ])
}

#[cfg(test)]
fn custom_domain_body(args: &CustomDomainSetArgs) -> Result<Vec<u8>, CliError> {
    let mut rule = serde_json::Map::new();
    rule.insert("Domain".to_string(), Value::String(args.domain.clone()));
    insert_optional_string(&mut rule, "CertId", args.certificate_id.clone());
    insert_optional_string(&mut rule, "CertStatus", args.certificate_status.clone());
    insert_optional_string(&mut rule, "ForbiddenReason", args.forbidden_reason.clone());
    insert_optional_string(&mut rule, "Cname", args.cname.clone());
    insert_optional_string(&mut rule, "Protocol", args.protocol.clone());
    if let Some(forbidden) = args.forbidden {
        rule.insert("Forbidden".to_string(), Value::Bool(forbidden));
    }
    serde_json::to_vec(&json!({ "Rule": Value::Object(rule) })).map_err(CliError::Json)
}

#[cfg(test)]
fn notification_body(args: &NotificationSetArgs) -> Result<Vec<u8>, CliError> {
    let structured = [
        args.rule_id.is_some(),
        args.events.is_some(),
        args.filter_rules.is_some(),
        args.destination_vefaas.is_some(),
        args.vefaas_function_ids.is_some(),
        args.destination_kafka.is_some(),
        args.kafka_role.is_some(),
        args.kafka_instance_id.is_some(),
        args.kafka_topic.is_some(),
        args.kafka_user.is_some(),
        args.kafka_region.is_some(),
        args.destination_rocketmq.is_some(),
        args.rocketmq_role.is_some(),
        args.rocketmq_instance_id.is_some(),
        args.rocketmq_topic.is_some(),
        args.rocketmq_access_key_id.is_some(),
    ]
    .into_iter()
    .any(|present| present);

    ensure_payload_mode("notification", args.rules.is_some(), structured)?;

    let version = args.version.clone().ok_or_else(|| {
        CliError::ValidationError("ve-tos notification set requires --version".into())
    })?;

    let rules = if let Some(input) = &args.rules {
        parse_json_array(input, "Rules")?
    } else {
        let events = parse_string_list(args.events.as_deref().ok_or_else(|| {
            CliError::ValidationError(
                "ve-tos notification set requires --events when using field-level notification_v2 flags".into(),
            )
        })?)?;

        let vefaas = if let Some(input) = &args.destination_vefaas {
            parse_json_array(input, "Destination.VeFaaS")?
        } else if let Some(function_ids) = args.vefaas_function_ids.as_deref() {
            let ids = parse_string_list(function_ids)?;
            if let Value::Array(items) = ids {
                Value::Array(
                    items
                        .into_iter()
                        .map(|item| json!({ "FunctionId": item.as_str().unwrap_or_default() }))
                        .collect(),
                )
            } else {
                Value::Array(Vec::new())
            }
        } else {
            Value::Array(Vec::new())
        };

        let kafka = if let Some(input) = &args.destination_kafka {
            parse_json_array(input, "Destination.Kafka")?
        } else if args.kafka_role.is_some()
            || args.kafka_instance_id.is_some()
            || args.kafka_topic.is_some()
            || args.kafka_user.is_some()
            || args.kafka_region.is_some()
        {
            Value::Array(vec![json!({
                "Role": args.kafka_role.clone().ok_or_else(|| CliError::ValidationError("ve-tos notification set requires --kafka-role when Kafka destination fields are used".into()))?,
                "InstanceId": args.kafka_instance_id.clone().ok_or_else(|| CliError::ValidationError("ve-tos notification set requires --kafka-instance-id when Kafka destination fields are used".into()))?,
                "Topic": args.kafka_topic.clone().ok_or_else(|| CliError::ValidationError("ve-tos notification set requires --kafka-topic when Kafka destination fields are used".into()))?,
                "User": args.kafka_user.clone().ok_or_else(|| CliError::ValidationError("ve-tos notification set requires --kafka-user when Kafka destination fields are used".into()))?,
                "Region": args.kafka_region.clone().unwrap_or_default(),
            })])
        } else {
            Value::Array(Vec::new())
        };

        let rocketmq = if let Some(input) = &args.destination_rocketmq {
            parse_json_array(input, "Destination.RocketMQ")?
        } else if args.rocketmq_role.is_some()
            || args.rocketmq_instance_id.is_some()
            || args.rocketmq_topic.is_some()
            || args.rocketmq_access_key_id.is_some()
        {
            Value::Array(vec![json!({
                "Role": args.rocketmq_role.clone().ok_or_else(|| CliError::ValidationError("ve-tos notification set requires --rocketmq-role when RocketMQ destination fields are used".into()))?,
                "InstanceId": args.rocketmq_instance_id.clone().ok_or_else(|| CliError::ValidationError("ve-tos notification set requires --rocketmq-instance-id when RocketMQ destination fields are used".into()))?,
                "Topic": args.rocketmq_topic.clone().ok_or_else(|| CliError::ValidationError("ve-tos notification set requires --rocketmq-topic when RocketMQ destination fields are used".into()))?,
                "AccessKeyId": args.rocketmq_access_key_id.clone().ok_or_else(|| CliError::ValidationError("ve-tos notification set requires --rocketmq-access-key-id when RocketMQ destination fields are used".into()))?,
            })])
        } else {
            Value::Array(Vec::new())
        };

        if vefaas.as_array().map_or(true, |items| items.is_empty())
            && kafka.as_array().map_or(true, |items| items.is_empty())
            && rocketmq.as_array().map_or(true, |items| items.is_empty())
        {
            return Err(CliError::ValidationError(
                "ve-tos notification set requires at least one destination in notification_v2 mode"
                    .into(),
            ));
        }

        Value::Array(vec![json!({
            "RuleId": args.rule_id.clone(),
            "Events": events,
            "Filter": build_notification_filter(args.filter_rules.as_deref())?,
            "Destination": {
                "VeFaaS": vefaas,
                "Kafka": kafka,
                "RocketMQ": rocketmq,
            }
        })])
    };

    serde_json::to_vec(&json!({
        "Rules": rules,
        "Version": version,
    }))
    .map_err(CliError::Json)
}

#[cfg(test)]
fn website_body(args: &WebsiteSetArgs) -> Result<Vec<u8>, CliError> {
    let has_redirect_all = args.redirect_all_requests_to_host_name.is_some()
        || args.redirect_all_requests_to_protocol.is_some();
    let has_index_document =
        args.index_document_suffix.is_some() || args.index_document_forbidden_sub_dir.is_some();
    let has_routing_rule_fields = [
        args.routing_rule_key_prefix_equals.is_some(),
        args.routing_rule_http_error_code_returned_equals.is_some(),
        args.routing_rule_protocol.is_some(),
        args.routing_rule_host_name.is_some(),
        args.routing_rule_replace_key_prefix_with.is_some(),
        args.routing_rule_replace_key_with.is_some(),
        args.routing_rule_http_redirect_code.is_some(),
    ]
    .into_iter()
    .any(|present| present);

    if args.redirect_all_requests_to_protocol.is_some()
        && args.redirect_all_requests_to_host_name.is_none()
    {
        return Err(CliError::ValidationError(
            "ve-tos website set requires --redirect-all-requests-to-host-name when --redirect-all-requests-to-protocol is used".into(),
        ));
    }
    if args.index_document_forbidden_sub_dir.is_some() && args.index_document_suffix.is_none() {
        return Err(CliError::ValidationError(
            "ve-tos website set requires --index-document-suffix when --index-document-forbidden-sub-dir is used".into(),
        ));
    }
    if args.routing_rules.is_some() && has_routing_rule_fields {
        return Err(CliError::ValidationError(
            "ve-tos website set cannot mix --routing-rules with single routing rule flags".into(),
        ));
    }
    if has_redirect_all
        && (has_index_document
            || args.error_document_key.is_some()
            || args.routing_rules.is_some()
            || has_routing_rule_fields)
    {
        return Err(CliError::ValidationError(
            "ve-tos website set cannot combine RedirectAllRequestsTo with index/error/routing website configuration".into(),
        ));
    }
    if args.routing_rule_replace_key_prefix_with.is_some()
        && args.routing_rule_replace_key_with.is_some()
    {
        return Err(CliError::ValidationError(
            "ve-tos website set cannot use --routing-rule-replace-key-prefix-with together with --routing-rule-replace-key-with".into(),
        ));
    }

    let mut body = serde_json::Map::new();

    if let Some(host_name) = &args.redirect_all_requests_to_host_name {
        let mut redirect_all = serde_json::Map::new();
        redirect_all.insert("HostName".to_string(), Value::String(host_name.clone()));
        insert_optional_string(
            &mut redirect_all,
            "Protocol",
            args.redirect_all_requests_to_protocol.clone(),
        );
        body.insert(
            "RedirectAllRequestsTo".to_string(),
            Value::Object(redirect_all),
        );
    }

    if let Some(suffix) = &args.index_document_suffix {
        let mut index_document = serde_json::Map::new();
        index_document.insert("Suffix".to_string(), Value::String(suffix.clone()));
        if let Some(forbidden_sub_dir) = args.index_document_forbidden_sub_dir {
            index_document.insert(
                "ForbiddenSubDir".to_string(),
                Value::Bool(forbidden_sub_dir),
            );
        }
        body.insert("IndexDocument".to_string(), Value::Object(index_document));
    }

    if let Some(error_document_key) = &args.error_document_key {
        body.insert(
            "ErrorDocument".to_string(),
            json!({ "Key": error_document_key }),
        );
    }

    if let Some(routing_rules) = &args.routing_rules {
        body.insert(
            "RoutingRules".to_string(),
            parse_json_array(routing_rules, "RoutingRules")?,
        );
    } else if has_routing_rule_fields {
        let has_redirect = [
            args.routing_rule_protocol.is_some(),
            args.routing_rule_host_name.is_some(),
            args.routing_rule_replace_key_prefix_with.is_some(),
            args.routing_rule_replace_key_with.is_some(),
            args.routing_rule_http_redirect_code.is_some(),
        ]
        .into_iter()
        .any(|present| present);
        if !has_redirect {
            return Err(CliError::ValidationError(
                "ve-tos website set requires at least one redirect field when using single routing rule flags".into(),
            ));
        }

        let mut condition = serde_json::Map::new();
        let mut redirect = serde_json::Map::new();
        insert_optional_string(
            &mut condition,
            "KeyPrefixEquals",
            args.routing_rule_key_prefix_equals.clone(),
        );
        if let Some(http_error_code) = args.routing_rule_http_error_code_returned_equals {
            condition.insert(
                "HttpErrorCodeReturnedEquals".to_string(),
                json!(http_error_code),
            );
        }
        insert_optional_string(
            &mut redirect,
            "Protocol",
            args.routing_rule_protocol.clone(),
        );
        insert_optional_string(
            &mut redirect,
            "HostName",
            args.routing_rule_host_name.clone(),
        );
        insert_optional_string(
            &mut redirect,
            "ReplaceKeyPrefixWith",
            args.routing_rule_replace_key_prefix_with.clone(),
        );
        insert_optional_string(
            &mut redirect,
            "ReplaceKeyWith",
            args.routing_rule_replace_key_with.clone(),
        );
        if let Some(http_redirect_code) = args.routing_rule_http_redirect_code {
            redirect.insert("HttpRedirectCode".to_string(), json!(http_redirect_code));
        }
        body.insert(
            "RoutingRules".to_string(),
            Value::Array(vec![json!({
                "Condition": Value::Object(condition),
                "Redirect": Value::Object(redirect),
            })]),
        );
    }

    if body.is_empty() {
        return Err(CliError::ValidationError(
            "ve-tos website set requires at least one website configuration field".into(),
        ));
    }

    serde_json::to_vec(&Value::Object(body)).map_err(CliError::Json)
}

#[cfg(test)]
fn mirror_body(args: &MirrorSetArgs) -> Result<Vec<u8>, CliError> {
    let structured = [
        args.id.is_some(),
        args.condition_http_code.is_some(),
        args.condition_key_prefix.is_some(),
        args.condition_key_suffix.is_some(),
        args.condition_allow_hosts.is_some(),
        args.condition_http_methods.is_some(),
        args.redirect_type.is_some(),
        args.fetch_source_on_redirect.is_some(),
        args.pass_query.is_some(),
        args.follow_redirect.is_some(),
        args.mirror_header_pass_all.is_some(),
        args.mirror_header_pass.is_some(),
        args.mirror_header_remove.is_some(),
        args.mirror_header_set.is_some(),
        args.public_source_primary_endpoints.is_some(),
        args.public_source_follower_endpoints.is_some(),
        args.public_source_fixed_endpoint.is_some(),
        args.transform_with_key_prefix.is_some(),
        args.transform_with_key_suffix.is_some(),
        args.transform_replace_key_prefix.is_some(),
        args.transform_replace_key_prefix_with.is_some(),
        args.fetch_header_to_metadata_rules.is_some(),
        args.private_source_primary_endpoints.is_some(),
        args.private_source_follower_endpoints.is_some(),
        args.private_source_bucket_name.is_some(),
        args.private_source_role.is_some(),
        args.private_source_region.is_some(),
        args.private_source_storage_vendor.is_some(),
        args.private_source_ak.is_some(),
        args.private_source_sk.is_some(),
        args.private_source_sk_encrypt_type.is_some(),
        args.fetch_source_on_redirect_with_query.is_some(),
        args.pass_status_code_from_source.is_some(),
        args.pass_header_from_source.is_some(),
    ]
    .into_iter()
    .any(|present| present);
    ensure_payload_mode("mirror", args.rules.is_some(), structured)?;

    if let Some(rules) = &args.rules {
        let value = read_json_input(rules)?;
        return match value {
            Value::Array(items) => {
                serde_json::to_vec(&json!({ "Rules": items })).map_err(CliError::Json)
            }
            Value::Object(map) if map.contains_key("Rules") => {
                serde_json::to_vec(&Value::Object(map)).map_err(CliError::Json)
            }
            other => Err(CliError::ValidationError(format!(
                "ve-tos mirror set expects a JSON array of rules or an object containing `Rules`, got {other}"
            ))),
        };
    }

    if args.transform_replace_key_prefix.is_some()
        ^ args.transform_replace_key_prefix_with.is_some()
    {
        return Err(CliError::ValidationError(
            "ve-tos mirror set requires both --transform-replace-key-prefix and --transform-replace-key-prefix-with when using ReplaceKeyPrefix".into(),
        ));
    }

    let mut condition = serde_json::Map::new();
    if let Some(http_code) = args.condition_http_code {
        condition.insert("HttpCode".to_string(), json!(http_code));
    }
    insert_optional_string(
        &mut condition,
        "KeyPrefix",
        args.condition_key_prefix.clone(),
    );
    insert_optional_string(
        &mut condition,
        "KeySuffix",
        args.condition_key_suffix.clone(),
    );
    insert_optional_parsed(
        &mut condition,
        "AllowHost",
        args.condition_allow_hosts.as_deref(),
        parse_string_list,
    )?;
    insert_optional_parsed(
        &mut condition,
        "HttpMethod",
        args.condition_http_methods.as_deref(),
        parse_string_list,
    )?;

    let mut redirect = serde_json::Map::new();
    insert_optional_string(&mut redirect, "RedirectType", args.redirect_type.clone());
    insert_optional_bool(
        &mut redirect,
        "FetchSourceOnRedirect",
        args.fetch_source_on_redirect,
    );
    insert_optional_bool(&mut redirect, "PassQuery", args.pass_query);
    insert_optional_bool(&mut redirect, "FollowRedirect", args.follow_redirect);
    insert_optional_bool(
        &mut redirect,
        "FetchSourceOnRedirectWithQuery",
        args.fetch_source_on_redirect_with_query,
    );
    insert_optional_parsed(
        &mut redirect,
        "PassStatusCodeFromSource",
        args.pass_status_code_from_source.as_deref(),
        parse_int_list,
    )?;
    insert_optional_parsed(
        &mut redirect,
        "PassHeaderFromSource",
        args.pass_header_from_source.as_deref(),
        parse_string_list,
    )?;

    let mut mirror_header = serde_json::Map::new();
    insert_optional_bool(&mut mirror_header, "PassAll", args.mirror_header_pass_all);
    insert_optional_parsed(
        &mut mirror_header,
        "Pass",
        args.mirror_header_pass.as_deref(),
        parse_string_list,
    )?;
    insert_optional_parsed(
        &mut mirror_header,
        "Remove",
        args.mirror_header_remove.as_deref(),
        parse_string_list,
    )?;
    insert_optional_parsed(
        &mut mirror_header,
        "Set",
        args.mirror_header_set.as_deref(),
        parse_key_value_objects,
    )?;
    if !mirror_header.is_empty() {
        redirect.insert("MirrorHeader".to_string(), Value::Object(mirror_header));
    }

    let public_source = build_public_source(args)?;
    if let Some(public_source) = public_source {
        redirect.insert("PublicSource".to_string(), public_source);
    }

    let private_source = build_private_source(args)?;
    if let Some(private_source) = private_source {
        redirect.insert("PrivateSource".to_string(), private_source);
    }

    let mut transform = serde_json::Map::new();
    insert_optional_string(
        &mut transform,
        "WithKeyPrefix",
        args.transform_with_key_prefix.clone(),
    );
    insert_optional_string(
        &mut transform,
        "WithKeySuffix",
        args.transform_with_key_suffix.clone(),
    );
    if let (Some(key_prefix), Some(replace_with)) = (
        args.transform_replace_key_prefix.clone(),
        args.transform_replace_key_prefix_with.clone(),
    ) {
        transform.insert(
            "ReplaceKeyPrefix".to_string(),
            json!({
                "KeyPrefix": key_prefix,
                "ReplaceWith": replace_with,
            }),
        );
    }
    if !transform.is_empty() {
        redirect.insert("Transform".to_string(), Value::Object(transform));
    }

    insert_optional_parsed(
        &mut redirect,
        "FetchHeaderToMetaDataRules",
        args.fetch_header_to_metadata_rules.as_deref(),
        parse_fetch_header_metadata_rules,
    )?;

    let mut rule = serde_json::Map::new();
    insert_optional_string(&mut rule, "ID", args.id.clone());
    if !condition.is_empty() {
        rule.insert("Condition".to_string(), Value::Object(condition));
    }
    if !redirect.is_empty() {
        rule.insert("Redirect".to_string(), Value::Object(redirect));
    }

    if rule.is_empty() {
        return Err(CliError::ValidationError(
            "ve-tos mirror set requires either a full JSON payload or field-level mirror schema flags"
                .into(),
        ));
    }

    serde_json::to_vec(&json!({ "Rules": [Value::Object(rule)] })).map_err(CliError::Json)
}

#[cfg(test)]
fn inventory_body(args: &InventorySetArgs) -> Result<Vec<u8>, CliError> {
    let is_enabled = args.is_enabled.ok_or_else(|| {
        CliError::ValidationError("ve-tos inventory set requires --is-enabled".into())
    })?;
    let destination_format = args.destination_format.clone().ok_or_else(|| {
        CliError::ValidationError("ve-tos inventory set requires --destination-format".into())
    })?;
    let destination_account_id = args.destination_account_id.clone().ok_or_else(|| {
        CliError::ValidationError("ve-tos inventory set requires --destination-account-id".into())
    })?;
    let destination_role = args.destination_role.clone().ok_or_else(|| {
        CliError::ValidationError("ve-tos inventory set requires --destination-role".into())
    })?;
    let destination_bucket = args.destination_bucket.clone().ok_or_else(|| {
        CliError::ValidationError("ve-tos inventory set requires --destination-bucket".into())
    })?;
    let schedule_frequency = args.schedule_frequency.clone().ok_or_else(|| {
        CliError::ValidationError("ve-tos inventory set requires --schedule-frequency".into())
    })?;
    let included_object_versions = args.included_object_versions.clone().ok_or_else(|| {
        CliError::ValidationError("ve-tos inventory set requires --included-object-versions".into())
    })?;
    let is_uncompressed = args.is_uncompressed.ok_or_else(|| {
        CliError::ValidationError("ve-tos inventory set requires --is-uncompressed".into())
    })?;

    let mut destination = serde_json::Map::new();
    destination.insert("Format".to_string(), Value::String(destination_format));
    destination.insert(
        "AccountID".to_string(),
        Value::String(destination_account_id),
    );
    destination.insert("Role".to_string(), Value::String(destination_role));
    destination.insert("Bucket".to_string(), Value::String(destination_bucket));
    insert_optional_string(&mut destination, "Prefix", args.destination_prefix.clone());

    let mut body = serde_json::Map::new();
    body.insert("Id".to_string(), Value::String(args.id.clone()));
    body.insert("IsEnabled".to_string(), Value::Bool(is_enabled));
    if let Some(filter_prefix) = &args.filter_prefix {
        body.insert("Filter".to_string(), json!({ "Prefix": filter_prefix }));
    }
    body.insert(
        "Destination".to_string(),
        json!({ "TOSBucketDestination": Value::Object(destination) }),
    );
    body.insert(
        "Schedule".to_string(),
        json!({ "Frequency": schedule_frequency }),
    );
    body.insert(
        "IncludedObjectVersions".to_string(),
        Value::String(included_object_versions),
    );
    if let Some(optional_fields) = &args.optional_fields {
        body.insert(
            "OptionalFields".to_string(),
            json!({ "Field": parse_string_list(optional_fields)? }),
        );
    }
    body.insert("IsUnCompressed".to_string(), Value::Bool(is_uncompressed));
    serde_json::to_vec(&Value::Object(body)).map_err(CliError::Json)
}

fn real_time_log_body(args: &RealTimeLogSetArgs) -> Result<Vec<u8>, CliError> {
    // [Review Fix #10] Respect --config passthrough: if the user provides a full
    // JSON body via --config, use it directly instead of requiring structured args.
    if let Some(config) = &args.config {
        return json_bytes_from_input(config);
    }
    let use_service_topic = args.use_service_topic.ok_or_else(|| {
        CliError::ValidationError("ve-tos real-time-log set requires --use-service-topic".into())
    })?;
    if use_service_topic && (args.tls_project_id.is_some() || args.tls_topic_id.is_some()) {
        return Err(CliError::ValidationError(
            "ve-tos real-time-log set cannot provide TLS project/topic IDs when --use-service-topic true is used".into(),
        ));
    }
    if !use_service_topic && (args.tls_project_id.is_none() || args.tls_topic_id.is_none()) {
        return Err(CliError::ValidationError(
            "ve-tos real-time-log set requires --tls-project-id and --tls-topic-id when --use-service-topic false is used".into(),
        ));
    }

    let mut access_log_configuration = serde_json::Map::new();
    access_log_configuration.insert(
        "UseServiceTopic".to_string(),
        Value::Bool(use_service_topic),
    );
    insert_optional_string(
        &mut access_log_configuration,
        "TLSProjectID",
        args.tls_project_id.clone(),
    );
    insert_optional_string(
        &mut access_log_configuration,
        "TLSTopicID",
        args.tls_topic_id.clone(),
    );

    // [Review Fix #11] API expects body wrapped in "RealTimeLogConfiguration"
    serde_json::to_vec(&json!({
        "RealTimeLogConfiguration": {
            "Role": args.role,
            "AccessLogConfiguration": Value::Object(access_log_configuration),
        }
    }))
    .map_err(CliError::Json)
}

#[cfg(test)]
fn logging_body(args: &LoggingSetArgs) -> Result<Vec<u8>, CliError> {
    match (&args.target_bucket, &args.target_prefix) {
        (Some(target_bucket), Some(target_prefix)) => serde_json::to_vec(&json!({
            "BucketLoggingStatus": {
                "LoggingEnabled": {
                    "TargetBucket": target_bucket,
                    "TargetPrefix": target_prefix,
                }
            }
        }))
        .map_err(CliError::Json),
        (None, None) => serde_json::to_vec(&json!({
            "BucketLoggingStatus": {}
        }))
        .map_err(CliError::Json),
        _ => Err(CliError::ValidationError(
            "ve-tos logging set requires --target-bucket and --target-prefix together, or omits both to disable logging"
                .into(),
        )),
    }
}

#[cfg(test)]
fn worm_body(args: &WormSetArgs) -> Result<Vec<u8>, CliError> {
    let object_lock_enabled = args
        .object_lock_enabled
        .clone()
        .unwrap_or_else(|| "Enabled".to_string());
    let object_lock_enabled = normalize_enum_value(
        "ve-tos worm set",
        "object-lock-enabled",
        &object_lock_enabled,
        &["Enabled", "Disabled"],
    )?;

    if args.default_retention_days.is_some() && args.default_retention_years.is_some() {
        return Err(CliError::ValidationError(
            "ve-tos worm set cannot use --default-retention-days together with --default-retention-years".into(),
        ));
    }
    if [
        args.default_retention_mode.is_some(),
        args.default_retention_days.is_some(),
        args.default_retention_years.is_some(),
    ]
    .into_iter()
    .any(|present| present)
    {
        let mode = normalize_enum_value(
            "ve-tos worm set",
            "default-retention-mode",
            args.default_retention_mode.as_deref().ok_or_else(|| {
                CliError::ValidationError(
                    "ve-tos worm set requires --default-retention-mode when default retention is configured".into(),
                )
            })?,
            &["COMPLIANCE", "GOVERNANCE"],
        )?;
        let mut default_retention = serde_json::Map::new();
        default_retention.insert("Mode".to_string(), Value::String(mode));
        if let Some(days) = args.default_retention_days {
            default_retention.insert("Days".to_string(), json!(days));
        }
        if let Some(years) = args.default_retention_years {
            default_retention.insert("Years".to_string(), json!(years));
        }
        if !default_retention.contains_key("Days") && !default_retention.contains_key("Years") {
            return Err(CliError::ValidationError(
                "ve-tos worm set requires either --default-retention-days or --default-retention-years when default retention is configured".into(),
            ));
        }
        return serde_json::to_vec(&json!({
            "ObjectLockEnabled": object_lock_enabled,
            "Rule": {
                "DefaultRetention": Value::Object(default_retention),
            }
        }))
        .map_err(CliError::Json);
    }

    serde_json::to_vec(&json!({
        "ObjectLockEnabled": object_lock_enabled,
    }))
    .map_err(CliError::Json)
}

#[cfg(test)]
fn build_notification_filter(filter_rules: Option<&str>) -> Result<Value, CliError> {
    Ok(json!({
        "TOSKey": {
            "FilterRules": filter_rules
                .map(parse_filter_rules)
                .transpose()?
                .unwrap_or_else(|| Value::Array(Vec::new()))
        }
    }))
}

#[cfg(test)]
fn build_public_source(args: &MirrorSetArgs) -> Result<Option<Value>, CliError> {
    let mut public_source = serde_json::Map::new();
    let mut source_endpoint = serde_json::Map::new();
    insert_optional_parsed(
        &mut source_endpoint,
        "Primary",
        args.public_source_primary_endpoints.as_deref(),
        parse_string_list,
    )?;
    insert_optional_parsed(
        &mut source_endpoint,
        "Follower",
        args.public_source_follower_endpoints.as_deref(),
        parse_string_list,
    )?;
    if !source_endpoint.is_empty() {
        public_source.insert("SourceEndpoint".to_string(), Value::Object(source_endpoint));
    }
    insert_optional_bool(
        &mut public_source,
        "FixedEndpoint",
        args.public_source_fixed_endpoint,
    );
    if public_source.is_empty() {
        Ok(None)
    } else {
        Ok(Some(Value::Object(public_source)))
    }
}

#[cfg(test)]
fn build_private_source(args: &MirrorSetArgs) -> Result<Option<Value>, CliError> {
    let has_private = [
        args.private_source_primary_endpoints.is_some(),
        args.private_source_follower_endpoints.is_some(),
        args.private_source_bucket_name.is_some(),
        args.private_source_role.is_some(),
        args.private_source_region.is_some(),
        args.private_source_storage_vendor.is_some(),
        args.private_source_ak.is_some(),
        args.private_source_sk.is_some(),
        args.private_source_sk_encrypt_type.is_some(),
    ]
    .into_iter()
    .any(|present| present);
    if !has_private {
        return Ok(None);
    }

    let static_credential = if args.private_source_storage_vendor.is_some()
        || args.private_source_ak.is_some()
        || args.private_source_sk.is_some()
        || args.private_source_sk_encrypt_type.is_some()
    {
        Some(json!({
            "StorageVendor": args.private_source_storage_vendor.clone().ok_or_else(|| {
                CliError::ValidationError(
                    "ve-tos mirror set requires --private-source-storage-vendor when private static credential is used".into(),
                )
            })?,
            "AK": args.private_source_ak.clone().ok_or_else(|| {
                CliError::ValidationError(
                    "ve-tos mirror set requires --private-source-ak when private static credential is used".into(),
                )
            })?,
            "SK": args.private_source_sk.clone().ok_or_else(|| {
                CliError::ValidationError(
                    "ve-tos mirror set requires --private-source-sk when private static credential is used".into(),
                )
            })?,
            "SKEncryptType": args.private_source_sk_encrypt_type.clone(),
        }))
    } else {
        None
    };

    let credential_provider = if args.private_source_role.is_some()
        || args.private_source_region.is_some()
        || static_credential.is_some()
    {
        let mut provider = serde_json::Map::new();
        insert_optional_string(&mut provider, "Role", args.private_source_role.clone());
        insert_optional_string(&mut provider, "Region", args.private_source_region.clone());
        if let Some(static_credential) = static_credential {
            provider.insert("StaticCredential".to_string(), static_credential);
        }
        Some(Value::Object(provider))
    } else {
        None
    };

    let primary = parse_endpoint_providers(
        args.private_source_primary_endpoints.as_deref(),
        args.private_source_bucket_name.clone(),
        credential_provider.clone(),
    )?;
    let follower = parse_endpoint_providers(
        args.private_source_follower_endpoints.as_deref(),
        args.private_source_bucket_name.clone(),
        credential_provider,
    )?;

    Ok(Some(json!({
        "SourceEndpoint": {
            "Primary": primary,
            "Follower": follower,
        }
    })))
}

#[cfg(test)]
fn parse_endpoint_providers(
    endpoints: Option<&str>,
    bucket_name: Option<String>,
    credential_provider: Option<Value>,
) -> Result<Value, CliError> {
    let Some(endpoints) = endpoints else {
        return Ok(Value::Array(Vec::new()));
    };
    let endpoints = parse_string_list(endpoints)?;
    let mut providers = Vec::new();
    if let Value::Array(items) = endpoints {
        for endpoint in items {
            let mut provider = serde_json::Map::new();
            provider.insert("Endpoint".to_string(), endpoint);
            if let Some(bucket_name) = &bucket_name {
                provider.insert("BucketName".to_string(), Value::String(bucket_name.clone()));
            }
            if let Some(credential_provider) = &credential_provider {
                provider.insert(
                    "CredentialProvider".to_string(),
                    credential_provider.clone(),
                );
            }
            providers.push(Value::Object(provider));
        }
    }
    Ok(Value::Array(providers))
}

fn custom_domain_operation(action: &CustomDomainAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        CustomDomainAction::List(args) => op(
            "ve-tos custom-domain list",
            "ListBucketCustomDomain",
            "List bucket custom domain bindings",
            Method::GET,
            args.bucket.require()?,
            query_flag("customdomain"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        CustomDomainAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos custom-domain set")?;
            let mut headers = BTreeMap::new();
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos custom-domain set",
                "PutBucketCustomDomain",
                "Set bucket custom domain binding",
                Method::PUT,
                args.bucket.require()?,
                query_flag("customdomain"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "Rule.Domain(body)",
                        ParameterLocation::Body,
                        true,
                        "Custom domain",
                    ),
                    parameter(
                        "Rule.CertId(body)",
                        ParameterLocation::Body,
                        false,
                        "Certificate ID",
                    ),
                    parameter(
                        "Rule.CertStatus(body)",
                        ParameterLocation::Body,
                        false,
                        "Certificate status",
                    ),
                    parameter(
                        "Rule.Forbidden(body)",
                        ParameterLocation::Body,
                        false,
                        "Whether the custom domain is forbidden",
                    ),
                    parameter(
                        "Rule.ForbiddenReason(body)",
                        ParameterLocation::Body,
                        false,
                        "Forbidden reason",
                    ),
                    parameter(
                        "Rule.Cname(body)",
                        ParameterLocation::Body,
                        false,
                        "CNAME target",
                    ),
                    parameter(
                        "Rule.Protocol(body)",
                        ParameterLocation::Body,
                        false,
                        "Authentication protocol",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        CustomDomainAction::Delete(args) => {
            let query = build_query(&[("customdomain", Some(args.domain.clone()))]);
            op(
                "ve-tos custom-domain delete",
                "DeleteBucketCustomDomain",
                "Delete bucket custom domain binding (requires --force for execution)",
                Method::DELETE,
                args.bucket.require()?,
                query,
                None,
                RiskLevel::High,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "customdomain",
                        ParameterLocation::Query,
                        true,
                        "Custom domain to remove",
                    ),
                    parameter(
                        "force",
                        ParameterLocation::Flag,
                        false,
                        "Required for destructive execution",
                    ),
                ],
                true,
                args.force,
            )
        }
        CustomDomainAction::SetToken(args) => {
            let body = required_config_body(&args.config, "ve-tos custom-domain set-token")?;
            let headers = md5_headers(&body, args.content_md5.clone());
            op_with_headers(
                "ve-tos custom-domain set-token",
                "PutBucketCustomDomainToken",
                "Set custom domain certificate token",
                Method::PUT,
                args.bucket.require()?,
                query_flag_pairs(&["customdomain", "token"]),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "customdomain",
                        ParameterLocation::Query,
                        true,
                        "Custom domain token operation flag",
                    ),
                    parameter(
                        "token",
                        ParameterLocation::Query,
                        true,
                        "Token operation flag",
                    ),
                    parameter(
                        "Domain(body)",
                        ParameterLocation::Body,
                        true,
                        "Custom domain",
                    ),
                    parameter(
                        "Token(body)",
                        ParameterLocation::Body,
                        true,
                        "Certificate token",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        CustomDomainAction::GetToken(args) => op(
            "ve-tos custom-domain get-token",
            "GetBucketCustomDomainToken",
            "Get custom domain certificate token",
            Method::GET,
            args.bucket.require()?,
            build_query(&[
                ("customdomain", Some(args.domain.clone())),
                ("token", Some(String::new())),
            ]),
            None,
            RiskLevel::Low,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "customdomain",
                    ParameterLocation::Query,
                    true,
                    "Custom domain",
                ),
                parameter(
                    "token",
                    ParameterLocation::Query,
                    true,
                    "Token operation flag",
                ),
            ],
            false,
            false,
        ),
    })
}

fn notification_operation(action: &NotificationAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        NotificationAction::Get(args) => op(
            "ve-tos notification get",
            "GetBucketNotificationV2",
            "Get bucket event notification configuration (notification_v2)",
            Method::GET,
            args.bucket.require()?,
            query_flag("notification_v2"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        NotificationAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos notification set")?;
            let mut headers = BTreeMap::new();
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos notification set",
                "PutBucketNotificationV2",
                "Set bucket event notification configuration (notification_v2)",
                Method::PUT,
                args.bucket.require()?,
                query_flag("notification_v2"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "Rules(body)",
                        ParameterLocation::Body,
                        false,
                        "Full notification_v2 Rules array",
                    ),
                    parameter(
                        "Rules[].RuleId(body)",
                        ParameterLocation::Body,
                        false,
                        "Notification rule ID",
                    ),
                    parameter(
                        "Rules[].Events(body)",
                        ParameterLocation::Body,
                        false,
                        "Notification events",
                    ),
                    parameter(
                        "Rules[].Filter.TOSKey.FilterRules(body)",
                        ParameterLocation::Body,
                        false,
                        "Notification filter rules",
                    ),
                    parameter(
                        "Rules[].Destination.VeFaaS(body)",
                        ParameterLocation::Body,
                        false,
                        "VeFaaS destinations",
                    ),
                    parameter(
                        "Rules[].Destination.VeFaaS[].FunctionId(body)",
                        ParameterLocation::Body,
                        false,
                        "VeFaaS function ID",
                    ),
                    parameter(
                        "Rules[].Destination.Kafka(body)",
                        ParameterLocation::Body,
                        false,
                        "Kafka destinations",
                    ),
                    parameter(
                        "Rules[].Destination.Kafka[].Role(body)",
                        ParameterLocation::Body,
                        false,
                        "Kafka role",
                    ),
                    parameter(
                        "Rules[].Destination.Kafka[].InstanceId(body)",
                        ParameterLocation::Body,
                        false,
                        "Kafka instance ID",
                    ),
                    parameter(
                        "Rules[].Destination.Kafka[].Topic(body)",
                        ParameterLocation::Body,
                        false,
                        "Kafka topic / Kafka Topic",
                    ),
                    parameter(
                        "Rules[].Destination.Kafka[].User(body)",
                        ParameterLocation::Body,
                        false,
                        "Kafka user",
                    ),
                    parameter(
                        "Rules[].Destination.Kafka[].Region(body)",
                        ParameterLocation::Body,
                        false,
                        "Kafka region",
                    ),
                    parameter(
                        "Rules[].Destination.RocketMQ(body)",
                        ParameterLocation::Body,
                        false,
                        "RocketMQ destinations",
                    ),
                    parameter(
                        "Rules[].Destination.RocketMQ[].Role(body)",
                        ParameterLocation::Body,
                        false,
                        "RocketMQ role",
                    ),
                    parameter(
                        "Rules[].Destination.RocketMQ[].InstanceId(body)",
                        ParameterLocation::Body,
                        false,
                        "RocketMQ instance ID",
                    ),
                    parameter(
                        "Rules[].Destination.RocketMQ[].Topic(body)",
                        ParameterLocation::Body,
                        false,
                        "RocketMQ topic / RocketMQ Topic",
                    ),
                    parameter(
                        "Rules[].Destination.RocketMQ[].AccessKeyId(body)",
                        ParameterLocation::Body,
                        false,
                        "RocketMQ access key ID / RocketMQ AccessKeyId",
                    ),
                    parameter(
                        "Version(body)",
                        ParameterLocation::Body,
                        true,
                        "notification_v2 version",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
    })
}

fn website_operation(action: &WebsiteAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        WebsiteAction::Get(args) => op(
            "ve-tos website get",
            "GetBucketWebsite",
            "Get bucket website configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("website"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        WebsiteAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos website set")?;
            let mut headers = BTreeMap::new();
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos website set",
                "PutBucketWebsite",
                "Set bucket website configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("website"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "RedirectAllRequestsTo.HostName(body)",
                        ParameterLocation::Body,
                        false,
                        "Redirect-all hostname",
                    ),
                    parameter(
                        "RedirectAllRequestsTo.Protocol(body)",
                        ParameterLocation::Body,
                        false,
                        "Redirect-all protocol",
                    ),
                    parameter(
                        "IndexDocument.Suffix(body)",
                        ParameterLocation::Body,
                        false,
                        "Index document suffix",
                    ),
                    parameter(
                        "IndexDocument.ForbiddenSubDir(body)",
                        ParameterLocation::Body,
                        false,
                        "Whether to forbid sub-directory access",
                    ),
                    parameter(
                        "ErrorDocument.Key(body)",
                        ParameterLocation::Body,
                        false,
                        "Error document key",
                    ),
                    parameter(
                        "RoutingRules(body)",
                        ParameterLocation::Body,
                        false,
                        "Routing rules array",
                    ),
                    parameter(
                        "RoutingRules[].Condition.KeyPrefixEquals(body)",
                        ParameterLocation::Body,
                        false,
                        "Routing rule key prefix",
                    ),
                    parameter(
                        "RoutingRules[].Condition.HttpErrorCodeReturnedEquals(body)",
                        ParameterLocation::Body,
                        false,
                        "Routing rule HTTP error code",
                    ),
                    parameter(
                        "RoutingRules[].Redirect.Protocol(body)",
                        ParameterLocation::Body,
                        false,
                        "Routing rule redirect protocol",
                    ),
                    parameter(
                        "RoutingRules[].Redirect.HostName(body)",
                        ParameterLocation::Body,
                        false,
                        "Routing rule redirect hostname",
                    ),
                    parameter(
                        "RoutingRules[].Redirect.ReplaceKeyPrefixWith(body)",
                        ParameterLocation::Body,
                        false,
                        "Routing rule ReplaceKeyPrefixWith",
                    ),
                    parameter(
                        "RoutingRules[].Redirect.ReplaceKeyWith(body)",
                        ParameterLocation::Body,
                        false,
                        "Routing rule ReplaceKeyWith",
                    ),
                    parameter(
                        "RoutingRules[].Redirect.HttpRedirectCode(body)",
                        ParameterLocation::Body,
                        false,
                        "Routing rule HTTP redirect code",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        WebsiteAction::Delete(args) => op(
            "ve-tos website delete",
            "DeleteBucketWebsite",
            "Delete bucket website configuration (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("website"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn mirror_operation(action: &MirrorAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        MirrorAction::Get(args) => op(
            "ve-tos mirror get",
            "GetBucketMirrorBack",
            "Get bucket mirror back-to-source rules",
            Method::GET,
            args.bucket.require()?,
            query_flag("mirror"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        MirrorAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos mirror set")?;
            let mut headers = BTreeMap::new();
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos mirror set",
                "PutBucketMirrorBack",
                "Set bucket mirror back-to-source rules",
                Method::PUT,
                args.bucket.require()?,
                query_flag("mirror"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "Rules(body)",
                        ParameterLocation::Body,
                        false,
                        "Full mirror rule array",
                    ),
                    parameter(
                        "Rules[].ID(body)",
                        ParameterLocation::Body,
                        false,
                        "Mirror rule ID",
                    ),
                    parameter(
                        "Rules[].Condition.HttpCode(body)",
                        ParameterLocation::Body,
                        false,
                        "Condition HTTP code",
                    ),
                    parameter(
                        "Rules[].Condition.KeyPrefix(body)",
                        ParameterLocation::Body,
                        false,
                        "Condition key prefix",
                    ),
                    parameter(
                        "Rules[].Condition.KeySuffix(body)",
                        ParameterLocation::Body,
                        false,
                        "Condition key suffix",
                    ),
                    parameter(
                        "Rules[].Condition.AllowHost(body)",
                        ParameterLocation::Body,
                        false,
                        "Condition allow hosts",
                    ),
                    parameter(
                        "Rules[].Condition.HttpMethod(body)",
                        ParameterLocation::Body,
                        false,
                        "Condition HTTP methods",
                    ),
                    parameter(
                        "Rules[].Redirect.RedirectType(body)",
                        ParameterLocation::Body,
                        false,
                        "Redirect type",
                    ),
                    parameter(
                        "Rules[].Redirect.FetchSourceOnRedirect(body)",
                        ParameterLocation::Body,
                        false,
                        "Fetch source on redirect",
                    ),
                    parameter(
                        "Rules[].Redirect.PassQuery(body)",
                        ParameterLocation::Body,
                        false,
                        "Pass query string",
                    ),
                    parameter(
                        "Rules[].Redirect.FollowRedirect(body)",
                        ParameterLocation::Body,
                        false,
                        "Follow redirect",
                    ),
                    parameter(
                        "Rules[].Redirect.MirrorHeader(body)",
                        ParameterLocation::Body,
                        false,
                        "Mirror header rules",
                    ),
                    parameter(
                        "Rules[].Redirect.PublicSource(body)",
                        ParameterLocation::Body,
                        false,
                        "Public source config",
                    ),
                    parameter(
                        "Rules[].Redirect.Transform(body)",
                        ParameterLocation::Body,
                        false,
                        "Transform config",
                    ),
                    parameter(
                        "Rules[].Redirect.FetchHeaderToMetaDataRules(body)",
                        ParameterLocation::Body,
                        false,
                        "FetchHeaderToMetaDataRules",
                    ),
                    parameter(
                        "Rules[].Redirect.PrivateSource(body)",
                        ParameterLocation::Body,
                        false,
                        "Private source config",
                    ),
                    parameter(
                        "Rules[].Redirect.FetchSourceOnRedirectWithQuery(body)",
                        ParameterLocation::Body,
                        false,
                        "Fetch source on redirect with query",
                    ),
                    parameter(
                        "Rules[].Redirect.PassStatusCodeFromSource(body)",
                        ParameterLocation::Body,
                        false,
                        "Pass status codes from source",
                    ),
                    parameter(
                        "Rules[].Redirect.PassHeaderFromSource(body)",
                        ParameterLocation::Body,
                        false,
                        "Pass headers from source",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        MirrorAction::Delete(args) => op(
            "ve-tos mirror delete",
            "DeleteBucketMirrorBack",
            "Delete bucket mirror back-to-source rules (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("mirror"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn inventory_operation(action: &InventoryAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        InventoryAction::Get(args) => {
            let query = build_query(&[
                ("inventory", Some(String::new())),
                ("id", Some(args.id.clone())),
            ]);
            op(
                "ve-tos inventory get",
                "GetBucketInventory",
                "Get bucket inventory configuration",
                Method::GET,
                args.bucket.require()?,
                query,
                None,
                RiskLevel::Low,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "id",
                        ParameterLocation::Query,
                        true,
                        "Inventory configuration ID",
                    ),
                ],
                false,
                false,
            )
        }
        InventoryAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos inventory set")?;
            let query = build_query(&[
                ("inventory", Some(String::new())),
                ("id", Some(args.id.clone())),
            ]);
            let mut headers = BTreeMap::new();
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos inventory set",
                "PutBucketInventory",
                "Set bucket inventory configuration",
                Method::PUT,
                args.bucket.require()?,
                query,
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "id",
                        ParameterLocation::Query,
                        true,
                        "Inventory configuration ID",
                    ),
                    parameter(
                        "Id(body)",
                        ParameterLocation::Body,
                        true,
                        "Inventory configuration ID",
                    ),
                    parameter(
                        "IsEnabled(body)",
                        ParameterLocation::Body,
                        true,
                        "Whether the inventory is enabled",
                    ),
                    parameter(
                        "Filter.Prefix(body)",
                        ParameterLocation::Body,
                        false,
                        "Inventory filter prefix",
                    ),
                    parameter(
                        "Destination.TOSBucketDestination.Format(body)",
                        ParameterLocation::Body,
                        true,
                        "Inventory destination format",
                    ),
                    parameter(
                        "Destination.TOSBucketDestination.AccountID(body)",
                        ParameterLocation::Body,
                        true,
                        "Inventory destination account ID",
                    ),
                    parameter(
                        "Destination.TOSBucketDestination.Role(body)",
                        ParameterLocation::Body,
                        true,
                        "Inventory destination role",
                    ),
                    parameter(
                        "Destination.TOSBucketDestination.Bucket(body)",
                        ParameterLocation::Body,
                        true,
                        "Inventory destination bucket",
                    ),
                    parameter(
                        "Destination.TOSBucketDestination.Prefix(body)",
                        ParameterLocation::Body,
                        false,
                        "Inventory destination prefix",
                    ),
                    parameter(
                        "Schedule.Frequency(body)",
                        ParameterLocation::Body,
                        true,
                        "Inventory schedule frequency",
                    ),
                    parameter(
                        "IncludedObjectVersions(body)",
                        ParameterLocation::Body,
                        true,
                        "Included object versions",
                    ),
                    parameter(
                        "OptionalFields.Field(body)",
                        ParameterLocation::Body,
                        false,
                        "Optional inventory fields",
                    ),
                    parameter(
                        "IsUnCompressed(body)",
                        ParameterLocation::Body,
                        true,
                        "Whether the output is uncompressed",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        InventoryAction::Delete(args) => {
            let query = build_query(&[
                ("inventory", Some(String::new())),
                ("id", Some(args.id.clone())),
            ]);
            op(
                "ve-tos inventory delete",
                "DeleteBucketInventory",
                "Delete bucket inventory configuration (requires --force for execution)",
                Method::DELETE,
                args.bucket.require()?,
                query,
                None,
                RiskLevel::High,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "id",
                        ParameterLocation::Query,
                        true,
                        "Inventory configuration ID",
                    ),
                    parameter(
                        "force",
                        ParameterLocation::Flag,
                        false,
                        "Required for destructive execution",
                    ),
                ],
                true,
                args.force,
            )
        }
        InventoryAction::List(args) => {
            let query = build_query(&[
                ("inventory", Some(String::new())),
                ("continuation-token", args.continuation_token.clone()),
            ]);
            op(
                "ve-tos inventory list",
                "ListBucketInventory",
                "List bucket inventory configurations",
                Method::GET,
                args.bucket.require()?,
                query,
                None,
                RiskLevel::Low,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "continuation-token",
                        ParameterLocation::Query,
                        false,
                        "Inventory list continuation token",
                    ),
                ],
                false,
                false,
            )
        }
    })
}

fn real_time_log_operation(action: &RealTimeLogAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        RealTimeLogAction::Get(args) => op(
            "ve-tos real-time-log get",
            "GetBucketRealTimeLog",
            "Get bucket real-time log configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("realtimeLog"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        RealTimeLogAction::Set(args) => {
            let body = real_time_log_body(args)?;
            let mut headers = BTreeMap::new();
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos real-time-log set",
                "PutBucketRealTimeLog",
                "Set bucket real-time log configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("realtimeLog"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "Role(body)",
                        ParameterLocation::Body,
                        true,
                        "IAM role for log delivery",
                    ),
                    parameter(
                        "AccessLogConfiguration.UseServiceTopic(body)",
                        ParameterLocation::Body,
                        true,
                        "Whether to use service-managed topic",
                    ),
                    parameter(
                        "AccessLogConfiguration.TLSProjectID(body)",
                        ParameterLocation::Body,
                        false,
                        "TLS project ID / TLS Project ID",
                    ),
                    parameter(
                        "AccessLogConfiguration.TLSTopicID(body)",
                        ParameterLocation::Body,
                        false,
                        "TLS topic ID / TLS Topic ID",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
        RealTimeLogAction::Delete(args) => op(
            "ve-tos real-time-log delete",
            "DeleteBucketRealTimeLog",
            "Delete bucket real-time log configuration (requires --force for execution)",
            Method::DELETE,
            args.bucket.require()?,
            query_flag("realtimeLog"),
            None,
            RiskLevel::High,
            vec![
                parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                parameter(
                    "force",
                    ParameterLocation::Flag,
                    false,
                    "Required for destructive execution",
                ),
            ],
            true,
            args.force,
        ),
    })
}

fn worm_operation(action: &WormAction) -> Result<BucketConfigOperation, CliError> {
    Ok(match action {
        WormAction::Get(args) => op(
            "ve-tos worm get",
            "GetBucketObjectLockConfiguration",
            "Get bucket object lock configuration",
            Method::GET,
            args.bucket.require()?,
            query_flag("object-lock"),
            None,
            RiskLevel::Low,
            vec![parameter(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name",
            )],
            false,
            false,
        ),
        WormAction::Set(args) => {
            let body = required_config_body(&args.config, "ve-tos worm set")?;
            let mut headers = BTreeMap::new();
            headers.insert(
                "Content-MD5".to_string(),
                args.content_md5
                    .clone()
                    .unwrap_or_else(|| content_md5_base64(&body)),
            );
            op_with_headers(
                "ve-tos worm set",
                "PutBucketObjectLockConfiguration",
                "Set bucket object lock configuration",
                Method::PUT,
                args.bucket.require()?,
                query_flag("object-lock"),
                headers,
                Some(body),
                RiskLevel::Medium,
                vec![
                    parameter("bucket", ParameterLocation::Path, true, "Bucket name"),
                    parameter(
                        "ObjectLockEnabled(body)",
                        ParameterLocation::Body,
                        true,
                        "Object lock enabled status",
                    ),
                    parameter(
                        "Rule.DefaultRetention.Mode(body)",
                        ParameterLocation::Body,
                        false,
                        "Default retention mode",
                    ),
                    parameter(
                        "Rule.DefaultRetention.Days(body)",
                        ParameterLocation::Body,
                        false,
                        "Default retention days",
                    ),
                    parameter(
                        "Rule.DefaultRetention.Years(body)",
                        ParameterLocation::Body,
                        false,
                        "Default retention years",
                    ),
                    parameter(
                        "Content-MD5",
                        ParameterLocation::Header,
                        false,
                        "Content-MD5 header (auto-computed when omitted)",
                    ),
                ],
                false,
                false,
            )
        }
    })
}

fn op(
    command: &'static str,
    api: &'static str,
    description: &'static str,
    method: Method,
    bucket: String,
    query: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
    risk: RiskLevel,
    parameters: Vec<CommandParameter>,
    destructive: bool,
    force: bool,
) -> BucketConfigOperation {
    op_with_headers(
        command,
        api,
        description,
        method,
        bucket,
        query,
        BTreeMap::new(),
        body,
        risk,
        parameters,
        destructive,
        force,
    )
}

fn op_with_headers(
    command: &'static str,
    api: &'static str,
    description: &'static str,
    method: Method,
    bucket: String,
    query: BTreeMap<String, String>,
    mut headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
    risk: RiskLevel,
    parameters: Vec<CommandParameter>,
    destructive: bool,
    force: bool,
) -> BucketConfigOperation {
    if body.is_some() {
        headers
            .entry("content-type".to_string())
            .or_insert_with(|| "application/json".to_string());
    }
    BucketConfigOperation {
        command,
        api,
        description,
        method,
        bucket,
        query,
        headers,
        body,
        risk,
        parameters,
        supports_pipe: false,
        destructive,
        force,
    }
}

fn dry_run(op: &BucketConfigOperation) -> DryRunResult {
    DryRunResult {
        action: op.command.to_string(),
        dry_run: true,
        impact: Impact {
            affected_objects: if matches!(op.risk, RiskLevel::High | RiskLevel::Critical) {
                1
            } else {
                0
            },
            affected_bytes: 0,
            risk_level: format!("{:?}", op.risk).to_lowercase(),
            estimated_duration: Some("< 1s".to_string()),
            scanned_count: None,
            preview_truncated: None,
        },
        plan: vec![format!("{} tos://{}", op.method.as_str(), op.bucket)],
        warnings: {
            let mut warnings = Vec::new();
            if op.body.is_some() {
                warnings.push(
                    "Request body is omitted from dry-run output; validate your input before execution."
                        .to_string(),
                );
            }
            if op.destructive {
                warnings
                    .push("This command is destructive; execution requires --force.".to_string());
            }
            warnings
        },
        confirm_command: None,
    }
}

fn describe_action(op: &BucketConfigOperation) -> CommandDescription {
    let mut parameters: Vec<CommandParameter> = op
        .parameters
        .iter()
        .filter(|param| {
            !matches!(param.location, ParameterLocation::Body) || param.name == "config(body)"
        })
        .map(|param| CommandParameter {
            name: param.name.clone(),
            location: copy_location(&param.location),
            required: param.required,
            description: param.description.clone(),
            ..Default::default()
        })
        .collect();
    if op.body.is_some() && !parameters.iter().any(|param| param.name == "config(body)") {
        parameters.push(CommandParameter {
            name: "config(body)".to_string(),
            location: ParameterLocation::Body,
            required: true,
            description: "Full request body JSON via --config".to_string(),
            ..Default::default()
        });
    }
    CommandDescription {
        command: op.command.to_string(),
        layer: CommandLayer::LowLevel,
        api: Some(op.api.to_string()),
        description: op.description.to_string(),
        risk_level: op.risk,
        supports_dry_run: true,
        supports_pipe: op.supports_pipe,
        parameters: Some(parameters),
        scenario_routing: Some(HashMap::from([
            (
                "English Example".to_string(),
                format!("{} --help", op.command),
            ),
            (
                "Describe Example".to_string(),
                format!("{} --describe", op.command),
            ),
        ])),
        related_commands: None,
        low_level_apis: None,
        // [G5] Spec-mandated alias. For low-level commands, `wraps_apis` is the
        // single underlying OpenAPI operation.
        wraps_apis: Some(vec![op.api.to_string()]),
        // [G5] Generic JMESPath/jq examples for inspecting low-level responses.
        output_filter_examples: Some(vec![
            format!(
                "{} --output json | jq '.data'",
                crate::registry::public_tos_command(op.command)
            ),
            format!(
                "{} --query 'data'",
                crate::registry::public_tos_command(op.command)
            ),
        ]),
        // [G5] Low-level commands forward arbitrary --config JSON; surface the
        // typical quoting trap.
        shell_quoting_tips: Some(vec![
            "When passing --config inline, single-quote the JSON payload: --config '{\"key\":\"value\"}'".to_string(),
            "Prefer `--config @path/to/file.json` for any config larger than a one-liner.".to_string(),
        ]),
        ..Default::default()
    }
}

fn copy_location(location: &ParameterLocation) -> ParameterLocation {
    match location {
        ParameterLocation::Path => ParameterLocation::Path,
        ParameterLocation::Query => ParameterLocation::Query,
        ParameterLocation::Header => ParameterLocation::Header,
        ParameterLocation::Body => ParameterLocation::Body,
        ParameterLocation::Flag => ParameterLocation::Flag,
    }
}

fn describe_group(command: &str, description: &str, subcommands: &[(&str, &str)]) -> Value {
    json!({
        "command": command,
        "layer": "low_level",
        "description": description,
        "supports_help": true,
        "supports_describe": true,
        "subcommands": subcommands
            .iter()
            .map(|(name, desc)| json!({ "name": name, "description": desc }))
            .collect::<Vec<Value>>()
    })
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

fn query_flag(flag: &str) -> BTreeMap<String, String> {
    build_query(&[(flag, Some(String::new()))])
}

fn query_flag_pairs(flags: &[&str]) -> BTreeMap<String, String> {
    flags
        .iter()
        .map(|flag| ((*flag).to_string(), String::new()))
        .collect()
}

#[cfg(test)]
fn tagging_body(args: &TaggingSetArgs) -> Result<Vec<u8>, CliError> {
    let tags = parse_tagging_entries(&args.tags)?;
    serde_json::to_vec(&json!({ "TagSet": { "Tags": tags } })).map_err(CliError::Json)
}

#[cfg(test)]
fn acl_body(args: &AclSetArgs) -> Result<Vec<u8>, CliError> {
    let structured_grant = [
        args.grantee_type.is_some(),
        args.grantee_id.is_some(),
        args.grantee_canned.is_some(),
        args.permission.is_some(),
    ]
    .into_iter()
    .any(|present| present);
    if args.grants.is_some() && structured_grant {
        return Err(CliError::ValidationError(
            "ve-tos acl set cannot mix --grants with single-grant body fields".into(),
        ));
    }
    if args.bucket_acl_delivered == Some(true) && args.owner_id.is_none() {
        return Err(CliError::ValidationError(
            "ve-tos acl set requires --owner-id when --bucket-acl-delivered true is used".into(),
        ));
    }

    let mut body = serde_json::Map::new();
    if let Some(owner_id) = &args.owner_id {
        body.insert("Owner".to_string(), json!({ "ID": owner_id }));
    }
    if let Some(bucket_acl_delivered) = args.bucket_acl_delivered {
        body.insert(
            "BucketAclDelivered".to_string(),
            Value::Bool(bucket_acl_delivered),
        );
    }
    if let Some(grants) = &args.grants {
        body.insert("Grants".to_string(), parse_json_array(grants, "Grants")?);
    } else if structured_grant {
        let grantee_type = normalize_enum_value(
            "ve-tos acl set",
            "grantee-type",
            args.grantee_type.as_deref().unwrap_or_default(),
            &["CanonicalUser", "Group"],
        )?;
        let permission = normalize_enum_value(
            "ve-tos acl set",
            "permission",
            args.permission.as_deref().unwrap_or_default(),
            &[
                "READ",
                "READ_NON_LIST",
                "WRITE",
                "READ_ACP",
                "WRITE_ACP",
                "FULL_CONTROL",
            ],
        )?;
        let mut grantee = serde_json::Map::new();
        grantee.insert("Type".to_string(), Value::String(grantee_type.clone()));
        match grantee_type.as_str() {
            "CanonicalUser" => {
                let id = args.grantee_id.clone().ok_or_else(|| {
                    CliError::ValidationError(
                        "ve-tos acl set requires --grantee-id when --grantee-type CanonicalUser is used"
                            .into(),
                    )
                })?;
                if args.grantee_canned.is_some() {
                    return Err(CliError::ValidationError(
                        "ve-tos acl set cannot use --grantee-canned with --grantee-type CanonicalUser"
                            .into(),
                    ));
                }
                grantee.insert("ID".to_string(), Value::String(id));
            }
            "Group" => {
                let canned = normalize_enum_value(
                    "ve-tos acl set",
                    "grantee-canned",
                    args.grantee_canned.as_deref().unwrap_or_default(),
                    &["AllUsers", "AuthenticatedUsers"],
                )?;
                if args.grantee_id.is_some() {
                    return Err(CliError::ValidationError(
                        "ve-tos acl set cannot use --grantee-id with --grantee-type Group".into(),
                    ));
                }
                grantee.insert("Canned".to_string(), Value::String(canned));
            }
            _ => unreachable!("normalized above"),
        }
        body.insert(
            "Grants".to_string(),
            Value::Array(vec![json!({
                "Grantee": Value::Object(grantee),
                "Permission": permission,
            })]),
        );
    }
    if body.is_empty() {
        return Err(CliError::ValidationError(
            "ve-tos acl set body mode requires at least one ACL body field".into(),
        ));
    }
    serde_json::to_vec(&Value::Object(body)).map_err(CliError::Json)
}

#[cfg(test)]
fn lifecycle_body(args: &LifecycleSetArgs) -> Result<Vec<u8>, CliError> {
    let structured = [
        args.id.is_some(),
        args.prefix.is_some(),
        args.status.is_some(),
        args.tags.is_some(),
        args.filter.is_some(),
        args.expiration.is_some(),
        args.noncurrent_version_expiration.is_some(),
        args.abort_incomplete_multipart_upload.is_some(),
        args.transitions.is_some(),
        args.noncurrent_version_transitions.is_some(),
        args.access_time_transitions.is_some(),
        args.noncurrent_version_access_time_transitions.is_some(),
    ]
    .into_iter()
    .any(|present| present);
    // [Review Fix #4] Lifecycle 不能只保留 raw JSON 透传；字段级 CLI 与整包 JSON 二选一，避免 schema 不可见。
    ensure_payload_mode("lifecycle", args.rules.is_some(), structured)?;

    if let Some(rules) = &args.rules {
        return json_bytes_from_input(rules);
    }

    let status = args.status.clone().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos lifecycle set requires --status when using field-level lifecycle flags".into(),
        )
    })?;
    let mut rule = serde_json::Map::new();
    rule.insert("Status".to_string(), Value::String(status));
    insert_optional_string(&mut rule, "ID", args.id.clone());
    insert_optional_string(&mut rule, "Prefix", args.prefix.clone());
    insert_optional_parsed(&mut rule, "Tags", args.tags.as_deref(), parse_tag_set)?;
    insert_optional_parsed(
        &mut rule,
        "Filter",
        args.filter.as_deref(),
        parse_json_value,
    )?;
    insert_optional_parsed(
        &mut rule,
        "Expiration",
        args.expiration.as_deref(),
        parse_json_value,
    )?;
    insert_optional_parsed(
        &mut rule,
        "NoncurrentVersionExpiration",
        args.noncurrent_version_expiration.as_deref(),
        parse_json_value,
    )?;
    insert_optional_parsed(
        &mut rule,
        "AbortIncompleteMultipartUpload",
        args.abort_incomplete_multipart_upload.as_deref(),
        parse_json_value,
    )?;
    insert_optional_parsed(
        &mut rule,
        "Transitions",
        args.transitions.as_deref(),
        parse_json_value,
    )?;
    insert_optional_parsed(
        &mut rule,
        "NoncurrentVersionTransitions",
        args.noncurrent_version_transitions.as_deref(),
        parse_json_value,
    )?;
    insert_optional_parsed(
        &mut rule,
        "AccessTimeTransitions",
        args.access_time_transitions.as_deref(),
        parse_json_value,
    )?;
    insert_optional_parsed(
        &mut rule,
        "NoncurrentVersionAccessTimeTransitions",
        args.noncurrent_version_access_time_transitions.as_deref(),
        parse_json_value,
    )?;
    serde_json::to_vec(&json!({ "Rules": [Value::Object(rule)] })).map_err(CliError::Json)
}

#[cfg(test)]
fn cors_body(args: &CorsSetArgs) -> Result<Vec<u8>, CliError> {
    let structured = [
        args.allowed_origins.is_some(),
        args.allowed_methods.is_some(),
        args.allowed_headers.is_some(),
        args.expose_headers.is_some(),
        args.max_age_seconds.is_some(),
        args.response_vary.is_some(),
    ]
    .into_iter()
    .any(|present| present);
    ensure_payload_mode("cors", args.rules.is_some(), structured)?;

    if let Some(rules) = &args.rules {
        return json_bytes_from_input(rules);
    }

    let allowed_origins = parse_string_list(args.allowed_origins.as_deref().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos cors set requires --allowed-origins when using field-level CORS flags".into(),
        )
    })?)?;
    let allowed_methods = parse_string_list(args.allowed_methods.as_deref().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos cors set requires --allowed-methods when using field-level CORS flags".into(),
        )
    })?)?;

    let mut rule = serde_json::Map::new();
    rule.insert("AllowedOrigins".to_string(), allowed_origins);
    rule.insert("AllowedMethods".to_string(), allowed_methods);
    insert_optional_parsed(
        &mut rule,
        "AllowedHeaders",
        args.allowed_headers.as_deref(),
        parse_string_list,
    )?;
    insert_optional_parsed(
        &mut rule,
        "ExposeHeaders",
        args.expose_headers.as_deref(),
        parse_string_list,
    )?;
    if let Some(max_age_seconds) = args.max_age_seconds {
        rule.insert("MaxAgeSeconds".to_string(), json!(max_age_seconds));
    }
    if let Some(response_vary) = args.response_vary {
        rule.insert("ResponseVary".to_string(), json!(response_vary));
    }
    serde_json::to_vec(&json!({ "CORSRules": [Value::Object(rule)] })).map_err(CliError::Json)
}

#[cfg(test)]
fn replication_body(args: &ReplicationSetArgs) -> Result<Vec<u8>, CliError> {
    let structured = [
        args.role.is_some(),
        args.id.is_some(),
        args.status.is_some(),
        args.prefix_set.is_some(),
        args.tags.is_some(),
        args.destination_bucket.is_some(),
        args.destination_location.is_some(),
        args.destination_storage_class.is_some(),
        args.storage_class_inherit_directive.is_some(),
        args.historical_object_replication.is_some(),
        args.transfer_type.is_some(),
        args.access_control_translation_owner.is_some(),
    ]
    .into_iter()
    .any(|present| present);
    // [Review Fix #5] Replication 字段较多，保留整包 JSON 兼容的同时显式建模核心 schema 字段。
    ensure_payload_mode("replication", args.rules.is_some(), structured)?;

    if let Some(rules) = &args.rules {
        return json_bytes_from_input(rules);
    }

    let role = args.role.clone().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos replication set requires --role when using field-level replication flags"
                .into(),
        )
    })?;
    let status = args.status.clone().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos replication set requires --status when using field-level replication flags"
                .into(),
        )
    })?;
    let destination_bucket = args.destination_bucket.clone().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos replication set requires --destination-bucket when using field-level replication flags".into(),
        )
    })?;
    let destination_location = args.destination_location.clone().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos replication set requires --destination-location when using field-level replication flags".into(),
        )
    })?;

    let mut destination = serde_json::Map::new();
    destination.insert("Bucket".to_string(), Value::String(destination_bucket));
    destination.insert("Location".to_string(), Value::String(destination_location));
    insert_optional_string(
        &mut destination,
        "StorageClass",
        args.destination_storage_class.clone(),
    );
    insert_optional_string(
        &mut destination,
        "StorageClassInheritDirective",
        args.storage_class_inherit_directive.clone(),
    );
    if let Some(owner) = &args.access_control_translation_owner {
        destination.insert(
            "AccessControlTranslation".to_string(),
            json!({ "Owner": owner }),
        );
    }

    let mut rule = serde_json::Map::new();
    rule.insert("Status".to_string(), Value::String(status));
    rule.insert("Destination".to_string(), Value::Object(destination));
    insert_optional_string(&mut rule, "ID", args.id.clone());
    insert_optional_parsed(
        &mut rule,
        "PrefixSet",
        args.prefix_set.as_deref(),
        parse_string_list,
    )?;
    insert_optional_parsed(&mut rule, "Tags", args.tags.as_deref(), parse_tag_set)?;
    insert_optional_string(
        &mut rule,
        "HistoricalObjectReplication",
        args.historical_object_replication.clone(),
    );
    insert_optional_string(&mut rule, "TransferType", args.transfer_type.clone());

    serde_json::to_vec(&json!({
        "Role": role,
        "Rules": [Value::Object(rule)],
    }))
    .map_err(CliError::Json)
}

#[cfg(test)]
fn encryption_body(args: &EncryptionSetArgs) -> Result<Vec<u8>, CliError> {
    if args.sse_algorithm.eq_ignore_ascii_case("KMS") && args.kms_master_key_id.is_none() {
        return Err(CliError::ValidationError(
            "ve-tos encryption set requires --kms-master-key-id when --sse-algorithm KMS is used"
                .into(),
        ));
    }

    let mut defaults = serde_json::Map::new();
    defaults.insert(
        "SSEAlgorithm".to_string(),
        Value::String(args.sse_algorithm.clone()),
    );
    insert_optional_string(
        &mut defaults,
        "KMSDataEncryption",
        args.kms_data_encryption.clone(),
    );
    insert_optional_string(
        &mut defaults,
        "KMSMasterKeyID",
        args.kms_master_key_id.clone(),
    );
    serde_json::to_vec(&json!({
        "Rule": {
            "ApplyServerSideEncryptionByDefault": Value::Object(defaults)
        }
    }))
    .map_err(CliError::Json)
}

fn ensure_payload_mode(
    command: &str,
    has_raw_payload: bool,
    has_structured_payload: bool,
) -> Result<(), CliError> {
    if has_raw_payload && has_structured_payload {
        return Err(CliError::ValidationError(format!(
            "ve-tos {command} set cannot mix raw JSON payload input with field-level flags"
        )));
    }
    if has_raw_payload || has_structured_payload {
        return Ok(());
    }
    Err(CliError::ValidationError(format!(
        "ve-tos {command} set requires either a full JSON payload or field-level schema flags"
    )))
}

fn json_bytes_from_input(input: &str) -> Result<Vec<u8>, CliError> {
    serde_json::to_vec(&read_json_input(input)?).map_err(CliError::Json)
}

#[cfg(test)]
fn parse_json_value(input: &str) -> Result<Value, CliError> {
    read_json_input(input)
}

#[cfg(test)]
fn parse_json_array(input: &str, field_name: &str) -> Result<Value, CliError> {
    match read_json_input(input)? {
        Value::Array(items) => Ok(Value::Array(items)),
        other => Err(CliError::ValidationError(format!(
            "expected `{field_name}` to be a JSON array, got {other}"
        ))),
    }
}

fn parse_string_list(input: &str) -> Result<Value, CliError> {
    if let Ok(value) = read_json_input(input) {
        return match value {
            Value::Array(_) => Ok(value),
            other => Err(CliError::ValidationError(format!(
                "expected JSON array input, got {}",
                other
            ))),
        };
    }

    let items = input
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| Value::String(item.to_string()))
        .collect::<Vec<_>>();
    if items.is_empty() {
        return Err(CliError::ValidationError(
            "expected a non-empty comma-separated list or JSON array".into(),
        ));
    }
    Ok(Value::Array(items))
}

#[cfg(test)]
fn parse_tag_set(input: &str) -> Result<Value, CliError> {
    if let Ok(value) = read_json_input(input) {
        return Ok(value);
    }

    let mut items = Vec::new();
    match parse_kv_pairs(input) {
        Value::Object(map) => {
            for (key, value) in map {
                items.push(json!({
                    "Key": key,
                    "Value": value.as_str().unwrap_or_default(),
                }));
            }
        }
        _ => unreachable!("parse_kv_pairs always returns an object"),
    }
    Ok(Value::Array(items))
}

fn parse_filter_rules(input: &str) -> Result<Value, CliError> {
    if let Ok(value) = read_json_input(input) {
        return match value {
            Value::Array(_) => Ok(value),
            other => Err(CliError::ValidationError(format!(
                "expected filter rules to be a JSON array, got {other}"
            ))),
        };
    }

    let mut rules = Vec::new();
    match parse_kv_pairs(input) {
        Value::Object(map) => {
            for (name, value) in map {
                rules.push(json!({
                    "Name": name,
                    "Value": value.as_str().unwrap_or_default(),
                }));
            }
        }
        _ => unreachable!("parse_kv_pairs always returns an object"),
    }
    Ok(Value::Array(rules))
}

#[cfg(test)]
fn parse_key_value_objects(input: &str) -> Result<Value, CliError> {
    if let Ok(value) = read_json_input(input) {
        return match value {
            Value::Array(_) => Ok(value),
            other => Err(CliError::ValidationError(format!(
                "expected key/value JSON array, got {other}"
            ))),
        };
    }

    let mut items = Vec::new();
    match parse_kv_pairs(input) {
        Value::Object(map) => {
            for (key, value) in map {
                items.push(json!({
                    "Key": key,
                    "Value": value.as_str().unwrap_or_default(),
                }));
            }
        }
        _ => unreachable!("parse_kv_pairs always returns an object"),
    }
    Ok(Value::Array(items))
}

#[cfg(test)]
fn parse_fetch_header_metadata_rules(input: &str) -> Result<Value, CliError> {
    if let Ok(value) = read_json_input(input) {
        return match value {
            Value::Array(_) => Ok(value),
            other => Err(CliError::ValidationError(format!(
                "expected FetchHeaderToMetaDataRules to be a JSON array, got {other}"
            ))),
        };
    }

    let mut rules = Vec::new();
    match parse_kv_pairs(input) {
        Value::Object(map) => {
            for (source_header, value) in map {
                rules.push(json!({
                    "SourceHeader": source_header,
                    "MetaDataSuffix": value.as_str().unwrap_or_default(),
                }));
            }
        }
        _ => unreachable!("parse_kv_pairs always returns an object"),
    }
    Ok(Value::Array(rules))
}

#[cfg(test)]
fn parse_int_list(input: &str) -> Result<Value, CliError> {
    if let Ok(value) = read_json_input(input) {
        return match value {
            Value::Array(_) => Ok(value),
            other => Err(CliError::ValidationError(format!(
                "expected JSON array of integers, got {other}"
            ))),
        };
    }

    let mut items = Vec::new();
    for part in input
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let parsed = part.parse::<i64>().map_err(|err| {
            CliError::ValidationError(format!(
                "expected comma-separated integers, failed to parse `{part}`: {err}"
            ))
        })?;
        items.push(json!(parsed));
    }
    if items.is_empty() {
        return Err(CliError::ValidationError(
            "expected a non-empty comma-separated integer list or JSON array".into(),
        ));
    }
    Ok(Value::Array(items))
}

#[cfg(test)]
fn parse_tagging_entries(input: &str) -> Result<Value, CliError> {
    if let Ok(value) = read_json_input(input) {
        return match value {
            Value::Array(items) => Ok(Value::Array(items)),
            Value::Object(map) => {
                if let Some(tag_set) = map.get("TagSet").and_then(|value| value.get("Tags")) {
                    return match tag_set {
                        Value::Array(items) => Ok(Value::Array(items.clone())),
                        other => Err(CliError::ValidationError(format!(
                            "expected `TagSet.Tags` to be a JSON array, got {other}"
                        ))),
                    };
                }
                if let Some(tags) = map.get("Tags") {
                    return match tags {
                        Value::Array(items) => Ok(Value::Array(items.clone())),
                        other => Err(CliError::ValidationError(format!(
                            "expected `Tags` to be a JSON array, got {other}"
                        ))),
                    };
                }
                Err(CliError::ValidationError(
                    "expected tagging JSON to be either an array or an object containing Tags"
                        .into(),
                ))
            }
            other => Err(CliError::ValidationError(format!(
                "expected tagging JSON to be an array or object, got {other}"
            ))),
        };
    }
    parse_tag_set(input)
}

fn normalize_enum_value(
    command: &str,
    field: &str,
    value: &str,
    allowed: &[&str],
) -> Result<String, CliError> {
    allowed
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(value))
        .map(|candidate| (*candidate).to_string())
        .ok_or_else(|| {
            CliError::ValidationError(format!(
                "{command} requires `{field}` to be one of: {}",
                allowed.join(", ")
            ))
        })
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

fn insert_optional_string(
    map: &mut serde_json::Map<String, Value>,
    key: &str,
    value: Option<String>,
) {
    if let Some(value) = value {
        map.insert(key.to_string(), Value::String(value));
    }
}

#[cfg(test)]
fn insert_optional_bool(map: &mut serde_json::Map<String, Value>, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        map.insert(key.to_string(), Value::Bool(value));
    }
}

#[cfg(test)]
fn insert_optional_parsed(
    map: &mut serde_json::Map<String, Value>,
    key: &str,
    value: Option<&str>,
    parser: fn(&str) -> Result<Value, CliError>,
) -> Result<(), CliError> {
    if let Some(value) = value {
        map.insert(key.to_string(), parser(value)?);
    }
    Ok(())
}

fn content_md5_base64(body: &[u8]) -> String {
    let digest = md5::compute(body);
    BASE64_STANDARD.encode(digest.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_storage_class_uses_header_without_body() {
        let op = storageclass_operation(&StorageclassAction::Set(StorageclassSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            storage_class: "STANDARD".to_string(),
        }))
        .expect("storageclass op");

        assert_eq!(
            op.headers.get("x-tos-storage-class").map(String::as_str),
            Some("STANDARD")
        );
        assert!(op.body.is_none());
    }

    #[test]
    fn test_trash_short_config_expands_to_put_bucket_trash_schema() {
        let body = trash_body(&TrashSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(r#"{"Status":"Disabled"}"#.to_string()),
            status: None,
            days: None,
            content_md5: None,
        })
        .expect("trash body");

        let json: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(
            json,
            serde_json::json!({
                "Trash": {
                    "Status": "Disabled",
                    "CleanInterval": 7,
                    "ForbiddenOverWrite": "Disabled",
                    "TrashPath": ".Trash/"
                }
            })
        );
    }

    #[test]
    fn test_trash_lowercase_config_normalizes_to_service_field_names() {
        let body = trash_body(&TrashSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(
                r#"{"trash":{"status":"Enabled","clean_interval":30,"forbidden_over_write":"Enabled","trash_path":".Recycle/","prefix_match_rules":[{"Prefix":"logs/"}]}}"#
                    .to_string(),
            ),
            status: None,
            days: None,
            content_md5: None,
        })
        .expect("trash body");

        let json: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json["Trash"]["Status"], "Enabled");
        assert_eq!(json["Trash"]["CleanInterval"], 30);
        assert_eq!(json["Trash"]["ForbiddenOverWrite"], "Enabled");
        assert_eq!(json["Trash"]["TrashPath"], ".Recycle/");
        assert_eq!(json["Trash"]["PrefixMatchRules"][0]["Prefix"], "logs/");
    }

    #[test]
    fn test_trash_field_flags_map_days_to_clean_interval() {
        let body = trash_body(&TrashSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            status: Some("enabled".to_string()),
            days: Some(15),
            content_md5: None,
        })
        .expect("trash body");

        let json: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(json["Trash"]["Status"], "Enabled");
        assert_eq!(json["Trash"]["CleanInterval"], 15);
        assert!(json["Trash"].get("Days").is_none());
    }

    #[test]
    fn test_redundancy_transition_config_moves_target_redundancy_to_query() {
        let op = redundancy_transition_operation(&RedundancyTransitionAction::Create(
            RedundancyTransitionCreateArgs {
                bucket: BucketTarget::from_name("demo-bucket"),
                config: Some(r#"{"TargetRedundancy":"ZRS","Prefix":"logs/"}"#.to_string()),
                target_redundancy: None,
                prefix: None,
                storage_class: None,
                content_md5: None,
            },
        ))
        .expect("redundancy transition op");

        let body: Value =
            serde_json::from_slice(op.body.as_ref().expect("body")).expect("json body");
        assert_eq!(
            op.query
                .get("x-tos-target-redundancy-type")
                .map(String::as_str),
            Some("ZRS")
        );
        assert_eq!(body["Prefix"], "logs/");
        assert!(body.get("TargetRedundancy").is_none());
        assert!(op.headers.contains_key("Content-MD5"));
        assert!(!op.headers.contains_key("x-tos-target-redundancy-type"));
    }

    #[test]
    fn test_encryption_body_matches_documented_schema_and_md5() {
        let op = encryption_operation(&EncryptionAction::Set(EncryptionSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(r#"{"Rule":{"ApplyServerSideEncryptionByDefault":{"SSEAlgorithm":"KMS","KMSDataEncryption":"SM4","KMSMasterKeyID":"kms-key-1"}}}"#.to_string()),
            sse_algorithm: "KMS".to_string(),
            kms_data_encryption: Some("SM4".to_string()),
            kms_master_key_id: Some("kms-key-1".to_string()),
            content_md5: None,
        }))
        .expect("encryption op");

        let body: Value =
            serde_json::from_slice(op.body.as_ref().expect("body")).expect("json body");
        assert_eq!(
            body["Rule"]["ApplyServerSideEncryptionByDefault"]["SSEAlgorithm"],
            "KMS"
        );
        assert_eq!(
            body["Rule"]["ApplyServerSideEncryptionByDefault"]["KMSDataEncryption"],
            "SM4"
        );
        assert_eq!(
            body["Rule"]["ApplyServerSideEncryptionByDefault"]["KMSMasterKeyID"],
            "kms-key-1"
        );
        assert!(op.headers.contains_key("Content-MD5"));
    }

    #[test]
    fn test_lifecycle_structured_flags_build_rules_payload() {
        let body = lifecycle_body(&LifecycleSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            rules: None,
            id: Some("rule-1".to_string()),
            prefix: Some("logs/".to_string()),
            status: Some("Enabled".to_string()),
            tags: Some("env=prod".to_string()),
            filter: None,
            expiration: Some("{\"Days\":30}".to_string()),
            noncurrent_version_expiration: None,
            abort_incomplete_multipart_upload: None,
            transitions: Some("[{\"Days\":7,\"StorageClass\":\"IA\"}]".to_string()),
            noncurrent_version_transitions: None,
            access_time_transitions: None,
            noncurrent_version_access_time_transitions: None,
        })
        .expect("lifecycle body");

        let value: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["Rules"][0]["ID"], "rule-1");
        assert_eq!(value["Rules"][0]["Status"], "Enabled");
        assert_eq!(value["Rules"][0]["Tags"][0]["Key"], "env");
        assert_eq!(value["Rules"][0]["Expiration"]["Days"], 30);
    }

    #[test]
    fn test_cors_structured_flags_build_cors_rules_payload() {
        let body = cors_body(&CorsSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            rules: None,
            allowed_origins: Some("https://a.example.com,https://b.example.com".to_string()),
            allowed_methods: Some("GET,PUT".to_string()),
            allowed_headers: Some("Authorization".to_string()),
            expose_headers: Some("ETag".to_string()),
            max_age_seconds: Some(300),
            response_vary: Some(true),
            content_md5: None,
        })
        .expect("cors body");

        let value: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(
            value["CORSRules"][0]["AllowedOrigins"][0],
            "https://a.example.com"
        );
        assert_eq!(value["CORSRules"][0]["AllowedMethods"][1], "PUT");
        assert_eq!(value["CORSRules"][0]["MaxAgeSeconds"], 300);
        assert_eq!(value["CORSRules"][0]["ResponseVary"], true);
    }

    #[test]
    fn test_replication_structured_flags_build_replication_payload() {
        let body = replication_body(&ReplicationSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            rules: None,
            role: Some("trn:iam::123:role/replication".to_string()),
            id: Some("rule-1".to_string()),
            status: Some("Enabled".to_string()),
            prefix_set: Some("logs/,archive/".to_string()),
            tags: Some("env=prod".to_string()),
            destination_bucket: Some("trn:tos:::target-bucket".to_string()),
            destination_location: Some("cn-beijing".to_string()),
            destination_storage_class: Some("IA".to_string()),
            storage_class_inherit_directive: Some("DESTINATION_BUCKET".to_string()),
            historical_object_replication: Some("Enabled".to_string()),
            transfer_type: Some("Async".to_string()),
            access_control_translation_owner: Some("Destination".to_string()),
        })
        .expect("replication body");

        let value: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["Role"], "trn:iam::123:role/replication");
        assert_eq!(
            value["Rules"][0]["Destination"]["Bucket"],
            "trn:tos:::target-bucket"
        );
        assert_eq!(value["Rules"][0]["PrefixSet"][1], "archive/");
        assert_eq!(
            value["Rules"][0]["Destination"]["AccessControlTranslation"]["Owner"],
            "Destination"
        );
    }

    #[test]
    fn test_tagging_body_matches_documented_schema_and_md5() {
        let op = tagging_operation(&TaggingAction::Set(TaggingSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(r#"{"TagSet":{"Tags":[{"Key":"env","Value":"prod"},{"Key":"team","Value":"storage"}]}}"#.to_string()),
            tags: "env=prod&team=storage".to_string(),
            content_md5: None,
        }))
        .expect("tagging op");

        let body: Value =
            serde_json::from_slice(op.body.as_ref().expect("body")).expect("json body");
        assert_eq!(body["TagSet"]["Tags"][0]["Key"], "env");
        assert_eq!(body["TagSet"]["Tags"][1]["Value"], "storage");
        assert!(op.headers.contains_key("Content-MD5"));
    }

    #[test]
    fn test_acl_header_mode_uses_documented_headers() {
        let op = acl_operation(&AclAction::Set(AclSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            acl: Some("public-read".to_string()),
            grant_full_control: None,
            grant_read: None,
            grant_read_non_list: None,
            grant_read_acp: None,
            grant_write: None,
            grant_write_acp: None,
            owner_id: None,
            bucket_acl_delivered: None,
            grants: None,
            grantee_type: None,
            grantee_id: None,
            grantee_canned: None,
            permission: None,
        }))
        .expect("acl op");

        assert_eq!(
            op.headers.get("x-tos-acl").map(String::as_str),
            Some("public-read")
        );
        assert!(op.body.is_none());
    }

    #[test]
    fn test_acl_body_mode_builds_documented_schema() {
        let body = acl_body(&AclSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            acl: None,
            grant_full_control: None,
            grant_read: None,
            grant_read_non_list: None,
            grant_read_acp: None,
            grant_write: None,
            grant_write_acp: None,
            owner_id: Some("owner-1".to_string()),
            bucket_acl_delivered: Some(true),
            grants: None,
            grantee_type: Some("CanonicalUser".to_string()),
            grantee_id: Some("user-1".to_string()),
            grantee_canned: None,
            permission: Some("FULL_CONTROL".to_string()),
        })
        .expect("acl body");

        let value: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["Owner"]["ID"], "owner-1");
        assert_eq!(value["BucketAclDelivered"], true);
        assert_eq!(value["Grants"][0]["Grantee"]["Type"], "CanonicalUser");
        assert_eq!(value["Grants"][0]["Permission"], "FULL_CONTROL");
    }

    #[test]
    fn test_access_monitor_body_and_md5_follow_documented_shape() {
        let op = access_monitor_operation(&AccessMonitorAction::Set(AccessMonitorSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(r#"{"Status":"Enabled"}"#.to_string()),
            status: "Enabled".to_string(),
            content_md5: None,
        }))
        .expect("access monitor op");

        let body: Value =
            serde_json::from_slice(op.body.as_ref().expect("body")).expect("json body");
        assert_eq!(body["Status"], "Enabled");
        assert!(op.headers.contains_key("Content-MD5"));
    }

    #[test]
    fn test_https_config_requires_versions_when_enabled() {
        let err = https_config_operation(&HttpsConfigAction::Set(HttpsConfigSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            enable: Some(true),
            min_tls_version: None,
            max_tls_version: Some("TLSv1.3".to_string()),
        }))
        .expect_err("https config should fail without min version");

        assert!(err.to_string().contains("requires --config JSON body"));
    }

    #[test]
    fn test_custom_domain_body_matches_documented_schema_and_md5() {
        let op = custom_domain_operation(&CustomDomainAction::Set(CustomDomainSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(r#"{"Rule":{"Domain":"static.example.com","CertId":"cert-1","CertStatus":"CertBound","Forbidden":false,"ForbiddenReason":"none","Cname":"cname.example.com","Protocol":"https"}}"#.to_string()),
            domain: "static.example.com".to_string(),
            certificate_id: Some("cert-1".to_string()),
            certificate_status: Some("CertBound".to_string()),
            forbidden: Some(false),
            forbidden_reason: Some("none".to_string()),
            cname: Some("cname.example.com".to_string()),
            protocol: Some("https".to_string()),
            content_md5: None,
        }))
        .expect("custom domain op");

        let body: Value =
            serde_json::from_slice(op.body.as_ref().expect("body")).expect("json body");
        assert_eq!(body["Rule"]["Domain"], "static.example.com");
        assert_eq!(body["Rule"]["CertId"], "cert-1");
        assert_eq!(body["Rule"]["Protocol"], "https");
        assert!(op.headers.contains_key("Content-MD5"));
    }

    #[test]
    fn test_notification_body_matches_v2_documented_schema() {
        let op = notification_operation(&NotificationAction::Set(NotificationSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(r#"{"Version":"1755056218115649299","Rules":[{"RuleId":"event","Destination":{"VeFaaS":[{"FunctionId":"l2u0demo"}],"Kafka":[{"Role":"trn:iam::123:role/kafka","InstanceId":"kafka-instance-1","Topic":"topic-a","User":"user-a","Region":"cn-beijing"}],"RocketMQ":[{"Role":"trn:iam::123:role/notification","InstanceId":"rmq-instance-1","Topic":"topic-a","AccessKeyId":"ak-1"}]}}]}"#.to_string()),
            rules: None,
            version: Some("1755056218115649299".to_string()),
            rule_id: Some("event".to_string()),
            events: Some("tos:ObjectCreated:Put".to_string()),
            filter_rules: Some("prefix=images/".to_string()),
            destination_vefaas: None,
            vefaas_function_ids: Some("l2u0demo".to_string()),
            destination_kafka: None,
            kafka_role: Some("trn:iam::123:role/kafka".to_string()),
            kafka_instance_id: Some("kafka-instance-1".to_string()),
            kafka_topic: Some("topic-a".to_string()),
            kafka_user: Some("user-a".to_string()),
            kafka_region: Some("cn-beijing".to_string()),
            destination_rocketmq: None,
            rocketmq_role: Some("trn:iam::123:role/notification".to_string()),
            rocketmq_instance_id: Some("rmq-instance-1".to_string()),
            rocketmq_topic: Some("topic-a".to_string()),
            rocketmq_access_key_id: Some("ak-1".to_string()),
            content_md5: None,
        }))
        .expect("notification op");

        let body: Value =
            serde_json::from_slice(op.body.as_ref().expect("body")).expect("json body");
        assert_eq!(body["Rules"][0]["RuleId"], "event");
        assert_eq!(
            body["Rules"][0]["Destination"]["VeFaaS"][0]["FunctionId"],
            "l2u0demo"
        );
        assert_eq!(
            body["Rules"][0]["Destination"]["Kafka"][0]["InstanceId"],
            "kafka-instance-1"
        );
        assert_eq!(
            body["Rules"][0]["Destination"]["RocketMQ"][0]["AccessKeyId"],
            "ak-1"
        );
        assert_eq!(body["Version"], "1755056218115649299");
        assert!(op.headers.contains_key("Content-MD5"));
    }

    #[test]
    fn test_logging_operation_accepts_json_config_and_md5() {
        let op = logging_operation(&LoggingAction::Set(LoggingSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(r#"{"BucketLoggingStatus":{"LoggingEnabled":{"TargetBucket":"access-log-bucket","TargetPrefix":"logs/"}}}"#.to_string()),
            target_bucket: Some("access-log-bucket".to_string()),
            target_prefix: Some("logs/".to_string()),
            content_md5: None,
        }))
        .expect("logging op");

        let body: Value =
            serde_json::from_slice(op.body.as_ref().expect("body")).expect("json body");
        assert_eq!(
            body["BucketLoggingStatus"]["LoggingEnabled"]["TargetBucket"],
            "access-log-bucket"
        );
        assert_eq!(
            body["BucketLoggingStatus"]["LoggingEnabled"]["TargetPrefix"],
            "logs/"
        );
        assert!(op.headers.contains_key("Content-MD5"));
    }

    #[test]
    fn test_logging_body_supports_disable_mode() {
        let body = logging_body(&LoggingSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            target_bucket: None,
            target_prefix: None,
            content_md5: None,
        })
        .expect("logging disable body");

        let json: Value = serde_json::from_slice(&body).expect("json logging body");
        assert_eq!(json, json!({"BucketLoggingStatus": {}}));
    }

    #[test]
    fn test_logging_body_rejects_partial_configuration() {
        let err = logging_body(&LoggingSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            target_bucket: Some("access-log-bucket".to_string()),
            target_prefix: None,
            content_md5: None,
        })
        .expect_err("logging body should reject partial config");

        assert!(err
            .to_string()
            .contains("--target-bucket and --target-prefix together"));
    }

    #[test]
    fn test_website_structured_flags_build_documented_schema() {
        let body = website_body(&WebsiteSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            redirect_all_requests_to_host_name: None,
            redirect_all_requests_to_protocol: None,
            index_document_suffix: Some("index.html".to_string()),
            index_document_forbidden_sub_dir: Some(false),
            error_document_key: Some("error.html".to_string()),
            routing_rules: None,
            routing_rule_key_prefix_equals: Some("docs/".to_string()),
            routing_rule_http_error_code_returned_equals: Some(404),
            routing_rule_protocol: Some("https".to_string()),
            routing_rule_host_name: Some("www.example.com".to_string()),
            routing_rule_replace_key_prefix_with: Some("public/".to_string()),
            routing_rule_replace_key_with: None,
            routing_rule_http_redirect_code: Some(302),
            content_md5: None,
        })
        .expect("website body");

        let value: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["IndexDocument"]["Suffix"], "index.html");
        assert_eq!(
            value["RoutingRules"][0]["Condition"]["KeyPrefixEquals"],
            "docs/"
        );
        assert_eq!(
            value["RoutingRules"][0]["Redirect"]["ReplaceKeyPrefixWith"],
            "public/"
        );
    }

    #[test]
    fn test_mirror_body_matches_documented_schema() {
        let op = mirror_operation(&MirrorAction::Set(MirrorSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(r#"{"Rules":[{"ID":"mirror-rule-1","Redirect":{"PublicSource":{"SourceEndpoint":{"Primary":["https://origin.example.com"]}},"PrivateSource":{"SourceEndpoint":{"Primary":[{"CredentialProvider":{"StaticCredential":{"StorageVendor":"TOS"}}}]}}}}]}"#.to_string()),
            rules: None,
            id: Some("mirror-rule-1".to_string()),
            condition_http_code: Some(404),
            condition_key_prefix: Some("docs/".to_string()),
            condition_key_suffix: Some(".html".to_string()),
            condition_allow_hosts: Some("www.example.com".to_string()),
            condition_http_methods: Some("GET,HEAD".to_string()),
            redirect_type: Some("Mirror".to_string()),
            fetch_source_on_redirect: Some(true),
            pass_query: Some(true),
            follow_redirect: Some(false),
            mirror_header_pass_all: Some(false),
            mirror_header_pass: Some("Authorization".to_string()),
            mirror_header_remove: Some("Cookie".to_string()),
            mirror_header_set: Some("X-Env=prod".to_string()),
            public_source_primary_endpoints: Some("https://origin.example.com".to_string()),
            public_source_follower_endpoints: Some("https://origin-backup.example.com".to_string()),
            public_source_fixed_endpoint: Some(true),
            transform_with_key_prefix: Some("prefix/".to_string()),
            transform_with_key_suffix: Some(".bak".to_string()),
            transform_replace_key_prefix: Some("docs/".to_string()),
            transform_replace_key_prefix_with: Some("public/".to_string()),
            fetch_header_to_metadata_rules: Some("ETag=etag".to_string()),
            private_source_primary_endpoints: Some(
                "https://private-origin.example.com".to_string(),
            ),
            private_source_follower_endpoints: Some(
                "https://private-backup.example.com".to_string(),
            ),
            private_source_bucket_name: Some("private-bucket".to_string()),
            private_source_role: Some("trn:iam::123:role/mirror".to_string()),
            private_source_region: Some("cn-beijing".to_string()),
            private_source_storage_vendor: Some("TOS".to_string()),
            private_source_ak: Some("ak-test".to_string()),
            private_source_sk: Some("sk-test".to_string()),
            private_source_sk_encrypt_type: Some("plain".to_string()),
            fetch_source_on_redirect_with_query: Some(true),
            pass_status_code_from_source: Some("301,302".to_string()),
            pass_header_from_source: Some("Content-Type,Cache-Control".to_string()),
            content_md5: None,
        }))
        .expect("mirror op");

        let body: Value =
            serde_json::from_slice(op.body.as_ref().expect("body")).expect("json body");
        assert_eq!(body["Rules"][0]["ID"], "mirror-rule-1");
        assert_eq!(
            body["Rules"][0]["Redirect"]["PublicSource"]["SourceEndpoint"]["Primary"][0],
            "https://origin.example.com"
        );
        assert_eq!(
            body["Rules"][0]["Redirect"]["PrivateSource"]["SourceEndpoint"]["Primary"][0]
                ["CredentialProvider"]["StaticCredential"]["StorageVendor"],
            "TOS"
        );
        assert!(op.headers.contains_key("Content-MD5"));
    }

    #[test]
    fn test_inventory_body_matches_documented_schema() {
        let body = inventory_body(&InventorySetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            id: "daily-report".to_string(),
            is_enabled: Some(true),
            filter_prefix: Some("logs/".to_string()),
            destination_format: Some("CSV".to_string()),
            destination_account_id: Some("1234567890".to_string()),
            destination_role: Some("trn:iam::123:role/inventory".to_string()),
            destination_bucket: Some("target-bucket".to_string()),
            destination_prefix: Some("inventory/".to_string()),
            schedule_frequency: Some("Daily".to_string()),
            included_object_versions: Some("All".to_string()),
            optional_fields: Some("Size,StorageClass".to_string()),
            is_uncompressed: Some(false),
            content_md5: None,
        })
        .expect("inventory body");

        let value: Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["Id"], "daily-report");
        assert_eq!(
            value["Destination"]["TOSBucketDestination"]["Bucket"],
            "target-bucket"
        );
        assert_eq!(value["OptionalFields"]["Field"][1], "StorageClass");
        assert_eq!(value["IsUnCompressed"], false);
    }

    #[test]
    fn test_worm_body_matches_object_lock_schema() {
        let op = worm_operation(&WormAction::Set(WormSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(r#"{"ObjectLockEnabled":"Enabled","Rule":{"DefaultRetention":{"Mode":"COMPLIANCE","Days":30}}}"#.to_string()),
            object_lock_enabled: Some("Enabled".to_string()),
            default_retention_mode: Some("COMPLIANCE".to_string()),
            default_retention_days: Some(30),
            default_retention_years: None,
            content_md5: None,
        }))
        .expect("worm op");

        let body: Value =
            serde_json::from_slice(op.body.as_ref().expect("body")).expect("json body");
        assert_eq!(body["ObjectLockEnabled"], "Enabled");
        assert_eq!(body["Rule"]["DefaultRetention"]["Mode"], "COMPLIANCE");
        assert_eq!(body["Rule"]["DefaultRetention"]["Days"], 30);
        assert!(op.headers.contains_key("Content-MD5"));
    }

    #[test]
    fn test_cdn_notification_config_matches_service_schema() {
        let body = cdn_notification_body(&CdnNotificationSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(
                r#"{"Role":"role","Rules":[{"RuleId":"rule-1","CustomDomain":"example.com","Events":["tos:ObjectCreated:*"],"Filter":{"TOSKey":{"FilterRules":[{"Name":"prefix","Value":"cdn/"}]}}}]}"#
                    .to_string(),
            ),
            events: None,
            filter_rules: None,
            role: None,
            endpoint: None,
            content_md5: None,
        })
        .expect("cdn notification body");
        let value: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(value["Role"], "role");
        assert_eq!(value["Rules"][0]["RuleId"], "rule-1");
        assert_eq!(value["Rules"][0]["CustomDomain"], "example.com");
        assert_eq!(value["Rules"][0]["Events"][0], "tos:ObjectCreated:*");
        assert_eq!(
            value["Rules"][0]["Filter"]["TOSKey"]["FilterRules"][0]["Name"],
            "prefix"
        );
    }

    #[test]
    fn test_cdn_notification_flat_config_is_wrapped_into_rules() {
        let body = cdn_notification_body(&CdnNotificationSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: Some(
                r#"{"Role":"role","Endpoint":"example.com","Events":["tos:ObjectCreated:Put"],"Filter":{"TOSKey":{"FilterRules":[{"Name":"prefix","Value":"cdn/"}]}}}"#
                    .to_string(),
            ),
            events: None,
            filter_rules: None,
            role: None,
            endpoint: None,
            content_md5: None,
        })
        .expect("cdn notification body");
        let value: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(value["Role"], "role");
        assert_eq!(value["Rules"][0]["RuleId"], "default");
        assert_eq!(value["Rules"][0]["CustomDomain"], "example.com");
        assert_eq!(value["Rules"][0]["Events"][0], "tos:ObjectCreated:Put");
    }

    #[test]
    fn test_real_time_log_body_requires_tls_ids_when_not_using_service_topic() {
        let err = real_time_log_body(&RealTimeLogSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            role: "trn:iam::123:role/logging".to_string(),
            use_service_topic: Some(false),
            tls_project_id: None,
            tls_topic_id: Some("topic-1".to_string()),
            content_md5: None,
        })
        .expect_err("real-time-log body should fail without project id");

        assert!(err
            .to_string()
            .contains("--tls-project-id and --tls-topic-id"));
    }

    #[test]
    fn test_worm_body_rejects_days_and_years_together() {
        let err = worm_body(&WormSetArgs {
            bucket: BucketTarget::from_name("demo-bucket"),
            config: None,
            object_lock_enabled: Some("Enabled".to_string()),
            default_retention_mode: Some("GOVERNANCE".to_string()),
            default_retention_days: Some(30),
            default_retention_years: Some(1),
            content_md5: None,
        })
        .expect_err("worm body should reject days and years together");

        assert!(err
            .to_string()
            .contains("--default-retention-days together with --default-retention-years"));
    }
}
