use super::{
    http::parse_id,
    lifecycle::{is_claimed, is_ended},
    LoginError, LoginService, ProviderRefreshRequest, RefreshFailure,
};
use crate::{
    crypto::Purpose,
    storage::{RefreshFence, RefreshRequest as Ledger, Session, StorageError},
    validation::RefreshRequest,
};
use chrono::{Duration, NaiveDateTime};
use serde_json::Value;
use uuid::Uuid;

pub(super) struct RefreshOperation {
    pub session: Session,
    pub request: Ledger,
    pub fence: RefreshFence,
}
impl LoginService {
    pub(super) async fn refresh(
        &self,
        input: RefreshRequest,
        broker: &str,
        key: Uuid,
    ) -> Result<Value, LoginError> {
        let auth_id = parse_id(&input.auth_id)?;
        for _ in 0..3 {
            let (found, existing, now) = self.store.refresh_snapshot(auth_id, key).await?;
            let row = found.ok_or(LoginError::AuthenticationRequired)?;
            if !is_claimed(&row) {
                return Err(LoginError::AuthenticationRequired);
            }
            if let Some(request) = existing {
                self.authenticate_refresh(&input, broker, &request)?;
                return self.recover_refresh(&row, &request, now).await;
            }
            self.authenticate_current_refresh(&row, &input, broker)?;
            check_new_refresh(&row, &input, now)?;
            let policy_deadline = self.require_admission(&row).await?;
            let request = self.pending_refresh(&row, &input, broker, (key, now))?;
            match self
                .store
                .reserve_refresh(&row, &request, policy_deadline)
                .await
            {
                Ok(request) => {
                    let mut session = row;
                    session.status = "refreshing".into();
                    let fence = RefreshFence::capture(&session, &request)?;
                    return self
                        .exchange_refresh(
                            RefreshOperation {
                                session,
                                request,
                                fence,
                            },
                            &input,
                        )
                        .await;
                }
                Err(StorageError::Conflict | StorageError::NotFound) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(LoginError::Unavailable)
    }
    fn authenticate_current_refresh(
        &self,
        row: &Session,
        input: &RefreshRequest,
        broker: &str,
    ) -> Result<(), LoginError> {
        let is_broker_valid = self.verify(
            row,
            Purpose::Broker,
            broker.as_bytes(),
            row.broker_hash.as_ref(),
        )?;
        let is_refresh_valid = self.verify(
            row,
            Purpose::RefreshToken,
            input.refresh_token.as_bytes(),
            row.refresh_token_hash.as_ref(),
        )?;
        if !is_broker_valid || !is_refresh_valid {
            return Err(LoginError::AuthenticationRequired);
        }
        Ok(())
    }
    fn authenticate_refresh(
        &self,
        input: &RefreshRequest,
        broker: &str,
        request: &Ledger,
    ) -> Result<(), LoginError> {
        let generation = input.credential_generation.to_be_bytes();
        let fields = refresh_fields(input, broker, request.request_id.as_bytes(), &generation);
        if !self.crypto.verify(
            request.hmac_key_version,
            Purpose::RefreshRequest,
            request.auth_id,
            &fields,
            &request.request_hash,
        )? {
            return Err(LoginError::AuthenticationRequired);
        }
        Ok(())
    }
    fn pending_refresh(
        &self,
        row: &Session,
        input: &RefreshRequest,
        broker: &str,
        operation: (Uuid, NaiveDateTime),
    ) -> Result<Ledger, LoginError> {
        let (key, now) = operation;
        let generation = input.credential_generation.to_be_bytes();
        let fields = refresh_fields(input, broker, key.as_bytes(), &generation);
        Ok(Ledger {
            auth_id: row.auth_id,
            request_id: key,
            request_hash: *self.crypto.digest(
                self.config.key_version,
                Purpose::RefreshRequest,
                row.auth_id,
                &fields,
            )?,
            hmac_key_version: self.config.key_version,
            input_generation: input.credential_generation,
            status: "pending".into(),
            expires_at: now + Duration::seconds(45),
            encrypted_response: None,
            error_code: None,
        })
    }
    async fn exchange_refresh(
        &self,
        operation: RefreshOperation,
        input: &RefreshRequest,
    ) -> Result<Value, LoginError> {
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.adapters.provider.refresh(ProviderRefreshRequest {
                refresh_token: input.refresh_token.clone(),
            }),
        )
        .await;
        match outcome {
            Ok(Ok(tokens)) => self.finish_refresh_tokens(operation, tokens).await,
            Ok(Err(failure)) => self.finish_refresh_failure(operation, failure).await,
            Err(_) => {
                self.finish_refresh_failure(operation, RefreshFailure::OutcomeUnknown)
                    .await
            }
        }
    }
    pub(super) async fn recover_refresh(
        &self,
        row: &Session,
        request: &Ledger,
        now: NaiveDateTime,
    ) -> Result<Value, LoginError> {
        let (mut current, mut child, mut checked_now) = (row.clone(), request.clone(), now);
        for _ in 0..3 {
            let (bundle, deadline) = self
                .refresh_candidate(&current, &child, checked_now)
                .await?;
            match self.store.confirm_refresh(&current, &child, deadline).await {
                Ok(()) => {
                    return serde_json::to_value(&bundle).map_err(|_| LoginError::Unavailable)
                }
                Err(StorageError::Conflict | StorageError::NotFound) => {
                    // [Review Fix #35] Reclassify exact authenticated recovery after a concurrent refresh/revoke/cleanup.
                    (current, child, checked_now) = self.reread_refresh(request).await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(LoginError::Unavailable)
    }
    async fn reread_refresh(
        &self,
        original: &Ledger,
    ) -> Result<(Session, Ledger, NaiveDateTime), LoginError> {
        let (found, child, now) = self
            .store
            .refresh_snapshot(original.auth_id, original.request_id)
            .await?;
        let row = found.ok_or(LoginError::AuthenticationRequired)?;
        let child = child.ok_or(LoginError::RefreshRecoveryExpired)?;
        if child.request_hash != original.request_hash
            || child.hmac_key_version != original.hmac_key_version
            || child.input_generation != original.input_generation
            || !is_claimed(&row)
        {
            return Err(LoginError::AuthenticationRequired);
        }
        Ok((row, child, now))
    }
    async fn refresh_candidate(
        &self,
        row: &Session,
        request: &Ledger,
        now: NaiveDateTime,
    ) -> Result<(super::models::TokenBundle, NaiveDateTime), LoginError> {
        self.store.validate_refresh_snapshot(row, request)?;
        match request.status.as_str() {
            "unknown" => return Err(LoginError::RefreshOutcomeUnknown),
            "pending" => return Err(LoginError::RefreshInProgress),
            "failed" | "completed" if request.expires_at <= now => {
                return Err(LoginError::RefreshRecoveryExpired)
            }
            "failed" => return Err(super::refresh_outcome::retained_failure(request)),
            "completed" => {}
            _ => return Err(LoginError::Unavailable),
        }
        if request.input_generation.checked_add(1) != row.credential_generation {
            return Err(LoginError::GenerationMismatch);
        }
        if is_ended(row) || row.auth_expires_at.is_none_or(|expiry| expiry <= now) {
            return Err(LoginError::BrokerExpired);
        }
        if row.status != "active" {
            return Err(LoginError::RefreshInProgress);
        }
        let bundle = self.refresh_cache(row, request)?;
        let deadline = bundle.deadline()?;
        if deadline <= now {
            return Err(LoginError::RefreshRecoveryExpired);
        }
        let policy_deadline = self.require_admission(row).await?;
        Ok((bundle, deadline.min(policy_deadline)))
    }
}
fn refresh_fields<'a>(
    input: &'a RefreshRequest,
    broker: &'a str,
    key: &'a [u8],
    generation: &'a [u8],
) -> [&'a [u8]; 6] {
    [
        b"POST /v1/auth/refresh",
        input.auth_id.as_bytes(),
        key,
        broker.as_bytes(),
        input.refresh_token.as_bytes(),
        generation,
    ]
}
fn check_new_refresh(
    row: &Session,
    input: &RefreshRequest,
    now: NaiveDateTime,
) -> Result<(), LoginError> {
    if is_ended(row) || row.auth_expires_at.is_none_or(|expiry| expiry <= now) {
        return Err(LoginError::BrokerExpired);
    }
    if row.credential_generation != Some(input.credential_generation)
        || input.credential_generation.checked_add(1).is_none()
    {
        return Err(LoginError::GenerationMismatch);
    }
    if row.status != "active" {
        return Err(LoginError::RefreshInProgress);
    }
    Ok(())
}
