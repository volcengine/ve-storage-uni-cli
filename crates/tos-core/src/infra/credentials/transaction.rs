use super::*;
use std::fs::{File, OpenOptions};
use std::time::{Duration, Instant};

use fs2::FileExt;

const TRANSACTION_LOCK_TIMEOUT: Duration = Duration::from_secs(3);
const TRANSACTION_LOCK_RETRY_DELAY: Duration = Duration::from_millis(25);

/// Coordination guard whose descriptor releases the OS lock on drop, including unwinding.
pub(crate) struct Lock {
    _file: File,
}

impl Lock {
    pub(super) fn acquire(path: &Path) -> Result<Self, CliError> {
        Self::acquire_file(&transaction_lock_path(path)?)
    }

    /// Acquire the directory's distinct first-key-initialization lock, creating the directory if absent.
    /// Returns an I/O, unsafe-lock validation or bounded lock-contention error.
    pub(crate) fn acquire_key_init(directory: &Path) -> Result<Self, CliError> {
        let directory = if directory.as_os_str().is_empty() {
            Path::new(".")
        } else {
            directory
        };
        std::fs::create_dir_all(directory)?;
        // [Review Fix #CredentialTransaction12] Key initialization uses a distinct lock so a credentials transaction can safely initialize its shared master key.
        Self::acquire_file(&directory.canonicalize()?.join(".key.lock"))
    }

    fn acquire_file(lock_path: &Path) -> Result<Self, CliError> {
        let file = open_lock_file(lock_path)?;
        let deadline = Instant::now() + TRANSACTION_LOCK_TIMEOUT;
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => return Ok(Self { _file: file }),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(CliError::Conflict(
                            "timed out waiting 3 seconds for the credential coordination lock"
                                .into(),
                        ));
                    }
                    std::thread::sleep(remaining.min(TRANSACTION_LOCK_RETRY_DELAY));
                }
                Err(error) => return Err(CliError::Io(error)),
            }
        }
    }
}

fn transaction_lock_path(path: &Path) -> Result<PathBuf, CliError> {
    path.file_name()
        .ok_or_else(|| CliError::ValidationError("credentials path must name a file".into()))?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    // [Review Fix #CredentialTransaction10] All credential files in this directory share .key, so its first initialization needs one directory-wide lock.
    Ok(parent.join(".credentials.lock"))
}

