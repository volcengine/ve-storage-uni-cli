//! Select a complete compiled application or a wholly separate internal runtime override.
use super::{
    application_values::{parse_bundle, validate_scopes, ApplicationValues, ASK_SCOPES},
    Config, Error, Result,
};
use std::{ffi::OsString, path::PathBuf};

/// Snapshot of runtime settings; presence selects override mode even for invalid values.
pub(super) struct ApplicationEnvironment {
    app_id: Option<OsString>,
    scopes: Option<OsString>,
    secret_file: Option<OsString>,
    callback: Option<OsString>,
    home: Option<OsString>,
}
impl ApplicationEnvironment {
    pub(super) fn read() -> Self {
        Self {
            app_id: std::env::var_os("STORAGE_CLI_DOUBAO_APP_ID"),
            scopes: std::env::var_os("STORAGE_CLI_DOUBAO_SCOPES"),
            secret_file: std::env::var_os("STORAGE_CLI_DOUBAO_APP_SECRET_FILE"),
            callback: std::env::var_os("STORAGE_CLI_DOUBAO_CALLBACK"),
            home: std::env::var_os("STORAGE_CLI_ASK_HOME"),
        }
    }
    fn has_application_override(&self) -> bool {
        self.app_id.is_some() || self.scopes.is_some() || self.secret_file.is_some()
    }
}

pub(super) fn compiled_application() -> Result<Option<ApplicationValues<'static>>> {
    parse_bundle(
        option_env!("STORAGE_CLI_BUILD_DOUBAO_APP_ID"),
        option_env!("STORAGE_CLI_BUILD_DOUBAO_APP_SECRET"),
    )
    .map_err(|_| Error::Configuration)
}

pub(super) fn configure(
    environment: ApplicationEnvironment,
    bundled: Option<ApplicationValues<'static>>,
) -> Result<Config> {
    let redirect = match environment.callback.as_ref() {
        Some(value) => value.to_str().ok_or(Error::Configuration)?,
        None => "http://localhost:8080/callback",
    };
    let home = environment
        .home
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| dirs_next::home_dir().map(|path| path.join(".tos/ask")))
        .ok_or(Error::Configuration)?;
    // [Review Fix #14] Any override presence selects an independent app; never mix credentials.
    if environment.has_application_override() {
        return configure_override(&environment, redirect, home);
    }
    let bundled = bundled.ok_or(Error::Configuration)?;
    let scopes = validate_scopes(ASK_SCOPES).map_err(|_| Error::Configuration)?;
    let mut config = Config::new(
        bundled.app_id.into(),
        redirect,
        scopes.into_iter().map(String::from).collect(),
        home,
    )?;
    config.bundled_secret = Some(bundled.secret);
    Ok(config)
}

fn configure_override(
    environment: &ApplicationEnvironment,
    redirect: &str,
    home: PathBuf,
) -> Result<Config> {
    let app_id = required_text(environment.app_id.as_ref())?;
    let scopes = validate_scopes(required_text(environment.scopes.as_ref())?)
        .map_err(|_| Error::Configuration)?;
    let mut config = Config::new(
        app_id.into(),
        redirect,
        scopes.into_iter().map(String::from).collect(),
        home,
    )?;
    config.secret_file = environment
        .secret_file
        .as_ref()
        .map(|path| required_text(Some(path)).map(PathBuf::from))
        .transpose()?;
    Ok(config)
}

fn required_text(value: Option<&OsString>) -> Result<&str> {
    let value = value
        .and_then(|value| value.to_str())
        .ok_or(Error::Configuration)?;
    if value.is_empty() {
        return Err(Error::Configuration);
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
