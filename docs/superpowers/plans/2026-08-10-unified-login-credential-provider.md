# Unified Login Credential Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add explicit `unified` authentication to `ve-tos-cli` and `ve-adrive-cli`, resolving the same-name unified-login profile through the published credential SDK for every HTTP attempt while leaving `tos-cli`, AK/SK, and OAuth behavior unchanged.

**Architecture:** A small `tos-core` adapter owns an `Arc<CliCredentials>` and invokes synchronous `get()` through `spawn_blocking`; it contains no cache or refresh policy. Each supported resource client asks the adapter for one AK/SK/SessionToken triple immediately before signing an HTTP attempt.

**Tech Stack:** Rust 2021 workspace, Tokio, Clap, Reqwest, existing TOS/IDS HMAC signers, `volcengine-rust-sdk-auth` 0.1.0 from crates.io (Rust import path: `volcengine_rust_sdk_auth`).

---

## File map

### Create

- `crates/tos-core/src/infra/unified_credentials.rs` — SDK boundary and error mapping.
- `crates/tos/src/domain/auth.rs` — `ve-tos` auth mode and resolution.
- `crates/tos/src/cli/auth.rs` — `ve-tos`-scoped `--auth-mode`.

### Modify

- `Cargo.toml`, `crates/tos-core/Cargo.toml`, `Cargo.lock` — SDK dependency.
- `crates/tos-core/src/infra/{mod.rs,config.rs,client.rs}` — provider export, schema, credential-free profiles, and dynamic signing.
- `crates/tos-core/src/agent/global_args.rs` — internal tool-scoped mode handoff.
- `crates/tos/src/{cli/mod.rs,domain/mod.rs,handler/common.rs,handler/config.rs}` — mode/config resolution.
- `crates/tos/src/handler/{advanced,bucket,bucket_config,high_level,meta,multipart,object,turbo}.rs` — mode-aware client factory.
- `crates/adrive/src/{domain/auth.rs,domain/client.rs,handler/common.rs,handler/auth.rs,handler/meta.rs}` — third mode and request signing.
- `src/lib.rs` — `ve-tos` dispatch and repair hints.
- Registries, help, README, and `skills/{ve-tos-cli,ve-adrive-cli}/SKILL.md` — user-facing synchronization.

## Task 1: Add the SDK and blocking adapter

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/tos-core/Cargo.toml`
- Modify: `crates/tos-core/src/infra/mod.rs`
- Create: `crates/tos-core/src/infra/unified_credentials.rs`
- Modify: `Cargo.lock`

- [ ] **Step 1: Verify the public package**

Run:

```bash
cargo info volcengine-rust-sdk-auth@0.1.0
```

Expected: version 0.1.0 exists, declares a license and compatible Rust version, and exposes `CliCredentials::new` plus synchronous `get()`. Stop and revise the design if the public API differs.

- [ ] **Step 2: Write failing adapter tests**

Add module tests covering a call counter, blocking-thread execution, complete triple preservation, stable error categories, and redacted `Debug`:

```rust
#[tokio::test(flavor = "current_thread")]
async fn get_calls_the_resolver_on_a_blocking_thread_every_time() {
    let runtime_thread = std::thread::current().id();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&calls);
    let provider = UnifiedCredentialProvider::from_resolver(move || {
        observed.fetch_add(1, Ordering::SeqCst);
        assert_ne!(std::thread::current().id(), runtime_thread);
        Ok(UnifiedCredentialValue::new("ak", "sk", "sts", "test"))
    });

    provider.get().await.unwrap();
    provider.get().await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn debug_redacts_every_credential_field() {
    let value = UnifiedCredentialValue::new("AK_SECRET", "SK_SECRET", "STS_SECRET", "test");
    let output = format!("{value:?}");
    assert!(!output.contains("AK_SECRET"));
    assert!(!output.contains("SK_SECRET"));
    assert!(!output.contains("STS_SECRET"));
}
```

- [ ] **Step 3: Verify the tests fail**

Run: `cargo test -p tos-core unified_credentials -- --nocapture`

Expected: compilation fails because the adapter types do not exist.

- [ ] **Step 4: Add the dependency and minimal adapter**

Add:

```toml
# root Cargo.toml
[workspace.dependencies]
volcengine-rust-sdk-auth = "=0.1.0"

