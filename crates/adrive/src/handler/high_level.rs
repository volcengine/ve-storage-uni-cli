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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::io::SeekFrom;
use std::io::{IsTerminal, Write as IoWrite};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crc64fast::Digest;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tos_core::agent::describe::{
    CommandDescription, CommandLayer, CommandParameter, ParameterLocation, RelatedCommands,
    RiskLevel,
};
use tos_core::agent::dryrun::Impact;
use tos_core::agent::envelope::{Envelope, PaginationInfo};
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::agent::output::OutputFormat;
use tos_core::infra::config::{
    DEFAULT_BATCH_CONCURRENCY, DEFAULT_LIST_CONCURRENCY, DEFAULT_MULTIPART_CONCURRENCY,
    DEFAULT_OVERWRITE_STRATEGY, DEFAULT_PROGRESS_GRANULARITY, DEFAULT_TOS_BATCH_REPORT_FORMAT,
    DEFAULT_TOS_PROGRESS_ENABLED, DEFAULT_TRANSFER_CHECKPOINT_THRESHOLD,
};

use crate::domain::client::{Client as IdsClient, Error as IdsSdkError};
use crate::domain::rate_limiter::RateLimiter;
use crate::domain::types::{
    AbortMultipartUploadInput, Body as IdsBody, CompleteMultipartUploadInput, CopyFileInput,
    CreateFolderInput, CreateInstanceInput, CreateSpaceInput, DeleteFileInput, DeleteFolderInput,
    DeleteInstanceInput, DeleteSpaceInput, FileInfo, FolderInfo, GetFileInput, GetFileOutput,
    GetInstanceByNameInput, GetInstanceInput, GetSpaceByNameInput, GetSpaceInput, HeadFileInput,
    InitiateMultipartUploadInput, InstanceInfo, ListFilesInput, ListInstancesInput,
    ListSpacesInput, PartInfo, PutFileInput, RenameFileInput, RenameFolderInput, SpaceInfo,
    UploadPartInput,
};

use crate::cli::high_level::*;
use crate::cli::ADriveCommand;
use crate::handler::common::{
    build_ids_client, build_profile, ensure_force_for_destructive, map_ids_error, output_envelope,
    output_result_with_columns, parse_adrive_uri, public_adrive_command_path, resolve_target,
    ParsedADriveUri,
};
use crate::registry::{find_capability, RegistryParameter};

const ADRIVE_EXAMPLE_PREFIX_ENV: &str = "VE_STORAGE_UNI_ADRIVE_EXAMPLE_PREFIX";
const INSTANCE_TABLE_COLUMNS: &[&str] = &[
    "instance_id",
    "name",
    "display_name",
    "status",
    "run_state",
    "space_count",
    "created_at",
    "updated_at",
];
const SPACE_TABLE_COLUMNS: &[&str] = &[
    "space_id",
    "name",
    "display_name",
    "owner_type",
    "owner_id",
    "created_at",
    "updated_at",
];
const FILE_TABLE_COLUMNS: &[&str] = &["file_path", "size", "file_type", "updated_at", "is_folder"];
const ADRIVE_SINGLE_PUT_LIMIT: u64 = 64 * 1024 * 1024;
const ADRIVE_MULTIPART_PART_SIZE: u64 = 16 * 1024 * 1024;
const DEFAULT_BATCH_FILE_ROLLOVER_BYTES: u64 = 50 * 1024 * 1024;
const ADRIVE_DEFAULT_CHECKPOINT_DIR: &str = "~/.tos/checkpoints/ve-adrive";
const ADRIVE_LEGACY_CHECKPOINT_DIR: &str = "~/.tos/checkpoints/adrive";
const ADRIVE_DEFAULT_BATCH_REPORT_DIR: &str = "~/.tos/reports/ve-adrive";
const SYNC_DELETE_EXTRA_FILE: &str = "delete-extra";
const SYNC_DELETE_EXTRA_FOLDER: &str = "delete-extra-folder";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectiveProgressGranularity {
    Part,
    Byte,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EffectiveOverwriteStrategy {
    Force,
    NoClobber,
    Newer,
}

#[derive(Debug, Clone, Copy)]
struct OutputRenderPlan {
    enabled: bool,
    disabled_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Copy)]
enum FindSizeFilter {
    MinInclusive(u64),
    MaxInclusive(u64),
    Equal(u64),
}

#[derive(Debug, Clone, Copy)]
enum FindMtimeFilterMillis {
    WithinLast(u64),
    OlderThanOrEqual(u64),
    EqualAge {
        duration_millis: u64,
        unit_millis: u64,
    },
}

#[derive(Debug, Clone, Copy)]
struct RelativeDurationMillis {
    duration_millis: u64,
    unit_millis: u64,
}

