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
use std::collections::HashMap;

use crate::cli::low_level::*;
use crate::domain::core;
use crate::handler::common::{
    build_profile, ensure_force_for_destructive, output_result, read_body_input,
};
use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};
use tos_core::agent::describe::{
    CommandDescription, CommandLayer, CommandParameter, ParameterLocation, RiskLevel,
};
use tos_core::agent::dryrun::{DryRunResult, Impact};
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::client::TosClient;

#[derive(Clone, Copy, Debug, Serialize)]
enum EndpointKind {
    DataPlane,
    ControlPlane,
}

#[derive(Clone, Copy, Debug)]
enum Source {
    Name,
    Bucket,
    Id,
    StyleName,
    JobId,
    Alias,
    Accelerator,
    AcceleratorId,
    BucketName,
    Domain,
    Az,
    Region,
    ResourceTrn,
    TagKeys,
    Tag,
    ObjectSetName,
    Object,
}

#[derive(Clone, Copy, Debug)]
enum QuerySpec {
    Flag(&'static str),
    Param(&'static str, Source),
    Key(Source),
}

#[derive(Clone, Debug)]
struct AdvancedSpec {
    command: &'static str,
    api: &'static str,
    description: &'static str,
    endpoint: EndpointKind,
    method: Method,
    path: &'static str,
    query: &'static [QuerySpec],
    has_body: bool,
}

#[derive(Debug)]
struct AdvancedOperation {
    spec: AdvancedSpec,
    path: String,
    bucket: Option<String>,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
    parameters: Vec<CommandParameter>,
    destructive: bool,
    force: bool,
    target: String,
}

pub async fn handle_data_process_command(
    global: &GlobalArgs,
    action: &Option<DataProcessAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        "ve-tos data-process",
        "Data processing APIs",
        data_process_actions(),
        action,
        data_process_operation,
    )
    .await
}

pub async fn handle_object_set_command(
    global: &GlobalArgs,
    action: &Option<ObjectSetAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        "ve-tos object-set",
        "Object set APIs",
        object_set_actions(),
        action,
        object_set_operation,
    )
    .await
}

pub async fn handle_accelerator_command(
    global: &GlobalArgs,
    action: &Option<AcceleratorAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        "ve-tos accelerator",
        "Accelerator control-plane APIs",
        accelerator_actions(),
        action,
        accelerator_operation,
    )
    .await
}

pub async fn handle_mrap_command(
    global: &GlobalArgs,
    action: &Option<MrapAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        "ve-tos mrap",
        "Multi-region access point APIs",
        mrap_actions(),
        action,
        mrap_operation,
    )
    .await
}

pub async fn handle_ap_command(
    global: &GlobalArgs,
    action: &Option<ApAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        "ve-tos ap",
        "Access point APIs",
        ap_actions(),
        action,
        ap_operation,
    )
    .await
}

pub async fn handle_cap_command(
    global: &GlobalArgs,
    action: &Option<CapAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        "ve-tos cap",
        "Converged access point APIs",
        cap_actions(),
        action,
        cap_operation,
    )
    .await
}

pub async fn handle_dataset_command(
    global: &GlobalArgs,
    action: &Option<DatasetAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        "ve-tos dataset",
        "Intelligent retrieval dataset APIs",
        dataset_actions(),
        action,
        dataset_operation,
    )
    .await
}

pub async fn handle_control_command(
    global: &GlobalArgs,
    action: &Option<ControlAction>,
) -> Result<i32, CliError> {
    handle_group(
        global,
        "ve-tos control",
        "Control-plane APIs",
        control_actions(),
        action,
        control_operation,
    )
    .await
}

async fn handle_group<A, F>(
    global: &GlobalArgs,
    group: &'static str,
    description: &'static str,
    actions: &'static [(&'static str, &'static str)],
    action: &Option<A>,
    op_builder: F,
) -> Result<i32, CliError>
where
    F: Fn(&A, Option<&str>) -> Result<AdvancedOperation, CliError>,
{
    if global.describe {
        if let Some(action) = action {
            match op_builder(action, None) {
                Ok(op) => {
                    output_result(
                        global,
                        &Envelope::success(op.spec.command, describe_action(&op)),
                    )?;
                }
                Err(err) => {
                    let command = current_advanced_command_path(group);
                    if let Some(desc) = crate::registry::describe_command_metadata(&command) {
                        output_result(global, &Envelope::success(command, desc))?;
                    } else {
                        return Err(err);
                    }
                }
            }
        } else {
            output_result(global, &describe_group(group, description, actions))?;
        }
        return Ok(0);
    }

    let Some(action) = action else {
        return Err(CliError::ValidationError(format!(
            "`{group}` requires a subcommand; use `{group} --help` or `{group} --describe`"
        )));
    };
    let profile = build_profile(global)?;
    // [Review Fix #4] Advanced control-plane APIs often model region as an
    // explicit query parameter. The CLI flag wins, but a configured/env region
    // must still satisfy that request field when --region is omitted.
    let op = op_builder(action, profile.region.as_deref())?;
    if op.spec.has_body && op.body.is_none() {
        return Err(CliError::ValidationError(format!(
            "{} requires --config for JSON body",
            op.spec.command
        )));
    }
    if global.dry_run {
        output_result(global, &dry_run(&op))?;
        return Ok(0);
    }
    // [Review Fix #3] Dry-run is side-effect free; force is required only before real destructive execution.
    if op.destructive {
        ensure_force_for_destructive(global, op.force, op.spec.command, &op.target)?;
    }

    let client = TosClient::new(&profile, "tos")?;
    let result = match op.spec.endpoint {
        EndpointKind::DataPlane => {
            let bucket = op.bucket.as_deref().ok_or_else(|| {
                CliError::ValidationError(format!("{} requires --bucket", op.spec.command))
            })?;
            if op.spec.path == "/{bucket}" {
                core::execute_bucket_request(
                    &client,
                    op.spec.command,
                    op.spec.method.clone(),
                    bucket,
                    op.query,
                    op.headers,
                    op.body,
                )
                .await?
            } else {
                let endpoint = client.bucket_endpoint(bucket)?;
                core::execute_endpoint_request(
                    &client,
                    op.spec.command,
                    op.spec.method.clone(),
                    &endpoint,
                    &op.path,
                    op.query,
                    op.headers,
                    op.body,
                )
                .await?
            }
        }
        EndpointKind::ControlPlane => {
            let endpoint = client.control_endpoint()?;
            // [Review Fix #CP1] Control plane 请求必须携带 X-Tos-Account-Id 参与 V4 签名
            let mut headers = op.headers;
            if let Some(account_id) = client.account_id() {
                headers.insert("x-tos-account-id".to_string(), account_id.to_string());
            }
            core::execute_endpoint_request(
                &client,
                op.spec.command,
                op.spec.method.clone(),
                &endpoint,
                &op.path,
                op.query,
                headers,
                op.body,
            )
            .await?
        }
    };
    output_result(global, &result)?;
    Ok(0)
}

