//! Local directory upload discovery, zero-byte execution, and sync retention.

use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
/// Controls explicit directory tasks without changing file uploads.
pub(super) enum DirectoryUploadPolicy {
    Preserve,
    FilesOnly,
}

impl DirectoryUploadPolicy {
    /// Convert the CLI disable flag into its explicit upload policy.
    pub(super) fn from_disabled(is_disabled: bool) -> Self {
        if is_disabled {
            Self::FilesOnly
        } else {
            Self::Preserve
        }
    }
}

/// Iteratively walks a selected source root, skipping descendant symlinks.
pub(super) struct LocalUploadWalker {
    source_root: PathBuf,
    destination: ParsedTosUri,
    parent_prefix: Option<String>,
    pending: Vec<PathBuf>,
}

impl LocalUploadWalker {
    /// Build a local-to-TOS walker; invalid roots or destination URIs return validation errors.
    pub(super) fn new(
        source: &str,
        destination: &str,
        include_parent: bool,
    ) -> Result<Self, CliError> {
        if !Path::new(source).is_dir() {
            return Err(CliError::ValidationError(format!(
                "recursive source '{source}' must be a local directory"
            )));
        }
        Ok(Self {
            source_root: PathBuf::from(source),
            destination: parse_tos_uri(destination, true)?,
            parent_prefix: recursive_source_parent_prefix(source, include_parent)?,
            pending: vec![PathBuf::from(source)],
        })
    }

    fn inspect(&mut self, source: PathBuf) -> Result<Option<TransferPlanItem>, CliError> {
        let metadata = if source == self.source_root {
            fs::metadata(&source)?
        } else {
            fs::symlink_metadata(&source)?
        };
        // [Review Fix #Directory-Symlink] Descendants must not escape the tree or form cycles.
        if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
            return Ok(None);
        }
        if metadata.is_dir() {
            let children = sorted_read_dir_entries(&source)?;
            self.pending
                .extend(children.into_iter().rev().map(|entry| entry.path()));
        }
        self.plan_item(source, metadata.is_dir(), metadata.len())
    }

    fn plan_item(
        &self,
        source: PathBuf,
        is_directory: bool,
        size: u64,
    ) -> Result<Option<TransferPlanItem>, CliError> {
        let relative = source.strip_prefix(&self.source_root).map_err(|error| {
            CliError::ValidationError(format!("failed to derive relative upload path: {error}"))
        })?;
        let relative_key = prepend_parent_prefix(
            &relative.to_string_lossy().replace('\\', "/"),
            self.parent_prefix.as_deref(),
        );
        let mut key = join_tos_key(self.destination.key.as_deref().unwrap_or(""), &relative_key);
        // [Review Fix #Directory-Root] Bucket roots are not objects; still visit their children.
        if key.is_empty() {
            return Ok(None);
        }
        let mut relative_key = relative_key;
        if is_directory {
            if !key.ends_with('/') {
                key.push('/');
            }
            if !relative_key.is_empty() && !relative_key.ends_with('/') {
                relative_key.push('/');
            }
        }
        Ok(Some(TransferPlanItem {
            relative_key,
            source: source.to_string_lossy().into_owned(),
            destination: format!("tos://{}/{}", self.destination.bucket, key),
            size: if is_directory { 0 } else { size },
            etag: None,
            crc64: None,
            last_modified: None,
        }))
    }
}

impl Iterator for LocalUploadWalker {
    type Item = Result<TransferPlanItem, CliError>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(source) = self.pending.pop() {
            match self.inspect(source) {
                Ok(Some(item)) => return Some(Ok(item)),
                Ok(None) => {}
                Err(error) => return Some(Err(error)),
            }
        }
        None
    }
}

/// Return whether the source is a local directory and the destination is TOS.
pub(super) fn is_local_upload(source: &str, destination: &str) -> bool {
    !source.starts_with("tos://") && Path::new(source).is_dir() && destination.starts_with("tos://")
}

