//! Fixed-endpoint Doubao OAuth for explicit trusted runtime injection.
//!
//! `open_id` is app-scoped; `tenant_key` must be positively supplied by userinfo.
//! External/personal-account identity without a tenant is not yet a verified contract.
mod wire;

use crate::login::{
    AuthorizationRequest, ExchangeRequest, OAuthEndpoints, OAuthProvider, ProviderError,
    ProviderFuture, ProviderRefreshRequest, ProviderTokens, RefreshFailure, RefreshFuture,
};
use chrono::{NaiveDateTime, Utc};
use reqwest::{Client, ClientBuilder};
use std::time::Duration;
use url::Url;
use zeroize::Zeroize;

const AUTHORIZATION_URL: &str = "https://accounts.doubao.com/open-apis/authen/v1/authorize";
const TOKEN_URL: &str = "https://accounts.doubao.com/oauth/v3/token";
const USERINFO_URL: &str = "https://accounts.doubao.com/open-apis/authen/v1/user_info";
const OPERATION_TIMEOUT: Duration = Duration::from_secs(9);
const TOKEN_BODY_LIMIT: usize = 65536;
const USERINFO_BODY_LIMIT: usize = 32768;

/// Owned sensitive text, erased on drop and deliberately without Debug or Display.
pub struct SecretString(String);
impl SecretString {
    /// Wrap owned text without copying. Returns an opaque owner; validation belongs to its consumer.
    pub fn new(value: String) -> Self {
        Self(value)
    }
    fn as_str(&self) -> &str {
        &self.0
    }
    fn take(&mut self) -> String {
        std::mem::take(&mut self.0)
    }
}
impl Drop for SecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
impl<'de> serde::Deserialize<'de> for SecretString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

/// Trusted application credentials. Neither credentials nor their secret implement Debug.
pub struct AppCredentials {
    app_id: String,
    client_secret: SecretString,
}
impl AppCredentials {
    /// Validate owned app ID and secret as nonempty visible ASCII (256/4096-byte limits).
    /// Returns sanitized Contract on invalid input; never reads configuration or performs I/O.
    pub fn new(app_id: String, client_secret: SecretString) -> Result<Self, ProviderError> {
        if !visible_ascii(&app_id, 256) || !visible_ascii(client_secret.as_str(), 4096) {
            return Err(ProviderError::Contract);
        }
        Ok(Self {
            app_id,
            client_secret,
        })
    }
}

