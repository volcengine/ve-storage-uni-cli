//! Metadata-only download planning; no URL issuance, content requests or local writes.
use super::{
    common::output_result,
    share_checkpoint::{Checkpoint, Journal},
    share_download::{check_reserved, task_identity, verify_resume_entry},
    share_manifest::enumerate_with_options,
    share_plan_local::{checkpoint_path, PlanRoot},
    share_receive::ShareReceiver,
};
use crate::cli::{share::ShareDownloadArgs, ADriveAuthArgs};
use serde_json::{json, Value};
use std::path::Path;
use tos_core::agent::{
    envelope::{Envelope, ErrorDetail, ErrorKind, Status},
    error::CliError,
    global_args::GlobalArgs,
};

#[derive(Default)]
struct PlanItems {
    files: Vec<Value>,
    directories: Vec<Value>,
    conflicts: Vec<Value>,
    total_bytes: u64,
    skipped: usize,
    metadata_responses: usize,
    artifact_paths: Option<super::share_artifacts::Paths>,
}

/// Enumerate and inspect the selected task without local mutation or download-URL issuance; print a plan even when conflicts are found.
pub(crate) async fn dry_run(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    args: &ShareDownloadArgs,
) -> Result<i32, CliError> {
    let resolved = super::share_transfer_options::resolve(global, args)?;
    let args = &resolved;
    let plan = tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(CliError::Io)?;
            return Err(CliError::TransferFailed("download planning interrupted; no files or checkpoint were written".into()));
        }
        result = build_plan(global, auth, args) => result?,
    };
    let has_conflicts = plan["selection"]["Conflicts"]
        .as_array()
        .is_some_and(|items| !items.is_empty());
    let mut envelope = Envelope::success("ve-adrive share download", plan);
    if has_conflicts {
        apply_conflicts(&mut envelope);
    }
    output_result(global, &envelope)?;
    Ok(if has_conflicts { 9 } else { 0 })
}

async fn build_plan(
    global: &GlobalArgs,
    auth: &ADriveAuthArgs,
    args: &ShareDownloadArgs,
) -> Result<Value, CliError> {
    let root = PlanRoot::open(Path::new(&args.destination_dir))?;
    let checkpoint = args
        .checkpoint
        .as_deref()
        .map(checkpoint_path)
        .transpose()?;
    let mut receiver = ShareReceiver::for_command(global, auth, &args.share_url)?;
    let info = receiver.info().await?;
    receiver
        .authorize(global, &info, args.is_password_stdin)
        .await?;
    let mut display = super::share_download_progress::DownloadProgress::new(global, args);
    display.begin_scan();
    let journal = read_snapshot(&mut receiver, &root, args, info.file_path_list.as_deref()).await?;
    display.finish_scan(journal.manifest.files.len());
    journal.validate(&journal.identity)?;
    check_reserved(&journal, checkpoint.as_deref())?;
    let mut items = inspect_files(&mut receiver, &root, &journal, args).await?;
    let paths = super::share_artifacts::Paths::resolve(global, args)?;
    paths.validate_against_manifest(&journal.manifest, &root.path, checkpoint.as_deref())?;
    items.artifact_paths = Some(paths);
    inspect_directories(&root, &journal, &mut items)?;
    inspect_checkpoint(checkpoint.as_deref(), args.is_resume, &mut items)?;
    items.metadata_responses = global.request_trace.snapshot().response_count;
    Ok(plan_output(args, &receiver, &root, &journal, items))
}

async fn read_snapshot(
    receiver: &mut ShareReceiver,
    root: &PlanRoot,
    args: &ShareDownloadArgs,
    declared_roots: Option<&[String]>,
) -> Result<Journal, CliError> {
    let identity = task_identity(receiver, args, &root.path);
    if args.is_resume {
        let path = args
            .checkpoint
            .as_deref()
            .ok_or_else(|| CliError::ValidationError("--resume requires --checkpoint".into()))?;
        return Checkpoint::read_only(&checkpoint_path(path)?, &identity);
    }
    let manifest = enumerate_with_options(receiver, args, declared_roots).await?;
    Ok(Journal {
        version: 1,
        identity,
        completed: vec![None; manifest.files.len()],
        manifest,
    })
}