/// Reject an enabled disable flag outside local-directory-to-TOS transfers.
pub(super) fn validate_directory_upload_option(
    source: &str,
    destination: &str,
    is_disabled: bool,
) -> Result<(), CliError> {
    if is_disabled && !is_local_upload(source, destination) {
        return Err(CliError::ValidationError(
            "--disable-dir-object requires a local-directory upload to tos://".into(),
        ));
    }
    Ok(())
}

/// Apply filters to the item's source-relative path, excluding parent prefixes and slash suffixes.
pub(super) fn local_upload_item_matches(
    item: &TransferPlanItem,
    source: &str,
    include: Option<&str>,
    exclude: Option<&str>,
) -> bool {
    if Path::new(&item.source) == Path::new(source) {
        return true;
    }
    let Ok(relative) = Path::new(&item.source).strip_prefix(source) else {
        return false;
    };
    pattern_allows(
        &relative.to_string_lossy().replace('\\', "/"),
        include,
        exclude,
    )
}

/// Collect selected upload kinds; propagate filesystem discovery or URI validation errors.
pub(super) fn build_local_upload_mappings(
    source: &str,
    destination: &str,
    options: &RecursiveMappingOptions<'_>,
) -> Result<Vec<TransferPlanItem>, CliError> {
    LocalUploadWalker::new(source, destination, options.include_parent)?
        .filter(|result| {
            options.directory_upload_policy == Some(DirectoryUploadPolicy::Preserve)
                || !result
                    .as_ref()
                    .is_ok_and(|item| item.destination.ends_with('/'))
        })
        .collect()
}

fn cp_upload_item_matches(item: &TransferPlanItem, args: &CpArgs) -> bool {
    // [Review Fix #Directory-Filters] Preserve cp's include-parent filter convention for files.
    Path::new(&item.source) == Path::new(&args.source)
        || pattern_allows(
            item.relative_key.trim_end_matches('/'),
            args.include.as_deref(),
            args.exclude.as_deref(),
        )
}

fn required_local_upload_ancestors(
    walker: &LocalUploadWalker,
    item: &TransferPlanItem,
) -> Result<Vec<TransferPlanItem>, CliError> {
    let mut ancestors = Vec::new();
    for source in Path::new(&item.source).ancestors().skip(1) {
        if source.strip_prefix(&walker.source_root).is_err() {
            break;
        }
        if let Some(directory) = walker.plan_item(source.to_path_buf(), true, 0)? {
            ancestors.push(directory);
        }
        if source == walker.source_root {
            break;
        }
    }
    ancestors.reverse();
    Ok(ancestors)
}

fn select_local_upload_actions(
    walker: &LocalUploadWalker,
    item: TransferPlanItem,
    is_preserving_directories: bool,
    emitted: &mut HashSet<String>,
) -> Result<Vec<TransferPlanItem>, CliError> {
    if !is_preserving_directories && item.destination.ends_with('/') {
        return Ok(Vec::new());
    }
    // [Review Fix #Directory-Ancestors] Selected files need explicit FNS parents even when filters omit directory names.
    let mut actions = if is_preserving_directories {
        required_local_upload_ancestors(walker, &item)?
    } else {
        Vec::new()
    };
    actions.push(item);
    actions.retain(|action| {
        !action.destination.ends_with('/') || emitted.insert(action.destination.clone())
    });
    Ok(actions)
}

/// Apply cp filters and add unique ancestor directory actions, preserving the existing file contract.
pub(super) fn filter_local_cp_upload_mappings(
    args: &CpArgs,
    mappings: Vec<TransferPlanItem>,
) -> Result<Vec<TransferPlanItem>, CliError> {
    let walker = LocalUploadWalker::new(&args.source, &args.destination, args.include_parent)?;
    let mut emitted = HashSet::new();
    let mut planned = Vec::new();
    for item in mappings {
        if cp_upload_item_matches(&item, args) {
            planned.extend(select_local_upload_actions(
                &walker,
                item,
                !args.disable_dir_object,
                &mut emitted,
            )?);
        }
    }
    Ok(planned)
}

