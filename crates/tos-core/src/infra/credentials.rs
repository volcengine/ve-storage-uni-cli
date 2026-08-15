/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Encrypted credential persistence shared by the public CLI surfaces.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::error::CliError;
use crate::infra::config::Profile;
use crate::infra::crypto;

const CREDENTIALS_FILE_NAME: &str = "credentials.toml";
const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Credential namespace in `credentials.toml`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialSection {
    /// Shared credentials inherited by both TOS command surfaces.
    Shared,
    /// ByteCloud `tos-cli` credentials.
    Tos,
    /// Volcengine `ve-tos-cli` credentials.
    VeTos,
    /// ADrive credentials.
    ADrive,
}

/// Persisted AK/SK fields. Every field is independently optional for `config set`.
// [Review Fix #12] Secret-bearing structures intentionally omit Debug so
// future diagnostic logging cannot accidentally print raw credentials.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredAkskCredentials {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_token: Option<String>,
}

impl StoredAkskCredentials {
    /// Overlay `other` field-by-field with `other` taking precedence.
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            access_key_id: other
                .access_key_id
                .clone()
                .or_else(|| self.access_key_id.clone()),
            secret_access_key: other
                .secret_access_key
                .clone()
                .or_else(|| self.secret_access_key.clone()),
            security_token: other
                .security_token
                .clone()
                .or_else(|| self.security_token.clone()),
        }
    }

    /// Return true when no credential field is present.
    pub fn is_empty(&self) -> bool {
        self.access_key_id.is_none()
            && self.secret_access_key.is_none()
            && self.security_token.is_none()
    }

    /// Overlay present credential fields onto an existing runtime profile.
    pub fn apply_to_profile(&self, profile: &mut Profile) {
        if self.access_key_id.is_some() {
            profile.access_key_id.clone_from(&self.access_key_id);
        }
        if self.secret_access_key.is_some() {
            profile
                .secret_access_key
                .clone_from(&self.secret_access_key);
        }
        if self.security_token.is_some() {
            profile.security_token.clone_from(&self.security_token);
        }
    }
}

impl StoredOAuthCredentials {
    /// Overlay `other` OAuth fields with `other` taking precedence.
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            access_token: other
                .access_token
                .clone()
                .or_else(|| self.access_token.clone()),
            refresh_token: other
                .refresh_token
                .clone()
                .or_else(|| self.refresh_token.clone()),
            expires_at: other.expires_at.clone().or_else(|| self.expires_at.clone()),
            token_type: other.token_type.clone().or_else(|| self.token_type.clone()),
            scope: if other.scope.is_empty() {
                self.scope.clone()
            } else {
                other.scope.clone()
            },
            legacy_client_id: None,
            instance_id: other
                .instance_id
                .clone()
                .or_else(|| self.instance_id.clone()),
            user_id: other.user_id.clone().or_else(|| self.user_id.clone()),
            auth_endpoint: other
                .auth_endpoint
                .clone()
                .or_else(|| self.auth_endpoint.clone()),
        }
    }

    /// Return true when no OAuth token is present.
    pub fn is_empty(&self) -> bool {
        self.access_token.is_none() && self.refresh_token.is_none()
    }
}

/// Persisted OAuth tokens and issuer metadata used by ADrive authentication.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredOAuthCredentials {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<String>,
    // [Review Fix #1] Read the field written by pre-release OAuth builds so a
    // shared credentials file remains usable, but never persist it again.
    #[doc(hidden)]
    #[serde(default, rename = "client_id", skip_serializing)]
    pub legacy_client_id: Option<String>,
    /// IDS Instance bound to this credential set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    /// OAuth subject returned by the Token endpoint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// OAuth Authorization Server base URL that issued this credential set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_endpoint: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CredentialProfile {
    #[serde(skip_serializing_if = "Option::is_none")]
    access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret_access_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    security_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tos: Option<StoredAkskCredentials>,
    #[serde(
        rename = "ve-tos",
        alias = "ve_tos",
        skip_serializing_if = "Option::is_none"
    )]
    ve_tos: Option<StoredAkskCredentials>,
    #[serde(skip_serializing_if = "Option::is_none")]
    adrive: Option<AdriveCredentials>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AdriveCredentials {
    #[serde(skip_serializing_if = "Option::is_none")]
    access_key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret_access_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    security_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    oauth: Option<StoredOAuthCredentials>,
}