#[derive(Debug, Clone, Copy)]
struct TransferRuntimeConfig {
    checkpoint_threshold: u64,
    batch_concurrency: usize,
    list_concurrency: usize,
    multipart_concurrency: usize,
    progress_granularity: EffectiveProgressGranularity,
    overwrite_strategy: EffectiveOverwriteStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchItemResult {
    source: String,
    destination: Option<String>,
    operation: String,
    status: String,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchManifestItem {
    operation: String,
    source: String,
    destination: Option<String>,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    etag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crc64: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchManifest {
    object_count: usize,
    total_size: u64,
    items: Vec<BatchManifestItem>,
}

impl BatchManifest {
    fn from_items(items: Vec<BatchManifestItem>) -> Self {
        Self {
            object_count: items.len(),
            total_size: items.iter().map(|item| item.size).sum(),
            items,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatchReport {
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest: Option<BatchManifest>,
    total: usize,
    succeeded: usize,
    failed: usize,
    skipped: usize,
    items: Vec<BatchItemResult>,
}

#[derive(Debug, Clone, Serialize)]
struct DuDistributionBucket {
    count: u64,
    bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
struct DuFileSample {
    file_path: String,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    storage_class: Option<String>,
}

#[derive(Debug, Clone)]
struct DuAccumulator {
    file_count: u64,
    folder_count: u64,
    folder_prefixes: HashSet<String>,
    total_bytes: u64,
    manifest_items: Vec<BatchManifestItem>,
    file_types: BTreeMap<String, DuDistributionBucket>,
    directories: BTreeMap<String, DuDistributionBucket>,
    size_histogram: BTreeMap<&'static str, DuDistributionBucket>,
    storage_classes: BTreeMap<String, DuDistributionBucket>,
    largest_files: Vec<DuFileSample>,
    oldest_files: Vec<DuFileSample>,
    top_k: usize,
}

#[derive(Debug, Default)]
struct SyncDesiredRelativePaths {
    files: BTreeSet<String>,
    folders: BTreeSet<String>,
}

impl BatchReport {
    fn new(command: &str) -> Self {
        Self {
            command: command.to_string(),
            manifest: None,
            total: 0,
            succeeded: 0,
            failed: 0,
            skipped: 0,
            items: Vec::new(),
        }
    }

    fn set_manifest(&mut self, items: Vec<BatchManifestItem>) {
        self.manifest = Some(BatchManifest::from_items(items));
    }

    fn push_success(&mut self, source: String, destination: Option<String>, operation: &str) {
        self.total += 1;
        self.succeeded += 1;
        self.items.push(BatchItemResult {
            source,
            destination,
            operation: operation.to_string(),
            status: "succeeded".to_string(),
            error: None,
        });
    }

    fn push_failure(
        &mut self,
        source: String,
        destination: Option<String>,
        operation: &str,
        error: impl ToString,
    ) {
        self.total += 1;
        self.failed += 1;
        self.items.push(BatchItemResult {
            source,
            destination,
            operation: operation.to_string(),
            status: "failed".to_string(),
            error: Some(error.to_string()),
        });
    }

    fn push_skipped(&mut self, source: String, destination: Option<String>, operation: &str) {
        self.total += 1;
        self.skipped += 1;
        self.items.push(BatchItemResult {
            source,
            destination,
            operation: operation.to_string(),
            status: "skipped".to_string(),
            error: None,
        });
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BatchCheckpoint {
    completed: HashSet<String>,
}

type ADriveBatchFutureOutput = (
    String,
    Option<String>,
    String,
    u64,
    &'static str,
    Result<(), CliError>,
);
type ADriveBatchFuture<'a> = Pin<Box<dyn Future<Output = ADriveBatchFutureOutput> + 'a>>;

struct ADriveMoveFutureOutput {
    copy_source: String,
    copy_destination: Option<String>,
    copy_operation: &'static str,
    copy_result: Result<(), CliError>,
    delete_source: String,
    delete_result: Option<Result<(), CliError>>,
}

type ADriveMoveFuture<'a> = Pin<Box<dyn Future<Output = ADriveMoveFutureOutput> + 'a>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UploadPartCheckpoint {
    part_number: i32,
    etag: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UploadCheckpoint {
    instance: String,
    space: String,
    file_path: String,
    source_path: String,
    file_size: u64,
    part_size: u64,
    upload_id: Option<String>,
    completed_parts: Vec<UploadPartCheckpoint>,
}

struct ADriveUploadCheckpointRef {
    path: PathBuf,
    checkpoint: UploadCheckpoint,
}

/// Handle High-Level ADrive commands.
///
/// Keeps describe/dry-run paths side-effect free, routes real execution through
/// the internal IDS REST client.
pub async fn handle_high_level_command(
    global: &GlobalArgs,
    command: &ADriveCommand,
) -> Result<i32, CliError> {
    if global.describe {
        let desc = describe_command(command);
        output_envelope(global, &Envelope::success(desc.command.clone(), desc))?;
        return Ok(0);
    }

    if global.dry_run {
        let plan = build_plan(global, command).await?;
        output_envelope(
            global,
            &Envelope::success(crate::cli::command_path(command), plan),
        )?;
        return Ok(0);
    }

    if let ADriveCommand::Mv(args) = command {
        // [Review Fix #8] Move deletes the source after the copy/rename phase;
        // automation must confirm that source, not the destination.
        ensure_force_for_destructive(global, args.force, "ve-adrive mv", &args.source)?;
        enforce_critical_confirmation(global, args.force, "ve-adrive mv", &args.source)?;
    }
    if let ADriveCommand::Rm(args) = command {
        let target = resolve_rm_target(args)?;
        let display_target = format_target(&target);
        ensure_force_for_destructive(global, args.force, "ve-adrive rm", &display_target)?;
        enforce_critical_confirmation(global, args.force, "ve-adrive rm", &display_target)?;
    }
    if let ADriveCommand::Del(args) = command {
        let target = resolve_delete_target(args)?;
        let display_target = target.display();
        ensure_force_for_destructive(global, args.force, "ve-adrive del", &display_target)?;
        enforce_critical_confirmation(global, args.force, "ve-adrive del", &display_target)?;
    }
    if let ADriveCommand::Sync(args) = command {
        if args.delete {
            // [Review Fix #2] Gate sync deletions before any copy/upload work starts.
            ensure_force_for_destructive(
                global,
                args.force,
                "ve-adrive sync --delete",
                &args.destination,
            )?;
            enforce_critical_confirmation(
                global,
                args.force,
                "ve-adrive sync --delete",
                &args.destination,
            )?;
        }
    }

    prevalidate_command(command)?;
    execute_command(global, command).await
}

fn enforce_critical_confirmation(
    global: &GlobalArgs,
    force: bool,
    command: &str,
    target: &str,
) -> Result<(), CliError> {
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let stderr_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let can_prompt = stdin_tty && stderr_tty && !global.quiet;
    if can_prompt {
        return Ok(());
    }

    if !force {
        return Err(CliError::ValidationError(format!(
            "critical delete command '{}' targeting '{}' requires --force and --confirm {} in non-interactive execution",
            command, target, target
        )));
    }

    // [Review Fix #2] Align ADrive critical delete high-level commands with TOS:
    // `--force` confirms destructive intent, while `--confirm <target>` confirms
    // the exact public adrive:// resource in non-interactive executions.
    if let Some(provided) = global.confirm.as_deref() {
        if provided == target {
            return Ok(());
        }
        return Err(CliError::ValidationError(format!(
            "--confirm '{}' does not match the critical resource '{}' for {}",
            provided, target, command
        )));
    }

    Err(CliError::ValidationError(format!(
        "critical delete command '{}' targeting '{}' requires --confirm {} in non-interactive execution",
        command, target, target
    )))
}

fn prevalidate_command(command: &ADriveCommand) -> Result<(), CliError> {
    match command {
        ADriveCommand::Cp(args) => {
            validate_transfer_endpoints("ve-adrive cp", &args.source, &args.destination)
        }
        ADriveCommand::Mv(args) => {
            validate_transfer_endpoints("ve-adrive mv", &args.source, &args.destination)
        }
        ADriveCommand::Sync(args) => {
            validate_transfer_endpoints("ve-adrive sync", &args.source, &args.destination)
        }
        ADriveCommand::Crt(args) => {
            resolve_create_target(args)?;
            Ok(())
        }
        ADriveCommand::Del(args) => {
            resolve_delete_target(args)?;
            Ok(())
        }
        ADriveCommand::Stat(args) => {
            let target = resolve_hierarchical_target(
                args.path.as_deref(),
                args.instance.as_deref(),
                args.space.as_deref(),
                args.folder.as_deref(),
                args.file.as_deref(),
                false,
            )?;
            if matches!(target, ADriveTarget::Instances) {
                return Err(CliError::ValidationError(
                    "ve-adrive stat requires an instance, space, file, or folder target"
                        .to_string(),
                ));
            }
            Ok(())
        }
        ADriveCommand::Du(args) => {
            resolve_target(
                args.path.as_deref(),
                args.instance.as_deref(),
                args.space.as_deref(),
                args.folder.as_deref(),
                None,
            )?;
            Ok(())
        }
        ADriveCommand::Find(args) => {
            resolve_target(
                args.path.as_deref(),
                args.instance.as_deref(),
                args.space.as_deref(),
                args.folder.as_deref(),
                None,
            )?;
            Ok(())
        }
        ADriveCommand::Cat(args) => {
            let target = resolve_target(
                args.path.as_deref(),
                args.instance.as_deref(),
                args.space.as_deref(),
                args.folder.as_deref(),
                args.file.as_deref(),
            )?;
            parse_file_target(target)?;
            Ok(())
        }
        ADriveCommand::Put(args) => {
            resolve_put_file_target(args)?;
            Ok(())
        }
        ADriveCommand::Mkdir(args) => {
            if args.path.is_some() {
                parse_adrive_uri(args.path.as_deref().unwrap_or_default(), false)?;
                return Ok(());
            }
            if args.instance.is_none() || args.space.is_none() || args.folder.is_none() {
                return Err(CliError::ValidationError(
                    "ve-adrive mkdir requires adrive://instance/space/folder or --instance --space --folder"
                        .to_string(),
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_transfer_endpoints(
    command: &str,
    source: &str,
    destination: &str,
) -> Result<(), CliError> {
    let source_is_remote = source.starts_with("adrive://");
    let dest_is_remote = destination.starts_with("adrive://");
    if !source_is_remote && !dest_is_remote {
        return Err(CliError::ValidationError(format!(
            "{command} requires at least one endpoint to be adrive://instance/space[/path]"
        )));
    }
    if source_is_remote {
        parse_adrive_uri(source, false)?;
    }
    if dest_is_remote {
        parse_adrive_uri(destination, false)?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum ADriveTarget {
    Instances,
    Instance { instance: String },
    Space { instance: String, space: String },
    Path(ParsedADriveUri),
}

#[derive(Debug, Clone)]
enum ADriveCreateTarget {
    Instance { name: String },
    Space { instance: String, name: String },
}

impl ADriveTarget {
    fn display(&self) -> String {
        match self {
            ADriveTarget::Instances => "adrive://".to_string(),
            ADriveTarget::Instance { instance } => format!("adrive://{instance}"),
            ADriveTarget::Space { instance, space } => format!("adrive://{instance}/{space}"),
            ADriveTarget::Path(target) => format_target(target),
        }
    }
}

async fn resolve_remote_uri_by_name(
    client: &IdsClient,
    uri: &str,
    allow_instance_only: bool,
) -> Result<String, CliError> {
    let target = parse_adrive_uri(uri, allow_instance_only)?;
    let resolved = resolve_parsed_target_by_name(client, target).await?;
    Ok(format_target(&resolved))
}

async fn resolve_parsed_target_by_name(
    client: &IdsClient,
    target: ParsedADriveUri,
) -> Result<ParsedADriveUri, CliError> {
    let instance_id = resolve_instance_segment_by_name(client, &target.instance).await?;
    if target.space.is_empty() {
        return Ok(ParsedADriveUri {
            instance: instance_id,
            space: String::new(),
            path: target.path,
        });
    }
    let space_id = resolve_space_segment_by_name(client, &instance_id, &target.space).await?;
    Ok(ParsedADriveUri {
        instance: instance_id,
        space: space_id,
        path: target.path,
    })
}

async fn resolve_target_parts_by_name(
    client: &IdsClient,
    path: &mut Option<String>,
    instance: &mut Option<String>,
    space: &mut Option<String>,
    allow_instance_only: bool,
) -> Result<(), CliError> {
    if let Some(uri) = path.as_deref() {
        *path = Some(resolve_remote_uri_by_name(client, uri, allow_instance_only).await?);
        return Ok(());
    }

    let Some(instance_value) = instance.clone() else {
        return Ok(());
    };
    let instance_id = resolve_instance_segment_by_name(client, &instance_value).await?;
    *instance = Some(instance_id.clone());
    if let Some(space_value) = space.clone() {
        let space_id = resolve_space_segment_by_name(client, &instance_id, &space_value).await?;
        *space = Some(space_id);
    }
    Ok(())
}

async fn resolve_instance_segment_by_name(
    client: &IdsClient,
    instance: &str,
) -> Result<String, CliError> {
    match get_instance_info_by_name(client, instance).await {
        Ok(info) => Ok(non_empty_or_original(info.instance_id, instance)),
        // [Review Fix #ADrive-ByName-1] `--by-name` resolves names via GetInstanceByName,
        // but must stay compatible with callers that already pass an id.
        Err(CliError::ResourceNotFound(_)) => Ok(instance.to_string()),
        Err(err) => Err(err),
    }
}

async fn resolve_space_segment_by_name(
    client: &IdsClient,
    instance_id: &str,
    space: &str,
) -> Result<String, CliError> {
    match get_space_info_by_name(client, instance_id, space).await {
        Ok(info) => Ok(non_empty_or_original(info.space_id, space)),
        // [Review Fix #ADrive-ByName-2] Preserve id inputs under `--by-name`; a real typo
        // still fails when the later operation uses the unresolved segment.
        Err(CliError::ResourceNotFound(_)) => Ok(space.to_string()),
        Err(err) => Err(err),
    }
}

fn non_empty_or_original(value: String, original: &str) -> String {
    if value.is_empty() {
        original.to_string()
    } else {
        value
    }
}

async fn resolve_cp_args_by_name(client: &IdsClient, args: &CpArgs) -> Result<CpArgs, CliError> {
    let mut resolved = args.clone();
    if !resolved.by_name {
        return Ok(resolved);
    }
    if resolved.source.starts_with("adrive://") {
        resolved.source = resolve_remote_uri_by_name(client, &resolved.source, false).await?;
    }
    if resolved.destination.starts_with("adrive://") {
        resolved.destination =
            resolve_remote_uri_by_name(client, &resolved.destination, false).await?;
    }
    resolved.by_name = false;
    Ok(resolved)
}

async fn resolve_mv_args_by_name(client: &IdsClient, args: &MvArgs) -> Result<MvArgs, CliError> {
    let mut resolved = args.clone();
    if !resolved.by_name {
        return Ok(resolved);
    }
    if resolved.source.starts_with("adrive://") {
        resolved.source = resolve_remote_uri_by_name(client, &resolved.source, false).await?;
    }
    if resolved.destination.starts_with("adrive://") {
        resolved.destination =
            resolve_remote_uri_by_name(client, &resolved.destination, false).await?;
    }
    resolved.by_name = false;
    Ok(resolved)
}

async fn resolve_sync_args_by_name(
    client: &IdsClient,
    args: &SyncArgs,
) -> Result<SyncArgs, CliError> {
    let mut resolved = args.clone();
    if !resolved.by_name {
        return Ok(resolved);
    }
    if resolved.source.starts_with("adrive://") {
        resolved.source = resolve_remote_uri_by_name(client, &resolved.source, false).await?;
    }
    if resolved.destination.starts_with("adrive://") {
        resolved.destination =
            resolve_remote_uri_by_name(client, &resolved.destination, false).await?;
    }
    resolved.by_name = false;
    Ok(resolved)
}

async fn resolve_create_args_by_name(
    client: &IdsClient,
    args: &CreateArgs,
) -> Result<CreateArgs, CliError> {
    let mut resolved = args.clone();
    if !resolved.by_name {
        return Ok(resolved);
    }
    if let ADriveCreateTarget::Space { instance, .. } = resolve_create_target(args)? {
        let instance_info = get_instance_info(client, &instance).await?;
        if let Some(path) = resolved.path.as_deref() {
            let parsed = parse_adrive_uri(path, true)?;
            resolved.path = Some(format_target(&ParsedADriveUri {
                instance: instance_info.instance_id,
                space: parsed.space,
                path: parsed.path,
            }));
        } else {
            resolved.instance = Some(instance_info.instance_id);
        }
    }
    resolved.by_name = false;
    Ok(resolved)
}

async fn resolve_delete_args_by_name(
    client: &IdsClient,
    args: &DeleteArgs,
) -> Result<DeleteArgs, CliError> {
    let mut resolved = args.clone();
    if resolved.by_name {
        resolve_target_parts_by_name(
            client,
            &mut resolved.path,
            &mut resolved.instance,
            &mut resolved.space,
            true,
        )
        .await?;
        resolved.by_name = false;
    }
    Ok(resolved)
}

async fn resolve_rm_args_by_name(client: &IdsClient, args: &RmArgs) -> Result<RmArgs, CliError> {
    let mut resolved = args.clone();
    if resolved.by_name {
        resolve_target_parts_by_name(
            client,
            &mut resolved.path,
            &mut resolved.instance,
            &mut resolved.space,
            false,
        )
        .await?;
        resolved.by_name = false;
    }
    Ok(resolved)
}

async fn resolve_ls_args_by_name(client: &IdsClient, args: &LsArgs) -> Result<LsArgs, CliError> {
    let mut resolved = args.clone();
    if resolved.by_name {
        resolve_target_parts_by_name(
            client,
            &mut resolved.path,
            &mut resolved.instance,
            &mut resolved.space,
            true,
        )
        .await?;
        resolved.by_name = false;
    }
    Ok(resolved)
}

async fn resolve_stat_args_by_name(
    client: &IdsClient,
    args: &StatArgs,
) -> Result<StatArgs, CliError> {
    let mut resolved = args.clone();
    if resolved.by_name {
        resolve_target_parts_by_name(
            client,
            &mut resolved.path,
            &mut resolved.instance,
            &mut resolved.space,
            true,
        )
        .await?;
        resolved.by_name = false;
    }
    Ok(resolved)
}

async fn resolve_du_args_by_name(client: &IdsClient, args: &DuArgs) -> Result<DuArgs, CliError> {
    let mut resolved = args.clone();
    if resolved.by_name {
        resolve_target_parts_by_name(
            client,
            &mut resolved.path,
            &mut resolved.instance,
            &mut resolved.space,
            false,
        )
        .await?;
        resolved.by_name = false;
    }
    Ok(resolved)
}

async fn resolve_find_args_by_name(
    client: &IdsClient,
    args: &FindArgs,
) -> Result<FindArgs, CliError> {
    let mut resolved = args.clone();
    if resolved.by_name {
        resolve_target_parts_by_name(
            client,
            &mut resolved.path,
            &mut resolved.instance,
            &mut resolved.space,
            false,
        )
        .await?;
        resolved.by_name = false;
    }
    Ok(resolved)
}

async fn resolve_cat_args_by_name(client: &IdsClient, args: &CatArgs) -> Result<CatArgs, CliError> {
    let mut resolved = args.clone();
    if resolved.by_name {
        resolve_target_parts_by_name(
            client,
            &mut resolved.path,
            &mut resolved.instance,
            &mut resolved.space,
            false,
        )
        .await?;
        resolved.by_name = false;
    }
    Ok(resolved)
}

async fn resolve_put_args_by_name(client: &IdsClient, args: &PutArgs) -> Result<PutArgs, CliError> {
    let mut resolved = args.clone();
    if resolved.by_name {
        resolve_target_parts_by_name(
            client,
            &mut resolved.path,
            &mut resolved.instance,
            &mut resolved.space,
            false,
        )
        .await?;
        resolved.by_name = false;
    }
    Ok(resolved)
}

async fn resolve_mkdir_args_by_name(
    client: &IdsClient,
    args: &MkdirArgs,
) -> Result<MkdirArgs, CliError> {
    let mut resolved = args.clone();
    if resolved.by_name {
        resolve_target_parts_by_name(
            client,
            &mut resolved.path,
            &mut resolved.instance,
            &mut resolved.space,
            false,
        )
        .await?;
        resolved.by_name = false;
    }
    Ok(resolved)
}

async fn execute_command(global: &GlobalArgs, command: &ADriveCommand) -> Result<i32, CliError> {
    let client = build_ids_client(global)?;

    match command {
        ADriveCommand::Cp(args) => execute_cp(global, &client, args).await,
        ADriveCommand::Mv(args) => execute_mv(global, &client, args).await,
        ADriveCommand::Sync(args) => execute_sync(global, &client, args).await,
        ADriveCommand::Crt(args) => execute_create(global, &client, args).await,
        ADriveCommand::Del(args) => execute_delete(global, &client, args).await,
        ADriveCommand::Rm(args) => execute_rm(global, &client, args).await,
        ADriveCommand::Ls(args) => execute_ls(global, &client, args).await,
        ADriveCommand::Stat(args) => execute_stat(global, &client, args).await,
        ADriveCommand::Du(args) => execute_du(global, &client, args).await,
        ADriveCommand::Find(args) => execute_find(global, &client, args).await,
        ADriveCommand::Cat(args) => execute_cat(global, &client, args).await,
        ADriveCommand::Put(args) => execute_put(global, &client, args).await,
        ADriveCommand::Mkdir(args) => execute_mkdir(global, &client, args).await,
        _ => Err(CliError::ValidationError(
            "unsupported high-level command".to_string(),
        )),
    }
}

// ─── Execute Implementations ────────────────────────────────────────────────

async fn execute_cp(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &CpArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_cp_args_by_name(client, args).await?;
    let args = &resolved_args;
    let runtime = effective_cp_runtime_config(global, args)?;
    if args.recursive {
        return execute_recursive_cp(global, client, args, runtime).await;
    }
    reject_single_transfer_artifacts(
        "ve-adrive cp",
        args.report_path.as_deref(),
        args.report_failures_only,
        args.manifest_path.as_deref(),
        args.no_manifest,
        args.batch_concurrency,
        args.list_concurrency,
    )?;

    let source_is_remote = args.source.starts_with("adrive://");
    let dest_is_remote = args.destination.starts_with("adrive://");
    match (source_is_remote, dest_is_remote) {
        (false, true) => upload_file(global, client, args, runtime).await,
        (true, false) => download_file(global, client, args, runtime).await,
        (true, true) => copy_remote_file(global, client, args, runtime).await,
        (false, false) => Err(CliError::ValidationError(
            "ve-adrive cp requires one side to be adrive://instance/space/path".to_string(),
        )),
    }
}

async fn execute_recursive_cp(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
) -> Result<i32, CliError> {
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let list_echo_enabled = effective_list_echo_enabled(global, args.list_echo, args.no_list_echo);
    if args.no_manifest {
        let report_path =
            effective_report_path(global, args.report_path.as_deref(), "ve-adrive cp")?;
        let manifest_path = effective_optional_manifest_path(
            global,
            args.manifest_path.as_deref(),
            args.no_manifest,
            "ve-adrive cp",
        )?;
        return execute_recursive_cp_streaming_no_manifest(
            global,
            client,
            args,
            runtime,
            report_path,
            manifest_path,
            progress_enabled,
        )
        .await;
    }
    let report =
        recursive_cp_report(client, args, runtime, list_echo_enabled, progress_enabled).await?;
    let report_path = effective_report_path(global, args.report_path.as_deref(), "ve-adrive cp")?;
    let manifest_path = effective_optional_manifest_path(
        global,
        args.manifest_path.as_deref(),
        args.no_manifest,
        "ve-adrive cp",
    )?;
    write_batch_report(
        report_path.as_deref(),
        &report,
        args.report_failures_only,
        manifest_path.is_some(),
    )
    .await?;
    write_adrive_manifest_file(
        manifest_path.as_deref(),
        "ve-adrive cp",
        report.manifest.as_ref(),
    )
    .await?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive cp",
            json!({
                "operation": "recursive-copy",
                "source": args.source,
                "destination": args.destination,
                "summary": batch_summary(&report),
                "report_path": report_path,
                "manifest_path": manifest_path,
                "status": if report.failed == 0 { "succeeded" } else { "partial_failure" },
            }),
        ),
    )?;
    if report.failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

async fn recursive_cp_report(
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
    list_echo_enabled: bool,
    progress_enabled: bool,
) -> Result<BatchReport, CliError> {
    match (
        args.source.starts_with("adrive://"),
        args.destination.starts_with("adrive://"),
    ) {
        (false, true) => {
            upload_directory_recursive(client, args, runtime, list_echo_enabled, progress_enabled)
                .await
        }
        (true, false) => {
            download_directory_recursive(client, args, runtime, list_echo_enabled, progress_enabled)
                .await
        }
        (true, true) => {
            copy_remote_recursive(client, args, runtime, list_echo_enabled, progress_enabled).await
        }
        (false, false) => {
            return Err(CliError::ValidationError(
                "ve-adrive cp --recursive requires one side to be adrive://instance/space/path"
                    .to_string(),
            ));
        }
    }
}

async fn execute_recursive_cp_streaming_no_manifest(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
    report_path: Option<String>,
    manifest_path: Option<String>,
    progress_enabled: bool,
) -> Result<i32, CliError> {
    let (report, stream_result, drain_result) = recursive_cp_streaming_no_manifest_report(
        client,
        args,
        runtime,
        "ve-adrive cp",
        "ve-adrive cp",
        progress_enabled,
    )
    .await?;
    let failed = report.failed;
    write_batch_report(
        report_path.as_deref(),
        &report,
        args.report_failures_only,
        false,
    )
    .await?;
    stream_result?;
    drain_result?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive cp",
            json!({
                "operation": "recursive-copy",
                "source": args.source,
                "destination": args.destination,
                "summary": batch_summary(&report),
                "report_path": report_path,
                "manifest_path": manifest_path,
                "status": if failed == 0 { "succeeded" } else { "partial_failure" },
            }),
        ),
    )?;
    if failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

async fn recursive_cp_streaming_no_manifest_report(
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
    progress_label: &str,
    report_command: &str,
    progress_enabled: bool,
) -> Result<(BatchReport, Result<(), CliError>, Result<(), CliError>), CliError> {
    let progress = streaming_batch_progress(progress_enabled, progress_label);
    let checkpoint_path = checkpoint_path_for_cp(args)?;
    let checkpoint = load_batch_checkpoint(checkpoint_path.as_deref()).await?;
    let mut report = BatchReport::new(report_command);
    // [Review Fix #SyncNoManifest] Share streaming recursive copy with sync so
    // `--no-manifest` does not silently fall back to the pre-scan copy path.
    let (stream_result, drain_result) = {
        let mut context = ADriveStreamCpContext {
            client,
            args,
            runtime,
            progress: &progress,
            checkpoint_path,
            checkpoint,
            report: &mut report,
            in_flight: FuturesUnordered::new(),
            limit: runtime.batch_concurrency.max(1),
        };
        let result = match (
            args.source.starts_with("adrive://"),
            args.destination.starts_with("adrive://"),
        ) {
            (false, true) => stream_adrive_upload_no_manifest(&mut context).await,
            (true, false) => stream_adrive_download_no_manifest(&mut context).await,
            (true, true) => stream_adrive_remote_copy_no_manifest(&mut context).await,
            (false, false) => Err(CliError::ValidationError(
                "ve-adrive cp --recursive requires one side to be adrive://instance/space/path"
                    .to_string(),
            )),
        };
        let drain = context.drain_all().await;
        (result, drain)
    };
    finish_streaming_progress(progress, report.total as u64);
    Ok((report, stream_result, drain_result))
}

struct ADriveStreamCpContext<'a> {
    client: &'a IdsClient,
    args: &'a CpArgs,
    runtime: TransferRuntimeConfig,
    progress: &'a Option<ProgressBar>,
    checkpoint_path: Option<PathBuf>,
    checkpoint: BatchCheckpoint,
    report: &'a mut BatchReport,
    in_flight: FuturesUnordered<ADriveBatchFuture<'a>>,
    limit: usize,
}

impl<'a> ADriveStreamCpContext<'a> {
    async fn queue(&mut self, task: ADriveBatchFuture<'a>) -> Result<(), CliError> {
        while self.in_flight.len() >= self.limit {
            if !self.drain_one().await? {
                break;
            }
        }
        self.in_flight.push(task);
        Ok(())
    }

    async fn drain_one(&mut self) -> Result<bool, CliError> {
        let Some((source, destination, item_key, progress_units, operation, result)) =
            self.in_flight.next().await
        else {
            return Ok(false);
        };
        record_adrive_batch_result(
            self.report,
            self.checkpoint_path.as_deref(),
            &mut self.checkpoint,
            self.progress,
            operation,
            source,
            destination,
            item_key,
            progress_units,
            result,
        )
        .await?;
        Ok(true)
    }

    async fn drain_all(&mut self) -> Result<(), CliError> {
        while self.drain_one().await? {}
        Ok(())
    }
}

async fn stream_adrive_upload_no_manifest(
    context: &mut ADriveStreamCpContext<'_>,
) -> Result<(), CliError> {
    let destination = parse_adrive_uri(&context.args.destination, false)?;
    let source_root = PathBuf::from(&context.args.source);
    if !source_root.is_dir() {
        return Err(CliError::ValidationError(format!(
            "recursive upload source must be a directory: {}",
            context.args.source
        )));
    }
    let parent_prefix =
        recursive_adrive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let mut pending = vec![source_root.clone()];
    while let Some(directory) = pending.pop() {
        let mut child_directories = Vec::new();
        for entry in sorted_adrive_read_dir_entries(&directory)? {
            let path = entry.path();
            if path.is_dir() {
                child_directories.push(path);
                continue;
            }
            if path.is_file() {
                queue_adrive_upload_file(
                    context,
                    &destination,
                    &source_root,
                    parent_prefix.as_deref(),
                    path,
                )
                .await?;
            }
        }
        child_directories.sort();
        child_directories.reverse();
        pending.extend(child_directories);
    }
    Ok(())
}

async fn queue_adrive_upload_file(
    context: &mut ADriveStreamCpContext<'_>,
    destination: &ParsedADriveUri,
    source_root: &Path,
    parent_prefix: Option<&str>,
    file: PathBuf,
) -> Result<(), CliError> {
    let relative = file.strip_prefix(source_root).map_err(|err| {
        CliError::ValidationError(format!("failed to derive relative path: {}", err))
    })?;
    let source_relative = normalize_local_relative_path(relative)?;
    let relative = prepend_adrive_parent_prefix(&source_relative, parent_prefix);
    if !path_matches_filters(
        &relative,
        context.args.include.as_deref(),
        context.args.exclude.as_deref(),
    ) {
        return Ok(());
    }
    let remote_path = join_remote_path(&destination.path, &relative);
    let remote_target = format_target(&ParsedADriveUri {
        instance: destination.instance.clone(),
        space: destination.space.clone(),
        path: remote_path.clone(),
    });
    let source_label = file.display().to_string();
    let source_updated_at = local_modified_millis(&file)?;
    if should_skip_remote_destination(
        context.client,
        &destination.instance,
        &destination.space,
        &remote_path,
        Some(source_updated_at),
        context.runtime.overwrite_strategy,
    )
    .await?
    {
        context
            .report
            .push_skipped(source_label, Some(remote_target), "upload");
        tick_progress(context.progress);
        return Ok(());
    }
    let item_key = checkpoint_item_key("upload", &source_label, Some(&remote_target));
    if context.checkpoint.completed.contains(&item_key) {
        context
            .report
            .push_skipped(source_label, Some(remote_target), "upload");
        tick_progress(context.progress);
        return Ok(());
    }
    let instance = destination.instance.clone();
    let space = destination.space.clone();
    let checkpoint_enabled = context.args.checkpoint;
    let checkpoint_dir = context.args.checkpoint_dir.clone();
    let bandwidth_limit = context.args.bandwidth_limit.clone();
    let runtime = context.runtime;
    let client = context.client;
    context
        .queue(Box::pin(async move {
            let result = upload_adrive_file_for_batch(
                client,
                &instance,
                &space,
                &remote_path,
                &file,
                checkpoint_enabled,
                checkpoint_dir.as_deref(),
                bandwidth_limit.as_deref(),
                runtime,
            )
            .await;
            (
                source_label,
                Some(remote_target),
                item_key,
                1,
                "upload",
                result,
            )
        }))
        .await
}

async fn stream_adrive_download_no_manifest(
    context: &mut ADriveStreamCpContext<'_>,
) -> Result<(), CliError> {
    let source = parse_adrive_uri(&context.args.source, false)?;
    let destination_root = PathBuf::from(&context.args.destination);
    let source_prefix = trim_folder_prefix(&source.path);
    let parent_prefix =
        recursive_adrive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let mut pending_prefixes = vec![source_prefix.clone()];
    let mut seen_prefixes = HashSet::new();
    while let Some(prefix) = pending_prefixes.pop() {
        if !seen_prefixes.insert(prefix.clone()) {
            continue;
        }
        let mut marker = None;
        loop {
            let mut input = ListFilesInput::new(&source.instance, &source.space)
                .with_limit(1000)
                .with_delimiter("/");
            if !prefix.is_empty() {
                input = input.with_prefix(&prefix);
            }
            if let Some(value) = marker.take() {
                input = input.with_marker(value);
            }
            let out = context
                .client
                .list_files(&input)
                .await
                .map_err(map_ids_error)?;
            for file in out.files {
                queue_adrive_download_file(
                    context,
                    &source,
                    &source_prefix,
                    parent_prefix.as_deref(),
                    &destination_root,
                    file,
                )
                .await?;
            }
            for folder in out.folders {
                pending_prefixes.push(trim_folder_prefix(&folder.folder));
            }
            if !out.is_truncated || out.next_marker.is_empty() {
                break;
            }
            marker = Some(out.next_marker);
        }
    }
    Ok(())
}

async fn queue_adrive_download_file(
    context: &mut ADriveStreamCpContext<'_>,
    source: &ParsedADriveUri,
    source_prefix: &str,
    parent_prefix: Option<&str>,
    destination_root: &Path,
    file: FileInfo,
) -> Result<(), CliError> {
    let source_relative = remote_relative_path(&file.file_path, source_prefix);
    let relative = prepend_adrive_parent_prefix(&source_relative, parent_prefix);
    if !path_matches_filters(
        &relative,
        context.args.include.as_deref(),
        context.args.exclude.as_deref(),
    ) {
        return Ok(());
    }
    let destination_path = destination_root.join(relative);
    let remote_source = format!(
        "adrive://{}/{}/{}",
        source.instance, source.space, file.file_path
    );
    let local_destination = destination_path.display().to_string();
    let item_key = checkpoint_item_key("download", &remote_source, Some(&local_destination));
    if should_skip_local_destination(
        &destination_path,
        Some(file.updated_at),
        context.runtime.overwrite_strategy,
    )? {
        context
            .report
            .push_skipped(remote_source, Some(local_destination), "download");
        tick_progress(context.progress);
        return Ok(());
    }
    if context.checkpoint.completed.contains(&item_key) {
        context
            .report
            .push_skipped(remote_source, Some(local_destination), "download");
        tick_progress(context.progress);
        return Ok(());
    }
    if let Some(parent) = destination_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let instance = source.instance.clone();
    let space = source.space.clone();
    let file_path = file.file_path.clone();
    let etag = file.etag.clone();
    let crc64 = file.hash_crc64_ecma;
    let file_size = adrive_file_size(&file);
    let checkpoint_enabled = context.args.checkpoint;
    let bandwidth_limit = context.args.bandwidth_limit.clone();
    let runtime = context.runtime;
    let client = context.client;
    context
        .queue(Box::pin(async move {
            let result = download_adrive_file_for_batch(
                client,
                &instance,
                &space,
                &file_path,
                &etag,
                crc64,
                file_size,
                &destination_path,
                checkpoint_enabled,
                bandwidth_limit.as_deref(),
                runtime,
            )
            .await;
            (
                remote_source,
                Some(local_destination),
                item_key,
                1,
                "download",
                result,
            )
        }))
        .await
}

async fn stream_adrive_remote_copy_no_manifest(
    context: &mut ADriveStreamCpContext<'_>,
) -> Result<(), CliError> {
    let source = parse_adrive_uri(&context.args.source, false)?;
    let destination = parse_adrive_uri(&context.args.destination, false)?;
    if source.instance != destination.instance {
        return Err(CliError::ValidationError(
            "ve-adrive recursive remote copy requires source and destination in the same instance"
                .to_string(),
        ));
    }
    let source_prefix = trim_folder_prefix(&source.path);
    let parent_prefix =
        recursive_adrive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let mut pending_prefixes = vec![source_prefix.clone()];
    let mut seen_prefixes = HashSet::new();
    while let Some(prefix) = pending_prefixes.pop() {
        if !seen_prefixes.insert(prefix.clone()) {
            continue;
        }
        let mut marker = None;
        loop {
            let mut input = ListFilesInput::new(&source.instance, &source.space)
                .with_limit(1000)
                .with_delimiter("/");
            if !prefix.is_empty() {
                input = input.with_prefix(&prefix);
            }
            if let Some(value) = marker.take() {
                input = input.with_marker(value);
            }
            let out = context
                .client
                .list_files(&input)
                .await
                .map_err(map_ids_error)?;
            for file in out.files {
                queue_adrive_remote_copy_file(
                    context,
                    &source,
                    &destination,
                    &source_prefix,
                    parent_prefix.as_deref(),
                    file,
                )
                .await?;
            }
            for folder in out.folders {
                pending_prefixes.push(trim_folder_prefix(&folder.folder));
            }
            if !out.is_truncated || out.next_marker.is_empty() {
                break;
            }
            marker = Some(out.next_marker);
        }
    }
    Ok(())
}

async fn queue_adrive_remote_copy_file(
    context: &mut ADriveStreamCpContext<'_>,
    source: &ParsedADriveUri,
    destination: &ParsedADriveUri,
    source_prefix: &str,
    parent_prefix: Option<&str>,
    file: FileInfo,
) -> Result<(), CliError> {
    let source_relative = remote_relative_path(&file.file_path, source_prefix);
    let relative = prepend_adrive_parent_prefix(&source_relative, parent_prefix);
    if !path_matches_filters(
        &relative,
        context.args.include.as_deref(),
        context.args.exclude.as_deref(),
    ) {
        return Ok(());
    }
    let destination_path = join_remote_path(&destination.path, &relative);
    let remote_source = format!(
        "adrive://{}/{}/{}",
        source.instance, source.space, file.file_path
    );
    let remote_destination = format!(
        "adrive://{}/{}/{}",
        destination.instance, destination.space, destination_path
    );
    let item_key = checkpoint_item_key("remote-copy", &remote_source, Some(&remote_destination));
    if should_skip_remote_destination(
        context.client,
        &destination.instance,
        &destination.space,
        &destination_path,
        Some(file.updated_at),
        context.runtime.overwrite_strategy,
    )
    .await?
    {
        context
            .report
            .push_skipped(remote_source, Some(remote_destination), "remote-copy");
        tick_progress(context.progress);
        return Ok(());
    }
    if context.checkpoint.completed.contains(&item_key) {
        context
            .report
            .push_skipped(remote_source, Some(remote_destination), "remote-copy");
        tick_progress(context.progress);
        return Ok(());
    }
    let instance = source.instance.clone();
    let source_space = source.space.clone();
    let source_path = file.file_path.clone();
    let destination_space = destination.space.clone();
    let source_etag = file.etag.clone();
    let client = context.client;
    context
        .queue(Box::pin(async move {
            let result = copy_adrive_file_simple(
                client,
                &instance,
                &source_space,
                &source_path,
                &destination_space,
                &destination_path,
                &source_etag,
            )
            .await;
            (
                remote_source,
                Some(remote_destination),
                item_key,
                1,
                "remote-copy",
                result,
            )
        }))
        .await
}

fn sorted_adrive_read_dir_entries(path: &Path) -> Result<Vec<fs::DirEntry>, CliError> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(path)? {
        entries.push(entry?);
    }
    entries.sort_by_key(|entry| entry.path());
    Ok(entries)
}

async fn upload_directory_recursive(
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
    list_echo_enabled: bool,
    progress_enabled: bool,
) -> Result<BatchReport, CliError> {
    let destination = parse_adrive_uri(&args.destination, false)?;
    let source_root = PathBuf::from(&args.source);
    if !source_root.is_dir() {
        return Err(CliError::ValidationError(format!(
            "recursive upload source must be a directory: {}",
            args.source
        )));
    }
    let parent_prefix = recursive_adrive_source_parent_prefix(&args.source, args.include_parent)?;
    // [Review Fix #OutputControls1] Local recursive uploads still build a
    // manifest from a list phase, so they must honor --list-echo/--no-list-echo.
    let mut scan_progress =
        PlanScanProgress::new(list_echo_enabled, "ve-adrive cp plan", &args.source);
    let mut files = Vec::new();
    for file in collect_local_files(&source_root)? {
        let relative = file.strip_prefix(&source_root).map_err(|err| {
            CliError::ValidationError(format!("failed to derive relative path: {}", err))
        })?;
        let source_relative = normalize_local_relative_path(relative)?;
        let relative = prepend_adrive_parent_prefix(&source_relative, parent_prefix.as_deref());
        if !path_matches_filters(&relative, args.include.as_deref(), args.exclude.as_deref()) {
            continue;
        }
        let size = std::fs::metadata(&file)?.len();
        files.push((file, relative, size));
    }
    scan_progress.finish_with_count(files.len() as u64, "file(s)");
    let progress = batch_progress(
        "ve-adrive cp upload",
        batch_progress_total(files.iter().map(|(_, _, size)| *size), runtime),
        progress_enabled,
    );
    let checkpoint_path = checkpoint_path_for_cp(args)?;
    let mut checkpoint = load_batch_checkpoint(checkpoint_path.as_deref()).await?;
    let mut report = BatchReport::new("ve-adrive cp");
    report.set_manifest(
        files
            .iter()
            .map(|(file, relative, size)| {
                let remote_path = join_remote_path(&destination.path, relative);
                BatchManifestItem {
                    operation: "upload".to_string(),
                    source: file.display().to_string(),
                    destination: Some(format_target(&ParsedADriveUri {
                        instance: destination.instance.clone(),
                        space: destination.space.clone(),
                        path: remote_path,
                    })),
                    size: *size,
                    etag: None,
                    crc64: None,
                }
            })
            .collect(),
    );
    let mut in_flight = FuturesUnordered::new();
    for (file, relative, file_size) in files {
        while in_flight.len() >= runtime.batch_concurrency {
            if let Some((source, destination, item_key, progress_units, result)) =
                in_flight.next().await
            {
                record_adrive_batch_result(
                    &mut report,
                    checkpoint_path.as_deref(),
                    &mut checkpoint,
                    &progress,
                    "upload",
                    source,
                    destination,
                    item_key,
                    progress_units,
                    result,
                )
                .await?;
            }
        }
        let progress_units = progress_units_for_size(file_size, runtime);
        let remote_path = join_remote_path(&destination.path, &relative);
        let remote_target = format_target(&ParsedADriveUri {
            instance: destination.instance.clone(),
            space: destination.space.clone(),
            path: remote_path.clone(),
        });
        let source_updated_at = local_modified_millis(&file)?;
        if should_skip_remote_destination(
            client,
            &destination.instance,
            &destination.space,
            &remote_path,
            Some(source_updated_at),
            runtime.overwrite_strategy,
        )
        .await?
        {
            report.push_skipped(
                file.display().to_string(),
                Some(remote_target.clone()),
                "upload",
            );
            tick_progress_by(&progress, progress_units);
            continue;
        }
        let item_key =
            checkpoint_item_key("upload", &file.display().to_string(), Some(&remote_target));
        if checkpoint.completed.contains(&item_key) {
            report.push_skipped(file.display().to_string(), Some(remote_target), "upload");
            tick_progress_by(&progress, progress_units);
            continue;
        }
        let instance = destination.instance.clone();
        let space = destination.space.clone();
        let source = file.display().to_string();
        let destination_label = Some(remote_target);
        let checkpoint_enabled = args.checkpoint;
        let checkpoint_dir = args.checkpoint_dir.clone();
        let bandwidth_limit = args.bandwidth_limit.clone();
        in_flight.push(async move {
            let result = upload_adrive_file_for_batch(
                client,
                &instance,
                &space,
                &remote_path,
                &file,
                checkpoint_enabled,
                checkpoint_dir.as_deref(),
                bandwidth_limit.as_deref(),
                runtime,
            )
            .await;
            (source, destination_label, item_key, progress_units, result)
        });
    }
    while let Some((source, destination, item_key, progress_units, result)) = in_flight.next().await
    {
        record_adrive_batch_result(
            &mut report,
            checkpoint_path.as_deref(),
            &mut checkpoint,
            &progress,
            "upload",
            source,
            destination,
            item_key,
            progress_units,
            result,
        )
        .await?;
    }
    finish_progress(progress);
    Ok(report)
}

async fn download_directory_recursive(
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
    list_echo_enabled: bool,
    progress_enabled: bool,
) -> Result<BatchReport, CliError> {
    let source = parse_adrive_uri(&args.source, false)?;
    let mut scan_progress =
        PlanScanProgress::new(list_echo_enabled, "ve-adrive cp plan", &args.source);
    let source_prefix = trim_folder_prefix(&source.path);
    let parent_prefix = recursive_adrive_source_parent_prefix(&args.source, args.include_parent)?;
    let files = list_all_files(
        client,
        &source.instance,
        &source.space,
        &source.path,
        true,
        runtime.list_concurrency,
    )
    .await?;
    let files = files
        .into_iter()
        .filter_map(|file| {
            let relative = recursive_adrive_relative_path(
                &file.file_path,
                &source_prefix,
                parent_prefix.as_deref(),
            );
            path_matches_filters(&relative, args.include.as_deref(), args.exclude.as_deref())
                .then_some((file, relative))
        })
        .collect::<Vec<_>>();
    scan_progress.finish_with_count(files.len() as u64, "file(s)");
    let progress = batch_progress(
        "ve-adrive cp download",
        batch_progress_total(
            files.iter().map(|(file, _)| adrive_file_size(file)),
            runtime,
        ),
        progress_enabled,
    );
    let checkpoint_path = checkpoint_path_for_cp(args)?;
    let mut checkpoint = load_batch_checkpoint(checkpoint_path.as_deref()).await?;
    let destination_root = PathBuf::from(&args.destination);
    let mut report = BatchReport::new("ve-adrive cp");
    report.set_manifest(
        files
            .iter()
            .map(|(file, relative)| {
                let destination_path = destination_root.join(relative);
                let remote_source = format!(
                    "adrive://{}/{}/{}",
                    source.instance, source.space, file.file_path
                );
                adrive_remote_file_manifest_item(
                    "download",
                    file,
                    remote_source,
                    Some(destination_path.display().to_string()),
                )
            })
            .collect(),
    );
    let mut in_flight = FuturesUnordered::new();
    for (file, relative) in files {
        while in_flight.len() >= runtime.batch_concurrency {
            if let Some((source, destination, item_key, progress_units, result)) =
                in_flight.next().await
            {
                record_adrive_batch_result(
                    &mut report,
                    checkpoint_path.as_deref(),
                    &mut checkpoint,
                    &progress,
                    "download",
                    source,
                    destination,
                    item_key,
                    progress_units,
                    result,
                )
                .await?;
            }
        }
        let progress_units = progress_units_for_size(adrive_file_size(&file), runtime);
        let destination_path = destination_root.join(relative);
        let remote_source = format!(
            "adrive://{}/{}/{}",
            source.instance, source.space, file.file_path
        );
        let local_destination = destination_path.display().to_string();
        let item_key = checkpoint_item_key("download", &remote_source, Some(&local_destination));
        if should_skip_local_destination(
            &destination_path,
            Some(file.updated_at),
            runtime.overwrite_strategy,
        )? {
            report.push_skipped(remote_source, Some(local_destination), "download");
            tick_progress_by(&progress, progress_units);
            continue;
        }
        if checkpoint.completed.contains(&item_key) {
            report.push_skipped(remote_source, Some(local_destination), "download");
            tick_progress_by(&progress, progress_units);
            continue;
        }
        if let Some(parent) = destination_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let instance = source.instance.clone();
        let space = source.space.clone();
        let file_path = file.file_path.clone();
        let etag = file.etag.clone();
        let crc64 = file.hash_crc64_ecma;
        let file_size = adrive_file_size(&file);
        let checkpoint_enabled = args.checkpoint;
        let bandwidth_limit = args.bandwidth_limit.clone();
        in_flight.push(async move {
            let result = download_adrive_file_for_batch(
                client,
                &instance,
                &space,
                &file_path,
                &etag,
                crc64,
                file_size,
                &destination_path,
                checkpoint_enabled,
                bandwidth_limit.as_deref(),
                runtime,
            )
            .await;
            (
                remote_source,
                Some(local_destination),
                item_key,
                progress_units,
                result,
            )
        });
    }
    while let Some((source, destination, item_key, progress_units, result)) = in_flight.next().await
    {
        record_adrive_batch_result(
            &mut report,
            checkpoint_path.as_deref(),
            &mut checkpoint,
            &progress,
            "download",
            source,
            destination,
            item_key,
            progress_units,
            result,
        )
        .await?;
    }
    finish_progress(progress);
    Ok(report)
}

async fn copy_remote_recursive(
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
    list_echo_enabled: bool,
    progress_enabled: bool,
) -> Result<BatchReport, CliError> {
    let source = parse_adrive_uri(&args.source, false)?;
    let destination = parse_adrive_uri(&args.destination, false)?;
    if source.instance != destination.instance {
        return Err(CliError::ValidationError(
            "ve-adrive recursive remote copy requires source and destination in the same instance"
                .to_string(),
        ));
    }
    let mut scan_progress =
        PlanScanProgress::new(list_echo_enabled, "ve-adrive cp plan", &args.source);
    let source_prefix = trim_folder_prefix(&source.path);
    let parent_prefix = recursive_adrive_source_parent_prefix(&args.source, args.include_parent)?;
    let files = list_all_files(
        client,
        &source.instance,
        &source.space,
        &source.path,
        true,
        runtime.list_concurrency,
    )
    .await?;
    let files = files
        .into_iter()
        .filter_map(|file| {
            let relative = recursive_adrive_relative_path(
                &file.file_path,
                &source_prefix,
                parent_prefix.as_deref(),
            );
            path_matches_filters(&relative, args.include.as_deref(), args.exclude.as_deref())
                .then_some((file, relative))
        })
        .collect::<Vec<_>>();
    scan_progress.finish_with_count(files.len() as u64, "file(s)");
    let progress = batch_progress(
        "ve-adrive cp remote-copy",
        batch_progress_total(
            files.iter().map(|(file, _)| adrive_file_size(file)),
            runtime,
        ),
        progress_enabled,
    );
    let checkpoint_path = checkpoint_path_for_cp(args)?;
    let mut checkpoint = load_batch_checkpoint(checkpoint_path.as_deref()).await?;
    let mut report = BatchReport::new("ve-adrive cp");
    report.set_manifest(
        files
            .iter()
            .map(|(file, relative)| {
                let destination_path = join_remote_path(&destination.path, relative);
                let remote_source = format!(
                    "adrive://{}/{}/{}",
                    source.instance, source.space, file.file_path
                );
                let remote_destination = format!(
                    "adrive://{}/{}/{}",
                    destination.instance, destination.space, destination_path
                );
                adrive_remote_file_manifest_item(
                    "remote-copy",
                    file,
                    remote_source,
                    Some(remote_destination),
                )
            })
            .collect(),
    );
    let mut in_flight = FuturesUnordered::new();
    for (file, relative) in files {
        while in_flight.len() >= runtime.batch_concurrency {
            if let Some((source, destination, item_key, progress_units, result)) =
                in_flight.next().await
            {
                record_adrive_batch_result(
                    &mut report,
                    checkpoint_path.as_deref(),
                    &mut checkpoint,
                    &progress,
                    "remote-copy",
                    source,
                    destination,
                    item_key,
                    progress_units,
                    result,
                )
                .await?;
            }
        }
        let progress_units = progress_units_for_size(adrive_file_size(&file), runtime);
        let destination_path = join_remote_path(&destination.path, &relative);
        let remote_source = format!(
            "adrive://{}/{}/{}",
            source.instance, source.space, file.file_path
        );
        let remote_destination = format!(
            "adrive://{}/{}/{}",
            destination.instance, destination.space, destination_path
        );
        let item_key =
            checkpoint_item_key("remote-copy", &remote_source, Some(&remote_destination));
        if should_skip_remote_destination(
            client,
            &destination.instance,
            &destination.space,
            &destination_path,
            Some(file.updated_at),
            runtime.overwrite_strategy,
        )
        .await?
        {
            report.push_skipped(remote_source, Some(remote_destination), "remote-copy");
            tick_progress_by(&progress, progress_units);
            continue;
        }
        if checkpoint.completed.contains(&item_key) {
            report.push_skipped(remote_source, Some(remote_destination), "remote-copy");
            tick_progress_by(&progress, progress_units);
            continue;
        }
        let instance = source.instance.clone();
        let source_space = source.space.clone();
        let source_path = file.file_path.clone();
        let destination_space = destination.space.clone();
        let source_etag = file.etag.clone();
        in_flight.push(async move {
            let result = copy_adrive_file_simple(
                client,
                &instance,
                &source_space,
                &source_path,
                &destination_space,
                &destination_path,
                &source_etag,
            )
            .await;
            (
                remote_source,
                Some(remote_destination),
                item_key,
                progress_units,
                result,
            )
        });
    }
    while let Some((source, destination, item_key, progress_units, result)) = in_flight.next().await
    {
        record_adrive_batch_result(
            &mut report,
            checkpoint_path.as_deref(),
            &mut checkpoint,
            &progress,
            "remote-copy",
            source,
            destination,
            item_key,
            progress_units,
            result,
        )
        .await?;
    }
    finish_progress(progress);
    Ok(report)
}

async fn upload_file(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
) -> Result<i32, CliError> {
    let dest = parse_adrive_uri(&args.destination, false)?;
    let source_path = PathBuf::from(&args.source);
    if !source_path.is_file() {
        return Err(CliError::ValidationError(format!(
            "upload source must be a file: {}",
            args.source
        )));
    }
    let file_path = remote_file_path_for_upload(&dest, source_path.as_path())?;
    let size = tokio::fs::metadata(&source_path).await?.len();
    let source_updated_at = local_modified_millis(&source_path)?;
    if should_skip_remote_destination(
        client,
        &dest.instance,
        &dest.space,
        &file_path,
        Some(source_updated_at),
        runtime.overwrite_strategy,
    )
    .await?
    {
        write_adrive_single_report(
            args.report_path.as_deref(),
            "ve-adrive cp",
            args.source.clone(),
            Some(format_target(&ParsedADriveUri {
                path: file_path,
                ..dest
            })),
            "upload",
            "skipped",
            None,
            args.report_failures_only,
        )
        .await?;
        return Ok(0);
    }
    if should_use_multipart(size, args.checkpoint, runtime.checkpoint_threshold) {
        return upload_file_multipart(
            global,
            client,
            args,
            runtime,
            dest,
            source_path,
            file_path,
            size,
        )
        .await;
    }
    let mut input = PutFileInput::new(
        dest.instance.clone(),
        dest.space.clone(),
        file_path.clone(),
        IdsBody::from_file(source_path.clone()),
    )
    .with_content_length(size);
    if let Some(rate_limiter) = rate_limiter_from_limit(args.bandwidth_limit.as_deref())? {
        input = input.with_rate_limiter(rate_limiter);
    }
    let out = client.put_file(input).await.map_err(map_ids_error)?;
    let local_crc64 = local_file_crc64(&source_path).await?;
    if out.hash_crc64_ecma != 0 && out.hash_crc64_ecma != local_crc64 {
        return Err(CliError::TransferFailed(format!(
            "CRC64 mismatch for '{}': local={}, remote={}",
            args.source, local_crc64, out.hash_crc64_ecma
        )));
    }
    let destination = format_target(&ParsedADriveUri {
        path: file_path.clone(),
        ..dest.clone()
    });
    write_adrive_single_report(
        args.report_path.as_deref(),
        "ve-adrive cp",
        args.source.clone(),
        Some(destination.clone()),
        "upload",
        "succeeded",
        None,
        args.report_failures_only,
    )
    .await?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive cp",
            json!({
                "operation": "upload",
                "source": args.source,
                "destination": destination,
                "size": out.size,
                "etag": out.etag,
                "request_id": out.response_info.request_id(),
                "status": "succeeded",
            }),
        ),
    )?;
    Ok(0)
}

async fn upload_file_multipart(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
    dest: ParsedADriveUri,
    source_path: PathBuf,
    file_path: String,
    file_size: u64,
) -> Result<i32, CliError> {
    // [Review Fix #ADrive-ResumeUpload] Local uploads larger than single PUT,
    // or explicitly marked --checkpoint, now persist multipart state.
    let checkpoint_path = checkpoint_path_for_cp(args)?;
    let mut checkpoint = load_upload_checkpoint(checkpoint_path.as_deref())
        .await?
        .unwrap_or(UploadCheckpoint {
            instance: dest.instance.clone(),
            space: dest.space.clone(),
            file_path: file_path.clone(),
            source_path: source_path.display().to_string(),
            file_size,
            part_size: ADRIVE_MULTIPART_PART_SIZE,
            upload_id: None,
            completed_parts: Vec::new(),
        });
    validate_upload_checkpoint(&checkpoint, &dest, &source_path, &file_path, file_size)?;

    if checkpoint.upload_id.is_none() {
        let initiated = client
            .initiate_multipart_upload(&InitiateMultipartUploadInput {
                instance_id: dest.instance.clone(),
                space_id: dest.space.clone(),
                file_path: file_path.clone(),
                content_type: None,
                meta: None,
            })
            .await
            .map_err(map_ids_error)?;
        checkpoint.upload_id = Some(initiated.upload_id);
        save_upload_checkpoint(checkpoint_path.as_deref(), &checkpoint).await?;
    }

    let upload_id = checkpoint
        .upload_id
        .clone()
        .ok_or_else(|| CliError::ValidationError("multipart upload id is missing".to_string()))?;
    let rate_limiter = rate_limiter_from_limit(args.bandwidth_limit.as_deref())?;
    let part_count = file_size.div_ceil(checkpoint.part_size);
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let progress = batch_progress(
        "ve-adrive cp upload",
        progress_units_for_size(file_size, runtime),
        progress_enabled,
    );
    if let Some(progress) = &progress {
        let completed_units = checkpoint
            .completed_parts
            .iter()
            .map(|part| {
                multipart_progress_units_for_part(
                    part.part_number as u64,
                    checkpoint.part_size,
                    file_size,
                    runtime,
                )
            })
            .sum();
        progress.set_position(completed_units);
    }
    let mut completed = checkpoint
        .completed_parts
        .iter()
        .map(|part| (part.part_number, part.etag.clone()))
        .collect::<HashMap<_, _>>();

    let mut pending_parts = Vec::new();
    for part_index in 0..part_count {
        let part_number = (part_index + 1) as i32;
        if completed.contains_key(&part_number) {
            continue;
        }
        let offset = part_index * checkpoint.part_size;
        let length = std::cmp::min(checkpoint.part_size, file_size - offset);
        pending_parts.push((part_number, offset, length));
    }
    let mut pending_parts = pending_parts.into_iter();
    let mut in_flight = FuturesUnordered::new();
    loop {
        while in_flight.len() < runtime.multipart_concurrency {
            let Some((part_number, offset, length)) = pending_parts.next() else {
                break;
            };
            in_flight.push(upload_adrive_part(
                client,
                &dest,
                &file_path,
                &source_path,
                &upload_id,
                part_number,
                offset,
                length,
                rate_limiter.clone(),
            ));
        }
        let Some(result) = in_flight.next().await else {
            break;
        };
        let (part_number, etag, length) = match result {
            Ok(uploaded) => uploaded,
            Err(err) => {
                finish_progress(progress);
                if !args.checkpoint {
                    // [Review Fix #6] Non-checkpoint multipart uploads cannot
                    // resume safely, so abort failed upload sessions.
                    let _ = abort_adrive_multipart_upload(
                        client,
                        &dest.instance,
                        &dest.space,
                        &file_path,
                        &upload_id,
                    )
                    .await;
                }
                return Err(err);
            }
        };
        completed.insert(part_number, etag.clone());
        checkpoint
            .completed_parts
            .push(UploadPartCheckpoint { part_number, etag });
        save_upload_checkpoint(checkpoint_path.as_deref(), &checkpoint).await?;
        tick_progress_by(&progress, progress_units_for_size(length, runtime));
    }
    finish_progress(progress);

    checkpoint
        .completed_parts
        .sort_by_key(|part| part.part_number);
    let complete = CompleteMultipartUploadInput {
        instance_id: dest.instance.clone(),
        space_id: dest.space.clone(),
        file_path: file_path.clone(),
        upload_id: upload_id.clone(),
        parts: checkpoint
            .completed_parts
            .iter()
            .map(|part| PartInfo {
                part_number: part.part_number,
                etag: part.etag.clone(),
            })
            .collect(),
    };
    let out = match client.complete_multipart_upload(&complete).await {
        Ok(out) => out,
        Err(err) => {
            if !args.checkpoint {
                // [Review Fix #7] Abort non-checkpoint multipart uploads when
                // CompleteMultipartUpload fails after parts were uploaded.
                let _ = abort_adrive_multipart_upload(
                    client,
                    &dest.instance,
                    &dest.space,
                    &file_path,
                    &upload_id,
                )
                .await;
            }
            return Err(map_ids_error(err));
        }
    };
    let local_crc64 = local_file_crc64(&source_path).await?;
    if out.hash_crc64_ecma != 0 && out.hash_crc64_ecma != local_crc64 {
        return Err(CliError::TransferFailed(format!(
            "multipart complete CRC64 mismatch: local={}, remote={}",
            local_crc64, out.hash_crc64_ecma
        )));
    }
    remove_checkpoint_file(checkpoint_path.as_deref()).await?;
    write_adrive_single_report(
        args.report_path.as_deref(),
        "ve-adrive cp",
        args.source.clone(),
        Some(format_target(&ParsedADriveUri {
            path: file_path.clone(),
            ..dest.clone()
        })),
        "multipart-upload",
        "succeeded",
        None,
        args.report_failures_only,
    )
    .await?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive cp",
            json!({
                "operation": "multipart-upload",
                "source": args.source,
                "destination": format_target(&ParsedADriveUri {
                    path: file_path.clone(),
                    ..dest.clone()
                }),
                "size": out.size,
                "etag": out.etag,
                "parts": complete.parts.len(),
                "checkpoint": {
                    "enabled": args.checkpoint,
                    "scope": "multipart_upload",
                    "path": checkpoint_path.as_ref().map(|path| path.display().to_string()),
                },
                "request_id": out.response_info.request_id(),
                "status": "succeeded",
            }),
        ),
    )?;
    Ok(0)
}

async fn upload_adrive_part(
    client: &IdsClient,
    dest: &ParsedADriveUri,
    file_path: &str,
    source_path: &Path,
    upload_id: &str,
    part_number: i32,
    offset: u64,
    length: u64,
    rate_limiter: Option<Arc<RateLimiter>>,
) -> Result<(i32, String, u64), CliError> {
    let body = read_file_part(source_path, offset, length).await?;
    let mut digest = Digest::new();
    let _ = digest.write(&body);
    let local_crc64 = digest.sum64();
    let mut input = UploadPartInput::new(
        dest.instance.clone(),
        dest.space.clone(),
        file_path.to_string(),
        upload_id.to_string(),
        part_number,
        IdsBody::from_bytes(body),
    )
    .with_content_length(length);
    if let Some(rate_limiter) = rate_limiter {
        input = input.with_rate_limiter(rate_limiter);
    }
    let out = client.upload_part(input).await.map_err(map_ids_error)?;
    if out.hash_crc64_ecma != 0 && out.hash_crc64_ecma != local_crc64 {
        return Err(CliError::TransferFailed(format!(
            "CRC64 mismatch for part {}: local={}, remote={}",
            part_number, local_crc64, out.hash_crc64_ecma
        )));
    }
    Ok((part_number, out.etag, length))
}

async fn upload_adrive_file_simple(
    client: &IdsClient,
    instance: &str,
    space: &str,
    remote_path: &str,
    local_path: &Path,
) -> Result<(), CliError> {
    let size = tokio::fs::metadata(local_path).await?.len();
    let input = PutFileInput::new(
        instance.to_string(),
        space.to_string(),
        remote_path.to_string(),
        IdsBody::from_file(local_path.to_path_buf()),
    )
    .with_content_length(size);
    let out = client.put_file(input).await.map_err(map_ids_error)?;
    let local_crc64 = local_file_crc64(local_path).await?;
    if out.hash_crc64_ecma != 0 && out.hash_crc64_ecma != local_crc64 {
        return Err(CliError::TransferFailed(format!(
            "CRC64 mismatch for '{}': local={}, remote={}",
            local_path.display(),
            local_crc64,
            out.hash_crc64_ecma
        )));
    }
    Ok(())
}

async fn upload_adrive_file_for_batch(
    client: &IdsClient,
    instance: &str,
    space: &str,
    remote_path: &str,
    local_path: &Path,
    checkpoint_enabled: bool,
    checkpoint_dir: Option<&str>,
    bandwidth_limit: Option<&str>,
    runtime: TransferRuntimeConfig,
) -> Result<(), CliError> {
    let file_size = tokio::fs::metadata(local_path).await?.len();
    if should_use_multipart(file_size, checkpoint_enabled, runtime.checkpoint_threshold) {
        return upload_adrive_file_multipart_for_batch(
            client,
            instance,
            space,
            remote_path,
            local_path,
            file_size,
            checkpoint_enabled,
            checkpoint_dir,
            bandwidth_limit,
            runtime,
        )
        .await;
    }
    upload_adrive_file_simple(client, instance, space, remote_path, local_path).await
}

async fn upload_adrive_file_multipart_for_batch(
    client: &IdsClient,
    instance: &str,
    space: &str,
    remote_path: &str,
    local_path: &Path,
    file_size: u64,
    checkpoint_enabled: bool,
    checkpoint_dir: Option<&str>,
    bandwidth_limit: Option<&str>,
    runtime: TransferRuntimeConfig,
) -> Result<(), CliError> {
    let destination = ParsedADriveUri {
        instance: instance.to_string(),
        space: space.to_string(),
        path: remote_path.to_string(),
    };
    let checkpoint_path = checkpoint_path_for_adrive_batch_file(
        checkpoint_enabled,
        checkpoint_dir,
        local_path,
        &destination,
    )?;
    let mut checkpoint = load_upload_checkpoint(checkpoint_path.as_deref())
        .await?
        .unwrap_or(UploadCheckpoint {
            instance: instance.to_string(),
            space: space.to_string(),
            file_path: remote_path.to_string(),
            source_path: local_path.display().to_string(),
            file_size,
            part_size: ADRIVE_MULTIPART_PART_SIZE,
            upload_id: None,
            completed_parts: Vec::new(),
        });
    validate_upload_checkpoint(
        &checkpoint,
        &destination,
        local_path,
        remote_path,
        file_size,
    )?;

    if checkpoint.upload_id.is_none() {
        let initiated = client
            .initiate_multipart_upload(&InitiateMultipartUploadInput {
                instance_id: instance.to_string(),
                space_id: space.to_string(),
                file_path: remote_path.to_string(),
                content_type: None,
                meta: None,
            })
            .await
            .map_err(map_ids_error)?;
        checkpoint.upload_id = Some(initiated.upload_id);
        save_upload_checkpoint(checkpoint_path.as_deref(), &checkpoint).await?;
    }

    let upload_id = checkpoint
        .upload_id
        .clone()
        .ok_or_else(|| CliError::ValidationError("multipart upload id is missing".to_string()))?;
    let rate_limiter = rate_limiter_from_limit(bandwidth_limit)?;
    let part_count = file_size.div_ceil(checkpoint.part_size);
    let mut completed = checkpoint
        .completed_parts
        .iter()
        .map(|part| (part.part_number, part.etag.clone()))
        .collect::<HashMap<_, _>>();

    let mut pending_parts = Vec::new();
    for part_index in 0..part_count {
        let part_number = (part_index + 1) as i32;
        if completed.contains_key(&part_number) {
            continue;
        }
        let offset = part_index * checkpoint.part_size;
        let length = std::cmp::min(checkpoint.part_size, file_size - offset);
        pending_parts.push((part_number, offset, length));
    }
    let mut pending_parts = pending_parts.into_iter();
    let mut in_flight = FuturesUnordered::new();
    loop {
        while in_flight.len() < runtime.multipart_concurrency {
            let Some((part_number, offset, length)) = pending_parts.next() else {
                break;
            };
            in_flight.push(upload_adrive_part(
                client,
                &destination,
                remote_path,
                local_path,
                &upload_id,
                part_number,
                offset,
                length,
                rate_limiter.clone(),
            ));
        }
        let Some(result) = in_flight.next().await else {
            break;
        };
        let (part_number, etag, _length) = match result {
            Ok(uploaded) => uploaded,
            Err(err) => {
                if !checkpoint_enabled {
                    // [Review Fix #8] Batch multipart uploads without
                    // checkpoint state should not leave orphaned parts.
                    let _ = abort_adrive_multipart_upload(
                        client,
                        instance,
                        space,
                        remote_path,
                        &upload_id,
                    )
                    .await;
                }
                return Err(err);
            }
        };
        completed.insert(part_number, etag.clone());
        checkpoint
            .completed_parts
            .push(UploadPartCheckpoint { part_number, etag });
        save_upload_checkpoint(checkpoint_path.as_deref(), &checkpoint).await?;
    }

    checkpoint
        .completed_parts
        .sort_by_key(|part| part.part_number);
    let complete = CompleteMultipartUploadInput {
        instance_id: instance.to_string(),
        space_id: space.to_string(),
        file_path: remote_path.to_string(),
        upload_id: upload_id.clone(),
        parts: checkpoint
            .completed_parts
            .iter()
            .map(|part| PartInfo {
                part_number: part.part_number,
                etag: part.etag.clone(),
            })
            .collect(),
    };
    let out = match client.complete_multipart_upload(&complete).await {
        Ok(out) => out,
        Err(err) => {
            if !checkpoint_enabled {
                // [Review Fix #9] Complete failure after non-checkpoint batch
                // upload should abort the service-side multipart session.
                let _ =
                    abort_adrive_multipart_upload(client, instance, space, remote_path, &upload_id)
                        .await;
            }
            return Err(map_ids_error(err));
        }
    };
    let local_crc64 = local_file_crc64(local_path).await?;
    if out.hash_crc64_ecma != 0 && out.hash_crc64_ecma != local_crc64 {
        return Err(CliError::TransferFailed(format!(
            "multipart complete CRC64 mismatch for '{}': local={}, remote={}",
            local_path.display(),
            local_crc64,
            out.hash_crc64_ecma
        )));
    }
    remove_checkpoint_file(checkpoint_path.as_deref()).await?;
    Ok(())
}

async fn download_adrive_file_simple(
    client: &IdsClient,
    instance: &str,
    space: &str,
    remote_path: &str,
    etag: &str,
    expected_crc64: u64,
    destination_path: &Path,
) -> Result<(), CliError> {
    let mut input = GetFileInput::new(instance, space, remote_path);
    input.if_match = Some(etag.to_string());
    let out = client.get_file(&input).await.map_err(map_ids_error)?;
    let bytes = out.read_all().await.map_err(map_ids_error)?;
    if expected_crc64 != 0 {
        let mut digest = Digest::new();
        let _ = digest.write(&bytes);
        let local_crc64 = digest.sum64();
        if local_crc64 != expected_crc64 {
            return Err(CliError::TransferFailed(format!(
                "CRC64 mismatch for '{}': local={}, remote={}",
                destination_path.display(),
                local_crc64,
                expected_crc64
            )));
        }
    }
    tokio::fs::write(destination_path, &bytes).await?;
    Ok(())
}

async fn download_adrive_file_for_batch(
    client: &IdsClient,
    instance: &str,
    space: &str,
    remote_path: &str,
    etag: &str,
    expected_crc64: u64,
    file_size: u64,
    destination_path: &Path,
    checkpoint_enabled: bool,
    bandwidth_limit: Option<&str>,
    runtime: TransferRuntimeConfig,
) -> Result<(), CliError> {
    if checkpoint_enabled && file_size >= runtime.checkpoint_threshold {
        let source = ParsedADriveUri {
            instance: instance.to_string(),
            space: space.to_string(),
            path: remote_path.to_string(),
        };
        let rate_limiter = rate_limiter_from_limit(bandwidth_limit)?;
        download_adrive_file_ranges(
            client,
            &source,
            etag,
            file_size,
            destination_path,
            runtime,
            rate_limiter,
        )
        .await?;
        if expected_crc64 != 0 {
            let local_crc64 = local_file_crc64(destination_path).await?;
            if local_crc64 != expected_crc64 {
                let _ = tokio::fs::remove_file(destination_path).await;
                return Err(CliError::TransferFailed(format!(
                    "CRC64 mismatch for '{}': local={}, remote={}",
                    destination_path.display(),
                    local_crc64,
                    expected_crc64
                )));
            }
        }
        return Ok(());
    }
    download_adrive_file_simple(
        client,
        instance,
        space,
        remote_path,
        etag,
        expected_crc64,
        destination_path,
    )
    .await
}

async fn copy_adrive_file_simple(
    client: &IdsClient,
    instance: &str,
    source_space: &str,
    source_path: &str,
    destination_space: &str,
    destination_path: &str,
    source_etag: &str,
) -> Result<(), CliError> {
    let input = CopyFileInput {
        instance_id: instance.to_string(),
        space_id: source_space.to_string(),
        file_path: source_path.to_string(),
        copy_to_space_id: destination_space.to_string(),
        copy_to_path: destination_path.to_string(),
        copy_source_if_match: Some(source_etag.to_string()),
        auto_rename: false,
    };
    client.copy_file(&input).await.map_err(map_ids_error)?;
    Ok(())
}

async fn record_adrive_batch_result(
    report: &mut BatchReport,
    checkpoint_path: Option<&Path>,
    checkpoint: &mut BatchCheckpoint,
    progress: &Option<ProgressBar>,
    operation: &str,
    source: String,
    destination: Option<String>,
    item_key: String,
    progress_units: u64,
    result: Result<(), CliError>,
) -> Result<(), CliError> {
    match result {
        Ok(()) => {
            report.push_success(source, destination, operation);
            checkpoint.completed.insert(item_key);
            save_batch_checkpoint(checkpoint_path, checkpoint).await?;
        }
        Err(err) => report.push_failure(source, destination, operation, err),
    }
    tick_progress_by(progress, progress_units);
    Ok(())
}

async fn download_file(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
) -> Result<i32, CliError> {
    let source = parse_file_uri(&args.source)?;
    let dest_path = local_destination_path(&args.destination, &source)?;
    let head = client
        .head_file(&HeadFileInput::new(
            &source.instance,
            &source.space,
            &source.path,
        ))
        .await
        .map_err(map_ids_error)?;
    if should_skip_local_destination(
        &dest_path,
        Some(head.updated_at),
        runtime.overwrite_strategy,
    )? {
        write_adrive_single_report(
            args.report_path.as_deref(),
            "ve-adrive cp",
            format_target(&source),
            Some(dest_path.display().to_string()),
            "download",
            "skipped",
            None,
            args.report_failures_only,
        )
        .await?;
        return Ok(0);
    }
    if let Some(parent) = dest_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let source_size = head.content_length.max(0) as u64;
    if args.checkpoint && source_size >= runtime.checkpoint_threshold {
        let rate_limiter = rate_limiter_from_limit(args.bandwidth_limit.as_deref())?;
        let parts = download_adrive_file_ranges(
            client,
            &source,
            &head.etag,
            source_size,
            &dest_path,
            runtime,
            rate_limiter,
        )
        .await?;
        if head.hash_crc64_ecma != 0 {
            let local_crc64 = local_file_crc64(&dest_path).await?;
            if local_crc64 != head.hash_crc64_ecma {
                let _ = tokio::fs::remove_file(&dest_path).await;
                return Err(CliError::TransferFailed(format!(
                    "CRC64 mismatch for '{}': local={}, remote={}",
                    dest_path.display(),
                    local_crc64,
                    head.hash_crc64_ecma
                )));
            }
        }
        write_adrive_single_report(
            args.report_path.as_deref(),
            "ve-adrive cp",
            format_target(&source),
            Some(dest_path.display().to_string()),
            "range-download",
            "succeeded",
            None,
            args.report_failures_only,
        )
        .await?;
        output_envelope(
            global,
            &Envelope::success(
                "ve-adrive cp",
                json!({
                    "operation": "range-download",
                    "source": format_target(&source),
                    "destination": dest_path.display().to_string(),
                    "bytes": source_size,
                    "range_parts": parts,
                    "checkpoint": {
                        "enabled": args.checkpoint,
                        "scope": "range_download",
                    },
                    "status": "succeeded",
                }),
            ),
        )?;
        return Ok(0);
    }
    let resume_offset =
        if args.checkpoint && source_size >= runtime.checkpoint_threshold && dest_path.exists() {
            tokio::fs::metadata(&dest_path).await?.len()
        } else {
            0
        };
    let mut input = GetFileInput::new(&source.instance, &source.space, &source.path);
    input.if_match = Some(head.etag.clone());
    if resume_offset > 0 {
        input = input.with_range_raw(format!("bytes={resume_offset}-"));
    }
    let out = client.get_file(&input).await.map_err(map_ids_error)?;
    let rate_limiter = rate_limiter_from_limit(args.bandwidth_limit.as_deref())?;
    let bytes_written =
        write_download_stream(out, &dest_path, resume_offset > 0, rate_limiter).await?;
    if head.hash_crc64_ecma != 0 {
        let local_crc64 = local_file_crc64(&dest_path).await?;
        if local_crc64 != head.hash_crc64_ecma {
            let _ = tokio::fs::remove_file(&dest_path).await;
            return Err(CliError::TransferFailed(format!(
                "CRC64 mismatch for '{}': local={}, remote={}",
                dest_path.display(),
                local_crc64,
                head.hash_crc64_ecma
            )));
        }
    }
    write_adrive_single_report(
        args.report_path.as_deref(),
        "ve-adrive cp",
        format_target(&source),
        Some(dest_path.display().to_string()),
        "download",
        "succeeded",
        None,
        args.report_failures_only,
    )
    .await?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive cp",
            json!({
                "operation": "download",
                "source": format_target(&source),
                "destination": dest_path.display().to_string(),
                "bytes": bytes_written,
                "resume_offset": resume_offset,
                "checkpoint": {
                    "enabled": args.checkpoint,
                    "scope": "range_download",
                },
                "status": "succeeded",
            }),
        ),
    )?;
    Ok(0)
}

async fn copy_remote_file(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &CpArgs,
    runtime: TransferRuntimeConfig,
) -> Result<i32, CliError> {
    let source = parse_file_uri(&args.source)?;
    let dest = parse_adrive_uri(&args.destination, false)?;
    if source.instance != dest.instance {
        return Err(CliError::ValidationError(
            "ve-adrive remote copy requires source and destination in the same instance"
                .to_string(),
        ));
    }
    let destination_path = remote_file_path_for_destination(&dest, &source)?;
    let destination = ParsedADriveUri {
        path: destination_path.clone(),
        ..dest.clone()
    };
    let source_head = client
        .head_file(&HeadFileInput::new(
            &source.instance,
            &source.space,
            &source.path,
        ))
        .await
        .map_err(map_ids_error)?;
    if should_skip_remote_destination(
        client,
        &destination.instance,
        &destination.space,
        &destination.path,
        Some(source_head.updated_at),
        runtime.overwrite_strategy,
    )
    .await?
    {
        write_adrive_single_report(
            args.report_path.as_deref(),
            "ve-adrive cp",
            format_target(&source),
            Some(format_target(&destination)),
            "remote-copy",
            "skipped",
            None,
            args.report_failures_only,
        )
        .await?;
        return Ok(0);
    }
    let input = CopyFileInput {
        instance_id: source.instance.clone(),
        space_id: source.space.clone(),
        file_path: source.path.clone(),
        copy_to_space_id: destination.space.clone(),
        copy_to_path: destination.path.clone(),
        copy_source_if_match: Some(source_head.etag.clone()),
        auto_rename: false,
    };
    let out = client.copy_file(&input).await.map_err(map_ids_error)?;
    write_adrive_single_report(
        args.report_path.as_deref(),
        "ve-adrive cp",
        format_target(&source),
        Some(format_target(&destination)),
        "remote-copy",
        "succeeded",
        None,
        args.report_failures_only,
    )
    .await?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive cp",
            json!({
                "operation": "remote-copy",
                "source": format_target(&source),
                "destination": format_target(&destination),
                "copy_to_file_path": out.copy_to_file_path,
                "copy_status": out.status,
                "request_id": out.response_info.request_id(),
                "status": "succeeded",
            }),
        ),
    )?;
    Ok(0)
}

async fn copy_single_for_move(
    client: &IdsClient,
    args: &MvArgs,
) -> Result<serde_json::Value, CliError> {
    let source_is_remote = args.source.starts_with("adrive://");
    let dest_is_remote = args.destination.starts_with("adrive://");
    match (source_is_remote, dest_is_remote) {
        (false, true) => {
            let dest = parse_adrive_uri(&args.destination, false)?;
            let source_path = PathBuf::from(&args.source);
            if !source_path.is_file() {
                return Err(CliError::ValidationError(format!(
                    "move source must be a file: {}",
                    args.source
                )));
            }
            let file_path = remote_file_path_for_upload(&dest, source_path.as_path())?;
            let size = tokio::fs::metadata(&source_path).await?.len();
            let out = client
                .put_file(
                    PutFileInput::new(
                        dest.instance.clone(),
                        dest.space.clone(),
                        file_path.clone(),
                        IdsBody::from_file(source_path),
                    )
                    .with_content_length(size),
                )
                .await
                .map_err(map_ids_error)?;
            Ok(json!({
                "operation": "upload",
                "destination": format_target(&ParsedADriveUri { path: file_path, ..dest }),
                "size": out.size,
                "etag": out.etag,
                "request_id": out.response_info.request_id(),
            }))
        }
        (true, false) => {
            let source = parse_adrive_uri(&args.source, false)?;
            ensure_adrive_mv_folder_requires_recursive(
                &args.source,
                source.path.ends_with('/'),
                args.recursive,
            )?;
            let source = parse_file_target(source)?;
            let source_head = client
                .head_file(&HeadFileInput::new(
                    &source.instance,
                    &source.space,
                    &source.path,
                ))
                .await
                .map_err(map_ids_error)?;
            ensure_adrive_mv_folder_requires_recursive(
                &args.source,
                source_head.is_folder,
                args.recursive,
            )?;
            let out = client
                .get_file(&GetFileInput::new(
                    &source.instance,
                    &source.space,
                    &source.path,
                ))
                .await
                .map_err(map_ids_error)?;
            let dest_path = local_destination_path(&args.destination, &source)?;
            if let Some(parent) = dest_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let bytes = out.read_all().await.map_err(map_ids_error)?;
            tokio::fs::write(&dest_path, &bytes).await?;
            Ok(json!({
                "operation": "download",
                "destination": dest_path.display().to_string(),
                "bytes": bytes.len(),
            }))
        }
        (true, true) => {
            let source = parse_adrive_uri(&args.source, false)?;
            ensure_adrive_mv_folder_requires_recursive(
                &args.source,
                source.path.ends_with('/'),
                args.recursive,
            )?;
            let source = parse_file_target(source)?;
            let dest = parse_adrive_uri(&args.destination, false)?;
            if source.instance != dest.instance {
                return Err(CliError::ValidationError(
                    "ve-adrive remote move requires source and destination in the same instance"
                        .to_string(),
                ));
            }
            let destination_path = remote_file_path_for_destination(&dest, &source)?;
            let destination = ParsedADriveUri {
                path: destination_path.clone(),
                ..dest.clone()
            };
            let source_head = client
                .head_file(&HeadFileInput::new(
                    &source.instance,
                    &source.space,
                    &source.path,
                ))
                .await
                .map_err(map_ids_error)?;
            ensure_adrive_mv_folder_requires_recursive(
                &args.source,
                source_head.is_folder,
                args.recursive,
            )?;
            let out = client
                .copy_file(&CopyFileInput {
                    instance_id: source.instance.clone(),
                    space_id: source.space.clone(),
                    file_path: source.path.clone(),
                    copy_to_space_id: destination.space.clone(),
                    copy_to_path: destination.path.clone(),
                    copy_source_if_match: Some(source_head.etag),
                    auto_rename: false,
                })
                .await
                .map_err(map_ids_error)?;
            Ok(json!({
                "operation": "remote-copy",
                "destination": format_target(&destination),
                "copy_to_file_path": out.copy_to_file_path,
                "copy_status": out.status,
                "request_id": out.response_info.request_id(),
            }))
        }
        (false, false) => Err(CliError::ValidationError(
            "ve-adrive mv requires one side to be adrive://instance/space/path".to_string(),
        )),
    }
}

fn ensure_adrive_mv_folder_requires_recursive(
    source: &str,
    is_folder_source: bool,
    recursive: bool,
) -> Result<(), CliError> {
    if is_folder_source && !recursive {
        return Err(CliError::ValidationError(format!(
            "ve-adrive mv directory source requires --recursive: {}",
            source
        )));
    }
    Ok(())
}

fn adrive_recursive_mv_source_folder_path(source: &ParsedADriveUri) -> Result<String, CliError> {
    let folder_path = source.path.trim_end_matches('/').to_string();
    if folder_path.is_empty() {
        return Err(CliError::ValidationError(
            "ve-adrive mv --recursive requires a source directory path, not a space root"
                .to_string(),
        ));
    }
    Ok(folder_path)
}

fn adrive_recursive_mv_destination_folder_path(
    source: &str,
    destination: &ParsedADriveUri,
    include_parent: bool,
) -> Result<String, CliError> {
    let destination_path = destination.path.trim_end_matches('/');
    let parent_prefix = recursive_adrive_source_parent_prefix(source, include_parent)?;
    Ok(match parent_prefix {
        Some(parent) => join_remote_path(destination_path, &parent),
        None => destination_path.to_string(),
    })
}

async fn execute_mv(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &MvArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_mv_args_by_name(client, args).await?;
    let args = &resolved_args;
    if args.source.starts_with("adrive://") && args.destination.starts_with("adrive://") {
        let source = parse_adrive_uri(&args.source, false)?;
        let dest = parse_adrive_uri(&args.destination, false)?;
        if source.instance == dest.instance && source.space == dest.space {
            reject_single_transfer_artifacts(
                "ve-adrive mv",
                args.report_path.as_deref(),
                args.report_failures_only,
                args.manifest_path.as_deref(),
                args.no_manifest,
                args.batch_concurrency,
                args.list_concurrency,
            )?;
            // [Review Fix #5] Same instance+space ADrive move is a service-side
            // rename for both file and folder paths, so it must not generate
            // batch manifest/report even when --recursive is present.
            if args.recursive {
                let folder_path = adrive_recursive_mv_source_folder_path(&source)?;
                let new_folder_path = adrive_recursive_mv_destination_folder_path(
                    &args.source,
                    &dest,
                    args.include_parent,
                )?;
                let destination = ParsedADriveUri {
                    path: new_folder_path.clone(),
                    ..dest.clone()
                };
                let out = client
                    .rename_folder(&RenameFolderInput {
                        instance_id: source.instance.clone(),
                        space_id: source.space.clone(),
                        folder_path,
                        new_folder_path,
                        forbid_overwrite: !args.force,
                    })
                    .await
                    .map_err(map_ids_error)?;
                output_envelope(
                    global,
                    &Envelope::success(
                        "ve-adrive mv",
                        json!({
                            "operation": "rename_folder",
                            "source": format_target(&source),
                            "destination": format_target(&destination),
                            "folder_path": out.folder_path,
                            "request_id": out.response_info.request_id(),
                            "status": "succeeded",
                        }),
                    ),
                )?;
                return Ok(0);
            }
            ensure_adrive_mv_folder_requires_recursive(
                &args.source,
                source.path.ends_with('/'),
                args.recursive,
            )?;
            let source_head = client
                .head_file(&HeadFileInput::new(
                    &source.instance,
                    &source.space,
                    &source.path,
                ))
                .await
                .map_err(map_ids_error)?;
            let should_rename_folder = source_head.is_folder;
            ensure_adrive_mv_folder_requires_recursive(
                &args.source,
                should_rename_folder,
                args.recursive,
            )?;

            let source = parse_file_target(source)?;
            let destination_path = remote_file_path_for_destination(&dest, &source)?;
            let dest = parse_file_target(ParsedADriveUri {
                path: destination_path,
                ..dest
            })?;
            let out = client
                .rename_file(&RenameFileInput {
                    instance_id: source.instance.clone(),
                    space_id: source.space.clone(),
                    file_path: source.path.clone(),
                    new_file_path: dest.path.clone(),
                    forbid_overwrite: !args.force,
                })
                .await
                .map_err(map_ids_error)?;
            output_envelope(
                global,
                &Envelope::success(
                    "ve-adrive mv",
                    json!({
                        "operation": "rename_file",
                        "source": format_target(&source),
                        "destination": format_target(&dest),
                        "file_path": out.file_path,
                        "request_id": out.response_info.request_id(),
                        "status": "succeeded",
                    }),
                ),
            )?;
            return Ok(0);
        }
    }

    if args.recursive {
        if args.no_manifest {
            let runtime = effective_mv_runtime_config(global, args)?;
            return execute_recursive_mv_streaming_no_manifest(global, client, args, runtime).await;
        }
        let cp_args = CpArgs {
            source: args.source.clone(),
            destination: args.destination.clone(),
            by_name: false,
            recursive: true,
            include_parent: args.include_parent,
            include: args.include.clone(),
            exclude: args.exclude.clone(),
            checkpoint: false,
            checkpoint_dir: args.checkpoint_dir.clone(),
            checkpoint_threshold: None,
            batch_concurrency: args.batch_concurrency,
            list_concurrency: args.list_concurrency,
            multipart_concurrency: None,
            progress_granularity: None,
            overwrite_strategy: Some(OverwriteStrategy::Force),
            report_path: args.report_path.clone(),
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
        };
        let runtime = effective_cp_runtime_config(global, &cp_args)?;
        let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
        let list_echo_enabled =
            effective_list_echo_enabled(global, args.list_echo, args.no_list_echo);
        let mut report = recursive_cp_report(
            client,
            &cp_args,
            runtime,
            list_echo_enabled,
            progress_enabled,
        )
        .await?;
        report.command = "ve-adrive mv".to_string();
        let delete_manifest_items = adrive_delete_source_manifest_items(&report);
        append_adrive_manifest_items(&mut report, delete_manifest_items.clone());
        // [Review Fix #MoveReport] Recursive move is a two-stage operation. Do not
        // delete sources until the copy stage is fully successful, and record the
        // delete-source stage in the same report for deterministic auditing.
        if report.failed > 0 {
            for item in &delete_manifest_items {
                report.push_skipped(item.source.clone(), None, "delete-source");
            }
        } else {
            match cleanup_recursive_move_source(client, &args.source).await {
                Ok(()) => {
                    for item in &delete_manifest_items {
                        report.push_success(item.source.clone(), None, "delete-source");
                    }
                }
                Err(err) => {
                    for item in &delete_manifest_items {
                        report.push_failure(
                            item.source.clone(),
                            None,
                            "delete-source",
                            err.to_string(),
                        );
                    }
                }
            }
        }
        let report_path =
            effective_report_path(global, args.report_path.as_deref(), "ve-adrive mv")?;
        let manifest_path = effective_optional_manifest_path(
            global,
            args.manifest_path.as_deref(),
            args.no_manifest,
            "ve-adrive mv",
        )?;
        write_batch_report(
            report_path.as_deref(),
            &report,
            args.report_failures_only,
            manifest_path.is_some(),
        )
        .await?;
        write_adrive_manifest_file(
            manifest_path.as_deref(),
            "ve-adrive mv",
            report.manifest.as_ref(),
        )
        .await?;
        output_envelope(
            global,
            &Envelope::success(
                "ve-adrive mv",
                json!({
                    "operation": "recursive-move",
                    "source": args.source,
                    "destination": args.destination,
                    "summary": batch_summary(&report),
                    "report_path": report_path,
                    "manifest_path": manifest_path,
                    "status": if report.failed == 0 { "succeeded" } else { "partial_failure" },
                }),
            ),
        )?;
        return if report.failed == 0 { Ok(0) } else { Ok(1) };
    }

    reject_single_transfer_artifacts(
        "ve-adrive mv",
        args.report_path.as_deref(),
        args.report_failures_only,
        args.manifest_path.as_deref(),
        args.no_manifest,
        args.batch_concurrency,
        args.list_concurrency,
    )?;
    let copy_result = copy_single_for_move(client, args).await?;
    let destination = copy_result
        .get("destination")
        .and_then(Value::as_str)
        .unwrap_or(&args.destination)
        .to_string();
    if args.source.starts_with("adrive://") {
        let source = parse_file_uri(&args.source)?;
        client
            .delete_file(&DeleteFileInput::new(
                &source.instance,
                &source.space,
                &source.path,
            ))
            .await
            .map_err(map_ids_error)?;
    } else {
        tokio::fs::remove_file(&args.source).await?;
    }
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive mv",
            json!({
                "operation": "copy_delete",
                "source": args.source,
                "destination": destination,
                "copy": copy_result,
                "status": "succeeded",
            }),
        ),
    )?;
    Ok(0)
}

fn effective_mv_runtime_config(
    global: &GlobalArgs,
    args: &MvArgs,
) -> Result<TransferRuntimeConfig, CliError> {
    let cp_args = CpArgs {
        source: args.source.clone(),
        destination: args.destination.clone(),
        by_name: false,
        recursive: true,
        include_parent: args.include_parent,
        include: args.include.clone(),
        exclude: args.exclude.clone(),
        checkpoint: false,
        checkpoint_dir: args.checkpoint_dir.clone(),
        checkpoint_threshold: args.checkpoint_threshold.clone(),
        batch_concurrency: args.batch_concurrency,
        list_concurrency: args.list_concurrency,
        multipart_concurrency: args.multipart_concurrency,
        progress_granularity: args.progress_granularity,
        overwrite_strategy: Some(OverwriteStrategy::Force),
        report_path: args.report_path.clone(),
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
    };
    effective_cp_runtime_config(global, &cp_args)
}

async fn execute_recursive_mv_streaming_no_manifest(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &MvArgs,
    runtime: TransferRuntimeConfig,
) -> Result<i32, CliError> {
    let report_path = effective_report_path(global, args.report_path.as_deref(), "ve-adrive mv")?;
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let progress = streaming_batch_progress(progress_enabled, "ve-adrive mv");
    let mut report = BatchReport::new("ve-adrive mv");
    let stream_result = {
        let mut context = ADriveStreamMoveContext {
            client,
            args,
            runtime,
            progress: &progress,
            report: &mut report,
            in_flight: FuturesUnordered::new(),
            limit: runtime.batch_concurrency.max(1),
        };
        let result = match (
            args.source.starts_with("adrive://"),
            args.destination.starts_with("adrive://"),
        ) {
            (false, true) => stream_adrive_upload_move_no_manifest(&mut context).await,
            (true, false) => stream_adrive_download_move_no_manifest(&mut context).await,
            (true, true) => stream_adrive_remote_move_no_manifest(&mut context).await,
            (false, false) => Err(CliError::ValidationError(
                "ve-adrive mv --recursive requires one side to be adrive://instance/space/path"
                    .to_string(),
            )),
        };
        context.drain_all().await;
        result
    };
    let cleanup_result = if stream_result.is_ok()
        && report.failed == 0
        && args.include.is_none()
        && args.exclude.is_none()
    {
        cleanup_recursive_move_source(client, &args.source).await
    } else {
        Ok(())
    };
    if stream_result.is_ok()
        && report.failed == 0
        && args.include.is_none()
        && args.exclude.is_none()
    {
        match &cleanup_result {
            Ok(()) => report.push_success(args.source.clone(), None, "delete-source-root"),
            Err(err) => report.push_failure(
                args.source.clone(),
                None,
                "delete-source-root",
                err.to_string(),
            ),
        }
        tick_progress(&progress);
    }
    finish_streaming_progress(progress, report.total as u64);
    let failed = report.failed;
    write_batch_report(
        report_path.as_deref(),
        &report,
        args.report_failures_only,
        false,
    )
    .await?;
    stream_result?;
    cleanup_result?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive mv",
            json!({
                "operation": "recursive-move",
                "source": args.source,
                "destination": args.destination,
                "summary": batch_summary(&report),
                "report_path": report_path,
                "manifest_path": Value::Null,
                "status": if failed == 0 { "succeeded" } else { "partial_failure" },
            }),
        ),
    )?;
    if failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

struct ADriveStreamMoveContext<'a> {
    client: &'a IdsClient,
    args: &'a MvArgs,
    runtime: TransferRuntimeConfig,
    progress: &'a Option<ProgressBar>,
    report: &'a mut BatchReport,
    in_flight: FuturesUnordered<ADriveMoveFuture<'a>>,
    limit: usize,
}

impl<'a> ADriveStreamMoveContext<'a> {
    async fn queue(&mut self, task: ADriveMoveFuture<'a>) {
        while self.in_flight.len() >= self.limit {
            if !self.drain_one().await {
                break;
            }
        }
        self.in_flight.push(task);
    }

    async fn drain_one(&mut self) -> bool {
        let Some(result) = self.in_flight.next().await else {
            return false;
        };
        record_adrive_stream_move_result(self.progress, self.report, result);
        true
    }

    async fn drain_all(&mut self) {
        while self.drain_one().await {}
    }
}

async fn stream_adrive_upload_move_no_manifest(
    context: &mut ADriveStreamMoveContext<'_>,
) -> Result<(), CliError> {
    let destination = parse_adrive_uri(&context.args.destination, false)?;
    let source_root = PathBuf::from(&context.args.source);
    if !source_root.is_dir() {
        return Err(CliError::ValidationError(format!(
            "recursive move source must be a directory: {}",
            context.args.source
        )));
    }
    let parent_prefix =
        recursive_adrive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let mut pending = vec![source_root.clone()];
    while let Some(directory) = pending.pop() {
        let mut child_directories = Vec::new();
        for entry in sorted_adrive_read_dir_entries(&directory)? {
            let path = entry.path();
            if path.is_dir() {
                child_directories.push(path);
                continue;
            }
            if path.is_file() {
                queue_adrive_upload_move_file(
                    context,
                    &destination,
                    &source_root,
                    parent_prefix.as_deref(),
                    path,
                )
                .await?;
            }
        }
        child_directories.sort();
        child_directories.reverse();
        pending.extend(child_directories);
    }
    Ok(())
}

async fn queue_adrive_upload_move_file(
    context: &mut ADriveStreamMoveContext<'_>,
    destination: &ParsedADriveUri,
    source_root: &Path,
    parent_prefix: Option<&str>,
    file: PathBuf,
) -> Result<(), CliError> {
    let relative = file.strip_prefix(source_root).map_err(|err| {
        CliError::ValidationError(format!("failed to derive relative path: {}", err))
    })?;
    let source_relative = normalize_local_relative_path(relative)?;
    let relative = prepend_adrive_parent_prefix(&source_relative, parent_prefix);
    if !path_matches_filters(
        &relative,
        context.args.include.as_deref(),
        context.args.exclude.as_deref(),
    ) {
        return Ok(());
    }
    let remote_path = join_remote_path(&destination.path, &relative);
    let remote_target = format_target(&ParsedADriveUri {
        instance: destination.instance.clone(),
        space: destination.space.clone(),
        path: remote_path.clone(),
    });
    let source_label = file.display().to_string();
    let instance = destination.instance.clone();
    let space = destination.space.clone();
    let checkpoint_dir = context.args.checkpoint_dir.clone();
    let runtime = context.runtime;
    let client = context.client;
    context
        .queue(Box::pin(async move {
            let copy_result = upload_adrive_file_for_batch(
                client,
                &instance,
                &space,
                &remote_path,
                &file,
                false,
                checkpoint_dir.as_deref(),
                None,
                runtime,
            )
            .await;
            let delete_result = if copy_result.is_ok() {
                Some(tokio::fs::remove_file(&file).await.map_err(CliError::Io))
            } else {
                None
            };
            ADriveMoveFutureOutput {
                copy_source: source_label.clone(),
                copy_destination: Some(remote_target),
                copy_operation: "upload",
                copy_result,
                delete_source: source_label,
                delete_result,
            }
        }))
        .await;
    Ok(())
}

async fn stream_adrive_download_move_no_manifest(
    context: &mut ADriveStreamMoveContext<'_>,
) -> Result<(), CliError> {
    let source = parse_adrive_uri(&context.args.source, false)?;
    let destination_root = PathBuf::from(&context.args.destination);
    let source_prefix = trim_folder_prefix(&source.path);
    let parent_prefix =
        recursive_adrive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let mut pending_prefixes = vec![source_prefix.clone()];
    let mut seen_prefixes = HashSet::new();
    while let Some(prefix) = pending_prefixes.pop() {
        if !seen_prefixes.insert(prefix.clone()) {
            continue;
        }
        let mut marker = None;
        loop {
            let mut input = ListFilesInput::new(&source.instance, &source.space)
                .with_limit(1000)
                .with_delimiter("/");
            if !prefix.is_empty() {
                input = input.with_prefix(&prefix);
            }
            if let Some(value) = marker.take() {
                input = input.with_marker(value);
            }
            let out = context
                .client
                .list_files(&input)
                .await
                .map_err(map_ids_error)?;
            for file in out.files {
                queue_adrive_download_move_file(
                    context,
                    &source,
                    &source_prefix,
                    parent_prefix.as_deref(),
                    &destination_root,
                    file,
                )
                .await?;
            }
            for folder in out.folders {
                pending_prefixes.push(trim_folder_prefix(&folder.folder));
            }
            if !out.is_truncated || out.next_marker.is_empty() {
                break;
            }
            marker = Some(out.next_marker);
        }
    }
    Ok(())
}

async fn queue_adrive_download_move_file(
    context: &mut ADriveStreamMoveContext<'_>,
    source: &ParsedADriveUri,
    source_prefix: &str,
    parent_prefix: Option<&str>,
    destination_root: &Path,
    file: FileInfo,
) -> Result<(), CliError> {
    let source_relative = remote_relative_path(&file.file_path, source_prefix);
    let relative = prepend_adrive_parent_prefix(&source_relative, parent_prefix);
    if !path_matches_filters(
        &relative,
        context.args.include.as_deref(),
        context.args.exclude.as_deref(),
    ) {
        return Ok(());
    }
    let destination_path = destination_root.join(relative);
    if let Some(parent) = destination_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let remote_source = format!(
        "adrive://{}/{}/{}",
        source.instance, source.space, file.file_path
    );
    let local_destination = destination_path.display().to_string();
    let instance = source.instance.clone();
    let space = source.space.clone();
    let file_path = file.file_path.clone();
    let etag = file.etag.clone();
    let crc64 = file.hash_crc64_ecma;
    let file_size = adrive_file_size(&file);
    let runtime = context.runtime;
    let client = context.client;
    context
        .queue(Box::pin(async move {
            let copy_result = download_adrive_file_for_batch(
                client,
                &instance,
                &space,
                &file_path,
                &etag,
                crc64,
                file_size,
                &destination_path,
                false,
                None,
                runtime,
            )
            .await;
            let delete_result = if copy_result.is_ok() {
                Some(
                    client
                        .delete_file(&DeleteFileInput::new(&instance, &space, &file_path))
                        .await
                        .map(|_| ())
                        .map_err(map_ids_error),
                )
            } else {
                None
            };
            ADriveMoveFutureOutput {
                copy_source: remote_source.clone(),
                copy_destination: Some(local_destination),
                copy_operation: "download",
                copy_result,
                delete_source: remote_source,
                delete_result,
            }
        }))
        .await;
    Ok(())
}

async fn stream_adrive_remote_move_no_manifest(
    context: &mut ADriveStreamMoveContext<'_>,
) -> Result<(), CliError> {
    let source = parse_adrive_uri(&context.args.source, false)?;
    let destination = parse_adrive_uri(&context.args.destination, false)?;
    if source.instance != destination.instance {
        return Err(CliError::ValidationError(
            "ve-adrive recursive remote move requires source and destination in the same instance"
                .to_string(),
        ));
    }
    let source_prefix = trim_folder_prefix(&source.path);
    let parent_prefix =
        recursive_adrive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let mut pending_prefixes = vec![source_prefix.clone()];
    let mut seen_prefixes = HashSet::new();
    while let Some(prefix) = pending_prefixes.pop() {
        if !seen_prefixes.insert(prefix.clone()) {
            continue;
        }
        let mut marker = None;
        loop {
            let mut input = ListFilesInput::new(&source.instance, &source.space)
                .with_limit(1000)
                .with_delimiter("/");
            if !prefix.is_empty() {
                input = input.with_prefix(&prefix);
            }
            if let Some(value) = marker.take() {
                input = input.with_marker(value);
            }
            let out = context
                .client
                .list_files(&input)
                .await
                .map_err(map_ids_error)?;
            for file in out.files {
                queue_adrive_remote_move_file(
                    context,
                    &source,
                    &destination,
                    &source_prefix,
                    parent_prefix.as_deref(),
                    file,
                )
                .await?;
            }
            for folder in out.folders {
                pending_prefixes.push(trim_folder_prefix(&folder.folder));
            }
            if !out.is_truncated || out.next_marker.is_empty() {
                break;
            }
            marker = Some(out.next_marker);
        }
    }
    Ok(())
}

async fn queue_adrive_remote_move_file(
    context: &mut ADriveStreamMoveContext<'_>,
    source: &ParsedADriveUri,
    destination: &ParsedADriveUri,
    source_prefix: &str,
    parent_prefix: Option<&str>,
    file: FileInfo,
) -> Result<(), CliError> {
    let source_relative = remote_relative_path(&file.file_path, source_prefix);
    let relative = prepend_adrive_parent_prefix(&source_relative, parent_prefix);
    if !path_matches_filters(
        &relative,
        context.args.include.as_deref(),
        context.args.exclude.as_deref(),
    ) {
        return Ok(());
    }
    let destination_path = join_remote_path(&destination.path, &relative);
    let remote_source = format!(
        "adrive://{}/{}/{}",
        source.instance, source.space, file.file_path
    );
    let remote_destination = format!(
        "adrive://{}/{}/{}",
        destination.instance, destination.space, destination_path
    );
    let instance = source.instance.clone();
    let source_space = source.space.clone();
    let source_path = file.file_path.clone();
    let destination_space = destination.space.clone();
    let source_etag = file.etag.clone();
    let client = context.client;
    context
        .queue(Box::pin(async move {
            let copy_result = copy_adrive_file_simple(
                client,
                &instance,
                &source_space,
                &source_path,
                &destination_space,
                &destination_path,
                &source_etag,
            )
            .await;
            let delete_result = if copy_result.is_ok() {
                Some(
                    client
                        .delete_file(&DeleteFileInput::new(
                            &instance,
                            &source_space,
                            &source_path,
                        ))
                        .await
                        .map(|_| ())
                        .map_err(map_ids_error),
                )
            } else {
                None
            };
            ADriveMoveFutureOutput {
                copy_source: remote_source.clone(),
                copy_destination: Some(remote_destination),
                copy_operation: "remote-copy",
                copy_result,
                delete_source: remote_source,
                delete_result,
            }
        }))
        .await;
    Ok(())
}

fn record_adrive_stream_move_result(
    progress: &Option<ProgressBar>,
    report: &mut BatchReport,
    result: ADriveMoveFutureOutput,
) {
    match result.copy_result {
        Ok(()) => report.push_success(
            result.copy_source,
            result.copy_destination,
            result.copy_operation,
        ),
        Err(err) => {
            report.push_failure(
                result.copy_source,
                result.copy_destination,
                result.copy_operation,
                err,
            );
            report.push_skipped(result.delete_source, None, "delete-source");
            tick_progress_by(progress, 2);
            return;
        }
    }
    match result.delete_result {
        Some(Ok(())) => report.push_success(result.delete_source, None, "delete-source"),
        Some(Err(err)) => report.push_failure(result.delete_source, None, "delete-source", err),
        None => report.push_skipped(result.delete_source, None, "delete-source"),
    }
    tick_progress_by(progress, 2);
}

async fn execute_sync(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &SyncArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_sync_args_by_name(client, args).await?;
    let args = &resolved_args;
    let cp_args = CpArgs {
        source: args.source.clone(),
        destination: args.destination.clone(),
        by_name: false,
        recursive: true,
        include_parent: args.include_parent,
        include: args.include.clone(),
        exclude: args.exclude.clone(),
        checkpoint: true,
        checkpoint_dir: args.checkpoint_dir.clone(),
        checkpoint_threshold: args.checkpoint_threshold.clone(),
        batch_concurrency: args.batch_concurrency,
        list_concurrency: args.list_concurrency,
        multipart_concurrency: args.multipart_concurrency,
        progress_granularity: args.progress_granularity,
        overwrite_strategy: args.overwrite_strategy,
        report_path: args.report_path.clone(),
        report_failures_only: args.report_failures_only,
        manifest_path: args.manifest_path.clone(),
        no_manifest: args.no_manifest,
        bandwidth_limit: args.bandwidth_limit.clone(),
        list_echo: args.list_echo,
        no_list_echo: args.no_list_echo,
        progress: args.progress,
        no_progress: args.no_progress,
        force: args.force,
        no_clobber: false,
    };
    let runtime = effective_cp_runtime_config(global, &cp_args)?;
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let list_echo_enabled = effective_list_echo_enabled(global, args.list_echo, args.no_list_echo);
    if !args.source.starts_with("adrive://") && !args.destination.starts_with("adrive://") {
        return Err(CliError::ValidationError(
            "ve-adrive sync requires one side to be adrive://instance/space/path".to_string(),
        ));
    }
    let (mut report, copy_stream_result, copy_drain_result) = if args.no_manifest {
        recursive_cp_streaming_no_manifest_report(
            client,
            &cp_args,
            runtime,
            "ve-adrive sync copy",
            "ve-adrive sync",
            progress_enabled,
        )
        .await?
    } else {
        let mut report = match (
            args.source.starts_with("adrive://"),
            args.destination.starts_with("adrive://"),
        ) {
            (false, true) => {
                upload_directory_recursive(
                    client,
                    &cp_args,
                    runtime,
                    list_echo_enabled,
                    progress_enabled,
                )
                .await?
            }
            (true, false) => {
                download_directory_recursive(
                    client,
                    &cp_args,
                    runtime,
                    list_echo_enabled,
                    progress_enabled,
                )
                .await?
            }
            (true, true) => {
                copy_remote_recursive(
                    client,
                    &cp_args,
                    runtime,
                    list_echo_enabled,
                    progress_enabled,
                )
                .await?
            }
            (false, false) => unreachable!("ve-adrive sync endpoint shape was validated above"),
        };
        report.command = "ve-adrive sync".to_string();
        (report, Ok(()), Ok(()))
    };
    let copy_had_fatal_error = copy_stream_result.is_err() || copy_drain_result.is_err();
    let report_path = effective_report_path(global, args.report_path.as_deref(), "ve-adrive sync")?;
    let manifest_path = effective_optional_manifest_path(
        global,
        args.manifest_path.as_deref(),
        args.no_manifest,
        "ve-adrive sync",
    )?;
    let delete_plan_result = if args.delete && !copy_had_fatal_error {
        let mut scan_progress =
            PlanScanProgress::new(list_echo_enabled, "ve-adrive sync plan", &args.destination);
        let delete_plan =
            build_adrive_sync_delete_plan(client, args, runtime.list_concurrency).await?;
        scan_progress.finish_with_count(delete_plan.len() as u64, "item(s)");
        Ok(delete_plan)
    } else {
        Ok(Vec::new())
    };
    let delete_plan = match delete_plan_result {
        Ok(delete_plan) => delete_plan,
        Err(err) => {
            // [Review Fix #SyncReportOnPlanError] Preserve the completed copy
            // rows even when sync delete planning fails after streaming copy.
            write_batch_report(
                report_path.as_deref(),
                &report,
                args.report_failures_only,
                manifest_path.is_some(),
            )
            .await?;
            copy_stream_result?;
            copy_drain_result?;
            return Err(err);
        }
    };
    if !args.no_manifest {
        append_adrive_manifest_items(&mut report, delete_plan.clone());
    }
    let deleted = if args.delete && !copy_had_fatal_error && report.failed == 0 {
        execute_adrive_sync_delete_plan(
            client,
            delete_plan,
            runtime.batch_concurrency,
            progress_enabled,
            &mut report,
        )
        .await?
    } else if args.delete && !copy_had_fatal_error {
        // [Review Fix #6] ADrive sync uses the same delete safety gate as TOS:
        // copy failures turn planned destination cleanup into skipped rows.
        for item in delete_plan {
            report.push_skipped(item.source, None, &item.operation);
        }
        0
    } else {
        0
    };
    write_batch_report(
        report_path.as_deref(),
        &report,
        args.report_failures_only,
        manifest_path.is_some(),
    )
    .await?;
    write_adrive_manifest_file(
        manifest_path.as_deref(),
        "ve-adrive sync",
        report.manifest.as_ref(),
    )
    .await?;
    copy_stream_result?;
    copy_drain_result?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive sync",
            json!({
                "operation": "sync",
                "source": args.source,
                "destination": args.destination,
                "delete": args.delete,
                "deleted": deleted,
                "summary": batch_summary(&report),
                "report_path": report_path,
                "manifest_path": manifest_path,
                "status": if report.failed == 0 { "succeeded" } else { "partial_failure" },
            }),
        ),
    )?;
    if report.failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

async fn execute_create(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &CreateArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_create_args_by_name(client, args).await?;
    let args = &resolved_args;
    let response = match resolve_create_target(args)? {
        ADriveCreateTarget::Instance { name } => {
            let input = CreateInstanceInput {
                name: name.clone(),
                display_name: args.display_name.clone(),
                description: args.description.clone().unwrap_or_default(),
                ..Default::default()
            };
            let out = client
                .create_instance(&input)
                .await
                .map_err(map_ids_error)?;
            let request_id = out.response_info.request_id().to_string();
            let instance = out.instance;
            json!({
                "resource_type": "instance",
                "target": format!("adrive://{}", instance.instance_id),
                "instance_id": instance.instance_id,
                "name": instance.name,
                "display_name": instance.display_name,
                "description": instance.description,
                "request_id": request_id,
                "status": "succeeded",
            })
        }
        ADriveCreateTarget::Space { instance, name } => {
            let input = CreateSpaceInput {
                instance_id: instance.clone(),
                space_name: name.clone(),
                display_name: args.display_name.clone(),
                index_enabled: args.index_enabled,
                description: args.description.clone(),
                ..Default::default()
            };
            let out = client.create_space(&input).await.map_err(map_ids_error)?;
            let request_id = out.response_info.request_id().to_string();
            let space = out.space;
            json!({
                "resource_type": "space",
                "target": format!("adrive://{}/{}", instance, space.space_id),
                "instance_id": instance,
                "space_id": space.space_id,
                "name": space.name,
                "display_name": space.display_name,
                "description": space.description,
                "index_enabled": args.index_enabled,
                "request_id": request_id,
                "status": "succeeded",
            })
        }
    };
    output_envelope(global, &Envelope::success("ve-adrive crt", response))?;
    Ok(0)
}

async fn execute_delete(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &DeleteArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_delete_args_by_name(client, args).await?;
    let args = &resolved_args;
    let target = resolve_delete_target(args)?;
    let response = match target {
        ADriveTarget::Instance { instance } => {
            let out = client
                .delete_instance(&DeleteInstanceInput::new(&instance))
                .await
                .map_err(map_ids_error)?;
            json!({
                "resource_type": "instance",
                "target": format!("adrive://{instance}"),
                "deleted": out.deleted,
                "request_id": out.response_info.request_id(),
                "status": "succeeded",
            })
        }
        ADriveTarget::Space { instance, space } => {
            let out = client
                .delete_space(&DeleteSpaceInput::new(&instance, &space))
                .await
                .map_err(map_ids_error)?;
            json!({
                "resource_type": "space",
                "target": format!("adrive://{instance}/{space}"),
                "deleted": out.deleted,
                "request_id": out.response_info.request_id(),
                "status": "succeeded",
            })
        }
        ADriveTarget::Instances | ADriveTarget::Path(_) => {
            return Err(CliError::ValidationError(
                "ve-adrive del only removes instances or spaces".to_string(),
            ));
        }
    };
    output_envelope(global, &Envelope::success("ve-adrive del", response))?;
    Ok(0)
}

fn adrive_rm_should_delete_folder(args: &RmArgs, target: &ParsedADriveUri) -> bool {
    args.recursive || target.path.ends_with('/') || target.file().is_none()
}

async fn execute_rm(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &RmArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_rm_args_by_name(client, args).await?;
    let args = &resolved_args;
    let target = resolve_rm_target(args)?;
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let list_echo_enabled = effective_list_echo_enabled(global, args.list_echo, args.no_list_echo);
    let (batch_concurrency, list_concurrency) = if args.recursive {
        (
            effective_batch_concurrency(global, args.batch_concurrency)?,
            effective_list_concurrency(global, args.list_concurrency)?,
        )
    } else {
        reject_single_transfer_artifacts(
            "ve-adrive rm",
            args.report_path.as_deref(),
            args.report_failures_only,
            args.manifest_path.as_deref(),
            args.no_manifest,
            args.batch_concurrency,
            args.list_concurrency,
        )?;
        (DEFAULT_BATCH_CONCURRENCY, DEFAULT_LIST_CONCURRENCY)
    };
    let mut direct_recursive_report = None;
    let response = if adrive_rm_should_delete_folder(args, &target) {
        if args.recursive {
            if args.recursive_delete_mode == RecursiveDeleteMode::BottomUp {
                return execute_rm_bottom_up(
                    global,
                    client,
                    args,
                    &target,
                    batch_concurrency,
                    list_concurrency,
                    list_echo_enabled,
                    progress_enabled,
                )
                .await;
            }
            reject_direct_adrive_rm_filters(args)?;
        }
        let folder_path = target.path.trim_end_matches('/');
        let out = client
            .delete_folder(&DeleteFolderInput::new(
                &target.instance,
                &target.space,
                folder_path,
            ))
            .await
            .map_err(map_ids_error)?;
        // [Review Fix #ADrive-RmUploads-1] Keep the service response value so
        // direct recursive rm can include request_id in its batch envelope.
        let response = json!({
            "target": format_target(&target),
            "folder_path": out.folder_path,
            "recursive_delete_mode": args.recursive.then(|| recursive_delete_mode_name(args.recursive_delete_mode)),
            "request_id": out.response_info.request_id(),
            "status": "succeeded",
        });
        if args.recursive && args.recursive_delete_mode == RecursiveDeleteMode::Direct {
            let mut report = BatchReport::new("ve-adrive rm");
            report.push_success(format_target(&target), None, "delete_folder");
            direct_recursive_report = Some(report);
        }
        response
    } else {
        let out = client
            .delete_file(&DeleteFileInput::new(
                &target.instance,
                &target.space,
                &target.path,
            ))
            .await
            .map_err(map_ids_error)?;
        json!({
            "target": format_target(&target),
            "version_id": out.version_id,
            "delete_marker": out.delete_marker,
            "request_id": out.response_info.request_id(),
            "status": "succeeded",
        })
    };
    let is_direct_recursive_delete = direct_recursive_report.is_some();
    let mut upload_report =
        direct_recursive_report.unwrap_or_else(|| BatchReport::new("ve-adrive rm"));
    if args.include_uploads {
        abort_checkpointed_multipart_uploads_for_rm(
            global,
            client,
            &target,
            args.checkpoint_dir.as_deref(),
            progress_enabled,
            &mut upload_report,
        )
        .await?;
    }
    let upload_failed = upload_report.failed;
    if is_direct_recursive_delete {
        let report_path =
            effective_report_path(global, args.report_path.as_deref(), "ve-adrive rm")?;
        write_batch_report(
            report_path.as_deref(),
            &upload_report,
            args.report_failures_only,
            false,
        )
        .await?;
        let failed = upload_report.failed;
        output_envelope(
            global,
            &Envelope::success(
                "ve-adrive rm",
                json!({
                    "operation": "recursive-delete",
                    "recursive_delete_mode": recursive_delete_mode_name(args.recursive_delete_mode),
                    "target": format_target(&target),
                    "summary": batch_summary(&upload_report),
                    "report_path": report_path,
                    "manifest_path": Value::Null,
                    "request_id": response.get("request_id").cloned().unwrap_or(Value::Null),
                    "status": if failed == 0 { "succeeded" } else { "partial_failure" },
                }),
            ),
        )?;
        return if failed == 0 { Ok(0) } else { Ok(1) };
    }
    let mut response = response;
    if upload_report.total > 0 {
        if let Some(map) = response.as_object_mut() {
            map.insert("uploads_summary".to_string(), batch_summary(&upload_report));
            if upload_failed > 0 {
                map.insert("status".to_string(), json!("partial_failure"));
            }
        }
    }
    output_envelope(global, &Envelope::success("ve-adrive rm", response))?;
    if upload_failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

async fn execute_rm_bottom_up(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &RmArgs,
    target: &ParsedADriveUri,
    batch_concurrency: usize,
    list_concurrency: usize,
    list_echo_enabled: bool,
    progress_enabled: bool,
) -> Result<i32, CliError> {
    if args.no_manifest {
        return execute_rm_bottom_up_streaming_no_manifest(
            global,
            client,
            args,
            target,
            batch_concurrency,
            list_concurrency,
            progress_enabled,
        )
        .await;
    }
    let mut scan_progress =
        PlanScanProgress::new(list_echo_enabled, "ve-adrive rm", &format_target(target));
    let mut entries = list_adrive_delete_entries(client, target, list_concurrency).await?;
    push_recursive_target_folder_entry(&mut entries, target);
    sort_delete_entries_bottom_up(&mut entries);
    scan_progress.finish_with_count(entries.len() as u64, "item(s)");
    let manifest_entries = adrive_delete_entries_for_rm(
        entries.clone(),
        args.include.as_deref(),
        args.exclude.as_deref(),
    );
    let report_path = effective_report_path(global, args.report_path.as_deref(), "ve-adrive rm")?;
    let manifest_path = effective_optional_manifest_path(
        global,
        args.manifest_path.as_deref(),
        args.no_manifest,
        "ve-adrive rm",
    )?;
    let progress = batch_progress("ve-adrive rm", entries.len() as u64, progress_enabled);
    let mut report = BatchReport::new("ve-adrive rm");
    report.set_manifest(adrive_delete_manifest_items(target, &manifest_entries));
    delete_adrive_entries_by_depth(
        client,
        target,
        entries,
        args.include.as_deref(),
        args.exclude.as_deref(),
        batch_concurrency,
        &progress,
        &mut report,
    )
    .await;
    finish_progress(progress);
    if args.include_uploads {
        abort_checkpointed_multipart_uploads_for_rm(
            global,
            client,
            target,
            args.checkpoint_dir.as_deref(),
            progress_enabled,
            &mut report,
        )
        .await?;
    }
    write_batch_report(
        report_path.as_deref(),
        &report,
        args.report_failures_only,
        manifest_path.is_some(),
    )
    .await?;
    write_adrive_manifest_file(
        manifest_path.as_deref(),
        "ve-adrive rm",
        report.manifest.as_ref(),
    )
    .await?;
    let failed = report.failed;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive rm",
            json!({
                "operation": "recursive-delete",
                "recursive_delete_mode": recursive_delete_mode_name(args.recursive_delete_mode),
                "target": format_target(target),
                "summary": batch_summary(&report),
                "report_path": report_path,
                "manifest_path": manifest_path,
                "status": if failed == 0 { "succeeded" } else { "partial_failure" },
            }),
        ),
    )?;
    // [Review Fix #BatchExitCode] 批量 summary 已经输出，失败项用
    // exit code=1 表达，避免顶层错误处理再打印一份 JSON。
    if failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

async fn execute_rm_bottom_up_streaming_no_manifest(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &RmArgs,
    target: &ParsedADriveUri,
    batch_concurrency: usize,
    list_concurrency: usize,
    progress_enabled: bool,
) -> Result<i32, CliError> {
    let report_path = effective_report_path(global, args.report_path.as_deref(), "ve-adrive rm")?;
    let progress = streaming_batch_progress(progress_enabled, "ve-adrive rm");
    let mut report = BatchReport::new("ve-adrive rm");
    let stream_result = stream_delete_adrive_bottom_up(
        client,
        target,
        args.include.as_deref(),
        args.exclude.as_deref(),
        batch_concurrency,
        list_concurrency,
        &progress,
        &mut report,
    )
    .await;
    finish_streaming_progress(progress, report.total as u64);
    if stream_result.is_ok() && args.include_uploads {
        abort_checkpointed_multipart_uploads_for_rm(
            global,
            client,
            target,
            args.checkpoint_dir.as_deref(),
            progress_enabled,
            &mut report,
        )
        .await?;
    }
    write_batch_report(
        report_path.as_deref(),
        &report,
        args.report_failures_only,
        false,
    )
    .await?;
    let failed = report.failed;
    stream_result?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive rm",
            json!({
                "operation": "recursive-delete",
                "recursive_delete_mode": recursive_delete_mode_name(args.recursive_delete_mode),
                "target": format_target(target),
                "summary": batch_summary(&report),
                "report_path": report_path,
                "manifest_path": Value::Null,
                "status": if failed == 0 { "succeeded" } else { "partial_failure" },
            }),
        ),
    )?;
    // [Review Fix #BatchExitCode] 批量 summary 已经输出，失败项用
    // exit code=1 表达，避免顶层错误处理再打印一份 JSON。
    if failed == 0 {
        Ok(0)
    } else {
        Ok(1)
    }
}

