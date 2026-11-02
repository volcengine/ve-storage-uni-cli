//! Streaming CSV artifacts for receiver downloads, independent of terminal rendering.
use super::{
    high_level::{
        csv_record, rolled_csv_path, ADRIVE_REPORT_COLUMNS, DEFAULT_BATCH_FILE_ROLLOVER_BYTES,
    },
    share_local::{resolve_directory, SafeDir, StagedFile},
    share_manifest::{validate_relative, Manifest},
};
use crate::cli::share::ShareDownloadArgs;
use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};
use tos_core::agent::{error::CliError, global_args::GlobalArgs, request_id::sanitize_request_id};
use unicode_normalization::UnicodeNormalization;

const COMMAND: &str = "ve-adrive-cli share download";
const DEFAULT_REPORT_DIRECTORY: &str = "~/.tos/reports/ve-adrive";
const MANIFEST_COLUMNS: &[&str] = &[
    "command",
    "operation",
    "source",
    "destination",
    "size",
    "etag",
    "crc64",
];

#[derive(Clone, Debug)]
pub(super) struct Paths {
    pub(super) report_path: String,
    pub(super) manifest_path: Option<String>,
}

pub(super) struct Record<'a> {
    pub(super) source: &'a str,
    pub(super) destination: &'a Path,
    pub(super) status: &'a str,
    pub(super) error: Option<&'a str>,
    pub(super) request_id: Option<&'a str>,
}

pub(super) struct Artifacts {
    paths: Paths,
    report: PinnedCsvWriter,
    is_failures_only: bool,
}

impl Paths {
    /// Resolve CLI and profile artifact paths without writing files or loading credentials.
    /// Returns the expected execution paths, or a configuration/path validation error.
    pub(super) fn resolve(global: &GlobalArgs, args: &ShareDownloadArgs) -> Result<Self, CliError> {
        let (directory, format) = configured_artifact_settings(global)?;
        if args.report_path.is_none() && format != "csv" {
            return Err(invalid(
                "unsupported batch_report_format: only csv is supported",
            ));
        }
        // [Review Fix #ShareArtifacts1] Explicit outputs must not fail on an unused default directory.
        let needs_defaults =
            args.report_path.is_none() || (!args.is_no_manifest && args.manifest_path.is_none());
        let directory = if needs_defaults {
            predicted_report_directory(&directory)?
        } else {
            PathBuf::new()
        };
        let suffix = format!(
            "{}-{}.csv",
            std::process::id(),
            chrono::Utc::now().timestamp_millis()
        );
        let report_path = resolve_artifact(
            args.report_path.as_deref(),
            &directory,
            &format!("share-download-{suffix}"),
        )?;
        let manifest_path = if args.is_no_manifest {
            None
        } else {
            Some(resolve_artifact(
                args.manifest_path.as_deref(),
                &directory,
                &format!("share-download-manifest-{suffix}"),
            )?)
        };
        Ok(Self {
            report_path,
            manifest_path,
        })
    }

    /// Reject artifact volumes that can overwrite download mappings or checkpoint state.
    /// Resolves existing aliases without creating paths and returns native I/O or collision errors.
    pub(super) fn validate_against_manifest(
        &self,
        manifest: &Manifest,
        destination: &Path,
        checkpoint: Option<&Path>,
    ) -> Result<(), CliError> {
        let families = self.validated_families()?;
        let destination = normalize_path(destination)?;
        for relative in manifest
            .directories
            .iter()
            .chain(manifest.files.iter().map(|entry| &entry.local))
        {
            validate_relative(relative)?;
            // [Review Fix #ShareArtifacts10] Download descendants are checked per item without following aliases; conflicts must not abort unrelated jobs.
            let target = destination.join(relative);
            if families.iter().any(|family| family.overlaps(&target)) {
                return Err(invalid(
                    "report/manifest volume collides with download mapping",
                ));
            }
        }
        if let Some(checkpoint) = checkpoint {
            for protected in [
                checkpoint.to_owned(),
                PathBuf::from(format!("{}.lock", checkpoint.display())),
                // [Review Fix #ShareArtifacts6] Range state and every descendant must survive artifact writes and later cleanup.
                PathBuf::from(format!("{}.parts", checkpoint.display())),
                PathBuf::from(format!("{}.completed", checkpoint.display())),
            ] {
                let protected = normalize_path(&protected)?;
                if families.iter().any(|family| family.overlaps(&protected)) {
                    return Err(invalid(
                        "report/manifest volume collides with checkpoint/lock/recovery state",
                    ));
                }
            }
        }
        Ok(())
    }

