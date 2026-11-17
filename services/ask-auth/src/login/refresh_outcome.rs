use super::{
    models::{self, TokenBundle},
    refresh::RefreshOperation,
    LoginError, LoginService, NoConsumptionReason, ProviderTokens, RefreshFailure,
};
use crate::{
    crypto::{Binding, Purpose},
    storage::{RefreshRequest as Ledger, Session},
};
use chrono::{Duration, NaiveDateTime};
use serde_json::Value;
use zeroize::Zeroizing;

impl LoginService {
    pub(super) fn refresh_cache(
        &self,
        row: &Session,
        request: &Ledger,
    ) -> Result<TokenBundle, LoginError> {
        let binding = Binding::refresh(
            request.auth_id,
            request.request_id,
            request.input_generation,
        )?;
        let plaintext = self.crypto.decrypt(
            &binding,
            request
                .encrypted_response
                .as_ref()
                .ok_or(LoginError::Unavailable)?,
        )?;
        let bundle: TokenBundle =
            serde_json::from_slice(&plaintext).map_err(|_| LoginError::Unavailable)?;
        self.validate_bundle(row, &bundle)?;
        if Some(bundle.credential_generation) != request.input_generation.checked_add(1)
            || request.expires_at > bundle.deadline()?
        {
            return Err(LoginError::Unavailable);
        }
        Ok(bundle)
    }
    pub(super) async fn finish_refresh_tokens(
        &self,
        operation: RefreshOperation,
        tokens: ProviderTokens,
    ) -> Result<Value, LoginError> {
        let policy = match self.policy().await {
            Ok(policy) => policy,
            Err(_) => {
                return self
                    .finish_refresh_failure(operation, RefreshFailure::OutcomeUnknown)
                    .await
            }
        };
        let (_, _, now) = self
            .store
            .refresh_snapshot(operation.session.auth_id, operation.request.request_id)
            .await?;
        let bundle = match self.refreshed_bundle(&operation.session, &tokens, now) {
            Ok(bundle) => bundle,
            Err(_) => {
                return self
                    .finish_refresh_failure(operation, RefreshFailure::OutcomeUnknown)
                    .await
            }
        };
        if !policy.allows(
            &bundle.identity.user_id,
            bundle.identity.tenant_id.as_deref(),
            now,
        ) {
            return self
                .finish_refresh_failure(operation, RefreshFailure::OutcomeUnknown)
                .await;
        }
        let (next, next_request) = self.completed_refresh(&operation, &bundle, now)?;
        self.store
            .finish_active_refresh(
                (operation.session, next.clone()),
                (operation.request, next_request.clone()),
                operation.fence,
            )
            .await?;
        // A fresh policy and an exact A->R locked check protect release after the publication transaction.
        let (_, _, release_now) = self
            .store
            .refresh_snapshot(next.auth_id, next_request.request_id)
            .await?;
        self.recover_refresh(&next, &next_request, release_now)
            .await
    }
    fn refreshed_bundle(
        &self,
        row: &Session,
        tokens: &ProviderTokens,
        now: NaiveDateTime,
    ) -> Result<TokenBundle, LoginError> {
        let mut bundle = self.new_bundle(row, tokens, now)?;
        let is_identity_valid = Some(&tokens.identity.user_id) == row.user_id.as_ref()
            && tokens.identity.tenant_id == row.tenant_id;
        let has_no_expansion = row
            .scope_json
            .as_ref()
            .is_some_and(|actual| tokens.scope.iter().all(|scope| actual.contains(scope)));
        if !is_identity_valid || !has_no_expansion || !self.has_required_scope(&tokens.scope) {
            return Err(LoginError::ProviderContract);
        }
        if tokens.refresh_expires_at.is_none() {
            let prior = row.auth_expires_at.ok_or(LoginError::Unavailable)?;
            bundle.broker_expires_at =
                models::timestamp(prior.min(models::parse_time(&bundle.broker_expires_at)?));
        }
        bundle.credential_generation = row
            .credential_generation
            .and_then(|generation| generation.checked_add(1))
            .ok_or(LoginError::GenerationMismatch)?;
        Ok(bundle)
    }
    fn completed_refresh(
        &self,
        operation: &RefreshOperation,
        bundle: &TokenBundle,
        now: NaiveDateTime,
    ) -> Result<(Session, Ledger), LoginError> {
        let mut next = operation.session.clone();
        next.clear_login();
        next.status = "active".into();
        next.hmac_key_version = self.config.key_version;
        next.scope_json = Some(bundle.scope.clone());
        next.credential_generation = Some(bundle.credential_generation);
        next.auth_expires_at = Some(models::parse_time(&bundle.broker_expires_at)?);
        next.broker_hash = Some(*self.crypto.digest(
            next.hmac_key_version,
            Purpose::Broker,
            next.auth_id,
            &[bundle.broker_token.as_bytes()],
        )?);
        next.refresh_token_hash = Some(*self.crypto.digest(
            next.hmac_key_version,
            Purpose::RefreshToken,
            next.auth_id,
            &[bundle.refresh_token.as_bytes()],
        )?);
        let mut request = operation.request.clone();
        request.status = "completed".into();
        request.expires_at = (now + Duration::minutes(5)).min(bundle.deadline()?);
        let binding = Binding::refresh(
            request.auth_id,
            request.request_id,
            request.input_generation,
        )?;
        let plaintext =
            Zeroizing::new(serde_json::to_vec(bundle).map_err(|_| LoginError::Unavailable)?);
        request.encrypted_response = Some(
            self.crypto
                .encrypt(self.config.key_version, &binding, &plaintext)?
                .to_vec(),
        );
        self.validate_bundle(&next, bundle)?;
        Ok((next, request))
    }
    pub(super) async fn finish_refresh_failure(
        &self,
        operation: RefreshOperation,
        failure: RefreshFailure,
    ) -> Result<Value, LoginError> {
        let (_, _, now) = self
            .store
            .refresh_snapshot(operation.session.auth_id, operation.request.request_id)
            .await?;
        // [Review Fix #34] Only the sweeper may settle after the original execution lease; active workers make no late writes.
        if operation.request.expires_at <= now {
            return Err(LoginError::Unavailable);
        }
        let mut next = operation.session.clone();
        let mut request = operation.request.clone();
        let error = match failure {
            RefreshFailure::OutcomeUnknown => {
                request.status = "unknown".into();
                request.error_code = Some("refresh_outcome_unknown".into());
                request.expires_at = now;
                end_refresh(&mut next, "reauth_required", "refresh_outcome_unknown", now);
                LoginError::RefreshOutcomeUnknown
            }
            RefreshFailure::NoConsumption(reason) => {
                request.status = "failed".into();
                request.expires_at = now + Duration::minutes(5);
                request.error_code = Some(failure_code(reason).into());
                next.status = "active".into();
                if matches!(reason, NoConsumptionReason::CredentialInvalid) {
                    end_refresh(&mut next, "reauth_required", "invalid_grant", now);
                } else if next.auth_expires_at.is_none_or(|expiry| expiry <= now) {
                    end_refresh(&mut next, "expired", "authorization_expired", now);
                }
                retained_failure(&request)
            }
        };
        self.store
            .finish_active_refresh(
                (operation.session, next),
                (operation.request, request),
                operation.fence,
            )
            .await?;
        Err(error)
    }
}
fn end_refresh(row: &mut Session, status: &str, reason: &str, now: NaiveDateTime) {
    row.status = status.into();
    row.reason_code = Some(reason.into());
    row.clear_login();
    row.auth_expires_at = row.auth_expires_at.map(|expiry| expiry.min(now));
}
fn failure_code(reason: NoConsumptionReason) -> &'static str {
    match reason {
        NoConsumptionReason::Unavailable => "upstream_unavailable",
        NoConsumptionReason::Contract => "provider_contract_error",
        NoConsumptionReason::CredentialInvalid => "invalid_grant",
    }
}
pub(super) fn retained_failure(request: &Ledger) -> LoginError {
    match request.error_code.as_deref() {
        Some("upstream_unavailable") => LoginError::Unavailable,
        Some("provider_contract_error") => LoginError::RefreshProviderContract,
        Some("invalid_grant") => LoginError::AuthenticationRequired,
        _ => LoginError::Unavailable,
    }
}
