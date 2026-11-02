//! Remote prescan and portable, unambiguous local path mapping.
use tos_core::agent::error::CliError;

/// Validate a raw relative API/local path; return an error for unsafe or unbounded components.

pub(crate) fn validate_relative(path: &str) -> Result<(), CliError> {
    if path.is_empty() || path.len() > 4096 || path.split('/').count() > 64 {
        return Err(invalid(
            "download path is empty or exceeds path/depth limit",
        ));
    }
    for component in path.split('/') {
        let base = component.split('.').next().unwrap_or("").to_uppercase();
        let is_device = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (base.len() == 4
                && (base.starts_with("COM") || base.starts_with("LPT"))
                && base.as_bytes()[3].is_ascii_digit());
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with(['.', ' '])
            || is_device
            || component
                .chars()
                .any(|letter| letter.is_control() || "\\:<>\"|?*".contains(letter))
        {
            return Err(invalid("unsafe or non-portable download path"));
        }
    }
    Ok(())
}

fn invalid(message: &str) -> CliError {
    CliError::ValidationError(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_paths_that_can_escape_or_alias_on_supported_filesystems() {
        for path in [
            "/a", "../a", "a/../b", "a//b", "a\\b", "a\0b", "C:a", "a/.", "a.", "CON",
        ] {
            assert!(validate_relative(path).is_err(), "accepted {path:?}");
        }
        assert!(validate_relative("中文/100%原样.txt").is_ok());
    }
}

use super::share_receive::ShareReceiver;
use crate::domain::types::FileInfo;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use unicode_normalization::UnicodeNormalization;

#[path = "share_manifest_mapping.rs"]
mod mapping;
#[path = "share_manifest_scan.rs"]
mod scan;
use mapping::MappingIndex;
pub(crate) use mapping::MappingPolicy;
#[cfg(test)]
use scan::Snapshot;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteFile {
    pub(crate) path: String,
    pub(crate) size: u64,
    pub(crate) etag: String,
    pub(crate) crc64: u64,
    pub(crate) updated_at: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub(crate) local: String,
    pub(crate) remote: RemoteFile,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) roots: Vec<String>,
    pub(crate) files: Vec<Entry>,
    pub(crate) directories: Vec<String>,
    pub(crate) remote_directories: Vec<String>,
    #[serde(default)]
    pub(crate) policy: MappingPolicy,
}
impl Manifest {
    /// Validate all remote/local mappings; reject collisions and malformed entries.
    pub(crate) fn validate(&self) -> Result<(), CliError> {
        let mapping = MappingIndex::new(&self.roots, &self.policy);
        self.validate_roots_and_directories(&mapping)?;
        mapping::validate_directory_policy(self)?;
        let mut remote_paths = BTreeSet::new();
        let mut paths = BTreeMap::new();
        for directory in &self.directories {
            insert_path(&mut paths, directory, true)?;
        }
        for entry in &self.files {
            entry.remote.validate()?;
            if !remote_paths.insert(&entry.remote.path)
                || mapping.local(&entry.remote.path)? != entry.local
            {
                return Err(invalid("invalid or duplicate manifest remote mapping"));
            }
            insert_path(&mut paths, &entry.local, false)?;
        }
        self.files.iter().try_fold(0u64, |total, entry| {
            total
                .checked_add(entry.remote.size)
                .ok_or_else(|| invalid("manifest total byte size overflows u64"))
        })?;
        Ok(())
    }
    fn validate_roots_and_directories(&self, mapping: &MappingIndex<'_>) -> Result<(), CliError> {
        // [Review Fix #Download6] Local directories need the same remote-to-local proof as files.
        if self.remote_directories.len() != self.directories.len() {
            return Err(invalid("directory mapping count differs"));
        }
        self.validate_roots(mapping)?;
        let mut seen_directories = BTreeSet::new();
        let mut seen_local = BTreeSet::new();
        for (remote, local) in self.remote_directories.iter().zip(&self.directories) {
            if !remote.ends_with('/')
                || !seen_directories.insert(remote)
                || !seen_local.insert(local)
            {
                return Err(invalid("invalid or duplicate directory mapping"));
            }
            validate_relative(remote.strip_suffix('/').unwrap_or(remote))?;
            if local.is_empty() || mapping.local(remote)? != *local {
                return Err(invalid("directory mapping differs from remote roots"));
            }
        }
        Ok(())
    }
    fn validate_roots(&self, mapping: &MappingIndex<'_>) -> Result<(), CliError> {
        // [Review Fix #ShareSpaceManifest1] Only the explicit SpaceRoot policy may persist `/`; legacy and ordinary shares retain relative roots.
        if self.policy.is_space_root() {
            return if self.roots.as_slice() == ["/"] {
                Ok(())
            } else {
                Err(invalid("Space-root manifest must declare exactly `/`"))
            };
        }
        let mut seen_roots = BTreeSet::new();
        // [Review Fix #ShareManifestScale1] Root proofs must remain logarithmic when prescans contain many individually shared files.
        let represented = self
            .remote_directories
            .iter()
            .chain(self.files.iter().map(|entry| &entry.remote.path))
            .collect::<BTreeSet<_>>();
        for root in &self.roots {
            validate_relative(root.strip_suffix('/').unwrap_or(root))?;
            let local = mapping.local(root)?;
            let needs_target = matches!(self.policy, MappingPolicy::Legacy)
                || (!local.is_empty() && self.policy.matches(&local));
            if !seen_roots.insert(root) || (needs_target && !represented.contains(root)) {
                return Err(invalid("duplicate or unrepresented manifest root"));
            }
        }
        Ok(())
    }
    /// Bind persisted mapping and filtering to immutable download options.
    /// Legacy snapshots require their historical include-parent mode without filters.
    pub(crate) fn validate_options(
        &self,
        is_include_parent: bool,
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Result<(), CliError> {
        self.policy
            .validate_options(is_include_parent, include, exclude)
    }
    /// Bind a validated snapshot to the immutable selection and recursion policy; reject impossible task shapes.
    pub(crate) fn validate_selection(
        &self,
        selection: Option<&str>,
        is_recursive: bool,
    ) -> Result<(), CliError> {
        if !is_recursive
            && (!self.directories.is_empty() || self.roots.iter().any(|root| root.ends_with('/')))
        {
            return Err(invalid("directory manifest requires recursive policy"));
        }
        let Some(selection) = selection else {
            return Ok(());
        };
        if self.roots.as_slice() != [selection] {
            return Err(invalid("manifest roots differ from selected target"));
        }
        if selection.ends_with('/') {
            if !is_recursive
                || (matches!(self.policy, MappingPolicy::Legacy)
                    && !self
                        .remote_directories
                        .iter()
                        .any(|directory| directory == selection))
            {
                return Err(invalid("selected directory is absent or nonrecursive"));
            }
        } else if !self.is_valid_exact_selection(selection)? {
            return Err(invalid(
                "exact file selection must contain exactly that file and no directories",
            ));
        }
        Ok(())
    }

    fn is_valid_exact_selection(&self, selection: &str) -> Result<bool, CliError> {
        let local = MappingIndex::new(&self.roots, &self.policy).local(selection)?;
        let expected_count = usize::from(self.policy.matches(&local));
        Ok(self.directories.is_empty()
            && self.files.len() == expected_count
            && self
                .files
                .iter()
                .all(|entry| entry.remote.path == selection))
    }
}
fn insert_path(
    paths: &mut BTreeMap<String, (String, bool)>,
    path: &str,
    is_directory: bool,
) -> Result<(), CliError> {
    validate_relative(path)?;
    let components: Vec<_> = path.split('/').collect();
    for count in 1..=components.len() {
        let original = components[..count].join("/");
        let folded: String = original.nfd().flat_map(char::to_uppercase).collect();
        let is_parent = count < components.len() || is_directory;
        if let Some((previous, was_directory)) = paths.get(&folded) {
            if previous != &original || !was_directory || !is_parent {
                return Err(invalid(
                    "download paths collide (case, Unicode, or file/directory)",
                ));
            }
        } else {
            paths.insert(folded, (original, is_parent));
        }
    }
    Ok(())
}
impl RemoteFile {
    fn validate(&self) -> Result<(), CliError> {
        validate_relative(&self.path)?;
        if self.etag.len() > 1024 || self.etag.chars().any(char::is_control) {
            return Err(invalid("invalid remote ETag"));
        }
        Ok(())
    }
    /// Extract the minimal remote fingerprint from metadata, rejecting unsafe paths or invalid sizes.
    pub(crate) fn from_info(info: FileInfo) -> Result<Self, CliError> {
        validate_relative(&info.file_path)?;
        let size = u64::try_from(info.size).map_err(|_| {
            invalid("remote file size is missing or negative; cannot build an exact download plan")
        })?;
        if info.etag.len() > 1024 || info.etag.chars().any(char::is_control) {
            return Err(invalid("invalid remote ETag"));
        }
        Ok(Self {
            path: info.file_path,
            size,
            etag: info.etag,
            crc64: info.hash_crc64_ecma,
            updated_at: info.updated_at,
        })
    }
    /// Return whether the fingerprint has a strong ETag or CRC64 suitable for recovery.
    pub(crate) fn has_identity(&self) -> bool {
        (!self.etag.is_empty() && !self.etag.starts_with("W/")) || self.crc64 != 0
    }
}
fn is_folder(info: &FileInfo) -> bool {
    info.file_type.eq_ignore_ascii_case("folder") || info.file_path.ends_with('/')
}
fn directory_path(path: &str) -> Result<String, CliError> {
    validate_relative(path.strip_suffix('/').unwrap_or(path))?;
    Ok(format!("{}/", path.trim_end_matches('/')))
}

/// Enumerate a download selection with cp-compatible filtering, mapping and concurrency.
/// Returns remote metadata, pagination, path-validation or incompatible-policy errors.
pub(crate) async fn enumerate_with_options(
    receiver: &mut ShareReceiver,
    args: &crate::cli::share::ShareDownloadArgs,
    declared_roots: Option<&[String]>,
) -> Result<Manifest, CliError> {
    scan::enumerate(
        receiver,
        scan::Options {
            selection: args.path.clone(),
            is_recursive: args.is_recursive,
            concurrency: args.list_concurrency.unwrap_or(4),
            policy: MappingPolicy::from_args(args),
        },
        declared_roots,
    )
    .await
}

/// Enumerate a legacy selection, retaining the historical root-basename mapping.
/// Accepts one authorized receiver and selection; propagates metadata and path-validation failures.
#[cfg(test)]
pub(crate) async fn enumerate(
    receiver: &mut ShareReceiver,
    selection: Option<&str>,
    is_recursive: bool,
    declared_roots: Option<&[String]>,
) -> Result<Manifest, CliError> {
    scan::enumerate(
        receiver,
        scan::Options {
            selection: selection.map(str::to_owned),
            is_recursive,
            concurrency: 1,
            policy: MappingPolicy::Legacy,
        },
        declared_roots,
    )
    .await
}
#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use crate::domain::share_access::fixture::{reply, Fixture};
    use tos_core::agent::global_args::GlobalArgs;
    #[test]
    fn prescan_supports_more_than_twenty_thousand_files_without_artifact_limits() {
        let mut snapshot = Snapshot::default();
        snapshot.roots.push("root/".into());
        snapshot.add_directory("root/").unwrap();
        for index in 0..20_001 {
            snapshot
                .add_file(
                    serde_json::from_value(serde_json::json!({
                        "FilePath":format!("root/{index}.txt"),"Size":1,"ETag":"etag",
                    }))
                    .unwrap(),
                )
                .unwrap();
        }
        assert_eq!(snapshot.finish().unwrap().files.len(), 20_001);
    }
    #[test]
    fn prescan_supports_snapshots_larger_than_eight_mibibytes() {
        let mut snapshot = Snapshot::default();
        snapshot.roots.push("root/".into());
        snapshot.add_directory("root/").unwrap();
        for index in 0..8_500 {
            snapshot
                .add_file(
                    serde_json::from_value(serde_json::json!({
                        "FilePath":format!("root/{index}.txt"),"Size":1,"ETag":"e".repeat(1024),
                    }))
                    .unwrap(),
                )
                .unwrap();
        }
        let manifest = snapshot.finish().unwrap();
        assert!(serde_json::to_vec(&manifest).unwrap().len() > 8 * 1024 * 1024);
        manifest.validate().unwrap();
    }
    #[test]
    fn rejects_case_normalization_and_ancestor_collisions() {
        for paths in [["A", "a"], ["é", "e\u{301}"], ["a", "a/b"]] {
            let manifest = Manifest {
                roots: paths.map(str::to_owned).into(),
                files: paths
                    .map(|path| Entry {
                        local: path.into(),
                        remote: RemoteFile {
                            path: path.into(),
                            size: 0,
                            etag: "tag".into(),
                            crc64: 0,
                            updated_at: 0,
                        },
                    })
                    .into(),
                directories: vec![],
                remote_directories: vec![],
                policy: Default::default(),
            };
            assert!(manifest.validate().is_err(), "accepted {paths:?}");
        }
    }
    #[tokio::test]
    async fn exact_selection_maps_basename_once() {
        let fixture = Fixture::new(vec![
            reply(200, r#"{"ShareID":"s"}"#),
            reply(200, r#"{"AccessToken":"token","ExpiresIn":60}"#),
            reply(
                200,
                r#"{"FilePath":"parent/中文%.txt","Size":3,"ETag":"tag"}"#,
            ),
        ]);
        let global = GlobalArgs::default();
        let mut receiver =
            ShareReceiver::new(&global, &format!("{}/share/s", fixture.endpoint)).unwrap();
        let info = receiver.info().await.unwrap();
        receiver.authorize(&global, &info, false).await.unwrap();
        let manifest = enumerate(&mut receiver, Some("parent/中文%.txt"), false, None)
            .await
            .unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].local, "中文%.txt");
        fixture.finish();
    }
}