    /// Create a fresh report for execution; returns the writer or an artifact collision/I/O error.
    pub(super) fn open(self, is_failures_only: bool) -> Result<Artifacts, CliError> {
        self.validated_families()?;
        let report = PinnedCsvWriter::new(&self.report_path, ADRIVE_REPORT_COLUMNS)?;
        Ok(Artifacts {
            paths: self,
            report,
            is_failures_only,
        })
    }

    fn validated_families(&self) -> Result<Vec<VolumeFamily>, CliError> {
        let mut families = vec![VolumeFamily::new(Path::new(&self.report_path))?];
        if let Some(path) = &self.manifest_path {
            let manifest = VolumeFamily::new(Path::new(path))?;
            // [Review Fix #ShareArtifacts3] Different first-volume extensions can still produce identical rollover filenames.
            let has_shared_volumes = families[0].parent == manifest.parent
                && families[0].part_prefix == manifest.part_prefix;
            if has_shared_volumes
                || families[0].overlaps(&manifest.base)
                || manifest.overlaps(&families[0].base)
            {
                return Err(invalid("report and manifest volumes collide"));
            }
            families.push(manifest);
        }
        Ok(families)
    }
}

impl Artifacts {
    /// Return the effective CSV artifact paths for aggregate output.
    pub(super) fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Write every planned file using the existing local mapping, independently of report filtering.
    /// Returns validation or storage errors; disabled manifests perform no writes.
    pub(super) fn write_manifest(
        &mut self,
        manifest: &Manifest,
        destination: &Path,
    ) -> Result<(), CliError> {
        let Some(path) = &self.paths.manifest_path else {
            return Ok(());
        };
        let mut writer = PinnedCsvWriter::new(path, MANIFEST_COLUMNS)?;
        for entry in &manifest.files {
            validate_relative(&entry.remote.path)?;
            validate_relative(&entry.local)?;
            writer.write_record(&[
                COMMAND.into(),
                "download".into(),
                entry.remote.path.clone(),
                destination
                    .join(&entry.local)
                    .to_string_lossy()
                    .into_owned(),
                entry.remote.size.to_string(),
                entry.remote.etag.clone(),
                (entry.remote.crc64 != 0)
                    .then(|| entry.remote.crc64.to_string())
                    .unwrap_or_default(),
            ])?;
        }
        writer.finish()?;
        Ok(())
    }

    /// Stream one outcome with only its supplied operation request ID.
    /// Rejects URL sources/invalid statuses and propagates CSV write failures.
    pub(super) fn record(&mut self, record: Record<'_>) -> Result<(), CliError> {
        // [Review Fix #ShareArtifacts9] Directory failures retain a trailing slash while validating the underlying relative path.
        let is_directory = record.source.ends_with('/');
        validate_relative(record.source.strip_suffix('/').unwrap_or(record.source))?;
        if !matches!(record.status, "succeeded" | "failed" | "skipped") {
            return Err(invalid("invalid download report status"));
        }
        if self.is_failures_only && record.status != "failed" {
            return Ok(());
        }
        self.report.write_record(&[
            COMMAND.into(),
            if is_directory { "mkdir" } else { "download" }.into(),
            record.source.into(),
            record.destination.to_string_lossy().into_owned(),
            record.status.into(),
            record.error.unwrap_or_default().into(),
            record
                .request_id
                .and_then(sanitize_request_id)
                .unwrap_or_default(),
        ])
    }

    /// Flush and synchronize the current report volume, returning any storage failure.
    pub(super) fn finish(&mut self) -> Result<(), CliError> {
        self.report.finish()
    }
}

struct PinnedCsvWriter {
    directory: SafeDir,
    base_leaf: PathBuf,
    header_line: String,
    current: Option<StagedFile>,
    current_part: usize,
    current_bytes: u64,
}

impl PinnedCsvWriter {
    fn new(path: &str, headers: &[&str]) -> Result<Self, CliError> {
        let path = Path::new(path);
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = resolve_directory(parent, true)?;
        let directory = SafeDir::open_resolved(&parent)?;
        let base_leaf = path
            .file_name()
            .ok_or_else(|| invalid("CSV output needs a filename"))?
            .into();
        // [Review Fix #ShareArtifacts7] Pin the parent and publish staged volumes atomically; never truncate an output path.
        let mut writer = Self {
            directory,
            base_leaf,
            header_line: csv_record(headers.iter().copied()),
            current: None,
            current_part: 1,
            current_bytes: 0,
        };
        writer.start_volume()?;
        Ok(writer)
    }