async fn stream_delete_adrive_bottom_up(
    client: &IdsClient,
    target: &ParsedADriveUri,
    include: Option<&str>,
    exclude: Option<&str>,
    batch_concurrency: usize,
    list_concurrency: usize,
    progress: &Option<ProgressBar>,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let mut entries = list_adrive_delete_entries(client, target, list_concurrency).await?;
    push_recursive_target_folder_entry(&mut entries, target);
    sort_delete_entries_bottom_up(&mut entries);
    for _ in &entries {
        if let Some(progress) = progress {
            progress.inc_length(1);
        }
    }
    delete_adrive_entries_by_depth(
        client,
        target,
        entries,
        include,
        exclude,
        batch_concurrency,
        progress,
        report,
    )
    .await;
    Ok(())
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum ADriveDeleteEntry {
    File(String),
    Folder(String),
}

impl ADriveDeleteEntry {
    fn path(&self) -> &str {
        match self {
            ADriveDeleteEntry::File(path) | ADriveDeleteEntry::Folder(path) => path,
        }
    }

    fn operation(&self) -> &'static str {
        match self {
            ADriveDeleteEntry::File(_) => "delete_file",
            ADriveDeleteEntry::Folder(_) => "delete_folder",
        }
    }

    fn is_file(&self) -> bool {
        matches!(self, ADriveDeleteEntry::File(_))
    }
}

