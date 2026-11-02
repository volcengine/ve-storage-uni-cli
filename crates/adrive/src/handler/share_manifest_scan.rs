//! Bounded-concurrency directory scans with serial per-prefix continuation.
use super::super::{common::map_ids_error, share_receive::ShareReceiver};
use super::{
    directory_path, invalid, is_folder, mapping, validate_relative, Manifest, MappingPolicy,
    RemoteFile,
};
use crate::domain::{
    share_types::ShareListFilesInput,
    types::{FileInfo, ListFilesOutput},
};
use futures::{stream::FuturesUnordered, StreamExt};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tos_core::agent::error::CliError;

pub(super) struct Options {
    pub(super) selection: Option<String>,
    pub(super) is_recursive: bool,
    pub(super) concurrency: usize,
    pub(super) policy: MappingPolicy,
}

pub(super) async fn enumerate(
    receiver: &mut ShareReceiver,
    options: Options,
    declared_roots: Option<&[String]>,
) -> Result<Manifest, CliError> {
    if options.concurrency == 0 {
        return Err(invalid("--list-concurrency must be greater than 0"));
    }
    let mut snapshot = Snapshot {
        policy: options.policy,
        ..Default::default()
    };
    match options.selection.as_deref() {
        Some("/") => {
            snapshot.select_space_root(options.is_recursive)?;
            snapshot.accept(read_directory(receiver, Some("/".into())).await?)?;
        }
        Some(path) => snapshot.select_path(receiver, path).await?,
        None => {
            snapshot
                .select_roots(receiver, declared_roots, options.is_recursive)
                .await?;
        }
    }
    if !snapshot.directories.is_empty() && !options.is_recursive {
        return Err(invalid("directory downloads require --recursive"));
    }
    snapshot
        .scan_directories(receiver, options.concurrency)
        .await?;
    snapshot.finish()
}

#[derive(Default)]
pub(super) struct Snapshot {
    pub(super) roots: Vec<String>,
    files: BTreeMap<String, RemoteFile>,
    directories: BTreeSet<String>,
    queue: VecDeque<String>,
    directory_versions: BTreeMap<String, i64>,
    policy: MappingPolicy,
}

impl Snapshot {
    async fn select_path(
        &mut self,
        receiver: &mut ShareReceiver,
        path: &str,
    ) -> Result<(), CliError> {
        let info = receiver
            .ensure_session()
            .await?
            .get_file(path)
            .await
            .map_err(map_ids_error)?;
        self.roots.push(path.to_owned());
        if !is_folder(&info) {
            return self.add_file(info);
        }
        if !path.ends_with('/') {
            return Err(invalid("directory selection must end with a slash"));
        }
        self.add_directory(path)
    }

    async fn select_roots(
        &mut self,
        receiver: &mut ShareReceiver,
        declared_roots: Option<&[String]>,
        is_recursive: bool,
    ) -> Result<(), CliError> {
        if receiver.has_space_root() {
            self.select_space_root(is_recursive)?;
        }
        // [Review Fix #ShareSpacePagination1] Keep the successful root probe on the caller's receiver so subsequent pages never retry the virtual root.
        let listing = read_directory(receiver, None).await?;
        if receiver.has_space_root() {
            self.select_space_root(is_recursive)?;
        } else {
            self.select_declared_roots(declared_roots)?;
        }
        self.accept(listing)?;
        self.infer_roots();
        Ok(())
    }

    fn select_declared_roots(&mut self, declared_roots: Option<&[String]>) -> Result<(), CliError> {
        if let Some(roots) = declared_roots.filter(|roots| !roots.is_empty()) {
            self.roots = roots.to_vec();
            for root in roots {
                validate_relative(root.trim_end_matches('/'))?;
                if root.ends_with('/') {
                    self.add_directory(root)?;
                }
            }
        }
        Ok(())
    }

    fn select_space_root(&mut self, is_recursive: bool) -> Result<(), CliError> {
        if !is_recursive {
            return Err(invalid("whole-Space downloads require --recursive"));
        }
        self.roots = vec!["/".into()];
        self.policy = std::mem::take(&mut self.policy).into_space_root();
        Ok(())
    }
    pub(super) fn add_directory(&mut self, path: &str) -> Result<(), CliError> {
        let directory = directory_path(path)?;
        if self.directories.insert(directory.clone()) {
            self.queue.push_back(directory);
        }
        Ok(())
    }

    pub(super) fn add_file(&mut self, info: FileInfo) -> Result<(), CliError> {
        if is_folder(&info) {
            self.directory_version(&info.file_path, info.updated_at)?;
            return self.add_directory(&info.file_path);
        }
        let file = RemoteFile::from_info(info)?;
        if self
            .files
            .get(&file.path)
            .is_some_and(|previous| previous != &file)
        {
            return Err(invalid(
                "remote file changed or conflicting duplicate during enumeration",
            ));
        }
        self.files.insert(file.path.clone(), file);
        Ok(())
    }

