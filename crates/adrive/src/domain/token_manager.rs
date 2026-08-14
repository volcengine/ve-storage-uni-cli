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

//! On-demand ADrive OAuth Access Token selection and refresh coordination.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use fs2::FileExt;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use tokio::time::Instant;
use tos_core::agent::error::CliError;
use tos_core::infra::config::DEFAULT_HTTP_MAX_RETRY_COUNT;
use tos_core::infra::credentials::{CredentialsFile, StoredOAuthCredentials};

use super::auth::resolve_oauth_client_id;
use super::client::ClientOptions;
use super::oauth::{OAuthClient, OAuthClientError, OAuthFailure, TokenResponse};

pub(crate) const ACCESS_TOKEN_REFRESH_WINDOW_SECONDS: i64 = 60;
const REFRESH_LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const REFRESH_LOCK_RETRY_DELAY: Duration = Duration::from_millis(50);
const REFRESH_RETRY_BASE_DELAY: Duration = Duration::from_millis(200);
const MAX_REFRESH_RETRY_DELAY: Duration = Duration::from_secs(30);
const MAX_REFRESH_RETRIES: u32 = 10;

static PROCESS_REFRESH_LOCKS: OnceLock<StdMutex<HashMap<PathBuf, Arc<AsyncMutex<()>>>>> =
    OnceLock::new();

#[derive(Clone)]
enum CredentialSource {
    File,
    Environment { access_token: Option<String> },
}

/// Cloneable manager for one ADrive Profile's selected OAuth credential group.
///
/// File credentials are selected as a complete group ahead of environment
/// credentials. Environment credentials are read-only and never refreshed.
#[derive(Clone)]
pub struct OAuthTokenManager {
    credentials_path: PathBuf,
    profile_name: String,
    client_options: ClientOptions,
    source: CredentialSource,
}

impl OAuthTokenManager {
    /// Select the file credential group or the process environment group.
    pub fn new(
        credentials_path: PathBuf,
        profile_name: String,
        client_options: ClientOptions,
    ) -> Result<Self, CliError> {
        Self::new_with_environment(
            credentials_path,
            profile_name,
            client_options,
            std::env::var("ADRIVE_ACCESS_TOKEN").ok(),
            std::env::var("ADRIVE_REFRESH_TOKEN").ok(),
        )
    }

    pub(crate) fn new_with_environment(
        credentials_path: PathBuf,
        profile_name: String,
        client_options: ClientOptions,
        environment_access_token: Option<String>,
        _environment_refresh_token: Option<String>,
    ) -> Result<Self, CliError> {
        // [Review Fix #4] A blank Profile would create an unaddressable lock
        // namespace and bypass the normal config/Profile validation path.
        if profile_name.trim().is_empty() {
            return Err(CliError::ValidationError(
                "Invalid profile name: profile must not be empty".to_string(),
            ));
        }
        let stored = load_oauth(&credentials_path, &profile_name)?;
        let source = if stored.is_empty() {
            CredentialSource::Environment {
                access_token: nonempty(environment_access_token),
            }
        } else {
            CredentialSource::File
        };
        Ok(Self {
            credentials_path,
            profile_name,
            client_options,
            source,
        })
    }

    /// Return a usable Access Token, refreshing file credentials when needed.
    pub async fn access_token(&self) -> Result<String, CliError> {
        match &self.source {
            CredentialSource::Environment { access_token } => access_token
                .clone()
                .ok_or_else(|| login_required("environment Access Token is missing")),
            CredentialSource::File => {
                let stored = load_oauth(&self.credentials_path, &self.profile_name)?;
                if let Some(access_token) = reusable_access_token(&stored, true)? {
                    return Ok(access_token);
                }
                self.refresh_file(None).await
            }
        }
    }

    /// Return the selected OAuth Access Token and bound user ID from one credential state.
    pub(crate) async fn access_token_and_user_id(
        &self,
        apply_refresh_window: bool,
    ) -> Result<(String, Option<String>), CliError> {
        match &self.source {
            CredentialSource::Environment { access_token } => access_token
                .clone()
                .map(|token| (token, None))
                .ok_or_else(|| login_required("environment Access Token is missing")),
            CredentialSource::File => {
                let stored = load_oauth(&self.credentials_path, &self.profile_name)?;
                if let Some(snapshot) = oauth_credential_snapshot(&stored, apply_refresh_window)? {
                    return Ok(snapshot);
                }
                let _guard =
                    RefreshLockGuard::acquire(&self.credentials_path, &self.profile_name).await?;
                let stored = load_oauth(&self.credentials_path, &self.profile_name)?;
                if let Some(snapshot) = oauth_credential_snapshot(&stored, apply_refresh_window)? {
                    return Ok(snapshot);
                }
                self.refresh_locked(stored).await?;
                let refreshed = load_oauth(&self.credentials_path, &self.profile_name)?;
                // [Review Fix #4] A just-refreshed Token is valid until expiry even
                // inside the proactive refresh window used before a refresh.
                oauth_credential_snapshot(&refreshed, false)?.ok_or_else(|| {
                    login_required(
                        "OAuth credentials do not contain a usable Access Token after refresh",
                    )
                })
            }
        }
    }