# crates/tos-core/Cargo.toml
[dependencies]
volcengine-rust-sdk-auth = { workspace = true }
```

Implement this shape:

```rust
type CredentialResolver =
    dyn Fn() -> Result<UnifiedCredentialValue, CliError> + Send + Sync + 'static;

#[derive(Clone)]
pub struct UnifiedCredentialProvider {
    resolver: Arc<CredentialResolver>,
}

pub struct UnifiedCredentialValue {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub provider_name: String,
}

impl UnifiedCredentialProvider {
    pub fn new(profile_name: impl Into<String>) -> Self {
        let sdk = Arc::new(CliCredentials::new(None, Some(profile_name.into())));
        Self {
            resolver: Arc::new(move || {
                sdk.get()
                    .map(UnifiedCredentialValue::from)
                    .map_err(map_sdk_error)
            }),
        }
    }

    pub async fn get(&self) -> Result<UnifiedCredentialValue, CliError> {
        let resolver = Arc::clone(&self.resolver);
        tokio::task::spawn_blocking(move || resolver())
            .await
            .map_err(|error| {
                CliError::Unknown(format!(
                    "[UnifiedCredentialJoin] credential task failed: {error}"
                ))
            })?
    }
}
```

Implement a manual `Debug` that displays only `provider_name` and redacted markers. Map SDK codes to fixed secret-safe messages:

```rust
fn map_sdk_error(error: CredentialError) -> CliError {
    let code = error.code();
    let message = format!("[{code}] unified login credentials are unavailable");
    match code {
        "CliConfigLoad" | "CliConfigNoProfiles" | "CliConfigProfileNotFound"
        | "CliConfigAccessKey" | "CliConfigSecretKey"
        | "CliConfigLoginSessionMissing" | "CliConfigRoleNameMissing"
        | "CliConfigAccountIDMissing" | "CliConfigOIDCTokenFileMissing"
        | "CliConfigOIDCRoleTrnMissing" | "CliConfigSsoSessionNameMissing"
        | "CliConfigSsoSessionNotFound" | "CliConfigSsoStartURLMissing"
        | "CliConsoleLoginCacheLoad" | "CliConsoleLoginCacheMissing"
        | "CliSsoTokenCacheLoad" | "CliSsoTokenCacheMissing" =>
            CliError::ConfigMissing(message),
        "CliConsoleLoginInvalidGrant" | "CliConsoleLoginRefreshTokenExpired"
        | "CliConsoleLoginRefreshTokenMissing" | "CliConsoleLoginAccessTokenInvalid"
        | "CliConsoleLoginAccessTokenParse" | "CliConsoleLoginClientIDMissing"
        | "CliSsoTokenClientMissing" | "CliSsoTokenRefreshEmpty"
        | "CliSsoTokenRefreshExpired" | "CliSsoTokenRefreshInvalidGrant"
        | "CliSsoTokenRefreshMissing" =>
            CliError::AuthFailed(message),
        "CliConfigModeInvalid" | "CliConfigUnmarshal"
        | "CliConfigExpiration" | "CliConsoleLoginCacheEmpty"
        | "CliConsoleLoginCacheUnmarshal" | "CliConsoleLoginTokenExpiration"
        | "CliSsoTokenCacheEmpty" | "CliSsoTokenCacheUnmarshal"
        | "CliSsoTokenRefreshExpiresIn" | "CliSsoTokenRefreshExpiresParse"
        | "CliTokenExpirationParse" =>
            CliError::ValidationError(message),
        "CliConsoleLoginRefreshTokenFailed" | "CliSsoPortalCredentials"
        | "CliSsoTokenRefreshFailed" | "EcsRoleCredentialsFailed"
        | "OIDCAssumeRole" | "StsProviderAssumeRole" =>
            CliError::TransferFailed(message),
        _ => CliError::Unknown(message),
    }
}
```

Export with `pub mod unified_credentials;`. Do not forward arbitrary provider bodies into errors.

- [ ] **Step 5: Run focused verification**

Run:

```bash
cargo test -p tos-core unified_credentials
cargo tree -p tos-core -i volcengine-rust-sdk-auth
```

Expected: tests pass and the SDK is present through `tos-core`.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tos-core/Cargo.toml crates/tos-core/src/infra/mod.rs crates/tos-core/src/infra/unified_credentials.rs
git commit -m "feat: add unified credential SDK adapter"
```

