//! Independent Doubao state using the shared credential cipher and file writer.
//! File permissions follow the existing ADrive platform policy.
use super::{client::now, Client, Config, Error, Result, Secret, TokenSet};
use crate::infra::credentials::{CredentialsFile, StoredDoubaoOAuth};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    path::Path,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
mod codec;
mod migration;
mod protected;
#[cfg(test)]
mod tests;

/// Secret-free local authentication state. No remote call is made by status.
#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    /// No saved credentials exist.
    LoggedOut,
    /// Locally valid credentials; remote revocation is not checked.
    Ready {
        access_expires_at: u64,
        refresh_expires_at: u64,
    },
    /// Expired credentials or an unresolved one-time refresh exchange require login.
    LoginRequired,
}
/// An application-bound store. Each operation takes the same bounded OS file lock.
pub struct Store {
    config: Config,
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    app_id: String,
    redirect_uri: String,
    required_scopes: Vec<String>,
    tokens: TokenSet,
}
/// Held OS lock, released automatically on drop and by the OS after a crash.
pub(crate) struct Guard(File);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
impl Store {
    /// Open local coordination state for a profile, without generating a separate encryption key.
    /// Credentials use the common file and master key; unsafe local state returns `Storage`.
    pub fn open(config: &Config) -> Result<Self> {
        std::fs::create_dir_all(config.credentials_path.parent().ok_or(Error::Storage)?)
            .map_err(|_| Error::Storage)?;
        let store = Self {
            config: config.clone(),
        };
        protected::directory(&store.coordination_directory()?)?;
        let _guard = store.lock()?;
        protected::exists(&config.credentials_path)?;
        let key = config
            .credentials_path
            .parent()
            .ok_or(Error::Storage)?
            .join(".key");
        if protected::exists(&key)? && protected::read(&key, 32)?.len() != 32 {
            return Err(Error::Storage);
        }
        Ok(store)
    }
    pub(crate) fn lock(&self) -> Result<Guard> {
        let file = self.open_file(".lock", true)?;
        Self::lock_file(file)
    }
    fn lock_file(file: File) -> Result<Guard> {
        let started_at = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Guard(file)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(Error::Storage),
            }
            if started_at.elapsed() >= Duration::from_millis(500) {
                return Err(Error::Busy);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    async fn lock_async(&self) -> Result<Guard> {
        let file = self.open_file(".lock", true)?;
        let started_at = Instant::now();
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => return Ok(Guard(file)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err(Error::Storage),
            }
            if started_at.elapsed() >= Duration::from_millis(500) {
                return Err(Error::Busy);
            }
            // [Review Fix #15] Yield while waiting so the refresh holding this lock can complete.
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    /// Save a completed interactive login atomically and clear any unresolved refresh marker.
    pub fn save_login(&self, tokens: &TokenSet) -> Result<()> {
        let _guard = self.lock()?;
        self.save_tokens(tokens)?;
        self.remove("pending")
    }
    /// Read local expiry and pending-refresh state without exposing credentials or loading app secret.
    pub fn status(&self) -> Result<Status> {
        let _guard = self.lock()?;
        let tokens = match self.load() {
            Ok(Some(tokens)) => tokens,
            Ok(None) => return Ok(Status::LoggedOut),
            Err(Error::LoginRequired) => return Ok(Status::LoginRequired),
            Err(error) => return Err(error),
        };
        if self.has_pending()? || tokens.refresh_expires_at <= now()? {
            return Ok(Status::LoginRequired);
        }
        Ok(Status::Ready {
            access_expires_at: tokens.access_expires_at,
            refresh_expires_at: tokens.refresh_expires_at,
        })
    }
    /// Clear only the selected profile's Doubao OAuth namespace under the refresh lock.
    pub fn logout(&self) -> Result<()> {
        let _guard = self.lock()?;
        let credentials = CredentialsFile::load_from(&self.config.credentials_path)
            .map_err(|_| Error::Storage)?;
        let has_legacy = self.config.profile == "default"
            && !credentials.doubao_legacy_import_disabled(&self.config.profile)
            && self
                .config
                .home
                .join("tokens")
                .try_exists()
                .map_err(|_| Error::Storage)?;
        // [Review Fix #3] Clear OAuth and disable legacy import in the same atomic credential transaction.
        if credentials.has_doubao_oauth(&self.config.profile) || has_legacy {
            CredentialsFile::update(&self.config.credentials_path, |credentials| {
                credentials.clear_doubao_oauth(&self.config.profile);
                Ok(())
            })
            .map_err(|_| Error::Storage)?;
        }
        self.remove("pending")
    }
    /// Obtain a bearer token, refreshing under a cross-process lock when near expiry.
    /// A durable marker precedes HTTP; any unknown outcome requires interactive login.
    pub async fn access_token(&self, client: &Client) -> Result<Secret> {
        if !self.matches(&client.config) {
            return Err(Error::Configuration);
        }
        let guard = self.lock_async().await?;
        if self.has_pending()? {
            return Err(Error::LoginRequired);
        }
        let tokens = self.load()?.ok_or(Error::LoginRequired)?;
        // [Review Fix #4] Loading an old login can create a pending marker; never reuse its uncertain refresh token.
        if self.has_pending()? {
            return Err(Error::LoginRequired);
        }
        let current = now()?;
        if tokens.refresh_expires_at <= current {
            return Err(Error::LoginRequired);
        }
        if tokens.access_expires_at > current.saturating_add(60) {
            return Ok(tokens.access_token);
        }
        self.mark_pending(&guard)?;
        let rotated = client.refresh(&tokens.refresh_token).await?;
        if rotated.refresh_token.expose() == tokens.refresh_token.expose() {
            return Err(Error::Response);
        }
        self.save_tokens(&rotated)?;
        self.remove("pending")?;
        Ok(rotated.access_token)
    }
    fn matches(&self, config: &Config) -> bool {
        self.config.app_id == config.app_id
            && self.config.redirect_uri == config.redirect_uri
            && self.config.required_scopes == config.required_scopes
    }
    pub(crate) fn mark_pending(&self, _guard: &Guard) -> Result<()> {
        self.replace("pending", b"refresh outcome unresolved")
    }
    fn save_tokens(&self, tokens: &TokenSet) -> Result<()> {
        let stored = StoredDoubaoOAuth {
            app_id: self.config.app_id.clone(),
            redirect_uri: self.config.redirect_uri.to_string(),
            required_scopes: self.config.required_scopes.clone(),
            access_token: tokens.access_token.expose().to_owned(),
            refresh_token: tokens.refresh_token.expose().to_owned(),
            access_expires_at: tokens.access_expires_at,
            refresh_expires_at: tokens.refresh_expires_at,
            scope: tokens.scope.clone(),
        };
        CredentialsFile::update(&self.config.credentials_path, |credentials| {
            credentials.set_doubao_oauth(&self.config.profile, stored)
        })
        .map_err(|_| Error::Storage)?;
        Ok(())
    }
    fn load(&self) -> Result<Option<TokenSet>> {
        let credentials = CredentialsFile::load_from(&self.config.credentials_path)
            .map_err(|_| Error::Storage)?;
        let Some(stored) = credentials
            .doubao_oauth(&self.config.profile, &self.config.credentials_path)
            .map_err(|_| Error::Storage)?
        else {
            if credentials.doubao_legacy_import_disabled(&self.config.profile) {
                return Ok(None);
            }
            return self.migrate_legacy();
        };
        self.validate_snapshot(Snapshot {
            app_id: stored.app_id,
            redirect_uri: stored.redirect_uri,
            required_scopes: stored.required_scopes,
            tokens: TokenSet {
                access_token: Secret::new(stored.access_token),
                refresh_token: Secret::new(stored.refresh_token),
                access_expires_at: stored.access_expires_at,
                refresh_expires_at: stored.refresh_expires_at,
                scope: stored.scope,
            },
        })
        .map(Some)
    }

    fn validate_snapshot(&self, snapshot: Snapshot) -> Result<TokenSet> {
        if snapshot.app_id != self.config.app_id
            || snapshot.redirect_uri != self.config.redirect_uri.as_str()
            || snapshot.required_scopes != self.config.required_scopes
        {
            return Err(Error::Storage);
        }
        if !self.config.required_scopes.iter().all(|needed| {
            snapshot
                .tokens
                .scope
                .split_whitespace()
                .any(|s| s == needed)
        }) {
            return Err(Error::Storage);
        }
        Ok(snapshot.tokens)
    }
}
/// Read a bounded application-secret file using the same platform permission policy.
pub(crate) fn read_secret_file(path: &Path) -> Result<Zeroizing<Vec<u8>>> {
    protected::read_external(path)
}
