//! Persisted mapping policy and deterministic projection of a share prescan.
use super::{invalid, validate_relative, Entry, Manifest, RemoteFile};
use crate::cli::share::ShareDownloadArgs;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use tos_core::agent::error::CliError;

/// Persist the exact local mapping and path filters so resumed snapshots cannot redefine them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum MappingPolicy {
    #[default]
    Legacy,
    Cp {
        is_include_parent: bool,
        include: Option<String>,
        exclude: Option<String>,
    },
    SpaceRoot {
        is_include_parent: bool,
        include: Option<String>,
        exclude: Option<String>,
    },
}

impl MappingPolicy {
    pub(super) fn from_args(args: &ShareDownloadArgs) -> Self {
        Self::Cp {
            is_include_parent: args.is_include_parent,
            include: args.include.clone(),
            exclude: args.exclude.clone(),
        }
    }

    pub(super) fn includes_parent(&self) -> bool {
        match self {
            Self::Legacy => true,
            Self::Cp {
                is_include_parent, ..
            } => *is_include_parent,
            Self::SpaceRoot { .. } => false,
        }
    }

    pub(super) fn into_space_root(self) -> Self {
        let (is_include_parent, include, exclude) = match self {
            Self::Legacy => (true, None, None),
            Self::Cp {
                is_include_parent,
                include,
                exclude,
            }
            | Self::SpaceRoot {
                is_include_parent,
                include,
                exclude,
            } => (is_include_parent, include, exclude),
        };
        Self::SpaceRoot {
            is_include_parent,
            include,
            exclude,
        }
    }

    pub(super) fn is_space_root(&self) -> bool {
        matches!(self, Self::SpaceRoot { .. })
    }

    pub(super) fn matches(&self, local: &str) -> bool {
        match self {
            Self::Legacy => true,
            Self::Cp {
                include, exclude, ..
            }
            | Self::SpaceRoot {
                include, exclude, ..
            } => super::super::high_level::path_matches_filters(
                local,
                include.as_deref(),
                exclude.as_deref(),
            ),
        }
    }

    pub(super) fn validate_options(
        &self,
        is_include_parent: bool,
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Result<(), CliError> {
        let is_matching = match self {
            Self::Legacy => is_include_parent && include.is_none() && exclude.is_none(),
            Self::Cp {
                is_include_parent: recorded,
                include: recorded_include,
                exclude: recorded_exclude,
            }
            | Self::SpaceRoot {
                is_include_parent: recorded,
                include: recorded_include,
                exclude: recorded_exclude,
            } => {
                *recorded == is_include_parent
                    && recorded_include.as_deref() == include
                    && recorded_exclude.as_deref() == exclude
            }
        };
        if !is_matching {
            return Err(invalid(
                "manifest mapping or filtering policy differs from this download",
            ));
        }
        Ok(())
    }
}

pub(super) struct MappingIndex<'a> {
    roots: HashSet<&'a str>,
    policy: &'a MappingPolicy,
}

impl<'a> MappingIndex<'a> {
    pub(super) fn new(roots: &'a [String], policy: &'a MappingPolicy) -> Self {
        Self {
            roots: roots.iter().map(String::as_str).collect(),
            policy,
        }
    }

    pub(super) fn local(&self, path: &str) -> Result<String, CliError> {
        let root = self
            .root(path)
            .ok_or_else(|| invalid("path is outside manifest roots"))?;
        if root == "/" {
            // [Review Fix #ShareSpaceMapping1] Space has no basename; keep the validated relative remote hierarchy for both include-parent modes.
            let local = path.strip_suffix('/').unwrap_or(path);
            if path != "/" {
                validate_relative(local)?;
            }
            return Ok(if path == "/" {
                String::new()
            } else {
                local.into()
            });
        }
        let root_name = root.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        let suffix = path.strip_prefix(root).unwrap_or("").trim_end_matches('/');
        let local = if !root.ends_with('/') || self.policy.includes_parent() {
            if suffix.is_empty() {
                root_name.to_owned()
            } else {
                format!("{root_name}/{suffix}")
            }
        } else {
            suffix.to_owned()
        };
        if !local.is_empty() {
            validate_relative(&local)?;
        }
        Ok(local)
    }

