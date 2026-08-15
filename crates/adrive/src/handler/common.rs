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

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
#[cfg(test)]
use std::sync::Arc;
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::{AgentErrorCategory, CliError};
use tos_core::agent::global_args::GlobalArgs;
use tos_core::agent::output::{format_markdown, format_table, format_xml, OutputFormat};
use tos_core::infra::config::{Binary, ConfigFile, FieldSource, Profile};
use tos_core::infra::credentials::{CredentialSection, CredentialsFile};
use tos_core::infra::unified_credentials::UnifiedCredentialProvider;

use crate::domain::auth::{
    AkskAuthProvider, AuthMode, AuthModeSource, AuthProvider, CredentialAvailability,
    OAuthAuthProvider, OAuthCredentials, ResolvedAuthMode, UnifiedAuthProvider,
};
use crate::domain::client::{Client as IdsClient, ClientOptions, Error as IdsError};
use crate::domain::token_manager::{OAuthTokenManager, ACCESS_TOKEN_REFRESH_WINDOW_SECONDS};

/// Build the effective runtime profile for ADrive commands.
///
/// Priority order (for every field, including credentials):
/// CLI flags > config file > environment variables > derived.
///
/// ADrive-specific rule: it never inherits shared-level (`[profile]`)
/// network settings or credentials from the config file, because those belong
/// to TOS. Only `[profile.adrive]` overrides, `ADRIVE_*` environment variables,
/// or explicit CLI flags provide ADrive runtime settings.
pub(crate) fn build_profile(global: &GlobalArgs) -> Result<Profile, CliError> {
    if global.profile.is_empty() {
        // [Review Fix #23] Runtime commands must not silently use env/default
        // ADrive credentials when the selected profile name is empty.
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }

    let config_path = global.existing_runtime_config_path()?;
    let config_dir = ConfigFile::config_dir_from_path(&config_path);
    let config = ConfigFile::load_from(&config_path)?;
    let credentials_path = global.existing_runtime_credentials_path()?;
    let credentials = CredentialsFile::load_from(&credentials_path)?;
    let stored_credentials = credentials.effective_aksk(
        &global.profile,
        CredentialSection::ADrive,
        &credentials_path,
    )?;
    let mut config_profile = if config.profiles.is_empty() && global.profile == "default" {
        Profile::default()
    } else {
        let effective =
            match config.get_effective_profile_in_dir(&global.profile, Binary::Adrive, &config_dir)
            {
                Ok(effective) => effective,
                Err(CliError::ConfigMissing(_)) if !stored_credentials.is_empty() => {
                    let mut credentials_only_config = ConfigFile::default();
                    credentials_only_config.get_or_insert_profile(&global.profile);
                    credentials_only_config.get_effective_profile_in_dir(
                        &global.profile,
                        Binary::Adrive,
                        &config_dir,
                    )?
                }
                Err(error) => return Err(error),
            };
        let mut flat = effective.into_flat_profile();
        // [Review Fix #6] ADrive must not inherit shared TOS network settings.
        if effective.region.source == FieldSource::Shared {
            flat.region = None;
        }
        if effective.endpoint.source == FieldSource::Shared {
            flat.endpoint = None;
        }
        if effective.control_endpoint.source == FieldSource::Shared {
            flat.control_endpoint = None;
        }
        // ADrive must not inherit shared-level credentials (those belong to TOS).
        if effective.access_key_id.source == FieldSource::Shared {
            flat.access_key_id = None;
        }
        if effective.secret_access_key.source == FieldSource::Shared {
            flat.secret_access_key = None;
        }
        if effective.security_token.source == FieldSource::Shared {
            flat.security_token = None;
        }
        flat
    };
    stored_credentials.apply_to_profile(&mut config_profile);

    let env_profile = adrive_environment_profile(true);
    let cli_profile = adrive_cli_profile(global);
    // Priority: CLI > Config > Env (applies uniformly to all fields).
    Ok(env_profile.merge(&config_profile).merge(&cli_profile))
}

fn adrive_environment_profile(include_credentials: bool) -> Profile {
    Profile {
        region: std::env::var("ADRIVE_REGION").ok(),
        access_key_id: include_credentials
            .then(|| std::env::var("ADRIVE_ACCESS_KEY").ok())
            .flatten(),
        secret_access_key: include_credentials
            .then(|| std::env::var("ADRIVE_SECRET_KEY").ok())
            .flatten(),
        security_token: include_credentials
            .then(|| std::env::var("ADRIVE_SECURITY_TOKEN").ok())
            .flatten(),
        endpoint: std::env::var("ADRIVE_ENDPOINT").ok(),
        psm: None,
        idc: None,
        cluster: None,
        addr_family: None,
        control_endpoint: None,
        account_id: std::env::var("ADRIVE_ACCOUNT_ID").ok(),
        checkpoint_dir: None,
        batch_report_dir: None,
        batch_report_format: None,
        progress_enabled: None,
        checkpoint_threshold: std::env::var("ADRIVE_CHECKPOINT_THRESHOLD").ok(),
        batch_concurrency: env_var_any(&["ADRIVE_BATCH_CONCURRENCY"])
            .and_then(|value| parse_positive_usize_env(&value)),
        list_concurrency: env_var_any(&["ADRIVE_LIST_CONCURRENCY"])
            .and_then(|value| parse_positive_usize_env(&value)),
        multipart_concurrency: env_var_any(&["ADRIVE_MULTIPART_CONCURRENCY"])
            .and_then(|value| parse_positive_usize_env(&value)),
        progress_granularity: std::env::var("ADRIVE_PROGRESS_GRANULARITY").ok(),
        overwrite_strategy: std::env::var("ADRIVE_OVERWRITE_STRATEGY").ok(),
        max_retry_count: env_var_any(&["ADRIVE_MAX_RETRY_COUNT"])
            .and_then(|value| parse_u32_env(&value)),
        requesttimeout: env_var_any(&["ADRIVE_REQUESTTIMEOUT", "ADRIVE_REQUEST_TIMEOUT"])
            .and_then(|value| parse_positive_u64_env(&value)),
        connecttimeout: env_var_any(&["ADRIVE_CONNECTTIMEOUT", "ADRIVE_CONNECT_TIMEOUT"])
            .and_then(|value| parse_positive_u64_env(&value)),
        maxconnections: env_var_any(&["ADRIVE_MAXCONNECTIONS", "ADRIVE_MAX_CONNECTIONS"])
            .and_then(|value| parse_positive_usize_env(&value)),
        tos: None,
        ve_tos: None,
        tosvector: None,
        tostable: None,
        adrive: None,
    }
}

fn adrive_cli_profile(global: &GlobalArgs) -> Profile {
    // global.region / global.endpoint / global.account_id are now pure CLI flags
    // (their TOS_* env bindings were removed), so they are safe to use here.
    Profile {
        region: global.region.clone(),
        access_key_id: None,
        secret_access_key: None,
        security_token: None,
        endpoint: global.endpoint.clone(),
        psm: None,
        idc: None,
        cluster: None,
        addr_family: None,
        control_endpoint: None,
        account_id: global.account_id.clone(),
        checkpoint_dir: None,
        batch_report_dir: None,
        batch_report_format: None,
        progress_enabled: None,
        checkpoint_threshold: None,
        batch_concurrency: None,
        list_concurrency: None,
        multipart_concurrency: None,
        progress_granularity: None,
        overwrite_strategy: None,
        max_retry_count: None,
        requesttimeout: None,
        connecttimeout: None,
        maxconnections: None,
        tos: None,
        ve_tos: None,
        tosvector: None,
        tostable: None,
        adrive: None,
    }
}

/// Resolve the effective ADrive auth mode without changing any credential state.
pub(crate) fn resolve_auth_mode(
    global: &GlobalArgs,
    command_line_mode: Option<AuthMode>,
) -> Result<ResolvedAuthMode, CliError> {
    // [Review Fix #3] Reject an empty selected profile before honoring the CLI
    // mode. The Unified SDK treats an empty name as a fallback request, which
    // would inspect a different identity than the one selected by this CLI.
    if global.profile.is_empty() {
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }
    if let Some(mode) = command_line_mode {
        return Ok(ResolvedAuthMode {
            mode,
            source: AuthModeSource::CommandLine,
        });
    }
    let config_path = global.existing_runtime_config_path()?;
    let config = ConfigFile::load_from(&config_path)?;
    resolve_auth_mode_from_config(global, None, &config)
}

fn resolve_auth_mode_from_config(
    global: &GlobalArgs,
    command_line_mode: Option<AuthMode>,
    config: &ConfigFile,
) -> Result<ResolvedAuthMode, CliError> {
    if let Some(mode) = command_line_mode {
        return Ok(ResolvedAuthMode {
            mode,
            source: AuthModeSource::CommandLine,
        });
    }
    if let Some(profile) = config.profiles.get(&global.profile) {
        // [Review Fix #5] Auth Mode is non-secret ADrive metadata. Read it
        // directly so selecting OAuth never decrypts unselected AK/SK fields.
        if let Some(value) = profile
            .adrive
            .as_ref()
            .and_then(|settings| settings.auth_mode.as_deref())
        {
            return Ok(ResolvedAuthMode {
                mode: AuthMode::parse(value, "profile config")?,
                source: AuthModeSource::Config,
            });
        }
    }
    if let Ok(value) = std::env::var("ADRIVE_AUTH_MODE") {
        return Ok(ResolvedAuthMode {
            mode: AuthMode::parse(&value, "ADRIVE_AUTH_MODE")?,
            source: AuthModeSource::Environment,
        });
    }

    Ok(ResolvedAuthMode {
        mode: AuthMode::Aksk,
        source: AuthModeSource::CompatibilityDefault,
    })
}

/// Resolve credentials strictly within the selected ADrive auth mode.
pub(crate) fn build_auth_provider(
    global: &GlobalArgs,
    command_line_mode: Option<AuthMode>,
) -> Result<AuthProvider, CliError> {
    if global.profile.is_empty() {
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }
    let config_path = global.existing_runtime_config_path()?;
    let config = ConfigFile::load_from(&config_path)?;
    let resolved = resolve_auth_mode_from_config(global, command_line_mode, &config)?;
    match resolved.mode {
        AuthMode::Oauth => return Ok(AuthProvider::OAuth(build_oauth_auth_provider(global)?)),
        AuthMode::Unified => {
            let profile = build_unified_profile(global, &config_path, &config)?;
            let client_options = client_options_from_profile(&profile);
            return Ok(AuthProvider::Unified(UnifiedAuthProvider {
                credential_provider: UnifiedCredentialProvider::new(global.profile.clone()),
                endpoint: profile.endpoint,
                region: profile.region,
                client_options,
            }));
        }
        AuthMode::Aksk => {}
    }

    let profile = build_profile(global)?;
    let client_options = client_options_from_profile(&profile);
    let access_key = profile
        .access_key_id
        .ok_or_else(|| CliError::ConfigMissing("ADRIVE_ACCESS_KEY is required".to_string()))?;
    let secret_key = profile
        .secret_access_key
        .ok_or_else(|| CliError::ConfigMissing("ADRIVE_SECRET_KEY is required".to_string()))?;

    Ok(AuthProvider::Aksk(AkskAuthProvider {
        access_key,
        secret_key,
        security_token: profile.security_token,
        endpoint: profile.endpoint,
        region: profile.region,
        client_options,
    }))
}