/// Versioned encrypted credential store.
#[derive(Clone, Deserialize, Serialize)]
pub struct CredentialsFile {
    schema_version: u32,
    #[serde(default, flatten)]
    profiles: BTreeMap<String, CredentialProfile>,
}

impl Default for CredentialsFile {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            profiles: BTreeMap::new(),
        }
    }
}

impl CredentialsFile {
    /// Resolve an explicit credentials path or a sibling of the config file.
    pub fn path_from(config_path: &Path, explicit_path: Option<&Path>) -> PathBuf {
        explicit_path.map(Path::to_path_buf).unwrap_or_else(|| {
            config_path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."))
                .join(CREDENTIALS_FILE_NAME)
        })
    }

    /// Load a credentials file. A missing file is an empty store.
    pub fn load_from(path: &Path) -> Result<Self, CliError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path).map_err(|error| {
            CliError::ConfigMissing(format!(
                "Failed to read credentials file {}: {}",
                path.display(),
                error
            ))
        })?;
        let store: Self = toml::from_str(&content).map_err(|_error| {
            // [Review Fix #17] TOML parser diagnostics include the source
            // line, which may contain a plaintext credential. Never echo it.
            CliError::ValidationError(format!(
                "Failed to parse credentials file {}; validate its TOML syntax and schema",
                path.display()
            ))
        })?;
        store.validate()?;
        Ok(store)
    }

    /// Return all profile names stored in this credentials file.
    pub fn profile_names(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    /// Save encrypted credentials atomically with owner-only permissions.
    pub fn save_to_path(&self, path: &Path) -> Result<(), CliError> {
        self.validate()?;
        let directory = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(directory).map_err(CliError::Io)?;
        let key = crypto::load_or_init_key(directory)?;
        let mut encrypted = self.clone();
        encrypted.encrypt_in_place(&key)?;
        let content = toml::to_string_pretty(&encrypted).map_err(|error| {
            CliError::ValidationError(format!("Failed to serialize credentials: {error}"))
        })?;
        super::atomic_file::write_owner_only_atomic(path, content.as_bytes())
    }

    /// Set one AK/SK field in a profile and credential section.
    pub fn set_aksk_field(
        &mut self,
        profile_name: &str,
        section: CredentialSection,
        field: &str,
        value: &str,
    ) -> Result<(), CliError> {
        validate_profile_name(profile_name)?;
        let profile = self.profiles.entry(profile_name.to_string()).or_default();
        match section {
            CredentialSection::Shared => set_aksk_field(
                &mut profile.access_key_id,
                &mut profile.secret_access_key,
                &mut profile.security_token,
                field,
                value,
            ),
            CredentialSection::Tos => profile
                .tos
                .get_or_insert_with(StoredAkskCredentials::default)
                .set_field(field, value),
            CredentialSection::VeTos => profile
                .ve_tos
                .get_or_insert_with(StoredAkskCredentials::default)
                .set_field(field, value),
            CredentialSection::ADrive => {
                let adrive = profile
                    .adrive
                    .get_or_insert_with(AdriveCredentials::default);
                set_aksk_field(
                    &mut adrive.access_key_id,
                    &mut adrive.secret_access_key,
                    &mut adrive.security_token,
                    field,
                    value,
                )
            }
        }
    }

    /// Read and decrypt the effective AK/SK overlay for a surface.
    pub fn effective_aksk(
        &self,
        profile_name: &str,
        section: CredentialSection,
        path: &Path,
    ) -> Result<StoredAkskCredentials, CliError> {
        let Some(profile) = self.profiles.get(profile_name) else {
            return Ok(StoredAkskCredentials::default());
        };
        let shared = if matches!(section, CredentialSection::Tos | CredentialSection::VeTos) {
            decrypted_aksk(profile.shared_aksk(), path)?
        } else {
            StoredAkskCredentials::default()
        };
        let specific = decrypted_aksk(profile.surface_aksk(section), path)?;
        Ok(shared.merge(&specific))
    }

    /// Read and decrypt ADrive OAuth credentials.
    pub fn adrive_oauth(
        &self,
        profile_name: &str,
        path: &Path,
    ) -> Result<StoredOAuthCredentials, CliError> {
        let oauth = self
            .profiles
            .get(profile_name)
            .and_then(|profile| profile.adrive.as_ref())
            .and_then(|adrive| adrive.oauth.clone())
            .unwrap_or_default();
        decrypt_oauth(oauth, path)
    }

    /// Return whether a profile contains persisted ADrive OAuth tokens without
    /// decrypting them or creating local key material.
    pub fn has_adrive_oauth_tokens(&self, profile_name: &str) -> bool {
        self.profiles
            .get(profile_name)
            .and_then(|profile| profile.adrive.as_ref())
            .and_then(|adrive| adrive.oauth.as_ref())
            .is_some_and(|oauth| oauth.access_token.is_some() || oauth.refresh_token.is_some())
    }

    /// Replace the OAuth credentials for an ADrive profile.
    pub fn set_adrive_oauth(
        &mut self,
        profile_name: &str,
        credentials: StoredOAuthCredentials,
    ) -> Result<(), CliError> {
        // [Review Fix #20] All credential setters reject the reserved metadata
        // key before mutating the in-memory store.
        validate_profile_name(profile_name)?;
        let profile = self.profiles.entry(profile_name.to_string()).or_default();
        profile
            .adrive
            .get_or_insert_with(AdriveCredentials::default)
            .oauth = Some(credentials);
        Ok(())
    }

    /// Remove OAuth credentials for an ADrive profile.
    pub fn clear_adrive_oauth(&mut self, profile_name: &str) {
        if let Some(surface) = self
            .profiles
            .get_mut(profile_name)
            .and_then(|profile| profile.adrive.as_mut())
        {
            surface.oauth = None;
        }
    }

    fn validate(&self) -> Result<(), CliError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(CliError::ValidationError(format!(
                "unsupported credentials schema_version {}; expected {}",
                self.schema_version, CURRENT_SCHEMA_VERSION
            )));
        }
        for profile_name in self.profiles.keys() {
            validate_profile_name(profile_name)?;
        }
        Ok(())
    }

    fn encrypt_in_place(&mut self, key: &[u8; 32]) -> Result<(), CliError> {
        for profile in self.profiles.values_mut() {
            encrypted_value(&mut profile.access_key_id, key)?;
            encrypted_value(&mut profile.secret_access_key, key)?;
            encrypted_value(&mut profile.security_token, key)?;
            if let Some(credentials) = profile.tos.as_mut() {
                encrypt_aksk(credentials, key)?;
            }
            if let Some(credentials) = profile.ve_tos.as_mut() {
                encrypt_aksk(credentials, key)?;
            }
            if let Some(credentials) = profile.adrive.as_mut() {
                encrypt_adrive(credentials, key)?;
            }
        }
        Ok(())
    }
}

