use super::{http::parse_id, LoginError, LoginService};
use crate::{
    storage::{Mutation, Session, StorageError},
    validation::ClaimRequest,
};
use chrono::{Duration, NaiveDateTime};
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

const MAX_CLAIM_ATTEMPTS: usize = 3;

impl LoginService {
    pub(super) async fn claim(
        &self,
        request: ClaimRequest,
        ticket: &str,
        request_id: Uuid,
    ) -> Result<Value, LoginError> {
        let auth_id = parse_id(&request.auth_id)?;
        for _ in 0..MAX_CLAIM_ATTEMPTS {
            let (found, now) = self.store.login_snapshot(auth_id).await?;
            let row = found.ok_or(LoginError::AuthenticationRequired)?;
            self.authenticate_claim(&row, ticket, &request.claim_verifier)?;
            self.claim_phase(&row, request_id, now)?;
            let bundle = self.bundle(&row)?;
            let credential_deadline = bundle.deadline()?;
            if credential_deadline <= now {
                return Err(expired_claim(&row));
            }
            let policy_deadline = self.require_admission(&row).await?;
            let active = if row.status == "ready" {
                let next = claimed(&row, request_id, now, credential_deadline)?;
                match self
                    .store
                    .apply(Mutation::Claim {
                        expected: row,
                        next: next.clone(),
                    })
                    .await
                {
                    Ok(()) => next,
                    Err(StorageError::Conflict | StorageError::NotFound) => continue,
                    Err(error) => return Err(error.into()),
                }
            } else {
                row
            };
            match self
                .store
                .confirm_login(&active, credential_deadline.min(policy_deadline))
                .await
            {
                Ok(()) => {
                    return serde_json::to_value(&bundle).map_err(|_| LoginError::Unavailable)
                }
                Err(StorageError::Conflict | StorageError::NotFound) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(LoginError::Unavailable)
    }
    fn authenticate_claim(
        &self,
        row: &Session,
        ticket: &str,
        verifier: &str,
    ) -> Result<(), LoginError> {
        let is_ticket_valid = self.authenticate_ticket(row, ticket).is_ok();
        let proof: [u8; 32] = Sha256::digest(verifier.as_bytes()).into();
        let expected = row.claim_challenge.unwrap_or([0; 32]);
        let is_proof_valid = bool::from(proof.ct_eq(&expected)) && row.claim_challenge.is_some();
        (is_ticket_valid && is_proof_valid)
            .then_some(())
            .ok_or(LoginError::AuthenticationRequired)
    }
    fn claim_phase(
        &self,
        row: &Session,
        request_id: Uuid,
        now: NaiveDateTime,
    ) -> Result<(), LoginError> {
        match row.status.as_str() {
            // [Review Fix #27] An authenticated elapsed pending lease is expired before pending-state errors.
            "pending" | "exchanging" if row.login_expires_at.is_none_or(|expiry| expiry <= now) => {
                Err(LoginError::LoginExpired)
            }
            "pending" | "exchanging" => Err(LoginError::LoginPending),
            "denied" => Err(LoginError::LoginDenied),
            "expired" => Err(LoginError::LoginExpired),
            "error" => Err(match row.reason_code.as_deref() {
                Some("login_outcome_unknown") => LoginError::LoginOutcomeUnknown,
                Some("provider_contract_error") => LoginError::ProviderContract,
                _ => LoginError::Unavailable,
            }),
            "ready"
                if row.login_expires_at.is_none_or(|expiry| expiry <= now)
                    || row.auth_expires_at.is_none_or(|expiry| expiry <= now) =>
            {
                Err(LoginError::LoginExpired)
            }
            "ready" => Ok(()),
            "active" | "refreshing"
                if row.login_expires_at.is_none_or(|expiry| expiry <= now)
                    || row.auth_expires_at.is_none_or(|expiry| expiry <= now) =>
            {
                Err(LoginError::ClaimRecoveryExpired)
            }
            "refreshing" => Err(LoginError::RefreshInProgress),
            "active" if row.claim_request_id != Some(request_id) => {
                Err(LoginError::IdempotencyConflict)
            }
            "active" => Ok(()),
            _ => Err(LoginError::AuthenticationRequired),
        }
    }
}
fn expired_claim(row: &Session) -> LoginError {
    if row.status == "ready" {
        LoginError::LoginExpired
    } else {
        LoginError::ClaimRecoveryExpired
    }
}
fn claimed(
    row: &Session,
    request_id: Uuid,
    now: NaiveDateTime,
    credential_deadline: NaiveDateTime,
) -> Result<Session, LoginError> {
    let mut next = row.clone();
    next.status = "active".into();
    next.claim_request_id = Some(request_id);
    next.login_expires_at = Some((now + Duration::minutes(5)).min(credential_deadline));
    Ok(next)
}