fn current_advanced_command_path(group: &str) -> String {
    let args = std::env::args().collect::<Vec<_>>();
    let binary_stem = args
        .first()
        .and_then(|arg| std::path::Path::new(arg).file_stem())
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let start = if matches!(binary_stem, "ve-tos" | "ve-tos-cli") {
        1
    } else {
        // [Review Fix #27] Recover leaf describe metadata from the canonical
        // ve-tos public entrypoint only.
        let Some(tos_idx) = args.iter().position(|arg| arg.as_str() == "ve-tos") else {
            return group.to_string();
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
    group.to_string()
}

fn data_process_operation(
    action: &DataProcessAction,
    region_fallback: Option<&str>,
) -> Result<AdvancedOperation, CliError> {
    let (spec, args) = match action {
        DataProcessAction::DeleteImageStyle(a) => (
            dp(
                "ve-tos data-process delete-image-style",
                "DeleteBucketImageStyle",
                Method::DELETE,
                "/{bucket}",
                &[
                    QuerySpec::Flag("imageStyle"),
                    QuerySpec::Param("styleName", Source::StyleName),
                ],
                false,
            ),
            a,
        ),
        DataProcessAction::GetImageStyle(a) => (
            dp(
                "ve-tos data-process get-image-style",
                "GetBucketImageStyle",
                Method::GET,
                "/{bucket}",
                &[
                    QuerySpec::Flag("imageStyle"),
                    QuerySpec::Param("styleName", Source::StyleName),
                ],
                false,
            ),
            a,
        ),
        DataProcessAction::ListImageStyles(a) => (
            dp(
                "ve-tos data-process list-image-styles",
                "ListBucketImageStyle",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("imageStyle")],
                false,
            ),
            a,
        ),
        DataProcessAction::ListImageStyleBriefInfos(a) => (
            dp(
                "ve-tos data-process list-image-style-brief-infos",
                "ListBucketImageStyleBriefInfo",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("imageStyleBriefInfo")],
                false,
            ),
            a,
        ),
        DataProcessAction::ListImageStyleContents(a) => (
            dp(
                "ve-tos data-process list-image-style-contents",
                "ListBucketImageStyleContent",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("imageStyleContent")],
                false,
            ),
            a,
        ),
        DataProcessAction::SetImageStyle(a) => (
            dp(
                "ve-tos data-process set-image-style",
                "PutBucketImageStyle",
                Method::PUT,
                "/{bucket}",
                &[
                    QuerySpec::Flag("imageStyle"),
                    QuerySpec::Param("styleName", Source::StyleName),
                ],
                true,
            ),
            a,
        ),
        DataProcessAction::SetImageProtectRule(a) => (
            dp(
                "ve-tos data-process set-image-protect-rule",
                "PutOriginalImageProtectRule",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("originalImageProtect")],
                true,
            ),
            a,
        ),
        DataProcessAction::GetImageProtectRule(a) => (
            dp(
                "ve-tos data-process get-image-protect-rule",
                "GetOriginalImageProtectRule",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("originalImageProtect")],
                false,
            ),
            a,
        ),
        DataProcessAction::SetImageStyleSeparator(a) => (
            dp(
                "ve-tos data-process set-image-style-separator",
                "PutImageStyleSeparator",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("imageStyleSeparator")],
                true,
            ),
            a,
        ),
        DataProcessAction::GetImageStyleSeparator(a) => (
            dp(
                "ve-tos data-process get-image-style-separator",
                "GetImageStyleSeparator",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("imageStyleSeparator")],
                false,
            ),
            a,
        ),
        DataProcessAction::SetPrivateM3u8Rule(a) => (
            dp(
                "ve-tos data-process set-private-m3u8-rule",
                "PutPrivateM3U8Rule",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("privateM3U8")],
                true,
            ),
            a,
        ),
        DataProcessAction::GetPrivateM3u8Rule(a) => (
            dp(
                "ve-tos data-process get-private-m3u8-rule",
                "GetPrivateM3U8Rule",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("privateM3U8")],
                false,
            ),
            a,
        ),
        DataProcessAction::SetBlindWatermarkRule(a) => (
            dp(
                "ve-tos data-process set-blind-watermark-rule",
                "PutBlindWatermarkRule",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("blindWatermark")],
                true,
            ),
            a,
        ),
        DataProcessAction::GetBlindWatermarkRule(a) => (
            dp(
                "ve-tos data-process get-blind-watermark-rule",
                "GetBlindWatermarkRule",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("blindWatermark")],
                false,
            ),
            a,
        ),
        DataProcessAction::DeleteWorkflow(a) => (
            dp(
                "ve-tos data-process delete-workflow",
                "DeleteBucketWorkflow",
                Method::DELETE,
                "/{bucket}",
                &[QuerySpec::Flag("workflow")],
                false,
            ),
            a,
        ),
        DataProcessAction::GetWorkflow(a) => (
            dp(
                "ve-tos data-process get-workflow",
                "GetBucketWorkflow",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("workflow")],
                false,
            ),
            a,
        ),
        DataProcessAction::SetWorkflow(a) => (
            dp(
                "ve-tos data-process set-workflow",
                "PutBucketWorkflow",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("workflow")],
                true,
            ),
            a,
        ),
        DataProcessAction::GetWorkflowExecution(a) => (
            dp(
                "ve-tos data-process get-workflow-execution",
                "GetWorkflowExecution",
                Method::GET,
                "/{bucket}",
                &[
                    QuerySpec::Flag("workflow_execution"),
                    QuerySpec::Param("id", Source::Id),
                ],
                false,
            ),
            a,
        ),
        DataProcessAction::ListWorkflowExecutions(a) => (
            dp(
                "ve-tos data-process list-workflow-executions",
                "ListWorkflowExecution",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("workflow_execution")],
                false,
            ),
            a,
        ),
        DataProcessAction::DeleteTemplate(a) => (
            dp(
                "ve-tos data-process delete-template",
                "DeleteDataProcessTemplate",
                Method::DELETE,
                "/{bucket}",
                // [Review Fix #DataProcessTemplateQuery] The required
                // service tag query must not use the global --query JMESPath
                // flag; expose it as an explicit --tag protocol parameter.
                &[
                    QuerySpec::Flag("process_template"),
                    QuerySpec::Param("tag", Source::Tag),
                    QuerySpec::Param("id", Source::Id),
                ],
                false,
            ),
            a,
        ),
        DataProcessAction::GetTemplate(a) => (
            dp(
                "ve-tos data-process get-template",
                "GetDataProcessTemplate",
                Method::GET,
                "/{bucket}",
                // [Review Fix #DataProcessTemplateQuery] See delete-template:
                // tag is an HTTP query parameter, not an output filter.
                &[
                    QuerySpec::Flag("process_template"),
                    QuerySpec::Param("tag", Source::Tag),
                ],
                false,
            ),
            a,
        ),
        DataProcessAction::SetTemplate(a) => (
            dp(
                "ve-tos data-process set-template",
                "PutDataProcessTemplate",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("process_template")],
                true,
            ),
            a,
        ),
        DataProcessAction::CreateAuditJob(a) => (
            dp(
                "ve-tos data-process create-audit-job",
                "CreateAuditJobs",
                Method::POST,
                "/{bucket}",
                &[QuerySpec::Flag("audit_jobs"), QuerySpec::Flag("job_type")],
                true,
            ),
            a,
        ),
        DataProcessAction::CreateDocJob(a) => (
            dp(
                "ve-tos data-process create-doc-job",
                "CreateDocProcessJobs",
                Method::POST,
                "/{bucket}",
                &[QuerySpec::Flag("doc_jobs"), QuerySpec::Flag("job_type")],
                true,
            ),
            a,
        ),
        DataProcessAction::CreateFileJob(a) => (
            dp(
                "ve-tos data-process create-file-job",
                "CreateFileProcessJobs",
                Method::POST,
                "/{bucket}",
                &[QuerySpec::Flag("file_jobs"), QuerySpec::Flag("job_type")],
                true,
            ),
            a,
        ),
        DataProcessAction::CreateMediaJob(a) => (
            dp(
                "ve-tos data-process create-media-job",
                "CreateMediaProcessJobs",
                Method::POST,
                "/{bucket}",
                &[QuerySpec::Flag("media_jobs"), QuerySpec::Flag("job_type")],
                true,
            ),
            a,
        ),
        DataProcessAction::GetJob(a) => (
            dp(
                "ve-tos data-process get-job",
                "GetDataProcessJob",
                Method::GET,
                "/{bucket}",
                &[
                    QuerySpec::Flag("job_type"),
                    QuerySpec::Param("job_id", Source::Id),
                ],
                false,
            ),
            a,
        ),
        DataProcessAction::ListJobs(a) => (
            dp(
                "ve-tos data-process list-jobs",
                "ListDataProcessJobs",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("job_type")],
                false,
            ),
            a,
        ),
        DataProcessAction::DeleteIncrementAudit(a) => (
            dp(
                "ve-tos data-process delete-increment-audit",
                "DeleteBucketIncrementAudit",
                Method::DELETE,
                "/{bucket}",
                &[QuerySpec::Flag("increment_audit")],
                false,
            ),
            a,
        ),
        DataProcessAction::GetAudit(a) => (
            dp(
                "ve-tos data-process get-audit",
                "GetBucketAudit",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("audit")],
                false,
            ),
            a,
        ),
        DataProcessAction::GetIncrementAudit(a) => (
            dp(
                "ve-tos data-process get-increment-audit",
                "GetBucketIncrementAudit",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("increment_audit")],
                false,
            ),
            a,
        ),
        DataProcessAction::ListAudits(a) => (
            dp(
                "ve-tos data-process list-audits",
                "ListBucketAudits",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("audits")],
                false,
            ),
            a,
        ),
        DataProcessAction::ListIncrementAudits(a) => (
            dp(
                "ve-tos data-process list-increment-audits",
                "ListBucketIncrementAudits",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("increment_audits")],
                false,
            ),
            a,
        ),
        DataProcessAction::CreateIncrementAudit(a) => (
            dp(
                "ve-tos data-process create-increment-audit",
                "PostBucketIncrementAudit",
                Method::POST,
                "/{bucket}",
                &[QuerySpec::Flag("increment_audit")],
                true,
            ),
            a,
        ),
        DataProcessAction::CreateAudit(a) => (
            dp(
                "ve-tos data-process create-audit",
                "PutBucketAudit",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("audit")],
                true,
            ),
            a,
        ),
    };
    build_operation(spec, args, region_fallback)
}

fn object_set_operation(
    action: &ObjectSetAction,
    region_fallback: Option<&str>,
) -> Result<AdvancedOperation, CliError> {
    let (spec, args) = match action {
        ObjectSetAction::Delete(a) => (
            dp(
                "ve-tos object-set delete",
                "DeleteObjectSet",
                Method::DELETE,
                "/{bucket}",
                &[QuerySpec::Flag("objectset")],
                false,
            ),
            a,
        ),
        ObjectSetAction::DeleteLifecycle(a) => (
            dp(
                "ve-tos object-set delete-lifecycle",
                "DeleteObjectSetLifecycle",
                Method::DELETE,
                "/{bucket}",
                &[
                    QuerySpec::Flag("objectset-lifecycle"),
                    QuerySpec::Key(Source::ObjectSetName),
                ],
                false,
            ),
            a,
        ),
        ObjectSetAction::DeleteLifecycleByTag(a) => (
            dp(
                "ve-tos object-set delete-lifecycle-by-tag",
                "DeleteObjectSetLifecycleByTag",
                Method::DELETE,
                "/{bucket}",
                &[QuerySpec::Flag("objectset-lifecycle-bytag")],
                false,
            ),
            a,
        ),
        ObjectSetAction::DeleteQuotaByTag(a) => (
            dp(
                "ve-tos object-set delete-quota-by-tag",
                "DeleteObjectSetQuotaByTag",
                Method::DELETE,
                "/{bucket}",
                &[QuerySpec::Flag("objectsetquotabytag")],
                false,
            ),
            a,
        ),
        ObjectSetAction::GetGlobal(a) => (
            dp(
                "ve-tos object-set get-global",
                "GetBucketObjectSetConfiguration",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("objectsetconfiguration")],
                false,
            ),
            a,
        ),
        ObjectSetAction::Get(a) => (
            dp(
                "ve-tos object-set get",
                "GetObjectSet",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("objectset")],
                false,
            ),
            a,
        ),
        ObjectSetAction::GetEndpoint(a) => (
            dp(
                "ve-tos object-set get-endpoint",
                "GetObjectSetEndPoint",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("objectsetendpoint")],
                false,
            ),
            a,
        ),
        ObjectSetAction::GetLifecycle(a) => (
            dp(
                "ve-tos object-set get-lifecycle",
                "GetObjectSetLifecycle",
                Method::GET,
                "/{bucket}",
                &[
                    QuerySpec::Flag("objectset-lifecycle"),
                    QuerySpec::Key(Source::ObjectSetName),
                ],
                false,
            ),
            a,
        ),
        ObjectSetAction::GetLifecycleByTag(a) => (
            dp(
                "ve-tos object-set get-lifecycle-by-tag",
                "GetObjectSetLifecycleByTag",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("objectset-lifecycle-bytag")],
                false,
            ),
            a,
        ),
        ObjectSetAction::GetQuota(a) => (
            dp(
                "ve-tos object-set get-quota",
                "GetObjectSetQuota",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("objectsetquota")],
                false,
            ),
            a,
        ),
        ObjectSetAction::GetQuotaByTag(a) => (
            dp(
                "ve-tos object-set get-quota-by-tag",
                "GetObjectSetQuotaByTag",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("objectsetquotabytag")],
                false,
            ),
            a,
        ),
        ObjectSetAction::GetStorage(a) => (
            dp(
                "ve-tos object-set get-storage",
                "GetObjectSetStorage",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("objectsetstorage")],
                false,
            ),
            a,
        ),
        ObjectSetAction::GetTagging(a) => (
            dp(
                "ve-tos object-set get-tagging",
                "GetObjectSetTagging",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("objectsettagging")],
                false,
            ),
            a,
        ),
        ObjectSetAction::List(a) => (
            dp(
                "ve-tos object-set list",
                "ListObjectSet",
                Method::GET,
                "/{bucket}",
                &[QuerySpec::Flag("objectsets")],
                false,
            ),
            a,
        ),
        ObjectSetAction::SetGlobal(a) => (
            dp(
                "ve-tos object-set set-global",
                "PutBucketObjectSetConfiguration",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("objectsetconfiguration")],
                true,
            ),
            a,
        ),
        ObjectSetAction::Set(a) => (
            dp(
                "ve-tos object-set set",
                "PutObjectSet",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("objectset")],
                true,
            ),
            a,
        ),
        ObjectSetAction::SetLifecycle(a) => (
            dp(
                "ve-tos object-set set-lifecycle",
                "PutObjectSetLifecycle",
                Method::PUT,
                "/{bucket}",
                &[
                    QuerySpec::Flag("objectset-lifecycle"),
                    QuerySpec::Key(Source::ObjectSetName),
                ],
                true,
            ),
            a,
        ),
        ObjectSetAction::SetLifecycleByTag(a) => (
            dp(
                "ve-tos object-set set-lifecycle-by-tag",
                "PutObjectSetLifecycleByTag",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("objectset-lifecycle-bytag")],
                true,
            ),
            a,
        ),
        ObjectSetAction::SetQuota(a) => (
            dp(
                "ve-tos object-set set-quota",
                "PutObjectSetQuota",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("objectsetquota")],
                true,
            ),
            a,
        ),
        ObjectSetAction::SetQuotaByTag(a) => (
            dp(
                "ve-tos object-set set-quota-by-tag",
                "PutObjectSetQuotaByTag",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("objectsetquotabytag")],
                true,
            ),
            a,
        ),
        ObjectSetAction::SetTagging(a) => (
            dp(
                "ve-tos object-set set-tagging",
                "PutObjectSetTagging",
                Method::PUT,
                "/{bucket}",
                &[QuerySpec::Flag("objectsettagging")],
                true,
            ),
            a,
        ),
    };
    build_operation(spec, args, region_fallback)
}