    fn start_volume(&mut self) -> Result<(), CliError> {
        let leaf = rolled_csv_path(&self.base_leaf, self.current_part);
        let leaf = leaf
            .to_str()
            .ok_or_else(|| invalid("CSV output filename must be UTF-8"))?;
        let mut staged = StagedFile::new(&self.directory, leaf)?;
        staged.file.write_all(self.header_line.as_bytes())?;
        self.current_bytes = self.header_line.len() as u64;
        self.current = Some(staged);
        Ok(())
    }

    fn write_record(&mut self, fields: &[String]) -> Result<(), CliError> {
        let line = csv_record(fields.iter().map(String::as_str));
        let bytes = line.len() as u64;
        if self.current_bytes > self.header_line.len() as u64
            && self.current_bytes.saturating_add(bytes) > DEFAULT_BATCH_FILE_ROLLOVER_BYTES
        {
            self.finish()?;
            self.current_part = self
                .current_part
                .checked_add(1)
                .ok_or_else(|| invalid("CSV volume count overflow"))?;
            self.start_volume()?;
        }
        let staged = self
            .current
            .as_mut()
            .ok_or_else(|| invalid("CSV writer is already finished"))?;
        if let Err(error) = staged.file.write_all(line.as_bytes()) {
            // A short failed write must not publish a malformed final CSV record.
            self.current = None;
            return Err(error.into());
        }
        self.current_bytes = self.current_bytes.saturating_add(bytes);
        Ok(())
    }

    fn finish(&mut self) -> Result<(), CliError> {
        if let Some(staged) = self.current.take() {
            staged.publish(true)?;
        }
        Ok(())
    }
}

fn invalid(message: &str) -> CliError {
    CliError::ValidationError(message.into())
}

fn configured_artifact_settings(global: &GlobalArgs) -> Result<(String, String), CliError> {
    // [Review Fix #ShareArtifacts8] Reports inherit the same scoped/shared transfer settings and low-priority environment as cp.
    let profile = super::share_transfer_options::local_profile(global)?;
    let directory = profile
        .batch_report_dir
        .or_else(|| std::env::var("ADRIVE_BATCH_REPORT_DIR").ok())
        .unwrap_or_else(|| DEFAULT_REPORT_DIRECTORY.into());
    let format = profile
        .batch_report_format
        .or_else(|| std::env::var("ADRIVE_BATCH_REPORT_FORMAT").ok())
        .unwrap_or_else(|| "csv".into());
    Ok((directory, format))
}

fn resolve_artifact(
    explicit: Option<&str>,
    directory: &Path,
    filename: &str,
) -> Result<String, CliError> {
    let path = match explicit {
        Some("") => return Err(invalid("artifact path must not be empty")),
        Some(path) => expand_user_path(path),
        None => directory.join(filename),
    };
    Ok(path.to_string_lossy().into_owned())
}

fn expand_user_path(path: &str) -> PathBuf {
    if path == "~" || path.starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(path.strip_prefix("~/").unwrap_or(""));
        }
    }
    PathBuf::from(path)
}

fn predicted_report_directory(raw: &str) -> Result<PathBuf, CliError> {
    // [Review Fix #ShareArtifacts5] Trimming separators turns a valid filesystem root into the current directory.
    if raw.is_empty() {
        return Err(invalid("batch_report_dir must not be empty"));
    }
    let path = expand_user_path(raw);
    let mut ancestor = path.as_path();
    loop {
        match fs::metadata(ancestor) {
            Ok(metadata) if metadata.permissions().readonly() => {
                return Ok(fallback_report_directory())
            }
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Ok(fallback_report_directory())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        match ancestor
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            Some(parent) => ancestor = parent,
            None => return Ok(path),
        }
    }
}

fn fallback_report_directory() -> PathBuf {
    let root = if Path::new("/private/tmp").exists() {
        PathBuf::from("/private/tmp")
    } else {
        std::env::temp_dir()
    };
    root.join("ve-storage-uni-cli/reports")
}

fn normalize_path(path: &Path) -> Result<PathBuf, CliError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => continue,
            Component::ParentDir => {
                resolved.pop();
            }
            component => resolved.push(component.as_os_str()),
        }
        match fs::canonicalize(&resolved) {
            Ok(canonical) => resolved = canonical,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(resolved)
}

fn folded(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .nfd()
        .flat_map(char::to_uppercase)
        .collect()
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = folded(left);
    let right = folded(right);
    left == right
        || left.starts_with(&format!("{right}/"))
        || right.starts_with(&format!("{left}/"))
}

