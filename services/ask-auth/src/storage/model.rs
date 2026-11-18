use super::StorageError;
use crate::validation::is_uuid_v7;
use chrono::NaiveDateTime;
use uuid::Uuid;

/// Exact 20-column A row. No Debug: rows contain proofs, identities and ciphertext.
#[derive(Clone, PartialEq, Eq)]
pub struct Session {
    pub auth_id: Uuid,
    pub app_id: String,
    pub status: String,
    pub request_hash: Option<[u8; 32]>,
    pub login_ticket_hash: Option<[u8; 32]>,
    pub claim_challenge: Option<[u8; 32]>,
    pub oauth_state_hash: Option<[u8; 32]>,
    pub hmac_key_version: u16,
    pub encrypted_oauth_context: Option<Vec<u8>>,
    pub claim_request_id: Option<Uuid>,
    pub encrypted_result: Option<Vec<u8>>,
    pub login_expires_at: Option<NaiveDateTime>,
    pub user_id: Option<String>,
    pub tenant_id: Option<String>,
    pub scope_json: Option<Vec<String>>,
    pub broker_hash: Option<[u8; 32]>,
    pub refresh_token_hash: Option<[u8; 32]>,
    pub credential_generation: Option<u64>,
    pub auth_expires_at: Option<NaiveDateTime>,
    pub reason_code: Option<String>,
}
impl Session {
    /// Validate a complete row against the immutable deployment app; invalid rows fail closed.
    pub fn validate(&self, app: &str) -> Result<(), StorageError> {
        // [Review Fix #19] Boolean names distinguish predicates from persisted values.
        let is_identity_valid = is_uuid_v7(self.auth_id)
            && self.app_id == app
            && valid_text(app, 256)
            && self.hmac_key_version > 0
            && valid_reason(self.reason_code.as_deref());
        if !is_identity_valid
            || !self.valid_authorization()
            || !self.valid_envelopes()
            || !self.valid_phase()
        {
            return Err(StorageError::Integrity);
        }
        Ok(())
    }
    /// Clear all eight login fields together; subsequent validate still enforces phase shape.
    pub fn clear_login(&mut self) {
        self.request_hash = None;
        self.login_ticket_hash = None;
        self.claim_challenge = None;
        self.oauth_state_hash = None;
        self.encrypted_oauth_context = None;
        self.claim_request_id = None;
        self.encrypted_result = None;
        self.login_expires_at = None;
    }
    pub(crate) fn login_cleared(&self) -> bool {
        self.request_hash.is_none()
            && self.login_ticket_hash.is_none()
            && self.claim_challenge.is_none()
            && self.oauth_state_hash.is_none()
            && self.encrypted_oauth_context.is_none()
            && self.claim_request_id.is_none()
            && self.encrypted_result.is_none()
            && self.login_expires_at.is_none()
    }
    fn login_proofs(&self) -> bool {
        self.request_hash.is_some()
            && self.login_ticket_hash.is_some()
            && self.claim_challenge.is_some()
            && self.oauth_state_hash.is_some()
            && self.login_expires_at.is_some()
    }
    fn valid_envelopes(&self) -> bool {
        self.encrypted_oauth_context
            .as_ref()
            .is_none_or(|value| (32..=32768).contains(&value.len()))
            && self
                .encrypted_result
                .as_ref()
                .is_none_or(|value| (32..=262144).contains(&value.len()))
            // [Review Fix #21] Retained first-claim operation IDs follow the same v7 contract.
            && self.claim_request_id.is_none_or(is_uuid_v7)
    }
    fn valid_authorization(&self) -> bool {
        let fields = [
            self.user_id.is_some(),
            self.scope_json.is_some(),
            self.broker_hash.is_some(),
            self.refresh_token_hash.is_some(),
            self.credential_generation.is_some(),
            self.auth_expires_at.is_some(),
        ];
        if fields.iter().all(|value| !value) {
            return self.tenant_id.is_none();
        }
        fields.iter().all(|value| *value)
            && self
                .user_id
                .as_deref()
                .is_some_and(|value| valid_text(value, 256))
            && self
                .tenant_id
                .as_deref()
                .is_none_or(|value| valid_text(value, 256))
            && self.credential_generation.is_some_and(|value| value > 0)
            && self.scope_json.as_ref().is_some_and(|scopes| {
                scopes.len() <= 100
                    && scopes.iter().all(|scope| valid_scope(scope))
                    && scopes.windows(2).all(|pair| pair[0] < pair[1])
            })
    }
    fn valid_phase(&self) -> bool {
        let has_authorization = self.credential_generation.is_some();
        let has_no_payload = self.encrypted_oauth_context.is_none()
            && self.encrypted_result.is_none()
            && self.claim_request_id.is_none();
        match self.status.as_str() {
            "pending" | "exchanging" => {
                !has_authorization
                    && self.login_proofs()
                    && self.encrypted_oauth_context.is_some()
                    && self.encrypted_result.is_none()
                    && self.claim_request_id.is_none()
                    && self.reason_code.is_none()
            }
            "ready" => {
                has_authorization
                    && self.login_proofs()
                    && self.credential_generation == Some(1)
                    && self.encrypted_oauth_context.is_none()
                    && self.encrypted_result.is_some()
                    && self.claim_request_id.is_none()
                    && self.reason_code.is_none()
            }
            "active" | "refreshing" => {
                has_authorization
                    && self.reason_code.is_none()
                    && (self.login_cleared()
                        || (self.login_proofs()
                            && self.credential_generation == Some(1)
                            && self.claim_request_id.is_some()
                            && self.encrypted_result.is_some()
                            && self.encrypted_oauth_context.is_none()))
            }
            "denied" | "error" => {
                !has_authorization
                    && self.login_proofs()
                    && has_no_payload
                    && self.reason_code.is_some()
            }
            "expired" if !has_authorization => {
                self.login_proofs() && has_no_payload && self.reason_code.is_some()
            }
            "expired" | "revoked" | "reauth_required" => {
                has_authorization && self.login_cleared() && self.reason_code.is_some()
            }
            _ => false,
        }
    }
    /// Validate immutable identity, complete shapes and legal phase/generation/key transition.
    /// Both rows must belong to app; invalid transition returns Conflict without mutation.
    pub fn validate_transition(&self, next: &Self, app: &str) -> Result<(), StorageError> {
        self.validate(app)?;
        next.validate(app)?;
        if self.auth_id != next.auth_id
            || self.app_id != next.app_id
            || !self.legal_edge(next)
            || !self.valid_generation_change(next)
            || !self.stable_authorization(next)
            || !self.stable_login(next)
        {
            return Err(StorageError::Conflict);
        }
        Ok(())
    }
    // [Review Fix #2] A legal state edge alone must not permit replacing leases or payloads.
    fn legal_edge(&self, next: &Self) -> bool {
        let mut allowed = self.clone();
        allowed.status = next.status.clone();
        match (self.status.as_str(), next.status.as_str()) {
            ("pending", "exchanging") | ("active", "refreshing") => {}
            ("exchanging", "ready") => {
                allowed.establish_authorization(next);
            }
            ("exchanging", "denied" | "error" | "expired") | ("pending", "expired") => {
                allowed.encrypted_oauth_context = None;
                allowed.reason_code = next.reason_code.clone();
                if next.login_expires_at > self.login_expires_at {
                    return false;
                }
                allowed.login_expires_at = next.login_expires_at;
            }
            ("ready", "active") => {
                // [Review Fix #11] Claim recovery cannot outlive the authorization protecting its result.
                if next.login_expires_at > next.auth_expires_at {
                    return false;
                }
                allowed.claim_request_id = next.claim_request_id;
                allowed.login_expires_at = next.login_expires_at;
            }
            ("refreshing", "active")
                if self.credential_generation != next.credential_generation =>
            {
                allowed.advance_credentials(next);
            }
            ("refreshing", "active") => {}
            ("ready" | "active" | "refreshing", "expired" | "revoked" | "reauth_required") => {
                allowed.clear_login();
                allowed.reason_code = next.reason_code.clone();
                if next.auth_expires_at > self.auth_expires_at {
                    return false;
                }
                allowed.auth_expires_at = next.auth_expires_at;
            }
            ("active", "active") | ("refreshing", "refreshing") if next.login_cleared() => {
                allowed.clear_login()
            }
            _ if self == next => {}
            _ => return false,
        }
        allowed == *next
    }
    fn advance_credentials(&mut self, next: &Self) {
        self.clear_login();
        self.broker_hash = next.broker_hash;
        self.refresh_token_hash = next.refresh_token_hash;
        self.credential_generation = next.credential_generation;
        self.hmac_key_version = next.hmac_key_version;
        self.scope_json = next.scope_json.clone();
        self.auth_expires_at = next.auth_expires_at;
    }
    fn establish_authorization(&mut self, next: &Self) {
        self.encrypted_oauth_context = None;
        self.encrypted_result = next.encrypted_result.clone();
        self.user_id = next.user_id.clone();
        self.tenant_id = next.tenant_id.clone();
        self.scope_json = next.scope_json.clone();
        self.broker_hash = next.broker_hash;
        self.refresh_token_hash = next.refresh_token_hash;
        self.credential_generation = Some(1);
        self.auth_expires_at = next.auth_expires_at;
    }
    fn valid_generation_change(&self, next: &Self) -> bool {
        match (self.credential_generation, next.credential_generation) {
            (None, None) => self.hmac_key_version == next.hmac_key_version,
            (None, Some(1)) => {
                self.status == "exchanging"
                    && next.status == "ready"
                    && self.hmac_key_version == next.hmac_key_version
            }
            (Some(before), Some(after)) if before == after => {
                self.hmac_key_version == next.hmac_key_version
                    && self.broker_hash == next.broker_hash
                    && self.refresh_token_hash == next.refresh_token_hash
            }
            (Some(before), Some(after)) => {
                self.status == "refreshing"
                    && next.status == "active"
                    && before.checked_add(1) == Some(after)
                    && next.login_cleared()
            }
            _ => false,
        }
    }
    fn stable_authorization(&self, next: &Self) -> bool {
        if self.credential_generation.is_none() {
            return true;
        }
        self.user_id == next.user_id
            && self.tenant_id == next.tenant_id
            && self
                .scope_json
                .as_ref()
                .zip(next.scope_json.as_ref())
                .is_some_and(|(old, new)| new.iter().all(|scope| old.contains(scope)))
    }
    fn stable_login(&self, next: &Self) -> bool {
        if next.login_cleared() {
            return true;
        }
        self.request_hash == next.request_hash
            && self.login_ticket_hash == next.login_ticket_hash
            && self.claim_challenge == next.claim_challenge
            && self.oauth_state_hash == next.oauth_state_hash
            && (self.claim_request_id.is_none() || self.claim_request_id == next.claim_request_id)
    }
}