fn accelerator_operation(
    action: &AcceleratorAction,
    region_fallback: Option<&str>,
) -> Result<AdvancedOperation, CliError> {
    let (spec, args) = match action {
        AcceleratorAction::Delete(a) => (
            cp(
                "ve-tos accelerator delete",
                "DeleteAccelerator",
                Method::DELETE,
                "/accelerator",
                &[QuerySpec::Param("id", Source::Id)],
                false,
            ),
            a,
        ),
        AcceleratorAction::DeleteEvictJob(a) => (
            cp(
                "ve-tos accelerator delete-evict-job",
                "DeleteAcceleratorEvictJob",
                Method::DELETE,
                "/accelerator/evictJob",
                &[QuerySpec::Param("jobId", Source::JobId)],
                false,
            ),
            a,
        ),
        AcceleratorAction::DeletePrefetchJob(a) => (
            cp(
                "ve-tos accelerator delete-prefetch-job",
                "DeleteAcceleratorPrefetchJob",
                Method::DELETE,
                "/accelerator/prefetchJob",
                &[QuerySpec::Param("jobId", Source::JobId)],
                false,
            ),
            a,
        ),
        AcceleratorAction::UnbindBucket(a) => (
            cp(
                "ve-tos accelerator unbind-bucket",
                "UnbindAcceleratorBucket",
                Method::DELETE,
                "/accelerator/{accelerator_id}/bucket/{bucket_name}",
                &[],
                false,
            ),
            a,
        ),
        AcceleratorAction::Get(a) => (
            cp(
                "ve-tos accelerator get",
                "GetAccelerator",
                Method::GET,
                "/accelerator",
                &[QuerySpec::Param("id", Source::Id)],
                false,
            ),
            a,
        ),
        AcceleratorAction::GetEvictJob(a) => (
            cp(
                "ve-tos accelerator get-evict-job",
                "GetAcceleratorEvictJob",
                Method::GET,
                "/accelerator/evictJob",
                &[QuerySpec::Param("jobId", Source::JobId)],
                false,
            ),
            a,
        ),
        AcceleratorAction::GetPrefetchJob(a) => (
            cp(
                "ve-tos accelerator get-prefetch-job",
                "GetAcceleratorPrefetchJob",
                Method::GET,
                "/accelerator/prefetchJob",
                &[QuerySpec::Param("jobId", Source::JobId)],
                false,
            ),
            a,
        ),
        AcceleratorAction::GetBandwidth(a) => (
            cp(
                "ve-tos accelerator get-bandwidth",
                "GetBandwidthQuota",
                Method::GET,
                "/accelerator/bandwidth",
                &[QuerySpec::Param("az", Source::Az)],
                false,
            ),
            a,
        ),
        AcceleratorAction::GetCapacity(a) => (
            cp(
                "ve-tos accelerator get-capacity",
                "GetCapacityQuota",
                Method::GET,
                "/accelerator/capacity",
                &[QuerySpec::Param("az", Source::Az)],
                false,
            ),
            a,
        ),
        AcceleratorAction::List(a) => (
            cp(
                "ve-tos accelerator list",
                "ListAccelerator",
                Method::GET,
                "/accelerator",
                &[],
                false,
            ),
            a,
        ),
        AcceleratorAction::ListEvictJobs(a) => (
            cp(
                "ve-tos accelerator list-evict-jobs",
                "ListAcceleratorEvictJob",
                Method::GET,
                "/accelerator/evictJob",
                &[QuerySpec::Param("acceleratorId", Source::AcceleratorId)],
                false,
            ),
            a,
        ),
        AcceleratorAction::ListPrefetchJobs(a) => (
            cp(
                "ve-tos accelerator list-prefetch-jobs",
                "ListAcceleratorPrefetchJob",
                Method::GET,
                "/accelerator/prefetchJob",
                &[QuerySpec::Param("acceleratorId", Source::AcceleratorId)],
                false,
            ),
            a,
        ),
        AcceleratorAction::ListPrefetchRecords(a) => (
            cp(
                "ve-tos accelerator list-prefetch-records",
                "ListAcceleratorPrefetchRecord",
                Method::GET,
                "/accelerator/prefetchRecord",
                &[QuerySpec::Param("jobId", Source::JobId)],
                false,
            ),
            a,
        ),
        AcceleratorAction::ListAz(a) => (
            cp(
                "ve-tos accelerator list-az",
                "ListAz",
                Method::GET,
                "/accelerator/az",
                &[QuerySpec::Param("region", Source::Region)],
                false,
            ),
            a,
        ),
        AcceleratorAction::ListForBucket(a) => (
            cp(
                "ve-tos accelerator list-for-bucket",
                "ListBindAcceleratorForBucket",
                Method::GET,
                "/bucket/{bucket_name}/accelerator",
                &[],
                false,
            ),
            a,
        ),
        AcceleratorAction::ListBindedAps(a) => (
            cp(
                "ve-tos accelerator list-binded-aps",
                "ListBindAccessPointForAccelerator",
                Method::GET,
                "/accelerator/{accelerator}/accesspoint",
                &[],
                false,
            ),
            a,
        ),
        AcceleratorAction::ListBindedBuckets(a) => (
            cp(
                "ve-tos accelerator list-binded-buckets",
                "ListBindBucketForAccelerator",
                Method::GET,
                "/accelerator/{accelerator_id}/bucket",
                &[],
                false,
            ),
            a,
        ),
        AcceleratorAction::Create(a) => (
            cp(
                "ve-tos accelerator create",
                "PutAccelerator",
                Method::POST,
                "/accelerator",
                &[
                    QuerySpec::Param("name", Source::Name),
                    // [Review Fix #1] PutAccelerator requires the region query; omitting it
                    // turns a valid --region CLI argument into a service-side InvalidArgument.
                    QuerySpec::Param("region", Source::Region),
                ],
                true,
            ),
            a,
        ),
        AcceleratorAction::CreateEvictJob(a) => (
            cp(
                "ve-tos accelerator create-evict-job",
                "PutAcceleratorEvictJob",
                Method::POST,
                "/accelerator/evictJob",
                &[QuerySpec::Param("acceleratorId", Source::AcceleratorId)],
                true,
            ),
            a,
        ),
        AcceleratorAction::CreatePrefetchJob(a) => (
            cp(
                "ve-tos accelerator create-prefetch-job",
                "PutAcceleratorPrefetchJob",
                Method::POST,
                "/accelerator/prefetchJob",
                &[QuerySpec::Param("acceleratorId", Source::AcceleratorId)],
                true,
            ),
            a,
        ),
        AcceleratorAction::BindBucket(a) => (
            cp(
                "ve-tos accelerator bind-bucket",
                "BindAcceleratorBucket",
                Method::PUT,
                "/accelerator/{accelerator_id}/bucket/{bucket_name}",
                &[],
                true,
            ),
            a,
        ),
    };
    build_operation(spec, args, region_fallback)
}