    pub(super) fn root(&self, path: &str) -> Option<&str> {
        if self.policy.is_space_root()
            && self.roots.contains("/")
            && (path == "/" || validate_relative(path.strip_suffix('/').unwrap_or(path)).is_ok())
        {
            return Some("/");
        }
        for (offset, byte) in path.bytes().enumerate() {
            if byte == b'/' {
                if let Some(root) = self.roots.get(&path[..=offset]) {
                    return Some(root);
                }
            }
        }
        self.roots.get(path).copied()
    }
}

pub(super) fn build_manifest(
    roots: Vec<String>,
    files: BTreeMap<String, RemoteFile>,
    directories: BTreeSet<String>,
    policy: MappingPolicy,
) -> Result<Manifest, CliError> {
    let mapping = MappingIndex::new(&roots, &policy);
    let mut kept_files = Vec::new();
    let mut kept_directories = BTreeMap::new();
    for remote in files.into_values() {
        let local = mapping.local(&remote.path)?;
        if policy.matches(&local) {
            retain_parents(&remote.path, false, &mapping, &mut kept_directories)?;
            kept_files.push(Entry { local, remote });
        }
    }
    for directory in directories {
        let local = mapping.local(&directory)?;
        if !local.is_empty() && policy.matches(&local) {
            retain_parents(&directory, true, &mapping, &mut kept_directories)?;
        }
    }
    let (remote_directories, directories) = kept_directories.into_iter().unzip();
    let manifest = Manifest {
        roots,
        files: kept_files,
        directories,
        remote_directories,
        policy,
    };
    manifest.validate()?;
    Ok(manifest)
}

