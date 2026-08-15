/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use serde_json::json;
use tokio::time::Instant;
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::config::ConfigFile;
use tos_core::infra::credentials::{CredentialsFile, StoredOAuthCredentials};

use crate::cli::auth::{AuthAction, AuthCommand, LoginArgs};
use crate::cli::ADriveAuthArgs;
use crate::domain::auth::{resolve_oauth_client_id, AuthMode, ResolvedAuthMode, OAUTH_SCOPE};
use crate::domain::client::{normalize_endpoint_scheme, ClientOptions};
use crate::domain::oauth::{
    DeviceAuthorizationRequest, DeviceAuthorizationResponse, DeviceTokenOutcome, OAuthClient,
    OAuthClientError, OAuthFailure, TokenResponse,
};
use crate::handler::common::{
    inspect_selected_credentials, inspect_unified_credentials_for_profile, oauth_client_options,
    output_envelope, resolve_auth_mode, UnifiedCredentialInspection,
};
use crate::handler::meta::describe_adrive_command_metadata;

const SLOW_DOWN_INCREMENT: Duration = Duration::from_secs(5);

struct LoginSettings {
    client_id: String,
    instance_id: String,
    device_name: String,
    resource_endpoint: Option<String>,
    client_options: ClientOptions,
}

struct SavedLogin {
    expires_at: String,
    scope: Vec<String>,
    user_id: Option<String>,
}

struct PollingSchedule {
    interval: Duration,
}

enum PollOnceError {
    Client(OAuthClientError),
    Expired,
    Cancelled,
    Signal(std::io::Error),
}

impl PollingSchedule {
    fn new(interval: Duration) -> Result<Self, CliError> {
        if interval.is_zero() {
            return Err(CliError::ValidationError(
                "OAuth polling interval must be greater than zero".to_string(),
            ));
        }
        Ok(Self { interval })
    }

    fn interval(&self) -> Duration {
        self.interval
    }

    fn apply_slow_down(&mut self, server_interval: Option<Duration>) {
        let incremented = self
            .interval
            .checked_add(SLOW_DOWN_INCREMENT)
            .unwrap_or(Duration::MAX);
        self.interval = server_interval.map_or(incremented, |value| incremented.max(value));
    }

    fn apply_retry_hint(&mut self, retry_after: Option<Duration>) {
        if let Some(retry_after) = retry_after {
            self.interval = self.interval.max(retry_after);
        }
    }
}

/// Handle ADrive authentication commands.
pub async fn handle_auth_command(
    global: &GlobalArgs,
    auth_args: &ADriveAuthArgs,
    command: &AuthCommand,
) -> Result<i32, CliError> {
    if global.describe {
        // [Review Fix #4] Describe is metadata-only: do not resolve a profile
        // or touch either SDK-managed or local OAuth/AKSK credentials.
        let description = describe_adrive_command_metadata("ve-adrive auth").ok_or_else(|| {
            CliError::ValidationError("no metadata registered for ve-adrive auth".to_string())
        })?;
        output_envelope(global, &Envelope::success("ve-adrive auth", description))?;
        return Ok(0);
    }
    let resolved = resolve_auth_mode(global, auth_args.auth_mode)?;
    if resolved.mode == AuthMode::Unified {
        match command.action.as_ref() {
            Some(AuthAction::Login(_)) => {
                return Err(unified_login_managed_externally(global));
            }
            Some(AuthAction::Logout) => {
                return Err(unified_logout_managed_externally(global));
            }
            None | Some(AuthAction::Status) => {}
        }
    }
    match command.action.as_ref() {
        None | Some(AuthAction::Status) => handle_status(global, command, resolved).await,
        Some(AuthAction::Login(args)) => handle_login(global, resolved.mode, args).await,
        Some(AuthAction::Logout) => handle_logout(global, resolved.mode),
    }
}

fn unified_login_managed_externally(global: &GlobalArgs) -> CliError {
    CliError::ValidationError(format!(
        "[unified_login_managed_externally] ADrive Unified auth login for profile '{}' is managed externally; run `ve login` for the same profile. This command does not modify Unified login state",
        global.profile
    ))
}

// [Review Fix #4] Keep logout machine-readable and actionable independently
// from login, while leaving all externally owned state untouched.
fn unified_logout_managed_externally(global: &GlobalArgs) -> CliError {
    CliError::ValidationError(format!(
        "[unified_logout_managed_externally] ADrive Unified auth logout for profile '{}' is managed externally; run `ve logout` for the same profile. This command does not modify Unified login state",
        global.profile
    ))
}

