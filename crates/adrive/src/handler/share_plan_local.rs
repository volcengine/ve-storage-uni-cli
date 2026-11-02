//! Read-only native path inspection for share download plans.
use super::share_local::SafeDir;
use std::path::{Component, Path, PathBuf};
use tos_core::agent::error::CliError;

pub(super) struct PlanRoot {
    pub path: PathBuf,
    pub directory: Option<SafeDir>,
}
impl PlanRoot {
    /// Resolve OS aliases in the user-selected root without creating missing directories; pin an existing root.
    pub fn open(path: &Path) -> Result<Self, CliError> {
        let resolved = resolve_directory(path)?;
        let directory = match std::fs::symlink_metadata(&resolved) {
            Ok(_) => Some(SafeDir::open_resolved(&resolved)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path: resolved,
            directory,
        })
    }
    /// Classify a selected file without creating or following downloaded descendants; return a local conflict or native I/O error.
    pub fn file_action(&self, path: &str, can_overwrite: bool) -> Result<&'static str, CliError> {
        let Some(directory) = &self.directory else {
            return Ok("create");
        };
        let (parent, name) = match directory.parent(path, false) {
            Ok(parent) => parent,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok("create"),
            Err(error) => return Err(path_error(error)),
        };
        match parent.ordinary(&name) {
            Ok(false) => Ok("create"),
            Ok(true) if can_overwrite => Ok("overwrite"),
            Ok(true) => Err(CliError::Conflict(
                "destination file exists; use --overwrite or a matching completed checkpoint"
                    .into(),
            )),
            Err(error) => Err(path_error(error)),
        }
    }
    /// Inspect a selected directory without creating it or following a downloaded link.
    pub fn directory_action(&self, path: &str) -> Result<&'static str, CliError> {
        let Some(directory) = &self.directory else {
            return Ok("create");
        };
        match directory.directory(path, false) {
            Ok(_) => Ok("existing"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("create"),
            Err(error) => Err(path_error(error)),
        }
    }
}
fn path_error(error: std::io::Error) -> CliError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::NotADirectory
    ) {
        return CliError::Conflict("[share_local_path_conflict] destination has a nonordinary entry or an unsafe ancestor; inspect the local path or choose another destination".into());
    }
    // [Review Fix #D02] Preserve native permission/resource/I/O failures instead of classifying every inspection failure as a conflict.
    error.into()
}

/// Resolve the existing native prefix, then lexically append missing components; perform no filesystem mutation.
pub(super) fn resolve_directory(path: &Path) -> Result<PathBuf, CliError> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    for ancestor in absolute.ancestors() {
        match std::fs::canonicalize(ancestor) {
            Ok(resolved) => return resolve_suffix(&absolute, ancestor, resolved),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                reject_dangling_alias(ancestor)?
            }
            Err(error) => return Err(path_error(error)),
        }
    }
    Err(CliError::ValidationError(
        "destination has no accessible directory ancestor".into(),
    ))
}
fn resolve_suffix(
    absolute: &Path,
    ancestor: &Path,
    mut resolved: PathBuf,
) -> Result<PathBuf, CliError> {
    if !std::fs::metadata(&resolved)?.is_dir() {
        return Err(CliError::Conflict(
            "[share_local_path_conflict] destination ancestor is not a directory".into(),
        ));
    }
    let suffix = absolute
        .strip_prefix(ancestor)
        .map_err(|_| CliError::ValidationError("invalid destination path".into()))?;
    append_missing(&mut resolved, suffix)?;
    Ok(resolved)
}
fn reject_dangling_alias(path: &Path) -> Result<(), CliError> {
    match std::fs::symlink_metadata(path) {
        // [Review Fix #D02] A dangling user-root alias cannot be created by execution; do not advertise a successful plan for it.
        Ok(_) => Err(CliError::Conflict("[share_local_path_conflict] destination contains an unresolved directory alias; fix the local path or choose another destination".into())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(path_error(error)),
    }
}
fn append_missing(resolved: &mut PathBuf, suffix: &Path) -> Result<(), CliError> {
    for component in suffix.components() {
        match component {
            Component::Normal(name) => resolved.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            _ => {
                return Err(CliError::ValidationError(
                    "invalid destination component".into(),
                ))
            }
        }
    }
    Ok(())
}

/// Resolve a checkpoint's parent without creating it; validate its leaf separately from the user-selected parent.
pub(super) fn checkpoint_path(path: &str) -> Result<PathBuf, CliError> {
    let path = Path::new(path);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CliError::ValidationError("invalid checkpoint filename".into()))?;
    super::share_manifest::validate_relative(name)?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(resolve_directory(parent)?.join(name))
}