struct VolumeFamily {
    base: PathBuf,
    parent: String,
    part_prefix: String,
}

impl VolumeFamily {
    fn new(path: &Path) -> Result<Self, CliError> {
        ensure_plain_output(path)?;
        let base = normalize_path(path)?;
        // [Review Fix #ShareArtifacts2] Rollover follows the requested name, even if the base CSV itself is an alias.
        let second = rolled_csv_path(path, 2);
        let filename = second
            .file_name()
            .ok_or_else(|| invalid("artifact path needs a filename"))?;
        let part_prefix = folded(Path::new(filename))
            .strip_suffix("0002.CSV")
            .ok_or_else(|| invalid("unexpected artifact rollover filename"))?
            .to_owned();
        let requested_parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let requested_parent = normalize_path(requested_parent)?;
        let family = Self {
            parent: folded(&requested_parent),
            base,
            part_prefix,
        };
        family.check_existing_volumes(&requested_parent)?;
        Ok(family)
    }

    fn check_existing_volumes(&self, parent: &Path) -> Result<(), CliError> {
        let entries = match fs::read_dir(parent) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        // [Review Fix #ShareArtifacts4] Existing future volumes must not redirect truncation through links.
        for entry in entries {
            let path = entry?.path();
            if self.contains_volume(&path) {
                ensure_plain_output(&path)?;
            }
        }
        Ok(())
    }

    fn contains_volume(&self, path: &Path) -> bool {
        if folded(path.parent().unwrap_or(Path::new(""))) != self.parent {
            return false;
        }
        let Some(filename) = path.file_name() else {
            return false;
        };
        let filename = folded(Path::new(filename));
        let Some(part) = filename
            .strip_prefix(&self.part_prefix)
            .and_then(|tail| tail.strip_suffix(".CSV"))
        else {
            return false;
        };
        part.len() >= 4
            && part.bytes().all(|byte| byte.is_ascii_digit())
            && part.parse::<usize>().is_ok_and(|part| part > 1)
    }

    fn overlaps(&self, target: &Path) -> bool {
        paths_overlap(&self.base, target)
            || target
                .ancestors()
                .any(|ancestor| self.contains_volume(ancestor))
    }
}