impl CredentialProfile {
    fn shared_aksk(&self) -> StoredAkskCredentials {
        StoredAkskCredentials {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            security_token: self.security_token.clone(),
        }
    }

    fn surface_aksk(&self, section: CredentialSection) -> StoredAkskCredentials {
        match section {
            CredentialSection::Shared => self.shared_aksk(),
            CredentialSection::Tos => self.tos.clone().unwrap_or_default(),
            CredentialSection::VeTos => self.ve_tos.clone().unwrap_or_default(),
            CredentialSection::ADrive => self
                .adrive
                .as_ref()
                .map(AdriveCredentials::aksk)
                .unwrap_or_default(),
        }
    }
}

impl AdriveCredentials {
    fn aksk(&self) -> StoredAkskCredentials {
        StoredAkskCredentials {
            access_key_id: self.access_key_id.clone(),
            secret_access_key: self.secret_access_key.clone(),
            security_token: self.security_token.clone(),
        }
    }
}

impl StoredAkskCredentials {
    fn set_field(&mut self, field: &str, value: &str) -> Result<(), CliError> {
        set_aksk_field(
            &mut self.access_key_id,
            &mut self.secret_access_key,
            &mut self.security_token,
            field,
            value,
        )
    }
}

fn validate_profile_name(profile_name: &str) -> Result<(), CliError> {
    if profile_name == "schema_version" {
        return Err(CliError::ValidationError(
            "'schema_version' is reserved and cannot be used as a credentials profile".to_string(),
        ));
    }
    Ok(())
}