async fn handle_status(
    global: &GlobalArgs,
    command: &AuthCommand,
    resolved: ResolvedAuthMode,
) -> Result<i32, CliError> {
    let command_path = if command.action.is_some() {
        "ve-adrive auth status"
    } else {
        "ve-adrive auth"
    };
    if resolved.mode == AuthMode::Unified {
        let inspection = inspect_unified_credentials_for_profile(&global.profile).await;
        output_envelope(
            global,
            &Envelope::success(
                command_path,
                unified_status_payload(global, resolved, &inspection),
            ),
        )?;
        return Ok(0);
    }

    let credentials = inspect_selected_credentials(global, resolved.mode)?;
    output_envelope(
        global,
        &Envelope::success(
            command_path,
            json!({
                "mode": resolved.mode.as_str(),
                "source": resolved.source.as_str(),
                "has_access_key": credentials.has_access_key,
                "has_secret_key": credentials.has_secret_key,
                "has_security_token": credentials.has_security_token,
                "has_access_token": credentials.has_access_token,
                "has_refresh_token": credentials.has_refresh_token,
                "access_token_expiry": credentials.access_token_expiry,
                "expires_at": credentials.expires_at,
                "scope": credentials.scope,
                "instance_id": credentials.instance_id,
                "ready": credentials.is_ready(),
                "oauth_service_integration": credentials.oauth_service_integration,
                // [Review Fix #2] AK/SK provenance is a Doctor enhancement;
                // preserve the existing auth-status contract for compatibility.
                "credential_source": if resolved.mode == AuthMode::Aksk {
                    "resolved_profile"
                } else {
                    credentials.credential_source
                }
            }),
        ),
    )?;
    Ok(0)
}

fn unified_status_payload(
    global: &GlobalArgs,
    resolved: ResolvedAuthMode,
    inspection: &UnifiedCredentialInspection,
) -> serde_json::Value {
    let mut payload = json!({
        "mode": resolved.mode.as_str(),
        "source": resolved.source.as_str(),
        "profile": global.profile,
        "provider_name": inspection.provider_name,
        "has_session_token": inspection.has_session_token,
        "ready": inspection.ready,
    });
    if let (Some(payload), Some(sdk_code)) = (payload.as_object_mut(), inspection.sdk_code.as_ref())
    {
        payload.insert("sdk_code".to_string(), json!(sdk_code));
    }
    payload
}

async fn handle_login(
    global: &GlobalArgs,
    mode: AuthMode,
    args: &LoginArgs,
) -> Result<i32, CliError> {
    require_oauth_mode(mode, "login")?;
    let (settings, auth_endpoint) = resolve_login_settings(global, args)?;
    let oauth_client = OAuthClient::new_with_request_trace(
        auth_endpoint,
        settings.client_options.clone(),
        std::sync::Arc::clone(&global.request_trace),
    )
    .map_err(map_oauth_client_error)?;
    reject_matching_resource_origin(&oauth_client, settings.resource_endpoint.as_deref())?;
    if global.dry_run {
        return output_login_dry_run(global, &settings, oauth_client.endpoint().as_str());
    }

    let authorization = create_device_grant(&oauth_client, &settings).await?;
    show_login_instructions(&authorization);
    let token = poll_until_authorized(&oauth_client, &authorization, &settings.client_id).await?;
    let saved = validate_and_save_token(global, &settings, oauth_client.endpoint(), token)?;
    output_login_success(global, &settings, saved)
}

async fn create_device_grant(
    client: &OAuthClient,
    settings: &LoginSettings,
) -> Result<DeviceAuthorizationResponse, CliError> {
    let response = client
        .create_device_authorization(&DeviceAuthorizationRequest {
            client_id: settings.client_id.clone(),
            instance_id: settings.instance_id.clone(),
            device_name: settings.device_name.clone(),
            scope: OAUTH_SCOPE.to_string(),
        })
        .await
        .map_err(map_oauth_client_error)?;
    validate_device_authorization(response)
}

async fn poll_until_authorized(
    client: &OAuthClient,
    authorization: &DeviceAuthorizationResponse,
    client_id: &str,
) -> Result<TokenResponse, CliError> {
    let mut schedule = PollingSchedule::new(Duration::from_secs(authorization.interval))?;
    let lifetime = Duration::from_secs(authorization.expires_in);
    let deadline = Instant::now()
        .checked_add(lifetime)
        .ok_or_else(|| invalid_oauth_response("Device Grant lifetime is too large"))?;
    loop {
        wait_for_next_poll(schedule.interval(), deadline).await?;
        let outcome = match poll_once(client, &authorization.device_code, client_id, deadline).await
        {
            Ok(outcome) => outcome,
            Err(PollOnceError::Expired) => {
                return Err(CliError::AuthFailed(
                    "[expired_token] Device authorization expired; run auth login again"
                        .to_string(),
                ))
            }
            // [Review Fix #3] A transient connection/timeout does not consume
            // the Device Grant; retry on the normal schedule until its deadline.
            Err(PollOnceError::Client(OAuthClientError::Http(error)))
                if is_retryable_transport_error(&error) =>
            {
                continue
            }
            Err(PollOnceError::Client(error)) => return Err(map_oauth_client_error(error)),
            Err(PollOnceError::Cancelled) => {
                return Err(CliError::AuthFailed("OAuth login cancelled".to_string()))
            }
            Err(PollOnceError::Signal(error)) => return Err(CliError::Io(error)),
        };
        match outcome {
            DeviceTokenOutcome::Success(token) => return Ok(token),
            DeviceTokenOutcome::Pending(_) => {}
            DeviceTokenOutcome::Temporary(failure) => schedule.apply_retry_hint(
                failure
                    .retry_after_seconds
                    .or(failure.interval)
                    .map(Duration::from_secs),
            ),
            DeviceTokenOutcome::SlowDown(failure) => {
                schedule.apply_slow_down(failure.interval.map(Duration::from_secs))
            }
            DeviceTokenOutcome::Denied(failure)
            | DeviceTokenOutcome::Expired(failure)
            | DeviceTokenOutcome::Terminal(failure) => return Err(map_oauth_failure(failure)),
        }
    }
}

