use super::{
    models::{self, TokenBundle},
    ExchangeRequest, LoginError, LoginService, ProviderError,
};
use crate::{
    crypto::Purpose,
    storage::{CallbackFence, Mutation, Session},
    validation,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::time::Duration;

impl LoginService {
    pub(super) async fn callback(&self, query: Option<&str>) -> Result<(), LoginError> {
        validation::validate_callback(query)?;
        let fields = validation::query_fields(query)?;
        let state = fields.get("state").ok_or(LoginError::InvalidRequest)?;
        let (exchanging, fence) = self.reserve_callback(state).await?;
        if let Some(error) = fields.get("error") {
            // [Review Fix #31] Only explicit access_denied proves refusal; other OAuth errors are sanitized provider failures.
            let (status, reason) = if error == "access_denied" {
                ("denied", "access_denied")
            } else {
                ("error", "provider_error")
            };
            return self.finish_error(exchanging, fence, status, reason).await;
        }
        let code = fields
            .get("code")
            .ok_or(LoginError::InvalidRequest)?
            .clone();
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            self.adapters.provider.exchange(ExchangeRequest {
                code,
                callback_url: self.config.callback.to_string(),
            }),
        )
        .await;
        match outcome {
            Ok(Ok(tokens)) => self.finish_tokens(exchanging, fence, tokens).await,
            Ok(Err(error)) => {
                let (status, reason) = provider_failure(error);
                self.finish_error(exchanging, fence, status, reason).await
            }
            Err(_) => {
                self.finish_error(exchanging, fence, "error", "login_outcome_unknown")
                    .await
            }
        }
    }
    async fn reserve_callback(&self, state: &str) -> Result<(Session, CallbackFence), LoginError> {
        let decoded = URL_SAFE_NO_PAD
            .decode(state)
            .map_err(|_| LoginError::InvalidRequest)?;
        let auth_id =
            uuid::Uuid::from_slice(&decoded[..16]).map_err(|_| LoginError::InvalidRequest)?;
        let (found, now) = self.store.login_snapshot(auth_id).await?;
        let original = found.ok_or(LoginError::InvalidRequest)?;
        if !self.verify(
            &original,
            Purpose::OAuthState,
            state.as_bytes(),
            original.oauth_state_hash.as_ref(),
        )? {
            return Err(LoginError::InvalidRequest);
        }
        if original
            .login_expires_at
            .is_none_or(|deadline| deadline <= now)
        {
            return Err(LoginError::LoginExpired);
        }
        if original.status != "pending" {
            return Err(LoginError::InvalidRequest);
        }
        let context = self.context(&original)?;
        if context.state != state {
            return Err(LoginError::InvalidRequest);
        }
        let fence = CallbackFence::capture(&original)?;
        let mut exchanging = original.clone();
        exchanging.status = "exchanging".into();
        self.store
            .apply(Mutation::Callback {
                expected: original,
                next: exchanging.clone(),
                fence: fence.clone(),
            })
            .await?;
        Ok((exchanging, fence))
    }
    async fn finish_error(
        &self,
        expected: Session,
        fence: CallbackFence,
        status: &str,
        reason: &str,
    ) -> Result<(), LoginError> {
        let mut next = expected.clone();
        next.status = status.into();
        next.reason_code = Some(reason.into());
        next.encrypted_oauth_context = None;
        self.store
            .apply(Mutation::Callback {
                expected,
                next,
                fence,
            })
            .await?;
        Ok(())
    }
    async fn finish_tokens(
        &self,
        expected: Session,
        fence: CallbackFence,
        tokens: super::ProviderTokens,
    ) -> Result<(), LoginError> {
        let policy = match self.policy().await {
            Ok(policy) => policy,
            Err(_) => {
                return self
                    .finish_error(expected, fence, "error", "upstream_unavailable")
                    .await
            }
        };
        let (_, now) = self.store.login_snapshot(expected.auth_id).await?;
        let bundle = match self.new_bundle(&expected, &tokens, now) {
            Ok(bundle) => bundle,
            Err(_) => {
                return self
                    .finish_error(expected, fence, "error", "provider_contract_error")
                    .await
            }
        };
        if !self
            .config
            .required_scope
            .iter()
            .all(|scope| bundle.scope.contains(scope))
        {
            return self
                .finish_error(expected, fence, "denied", "invalid_scope")
                .await;
        }
        if !policy.allows(
            &bundle.identity.user_id,
            bundle.identity.tenant_id.as_deref(),
            now,
        ) {
            return self
                .finish_error(expected, fence, "denied", "admission_denied")
                .await;
        }
        let next = self.ready(&expected, &bundle)?;
        self.store
            .apply(Mutation::Callback {
                expected,
                next,
                fence,
            })
            .await?;
        Ok(())
    }
    fn ready(&self, expected: &Session, bundle: &TokenBundle) -> Result<Session, LoginError> {
        let mut next = expected.clone();
        next.status = "ready".into();
        next.encrypted_oauth_context = None;
        next.encrypted_result = Some(self.encrypted(expected, bundle, false)?);
        next.user_id = Some(bundle.identity.user_id.clone());
        next.tenant_id = bundle.identity.tenant_id.clone();
        next.scope_json = Some(bundle.scope.clone());
        next.broker_hash = Some(*self.crypto.digest(
            expected.hmac_key_version,
            Purpose::Broker,
            expected.auth_id,
            &[bundle.broker_token.as_bytes()],
        )?);
        next.refresh_token_hash = Some(*self.crypto.digest(
            expected.hmac_key_version,
            Purpose::RefreshToken,
            expected.auth_id,
            &[bundle.refresh_token.as_bytes()],
        )?);
        next.credential_generation = Some(1);
        next.auth_expires_at = Some(models::parse_time(&bundle.broker_expires_at)?);
        Ok(next)
    }
}
fn provider_failure(error: ProviderError) -> (&'static str, &'static str) {
    match error {
        ProviderError::AccessDenied => ("denied", "access_denied"),
        ProviderError::Unavailable => ("error", "upstream_unavailable"),
        ProviderError::OutcomeUnknown => ("error", "login_outcome_unknown"),
        ProviderError::Contract => ("error", "provider_contract_error"),
    }
}