fn mrap_operation(
    action: &MrapAction,
    region_fallback: Option<&str>,
) -> Result<AdvancedOperation, CliError> {
    let (spec, args) = match action {
        MrapAction::Delete(a) => (
            cp(
                "ve-tos mrap delete",
                "DeleteMultiRegionAccessPoint",
                Method::DELETE,
                "/mrap",
                &[QuerySpec::Param("name", Source::Name)],
                false,
            ),
            a,
        ),
        MrapAction::DeleteMirror(a) => (
            cp(
                "ve-tos mrap delete-mirror",
                "DeleteMultiRegionAccessPointMirrorBack",
                Method::DELETE,
                "/mrap/mirror",
                &[QuerySpec::Param("alias", Source::Alias)],
                false,
            ),
            a,
        ),
        MrapAction::DeletePolicy(a) => (
            cp(
                "ve-tos mrap delete-policy",
                "DeleteMultiRegionAccessPointPolicy",
                Method::DELETE,
                "/mrap/{name}/policy",
                &[],
                false,
            ),
            a,
        ),
        MrapAction::UnbindAccelerator(a) => (
            cp(
                "ve-tos mrap unbind-accelerator",
                "UnBindAcceleratorWithMultiRegionAccessPoint",
                Method::DELETE,
                "/accelerator/{accelerator}/mrap/{alias}",
                &[],
                false,
            ),
            a,
        ),
        MrapAction::BindAccelerator(a) => (
            cp(
                "ve-tos mrap bind-accelerator",
                "BindAcceleratorWithMultiRegionAccessPoint",
                Method::PUT,
                "/accelerator/{accelerator}/mrap/{alias}",
                &[],
                true,
            ),
            a,
        ),
        MrapAction::Get(a) => (
            cp(
                "ve-tos mrap get",
                "GetMultiRegionAccessPoint",
                Method::GET,
                "/mrap",
                &[QuerySpec::Param("name", Source::Name)],
                false,
            ),
            a,
        ),
        MrapAction::GetMirror(a) => (
            cp(
                "ve-tos mrap get-mirror",
                "GetMultiRegionAccessPointMirrorBack",
                Method::GET,
                "/mrap/mirror",
                &[QuerySpec::Param("alias", Source::Alias)],
                false,
            ),
            a,
        ),
        MrapAction::GetPolicy(a) => (
            cp(
                "ve-tos mrap get-policy",
                "GetMultiRegionAccessPointPolicy",
                Method::GET,
                "/mrap/{name}/policy",
                &[],
                false,
            ),
            a,
        ),
        MrapAction::GetRoutes(a) => (
            cp(
                "ve-tos mrap get-routes",
                "GetMultiRegionAccessPointRoutes",
                Method::GET,
                "/mrap/routes",
                &[QuerySpec::Param("alias", Source::Alias)],
                false,
            ),
            a,
        ),
        MrapAction::ListAccelerators(a) => (
            cp(
                "ve-tos mrap list-accelerators",
                "ListAcceleratorForMultiRegionAccessPoint",
                Method::GET,
                "/mrap/{name}/accelerator",
                &[],
                false,
            ),
            a,
        ),
        MrapAction::ListMrapsForAccelerator(a) => (
            cp(
                "ve-tos mrap list-mraps-for-accelerator",
                "ListMultiRegionAccessPointForAccelerator",
                Method::GET,
                "/accelerator/{accelerator}/mrap",
                &[],
                false,
            ),
            a,
        ),
        MrapAction::List(a) => (
            cp(
                "ve-tos mrap list",
                "ListMultiRegionAccessPoints",
                Method::GET,
                "/mrap",
                &[],
                false,
            ),
            a,
        ),
        MrapAction::CreateRoutes(a) => (
            cp(
                "ve-tos mrap create-routes",
                "SubmitMultiRegionAccessPointRoutes",
                Method::PATCH,
                "/mrap/routes",
                &[QuerySpec::Param("alias", Source::Alias)],
                true,
            ),
            a,
        ),
        MrapAction::Create(a) => (
            cp(
                "ve-tos mrap create",
                "CreateMultiRegionAccessPoint",
                Method::POST,
                "/mrap",
                &[QuerySpec::Param("name", Source::Name)],
                true,
            ),
            a,
        ),
        MrapAction::SetMirror(a) => (
            cp(
                "ve-tos mrap set-mirror",
                "PutMultiRegionAccessPointMirrorBack",
                Method::PUT,
                "/mrap/mirror",
                &[QuerySpec::Param("alias", Source::Alias)],
                true,
            ),
            a,
        ),
        MrapAction::SetPolicy(a) => (
            cp(
                "ve-tos mrap set-policy",
                "PutMultiRegionAccessPointPolicy",
                Method::PUT,
                "/mrap/{name}/policy",
                &[],
                true,
            ),
            a,
        ),
    };
    build_operation(spec, args, region_fallback)
}

fn ap_operation(
    action: &ApAction,
    region_fallback: Option<&str>,
) -> Result<AdvancedOperation, CliError> {
    let (spec, args) = match action {
        ApAction::Delete(a) => (
            cp(
                "ve-tos ap delete",
                "DeleteAccessPoint",
                Method::DELETE,
                "/accesspoint/{name}",
                &[],
                false,
            ),
            a,
        ),
        ApAction::DeletePolicy(a) => (
            cp(
                "ve-tos ap delete-policy",
                "DeleteAccessPointPolicy",
                Method::DELETE,
                "/accesspoint/{name}/policy",
                &[],
                false,
            ),
            a,
        ),
        ApAction::UnbindAccelerator(a) => (
            cp(
                "ve-tos ap unbind-accelerator",
                "UnbindAcceleratorWithAccessPoint",
                Method::DELETE,
                "/accesspoint/{name}/accelerator/{accelerator}",
                &[],
                false,
            ),
            a,
        ),
        ApAction::BindAccelerator(a) => (
            cp(
                "ve-tos ap bind-accelerator",
                "BindAcceleratorWithAccessPoint",
                Method::PUT,
                "/accesspoint/{name}/accelerator/{accelerator}",
                &[],
                true,
            ),
            a,
        ),
        ApAction::Get(a) => (
            cp(
                "ve-tos ap get",
                "GetAccessPoint",
                Method::GET,
                "/accesspoint/{name}",
                &[],
                false,
            ),
            a,
        ),
        ApAction::GetPolicy(a) => (
            cp(
                "ve-tos ap get-policy",
                "GetAccessPointPolicy",
                Method::GET,
                "/accesspoint/{name}/policy",
                &[],
                false,
            ),
            a,
        ),
        ApAction::List(a) => (
            cp(
                "ve-tos ap list",
                "ListAccessPoints",
                Method::GET,
                "/accesspoint",
                &[],
                false,
            ),
            a,
        ),
        ApAction::ListAccelerators(a) => (
            cp(
                "ve-tos ap list-accelerators",
                "ListBindAcceleratorForAccessPoint",
                Method::GET,
                "/accesspoint/{name}/accelerator",
                &[],
                false,
            ),
            a,
        ),
        ApAction::Create(a) => (
            cp(
                "ve-tos ap create",
                "CreateAccessPoint",
                Method::PUT,
                "/accesspoint/{name}",
                &[],
                true,
            ),
            a,
        ),
        ApAction::SetPolicy(a) => (
            cp(
                "ve-tos ap set-policy",
                "PutAccessPointPolicy",
                Method::PUT,
                "/accesspoint/{name}/policy",
                &[],
                true,
            ),
            a,
        ),
    };
    build_operation(spec, args, region_fallback)
}

fn cap_operation(
    action: &CapAction,
    region_fallback: Option<&str>,
) -> Result<AdvancedOperation, CliError> {
    let (spec, args) = match action {
        CapAction::Delete(a) => (
            cp(
                "ve-tos cap delete",
                "DeleteConvergedAccessPoint",
                Method::DELETE,
                "/caps/{name}",
                &[],
                false,
            ),
            a,
        ),
        CapAction::DeleteCustomEndpoint(a) => (
            cp(
                "ve-tos cap delete-custom-endpoint",
                "DeleteConvergedAccessPointCustomEndpoint",
                Method::DELETE,
                "/caps/{name}/custom-endpoints",
                &[QuerySpec::Param("domain", Source::Domain)],
                false,
            ),
            a,
        ),
        CapAction::Get(a) => (
            cp(
                "ve-tos cap get",
                "GetConvergedAccessPoint",
                Method::GET,
                "/caps/{name}",
                &[],
                false,
            ),
            a,
        ),
        CapAction::GetCustomEndpointToken(a) => (
            cp(
                "ve-tos cap get-custom-endpoint-token",
                "GetConvergedAccessPointCustomEndpointToken",
                Method::GET,
                "/caps/{name}/custom-endpoints",
                &[QuerySpec::Flag("token")],
                false,
            ),
            a,
        ),
        CapAction::List(a) => (
            cp(
                "ve-tos cap list",
                "ListConvergedAccessPoints",
                Method::GET,
                "/caps",
                &[],
                false,
            ),
            a,
        ),
        CapAction::Create(a) => (
            cp(
                "ve-tos cap create",
                "CreateConvergedAccessPoint",
                Method::PUT,
                "/caps",
                &[],
                true,
            ),
            a,
        ),
        CapAction::CreateCustomEndpoint(a) => (
            cp(
                "ve-tos cap create-custom-endpoint",
                "PutConvergedAccessPointCustomEndpoint",
                Method::PUT,
                "/caps/{name}/custom-endpoints",
                &[],
                true,
            ),
            a,
        ),
        CapAction::CreateCustomEndpointToken(a) => (
            cp(
                "ve-tos cap create-custom-endpoint-token",
                "PutConvergedAccessPointCustomEndpointToken",
                Method::PUT,
                "/caps/{name}/custom-endpoints",
                &[QuerySpec::Flag("token")],
                true,
            ),
            a,
        ),
        CapAction::CreateObjectSet(a) => (
            cp(
                "ve-tos cap create-object-set",
                "PutConvergedAccessPointObjectSet",
                Method::PUT,
                "/caps/{name}/object-sets",
                &[],
                true,
            ),
            a,
        ),
    };
    build_operation(spec, args, region_fallback)
}

fn dataset_operation(
    action: &DatasetAction,
    region_fallback: Option<&str>,
) -> Result<AdvancedOperation, CliError> {
    let (spec, args) = match action {
        DatasetAction::Delete(a) => (
            cp(
                "ve-tos dataset delete",
                "DeleteDataset",
                Method::DELETE,
                "/dataset",
                // [Review Fix #13] API requires datasetname query parameter
                &[QuerySpec::Param("datasetname", Source::Name)],
                false,
            ),
            a,
        ),
        DatasetAction::DeleteBinding(a) => (
            cp(
                "ve-tos dataset delete-binding",
                "DeleteDatasetBinding",
                Method::DELETE,
                "/datasetbinding",
                &[],
                false,
            ),
            a,
        ),
        DatasetAction::Get(a) => (
            cp(
                "ve-tos dataset get",
                "GetDataset",
                Method::GET,
                "/dataset",
                // [Review Fix #13] API requires datasetname query parameter
                &[QuerySpec::Param("datasetname", Source::Name)],
                false,
            ),
            a,
        ),
        DatasetAction::GetBinding(a) => (
            cp(
                "ve-tos dataset get-binding",
                "GetDatasetBinding",
                Method::GET,
                "/datasetbinding",
                &[],
                false,
            ),
            a,
        ),
        DatasetAction::ListBindings(a) => (
            cp(
                "ve-tos dataset list-bindings",
                "ListDatasetBindings",
                Method::GET,
                "/datasetbindings",
                &[],
                false,
            ),
            a,
        ),
        DatasetAction::List(a) => (
            cp(
                "ve-tos dataset list",
                "ListDatasets",
                Method::GET,
                "/datasets",
                &[],
                false,
            ),
            a,
        ),
        DatasetAction::ListTemplates(a) => (
            cp(
                "ve-tos dataset list-templates",
                "ListTemplates",
                Method::GET,
                "/templates",
                &[],
                false,
            ),
            a,
        ),
        DatasetAction::Create(a) => (
            cp(
                "ve-tos dataset create",
                "CreateDataset",
                Method::POST,
                "/dataset",
                &[],
                true,
            ),
            a,
        ),
        DatasetAction::CreateBinding(a) => (
            cp(
                "ve-tos dataset create-binding",
                "CreateDatasetBinding",
                Method::POST,
                "/datasetbinding",
                &[],
                true,
            ),
            a,
        ),
        DatasetAction::Query(a) => (
            cp(
                "ve-tos dataset query",
                "QueryDataset",
                Method::POST,
                "/datasetquery",
                &[],
                true,
            ),
            a,
        ),
        DatasetAction::Update(a) => (
            cp(
                "ve-tos dataset update",
                "UpdateDataset",
                Method::PUT,
                "/dataset",
                &[],
                true,
            ),
            a,
        ),
    };
    build_operation(spec, args, region_fallback)
}