async fn poll_once(
    client: &OAuthClient,
    device_code: &str,
    client_id: &str,
    deadline: Instant,
) -> Result<DeviceTokenOutcome, PollOnceError> {
    tokio::select! {
        result = client.poll_device_token(device_code, client_id) => {
            result.map_err(PollOnceError::Client)
        }
        _ = tokio::time::sleep_until(deadline) => Err(PollOnceError::Expired),
        // [Review Fix #7] Keep Ctrl-C responsive while an HTTP poll is in
        // flight; Tokio installs a process-wide signal handler after first use.
        signal = tokio::signal::ctrl_c() => match signal {
            Ok(()) => Err(PollOnceError::Cancelled),
            Err(error) => Err(PollOnceError::Signal(error)),
        },
    }
}

async fn wait_for_next_poll(interval: Duration, deadline: Instant) -> Result<(), CliError> {
    let wake_at = Instant::now()
        .checked_add(interval)
        .map_or(deadline, |candidate| candidate.min(deadline));
    tokio::select! {
        _ = tokio::time::sleep_until(wake_at) => {
            if Instant::now() >= deadline {
                Err(CliError::AuthFailed("[expired_token] Device authorization expired; run auth login again".to_string()))
            } else {
                Ok(())
            }
        }
        result = tokio::signal::ctrl_c() => {
            result.map_err(CliError::Io)?;
            Err(CliError::AuthFailed("OAuth login cancelled".to_string()))
        }
    }
}

fn resolve_login_settings(
    global: &GlobalArgs,
    args: &LoginArgs,
) -> Result<(LoginSettings, String), CliError> {
    let config_path = global.existing_runtime_config_path()?;
    let config = ConfigFile::load_from(&config_path)?;
    let profile = config.profiles.get(&global.profile);
    if profile.is_none() && !(config.profiles.is_empty() && global.profile == "default") {
        return Err(CliError::ConfigMissing(format!(
            "Profile '{}' not found in {}",
            global.profile,
            config_path.display()
        )));
    }
    // [Review Fix #5] Login resolves only non-secret raw config fields, so an
    // OAuth flow never parses or decrypts the unselected AK/SK credential set.
    let adrive = profile.and_then(|profile| profile.adrive.as_ref());
    let instance_id = first_nonempty([
        args.instance.clone(),
        adrive.and_then(|settings| settings.default_instance.clone()),
        std::env::var("ADRIVE_DEFAULT_INSTANCE").ok(),
    ])
    .ok_or_else(|| {
        // [Review Fix #3] Preserve the missing-Instance cause so the outer
        // error contract recommends the login command instead of config init.
        CliError::ConfigMissing("[oauth_instance_required] ADrive OAuth instance is required; use --instance, profile default_instance, or ADRIVE_DEFAULT_INSTANCE".to_string())
    })?;
    let auth_endpoint = resolve_login_auth_endpoint_value(
        args.auth_endpoint.clone(),
        adrive.and_then(|settings| settings.auth_endpoint.clone()),
        std::env::var("ADRIVE_AUTH_ENDPOINT").ok(),
    )?;
    let device_name = validate_device_name(resolve_device_name(args.device_name.clone()))?;
    let resource_endpoint = resolve_resource_endpoint(global, adrive);
    let client_options = oauth_client_options(profile, adrive);
    Ok((
        LoginSettings {
            client_id: resolve_oauth_client_id(),
            instance_id,
            device_name,
            resource_endpoint,
            client_options,
        },
        auth_endpoint,
    ))
}

fn resolve_login_auth_endpoint_value(
    command_line_value: Option<String>,
    config_value: Option<String>,
    environment_value: Option<String>,
) -> Result<String, CliError> {
    first_nonempty([command_line_value, config_value, environment_value]).ok_or_else(|| {
        CliError::ConfigMissing(
            "[oauth_auth_endpoint_required] ADrive OAuth auth endpoint is required; use --auth-endpoint, [profile.adrive].auth_endpoint, or ADRIVE_AUTH_ENDPOINT"
                .to_string(),
        )
    })
}

pub(crate) fn has_configured_login_auth_endpoint(global: &GlobalArgs) -> Result<bool, CliError> {
    let config_path = global.existing_runtime_config_path()?;
    let config = ConfigFile::load_from(&config_path)?;
    let profile = config.profiles.get(&global.profile);
    if profile.is_none() && !(config.profiles.is_empty() && global.profile == "default") {
        return Err(CliError::ConfigMissing(format!(
            "Profile '{}' not found in {}",
            global.profile,
            config_path.display()
        )));
    }
    let config_value = profile
        .and_then(|profile| profile.adrive.as_ref())
        .and_then(|settings| settings.auth_endpoint.clone());
    Ok(first_nonempty([config_value, std::env::var("ADRIVE_AUTH_ENDPOINT").ok()]).is_some())
}