fn build_unified_profile(
    global: &GlobalArgs,
    config_path: &std::path::Path,
    config: &ConfigFile,
) -> Result<Profile, CliError> {
    let config_dir = ConfigFile::config_dir_from_path(config_path);
    let config_profile = if config.profiles.is_empty() && global.profile == "default" {
        Profile::default()
    } else {
        let effective = config.get_effective_profile_without_credentials_in_dir(
            &global.profile,
            Binary::Adrive,
            &config_dir,
        )?;
        let mut flat = effective.into_flat_profile();
        // ADrive resource settings are isolated from shared TOS settings.
        if effective.region.source == FieldSource::Shared {
            flat.region = None;
        }
        if effective.endpoint.source == FieldSource::Shared {
            flat.endpoint = None;
        }
        if effective.control_endpoint.source == FieldSource::Shared {
            flat.control_endpoint = None;
        }
        flat
    };
    Ok(adrive_environment_profile(false)
        .merge(&config_profile)
        .merge(&adrive_cli_profile(global)))
}

fn build_oauth_runtime_profile(
    global: &GlobalArgs,
    config_path: &std::path::Path,
    config: &ConfigFile,
) -> Result<Profile, CliError> {
    let has_config_profile = config.profiles.contains_key(&global.profile);
    let is_implicit_default = config.profiles.is_empty() && global.profile == "default";
    if has_config_profile || is_implicit_default {
        return build_unified_profile(global, config_path, config);
    }

    let credentials_path = global.existing_runtime_credentials_path()?;
    let oauth = CredentialsFile::load_from(&credentials_path)?
        .adrive_oauth(&global.profile, &credentials_path)?;
    if oauth.is_empty() {
        return build_unified_profile(global, config_path, config);
    }

    // [Review Fix #6] OAuth historically accepts a named profile stored only
    // in credentials.toml. Preserve that rule without resolving AK/SK fields;
    // non-secret runtime controls still come from ADrive env and CLI overlays.
    Ok(adrive_environment_profile(false)
        .merge(&Profile::default())
        .merge(&adrive_cli_profile(global)))
}

fn client_options_from_profile(profile: &Profile) -> ClientOptions {
    ClientOptions {
        max_retry_count: profile.max_retry_count,
        requesttimeout: profile.requesttimeout,
        connecttimeout: profile.connecttimeout,
        maxconnections: profile.maxconnections,
    }
}

/// Build runtime controls for a mode that has already been selected.
///
/// Unified and OAuth resolve only their non-AK/SK configuration layers. AK/SK
/// retains the legacy credential-bearing profile path for compatibility.
pub(crate) fn build_runtime_profile(
    global: &GlobalArgs,
    mode: AuthMode,
) -> Result<Profile, CliError> {
    if global.profile.is_empty() {
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }
    if mode == AuthMode::Aksk {
        return build_profile(global);
    }

    let config_path = global.existing_runtime_config_path()?;
    let config = ConfigFile::load_from(&config_path)?;
    match mode {
        AuthMode::Unified => build_unified_profile(global, &config_path, &config),
        AuthMode::Oauth => build_oauth_runtime_profile(global, &config_path, &config),
        AuthMode::Aksk => unreachable!("AK/SK returned before non-secret resolution"),
    }
}

#[cfg(test)]
type TestUnifiedInspectionResolver = dyn Fn() -> Result<tos_core::infra::unified_credentials::UnifiedCredentialValue, CliError>
    + Send
    + Sync
    + 'static;

enum UnifiedInspectionSource {
    Provider(UnifiedCredentialProvider),
    #[cfg(test)]
    Resolver(Arc<TestUnifiedInspectionResolver>),
}

impl UnifiedInspectionSource {
    async fn get(
        &self,
    ) -> Result<tos_core::infra::unified_credentials::UnifiedCredentialValue, CliError> {
        match self {
            Self::Provider(provider) => provider.get().await,
            #[cfg(test)]
            Self::Resolver(resolver) => resolver(),
        }
    }
}

/// Secret-free result of one Unified credential SDK inspection.
pub(crate) struct UnifiedCredentialInspection {
    /// Non-secret SDK provider identifier returned with the credential triple.
    pub(crate) provider_name: Option<String>,
    /// Whether the returned credential triple contains a session token.
    pub(crate) has_session_token: bool,
    /// Whether all fields required for request signing are present.
    pub(crate) ready: bool,
    /// Sanitized SDK error code when credential resolution failed.
    pub(crate) sdk_code: Option<String>,
}

/// Inspect the selected profile's Unified credentials exactly once.
pub(crate) async fn inspect_unified_credentials_for_profile(
    profile_name: &str,
) -> UnifiedCredentialInspection {
    inspect_unified_credentials(UnifiedInspectionSource::Provider(
        UnifiedCredentialProvider::new(profile_name.to_string()),
    ))
    .await
}

async fn inspect_unified_credentials(
    source: UnifiedInspectionSource,
) -> UnifiedCredentialInspection {
    match source.get().await {
        Ok(credentials) => {
            let has_access_key = !credentials.access_key_id.trim().is_empty();
            let has_secret_key = !credentials.secret_access_key.trim().is_empty();
            let has_session_token = !credentials.session_token.trim().is_empty();
            let provider_name = (!credentials.provider_name.trim().is_empty())
                .then(|| credentials.provider_name.trim().to_string());
            UnifiedCredentialInspection {
                provider_name,
                has_session_token,
                // [Review Fix #1] Session tokens are optional for both static
                // and Unified HMAC credentials; AK/SK alone is ready to sign.
                ready: has_access_key && has_secret_key,
                sdk_code: None,
            }
        }
        Err(error) => UnifiedCredentialInspection {
            provider_name: None,
            has_session_token: false,
            ready: false,
            sdk_code: Some(sanitized_unified_sdk_code(&error)),
        },
    }
}

fn sanitized_unified_sdk_code(error: &CliError) -> String {
    let message = error.to_string();
    message
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(code, _)| code)
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 64
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
        .unwrap_or("UnifiedCredentialUnavailable")
        .to_string()
}

/// Authentication, resource client, and non-secret controls resolved for one
/// high-level ADrive invocation.
pub(crate) struct ADriveRuntime {
    /// Effective non-secret resource and transfer settings.
    pub(crate) profile: Profile,
    /// IDS client using the authentication mode selected by this snapshot.
    pub(crate) client: IdsClient,
    /// Authentication mode selected by the same configuration snapshot.
    pub(crate) resolved_auth_mode: ResolvedAuthMode,
}

/// Dry-run authentication snapshot. Unified fields are populated together
/// from one configuration read; legacy modes keep their prior lazy behavior.
pub(crate) struct ADriveDryRunRuntime {
    /// Authentication mode selected for the plan.
    pub(crate) resolved_auth_mode: ResolvedAuthMode,
    /// Unified non-secret controls, when Unified was selected.
    pub(crate) unified_profile: Option<Profile>,
    /// Unified client when its non-secret resource settings are complete.
    pub(crate) unified_client: Option<IdsClient>,
}

/// Resolve the dry-run authentication snapshot without resolving credentials.
pub(crate) fn build_adrive_dry_run_runtime(
    global: &GlobalArgs,
    command_line_mode: Option<AuthMode>,
) -> Result<ADriveDryRunRuntime, CliError> {
    if global.profile.is_empty() {
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }
    let config_path = global.existing_runtime_config_path()?;
    let config = ConfigFile::load_from(&config_path)?;
    let resolved_auth_mode = resolve_auth_mode_from_config(global, command_line_mode, &config)?;
    if resolved_auth_mode.mode != AuthMode::Unified {
        return Ok(ADriveDryRunRuntime {
            resolved_auth_mode,
            unified_profile: None,
            unified_client: None,
        });
    }

    let profile = build_unified_profile(global, &config_path, &config)?;
    let provider = AuthProvider::Unified(UnifiedAuthProvider {
        credential_provider: UnifiedCredentialProvider::new(global.profile.clone()),
        endpoint: profile.endpoint.clone(),
        region: profile.region.clone(),
        client_options: client_options_from_profile(&profile),
    });
    let unified_client = build_ids_client_from_provider(global, provider).ok();
    Ok(ADriveDryRunRuntime {
        resolved_auth_mode,
        unified_profile: Some(profile),
        unified_client,
    })
}

/// Build one high-level ADrive runtime from a single authentication snapshot.
///
/// Unified mode derives both its non-secret profile and client from the one
/// loaded [`ConfigFile`]. Existing AK/SK and OAuth construction remains on its
/// compatibility path.
pub(crate) fn build_adrive_runtime(
    global: &GlobalArgs,
    command_line_mode: Option<AuthMode>,
) -> Result<ADriveRuntime, CliError> {
    if global.profile.is_empty() {
        return Err(CliError::ValidationError(
            "Invalid profile name: profile must not be empty".to_string(),
        ));
    }
    let config_path = global.existing_runtime_config_path()?;
    let config = ConfigFile::load_from(&config_path)?;
    let resolved_auth_mode = resolve_auth_mode_from_config(global, command_line_mode, &config)?;
    // [Review Fix #2] Unified authentication and all high-level controls must
    // be derived from this one snapshot so a concurrent config change cannot
    // switch the invocation to a local secret-bearing mode.
    build_adrive_runtime_from_snapshot(
        global,
        command_line_mode,
        &config_path,
        &config,
        resolved_auth_mode,
    )
}

fn build_adrive_runtime_from_snapshot(
    global: &GlobalArgs,
    command_line_mode: Option<AuthMode>,
    config_path: &std::path::Path,
    config: &ConfigFile,
    resolved_auth_mode: ResolvedAuthMode,
) -> Result<ADriveRuntime, CliError> {
    if resolved_auth_mode.mode == AuthMode::Unified {
        let profile = build_unified_profile(global, config_path, config)?;
        let provider = AuthProvider::Unified(UnifiedAuthProvider {
            credential_provider: UnifiedCredentialProvider::new(global.profile.clone()),
            endpoint: profile.endpoint.clone(),
            region: profile.region.clone(),
            client_options: client_options_from_profile(&profile),
        });
        let client = build_ids_client_from_provider(global, provider)?;
        return Ok(ADriveRuntime {
            profile,
            client,
            resolved_auth_mode,
        });
    }

    // [Review Fix #4] OAuth must carry non-secret runtime controls without
    // loading or decrypting the unselected AK/SK credential family.
    let profile = if resolved_auth_mode.mode == AuthMode::Oauth {
        build_oauth_runtime_profile(global, config_path, config)?
    } else {
        build_profile(global)?
    };
    let client = build_ids_client(global, command_line_mode)?;
    Ok(ADriveRuntime {
        profile,
        client,
        resolved_auth_mode,
    })
}

/// Inspect only the credential family selected for this invocation.
pub(crate) fn inspect_selected_credentials(
    global: &GlobalArgs,
    mode: AuthMode,
) -> Result<CredentialAvailability, CliError> {
    // [Review Fix #6] Strict mode selection also applies to diagnostics: an
    // OAuth check must not parse or depend on legacy AK/SK profile values.
    match mode {
        AuthMode::Oauth => return inspect_oauth_credentials(global),
        AuthMode::Unified => {
            if global.profile.is_empty() {
                return Err(CliError::ValidationError(
                    "Invalid profile name: profile must not be empty".to_string(),
                ));
            }
            return Ok(CredentialAvailability {
                has_access_key: None,
                has_secret_key: None,
                has_security_token: None,
                access_key_source: None,
                secret_key_source: None,
                security_token_source: None,
                has_access_token: None,
                has_refresh_token: None,
                access_token_expiry: None,
                expires_at: None,
                scope: None,
                instance_id: None,
                ready: true,
                oauth_service_integration: "not_applicable",
                credential_source: "unified_sdk",
            });
        }
        AuthMode::Aksk => {}
    }

    let profile = build_profile(global)?;
    let sources = inspect_aksk_credential_sources(global)?;
    let has_access_key = profile.access_key_id.is_some();
    let has_secret_key = profile.secret_access_key.is_some();
    Ok(CredentialAvailability {
        has_access_key: Some(has_access_key),
        has_secret_key: Some(has_secret_key),
        has_security_token: Some(profile.security_token.is_some()),
        access_key_source: Some(sources.access_key),
        secret_key_source: Some(sources.secret_key),
        security_token_source: Some(sources.security_token),
        has_access_token: None,
        has_refresh_token: None,
        access_token_expiry: None,
        expires_at: None,
        scope: None,
        instance_id: None,
        ready: has_access_key && has_secret_key,
        oauth_service_integration: "not_applicable",
        credential_source: sources.aggregate(),
    })
}

