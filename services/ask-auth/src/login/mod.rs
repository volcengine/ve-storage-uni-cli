//! Explicitly injected login lifecycle; the standalone process never enables it.
mod adapters;
mod callback;
mod claim;
mod config;
mod create;
mod errors;
mod http;
mod lifecycle;
mod models;
mod refresh;
mod refresh_outcome;
use crate::{crypto::Crypto, storage::Store};
pub use adapters::*;
pub use config::{AdmissionSnapshot, DeploymentIdentity, EnabledConfig, OAuthEndpoints};
pub use errors::LoginError;
use std::sync::Arc;

/// Injected service; construction never enables the standalone process.
pub struct LoginService {
    store: Arc<Store>,
    crypto: Arc<Crypto>,
    config: EnabledConfig,
    adapters: LoginAdapters,
}
impl LoginService {
    pub(crate) fn runtime_store(&self) -> Arc<Store> {
        self.store.clone()
    }
    pub(crate) fn runtime_crypto(&self) -> &Crypto {
        &self.crypto
    }
    /// Bind validated configuration, writer Store, crypto ring and trusted adapters.
    /// Returns InvalidConfiguration for mismatching app/deployment/key capabilities; no I/O.
    pub fn new(
        store: Arc<Store>,
        crypto: Arc<Crypto>,
        config: EnabledConfig,
        adapters: LoginAdapters,
    ) -> Result<Self, LoginError> {
        if !store.matches_crypto(&crypto)
            || crypto.app() != config.identity.app_id
            || crypto.deployment() != config.identity.deployment
            || !crypto.has_version(config.key_version)
        {
            return Err(LoginError::InvalidConfiguration);
        }
        Ok(Self {
            store,
            crypto,
            config,
            adapters,
        })
    }
    async fn policy(&self) -> Result<AdmissionSnapshot, LoginError> {
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            self.adapters.admission.snapshot(),
        )
        .await
        .map_err(|_| LoginError::Unavailable)?
        .map_err(|_| LoginError::Unavailable)
    }
    async fn require_admission(
        &self,
        row: &crate::storage::Session,
    ) -> Result<chrono::NaiveDateTime, LoginError> {
        let policy = self.policy().await?;
        let (_, now) = self.store.login_snapshot(row.auth_id).await?;
        if !policy.allows(
            row.user_id.as_deref().ok_or(LoginError::Unavailable)?,
            row.tenant_id.as_deref(),
            now,
        ) {
            return Err(LoginError::AccessDenied);
        }
        if !row.scope_json.as_ref().is_some_and(|actual| {
            self.config
                .required_scope
                .iter()
                .all(|scope| actual.contains(scope))
        }) {
            return Err(LoginError::ScopeMissing);
        }
        Ok(policy.fetched_at + chrono::Duration::minutes(5))
    }
}