fn control_operation(
    action: &ControlAction,
    region_fallback: Option<&str>,
) -> Result<AdvancedOperation, CliError> {
    let (spec, args) = match action {
        ControlAction::CreateUrlCache(a) => (
            dp(
                "ve-tos control create-url-cache",
                "CreateUrlCache",
                Method::POST,
                "/{bucket}",
                &[QuerySpec::Flag("url_cache")],
                true,
            ),
            a,
        ),
        ControlAction::DeleteUrlCache(a) => (
            dp(
                "ve-tos control delete-url-cache",
                "DeleteUrlCache",
                Method::DELETE,
                "/{bucket}",
                &[QuerySpec::Flag("url_cache")],
                false,
            ),
            a,
        ),
        ControlAction::DeleteSubscribe(a) => (
            cp(
                "ve-tos control delete-subscribe",
                "DeleteSubscribeConfiguration",
                Method::DELETE,
                "/",
                &[QuerySpec::Flag("subscribeconfiguration")],
                false,
            ),
            a,
        ),
        ControlAction::GetSubscribe(a) => (
            cp(
                "ve-tos control get-subscribe",
                "GetSubscribeConfiguration",
                Method::GET,
                "/",
                &[QuerySpec::Flag("subscribeconfiguration")],
                false,
            ),
            a,
        ),
        ControlAction::SetSubscribe(a) => (
            cp(
                "ve-tos control set-subscribe",
                "PutSubscribeConfiguration",
                Method::PUT,
                "/",
                &[QuerySpec::Flag("subscribeconfiguration")],
                true,
            ),
            a,
        ),
        ControlAction::DeleteBatchJob(a) => (
            cp(
                "ve-tos control delete-batch-job",
                "DeleteJob",
                Method::DELETE,
                "/jobs/{job_id}",
                &[],
                false,
            ),
            a,
        ),
        ControlAction::GetBatchJob(a) => (
            cp(
                "ve-tos control get-batch-job",
                "DescribeJob",
                Method::GET,
                "/jobs/{job_id}",
                &[],
                false,
            ),
            a,
        ),
        ControlAction::ListBatchJobs(a) => (
            cp(
                "ve-tos control list-batch-jobs",
                "ListJobs",
                Method::GET,
                "/jobs",
                &[],
                false,
            ),
            a,
        ),
        ControlAction::CreateBatchJob(a) => (
            cp(
                "ve-tos control create-batch-job",
                "CreateJob",
                Method::POST,
                "/jobs",
                &[],
                true,
            ),
            a,
        ),
        ControlAction::SetBatchJobPriority(a) => (
            cp(
                "ve-tos control set-batch-job-priority",
                "UpdateJobPriority",
                Method::POST,
                "/jobs/{job_id}/priority",
                &[],
                true,
            ),
            a,
        ),
        ControlAction::SetBatchJobStatus(a) => (
            cp(
                "ve-tos control set-batch-job-status",
                "UpdateJobStatus",
                Method::POST,
                "/jobs/{job_id}/status",
                &[],
                true,
            ),
            a,
        ),
        ControlAction::DeleteLens(a) => (
            cp(
                "ve-tos control delete-lens",
                "DeleteStorageLensConfiguration",
                Method::DELETE,
                "/storagelens",
                &[QuerySpec::Param("id", Source::Id)],
                false,
            ),
            a,
        ),
        ControlAction::GetLens(a) => (
            cp(
                "ve-tos control get-lens",
                "GetStorageLensConfiguration",
                Method::GET,
                "/storagelens",
                &[QuerySpec::Param("id", Source::Id)],
                false,
            ),
            a,
        ),
        ControlAction::ListLens(a) => (
            cp(
                "ve-tos control list-lens",
                "ListStorageLensConfigurations",
                Method::GET,
                "/storagelens",
                &[],
                false,
            ),
            a,
        ),
        ControlAction::SetLens(a) => (
            cp(
                "ve-tos control set-lens",
                "PutStorageLensConfiguration",
                Method::PUT,
                "/storagelens",
                &[QuerySpec::Param("id", Source::Id)],
                true,
            ),
            a,
        ),
        ControlAction::DeleteQosPolicy(a) => (
            cp(
                "ve-tos control delete-qos-policy",
                "DeleteQosPolicy",
                Method::DELETE,
                "/qospolicy",
                &[],
                false,
            ),
            a,
        ),
        ControlAction::GetQosPolicy(a) => (
            cp(
                "ve-tos control get-qos-policy",
                "GetQosPolicy",
                Method::GET,
                "/qospolicy",
                &[],
                false,
            ),
            a,
        ),
        ControlAction::SetQosPolicy(a) => (
            cp(
                "ve-tos control set-qos-policy",
                "PutQosPolicy",
                Method::PUT,
                "/qospolicy",
                &[],
                true,
            ),
            a,
        ),
        ControlAction::ListResourceTags(a) => (
            dp(
                "ve-tos control list-resource-tags",
                "ListTagsForResource",
                Method::GET,
                "/tags/{resource_trn}",
                &[],
                false,
            ),
            a,
        ),
        ControlAction::SetResourceTag(a) => (
            dp(
                "ve-tos control set-resource-tag",
                "TagResource",
                Method::POST,
                "/tags/{resource_trn}",
                &[],
                true,
            ),
            a,
        ),
        ControlAction::DeleteResourceTag(a) => (
            dp(
                "ve-tos control delete-resource-tag",
                "UntagResource",
                Method::DELETE,
                "/tags/{resource_trn}",
                &[QuerySpec::Param("tagKeys", Source::TagKeys)],
                false,
            ),
            a,
        ),
    };
    build_operation(spec, args, region_fallback)
}

fn build_operation(
    spec: AdvancedSpec,
    args: &GenericArgs,
    region_fallback: Option<&str>,
) -> Result<AdvancedOperation, CliError> {
    let path = fill_path(spec.path, args, region_fallback)?;
    let mut query = BTreeMap::new();
    for item in spec.query {
        match item {
            QuerySpec::Flag(name) => {
                query.insert((*name).to_string(), String::new());
            }
            QuerySpec::Param(name, source) => {
                query.insert(
                    (*name).to_string(),
                    source_value(*source, args, region_fallback)?.to_string(),
                );
            }
            QuerySpec::Key(source) => {
                query.insert(
                    source_value(*source, args, region_fallback)?.to_string(),
                    String::new(),
                );
            }
        }
    }
    append_kv_pairs(&mut query, &args.query, "--query")?;

    let mut headers = BTreeMap::new();
    if let Some(content_md5) = &args.content_md5 {
        headers.insert("Content-MD5".to_string(), content_md5.clone());
    }
    append_kv_pairs(&mut headers, &args.header, "--header")?;

    let body = if spec.has_body {
        args.config
            .as_deref()
            .map(|source| advanced_body(&spec, args, region_fallback, source))
            .transpose()?
    } else {
        None
    };
    if spec.api == "PutDataProcessTemplate" {
        if let Some(body) = body.as_deref() {
            let tag = data_process_template_tag(body)?;
            match query.get("tag") {
                // [Review Fix #DataProcessTemplateTag] The service reads the
                // template type from the tag query parameter, while users
                // naturally put Tag in the JSON body. Keep the public CLI
                // ergonomic but reject ambiguous mismatches.
                Some(existing_tag) if existing_tag != &tag => {
                    return Err(CliError::ValidationError(format!(
                        "ve-tos data-process set-template --query tag={existing_tag} conflicts with body Tag={tag}"
                    )));
                }
                Some(_) => {}
                None => {
                    query.insert("tag".to_string(), tag);
                }
            }
        }
    }
    // [Review Fix #1] Data Plane Advanced APIs must resolve the bucket endpoint,
    // while Control Plane APIs must use the service/control endpoint.
    let bucket = if matches!(spec.endpoint, EndpointKind::DataPlane) {
        Some(source_value(Source::Bucket, args, region_fallback)?.to_string())
    } else {
        None
    };
    let target = args
        .bucket
        .clone()
        .or_else(|| args.name.clone())
        .or_else(|| args.id.clone())
        .unwrap_or_else(|| spec.path.to_string());
    let parameters = parameters(&spec);
    let destructive = spec.method == Method::DELETE;
    Ok(AdvancedOperation {
        spec,
        path,
        bucket,
        query,
        headers,
        body,
        parameters,
        destructive,
        force: args.force,
        target,
    })
}

fn advanced_body(
    spec: &AdvancedSpec,
    args: &GenericArgs,
    region_fallback: Option<&str>,
    source: &str,
) -> Result<Vec<u8>, CliError> {
    let body = read_body_input(source)?;
    if spec.api == "PutAccelerator" {
        // [Review Fix #5] PutAccelerator validates Region in the JSON body.
        // Keep --config usable while filling the required field from the same
        // source as the region query when the user did not include it manually.
        return accelerator_create_body(
            body,
            source_value(Source::Region, args, region_fallback)?,
            args.az.as_deref(),
        );
    }
    if spec.api == "CreateAccessPoint" {
        return access_point_create_body(body);
    }
    if spec.api == "PutImageStyleSeparator" {
        return image_style_separator_body(body);
    }
    if spec.api == "PutDataProcessTemplate" {
        return data_process_template_body(body);
    }
    if spec.api == "CreateDataset" {
        return dataset_create_body(body);
    }
    Ok(body)
}