#[derive(Clone, Copy)]
struct AkskCredentialSources {
    access_key: &'static str,
    secret_key: &'static str,
    security_token: &'static str,
}

impl AkskCredentialSources {
    fn aggregate(self) -> &'static str {
        let mut present_sources = [self.access_key, self.secret_key, self.security_token]
            .into_iter()
            .filter(|source| *source != "none");
        let Some(first_source) = present_sources.next() else {
            return "none";
        };
        if present_sources.all(|source| source == first_source) {
            first_source
        } else {
            "mixed"
        }
    }
}

fn inspect_aksk_credential_sources(global: &GlobalArgs) -> Result<AkskCredentialSources, CliError> {
    let config_path = global.existing_runtime_config_path()?;
    let config_dir = ConfigFile::config_dir_from_path(&config_path);
    let config = ConfigFile::load_from(&config_path)?;
    let credentials_path = global.existing_runtime_credentials_path()?;
    let stored = CredentialsFile::load_from(&credentials_path)?.effective_aksk(
        &global.profile,
        CredentialSection::ADrive,
        &credentials_path,
    )?;
    let config_fields =
        inspect_config_aksk_fields(&config, global, &config_dir, !stored.is_empty())?;
    Ok(AkskCredentialSources {
        access_key: credential_field_source(
            stored.access_key_id.is_some(),
            config_fields.0,
            "ADRIVE_ACCESS_KEY",
        ),
        secret_key: credential_field_source(
            stored.secret_access_key.is_some(),
            config_fields.1,
            "ADRIVE_SECRET_KEY",
        ),
        security_token: credential_field_source(
            stored.security_token.is_some(),
            config_fields.2,
            "ADRIVE_SECURITY_TOKEN",
        ),
    })
}

fn inspect_config_aksk_fields(
    config: &ConfigFile,
    global: &GlobalArgs,
    config_dir: &std::path::Path,
    has_stored_credentials: bool,
) -> Result<(bool, bool, bool), CliError> {
    if config.profiles.is_empty() && global.profile == "default" {
        return Ok((false, false, false));
    }
    let effective =
        match config.get_effective_profile_in_dir(&global.profile, Binary::Adrive, config_dir) {
            Ok(effective) => effective,
            Err(CliError::ConfigMissing(_)) if has_stored_credentials => {
                return Ok((false, false, false));
            }
            Err(error) => return Err(error),
        };
    Ok((
        effective.access_key_id.value.is_some()
            && effective.access_key_id.source == FieldSource::BinaryOverride,
        effective.secret_access_key.value.is_some()
            && effective.secret_access_key.source == FieldSource::BinaryOverride,
        effective.security_token.value.is_some()
            && effective.security_token.source == FieldSource::BinaryOverride,
    ))
}

fn credential_field_source(
    has_stored_value: bool,
    has_config_value: bool,
    environment_key: &str,
) -> &'static str {
    if has_stored_value {
        "credentials_file"
    } else if has_config_value {
        "config_file"
    // [Review Fix #1] Match the runtime resolver exactly: non-UTF-8 values are
    // ignored by `std::env::var(...).ok()` and must not be reported as active.
    } else if std::env::var(environment_key).is_ok() {
        "environment"
    } else {
        "none"
    }
}

fn inspect_oauth_credentials(global: &GlobalArgs) -> Result<CredentialAvailability, CliError> {
    let credentials_path = global.existing_runtime_credentials_path()?;
    let stored = CredentialsFile::load_from(&credentials_path)?
        .adrive_oauth(&global.profile, &credentials_path)?;
    let has_stored_token = !stored.is_empty();
    let credentials = OAuthCredentials::from_stored(stored.clone());
    let source = if has_stored_token {
        "credentials_file"
    } else if credentials.has_access_token() || credentials.has_refresh_token() {
        "environment"
    } else {
        "none"
    };
    let has_access_token = credentials.has_access_token();
    let has_refresh_token = credentials.has_refresh_token();
    let ready = if has_stored_token {
        stored_oauth_is_ready(&stored, has_access_token, has_refresh_token)
    } else {
        // [Review Fix #6] Process environment credentials are read-only.
        // A Refresh Token alone cannot be rotated or persisted by the CLI.
        has_access_token
    };
    Ok(CredentialAvailability {
        has_access_key: None,
        has_secret_key: None,
        has_security_token: None,
        access_key_source: None,
        secret_key_source: None,
        security_token_source: None,
        has_access_token: Some(has_access_token),
        has_refresh_token: Some(has_refresh_token),
        access_token_expiry: Some(oauth_expiry_state(
            has_access_token,
            has_stored_token
                .then_some(stored.expires_at.as_deref())
                .flatten(),
        )),
        expires_at: has_stored_token.then_some(stored.expires_at).flatten(),
        scope: (has_stored_token && !stored.scope.is_empty()).then_some(stored.scope),
        instance_id: has_stored_token.then_some(stored.instance_id).flatten(),
        ready,
        oauth_service_integration: "enabled",
        credential_source: source,
    })
}

fn stored_oauth_is_ready(
    stored: &tos_core::infra::credentials::StoredOAuthCredentials,
    has_access_token: bool,
    has_refresh_token: bool,
) -> bool {
    let can_refresh = has_refresh_token
        && has_nonempty(&stored.instance_id)
        && has_nonempty(&stored.auth_endpoint);
    if !has_access_token {
        return can_refresh;
    }
    let Some(expires_at) = stored.expires_at.as_deref() else {
        return true;
    };
    let Ok(expiry) = DateTime::parse_from_rfc3339(expires_at) else {
        return false;
    };
    expiry.with_timezone(&Utc)
        > Utc::now() + chrono::Duration::seconds(ACCESS_TOKEN_REFRESH_WINDOW_SECONDS)
        || can_refresh
}

