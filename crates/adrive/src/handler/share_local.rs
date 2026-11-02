//! Descriptor-relative no-follow destination access; never reopen a validated path globally.
#[cfg(windows)]
pub(crate) use super::share_local_windows::SafeDir;
#[cfg(unix)]
use rustix::{
    fd::OwnedFd,
    fs::{self, AtFlags, Mode, OFlags},
};
#[cfg(unix)]
use std::path::Path;
use std::{fs::File, io};

/// Mark an intentionally refused local entry as a conflict while preserving the native I/O interface.
/// The marker lets share output provide destination-specific recovery guidance.
pub(super) fn path_conflict(message: &str) -> io::Error {
    // [Review Fix #D04] Unsafe local types are actionable destination conflicts, not unclassified I/O failures.
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("[share_local_path_conflict] {message}"),
    )
}

/// Resolve a user-selected local directory with OS semantics, optionally creating it.
/// Returns its canonical path or the native permission/type/I/O error; remote subpaths never use this resolver.
pub(crate) fn resolve_directory(
    path: &std::path::Path,
    is_create: bool,
) -> io::Result<std::path::PathBuf> {
    // [Review Fix #LiveRoot1] User roots may contain system or configured aliases; only downloaded descendants require no-follow.
    if is_create {
        std::fs::create_dir_all(path)?;
    }
    std::fs::canonicalize(path)
}