fn resolve_resource_endpoint(
    global: &GlobalArgs,
    configured: Option<&tos_core::infra::config::AdriveOverride>,
) -> Option<String> {
    first_nonempty([
        global.endpoint.clone(),
        configured.and_then(|settings| settings.endpoint.clone()),
        std::env::var("ADRIVE_ENDPOINT").ok(),
    ])
}

fn resolve_device_name(command_line: Option<String>) -> String {
    first_nonempty([
        command_line,
        std::env::var("ADRIVE_DEVICE_NAME").ok(),
        std::env::var("HOSTNAME").ok(),
        std::env::var("COMPUTERNAME").ok(),
        Some("ve-adrive-cli".to_string()),
    ])
    .expect("default device name is non-empty")
}

fn validate_device_name(device_name: String) -> Result<String, CliError> {
    // [Review Fix #4] Device Name is shown in an authorization UI, so reject
    // control characters and enforce the frozen 1–64 Unicode scalar limit.
    let character_count = device_name.chars().count();
    if character_count == 0 || character_count > 64 || device_name.chars().any(char::is_control) {
        return Err(CliError::ValidationError(
            "OAuth device name must contain 1 to 64 characters and no control characters"
                .to_string(),
        ));
    }
    Ok(device_name)
}

fn first_nonempty<const N: usize>(values: [Option<String>; N]) -> Option<String> {
    values
        .into_iter()
        .flatten()
        .find(|value| !value.trim().is_empty())
}

fn nonempty_string(value: Option<String>) -> Option<String> {
    // [Review Fix #1] Persist a canonical user binding so whitespace from IDS
    // cannot become part of the owner identity used by later commands.
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn validate_device_authorization(
    response: DeviceAuthorizationResponse,
) -> Result<DeviceAuthorizationResponse, CliError> {
    if response.device_code.trim().is_empty()
        || response.user_code.trim().is_empty()
        || response.user_code.chars().count() > 128
        || response.user_code.chars().any(char::is_control)
        || response.verification_uri.trim().is_empty()
        || response.verification_uri_complete.trim().is_empty()
        || response.expires_in == 0
        || response.interval == 0
    {
        // [Review Fix #8] User Code is rendered directly in the terminal.
        // Bound it and reject control characters before any output occurs.
        return Err(invalid_oauth_response(
            "Device Authorization response contains an invalid required field",
        ));
    }
    // [Review Fix #3] Never direct a user to a local file/custom protocol from
    // an Authorization Server response. Preserve the validated URI bytes.
    validate_verification_url(&response.verification_uri)?;
    validate_verification_url(&response.verification_uri_complete)?;
    Ok(response)
}

fn validate_verification_url(value: &str) -> Result<(), CliError> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| invalid_oauth_response("verification URI must be an absolute URL"))?;
    let is_loopback = url.host().is_some_and(|host| match host {
        url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    });
    let is_safe_scheme = url.scheme() == "https" || (url.scheme() == "http" && is_loopback);
    if !is_safe_scheme || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid_oauth_response(
            "verification URI must use HTTPS without userinfo",
        ));
    }
    Ok(())
}

fn show_login_instructions(response: &DeviceAuthorizationResponse) {
    for line in login_instruction_lines(response) {
        eprintln!("{line}");
    }
}

fn login_instruction_lines(response: &DeviceAuthorizationResponse) -> Vec<String> {
    vec![
        "Open this URL to authorize ve-adrive-cli:".to_string(),
        response.verification_uri_complete.clone(),
        "Waiting for authorization…".to_string(),
    ]
}

fn validate_and_save_token(
    global: &GlobalArgs,
    settings: &LoginSettings,
    auth_endpoint: &reqwest::Url,
    token: TokenResponse,
) -> Result<SavedLogin, CliError> {
    let (stored, saved) = validated_login(settings, auth_endpoint, token)?;
    let credentials_path = global.credentials_path();
    let mut credentials = CredentialsFile::load_from(&credentials_path)?;
    credentials.set_adrive_oauth(&global.profile, stored)?;
    credentials.save_to_path(&credentials_path)?;
    Ok(saved)
}

fn validated_login(
    settings: &LoginSettings,
    auth_endpoint: &reqwest::Url,
    token: TokenResponse,
) -> Result<(StoredOAuthCredentials, SavedLogin), CliError> {
    if token.access_token.trim().is_empty() || token.refresh_token.trim().is_empty() {
        return Err(invalid_oauth_response(
            "Token response omitted a required token",
        ));
    }
    if !token.token_type.eq_ignore_ascii_case("bearer") || token.expires_in == 0 {
        return Err(invalid_oauth_response(
            "Token response has an invalid token_type or expires_in",
        ));
    }
    if token.instance_id != settings.instance_id {
        return Err(invalid_oauth_response(
            "Token response instance_id does not match the authorization request",
        ));
    }
    let scope = token
        .scope
        .split_ascii_whitespace()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if scope.is_empty() {
        return Err(invalid_oauth_response("Token response scope is empty"));
    }
    let expires_at = expires_at(token.expires_in)?;
    let user_id = nonempty_string(token.user_id);
    let saved = SavedLogin {
        expires_at: expires_at.clone(),
        scope: scope.clone(),
        user_id: user_id.clone(),
    };
    let stored = StoredOAuthCredentials {
        access_token: Some(token.access_token),
        refresh_token: Some(token.refresh_token),
        expires_at: Some(expires_at),
        token_type: Some("Bearer".to_string()),
        scope,
        legacy_client_id: None,
        instance_id: Some(settings.instance_id.clone()),
        user_id,
        auth_endpoint: Some(auth_endpoint.as_str().to_string()),
    };
    Ok((stored, saved))
}