/// Preserve ACL/metadata while enforcing empty-body directory content and valid HNS storage headers.
pub(super) fn directory_upload_headers(options: &ObjectWriteOptions) -> BTreeMap<String, String> {
    let mut headers = options.headers(false);
    // [Review Fix #Directory-HNS] HNS directories accept no body and STANDARD only.
    headers.remove("x-tos-storage-class");
    headers.insert("content-type".into(), "application/x-directory".into());
    headers.insert("content-length".into(), "0".into());
    headers
}

/// PUT an empty directory object and report its outcome; preserve validation, service, and report errors.
pub(super) async fn upload_directory_to_tos(
    global: &GlobalArgs,
    client: &TosClient,
    source: &str,
    destination: &str,
    options: CopyOptions<'_>,
) -> Result<CopyTransferResult, CliError> {
    let target = parse_tos_uri(destination, false)?;
    let key = target.key.expect("validated directory key");
    if !key.ends_with('/') {
        return Err(CliError::ValidationError(
            "directory upload requires a trailing-slash object key".into(),
        ));
    }
    if options.overwrite_strategy != EffectiveOverwriteStrategy::Force
        && object_entry_for_bucket_key(client, &target.bucket, &key)
            .await?
            .is_some()
    {
        return Ok(CopyTransferResult::skipped());
    }
    let Some(response) = put_tos_directory(client, &target.bucket, &key, &options).await? else {
        return Ok(CopyTransferResult::skipped());
    };
    if let Some(progress) = options.overall_bar {
        progress.inc(1);
    }
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

async fn put_tos_directory(
    client: &TosClient,
    bucket: &str,
    key: &str,
    options: &CopyOptions<'_>,
) -> Result<Option<Envelope<core::RawResponseData>>, CliError> {
    let mut headers = directory_upload_headers(&options.write_options);
    // [Review Fix #Directory-NoClobber] HEAD cannot protect against concurrent marker creation.
    if options.overwrite_strategy == EffectiveOverwriteStrategy::NoClobber {
        headers.insert("if-none-match".into(), "*".into());
    }
    let response = core::execute_object_request(
        client,
        "ve-tos cp upload-directory",
        Method::PUT,
        bucket,
        key,
        BTreeMap::new(),
        headers,
        Some(Vec::new()),
    )
    .await;
    match response {
        Ok(response) => Ok(Some(response)),
        // [Review Fix #Directory-HNSRace] Child uploads may create the parent after its HEAD.
        Err(error)
            if options.overwrite_strategy == EffectiveOverwriteStrategy::NoClobber
                && error.agent_semantics().status_code == Some(412) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

/// Describe a selected directory policy for dry-run without implying execution.
pub(super) fn directory_policy_guard(is_disabled: bool) -> &'static str {
    if is_disabled {
        "directory policy: explicit directory uploads disabled; files may still create required parent directories"
    } else {
        "directory policy: preserve selected directories, including empty directories, with zero-byte PutObject directory requests"
    }
}

/// Add ancestors within the destination prefix to a sync retention set.
pub(super) fn protect_tos_key_ancestors(
    desired: &mut HashSet<String>,
    key: &str,
    destination_prefix: &str,
) {
    for (index, _) in key.match_indices('/') {
        let parent = &key[..=index];
        if parent.starts_with(destination_prefix) {
            desired.insert(parent.to_string());
        }
    }
}

/// Return explicit upload tasks and retained keys; filesystem discovery failures abort planning.
pub(super) fn build_local_upload_sync_plan(
    args: &SyncArgs,
    destination_prefix: &str,
) -> Result<(Vec<(PathBuf, String, u64)>, HashSet<String>), CliError> {
    let mut planned = Vec::new();
    let mut desired = HashSet::new();
    let mut emitted = HashSet::new();
    let mut walker = LocalUploadWalker::new(&args.source, &args.destination, args.include_parent)?;
    while let Some(item) = walker.next() {
        let item = item?;
        if !local_upload_item_matches(
            &item,
            &args.source,
            args.include.as_deref(),
            args.exclude.as_deref(),
        ) {
            continue;
        }
        let key = parse_tos_uri(&item.destination, false)?
            .key
            .expect("validated destination key");
        desired.insert(key.clone());
        protect_tos_key_ancestors(&mut desired, &key, destination_prefix);
        // [Review Fix #Directory-SyncDelete] Disabling creation never authorizes deleting source-present folders.
        for action in
            select_local_upload_actions(&walker, item, !args.disable_dir_object, &mut emitted)?
        {
            let key = parse_tos_uri(&action.destination, false)?
                .key
                .expect("validated destination key");
            planned.push((PathBuf::from(action.source), key, action.size));
        }
    }
    Ok((planned, desired))
}

/// Queue matching local upload entries; propagate traversal failures for the caller's drain/report gate.
pub(super) async fn stream_cp_local_upload(
    context: &mut TosStreamCopyContext<'_>,
) -> Result<(), CliError> {
    let mut emitted = HashSet::new();
    let mut walker = LocalUploadWalker::new(
        &context.args.source,
        &context.args.destination,
        context.args.include_parent,
    )?;
    while let Some(item) = walker.next() {
        let item = item?;
        if !cp_upload_item_matches(&item, context.args) {
            continue;
        }
        for action in select_local_upload_actions(
            &walker,
            item,
            !context.args.disable_dir_object,
            &mut emitted,
        )? {
            context.queue(action).await;
        }
    }
    Ok(())
}

/// Identify managed destination extras after protecting retained ancestors; invalid source roots fail.
pub(super) fn plan_local_upload_sync_extras(
    args: &SyncArgs,
    destination_prefix: &str,
    inventory: &HashMap<String, ObjectEntry>,
    desired: &mut HashSet<String>,
) -> Result<Vec<ObjectEntry>, CliError> {
    if !args.delete {
        return Ok(Vec::new());
    }
    let parent_prefix = recursive_source_parent_prefix(&args.source, args.include_parent)?;
    // [Review Fix #Directory-ExcludedParent] Retained excluded objects keep their parents alive.
    for entry in inventory.values() {
        if !sync_remote_key_matches(
            args,
            &entry.key,
            destination_prefix,
            parent_prefix.as_deref(),
        ) {
            protect_tos_key_ancestors(desired, &entry.key, destination_prefix);
        }
    }
    let mut extras = inventory
        .values()
        .filter(|entry| {
            !desired.contains(&entry.key)
                && sync_remote_key_matches(
                    args,
                    &entry.key,
                    destination_prefix,
                    parent_prefix.as_deref(),
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    extras.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(extras)
}

/// Return boolean describe metadata for cp/sync without exposing the option on mv.
pub(super) fn directory_upload_parameter() -> CommandParameter {
    CommandParameter {
        schema: Some(json!({"type": "boolean"})),
        ..param(
            "disable-dir-object", ParameterLocation::Flag, false,
            "Skip explicit directory uploads. By default, preserve directories including empty directories. Files may still create required parent directories",
        )
    }
}

/// Credit the one progress unit assigned to a zero-byte directory action.
pub(super) fn directory_progress_bytes(source: &str, destination: &str, bytes: u64) -> u64 {
    if bytes == 0
        && !source.starts_with("tos://")
        && destination.starts_with("tos://")
        && destination.ends_with('/')
    {
        1
    } else {
        bytes
    }
}

fn sync_remote_key_matches(
    args: &SyncArgs,
    key: &str,
    destination_prefix: &str,
    parent_prefix: Option<&str>,
) -> bool {
    let relative = strip_tos_prefix(key, destination_prefix).trim_end_matches('/');
    let relative = if let Some(parent) = parent_prefix {
        if relative == parent {
            ""
        } else {
            relative
                .strip_prefix(&format!("{parent}/"))
                .unwrap_or(relative)
        }
    } else {
        relative
    };
    pattern_allows(relative, args.include.as_deref(), args.exclude.as_deref())
}
