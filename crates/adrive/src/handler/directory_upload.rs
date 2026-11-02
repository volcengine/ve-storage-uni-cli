//! Explicit ADrive directory tasks for local recursive uploads.

use super::*;

const CREATE_FOLDER_OPERATION: &str = "create-folder";

pub(super) struct UploadDirectoryTask {
    source: PathBuf,
    remote_path: String,
}

#[derive(Default)]
struct ParentEntries {
    folders: HashSet<String>,
    files: HashSet<String>,
}

#[derive(Default)]
pub(super) struct DirectoryUploadState {
    parents: HashMap<String, ParentEntries>,
    verified: HashSet<String>,
    reported: HashSet<String>,
}

pub(super) fn validate_directory_upload_option(
    source: &str,
    destination: &str,
    is_recursive: bool,
    is_disabled: bool,
) -> Result<(), CliError> {
    if is_disabled
        && (!is_recursive
            || source.starts_with("adrive://")
            || !destination.starts_with("adrive://")
            || !Path::new(source).is_dir())
    {
        return Err(CliError::ValidationError(
            "--disable-dir-object requires a recursive local-directory-to-ADrive upload".into(),
        ));
    }
    Ok(())
}

pub(super) fn add_directory_upload_plan(plan: &mut Value, command: &ADriveCommand) {
    let (source, destination, is_recursive, is_disabled) = match command {
        ADriveCommand::Cp(args) => (
            &args.source,
            &args.destination,
            args.recursive,
            args.disable_dir_object,
        ),
        ADriveCommand::Sync(args) => (
            &args.source,
            &args.destination,
            true,
            args.disable_dir_object,
        ),
        _ => return,
    };
    if !is_recursive || source.starts_with("adrive://") || !destination.starts_with("adrive://") {
        return;
    }
    plan["directory_upload"] = json!({
        "enabled": !is_disabled,
        "disable_dir_object": is_disabled,
        "implicit_file_parents": true,
        "representation": "create_folder",
        "existing_directories": "revalidated",
    });
    if !is_disabled {
        if let Some(requests) = plan["request_plan"].as_array_mut() {
            requests.push(json!("create_folder"));
        }
    }
}

pub(super) fn collect_upload_directory_tasks(
    args: &CpArgs,
    destination: &ParsedADriveUri,
) -> Result<Vec<UploadDirectoryTask>, CliError> {
    if args.disable_dir_object {
        return Ok(Vec::new());
    }
    let source = Path::new(&args.source);
    let parent = recursive_adrive_source_parent_prefix(&args.source, args.include_parent)?;
    let mut desired = collect_local_sync_desired_relative_paths(
        source,
        parent.as_deref(),
        args.include.as_deref(),
        args.exclude.as_deref(),
    )?;
    desired.folders.insert(parent.clone().unwrap_or_default());
    let mut tasks = desired
        .folders
        .into_iter()
        .filter_map(|relative| {
            let remote_path = join_remote_path(&destination.path, &relative);
            if remote_path.is_empty() {
                return None;
            }
            let local_relative = parent
                .as_deref()
                .and_then(|prefix| relative.strip_prefix(prefix))
                .unwrap_or(&relative)
                .trim_start_matches('/');
            Some(UploadDirectoryTask {
                source: source.join(local_relative),
                remote_path,
            })
        })
        .collect::<Vec<_>>();
    tasks.sort_by_key(|task| {
        (
            adrive_path_depth(&task.remote_path),
            task.remote_path.clone(),
        )
    });
    Ok(tasks)
}

pub(super) fn directory_upload_manifest(
    destination: &ParsedADriveUri,
    tasks: &[UploadDirectoryTask],
) -> Vec<BatchManifestItem> {
    tasks
        .iter()
        .map(|task| BatchManifestItem {
            operation: CREATE_FOLDER_OPERATION.into(),
            source: task.source.display().to_string(),
            destination: Some(directory_target(destination, &task.remote_path)),
            size: 0,
            etag: None,
            crc64: None,
        })
        .collect()
}

fn directory_target(destination: &ParsedADriveUri, remote_path: &str) -> String {
    format!(
        "adrive://{}/{}/{}/",
        destination.instance, destination.space, remote_path
    )
}

impl DirectoryUploadState {
    async fn ensure(
        &mut self,
        client: &IdsClient,
        destination: &ParsedADriveUri,
        remote_path: &str,
    ) -> Result<(), CliError> {
        for folder in folder_paths_for_mkdir(remote_path, true) {
            if !self.verified.contains(&folder) {
                self.ensure_one(client, destination, &folder).await?;
                self.verified.insert(folder);
            }
        }
        Ok(())
    }

    async fn ensure_one(
        &mut self,
        client: &IdsClient,
        destination: &ParsedADriveUri,
        folder: &str,
    ) -> Result<(), CliError> {
        let parent = folder
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or("");
        if !self.parents.contains_key(parent) {
            let entries = list_parent_entries(client, destination, parent).await?;
            self.parents.insert(parent.to_string(), entries);
        }
        let entries = &self.parents[parent];
        if entries.files.contains(folder) {
            return Err(CliError::Conflict(format!(
                "cannot create folder over existing file: {}",
                directory_target(destination, folder)
            )));
        }
        if entries.folders.contains(folder) {
            return Ok(());
        }
        self.create_missing_folder(client, destination, folder, parent)
            .await?;
        self.parents
            .get_mut(parent)
            .expect("parent listing cached")
            .folders
            .insert(folder.to_string());
        Ok(())
    }