fn expires_at(expires_in: u64) -> Result<String, CliError> {
    let seconds = i64::try_from(expires_in)
        .map_err(|_| invalid_oauth_response("Token lifetime is too large"))?;
    Utc::now()
        .checked_add_signed(chrono::Duration::seconds(seconds))
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
        .ok_or_else(|| invalid_oauth_response("Token expiry is outside the supported range"))
}

fn output_login_success(
    global: &GlobalArgs,
    settings: &LoginSettings,
    saved: SavedLogin,
) -> Result<i32, CliError> {
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive auth login",
            json!({
                "status": "logged_in",
                "profile": global.profile,
                "instance_id": settings.instance_id,
                "scope": saved.scope,
                "expires_at": saved.expires_at,
                "user_id": saved.user_id
            }),
        ),
    )?;
    Ok(0)
}

fn output_login_dry_run(
    global: &GlobalArgs,
    settings: &LoginSettings,
    auth_endpoint: &str,
) -> Result<i32, CliError> {
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive auth login",
            json!({
                "dry_run": true,
                "profile": global.profile,
                "instance_id": settings.instance_id,
                "device_name": settings.device_name,
                "scope": OAUTH_SCOPE,
                "auth_endpoint": auth_endpoint,
                "network_requests": 0,
                "credentials_written": false
            }),
        ),
    )?;
    Ok(0)
}

fn handle_logout(global: &GlobalArgs, mode: AuthMode) -> Result<i32, CliError> {
    require_oauth_mode(mode, "logout")?;
    let credentials_path = global.credentials_path();
    if global.dry_run {
        // [Review Fix #1] Registry-backed dry-run must never clear OAuth state
        // or create encryption key material while previewing logout.
        let would_clear = if credentials_path.exists() {
            CredentialsFile::load_from(&credentials_path)?.has_adrive_oauth_tokens(&global.profile)
        } else {
            false
        };
        return output_logout_dry_run(global, would_clear);
    }
    let mut cleared = false;
    if credentials_path.exists() {
        let mut credentials = CredentialsFile::load_from(&credentials_path)?;
        let existing = credentials.adrive_oauth(&global.profile, &credentials_path)?;
        if !existing.is_empty() {
            credentials.clear_adrive_oauth(&global.profile);
            credentials.save_to_path(&credentials_path)?;
            cleared = true;
        }
    }
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive auth logout",
            json!({
                "mode": "oauth",
                "status": if cleared { "logged_out" } else { "already_logged_out" },
                "profile": global.profile,
                "scope": "local_credentials_file_only"
            }),
        ),
    )?;
    Ok(0)
}

fn output_logout_dry_run(global: &GlobalArgs, would_clear: bool) -> Result<i32, CliError> {
    output_envelope(
        global,
        &Envelope::success(
            "ve-adrive auth logout",
            json!({
                "mode": "oauth",
                "dry_run": true,
                "status": if would_clear { "would_log_out" } else { "already_logged_out" },
                "profile": global.profile,
                "scope": "local_credentials_file_only",
                "would_clear": would_clear,
                "credentials_written": false
            }),
        ),
    )?;
    Ok(0)
}

fn require_oauth_mode(mode: AuthMode, action: &str) -> Result<(), CliError> {
    if mode == AuthMode::Oauth {
        return Ok(());
    }
    Err(CliError::ValidationError(format!(
        "auth {action} requires OAuth mode; select it with --auth-mode oauth, ADRIVE_AUTH_MODE=oauth, or profile auth_mode=oauth"
    )))
}

fn reject_matching_resource_origin(
    auth_client: &OAuthClient,
    resource_endpoint: Option<&str>,
) -> Result<(), CliError> {
    let Some(resource_endpoint) = resource_endpoint else {
        return Ok(());
    };
    let resource_endpoint = normalize_endpoint_scheme(resource_endpoint);
    let resource = reqwest::Url::parse(&resource_endpoint).map_err(|_| {
        CliError::ValidationError("ADrive resource endpoint must be an absolute URL".to_string())
    })?;
    if same_origin(auth_client.endpoint(), &resource) {
        return Err(CliError::ValidationError(
            "ADrive auth endpoint and resource endpoint must use different origins".to_string(),
        ));
    }
    Ok(())
}

