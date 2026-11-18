use crate::platform::{config::ExpectedEnvironment, json, AppError};
use serde::Deserialize;
use std::path::PathBuf;
mod policy;
mod secrets;
pub(crate) use policy::{AdmissionSettings, CleanupPolicy};
pub(crate) use secrets::{SecretText, Secrets};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeFields {
    schema_version: u16,
    is_enabled: bool,
    app_id: Option<String>,
    environment: Option<String>,
    deployment: Option<String>,
    callback_url: Option<String>,
    required_scopes: Option<Vec<String>>,
    current_key_version: Option<u16>,
    database_ca_path: Option<PathBuf>,
    api_pool_connections: Option<u32>,
    cleanup_pool_connections: Option<u32>,
}
pub(crate) struct ActiveSettings {
    pub(crate) app_id: String,
    pub(crate) environment: String,
    pub(crate) deployment: String,
    pub(crate) callback_url: String,
    pub(crate) required_scopes: Vec<String>,
    pub(crate) current_key_version: u16,
    pub(crate) database_ca_path: PathBuf,
    pub(crate) api_pool_connections: u32,
    pub(crate) cleanup_pool_connections: u32,
}
pub(crate) fn runtime(
    text: &str,
    environment: ExpectedEnvironment,
) -> Result<Option<ActiveSettings>, AppError> {
    let fields: RuntimeFields = json::parse(text)?;
    schema(fields.schema_version)?;
    if !fields.is_enabled {
        return Ok(None);
    }
    let active = active(fields)?;
    active.validate(environment)?;
    Ok(Some(active))
}
fn active(fields: RuntimeFields) -> Result<ActiveSettings, AppError> {
    Ok(ActiveSettings {
        app_id: required(fields.app_id)?,
        environment: required(fields.environment)?,
        deployment: required(fields.deployment)?,
        callback_url: required(fields.callback_url)?,
        required_scopes: required(fields.required_scopes)?,
        current_key_version: required(fields.current_key_version)?,
        database_ca_path: required(fields.database_ca_path)?,
        api_pool_connections: required(fields.api_pool_connections)?,
        cleanup_pool_connections: required(fields.cleanup_pool_connections)?,
    })
}
impl ActiveSettings {
    fn validate(&self, environment: ExpectedEnvironment) -> Result<(), AppError> {
        let expected = match environment {
            ExpectedEnvironment::Boe => "boe",
            ExpectedEnvironment::Production => "prod",
        };
        let is_valid = text(&self.app_id, 256)
            && self
                .app_id
                .bytes()
                .all(|byte| (0x21..=0x7e).contains(&byte))
            && self.environment == expected
            && text(&self.deployment, 256)
            && self.current_key_version > 0
            && self.database_ca_path.is_absolute()
            && (1..=32).contains(&self.api_pool_connections)
            && (1..=32).contains(&self.cleanup_pool_connections);
        if !is_valid {
            return Err(AppError::Configuration);
        }
        callback(&self.callback_url)?;
        scopes(&self.required_scopes)
    }
}
fn callback(value: &str) -> Result<(), AppError> {
    let url = url::Url::parse(value).map_err(|_| AppError::Configuration)?;
    let is_valid = value.len() <= 8192
        && url.as_str() == value
        && url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && !value.chars().any(char::is_control)
        && url.path() == "/v1/oauth/doubao/callback";
    is_valid.then_some(()).ok_or(AppError::Configuration)
}
fn scopes(values: &[String]) -> Result<(), AppError> {
    let unique: std::collections::BTreeSet<_> = values.iter().collect();
    let is_valid = !values.is_empty()
        && values.len() <= 100
        && unique.len() == values.len()
        && values.iter().any(|value| value == "offline_access")
        && values.iter().all(|value| {
            !value.is_empty()
                && value.len() <= 256
                && value
                    .bytes()
                    .all(|byte| matches!(byte, 0x21 | 0x23..=0x5b | 0x5d..=0x7e))
        })
        && values.iter().map(String::len).sum::<usize>() <= 4096;
    is_valid.then_some(()).ok_or(AppError::Configuration)
}
pub(crate) fn required<T>(value: Option<T>) -> Result<T, AppError> {
    value.ok_or(AppError::Configuration)
}
pub(crate) fn schema(version: u16) -> Result<(), AppError> {
    (version == 1).then_some(()).ok_or(AppError::Configuration)
}
pub(crate) fn text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}
#[cfg(test)]
fn validate(
    runtime_text: &str,
    secrets_text: Option<&str>,
    environment: ExpectedEnvironment,
) -> Result<(), AppError> {
    if let Some(active) = runtime(runtime_text, environment)? {
        Secrets::parse(required(secrets_text)?, active.current_key_version)?;
    }
    Ok(())
}
#[cfg(test)]
pub(crate) mod tests;