fn has_nonempty(value: &Option<String>) -> bool {
    value
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

fn oauth_expiry_state(has_access_token: bool, expires_at: Option<&str>) -> String {
    if !has_access_token {
        return "missing".to_string();
    }
    let Some(expires_at) = expires_at else {
        return "unknown".to_string();
    };
    match DateTime::parse_from_rfc3339(expires_at) {
        Ok(expiry) if expiry.with_timezone(&Utc) <= Utc::now() => "expired".to_string(),
        Ok(_) => "valid".to_string(),
        Err(_) => "invalid".to_string(),
    }
}

fn build_oauth_auth_provider(global: &GlobalArgs) -> Result<OAuthAuthProvider, CliError> {
    let credentials_path = global.existing_runtime_credentials_path()?;
    let (endpoint, region, client_options) =
        resolve_oauth_runtime_settings(global, &credentials_path)?;
    // [Review Fix #25] Preserve the OAuth remediation order: a command without
    // usable OAuth credentials must direct the user to login after validating
    // profile selection but before the independent Resource Server endpoint.
    if !inspect_oauth_credentials(global)?.ready {
        return Err(CliError::ConfigMissing(
            "[login_required] usable OAuth credentials are required; run ve-adrive auth login"
                .to_string(),
        ));
    }
    let token_manager = OAuthTokenManager::new(
        credentials_path,
        global.profile.clone(),
        client_options.clone(),
    )?;
    Ok(OAuthAuthProvider {
        token_manager,
        endpoint,
        region,
        client_options,
    })
}

fn resolve_oauth_runtime_settings(
    global: &GlobalArgs,
    credentials_path: &std::path::Path,
) -> Result<(Option<String>, Option<String>, ClientOptions), CliError> {
    let config_path = global.existing_runtime_config_path()?;
    let config = ConfigFile::load_from(&config_path)?;
    let stored = CredentialsFile::load_from(credentials_path)?
        .adrive_oauth(&global.profile, credentials_path)?;
    let profile = config.profiles.get(&global.profile);
    if profile.is_none()
        && !((config.profiles.is_empty() && global.profile == "default") || !stored.is_empty())
    {
        return Err(CliError::ConfigMissing(format!(
            "Profile '{}' not found in {}",
            global.profile,
            config_path.display()
        )));
    }
    let adrive = profile.and_then(|profile| profile.adrive.as_ref());
    let endpoint = global
        .endpoint
        .clone()
        .or_else(|| adrive.and_then(|settings| settings.endpoint.clone()))
        .or_else(|| std::env::var("ADRIVE_ENDPOINT").ok());
    let region = global
        .region
        .clone()
        .or_else(|| adrive.and_then(|settings| settings.region.clone()))
        .or_else(|| std::env::var("ADRIVE_REGION").ok());
    let client_options = oauth_client_options(profile, adrive);
    Ok((endpoint, region, client_options))
}

pub(crate) fn oauth_client_options(
    profile: Option<&Profile>,
    adrive: Option<&tos_core::infra::config::AdriveOverride>,
) -> ClientOptions {
    ClientOptions {
        max_retry_count: adrive
            .and_then(|settings| settings.max_retry_count)
            .or_else(|| profile.and_then(|profile| profile.max_retry_count))
            .or_else(|| {
                env_var_any(&["ADRIVE_MAX_RETRY_COUNT"]).and_then(|value| parse_u32_env(&value))
            }),
        requesttimeout: adrive
            .and_then(|settings| settings.requesttimeout)
            .or_else(|| profile.and_then(|profile| profile.requesttimeout))
            .or_else(|| {
                env_var_any(&["ADRIVE_REQUESTTIMEOUT", "ADRIVE_REQUEST_TIMEOUT"])
                    .and_then(|value| parse_positive_u64_env(&value))
            }),
        connecttimeout: adrive
            .and_then(|settings| settings.connecttimeout)
            .or_else(|| profile.and_then(|profile| profile.connecttimeout))
            .or_else(|| {
                env_var_any(&["ADRIVE_CONNECTTIMEOUT", "ADRIVE_CONNECT_TIMEOUT"])
                    .and_then(|value| parse_positive_u64_env(&value))
            }),
        maxconnections: adrive
            .and_then(|settings| settings.maxconnections)
            .or_else(|| profile.and_then(|profile| profile.maxconnections))
            .or_else(|| {
                env_var_any(&["ADRIVE_MAXCONNECTIONS", "ADRIVE_MAX_CONNECTIONS"])
                    .and_then(|value| parse_positive_usize_env(&value))
            }),
    }
}

/// Build a real IDS REST client for ADrive operations.
pub(crate) fn build_ids_client(
    global: &GlobalArgs,
    command_line_mode: Option<AuthMode>,
) -> Result<IdsClient, CliError> {
    let provider = build_auth_provider(global, command_line_mode)?;
    build_ids_client_from_provider(global, provider)
}

fn build_ids_client_from_provider(
    global: &GlobalArgs,
    provider: AuthProvider,
) -> Result<IdsClient, CliError> {
    let client = match provider {
        AuthProvider::Aksk(credentials) => IdsClient::new(
            credentials.access_key,
            credentials.secret_key,
            credentials.security_token,
            credentials.endpoint,
            credentials.region,
            credentials.client_options,
        ),
        AuthProvider::OAuth(credentials) => IdsClient::new_oauth(
            credentials.token_manager,
            credentials.endpoint,
            credentials.region,
            credentials.client_options,
        ),
        AuthProvider::Unified(credentials) => IdsClient::new_unified(
            credentials.credential_provider,
            credentials.endpoint,
            credentials.region,
            credentials.client_options,
        ),
    };
    client
        .map(|client| client.with_request_trace(std::sync::Arc::clone(&global.request_trace)))
        .map_err(|err| match err {
            IdsError::Cli(error) | IdsError::UnifiedCredential(error) => error,
            IdsError::Client(message)
                if message.contains("ADRIVE_ENDPOINT") || message.contains("ADRIVE_REGION") =>
            {
                // [Review Fix #1] Keep resource setup distinct from a missing
                // OAuth login so the error wrapper can recommend the right command.
                CliError::ConfigMissing(format!("[missing_resource_config] {message}"))
            }
            other => CliError::Unknown(format!("failed to build IDS client: {other}")),
        })
}

pub(crate) fn map_ids_error(err: IdsError) -> CliError {
    let err = match err {
        IdsError::Cli(error) | IdsError::UnifiedCredential(error) => return error,
        other => other,
    };
    match &err {
        IdsError::Server(server) => match server.status_code {
            Some(401) => CliError::AuthFailed(err.to_string()),
            Some(403) => CliError::PermissionDenied(err.to_string()),
            Some(404) => CliError::ResourceNotFound(err.to_string()),
            Some(409) | Some(412) => CliError::Conflict(err.to_string()),
            Some(429) | Some(503) => CliError::RateLimited(err.to_string()),
            Some(500..=599) | Some(408) => CliError::TransferFailed(err.to_string()),
            _ => CliError::Unknown(err.to_string()),
        },
        IdsError::Http(_) | IdsError::HttpBody(_) => CliError::TransferFailed(err.to_string()),
        IdsError::Json(_) | IdsError::Client(_) | IdsError::InvalidResponse(_) => {
            CliError::ValidationError(err.to_string())
        }
        IdsError::Cli(_) | IdsError::UnifiedCredential(_) => {
            unreachable!("credential error returned above")
        }
    }
}

/// Actionable remediation fields for one ADrive runtime error.
pub struct AdriveErrorGuidance {
    /// Human-readable action that explains the remediation.
    pub suggested_action: String,
    /// Direct repair command or command template when one is known.
    pub fix_command: Option<String>,
    /// Focused Doctor command for additional diagnosis.
    pub doctor_hint: Option<String>,
}

/// Select mode-aware ADrive remediation without changing configuration or credentials.
pub fn adrive_error_guidance(
    global: &GlobalArgs,
    requested_mode: Option<AuthMode>,
    error: &CliError,
    command_path: &str,
) -> AdriveErrorGuidance {
    let semantics = error.agent_semantics();
    let mode = error_auth_mode(global, requested_mode);
    if mode == Some(AuthMode::Oauth)
        && semantics.code == "oauth_instance_required"
        && command_path == "ve-adrive ls"
    {
        return guidance(
            "Specify the ADrive Instance bound to the OAuth Access Token",
            Some("ve-adrive ls --instance <instance_id>"),
            Some("ve-adrive doctor --check auth"),
        );
    }
    if let Some(guidance) = auth_error_guidance(mode, error, &semantics.code) {
        return guidance;
    }
    match error {
        CliError::AuthFailed(_) => guidance(
            &semantics.suggested_action,
            None,
            Some("ve-adrive doctor --check auth"),
        ),
        CliError::ValidationError(message) if message.contains("raw API execution") => guidance(
            &semantics.suggested_action,
            Some("ve-adrive api <group> <action> --dry-run"),
            None,
        ),
        CliError::ValidationError(_) => guidance(
            &semantics.suggested_action,
            Some(&format!("{command_path} --help")),
            None,
        ),
        CliError::Http(_) | CliError::TransferFailed(_) | CliError::RateLimited(_) => guidance(
            &semantics.suggested_action,
            None,
            Some("ve-adrive doctor --check network --live-network"),
        ),
        _ => guidance(&semantics.suggested_action, None, Some("ve-adrive doctor")),
    }
}

/// Return an ADrive OAuth-specific category without changing the shared TOS classifier.
pub fn adrive_error_category(
    global: &GlobalArgs,
    requested_mode: Option<AuthMode>,
    error: &CliError,
) -> Option<AgentErrorCategory> {
    if error_auth_mode(global, requested_mode) != Some(AuthMode::Oauth) {
        return None;
    }
    let code = error.agent_semantics().code;
    oauth_error_descriptor(&code).map(|descriptor| descriptor.category)
}

fn error_auth_mode(global: &GlobalArgs, requested_mode: Option<AuthMode>) -> Option<AuthMode> {
    requested_mode.or_else(|| {
        resolve_auth_mode(global, None)
            .ok()
            .map(|resolved| resolved.mode)
    })
}

fn auth_error_guidance(
    mode: Option<AuthMode>,
    error: &CliError,
    code: &str,
) -> Option<AdriveErrorGuidance> {
    // [Review Fix #4] Logout is also owned externally, but its repair action
    // must not tell users to create or refresh a login session.
    if mode == Some(AuthMode::Unified) && code == "unified_logout_managed_externally" {
        return Some(guidance(
            "Log out the selected profile through the externally managed Unified login framework",
            Some("ve logout"),
            Some("ve-adrive doctor --check auth"),
        ));
    }
    if mode == Some(AuthMode::Unified)
        && (code == "unified_login_managed_externally"
            || error.to_string().contains("unified login credential"))
    {
        return Some(guidance(
            "Refresh the selected profile's externally managed Unified login",
            Some("ve login"),
            Some("ve-adrive doctor --check auth"),
        ));
    }
    if mode == Some(AuthMode::Oauth) && code == "oauth_user_id_required" {
        // [Review Fix #2] `auth login` requires the target Instance on this CLI surface.
        return Some(guidance(
            "Run ve-adrive auth login --instance <instance_id> to refresh the selected user identity, or pass --owner-id explicitly",
            Some("ve-adrive auth login --instance <instance_id>"),
            Some("ve-adrive doctor --check auth"),
        ));
    }
    if mode == Some(AuthMode::Oauth) && code == "oauth_instance_required" {
        return Some(guidance(
            "Select the ADrive Instance to authorize",
            Some("ve-adrive auth login --instance <instance_id>"),
            Some("ve-adrive doctor --check config"),
        ));
    }
    if mode == Some(AuthMode::Oauth) && code == "oauth_auth_endpoint_required" {
        return Some(guidance(
            "Configure the OAuth Authorization Server before starting login",
            Some("ve-adrive config set auth_endpoint <url>"),
            Some("ve-adrive doctor --check config"),
        ));
    }
    if code == "missing_resource_config" {
        return Some(guidance(
            "Configure the ADrive resource endpoint and a region when it cannot be parsed from that endpoint",
            Some("ve-adrive config set endpoint <endpoint>"),
            Some("ve-adrive doctor --check config"),
        ));
    }
    if mode == Some(AuthMode::Oauth) && oauth_login_will_repair(code) {
        return Some(guidance(
            "Log in to ADrive with OAuth",
            Some("ve-adrive auth login"),
            Some("ve-adrive doctor --check auth"),
        ));
    }
    if mode == Some(AuthMode::Oauth) {
        if let Some(guidance) = oauth_error_guidance(code) {
            return Some(guidance);
        }
    }
    if mode == Some(AuthMode::Aksk) {
        if let Some(guidance) = aksk_error_guidance(error, code) {
            return Some(guidance);
        }
    }
    match (mode, error) {
        (_, CliError::ConfigMissing(_)) => Some(guidance(
            "Initialize the ADrive profile and credentials",
            Some("ve-adrive config init"),
            Some("ve-adrive doctor --check config"),
        )),
        (Some(AuthMode::Aksk), CliError::AuthFailed(_)) => Some(guidance(
            "Reconfigure the ADrive AK/SK credentials",
            Some("ve-adrive config init"),
            Some("ve-adrive doctor --check auth"),
        )),
        _ => None,
    }
}

fn aksk_error_guidance(error: &CliError, code: &str) -> Option<AdriveErrorGuidance> {
    if let CliError::ConfigMissing(message) = error {
        if message.contains("ADRIVE_ACCESS_KEY is required") {
            return Some(guidance(
                "Configure the ADrive Access Key",
                Some("ve-adrive config set access_key_id <access_key_id>"),
                Some("ve-adrive doctor --check auth"),
            ));
        }
        if message.contains("ADRIVE_SECRET_KEY is required") {
            return Some(guidance(
                "Configure the ADrive Secret Key",
                Some("ve-adrive config set secret_access_key <secret_access_key>"),
                Some("ve-adrive doctor --check auth"),
            ));
        }
    }
    match normalize_adrive_error_code(code).as_str() {
        "invalidaccesskeyid" => Some(guidance(
            "Update the ADrive Access Key",
            Some("ve-adrive config set access_key_id <access_key_id>"),
            Some("ve-adrive doctor --check auth"),
        )),
        "invalidsecretkey" | "signaturenotmatch" => Some(guidance(
            "Verify the ADrive AK/SK pair and update the Secret Key",
            Some("ve-adrive config set secret_access_key <secret_access_key>"),
            Some("ve-adrive doctor --check auth"),
        )),
        "invalidsecuritytoken" => Some(guidance(
            "Update or clear the ADrive Security Token; remove ADRIVE_SECURITY_TOKEN when the AK/SK pair does not require one",
            Some("ve-adrive config set security_token <security_token>"),
            Some("ve-adrive doctor --check auth"),
        )),
        "invalidcredential" => Some(guidance(
            "Verify that the configured ADrive Access Key and Secret Key belong to the same credential pair",
            Some("ve-adrive config init"),
            Some("ve-adrive doctor --check auth"),
        )),
        _ => None,
    }
}

fn normalize_adrive_error_code(code: &str) -> String {
    code.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn oauth_error_guidance(code: &str) -> Option<AdriveErrorGuidance> {
    let descriptor = oauth_error_descriptor(code)?;
    Some(AdriveErrorGuidance {
        suggested_action: descriptor.suggested_action.to_string(),
        fix_command: None,
        doctor_hint: Some(descriptor.doctor_hint.to_string()),
    })
}

#[derive(Clone, Copy)]
struct OAuthErrorDescriptor {
    code: &'static str,
    category: AgentErrorCategory,
    suggested_action: &'static str,
    doctor_hint: &'static str,
}

fn oauth_error_descriptor(code: &str) -> Option<OAuthErrorDescriptor> {
    OAUTH_ERROR_DESCRIPTORS
        .iter()
        .copied()
        .find(|descriptor| descriptor.code == code)
}

// [Review Fix #2] Keep OAuth overrides outside the public guidance struct so
// adding ADrive semantics does not break consumers constructing that struct.
const OAUTH_ERROR_DESCRIPTORS: &[OAuthErrorDescriptor] = &[
    OAuthErrorDescriptor {
        code: "invalid_request",
        category: AgentErrorCategory::InvalidParam,
        suggested_action: "Verify the OAuth request, Auth Endpoint, and stored authorization metadata; use the request_id to diagnose a persistent contract error",
        doctor_hint: "ve-adrive doctor --check auth",
    },
    OAuthErrorDescriptor {
        code: "invalid_client",
        category: AgentErrorCategory::AuthError,
        suggested_action: "Verify the OAuth Client registration; test environments must use the same ADRIVE_OAUTH_CLIENT_ID for login and refresh",
        doctor_hint: "ve-adrive doctor --check auth",
    },
    OAuthErrorDescriptor {
        code: "unauthorized_client",
        category: AgentErrorCategory::AuthError,
        suggested_action: "Verify the OAuth application status, grant type, and authorization for the target ADrive Instance",
        doctor_hint: "ve-adrive doctor --check auth",
    },
    OAuthErrorDescriptor {
        code: "invalid_scope",
        category: AgentErrorCategory::InvalidParam,
        // [Review Fix #3] This message must be correct for both login and refresh failures.
        suggested_action: "Verify that scope=all is allowed for this OAuth Client; Refresh requests do not send scope, so a refresh failure indicates a Client/Server contract mismatch",
        doctor_hint: "ve-adrive doctor --check auth",
    },
    OAuthErrorDescriptor {
        code: "access_denied",
        category: AgentErrorCategory::AuthError,
        suggested_action: "Verify that the account can sign in and the OAuth application can access the target ADrive Instance",
        doctor_hint: "ve-adrive doctor --check auth",
    },
    OAuthErrorDescriptor {
        code: "unsupported_response_type",
        category: AgentErrorCategory::InvalidParam,
        suggested_action: "Report an OAuth Client/Server contract mismatch using the request_id; Device Authorization does not use an authorization response type",
        doctor_hint: "ve-adrive doctor --check auth",
    },
    OAuthErrorDescriptor {
        code: "unsupported_grant_type",
        category: AgentErrorCategory::InvalidParam,
        suggested_action: "Verify that the OAuth server supports the Device Authorization or Refresh Token grant type used by ve-adrive",
        doctor_hint: "ve-adrive doctor --check auth",
    },
    OAuthErrorDescriptor {
        code: "temporarily_unavailable",
        category: AgentErrorCategory::Retryable,
        suggested_action: "Retry the command later; the OAuth service is temporarily unavailable",
        doctor_hint: "ve-adrive doctor --check network --live-network",
    },
    OAuthErrorDescriptor {
        code: "server_error",
        category: AgentErrorCategory::Retryable,
        suggested_action: "Retry the command later; use the request_id to diagnose a persistent OAuth service error",
        doctor_hint: "ve-adrive doctor --check network --live-network",
    },
];

fn oauth_login_will_repair(code: &str) -> bool {
    matches!(code, "login_required" | "invalid_grant" | "expired_token")
}

fn guidance(
    suggested_action: &str,
    fix_command: Option<&str>,
    doctor_hint: Option<&str>,
) -> AdriveErrorGuidance {
    AdriveErrorGuidance {
        suggested_action: suggested_action.to_string(),
        fix_command: fix_command.map(ToString::to_string),
        doctor_hint: doctor_hint.map(ToString::to_string),
    }
}

fn env_var_any(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| std::env::var(key).ok())
}

fn parse_u32_env(value: &str) -> Option<u32> {
    value.trim().parse::<u32>().ok()
}

fn parse_positive_u64_env(value: &str) -> Option<u64> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|parsed| *parsed > 0)
}

