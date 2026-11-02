//! Immutable snapshot binding and synchronized fixed-record completion recovery.
use super::{invalid, Identity, Journal};
use crate::handler::share_local::{SafeDir, StagedFile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
};
use tos_core::agent::error::CliError;

const MAGIC: &[u8; 8] = b"ADRCMP01";
const HEADER_SIZE: u64 = 40;
const RECORD_SIZE: usize = 72;
const LEDGER_NAME: &str = "ledger.bin";
type Hash = [u8; 32];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedJournal {
    version: u32,
    identity: Identity,
    manifest: crate::handler::share_manifest::Manifest,
    completed: Vec<Option<String>>,
    #[serde(default)]
    completion_ledger: Option<String>,
}
#[derive(Serialize)]
struct Snapshot<'a> {
    #[serde(flatten)]
    journal: &'a Journal,
    completion_ledger: String,
}
struct HashWriter(Sha256);
impl Write for HashWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn fingerprint(value: &impl Serialize) -> Result<Hash, CliError> {
    let mut writer = HashWriter(Sha256::new());
    serde_json::to_writer(&mut writer, value)?;
    Ok(writer.0.finalize().into())
}
fn digest_bytes(value: &str) -> Result<Hash, CliError> {
    let mut hash = [0; 32];
    hex::decode_to_slice(value, &mut hash).map_err(|_| invalid("invalid completion digest"))?;
    Ok(hash)
}
fn completions(journal: &Journal) -> Result<Vec<Option<Hash>>, CliError> {
    journal
        .completed
        .iter()
        .map(|digest| digest.as_deref().map(digest_bytes).transpose())
        .collect()
}

