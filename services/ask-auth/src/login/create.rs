use super::{
    http::parse_id,
    models::{self, OAuthContext},
    AuthorizationRequest, LoginError, LoginService,
};
use crate::{
    crypto::Purpose,
    storage::{Session, StorageError},
    validation::LoginCreateRequest,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::NaiveDateTime;
use serde_json::{json, Value};
use uuid::Uuid;
use zeroize::Zeroizing;

impl LoginService {
    pub(super) async fn create(&self, request: LoginCreateRequest) -> Result<Value, LoginError> {
        let auth_id = parse_id(&request.auth_id)?;
        let (found, _) = self.store.login_snapshot(auth_id).await?;
        let row = if let Some(found) = found {
            found
        } else {
            let pending = self.pending(&request)?;
            match self.store.create_pending(pending).await {
                Ok(created) => created,
                Err(StorageError::Conflict) => self
                    .store
                    .login_snapshot(auth_id)
                    .await?
                    .0
                    .ok_or(LoginError::Unavailable)?,
                Err(error) => return Err(error.into()),
            }
        };
        self.verify_create(&row, &request)?;
        // [Review Fix #23] Re-read payload and DB clock together after concurrent callback/claim changes.
        let (current, now) = self.store.login_snapshot(auth_id).await?;
        let row = current.ok_or(LoginError::IdempotencyConflict)?;
        self.verify_create(&row, &request)?;
        if row.login_expires_at.is_none_or(|deadline| deadline <= now) {
            return Err(LoginError::LoginExpired);
        }
        let mut result = self.projection(&row, now)?;
        let authorization_url = if row.status == "pending"
            && row.login_expires_at.is_some_and(|deadline| deadline > now)
        {
            Some(self.context(&row)?.authorization_url.clone())
        } else {
            None
        };
        result["authorization_url"] = json!(authorization_url);
        result
            .as_object_mut()
            .ok_or(LoginError::Unavailable)?
            .remove("reason_code");
        Ok(result)
    }
    fn new_context(&self, auth_id: Uuid) -> Result<OAuthContext, LoginError> {
        let key_version = self.config.key_version;
        let state = models::state(auth_id)?;
        let authorization_url = self
            .adapters
            .provider
            .authorization_url(AuthorizationRequest {
                state: &state,
                callback_url: self.config.callback.as_str(),
                required_scope: &self.config.required_scope,
            })
            .map_err(|_| LoginError::ProviderContract)?;
        self.config
            .validate_authorization(&authorization_url, &state)?;
        Ok(OAuthContext {
            authorization_url,
            state,
            callback_url: self.config.callback.to_string(),
            app_id: self.config.identity.app_id.clone(),
            environment: self.config.identity.environment.clone(),
            deployment: self.config.identity.deployment.clone(),
            key_version,
        })
    }

    fn pending(&self, request: &LoginCreateRequest) -> Result<Session, LoginError> {
        let auth_id = parse_id(&request.auth_id)?;
        let key_version = self.config.key_version;
        let context = self.new_context(auth_id)?;
        let canonical =
            Zeroizing::new(serde_json::to_vec(request).map_err(|_| LoginError::InvalidRequest)?);
        let challenge: [u8; 32] = URL_SAFE_NO_PAD
            .decode(&request.claim_challenge)
            .map_err(|_| LoginError::InvalidRequest)?
            .try_into()
            .map_err(|_| LoginError::InvalidRequest)?;
        let mut row = Session {
            auth_id,
            app_id: self.config.identity.app_id.clone(),
            status: "pending".into(),
            request_hash: Some(self.proof(auth_id, Purpose::LoginRequest, &canonical)?),
            login_ticket_hash: Some(self.proof(
                auth_id,
                Purpose::LoginTicket,
                request.login_ticket.as_bytes(),
            )?),
            claim_challenge: Some(challenge),
            oauth_state_hash: Some(self.proof(
                auth_id,
                Purpose::OAuthState,
                context.state.as_bytes(),
            )?),
            hmac_key_version: key_version,
            encrypted_oauth_context: None,
            claim_request_id: None,
            encrypted_result: None,
            login_expires_at: None,
            user_id: None,
            tenant_id: None,
            scope_json: None,
            broker_hash: None,
            refresh_token_hash: None,
            credential_generation: None,
            auth_expires_at: None,
            reason_code: None,
        };
        row.encrypted_oauth_context = Some(self.encrypted(&row, &context, true)?);
        Ok(row)
    }
    fn proof(&self, auth_id: Uuid, purpose: Purpose, value: &[u8]) -> Result<[u8; 32], LoginError> {
        Ok(*self
            .crypto
            .digest(self.config.key_version, purpose, auth_id, &[value])?)
    }
    fn verify_create(&self, row: &Session, request: &LoginCreateRequest) -> Result<(), LoginError> {
        let canonical =
            Zeroizing::new(serde_json::to_vec(request).map_err(|_| LoginError::InvalidRequest)?);
        let is_request_valid = self.verify(
            row,
            Purpose::LoginRequest,
            &canonical,
            row.request_hash.as_ref(),
        )?;
        let is_ticket_valid = self.verify(
            row,
            Purpose::LoginTicket,
            request.login_ticket.as_bytes(),
            row.login_ticket_hash.as_ref(),
        )?;
        (is_request_valid && is_ticket_valid)
            .then_some(())
            .ok_or(LoginError::IdempotencyConflict)
    }
    pub(super) async fn poll(&self, auth_id: Uuid, ticket: &str) -> Result<Value, LoginError> {
        let (found, now) = self.store.login_snapshot(auth_id).await?;
        let row = found.ok_or(LoginError::AuthenticationRequired)?;
        self.authenticate_ticket(&row, ticket)?;
        self.projection(&row, now)
    }
    pub(super) fn authenticate_ticket(
        &self,
        row: &Session,
        ticket: &str,
    ) -> Result<(), LoginError> {
        self.verify(
            row,
            Purpose::LoginTicket,
            ticket.as_bytes(),
            row.login_ticket_hash.as_ref(),
        )?
        .then_some(())
        .ok_or(LoginError::AuthenticationRequired)
    }
    fn projection(&self, row: &Session, now: NaiveDateTime) -> Result<Value, LoginError> {
        // [Review Fix #30] Only authenticated callers can observe context/cache integrity outcomes.
        if row.encrypted_oauth_context.is_some() {
            self.context(row)?;
        }
        let cached_bundle = row
            .encrypted_result
            .as_ref()
            .map(|_| self.bundle(row))
            .transpose()?;
        let deadline = row
            .login_expires_at
            .ok_or(LoginError::AuthenticationRequired)?;
        let (state, reason) = match row.status.as_str() {
            "active" | "refreshing" if deadline <= now => {
                ("expired", Some("claim_recovery_expired"))
            }
            "active" | "refreshing" => {
                let bundle = cached_bundle.as_ref().ok_or(LoginError::Unavailable)?;
                if bundle.deadline()? <= now
                    || row.auth_expires_at.is_none_or(|expiry| expiry <= now)
                {
                    ("expired", Some("claim_recovery_expired"))
                } else {
                    ("claimed", None)
                }
            }
            "pending" | "exchanging" | "ready" if deadline <= now => {
                ("expired", Some("login_expired"))
            }
            "pending" | "exchanging" => ("pending", None),
            "ready" => ("ready", None),
            "denied" | "error" | "expired" => (row.status.as_str(), row.reason_code.as_deref()),
            _ => return Err(LoginError::AuthenticationRequired),
        };
        Ok(
            json!({"auth_id":row.auth_id.to_string(),"state":state,"expires_at":models::timestamp(deadline),"poll_interval_seconds":2,"reason_code":reason}),
        )
    }
}
