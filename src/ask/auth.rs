use serde_json::{json, Value};
use std::{io::IsTerminal, time::Duration};
use tos_core::{
    agent::{error::CliError, global_args::GlobalArgs},
    ask::{Client, Config, Error, Secret, SessionError, Status, Store},
};

const LOGIN_TIMEOUT: Duration = Duration::from_secs(600);

// [Review Fix #10] Recovery uses the accepted assistant auth namespace.
pub(super) fn fix_command(global: &GlobalArgs, prefix: &str) -> String {
    let mut command = format!("{prefix} ask auth login --no-browser");
    // [Review Fix #23] Recovery must target the same profile and stores, quoting opaque values.
    if global.profile != "default" {
        command.push_str(&recovery_option("--profile", &global.profile));
    }
    for (flag, path) in [
        ("--config-path", &global.config_path),
        ("--credentials-path", &global.credentials_path),
    ] {
        if let Some(path) = path {
            command.push_str(&recovery_option(flag, &path.to_string_lossy()));
        }
    }
    command
}

fn recovery_option(flag: &str, value: &str) -> String {
    // [Review Fix #24] Quotes do not stop Clap treating a leading hyphen as another option.
    let separator = if value.starts_with('-') { "=" } else { " " };
    format!(" {flag}{separator}{}", super::reply::quote(value))
}

fn configuration(global: &GlobalArgs) -> Result<Config, CliError> {
    Config::for_cli()
        .and_then(|config| {
            config.with_credentials_store(global.credentials_path(), global.profile.clone())
        })
        .map_err(core_error)
}
fn configuration_error() -> CliError {
    // [Review Fix #14] Customer recovery belongs to the publisher, never an app-secret prompt.
    CliError::ConfigMissing("Doubao application configuration is unavailable; use an official prebuilt binary or contact the publisher. Internal application overrides must be complete".into())
}
fn core_error(error: Error) -> CliError {
    match error {
        Error::Configuration => configuration_error(),
        // [Review Fix #25] The envelope supplies profile-aware recovery; omit a conflicting bare command.
        Error::LoginRequired | Error::Denied => {
            CliError::AuthFailed("Doubao login is required".into())
        }
        _ => CliError::Unknown(error.to_string()),
    }
}
pub(super) fn session_error(error: SessionError) -> CliError {
    match error {
        SessionError::Unauthorized => CliError::AuthFailed(
            "Doubao access was denied; login again. The question was not replayed".into(),
        ),
        // [Review Fix #2] Keep permission and quota failures distinct from login recovery.
        SessionError::PermissionDenied => CliError::PermissionDenied("Doubao permission was denied; verify application permissions, user scopes and allowlisting. The question was not replayed".into()),
        SessionError::RateLimited => CliError::RateLimited("Doubao rate limit was reached; the question was not retried".into()),
        _ => CliError::Unknown(error.to_string()),
    }
}

pub(super) fn status(global: &GlobalArgs) -> Result<Value, CliError> {
    let config = configuration(global)?;
    let store = Store::open(&config).map_err(core_error)?;
    match store.status().map_err(core_error)? {
        Status::LoggedOut => {
            Ok(json!({"provider":"doubao","status":"logged_out","remote_checked":false}))
        }
        Status::LoginRequired => {
            Ok(json!({"provider":"doubao","status":"login_required","remote_checked":false}))
        }
        Status::Ready {
            access_expires_at,
            refresh_expires_at,
        } => Ok(
            json!({"provider":"doubao","status":"ready","remote_checked":false,"access_expires_at":access_expires_at,"refresh_expires_at":refresh_expires_at}),
        ),
    }
}
pub(super) fn logout(global: &GlobalArgs) -> Result<Value, CliError> {
    let config = configuration(global)?;
    Store::open(&config)
        .map_err(core_error)?
        .logout()
        .map_err(core_error)?;
    Ok(json!({"provider":"doubao","status":"logged_out","remote_revoked":false}))
}

pub(super) async fn access_token(global: &GlobalArgs, prefix: &str) -> Result<Secret, CliError> {
    let config = configuration(global)?;
    let store = Store::open(&config).map_err(core_error)?;
    let status = store.status().map_err(core_error)?;
    if status == Status::LoggedOut || status == Status::LoginRequired {
        if !can_auto_login(
            global.quiet,
            std::io::stdin().is_terminal(),
            std::io::stderr().is_terminal(),
        ) {
            return Err(CliError::AuthFailed(format!(
                "Doubao login is required; run {}",
                fix_command(global, prefix)
            )));
        }
        let client = client(&config)?;
        complete_login(&client, &store, false).await?;
        return store.access_token(&client).await.map_err(core_error);
    }
    store
        .access_token(&client(&config)?)
        .await
        .map_err(core_error)
}
fn client(config: &Config) -> Result<Client, CliError> {
    let secret = config.load_secret().map_err(core_error)?;
    Client::new(config.clone(), secret).map_err(core_error)
}
fn can_auto_login(is_quiet: bool, is_stdin_tty: bool, is_stderr_tty: bool) -> bool {
    !is_quiet && is_stdin_tty && is_stderr_tty
}

pub(super) async fn login(
    global: &GlobalArgs,
    prefix: &str,
    has_no_browser: bool,
) -> Result<Value, CliError> {
    let config = configuration(global)?;
    if !has_no_browser
        && !can_auto_login(
            global.quiet,
            std::io::stdin().is_terminal(),
            std::io::stderr().is_terminal(),
        )
    {
        return Err(CliError::AuthFailed(format!(
            "Use manual authorization in this context: {}",
            fix_command(global, prefix)
        )));
    }
    let store = Store::open(&config).map_err(core_error)?;
    complete_login(&client(&config)?, &store, has_no_browser).await?;
    Ok(json!({"provider":"doubao","status":"ready","experimental":true}))
}
async fn complete_login(
    client: &Client,
    store: &Store,
    has_no_browser: bool,
) -> Result<(), CliError> {
    let pending = client.begin_login().await.map_err(core_error)?;
    // The listener is already bound before the one-time URL is presented or opened.
    eprintln!(
        "Authorize Doubao (expires in 10 minutes):\n{}",
        pending.authorization_url()
    );
    if !has_no_browser {
        super::browser::open(pending.authorization_url()).await?;
    }
    let code = pending.wait(LOGIN_TIMEOUT).await.map_err(core_error)?;
    let tokens = client.finish_code(code).await.map_err(core_error)?;
    store.save_login(&tokens).map_err(core_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_auth_cannot_start_interactive_work_in_pipes_or_quiet_mode() {
        assert!(can_auto_login(false, true, true));
        for flags in [
            (true, true, true),
            (false, false, true),
            (false, true, false),
            (false, false, false),
        ] {
            assert!(!can_auto_login(flags.0, flags.1, flags.2));
        }
    }
}