pub(super) struct State {
    identity: Identity,
    immutable: Hash,
    entry_fingerprints: Vec<Hash>,
    durable_completions: Vec<Option<Hash>>,
    binding: Hash,
    is_snapshot_pending: bool,
    pub(super) writer: Option<LedgerWriter>,
}
impl State {
    pub(super) fn new(
        journal: &Journal,
        binding: Hash,
        is_snapshot_pending: bool,
    ) -> Result<Self, CliError> {
        Ok(Self {
            identity: journal.identity.clone(),
            immutable: fingerprint(&(journal.version, &journal.identity, &journal.manifest))?,
            entry_fingerprints: journal
                .manifest
                .files
                .iter()
                .map(fingerprint)
                .collect::<Result<_, _>>()?,
            durable_completions: completions(journal)?,
            binding,
            is_snapshot_pending,
            writer: None,
        })
    }
    pub(super) fn fresh(journal: &Journal) -> Result<Self, CliError> {
        journal.validate(&journal.identity)?;
        Self::new(journal, fingerprint(journal)?, true)
    }
    pub(super) fn save(
        &mut self,
        root: &SafeDir,
        name: &str,
        journal: &Journal,
        is_fresh: bool,
    ) -> Result<(), CliError> {
        if self.immutable != fingerprint(&(journal.version, &journal.identity, &journal.manifest))?
        {
            return Err(invalid(
                "checkpoint snapshot metadata changed after initialization",
            ));
        }
        self.check_completions(journal)?;
        self.prepare_writer(root, name, journal, is_fresh)?;
        if self.is_snapshot_pending {
            let mut staged = StagedFile::new(root, name)?;
            serde_json::to_writer(
                &mut staged.file,
                &Snapshot {
                    journal,
                    completion_ledger: hex::encode(self.binding),
                },
            )?;
            staged.publish(!is_fresh)?;
            self.is_snapshot_pending = false;
        }
        for index in 0..journal.completed.len() {
            if journal.completed[index].is_some() {
                self.save_completion(journal, index)?;
            }
        }
        Ok(())
    }
    fn prepare_writer(
        &mut self,
        root: &SafeDir,
        name: &str,
        journal: &Journal,
        is_fresh: bool,
    ) -> Result<(), CliError> {
        if let Some(writer) = &self.writer {
            writer.ensure_usable()?;
            // [Review Fix #ShareLedger2] Failed publication cannot reuse a prepared header with a different completion snapshot.
            if self.is_snapshot_pending && self.binding != fingerprint(journal)? {
                return Err(invalid(
                    "prepared checkpoint snapshot changed after publication failure",
                ));
            }
        } else {
            if self.is_snapshot_pending {
                self.binding = fingerprint(journal)?;
                self.durable_completions = completions(journal)?;
            }
            self.writer = Some(LedgerWriter::create(root, name, self, is_fresh)?);
        }
        Ok(())
    }
    fn check_completions(&self, journal: &Journal) -> Result<(), CliError> {
        if journal.completed.len() != self.durable_completions.len() {
            return Err(invalid("checkpoint completion count changed"));
        }
        for (saved, current) in self.durable_completions.iter().zip(&journal.completed) {
            let current = current.as_deref().map(digest_bytes).transpose()?;
            if saved.is_some() && *saved != current {
                return Err(invalid("a durable completion cannot be cleared or changed"));
            }
        }
        Ok(())
    }
    pub(super) fn save_completion(
        &mut self,
        journal: &Journal,
        index: usize,
    ) -> Result<(), CliError> {
        if self.is_snapshot_pending {
            return Err(invalid(
                "snapshot must be durably saved before completion records",
            ));
        }
        self.writer
            .as_ref()
            .ok_or_else(|| invalid("save snapshot before appending completions"))?
            .ensure_usable()?;
        if journal.version != 1
            || journal.identity != self.identity
            || journal.completed.len() != self.durable_completions.len()
            || journal.manifest.files.len() != self.entry_fingerprints.len()
        {
            return Err(invalid(
                "checkpoint completion task identity or size differs",
            ));
        }
        let entry = journal
            .manifest
            .files
            .get(index)
            .ok_or_else(|| invalid("completion index is outside snapshot"))?;
        if fingerprint(entry)? != self.entry_fingerprints[index] {
            return Err(invalid("completed file fingerprint differs from snapshot"));
        }
        let digest = journal.completed[index]
            .as_deref()
            .ok_or_else(|| invalid("completion digest is missing"))?;
        let digest = digest_bytes(digest)?;
        if let Some(saved) = self.durable_completions[index] {
            return if saved == digest {
                Ok(())
            } else {
                Err(invalid("a durable completion digest changed"))
            };
        }
        self.writer
            .as_mut()
            .ok_or_else(|| invalid("save snapshot before appending completions"))?
            .append(index, digest)?;
        self.durable_completions[index] = Some(digest);
        Ok(())
    }
}

