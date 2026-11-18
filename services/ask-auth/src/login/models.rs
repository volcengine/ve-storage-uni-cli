use super::{
    config::{valid_scope, valid_text},
    Identity, LoginError, LoginService, ProviderTokens,
};
use crate::{
    crypto::{Binding, Purpose},
    storage::Session,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, NaiveDateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OAuthContext {
    pub authorization_url: String,
    pub state: String,
    pub callback_url: String,
    pub app_id: String,
    pub environment: String,
    pub deployment: String,
    pub key_version: u16,
}
impl Drop for OAuthContext {
    fn drop(&mut self) {
        self.state.zeroize();
        self.authorization_url.zeroize();
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TokenBundle {
    pub auth_id: String,
    pub provider: String,
    pub app_id: String,
    pub environment: String,
    pub identity: Identity,
    pub scope: Vec<String>,
    pub token_type: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
    // [Review Fix #24] Nullable fields are required by the frozen schema, including in authenticated caches.
    #[serde(deserialize_with = "required_nullable")]
    pub refresh_expires_at: Option<String>,
    pub broker_token: String,
    pub broker_expires_at: String,
    pub credential_generation: u64,
}
impl Drop for TokenBundle {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.broker_token.zeroize();
    }
}
impl LoginService {
    pub(super) fn context(&self, row: &Session) -> Result<OAuthContext, LoginError> {
        let plaintext = self.decrypt(row, true)?;
        let context: OAuthContext =
            serde_json::from_slice(&plaintext).map_err(|_| LoginError::Unavailable)?;
        if context.app_id != self.config.identity.app_id
            || context.environment != self.config.identity.environment
            || context.deployment != self.config.identity.deployment
            || context.key_version != row.hmac_key_version
            || context.callback_url != self.config.callback.as_str()
            || !self.verify(
                row,
                Purpose::OAuthState,
                context.state.as_bytes(),
                row.oauth_state_hash.as_ref(),
            )?
        {
            return Err(LoginError::Unavailable);
        }
        self.config
            .validate_authorization(&context.authorization_url, &context.state)?;
        Ok(context)
    }
    pub(super) fn bundle(&self, row: &Session) -> Result<TokenBundle, LoginError> {
        let plaintext = self.decrypt(row, false)?;
        let bundle: TokenBundle =
            serde_json::from_slice(&plaintext).map_err(|_| LoginError::Unavailable)?;
        if bundle.credential_generation != 1 || row.credential_generation != Some(1) {
            return Err(LoginError::Unavailable);
        }
        self.validate_bundle(row, &bundle)?;
        Ok(bundle)
    }
    pub(super) fn validate_bundle(
        &self,
        row: &Session,
        bundle: &TokenBundle,
    ) -> Result<(), LoginError> {
        let is_metadata_valid = bundle.auth_id == row.auth_id.to_string()
            && bundle.provider == "doubao"
            && bundle.app_id == self.config.identity.app_id
            && bundle.environment == self.config.identity.environment
            && bundle.token_type == "Bearer"
            && bundle.credential_generation > 0
            && row.credential_generation == Some(bundle.credential_generation)
            && Some(&bundle.identity.user_id) == row.user_id.as_ref()
            && bundle.identity.tenant_id == row.tenant_id
            && Some(&bundle.scope) == row.scope_json.as_ref()
            && valid_text(&bundle.access_token, 16384)
            && valid_text(&bundle.refresh_token, 16384)
            && crate::validation::is_nonce(&bundle.broker_token)
            && Some(parse_time(&bundle.broker_expires_at)?) == row.auth_expires_at;
        if !is_metadata_valid
            || !self.verify(
                row,
                Purpose::Broker,
                bundle.broker_token.as_bytes(),
                row.broker_hash.as_ref(),
            )?
            || !self.verify(
                row,
                Purpose::RefreshToken,
                bundle.refresh_token.as_bytes(),
                row.refresh_token_hash.as_ref(),
            )?
        {
            return Err(LoginError::Unavailable);
        }
        bundle.deadline()?;
        Ok(())
    }
    fn decrypt(
        &self,
        row: &Session,
        is_context: bool,
    ) -> Result<crate::crypto::SecretBytes, LoginError> {
        let envelope = if is_context {
            row.encrypted_oauth_context.as_ref()
        } else {
            row.encrypted_result.as_ref()
        }
        .ok_or(LoginError::Unavailable)?;
        // [Review Fix #25] AEAD selects its envelope key independently of the immutable proof HMAC version.
        let binding = if is_context {
            Binding::context(row.auth_id)
        } else {
            Binding::login(row.auth_id)
        };
        Ok(self.crypto.decrypt(&binding, envelope)?)
    }
    pub(super) fn verify(
        &self,
        row: &Session,
        purpose: Purpose,
        value: &[u8],
        expected: Option<&[u8; 32]>,
    ) -> Result<bool, LoginError> {
        let Some(expected) = expected else {
            return Ok(false);
        };
        Ok(self.crypto.verify(
            row.hmac_key_version,
            purpose,
            row.auth_id,
            &[value],
            expected,
        )?)
    }
    pub(super) fn encrypted<T: Serialize>(
        &self,
        row: &Session,
        value: &T,
        is_context: bool,
    ) -> Result<Vec<u8>, LoginError> {
        let plaintext =
            Zeroizing::new(serde_json::to_vec(value).map_err(|_| LoginError::Unavailable)?);
        let binding = if is_context {
            Binding::context(row.auth_id)
        } else {
            Binding::login(row.auth_id)
        };
        Ok(self
            .crypto
            .encrypt(row.hmac_key_version, &binding, &plaintext)?
            .to_vec())
    }
    pub(super) fn new_bundle(
        &self,
        row: &Session,
        tokens: &ProviderTokens,
        now: NaiveDateTime,
    ) -> Result<TokenBundle, LoginError> {
        validate_provider(tokens, now)?;
        let auth_deadline = tokens
            .refresh_expires_at
            .map_or(now + Duration::days(30), |deadline| {
                deadline.min(now + Duration::days(30))
            });
        Ok(TokenBundle {
            auth_id: row.auth_id.to_string(),
            provider: "doubao".into(),
            app_id: self.config.identity.app_id.clone(),
            environment: self.config.identity.environment.clone(),
            identity: tokens.identity.clone(),
            scope: tokens.scope.clone(),
            token_type: "Bearer".into(),
            access_token: tokens.access_token.clone(),
            refresh_token: tokens.refresh_token.clone(),
            expires_at: timestamp(tokens.expires_at),
            refresh_expires_at: tokens.refresh_expires_at.map(timestamp),
            broker_token: random_nonce()?,
            broker_expires_at: timestamp(auth_deadline),
            credential_generation: 1,
        })
    }
}
impl TokenBundle {
    pub(super) fn deadline(&self) -> Result<NaiveDateTime, LoginError> {
        let mut deadline = parse_time(&self.expires_at)?.min(parse_time(&self.broker_expires_at)?);
        if let Some(expiry) = &self.refresh_expires_at {
            deadline = deadline.min(parse_time(expiry)?);
        }
        Ok(deadline)
    }
}
fn validate_provider(tokens: &ProviderTokens, now: NaiveDateTime) -> Result<(), LoginError> {
    let is_valid = valid_text(&tokens.identity.user_id,256)
        && tokens.identity.tenant_id.as_deref().is_none_or(|tenant| valid_text(tenant,256))
        && tokens.scope.len() <= 100 && tokens.scope.iter().all(|scope| valid_scope(scope))
        && tokens.scope.windows(2).all(|pair| pair[0] < pair[1])
        && valid_text(&tokens.access_token,16384) && valid_text(&tokens.refresh_token,16384)
        // [Review Fix #26] Reject provider times outside the wire schema or expired after millisecond truncation.
        && valid_provider_time(tokens.expires_at,now)
        && tokens.refresh_expires_at.is_none_or(|expiry| valid_provider_time(expiry,now));
    is_valid.then_some(()).ok_or(LoginError::ProviderContract)
}
pub(super) fn timestamp(value: NaiveDateTime) -> String {
    value.and_utc().to_rfc3339_opts(SecondsFormat::Millis, true)
}
pub(super) fn parse_time(value: &str) -> Result<NaiveDateTime, LoginError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| LoginError::Unavailable)?;
    // [Review Fix #29] Apply the wire timestamp range to authenticated caches as well as provider output.
    use chrono::Datelike;
    if !(1..=9999).contains(&parsed.year()) || timestamp(parsed.naive_utc()) != value {
        return Err(LoginError::Unavailable);
    }
    Ok(parsed.with_timezone(&Utc).naive_utc())
}
pub(super) fn random_nonce() -> Result<String, LoginError> {
    use aes_gcm::aead::{rand_core::RngCore, OsRng};
    let mut nonce = Zeroizing::new([0u8; 32]);
    OsRng
        .try_fill_bytes(&mut *nonce)
        .map_err(|_| LoginError::Unavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(*nonce))
}
pub(super) fn state(auth_id: Uuid) -> Result<String, LoginError> {
    let nonce = Zeroizing::new(random_nonce()?);
    let random = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(nonce.as_bytes())
            .map_err(|_| LoginError::Unavailable)?,
    );
    let mut bytes = Zeroizing::new(auth_id.as_bytes().to_vec());
    bytes.extend_from_slice(&random);
    Ok(URL_SAFE_NO_PAD.encode(&*bytes))
}

fn valid_provider_time(value: NaiveDateTime, now: NaiveDateTime) -> bool {
    use chrono::Datelike;
    (1..=9999).contains(&value.year())
        && parse_time(&timestamp(value)).is_ok_and(|expiry| expiry > now)
}
pub(super) fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_timestamp_rejects_expanded_years_and_noncanonical_offsets() {
        assert!(parse_time("+010000-01-01T00:00:00.000Z").is_err());
        assert!(parse_time("0000-01-01T00:00:00.000Z").is_err());
        assert!(parse_time("2026-01-01T00:00:00.000+00:00").is_err());
        assert!(parse_time("2026-01-01T00:00:00.000Z").is_ok());
    }
}
