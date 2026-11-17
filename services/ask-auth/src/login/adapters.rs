use super::AdmissionSnapshot;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc};
use zeroize::Zeroize;

/// Trusted provider exchange future; errors contain classifications only.
pub type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderTokens, ProviderError>> + Send + 'a>>;
/// Trusted policy retrieval future; no CLI claims are passed to this adapter.
pub type AdmissionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<AdmissionSnapshot, AdmissionError>> + Send + 'a>>;

/// Server-owned authorization parameters. No client redirect or identity is accepted.
pub struct AuthorizationRequest<'a> {
    /// Complete server-generated opaque state.
    pub state: &'a str,
    /// Immutable configured HTTPS callback URL.
    pub callback_url: &'a str,
    /// Required scope verified again after exchange.
    pub required_scope: &'a [String],
}
/// One reserved callback exchange. Dropping this value erases its authorization code.
pub struct ExchangeRequest {
    /// Sensitive one-use code; adapters must not log it.
    pub code: String,
    /// Immutable server callback, never supplied by the CLI.
    pub callback_url: String,
}
impl Drop for ExchangeRequest {
    fn drop(&mut self) {
        self.code.zeroize();
    }
}
/// Provider adapter with a proven platform contract; production provides no wire implementation.
pub trait OAuthProvider: Send + Sync {
    /// Build a browser URL from trusted server parameters; output is independently allowlisted.
    /// Returns sanitized ProviderError on failure. Must perform no external I/O or log secrets.
    fn authorization_url(&self, request: AuthorizationRequest<'_>)
        -> Result<String, ProviderError>;
    /// Exchange once after reservation commits; returns verified identity/scope and actual expiries.
    /// The service cancels after ten seconds and never retries or takes over this operation.
    fn exchange(&self, request: ExchangeRequest) -> ProviderFuture<'_>;
    /// Consume one reserved refresh operation once. The effective RT must be verified usable;
    /// preserving an omitted RT requires trusted platform proof. Returns classified outcomes only.
    /// The service cancels after ten seconds and never retries this call.
    fn refresh(&self, request: ProviderRefreshRequest) -> RefreshFuture<'_>;
}
/// Sanitized provider outcome; unclassified or uncertain outcomes must be OutcomeUnknown.
#[derive(Debug, Clone, Copy)]
pub enum ProviderError {
    AccessDenied,
    Unavailable,
    OutcomeUnknown,
    Contract,
}
/// Sanitized trusted policy-source failure.
#[derive(Debug, Clone, Copy)]
pub struct AdmissionError;
/// Trusted policy adapter. Implementations must not derive policy from request claims.
pub trait AdmissionProvider: Send + Sync {
    /// Load a current immutable snapshot. Service bounds retrieval to three seconds.
    /// Returns AdmissionError on any backend failure; backend text is never exposed.
    fn snapshot(&self) -> AdmissionFuture<'_>;
}
/// Provider-verified stable identity. Deliberately has no Debug implementation.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// Nonempty verified provider user identifier.
    pub user_id: String,
    /// Verified provider tenant, or null when the provider proves none exists.
    // [Review Fix #24] Missing identity fields cannot silently become null after decryption.
    #[serde(deserialize_with = "super::models::required_nullable")]
    pub tenant_id: Option<String>,
}
/// Complete verified exchange result; secrets are erased on drop and have no Debug output.
pub struct ProviderTokens {
    /// Provider-verified identity, never copied from CLI claims.
    pub identity: Identity,
    /// Actual granted scope, independently checked against configured required scope.
    pub scope: Vec<String>,
    /// Actual access credential, without formatting assumptions beyond the frozen bound.
    pub access_token: String,
    /// Actual refresh credential.
    pub refresh_token: String,
    /// Actual provider access deadline in UTC.
    pub expires_at: NaiveDateTime,
    /// Known refresh deadline, or null when platform evidence proves it is unknown.
    pub refresh_expires_at: Option<NaiveDateTime>,
}
impl Drop for ProviderTokens {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}
/// Explicit production adapters; there is no fallback provider or implicit allow policy.
pub struct LoginAdapters {
    /// Proven platform OAuth adapter.
    pub provider: Arc<dyn OAuthProvider>,
    /// Trusted server admission policy source.
    pub admission: Arc<dyn AdmissionProvider>,
}

/// Trusted refresh result future. Unknown or uncertain consumption must be OutcomeUnknown.
pub type RefreshFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderTokens, RefreshFailure>> + Send + 'a>>;
/// Effective refresh input from an authenticated, committed reservation. No Debug output.
pub struct ProviderRefreshRequest {
    /// Sensitive RT; adapters must never log it or retry it automatically.
    pub refresh_token: String,
}
impl Drop for ProviderRefreshRequest {
    fn drop(&mut self) {
        self.refresh_token.zeroize();
    }
}
/// Explicit platform-proven no-consumption reason. No provider text is accepted.
#[derive(Clone, Copy)]
pub enum NoConsumptionReason {
    /// A transient outage with proof that no credential was consumed.
    Unavailable,
    /// Provider response invalid, with proof that no credential was consumed.
    Contract,
    /// Credential is known invalid, with proof that no credential was consumed.
    CredentialInvalid,
}
/// Trusted refresh failure classification. Only NoConsumption permits a failed ledger.
#[derive(Clone, Copy)]
pub enum RefreshFailure {
    /// Platform-proven no consumption; the service may retain a sanitized failure.
    NoConsumption(NoConsumptionReason),
    /// Timeout, transport loss, unclassified error or uncertain consumption.
    OutcomeUnknown,
}
