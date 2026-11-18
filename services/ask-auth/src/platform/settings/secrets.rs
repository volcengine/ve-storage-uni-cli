use super::{schema, text};
use crate::platform::{
    json::{self, ObjectOnly},
    AppError,
};
use crate::{crypto::KeyMaterial, doubao::SecretString};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Deserializer};
use std::collections::BTreeSet;
use zeroize::Zeroizing;

pub(crate) struct SecretText(pub(crate) Zeroizing<String>);
impl<'de> Deserialize<'de> for SecretText {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|text| Self(Zeroizing::new(text)))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretFields {
    schema_version: u16,
    client_secret: SecretText,
    database_url: SecretText,
    keys: Vec<ObjectOnly<KeyFields>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeyFields {
    version: u16,
    hmac_key: SecretText,
    aead_key: SecretText,
}
pub(crate) struct Secrets {
    pub(crate) client_secret: SecretString,
    pub(crate) database_url: Zeroizing<String>,
    pub(crate) keys: Vec<KeyMaterial>,
}
impl Secrets {
    pub(crate) fn parse(value: &str, current: u16) -> Result<Self, AppError> {
        let mut fields: SecretFields = json::parse(value)?;
        schema(fields.schema_version)?;
        if !text(&fields.client_secret.0, 4096)
            || !fields
                .client_secret
                .0
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte))
            || !text(&fields.database_url.0, 16384)
            || !(1..=64).contains(&fields.keys.len())
        {
            return Err(AppError::Configuration);
        }
        let keys = materials(fields.keys, current)?;
        Ok(Self {
            client_secret: SecretString::new(std::mem::take(&mut fields.client_secret.0)),
            database_url: fields.database_url.0,
            keys,
        })
    }
}
fn materials(
    fields: Vec<ObjectOnly<KeyFields>>,
    current: u16,
) -> Result<Vec<KeyMaterial>, AppError> {
    let mut versions = BTreeSet::new();
    let mut keys = Vec::new();
    let mut used = Vec::<Zeroizing<[u8; 32]>>::new();
    for ObjectOnly(field) in fields {
        let hmac = key(&field.hmac_key.0)?;
        let aead = key(&field.aead_key.0)?;
        if !versions.insert(field.version)
            || hmac == aead
            || used.iter().any(|key| **key == *hmac || **key == *aead)
        {
            return Err(AppError::Configuration);
        }
        keys.push(
            KeyMaterial::new(field.version, *hmac, *aead).map_err(|_| AppError::Configuration)?,
        );
        used.extend([hmac, aead]);
    }
    if !versions.contains(&current) {
        return Err(AppError::Configuration);
    }
    Ok(keys)
}
fn key(encoded: &str) -> Result<Zeroizing<[u8; 32]>, AppError> {
    if encoded.len() != 43 {
        return Err(AppError::Configuration);
    }
    let decoded = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| AppError::Configuration)?,
    );
    if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(&*decoded) != encoded {
        return Err(AppError::Configuration);
    }
    let mut key = Zeroizing::new([0; 32]);
    key.copy_from_slice(&decoded);
    Ok(key)
}