    async fn create_missing_folder(
        &mut self,
        client: &IdsClient,
        destination: &ParsedADriveUri,
        folder: &str,
        parent: &str,
    ) -> Result<(), CliError> {
        let input = CreateFolderInput {
            instance_id: destination.instance.clone(),
            space_id: destination.space.clone(),
            folder_path: folder.to_string(),
        };
        if let Err(error) = client.create_folder(&input).await {
            // [Review Fix #DirectoryRace] A concurrent writer may create this folder;
            // only a typed exact folder listing may turn a conflict into success.
            if !matches!(&error, IdsSdkError::Server(server) if server.status_code == Some(409)) {
                return Err(map_ids_error(error));
            }
            let entries = list_parent_entries(client, destination, parent).await?;
            if entries.files.contains(folder) || !entries.folders.contains(folder) {
                return Err(map_ids_error(error));
            }
            self.parents.insert(parent.to_string(), entries);
        }
        Ok(())
    }
}

async fn list_parent_entries(
    client: &IdsClient,
    destination: &ParsedADriveUri,
    parent: &str,
) -> Result<ParentEntries, CliError> {
    let mut entries = ParentEntries::default();
    let mut marker = None;
    let mut seen_markers = HashSet::new();
    loop {
        let mut input = ListFilesInput::new(&destination.instance, &destination.space)
            .with_delimiter("/")
            .with_limit(1000);
        if !parent.is_empty() {
            input = input.with_prefix(trim_folder_prefix(parent));
        }
        if let Some(value) = marker.take() {
            input = input.with_marker(value);
        }
        let output = client.list_files(&input).await.map_err(map_ids_error)?;
        for folder in output.folders {
            entries
                .folders
                .insert(normalize_adrive_folder_path(&folder.folder));
        }
        for file in output.files {
            let path = normalize_adrive_folder_path(&file.file_path);
            if adrive_file_info_is_folder_marker(&file) {
                entries.folders.insert(path);
            } else {
                entries.files.insert(path);
            }
        }
        if !output.is_truncated {
            return Ok(entries);
        }
        if output.next_marker.is_empty() || !seen_markers.insert(output.next_marker.clone()) {
            return Err(CliError::TransferFailed(
                "ADrive folder listing returned an invalid pagination marker".into(),
            ));
        }
        marker = Some(output.next_marker);
    }
}

pub(super) async fn execute_upload_directories(
    context: &mut ADriveStreamCpContext<'_>,
    destination: &ParsedADriveUri,
    tasks: Vec<UploadDirectoryTask>,
) -> Result<(), CliError> {
    for task in tasks {
        record_directory_upload(context, destination, task).await?;
    }
    Ok(())
}

async fn record_directory_upload(
    context: &mut ADriveStreamCpContext<'_>,
    destination: &ParsedADriveUri,
    task: UploadDirectoryTask,
) -> Result<(), CliError> {
    if !context
        .directory_state
        .reported
        .insert(task.remote_path.clone())
    {
        return Ok(());
    }
    let target = directory_target(destination, &task.remote_path);
    let source = task.source.display().to_string();
    let item_key = checkpoint_item_key(CREATE_FOLDER_OPERATION, &source, Some(&target));
    // [Review Fix #DirectoryResume] Directory checkpoints never prove present-day
    // existence; revalidate typed remote state on every invocation.
    // [Review Fix #ReportId3] Capture directory requests separately from file transfers.
    let result = capture_service_request_id(context.directory_state.ensure(
        context.client,
        destination,
        &task.remote_path,
    ))
    .await;
    let units = progress_units_for_size(0, context.runtime);
    record_adrive_batch_result(
        context.report,
        context.checkpoint_path.as_deref(),
        &mut context.checkpoint,
        context.progress,
        CREATE_FOLDER_OPERATION,
        source,
        Some(target),
        item_key,
        units,
        result,
    )
    .await
}

pub(super) async fn stream_upload_directory(
    context: &mut ADriveStreamCpContext<'_>,
    destination: &ParsedADriveUri,
    source_root: &Path,
    directory: &Path,
    is_required: bool,
) -> Result<(), CliError> {
    if context.args.disable_dir_object {
        return Ok(());
    }
    let parent =
        recursive_adrive_source_parent_prefix(&context.args.source, context.args.include_parent)?;
    let relative = directory_relative_path(source_root, directory, parent.as_deref())?;
    if !is_required
        && directory != source_root
        && !path_matches_filters(
            &relative,
            context.args.include.as_deref(),
            context.args.exclude.as_deref(),
        )
    {
        return Ok(());
    }
    let ancestors = directory
        .ancestors()
        .take_while(|path| path.starts_with(source_root))
        .collect::<Vec<_>>();
    for ancestor in ancestors.into_iter().rev() {
        let relative = directory_relative_path(source_root, ancestor, parent.as_deref())?;
        let remote_path = join_remote_path(&destination.path, &relative);
        if !remote_path.is_empty() {
            record_directory_upload(
                context,
                destination,
                UploadDirectoryTask {
                    source: ancestor.to_path_buf(),
                    remote_path,
                },
            )
            .await?;
        }
    }
    Ok(())
}

fn directory_relative_path(
    source_root: &Path,
    directory: &Path,
    parent: Option<&str>,
) -> Result<String, CliError> {
    let relative = directory
        .strip_prefix(source_root)
        .map_err(|error| CliError::ValidationError(error.to_string()))?;
    let relative = normalize_local_relative_path(relative)?;
    Ok(prepend_adrive_parent_prefix(&relative, parent))
}
