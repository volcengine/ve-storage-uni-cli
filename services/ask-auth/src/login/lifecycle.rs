use super::{http::parse_id, models::timestamp, LoginError, LoginService};
use crate::{
    crypto::Purpose,
    storage::{Session, StorageError},
    validation::LogoutRequest,
};
use serde_json::{json, Value};
use uuid::Uuid;

const MAX_SNAPSHOT_ATTEMPTS: usize = 3;
impl LoginService {
    pub(super) fn authenticate_broker(
        &self,
        row: &Session,
        broker: &str,
    ) -> Result<(), LoginError> {
        let is_valid = self.verify(
            row,
            Purpose::Broker,
            broker.as_bytes(),
            row.broker_hash.as_ref(),
        )?;
        if !is_valid || !is_claimed(row) {
            return Err(LoginError::AuthenticationRequired);
        }
        Ok(())
    }
    pub(super) async fn status(&self, auth_id: Uuid, broker: &str) -> Result<Value, LoginError> {
        for _ in 0..MAX_SNAPSHOT_ATTEMPTS {
            let (found, now) = self.store.login_snapshot(auth_id).await?;
            let row = found.ok_or(LoginError::AuthenticationRequired)?;
            self.authenticate_broker(&row, broker)?;
            if !is_ended(&row) && row.auth_expires_at.is_none_or(|expiry| expiry <= now) {
                match self.store.terminate_authorization(&row, false).await {
                    Ok(_) | Err(StorageError::Conflict | StorageError::NotFound) => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            let policy = self.policy().await?;
            let (_, checked_now) = self.store.login_snapshot(auth_id).await?;
            let is_allowed = !is_ended(&row)
                && policy.allows(
                    row.user_id.as_deref().ok_or(LoginError::Unavailable)?,
                    row.tenant_id.as_deref(),
                    checked_now,
                )
                && self
                    .has_required_scope(row.scope_json.as_deref().ok_or(LoginError::Unavailable)?);
            // [Review Fix #39] Reject malformed/future policy times before deadline arithmetic can overflow.
            if !policy.is_current(checked_now) {
                return Err(LoginError::Unavailable);
            }
            let deadline = policy.fetched_at + chrono::Duration::minutes(5);
            match self.store.confirm_status(&row, deadline).await {
                Ok(()) => return status_projection(&row, is_allowed),
                Err(StorageError::Conflict | StorageError::NotFound) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(LoginError::Unavailable)
    }
    pub(super) async fn logout(
        &self,
        request: LogoutRequest,
        broker: &str,
    ) -> Result<Value, LoginError> {
        let auth_id = parse_id(&request.auth_id)?;
        for _ in 0..MAX_SNAPSHOT_ATTEMPTS {
            let (found, _) = self.store.login_snapshot(auth_id).await?;
            let row = found.ok_or(LoginError::AuthenticationRequired)?;
            self.authenticate_broker(&row, broker)?;
            match self.store.terminate_authorization(&row, true).await {
                Ok(_) => return Ok(json!({"auth_id":auth_id.to_string(),"status":"revoked"})),
                Err(StorageError::Conflict | StorageError::NotFound) => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(LoginError::Unavailable)
    }
    pub(super) fn has_required_scope(&self, actual: &[String]) -> bool {
        self.config
            .required_scope
            .iter()
            .all(|scope| actual.contains(scope))
    }
}
pub(super) fn is_claimed(row: &Session) -> bool {
    row.credential_generation.is_some()
        && matches!(
            row.status.as_str(),
            "active" | "refreshing" | "expired" | "revoked" | "reauth_required"
        )
}
pub(super) fn is_ended(row: &Session) -> bool {
    matches!(
        row.status.as_str(),
        "expired" | "revoked" | "reauth_required"
    )
}
fn status_projection(row: &Session, is_allowed: bool) -> Result<Value, LoginError> {
    Ok(
        json!({"auth_id":row.auth_id.to_string(),"status":row.status,"is_allowed":is_allowed,
        "identity":{"user_id":row.user_id.as_ref().ok_or(LoginError::Unavailable)?,"tenant_id":row.tenant_id},
        "scope":row.scope_json.as_ref().ok_or(LoginError::Unavailable)?,
        "broker_expires_at":timestamp(row.auth_expires_at.ok_or(LoginError::Unavailable)?),
        "credential_generation":row.credential_generation.ok_or(LoginError::Unavailable)?}),
    )
}