fn same_origin(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn map_oauth_client_error(error: OAuthClientError) -> CliError {
    match error {
        OAuthClientError::OAuth(failure) => map_oauth_failure(failure),
        OAuthClientError::Http(error) => CliError::Http(error),
        OAuthClientError::Response(message) => invalid_oauth_response(&message),
    }
}

fn is_retryable_transport_error(error: &reqwest::Error) -> bool {
    error.is_timeout() || error.is_connect()
}

fn map_oauth_failure(failure: OAuthFailure) -> CliError {
    let diagnostic = oauth_failure_diagnostic(&failure);
    match failure.code.as_str() {
        "invalid_request"
        | "invalid_scope"
        | "unsupported_response_type"
        | "unsupported_grant_type" => CliError::ValidationError(diagnostic),
        "temporarily_unavailable" => CliError::RateLimited(diagnostic),
        "server_error" => CliError::Unknown(diagnostic),
        _ => CliError::AuthFailed(diagnostic),
    }
}

fn oauth_failure_diagnostic(failure: &OAuthFailure) -> String {
    let request = failure
        .request_id
        .as_deref()
        .map(|value| format!(" (RequestId: {value})"))
        .unwrap_or_default();
    format!(
        "HTTP {} [{}] OAuth authorization failed{}",
        failure.status, failure.code, request
    )
}

fn invalid_oauth_response(message: &str) -> CliError {
    CliError::ValidationError(format!("invalid OAuth response: {message}"))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    use tos_core::agent::global_args::GlobalArgs;
    use tos_core::infra::config::ConfigFile;
    use tos_core::infra::credentials::CredentialsFile;

    use super::{
        handle_auth_command, login_instruction_lines, reject_matching_resource_origin,
        validate_device_authorization, validated_login, LoginSettings, PollingSchedule,
    };
    use crate::cli::auth::{AuthAction, AuthCommand, LoginArgs};
    use crate::cli::ADriveAuthArgs;
    use crate::domain::auth::AuthMode;
    use crate::domain::client::ClientOptions;
    use crate::domain::oauth::OAuthClient;
    use crate::domain::oauth::{DeviceAuthorizationResponse, TokenResponse};

    #[test]
    fn bare_resource_endpoint_is_normalized_for_origin_validation() {
        let auth_client = OAuthClient::new(
            "https://idsauth.volces.com".to_string(),
            ClientOptions::default(),
        )
        .expect("auth endpoint should be valid");

        let error = reject_matching_resource_origin(&auth_client, Some("idsauth.volces.com"))
            .expect_err("bare resource endpoint should resolve to the same HTTPS origin");

        assert!(error.to_string().contains("must use different origins"));
    }

    #[test]
    fn polling_starts_after_server_interval_and_slow_down_only_increases_it() {
        let mut schedule = PollingSchedule::new(Duration::from_secs(5))
            .expect("positive interval should be accepted");

        assert_eq!(schedule.interval(), Duration::from_secs(5));
        schedule.apply_slow_down(Some(Duration::from_secs(10)));
        assert_eq!(schedule.interval(), Duration::from_secs(10));
        schedule.apply_slow_down(Some(Duration::from_secs(6)));
        assert_eq!(schedule.interval(), Duration::from_secs(15));
    }

    #[test]
    fn polling_rejects_zero_interval() {
        assert!(PollingSchedule::new(Duration::ZERO).is_err());
    }

    #[test]
    fn temporary_retry_hint_never_reduces_polling_interval() {
        let mut schedule = PollingSchedule::new(Duration::from_secs(5)).unwrap();

        schedule.apply_retry_hint(Some(Duration::from_secs(9)));
        assert_eq!(schedule.interval(), Duration::from_secs(9));
        schedule.apply_retry_hint(Some(Duration::from_secs(2)));
        assert_eq!(schedule.interval(), Duration::from_secs(9));
    }

    #[test]
    fn device_authorization_rejects_unsafe_verification_url() {
        let response = DeviceAuthorizationResponse {
            device_code: "device-code".to_string(),
            user_code: "user-code".to_string(),
            verification_uri: "file:///tmp/fake-login".to_string(),
            verification_uri_complete: "file:///tmp/fake-login#user-code".to_string(),
            expires_in: 60,
            interval: 1,
        };

        assert!(validate_device_authorization(response).is_err());
    }

    #[test]
    fn login_instructions_show_only_the_complete_url() {
        let authorization = DeviceAuthorizationResponse {
            device_code: "device-code".to_string(),
            user_code: "user-code-must-not-be-printed".to_string(),
            verification_uri: "https://auth.example.com/device".to_string(),
            verification_uri_complete: "https://auth.example.com/device?code=complete".to_string(),
            expires_in: 60,
            interval: 1,
        };

        let lines = login_instruction_lines(&authorization);

        assert_eq!(
            lines,
            vec![
                "Open this URL to authorize ve-adrive-cli:".to_string(),
                authorization.verification_uri_complete,
                "Waiting for authorization…".to_string(),
            ]
        );
        assert!(!lines.join("\n").contains("user-code-must-not-be-printed"));
    }

    #[test]
    fn device_authorization_rejects_control_characters_in_user_code() {
        let response = DeviceAuthorizationResponse {
            device_code: "device-code".to_string(),
            user_code: "CODE\u{1b}[31m".to_string(),
            verification_uri: "https://login.example.com/device".to_string(),
            verification_uri_complete: "https://login.example.com/device#CODE".to_string(),
            expires_in: 600,
            interval: 5,
        };

        assert!(validate_device_authorization(response).is_err());
    }

    #[test]
    fn device_name_rejects_control_characters_and_more_than_sixty_four_characters() {
        assert!(super::validate_device_name("bad\ndevice".to_string()).is_err());
        assert!(super::validate_device_name("x".repeat(65)).is_err());
        assert_eq!(
            super::validate_device_name("workstation".to_string()).unwrap(),
            "workstation"
        );
    }

    #[test]
    fn login_settings_do_not_decrypt_unselected_aksk_fields() {
        let directory = test_directory();
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        std::fs::write(
            &config_path,
            "[default.adrive]\nauth_mode = \"oauth\"\naccess_key_id = \"ENC:not-valid\"\ndefault_instance = \"inst-1\"\n",
        )
        .unwrap();
        let global = GlobalArgs {
            config_path: Some(config_path),
            ..GlobalArgs::default()
        };

        let result = super::resolve_login_settings(
            &global,
            &LoginArgs {
                instance: None,
                auth_endpoint: Some("https://auth.example.com".to_string()),
                device_name: Some("test-device".to_string()),
            },
        );

        assert!(result.is_ok());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn login_auth_endpoint_requires_an_explicit_source() {
        let error = super::resolve_login_auth_endpoint_value(None, None, None).unwrap_err();

        assert!(error.to_string().contains("oauth_auth_endpoint_required"));
        assert!(error.to_string().contains("ADRIVE_AUTH_ENDPOINT"));
    }

    #[test]
    fn login_auth_endpoint_keeps_cli_config_environment_precedence() {
        assert_eq!(
            super::resolve_login_auth_endpoint_value(
                Some("https://cli.example.com".to_string()),
                Some("https://config.example.com".to_string()),
                Some("https://env.example.com".to_string()),
            )
            .unwrap(),
            "https://cli.example.com"
        );
    }

    #[test]
    fn validated_login_discards_blank_user_id() {
        let settings = LoginSettings {
            client_id: "client-1".to_string(),
            instance_id: "inst-1".to_string(),
            device_name: "test-device".to_string(),
            resource_endpoint: None,
            client_options: ClientOptions::default(),
        };
        let endpoint = reqwest::Url::parse("https://auth.example.com").unwrap();
        let token = TokenResponse {
            access_token: "access-1".to_string(),
            refresh_token: "refresh-1".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            scope: "all".to_string(),
            user_id: Some("   ".to_string()),
            instance_id: "inst-1".to_string(),
        };

        let (stored, saved) = validated_login(&settings, &endpoint, token).unwrap();

        assert_eq!(saved.user_id, None);
        assert_eq!(stored.user_id, None);
    }

    #[test]
    fn validated_login_trims_user_id() {
        let settings = LoginSettings {
            client_id: "client-1".to_string(),
            instance_id: "inst-1".to_string(),
            device_name: "test-device".to_string(),
            resource_endpoint: None,
            client_options: ClientOptions::default(),
        };
        let endpoint = reqwest::Url::parse("https://auth.example.com").unwrap();
        let token = TokenResponse {
            access_token: "access-1".to_string(),
            refresh_token: "refresh-1".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            scope: "all".to_string(),
            user_id: Some("  user-1  ".to_string()),
            instance_id: "inst-1".to_string(),
        };

        let (stored, saved) = validated_login(&settings, &endpoint, token).unwrap();

        assert_eq!(saved.user_id.as_deref(), Some("user-1"));
        assert_eq!(stored.user_id.as_deref(), Some("user-1"));
    }

    #[tokio::test]
    async fn login_polls_and_persists_complete_token_pair() {
        let directory = test_directory();
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        let credentials_path = directory.join("credentials.toml");
        ConfigFile::default()
            .save_to(&directory, &config_path)
            .unwrap();
        let (endpoint, server) = serve_successful_login();
        let global = GlobalArgs {
            config_path: Some(config_path),
            credentials_path: Some(credentials_path.clone()),
            quiet: true,
            ..GlobalArgs::default()
        };
        let command = AuthCommand {
            action: Some(AuthAction::Login(LoginArgs {
                instance: Some("inst-1".to_string()),
                auth_endpoint: Some(endpoint),
                device_name: Some("test-device".to_string()),
            })),
        };

        let exit_code = handle_auth_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Oauth),
            },
            &command,
        )
        .await
        .unwrap();

        assert_eq!(exit_code, 0);
        let credentials = CredentialsFile::load_from(&credentials_path).unwrap();
        let oauth = credentials
            .adrive_oauth("default", &credentials_path)
            .unwrap();
        assert_eq!(oauth.access_token.as_deref(), Some("access-1"));
        assert_eq!(oauth.refresh_token.as_deref(), Some("refresh-1"));
        assert_eq!(oauth.instance_id.as_deref(), Some("inst-1"));
        assert_eq!(oauth.user_id.as_deref(), Some("user-1"));
        assert!(oauth.expires_at.is_some());
        let raw_credentials = std::fs::read_to_string(&credentials_path).unwrap();
        assert!(!raw_credentials.contains("client_id"));
        server.join().expect("mock OAuth server should finish");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn unified_login_stops_before_oauth_validation_or_credential_writes() {
        let directory = test_directory();
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        let credentials_path = directory.join("credentials.toml");
        std::fs::write(&config_path, "malformed config sentinel = [").unwrap();
        std::fs::write(&credentials_path, "malformed credentials sentinel").unwrap();
        let global = GlobalArgs {
            profile: "selected".to_string(),
            config_path: Some(config_path.clone()),
            credentials_path: Some(credentials_path.clone()),
            dry_run: true,
            ..GlobalArgs::default()
        };
        let command = AuthCommand {
            action: Some(AuthAction::Login(LoginArgs {
                instance: None,
                auth_endpoint: None,
                device_name: None,
            })),
        };

        let error = handle_auth_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Unified),
            },
            &command,
        )
        .await
        .expect_err("Unified login must be managed by the external framework");

        assert!(error
            .to_string()
            .contains("[unified_login_managed_externally]"));
        assert!(error.to_string().contains("ve login"));
        assert!(error.to_string().contains("selected"));
        assert_eq!(
            std::fs::read_to_string(&credentials_path).unwrap(),
            "malformed credentials sentinel"
        );
        assert_eq!(
            std::fs::read_to_string(&config_path).unwrap(),
            "malformed config sentinel = ["
        );
        assert!(!directory.join(".key").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn auth_describe_does_not_resolve_or_read_credentials() {
        let directory = test_directory();
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        let credentials_path = directory.join("credentials.toml");
        std::fs::write(&config_path, "malformed config sentinel = [").unwrap();
        std::fs::write(&credentials_path, "malformed credentials sentinel").unwrap();
        let global = GlobalArgs {
            profile: String::new(),
            config_path: Some(config_path),
            credentials_path: Some(credentials_path.clone()),
            describe: true,
            ..GlobalArgs::default()
        };
        let command = AuthCommand {
            action: Some(AuthAction::Login(LoginArgs {
                instance: None,
                auth_endpoint: None,
                device_name: None,
            })),
        };

        let exit_code = handle_auth_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Unified),
            },
            &command,
        )
        .await
        .expect("describe must return metadata before auth resolution");

        assert_eq!(exit_code, 0);
        assert_eq!(
            std::fs::read_to_string(&credentials_path).unwrap(),
            "malformed credentials sentinel"
        );
        assert!(!directory.join(".key").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn unified_logout_does_not_read_or_modify_local_credentials() {
        let directory = test_directory();
        std::fs::create_dir_all(&directory).unwrap();
        let credentials_path = directory.join("credentials.toml");
        std::fs::write(&credentials_path, "malformed credentials sentinel").unwrap();
        let global = GlobalArgs {
            profile: "selected".to_string(),
            credentials_path: Some(credentials_path.clone()),
            ..GlobalArgs::default()
        };
        let command = AuthCommand {
            action: Some(AuthAction::Logout),
        };

        let error = handle_auth_command(
            &global,
            &ADriveAuthArgs {
                auth_mode: Some(AuthMode::Unified),
            },
            &command,
        )
        .await
        .expect_err("Unified logout must leave external login state unchanged");

        assert!(error
            .to_string()
            .contains("[unified_logout_managed_externally]"));
        assert!(error.to_string().contains("ve logout"));
        assert!(!error.to_string().contains("ve login"));
        assert!(error.to_string().contains("selected"));
        assert_eq!(
            std::fs::read_to_string(&credentials_path).unwrap(),
            "malformed credentials sentinel"
        );
        assert!(!directory.join(".key").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn unified_status_payload_exposes_only_approved_metadata() {
        let global = GlobalArgs {
            profile: "selected".to_string(),
            ..GlobalArgs::default()
        };
        let resolved = crate::domain::auth::ResolvedAuthMode {
            mode: AuthMode::Unified,
            source: crate::domain::auth::AuthModeSource::Config,
        };
        let inspection = crate::handler::common::UnifiedCredentialInspection {
            provider_name: Some("safe-provider".to_string()),
            has_session_token: true,
            ready: true,
            sdk_code: None,
        };

        let payload = super::unified_status_payload(&global, resolved, &inspection);
        let keys = payload
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "has_session_token",
                "mode",
                "profile",
                "provider_name",
                "ready",
                "source",
            ])
        );
        let serialized = serde_json::to_string(&payload).unwrap();
        for secret in ["SECRET_AK", "SECRET_SK", "SECRET_SESSION_TOKEN"] {
            assert!(!serialized.contains(secret));
        }
    }

    fn test_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "ve-adrive-oauth-login-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ))
    }

    fn serve_successful_login() -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let authorize = accept_and_respond(
                &listener,
                r#"{"device_code":"device-1","user_code":"CODE-1","verification_uri":"https://login.example.com/device","verification_uri_complete":"https://login.example.com/device#CODE-1","expires_in":10,"interval":1}"#,
            );
            assert!(authorize.starts_with("POST /v1/oauth/device_authorization "));
            let token = accept_and_respond(
                &listener,
                r#"{"access_token":"access-1","refresh_token":"refresh-1","token_type":"Bearer","expires_in":3600,"scope":"all","user_id":"user-1","instance_id":"inst-1"}"#,
            );
            assert!(token.starts_with("POST /v1/oauth/token "));
        });
        (format!("http://{address}"), server)
    }

    fn accept_and_respond(listener: &TcpListener, response_body: &str) -> String {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(
                count > 0,
                "OAuth request ended before its body was complete"
            );
            bytes.extend_from_slice(&buffer[..count]);
            let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end + 4]);
            let content_length = headers.lines().find_map(parse_content_length).unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                break;
            }
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
            response_body.len()
        )
        .unwrap();
        String::from_utf8_lossy(&bytes).to_string()
    }

    fn parse_content_length(line: &str) -> Option<usize> {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    }
}