## Task 2: Extend config schemas and mode enums

**Files:**
- Modify: `crates/tos-core/src/infra/config.rs`
- Create: `crates/tos/src/domain/auth.rs`
- Modify: `crates/tos/src/domain/mod.rs`
- Modify: `crates/adrive/src/domain/auth.rs`
- Modify: `crates/tos/src/handler/config.rs`

- [ ] **Step 1: Write failing schema tests**

```rust
#[test]
fn ve_tos_accepts_unified_but_tos_rejects_auth_mode() {
    let mut config = ConfigFile::default();
    config
        .set_by_path(&["default", "ve-tos", "auth_mode"], "unified")
        .unwrap();
    assert_eq!(
        config.profiles["default"].ve_tos.as_ref().unwrap().auth_mode.as_deref(),
        Some("unified")
    );
    assert!(config
        .set_by_path(&["default", "tos", "auth_mode"], "unified")
        .is_err());
}

#[test]
fn adrive_accepts_all_three_modes() {
    for value in ["aksk", "oauth", "unified"] {
        assert_eq!(AuthMode::parse(value, "test").unwrap().as_str(), value);
    }
}
```

- [ ] **Step 2: Verify the tests fail**

Run:

```bash
cargo test -p tos-core ve_tos_accepts_unified_but_tos_rejects_auth_mode
cargo test -p ve-adrive-cli-core adrive_accepts_all_three_modes
```

Expected: failures because the fields/modes are absent.

- [ ] **Step 3: Add binary-aware config validation**

Add `auth_mode: Option<String>` to `TosOverride`. Refactor the current TOS override setter to take `supports_auth_mode`; pass `false` for `Binary::Tos` and `true` for `Binary::VeTos`:

```rust
"auth_mode" if supports_auth_mode => {
    let normalized = value.trim().to_ascii_lowercase();
    if !matches!(normalized.as_str(), "aksk" | "unified") {
        return Err(CliError::ValidationError(format!(
            "invalid ve-tos auth_mode '{value}': expected aksk or unified"
        )));
    }
    override_value.auth_mode = Some(normalized);
}
"auth_mode" => {
    return Err(CliError::ValidationError(
        "auth_mode is not supported by tos".to_string(),
    ));
}
```

Populate `EffectiveProfile.auth_mode` for VeTos and ADrive. Route bare `ve-tos config set auth_mode` to `[active-profile.ve-tos]`; continue rejecting explicit `.tos.auth_mode`.

Because `tos` and `ve-tos` currently deserialize through the same `TosOverride` type, add post-deserialization validation in `ConfigFile::load_from`: reject any profile whose `[profile.tos]` has a non-empty `auth_mode`. This prevents hand-written `tos` config from bypassing the setter and silently gaining the new mode.

- [ ] **Step 4: Add surface-specific enums**

