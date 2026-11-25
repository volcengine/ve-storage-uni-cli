use super::*;
use crate::infra::atomic_file;
use std::{
    fs::{self, Metadata, OpenOptions},
    io::Read,
    path::Component,
};

fn verify_owner(metadata: &Metadata, is_directory: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
            || (!is_directory && metadata.nlink() != 1)
        {
            return Err(Error::Storage);
        }
    }
    #[cfg(not(unix))]
    let _ = (metadata, is_directory);
    Ok(())
}

fn verify_ancestors(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(Error::Storage);
    }
    for ancestor in path.ancestors() {
        let metadata = match fs::symlink_metadata(ancestor) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(Error::Storage),
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::Storage);
        }
        verify_parent_owner(&metadata)?;
    }
    Ok(())
}

fn verify_parent_owner(metadata: &Metadata) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let is_sticky = metadata.mode() & u32::from(libc::S_ISVTX) != 0;
        let has_safe_owner = metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() };
        if !has_safe_owner || (metadata.mode() & 0o022 != 0 && !is_sticky) {
            return Err(Error::Storage);
        }
    }
    #[cfg(not(unix))]
    let _ = metadata;
    Ok(())
}

pub(super) fn directory(path: &Path) -> Result<()> {
    verify_ancestors(path)?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| Error::Storage)?;
    verify_ancestors(path)?;
    verify_owner(&fs::metadata(path).map_err(|_| Error::Storage)?, true)
}

pub(super) fn exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(Error::Storage);
            }
            verify_owner(&metadata, false)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(Error::Storage),
    }
}

pub(super) fn open_file(path: &Path, is_create: bool) -> Result<File> {
    exists(path)?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(is_create)
        .create(is_create)
        .truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
    }
    let file = options.open(path).map_err(|_| Error::Storage)?;
    let metadata = file.metadata().map_err(|_| Error::Storage)?;
    if !metadata.is_file() {
        return Err(Error::Storage);
    }
    verify_owner(&metadata, false)?;
    Ok(file)
}

pub(super) fn read(path: &Path, limit: u64) -> Result<Zeroizing<Vec<u8>>> {
    let mut file = open_file(path, false)?;
    if file.metadata().map_err(|_| Error::Storage)?.len() > limit {
        return Err(Error::Storage);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    (&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Storage);
    }
    Ok(bytes)
}

pub(super) fn read_external(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    verify_ancestors(path.parent().ok_or(Error::Storage)?)?;
    read(path, 4096)
}

impl Store {
    pub(super) fn coordination_directory(&self) -> Result<std::path::PathBuf> {
        // [Review Fix #5] Same credentials/profile must share coordination even when legacy-home overrides differ.
        Ok(self
            .config
            .credentials_path
            .parent()
            .ok_or(Error::Storage)?
            .canonicalize()
            .map_err(|_| Error::Storage)?
            .join("ask"))
    }
    pub(crate) fn state_path(&self, name: &str) -> Result<std::path::PathBuf> {
        if !matches!(name, ".lock" | "pending") {
            return Ok(self.config.home.join(name));
        }
        use sha2::{Digest, Sha256};
        let parent = self
            .config
            .credentials_path
            .parent()
            .ok_or(Error::Storage)?
            .canonicalize()
            .map_err(|_| Error::Storage)?;
        let path = parent.join(
            self.config
                .credentials_path
                .file_name()
                .ok_or(Error::Storage)?,
        );
        // Existing files canonicalize their basename too, so Windows casing aliases share refresh state.
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => path,
            Err(_) => return Err(Error::Storage),
        };
        let mut hash = Sha256::new();
        hash.update(path.as_os_str().as_encoded_bytes());
        hash.update([0]);
        hash.update(self.config.profile.as_bytes());
        Ok(self
            .coordination_directory()?
            .join(format!("{name}-{}", hex::encode(hash.finalize()))))
    }
    pub(super) fn open_file(&self, name: &str, is_create: bool) -> Result<File> {
        open_file(&self.state_path(name)?, is_create)
    }
    pub(super) fn replace(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let path = self.state_path(name)?;
        exists(&path)?;
        // [Review Fix #2] Use the existing cross-platform credential writer, including Windows replacement.
        atomic_file::write_owner_only_atomic(&path, bytes).map_err(|_| Error::Storage)
    }
    pub(super) fn has_pending(&self) -> Result<bool> {
        exists(&self.state_path("pending")?)
    }
    pub(super) fn remove(&self, name: &str) -> Result<()> {
        let path = self.state_path(name)?;
        if exists(&path)? {
            fs::remove_file(&path).map_err(|_| Error::Storage)?;
            atomic_file::sync_parent_directory(&path).map_err(|_| Error::Storage)?;
        }
        Ok(())
    }
}
