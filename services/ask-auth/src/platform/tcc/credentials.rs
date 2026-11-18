//! Fresh platform DPS credentials with fixed wiped buffers and no background cache.
use super::TccError;
use reqwest::header::HeaderValue;
use std::{path::PathBuf, time::Duration};
#[cfg(unix)]
use tokio::io::AsyncReadExt;
use zeroize::Zeroizing;

pub(super) const MAX_TOKEN_BYTES: usize = 16 * 1024;
const CREDENTIAL_TIMEOUT: Duration = Duration::from_millis(250);
pub(super) enum CredentialSource {
    Static(Zeroizing<String>),
    File(PathBuf),
}
impl CredentialSource {
    pub(super) fn from_env() -> Result<Self, TccError> {
        let token = match std::env::var("SEC_TOKEN_STRING") {
            Ok(value) => Some(value),
            Err(std::env::VarError::NotPresent) => None,
            Err(_) => return Err(TccError::Identity),
        };
        Self::from_values(token, std::env::var_os("SEC_TOKEN_PATH").map(PathBuf::from))
    }
    pub(super) fn from_values(
        token: Option<String>,
        path: Option<PathBuf>,
    ) -> Result<Self, TccError> {
        if let Some(value) = token {
            let value = Zeroizing::new(value);
            validate_token(Some(&value))?;
            return Ok(Self::Static(value));
        }
        let path = path
            .filter(|path| path.is_absolute())
            .ok_or(TccError::Identity)?;
        Ok(Self::File(path))
    }
    pub(super) async fn load(&self) -> Result<Credential, TccError> {
        // [Review Fix #96] Re-read the current platform file within the snapshot deadline; never reuse stale success.
        tokio::time::timeout(CREDENTIAL_TIMEOUT, self.load_current())
            .await
            .map_err(|_| TccError::Identity)?
    }
    async fn load_current(&self) -> Result<Credential, TccError> {
        match self {
            Self::Static(value) => Credential::from_bytes(value.as_bytes(), None),
            Self::File(path) => load_file(path).await,
        }
    }
    pub(super) async fn check_current(&self, initial: &Credential) -> Result<(), TccError> {
        let current = self.load().await?;
        if current.bytes() != initial.bytes() || current.stamp != initial.stamp {
            return Err(TccError::Identity);
        }
        Ok(())
    }
}
pub(super) struct Credential {
    storage: Zeroizing<Box<[u8]>>,
    length: usize,
    stamp: Option<FileStamp>,
}
impl Credential {
    fn from_bytes(value: &[u8], stamp: Option<FileStamp>) -> Result<Self, TccError> {
        let text = std::str::from_utf8(value).map_err(|_| TccError::Identity)?;
        validate_token(Some(text))?;
        let mut credential = Self::empty();
        credential.storage[..value.len()].copy_from_slice(value);
        credential.length = value.len();
        credential.stamp = stamp;
        Ok(credential)
    }
    fn empty() -> Self {
        Self {
            storage: Zeroizing::new(vec![0; MAX_TOKEN_BYTES + 1].into_boxed_slice()),
            length: 0,
            stamp: None,
        }
    }
    pub(super) fn bytes(&self) -> &[u8] {
        &self.storage[..self.length]
    }
    pub(super) fn header(&self) -> Result<HeaderValue, TccError> {
        let mut value = HeaderValue::from_bytes(self.bytes()).map_err(|_| TccError::Identity)?;
        // [Review Fix #97] Reqwest's required header copy is sensitive even though it cannot be wiped by this adapter.
        value.set_sensitive(true);
        Ok(value)
    }
}
#[cfg(not(unix))]
#[derive(PartialEq, Eq)]
struct FileStamp;
#[cfg(unix)]
#[derive(PartialEq, Eq)]
struct FileStamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
#[cfg(unix)]
impl FileStamp {
    fn new(metadata: &std::fs::Metadata) -> Result<Self, TccError> {
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_file() || metadata.len() > MAX_TOKEN_BYTES as u64 {
            return Err(TccError::Identity);
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}
#[cfg(unix)]
async fn load_file(path: &std::path::Path) -> Result<Credential, TccError> {
    // [Review Fix #98] O_NONBLOCK prevents a substituted FIFO from hanging open; fstat rejects every nonregular target.
    // Trusted platform symlinks remain supported for token rotation; no undocumented mode/owner restriction is imposed.
    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .await
        .map_err(|_| TccError::Identity)?;
    let before = FileStamp::new(&file.metadata().await.map_err(|_| TccError::Identity)?)?;
    let mut credential = Credential::empty();
    while credential.length < credential.storage.len() {
        let count = file
            .read(&mut credential.storage[credential.length..])
            .await
            .map_err(|_| TccError::Identity)?;
        if count == 0 {
            break;
        }
        credential.length += count;
    }
    let after = FileStamp::new(&file.metadata().await.map_err(|_| TccError::Identity)?)?;
    if before != after || before.size != credential.length as u64 {
        return Err(TccError::Identity);
    }
    let text = std::str::from_utf8(credential.bytes()).map_err(|_| TccError::Identity)?;
    validate_token(Some(text))?;
    credential.stamp = Some(after);
    Ok(credential)
}
pub(super) fn validate_token(token: Option<&str>) -> Result<(), TccError> {
    if token.is_some_and(|value| {
        !value.is_empty()
            && value.len() <= MAX_TOKEN_BYTES
            && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
    }) {
        Ok(())
    } else {
        Err(TccError::Identity)
    }
}
#[cfg(not(unix))]
async fn load_file(_path: &std::path::Path) -> Result<Credential, TccError> {
    // [Review Fix #102] Unsupported platforms must compile while refusing unverified file identity checks.
    Err(TccError::Identity)
}
#[cfg(all(test, unix))]
mod tests;