Create the VeTos enum:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    Aksk,
    Unified,
}
```

Implement `as_str` and `parse` accepting exactly `aksk|unified`. Extend ADrive's existing enum/parser/messages to exactly `aksk|oauth|unified`. Keep the enums separate.

Define the VeTos resolution result explicitly so later tasks use stable names:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum AuthModeSource {
    CommandLine,
    Config,
    Environment,
    CompatibilityDefault,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedAuthMode {
    pub mode: AuthMode,
    pub source: AuthModeSource,
}
```

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test -p tos-core infra::config
cargo test -p ve-tos-cli-core domain::auth
cargo test -p ve-adrive-cli-core domain::auth
cargo test -p ve-tos-cli-core handler::config
```

Expected: all pass.

- [ ] **Step 6: Commit**

```bash
git add crates/tos-core/src/infra/config.rs crates/tos/src/domain/auth.rs crates/tos/src/domain/mod.rs crates/adrive/src/domain/auth.rs crates/tos/src/handler/config.rs
git commit -m "feat: define unified authentication modes"
```

## Task 3: Add tool-scoped VeTos mode dispatch and credential isolation

**Files:**
- Create: `crates/tos/src/cli/auth.rs`
- Modify: `crates/tos/src/cli/mod.rs`
- Modify: `crates/tos-core/src/agent/global_args.rs`
- Modify: `src/lib.rs`
- Modify: `crates/tos/src/handler/common.rs`
- Modify: `crates/tos-core/src/infra/config.rs`

- [ ] **Step 1: Write failing parser and isolation tests**

Add root parser assertions:

```rust
assert!(Cli::try_parse_from([
    "ve-storage-uni-cli", "ve-tos", "--auth-mode", "unified", "ls"
]).is_ok());
assert!(Cli::try_parse_from([
    "ve-storage-uni-cli", "tos", "--auth-mode", "unified", "ls"
]).is_err());
```

Add `handler/common.rs` tests selecting unified mode with `secret_access_key = "ENC:not-valid"` in both local config and credentials fixtures. `build_profile` must return endpoint/region without attempting to decrypt those fields.

- [ ] **Step 2: Verify the tests fail**

Run:

```bash
cargo test --lib ve_tos_auth_mode_is_tool_scoped
cargo test -p ve-tos-cli-core unified_mode_ignores_unselected_local_credentials
```

Expected: parser and credential-isolation failures.

- [ ] **Step 3: Add tool-scoped args without widening `tos-cli`**

Create:

```rust
#[derive(Clone, Copy, Debug, Default, clap::Args)]
pub struct VeTosAuthArgs {
    #[arg(long, value_enum, global = true)]
    pub auth_mode: Option<crate::domain::auth::AuthMode>,
}
```

Flatten it only into `ToolCommand::Tos`. Add this internal field to `GlobalArgs`:

```rust
#[arg(skip)]
pub ve_tos_auth_mode: Option<String>,
```

Initialize it to `None`. In `handle_tos`, copy the parsed tool-specific value into the field before handler dispatch. Do not add `auth_mode` as a shared/global Clap option.

- [ ] **Step 4: Implement exact precedence**

Resolve VeTos mode in this order:

```rust
fn resolved(mode: AuthMode, source: AuthModeSource) -> Result<ResolvedAuthMode, CliError> {
    Ok(ResolvedAuthMode { mode, source })
}