fn set_aksk_field(
    access_key_id: &mut Option<String>,
    secret_access_key: &mut Option<String>,
    security_token: &mut Option<String>,
    field: &str,
    value: &str,
) -> Result<(), CliError> {
    match field {
        "access_key_id" => *access_key_id = Some(value.to_string()),
        "secret_access_key" => *secret_access_key = Some(value.to_string()),
        "security_token" => *security_token = Some(value.to_string()),
        _ => {
            return Err(CliError::ValidationError(format!(
                "unsupported credential field '{field}'"
            )))
        }
    }
    Ok(())
}

fn encrypted_value(value: &mut Option<String>, key: &[u8; 32]) -> Result<(), CliError> {
    if let Some(plaintext) = value.as_ref().filter(|value| !crypto::is_encrypted(value)) {
        *value = Some(crypto::encrypt_with_key(key, plaintext)?);
    }
    Ok(())
}

fn encrypt_aksk(credentials: &mut StoredAkskCredentials, key: &[u8; 32]) -> Result<(), CliError> {
    encrypted_value(&mut credentials.access_key_id, key)?;
    encrypted_value(&mut credentials.secret_access_key, key)?;
    encrypted_value(&mut credentials.security_token, key)
}

fn encrypt_adrive(credentials: &mut AdriveCredentials, key: &[u8; 32]) -> Result<(), CliError> {
    encrypted_value(&mut credentials.access_key_id, key)?;
    encrypted_value(&mut credentials.secret_access_key, key)?;
    encrypted_value(&mut credentials.security_token, key)?;
    if let Some(oauth) = credentials.oauth.as_mut() {
        encrypted_value(&mut oauth.access_token, key)?;
        encrypted_value(&mut oauth.refresh_token, key)?;
    }
    Ok(())
}

fn decrypted_value(value: Option<String>, key: &[u8; 32]) -> Result<Option<String>, CliError> {
    value
        .map(|value| {
            if crypto::is_encrypted(&value) {
                crypto::decrypt_with_key(key, &value)
            } else {
                Ok(value)
            }
        })
        .transpose()
}

fn decrypted_aksk(
    mut credentials: StoredAkskCredentials,
    path: &Path,
) -> Result<StoredAkskCredentials, CliError> {
    if credentials.is_empty() {
        return Ok(credentials);
    }
    // [Review Fix #11] Reading plaintext compatibility input is read-only and
    // must not create a master key until an encrypted value actually exists.
    if !aksk_requires_decryption(&credentials) {
        return Ok(credentials);
    }
    let key = crypto::load_or_init_key(path.parent().unwrap_or_else(|| Path::new(".")))?;
    credentials.access_key_id = decrypted_value(credentials.access_key_id, &key)?;
    credentials.secret_access_key = decrypted_value(credentials.secret_access_key, &key)?;
    credentials.security_token = decrypted_value(credentials.security_token, &key)?;
    Ok(credentials)
}