async fn inspect_files(
    receiver: &mut ShareReceiver,
    root: &PlanRoot,
    journal: &Journal,
    args: &ShareDownloadArgs,
) -> Result<PlanItems, CliError> {
    let mut items = PlanItems::default();
    for (index, entry) in journal.manifest.files.iter().enumerate() {
        items.total_bytes = items
            .total_bytes
            .checked_add(entry.remote.size)
            .ok_or_else(|| {
                CliError::ValidationError("download plan total byte count overflows".into())
            })?;
        let action = inspect_file(receiver, root, journal, index, args).await;
        let action = plan_action(action, &entry.local, &mut items.conflicts)?;
        items.skipped += usize::from(action == "skip");
        items.files.push(json!({"FilePath":entry.remote.path,"LocalPath":entry.local,
            "DestinationPath":root.path.join(&entry.local),"Size":entry.remote.size,"Action":action}));
    }
    Ok(items)
}
async fn inspect_file(
    receiver: &mut ShareReceiver,
    root: &PlanRoot,
    journal: &Journal,
    index: usize,
    args: &ShareDownloadArgs,
) -> Result<&'static str, CliError> {
    let entry = &journal.manifest.files[index];
    let expected = journal.completed[index].as_deref();
    let action = root.file_action(&entry.local, true)?;
    if args.is_resume
        && verify_resume_entry(receiver, root.directory.as_ref(), entry, expected).await?
    {
        return Ok("skip");
    }
    if action == "overwrite" {
        if let Some(directory) = &root.directory {
            let (parent, name) = directory.parent(&entry.local, false)?;
            if super::share_transfer_options::skip_existing(
                args,
                entry.remote.updated_at,
                &parent.read(&name)?.metadata()?,
            )? {
                return Ok("skip");
            }
        }
    }
    Ok(action)
}
fn inspect_directories(
    root: &PlanRoot,
    journal: &Journal,
    items: &mut PlanItems,
) -> Result<(), CliError> {
    for path in &journal.manifest.directories {
        let action = plan_action(root.directory_action(path), path, &mut items.conflicts)?;
        items
            .directories
            .push(json!({"LocalPath":path,"DestinationPath":root.path.join(path),"Action":action}));
    }
    Ok(())
}
fn plan_action(
    result: Result<&'static str, CliError>,
    path: &str,
    conflicts: &mut Vec<Value>,
) -> Result<&'static str, CliError> {
    match result {
        Ok(action) => Ok(action),
        Err(CliError::Conflict(reason)) => {
            conflicts.push(json!({"LocalPath":path,"Reason":reason}));
            Ok("conflict")
        }
        Err(error) => Err(error),
    }
}
fn inspect_checkpoint(
    path: Option<&Path>,
    is_resume: bool,
    items: &mut PlanItems,
) -> Result<(), CliError> {
    let Some(path) = path else {
        return Ok(());
    };
    let parent = PlanRoot::open(path.parent().unwrap_or(Path::new(".")))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::ValidationError("invalid checkpoint filename".into()))?;
    // [Review Fix #D02] Inspect a sidecar's type without creating or taking its lock; execution still performs the authoritative lock check.
    plan_action(
        parent.file_action(&format!("{name}.lock"), true),
        &format!("{name}.lock"),
        &mut items.conflicts,
    )?;
    if is_resume {
        return Ok(());
    }
    match std::fs::symlink_metadata(path) {
        Ok(_) => items.conflicts.push(json!({"Checkpoint":path,"Reason":"checkpoint exists; use --resume or choose a different checkpoint"})),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
fn plan_output(
    args: &ShareDownloadArgs,
    receiver: &ShareReceiver,
    root: &PlanRoot,
    journal: &Journal,
    items: PlanItems,
) -> Value {
    json!({"dry_run":true,"action":"download","execution_status":"planned_not_executed",
        "report_path":items.artifact_paths.as_ref().map(|paths|&paths.report_path),
        "manifest_path":items.artifact_paths.as_ref().and_then(|paths|paths.manifest_path.as_ref()),
        "checkpoint_path":args.checkpoint,
        "selection":{
        "ShareUrl":receiver.identity.safe_url,"Path":args.path,"DestinationDir":root.path,
        "Checkpoint":args.checkpoint,"Resume":args.is_resume,"Recursive":args.is_recursive,"Overwrite":args.is_overwrite,
        "IncludeParent":args.is_include_parent,"Include":args.include,"Exclude":args.exclude,
        "BatchConcurrency":args.batch_concurrency,"ListConcurrency":args.list_concurrency,
        "MultipartConcurrency":args.multipart_concurrency,"BandwidthLimit":args.bandwidth_limit,
        "PlanComplete":true,"FileCount":items.files.len(),"DirectoryCount":items.directories.len(),
        "TotalBytes":items.total_bytes,"SkippedCount":items.skipped,
        "Manifest":{"Roots":journal.manifest.roots,"Files":items.files,"Directories":items.directories},
        "Conflicts":items.conflicts,"NetworkRequests":null,"MetadataResponses":items.metadata_responses,
        "DownloadsIssued":0,"LocalWrites":false,"SnapshotGuaranteed":false}})
}
fn apply_conflicts(envelope: &mut Envelope<Value>) {
    let error = CliError::Conflict(
        "download plan contains local conflicts; inspect selection.Conflicts before executing"
            .into(),
    );
    let semantics = error.agent_semantics();
    envelope.success = false;
    envelope.status = Status::Failed;
    envelope.ec = Some(semantics.code.clone());
    envelope.error = Some(ErrorDetail { status_code:None, code:semantics.code, message:semantics.message,
        exit_code:9,kind:ErrorKind::Conflict,category:semantics.category,
        suggested_action:Some("Inspect selection.Conflicts; choose another destination/checkpoint or explicitly resolve existing local entries".into()),
        fix_command:None,doctor_hint:None,docs_url:None });
}