fn parse_json_object_body(
    body: Vec<u8>,
    command: &str,
) -> Result<serde_json::Map<String, Value>, CliError> {
    let json_body: Value = serde_json::from_slice(&body).map_err(|err| {
        CliError::ValidationError(format!("{command} request body must be valid JSON: {err}"))
    })?;
    match json_body {
        Value::Object(map) => Ok(map),
        _ => Err(CliError::ValidationError(format!(
            "{command} request body must be a JSON object"
        ))),
    }
}

fn serialize_json_object_body(map: serde_json::Map<String, Value>) -> Result<Vec<u8>, CliError> {
    serde_json::to_vec(&Value::Object(map)).map_err(CliError::Json)
}

fn access_point_create_body(body: Vec<u8>) -> Result<Vec<u8>, CliError> {
    let mut map = parse_json_object_body(body, "ve-tos ap create")?;
    if let Some(origin) = map.get_mut("NetworkOrigin") {
        let Some(origin_text) = origin.as_str() else {
            return Err(CliError::ValidationError(
                "ve-tos ap create requires NetworkOrigin to be a string".into(),
            ));
        };
        let normalized = origin_text.trim().to_ascii_lowercase();
        if normalized != "vpc" && normalized != "internet" {
            return Err(CliError::ValidationError(
                "ve-tos ap create requires NetworkOrigin to be `vpc` or `internet`".into(),
            ));
        }
        *origin = Value::String(normalized);
    }
    serialize_json_object_body(map)
}

fn image_style_separator_body(body: Vec<u8>) -> Result<Vec<u8>, CliError> {
    let mut map = parse_json_object_body(body, "ve-tos data-process set-image-style-separator")?;
    if let Some(legacy) = map.remove("Separators") {
        if map.contains_key("Separator") {
            return Err(CliError::ValidationError(
                "ve-tos data-process set-image-style-separator cannot use both Separator and Separators"
                    .into(),
            ));
        }
        // [Review Fix #ImageStyleSeparator] Older tests used the SDK-internal
        // map shape. The service schema is Separator: []string, so convert that
        // legacy map into the documented list while preserving canonical input.
        let separators = match legacy {
            Value::Object(entries) => entries.keys().cloned().map(Value::String).collect(),
            Value::Array(items) => items,
            _ => {
                return Err(CliError::ValidationError(
                    "ve-tos data-process set-image-style-separator requires Separators to be an object or array".into(),
                ))
            }
        };
        map.insert("Separator".to_string(), Value::Array(separators));
    }
    validate_string_array_field(
        &map,
        "Separator",
        true,
        "ve-tos data-process set-image-style-separator",
    )?;
    validate_string_map_field(
        &map,
        "SeparatorPrefix",
        "ve-tos data-process set-image-style-separator",
    )?;
    validate_string_map_field(
        &map,
        "SeparatorSuffix",
        "ve-tos data-process set-image-style-separator",
    )?;
    serialize_json_object_body(map)
}

fn data_process_template_body(body: Vec<u8>) -> Result<Vec<u8>, CliError> {
    let mut map = parse_json_object_body(body, "ve-tos data-process set-template")?;
    let tag = map.get("Tag").and_then(Value::as_str).ok_or_else(|| {
        CliError::ValidationError("ve-tos data-process set-template requires Tag".into())
    })?;
    if !matches!(tag, "Transcode" | "AudioConvert" | "Watermark") {
        return Err(CliError::ValidationError(
            "ve-tos data-process set-template requires Tag to be Transcode, AudioConvert, or Watermark"
                .into(),
        ));
    }
    if tag == "Transcode" {
        if !map.contains_key("TranscodeConfig") {
            if let Some(legacy) = map.remove("TranscodeTemplate") {
                // [Review Fix #DataProcessTemplatePayload] The public API uses
                // TranscodeConfig for tag=Transcode. Convert the earlier e2e
                // placeholder name to the service schema instead of sending an
                // invalid payload to TOS.
                map.insert("TranscodeConfig".to_string(), legacy);
            }
        }
        match map.get("TranscodeConfig") {
            Some(Value::Object(_)) => {}
            _ => {
                return Err(CliError::ValidationError(
                    "ve-tos data-process set-template requires TranscodeConfig object when Tag=Transcode"
                        .into(),
                ));
            }
        }
    }
    serialize_json_object_body(map)
}

fn data_process_template_tag(body: &[u8]) -> Result<String, CliError> {
    let json_body: Value = serde_json::from_slice(body).map_err(|err| {
        CliError::ValidationError(format!(
            "ve-tos data-process set-template request body must be valid JSON: {err}"
        ))
    })?;
    json_body
        .get("Tag")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| {
            CliError::ValidationError("ve-tos data-process set-template requires Tag".into())
        })
}

fn dataset_create_body(body: Vec<u8>) -> Result<Vec<u8>, CliError> {
    let map = parse_json_object_body(body, "ve-tos dataset create")?;
    let template_id = map.get("TemplateId").and_then(Value::as_str).unwrap_or("");
    if template_id.trim().is_empty() {
        return Err(CliError::ValidationError(
            // [Review Fix #DatasetTemplateId] CreateDataset requires TemplateId
            // in the request body; without it the later object-set setup fails
            // against the service schema.
            "ve-tos dataset create requires TemplateId in --config".into(),
        ));
    }
    serialize_json_object_body(map)
}

fn validate_string_array_field(
    map: &serde_json::Map<String, Value>,
    field: &str,
    required: bool,
    command: &str,
) -> Result<(), CliError> {
    let Some(value) = map.get(field) else {
        if required {
            return Err(CliError::ValidationError(format!(
                "{command} requires {field}"
            )));
        }
        return Ok(());
    };
    let Value::Array(items) = value else {
        return Err(CliError::ValidationError(format!(
            "{command} requires {field} to be an array of strings"
        )));
    };
    if items.iter().any(|item| !item.is_string()) {
        return Err(CliError::ValidationError(format!(
            "{command} requires {field} to contain only strings"
        )));
    }
    Ok(())
}

fn validate_string_map_field(
    map: &serde_json::Map<String, Value>,
    field: &str,
    command: &str,
) -> Result<(), CliError> {
    let Some(value) = map.get(field) else {
        return Ok(());
    };
    let Value::Object(entries) = value else {
        return Err(CliError::ValidationError(format!(
            "{command} requires {field} to be an object"
        )));
    };
    if entries.values().any(|value| !value.is_string()) {
        return Err(CliError::ValidationError(format!(
            "{command} requires {field} values to be strings"
        )));
    }
    Ok(())
}

fn accelerator_create_body(
    body: Vec<u8>,
    region: &str,
    az: Option<&str>,
) -> Result<Vec<u8>, CliError> {
    let mut json_body: Value = serde_json::from_slice(&body).map_err(|err| {
        CliError::ValidationError(format!(
            "ve-tos accelerator create request body must be valid JSON: {err}"
        ))
    })?;
    let Value::Object(map) = &mut json_body else {
        return Err(CliError::ValidationError(format!(
            "ve-tos accelerator create request body must be a JSON object"
        )));
    };
    insert_or_validate_json_string(map, "Region", region)?;
    if let Some(az) = az {
        // [Review Fix #6] PutAccelerator also validates Az in the JSON body;
        // --az should therefore augment --config the same way --region does.
        insert_or_validate_json_string(map, "Az", az)?;
    }
    serde_json::to_vec(&json_body).map_err(CliError::Json)
}

fn insert_or_validate_json_string(
    map: &mut serde_json::Map<String, Value>,
    field: &str,
    value: &str,
) -> Result<(), CliError> {
    match map.get(field).and_then(Value::as_str) {
        Some(existing) if existing == value => Ok(()),
        Some(existing) => Err(CliError::ValidationError(format!(
            "ve-tos accelerator create has conflicting {field}: --config uses `{existing}` but CLI/profile resolves `{value}`"
        ))),
        None => {
            map.insert(field.to_string(), Value::String(value.to_string()));
            Ok(())
        }
    }
}

fn fill_path(
    path: &str,
    args: &GenericArgs,
    region_fallback: Option<&str>,
) -> Result<String, CliError> {
    let mut out = path.to_string();
    for (name, source) in [
        ("bucket", Source::Bucket),
        ("name", Source::Name),
        ("id", Source::Id),
        ("job_id", Source::JobId),
        ("jobID", Source::JobId),
        ("alias", Source::Alias),
        ("accelerator", Source::Accelerator),
        ("accelerator_id", Source::AcceleratorId),
        ("bucket_name", Source::BucketName),
        ("resource_trn", Source::ResourceTrn),
        ("object", Source::Object),
    ] {
        let marker = format!("{{{name}}}");
        if out.contains(&marker) {
            out = out.replace(&marker, source_value(source, args, region_fallback)?);
        }
    }
    Ok(out)
}

fn source_value<'a>(
    source: Source,
    args: &'a GenericArgs,
    region_fallback: Option<&'a str>,
) -> Result<&'a str, CliError> {
    let (name, value) = match source {
        Source::Name => ("--name", args.name.as_deref()),
        Source::Bucket => ("--bucket", args.bucket.as_deref()),
        Source::Id => ("--id", args.id.as_deref()),
        Source::StyleName => ("--style-name", args.style_name.as_deref()),
        Source::JobId => (
            "--job-id/--id",
            args.job_id.as_deref().or(args.id.as_deref()),
        ),
        Source::Alias => ("--alias", args.alias.as_deref()),
        Source::Accelerator => ("--accelerator", args.accelerator.as_deref()),
        Source::AcceleratorId => ("--accelerator-id", args.accelerator_id.as_deref()),
        Source::BucketName => ("--bucket-name", args.bucket_name.as_deref()),
        Source::Domain => ("--domain", args.domain.as_deref()),
        Source::Az => ("--az", args.az.as_deref()),
        Source::Region => ("--region", args.region.as_deref().or(region_fallback)),
        Source::ResourceTrn => ("--resource-trn", args.resource_trn.as_deref()),
        Source::TagKeys => ("--tag-keys", args.tag_keys.as_deref()),
        Source::Tag => ("--tag", args.tag.as_deref()),
        Source::ObjectSetName => ("--object-set-name", args.object_set_name.as_deref()),
        Source::Object => ("--object", args.object.as_deref()),
    };
    value.ok_or_else(|| CliError::ValidationError(format!("missing required {name}")))
}