fn parse_positive_usize_env(value: &str) -> Option<usize> {
    value
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|parsed| *parsed > 0)
}

/// Output a serializable result through the unified Envelope pipeline.
///
/// This is the single entry point for all ADrive command output. It:
/// 1. Auto-wraps in Envelope if not already envelope-shaped
/// 2. Injects an invocation-traced request_id or generated ULID
/// 3. Applies `--query` JMESPath filter if present
/// 4. Routes to the correct output format (json/yaml/xml/table/csv/markdown)
pub(crate) fn output_result<T: Serialize>(global: &GlobalArgs, data: &T) -> Result<(), CliError> {
    output_result_with_columns(global, data, None)
}

/// Output a result with declared table/csv columns through the unified pipeline.
pub(crate) fn output_result_with_columns<T: Serialize>(
    global: &GlobalArgs,
    data: &T,
    columns: Option<&'static [&'static str]>,
) -> Result<(), CliError> {
    let raw = serde_json::to_value(data)?;
    let mut enveloped = ensure_envelope(global, raw);
    publicize_adrive_output_value(&mut enveloped);
    let value = apply_query(global, enveloped)?;
    render_value(global, &value, columns)
}

/// [Review Fix #3] Kept for backward compatibility — routes through the unified pipeline.
pub(crate) fn output_envelope<T: Serialize>(
    global: &GlobalArgs,
    envelope: &Envelope<T>,
) -> Result<(), CliError> {
    output_result(global, envelope)
}

// ─── Envelope Wrapping ──────────────────────────────────────────────────────

/// Detect if the value is already Envelope-shaped, wrap if not, then inject request_id.
fn ensure_envelope(global: &GlobalArgs, value: Value) -> Value {
    let mut value = if is_envelope_shape(&value) {
        value
    } else {
        let command = describe_command(global);
        let envelope = Envelope::success(command, value);
        serde_json::to_value(envelope).unwrap_or(Value::Null)
    };
    tos_core::agent::request_id::apply_service_trace_to_success_envelope(
        &mut value,
        &global.request_trace.snapshot(),
    );
    inject_request_id(&mut value);
    value
}

fn is_envelope_shape(value: &Value) -> bool {
    matches!(value, Value::Object(map)
        if map.get("status").and_then(Value::as_str).is_some()
            && map.contains_key("command"))
}

fn describe_command(_global: &GlobalArgs) -> String {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(idx) = args.iter().position(|arg| arg.as_str() == "ve-adrive") else {
        return "ve-adrive".to_string();
    };
    let mut tokens = Vec::new();
    for arg in &args[(idx + 1)..] {
        if arg.starts_with('-') {
            break;
        }
        tokens.push(arg.as_str());
    }
    if tokens.is_empty() {
        "ve-adrive".to_string()
    } else {
        format!("ve-adrive {}", tokens.join(" "))
    }
}

/// Convert an ADrive command path into the public top-level command path
/// exposed by the unified CLI.
pub fn public_adrive_command_path(command: &str) -> String {
    // [Review Fix #24] `adrive` is no longer a public or compatibility command
    // prefix; only normalize the supported `ve-adrive` surface.
    if command == "ve-adrive" {
        return "ve-adrive".to_string();
    }
    command
        .strip_prefix("ve-adrive ")
        .map(|suffix| format!("ve-adrive {suffix}"))
        .unwrap_or_else(|| command.to_string())
}

/// Normalize ADrive user-facing JSON output to the supported public command
/// paths agents and users can actually execute.
pub fn publicize_adrive_output_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                publicize_adrive_output_field(key, child);
                publicize_adrive_output_value(child);
            }
        }
        Value::Array(items) => {
            for item in items {
                publicize_adrive_output_value(item);
            }
        }
        Value::String(_) => {}
        _ => {}
    }
}

fn publicize_adrive_output_field(key: &str, value: &mut Value) {
    match (key, value) {
        ("tool", Value::String(tool)) if tool == "ve-adrive" => {
            *tool = "ve-adrive".to_string();
        }
        ("command", Value::String(command)) => {
            *command = public_adrive_command_path(command);
        }
        ("commands", Value::Array(commands)) => {
            for command in commands {
                if let Value::String(command) = command {
                    *command = public_adrive_command_path(command);
                }
            }
        }
        ("lines", Value::Array(lines)) => {
            for line in lines {
                if let Value::String(line) = line {
                    if line == "ve-adrive" {
                        *line = "ve-adrive".to_string();
                    } else if let Some(suffix) = line.strip_prefix("ve-adrive ") {
                        *line = format!("ve-adrive {suffix}");
                    }
                }
            }
        }
        _ => {}
    }
}

// ─── Request ID Injection ───────────────────────────────────────────────────

/// Ensure the Envelope carries a request_id.
/// Priority: existing safe request_id > explicit null > generated ULID.
fn inject_request_id(value: &mut Value) {
    let Value::Object(map) = value else {
        return;
    };
    let needs_id = match map.get("request_id") {
        Some(Value::Null) => false,
        Some(value) => value.as_str().map(str::is_empty).unwrap_or(true),
        None => true,
    };
    if !needs_id {
        return;
    }
    // [Review Fix #5] Never treat the process-wide compatibility mirror as an
    // authoritative source for successful ADrive output.
    map.insert(
        "request_id".to_string(),
        Value::String(ulid::Ulid::new().to_string()),
    );
}

// ─── JMESPath Query ─────────────────────────────────────────────────────────

/// Apply a `--query` JMESPath filter expression.
/// Returns the original value unchanged when no --query is specified.
fn apply_query(global: &GlobalArgs, value: Value) -> Result<Value, CliError> {
    let Some(expr) = global.query.as_deref() else {
        return Ok(value);
    };
    let expr = expr.trim();
    if expr.is_empty() {
        return Ok(value);
    }
    let compiled = jmespath::compile(expr).map_err(|err| {
        CliError::ValidationError(format!("invalid --query expression '{}': {}", expr, err))
    })?;
    let var = jmespath::Variable::from_serializable(&value).map_err(|err| {
        CliError::ValidationError(format!(
            "failed to convert response into JMESPath input: {}",
            err
        ))
    })?;
    let result = compiled.search(var).map_err(|err| {
        CliError::ValidationError(format!("--query evaluation failed for '{}': {}", expr, err))
    })?;
    let json_str = serde_json::to_string(&*result).map_err(CliError::Json)?;
    let value: Value = serde_json::from_str(&json_str).map_err(CliError::Json)?;
    Ok(value)
}

// ─── Render Value ───────────────────────────────────────────────────────────

fn render_value(
    global: &GlobalArgs,
    value: &Value,
    columns: Option<&'static [&'static str]>,
) -> Result<(), CliError> {
    match global.output.unwrap_or_else(OutputFormat::auto_detect) {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).map_err(CliError::Json)?
            );
        }
        OutputFormat::Yaml => {
            println!(
                "{}",
                serde_yaml::to_string(value).map_err(|err| CliError::Unknown(err.to_string()))?
            );
        }
        OutputFormat::Xml => {
            println!("{}", format_xml(value).map_err(CliError::Json)?);
        }
        OutputFormat::Table => {
            let payload = unwrap_envelope_data(value);
            println!("{}", render_table(payload, columns));
            if let Some(footer) = envelope_footer(value) {
                println!("{}", footer);
            }
        }
        OutputFormat::Csv => {
            let payload = unwrap_envelope_data(value);
            println!("{}", render_csv(payload, columns));
        }
        OutputFormat::Markdown => {
            println!("{}", format_markdown(value).map_err(CliError::Json)?);
        }
    }
    Ok(())
}

// ─── Envelope Helpers ───────────────────────────────────────────────────────

fn unwrap_envelope_data(value: &Value) -> &Value {
    if is_envelope_shape(value) {
        match value {
            Value::Object(map) => map.get("data").unwrap_or(value),
            _ => value,
        }
    } else {
        value
    }
}