async fn list_adrive_delete_entries(
    client: &IdsClient,
    target: &ParsedADriveUri,
    list_concurrency: usize,
) -> Result<Vec<ADriveDeleteEntry>, CliError> {
    let mut entries = Vec::new();
    let listed = list_all_files_and_folders_hierarchical(
        client,
        &target.instance,
        &target.space,
        &target.path,
        list_concurrency,
    )
    .await?;
    entries.extend(listed.files.into_iter().map(adrive_delete_entry_for_file));
    entries.extend(
        listed
            .folders
            .into_iter()
            .map(|folder| ADriveDeleteEntry::Folder(normalize_adrive_folder_path(&folder.folder))),
    );
    Ok(entries)
}

fn reject_direct_adrive_rm_filters(args: &RmArgs) -> Result<(), CliError> {
    if args.include.is_some() || args.exclude.is_some() {
        return Err(CliError::ValidationError(
            "ve-adrive rm --recursive-delete-mode direct does not support --include/--exclude"
                .to_string(),
        ));
    }
    Ok(())
}

fn adrive_delete_entries_for_rm(
    entries: Vec<ADriveDeleteEntry>,
    include: Option<&str>,
    exclude: Option<&str>,
) -> Vec<ADriveDeleteEntry> {
    entries
        .into_iter()
        .filter(|entry| adrive_delete_entry_matches_filters(entry, include, exclude))
        .collect()
}

fn adrive_delete_entry_matches_filters(
    entry: &ADriveDeleteEntry,
    include: Option<&str>,
    exclude: Option<&str>,
) -> bool {
    let path = entry.path();
    let folder_path = matches!(entry, ADriveDeleteEntry::Folder(_)).then(|| format!("{path}/"));
    if exclude.is_some_and(|pattern| {
        adrive_delete_entry_pattern_matches(path, folder_path.as_deref(), pattern)
    }) {
        return false;
    }
    // [Review Fix #4] ADrive normalizes folder paths without a trailing slash,
    // while TOS HNS directory markers are matched with one; check both forms so
    // recursive rm filter semantics stay aligned.
    include
        .map(|pattern| adrive_delete_entry_pattern_matches(path, folder_path.as_deref(), pattern))
        .unwrap_or(true)
}

fn adrive_delete_entry_pattern_matches(
    path: &str,
    folder_path: Option<&str>,
    pattern: &str,
) -> bool {
    simple_pattern_match(pattern, path)
        || folder_path.is_some_and(|path_with_slash| simple_pattern_match(pattern, path_with_slash))
}

fn push_unique_delete_entry(entries: &mut Vec<ADriveDeleteEntry>, entry: ADriveDeleteEntry) {
    if !entries.iter().any(|existing| existing == &entry) {
        entries.push(entry);
    }
}

fn push_recursive_target_folder_entry(
    entries: &mut Vec<ADriveDeleteEntry>,
    target: &ParsedADriveUri,
) {
    let folder_path = normalize_adrive_folder_path(&target.path);
    if !folder_path.is_empty() {
        push_unique_delete_entry(entries, ADriveDeleteEntry::Folder(folder_path));
    }
}

fn normalize_adrive_folder_path(path: &str) -> String {
    // [Review Fix #2] ListFiles and URI parsing may disagree on a trailing
    // slash; normalize before de-duplication and before DeleteFolder calls.
    path.trim_end_matches('/').to_string()
}

fn adrive_file_info_is_folder_marker(file: &FileInfo) -> bool {
    // [Review Fix #ADrive-RmFolderMarker] IDS can return folder markers in
    // the `files` array; delete planning must route those to DeleteFolder.
    file.file_path.ends_with('/') || file.file_type.eq_ignore_ascii_case("folder")
}

fn adrive_folder_info_from_file_marker(file: &FileInfo) -> Option<FolderInfo> {
    let folder = normalize_adrive_folder_path(&file.file_path);
    if folder.is_empty() {
        return None;
    }
    Some(FolderInfo {
        folder: file.file_path.clone(),
        updated_at: file.updated_at,
    })
}

fn adrive_delete_entry_for_file(file: FileInfo) -> ADriveDeleteEntry {
    if adrive_file_info_is_folder_marker(&file) {
        ADriveDeleteEntry::Folder(normalize_adrive_folder_path(&file.file_path))
    } else {
        ADriveDeleteEntry::File(file.file_path)
    }
}

fn sort_delete_entries_bottom_up(entries: &mut Vec<ADriveDeleteEntry>) {
    entries.sort_by(|left, right| {
        adrive_path_depth(right.path())
            .cmp(&adrive_path_depth(left.path()))
            .then_with(|| right.path().len().cmp(&left.path().len()))
            .then_with(|| right.is_file().cmp(&left.is_file()))
            .then_with(|| left.path().cmp(right.path()))
    });
    entries.dedup();
}

async fn delete_adrive_entries_by_depth(
    client: &IdsClient,
    target: &ParsedADriveUri,
    entries: Vec<ADriveDeleteEntry>,
    include: Option<&str>,
    exclude: Option<&str>,
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report: &mut BatchReport,
) {
    let mut delete_entries = Vec::new();
    for entry in entries {
        if adrive_delete_entry_matches_filters(&entry, include, exclude) {
            delete_entries.push(entry);
            continue;
        }
        report.push_skipped(
            format_adrive_entry_target(target, entry.path()),
            None,
            entry.operation(),
        );
        tick_progress(progress);
    }
    let (files, mut folders): (Vec<_>, Vec<_>) = delete_entries
        .into_iter()
        .partition(ADriveDeleteEntry::is_file);
    delete_adrive_entry_group(client, target, &files, batch_concurrency, progress, report).await;
    sort_delete_entries_bottom_up(&mut folders);
    let mut index = 0;
    while index < folders.len() {
        let depth = adrive_path_depth(folders[index].path());
        let mut end = index + 1;
        while end < folders.len() && adrive_path_depth(folders[end].path()) == depth {
            end += 1;
        }
        delete_adrive_entry_group(
            client,
            target,
            &folders[index..end],
            batch_concurrency,
            progress,
            report,
        )
        .await;
        index = end;
    }
}

async fn delete_adrive_entry_group(
    client: &IdsClient,
    target: &ParsedADriveUri,
    entries: &[ADriveDeleteEntry],
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report: &mut BatchReport,
) {
    let limit = batch_concurrency.max(1);
    let mut pending = entries.iter().cloned();
    let mut in_flight = FuturesUnordered::new();
    loop {
        while in_flight.len() < limit {
            let Some(entry) = pending.next() else {
                break;
            };
            let client = client.clone();
            let instance = target.instance.clone();
            let space = target.space.clone();
            let source = format_adrive_entry_target(target, entry.path());
            let operation = entry.operation().to_string();
            in_flight.push(async move {
                let result = match &entry {
                    ADriveDeleteEntry::File(path) => client
                        .delete_file(&DeleteFileInput::new(&instance, &space, path))
                        .await
                        .map(|_| ()),
                    ADriveDeleteEntry::Folder(path) => client
                        .delete_folder(&DeleteFolderInput::new(&instance, &space, path))
                        .await
                        .map(|_| ()),
                };
                (source, operation, result)
            });
        }
        let Some((source, operation, result)) = in_flight.next().await else {
            break;
        };
        match result {
            Ok(()) => report.push_success(source, None, &operation),
            Err(err) => report.push_failure(source, None, &operation, map_ids_error(err)),
        }
        tick_progress(progress);
    }
}

fn adrive_delete_manifest_items(
    target: &ParsedADriveUri,
    entries: &[ADriveDeleteEntry],
) -> Vec<BatchManifestItem> {
    entries
        .iter()
        .map(|entry| BatchManifestItem {
            operation: entry.operation().to_string(),
            source: format_adrive_entry_target(target, entry.path()),
            destination: None,
            size: 0,
            etag: None,
            crc64: None,
        })
        .collect()
}

fn adrive_path_depth(path: &str) -> usize {
    path.trim_end_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

fn format_adrive_entry_target(target: &ParsedADriveUri, path: &str) -> String {
    format!("adrive://{}/{}/{}", target.instance, target.space, path)
}

fn recursive_delete_mode_name(mode: RecursiveDeleteMode) -> &'static str {
    match mode {
        RecursiveDeleteMode::BottomUp => "bottom-up",
        RecursiveDeleteMode::Direct => "direct",
    }
}

struct BoundedInstances {
    instances: Vec<InstanceInfo>,
    next_marker: Option<String>,
    is_truncated: bool,
    request_id: String,
}

struct BoundedSpaces {
    spaces: Vec<SpaceInfo>,
    next_marker: Option<String>,
    is_truncated: bool,
    request_id: String,
}

struct BoundedFiles {
    files: Vec<FileInfo>,
    folders: Vec<FolderInfo>,
    next_marker: Option<String>,
    is_truncated: bool,
    request_id: String,
}

async fn list_instances_bounded(
    client: &IdsClient,
    max_keys: i32,
    initial_marker: Option<&str>,
) -> Result<BoundedInstances, CliError> {
    let mut instances = Vec::new();
    let mut marker = initial_marker.map(ToString::to_string);
    let mut next_marker = None;
    let mut request_id = String::new();

    while (instances.len() as i32) < max_keys {
        let page_limit = bounded_list_page_limit(instances.len() as i32, max_keys);
        let mut input = ListInstancesInput::new().with_limit(page_limit);
        input.marker = marker.take();
        let out = client.list_instances(&input).await.map_err(map_ids_error)?;
        request_id = out.response_info.request_id().to_string();
        // [Review Fix #11] Enforce the public max-keys cap even if the service
        // ever returns more entries than requested.
        for instance in out.instances {
            if (instances.len() as i32) >= max_keys {
                break;
            }
            instances.push(instance);
        }
        if !out.is_truncated || out.next_marker.is_empty() {
            next_marker = None;
            break;
        }
        next_marker = Some(out.next_marker);
        if (instances.len() as i32) >= max_keys {
            break;
        }
        marker = next_marker.clone();
    }

    Ok(BoundedInstances {
        instances,
        is_truncated: next_marker.is_some(),
        next_marker,
        request_id,
    })
}

async fn list_spaces_bounded(
    client: &IdsClient,
    instance: &str,
    max_keys: i32,
    initial_marker: Option<&str>,
) -> Result<BoundedSpaces, CliError> {
    let mut spaces = Vec::new();
    let mut marker = initial_marker.map(ToString::to_string);
    let mut next_marker = None;
    let mut request_id = String::new();

    while (spaces.len() as i32) < max_keys {
        let page_limit = bounded_list_page_limit(spaces.len() as i32, max_keys);
        let mut input = ListSpacesInput::new(instance).with_limit(page_limit);
        input.marker = marker.take();
        let out = client.list_spaces(&input).await.map_err(map_ids_error)?;
        request_id = out.response_info.request_id().to_string();
        for space in out.spaces {
            if (spaces.len() as i32) >= max_keys {
                break;
            }
            spaces.push(space);
        }
        if !out.is_truncated || out.next_marker.is_empty() {
            next_marker = None;
            break;
        }
        next_marker = Some(out.next_marker);
        if (spaces.len() as i32) >= max_keys {
            break;
        }
        marker = next_marker.clone();
    }

    Ok(BoundedSpaces {
        spaces,
        is_truncated: next_marker.is_some(),
        next_marker,
        request_id,
    })
}

async fn list_files_bounded(
    client: &IdsClient,
    instance: &str,
    space: &str,
    path: &str,
    max_keys: i32,
    initial_marker: Option<&str>,
) -> Result<BoundedFiles, CliError> {
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let prefix = trim_folder_prefix(path);
    let mut marker = initial_marker.map(ToString::to_string);
    let mut next_marker = None;
    let mut request_id = String::new();
    let mut returned = 0i32;

    while returned < max_keys {
        // [Review Fix #9] `ve-adrive ls --max-keys` is a result cap, while the
        // service page is capped at 1000. Keep listing the current delimiter
        // level until the requested cap or the service page chain ends.
        let page_limit = bounded_list_page_limit(returned, max_keys);
        let mut input = ListFilesInput::new(instance, space)
            .with_limit(page_limit)
            .with_delimiter("/");
        if !prefix.is_empty() {
            input = input.with_prefix(&prefix);
        }
        if let Some(value) = marker.take() {
            input = input.with_marker(value);
        }
        let out = client.list_files(&input).await.map_err(map_ids_error)?;
        request_id = out.response_info.request_id().to_string();
        for folder in out.folders {
            if returned >= max_keys {
                break;
            }
            folders.push(folder);
            returned += 1;
        }
        for file in out.files {
            if returned >= max_keys {
                break;
            }
            files.push(file);
            returned += 1;
        }
        if !out.is_truncated || out.next_marker.is_empty() {
            next_marker = None;
            break;
        }
        next_marker = Some(out.next_marker);
        if returned >= max_keys {
            break;
        }
        marker = next_marker.clone();
    }

    Ok(BoundedFiles {
        files,
        folders,
        is_truncated: next_marker.is_some(),
        next_marker,
        request_id,
    })
}

fn normalize_ls_files_and_folders(
    files: Vec<FileInfo>,
    folders: Vec<FolderInfo>,
    root_path: &str,
) -> (Vec<FileInfo>, Vec<FolderInfo>) {
    let current_folder_marker = (!root_path.is_empty())
        .then(|| normalize_adrive_folder_path(&trim_folder_prefix(root_path)));
    let mut folder_keys = folders
        .iter()
        .map(|folder| normalize_adrive_folder_path(&folder.folder))
        .collect::<HashSet<_>>();
    let mut normalized_files = Vec::new();
    let mut normalized_folders = folders;

    for file in files {
        let is_folder_marker =
            file.file_path.ends_with('/') || file.file_type.eq_ignore_ascii_case("folder");
        if !is_folder_marker {
            normalized_files.push(file);
            continue;
        }

        let folder_key = normalize_adrive_folder_path(&file.file_path);
        if folder_key.is_empty() {
            // [Review Fix #3] Ignore malformed root folder markers so root `ls`
            // cannot render an empty folder row.
            continue;
        }
        if Some(folder_key.as_str()) == current_folder_marker.as_deref() {
            // [Review Fix #1] Listing adrive://inst/space/folder/ should show children,
            // not the current folder marker as a file row.
            continue;
        }
        if folder_keys.insert(folder_key) {
            // [Review Fix #2] High-level ls treats trailing-slash file markers as folders.
            normalized_folders.push(FolderInfo {
                folder: file.file_path,
                updated_at: file.updated_at,
            });
        }
    }

    (normalized_files, normalized_folders)
}

fn dedupe_adrive_du_page_entries(
    files: Vec<FileInfo>,
    folders: Vec<FolderInfo>,
    current_prefix: &str,
) -> (Vec<FileInfo>, Vec<String>) {
    let current_folder = normalize_adrive_folder_path(current_prefix);
    let current_folder = (!current_folder.is_empty()).then_some(current_folder);
    let mut folder_keys = HashSet::new();
    let mut folder_prefixes = Vec::new();
    for folder in folders {
        push_adrive_du_folder_prefix(
            &folder.folder,
            current_folder.as_deref(),
            &mut folder_keys,
            &mut folder_prefixes,
        );
    }

    let files = files
        .into_iter()
        .filter_map(|file| {
            if adrive_file_info_is_folder_marker(&file) {
                push_adrive_du_folder_prefix(
                    &file.file_path,
                    current_folder.as_deref(),
                    &mut folder_keys,
                    &mut folder_prefixes,
                );
                None
            } else {
                Some(file)
            }
        })
        .collect();

    (files, folder_prefixes)
}

fn push_adrive_du_folder_prefix(
    path: &str,
    current_folder: Option<&str>,
    folder_keys: &mut HashSet<String>,
    folder_prefixes: &mut Vec<String>,
) {
    let folder_key = normalize_adrive_folder_path(path);
    if folder_key.is_empty() || current_folder == Some(folder_key.as_str()) {
        return;
    }
    // [Review Fix #9] `du` uses the same high-level classification as `ls`:
    // folder marker files are directories, not extra file rows.
    if folder_keys.insert(folder_key.clone()) {
        folder_prefixes.push(trim_folder_prefix(&folder_key));
    }
}

async fn execute_ls(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &LsArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_ls_args_by_name(client, args).await?;
    let args = &resolved_args;
    validate_adrive_ls_max_keys(args.max_keys)?;
    let manifest_path = effective_explicit_manifest_path(args.manifest_path.as_deref());
    let target = resolve_hierarchical_target(
        args.path.as_deref(),
        args.instance.as_deref(),
        args.space.as_deref(),
        args.folder.as_deref(),
        None,
        true,
    )?;
    match target {
        ADriveTarget::Instances => {
            let listed =
                list_instances_bounded(client, args.max_keys, args.marker.as_deref()).await?;
            let mut instances = Vec::new();
            let mut manifest_items = Vec::new();
            for instance in listed.instances {
                let source = format!("adrive://{}", instance.instance_id);
                manifest_items.push(BatchManifestItem {
                    operation: "list".to_string(),
                    source: source.clone(),
                    destination: None,
                    size: 0,
                    etag: None,
                    crc64: None,
                });
                instances.push(json!({
                    "instance_id": instance.instance_id,
                    "name": instance.name,
                    "display_name": instance.display_name,
                    "status": instance.status,
                    "run_state": instance.run_state,
                    "space_count": instance.space_count,
                    "created_at": instance.created_at,
                    "updated_at": instance.updated_at,
                }));
            }
            let manifest = BatchManifest::from_items(manifest_items);
            write_adrive_manifest_file(manifest_path.as_deref(), "ve-adrive ls", Some(&manifest))
                .await?;
            let total = instances.len() as u64;
            let next_marker = listed.next_marker.unwrap_or_default();
            // [Review Fix #3] Instance listing must honor --columns just like file listing.
            output_result_with_columns(
                global,
                &Envelope::success(
                    "ve-adrive ls",
                    json!({
                        "scope": "instances",
                        "instances": instances,
                        "next_marker": next_marker,
                        "is_truncated": listed.is_truncated,
                        "request_id": listed.request_id,
                        "manifest_path": manifest_path,
                    }),
                )
                .with_pagination(adrive_marker_pagination(&next_marker, total)),
                Some(parse_columns(args.columns.as_deref()).unwrap_or(INSTANCE_TABLE_COLUMNS)),
            )?;
        }
        ADriveTarget::Instance { instance } => {
            let listed =
                list_spaces_bounded(client, &instance, args.max_keys, args.marker.as_deref())
                    .await?;
            let mut spaces = Vec::new();
            let mut manifest_items = Vec::new();
            for space in listed.spaces {
                let source = format!("adrive://{}/{}", instance, space.space_id);
                manifest_items.push(BatchManifestItem {
                    operation: "list".to_string(),
                    source: source.clone(),
                    destination: None,
                    size: 0,
                    etag: None,
                    crc64: None,
                });
                spaces.push(json!({
                    "space_id": space.space_id,
                    "name": space.name,
                    "display_name": space.display_name,
                    "owner_type": space.owner_type,
                    "owner_id": space.owner_id,
                    "created_at": space.created_at,
                    "updated_at": space.updated_at,
                }));
            }
            let manifest = BatchManifest::from_items(manifest_items);
            write_adrive_manifest_file(manifest_path.as_deref(), "ve-adrive ls", Some(&manifest))
                .await?;
            let total = spaces.len() as u64;
            let next_marker = listed.next_marker.unwrap_or_default();
            // [Review Fix #3] Space listing must honor --columns just like file listing.
            output_result_with_columns(
                global,
                &Envelope::success(
                    "ve-adrive ls",
                    json!({
                        "scope": "spaces",
                        "instance": instance,
                        "spaces": spaces,
                        "next_marker": next_marker,
                        "is_truncated": listed.is_truncated,
                        "request_id": listed.request_id,
                        "manifest_path": manifest_path,
                    }),
                )
                .with_pagination(adrive_marker_pagination(&next_marker, total)),
                Some(parse_columns(args.columns.as_deref()).unwrap_or(SPACE_TABLE_COLUMNS)),
            )?;
        }
        ADriveTarget::Space { instance, space } => {
            list_files(
                global,
                client,
                &instance,
                &space,
                "",
                args.max_keys,
                args.marker.as_deref(),
                args.columns.as_deref(),
                manifest_path.as_deref(),
                args.human_readable,
            )
            .await?;
        }
        ADriveTarget::Path(target) => {
            list_files(
                global,
                client,
                &target.instance,
                &target.space,
                &target.path,
                args.max_keys,
                args.marker.as_deref(),
                args.columns.as_deref(),
                manifest_path.as_deref(),
                args.human_readable,
            )
            .await?;
        }
    }
    Ok(0)
}