#[cfg(unix)]
pub(crate) struct SafeDir {
    handle: OwnedFd,
}
#[cfg(unix)]
impl SafeDir {
    /// Resolve a user-selected directory, then pin its canonical no-follow components; return a handle or native I/O error.
    #[cfg(test)]
    pub(crate) fn open(path: &Path, is_create: bool) -> io::Result<Self> {
        let absolute = resolve_directory(path, is_create)?;
        Self::open_resolved(&absolute)
    }
    /// Pin an already-resolved absolute directory without following any changed component.
    /// Returns a native I/O error if a component is now a link, missing or not a directory.
    pub(crate) fn open_resolved(path: &Path) -> io::Result<Self> {
        // [Review Fix #LiveRoot3] Re-canonicalizing after choosing task identity could silently pin a different root.
        if !path.is_absolute() {
            return Err(io::Error::other("resolved directory must be absolute"));
        }
        let mut directory = Self {
            handle: fs::open("/", directory_flags(), Mode::empty())?,
        };
        for component in path.components() {
            match component {
                std::path::Component::RootDir | std::path::Component::CurDir => {}
                std::path::Component::Normal(name) => directory = directory.child(name, false)?,
                _ => return Err(io::Error::other("unsafe local directory component")),
            }
        }
        Ok(directory)
    }
    fn child(&self, name: &std::ffi::OsStr, is_create: bool) -> io::Result<Self> {
        match fs::openat(&self.handle, name, directory_flags(), Mode::empty()) {
            Ok(handle) => Ok(Self { handle }),
            Err(rustix::io::Errno::NOENT) if is_create => {
                match fs::mkdirat(&self.handle, name, Mode::from_raw_mode(0o700)) {
                    Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                    Err(error) => return Err(error.into()),
                }
                fs::fsync(&self.handle)?;
                Ok(Self {
                    handle: fs::openat(&self.handle, name, directory_flags(), Mode::empty())
                        .map_err(local_open_error)?,
                })
            }
            Err(error) => Err(local_open_error(error)),
        }
    }
    /// Resolve a validated relative directory beneath this handle, optionally creating it; reject symlinks.
    pub(crate) fn directory(&self, path: &str, is_create: bool) -> io::Result<Self> {
        super::share_manifest::validate_relative(path)
            .map_err(|_| io::Error::other("unsafe relative path"))?;
        let mut current = Self {
            handle: rustix::io::dup(&self.handle)?,
        };
        for component in path.split('/') {
            current = current.child(std::ffi::OsStr::new(component), is_create)?;
        }
        Ok(current)
    }
    /// Return the pinned parent and leaf for a relative path; optionally create parents and reject unsafe components.
    pub(crate) fn parent(&self, path: &str, is_create: bool) -> io::Result<(Self, String)> {
        super::share_manifest::validate_relative(path)
            .map_err(|_| io::Error::other("unsafe relative path"))?;
        match path.rsplit_once('/') {
            Some((parent, name)) => Ok((self.directory(parent, is_create)?, name.into())),
            None => Ok((
                Self {
                    handle: rustix::io::dup(&self.handle)?,
                },
                path.into(),
            )),
        }
    }
    /// Return whether the leaf exists as an ordinary file; reject symlinks and special files.
    pub(crate) fn ordinary(&self, name: &str) -> io::Result<bool> {
        match fs::statat(&self.handle, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if fs::FileType::from_raw_mode(stat.st_mode) == fs::FileType::RegularFile => {
                Ok(true)
            }
            Ok(_) => Err(path_conflict("destination is not an ordinary file")),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
    /// Open an existing ordinary leaf without following links; return the native file or I/O error.
    pub(crate) fn read(&self, name: &str) -> io::Result<File> {
        self.ordinary(name)?;
        let handle = fs::openat(
            &self.handle,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(local_open_error)?;
        let file = File::from(handle);
        if !file.metadata()?.is_file() {
            return Err(path_conflict("destination is not an ordinary file"));
        }
        Ok(file)
    }
    /// Read a private ledger alongside its held writer, without following links or accepting hard-linked aliases.
    pub(crate) fn read_shared(&self, name: &str) -> io::Result<File> {
        use std::os::unix::fs::MetadataExt;
        let file = self.read(name)?;
        if file.metadata()?.nlink() != 1 {
            return Err(path_conflict(
                "completion ledger must have exactly one link",
            ));
        }
        Ok(file)
    }
    /// Create a new mode-0600 leaf exclusively; fail rather than open any existing entry.
    pub(crate) fn exclusive(&self, name: &str) -> io::Result<File> {
        Ok(File::from(fs::openat(
            &self.handle,
            name,
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?))
    }
    /// Atomically publish the staged leaf with the requested overwrite policy and sync the directory; propagate I/O failures.
    pub(crate) fn replace(&self, source: &str, target: &str, is_overwrite: bool) -> io::Result<()> {
        self.ordinary(target)?;
        if is_overwrite {
            fs::renameat(&self.handle, source, &self.handle, target)?;
        } else {
            fs::linkat(&self.handle, source, &self.handle, target, AtFlags::empty())?;
            fs::unlinkat(&self.handle, source, AtFlags::empty())?;
        }
        fs::fsync(&self.handle)?;
        Ok(())
    }
    /// Unlink one temporary leaf relative to this directory handle; propagate I/O failures.
    pub(crate) fn remove(&self, name: &str) -> io::Result<()> {
        fs::unlinkat(&self.handle, name, AtFlags::empty()).map_err(Into::into)
    }
}
#[cfg(unix)]
fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

#[cfg(unix)]
fn local_open_error(error: rustix::io::Errno) -> io::Error {
    match error {
        rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR => {
            path_conflict("destination component is a symlink or has an unexpected file type")
        }
        error => error.into(),
    }
}

pub(crate) struct StagedFile {
    pub(crate) file: File,
    directory: SafeDir,
    name: String,
    target: String,
    is_published: bool,
}
impl StagedFile {
    /// Create an exclusive same-parent temporary file for the relative destination; return cleanup ownership or I/O error.
    pub(crate) fn new(directory: &SafeDir, path: &str) -> io::Result<Self> {
        let (parent, target) = directory.parent(path, true)?;
        let name = format!(".adrive-share-{}.partial", ulid::Ulid::new());
        let file = parent.exclusive(&name)?;
        Ok(Self {
            file,
            directory: parent,
            name,
            target,
            is_published: false,
        })
    }
    /// Sync and atomically publish the verified temporary file; errors retain cleanup ownership.
    pub(crate) fn publish(mut self, is_overwrite: bool) -> io::Result<()> {
        self.file.sync_all()?;
        self.directory
            .replace(&self.name, &self.target, is_overwrite)?;
        self.is_published = true;
        Ok(())
    }
}
impl Drop for StagedFile {
    fn drop(&mut self) {
        if !self.is_published {
            // Best-effort cleanup never follows a replaced ancestor or masks the original failure.
            if let Err(error) = self.directory.remove(&self.name) {
                if error.kind() != io::ErrorKind::NotFound {
                    eprintln!("share: temporary-file cleanup failed ({:?})", error.kind());
                }
            }
        }
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn fixture() -> std::path::PathBuf {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-local-{}", ulid::Ulid::new()));
        std::fs::create_dir(&root).unwrap();
        root
    }
    #[test]
    fn user_root_alias_is_resolved_but_remote_descendants_reject_links() {
        let root = fixture();
        assert!(SafeDir::open(&root.join("new/child"), true).is_ok());
        std::os::unix::fs::symlink(root.join("new"), root.join("link")).unwrap();
        let directory = SafeDir::open(&root.join("link/child"), true).unwrap();
        assert!(root.join("new/child").is_dir());
        std::os::unix::fs::symlink(root.join("new"), root.join("new/child/escape")).unwrap();
        assert!(directory.directory("escape/sub", true).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_system_directory_aliases_are_ordinary_user_roots() {
        let root = fixture();
        let selected =
            std::path::PathBuf::from("/var").join(root.strip_prefix("/private/var").unwrap());
        assert!(SafeDir::open(&selected, false).is_ok());
        assert!(SafeDir::open(Path::new("/tmp"), false).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replacing_resolved_root_with_a_link_cannot_change_the_anchor() {
        let root = fixture();
        std::fs::create_dir(root.join("selected")).unwrap();
        std::fs::create_dir(root.join("other")).unwrap();
        let resolved = resolve_directory(&root.join("selected"), false).unwrap();
        std::fs::rename(&resolved, root.join("held")).unwrap();
        std::os::unix::fs::symlink(root.join("other"), &resolved).unwrap();
        assert!(
            SafeDir::open_resolved(&resolved).is_err(),
            "re-resolving changed the root after its identity was chosen"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsafe_destination_entries_are_conflicts_but_missing_entries_remain_io_errors() {
        let root = fixture();
        std::fs::create_dir(root.join("folder")).unwrap();
        std::fs::write(root.join("ordinary"), b"keep").unwrap();
        std::os::unix::fs::symlink(root.join("ordinary"), root.join("link")).unwrap();
        let directory = SafeDir::open(&root, false).unwrap();
        let errors = [
            directory.ordinary("folder").unwrap_err(),
            directory.ordinary("link").unwrap_err(),
            directory.read("folder").unwrap_err(),
            directory.read("link").unwrap_err(),
            directory.directory("ordinary/child", true).err().unwrap(),
            directory.directory("link/child", true).err().unwrap(),
        ];
        for error in errors {
            assert_eq!(error.kind(), io::ErrorKind::AlreadyExists, "{error}");
            assert!(error.to_string().contains("[share_local_path_conflict]"));
        }
        assert_eq!(
            directory.read("missing").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(all(test, unix))]
mod publish_tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn no_clobber_and_atomic_overwrite_never_publish_partial_bytes() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-publish-{}", ulid::Ulid::new()));
        std::fs::create_dir(&root).unwrap();
        let directory = SafeDir::open(&root, false).unwrap();
        let mut staged = StagedFile::new(&directory, "file").unwrap();
        staged.file.write_all(b"new").unwrap();
        std::fs::write(root.join("file"), b"old").unwrap();
        assert!(staged.publish(false).is_err());
        assert_eq!(std::fs::read(root.join("file")).unwrap(), b"old");
        let mut staged = StagedFile::new(&directory, "file").unwrap();
        staged.file.write_all(b"new").unwrap();
        staged.publish(true).unwrap();
        assert_eq!(std::fs::read(root.join("file")).unwrap(), b"new");
        let mut aborted = StagedFile::new(&directory, "aborted").unwrap();
        aborted.file.write_all(b"half").unwrap();
        drop(aborted);
        assert!(!root.join("aborted").exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn parent_swap_and_leaf_symlink_cannot_redirect_publication() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-swap-{}", ulid::Ulid::new()));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("victim")).unwrap();
        let directory = SafeDir::open(&root, false).unwrap();
        let mut staged = StagedFile::new(&directory, "parent/file").unwrap();
        staged.file.write_all(b"safe").unwrap();
        std::fs::rename(root.join("parent"), root.join("held")).unwrap();
        std::os::unix::fs::symlink(root.join("victim"), root.join("parent")).unwrap();
        staged.publish(false).unwrap();
        assert!(!root.join("victim/file").exists());
        assert_eq!(std::fs::read(root.join("held/file")).unwrap(), b"safe");
        std::os::unix::fs::symlink(root.join("held/file"), root.join("leaf")).unwrap();
        assert!(directory.ordinary("leaf").is_err());
        assert!(directory.read("leaf").is_err());
        let staged = StagedFile::new(&directory, "leaf").unwrap();
        assert!(staged.publish(true).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod ledger_read_tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn shared_ledger_read_remains_read_only_while_the_writer_is_held() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-ledger-read-{}", ulid::Ulid::new()));
        let directory = SafeDir::open(&root, true).unwrap();
        let mut writer = directory.exclusive("ledger.bin").unwrap();
        writer.write_all(b"abc").unwrap();
        let mut reader = directory.read_shared("ledger.bin").unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"abc");
        assert!(reader.write_all(b"unauthorized").is_err());
        drop(reader);
        drop(writer);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn shared_ledger_reads_refuse_symlinks_and_hard_links() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-ledger-read-links-{}", ulid::Ulid::new()));
        let directory = SafeDir::open(&root, true).unwrap();
        std::fs::write(root.join("victim"), b"retain").unwrap();
        std::os::unix::fs::symlink(root.join("victim"), root.join("symbolic")).unwrap();
        std::fs::hard_link(root.join("victim"), root.join("hard")).unwrap();
        assert!(directory.read_shared("symbolic").is_err());
        assert!(directory.read_shared("hard").is_err());
        assert_eq!(std::fs::read(root.join("victim")).unwrap(), b"retain");
        std::fs::remove_dir_all(root).unwrap();
    }
}