/// When the Envelope carries a `pagination` field, produce a footer like `Total: N`.
fn envelope_footer(value: &Value) -> Option<String> {
    if !is_envelope_shape(value) {
        return None;
    }
    let map = value.as_object()?;
    let pagination = map.get("pagination")?.as_object()?;
    let total = pagination.get("total_returned").and_then(Value::as_u64);
    let next_token = pagination
        .get("next_token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let next_marker = pagination
        .get("next_marker")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let next_label = next_marker
        .map(|marker| ("next_marker", marker))
        .or_else(|| next_token.map(|token| ("next_token", token)));
    match (total, next_label) {
        (Some(n), Some((label, value))) => Some(format!("Total: {} ({}={})", n, label, value)),
        (Some(n), None) => Some(format!("Total: {}", n)),
        (None, Some((label, value))) => Some(format!("{}={}", label, value)),
        (None, None) => None,
    }
}

// ─── Table / CSV Rendering ──────────────────────────────────────────────────

fn render_table(value: &Value, columns: Option<&'static [&'static str]>) -> String {
    let array = pick_array_payload(value, columns);
    match (array, value) {
        (Some(items), _) if items.iter().all(|item| item.is_object()) => {
            let headers = resolve_headers(items, columns);
            let rows = items
                .iter()
                .map(|item| {
                    let map = item.as_object().expect("checked object");
                    headers
                        .iter()
                        .map(|key| cell_value(map.get(key).unwrap_or(&Value::Null)))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let header_refs = headers.iter().map(String::as_str).collect::<Vec<_>>();
            format_table(&header_refs, &rows)
        }
        (Some(items), _) => {
            let rows = items
                .iter()
                .enumerate()
                .map(|(idx, item)| vec![idx.to_string(), cell_value(item)])
                .collect::<Vec<_>>();
            format_table(&["index", "value"], &rows)
        }
        (None, Value::Object(_)) => {
            // [Review Fix #DescribeTable] Match TOS object-detail rendering so
            // nested describe metadata is visible as field/value rows.
            let rows = flatten_object_to_rows("", value);
            format_table(&["field", "value"], &rows)
        }
        (None, _) => format_table(&["value"], &[vec![cell_value(value)]]),
    }
}

fn render_csv(value: &Value, columns: Option<&'static [&'static str]>) -> String {
    let array = pick_array_payload(value, columns);
    match (array, value) {
        (Some(items), _) if items.iter().all(|item| item.is_object()) => {
            let headers = resolve_headers(items, columns);
            let mut lines = vec![headers.join(",")];
            for item in items {
                if let Value::Object(map) = item {
                    lines.push(
                        headers
                            .iter()
                            .map(|key| {
                                csv_escape(&cell_value(map.get(key).unwrap_or(&Value::Null)))
                            })
                            .collect::<Vec<_>>()
                            .join(","),
                    );
                }
            }
            lines.join("\n")
        }
        (Some(items), _) => {
            let mut lines = vec!["index,value".to_string()];
            for (idx, item) in items.iter().enumerate() {
                lines.push(format!("{},{}", idx, csv_escape(&cell_value(item))));
            }
            lines.join("\n")
        }
        (None, Value::Object(_)) => {
            // [Review Fix #DescribeTable] Keep CSV object details aligned with
            // table output instead of returning one opaque JSON blob.
            let rows = flatten_object_to_rows("", value);
            let mut lines = vec!["field,value".to_string()];
            for row in rows {
                lines.push(format!(
                    "{},{}",
                    csv_escape(row.first().map(String::as_str).unwrap_or_default()),
                    csv_escape(row.get(1).map(String::as_str).unwrap_or_default())
                ));
            }
            lines.join("\n")
        }
        (None, _) => format!("value\n{}", csv_escape(&cell_value(value))),
    }
}

fn pick_array_payload<'a>(
    value: &'a Value,
    columns: Option<&'static [&'static str]>,
) -> Option<&'a Vec<Value>> {
    if let Value::Array(items) = value {
        return Some(items);
    }
    let Value::Object(map) = value else {
        return None;
    };
    if let Some(columns) = columns {
        let mut empty_candidate = None;
        for candidate in map.values().filter_map(Value::as_array) {
            if candidate.iter().any(|item| {
                item.as_object()
                    .map(|obj| columns.iter().any(|column| obj.contains_key(*column)))
                    .unwrap_or(false)
            }) {
                return Some(candidate);
            }
            if candidate.is_empty() && empty_candidate.is_none() {
                empty_candidate = Some(candidate);
            }
        }
        if empty_candidate.is_some() {
            return empty_candidate;
        }
    }
    let mut found = None;
    for candidate in map.values().filter_map(Value::as_array) {
        if found.is_some() {
            return None;
        }
        found = Some(candidate);
    }
    found
}

fn resolve_headers(items: &[Value], columns: Option<&'static [&'static str]>) -> Vec<String> {
    if let Some(columns) = columns {
        return columns.iter().map(|value| value.to_string()).collect();
    }
    let mut headers = Vec::new();
    for item in items {
        if let Some(map) = item.as_object() {
            for key in map.keys() {
                if !headers.contains(key) {
                    headers.push(key.clone());
                }
            }
        }
    }
    headers
}

