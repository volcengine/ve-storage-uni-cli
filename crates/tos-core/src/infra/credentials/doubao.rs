use super::*;

/// Doubao OAuth credentials and their application binding within one CLI profile.
/// Token fields are encrypted by the common credentials writer before persistence.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StoredDoubaoOAuth {
    pub app_id: String,
    pub redirect_uri: String,
    pub required_scopes: Vec<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: u64,
    pub refresh_expires_at: u64,
    pub scope: String,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DoubaoCredentials {
    #[serde(skip_serializing_if = "Option::is_none")]
    oauth: Option<StoredDoubaoOAuth>,
    #[serde(default, skip_serializing_if = "is_false")]
    legacy_import_disabled: bool,
    #[serde(skip)]
    is_plaintext: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl CredentialsFile {
    /// Read one profile's Doubao OAuth credentials without touching ADrive state.
    /// `path` supplies the shared key location. Returns `None` when absent and errors on decryption failure.
    pub fn doubao_oauth(
        &self,
        profile: &str,
        path: &Path,
    ) -> Result<Option<StoredDoubaoOAuth>, CliError> {
        let Some(container) = self
            .profiles
            .get(profile)
            .and_then(|entry| entry.doubao.as_ref())
        else {
            return Ok(None);
        };
        let Some(mut oauth) = container.oauth.clone() else {
            return Ok(None);
        };
        if !container.is_plaintext
            && [&oauth.access_token, &oauth.refresh_token]
                .iter()
                .any(|value| crypto::is_encrypted(value))
        {
            let directory = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            if !crypto::key_path(directory).is_file() {
                return Err(CliError::ConfigMissing(
                    "credential encryption key is unavailable".into(),
                ));
            }
            let key = crypto::load_or_init_key(directory)?;
            for value in [&mut oauth.access_token, &mut oauth.refresh_token] {
                if crypto::is_encrypted(value) {
                    *value = crypto::decrypt_with_key(&key, value)?;
                }
            }
        }
        Ok(Some(oauth))
    }

    /// Replace only a profile's Doubao OAuth namespace.
    /// Rejects invalid profile names; `save_to_path` or `update` performs encryption and persistence.
    pub fn set_doubao_oauth(
        &mut self,
        profile: &str,
        oauth: StoredDoubaoOAuth,
    ) -> Result<(), CliError> {
        validate_profile_name(profile)?;
        // [Review Fix #2] Token strings are opaque; a provider's ENC: prefix is not proof of local encryption.
        self.profiles.entry(profile.into()).or_default().doubao = Some(DoubaoCredentials {
            oauth: Some(oauth),
            is_plaintext: true,
            legacy_import_disabled: profile == "default",
        });
        Ok(())
    }

    /// Return whether one profile has Doubao credentials, without reading the key or decrypting tokens.
    pub fn has_doubao_oauth(&self, profile: &str) -> bool {
        self.profiles
            .get(profile)
            .and_then(|entry| entry.doubao.as_ref())
            .is_some_and(|entry| entry.oauth.is_some())
    }

    /// Return whether the legacy global login was superseded for this profile.
    /// This nonsecret tombstone prevents re-import after logout or an interrupted migration.
    pub fn doubao_legacy_import_disabled(&self, profile: &str) -> bool {
        self.profiles
            .get(profile)
            .and_then(|entry| entry.doubao.as_ref())
            .is_some_and(|entry| entry.legacy_import_disabled)
    }

    /// Clear only a profile's Doubao OAuth namespace; leaves ADrive and other profiles unchanged.
    pub fn clear_doubao_oauth(&mut self, profile: &str) {
        if profile == "default" {
            self.profiles.entry(profile.into()).or_default().doubao = Some(DoubaoCredentials {
                legacy_import_disabled: true,
                ..Default::default()
            });
        } else if let Some(entry) = self.profiles.get_mut(profile) {
            entry.doubao = None;
        }
    }
}

pub(super) fn encrypt(credentials: &mut DoubaoCredentials, key: &[u8; 32]) -> Result<(), CliError> {
    if let Some(oauth) = credentials.oauth.as_mut() {
        for value in [&mut oauth.access_token, &mut oauth.refresh_token] {
            if credentials.is_plaintext || !crypto::is_encrypted(value) {
                *value = crypto::encrypt_with_key(key, value)?;
            }
        }
    }
    credentials.is_plaintext = false;
    Ok(())
}