pub(super) struct LedgerWriter {
    file: File,
    binding: Hash,
    is_poisoned: bool,
    directory: SafeDir,
}
impl LedgerWriter {
    fn create(root: &SafeDir, name: &str, state: &State, is_fresh: bool) -> Result<Self, CliError> {
        let directory = root.directory(&format!("{name}.completed"), true)?;
        if is_fresh && directory.ordinary(LEDGER_NAME)? {
            return Err(invalid(
                "completion ledger already exists; choose another checkpoint",
            ));
        }
        let mut staged = StagedFile::new(&directory, LEDGER_NAME)?;
        staged.file.write_all(MAGIC)?;
        staged.file.write_all(&state.binding)?;
        for (index, digest) in state.durable_completions.iter().enumerate() {
            if let Some(digest) = digest {
                staged
                    .file
                    .write_all(&record(state.binding, index, *digest))?;
            }
        }
        let file = staged.file.try_clone()?;
        staged.publish(!is_fresh)?;
        Ok(Self {
            file,
            binding: state.binding,
            is_poisoned: false,
            directory,
        })
    }
    fn append(&mut self, index: usize, digest: Hash) -> Result<(), CliError> {
        self.ensure_usable()?;
        // [Review Fix #ShareLedger1] A partial write or failed fsync must never be followed by another record in this process.
        self.is_poisoned = true;
        self.verify_open_file()?;
        self.file.seek(SeekFrom::End(0))?;
        self.file.write_all(&record(self.binding, index, digest))?;
        self.file.sync_all()?;
        self.is_poisoned = false;
        Ok(())
    }
    fn ensure_usable(&self) -> Result<(), CliError> {
        if self.is_poisoned {
            return Err(invalid(
                "completion ledger requires resume after a failed append",
            ));
        }
        Ok(())
    }
    fn verify_open_file(&self) -> Result<(), CliError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let held = self.file.metadata()?;
            let current = self.directory.read_shared(LEDGER_NAME)?.metadata()?;
            // [Review Fix #ShareLedger3] Refuse removed/replaced/hard-linked ledgers before mutating the held private inode.
            if held.nlink() != 1 || held.dev() != current.dev() || held.ino() != current.ino() {
                return Err(invalid(
                    "completion ledger was removed, replaced or hard-linked",
                ));
            }
        }
        #[cfg(windows)]
        {
            use cap_fs_ext::MetadataExt;
            let held = cap_std::fs::Metadata::from_file(&self.file)?;
            let current =
                cap_std::fs::Metadata::from_file(&self.directory.read_shared(LEDGER_NAME)?)?;
            if held.nlink() != 1 || held.dev() != current.dev() || held.ino() != current.ino() {
                return Err(invalid(
                    "completion ledger was removed, replaced or hard-linked",
                ));
            }
        }
        Ok(())
    }
    fn reader(&self) -> Result<File, CliError> {
        self.verify_open_file()?;
        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        Ok(file)
    }
}
fn record(binding: Hash, index: usize, digest: Hash) -> [u8; RECORD_SIZE] {
    let mut bytes = [0; RECORD_SIZE];
    bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
    bytes[8..40].copy_from_slice(&digest);
    let mut hash = Sha256::new();
    hash.update(binding);
    hash.update(&bytes[..40]);
    bytes[40..].copy_from_slice(&hash.finalize());
    bytes
}