    /// Force one coordinated refresh after the Resource Server rejects a Token.
    pub async fn force_refresh(&self, rejected_access_token: &str) -> Result<String, CliError> {
        if matches!(self.source, CredentialSource::Environment { .. }) {
            return Err(login_required(
                "environment OAuth credentials cannot be refreshed; provide a new Access Token",
            ));
        }
        self.refresh_file(Some(rejected_access_token)).await
    }

    /// Return the persisted OAuth Instance binding, when the selected source has one.
    pub(crate) fn bound_instance_id(&self) -> Result<Option<String>, CliError> {
        match &self.source {
            // [Review Fix #1] Preserve missing-credential precedence: without
            // an Access Token the caller must recommend login, not an Instance.
            CredentialSource::Environment { access_token: None } => {
                return Err(login_required("environment Access Token is missing"));
            }
            CredentialSource::Environment { .. } => return Ok(None),
            CredentialSource::File => {}
        }
        let stored = load_oauth(&self.credentials_path, &self.profile_name)?;
        Ok(nonempty(stored.instance_id))
    }

    /// Return the persisted OAuth user binding, when the selected source has one.
    pub(crate) fn bound_user_id(&self) -> Result<Option<String>, CliError> {
        match &self.source {
            CredentialSource::Environment { access_token: None } => {
                return Err(login_required("environment Access Token is missing"));
            }
            CredentialSource::Environment { .. } => return Ok(None),
            CredentialSource::File => {}
        }
        let stored = load_oauth(&self.credentials_path, &self.profile_name)?;
        Ok(normalize_user_id(stored.user_id))
    }

    /// Reject a known target Instance that differs from file credential metadata.
    pub fn validate_instance(&self, target_instance: &str) -> Result<(), CliError> {
        if matches!(self.source, CredentialSource::Environment { .. }) {
            return Ok(());
        }
        let stored = load_oauth(&self.credentials_path, &self.profile_name)?;
        if stored
            .instance_id
            .as_deref()
            .is_some_and(|instance_id| instance_id != target_instance)
        {
            return Err(login_required(
                "OAuth credentials belong to a different ADrive Instance",
            ));
        }
        Ok(())
    }

    /// Reject a Resource endpoint that shares the stored Authorization origin.
    pub fn validate_resource_origin(&self, resource_endpoint: &str) -> Result<(), CliError> {
        if matches!(self.source, CredentialSource::Environment { .. }) {
            return Ok(());
        }
        let stored = load_oauth(&self.credentials_path, &self.profile_name)?;
        let Some(auth_endpoint) = stored.auth_endpoint.as_deref() else {
            return Ok(());
        };
        let auth = reqwest::Url::parse(auth_endpoint).map_err(|_| {
            CliError::ValidationError("stored OAuth auth_endpoint is invalid".to_string())
        })?;
        let resource = reqwest::Url::parse(resource_endpoint).map_err(|_| {
            CliError::ValidationError("ADrive resource endpoint is invalid".to_string())
        })?;
        if same_origin(&auth, &resource) {
            return Err(CliError::ValidationError(
                "ADrive auth endpoint and resource endpoint must use different origins".to_string(),
            ));
        }
        Ok(())
    }