#[cfg(test)]
mod tree_tests {
    use super::*;
    use crate::domain::share_access::fixture::{reply, Fixture};
    use tos_core::agent::global_args::GlobalArgs;
    async fn receiver(fixture: &Fixture) -> ShareReceiver {
        let global = GlobalArgs::default();
        let mut receiver =
            ShareReceiver::new(&global, &format!("{}/share/s", fixture.endpoint)).unwrap();
        let info = receiver.info().await.unwrap();
        receiver.authorize(&global, &info, false).await.unwrap();
        receiver
    }
    fn responses(pages: &[&str]) -> Vec<String> {
        let mut responses = vec![
            reply(200, r#"{"ShareID":"s"}"#),
            reply(200, r#"{"AccessToken":"t","ExpiresIn":60}"#),
        ];
        responses.extend(pages.iter().map(|page| reply(200, page)));
        responses
    }
    #[tokio::test]
    async fn download_options_use_contents_mapping_and_filter_local_relative_paths() {
        let fixture = Fixture::new(responses(&[
            r#"{"FilePath":"parent/root/","FileType":"folder"}"#,
            r#"{"Files":[{"FilePath":"parent/root/a.txt","Size":1},{"FilePath":"parent/root/b.log","Size":1}],"Folders":[{"Folder":"parent/root/empty/"}]}"#,
            r#"{}"#,
        ]));
        let args = crate::cli::share::ShareDownloadArgs {
            path: Some("parent/root/".into()),
            is_recursive: true,
            include: Some("*.txt".into()),
            ..Default::default()
        };
        let manifest = enumerate_with_options(&mut receiver(&fixture).await, &args, None)
            .await
            .unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].local, "a.txt");
        assert!(manifest.directories.is_empty());
        fixture.finish();
    }
    #[tokio::test]
    async fn download_options_prescan_more_than_twenty_thousand_files_across_service_pages() {
        let mut pages = vec![r#"{"FilePath":"root/","FileType":"folder"}"#.to_owned()];
        for page in 0..21 {
            let end = ((page + 1) * 1000).min(20_001);
            let files = (page * 1000..end)
                .map(|index| {
                    serde_json::json!({
                        "FilePath":format!("root/{index}.txt"),"Size":1,
                    })
                })
                .collect::<Vec<_>>();
            pages.push(serde_json::json!({"Files":files,"NextMarker":if page<20 {format!("page-{}",page+1)} else {String::new()},"IsTruncated":page<20}).to_string());
        }
        let fixture = Fixture::new(responses(
            &pages.iter().map(String::as_str).collect::<Vec<_>>(),
        ));
        let args = crate::cli::share::ShareDownloadArgs {
            path: Some("root/".into()),
            is_recursive: true,
            ..Default::default()
        };
        let manifest = enumerate_with_options(&mut receiver(&fixture).await, &args, None)
            .await
            .unwrap();
        assert_eq!(manifest.files.len(), 20_001);
        assert!(manifest
            .files
            .iter()
            .all(|entry| !entry.local.starts_with("root/")));
        assert_eq!(fixture.finish().len(), 24);
    }
    #[tokio::test]
    async fn root_tree_retains_empty_folders_pages_and_subtree_duplicates() {
        let fixture = Fixture::new(responses(&[
            r#"{"Folders":[{"Folder":"parent/root/"}],"NextMarker":"p2"}"#,
            r#"{"Files":[{"FilePath":"parent/root/a","Size":1,"ETag":"a"}]}"#,
            r#"{"Files":[{"FilePath":"parent/root/a","Size":1,"ETag":"a"},{"FilePath":"parent/root/empty/","FileType":"folder"}]}"#,
            r#"{"Files":[],"Folders":[]}"#,
        ]));
        let manifest = enumerate(&mut receiver(&fixture).await, None, true, None)
            .await
            .unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].local, "root/a");
        assert_eq!(manifest.directories, vec!["root", "root/empty"]);
        let requests = fixture.finish();
        assert!(requests[3].contains("marker=p2"));
        assert_eq!(requests.len(), 6);
    }
    #[tokio::test]
    async fn explicit_directory_preserves_name_once_and_requires_existence() {
        let fixture = Fixture::new(responses(&[
            r#"{"FilePath":"parent/child/","FileType":"folder"}"#,
            r#"{"Files":[]}"#,
        ]));
        let manifest = enumerate(
            &mut receiver(&fixture).await,
            Some("parent/child/"),
            true,
            None,
        )
        .await
        .unwrap();
        assert_eq!(manifest.directories, vec!["child"]);
        assert_eq!(fixture.finish().len(), 4);
        let fixture = Fixture::new(responses(&["{}"]));
        assert!(
            enumerate(&mut receiver(&fixture).await, Some("missing/"), true, None)
                .await
                .is_err()
        );
        fixture.finish();
    }
    #[tokio::test]
    async fn malformed_pagination_never_silently_truncates() {
        for pages in [
            vec![r#"{"IsTruncated":true}"#],
            vec![r#"{"NextMarker":"x"}"#, r#"{"NextMarker":"x"}"#],
        ] {
            let fixture = Fixture::new(responses(&pages));
            assert!(enumerate(&mut receiver(&fixture).await, None, true, None)
                .await
                .is_err());
            fixture.finish();
        }
    }
    #[tokio::test]
    async fn selected_subtree_rejects_out_of_scope_response() {
        let fixture = Fixture::new(responses(&[
            r#"{"FilePath":"selected/","FileType":"folder"}"#,
            r#"{"Files":[{"FilePath":"other/file","Size":1}]}"#,
        ]));
        assert!(
            enumerate(&mut receiver(&fixture).await, Some("selected/"), true, None)
                .await
                .is_err()
        );
        fixture.finish();
    }
}