fn retain_parents(
    remote: &str,
    is_directory: bool,
    mapping: &MappingIndex<'_>,
    retained: &mut BTreeMap<String, String>,
) -> Result<(), CliError> {
    let mut current = remote.trim_end_matches('/');
    if !is_directory {
        current = current.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
    while !current.is_empty() {
        let directory = format!("{current}/");
        if mapping.root(&directory).is_none() {
            break;
        }
        let local = mapping.local(&directory)?;
        if !local.is_empty() {
            retained.insert(directory, local);
        }
        current = current.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
    Ok(())
}

pub(super) fn validate_directory_policy(manifest: &Manifest) -> Result<(), CliError> {
    if matches!(manifest.policy, MappingPolicy::Legacy) {
        return Ok(());
    }
    let mut expected = BTreeSet::new();
    for entry in &manifest.files {
        if !manifest.policy.matches(&entry.local) {
            return Err(invalid("manifest file violates its filter policy"));
        }
        add_local_parents(&entry.local, false, &mut expected);
    }
    for directory in &manifest.directories {
        if manifest.policy.matches(directory) {
            add_local_parents(directory, true, &mut expected);
        }
    }
    let actual = manifest
        .directories
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(invalid(
            "manifest directories violate filters or omit necessary parents",
        ));
    }
    Ok(())
}

fn add_local_parents(path: &str, is_directory: bool, parents: &mut BTreeSet<String>) {
    let mut current = if is_directory {
        path
    } else {
        path.rsplit_once('/').map_or("", |(parent, _)| parent)
    };
    while !current.is_empty() {
        parents.insert(current.to_owned());
        current = current.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn space_manifest(is_include_parent: bool) -> Manifest {
        let files = ["docs/a.txt", "other/a.txt"]
            .map(|path| {
                let remote = RemoteFile::from_info(
                    serde_json::from_value(json!({"FilePath":path,"Size":1})).unwrap(),
                )
                .unwrap();
                (path.to_owned(), remote)
            })
            .into_iter()
            .collect();
        build_manifest(
            vec!["/".into()],
            files,
            ["docs/empty/".into()].into_iter().collect(),
            MappingPolicy::Cp {
                is_include_parent,
                include: None,
                exclude: None,
            }
            .into_space_root(),
        )
        .unwrap()
    }

    #[test]
    fn space_root_preserves_full_relative_paths_without_a_parent_basename() {
        for is_include_parent in [false, true] {
            let manifest = space_manifest(is_include_parent);
            assert_eq!(manifest.roots, ["/"]);
            assert_eq!(
                manifest
                    .files
                    .iter()
                    .map(|entry| entry.local.as_str())
                    .collect::<Vec<_>>(),
                ["docs/a.txt", "other/a.txt"]
            );
            assert_eq!(manifest.directories, ["docs", "docs/empty", "other"]);
            manifest
                .validate_options(is_include_parent, None, None)
                .unwrap();
            assert!(manifest
                .validate_options(!is_include_parent, None, None)
                .is_err());
            manifest.validate_selection(None, true).unwrap();
            manifest.validate_selection(Some("/"), true).unwrap();
            assert!(manifest.validate_selection(Some("/"), false).is_err());
            assert!(manifest.validate_selection(Some("docs/"), true).is_err());
            let restored: Manifest =
                serde_json::from_value(serde_json::to_value(manifest).unwrap()).unwrap();
            restored.validate().unwrap();
        }
    }

    #[test]
    fn only_space_root_policy_can_declare_the_single_root_sentinel() {
        let mut manifest = space_manifest(false);
        manifest.policy = MappingPolicy::Cp {
            is_include_parent: false,
            include: None,
            exclude: None,
        };
        assert!(manifest.validate().is_err());
        manifest.policy = MappingPolicy::Legacy;
        assert!(manifest.validate().is_err());
        let mut manifest = space_manifest(false);
        manifest.roots.push("docs/".into());
        assert!(manifest.validate().is_err());
        manifest.roots.clear();
        assert!(manifest.validate().is_err());
        assert!(validate_relative("/").is_err());
    }

    #[test]
    fn space_root_mapping_still_rejects_absolute_traversal_and_alias_paths() {
        let manifest = space_manifest(false);
        let mapping = MappingIndex::new(&manifest.roots, &manifest.policy);
        for path in [
            "/escape",
            "../escape",
            "docs/../escape",
            "docs//",
            "docs/CON",
            "C:escape",
        ] {
            assert!(mapping.local(path).is_err(), "accepted {path:?}");
            let mut tampered = manifest.clone();
            tampered.files[0].remote.path = path.into();
            assert!(tampered.validate().is_err(), "accepted {path:?}");
        }
    }

    #[test]
    fn empty_space_manifest_remains_recursive_and_preserves_filter_policy() {
        let policy = MappingPolicy::Cp {
            is_include_parent: false,
            include: Some("*.txt".into()),
            exclude: None,
        }
        .into_space_root();
        let manifest =
            build_manifest(vec!["/".into()], BTreeMap::new(), BTreeSet::new(), policy).unwrap();
        manifest.validate_selection(None, true).unwrap();
        assert!(manifest.validate_selection(None, false).is_err());
        manifest
            .validate_options(false, Some("*.txt"), None)
            .unwrap();
        assert!(manifest.validate_options(false, None, None).is_err());
    }

    fn projected(
        is_include_parent: bool,
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Manifest {
        let files = ["parent/root/sub/a.txt", "parent/root/sub/b.log"]
            .map(|path| {
                let remote = RemoteFile::from_info(
                    serde_json::from_value(json!({"FilePath":path,"Size":1})).unwrap(),
                )
                .unwrap();
                (path.to_owned(), remote)
            })
            .into_iter()
            .collect();
        build_manifest(
            vec!["parent/root/".into()],
            files,
            ["parent/root/", "parent/root/sub/", "parent/root/empty/"]
                .map(str::to_owned)
                .into_iter()
                .collect(),
            MappingPolicy::Cp {
                is_include_parent,
                include: include.map(str::to_owned),
                exclude: exclude.map(str::to_owned),
            },
        )
        .unwrap()
    }

    #[test]
    fn filters_keep_required_ancestors_and_drop_unmatched_empty_directories() {
        let manifest = projected(false, Some("*.txt"), Some("*sub"));
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].local, "sub/a.txt");
        assert_eq!(manifest.directories, ["sub"]);
        assert_eq!(manifest.remote_directories, ["parent/root/sub/"]);
        manifest
            .validate_options(false, Some("*.txt"), Some("*sub"))
            .unwrap();
    }

    #[test]
    fn include_parent_retains_exactly_one_directory_basename_and_filters_that_mapping() {
        let manifest = projected(true, Some("root/sub/*.txt"), None);
        assert_eq!(manifest.files[0].local, "root/sub/a.txt");
        assert_eq!(manifest.directories, ["root", "root/sub"]);
        assert_eq!(projected(false, None, None).directories, ["empty", "sub"]);
        assert_eq!(
            projected(true, None, None).directories,
            ["root", "root/empty", "root/sub"]
        );
    }

    #[test]
    fn persisted_policy_cannot_rebase_files_or_add_filtered_directories() {
        let mut manifest = projected(false, Some("*.txt"), None);
        assert!(manifest
            .validate_options(true, Some("*.txt"), None)
            .is_err());
        assert!(manifest.validate_options(false, None, None).is_err());
        manifest.files[0].local = "root/sub/a.txt".into();
        assert!(manifest.validate().is_err());
        let mut manifest = projected(false, Some("*.txt"), None);
        manifest.directories.push("empty".into());
        manifest
            .remote_directories
            .push("parent/root/empty/".into());
        assert!(manifest.validate().is_err());
        let mut manifest = projected(false, Some("*.txt"), None);
        manifest.directories.clear();
        manifest.remote_directories.clear();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn excluded_exact_file_is_a_valid_empty_selection_but_missing_matching_file_is_not() {
        let mut manifest = projected(false, None, None);
        manifest.roots = vec!["parent/root/a.txt".into()];
        manifest.files.clear();
        manifest.directories.clear();
        manifest.remote_directories.clear();
        assert!(manifest
            .validate_selection(Some("parent/root/a.txt"), false)
            .is_err());
        manifest.policy = MappingPolicy::Cp {
            is_include_parent: false,
            include: None,
            exclude: Some("*.txt".into()),
        };
        manifest.validate().unwrap();
        manifest
            .validate_selection(Some("parent/root/a.txt"), false)
            .unwrap();
    }

    #[test]
    fn malformed_fingerprints_duplicate_roots_and_overflow_are_rejected() {
        let mut manifest = projected(false, None, None);
        manifest.files[0].remote.etag = "line\nbreak".into();
        assert!(manifest.validate().is_err());
        let mut manifest = projected(false, None, None);
        manifest.roots.push(manifest.roots[0].clone());
        assert!(manifest.validate().is_err());
        let mut manifest = projected(false, None, None);
        manifest
            .files
            .iter_mut()
            .for_each(|entry| entry.remote.size = u64::MAX);
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn matching_declared_file_roots_require_metadata_and_legacy_policies_stay_explicit() {
        let mut manifest = projected(false, None, None);
        manifest.roots.push("other/missing.txt".into());
        assert!(manifest.validate().is_err());
        assert!(MappingPolicy::Legacy
            .validate_options(false, None, None)
            .is_err());
        assert!(MappingPolicy::Legacy
            .validate_options(true, Some("*"), None)
            .is_err());
        MappingPolicy::Legacy
            .validate_options(true, None, None)
            .unwrap();
    }
}
