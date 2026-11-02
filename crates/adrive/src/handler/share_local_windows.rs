//! Windows capability backend. Every directory handle excludes DELETE sharing;
//! every opened component rejects all reparse tags, including junctions.
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{
    Dir, MetadataExt as CapMetadataExt, OpenOptions, OpenOptionsExt as CapOpenOptionsExt,
};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::{
    fs::File,
    io,
    path::{Component, Path},
    sync::Arc,
};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const FILE_SHARE_DELETE: u32 = 4;

pub(crate) struct SafeDir {
    directory: Arc<Dir>,
    ancestors: Vec<Arc<Dir>>,
}
impl SafeDir {
    /// Resolve the user's root, then pin canonical volume ancestors; reject subsequent reparse points and propagate I/O errors.
    #[cfg(test)]
    pub(crate) fn open(path: &Path, is_create: bool) -> io::Result<Self> {
        // [Review Fix #LiveRoot1] Apply the same user-root resolution policy before the Windows capability boundary.
        let absolute = super::share_local::resolve_directory(path, is_create)?;
        Self::open_resolved(&absolute)
    }
    /// Pin an already-resolved volume directory without re-resolving aliases; return a no-follow capability or I/O error.
    pub(crate) fn open_resolved(path: &Path) -> io::Result<Self> {
        // [Review Fix #LiveRoot3] The capability must identify the same canonical root used for collision/resume policy.
        let mut components = path.components();
        let prefix = components.next().ok_or_else(|| invalid("missing volume"))?;
        if !matches!(prefix, Component::Prefix(_)) || components.next() != Some(Component::RootDir)
        {
            return Err(invalid("absolute Windows volume path required"));
        }
        let mut root = std::path::PathBuf::from(prefix.as_os_str());
        root.push("\\");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(root)?;
        reject_reparse(&file, true)?;
        let mut directory = Self {
            directory: Arc::new(Dir::from_std_file(file)),
            ancestors: vec![],
        };
        for component in components {
            match component {
                Component::Normal(name) => directory = directory.child(name, false)?,
                Component::CurDir => {}
                _ => return Err(invalid("unsafe directory component")),
            }
        }
        Ok(directory)
    }
    fn child(&self, name: &std::ffi::OsStr, is_create: bool) -> io::Result<Self> {
        let opened = match self.directory.open_dir_nofollow(name) {
            Ok(opened) => opened,
            Err(error) if error.kind() == io::ErrorKind::NotFound && is_create => {
                match self.directory.create_dir(name) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                self.directory
                    .open_dir_nofollow(name)
                    .map_err(|error| self.classify_entry_error(name, true, error))?
            }
            Err(error) => return Err(self.classify_entry_error(name, true, error)),
        };
        let file = opened.into_std_file();
        reject_reparse(&file, true)?;
        let mut ancestors = self.ancestors.clone();
        ancestors.push(self.directory.clone());
        Ok(Self {
            directory: Arc::new(Dir::from_std_file(file)),
            ancestors,
        })
    }
    fn check_entry(&self, name: &std::ffi::OsStr, is_directory: bool) -> io::Result<()> {
        let metadata = self.directory.symlink_metadata(name)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (is_directory && !metadata.is_dir())
            || (!is_directory && !metadata.is_file())
        {
            return Err(super::share_local::path_conflict(
                "reparse point or unexpected destination file type",
            ));
        }
        Ok(())
    }
    fn classify_entry_error(
        &self,
        name: &std::ffi::OsStr,
        is_directory: bool,
        error: io::Error,
    ) -> io::Error {
        // [Review Fix #D04] Reclassify only a verified unsafe entry; preserve permission, disk and unrelated native failures.
        match self.check_entry(name, is_directory) {
            Err(conflict) if conflict.kind() == io::ErrorKind::AlreadyExists => conflict,
            _ => error,
        }
    }
    /// Resolve a relative directory below pinned handles; optionally create missing components, refusing reparse points.
    pub(crate) fn directory(&self, path: &str, is_create: bool) -> io::Result<Self> {
        super::share_manifest::validate_relative(path)
            .map_err(|_| invalid("unsafe relative path"))?;
        let mut current = self.clone_handle();
        for component in path.split('/') {
            current = current.child(std::ffi::OsStr::new(component), is_create)?;
        }
        Ok(current)
    }
    fn clone_handle(&self) -> Self {
        Self {
            directory: self.directory.clone(),
            ancestors: self.ancestors.clone(),
        }
    }
    /// Return the pinned parent and validated leaf name; optionally create ancestors or report I/O failure.
    pub(crate) fn parent(&self, path: &str, is_create: bool) -> io::Result<(Self, String)> {
        super::share_manifest::validate_relative(path)
            .map_err(|_| invalid("unsafe relative path"))?;
        match path.rsplit_once('/') {
            Some((parent, name)) => Ok((self.directory(parent, is_create)?, name.into())),
            None => Ok((self.clone_handle(), path.into())),
        }
    }
    /// Return true for an existing ordinary leaf, false for absence; reject reparse/special entries.
    pub(crate) fn ordinary(&self, name: &str) -> io::Result<bool> {
        match self.read(name) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
    /// Open an ordinary leaf without following reparse points and return its native handle or I/O error.
    pub(crate) fn read(&self, name: &str) -> io::Result<File> {
        self.check_entry(std::ffi::OsStr::new(name), false)?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .share_mode(FILE_SHARE_READ);
        let file = self
            .directory
            .open_with(name, &options)
            .map_err(|error| self.classify_entry_error(std::ffi::OsStr::new(name), false, error))?
            .into_std();
        reject_reparse(&file, false)?;
        Ok(file)
    }
    /// Open a read-only private ledger beside its existing writer; refuse reparse points, hard links and native failures.
    pub(crate) fn read_shared(&self, name: &str) -> io::Result<File> {
        self.check_entry(std::ffi::OsStr::new(name), false)?;
        let mut options = OpenOptions::new();
        // [Review Fix #ShareLedgerWin1] Sharing WRITE permits the existing RW handle, while this new handle requests only READ; the writer still refuses other writers.
        options
            .read(true)
            .follow(FollowSymlinks::No)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        let file = self
            .directory
            .open_with(name, &options)
            .map_err(|error| self.classify_entry_error(std::ffi::OsStr::new(name), false, error))?
            .into_std();
        reject_reparse(&file, false)?;
        if cap_fs_ext::MetadataExt::nlink(&cap_std::fs::Metadata::from_file(&file)?) != 1 {
            return Err(super::share_local::path_conflict(
                "completion ledger must have exactly one link",
            ));
        }
        Ok(file)
    }
    /// Exclusively create a new ordinary temporary leaf; permit atomic publication while its handle is held.
    pub(crate) fn exclusive(&self, name: &str) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE);
        let file = self.directory.open_with(name, &options)?.into_std();
        reject_reparse(&file, false)?;
        Ok(file)
    }
    /// Publish by capability-relative rename or no-clobber hard link; propagate failures without following target links.
    pub(crate) fn replace(&self, source: &str, target: &str, is_overwrite: bool) -> io::Result<()> {
        self.ordinary(target)?;
        if is_overwrite {
            self.directory.rename(source, &self.directory, target)?;
        } else {
            self.directory.hard_link(source, &self.directory, target)?;
            self.directory.remove_file(source)?;
        }
        Ok(())
    }
    /// Remove a leaf relative to the pinned directory; propagate I/O failures.
    pub(crate) fn remove(&self, name: &str) -> io::Result<()> {
        self.directory.remove_file(name)
    }
}
fn reject_reparse(file: &File, is_directory: bool) -> io::Result<()> {
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (is_directory && !metadata.is_dir())
        || (!is_directory && !metadata.is_file())
    {
        return Err(super::share_local::path_conflict(
            "reparse point or unexpected destination file type",
        ));
    }
    Ok(())
}
fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}
