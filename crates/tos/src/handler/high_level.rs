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

use std::collections::{hash_map::DefaultHasher, BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::io::{Read as IoRead, Seek, SeekFrom, Write as IoWrite};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use futures::stream::{FuturesUnordered, StreamExt};
use indicatif::{HumanBytes, ProgressBar, ProgressStyle};

use reqwest::{Body, Method, Response};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;
use tos_core::agent::describe::{
    CommandDescription, CommandLayer, CommandParameter, ParameterLocation, RelatedCommands,
    RiskLevel,
};
use tos_core::agent::dryrun::Impact;
use tos_core::agent::envelope::{Envelope, PaginationInfo};
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::agent::output::OutputFormat;
use tos_core::infra::auth::{hash_payload, hash_reader};
use tos_core::infra::client::TosClient;
use tos_core::infra::config::{
    Binary, DEFAULT_BATCH_CONCURRENCY, DEFAULT_LIST_CONCURRENCY, DEFAULT_MULTIPART_CONCURRENCY,
    DEFAULT_OVERWRITE_STRATEGY, DEFAULT_PROGRESS_GRANULARITY, DEFAULT_TOS_BATCH_REPORT_DIR,
    DEFAULT_TOS_BATCH_REPORT_FORMAT, DEFAULT_TOS_CHECKPOINT_DIR, DEFAULT_TOS_PROGRESS_ENABLED,
    DEFAULT_TRANSFER_CHECKPOINT_THRESHOLD,
};
use tos_core::transfer::checkpoint::{Checkpoint, CompletedPart};
use tos_core::transfer::upload::UploadStrategy;

use crate::cli::high_level::*;
use crate::cli::TosCommand;
use crate::domain::{bucket, core};
use crate::handler::common::{
    active_tos_config_binary, build_profile, ensure_force_for_destructive, output_result,
    output_result_with_columns,
};
use crate::registry::{describe_command_metadata, enforce_registry_guards};

/// Default columns for `ve-tos ls` table/csv view when listing objects.
const LS_OBJECT_TABLE_COLUMNS: &[&str] = &[
    "entry_type",
    "key",
    "size",
    "last_modified",
    "storage_class",
];
// [Review Fix #1] Keep verbose du diagnostics bounded on buckets with massive cardinality.
const DU_CATEGORY_BUCKET_LIMIT: usize = 1024;
const DU_REQUEST_ID_LIMIT: usize = 1024;
const DU_OVERFLOW_BUCKET: &str = "(other)";
const DEFAULT_BATCH_FILE_ROLLOVER_BYTES: u64 = 50 * 1024 * 1024;
const TOS_OBJECT_STORAGE_CLASS_VALUES: &[&str] = &[
    "STANDARD",
    "IA",
    "ARCHIVE_FR",
    "INTELLIGENT_TIERING",
    "COLD_ARCHIVE",
    "ARCHIVE",
    "DEEP_COLD_ARCHIVE",
];
const TOS_OBJECT_ACL_VALUES: &[&str] = &[
    "private",
    "public-read",
    "public-read-write",
    "authenticated-read",
    "bucket-owner-read",
    "bucket-owner-full-control",
    "bucket-owner-entrusted",
    "default",
];

#[derive(Debug, Serialize)]
struct HighLevelPlan {
    command: String,
    dry_run: bool,
    execution_status: &'static str,
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    batch: BatchPlan,
    list_echo: ProgressPlan,
    progress: ProgressPlan,
    checkpoint: CheckpointPlan,
    report: ReportPlan,
    summary: PlanSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    impact: Option<Impact>,
    filters: BTreeMap<&'static str, String>,
    request_plan: Vec<RequestPlanStep>,
    samples: Vec<PlanSample>,
    consistency_guards: Vec<&'static str>,
    low_level_apis: Vec<&'static str>,
    plan: Vec<String>,
    warnings: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confirm_command: Option<String>,
}

#[derive(Debug, Serialize)]
struct BatchPlan {
    enabled: bool,
    source: &'static str,
    records_success_failure: bool,
}

#[derive(Debug, Serialize)]
struct ProgressPlan {
    enabled: bool,
    render_to: &'static str,
    disabled_reason: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct CheckpointPlan {
    enabled: bool,
    directory: String,
    identity: &'static str,
    lock: &'static str,
}

#[derive(Debug, Serialize)]
struct ReportPlan {
    path: String,
    format: &'static str,
}

#[derive(Debug, Serialize)]
struct PlanSummary {
    planned: u64,
    to_read: u64,
    to_write: u64,
    to_delete: u64,
    unknown_until_discovery: bool,
}

#[derive(Debug, Serialize)]
struct RequestPlanStep {
    phase: &'static str,
    api: &'static str,
    mutates: bool,
    requires_force: bool,
}

#[derive(Debug, Serialize)]
struct PlanSample {
    operation: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct BatchReport {
    summary: BatchSummary,
    succeeded: Vec<BatchItemResult>,
    failed: Vec<BatchItemResult>,
    skipped: Vec<BatchItemResult>,
}

#[derive(Debug, Default, Serialize)]
struct BatchSummary {
    planned: u64,
    succeeded: u64,
    failed: u64,
    skipped: u64,
}

type TosDeleteFuture<'a> = Pin<Box<dyn Future<Output = (String, Result<(), CliError>)> + 'a>>;
type TosObjectActionFuture<'a> = Pin<Box<dyn Future<Output = (String, Result<(), CliError>)> + 'a>>;
type TosCopyFuture<'a> =
    Pin<Box<dyn Future<Output = (String, String, u64, Result<CopyTransferResult, CliError>)> + 'a>>;
type TosMoveFuture<'a> = Pin<
    Box<
        dyn Future<
                Output = (
                    TransferPlanItem,
                    Result<CopyTransferResult, CliError>,
                    Option<Result<(), CliError>>,
                ),
            > + 'a,
    >,
>;

#[derive(Debug, Serialize)]
struct BatchItemResult {
    operation: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    retryable: bool,
}

impl BatchReport {
    fn new(planned: u64) -> Self {
        Self {
            summary: BatchSummary {
                planned,
                ..BatchSummary::default()
            },
            ..Self::default()
        }
    }

    fn record_success(&mut self, operation: &str, source: &str, destination: Option<&str>) {
        self.summary.succeeded += 1;
        self.succeeded
            .push(BatchItemResult::success(operation, source, destination));
    }

    fn record_failure(
        &mut self,
        operation: &str,
        source: &str,
        destination: Option<&str>,
        err: &CliError,
    ) {
        self.summary.failed += 1;
        self.failed.push(BatchItemResult::failure(
            operation,
            source,
            destination,
            err,
        ));
    }

    fn record_skipped(&mut self, operation: &str, source: &str, destination: Option<&str>) {
        self.summary.skipped += 1;
        self.skipped
            .push(BatchItemResult::skipped(operation, source, destination));
    }
}

impl BatchItemResult {
    fn success(operation: &str, source: &str, destination: Option<&str>) -> Self {
        Self::new(operation, source, destination, "succeeded", None)
    }

    fn skipped(operation: &str, source: &str, destination: Option<&str>) -> Self {
        Self::new(operation, source, destination, "skipped", None)
    }

    fn failure(operation: &str, source: &str, destination: Option<&str>, err: &CliError) -> Self {
        Self::new(operation, source, destination, "failed", Some(err))
    }

    fn new(
        operation: &str,
        source: &str,
        destination: Option<&str>,
        status: &'static str,
        err: Option<&CliError>,
    ) -> Self {
        Self {
            operation: operation.to_string(),
            source: source.to_string(),
            destination: destination.map(str::to_string),
            status,
            error_kind: err.map(error_kind),
            error_code: err.map(|err| err.exit_code().as_i32()),
            message: err.map(ToString::to_string),
            retryable: err.map(is_retryable_error).unwrap_or(false),
        }
    }
}

struct HighLevelOperation {
    command: &'static str,
    description: &'static str,
    risk: RiskLevel,
    target: String,
    source: Option<String>,
    destination: Option<String>,
    batch_enabled: bool,
    batch_source: &'static str,
    list_echo_requested: bool,
    list_echo_disabled: bool,
    progress_requested: bool,
    progress_disabled: bool,
    checkpoint_enabled: bool,
    checkpoint_dir: Option<String>,
    report_path: Option<String>,
    force: bool,
    requires_force: bool,
    consistency_guards: Vec<&'static str>,
    low_level_apis: Vec<&'static str>,
    confirm_command: Option<String>,
    parameters: Vec<CommandParameter>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ObjectWriteOptions {
    content_type: Option<String>,
    storage_class: Option<String>,
    acl: Option<String>,
    metadata: BTreeMap<String, String>,
}

impl ObjectWriteOptions {
    fn is_empty(&self) -> bool {
        self.content_type.is_none()
            && self.storage_class.is_none()
            && self.acl.is_none()
            && self.metadata.is_empty()
    }

    fn headers(&self, is_copy: bool) -> BTreeMap<String, String> {
        let mut headers = BTreeMap::new();
        if let Some(content_type) = &self.content_type {
            headers.insert("content-type".to_string(), content_type.clone());
        }
        if let Some(storage_class) = &self.storage_class {
            headers.insert("x-tos-storage-class".to_string(), storage_class.clone());
        }
        if let Some(acl) = &self.acl {
            headers.insert("x-tos-acl".to_string(), acl.clone());
        }
        if is_copy && !self.is_empty() {
            // [Review Fix #2] CopyObject must use the TOS SDK metadata
            // directive header even when only storage class/ACL changes.
            headers.insert(
                "x-tos-metadata-directive".to_string(),
                "REPLACE_NEW".to_string(),
            );
        }
        for (key, value) in &self.metadata {
            headers.insert(format!("x-tos-meta-{key}"), value.clone());
        }
        headers
    }

    fn checkpoint_fingerprint(&self) -> String {
        let mut hasher = DefaultHasher::new();
        self.content_type.hash(&mut hasher);
        self.storage_class.hash(&mut hasher);
        self.acl.hash(&mut hasher);
        self.metadata.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }
}

fn public_high_level_command_for_binary(binary: Binary, command: &str) -> String {
    // [Review Fix #1] Keep shared ve-tos routing on internal command ids while
    // rendering the logical user-facing TOS surface in envelopes and dry-runs.
    if binary == Binary::Tos {
        command
            .strip_prefix("ve-tos ")
            .map(|suffix| format!("tos {suffix}"))
            .unwrap_or_else(|| command.to_string())
    } else {
        command.to_string()
    }
}

fn public_high_level_command(command: &str) -> String {
    public_high_level_command_for_binary(active_tos_config_binary(), command)
}

fn high_level_success_envelope_for_binary<T: Serialize>(
    binary: Binary,
    command: &str,
    data: T,
) -> Envelope<T> {
    // [Review Fix #8] Real execution paths must pass through the same public
    // command mapping as dry-run/describe; otherwise `tos ls` leaks `ve-tos ls`.
    Envelope::success(public_high_level_command_for_binary(binary, command), data)
}

fn high_level_success_envelope<T: Serialize>(command: &str, data: T) -> Envelope<T> {
    high_level_success_envelope_for_binary(active_tos_config_binary(), command, data)
}

fn retag_success_envelope<T: Serialize>(
    mut envelope: Envelope<T>,
    command: impl Into<String>,
) -> Envelope<T> {
    // [Review Fix #2] High-level aliases such as mb/rb must report the user
    // command, not the lower-level bucket helper used internally.
    envelope.command = command.into();
    envelope
}

fn tos_put_success_payload(
    operation: &str,
    destination: &str,
    bytes: u64,
    response: &core::RawResponseData,
) -> Value {
    // [Review Fix #3] Successful high-level put output needs transfer metadata
    // so JSON users can consume it consistently with ve-adrive put.
    json!({
        "operation": operation,
        "destination": destination,
        "bytes": bytes,
        "etag": header_value(&response.headers, &["etag", "x-tos-etag"]),
        "crc64": header_value(&response.headers, &[
            "x-tos-hash-crc64ecma",
            "x-hash-crc64ecma",
            "x-tos-crc64",
        ]),
        "status": "succeeded",
        "response": response,
    })
}

#[derive(Clone)]
struct CopyOptions<'a> {
    overwrite_strategy: EffectiveOverwriteStrategy,
    report_path: Option<&'a str>,
    report_failures_only: bool,
    checkpoint_enabled: bool,
    checkpoint_dir: Option<&'a str>,
    checkpoint_threshold: u64,
    multipart_concurrency: usize,
    progress_granularity: EffectiveProgressGranularity,
    progress_enabled: bool,
    write_options: ObjectWriteOptions,
    // [Review Fix #Recursive-Summary] 当被批量上层（execute_cp_recursive
    // / execute_sync_recursive）驱动时设为 true：单文件 envelope 不再 output_result，
    // 由批量调用方在结尾统一汇总。--verbose 时上层会保持 false。
    silent_per_file: bool,
    // [Review Fix #Progress-Overall] 当批量上层提供 overall summary 时，本字段
    // 指向其只读引用：单文件传输路径将通过它推进字节进度并更新当前文件名 prefix，
    // 而不再启用 per-file FileProgress（避免双层进度条互相覆盖）。
    overall_bar: Option<&'a ProgressBar>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EffectiveProgressGranularity {
    Part,
    Byte,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EffectiveOverwriteStrategy {
    Force,
    NoClobber,
    Newer,
}

#[derive(Debug, Clone, Copy)]
enum FindSizeFilter {
    MinInclusive(u64),
    MaxInclusive(u64),
    Equal(u64),
}

#[derive(Debug, Clone)]
enum FindMtimeFilter {
    WithinLast(DateTime<Utc>),
    OlderThanOrEqual(DateTime<Utc>),
    EqualAge {
        newest: DateTime<Utc>,
        oldest_exclusive: DateTime<Utc>,
    },
}

#[derive(Debug, Clone)]
struct RelativeDuration {
    duration: chrono::Duration,
    unit: chrono::Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyOutcome {
    Transferred,
    Skipped,
}

#[derive(Debug, Clone)]
struct CopyTransferResult {
    // [Review Fix #2] Single-file high-level output must preserve the
    // underlying service response fields while batch callers still need a compact status.
    outcome: CopyOutcome,
    response_data: Option<Value>,
    request_id: Option<String>,
    status_code: Option<u16>,
    ec: Option<String>,
}

impl CopyTransferResult {
    fn skipped() -> Self {
        Self {
            outcome: CopyOutcome::Skipped,
            response_data: None,
            request_id: None,
            status_code: None,
            ec: None,
        }
    }

    fn from_raw_response(response: &Envelope<core::RawResponseData>) -> Result<Self, CliError> {
        Self::from_envelope(response)
    }

    fn from_envelope<T: Serialize>(response: &Envelope<T>) -> Result<Self, CliError> {
        let response_data = response
            .data
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(CliError::Json)?;
        let status_code = response
            .status_code
            .or_else(|| response_data.as_ref().and_then(status_code_from_payload));
        Ok(Self {
            outcome: CopyOutcome::Transferred,
            response_data,
            request_id: response.request_id.clone(),
            status_code,
            ec: response.ec.clone(),
        })
    }

    fn is_skipped(&self) -> bool {
        self.outcome == CopyOutcome::Skipped
    }
}

#[derive(Clone, Copy, Debug)]
struct TransferRuntimeConfig {
    checkpoint_threshold: u64,
    batch_concurrency: usize,
    list_concurrency: usize,
    multipart_concurrency: usize,
    progress_granularity: EffectiveProgressGranularity,
    overwrite_strategy: EffectiveOverwriteStrategy,
}

#[derive(Clone, Copy, Debug)]
struct TosRecursiveListOptions {
    use_hierarchical_listing: bool,
    list_concurrency: usize,
}

impl TransferRuntimeConfig {
    fn copy_options<'a>(
        self,
        report_path: Option<&'a str>,
        report_failures_only: bool,
        checkpoint_enabled: bool,
        checkpoint_dir: Option<&'a str>,
        write_options: ObjectWriteOptions,
        progress_enabled: bool,
        silent_per_file: bool,
        overall_bar: Option<&'a ProgressBar>,
    ) -> CopyOptions<'a> {
        CopyOptions {
            overwrite_strategy: self.overwrite_strategy,
            report_path,
            report_failures_only,
            checkpoint_enabled,
            checkpoint_dir,
            checkpoint_threshold: self.checkpoint_threshold,
            multipart_concurrency: self.multipart_concurrency,
            progress_granularity: self.progress_granularity,
            progress_enabled,
            write_options,
            silent_per_file,
            overall_bar,
        }
    }
}

fn object_write_options_from_parts(
    content_type: Option<&str>,
    storage_class: Option<&str>,
    acl: Option<&str>,
    meta: Option<&str>,
) -> Result<ObjectWriteOptions, CliError> {
    validate_optional_header_value("content-type", content_type)?;
    validate_optional_value(
        "storage-class",
        storage_class,
        TOS_OBJECT_STORAGE_CLASS_VALUES,
    )?;
    validate_optional_value("acl", acl, TOS_OBJECT_ACL_VALUES)?;
    Ok(ObjectWriteOptions {
        content_type: content_type.map(ToString::to_string),
        storage_class: storage_class.map(ToString::to_string),
        acl: acl.map(ToString::to_string),
        metadata: parse_tos_metadata(meta)?,
    })
}

fn copy_write_options(args: &CpArgs) -> Result<ObjectWriteOptions, CliError> {
    object_write_options_from_parts(
        args.content_type.as_deref(),
        args.storage_class.as_deref(),
        args.acl.as_deref(),
        args.meta.as_deref(),
    )
}

fn mv_write_options(args: &MvArgs) -> Result<ObjectWriteOptions, CliError> {
    object_write_options_from_parts(
        args.content_type.as_deref(),
        args.storage_class.as_deref(),
        args.acl.as_deref(),
        args.meta.as_deref(),
    )
}

fn sync_write_options(args: &SyncArgs) -> Result<ObjectWriteOptions, CliError> {
    object_write_options_from_parts(
        args.content_type.as_deref(),
        args.storage_class.as_deref(),
        args.acl.as_deref(),
        args.meta.as_deref(),
    )
}

fn put_write_options(args: &PutArgs) -> Result<ObjectWriteOptions, CliError> {
    object_write_options_from_parts(
        args.content_type.as_deref(),
        args.storage_class.as_deref(),
        args.acl.as_deref(),
        args.meta.as_deref(),
    )
}

fn ensure_tos_write_destination(
    command: &str,
    destination: &str,
    write_options: &ObjectWriteOptions,
) -> Result<(), CliError> {
    if !write_options.is_empty() && !destination.starts_with("tos://") {
        return Err(CliError::ValidationError(format!(
            "{}: --content-type, --storage-class, --acl, and --meta are only valid when the destination is tos://",
            command
        )));
    }
    Ok(())
}

/// Reject ByteTOS upload storage-class overrides while preserving ve-tos and TOS copy behavior.
pub(crate) fn ensure_tos_upload_storage_class_supported(
    command: &str,
    source: Option<&str>,
    destination: &str,
    storage_class: Option<&str>,
) -> Result<(), CliError> {
    ensure_tos_upload_storage_class_supported_for_binary(
        active_tos_config_binary(),
        command,
        source,
        destination,
        storage_class,
    )
}

fn ensure_tos_upload_storage_class_supported_for_binary(
    binary: Binary,
    command: &str,
    source: Option<&str>,
    destination: &str,
    storage_class: Option<&str>,
) -> Result<(), CliError> {
    if binary != Binary::Tos || storage_class.is_none() || !destination.starts_with("tos://") {
        return Ok(());
    }
    let is_upload = match source {
        Some(source) => !source.starts_with("tos://"),
        None => true,
    };
    if !is_upload {
        return Ok(());
    }
    Err(CliError::ValidationError(format!(
        "{}: ByteTOS upload does not support --storage-class; PutObject/CreateMultipartUpload ignores x-tos-storage-class for object creation. Remove --storage-class or use a bucket default/lifecycle policy instead.",
        command
    )))
}

fn validate_optional_value(
    name: &str,
    value: Option<&str>,
    allowed: &[&str],
) -> Result<(), CliError> {
    let Some(value) = value else {
        return Ok(());
    };
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(CliError::ValidationError(format!(
        "invalid --{} '{}': expected one of {}",
        name,
        value,
        allowed.join(", ")
    )))
}

fn parse_tos_metadata(meta: Option<&str>) -> Result<BTreeMap<String, String>, CliError> {
    let Some(meta) = meta else {
        return Ok(BTreeMap::new());
    };
    if meta.trim().is_empty() {
        return Err(CliError::ValidationError(
            "invalid --meta: expected key=value pairs separated by '#'".to_string(),
        ));
    }
    let mut metadata = BTreeMap::new();
    for pair in meta.split('#') {
        let (key, value) = pair.split_once('=').ok_or_else(|| {
            CliError::ValidationError(format!(
                "invalid --meta pair '{}': expected key=value",
                pair
            ))
        })?;
        let key = normalize_tos_metadata_key(key)?;
        validate_tos_metadata_value(value)?;
        if metadata.insert(key.clone(), value.to_string()).is_some() {
            return Err(CliError::ValidationError(format!(
                "invalid --meta: duplicate metadata key '{}'",
                key
            )));
        }
    }
    Ok(metadata)
}

fn normalize_tos_metadata_key(key: &str) -> Result<String, CliError> {
    let key = key.trim();
    let key = key.strip_prefix("x-tos-meta-").unwrap_or(key);
    if key.is_empty() {
        return Err(CliError::ValidationError(
            "invalid --meta: metadata key must not be empty".to_string(),
        ));
    }
    if !key.chars().all(is_http_token_char) {
        return Err(CliError::ValidationError(format!(
            "invalid --meta key '{}': metadata keys must be valid HTTP token characters",
            key
        )));
    }
    Ok(key.to_string())
}

fn is_http_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            '!' | '$' | '%' | '&' | '\'' | '*' | '+' | '-' | '.' | '^' | '_' | '`' | '|' | '~'
        )
}

fn validate_tos_metadata_value(value: &str) -> Result<(), CliError> {
    if value.chars().any(char::is_control) {
        return Err(CliError::ValidationError(
            "invalid --meta: metadata values must not contain control characters".to_string(),
        ));
    }
    Ok(())
}

fn validate_optional_header_value(name: &str, value: Option<&str>) -> Result<(), CliError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(CliError::ValidationError(format!(
            "invalid --{}: header values must be non-empty and must not contain control characters",
            name
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct ObjectEntry {
    key: String,
    size: u64,
    last_modified: Option<String>,
    etag: Option<String>,
    storage_class: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct LsEntry {
    key: String,
    entry_type: &'static str,
    size: u64,
    last_modified: Option<String>,
    etag: Option<String>,
    storage_class: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct DuDistributionBucket {
    count: u64,
    bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
struct DuObjectSample {
    key: String,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    storage_class: Option<String>,
    #[serde(skip_serializing)]
    timestamp_millis: Option<i64>,
}

#[derive(Debug, Clone)]
struct DuAccumulator {
    object_count: u64,
    total_bytes: u64,
    directory_count: u64,
    directory_prefixes: HashSet<String>,
    request_ids: Vec<String>,
    request_ids_omitted: u64,
    capture_manifest: bool,
    manifest_items: Vec<ListManifestItem>,
    file_types: BTreeMap<String, DuDistributionBucket>,
    directories: BTreeMap<String, DuDistributionBucket>,
    size_histogram: BTreeMap<&'static str, DuDistributionBucket>,
    storage_classes: BTreeMap<String, DuDistributionBucket>,
    largest_objects: Vec<DuObjectSample>,
    oldest_objects: Vec<DuObjectSample>,
    top_k: usize,
}

#[derive(Debug, Clone, Serialize)]
struct TransferManifestItem {
    operation: &'static str,
    relative_key: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crc64: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct TransferManifest {
    object_count: u64,
    total_size: u64,
    items: Vec<TransferManifestItem>,
}

impl TransferManifest {
    fn from_items(items: Vec<TransferManifestItem>) -> Self {
        Self {
            object_count: items.len() as u64,
            total_size: items.iter().map(|item| item.size).sum(),
            items,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ListManifestItem {
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    relative_key: Option<String>,
    item_type: &'static str,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    storage_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ListManifest {
    item_count: u64,
    object_count: u64,
    directory_count: u64,
    total_size: u64,
    items: Vec<ListManifestItem>,
}

#[derive(Debug, Clone)]
struct TransferPlanItem {
    relative_key: String,
    source: String,
    destination: String,
    size: u64,
    etag: Option<String>,
    crc64: Option<u64>,
    last_modified: Option<String>,
}

impl TransferPlanItem {
    fn manifest_item(&self) -> TransferManifestItem {
        TransferManifestItem {
            operation: "copy",
            relative_key: self.relative_key.clone(),
            source: self.source.clone(),
            destination: Some(self.destination.clone()),
            size: self.size,
            etag: self.etag.clone(),
            crc64: self.crc64,
            last_modified: self.last_modified.clone(),
        }
    }
}

fn build_transfer_manifest(items: &[TransferPlanItem]) -> TransferManifest {
    TransferManifest::from_items(items.iter().map(TransferPlanItem::manifest_item).collect())
}

fn build_move_transfer_manifest(items: &[TransferPlanItem]) -> TransferManifest {
    let mut manifest_items = items
        .iter()
        .map(TransferPlanItem::manifest_item)
        .collect::<Vec<_>>();
    // [Review Fix #MoveManifest] Recursive move has two planned stages. The
    // manifest records both so the later report can be reconciled item-for-item.
    manifest_items.extend(items.iter().map(|item| TransferManifestItem {
        operation: "delete-source",
        relative_key: item.relative_key.clone(),
        source: item.source.clone(),
        destination: None,
        size: item.size,
        etag: item.etag.clone(),
        crc64: item.crc64,
        last_modified: item.last_modified.clone(),
    }));
    TransferManifest::from_items(manifest_items)
}

/// Handle High-Level TOS commands.
///
/// The handler keeps describe/dry-run paths side-effect free, and routes real
/// execution through the same signed HTTP primitives as Low-Level commands.
pub async fn handle_high_level_command(
    global: &GlobalArgs,
    command: &TosCommand,
) -> Result<i32, CliError> {
    if global.describe {
        // [Review Fix #RebaseDescribe] --describe must be parameter-independent;
        // do not build the executable operation before registry metadata lookup.
        let mut desc = describe_command_metadata(tos_high_level_command_path(command))
            .or_else(|| {
                build_operation(command)
                    .ok()
                    .map(|operation| describe_operation(&operation))
            })
            .ok_or_else(|| {
                CliError::ValidationError("unsupported high-level command".to_string())
            })?;
        desc.command = public_high_level_command(&desc.command);
        output_result(global, &Envelope::success(desc.command.clone(), desc))?;
        return Ok(0);
    }

    let operation = build_operation(command)?;
    let path_traversal_confirm_target = command_path_traversal_confirm_target(command, &operation)?;

    if global.dry_run {
        let plan = build_plan(global, &operation, path_traversal_confirm_target.as_deref()).await?;
        output_result(global, &Envelope::success(plan.command.clone(), plan))?;
        return Ok(0);
    }

    // [Review Fix #3] Dry-run is the safe planning path and must not require --force.
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let stderr_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let can_prompt = stdin_tty && stderr_tty && !global.quiet;
    let guard_force =
        operation.force || !operation.requires_force || (global.yes && can_prompt) || can_prompt;
    if let Err(violation) = enforce_registry_guards(operation.command, guard_force, can_prompt) {
        return Err(CliError::ValidationError(violation.to_string()));
    }

    if let Some(confirm_target) = path_traversal_confirm_target.as_deref() {
        enforce_path_traversal_confirmation(global, &operation, confirm_target, can_prompt)?;
    }

    // [G2] Critical-risk operations require explicit out-of-band confirmation
    // beyond `--force`. This prevents an Agent from silently destroying data
    // when a single flag flip would otherwise suffice.
    if matches!(operation.risk, RiskLevel::Critical) {
        enforce_critical_confirmation(global, &operation, can_prompt)?;
    }

    if operation.requires_force && !operation.force {
        ensure_force_for_destructive(global, false, operation.command, &operation.target)?;
    }

    execute_high_level_command(global, command).await
}

async fn execute_high_level_command(
    global: &GlobalArgs,
    command: &TosCommand,
) -> Result<i32, CliError> {
    if let TosCommand::Cp(args) = command {
        if !args.source.starts_with("tos://") && !args.destination.starts_with("tos://") {
            let runtime = effective_cp_runtime_config(global, args)?;
            let progress_enabled =
                effective_progress_enabled(global, args.progress, args.no_progress)?;
            if args.recursive {
                let report_path =
                    effective_report_path(global, args.report_path.as_deref(), "ve-tos cp")?;
                let manifest_path = effective_optional_manifest_path(
                    global,
                    args.manifest_path.as_deref(),
                    args.no_manifest,
                    "ve-tos cp",
                )?;
                return execute_cp_recursive_local(
                    global,
                    args,
                    report_path.as_deref(),
                    manifest_path.as_deref(),
                );
            } else {
                reject_single_transfer_artifacts(
                    "ve-tos cp",
                    args.report_path.as_deref(),
                    args.report_failures_only,
                    args.manifest_path.as_deref(),
                    args.no_manifest,
                    args.batch_concurrency,
                    args.list_concurrency,
                )?;
                let destination =
                    resolve_single_transfer_destination(&args.source, &args.destination)?;
                copy_local_to_local(
                    global,
                    &args.source,
                    &destination,
                    runtime.copy_options(
                        None,
                        false,
                        args.checkpoint,
                        args.checkpoint_dir.as_deref(),
                        ObjectWriteOptions::default(),
                        progress_enabled,
                        false,
                        None,
                    ),
                )?;
            }
            return Ok(0);
        }
    }
    if let TosCommand::Mv(args) = command {
        if !args.source.starts_with("tos://") && !args.destination.starts_with("tos://") {
            if args.recursive {
                let report_path =
                    effective_report_path(global, args.report_path.as_deref(), "ve-tos mv")?;
                let manifest_path = effective_optional_manifest_path(
                    global,
                    args.manifest_path.as_deref(),
                    args.no_manifest,
                    "ve-tos mv",
                )?;
                return execute_mv_recursive_local(
                    global,
                    args,
                    report_path.as_deref(),
                    manifest_path.as_deref(),
                );
            } else {
                reject_single_transfer_artifacts(
                    "ve-tos mv",
                    args.report_path.as_deref(),
                    args.report_failures_only,
                    args.manifest_path.as_deref(),
                    args.no_manifest,
                    args.batch_concurrency,
                    args.list_concurrency,
                )?;
                let progress_enabled =
                    effective_progress_enabled(global, args.progress, args.no_progress)?;
                let mut runtime = effective_default_runtime_config(global)?;
                runtime.overwrite_strategy = EffectiveOverwriteStrategy::Force;
                let destination =
                    resolve_single_transfer_destination(&args.source, &args.destination)?;
                let outcome = copy_local_to_local(
                    global,
                    &args.source,
                    &destination,
                    runtime.copy_options(
                        None,
                        false,
                        false,
                        args.checkpoint_dir.as_deref(),
                        ObjectWriteOptions::default(),
                        progress_enabled,
                        false,
                        None,
                    ),
                )?;
                if outcome.is_skipped() {
                    return Err(CliError::Conflict(
                        "ve-tos mv copy was skipped; source was not deleted".to_string(),
                    ));
                }
                fs::remove_file(&args.source)?;
            }
            return Ok(0);
        }
    }
    if let TosCommand::Sync(args) = command {
        if !args.source.starts_with("tos://") && !args.destination.starts_with("tos://") {
            return execute_sync_local_to_local(global, args);
        }
    }

    let profile = build_profile(global)?;
    let client = TosClient::new(&profile, "tos")?;

    match command {
        TosCommand::Cp(args) => execute_cp(global, &client, args).await,
        TosCommand::Mv(args) => execute_mv(global, &client, args).await,
        TosCommand::Sync(args) => execute_sync(global, &client, args).await,
        TosCommand::Mb(args) => execute_mb(global, &client, args).await,
        TosCommand::Rb(args) => execute_rb(global, &client, args).await,
        TosCommand::Mkdir(args) => execute_mkdir(global, &client, args).await,
        TosCommand::Rm(args) => execute_rm(global, &client, args).await,
        TosCommand::Ls(args) => execute_ls(global, &client, args).await,
        TosCommand::Stat(args) => execute_stat(global, &client, args).await,
        TosCommand::Du(args) => execute_du(global, &client, args).await,
        TosCommand::Find(args) => execute_find(global, &client, args).await,
        TosCommand::Cat(args) => execute_cat(global, &client, args).await,
        TosCommand::Put(args) => execute_put(global, &client, args).await,
        TosCommand::Presign(args) => execute_presign(global, &client, args).await,
        TosCommand::Restore(args) => execute_restore(global, &client, args).await,
        _ => Err(CliError::ValidationError(
            "unsupported high-level command".to_string(),
        )),
    }
}

async fn execute_mb(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MbArgs,
) -> Result<i32, CliError> {
    validate_optional_value("bucket-type", args.bucket_type.as_deref(), &["fns", "hns"])?;
    let override_client;
    let effective_client = if let Some(region) = &args.region {
        let mut profile = build_profile(global)?;
        // [Review Fix #MbRegion] Keep high-level `mb --region` aligned with
        // low-level `ve-tos bucket create --region`; it must affect the request
        // client, not only appear in help/describe output.
        profile.region = Some(region.clone());
        override_client = Some(TosClient::new(&profile, "tos")?);
        override_client.as_ref().unwrap()
    } else {
        client
    };
    let req = bucket::CreateBucketRequest {
        bucket: parse_bucket_target(&args.bucket)?,
        storage_class: (args.storage_class != "STANDARD").then(|| args.storage_class.clone()),
        acl: args.acl.clone(),
        grant_full_control: None,
        grant_read: None,
        grant_read_non_list: None,
        grant_read_acp: None,
        grant_write: None,
        grant_write_acp: None,
        az_redundancy: args.az_redundancy.clone(),
        bucket_type: args.bucket_type.clone(),
        bucket_object_lock_enabled: args.bucket_object_lock_enabled.then_some(true),
        tagging: None,
        project_name: None,
    };
    let created = bucket::create_bucket(effective_client, &req).await?;
    let created = retag_success_envelope(created, public_high_level_command("ve-tos mb"));
    output_result(global, &created)?;
    Ok(0)
}

async fn execute_rb(
    global: &GlobalArgs,
    client: &TosClient,
    args: &RbArgs,
) -> Result<i32, CliError> {
    let bucket_name = parse_bucket_target(&args.bucket)?;
    let deleted = bucket::delete_bucket(client, &bucket_name).await?;
    let deleted = retag_success_envelope(deleted, public_high_level_command("ve-tos rb"));
    output_result(global, &deleted)?;
    Ok(0)
}

async fn execute_mkdir(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MkdirArgs,
) -> Result<i32, CliError> {
    let target = resolve_mkdir_target(args)?;
    let bucket = target.bucket;
    let key = target.key.ok_or_else(|| {
        CliError::ValidationError("ve-tos mkdir requires tos://bucket/folder".to_string())
    })?;
    let keys = folder_keys_for_mkdir(&key, args.parents);
    let mut created = Vec::new();
    let mut request_id = None;
    let mut last_response = None;
    for folder_key in &keys {
        let raw = core::execute_object_request(
            client,
            "ve-tos mkdir",
            Method::PUT,
            &bucket,
            folder_key,
            BTreeMap::new(),
            BTreeMap::from([(
                "content-type".to_string(),
                "application/x-directory".to_string(),
            )]),
            Some(Vec::new()),
        )
        .await?;
        request_id = raw.request_id;
        last_response = raw.data;
        created.push(format!("tos://{}/{}", bucket, folder_key));
    }
    let path = format!("tos://{}/{}", bucket, key);
    let created_count = created.len();
    let response = json!({
        "bucket": bucket,
        "key": key,
        "path": path,
        "parents": args.parents,
        "created": created,
        "created_count": created_count,
        "status": "succeeded",
        "response": last_response,
    });
    let envelope = high_level_success_envelope("ve-tos mkdir", response);
    let envelope = if let Some(request_id) = request_id {
        envelope.with_request_id(request_id)
    } else {
        envelope
    };
    output_result(global, &envelope)?;
    Ok(0)
}

async fn execute_ls(
    global: &GlobalArgs,
    client: &TosClient,
    args: &LsArgs,
) -> Result<i32, CliError> {
    let manifest_path = effective_explicit_manifest_path(args.manifest_path.as_deref());
    validate_high_level_ls_max_keys(args.max_keys)?;
    // [Review Fix #LsDualMode] Support both positional URI and --bucket/--key flags.
    let resolved_path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos ls",
    );
    if let Ok(path) = resolved_path {
        let target = parse_tos_uri(&path, true)?;
        let (objects, prefixes, next_token) = list_object_entries_with_prefixes_limited(
            client,
            &target.bucket,
            target.key.as_deref(),
            args.max_keys,
            args.continuation_token.as_deref(),
        )
        .await?;
        let (objects, prefixes) =
            dedupe_ls_objects_and_prefixes(objects, prefixes, target.key.as_deref());
        let mut entries = merge_ls_entries(objects.clone(), prefixes.clone());
        sort_ls_entries(&mut entries, args.sort.as_deref())?;
        let is_tabular_output = uses_tabular_output(global);
        // [Review Fix #LsTable] Use output_result_with_columns so table/csv views
        // correctly pick the objects array and display declared columns.
        // [Review Fix #2] Apply --columns consistently across object and bucket ls scopes.
        let columns = parse_ls_columns(args.columns.as_deref(), LS_OBJECT_TABLE_COLUMNS);
        let total = entries.len() as u64;
        let manifest_items = objects
            .iter()
            .map(|entry| object_entry_manifest_item(&target.bucket, target.key.as_deref(), entry))
            .chain(prefixes.iter().map(|prefix| {
                directory_manifest_item(&target.bucket, target.key.as_deref(), prefix)
            }))
            .collect::<Vec<_>>();
        let manifest = build_list_manifest(manifest_items);
        write_list_manifest_file(manifest_path.as_deref(), "ve-tos ls", &manifest)?;
        let payload = if is_tabular_output {
            json!({
                "bucket": target.bucket,
                "prefix": target.key,
                "entries": ls_entries_for_output(&entries, args.human_readable),
                "objects": objects,
                "common_prefixes": prefixes,
                "next_continuation_token": next_token.clone(),
                "human_readable": args.human_readable,
                "manifest_path": manifest_path,
            })
        } else {
            json!({
                "bucket": target.bucket,
                "prefix": target.key,
                "objects": objects,
                "common_prefixes": prefixes,
                "next_continuation_token": next_token.clone(),
                "human_readable": args.human_readable,
                "manifest_path": manifest_path,
            })
        };
        output_result_with_columns(
            global,
            &high_level_success_envelope("ve-tos ls", payload).with_pagination(PaginationInfo {
                next_token,
                next_marker: None,
                total_returned: total,
            }),
            Some(columns),
        )?;
    } else {
        // [Review Fix #LsTable] No path -> list buckets. Mirror `ve-tos bucket list`'s
        // declared column order (name/location/creation_date) so the table view is
        // consistent across both entry points instead of falling back to the
        // alphabetical JSON-key order that pushed `name` to the last column.
        let bucket_envelope = bucket::list_buckets(client, None, None).await?;
        let buckets = bucket_envelope
            .data
            .as_ref()
            .map(|data| data.buckets.as_slice())
            .unwrap_or_default();
        let buckets = limit_bucket_listing_for_ls(buckets, args.max_keys);
        let manifest = build_list_manifest(buckets.iter().map(bucket_manifest_item).collect());
        write_list_manifest_file(manifest_path.as_deref(), "ve-tos ls", &manifest)?;
        let mut payload = bucket_envelope
            .data
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(CliError::Json)?
            .unwrap_or_else(|| json!({"buckets": []}));
        if let Value::Object(map) = &mut payload {
            map.insert("buckets".to_string(), json!(buckets));
            map.insert(
                "client_side_truncated".to_string(),
                json!(
                    buckets.len()
                        < bucket_envelope
                            .data
                            .as_ref()
                            .map(|data| data.buckets.len())
                            .unwrap_or_default()
                ),
            );
            map.insert("manifest_path".to_string(), json!(manifest_path));
        }
        let mut envelope =
            high_level_success_envelope("ve-tos ls", payload).with_pagination(PaginationInfo {
                next_token: None,
                next_marker: None,
                total_returned: buckets.len() as u64,
            });
        if let Some(request_id) = bucket_envelope.request_id {
            envelope = envelope.with_request_id(request_id);
        }
        // [Review Fix #2] Root bucket listing must honor the same --columns flag.
        let columns = parse_ls_columns(args.columns.as_deref(), BUCKET_LS_TABLE_COLUMNS);
        output_result_with_columns(global, &envelope, Some(columns))?;
    }
    Ok(0)
}

/// Column order for `ve-tos ls` when listing buckets (no path). Kept in sync with
/// `ve-tos bucket list`'s `BUCKET_LIST_TABLE_COLUMNS`.
const BUCKET_LS_TABLE_COLUMNS: &[&str] = &["name", "location", "bucket_type", "creation_date"];

fn limit_bucket_listing_for_ls(
    buckets: &[bucket::BucketInfo],
    max_keys: u32,
) -> &[bucket::BucketInfo] {
    let requested = usize::try_from(max_keys).unwrap_or(usize::MAX);
    &buckets[..buckets.len().min(requested)]
}

fn parse_ls_columns(
    columns: Option<&str>,
    default_columns: &'static [&'static str],
) -> &'static [&'static str] {
    let Some(columns) = columns.map(str::trim).filter(|columns| !columns.is_empty()) else {
        return default_columns;
    };
    let leaked = Box::leak(columns.to_string().into_boxed_str());
    let values = leaked
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        default_columns
    } else {
        Box::leak(values.into_boxed_slice())
    }
}

async fn execute_stat(
    global: &GlobalArgs,
    client: &TosClient,
    args: &StatArgs,
) -> Result<i32, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos stat",
    )?;
    let target = parse_tos_uri(&path, true)?;
    if let Some(key) = target.key {
        let query = args
            .version_id
            .clone()
            .map(|version_id| BTreeMap::from([("versionId".to_string(), version_id)]))
            .unwrap_or_default();
        output_result(
            global,
            &core::execute_object_request(
                client,
                "ve-tos stat",
                Method::HEAD,
                &target.bucket,
                &key,
                query,
                BTreeMap::new(),
                None,
            )
            .await?,
        )?;
    } else {
        output_result(global, &bucket::head_bucket(client, &target.bucket).await?)?;
    }
    Ok(0)
}

async fn execute_rm(
    global: &GlobalArgs,
    client: &TosClient,
    args: &RmArgs,
) -> Result<i32, CliError> {
    let path = resolve_rm_path(args)?;
    if args.recursive {
        let report_path = effective_report_path(global, args.report_path.as_deref(), "ve-tos rm")?;
        let manifest_path = effective_optional_manifest_path(
            global,
            args.manifest_path.as_deref(),
            args.no_manifest,
            "ve-tos rm",
        )?;
        let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
        let list_echo_enabled =
            effective_list_echo_enabled(global, args.list_echo, args.no_list_echo);
        let batch_concurrency = effective_batch_concurrency(global, args.batch_concurrency)?;
        let list_concurrency = effective_list_concurrency(global, args.list_concurrency)?;
        let mut target = parse_tos_uri(&path, true)?;
        normalize_recursive_tos_target(&mut target);
        let prefix = target.key.as_deref();
        let mut scan_progress = RemoteScanProgress::new(list_echo_enabled, "ve-tos rm", &path);
        let is_hns_bucket = bucket_is_hns(client, &target.bucket).await?;
        let list_options = TosRecursiveListOptions {
            use_hierarchical_listing: resolve_tos_recursive_list_mode(
                is_hns_bucket,
                args.recursive_list_mode,
            ),
            list_concurrency,
        };
        let delete_mode = resolve_tos_recursive_delete_mode(is_hns_bucket, args)?;
        if delete_mode == Some(RecursiveDeleteMode::Direct) {
            if args.all_versions {
                return Err(CliError::ValidationError(
                    "ve-tos rm --recursive-delete-mode direct cannot be combined with --all-versions"
                        .to_string(),
                ));
            }
            scan_progress.finish_and_clear();
            return execute_rm_hns_direct(global, client, args, &target, &path, report_path).await;
        }
        if args.all_versions {
            let mut versions = list_object_versions_recursive(
                client,
                &target.bucket,
                prefix,
                list_options.use_hierarchical_listing,
                list_options.list_concurrency,
            )
            .await?;
            scan_progress.finish_with_count(versions.len() as u64, "version(s)");
            if delete_mode == Some(RecursiveDeleteMode::BottomUp) {
                sort_versions_bottom_up(&mut versions);
            }
            let total_versions = versions.len() as u64;
            let mut report = BatchReport::new(total_versions);
            let bar = if progress_enabled && total_versions > 0 {
                let pb = ProgressBar::new(total_versions);
                pb.set_style(
                    ProgressStyle::with_template(
                        "ve-tos rm --all-versions [{bar:30.red/blue}] {pos}/{len} versions ({per_sec}, ETA {eta})",
                    )
                        .unwrap_or_else(|_| ProgressStyle::default_bar())
                        .progress_chars("=>-"),
                );
                pb.enable_steady_tick(Duration::from_millis(200));
                Some(pb)
            } else {
                None
            };
            let mut versions_to_delete = Vec::new();
            for v in versions {
                if !pattern_allows(&v.key, args.include.as_deref(), args.exclude.as_deref()) {
                    if let Some(b) = &bar {
                        b.inc(1);
                    }
                    report.record_skipped(
                        "delete",
                        &format!(
                            "tos://{}/{}?versionId={}",
                            target.bucket, v.key, v.version_id
                        ),
                        None,
                    );
                    continue;
                }
                versions_to_delete.push(v);
            }
            let manifest = build_list_manifest(
                versions_to_delete
                    .iter()
                    .map(|version| version_manifest_item(&target.bucket, prefix, version))
                    .collect(),
            );
            write_list_manifest_file(
                manifest_path.as_deref(),
                "ve-tos rm all-versions",
                &manifest,
            )?;
            // [Review Fix #Tos-RmAllVersionsConcurrency] `--batch-concurrency`
            // controls version deletes too. Bottom-up mode keeps depth groups
            // sequential while allowing concurrency within the same depth.
            delete_tos_versions_with_concurrency(
                client,
                &target.bucket,
                versions_to_delete,
                delete_mode == Some(RecursiveDeleteMode::BottomUp),
                batch_concurrency,
                &bar,
                report_path.as_deref(),
                args.report_failures_only,
                &mut report,
            )
            .await?;
            if let Some(b) = bar {
                b.finish();
            }
            let failed_count = report.summary.failed;
            write_tos_batch_report(
                report_path.as_deref(),
                "ve-tos rm all-versions",
                &report,
                args.report_failures_only,
            )?;
            output_result(
                global,
                &Envelope::success(
                    "ve-tos rm all-versions",
                    json!({
                        "path": &path,
                        "status": if failed_count == 0 { "succeeded" } else { "partial_failure" },
                        "summary": &report.summary,
                        "report_path": &report_path,
                        "manifest_path": &manifest_path,
                    }),
                ),
            )?;
            // [Review Fix #BatchExitCode] 批量 summary 已经输出，失败项用
            // exit code=1 表达，避免顶层错误处理再打印一份 JSON。
            return if failed_count == 0 { Ok(0) } else { Ok(1) };
        }
        if args.no_manifest {
            scan_progress.finish_and_clear();
            return execute_rm_streaming_no_manifest(
                global,
                client,
                args,
                &target,
                &path,
                prefix,
                delete_mode,
                is_hns_bucket,
                batch_concurrency,
                list_options,
                progress_enabled,
                report_path,
            )
            .await;
        }
        let mut keys =
            list_object_keys_recursive(client, &target.bucket, prefix, list_options).await?;
        scan_progress.finish_with_count(keys.len() as u64, "object(s)");
        if delete_mode == Some(RecursiveDeleteMode::BottomUp) {
            if let Some(key) = prefix {
                push_unique_key(&mut keys, key);
            }
            sort_keys_bottom_up(&mut keys);
        }
        let total_keys = keys.len() as u64;
        let mut report = BatchReport::new(total_keys);
        let bar = if progress_enabled && total_keys > 0 {
            let pb = ProgressBar::new(total_keys);
            pb.set_style(
                ProgressStyle::with_template(
                    "ve-tos rm [{bar:30.red/blue}] {pos}/{len} files ({per_sec}, ETA {eta})",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=>-"),
            );
            pb.enable_steady_tick(Duration::from_millis(200));
            Some(pb)
        } else {
            None
        };
        let keys_to_delete = keys
            .iter()
            .filter(|key| pattern_allows(key, args.include.as_deref(), args.exclude.as_deref()))
            .map(|key| key_manifest_item(&target.bucket, prefix, key))
            .collect::<Vec<_>>();
        let manifest = build_list_manifest(keys_to_delete);
        write_list_manifest_file(manifest_path.as_deref(), "ve-tos rm", &manifest)?;
        delete_tos_keys_for_rm(
            client,
            &target,
            keys,
            args,
            delete_mode == Some(RecursiveDeleteMode::BottomUp),
            batch_concurrency,
            false,
            &bar,
            report_path.as_deref(),
            &mut report,
        )
        .await?;
        if let Some(b) = bar {
            b.finish();
        }

        if args.include_uploads {
            abort_multipart_uploads_for_rm(
                client,
                &target,
                prefix,
                progress_enabled,
                report_path.as_deref(),
                args.report_failures_only,
                &mut report,
            )
            .await?;
        }

        let failed_count = report.summary.failed;
        write_tos_batch_report(
            report_path.as_deref(),
            "ve-tos rm",
            &report,
            args.report_failures_only,
        )?;
        output_result(
            global,
            &Envelope::success(
                "ve-tos rm recursive",
                json!({
                    "path": &path,
                    "status": if failed_count == 0 { "succeeded" } else { "partial_failure" },
                    "summary": &report.summary,
                    "report_path": &report_path,
                    "manifest_path": &manifest_path,
                }),
            ),
        )?;
        // [Review Fix #BatchExitCode] 批量 summary 已经输出，失败项用
        // exit code=1 表达，避免顶层错误处理再打印一份 JSON。
        return if failed_count == 0 { Ok(0) } else { Ok(1) };
    }
    reject_single_transfer_artifacts(
        "ve-tos rm",
        args.report_path.as_deref(),
        args.report_failures_only,
        args.manifest_path.as_deref(),
        args.no_manifest,
        args.batch_concurrency,
        args.list_concurrency,
    )?;
    let target = parse_tos_uri(&path, false)?;
    let key = target.key.expect("validated object key");
    output_result(
        global,
        &core::execute_object_request(
            client,
            "ve-tos rm",
            Method::DELETE,
            &target.bucket,
            &key,
            BTreeMap::new(),
            BTreeMap::new(),
            None,
        )
        .await?,
    )?;
    Ok(0)
}

async fn execute_rm_streaming_no_manifest(
    global: &GlobalArgs,
    client: &TosClient,
    args: &RmArgs,
    target: &ParsedTosUri,
    path: &str,
    prefix: Option<&str>,
    delete_mode: Option<RecursiveDeleteMode>,
    is_hns_bucket: bool,
    batch_concurrency: usize,
    list_options: TosRecursiveListOptions,
    progress_enabled: bool,
    report_path: Option<String>,
) -> Result<i32, CliError> {
    let progress = streaming_batch_progress(progress_enabled, "ve-tos rm");
    let mut report = BatchReport::new(0);
    let stream_result = if delete_mode == Some(RecursiveDeleteMode::BottomUp) && is_hns_bucket {
        stream_delete_tos_hns_bottom_up(
            client,
            target,
            prefix,
            args,
            batch_concurrency,
            list_options,
            &progress,
            report_path.as_deref(),
            &mut report,
        )
        .await
    } else if list_options.use_hierarchical_listing {
        stream_delete_tos_hierarchical_pages(
            client,
            target,
            prefix,
            args,
            batch_concurrency,
            list_options.list_concurrency,
            &progress,
            report_path.as_deref(),
            &mut report,
        )
        .await
    } else {
        stream_delete_tos_flat_pages(
            client,
            target,
            prefix,
            args,
            batch_concurrency,
            &progress,
            report_path.as_deref(),
            &mut report,
        )
        .await
    };
    finish_streaming_progress(progress, report.summary.planned);
    let upload_result = if stream_result.is_ok() && args.include_uploads {
        abort_multipart_uploads_for_rm(
            client,
            target,
            prefix,
            progress_enabled,
            report_path.as_deref(),
            args.report_failures_only,
            &mut report,
        )
        .await
    } else {
        Ok(())
    };
    let failed_count = report.summary.failed;
    write_tos_batch_report(
        report_path.as_deref(),
        "ve-tos rm",
        &report,
        args.report_failures_only,
    )?;
    stream_result?;
    upload_result?;
    output_result(
        global,
        &Envelope::success(
            "ve-tos rm recursive",
            json!({
                "path": path,
                "status": if failed_count == 0 { "succeeded" } else { "partial_failure" },
                "summary": &report.summary,
                "report_path": &report_path,
                "manifest_path": Value::Null,
            }),
        ),
    )?;
    // [Review Fix #BatchExitCode] 批量 summary 已经输出，失败项用
    // exit code=1 表达，避免顶层错误处理再打印一份 JSON。
    if failed_count == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

async fn stream_delete_tos_flat_pages(
    client: &TosClient,
    target: &ParsedTosUri,
    prefix: Option<&str>,
    args: &RmArgs,
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let mut continuation_token = None;
    let mut in_flight: FuturesUnordered<TosDeleteFuture<'_>> = FuturesUnordered::new();
    let limit = batch_concurrency.max(1);
    let stream_result = loop {
        let page = match list_object_entries_page(
            client,
            &target.bucket,
            prefix,
            None,
            continuation_token.as_deref(),
        )
        .await
        {
            Ok(page) => page,
            Err(err) => break Err(err),
        };
        continuation_token = page.next_token.clone();
        for entry in page.entries {
            queue_tos_stream_delete(
                client,
                target,
                entry.key,
                args,
                progress,
                report_path,
                report,
                &mut in_flight,
                limit,
            )
            .await?;
        }
        if !page.is_truncated {
            break Ok(());
        }
    };
    while let Some((uri, result)) = in_flight.next().await {
        record_tos_stream_delete_result(
            progress,
            report_path,
            args.report_failures_only,
            report,
            uri,
            result,
        )?;
    }
    stream_result
}

async fn queue_tos_stream_delete<'a>(
    client: &'a TosClient,
    target: &ParsedTosUri,
    key: String,
    args: &RmArgs,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report: &mut BatchReport,
    in_flight: &mut FuturesUnordered<TosDeleteFuture<'a>>,
    limit: usize,
) -> Result<(), CliError> {
    report.summary.planned += 1;
    if !pattern_allows(&key, args.include.as_deref(), args.exclude.as_deref()) {
        if let Some(progress) = progress {
            progress.inc(1);
        }
        report.record_skipped("delete", &format!("tos://{}/{}", target.bucket, key), None);
        return Ok(());
    }
    while in_flight.len() >= limit {
        let Some((uri, result)) = in_flight.next().await else {
            break;
        };
        record_tos_stream_delete_result(
            progress,
            report_path,
            args.report_failures_only,
            report,
            uri,
            result,
        )?;
    }
    let uri = format!("tos://{}/{}", target.bucket, key);
    let bucket = target.bucket.clone();
    in_flight.push(Box::pin(async move {
        let result = core::execute_object_request(
            client,
            "ve-tos rm recursive",
            Method::DELETE,
            &bucket,
            &key,
            BTreeMap::new(),
            BTreeMap::new(),
            None,
        )
        .await
        .map(|_| ());
        (uri, result)
    }));
    Ok(())
}

async fn delete_tos_keys_for_rm(
    client: &TosClient,
    target: &ParsedTosUri,
    keys: Vec<String>,
    args: &RmArgs,
    bottom_up: bool,
    batch_concurrency: usize,
    increment_planned: bool,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let mut delete_keys = Vec::new();
    for key in keys {
        if increment_planned {
            report.summary.planned += 1;
        }
        if !pattern_allows(&key, args.include.as_deref(), args.exclude.as_deref()) {
            if let Some(progress) = progress {
                progress.inc(1);
            }
            report.record_skipped("delete", &format!("tos://{}/{}", target.bucket, key), None);
            continue;
        }
        delete_keys.push(key);
    }
    if bottom_up {
        delete_tos_keys_bottom_up(
            client,
            &target.bucket,
            delete_keys,
            batch_concurrency,
            progress,
            report_path,
            args.report_failures_only,
            report,
        )
        .await
    } else {
        delete_tos_key_group(
            client,
            &target.bucket,
            delete_keys,
            batch_concurrency,
            progress,
            report_path,
            args.report_failures_only,
            report,
        )
        .await
    }
}

async fn delete_tos_keys_bottom_up(
    client: &TosClient,
    bucket: &str,
    keys: Vec<String>,
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report_failures_only: bool,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let (leaf_keys, mut directory_keys): (Vec<_>, Vec<_>) = keys
        .into_iter()
        .partition(|key| !tos_delete_key_is_directory(key));
    delete_tos_key_group(
        client,
        bucket,
        leaf_keys,
        batch_concurrency,
        progress,
        report_path,
        report_failures_only,
        report,
    )
    .await?;

    sort_keys_bottom_up(&mut directory_keys);
    let mut index = 0;
    while index < directory_keys.len() {
        let depth = key_depth(&directory_keys[index]);
        let mut end = index + 1;
        while end < directory_keys.len() && key_depth(&directory_keys[end]) == depth {
            end += 1;
        }
        delete_tos_key_group(
            client,
            bucket,
            directory_keys[index..end].to_vec(),
            batch_concurrency,
            progress,
            report_path,
            report_failures_only,
            report,
        )
        .await?;
        index = end;
    }
    Ok(())
}

fn tos_delete_key_is_directory(key: &str) -> bool {
    key.ends_with('/')
}

async fn delete_tos_key_group(
    client: &TosClient,
    bucket: &str,
    keys: Vec<String>,
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report_failures_only: bool,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let mut in_flight = FuturesUnordered::new();
    let max_in_flight = batch_concurrency.max(1);
    let mut pending = keys.into_iter();
    loop {
        while in_flight.len() < max_in_flight {
            let Some(key) = pending.next() else {
                break;
            };
            let uri = format!("tos://{}/{}", bucket, key);
            let bucket = bucket.to_string();
            in_flight.push(async move {
                let result = core::execute_object_request(
                    client,
                    "ve-tos rm recursive",
                    Method::DELETE,
                    &bucket,
                    &key,
                    BTreeMap::new(),
                    BTreeMap::new(),
                    None,
                )
                .await;
                (uri, result)
            });
        }

        let Some((uri, result)) = in_flight.next().await else {
            break;
        };
        record_tos_key_delete_result(
            progress,
            report_path,
            report_failures_only,
            report,
            uri,
            result.map(|_| ()),
        )?;
    }
    Ok(())
}

fn record_tos_key_delete_result(
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report_failures_only: bool,
    report: &mut BatchReport,
    uri: String,
    result: Result<(), CliError>,
) -> Result<(), CliError> {
    if let Some(progress) = progress {
        progress.inc(1);
    }
    match result {
        Ok(()) => {
            write_single_report(
                success_report_path(report_path, report_failures_only),
                "delete",
                &uri,
                None,
                "succeeded",
            )?;
            report.record_success("delete", &uri, None);
        }
        Err(err) => {
            write_error_report(report_path, "delete", &uri, None, &err)?;
            report.record_failure("delete", &uri, None, &err);
        }
    }
    Ok(())
}

async fn stream_delete_tos_hierarchical_pages(
    client: &TosClient,
    target: &ParsedTosUri,
    prefix: Option<&str>,
    args: &RmArgs,
    batch_concurrency: usize,
    list_concurrency: usize,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let mut pending_prefixes = vec![prefix.unwrap_or("").to_string()];
    let mut seen_prefixes = HashSet::new();
    let mut list_in_flight = FuturesUnordered::new();
    let list_limit = list_concurrency.max(1);
    let mut delete_in_flight: FuturesUnordered<TosDeleteFuture<'_>> = FuturesUnordered::new();
    let delete_limit = batch_concurrency.max(1);

    while !pending_prefixes.is_empty() || !list_in_flight.is_empty() {
        while list_in_flight.len() < list_limit {
            let Some(current_prefix) = pending_prefixes.pop() else {
                break;
            };
            if !seen_prefixes.insert(current_prefix.clone()) {
                continue;
            }
            list_in_flight.push(scan_tos_entries_prefix(
                client,
                &target.bucket,
                current_prefix,
            ));
        }

        let Some(scan) = list_in_flight.next().await else {
            continue;
        };
        let scan = scan?;
        for child_prefix in scan.child_prefixes {
            if !seen_prefixes.contains(&child_prefix) {
                pending_prefixes.push(child_prefix);
            }
        }
        for entry in scan.entries {
            queue_tos_stream_delete(
                client,
                target,
                entry.key,
                args,
                progress,
                report_path,
                report,
                &mut delete_in_flight,
                delete_limit,
            )
            .await?;
        }
    }
    while let Some((uri, result)) = delete_in_flight.next().await {
        record_tos_stream_delete_result(
            progress,
            report_path,
            args.report_failures_only,
            report,
            uri,
            result,
        )?;
    }
    Ok(())
}

async fn stream_delete_tos_hns_bottom_up(
    client: &TosClient,
    target: &ParsedTosUri,
    prefix: Option<&str>,
    args: &RmArgs,
    batch_concurrency: usize,
    list_options: TosRecursiveListOptions,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let root_prefix = prefix.unwrap_or("").to_string();
    let mut keys = list_object_keys_recursive(client, &target.bucket, prefix, list_options).await?;
    if !root_prefix.is_empty() {
        push_unique_key(&mut keys, &root_prefix);
    }
    delete_tos_keys_for_rm(
        client,
        target,
        keys,
        args,
        true,
        batch_concurrency,
        true,
        progress,
        report_path,
        report,
    )
    .await
}

fn record_tos_stream_delete_result(
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report_failures_only: bool,
    report: &mut BatchReport,
    uri: String,
    result: Result<(), CliError>,
) -> Result<(), CliError> {
    if let Some(progress) = progress {
        progress.inc(1);
    }
    match result {
        Ok(()) => {
            write_single_report(
                success_report_path(report_path, report_failures_only),
                "delete",
                &uri,
                None,
                "succeeded",
            )?;
            report.record_success("delete", &uri, None);
        }
        Err(err) => {
            write_error_report(report_path, "delete", &uri, None, &err)?;
            report.record_failure("delete", &uri, None, &err);
        }
    }
    Ok(())
}

async fn abort_multipart_uploads_for_rm(
    client: &TosClient,
    target: &ParsedTosUri,
    prefix: Option<&str>,
    progress_enabled: bool,
    report_path: Option<&str>,
    report_failures_only: bool,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let uploads = list_multipart_uploads_for_rm(client, &target.bucket, prefix).await?;
    if uploads.is_empty() {
        return Ok(());
    }
    let upload_total = uploads.len() as u64;
    let upload_bar = if progress_enabled {
        let pb = ProgressBar::new(upload_total);
        pb.set_style(
            ProgressStyle::with_template(
                "ve-tos rm uploads [{bar:30.yellow/blue}] {pos}/{len} aborted ({per_sec}, ETA {eta})",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-"),
        );
        pb.enable_steady_tick(Duration::from_millis(200));
        Some(pb)
    } else {
        None
    };
    for upload in uploads {
        let uri = format!(
            "tos://{}/{}?{}={}",
            target.bucket,
            upload.key,
            multipart_upload_id_query_key(),
            upload.upload_id
        );
        let query = multipart_upload_id_query(&upload.upload_id);
        match core::execute_object_request(
            client,
            "ve-tos rm abort-upload",
            Method::DELETE,
            &target.bucket,
            &upload.key,
            query,
            BTreeMap::new(),
            None,
        )
        .await
        {
            Ok(_) => {
                if let Some(progress) = &upload_bar {
                    progress.inc(1);
                }
                write_single_report(
                    success_report_path(report_path, report_failures_only),
                    "abort-upload",
                    &uri,
                    None,
                    "succeeded",
                )?;
                report.record_success("abort-upload", &uri, None);
            }
            Err(err) => {
                if let Some(progress) = &upload_bar {
                    progress.inc(1);
                }
                write_error_report(report_path, "abort-upload", &uri, None, &err)?;
                report.record_failure("abort-upload", &uri, None, &err);
            }
        }
    }
    if let Some(progress) = upload_bar {
        progress.finish();
    }
    Ok(())
}

async fn execute_rm_hns_direct(
    global: &GlobalArgs,
    client: &TosClient,
    args: &RmArgs,
    target: &ParsedTosUri,
    path: &str,
    report_path: Option<String>,
) -> Result<i32, CliError> {
    if args.include.is_some() || args.exclude.is_some() {
        return Err(CliError::ValidationError(
            "ve-tos rm --recursive-delete-mode direct does not support --include/--exclude"
                .to_string(),
        ));
    }
    let key = target.key.as_deref().ok_or_else(|| {
        CliError::ValidationError(
            "ve-tos rm --recursive-delete-mode direct requires an HNS directory object target"
                .to_string(),
        )
    })?;
    let query = BTreeMap::from([("recursive".to_string(), "true".to_string())]);
    let result = core::execute_object_request(
        client,
        "ve-tos rm recursive direct",
        Method::DELETE,
        &target.bucket,
        key,
        query,
        BTreeMap::new(),
        None,
    )
    .await?;
    let mut report = BatchReport::new(1);
    report.record_success("delete", path, None);
    write_tos_batch_report(
        report_path.as_deref(),
        "ve-tos rm",
        &report,
        args.report_failures_only,
    )?;
    let mut envelope = Envelope::success(
        "ve-tos rm recursive",
        json!({
            "path": path,
            "status": "succeeded",
            "recursive_delete_mode": recursive_delete_mode_name(RecursiveDeleteMode::Direct),
            "summary": &report.summary,
            "report_path": &report_path,
        }),
    );
    if let Some(request_id) = result.request_id {
        envelope = envelope.with_request_id(request_id);
    }
    output_result(global, &envelope)?;
    Ok(0)
}

async fn bucket_is_hns(client: &TosClient, bucket_name: &str) -> Result<bool, CliError> {
    let head = bucket::head_bucket(client, bucket_name).await?;
    Ok(head
        .data
        .and_then(|data| data.bucket_type)
        .map(|bucket_type| bucket_type.eq_ignore_ascii_case("hns"))
        .unwrap_or(false))
}

fn push_unique_key(keys: &mut Vec<String>, key: &str) {
    if !keys.iter().any(|existing| existing == key) {
        keys.push(key.to_string());
    }
}

fn sort_keys_bottom_up(keys: &mut Vec<String>) {
    keys.sort_by(|left, right| {
        key_depth(right)
            .cmp(&key_depth(left))
            .then_with(|| right.len().cmp(&left.len()))
            .then_with(|| left.cmp(right))
    });
    keys.dedup();
}

fn sort_versions_bottom_up(versions: &mut [ObjectVersionRef]) {
    versions.sort_by(|left, right| {
        key_depth(&right.key)
            .cmp(&key_depth(&left.key))
            .then_with(|| right.key.len().cmp(&left.key.len()))
            .then_with(|| left.key.cmp(&right.key))
    });
}

async fn delete_tos_versions_with_concurrency(
    client: &TosClient,
    bucket: &str,
    versions: Vec<ObjectVersionRef>,
    bottom_up: bool,
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report_failures_only: bool,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    if !bottom_up {
        return delete_tos_version_group(
            client,
            bucket,
            &versions,
            batch_concurrency,
            progress,
            report_path,
            report_failures_only,
            report,
        )
        .await;
    }
    let mut index = 0;
    while index < versions.len() {
        let depth = key_depth(&versions[index].key);
        let mut end = index + 1;
        while end < versions.len() && key_depth(&versions[end].key) == depth {
            end += 1;
        }
        delete_tos_version_group(
            client,
            bucket,
            &versions[index..end],
            batch_concurrency,
            progress,
            report_path,
            report_failures_only,
            report,
        )
        .await?;
        index = end;
    }
    Ok(())
}

async fn delete_tos_version_group(
    client: &TosClient,
    bucket: &str,
    versions: &[ObjectVersionRef],
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report_failures_only: bool,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let limit = batch_concurrency.max(1);
    let mut pending = versions.iter().cloned();
    let mut in_flight = FuturesUnordered::new();
    loop {
        while in_flight.len() < limit {
            let Some(version) = pending.next() else {
                break;
            };
            let uri = version_uri(bucket, &version);
            in_flight.push(async move {
                let mut query = BTreeMap::new();
                query.insert("versionId".to_string(), version.version_id.clone());
                let result = core::execute_object_request(
                    client,
                    "ve-tos rm all-versions",
                    Method::DELETE,
                    bucket,
                    &version.key,
                    query,
                    BTreeMap::new(),
                    None,
                )
                .await
                .map(|_| ());
                (uri, result)
            });
        }
        let Some((uri, result)) = in_flight.next().await else {
            break;
        };
        if let Some(progress) = progress {
            progress.inc(1);
        }
        match result {
            Ok(()) => {
                write_single_report(
                    success_report_path(report_path, report_failures_only),
                    "delete",
                    &uri,
                    None,
                    "succeeded",
                )?;
                report.record_success("delete", &uri, None);
            }
            Err(err) => {
                write_error_report(report_path, "delete", &uri, None, &err)?;
                report.record_failure("delete", &uri, None, &err);
            }
        }
    }
    Ok(())
}

fn version_uri(bucket: &str, version: &ObjectVersionRef) -> String {
    format!(
        "tos://{}/{}?versionId={}",
        bucket, version.key, version.version_id
    )
}

fn key_depth(key: &str) -> usize {
    key.trim_end_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

fn recursive_delete_mode_name(mode: RecursiveDeleteMode) -> &'static str {
    match mode {
        RecursiveDeleteMode::BottomUp => "bottom-up",
        RecursiveDeleteMode::Direct => "direct",
    }
}

fn resolve_tos_recursive_delete_mode(
    is_hns_bucket: bool,
    args: &RmArgs,
) -> Result<Option<RecursiveDeleteMode>, CliError> {
    if std::env::var("VE_STORAGE_UNI_TOS_FORCE_FNS_DELETE")
        .ok()
        .as_deref()
        == Some("1")
    {
        if let Some(mode) = args.recursive_delete_mode {
            return Err(CliError::ValidationError(format!(
                "tos-cli recursive delete uses FNS-style planned object deletes; --recursive-delete-mode {} is not supported",
                recursive_delete_mode_name(mode)
            )));
        }
        // [Review Fix #2] ByteCloud `tos-cli` must not use bottom-up HNS
        // directory deletion; it follows the same planned key delete flow as
        // ve-tos FNS buckets.
        return Ok(None);
    }
    if !is_hns_bucket {
        if let Some(mode) = args.recursive_delete_mode {
            return Err(CliError::ValidationError(format!(
                "ve-tos rm --recursive-delete-mode {} is only supported for HNS buckets",
                recursive_delete_mode_name(mode)
            )));
        }
        return Ok(None);
    }
    Ok(Some(
        args.recursive_delete_mode
            .unwrap_or(RecursiveDeleteMode::BottomUp),
    ))
}

async fn execute_restore(
    global: &GlobalArgs,
    client: &TosClient,
    args: &RestoreArgs,
) -> Result<i32, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos restore",
    )?;
    if args.recursive || args.manifest.is_some() {
        let report_path =
            effective_report_path(global, args.report_path.as_deref(), "ve-tos restore")?;
        let manifest_path = effective_optional_manifest_path(
            global,
            args.manifest_path.as_deref(),
            args.no_manifest,
            "ve-tos restore",
        )?;
        let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
        let list_echo_enabled =
            effective_list_echo_enabled(global, args.list_echo, args.no_list_echo);
        let batch_concurrency = effective_batch_concurrency(global, args.batch_concurrency)?;
        let list_concurrency = effective_list_concurrency(global, args.list_concurrency)?;
        if args.no_manifest {
            return execute_restore_streaming_no_manifest(
                global,
                client,
                args,
                &path,
                report_path,
                progress_enabled,
                batch_concurrency,
                list_concurrency,
            )
            .await;
        }
        let mut scan_progress = RemoteScanProgress::new(list_echo_enabled, "ve-tos restore", &path);
        let plan = restore_batch_plan(client, args, &path, list_concurrency)
            .await?
            .into_iter()
            .filter(|item| restore_key_matches(args, &item.key, &path))
            .collect::<Vec<_>>();
        scan_progress.finish_with_count(plan.len() as u64, "item(s)");
        let mut target = parse_tos_uri(&path, true)?;
        normalize_recursive_tos_target(&mut target);
        let manifest = build_list_manifest(
            plan.iter()
                .map(|item| restore_plan_manifest_item(&target.bucket, target.key.as_deref(), item))
                .collect(),
        );
        write_list_manifest_file(manifest_path.as_deref(), "ve-tos restore", &manifest)?;
        let total_items = plan.len() as u64;
        let mut report = BatchReport::new(total_items);
        let bar = if progress_enabled && total_items > 0 {
            let pb = ProgressBar::new(total_items);
            pb.set_style(
                ProgressStyle::with_template(
                    "ve-tos restore [{bar:30.cyan/blue}] {pos}/{len} items ({per_sec}, ETA {eta})",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=>-"),
            );
            pb.enable_steady_tick(Duration::from_millis(200));
            Some(pb)
        } else {
            None
        };
        let mut in_flight = FuturesUnordered::new();
        // [Review Fix #3] Batch restore uses the same bounded queue pattern as
        // recursive transfers so completed requests are always polled.
        let max_in_flight = batch_concurrency.max(1);
        let mut pending = plan.into_iter();
        loop {
            while in_flight.len() < max_in_flight {
                let Some(item) = pending.next() else {
                    break;
                };
                let uri = format!("tos://{}/{}", target.bucket, item.key);
                if item.is_directory {
                    record_tos_restore_skipped(
                        bar.as_ref(),
                        report_path.as_deref(),
                        args.report_failures_only,
                        &mut report,
                        &uri,
                    )?;
                    continue;
                }
                let bucket = target.bucket.clone();
                in_flight.push(async move {
                    let result = restore_one_object(
                        client,
                        &bucket,
                        &item.key,
                        args.days,
                        args.tier.as_deref(),
                        args.version_id.as_deref(),
                    )
                    .await;
                    (uri, result)
                });
            }

            let Some((uri, result)) = in_flight.next().await else {
                break;
            };
            match result {
                Ok(_) => {
                    if let Some(b) = &bar {
                        b.inc(1);
                    }
                    write_single_report(
                        success_report_path(report_path.as_deref(), args.report_failures_only),
                        "restore",
                        &uri,
                        None,
                        "succeeded",
                    )?;
                    report.record_success("restore", &uri, None);
                }
                Err(err) => {
                    if let Some(b) = &bar {
                        b.inc(1);
                    }
                    write_error_report(report_path.as_deref(), "restore", &uri, None, &err)?;
                    report.record_failure("restore", &uri, None, &err);
                }
            }
        }
        if let Some(b) = bar {
            b.finish();
        }
        let failed_count = report.summary.failed;
        write_tos_batch_report(
            report_path.as_deref(),
            "ve-tos restore",
            &report,
            args.report_failures_only,
        )?;
        output_result(
            global,
            &Envelope::success(
                "ve-tos restore batch",
                json!({
                    "path": &path,
                    "status": if failed_count == 0 { "succeeded" } else { "partial_failure" },
                    "summary": &report.summary,
                    "report_path": &report_path,
                    "manifest_path": &manifest_path,
                }),
            ),
        )?;
        // [Review Fix #BatchExitCode] 批量 summary 已经输出，失败项用
        // exit code=1 表达，避免顶层错误处理再打印一份 JSON。
        return if failed_count == 0 { Ok(0) } else { Ok(1) };
    }
    reject_single_transfer_artifacts(
        "ve-tos restore",
        args.report_path.as_deref(),
        args.report_failures_only,
        args.manifest_path.as_deref(),
        args.no_manifest,
        args.batch_concurrency,
        args.list_concurrency,
    )?;
    let target = parse_tos_uri(&path, false)?;
    let key = target.key.expect("validated object key");
    output_result(
        global,
        &restore_one_object(
            client,
            &target.bucket,
            &key,
            args.days,
            args.tier.as_deref(),
            args.version_id.as_deref(),
        )
        .await?,
    )?;
    Ok(0)
}

async fn execute_restore_streaming_no_manifest(
    global: &GlobalArgs,
    client: &TosClient,
    args: &RestoreArgs,
    path: &str,
    report_path: Option<String>,
    progress_enabled: bool,
    batch_concurrency: usize,
    list_concurrency: usize,
) -> Result<i32, CliError> {
    let mut target = parse_tos_uri(path, true)?;
    normalize_recursive_tos_target(&mut target);
    let progress = streaming_batch_progress(progress_enabled, "ve-tos restore");
    let mut report = BatchReport::new(0);
    let stream_result = stream_restore_no_manifest(
        client,
        args,
        path,
        &target,
        batch_concurrency,
        list_concurrency,
        &progress,
        report_path.as_deref(),
        &mut report,
    )
    .await;
    finish_streaming_progress(progress, report.summary.planned);
    let failed_count = report.summary.failed;
    write_tos_batch_report(
        report_path.as_deref(),
        "ve-tos restore",
        &report,
        args.report_failures_only,
    )?;
    stream_result?;
    output_result(
        global,
        &Envelope::success(
            "ve-tos restore batch",
            json!({
                "path": path,
                "status": if failed_count == 0 { "succeeded" } else { "partial_failure" },
                "summary": &report.summary,
                "report_path": &report_path,
                "manifest_path": Value::Null,
            }),
        ),
    )?;
    // [Review Fix #BatchExitCode] 批量 summary 已经输出，失败项用
    // exit code=1 表达，避免顶层错误处理再打印一份 JSON。
    if failed_count == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

async fn stream_restore_no_manifest(
    client: &TosClient,
    args: &RestoreArgs,
    path: &str,
    target: &ParsedTosUri,
    batch_concurrency: usize,
    list_concurrency: usize,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let mut in_flight: FuturesUnordered<TosObjectActionFuture<'_>> = FuturesUnordered::new();
    let limit = batch_concurrency.max(1);
    let stream_result = if let Some(manifest) = &args.manifest {
        let content = fs::read_to_string(manifest)?;
        let keys = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| restore_manifest_line_to_key(line, target))
            .collect::<Result<Vec<_>, _>>()?;
        let mut result = Ok(());
        for key in keys {
            let item = restore_plan_item_from_key(key);
            if let Err(err) = queue_tos_stream_restore(
                client,
                args,
                path,
                target,
                item,
                progress,
                report_path,
                report,
                &mut in_flight,
                limit,
            )
            .await
            {
                result = Err(err);
                break;
            }
        }
        result
    } else {
        let is_hns_bucket = bucket_is_hns(client, &target.bucket).await?;
        let prefix = target.key.as_deref().unwrap_or_default();
        let use_hierarchical_listing =
            resolve_tos_recursive_list_mode(is_hns_bucket, args.recursive_list_mode);
        if use_hierarchical_listing {
            stream_restore_hierarchical_prefix(
                client,
                args,
                path,
                target,
                prefix,
                list_concurrency,
                progress,
                report_path,
                report,
                &mut in_flight,
                limit,
            )
            .await
        } else {
            stream_restore_flat_prefix(
                client,
                args,
                path,
                target,
                prefix,
                progress,
                report_path,
                report,
                &mut in_flight,
                limit,
            )
            .await
        }
    };
    while let Some((uri, result)) = in_flight.next().await {
        record_tos_stream_restore_result(
            progress,
            report_path,
            args.report_failures_only,
            report,
            uri,
            result,
        )?;
    }
    stream_result
}

fn restore_manifest_line_to_key(line: &str, target: &ParsedTosUri) -> Result<String, CliError> {
    if line.starts_with("tos://") {
        let uri = parse_tos_uri(line, false)?;
        if uri.bucket != target.bucket {
            return Err(CliError::ValidationError(format!(
                "manifest object '{}' belongs to a different bucket",
                line
            )));
        }
        Ok(uri.key.expect("validated object key"))
    } else {
        Ok(line.to_string())
    }
}

async fn stream_restore_flat_prefix<'a>(
    client: &'a TosClient,
    args: &RestoreArgs,
    path: &str,
    target: &ParsedTosUri,
    prefix: &str,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report: &mut BatchReport,
    in_flight: &mut FuturesUnordered<TosObjectActionFuture<'a>>,
    limit: usize,
) -> Result<(), CliError> {
    let mut continuation_token = None;
    loop {
        let page = list_object_entries_page(
            client,
            &target.bucket,
            Some(prefix),
            None,
            continuation_token.as_deref(),
        )
        .await?;
        continuation_token = page.next_token.clone();
        for entry in page.entries {
            queue_tos_stream_restore(
                client,
                args,
                path,
                target,
                restore_plan_item_from_entry(entry),
                progress,
                report_path,
                report,
                in_flight,
                limit,
            )
            .await?;
        }
        if !page.is_truncated {
            break;
        }
    }
    Ok(())
}

async fn stream_restore_hierarchical_prefix<'a>(
    client: &'a TosClient,
    args: &RestoreArgs,
    path: &str,
    target: &ParsedTosUri,
    prefix: &str,
    list_concurrency: usize,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report: &mut BatchReport,
    in_flight: &mut FuturesUnordered<TosObjectActionFuture<'a>>,
    limit: usize,
) -> Result<(), CliError> {
    let mut pending_prefixes = vec![prefix.to_string()];
    let mut seen_prefixes = HashSet::new();
    let mut list_in_flight = FuturesUnordered::new();
    let list_limit = list_concurrency.max(1);
    while !pending_prefixes.is_empty() || !list_in_flight.is_empty() {
        while list_in_flight.len() < list_limit {
            let Some(current_prefix) = pending_prefixes.pop() else {
                break;
            };
            if !seen_prefixes.insert(current_prefix.clone()) {
                continue;
            }
            list_in_flight.push(scan_tos_entries_prefix(
                client,
                &target.bucket,
                current_prefix,
            ));
        }

        let Some(scan) = list_in_flight.next().await else {
            continue;
        };
        let scan = scan?;
        for child_prefix in scan.child_prefixes {
            if !seen_prefixes.contains(&child_prefix) {
                pending_prefixes.push(child_prefix);
            }
        }
        for entry in scan.entries {
            queue_tos_stream_restore(
                client,
                args,
                path,
                target,
                restore_plan_item_from_entry(entry),
                progress,
                report_path,
                report,
                in_flight,
                limit,
            )
            .await?;
        }
    }
    Ok(())
}

async fn queue_tos_stream_restore<'a>(
    client: &'a TosClient,
    args: &RestoreArgs,
    path: &str,
    target: &ParsedTosUri,
    item: RestorePlanItem,
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report: &mut BatchReport,
    in_flight: &mut FuturesUnordered<TosObjectActionFuture<'a>>,
    limit: usize,
) -> Result<(), CliError> {
    if !restore_key_matches(args, &item.key, path) {
        return Ok(());
    }
    report.summary.planned += 1;
    let uri = format!("tos://{}/{}", target.bucket, item.key);
    if item.is_directory {
        record_tos_restore_skipped(
            progress.as_ref(),
            report_path,
            args.report_failures_only,
            report,
            &uri,
        )?;
        return Ok(());
    }
    while in_flight.len() >= limit {
        let Some((uri, result)) = in_flight.next().await else {
            break;
        };
        record_tos_stream_restore_result(
            progress,
            report_path,
            args.report_failures_only,
            report,
            uri,
            result,
        )?;
    }
    let bucket = target.bucket.clone();
    let key = item.key;
    let days = args.days;
    let tier = args.tier.clone();
    let version_id = args.version_id.clone();
    in_flight.push(Box::pin(async move {
        let result = restore_one_object(
            client,
            &bucket,
            &key,
            days,
            tier.as_deref(),
            version_id.as_deref(),
        )
        .await
        .map(|_| ());
        (uri, result)
    }));
    Ok(())
}

fn record_tos_stream_restore_result(
    progress: &Option<ProgressBar>,
    report_path: Option<&str>,
    report_failures_only: bool,
    report: &mut BatchReport,
    uri: String,
    result: Result<(), CliError>,
) -> Result<(), CliError> {
    if let Some(progress) = progress {
        progress.inc(1);
    }
    match result {
        Ok(()) => {
            write_single_report(
                success_report_path(report_path, report_failures_only),
                "restore",
                &uri,
                None,
                "succeeded",
            )?;
            report.record_success("restore", &uri, None);
        }
        Err(err) => {
            write_error_report(report_path, "restore", &uri, None, &err)?;
            report.record_failure("restore", &uri, None, &err);
        }
    }
    Ok(())
}

fn record_tos_restore_skipped(
    progress: Option<&ProgressBar>,
    report_path: Option<&str>,
    report_failures_only: bool,
    report: &mut BatchReport,
    uri: &str,
) -> Result<(), CliError> {
    if let Some(progress) = progress {
        progress.inc(1);
    }
    // [Review Fix #RestoreSkipDirs] Directory keys cannot be restored by
    // RestoreObject, so recursive restore records them as skipped instead of
    // sending a request that would fail with a service-side 400.
    write_single_report(
        success_report_path(report_path, report_failures_only),
        "restore",
        uri,
        None,
        "skipped",
    )?;
    report.record_skipped("restore", uri, None);
    Ok(())
}

async fn execute_cp(
    global: &GlobalArgs,
    client: &TosClient,
    args: &CpArgs,
) -> Result<i32, CliError> {
    ensure_same_region_for_tos_uris(client, &args.source, &args.destination).await?;
    if args.recursive {
        return execute_cp_recursive(global, client, args).await;
    }
    reject_single_transfer_artifacts(
        "ve-tos cp",
        args.report_path.as_deref(),
        args.report_failures_only,
        args.manifest_path.as_deref(),
        args.no_manifest,
        args.batch_concurrency,
        args.list_concurrency,
    )?;
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let runtime = effective_cp_runtime_config(global, args)?;
    let write_options = copy_write_options(args)?;
    let destination = resolve_single_transfer_destination(&args.source, &args.destination)?;
    ensure_tos_upload_storage_class_supported(
        "ve-tos cp",
        Some(&args.source),
        &destination,
        write_options.storage_class.as_deref(),
    )?;
    let outcome = copy_one(
        global,
        client,
        &args.source,
        &destination,
        runtime.copy_options(
            None,
            false,
            args.checkpoint,
            args.checkpoint_dir.as_deref(),
            write_options,
            progress_enabled,
            true,
            None,
        ),
    )
    .await?;
    output_single_transfer_envelope(
        global,
        "ve-tos cp",
        single_transfer_operation(&args.source, &destination),
        &args.source,
        &destination,
        outcome,
    )?;
    Ok(0)
}

async fn execute_mv(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MvArgs,
) -> Result<i32, CliError> {
    ensure_same_region_for_tos_uris(client, &args.source, &args.destination).await?;
    if args.recursive {
        return execute_mv_recursive_tos(global, client, args).await;
    }
    let source_delete_etag = if args.source.starts_with("tos://") {
        let source = parse_tos_uri(&args.source, false)?;
        let key = source.key.expect("validated object key");
        source_object_etag(client, &source.bucket, &key).await?
    } else {
        None
    };
    reject_single_transfer_artifacts(
        "ve-tos mv",
        args.report_path.as_deref(),
        args.report_failures_only,
        args.manifest_path.as_deref(),
        args.no_manifest,
        args.batch_concurrency,
        args.list_concurrency,
    )?;
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let mut runtime = effective_default_runtime_config(global)?;
    runtime.overwrite_strategy = EffectiveOverwriteStrategy::Force;
    let write_options = mv_write_options(args)?;
    let destination = resolve_single_transfer_destination(&args.source, &args.destination)?;
    ensure_tos_upload_storage_class_supported(
        "ve-tos mv",
        Some(&args.source),
        &destination,
        write_options.storage_class.as_deref(),
    )?;
    let outcome = copy_one(
        global,
        client,
        &args.source,
        &destination,
        runtime.copy_options(
            None,
            false,
            true,
            args.checkpoint_dir.as_deref(),
            write_options,
            progress_enabled,
            true,
            None,
        ),
    )
    .await?;
    if outcome.is_skipped() {
        return Err(CliError::Conflict(
            "ve-tos mv copy was skipped; source was not deleted".to_string(),
        ));
    }
    if args.source.starts_with("tos://") {
        let source = parse_tos_uri(&args.source, false)?;
        let key = source.key.expect("validated object key");
        delete_tos_object(
            client,
            "ve-tos mv delete-source",
            &source.bucket,
            &key,
            source_delete_etag.as_deref(),
        )
        .await?;
    } else {
        let source_path = Path::new(&args.source);
        if source_path.is_file() {
            fs::remove_file(source_path)?;
        } else if source_path.is_dir() {
            fs::remove_dir(source_path)?;
        }
    }
    output_single_transfer_envelope(
        global,
        "ve-tos mv",
        "copy-delete",
        &args.source,
        &destination,
        outcome,
    )?;
    Ok(0)
}

async fn execute_mv_recursive_tos(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MvArgs,
) -> Result<i32, CliError> {
    let report_path = effective_report_path(global, args.report_path.as_deref(), "ve-tos mv")?;
    let manifest_path = effective_optional_manifest_path(
        global,
        args.manifest_path.as_deref(),
        args.no_manifest,
        "ve-tos mv",
    )?;
    let cp_args = cp_args_from_mv(args, true, report_path.clone());
    let runtime = effective_cp_runtime_config(global, &cp_args)?;
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let list_echo_enabled = effective_list_echo_enabled(global, args.list_echo, args.no_list_echo);
    let write_options = mv_write_options(args)?;
    ensure_tos_upload_storage_class_supported(
        "ve-tos mv",
        Some(&args.source),
        &args.destination,
        write_options.storage_class.as_deref(),
    )?;
    if let Some(rename_plan) =
        resolve_tos_hns_recursive_rename_plan(client, args, &write_options).await?
    {
        return execute_tos_hns_recursive_rename(
            global,
            client,
            args,
            rename_plan,
            report_path,
            manifest_path,
        )
        .await;
    }
    if args.no_manifest {
        return execute_mv_recursive_streaming_no_manifest(
            global,
            client,
            args,
            runtime,
            write_options,
            report_path,
            progress_enabled,
        )
        .await;
    }
    let mut scan_progress =
        RemoteScanProgress::new(list_echo_enabled, "ve-tos mv plan", &args.source);
    let planned = build_recursive_copy_mappings(
        client,
        &args.source,
        &args.destination,
        args.include_parent,
        args.recursive_list_mode,
        runtime.list_concurrency,
    )
    .await?
    .into_iter()
    .filter(|item| {
        pattern_allows(
            &item.relative_key,
            args.include.as_deref(),
            args.exclude.as_deref(),
        )
    })
    .collect::<Vec<_>>();
    enforce_transfer_plan_path_traversal(
        global,
        "ve-tos mv",
        args.force,
        true,
        &args.source,
        &planned,
    )?;
    scan_progress.finish_with_count(planned.len() as u64, "item(s)");
    let manifest = build_move_transfer_manifest(&planned);
    write_manifest_file(manifest_path.as_deref(), "ve-tos mv", &manifest)?;

    let total_files = planned.len() as u64;
    let total_bytes: u64 = planned.iter().map(|item| item.size).sum();
    let total_progress_units: u64 = planned
        .iter()
        .map(|item| progress_units_for_size(item.size, runtime, true))
        .sum();
    let mut report = BatchReport::new(manifest.object_count);
    let mut summary = BatchProgressSummary::new(
        "ve-tos mv",
        &args.source,
        &args.destination,
        report_path.as_deref(),
        manifest_path.as_deref(),
        total_files,
        total_progress_units,
        total_bytes,
        runtime.progress_granularity,
        progress_enabled,
    );

    // [Review Fix #MoveReport] Recursive mv must know whether every copy item
    // finished before it may start deleting sources.
    let overall_bar_owned = summary.overall.clone();
    let overall_bar_ref = overall_bar_owned.as_ref();
    let mut in_flight = FuturesUnordered::new();
    // [Review Fix #2] Keep recursive move bounded without blocking before
    // queued copy futures get a chance to finish and release capacity.
    let max_in_flight = runtime.batch_concurrency.max(1);
    let mut pending = planned.clone().into_iter();

    loop {
        while in_flight.len() < max_in_flight {
            let Some(item) = pending.next() else {
                break;
            };
            let source = item.source;
            let destination = item.destination;
            let bytes = item.size;
            summary.set_current_file(&source);
            let checkpoint_dir = args.checkpoint_dir.as_deref();
            let write_options = write_options.clone();

            in_flight.push(async move {
                let result = copy_one(
                    global,
                    client,
                    &source,
                    &destination,
                    runtime.copy_options(
                        None,
                        false,
                        true,
                        checkpoint_dir,
                        write_options,
                        progress_enabled,
                        true,
                        overall_bar_ref,
                    ),
                )
                .await;
                (source, destination, bytes, result)
            });
        }

        let Some((src, dst, b, result)) = in_flight.next().await else {
            break;
        };
        record_tos_copy_result(&mut report, &mut summary, "copy", src, dst, b, result);
    }
    if let Some(bar) = &summary.overall {
        bar.finish();
    }

    if report.summary.failed > 0 {
        for item in &planned {
            report.record_skipped("delete-source", &item.source, None);
        }
        write_tos_batch_report(
            report_path.as_deref(),
            "ve-tos mv",
            &report,
            args.report_failures_only,
        )?;
        output_tos_batch_envelope(
            global,
            "ve-tos mv",
            &args.source,
            &args.destination,
            report_path.as_deref(),
            manifest_path.as_deref(),
            &report,
        )?;
        return Ok(1);
    }

    delete_recursive_move_sources_tos(
        client,
        args,
        &planned,
        runtime.batch_concurrency,
        progress_enabled,
        &mut report,
    )
    .await?;
    write_tos_batch_report(
        report_path.as_deref(),
        "ve-tos mv",
        &report,
        args.report_failures_only,
    )?;
    output_tos_batch_envelope(
        global,
        "ve-tos mv",
        &args.source,
        &args.destination,
        report_path.as_deref(),
        manifest_path.as_deref(),
        &report,
    )?;
    if report.summary.failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

#[derive(Debug, Clone)]
struct TosRecursiveRenamePlan {
    bucket: String,
    source_key: String,
    destination_key: String,
    source_uri: String,
    destination_uri: String,
    relative_key: String,
}

async fn resolve_tos_hns_recursive_rename_plan(
    client: &TosClient,
    args: &MvArgs,
    write_options: &ObjectWriteOptions,
) -> Result<Option<TosRecursiveRenamePlan>, CliError> {
    if args.include.is_some()
        || args.exclude.is_some()
        || !write_options.is_empty()
        || !args.source.starts_with("tos://")
        || !args.destination.starts_with("tos://")
    {
        return Ok(None);
    }
    let rename_plan =
        build_tos_recursive_rename_plan(&args.source, &args.destination, args.include_parent)?;
    let Some(plan) = rename_plan else {
        return Ok(None);
    };
    if !bucket_is_hns(client, &plan.bucket).await? {
        return Ok(None);
    }
    Ok(Some(plan))
}

fn build_tos_recursive_rename_plan(
    source: &str,
    destination: &str,
    include_parent: bool,
) -> Result<Option<TosRecursiveRenamePlan>, CliError> {
    let mut source_target = parse_tos_uri(source, true)?;
    let mut destination_target = parse_tos_uri(destination, true)?;
    normalize_recursive_tos_target(&mut source_target);
    normalize_recursive_tos_target(&mut destination_target);
    if source_target.bucket != destination_target.bucket {
        return Ok(None);
    }
    let Some(source_key) = source_target.key.clone().filter(|key| !key.is_empty()) else {
        return Ok(None);
    };
    let destination_prefix = destination_target.key.clone().unwrap_or_default();
    let parent_prefix = recursive_source_parent_prefix(source, include_parent)?;
    let destination_key =
        normalize_recursive_tos_prefix(Some(&prepend_parent_prefix("", parent_prefix.as_deref())));
    let destination_key =
        normalize_recursive_tos_prefix(Some(&join_tos_key(&destination_prefix, &destination_key)));
    if destination_key.is_empty() {
        return Ok(None);
    }
    if source_key == destination_key {
        return Err(CliError::ValidationError(
            "source and destination resolve to the same TOS prefix".to_string(),
        ));
    }
    if destination_key.starts_with(&source_key) {
        return Err(CliError::ValidationError(
            "recursive mv destination must not be inside the source prefix".to_string(),
        ));
    }
    let relative_key = source_key.trim_end_matches('/').to_string();
    Ok(Some(TosRecursiveRenamePlan {
        bucket: source_target.bucket.clone(),
        source_uri: format!("tos://{}/{}", source_target.bucket, source_key),
        destination_uri: format!("tos://{}/{}", destination_target.bucket, destination_key),
        source_key,
        destination_key,
        relative_key,
    }))
}

async fn execute_tos_hns_recursive_rename(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MvArgs,
    plan: TosRecursiveRenamePlan,
    report_path: Option<String>,
    manifest_path: Option<String>,
) -> Result<i32, CliError> {
    let manifest = build_tos_recursive_rename_manifest(&plan);
    write_manifest_file(manifest_path.as_deref(), "ve-tos mv", &manifest)?;

    let rename_result = core::execute_object_request(
        client,
        "ve-tos mv recursive rename",
        Method::PUT,
        &plan.bucket,
        &plan.source_key,
        tos_recursive_rename_query(&plan),
        tos_recursive_rename_headers(),
        None,
    )
    .await;

    let mut report = BatchReport::new(1);
    match rename_result {
        Ok(result) => finish_tos_recursive_rename_success(
            global,
            args,
            &plan,
            report_path.as_deref(),
            manifest_path.as_deref(),
            &mut report,
            result.request_id,
        ),
        Err(err) => finish_tos_recursive_rename_failure(
            global,
            args,
            &plan,
            report_path.as_deref(),
            manifest_path.as_deref(),
            &mut report,
            &err,
        ),
    }
}

fn finish_tos_recursive_rename_success(
    global: &GlobalArgs,
    args: &MvArgs,
    plan: &TosRecursiveRenamePlan,
    report_path: Option<&str>,
    manifest_path: Option<&str>,
    report: &mut BatchReport,
    request_id: Option<String>,
) -> Result<i32, CliError> {
    report.record_success(
        "rename-recursive",
        &plan.source_uri,
        Some(&plan.destination_uri),
    );
    write_tos_batch_report(report_path, "ve-tos mv", report, args.report_failures_only)?;
    output_tos_recursive_rename_success(
        global,
        plan,
        report_path,
        manifest_path,
        report,
        request_id,
    )?;
    Ok(0)
}

fn finish_tos_recursive_rename_failure(
    global: &GlobalArgs,
    args: &MvArgs,
    plan: &TosRecursiveRenamePlan,
    report_path: Option<&str>,
    manifest_path: Option<&str>,
    report: &mut BatchReport,
    err: &CliError,
) -> Result<i32, CliError> {
    report.record_failure(
        "rename-recursive",
        &plan.source_uri,
        Some(&plan.destination_uri),
        err,
    );
    write_tos_batch_report(report_path, "ve-tos mv", report, args.report_failures_only)?;
    output_tos_batch_envelope(
        global,
        "ve-tos mv",
        &args.source,
        &args.destination,
        report_path,
        manifest_path,
        report,
    )?;
    Ok(1)
}

fn build_tos_recursive_rename_manifest(plan: &TosRecursiveRenamePlan) -> TransferManifest {
    TransferManifest::from_items(vec![TransferManifestItem {
        // [Review Fix #HNS-Move] HNS recursive mv can be one service-side
        // RenameObject request; the manifest records that real operation
        // instead of synthetic per-object copy/delete rows.
        operation: "rename-recursive",
        relative_key: plan.relative_key.clone(),
        source: plan.source_uri.clone(),
        destination: Some(plan.destination_uri.clone()),
        size: 0,
        etag: None,
        crc64: None,
        last_modified: None,
    }])
}

fn tos_recursive_rename_query(plan: &TosRecursiveRenamePlan) -> BTreeMap<String, String> {
    let mut query = BTreeMap::new();
    query.insert("rename".to_string(), String::new());
    query.insert("name".to_string(), plan.destination_key.clone());
    query.insert("recursive".to_string(), "true".to_string());
    query
}

fn tos_recursive_rename_headers() -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    // [Review Fix #HNS-Move-Overwrite] Recursive mv historically uses force
    // overwrite for the copy phase. Do not set x-tos-forbid-overwrite here:
    // args.force is the destructive-operation confirmation, not no-clobber.
    headers.insert("x-tos-recursive-mkdir".to_string(), "true".to_string());
    headers
}

fn output_tos_recursive_rename_success(
    global: &GlobalArgs,
    plan: &TosRecursiveRenamePlan,
    report_path: Option<&str>,
    manifest_path: Option<&str>,
    report: &BatchReport,
    request_id: Option<String>,
) -> Result<(), CliError> {
    let mut envelope = Envelope::success(
        "ve-tos mv recursive rename",
        json!({
            "operation": "rename-recursive",
            "source": &plan.source_uri,
            "destination": &plan.destination_uri,
            "summary": &report.summary,
            "report_path": report_path,
            "manifest_path": manifest_path,
            "status": "succeeded",
        }),
    );
    if let Some(request_id) = request_id {
        envelope = envelope.with_request_id(request_id);
    }
    output_result(global, &envelope)
}

async fn execute_mv_recursive_streaming_no_manifest(
    global: &GlobalArgs,
    client: &TosClient,
    args: &MvArgs,
    runtime: TransferRuntimeConfig,
    write_options: ObjectWriteOptions,
    report_path: Option<String>,
    progress_enabled: bool,
) -> Result<i32, CliError> {
    let progress = streaming_batch_progress(progress_enabled, "ve-tos mv");
    let mut report = BatchReport::new(0);
    let stream_result = {
        let mut context = TosStreamMoveContext {
            global,
            client,
            args,
            runtime,
            write_options,
            progress: &progress,
            report_path: report_path.as_deref(),
            report: &mut report,
            in_flight: FuturesUnordered::new(),
            limit: runtime.batch_concurrency.max(1),
        };
        let result = if args.source.starts_with("tos://") {
            stream_mv_tos_source_no_manifest(&mut context).await
        } else {
            stream_mv_local_source_no_manifest(&mut context).await
        };
        context.drain_all().await;
        result
    };
    let prune_result = if stream_result.is_ok() && !args.source.starts_with("tos://") {
        prune_empty_directories(Path::new(&args.source))
    } else {
        Ok(())
    };
    finish_streaming_progress(progress, report.summary.planned);
    let failed = report.summary.failed;
    write_tos_batch_report(
        report_path.as_deref(),
        "ve-tos mv",
        &report,
        args.report_failures_only,
    )?;
    stream_result?;
    prune_result?;
    output_tos_batch_envelope(
        global,
        "ve-tos mv",
        &args.source,
        &args.destination,
        report_path.as_deref(),
        None,
        &report,
    )?;
    if failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

struct TosStreamMoveContext<'a> {
    global: &'a GlobalArgs,
    client: &'a TosClient,
    args: &'a MvArgs,
    runtime: TransferRuntimeConfig,
    write_options: ObjectWriteOptions,
    progress: &'a Option<ProgressBar>,
    report_path: Option<&'a str>,
    report: &'a mut BatchReport,
    in_flight: FuturesUnordered<TosMoveFuture<'a>>,
    limit: usize,
}

impl<'a> TosStreamMoveContext<'a> {
    async fn queue(&mut self, item: TransferPlanItem) {
        self.report.summary.planned += 2;
        while self.in_flight.len() >= self.limit {
            if !self.drain_one().await {
                break;
            }
        }
        let runtime = self.runtime;
        let global = self.global;
        let client = self.client;
        let report_path = self.report_path;
        let report_failures_only = self.args.report_failures_only;
        let checkpoint_dir = self.args.checkpoint_dir.as_deref();
        let write_options = self.write_options.clone();
        self.in_flight.push(Box::pin(async move {
            let source = item.source.clone();
            let destination = item.destination.clone();
            let copy_result = copy_one(
                global,
                client,
                &source,
                &destination,
                runtime.copy_options(
                    report_path,
                    report_failures_only,
                    true,
                    checkpoint_dir,
                    write_options,
                    false,
                    true,
                    None,
                ),
            )
            .await;
            let delete_result = if copy_result.is_ok() {
                Some(delete_move_source_item(client, &item).await)
            } else {
                None
            };
            (item, copy_result, delete_result)
        }));
    }

    async fn drain_one(&mut self) -> bool {
        let Some((item, copy_result, delete_result)) = self.in_flight.next().await else {
            return false;
        };
        record_tos_stream_move_result(self.progress, self.report, item, copy_result, delete_result);
        true
    }

    async fn drain_all(&mut self) {
        while self.drain_one().await {}
    }
}

async fn stream_mv_local_source_no_manifest(
    context: &mut TosStreamMoveContext<'_>,
) -> Result<(), CliError> {
    let source_root = Path::new(&context.args.source);
    if !source_root.is_dir() {
        return Err(CliError::ValidationError(format!(
            "recursive source '{}' must be a local directory or tos:// prefix",
            context.args.source
        )));
    }
    let parent_prefix =
        recursive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let mut pending = vec![source_root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut child_directories = Vec::new();
        for entry in sorted_read_dir_entries(&directory)? {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                child_directories.push(entry_path);
            } else if entry_path.is_file() {
                let item = local_stream_copy_item(
                    source_root,
                    entry_path,
                    &context.args.destination,
                    parent_prefix.as_deref(),
                )?;
                if pattern_allows(
                    &item.relative_key,
                    context.args.include.as_deref(),
                    context.args.exclude.as_deref(),
                ) {
                    enforce_transfer_plan_path_traversal(
                        context.global,
                        "ve-tos mv",
                        context.args.force,
                        true,
                        &context.args.source,
                        std::slice::from_ref(&item),
                    )?;
                    context.queue(item).await;
                }
            }
        }
        child_directories.sort();
        child_directories.reverse();
        pending.extend(child_directories);
    }
    Ok(())
}

async fn stream_mv_tos_source_no_manifest(
    context: &mut TosStreamMoveContext<'_>,
) -> Result<(), CliError> {
    let mut source_target = parse_tos_uri(&context.args.source, true)?;
    normalize_recursive_tos_target(&mut source_target);
    let source_prefix = source_target.key.clone().unwrap_or_default();
    let parent_prefix =
        recursive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let source_is_hns = bucket_is_hns(context.client, &source_target.bucket).await?;
    if resolve_tos_recursive_list_mode(source_is_hns, context.args.recursive_list_mode) {
        stream_mv_tos_hierarchical_source(
            context,
            &source_target,
            &source_prefix,
            parent_prefix.as_deref(),
        )
        .await
    } else {
        stream_mv_tos_flat_source(
            context,
            &source_target,
            &source_prefix,
            parent_prefix.as_deref(),
        )
        .await
    }
}

async fn stream_mv_tos_flat_source(
    context: &mut TosStreamMoveContext<'_>,
    source_target: &ParsedTosUri,
    source_prefix: &str,
    parent_prefix: Option<&str>,
) -> Result<(), CliError> {
    let mut continuation_token = None;
    loop {
        let page = list_object_entries_page(
            context.client,
            &source_target.bucket,
            Some(source_prefix),
            None,
            continuation_token.as_deref(),
        )
        .await?;
        continuation_token = page.next_token.clone();
        for entry in page.entries {
            queue_tos_stream_move_entry(
                context,
                source_target,
                source_prefix,
                parent_prefix,
                entry,
            )
            .await?;
        }
        if !page.is_truncated {
            break;
        }
    }
    Ok(())
}

async fn stream_mv_tos_hierarchical_source(
    context: &mut TosStreamMoveContext<'_>,
    source_target: &ParsedTosUri,
    source_prefix: &str,
    parent_prefix: Option<&str>,
) -> Result<(), CliError> {
    let mut pending_prefixes = vec![source_prefix.to_string()];
    let mut seen_prefixes = HashSet::new();
    let mut in_flight = FuturesUnordered::new();
    let limit = context.runtime.list_concurrency.max(1);
    while !pending_prefixes.is_empty() || !in_flight.is_empty() {
        while in_flight.len() < limit {
            let Some(current_prefix) = pending_prefixes.pop() else {
                break;
            };
            if !seen_prefixes.insert(current_prefix.clone()) {
                continue;
            }
            in_flight.push(scan_tos_entries_prefix(
                context.client,
                &source_target.bucket,
                current_prefix,
            ));
        }

        let Some(scan) = in_flight.next().await else {
            continue;
        };
        let scan = scan?;
        for child_prefix in scan.child_prefixes {
            if !seen_prefixes.contains(&child_prefix) {
                pending_prefixes.push(child_prefix);
            }
        }
        for entry in scan.entries {
            queue_tos_stream_move_entry(
                context,
                source_target,
                source_prefix,
                parent_prefix,
                entry,
            )
            .await?;
        }
    }
    Ok(())
}

async fn queue_tos_stream_move_entry(
    context: &mut TosStreamMoveContext<'_>,
    source_target: &ParsedTosUri,
    source_prefix: &str,
    parent_prefix: Option<&str>,
    entry: ObjectEntry,
) -> Result<(), CliError> {
    let item = tos_stream_copy_item(
        source_target,
        source_prefix,
        &context.args.destination,
        parent_prefix,
        entry,
    )?;
    if pattern_allows(
        &item.relative_key,
        context.args.include.as_deref(),
        context.args.exclude.as_deref(),
    ) {
        enforce_transfer_plan_path_traversal(
            context.global,
            "ve-tos mv",
            context.args.force,
            true,
            &context.args.source,
            std::slice::from_ref(&item),
        )?;
        context.queue(item).await;
    }
    Ok(())
}

async fn delete_move_source_item(
    client: &TosClient,
    item: &TransferPlanItem,
) -> Result<(), CliError> {
    if item.source.starts_with("tos://") {
        let source = parse_tos_uri(&item.source, false)?;
        let key = source.key.expect("validated object key");
        delete_tos_object(
            client,
            "ve-tos mv delete-source",
            &source.bucket,
            &key,
            item.etag.as_deref(),
        )
        .await
        .map(|_| ())
    } else {
        fs::remove_file(&item.source).map_err(CliError::Io)
    }
}

fn record_tos_stream_move_result(
    progress: &Option<ProgressBar>,
    report: &mut BatchReport,
    item: TransferPlanItem,
    copy_result: Result<CopyTransferResult, CliError>,
    delete_result: Option<Result<(), CliError>>,
) {
    if let Some(progress) = progress {
        progress.inc(1);
    }
    let copy_ok = match copy_result {
        Ok(result) if result.is_skipped() => {
            report.record_skipped("copy", &item.source, Some(&item.destination));
            true
        }
        Ok(_) => {
            report.record_success("copy", &item.source, Some(&item.destination));
            true
        }
        Err(ref err) => {
            report.record_failure("copy", &item.source, Some(&item.destination), err);
            eprintln!(
                "warn: mv copy failed source={} destination={} error={}",
                item.source, item.destination, err
            );
            false
        }
    };
    if !copy_ok {
        report.record_skipped("delete-source", &item.source, None);
        if let Some(progress) = progress {
            progress.inc(1);
        }
        return;
    }
    match delete_result {
        Some(Ok(())) => report.record_success("delete-source", &item.source, None),
        Some(Err(ref err)) => {
            report.record_failure("delete-source", &item.source, None, err);
            eprintln!("warn: failed to delete source {}: {}", item.source, err);
        }
        None => report.record_skipped("delete-source", &item.source, None),
    }
    if let Some(progress) = progress {
        progress.inc(1);
    }
}

fn execute_mv_recursive_local(
    global: &GlobalArgs,
    args: &MvArgs,
    report_path: Option<&str>,
    manifest_path: Option<&str>,
) -> Result<i32, CliError> {
    let cp_args = cp_args_from_mv(args, false, report_path.map(ToString::to_string));
    let runtime = effective_cp_runtime_config(global, &cp_args)?;
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let planned =
        build_local_source_mappings(&args.source, &args.destination, args.include_parent)?
            .into_iter()
            .filter(|item| {
                pattern_allows(
                    &item.relative_key,
                    args.include.as_deref(),
                    args.exclude.as_deref(),
                )
            })
            .collect::<Vec<_>>();
    enforce_transfer_plan_path_traversal(
        global,
        "ve-tos mv",
        args.force,
        true,
        &args.source,
        &planned,
    )?;
    let manifest = build_move_transfer_manifest(&planned);
    write_manifest_file(manifest_path, "ve-tos mv", &manifest)?;
    let mut report = BatchReport::new(manifest.object_count);

    for item in &planned {
        match copy_local_to_local(
            global,
            &item.source,
            &item.destination,
            runtime.copy_options(
                None,
                false,
                false,
                args.checkpoint_dir.as_deref(),
                ObjectWriteOptions::default(),
                progress_enabled,
                true,
                None,
            ),
        ) {
            Ok(result) => match result.outcome {
                CopyOutcome::Transferred => {
                    report.record_success("copy", &item.source, Some(&item.destination));
                }
                CopyOutcome::Skipped => {
                    report.record_skipped("copy", &item.source, Some(&item.destination));
                }
            },
            Err(err) => {
                report.record_failure("copy", &item.source, Some(&item.destination), &err);
            }
        }
    }

    if report.summary.failed > 0 {
        for item in &planned {
            report.record_skipped("delete-source", &item.source, None);
        }
    } else {
        for item in &planned {
            match fs::remove_file(&item.source) {
                Ok(()) => report.record_success("delete-source", &item.source, None),
                Err(err) => {
                    let err = CliError::Io(err);
                    report.record_failure("delete-source", &item.source, None, &err);
                }
            }
        }
        prune_empty_directories(Path::new(&args.source))?;
    }
    write_tos_batch_report(report_path, "ve-tos mv", &report, args.report_failures_only)?;
    output_tos_batch_envelope(
        global,
        "ve-tos mv",
        &args.source,
        &args.destination,
        report_path,
        manifest_path,
        &report,
    )?;
    if report.summary.failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn cp_args_from_mv(args: &MvArgs, checkpoint: bool, report_path: Option<String>) -> CpArgs {
    CpArgs {
        source: args.source.clone(),
        destination: args.destination.clone(),
        recursive: true,
        include_parent: args.include_parent,
        include: args.include.clone(),
        exclude: args.exclude.clone(),
        checkpoint,
        checkpoint_dir: args.checkpoint_dir.clone(),
        content_type: args.content_type.clone(),
        storage_class: args.storage_class.clone(),
        acl: args.acl.clone(),
        meta: args.meta.clone(),
        checkpoint_threshold: None,
        batch_concurrency: args.batch_concurrency,
        list_concurrency: args.list_concurrency,
        recursive_list_mode: args.recursive_list_mode,
        multipart_concurrency: args.multipart_concurrency,
        progress_granularity: args.progress_granularity,
        overwrite_strategy: Some(OverwriteStrategy::Force),
        report_path,
        report_failures_only: args.report_failures_only,
        manifest_path: args.manifest_path.clone(),
        no_manifest: args.no_manifest,
        bandwidth_limit: None,
        list_echo: args.list_echo,
        no_list_echo: args.no_list_echo,
        progress: args.progress,
        no_progress: args.no_progress,
        force: args.force,
        no_clobber: false,
    }
}

fn record_tos_copy_result(
    report: &mut BatchReport,
    summary: &mut BatchProgressSummary,
    operation: &str,
    source: String,
    destination: String,
    bytes: u64,
    result: Result<CopyTransferResult, CliError>,
) {
    match result {
        Ok(result) => match result.outcome {
            CopyOutcome::Transferred => {
                summary.record_success(bytes);
                report.record_success(operation, &source, Some(&destination));
            }
            CopyOutcome::Skipped => {
                summary.record_skip();
                summary.add_bytes(bytes);
                report.record_skipped(operation, &source, Some(&destination));
            }
        },
        Err(ref err) => {
            summary.record_failure(bytes);
            report.record_failure(operation, &source, Some(&destination), err);
            eprintln!(
                "warn: copy failed source={} destination={} error={}",
                source, destination, err
            );
        }
    }
}

async fn delete_recursive_move_sources_tos(
    client: &TosClient,
    args: &MvArgs,
    planned: &[TransferPlanItem],
    batch_concurrency: usize,
    progress_enabled: bool,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let use_hns_bottom_up = if args.source.starts_with("tos://") {
        let source = parse_tos_uri(&args.source, true)?;
        bucket_is_hns(client, &source.bucket).await?
    } else {
        false
    };
    let delete_items = ordered_recursive_move_delete_items(planned, use_hns_bottom_up);
    let del_bar = if progress_enabled && !planned.is_empty() {
        let pb = ProgressBar::new(planned.len() as u64);
        pb.set_style(
            ProgressStyle::with_template(
                "ve-tos mv delete-source [{bar:30.red/blue}] {pos}/{len} ({per_sec}, ETA {eta})",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-"),
        );
        pb.enable_steady_tick(Duration::from_millis(200));
        Some(pb)
    } else {
        None
    };

    if use_hns_bottom_up {
        delete_move_source_items_bottom_up(
            client,
            delete_items,
            batch_concurrency,
            &del_bar,
            report,
        )
        .await;
        delete_tos_recursive_move_source_root(client, args, planned, report).await;
    } else {
        delete_move_source_item_group(client, &delete_items, batch_concurrency, &del_bar, report)
            .await;
    }
    if !args.source.starts_with("tos://") {
        prune_empty_directories(Path::new(&args.source))?;
    }
    if let Some(bar) = del_bar {
        bar.finish();
    }
    Ok(())
}

async fn delete_move_source_items_bottom_up(
    client: &TosClient,
    items: Vec<&TransferPlanItem>,
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report: &mut BatchReport,
) {
    let (leaf_items, mut directory_items): (Vec<_>, Vec<_>) = items
        .into_iter()
        .partition(|item| !tos_delete_key_is_directory(&tos_move_delete_sort_key(item)));
    delete_move_source_item_group(client, &leaf_items, batch_concurrency, progress, report).await;
    directory_items.sort_by(|left, right| {
        let left_key = tos_move_delete_sort_key(left);
        let right_key = tos_move_delete_sort_key(right);
        key_depth(&right_key)
            .cmp(&key_depth(&left_key))
            .then_with(|| right_key.len().cmp(&left_key.len()))
            .then_with(|| left_key.cmp(&right_key))
    });
    let mut index = 0;
    while index < directory_items.len() {
        let depth = key_depth(&tos_move_delete_sort_key(directory_items[index]));
        let mut end = index + 1;
        while end < directory_items.len()
            && key_depth(&tos_move_delete_sort_key(directory_items[end])) == depth
        {
            end += 1;
        }
        delete_move_source_item_group(
            client,
            &directory_items[index..end],
            batch_concurrency,
            progress,
            report,
        )
        .await;
        index = end;
    }
}

async fn delete_move_source_item_group(
    client: &TosClient,
    items: &[&TransferPlanItem],
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report: &mut BatchReport,
) {
    let mut pending = items.iter().copied();
    let mut in_flight = FuturesUnordered::new();
    let limit = batch_concurrency.max(1);
    loop {
        while in_flight.len() < limit {
            let Some(item) = pending.next() else {
                break;
            };
            in_flight.push(async move {
                let result = delete_move_source_item(client, item).await;
                (item.source.clone(), result)
            });
        }
        let Some((source, result)) = in_flight.next().await else {
            break;
        };
        match result {
            Ok(()) => report.record_success("delete-source", &source, None),
            Err(err) => {
                eprintln!("warn: failed to delete source {}: {}", source, err);
                report.record_failure("delete-source", &source, None, &err);
            }
        }
        if let Some(bar) = progress {
            bar.inc(1);
        }
    }
}

async fn delete_tos_recursive_move_source_root(
    client: &TosClient,
    args: &MvArgs,
    planned: &[TransferPlanItem],
    report: &mut BatchReport,
) {
    let Some((bucket, key, source_uri)) = tos_recursive_move_source_root(args) else {
        return;
    };
    // [Review Fix #2] HNS listing may already include the source directory
    // marker as a planned item; deleting it twice turns an already-successful
    // move into a false partial failure.
    if planned.iter().any(|item| item.source == source_uri) {
        return;
    }
    match delete_tos_object(client, "ve-tos mv delete-source", &bucket, &key, None).await {
        Ok(_) => report.record_success("delete-source", &source_uri, None),
        Err(err) => {
            eprintln!("warn: failed to delete source {}: {}", source_uri, err);
            report.record_failure("delete-source", &source_uri, None, &err);
        }
    }
}

fn tos_recursive_move_source_root(args: &MvArgs) -> Option<(String, String, String)> {
    let source = parse_tos_uri(&args.source, true).ok()?;
    let raw_key = source.key?;
    let key = normalize_recursive_tos_prefix(Some(&raw_key));
    if key.is_empty() {
        return None;
    }
    let source_uri = format!("tos://{}/{}", source.bucket, key);
    Some((source.bucket, key, source_uri))
}

fn ordered_recursive_move_delete_items(
    planned: &[TransferPlanItem],
    use_hns_bottom_up: bool,
) -> Vec<&TransferPlanItem> {
    let mut items = planned.iter().collect::<Vec<_>>();
    if use_hns_bottom_up {
        // [Review Fix #HNS-Move-DeleteOrder] HNS directories must be removed
        // after their children when mv falls back to copy+delete.
        items.sort_by(|left, right| {
            let left_key = tos_move_delete_sort_key(left);
            let right_key = tos_move_delete_sort_key(right);
            key_depth(&right_key)
                .cmp(&key_depth(&left_key))
                .then_with(|| right_key.len().cmp(&left_key.len()))
                .then_with(|| left_key.cmp(&right_key))
        });
    }
    items
}

fn tos_move_delete_sort_key(item: &TransferPlanItem) -> String {
    if item.source.starts_with("tos://") {
        return parse_tos_uri(&item.source, false)
            .ok()
            .and_then(|target| target.key)
            .unwrap_or_else(|| item.source.clone());
    }
    item.source.clone()
}

fn output_tos_batch_envelope(
    global: &GlobalArgs,
    command: &'static str,
    source: &str,
    destination: &str,
    report_path: Option<&str>,
    manifest_path: Option<&str>,
    report: &BatchReport,
) -> Result<(), CliError> {
    output_result(
        global,
        &Envelope::success(
            command,
            json!({
                "source": source,
                "destination": destination,
                "succeeded": report.summary.succeeded,
                "failed": report.summary.failed,
                "skipped": report.summary.skipped,
                "report_path": report_path,
                "manifest_path": manifest_path,
                "status": if report.summary.failed == 0 { "succeeded" } else { "partial_failure" },
            }),
        ),
    )
}

fn output_single_transfer_envelope(
    global: &GlobalArgs,
    command: &'static str,
    operation: &'static str,
    source: &str,
    destination: &str,
    result: CopyTransferResult,
) -> Result<(), CliError> {
    output_result(
        global,
        &single_transfer_envelope(command, operation, source, destination, result),
    )
}

fn single_transfer_envelope(
    command: &'static str,
    operation: &'static str,
    source: &str,
    destination: &str,
    result: CopyTransferResult,
) -> Envelope<Value> {
    // [Review Fix #1] High-level single-file commands report the resolved
    // URI/path without dropping the service response data (status_code, ETag).
    let mut envelope = Envelope::success(
        command,
        single_transfer_payload(operation, source, destination, &result),
    );
    envelope.request_id = result.request_id.or(envelope.request_id);
    envelope.status_code = result.status_code;
    envelope.ec = result.ec;
    envelope
}

fn single_transfer_payload(
    operation: &'static str,
    source: &str,
    destination: &str,
    result: &CopyTransferResult,
) -> Value {
    let mut payload = match result.response_data.clone() {
        Some(Value::Object(map)) => map,
        Some(value) => {
            let mut map = serde_json::Map::new();
            map.insert("response".to_string(), value);
            map
        }
        None => serde_json::Map::new(),
    };
    payload.insert("operation".to_string(), json!(operation));
    payload.insert("source".to_string(), json!(source));
    payload.insert("destination".to_string(), json!(destination));
    payload.insert(
        "status".to_string(),
        json!(single_transfer_status(result.outcome)),
    );
    Value::Object(payload)
}

fn single_transfer_status(outcome: CopyOutcome) -> &'static str {
    match outcome {
        CopyOutcome::Transferred => "succeeded",
        CopyOutcome::Skipped => "skipped",
    }
}

fn status_code_from_payload(payload: &Value) -> Option<u16> {
    payload
        .get("status_code")
        .and_then(Value::as_u64)
        .and_then(|status_code| u16::try_from(status_code).ok())
}

fn single_transfer_operation(source: &str, destination: &str) -> &'static str {
    match (
        source.starts_with("tos://"),
        destination.starts_with("tos://"),
    ) {
        (false, true) => "upload",
        (true, false) => "download",
        (true, true) => "copy",
        (false, false) => "local-copy",
    }
}

async fn execute_sync(
    global: &GlobalArgs,
    client: &TosClient,
    args: &SyncArgs,
) -> Result<i32, CliError> {
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let runtime = effective_sync_runtime_config(global, args)?;
    ensure_same_region_for_tos_uris(client, &args.source, &args.destination).await?;
    let validation_write_options = sync_write_options(args)?;
    ensure_tos_upload_storage_class_supported(
        "ve-tos sync",
        Some(&args.source),
        &args.destination,
        validation_write_options.storage_class.as_deref(),
    )?;
    if Path::new(&args.source).is_dir() && args.destination.starts_with("tos://") {
        let report_path =
            effective_report_path(global, args.report_path.as_deref(), "ve-tos sync")?;
        let manifest_path = effective_optional_manifest_path(
            global,
            args.manifest_path.as_deref(),
            args.no_manifest,
            "ve-tos sync",
        )?;
        return execute_sync_local_to_tos(
            global,
            client,
            args,
            runtime,
            report_path.as_deref(),
            manifest_path.as_deref(),
            progress_enabled,
        )
        .await;
    }
    if sync_is_recursive(args) {
        let report_path =
            effective_report_path(global, args.report_path.as_deref(), "ve-tos sync")?;
        let manifest_path = effective_optional_manifest_path(
            global,
            args.manifest_path.as_deref(),
            args.no_manifest,
            "ve-tos sync",
        )?;
        let (copied, skipped, deleted, failed) = execute_sync_recursive(
            global,
            client,
            args,
            runtime,
            report_path.as_deref(),
            manifest_path.as_deref(),
            progress_enabled,
        )
        .await?;
        output_result(
            global,
            &Envelope::success(
                "ve-tos sync",
                json!({
                    "source": args.source,
                    "destination": args.destination,
                    "copied": copied,
                    "skipped": skipped,
                    "deleted": deleted,
                    "failed": failed,
                    "manifest_path": manifest_path,
                    "report_path": report_path,
                    "status": if failed == 0 { "succeeded" } else { "partial_failure" },
                }),
            ),
        )?;
        return if failed == 0 { Ok(0) } else { Ok(1) };
    }
    reject_single_transfer_artifacts(
        "ve-tos sync",
        args.report_path.as_deref(),
        args.report_failures_only,
        args.manifest_path.as_deref(),
        args.no_manifest,
        args.batch_concurrency,
        args.list_concurrency,
    )?;
    let write_options = sync_write_options(args)?;
    copy_one(
        global,
        client,
        &args.source,
        &args.destination,
        runtime.copy_options(
            None,
            false,
            true,
            args.checkpoint_dir.as_deref(),
            write_options,
            progress_enabled,
            false,
            None,
        ),
    )
    .await?;
    Ok(0)
}

async fn execute_sync_local_to_tos(
    global: &GlobalArgs,
    client: &TosClient,
    args: &SyncArgs,
    runtime: TransferRuntimeConfig,
    report_path: Option<&str>,
    manifest_path: Option<&str>,
    progress_enabled: bool,
) -> Result<i32, CliError> {
    let source_root = Path::new(&args.source);
    let mut destination = parse_tos_uri(&args.destination, true)?;
    normalize_recursive_tos_target(&mut destination);
    let destination_prefix = destination.key.clone().unwrap_or_default();
    let parent_prefix = recursive_source_parent_prefix(&args.source, args.include_parent)?;
    let list_echo_enabled = effective_list_echo_enabled(global, args.list_echo, args.no_list_echo);
    let mut scan_progress =
        RemoteScanProgress::new(list_echo_enabled, "ve-tos sync plan", &args.destination);
    let files = collect_local_files(source_root)?;
    let destination_manifest =
        list_object_entries_for_bucket(client, &destination.bucket, Some(&destination_prefix))
            .await?
            .into_iter()
            .map(|entry| (entry.key.clone(), entry))
            .collect::<HashMap<_, _>>();
    let mut desired_keys = HashSet::new();

    let planned: Vec<_> = files
        .into_iter()
        .filter_map(|file| {
            let relative = file.strip_prefix(source_root).ok()?;
            let relative_key = relative.to_string_lossy().replace('\\', "/");
            if !pattern_allows(
                &relative_key,
                args.include.as_deref(),
                args.exclude.as_deref(),
            ) {
                return None;
            }
            let target_relative_key =
                prepend_parent_prefix(&relative_key, parent_prefix.as_deref());
            let key = join_tos_key(&destination_prefix, &target_relative_key);
            desired_keys.insert(key.clone());
            let local_size = fs::metadata(&file).ok()?.len();
            Some((file, key, local_size))
        })
        .collect();

    let mut extras = if args.delete {
        destination_manifest
            .values()
            .filter(|entry| {
                let relative_key = strip_tos_prefix(&entry.key, &destination_prefix);
                pattern_allows(
                    relative_key,
                    args.include.as_deref(),
                    args.exclude.as_deref(),
                ) && !desired_keys.contains(&entry.key)
            })
            .cloned()
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    if args.delete && !extras.is_empty() && bucket_is_hns(client, &destination.bucket).await? {
        // [Review Fix #1] HNS directory-marker objects must be deleted after their children.
        sort_tos_sync_delete_entries_bottom_up(&mut extras);
    }
    scan_progress.finish_with_count((planned.len() + extras.len()) as u64, "item(s)");
    let total_files = planned.len() as u64;
    let total_bytes: u64 = planned.iter().map(|(_, _, size)| *size).sum();
    let total_progress_units: u64 = planned
        .iter()
        .map(|(_, _, size)| progress_units_for_size(*size, runtime, true))
        .sum();
    let mut manifest_items = planned
        .iter()
        .map(|(file, key, size)| TransferManifestItem {
            operation: "copy",
            relative_key: strip_tos_prefix(key, &destination_prefix).to_string(),
            source: file.to_string_lossy().into_owned(),
            destination: Some(format!("tos://{}/{}", destination.bucket, key)),
            size: *size,
            etag: None,
            crc64: None,
            last_modified: None,
        })
        .collect::<Vec<_>>();
    manifest_items.extend(extras.iter().map(|entry| TransferManifestItem {
        operation: "delete-extra",
        relative_key: strip_tos_prefix(&entry.key, &destination_prefix).to_string(),
        source: format!("tos://{}/{}", destination.bucket, entry.key),
        destination: None,
        size: entry.size,
        etag: entry.etag.clone(),
        crc64: None,
        last_modified: entry.last_modified.clone(),
    }));
    let manifest = TransferManifest {
        object_count: manifest_items.len() as u64,
        total_size: total_bytes,
        items: manifest_items,
    };
    write_manifest_file(manifest_path, "ve-tos sync", &manifest)?;
    let mut report = BatchReport::new(manifest.object_count);
    let mut summary = BatchProgressSummary::new(
        "ve-tos sync",
        &args.source,
        &args.destination,
        report_path,
        manifest_path,
        total_files,
        total_progress_units,
        total_bytes,
        runtime.progress_granularity,
        progress_enabled,
    );
    let mut uploaded = 0_u64;
    let mut skipped = 0_u64;
    let write_options = sync_write_options(args)?;

    for (file, key, local_size) in &planned {
        if remote_sync_should_skip(*local_size, destination_manifest.get(key), args) {
            skipped += 1;
            summary.record_skip();
            summary.add_bytes(*local_size);
            report.record_skipped(
                "sync-copy",
                &file.to_string_lossy(),
                Some(&format!("tos://{}/{}", destination.bucket, key)),
            );
            continue;
        }
        let target = format!("tos://{}/{}", destination.bucket, key);
        summary.set_current_file(&file.to_string_lossy());
        let result = copy_one(
            global,
            client,
            file.to_string_lossy().as_ref(),
            &target,
            runtime.copy_options(
                None,
                false,
                true,
                args.checkpoint_dir.as_deref(),
                write_options.clone(),
                progress_enabled,
                true,
                summary.overall.as_ref(),
            ),
        )
        .await;
        match result {
            Ok(result) => match result.outcome {
                CopyOutcome::Transferred => {
                    summary.record_success(*local_size);
                    report.record_success("sync-copy", &file.to_string_lossy(), Some(&target));
                    uploaded += 1;
                }
                CopyOutcome::Skipped => {
                    summary.record_skip();
                    summary.add_bytes(*local_size);
                    report.record_skipped("sync-copy", &file.to_string_lossy(), Some(&target));
                    skipped += 1;
                }
            },
            Err(err) => {
                summary.record_failure(*local_size);
                report.record_failure("sync-copy", &file.to_string_lossy(), Some(&target), &err);
                eprintln!(
                    "warn: sync upload failed source={} error={}",
                    file.display(),
                    err
                );
            }
        }
    }
    if let Some(bar) = &summary.overall {
        bar.finish();
    }

    let mut deleted = 0_u64;
    if args.delete && report.summary.failed > 0 {
        // [Review Fix #2] `sync --delete` must never delete destination extras
        // after a failed copy phase; the report keeps those planned deletions as skipped.
        for entry in extras {
            let uri = format!("tos://{}/{}", destination.bucket, entry.key);
            report.record_skipped("delete-extra", &uri, None);
        }
    } else if args.delete {
        let del_total = extras.len() as u64;
        let del_bar = if progress_enabled && del_total > 0 {
            let pb = ProgressBar::new(del_total);
            pb.set_style(
                ProgressStyle::with_template(
                    "ve-tos sync --delete [{bar:30.red/blue}] {pos}/{len} ({per_sec}, ETA {eta})",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=>-"),
            );
            pb.enable_steady_tick(Duration::from_millis(200));
            Some(pb)
        } else {
            None
        };
        for entry in extras {
            let uri = format!("tos://{}/{}", destination.bucket, entry.key);
            match delete_tos_object(
                client,
                "ve-tos sync delete-extra",
                &destination.bucket,
                &entry.key,
                entry.etag.as_deref(),
            )
            .await
            {
                Ok(_) => {
                    deleted += 1;
                    report.record_success("delete-extra", &uri, None);
                }
                Err(err) => {
                    report.record_failure("delete-extra", &uri, None, &err);
                    eprintln!("warn: sync delete-extra failed {}: {}", uri, err);
                }
            }
            if let Some(b) = &del_bar {
                b.inc(1);
            }
        }
        if let Some(b) = del_bar {
            b.finish();
        }
    }
    write_tos_batch_report(
        report_path,
        "ve-tos sync",
        &report,
        args.report_failures_only,
    )?;

    output_result(
        global,
        &Envelope::success(
            "ve-tos sync",
            json!({
                "source": args.source,
                "destination": args.destination,
                "uploaded": uploaded,
                "skipped": skipped,
                "deleted": deleted,
                "failed": report.summary.failed,
                "manifest_path": manifest_path,
                "report_path": report_path,
                "status": if report.summary.failed == 0 { "succeeded" } else { "partial_failure" },
            }),
        ),
    )?;
    if report.summary.failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

async fn execute_cp_recursive(
    global: &GlobalArgs,
    client: &TosClient,
    args: &CpArgs,
) -> Result<i32, CliError> {
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let list_echo_enabled = effective_list_echo_enabled(global, args.list_echo, args.no_list_echo);
    let write_options = copy_write_options(args)?;
    ensure_tos_upload_storage_class_supported(
        "ve-tos cp",
        Some(&args.source),
        &args.destination,
        write_options.storage_class.as_deref(),
    )?;
    if args.no_manifest {
        let report_path = effective_report_path(global, args.report_path.as_deref(), "ve-tos cp")?;
        let manifest_path = effective_optional_manifest_path(
            global,
            args.manifest_path.as_deref(),
            args.no_manifest,
            "ve-tos cp",
        )?;
        let runtime = effective_cp_runtime_config(global, args)?;
        return execute_cp_recursive_streaming_no_manifest(
            global,
            client,
            args,
            TosStreamingCpConfig {
                runtime,
                write_options,
                report_path,
                manifest_path,
                progress_enabled,
            },
        )
        .await;
    }
    let runtime = effective_cp_runtime_config(global, args)?;
    let mut scan_progress =
        RemoteScanProgress::new(list_echo_enabled, "ve-tos cp plan", &args.source);
    let mappings = build_recursive_copy_mappings(
        client,
        &args.source,
        &args.destination,
        args.include_parent,
        args.recursive_list_mode,
        runtime.list_concurrency,
    )
    .await?;
    let planned: Vec<_> = mappings
        .into_iter()
        .filter(|item| {
            pattern_allows(
                &item.relative_key,
                args.include.as_deref(),
                args.exclude.as_deref(),
            )
        })
        .collect();
    enforce_transfer_plan_path_traversal(
        global,
        "ve-tos cp",
        args.force,
        false,
        &args.destination,
        &planned,
    )?;
    scan_progress.finish_with_count(planned.len() as u64, "item(s)");
    let report_path = effective_report_path(global, args.report_path.as_deref(), "ve-tos cp")?;
    let manifest_path = effective_optional_manifest_path(
        global,
        args.manifest_path.as_deref(),
        args.no_manifest,
        "ve-tos cp",
    )?;
    let total_files = planned.len() as u64;
    let total_bytes: u64 = planned.iter().map(|item| item.size).sum();
    let total_progress_units: u64 = planned
        .iter()
        .map(|item| progress_units_for_size(item.size, runtime, args.checkpoint))
        .sum();
    let manifest = build_transfer_manifest(&planned);
    write_manifest_file(manifest_path.as_deref(), "ve-tos cp", &manifest)?;
    let mut report = BatchReport::new(total_files);
    let mut summary = BatchProgressSummary::new(
        "ve-tos cp",
        &args.source,
        &args.destination,
        report_path.as_deref(),
        manifest_path.as_deref(),
        total_files,
        total_progress_units,
        total_bytes,
        runtime.progress_granularity,
        progress_enabled,
    );

    // Clone the overall bar (ProgressBar is Arc-based, clone is cheap) to avoid
    // borrowing `summary` while we also need to mutate it.
    let overall_bar_owned = summary.overall.clone();
    let overall_bar_ref = overall_bar_owned.as_ref();
    let mut in_flight = FuturesUnordered::new();
    // [Review Fix #2] Poll in-flight copies while feeding the queue. Waiting
    // for a permit before polling completed futures can deadlock at capacity.
    let max_in_flight = runtime.batch_concurrency.max(1);
    let mut pending = planned.into_iter();

    loop {
        while in_flight.len() < max_in_flight {
            let Some(item) = pending.next() else {
                break;
            };
            let source = item.source;
            let destination = item.destination;
            let bytes = item.size;
            summary.set_current_file(&source);

            let checkpoint_enabled = args.checkpoint;
            let checkpoint_dir = args.checkpoint_dir.as_deref();
            let write_options = write_options.clone();

            in_flight.push(async move {
                let result = copy_one(
                    global,
                    client,
                    &source,
                    &destination,
                    runtime.copy_options(
                        None,
                        false,
                        checkpoint_enabled,
                        checkpoint_dir,
                        write_options,
                        progress_enabled,
                        true,
                        overall_bar_ref,
                    ),
                )
                .await;
                (source, destination, bytes, result)
            });
        }

        let Some((src, dst, b, result)) = in_flight.next().await else {
            break;
        };
        record_tos_copy_result(&mut report, &mut summary, "copy", src, dst, b, result);
    }

    let exit_code = if summary.failed > 0 { 1 } else { 0 };
    write_tos_batch_report(
        report_path.as_deref(),
        "ve-tos cp",
        &report,
        args.report_failures_only,
    )?;
    summary.finish_and_emit(global)?;
    Ok(exit_code)
}

async fn execute_cp_recursive_streaming_no_manifest(
    global: &GlobalArgs,
    client: &TosClient,
    args: &CpArgs,
    config: TosStreamingCpConfig,
) -> Result<i32, CliError> {
    let TosStreamingCpConfig {
        runtime,
        write_options,
        report_path,
        manifest_path,
        progress_enabled,
    } = config;
    let progress = streaming_batch_progress(progress_enabled, "ve-tos cp");
    let mut report = BatchReport::new(0);
    // [Review Fix #8] Streaming discovery can fail after copy tasks were
    // queued, so always drain queued work and persist its report first.
    let stream_result = {
        let mut context = TosStreamCopyContext {
            global,
            client,
            args,
            runtime,
            write_options,
            progress: &progress,
            report_path: report_path.as_deref(),
            report: &mut report,
            in_flight: FuturesUnordered::new(),
            limit: runtime.batch_concurrency.max(1),
        };
        let result = if args.source.starts_with("tos://") {
            stream_cp_tos_source_no_manifest(&mut context).await
        } else {
            stream_cp_local_source_no_manifest(&mut context).await
        };
        context.drain_all().await;
        result
    };
    finish_streaming_progress(progress, report.summary.planned);
    let failed = report.summary.failed;
    write_tos_batch_report(
        report_path.as_deref(),
        "ve-tos cp",
        &report,
        args.report_failures_only,
    )?;
    stream_result?;
    output_tos_batch_envelope(
        global,
        "ve-tos cp",
        &args.source,
        &args.destination,
        report_path.as_deref(),
        manifest_path.as_deref(),
        &report,
    )?;
    if failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

struct TosStreamingCpConfig {
    runtime: TransferRuntimeConfig,
    write_options: ObjectWriteOptions,
    report_path: Option<String>,
    manifest_path: Option<String>,
    progress_enabled: bool,
}

struct TosStreamCopyContext<'a> {
    global: &'a GlobalArgs,
    client: &'a TosClient,
    args: &'a CpArgs,
    runtime: TransferRuntimeConfig,
    write_options: ObjectWriteOptions,
    progress: &'a Option<ProgressBar>,
    report_path: Option<&'a str>,
    report: &'a mut BatchReport,
    in_flight: FuturesUnordered<TosCopyFuture<'a>>,
    limit: usize,
}

impl<'a> TosStreamCopyContext<'a> {
    async fn queue(&mut self, item: TransferPlanItem) {
        self.report.summary.planned += 1;
        while self.in_flight.len() >= self.limit {
            if !self.drain_one().await {
                break;
            }
        }
        let source = item.source;
        let destination = item.destination;
        let bytes = item.size;
        let runtime = self.runtime;
        let global = self.global;
        let client = self.client;
        let report_path = self.report_path;
        let report_failures_only = self.args.report_failures_only;
        let checkpoint_enabled = self.args.checkpoint;
        let checkpoint_dir = self.args.checkpoint_dir.as_deref();
        let write_options = self.write_options.clone();
        self.in_flight.push(Box::pin(async move {
            let result = copy_one(
                global,
                client,
                &source,
                &destination,
                runtime.copy_options(
                    report_path,
                    report_failures_only,
                    checkpoint_enabled,
                    checkpoint_dir,
                    write_options,
                    false,
                    true,
                    None,
                ),
            )
            .await;
            (source, destination, bytes, result)
        }));
    }

    async fn drain_one(&mut self) -> bool {
        let Some((source, destination, bytes, result)) = self.in_flight.next().await else {
            return false;
        };
        record_tos_stream_copy_result(
            self.progress,
            self.report,
            source,
            destination,
            bytes,
            result,
        );
        true
    }

    async fn drain_all(&mut self) {
        while self.drain_one().await {}
    }
}

async fn stream_cp_local_source_no_manifest(
    context: &mut TosStreamCopyContext<'_>,
) -> Result<(), CliError> {
    let source_root = Path::new(&context.args.source);
    if !source_root.is_dir() {
        return Err(CliError::ValidationError(format!(
            "recursive source '{}' must be a local directory or tos:// prefix",
            context.args.source
        )));
    }
    let parent_prefix =
        recursive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let mut pending = vec![source_root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let mut child_directories = Vec::new();
        for entry in sorted_read_dir_entries(&directory)? {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                child_directories.push(entry_path);
            } else if entry_path.is_file() {
                let item = local_stream_copy_item(
                    source_root,
                    entry_path,
                    &context.args.destination,
                    parent_prefix.as_deref(),
                )?;
                if pattern_allows(
                    &item.relative_key,
                    context.args.include.as_deref(),
                    context.args.exclude.as_deref(),
                ) {
                    enforce_transfer_plan_path_traversal(
                        context.global,
                        "ve-tos cp",
                        context.args.force,
                        false,
                        &context.args.destination,
                        std::slice::from_ref(&item),
                    )?;
                    context.queue(item).await;
                }
            }
        }
        child_directories.sort();
        child_directories.reverse();
        pending.extend(child_directories);
    }
    Ok(())
}

async fn stream_cp_tos_source_no_manifest(
    context: &mut TosStreamCopyContext<'_>,
) -> Result<(), CliError> {
    let mut source_target = parse_tos_uri(&context.args.source, true)?;
    normalize_recursive_tos_target(&mut source_target);
    let source_prefix = source_target.key.clone().unwrap_or_default();
    let parent_prefix =
        recursive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let source_is_hns = bucket_is_hns(context.client, &source_target.bucket).await?;
    if resolve_tos_recursive_list_mode(source_is_hns, context.args.recursive_list_mode) {
        stream_cp_tos_hierarchical_source(
            context,
            &source_target,
            &source_prefix,
            parent_prefix.as_deref(),
        )
        .await
    } else {
        stream_cp_tos_flat_source(
            context,
            &source_target,
            &source_prefix,
            parent_prefix.as_deref(),
        )
        .await
    }
}

async fn stream_cp_tos_flat_source(
    context: &mut TosStreamCopyContext<'_>,
    source_target: &ParsedTosUri,
    source_prefix: &str,
    parent_prefix: Option<&str>,
) -> Result<(), CliError> {
    let mut continuation_token = None;
    loop {
        let page = list_object_entries_page(
            context.client,
            &source_target.bucket,
            Some(source_prefix),
            None,
            continuation_token.as_deref(),
        )
        .await?;
        continuation_token = page.next_token.clone();
        for entry in page.entries {
            queue_tos_stream_copy_entry(
                context,
                source_target,
                source_prefix,
                parent_prefix,
                entry,
            )
            .await?;
        }
        if !page.is_truncated {
            break;
        }
    }
    Ok(())
}

async fn stream_cp_tos_hierarchical_source(
    context: &mut TosStreamCopyContext<'_>,
    source_target: &ParsedTosUri,
    source_prefix: &str,
    parent_prefix: Option<&str>,
) -> Result<(), CliError> {
    let mut pending_prefixes = vec![source_prefix.to_string()];
    let mut seen_prefixes = HashSet::new();
    let mut in_flight = FuturesUnordered::new();
    let limit = context.runtime.list_concurrency.max(1);
    while !pending_prefixes.is_empty() || !in_flight.is_empty() {
        while in_flight.len() < limit {
            let Some(current_prefix) = pending_prefixes.pop() else {
                break;
            };
            if !seen_prefixes.insert(current_prefix.clone()) {
                continue;
            }
            in_flight.push(scan_tos_entries_prefix(
                context.client,
                &source_target.bucket,
                current_prefix,
            ));
        }

        let Some(scan) = in_flight.next().await else {
            continue;
        };
        let scan = scan?;
        for child_prefix in scan.child_prefixes {
            if !seen_prefixes.contains(&child_prefix) {
                pending_prefixes.push(child_prefix);
            }
        }
        for entry in scan.entries {
            queue_tos_stream_copy_entry(
                context,
                source_target,
                source_prefix,
                parent_prefix,
                entry,
            )
            .await?;
        }
    }
    Ok(())
}

async fn queue_tos_stream_copy_entry(
    context: &mut TosStreamCopyContext<'_>,
    source_target: &ParsedTosUri,
    source_prefix: &str,
    parent_prefix: Option<&str>,
    entry: ObjectEntry,
) -> Result<(), CliError> {
    let item = tos_stream_copy_item(
        source_target,
        source_prefix,
        &context.args.destination,
        parent_prefix,
        entry,
    )?;
    if pattern_allows(
        &item.relative_key,
        context.args.include.as_deref(),
        context.args.exclude.as_deref(),
    ) {
        enforce_transfer_plan_path_traversal(
            context.global,
            "ve-tos cp",
            context.args.force,
            false,
            &context.args.destination,
            std::slice::from_ref(&item),
        )?;
        context.queue(item).await;
    }
    Ok(())
}

fn sorted_read_dir_entries(path: &Path) -> Result<Vec<fs::DirEntry>, CliError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        entries.push(entry?);
    }
    entries.sort_by_key(|entry| entry.path());
    Ok(entries)
}

fn local_stream_copy_item(
    source_root: &Path,
    file: PathBuf,
    destination: &str,
    parent_prefix: Option<&str>,
) -> Result<TransferPlanItem, CliError> {
    let relative = file.strip_prefix(source_root).map_err(|err| {
        CliError::ValidationError(format!("failed to derive relative path: {}", err))
    })?;
    let source_relative_key = relative.to_string_lossy().replace('\\', "/");
    let relative_key = prepend_parent_prefix(&source_relative_key, parent_prefix);
    let target = if destination.starts_with("tos://") {
        let target = parse_tos_uri(destination, true)?;
        format!(
            "tos://{}/{}",
            target.bucket,
            join_tos_key(target.key.as_deref().unwrap_or(""), &relative_key)
        )
    } else {
        Path::new(destination)
            .join(relative)
            .to_string_lossy()
            .into_owned()
    };
    let size = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
    Ok(TransferPlanItem {
        relative_key,
        source: file.to_string_lossy().into_owned(),
        destination: target,
        size,
        etag: None,
        crc64: None,
        last_modified: None,
    })
}

fn tos_stream_copy_item(
    source_target: &ParsedTosUri,
    source_prefix: &str,
    destination: &str,
    parent_prefix: Option<&str>,
    entry: ObjectEntry,
) -> Result<TransferPlanItem, CliError> {
    let source_relative_key = strip_tos_prefix(&entry.key, source_prefix).to_string();
    let relative_key = prepend_parent_prefix(&source_relative_key, parent_prefix);
    let source_uri = format!("tos://{}/{}", source_target.bucket, entry.key);
    let target = if destination.starts_with("tos://") {
        let destination_target = parse_tos_uri(destination, true)?;
        format!(
            "tos://{}/{}",
            destination_target.bucket,
            join_tos_key(
                destination_target.key.as_deref().unwrap_or(""),
                &relative_key
            )
        )
    } else {
        Path::new(destination)
            .join(relative_key.replace('/', std::path::MAIN_SEPARATOR_STR))
            .to_string_lossy()
            .into_owned()
    };
    Ok(TransferPlanItem {
        relative_key,
        source: source_uri,
        destination: target,
        size: entry.size,
        etag: entry.etag,
        crc64: None,
        last_modified: entry.last_modified,
    })
}

fn record_tos_stream_copy_result(
    progress: &Option<ProgressBar>,
    report: &mut BatchReport,
    source: String,
    destination: String,
    _bytes: u64,
    result: Result<CopyTransferResult, CliError>,
) {
    if let Some(progress) = progress {
        progress.inc(1);
    }
    match result {
        Ok(result) if result.is_skipped() => {
            report.record_skipped("copy", &source, Some(&destination));
        }
        Ok(_) => {
            report.record_success("copy", &source, Some(&destination));
        }
        Err(ref err) => {
            report.record_failure("copy", &source, Some(&destination), err);
            eprintln!(
                "warn: copy failed source={} destination={} error={}",
                source, destination, err
            );
        }
    }
}

async fn execute_sync_recursive(
    global: &GlobalArgs,
    client: &TosClient,
    args: &SyncArgs,
    runtime: TransferRuntimeConfig,
    report_path: Option<&str>,
    manifest_path: Option<&str>,
    progress_enabled: bool,
) -> Result<(u64, u64, u64, u64), CliError> {
    let list_echo_enabled = effective_list_echo_enabled(global, args.list_echo, args.no_list_echo);
    let mut scan_progress =
        RemoteScanProgress::new(list_echo_enabled, "ve-tos sync plan", &args.source);
    let mappings = build_recursive_copy_mappings(
        client,
        &args.source,
        &args.destination,
        args.include_parent,
        args.recursive_list_mode,
        runtime.list_concurrency,
    )
    .await?;
    let planned: Vec<_> = mappings
        .into_iter()
        .filter(|item| {
            pattern_allows(
                &item.relative_key,
                args.include.as_deref(),
                args.exclude.as_deref(),
            )
        })
        .collect();
    enforce_transfer_plan_path_traversal(
        global,
        "ve-tos sync",
        args.force,
        args.delete,
        &args.destination,
        &planned,
    )?;
    let delete_plan = if args.delete {
        build_sync_delete_plan(client, args, runtime.list_concurrency).await?
    } else {
        Vec::new()
    };
    scan_progress.finish_with_count((planned.len() + delete_plan.len()) as u64, "item(s)");
    let total_files = planned.len() as u64;
    let total_bytes: u64 = planned.iter().map(|item| item.size).sum();
    let total_progress_units: u64 = planned
        .iter()
        .map(|item| progress_units_for_size(item.size, runtime, true))
        .sum();
    let mut manifest_items = build_transfer_manifest(&planned).items;
    manifest_items.extend(delete_plan.clone());
    let manifest = TransferManifest::from_items(manifest_items);
    write_manifest_file(manifest_path, "ve-tos sync", &manifest)?;
    let mut report = BatchReport::new(manifest.object_count);
    let mut summary = BatchProgressSummary::new(
        "ve-tos sync",
        &args.source,
        &args.destination,
        report_path,
        manifest_path,
        total_files,
        total_progress_units,
        total_bytes,
        runtime.progress_granularity,
        progress_enabled,
    );
    let mut copied = 0_u64;
    let mut skipped = 0_u64;
    let write_options = sync_write_options(args)?;

    let overall_bar_owned = summary.overall.clone();
    let overall_bar_ref = overall_bar_owned.as_ref();
    let mut in_flight = FuturesUnordered::new();
    // [Review Fix #2] Sync still does skip checks sequentially, but transfer
    // work uses a bounded queue that drains as each copy completes.
    let max_in_flight = runtime.batch_concurrency.max(1);
    let mut pending = planned.into_iter();

    loop {
        while in_flight.len() < max_in_flight {
            let Some(item) = pending.next() else {
                break;
            };
            let source = item.source;
            let destination = item.destination;
            let bytes = item.size;
            // Skip check must be sequential (it may HEAD the object).
            if sync_mapping_should_skip(client, &source, &destination, args).await? {
                skipped += 1;
                summary.record_skip();
                summary.add_bytes(bytes);
                report.record_skipped("sync-copy", &source, Some(&destination));
                continue;
            }

            summary.set_current_file(&source);

            let checkpoint_dir = args.checkpoint_dir.as_deref();
            let write_options = write_options.clone();

            in_flight.push(async move {
                let result = copy_one(
                    global,
                    client,
                    &source,
                    &destination,
                    runtime.copy_options(
                        None,
                        false,
                        true,
                        checkpoint_dir,
                        write_options,
                        progress_enabled,
                        true,
                        overall_bar_ref,
                    ),
                )
                .await;
                (source, destination, bytes, result)
            });
        }

        let Some((src, dst, b, result)) = in_flight.next().await else {
            break;
        };
        match result {
            Ok(result) => match result.outcome {
                CopyOutcome::Transferred => {
                    summary.record_success(b);
                    report.record_success("sync-copy", &src, Some(&dst));
                    copied += 1;
                }
                CopyOutcome::Skipped => {
                    summary.record_skip();
                    summary.add_bytes(b);
                    report.record_skipped("sync-copy", &src, Some(&dst));
                    skipped += 1;
                }
            },
            Err(ref err) => {
                summary.record_failure(b);
                report.record_failure("sync-copy", &src, Some(&dst), err);
                eprintln!(
                    "warn: sync failed source={} destination={} error={}",
                    src, dst, err
                );
            }
        }
    }

    if let Some(bar) = &summary.overall {
        bar.finish();
    }
    let deleted = if report.summary.failed == 0 {
        execute_sync_delete_plan(
            client,
            delete_plan,
            runtime.batch_concurrency,
            progress_enabled,
            &mut report,
        )
        .await?
    } else {
        // [Review Fix #3] Remote/list-based sync follows the same safety gate:
        // if any copy/update item failed, all `delete-extra` rows are skipped.
        for item in delete_plan {
            report.record_skipped("delete-extra", &item.source, None);
        }
        0
    };
    let failed = report.summary.failed;
    write_tos_batch_report(
        report_path,
        "ve-tos sync",
        &report,
        args.report_failures_only,
    )?;
    Ok((copied, skipped, deleted, failed))
}

async fn sync_mapping_should_skip(
    client: &TosClient,
    source: &str,
    destination: &str,
    args: &SyncArgs,
) -> Result<bool, CliError> {
    match (
        source.starts_with("tos://"),
        destination.starts_with("tos://"),
    ) {
        (true, false) => {
            let Some(source_entry) = object_entry_from_head(client, source).await? else {
                return Ok(false);
            };
            Ok(local_destination_matches_remote(
                destination,
                &source_entry,
                args,
            )?)
        }
        (true, true) => {
            let Some(source_entry) = object_entry_from_head(client, source).await? else {
                return Ok(false);
            };
            let Some(destination_entry) = object_entry_from_head(client, destination).await? else {
                return Ok(false);
            };
            Ok(tos_entries_match_for_sync(
                &source_entry,
                &destination_entry,
                args,
            ))
        }
        (false, true) => {
            let local_size = fs::metadata(source)?.len();
            let destination_entry = object_entry_from_head(client, destination).await?;
            Ok(remote_sync_should_skip(
                local_size,
                destination_entry.as_ref(),
                args,
            ))
        }
        (false, false) => local_sync_should_skip(source, destination, args),
    }
}

async fn copy_one(
    global: &GlobalArgs,
    client: &TosClient,
    source: &str,
    destination: &str,
    options: CopyOptions<'_>,
) -> Result<CopyTransferResult, CliError> {
    let outcome = match (
        source.starts_with("tos://"),
        destination.starts_with("tos://"),
    ) {
        (false, true) => upload_local_to_tos(global, client, source, destination, options).await?,
        (true, false) => {
            download_tos_to_local(global, client, source, destination, options).await?
        }
        (true, true) => copy_tos_to_tos(global, client, source, destination, options).await?,
        (false, false) => copy_local_to_local(global, source, destination, options)?,
    };
    Ok(outcome)
}

async fn upload_local_to_tos(
    global: &GlobalArgs,
    client: &TosClient,
    source: &str,
    destination: &str,
    options: CopyOptions<'_>,
) -> Result<CopyTransferResult, CliError> {
    let target = parse_tos_uri(destination, false)?;
    let key = target.key.expect("validated object key");
    let metadata = fs::metadata(source)?;
    let file_size = metadata.len();
    if should_skip_upload_for_overwrite_strategy(
        client,
        source,
        &target.bucket,
        &key,
        &metadata,
        &options,
    )
    .await?
    {
        write_single_report(
            success_report_path(options.report_path, options.report_failures_only),
            "upload",
            source,
            Some(destination),
            "skipped",
        )?;
        return Ok(CopyTransferResult::skipped());
    }
    if should_use_multipart(
        file_size,
        options.checkpoint_enabled,
        options.checkpoint_threshold,
    ) {
        return upload_local_to_tos_multipart(
            global,
            client,
            source,
            &target.bucket,
            &key,
            options,
        )
        .await;
    }
    // [Review Fix #8] Simple upload streams hash/CRC/body instead of buffering the file.
    let payload_hash = file_sha256(source)?;
    let local_crc64 = file_crc64(source)?;
    let headers = cp_simple_upload_headers(
        &options.write_options,
        local_crc64,
        file_size,
        options.overwrite_strategy,
    );
    // [Review Fix #Progress-PartGranular] simple PUT 没有 part 维度，所以仅在
    // 请求结束时一次性推到 file_size。indicatif 在非 TTY/--no-progress/--quiet
    // 下自动降级为 NoOp。
    // [Review Fix #Progress-Overall] 当被批量上层驱动时（options.overall_bar=Some），
    // 跳过 per-file 进度条避免双层渲染冲突，直接通过 overall_bar.inc 推进整体进度。
    let progress = if options.overall_bar.is_none() {
        FileProgress::new(
            options.progress_enabled,
            &short_label_from(source),
            file_size,
        )
    } else {
        FileProgress { bar: None }
    };
    let response = match core::execute_object_streaming_request(
        client,
        "ve-tos cp upload",
        Method::PUT,
        &target.bucket,
        &key,
        BTreeMap::new(),
        headers,
        payload_hash,
        file_stream_body(source).await?,
    )
    .await
    {
        Ok(resp) => {
            progress.set_position(file_size);
            progress.finish(true);
            // [Review Fix #Progress-Overall] simple PUT 在成功后一次性推进 overall bar。
            if options.progress_granularity == EffectiveProgressGranularity::Byte {
                if let Some(bar) = options.overall_bar {
                    bar.inc(file_size);
                }
            } else if let Some(bar) = options.overall_bar {
                bar.inc(1);
            }
            resp
        }
        Err(err) => {
            progress.finish(false);
            return Err(err);
        }
    };
    if let Some(data) = &response.data {
        if let Some(remote_crc64) = find_crc64_header(&data.headers) {
            if remote_crc64 != local_crc64 {
                return Err(CliError::TransferFailed(format!(
                    "CRC64 mismatch for '{}': local={}, remote={}",
                    source, local_crc64, remote_crc64
                )));
            }
        }
    }
    // [Review Fix #Recursive-Summary] 批量场景下静默单文件 envelope，由调用方汇总。
    // --verbose（global.verbose）保持原行为，便于排障。
    if !options.silent_per_file || global.verbose {
        output_result(global, &response)?;
    }
    write_single_report(
        success_report_path(options.report_path, options.report_failures_only),
        "upload",
        source,
        Some(destination),
        "succeeded",
    )?;
    CopyTransferResult::from_raw_response(&response)
}

async fn upload_local_to_tos_multipart(
    global: &GlobalArgs,
    client: &TosClient,
    source: &str,
    bucket: &str,
    key: &str,
    options: CopyOptions<'_>,
) -> Result<CopyTransferResult, CliError> {
    let metadata = fs::metadata(source)?;
    let file_size = metadata.len();
    let part_size = multipart_part_size(file_size);
    let file_mtime = file_mtime_nanos(&metadata)?;
    let local_object_crc64 = file_crc64(source)?;
    let checkpoint_path = checkpoint_path(
        options.checkpoint_dir,
        source,
        bucket,
        key,
        file_size,
        file_mtime,
        part_size,
        &options.write_options.checkpoint_fingerprint(),
        &global.profile,
        &client.service_endpoint(),
    )?;
    let _lock = CheckpointLock::acquire(&checkpoint_path)?;

    let mut checkpoint = load_checkpoint(&checkpoint_path)?.unwrap_or_else(|| Checkpoint {
        bucket: bucket.to_string(),
        key: key.to_string(),
        source_path: Some(source.to_string()),
        file_size,
        part_size,
        upload_id: None,
        completed_parts: Vec::new(),
    });

    if checkpoint.file_size != file_size || checkpoint.part_size != part_size {
        return Err(CliError::ValidationError(
            "checkpoint metadata does not match the local file; remove the checkpoint to restart"
                .to_string(),
        ));
    }

    if checkpoint.upload_id.is_none() {
        // [Review Fix #NoClobber] Only prevent overwrite when --no-clobber is specified.
        let mut create_headers = options.write_options.headers(false);
        if options.overwrite_strategy == EffectiveOverwriteStrategy::NoClobber {
            create_headers.insert("if-none-match".to_string(), "*".to_string());
        }
        let created = core::execute_object_request(
            client,
            "ve-tos cp multipart create",
            Method::POST,
            bucket,
            key,
            BTreeMap::from([("uploads".to_string(), String::new())]),
            create_headers,
            None,
        )
        .await?;
        checkpoint.upload_id = extract_upload_id(&created);
        if checkpoint.upload_id.is_none() {
            return Err(CliError::ValidationError(
                "CreateMultipartUpload response did not include UploadId".to_string(),
            ));
        }
        save_checkpoint(&checkpoint_path, &checkpoint)?;
    }

    let upload_id = checkpoint.upload_id.clone().expect("validated upload id");
    // [Review Fix #3] Reconcile local checkpoint with server-side parts before resuming.
    let remote_parts = list_uploaded_part_numbers(client, bucket, key, &upload_id).await?;
    checkpoint
        .completed_parts
        .retain(|part| remote_parts.contains(&part.part_number));
    save_checkpoint(&checkpoint_path, &checkpoint)?;

    // [Review Fix #Progress-PartGranular] Part 粒度文件级进度条 — 与 checkpoint 对齐：
    // 已完成 parts 的字节数即时设为进度条的初始 position，剩余 parts 每完成一个 inc(part_size)。
    // [Review Fix #Progress-Overall] 批量上层驱动时跳过 per-file 进度条；overall bar
    // 在每个 part 完成时累加 current_size，断点续传场景下的初始已完成字节也一并推进。
    let progress = if options.overall_bar.is_none() {
        FileProgress::new(
            options.progress_enabled,
            &short_label_from(source),
            file_size,
        )
    } else {
        FileProgress { bar: None }
    };
    let initial_completed_bytes: u64 = checkpoint
        .completed_parts
        .iter()
        .map(|part| {
            let part_offset = (part.part_number as u64 - 1) * part_size;
            (file_size - part_offset).min(part_size)
        })
        .sum();
    progress.set_position(initial_completed_bytes);
    if options.progress_granularity == EffectiveProgressGranularity::Byte {
        if let Some(bar) = options.overall_bar {
            // [Review Fix #Progress-Overall] checkpoint 恢复场景：已完成的 parts 不会
            // 再次触发 inc，但属于本文件的总字节，所以一次性推进到 overall。
            bar.inc(initial_completed_bytes);
        }
    } else if let Some(bar) = options.overall_bar {
        bar.inc(checkpoint.completed_parts.len() as u64);
    }

    let mut pending_parts = Vec::new();
    let mut part_number = 1_u32;
    let mut offset = 0_u64;
    while offset < file_size {
        let current_size = (file_size - offset).min(part_size);
        if checkpoint
            .completed_parts
            .iter()
            .any(|part| part.part_number == part_number)
        {
            part_number += 1;
            offset += current_size;
            continue;
        }
        pending_parts.push((part_number, offset, current_size));
        // [Review Fix #2] Keep stdin multipart numbering bounded instead of
        // relying on integer overflow behavior for extremely large streams.
        part_number = part_number.checked_add(1).ok_or_else(|| {
            CliError::ValidationError("stdin multipart upload has too many parts".to_string())
        })?;
        offset += current_size;
    }

    let mut pending_parts = pending_parts.into_iter();
    let mut in_flight = FuturesUnordered::new();
    loop {
        while in_flight.len() < options.multipart_concurrency {
            let Some((part_number, part_offset, current_size)) = pending_parts.next() else {
                break;
            };
            in_flight.push(upload_tos_multipart_part(
                client,
                source,
                bucket,
                key,
                &upload_id,
                part_number,
                part_offset,
                current_size,
            ));
        }

        let Some(result) = in_flight.next().await else {
            break;
        };
        let (completed_part, current_size) = result?;
        let completed_part_number = completed_part.part_number;
        checkpoint.completed_parts.push(completed_part);
        checkpoint
            .completed_parts
            .sort_by_key(|part| part.part_number);
        save_checkpoint(&checkpoint_path, &checkpoint)?;
        progress.inc(current_size);
        if options.progress_granularity == EffectiveProgressGranularity::Byte {
            if let Some(bar) = options.overall_bar {
                bar.inc(current_size);
            }
        } else if let Some(bar) = options.overall_bar {
            bar.inc(1);
        }
        emit_progress(
            options.progress_enabled,
            "upload-part",
            &format!("tos://{}/{}#{}", bucket, key, completed_part_number),
        );
    }

    let (complete_query, complete_headers, complete_body) =
        complete_multipart_request(&upload_id, &checkpoint.completed_parts)?;
    let completed = core::execute_object_request(
        client,
        "ve-tos cp multipart complete",
        Method::POST,
        bucket,
        key,
        complete_query,
        complete_headers,
        Some(complete_body),
    )
    .await?;
    if let Some(headers) = completed.data.as_ref().map(|data| &data.headers) {
        if let Some(remote_crc64) = find_crc64_header(headers) {
            if remote_crc64 != local_object_crc64 {
                return Err(CliError::TransferFailed(format!(
                    "multipart complete CRC64 mismatch: local={}, remote={}",
                    local_object_crc64, remote_crc64
                )));
            }
        }
    }
    if !options.silent_per_file || global.verbose {
        output_result(global, &completed)?;
    }
    // [Review Fix #Progress-PartGranular] 多分片成功路径关闭进度条；失败路径走 ? 直接 return，
    // 进度条会随 FileProgress drop 自然停止（abandon 仅用于显式失败语义）。
    progress.finish(true);
    remove_checkpoint(&checkpoint_path)?;
    write_single_report(
        success_report_path(options.report_path, options.report_failures_only),
        "multipart-upload",
        source,
        Some(&format!("tos://{}/{}", bucket, key)),
        "succeeded",
    )?;
    CopyTransferResult::from_raw_response(&completed)
}

async fn upload_tos_multipart_part(
    client: &TosClient,
    source: &str,
    bucket: &str,
    key: &str,
    upload_id: &str,
    part_number: u32,
    offset: u64,
    current_size: u64,
) -> Result<(CompletedPart, u64), CliError> {
    let local_crc64 = file_part_crc64(source, offset, current_size)?;
    let uploaded = core::execute_object_streaming_request(
        client,
        "ve-tos cp multipart upload-part",
        Method::PUT,
        bucket,
        key,
        multipart_part_query(upload_id, part_number),
        cp_multipart_upload_part_headers(local_crc64, current_size),
        file_part_sha256(source, offset, current_size)?,
        file_part_stream_body(source, offset, current_size).await?,
    )
    .await?;

    let headers = uploaded
        .data
        .as_ref()
        .map(|data| &data.headers)
        .ok_or_else(|| CliError::ValidationError("UploadPart missing response data".to_string()))?;
    if let Some(remote_crc64) = find_crc64_header(headers) {
        if remote_crc64 != local_crc64 {
            return Err(CliError::TransferFailed(format!(
                "CRC64 mismatch for part {}: local={}, remote={}",
                part_number, local_crc64, remote_crc64
            )));
        }
    }
    let etag = upload_part_etag(headers)
        .ok_or_else(|| CliError::ValidationError("UploadPart response missing ETag".to_string()))?;
    Ok((
        CompletedPart {
            part_number,
            etag,
            crc64: Some(local_crc64),
        },
        current_size,
    ))
}

fn cp_simple_upload_headers(
    write_options: &ObjectWriteOptions,
    local_crc64: u64,
    file_size: u64,
    overwrite_strategy: EffectiveOverwriteStrategy,
) -> BTreeMap<String, String> {
    let mut headers = write_options.headers(false);
    // [Review Fix #1] PSM-backed ByteTOS rejects transfer-chunk bodies, so
    // high-level cp must send fixed-length PutObject requests like object upload.
    headers.insert("content-length".to_string(), file_size.to_string());
    headers.insert("x-hash-crc64ecma".to_string(), local_crc64.to_string());
    // [Review Fix #NoClobber] Only set if-none-match when --no-clobber is specified.
    // Default behavior: overwrite existing objects (aligned with aws s3 cp).
    if overwrite_strategy == EffectiveOverwriteStrategy::NoClobber {
        headers.insert("if-none-match".to_string(), "*".to_string());
    }
    headers
}

fn cp_multipart_upload_part_headers(
    local_crc64: u64,
    current_size: u64,
) -> BTreeMap<String, String> {
    BTreeMap::from([
        // [Review Fix #2] UploadPart uses a bounded file slice, so pass its
        // exact length to avoid chunked transfer on PSM-backed ByteTOS.
        ("content-length".to_string(), current_size.to_string()),
        ("x-hash-crc64ecma".to_string(), local_crc64.to_string()),
    ])
}

async fn download_tos_to_local(
    global: &GlobalArgs,
    client: &TosClient,
    source: &str,
    destination: &str,
    options: CopyOptions<'_>,
) -> Result<CopyTransferResult, CliError> {
    if is_tos_directory_marker_source(source)? {
        return download_tos_directory_marker_to_local(global, source, destination, options);
    }
    let source_target = parse_tos_uri(source, false)?;
    let key = source_target.key.expect("validated object key");
    let head = core::execute_object_request(
        client,
        "ve-tos cp head-source",
        Method::HEAD,
        &source_target.bucket,
        &key,
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    )
    .await?;
    let destination_path = Path::new(destination);
    if should_skip_download_for_overwrite_strategy(destination_path, &head, &options)? {
        write_single_report(
            success_report_path(options.report_path, options.report_failures_only),
            "download",
            source,
            Some(destination),
            "skipped",
        )?;
        return Ok(CopyTransferResult::skipped());
    }
    let mut headers = BTreeMap::new();
    let expected_length = head
        .data
        .as_ref()
        .and_then(|data| data.headers.get("content-length"))
        .and_then(|value| value.parse::<u64>().ok());
    let expected_crc64 = head
        .data
        .as_ref()
        .and_then(|data| find_crc64_header(&data.headers));
    if let Some(parent) = destination_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temp_path = partial_path(destination_path);
    let use_range = options.checkpoint_enabled
        && expected_length
            .map(|length| length >= options.checkpoint_threshold)
            .unwrap_or(false);
    let mut resume_offset = 0_u64;
    if use_range {
        resume_offset = fs::metadata(&temp_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if expected_length
            .map(|length| resume_offset >= length)
            .unwrap_or(false)
        {
            let _ = fs::remove_file(&temp_path);
            resume_offset = 0;
        }
        headers.insert("range".to_string(), format!("bytes={}-", resume_offset));
    }
    let etag = head
        .data
        .as_ref()
        .and_then(|data| data.headers.get("etag"))
        .cloned()
        .ok_or_else(|| {
            CliError::ValidationError(format!(
            "source '{}' HEAD response did not include ETag; cannot download with if-match guard",
            source
        ))
        })?;
    if use_range {
        return download_tos_to_local_ranges(
            global,
            client,
            source,
            &source_target.bucket,
            &key,
            destination,
            destination_path,
            expected_length.expect("use_range requires content-length"),
            expected_crc64,
            &etag,
            options,
        )
        .await;
    }
    headers.insert("if-match".to_string(), etag);
    let response = core::send_object_request(
        client,
        Method::GET,
        &source_target.bucket,
        &key,
        BTreeMap::new(),
        headers,
        None,
    )
    .await?;
    let status_code = response.status().as_u16();
    let response_headers = core::extract_headers(&response);
    let mut response = client.check_response(response).await?;
    // [Review Fix #11] Downloads write response chunks to a temp file before atomic persist.
    let bytes_written =
        write_response_stream_with_mode(&mut response, &temp_path, resume_offset > 0).await?;
    if let Some(expected_length) = expected_length {
        let total_written = resume_offset.saturating_add(bytes_written);
        if total_written != expected_length {
            let _ = fs::remove_file(&temp_path);
            return Err(CliError::TransferFailed(format!(
                "download length mismatch for '{}': expected={}, actual={}",
                source, expected_length, total_written
            )));
        }
    }
    if let Some(expected_crc64) = expected_crc64 {
        let local_crc64 = file_crc64(temp_path.to_string_lossy().as_ref())?;
        if local_crc64 != expected_crc64 {
            let _ = fs::remove_file(&temp_path);
            return Err(CliError::TransferFailed(format!(
                "download CRC64 mismatch for '{}': local={}, remote={}",
                source, local_crc64, expected_crc64
            )));
        }
    }
    persist_downloaded_file(&temp_path, destination_path, true)?;
    // [Review Fix #Progress-Overall] 下载成功后一次性把 bytes_written 推进 overall。
    // 注意：write_response_stream 已完成所有数据写入，这里仅做整体进度通知。
    if options.progress_granularity == EffectiveProgressGranularity::Byte {
        if let Some(bar) = options.overall_bar {
            bar.inc(bytes_written);
        }
    } else if let Some(bar) = options.overall_bar {
        bar.inc(1);
    }
    let result = Envelope::success(
        "ve-tos cp download",
        json!({
            "source": source,
            "destination": destination,
            "status_code": status_code,
            "bytes_written": bytes_written,
            "headers": response_headers,
        }),
    );
    if !options.silent_per_file || global.verbose {
        output_result(global, &result)?;
    }
    write_single_report(
        success_report_path(options.report_path, options.report_failures_only),
        "download",
        source,
        Some(destination),
        "succeeded",
    )?;
    CopyTransferResult::from_envelope(&result)
}

fn download_tos_directory_marker_to_local(
    global: &GlobalArgs,
    source: &str,
    destination: &str,
    options: CopyOptions<'_>,
) -> Result<CopyTransferResult, CliError> {
    let destination_path = Path::new(destination);
    create_local_directory_marker_destination(destination_path)?;
    if options.progress_granularity == EffectiveProgressGranularity::Part {
        if let Some(bar) = options.overall_bar {
            bar.inc(1);
        }
    }
    let result = Envelope::success(
        "ve-tos cp mkdir",
        json!({
            "source": source,
            "destination": destination,
            "local_action": "mkdir",
            "bytes_written": 0,
        }),
    );
    if !options.silent_per_file || global.verbose {
        output_result(global, &result)?;
    }
    write_single_report(
        success_report_path(options.report_path, options.report_failures_only),
        "download",
        source,
        Some(destination),
        "succeeded",
    )?;
    CopyTransferResult::from_envelope(&result)
}

async fn download_tos_to_local_ranges(
    global: &GlobalArgs,
    client: &TosClient,
    source: &str,
    bucket: &str,
    key: &str,
    destination: &str,
    destination_path: &Path,
    expected_length: u64,
    expected_crc64: Option<u64>,
    source_etag: &str,
    options: CopyOptions<'_>,
) -> Result<CopyTransferResult, CliError> {
    let temp_path = range_download_base_path(destination_path, source_etag, expected_length, "tos");
    let part_size = multipart_part_size(expected_length);
    let mut ranges = Vec::new();
    let mut part_number = 1_u32;
    let mut offset = 0_u64;
    while offset < expected_length {
        let current_size = (expected_length - offset).min(part_size);
        ranges.push((part_number, offset, current_size));
        part_number += 1;
        offset += current_size;
    }

    let mut pending_ranges = ranges.clone().into_iter().filter(|(part_number, _, size)| {
        let part_path = range_download_part_path(&temp_path, *part_number);
        fs::metadata(part_path)
            .map(|metadata| metadata.len() != *size)
            .unwrap_or(true)
    });
    let progress = if options.overall_bar.is_none() {
        FileProgress::new(
            options.progress_enabled,
            &short_label_from(source),
            expected_length,
        )
    } else {
        FileProgress { bar: None }
    };
    let initial_completed_bytes: u64 = ranges
        .iter()
        .filter_map(|(part_number, _, size)| {
            let part_path = range_download_part_path(&temp_path, *part_number);
            fs::metadata(part_path)
                .ok()
                .filter(|metadata| metadata.len() == *size)
                .map(|_| *size)
        })
        .sum();
    progress.set_position(initial_completed_bytes);
    if options.progress_granularity == EffectiveProgressGranularity::Byte {
        if let Some(bar) = options.overall_bar {
            bar.inc(initial_completed_bytes);
        }
    } else if let Some(bar) = options.overall_bar {
        let completed_parts = ranges
            .iter()
            .filter(|(part_number, _, size)| {
                let part_path = range_download_part_path(&temp_path, *part_number);
                fs::metadata(part_path)
                    .map(|metadata| metadata.len() == *size)
                    .unwrap_or(false)
            })
            .count() as u64;
        bar.inc(completed_parts);
    }

    let mut in_flight = FuturesUnordered::new();
    loop {
        while in_flight.len() < options.multipart_concurrency {
            let Some((part_number, part_offset, current_size)) = pending_ranges.next() else {
                break;
            };
            let part_path = range_download_part_path(&temp_path, part_number);
            in_flight.push(download_tos_range_part(
                client,
                bucket,
                key,
                source_etag,
                part_offset,
                current_size,
                part_path,
            ));
        }
        let Some(result) = in_flight.next().await else {
            break;
        };
        let current_size = result?;
        progress.inc(current_size);
        if options.progress_granularity == EffectiveProgressGranularity::Byte {
            if let Some(bar) = options.overall_bar {
                bar.inc(current_size);
            }
        } else if let Some(bar) = options.overall_bar {
            bar.inc(1);
        }
    }

    assemble_range_download_parts(&temp_path, &ranges)?;
    if let Some(expected_crc64) = expected_crc64 {
        let local_crc64 = file_crc64(temp_path.to_string_lossy().as_ref())?;
        if local_crc64 != expected_crc64 {
            cleanup_range_download_parts(&temp_path, &ranges)?;
            let _ = fs::remove_file(&temp_path);
            return Err(CliError::TransferFailed(format!(
                "download CRC64 mismatch for '{}': local={}, remote={}",
                source, local_crc64, expected_crc64
            )));
        }
    }
    persist_downloaded_file(&temp_path, destination_path, true)?;
    cleanup_range_download_parts(&temp_path, &ranges)?;
    progress.finish(true);
    let result = Envelope::success(
        "ve-tos cp download",
        json!({
            "source": source,
            "destination": destination,
            "bytes_written": expected_length,
            "range_parts": ranges.len(),
        }),
    );
    if !options.silent_per_file || global.verbose {
        output_result(global, &result)?;
    }
    write_single_report(
        success_report_path(options.report_path, options.report_failures_only),
        "range-download",
        source,
        Some(destination),
        "succeeded",
    )?;
    CopyTransferResult::from_envelope(&result)
}

async fn download_tos_range_part(
    client: &TosClient,
    bucket: &str,
    key: &str,
    source_etag: &str,
    offset: u64,
    size: u64,
    part_path: PathBuf,
) -> Result<u64, CliError> {
    let end = offset + size - 1;
    let response = core::send_object_request(
        client,
        Method::GET,
        bucket,
        key,
        BTreeMap::new(),
        BTreeMap::from([
            ("if-match".to_string(), source_etag.to_string()),
            ("range".to_string(), format!("bytes={offset}-{end}")),
        ]),
        None,
    )
    .await?;
    let mut response = client.check_response(response).await?;
    let written = write_response_stream_with_mode(&mut response, &part_path, false).await?;
    if written != size {
        let _ = fs::remove_file(&part_path);
        return Err(CliError::TransferFailed(format!(
            "range download length mismatch for '{}': expected={}, actual={}",
            part_path.display(),
            size,
            written
        )));
    }
    Ok(written)
}

fn range_download_part_path(temp_path: &Path, part_number: u32) -> PathBuf {
    let file_name = temp_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    temp_path.with_file_name(format!("{file_name}.range-{part_number}"))
}

fn range_download_base_path(
    destination_path: &Path,
    source_etag: &str,
    file_size: u64,
    namespace: &str,
) -> PathBuf {
    let file_name = destination_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let mut hasher = DefaultHasher::new();
    source_etag.hash(&mut hasher);
    file_size.hash(&mut hasher);
    destination_path.display().to_string().hash(&mut hasher);
    destination_path.with_file_name(format!(
        "{file_name}.{namespace}-range-{:016x}",
        hasher.finish()
    ))
}

fn assemble_range_download_parts(
    temp_path: &Path,
    ranges: &[(u32, u64, u64)],
) -> Result<(), CliError> {
    let mut output = File::create(temp_path)?;
    for (part_number, _, _) in ranges {
        let part_path = range_download_part_path(temp_path, *part_number);
        let mut input = File::open(&part_path)?;
        std::io::copy(&mut input, &mut output)?;
    }
    Ok(())
}

fn cleanup_range_download_parts(
    temp_path: &Path,
    ranges: &[(u32, u64, u64)],
) -> Result<(), CliError> {
    for (part_number, _, _) in ranges {
        let part_path = range_download_part_path(temp_path, *part_number);
        if let Err(err) = fs::remove_file(&part_path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                return Err(err.into());
            }
        }
    }
    Ok(())
}

async fn copy_tos_to_tos(
    global: &GlobalArgs,
    client: &TosClient,
    source: &str,
    destination: &str,
    options: CopyOptions<'_>,
) -> Result<CopyTransferResult, CliError> {
    let source_target = parse_tos_uri(source, false)?;
    let destination_target = parse_tos_uri(destination, false)?;
    let source_key = source_target.key.expect("validated object key");
    let destination_key = destination_target.key.expect("validated object key");
    ensure_same_region_remote_copy(client, &source_target.bucket, &destination_target.bucket)
        .await?;
    let head = core::execute_object_request(
        client,
        "ve-tos cp head-source",
        Method::HEAD,
        &source_target.bucket,
        &source_key,
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    )
    .await?;
    if should_skip_copy_for_overwrite_strategy(client, destination, &head, &options).await? {
        write_single_report(
            success_report_path(options.report_path, options.report_failures_only),
            "copy",
            source,
            Some(destination),
            "skipped",
        )?;
        return Ok(CopyTransferResult::skipped());
    }
    let mut headers = options.write_options.headers(true);
    headers.insert(
        // [Review Fix #M3] Use TOS-native copy headers per
        // docs/high_level_commands_plan.md §一致性规则.
        "x-tos-copy-source".to_string(),
        copy_source_header_value(&source_target.bucket, &source_key),
    );
    if options.overwrite_strategy == EffectiveOverwriteStrategy::NoClobber {
        headers.insert("If-None-Match".to_string(), "*".to_string());
    }
    let (copy_method, copy_query) = copy_object_method_query();
    let source_etag = head
        .data
        .as_ref()
        .and_then(|data| data.headers.get("etag"))
        .cloned()
        .ok_or_else(|| {
            CliError::ValidationError(format!(
            "source '{}' HEAD response did not include ETag; cannot copy with copy-source-if-match guard",
            source
        ))
        })?;
    let source_size = head
        .data
        .as_ref()
        .and_then(|data| data.headers.get("content-length"))
        .and_then(|value| value.parse::<u64>().ok());
    if source_size
        .map(|size| {
            should_use_multipart(
                size,
                options.checkpoint_enabled,
                options.checkpoint_threshold,
            )
        })
        .unwrap_or(false)
    {
        return copy_tos_to_tos_multipart(
            global,
            client,
            source,
            &source_target.bucket,
            &source_key,
            &destination_target.bucket,
            &destination_key,
            source_size.expect("checked Some"),
            &source_etag,
            &head,
            options,
        )
        .await;
    }
    headers.insert("x-tos-copy-source-if-match".to_string(), source_etag);
    let copy_response = core::execute_object_request(
        client,
        "ve-tos cp copy",
        copy_method,
        &destination_target.bucket,
        &destination_key,
        copy_query,
        headers,
        None,
    )
    .await?;
    if !options.silent_per_file || global.verbose {
        output_result(global, &copy_response)?;
    }
    // [Review Fix #Progress-Overall] tos→tos 复制是服务端拷贝，无客户端字节流。
    // 用 head 拿到的 content-length（若可得）一次性推进 overall；否则不推进。
    if options.progress_granularity == EffectiveProgressGranularity::Byte {
        if let Some(bar) = options.overall_bar {
            let bytes = head
                .data
                .as_ref()
                .and_then(|d| d.headers.get("content-length"))
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            bar.inc(bytes);
        }
    } else if let Some(bar) = options.overall_bar {
        bar.inc(1);
    }
    write_single_report(
        success_report_path(options.report_path, options.report_failures_only),
        "copy",
        source,
        Some(destination),
        "succeeded",
    )?;
    CopyTransferResult::from_raw_response(&copy_response)
}

async fn ensure_same_region_remote_copy(
    client: &TosClient,
    source_bucket: &str,
    destination_bucket: &str,
) -> Result<(), CliError> {
    if source_bucket == destination_bucket {
        return Ok(());
    }
    let source_location = bucket::get_bucket_location(client, source_bucket).await?;
    let destination_location = bucket::get_bucket_location(client, destination_bucket).await?;
    let source_region = source_location
        .data
        .as_ref()
        .map(|data| data.region.as_str())
        .unwrap_or_default();
    let destination_region = destination_location
        .data
        .as_ref()
        .map(|data| data.region.as_str())
        .unwrap_or_default();
    if source_region != destination_region {
        return Err(CliError::ValidationError(format!(
            "ve-tos cp remote copy only supports buckets in the same region: source bucket '{}' is '{}', destination bucket '{}' is '{}'; automatic cross-region dual-client streaming is not supported, use explicit download+upload or cat|put instead",
            source_bucket, source_region, destination_bucket, destination_region
        )));
    }
    Ok(())
}

async fn ensure_same_region_for_tos_uris(
    client: &TosClient,
    source: &str,
    destination: &str,
) -> Result<(), CliError> {
    if !(source.starts_with("tos://") && destination.starts_with("tos://")) {
        return Ok(());
    }
    let source = parse_tos_uri(source, true)?;
    let destination = parse_tos_uri(destination, true)?;
    // [Review Fix #7] Cloud-to-cloud cp/mv/sync must fail before planning when
    // buckets are in different regions, rather than producing partial reports.
    ensure_same_region_remote_copy(client, &source.bucket, &destination.bucket).await
}

async fn copy_tos_to_tos_multipart(
    global: &GlobalArgs,
    client: &TosClient,
    source: &str,
    source_bucket: &str,
    source_key: &str,
    destination_bucket: &str,
    destination_key: &str,
    file_size: u64,
    source_etag: &str,
    source_head: &Envelope<core::RawResponseData>,
    options: CopyOptions<'_>,
) -> Result<CopyTransferResult, CliError> {
    let part_size = multipart_part_size(file_size);
    let checkpoint_identity = remote_copy_checkpoint_identity(source, source_etag, source_head);
    let checkpoint_path = checkpoint_path(
        options.checkpoint_dir,
        &format!("copy:{source}:{source_etag}"),
        destination_bucket,
        destination_key,
        file_size,
        checkpoint_identity,
        part_size,
        &options.write_options.checkpoint_fingerprint(),
        &global.profile,
        &client.service_endpoint(),
    )?;
    let _lock = CheckpointLock::acquire(&checkpoint_path)?;

    let mut checkpoint = load_checkpoint(&checkpoint_path)?.unwrap_or_else(|| Checkpoint {
        bucket: destination_bucket.to_string(),
        key: destination_key.to_string(),
        source_path: Some(source.to_string()),
        file_size,
        part_size,
        upload_id: None,
        completed_parts: Vec::new(),
    });
    if checkpoint.bucket != destination_bucket
        || checkpoint.key != destination_key
        || checkpoint.file_size != file_size
        || checkpoint.part_size != part_size
    {
        return Err(CliError::ValidationError(
            "checkpoint metadata does not match this remote copy; remove the checkpoint to restart"
                .to_string(),
        ));
    }

    if checkpoint.upload_id.is_none() {
        let mut create_headers = options.write_options.headers(true);
        if options.overwrite_strategy == EffectiveOverwriteStrategy::NoClobber {
            create_headers.insert("if-none-match".to_string(), "*".to_string());
        }
        let created = core::execute_object_request(
            client,
            "ve-tos cp multipart-copy create",
            Method::POST,
            destination_bucket,
            destination_key,
            BTreeMap::from([("uploads".to_string(), String::new())]),
            create_headers,
            None,
        )
        .await?;
        checkpoint.upload_id = extract_upload_id(&created);
        if checkpoint.upload_id.is_none() {
            return Err(CliError::ValidationError(
                "CreateMultipartUpload response did not include UploadId".to_string(),
            ));
        }
        save_checkpoint(&checkpoint_path, &checkpoint)?;
    }

    let upload_id = checkpoint.upload_id.clone().expect("validated upload id");
    let remote_parts =
        list_uploaded_part_numbers(client, destination_bucket, destination_key, &upload_id).await?;
    checkpoint
        .completed_parts
        .retain(|part| remote_parts.contains(&part.part_number));
    save_checkpoint(&checkpoint_path, &checkpoint)?;

    let progress = if options.overall_bar.is_none() {
        FileProgress::new(
            options.progress_enabled,
            &short_label_from(source),
            file_size,
        )
    } else {
        FileProgress { bar: None }
    };
    let initial_completed_bytes: u64 = checkpoint
        .completed_parts
        .iter()
        .map(|part| {
            let part_offset = (part.part_number as u64 - 1) * part_size;
            (file_size - part_offset).min(part_size)
        })
        .sum();
    progress.set_position(initial_completed_bytes);
    if options.progress_granularity == EffectiveProgressGranularity::Byte {
        if let Some(bar) = options.overall_bar {
            bar.inc(initial_completed_bytes);
        }
    } else if let Some(bar) = options.overall_bar {
        bar.inc(checkpoint.completed_parts.len() as u64);
    }

    let mut pending_parts = Vec::new();
    let mut part_number = 1_u32;
    let mut offset = 0_u64;
    while offset < file_size {
        let current_size = (file_size - offset).min(part_size);
        if checkpoint
            .completed_parts
            .iter()
            .any(|part| part.part_number == part_number)
        {
            part_number += 1;
            offset += current_size;
            continue;
        }
        pending_parts.push((part_number, offset, current_size));
        part_number += 1;
        offset += current_size;
    }

    let mut pending_parts = pending_parts.into_iter();
    let mut in_flight = FuturesUnordered::new();
    loop {
        while in_flight.len() < options.multipart_concurrency {
            let Some((part_number, part_offset, current_size)) = pending_parts.next() else {
                break;
            };
            in_flight.push(copy_tos_multipart_part(
                client,
                source_bucket,
                source_key,
                destination_bucket,
                destination_key,
                &upload_id,
                part_number,
                part_offset,
                current_size,
                source_etag,
            ));
        }

        let Some(result) = in_flight.next().await else {
            break;
        };
        let (completed_part, current_size) = result?;
        let completed_part_number = completed_part.part_number;
        checkpoint.completed_parts.push(completed_part);
        checkpoint
            .completed_parts
            .sort_by_key(|part| part.part_number);
        save_checkpoint(&checkpoint_path, &checkpoint)?;
        progress.inc(current_size);
        if options.progress_granularity == EffectiveProgressGranularity::Byte {
            if let Some(bar) = options.overall_bar {
                bar.inc(current_size);
            }
        } else if let Some(bar) = options.overall_bar {
            bar.inc(1);
        }
        emit_progress(
            options.progress_enabled,
            "copy-part",
            &format!(
                "tos://{}/{}#{}",
                destination_bucket, destination_key, completed_part_number
            ),
        );
    }

    let (complete_query, complete_headers, complete_body) =
        complete_multipart_request(&upload_id, &checkpoint.completed_parts)?;
    let completed = core::execute_object_request(
        client,
        "ve-tos cp multipart-copy complete",
        Method::POST,
        destination_bucket,
        destination_key,
        complete_query,
        complete_headers,
        Some(complete_body),
    )
    .await?;
    if !options.silent_per_file || global.verbose {
        output_result(global, &completed)?;
    }
    progress.finish(true);
    remove_checkpoint(&checkpoint_path)?;
    write_single_report(
        success_report_path(options.report_path, options.report_failures_only),
        "multipart-copy",
        source,
        Some(&format!("tos://{}/{}", destination_bucket, destination_key)),
        "succeeded",
    )?;
    CopyTransferResult::from_raw_response(&completed)
}

async fn copy_tos_multipart_part(
    client: &TosClient,
    source_bucket: &str,
    source_key: &str,
    destination_bucket: &str,
    destination_key: &str,
    upload_id: &str,
    part_number: u32,
    offset: u64,
    current_size: u64,
    source_etag: &str,
) -> Result<(CompletedPart, u64), CliError> {
    let end = offset + current_size - 1;
    let mut headers = BTreeMap::from([
        (
            "x-tos-copy-source".to_string(),
            copy_source_header_value(source_bucket, source_key),
        ),
        (
            "x-tos-copy-source-if-match".to_string(),
            source_etag.to_string(),
        ),
    ]);
    if active_tos_config_binary() != Binary::Tos {
        headers.insert(
            "x-tos-copy-source-range".to_string(),
            format!("bytes={offset}-{end}"),
        );
    }
    let copied = core::execute_object_request(
        client,
        "ve-tos cp multipart upload-part-copy",
        Method::PUT,
        destination_bucket,
        destination_key,
        multipart_copy_part_query(upload_id, part_number, offset, current_size),
        headers,
        None,
    )
    .await?;
    let etag = extract_part_copy_etag(&copied).ok_or_else(|| {
        CliError::ValidationError("UploadPartCopy response missing ETag".to_string())
    })?;
    Ok((
        CompletedPart {
            part_number,
            etag,
            crc64: None,
        },
        current_size,
    ))
}

fn copy_local_to_local(
    global: &GlobalArgs,
    source: &str,
    destination: &str,
    options: CopyOptions<'_>,
) -> Result<CopyTransferResult, CliError> {
    if should_skip_local_copy_for_overwrite_strategy(source, destination, &options)? {
        write_single_report(
            success_report_path(options.report_path, options.report_failures_only),
            "local-copy",
            source,
            Some(destination),
            "skipped",
        )?;
        return Ok(CopyTransferResult::skipped());
    }
    if let Some(parent) = Path::new(destination)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        // [Review Fix #5] Recursive local copies create nested destination parents before fs::copy.
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)?;
    let result = Envelope::success(
        "ve-tos cp local",
        json!({
            "source": source,
            "destination": destination,
        }),
    );
    if !options.silent_per_file {
        output_result(global, &result)?;
    }
    write_single_report(
        success_report_path(options.report_path, options.report_failures_only),
        "local-copy",
        source,
        Some(destination),
        "succeeded",
    )?;
    CopyTransferResult::from_envelope(&result)
}

async fn should_skip_upload_for_overwrite_strategy(
    client: &TosClient,
    source: &str,
    bucket: &str,
    key: &str,
    source_metadata: &fs::Metadata,
    options: &CopyOptions<'_>,
) -> Result<bool, CliError> {
    match options.overwrite_strategy {
        EffectiveOverwriteStrategy::Force => Ok(false),
        EffectiveOverwriteStrategy::NoClobber => {
            Ok(object_entry_for_bucket_key(client, bucket, key)
                .await?
                .is_some())
        }
        EffectiveOverwriteStrategy::Newer => {
            let Some(destination) = object_entry_for_bucket_key(client, bucket, key).await? else {
                return Ok(false);
            };
            let Some(destination_mtime) =
                parse_remote_last_modified(destination.last_modified.as_deref())
            else {
                return Ok(false);
            };
            let source_mtime = system_time_to_utc(source_metadata.modified().map_err(|err| {
                CliError::Io(std::io::Error::new(
                    err.kind(),
                    format!("failed to read mtime for '{}': {}", source, err),
                ))
            })?);
            Ok(source_mtime <= destination_mtime)
        }
    }
}

fn should_skip_download_for_overwrite_strategy(
    destination_path: &Path,
    source_head: &Envelope<core::RawResponseData>,
    options: &CopyOptions<'_>,
) -> Result<bool, CliError> {
    if !destination_path.exists() {
        return Ok(false);
    }
    match options.overwrite_strategy {
        EffectiveOverwriteStrategy::Force => Ok(false),
        EffectiveOverwriteStrategy::NoClobber => Ok(true),
        EffectiveOverwriteStrategy::Newer => {
            let Some(source_mtime) = head_last_modified(source_head) else {
                return Ok(false);
            };
            let destination_mtime = system_time_to_utc(fs::metadata(destination_path)?.modified()?);
            Ok(source_mtime <= destination_mtime)
        }
    }
}

async fn should_skip_copy_for_overwrite_strategy(
    client: &TosClient,
    destination: &str,
    source_head: &Envelope<core::RawResponseData>,
    options: &CopyOptions<'_>,
) -> Result<bool, CliError> {
    match options.overwrite_strategy {
        EffectiveOverwriteStrategy::Force => Ok(false),
        EffectiveOverwriteStrategy::NoClobber => {
            Ok(object_entry_from_head(client, destination).await?.is_some())
        }
        EffectiveOverwriteStrategy::Newer => {
            let Some(destination_entry) = object_entry_from_head(client, destination).await? else {
                return Ok(false);
            };
            let Some(source_mtime) = head_last_modified(source_head) else {
                return Ok(false);
            };
            let Some(destination_mtime) =
                parse_remote_last_modified(destination_entry.last_modified.as_deref())
            else {
                return Ok(false);
            };
            Ok(source_mtime <= destination_mtime)
        }
    }
}

fn should_skip_local_copy_for_overwrite_strategy(
    source: &str,
    destination: &str,
    options: &CopyOptions<'_>,
) -> Result<bool, CliError> {
    let destination_path = Path::new(destination);
    if !destination_path.exists() {
        return Ok(false);
    }
    match options.overwrite_strategy {
        EffectiveOverwriteStrategy::Force => Ok(false),
        EffectiveOverwriteStrategy::NoClobber => Ok(true),
        EffectiveOverwriteStrategy::Newer => {
            let source_mtime = system_time_to_utc(fs::metadata(source)?.modified()?);
            let destination_mtime = system_time_to_utc(fs::metadata(destination_path)?.modified()?);
            Ok(source_mtime <= destination_mtime)
        }
    }
}

async fn object_entry_for_bucket_key(
    client: &TosClient,
    bucket: &str,
    key: &str,
) -> Result<Option<ObjectEntry>, CliError> {
    match core::execute_object_request(
        client,
        "ve-tos cp head-destination",
        Method::HEAD,
        bucket,
        key,
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    )
    .await
    {
        Ok(response) => {
            let Some(data) = response.data else {
                return Ok(None);
            };
            Ok(Some(ObjectEntry {
                key: key.to_string(),
                size: data
                    .headers
                    .get("content-length")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0),
                last_modified: data.headers.get("last-modified").cloned(),
                etag: data.headers.get("etag").cloned(),
                storage_class: data.headers.get("x-tos-storage-class").cloned(),
            }))
        }
        Err(CliError::ResourceNotFound(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

fn head_last_modified(head: &Envelope<core::RawResponseData>) -> Option<DateTime<Utc>> {
    let value = head
        .data
        .as_ref()
        .and_then(|data| data.headers.get("last-modified"))?;
    parse_remote_last_modified(Some(value))
}

fn parse_remote_last_modified(value: Option<&str>) -> Option<DateTime<Utc>> {
    let value = value?.trim();
    DateTime::parse_from_rfc2822(value)
        .or_else(|_| DateTime::parse_from_rfc3339(value))
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
}

fn system_time_to_utc(value: SystemTime) -> DateTime<Utc> {
    DateTime::<Utc>::from(value)
}

async fn source_object_etag(
    client: &TosClient,
    bucket: &str,
    key: &str,
) -> Result<Option<String>, CliError> {
    let head = core::execute_object_request(
        client,
        "ve-tos mv head-source-before-delete",
        Method::HEAD,
        bucket,
        key,
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    )
    .await?;
    Ok(head
        .data
        .as_ref()
        .and_then(|data| data.headers.get("etag").cloned()))
}

async fn object_entry_from_head(
    client: &TosClient,
    uri: &str,
) -> Result<Option<ObjectEntry>, CliError> {
    let target = parse_tos_uri(uri, false)?;
    let key = target.key.expect("validated object key");
    match core::execute_object_request(
        client,
        "ve-tos sync head-manifest-entry",
        Method::HEAD,
        &target.bucket,
        &key,
        BTreeMap::new(),
        BTreeMap::new(),
        None,
    )
    .await
    {
        Ok(response) => {
            let Some(data) = response.data else {
                return Ok(None);
            };
            Ok(Some(ObjectEntry {
                key,
                size: data
                    .headers
                    .get("content-length")
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0),
                last_modified: data.headers.get("last-modified").cloned(),
                etag: data.headers.get("etag").cloned(),
                storage_class: data.headers.get("x-tos-storage-class").cloned(),
            }))
        }
        Err(CliError::ResourceNotFound(_)) => Ok(None),
        Err(err) => Err(err),
    }
}

fn local_destination_matches_remote(
    destination: &str,
    source: &ObjectEntry,
    args: &SyncArgs,
) -> Result<bool, CliError> {
    let destination_path = Path::new(destination);
    if !destination_path.exists() {
        return Ok(false);
    }
    let metadata = fs::metadata(destination_path)?;
    if metadata.len() != source.size {
        return Ok(false);
    }
    if args.exact_timestamps {
        return Ok(false);
    }
    Ok(args.size_only || metadata.len() == source.size)
}

fn tos_entries_match_for_sync(
    source: &ObjectEntry,
    destination: &ObjectEntry,
    args: &SyncArgs,
) -> bool {
    // [Review Fix #Sync-LogicBug] 修复原 `A && (B || A)` 化简后退化为 `A` 的逻辑 bug：
    // 原写法 `source.size == destination.size && (args.size_only || source.size == destination.size)`
    // 第二个等式表达式完全冗余，clippy 报 logic-bug。重写为与
    // local_sync_should_skip/remote_sync_should_skip 家族一致的早返回结构：
    //   1) etag 匹配 → match；
    //   2) size 不等 → mismatch；
    //   3) exact_timestamps=true 且 etag 不可信 → 用 LastModified 字符串保守比对，
    //      任一端缺失 last_modified 视为 mismatch（tos→tos 没有本地 mtime 可校验）；
    //   4) 默认/size_only → size 已匹配，视为 match。
    if source.etag.is_some() && source.etag == destination.etag {
        return true;
    }
    if source.size != destination.size {
        return false;
    }
    if args.exact_timestamps {
        return source.last_modified.is_some() && source.last_modified == destination.last_modified;
    }
    true
}

async fn delete_tos_object(
    client: &TosClient,
    command: &str,
    bucket: &str,
    key: &str,
    source_etag: Option<&str>,
) -> Result<Envelope<core::RawResponseData>, CliError> {
    let mut headers = BTreeMap::new();
    if let Some(etag) = source_etag {
        // [Review Fix #10] Move deletes pin the copied source version to avoid deleting a new object.
        headers.insert("if-match".to_string(), etag.to_string());
    }
    core::execute_object_request(
        client,
        command,
        Method::DELETE,
        bucket,
        key,
        BTreeMap::new(),
        headers,
        None,
    )
    .await
}

fn execute_cp_recursive_local(
    global: &GlobalArgs,
    args: &CpArgs,
    report_path: Option<&str>,
    manifest_path: Option<&str>,
) -> Result<i32, CliError> {
    let mappings =
        build_local_source_mappings(&args.source, &args.destination, args.include_parent)?;
    let runtime = effective_cp_runtime_config(global, args)?;
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let planned = mappings
        .into_iter()
        .filter(|item| {
            pattern_allows(
                &item.relative_key,
                args.include.as_deref(),
                args.exclude.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    enforce_transfer_plan_path_traversal(
        global,
        "ve-tos cp",
        args.force,
        false,
        &args.destination,
        &planned,
    )?;
    write_manifest_file(
        manifest_path,
        "ve-tos cp",
        &build_transfer_manifest(&planned),
    )?;
    let mut report = BatchReport::new(planned.len() as u64);
    for item in planned {
        match copy_local_to_local(
            global,
            &item.source,
            &item.destination,
            runtime.copy_options(
                None,
                false,
                args.checkpoint,
                args.checkpoint_dir.as_deref(),
                ObjectWriteOptions::default(),
                progress_enabled,
                true,
                None,
            ),
        ) {
            Ok(result) => match result.outcome {
                CopyOutcome::Transferred => {
                    report.record_success("copy", &item.source, Some(&item.destination));
                }
                CopyOutcome::Skipped => {
                    report.record_skipped("copy", &item.source, Some(&item.destination));
                }
            },
            Err(err) => {
                report.record_failure("copy", &item.source, Some(&item.destination), &err);
            }
        }
    }
    write_tos_batch_report(report_path, "ve-tos cp", &report, args.report_failures_only)?;
    output_tos_batch_envelope(
        global,
        "ve-tos cp",
        &args.source,
        &args.destination,
        report_path,
        manifest_path,
        &report,
    )?;
    // [Review Fix #BatchExitCode] 本地递归 cp 也是批量操作，summary
    // 已经输出后用 exit code=1 表达部分失败，避免顶层再打印错误 JSON。
    if report.summary.failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn execute_sync_local_to_local(global: &GlobalArgs, args: &SyncArgs) -> Result<i32, CliError> {
    let report_path = effective_report_path(global, args.report_path.as_deref(), "ve-tos sync")?;
    let manifest_path = effective_optional_manifest_path(
        global,
        args.manifest_path.as_deref(),
        args.no_manifest,
        "ve-tos sync",
    )?;
    let mappings =
        build_local_source_mappings(&args.source, &args.destination, args.include_parent)?;
    let planned = mappings
        .into_iter()
        .filter(|item| {
            pattern_allows(
                &item.relative_key,
                args.include.as_deref(),
                args.exclude.as_deref(),
            )
        })
        .collect::<Vec<_>>();
    enforce_transfer_plan_path_traversal(
        global,
        "ve-tos sync",
        args.force,
        args.delete,
        &args.destination,
        &planned,
    )?;
    let delete_plan = if args.delete {
        build_local_extras_plan(
            Path::new(&args.source),
            Path::new(&args.destination),
            &args.source,
            args.include_parent,
            args.include.as_deref(),
            args.exclude.as_deref(),
        )?
    } else {
        Vec::new()
    };
    let mut manifest_items = build_transfer_manifest(&planned).items;
    manifest_items.extend(delete_plan.clone());
    let manifest = TransferManifest::from_items(manifest_items);
    write_manifest_file(manifest_path.as_deref(), "ve-tos sync", &manifest)?;
    let mut report = BatchReport::new(manifest.object_count);
    let mut copied = 0_u64;
    let mut skipped = 0_u64;
    for item in planned {
        if local_sync_should_skip(&item.source, &item.destination, args)? {
            skipped += 1;
            report.record_skipped("sync-copy", &item.source, Some(&item.destination));
            continue;
        }
        match copy_local_file_without_output(&item.source, &item.destination) {
            Ok(()) => {
                copied += 1;
                report.record_success("sync-copy", &item.source, Some(&item.destination));
            }
            Err(err) => {
                report.record_failure("sync-copy", &item.source, Some(&item.destination), &err);
            }
        }
    }
    let mut deleted = 0_u64;
    if args.delete && report.summary.failed > 0 {
        // [Review Fix #4] Local sync also treats copy failure as a hard gate for
        // `--delete`, instead of removing destination-only files after a partial copy.
        for item in delete_plan {
            report.record_skipped("delete-extra", &item.source, None);
        }
    } else if args.delete {
        for item in delete_plan {
            match fs::remove_file(&item.source) {
                Ok(()) => {
                    deleted += 1;
                    report.record_success("delete-extra", &item.source, None);
                }
                Err(err) => {
                    let err = CliError::Io(err);
                    report.record_failure("delete-extra", &item.source, None, &err);
                }
            }
        }
    }
    write_tos_batch_report(
        report_path.as_deref(),
        "ve-tos sync",
        &report,
        args.report_failures_only,
    )?;
    output_result(
        global,
        &Envelope::success(
            "ve-tos sync",
            json!({
                "source": args.source,
                "destination": args.destination,
                "copied": copied,
                "skipped": skipped,
                "deleted": deleted,
                "failed": report.summary.failed,
                "report_path": report_path,
                "manifest_path": manifest_path,
                "status": if report.summary.failed == 0 { "succeeded" } else { "partial_failure" },
            }),
        ),
    )?;
    // [Review Fix #BatchExitCode] 批量 summary 已经输出，失败项用
    // exit code=1 表达，避免顶层错误处理再打印一份 JSON。
    if report.summary.failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

fn local_sync_should_skip(
    source: &str,
    destination: &str,
    args: &SyncArgs,
) -> Result<bool, CliError> {
    let destination_path = Path::new(destination);
    if !destination_path.exists() {
        return Ok(false);
    }
    let source_metadata = fs::metadata(source)?;
    let destination_metadata = fs::metadata(destination_path)?;
    if source_metadata.len() != destination_metadata.len() {
        return Ok(false);
    }
    if args.exact_timestamps {
        return Ok(source_metadata.modified()? == destination_metadata.modified()?);
    }
    Ok(args.size_only || source_metadata.len() == destination_metadata.len())
}

fn remote_sync_should_skip(
    local_size: u64,
    destination: Option<&ObjectEntry>,
    args: &SyncArgs,
) -> bool {
    let Some(destination) = destination else {
        return false;
    };
    if destination.size != local_size {
        return false;
    }
    // [Review Fix #15] Exact timestamp mode cannot trust TOS LastModified as local mtime.
    if args.exact_timestamps {
        return false;
    }
    args.size_only || destination.size == local_size
}

fn copy_local_file_without_output(source: &str, destination: &str) -> Result<(), CliError> {
    if let Some(parent) = Path::new(destination)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, destination)?;
    Ok(())
}

async fn execute_cat(
    _global: &GlobalArgs,
    client: &TosClient,
    args: &CatArgs,
) -> Result<i32, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos cat",
    )?;
    let target = parse_tos_uri(&path, false)?;
    let key = target.key.expect("validated object key");
    let mut query = BTreeMap::new();
    if let Some(version_id) = &args.version_id {
        query.insert("versionId".to_string(), version_id.clone());
    }
    let mut headers = BTreeMap::new();
    if let Some(range) = &args.range {
        headers.insert("range".to_string(), normalize_http_range(range)?);
    }
    let response = core::send_object_request(
        client,
        Method::GET,
        &target.bucket,
        &key,
        query,
        headers,
        None,
    )
    .await?;
    let mut response = client.check_response(response).await?;
    stream_response_to_stdout(&mut response).await?;
    Ok(0)
}

async fn execute_put(
    global: &GlobalArgs,
    client: &TosClient,
    args: &PutArgs,
) -> Result<i32, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos put",
    )?;
    let target = parse_tos_uri(&path, false)?;
    let key = target.key.expect("validated object key");
    let part_size =
        effective_stdin_multipart_threshold(global, args.multipart_threshold.as_deref())?;
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let write_options = put_write_options(args)?;
    ensure_tos_upload_storage_class_supported(
        "ve-tos put",
        None,
        &path,
        write_options.storage_class.as_deref(),
    )?;
    let mut stdin = tokio::io::stdin();
    let first_part = read_stream_part(&mut stdin, part_size).await?;
    if first_part.len() < part_size {
        put_tos_single_stdin_object(
            global,
            client,
            args,
            &target.bucket,
            &key,
            first_part,
            &write_options,
        )
        .await?;
        return Ok(0);
    }
    put_tos_multipart_stdin_object(
        global,
        client,
        args,
        &target.bucket,
        &key,
        first_part,
        stdin,
        &write_options,
        progress_enabled,
    )
    .await?;
    Ok(0)
}

async fn put_tos_single_stdin_object(
    global: &GlobalArgs,
    client: &TosClient,
    args: &PutArgs,
    bucket: &str,
    key: &str,
    body: Vec<u8>,
    write_options: &ObjectWriteOptions,
) -> Result<(), CliError> {
    let body_len = body.len() as u64;
    let mut crc = !0_u64;
    crc64_update(&mut crc, &body);
    let local_crc64 = !crc;
    let mut headers = write_options.headers(false);
    headers.insert("content-length".to_string(), body.len().to_string());
    headers.insert("x-hash-crc64ecma".to_string(), local_crc64.to_string());
    if args.no_clobber {
        headers.insert("if-none-match".to_string(), "*".to_string());
    }
    let uploaded = core::execute_object_streaming_request(
        client,
        "ve-tos put",
        Method::PUT,
        bucket,
        key,
        BTreeMap::new(),
        headers,
        hash_payload(&body),
        Body::from(body),
    )
    .await?;
    verify_tos_crc64_response(&uploaded, local_crc64, "stdin upload")?;
    let response = uploaded.data.as_ref().ok_or_else(|| {
        CliError::ValidationError("PutObject response did not include response data".to_string())
    })?;
    let destination = format!("tos://{bucket}/{key}");
    let mut envelope = Envelope::success(
        public_high_level_command("ve-tos put"),
        tos_put_success_payload("stdin-upload", &destination, body_len, response),
    );
    if let Some(request_id) = uploaded.request_id {
        envelope = envelope.with_request_id(request_id);
    }
    output_result(global, &envelope)?;
    Ok(())
}

async fn put_tos_multipart_stdin_object(
    global: &GlobalArgs,
    client: &TosClient,
    args: &PutArgs,
    bucket: &str,
    key: &str,
    first_part: Vec<u8>,
    mut stdin: tokio::io::Stdin,
    write_options: &ObjectWriteOptions,
    progress_enabled: bool,
) -> Result<(), CliError> {
    let mut create_headers = write_options.headers(false);
    if args.no_clobber {
        create_headers.insert("if-none-match".to_string(), "*".to_string());
    }
    let created = core::execute_object_request(
        client,
        "ve-tos put multipart create",
        Method::POST,
        bucket,
        key,
        BTreeMap::from([("uploads".to_string(), String::new())]),
        create_headers,
        None,
    )
    .await?;
    let upload_id = extract_upload_id(&created).ok_or_else(|| {
        CliError::ValidationError("CreateMultipartUpload response did not include UploadId".into())
    })?;
    let result = put_tos_multipart_stdin_inner(
        client,
        bucket,
        key,
        &upload_id,
        first_part,
        &mut stdin,
        progress_enabled,
    )
    .await;
    match result {
        Ok((completed_parts, total_size, local_crc64)) => {
            // [Review Fix #1] Abort the upload when completion itself fails so
            // a retryable CompleteMultipartUpload error does not leave parts behind.
            let completed = match complete_tos_stdin_multipart(
                client,
                bucket,
                key,
                &upload_id,
                &completed_parts,
            )
            .await
            {
                Ok(completed) => completed,
                Err(err) => {
                    let _ = abort_tos_multipart_upload(client, bucket, key, &upload_id).await;
                    return Err(err);
                }
            };
            verify_tos_crc64_response(&completed, local_crc64, "stdin multipart upload")?;
            let response = completed.data.as_ref().ok_or_else(|| {
                CliError::ValidationError(
                    "CompleteMultipartUpload response did not include response data".to_string(),
                )
            })?;
            let destination = format!("tos://{bucket}/{key}");
            let mut payload = tos_put_success_payload(
                "stdin-multipart-upload",
                &destination,
                total_size,
                response,
            );
            if let Value::Object(ref mut object) = payload {
                object.insert("parts".to_string(), json!(completed_parts.len()));
                object.insert("upload_id".to_string(), json!(upload_id));
            }
            let mut envelope = Envelope::success(public_high_level_command("ve-tos put"), payload);
            if let Some(request_id) = completed.request_id {
                envelope = envelope.with_request_id(request_id);
            }
            output_result(global, &envelope)?;
            Ok(())
        }
        Err(err) => {
            let _ = abort_tos_multipart_upload(client, bucket, key, &upload_id).await;
            Err(err)
        }
    }
}

async fn put_tos_multipart_stdin_inner<R: AsyncRead + Unpin>(
    client: &TosClient,
    bucket: &str,
    key: &str,
    upload_id: &str,
    first_part: Vec<u8>,
    stdin: &mut R,
    progress_enabled: bool,
) -> Result<(Vec<CompletedPart>, u64, u64), CliError> {
    let mut completed_parts = Vec::new();
    let mut full_crc = !0_u64;
    let mut total_size = 0_u64;
    let mut part_number = 1_u32;
    let part_size = first_part.len();
    let progress = stdin_upload_progress("ve-tos put", progress_enabled);
    let mut next_part = Some(first_part);
    loop {
        let part = if let Some(part) = next_part.take() {
            part
        } else {
            read_stream_part(stdin, part_size).await?
        };
        if part.is_empty() {
            break;
        }
        let current_size = part.len() as u64;
        crc64_update(&mut full_crc, &part);
        total_size += current_size;
        let completed =
            upload_tos_stdin_part(client, bucket, key, upload_id, part_number, part).await?;
        completed_parts.push(completed);
        if let Some(progress) = &progress {
            progress.inc(current_size);
        }
        part_number += 1;
    }
    if let Some(progress) = progress {
        progress.finish_and_clear();
    }
    Ok((completed_parts, total_size, !full_crc))
}

async fn upload_tos_stdin_part(
    client: &TosClient,
    bucket: &str,
    key: &str,
    upload_id: &str,
    part_number: u32,
    body: Vec<u8>,
) -> Result<CompletedPart, CliError> {
    let mut part_crc = !0_u64;
    crc64_update(&mut part_crc, &body);
    let local_crc64 = !part_crc;
    let uploaded = core::execute_object_streaming_request(
        client,
        "ve-tos put multipart upload-part",
        Method::PUT,
        bucket,
        key,
        multipart_part_query(upload_id, part_number),
        BTreeMap::from([
            ("content-length".to_string(), body.len().to_string()),
            ("x-hash-crc64ecma".to_string(), local_crc64.to_string()),
        ]),
        hash_payload(&body),
        Body::from(body),
    )
    .await?;
    verify_tos_crc64_response(&uploaded, local_crc64, "stdin upload part")?;
    let etag = uploaded
        .data
        .as_ref()
        .and_then(|data| upload_part_etag(&data.headers))
        .ok_or_else(|| CliError::ValidationError("UploadPart response missing ETag".to_string()))?;
    Ok(CompletedPart {
        part_number,
        etag,
        crc64: Some(local_crc64),
    })
}

async fn complete_tos_stdin_multipart(
    client: &TosClient,
    bucket: &str,
    key: &str,
    upload_id: &str,
    completed_parts: &[CompletedPart],
) -> Result<Envelope<core::RawResponseData>, CliError> {
    let (query, headers, body) = complete_multipart_request(upload_id, completed_parts)?;
    core::execute_object_request(
        client,
        "ve-tos put multipart complete",
        Method::POST,
        bucket,
        key,
        query,
        headers,
        Some(body),
    )
    .await
}

async fn abort_tos_multipart_upload(
    client: &TosClient,
    bucket: &str,
    key: &str,
    upload_id: &str,
) -> Result<(), CliError> {
    let _ = core::execute_object_request(
        client,
        "ve-tos put multipart abort",
        Method::DELETE,
        bucket,
        key,
        multipart_upload_id_query(upload_id),
        BTreeMap::new(),
        None,
    )
    .await?;
    Ok(())
}

fn verify_tos_crc64_response(
    response: &Envelope<core::RawResponseData>,
    local_crc64: u64,
    label: &str,
) -> Result<(), CliError> {
    if let Some(headers) = response.data.as_ref().map(|data| &data.headers) {
        if let Some(remote_crc64) = find_crc64_header(headers) {
            if remote_crc64 != local_crc64 {
                return Err(CliError::TransferFailed(format!(
                    "{} CRC64 mismatch: local={}, remote={}",
                    label, local_crc64, remote_crc64
                )));
            }
        }
    }
    Ok(())
}

async fn read_stream_part<R: AsyncRead + Unpin>(
    reader: &mut R,
    part_size: usize,
) -> Result<Vec<u8>, CliError> {
    // [Review Fix #3] Do not reserve the full user-configured threshold; a
    // huge threshold must not allocate huge memory before stdin is read.
    let mut part = Vec::with_capacity(part_size.min(64 * 1024));
    let mut buffer = [0_u8; 64 * 1024];
    while part.len() < part_size {
        let remaining = part_size - part.len();
        let read_len = buffer.len().min(remaining);
        let bytes_read = reader.read(&mut buffer[..read_len]).await?;
        if bytes_read == 0 {
            break;
        }
        part.extend_from_slice(&buffer[..bytes_read]);
    }
    Ok(part)
}

fn stdin_upload_progress(label: &'static str, progress_enabled: bool) -> Option<ProgressBar> {
    if !progress_enabled {
        return None;
    }
    let progress = ProgressBar::new_spinner();
    progress.set_style(
        ProgressStyle::with_template("{prefix} {spinner} {bytes} uploaded")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    progress.set_prefix(label);
    progress.enable_steady_tick(Duration::from_millis(200));
    Some(progress)
}

fn traversal_progress(label: &'static str, target: &str, enabled: bool) -> Option<ProgressBar> {
    if !enabled {
        return None;
    }
    let progress = ProgressBar::new_spinner();
    progress.set_style(
        ProgressStyle::with_template("{prefix} {spinner} traversing {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    progress.set_prefix(label);
    progress.set_message(short_label_from(target));
    progress.enable_steady_tick(Duration::from_millis(200));
    Some(progress)
}

fn finish_traversal_progress(progress: Option<ProgressBar>, total: u64) {
    if let Some(progress) = progress {
        progress.finish_with_message(format!("{total} item(s) traversed"));
    }
}

fn streaming_batch_progress(enabled: bool, label: &'static str) -> Option<ProgressBar> {
    if !enabled {
        return None;
    }
    let bar = ProgressBar::new_spinner();
    bar.set_style(
        ProgressStyle::with_template("{prefix} {spinner} {pos} item(s) processed ({elapsed})")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    bar.set_prefix(label);
    // [Review Fix #6] `--no-manifest` streams discovery into execution, so the
    // total is intentionally unknown until the operation finishes.
    bar.enable_steady_tick(Duration::from_millis(200));
    Some(bar)
}

fn finish_streaming_progress(progress: Option<ProgressBar>, total: u64) {
    if let Some(progress) = progress {
        progress.finish_with_message(format!("{total} item(s) processed"));
    }
}

struct RemoteScanProgress {
    bar: Option<ProgressBar>,
}

impl RemoteScanProgress {
    fn new(enabled: bool, label: &'static str, target: &str) -> Self {
        if !enabled {
            return Self { bar: None };
        }
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{prefix} {spinner} scanning {msg} ({elapsed})")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        bar.set_prefix(label);
        bar.set_message(short_label_from(target));
        // [Review Fix #5] Remote batch commands need visible progress while
        // ListObjects/ListVersions is still discovering the delete/restore plan.
        bar.enable_steady_tick(Duration::from_millis(200));
        Self { bar: Some(bar) }
    }

    fn finish_with_count(&mut self, count: u64, unit: &'static str) {
        if let Some(bar) = self.bar.take() {
            bar.finish_with_message(format!("{count} {unit} discovered"));
        }
    }

    fn finish_and_clear(&mut self) {
        if let Some(bar) = self.bar.take() {
            bar.finish_and_clear();
        }
    }
}

impl Drop for RemoteScanProgress {
    fn drop(&mut self) {
        self.finish_and_clear();
    }
}

fn effective_stdin_multipart_threshold(
    global: &GlobalArgs,
    cli_value: Option<&str>,
) -> Result<usize, CliError> {
    let profile = build_profile(global)?;
    let threshold = effective_size_value(
        cli_value,
        profile.checkpoint_threshold.as_deref(),
        DEFAULT_TRANSFER_CHECKPOINT_THRESHOLD,
        "multipart_threshold",
    )?;
    if threshold == 0 || threshold > usize::MAX as u64 {
        return Err(CliError::ValidationError(
            "multipart_threshold must be a positive size that fits this platform".to_string(),
        ));
    }
    Ok(threshold as usize)
}

async fn execute_presign(
    global: &GlobalArgs,
    client: &TosClient,
    args: &PresignArgs,
) -> Result<i32, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos presign",
    )?;
    let target = parse_tos_uri(&path, false)?;
    let key = target.key.expect("validated object key");
    let method = validate_presign_method(&args.method)?;
    let url = client.presign_object_url(method, &target.bucket, &key, args.expires)?;
    output_result(
        global,
        &high_level_success_envelope(
            "ve-tos presign",
            json!({
                "method": method,
                "url": url,
                "expires": args.expires,
            }),
        ),
    )?;
    Ok(0)
}

async fn execute_du(
    global: &GlobalArgs,
    client: &TosClient,
    args: &DuArgs,
) -> Result<i32, CliError> {
    validate_du_top_k(args.top_k)?;
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos du",
    )?;
    let manifest_path = effective_explicit_manifest_path(args.manifest_path.as_deref());
    let target = parse_tos_uri(&path, true)?;
    let price_table = storage_price_table(&args.storage_price)?;
    let is_hns_bucket = bucket_is_hns(client, &target.bucket).await?;
    let use_hierarchical_listing = resolve_tos_du_list_mode(is_hns_bucket);
    let list_concurrency = effective_list_concurrency(global, args.list_concurrency)?;
    let progress = traversal_progress(
        "ve-tos du",
        &path,
        effective_traversal_echo_enabled(
            global,
            args.list_echo,
            args.no_list_echo,
            args.progress,
            args.no_progress,
        ),
    );
    let profile = collect_tos_du_profile(
        client,
        &target.bucket,
        target.key.as_deref(),
        use_hierarchical_listing,
        list_concurrency,
        args,
    )
    .await?;
    finish_traversal_progress(progress, profile.object_count + profile.directory_count);
    let groups = args
        .max_depth
        .is_some()
        .then(|| profile.directory_distribution_json());
    let cost = if args.cost {
        Some(profile.cost_estimate(&price_table))
    } else {
        None
    };
    // [Review Fix #2] Materializing a full object manifest is only safe when explicitly requested.
    if manifest_path.is_some() {
        let manifest = build_list_manifest(profile.manifest_items.clone());
        write_list_manifest_file(manifest_path.as_deref(), "ve-tos du", &manifest)?;
    }
    let payload = du_output_payload(
        global,
        &path,
        &target,
        &profile,
        args,
        groups,
        cost,
        manifest_path.as_deref(),
        is_hns_bucket,
        use_hierarchical_listing,
        list_concurrency,
    );
    output_result(
        global,
        &high_level_success_envelope("ve-tos du", payload).without_request_id(),
    )?;
    Ok(0)
}

fn du_output_payload(
    global: &GlobalArgs,
    path: &str,
    target: &ParsedTosUri,
    profile: &DuAccumulator,
    args: &DuArgs,
    groups: Option<BTreeMap<String, Value>>,
    cost: Option<Value>,
    manifest_path: Option<&str>,
    is_hns_bucket: bool,
    use_hierarchical_listing: bool,
    list_concurrency: usize,
) -> Value {
    let mut payload = json!({
        "path": path,
        "bucket": target.bucket,
        "prefix": target.key,
        "object_count": profile.object_count,
        "directory_count": profile.directory_count,
        "total_bytes": profile.total_bytes,
        "storage_classes": profile.storage_class_distribution_json(),
    });
    let Some(map) = payload.as_object_mut() else {
        return payload;
    };
    if args.human_readable {
        map.insert(
            "human_readable".to_string(),
            Value::String(human_bytes(profile.total_bytes)),
        );
    }
    if let Some(groups) = groups {
        map.insert("groups".to_string(), json!(groups));
    }
    if let Some(cost) = cost {
        map.insert("cost".to_string(), cost);
    }
    if let Some(manifest_path) = manifest_path {
        map.insert(
            "manifest_path".to_string(),
            Value::String(manifest_path.to_string()),
        );
    }
    if global.verbose {
        insert_du_diagnostics(
            map,
            profile,
            is_hns_bucket,
            use_hierarchical_listing,
            list_concurrency,
        );
    }
    payload
}

fn insert_du_diagnostics(
    map: &mut serde_json::Map<String, Value>,
    profile: &DuAccumulator,
    is_hns_bucket: bool,
    use_hierarchical_listing: bool,
    list_concurrency: usize,
) {
    map.insert(
        "diagnostics".to_string(),
        json!({
            "file_types": profile.file_type_distribution_json(),
            "directories": profile.directory_distribution_json(),
            "size_histogram": profile.size_histogram_json(),
            "storage_classes": profile.storage_class_distribution_json(),
            "largest_objects": &profile.largest_objects,
            "oldest_objects": &profile.oldest_objects,
            "service_request_ids": &profile.request_ids,
            "service_request_ids_omitted": profile.request_ids_omitted,
            "limits": {
                "category_buckets": DU_CATEGORY_BUCKET_LIMIT,
                "service_request_ids": DU_REQUEST_ID_LIMIT,
                "overflow_bucket": DU_OVERFLOW_BUCKET,
            },
            "traversal": {
                "streaming": true,
                "page_size": 1000,
                "prefix_concurrency": if use_hierarchical_listing { list_concurrency.max(1) } else { 1 },
                "bucket_mode": if is_hns_bucket { "hns" } else { "fns" },
                "delimiter": if use_hierarchical_listing { "/" } else { "" },
                "memory_model": "O(category_bucket_limit + top_k + optional_manifest_items + request_id_limit)",
            },
        }),
    );
}

async fn execute_find(
    global: &GlobalArgs,
    client: &TosClient,
    args: &FindArgs,
) -> Result<i32, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos find",
    )?;
    let manifest_path = effective_explicit_manifest_path(args.manifest_path.as_deref());
    let target = parse_tos_uri(&path, true)?;
    let size_filter = args
        .size
        .as_deref()
        .map(parse_find_size_filter)
        .transpose()?;
    let mtime_filter = args
        .mtime
        .as_deref()
        .map(parse_find_mtime_filter)
        .transpose()?;
    let progress = traversal_progress(
        "ve-tos find",
        &path,
        effective_traversal_echo_enabled(
            global,
            args.list_echo,
            args.no_list_echo,
            args.progress,
            args.no_progress,
        ),
    );
    let entries =
        list_object_entries_for_bucket(client, &target.bucket, target.key.as_deref()).await?;
    let matches = entries
        .into_iter()
        .filter(|entry| {
            // [Review Fix #Tos-FindFilters] TOS and ADrive find share parsed
            // filter semantics: size bounds are inclusive and -mtime means
            // modified within the requested relative duration.
            find_entry_matches(entry, args, size_filter, mtime_filter.as_ref())
        })
        .collect::<Vec<_>>();
    finish_traversal_progress(progress, matches.len() as u64);
    let manifest = build_list_manifest(
        matches
            .iter()
            .map(|entry| object_entry_manifest_item(&target.bucket, target.key.as_deref(), entry))
            .collect(),
    );
    write_list_manifest_file(manifest_path.as_deref(), "ve-tos find", &manifest)?;
    output_result(
        global,
        &Envelope::success(
            "ve-tos find",
            json!({
                "path": path,
                "matches": matches,
                "manifest_path": manifest_path,
            }),
        )
        .with_pagination(PaginationInfo {
            next_token: None,
            next_marker: None,
            total_returned: matches.len() as u64,
        }),
    )?;
    Ok(0)
}

fn build_operation(command: &TosCommand) -> Result<HighLevelOperation, CliError> {
    match command {
        TosCommand::Cp(args) => cp_operation(args),
        TosCommand::Mv(args) => mv_operation(args),
        TosCommand::Sync(args) => sync_operation(args),
        TosCommand::Mb(args) => mb_operation(args),
        TosCommand::Rb(args) => rb_operation(args),
        TosCommand::Mkdir(args) => mkdir_operation(args),
        TosCommand::Rm(args) => rm_operation(args),
        TosCommand::Ls(args) => ls_operation(args),
        TosCommand::Stat(args) => stat_operation(args),
        TosCommand::Du(args) => du_operation(args),
        TosCommand::Find(args) => find_operation(args),
        TosCommand::Cat(args) => cat_operation(args),
        TosCommand::Put(args) => put_operation(args),
        TosCommand::Presign(args) => presign_operation(args),
        TosCommand::Restore(args) => restore_operation(args),
        _ => Err(CliError::ValidationError(
            "unsupported high-level command".to_string(),
        )),
    }
}

fn tos_high_level_command_path(command: &TosCommand) -> &'static str {
    match command {
        TosCommand::Cp(_) => "ve-tos cp",
        TosCommand::Mv(_) => "ve-tos mv",
        TosCommand::Sync(_) => "ve-tos sync",
        TosCommand::Mb(_) => "ve-tos mb",
        TosCommand::Rb(_) => "ve-tos rb",
        TosCommand::Mkdir(_) => "ve-tos mkdir",
        TosCommand::Rm(_) => "ve-tos rm",
        TosCommand::Ls(_) => "ve-tos ls",
        TosCommand::Stat(_) => "ve-tos stat",
        TosCommand::Du(_) => "ve-tos du",
        TosCommand::Find(_) => "ve-tos find",
        TosCommand::Cat(_) => "ve-tos cat",
        TosCommand::Put(_) => "ve-tos put",
        TosCommand::Presign(_) => "ve-tos presign",
        TosCommand::Restore(_) => "ve-tos restore",
        _ => "tos",
    }
}

fn cp_operation(args: &CpArgs) -> Result<HighLevelOperation, CliError> {
    validate_transfer_pair(&args.source, &args.destination, args.recursive)?;
    let write_options = copy_write_options(args)?;
    ensure_tos_write_destination("ve-tos cp", &args.destination, &write_options)?;
    ensure_tos_upload_storage_class_supported(
        "ve-tos cp",
        Some(&args.source),
        &args.destination,
        write_options.storage_class.as_deref(),
    )?;
    let destination = if args.recursive {
        args.destination.clone()
    } else {
        resolve_single_transfer_destination(&args.source, &args.destination)?
    };
    Ok(HighLevelOperation {
        command: "ve-tos cp",
        description: "Copy local files, objects, or prefixes between local and TOS.",
        risk: if args.force {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        },
        target: format!("{} -> {}", args.source, destination),
        source: Some(args.source.clone()),
        destination: Some(destination),
        batch_enabled: args.recursive,
        batch_source: if args.recursive {
            "recursive"
        } else {
            "single"
        },
        list_echo_requested: args.list_echo,
        list_echo_disabled: args.no_list_echo,
        progress_requested: args.progress,
        progress_disabled: args.no_progress,
        checkpoint_enabled: args.checkpoint,
        checkpoint_dir: args.checkpoint_dir.clone(),
        report_path: args.report_path.clone(),
        force: args.force,
        requires_force: false,
        consistency_guards: vec![
            "download: GetObject uses If-Match or version_id and writes temp file before rename",
            "upload: client streams CRC64 and compares it with TOS response crc64",
            "copy: CopyObject uses copy-source-if-match when source etag is known",
            "copy: remote-to-remote copy is preflighted to same-region buckets only",
        ],
        low_level_apis: vec![
            "HeadObject/ListObjects",
            "GetObject",
            "PutObject",
            "CopyObject",
        ],
        confirm_command: None,
        parameters: transfer_parameters(true),
    })
}

fn mv_operation(args: &MvArgs) -> Result<HighLevelOperation, CliError> {
    validate_transfer_pair(&args.source, &args.destination, args.recursive)?;
    let write_options = mv_write_options(args)?;
    ensure_tos_write_destination("ve-tos mv", &args.destination, &write_options)?;
    ensure_tos_upload_storage_class_supported(
        "ve-tos mv",
        Some(&args.source),
        &args.destination,
        write_options.storage_class.as_deref(),
    )?;
    let destination = if args.recursive {
        args.destination.clone()
    } else {
        resolve_single_transfer_destination(&args.source, &args.destination)?
    };
    Ok(HighLevelOperation {
        command: "ve-tos mv",
        description: "Move files or objects by copy plus source delete.",
        risk: RiskLevel::Critical,
        target: format!("{} -> {}", args.source, destination),
        source: Some(args.source.clone()),
        destination: Some(destination),
        batch_enabled: args.recursive,
        batch_source: if args.recursive {
            "recursive"
        } else {
            "single"
        },
        list_echo_requested: args.list_echo,
        list_echo_disabled: args.no_list_echo,
        progress_requested: args.progress,
        progress_disabled: args.no_progress,
        checkpoint_enabled: false,
        checkpoint_dir: args.checkpoint_dir.clone(),
        report_path: args.report_path.clone(),
        force: args.force,
        requires_force: true,
        consistency_guards: vec![
            "copy phase follows cp consistency guards",
            "source delete happens only after destination write confirmation",
        ],
        low_level_apis: vec![
            "HeadObject/ListObjects",
            "GetObject",
            "PutObject",
            "DeleteObject",
        ],
        confirm_command: Some(format!(
            "rerun with --force --confirm {} after reviewing dry-run plan",
            args.source
        )),
        parameters: transfer_parameters(false),
    })
}

fn sync_operation(args: &SyncArgs) -> Result<HighLevelOperation, CliError> {
    validate_transfer_pair(&args.source, &args.destination, true)?;
    let write_options = sync_write_options(args)?;
    ensure_tos_write_destination("ve-tos sync", &args.destination, &write_options)?;
    ensure_tos_upload_storage_class_supported(
        "ve-tos sync",
        Some(&args.source),
        &args.destination,
        write_options.storage_class.as_deref(),
    )?;
    Ok(HighLevelOperation {
        command: "ve-tos sync",
        description: "Synchronize source and destination incrementally.",
        risk: if args.delete {
            RiskLevel::Critical
        } else {
            RiskLevel::Medium
        },
        target: format!("{} -> {}", args.source, args.destination),
        source: Some(args.source.clone()),
        destination: Some(args.destination.clone()),
        batch_enabled: true,
        batch_source: "recursive",
        list_echo_requested: args.list_echo,
        list_echo_disabled: args.no_list_echo,
        progress_requested: args.progress,
        progress_disabled: args.no_progress,
        checkpoint_enabled: true,
        checkpoint_dir: args.checkpoint_dir.clone(),
        report_path: args.report_path.clone(),
        force: args.force,
        requires_force: args.delete,
        consistency_guards: vec![
            "plan compares size and mtime unless size-only or exact-timestamps changes strategy",
            "transfer phase follows cp consistency guards",
            "delete phase requires --force and records every removed item",
        ],
        low_level_apis: vec![
            "ListObjects",
            "HeadObject",
            "GetObject",
            "PutObject",
            "DeleteObject",
        ],
        confirm_command: args.delete.then(|| {
            format!(
                "rerun with --force --confirm {} after reviewing delete plan",
                args.destination
            )
        }),
        parameters: sync_parameters(),
    })
}

fn mb_operation(args: &MbArgs) -> Result<HighLevelOperation, CliError> {
    validate_bucket_target(&args.bucket)?;
    Ok(HighLevelOperation {
        command: "ve-tos mb",
        description: "Create a bucket with optional storage class, ACL, and redundancy settings.",
        risk: RiskLevel::Low,
        target: args.bucket.clone(),
        source: None,
        destination: None,
        batch_enabled: false,
        batch_source: "single",
        list_echo_requested: false,
        list_echo_disabled: true,
        progress_requested: false,
        progress_disabled: true,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: None,
        force: false,
        requires_force: false,
        consistency_guards: vec!["create bucket is idempotency-aware at bucket name granularity"],
        low_level_apis: vec!["CreateBucket", "PutBucketAcl"],
        confirm_command: None,
        parameters: mb_parameters(),
    })
}

fn rb_operation(args: &RbArgs) -> Result<HighLevelOperation, CliError> {
    validate_bucket_target(&args.bucket)?;
    Ok(HighLevelOperation {
        command: "ve-tos rb",
        description: "Remove an empty bucket.",
        risk: RiskLevel::Critical,
        target: args.bucket.clone(),
        source: None,
        destination: None,
        batch_enabled: false,
        batch_source: "single",
        list_echo_requested: false,
        list_echo_disabled: true,
        progress_requested: false,
        progress_disabled: true,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: None,
        force: args.force,
        requires_force: true,
        consistency_guards: vec!["bucket contents are not cleaned implicitly; use ve-tos rm first"],
        low_level_apis: vec!["DeleteBucket"],
        confirm_command: Some(format!(
            "rerun with --force --confirm {} after reviewing dry-run plan",
            args.bucket
        )),
        parameters: vec![
            param(
                "bucket",
                ParameterLocation::Path,
                true,
                "Bucket name or tos://bucket",
            ),
            param(
                "force",
                ParameterLocation::Flag,
                false,
                "Confirm bucket deletion",
            ),
        ],
    })
}

fn rm_operation(args: &RmArgs) -> Result<HighLevelOperation, CliError> {
    let path = resolve_rm_path(args)?;
    validate_tos_uri(&path, args.recursive)?;
    let recursive_delete_guard = if std::env::var("VE_STORAGE_UNI_TOS_FORCE_FNS_DELETE")
        .ok()
        .as_deref()
        == Some("1")
    {
        // [Review Fix #10] ByteCloud tos does not expose the ve-tos HNS direct
        // recursive delete mode, so dry-run guidance must describe the actual
        // planned object-delete behavior selected by the tos-cli wrapper.
        "recursive deletes use planned object deletes with delimiter=\"/\""
    } else {
        "HNS recursive deletes can run bottom-up or use service-side direct recursion"
    };
    Ok(HighLevelOperation {
        command: "ve-tos rm",
        description: "Delete an object or prefix.",
        risk: RiskLevel::Critical,
        target: path.clone(),
        source: Some(path.clone()),
        destination: None,
        batch_enabled: args.recursive,
        batch_source: if args.recursive {
            "recursive"
        } else {
            "single"
        },
        list_echo_requested: args.list_echo,
        list_echo_disabled: args.no_list_echo,
        progress_requested: args.progress,
        progress_disabled: args.no_progress,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: args.report_path.clone(),
        force: args.force,
        requires_force: true,
        consistency_guards: vec![
            "delete operations are planned first and require --force",
            recursive_delete_guard,
        ],
        low_level_apis: vec!["HeadBucket", "HeadObject/ListObjects", "DeleteObject"],
        confirm_command: Some(format!(
            "rerun with --force --confirm {} after reviewing dry-run plan",
            path
        )),
        parameters: rm_parameters(),
    })
}

fn mkdir_operation(args: &MkdirArgs) -> Result<HighLevelOperation, CliError> {
    let target = resolve_mkdir_target(args)?;
    let key = target.key.as_deref().ok_or_else(|| {
        CliError::ValidationError("ve-tos mkdir requires tos://bucket/folder".to_string())
    })?;
    let path = format!("tos://{}/{}", target.bucket, key);
    Ok(HighLevelOperation {
        command: "ve-tos mkdir",
        description: "Create a folder.",
        risk: RiskLevel::Medium,
        target: path.clone(),
        source: None,
        destination: Some(path),
        batch_enabled: args.parents,
        batch_source: if args.parents { "parents" } else { "single" },
        list_echo_requested: false,
        list_echo_disabled: true,
        progress_requested: false,
        progress_disabled: true,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: None,
        force: false,
        requires_force: false,
        consistency_guards: vec![
            "folder targets are normalized to a trailing slash before PutObject",
        ],
        low_level_apis: vec!["PutObject"],
        confirm_command: None,
        parameters: vec![
            param(
                "path",
                ParameterLocation::Path,
                true,
                "tos://bucket/folder/",
            ),
            param(
                "bucket",
                ParameterLocation::Path,
                false,
                "Bucket name when path is omitted",
            ),
            param(
                "key",
                ParameterLocation::Path,
                false,
                "Folder key when --bucket is used",
            ),
            param(
                "parents",
                ParameterLocation::Flag,
                false,
                "Create parent folder markers as needed",
            ),
            param(
                "content-type",
                ParameterLocation::Header,
                true,
                "Fixed application/x-directory for created folder markers",
            ),
        ],
    })
}

fn resolve_rm_path(args: &RmArgs) -> Result<String, CliError> {
    resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos rm",
    )
}

fn resolve_mkdir_target(args: &MkdirArgs) -> Result<ParsedTosUri, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos mkdir",
    )?;
    let mut target = parse_tos_uri(&path, false)?;
    let key = target.key.take().ok_or_else(|| {
        CliError::ValidationError("ve-tos mkdir requires tos://bucket/folder".to_string())
    })?;
    target.key = Some(normalize_folder_key(&key)?);
    Ok(target)
}

fn normalize_folder_key(key: &str) -> Result<String, CliError> {
    let normalized = key.trim_start_matches('/');
    if normalized.trim().is_empty() || normalized.trim_matches('/').is_empty() {
        return Err(CliError::ValidationError(
            "ve-tos mkdir requires a non-empty folder key".to_string(),
        ));
    }
    if normalized.ends_with('/') {
        Ok(normalized.to_string())
    } else {
        Ok(format!("{normalized}/"))
    }
}

fn folder_keys_for_mkdir(key: &str, parents: bool) -> Vec<String> {
    if !parents {
        return vec![key.to_string()];
    }
    let mut keys = Vec::new();
    let mut current = String::new();
    for segment in key.trim_end_matches('/').split('/') {
        if segment.is_empty() {
            continue;
        }
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(segment);
        keys.push(format!("{current}/"));
    }
    if keys.is_empty() {
        vec![key.to_string()]
    } else {
        keys
    }
}

fn resolve_target_path(
    path: Option<&str>,
    bucket: Option<&str>,
    key: Option<&str>,
    command: &str,
) -> Result<String, CliError> {
    if let Some(path) = path {
        return Ok(path.to_string());
    }
    let bucket = bucket.ok_or_else(|| {
        CliError::ValidationError(format!(
            "{}: missing target; provide tos://bucket/key positional argument or --bucket",
            command
        ))
    })?;
    if bucket.starts_with("tos://") {
        return Err(CliError::ValidationError(format!(
            "{}: --bucket expects a bucket name only; use the positional tos://bucket/key form for URI targets",
            command
        )));
    }
    if bucket.trim().is_empty() || bucket.contains('/') {
        return Err(CliError::ValidationError(format!(
            "{}: --bucket expects a bucket name only",
            command
        )));
    }
    let bucket_name = format!("tos://{}", bucket);
    if let Some(key) = key {
        Ok(format!("{}/{}", bucket_name, key))
    } else {
        Ok(bucket_name)
    }
}

fn ls_operation(args: &LsArgs) -> Result<HighLevelOperation, CliError> {
    validate_high_level_ls_max_keys(args.max_keys)?;
    if let Some(path) = &args.path {
        validate_tos_uri(path, true)?;
    }
    let target_str = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos ls",
    )
    .unwrap_or_else(|_| "all buckets".to_string());
    Ok(HighLevelOperation {
        command: "ve-tos ls",
        description: "List buckets or objects.",
        risk: RiskLevel::Low,
        target: target_str,
        source: args.path.clone(),
        destination: None,
        batch_enabled: false,
        batch_source: "listing",
        list_echo_requested: false,
        list_echo_disabled: true,
        progress_requested: false,
        progress_disabled: true,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: None,
        force: false,
        requires_force: false,
        consistency_guards: vec!["listing is read-only and emits deterministic pagination plan"],
        low_level_apis: vec!["ListBuckets", "ListObjects"],
        confirm_command: None,
        parameters: ls_parameters(),
    })
}

fn stat_operation(args: &StatArgs) -> Result<HighLevelOperation, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos stat",
    )?;
    validate_tos_uri(&path, true)?;
    Ok(HighLevelOperation {
        command: "ve-tos stat",
        description: "Show bucket or object metadata.",
        risk: RiskLevel::Low,
        target: path.clone(),
        source: Some(path),
        destination: None,
        batch_enabled: false,
        batch_source: "single",
        list_echo_requested: false,
        list_echo_disabled: true,
        progress_requested: false,
        progress_disabled: true,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: None,
        force: false,
        requires_force: false,
        consistency_guards: vec![
            "metadata read can pin an object version when version_id is provided",
        ],
        low_level_apis: vec!["HeadBucket", "HeadObject"],
        confirm_command: None,
        parameters: vec![
            param(
                "path",
                ParameterLocation::Path,
                true,
                "tos://bucket or tos://bucket/key",
            ),
            param(
                "version-id",
                ParameterLocation::Query,
                false,
                "Object version ID",
            ),
        ],
    })
}

fn du_operation(args: &DuArgs) -> Result<HighLevelOperation, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos du",
    )?;
    validate_tos_uri(&path, true)?;
    Ok(HighLevelOperation {
        command: "ve-tos du",
        description: "Calculate object size statistics for a prefix.",
        risk: RiskLevel::Low,
        target: path.clone(),
        source: Some(path),
        destination: None,
        batch_enabled: true,
        batch_source: "recursive",
        list_echo_requested: args.list_echo || (!args.no_list_echo && args.progress),
        list_echo_disabled: args.no_list_echo || (!args.list_echo && args.no_progress),
        progress_requested: false,
        progress_disabled: true,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: None,
        force: false,
        requires_force: false,
        consistency_guards: vec!["read-only traversal records deterministic summary output"],
        low_level_apis: vec!["ListObjects"],
        confirm_command: None,
        parameters: du_parameters(),
    })
}

fn find_operation(args: &FindArgs) -> Result<HighLevelOperation, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos find",
    )?;
    validate_tos_uri(&path, true)?;
    if let Some(size) = &args.size {
        validate_size_filter(size)?;
    }
    if let Some(mtime) = &args.mtime {
        validate_mtime_filter(mtime)?;
    }
    Ok(HighLevelOperation {
        command: "ve-tos find",
        description: "Find objects by name, size, mtime, or storage class.",
        risk: RiskLevel::Low,
        target: path.clone(),
        source: Some(path),
        destination: None,
        batch_enabled: true,
        batch_source: "recursive",
        list_echo_requested: args.list_echo || (!args.no_list_echo && args.progress),
        list_echo_disabled: args.no_list_echo || (!args.list_echo && args.no_progress),
        progress_requested: false,
        progress_disabled: true,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: None,
        force: false,
        requires_force: false,
        consistency_guards: vec!["read-only traversal applies filters after deterministic listing"],
        low_level_apis: vec!["ListObjects"],
        confirm_command: None,
        parameters: find_parameters(),
    })
}

fn cat_operation(args: &CatArgs) -> Result<HighLevelOperation, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos cat",
    )?;
    validate_tos_uri(&path, false)?;
    Ok(HighLevelOperation {
        command: "ve-tos cat",
        description: "Stream object content to stdout.",
        risk: RiskLevel::Low,
        target: path.clone(),
        source: Some(path),
        destination: Some("stdout".to_string()),
        batch_enabled: false,
        batch_source: "single",
        list_echo_requested: false,
        list_echo_disabled: true,
        progress_requested: false,
        progress_disabled: true,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: None,
        force: false,
        requires_force: false,
        consistency_guards: vec![
            // [Review Fix #X2] cat is best-effort streaming and never issues a
            // HEAD round-trip; --range and --version-id are honored directly.
            "streams raw object body to stdout; honors --range and --version-id without extra HEAD",
        ],
        low_level_apis: vec!["GetObject"],
        confirm_command: None,
        parameters: vec![
            param("path", ParameterLocation::Path, true, "tos://bucket/key"),
            param("range", ParameterLocation::Header, false, "HTTP range"),
            param(
                "version-id",
                ParameterLocation::Query,
                false,
                "Object version ID",
            ),
        ],
    })
}

fn put_operation(args: &PutArgs) -> Result<HighLevelOperation, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos put",
    )?;
    validate_tos_uri(&path, false)?;
    let write_options = put_write_options(args)?;
    ensure_tos_upload_storage_class_supported(
        "ve-tos put",
        None,
        &path,
        write_options.storage_class.as_deref(),
    )?;
    Ok(HighLevelOperation {
        command: "ve-tos put",
        description: "Upload stdin to an object; upload starts/completes after stdin EOF.",
        risk: RiskLevel::Medium,
        target: path.clone(),
        source: Some("stdin".to_string()),
        destination: Some(path),
        batch_enabled: false,
        batch_source: "single",
        list_echo_requested: false,
        list_echo_disabled: true,
        progress_requested: args.progress,
        progress_disabled: args.no_progress,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: None,
        force: !args.no_clobber,
        requires_force: false,
        consistency_guards: vec![
            "interactive stdin is submitted with EOF (Ctrl+D on Unix/macOS; Ctrl+Z then Enter on Windows), while Ctrl+C cancels the command",
            "stdin upload streams in bounded multipart chunks when input reaches multipart size",
            "each upload part carries CRC64 and the completed object CRC64 is compared when TOS returns it",
        ],
        low_level_apis: vec!["PutObject", "CreateMultipartUpload", "UploadPart", "CompleteMultipartUpload"],
        confirm_command: None,
        parameters: put_parameters(),
    })
}

fn presign_operation(args: &PresignArgs) -> Result<HighLevelOperation, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos presign",
    )?;
    validate_tos_uri(&path, false)?;
    Ok(HighLevelOperation {
        command: "ve-tos presign",
        description: "Generate a presigned URL for object access.",
        risk: RiskLevel::Medium,
        target: path.clone(),
        source: Some(path),
        destination: None,
        batch_enabled: false,
        batch_source: "single",
        list_echo_requested: false,
        list_echo_disabled: true,
        progress_requested: false,
        progress_disabled: true,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: None,
        force: false,
        requires_force: false,
        consistency_guards: vec!["presigned URL scope is constrained by method and expiration"],
        low_level_apis: vec!["SignV4"],
        confirm_command: None,
        parameters: vec![
            param("path", ParameterLocation::Path, true, "tos://bucket/key"),
            param(
                "expires",
                ParameterLocation::Query,
                false,
                "URL expiration seconds",
            ),
            param("method", ParameterLocation::Query, false, "HTTP method"),
        ],
    })
}

fn restore_operation(args: &RestoreArgs) -> Result<HighLevelOperation, CliError> {
    let path = resolve_target_path(
        args.path.as_deref(),
        args.bucket.as_deref(),
        args.key.as_deref(),
        "ve-tos restore",
    )?;
    validate_tos_uri(&path, args.recursive)?;
    let batch_enabled = args.recursive || args.manifest.is_some();
    Ok(HighLevelOperation {
        command: "ve-tos restore",
        description: "Restore archived objects, including recursive and manifest-driven batches.",
        risk: if batch_enabled {
            RiskLevel::High
        } else {
            RiskLevel::Medium
        },
        target: path.clone(),
        source: Some(path),
        destination: None,
        batch_enabled,
        batch_source: if args.manifest.is_some() {
            "manifest"
        } else if args.recursive {
            "recursive"
        } else {
            "single"
        },
        list_echo_requested: args.list_echo,
        list_echo_disabled: args.no_list_echo,
        progress_requested: args.progress,
        progress_disabled: args.no_progress,
        checkpoint_enabled: false,
        checkpoint_dir: None,
        report_path: args.report_path.clone(),
        force: args.force,
        requires_force: batch_enabled,
        consistency_guards: vec![
            "restore requests record per-object success and failure for retry",
        ],
        low_level_apis: vec!["ListObjects", "RestoreObject"],
        confirm_command: batch_enabled
            .then(|| "rerun with --force after reviewing restore cost impact".to_string()),
        parameters: restore_parameters(),
    })
}

/// Maximum number of objects to enumerate during dry-run impact assessment.
/// Beyond this cap the preview is marked as truncated to avoid runaway listings
/// on huge prefixes.
const MAX_PREVIEW_OBJECTS: u64 = 10_000;

/// Lower bound for an estimated single-object delete duration (best effort heuristic).
const ESTIMATED_DELETE_MS_PER_OBJECT: u64 = 5;

/// Enforce the critical delete confirmation gate.
///
/// Spec rule (§4.5 Safe Execution):
/// - Interactive context: `--force` executes directly; without `--force` the
///   normal destructive prompt handles explicit user confirmation.
/// - Pipe (non-interactive) context: caller must pass both `--force` and
///   `--confirm <RESOURCE>`, whose value exactly matches the affected URI.
///
/// On violation a `ValidationError` (exit code 6) is returned with a fix hint.
fn enforce_critical_confirmation(
    global: &GlobalArgs,
    operation: &HighLevelOperation,
    can_prompt: bool,
) -> Result<(), CliError> {
    let expected = critical_confirmation_token(operation);

    if can_prompt {
        return Ok(());
    }

    if !operation.force {
        return Err(CliError::ValidationError(format!(
            "critical delete command '{}' targeting '{}' requires --force and --confirm {} in non-interactive execution",
            operation.command, operation.target, expected
        )));
    }

    if let Some(provided) = global.confirm.as_deref() {
        if provided == expected {
            return Ok(());
        }
        return Err(CliError::ValidationError(format!(
            "--confirm '{}' does not match the critical resource '{}' for {}",
            provided, expected, operation.command
        )));
    }

    Err(CliError::ValidationError(format!(
        "critical delete command '{}' targeting '{}' requires --confirm {} in non-interactive execution",
        operation.command, operation.target, expected
    )))
}

/// Derive the token a caller must echo back via `--confirm` to authorise a
/// critical operation. Bucket-level targets are normalized to `tos://bucket`
/// so TOS and ADrive both use their public URI forms.
fn critical_confirmation_token(operation: &HighLevelOperation) -> String {
    if operation.command == "ve-tos rb" {
        if let Ok(parsed) = parse_tos_uri_or_bucket(operation.target.as_str()) {
            return format!("tos://{}", parsed.bucket);
        }
    }
    if operation.command == "ve-tos mv" {
        if let Some(source) = &operation.source {
            return source.clone();
        }
    }
    if operation.command == "ve-tos sync" {
        if let Some(destination) = &operation.destination {
            return destination.clone();
        }
    }
    operation.target.clone()
}

/// Compute a real-world impact assessment for destructive High-Level operations.
///
/// For `ve-tos rm`, `ve-tos rb`, and `ve-tos sync --delete` we issue a deterministic
/// `ListObjects` traversal (capped by [`MAX_PREVIEW_OBJECTS`]) so the Agent
/// receives `affected_objects` and `affected_bytes` instead of the previous
/// "unknown until discovery" placeholder. For non-destructive or non-prefix
/// commands this returns `None` to keep dry-run side-effect free.
/// [Review Fix #M2/m2] Returns whether a high-level command needs to perform
/// a remote ListObjects/ListVersions call to compute its dry-run impact
/// (affected_objects / affected_bytes). Read-only commands such as `ve-tos cat`,
/// `ve-tos ls`, `ve-tos stat`, `ve-tos du`, `ve-tos find`, `ve-tos presign` MUST return
/// `false` so that `--dry-run` stays a pure planning operation and never
/// issues a GetObject / ListObjects against TOS.
pub(crate) fn command_needs_impact_listing(command: &str) -> bool {
    matches!(command, "ve-tos rm" | "ve-tos rb" | "ve-tos sync")
}

async fn compute_dry_run_impact(
    global: &GlobalArgs,
    operation: &HighLevelOperation,
) -> Option<Impact> {
    if !command_needs_impact_listing(operation.command) {
        return None;
    }

    // Pick the listing target.
    let raw_target = match operation.command {
        "ve-tos rb" => operation.target.as_str(),
        _ => operation
            .source
            .as_deref()
            .unwrap_or(operation.target.as_str()),
    };

    let parsed = if operation.command == "ve-tos rb" {
        parse_tos_uri_or_bucket(raw_target).ok()?
    } else if raw_target.starts_with("tos://") {
        parse_tos_uri(raw_target, true).ok()?
    } else {
        return None;
    };

    let profile = build_profile(global).ok()?;
    let client = TosClient::new(&profile, "tos").ok()?;

    let prefix = parsed.key.as_deref();
    let entries = match list_object_entries_for_bucket(&client, &parsed.bucket, prefix).await {
        Ok(entries) => entries,
        Err(_) => return None,
    };

    let mut affected_objects: u64 = 0;
    let mut affected_bytes: u64 = 0;
    // [Review Fix #m3] Track total entries inspected and whether the preview
    // was capped at MAX_PREVIEW_OBJECTS so Agents can distinguish a complete
    // scan from a truncated one when reasoning about destructive scope.
    let scanned_count = entries.len() as u64;
    let preview_truncated = scanned_count >= MAX_PREVIEW_OBJECTS;
    for entry in entries.iter().take(MAX_PREVIEW_OBJECTS as usize) {
        affected_objects += 1;
        affected_bytes = affected_bytes.saturating_add(entry.size);
    }

    let estimated_ms = affected_objects.saturating_mul(ESTIMATED_DELETE_MS_PER_OBJECT);
    let estimated_duration = if affected_objects == 0 {
        None
    } else if estimated_ms < 1_000 {
        Some(format!("{}ms", estimated_ms))
    } else {
        Some(format!("{:.1}s", estimated_ms as f64 / 1000.0))
    };

    Some(Impact {
        affected_objects,
        affected_bytes,
        risk_level: format!("{:?}", operation.risk).to_lowercase(),
        estimated_duration,
        scanned_count: Some(scanned_count),
        preview_truncated: Some(preview_truncated),
    })
}

async fn build_plan(
    global: &GlobalArgs,
    operation: &HighLevelOperation,
    path_traversal_confirm_target: Option<&str>,
) -> Result<HighLevelPlan, CliError> {
    let profile = build_profile(global)?;
    let checkpoint_dir = operation.checkpoint_dir.clone().unwrap_or_else(|| {
        scoped_default_path(
            profile
                .checkpoint_dir
                .as_deref()
                .unwrap_or(DEFAULT_TOS_CHECKPOINT_DIR),
            DEFAULT_TOS_CHECKPOINT_DIR,
        )
    });
    let report_dir = scoped_default_path(
        profile
            .batch_report_dir
            .as_deref()
            .unwrap_or(DEFAULT_TOS_BATCH_REPORT_DIR),
        DEFAULT_TOS_BATCH_REPORT_DIR,
    );
    let report_path = operation
        .report_path
        .clone()
        .unwrap_or_else(|| format!("{}/{}.csv", report_dir, report_name(operation.command)));
    let config_progress = profile
        .progress_enabled
        .unwrap_or(DEFAULT_TOS_PROGRESS_ENABLED);
    let list_echo = resolve_list_echo_plan(
        global,
        operation.list_echo_requested,
        operation.list_echo_disabled,
    );
    let progress = resolve_progress_plan(
        global,
        config_progress,
        operation.progress_requested,
        operation.progress_disabled,
    );

    let mut plan = vec![
        format!("DISCOVER target {}", operation.target),
        "BUILD deterministic execution graph".to_string(),
    ];
    if operation.batch_enabled {
        plan.push(format!(
            "EXECUTE batch from {} with success/failure report",
            operation.batch_source
        ));
    } else {
        plan.push("EXECUTE single-resource operation".to_string());
    }
    if operation.checkpoint_enabled {
        plan.push("RESUME with stable task fingerprint and checkpoint lock".to_string());
    }
    plan.push(
        "EMIT controlled output; list echo and progress go to stderr when enabled".to_string(),
    );
    let summary = plan_summary(operation);
    let filters = plan_filters(operation);
    let request_plan = request_plan_steps(operation);
    let samples = plan_samples(operation);
    let impact = compute_dry_run_impact(global, operation).await;
    let mut warnings = dry_run_warnings(operation, path_traversal_confirm_target);
    if let Some(ref imp) = impact {
        // [Review Fix #m3] Prefer the explicit truncation flag now produced by
        // `compute_dry_run_impact`; fall back to the cap comparison so older
        // call sites that hand-build `Impact` keep working.
        let truncated =
            imp.preview_truncated.unwrap_or(false) || imp.affected_objects >= MAX_PREVIEW_OBJECTS;
        if truncated {
            warnings.push(format!(
                "dry-run preview truncated at {} objects; actual scope may be larger",
                MAX_PREVIEW_OBJECTS
            ));
        }
    }

    Ok(HighLevelPlan {
        command: public_high_level_command(operation.command),
        dry_run: true,
        execution_status: "planned_not_executed",
        target: operation.target.clone(),
        source: operation.source.clone(),
        destination: operation.destination.clone(),
        batch: BatchPlan {
            enabled: operation.batch_enabled,
            source: operation.batch_source,
            records_success_failure: operation.batch_enabled || operation.report_path.is_some(),
        },
        progress: ProgressPlan {
            enabled: progress.enabled,
            render_to: progress.render_to,
            disabled_reason: progress.disabled_reason,
        },
        list_echo: ProgressPlan {
            enabled: list_echo.enabled,
            render_to: list_echo.render_to,
            disabled_reason: list_echo.disabled_reason,
        },
        checkpoint: CheckpointPlan {
            enabled: operation.checkpoint_enabled,
            directory: checkpoint_dir,
            identity: "stable_task_fingerprint",
            lock: "atomic_checkpoint_lock",
        },
        report: ReportPlan {
            path: report_path,
            format: "csv",
        },
        summary,
        impact,
        filters,
        request_plan,
        samples,
        consistency_guards: operation.consistency_guards.clone(),
        low_level_apis: operation.low_level_apis.clone(),
        plan,
        warnings,
        confirm_command: operation
            .confirm_command
            .as_deref()
            .map(public_high_level_command),
    })
}

fn describe_operation(operation: &HighLevelOperation) -> CommandDescription {
    let mut routing = HashMap::new();
    routing.insert("batch".to_string(), operation.batch_source.to_string());
    routing.insert(
        "progress".to_string(),
        "execution stderr; auto-enabled on TTY, disabled by --no-progress or --quiet, forced by --progress".to_string(),
    );
    routing.insert(
        "list_echo".to_string(),
        "listing stderr; auto-enabled on TTY, disabled by --no-list-echo or --quiet, forced by --list-echo"
            .to_string(),
    );
    routing.insert(
        "checkpoint".to_string(),
        "stable task fingerprint plus atomic lock".to_string(),
    );
    routing.insert(
        "output".to_string(),
        "structured dry-run, report path, and deterministic errors".to_string(),
    );

    CommandDescription {
        command: operation.command.to_string(),
        layer: CommandLayer::HighLevel,
        api: Some(operation.low_level_apis.join(" + ")),
        description: operation.description.to_string(),
        risk_level: operation.risk,
        supports_dry_run: true,
        supports_pipe: matches!(operation.command, "ve-tos cat" | "ve-tos put"),
        parameters: Some(operation.parameters.clone()),
        scenario_routing: Some(routing),
        related_commands: Some(RelatedCommands {
            high_level: None,
            low_level: Some(
                operation
                    .low_level_apis
                    .iter()
                    .map(|api| api.to_string())
                    .collect(),
            ),
        }),
        // [Review Fix #6] 直接复用 operation 中由 registry 维护的底层 API 列表，
        // 让 Agent 通过 --describe 即可拿到能力推理所需的底层依赖。
        low_level_apis: Some(
            operation
                .low_level_apis
                .iter()
                .map(|api| api.to_string())
                .collect(),
        ),
        // [G5] Spec-mandated alias for low_level_apis. Mirrored explicitly
        // because the field is also used by capabilities discovery.
        wraps_apis: Some(
            operation
                .low_level_apis
                .iter()
                .map(|api| api.to_string())
                .collect(),
        ),
        // [G5] Generic JMESPath examples that work for the high-level
        // dry-run/report payloads. Concrete commands can override.
        output_filter_examples: Some(high_level_filter_examples(operation.command)),
        // [G5] Quoting/escaping reminders shared across all transfer commands.
        shell_quoting_tips: Some(high_level_quoting_tips()),
        ..Default::default()
    }
}

/// [G5] Per-command JMESPath snippets that show Agents how to filter the
/// structured output of a high-level command without re-reading the full
/// envelope.
fn high_level_filter_examples(command: &str) -> Vec<String> {
    let public_command = crate::registry::public_tos_command(command);
    let mut examples = vec![
        // Common: pull just the `data` payload of a successful envelope.
        "COMMAND --output json | jq '.data'".to_string(),
        // Use the built-in --query (full JMESPath) to walk into the envelope.
        "COMMAND --query 'data.summary'".to_string(),
    ];
    match command {
        "ve-tos cp" | "ve-tos mv" => {
            examples.push(format!(
                "{public_command} ... --query 'data.transfers[?status==`failed`].key'"
            ));
            examples.push(format!(
                "{public_command} ... --query 'data.summary.total_bytes'"
            ));
        }
        "ve-tos sync" => {
            examples.push(format!("{public_command} ... --query 'data.summary'"));
            examples.push(format!(
                "{public_command} ... --query 'data.changes[?action==`upload`].key'"
            ));
        }
        "ve-tos rm" | "ve-tos rb" => {
            examples.push(format!("{public_command} ... --dry-run --query 'impact'"));
        }
        "ve-tos ls" => {
            examples.push(format!(
                "{public_command} tos://bucket/prefix --query 'data.objects[*].key'"
            ));
            examples.push(format!(
                "{public_command} tos://bucket --query 'data.common_prefixes[*]'"
            ));
        }
        _ => {}
    }
    // Replace the placeholder COMMAND in the leading two entries.
    examples
        .into_iter()
        .map(|s| s.replace("COMMAND", &public_command))
        .collect()
}

/// [G5] Quoting/escaping tips applicable to all high-level commands. Surfaces
/// shell-portability gotchas that frequently break Agent-generated commands.
fn high_level_quoting_tips() -> Vec<String> {
    vec![
        "Quote object keys that contain spaces or shell metacharacters: ve-tos cp 'tos://bucket/path with space.txt' ./out.txt".to_string(),
        "JMESPath literals inside --query use backticks; in bash, escape them: --query 'Contents[?Size > `1000`]'".to_string(),
        "Use --output json with jq for very large reports rather than --output table.".to_string(),
        "Set TOS_NO_COLOR=1 (or --no-color) when piping output into other tools.".to_string(),
    ]
}

fn dry_run_warnings(
    operation: &HighLevelOperation,
    path_traversal_confirm_target: Option<&str>,
) -> Vec<String> {
    // [Review Fix #2] Dry-run warning must reflect that real High-Level execution now exists.
    let mut warnings =
        vec!["dry-run does not send network requests or mutate local files".to_string()];
    if operation.requires_force {
        warnings.push(
            "real execution requires --force because this command has destructive or cost impact"
                .to_string(),
        );
    }
    if let Some(confirm_target) = path_traversal_confirm_target {
        warnings.push(format!(
            "path traversal risk detected; real execution requires --force and, outside a TTY, --confirm {}",
            confirm_target
        ));
    }
    warnings
}

fn plan_summary(operation: &HighLevelOperation) -> PlanSummary {
    let mut summary = PlanSummary {
        planned: if operation.batch_enabled { 0 } else { 1 },
        to_read: 0,
        to_write: 0,
        to_delete: 0,
        unknown_until_discovery: operation.batch_enabled,
    };
    match operation.command {
        "ve-tos cp" => {
            summary.to_read = 1;
            summary.to_write = 1;
        }
        "ve-tos mv" => {
            summary.to_read = 1;
            summary.to_write = 1;
            summary.to_delete = 1;
        }
        "ve-tos sync" => {
            summary.to_read = 1;
            summary.to_write = 1;
            if operation.requires_force {
                summary.to_delete = 1;
            }
        }
        "ve-tos rm" | "ve-tos rb" => summary.to_delete = 1,
        "ve-tos restore" | "ve-tos put" => summary.to_write = 1,
        "ve-tos ls" | "ve-tos stat" | "ve-tos du" | "ve-tos find" | "ve-tos cat" => {
            summary.to_read = 1
        }
        "ve-tos mb" | "ve-tos mkdir" => summary.to_write = 1,
        "ve-tos presign" => summary.to_read = 1,
        _ => {}
    }
    summary
}

fn plan_filters(operation: &HighLevelOperation) -> BTreeMap<&'static str, String> {
    let mut filters = BTreeMap::new();
    for parameter in &operation.parameters {
        if matches!(
            parameter.name.as_str(),
            "include" | "exclude" | "name" | "size" | "mtime" | "storage-class"
        ) {
            filters.insert(
                "declared",
                "see command parameters; applied during discover/plan".to_string(),
            );
            break;
        }
    }
    filters
}

fn request_plan_steps(operation: &HighLevelOperation) -> Vec<RequestPlanStep> {
    let mut steps = Vec::new();
    for api in &operation.low_level_apis {
        steps.push(RequestPlanStep {
            phase: if api.contains("List") || api.contains("Head") {
                "discover"
            } else {
                "execute"
            },
            api,
            mutates: !(api.contains("List") || api.contains("Head") || *api == "SignV4"),
            requires_force: operation.requires_force
                && !(api.contains("List") || api.contains("Head")),
        });
    }
    steps
}

fn plan_samples(operation: &HighLevelOperation) -> Vec<PlanSample> {
    vec![PlanSample {
        operation: public_high_level_command(operation.command),
        source: operation
            .source
            .clone()
            .unwrap_or_else(|| operation.target.clone()),
        destination: operation.destination.clone(),
    }]
}

fn effective_report_path(
    global: &GlobalArgs,
    explicit: Option<&str>,
    command: &str,
) -> Result<Option<String>, CliError> {
    // [Review Fix #ReportPath-Tilde] 显式指定路径也要展开 ~/，并且统一容忍尾部斜杠
    // / 字面 ~ 等用户书写习惯。否则 `--report-path ~/reports/x.csv` 会写到字面 `~`
    // 子目录下。同时默认路径必须经过 expand_user_path（与 checkpoint 路径对齐），
    // 之前直接拼 raw `~/.tos/reports` 会把报告写进 CWD，不可被 home 用户找到。
    if let Some(path) = explicit {
        return Ok(Some(expand_user_path(path).to_string_lossy().into_owned()));
    }
    let profile = build_profile(global)?;
    let report_dir_raw = profile
        .batch_report_dir
        .unwrap_or_else(|| DEFAULT_TOS_BATCH_REPORT_DIR.to_string());
    let report_format = profile
        .batch_report_format
        .unwrap_or_else(|| DEFAULT_TOS_BATCH_REPORT_FORMAT.to_string());
    if report_format != "csv" {
        return Err(CliError::ValidationError(format!(
            "unsupported batch_report_format '{}': only csv is supported",
            report_format
        )));
    }
    let scoped_report_dir = scoped_default_path(
        report_dir_raw.trim_end_matches('/'),
        DEFAULT_TOS_BATCH_REPORT_DIR,
    );
    let report_dir = writable_default_report_dir(&scoped_report_dir)?;
    let file_name = format!(
        "{}-{}-{}.csv",
        report_name(command),
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    );
    Ok(Some(
        report_dir.join(file_name).to_string_lossy().into_owned(),
    ))
}

fn effective_manifest_path(
    global: &GlobalArgs,
    explicit: Option<&str>,
    command: &str,
) -> Result<Option<String>, CliError> {
    if let Some(path) = explicit {
        return Ok(Some(expand_user_path(path).to_string_lossy().into_owned()));
    }
    let profile = build_profile(global)?;
    let report_dir_raw = profile
        .batch_report_dir
        .unwrap_or_else(|| DEFAULT_TOS_BATCH_REPORT_DIR.to_string());
    let scoped_report_dir = scoped_default_path(
        report_dir_raw.trim_end_matches('/'),
        DEFAULT_TOS_BATCH_REPORT_DIR,
    );
    let report_dir = writable_default_report_dir(&scoped_report_dir)?;
    let file_name = format!(
        "{}-manifest-{}-{}.csv",
        report_name(command),
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    );
    Ok(Some(
        report_dir.join(file_name).to_string_lossy().into_owned(),
    ))
}

fn effective_optional_manifest_path(
    global: &GlobalArgs,
    explicit: Option<&str>,
    no_manifest: bool,
    command: &str,
) -> Result<Option<String>, CliError> {
    if no_manifest {
        return Ok(None);
    }
    effective_manifest_path(global, explicit, command)
}

fn top_level_storage_surface() -> &'static str {
    match active_tos_config_binary() {
        tos_core::infra::config::Binary::Tos => "tos",
        tos_core::infra::config::Binary::VeTos => "ve-tos",
        _ => "tos",
    }
}

fn scoped_default_path(raw_path: &str, default_path: &str) -> String {
    let trimmed = raw_path.trim_end_matches('/');
    if trimmed == default_path {
        format!("{}/{}", default_path, top_level_storage_surface())
    } else {
        trimmed.to_string()
    }
}

fn effective_explicit_manifest_path(explicit: Option<&str>) -> Option<String> {
    explicit.map(|path| expand_user_path(path).to_string_lossy().into_owned())
}

fn reject_single_transfer_artifacts(
    command: &str,
    report_path: Option<&str>,
    report_failures_only: bool,
    manifest_path: Option<&str>,
    no_manifest: bool,
    batch_concurrency: Option<usize>,
    list_concurrency: Option<usize>,
) -> Result<(), CliError> {
    let has_batch_artifact = report_path.is_some()
        || report_failures_only
        || manifest_path.is_some()
        || no_manifest
        || batch_concurrency.is_some()
        || list_concurrency.is_some();
    if has_batch_artifact {
        return Err(CliError::ValidationError(format!(
            "{}: --report-path/--report-failures-only/--manifest-path/--no-manifest/--batch-concurrency/--list-concurrency are only valid for batch mode",
            command
        )));
    }
    Ok(())
}

fn success_report_path<'a>(report_path: Option<&'a str>, failures_only: bool) -> Option<&'a str> {
    if failures_only {
        None
    } else {
        report_path
    }
}

fn writable_default_report_dir(raw: &str) -> Result<PathBuf, CliError> {
    let primary = expand_user_path(raw);
    match ensure_directory_writable(&primary) {
        Ok(()) => return Ok(primary),
        Err(err) if err.kind() != std::io::ErrorKind::PermissionDenied => {
            return Err(CliError::Io(err));
        }
        Err(_) => {}
    }
    let fallback_root = if Path::new("/private/tmp").exists() {
        PathBuf::from("/private/tmp")
    } else {
        std::env::temp_dir()
    };
    let fallback = fallback_root.join("ve-storage-uni-cli").join("reports");
    ensure_directory_writable(&fallback)?;
    Ok(fallback)
}

fn ensure_directory_writable(dir: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".tos-write-probe-{}", std::process::id()));
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe)?;
    let _ = fs::remove_file(probe);
    Ok(())
}

fn stderr_is_tty() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stderr())
}

fn resolve_list_echo_plan(
    global: &GlobalArgs,
    list_echo: bool,
    no_list_echo: bool,
) -> ProgressPlan {
    let (enabled, disabled_reason) = if global.quiet {
        (false, Some("quiet"))
    } else if no_list_echo {
        (false, Some("no_list_echo"))
    } else if list_echo {
        (true, None)
    } else if !stderr_is_tty() {
        (false, Some("non_tty"))
    } else {
        (true, None)
    };
    ProgressPlan {
        enabled,
        render_to: "stderr",
        disabled_reason,
    }
}

fn resolve_progress_plan(
    global: &GlobalArgs,
    config_progress: bool,
    progress: bool,
    no_progress: bool,
) -> ProgressPlan {
    let (enabled, disabled_reason) = if global.quiet {
        (false, Some("quiet"))
    } else if no_progress {
        (false, Some("no_progress"))
    } else if progress {
        (true, None)
    } else if !config_progress {
        (false, Some("config"))
    } else if !stderr_is_tty() {
        (false, Some("non_tty"))
    } else {
        (true, None)
    };
    ProgressPlan {
        enabled,
        render_to: "stderr",
        disabled_reason,
    }
}

fn effective_list_echo_enabled(global: &GlobalArgs, list_echo: bool, no_list_echo: bool) -> bool {
    resolve_list_echo_plan(global, list_echo, no_list_echo).enabled
}

fn effective_traversal_echo_enabled(
    global: &GlobalArgs,
    list_echo: bool,
    no_list_echo: bool,
    progress: bool,
    no_progress: bool,
) -> bool {
    if list_echo || no_list_echo {
        effective_list_echo_enabled(global, list_echo, no_list_echo)
    } else {
        effective_list_echo_enabled(global, progress, no_progress)
    }
}

fn effective_progress_enabled(
    global: &GlobalArgs,
    progress: bool,
    no_progress: bool,
) -> Result<bool, CliError> {
    let profile = build_profile(global)?;
    Ok(resolve_progress_plan(
        global,
        profile
            .progress_enabled
            .unwrap_or(DEFAULT_TOS_PROGRESS_ENABLED),
        progress,
        no_progress,
    )
    .enabled)
}

fn effective_cp_runtime_config(
    global: &GlobalArgs,
    args: &CpArgs,
) -> Result<TransferRuntimeConfig, CliError> {
    let profile = build_profile(global)?;
    Ok(TransferRuntimeConfig {
        checkpoint_threshold: effective_size_value(
            args.checkpoint_threshold.as_deref(),
            profile.checkpoint_threshold.as_deref(),
            DEFAULT_TRANSFER_CHECKPOINT_THRESHOLD,
            "checkpoint_threshold",
        )?,
        batch_concurrency: positive_or_config(
            args.batch_concurrency,
            profile.batch_concurrency,
            DEFAULT_BATCH_CONCURRENCY,
            "batch_concurrency",
        )?,
        list_concurrency: positive_or_config(
            args.list_concurrency,
            profile.list_concurrency,
            DEFAULT_LIST_CONCURRENCY,
            "list_concurrency",
        )?,
        multipart_concurrency: positive_or_config(
            args.multipart_concurrency,
            profile.multipart_concurrency,
            DEFAULT_MULTIPART_CONCURRENCY,
            "multipart_concurrency",
        )?,
        progress_granularity: effective_progress_granularity(
            args.progress_granularity,
            profile.progress_granularity.as_deref(),
        )?,
        overwrite_strategy: effective_overwrite_strategy(
            args.overwrite_strategy,
            args.force,
            args.no_clobber,
            profile.overwrite_strategy.as_deref(),
        )?,
    })
}

fn effective_sync_runtime_config(
    global: &GlobalArgs,
    args: &SyncArgs,
) -> Result<TransferRuntimeConfig, CliError> {
    let profile = build_profile(global)?;
    Ok(TransferRuntimeConfig {
        checkpoint_threshold: effective_size_value(
            args.checkpoint_threshold.as_deref(),
            profile.checkpoint_threshold.as_deref(),
            DEFAULT_TRANSFER_CHECKPOINT_THRESHOLD,
            "checkpoint_threshold",
        )?,
        batch_concurrency: positive_or_config(
            args.batch_concurrency,
            profile.batch_concurrency,
            DEFAULT_BATCH_CONCURRENCY,
            "batch_concurrency",
        )?,
        list_concurrency: positive_or_config(
            args.list_concurrency,
            profile.list_concurrency,
            DEFAULT_LIST_CONCURRENCY,
            "list_concurrency",
        )?,
        multipart_concurrency: positive_or_config(
            args.multipart_concurrency,
            profile.multipart_concurrency,
            DEFAULT_MULTIPART_CONCURRENCY,
            "multipart_concurrency",
        )?,
        progress_granularity: effective_progress_granularity(
            args.progress_granularity,
            profile.progress_granularity.as_deref(),
        )?,
        overwrite_strategy: effective_overwrite_strategy(
            args.overwrite_strategy,
            args.force,
            false,
            profile.overwrite_strategy.as_deref(),
        )?,
    })
}

fn effective_default_runtime_config(
    global: &GlobalArgs,
) -> Result<TransferRuntimeConfig, CliError> {
    let profile = build_profile(global)?;
    Ok(TransferRuntimeConfig {
        checkpoint_threshold: effective_size_value(
            None,
            profile.checkpoint_threshold.as_deref(),
            DEFAULT_TRANSFER_CHECKPOINT_THRESHOLD,
            "checkpoint_threshold",
        )?,
        batch_concurrency: positive_or_config(
            None,
            profile.batch_concurrency,
            DEFAULT_BATCH_CONCURRENCY,
            "batch_concurrency",
        )?,
        list_concurrency: positive_or_config(
            None,
            profile.list_concurrency,
            DEFAULT_LIST_CONCURRENCY,
            "list_concurrency",
        )?,
        multipart_concurrency: positive_or_config(
            None,
            profile.multipart_concurrency,
            DEFAULT_MULTIPART_CONCURRENCY,
            "multipart_concurrency",
        )?,
        progress_granularity: effective_progress_granularity(
            None,
            profile.progress_granularity.as_deref(),
        )?,
        overwrite_strategy: effective_overwrite_strategy(
            None,
            false,
            false,
            profile.overwrite_strategy.as_deref(),
        )?,
    })
}

fn effective_batch_concurrency(
    global: &GlobalArgs,
    cli_value: Option<usize>,
) -> Result<usize, CliError> {
    let profile = build_profile(global)?;
    positive_or_config(
        cli_value,
        profile.batch_concurrency,
        DEFAULT_BATCH_CONCURRENCY,
        "batch_concurrency",
    )
}

fn effective_list_concurrency(
    global: &GlobalArgs,
    cli_value: Option<usize>,
) -> Result<usize, CliError> {
    let profile = build_profile(global)?;
    positive_or_config(
        cli_value,
        profile.list_concurrency,
        DEFAULT_LIST_CONCURRENCY,
        "list_concurrency",
    )
}

fn resolve_tos_recursive_list_mode(is_hns_bucket: bool, mode: Option<RecursiveListMode>) -> bool {
    if std::env::var("VE_STORAGE_UNI_TOS_FORCE_HIERARCHICAL_LISTING")
        .ok()
        .as_deref()
        == Some("1")
    {
        // [Review Fix #1] The new ByteCloud `tos-cli` surface only supports
        // delimiter="/" recursive listing, matching the internal TOS SDK's
        // folder semantics without relying on HNS/FNS auto-detection.
        return true;
    }
    resolve_tos_recursive_list_mode_for_binary(active_tos_config_binary(), is_hns_bucket, mode)
}

fn resolve_tos_recursive_list_mode_for_binary(
    binary: Binary,
    is_hns_bucket: bool,
    mode: Option<RecursiveListMode>,
) -> bool {
    if binary == Binary::Tos {
        // [Review Fix #3] ByteTOS list recursion must carry delimiter="/";
        // child prefixes are traversed by the hierarchical scanner.
        return true;
    }
    match mode.unwrap_or(RecursiveListMode::Auto) {
        RecursiveListMode::Auto => is_hns_bucket,
        RecursiveListMode::Flat => false,
        RecursiveListMode::Hierarchical => true,
    }
}

fn resolve_tos_du_list_mode(is_hns_bucket: bool) -> bool {
    if std::env::var("VE_STORAGE_UNI_TOS_FORCE_HIERARCHICAL_LISTING")
        .ok()
        .as_deref()
        == Some("1")
    {
        return true;
    }
    resolve_tos_du_list_mode_for_binary(active_tos_config_binary(), is_hns_bucket)
}

fn resolve_tos_du_list_mode_for_binary(binary: Binary, is_hns_bucket: bool) -> bool {
    if matches!(binary, Binary::Tos | Binary::VeTos) {
        // [Review Fix #5] `du` uses delimiter="/" for both ByteTOS and ve-tos,
        // regardless of bucket shape; other recursive commands keep their own
        // HNS/FNS list-mode selection.
        return true;
    }
    is_hns_bucket
}

fn is_tos_directory_marker_source(source: &str) -> Result<bool, CliError> {
    if !source.starts_with("tos://") {
        return Ok(false);
    }
    let source_target = parse_tos_uri(source, false)?;
    Ok(source_target
        .key
        .as_deref()
        .is_some_and(|key| key.ends_with('/')))
}

fn create_local_directory_marker_destination(destination_path: &Path) -> Result<(), CliError> {
    if destination_path.exists() {
        if destination_path.is_dir() {
            return Ok(());
        }
        return Err(CliError::Conflict(format!(
            "local destination '{}' exists and is not a directory",
            destination_path.display()
        )));
    }
    // [Review Fix #TOS-CpSlashSource] A trailing-slash TOS key represents a
    // directory marker for high-level local downloads. Creating the local
    // directory avoids issuing HEAD/GET for a value that cannot become a file.
    fs::create_dir_all(destination_path)?;
    Ok(())
}

fn effective_size_value(
    cli_value: Option<&str>,
    config_value: Option<&str>,
    default_value: &str,
    field: &str,
) -> Result<u64, CliError> {
    let value = cli_value.or(config_value).unwrap_or(default_value);
    parse_size_bytes(value).ok_or_else(|| {
        CliError::ValidationError(format!(
            "invalid {} '{}': expected <number>[B|KB|MB|GB|TB]",
            field, value
        ))
    })
}

fn positive_or_config(
    cli_value: Option<usize>,
    config_value: Option<usize>,
    default_value: usize,
    field: &str,
) -> Result<usize, CliError> {
    let value = cli_value.or(config_value).unwrap_or(default_value);
    if value == 0 {
        return Err(CliError::ValidationError(format!(
            "{} must be a positive integer",
            field
        )));
    }
    Ok(value)
}

fn effective_progress_granularity(
    cli_value: Option<ProgressGranularity>,
    config_value: Option<&str>,
) -> Result<EffectiveProgressGranularity, CliError> {
    if let Some(value) = cli_value {
        return Ok(match value {
            ProgressGranularity::Part => EffectiveProgressGranularity::Part,
            ProgressGranularity::Byte => EffectiveProgressGranularity::Byte,
        });
    }
    match config_value
        .unwrap_or(DEFAULT_PROGRESS_GRANULARITY)
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "part" | "parts" => Ok(EffectiveProgressGranularity::Part),
        "byte" | "bytes" => Ok(EffectiveProgressGranularity::Byte),
        value => Err(CliError::ValidationError(format!(
            "invalid progress_granularity '{}': expected part or byte",
            value
        ))),
    }
}

fn effective_overwrite_strategy(
    cli_value: Option<OverwriteStrategy>,
    legacy_force: bool,
    legacy_no_clobber: bool,
    config_value: Option<&str>,
) -> Result<EffectiveOverwriteStrategy, CliError> {
    if legacy_force && legacy_no_clobber {
        return Err(CliError::ValidationError(
            "--force and --no-clobber cannot be used together".to_string(),
        ));
    }
    if let Some(value) = cli_value {
        return Ok(match value {
            OverwriteStrategy::Force => EffectiveOverwriteStrategy::Force,
            OverwriteStrategy::NoClobber => EffectiveOverwriteStrategy::NoClobber,
            OverwriteStrategy::Newer => EffectiveOverwriteStrategy::Newer,
        });
    }
    if legacy_no_clobber {
        return Ok(EffectiveOverwriteStrategy::NoClobber);
    }
    if legacy_force {
        return Ok(EffectiveOverwriteStrategy::Force);
    }
    match config_value
        .unwrap_or(DEFAULT_OVERWRITE_STRATEGY)
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "force" | "overwrite" => Ok(EffectiveOverwriteStrategy::Force),
        "no-clobber" | "no_clobber" | "skip-existing" | "skip_existing" => {
            Ok(EffectiveOverwriteStrategy::NoClobber)
        }
        "newer" | "if-newer" | "if_newer" => Ok(EffectiveOverwriteStrategy::Newer),
        value => Err(CliError::ValidationError(format!(
            "invalid overwrite_strategy '{}': expected force, no-clobber, or newer",
            value
        ))),
    }
}

fn transfer_parameters(include_checkpoint_flag: bool) -> Vec<CommandParameter> {
    let mut params = vec![
        param(
            "source",
            ParameterLocation::Path,
            true,
            "Local path or tos://bucket/key",
        ),
        param(
            "destination",
            ParameterLocation::Path,
            true,
            "Local path or tos://bucket/key",
        ),
        param(
            "recursive",
            ParameterLocation::Flag,
            false,
            "Enable recursive batch operation",
        ),
        param(
            "include-parent",
            ParameterLocation::Flag,
            false,
            "Include the source directory/prefix name under the destination prefix",
        ),
        param("include", ParameterLocation::Flag, false, "Include pattern"),
        param("exclude", ParameterLocation::Flag, false, "Exclude pattern"),
        param(
            "checkpoint-dir",
            ParameterLocation::Flag,
            false,
            "Checkpoint directory override",
        ),
        param(
            "batch-concurrency",
            ParameterLocation::Flag,
            false,
            "Maximum files/items running concurrently in batch execution",
        ),
        param(
            "list-concurrency",
            ParameterLocation::Flag,
            false,
            "Maximum prefixes listed concurrently when recursive listing uses delimiter=\"/\"",
        ),
        param(
            "recursive-list-mode",
            ParameterLocation::Flag,
            false,
            "Recursive listing mode: tos always uses hierarchical delimiter=\"/\"; ve-tos auto uses bucket shape, with flat/hierarchical overrides",
        ),
        param(
            "storage-class",
            ParameterLocation::Header,
            false,
            "Storage class for ve-tos uploads and TOS-to-TOS copies; ByteTOS uploads reject this override. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE",
        ),
        param(
            "acl",
            ParameterLocation::Header,
            false,
            "Target object ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default",
        ),
        param(
            "meta",
            ParameterLocation::Header,
            false,
            "Custom metadata as key=value#key2=value2; writes x-tos-meta-* headers",
        ),
        param(
            "report-path",
            ParameterLocation::Flag,
            false,
            "Batch report path",
        ),
        param(
            "report-failures-only",
            ParameterLocation::Flag,
            false,
            "Write only failed items to the batch report",
        ),
        param(
            "manifest-path",
            ParameterLocation::Flag,
            false,
            "Planned transfer manifest path",
        ),
        param(
            "no-manifest",
            ParameterLocation::Flag,
            false,
            "Disable planned manifest output",
        ),
        param(
            "list-echo",
            ParameterLocation::Flag,
            false,
            "Enable listing-phase echo output",
        ),
        param(
            "no-list-echo",
            ParameterLocation::Flag,
            false,
            "Disable listing-phase echo output",
        ),
        param(
            "progress",
            ParameterLocation::Flag,
            false,
            "Enable execution progress output",
        ),
        param(
            "no-progress",
            ParameterLocation::Flag,
            false,
            "Disable execution progress output",
        ),
        param(
            "force",
            ParameterLocation::Flag,
            false,
            "Confirm overwrite or destructive side effects",
        ),
    ];
    if include_checkpoint_flag {
        params.push(param(
            "checkpoint",
            ParameterLocation::Flag,
            false,
            "Enable resumable transfer checkpoint",
        ));
    }
    params
}

fn sync_parameters() -> Vec<CommandParameter> {
    vec![
        param(
            "source",
            ParameterLocation::Path,
            true,
            "Local path or tos://bucket/prefix",
        ),
        param(
            "destination",
            ParameterLocation::Path,
            true,
            "Local path or tos://bucket/prefix",
        ),
        param(
            "delete",
            ParameterLocation::Flag,
            false,
            "Delete extra destination entries",
        ),
        param(
            "force",
            ParameterLocation::Flag,
            false,
            "Required with --delete",
        ),
        param(
            "size-only",
            ParameterLocation::Flag,
            false,
            "Compare size only",
        ),
        param(
            "exact-timestamps",
            ParameterLocation::Flag,
            false,
            "Require exact timestamp match",
        ),
        param(
            "include-parent",
            ParameterLocation::Flag,
            false,
            "Include the source directory/prefix name under the destination prefix",
        ),
        param("include", ParameterLocation::Flag, false, "Include pattern"),
        param("exclude", ParameterLocation::Flag, false, "Exclude pattern"),
        param(
            "checkpoint-dir",
            ParameterLocation::Flag,
            false,
            "Checkpoint directory override",
        ),
        param(
            "batch-concurrency",
            ParameterLocation::Flag,
            false,
            "Maximum files/items running concurrently in batch execution",
        ),
        param(
            "list-concurrency",
            ParameterLocation::Flag,
            false,
            "Maximum prefixes listed concurrently when recursive listing uses delimiter=\"/\"",
        ),
        param(
            "recursive-list-mode",
            ParameterLocation::Flag,
            false,
            "Recursive listing mode: tos always uses hierarchical delimiter=\"/\"; ve-tos auto uses bucket shape, with flat/hierarchical overrides",
        ),
        param(
            "storage-class",
            ParameterLocation::Header,
            false,
            "Storage class for ve-tos uploads and TOS-to-TOS copies; ByteTOS uploads reject this override. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE",
        ),
        param(
            "acl",
            ParameterLocation::Header,
            false,
            "Target object ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default",
        ),
        param(
            "meta",
            ParameterLocation::Header,
            false,
            "Custom metadata as key=value#key2=value2; writes x-tos-meta-* headers",
        ),
        param(
            "bandwidth-limit",
            ParameterLocation::Flag,
            false,
            "Bandwidth limit",
        ),
        param(
            "report-path",
            ParameterLocation::Flag,
            false,
            "Batch report path",
        ),
        param(
            "report-failures-only",
            ParameterLocation::Flag,
            false,
            "Write only failed items to the batch report",
        ),
        param(
            "manifest-path",
            ParameterLocation::Flag,
            false,
            "Planned transfer manifest path",
        ),
        param(
            "no-manifest",
            ParameterLocation::Flag,
            false,
            "Disable planned manifest output",
        ),
        param(
            "list-echo",
            ParameterLocation::Flag,
            false,
            "Enable listing-phase echo output",
        ),
        param(
            "no-list-echo",
            ParameterLocation::Flag,
            false,
            "Disable listing-phase echo output",
        ),
        param(
            "progress",
            ParameterLocation::Flag,
            false,
            "Enable execution progress output",
        ),
        param(
            "no-progress",
            ParameterLocation::Flag,
            false,
            "Disable execution progress output",
        ),
    ]
}

fn mb_parameters() -> Vec<CommandParameter> {
    vec![
        param(
            "bucket",
            ParameterLocation::Path,
            true,
            "Bucket name or tos://bucket",
        ),
        param(
            "region",
            ParameterLocation::Flag,
            false,
            "Region override for this request",
        ),
        param(
            "storage-class",
            ParameterLocation::Flag,
            false,
            "Bucket storage class. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE",
        ),
        param(
            "acl",
            ParameterLocation::Flag,
            false,
            "Bucket ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control",
        ),
        param(
            "az-redundancy",
            ParameterLocation::Flag,
            false,
            "AZ redundancy mode. Allowed: single-az, multi-az",
        ),
        param(
            "bucket-type",
            ParameterLocation::Flag,
            false,
            "Bucket type. Allowed: fns, hns",
        ),
        param(
            "bucket-object-lock-enabled",
            ParameterLocation::Flag,
            false,
            "Enable bucket object lock",
        ),
    ]
}

fn rm_parameters() -> Vec<CommandParameter> {
    vec![
        param(
            "path",
            ParameterLocation::Path,
            true,
            "tos://bucket/key or tos://bucket/prefix",
        ),
        param(
            "recursive",
            ParameterLocation::Flag,
            false,
            "Enable recursive batch operation",
        ),
        // [Review Fix #1] This mode is rm-only; transfer commands keep the
        // generic recursive parameter without delete-strategy semantics.
        param(
            "recursive-delete-mode",
            ParameterLocation::Flag,
            false,
            "HNS-only recursive delete strategy: bottom-up or direct",
        ),
        param(
            "force",
            ParameterLocation::Flag,
            false,
            "Required for destructive execution",
        ),
        param(
            "batch-concurrency",
            ParameterLocation::Flag,
            false,
            "Maximum files/items running concurrently in batch execution",
        ),
        param(
            "list-concurrency",
            ParameterLocation::Flag,
            false,
            "Maximum prefixes listed concurrently when recursive listing uses delimiter=\"/\"",
        ),
        param(
            "recursive-list-mode",
            ParameterLocation::Flag,
            false,
            "Recursive listing mode: tos always uses hierarchical delimiter=\"/\"; ve-tos auto uses bucket shape, with flat/hierarchical overrides",
        ),
        param(
            "report-path",
            ParameterLocation::Flag,
            false,
            "Batch report path",
        ),
        param(
            "report-failures-only",
            ParameterLocation::Flag,
            false,
            "Write only failed items to the batch report",
        ),
        param(
            "manifest-path",
            ParameterLocation::Flag,
            false,
            "Planned delete manifest path",
        ),
        param(
            "no-manifest",
            ParameterLocation::Flag,
            false,
            "Disable planned manifest output",
        ),
        param("include", ParameterLocation::Flag, false, "Include pattern"),
        param("exclude", ParameterLocation::Flag, false, "Exclude pattern"),
        param(
            "list-echo",
            ParameterLocation::Flag,
            false,
            "Enable listing-phase echo output",
        ),
        param(
            "no-list-echo",
            ParameterLocation::Flag,
            false,
            "Disable listing-phase echo output",
        ),
        param(
            "progress",
            ParameterLocation::Flag,
            false,
            "Enable execution progress output",
        ),
        param(
            "no-progress",
            ParameterLocation::Flag,
            false,
            "Disable execution progress output",
        ),
    ]
}

fn du_parameters() -> Vec<CommandParameter> {
    vec![
        param(
            "path",
            ParameterLocation::Path,
            true,
            "tos://bucket or tos://bucket/prefix",
        ),
        param(
            "human-readable",
            ParameterLocation::Flag,
            false,
            "Render human-readable sizes",
        ),
        param(
            "max-depth",
            ParameterLocation::Flag,
            false,
            "Maximum directory depth",
        ),
        param(
            "top-k",
            ParameterLocation::Flag,
            false,
            "Number of largest and oldest object samples to keep",
        ),
        param(
            "cost",
            ParameterLocation::Flag,
            false,
            "Include estimated monthly storage cost by storage class",
        ),
        param(
            "storage-price",
            ParameterLocation::Flag,
            false,
            "Override storage price as CLASS=PRICE in CNY/GB/month",
        ),
        param(
            "manifest-path",
            ParameterLocation::Flag,
            false,
            "Optional traversed object manifest path",
        ),
        param(
            "list-concurrency",
            ParameterLocation::Flag,
            false,
            "Maximum prefixes listed concurrently when the bucket is listed hierarchically",
        ),
        param(
            "list-echo",
            ParameterLocation::Flag,
            false,
            "Enable traversal echo output",
        ),
        param(
            "no-list-echo",
            ParameterLocation::Flag,
            false,
            "Disable traversal echo output",
        ),
        param(
            "progress",
            ParameterLocation::Flag,
            false,
            "Legacy alias to enable traversal echo when list echo flags are absent",
        ),
        param(
            "no-progress",
            ParameterLocation::Flag,
            false,
            "Legacy alias to disable traversal echo when list echo flags are absent",
        ),
    ]
}

fn ls_parameters() -> Vec<CommandParameter> {
    vec![
        param(
            "path",
            ParameterLocation::Path,
            false,
            "tos://bucket or tos://bucket/prefix",
        ),
        param(
            "max-keys",
            ParameterLocation::Query,
            false,
            "Maximum buckets, objects, or prefixes to return from the current level",
        ),
        param(
            "continuation-token",
            ParameterLocation::Query,
            false,
            "Continuation token returned by a previous listing",
        ),
        param(
            "human-readable",
            ParameterLocation::Flag,
            false,
            "Render human-readable sizes",
        ),
        param("sort", ParameterLocation::Flag, false, "Sort field"),
        param(
            "columns",
            ParameterLocation::Flag,
            false,
            "Comma-separated table/csv columns",
        ),
        param(
            "manifest-path",
            ParameterLocation::Flag,
            false,
            "Optional listing manifest path",
        ),
    ]
}

fn find_parameters() -> Vec<CommandParameter> {
    vec![
        param("path", ParameterLocation::Path, true, "tos://bucket/prefix"),
        param(
            "name",
            ParameterLocation::Flag,
            false,
            "Name glob or substring",
        ),
        param(
            "size",
            ParameterLocation::Flag,
            false,
            "Size predicate, e.g. +1GB",
        ),
        param(
            "mtime",
            ParameterLocation::Flag,
            false,
            "Modified time predicate",
        ),
        param(
            "storage-class",
            ParameterLocation::Flag,
            false,
            "Storage class filter",
        ),
        param(
            "manifest-path",
            ParameterLocation::Flag,
            false,
            "Optional matched object manifest path",
        ),
        param(
            "list-echo",
            ParameterLocation::Flag,
            false,
            "Enable traversal echo output",
        ),
        param(
            "no-list-echo",
            ParameterLocation::Flag,
            false,
            "Disable traversal echo output",
        ),
        param(
            "progress",
            ParameterLocation::Flag,
            false,
            "Legacy alias to enable traversal echo when list echo flags are absent",
        ),
        param(
            "no-progress",
            ParameterLocation::Flag,
            false,
            "Legacy alias to disable traversal echo when list echo flags are absent",
        ),
    ]
}

fn put_parameters() -> Vec<CommandParameter> {
    vec![
        param("path", ParameterLocation::Path, true, "tos://bucket/key"),
        param(
            "content-type",
            ParameterLocation::Header,
            false,
            "Content-Type for uploaded object",
        ),
        param(
            "storage-class",
            ParameterLocation::Header,
            false,
            "Storage class for ve-tos stdin uploads; ByteTOS tos put rejects this override. Allowed: STANDARD, IA, ARCHIVE_FR, INTELLIGENT_TIERING, COLD_ARCHIVE, ARCHIVE, DEEP_COLD_ARCHIVE",
        ),
        param(
            "acl",
            ParameterLocation::Header,
            false,
            "Target object ACL. Allowed: private, public-read, public-read-write, authenticated-read, bucket-owner-read, bucket-owner-full-control, bucket-owner-entrusted, default",
        ),
        param(
            "meta",
            ParameterLocation::Header,
            false,
            "Custom metadata as key=value#key2=value2; writes x-tos-meta-* headers",
        ),
        param(
            "multipart-threshold",
            ParameterLocation::Flag,
            false,
            "Stdin size threshold for multipart upload; data is uploaded after stdin EOF",
        ),
        param(
            "no-clobber",
            ParameterLocation::Flag,
            false,
            "Do not overwrite an existing object",
        ),
        param(
            "progress",
            ParameterLocation::Flag,
            false,
            "Enable execution progress output",
        ),
        param(
            "no-progress",
            ParameterLocation::Flag,
            false,
            "Disable execution progress output",
        ),
    ]
}

fn restore_parameters() -> Vec<CommandParameter> {
    vec![
        param(
            "path",
            ParameterLocation::Path,
            true,
            "tos://bucket/key or prefix",
        ),
        param(
            "recursive",
            ParameterLocation::Flag,
            false,
            "Restore prefix recursively",
        ),
        param(
            "manifest",
            ParameterLocation::Flag,
            false,
            "Manifest file path",
        ),
        param("include", ParameterLocation::Flag, false, "Include pattern"),
        param("exclude", ParameterLocation::Flag, false, "Exclude pattern"),
        param("days", ParameterLocation::Body, false, "Restore days"),
        param("tier", ParameterLocation::Body, false, "Restore tier"),
        param(
            "version-id",
            ParameterLocation::Query,
            false,
            "Object version ID",
        ),
        param(
            "report-path",
            ParameterLocation::Flag,
            false,
            "Batch report path",
        ),
        param(
            "report-failures-only",
            ParameterLocation::Flag,
            false,
            "Write only failed items to the batch report",
        ),
        param(
            "manifest-path",
            ParameterLocation::Flag,
            false,
            "Planned restore manifest path",
        ),
        param(
            "no-manifest",
            ParameterLocation::Flag,
            false,
            "Disable planned restore manifest output",
        ),
        param(
            "batch-concurrency",
            ParameterLocation::Flag,
            false,
            "Maximum files/items running concurrently in batch restore execution",
        ),
        param(
            "list-concurrency",
            ParameterLocation::Flag,
            false,
            "Maximum prefixes listed concurrently when recursive listing uses delimiter=\"/\"",
        ),
        param(
            "recursive-list-mode",
            ParameterLocation::Flag,
            false,
            "Recursive listing mode: tos always uses hierarchical delimiter=\"/\"; ve-tos auto uses bucket shape, with flat/hierarchical overrides",
        ),
        param(
            "force",
            ParameterLocation::Flag,
            false,
            "Required for recursive/manifest restore",
        ),
        param(
            "list-echo",
            ParameterLocation::Flag,
            false,
            "Enable listing-phase echo output",
        ),
        param(
            "no-list-echo",
            ParameterLocation::Flag,
            false,
            "Disable listing-phase echo output",
        ),
        param(
            "progress",
            ParameterLocation::Flag,
            false,
            "Enable execution progress output",
        ),
        param(
            "no-progress",
            ParameterLocation::Flag,
            false,
            "Disable execution progress output",
        ),
    ]
}

fn param(
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

fn report_name(command: &str) -> String {
    command.replace("ve-tos ", "").replace(' ', "-")
}

struct ParsedTosUri {
    bucket: String,
    key: Option<String>,
}

fn parse_bucket_target(input: &str) -> Result<String, CliError> {
    let parsed = parse_tos_uri(input, true)?;
    if parsed.key.is_some() {
        return Err(CliError::ValidationError(
            "bucket target must be tos://bucket; pass object/prefix parameters separately"
                .to_string(),
        ));
    }
    Ok(parsed.bucket)
}

fn parse_tos_uri_or_bucket(input: &str) -> Result<ParsedTosUri, CliError> {
    if input.starts_with("tos://") {
        return parse_tos_uri(input, true);
    }
    if input.trim().is_empty() || input.contains('/') {
        return Err(CliError::ValidationError(
            "bucket target must be a bucket name or tos://bucket".to_string(),
        ));
    }
    Ok(ParsedTosUri {
        bucket: input.to_string(),
        key: None,
    })
}

fn parse_tos_uri(uri: &str, allow_bucket_only: bool) -> Result<ParsedTosUri, CliError> {
    if !uri.starts_with("tos://") {
        return Err(CliError::ValidationError(format!(
            "invalid TOS URI '{}': expected tos://bucket/key",
            uri
        )));
    }
    let rest = uri.trim_start_matches("tos://");
    let mut parts = rest.splitn(2, '/');
    let bucket = parts.next().unwrap_or_default();
    let key = parts.next().filter(|value| !value.is_empty());
    if bucket.is_empty() || (!allow_bucket_only && key.is_none()) {
        return Err(CliError::ValidationError(format!(
            "invalid TOS URI '{}': expected {}",
            uri,
            if allow_bucket_only {
                "tos://bucket or tos://bucket/key"
            } else {
                "tos://bucket/key"
            }
        )));
    }
    Ok(ParsedTosUri {
        bucket: bucket.to_string(),
        key: key.map(ToString::to_string),
    })
}

fn resolve_single_transfer_destination(
    source: &str,
    destination: &str,
) -> Result<String, CliError> {
    if !destination.starts_with("tos://") {
        if local_destination_uses_directory_semantics(destination) {
            let file_name = source_file_name_for_tos_transfer(source)?;
            return Ok(Path::new(destination)
                .join(file_name)
                .to_string_lossy()
                .into_owned());
        }
        return Ok(destination.to_string());
    }

    let target = parse_tos_uri(destination, true)?;
    if target.key.as_deref().is_some_and(|key| !key.ends_with('/')) {
        return Ok(destination.to_string());
    }

    let file_name = source_file_name_for_tos_transfer(source)?;
    let key = join_tos_key(target.key.as_deref().unwrap_or_default(), &file_name);
    if source.starts_with("tos://") {
        let source_target = parse_tos_uri(source, false)?;
        // [Review Fix #1] Directory-style destinations can resolve back to the
        // source object; reject that before `mv` reaches its delete-source phase.
        if source_target.bucket == target.bucket && source_target.key.as_deref() == Some(&key) {
            return Err(CliError::ValidationError(
                "source and destination resolve to the same TOS object".to_string(),
            ));
        }
    }
    Ok(format!("tos://{}/{}", target.bucket, key))
}

fn local_destination_uses_directory_semantics(destination: &str) -> bool {
    destination.ends_with('/') || destination.ends_with('\\') || Path::new(destination).is_dir()
}

fn command_path_traversal_confirm_target(
    command: &TosCommand,
    operation: &HighLevelOperation,
) -> Result<Option<String>, CliError> {
    let candidate = match command {
        TosCommand::Cp(args) => transfer_path_traversal_candidate(
            &args.source,
            &args.destination,
            operation.destination.as_deref(),
        )?,
        TosCommand::Mv(args) => transfer_path_traversal_candidate(
            &args.source,
            &args.destination,
            operation.destination.as_deref(),
        )?,
        TosCommand::Sync(args) => {
            transfer_path_traversal_candidate(&args.source, &args.destination, None)?
        }
        _ => None,
    };
    Ok(candidate.map(|target| path_traversal_confirmation_token(operation, &target)))
}

fn transfer_path_traversal_candidate(
    source: &str,
    destination: &str,
    resolved_destination: Option<&str>,
) -> Result<Option<String>, CliError> {
    let mut risky_targets = Vec::new();
    if !source.starts_with("tos://") && path_contains_parent_dir(source) {
        risky_targets.push(source.to_string());
    }
    if !destination.starts_with("tos://") && path_contains_parent_dir(destination) {
        risky_targets.push(resolved_destination.unwrap_or(destination).to_string());
    }
    if source.starts_with("tos://") && !destination.starts_with("tos://") {
        let source_target = parse_tos_uri(source, true)?;
        if source_target
            .key
            .as_deref()
            .is_some_and(tos_key_contains_parent_dir)
        {
            risky_targets.push(resolved_destination.unwrap_or(destination).to_string());
        }
    }
    if risky_targets.is_empty() {
        Ok(None)
    } else if risky_targets.len() == 1 {
        Ok(risky_targets.pop())
    } else {
        Ok(Some(format!("{source} -> {destination}")))
    }
}

fn path_traversal_confirmation_token(operation: &HighLevelOperation, candidate: &str) -> String {
    if matches!(operation.risk, RiskLevel::Critical) {
        critical_confirmation_token(operation)
    } else {
        candidate.to_string()
    }
}

fn path_contains_parent_dir(path: &str) -> bool {
    Path::new(path)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
        || path
            .split(|ch| ch == '/' || ch == '\\')
            .any(|segment| segment == "..")
}

fn tos_key_contains_parent_dir(key: &str) -> bool {
    key.split(|ch| ch == '/' || ch == '\\')
        .any(|segment| segment == "..")
}

fn enforce_path_traversal_confirmation(
    global: &GlobalArgs,
    operation: &HighLevelOperation,
    confirm_target: &str,
    can_prompt: bool,
) -> Result<(), CliError> {
    enforce_path_traversal_confirmation_for_command(
        global,
        operation.force,
        operation.requires_force,
        operation.command,
        confirm_target,
        can_prompt,
    )
}

fn enforce_path_traversal_confirmation_for_command(
    global: &GlobalArgs,
    force: bool,
    requires_force: bool,
    command: &str,
    confirm_target: &str,
    can_prompt: bool,
) -> Result<(), CliError> {
    if can_prompt {
        if force {
            return Ok(());
        }
        if requires_force {
            eprintln!(
                "warn: path traversal risk detected for {}; destructive confirmation is still required",
                confirm_target
            );
            return Ok(());
        }
        eprint!(
            "path traversal risk targeting '{}'\n  Type 'yes' to proceed: ",
            confirm_target
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).map_err(|err| {
            CliError::ValidationError(format!("failed to read confirmation input: {}", err))
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

    if !force {
        return Err(CliError::ValidationError(format!(
            "path traversal risk for '{}' in {} requires --force and --confirm {} in non-interactive execution",
            confirm_target, command, confirm_target
        )));
    }

    match global.confirm.as_deref() {
        Some(provided) if provided == confirm_target => Ok(()),
        Some(provided) => Err(CliError::ValidationError(format!(
            "--confirm '{}' does not match the path traversal target '{}' for {}",
            provided, confirm_target, command
        ))),
        None => Err(CliError::ValidationError(format!(
            "path traversal risk for '{}' in {} requires --confirm {} in non-interactive execution",
            confirm_target, command, confirm_target
        ))),
    }
}

fn enforce_transfer_plan_path_traversal(
    global: &GlobalArgs,
    command: &'static str,
    force: bool,
    requires_force: bool,
    confirm_target: &str,
    items: &[TransferPlanItem],
) -> Result<(), CliError> {
    if items.iter().any(transfer_plan_item_has_local_parent_dir) {
        enforce_path_traversal_confirmation_for_command(
            global,
            force,
            requires_force,
            command,
            confirm_target,
            can_prompt_for_confirmation(global),
        )?;
    }
    Ok(())
}

fn transfer_plan_item_has_local_parent_dir(item: &TransferPlanItem) -> bool {
    (!item.source.starts_with("tos://") && path_contains_parent_dir(&item.source))
        || item.destination.strip_prefix("tos://").is_none()
            && path_contains_parent_dir(&item.destination)
}

fn can_prompt_for_confirmation(global: &GlobalArgs) -> bool {
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let stderr_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    stdin_tty && stderr_tty && !global.quiet
}

fn source_file_name_for_tos_transfer(source: &str) -> Result<String, CliError> {
    if source.starts_with("tos://") {
        let parsed = parse_tos_uri(source, false)?;
        let key = parsed
            .key
            .as_deref()
            .unwrap_or_default()
            .trim_end_matches('/');
        return key
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
            .ok_or_else(|| {
                CliError::ValidationError(format!(
                    "source URI '{}' does not include a file name",
                    source
                ))
            });
    }

    Path::new(source)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            CliError::ValidationError(format!(
                "source path '{}' does not include a valid file name",
                source
            ))
        })
}

fn collect_local_files(root: &Path) -> Result<Vec<PathBuf>, CliError> {
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            let entry_path = entry.path();
            if entry_path.is_dir() {
                pending.push(entry_path);
            } else if entry_path.is_file() {
                files.push(entry_path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn prune_empty_directories(root: &Path) -> Result<(), CliError> {
    if !root.exists() {
        return Ok(());
    }
    let mut directories = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path)? {
            let entry_path = entry?.path();
            if entry_path.is_dir() {
                pending.push(entry_path.clone());
                directories.push(entry_path);
            }
        }
    }
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        if fs::read_dir(&directory)?.next().is_none() {
            fs::remove_dir(&directory)?;
        }
    }
    if fs::read_dir(root)?.next().is_none() {
        fs::remove_dir(root)?;
    }
    Ok(())
}

// [Review Fix #Progress-Overall] 返回元组改为 (relative_key, source, destination, size)，
// size 用于 BatchProgressSummary 计算 total_bytes 以渲染整体字节进度条。
// 本地源使用 metadata().len()；TOS 源使用 ListObjects 返回的 Size 字段。
fn normalize_recursive_tos_prefix(prefix: Option<&str>) -> String {
    let prefix = prefix.unwrap_or_default().trim_start_matches('/');
    if prefix.is_empty() || prefix.ends_with('/') {
        prefix.to_string()
    } else {
        format!("{prefix}/")
    }
}

fn normalize_recursive_tos_target(target: &mut ParsedTosUri) {
    if target.key.is_some() {
        // [Review Fix #RecursivePrefix] Recursive high-level commands use
        // directory semantics; a bare `folder` prefix must not match `folder2`.
        target.key = Some(normalize_recursive_tos_prefix(target.key.as_deref()));
    }
}

fn recursive_source_parent_prefix(
    source: &str,
    include_parent: bool,
) -> Result<Option<String>, CliError> {
    if !include_parent {
        return Ok(None);
    }
    let parent_name = if source.starts_with("tos://") {
        let parsed = parse_tos_uri(source, true)?;
        normalize_recursive_tos_prefix(parsed.key.as_deref())
            .trim_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
    } else {
        Path::new(source)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .map(ToString::to_string)
    };
    parent_name.map(Some).ok_or_else(|| {
        CliError::ValidationError(
            "--include-parent requires a source directory or prefix with a final path segment"
                .to_string(),
        )
    })
}

fn prepend_parent_prefix(relative_key: &str, parent_prefix: Option<&str>) -> String {
    match parent_prefix {
        Some(parent) => join_tos_key(parent, relative_key),
        None => relative_key.to_string(),
    }
}

async fn build_recursive_copy_mappings(
    client: &TosClient,
    source: &str,
    destination: &str,
    include_parent: bool,
    recursive_list_mode: Option<RecursiveListMode>,
    list_concurrency: usize,
) -> Result<Vec<TransferPlanItem>, CliError> {
    match (
        source.starts_with("tos://"),
        destination.starts_with("tos://"),
    ) {
        (false, false) => build_local_source_mappings(source, destination, include_parent),
        (false, true) => build_local_source_mappings(source, destination, include_parent),
        (true, false) => {
            build_tos_source_mappings(
                client,
                source,
                destination,
                include_parent,
                recursive_list_mode,
                list_concurrency,
            )
            .await
        }
        (true, true) => {
            build_tos_source_mappings(
                client,
                source,
                destination,
                include_parent,
                recursive_list_mode,
                list_concurrency,
            )
            .await
        }
    }
}

fn build_local_source_mappings(
    source: &str,
    destination: &str,
    include_parent: bool,
) -> Result<Vec<TransferPlanItem>, CliError> {
    let source_root = Path::new(source);
    if !source_root.is_dir() {
        return Err(CliError::ValidationError(format!(
            "recursive source '{}' must be a local directory or tos:// prefix",
            source
        )));
    }
    let parent_prefix = recursive_source_parent_prefix(source, include_parent)?;
    collect_local_files(source_root)?
        .into_iter()
        .map(|file| {
            let relative = file.strip_prefix(source_root).map_err(|err| {
                CliError::ValidationError(format!("failed to derive relative path: {}", err))
            })?;
            let source_relative_key = relative.to_string_lossy().replace('\\', "/");
            let relative_key =
                prepend_parent_prefix(&source_relative_key, parent_prefix.as_deref());
            let target = if destination.starts_with("tos://") {
                let target = parse_tos_uri(destination, true)?;
                format!(
                    "tos://{}/{}",
                    target.bucket,
                    join_tos_key(target.key.as_deref().unwrap_or(""), &relative_key)
                )
            } else {
                Path::new(destination)
                    .join(relative)
                    .to_string_lossy()
                    .into_owned()
            };
            // [Review Fix #Progress-Overall] 本地源以 metadata.len() 提供精确字节数；
            // 失败时退化为 0（不阻塞复制流程，仅影响进度条精度）。
            let size = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
            Ok(TransferPlanItem {
                relative_key,
                source: file.to_string_lossy().into_owned(),
                destination: target,
                size,
                etag: None,
                crc64: None,
                last_modified: None,
            })
        })
        .collect()
}

async fn build_tos_source_mappings(
    client: &TosClient,
    source: &str,
    destination: &str,
    include_parent: bool,
    recursive_list_mode: Option<RecursiveListMode>,
    list_concurrency: usize,
) -> Result<Vec<TransferPlanItem>, CliError> {
    let mut source_target = parse_tos_uri(source, true)?;
    normalize_recursive_tos_target(&mut source_target);
    let source_prefix = source_target.key.clone().unwrap_or_default();
    let parent_prefix = recursive_source_parent_prefix(source, include_parent)?;
    let source_is_hns = bucket_is_hns(client, &source_target.bucket).await?;
    let use_hierarchical_listing =
        resolve_tos_recursive_list_mode(source_is_hns, recursive_list_mode);
    // [Review Fix #Progress-Overall] 改用 list_object_entries 以同时拿到 Size 字段；
    // FNS 平铺列举，HNS 通过 delimiter="/" 分层展开 common prefixes。
    let entries = list_object_entries_recursive(
        client,
        &source_target.bucket,
        Some(&source_prefix),
        use_hierarchical_listing,
        list_concurrency,
    )
    .await?;
    build_tos_source_transfer_items(
        &source_target,
        &source_prefix,
        destination,
        parent_prefix.as_deref(),
        entries,
    )
}

fn build_tos_source_transfer_items(
    source_target: &ParsedTosUri,
    source_prefix: &str,
    destination: &str,
    parent_prefix: Option<&str>,
    entries: Vec<ObjectEntry>,
) -> Result<Vec<TransferPlanItem>, CliError> {
    let destination_target = if destination.starts_with("tos://") {
        Some(parse_tos_uri(destination, true)?)
    } else {
        None
    };
    let mut items = Vec::new();
    for entry in entries {
        // [Review Fix #TOS-RecursiveDirMarker] Preserve trailing-slash entries
        // in local recursive downloads so empty directories are materialized by
        // the mkdir download branch instead of being silently dropped.
        let source_relative_key = strip_tos_prefix(&entry.key, source_prefix).to_string();
        let relative_key = prepend_parent_prefix(&source_relative_key, parent_prefix);
        let source_uri = format!("tos://{}/{}", source_target.bucket, entry.key);
        let target = if let Some(destination_target) = &destination_target {
            format!(
                "tos://{}/{}",
                destination_target.bucket,
                join_tos_key(
                    destination_target.key.as_deref().unwrap_or(""),
                    &relative_key
                )
            )
        } else {
            Path::new(destination)
                .join(relative_key.replace('/', std::path::MAIN_SEPARATOR_STR))
                .to_string_lossy()
                .into_owned()
        };
        items.push(TransferPlanItem {
            relative_key,
            source: source_uri,
            destination: target,
            size: entry.size,
            etag: entry.etag,
            crc64: None,
            last_modified: entry.last_modified,
        });
    }
    Ok(items)
}

fn sync_is_recursive(args: &SyncArgs) -> bool {
    Path::new(&args.source).is_dir()
        || Path::new(&args.destination).is_dir()
        || args.source.starts_with("tos://")
        || args.destination.starts_with("tos://")
}

async fn build_sync_delete_plan(
    client: &TosClient,
    args: &SyncArgs,
    list_concurrency: usize,
) -> Result<Vec<TransferManifestItem>, CliError> {
    match (
        args.source.starts_with("tos://"),
        args.destination.starts_with("tos://"),
    ) {
        (false, false) => build_local_extras_plan(
            Path::new(&args.source),
            Path::new(&args.destination),
            &args.source,
            args.include_parent,
            args.include.as_deref(),
            args.exclude.as_deref(),
        ),
        (true, false) => {
            build_local_extras_plan_for_tos_source(client, args, list_concurrency).await
        }
        (false, true) => {
            build_tos_extras_plan_for_local_source(client, args, list_concurrency).await
        }
        (true, true) => build_tos_extras_plan_for_tos_source(client, args, list_concurrency).await,
    }
}

async fn execute_sync_delete_plan(
    client: &TosClient,
    mut delete_plan: Vec<TransferManifestItem>,
    batch_concurrency: usize,
    progress_enabled: bool,
    report: &mut BatchReport,
) -> Result<u64, CliError> {
    // [Review Fix #2] Remote sync delete-extra may include HNS directory markers.
    sort_tos_sync_delete_manifest_items_bottom_up(&mut delete_plan);
    let del_bar = if progress_enabled && !delete_plan.is_empty() {
        let pb = ProgressBar::new(delete_plan.len() as u64);
        pb.set_style(
            ProgressStyle::with_template(
                "ve-tos sync delete-extra [{bar:30.red/blue}] {pos}/{len} ({per_sec}, ETA {eta})",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-"),
        );
        pb.enable_steady_tick(Duration::from_millis(200));
        Some(pb)
    } else {
        None
    };

    let mut deleted = 0_u64;
    let (leaf_items, mut directory_items): (Vec<_>, Vec<_>) =
        delete_plan.into_iter().partition(|item| {
            !item.source.starts_with("tos://")
                || !tos_delete_key_is_directory(&tos_sync_delete_sort_key(item))
        });
    deleted +=
        delete_sync_delete_item_group(client, leaf_items, batch_concurrency, &del_bar, report)
            .await?;
    sort_tos_sync_delete_manifest_items_bottom_up(&mut directory_items);
    let mut index = 0;
    while index < directory_items.len() {
        let depth = key_depth(&tos_sync_delete_sort_key(&directory_items[index]));
        let mut end = index + 1;
        while end < directory_items.len()
            && key_depth(&tos_sync_delete_sort_key(&directory_items[end])) == depth
        {
            end += 1;
        }
        deleted += delete_sync_delete_item_group(
            client,
            directory_items[index..end].to_vec(),
            batch_concurrency,
            &del_bar,
            report,
        )
        .await?;
        index = end;
    }
    if let Some(bar) = del_bar {
        bar.finish();
    }
    Ok(deleted)
}

async fn delete_sync_delete_item_group(
    client: &TosClient,
    items: Vec<TransferManifestItem>,
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report: &mut BatchReport,
) -> Result<u64, CliError> {
    let mut pending = items.into_iter();
    let mut in_flight = FuturesUnordered::new();
    let limit = batch_concurrency.max(1);
    let mut deleted = 0_u64;
    loop {
        while in_flight.len() < limit {
            let Some(item) = pending.next() else {
                break;
            };
            in_flight.push(async move {
                let result = delete_sync_delete_item(client, &item).await;
                (item, result)
            });
        }
        let Some((item, result)) = in_flight.next().await else {
            break;
        };
        match result {
            Ok(()) => {
                deleted += 1;
                report.record_success("delete-extra", &item.source, None);
            }
            Err(err) => {
                report.record_failure("delete-extra", &item.source, None, &err);
                eprintln!("warn: sync delete-extra failed {}: {}", item.source, err);
            }
        }
        if let Some(bar) = progress {
            bar.inc(1);
        }
    }
    Ok(deleted)
}

async fn delete_sync_delete_item(
    client: &TosClient,
    item: &TransferManifestItem,
) -> Result<(), CliError> {
    if item.source.starts_with("tos://") {
        let target = parse_tos_uri(&item.source, false)?;
        let key = target.key.expect("validated object key");
        delete_tos_object(
            client,
            "ve-tos sync delete-extra",
            &target.bucket,
            &key,
            item.etag.as_deref(),
        )
        .await
        .map(|_| ())
    } else {
        fs::remove_file(&item.source).map_err(CliError::Io)
    }
}

fn sort_tos_sync_delete_entries_bottom_up(entries: &mut [ObjectEntry]) {
    entries.sort_by(|left, right| {
        key_depth(&right.key)
            .cmp(&key_depth(&left.key))
            .then_with(|| right.key.len().cmp(&left.key.len()))
            .then_with(|| left.key.cmp(&right.key))
    });
}

fn sort_tos_sync_delete_manifest_items_bottom_up(items: &mut [TransferManifestItem]) {
    items.sort_by(|left, right| {
        let left_key = tos_sync_delete_sort_key(left);
        let right_key = tos_sync_delete_sort_key(right);
        key_depth(&right_key)
            .cmp(&key_depth(&left_key))
            .then_with(|| right_key.len().cmp(&left_key.len()))
            .then_with(|| left_key.cmp(&right_key))
    });
}

fn tos_sync_delete_sort_key(item: &TransferManifestItem) -> String {
    if item.source.starts_with("tos://") {
        return parse_tos_uri(&item.source, false)
            .ok()
            .and_then(|target| target.key)
            .unwrap_or_else(|| item.relative_key.clone());
    }
    item.relative_key.clone()
}

fn build_local_extras_plan(
    source_root: &Path,
    destination_root: &Path,
    source: &str,
    include_parent: bool,
    include: Option<&str>,
    exclude: Option<&str>,
) -> Result<Vec<TransferManifestItem>, CliError> {
    let parent_prefix = recursive_source_parent_prefix(source, include_parent)?;
    let desired = collect_local_files(source_root)?
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(source_root).map_err(|err| {
                CliError::ValidationError(format!("failed to derive relative path: {}", err))
            })?;
            let relative_key = relative.to_string_lossy().replace('\\', "/");
            Ok::<PathBuf, CliError>(PathBuf::from(
                prepend_parent_prefix(&relative_key, parent_prefix.as_deref())
                    .replace('/', std::path::MAIN_SEPARATOR_STR),
            ))
        })
        .collect::<Result<HashSet<_>, _>>()?;
    build_local_extras_plan_from_desired(destination_root, &desired, include, exclude)
}

async fn build_local_extras_plan_for_tos_source(
    client: &TosClient,
    args: &SyncArgs,
    list_concurrency: usize,
) -> Result<Vec<TransferManifestItem>, CliError> {
    let mut source = parse_tos_uri(&args.source, true)?;
    normalize_recursive_tos_target(&mut source);
    let source_prefix = source.key.clone().unwrap_or_default();
    let parent_prefix = recursive_source_parent_prefix(&args.source, args.include_parent)?;
    let source_is_hns = bucket_is_hns(client, &source.bucket).await?;
    let source_listing = TosRecursiveListOptions {
        use_hierarchical_listing: resolve_tos_recursive_list_mode(
            source_is_hns,
            args.recursive_list_mode,
        ),
        list_concurrency,
    };
    let desired =
        list_object_keys_recursive(client, &source.bucket, Some(&source_prefix), source_listing)
            .await?
            .into_iter()
            .map(|key| {
                let relative_key = prepend_parent_prefix(
                    strip_tos_prefix(&key, &source_prefix),
                    parent_prefix.as_deref(),
                );
                PathBuf::from(relative_key.replace('/', std::path::MAIN_SEPARATOR_STR))
            })
            .collect::<HashSet<_>>();
    let destination_root = Path::new(&args.destination);
    build_local_extras_plan_from_desired(
        destination_root,
        &desired,
        args.include.as_deref(),
        args.exclude.as_deref(),
    )
}

fn build_local_extras_plan_from_desired(
    destination_root: &Path,
    desired: &HashSet<PathBuf>,
    include: Option<&str>,
    exclude: Option<&str>,
) -> Result<Vec<TransferManifestItem>, CliError> {
    // [Review Fix #8] A missing local destination has no extras; treating it as
    // an IO error makes `sync --delete` fail before the copy phase can create it.
    if !destination_root.exists() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for file in collect_local_files(destination_root)? {
        let relative = file.strip_prefix(destination_root).map_err(|err| {
            CliError::ValidationError(format!("failed to derive relative path: {}", err))
        })?;
        let relative_key = relative.to_string_lossy().replace('\\', "/");
        if !pattern_allows(&relative_key, include, exclude) {
            continue;
        }
        if !desired.contains(relative) {
            let size = fs::metadata(&file)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            items.push(TransferManifestItem {
                operation: "delete-extra",
                relative_key,
                source: file.to_string_lossy().into_owned(),
                destination: None,
                size,
                etag: None,
                crc64: None,
                last_modified: None,
            });
        }
    }
    Ok(items)
}

async fn build_tos_extras_plan_for_local_source(
    client: &TosClient,
    args: &SyncArgs,
    list_concurrency: usize,
) -> Result<Vec<TransferManifestItem>, CliError> {
    let source_root = Path::new(&args.source);
    let mut destination = parse_tos_uri(&args.destination, true)?;
    normalize_recursive_tos_target(&mut destination);
    let destination_prefix = destination.key.clone().unwrap_or_default();
    let parent_prefix = recursive_source_parent_prefix(&args.source, args.include_parent)?;
    let destination_is_hns = bucket_is_hns(client, &destination.bucket).await?;
    let destination_listing = TosRecursiveListOptions {
        use_hierarchical_listing: resolve_tos_recursive_list_mode(
            destination_is_hns,
            args.recursive_list_mode,
        ),
        list_concurrency,
    };
    let desired = collect_local_files(source_root)?
        .into_iter()
        .map(|path| {
            let relative = path.strip_prefix(source_root).map_err(|err| {
                CliError::ValidationError(format!("failed to derive relative path: {}", err))
            })?;
            let relative_key = relative.to_string_lossy().replace('\\', "/");
            if !pattern_allows(
                &relative_key,
                args.include.as_deref(),
                args.exclude.as_deref(),
            ) {
                return Ok(None);
            }
            let target_relative_key =
                prepend_parent_prefix(&relative_key, parent_prefix.as_deref());
            Ok(Some(join_tos_key(
                &destination_prefix,
                &target_relative_key,
            )))
        })
        .collect::<Result<Vec<_>, CliError>>()?
        .into_iter()
        .flatten()
        .collect::<HashSet<_>>();
    build_tos_extras_plan(
        client,
        &destination.bucket,
        &destination_prefix,
        &desired,
        args.include.as_deref(),
        args.exclude.as_deref(),
        destination_listing,
    )
    .await
}

async fn build_tos_extras_plan_for_tos_source(
    client: &TosClient,
    args: &SyncArgs,
    list_concurrency: usize,
) -> Result<Vec<TransferManifestItem>, CliError> {
    let mut source = parse_tos_uri(&args.source, true)?;
    let mut destination = parse_tos_uri(&args.destination, true)?;
    normalize_recursive_tos_target(&mut source);
    normalize_recursive_tos_target(&mut destination);
    let source_prefix = source.key.clone().unwrap_or_default();
    let destination_prefix = destination.key.clone().unwrap_or_default();
    let parent_prefix = recursive_source_parent_prefix(&args.source, args.include_parent)?;
    let source_is_hns = bucket_is_hns(client, &source.bucket).await?;
    let destination_is_hns = bucket_is_hns(client, &destination.bucket).await?;
    let source_listing = TosRecursiveListOptions {
        use_hierarchical_listing: resolve_tos_recursive_list_mode(
            source_is_hns,
            args.recursive_list_mode,
        ),
        list_concurrency,
    };
    let destination_listing = TosRecursiveListOptions {
        use_hierarchical_listing: resolve_tos_recursive_list_mode(
            destination_is_hns,
            args.recursive_list_mode,
        ),
        list_concurrency,
    };
    let desired =
        list_object_keys_recursive(client, &source.bucket, Some(&source_prefix), source_listing)
            .await?
            .into_iter()
            .map(|key| {
                let target_relative_key = prepend_parent_prefix(
                    strip_tos_prefix(&key, &source_prefix),
                    parent_prefix.as_deref(),
                );
                join_tos_key(&destination_prefix, &target_relative_key)
            })
            .collect::<HashSet<_>>();
    build_tos_extras_plan(
        client,
        &destination.bucket,
        &destination_prefix,
        &desired,
        args.include.as_deref(),
        args.exclude.as_deref(),
        destination_listing,
    )
    .await
}

async fn build_tos_extras_plan(
    client: &TosClient,
    bucket: &str,
    prefix: &str,
    desired: &HashSet<String>,
    include: Option<&str>,
    exclude: Option<&str>,
    list_options: TosRecursiveListOptions,
) -> Result<Vec<TransferManifestItem>, CliError> {
    let mut items = Vec::new();
    for entry in list_object_entries_recursive(
        client,
        bucket,
        Some(prefix),
        list_options.use_hierarchical_listing,
        list_options.list_concurrency,
    )
    .await?
    {
        let relative_key = strip_tos_prefix(&entry.key, prefix);
        if !pattern_allows(relative_key, include, exclude) {
            continue;
        }
        if !desired.contains(&entry.key) {
            // [Review Fix #SyncManifest] Plan delete-extra with the discovered ETag
            // before execution so manifest and report share the same target set.
            items.push(TransferManifestItem {
                operation: "delete-extra",
                relative_key: relative_key.to_string(),
                source: format!("tos://{}/{}", bucket, entry.key),
                destination: None,
                size: entry.size,
                etag: entry.etag.clone(),
                crc64: None,
                last_modified: entry.last_modified.clone(),
            });
        }
    }
    Ok(items)
}

async fn list_object_keys_recursive(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    list_options: TosRecursiveListOptions,
) -> Result<Vec<String>, CliError> {
    Ok(list_object_entries_recursive(
        client,
        bucket,
        prefix,
        list_options.use_hierarchical_listing,
        list_options.list_concurrency,
    )
    .await?
    .into_iter()
    .map(|entry| entry.key)
    .collect())
}

async fn list_object_entries_for_bucket(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
) -> Result<Vec<ObjectEntry>, CliError> {
    let use_hierarchical_listing =
        resolve_tos_recursive_list_mode(bucket_is_hns(client, bucket).await?, None);
    list_object_entries_recursive(
        client,
        bucket,
        prefix,
        use_hierarchical_listing,
        DEFAULT_LIST_CONCURRENCY,
    )
    .await
}

#[derive(Debug, Clone)]
struct MultipartUploadRef {
    key: String,
    upload_id: String,
}

async fn list_multipart_uploads_for_rm(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
) -> Result<Vec<MultipartUploadRef>, CliError> {
    let mut uploads = Vec::new();
    let mut key_marker: Option<String> = None;
    let mut upload_id_marker: Option<String> = None;
    loop {
        let mut query = BTreeMap::from([
            ("uploads".to_string(), String::new()),
            ("max-uploads".to_string(), "1000".to_string()),
        ]);
        if let Some(p) = prefix {
            query.insert("prefix".to_string(), p.to_string());
        }
        if let Some(km) = key_marker.take() {
            query.insert("key-marker".to_string(), km);
        }
        if let Some(uim) = upload_id_marker.take() {
            query.insert("upload-id-marker".to_string(), uim);
        }
        let response =
            core::send_bucket_request(client, Method::GET, bucket, query, BTreeMap::new(), None)
                .await?;
        let response = client.check_response(response).await?;
        let body = response.text().await.map_err(CliError::Http)?;
        let json = parse_json_body(&body, "ListMultipartUploads")?;
        for item in json_array(&json, &["uploads", "upload", "Uploads", "Upload"]) {
            let key = item
                .get("Key")
                .or_else(|| item.get("key"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let upload_id = item
                .get("UploadId")
                .or_else(|| item.get("upload_id"))
                .or_else(|| item.get("uploadId"))
                .or_else(|| item.get("uploadID"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !key.is_empty() && !upload_id.is_empty() {
                uploads.push(MultipartUploadRef { key, upload_id });
            }
        }
        let is_truncated = json_bool(&json, &["is_truncated", "IsTruncated"]).unwrap_or(false);
        if !is_truncated {
            break;
        }
        key_marker = json_string(&json, &["next_key_marker", "NextKeyMarker"]);
        upload_id_marker = json_string(&json, &["next_upload_id_marker", "NextUploadIdMarker"]);
        if key_marker.is_none() {
            break;
        }
    }
    Ok(uploads)
}

async fn list_object_entries_with_prefixes(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    delimiter: Option<&str>,
) -> Result<(Vec<ObjectEntry>, Vec<String>), CliError> {
    let mut entries = Vec::new();
    let mut prefixes = Vec::new();
    let mut continuation_token = None;
    loop {
        let mut query = list_objects_type2_query(1000);
        if let Some(prefix) = prefix {
            query.insert("prefix".to_string(), prefix.to_string());
        }
        if let Some(delimiter) = delimiter {
            query.insert("delimiter".to_string(), delimiter.to_string());
        }
        if let Some(token) = continuation_token.take() {
            query.insert("continuation-token".to_string(), token);
        }
        let response =
            core::send_bucket_request(client, Method::GET, bucket, query, BTreeMap::new(), None)
                .await?;
        let response = client.check_response(response).await?;
        let body = response.text().await.map_err(CliError::Http)?;
        let json = parse_json_body(&body, "ListObjects")?;
        entries.extend(
            json_array(&json, &["contents", "Contents", "objects"])
                .into_iter()
                .filter_map(parse_object_entry),
        );
        for cp in json_array(
            &json,
            &[
                "common_prefixes",
                "CommonPrefixes",
                "commonPrefixes",
                "common_prefix",
                "commonPrefix",
            ],
        ) {
            if let Some(p) = parse_common_prefix_entry(cp) {
                prefixes.push(p.to_string());
            }
        }
        if !json_bool(&json, &["is_truncated", "IsTruncated"]).unwrap_or(false) {
            break;
        }
        continuation_token = json_string(
            &json,
            &[
                "next_continuation_token",
                "NextContinuationToken",
                "next_marker",
                "NextMarker",
            ],
        );
        if continuation_token.is_none() {
            return Err(CliError::ValidationError(
                "truncated ListObjects JSON response is missing a continuation token".to_string(),
            ));
        }
    }
    entries.sort_by(|left, right| left.key.cmp(&right.key));
    prefixes.sort();
    Ok((entries, prefixes))
}

async fn list_object_entries_with_prefixes_limited(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    max_keys: u32,
    initial_continuation_token: Option<&str>,
) -> Result<(Vec<ObjectEntry>, Vec<String>, Option<String>), CliError> {
    let mut entries = Vec::new();
    let mut prefixes = Vec::new();
    let mut returned = 0u32;
    let mut continuation_token = initial_continuation_token.map(ToString::to_string);
    let mut next_token = None;

    while returned < max_keys {
        let page_size = bounded_list_page_size(returned, max_keys);
        // [Review Fix #10] High-level `ls` is always a single directory-level
        // listing, so force delimiter="/" instead of carrying a recursive mode.
        let mut query = list_objects_type2_query(page_size);
        query.insert("delimiter".to_string(), "/".to_string());
        if let Some(prefix) = prefix {
            query.insert("prefix".to_string(), prefix.to_string());
        }
        if let Some(token) = continuation_token.take() {
            query.insert("continuation-token".to_string(), token);
        }

        let response =
            core::send_bucket_request(client, Method::GET, bucket, query, BTreeMap::new(), None)
                .await?;
        let response = client.check_response(response).await?;
        let body = response.text().await.map_err(CliError::Http)?;
        let json = parse_json_body(&body, "ListObjects")?;

        let page_entries = json_array(&json, &["contents", "Contents", "objects"])
            .into_iter()
            .filter_map(parse_object_entry)
            .collect::<Vec<_>>();
        let page_prefixes = json_array(
            &json,
            &[
                "common_prefixes",
                "CommonPrefixes",
                "commonPrefixes",
                "common_prefix",
                "commonPrefix",
            ],
        )
        .into_iter()
        .filter_map(parse_common_prefix_entry)
        .collect::<Vec<_>>();

        for entry in page_entries {
            if returned >= max_keys {
                break;
            }
            entries.push(entry);
            returned += 1;
        }
        for prefix in page_prefixes {
            if returned >= max_keys {
                break;
            }
            prefixes.push(prefix);
            returned += 1;
        }

        if !json_bool(&json, &["is_truncated", "IsTruncated"]).unwrap_or(false) {
            next_token = None;
            break;
        }
        next_token = json_string(
            &json,
            &[
                "next_continuation_token",
                "NextContinuationToken",
                "next_marker",
                "NextMarker",
            ],
        );
        if next_token.is_none() {
            return Err(CliError::ValidationError(
                "truncated ListObjects JSON response is missing a continuation token".to_string(),
            ));
        }
        if returned >= max_keys {
            break;
        }
        continuation_token = next_token.clone();
    }

    entries.sort_by(|left, right| left.key.cmp(&right.key));
    entries.dedup_by(|left, right| left.key == right.key);
    prefixes.sort();
    prefixes.dedup();
    Ok((entries, prefixes, next_token))
}

struct ListObjectsPage {
    entries: Vec<ObjectEntry>,
    prefixes: Vec<String>,
    next_token: Option<String>,
    is_truncated: bool,
    request_id: Option<String>,
}

struct TosEntriesPrefixScan {
    entries: Vec<ObjectEntry>,
    child_prefixes: Vec<String>,
}

struct TosVersionsPrefixScan {
    versions: Vec<ObjectVersionRef>,
    child_prefixes: Vec<String>,
}

struct DuPrefixScan {
    accumulator: DuAccumulator,
    child_prefixes: Vec<String>,
}

async fn list_object_entries_page(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    delimiter: Option<&str>,
    continuation_token: Option<&str>,
) -> Result<ListObjectsPage, CliError> {
    let mut query = list_objects_type2_query(1000);
    if let Some(prefix) = prefix {
        query.insert("prefix".to_string(), prefix.to_string());
    }
    if let Some(delimiter) = delimiter {
        query.insert("delimiter".to_string(), delimiter.to_string());
    }
    if let Some(token) = continuation_token {
        query.insert("continuation-token".to_string(), token.to_string());
    }

    let response =
        core::send_bucket_request(client, Method::GET, bucket, query, BTreeMap::new(), None)
            .await?;
    let response = client.check_response(response).await?;
    let request_id = core::extract_request_id(&response);
    let body = response.text().await.map_err(CliError::Http)?;
    let json = parse_json_body(&body, "ListObjects")?;
    let entries = json_array(&json, &["contents", "Contents", "objects"])
        .into_iter()
        .filter_map(parse_object_entry)
        .collect::<Vec<_>>();
    let prefixes = json_array(
        &json,
        &[
            "common_prefixes",
            "CommonPrefixes",
            "commonPrefixes",
            "common_prefix",
            "commonPrefix",
        ],
    )
    .into_iter()
    .filter_map(parse_common_prefix_entry)
    .collect::<Vec<_>>();
    let is_truncated = json_bool(&json, &["is_truncated", "IsTruncated"]).unwrap_or(false);
    let next_token = if is_truncated {
        let token = json_string(
            &json,
            &[
                "next_continuation_token",
                "NextContinuationToken",
                "next_marker",
                "NextMarker",
            ],
        );
        if token.is_none() {
            return Err(CliError::ValidationError(
                "truncated ListObjects JSON response is missing a continuation token".to_string(),
            ));
        }
        token
    } else {
        None
    };
    Ok(ListObjectsPage {
        entries,
        prefixes,
        next_token,
        is_truncated,
        request_id: (!request_id.is_empty()).then_some(request_id),
    })
}

async fn collect_tos_du_profile(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    use_hierarchical_listing: bool,
    list_concurrency: usize,
    args: &DuArgs,
) -> Result<DuAccumulator, CliError> {
    if use_hierarchical_listing {
        collect_tos_du_hierarchical(client, bucket, prefix.unwrap_or(""), list_concurrency, args)
            .await
    } else {
        collect_tos_du_flat(client, bucket, prefix, args).await
    }
}

async fn collect_tos_du_flat(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    args: &DuArgs,
) -> Result<DuAccumulator, CliError> {
    let mut accumulator = DuAccumulator::new(args.top_k, args.manifest_path.is_some());
    let mut continuation_token = None;
    loop {
        let page =
            list_object_entries_page(client, bucket, prefix, None, continuation_token.as_deref())
                .await?;
        accumulator.record_request_id(page.request_id.clone());
        for entry in &page.entries {
            accumulator.record_tos_object(bucket, entry, prefix, args.max_depth);
        }
        if !page.is_truncated {
            break;
        }
        continuation_token = page.next_token;
    }
    Ok(accumulator)
}

async fn collect_tos_du_hierarchical(
    client: &TosClient,
    bucket: &str,
    root_prefix: &str,
    list_concurrency: usize,
    args: &DuArgs,
) -> Result<DuAccumulator, CliError> {
    let mut accumulator = DuAccumulator::new(args.top_k, args.manifest_path.is_some());
    let mut pending_prefixes = vec![root_prefix.to_string()];
    let mut seen_prefixes = HashSet::new();
    let mut in_flight = FuturesUnordered::new();

    while !pending_prefixes.is_empty() || !in_flight.is_empty() {
        while in_flight.len() < list_concurrency.max(1) {
            let Some(current_prefix) = pending_prefixes.pop() else {
                break;
            };
            if !seen_prefixes.insert(current_prefix.clone()) {
                continue;
            }
            in_flight.push(scan_tos_du_prefix(
                client,
                bucket,
                root_prefix,
                current_prefix,
                args,
            ));
        }
        let Some(scan) = in_flight.next().await else {
            continue;
        };
        let scan = scan?;
        accumulator.merge(scan.accumulator);
        for child_prefix in scan.child_prefixes {
            if !seen_prefixes.contains(&child_prefix) {
                pending_prefixes.push(child_prefix);
            }
        }
    }
    Ok(accumulator)
}

async fn scan_tos_du_prefix(
    client: &TosClient,
    bucket: &str,
    root_prefix: &str,
    current_prefix: String,
    args: &DuArgs,
) -> Result<DuPrefixScan, CliError> {
    let mut accumulator = DuAccumulator::new(args.top_k, args.manifest_path.is_some());
    let mut child_prefixes = Vec::new();
    let mut continuation_token = None;
    loop {
        let page = list_object_entries_page(
            client,
            bucket,
            (!current_prefix.is_empty()).then_some(current_prefix.as_str()),
            Some("/"),
            continuation_token.as_deref(),
        )
        .await?;
        accumulator.record_request_id(page.request_id.clone());
        let (objects, directory_prefixes, page_child_prefixes) =
            dedupe_tos_du_page_entries(page.entries, page.prefixes, &current_prefix);
        for entry in &objects {
            accumulator.record_tos_object(bucket, entry, Some(root_prefix), args.max_depth);
        }
        for directory_prefix in directory_prefixes {
            accumulator.record_directory_prefix(directory_prefix);
        }
        child_prefixes.extend(page_child_prefixes);
        if !page.is_truncated {
            break;
        }
        continuation_token = page.next_token;
    }
    Ok(DuPrefixScan {
        accumulator,
        child_prefixes,
    })
}

async fn list_object_entries(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    delimiter: Option<&str>,
) -> Result<Vec<ObjectEntry>, CliError> {
    let mut entries = Vec::new();
    let mut continuation_token = None;
    loop {
        let mut query = list_objects_type2_query(1000);
        if let Some(prefix) = prefix {
            query.insert("prefix".to_string(), prefix.to_string());
        }
        if let Some(delimiter) = delimiter {
            query.insert("delimiter".to_string(), delimiter.to_string());
        }
        if let Some(token) = continuation_token.take() {
            query.insert("continuation-token".to_string(), token);
        }
        let response =
            core::send_bucket_request(client, Method::GET, bucket, query, BTreeMap::new(), None)
                .await?;
        let response = client.check_response(response).await?;
        let body = response.text().await.map_err(CliError::Http)?;
        let json = parse_json_body(&body, "ListObjects")?;
        entries.extend(
            json_array(&json, &["contents", "Contents", "objects"])
                .into_iter()
                .filter_map(parse_object_entry),
        );
        if !json_bool(&json, &["is_truncated", "IsTruncated"]).unwrap_or(false) {
            break;
        }
        continuation_token = json_string(
            &json,
            &[
                "next_continuation_token",
                "NextContinuationToken",
                "next_marker",
                "NextMarker",
            ],
        );
        if continuation_token.is_none() {
            return Err(CliError::ValidationError(
                "truncated ListObjects JSON response is missing a continuation token".to_string(),
            ));
        }
    }
    entries.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(entries)
}

async fn list_object_entries_recursive(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    use_hierarchical_listing: bool,
    list_concurrency: usize,
) -> Result<Vec<ObjectEntry>, CliError> {
    if !use_hierarchical_listing {
        return list_object_entries(client, bucket, prefix, None).await;
    }
    let mut entries = Vec::new();
    let mut pending_prefixes = vec![prefix.unwrap_or("").to_string()];
    let mut seen_prefixes = HashSet::new();
    let mut in_flight = FuturesUnordered::new();
    let limit = list_concurrency.max(1);
    while !pending_prefixes.is_empty() || !in_flight.is_empty() {
        while in_flight.len() < limit {
            let Some(current_prefix) = pending_prefixes.pop() else {
                break;
            };
            if !seen_prefixes.insert(current_prefix.clone()) {
                continue;
            }
            in_flight.push(scan_tos_entries_prefix(client, bucket, current_prefix));
        }
        let Some(scan) = in_flight.next().await else {
            continue;
        };
        let mut scan = scan?;
        entries.append(&mut scan.entries);
        for child_prefix in scan.child_prefixes {
            if !seen_prefixes.contains(&child_prefix) {
                pending_prefixes.push(child_prefix);
            }
        }
    }
    entries.sort_by(|left, right| left.key.cmp(&right.key));
    entries.dedup_by(|left, right| left.key == right.key);
    Ok(entries)
}

async fn scan_tos_entries_prefix(
    client: &TosClient,
    bucket: &str,
    current_prefix: String,
) -> Result<TosEntriesPrefixScan, CliError> {
    let (entries, child_prefixes) =
        list_object_entries_with_prefixes(client, bucket, Some(&current_prefix), Some("/")).await?;
    Ok(TosEntriesPrefixScan {
        entries,
        child_prefixes,
    })
}

/// A single object-version reference, used by `--all-versions` deletion paths.
#[derive(Debug, Clone)]
struct ObjectVersionRef {
    key: String,
    version_id: String,
}

async fn list_object_versions_recursive(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    use_hierarchical_listing: bool,
    list_concurrency: usize,
) -> Result<Vec<ObjectVersionRef>, CliError> {
    if !use_hierarchical_listing {
        return Ok(
            list_object_versions_with_prefixes(client, bucket, prefix, None)
                .await?
                .0,
        );
    }
    let mut versions = Vec::new();
    let mut pending_prefixes = vec![prefix.unwrap_or("").to_string()];
    let mut seen_prefixes = HashSet::new();
    let mut in_flight = FuturesUnordered::new();
    let limit = list_concurrency.max(1);
    while !pending_prefixes.is_empty() || !in_flight.is_empty() {
        while in_flight.len() < limit {
            let Some(current_prefix) = pending_prefixes.pop() else {
                break;
            };
            if !seen_prefixes.insert(current_prefix.clone()) {
                continue;
            }
            in_flight.push(scan_tos_versions_prefix(client, bucket, current_prefix));
        }
        let Some(scan) = in_flight.next().await else {
            continue;
        };
        let mut scan = scan?;
        versions.append(&mut scan.versions);
        for child_prefix in scan.child_prefixes {
            if !seen_prefixes.contains(&child_prefix) {
                pending_prefixes.push(child_prefix);
            }
        }
    }
    versions.sort_by(|left, right| {
        left.key
            .cmp(&right.key)
            .then_with(|| left.version_id.cmp(&right.version_id))
    });
    versions.dedup_by(|left, right| left.key == right.key && left.version_id == right.version_id);
    Ok(versions)
}

async fn scan_tos_versions_prefix(
    client: &TosClient,
    bucket: &str,
    current_prefix: String,
) -> Result<TosVersionsPrefixScan, CliError> {
    let (versions, child_prefixes) =
        list_object_versions_with_prefixes(client, bucket, Some(&current_prefix), Some("/"))
            .await?;
    Ok(TosVersionsPrefixScan {
        versions,
        child_prefixes,
    })
}

/// Enumerate every version (and delete marker) under `bucket[/prefix]` via the
/// `?versions` API. This is required when `--all-versions` is set so that
/// versioning-enabled buckets can be permanently emptied.
async fn list_object_versions_with_prefixes(
    client: &TosClient,
    bucket: &str,
    prefix: Option<&str>,
    delimiter: Option<&str>,
) -> Result<(Vec<ObjectVersionRef>, Vec<String>), CliError> {
    let mut versions = Vec::new();
    let mut common_prefixes = Vec::new();
    let mut key_marker: Option<String> = None;
    let mut version_id_marker: Option<String> = None;
    loop {
        let mut query = BTreeMap::from([
            ("versions".to_string(), String::new()),
            ("max-keys".to_string(), "1000".to_string()),
        ]);
        if let Some(prefix) = prefix {
            query.insert("prefix".to_string(), prefix.to_string());
        }
        if let Some(delimiter) = delimiter {
            query.insert("delimiter".to_string(), delimiter.to_string());
        }
        if let Some(km) = key_marker.take() {
            query.insert("key-marker".to_string(), km);
        }
        if let Some(vm) = version_id_marker.take() {
            query.insert("version-id-marker".to_string(), vm);
        }
        let response =
            core::send_bucket_request(client, Method::GET, bucket, query, BTreeMap::new(), None)
                .await?;
        let response = client.check_response(response).await?;
        let body = response.text().await.map_err(CliError::Http)?;
        let json = parse_json_body(&body, "ListObjectVersions")?;

        for item in json_array(&json, &["versions", "Versions"]) {
            if let (Some(key), Some(version_id)) = (
                json_string(item, &["key", "Key"]),
                json_string(item, &["version_id", "VersionId"]),
            ) {
                versions.push(ObjectVersionRef { key, version_id });
            }
        }
        for item in json_array(&json, &["delete_markers", "DeleteMarkers"]) {
            if let (Some(key), Some(version_id)) = (
                json_string(item, &["key", "Key"]),
                json_string(item, &["version_id", "VersionId"]),
            ) {
                versions.push(ObjectVersionRef { key, version_id });
            }
        }
        for cp in json_array(
            &json,
            &[
                "common_prefixes",
                "CommonPrefixes",
                "commonPrefixes",
                "common_prefix",
                "commonPrefix",
            ],
        ) {
            if let Some(prefix) = parse_common_prefix_entry(cp) {
                common_prefixes.push(prefix.to_string());
            }
        }

        if !json_bool(&json, &["is_truncated", "IsTruncated"]).unwrap_or(false) {
            break;
        }
        key_marker = json_string(&json, &["next_key_marker", "NextKeyMarker"]);
        version_id_marker = json_string(&json, &["next_version_id_marker", "NextVersionIdMarker"]);
        if key_marker.is_none() && version_id_marker.is_none() {
            return Err(CliError::ValidationError(
                "truncated ListObjectVersions response is missing pagination markers".to_string(),
            ));
        }
    }
    versions.sort_by(|left, right| {
        left.key
            .cmp(&right.key)
            .then_with(|| left.version_id.cmp(&right.version_id))
    });
    common_prefixes.sort();
    common_prefixes.dedup();
    Ok((versions, common_prefixes))
}

async fn list_uploaded_part_numbers(
    client: &TosClient,
    bucket: &str,
    key: &str,
    upload_id: &str,
) -> Result<HashSet<u32>, CliError> {
    let mut parts = HashSet::new();
    let mut part_number_marker = None;
    loop {
        let marker = part_number_marker.take();
        let query = list_parts_query(upload_id, marker.as_deref());
        let response = core::send_object_request(
            client,
            Method::GET,
            bucket,
            key,
            query,
            BTreeMap::new(),
            None,
        )
        .await?;
        let response = client.check_response(response).await?;
        let body = response.text().await.map_err(CliError::Http)?;
        let json = parse_json_body(&body, "ListParts")?;
        parts.extend(
            json_array(&json, &["parts", "part", "Parts", "Part"])
                .into_iter()
                .filter_map(|part| json_u32(part, &["part_number", "part_id", "PartNumber"])),
        );
        if !json_bool(&json, &["is_truncated", "IsTruncated"]).unwrap_or(false) {
            break;
        }
        part_number_marker =
            json_string(&json, &["next_part_number_marker", "NextPartNumberMarker"]);
        if part_number_marker.is_none() {
            return Err(CliError::ValidationError(
                "truncated ListParts JSON response is missing next part marker".to_string(),
            ));
        }
    }
    Ok(parts)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RestorePlanItem {
    key: String,
    is_directory: bool,
}

fn restore_plan_item_from_key(key: String) -> RestorePlanItem {
    RestorePlanItem {
        is_directory: is_restore_directory_key(&key),
        key,
    }
}

fn restore_plan_item_from_entry(entry: ObjectEntry) -> RestorePlanItem {
    restore_plan_item_from_key(entry.key)
}

fn is_restore_directory_key(key: &str) -> bool {
    key.ends_with('/')
}

fn restore_plan_manifest_item(
    bucket: &str,
    root_prefix: Option<&str>,
    item: &RestorePlanItem,
) -> ListManifestItem {
    if item.is_directory {
        directory_manifest_item(bucket, root_prefix, &item.key)
    } else {
        key_manifest_item(bucket, root_prefix, &item.key)
    }
}

async fn restore_batch_plan(
    client: &TosClient,
    args: &RestoreArgs,
    path: &str,
    list_concurrency: usize,
) -> Result<Vec<RestorePlanItem>, CliError> {
    let mut target = parse_tos_uri(path, true)?;
    normalize_recursive_tos_target(&mut target);
    if let Some(manifest) = &args.manifest {
        let content = fs::read_to_string(manifest)?;
        let keys = content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(|line| restore_manifest_line_to_key(line, &target))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(keys.into_iter().map(restore_plan_item_from_key).collect());
    }
    let is_hns_bucket = bucket_is_hns(client, &target.bucket).await?;
    let use_hierarchical_listing =
        resolve_tos_recursive_list_mode(is_hns_bucket, args.recursive_list_mode);
    list_object_entries_recursive(
        client,
        &target.bucket,
        target.key.as_deref(),
        use_hierarchical_listing,
        list_concurrency,
    )
    .await
    .map(|entries| {
        entries
            .into_iter()
            .map(restore_plan_item_from_entry)
            .collect()
    })
}

fn restore_key_matches(args: &RestoreArgs, key: &str, path: &str) -> bool {
    let relative = parse_tos_uri(path, true)
        .ok()
        .map(|mut target| {
            normalize_recursive_tos_target(&mut target);
            target
        })
        .and_then(|target| target.key)
        .map(|prefix| strip_tos_prefix(key, &prefix).to_string())
        .unwrap_or_else(|| key.to_string());
    pattern_allows(&relative, args.include.as_deref(), args.exclude.as_deref())
}

async fn restore_one_object(
    client: &TosClient,
    bucket: &str,
    key: &str,
    days: Option<u32>,
    tier: Option<&str>,
    version_id: Option<&str>,
) -> Result<Envelope<core::RawResponseData>, CliError> {
    let mut query = BTreeMap::from([("restore".to_string(), String::new())]);
    if let Some(version_id) = version_id {
        query.insert("queryVersionID".to_string(), version_id.to_string());
    }
    let body = restore_request_body(days, tier)?;
    core::execute_object_request(
        client,
        "ve-tos restore",
        Method::POST,
        bucket,
        key,
        query,
        BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        Some(body),
    )
    .await
}

fn restore_request_body(days: Option<u32>, tier: Option<&str>) -> Result<Vec<u8>, CliError> {
    let mut body = serde_json::Map::new();
    // [Review Fix #9] ve-tos sends JSON bodies; use the Go struct's json tags
    // (`Days`, `RestoreJobParameters`) while preserving SDK tier casing.
    body.insert("Days".to_string(), json!(days.unwrap_or(1)));
    if let Some(tier) = tier {
        body.insert(
            "RestoreJobParameters".to_string(),
            json!({ "Tier": normalize_restore_tier(tier)? }),
        );
    }
    serde_json::to_vec(&Value::Object(body)).map_err(CliError::Json)
}

fn normalize_restore_tier(tier: &str) -> Result<&'static str, CliError> {
    match tier.trim().to_ascii_lowercase().as_str() {
        "expedited" => Ok("Expedited"),
        "standard" => Ok("Standard"),
        "bulk" => Ok("Bulk"),
        _ => Err(CliError::ValidationError(
            "invalid restore tier: expected Expedited, Standard, or Bulk".to_string(),
        )),
    }
}

fn emit_progress(enabled: bool, operation: &str, target: &str) {
    if enabled {
        eprintln!(
            "progress operation={} target={} status=done",
            operation, target
        );
    }
}

// [Review Fix #Progress-PartGranular] Part 粒度文件级进度条。
// 单文件场景下：按字节数推进，simple PUT 一次到 file_size，multipart 每完成
// 一个 part 推进 part_size，断点续传场景在初始化时按已完成 parts 设置 position。
// 上层输出策略禁用时返回 NoOp 句柄，零开销。
pub(crate) struct FileProgress {
    bar: Option<ProgressBar>,
}

fn short_label_from(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

impl FileProgress {
    pub(crate) fn new(enabled: bool, label: &str, total_bytes: u64) -> Self {
        if !enabled {
            return Self { bar: None };
        }
        let bar = ProgressBar::new(total_bytes);
        bar.set_style(
            ProgressStyle::with_template(
                "  {msg} [{bar:30.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta})",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-"),
        );
        bar.set_message(label.to_string());
        bar.enable_steady_tick(Duration::from_millis(120));
        Self { bar: Some(bar) }
    }

    pub(crate) fn set_position(&self, bytes: u64) {
        if let Some(bar) = &self.bar {
            bar.set_position(bytes);
        }
    }

    pub(crate) fn inc(&self, bytes: u64) {
        if let Some(bar) = &self.bar {
            bar.inc(bytes);
        }
    }

    pub(crate) fn finish(self, succeeded: bool) {
        if let Some(bar) = self.bar {
            if succeeded {
                bar.finish();
            } else {
                bar.abandon();
            }
        }
    }
}

// [Review Fix #Progress-Overall] 批量递归命令的"整体字节进度 + 当前文件名"聚合器。
// 维护 succeeded/failed/skipped 计数 + report_path + total_bytes，最终一次 output_result。
// 与 indicatif overall bar 协作：bar 长度为待传输总字节数，单位字节；当前正在
// 处理的文件路径放到 prefix 里以便用户感知进度。逐文件成败仍然落到 batch report。
pub(crate) struct BatchProgressSummary {
    pub command: &'static str,
    pub source: String,
    pub destination: String,
    pub succeeded: u64,
    pub failed: u64,
    pub skipped: u64,
    pub total_bytes: u64,
    pub report_path: Option<String>,
    pub manifest_path: Option<String>,
    pub overall: Option<ProgressBar>,
    progress_granularity: EffectiveProgressGranularity,
    /// Total files used to render the `(n/N files)` suffix in templates.
    files_total: u64,
    files_done: u64,
}

impl BatchProgressSummary {
    pub(crate) fn new(
        command: &'static str,
        source: &str,
        destination: &str,
        report_path: Option<&str>,
        manifest_path: Option<&str>,
        total_files: u64,
        total_progress_units: u64,
        total_bytes: u64,
        progress_granularity: EffectiveProgressGranularity,
        progress_enabled: bool,
    ) -> Self {
        let overall = if progress_enabled && total_files > 0 {
            let (len, template) = match progress_granularity {
                EffectiveProgressGranularity::Part => (
                    total_progress_units.max(1),
                    "{prefix} [{bar:30.green/blue}] {pos}/{len} parts ({per_sec}, ETA {eta}) {msg}",
                ),
                EffectiveProgressGranularity::Byte => (
                    total_bytes.max(1),
                    "{prefix} [{bar:30.green/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, ETA {eta}) {msg}",
                ),
            };
            let bar = ProgressBar::new(len);
            bar.set_style(
                ProgressStyle::with_template(template)
                    .unwrap_or_else(|_| ProgressStyle::default_bar())
                    .progress_chars("=>-"),
            );
            bar.set_prefix(format!("{}", command));
            bar.set_message(format!("0/{}", total_files));
            bar.enable_steady_tick(Duration::from_millis(200));
            Some(bar)
        } else {
            None
        };
        Self {
            command,
            source: source.to_string(),
            destination: destination.to_string(),
            succeeded: 0,
            failed: 0,
            skipped: 0,
            total_bytes: 0,
            report_path: report_path.map(ToString::to_string),
            manifest_path: manifest_path.map(ToString::to_string),
            overall,
            progress_granularity,
            files_total: total_files,
            files_done: 0,
        }
    }

    /// Update the bar message to show which file is currently being processed.
    /// File name appears at the END of the progress line (in {msg}) to prevent
    /// varying name lengths from causing the progress bar to jitter.
    pub(crate) fn set_current_file(&self, label: &str) {
        if let Some(bar) = &self.overall {
            let truncated = if label.len() > 60 {
                format!("…{}", &label[label.len() - 59..])
            } else {
                label.to_string()
            };
            bar.set_message(format!(
                "{}/{} {}",
                self.files_done, self.files_total, truncated
            ));
        }
    }

    /// Advance the byte counter on the overall bar (used while a file is in
    /// flight; granularity matches the underlying transfer driver).
    pub(crate) fn add_bytes(&self, bytes: u64) {
        if self.progress_granularity == EffectiveProgressGranularity::Byte {
            if let Some(bar) = &self.overall {
                bar.inc(bytes);
            }
        }
    }

    pub(crate) fn add_part(&self) {
        if self.progress_granularity == EffectiveProgressGranularity::Part {
            if let Some(bar) = &self.overall {
                bar.inc(1);
            }
        }
    }

    fn refresh_files_msg(&mut self) {
        self.files_done += 1;
        if let Some(bar) = &self.overall {
            bar.set_message(format!("{}/{}", self.files_done, self.files_total));
        }
    }

    pub(crate) fn record_success(&mut self, bytes: u64) {
        self.succeeded += 1;
        self.total_bytes += bytes;
        self.refresh_files_msg();
    }

    #[allow(dead_code)]
    pub(crate) fn record_skip(&mut self) {
        self.skipped += 1;
        self.add_part();
        self.refresh_files_msg();
    }

    pub(crate) fn record_failure(&mut self, _bytes: u64) {
        self.failed += 1;
        self.add_part();
        self.refresh_files_msg();
    }

    pub(crate) fn finish_and_emit(self, global: &GlobalArgs) -> Result<(), CliError> {
        if let Some(bar) = &self.overall {
            // [Review Fix #ProgressRetain] Keep the final progress bar visible so users
            // can see speed, elapsed time, and total bytes after completion.
            bar.finish();
        }
        let envelope = Envelope::success(
            self.command,
            json!({
                "source": self.source,
                "destination": self.destination,
                "succeeded": self.succeeded,
                "failed": self.failed,
                "skipped": self.skipped,
                "total_bytes": self.total_bytes,
                "total_bytes_human": HumanBytes(self.total_bytes).to_string(),
                "report_path": self.report_path,
                "manifest_path": self.manifest_path,
            }),
        );
        output_result(global, &envelope)
    }
}

fn parse_json_body(body: &str, context: &str) -> Result<Value, CliError> {
    let trimmed = body.trim_start();
    let parsed = if trimmed.starts_with('<') {
        // [Review Fix #2] BOE / S3-compatible list APIs may return XML even
        // when high-level commands expect JSON-shaped fields.
        core::parse_xml_to_json(body.as_bytes())
            .map(core::normalize_keys)
            .map_err(|err| {
                CliError::ValidationError(format!("{} response XML parse failed: {}", context, err))
            })?
    } else {
        serde_json::from_str::<Value>(body)
            .map(core::normalize_keys)
            .map_err(|err| {
                CliError::ValidationError(format!(
                    "{} response must be JSON or XML: {}",
                    context, err
                ))
            })?
    };
    Ok(unwrap_response_payload(parsed))
}

fn unwrap_response_payload(mut value: Value) -> Value {
    const WRAPPER_KEYS: &[&str] = &[
        "result",
        "data",
        "body",
        "payload",
        "response",
        "list_bucket_result",
        "list_objects_result",
    ];

    for _ in 0..8 {
        if has_list_response_fields(&value) {
            return value;
        }
        let Value::Object(mut fields) = value else {
            return value;
        };
        let Some(nested) = WRAPPER_KEYS.iter().find_map(|key| fields.remove(*key)) else {
            return Value::Object(fields);
        };
        value = parse_embedded_payload(nested);
    }
    value
}

fn parse_embedded_payload(value: Value) -> Value {
    match value {
        Value::String(text) => match parse_json_body(&text, "embedded response body") {
            Ok(parsed) => parsed,
            Err(_) => Value::String(text),
        },
        other => other,
    }
}

fn has_list_response_fields(value: &Value) -> bool {
    [
        "contents",
        "objects",
        "common_prefixes",
        "common_prefix",
        "uploads",
        "parts",
        "versions",
        "delete_markers",
        "is_truncated",
        "key_count",
        "next_continuation_token",
        "next_marker",
    ]
    .iter()
    .any(|name| value.get(*name).is_some())
}

fn json_array<'a>(value: &'a Value, names: &[&str]) -> Vec<&'a Value> {
    for name in names {
        match value.get(*name) {
            Some(Value::Array(items)) => return items.iter().collect(),
            Some(Value::Object(_)) => return vec![&value[*name]],
            _ => {}
        }
    }
    Vec::new()
}

fn json_string(value: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        value.get(*name).and_then(|field| {
            field
                .as_str()
                .map(ToString::to_string)
                .or_else(|| field.as_u64().map(|number| number.to_string()))
        })
    })
}

fn json_bool(value: &Value, names: &[&str]) -> Option<bool> {
    names.iter().find_map(|name| {
        value.get(*name).and_then(|field| {
            field.as_bool().or_else(|| {
                field
                    .as_str()
                    .and_then(|text| match text.to_ascii_lowercase().as_str() {
                        "true" => Some(true),
                        "false" => Some(false),
                        _ => None,
                    })
            })
        })
    })
}

fn json_u32(value: &Value, names: &[&str]) -> Option<u32> {
    names.iter().find_map(|name| {
        value.get(*name).and_then(|field| {
            field
                .as_u64()
                .and_then(|number| u32::try_from(number).ok())
                .or_else(|| field.as_str().and_then(|text| text.parse::<u32>().ok()))
        })
    })
}

fn json_u64(value: &Value, names: &[&str]) -> Option<u64> {
    names.iter().find_map(|name| {
        value.get(*name).and_then(|field| {
            field
                .as_u64()
                .or_else(|| field.as_str().and_then(|text| text.parse::<u64>().ok()))
        })
    })
}

fn parse_object_entry(value: &Value) -> Option<ObjectEntry> {
    Some(ObjectEntry {
        key: json_string(value, &["key", "Key"])?,
        size: json_u64(value, &["size", "Size", "content_length", "ContentLength"]).unwrap_or(0),
        last_modified: json_string(value, &["last_modified", "LastModified"]),
        etag: json_string(value, &["etag", "e_tag", "ETag"]),
        storage_class: json_string(value, &["storage_class", "StorageClass"]),
    })
}

fn parse_common_prefix_entry(value: &Value) -> Option<String> {
    value.as_str().map(ToString::to_string).or_else(|| {
        value
            .get("Prefix")
            .or_else(|| value.get("prefix"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

fn validate_high_level_ls_max_keys(max_keys: u32) -> Result<(), CliError> {
    if max_keys == 0 {
        return Err(CliError::ValidationError(
            "ve-tos ls --max-keys must be greater than 0".to_string(),
        ));
    }
    Ok(())
}

fn list_objects_type2_query(max_keys: u32) -> BTreeMap<String, String> {
    BTreeMap::from([
        // [Review Fix #5] Match the ByteTOS Rust SDK's ListObjectsType2Input:
        // high-level listing must request the v2 list API explicitly.
        ("list-type".to_string(), "2".to_string()),
        ("max-keys".to_string(), max_keys.to_string()),
    ])
}

fn copy_object_method_query() -> (Method, BTreeMap<String, String>) {
    if active_tos_config_binary() == Binary::Tos {
        // [Review Fix #8] Match the ByteTOS SDK's CopyObjectInput:
        // SigV1 uses POST with ?copyobject, while V4 keeps the legacy PUT path.
        (
            Method::POST,
            BTreeMap::from([("copyobject".to_string(), String::new())]),
        )
    } else {
        (Method::PUT, BTreeMap::new())
    }
}

fn copy_source_header_value(bucket: &str, key: &str) -> String {
    let raw = format!("/{bucket}/{key}");
    if active_tos_config_binary() != Binary::Tos {
        return raw;
    }

    // [Review Fix #9] Match the ByteTOS SDK's copy_object handling: encode the
    // source key with "/" safe, then encode the full "/bucket/key" copy path.
    let encoded_key = byted_tos_url_encode_with_safe(key, "/");
    byted_tos_url_encode_with_safe(&format!("/{bucket}/{encoded_key}"), "")
}

fn byted_tos_url_encode_with_safe(input: &str, safe: &str) -> String {
    const ALLOWED_IN_URL: &str =
        "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    if input.is_empty() {
        return String::new();
    }

    let mut encoded = String::with_capacity(input.len() * 2);
    for byte in input.bytes() {
        let ch = byte as char;
        if ALLOWED_IN_URL.contains(ch) || safe.contains(ch) {
            encoded.push(ch);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{:X}", byte));
        }
    }
    encoded
}

fn multipart_upload_id_query_key() -> &'static str {
    if active_tos_config_binary() == Binary::Tos {
        "uploadID"
    } else {
        "uploadId"
    }
}

fn multipart_upload_id_query(upload_id: &str) -> BTreeMap<String, String> {
    BTreeMap::from([(
        multipart_upload_id_query_key().to_string(),
        upload_id.to_string(),
    )])
}

fn multipart_part_query(upload_id: &str, part_number: u32) -> BTreeMap<String, String> {
    let mut query = multipart_upload_id_query(upload_id);
    query.insert("partNumber".to_string(), part_number.to_string());
    query
}

fn multipart_copy_part_query(
    upload_id: &str,
    part_number: u32,
    start_offset: u64,
    part_size: u64,
) -> BTreeMap<String, String> {
    let mut query = multipart_part_query(upload_id, part_number);
    if active_tos_config_binary() == Binary::Tos {
        // [Review Fix #6] Match the ByteTOS SDK's UploadPartCopyInput:
        // SigV1 carries copy ranges as query params, not x-tos-copy-source-range.
        query.insert("startOffset".to_string(), start_offset.to_string());
        query.insert("partSize".to_string(), part_size.to_string());
    }
    query
}

fn list_parts_query(upload_id: &str, part_number_marker: Option<&str>) -> BTreeMap<String, String> {
    let mut query = multipart_upload_id_query(upload_id);
    if active_tos_config_binary() != Binary::Tos {
        query.insert("max-parts".to_string(), "1000".to_string());
        if let Some(marker) = part_number_marker {
            query.insert("part-number-marker".to_string(), marker.to_string());
        }
    }
    query
}

fn complete_multipart_request(
    upload_id: &str,
    completed_parts: &[CompletedPart],
) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>, Vec<u8>), CliError> {
    if completed_parts.is_empty() {
        return Err(CliError::ValidationError(
            "CompleteMultipartUpload requires at least one part".to_string(),
        ));
    }
    let query = multipart_upload_id_query(upload_id);
    if active_tos_config_binary() == Binary::Tos {
        // [Review Fix #7] Mirror the ByteTOS SDK's CompleteMultipartUpload
        // SigV1 body: "partNumber:etag,partNumber:etag" instead of V4 JSON.
        let body = completed_parts
            .iter()
            .map(|part| {
                if part.etag.is_empty() {
                    part.part_number.to_string()
                } else {
                    format!("{}:{}", part.part_number, part.etag)
                }
            })
            .collect::<Vec<_>>()
            .join(",")
            .into_bytes();
        return Ok((query, BTreeMap::new(), body));
    }

    let complete_body = json!({
        "Parts": completed_parts.iter().map(|part| {
            json!({
                "PartNumber": part.part_number,
                "ETag": part.etag,
            })
        }).collect::<Vec<_>>()
    });
    Ok((
        query,
        BTreeMap::from([("content-type".to_string(), "application/json".to_string())]),
        serde_json::to_vec(&complete_body).map_err(CliError::Json)?,
    ))
}

fn bounded_list_page_size(returned: u32, max_keys: u32) -> u32 {
    max_keys.saturating_sub(returned).min(1000)
}

fn dedupe_ls_objects_and_prefixes(
    objects: Vec<ObjectEntry>,
    common_prefixes: Vec<String>,
    root_prefix: Option<&str>,
) -> (Vec<ObjectEntry>, Vec<String>) {
    let mut prefixes = common_prefixes;
    let mut prefix_set = prefixes.iter().cloned().collect::<HashSet<_>>();
    let current_folder_marker = root_prefix.filter(|prefix| prefix.ends_with('/'));
    let mut deduped_objects = Vec::new();
    for object in objects {
        if current_folder_marker == Some(object.key.as_str()) {
            // [Review Fix #1] Listing tos://bucket/folder/ should show that folder's children,
            // not the folder marker object itself as a file row.
            continue;
        }
        if prefix_set.contains(&object.key) {
            continue;
        }
        if object.key.ends_with('/') {
            // [Review Fix #2] High-level ls treats trailing-slash object markers as directories.
            prefix_set.insert(object.key.clone());
            prefixes.push(object.key);
            continue;
        }
        deduped_objects.push(object);
    }
    (deduped_objects, prefixes)
}

fn dedupe_tos_du_page_entries(
    objects: Vec<ObjectEntry>,
    common_prefixes: Vec<String>,
    current_prefix: &str,
) -> (Vec<ObjectEntry>, Vec<String>, Vec<String>) {
    let current_directory = normalize_tos_directory_prefix(current_prefix);
    let mut directory_set = HashSet::new();
    let mut directory_prefixes = Vec::new();
    let mut child_prefixes = Vec::new();
    for common_prefix in common_prefixes {
        let Some(directory_prefix) = normalize_tos_directory_prefix(&common_prefix) else {
            continue;
        };
        if current_directory.as_deref() == Some(directory_prefix.as_str()) {
            continue;
        }
        if directory_set.insert(directory_prefix.clone()) {
            directory_prefixes.push(directory_prefix.clone());
            child_prefixes.push(directory_prefix);
        }
    }

    let objects = objects
        .into_iter()
        .filter_map(|object| {
            if let Some(directory_prefix) = tos_directory_marker_prefix(&object.key) {
                if current_directory.as_deref() != Some(directory_prefix.as_str())
                    && directory_set.insert(directory_prefix.clone())
                {
                    // [Review Fix #10] `du` de-dupes folder marker objects with CommonPrefixes.
                    directory_prefixes.push(directory_prefix);
                }
                None
            } else {
                Some(object)
            }
        })
        .collect();

    (objects, directory_prefixes, child_prefixes)
}

fn tos_directory_marker_prefix(key: &str) -> Option<String> {
    key.ends_with('/')
        .then(|| normalize_tos_directory_prefix(key))
        .flatten()
}

fn normalize_tos_directory_prefix(prefix: &str) -> Option<String> {
    let trimmed = prefix.trim_end_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        Some(format!("{trimmed}/"))
    }
}

fn merge_ls_entries(objects: Vec<ObjectEntry>, common_prefixes: Vec<String>) -> Vec<LsEntry> {
    let mut merged = BTreeMap::new();
    for object in objects {
        merged.insert(
            object.key.clone(),
            LsEntry {
                key: object.key,
                entry_type: "file",
                size: object.size,
                last_modified: object.last_modified,
                etag: object.etag,
                storage_class: object.storage_class,
            },
        );
    }
    for prefix in common_prefixes {
        // [Review Fix #LsDirectories] CommonPrefixes are directory entries at
        // the current delimiter level; they win over same-key folder marker
        // objects so `ls` does not render duplicate file+directory rows.
        merged.insert(
            prefix.clone(),
            LsEntry {
                key: prefix,
                entry_type: "directory",
                size: 0,
                last_modified: None,
                etag: None,
                storage_class: None,
            },
        );
    }
    merged.into_values().collect()
}

fn sort_ls_entries(entries: &mut [LsEntry], sort: Option<&str>) -> Result<(), CliError> {
    match sort.unwrap_or("key") {
        "key" | "name" => entries.sort_by(|left, right| left.key.cmp(&right.key)),
        "size" => entries.sort_by(|left, right| left.size.cmp(&right.size)),
        "last-modified" | "mtime" => entries.sort_by(|left, right| {
            left.last_modified
                .as_deref()
                .unwrap_or("")
                .cmp(right.last_modified.as_deref().unwrap_or(""))
        }),
        other => {
            return Err(CliError::ValidationError(format!(
                "unsupported ls sort field '{}': expected key, size, or last-modified",
                other
            )));
        }
    }
    Ok(())
}

fn ls_entries_for_output(entries: &[LsEntry], human_readable: bool) -> Vec<Value> {
    entries
        .iter()
        .map(|entry| {
            json!({
                "key": entry.key,
                "entry_type": entry.entry_type,
                // [Review Fix #1] Keep raw objects[] numeric while making display
                // entries honor --human-readable for table/csv and optional entries consumers.
                "size": ls_entry_size_value(entry.size, human_readable),
                "last_modified": entry.last_modified,
                "etag": entry.etag,
                "storage_class": entry.storage_class,
            })
        })
        .collect()
}

fn ls_entry_size_value(size: u64, human_readable: bool) -> Value {
    if human_readable {
        Value::String(human_bytes(size))
    } else {
        json!(size)
    }
}

impl DuAccumulator {
    fn new(top_k: usize, capture_manifest: bool) -> Self {
        let mut size_histogram = BTreeMap::new();
        for name in ["0-1K", "1K-1M", "1M-100M", ">100M"] {
            size_histogram.insert(name, DuDistributionBucket { count: 0, bytes: 0 });
        }
        Self {
            object_count: 0,
            total_bytes: 0,
            directory_count: 0,
            directory_prefixes: HashSet::new(),
            request_ids: Vec::new(),
            request_ids_omitted: 0,
            capture_manifest,
            manifest_items: Vec::new(),
            file_types: BTreeMap::new(),
            directories: BTreeMap::new(),
            size_histogram,
            storage_classes: BTreeMap::new(),
            largest_objects: Vec::new(),
            oldest_objects: Vec::new(),
            top_k,
        }
    }

    fn record_tos_object(
        &mut self,
        bucket: &str,
        entry: &ObjectEntry,
        prefix: Option<&str>,
        max_depth: Option<u32>,
    ) {
        self.object_count += 1;
        self.total_bytes += entry.size;
        if self.capture_manifest {
            self.manifest_items
                .push(object_entry_manifest_item(bucket, prefix, entry));
        }
        increment_string_du_bucket_limited(
            &mut self.file_types,
            file_extension_bucket(&entry.key),
            entry.size,
            DU_CATEGORY_BUCKET_LIMIT,
        );
        increment_string_du_bucket_limited(
            &mut self.directories,
            directory_group(&entry.key, prefix.unwrap_or(""), max_depth),
            entry.size,
            DU_CATEGORY_BUCKET_LIMIT,
        );
        increment_du_bucket(
            &mut self.size_histogram,
            size_histogram_bucket(entry.size),
            entry.size,
        );
        let storage_class = entry
            .storage_class
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "STANDARD".to_string())
            .to_ascii_uppercase();
        increment_du_bucket(&mut self.storage_classes, storage_class.clone(), entry.size);
        let sample = DuObjectSample {
            key: entry.key.clone(),
            size: entry.size,
            last_modified: entry.last_modified.clone(),
            storage_class: Some(storage_class),
            timestamp_millis: entry
                .last_modified
                .as_deref()
                .and_then(parse_rfc3339_millis),
        };
        self.record_sample(sample);
    }

    fn record_directory_prefix(&mut self, prefix: impl AsRef<str>) -> bool {
        let Some(directory_prefix) = normalize_tos_directory_prefix(prefix.as_ref()) else {
            return false;
        };
        if self.directory_prefixes.insert(directory_prefix) {
            // [Review Fix #7] Directory totals are unique by normalized prefix
            // across paginated and concurrent hierarchical scans.
            self.directory_count += 1;
            true
        } else {
            false
        }
    }

    fn merge(&mut self, other: DuAccumulator) {
        self.object_count += other.object_count;
        self.total_bytes += other.total_bytes;
        for directory_prefix in other.directory_prefixes {
            self.record_directory_prefix(directory_prefix);
        }
        self.merge_request_ids(other.request_ids, other.request_ids_omitted);
        self.manifest_items.extend(other.manifest_items);
        merge_string_du_bucket_map_limited(
            &mut self.file_types,
            other.file_types,
            DU_CATEGORY_BUCKET_LIMIT,
        );
        merge_string_du_bucket_map_limited(
            &mut self.directories,
            other.directories,
            DU_CATEGORY_BUCKET_LIMIT,
        );
        merge_du_bucket_map(&mut self.size_histogram, other.size_histogram);
        merge_du_bucket_map(&mut self.storage_classes, other.storage_classes);
        for sample in other.largest_objects {
            self.record_largest_sample(sample);
        }
        for sample in other.oldest_objects {
            self.record_oldest_sample(sample);
        }
    }

    fn record_sample(&mut self, sample: DuObjectSample) {
        self.record_largest_sample(sample.clone());
        if sample.timestamp_millis.is_some() {
            self.record_oldest_sample(sample);
        }
    }

    fn record_request_id(&mut self, request_id: Option<String>) {
        if let Some(request_id) = request_id.filter(|value| !value.is_empty()) {
            // [Review Fix #3] Preserve request context without retaining one ID per listed page forever.
            if self.request_ids.len() < DU_REQUEST_ID_LIMIT {
                self.request_ids.push(request_id);
            } else {
                self.request_ids_omitted += 1;
            }
        }
    }

    fn merge_request_ids(&mut self, request_ids: Vec<String>, omitted_count: u64) {
        self.request_ids_omitted += omitted_count;
        for request_id in request_ids {
            if self.request_ids.len() < DU_REQUEST_ID_LIMIT {
                self.request_ids.push(request_id);
            } else {
                self.request_ids_omitted += 1;
            }
        }
    }

    fn record_largest_sample(&mut self, sample: DuObjectSample) {
        if self.top_k == 0 {
            return;
        }
        self.largest_objects.push(sample);
        self.largest_objects.sort_by(|left, right| {
            right
                .size
                .cmp(&left.size)
                .then_with(|| left.key.cmp(&right.key))
        });
        self.largest_objects.truncate(self.top_k);
    }

    fn record_oldest_sample(&mut self, sample: DuObjectSample) {
        if self.top_k == 0 {
            return;
        }
        self.oldest_objects.push(sample);
        self.oldest_objects.sort_by(|left, right| {
            left.timestamp_millis
                .unwrap_or(i64::MAX)
                .cmp(&right.timestamp_millis.unwrap_or(i64::MAX))
                .then_with(|| left.key.cmp(&right.key))
        });
        self.oldest_objects.truncate(self.top_k);
    }

    fn file_type_distribution_json(&self) -> BTreeMap<String, Value> {
        du_bucket_map_json(&self.file_types, "object_count")
    }

    fn directory_distribution_json(&self) -> BTreeMap<String, Value> {
        du_bucket_map_json(&self.directories, "object_count")
    }

    fn size_histogram_json(&self) -> BTreeMap<String, Value> {
        du_bucket_map_json(&self.size_histogram, "object_count")
    }

    fn storage_class_distribution_json(&self) -> BTreeMap<String, Value> {
        du_bucket_map_json(&self.storage_classes, "object_count")
    }

    fn cost_estimate(&self, prices: &BTreeMap<String, f64>) -> Value {
        let mut by_storage_class = BTreeMap::new();
        let mut total = 0.0;
        for (class, bucket) in &self.storage_classes {
            let price = prices.get(class).copied().unwrap_or(0.0);
            let gb = bucket.bytes as f64 / 1024_f64.powi(3);
            let estimated = gb * price;
            total += estimated;
            by_storage_class.insert(
                class.clone(),
                json!({
                    "bytes": bucket.bytes,
                    "gb": round_cost(gb),
                    "price_cny_per_gb_month": price,
                    "estimated_cny_month": round_cost(estimated),
                    "priced": prices.contains_key(class),
                }),
            );
        }
        json!({
            "currency": "CNY",
            "period": "month",
            "estimated_total_cny_month": round_cost(total),
            "by_storage_class": by_storage_class,
            "savings_opportunities": self.savings_opportunities(),
            "disclaimer": "估算值，仅包含存储容量单价，不含请求、流量、取回、数据处理等费用；以实际账单为准。",
        })
    }

    fn savings_opportunities(&self) -> Vec<String> {
        let mut suggestions = Vec::new();
        if self
            .storage_classes
            .get("STANDARD")
            .map(|bucket| bucket.bytes > 0)
            .unwrap_or(false)
            && self.oldest_objects.iter().any(|sample| {
                sample
                    .timestamp_millis
                    .map(|ts| {
                        Utc::now().timestamp_millis().saturating_sub(ts) > 90 * 24 * 60 * 60 * 1000
                    })
                    .unwrap_or(false)
            })
        {
            suggestions.push(
                "STANDARD 中存在较旧对象样本，可结合访问日志评估转 IA/归档的生命周期规则。"
                    .to_string(),
            );
        }
        if self.storage_classes.keys().any(|class| class == "UNKNOWN") {
            suggestions.push(
                "部分对象缺少 storage_class，成本估算使用 0 单价，请补充 --storage-price 后复算。"
                    .to_string(),
            );
        }
        suggestions
    }
}

fn du_bucket_map_json<K>(
    map: &BTreeMap<K, DuDistributionBucket>,
    count_key: &str,
) -> BTreeMap<String, Value>
where
    K: ToString + Ord,
{
    map.iter()
        .map(|(key, bucket)| {
            (
                key.to_string(),
                json!({
                    "bytes": bucket.bytes,
                    count_key: bucket.count,
                }),
            )
        })
        .collect()
}

fn increment_du_bucket<K>(map: &mut BTreeMap<K, DuDistributionBucket>, key: K, bytes: u64)
where
    K: Ord,
{
    let bucket = map
        .entry(key)
        .or_insert(DuDistributionBucket { count: 0, bytes: 0 });
    bucket.count += 1;
    bucket.bytes += bytes;
}

fn increment_string_du_bucket_limited(
    map: &mut BTreeMap<String, DuDistributionBucket>,
    key: String,
    bytes: u64,
    limit: usize,
) {
    let bucket_key = if map.contains_key(&key) || map.len() < limit {
        key
    } else {
        DU_OVERFLOW_BUCKET.to_string()
    };
    increment_du_bucket(map, bucket_key, bytes);
}

fn merge_du_bucket_map<K>(
    target: &mut BTreeMap<K, DuDistributionBucket>,
    source: BTreeMap<K, DuDistributionBucket>,
) where
    K: Ord,
{
    for (key, value) in source {
        let bucket = target
            .entry(key)
            .or_insert(DuDistributionBucket { count: 0, bytes: 0 });
        bucket.count += value.count;
        bucket.bytes += value.bytes;
    }
}

fn merge_string_du_bucket_map_limited(
    target: &mut BTreeMap<String, DuDistributionBucket>,
    source: BTreeMap<String, DuDistributionBucket>,
    limit: usize,
) {
    for (key, value) in source {
        merge_one_string_du_bucket_limited(target, key, value, limit);
    }
}

fn merge_one_string_du_bucket_limited(
    target: &mut BTreeMap<String, DuDistributionBucket>,
    key: String,
    value: DuDistributionBucket,
    limit: usize,
) {
    let bucket_key = if target.contains_key(&key) || target.len() < limit {
        key
    } else {
        DU_OVERFLOW_BUCKET.to_string()
    };
    let bucket = target
        .entry(bucket_key)
        .or_insert(DuDistributionBucket { count: 0, bytes: 0 });
    bucket.count += value.count;
    bucket.bytes += value.bytes;
}

fn file_extension_bucket(key: &str) -> String {
    let name = key.rsplit('/').next().unwrap_or(key);
    name.rsplit_once('.')
        .and_then(|(_, ext)| (!ext.is_empty()).then(|| ext.to_ascii_lowercase()))
        .unwrap_or_else(|| "(none)".to_string())
}

fn directory_group(key: &str, prefix: &str, max_depth: Option<u32>) -> String {
    let relative = strip_tos_prefix(key, prefix).trim_matches('/');
    let depth = max_depth.unwrap_or(u32::MAX) as usize;
    if depth == 0 {
        return ".".to_string();
    }
    let mut segments = relative
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    segments.pop();
    let group = segments
        .into_iter()
        .take(depth)
        .collect::<Vec<_>>()
        .join("/");
    if group.is_empty() {
        ".".to_string()
    } else {
        group
    }
}

fn size_histogram_bucket(size: u64) -> &'static str {
    match size {
        0..=1024 => "0-1K",
        1025..=1_048_576 => "1K-1M",
        1_048_577..=104_857_600 => "1M-100M",
        _ => ">100M",
    }
}

fn parse_rfc3339_millis(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|parsed| parsed.timestamp_millis())
}

fn round_cost(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn storage_price_table(overrides: &[String]) -> Result<BTreeMap<String, f64>, CliError> {
    let mut prices = BTreeMap::from([
        ("STANDARD".to_string(), 0.12),
        ("IA".to_string(), 0.08),
        ("ARCHIVE".to_string(), 0.033),
        ("COLD_ARCHIVE".to_string(), 0.016),
    ]);
    for override_value in overrides {
        let Some((class, price)) = override_value.split_once('=') else {
            return Err(CliError::ValidationError(format!(
                "--storage-price expects CLASS=PRICE, got '{}'",
                override_value
            )));
        };
        let parsed = price.parse::<f64>().map_err(|err| {
            CliError::ValidationError(format!(
                "invalid --storage-price '{}': {}",
                override_value, err
            ))
        })?;
        if parsed < 0.0 {
            return Err(CliError::ValidationError(format!(
                "--storage-price must be non-negative, got '{}'",
                override_value
            )));
        }
        prices.insert(class.to_ascii_uppercase(), parsed);
    }
    Ok(prices)
}

fn validate_du_top_k(top_k: usize) -> Result<(), CliError> {
    if top_k > 1000 {
        return Err(CliError::ValidationError(
            "du --top-k must be less than or equal to 1000".to_string(),
        ));
    }
    Ok(())
}

fn uses_tabular_output(global: &GlobalArgs) -> bool {
    matches!(
        global.output.unwrap_or_else(OutputFormat::auto_detect),
        OutputFormat::Table | OutputFormat::Csv
    )
}

fn find_entry_matches(
    entry: &ObjectEntry,
    args: &FindArgs,
    size_filter: Option<FindSizeFilter>,
    mtime_filter: Option<&FindMtimeFilter>,
) -> bool {
    if let Some(pattern) = &args.name {
        if !pattern_matches(&entry.key, pattern) {
            return false;
        }
    }
    if let Some(storage_class) = &args.storage_class {
        if entry.storage_class.as_deref() != Some(storage_class.as_str()) {
            return false;
        }
    }
    if let Some(size_filter) = size_filter {
        if !find_size_matches(entry.size, size_filter) {
            return false;
        }
    }
    if let Some(mtime_filter) = mtime_filter {
        if !find_mtime_matches(entry.last_modified.as_deref(), mtime_filter) {
            return false;
        }
    }
    true
}

fn parse_find_size_filter(filter: &str) -> Result<FindSizeFilter, CliError> {
    let (operator, value) = match filter.as_bytes().first().copied() {
        Some(b'+') => (b'+', &filter[1..]),
        Some(b'-') => (b'-', &filter[1..]),
        _ => (b'=', filter),
    };
    let Some(expected) = parse_size_bytes(value) else {
        return Err(CliError::ValidationError(format!(
            "invalid --size filter '{}': expected [+|-]<number>[B|KB|MB|GB|TB]",
            filter
        )));
    };
    Ok(match operator {
        b'+' => FindSizeFilter::MinInclusive(expected),
        b'-' => FindSizeFilter::MaxInclusive(expected),
        _ => FindSizeFilter::Equal(expected),
    })
}

fn find_size_matches(size: u64, filter: FindSizeFilter) -> bool {
    match filter {
        FindSizeFilter::MinInclusive(expected) => size >= expected,
        FindSizeFilter::MaxInclusive(expected) => size <= expected,
        FindSizeFilter::Equal(expected) => size == expected,
    }
}

fn validate_size_filter(filter: &str) -> Result<(), CliError> {
    parse_find_size_filter(filter).map(|_| ())
}

fn parse_find_mtime_filter(filter: &str) -> Result<FindMtimeFilter, CliError> {
    let (operator, value) = match filter.as_bytes().first().copied() {
        Some(b'-') => (b'-', &filter[1..]),
        Some(b'+') => (b'+', &filter[1..]),
        _ => (b'=', filter),
    };
    let duration = parse_relative_duration(value).ok_or_else(|| {
        CliError::ValidationError(format!(
            "invalid --mtime filter '{}': expected [+|-]<number>[s|m|h|d] or <number>[s|m|h|d]",
            filter
        ))
    })?;
    let now = Utc::now();
    let threshold = now - duration.duration;
    Ok(match operator {
        b'-' => FindMtimeFilter::WithinLast(threshold),
        b'+' => FindMtimeFilter::OlderThanOrEqual(threshold),
        b'=' => FindMtimeFilter::EqualAge {
            newest: threshold,
            oldest_exclusive: threshold - duration.unit,
        },
        _ => unreachable!("operator is checked above"),
    })
}

fn validate_mtime_filter(filter: &str) -> Result<(), CliError> {
    parse_find_mtime_filter(filter).map(|_| ())
}

fn parse_relative_duration(value: &str) -> Option<RelativeDuration> {
    let trimmed = value.trim();
    let digits_len = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .map(char::len_utf8)
        .sum::<usize>();
    let number = trimmed.get(..digits_len)?.parse::<i64>().ok()?;
    let unit = trimmed.get(digits_len..)?.trim().to_ascii_lowercase();
    let unit_seconds = match unit.as_str() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => 1,
        "m" | "min" | "mins" | "minute" | "minutes" => 60,
        "h" | "hr" | "hrs" | "hour" | "hours" => 60 * 60,
        "d" | "day" | "days" => 24 * 60 * 60,
        _ => return None,
    };
    Some(RelativeDuration {
        duration: chrono::Duration::try_seconds(number.checked_mul(unit_seconds)?)?,
        unit: chrono::Duration::try_seconds(unit_seconds)?,
    })
}

fn find_mtime_matches(last_modified: Option<&str>, filter: &FindMtimeFilter) -> bool {
    let Some(timestamp) = parse_remote_last_modified(last_modified) else {
        return false;
    };
    match filter {
        FindMtimeFilter::WithinLast(threshold) => timestamp >= *threshold,
        FindMtimeFilter::OlderThanOrEqual(threshold) => timestamp <= *threshold,
        FindMtimeFilter::EqualAge {
            newest,
            oldest_exclusive,
        } => timestamp <= *newest && timestamp > *oldest_exclusive,
    }
}

fn normalize_http_range(range: &str) -> Result<String, CliError> {
    let trimmed = range.trim();
    let value = trimmed.strip_prefix("bytes=").unwrap_or(trimmed);
    let Some((start, end)) = value.split_once('-') else {
        return Err(CliError::ValidationError(format!(
            "invalid --range '{}': expected START-END or bytes=START-END",
            range
        )));
    };
    if start.is_empty()
        || end.is_empty()
        || !start.chars().all(|c| c.is_ascii_digit())
        || !end.chars().all(|c| c.is_ascii_digit())
    {
        return Err(CliError::ValidationError(format!(
            "invalid --range '{}': range bounds must be decimal bytes",
            range
        )));
    }
    let start_num = start.parse::<u64>().map_err(|_| {
        CliError::ValidationError(format!(
            "invalid --range '{}': start is out of range",
            range
        ))
    })?;
    let end_num = end.parse::<u64>().map_err(|_| {
        CliError::ValidationError(format!("invalid --range '{}': end is out of range", range))
    })?;
    if start_num > end_num {
        return Err(CliError::ValidationError(format!(
            "invalid --range '{}': start must be <= end",
            range
        )));
    }
    Ok(format!("bytes={}-{}", start_num, end_num))
}

fn validate_presign_method(method: &str) -> Result<&'static str, CliError> {
    match method.to_ascii_uppercase().as_str() {
        "GET" => Ok("GET"),
        "PUT" => Ok("PUT"),
        "HEAD" => Ok("HEAD"),
        "DELETE" => Ok("DELETE"),
        _ => Err(CliError::ValidationError(format!(
            "unsupported presign method '{}': expected GET, PUT, HEAD, or DELETE",
            method
        ))),
    }
}

fn parse_size_bytes(value: &str) -> Option<u64> {
    let trimmed = value.trim();
    let digits_len = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .map(char::len_utf8)
        .sum::<usize>();
    let number = trimmed.get(..digits_len)?.parse::<u64>().ok()?;
    let unit = trimmed.get(digits_len..)?.trim().to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024_u64.pow(2),
        "g" | "gb" | "gib" => 1024_u64.pow(3),
        "t" | "tb" | "tib" => 1024_u64.pow(4),
        _ => return None,
    };
    number.checked_mul(multiplier)
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.2} {}", value, UNITS[unit])
    }
}

fn pattern_allows(value: &str, include: Option<&str>, exclude: Option<&str>) -> bool {
    if let Some(pattern) = exclude {
        if pattern_matches(value, pattern) {
            return false;
        }
    }
    include
        .map(|pattern| pattern_matches(value, pattern))
        .unwrap_or(true)
}

fn pattern_matches(value: &str, pattern: &str) -> bool {
    if !pattern.contains('*') && !pattern.contains('?') {
        return value.contains(pattern);
    }
    wildcard_matches(value.as_bytes(), pattern.as_bytes())
}

fn wildcard_matches(value: &[u8], pattern: &[u8]) -> bool {
    let (mut vi, mut pi) = (0_usize, 0_usize);
    let mut star = None;
    let mut match_i = 0_usize;
    while vi < value.len() {
        if pi < pattern.len() && (pattern[pi] == b'?' || pattern[pi] == value[vi]) {
            vi += 1;
            pi += 1;
        } else if pi < pattern.len() && pattern[pi] == b'*' {
            star = Some(pi);
            match_i = vi;
            pi += 1;
        } else if let Some(star_i) = star {
            pi = star_i + 1;
            match_i += 1;
            vi = match_i;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == b'*' {
        pi += 1;
    }
    pi == pattern.len()
}

fn join_tos_key(prefix: &str, key: &str) -> String {
    match (prefix.trim_matches('/'), key.trim_start_matches('/')) {
        ("", key) => key.to_string(),
        (prefix, key) => format!("{}/{}", prefix, key),
    }
}

fn strip_tos_prefix<'a>(key: &'a str, prefix: &str) -> &'a str {
    let normalized_prefix = prefix.trim_start_matches('/');
    key.strip_prefix(normalized_prefix)
        .unwrap_or(key)
        .trim_start_matches('/')
}

/// [Review Fix #M1] Build a sibling temp path (`<file>.tos-partial-<pid>`)
/// used as the staging file during streamed downloads. Promoted to
/// `pub(crate)` so low-level handlers can reuse the same naming scheme.
pub(crate) fn partial_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    path.with_file_name(format!("{}.tos-partial-{}", file_name, std::process::id()))
}

/// [Review Fix #M4] Promoted to `pub(crate)` so low-level upload handlers
/// can pre-compute the V4 payload hash without buffering the file body.
pub(crate) fn file_sha256(path: &str) -> Result<String, CliError> {
    let mut file = File::open(path)?;
    hash_reader(&mut file).map_err(CliError::Io)
}

fn file_part_sha256(path: &str, offset: u64, size: u64) -> Result<String, CliError> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut limited = file.take(size);
    hash_reader(&mut limited).map_err(CliError::Io)
}

/// [Review Fix #M4] Promoted to `pub(crate)` for cross-module reuse.
pub(crate) async fn file_stream_body(path: &str) -> Result<Body, CliError> {
    let file = tokio::fs::File::open(path).await?;
    Ok(Body::wrap_stream(ReaderStream::new(file)))
}

async fn file_part_stream_body(path: &str, offset: u64, size: u64) -> Result<Body, CliError> {
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(SeekFrom::Start(offset)).await?;
    Ok(Body::wrap_stream(ReaderStream::new(file.take(size))))
}

/// [Review Fix #M4] Promoted to `pub(crate)` so low-level upload handlers
/// can stream a CRC64 over the file in a single pass.
pub(crate) fn file_crc64(path: &str) -> Result<u64, CliError> {
    let mut file = File::open(path)?;
    crc64_reader(&mut file, None)
}

fn file_part_crc64(path: &str, offset: u64, size: u64) -> Result<u64, CliError> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut limited = file.take(size);
    crc64_reader(&mut limited, None)
}

/// [Review Fix #M1] Streaming response writer: pulls chunks via
/// `response.chunk()` and writes them to `temp_path`, returning the total
/// number of bytes written. Callers are responsible for atomically renaming
/// `temp_path` to its final destination once the stream completes
/// successfully. Exposed at crate visibility so that low-level handlers
/// (e.g. `object download`) can share the same streaming pipeline as the
/// high-level `cp` flow.
pub(crate) async fn write_response_stream(
    response: &mut Response,
    temp_path: &Path,
) -> Result<u64, CliError> {
    write_response_stream_with_mode(response, temp_path, false).await
}

async fn write_response_stream_with_mode(
    response: &mut Response,
    temp_path: &Path,
    append: bool,
) -> Result<u64, CliError> {
    let mut file = if append {
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(temp_path)
            .await?
    } else {
        tokio::fs::File::create(temp_path).await?
    };
    let mut bytes_written = 0_u64;
    while let Some(chunk) = response.chunk().await.map_err(CliError::Http)? {
        file.write_all(&chunk).await?;
        bytes_written += chunk.len() as u64;
    }
    file.flush().await?;
    Ok(bytes_written)
}

/// [Review Fix #M1] Streaming stdout writer used by `ve-tos cat` and `ve-tos object
/// download -`. Promoted to `pub(crate)` for cross-module reuse.
pub(crate) async fn stream_response_to_stdout(response: &mut Response) -> Result<(), CliError> {
    let mut stdout = std::io::stdout().lock();
    if response.content_length() == Some(0) {
        stdout.flush()?;
        return Ok(());
    }
    while let Some(chunk) = response.chunk().await.map_err(CliError::Http)? {
        stdout.write_all(&chunk)?;
    }
    stdout.flush()?;
    Ok(())
}

fn persist_downloaded_file(
    temp_path: &Path,
    destination_path: &Path,
    force: bool,
) -> Result<(), CliError> {
    if force {
        if destination_path.exists() {
            fs::remove_file(destination_path)?;
        }
        fs::rename(temp_path, destination_path)?;
        return Ok(());
    }

    match fs::hard_link(temp_path, destination_path) {
        Ok(()) => {
            fs::remove_file(temp_path)?;
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = fs::remove_file(temp_path);
            Err(CliError::Conflict(format!(
                "local destination '{}' was created during download; pass --force to overwrite",
                destination_path.display()
            )))
        }
        Err(err) => {
            let _ = fs::remove_file(temp_path);
            Err(CliError::Io(err))
        }
    }
}

fn should_use_multipart(
    file_size: u64,
    checkpoint_enabled: bool,
    checkpoint_threshold: u64,
) -> bool {
    matches!(
        UploadStrategy::auto_select(file_size),
        UploadStrategy::Multipart { .. }
    ) || (checkpoint_enabled && file_size >= checkpoint_threshold)
}

fn progress_units_for_size(
    file_size: u64,
    runtime: TransferRuntimeConfig,
    checkpoint_enabled: bool,
) -> u64 {
    if runtime.progress_granularity == EffectiveProgressGranularity::Byte {
        return file_size.max(1);
    }
    if should_use_multipart(file_size, checkpoint_enabled, runtime.checkpoint_threshold) {
        file_size.div_ceil(multipart_part_size(file_size)).max(1)
    } else {
        1
    }
}

fn multipart_part_size(file_size: u64) -> u64 {
    match UploadStrategy::auto_select(file_size) {
        UploadStrategy::Multipart { part_size } => part_size,
        _ => 20 * 1024 * 1024,
    }
}

fn checkpoint_path(
    checkpoint_dir: Option<&str>,
    source: &str,
    bucket: &str,
    key: &str,
    file_size: u64,
    file_mtime: u128,
    part_size: u64,
    write_context: &str,
    profile: &str,
    endpoint: &str,
) -> Result<PathBuf, CliError> {
    checkpoint_path_for_surface(
        top_level_storage_surface(),
        checkpoint_dir,
        source,
        bucket,
        key,
        file_size,
        file_mtime,
        part_size,
        write_context,
        profile,
        endpoint,
    )
}

fn checkpoint_path_for_surface(
    surface: &str,
    checkpoint_dir: Option<&str>,
    source: &str,
    bucket: &str,
    key: &str,
    file_size: u64,
    file_mtime: u128,
    part_size: u64,
    write_context: &str,
    profile: &str,
    endpoint: &str,
) -> Result<PathBuf, CliError> {
    let mut hasher = DefaultHasher::new();
    // [Review Fix #13] Checkpoint identity must change when file/profile/endpoint/write semantics change.
    // [Review Fix #12] The top-level command is part of the identity because
    // ByteCloud `tos` and legacy `ve-tos` can target the same bucket/key with
    // different credentials, endpoints, and signing semantics.
    surface.hash(&mut hasher);
    source.hash(&mut hasher);
    bucket.hash(&mut hasher);
    key.hash(&mut hasher);
    file_size.hash(&mut hasher);
    file_mtime.hash(&mut hasher);
    part_size.hash(&mut hasher);
    write_context.hash(&mut hasher);
    profile.hash(&mut hasher);
    endpoint.hash(&mut hasher);
    let checkpoint_dir = checkpoint_dir.map(ToString::to_string).unwrap_or_else(|| {
        scoped_default_path(DEFAULT_TOS_CHECKPOINT_DIR, DEFAULT_TOS_CHECKPOINT_DIR)
    });
    let dir = expand_user_path(&checkpoint_dir);
    fs::create_dir_all(&dir)?;
    Ok(dir.join(format!("cp-upload-{:016x}.json", hasher.finish())))
}

fn file_mtime_nanos(metadata: &fs::Metadata) -> Result<u128, CliError> {
    Ok(metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|err| CliError::ValidationError(format!("invalid file mtime: {}", err)))?
        .as_nanos())
}

fn expand_user_path(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn load_checkpoint(path: &Path) -> Result<Option<Checkpoint>, CliError> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    serde_json::from_str(&content)
        .map(Some)
        .map_err(CliError::Json)
}

fn save_checkpoint(path: &Path, checkpoint: &Checkpoint) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = path.with_extension("json.tmp");
    fs::write(
        &temp_path,
        serde_json::to_vec_pretty(checkpoint).map_err(CliError::Json)?,
    )?;
    fs::rename(temp_path, path)?;
    Ok(())
}

fn remove_checkpoint(path: &Path) -> Result<(), CliError> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn extract_upload_id(response: &Envelope<core::RawResponseData>) -> Option<String> {
    response
        .data
        .as_ref()
        .and_then(|data| data.body.as_ref())
        .and_then(|body| {
            json_string(body, &["upload_id", "UploadId", "uploadId", "uploadID"])
                .or_else(|| json_string_deep(body, "upload_id"))
        })
}

fn extract_part_copy_etag(response: &Envelope<core::RawResponseData>) -> Option<String> {
    response.data.as_ref().and_then(|data| {
        upload_part_etag(&data.headers).or_else(|| {
            data.body.as_ref().and_then(|body| {
                json_string_deep(body, "etag").or_else(|| json_string_deep(body, "e_tag"))
            })
        })
    })
}

fn upload_part_etag(headers: &BTreeMap<String, String>) -> Option<String> {
    header_value(headers, &["etag", "x-tos-etag"]).map(|etag| etag.trim_matches('"').to_string())
}

fn header_value(headers: &BTreeMap<String, String>, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        headers.get(*name).cloned().or_else(|| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.clone())
        })
    })
}

fn json_string_deep(value: &Value, name: &str) -> Option<String> {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                if key.eq_ignore_ascii_case(name) {
                    if let Some(value) = child.as_str() {
                        return Some(value.to_string());
                    }
                }
                if let Some(value) = json_string_deep(child, name) {
                    return Some(value);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(|item| json_string_deep(item, name)),
        _ => None,
    }
}

fn remote_copy_checkpoint_identity(
    source: &str,
    source_etag: &str,
    source_head: &Envelope<core::RawResponseData>,
) -> u128 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    source_etag.hash(&mut hasher);
    source_head
        .data
        .as_ref()
        .and_then(|data| data.headers.get("last-modified"))
        .hash(&mut hasher);
    hasher.finish() as u128
}

struct CheckpointLock {
    path: PathBuf,
}

impl CheckpointLock {
    fn acquire(checkpoint_path: &Path) -> Result<Self, CliError> {
        let lock_path = checkpoint_path.with_extension("json.lock");
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                writeln!(file, "pid={}", std::process::id())?;
                Ok(Self { path: lock_path })
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Err(CliError::Conflict(
                format!("checkpoint is locked: {}", lock_path.display()),
            )),
            Err(err) => Err(CliError::Io(err)),
        }
    }
}

impl Drop for CheckpointLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn find_crc64_header(headers: &BTreeMap<String, String>) -> Option<u64> {
    ["x-tos-hash-crc64ecma", "x-hash-crc64ecma", "x-tos-crc64"]
        .iter()
        .find_map(|name| {
            headers
                .get(*name)
                .and_then(|value| value.parse::<u64>().ok())
        })
}

const TOS_REPORT_COLUMNS: &[&str] = &[
    "command",
    "operation",
    "source",
    "destination",
    "status",
    "error_kind",
    "error_code",
    "message",
    "retryable",
];

struct RollingCsvWriter {
    base_path: PathBuf,
    header_line: String,
    current_file: File,
    current_part: usize,
    current_bytes: u64,
    max_bytes: u64,
}

impl RollingCsvWriter {
    fn new(base_path: &str, headers: &'static [&'static str]) -> Result<Self, CliError> {
        let base_path = PathBuf::from(base_path);
        if let Some(parent) = base_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let header_line = csv_record(headers.iter().copied());
        let (current_file, current_bytes) = Self::open_part(&base_path, 1, &header_line, true)?;
        Ok(Self {
            base_path,
            header_line,
            current_file,
            current_part: 1,
            current_bytes,
            max_bytes: DEFAULT_BATCH_FILE_ROLLOVER_BYTES,
        })
    }

    fn write_record(&mut self, fields: &[String]) -> Result<(), CliError> {
        let line = csv_record(fields.iter().map(String::as_str));
        let line_bytes = line.len() as u64;
        if self.current_bytes > self.header_line.len() as u64
            && self.current_bytes.saturating_add(line_bytes) > self.max_bytes
        {
            self.current_part += 1;
            let (file, bytes) =
                Self::open_part(&self.base_path, self.current_part, &self.header_line, true)?;
            self.current_file = file;
            self.current_bytes = bytes;
        }
        self.current_file.write_all(line.as_bytes())?;
        self.current_bytes += line_bytes;
        Ok(())
    }

    fn open_part(
        base_path: &Path,
        part: usize,
        header_line: &str,
        truncate: bool,
    ) -> Result<(File, u64), CliError> {
        let part_path = rolled_csv_path(base_path, part);
        if let Some(parent) = part_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).write(true);
        if truncate {
            options.truncate(true);
        } else {
            options.append(true);
        }
        let mut file = options.open(&part_path)?;
        let mut bytes = file.metadata()?.len();
        if bytes == 0 {
            file.write_all(header_line.as_bytes())?;
            bytes = header_line.len() as u64;
        }
        Ok((file, bytes))
    }
}

fn rolled_csv_path(base_path: &Path, part: usize) -> PathBuf {
    if part == 1 {
        return base_path.to_path_buf();
    }
    let stem = base_path
        .file_stem()
        .or_else(|| base_path.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("report");
    base_path.with_file_name(format!("{stem}.part-{part:04}.csv"))
}

fn csv_record<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let mut record = values
        .into_iter()
        .map(csv_escape)
        .collect::<Vec<_>>()
        .join(",");
    record.push('\n');
    record
}

fn csv_escape(value: &str) -> String {
    if value.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn append_tos_report_record(report_path: &str, fields: &[String]) -> Result<(), CliError> {
    // [Review Fix #1] Legacy per-item report hooks must also write CSV part files;
    // otherwise batch commands can leave stale JSONL beside the final CSV report.
    let header_line = csv_record(TOS_REPORT_COLUMNS.iter().copied());
    let (mut file, _) =
        RollingCsvWriter::open_part(Path::new(report_path), 1, &header_line, false)?;
    let line = csv_record(fields.iter().map(String::as_str));
    file.write_all(line.as_bytes())?;
    Ok(())
}

fn tos_report_record(command: &str, item: &BatchItemResult) -> [String; 9] {
    [
        command.to_string(),
        item.operation.clone(),
        item.source.clone(),
        item.destination.clone().unwrap_or_default(),
        item.status.to_string(),
        item.error_kind.clone().unwrap_or_default(),
        item.error_code
            .map(|value| value.to_string())
            .unwrap_or_default(),
        item.message.clone().unwrap_or_default(),
        item.retryable.to_string(),
    ]
}

fn write_single_report(
    report_path: Option<&str>,
    operation: &str,
    source: &str,
    destination: Option<&str>,
    status: &str,
) -> Result<(), CliError> {
    let Some(report_path) = report_path else {
        return Ok(());
    };
    append_tos_report_record(
        report_path,
        &[
            String::new(),
            operation.to_string(),
            source.to_string(),
            destination.unwrap_or_default().to_string(),
            status.to_string(),
            String::new(),
            String::new(),
            String::new(),
            "false".to_string(),
        ],
    )
}

fn write_manifest_file(
    manifest_path: Option<&str>,
    command: &str,
    manifest: &TransferManifest,
) -> Result<(), CliError> {
    let Some(manifest_path) = manifest_path else {
        return Ok(());
    };
    let mut writer = RollingCsvWriter::new(
        manifest_path,
        &[
            "command",
            "operation",
            "relative_key",
            "source",
            "destination",
            "size",
            "etag",
            "crc64",
            "last_modified",
        ],
    )?;
    for item in &manifest.items {
        writer.write_record(&[
            command.to_string(),
            item.operation.to_string(),
            item.relative_key.clone(),
            item.source.clone(),
            item.destination.clone().unwrap_or_default(),
            item.size.to_string(),
            item.etag.clone().unwrap_or_default(),
            item.crc64
                .map(|value| value.to_string())
                .unwrap_or_default(),
            item.last_modified.clone().unwrap_or_default(),
        ])?;
    }
    Ok(())
}

fn write_list_manifest_file(
    manifest_path: Option<&str>,
    command: &str,
    manifest: &ListManifest,
) -> Result<(), CliError> {
    let Some(manifest_path) = manifest_path else {
        return Ok(());
    };
    let mut writer = RollingCsvWriter::new(
        manifest_path,
        &[
            "command",
            "item_type",
            "source",
            "relative_key",
            "size",
            "etag",
            "last_modified",
            "storage_class",
            "version_id",
        ],
    )?;
    for item in &manifest.items {
        writer.write_record(&[
            command.to_string(),
            item.item_type.to_string(),
            item.source.clone(),
            item.relative_key.clone().unwrap_or_default(),
            item.size.to_string(),
            item.etag.clone().unwrap_or_default(),
            item.last_modified.clone().unwrap_or_default(),
            item.storage_class.clone().unwrap_or_default(),
            item.version_id.clone().unwrap_or_default(),
        ])?;
    }
    Ok(())
}

fn write_tos_batch_report(
    report_path: Option<&str>,
    command: &str,
    report: &BatchReport,
    failures_only: bool,
) -> Result<(), CliError> {
    let Some(report_path) = report_path else {
        return Ok(());
    };
    let mut writer = RollingCsvWriter::new(report_path, TOS_REPORT_COLUMNS)?;
    if !failures_only {
        for item in &report.succeeded {
            writer.write_record(&tos_report_record(command, item))?;
        }
        for item in &report.skipped {
            writer.write_record(&tos_report_record(command, item))?;
        }
    }
    for item in &report.failed {
        writer.write_record(&tos_report_record(command, item))?;
    }
    Ok(())
}

fn build_list_manifest(items: Vec<ListManifestItem>) -> ListManifest {
    let object_count = items
        .iter()
        .filter(|item| item.item_type == "object" || item.item_type == "version")
        .count() as u64;
    let directory_count = items
        .iter()
        .filter(|item| item.item_type == "directory")
        .count() as u64;
    let total_size = items.iter().map(|item| item.size).sum();
    ListManifest {
        item_count: items.len() as u64,
        object_count,
        directory_count,
        total_size,
        items,
    }
}

fn object_entry_manifest_item(
    bucket: &str,
    root_prefix: Option<&str>,
    entry: &ObjectEntry,
) -> ListManifestItem {
    ListManifestItem {
        source: format!("tos://{}/{}", bucket, entry.key),
        relative_key: Some(strip_tos_prefix(&entry.key, root_prefix.unwrap_or("")).to_string()),
        item_type: "object",
        size: entry.size,
        etag: entry.etag.clone(),
        last_modified: entry.last_modified.clone(),
        storage_class: entry.storage_class.clone(),
        version_id: None,
    }
}

fn key_manifest_item(bucket: &str, root_prefix: Option<&str>, key: &str) -> ListManifestItem {
    ListManifestItem {
        source: format!("tos://{}/{}", bucket, key),
        relative_key: Some(strip_tos_prefix(key, root_prefix.unwrap_or("")).to_string()),
        item_type: "object",
        size: 0,
        etag: None,
        last_modified: None,
        storage_class: None,
        version_id: None,
    }
}

fn directory_manifest_item(
    bucket: &str,
    root_prefix: Option<&str>,
    prefix: &str,
) -> ListManifestItem {
    ListManifestItem {
        source: format!("tos://{}/{}", bucket, prefix),
        relative_key: Some(strip_tos_prefix(prefix, root_prefix.unwrap_or("")).to_string()),
        item_type: "directory",
        size: 0,
        etag: None,
        last_modified: None,
        storage_class: None,
        version_id: None,
    }
}

fn bucket_manifest_item(bucket: &bucket::BucketInfo) -> ListManifestItem {
    ListManifestItem {
        source: format!("tos://{}", bucket.name),
        relative_key: Some(bucket.name.clone()),
        item_type: "bucket",
        size: 0,
        etag: None,
        last_modified: Some(bucket.creation_date.clone()),
        storage_class: None,
        version_id: None,
    }
}

fn version_manifest_item(
    bucket: &str,
    root_prefix: Option<&str>,
    version: &ObjectVersionRef,
) -> ListManifestItem {
    ListManifestItem {
        source: version_uri(bucket, version),
        relative_key: Some(strip_tos_prefix(&version.key, root_prefix.unwrap_or("")).to_string()),
        item_type: "version",
        size: 0,
        etag: None,
        last_modified: None,
        storage_class: None,
        version_id: Some(version.version_id.clone()),
    }
}

fn write_error_report(
    report_path: Option<&str>,
    operation: &str,
    source: &str,
    destination: Option<&str>,
    err: &CliError,
) -> Result<(), CliError> {
    let Some(report_path) = report_path else {
        return Ok(());
    };
    append_tos_report_record(
        report_path,
        &[
            String::new(),
            operation.to_string(),
            source.to_string(),
            destination.unwrap_or_default().to_string(),
            "failed".to_string(),
            error_kind(err),
            err.exit_code().as_i32().to_string(),
            err.to_string(),
            is_retryable_error(err).to_string(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ],
    )
}

fn error_kind(err: &CliError) -> String {
    match err {
        CliError::Unknown(_) => "unknown",
        CliError::AuthFailed(_) => "auth_failed",
        CliError::ConfigMissing(_) => "config_missing",
        CliError::ResourceNotFound(_) => "resource_not_found",
        CliError::PermissionDenied(_) => "permission_denied",
        CliError::ValidationError(_) => "validation_error",
        CliError::RateLimited(_) => "rate_limited",
        CliError::TransferFailed(_) => "transfer_failed",
        CliError::Conflict(_) => "conflict",
        CliError::Http(_) => "http_error",
        CliError::Io(_) => "io_error",
        CliError::Json(_) => "json_error",
    }
    .to_string()
}

fn is_retryable_error(err: &CliError) -> bool {
    matches!(
        err,
        CliError::RateLimited(_) | CliError::TransferFailed(_) | CliError::Http(_)
    )
}

// [Review Fix #CRC64-XZ] 完整对齐 Go 标准库 hash/crc64 + crc64.MakeTable(crc64.ECMA)
// 的实际行为（即 CRC-64/XZ 变体）：
//   * Reflected polynomial = 0xC96C5795D7870F42 （0x42F0E1EBA9EA3693 的位反转）
//   * RefIn = RefOut = true
//   * Init = 0xFFFFFFFFFFFFFFFF（Go update() 入口的 `crc = ^crc`）
//   * XorOut = 0xFFFFFFFFFFFFFFFF（Go update() 出口的 `return ^crc`）
// 由 `crc64_reader` / `crc64_ecma` 在边界处统一处理 init/xor 反转，
// 让 `crc64_update` 保持为纯增量更新器，方便分片场景拼接。
// 此修复对齐 TOS 服务端复算 x-hash-crc64ecma 的方式（见 ve-tos-golang-sdk/tos/consts.go:
// DefaultCrcTable = crc64.MakeTable(crc64.ECMA)），消除既往 PUT 请求触发的 HTTP 400 [BadDigest]。
#[cfg(test)]
fn crc64_ecma(bytes: &[u8]) -> u64 {
    let mut crc = !0_u64;
    crc64_update(&mut crc, bytes);
    !crc
}

fn crc64_reader<R: IoRead>(reader: &mut R, limit: Option<u64>) -> Result<u64, CliError> {
    let mut crc = !0_u64;
    let mut remaining = limit.unwrap_or(u64::MAX);
    let mut buffer = [0_u8; 1024 * 1024];
    while remaining > 0 {
        let read_len = buffer.len().min(remaining as usize);
        let bytes_read = reader.read(&mut buffer[..read_len])?;
        if bytes_read == 0 {
            break;
        }
        crc64_update(&mut crc, &buffer[..bytes_read]);
        remaining -= bytes_read as u64;
    }
    Ok(!crc)
}

fn crc64_update(crc: &mut u64, bytes: &[u8]) {
    const POLY_REFLECTED: u64 = 0xC96C_5795_D787_0F42;
    for byte in bytes {
        *crc ^= *byte as u64;
        for _ in 0..8 {
            if (*crc & 1) != 0 {
                *crc = (*crc >> 1) ^ POLY_REFLECTED;
            } else {
                *crc >>= 1;
            }
        }
    }
}

fn validate_transfer_pair(
    source: &str,
    destination: &str,
    allow_source_bucket_only: bool,
) -> Result<(), CliError> {
    if source.trim().is_empty() || destination.trim().is_empty() {
        return Err(CliError::ValidationError(
            "source and destination must not be empty".to_string(),
        ));
    }
    if source == destination {
        return Err(CliError::ValidationError(
            "source and destination must be different".to_string(),
        ));
    }
    if source.starts_with("tos://") {
        validate_tos_uri(source, allow_source_bucket_only)?;
    }
    if destination.starts_with("tos://") {
        validate_tos_uri(destination, true)?;
    }
    Ok(())
}

fn validate_bucket_target(bucket: &str) -> Result<(), CliError> {
    parse_bucket_target(bucket).map(|_| ())
}

fn validate_tos_uri(uri: &str, allow_bucket_only: bool) -> Result<(), CliError> {
    if !uri.starts_with("tos://") {
        return Err(CliError::ValidationError(format!(
            "invalid TOS URI '{}': expected tos://bucket/key",
            uri
        )));
    }
    let rest = uri.trim_start_matches("tos://");
    let mut parts = rest.splitn(2, '/');
    let bucket = parts.next().unwrap_or_default();
    let key = parts.next().unwrap_or_default();
    if bucket.is_empty() || (!allow_bucket_only && key.is_empty()) {
        return Err(CliError::ValidationError(format!(
            "invalid TOS URI '{}': expected {}",
            uri,
            if allow_bucket_only {
                "tos://bucket or tos://bucket/key"
            } else {
                "tos://bucket/key"
            }
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("tos-high-level-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    fn rm_args_with_mode(mode: Option<RecursiveDeleteMode>) -> RmArgs {
        RmArgs {
            path: Some("tos://bucket/prefix/".to_string()),
            bucket: None,
            key: None,
            recursive: true,
            recursive_delete_mode: mode,
            force: true,
            all_versions: false,
            include_uploads: false,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            batch_concurrency: None,
            list_concurrency: None,
            recursive_list_mode: None,
            include: None,
            exclude: None,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
        }
    }

    fn test_plan_item(source: &str) -> TransferPlanItem {
        TransferPlanItem {
            relative_key: source.to_string(),
            source: source.to_string(),
            destination: "tos://bucket/dst".to_string(),
            size: 0,
            etag: None,
            crc64: None,
            last_modified: None,
        }
    }

    fn cp_args(source: &str, destination: &str, recursive: bool) -> CpArgs {
        CpArgs {
            source: source.to_string(),
            destination: destination.to_string(),
            recursive,
            include_parent: false,
            include: None,
            exclude: None,
            checkpoint: false,
            checkpoint_dir: None,
            content_type: None,
            storage_class: None,
            acl: None,
            meta: None,
            checkpoint_threshold: None,
            batch_concurrency: None,
            list_concurrency: None,
            recursive_list_mode: None,
            multipart_concurrency: None,
            progress_granularity: None,
            overwrite_strategy: None,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            bandwidth_limit: None,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
            force: false,
            no_clobber: false,
        }
    }

    fn mv_args(source: &str, destination: &str, recursive: bool) -> MvArgs {
        MvArgs {
            source: source.to_string(),
            destination: destination.to_string(),
            recursive,
            include_parent: false,
            include: None,
            exclude: None,
            checkpoint_dir: None,
            content_type: None,
            storage_class: None,
            acl: None,
            meta: None,
            checkpoint_threshold: None,
            batch_concurrency: None,
            list_concurrency: None,
            recursive_list_mode: None,
            multipart_concurrency: None,
            progress_granularity: None,
            overwrite_strategy: None,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
            force: true,
        }
    }

    fn sync_args(source: &str, destination: &str) -> SyncArgs {
        SyncArgs {
            source: source.to_string(),
            destination: destination.to_string(),
            delete: false,
            force: false,
            size_only: false,
            exact_timestamps: false,
            include_parent: false,
            include: None,
            exclude: None,
            checkpoint_dir: None,
            content_type: None,
            storage_class: None,
            acl: None,
            meta: None,
            checkpoint_threshold: None,
            batch_concurrency: None,
            list_concurrency: None,
            recursive_list_mode: None,
            multipart_concurrency: None,
            progress_granularity: None,
            overwrite_strategy: None,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            bandwidth_limit: None,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
        }
    }

    fn du_args(max_depth: Option<u32>) -> DuArgs {
        DuArgs {
            path: Some("tos://bucket/root/".to_string()),
            bucket: None,
            key: None,
            human_readable: false,
            max_depth,
            top_k: 10,
            cost: false,
            storage_price: Vec::new(),
            manifest_path: None,
            list_concurrency: None,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
        }
    }

    fn object_entry(key: &str) -> ObjectEntry {
        ObjectEntry {
            key: key.to_string(),
            size: 0,
            last_modified: None,
            etag: None,
            storage_class: None,
        }
    }

    fn sync_delete_item(source: &str) -> TransferManifestItem {
        TransferManifestItem {
            operation: "delete-extra",
            relative_key: source
                .strip_prefix("tos://bucket/")
                .unwrap_or(source)
                .to_string(),
            source: source.to_string(),
            destination: None,
            size: 0,
            etag: None,
            crc64: None,
            last_modified: None,
        }
    }

    #[test]
    fn test_bucket_root_source_validation_depends_on_recursive_mode() {
        let non_recursive = match cp_operation(&cp_args("tos://bucket", "tos://dst/prefix/", false))
        {
            Ok(_) => panic!("single-object cp must require a source key"),
            Err(err) => err,
        };
        assert!(non_recursive
            .to_string()
            .contains("expected tos://bucket/key"));

        assert!(cp_operation(&cp_args("tos://bucket", "tos://dst/prefix/", true)).is_ok());
        assert!(mv_operation(&mv_args("tos://bucket", "tos://dst/prefix/", true)).is_ok());
        assert!(sync_operation(&sync_args("tos://bucket", "tos://dst/prefix/")).is_ok());
    }

    #[test]
    fn test_single_transfer_payload_keeps_service_response_fields() {
        let result = CopyTransferResult {
            outcome: CopyOutcome::Transferred,
            response_data: Some(json!({
                "status_code": 200,
                "headers": {
                    "etag": "\"abc\"",
                },
            })),
            request_id: Some("req-1".to_string()),
            status_code: Some(200),
            ec: None,
        };
        let envelope = single_transfer_envelope(
            "ve-tos cp",
            "upload",
            "1.txt",
            "tos://xsj-fns-test/1.txt",
            result,
        );
        let payload = envelope.data.as_ref().expect("payload");

        assert_eq!(envelope.status_code, Some(200));
        assert_eq!(envelope.request_id.as_deref(), Some("req-1"));
        assert_eq!(payload["operation"], "upload");
        assert_eq!(payload["source"], "1.txt");
        assert_eq!(payload["destination"], "tos://xsj-fns-test/1.txt");
        assert_eq!(payload["status"], "succeeded");
        assert_eq!(payload["status_code"], 200);
        assert_eq!(payload["headers"]["etag"], "\"abc\"");
    }

    #[test]
    fn test_recursive_include_parent_adds_source_directory_prefix() {
        let dir = temp_dir("recursive-include-parent");
        let source = dir.join("folder").join("subfolder");
        fs::create_dir_all(&source).expect("create source");
        fs::write(source.join("README.md"), "demo").expect("write file");

        let without_parent = build_local_source_mappings(
            source.to_str().expect("source"),
            "tos://bucket/test",
            false,
        )
        .expect("default mappings");
        assert_eq!(without_parent[0].relative_key, "README.md");
        assert_eq!(without_parent[0].destination, "tos://bucket/test/README.md");

        let with_parent = build_local_source_mappings(
            source.to_str().expect("source"),
            "tos://bucket/test",
            true,
        )
        .expect("include-parent mappings");
        assert_eq!(with_parent[0].relative_key, "subfolder/README.md");
        assert_eq!(
            with_parent[0].destination,
            "tos://bucket/test/subfolder/README.md"
        );
    }

    #[test]
    fn test_recursive_tos_prefix_is_directory_bounded() {
        assert_eq!(normalize_recursive_tos_prefix(None), "");
        assert_eq!(normalize_recursive_tos_prefix(Some("")), "");
        assert_eq!(normalize_recursive_tos_prefix(Some("folder")), "folder/");
        assert_eq!(normalize_recursive_tos_prefix(Some("folder/")), "folder/");
        assert_eq!(
            normalize_recursive_tos_prefix(Some("folder/sub")),
            "folder/sub/"
        );
    }

    #[test]
    fn test_recursive_include_parent_uses_normalized_tos_prefix() {
        assert_eq!(
            recursive_source_parent_prefix("tos://bucket/folder", true).expect("include parent"),
            Some("folder".to_string())
        );
        assert_eq!(
            recursive_source_parent_prefix("tos://bucket/folder/sub", true)
                .expect("include parent"),
            Some("sub".to_string())
        );
    }

    #[test]
    fn test_recursive_tos_download_preserves_directory_markers() {
        let source_target = parse_tos_uri("tos://bucket/xsj/", true).expect("source uri");
        let entries = vec![
            ObjectEntry {
                key: "xsj/".to_string(),
                size: 0,
                last_modified: None,
                etag: None,
                storage_class: None,
            },
            ObjectEntry {
                key: "xsj/sub/".to_string(),
                size: 0,
                last_modified: None,
                etag: None,
                storage_class: None,
            },
            ObjectEntry {
                key: "xsj/sub/file.txt".to_string(),
                size: 42,
                last_modified: None,
                etag: Some("etag-file".to_string()),
                storage_class: None,
            },
        ];

        let planned =
            build_tos_source_transfer_items(&source_target, "xsj/", "temp", None, entries)
                .expect("transfer items");

        assert_eq!(planned.len(), 3);
        assert_eq!(planned[0].relative_key, "");
        assert_eq!(planned[0].source, "tos://bucket/xsj/");
        assert!(planned[0]
            .destination
            .trim_end_matches(std::path::MAIN_SEPARATOR)
            .ends_with("temp"));
        assert_eq!(planned[1].relative_key, "sub/");
        assert_eq!(planned[1].source, "tos://bucket/xsj/sub/");
        assert!(planned[1]
            .destination
            .trim_end_matches(std::path::MAIN_SEPARATOR)
            .ends_with(&format!("temp{}sub", std::path::MAIN_SEPARATOR)));
        assert_eq!(planned[2].relative_key, "sub/file.txt");
        assert_eq!(planned[2].source, "tos://bucket/xsj/sub/file.txt");
        assert!(planned[2].destination.ends_with(&format!(
            "temp{}sub{}file.txt",
            std::path::MAIN_SEPARATOR,
            std::path::MAIN_SEPARATOR
        )));
    }

    #[test]
    fn test_tos_cp_trailing_slash_source_is_directory_marker_download() {
        assert!(cp_operation(&cp_args("tos://bucket/xsj/", "temp/", false)).is_ok());
        assert!(cp_operation(&cp_args("tos://bucket/xsj/", "temp/", true)).is_ok());
        assert!(is_tos_directory_marker_source("tos://bucket/xsj/").expect("directory marker"));
        assert!(
            !is_tos_directory_marker_source("tos://bucket/xsj/file.txt").expect("regular object")
        );
    }

    #[test]
    fn test_tos_directory_marker_download_creates_local_directory() {
        let root = temp_dir("tos-dir-marker-download");
        let destination = root.join("xsj");

        create_local_directory_marker_destination(&destination).expect("mkdir marker destination");

        assert!(destination.is_dir());
    }

    #[test]
    fn test_hns_recursive_mv_rename_plan_normalizes_prefixes() {
        let plan = build_tos_recursive_rename_plan("tos://bucket/pip", "tos://bucket/ppp", false)
            .expect("rename plan")
            .expect("same bucket plan");

        assert_eq!(plan.bucket, "bucket");
        assert_eq!(plan.source_key, "pip/");
        assert_eq!(plan.destination_key, "ppp/");
        assert_eq!(plan.source_uri, "tos://bucket/pip/");
        assert_eq!(plan.destination_uri, "tos://bucket/ppp/");
    }

    #[test]
    fn test_hns_recursive_mv_rename_plan_supports_include_parent() {
        let plan = build_tos_recursive_rename_plan(
            "tos://bucket/folder/subfolder",
            "tos://bucket/test",
            true,
        )
        .expect("rename plan")
        .expect("same bucket plan");

        assert_eq!(plan.source_key, "folder/subfolder/");
        assert_eq!(plan.destination_key, "test/subfolder/");
    }

    #[test]
    fn test_hns_recursive_mv_rename_plan_rejects_self_destination() {
        let err = build_tos_recursive_rename_plan(
            "tos://bucket/folder",
            "tos://bucket/folder/child",
            false,
        )
        .expect_err("self move should fail");

        assert!(err
            .to_string()
            .contains("destination must not be inside the source prefix"));
    }

    #[test]
    fn test_hns_recursive_mv_rename_plan_skips_cross_bucket_or_bucket_root() {
        assert!(build_tos_recursive_rename_plan(
            "tos://source/pip",
            "tos://destination/ppp",
            false,
        )
        .expect("cross-bucket")
        .is_none());
        assert!(
            build_tos_recursive_rename_plan("tos://bucket/pip", "tos://bucket", false)
                .expect("bucket root destination")
                .is_none()
        );
    }

    #[test]
    fn test_restore_request_body_matches_service_schema() {
        let body = restore_request_body(None, None).expect("restore body");
        let json: Value = serde_json::from_slice(&body).expect("restore json body");
        assert_eq!(json["Days"], 1);
        assert!(json.get("GlacierJobParameters").is_none());

        let body = restore_request_body(Some(3), Some("bulk")).expect("restore body with tier");
        let json: Value = serde_json::from_slice(&body).expect("restore json body");
        assert_eq!(json["Days"], 3);
        assert_eq!(json["RestoreJobParameters"]["Tier"], "Bulk");
    }

    #[test]
    fn test_high_level_success_envelope_uses_active_public_command() {
        let envelope = high_level_success_envelope_for_binary(
            Binary::Tos,
            "ve-tos ls",
            json!({"bucket": "bucket"}),
        );

        assert_eq!(envelope.command, "tos ls");
        assert_eq!(envelope.data.unwrap()["bucket"], "bucket");
    }

    #[test]
    fn test_ve_tos_du_always_uses_hierarchical_listing() {
        assert!(resolve_tos_du_list_mode_for_binary(Binary::VeTos, false));
        assert!(resolve_tos_du_list_mode_for_binary(Binary::VeTos, true));
    }

    #[test]
    fn test_retag_success_envelope_preserves_payload_and_request_id() {
        let envelope = Envelope::success("ve-tos bucket create", json!({"bucket": "bucket"}))
            .with_request_id("req-1");

        let retagged = retag_success_envelope(envelope, "ve-tos mb");

        assert_eq!(retagged.command, "ve-tos mb");
        assert_eq!(retagged.request_id.as_deref(), Some("req-1"));
        assert_eq!(retagged.data.unwrap()["bucket"], "bucket");
    }

    #[test]
    fn test_tos_put_success_payload_includes_pipe_friendly_fields() {
        let mut headers = BTreeMap::new();
        headers.insert("etag".to_string(), "\"etag-1\"".to_string());
        headers.insert("x-tos-hash-crc64ecma".to_string(), "12345".to_string());
        let raw = core::RawResponseData {
            status_code: 200,
            headers,
            body_format: None,
            body: None,
        };

        let payload = tos_put_success_payload("stdin-upload", "tos://bucket/key", 42, &raw);

        assert_eq!(payload["operation"], "stdin-upload");
        assert_eq!(payload["destination"], "tos://bucket/key");
        assert_eq!(payload["bytes"], 42);
        assert_eq!(payload["etag"], "\"etag-1\"");
        assert_eq!(payload["crc64"], "12345");
        assert_eq!(payload["status"], "succeeded");
        assert_eq!(payload["response"]["status_code"], 200);
    }

    #[test]
    fn test_resolve_recursive_delete_mode_is_hns_only() {
        let default_args = rm_args_with_mode(None);
        assert_eq!(
            resolve_tos_recursive_delete_mode(false, &default_args).unwrap(),
            None
        );
        assert_eq!(
            resolve_tos_recursive_delete_mode(true, &default_args).unwrap(),
            Some(RecursiveDeleteMode::BottomUp)
        );

        let explicit_bottom_up = rm_args_with_mode(Some(RecursiveDeleteMode::BottomUp));
        assert!(resolve_tos_recursive_delete_mode(false, &explicit_bottom_up).is_err());
        assert_eq!(
            resolve_tos_recursive_delete_mode(true, &explicit_bottom_up).unwrap(),
            Some(RecursiveDeleteMode::BottomUp)
        );
    }

    #[test]
    fn test_sort_keys_bottom_up_places_children_before_parents() {
        let mut keys = vec![
            "docs/".to_string(),
            "docs/a/".to_string(),
            "docs/a/file.txt".to_string(),
            "docs/b/file.txt".to_string(),
        ];

        sort_keys_bottom_up(&mut keys);

        let parent_index = keys.iter().position(|key| key == "docs/").unwrap();
        let child_dir_index = keys.iter().position(|key| key == "docs/a/").unwrap();
        let file_index = keys
            .iter()
            .position(|key| key == "docs/a/file.txt")
            .unwrap();
        assert!(file_index < child_dir_index);
        assert!(child_dir_index < parent_index);
    }

    #[test]
    fn test_hns_recursive_move_delete_items_are_bottom_up() {
        let planned = vec![
            test_plan_item("tos://bucket/docs/"),
            test_plan_item("tos://bucket/docs/a/"),
            test_plan_item("tos://bucket/docs/a/file.txt"),
            test_plan_item("tos://bucket/docs/b/file.txt"),
        ];

        let ordered = ordered_recursive_move_delete_items(&planned, true)
            .into_iter()
            .map(|item| item.source.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ordered[0], "tos://bucket/docs/a/file.txt");
        assert_eq!(ordered[1], "tos://bucket/docs/b/file.txt");
        assert_eq!(ordered[2], "tos://bucket/docs/a/");
        assert_eq!(ordered[3], "tos://bucket/docs/");
    }

    #[test]
    fn test_hns_recursive_move_source_root_normalizes_bare_prefix() {
        let args = mv_args("tos://bucket/docs", "tos://bucket/archive", true);
        let (_, key, uri) = tos_recursive_move_source_root(&args).expect("source root");

        assert_eq!(key, "docs/");
        assert_eq!(uri, "tos://bucket/docs/");
    }

    #[test]
    fn test_hns_recursive_move_source_root_can_be_planned_item() {
        let args = mv_args("tos://bucket/docs", "tos://bucket/archive", true);
        let (_, _, uri) = tos_recursive_move_source_root(&args).expect("source root");
        let planned = vec![test_plan_item("tos://bucket/docs/")];

        assert!(planned.iter().any(|item| item.source == uri));
    }

    #[test]
    fn test_bucket_manifest_item_uses_bucket_uri() {
        let bucket = bucket::BucketInfo {
            name: "demo-bucket".to_string(),
            location: "cn-beijing".to_string(),
            creation_date: "2026-01-02T03:04:05Z".to_string(),
            extranet_endpoint: String::new(),
            intranet_endpoint: String::new(),
            project_name: None,
            bucket_type: Some("hns".to_string()),
        };

        let item = bucket_manifest_item(&bucket);

        assert_eq!(item.source, "tos://demo-bucket");
        assert_eq!(item.relative_key.as_deref(), Some("demo-bucket"));
        assert_eq!(item.item_type, "bucket");
        assert_eq!(item.last_modified.as_deref(), Some("2026-01-02T03:04:05Z"));
    }

    #[test]
    fn test_limit_bucket_listing_for_ls_applies_max_keys() {
        let buckets = ["a", "b", "c"]
            .into_iter()
            .map(|name| bucket::BucketInfo {
                name: name.to_string(),
                location: "cn-beijing".to_string(),
                creation_date: "2026-01-02T03:04:05Z".to_string(),
                extranet_endpoint: String::new(),
                intranet_endpoint: String::new(),
                project_name: None,
                bucket_type: Some("hns".to_string()),
            })
            .collect::<Vec<_>>();

        let limited = limit_bucket_listing_for_ls(&buckets, 2);
        assert_eq!(
            limited
                .iter()
                .map(|bucket| bucket.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );

        let unlimited = limit_bucket_listing_for_ls(&buckets, 10);
        assert_eq!(unlimited.len(), 3);
    }

    #[test]
    fn test_parse_ls_columns_uses_default_or_custom_columns() {
        assert!(BUCKET_LS_TABLE_COLUMNS.contains(&"bucket_type"));
        assert_eq!(
            parse_ls_columns(None, BUCKET_LS_TABLE_COLUMNS),
            BUCKET_LS_TABLE_COLUMNS
        );
        assert_eq!(
            parse_ls_columns(Some(" name,creation_date "), BUCKET_LS_TABLE_COLUMNS),
            &["name", "creation_date"]
        );
        assert_eq!(
            parse_ls_columns(Some(" , "), BUCKET_LS_TABLE_COLUMNS),
            BUCKET_LS_TABLE_COLUMNS
        );
    }

    #[test]
    fn test_validate_high_level_ls_max_keys_rejects_zero_for_all_scopes() {
        assert!(validate_high_level_ls_max_keys(0).is_err());
        assert!(validate_high_level_ls_max_keys(1).is_ok());
    }

    #[test]
    fn test_write_tos_batch_report_failures_only_keeps_failed_items_without_summary_columns() {
        let dir = temp_dir("tos-report-failures-only");
        let report_path = dir.join("report.csv");
        let mut report = BatchReport::new(2);
        report.record_success("copy", "tos://bucket/a.txt", Some("tos://bucket/b.txt"));
        report.record_failure(
            "copy",
            "tos://bucket/c.txt",
            Some("tos://bucket/d.txt"),
            &CliError::TransferFailed("boom".to_string()),
        );

        write_tos_batch_report(report_path.to_str(), "ve-tos cp", &report, true)
            .expect("write report");

        let body = fs::read_to_string(rolled_csv_path(&report_path, 1)).expect("read report");
        assert!(body.starts_with("command,operation,source,destination,status"));
        assert!(!body.lines().next().unwrap_or_default().contains("total"));
        assert_eq!(body.lines().count(), 2);
        assert!(body.contains(",failed,"));
        assert!(!body.contains("a.txt"));
    }

    #[test]
    fn test_json_helpers_handle_list_objects_pagination() {
        let body = json!({
            "contents": [
                {"key": "a.txt"},
                {"key": "b&c.txt"}
            ],
            "is_truncated": true,
            "next_continuation_token": "token-1"
        });

        assert_eq!(
            json_array(&body, &["contents", "Contents"])
                .into_iter()
                .filter_map(|item| json_string(item, &["key", "Key"]))
                .collect::<Vec<_>>(),
            vec!["a.txt".to_string(), "b&c.txt".to_string()]
        );
        assert_eq!(json_bool(&body, &["is_truncated"]), Some(true));
        assert_eq!(
            json_string(&body, &["next_continuation_token"]),
            Some("token-1".to_string())
        );
    }

    #[test]
    fn test_list_objects_parser_handles_xml_single_content() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
    <Name>dms-agent-boe</Name>
    <IsTruncated>false</IsTruncated>
    <Contents>
        <Key>one.txt</Key>
        <Size>42</Size>
        <ETag>"etag-one"</ETag>
        <LastModified>2026-01-02T03:04:05.000Z</LastModified>
        <StorageClass>STANDARD</StorageClass>
    </Contents>
</ListBucketResult>"#;

        let parsed = parse_json_body(body, "ListObjects").expect("parse xml");
        let entries = json_array(&parsed, &["contents", "Contents"]);
        assert_eq!(entries.len(), 1);
        let entry = parse_object_entry(entries[0]).expect("object entry");
        assert_eq!(entry.key, "one.txt");
        assert_eq!(entry.size, 42);
        assert_eq!(entry.etag.as_deref(), Some("\"etag-one\""));
        assert_eq!(json_bool(&parsed, &["is_truncated"]), Some(false));
    }

    #[test]
    fn test_list_objects_parser_unwraps_boe_result_payload() {
        let body = json!({
            "ResponseMetadata": {
                "RequestId": "req-1"
            },
            "Result": {
                "Contents": [
                    {"Key": "a.txt", "Size": "7", "ETag": "\"etag-a\""}
                ],
                "IsTruncated": false
            }
        })
        .to_string();

        let parsed = parse_json_body(&body, "ListObjects").expect("parse wrapped json");
        let entries = json_array(&parsed, &["contents", "Contents"]);
        assert_eq!(entries.len(), 1);
        let entry = parse_object_entry(entries[0]).expect("object entry");
        assert_eq!(entry.key, "a.txt");
        assert_eq!(entry.size, 7);
        assert_eq!(entry.etag.as_deref(), Some("\"etag-a\""));
        assert_eq!(json_bool(&parsed, &["is_truncated"]), Some(false));
    }

    #[test]
    fn test_list_objects_parser_handles_byted_sdk_payload() {
        let body = json!({
            "success": 0,
            "payload": {
                "objects": [
                    {
                        "key": "sdk-object.txt",
                        "size": 11,
                        "etag": "etag-sdk",
                        "lastModified": "2026-01-02T03:04:05Z",
                        "storageClass": "STANDARD"
                    }
                ],
                "commonPrefix": ["folder/"],
                "isTruncated": false,
                "keyCount": 2,
                "nextContinuationToken": ""
            }
        })
        .to_string();

        let parsed = parse_json_body(&body, "ListObjects").expect("parse sdk payload");
        let entries = json_array(&parsed, &["contents", "Contents", "objects"]);
        let prefixes = json_array(
            &parsed,
            &[
                "common_prefixes",
                "CommonPrefixes",
                "commonPrefixes",
                "common_prefix",
                "commonPrefix",
            ],
        );
        assert_eq!(entries.len(), 1);
        assert_eq!(prefixes.len(), 1);
        let entry = parse_object_entry(entries[0]).expect("object entry");
        assert_eq!(entry.key, "sdk-object.txt");
        assert_eq!(entry.size, 11);
        assert_eq!(entry.last_modified.as_deref(), Some("2026-01-02T03:04:05Z"));
        assert_eq!(
            prefixes[0].as_str().expect("common prefix string"),
            "folder/"
        );
    }

    #[test]
    fn test_list_objects_type2_query_matches_sdk_required_param() {
        let query = list_objects_type2_query(1000);
        assert_eq!(query.get("list-type").map(String::as_str), Some("2"));
        assert_eq!(query.get("max-keys").map(String::as_str), Some("1000"));
    }

    #[test]
    fn test_byted_tos_copy_object_method_matches_sdk_sigv1() {
        let (method, query) = copy_object_method_query();
        assert_eq!(method, Method::POST);
        assert_eq!(query.get("copyobject").map(String::as_str), Some(""));
    }

    #[test]
    fn test_byted_tos_copy_source_header_matches_sdk_encoding() {
        assert_eq!(
            copy_source_header_value("bucket", "dir/a b.txt"),
            "%2Fbucket%2Fdir%2Fa%2520b.txt"
        );
    }

    #[test]
    fn test_byted_tos_multipart_queries_match_sdk_sigv1_keys() {
        let upload_query = multipart_upload_id_query("upload-1");
        assert_eq!(
            upload_query.get("uploadID").map(String::as_str),
            Some("upload-1")
        );
        assert!(!upload_query.contains_key("uploadId"));

        let part_query = multipart_part_query("upload-1", 7);
        assert_eq!(
            part_query.get("uploadID").map(String::as_str),
            Some("upload-1")
        );
        assert_eq!(part_query.get("partNumber").map(String::as_str), Some("7"));

        let copy_query = multipart_copy_part_query("upload-1", 7, 1024, 4096);
        assert_eq!(
            copy_query.get("startOffset").map(String::as_str),
            Some("1024")
        );
        assert_eq!(copy_query.get("partSize").map(String::as_str), Some("4096"));
    }

    #[test]
    fn test_byted_tos_complete_multipart_body_matches_sdk_sigv1() {
        let completed_parts = vec![
            CompletedPart {
                part_number: 2,
                etag: "etag-2".to_string(),
                crc64: None,
            },
            CompletedPart {
                part_number: 1,
                etag: "etag-1".to_string(),
                crc64: None,
            },
        ];

        let (query, headers, body) =
            complete_multipart_request("upload-1", &completed_parts).expect("complete request");
        assert_eq!(query.get("uploadID").map(String::as_str), Some("upload-1"));
        assert!(headers.is_empty());
        assert_eq!(
            String::from_utf8(body).expect("utf8 body"),
            "2:etag-2,1:etag-1"
        );
    }

    #[test]
    fn test_extract_upload_id_unwraps_byted_tos_payload() {
        let response = Envelope::success(
            "test",
            core::RawResponseData {
                status_code: 200,
                headers: BTreeMap::new(),
                body_format: Some("json".to_string()),
                body: Some(json!({
                    "success": 0,
                    "payload": {
                        "upload_id": "upload-1"
                    }
                })),
            },
        );

        assert_eq!(extract_upload_id(&response).as_deref(), Some("upload-1"));
    }

    #[test]
    fn test_json_helpers_handle_list_parts_markers() {
        let body = json!({
            "parts": [
                {"part_number": 1, "etag": "etag-1"},
                {"part_id": "2", "etag": "etag-2"}
            ],
            "is_truncated": true,
            "next_part_number_marker": "2"
        });

        assert_eq!(
            json_array(&body, &["parts", "part", "Parts"])
                .into_iter()
                .filter_map(|part| json_u32(part, &["part_number", "part_id", "PartNumber"]))
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(json_bool(&body, &["is_truncated"]), Some(true));
        assert_eq!(
            json_string(&body, &["next_part_number_marker"]),
            Some("2".to_string())
        );
    }

    #[test]
    fn test_bounded_list_page_size_uses_remainder_page() {
        let mut returned = 0;
        let mut pages = Vec::new();
        while returned < 9950 {
            let page_size = bounded_list_page_size(returned, 9950);
            pages.push(page_size);
            returned += page_size;
        }

        assert_eq!(pages.len(), 10);
        assert_eq!(&pages[..9], &[1000; 9]);
        assert_eq!(pages[9], 950);
    }

    #[test]
    fn test_dedupe_ls_filters_current_folder_marker() {
        let objects = vec![
            ObjectEntry {
                key: "folder/".to_string(),
                size: 0,
                last_modified: None,
                etag: None,
                storage_class: None,
            },
            ObjectEntry {
                key: "folder/file.txt".to_string(),
                size: 10,
                last_modified: None,
                etag: None,
                storage_class: None,
            },
        ];

        let (objects, prefixes) = dedupe_ls_objects_and_prefixes(
            objects,
            vec!["folder/sub/".to_string()],
            Some("folder/"),
        );
        let entries = merge_ls_entries(objects, prefixes);

        assert!(!entries
            .iter()
            .any(|entry| entry.key == "folder/" && entry.entry_type == "file"));
        assert!(entries
            .iter()
            .any(|entry| entry.key == "folder/file.txt" && entry.entry_type == "file"));
        assert!(entries
            .iter()
            .any(|entry| entry.key == "folder/sub/" && entry.entry_type == "directory"));
    }

    #[test]
    fn test_dedupe_ls_common_prefix_wins_over_folder_marker() {
        let objects = vec![ObjectEntry {
            key: "folder/sub/".to_string(),
            size: 0,
            last_modified: None,
            etag: None,
            storage_class: None,
        }];

        let (objects, prefixes) = dedupe_ls_objects_and_prefixes(
            objects,
            vec!["folder/sub/".to_string()],
            Some("folder/"),
        );
        let entries = merge_ls_entries(objects, prefixes);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "folder/sub/");
        assert_eq!(entries[0].entry_type, "directory");
    }

    #[test]
    fn test_dedupe_ls_trailing_slash_object_marker_is_directory() {
        let objects = vec![ObjectEntry {
            key: "folder/standalone/".to_string(),
            size: 0,
            last_modified: None,
            etag: None,
            storage_class: None,
        }];

        let (objects, prefixes) =
            dedupe_ls_objects_and_prefixes(objects, Vec::new(), Some("folder/"));
        let entries = merge_ls_entries(objects, prefixes);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "folder/standalone/");
        assert_eq!(entries[0].entry_type, "directory");
    }

    #[test]
    fn test_du_accumulator_streams_profile_buckets_and_top_k() {
        let mut accumulator = DuAccumulator::new(2, true);
        for entry in [
            ObjectEntry {
                key: "root/a/old.log".to_string(),
                size: 512,
                last_modified: Some("2025-01-01T00:00:00Z".to_string()),
                etag: None,
                storage_class: Some("STANDARD".to_string()),
            },
            ObjectEntry {
                key: "root/a/new.bin".to_string(),
                size: 2_000_000,
                last_modified: Some("2026-01-01T00:00:00Z".to_string()),
                etag: None,
                storage_class: Some("IA".to_string()),
            },
            ObjectEntry {
                key: "root/b/huge.dat".to_string(),
                size: 200_000_000,
                last_modified: Some("2024-01-01T00:00:00Z".to_string()),
                etag: None,
                storage_class: Some("STANDARD".to_string()),
            },
        ] {
            accumulator.record_tos_object("bucket", &entry, Some("root/"), Some(1));
        }
        accumulator.record_directory_prefix("root/a/");

        assert_eq!(accumulator.object_count, 3);
        assert_eq!(accumulator.directory_count, 1);
        assert_eq!(accumulator.manifest_items.len(), 3);
        assert_eq!(accumulator.largest_objects[0].key, "root/b/huge.dat");
        assert_eq!(accumulator.oldest_objects[0].key, "root/b/huge.dat");
        assert_eq!(
            accumulator.file_types.get("log").expect("log bucket").count,
            1
        );
        assert_eq!(
            accumulator
                .directories
                .get("a")
                .expect("directory bucket")
                .count,
            2
        );
        assert_eq!(
            accumulator
                .size_histogram
                .get(">100M")
                .expect("histogram bucket")
                .count,
            1
        );
        assert_eq!(
            accumulator
                .storage_classes
                .get("STANDARD")
                .expect("STANDARD bucket")
                .count,
            2
        );
        assert!(accumulator.storage_classes.get("UNKNOWN").is_none());
        assert!(accumulator
            .cost_estimate(&storage_price_table(&[]).expect("prices"))
            .get("disclaimer")
            .is_some());
    }

    #[test]
    fn test_du_page_dedupes_common_prefix_and_folder_marker_object() {
        let (objects, directory_prefixes, child_prefixes) = dedupe_tos_du_page_entries(
            vec![
                object_entry("root/"),
                object_entry("root/a/"),
                object_entry("root/a/file.txt"),
            ],
            vec![
                "root/".to_string(),
                "root/a/".to_string(),
                "root/a/".to_string(),
            ],
            "root/",
        );

        assert_eq!(
            objects
                .iter()
                .map(|entry| entry.key.as_str())
                .collect::<Vec<_>>(),
            vec!["root/a/file.txt"]
        );
        assert_eq!(directory_prefixes, vec!["root/a/"]);
        assert_eq!(child_prefixes, vec!["root/a/"]);
    }

    #[test]
    fn test_du_accumulator_dedupes_directory_prefixes_across_merges() {
        let mut left = DuAccumulator::new(2, false);
        let mut right = DuAccumulator::new(2, false);

        left.record_directory_prefix("root/a/");
        left.record_directory_prefix("root/a/");
        right.record_directory_prefix("root/a/");
        right.record_directory_prefix("root/b/");
        left.merge(right);

        assert_eq!(left.directory_count, 2);
    }

    #[test]
    fn test_du_payload_defaults_to_aggregate_fields() {
        let mut accumulator = DuAccumulator::new(2, false);
        accumulator.record_request_id(Some("req-page-1".to_string()));
        accumulator.record_tos_object(
            "bucket",
            &ObjectEntry {
                key: "root/a/file.log".to_string(),
                size: 512,
                last_modified: Some("2025-01-01T00:00:00Z".to_string()),
                etag: None,
                storage_class: Some("STANDARD".to_string()),
            },
            Some("root/"),
            None,
        );
        accumulator.record_directory_prefix("root/a/");
        let target = ParsedTosUri {
            bucket: "bucket".to_string(),
            key: Some("root/".to_string()),
        };
        let args = du_args(None);
        let payload = du_output_payload(
            &GlobalArgs::default(),
            "tos://bucket/root/",
            &target,
            &accumulator,
            &args,
            None,
            None,
            None,
            false,
            false,
            DEFAULT_LIST_CONCURRENCY,
        );

        assert_eq!(payload["object_count"], 1);
        assert_eq!(payload["directory_count"], 1);
        assert_eq!(payload["total_bytes"], 512);
        assert_eq!(payload["storage_classes"]["STANDARD"]["object_count"], 1);
        assert_eq!(payload["storage_classes"]["STANDARD"]["bytes"], 512);
        assert!(payload["storage_classes"].get("UNKNOWN").is_none());
        assert!(accumulator.manifest_items.is_empty());
        assert!(payload.get("diagnostics").is_none());
        assert!(payload.get("file_types").is_none());
        assert!(payload.get("directories").is_none());
        assert!(payload.get("traversal").is_none());
        assert!(payload.get("groups").is_none());
        assert_eq!(
            accumulator
                .directories
                .get("a")
                .expect("directory bucket")
                .count,
            1
        );
    }

    #[test]
    fn test_du_payload_verbose_exposes_diagnostics() {
        let mut accumulator = DuAccumulator::new(2, false);
        accumulator.record_request_id(Some("req-page-1".to_string()));
        let target = ParsedTosUri {
            bucket: "bucket".to_string(),
            key: None,
        };
        let args = du_args(Some(1));
        let mut global = GlobalArgs::default();
        global.verbose = true;

        let payload = du_output_payload(
            &global,
            "tos://bucket",
            &target,
            &accumulator,
            &args,
            Some(accumulator.directory_distribution_json()),
            None,
            Some("/tmp/du-manifest.json"),
            true,
            true,
            DEFAULT_LIST_CONCURRENCY,
        );

        assert!(payload.get("groups").is_some());
        assert_eq!(payload["manifest_path"], "/tmp/du-manifest.json");
        assert_eq!(
            payload["diagnostics"]["service_request_ids"][0],
            "req-page-1"
        );
        assert_eq!(payload["diagnostics"]["service_request_ids_omitted"], 0);
        assert_eq!(
            payload["diagnostics"]["limits"]["category_buckets"],
            DU_CATEGORY_BUCKET_LIMIT
        );
        assert_eq!(payload["diagnostics"]["traversal"]["bucket_mode"], "hns");
    }

    #[test]
    fn test_du_diagnostics_categories_are_bounded() {
        let mut accumulator = DuAccumulator::new(0, false);
        for index in 0..(DU_CATEGORY_BUCKET_LIMIT + 3) {
            accumulator.record_tos_object(
                "bucket",
                &ObjectEntry {
                    key: format!("root/dir-{index}/file.ext-{index}"),
                    size: 1,
                    last_modified: None,
                    etag: None,
                    storage_class: Some("STANDARD".to_string()),
                },
                Some("root/"),
                None,
            );
        }

        assert!(accumulator.file_types.len() <= DU_CATEGORY_BUCKET_LIMIT + 1);
        assert!(accumulator.directories.len() <= DU_CATEGORY_BUCKET_LIMIT + 1);
        assert_eq!(
            accumulator
                .file_types
                .get(DU_OVERFLOW_BUCKET)
                .expect("overflow file type bucket")
                .count,
            3
        );
        assert_eq!(
            accumulator
                .directories
                .get(DU_OVERFLOW_BUCKET)
                .expect("overflow directory bucket")
                .count,
            3
        );
    }

    #[test]
    fn test_du_request_ids_are_bounded() {
        let mut accumulator = DuAccumulator::new(0, false);
        for index in 0..(DU_REQUEST_ID_LIMIT + 2) {
            accumulator.record_request_id(Some(format!("req-{index}")));
        }

        assert_eq!(accumulator.request_ids.len(), DU_REQUEST_ID_LIMIT);
        assert_eq!(accumulator.request_ids_omitted, 2);
    }

    #[test]
    fn test_find_size_filter_bounds_are_inclusive() {
        assert!(find_size_matches(
            100,
            parse_find_size_filter("+100B").expect("min filter")
        ));
        assert!(find_size_matches(
            100,
            parse_find_size_filter("-100B").expect("max filter")
        ));
        assert!(find_size_matches(
            100,
            parse_find_size_filter("100B").expect("equal filter")
        ));
        assert!(!find_size_matches(
            99,
            parse_find_size_filter("+100B").expect("min filter")
        ));
    }

    #[test]
    fn test_find_mtime_filter_uses_relative_time_window() {
        let within = parse_find_mtime_filter("-7d").expect("within filter");
        let threshold = match &within {
            FindMtimeFilter::WithinLast(value) => *value,
            _ => panic!("expected within filter"),
        };
        let boundary = threshold.to_rfc3339();
        let older = (threshold - chrono::Duration::milliseconds(1)).to_rfc3339();
        assert!(find_mtime_matches(Some(&boundary), &within));
        assert!(!find_mtime_matches(Some(&older), &within));

        let older_or_equal = parse_find_mtime_filter("+7d").expect("older filter");
        let threshold = match &older_or_equal {
            FindMtimeFilter::OlderThanOrEqual(value) => *value,
            _ => panic!("expected older filter"),
        };
        let boundary = threshold.to_rfc3339();
        let newer = (threshold + chrono::Duration::milliseconds(1)).to_rfc3339();
        assert!(find_mtime_matches(Some(&boundary), &older_or_equal));
        assert!(!find_mtime_matches(Some(&newer), &older_or_equal));
    }

    #[test]
    fn test_find_mtime_filter_bare_duration_matches_exact_age_bucket() {
        let exact = parse_find_mtime_filter("7d").expect("bare duration");
        let (newest, oldest_exclusive) = match &exact {
            FindMtimeFilter::EqualAge {
                newest,
                oldest_exclusive,
            } => (*newest, *oldest_exclusive),
            _ => panic!("expected exact age filter"),
        };
        let boundary = newest.to_rfc3339();
        let older_inside = (newest - chrono::Duration::milliseconds(1)).to_rfc3339();
        let too_new = (newest + chrono::Duration::milliseconds(1)).to_rfc3339();
        let too_old = oldest_exclusive.to_rfc3339();

        assert!(find_mtime_matches(Some(&boundary), &exact));
        assert!(find_mtime_matches(Some(&older_inside), &exact));
        assert!(!find_mtime_matches(Some(&too_new), &exact));
        assert!(!find_mtime_matches(Some(&too_old), &exact));
    }

    #[test]
    fn test_ls_entries_use_human_readable_size_when_requested() {
        let entries = vec![LsEntry {
            key: "docs/big.bin".to_string(),
            entry_type: "file",
            size: 10 * 1024 * 1024,
            last_modified: None,
            etag: None,
            storage_class: None,
        }];

        let readable = ls_entries_for_output(&entries, true);
        assert_eq!(readable[0]["size"], "10.00 MiB");

        let raw = ls_entries_for_output(&entries, false);
        assert_eq!(raw[0]["size"], 10 * 1024 * 1024);
    }

    #[test]
    fn test_checkpoint_path_is_stable_for_same_task() {
        let dir = temp_dir("checkpoint-path");
        let first = checkpoint_path(
            Some(dir.to_str().expect("checkpoint dir")),
            "/tmp/source.bin",
            "bucket",
            "key",
            1024,
            123,
            20 * 1024 * 1024,
            "",
            "default",
            "https://tos-cn-beijing.volces.com",
        )
        .expect("first checkpoint path");
        let second = checkpoint_path(
            Some(dir.to_str().expect("checkpoint dir")),
            "/tmp/source.bin",
            "bucket",
            "key",
            1024,
            123,
            20 * 1024 * 1024,
            "",
            "default",
            "https://tos-cn-beijing.volces.com",
        )
        .expect("second checkpoint path");

        assert_eq!(first, second);
        assert!(first
            .file_name()
            .and_then(|name| name.to_str())
            .expect("file name")
            .starts_with("cp-upload-"));
    }

    #[test]
    fn test_object_write_options_parse_metadata_and_headers() {
        let options = object_write_options_from_parts(
            Some("text/plain"),
            Some("IA"),
            Some("bucket-owner-entrusted"),
            Some("key1=value1#x-tos-meta-key2=value2"),
        )
        .expect("valid write options");

        let upload_headers = options.headers(false);
        assert_eq!(
            upload_headers.get("content-type").map(String::as_str),
            Some("text/plain")
        );
        assert_eq!(
            upload_headers
                .get("x-tos-storage-class")
                .map(String::as_str),
            Some("IA")
        );
        assert_eq!(
            upload_headers.get("x-tos-acl").map(String::as_str),
            Some("bucket-owner-entrusted")
        );
        assert_eq!(
            upload_headers.get("x-tos-meta-key1").map(String::as_str),
            Some("value1")
        );
        assert_eq!(
            upload_headers.get("x-tos-meta-key2").map(String::as_str),
            Some("value2")
        );
        assert!(!upload_headers.contains_key("x-metadata-directive"));

        let copy_headers = options.headers(true);
        assert_eq!(
            copy_headers
                .get("x-tos-metadata-directive")
                .map(String::as_str),
            Some("REPLACE_NEW")
        );
        assert!(!copy_headers.contains_key("x-metadata-directive"));
    }

    #[test]
    fn test_tos_copy_storage_class_sets_metadata_directive() {
        let options = object_write_options_from_parts(None, Some("ARCHIVE"), None, None)
            .expect("valid write options");

        let copy_headers = options.headers(true);

        assert_eq!(
            copy_headers.get("x-tos-storage-class").map(String::as_str),
            Some("ARCHIVE")
        );
        assert_eq!(
            copy_headers
                .get("x-tos-metadata-directive")
                .map(String::as_str),
            Some("REPLACE_NEW")
        );
        assert!(!copy_headers.contains_key("x-metadata-directive"));
    }

    #[test]
    fn test_tos_cp_simple_upload_headers_include_content_length() {
        let headers = cp_simple_upload_headers(
            &ObjectWriteOptions::default(),
            12345,
            42,
            EffectiveOverwriteStrategy::Force,
        );

        assert_eq!(
            headers.get("content-length").map(String::as_str),
            Some("42")
        );
        assert_eq!(
            headers.get("x-hash-crc64ecma").map(String::as_str),
            Some("12345")
        );
    }

    #[test]
    fn test_tos_cp_multipart_part_headers_include_content_length() {
        let headers = cp_multipart_upload_part_headers(12345, 64);

        assert_eq!(
            headers.get("content-length").map(String::as_str),
            Some("64")
        );
        assert_eq!(
            headers.get("x-hash-crc64ecma").map(String::as_str),
            Some("12345")
        );
    }

    #[test]
    fn test_tos_local_cp_rejects_upload_storage_class_override() {
        let mut args = cp_args("/tmp/source.bin", "tos://bucket/source.bin", false);
        args.storage_class = Some("ARCHIVE".to_string());

        let err = match cp_operation(&args) {
            Ok(_) => panic!("ve-tos upload storage class must be rejected"),
            Err(err) => err,
        };

        assert!(err
            .to_string()
            .contains("ByteTOS upload does not support --storage-class"));
    }

    #[test]
    fn test_tos_put_rejects_upload_storage_class_override() {
        let args = PutArgs {
            path: Some("tos://bucket/stdin.bin".to_string()),
            bucket: None,
            key: None,
            content_type: None,
            storage_class: Some("ARCHIVE".to_string()),
            acl: None,
            meta: None,
            multipart_threshold: None,
            no_clobber: false,
            progress: false,
            no_progress: true,
        };

        let err = match put_operation(&args) {
            Ok(_) => panic!("ve-tos put storage class must be rejected"),
            Err(err) => err,
        };

        assert!(err
            .to_string()
            .contains("ByteTOS upload does not support --storage-class"));
    }

    #[test]
    fn test_tos_remote_copy_allows_storage_class_override() {
        let mut args = cp_args("tos://bucket/source.bin", "tos://bucket/dest.bin", false);
        args.storage_class = Some("ARCHIVE".to_string());

        assert!(cp_operation(&args).is_ok());
    }

    #[test]
    fn test_ve_tos_upload_allows_storage_class_override() {
        assert!(ensure_tos_upload_storage_class_supported_for_binary(
            Binary::VeTos,
            "ve-tos cp",
            Some("/tmp/source.bin"),
            "tos://bucket/source.bin",
            Some("ARCHIVE"),
        )
        .is_ok());
    }

    #[test]
    fn test_tos_auto_recursive_list_mode_uses_hierarchical_listing() {
        assert!(resolve_tos_recursive_list_mode_for_binary(
            Binary::Tos,
            false,
            None
        ));
        assert!(resolve_tos_recursive_list_mode_for_binary(
            Binary::Tos,
            false,
            Some(RecursiveListMode::Auto)
        ));
        assert!(resolve_tos_recursive_list_mode_for_binary(
            Binary::Tos,
            false,
            Some(RecursiveListMode::Flat)
        ));
    }

    #[test]
    fn test_tos_du_uses_hierarchical_listing_for_byted_tos() {
        assert!(resolve_tos_du_list_mode_for_binary(Binary::Tos, false));
        assert!(resolve_tos_du_list_mode_for_binary(Binary::Tos, true));
        assert!(resolve_tos_du_list_mode_for_binary(Binary::VeTos, false));
        assert!(resolve_tos_du_list_mode_for_binary(Binary::VeTos, true));
    }

    #[test]
    fn test_ve_tos_auto_recursive_list_mode_uses_bucket_shape() {
        assert!(!resolve_tos_recursive_list_mode_for_binary(
            Binary::VeTos,
            false,
            None
        ));
        assert!(resolve_tos_recursive_list_mode_for_binary(
            Binary::VeTos,
            true,
            None
        ));
        assert!(!resolve_tos_recursive_list_mode_for_binary(
            Binary::VeTos,
            true,
            Some(RecursiveListMode::Flat)
        ));
        assert!(resolve_tos_recursive_list_mode_for_binary(
            Binary::VeTos,
            false,
            Some(RecursiveListMode::Hierarchical)
        ));
    }

    #[test]
    fn test_object_write_options_reject_invalid_values() {
        assert!(object_write_options_from_parts(Some(""), None, None, None).is_err());
        assert!(object_write_options_from_parts(None, Some("bad"), None, None).is_err());
        assert!(object_write_options_from_parts(None, None, Some("bad"), None).is_err());
        assert!(object_write_options_from_parts(None, None, None, Some("=value")).is_err());
        assert!(
            object_write_options_from_parts(None, None, None, Some("key=value#key=again")).is_err()
        );
        assert!(
            object_write_options_from_parts(None, None, None, Some("key=line1\nline2")).is_err()
        );
        assert!(object_write_options_from_parts(None, None, None, Some("key=\u{0000}")).is_err());
    }

    #[test]
    fn test_checkpoint_path_changes_when_task_fingerprint_changes() {
        let dir = temp_dir("checkpoint-path-fingerprint");
        let first = checkpoint_path(
            Some(dir.to_str().expect("checkpoint dir")),
            "/tmp/source.bin",
            "bucket",
            "key",
            1024,
            123,
            20 * 1024 * 1024,
            "",
            "default",
            "https://tos-cn-beijing.volces.com",
        )
        .expect("first checkpoint path");
        let changed_mtime = checkpoint_path(
            Some(dir.to_str().expect("checkpoint dir")),
            "/tmp/source.bin",
            "bucket",
            "key",
            1024,
            456,
            20 * 1024 * 1024,
            "",
            "default",
            "https://tos-cn-beijing.volces.com",
        )
        .expect("mtime checkpoint path");
        let changed_profile = checkpoint_path(
            Some(dir.to_str().expect("checkpoint dir")),
            "/tmp/source.bin",
            "bucket",
            "key",
            1024,
            123,
            20 * 1024 * 1024,
            "",
            "prod",
            "https://tos-cn-beijing.volces.com",
        )
        .expect("profile checkpoint path");
        let changed_endpoint = checkpoint_path(
            Some(dir.to_str().expect("checkpoint dir")),
            "/tmp/source.bin",
            "bucket",
            "key",
            1024,
            123,
            20 * 1024 * 1024,
            "",
            "default",
            "https://tos-test-cn-beijing.volces.com",
        )
        .expect("endpoint checkpoint path");
        let changed_write_context = checkpoint_path(
            Some(dir.to_str().expect("checkpoint dir")),
            "/tmp/source.bin",
            "bucket",
            "key",
            1024,
            123,
            20 * 1024 * 1024,
            "storage-class=IA",
            "default",
            "https://tos-cn-beijing.volces.com",
        )
        .expect("write context checkpoint path");

        assert_ne!(first, changed_mtime);
        assert_ne!(first, changed_profile);
        assert_ne!(first, changed_endpoint);
        assert_ne!(first, changed_write_context);
    }

    #[test]
    fn test_checkpoint_path_changes_when_top_level_surface_changes() {
        let dir = temp_dir("checkpoint-path-surface");
        let byted_tos = checkpoint_path_for_surface(
            "tos",
            Some(dir.to_str().expect("checkpoint dir")),
            "/tmp/source.bin",
            "bucket",
            "key",
            1024,
            123,
            20 * 1024 * 1024,
            "",
            "default",
            "https://tos-cn-boe.byted.org",
        )
        .expect("ve-tos checkpoint path");
        let ve_tos = checkpoint_path_for_surface(
            "ve-tos",
            Some(dir.to_str().expect("checkpoint dir")),
            "/tmp/source.bin",
            "bucket",
            "key",
            1024,
            123,
            20 * 1024 * 1024,
            "",
            "default",
            "https://tos-cn-boe.byted.org",
        )
        .expect("ve-tos checkpoint path");

        assert_ne!(byted_tos, ve_tos);
    }

    #[test]
    fn test_checkpoint_lock_conflict_is_deterministic() {
        let dir = temp_dir("checkpoint-lock");
        let checkpoint = dir.join("upload.json");
        let first_lock = CheckpointLock::acquire(&checkpoint).expect("first lock");
        let second = CheckpointLock::acquire(&checkpoint);

        assert!(matches!(second, Err(CliError::Conflict(_))));
        drop(first_lock);
        assert!(CheckpointLock::acquire(&checkpoint).is_ok());
    }

    #[test]
    fn test_checkpoint_round_trip() {
        let dir = temp_dir("checkpoint-round-trip");
        let path = dir.join("upload.json");
        let checkpoint = Checkpoint {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            source_path: Some("/tmp/source.bin".to_string()),
            file_size: 10,
            part_size: 5,
            upload_id: Some("upload-id".to_string()),
            completed_parts: vec![CompletedPart {
                part_number: 1,
                etag: "etag-1".to_string(),
                crc64: Some(123),
            }],
        };

        save_checkpoint(&path, &checkpoint).expect("save checkpoint");
        let loaded = load_checkpoint(&path)
            .expect("load checkpoint")
            .expect("checkpoint exists");
        assert_eq!(loaded.upload_id.as_deref(), Some("upload-id"));
        assert_eq!(loaded.completed_parts.len(), 1);
        remove_checkpoint(&path).expect("remove checkpoint");
        assert!(load_checkpoint(&path).expect("load missing").is_none());
    }

    #[test]
    fn test_streaming_crc64_matches_in_memory_crc64() {
        let dir = temp_dir("crc64-streaming");
        let path = dir.join("object.bin");
        fs::write(&path, b"streaming crc64 input").expect("write file");
        let mut file = File::open(&path).expect("open file");

        assert_eq!(
            crc64_reader(&mut file, None).expect("stream crc"),
            crc64_ecma(b"streaming crc64 input")
        );
    }

    // [Review Fix #CRC64-XZ] 锁定 CLI 的 CRC64 实现与 TOS 服务端 (Go hash/crc64.ECMA = CRC-64/XZ) 同源。
    // 这些向量是 Go 标准库 crc64.New(crc64.MakeTable(crc64.ECMA)) 在相同输入下的输出值
    // （含 init = 0xFFFFFFFFFFFFFFFF / xorOut = 0xFFFFFFFFFFFFFFFF），等同于 TOS 服务端复算
    // x-hash-crc64ecma 的方式。其中 "123456789" => 0x995DC9BBDF1939FA 是 CRC-64/XZ 公认基准向量。
    // 任何破坏对齐的改动会立即被本测试拦截。
    #[test]
    fn test_crc64_matches_tos_server_reflected_ecma_vectors() {
        let cases: &[(&[u8], u64)] = &[
            (b"", 0x0000_0000_0000_0000),
            (b"a", 0x3302_8477_2e65_2b05),
            (b"abc", 0x2cd8_094a_1a27_7627),
            (b"123456789", 0x995d_c9bb_df19_39fa),
            (b"hello", 0x9b1e_dae5_dbb9_37b1),
            (
                b"The quick brown fox jumps over the lazy dog",
                0x5b5e_b8c2_e54a_a1c4,
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(
                crc64_ecma(input),
                *expected,
                "CRC64 mismatch for input={:?}: expected={:#x}",
                input,
                expected
            );
        }
    }

    // [Review Fix #CRC64-XZ] 验证流式 CRC64 与一次性 CRC64 在 TOS 标准向量上完全一致，
    // 防止"自一致但与服务端不一致"的盲区（既往的回归测试只对比了内部一致性）。
    #[test]
    fn test_streaming_crc64_matches_tos_server_reflected_ecma_vectors() {
        let cases: &[(&[u8], u64)] = &[
            (b"abc", 0x2cd8_094a_1a27_7627),
            (b"123456789", 0x995d_c9bb_df19_39fa),
            (
                b"The quick brown fox jumps over the lazy dog",
                0x5b5e_b8c2_e54a_a1c4,
            ),
        ];
        let dir = temp_dir("crc64-tos-vectors");
        for (idx, (input, expected)) in cases.iter().enumerate() {
            let path = dir.join(format!("vec-{idx}.bin"));
            fs::write(&path, input).expect("write file");
            let mut file = File::open(&path).expect("open file");
            assert_eq!(
                crc64_reader(&mut file, None).expect("stream crc"),
                *expected,
                "streaming CRC64 mismatch for input={:?}: expected={:#x}",
                input,
                expected
            );
        }
    }

    // [Review Fix #CRC64-XZ] 大块输入（>1 MiB）跨缓冲区边界仍要保持与 TOS 一致；
    // crc64_reader 的内部缓冲区是 1 MiB，这里强制触发多次填充。
    #[test]
    fn test_streaming_crc64_handles_multi_buffer_chunks() {
        let payload = vec![0xA5_u8; 3 * 1024 * 1024 + 7];
        let one_shot = crc64_ecma(&payload);
        let dir = temp_dir("crc64-large");
        let path = dir.join("large.bin");
        fs::write(&path, &payload).expect("write file");
        let mut file = File::open(&path).expect("open file");
        assert_eq!(
            crc64_reader(&mut file, None).expect("stream crc"),
            one_shot,
            "streaming CRC64 must equal one-shot CRC64 across buffer boundaries"
        );
    }

    #[test]
    fn test_restore_key_matches_include_exclude() {
        let args = RestoreArgs {
            path: Some("tos://bucket/prefix/".to_string()),
            bucket: None,
            key: None,
            recursive: true,
            manifest: None,
            include: Some(".txt".to_string()),
            exclude: Some("skip".to_string()),
            days: None,
            tier: None,
            version_id: None,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            batch_concurrency: None,
            list_concurrency: None,
            recursive_list_mode: None,
            force: true,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
        };

        let path = "tos://bucket/prefix/";
        assert!(restore_key_matches(&args, "prefix/keep.txt", path));
        assert!(!restore_key_matches(&args, "prefix/skip.txt", path));
        assert!(!restore_key_matches(&args, "prefix/image.png", path));
    }

    #[test]
    fn test_restore_plan_marks_trailing_slash_key_as_directory() {
        let item = restore_plan_item_from_key("prefix/folder/".to_string());

        assert!(item.is_directory);
        let manifest_item = restore_plan_manifest_item("bucket", Some("prefix/"), &item);
        assert_eq!(manifest_item.item_type, "directory");
        assert_eq!(manifest_item.relative_key.as_deref(), Some("folder/"));
    }

    #[test]
    fn test_restore_plan_keeps_non_directory_key_as_object() {
        let item = restore_plan_item_from_key("prefix/file.txt".to_string());

        assert!(!item.is_directory);
        let manifest_item = restore_plan_manifest_item("bucket", Some("prefix/"), &item);
        assert_eq!(manifest_item.item_type, "object");
        assert_eq!(manifest_item.relative_key.as_deref(), Some("file.txt"));
    }

    #[test]
    fn test_restore_manifest_uri_directory_marker_uses_target_bucket() {
        let target = parse_tos_uri("tos://bucket/prefix/", true).expect("parse target");
        let key = restore_manifest_line_to_key("tos://bucket/prefix/folder/", &target)
            .expect("manifest key");
        let item = restore_plan_item_from_key(key);

        assert_eq!(item.key, "prefix/folder/");
        assert!(item.is_directory);
        assert!(restore_manifest_line_to_key("tos://other/prefix/folder/", &target).is_err());
    }

    #[test]
    fn test_record_tos_restore_skipped_updates_report() {
        let dir = temp_dir("restore-skip-report");
        let report_path = dir.join("restore-report.csv");
        let mut report = BatchReport::new(1);

        record_tos_restore_skipped(
            None,
            Some(report_path.to_str().expect("report path")),
            false,
            &mut report,
            "tos://bucket/prefix/folder/",
        )
        .expect("record skipped");

        assert_eq!(report.summary.skipped, 1);
        let body = fs::read_to_string(report_path).expect("read report");
        assert!(body.contains(",restore,"));
        assert!(body.contains(",skipped,"));
    }

    #[test]
    fn test_local_sync_should_skip_uses_size_or_exact_timestamps() {
        let dir = temp_dir("sync-skip");
        let source = dir.join("source.txt");
        let destination = dir.join("destination.txt");
        fs::write(&destination, "same-size-b").expect("write destination");
        // [Review Fix #13] Sleep briefly between the two writes so the OS
        // reports distinct `mtime`s. On filesystems with coarse mtime
        // resolution (e.g. some tmpfs, ext4 noatime), two writes issued in
        // the same millisecond would otherwise collapse to identical
        // timestamps and make the `exact_timestamps` branch flake.
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&source, "same-size-a").expect("write source");

        let size_only = SyncArgs {
            source: source.display().to_string(),
            destination: destination.display().to_string(),
            delete: false,
            force: false,
            size_only: true,
            exact_timestamps: false,
            include_parent: false,
            include: None,
            exclude: None,
            checkpoint_dir: None,
            content_type: None,
            storage_class: None,
            acl: None,
            meta: None,
            checkpoint_threshold: None,
            batch_concurrency: None,
            list_concurrency: None,
            recursive_list_mode: None,
            multipart_concurrency: None,
            progress_granularity: None,
            overwrite_strategy: None,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            bandwidth_limit: None,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
        };
        assert!(local_sync_should_skip(
            source.to_str().expect("source"),
            destination.to_str().expect("destination"),
            &size_only
        )
        .expect("size-only skip"));

        let exact = SyncArgs {
            exact_timestamps: true,
            size_only: false,
            ..size_only
        };
        assert!(!local_sync_should_skip(
            source.to_str().expect("source"),
            destination.to_str().expect("destination"),
            &exact
        )
        .expect("exact timestamp check"));
    }

    #[test]
    fn test_remote_sync_should_skip_respects_exact_timestamps() {
        let args = SyncArgs {
            source: "./source".to_string(),
            destination: "tos://bucket/key".to_string(),
            delete: false,
            force: false,
            size_only: true,
            exact_timestamps: false,
            include_parent: false,
            include: None,
            exclude: None,
            checkpoint_dir: None,
            content_type: None,
            storage_class: None,
            acl: None,
            meta: None,
            checkpoint_threshold: None,
            batch_concurrency: None,
            list_concurrency: None,
            recursive_list_mode: None,
            multipart_concurrency: None,
            progress_granularity: None,
            overwrite_strategy: None,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            bandwidth_limit: None,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
        };
        let destination = ObjectEntry {
            key: "key".to_string(),
            size: 10,
            last_modified: Some("2026-05-16T00:00:00Z".to_string()),
            etag: Some("etag".to_string()),
            storage_class: None,
        };

        assert!(remote_sync_should_skip(10, Some(&destination), &args));
        let exact = SyncArgs {
            exact_timestamps: true,
            ..args
        };
        assert!(!remote_sync_should_skip(10, Some(&destination), &exact));
    }

    #[test]
    fn test_tos_sync_delete_manifest_items_are_bottom_up() {
        let mut items = vec![
            sync_delete_item("tos://bucket/root/"),
            sync_delete_item("tos://bucket/root/a/"),
            sync_delete_item("tos://bucket/root/a/file.txt"),
            sync_delete_item("tos://bucket/root/b.txt"),
        ];

        sort_tos_sync_delete_manifest_items_bottom_up(&mut items);

        let root_index = items
            .iter()
            .position(|item| item.source == "tos://bucket/root/")
            .expect("root marker");
        let child_dir_index = items
            .iter()
            .position(|item| item.source == "tos://bucket/root/a/")
            .expect("child marker");
        let file_index = items
            .iter()
            .position(|item| item.source == "tos://bucket/root/a/file.txt")
            .expect("child file");
        assert!(file_index < child_dir_index);
        assert!(child_dir_index < root_index);
    }

    #[test]
    fn test_tos_sync_delete_entries_are_bottom_up() {
        let mut entries = vec![
            object_entry("root/"),
            object_entry("root/a/"),
            object_entry("root/a/file.txt"),
            object_entry("root/b.txt"),
        ];

        sort_tos_sync_delete_entries_bottom_up(&mut entries);

        let keys = entries
            .iter()
            .map(|entry| entry.key.as_str())
            .collect::<Vec<_>>();
        assert_eq!(keys[0], "root/a/file.txt");
        assert!(
            keys.iter().position(|key| *key == "root/a/").unwrap()
                < keys.iter().position(|key| *key == "root/").unwrap()
        );
    }

    #[test]
    fn test_tos_entries_match_prefers_etag_over_size() {
        let source = ObjectEntry {
            key: "source".to_string(),
            size: 10,
            last_modified: None,
            etag: Some("same".to_string()),
            storage_class: None,
        };
        let destination = ObjectEntry {
            key: "destination".to_string(),
            size: 20,
            last_modified: None,
            etag: Some("same".to_string()),
            storage_class: None,
        };
        let args = SyncArgs {
            source: "tos://bucket/source".to_string(),
            destination: "tos://bucket/destination".to_string(),
            delete: false,
            force: false,
            size_only: false,
            exact_timestamps: true,
            include_parent: false,
            include: None,
            exclude: None,
            checkpoint_dir: None,
            content_type: None,
            storage_class: None,
            acl: None,
            meta: None,
            checkpoint_threshold: None,
            batch_concurrency: None,
            list_concurrency: None,
            recursive_list_mode: None,
            multipart_concurrency: None,
            progress_granularity: None,
            overwrite_strategy: None,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            bandwidth_limit: None,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
        };

        assert!(tos_entries_match_for_sync(&source, &destination, &args));
    }

    // [Review Fix #Sync-LogicBug] 补齐 tos_entries_match_for_sync 的回归测试，
    // 覆盖修复后的早返回路径：size mismatch / exact_timestamps with last_modified /
    // exact_timestamps without last_modified / size-only / 默认。
    fn make_sync_args(size_only: bool, exact_timestamps: bool) -> SyncArgs {
        SyncArgs {
            source: "tos://bucket/source".to_string(),
            destination: "tos://bucket/destination".to_string(),
            delete: false,
            force: false,
            size_only,
            exact_timestamps,
            include_parent: false,
            include: None,
            exclude: None,
            checkpoint_dir: None,
            content_type: None,
            storage_class: None,
            acl: None,
            meta: None,
            checkpoint_threshold: None,
            batch_concurrency: None,
            list_concurrency: None,
            recursive_list_mode: None,
            multipart_concurrency: None,
            progress_granularity: None,
            overwrite_strategy: None,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            bandwidth_limit: None,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
        }
    }

    fn make_entry(size: u64, etag: Option<&str>, last_modified: Option<&str>) -> ObjectEntry {
        ObjectEntry {
            key: "k".to_string(),
            size,
            last_modified: last_modified.map(ToString::to_string),
            etag: etag.map(ToString::to_string),
            storage_class: None,
        }
    }

    #[test]
    fn test_tos_entries_match_size_mismatch_returns_false() {
        // 正常路径：etag 不同 + size 不同 -> 必须 mismatch（修复前因为冗余表达式恒等于
        // size 比较，逻辑虽巧合正确，但行为模糊；修复后该 case 走早返回更清晰）。
        let source = make_entry(10, Some("a"), None);
        let destination = make_entry(20, Some("b"), None);
        let args = make_sync_args(false, false);
        assert!(!tos_entries_match_for_sync(&source, &destination, &args));
    }

    #[test]
    fn test_tos_entries_match_default_size_match_returns_true() {
        // 默认模式（无 size_only/无 exact_timestamps）：etag 不一致但 size 一致 -> match。
        // 修复前同样为 true，本用例锁住该行为防止后续回归。
        let source = make_entry(10, Some("a"), None);
        let destination = make_entry(10, Some("b"), None);
        let args = make_sync_args(false, false);
        assert!(tos_entries_match_for_sync(&source, &destination, &args));
    }

    #[test]
    fn test_tos_entries_match_size_only_returns_true_on_size_match() {
        // size_only=true：仅按 size 判定，etag 不影响（已被前面早返回拦不住时）。
        let source = make_entry(10, Some("a"), None);
        let destination = make_entry(10, Some("b"), None);
        let args = make_sync_args(true, false);
        assert!(tos_entries_match_for_sync(&source, &destination, &args));
    }

    #[test]
    fn test_tos_entries_match_exact_timestamps_with_matching_last_modified() {
        // 异常路径修复：原实现 exact_timestamps=true 直接 false（除非 etag 命中），
        // 即使两端 LastModified 相同也判 mismatch，导致已经一致的对象被反复重传。
        // 修复后：last_modified 字符串相同则 match。
        let source = make_entry(10, Some("a"), Some("2026-05-26T10:00:00Z"));
        let destination = make_entry(10, Some("b"), Some("2026-05-26T10:00:00Z"));
        let args = make_sync_args(false, true);
        assert!(tos_entries_match_for_sync(&source, &destination, &args));
    }

    #[test]
    fn test_tos_entries_match_exact_timestamps_missing_last_modified_is_mismatch() {
        // 边界条件 + 安全保守：exact_timestamps=true 但任一端缺 last_modified
        // 时无法对齐时间，必须保守地判 mismatch（避免误跳过应当同步的对象）。
        let source = make_entry(10, Some("a"), None);
        let destination = make_entry(10, Some("b"), Some("2026-05-26T10:00:00Z"));
        let args = make_sync_args(false, true);
        assert!(!tos_entries_match_for_sync(&source, &destination, &args));

        // 反向同样 mismatch
        let source = make_entry(10, Some("a"), Some("2026-05-26T10:00:00Z"));
        let destination = make_entry(10, Some("b"), None);
        assert!(!tos_entries_match_for_sync(&source, &destination, &args));

        // 时间戳不同也 mismatch
        let source = make_entry(10, Some("a"), Some("2026-05-26T10:00:00Z"));
        let destination = make_entry(10, Some("b"), Some("2026-05-26T11:00:00Z"));
        assert!(!tos_entries_match_for_sync(&source, &destination, &args));
    }
}
