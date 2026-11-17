use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{de::Error, Deserialize, Deserializer, Serialize};
use std::collections::HashMap;

use crate::error::ProtocolError;

type ValidationResult = Result<(), ProtocolError>;

/// Test the UUIDv7 version and RFC variant of an already parsed identifier.
// [Review Fix #21] Share the version guard across roles without reusing their identities.
pub(crate) fn is_uuid_v7(identifier: uuid::Uuid) -> bool {
    identifier.get_version_num() == 7 && identifier.get_variant() == uuid::Variant::RFC4122
}

fn is_canonical_uuid_v7(value: &str) -> bool {
    let Ok(identifier) = uuid::Uuid::parse_str(value) else {
        return false;
    };
    is_uuid_v7(identifier) && identifier.hyphenated().to_string() == value
}

pub(crate) fn is_request_id(value: &str) -> bool {
    is_canonical_uuid_v7(value)
}

pub(crate) fn is_nonce(value: &str) -> bool {
    canonical_bytes(value, 32).is_some()
}

fn canonical_bytes(value: &str, expected_length: usize) -> Option<Vec<u8>> {
    if value.len() != (expected_length * 8).div_ceil(6) {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(value).ok()?;
    (decoded.len() == expected_length && URL_SAFE_NO_PAD.encode(&decoded) == value)
        .then_some(decoded)
}

fn auth_id_field<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if is_auth_id(&value) {
        Ok(value)
    } else {
        Err(D::Error::custom("invalid identifier"))
    }
}

pub(crate) fn is_auth_id(value: &str) -> bool {
    is_canonical_uuid_v7(value)
}

fn nonce_field<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if is_nonce(&value) {
        Ok(value)
    } else {
        Err(D::Error::custom("invalid proof"))
    }
}

fn bounded(value: &str, maximum: usize) -> bool {
    let length = value.chars().count();
    (1..=maximum).contains(&length) && !value.chars().any(char::is_control)
}

pub(crate) trait Validate {
    fn validate(&self) -> ValidationResult;
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LoginCreateRequest {
    #[serde(deserialize_with = "auth_id_field")]
    pub(crate) auth_id: String,
    #[serde(deserialize_with = "nonce_field")]
    pub(crate) login_ticket: String,
    #[serde(deserialize_with = "nonce_field")]
    pub(crate) claim_challenge: String,
    pub(crate) device_name: String,
    pub(crate) cli_version: String,
}

impl Validate for LoginCreateRequest {
    fn validate(&self) -> ValidationResult {
        let is_valid = is_auth_id(&self.auth_id)
            && is_nonce(&self.login_ticket)
            && is_nonce(&self.claim_challenge)
            && bounded(&self.device_name, 128)
            && bounded(&self.cli_version, 64);
        is_valid.then_some(()).ok_or(ProtocolError::InvalidRequest)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaimRequest {
    #[serde(deserialize_with = "auth_id_field")]
    pub(crate) auth_id: String,
    #[serde(deserialize_with = "nonce_field")]
    pub(crate) claim_verifier: String,
}

impl Validate for ClaimRequest {
    fn validate(&self) -> ValidationResult {
        (is_auth_id(&self.auth_id) && is_nonce(&self.claim_verifier))
            .then_some(())
            .ok_or(ProtocolError::InvalidRequest)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RefreshRequest {
    #[serde(deserialize_with = "auth_id_field")]
    pub(crate) auth_id: String,
    pub(crate) refresh_token: String,
    pub(crate) credential_generation: u64,
}

impl Validate for RefreshRequest {
    fn validate(&self) -> ValidationResult {
        (is_auth_id(&self.auth_id)
            && bounded(&self.refresh_token, 16384)
            && self.credential_generation > 0)
            .then_some(())
            .ok_or(ProtocolError::InvalidRequest)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogoutRequest {
    #[serde(deserialize_with = "auth_id_field")]
    pub(crate) auth_id: String,
}

impl Validate for LogoutRequest {
    fn validate(&self) -> ValidationResult {
        is_auth_id(&self.auth_id)
            .then_some(())
            .ok_or(ProtocolError::InvalidRequest)
    }
}

pub(crate) fn query_fields(query: Option<&str>) -> Result<HashMap<String, String>, ProtocolError> {
    let query = query.unwrap_or_default();
    validate_query_encoding(query)?;
    let mut fields = HashMap::new();
    for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if fields
            .insert(name.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(ProtocolError::InvalidRequest);
        }
    }
    Ok(fields)
}

fn validate_query_encoding(query: &str) -> ValidationResult {
    // [Review Fix #2] Form decoding is lossy; reject malformed escapes and UTF-8 before interpreting OAuth input.
    let mut bytes = query.as_bytes().iter();
    while let Some(byte) = bytes.next() {
        if *byte == b'%'
            && !(bytes.next().is_some_and(u8::is_ascii_hexdigit)
                && bytes.next().is_some_and(u8::is_ascii_hexdigit))
        {
            return Err(ProtocolError::InvalidRequest);
        }
    }
    percent_encoding::percent_decode_str(query)
        .decode_utf8()
        .map(|_| ())
        .map_err(|_| ProtocolError::InvalidRequest)
}

pub(crate) fn validate_status(query: Option<&str>) -> ValidationResult {
    let fields = query_fields(query)?;
    (fields.len() == 1 && fields.get("auth_id").is_some_and(|value| is_auth_id(value)))
        .then_some(())
        .ok_or(ProtocolError::InvalidRequest)
}

pub(crate) fn validate_callback(query: Option<&str>) -> ValidationResult {
    let fields = query_fields(query)?;
    let has_code = fields.get("code").is_some_and(|value| bounded(value, 8192));
    let has_error = fields.get("error").is_some_and(|value| bounded(value, 128));
    let is_known = fields
        .keys()
        .all(|name| matches!(name.as_str(), "state" | "code" | "error"));
    let has_state = fields
        .get("state")
        .is_some_and(|value| is_oauth_state(value));
    (fields.len() == 2 && has_state && (has_code ^ has_error) && is_known)
        .then_some(())
        .ok_or(ProtocolError::InvalidRequest)
}

fn is_oauth_state(value: &str) -> bool {
    let Some(decoded) = canonical_bytes(value, 48) else {
        return false;
    };
    let Ok(identifier) = uuid::Uuid::from_slice(&decoded[..16]) else {
        return false;
    };
    value.len() == 64 && is_uuid_v7(identifier)
}

impl Drop for RefreshRequest {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.refresh_token);
    }
}
