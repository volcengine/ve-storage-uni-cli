use super::LoginError;
use std::collections::BTreeSet;
use url::Url;

/// Immutable application, environment and cryptographic deployment identity.
pub struct DeploymentIdentity {
    /// Application owning the OAuth registration and persisted authorization.
    pub app_id: String,
    /// Environment returned in the frozen token bundle.
    pub environment: String,
    /// Deployment bound into all cryptographic operations.
    pub deployment: String,
}

/// Trusted server-owned endpoints; authorization query values are fixed except state/redirect_uri.
pub struct OAuthEndpoints {
    /// Fixed HTTPS callback URL, without query, credentials or fragment.
    pub callback_url: String,
    /// Approved HTTPS authorization origin/path and fixed query parameters.
    pub authorization_url: String,
}

/// Validated immutable configuration. Construction performs no network or environment reads.
pub struct EnabledConfig {
    pub(super) identity: DeploymentIdentity,
    pub(super) key_version: u16,
    pub(super) required_scope: Vec<String>,
    pub(super) callback: Url,
    pub(super) authorization: Url,
}
impl EnabledConfig {
    /// Validate deployment identity, key version, required scopes and server-owned endpoints.
    /// Returns sanitized InvalidConfiguration for invalid or unsafe values; performs no I/O.
    pub fn new(
        identity: DeploymentIdentity,
        key_version: u16,
        mut required_scope: Vec<String>,
        endpoints: OAuthEndpoints,
    ) -> Result<Self, LoginError> {
        let callback = safe_url(&endpoints.callback_url)?;
        let authorization = safe_url(&endpoints.authorization_url)?;
        required_scope.sort();
        let is_valid = valid_text(&identity.app_id, 256)
            && valid_text(&identity.environment, 32)
            && valid_text(&identity.deployment, 256)
            && key_version > 0
            && callback.query().is_none()
            && callback.path() == "/v1/oauth/doubao/callback"
            && !required_scope.is_empty()
            && required_scope.len() <= 100
            && required_scope.iter().all(|scope| valid_scope(scope))
            && required_scope.windows(2).all(|pair| pair[0] != pair[1]);
        if !is_valid || !valid_fixed_query(&authorization) {
            return Err(LoginError::InvalidConfiguration);
        }
        Ok(Self {
            identity,
            key_version,
            required_scope,
            callback,
            authorization,
        })
    }
    pub(super) fn validate_authorization(
        &self,
        value: &str,
        state: &str,
    ) -> Result<(), LoginError> {
        let actual = safe_url(value).map_err(|_| LoginError::ProviderContract)?;
        let mut expected = self.authorization.clone();
        expected
            .query_pairs_mut()
            .append_pair("state", state)
            .append_pair("redirect_uri", self.callback.as_str());
        let actual_pairs = unique_query(&actual)?;
        let expected_pairs = unique_query(&expected)?;
        if actual.origin() != expected.origin()
            || actual.path() != expected.path()
            || actual_pairs != expected_pairs
            || value.len() > 8192
        {
            return Err(LoginError::ProviderContract);
        }
        Ok(())
    }
}
fn safe_url(value: &str) -> Result<Url, LoginError> {
    let parsed = Url::parse(value).map_err(|_| LoginError::InvalidConfiguration)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
        || value.chars().any(char::is_control)
    {
        return Err(LoginError::InvalidConfiguration);
    }
    Ok(parsed)
}
fn valid_fixed_query(authorization: &Url) -> bool {
    let Ok(fields) = unique_query(authorization) else {
        return false;
    };
    fields
        .keys()
        .all(|name| matches!(name.as_str(), "client_id" | "response_type" | "scope"))
}
fn unique_query(url: &Url) -> Result<std::collections::BTreeMap<String, String>, LoginError> {
    let mut pairs = std::collections::BTreeMap::new();
    for (name, value) in url.query_pairs() {
        if pairs
            .insert(name.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(LoginError::ProviderContract);
        }
    }
    Ok(pairs)
}
pub(super) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.chars().count() <= maximum && !value.chars().any(char::is_control)
}
pub(super) fn valid_scope(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| matches!(byte, 0x21 | 0x23..=0x5b | 0x5d..=0x7e))
}

/// Fresh, versioned server policy; deny rules take precedence over allow rules.
pub struct AdmissionSnapshot {
    /// Explicit enable switch; false denies everyone.
    pub is_enabled: bool,
    /// Nonempty version of the loaded server policy.
    pub version: String,
    /// Trusted UTC load-completion time, sampled in whole milliseconds like DB `UTC_TIMESTAMP(3)`.
    /// Adapters using a host clock must normalize with `DateTime::from_timestamp_millis(clock.timestamp_millis())`
    /// and manage clock alignment separately; precision normalization does not correct clock skew.
    pub fetched_at: chrono::NaiveDateTime,
    /// Allowed verified user identities.
    pub allowed_users: BTreeSet<String>,
    /// Allowed verified tenant identities.
    pub allowed_tenants: BTreeSet<String>,
    /// Denied verified user identities, evaluated first.
    pub denied_users: BTreeSet<String>,
    /// Denied verified tenant identities, evaluated first.
    pub denied_tenants: BTreeSet<String>,
}
impl AdmissionSnapshot {
    /// Construct an explicitly disabled empty policy at the supplied trusted timestamp.
    /// Returns a snapshot and performs no I/O; cannot fail.
    pub fn deny_all(fetched_at: chrono::NaiveDateTime) -> Self {
        Self {
            is_enabled: false,
            version: String::new(),
            fetched_at,
            allowed_users: BTreeSet::new(),
            allowed_tenants: BTreeSet::new(),
            denied_users: BTreeSet::new(),
            denied_tenants: BTreeSet::new(),
        }
    }
    /// Test enable/version/freshness without evaluating an identity. Future timestamps fail closed.
    /// Adapters must supply whole-millisecond UTC timestamps; finer precision is unsupported and
    /// must be normalized at the source, without relying on any future-timestamp tolerance here.
    /// Returns false for disabled, malformed, submillisecond or older-than-five-minute policy.
    pub fn is_fresh(&self, now: chrono::NaiveDateTime) -> bool {
        self.is_enabled && self.is_current(now)
    }
    // [Review Fix #33] Disabled current policy is a successful status denial, not a backend outage.
    pub(super) fn is_current(&self, now: chrono::NaiveDateTime) -> bool {
        // [Review Fix #32] DB-clock checks and the later policy deadline require one shared millisecond precision.
        let has_database_precision = self
            .fetched_at
            .and_utc()
            .timestamp_subsec_nanos()
            .is_multiple_of(1_000_000);
        has_database_precision
            && valid_text(&self.version, 256)
            && self.fetched_at <= now
            && now - self.fetched_at <= chrono::Duration::minutes(5)
    }
    /// Apply current server policy to a provider-verified identity at DB now.
    /// Returns false unless fresh and explicitly allowed, with all deny rules taking precedence.
    pub fn allows(&self, user: &str, tenant: Option<&str>, now: chrono::NaiveDateTime) -> bool {
        self.is_fresh(now)
            && !self.denied_users.contains(user)
            && !tenant.is_some_and(|tenant| self.denied_tenants.contains(tenant))
            && (self.allowed_users.contains(user)
                || tenant.is_some_and(|tenant| self.allowed_tenants.contains(tenant)))
    }
}