pub(super) fn load(
    root: &SafeDir,
    name: &str,
    identity: &Identity,
    writer: Option<&LedgerWriter>,
) -> Result<(Journal, State), CliError> {
    let saved: SavedJournal = serde_json::from_reader(std::io::BufReader::new(root.read(name)?))
        .map_err(|_| invalid("invalid checkpoint JSON/schema"))?;
    let marker = saved.completion_ledger;
    let mut journal = Journal {
        version: saved.version,
        identity: saved.identity,
        manifest: saved.manifest,
        completed: saved.completed,
    };
    journal.validate(identity)?;
    let binding = fingerprint(&journal)?;
    if marker
        .as_deref()
        .is_some_and(|marker| marker != hex::encode(binding))
    {
        return Err(invalid("completion ledger snapshot binding differs"));
    }
    normalize_identity(&mut journal, identity);
    // [Review Fix #ShareLegacy3] Only unmarked legacy snapshots may recognize a ledger prepared for their verified API normalization before an interrupted upgrade.
    let migration_binding = marker
        .is_none()
        .then(|| fingerprint(&journal))
        .transpose()?;
    let file = match writer {
        Some(writer) => Some(writer.reader()?),
        None => read_ledger(root, name)?,
    };
    match file {
        Some(file) => merge(file, binding, migration_binding, &mut journal.completed)?,
        None if marker.is_some() => return Err(invalid("checkpoint completion ledger is missing")),
        None => {}
    }
    let state = State::new(&journal, binding, marker.is_none())?;
    Ok((journal, state))
}
fn normalize_identity(journal: &mut Journal, expected: &Identity) {
    if journal.identity.selection_policy.is_some() {
        journal.identity = expected.clone();
    } else if journal.identity.api_origin.is_none() {
        // Validation already proved the legacy anonymous API is the URL-derived API; preserve its historical mapping marker.
        journal.identity.api_origin = expected.api_origin.clone();
    }
}
fn read_ledger(root: &SafeDir, name: &str) -> Result<Option<File>, CliError> {
    let directory = match root.directory(&format!("{name}.completed"), false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match directory.read_shared(LEDGER_NAME) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn merge(
    mut file: File,
    binding: Hash,
    migration_binding: Option<Hash>,
    completed: &mut [Option<String>],
) -> Result<(), CliError> {
    let maximum = HEADER_SIZE
        .saturating_add((completed.len() as u64).saturating_mul(RECORD_SIZE as u64))
        .saturating_add(RECORD_SIZE as u64 - 1);
    if file.metadata()?.len() > maximum {
        return Err(invalid(
            "completion ledger exceeds its snapshot record bound",
        ));
    }
    let mut header = [0; HEADER_SIZE as usize];
    file.read_exact(&mut header)
        .map_err(|_| invalid("incomplete completion ledger header"))?;
    let recorded_binding: Hash = header[8..].try_into().unwrap();
    if &header[..8] != MAGIC
        || (recorded_binding != binding && migration_binding != Some(recorded_binding))
    {
        return Err(invalid("completion ledger belongs to a different snapshot"));
    }
    let mut seen = vec![false; completed.len()];
    loop {
        let mut bytes = [0; RECORD_SIZE];
        if !read_record(&mut file, &mut bytes)? {
            break;
        }
        merge_record(bytes, recorded_binding, completed, &mut seen)?;
    }
    Ok(())
}
fn read_record(file: &mut File, bytes: &mut [u8; RECORD_SIZE]) -> Result<bool, CliError> {
    let mut offset = 0;
    while offset < RECORD_SIZE {
        match file.read(&mut bytes[offset..]) {
            Ok(0) => return Ok(false), // An incomplete final record is never credited.
            Ok(count) => offset += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(true)
}
fn merge_record(
    bytes: [u8; RECORD_SIZE],
    binding: Hash,
    completed: &mut [Option<String>],
    seen: &mut [bool],
) -> Result<(), CliError> {
    let index = usize::try_from(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
        .map_err(|_| invalid("invalid completion index"))?;
    let digest: Hash = bytes[8..40].try_into().unwrap();
    if index >= completed.len() || seen[index] || record(binding, index, digest) != bytes {
        return Err(invalid("invalid completion ledger record"));
    }
    let digest = hex::encode(digest);
    if completed[index]
        .as_ref()
        .is_some_and(|saved| saved.to_ascii_lowercase() != digest)
    {
        return Err(invalid("completion ledger conflicts with base snapshot"));
    }
    completed[index] = Some(digest);
    seen[index] = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksummed_records_reject_duplicate_and_out_of_bounds_indices() {
        let binding = [1; 32];
        let mut completed = vec![None, None];
        let mut seen = vec![false; 2];
        let first = record(binding, 0, [2; 32]);
        merge_record(first, binding, &mut completed, &mut seen).unwrap();
        assert!(merge_record(first, binding, &mut completed, &mut seen).is_err());
        assert!(merge_record(
            record(binding, 2, [3; 32]),
            binding,
            &mut completed,
            &mut seen
        )
        .is_err());
        let mut damaged = record(binding, 1, [4; 32]);
        damaged[12] ^= 1;
        assert!(merge_record(damaged, binding, &mut completed, &mut seen).is_err());
        assert_eq!(completed[1], None);
    }
    #[cfg(unix)]
    #[test]
    fn append_failure_poisoning_survives_replacing_the_failed_io_handle() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-ledger-poison-{}", ulid::Ulid::new()));
        let directory = SafeDir::open(&root, true).unwrap();
        let mut writable = directory.exclusive(LEDGER_NAME).unwrap();
        writable.write_all(MAGIC).unwrap();
        writable.write_all(&[1; 32]).unwrap();
        writable.sync_all().unwrap();
        let file = directory.read(LEDGER_NAME).unwrap();
        let mut writer = LedgerWriter {
            file,
            directory,
            binding: [1; 32],
            is_poisoned: false,
        };
        assert!(writer.append(0, [2; 32]).is_err());
        writer.file = writable;
        assert!(writer.append(1, [3; 32]).is_err());
        assert_eq!(
            std::fs::metadata(root.join(LEDGER_NAME)).unwrap().len(),
            HEADER_SIZE
        );
        drop(writer);
        std::fs::remove_dir_all(root).unwrap();
    }
}