fn ensure_plain_output(path: &Path) -> Result<(), CliError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        return Err(invalid(
            "report/manifest volume must be an ordinary file, not a symlink or special entry",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err(invalid("report/manifest volume must not be hard-linked"));
        }
    }
    #[cfg(windows)]
    {
        use cap_fs_ext::MetadataExt;
        let file = fs::File::open(path)?;
        let metadata = cap_std::fs::Metadata::from_file(&file)?;
        if metadata.nlink() > 1 {
            return Err(invalid("report/manifest volume must not be hard-linked"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::share_manifest::{Entry, RemoteFile};
    use clap::{Args, FromArgMatches};
    use std::fs;

    struct TestDirectory(PathBuf);
    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("share-artifacts-{}", ulid::Ulid::new()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn paths(&self) -> Paths {
            Paths {
                report_path: self.0.join("report.csv").to_string_lossy().into_owned(),
                manifest_path: Some(self.0.join("manifest.csv").to_string_lossy().into_owned()),
            }
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn arguments(extra: &[&str]) -> ShareDownloadArgs {
        let mut command = vec![
            "download",
            "https://example.test/share/id?pwd=secret",
            "target",
        ];
        command.extend_from_slice(extra);
        let matches = ShareDownloadArgs::augment_args(clap::Command::new("download"))
            .try_get_matches_from(command)
            .unwrap();
        ShareDownloadArgs::from_arg_matches(&matches).unwrap()
    }

    fn manifest() -> Manifest {
        Manifest {
            policy: Default::default(),
            roots: vec!["remote/file.txt".into()],
            files: vec![Entry {
                local: "local/file.txt".into(),
                remote: RemoteFile {
                    path: "remote/file.txt".into(),
                    size: 12,
                    etag: "etag".into(),
                    crc64: 42,
                    updated_at: 0,
                },
            }],
            ..Manifest::default()
        }
    }

    #[test]
    fn resolve_reads_artifact_settings_without_credentials_or_writes() {
        let directory = TestDirectory::new();
        let expected = directory.0.join("not-created");
        let config = directory.0.join("config.toml");
        fs::write(
            &config,
            format!("[default.adrive]\nbatch_report_dir = {:?}\n", expected),
        )
        .unwrap();
        let global = GlobalArgs {
            config_path: Some(config),
            credentials_path: Some(directory.0.join("missing-credentials.toml")),
            dry_run: true,
            ..GlobalArgs::default()
        };
        let paths = Paths::resolve(&global, &arguments(&[])).unwrap();
        assert!(Path::new(&paths.report_path).starts_with(&expected));
        assert!(Path::new(paths.manifest_path.as_ref().unwrap()).starts_with(&expected));
        assert!(!expected.exists());
        assert!(!global.credentials_path.unwrap().exists());
    }

    #[test]
    fn explicit_paths_expand_home_and_no_manifest_keeps_report() {
        let global = GlobalArgs::default();
        let paths = Paths::resolve(
            &global,
            &arguments(&["--report-path", "~/share-report.csv", "--no-manifest"]),
        )
        .unwrap();
        assert_eq!(
            PathBuf::from(paths.report_path),
            PathBuf::from(std::env::var_os("HOME").unwrap()).join("share-report.csv")
        );
        assert!(paths.manifest_path.is_none());
    }

    #[test]
    fn report_streams_rows_and_retains_only_supplied_request_id() {
        let directory = TestDirectory::new();
        let report = directory.0.join("report.csv");
        let destination = directory.0.join("local/file.txt");
        let mut artifacts = directory.paths().open(false).unwrap();
        for (status, request_id) in [("succeeded", Some("request-one")), ("skipped", None)] {
            artifacts
                .record(Record {
                    source: "remote/file.txt",
                    destination: &destination,
                    status,
                    error: None,
                    request_id,
                })
                .unwrap();
        }
        artifacts.finish().unwrap();
        let contents = fs::read_to_string(report).unwrap();
        assert_eq!(contents.lines().count(), 3);
        assert!(
            contents.starts_with("command,operation,source,destination,status,error,request_id\n")
        );
        assert!(contents.contains("ve-adrive-cli share download,download,remote/file.txt,"));
        assert!(contents.lines().nth(1).unwrap().ends_with(",request-one"));
        assert!(contents.lines().nth(2).unwrap().ends_with(",skipped,,"));
        assert!(!contents.contains("secret"));
    }

    #[test]
    fn failures_only_does_not_filter_manifest() {
        let directory = TestDirectory::new();
        let mut artifacts = directory.paths().open(true).unwrap();
        let destination = directory.0.join("local/file.txt");
        for (status, error) in [("succeeded", None), ("failed", Some("permission, denied"))] {
            artifacts
                .record(Record {
                    source: "remote/file.txt",
                    destination: &destination,
                    status,
                    error,
                    request_id: Some("request-id"),
                })
                .unwrap();
        }
        artifacts.write_manifest(&manifest(), &directory.0).unwrap();
        artifacts.finish().unwrap();
        let report = fs::read_to_string(&artifacts.paths().report_path).unwrap();
        let csv_manifest =
            fs::read_to_string(artifacts.paths().manifest_path.as_ref().unwrap()).unwrap();
        assert_eq!(report.lines().count(), 2);
        assert!(report.contains("\"permission, denied\""));
        assert_eq!(csv_manifest.lines().count(), 2);
        assert!(csv_manifest.contains(",12,etag,42\n"));
        assert!(csv_manifest.contains(destination.to_str().unwrap()));
    }

    #[test]
    fn new_report_replaces_old_header_instead_of_appending() {
        let directory = TestDirectory::new();
        let paths = directory.paths();
        fs::write(&paths.report_path, "old,header\nold,row\n").unwrap();
        let mut artifacts = paths.open(false).unwrap();
        artifacts.finish().unwrap();
        assert_eq!(
            fs::read_to_string(&artifacts.paths().report_path).unwrap(),
            "command,operation,source,destination,status,error,request_id\n"
        );
    }

    #[test]
    fn rejects_download_checkpoint_and_rolled_volume_collisions() {
        let directory = TestDirectory::new();
        let snapshot = manifest();
        for report in ["local/file.txt", "local", "local/file.txt/child"] {
            let paths = Paths {
                report_path: directory.0.join(report).to_string_lossy().into_owned(),
                manifest_path: None,
            };
            assert!(
                paths
                    .validate_against_manifest(&snapshot, &directory.0, None)
                    .is_err(),
                "accepted {report}"
            );
        }
        let paths = directory.paths();
        let mut collision = snapshot.clone();
        collision.files[0].local = "REPORT.part-0002.csv".into();
        assert!(paths
            .validate_against_manifest(&collision, &directory.0, None)
            .is_err());
        let mut paths = directory.paths();
        paths.report_path = directory
            .0
            .join("checkpoint.lock")
            .to_string_lossy()
            .into_owned();
        assert!(paths
            .validate_against_manifest(
                &snapshot,
                &directory.0,
                Some(&directory.0.join("checkpoint"))
            )
            .is_err());
    }

    #[test]
    fn rejects_same_report_and_manifest_and_local_open_errors() {
        let directory = TestDirectory::new();
        let mut paths = directory.paths();
        paths.manifest_path = Some(paths.report_path.clone());
        assert!(paths
            .validate_against_manifest(&manifest(), &directory.0, None)
            .is_err());
        let mut paths = directory.paths();
        paths.report_path = directory.0.to_string_lossy().into_owned();
        assert!(paths.open(false).is_err());
    }

    #[test]
    fn refuses_secret_urls_as_sources_and_invalid_statuses() {
        let directory = TestDirectory::new();
        let mut artifacts = directory.paths().open(false).unwrap();
        for (source, status) in [
            ("https://example.test/share/id?pwd=secret", "failed"),
            ("remote/file.txt", "unknown"),
        ] {
            assert!(artifacts
                .record(Record {
                    source,
                    destination: &directory.0,
                    status,
                    error: None,
                    request_id: None
                })
                .is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn collision_check_resolves_existing_symlink_ancestors() {
        let directory = TestDirectory::new();
        fs::create_dir(directory.0.join("actual")).unwrap();
        std::os::unix::fs::symlink(directory.0.join("actual"), directory.0.join("alias")).unwrap();
        let paths = Paths {
            report_path: directory
                .0
                .join("alias/file.txt")
                .to_string_lossy()
                .into_owned(),
            manifest_path: None,
        };
        let mut snapshot = manifest();
        snapshot.files[0].local = "file.txt".into();
        assert!(paths
            .validate_against_manifest(&snapshot, &directory.0.join("actual"), None)
            .is_err());
    }

    #[test]
    fn explicit_paths_do_not_depend_on_unused_default_directory() {
        let directory = TestDirectory::new();
        let blocker = directory.0.join("ordinary-file");
        fs::write(&blocker, "file").unwrap();
        let config = directory.0.join("config.toml");
        fs::write(
            &config,
            format!(
                "[default.adrive]\nbatch_report_dir = {:?}\n",
                blocker.join("child")
            ),
        )
        .unwrap();
        let global = GlobalArgs {
            config_path: Some(config),
            ..GlobalArgs::default()
        };
        let report = directory.0.join("chosen-report.csv");
        let paths = Paths::resolve(
            &global,
            &arguments(&["--report-path", report.to_str().unwrap(), "--no-manifest"]),
        )
        .unwrap();
        assert_eq!(Path::new(&paths.report_path), report);
    }

    #[cfg(unix)]
    #[test]
    fn rollover_collision_uses_requested_name_even_when_base_is_symlink() {
        let directory = TestDirectory::new();
        fs::write(directory.0.join("actual.csv"), "old").unwrap();
        std::os::unix::fs::symlink(
            directory.0.join("actual.csv"),
            directory.0.join("report.csv"),
        )
        .unwrap();
        let mut snapshot = manifest();
        snapshot.files[0].local = "report.part-0002.csv".into();
        assert!(directory
            .paths()
            .validate_against_manifest(&snapshot, &directory.0, None)
            .is_err());
    }

    #[test]
    fn rejects_distinct_extensions_that_share_rollover_names() {
        let directory = TestDirectory::new();
        let mut paths = directory.paths();
        for filename in ["report.json", "report"] {
            paths.manifest_path = Some(directory.0.join(filename).to_string_lossy().into_owned());
            assert!(paths
                .validate_against_manifest(&manifest(), &directory.0, None)
                .is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_linked_output_volumes_before_they_can_overwrite_other_files() {
        let directory = TestDirectory::new();
        let target = directory.0.join("target.txt");
        fs::write(&target, "retain").unwrap();
        let rollover = directory.0.join("report.part-0002.csv");
        std::os::unix::fs::symlink(&target, &rollover).unwrap();
        assert!(directory.paths().open(false).is_err());
        fs::remove_file(&rollover).unwrap();
        fs::hard_link(&target, &rollover).unwrap();
        assert!(directory.paths().open(false).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "retain");
    }

    #[test]
    fn report_directory_keeps_filesystem_root_and_rejects_empty_setting() {
        let root = std::env::current_dir()
            .unwrap()
            .ancestors()
            .last()
            .unwrap()
            .to_owned();
        assert_eq!(
            predicted_report_directory(root.to_str().unwrap()).unwrap(),
            root
        );
        assert!(predicted_report_directory("").is_err());
    }

    #[test]
    fn refuses_csv_bases_and_rollovers_inside_range_checkpoint_state() {
        let directory = TestDirectory::new();
        let checkpoint = directory.0.join("checkpoint.json");
        let parts = directory.0.join("checkpoint.json.parts");
        for relative in [
            "receipt.json",
            "nested/report.csv",
            "nested/report.part-0002.csv",
        ] {
            let mut paths = directory.paths();
            paths.report_path = parts.join(relative).to_string_lossy().into_owned();
            assert!(
                paths
                    .validate_against_manifest(&manifest(), &directory.0, Some(&checkpoint))
                    .is_err(),
                "accepted report {relative}"
            );
            let mut paths = directory.paths();
            paths.manifest_path = Some(parts.join(relative).to_string_lossy().into_owned());
            assert!(
                paths
                    .validate_against_manifest(&manifest(), &directory.0, Some(&checkpoint))
                    .is_err(),
                "accepted manifest {relative}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn publication_never_follows_a_leaf_replaced_after_open() {
        let directory = TestDirectory::new();
        let sentinel = directory.0.join("outside.txt");
        fs::write(&sentinel, "sentinel").unwrap();
        for is_symlink in [true, false] {
            let paths = directory.paths();
            let report = PathBuf::from(&paths.report_path);
            let mut artifacts = paths.open(false).unwrap();
            if report.exists() {
                fs::remove_file(&report).unwrap();
            }
            if is_symlink {
                std::os::unix::fs::symlink(&sentinel, &report).unwrap();
            } else {
                fs::hard_link(&sentinel, &report).unwrap();
            }
            artifacts
                .record(Record {
                    source: "file.txt",
                    destination: &directory.0,
                    status: "succeeded",
                    error: None,
                    request_id: None,
                })
                .unwrap();
            let result = artifacts.finish();
            assert_eq!(fs::read_to_string(&sentinel).unwrap(), "sentinel");
            if result.is_ok() {
                assert!(fs::read_to_string(&report)
                    .unwrap()
                    .starts_with("command,operation,"));
            }
            fs::remove_file(&report).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn rollover_never_truncates_a_link_inserted_after_open() {
        let long_error = "x".repeat(26 * 1024 * 1024);
        for is_symlink in [true, false] {
            let directory = TestDirectory::new();
            let sentinel = directory.0.join("outside.txt");
            fs::write(&sentinel, "sentinel").unwrap();
            let mut artifacts = directory.paths().open(false).unwrap();
            let volume = directory.0.join("report.part-0002.csv");
            if is_symlink {
                std::os::unix::fs::symlink(&sentinel, &volume).unwrap();
            } else {
                fs::hard_link(&sentinel, &volume).unwrap();
            }
            let result = (|| -> Result<(), CliError> {
                for _ in 0..2 {
                    artifacts.record(Record {
                        source: "file.txt",
                        destination: &directory.0,
                        status: "failed",
                        error: Some(&long_error),
                        request_id: None,
                    })?;
                }
                artifacts.finish()
            })();
            assert_eq!(
                fs::metadata(&sentinel).unwrap().len(),
                8,
                "rollover changed the linked sentinel"
            );
            assert_eq!(fs::read_to_string(&sentinel).unwrap(), "sentinel");
            if result.is_ok() {
                assert!(fs::read_to_string(&volume)
                    .unwrap()
                    .starts_with("command,operation,"));
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn atomic_output_accepts_user_selected_directory_aliases() {
        let directory = TestDirectory::new();
        fs::create_dir(directory.0.join("actual")).unwrap();
        std::os::unix::fs::symlink(directory.0.join("actual"), directory.0.join("alias")).unwrap();
        let paths = Paths {
            report_path: directory
                .0
                .join("alias/report.csv")
                .to_string_lossy()
                .into_owned(),
            manifest_path: None,
        };
        let mut artifacts = paths.open(false).unwrap();
        artifacts.finish().unwrap();
        assert!(fs::read_to_string(directory.0.join("actual/report.csv"))
            .unwrap()
            .starts_with("command,operation,"));
    }

    #[test]
    fn failed_csv_writes_return_io_without_publishing_an_incomplete_volume() {
        let directory = TestDirectory::new();
        let sentinel = directory.0.join("readonly.txt");
        fs::write(&sentinel, "sentinel").unwrap();
        let mut artifacts = directory.paths().open(false).unwrap();
        artifacts.report.current.as_mut().unwrap().file = fs::File::open(&sentinel).unwrap();
        let result = artifacts.record(Record {
            source: "file.txt",
            destination: &directory.0,
            status: "failed",
            error: None,
            request_id: None,
        });
        assert!(matches!(result, Err(CliError::Io(_))));
        artifacts.finish().unwrap();
        assert!(!Path::new(&artifacts.paths().report_path).exists());
        assert_eq!(fs::read_to_string(sentinel).unwrap(), "sentinel");
    }

    #[test]
    fn artifact_settings_inherit_shared_transfer_fields_and_adrive_overrides() {
        let directory = TestDirectory::new();
        let config = directory.0.join("config.toml");
        let shared = directory.0.join("shared");
        let adrive = directory.0.join("adrive");
        let global = GlobalArgs {
            config_path: Some(config.clone()),
            ..GlobalArgs::default()
        };
        fs::write(
            &config,
            format!("[default]\nbatch_report_dir={shared:?}\nbatch_report_format='csv'\n"),
        )
        .unwrap();
        let paths = Paths::resolve(&global, &arguments(&[])).unwrap();
        assert!(Path::new(&paths.report_path).starts_with(&shared));
        fs::write(&config, format!("[default]\nbatch_report_dir={shared:?}\nbatch_report_format='invalid'\n[default.adrive]\nbatch_report_dir={adrive:?}\nbatch_report_format='csv'\n")).unwrap();
        let paths = Paths::resolve(&global, &arguments(&[])).unwrap();
        assert!(Path::new(&paths.report_path).starts_with(&adrive));
    }

    #[test]
    fn artifact_paths_cannot_overlap_completion_ledgers() {
        let directory = TestDirectory::new();
        let checkpoint = directory.0.join("checkpoint.json");
        for relative in [
            "checkpoint.json.completed",
            "checkpoint.json.completed/part.csv",
        ] {
            let mut paths = directory.paths();
            paths.report_path = directory.0.join(relative).to_string_lossy().into_owned();
            assert!(
                paths
                    .validate_against_manifest(&manifest(), &directory.0, Some(&checkpoint))
                    .is_err(),
                "accepted report {relative}"
            );
            let mut paths = directory.paths();
            paths.manifest_path = Some(directory.0.join(relative).to_string_lossy().into_owned());
            assert!(
                paths
                    .validate_against_manifest(&manifest(), &directory.0, Some(&checkpoint))
                    .is_err(),
                "accepted manifest {relative}"
            );
        }
    }

    #[test]
    fn directory_failures_are_csv_mkdir_rows_with_safe_directory_sources() {
        let directory = TestDirectory::new();
        let mut artifacts = directory.paths().open(false).unwrap();
        artifacts
            .record(Record {
                source: "folder/",
                destination: &directory.0.join("folder"),
                status: "failed",
                error: Some("not a directory"),
                request_id: None,
            })
            .unwrap();
        for source in [
            "/",
            "///",
            "../",
            "folder//",
            "https://example.test/share/s/",
        ] {
            assert!(artifacts
                .record(Record {
                    source,
                    destination: &directory.0,
                    status: "failed",
                    error: None,
                    request_id: None
                })
                .is_err());
        }
        artifacts.finish().unwrap();
        let csv = fs::read_to_string(&artifacts.paths().report_path).unwrap();
        assert!(csv
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("ve-adrive-cli share download,mkdir,folder/,"));
        assert_eq!(csv.lines().count(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn download_parent_conflicts_do_not_abort_artifact_collision_validation() {
        let directory = TestDirectory::new();
        let mut snapshot = manifest();
        snapshot.files[0].local = "sub/file.txt".into();
        fs::write(directory.0.join("sub"), "ordinary file").unwrap();
        assert!(directory
            .paths()
            .validate_against_manifest(&snapshot, &directory.0, None)
            .is_ok());
        fs::remove_file(directory.0.join("sub")).unwrap();
        fs::write(directory.0.join("outside"), "sentinel").unwrap();
        std::os::unix::fs::symlink(directory.0.join("outside"), directory.0.join("sub")).unwrap();
        assert!(directory
            .paths()
            .validate_against_manifest(&snapshot, &directory.0, None)
            .is_ok());
        snapshot.files[0].local = "report.csv".into();
        assert!(directory
            .paths()
            .validate_against_manifest(&snapshot, &directory.0, None)
            .is_err());
    }
}
