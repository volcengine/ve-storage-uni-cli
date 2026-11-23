use super::*;

const SOURCE_IMPORT_DISABLED: &[u8] = b"storage-cli-legacy-import-disabled-v1";

impl Store {
    fn legacy_lock(&self) -> Result<Guard> {
        protected::directory(&self.config.home)?;
        // [Review Fix #6] Coordinate with pre-upgrade processes while reading the old one-time refresh state.
        Self::lock_file(protected::open_file(&self.config.home.join(".lock"), true)?)
    }

    pub(super) fn migrate_legacy(&self) -> Result<Option<TokenSet>> {
        // Old logins were global: only default may adopt them.
        if self.config.profile != "default" || !protected::exists(&self.config.home.join("tokens"))?
        {
            return Ok(None);
        }
        let _legacy_guard = self.legacy_lock()?;
        let pending_path = self.config.home.join("pending");
        if protected::exists(&pending_path)? {
            if protected::read(&pending_path, 4096)?.starts_with(SOURCE_IMPORT_DISABLED) {
                return Ok(None);
            }
            // [Review Fix #9] Never replace an existing uncertainty marker, including Windows' backup/rename window.
            return Err(Error::LoginRequired);
        }
        let Some(plaintext) = self.decrypt_legacy()? else {
            return Ok(None);
        };
        let snapshot = serde_json::from_slice(&plaintext).map_err(|_| Error::Storage)?;
        let tokens = self.validate_snapshot(snapshot)?;
        // [Review Fix #8] Claim the source before committing: older clients and other destinations must never reuse its one-time token.
        crate::infra::atomic_file::write_owner_only_atomic(&pending_path, SOURCE_IMPORT_DISABLED)
            .map_err(|_| Error::Storage)?;
        // Commit the login and import tombstone together. A failed claim is recovered by fresh login, never a repeated import.
        self.save_tokens(&tokens)?;
        Ok(Some(tokens))
    }
}