if let Some(value) = global.ve_tos_auth_mode.as_deref() {
    return resolved(
        AuthMode::parse(value, "command line")?,
        AuthModeSource::CommandLine,
    );
}
if let Some(value) = selected_profile
    .ve_tos
    .as_ref()
    .and_then(|settings| settings.auth_mode.as_deref())
{
    return resolved(
        AuthMode::parse(value, "profile config")?,
        AuthModeSource::Config,
    );
}
if let Ok(value) = std::env::var("TOS_AUTH_MODE") {
    return resolved(
        AuthMode::parse(&value, "TOS_AUTH_MODE")?,
        AuthModeSource::Environment,
    );
}
resolved(AuthMode::Aksk, AuthModeSource::CompatibilityDefault)
```

- [ ] **Step 5: Build non-secret profiles for unified mode**

Refactor effective profile construction to take `decrypt_credentials: bool`. For `unified`, clear shared and VeTos AK/SK/ST fields before effective-profile decryption and do not load `CredentialsFile`. For `aksk`, pass `true` and preserve the current overlay/decryption sequence. The network/timeout/control-plane fields must be identical in both outputs.

- [ ] **Step 6: Run focused regression tests**

Run:

```bash
cargo test --lib ve_tos_auth_mode_is_tool_scoped
cargo test -p ve-tos-cli-core handler::common
cargo test -p tos-cli-core
```

Expected: VeTos accepts the option, `tos-cli` rejects it, unified ignores local secrets, and ByteCloud tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/tos/src/cli/auth.rs crates/tos/src/cli/mod.rs crates/tos-core/src/agent/global_args.rs src/lib.rs crates/tos/src/handler/common.rs crates/tos-core/src/infra/config.rs
git commit -m "feat: resolve ve-tos unified auth mode"
```

## Task 4: Resolve TOS credentials once per HTTP attempt

**Files:**
- Modify: `crates/tos-core/src/infra/client.rs`
- Modify: `crates/tos/src/handler/common.rs`
- Modify: `crates/tos/src/handler/{advanced,bucket,bucket_config,high_level,meta,multipart,object,turbo}.rs`

- [ ] **Step 1: Write failing attempt-scope tests**

Use a counting provider and a test server that returns 500 then 200. Assert:

```rust
assert_eq!(credential_calls.load(Ordering::SeqCst), 2);
assert_eq!(captured_requests.len(), 2);
assert!(captured_requests[0].contains("x-tos-security-token: token-1"));
assert!(captured_requests[1].contains("x-tos-security-token: token-2"));
```

Add tests proving CopyObject shares one returned signer between copy-source and request signatures, presign calls the provider once, and PostObject preparation/signing calls it once.

- [ ] **Step 2: Verify the tests fail**

Run: `cargo test -p tos-core unified_signing -- --nocapture`

Expected: failures because `TosClient` owns only a static signer.

- [ ] **Step 3: Introduce request auth without changing static behavior**

Replace the private signer field with:

```rust
enum TosRequestAuth {
    Static(TosSigner),
    Unified {
        provider: UnifiedCredentialProvider,
        algorithm: TosSignAlgorithm,
        region: String,
        service: String,
    },
}
```

Keep `TosClient::new` constructing `Static`. Add `TosClient::new_unified`, reuse the same endpoint/region/HTTP option validation, and do not read `Profile` credentials.

Inside `send_request_once` and `send_signed_request_once`, resolve the unified signer after entering the retry attempt and before copy-source/request signing. The same owned signer must serve both signatures in that attempt. Static mode continues borrowing its existing signer.

- [ ] **Step 4: Make one-shot signing operations cohesive**

Change presign to an async operation that resolves one signer:

```rust
pub async fn presign_object_url(
    &self,
    method: &str,
    bucket: &str,
    key: &str,
    expires: u64,
) -> Result<String, CliError>;
```

Add `prepare_form_auth().await -> FormAuthContext`; the returned non-Debug context owns one signer and exposes both `form_prepare()` and `form_sign()`. Update PostObject to retain that context across policy construction.

- [ ] **Step 5: Centralize VeTos client construction**

Add:

```rust
pub(crate) fn build_client(
    global: &GlobalArgs,
    profile: &Profile,
    service: &str,
) -> Result<TosClient, CliError> {
    match resolve_auth_mode(global)?.mode {
        AuthMode::Aksk => TosClient::new(profile, service),
        AuthMode::Unified => TosClient::new_unified(
            profile,
            service,
            UnifiedCredentialProvider::new(global.profile.clone()),
        ),
    }
}
```