    /// Return the stable selected credential source label.
    pub fn source(&self) -> &'static str {
        match self.source {
            CredentialSource::File => "credentials_file",
            CredentialSource::Environment { .. } => "environment",
        }
    }

    /// Redact the selected Access Token if a Resource Server echoes it.
    pub(crate) fn redact_resource_error(&self, message: &str) -> String {
        let access_token = match &self.source {
            CredentialSource::Environment { access_token } => access_token.clone(),
            CredentialSource::File => load_oauth(&self.credentials_path, &self.profile_name)
                .ok()
                .and_then(|stored| nonempty(stored.access_token)),
        };
        access_token.filter(|token| !token.is_empty()).map_or_else(
            || message.to_string(),
            |token| message.replace(&token, "***"),
        )
    }

    async fn refresh_file(&self, rejected_access_token: Option<&str>) -> Result<String, CliError> {
        let _guard = RefreshLockGuard::acquire(&self.credentials_path, &self.profile_name).await?;
        let stored = load_oauth(&self.credentials_path, &self.profile_name)?;
        if let Some(rejected) = rejected_access_token {
            if stored.access_token.as_deref() != Some(rejected) {
                if let Some(access_token) = reusable_access_token(&stored, false)? {
                    return Ok(access_token);
                }
            }
        } else if let Some(access_token) = reusable_access_token(&stored, true)? {
            return Ok(access_token);
        }
        self.refresh_locked(stored).await
    }

    async fn refresh_locked(&self, stored: StoredOAuthCredentials) -> Result<String, CliError> {
        let refresh_token = required_metadata(stored.refresh_token.as_deref(), "refresh_token")?;
        let client_id = resolve_oauth_client_id();
        let instance_id = required_metadata(stored.instance_id.as_deref(), "instance_id")?;
        let user_id = normalize_user_id(stored.user_id);
        let auth_endpoint = required_metadata(stored.auth_endpoint.as_deref(), "auth_endpoint")?;
        let client = OAuthClient::new(auth_endpoint.to_string(), self.client_options.clone())
            .map_err(map_refresh_error)?;
        let response = self
            .request_refresh(&client, refresh_token, &client_id)
            .await;
        match response {
            Ok(token) => self.save_refreshed_token(&client, instance_id, user_id, token),
            Err(OAuthClientError::OAuth(failure)) if failure.code == "invalid_grant" => {
                self.clear_invalid_grant(refresh_token)?;
                Err(login_required_with_request(
                    "Refresh Token is invalid, expired, or revoked",
                    failure.request_id.as_deref(),
                ))
            }
            Err(error) => Err(map_refresh_error(error)),
        }
    }

    async fn request_refresh(
        &self,
        client: &OAuthClient,
        refresh_token: &str,
        client_id: &str,
    ) -> Result<TokenResponse, OAuthClientError> {
        let retries = bounded_refresh_retries(
            self.client_options
                .max_retry_count
                .unwrap_or(DEFAULT_HTTP_MAX_RETRY_COUNT),
        );
        for attempt in 0..=retries {
            match client.refresh_token(refresh_token, client_id).await {
                Ok(token) => return Ok(token),
                Err(error) if attempt < retries && is_retryable_refresh_error(&error) => {
                    tokio::time::sleep(refresh_retry_delay(&error, attempt)).await;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("bounded refresh loop always returns")
    }

    fn save_refreshed_token(
        &self,
        client: &OAuthClient,
        instance_id: &str,
        stored_user_id: Option<String>,
        token: TokenResponse,
    ) -> Result<String, CliError> {
        let validated = validate_refresh_token(token, instance_id)?;
        let access_token = validated.access_token.clone();
        let mut credentials = CredentialsFile::load_from(&self.credentials_path)?;
        credentials.set_adrive_oauth(
            &self.profile_name,
            StoredOAuthCredentials {
                access_token: Some(validated.access_token),
                refresh_token: Some(validated.refresh_token),
                expires_at: Some(expires_at(validated.expires_in)?),
                token_type: Some("Bearer".to_string()),
                scope: validated.scope,
                legacy_client_id: None,
                instance_id: Some(instance_id.to_string()),
                user_id: validated.user_id.or(stored_user_id),
                auth_endpoint: Some(client.endpoint().as_str().to_string()),
            },
        )?;
        credentials.save_to_path(&self.credentials_path)?;
        Ok(access_token)
    }

    fn clear_invalid_grant(&self, rejected_refresh_token: &str) -> Result<(), CliError> {
        let mut credentials = CredentialsFile::load_from(&self.credentials_path)?;
        let current = credentials.adrive_oauth(&self.profile_name, &self.credentials_path)?;
        if current.refresh_token.as_deref() == Some(rejected_refresh_token) {
            credentials.clear_adrive_oauth(&self.profile_name);
            credentials.save_to_path(&self.credentials_path)?;
        }
        Ok(())
    }
}

struct ValidatedRefreshToken {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    scope: Vec<String>,
    user_id: Option<String>,
}

fn validate_refresh_token(
    token: TokenResponse,
    expected_instance: &str,
) -> Result<ValidatedRefreshToken, CliError> {
    if token.access_token.trim().is_empty() || token.refresh_token.trim().is_empty() {
        return Err(invalid_response("Token response omitted a required token"));
    }
    if !token.token_type.eq_ignore_ascii_case("bearer") || token.expires_in == 0 {
        return Err(invalid_response(
            "Token response has an invalid token_type or expires_in",
        ));
    }
    if token.instance_id != expected_instance {
        return Err(invalid_response(
            "Token response instance_id does not match stored credentials",
        ));
    }
    let scope = token
        .scope
        .split_ascii_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if scope.is_empty() {
        return Err(invalid_response("Token response scope is empty"));
    }
    Ok(ValidatedRefreshToken {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_in: token.expires_in,
        scope,
        user_id: normalize_user_id(token.user_id),
    })
}

fn reusable_access_token(
    stored: &StoredOAuthCredentials,
    apply_refresh_window: bool,
) -> Result<Option<String>, CliError> {
    let Some(access_token) = nonempty(stored.access_token.clone()) else {
        return Ok(None);
    };
    let Some(expires_at) = stored.expires_at.as_deref() else {
        return Ok(Some(access_token));
    };
    let expiry = DateTime::parse_from_rfc3339(expires_at).map_err(|_| {
        CliError::ValidationError("OAuth credentials contain an invalid expires_at".to_string())
    })?;
    let refresh_window = if apply_refresh_window {
        chrono::Duration::seconds(ACCESS_TOKEN_REFRESH_WINDOW_SECONDS)
    } else {
        chrono::Duration::zero()
    };
    Ok((expiry.with_timezone(&Utc) > Utc::now() + refresh_window).then_some(access_token))
}

fn oauth_credential_snapshot(
    stored: &StoredOAuthCredentials,
    apply_refresh_window: bool,
) -> Result<Option<(String, Option<String>)>, CliError> {
    // [Review Fix #3] Extract both fields from one persisted record so a
    // concurrent credential save cannot pair one user's ID with another Token.
    Ok(reusable_access_token(stored, apply_refresh_window)?
        .map(|access_token| (access_token, normalize_user_id(stored.user_id.clone()))))
}

fn load_oauth(path: &Path, profile_name: &str) -> Result<StoredOAuthCredentials, CliError> {
    CredentialsFile::load_from(path)?.adrive_oauth(profile_name, path)
}

fn required_metadata<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str, CliError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            login_required(&format!(
                "OAuth credentials cannot refresh without stored {field}"
            ))
        })
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn normalize_user_id(value: Option<String>) -> Option<String> {
    // [Review Fix #1] Canonicalize user metadata while leaving opaque OAuth
    // tokens untouched; padded IDs must not reach ownership resolution.
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn same_origin(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn expires_at(expires_in: u64) -> Result<String, CliError> {
    let seconds =
        i64::try_from(expires_in).map_err(|_| invalid_response("Token lifetime is too large"))?;
    Utc::now()
        .checked_add_signed(chrono::Duration::seconds(seconds))
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
        .ok_or_else(|| invalid_response("Token expiry is outside the supported range"))
}

fn is_retryable_refresh_error(error: &OAuthClientError) -> bool {
    match error {
        OAuthClientError::Http(error) => error.is_timeout() || error.is_connect(),
        OAuthClientError::OAuth(failure) => match failure.code.as_str() {
            "temporarily_unavailable" | "server_error" => true,
            // [Review Fix #4] OAuth error semantics take precedence over an
            // inconsistent status code; known Client/Grant errors are terminal.
            "invalid_request"
            | "invalid_client"
            | "unauthorized_client"
            | "invalid_scope"
            | "invalid_grant"
            | "access_denied"
            | "unsupported_response_type"
            | "unsupported_grant_type" => false,
            _ => failure.status == 429 || failure.status >= 500,
        },
        OAuthClientError::Response(_) => false,
    }
}

fn refresh_retry_delay(error: &OAuthClientError, attempt: u32) -> Duration {
    let server_delay = match error {
        OAuthClientError::OAuth(failure) => failure.retry_after_seconds.map(Duration::from_secs),
        _ => None,
    };
    let multiplier = 1_u32.checked_shl(attempt.min(16)).unwrap_or(u32::MAX);
    let backoff = REFRESH_RETRY_BASE_DELAY
        .checked_mul(multiplier)
        .unwrap_or(MAX_REFRESH_RETRY_DELAY);
    server_delay.unwrap_or(backoff).min(MAX_REFRESH_RETRY_DELAY)
}

fn bounded_refresh_retries(configured: u32) -> u32 {
    configured.min(MAX_REFRESH_RETRIES)
}

fn map_refresh_error(error: OAuthClientError) -> CliError {
    match error {
        OAuthClientError::Http(error) => CliError::Http(error),
        OAuthClientError::Response(message) => invalid_response(&message),
        OAuthClientError::OAuth(failure) => map_refresh_failure(failure),
    }
}

fn map_refresh_failure(failure: OAuthFailure) -> CliError {
    let diagnostic = failure_diagnostic(&failure);
    match failure.code.as_str() {
        "invalid_request"
        | "invalid_scope"
        | "unsupported_response_type"
        | "unsupported_grant_type" => CliError::ValidationError(diagnostic),
        "temporarily_unavailable" => CliError::RateLimited(diagnostic),
        "server_error" => CliError::Unknown(diagnostic),
        _ => CliError::AuthFailed(diagnostic),
    }
}

fn failure_diagnostic(failure: &OAuthFailure) -> String {
    let request = failure
        .request_id
        .as_deref()
        .map(|value| format!(" (RequestId: {value})"))
        .unwrap_or_default();
    format!(
        "HTTP {} [{}] OAuth Token refresh failed{}",
        failure.status, failure.code, request
    )
}

fn login_required(message: &str) -> CliError {
    CliError::AuthFailed(format!(
        "[login_required] {message}; run ve-adrive auth login"
    ))
}

fn login_required_with_request(message: &str, request_id: Option<&str>) -> CliError {
    let request = request_id
        .map(|value| format!(" (RequestId: {value})"))
        .unwrap_or_default();
    CliError::AuthFailed(format!(
        "[login_required] {message}{request}; run ve-adrive auth login"
    ))
}

fn invalid_response(message: &str) -> CliError {
    CliError::ValidationError(format!("invalid OAuth response: {message}"))
}

struct RefreshLockGuard {
    _process_guard: OwnedMutexGuard<()>,
    file: File,
}

impl RefreshLockGuard {
    async fn acquire(credentials_path: &Path, profile_name: &str) -> Result<Self, CliError> {
        let lock_path = refresh_lock_path(credentials_path, profile_name)?;
        let deadline = Instant::now() + REFRESH_LOCK_TIMEOUT;
        let process_lock = process_refresh_lock(&lock_path)?;
        let process_guard = tokio::time::timeout_at(deadline, process_lock.lock_owned())
            .await
            .map_err(|_| refresh_lock_timeout())?;
        let file = open_owner_only_lock_file(&lock_path)?;
        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => {
                    // [Review Fix #4] The persistent coordination file carries
                    // no state or credentials; keep it empty after locking.
                    file.set_len(0)?;
                    return Ok(Self {
                        _process_guard: process_guard,
                        file,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(refresh_lock_timeout());
                    }
                    tokio::time::sleep(REFRESH_LOCK_RETRY_DELAY).await;
                }
                Err(error) => return Err(CliError::Io(error)),
            }
        }
    }
}

impl Drop for RefreshLockGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn process_refresh_lock(path: &Path) -> Result<Arc<AsyncMutex<()>>, CliError> {
    let locks = PROCESS_REFRESH_LOCKS.get_or_init(|| StdMutex::new(HashMap::new()));
    let mut locks = locks
        .lock()
        .map_err(|_| CliError::Unknown("OAuth refresh lock registry is poisoned".to_string()))?;
    Ok(locks
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone())
}

fn refresh_lock_path(credentials_path: &Path, profile_name: &str) -> Result<PathBuf, CliError> {
    let absolute = if credentials_path.is_absolute() {
        credentials_path.to_path_buf()
    } else {
        std::env::current_dir()?.join(credentials_path)
    };
    // [Review Fix #4] Canonicalize an existing credentials file so symlink and
    // relative aliases coordinate through the same cross-process lock.
    let normalized = std::fs::canonicalize(&absolute).unwrap_or(absolute);
    let mut digest = Sha256::new();
    digest.update(normalized.as_os_str().to_string_lossy().as_bytes());
    digest.update([0]);
    digest.update(profile_name.as_bytes());
    let file_name = format!(".oauth-refresh-{:x}.lock", digest.finalize());
    Ok(normalized
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(file_name))
}

fn open_owner_only_lock_file(path: &Path) -> Result<File, CliError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

fn refresh_lock_timeout() -> CliError {
    CliError::Conflict(
        "timed out waiting 10 seconds for another OAuth refresh to finish".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread;

    use chrono::{Duration, SecondsFormat, Utc};
    use tos_core::infra::credentials::{CredentialsFile, StoredOAuthCredentials};

    use super::{oauth_credential_snapshot, OAuthTokenManager};
    use crate::domain::auth::resolve_oauth_client_id;
    use crate::domain::client::ClientOptions;

    #[tokio::test]
    async fn credentials_file_group_wins_without_environment_field_mixing() {
        let (directory, credentials_path) = credentials_path();
        save_oauth(
            &credentials_path,
            StoredOAuthCredentials {
                access_token: Some("file-access".to_string()),
                refresh_token: None,
                expires_at: Some(future_expiry()),
                token_type: Some("Bearer".to_string()),
                scope: vec!["all".to_string()],
                legacy_client_id: None,
                instance_id: Some("inst-1".to_string()),
                user_id: None,
                auth_endpoint: Some("https://auth.example.com".to_string()),
            },
        );
        let manager = OAuthTokenManager::new_with_environment(
            credentials_path,
            "default".to_string(),
            ClientOptions::default(),
            Some("env-access".to_string()),
            Some("env-refresh".to_string()),
        )
        .unwrap();

        assert_eq!(manager.access_token().await.unwrap(), "file-access");
        let error = manager.force_refresh("file-access").await.unwrap_err();
        assert!(error.to_string().contains("login_required"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn environment_access_token_is_used_when_file_group_is_empty() {
        let (directory, credentials_path) = credentials_path();
        let manager = OAuthTokenManager::new_with_environment(
            credentials_path,
            "default".to_string(),
            ClientOptions::default(),
            Some("env-access".to_string()),
            Some("env-refresh-must-not-be-used".to_string()),
        )
        .unwrap();

        assert_eq!(manager.access_token().await.unwrap(), "env-access");
        assert!(manager.force_refresh("env-access").await.is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn file_credentials_expose_bound_instance_id() {
        let (directory, credentials_path) = credentials_path();
        save_oauth(
            &credentials_path,
            StoredOAuthCredentials {
                access_token: Some("access".to_string()),
                instance_id: Some("inst-1".to_string()),
                ..StoredOAuthCredentials::default()
            },
        );
        let manager = file_manager(credentials_path);

        assert_eq!(
            manager.bound_instance_id().unwrap().as_deref(),
            Some("inst-1")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn file_credentials_expose_bound_user_id() {
        let (directory, credentials_path) = credentials_path();
        save_oauth(
            &credentials_path,
            StoredOAuthCredentials {
                access_token: Some("access".to_string()),
                user_id: Some("  user-1  ".to_string()),
                ..StoredOAuthCredentials::default()
            },
        );
        let manager = file_manager(credentials_path);

        assert_eq!(manager.bound_user_id().unwrap().as_deref(), Some("user-1"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn oauth_credential_snapshot_keeps_token_and_user_from_one_record() {
        // [Review Fix #3] A refreshed request must not combine fields from different saves.
        let old = StoredOAuthCredentials {
            access_token: Some("access-old".to_string()),
            expires_at: Some(future_expiry()),
            user_id: Some("user-old".to_string()),
            ..Default::default()
        };
        let refreshed = StoredOAuthCredentials {
            access_token: Some("access-new".to_string()),
            expires_at: Some(future_expiry()),
            user_id: Some("user-new".to_string()),
            ..Default::default()
        };

        assert_eq!(
            oauth_credential_snapshot(&old, true).unwrap(),
            Some(("access-old".to_string(), Some("user-old".to_string())))
        );
        assert_eq!(
            oauth_credential_snapshot(&refreshed, true).unwrap(),
            Some(("access-new".to_string(), Some("user-new".to_string())))
        );
    }

    #[test]
    fn oauth_snapshot_accepts_fresh_short_lived_token_after_refresh() {
        // [Review Fix #4] The refresh window triggers renewal, not rejection of
        // a successful response that has not yet expired.
        let refreshed = StoredOAuthCredentials {
            access_token: Some("access-new".to_string()),
            expires_at: Some(
                (Utc::now() + Duration::seconds(30)).to_rfc3339_opts(SecondsFormat::Secs, true),
            ),
            user_id: Some("user-new".to_string()),
            ..Default::default()
        };

        assert_eq!(oauth_credential_snapshot(&refreshed, true).unwrap(), None);
        assert_eq!(
            oauth_credential_snapshot(&refreshed, false).unwrap(),
            Some(("access-new".to_string(), Some("user-new".to_string())))
        );
    }

    #[test]
    fn environment_credentials_have_no_bound_instance_id() {
        let (directory, credentials_path) = credentials_path();
        let manager = OAuthTokenManager::new_with_environment(
            credentials_path,
            "default".to_string(),
            ClientOptions::default(),
            Some("access".to_string()),
            None,
        )
        .unwrap();

        assert_eq!(manager.bound_instance_id().unwrap(), None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn environment_credentials_have_no_bound_user_id() {
        let (directory, credentials_path) = credentials_path();
        let manager = OAuthTokenManager::new_with_environment(
            credentials_path,
            "default".to_string(),
            ClientOptions::default(),
            Some("access".to_string()),
            None,
        )
        .unwrap();

        assert_eq!(manager.bound_user_id().unwrap(), None);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn missing_environment_access_token_requires_login_before_metadata_resolution() {
        let (directory, credentials_path) = credentials_path();
        let manager = OAuthTokenManager::new_with_environment(
            credentials_path,
            "default".to_string(),
            ClientOptions::default(),
            None,
            None,
        )
        .unwrap();

        let instance_error = manager.bound_instance_id().unwrap_err();
        let user_error = manager.bound_user_id().unwrap_err();

        assert!(instance_error.to_string().contains("login_required"));
        assert!(user_error.to_string().contains("login_required"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn historical_access_token_without_metadata_is_used_until_rejected() {
        let (directory, credentials_path) = credentials_path();
        save_oauth(
            &credentials_path,
            StoredOAuthCredentials {
                access_token: Some("historical-access".to_string()),
                ..StoredOAuthCredentials::default()
            },
        );
        let manager = OAuthTokenManager::new_with_environment(
            credentials_path,
            "default".to_string(),
            ClientOptions::default(),
            None,
            None,
        )
        .unwrap();

        assert_eq!(manager.access_token().await.unwrap(), "historical-access");
        assert!(manager.force_refresh("historical-access").await.is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn access_token_refreshes_inside_sixty_second_window_and_rotates_pair() {
        let (endpoint, captured) = serve_refresh_once(
            200,
            r#"{"access_token":"access-new","refresh_token":"refresh-new","token_type":"Bearer","expires_in":3600,"scope":"all","instance_id":"inst-1"}"#,
        );
        let (directory, credentials_path) = credentials_path();
        save_refreshable_oauth(&credentials_path, &endpoint, "refresh-old", near_expiry());
        let manager = file_manager(credentials_path.clone());

        assert_eq!(manager.access_token().await.unwrap(), "access-new");
        let request = captured.recv().unwrap();
        assert!(request.contains("grant_type=refresh_token"));
        assert!(request.contains("refresh_token=refresh-old"));
        // [Review Fix #3] Parse the form so inherited overrides containing
        // reserved characters are compared after URL decoding.
        let expected_client_id = resolve_oauth_client_id();
        assert_eq!(
            request_form_value(&request, "client_id").as_deref(),
            Some(expected_client_id.as_str())
        );
        let stored = load_stored(&credentials_path);
        assert_eq!(stored.access_token.as_deref(), Some("access-new"));
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-new"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn refresh_persists_a_same_value_refresh_token() {
        let (endpoint, captured) = serve_refresh_once(
            200,
            r#"{"access_token":"access-new","refresh_token":"refresh-same","token_type":"Bearer","expires_in":3600,"scope":"all","instance_id":"inst-1"}"#,
        );
        let (directory, credentials_path) = credentials_path();
        save_refreshable_oauth(&credentials_path, &endpoint, "refresh-same", expired_at());
        let manager = file_manager(credentials_path.clone());

        assert_eq!(manager.access_token().await.unwrap(), "access-new");
        captured.recv().unwrap();
        let stored = load_stored(&credentials_path);
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-same"));
        assert_eq!(stored.access_token.as_deref(), Some("access-new"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn refresh_replaces_stored_user_id_when_response_supplies_one() {
        let (endpoint, captured) = serve_refresh_once(
            200,
            r#"{"access_token":"access-new","refresh_token":"refresh-new","token_type":"Bearer","expires_in":3600,"scope":"all","user_id":"  user-new  ","instance_id":"inst-1"}"#,
        );
        let (directory, credentials_path) = credentials_path();
        save_refreshable_oauth_with_user(
            &credentials_path,
            &endpoint,
            "refresh-old",
            expired_at(),
            Some("user-old"),
        );
        let manager = file_manager(credentials_path.clone());

        assert_eq!(manager.access_token().await.unwrap(), "access-new");
        captured.recv().unwrap();
        assert_eq!(
            load_stored(&credentials_path).user_id.as_deref(),
            Some("user-new")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn refresh_without_user_id_preserves_stored_user_id() {
        let (endpoint, captured) = serve_refresh_once(
            200,
            r#"{"access_token":"access-new","refresh_token":"refresh-new","token_type":"Bearer","expires_in":3600,"scope":"all","instance_id":"inst-1"}"#,
        );
        let (directory, credentials_path) = credentials_path();
        save_refreshable_oauth_with_user(
            &credentials_path,
            &endpoint,
            "refresh-old",
            expired_at(),
            Some("user-old"),
        );
        let manager = file_manager(credentials_path.clone());

        assert_eq!(manager.access_token().await.unwrap(), "access-new");
        captured.recv().unwrap();
        assert_eq!(
            load_stored(&credentials_path).user_id.as_deref(),
            Some("user-old")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn refresh_with_blank_user_id_preserves_stored_user_id() {
        let (endpoint, captured) = serve_refresh_once(
            200,
            r#"{"access_token":"access-new","refresh_token":"refresh-new","token_type":"Bearer","expires_in":3600,"scope":"all","user_id":"   ","instance_id":"inst-1"}"#,
        );
        let (directory, credentials_path) = credentials_path();
        save_refreshable_oauth_with_user(
            &credentials_path,
            &endpoint,
            "refresh-old",
            expired_at(),
            Some("user-old"),
        );
        let manager = file_manager(credentials_path.clone());

        assert_eq!(manager.access_token().await.unwrap(), "access-new");
        captured.recv().unwrap();
        assert_eq!(
            load_stored(&credentials_path).user_id.as_deref(),
            Some("user-old")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn invalid_grant_clears_only_current_oauth_group() {
        let (endpoint, captured) = serve_refresh_once(
            400,
            r#"{"error":"invalid_grant","request_id":"req-invalid-grant"}"#,
        );
        let (directory, credentials_path) = credentials_path();
        save_refreshable_oauth(
            &credentials_path,
            &endpoint,
            "refresh-invalid",
            expired_at(),
        );
        let manager = file_manager(credentials_path.clone());

        let error = manager.access_token().await.unwrap_err();

        captured.recv().unwrap();
        assert!(error.to_string().contains("login_required"));
        assert!(error.to_string().contains("req-invalid-grant"));
        assert!(load_stored(&credentials_path).is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn terminal_refresh_error_preserves_credentials() {
        let (endpoint, captured) = serve_refresh_once(
            401,
            r#"{"error":"invalid_client","request_id":"req-invalid-client"}"#,
        );
        let (directory, credentials_path) = credentials_path();
        save_refreshable_oauth(
            &credentials_path,
            &endpoint,
            "refresh-preserved",
            expired_at(),
        );
        let manager = file_manager(credentials_path.clone());

        let error = manager.access_token().await.unwrap_err();

        captured.recv().unwrap();
        assert!(error.to_string().contains("invalid_client"));
        let stored = load_stored(&credentials_path);
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-preserved"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn concurrent_managers_consume_one_rotating_refresh_token() {
        let (endpoint, captured) = serve_refresh_once(
            200,
            r#"{"access_token":"access-shared","refresh_token":"refresh-rotated","token_type":"Bearer","expires_in":3600,"scope":"all","instance_id":"inst-1"}"#,
        );
        let (directory, credentials_path) = credentials_path();
        save_refreshable_oauth(
            &credentials_path,
            &endpoint,
            "refresh-single-use",
            expired_at(),
        );
        let first = file_manager(credentials_path.clone());
        let second = file_manager(credentials_path.clone());

        let (first_result, second_result) =
            tokio::join!(first.access_token(), second.access_token());

        assert_eq!(first_result.unwrap(), "access-shared");
        assert_eq!(second_result.unwrap(), "access-shared");
        captured.recv().unwrap();
        assert!(captured.try_recv().is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn expired_historical_credentials_without_issuer_metadata_require_login() {
        let (directory, credentials_path) = credentials_path();
        save_oauth(
            &credentials_path,
            StoredOAuthCredentials {
                access_token: Some("expired-access".to_string()),
                refresh_token: Some("refresh-without-metadata".to_string()),
                expires_at: Some(expired_at()),
                ..StoredOAuthCredentials::default()
            },
        );
        let manager = file_manager(credentials_path.clone());

        let error = manager.access_token().await.unwrap_err();

        assert!(error.to_string().contains("login_required"));
        assert!(!load_stored(&credentials_path).is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn known_instance_mismatch_is_rejected_before_resource_access() {
        let (directory, credentials_path) = credentials_path();
        save_oauth(
            &credentials_path,
            StoredOAuthCredentials {
                access_token: Some("access".to_string()),
                instance_id: Some("inst-1".to_string()),
                ..StoredOAuthCredentials::default()
            },
        );
        let manager = file_manager(credentials_path);

        assert!(manager.validate_instance("inst-2").is_err());
        assert!(manager.validate_instance("inst-1").is_ok());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn known_terminal_oauth_error_is_not_retried_even_with_5xx_status() {
        let error = super::OAuthClientError::OAuth(super::OAuthFailure {
            code: "invalid_client".to_string(),
            request_id: None,
            status: 500,
            interval: None,
            retry_after_seconds: None,
        });

        assert!(!super::is_retryable_refresh_error(&error));
    }

    #[test]
    fn empty_profile_is_rejected_before_credentials_are_read() {
        let (_directory, credentials_path) = credentials_path();

        let result = OAuthTokenManager::new_with_environment(
            credentials_path,
            String::new(),
            ClientOptions::default(),
            Some("access".to_string()),
            None,
        );

        assert!(result.is_err());
    }

    #[test]
    fn configured_refresh_retry_count_is_safely_bounded() {
        assert_eq!(super::bounded_refresh_retries(u32::MAX), 10);
        assert_eq!(super::bounded_refresh_retries(2), 2);
    }

    #[test]
    fn request_form_value_decodes_reserved_characters() {
        let request = "POST /token HTTP/1.1\r\nContent-Length: 19\r\n\r\nclient_id=test%2Bid";

        assert_eq!(
            request_form_value(request, "client_id").as_deref(),
            Some("test+id")
        );
    }

    fn credentials_path() -> (PathBuf, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-token-manager-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("credentials.toml");
        (directory, path)
    }

    fn save_oauth(path: &std::path::Path, oauth: StoredOAuthCredentials) {
        let mut credentials = CredentialsFile::default();
        credentials.set_adrive_oauth("default", oauth).unwrap();
        credentials.save_to_path(path).unwrap();
    }

    fn save_refreshable_oauth(
        path: &std::path::Path,
        endpoint: &str,
        refresh_token: &str,
        expires_at: String,
    ) {
        save_refreshable_oauth_with_user(path, endpoint, refresh_token, expires_at, None);
    }

    fn save_refreshable_oauth_with_user(
        path: &std::path::Path,
        endpoint: &str,
        refresh_token: &str,
        expires_at: String,
        user_id: Option<&str>,
    ) {
        save_oauth(
            path,
            StoredOAuthCredentials {
                access_token: Some("access-old".to_string()),
                refresh_token: Some(refresh_token.to_string()),
                expires_at: Some(expires_at),
                token_type: Some("Bearer".to_string()),
                scope: vec!["all".to_string()],
                legacy_client_id: None,
                instance_id: Some("inst-1".to_string()),
                user_id: user_id.map(ToString::to_string),
                auth_endpoint: Some(endpoint.to_string()),
            },
        );
    }

    fn file_manager(credentials_path: PathBuf) -> OAuthTokenManager {
        OAuthTokenManager::new_with_environment(
            credentials_path,
            "default".to_string(),
            ClientOptions {
                max_retry_count: Some(0),
                ..ClientOptions::default()
            },
            None,
            None,
        )
        .unwrap()
    }

    fn load_stored(path: &std::path::Path) -> StoredOAuthCredentials {
        CredentialsFile::load_from(path)
            .unwrap()
            .adrive_oauth("default", path)
            .unwrap()
    }

    fn future_expiry() -> String {
        (Utc::now() + Duration::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    fn near_expiry() -> String {
        (Utc::now() + Duration::seconds(30)).to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    fn expired_at() -> String {
        (Utc::now() - Duration::minutes(1)).to_rfc3339_opts(SecondsFormat::Secs, true)
    }

    fn serve_refresh_once(status: u16, body: &str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response_body = body.to_string();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            sender.send(request).unwrap();
            let reason = if status == 200 { "OK" } else { "Bad Request" };
            write!(
                stream,
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            )
            .unwrap();
        });
        (format!("http://{address}"), receiver)
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let content_length = String::from_utf8_lossy(&bytes[..header_end + 4])
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return String::from_utf8_lossy(&bytes).to_string();
            }
        }
    }

    fn request_form_value(request: &str, key: &str) -> Option<String> {
        let (_, body) = request.split_once("\r\n\r\n")?;
        url::form_urlencoded::parse(body.as_bytes())
            .find_map(|(name, value)| (name == key).then(|| value.into_owned()))
    }
}
