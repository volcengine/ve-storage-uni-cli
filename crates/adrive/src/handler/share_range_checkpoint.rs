//! Secret-free, descriptor-relative recovery for independently verified ranges.
use super::{
    download_ranges::DownloadRange,
    share_content::failed,
    share_local::{resolve_directory, SafeDir, StagedFile},
    share_manifest::RemoteFile,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
use tos_core::agent::error::CliError;

const MAX_RECEIPT_BYTES: u64 = 1024;

/// Pin the checkpoint directory leaf without following links; only user-selected parent aliases are resolved.
pub(super) fn open_directory(path: &Path) -> Result<SafeDir, CliError> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| failed("invalid range checkpoint directory"))?;
    super::share_manifest::validate_relative(name)?;
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let resolved = resolve_directory(parent, true)?;
    Ok(SafeDir::open_resolved(&resolved)?.directory(name, true)?)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u32,
    number: u32,
    offset: u64,
    size: u64,
    sha256: String,
}

pub(super) struct RangeCheckpoint {
    directory: SafeDir,
    _lock: File,
    size: u64,
}
impl RangeCheckpoint {
    /// Lock one fingerprint-isolated file checkpoint; reject unpinned identities, unsafe entries and competing downloads.
    pub(super) fn open(root: &SafeDir, remote: &RemoteFile, local: &str) -> Result<Self, CliError> {
        if !remote.has_identity() {
            return Err(failed("range recovery requires a pinned remote identity"));
        }
        super::share_manifest::validate_relative(local)?;
        let identity = serde_json::to_vec(&(1, remote, local))?;
        let fingerprint = hex::encode(Sha256::digest(identity));
        let directory = root.directory(&fingerprint, true)?;
        let lock = match directory.exclusive("file.lock") {
            Ok(lock) => lock,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                directory.read("file.lock")?
            }
            Err(error) => return Err(error.into()),
        };
        lock.try_lock_exclusive().map_err(|_| {
            CliError::Conflict("range checkpoint is locked by another download".into())
        })?;
        Ok(Self {
            directory,
            _lock: lock,
            size: remote.size,
        })
    }
    /// Remove only this fingerprint's known range files after durable publication or failed whole-file integrity verification.
    pub(super) fn clear(&self) -> Result<(), CliError> {
        for range in super::download_ranges::RangePlan::new(self.size)? {
            self.discard(range)?;
        }
        Ok(())
    }
    /// Start a new exclusive unpublished range; dropping it removes only these unverified bytes.
    pub(super) fn stage(&self, range: DownloadRange) -> Result<StagedFile, CliError> {
        Ok(StagedFile::new(&self.directory, &part_name(range))?)
    }
    /// Persist complete bytes before their bounded hash receipt; failures leave safely recoverable or discardable files.
    pub(super) fn complete(
        &self,
        range: DownloadRange,
        staged: StagedFile,
        digest: &str,
    ) -> Result<(), CliError> {
        staged.publish(false)?;
        let receipt = Receipt {
            version: 1,
            number: range.number,
            offset: range.offset,
            size: range.size,
            sha256: digest.into(),
        };
        let mut staged = StagedFile::new(&self.directory, &receipt_name(range))?;
        staged.file.write_all(&serde_json::to_vec(&receipt)?)?;
        staged.publish(true)?;
        Ok(())
    }
    /// Rehash a recorded range before reuse; malformed, missing or changed ordinary files are discarded, unsafe paths fail.
    pub(super) async fn restore(
        &self,
        range: DownloadRange,
    ) -> Result<Option<(File, String)>, CliError> {
        let Some((mut file, digest)) = self.open_completed(range)? else {
            return Ok(None);
        };
        if hash_file(&mut file, range.size).await? != digest {
            // [Review Fix #SharePart3] Windows read handles exclude DELETE sharing; close before removing an invalid stored range.
            drop(file);
            self.discard(range)?;
            return Ok(None);
        }
        file.seek(SeekFrom::Start(0))?;
        Ok(Some((file, digest)))
    }
    /// Open a receipt-paired range for assembly; the caller must verify the returned digest while copying its bytes.
    pub(super) fn open_completed(
        &self,
        range: DownloadRange,
    ) -> Result<Option<(File, String)>, CliError> {
        let receipt = self.receipt(range)?;
        let Some(receipt) = receipt else {
            self.discard(range)?;
            return Ok(None);
        };
        let file = self.directory.read(&part_name(range))?;
        if file.metadata()?.len() != range.size {
            drop(file);
            self.discard(range)?;
            return Ok(None);
        }
        Ok(Some((file, receipt.sha256)))
    }
    fn receipt(&self, range: DownloadRange) -> Result<Option<Receipt>, CliError> {
        let has_part = self.directory.ordinary(&part_name(range))?;
        let has_receipt = self.directory.ordinary(&receipt_name(range))?;
        if !has_part || !has_receipt {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        self.directory
            .read(&receipt_name(range))?
            .take(MAX_RECEIPT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_RECEIPT_BYTES as usize {
            return Ok(None);
        }
        let receipt = match serde_json::from_slice::<Receipt>(&bytes) {
            Ok(receipt) => receipt,
            Err(_) => return Ok(None),
        };
        let is_valid = receipt.version == 1
            && receipt.number == range.number
            && receipt.offset == range.offset
            && receipt.size == range.size
            && receipt.sha256.len() == 64
            && receipt.sha256.bytes().all(|byte| byte.is_ascii_hexdigit());
        Ok(is_valid.then_some(receipt))
    }
    fn discard(&self, range: DownloadRange) -> Result<(), CliError> {
        for name in [part_name(range), receipt_name(range)] {
            // [Review Fix #SharePart1] Validate type before unlinking; never replace an unexpected persisted symlink or special file.
            if self.directory.ordinary(&name)? {
                self.directory.remove(&name)?;
            }
        }
        Ok(())
    }
}
fn part_name(range: DownloadRange) -> String {
    format!("part-{}.bin", range.number)
}
fn receipt_name(range: DownloadRange) -> String {
    format!("part-{}.json", range.number)
}

async fn hash_file(file: &mut File, expected_size: u64) -> Result<String, CliError> {
    let mut hash = Sha256::new();
    let mut length = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        length = length.saturating_add(count as u64);
        if length > expected_size {
            return Ok(String::new());
        }
        hash.update(&buffer[..count]);
        tokio::task::yield_now().await;
    }
    Ok(if length == expected_size {
        hex::encode(hash.finalize())
    } else {
        String::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::{download_ranges::DownloadRange, share_manifest::RemoteFile};
    use std::io::Write;

    fn fixture() -> (std::path::PathBuf, SafeDir) {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-parts-{}", ulid::Ulid::new()));
        let directory = SafeDir::open(&root, true).unwrap();
        (root, directory)
    }
    fn remote() -> RemoteFile {
        RemoteFile {
            path: "file".into(),
            size: 3,
            etag: "version".into(),
            crc64: 0,
            updated_at: 1,
        }
    }
    fn range() -> DownloadRange {
        DownloadRange {
            number: 1,
            offset: 0,
            size: 3,
        }
    }
    #[tokio::test]
    async fn verified_parts_survive_drop_and_are_invalidated_by_tampering() {
        let (root, directory) = fixture();
        let checkpoint = RangeCheckpoint::open(&directory, &remote(), "target").unwrap();
        let mut staged = checkpoint.stage(range()).unwrap();
        staged.file.write_all(b"abc").unwrap();
        checkpoint
            .complete(range(), staged, &hex::encode(sha2::Sha256::digest(b"abc")))
            .unwrap();
        assert!(checkpoint.restore(range()).await.unwrap().is_some());
        drop(checkpoint);
        let checkpoint = RangeCheckpoint::open(&directory, &remote(), "target").unwrap();
        assert!(checkpoint.restore(range()).await.unwrap().is_some());
        let folder = std::fs::read_dir(&root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(folder.join("part-1.bin"), b"xyz").unwrap();
        assert!(checkpoint.restore(range()).await.unwrap().is_none());
        assert!(!folder.join("part-1.bin").exists());
        drop(checkpoint);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn fingerprints_isolate_remote_changes_and_local_mappings_and_lock_duplicates() {
        let (root, directory) = fixture();
        let checkpoint = RangeCheckpoint::open(&directory, &remote(), "target").unwrap();
        assert!(RangeCheckpoint::open(&directory, &remote(), "target").is_err());
        let mut changed = remote();
        changed.etag = "new-version".into();
        assert!(RangeCheckpoint::open(&directory, &changed, "target")
            .unwrap()
            .restore(range())
            .await
            .unwrap()
            .is_none());
        assert!(RangeCheckpoint::open(&directory, &remote(), "other")
            .unwrap()
            .restore(range())
            .await
            .unwrap()
            .is_none());
        drop(checkpoint);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn persisted_part_symlinks_are_refused_without_touching_the_target() {
        let (root, directory) = fixture();
        let checkpoint = RangeCheckpoint::open(&directory, &remote(), "target").unwrap();
        let folder = std::fs::read_dir(&root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(root.join("victim"), b"safe").unwrap();
        std::os::unix::fs::symlink(root.join("victim"), folder.join("part-1.bin")).unwrap();
        assert!(checkpoint.restore(range()).await.is_err());
        assert_eq!(std::fs::read(root.join("victim")).unwrap(), b"safe");
        drop(checkpoint);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn incomplete_or_malformed_receipts_never_credit_a_persisted_range() {
        for mutation in [
            "missing",
            "unknown-field",
            "oversized",
            "wrong-range",
            "short-part",
        ] {
            let (root, directory) = fixture();
            let checkpoint = RangeCheckpoint::open(&directory, &remote(), "target").unwrap();
            let mut staged = checkpoint.stage(range()).unwrap();
            staged.file.write_all(b"abc").unwrap();
            checkpoint
                .complete(range(), staged, &hex::encode(Sha256::digest(b"abc")))
                .unwrap();
            let folder = std::fs::read_dir(&root)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let receipt_path = folder.join("part-1.json");
            match mutation {
                "missing" => std::fs::remove_file(&receipt_path).unwrap(),
                "oversized" => std::fs::write(&receipt_path, "x".repeat(2048)).unwrap(),
                "short-part" => std::fs::write(folder.join("part-1.bin"), "a").unwrap(),
                _ => {
                    let mut receipt: serde_json::Value =
                        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
                    receipt[if mutation == "wrong-range" {
                        "offset"
                    } else {
                        "unknown"
                    }] = serde_json::json!(999);
                    std::fs::write(&receipt_path, serde_json::to_vec(&receipt).unwrap()).unwrap();
                }
            }
            assert!(
                checkpoint.restore(range()).await.unwrap().is_none(),
                "accepted {mutation}"
            );
            assert!(!folder.join("part-1.bin").exists());
            drop(checkpoint);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[cfg(unix)]
    #[test]
    fn checkpoint_directory_links_and_unpinned_versions_are_rejected() {
        let (root, directory) = fixture();
        std::fs::create_dir(root.join("victim")).unwrap();
        std::os::unix::fs::symlink(root.join("victim"), root.join("parts")).unwrap();
        assert!(open_directory(&root.join("parts")).is_err());
        let mut remote = remote();
        remote.etag.clear();
        assert!(RangeCheckpoint::open(&directory, &remote, "target").is_err());
        remote.etag = "W/weak".into();
        assert!(RangeCheckpoint::open(&directory, &remote, "target").is_err());
        assert_eq!(std::fs::read_dir(root.join("victim")).unwrap().count(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn completed_range_cleanup_removes_only_its_recorded_parts() {
        let (root, directory) = fixture();
        let checkpoint = RangeCheckpoint::open(&directory, &remote(), "target").unwrap();
        let mut staged = checkpoint.stage(range()).unwrap();
        staged.file.write_all(b"abc").unwrap();
        checkpoint
            .complete(range(), staged, &hex::encode(Sha256::digest(b"abc")))
            .unwrap();
        checkpoint.clear().unwrap();
        assert!(checkpoint.restore(range()).await.unwrap().is_none());
        drop(checkpoint);
        std::fs::remove_dir_all(root).unwrap();
    }
}