    fn directory_version(&mut self, path: &str, version: i64) -> Result<(), CliError> {
        let path = directory_path(path)?;
        if version == 0 {
            return Ok(());
        }
        if self
            .directory_versions
            .get(&path)
            .is_some_and(|previous| *previous != version)
        {
            return Err(invalid("conflicting duplicate directory metadata"));
        }
        self.directory_versions.insert(path, version);
        Ok(())
    }

    fn infer_roots(&mut self) {
        if !self.roots.is_empty() {
            return;
        }
        self.roots.extend(self.directories.iter().cloned());
        let mapping = mapping::MappingIndex::new(&self.roots, &self.policy);
        let files = self
            .files
            .keys()
            .filter(|path| mapping.root(path).is_none())
            .cloned()
            .collect::<Vec<_>>();
        self.roots.extend(files);
    }

    fn accept(&mut self, listing: DirectoryListing) -> Result<(), CliError> {
        for folder in listing.folders {
            self.directory_version(&folder.folder, folder.updated_at)?;
            self.add_directory(&folder.folder)?;
        }
        for file in listing.files {
            self.add_file(file)?;
        }
        Ok(())
    }

    async fn scan_directories(
        &mut self,
        receiver: &ShareReceiver,
        concurrency: usize,
    ) -> Result<(), CliError> {
        let mut pending = FuturesUnordered::new();
        loop {
            while pending.len() < concurrency {
                let Some(directory) = self.queue.pop_front() else {
                    break;
                };
                pending.push(read_owned_directory(receiver.clone(), directory));
            }
            let Some(result) = pending.next().await else {
                break;
            };
            self.accept(result?)?;
        }
        Ok(())
    }

    pub(super) fn finish(self) -> Result<Manifest, CliError> {
        mapping::build_manifest(self.roots, self.files, self.directories, self.policy)
    }
}

#[derive(Default)]
struct DirectoryListing {
    files: Vec<FileInfo>,
    folders: Vec<crate::domain::types::FolderInfo>,
}

async fn read_owned_directory(
    mut receiver: ShareReceiver,
    prefix: String,
) -> Result<DirectoryListing, CliError> {
    read_directory(&mut receiver, Some(prefix)).await
}

async fn read_directory(
    receiver: &mut ShareReceiver,
    prefix: Option<String>,
) -> Result<DirectoryListing, CliError> {
    let mut marker = None;
    let mut markers = BTreeSet::new();
    let mut listing = DirectoryListing::default();
    loop {
        // [Review Fix #ShareEnumerationSession1] Each independent prefix renews expired tokens without changing its pinned recipient.
        let page = receiver
            .list_page(ShareListFilesInput {
                prefix: prefix.clone(),
                delimiter: Some("/".into()),
                marker: marker.clone(),
                limit: Some(1000),
                list_root: Some(prefix.is_none()),
            })
            .await?;
        validate_scope(&page, prefix.as_deref())?;
        marker = next_marker(&page, &mut markers)?;
        listing.files.extend(page.files);
        listing.folders.extend(page.folders);
        if marker.is_none() {
            return Ok(listing);
        }
    }
}

fn validate_scope(page: &ListFilesOutput, prefix: Option<&str>) -> Result<(), CliError> {
    for file in &page.files {
        validate_scoped_path(&file.file_path, is_folder(file), prefix)?;
    }
    for folder in &page.folders {
        validate_scoped_path(&folder.folder, true, prefix)?;
    }
    Ok(())
}

fn validate_scoped_path(
    path: &str,
    is_directory: bool,
    prefix: Option<&str>,
) -> Result<(), CliError> {
    // [Review Fix #ShareManifestScope1] Canonical directory self markers remain in scope; regular files retain strict prefix boundaries.
    let canonical = if is_directory {
        directory_path(path)?
    } else {
        validate_relative(path)?;
        path.to_owned()
    };
    if prefix.is_some_and(|scope| scope != "/" && !canonical.starts_with(scope)) {
        return Err(invalid(
            "service returned a path outside the selected directory",
        ));
    }
    Ok(())
}

fn next_marker(
    page: &ListFilesOutput,
    markers: &mut BTreeSet<String>,
) -> Result<Option<String>, CliError> {
    if page.next_marker.is_empty() {
        if page.is_truncated {
            return Err(invalid("truncated page is missing NextMarker"));
        }
        return Ok(None);
    }
    if page.next_marker.len() > 4096 || page.next_marker.chars().any(char::is_control) {
        return Err(invalid(
            "pagination marker exceeds 4096 bytes or contains control characters",
        ));
    }
    if !markers.insert(page.next_marker.clone()) {
        return Err(invalid("cyclic pagination marker"));
    }
    Ok(Some(page.next_marker.clone()))
}

#[cfg(test)]
#[path = "share_manifest_scan_tests.rs"]
mod tests;
