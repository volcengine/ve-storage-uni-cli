use super::SecretString;
use crate::login::{Identity, ProviderError};
use chrono::{Datelike, Duration, NaiveDateTime};
use serde::{
    de::{value::MapAccessDeserializer, DeserializeOwned, IgnoredAny, MapAccess, Visitor},
    Deserialize, Deserializer,
};
use std::{fmt, marker::PhantomData};
use zeroize::Zeroizing;

#[derive(Deserialize)]
struct TokenWire {
    code: Option<i64>,
    access_token: Option<SecretString>,
    refresh_token: Option<SecretString>,
    expires_in: Option<i64>,
    refresh_token_expires_in: Option<i64>,
    token_type: Option<String>,
    scope: Option<String>,
    #[serde(default, deserialize_with = "present")]
    error: bool,
    #[serde(default, deserialize_with = "present")]
    error_description: bool,
}
pub(super) struct TokenResponse {
    pub(super) access_token: SecretString,
    pub(super) refresh_token: SecretString,
    pub(super) expires_at: NaiveDateTime,
    pub(super) refresh_expires_at: NaiveDateTime,
    pub(super) scope: Vec<String>,
}
#[derive(Deserialize)]
struct UserInfoWire {
    code: Option<i64>,
    // [Review Fix #58] Recognize documented msg for strict type/duplicate checks without retaining its text.
    #[serde(rename = "msg", default, deserialize_with = "message")]
    _message: (),
    data: Option<ObjectOnly<UserInfo>>,
    #[serde(default, deserialize_with = "present")]
    error: bool,
    #[serde(default, deserialize_with = "present")]
    error_description: bool,
}
#[derive(Deserialize)]
struct UserInfo {
    open_id: Option<String>,
    tenant_key: Option<String>,
}
// [Review Fix #69] Derived structs accept positional arrays; require maps at every schema object boundary.
struct ObjectOnly<T>(T);
impl<'de, T: Deserialize<'de>> Deserialize<'de> for ObjectOnly<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
}
struct ObjectVisitor<T>(PhantomData<T>);
impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
    type Value = ObjectOnly<T>;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an OAuth response object")
    }
    fn visit_map<A: MapAccess<'de>>(self, entries: A) -> Result<Self::Value, A::Error> {
        T::deserialize(MapAccessDeserializer::new(entries)).map(ObjectOnly)
    }
}
fn object_from_slice<T: DeserializeOwned>(body: &[u8]) -> Result<T, ProviderError> {
    // [Review Fix #70] Bound ignored additive subtrees before Serde can skip their structural depth.
    crate::json::check_json_depth(body).map_err(|_| ProviderError::Contract)?;
    let ObjectOnly(parsed) = serde_json::from_slice(body).map_err(|_| ProviderError::Contract)?;
    Ok(parsed)
}
struct MessageVisitor;
impl serde::de::Visitor<'_> for MessageVisitor {
    type Value = ();
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a string")
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
}
fn message<'de, D: Deserializer<'de>>(deserializer: D) -> Result<(), D::Error> {
    deserializer.deserialize_str(MessageVisitor)
}
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    IgnoredAny::deserialize(deserializer).map(|_| true)
}
pub(super) fn parse_token(
    body: &[u8],
    required: &[String],
    sampled: NaiveDateTime,
) -> Result<TokenResponse, ProviderError> {
    let parsed: TokenWire = object_from_slice(body)?;
    validate_success(parsed.code, parsed.error || parsed.error_description)?;
    let access_token = parsed.access_token.ok_or(ProviderError::Contract)?;
    let refresh_token = parsed.refresh_token.ok_or(ProviderError::Contract)?;
    let granted = parsed.scope.ok_or(ProviderError::Contract)?;
    let scope = scopes(granted.split(' ').map(String::from).collect())?;
    if parsed.token_type.as_deref() != Some("Bearer")
        || !valid_text(access_token.as_str(), 16384)
        || !valid_text(refresh_token.as_str(), 16384)
        || !required.iter().all(|required| scope.contains(required))
    {
        return Err(ProviderError::Contract);
    }
    Ok(TokenResponse {
        access_token,
        refresh_token,
        scope,
        expires_at: deadline(sampled, parsed.expires_in)?,
        refresh_expires_at: deadline(sampled, parsed.refresh_token_expires_in)?,
    })
}
pub(super) fn parse_identity(body: &[u8]) -> Result<Identity, ProviderError> {
    let parsed: UserInfoWire = object_from_slice(body)?;
    validate_success(parsed.code, parsed.error || parsed.error_description)?;
    let user = parsed.data.ok_or(ProviderError::Contract)?.0;
    let user_id = user.open_id.ok_or(ProviderError::Contract)?;
    let tenant_id = user.tenant_key.ok_or(ProviderError::Contract)?;
    if !valid_text(&user_id, 256) || !valid_text(&tenant_id, 256) {
        return Err(ProviderError::Contract);
    }
    Ok(Identity {
        user_id,
        tenant_id: Some(tenant_id),
    })
}
fn validate_success(code: Option<i64>, has_error: bool) -> Result<(), ProviderError> {
    match (code, has_error) {
        (Some(0), false) => Ok(()),
        (Some(0), true) | (None, false) => Err(ProviderError::Contract),
        _ => Err(ProviderError::Unavailable),
    }
}
fn deadline(sampled: NaiveDateTime, seconds: Option<i64>) -> Result<NaiveDateTime, ProviderError> {
    let seconds = seconds
        .filter(|seconds| *seconds > 0)
        .ok_or(ProviderError::Contract)?;
    let duration = Duration::try_seconds(seconds).ok_or(ProviderError::Contract)?;
    let deadline = sampled
        .checked_add_signed(duration)
        .ok_or(ProviderError::Contract)?;
    if !(1..=9999).contains(&deadline.year()) {
        return Err(ProviderError::Contract);
    }
    Ok(deadline)
}
pub(super) fn scopes(mut scopes: Vec<String>) -> Result<Vec<String>, ProviderError> {
    scopes.sort();
    if scopes.is_empty()
        || scopes.len() > 100
        || !scopes.iter().all(|scope| valid_scope(scope))
        || scopes.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(ProviderError::Contract);
    }
    Ok(scopes)
}
fn valid_scope(scope: &str) -> bool {
    !scope.is_empty()
        && scope.len() <= 256
        && scope
            .bytes()
            .all(|byte| matches!(byte, 0x21 | 0x23..=0x5b | 0x5d..=0x7e))
}
pub(super) fn valid_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.chars().count() <= maximum && !value.chars().any(char::is_control)
}
pub(super) async fn read_body(
    mut response: reqwest::Response,
    maximum: usize,
) -> Result<Zeroizing<Vec<u8>>, ProviderError> {
    if !response.status().is_success() {
        return Err(ProviderError::Unavailable);
    }
    if !is_json(&response)
        || response
            .content_length()
            .is_some_and(|length| length > maximum as u64)
    {
        return Err(ProviderError::Contract);
    }
    // [Review Fix #56] Reserve the bounded capacity once so growth cannot leave old plaintext allocations.
    let mut body = Zeroizing::new(Vec::with_capacity(maximum));
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProviderError::Unavailable)?
    {
        if chunk.len() > maximum.saturating_sub(body.len()) {
            return Err(ProviderError::Contract);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
fn is_json(response: &reqwest::Response) -> bool {
    let mut values = response
        .headers()
        .get_all(reqwest::header::CONTENT_TYPE)
        .iter();
    let Some(value) = values.next().and_then(|value| value.to_str().ok()) else {
        return false;
    };
    values.next().is_none()
        && value
            .split(';')
            .next()
            .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}