fn append_kv_pairs(
    target: &mut BTreeMap<String, String>,
    pairs: &[String],
    flag: &str,
) -> Result<(), CliError> {
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(CliError::ValidationError(format!(
                "{flag} expects k=v, got '{pair}'"
            )));
        };
        target.insert(key.to_string(), value.to_string());
    }
    Ok(())
}

fn dp(
    command: &'static str,
    api: &'static str,
    method: Method,
    path: &'static str,
    query: &'static [QuerySpec],
    has_body: bool,
) -> AdvancedSpec {
    spec(
        command,
        api,
        EndpointKind::DataPlane,
        method,
        path,
        query,
        has_body,
    )
}

fn cp(
    command: &'static str,
    api: &'static str,
    method: Method,
    path: &'static str,
    query: &'static [QuerySpec],
    has_body: bool,
) -> AdvancedSpec {
    spec(
        command,
        api,
        EndpointKind::ControlPlane,
        method,
        path,
        query,
        has_body,
    )
}

fn spec(
    command: &'static str,
    api: &'static str,
    endpoint: EndpointKind,
    method: Method,
    path: &'static str,
    query: &'static [QuerySpec],
    has_body: bool,
) -> AdvancedSpec {
    AdvancedSpec {
        command,
        api,
        description: api,
        endpoint,
        method,
        path,
        query,
        has_body,
    }
}

fn parameters(spec: &AdvancedSpec) -> Vec<CommandParameter> {
    let mut params = Vec::new();
    if spec.path.contains("{bucket}") {
        params.push(parameter(
            "bucket",
            ParameterLocation::Path,
            true,
            "Bucket name",
        ));
    }
    for (marker, parameter_name) in [
        ("{name}", "name"),
        ("{id}", "id"),
        ("{job_id}", "job-id"),
        ("{jobID}", "job-id"),
        ("{alias}", "alias"),
        ("{accelerator}", "accelerator"),
        ("{accelerator_id}", "accelerator-id"),
        ("{bucket_name}", "bucket-name"),
        ("{resource_trn}", "resource-trn"),
        ("{object}", "object"),
    ] {
        if spec.path.contains(marker) {
            params.push(parameter(
                parameter_name,
                ParameterLocation::Path,
                true,
                "Documented path parameter",
            ));
        }
    }
    for item in spec.query {
        match item {
            QuerySpec::Flag(name) => params.push(parameter(
                name,
                ParameterLocation::Query,
                true,
                "Fixed operation query flag",
            )),
            QuerySpec::Param(name, _) => params.push(parameter(
                name,
                ParameterLocation::Query,
                true,
                "Documented query parameter",
            )),
            QuerySpec::Key(_) => params.push(parameter(
                "object-set-name",
                ParameterLocation::Query,
                true,
                "Object set name as query key",
            )),
        }
    }
    if spec.has_body {
        params.push(parameter(
            "config",
            ParameterLocation::Body,
            true,
            "JSON body via --config",
        ));
        params.push(parameter(
            "Content-MD5",
            ParameterLocation::Header,
            false,
            "Body MD5 header",
        ));
    }
    params
}

fn parameter(
    name: &'static str,
    location: ParameterLocation,
    required: bool,
    description: &'static str,
) -> CommandParameter {
    CommandParameter {
        name: name.to_string(),
        location,
        required,
        description: description.to_string(),
        ..Default::default()
    }
}

fn describe_action(op: &AdvancedOperation) -> CommandDescription {
    let risk_level = operation_risk(op);
    CommandDescription {
        command: op.spec.command.to_string(),
        layer: CommandLayer::LowLevel,
        api: Some(op.spec.api.to_string()),
        description: op.spec.description.to_string(),
        risk_level,
        supports_dry_run: true,
        supports_pipe: false,
        parameters: Some(copy_parameters(&op.parameters)),
        scenario_routing: Some(HashMap::from([
            (
                "endpoint_kind".to_string(),
                format!("{:?}", op.spec.endpoint),
            ),
            (
                "request".to_string(),
                json!({
                    "method": op.spec.method.as_str(),
                    "path": op.path,
                    "query": op.query,
                    "headers": op.headers.keys().collect::<Vec<_>>(),
                    "body": op.body.as_ref().map(|body| format!("{} bytes", body.len())),
                })
                .to_string(),
            ),
            (
                "body_contract".to_string(),
                "Header/Query use explicit CLI parameters; Body uses --config JSON.".to_string(),
            ),
        ])),
        related_commands: None,
        low_level_apis: None,
        ..Default::default()
    }
}

fn dry_run(op: &AdvancedOperation) -> DryRunResult {
    let risk_level = operation_risk(op);
    let endpoint = match op.spec.endpoint {
        EndpointKind::DataPlane => "data-plane",
        EndpointKind::ControlPlane => "control-plane",
    };
    DryRunResult {
        action: op.spec.command.to_string(),
        dry_run: true,
        impact: Impact {
            affected_objects: if op.destructive { 1 } else { 0 },
            affected_bytes: 0,
            risk_level: format!("{:?}", risk_level).to_lowercase(),
            estimated_duration: Some("< 1s".to_string()),
            scanned_count: None,
            preview_truncated: None,
        },
        plan: vec![
            format!(
                "{} {} via {} endpoint",
                op.spec.method.as_str(),
                op.path,
                endpoint
            ),
            format!("api={}", op.spec.api),
            format!("query={}", json!(&op.query)),
        ],
        warnings: {
            let mut warnings = Vec::new();
            if op.spec.has_body {
                warnings.push(
                    "Request body is omitted from dry-run output; validate --config before execution."
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

fn operation_risk(op: &AdvancedOperation) -> RiskLevel {
    if op.destructive {
        RiskLevel::High
    } else if op.spec.has_body {
        RiskLevel::Medium
    } else {
        RiskLevel::Low
    }
}

fn copy_parameters(parameters: &[CommandParameter]) -> Vec<CommandParameter> {
    parameters
        .iter()
        .map(|param| CommandParameter {
            name: param.name.clone(),
            location: copy_location(&param.location),
            required: param.required,
            description: param.description.clone(),
            ..Default::default()
        })
        .collect()
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
        "endpoint_rule": {
            "data_plane": "Use bucket endpoint for bucket-scoped Advanced APIs.",
            "control_plane": "Use control endpoint for management APIs."
        },
        "subcommands": subcommands
            .iter()
            .map(|(name, desc)| json!({ "name": name, "description": desc }))
            .collect::<Vec<Value>>()
    })
}

// [Review Fix #2] Group-level --describe must enumerate every Advanced action so
// agents can discover the full low-level surface without falling back to --help.
fn data_process_actions() -> &'static [(&'static str, &'static str)] {
    &[
        ("delete-image-style", "Delete image style"),
        ("get-image-style", "Get image style"),
        ("list-image-styles", "List image styles"),
        ("set-image-style", "Set image style with --config JSON"),
        (
            "list-image-style-brief-infos",
            "List image style brief info",
        ),
        ("list-image-style-contents", "List image style contents"),
        ("set-image-protect-rule", "Set original image protect rule"),
        ("get-image-protect-rule", "Get original image protect rule"),
        ("set-image-style-separator", "Set image style separator"),
        ("get-image-style-separator", "Get image style separator"),
        ("set-private-m3u8-rule", "Set private M3U8 rule"),
        ("get-private-m3u8-rule", "Get private M3U8 rule"),
        ("set-blind-watermark-rule", "Set blind watermark rule"),
        ("get-blind-watermark-rule", "Get blind watermark rule"),
        ("delete-workflow", "Delete workflow"),
        ("get-workflow", "Get workflow"),
        ("set-workflow", "Set workflow"),
        ("get-workflow-execution", "Get workflow execution"),
        ("list-workflow-executions", "List workflow executions"),
        ("delete-template", "Delete process template"),
        ("get-template", "Get process template"),
        ("set-template", "Set process template"),
        ("create-audit-job", "Create audit job"),
        ("create-doc-job", "Create document job"),
        ("create-file-job", "Create file job"),
        ("create-media-job", "Create media job"),
        ("get-job", "Get data process job"),
        ("list-jobs", "List data process jobs"),
        ("delete-increment-audit", "Delete increment audit"),
        ("get-audit", "Get audit"),
        ("get-increment-audit", "Get increment audit"),
        ("list-audits", "List audits"),
        ("list-increment-audits", "List increment audits"),
        ("create-increment-audit", "Create increment audit"),
        ("create-audit", "Create audit"),
    ]
}

fn object_set_actions() -> &'static [(&'static str, &'static str)] {
    &[
        ("delete", "Delete object set"),
        ("delete-lifecycle", "Delete object set lifecycle"),
        (
            "delete-lifecycle-by-tag",
            "Delete object set lifecycle by tag",
        ),
        ("delete-quota-by-tag", "Delete object set quota by tag"),
        ("get", "Get object set"),
        ("list", "List object sets"),
        ("set", "Set object set via --config JSON"),
        ("get-global", "Get bucket object-set configuration"),
        ("set-global", "Set bucket object-set configuration"),
        ("get-endpoint", "Get object set endpoint"),
        ("get-lifecycle", "Get object set lifecycle"),
        ("get-lifecycle-by-tag", "Get object set lifecycle by tag"),
        ("set-lifecycle", "Set object set lifecycle"),
        ("set-lifecycle-by-tag", "Set object set lifecycle by tag"),
        ("get-quota", "Get object set quota"),
        ("get-quota-by-tag", "Get object set quota by tag"),
        ("set-quota", "Set object set quota"),
        ("set-quota-by-tag", "Set object set quota by tag"),
        ("get-storage", "Get object set storage"),
        ("get-tagging", "Get object set tagging"),
        ("set-tagging", "Set object set tagging"),
    ]
}

fn accelerator_actions() -> &'static [(&'static str, &'static str)] {
    &[
        ("delete", "Delete accelerator"),
        ("delete-evict-job", "Delete evict job"),
        ("delete-prefetch-job", "Delete prefetch job"),
        ("get", "Get accelerator"),
        ("get-evict-job", "Get evict job"),
        ("get-prefetch-job", "Get prefetch job"),
        ("list", "List accelerators"),
        ("create", "Create accelerator"),
        ("bind-bucket", "Bind bucket to accelerator"),
        ("unbind-bucket", "Unbind bucket from accelerator"),
        ("get-bandwidth", "Get bandwidth quota"),
        ("get-capacity", "Get capacity quota"),
        ("list-az", "List availability zones"),
        ("list-evict-jobs", "List evict jobs"),
        ("list-prefetch-jobs", "List prefetch jobs"),
        ("list-prefetch-records", "List prefetch records"),
        ("list-for-bucket", "List accelerators bound to bucket"),
        ("list-binded-aps", "List access points bound to accelerator"),
        ("list-binded-buckets", "List buckets bound to accelerator"),
        ("create-prefetch-job", "Create prefetch job"),
        ("create-evict-job", "Create evict job"),
    ]
}