async fn execute_stat(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &StatArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_stat_args_by_name(client, args).await?;
    let args = &resolved_args;
    let target = resolve_hierarchical_target(
        args.path.as_deref(),
        args.instance.as_deref(),
        args.space.as_deref(),
        args.folder.as_deref(),
        args.file.as_deref(),
        false,
    )?;
    let payload = match target {
        ADriveTarget::Instances => {
            return Err(CliError::ValidationError(
                "ve-adrive stat requires an instance, space, file, or folder target".to_string(),
            ));
        }
        ADriveTarget::Instance { instance } => {
            let instance_info = get_instance_info(client, &instance).await?;
            json!({
                "resource_type": "instance",
                "target": format!("adrive://{}", instance_info.instance_id),
                "instance_id": instance_info.instance_id,
                "name": instance_info.name,
                "display_name": instance_info.display_name,
                "description": instance_info.description,
                "status": instance_info.status,
                "run_state": instance_info.run_state,
                "space_count": instance_info.space_count,
                "created_at": instance_info.created_at,
                "updated_at": instance_info.updated_at,
            })
        }
        ADriveTarget::Space { instance, space } => {
            let instance_info = get_instance_info(client, &instance).await?;
            let space_info = get_space_info(client, &instance_info.instance_id, &space).await?;
            json!({
                "resource_type": "space",
                "target": format!("adrive://{}/{}", space_info.instance_id, space_info.space_id),
                "instance_id": space_info.instance_id,
                "space_id": space_info.space_id,
                "name": space_info.name,
                "display_name": space_info.display_name,
                "description": space_info.description,
                "owner_type": space_info.owner_type,
                "owner_id": space_info.owner_id,
                "created_at": space_info.created_at,
                "updated_at": space_info.updated_at,
            })
        }
        ADriveTarget::Path(target) => {
            let out = client
                .head_file(&HeadFileInput::new(
                    &target.instance,
                    &target.space,
                    &target.path,
                ))
                .await
                .map_err(map_ids_error)?;
            json!({
                "resource_type": if out.is_folder { "folder" } else { "file" },
                "target": format_target(&target),
                "size": out.content_length,
                "content_type": out.content_type,
                "etag": out.etag,
                "crc64": out.hash_crc64_ecma,
                "created_at": out.created_at,
                "updated_at": out.updated_at,
                "file_type": out.file_type,
                "storage_class": out.storage_class,
                "is_folder": out.is_folder,
                "meta": out.meta,
                "request_id": out.response_info.request_id(),
            })
        }
    };
    output_envelope(global, &Envelope::success("ve-adrive stat", payload))?;
    Ok(0)
}

async fn execute_du(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &DuArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_du_args_by_name(client, args).await?;
    let args = &resolved_args;
    validate_du_top_k(args.top_k)?;
    let target = resolve_target(
        args.path.as_deref(),
        args.instance.as_deref(),
        args.space.as_deref(),
        args.folder.as_deref(),
        None,
    )?;
    let price_table = storage_price_table(&args.storage_price)?;
    let manifest_path = effective_explicit_manifest_path(args.manifest_path.as_deref());
    let list_concurrency = effective_list_concurrency(global, args.list_concurrency)?;
    let progress = traversal_progress(
        "ve-adrive du",
        &format_target(&target),
        effective_traversal_echo_enabled(
            global,
            args.list_echo,
            args.no_list_echo,
            args.progress,
            args.no_progress,
        ),
    );
    let profile = collect_adrive_du_profile(
        client,
        &target.instance,
        &target.space,
        &target.path,
        list_concurrency,
        args,
    )
    .await?;
    finish_progress(progress);
    let total_items = profile.file_count + profile.folder_count;
    let manifest = BatchManifest::from_items(profile.manifest_items.clone());
    write_adrive_manifest_file(manifest_path.as_deref(), "ve-adrive du", Some(&manifest)).await?;
    let cost = if args.cost {
        Some(profile.cost_estimate(&price_table))
    } else {
        None
    };
    let payload = adrive_du_output_payload(
        global,
        &target,
        &profile,
        args,
        cost,
        manifest_path.as_deref(),
        list_concurrency,
    );
    output_result_with_columns(
        global,
        &Envelope::success("ve-adrive du", payload)
            .with_pagination(adrive_marker_pagination("", total_items)),
        None,
    )?;
    Ok(0)
}

