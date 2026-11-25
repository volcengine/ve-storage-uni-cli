use super::*;
use crate::infra::{
    atomic_file,
    credentials::{CredentialsFile, StoredOAuthCredentials},
    crypto,
};

fn fixture() -> (std::path::PathBuf, Config) {
    let root = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("ask-unified-store-{}", uuid::Uuid::now_v7()));
    let config = Config::new(
        "synthetic-app".into(),
        "http://localhost:8080/callback",
        vec!["test:read".into()],
        root.join("ask"),
    )
    .unwrap()
    .with_credentials_store(root.join("credentials.toml"), "default".into())
    .unwrap();
    (root, config)
}

fn tokens(config: &Config) -> TokenSet {
    super::super::client::parse_tokens(br#"{"code":0,"access_token":"synthetic-access","refresh_token":"synthetic-refresh","expires_in":3600,"refresh_token_expires_in":86400,"token_type":"Bearer","scope":"test:read offline_access"}"#,
        config, now().unwrap()).unwrap()
}

#[test]
fn tokens_use_the_common_namespace_and_master_key_without_a_second_key() {
    let (root, config) = fixture();
    let store = Store::open(&config).unwrap();
    store.save_login(&tokens(&config)).unwrap();
    let raw = std::fs::read_to_string(&config.credentials_path).unwrap();
    assert!(raw.contains("[default.doubao.oauth]"));
    assert!(!raw.contains("synthetic-refresh"));
    assert!(!config.home.join("tokens").exists());
    assert!(!config.home.join(".key").exists());
    assert_eq!(std::fs::read(root.join(".key")).unwrap().len(), 32);
    let saved = CredentialsFile::load_from(&config.credentials_path)
        .unwrap()
        .doubao_oauth("default", &config.credentials_path)
        .unwrap()
        .unwrap();
    assert_eq!(saved.refresh_token, "synthetic-refresh");
    assert!(matches!(
        Store::open(&config).unwrap().status().unwrap(),
        Status::Ready { .. }
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn doubao_login_and_logout_preserve_adrive_credentials_and_reuse_its_key() {
    let (root, config) = fixture();
    CredentialsFile::update(&config.credentials_path, |credentials| {
        credentials.set_adrive_oauth(
            "default",
            StoredOAuthCredentials {
                access_token: Some("synthetic-adrive-access".into()),
                refresh_token: Some("synthetic-adrive-refresh".into()),
                ..Default::default()
            },
        )
    })
    .unwrap();
    let original_key = std::fs::read(root.join(".key")).unwrap();
    let store = Store::open(&config).unwrap();
    store.save_login(&tokens(&config)).unwrap();
    store.logout().unwrap();
    assert_eq!(std::fs::read(root.join(".key")).unwrap(), original_key);
    let credentials = CredentialsFile::load_from(&config.credentials_path).unwrap();
    assert!(!credentials.has_doubao_oauth("default"));
    assert_eq!(
        credentials
            .adrive_oauth("default", &config.credentials_path)
            .unwrap()
            .refresh_token
            .as_deref(),
        Some("synthetic-adrive-refresh")
    );
    assert!(!config.home.join(".key").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn simultaneous_provider_updates_keep_both_namespaces() {
    let (root, config) = fixture();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let writer_barrier = std::sync::Arc::clone(&barrier);
    let writer_config = config.clone();
    let writer = std::thread::spawn(move || {
        let store = Store::open(&writer_config).unwrap();
        writer_barrier.wait();
        store.save_login(&tokens(&writer_config)).unwrap();
    });
    barrier.wait();
    CredentialsFile::update(&config.credentials_path, |credentials| {
        credentials.set_adrive_oauth(
            "default",
            StoredOAuthCredentials {
                refresh_token: Some("synthetic-adrive-refresh".into()),
                ..Default::default()
            },
        )
    })
    .unwrap();
    writer.join().unwrap();
    let credentials = CredentialsFile::load_from(&config.credentials_path).unwrap();
    assert!(credentials.has_doubao_oauth("default"));
    assert_eq!(
        credentials
            .adrive_oauth("default", &config.credentials_path)
            .unwrap()
            .refresh_token
            .as_deref(),
        Some("synthetic-adrive-refresh")
    );
    assert_eq!(
        credentials
            .doubao_oauth("default", &config.credentials_path)
            .unwrap()
            .unwrap()
            .refresh_token,
        "synthetic-refresh"
    );
    assert!(!config.home.join(".key").exists());
    std::fs::remove_dir_all(root).unwrap();
}

fn seed_legacy(config: &Config, is_binary: bool) {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };
    protected::directory(&config.home).unwrap();
    let key = crypto::load_or_init_key(&config.home).unwrap();
    let snapshot = Snapshot {
        app_id: config.app_id.clone(),
        redirect_uri: config.redirect_uri.to_string(),
        required_scopes: config.required_scopes.clone(),
        tokens: tokens(config),
    };
    let plaintext = serde_json::to_string(&snapshot).unwrap();
    let ciphertext = if is_binary {
        let nonce = *b"ENC:12345678";
        let mut bytes = nonce.to_vec();
        bytes.extend(
            Aes256Gcm::new_from_slice(&key)
                .unwrap()
                .encrypt(Nonce::from_slice(&nonce), plaintext.as_bytes())
                .unwrap(),
        );
        bytes
    } else {
        crypto::encrypt_with_key(&key, &plaintext)
            .unwrap()
            .into_bytes()
    };
    atomic_file::write_owner_only_atomic(&config.home.join("tokens"), &ciphertext).unwrap();
}

#[test]
fn both_legacy_formats_migrate_once_without_generating_an_extra_key() {
    for is_binary in [true, false] {
        let (root, config) = fixture();
        seed_legacy(&config, is_binary);
        let store = Store::open(&config).unwrap();
        assert!(matches!(store.status().unwrap(), Status::Ready { .. }));
        assert!(config.home.join("tokens").exists());
        assert!(config.home.join(".key").exists());
        assert!(root.join(".key").exists());
        store.logout().unwrap();
        assert_eq!(
            Store::open(&config).unwrap().status().unwrap(),
            Status::LoggedOut
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn migration_preserves_uncertain_refresh_and_does_not_assign_it_to_other_profiles() {
    let (root, config) = fixture();
    seed_legacy(&config, false);
    atomic_file::write_owner_only_atomic(&config.home.join("pending"), b"unresolved").unwrap();
    let mut other = config.clone();
    other.profile = "other".into();
    assert_eq!(
        Store::open(&other).unwrap().status().unwrap(),
        Status::LoggedOut
    );
    let store = Store::open(&config).unwrap();
    assert_eq!(store.status().unwrap(), Status::LoginRequired);
    assert_eq!(
        std::fs::read(config.home.join("pending")).unwrap(),
        b"unresolved"
    );
    assert!(!config.credentials_path.exists());
    store.save_login(&tokens(&config)).unwrap();
    assert!(matches!(store.status().unwrap(), Status::Ready { .. }));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn profiles_share_one_key_but_not_tokens_refresh_locks_or_pending_state() {
    let (root, config) = fixture();
    let first = Store::open(&config).unwrap();
    first.save_login(&tokens(&config)).unwrap();
    let mut other = config.clone();
    other.profile = "other".into();
    let second = Store::open(&other).unwrap();
    second.save_login(&tokens(&other)).unwrap();
    let key = std::fs::read(root.join(".key")).unwrap();
    let guard = first.lock().unwrap();
    first.mark_pending(&guard).unwrap();
    assert!(matches!(second.status().unwrap(), Status::Ready { .. }));
    second.logout().unwrap();
    drop(guard);
    assert_eq!(first.status().unwrap(), Status::LoginRequired);
    assert_eq!(second.status().unwrap(), Status::LoggedOut);
    assert_eq!(std::fs::read(root.join(".key")).unwrap(), key);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn modified_ciphertext_is_rejected_and_logout_can_clear_only_that_namespace() {
    let (root, config) = fixture();
    let store = Store::open(&config).unwrap();
    store.save_login(&tokens(&config)).unwrap();
    let key = std::fs::read(root.join(".key")).unwrap();
    let raw = std::fs::read_to_string(&config.credentials_path).unwrap();
    let mut document: toml::Value = toml::from_str(&raw).unwrap();
    let token = document["default"]["doubao"]["oauth"]["access_token"]
        .as_str()
        .unwrap();
    let mut modified = token.as_bytes().to_vec();
    let index = modified.len() - 8;
    modified[index] = if modified[index] == b'A' { b'B' } else { b'A' };
    document["default"]["doubao"]["oauth"]["access_token"] =
        String::from_utf8(modified).unwrap().into();
    atomic_file::write_owner_only_atomic(
        &config.credentials_path,
        toml::to_string(&document).unwrap().as_bytes(),
    )
    .unwrap();
    assert!(matches!(store.status(), Err(Error::Storage)));
    store.logout().unwrap();
    assert_eq!(store.status().unwrap(), Status::LoggedOut);
    assert_eq!(std::fs::read(root.join(".key")).unwrap(), key);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn direct_access_cannot_bypass_a_migrated_uncertain_refresh() {
    let (root, config) = fixture();
    seed_legacy(&config, false);
    atomic_file::write_owner_only_atomic(&config.home.join("pending"), b"unresolved").unwrap();
    let store = Store::open(&config).unwrap();
    let client = super::super::Client::for_test(
        config,
        "http://127.0.0.1:1/no-request".into(),
        std::time::Duration::from_millis(50),
    );
    assert!(matches!(
        store.access_token(&client).await,
        Err(Error::LoginRequired)
    ));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_home_override_does_not_split_the_same_profile_refresh_lock() {
    let (root, config) = fixture();
    let first = Store::open(&config).unwrap();
    let mut other = config.clone();
    other.home = root.join("different-legacy-home");
    let second = Store::open(&other).unwrap();
    let guard = first.lock().unwrap();
    assert!(matches!(second.lock(), Err(Error::Busy)));
    drop(guard);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn opaque_tokens_starting_with_the_cipher_prefix_are_still_encrypted_once() {
    let (root, config) = fixture();
    let store = Store::open(&config).unwrap();
    let mut issued = tokens(&config);
    issued.access_token = Secret::new("ENC:synthetic-opaque-access".into());
    issued.refresh_token = Secret::new("ENC:synthetic-opaque-refresh".into());
    store.save_login(&issued).unwrap();
    let raw = std::fs::read_to_string(&config.credentials_path).unwrap();
    assert!(!raw.contains("ENC:synthetic-opaque-refresh"));
    let saved = CredentialsFile::load_from(&config.credentials_path)
        .unwrap()
        .doubao_oauth("default", &config.credentials_path)
        .unwrap()
        .unwrap();
    assert_eq!(saved.refresh_token, "ENC:synthetic-opaque-refresh");
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn logout_disables_legacy_import_without_deleting_unproven_files() {
    let (root, config) = fixture();
    let store = Store::open(&config).unwrap();
    store.save_login(&tokens(&config)).unwrap();
    seed_legacy(&config, false);
    let alias = root.join("legacy-hardlink");
    std::fs::hard_link(config.home.join("tokens"), &alias).unwrap();
    store.logout().unwrap();
    let saved = CredentialsFile::load_from(&config.credentials_path).unwrap();
    assert!(!saved.has_doubao_oauth("default"));
    assert!(saved.doubao_legacy_import_disabled("default"));
    assert_eq!(store.status().unwrap(), Status::LoggedOut);
    std::fs::remove_file(alias).unwrap();
    store.logout().unwrap();
    assert_eq!(store.status().unwrap(), Status::LoggedOut);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_home_overlap_never_deletes_another_stores_master_key() {
    let (root, mut config) = fixture();
    protected::directory(&root).unwrap();
    let original_path = root.join("original.toml");
    CredentialsFile::update(&original_path, |credentials| {
        credentials.set_aksk_field(
            "default",
            crate::infra::credentials::CredentialSection::ADrive,
            "access_key_id",
            "synthetic-original",
        )
    })
    .unwrap();
    let original_key = std::fs::read(root.join(".key")).unwrap();
    config.home = root.clone();
    config.credentials_path = root.join("selected/credentials.toml");
    let store = Store::open(&config).unwrap();
    store.save_login(&tokens(&config)).unwrap();
    store.logout().unwrap();
    assert_eq!(std::fs::read(root.join(".key")).unwrap(), original_key);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn legacy_migration_respects_the_pre_upgrade_refresh_lock() {
    use std::os::unix::fs::OpenOptionsExt;
    let (root, config) = fixture();
    seed_legacy(&config, false);
    let old_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(config.home.join(".lock"))
        .unwrap();
    FileExt::try_lock_exclusive(&old_lock).unwrap();
    let store = Store::open(&config).unwrap();
    assert!(matches!(store.status(), Err(Error::Busy)));
    assert!(config.home.join("tokens").exists());
    drop(old_lock);
    assert!(matches!(store.status().unwrap(), Status::Ready { .. }));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_legacy_login_can_be_imported_into_only_one_destination() {
    let (root, config) = fixture();
    seed_legacy(&config, false);
    let first = Store::open(&config).unwrap();
    assert!(matches!(first.status().unwrap(), Status::Ready { .. }));
    let mut second_config = config.clone();
    second_config.credentials_path = root.join("second/credentials.toml");
    let second = Store::open(&second_config).unwrap();
    assert_eq!(second.status().unwrap(), Status::LoggedOut);
    assert!(config.home.join("pending").exists());
    assert!(!second_config.credentials_path.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_import_claim_cannot_be_retried_in_a_different_store() {
    let (root, config) = fixture();
    seed_legacy(&config, false);
    std::fs::create_dir(root.join(".credentials.lock")).unwrap();
    let first = Store::open(&config).unwrap();
    assert!(first.status().is_err());
    let mut second_config = config.clone();
    second_config.credentials_path = root.join("second/credentials.toml");
    assert_eq!(
        Store::open(&second_config).unwrap().status().unwrap(),
        Status::LoggedOut
    );
    assert!(config.home.join("tokens").exists());
    assert!(!second_config.credentials_path.exists());
    std::fs::remove_dir_all(root).unwrap();
}