fn decrypt_oauth(
    mut credentials: StoredOAuthCredentials,
    path: &Path,
) -> Result<StoredOAuthCredentials, CliError> {
    if credentials.access_token.is_none() && credentials.refresh_token.is_none() {
        return Ok(credentials);
    }
    if !oauth_requires_decryption(&credentials) {
        return Ok(credentials);
    }
    let key = crypto::load_or_init_key(path.parent().unwrap_or_else(|| Path::new(".")))?;
    credentials.access_token = decrypted_value(credentials.access_token, &key)?;
    credentials.refresh_token = decrypted_value(credentials.refresh_token, &key)?;
    Ok(credentials)
}

fn aksk_requires_decryption(credentials: &StoredAkskCredentials) -> bool {
    [
        credentials.access_key_id.as_deref(),
        credentials.secret_access_key.as_deref(),
        credentials.security_token.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(crypto::is_encrypted)
}

fn oauth_requires_decryption(credentials: &StoredOAuthCredentials) -> bool {
    [
        credentials.access_token.as_deref(),
        credentials.refresh_token.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(crypto::is_encrypted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "tos-credentials-{name}-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ))
    }

    #[test]
    fn encrypted_round_trip_is_surface_scoped() {
        let directory = temp_path("round-trip");
        let path = directory.join("credentials.toml");
        let mut store = CredentialsFile::default();
        store
            .set_aksk_field(
                "default",
                CredentialSection::Tos,
                "access_key_id",
                "byte-ak",
            )
            .unwrap();
        store
            .set_aksk_field(
                "default",
                CredentialSection::ADrive,
                "access_key_id",
                "adrive-ak",
            )
            .unwrap();
        store.save_to_path(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("byte-ak"));
        assert!(!raw.contains("adrive-ak"));
        assert!(raw.contains("[default.tos]"), "raw={raw}");
        assert!(raw.contains("[default.adrive]"), "raw={raw}");
        assert!(!raw.contains("[profiles."), "raw={raw}");
        assert!(!raw.contains(".aksk]"), "raw={raw}");
        let loaded = CredentialsFile::load_from(&path).unwrap();
        assert_eq!(
            loaded
                .effective_aksk("default", CredentialSection::Tos, &path)
                .unwrap()
                .access_key_id
                .as_deref(),
            Some("byte-ak")
        );
        assert_eq!(
            loaded
                .effective_aksk("default", CredentialSection::ADrive, &path)
                .unwrap()
                .access_key_id
                .as_deref(),
            Some("adrive-ak")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn shared_aksk_uses_profile_root_and_only_tos_surfaces_inherit_it() {
        let directory = temp_path("shared-root");
        let path = directory.join("credentials.toml");
        let mut store = CredentialsFile::default();
        store
            .set_aksk_field(
                "default",
                CredentialSection::Shared,
                "access_key_id",
                "shared-ak",
            )
            .unwrap();
        store.save_to_path(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("[default]"), "raw={raw}");
        assert!(!raw.contains("[default.shared]"), "raw={raw}");
        let loaded = CredentialsFile::load_from(&path).unwrap();
        for section in [CredentialSection::Tos, CredentialSection::VeTos] {
            assert_eq!(
                loaded
                    .effective_aksk("default", section, &path)
                    .unwrap()
                    .access_key_id
                    .as_deref(),
                Some("shared-ak")
            );
        }
        assert!(loaded
            .effective_aksk("default", CredentialSection::ADrive, &path)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn default_path_is_config_sibling() {
        assert_eq!(
            CredentialsFile::path_from(Path::new("/tmp/custom/config.toml"), None),
            PathBuf::from("/tmp/custom/credentials.toml")
        );
    }

    #[test]
    fn reading_plaintext_canonical_values_does_not_create_key() {
        let directory = temp_path("plaintext-read");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("credentials.toml");
        std::fs::write(
            &path,
            "schema_version = 1\n[default.adrive]\naccess_key_id = \"ak\"\n",
        )
        .unwrap();

        let loaded = CredentialsFile::load_from(&path).unwrap();
        assert_eq!(
            loaded
                .effective_aksk("default", CredentialSection::ADrive, &path)
                .unwrap()
                .access_key_id
                .as_deref(),
            Some("ak")
        );
        assert!(!directory.join(".key").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn unreleased_private_schema_is_rejected_without_migration() {
        let directory = temp_path("old-private-schema");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("credentials.toml");
        std::fs::write(
            &path,
            "schema_version = 1\n[profiles.default.adrive.aksk]\naccess_key_id = \"ak\"\n",
        )
        .unwrap();

        assert!(CredentialsFile::load_from(&path).is_err());
        assert!(!directory.join(".key").exists());
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("[profiles.default.adrive.aksk]"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn repeated_save_replaces_store_and_oauth_can_be_cleared() {
        let directory = temp_path("replace");
        let path = directory.join("credentials.toml");
        let mut store = CredentialsFile::default();
        store
            .set_adrive_oauth(
                "default",
                StoredOAuthCredentials {
                    access_token: Some("first".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        store.save_to_path(&path).unwrap();
        store
            .set_adrive_oauth(
                "default",
                StoredOAuthCredentials {
                    access_token: Some("second".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        store.save_to_path(&path).unwrap();
        assert_eq!(
            CredentialsFile::load_from(&path)
                .unwrap()
                .adrive_oauth("default", &path)
                .unwrap()
                .access_token
                .as_deref(),
            Some("second")
        );
        store.clear_adrive_oauth("default");
        assert!(store.adrive_oauth("default", &path).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn oauth_metadata_round_trip() {
        let directory = temp_path("oauth-metadata");
        let path = directory.join("credentials.toml");
        let expected = StoredOAuthCredentials {
            access_token: Some("access".to_string()),
            refresh_token: Some("refresh".to_string()),
            expires_at: Some("2026-08-01T12:00:00Z".to_string()),
            token_type: Some("Bearer".to_string()),
            scope: vec!["file:read".to_string()],
            legacy_client_id: None,
            instance_id: Some("inst-1".to_string()),
            user_id: Some("user-1".to_string()),
            auth_endpoint: Some("https://idsauth.volces.com".to_string()),
        };
        let mut store = CredentialsFile::default();
        store.set_adrive_oauth("default", expected.clone()).unwrap();
        store.save_to_path(&path).unwrap();

        let loaded = CredentialsFile::load_from(&path)
            .unwrap()
            .adrive_oauth("default", &path)
            .unwrap();
        assert_eq!(loaded.access_token, expected.access_token);
        assert_eq!(loaded.refresh_token, expected.refresh_token);
        assert_eq!(loaded.expires_at, expected.expires_at);
        assert_eq!(loaded.token_type, expected.token_type);
        assert_eq!(loaded.scope, expected.scope);
        assert_eq!(loaded.instance_id, expected.instance_id);
        assert_eq!(loaded.user_id, expected.user_id);
        assert_eq!(loaded.auth_endpoint, expected.auth_endpoint);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("client_id"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn schema_v1_oauth_without_user_id_is_read_as_none() {
        let directory = temp_path("oauth-without-user-id");
        let path = directory.join("credentials.toml");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            &path,
            "schema_version = 1\n\n[default.adrive.oauth]\ninstance_id = \"inst-1\"\nauth_endpoint = \"https://idsauth.volces.com\"\n",
        )
        .unwrap();

        let oauth = CredentialsFile::load_from(&path)
            .unwrap()
            .adrive_oauth("default", &path)
            .unwrap();

        assert_eq!(oauth.user_id, None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn oauth_merge_preserves_user_id_when_override_omits_it() {
        let shared = StoredOAuthCredentials {
            user_id: Some("user-1".to_string()),
            ..StoredOAuthCredentials::default()
        };
        let override_credentials = StoredOAuthCredentials {
            instance_id: Some("inst-1".to_string()),
            ..StoredOAuthCredentials::default()
        };

        assert_eq!(
            shared.merge(&override_credentials).user_id.as_deref(),
            Some("user-1")
        );
    }

    #[test]
    fn legacy_oauth_client_id_is_read_but_not_rewritten() {
        let directory = temp_path("legacy-oauth-client-id");
        let path = directory.join("credentials.toml");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            &path,
            "schema_version = 1\n\n[default.adrive.oauth]\nclient_id = \"legacy-client\"\ninstance_id = \"inst-1\"\nauth_endpoint = \"https://idsauth.volces.com\"\n",
        )
        .unwrap();

        let credentials = CredentialsFile::load_from(&path).unwrap();
        let oauth = credentials.adrive_oauth("default", &path).unwrap();
        assert_eq!(oauth.instance_id.as_deref(), Some("inst-1"));

        credentials.save_to_path(&path).unwrap();
        let rewritten = std::fs::read_to_string(&path).unwrap();
        assert!(!rewritten.contains("client_id"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn adrive_aksk_and_oauth_coexist_without_overriding_each_other() {
        let directory = temp_path("adrive-auth-families");
        let path = directory.join("credentials.toml");
        let mut store = CredentialsFile::default();
        store
            .set_aksk_field(
                "default",
                CredentialSection::ADrive,
                "access_key_id",
                "adrive-ak",
            )
            .unwrap();
        store
            .set_adrive_oauth(
                "default",
                StoredOAuthCredentials {
                    access_token: Some("oauth-access".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        store.save_to_path(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("[default.adrive]"), "raw={raw}");
        assert!(raw.contains("[default.adrive.oauth]"), "raw={raw}");
        assert!(!raw.contains("adrive-ak"), "raw={raw}");
        assert!(!raw.contains("oauth-access"), "raw={raw}");

        let loaded = CredentialsFile::load_from(&path).unwrap();
        assert_eq!(
            loaded
                .effective_aksk("default", CredentialSection::ADrive, &path)
                .unwrap()
                .access_key_id
                .as_deref(),
            Some("adrive-ak")
        );
        assert_eq!(
            loaded
                .adrive_oauth("default", &path)
                .unwrap()
                .access_token
                .as_deref(),
            Some("oauth-access")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn schema_version_is_rejected_as_a_profile_name() {
        let mut store = CredentialsFile::default();
        let aksk_error = store
            .set_aksk_field(
                "schema_version",
                CredentialSection::Shared,
                "access_key_id",
                "ak",
            )
            .expect_err("reserved profile");
        assert!(aksk_error.to_string().contains("reserved"));

        let oauth_error = store
            .set_adrive_oauth(
                "schema_version",
                StoredOAuthCredentials {
                    access_token: Some("token".to_string()),
                    ..Default::default()
                },
            )
            .expect_err("reserved profile");
        assert!(oauth_error.to_string().contains("reserved"));
    }

    #[cfg(not(windows))]
    #[test]
    fn failed_atomic_replacement_removes_secret_temporary_file() {
        let directory = temp_path("atomic-cleanup");
        let destination = directory.join("credentials.toml");
        std::fs::create_dir_all(&destination).unwrap();

        assert!(
            super::super::atomic_file::write_owner_only_atomic(&destination, b"secret").is_err()
        );
        let has_temporary_file = std::fs::read_dir(&directory).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("credentials.tmp-")
        });
        assert!(!has_temporary_file);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn malformed_credentials_error_does_not_echo_secret_source_line() {
        let directory = temp_path("parse-redaction");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("credentials.toml");
        std::fs::write(
            &path,
            "schema_version = 1\n[default.adrive.oauth]\naccess_token = \"TOKEN_MUST_NOT_LEAK\n",
        )
        .unwrap();

        let error = CredentialsFile::load_from(&path)
            .err()
            .expect("parse error");
        assert!(!error.to_string().contains("TOKEN_MUST_NOT_LEAK"));
        let _ = std::fs::remove_dir_all(directory);
    }
}