fn open_lock_file(path: &Path) -> Result<File, CliError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // [Review Fix #CredentialTransaction1] Never follow a lock-file symlink or block on a substituted FIFO.
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(CliError::ValidationError(
            "credentials lock must be a regular file".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        // [Review Fix #CredentialTransaction2] Reject shared inodes before tightening permissions on an existing coordination file.
        if metadata.nlink() != 1 || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(CliError::ValidationError(
                "credentials lock must be owned by the current user without hard links".into(),
            ));
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

impl CredentialsFile {
    // [Review Fix #CredentialTransaction11] A directory-sync error can occur after atomic replacement has already committed.
    /// Update the latest credentials and save the result as one transaction.
    /// `path` selects the store; `change` modifies its current contents without network calls.
    /// Returns the closure result or a load, mutation, encryption or persistence error.
    /// Load and mutation failures preserve the prior file; persistence errors after atomic
    /// replacement may indicate that the update committed but its durability was not confirmed.
    pub fn update<T>(
        path: &Path,
        change: impl FnOnce(&mut Self) -> Result<T, CliError>,
    ) -> Result<T, CliError> {
        // [Review Fix #CredentialTransaction3] Lock the complete read-modify-write sequence so independent namespaces and the master key cannot race.
        let _lock = Lock::acquire(path)?;
        let mut credentials = Self::load_from(path)?;
        let result = change(&mut credentials)?;
        credentials.save_unlocked(path)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    #[test]
    fn simultaneous_updates_preserve_all_profiles_and_one_master_key() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("credential-transactions-{}", uuid::Uuid::now_v7()));
        let path = root.join("credentials.toml");
        let barrier = Arc::new(Barrier::new(8));
        let writers: Vec<_> = (0..8)
            .map(|index| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    CredentialsFile::update(&path, |credentials| {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        credentials.set_aksk_field(
                            &format!("writer-{index}"),
                            CredentialSection::ADrive,
                            "access_key_id",
                            &format!("synthetic-{index}"),
                        )
                    })
                    .unwrap();
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let credentials = CredentialsFile::load_from(&path).unwrap();
        assert_eq!(credentials.profile_names().len(), 8);
        for index in 0..8 {
            let saved = credentials
                .effective_aksk(&format!("writer-{index}"), CredentialSection::ADrive, &path)
                .unwrap();
            assert_eq!(
                saved.access_key_id.as_deref(),
                Some(format!("synthetic-{index}").as_str())
            );
        }
        assert_eq!(std::fs::read(root.join(".key")).unwrap().len(), 32);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn different_filenames_share_one_lock_and_stable_master_key() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("credential-shared-key-{}", uuid::Uuid::now_v7()));
        let first_path = root.join("first.toml");
        let second_path = root.join("second.toml");
        let first_lock = Lock::acquire(&first_path).unwrap();
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (entered_sender, entered_receiver) = std::sync::mpsc::channel();
        let second_writer_path = second_path.clone();
        let writer = std::thread::spawn(move || {
            started_sender.send(()).unwrap();
            CredentialsFile::update(&second_writer_path, |credentials| {
                entered_sender.send(()).unwrap();
                set_test_access_key(credentials, "synthetic-second")
            })
            .unwrap();
        });
        started_receiver.recv().unwrap();
        let has_entered_before_release = entered_receiver
            .recv_timeout(Duration::from_millis(200))
            .is_ok();
        let mut first = CredentialsFile::default();
        set_test_access_key(&mut first, "synthetic-first").unwrap();
        first.save_unlocked(&first_path).unwrap();
        let original_key = std::fs::read(root.join(".key")).unwrap();
        drop(first_lock);
        writer.join().unwrap();
        assert!(
            !has_entered_before_release,
            "files sharing .key must serialize their initial writes"
        );
        assert_eq!(std::fs::read(root.join(".key")).unwrap(), original_key);
        for (path, expected) in [
            (&first_path, "synthetic-first"),
            (&second_path, "synthetic-second"),
        ] {
            assert_test_access_key(path, expected);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    fn set_test_access_key(credentials: &mut CredentialsFile, value: &str) -> Result<(), CliError> {
        credentials.set_aksk_field("default", CredentialSection::ADrive, "access_key_id", value)
    }

    fn assert_test_access_key(path: &Path, expected: &str) {
        let saved = CredentialsFile::load_from(path)
            .unwrap()
            .effective_aksk("default", CredentialSection::ADrive, path)
            .unwrap();
        assert_eq!(saved.access_key_id.as_deref(), Some(expected));
    }

    #[cfg(unix)]
    #[test]
    fn transaction_rejects_symlink_lock_without_touching_its_target() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("credential-lock-symlink-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let sentinel = root.join("sentinel");
        std::fs::write(&sentinel, "untouched").unwrap();
        std::os::unix::fs::symlink(&sentinel, root.join(".credentials.lock")).unwrap();
        let outcome = CredentialsFile::update(&root.join("credentials.toml"), |_| Ok(()));
        assert!(outcome.is_err());
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "untouched");
        assert!(!root.join("credentials.toml").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn contention_times_out_without_changing_credentials() {
        use fs2::FileExt;
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("credential-lock-timeout-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let blocker = std::fs::File::create(root.join(".credentials.lock")).unwrap();
        FileExt::lock_exclusive(&blocker).unwrap();
        let started = std::time::Instant::now();
        let outcome = CredentialsFile::update(&root.join("credentials.toml"), |_| Ok(()));
        assert!(matches!(outcome, Err(CliError::Conflict(_))));
        assert!(started.elapsed() >= std::time::Duration::from_secs(3));
        assert!(started.elapsed() < std::time::Duration::from_secs(6));
        assert!(!root.join("credentials.toml").exists());
        FileExt::unlock(&blocker).unwrap();
        CredentialsFile::update(&root.join("credentials.toml"), |_| Ok(())).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_mutation_preserves_the_committed_file_and_key() {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("credential-rollback-{}", uuid::Uuid::now_v7()));
        let path = root.join("credentials.toml");
        CredentialsFile::update(&path, |credentials| {
            credentials.set_aksk_field(
                "default",
                CredentialSection::Tos,
                "access_key_id",
                "synthetic-original",
            )
        })
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let key = std::fs::read(root.join(".key")).unwrap();
        let outcome: Result<(), CliError> = CredentialsFile::update(&path, |credentials| {
            credentials.set_aksk_field(
                "default",
                CredentialSection::Tos,
                "access_key_id",
                "synthetic-replacement",
            )?;
            Err(CliError::ValidationError(
                "synthetic mutation failed".into(),
            ))
        });
        assert!(outcome.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read(root.join(".key")).unwrap(), key);
        std::fs::remove_dir_all(root).unwrap();
    }
}