fn cell_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn flatten_object_to_rows(prefix: &str, value: &Value) -> Vec<Vec<String>> {
    match value {
        Value::Object(map) => {
            let mut rows = Vec::new();
            for (key, val) in map {
                let full_key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{}.{}", prefix, key)
                };
                match val {
                    Value::Object(_) => {
                        rows.extend(flatten_object_to_rows(&full_key, val));
                    }
                    Value::Array(items)
                        if items.iter().all(|item| {
                            item.is_string() || item.is_number() || item.is_boolean()
                        }) =>
                    {
                        let joined = items.iter().map(cell_value).collect::<Vec<_>>().join(", ");
                        rows.push(vec![full_key, joined]);
                    }
                    _ => {
                        rows.push(vec![full_key, cell_value(val)]);
                    }
                }
            }
            rows
        }
        _ => vec![vec![prefix.to_string(), cell_value(value)]],
    }
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tos_core::infra::credentials::StoredOAuthCredentials;

    #[test]
    fn unified_mode_ignores_local_secrets_and_keeps_resource_settings() {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-unified-isolation-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        let credentials_path = directory.join("credentials.toml");
        std::fs::write(
            &config_path,
            r#"[selected.adrive]
auth_mode = "unified"
endpoint = "https://resource.example.com"
region = "cn-test"
access_key_id = "ENC:not-valid"
secret_access_key = "ENC:not-valid"
security_token = "ENC:not-valid"
max_retry_count = 7
checkpoint_threshold = "64MiB"
"#,
        )
        .unwrap();
        std::fs::write(&credentials_path, "not valid toml = [").unwrap();
        let global = GlobalArgs {
            profile: "selected".to_string(),
            config_path: Some(config_path),
            credentials_path: Some(credentials_path),
            ..GlobalArgs::default()
        };

        let AuthProvider::Unified(provider) = build_auth_provider(&global, None).unwrap() else {
            panic!("unified mode must build a Unified provider")
        };
        assert_eq!(
            provider.endpoint.as_deref(),
            Some("https://resource.example.com")
        );
        assert_eq!(provider.region.as_deref(), Some("cn-test"));
        assert_eq!(provider.client_options.max_retry_count, Some(7));
        assert!(!directory.join(".key").exists());

        let client = build_ids_client(&global, None).unwrap();
        assert_eq!(
            client.instance_listing_scope().unwrap(),
            crate::domain::client::InstanceListingScope::All
        );
        assert!(!client.uses_oauth());
        assert_eq!(client.oauth_user_id().unwrap(), None);
        assert!(!directory.join(".key").exists());

        let runtime = build_adrive_runtime(&global, None).unwrap();
        assert_eq!(
            runtime.profile.checkpoint_threshold.as_deref(),
            Some("64MiB")
        );
        assert!(!directory.join(".key").exists());

        let inspection = inspect_selected_credentials(&global, AuthMode::Unified).unwrap();
        assert!(inspection.ready);
        assert_eq!(inspection.credential_source, "unified_sdk");
        assert!(!directory.join(".key").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn unified_runtime_survives_config_removal_after_its_selected_snapshot() {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-unified-single-snapshot-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        std::fs::write(
            &config_path,
            r#"[selected.adrive]
auth_mode = "unified"
endpoint = "https://resource.example.com"
region = "cn-test"
checkpoint_threshold = "32MiB"
access_key_id = "ENC:not-valid"
secret_access_key = "ENC:not-valid"
"#,
        )
        .unwrap();
        let global = GlobalArgs {
            profile: "selected".to_string(),
            config_path: Some(config_path.clone()),
            credentials_path: Some(directory.join("missing-credentials.toml")),
            ..GlobalArgs::default()
        };
        let config = ConfigFile::load_from(&config_path).unwrap();
        let resolved = resolve_auth_mode_from_config(&global, None, &config).unwrap();
        std::fs::remove_file(&config_path).unwrap();

        let runtime =
            build_adrive_runtime_from_snapshot(&global, None, &config_path, &config, resolved)
                .unwrap();

        assert_eq!(runtime.resolved_auth_mode.mode, AuthMode::Unified);
        assert_eq!(
            runtime.profile.checkpoint_threshold.as_deref(),
            Some("32MiB")
        );
        assert_eq!(
            runtime.client.instance_listing_scope().unwrap(),
            crate::domain::client::InstanceListingScope::All
        );
        assert!(!directory.join(".key").exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn oauth_runtime_ignores_unselected_malformed_aksk_credentials() {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-oauth-runtime-isolation-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        let credentials_path = directory.join("credentials.toml");
        std::fs::write(
            &config_path,
            r#"[selected.adrive]
auth_mode = "oauth"
endpoint = "https://resource.example.com"
region = "cn-test"
access_key_id = "ENC:not-valid"
secret_access_key = "ENC:not-valid"
checkpoint_threshold = "32MiB"
"#,
        )
        .unwrap();
        let mut credentials = CredentialsFile::default();
        credentials
            .set_adrive_oauth(
                "selected",
                StoredOAuthCredentials {
                    access_token: Some("oauth-access".to_string()),
                    expires_at: Some("2099-01-01T00:00:00Z".to_string()),
                    auth_endpoint: Some("https://auth.example.com".to_string()),
                    ..StoredOAuthCredentials::default()
                },
            )
            .unwrap();
        credentials.save_to_path(&credentials_path).unwrap();
        let global = GlobalArgs {
            profile: "selected".to_string(),
            config_path: Some(config_path),
            credentials_path: Some(credentials_path),
            ..GlobalArgs::default()
        };

        let runtime = build_adrive_runtime(&global, None)
            .expect("OAuth runtime must ignore unselected malformed AK/SK");

        assert_eq!(runtime.resolved_auth_mode.mode, AuthMode::Oauth);
        assert_eq!(
            runtime.profile.checkpoint_threshold.as_deref(),
            Some("32MiB")
        );
        assert!(runtime.client.uses_oauth());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn explicit_oauth_runtime_accepts_credentials_only_named_profile() {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-oauth-credentials-only-runtime-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        let credentials_path = directory.join("credentials.toml");
        std::fs::write(
            &config_path,
            r#"[unrelated.adrive]
auth_mode = "aksk"
"#,
        )
        .unwrap();
        let mut credentials = CredentialsFile::default();
        credentials
            .set_adrive_oauth(
                "oauth-only",
                StoredOAuthCredentials {
                    access_token: Some("oauth-access".to_string()),
                    expires_at: Some("2099-01-01T00:00:00Z".to_string()),
                    auth_endpoint: Some("https://auth.example.com".to_string()),
                    ..StoredOAuthCredentials::default()
                },
            )
            .unwrap();
        credentials.save_to_path(&credentials_path).unwrap();
        let global = GlobalArgs {
            profile: "oauth-only".to_string(),
            config_path: Some(config_path),
            credentials_path: Some(credentials_path),
            endpoint: Some("https://cli-resource.example.com".to_string()),
            region: Some("cn-cli".to_string()),
            ..GlobalArgs::default()
        };

        let runtime = build_adrive_runtime(&global, Some(AuthMode::Oauth))
            .expect("explicit OAuth must retain credentials-only named profile support");

        assert_eq!(runtime.resolved_auth_mode.mode, AuthMode::Oauth);
        assert_eq!(
            runtime.profile.endpoint.as_deref(),
            Some("https://cli-resource.example.com")
        );
        assert_eq!(runtime.profile.region.as_deref(), Some("cn-cli"));
        assert!(runtime.client.uses_oauth());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn unified_credential_inspection_gets_once_and_projects_only_safe_metadata() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver_calls = Arc::clone(&calls);
        let inspection =
            inspect_unified_credentials(UnifiedInspectionSource::Resolver(Arc::new(move || {
                resolver_calls.fetch_add(1, Ordering::SeqCst);
                Ok(
                    tos_core::infra::unified_credentials::UnifiedCredentialValue::new(
                        "SECRET_AK",
                        "SECRET_SK",
                        "SECRET_SESSION_TOKEN",
                        "safe-provider",
                    ),
                )
            })))
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(inspection.provider_name.as_deref(), Some("safe-provider"));
        assert!(inspection.has_session_token);
        assert!(inspection.ready);
        assert_eq!(inspection.sdk_code, None);
    }

    #[tokio::test]
    async fn unified_credential_inspection_treats_session_token_as_optional() {
        let inspection =
            inspect_unified_credentials(UnifiedInspectionSource::Resolver(Arc::new(|| {
                Ok(
                    tos_core::infra::unified_credentials::UnifiedCredentialValue::new(
                        "SECRET_AK",
                        "SECRET_SK",
                        "",
                        "safe-provider",
                    ),
                )
            })))
            .await;

        assert!(inspection.ready);
        assert!(!inspection.has_session_token);
    }

    #[tokio::test]
    async fn unified_credential_inspection_sanitizes_sdk_failure_without_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver_calls = Arc::clone(&calls);
        let inspection =
            inspect_unified_credentials(UnifiedInspectionSource::Resolver(Arc::new(move || {
                resolver_calls.fetch_add(1, Ordering::SeqCst);
                Err(CliError::AuthFailed(
                    "[CredentialExpired] secret-value must not escape".to_string(),
                ))
            })))
            .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(inspection.provider_name, None);
        assert!(!inspection.has_session_token);
        assert!(!inspection.ready);
        assert_eq!(inspection.sdk_code.as_deref(), Some("CredentialExpired"));
    }

    #[test]
    fn unified_sdk_and_external_login_errors_recommend_ve_login() {
        let global = GlobalArgs::default();
        for error in [
            CliError::ConfigMissing(
                "[CliConfigLoginSessionMissing] unified login credentials are unavailable"
                    .to_string(),
            ),
            CliError::ValidationError(
                "[unified_login_managed_externally] managed externally".to_string(),
            ),
        ] {
            let guidance = adrive_error_guidance(
                &global,
                Some(AuthMode::Unified),
                &error,
                "ve-adrive auth login",
            );
            assert_eq!(guidance.fix_command.as_deref(), Some("ve login"));
            assert_eq!(
                guidance.doctor_hint.as_deref(),
                Some("ve-adrive doctor --check auth")
            );
            assert!(!guidance.suggested_action.contains("AK/SK"));
            assert!(!guidance.suggested_action.contains("config init"));
        }
    }

    #[test]
    fn unified_external_logout_error_recommends_ve_logout() {
        let guidance = adrive_error_guidance(
            &GlobalArgs::default(),
            Some(AuthMode::Unified),
            &CliError::ValidationError(
                "[unified_logout_managed_externally] managed externally".to_string(),
            ),
            "ve-adrive auth logout",
        );

        assert_eq!(guidance.fix_command.as_deref(), Some("ve logout"));
        assert_eq!(
            guidance.doctor_hint.as_deref(),
            Some("ve-adrive doctor --check auth")
        );
    }

    #[test]
    fn ensure_envelope_prefers_invocation_service_request_id() {
        let global = GlobalArgs::default();
        global
            .request_trace
            .record_response(Some("ids-service-id"), true);
        let envelope = json!({
            "success": true,
            "status": "success",
            "command": "ve-adrive ls",
            "request_id": "generated-id",
            "data": {"files": []},
        });

        let out = ensure_envelope(&global, envelope);

        assert_eq!(out["request_id"], "ids-service-id");
    }

    #[test]
    fn oauth_root_ls_missing_instance_points_to_explicit_target() {
        let guidance = adrive_error_guidance(
            &GlobalArgs::default(),
            Some(AuthMode::Oauth),
            &CliError::ConfigMissing(
                "[oauth_instance_required] OAuth credentials have no bound Instance".to_string(),
            ),
            "ve-adrive ls",
        );

        assert_eq!(
            guidance.fix_command.as_deref(),
            Some("ve-adrive ls --instance <instance_id>")
        );
        assert_eq!(
            guidance.doctor_hint.as_deref(),
            Some("ve-adrive doctor --check auth")
        );
    }

    #[test]
    fn oauth_user_id_required_guidance_recommends_login_or_explicit_owner() {
        // [Review Fix #2] Keep the stable envelope code and executable remediation together.
        let error = CliError::ValidationError(
            "[oauth_user_id_required] OAuth Space creation requires --owner-id or ve-adrive auth login --instance <instance_id>"
                .to_string(),
        );
        let semantics = error.agent_semantics();
        let guidance = adrive_error_guidance(
            &GlobalArgs::default(),
            Some(AuthMode::Oauth),
            &error,
            "ve-adrive crt",
        );

        assert_eq!(semantics.code, "oauth_user_id_required");
        assert_eq!(semantics.category, AgentErrorCategory::InvalidParam);
        assert_eq!(
            guidance.fix_command.as_deref(),
            Some("ve-adrive auth login --instance <instance_id>")
        );
        assert_eq!(
            guidance.doctor_hint.as_deref(),
            Some("ve-adrive doctor --check auth")
        );
        assert!(guidance.suggested_action.contains("--owner-id"));
        assert!(guidance
            .suggested_action
            .contains("ve-adrive auth login --instance <instance_id>"));
    }

    #[test]
    fn oauth_login_missing_instance_keeps_login_fix_command() {
        let guidance = adrive_error_guidance(
            &GlobalArgs::default(),
            Some(AuthMode::Oauth),
            &CliError::ConfigMissing(
                "[oauth_instance_required] ADrive OAuth instance is required".to_string(),
            ),
            "ve-adrive auth login",
        );

        assert_eq!(
            guidance.fix_command.as_deref(),
            Some("ve-adrive auth login --instance <instance_id>")
        );
    }

    #[test]
    fn oauth_login_missing_auth_endpoint_recommends_configuring_it() {
        let guidance = adrive_error_guidance(
            &GlobalArgs::default(),
            Some(AuthMode::Oauth),
            &CliError::ConfigMissing(
                "[oauth_auth_endpoint_required] ADrive OAuth auth endpoint is required".to_string(),
            ),
            "ve-adrive auth login",
        );

        assert_eq!(
            guidance.fix_command.as_deref(),
            Some("ve-adrive config set auth_endpoint <url>")
        );
        assert_eq!(
            guidance.doctor_hint.as_deref(),
            Some("ve-adrive doctor --check config")
        );
    }

    #[test]
    fn missing_resource_endpoint_recommends_endpoint_for_both_auth_modes() {
        for mode in [AuthMode::Aksk, AuthMode::Oauth] {
            let guidance = adrive_error_guidance(
                &GlobalArgs::default(),
                Some(mode),
                &CliError::ConfigMissing(
                    "[missing_resource_config] ADRIVE_ENDPOINT is required".to_string(),
                ),
                "ve-adrive ls",
            );

            assert_eq!(
                guidance.fix_command.as_deref(),
                Some("ve-adrive config set endpoint <endpoint>"),
                "mode={mode:?}"
            );
            assert_eq!(
                guidance.doctor_hint.as_deref(),
                Some("ve-adrive doctor --check config")
            );
        }
    }

    #[test]
    fn oauth_invalid_client_guidance_identifies_client_registration() {
        let guidance = adrive_error_guidance(
            &GlobalArgs::default(),
            Some(AuthMode::Oauth),
            &CliError::AuthFailed(
                "HTTP 400 [invalid_client] OAuth Token refresh failed (RequestId: req-1)"
                    .to_string(),
            ),
            "ve-adrive ls",
        );

        assert_eq!(
            adrive_error_category(
                &GlobalArgs::default(),
                Some(AuthMode::Oauth),
                &CliError::AuthFailed(
                    "HTTP 400 [invalid_client] OAuth Token refresh failed".to_string(),
                ),
            ),
            Some(tos_core::agent::error::AgentErrorCategory::AuthError)
        );
        assert!(guidance.suggested_action.contains("ADRIVE_OAUTH_CLIENT_ID"));
        assert!(guidance.fix_command.is_none());
        assert_eq!(
            guidance.doctor_hint.as_deref(),
            Some("ve-adrive doctor --check auth")
        );
    }

    #[test]
    fn oauth_terminal_error_guidance_matches_the_frozen_contract() {
        let cases = [
            (
                "unauthorized_client",
                tos_core::agent::error::AgentErrorCategory::AuthError,
                "application status",
            ),
            (
                "invalid_scope",
                tos_core::agent::error::AgentErrorCategory::InvalidParam,
                "do not send scope",
            ),
            (
                "access_denied",
                tos_core::agent::error::AgentErrorCategory::AuthError,
                "account",
            ),
            (
                "unsupported_grant_type",
                tos_core::agent::error::AgentErrorCategory::InvalidParam,
                "grant type",
            ),
            (
                "server_error",
                tos_core::agent::error::AgentErrorCategory::Retryable,
                "request_id",
            ),
        ];

        for (code, category, action_fragment) in cases {
            let guidance = adrive_error_guidance(
                &GlobalArgs::default(),
                Some(AuthMode::Oauth),
                &CliError::AuthFailed(format!(
                    "HTTP 400 [{code}] OAuth Token refresh failed (RequestId: req-1)"
                )),
                "ve-adrive ls",
            );

            assert_eq!(
                adrive_error_category(
                    &GlobalArgs::default(),
                    Some(AuthMode::Oauth),
                    &CliError::AuthFailed(format!("HTTP 400 [{code}] OAuth Token refresh failed")),
                ),
                Some(category),
                "code={code}"
            );
            assert!(
                guidance.suggested_action.contains(action_fragment),
                "code={code}, action={}",
                guidance.suggested_action
            );
            assert!(guidance.fix_command.is_none(), "code={code}");
        }
    }

    #[test]
    fn aksk_runtime_errors_recommend_the_affected_credential_field() {
        let cases = [
            (
                "InvalidAccessKeyId",
                "Access Key",
                "ve-adrive config set access_key_id <access_key_id>",
            ),
            (
                "SignatureNotMatch",
                "AK/SK pair",
                "ve-adrive config set secret_access_key <secret_access_key>",
            ),
            (
                "InvalidSecurityToken",
                "Security Token",
                "ve-adrive config set security_token <security_token>",
            ),
        ];

        for (code, action_fragment, fix_command) in cases {
            let guidance = adrive_error_guidance(
                &GlobalArgs::default(),
                Some(AuthMode::Aksk),
                &CliError::AuthFailed(format!("HTTP 401 [{code}] request rejected")),
                "ve-adrive ls",
            );

            assert!(
                guidance.suggested_action.contains(action_fragment),
                "code={code}, action={}",
                guidance.suggested_action
            );
            assert_eq!(guidance.fix_command.as_deref(), Some(fix_command));
            assert_eq!(
                guidance.doctor_hint.as_deref(),
                Some("ve-adrive doctor --check auth")
            );
        }
    }

    #[test]
    fn aksk_missing_field_errors_recommend_only_the_missing_field() {
        let cases = [
            (
                "ADRIVE_ACCESS_KEY is required",
                "ve-adrive config set access_key_id <access_key_id>",
            ),
            (
                "ADRIVE_SECRET_KEY is required",
                "ve-adrive config set secret_access_key <secret_access_key>",
            ),
        ];

        for (message, fix_command) in cases {
            let guidance = adrive_error_guidance(
                &GlobalArgs::default(),
                Some(AuthMode::Aksk),
                &CliError::ConfigMissing(message.to_string()),
                "ve-adrive ls",
            );

            assert_eq!(guidance.fix_command.as_deref(), Some(fix_command));
            assert_eq!(
                guidance.doctor_hint.as_deref(),
                Some("ve-adrive doctor --check auth")
            );
        }
    }

    #[test]
    fn aksk_forbidden_auth_code_still_gets_targeted_guidance() {
        let guidance = adrive_error_guidance(
            &GlobalArgs::default(),
            Some(AuthMode::Aksk),
            &CliError::PermissionDenied(
                "HTTP 403 [InvalidAccessKeyId] request rejected".to_string(),
            ),
            "ve-adrive ls",
        );

        assert_eq!(
            guidance.fix_command.as_deref(),
            Some("ve-adrive config set access_key_id <access_key_id>")
        );
    }

    #[test]
    fn persisted_oauth_status_reports_metadata_and_readiness() {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-oauth-status-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let credentials_path = directory.join("credentials.toml");
        let mut credentials = CredentialsFile::default();
        credentials
            .set_adrive_oauth(
                "default",
                StoredOAuthCredentials {
                    access_token: Some("ACCESS_TOKEN_MUST_NOT_LEAK".to_string()),
                    refresh_token: Some("REFRESH_TOKEN_MUST_NOT_LEAK".to_string()),
                    expires_at: Some("2099-01-01T00:00:00Z".to_string()),
                    token_type: Some("Bearer".to_string()),
                    scope: vec!["all".to_string()],
                    legacy_client_id: None,
                    instance_id: Some("inst-1".to_string()),
                    user_id: None,
                    auth_endpoint: Some("https://auth.example.com".to_string()),
                },
            )
            .unwrap();
        credentials.save_to_path(&credentials_path).unwrap();
        let global = GlobalArgs {
            credentials_path: Some(credentials_path),
            ..GlobalArgs::default()
        };

        let status = inspect_selected_credentials(&global, AuthMode::Oauth).unwrap();

        assert_eq!(status.access_token_expiry.as_deref(), Some("valid"));
        assert_eq!(status.scope, Some(vec!["all".to_string()]));
        assert_eq!(status.instance_id.as_deref(), Some("inst-1"));
        assert!(status.ready);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn oauth_mode_resolution_does_not_decrypt_unselected_aksk_fields() {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-auth-mode-isolation-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let config_path = directory.join("config.toml");
        std::fs::write(
            &config_path,
            "[default.adrive]\nauth_mode = \"oauth\"\nsecret_access_key = \"ENC:not-valid\"\n",
        )
        .unwrap();
        let global = GlobalArgs {
            config_path: Some(config_path),
            ..GlobalArgs::default()
        };

        let resolved = resolve_auth_mode(&global, None).unwrap();

        assert_eq!(resolved.mode, AuthMode::Oauth);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn explicit_unified_mode_rejects_empty_profile_before_sdk_fallback() {
        let global = GlobalArgs {
            profile: String::new(),
            ..GlobalArgs::default()
        };

        let error = resolve_auth_mode(&global, Some(AuthMode::Unified))
            .expect_err("an empty CLI profile must never reach the Unified SDK");

        assert!(error.to_string().contains("profile must not be empty"));
    }

    #[test]
    fn table_output_uses_declared_columns_for_empty_payloads() {
        let rendered = render_table(
            &json!({
                "files": [],
                "folders": [],
                "next_marker": "",
                "is_truncated": false,
            }),
            Some(&["file_path", "size", "file_type"]),
        );

        assert!(rendered.contains("file_path"));
        assert!(rendered.contains("file_type"));
        assert!(!rendered.contains("field"));
    }

    #[test]
    fn csv_output_uses_declared_columns_for_empty_payloads() {
        let rendered = render_csv(
            &json!({
                "instances": [],
                "next_marker": "",
                "is_truncated": false,
            }),
            Some(&["instance_id", "name"]),
        );

        assert_eq!(rendered, "instance_id,name");
    }

    #[test]
    fn envelope_footer_prefers_next_marker_for_adrive_pagination() {
        let footer = envelope_footer(&json!({
            "success": true,
            "status": "success",
            "command": "ve-adrive ls",
            "request_id": "req",
            "status_code": null,
            "ec": null,
            "data": {"files": []},
            "pagination": {
                "next_marker": "marker-1",
                "total_returned": 2
            }
        }));

        assert_eq!(footer, Some("Total: 2 (next_marker=marker-1)".to_string()));
    }

    #[test]
    fn publicizer_does_not_translate_legacy_adrive_command_paths() {
        assert_eq!(public_adrive_command_path("ve-adrive ls"), "ve-adrive ls");
        assert_eq!(public_adrive_command_path("adrive ls"), "adrive ls");

        let mut output = json!({
            "tool": "adrive",
            "command": "adrive ls",
            "commands": ["adrive cp", "ve-adrive rm"],
            "lines": ["adrive", "adrive ls", "ve-adrive ls"],
            "message": "run adrive ls for old command data"
        });

        publicize_adrive_output_value(&mut output);

        assert_eq!(output["tool"], "adrive");
        assert_eq!(output["command"], "adrive ls");
        assert_eq!(output["commands"], json!(["adrive cp", "ve-adrive rm"]));
        assert_eq!(
            output["lines"],
            json!(["adrive", "adrive ls", "ve-adrive ls"])
        );
        assert_eq!(output["message"], "run adrive ls for old command data");
    }
}

/// Parse an adrive:// URI into its components.
///
/// Format: `adrive://instance/space/folder_path.../file`
///
/// Returns `(instance, space, path_remainder)` where `path_remainder` is
/// everything after `space/` (could be a folder path, file path, or empty).
#[derive(Debug, Clone)]
pub(crate) struct ParsedADriveUri {
    pub instance: String,
    pub space: String,
    /// The remaining path after instance/space (folder/file or just folder/).
    /// Empty string if only instance/space are present.
    pub path: String,
}

impl ParsedADriveUri {
    /// Extract the file name (last segment, only if not ending with '/').
    pub fn file(&self) -> Option<&str> {
        if self.path.is_empty() || self.path.ends_with('/') {
            return None;
        }
        self.path.rsplit('/').next()
    }
}

/// Parse an `adrive://instance/space[/path...]` URI.
///
/// If `require_space` is true, the URI must contain at least instance and space.
/// If `allow_instance_only` is true, `adrive://instance` is valid.
pub(crate) fn parse_adrive_uri(
    uri: &str,
    allow_instance_only: bool,
) -> Result<ParsedADriveUri, CliError> {
    if !uri.starts_with("adrive://") {
        return Err(CliError::ValidationError(format!(
            "invalid A-Drive URI '{}': expected adrive://instance/space[/path]",
            uri
        )));
    }
    let rest = uri.trim_start_matches("adrive://");
    let parts: Vec<&str> = rest.splitn(3, '/').collect();

    let instance = parts.first().filter(|s| !s.is_empty()).ok_or_else(|| {
        CliError::ValidationError(format!("invalid A-Drive URI '{}': missing instance", uri))
    })?;

    if parts.len() < 2 || parts[1].is_empty() {
        if allow_instance_only {
            return Ok(ParsedADriveUri {
                instance: instance.to_string(),
                space: String::new(),
                path: String::new(),
            });
        }
        return Err(CliError::ValidationError(format!(
            "invalid A-Drive URI '{}': expected adrive://instance/space[/path]",
            uri
        )));
    }

    let space = parts[1];
    let path = if parts.len() > 2 { parts[2] } else { "" };

    Ok(ParsedADriveUri {
        instance: instance.to_string(),
        space: space.to_string(),
        path: path.to_string(),
    })
}

/// Resolve target from either positional URI or explicit flags.
pub(crate) fn resolve_target(
    uri: Option<&str>,
    instance: Option<&str>,
    space: Option<&str>,
    folder: Option<&str>,
    file: Option<&str>,
) -> Result<ParsedADriveUri, CliError> {
    if let Some(uri) = uri {
        return parse_adrive_uri(uri, false);
    }

    let instance = instance.ok_or_else(|| {
        CliError::ValidationError(
            "missing target: provide adrive://instance/space/path or --instance".into(),
        )
    })?;
    let space = space.ok_or_else(|| {
        CliError::ValidationError("missing --space: required with --instance".into())
    })?;

    let path = match (folder, file) {
        (Some(f), Some(name)) => {
            let f = f.trim_end_matches('/');
            format!("{f}/{name}")
        }
        (Some(f), None) => {
            let f = f.trim_end_matches('/');
            format!("{f}/")
        }
        (None, Some(name)) => name.to_string(),
        (None, None) => String::new(),
    };

    Ok(ParsedADriveUri {
        instance: instance.to_string(),
        space: space.to_string(),
        path,
    })
}

/// Block destructive commands unless the caller explicitly confirms.
pub(crate) fn ensure_force_for_destructive(
    global: &GlobalArgs,
    force: bool,
    command: &str,
    target: &str,
) -> Result<(), CliError> {
    let stdin_tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let stderr_tty = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let can_prompt = stdin_tty && stderr_tty && !global.quiet;

    if force {
        if requires_delete_confirm(command) && !can_prompt {
            return ensure_exact_confirm(global, command, target);
        }
        return Ok(());
    }

    if global.yes && can_prompt {
        return Ok(());
    }

    if can_prompt {
        eprint!(
            "⚠ destructive command '{}' targeting '{}'\n  Type 'yes' to proceed: ",
            command, target
        );
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).map_err(|e| {
            CliError::ValidationError(format!("failed to read confirmation input: {}", e))
        })?;
        let trimmed = input.trim();
        if trimmed.eq_ignore_ascii_case("yes") || trimmed.eq_ignore_ascii_case("y") {
            return Ok(());
        }
        return Err(CliError::ValidationError(format!(
            "operation cancelled by user (received '{}')",
            trimmed
        )));
    }

    if requires_delete_confirm(command) {
        return Err(CliError::ValidationError(format!(
            "critical delete command '{}' for '{}' requires --force and --confirm {} in non-interactive execution",
            command, target, target
        )));
    }

    Err(CliError::ValidationError(format!(
        "destructive command '{}' for '{}' requires --force (or --yes in interactive shell)",
        command, target
    )))
}

fn requires_delete_confirm(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.contains(" del")
        || normalized.contains(" mv")
        || normalized.contains(" rm")
        || normalized.contains(" delete")
        || normalized.contains(" --delete")
}

fn ensure_exact_confirm(
    global: &GlobalArgs,
    command: &str,
    expected: &str,
) -> Result<(), CliError> {
    // [Review Fix #6] ADrive delete-class commands are critical in
    // non-interactive execution and must echo the public adrive:// target.
    match global.confirm.as_deref() {
        Some(provided) if provided == expected => Ok(()),
        Some(provided) => Err(CliError::ValidationError(format!(
            "--confirm '{}' does not match the critical resource '{}' for {}",
            provided, expected, command
        ))),
        None => Err(CliError::ValidationError(format!(
            "critical delete command '{}' requires --confirm {} in non-interactive execution",
            command, expected
        ))),
    }
}