Replace production `TosClient::new` calls in the listed handlers with this factory. Await the presign method and use one `FormAuthContext` for PostObject. Leave test-only static client construction unchanged unless the test targets the factory.

- [ ] **Step 6: Run TOS regression tests**

Run:

```bash
cargo test -p tos-core infra::client
cargo test -p ve-tos-cli-core
cargo test -p tos-cli-core
```

Expected: all pass, including existing AK/SK tests.

- [ ] **Step 7: Commit**

```bash
git add crates/tos-core/src/infra/client.rs crates/tos/src/handler
git commit -m "feat: sign ve-tos requests with unified credentials"
```

## Task 5: Add per-attempt unified signing to ADrive

**Files:**
- Modify: `crates/adrive/src/domain/auth.rs`
- Modify: `crates/adrive/src/domain/client.rs`
- Modify: `crates/adrive/src/handler/common.rs`

- [ ] **Step 1: Write failing ADrive tests**

Cover:

```rust
assert_eq!(resolve_auth_mode(&global, Some(AuthMode::Unified))?.mode, AuthMode::Unified);
assert!(!client.uses_oauth());
assert_eq!(client.instance_listing_scope()?, InstanceListingScope::All);
assert!(captured_request.contains("x-security-token: temporary-sts"));
assert!(captured_request.contains("Authorization: HMAC-SHA256"));
assert!(!captured_request.contains("Bearer "));
```

Use invalid encrypted local AK/SK/OAuth fixtures and prove unified provider construction does not decrypt or load them.

- [ ] **Step 2: Verify the tests fail**

Run: `cargo test -p ve-adrive-cli-core unified -- --nocapture`

Expected: failures because no unified provider/request variant exists.

- [ ] **Step 3: Add isolated provider construction**

Add:

```rust
pub struct UnifiedAuthProvider {
    pub(crate) credentials: UnifiedCredentialProvider,
    pub(crate) endpoint: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) client_options: ClientOptions,
}

pub enum AuthProvider {
    Aksk(AkskAuthProvider),
    OAuth(OAuthAuthProvider),
    Unified(UnifiedAuthProvider),
}
```

Branch on all three modes before loading any credential family. Unified resource settings still resolve command line, `[profile.adrive]`, and `ADRIVE_*` endpoint/region/options, but never load credentials TOML, OAuth metadata, `auth_endpoint`, or local AK/SK.

Extend `build_ids_client` exhaustively:

```rust
let client = match build_auth_provider(global, command_line_mode)? {
    AuthProvider::Aksk(value) => IdsClient::new(
        value.access_key,
        value.secret_key,
        value.security_token,
        value.endpoint,
        value.region,
        value.client_options,
    ),
    AuthProvider::OAuth(value) => IdsClient::new_oauth(
        value.token_manager,
        value.endpoint,
        value.region,
        value.client_options,
    ),
    AuthProvider::Unified(value) => IdsClient::new_unified(
        value.credentials,
        value.endpoint,
        value.region,
        value.client_options,
    ),
};
```

- [ ] **Step 4: Sign every ADrive attempt through the SDK**

Extend:

```rust
enum RequestAuth {
    Aksk(AkskRequestAuth),
    OAuth(OAuthTokenManager),
    Unified(UnifiedCredentialProvider),
}
```

Add the branch:

```rust
RequestAuth::Unified(provider) => {
    let value = provider.get().await.map_err(Error::Cli)?;
    let auth = AkskRequestAuth {
        access_key: value.access_key_id,
        secret_key: value.secret_access_key,
        security_token: (!value.session_token.is_empty()).then_some(value.session_token),
    };
    self.apply_aksk_auth(&auth, method, path, url, headers)?;
    Ok(None)
}
```

Treat Unified like AK/SK in instance listing, OAuth-user lookup, resource targeting, and OAuth-only validation. Because `send_once` is inside retry iteration, each attempt calls the SDK once.

- [ ] **Step 5: Run ADrive tests**

Run:

```bash
cargo test -p ve-adrive-cli-core domain::client
cargo test -p ve-adrive-cli-core handler::common
```

Expected: unified is HMAC+SessionToken, OAuth remains Bearer-only, and static tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/adrive/src/domain/auth.rs crates/adrive/src/domain/client.rs crates/adrive/src/handler/common.rs
git commit -m "feat: sign adrive requests with unified credentials"
```

## Task 6: Add status, Doctor, and repair guidance

**Files:**
- Modify: `crates/adrive/src/handler/auth.rs`
- Modify: `crates/adrive/src/handler/common.rs`
- Modify: `crates/adrive/src/handler/meta.rs`
- Modify: `crates/tos/src/handler/meta.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Write failing command/diagnostic tests**

Assert unified errors recommend the external framework:

```rust
assert_eq!(guidance.fix_command.as_deref(), Some("ve login"));
assert_eq!(
    guidance.doctor_hint.as_deref(),
    Some("ve-adrive doctor --check auth")
);
```

Serialize successful status/Doctor output and assert it contains mode, source, profile, provider name, and `has_session_token`, but none of `access_key_id`, `secret_access_key`, `session_token`, or their fixture values. Also verify unified `auth login/logout`, including dry-run, leave local credentials and unified fixtures byte-identical.

- [ ] **Step 2: Verify the tests fail**

Run:

```bash
cargo test -p ve-adrive-cli-core handler::auth
cargo test -p ve-adrive-cli-core handler::meta
cargo test -p ve-tos-cli-core handler::meta
```

Expected: failures because handlers only understand AK/SK and OAuth.

- [ ] **Step 3: Implement external ownership for auth commands**

Branch before OAuth requirements:

```rust
AuthMode::Unified => match action {
    AuthAction::Status => unified_status(global).await,
    AuthAction::Login(_) => Err(CliError::ValidationError(
        "[unified_login_managed_externally] run `ve login` for the selected profile".to_string(),
    )),
    AuthAction::Logout => Err(CliError::ValidationError(
        "[unified_logout_managed_externally] use the unified-login framework to log out".to_string(),
    )),
},
```

Status and both Doctor implementations construct the provider with `global.profile`, call `get()` once, and expose only approved non-secret fields.

- [ ] **Step 4: Make error guidance mode-aware**

Detect `unified` before generic config/auth guidance. For SDK errors and external-login command errors, use `ve login` as `fix_command` and the surface-specific Doctor command as `doctor_hint`. Never recommend `config init`, local AK/SK fields, or `ve-adrive auth login` while unified is selected.

In `src/lib.rs`, make VeTos `suggest_fix` consult the resolved mode before its current generic `ConfigMissing/AuthFailed` branches.

- [ ] **Step 5: Run focused tests**

Run:

```bash
cargo test -p ve-adrive-cli-core handler::auth
cargo test -p ve-adrive-cli-core handler::meta
cargo test -p ve-tos-cli-core handler::meta
cargo test --lib suggest_fix
```

Expected: all pass and no captured output contains credentials.

- [ ] **Step 6: Commit**

```bash
git add crates/adrive/src/handler/auth.rs crates/adrive/src/handler/common.rs crates/adrive/src/handler/meta.rs crates/tos/src/handler/meta.rs src/lib.rs
git commit -m "feat: diagnose unified login credentials"
```

## Task 7: Synchronize help, describe, registries, skills, and README

**Files:**
- Modify: `crates/tos/src/cli/meta.rs`
- Modify: `crates/tos/src/registry.rs`
- Modify: `crates/adrive/src/cli/auth.rs`
- Modify: `crates/adrive/src/cli/meta.rs`
- Modify: `crates/adrive/src/registry.rs`
- Modify: `skills/ve-tos-cli/SKILL.md`
- Modify: `skills/ve-adrive-cli/SKILL.md`
- Modify: `README.md`

- [ ] **Step 1: Write failing metadata assertions**