/// Immutable fixed-endpoint OAuth adapter. Construction never enables the standalone server.
pub struct DoubaoOAuth {
    credentials: AppCredentials,
    callback_url: String,
    required_scopes: Vec<String>,
    authorization_url: Url,
    client: Client,
    token_url: String,
    userinfo_url: String,
}
impl DoubaoOAuth {
    /// Validate trusted credentials, exact HTTPS callback and required scopes including offline_access.
    /// Returns Contract for unsafe configuration or Unavailable if the HTTP client cannot initialize.
    /// Does not perform I/O. The returned adapter implements the runtime's injectable OAuthProvider.
    pub fn new(
        credentials: AppCredentials,
        callback_url: &str,
        required_scopes: Vec<String>,
    ) -> Result<Self, ProviderError> {
        let callback = validate_callback(callback_url)?;
        let required_scopes = wire::scopes(required_scopes)?;
        if !required_scopes
            .iter()
            .any(|scope| scope == "offline_access")
        {
            return Err(ProviderError::Contract);
        }
        let mut authorization_url =
            Url::parse(AUTHORIZATION_URL).map_err(|_| ProviderError::Contract)?;
        authorization_url
            .query_pairs_mut()
            .append_pair("client_id", &credentials.app_id)
            .append_pair("response_type", "code")
            .append_pair("scope", &required_scopes.join(" "));
        // [Review Fix #57] Reject scope configurations that can never fit the service's URL bound.
        authorization(&authorization_url, callback.as_str(), &"A".repeat(64))?;
        let client = client_builder()
            .https_only(true)
            .build()
            .map_err(|_| ProviderError::Unavailable)?;
        Ok(Self {
            credentials,
            callback_url: callback.to_string(),
            required_scopes,
            authorization_url,
            client,
            token_url: TOKEN_URL.into(),
            userinfo_url: USERINFO_URL.into(),
        })
    }
    /// Return the server-owned callback and authorization allowlist for EnabledConfig.
    /// Performs no I/O and cannot fail; state and redirect_uri are appended only per authorization.
    pub fn endpoints(&self) -> OAuthEndpoints {
        OAuthEndpoints {
            callback_url: self.callback_url.clone(),
            authorization_url: self.authorization_url.to_string(),
        }
    }
    async fn token_request(
        &self,
        grant: &[(&str, &str)],
        previous_refresh: Option<&str>,
    ) -> Result<ProviderTokens, ProviderError> {
        let started = now();
        let mut form = vec![
            ("client_id", self.credentials.app_id.as_str()),
            ("client_secret", self.credentials.client_secret.as_str()),
        ];
        form.extend_from_slice(grant);
        let response = self
            .client
            .post(&self.token_url)
            .form(&form)
            .send()
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        let body = wire::read_body(response, TOKEN_BODY_LIMIT).await?;
        let mut tokens = wire::parse_token(&body, &self.required_scopes, started)?;
        // [Review Fix #55] A rotating response cannot retain the just-consumed refresh credential.
        if previous_refresh == Some(tokens.refresh_token.as_str()) {
            return Err(ProviderError::Contract);
        }
        let identity = self.identity(tokens.access_token.as_str()).await?;
        let finished = now();
        if tokens.expires_at <= finished || tokens.refresh_expires_at <= finished {
            return Err(ProviderError::Contract);
        }
        Ok(ProviderTokens {
            identity,
            scope: std::mem::take(&mut tokens.scope),
            access_token: tokens.access_token.take(),
            refresh_token: tokens.refresh_token.take(),
            expires_at: tokens.expires_at,
            refresh_expires_at: Some(tokens.refresh_expires_at),
        })
    }
    async fn identity(&self, access_token: &str) -> Result<crate::login::Identity, ProviderError> {
        let response = self
            .client
            .get(&self.userinfo_url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        let body = wire::read_body(response, USERINFO_BODY_LIMIT).await?;
        wire::parse_identity(&body)
    }
}
impl OAuthProvider for DoubaoOAuth {
    fn authorization_url(
        &self,
        request: AuthorizationRequest<'_>,
    ) -> Result<String, ProviderError> {
        if !valid_state(request.state)
            || request.callback_url != self.callback_url
            || wire::scopes(request.required_scope.to_vec())? != self.required_scopes
        {
            return Err(ProviderError::Contract);
        }
        authorization(&self.authorization_url, &self.callback_url, request.state)
    }
    fn exchange(&self, request: ExchangeRequest) -> ProviderFuture<'_> {
        Box::pin(async move {
            if request.callback_url != self.callback_url || !wire::valid_text(&request.code, 8192) {
                return Err(ProviderError::Contract);
            }
            let grant = [
                ("grant_type", "authorization_code"),
                ("code", request.code.as_str()),
                ("redirect_uri", self.callback_url.as_str()),
            ];
            tokio::time::timeout(OPERATION_TIMEOUT, self.token_request(&grant, None))
                .await
                .map_err(|_| ProviderError::Unavailable)?
        })
    }
    fn refresh(&self, request: ProviderRefreshRequest) -> RefreshFuture<'_> {
        Box::pin(async move {
            if !wire::valid_text(&request.refresh_token, 16384) {
                return Err(RefreshFailure::OutcomeUnknown);
            }
            let grant = [
                ("grant_type", "refresh_token"),
                ("refresh_token", request.refresh_token.as_str()),
            ];
            // [Review Fix #54] No documented error proves RT non-consumption; all failures stay unknown.
            tokio::time::timeout(
                OPERATION_TIMEOUT,
                self.token_request(&grant, Some(&request.refresh_token)),
            )
            .await
            .map_err(|_| RefreshFailure::OutcomeUnknown)?
            .map_err(|_| RefreshFailure::OutcomeUnknown)
        })
    }
}
fn authorization(base: &Url, callback: &str, state: &str) -> Result<String, ProviderError> {
    let mut authorization = base.clone();
    authorization
        .query_pairs_mut()
        .append_pair("state", state)
        .append_pair("redirect_uri", callback);
    if authorization.as_str().len() > 8192 {
        return Err(ProviderError::Contract);
    }
    Ok(authorization.to_string())
}
fn client_builder() -> ClientBuilder {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .retry(reqwest::retry::never())
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .read_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8))
}
fn visible_ascii(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte))
}
fn validate_callback(value: &str) -> Result<Url, ProviderError> {
    let callback = Url::parse(value).map_err(|_| ProviderError::Contract)?;
    if value.len() > 8192
        || callback.as_str() != value
        || callback.scheme() != "https"
        || callback.host_str().is_none()
        || !callback.username().is_empty()
        || callback.password().is_some()
        || callback.query().is_some()
        || callback.fragment().is_some()
        || callback.path() != "/v1/oauth/doubao/callback"
        || value.chars().any(char::is_control)
    {
        return Err(ProviderError::Contract);
    }
    Ok(callback)
}
fn valid_state(value: &str) -> bool {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    if value.len() != 64 {
        return false;
    }
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(value) else {
        return false;
    };
    bytes.len() == 48
        && URL_SAFE_NO_PAD.encode(&bytes) == value
        && uuid::Uuid::from_slice(&bytes[..16]).is_ok_and(crate::validation::is_uuid_v7)
}
fn now() -> NaiveDateTime {
    let sampled = Utc::now();
    sampled.naive_utc()
        - chrono::Duration::nanoseconds(i64::from(sampled.timestamp_subsec_nanos() % 1_000_000))
}

#[cfg(test)]
mod tests;
