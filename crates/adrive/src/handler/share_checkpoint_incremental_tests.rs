use super::*;
use crate::handler::share_manifest::{Entry, RemoteFile};

struct Fixture {
    root: PathBuf,
    path: PathBuf,
    journal: Journal,
}
impl Fixture {
    fn new(count: usize) -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-ledger-{}", ulid::Ulid::new()));
        std::fs::create_dir(&root).unwrap();
        let mut journal = Journal {
            version: 1,
            identity: Identity {
                origin: "https://example.test".into(),
                share_id: "s".into(),
                selection: None,
                destination: root.to_string_lossy().into(),
                is_recursive: false,
                is_overwrite: false,
                selection_policy: None,
                api_origin: None,
                recipient: None,
            },
            manifest: Manifest::default(),
            completed: vec![None; count],
        };
        for index in 0..count {
            let path = format!("file-{index}");
            journal.manifest.roots.push(path.clone());
            journal.manifest.files.push(Entry {
                local: path.clone(),
                remote: RemoteFile {
                    path,
                    size: 3,
                    etag: "tag".into(),
                    crc64: 0,
                    updated_at: 1,
                },
            });
        }
        Self {
            path: root.join("checkpoint.json"),
            root,
            journal,
        }
    }
    fn ledger(&self) -> PathBuf {
        self.root.join("checkpoint.json.completed/ledger.bin")
    }
    fn complete(&mut self, index: usize) {
        self.journal.completed[index] = Some(format!("{index:064x}"));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn completions_append_out_of_order_without_rewriting_the_base_snapshot() {
    let mut fixture = Fixture::new(3);
    let checkpoint = Checkpoint::open(&fixture.path, false).unwrap();
    checkpoint.save(&fixture.journal).unwrap();
    let base = std::fs::read(&fixture.path).unwrap();
    for index in [2, 0, 1] {
        fixture.complete(index);
        checkpoint.save_completion(&fixture.journal, index).unwrap();
        checkpoint.save_completion(&fixture.journal, index).unwrap();
        assert_eq!(std::fs::read(&fixture.path).unwrap(), base);
    }
    assert_eq!(
        std::fs::metadata(fixture.ledger()).unwrap().len(),
        40 + 72 * 3
    );
    assert_eq!(
        Checkpoint::read_only(&fixture.path, &fixture.journal.identity)
            .unwrap()
            .completed,
        fixture.journal.completed
    );
    drop(checkpoint);
    assert_eq!(
        Checkpoint::read_only(&fixture.path, &fixture.journal.identity)
            .unwrap()
            .completed,
        fixture.journal.completed
    );
}

#[test]
fn incomplete_tail_is_ignored_and_repaired_before_a_resumed_append() {
    let mut fixture = Fixture::new(3);
    let checkpoint = Checkpoint::open(&fixture.path, false).unwrap();
    checkpoint.save(&fixture.journal).unwrap();
    fixture.complete(1);
    checkpoint.save_completion(&fixture.journal, 1).unwrap();
    drop(checkpoint);
    std::fs::OpenOptions::new()
        .append(true)
        .open(fixture.ledger())
        .unwrap()
        .write_all(b"torn")
        .unwrap();
    let checkpoint = Checkpoint::open(&fixture.path, true).unwrap();
    let mut resumed = checkpoint.load(&fixture.journal.identity).unwrap();
    assert_eq!(resumed.completed, fixture.journal.completed);
    checkpoint.save(&resumed).unwrap();
    assert_eq!(std::fs::metadata(fixture.ledger()).unwrap().len(), 40 + 72);
    resumed.completed[0] = Some(format!("{:064x}", 0));
    checkpoint.save_completion(&resumed, 0).unwrap();
    drop(checkpoint);
    assert_eq!(
        Checkpoint::read_only(&fixture.path, &resumed.identity)
            .unwrap()
            .completed,
        resumed.completed
    );
}

#[test]
fn missing_new_ledger_and_complete_corrupt_records_are_errors() {
    let mut fixture = Fixture::new(1);
    let checkpoint = Checkpoint::open(&fixture.path, false).unwrap();
    checkpoint.save(&fixture.journal).unwrap();
    fixture.complete(0);
    checkpoint.save_completion(&fixture.journal, 0).unwrap();
    drop(checkpoint);
    let mut bytes = std::fs::read(fixture.ledger()).unwrap();
    bytes[48] ^= 1;
    std::fs::write(fixture.ledger(), &bytes).unwrap();
    assert!(Checkpoint::read_only(&fixture.path, &fixture.journal.identity).is_err());
    std::fs::remove_file(fixture.ledger()).unwrap();
    assert!(Checkpoint::read_only(&fixture.path, &fixture.journal.identity).is_err());
}

#[test]
fn ledgers_cannot_be_replayed_into_a_different_snapshot() {
    let first = Fixture::new(1);
    let second = Fixture::new(1);
    let checkpoint = Checkpoint::open(&first.path, false).unwrap();
    checkpoint.save(&first.journal).unwrap();
    drop(checkpoint);
    let checkpoint = Checkpoint::open(&second.path, false).unwrap();
    checkpoint.save(&second.journal).unwrap();
    drop(checkpoint);
    std::fs::copy(first.ledger(), second.ledger()).unwrap();
    assert!(Checkpoint::read_only(&second.path, &second.journal.identity).is_err());
}

#[test]
fn legacy_nonrecursive_snapshots_resume_without_include_parent_or_filters() {
    let mut fixture = Fixture::new(1);
    fixture.complete(0);
    std::fs::write(&fixture.path, serde_json::to_vec(&fixture.journal).unwrap()).unwrap();
    let mut expected = fixture.journal.identity.clone();
    expected.selection_policy = Some(crate::handler::share_transfer_options::SelectionPolicy {
        is_include_parent: false,
        include: None,
        exclude: None,
        overwrite: "no-clobber".into(),
    });
    let checkpoint = Checkpoint::open(&fixture.path, true).unwrap();
    let restored = checkpoint.load(&expected).unwrap();
    assert_eq!(restored.completed, fixture.journal.completed);
    checkpoint.save(&restored).unwrap();
    drop(checkpoint);
    assert!(Checkpoint::read_only(&fixture.path, &expected).is_ok());
    expected.selection_policy.as_mut().unwrap().include = Some("*".into());
    assert!(Checkpoint::read_only(&fixture.path, &expected).is_err());
}

#[cfg(unix)]
#[test]
fn ledger_leaf_links_never_redirect_recovery_or_updates() {
    let fixture = Fixture::new(1);
    let checkpoint = Checkpoint::open(&fixture.path, false).unwrap();
    checkpoint.save(&fixture.journal).unwrap();
    drop(checkpoint);
    let victim = fixture.root.join("victim");
    std::fs::write(&victim, b"retain").unwrap();
    std::fs::remove_file(fixture.ledger()).unwrap();
    std::os::unix::fs::symlink(&victim, fixture.ledger()).unwrap();
    assert!(Checkpoint::read_only(&fixture.path, &fixture.journal.identity).is_err());
    assert_eq!(std::fs::read(victim).unwrap(), b"retain");
}

#[cfg(unix)]
#[test]
fn unlinking_or_linking_the_open_ledger_refuses_further_updates() {
    for is_hard_link in [false, true] {
        let mut fixture = Fixture::new(1);
        let checkpoint = Checkpoint::open(&fixture.path, false).unwrap();
        checkpoint.save(&fixture.journal).unwrap();
        let original = std::fs::read(fixture.ledger()).unwrap();
        let alias = fixture.root.join("alias");
        if is_hard_link {
            std::fs::hard_link(fixture.ledger(), &alias).unwrap();
        } else {
            std::fs::remove_file(fixture.ledger()).unwrap();
            std::fs::write(&alias, b"retain").unwrap();
            std::os::unix::fs::symlink(&alias, fixture.ledger()).unwrap();
        }
        fixture.complete(0);
        assert!(checkpoint.save_completion(&fixture.journal, 0).is_err());
        assert_eq!(
            std::fs::read(&alias).unwrap(),
            if is_hard_link {
                original
            } else {
                b"retain".to_vec()
            }
        );
        drop(checkpoint);
    }
}

#[test]
fn individual_updates_reject_wrong_identity_file_fingerprint_index_and_digest() {
    let mut fixture = Fixture::new(2);
    let checkpoint = Checkpoint::open(&fixture.path, false).unwrap();
    checkpoint.save(&fixture.journal).unwrap();
    fixture.complete(0);
    let mut changed = fixture.journal.clone();
    changed.identity.share_id = "other".into();
    assert!(checkpoint.save_completion(&changed, 0).is_err());
    changed = fixture.journal.clone();
    changed.manifest.files[0].remote.etag = "changed".into();
    assert!(checkpoint.save_completion(&changed, 0).is_err());
    assert!(checkpoint.save_completion(&fixture.journal, 9).is_err());
    changed = fixture.journal.clone();
    changed.completed[0] = Some("not-a-digest".into());
    assert!(checkpoint.save_completion(&changed, 0).is_err());
    checkpoint.save_completion(&fixture.journal, 0).unwrap();
    fixture.journal.completed[0] = Some("f".repeat(64));
    assert!(checkpoint.save_completion(&fixture.journal, 0).is_err());
}

#[test]
fn large_snapshot_incremental_storage_grows_only_by_fixed_record_bytes() {
    let mut fixture = Fixture::new(4096);
    let checkpoint = Checkpoint::open(&fixture.path, false).unwrap();
    checkpoint.save(&fixture.journal).unwrap();
    let original = std::fs::read(&fixture.path).unwrap();
    for ordinal in 0..128 {
        let index = ordinal * 13;
        fixture.complete(index);
        checkpoint.save_completion(&fixture.journal, index).unwrap();
    }
    assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
    assert_eq!(
        std::fs::metadata(fixture.ledger()).unwrap().len(),
        40 + 128 * 72
    );
    drop(checkpoint);
    assert_eq!(
        Checkpoint::read_only(&fixture.path, &fixture.journal.identity)
            .unwrap()
            .completed,
        fixture.journal.completed
    );
}

#[test]
fn a_failed_initial_publication_cannot_rebind_the_prepared_ledger() {
    let mut fixture = Fixture::new(1);
    let checkpoint = Checkpoint::open(&fixture.path, false).unwrap();
    std::fs::write(&fixture.path, b"another task").unwrap();
    assert!(checkpoint.save(&fixture.journal).is_err());
    std::fs::remove_file(&fixture.path).unwrap();
    fixture.complete(0);
    assert!(checkpoint.save(&fixture.journal).is_err());
    assert!(checkpoint.save_completion(&fixture.journal, 0).is_err());
}

#[test]
fn loading_then_saving_one_completion_needs_no_explicit_full_save_call() {
    let fixture = Fixture::new(2);
    let checkpoint = Checkpoint::open(&fixture.path, false).unwrap();
    checkpoint.save(&fixture.journal).unwrap();
    let original = std::fs::read(&fixture.path).unwrap();
    drop(checkpoint);
    let checkpoint = Checkpoint::open(&fixture.path, true).unwrap();
    let mut restored = checkpoint.load(&fixture.journal.identity).unwrap();
    restored.completed[1] = Some("a".repeat(64));
    checkpoint.save_completion(&restored, 1).unwrap();
    assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
    assert_eq!(
        Checkpoint::read_only(&fixture.path, &restored.identity)
            .unwrap()
            .completed,
        restored.completed
    );
}

#[test]
fn legacy_overwrite_boolean_cannot_migrate_to_a_different_strategy() {
    let mut fixture = Fixture::new(1);
    fixture.journal.identity.is_overwrite = true;
    std::fs::write(&fixture.path, serde_json::to_vec(&fixture.journal).unwrap()).unwrap();
    let mut expected = fixture.journal.identity.clone();
    expected.selection_policy = Some(crate::handler::share_transfer_options::SelectionPolicy {
        is_include_parent: false,
        include: None,
        exclude: None,
        overwrite: "force".into(),
    });
    assert!(Checkpoint::read_only(&fixture.path, &expected).is_ok());
    expected.selection_policy.as_mut().unwrap().overwrite = "newer".into();
    assert!(Checkpoint::read_only(&fixture.path, &expected).is_err());
}

#[test]
fn legacy_api_migration_is_persisted_and_survives_an_interrupted_upgrade() {
    let mut fixture = Fixture::new(1);
    fixture.journal.identity.origin = "http://inst.idsapp-boe.volces.com".into();
    fixture.complete(0);
    let original = serde_json::to_vec(&fixture.journal).unwrap();
    std::fs::write(&fixture.path, &original).unwrap();
    let mut expected = fixture.journal.identity.clone();
    expected.api_origin = Some("http://inst.idsapi-boe.volces.com".into());
    let checkpoint = Checkpoint::open(&fixture.path, true).unwrap();
    let restored = checkpoint.load(&expected).unwrap();
    assert_eq!(restored.identity.api_origin, expected.api_origin);
    checkpoint.save(&restored).unwrap();
    drop(checkpoint);
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.path).unwrap()).unwrap();
    assert_eq!(
        snapshot["identity"]["api_origin"],
        expected.api_origin.as_deref().unwrap()
    );
    assert!(Checkpoint::read_only(&fixture.path, &expected).is_ok());
    std::fs::write(&fixture.path, &original).unwrap();
    let checkpoint = Checkpoint::open(&fixture.path, true).unwrap();
    let restored = checkpoint.load(&expected).unwrap();
    checkpoint.save(&restored).unwrap();
    drop(checkpoint);
    assert_eq!(
        Checkpoint::read_only(&fixture.path, &expected)
            .unwrap()
            .completed,
        fixture.journal.completed
    );
}

#[test]
fn marked_snapshots_keep_the_original_binding_during_api_normalization() {
    let mut fixture = Fixture::new(1);
    fixture.journal.identity.origin = "http://inst.idsapp-boe.volces.com".into();
    let checkpoint = Checkpoint::open(&fixture.path, false).unwrap();
    checkpoint.save(&fixture.journal).unwrap();
    let original = std::fs::read(&fixture.path).unwrap();
    drop(checkpoint);
    let mut expected = fixture.journal.identity.clone();
    expected.api_origin = Some("http://inst.idsapi-boe.volces.com".into());
    let checkpoint = Checkpoint::open(&fixture.path, true).unwrap();
    let mut restored = checkpoint.load(&expected).unwrap();
    restored.completed[0] = Some("a".repeat(64));
    checkpoint.save_completion(&restored, 0).unwrap();
    assert_eq!(std::fs::read(&fixture.path).unwrap(), original);
    drop(checkpoint);
    assert_eq!(
        Checkpoint::read_only(&fixture.path, &expected)
            .unwrap()
            .completed,
        restored.completed
    );
}
