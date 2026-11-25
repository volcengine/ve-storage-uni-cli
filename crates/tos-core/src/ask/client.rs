use super::{AuthorizationCode, Config, Error, PendingLogin, Result, Secret};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

const TOKEN_ENDPOINT: &str = "https://accounts.doubao.com/oauth/v3/token";
const MAX_RESPONSE: usize = 64 * 1024;
// Local defensive ceiling for both OAuth lifetimes; not a provider entitlement guarantee.
const MAX_LIFETIME: u64 = 366 * 24 * 3600;

/// Validated OAuth credentials. Sensitive fields do not implement `Debug`.
#[derive(Serialize, Deserialize)]
pub struct TokenSet {
    pub(crate) access_token: Secret,
    pub(crate) refresh_token: Secret,
    pub(crate) access_expires_at: u64,
    pub(crate) refresh_expires_at: u64,
    pub(crate) scope: String,
}
/// Fixed-endpoint OAuth client with system TLS roots, no redirects or retries.
pub struct Client {
    pub(crate) config: Config,
    secret: Secret,
    http: reqwest::Client,
    endpoint: String,
}
impl Client {
    /// Construct a bounded HTTPS client from validated config and a protected application secret.
    /// Returns a redacted configuration error if transport initialization fails.
    pub fn new(config: Config, secret: Secret) -> Result<Self> {
        if secret.expose().is_empty() || secret.expose().len() > 4096 {
            return Err(Error::Configuration);
        }
        let http = transport(true, Duration::from_secs(30))?;
        Ok(Self {
            config,
            secret,
            http,
            endpoint: TOKEN_ENDPOINT.into(),
        })
    }
    /// Bind the configured loopback callback before producing the authorization URL.
    pub async fn begin_login(&self) -> Result<PendingLogin> {
        PendingLogin::bind(&self.config).await
    }
    /// Exchange one validated callback code with its original PKCE verifier; never retries.
    pub async fn finish_code(&self, authorization: AuthorizationCode) -> Result<TokenSet> {
        if authorization.redirect_uri != self.config.redirect_uri.as_str() {
            return Err(Error::Configuration);
        }
        self.exchange(&[
            ("grant_type", "authorization_code"),
            ("client_id", &self.config.app_id),
            ("client_secret", self.secret.expose()),
            ("code", authorization.code.expose()),
            ("redirect_uri", &authorization.redirect_uri),
            ("code_verifier", authorization.verifier.expose()),
        ])
        .await
    }
    pub(crate) async fn refresh(&self, refresh_token: &Secret) -> Result<TokenSet> {
        self.exchange(&[
            ("grant_type", "refresh_token"),
            ("client_id", &self.config.app_id),
            ("client_secret", self.secret.expose()),
            ("refresh_token", refresh_token.expose()),
        ])
        .await
    }
    async fn exchange(&self, fields: &[(&str, &str)]) -> Result<TokenSet> {
        let started_at = now()?;
        let mut response = self
            .http
            .post(&self.endpoint)
            .form(fields)
            .send()
            .await
            .map_err(|_| Error::Provider)?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > MAX_RESPONSE as u64)
        {
            return Err(Error::Provider);
        }
        let mut bytes = Zeroizing::new(Vec::new());
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Provider)? {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
                return Err(Error::Response);
            }
            bytes.extend_from_slice(&chunk);
        }
        parse_tokens(&bytes, &self.config, started_at)
    }
}
fn transport(is_https: bool, timeout: Duration) -> Result<reqwest::Client> {
    // [Review Fix #3] One-time codes and rotating refresh tokens must never be retried implicitly.
    reqwest::Client::builder()
        .retry(reqwest::retry::never())
        .https_only(is_https)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .build()
        .map_err(|_| Error::Configuration)
}
#[derive(Deserialize)]
struct Response {
    code: i64,
    access_token: Secret,
    refresh_token: Secret,
    expires_in: u64,
    refresh_token_expires_in: u64,
    token_type: String,
    scope: String,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde::de::IgnoredAny>,
}
pub(crate) fn parse_tokens(bytes: &[u8], config: &Config, started_at: u64) -> Result<TokenSet> {
    // [Review Fix #7] A successful token response must be an object, never a positional array.
    if bytes.len() > MAX_RESPONSE
        || bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{')
    {
        return Err(Error::Response);
    }
    let response: Response = serde_json::from_slice(bytes).map_err(|_| Error::Response)?;
    // [Review Fix #8] Reject contradictory OAuth errors and invalid/duplicate scope strings.
    if response
        .extra
        .keys()
        .any(|key| matches!(key.as_str(), "error" | "error_description" | "error_uri"))
    {
        return Err(Error::Response);
    }
    let scopes = super::config::validated_scopes(&response.scope)?;
    let has_scopes = config
        .required_scopes
        .iter()
        .all(|needed| scopes.contains(&needed.as_str()));
    let has_tokens = valid_token(&response.access_token) && valid_token(&response.refresh_token);
    let has_lifetimes = (1..=MAX_LIFETIME).contains(&response.expires_in)
        && (1..=MAX_LIFETIME).contains(&response.refresh_token_expires_in);
    if response.code != 0
        || !response.token_type.eq_ignore_ascii_case("Bearer")
        || !has_scopes
        || !has_tokens
        || !has_lifetimes
    {
        return Err(Error::Response);
    }
    Ok(TokenSet {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        access_expires_at: started_at
            .checked_add(response.expires_in)
            .ok_or(Error::Response)?,
        refresh_expires_at: started_at
            .checked_add(response.refresh_token_expires_in)
            .ok_or(Error::Response)?,
        scope: response.scope,
    })
}
pub(crate) fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| Error::Configuration)
}
#[cfg(test)]
impl Client {
    pub(crate) fn for_test(config: Config, endpoint: String, timeout: Duration) -> Self {
        Self {
            config,
            secret: Secret::new("test-app-secret".into()),
            http: transport(false, timeout).unwrap(),
            endpoint,
        }
    }
}

fn valid_token(token: &Secret) -> bool {
    // [Review Fix #16] Opaque tokens accept visible ASCII without imposing any JWT structure.
    !token.expose().is_empty() && token.expose().bytes().all(|byte| byte.is_ascii_graphic())
}