Require VeTos metadata to contain `--auth-mode <MODE>`, `aksk or unified`, `TOS_AUTH_MODE`, and `ve login`. Require ADrive metadata to contain `--auth-mode <MODE>`, `aksk, oauth, or unified`, `ADRIVE_AUTH_MODE`, and `ve login`:

```text
--auth-mode <MODE>
aksk or unified
aksk, oauth, or unified
TOS_AUTH_MODE
ADRIVE_AUTH_MODE
ve login
```

Assert `tos-cli` metadata contains none of the unified mode or auth-mode option. Assert `--describe` exposes exact enums and never unified config paths or credential values.

- [ ] **Step 2: Verify metadata tests fail**

Run:

```bash
cargo test -p ve-tos-cli-core registry
cargo test -p ve-adrive-cli-core registry
cargo test --lib help
```

Expected: failures on missing mode/help metadata.

- [ ] **Step 3: Update every user-facing surface**

Use these examples consistently:

```bash
ve-tos-cli --profile default --auth-mode unified ls
ve-tos-cli config set auth_mode unified
ve-adrive-cli --profile default --auth-mode unified ls
ve-adrive-cli config set auth_mode unified
ve login
```

State that unified mode selects a same-name SDK profile, ignores local AK/SK/OAuth, and leaves login/logout ownership external. Preserve existing AK/SK/OAuth examples and update config-key allowlists for VeTos.

- [ ] **Step 4: Run metadata checks**

Run:

```bash
cargo test -p ve-tos-cli-core registry
cargo test -p ve-adrive-cli-core registry
cargo test --lib help
rg -n "unified|TOS_AUTH_MODE|ADRIVE_AUTH_MODE|ve login" README.md skills crates/tos/src/registry.rs crates/adrive/src/registry.rs
```

Expected: tests pass and the frozen terminology appears only on supported surfaces.

- [ ] **Step 5: Commit**

```bash
git add README.md crates/tos/src/cli/meta.rs crates/tos/src/registry.rs crates/adrive/src/cli/auth.rs crates/adrive/src/cli/meta.rs crates/adrive/src/registry.rs skills/ve-tos-cli/SKILL.md skills/ve-adrive-cli/SKILL.md
git commit -m "docs: document unified login authentication"
```

## Task 8: Full review and compatibility verification

**Files:**
- Review: every file changed by Tasks 1–7
- Test: workspace and dedicated command surfaces

- [ ] **Step 1: Format and statically check**

Run:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
```

Expected: both succeed with no new warnings.

- [ ] **Step 2: Run the full suite**

Run: `cargo test --workspace`

Expected: every existing and new test passes.

- [ ] **Step 3: Verify public CLI boundaries**

Run:

```bash
cargo run -- ve-tos --help
cargo run -- ve-adrive --help
cargo run -- tos --auth-mode unified ls
```

Expected: supported help lists the correct modes; the `tos` invocation fails in argument parsing because the option is unsupported.

- [ ] **Step 4: Perform the mandatory Reviewer pass**

Review correctness, security, performance, maintainability, robustness, testability, and observability. Explicitly verify:

- exactly one SDK call per HTTP attempt, including retries;
- no CLI cache, expiry parser, or refresh policy;
- no Tokio worker blocking and no panic on `spawn_blocking` join failure;
- no credentials in `Debug`, logs, errors, status, or Doctor;
- identical storage/unified profile names;
- no read/decrypt of unselected local credentials;
- unchanged AK/SK and OAuth behavior;
- no unified option or provider invocation from `tos-cli`.

Fix every Critical/Major finding and repeat Steps 1–3. Do not create an empty review commit when no correction is necessary.

- [ ] **Step 5: Verify scope and history**

Run:

```bash
git status --short
git log --oneline --decorate -10
git diff origin/master...HEAD --stat
```

Expected: only planned implementation/documentation changes are present and each implementation unit has its own commit.