fn mrap_actions() -> &'static [(&'static str, &'static str)] {
    &[
        ("delete", "Delete MRAP"),
        ("get", "Get MRAP"),
        ("get-mirror", "Get MRAP mirror"),
        ("list", "List MRAPs"),
        ("create", "Create MRAP"),
        ("get-policy", "Get MRAP policy"),
        ("set-policy", "Set MRAP policy"),
        ("delete-policy", "Delete MRAP policy"),
        ("delete-mirror", "Delete MRAP mirror"),
        ("set-mirror", "Set MRAP mirror"),
        ("get-routes", "Get MRAP routes"),
        ("create-routes", "Submit MRAP routes"),
        ("list-accelerators", "List accelerators for MRAP"),
        ("list-mraps-for-accelerator", "List MRAPs for accelerator"),
        ("bind-accelerator", "Bind accelerator"),
        ("unbind-accelerator", "Unbind accelerator"),
    ]
}

fn ap_actions() -> &'static [(&'static str, &'static str)] {
    &[
        ("delete", "Delete access point"),
        ("get", "Get access point"),
        ("list", "List access points"),
        ("create", "Create access point"),
        ("get-policy", "Get access point policy"),
        ("set-policy", "Set access point policy"),
        ("delete-policy", "Delete access point policy"),
        ("bind-accelerator", "Bind accelerator"),
        ("unbind-accelerator", "Unbind accelerator"),
        ("list-accelerators", "List bound accelerators"),
    ]
}

fn cap_actions() -> &'static [(&'static str, &'static str)] {
    &[
        ("delete", "Delete converged access point"),
        ("get", "Get converged access point"),
        ("list", "List converged access points"),
        ("create", "Create converged access point"),
        ("create-custom-endpoint", "Create custom endpoint"),
        ("delete-custom-endpoint", "Delete custom endpoint"),
        ("get-custom-endpoint-token", "Get custom endpoint token"),
        (
            "create-custom-endpoint-token",
            "Create custom endpoint token",
        ),
        ("create-object-set", "Create object set binding"),
    ]
}

fn dataset_actions() -> &'static [(&'static str, &'static str)] {
    &[
        ("delete", "Delete dataset"),
        ("get", "Get dataset"),
        ("list", "List datasets"),
        ("create", "Create dataset"),
        ("update", "Update dataset"),
        ("query", "Query dataset"),
        ("create-binding", "Create dataset binding"),
        ("delete-binding", "Delete dataset binding"),
        ("get-binding", "Get dataset binding"),
        ("list-bindings", "List dataset bindings"),
        ("list-templates", "List templates"),
    ]
}

fn control_actions() -> &'static [(&'static str, &'static str)] {
    &[
        ("create-url-cache", "Create URL cache on Data Plane"),
        ("delete-url-cache", "Delete URL cache on Data Plane"),
        ("get-subscribe", "Get subscribe configuration"),
        ("set-subscribe", "Set subscribe configuration"),
        ("delete-subscribe", "Delete subscribe configuration"),
        ("create-batch-job", "Create batch job"),
        ("get-batch-job", "Get batch job"),
        ("list-batch-jobs", "List batch jobs"),
        ("delete-batch-job", "Delete batch job"),
        ("set-batch-job-priority", "Update batch job priority"),
        ("set-batch-job-status", "Update batch job status"),
        ("get-lens", "Get storage lens"),
        ("set-lens", "Set storage lens"),
        ("list-lens", "List storage lenses"),
        ("delete-lens", "Delete storage lens"),
        ("get-qos-policy", "Get QoS policy"),
        ("set-qos-policy", "Set QoS policy"),
        ("delete-qos-policy", "Delete QoS policy"),
        ("list-resource-tags", "List resource tags on Data Plane"),
        ("set-resource-tag", "Set resource tag on Data Plane"),
        ("delete-resource-tag", "Delete resource tag on Data Plane"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generic_args() -> GenericArgs {
        GenericArgs {
            name: None,
            bucket: None,
            id: None,
            style_name: None,
            job_id: None,
            job_type: None,
            alias: None,
            accelerator: None,
            accelerator_id: None,
            bucket_name: None,
            domain: None,
            az: None,
            region: None,
            resource_trn: None,
            tag_keys: None,
            tag: None,
            object_set_name: None,
            object: None,
            config: None,
            content_md5: None,
            query: Vec::new(),
            header: Vec::new(),
            force: false,
        }
    }

    #[test]
    fn accelerator_create_injects_fallback_region_into_query_and_body() {
        let mut args = generic_args();
        args.name = Some("acc-demo".to_string());
        args.az = Some("az-a".to_string());
        args.config = Some(r#"{"Name":"acc-demo"}"#.to_string());

        let op = accelerator_operation(&AcceleratorAction::Create(args), Some("cn-guilin-boe"))
            .expect("accelerator operation");
        let body: Value =
            serde_json::from_slice(op.body.as_ref().expect("body")).expect("json body");

        assert_eq!(
            op.query.get("region").map(String::as_str),
            Some("cn-guilin-boe")
        );
        assert_eq!(body["Region"], "cn-guilin-boe");
        assert_eq!(body["Az"], "az-a");
        assert_eq!(body["Name"], "acc-demo");
    }

    #[test]
    fn accelerator_create_rejects_conflicting_config_region() {
        let mut args = generic_args();
        args.name = Some("acc-demo".to_string());
        args.config = Some(r#"{"Name":"acc-demo","Region":"cn-beijing"}"#.to_string());

        let err = accelerator_operation(&AcceleratorAction::Create(args), Some("cn-guilin-boe"))
            .expect_err("conflicting region should fail");

        assert!(
            err.to_string().contains("conflicting Region"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn ap_create_normalizes_network_origin() {
        let body =
            access_point_create_body(br#"{"Bucket":"demo","NetworkOrigin":"Internet"}"#.to_vec())
                .expect("ap create body");
        let value: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(value["NetworkOrigin"], "internet");
    }

    #[test]
    fn image_style_separator_body_matches_service_schema() {
        let body = image_style_separator_body(
            br#"{"Separator":["-"],"SeparatorPrefix":{"dash":"-"},"SeparatorSuffix":{"dot":"."}}"#
                .to_vec(),
        )
        .expect("separator body");
        let value: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(value["Separator"], json!(["-"]));
        assert_eq!(value["SeparatorPrefix"]["dash"], "-");
        assert_eq!(value["SeparatorSuffix"]["dot"], ".");
    }

    #[test]
    fn image_style_separator_body_converts_legacy_map_shape() {
        let body = image_style_separator_body(br#"{"Separators":{"-":{},"_":{}}}"#.to_vec())
            .expect("separator body");
        let value: Value = serde_json::from_slice(&body).expect("json body");
        let mut separators = value["Separator"]
            .as_array()
            .expect("separator array")
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        separators.sort();

        assert_eq!(separators, vec!["-".to_string(), "_".to_string()]);
    }

    #[test]
    fn data_process_template_body_rejects_unknown_tag() {
        let err = data_process_template_body(br#"{"Name":"demo","Tag":"audio"}"#.to_vec())
            .expect_err("invalid tag should fail");

        assert!(
            err.to_string()
                .contains("Transcode, AudioConvert, or Watermark"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn data_process_template_body_converts_transcode_template_alias() {
        let body = data_process_template_body(
            br#"{"Name":"demo","Tag":"Transcode","TranscodeTemplate":{"Container":{"Format":"mp4"},"Video":{"Codec":"h264"}}}"#
                .to_vec(),
        )
        .expect("template body");
        let value: Value = serde_json::from_slice(&body).expect("json body");

        assert_eq!(value["TranscodeConfig"]["Container"]["Format"], "mp4");
        assert!(value.get("TranscodeTemplate").is_none());
    }

    #[test]
    fn data_process_template_operation_injects_tag_query_from_body() {
        let mut args = generic_args();
        args.bucket = Some("demo-bucket".to_string());
        args.config = Some(r#"{"Name":"demo","Tag":"Transcode","TranscodeConfig":{}}"#.to_string());

        let op = data_process_operation(&DataProcessAction::SetTemplate(args), None)
            .expect("set-template operation");

        assert_eq!(op.query.get("tag").map(String::as_str), Some("Transcode"));
        assert!(op.query.contains_key("process_template"));
    }

    #[test]
    fn data_process_template_operation_rejects_conflicting_tag_query() {
        let mut args = generic_args();
        args.bucket = Some("demo-bucket".to_string());
        args.config = Some(r#"{"Name":"demo","Tag":"Transcode","TranscodeConfig":{}}"#.to_string());
        args.query.push("tag=Watermark".to_string());

        let err = data_process_operation(&DataProcessAction::SetTemplate(args), None)
            .expect_err("conflicting tag should fail");

        assert!(
            err.to_string().contains("conflicts"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn data_process_get_template_uses_explicit_tag_parameter() {
        let mut args = generic_args();
        args.bucket = Some("demo-bucket".to_string());
        args.tag = Some("Transcode".to_string());

        let op = data_process_operation(&DataProcessAction::GetTemplate(args), None)
            .expect("get-template operation");

        assert_eq!(op.query.get("tag").map(String::as_str), Some("Transcode"));
        assert!(op.query.contains_key("process_template"));
    }

    #[test]
    fn data_process_delete_template_requires_tag_and_id_parameters() {
        let mut args = generic_args();
        args.bucket = Some("demo-bucket".to_string());
        args.tag = Some("Transcode".to_string());
        args.id = Some("template-id".to_string());
        args.force = true;

        let op = data_process_operation(&DataProcessAction::DeleteTemplate(args), None)
            .expect("delete-template operation");

        assert_eq!(op.query.get("tag").map(String::as_str), Some("Transcode"));
        assert_eq!(op.query.get("id").map(String::as_str), Some("template-id"));
        assert!(op.query.contains_key("process_template"));
    }

    #[test]
    fn dataset_create_body_requires_template_id() {
        let err = dataset_create_body(br#"{"DatasetName":"demo"}"#.to_vec())
            .expect_err("missing template id should fail");

        assert!(
            err.to_string().contains("TemplateId"),
            "unexpected error: {err}"
        );
    }
}