/// Exact nine-column R row. The request HMAC version is immutable.
#[derive(Clone, PartialEq, Eq)]
pub struct RefreshRequest {
    pub auth_id: Uuid,
    pub request_id: Uuid,
    pub request_hash: [u8; 32],
    pub hmac_key_version: u16,
    pub input_generation: u64,
    pub status: String,
    pub expires_at: NaiveDateTime,
    pub encrypted_response: Option<Vec<u8>>,
    pub error_code: Option<String>,
}
impl RefreshRequest {
    /// Validate request identity, output-generation bound and complete status shape.
    /// Invalid persisted values return Integrity.
    pub fn validate(&self) -> Result<(), StorageError> {
        // [Review Fix #21] Reject legacy refresh IDs before trusting a persisted R row.
        let is_valid = is_uuid_v7(self.auth_id)
            && is_uuid_v7(self.request_id)
            && self.hmac_key_version > 0
            && self.input_generation > 0
            && self.input_generation.checked_add(1).is_some()
            && valid_reason(self.error_code.as_deref())
            && match self.status.as_str() {
                "pending" => self.encrypted_response.is_none() && self.error_code.is_none(),
                "completed" => {
                    self.error_code.is_none()
                        && self
                            .encrypted_response
                            .as_ref()
                            .is_some_and(|value| (32..=262144).contains(&value.len()))
                }
                "unknown" | "failed" => {
                    self.encrypted_response.is_none() && self.error_code.is_some()
                }
                _ => false,
            };
        if is_valid {
            Ok(())
        } else {
            Err(StorageError::Integrity)
        }
    }
    pub(crate) fn transition(&self, next: &Self) -> Result<(), StorageError> {
        self.validate()?;
        next.validate()?;
        if self.auth_id != next.auth_id
            || self.request_id != next.request_id
            || self.request_hash != next.request_hash
            || self.hmac_key_version != next.hmac_key_version
            || self.input_generation != next.input_generation
            || self.status != "pending"
            || next.status == "pending"
        {
            return Err(StorageError::Conflict);
        }
        Ok(())
    }
}

fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.chars().count() <= maximum && !value.chars().any(char::is_control)
}
fn valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= 256
        && scope
            .bytes()
            .all(|byte| matches!(byte, 0x21 | 0x23..=0x5b | 0x5d..=0x7e))
}
fn valid_reason(reason: Option<&str>) -> bool {
    reason.is_none_or(|value| {
        matches!(
            value,
            "access_denied"
                | "admission_denied"
                | "login_expired"
                | "authorization_expired"
                | "revoked"
                | "reauth_required"
                | "login_outcome_unknown"
                | "refresh_outcome_unknown"
                | "upstream_unavailable"
                | "provider_error"
                | "invalid_grant"
                | "invalid_identity"
                | "invalid_scope"
                | "provider_contract_error"
                | "claim_recovery_expired"
                | "refresh_recovery_expired"
                | "refresh_failed"
        )
    })
}