fn adrive_du_output_payload(
    global: &GlobalArgs,
    target: &ParsedADriveUri,
    profile: &DuAccumulator,
    args: &DuArgs,
    cost: Option<Value>,
    manifest_path: Option<&str>,
    list_concurrency: usize,
) -> Value {
    let mut payload = json!({
        "target": format_target(target),
        "files": profile.file_count,
        "folders": profile.folder_count,
        "total_size": profile.total_bytes,
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
    if args.max_depth.is_some() {
        map.insert(
            "groups".to_string(),
            json!(profile.directory_distribution_json()),
        );
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
        map.insert(
            "diagnostics".to_string(),
            adrive_du_diagnostics(profile, list_concurrency),
        );
    }
    payload
}

fn adrive_du_diagnostics(profile: &DuAccumulator, list_concurrency: usize) -> Value {
    json!({
        "file_types": profile.file_type_distribution_json(),
        "directories": profile.directory_distribution_json(),
        "size_histogram": profile.size_histogram_json(),
        "storage_classes": profile.storage_class_distribution_json(),
        "largest_files": &profile.largest_files,
        "oldest_files": &profile.oldest_files,
        "traversal": {
            "streaming": true,
            "page_size": 1000,
            "prefix_concurrency": list_concurrency.max(1),
            "delimiter": "/",
            "memory_model": "O(category_count + top_k + manifest_items)",
        },
    })
}

async fn collect_adrive_du_profile(
    client: &IdsClient,
    instance: &str,
    space: &str,
    path: &str,
    list_concurrency: usize,
    args: &DuArgs,
) -> Result<DuAccumulator, CliError> {
    let mut accumulator = DuAccumulator::new(args.top_k);
    let mut pending_prefixes = vec![trim_folder_prefix(path)];
    let mut seen_prefixes = HashSet::new();
    let mut in_flight = FuturesUnordered::new();

    while !pending_prefixes.is_empty() || !in_flight.is_empty() {
        while in_flight.len() < list_concurrency.max(1) {
            let Some(prefix) = pending_prefixes.pop() else {
                break;
            };
            if !seen_prefixes.insert(prefix.clone()) {
                continue;
            }
            in_flight.push(scan_adrive_du_prefix(
                client, instance, space, path, prefix, args,
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

struct ADriveDuPrefixScan {
    accumulator: DuAccumulator,
    child_prefixes: Vec<String>,
}

async fn scan_adrive_du_prefix(
    client: &IdsClient,
    instance: &str,
    space: &str,
    root_path: &str,
    prefix: String,
    args: &DuArgs,
) -> Result<ADriveDuPrefixScan, CliError> {
    let mut accumulator = DuAccumulator::new(args.top_k);
    let mut child_prefixes = Vec::new();
    let mut marker = None;
    loop {
        let mut input = ListFilesInput::new(instance, space)
            .with_limit(1000)
            .with_delimiter("/");
        if !prefix.is_empty() {
            input = input.with_prefix(&prefix);
        }
        if let Some(value) = marker.take() {
            input = input.with_marker(value);
        }
        let out = client.list_files(&input).await.map_err(map_ids_error)?;
        let (files, folder_prefixes) =
            dedupe_adrive_du_page_entries(out.files, out.folders, &prefix);
        for file in &files {
            accumulator.record_adrive_file(file, instance, space, root_path, args.max_depth);
        }
        for folder_prefix in folder_prefixes {
            accumulator.record_folder_prefix(&folder_prefix);
            child_prefixes.push(folder_prefix);
        }
        if !out.is_truncated || out.next_marker.is_empty() {
            break;
        }
        marker = Some(out.next_marker);
    }
    Ok(ADriveDuPrefixScan {
        accumulator,
        child_prefixes,
    })
}

impl DuAccumulator {
    fn new(top_k: usize) -> Self {
        let mut size_histogram = BTreeMap::new();
        for name in ["0-1K", "1K-1M", "1M-100M", ">100M"] {
            size_histogram.insert(name, DuDistributionBucket { count: 0, bytes: 0 });
        }
        Self {
            file_count: 0,
            folder_count: 0,
            folder_prefixes: HashSet::new(),
            total_bytes: 0,
            manifest_items: Vec::new(),
            file_types: BTreeMap::new(),
            directories: BTreeMap::new(),
            size_histogram,
            storage_classes: BTreeMap::new(),
            largest_files: Vec::new(),
            oldest_files: Vec::new(),
            top_k,
        }
    }

    fn record_adrive_file(
        &mut self,
        file: &FileInfo,
        instance: &str,
        space: &str,
        prefix: &str,
        max_depth: Option<u32>,
    ) {
        let size = file.size.max(0) as u64;
        self.file_count += 1;
        self.total_bytes += size;
        self.manifest_items.push(adrive_remote_file_manifest_item(
            "du",
            file,
            format!("adrive://{}/{}/{}", instance, space, file.file_path),
            None,
        ));
        increment_du_bucket(
            &mut self.file_types,
            file_extension_bucket(&file.file_path),
            size,
        );
        increment_du_bucket(
            &mut self.directories,
            directory_group(&file.file_path, prefix, max_depth),
            size,
        );
        increment_du_bucket(&mut self.size_histogram, size_histogram_bucket(size), size);
        let storage_class = if file.storage_class.is_empty() {
            "STANDARD".to_string()
        } else {
            file.storage_class.clone()
        }
        .to_ascii_uppercase();
        increment_du_bucket(&mut self.storage_classes, storage_class.clone(), size);
        let sample = DuFileSample {
            file_path: file.file_path.clone(),
            size,
            updated_at: (file.updated_at > 0).then_some(file.updated_at),
            storage_class: Some(storage_class),
        };
        self.record_sample(sample);
    }

    fn record_folder_prefix(&mut self, prefix: impl AsRef<str>) -> bool {
        let folder_prefix = normalize_adrive_folder_path(prefix.as_ref());
        if folder_prefix.is_empty() {
            return false;
        }
        if self.folder_prefixes.insert(folder_prefix) {
            // [Review Fix #8] IDS can surface the same folder from multiple
            // pages or scan levels, so `du` counts unique normalized folders.
            self.folder_count += 1;
            true
        } else {
            false
        }
    }

    fn merge(&mut self, other: DuAccumulator) {
        self.file_count += other.file_count;
        for folder_prefix in other.folder_prefixes {
            self.record_folder_prefix(folder_prefix);
        }
        self.total_bytes += other.total_bytes;
        self.manifest_items.extend(other.manifest_items);
        merge_du_bucket_map(&mut self.file_types, other.file_types);
        merge_du_bucket_map(&mut self.directories, other.directories);
        merge_du_bucket_map(&mut self.size_histogram, other.size_histogram);
        merge_du_bucket_map(&mut self.storage_classes, other.storage_classes);
        for sample in other.largest_files {
            self.record_largest_sample(sample);
        }
        for sample in other.oldest_files {
            self.record_oldest_sample(sample);
        }
    }

    fn record_sample(&mut self, sample: DuFileSample) {
        self.record_largest_sample(sample.clone());
        if sample.updated_at.is_some() {
            self.record_oldest_sample(sample);
        }
    }

    fn record_largest_sample(&mut self, sample: DuFileSample) {
        if self.top_k == 0 {
            return;
        }
        self.largest_files.push(sample);
        self.largest_files.sort_by(|left, right| {
            right
                .size
                .cmp(&left.size)
                .then_with(|| left.file_path.cmp(&right.file_path))
        });
        self.largest_files.truncate(self.top_k);
    }

    fn record_oldest_sample(&mut self, sample: DuFileSample) {
        if self.top_k == 0 {
            return;
        }
        self.oldest_files.push(sample);
        self.oldest_files.sort_by(|left, right| {
            left.updated_at
                .unwrap_or(i64::MAX)
                .cmp(&right.updated_at.unwrap_or(i64::MAX))
                .then_with(|| left.file_path.cmp(&right.file_path))
        });
        self.oldest_files.truncate(self.top_k);
    }

    fn file_type_distribution_json(&self) -> BTreeMap<String, Value> {
        du_bucket_map_json(&self.file_types, "file_count")
    }

    fn directory_distribution_json(&self) -> BTreeMap<String, Value> {
        du_bucket_map_json(&self.directories, "file_count")
    }

    fn size_histogram_json(&self) -> BTreeMap<String, Value> {
        du_bucket_map_json(&self.size_histogram, "file_count")
    }

    fn storage_class_distribution_json(&self) -> BTreeMap<String, Value> {
        du_bucket_map_json(&self.storage_classes, "file_count")
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
        let now = current_unix_millis();
        if self
            .storage_classes
            .get("STANDARD")
            .map(|bucket| bucket.bytes > 0)
            .unwrap_or(false)
            && self.oldest_files.iter().any(|sample| {
                sample
                    .updated_at
                    .map(|updated| now.saturating_sub(updated) > 90 * 24 * 60 * 60 * 1000)
                    .unwrap_or(false)
            })
        {
            suggestions.push(
                "STANDARD 中存在较旧文件样本，可结合访问日志评估转 IA/归档的生命周期规则。"
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

fn file_extension_bucket(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .and_then(|(_, ext)| (!ext.is_empty()).then(|| ext.to_ascii_lowercase()))
        .unwrap_or_else(|| "(none)".to_string())
}

fn directory_group(path: &str, prefix: &str, max_depth: Option<u32>) -> String {
    let prefix = trim_folder_prefix(prefix);
    let relative = path
        .strip_prefix(&prefix)
        .unwrap_or(path)
        .trim_start_matches('/');
    let depth = max_depth.unwrap_or(u32::MAX) as usize;
    if depth == 0 {
        return ".".to_string();
    }
    let group = relative
        .split('/')
        .filter(|segment| !segment.is_empty())
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

fn round_cost(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn current_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut value = bytes as f64;
    let mut unit_index = 0usize;
    while value >= 1024.0 && unit_index + 1 < UNITS.len() {
        value /= 1024.0;
        unit_index += 1;
    }
    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{value:.2} {}", UNITS[unit_index])
    }
}

fn uses_tabular_output(global: &GlobalArgs) -> bool {
    matches!(
        global.output.unwrap_or_else(OutputFormat::auto_detect),
        OutputFormat::Table | OutputFormat::Csv
    )
}

async fn get_instance_info(client: &IdsClient, instance: &str) -> Result<InstanceInfo, CliError> {
    let out = client
        .get_instance(&GetInstanceInput::new(instance))
        .await
        .map_err(map_ids_error)?;
    Ok(out.instance)
}

async fn get_instance_info_by_name(
    client: &IdsClient,
    name: &str,
) -> Result<InstanceInfo, CliError> {
    let out = client
        .get_instance_by_name(&GetInstanceByNameInput::new(name))
        .await
        .map_err(map_ids_error)?;
    Ok(out.instance)
}

async fn get_space_info(
    client: &IdsClient,
    instance: &str,
    space: &str,
) -> Result<SpaceInfo, CliError> {
    let out = client
        .get_space(&GetSpaceInput::new(instance, space))
        .await
        .map_err(map_ids_error)?;
    Ok(out.space)
}

async fn get_space_info_by_name(
    client: &IdsClient,
    instance_id: &str,
    space_name: &str,
) -> Result<SpaceInfo, CliError> {
    let out = client
        .get_space_by_name(&GetSpaceByNameInput::new_with_instance_id(
            instance_id,
            space_name,
        ))
        .await
        .map_err(map_ids_error)?;
    Ok(out.space)
}

async fn execute_find(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &FindArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_find_args_by_name(client, args).await?;
    let args = &resolved_args;
    let target = resolve_target(
        args.path.as_deref(),
        args.instance.as_deref(),
        args.space.as_deref(),
        args.folder.as_deref(),
        None,
    )?;
    let size_filter = args
        .size
        .as_deref()
        .map(parse_find_size_filter)
        .transpose()?;
    let mtime_filter = args
        .mtime
        .as_deref()
        .map(parse_find_mtime_filter_millis)
        .transpose()?;
    // [Review Fix #ADrive-FindList] Match TOS find: discover by hierarchical
    // list, then apply all user filters locally so --name/--size/--mtime share
    // one deterministic implementation path.
    let progress = traversal_progress(
        "ve-adrive find",
        &format_target(&target),
        effective_traversal_echo_enabled(
            global,
            args.list_echo,
            args.no_list_echo,
            args.progress,
            args.no_progress,
        ),
    );
    let files = list_all_files_hierarchical(
        client,
        &target.instance,
        &target.space,
        &target.path,
        DEFAULT_LIST_CONCURRENCY,
    )
    .await?;
    let matches = files
        .into_iter()
        .filter(|file| adrive_find_entry_matches(file, args, size_filter, mtime_filter))
        .collect::<Vec<_>>();
    finish_progress(progress);
    let manifest_path = effective_explicit_manifest_path(args.manifest_path.as_deref());
    let manifest = BatchManifest::from_items(
        matches
            .iter()
            .map(|file| {
                adrive_remote_file_manifest_item(
                    "find",
                    file,
                    format_adrive_entry_target(&target, &file.file_path),
                    None,
                )
            })
            .collect(),
    );
    write_adrive_manifest_file(manifest_path.as_deref(), "ve-adrive find", Some(&manifest)).await?;
    let total_results = matches.len() as u64;
    let match_rows = matches
        .iter()
        .map(adrive_find_match_row)
        .collect::<Vec<_>>();
    output_result_with_columns(
        global,
        &Envelope::success(
            "ve-adrive find",
            json!({
                "target": format_target(&target),
                "matches": match_rows,
                "manifest_path": manifest_path,
            }),
        )
        .with_pagination(adrive_marker_pagination("", total_results)),
        Some(FILE_TABLE_COLUMNS),
    )?;
    Ok(0)
}

async fn execute_cat(
    _global: &GlobalArgs,
    client: &IdsClient,
    args: &CatArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_cat_args_by_name(client, args).await?;
    let args = &resolved_args;
    let target = resolve_target(
        args.path.as_deref(),
        args.instance.as_deref(),
        args.space.as_deref(),
        args.folder.as_deref(),
        args.file.as_deref(),
    )?;
    let target = parse_file_target(target)?;
    let mut input = GetFileInput::new(&target.instance, &target.space, &target.path);
    if let Some(range) = &args.range {
        input = input.with_range_raw(normalize_ids_range(range));
    }
    let mut out = client.get_file(&input).await.map_err(map_ids_error)?;
    let mut stdout = tokio::io::stdout();
    if out.content_length == 0 {
        stdout.flush().await?;
        return Ok(0);
    }
    while let Some(chunk) = out.next().await {
        stdout.write_all(&chunk?).await?;
    }
    stdout.flush().await?;
    Ok(0)
}

async fn execute_put(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &PutArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_put_args_by_name(client, args).await?;
    let args = &resolved_args;
    let target = resolve_put_file_target(args)?;
    if args.no_clobber
        && head_file_optional(client, &target.instance, &target.space, &target.path)
            .await?
            .is_some()
    {
        return Err(CliError::Conflict(format!(
            "destination already exists: {}",
            format_target(&target)
        )));
    }

    let part_size =
        effective_stdin_multipart_threshold(global, args.multipart_threshold.as_deref())?;
    let progress_enabled = effective_progress_enabled(global, args.progress, args.no_progress)?;
    let mut stdin = tokio::io::stdin();
    let first_part = read_stream_part(&mut stdin, part_size).await?;
    if first_part.len() < part_size {
        put_adrive_single_stdin_file(global, client, args, &target, first_part).await?;
        return Ok(0);
    }
    put_adrive_multipart_stdin_file(
        global,
        client,
        args,
        &target,
        first_part,
        stdin,
        progress_enabled,
    )
    .await?;
    Ok(0)
}

async fn put_adrive_single_stdin_file(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &PutArgs,
    target: &ParsedADriveUri,
    body: Vec<u8>,
) -> Result<(), CliError> {
    let size = body.len() as u64;
    let local_crc64 = crc64_for_bytes(&body);
    let mut input = PutFileInput::new(
        target.instance.clone(),
        target.space.clone(),
        target.path.clone(),
        IdsBody::from_bytes(body),
    )
    .with_content_length(size);
    input.content_type = args.content_type.clone();

    let out = client.put_file(input).await.map_err(map_ids_error)?;
    verify_adrive_crc64_response(out.hash_crc64_ecma, local_crc64, "stdin upload")?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive put",
            json!({
                "operation": "stdin-upload",
                "destination": format_target(target),
                "size": out.size,
                "bytes": size,
                "etag": out.etag,
                "crc64": out.hash_crc64_ecma,
                "request_id": out.response_info.request_id(),
                "status": "succeeded",
            }),
        ),
    )?;
    Ok(())
}

async fn put_adrive_multipart_stdin_file(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &PutArgs,
    target: &ParsedADriveUri,
    first_part: Vec<u8>,
    mut stdin: tokio::io::Stdin,
    progress_enabled: bool,
) -> Result<(), CliError> {
    let initiated = client
        .initiate_multipart_upload(&InitiateMultipartUploadInput {
            instance_id: target.instance.clone(),
            space_id: target.space.clone(),
            file_path: target.path.clone(),
            content_type: args.content_type.clone(),
            meta: None,
        })
        .await
        .map_err(map_ids_error)?;

    let (parts, total_size, local_crc64) = match put_adrive_multipart_stdin_inner(
        client,
        target,
        &initiated.upload_id,
        first_part,
        &mut stdin,
        progress_enabled,
    )
    .await
    {
        Ok(result) => result,
        Err(err) => {
            // [Review Fix #4] Stdin multipart has no checkpoint state; abort
            // failed sessions immediately so uploaded parts do not linger.
            let _ = abort_adrive_multipart_upload(
                client,
                &target.instance,
                &target.space,
                &target.path,
                &initiated.upload_id,
            )
            .await;
            return Err(err);
        }
    };
    let complete = CompleteMultipartUploadInput {
        instance_id: target.instance.clone(),
        space_id: target.space.clone(),
        file_path: target.path.clone(),
        upload_id: initiated.upload_id.clone(),
        parts,
    };
    let out = match client.complete_multipart_upload(&complete).await {
        Ok(out) => out,
        Err(err) => {
            // [Review Fix #5] CompleteMultipartUpload can fail after all parts
            // are uploaded; abort that upload id before surfacing the error.
            let _ = abort_adrive_multipart_upload(
                client,
                &target.instance,
                &target.space,
                &target.path,
                &initiated.upload_id,
            )
            .await;
            return Err(map_ids_error(err));
        }
    };
    verify_adrive_crc64_response(out.hash_crc64_ecma, local_crc64, "stdin multipart upload")?;
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive put",
            json!({
                "operation": "stdin-multipart-upload",
                "destination": format_target(target),
                "size": out.size,
                "bytes": total_size,
                "parts": complete.parts.len(),
                "upload_id": initiated.upload_id,
                "etag": out.etag,
                "crc64": out.hash_crc64_ecma,
                "request_id": out.response_info.request_id(),
                "status": "succeeded",
            }),
        ),
    )?;
    Ok(())
}

async fn put_adrive_multipart_stdin_inner<R: AsyncRead + Unpin>(
    client: &IdsClient,
    target: &ParsedADriveUri,
    upload_id: &str,
    first_part: Vec<u8>,
    stdin: &mut R,
    progress_enabled: bool,
) -> Result<(Vec<PartInfo>, u64, u64), CliError> {
    let part_size = first_part.len();
    let mut next_part = Some(first_part);
    let mut parts = Vec::new();
    let mut full_crc = Digest::new();
    let mut total_size = 0_u64;
    let mut part_number = 1_i32;
    let progress = stdin_upload_progress("ve-adrive put", progress_enabled);

    loop {
        let part = if let Some(part) = next_part.take() {
            part
        } else {
            read_stream_part(stdin, part_size).await?
        };
        if part.is_empty() {
            break;
        }
        let part_length = part.len() as u64;
        let _ = full_crc.write(&part);
        total_size += part_length;
        let uploaded =
            upload_adrive_stdin_part(client, target, upload_id, part_number, part).await?;
        parts.push(uploaded);
        if let Some(progress) = &progress {
            progress.inc(part_length);
        }
        part_number = part_number.checked_add(1).ok_or_else(|| {
            CliError::ValidationError("stdin multipart upload has too many parts".to_string())
        })?;
    }
    if let Some(progress) = progress {
        progress.finish_and_clear();
    }
    Ok((parts, total_size, full_crc.sum64()))
}

async fn upload_adrive_stdin_part(
    client: &IdsClient,
    target: &ParsedADriveUri,
    upload_id: &str,
    part_number: i32,
    body: Vec<u8>,
) -> Result<PartInfo, CliError> {
    let local_crc64 = crc64_for_bytes(&body);
    let length = body.len() as u64;
    let input = UploadPartInput::new(
        target.instance.clone(),
        target.space.clone(),
        target.path.clone(),
        upload_id.to_string(),
        part_number,
        IdsBody::from_bytes(body),
    )
    .with_content_length(length);
    let out = client.upload_part(input).await.map_err(map_ids_error)?;
    verify_adrive_crc64_response(out.hash_crc64_ecma, local_crc64, "stdin upload part")?;
    Ok(PartInfo {
        part_number,
        etag: out.etag,
    })
}

async fn abort_adrive_multipart_upload(
    client: &IdsClient,
    instance: &str,
    space: &str,
    file_path: &str,
    upload_id: &str,
) -> Result<(), CliError> {
    client
        .abort_multipart_upload(&AbortMultipartUploadInput {
            instance_id: instance.to_string(),
            space_id: space.to_string(),
            file_path: file_path.to_string(),
            upload_id: upload_id.to_string(),
        })
        .await
        .map_err(map_ids_error)?;
    Ok(())
}

async fn abort_checkpointed_multipart_uploads_for_rm(
    global: &GlobalArgs,
    client: &IdsClient,
    target: &ParsedADriveUri,
    checkpoint_dir: Option<&str>,
    progress_enabled: bool,
    report: &mut BatchReport,
) -> Result<(), CliError> {
    let uploads = list_checkpointed_uploads_for_rm(global, target, checkpoint_dir).await?;
    if uploads.is_empty() {
        return Ok(());
    }
    let progress = batch_progress(
        "ve-adrive rm uploads",
        uploads.len() as u64,
        progress_enabled,
    );
    for upload in uploads {
        let Some(upload_id) = upload.checkpoint.upload_id.as_deref() else {
            continue;
        };
        let source = format_adrive_upload_target(&upload.checkpoint, upload_id);
        let result = abort_adrive_multipart_upload(
            client,
            &upload.checkpoint.instance,
            &upload.checkpoint.space,
            &upload.checkpoint.file_path,
            upload_id,
        )
        .await;
        match result {
            Ok(()) => {
                report.push_success(source, None, "abort-upload");
                if let Err(err) = remove_checkpoint_file(Some(upload.path.as_path())).await {
                    // [Review Fix #ADrive-RmUploads-2] The remote upload is
                    // already aborted; keep going and surface local checkpoint
                    // cleanup failure through the same batch summary.
                    report.push_failure(
                        upload.path.display().to_string(),
                        None,
                        "remove-checkpoint",
                        err,
                    );
                }
            }
            Err(err) => report.push_failure(source, None, "abort-upload", err),
        }
        tick_progress(&progress);
    }
    finish_progress(progress);
    Ok(())
}

async fn list_checkpointed_uploads_for_rm(
    global: &GlobalArgs,
    target: &ParsedADriveUri,
    checkpoint_dir: Option<&str>,
) -> Result<Vec<ADriveUploadCheckpointRef>, CliError> {
    let mut uploads = Vec::new();
    let mut seen = HashSet::new();
    for directory in adrive_checkpoint_scan_dirs(global, checkpoint_dir)? {
        let mut entries = match tokio::fs::read_dir(&directory).await {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        };
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            if !seen.insert(path.clone()) {
                continue;
            }
            let bytes = match tokio::fs::read(&path).await {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let checkpoint = match serde_json::from_slice::<UploadCheckpoint>(&bytes) {
                Ok(checkpoint) => checkpoint,
                Err(_) => continue,
            };
            if upload_checkpoint_matches_rm_target(&checkpoint, target) {
                uploads.push(ADriveUploadCheckpointRef { path, checkpoint });
            }
        }
    }
    uploads.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(uploads)
}

fn adrive_checkpoint_scan_dirs(
    global: &GlobalArgs,
    checkpoint_dir: Option<&str>,
) -> Result<Vec<PathBuf>, CliError> {
    let mut directories = vec![
        expand_user_path(ADRIVE_DEFAULT_CHECKPOINT_DIR),
        expand_user_path(ADRIVE_LEGACY_CHECKPOINT_DIR),
    ];
    if let Some(directory) = checkpoint_dir {
        directories.push(expand_user_path(directory));
    }
    let profile = build_profile(global)?;
    if let Some(directory) = profile.checkpoint_dir.as_deref() {
        directories.push(expand_user_path(directory));
    }
    directories.sort();
    directories.dedup();
    Ok(directories)
}

fn upload_checkpoint_matches_rm_target(
    checkpoint: &UploadCheckpoint,
    target: &ParsedADriveUri,
) -> bool {
    if checkpoint
        .upload_id
        .as_deref()
        .map_or(true, |upload_id| upload_id.is_empty())
    {
        return false;
    }
    if checkpoint.instance != target.instance || checkpoint.space != target.space {
        return false;
    }
    if target.path.is_empty() {
        return true;
    }
    if target.path.ends_with('/') || target.file().is_none() {
        let prefix = target.path.trim_matches('/');
        if prefix.is_empty() {
            return true;
        }
        let child_prefix = format!("{prefix}/");
        return checkpoint.file_path == prefix || checkpoint.file_path.starts_with(&child_prefix);
    }
    checkpoint.file_path == target.path
}

fn format_adrive_upload_target(checkpoint: &UploadCheckpoint, upload_id: &str) -> String {
    let target = ParsedADriveUri {
        instance: checkpoint.instance.clone(),
        space: checkpoint.space.clone(),
        path: checkpoint.file_path.clone(),
    };
    format!("{}?uploadId={}", format_target(&target), upload_id)
}

fn normalize_ids_range(range: &str) -> String {
    let trimmed = range.trim();
    if trimmed.starts_with("bytes=") {
        trimmed.to_string()
    } else {
        format!("bytes={trimmed}")
    }
}

async fn execute_mkdir(
    global: &GlobalArgs,
    client: &IdsClient,
    args: &MkdirArgs,
) -> Result<i32, CliError> {
    let resolved_args = resolve_mkdir_args_by_name(client, args).await?;
    let args = &resolved_args;
    let target = if let Some(uri) = args.path.as_deref() {
        parse_adrive_uri(uri, false)?
    } else {
        let instance = args.instance.as_deref().ok_or_else(|| {
            CliError::ValidationError(
                "missing target: provide adrive://instance/space/folder or --instance".into(),
            )
        })?;
        let space = args.space.as_deref().ok_or_else(|| {
            CliError::ValidationError("missing --space: required with --instance".into())
        })?;
        let folder = args.folder.as_deref().ok_or_else(|| {
            CliError::ValidationError("missing --folder: required without positional URI".into())
        })?;
        ParsedADriveUri {
            instance: instance.to_string(),
            space: space.to_string(),
            path: format!("{}/", folder.trim_end_matches('/')),
        }
    };
    let folder_path = target.path.trim_matches('/');
    if folder_path.is_empty() {
        return Err(CliError::ValidationError(
            "ve-adrive mkdir requires a non-empty folder path".to_string(),
        ));
    }
    let folder_paths = folder_paths_for_mkdir(folder_path, args.parents);
    let mut created = Vec::new();
    let mut request_id = None;
    let mut created_at = None;
    for folder_path in &folder_paths {
        let input = CreateFolderInput {
            instance_id: target.instance.clone(),
            space_id: target.space.clone(),
            folder_path: folder_path.to_string(),
        };
        let out = client.create_folder(&input).await.map_err(map_ids_error)?;
        request_id = Some(out.response_info.request_id().to_string());
        created_at = Some(out.created_at);
        created.push(out.folder_path);
    }
    let created_count = created.len();
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive mkdir",
            json!({
                "target": format_target(&target),
                "folder_path": folder_path,
                "parents": args.parents,
                "created": created,
                "created_count": created_count,
                "created_at": created_at,
                "request_id": request_id,
                "status": "succeeded",
            }),
        ),
    )?;
    Ok(0)
}

fn folder_paths_for_mkdir(path: &str, parents: bool) -> Vec<String> {
    if !parents {
        return vec![path.to_string()];
    }
    let mut paths = Vec::new();
    let mut current = String::new();
    for segment in path.split('/') {
        if segment.is_empty() {
            continue;
        }
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(segment);
        paths.push(current.clone());
    }
    if paths.is_empty() {
        vec![path.to_string()]
    } else {
        paths
    }
}

fn parse_file_uri(uri: &str) -> Result<ParsedADriveUri, CliError> {
    parse_file_target(parse_adrive_uri(uri, false)?)
}

fn display_single_transfer_destination(
    source: &str,
    destination: &str,
    recursive: bool,
) -> Result<String, CliError> {
    if recursive {
        return Ok(destination.to_string());
    }

    resolve_single_transfer_destination(source, destination)
}

fn resolve_single_transfer_destination(
    source: &str,
    destination: &str,
) -> Result<String, CliError> {
    if !destination.starts_with("adrive://") {
        return Ok(destination.to_string());
    }

    let target = parse_adrive_uri(destination, false)?;
    let path = if source.starts_with("adrive://") {
        let source = parse_file_uri(source)?;
        remote_file_path_for_destination(&target, &source)?
    } else {
        let file_name = source_file_name_for_adrive_transfer(source)?;
        remote_file_path_for_name(&target, &file_name)
    };
    Ok(format_target(&ParsedADriveUri { path, ..target }))
}

fn source_file_name_for_adrive_transfer(source: &str) -> Result<String, CliError> {
    if source.starts_with("adrive://") {
        let parsed = parse_file_uri(source)?;
        return parsed.file().map(ToString::to_string).ok_or_else(|| {
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

fn remote_file_path_for_destination(
    target: &ParsedADriveUri,
    source: &ParsedADriveUri,
) -> Result<String, CliError> {
    let file_name = source.file().ok_or_else(|| {
        CliError::ValidationError(format!(
            "source URI '{}' does not include a file name",
            format_target(source)
        ))
    })?;
    let path = remote_file_path_for_name(target, file_name);
    // [Review Fix #1] Directory-style destinations such as `.../folder/` can
    // resolve to the original file; reject before a subsequent move deletes it.
    if target.instance == source.instance && target.space == source.space && path == source.path {
        return Err(CliError::ValidationError(
            "source and destination resolve to the same ADrive file".to_string(),
        ));
    }
    Ok(path)
}

fn remote_file_path_for_name(target: &ParsedADriveUri, file_name: &str) -> String {
    if target.path.is_empty() || target.path.ends_with('/') {
        format!("{}{}", target.path, file_name)
    } else {
        target.path.clone()
    }
}

fn resolve_put_file_target(args: &PutArgs) -> Result<ParsedADriveUri, CliError> {
    let target = resolve_target(
        args.path.as_deref(),
        args.instance.as_deref(),
        args.space.as_deref(),
        args.folder.as_deref(),
        args.file.as_deref(),
    )?;
    parse_file_target(target)
}

fn parse_file_target(target: ParsedADriveUri) -> Result<ParsedADriveUri, CliError> {
    if target.path.is_empty() || target.path.ends_with('/') {
        return Err(CliError::ValidationError(format!(
            "target must be an ADrive file path: {}",
            format_target(&target)
        )));
    }
    Ok(target)
}

fn remote_file_path_for_upload(
    target: &ParsedADriveUri,
    source_path: &Path,
) -> Result<String, CliError> {
    let file_name = source_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            CliError::ValidationError("source file name is not valid UTF-8".to_string())
        })?;
    Ok(remote_file_path_for_name(target, file_name))
}

fn local_destination_path(
    destination: &str,
    source: &ParsedADriveUri,
) -> Result<PathBuf, CliError> {
    let destination_path = PathBuf::from(destination);
    if destination.ends_with('/') || destination_path.is_dir() {
        let file_name = source.file().ok_or_else(|| {
            CliError::ValidationError("source URI does not include file name".to_string())
        })?;
        Ok(destination_path.join(file_name))
    } else {
        Ok(destination_path)
    }
}

async fn list_files(
    global: &GlobalArgs,
    client: &IdsClient,
    instance: &str,
    space: &str,
    path: &str,
    max_keys: i32,
    marker: Option<&str>,
    columns: Option<&str>,
    manifest_path: Option<&str>,
    human_readable: bool,
) -> Result<(), CliError> {
    let BoundedFiles {
        files,
        folders,
        next_marker,
        is_truncated,
        request_id,
    } = list_files_bounded(client, instance, space, path, max_keys, marker).await?;
    let (files, folders) = normalize_ls_files_and_folders(files, folders, path);
    let raw_files = files.clone();
    let raw_folders = folders.clone();
    let mut manifest_items = raw_files
        .iter()
        .map(|file| {
            adrive_remote_file_manifest_item(
                "list",
                file,
                format!("adrive://{}/{}/{}", instance, space, file.file_path),
                None,
            )
        })
        .collect::<Vec<_>>();
    manifest_items.extend(raw_folders.iter().map(|folder| {
        let source = format!("adrive://{}/{}/{}", instance, space, folder.folder);
        BatchManifestItem {
            operation: "list".to_string(),
            source,
            destination: None,
            size: 0,
            etag: None,
            crc64: None,
        }
    }));
    let manifest = BatchManifest::from_items(manifest_items);
    write_adrive_manifest_file(manifest_path, "ve-adrive ls", Some(&manifest)).await?;
    let mut entries = Vec::new();
    for folder in &folders {
        entries.push(adrive_ls_folder_entry(folder, human_readable));
    }
    for file in &files {
        entries.push(adrive_ls_file_entry(file, human_readable));
    }
    let target = ParsedADriveUri {
        instance: instance.to_string(),
        space: space.to_string(),
        path: path.to_string(),
    };
    let total = entries.len() as u64;
    let next_marker = next_marker.unwrap_or_default();
    let payload = if uses_tabular_output(global) {
        json!({
            "scope": "files",
            "target": format_target(&target),
            "entries": entries,
            "files": raw_files,
            "folders": raw_folders,
            "next_marker": next_marker.clone(),
            "is_truncated": is_truncated,
            "request_id": request_id,
            "manifest_path": manifest_path,
        })
    } else {
        json!({
            "scope": "files",
            "target": format_target(&target),
            "files": raw_files,
            "folders": raw_folders,
            "next_marker": next_marker.clone(),
            "is_truncated": is_truncated,
            "request_id": request_id,
            "manifest_path": manifest_path,
        })
    };
    let selected_columns = parse_columns(columns).unwrap_or(FILE_TABLE_COLUMNS);
    output_result_with_columns(
        global,
        &Envelope::success("ve-adrive ls", payload)
            .with_pagination(adrive_marker_pagination(&next_marker, total)),
        Some(selected_columns),
    )
}

fn adrive_ls_folder_entry(folder: &FolderInfo, human_readable: bool) -> Value {
    json!({
        "file_path": folder.folder,
        "size": adrive_ls_size_value(0, human_readable),
        "file_type": "folder",
        "updated_at": folder.updated_at,
        "is_folder": true,
    })
}

fn adrive_ls_file_entry(file: &FileInfo, human_readable: bool) -> Value {
    let size = adrive_file_size(file);
    json!({
        "file_path": file.file_path,
        // [Review Fix #1] Keep raw files[] numeric while making display entries
        // honor --human-readable for table/csv and optional entries consumers.
        "size": adrive_ls_size_value(size, human_readable),
        "file_type": adrive_file_type_for_output(file),
        "updated_at": file.updated_at,
        "is_folder": false,
    })
}

fn adrive_ls_size_value(size: u64, human_readable: bool) -> Value {
    if human_readable {
        Value::String(human_bytes(size))
    } else {
        json!(size)
    }
}

fn adrive_marker_pagination(next_marker: &str, total_returned: u64) -> PaginationInfo {
    PaginationInfo {
        // [Review Fix #3] ADrive's follow-up flag is `--marker`, so expose
        // marker pagination as `next_marker` instead of the TOS `next_token`.
        next_token: None,
        next_marker: (!next_marker.is_empty()).then(|| next_marker.to_string()),
        total_returned,
    }
}

fn validate_adrive_ls_max_keys(max_keys: i32) -> Result<(), CliError> {
    if max_keys <= 0 {
        return Err(CliError::ValidationError(
            "ve-adrive ls --max-keys must be greater than 0".to_string(),
        ));
    }
    Ok(())
}

fn bounded_list_page_limit(returned: i32, max_keys: i32) -> i32 {
    max_keys.saturating_sub(returned).min(1000)
}

async fn list_all_files(
    client: &IdsClient,
    instance: &str,
    space: &str,
    path: &str,
    recursive: bool,
    list_concurrency: usize,
) -> Result<Vec<FileInfo>, CliError> {
    if recursive {
        return list_all_files_hierarchical(client, instance, space, path, list_concurrency).await;
    }
    let mut marker = None;
    let mut files = Vec::new();
    loop {
        let mut input = ListFilesInput::new(instance, space).with_limit(1000);
        if !path.is_empty() {
            input = input.with_prefix(trim_folder_prefix(path));
        }
        input = input.with_delimiter("/");
        if let Some(value) = marker.take() {
            input = input.with_marker(value);
        }
        let out = client.list_files(&input).await.map_err(map_ids_error)?;
        files.extend(out.files);
        if !out.is_truncated || out.next_marker.is_empty() {
            break;
        }
        marker = Some(out.next_marker);
    }
    Ok(files)
}

struct ADriveListedEntries {
    files: Vec<FileInfo>,
    folders: Vec<FolderInfo>,
}

struct ADrivePrefixScan {
    files: Vec<FileInfo>,
    folders: Vec<FolderInfo>,
    child_prefixes: Vec<String>,
}

async fn list_all_files_hierarchical(
    client: &IdsClient,
    instance: &str,
    space: &str,
    path: &str,
    list_concurrency: usize,
) -> Result<Vec<FileInfo>, CliError> {
    Ok(
        list_all_files_and_folders_hierarchical(client, instance, space, path, list_concurrency)
            .await?
            .files,
    )
}

async fn list_all_files_and_folders_hierarchical(
    client: &IdsClient,
    instance: &str,
    space: &str,
    path: &str,
    list_concurrency: usize,
) -> Result<ADriveListedEntries, CliError> {
    let mut listed = ADriveListedEntries {
        files: Vec::new(),
        folders: Vec::new(),
    };
    let mut pending_prefixes = vec![trim_folder_prefix(path)];
    let mut seen_prefixes = HashSet::new();
    let mut in_flight = FuturesUnordered::new();
    let limit = list_concurrency.max(1);
    while !pending_prefixes.is_empty() || !in_flight.is_empty() {
        while in_flight.len() < limit {
            let Some(prefix) = pending_prefixes.pop() else {
                break;
            };
            if !seen_prefixes.insert(prefix.clone()) {
                continue;
            }
            in_flight.push(scan_adrive_list_prefix(client, instance, space, prefix));
        }

        let Some(scan) = in_flight.next().await else {
            continue;
        };
        let mut scan = scan?;
        listed.files.append(&mut scan.files);
        listed.folders.append(&mut scan.folders);
        for child_prefix in scan.child_prefixes {
            if !seen_prefixes.contains(&child_prefix) {
                pending_prefixes.push(child_prefix);
            }
        }
    }
    listed
        .files
        .sort_by(|left, right| left.file_path.cmp(&right.file_path));
    listed
        .files
        .dedup_by(|left, right| left.file_path == right.file_path);
    listed
        .folders
        .sort_by(|left, right| left.folder.cmp(&right.folder));
    listed
        .folders
        .dedup_by(|left, right| left.folder == right.folder);
    Ok(listed)
}

async fn scan_adrive_list_prefix(
    client: &IdsClient,
    instance: &str,
    space: &str,
    prefix: String,
) -> Result<ADrivePrefixScan, CliError> {
    let mut files = Vec::new();
    let mut folders = Vec::new();
    let mut child_prefixes = Vec::new();
    let mut marker = None;
    loop {
        let mut input = ListFilesInput::new(instance, space)
            .with_limit(1000)
            .with_delimiter("/");
        if !prefix.is_empty() {
            input = input.with_prefix(&prefix);
        }
        if let Some(value) = marker.take() {
            input = input.with_marker(value);
        }
        let out = client.list_files(&input).await.map_err(map_ids_error)?;
        for file in out.files {
            if adrive_file_info_is_folder_marker(&file) {
                if let Some(folder) = adrive_folder_info_from_file_marker(&file) {
                    child_prefixes.push(trim_folder_prefix(&folder.folder));
                    folders.push(folder);
                }
            } else {
                files.push(file);
            }
        }
        for folder in out.folders {
            child_prefixes.push(trim_folder_prefix(&folder.folder));
            folders.push(folder);
        }
        if !out.is_truncated || out.next_marker.is_empty() {
            break;
        }
        marker = Some(out.next_marker);
    }
    Ok(ADrivePrefixScan {
        files,
        folders,
        child_prefixes,
    })
}

fn collect_local_files(root: &Path) -> Result<Vec<PathBuf>, CliError> {
    let mut files = Vec::new();
    collect_local_files_inner(root, &mut files)?;
    Ok(files)
}

fn collect_local_files_inner(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), CliError> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_local_files_inner(&path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

fn normalize_local_relative_path(path: &Path) -> Result<String, CliError> {
    let value = path.to_str().ok_or_else(|| {
        CliError::ValidationError("local path contains non-UTF-8 characters".to_string())
    })?;
    Ok(value.replace(std::path::MAIN_SEPARATOR, "/"))
}

fn join_remote_path(base: &str, relative: &str) -> String {
    let base = base.trim_matches('/');
    let relative = relative.trim_start_matches('/');
    if base.is_empty() {
        relative.to_string()
    } else if relative.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{relative}")
    }
}

fn recursive_adrive_source_parent_prefix(
    source: &str,
    include_parent: bool,
) -> Result<Option<String>, CliError> {
    if !include_parent {
        return Ok(None);
    }
    let parent_name = if source.starts_with("adrive://") {
        let parsed = parse_adrive_uri(source, true)?;
        trim_folder_prefix(&parsed.path)
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

fn prepend_adrive_parent_prefix(relative: &str, parent_prefix: Option<&str>) -> String {
    match parent_prefix {
        Some(parent) => join_remote_path(parent, relative),
        None => relative.to_string(),
    }
}

fn recursive_adrive_relative_path(
    file_path: &str,
    source_prefix: &str,
    parent_prefix: Option<&str>,
) -> String {
    let source_relative = remote_relative_path(file_path, source_prefix);
    prepend_adrive_parent_prefix(&source_relative, parent_prefix)
}

fn path_matches_filters(path: &str, include: Option<&str>, exclude: Option<&str>) -> bool {
    if exclude.is_some_and(|pattern| simple_pattern_match(pattern, path)) {
        return false;
    }
    include
        .map(|pattern| simple_pattern_match(pattern, path))
        .unwrap_or(true)
}

fn simple_pattern_match(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let parts = pattern.split('*').collect::<Vec<_>>();
    if parts.len() == 1 {
        return value.contains(pattern);
    }
    let mut remainder = value;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let Some(position) = remainder.find(part) else {
            return false;
        };
        if index == 0 && !pattern.starts_with('*') && position != 0 {
            return false;
        }
        remainder = &remainder[position + part.len()..];
    }
    pattern.ends_with('*') || parts.last().is_some_and(|last| value.ends_with(last))
}

fn adrive_find_entry_matches(
    file: &FileInfo,
    args: &FindArgs,
    size_filter: Option<FindSizeFilter>,
    mtime_filter: Option<FindMtimeFilterMillis>,
) -> bool {
    if let Some(pattern) = &args.name {
        if !simple_pattern_match(pattern, &file.file_path) {
            return false;
        }
    }
    if let Some(filter) = size_filter {
        if !find_size_matches(adrive_file_size(file), filter) {
            return false;
        }
    }
    if let Some(filter) = mtime_filter {
        if !find_mtime_millis_matches(file.updated_at, filter) {
            return false;
        }
    }
    true
}

fn adrive_find_match_row(file: &FileInfo) -> Value {
    json!({
        "file_path": file.file_path.clone(),
        "size": adrive_file_size(file),
        "file_type": adrive_file_type_for_output(file),
        "storage_class": file.storage_class.clone(),
        "updated_at": file.updated_at,
        "is_folder": false,
        "etag": file.etag.clone(),
    })
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

fn parse_find_mtime_filter_millis(filter: &str) -> Result<FindMtimeFilterMillis, CliError> {
    let (operator, value) = match filter.as_bytes().first().copied() {
        Some(b'-') => (b'-', &filter[1..]),
        Some(b'+') => (b'+', &filter[1..]),
        _ => (b'=', filter),
    };
    let duration = parse_relative_duration_millis(value).ok_or_else(|| {
        CliError::ValidationError(format!(
            "invalid --mtime filter '{}': expected [+|-]<number>[s|m|h|d] or <number>[s|m|h|d]",
            filter
        ))
    })?;
    Ok(match operator {
        b'-' => FindMtimeFilterMillis::WithinLast(duration.duration_millis),
        b'+' => FindMtimeFilterMillis::OlderThanOrEqual(duration.duration_millis),
        b'=' => FindMtimeFilterMillis::EqualAge {
            duration_millis: duration.duration_millis,
            unit_millis: duration.unit_millis,
        },
        _ => unreachable!("operator is checked above"),
    })
}

fn parse_relative_duration_millis(value: &str) -> Option<RelativeDurationMillis> {
    let trimmed = value.trim();
    let digits_len = trimmed
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .map(char::len_utf8)
        .sum::<usize>();
    let number = trimmed.get(..digits_len)?.parse::<u64>().ok()?;
    let unit = trimmed.get(digits_len..)?.trim().to_ascii_lowercase();
    let unit_millis = match unit.as_str() {
        "" | "s" | "sec" | "secs" | "second" | "seconds" => 1_000,
        "m" | "min" | "mins" | "minute" | "minutes" => 60_000,
        "h" | "hr" | "hrs" | "hour" | "hours" => 60 * 60_000,
        "d" | "day" | "days" => 24 * 60 * 60_000,
        _ => return None,
    };
    Some(RelativeDurationMillis {
        duration_millis: number.checked_mul(unit_millis)?,
        unit_millis,
    })
}

fn find_mtime_millis_matches(updated_at: i64, filter: FindMtimeFilterMillis) -> bool {
    if updated_at <= 0 {
        return false;
    }
    let now = chrono::Utc::now().timestamp_millis();
    match filter {
        FindMtimeFilterMillis::WithinLast(millis) => {
            let threshold = now.saturating_sub(millis.min(i64::MAX as u64) as i64);
            updated_at >= threshold
        }
        FindMtimeFilterMillis::OlderThanOrEqual(millis) => {
            let threshold = now.saturating_sub(millis.min(i64::MAX as u64) as i64);
            updated_at <= threshold
        }
        FindMtimeFilterMillis::EqualAge {
            duration_millis,
            unit_millis,
        } => {
            // [Review Fix #2] Bare --mtime values match the whole age bucket
            // for their unit, e.g. 7d means age >= 7d and < 8d.
            let duration = duration_millis.min(i64::MAX as u64) as i64;
            let unit = unit_millis.min(i64::MAX as u64) as i64;
            let newest = now.saturating_sub(duration);
            let oldest_exclusive = newest.saturating_sub(unit);
            updated_at <= newest && updated_at > oldest_exclusive
        }
    }
}

const ADRIVE_REPORT_COLUMNS: &[&str] = &[
    "command",
    "operation",
    "source",
    "destination",
    "status",
    "error",
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

fn adrive_report_record(report: &BatchReport, item: &BatchItemResult) -> [String; 6] {
    [
        // [Review Fix #6] CSV reports are user-facing artifacts, while the
        // internal batch executor still keys reports by registry command.
        public_adrive_command_path(&report.command),
        item.operation.clone(),
        item.source.clone(),
        item.destination.clone().unwrap_or_default(),
        item.status.clone(),
        item.error.clone().unwrap_or_default(),
    ]
}

async fn write_batch_report(
    path: Option<&str>,
    report: &BatchReport,
    failures_only: bool,
    _include_manifest: bool,
) -> Result<(), CliError> {
    let Some(path) = path else {
        return Ok(());
    };
    let mut writer = RollingCsvWriter::new(path, ADRIVE_REPORT_COLUMNS)?;
    for item in &report.items {
        if failures_only && item.status != "failed" {
            continue;
        }
        writer.write_record(&adrive_report_record(report, item))?;
    }
    Ok(())
}

fn effective_report_path(
    global: &GlobalArgs,
    explicit: Option<&str>,
    command: &str,
) -> Result<Option<String>, CliError> {
    if let Some(path) = explicit {
        return Ok(Some(expand_user_path(path).to_string_lossy().into_owned()));
    }
    let profile = build_profile(global)?;
    let report_format = profile
        .batch_report_format
        .unwrap_or_else(|| DEFAULT_TOS_BATCH_REPORT_FORMAT.to_string());
    if report_format != "csv" {
        return Err(CliError::ValidationError(format!(
            "unsupported batch_report_format '{}': only csv is supported",
            report_format
        )));
    }
    let report_dir_raw = profile
        .batch_report_dir
        .unwrap_or_else(|| ADRIVE_DEFAULT_BATCH_REPORT_DIR.to_string());
    let report_dir = writable_default_report_dir(report_dir_raw.trim_end_matches('/'))?;
    let file_name = format!(
        "{}-{}-{}.csv",
        command.replace("ve-adrive ", "").replace(' ', "-"),
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
        .unwrap_or_else(|| ADRIVE_DEFAULT_BATCH_REPORT_DIR.to_string());
    let report_dir = writable_default_report_dir(report_dir_raw.trim_end_matches('/'))?;
    let file_name = format!(
        "{}-manifest-{}-{}.csv",
        command.replace("ve-adrive ", "").replace(' ', "-"),
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
    let probe = dir.join(format!(".adrive-write-probe-{}", std::process::id()));
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe)?;
    let _ = fs::remove_file(probe);
    Ok(())
}

async fn write_adrive_manifest_file(
    path: Option<&str>,
    command: &str,
    manifest: Option<&BatchManifest>,
) -> Result<(), CliError> {
    let Some(path) = path else {
        return Ok(());
    };
    let Some(manifest) = manifest else {
        return Err(CliError::ValidationError(format!(
            "{} has no manifest to write",
            command
        )));
    };
    let mut writer = RollingCsvWriter::new(
        path,
        &[
            "command",
            "operation",
            "source",
            "destination",
            "size",
            "etag",
            "crc64",
        ],
    )?;
    // [Review Fix #7] Manifest CSV is another user-facing batch artifact, so
    // it must expose the public top-level command path.
    let public_command = public_adrive_command_path(command);
    for item in &manifest.items {
        writer.write_record(&[
            public_command.clone(),
            item.operation.clone(),
            item.source.clone(),
            item.destination.clone().unwrap_or_default(),
            item.size.to_string(),
            item.etag.clone().unwrap_or_default(),
            item.crc64
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ])?;
    }
    Ok(())
}

async fn write_adrive_single_report(
    path: Option<&str>,
    command: &str,
    source: String,
    destination: Option<String>,
    operation: &str,
    status: &str,
    error: Option<String>,
    failures_only: bool,
) -> Result<(), CliError> {
    let Some(path) = path else {
        return Ok(());
    };
    if failures_only && status != "failed" {
        return Ok(());
    }
    let mut report = BatchReport::new(command);
    match status {
        "succeeded" => report.push_success(source, destination, operation),
        "skipped" => report.push_skipped(source, destination, operation),
        _ => report.items.push(BatchItemResult {
            source,
            destination,
            operation: operation.to_string(),
            status: "failed".to_string(),
            error,
        }),
    }
    write_batch_report(Some(path), &report, false, false).await
}

async fn read_file_part(path: &Path, offset: u64, length: u64) -> Result<Vec<u8>, CliError> {
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(SeekFrom::Start(offset)).await?;
    let mut limited = file.take(length);
    let mut buffer = Vec::with_capacity(length.min(ADRIVE_MULTIPART_PART_SIZE) as usize);
    limited.read_to_end(&mut buffer).await?;
    Ok(buffer)
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

fn crc64_for_bytes(body: &[u8]) -> u64 {
    let mut digest = Digest::new();
    let _ = digest.write(body);
    digest.sum64()
}

fn verify_adrive_crc64_response(
    remote_crc64: u64,
    local_crc64: u64,
    label: &str,
) -> Result<(), CliError> {
    if remote_crc64 != 0 && remote_crc64 != local_crc64 {
        return Err(CliError::TransferFailed(format!(
            "{} CRC64 mismatch: local={}, remote={}",
            label, local_crc64, remote_crc64
        )));
    }
    Ok(())
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
    progress.enable_steady_tick(std::time::Duration::from_millis(200));
    Some(progress)
}

async fn local_file_crc64(path: &Path) -> Result<u64, CliError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut digest = Digest::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let bytes_read = file.read(&mut buffer).await?;
        if bytes_read == 0 {
            break;
        }
        let _ = digest.write(&buffer[..bytes_read]);
    }
    Ok(digest.sum64())
}

async fn write_download_stream(
    mut output: GetFileOutput,
    destination: &Path,
    append: bool,
    rate_limiter: Option<Arc<RateLimiter>>,
) -> Result<u64, CliError> {
    let mut file = if append {
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(destination)
            .await?
    } else {
        tokio::fs::File::create(destination).await?
    };
    let mut written = 0_u64;
    while let Some(chunk) = output.next().await {
        let chunk = chunk?;
        throttle_bytes(rate_limiter.as_deref(), chunk.len()).await;
        file.write_all(&chunk).await?;
        written = written.saturating_add(chunk.len() as u64);
    }
    file.flush().await?;
    Ok(written)
}

async fn download_adrive_file_ranges(
    client: &IdsClient,
    source: &ParsedADriveUri,
    source_etag: &str,
    file_size: u64,
    destination: &Path,
    runtime: TransferRuntimeConfig,
    rate_limiter: Option<Arc<RateLimiter>>,
) -> Result<usize, CliError> {
    let range_base_path = adrive_range_base_path(destination, source_etag, file_size);
    let mut ranges = Vec::new();
    let mut part_number = 1_u32;
    let mut offset = 0_u64;
    while offset < file_size {
        let current_size = (file_size - offset).min(ADRIVE_MULTIPART_PART_SIZE);
        ranges.push((part_number, offset, current_size));
        part_number += 1;
        offset += current_size;
    }
    let mut pending_ranges = ranges.clone().into_iter().filter(|(part_number, _, size)| {
        let part_path = adrive_range_part_path(&range_base_path, *part_number);
        std::fs::metadata(part_path)
            .map(|metadata| metadata.len() != *size)
            .unwrap_or(true)
    });
    let mut in_flight = FuturesUnordered::new();
    loop {
        while in_flight.len() < runtime.multipart_concurrency {
            let Some((part_number, part_offset, current_size)) = pending_ranges.next() else {
                break;
            };
            let part_path = adrive_range_part_path(&range_base_path, part_number);
            in_flight.push(download_adrive_range_part(
                client,
                source,
                source_etag,
                part_offset,
                current_size,
                part_path,
                rate_limiter.clone(),
            ));
        }
        let Some(result) = in_flight.next().await else {
            break;
        };
        result?;
    }
    assemble_adrive_range_parts(&range_base_path, destination, &ranges)?;
    cleanup_adrive_range_parts(&range_base_path, &ranges)?;
    Ok(ranges.len())
}

async fn download_adrive_range_part(
    client: &IdsClient,
    source: &ParsedADriveUri,
    source_etag: &str,
    offset: u64,
    size: u64,
    part_path: PathBuf,
    rate_limiter: Option<Arc<RateLimiter>>,
) -> Result<u64, CliError> {
    let end = offset + size - 1;
    let mut input = GetFileInput::new(&source.instance, &source.space, &source.path)
        .with_range_raw(format!("bytes={offset}-{end}"));
    input.if_match = Some(source_etag.to_string());
    let out = client.get_file(&input).await.map_err(map_ids_error)?;
    let written = write_download_stream(out, &part_path, false, rate_limiter).await?;
    if written != size {
        let _ = std::fs::remove_file(&part_path);
        return Err(CliError::TransferFailed(format!(
            "range download length mismatch for '{}': expected={}, actual={}",
            part_path.display(),
            size,
            written
        )));
    }
    Ok(written)
}

fn adrive_range_base_path(destination: &Path, source_etag: &str, file_size: u64) -> PathBuf {
    let file_name = destination
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source_etag.hash(&mut hasher);
    file_size.hash(&mut hasher);
    destination.display().to_string().hash(&mut hasher);
    destination.with_file_name(format!("{file_name}.adrive-range-{:016x}", hasher.finish()))
}

fn adrive_range_part_path(range_base_path: &Path, part_number: u32) -> PathBuf {
    let file_name = range_base_path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    range_base_path.with_file_name(format!("{file_name}.part-{part_number}"))
}

fn assemble_adrive_range_parts(
    range_base_path: &Path,
    destination: &Path,
    ranges: &[(u32, u64, u64)],
) -> Result<(), CliError> {
    let mut output = std::fs::File::create(destination)?;
    for (part_number, _, _) in ranges {
        let part_path = adrive_range_part_path(range_base_path, *part_number);
        let mut input = std::fs::File::open(&part_path)?;
        std::io::copy(&mut input, &mut output)?;
    }
    Ok(())
}

fn cleanup_adrive_range_parts(
    range_base_path: &Path,
    ranges: &[(u32, u64, u64)],
) -> Result<(), CliError> {
    for (part_number, _, _) in ranges {
        let part_path = adrive_range_part_path(range_base_path, *part_number);
        if let Err(err) = std::fs::remove_file(&part_path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                return Err(err.into());
            }
        }
    }
    Ok(())
}

async fn throttle_bytes(rate_limiter: Option<&RateLimiter>, bytes: usize) {
    let Some(rate_limiter) = rate_limiter else {
        return;
    };
    let (allowed, wait) = rate_limiter.acquire(bytes);
    if !allowed {
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
    }
}

async fn head_file_optional(
    client: &IdsClient,
    instance: &str,
    space: &str,
    path: &str,
) -> Result<Option<crate::domain::types::HeadFileOutput>, CliError> {
    match client
        .head_file(&HeadFileInput::new(instance, space, path))
        .await
    {
        Ok(head) => Ok(Some(head)),
        Err(err) if is_ids_not_found(&err) => Ok(None),
        Err(err) => Err(map_ids_error(err)),
    }
}

async fn should_skip_remote_destination(
    client: &IdsClient,
    instance: &str,
    space: &str,
    path: &str,
    source_updated_at: Option<i64>,
    strategy: EffectiveOverwriteStrategy,
) -> Result<bool, CliError> {
    match strategy {
        EffectiveOverwriteStrategy::Force => Ok(false),
        EffectiveOverwriteStrategy::NoClobber => {
            Ok(head_file_optional(client, instance, space, path)
                .await?
                .is_some())
        }
        EffectiveOverwriteStrategy::Newer => {
            let Some(destination) = head_file_optional(client, instance, space, path).await? else {
                return Ok(false);
            };
            let Some(source_updated_at) = source_updated_at else {
                return Ok(false);
            };
            Ok(source_updated_at <= destination.updated_at)
        }
    }
}

fn local_modified_millis(path: &Path) -> Result<i64, CliError> {
    let modified = std::fs::metadata(path)?.modified()?;
    let millis = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|err| CliError::ValidationError(format!("invalid file mtime: {}", err)))?
        .as_millis();
    Ok(millis.min(i64::MAX as u128) as i64)
}

fn should_skip_local_destination(
    destination: &Path,
    source_updated_at: Option<i64>,
    strategy: EffectiveOverwriteStrategy,
) -> Result<bool, CliError> {
    if !destination.exists() {
        return Ok(false);
    }
    match strategy {
        EffectiveOverwriteStrategy::Force => Ok(false),
        EffectiveOverwriteStrategy::NoClobber => Ok(true),
        EffectiveOverwriteStrategy::Newer => {
            let Some(source_updated_at) = source_updated_at else {
                return Ok(false);
            };
            let destination_updated_at = local_modified_millis(destination)?;
            Ok(source_updated_at <= destination_updated_at)
        }
    }
}

fn is_ids_not_found(err: &IdsSdkError) -> bool {
    matches!(
        err,
        IdsSdkError::Server(server) if server.status_code == Some(404)
    )
}

fn rate_limiter_from_limit(limit: Option<&str>) -> Result<Option<Arc<RateLimiter>>, CliError> {
    let Some(limit) = limit else {
        return Ok(None);
    };
    let bytes_per_second = parse_size_bytes(limit).ok_or_else(|| {
        CliError::ValidationError(format!(
            "invalid bandwidth limit '{}': expected <number>[B|KB|MB|GB|TB]",
            limit
        ))
    })?;
    let capped = bytes_per_second.min(i64::MAX as u64) as i64;
    Ok(Some(Arc::new(RateLimiter::new(capped, capped))))
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

fn should_use_multipart(
    file_size: u64,
    checkpoint_enabled: bool,
    checkpoint_threshold: u64,
) -> bool {
    file_size > ADRIVE_SINGLE_PUT_LIMIT || (checkpoint_enabled && file_size >= checkpoint_threshold)
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

fn checkpoint_path_for_cp(args: &CpArgs) -> Result<Option<PathBuf>, CliError> {
    if !args.checkpoint {
        return Ok(None);
    }
    let dir = args
        .checkpoint_dir
        .as_deref()
        .unwrap_or(ADRIVE_DEFAULT_CHECKPOINT_DIR);
    let dir = expand_user_path(dir);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "ve-adrive cp".hash(&mut hasher);
    args.source.hash(&mut hasher);
    args.destination.hash(&mut hasher);
    args.include.hash(&mut hasher);
    args.exclude.hash(&mut hasher);
    args.recursive.hash(&mut hasher);
    let fingerprint = hasher.finish();
    Ok(Some(dir.join(format!("cp-{fingerprint:016x}.json"))))
}

fn checkpoint_path_for_adrive_batch_file(
    checkpoint_enabled: bool,
    checkpoint_dir: Option<&str>,
    source_path: &Path,
    destination: &ParsedADriveUri,
) -> Result<Option<PathBuf>, CliError> {
    if !checkpoint_enabled {
        return Ok(None);
    }
    let dir = checkpoint_dir.unwrap_or(ADRIVE_DEFAULT_CHECKPOINT_DIR);
    let dir = expand_user_path(dir);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    "ve-adrive cp batch file".hash(&mut hasher);
    source_path.display().to_string().hash(&mut hasher);
    destination.instance.hash(&mut hasher);
    destination.space.hash(&mut hasher);
    destination.path.hash(&mut hasher);
    let fingerprint = hasher.finish();
    Ok(Some(dir.join(format!("cp-file-{fingerprint:016x}.json"))))
}

async fn load_upload_checkpoint(path: Option<&Path>) -> Result<Option<UploadCheckpoint>, CliError> {
    let Some(path) = path else {
        return Ok(None);
    };
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

async fn save_upload_checkpoint(
    path: Option<&Path>,
    checkpoint: &UploadCheckpoint,
) -> Result<(), CliError> {
    let Some(path) = path else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let body = serde_json::to_vec_pretty(checkpoint)?;
    tokio::fs::write(path, body).await?;
    Ok(())
}

async fn remove_checkpoint_file(path: Option<&Path>) -> Result<(), CliError> {
    let Some(path) = path else {
        return Ok(());
    };
    match tokio::fs::remove_file(path).await {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn validate_upload_checkpoint(
    checkpoint: &UploadCheckpoint,
    destination: &ParsedADriveUri,
    source_path: &Path,
    file_path: &str,
    file_size: u64,
) -> Result<(), CliError> {
    if checkpoint.instance != destination.instance
        || checkpoint.space != destination.space
        || checkpoint.file_path != file_path
        || checkpoint.source_path != source_path.display().to_string()
        || checkpoint.file_size != file_size
        || checkpoint.part_size != ADRIVE_MULTIPART_PART_SIZE
    {
        return Err(CliError::Conflict(
            "checkpoint metadata does not match this ADrive upload task; remove the checkpoint to restart".to_string(),
        ));
    }
    Ok(())
}

fn checkpoint_item_key(operation: &str, source: &str, destination: Option<&str>) -> String {
    format!("{}|{}|{}", operation, source, destination.unwrap_or(""))
}

async fn load_batch_checkpoint(path: Option<&Path>) -> Result<BatchCheckpoint, CliError> {
    let Some(path) = path else {
        return Ok(BatchCheckpoint::default());
    };
    match tokio::fs::read(path).await {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(BatchCheckpoint::default()),
        Err(err) => Err(err.into()),
    }
}

async fn save_batch_checkpoint(
    path: Option<&Path>,
    checkpoint: &BatchCheckpoint,
) -> Result<(), CliError> {
    let Some(path) = path else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let body = serde_json::to_vec_pretty(checkpoint)?;
    tokio::fs::write(path, body).await?;
    Ok(())
}

fn expand_user_path(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn stderr_is_tty() -> bool {
    std::io::stderr().is_terminal()
}

fn resolve_list_echo_plan(
    global: &GlobalArgs,
    list_echo: bool,
    no_list_echo: bool,
) -> OutputRenderPlan {
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
    OutputRenderPlan {
        enabled,
        disabled_reason,
    }
}

fn resolve_progress_plan(
    global: &GlobalArgs,
    progress: bool,
    no_progress: bool,
) -> Result<OutputRenderPlan, CliError> {
    let profile = build_profile(global)?;
    let config_progress = profile
        .progress_enabled
        .unwrap_or(DEFAULT_TOS_PROGRESS_ENABLED);
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
    Ok(OutputRenderPlan {
        enabled,
        disabled_reason,
    })
}

fn effective_progress_enabled(
    global: &GlobalArgs,
    progress: bool,
    no_progress: bool,
) -> Result<bool, CliError> {
    Ok(resolve_progress_plan(global, progress, no_progress)?.enabled)
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

fn resolve_traversal_echo_plan(
    global: &GlobalArgs,
    list_echo: bool,
    no_list_echo: bool,
    progress: bool,
    no_progress: bool,
) -> OutputRenderPlan {
    if list_echo || no_list_echo {
        resolve_list_echo_plan(global, list_echo, no_list_echo)
    } else {
        resolve_list_echo_plan(global, progress, no_progress)
    }
}

fn output_not_applicable_plan() -> OutputRenderPlan {
    OutputRenderPlan {
        enabled: false,
        disabled_reason: Some("not_applicable"),
    }
}

fn batch_progress(label: &str, total: u64, progress_enabled: bool) -> Option<ProgressBar> {
    if !progress_enabled || total == 0 {
        return None;
    }
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{msg} [{bar:30.cyan/blue}] {pos}/{len} ({per_sec}, ETA {eta})",
        )
        .unwrap_or_else(|_| ProgressStyle::default_bar())
        .progress_chars("=> "),
    );
    pb.set_message(label.to_string());
    Some(pb)
}

fn streaming_batch_progress(enabled: bool, label: &str) -> Option<ProgressBar> {
    if !enabled {
        return None;
    }
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{prefix} {spinner} {pos} item(s) processed ({elapsed})")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.set_prefix(label.to_string());
    // [Review Fix #6] `--no-manifest` can stream recursive discovery into
    // execution, so the final total is not known when progress starts.
    pb.enable_steady_tick(Duration::from_millis(200));
    Some(pb)
}

fn finish_streaming_progress(progress: Option<ProgressBar>, total: u64) {
    if let Some(progress) = progress {
        progress.finish_with_message(format!("{total} item(s) processed"));
    }
}

fn traversal_progress(label: &str, target: &str, enabled: bool) -> Option<ProgressBar> {
    if !enabled {
        return None;
    }
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{prefix} {spinner} traversing {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    pb.set_prefix(label.to_string());
    pb.set_message(target.to_string());
    pb.enable_steady_tick(Duration::from_millis(200));
    Some(pb)
}

struct PlanScanProgress {
    bar: Option<ProgressBar>,
}

impl PlanScanProgress {
    fn new(enabled: bool, label: &str, target: &str) -> Self {
        if !enabled {
            return Self { bar: None };
        }
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{prefix} {spinner} scanning {msg} ({elapsed})")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        bar.set_prefix(label.to_string());
        bar.set_message(target.to_string());
        // [Review Fix #5] Recursive ADrive copy/sync/rm has a remote list
        // planning phase before manifest and batch progress can be created.
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

impl Drop for PlanScanProgress {
    fn drop(&mut self) {
        self.finish_and_clear();
    }
}

fn adrive_file_size(file: &FileInfo) -> u64 {
    file.size.max(0) as u64
}

fn adrive_file_type_for_output(file: &FileInfo) -> String {
    if file.file_type.trim().is_empty() {
        "file".to_string()
    } else {
        file.file_type.clone()
    }
}

fn adrive_remote_file_manifest_item(
    operation: &str,
    file: &FileInfo,
    source: String,
    destination: Option<String>,
) -> BatchManifestItem {
    BatchManifestItem {
        operation: operation.to_string(),
        source,
        destination,
        size: adrive_file_size(file),
        etag: (!file.etag.is_empty()).then(|| file.etag.clone()),
        crc64: (file.hash_crc64_ecma != 0).then_some(file.hash_crc64_ecma),
    }
}

fn adrive_remote_folder_manifest_item(
    operation: &str,
    instance: &str,
    space: &str,
    folder_path: &str,
) -> BatchManifestItem {
    BatchManifestItem {
        operation: operation.to_string(),
        source: format!(
            "adrive://{}/{}/{}",
            instance,
            space,
            normalize_adrive_folder_path(folder_path)
        ),
        destination: None,
        size: 0,
        etag: None,
        crc64: None,
    }
}

fn batch_progress_total(sizes: impl Iterator<Item = u64>, runtime: TransferRuntimeConfig) -> u64 {
    match runtime.progress_granularity {
        EffectiveProgressGranularity::Part => sizes.count() as u64,
        EffectiveProgressGranularity::Byte => sizes.sum(),
    }
}

fn progress_units_for_size(size: u64, runtime: TransferRuntimeConfig) -> u64 {
    match runtime.progress_granularity {
        EffectiveProgressGranularity::Part => 1,
        EffectiveProgressGranularity::Byte => size,
    }
}

fn multipart_progress_units_for_part(
    part_number: u64,
    part_size: u64,
    file_size: u64,
    runtime: TransferRuntimeConfig,
) -> u64 {
    let offset = part_number.saturating_sub(1).saturating_mul(part_size);
    let length = file_size.saturating_sub(offset).min(part_size);
    progress_units_for_size(length, runtime)
}

fn tick_progress_by(progress: &Option<ProgressBar>, units: u64) {
    if let Some(progress) = progress {
        progress.inc(units);
    }
}

fn tick_progress(progress: &Option<ProgressBar>) {
    tick_progress_by(progress, 1);
}

fn finish_progress(progress: Option<ProgressBar>) {
    if let Some(progress) = progress {
        progress.finish_and_clear();
    }
}

fn batch_summary(report: &BatchReport) -> serde_json::Value {
    json!({
        "total": report.total,
        "succeeded": report.succeeded,
        "failed": report.failed,
        "skipped": report.skipped,
    })
}

fn adrive_delete_source_manifest_items(report: &BatchReport) -> Vec<BatchManifestItem> {
    report
        .manifest
        .as_ref()
        .map(|manifest| {
            manifest
                .items
                .iter()
                .map(|item| BatchManifestItem {
                    operation: "delete-source".to_string(),
                    source: item.source.clone(),
                    destination: None,
                    size: item.size,
                    etag: item.etag.clone(),
                    crc64: item.crc64,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn append_adrive_manifest_items(report: &mut BatchReport, items: Vec<BatchManifestItem>) {
    if items.is_empty() {
        return;
    }
    // [Review Fix #MoveManifest] Keep recursive move's copy and cleanup stages in
    // one manifest so the report and manifest describe the same execution plan.
    match &mut report.manifest {
        Some(manifest) => {
            manifest.items.extend(items);
            manifest.object_count = manifest.items.len();
            manifest.total_size = manifest.items.iter().map(|item| item.size).sum();
        }
        None => report.set_manifest(items),
    }
}

async fn cleanup_recursive_move_source(client: &IdsClient, source: &str) -> Result<(), CliError> {
    if source.starts_with("adrive://") {
        let target = parse_adrive_uri(source, false)?;
        client
            .delete_folder(&DeleteFolderInput::new(
                &target.instance,
                &target.space,
                target.path.trim_end_matches('/'),
            ))
            .await
            .map_err(map_ids_error)?;
    } else {
        tokio::fs::remove_dir_all(source).await?;
    }
    Ok(())
}

async fn build_adrive_sync_delete_plan(
    client: &IdsClient,
    args: &SyncArgs,
    list_concurrency: usize,
) -> Result<Vec<BatchManifestItem>, CliError> {
    if !args.force {
        return Err(CliError::ValidationError(format!(
            "ve-adrive sync --delete requires --force and --confirm {} to delete extraneous destination files/folders",
            args.destination
        )));
    }
    let mut plan = match (
        args.source.starts_with("adrive://"),
        args.destination.starts_with("adrive://"),
    ) {
        (false, true) => {
            build_adrive_delete_plan_for_local_source(client, args, list_concurrency).await
        }
        (true, false) => {
            build_local_delete_plan_for_adrive_source(client, args, list_concurrency).await
        }
        (true, true) => {
            build_adrive_delete_plan_for_adrive_source(client, args, list_concurrency).await
        }
        (false, false) => Err(CliError::ValidationError(
            "ve-adrive sync requires one side to be adrive://instance/space/path".to_string(),
        )),
    }?;
    // [Review Fix #3] Keep sync delete-extra ordered child-first for hierarchical paths.
    sort_adrive_sync_delete_plan_bottom_up(&mut plan);
    Ok(plan)
}

async fn build_adrive_delete_plan_for_local_source(
    client: &IdsClient,
    args: &SyncArgs,
    list_concurrency: usize,
) -> Result<Vec<BatchManifestItem>, CliError> {
    let source_root = PathBuf::from(&args.source);
    let destination = parse_adrive_uri(&args.destination, false)?;
    let parent_prefix = recursive_adrive_source_parent_prefix(&args.source, args.include_parent)?;
    let desired = collect_local_sync_desired_relative_paths(
        &source_root,
        parent_prefix.as_deref(),
        args.include.as_deref(),
        args.exclude.as_deref(),
    )?;
    let destination_entries = list_all_files_and_folders_hierarchical(
        client,
        &destination.instance,
        &destination.space,
        &destination.path,
        list_concurrency,
    )
    .await?;
    let destination_prefix = trim_folder_prefix(&destination.path);
    let mut plan = Vec::new();
    for file in destination_entries.files {
        let relative = remote_relative_path(&file.file_path, &destination_prefix);
        if !path_matches_filters(&relative, args.include.as_deref(), args.exclude.as_deref())
            || desired.files.contains(&relative)
        {
            continue;
        }
        plan.push(adrive_remote_file_manifest_item(
            SYNC_DELETE_EXTRA_FILE,
            &file,
            format!(
                "adrive://{}/{}/{}",
                destination.instance, destination.space, file.file_path
            ),
            None,
        ));
    }
    // [Review Fix #4] ADrive sync --delete must mirror HNS behavior and remove extra folders too.
    append_adrive_extra_folder_delete_items(
        &mut plan,
        destination_entries.folders,
        &destination,
        &destination_prefix,
        &desired.folders,
        args.include.as_deref(),
        args.exclude.as_deref(),
    );
    Ok(plan)
}

async fn build_local_delete_plan_for_adrive_source(
    client: &IdsClient,
    args: &SyncArgs,
    list_concurrency: usize,
) -> Result<Vec<BatchManifestItem>, CliError> {
    let source = parse_adrive_uri(&args.source, false)?;
    let destination_root = PathBuf::from(&args.destination);
    let source_prefix = trim_folder_prefix(&source.path);
    let parent_prefix = recursive_adrive_source_parent_prefix(&args.source, args.include_parent)?;
    let source_files = list_all_files(
        client,
        &source.instance,
        &source.space,
        &source.path,
        true,
        list_concurrency,
    )
    .await?
    .into_iter()
    .filter_map(|file| {
        let relative = recursive_adrive_relative_path(
            &file.file_path,
            &source_prefix,
            parent_prefix.as_deref(),
        );
        path_matches_filters(&relative, args.include.as_deref(), args.exclude.as_deref())
            .then_some(relative)
    })
    .collect::<BTreeSet<_>>();
    let local_files = if destination_root.exists() {
        collect_local_files(&destination_root)?
    } else {
        Vec::new()
    };
    let mut plan = Vec::new();
    for file in local_files {
        let relative = file
            .strip_prefix(&destination_root)
            .ok()
            .and_then(|relative| normalize_local_relative_path(relative).ok());
        let Some(relative) = relative else {
            continue;
        };
        if !path_matches_filters(&relative, args.include.as_deref(), args.exclude.as_deref())
            || source_files.contains(&relative)
        {
            continue;
        }
        let size = fs::metadata(&file)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        plan.push(BatchManifestItem {
            operation: "delete-extra".to_string(),
            source: file.display().to_string(),
            destination: None,
            size,
            etag: None,
            crc64: None,
        });
    }
    Ok(plan)
}

async fn build_adrive_delete_plan_for_adrive_source(
    client: &IdsClient,
    args: &SyncArgs,
    list_concurrency: usize,
) -> Result<Vec<BatchManifestItem>, CliError> {
    let source = parse_adrive_uri(&args.source, false)?;
    let destination = parse_adrive_uri(&args.destination, false)?;
    if source.instance != destination.instance {
        return Err(CliError::ValidationError(
            "ve-adrive sync --delete for remote sync requires source and destination in the same instance".to_string(),
        ));
    }
    let source_prefix = trim_folder_prefix(&source.path);
    let destination_prefix = trim_folder_prefix(&destination.path);
    let parent_prefix = recursive_adrive_source_parent_prefix(&args.source, args.include_parent)?;
    let source_entries = list_all_files_and_folders_hierarchical(
        client,
        &source.instance,
        &source.space,
        &source.path,
        list_concurrency,
    )
    .await?;
    let desired = collect_remote_sync_desired_relative_paths(
        source_entries,
        &source_prefix,
        parent_prefix.as_deref(),
        args.include.as_deref(),
        args.exclude.as_deref(),
    );
    let destination_entries = list_all_files_and_folders_hierarchical(
        client,
        &destination.instance,
        &destination.space,
        &destination.path,
        list_concurrency,
    )
    .await?;
    let mut plan = Vec::new();
    for file in destination_entries.files {
        let relative = remote_relative_path(&file.file_path, &destination_prefix);
        if !path_matches_filters(&relative, args.include.as_deref(), args.exclude.as_deref())
            || desired.files.contains(&relative)
        {
            continue;
        }
        plan.push(adrive_remote_file_manifest_item(
            SYNC_DELETE_EXTRA_FILE,
            &file,
            format!(
                "adrive://{}/{}/{}",
                destination.instance, destination.space, file.file_path
            ),
            None,
        ));
    }
    // [Review Fix #4] ADrive remote sync delete planning includes folder deltas.
    append_adrive_extra_folder_delete_items(
        &mut plan,
        destination_entries.folders,
        &destination,
        &destination_prefix,
        &desired.folders,
        args.include.as_deref(),
        args.exclude.as_deref(),
    );
    Ok(plan)
}

async fn execute_adrive_sync_delete_plan(
    client: &IdsClient,
    mut delete_plan: Vec<BatchManifestItem>,
    batch_concurrency: usize,
    progress_enabled: bool,
    report: &mut BatchReport,
) -> Result<usize, CliError> {
    let progress = batch_progress(
        "ve-adrive sync delete-extra",
        delete_plan.len() as u64,
        progress_enabled,
    );
    let mut deleted = 0;
    sort_adrive_sync_delete_plan_bottom_up(&mut delete_plan);
    let (leaf_items, mut folder_items): (Vec<_>, Vec<_>) = delete_plan
        .into_iter()
        .partition(|item| item.operation != SYNC_DELETE_EXTRA_FOLDER);
    deleted += delete_adrive_sync_delete_item_group(
        client,
        leaf_items,
        batch_concurrency,
        &progress,
        report,
    )
    .await?;
    sort_adrive_sync_delete_plan_bottom_up(&mut folder_items);
    let mut index = 0;
    while index < folder_items.len() {
        let depth = adrive_sync_delete_item_depth(&folder_items[index]);
        let mut end = index + 1;
        while end < folder_items.len() && adrive_sync_delete_item_depth(&folder_items[end]) == depth
        {
            end += 1;
        }
        deleted += delete_adrive_sync_delete_item_group(
            client,
            folder_items[index..end].to_vec(),
            batch_concurrency,
            &progress,
            report,
        )
        .await?;
        index = end;
    }
    finish_progress(progress);
    Ok(deleted)
}

async fn delete_adrive_sync_delete_item_group(
    client: &IdsClient,
    items: Vec<BatchManifestItem>,
    batch_concurrency: usize,
    progress: &Option<ProgressBar>,
    report: &mut BatchReport,
) -> Result<usize, CliError> {
    let mut pending = items.into_iter();
    let mut in_flight = FuturesUnordered::new();
    let limit = batch_concurrency.max(1);
    let mut deleted = 0;
    loop {
        while in_flight.len() < limit {
            let Some(item) = pending.next() else {
                break;
            };
            in_flight.push(async move {
                let result = execute_adrive_sync_delete_item(client, &item).await;
                (item, result)
            });
        }
        let Some((item, result)) = in_flight.next().await else {
            break;
        };
        match result {
            Ok(()) => {
                deleted += 1;
                report.push_success(item.source, None, &item.operation);
            }
            Err(err) => {
                report.push_failure(item.source, None, &item.operation, err);
            }
        }
        tick_progress(progress);
    }
    Ok(deleted)
}

async fn execute_adrive_sync_delete_item(
    client: &IdsClient,
    item: &BatchManifestItem,
) -> Result<(), CliError> {
    if item.source.starts_with("adrive://") {
        if item.operation == SYNC_DELETE_EXTRA_FOLDER {
            let target = parse_adrive_uri(&item.source, false)?;
            client
                .delete_folder(&DeleteFolderInput::new(
                    &target.instance,
                    &target.space,
                    &normalize_adrive_folder_path(&target.path),
                ))
                .await
                .map(|_| ())
                .map_err(map_ids_error)
        } else {
            let target = parse_file_uri(&item.source)?;
            client
                .delete_file(&DeleteFileInput::new(
                    &target.instance,
                    &target.space,
                    &target.path,
                ))
                .await
                .map(|_| ())
                .map_err(map_ids_error)
        }
    } else {
        tokio::fs::remove_file(&item.source)
            .await
            .map_err(CliError::Io)
    }
}

fn adrive_sync_delete_item_depth(item: &BatchManifestItem) -> usize {
    adrive_path_depth(&adrive_sync_delete_sort_path(item))
}

fn collect_local_sync_desired_relative_paths(
    source_root: &Path,
    parent_prefix: Option<&str>,
    include: Option<&str>,
    exclude: Option<&str>,
) -> Result<SyncDesiredRelativePaths, CliError> {
    let mut desired = SyncDesiredRelativePaths::default();
    collect_local_sync_desired_relative_paths_inner(
        source_root,
        source_root,
        parent_prefix,
        include,
        exclude,
        &mut desired,
    )?;
    Ok(desired)
}

fn collect_local_sync_desired_relative_paths_inner(
    source_root: &Path,
    current: &Path,
    parent_prefix: Option<&str>,
    include: Option<&str>,
    exclude: Option<&str>,
    desired: &mut SyncDesiredRelativePaths,
) -> Result<(), CliError> {
    for entry in sorted_adrive_read_dir_entries(current)? {
        let path = entry.path();
        let file_type = entry.file_type()?;
        let relative = path.strip_prefix(source_root).map_err(|err| {
            CliError::ValidationError(format!("failed to derive relative path: {}", err))
        })?;
        let source_relative = normalize_local_relative_path(relative)?;
        let relative = prepend_adrive_parent_prefix(&source_relative, parent_prefix);
        if file_type.is_dir() {
            if path_matches_filters(&relative, include, exclude) {
                add_relative_folder_with_parents(&mut desired.folders, &relative);
            }
            collect_local_sync_desired_relative_paths_inner(
                source_root,
                &path,
                parent_prefix,
                include,
                exclude,
                desired,
            )?;
        } else if file_type.is_file() && path_matches_filters(&relative, include, exclude) {
            desired.files.insert(relative.clone());
            add_relative_file_parent_folders(&mut desired.folders, &relative);
        }
    }
    Ok(())
}

fn collect_remote_sync_desired_relative_paths(
    entries: ADriveListedEntries,
    source_prefix: &str,
    parent_prefix: Option<&str>,
    include: Option<&str>,
    exclude: Option<&str>,
) -> SyncDesiredRelativePaths {
    let mut desired = SyncDesiredRelativePaths::default();
    for file in entries.files {
        let relative =
            recursive_adrive_relative_path(&file.file_path, source_prefix, parent_prefix);
        if path_matches_filters(&relative, include, exclude) {
            desired.files.insert(relative.clone());
            add_relative_file_parent_folders(&mut desired.folders, &relative);
        }
    }
    for folder in entries.folders {
        let source_relative = remote_folder_relative_path(&folder.folder, source_prefix);
        let relative = prepend_adrive_parent_prefix(&source_relative, parent_prefix);
        if !relative.is_empty() && path_matches_filters(&relative, include, exclude) {
            add_relative_folder_with_parents(&mut desired.folders, &relative);
        }
    }
    desired
}

fn append_adrive_extra_folder_delete_items(
    plan: &mut Vec<BatchManifestItem>,
    folders: Vec<FolderInfo>,
    destination: &ParsedADriveUri,
    destination_prefix: &str,
    desired_folders: &BTreeSet<String>,
    include: Option<&str>,
    exclude: Option<&str>,
) {
    for folder in folders {
        let relative = remote_folder_relative_path(&folder.folder, destination_prefix);
        if relative.is_empty()
            || !path_matches_filters(&relative, include, exclude)
            || desired_folders.contains(&relative)
        {
            continue;
        }
        plan.push(adrive_remote_folder_manifest_item(
            SYNC_DELETE_EXTRA_FOLDER,
            &destination.instance,
            &destination.space,
            &folder.folder,
        ));
    }
}

fn normalize_relative_folder_path(path: &str) -> String {
    path.trim_matches('/').to_string()
}

fn remote_folder_relative_path(folder_path: &str, root_prefix: &str) -> String {
    let folder_path = normalize_relative_folder_path(folder_path);
    let root_prefix = normalize_relative_folder_path(root_prefix);
    if root_prefix.is_empty() {
        return folder_path;
    }
    // [Review Fix #6] Treat the listed destination root itself as an empty relative path.
    if folder_path == root_prefix {
        return String::new();
    }
    let root_prefix = format!("{root_prefix}/");
    folder_path
        .strip_prefix(&root_prefix)
        .unwrap_or(&folder_path)
        .to_string()
}

fn add_relative_file_parent_folders(folders: &mut BTreeSet<String>, relative_file: &str) {
    let relative_file = relative_file.trim_matches('/');
    let Some((parent, _)) = relative_file.rsplit_once('/') else {
        return;
    };
    add_relative_folder_with_parents(folders, parent);
}

fn add_relative_folder_with_parents(folders: &mut BTreeSet<String>, relative_folder: &str) {
    let relative_folder = normalize_relative_folder_path(relative_folder);
    if relative_folder.is_empty() {
        return;
    }
    let mut current = String::new();
    for segment in relative_folder
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        if !current.is_empty() {
            current.push('/');
        }
        current.push_str(segment);
        folders.insert(current.clone());
    }
}

fn sort_adrive_sync_delete_plan_bottom_up(items: &mut [BatchManifestItem]) {
    items.sort_by(|left, right| {
        let left_path = adrive_sync_delete_sort_path(left);
        let right_path = adrive_sync_delete_sort_path(right);
        adrive_path_depth(&right_path)
            .cmp(&adrive_path_depth(&left_path))
            .then_with(|| right_path.len().cmp(&left_path.len()))
            .then_with(|| left_path.cmp(&right_path))
    });
}

fn adrive_sync_delete_sort_path(item: &BatchManifestItem) -> String {
    if item.source.starts_with("adrive://") {
        return parse_adrive_uri(&item.source, false)
            .map(|target| target.path)
            .unwrap_or_else(|_| item.source.clone());
    }
    item.source.replace(std::path::MAIN_SEPARATOR, "/")
}

fn remote_relative_path(file_path: &str, root_prefix: &str) -> String {
    file_path
        .strip_prefix(root_prefix)
        .unwrap_or(file_path)
        .trim_start_matches('/')
        .to_string()
}

fn parse_columns(columns: Option<&str>) -> Option<&'static [&'static str]> {
    let columns = columns?.trim();
    if columns.is_empty() {
        return None;
    }
    let leaked = Box::leak(columns.to_string().into_boxed_str());
    let values = leaked
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.is_empty() {
        // [Review Fix #4] Empty user column input should fall back to default columns.
        None
    } else {
        Some(Box::leak(values.into_boxed_slice()))
    }
}

fn trim_folder_prefix(path: &str) -> String {
    if path.is_empty() || path.ends_with('/') {
        path.to_string()
    } else {
        format!("{path}/")
    }
}

fn resolve_hierarchical_target(
    uri: Option<&str>,
    instance: Option<&str>,
    space: Option<&str>,
    folder: Option<&str>,
    file: Option<&str>,
    allow_empty: bool,
) -> Result<ADriveTarget, CliError> {
    if let Some(uri) = uri {
        let parsed = parse_adrive_uri(uri, true)?;
        if parsed.space.is_empty() {
            return Ok(ADriveTarget::Instance {
                instance: parsed.instance,
            });
        }
        if parsed.path.is_empty() {
            return Ok(ADriveTarget::Space {
                instance: parsed.instance,
                space: parsed.space,
            });
        }
        return Ok(ADriveTarget::Path(parsed));
    }

    match (instance, space) {
        (None, None) if allow_empty => Ok(ADriveTarget::Instances),
        (None, _) => Err(CliError::ValidationError(
            "missing --instance: required when --space is provided".to_string(),
        )),
        (Some(instance), None) => Ok(ADriveTarget::Instance {
            instance: instance.to_string(),
        }),
        (Some(instance), Some(space)) => {
            let path = match (folder, file) {
                (Some(folder), Some(file)) => {
                    format!("{}/{}", folder.trim_end_matches('/'), file)
                }
                (Some(folder), None) => format!("{}/", folder.trim_end_matches('/')),
                (None, Some(file)) => file.to_string(),
                (None, None) => {
                    return Ok(ADriveTarget::Space {
                        instance: instance.to_string(),
                        space: space.to_string(),
                    });
                }
            };
            Ok(ADriveTarget::Path(ParsedADriveUri {
                instance: instance.to_string(),
                space: space.to_string(),
                path,
            }))
        }
    }
}

fn resolve_create_target(args: &CreateArgs) -> Result<ADriveCreateTarget, CliError> {
    let target = if let Some(uri) = args.path.as_deref() {
        let parsed = parse_adrive_uri(uri, true)?;
        if parsed.path.is_empty() {
            if parsed.space.is_empty() {
                ADriveCreateTarget::Instance {
                    name: parsed.instance,
                }
            } else {
                ADriveCreateTarget::Space {
                    instance: parsed.instance,
                    name: parsed.space,
                }
            }
        } else {
            return Err(CliError::ValidationError(
                "ve-adrive crt only creates instances or spaces; use ve-adrive mkdir for folders"
                    .to_string(),
            ));
        }
    } else {
        match (args.instance.as_deref(), args.space.as_deref()) {
            (Some(instance), Some(space)) => ADriveCreateTarget::Space {
                instance: instance.to_string(),
                name: space.to_string(),
            },
            (Some(instance), None) => ADriveCreateTarget::Instance {
                name: instance.to_string(),
            },
            (None, Some(_)) => {
                return Err(CliError::ValidationError(
                    "missing --instance: required when --space is provided".to_string(),
                ));
            }
            (None, None) => {
                return Err(CliError::ValidationError(
                    "ve-adrive crt requires adrive://instance-name, adrive://instance-id/space-name, or --instance".to_string(),
                ));
            }
        }
    };

    match &target {
        ADriveCreateTarget::Instance { name } => {
            if name.trim().is_empty() {
                return Err(CliError::ValidationError(
                    "ve-adrive crt requires a non-empty instance name".to_string(),
                ));
            }
            if args.index_enabled {
                return Err(CliError::ValidationError(
                    "--index-enabled is only valid when creating a space".to_string(),
                ));
            }
        }
        ADriveCreateTarget::Space { instance, name } => {
            if instance.trim().is_empty() || name.trim().is_empty() {
                return Err(CliError::ValidationError(
                    "ve-adrive crt requires non-empty instance and space names".to_string(),
                ));
            }
        }
    }
    Ok(target)
}

fn resolve_delete_target(args: &DeleteArgs) -> Result<ADriveTarget, CliError> {
    let target = resolve_hierarchical_target(
        args.path.as_deref(),
        args.instance.as_deref(),
        args.space.as_deref(),
        None,
        None,
        false,
    )?;
    match target {
        ADriveTarget::Instance { .. } | ADriveTarget::Space { .. } => Ok(target),
        ADriveTarget::Instances => Err(CliError::ValidationError(
            "ve-adrive del requires an instance or space target".to_string(),
        )),
        ADriveTarget::Path(_) => Err(CliError::ValidationError(
            "ve-adrive del only removes instances or spaces; use ve-adrive rm for files or folders"
                .to_string(),
        )),
    }
}

fn resolve_rm_target(args: &RmArgs) -> Result<ParsedADriveUri, CliError> {
    let target = resolve_hierarchical_target(
        args.path.as_deref(),
        args.instance.as_deref(),
        args.space.as_deref(),
        args.folder.as_deref(),
        args.file.as_deref(),
        true,
    )?;
    match target {
        ADriveTarget::Path(parsed) => Ok(parsed),
        ADriveTarget::Space { instance, space } => {
            if !args.recursive {
                return Err(CliError::ValidationError(
                    "ve-adrive rm requires a file or folder target; add --recursive to clear a space or use ve-adrive del to delete the space".to_string(),
                ));
            }
            if args.recursive_delete_mode == RecursiveDeleteMode::Direct {
                return Err(CliError::ValidationError(
                    "ve-adrive rm --recursive-delete-mode direct cannot target a space root; use bottom-up to clear space contents or ve-adrive del to delete the space".to_string(),
                ));
            }
            Ok(ParsedADriveUri {
                instance,
                space,
                path: String::new(),
            })
        }
        ADriveTarget::Instances | ADriveTarget::Instance { .. } => Err(CliError::ValidationError(
            "ve-adrive rm requires a file or folder target; use ve-adrive del for instances"
                .to_string(),
        )),
    }
}

// ─── Describe / Dry-Run ─────────────────────────────────────────────────────

fn describe_command(command: &ADriveCommand) -> CommandDescription {
    describe_high_level_command_path(&crate::cli::command_path(command)).unwrap_or_else(|| {
        CommandDescription {
            command: "ve-adrive".to_string(),
            layer: CommandLayer::HighLevel,
            description: "Unknown high-level command".to_string(),
            ..Default::default()
        }
    })
}

pub fn describe_high_level_command_path(command: &str) -> Option<CommandDescription> {
    match command {
        "ve-adrive cp" => Some(high_level_description(
            "ve-adrive cp",
            "put_file + get_file + copy_file",
            "Copy local files, ADrive files, or folders",
            RiskLevel::Medium,
            true,
        )),
        "ve-adrive mv" => Some(high_level_description(
            "ve-adrive mv",
            "rename_file + rename_folder + copy_file + delete_file",
            "Move files or folders by copy plus source delete",
            RiskLevel::Critical,
            false,
        )),
        "ve-adrive sync" => Some(high_level_description(
            "ve-adrive sync",
            "list_files + put_file + get_file + delete_file",
            "Synchronize source and destination incrementally",
            RiskLevel::Critical,
            false,
        )),
        "ve-adrive crt" => Some(high_level_description(
            "ve-adrive crt",
            "create_instance + create_space",
            "Create an instance or space",
            RiskLevel::Medium,
            false,
        )),
        "ve-adrive del" => Some(high_level_description(
            "ve-adrive del",
            "delete_instance + delete_space",
            "Delete an instance or space",
            RiskLevel::Critical,
            false,
        )),
        "ve-adrive rm" => Some(high_level_description(
            "ve-adrive rm",
            "delete_file + delete_folder + abort_multipart_upload",
            "Delete a file, folder, or recursively clear a space",
            RiskLevel::Critical,
            false,
        )),
        "ve-adrive ls" => Some(high_level_description(
            "ve-adrive ls",
            "get_instance + get_space + list_instances + list_spaces + list_files",
            "List instances, spaces, or files by target depth",
            RiskLevel::Low,
            true,
        )),
        "ve-adrive stat" => Some(high_level_description(
            "ve-adrive stat",
            "get_instance + get_space + head_file",
            "Show instance, space, file, or folder metadata",
            RiskLevel::Low,
            true,
        )),
        "ve-adrive du" => Some(high_level_description(
            "ve-adrive du",
            "list_files",
            "Calculate file size statistics for a folder",
            RiskLevel::Low,
            true,
        )),
        "ve-adrive find" => Some(high_level_description(
            "ve-adrive find",
            "list_files",
            "Find files by name, size, or mtime",
            RiskLevel::Low,
            true,
        )),
        "ve-adrive cat" => Some(high_level_description(
            "ve-adrive cat",
            "get_file",
            "Stream file content",
            RiskLevel::Low,
            true,
        )),
        "ve-adrive put" => Some(high_level_description(
            "ve-adrive put",
            "put_file + initiate_multipart_upload + upload_part + complete_multipart_upload + abort_multipart_upload",
            "Upload stdin to a file",
            RiskLevel::Medium,
            true,
        )),
        "ve-adrive mkdir" => Some(high_level_description(
            "ve-adrive mkdir",
            "create_folder",
            "Create a folder",
            RiskLevel::Medium,
            false,
        )),
        _ => None,
    }
}

fn high_level_description(
    command: &'static str,
    api: &'static str,
    description: &'static str,
    risk_level: RiskLevel,
    supports_pipe: bool,
) -> CommandDescription {
    let low_level_apis: Vec<String> = find_capability(command)
        .map(|row| row.api_actions.iter().map(|api| api.to_string()).collect())
        .unwrap_or_else(|| {
            api.split('+')
                .map(str::trim)
                .filter(|api| !api.is_empty())
                .map(str::to_string)
                .collect()
        });
    CommandDescription {
        command: command.to_string(),
        layer: CommandLayer::HighLevel,
        api: Some(api.to_string()),
        description: description.to_string(),
        risk_level,
        supports_dry_run: true,
        supports_pipe,
        parameters: command_parameters(command),
        scenario_routing: Some(high_level_scenario_routing(command)),
        related_commands: Some(RelatedCommands {
            high_level: None,
            low_level: Some(low_level_apis.clone()),
        }),
        low_level_apis: Some(low_level_apis.clone()),
        wraps_apis: Some(low_level_apis),
        output_filter_examples: Some(high_level_filter_examples(command)),
        shell_quoting_tips: Some(high_level_quoting_tips()),
    }
}

fn command_parameters(command: &str) -> Option<Vec<CommandParameter>> {
    let row = find_capability(command)?;
    Some(row.parameters.iter().map(to_command_parameter).collect())
}

fn to_command_parameter(parameter: &RegistryParameter) -> CommandParameter {
    let location = match parameter.name {
        "source" | "destination" | "path" => ParameterLocation::Path,
        _ => ParameterLocation::Flag,
    };
    CommandParameter {
        name: parameter.name.to_string(),
        location,
        required: parameter.required,
        description: parameter.description.to_string(),
        schema: Some(json!({ "type": parameter_schema_type(parameter.name) })),
    }
}

fn parameter_schema_type(name: &str) -> &'static str {
    if matches!(
        name,
        "by-name"
            | "recursive"
            | "include-parent"
            | "force"
            | "delete"
            | "size-only"
            | "exact-timestamps"
            | "include-uploads"
            | "index-enabled"
            | "parents"
            | "no-clobber"
            | "no-manifest"
            | "report-failures-only"
            | "progress"
            | "no-progress"
            | "list-echo"
            | "no-list-echo"
            | "human-readable"
            | "cost"
    ) {
        "boolean"
    } else if matches!(
        name,
        "max-keys"
            | "max-depth"
            | "top-k"
            | "batch-concurrency"
            | "list-concurrency"
            | "multipart-concurrency"
    ) {
        "integer"
    } else {
        "string"
    }
}

fn high_level_scenario_routing(command: &str) -> HashMap<String, String> {
    let mut routing = HashMap::new();
    routing.insert(
        "target_resolution".to_string(),
        "accept adrive://instance/space/path URI or --instance/--space/--folder/--file flags; --by-name resolves instance/space names to IDs before execution"
            .to_string(),
    );
    routing.insert(
        "dry_run".to_string(),
        "returns a deterministic plan without mutating local files or ADrive resources".to_string(),
    );
    routing.insert(
        "output".to_string(),
        "success and failure paths use Envelope plus --query and multi-format rendering"
            .to_string(),
    );
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
        "stable task fingerprint plus atomic lock when the command supports checkpoint state"
            .to_string(),
    );
    routing.insert(
        "low_level_boundary".to_string(),
        "ADrive Low-Level CLI is not implemented; High-Level commands wrap IDS API actions directly"
            .to_string(),
    );
    // [Review Fix #RebaseADriveDescribe] Rebase dropped ADrive/TOS describe
    // parity for composite high-level commands; restore explicit routing hints.
    if command == "ve-adrive ls" {
        routing.insert(
            "target_matrix".to_string(),
            "no target -> list instances; instance target -> list spaces; instance/space[/folder] target -> list files"
                .to_string(),
        );
        routing.insert(
            "output_shapes".to_string(),
            "instance listing returns data.instances; space listing returns data.spaces; JSON file listing returns raw data.files/data.folders; table/csv render a synthesized typed row view"
                .to_string(),
        );
    } else if command == "ve-adrive put" {
        routing.insert(
            "input".to_string(),
            "reads stdin and writes it to exactly one ADrive file target; pipe-friendly for cat | gzip | put"
                .to_string(),
        );
        routing.insert(
            "multipart".to_string(),
            "stdin input at or above --multipart-threshold uses initiate_multipart_upload + upload_part + complete_multipart_upload; failures abort_multipart_upload"
                .to_string(),
        );
    }
    if matches!(command, "ve-adrive mv" | "ve-adrive del" | "ve-adrive rm") {
        routing.insert(
            "destructive_guard".to_string(),
            "critical delete paths require --force and, in non-interactive shells, exact --confirm <deleted-source-or-target>"
                .to_string(),
        );
        if command == "ve-adrive rm" {
            routing.insert(
                "target_scope".to_string(),
                "rm accepts file/folder targets only; use ve-adrive del for instance or space deletion"
                    .to_string(),
            );
            routing.insert(
                "recursive_delete".to_string(),
                "bottom-up mode lists children then deletes files before folders; direct mode asks the service to delete the folder target"
                    .to_string(),
            );
        }
    } else if command == "ve-adrive sync" {
        routing.insert(
            "destructive_guard".to_string(),
            "sync --delete is critical: interactive shells may confirm or use --force; non-interactive shells require --force plus exact --confirm <destination>"
                .to_string(),
        );
    } else if matches!(command, "ve-adrive mv" | "ve-adrive sync") {
        routing.insert(
            "destructive_guard".to_string(),
            "destructive delete or overwrite paths require --force, --yes, or confirmation"
                .to_string(),
        );
    }
    routing
}

fn high_level_filter_examples(command: &str) -> Vec<String> {
    let public_command = public_adrive_command(command);
    let mut examples = vec![
        format!("{public_command} ... --output json | jq '.data'"),
        format!("{public_command} ... --query 'data'"),
    ];
    match command {
        "ve-adrive ls" => {
            examples.push(public_adrive_command(
                "ve-adrive ls adrive://inst/space/docs/ --query 'data.files[*].file_path'",
            ));
            examples.push(public_adrive_command(
                "ve-adrive ls --instance inst --space space --query 'pagination.next_marker'",
            ));
        }
        "ve-adrive cp" | "ve-adrive mv" | "ve-adrive sync" => {
            examples.push(format!(
                "{public_command} ... --dry-run --query 'data.plan'"
            ));
            examples.push(format!(
                "{public_command} ... --dry-run --query 'data.impact'"
            ));
        }
        "ve-adrive rm" => {
            examples.push(public_adrive_command(
                "ve-adrive rm adrive://inst/space/docs/ --dry-run --query 'data.impact'",
            ));
        }
        "ve-adrive crt" | "ve-adrive del" => {
            examples.push(format!(
                "{public_command} ... --dry-run --query 'data.request_plan'"
            ));
        }
        "ve-adrive cat" => {
            examples.push(public_adrive_command(
                "ve-adrive cat adrive://inst/space/docs/a.txt --output json --query 'data.content'",
            ));
        }
        "ve-adrive put" => {
            examples.push(public_adrive_example(
                "ve-adrive cat adrive://inst/space/docs/a.txt | gzip | ve-adrive put adrive://inst/space/docs/a.txt.gz",
            ));
        }
        _ => {}
    }
    examples
}

fn public_adrive_command(command: &str) -> String {
    let prefix =
        std::env::var(ADRIVE_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "ve-adrive".to_string());
    command
        .strip_prefix("ve-adrive ")
        .map(|suffix| format!("{prefix} {suffix}"))
        .unwrap_or_else(|| command.to_string())
}

fn public_adrive_example(example: &str) -> String {
    let prefix =
        std::env::var(ADRIVE_EXAMPLE_PREFIX_ENV).unwrap_or_else(|_| "ve-adrive".to_string());
    let with_public_pipeline = example.replace(" | ve-adrive ", &format!(" | {prefix} "));
    public_adrive_command(&with_public_pipeline)
}

fn high_level_quoting_tips() -> Vec<String> {
    vec![
        "Quote ADrive paths that contain spaces or shell metacharacters: ve-adrive cp 'adrive://inst/space/path with space.txt' ./out.txt".to_string(),
        "JMESPath literals inside --query use backticks; keep the expression inside single quotes in POSIX shells.".to_string(),
        "Use --output json when piping ADrive output into jq or another parser.".to_string(),
    ]
}

/// Maximum number of objects to scan during dry-run impact estimation.
const MAX_PREVIEW_OBJECTS: u64 = 10_000;

/// Estimated milliseconds per delete operation for duration estimation.
const ESTIMATED_DELETE_MS_PER_OBJECT: u64 = 5;

/// Determines whether a command needs a real listing to estimate impact.
fn command_needs_impact_listing(cmd_name: &str, destructive: bool) -> bool {
    matches!(cmd_name, "ve-adrive rm") || (cmd_name == "ve-adrive sync" && destructive)
}

fn cp_checkpoint_scope(args: &CpArgs) -> &'static str {
    if !args.checkpoint {
        return "disabled";
    }
    match (
        args.source.starts_with("adrive://"),
        args.destination.starts_with("adrive://"),
        args.recursive,
    ) {
        (_, _, true) => "recursive_item_manifest",
        (false, true, false) => "multipart_upload",
        (true, false, false) => "range_download",
        (true, true, false) => "remote_copy_not_supported",
        (false, false, false) => "invalid_transfer",
    }
}

/// Estimate the impact of a destructive ADrive command by listing the target
/// path and counting affected objects/bytes (capped at [`MAX_PREVIEW_OBJECTS`]).
///
/// Returns `None` for non-destructive commands or when the client cannot be
/// constructed (e.g. missing credentials in dry-run-only scenarios).
async fn compute_dry_run_impact(
    global: &GlobalArgs,
    command: &ADriveCommand,
    cmd_name: &str,
    destructive: bool,
) -> Option<Impact> {
    if !command_needs_impact_listing(cmd_name, destructive) {
        return None;
    }

    // Resolve the target path that will be affected.
    let (instance, space, path) = match command {
        ADriveCommand::Rm(args) => {
            let target = resolve_rm_target(args).ok()?;
            (target.instance, target.space, target.path)
        }
        ADriveCommand::Sync(args) => {
            // For sync --delete, the destination is the path that gets deletions.
            let parsed = parse_adrive_uri(&args.destination, true).ok()?;
            if parsed.space.is_empty() {
                return None;
            }
            (parsed.instance, parsed.space, parsed.path)
        }
        _ => return None,
    };

    // Build the IDS client; gracefully return None if credentials are absent.
    let client = build_ids_client(global).ok()?;

    let listed = match list_all_files_and_folders_hierarchical(
        &client,
        &instance,
        &space,
        &path,
        DEFAULT_LIST_CONCURRENCY,
    )
    .await
    {
        Ok(listed) => listed,
        Err(_) => return None,
    };
    let mut affected_objects: u64 = 0;
    let mut affected_bytes: u64 = 0;
    let mut scanned_count: u64 = 0;
    let count_folders = matches!(
        command,
        ADriveCommand::Rm(args)
            if args.recursive && args.recursive_delete_mode == RecursiveDeleteMode::BottomUp
    );

    for file in &listed.files {
        scanned_count += 1;
        if scanned_count <= MAX_PREVIEW_OBJECTS {
            affected_objects += 1;
            affected_bytes = affected_bytes.saturating_add(file.size.max(0) as u64);
        }
    }
    if count_folders {
        for _folder in &listed.folders {
            scanned_count += 1;
            if scanned_count <= MAX_PREVIEW_OBJECTS {
                affected_objects += 1;
            }
        }
    }
    let preview_truncated = scanned_count > MAX_PREVIEW_OBJECTS;

    // [Review Fix #7] `sync --delete` is a delete-class operation; dry-run
    // impact must report the same critical risk level enforced at execution.
    let risk_level = if destructive { "critical" } else { "medium" };

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
        risk_level: risk_level.to_string(),
        estimated_duration,
        scanned_count: Some(scanned_count),
        preview_truncated: Some(preview_truncated),
    })
}

async fn build_plan(
    global: &GlobalArgs,
    command: &ADriveCommand,
) -> Result<serde_json::Value, CliError> {
    // [Review Fix #ADrive-CheckpointPlan] Dry-run must describe checkpoint and
    // progress behavior for the same flags that real execution consumes.
    let (
        cmd_name,
        description,
        mutates,
        destructive,
        recursive,
        report_path,
        checkpoint_enabled,
        checkpoint_dir,
        checkpoint_scope,
        progress_plan,
        list_echo_plan,
        request_plan,
    ) = match command {
        ADriveCommand::Cp(args) => {
            let destination = display_single_transfer_destination(
                &args.source,
                &args.destination,
                args.recursive,
            )?;
            (
                "ve-adrive cp",
                format!("Copy {} -> {}", args.source, destination),
                true,
                false,
                args.recursive,
                args.report_path.as_deref(),
                args.checkpoint,
                args.checkpoint_dir.as_deref(),
                cp_checkpoint_scope(args),
                resolve_progress_plan(global, args.progress, args.no_progress)?,
                resolve_list_echo_plan(global, args.list_echo, args.no_list_echo),
                vec![
                    "put_file/get_file/copy_file",
                    "multipart upload when --checkpoint or file > 64MiB",
                    "range get_file when resuming downloads",
                    "list_files when --recursive",
                    "abort_multipart_upload for non-checkpoint multipart upload failures",
                ],
            )
        }
        ADriveCommand::Mv(args) => {
            let destination = display_single_transfer_destination(
                &args.source,
                &args.destination,
                args.recursive,
            )?;
            (
                "ve-adrive mv",
                format!("Move {} -> {}", args.source, destination),
                true,
                true,
                args.recursive,
                args.report_path.as_deref(),
                false,
                args.checkpoint_dir.as_deref(),
                "not_enabled_for_mv",
                resolve_progress_plan(global, args.progress, args.no_progress)?,
                resolve_list_echo_plan(global, args.list_echo, args.no_list_echo),
                vec![
                    "rename_file/rename_folder for same space",
                    "copy + delete otherwise",
                ],
            )
        }
        ADriveCommand::Sync(args) => (
            "ve-adrive sync",
            format!("Sync {} -> {}", args.source, args.destination),
            true,
            args.delete,
            true,
            args.report_path.as_deref(),
            false,
            args.checkpoint_dir.as_deref(),
            "not_enabled_for_sync",
            resolve_progress_plan(global, args.progress, args.no_progress)?,
            resolve_list_echo_plan(global, args.list_echo, args.no_list_echo),
            vec![
                "list_files",
                "put_file/get_file/copy_file",
                "delete_file/delete_folder when --delete",
            ],
        ),
        ADriveCommand::Crt(args) => {
            let target = resolve_create_target(args)?;
            let description = match target {
                ADriveCreateTarget::Instance { name } => {
                    format!("Create instance adrive://{name}")
                }
                ADriveCreateTarget::Space { instance, name } => {
                    format!("Create space adrive://{instance}/{name}")
                }
            };
            (
                "ve-adrive crt",
                description,
                true,
                false,
                false,
                None,
                false,
                None,
                "none",
                output_not_applicable_plan(),
                output_not_applicable_plan(),
                vec!["create_instance/create_space"],
            )
        }
        ADriveCommand::Del(args) => {
            let target = resolve_delete_target(args)?;
            (
                "ve-adrive del",
                format!("Delete {}", target.display()),
                true,
                true,
                false,
                None,
                false,
                None,
                "none",
                output_not_applicable_plan(),
                output_not_applicable_plan(),
                vec!["delete_instance/delete_space"],
            )
        }
        ADriveCommand::Rm(args) => {
            let target = resolve_rm_target(args)?;
            let mut request_plan = if args.recursive
                && args.recursive_delete_mode == RecursiveDeleteMode::BottomUp
            {
                vec![
                    "list_files",
                    "delete_file leaf entries",
                    "delete_folder bottom-up",
                ]
            } else if args.recursive && args.recursive_delete_mode == RecursiveDeleteMode::Direct {
                vec!["delete_folder direct"]
            } else {
                vec!["delete_file/delete_folder"]
            };
            if args.include_uploads {
                request_plan.push("abort_multipart_upload for matching checkpointed uploads");
            }
            (
                "ve-adrive rm",
                format!("Delete {}", format_target(&target)),
                true,
                true,
                args.recursive,
                args.report_path.as_deref(),
                args.include_uploads,
                args.checkpoint_dir.as_deref(),
                if args.include_uploads {
                    "rm_include_uploads"
                } else {
                    "none"
                },
                resolve_progress_plan(global, args.progress, args.no_progress)?,
                resolve_list_echo_plan(global, args.list_echo, args.no_list_echo),
                request_plan,
            )
        }
        ADriveCommand::Ls(args) => {
            validate_adrive_ls_max_keys(args.max_keys)?;
            (
                "ve-adrive ls",
                "List instances, spaces, files, or folders".to_string(),
                false,
                false,
                false,
                None,
                false,
                None,
                "none",
                output_not_applicable_plan(),
                output_not_applicable_plan(),
                vec!["get_instance/get_space/list_instances/list_spaces/list_files"],
            )
        }
        ADriveCommand::Stat(_) => (
            "ve-adrive stat",
            "Get instance, space, file, or folder metadata".to_string(),
            false,
            false,
            false,
            None,
            false,
            None,
            "none",
            output_not_applicable_plan(),
            output_not_applicable_plan(),
            vec!["get_instance/get_space/head_file"],
        ),
        ADriveCommand::Du(args) => (
            "ve-adrive du",
            "Calculate disk usage from listed files".to_string(),
            false,
            false,
            true,
            None,
            false,
            None,
            "none",
            output_not_applicable_plan(),
            resolve_traversal_echo_plan(
                global,
                args.list_echo,
                args.no_list_echo,
                args.progress,
                args.no_progress,
            ),
            vec!["list_files"],
        ),
        ADriveCommand::Find(args) => (
            "ve-adrive find",
            "Search files".to_string(),
            false,
            false,
            false,
            None,
            false,
            None,
            "none",
            output_not_applicable_plan(),
            resolve_traversal_echo_plan(
                global,
                args.list_echo,
                args.no_list_echo,
                args.progress,
                args.no_progress,
            ),
            vec!["list_files"],
        ),
        ADriveCommand::Cat(_) => (
            "ve-adrive cat",
            "Read file content".to_string(),
            false,
            false,
            false,
            None,
            false,
            None,
            "none",
            output_not_applicable_plan(),
            output_not_applicable_plan(),
            vec!["get_file"],
        ),
        ADriveCommand::Put(args) => {
            let target = resolve_put_file_target(args)?;
            (
                "ve-adrive put",
                format!("Upload stdin -> {}", format_target(&target)),
                true,
                false,
                false,
                None,
                false,
                None,
                "none",
                resolve_progress_plan(global, args.progress, args.no_progress)?,
                output_not_applicable_plan(),
                vec![
                    "put_file when stdin is below --multipart-threshold",
                    "initiate_multipart_upload + upload_part + complete_multipart_upload when stdin reaches --multipart-threshold",
                    "abort_multipart_upload when stdin multipart upload fails before completion",
                    "CRC64 validation when the service returns checksum headers",
                ],
            )
        }
        ADriveCommand::Mkdir(args) => (
            "ve-adrive mkdir",
            format!(
                "Create folder {}",
                args.path
                    .as_deref()
                    .or(args.folder.as_deref())
                    .unwrap_or("<missing>")
            ),
            true,
            false,
            args.parents,
            None,
            false,
            None,
            "none",
            output_not_applicable_plan(),
            output_not_applicable_plan(),
            if args.parents {
                vec!["create_folder for each parent prefix"]
            } else {
                vec!["create_folder"]
            },
        ),
        _ => (
            "ve-adrive",
            "Unknown command".to_string(),
            false,
            false,
            false,
            None,
            false,
            None,
            "none",
            output_not_applicable_plan(),
            output_not_applicable_plan(),
            vec![],
        ),
    };

    let impact = compute_dry_run_impact(global, command, cmd_name, destructive).await;

    let mut plan = json!({
        "command": cmd_name,
        "dry_run": true,
        "execution_status": "planned_not_executed",
        "description": description,
        "summary": {
            "mutates": mutates,
            "destructive": destructive,
            "recursive": recursive,
            "requires_force": destructive,
        },
        "request_plan": request_plan,
        "report": {
            "enabled": report_path.is_some(),
            "path": report_path,
            "format": "csv",
        },
        "checkpoint": {
            "enabled": checkpoint_enabled,
            "directory": checkpoint_dir.unwrap_or(ADRIVE_DEFAULT_CHECKPOINT_DIR),
            "identity": "stable_task_fingerprint",
            "scope": checkpoint_scope,
        },
        "progress": {
            "enabled": progress_plan.enabled,
            "disabled_reason": progress_plan.disabled_reason,
            "render_to": "stderr",
            "mode": "batch-summary",
        },
        "list_echo": {
            "enabled": list_echo_plan.enabled,
            "disabled_reason": list_echo_plan.disabled_reason,
            "render_to": "stderr",
        },
        "consistency_guards": {
            "side_effect_free": true,
            "force_required_for_destructive": destructive,
        },
    });

    if let Some(ref imp) = impact {
        plan["impact"] = serde_json::to_value(imp).unwrap_or_else(|_| serde_json::Value::Null);
    }
    if let ADriveCommand::Rm(args) = command {
        plan["summary"]["recursive_delete_mode"] =
            json!(recursive_delete_mode_name(args.recursive_delete_mode));
    }

    Ok(plan)
}

fn format_target(target: &ParsedADriveUri) -> String {
    if target.space.is_empty() {
        format!("adrive://{}", target.instance)
    } else if target.path.is_empty() {
        format!("adrive://{}/{}", target.instance, target.space)
    } else {
        format!(
            "adrive://{}/{}/{}",
            target.instance, target.space, target.path
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_file_info(path: &str, file_type: &str) -> FileInfo {
        FileInfo {
            instance_id: String::new(),
            space_id: String::new(),
            file_path: path.to_string(),
            file_type: file_type.to_string(),
            storage_class: String::new(),
            meta: HashMap::new(),
            hash_crc64_ecma: 0,
            size: 0,
            etag: String::new(),
            created_at: 0,
            updated_at: 7,
        }
    }

    fn test_folder_info(path: &str) -> FolderInfo {
        FolderInfo {
            folder: path.to_string(),
            updated_at: 7,
        }
    }

    fn sync_delete_item(source: &str) -> BatchManifestItem {
        BatchManifestItem {
            operation: SYNC_DELETE_EXTRA_FILE.to_string(),
            source: source.to_string(),
            destination: None,
            size: 0,
            etag: None,
            crc64: None,
        }
    }

    fn sync_delete_folder_item(source: &str) -> BatchManifestItem {
        BatchManifestItem {
            operation: SYNC_DELETE_EXTRA_FOLDER.to_string(),
            source: source.to_string(),
            destination: None,
            size: 0,
            etag: None,
            crc64: None,
        }
    }

    fn stat_args(path: Option<&str>, instance: Option<&str>, space: Option<&str>) -> StatArgs {
        StatArgs {
            path: path.map(ToString::to_string),
            by_name: false,
            instance: instance.map(ToString::to_string),
            space: space.map(ToString::to_string),
            folder: None,
            file: None,
        }
    }

    fn du_args(max_depth: Option<u32>) -> DuArgs {
        DuArgs {
            path: Some("adrive://inst/space/docs/".to_string()),
            by_name: false,
            instance: None,
            space: None,
            folder: None,
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

    fn rm_args(path: Option<&str>, recursive: bool, mode: RecursiveDeleteMode) -> RmArgs {
        RmArgs {
            path: path.map(ToString::to_string),
            by_name: false,
            instance: None,
            space: None,
            folder: None,
            file: None,
            recursive,
            recursive_delete_mode: mode,
            force: true,
            include_uploads: false,
            checkpoint_dir: None,
            report_path: None,
            report_failures_only: false,
            manifest_path: None,
            no_manifest: false,
            batch_concurrency: None,
            list_concurrency: None,
            include: None,
            exclude: None,
            list_echo: false,
            no_list_echo: false,
            progress: false,
            no_progress: true,
        }
    }

    fn upload_checkpoint(file_path: &str, upload_id: Option<&str>) -> UploadCheckpoint {
        UploadCheckpoint {
            instance: "inst".to_string(),
            space: "space".to_string(),
            file_path: file_path.to_string(),
            source_path: "/local/source.bin".to_string(),
            file_size: 1,
            part_size: ADRIVE_MULTIPART_PART_SIZE,
            upload_id: upload_id.map(ToString::to_string),
            completed_parts: Vec::new(),
        }
    }

    #[test]
    fn recursive_rm_treats_file_like_path_as_folder_target() {
        let recursive_args = rm_args(
            Some("adrive://inst/space/docs/a"),
            true,
            RecursiveDeleteMode::BottomUp,
        );
        let target = resolve_rm_target(&recursive_args).expect("recursive folder target");

        assert_eq!(target.path, "docs/a");
        assert!(adrive_rm_should_delete_folder(&recursive_args, &target));

        let non_recursive_args = rm_args(
            Some("adrive://inst/space/docs/a"),
            false,
            RecursiveDeleteMode::BottomUp,
        );
        let target = resolve_rm_target(&non_recursive_args).expect("single file-like target");

        assert!(!adrive_rm_should_delete_folder(
            &non_recursive_args,
            &target
        ));
    }

    #[test]
    fn mv_directory_source_requires_recursive() {
        let err =
            ensure_adrive_mv_folder_requires_recursive("adrive://inst/space/docs/a", true, false)
                .expect_err("folder move should require recursive");
        assert!(err.to_string().contains("requires --recursive"));

        ensure_adrive_mv_folder_requires_recursive("adrive://inst/space/docs/a", true, true)
            .expect("recursive folder move is allowed");
        ensure_adrive_mv_folder_requires_recursive("adrive://inst/space/docs/a.txt", false, false)
            .expect("single file move is allowed");
    }

    #[test]
    fn recursive_mv_folder_paths_do_not_require_head_metadata() {
        let source = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: "docs/src".to_string(),
        };
        let destination = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: "backup".to_string(),
        };

        assert_eq!(
            adrive_recursive_mv_source_folder_path(&source).expect("source folder"),
            "docs/src"
        );
        assert_eq!(
            adrive_recursive_mv_destination_folder_path(
                "adrive://inst/space/docs/src",
                &destination,
                false,
            )
            .expect("destination folder"),
            "backup"
        );
        assert_eq!(
            adrive_recursive_mv_destination_folder_path(
                "adrive://inst/space/docs/src",
                &destination,
                true,
            )
            .expect("destination folder with parent"),
            "backup/src"
        );

        let root = ParsedADriveUri {
            path: String::new(),
            ..source
        };
        let err =
            adrive_recursive_mv_source_folder_path(&root).expect_err("space root is not a folder");
        assert!(err.to_string().contains("not a space root"));
    }

    #[test]
    fn recursive_include_parent_helpers_add_source_segment() {
        assert_eq!(
            recursive_adrive_source_parent_prefix("adrive://inst/space/docs/src/", true)
                .expect("remote parent")
                .as_deref(),
            Some("src")
        );
        assert_eq!(
            recursive_adrive_relative_path("docs/src/a.txt", "docs/src/", Some("src")),
            "src/a.txt"
        );
        assert_eq!(
            prepend_adrive_parent_prefix("nested/a.txt", Some("src")),
            "src/nested/a.txt"
        );
        assert!(
            recursive_adrive_source_parent_prefix("adrive://inst/space/docs/src/", false)
                .expect("disabled parent")
                .is_none()
        );
    }

    #[test]
    fn sync_desired_paths_include_parent_for_local_and_remote_sources() {
        let root =
            std::env::temp_dir().join(format!("adrive-sync-include-parent-{}", std::process::id()));
        let source = root.join("src");
        std::fs::create_dir_all(source.join("nested")).expect("create source dirs");
        std::fs::write(source.join("nested").join("a.txt"), "demo").expect("write source file");

        let local_desired =
            collect_local_sync_desired_relative_paths(&source, Some("src"), None, None)
                .expect("local desired paths");
        assert!(local_desired.files.contains("src/nested/a.txt"));
        assert!(local_desired.folders.contains("src"));
        assert!(local_desired.folders.contains("src/nested"));

        let remote_desired = collect_remote_sync_desired_relative_paths(
            ADriveListedEntries {
                files: vec![test_file_info("docs/src/nested/a.txt", "file")],
                folders: vec![test_folder_info("docs/src/nested")],
            },
            "docs/src/",
            Some("src"),
            None,
            None,
        );
        assert!(remote_desired.files.contains("src/nested/a.txt"));
        assert!(remote_desired.folders.contains("src"));
        assert!(remote_desired.folders.contains("src/nested"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn prevalidate_stat_accepts_instance_only_uri() {
        let command = ADriveCommand::Stat(stat_args(Some("adrive://inst"), None, None));

        assert!(prevalidate_command(&command).is_ok());
    }

    #[test]
    fn prevalidate_stat_accepts_space_uri() {
        let command = ADriveCommand::Stat(stat_args(Some("adrive://inst/space"), None, None));

        assert!(prevalidate_command(&command).is_ok());
    }

    #[test]
    fn prevalidate_stat_accepts_instance_flag_without_space() {
        let command = ADriveCommand::Stat(stat_args(None, Some("inst"), None));

        assert!(prevalidate_command(&command).is_ok());
    }

    #[test]
    fn adrive_file_type_output_falls_back_to_file() {
        let file = test_file_info("docs/a.txt", "");

        assert_eq!(adrive_file_type_for_output(&file), "file");
    }

    #[test]
    fn adrive_file_type_output_preserves_service_value() {
        let file = test_file_info("docs/a.txt", "text");

        assert_eq!(adrive_file_type_for_output(&file), "text");
    }

    #[test]
    fn adrive_du_payload_defaults_to_aggregate_fields() {
        let mut accumulator = DuAccumulator::new(2);
        let file = FileInfo {
            instance_id: String::new(),
            space_id: String::new(),
            file_path: "docs/a/file.log".to_string(),
            file_type: String::new(),
            size: 512,
            updated_at: 1,
            storage_class: "STANDARD".to_string(),
            meta: HashMap::new(),
            hash_crc64_ecma: 0,
            etag: String::new(),
            created_at: 0,
        };
        accumulator.record_adrive_file(&file, "inst", "space", "docs/", None);
        accumulator.record_folder_prefix("docs/a/");
        let target = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: "docs/".to_string(),
        };
        let payload = adrive_du_output_payload(
            &GlobalArgs::default(),
            &target,
            &accumulator,
            &du_args(None),
            None,
            None,
            DEFAULT_LIST_CONCURRENCY,
        );

        assert_eq!(payload["files"], 1);
        assert_eq!(payload["folders"], 1);
        assert_eq!(payload["total_bytes"], 512);
        assert_eq!(payload["storage_classes"]["STANDARD"]["file_count"], 1);
        assert!(payload.get("diagnostics").is_none());
        assert!(payload.get("file_types").is_none());
        assert!(payload.get("directories").is_none());
        assert!(payload.get("size_histogram").is_none());
        assert!(payload.get("largest_files").is_none());
        assert!(payload.get("oldest_files").is_none());
        assert!(payload.get("traversal").is_none());
        assert!(payload.get("groups").is_none());
        assert!(payload.get("cost").is_none());
        assert!(payload.get("manifest_path").is_none());
        assert!(payload.get("human_readable").is_none());
    }

    #[test]
    fn adrive_du_page_dedupes_folder_rows_and_file_markers() {
        let (files, folder_prefixes) = dedupe_adrive_du_page_entries(
            vec![
                test_file_info("docs/", "folder"),
                test_file_info("docs/a/", "file"),
                test_file_info("docs/a/file.txt", "file"),
            ],
            vec![
                test_folder_info("docs/"),
                test_folder_info("docs/a/"),
                test_folder_info("docs/a"),
            ],
            "docs/",
        );

        assert_eq!(
            files
                .iter()
                .map(|file| file.file_path.as_str())
                .collect::<Vec<_>>(),
            vec!["docs/a/file.txt"]
        );
        assert_eq!(folder_prefixes, vec!["docs/a/"]);
    }

    #[test]
    fn adrive_du_accumulator_dedupes_folder_prefixes_across_merges() {
        let mut left = DuAccumulator::new(2);
        let mut right = DuAccumulator::new(2);

        left.record_folder_prefix("docs/a/");
        left.record_folder_prefix("docs/a");
        right.record_folder_prefix("docs/a/");
        right.record_folder_prefix("docs/b/");
        left.merge(right);

        assert_eq!(left.folder_count, 2);
    }

    #[test]
    fn adrive_du_payload_verbose_exposes_diagnostics() {
        let mut accumulator = DuAccumulator::new(2);
        let file = FileInfo {
            instance_id: String::new(),
            space_id: String::new(),
            file_path: "docs/a/file.log".to_string(),
            file_type: String::new(),
            size: 512,
            updated_at: 1,
            storage_class: "STANDARD".to_string(),
            meta: HashMap::new(),
            hash_crc64_ecma: 0,
            etag: String::new(),
            created_at: 0,
        };
        accumulator.record_adrive_file(&file, "inst", "space", "docs/", Some(1));
        let target = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: "docs/".to_string(),
        };
        let mut global = GlobalArgs::default();
        global.verbose = true;
        let payload = adrive_du_output_payload(
            &global,
            &target,
            &accumulator,
            &du_args(Some(1)),
            None,
            Some("/tmp/adrive-du.json"),
            DEFAULT_LIST_CONCURRENCY,
        );

        assert!(payload.get("groups").is_some());
        assert_eq!(payload["manifest_path"], "/tmp/adrive-du.json");
        assert_eq!(payload["diagnostics"]["file_types"]["log"]["file_count"], 1);
        assert_eq!(
            payload["diagnostics"]["largest_files"][0]["file_path"],
            "docs/a/file.log"
        );
        assert_eq!(
            payload["diagnostics"]["traversal"]["prefix_concurrency"],
            DEFAULT_LIST_CONCURRENCY
        );
    }

    #[test]
    fn resolve_rm_target_accepts_recursive_space_root() {
        let args = rm_args(
            Some("adrive://inst/space"),
            true,
            RecursiveDeleteMode::BottomUp,
        );
        let target = resolve_rm_target(&args).expect("recursive root space target");

        assert_eq!(target.instance, "inst");
        assert_eq!(target.space, "space");
        assert_eq!(target.path, "");
        assert_eq!(format_target(&target), "adrive://inst/space");
    }

    #[test]
    fn resolve_rm_target_rejects_non_recursive_space_root() {
        let args = rm_args(
            Some("adrive://inst/space"),
            false,
            RecursiveDeleteMode::BottomUp,
        );
        let err = resolve_rm_target(&args).expect_err("space root requires recursive");

        assert!(err.to_string().contains("add --recursive to clear a space"));
    }

    #[test]
    fn resolve_rm_target_rejects_direct_space_root() {
        let args = rm_args(
            Some("adrive://inst/space"),
            true,
            RecursiveDeleteMode::Direct,
        );
        let err = resolve_rm_target(&args).expect_err("space root direct delete is invalid");

        assert!(err.to_string().contains("cannot target a space root"));
    }

    #[test]
    fn direct_recursive_rm_rejects_include_exclude_filters() {
        let mut args = rm_args(
            Some("adrive://inst/space/docs/"),
            true,
            RecursiveDeleteMode::Direct,
        );
        args.include = Some("*.txt".to_string());

        let err = reject_direct_adrive_rm_filters(&args).expect_err("direct delete cannot filter");

        assert!(err
            .to_string()
            .contains("recursive-delete-mode direct does not support"));
    }

    #[test]
    fn recursive_delete_target_folder_entry_skips_space_root() {
        let target = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: String::new(),
        };
        let mut entries = vec![ADriveDeleteEntry::File("a.txt".to_string())];

        push_recursive_target_folder_entry(&mut entries, &target);

        assert_eq!(entries, vec![ADriveDeleteEntry::File("a.txt".to_string())]);
    }

    #[test]
    fn adrive_delete_entry_classifies_folder_markers_as_folders() {
        let slash_marker = test_file_info("target/debug/.fingerprint/", "file");
        let type_marker = test_file_info("target/debug/build", "folder");

        assert_eq!(
            adrive_delete_entry_for_file(slash_marker),
            ADriveDeleteEntry::Folder("target/debug/.fingerprint".to_string())
        );
        assert_eq!(
            adrive_delete_entry_for_file(type_marker),
            ADriveDeleteEntry::Folder("target/debug/build".to_string())
        );
    }

    #[test]
    fn adrive_delete_entry_for_file_tracks_folder_marker_operation() {
        let entry =
            adrive_delete_entry_for_file(test_file_info("target/debug/.fingerprint/", "file"));

        assert_eq!(
            entry,
            ADriveDeleteEntry::Folder("target/debug/.fingerprint".to_string())
        );
        assert_eq!(entry.operation(), "delete_folder");
    }

    #[test]
    fn recursive_rm_filters_delete_entries_like_hns_markers() {
        let entries = vec![
            ADriveDeleteEntry::Folder("docs".to_string()),
            ADriveDeleteEntry::Folder("docs/keep".to_string()),
            ADriveDeleteEntry::File("docs/keep/a.txt".to_string()),
            ADriveDeleteEntry::File("docs/skip/a.txt".to_string()),
        ];

        let selected = adrive_delete_entries_for_rm(entries, Some("docs/*"), Some("docs/skip/*"));

        assert_eq!(
            selected,
            vec![
                ADriveDeleteEntry::Folder("docs".to_string()),
                ADriveDeleteEntry::Folder("docs/keep".to_string()),
                ADriveDeleteEntry::File("docs/keep/a.txt".to_string()),
            ]
        );
    }

    #[test]
    fn recursive_rm_filter_exclude_matches_normalized_folder_slash() {
        let entry = ADriveDeleteEntry::Folder("docs".to_string());

        assert!(!adrive_delete_entry_matches_filters(
            &entry,
            None,
            Some("docs/")
        ));
    }

    #[test]
    fn by_name_resolution_keeps_original_segment_when_id_is_already_available() {
        assert_eq!(
            non_empty_or_original(String::new(), "adrive-2147489060-IDS2000000194372652384"),
            "adrive-2147489060-IDS2000000194372652384"
        );
    }

    #[test]
    fn upload_checkpoint_match_accepts_space_root() {
        let target = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: String::new(),
        };

        assert!(upload_checkpoint_matches_rm_target(
            &upload_checkpoint("docs/a.bin", Some("upload-1")),
            &target
        ));
    }

    #[test]
    fn upload_checkpoint_match_requires_same_instance_space_and_upload_id() {
        let target = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: String::new(),
        };
        let mut other_space = upload_checkpoint("docs/a.bin", Some("upload-1"));
        other_space.space = "other".to_string();

        assert!(!upload_checkpoint_matches_rm_target(&other_space, &target));
        assert!(!upload_checkpoint_matches_rm_target(
            &upload_checkpoint("docs/a.bin", None),
            &target
        ));
    }

    #[test]
    fn upload_checkpoint_match_scopes_folder_prefix() {
        let target = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: "docs/".to_string(),
        };

        assert!(upload_checkpoint_matches_rm_target(
            &upload_checkpoint("docs/a.bin", Some("upload-1")),
            &target
        ));
        assert!(upload_checkpoint_matches_rm_target(
            &upload_checkpoint("docs/nested/a.bin", Some("upload-2")),
            &target
        ));
        assert!(!upload_checkpoint_matches_rm_target(
            &upload_checkpoint("docs2/a.bin", Some("upload-3")),
            &target
        ));
    }

    #[test]
    fn upload_checkpoint_match_scopes_exact_file() {
        let target = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: "docs/a.bin".to_string(),
        };

        assert!(upload_checkpoint_matches_rm_target(
            &upload_checkpoint("docs/a.bin", Some("upload-1")),
            &target
        ));
        assert!(!upload_checkpoint_matches_rm_target(
            &upload_checkpoint("docs/a.bin.part", Some("upload-2")),
            &target
        ));
    }

    #[test]
    fn test_sort_delete_entries_bottom_up_places_children_before_parents() {
        let mut entries = vec![
            ADriveDeleteEntry::Folder("docs".to_string()),
            ADriveDeleteEntry::Folder(normalize_adrive_folder_path("docs/")),
            ADriveDeleteEntry::Folder("docs/a".to_string()),
            ADriveDeleteEntry::File("docs/a/file.txt".to_string()),
            ADriveDeleteEntry::File("docs/b.txt".to_string()),
        ];

        sort_delete_entries_bottom_up(&mut entries);

        let root_index = entries
            .iter()
            .position(|entry| entry.path() == "docs")
            .unwrap();
        let child_dir_index = entries
            .iter()
            .position(|entry| entry.path() == "docs/a")
            .unwrap();
        let file_index = entries
            .iter()
            .position(|entry| entry.path() == "docs/a/file.txt")
            .unwrap();
        assert!(file_index < child_dir_index);
        assert!(child_dir_index < root_index);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.path() == "docs")
                .count(),
            1
        );
    }

    #[test]
    fn test_sync_delete_plan_orders_deep_paths_first() {
        let mut items = vec![
            sync_delete_item("adrive://inst/space/docs/root.txt"),
            sync_delete_item("adrive://inst/space/docs/a/deep.txt"),
            sync_delete_folder_item("adrive://inst/space/docs/a"),
            sync_delete_folder_item("adrive://inst/space/docs"),
        ];

        sort_adrive_sync_delete_plan_bottom_up(&mut items);

        assert_eq!(items[0].source, "adrive://inst/space/docs/a/deep.txt");
        assert!(
            items
                .iter()
                .position(|item| item.source == "adrive://inst/space/docs/root.txt")
                .expect("root file")
                > items
                    .iter()
                    .position(|item| item.source == "adrive://inst/space/docs/a/deep.txt")
                    .expect("deep file")
        );
        assert!(
            items
                .iter()
                .position(|item| item.source == "adrive://inst/space/docs/a/deep.txt")
                .expect("deep file")
                < items
                    .iter()
                    .position(|item| item.source == "adrive://inst/space/docs/a")
                    .expect("child folder")
        );
        assert!(
            items
                .iter()
                .position(|item| item.source == "adrive://inst/space/docs/a")
                .expect("child folder")
                < items
                    .iter()
                    .position(|item| item.source == "adrive://inst/space/docs")
                    .expect("root folder")
        );
    }

    #[test]
    fn test_extra_folder_delete_plan_keeps_desired_parent_folders() {
        let target = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: "dst/".to_string(),
        };
        let mut desired_folders = BTreeSet::new();
        add_relative_file_parent_folders(&mut desired_folders, "keep/file.txt");
        let mut plan = Vec::new();

        append_adrive_extra_folder_delete_items(
            &mut plan,
            vec![
                test_folder_info("dst"),
                test_folder_info("dst/keep/"),
                test_folder_info("dst/stale/"),
                test_folder_info("dst/stale/child/"),
            ],
            &target,
            "dst/",
            &desired_folders,
            None,
            None,
        );

        let sources = plan
            .iter()
            .map(|item| item.source.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            sources,
            vec![
                "adrive://inst/space/dst/stale",
                "adrive://inst/space/dst/stale/child"
            ]
        );
        assert!(plan
            .iter()
            .all(|item| item.operation == SYNC_DELETE_EXTRA_FOLDER));
    }

    #[test]
    fn test_remote_folder_relative_path_uses_path_boundary() {
        assert_eq!(remote_folder_relative_path("dst", "dst/"), "");
        assert_eq!(remote_folder_relative_path("dst/stale", "dst/"), "stale");
        assert_eq!(
            remote_folder_relative_path("dst-other/stale", "dst/"),
            "dst-other/stale"
        );
    }

    #[test]
    fn test_remote_file_path_for_destination_rejects_same_file_after_resolution() {
        let source = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: "docs/1.txt".to_string(),
        };
        let destination = ParsedADriveUri {
            instance: "inst".to_string(),
            space: "space".to_string(),
            path: "docs/".to_string(),
        };

        let err = remote_file_path_for_destination(&destination, &source)
            .expect_err("same resolved file must be rejected");

        assert!(err
            .to_string()
            .contains("source and destination resolve to the same ADrive file"));
    }

    #[test]
    fn test_bounded_list_page_limit_uses_remainder_page() {
        let mut returned = 0;
        let mut pages = Vec::new();
        while returned < 9950 {
            let page_size = bounded_list_page_limit(returned, 9950);
            pages.push(page_size);
            returned += page_size;
        }

        assert_eq!(pages.len(), 10);
        assert_eq!(&pages[..9], &[1000; 9]);
        assert_eq!(pages[9], 950);
    }

    #[test]
    fn test_normalize_ls_files_filters_current_folder_marker() {
        let (files, folders) = normalize_ls_files_and_folders(
            vec![
                test_file_info("folder/", "folder"),
                test_file_info("folder/file.txt", "file"),
            ],
            vec![FolderInfo {
                folder: "folder/sub/".to_string(),
                updated_at: 0,
            }],
            "folder/",
        );

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_path, "folder/file.txt");
        assert_eq!(
            folders
                .iter()
                .map(|folder| folder.folder.as_str())
                .collect::<Vec<_>>(),
            vec!["folder/sub/"]
        );
    }

    #[test]
    fn test_normalize_ls_files_promotes_trailing_slash_marker_to_folder() {
        let (files, folders) = normalize_ls_files_and_folders(
            vec![test_file_info("folder/standalone/", "file")],
            Vec::new(),
            "folder/",
        );

        assert!(files.is_empty());
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].folder, "folder/standalone/");
    }

    #[test]
    fn test_normalize_ls_files_filters_empty_root_marker() {
        let (files, folders) = normalize_ls_files_and_folders(
            vec![
                test_file_info("/", "folder"),
                test_file_info("file.txt", "file"),
            ],
            Vec::new(),
            "",
        );

        assert_eq!(files.len(), 1);
        assert_eq!(files[0].file_path, "file.txt");
        assert!(folders.is_empty());
    }

    #[test]
    fn test_normalize_ls_files_dedupes_existing_folder() {
        let (files, folders) = normalize_ls_files_and_folders(
            vec![test_file_info("folder/sub/", "folder")],
            vec![FolderInfo {
                folder: "folder/sub/".to_string(),
                updated_at: 0,
            }],
            "folder/",
        );

        assert!(files.is_empty());
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].folder, "folder/sub/");
    }

    #[test]
    fn test_parse_columns_uses_custom_columns() {
        assert_eq!(parse_columns(None), None);
        assert_eq!(
            parse_columns(Some(" instance_id,display_name ")).unwrap(),
            &["instance_id", "display_name"]
        );
        assert_eq!(parse_columns(Some(" , ")), None);
    }

    #[test]
    fn test_du_accumulator_streams_profile_buckets_and_top_k() {
        let mut accumulator = DuAccumulator::new(2);
        for file in [
            FileInfo {
                instance_id: String::new(),
                space_id: String::new(),
                file_path: "docs/a/old.log".to_string(),
                file_type: String::new(),
                size: 512,
                updated_at: 1,
                storage_class: "STANDARD".to_string(),
                meta: HashMap::new(),
                hash_crc64_ecma: 0,
                etag: String::new(),
                created_at: 0,
            },
            FileInfo {
                instance_id: String::new(),
                space_id: String::new(),
                file_path: "docs/a/new.bin".to_string(),
                file_type: String::new(),
                size: 2_000_000,
                updated_at: 2,
                storage_class: "IA".to_string(),
                meta: HashMap::new(),
                hash_crc64_ecma: 0,
                etag: String::new(),
                created_at: 0,
            },
            FileInfo {
                instance_id: String::new(),
                space_id: String::new(),
                file_path: "docs/b/huge.dat".to_string(),
                file_type: String::new(),
                size: 200_000_000,
                updated_at: 0,
                storage_class: "STANDARD".to_string(),
                meta: HashMap::new(),
                hash_crc64_ecma: 0,
                etag: String::new(),
                created_at: 0,
            },
        ] {
            accumulator.record_adrive_file(&file, "inst", "space", "docs/", Some(1));
        }

        assert_eq!(accumulator.file_count, 3);
        assert_eq!(accumulator.manifest_items.len(), 3);
        assert_eq!(accumulator.largest_files[0].file_path, "docs/b/huge.dat");
        assert_eq!(accumulator.oldest_files[0].file_path, "docs/a/old.log");
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
        assert!(accumulator
            .cost_estimate(&storage_price_table(&[]).expect("prices"))
            .get("disclaimer")
            .is_some());
    }

    #[tokio::test]
    async fn test_write_batch_report_failures_only_filters_items_and_manifest() {
        let dir = std::env::temp_dir().join(format!("adrive-report-filter-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("report.csv");
        let manifest_path = dir.join("manifest.csv");
        let mut report = BatchReport::new("ve-adrive cp");
        report.set_manifest(vec![BatchManifestItem {
            operation: "upload".to_string(),
            source: "adrive://inst/space/a.txt".to_string(),
            destination: None,
            size: 1,
            etag: None,
            crc64: None,
        }]);
        report.push_success("adrive://inst/space/a.txt".to_string(), None, "copy");
        report.push_failure(
            "adrive://inst/space/b.txt".to_string(),
            None,
            "copy",
            CliError::ValidationError("boom".to_string()),
        );

        write_batch_report(path.to_str(), &report, true, true)
            .await
            .expect("write report");
        write_adrive_manifest_file(
            manifest_path.to_str(),
            "ve-adrive cp",
            report.manifest.as_ref(),
        )
        .await
        .expect("write manifest");
        let body = fs::read_to_string(rolled_csv_path(&path, 1)).expect("read report");
        assert!(body.starts_with("command,operation,source,destination,status"));
        assert!(!body.lines().next().unwrap_or_default().contains("total"));
        assert_eq!(body.lines().count(), 2);
        assert!(
            body.lines()
                .nth(1)
                .unwrap_or_default()
                .starts_with("ve-adrive cp,"),
            "report body={body}"
        );
        assert!(body.contains(",failed,"));
        assert!(!body.contains("a.txt"));
        let manifest_body =
            fs::read_to_string(rolled_csv_path(&manifest_path, 1)).expect("read manifest");
        assert!(
            manifest_body
                .lines()
                .nth(1)
                .unwrap_or_default()
                .starts_with("ve-adrive cp,"),
            "manifest body={manifest_body}"
        );
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
        let seven_days = 7 * 24 * 60 * 60_000;
        let now = chrono::Utc::now().timestamp_millis();
        let within = FindMtimeFilterMillis::WithinLast(seven_days);
        assert!(find_mtime_millis_matches(now, within));
        assert!(!find_mtime_millis_matches(
            now - seven_days as i64 - 10_000,
            within
        ));

        let older = FindMtimeFilterMillis::OlderThanOrEqual(seven_days);
        assert!(find_mtime_millis_matches(
            now - seven_days as i64 - 10_000,
            older
        ));
        assert!(!find_mtime_millis_matches(now, older));
    }

    #[test]
    fn test_find_mtime_filter_bare_duration_matches_exact_age_bucket() {
        let seven_days = 7 * 24 * 60 * 60_000;
        let now = chrono::Utc::now().timestamp_millis();
        let exact = parse_find_mtime_filter_millis("7d").expect("bare duration");
        assert!(find_mtime_millis_matches(now - seven_days as i64, exact));
        assert!(find_mtime_millis_matches(
            now - seven_days as i64 - 60_000,
            exact
        ));
        assert!(!find_mtime_millis_matches(
            now - seven_days as i64 + 60_000,
            exact
        ));
        assert!(!find_mtime_millis_matches(
            now - seven_days as i64 - 24 * 60 * 60_000,
            exact
        ));
    }

    #[test]
    fn test_ls_file_entry_uses_human_readable_size_when_requested() {
        let mut file = test_file_info("docs/big.bin", "file");
        file.size = 10 * 1024 * 1024;

        let readable = adrive_ls_file_entry(&file, true);
        assert_eq!(readable["size"], "10.00 MiB");

        let raw = adrive_ls_file_entry(&file, false);
        assert_eq!(raw["size"], 10 * 1024 * 1024);
    }

    #[test]
    fn test_normalize_ids_range_accepts_raw_or_header_value() {
        assert_eq!(normalize_ids_range("0-1023"), "bytes=0-1023");
        assert_eq!(normalize_ids_range("bytes=0-1023"), "bytes=0-1023");
    }
}
