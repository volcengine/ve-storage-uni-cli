# ve-adrive-cli OAuth Complete Flow Implementation Plan

> Historical implementation plan. The current endpoint contract is defined by
> `docs/superpowers/specs/2026-08-05-explicit-service-endpoint-design.md` and
> supersedes this plan's built-in Auth endpoint and terminal QR assumptions.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement Device Authorization login, encrypted OAuth persistence, on-demand Refresh Token handling, and Bearer-authenticated ADrive requests without changing existing AK/SK behavior or the other two CLI surfaces.

**Architecture:** Keep OAuth protocol calls in a dedicated `OAuthClient`, lifecycle and persistence in an `OAuthTokenManager`, and request authentication selection inside the existing ADrive REST client. `auth login` owns the foreground polling loop; business commands ask the token manager for a usable Access Token before requests and force one refresh after the first 401. Configuration and credentials use atomic sibling replacement, while only the same-profile Refresh critical section uses a cross-process lock.

**Tech Stack:** Rust 2021, Tokio, Reqwest, Serde/TOML, Chrono, `qrcode`, `fs2`, AES-256-GCM credentials store, local TCP test servers.

---

## File structure

- Create `crates/tos-core/src/infra/atomic_file.rs`: shared owner-only sibling-file write, sync, and replace behavior for config and credentials.
- Modify `crates/tos-core/src/infra/config.rs`: `auth_endpoint` schema/resolution and atomic config saves.
- Modify `crates/tos-core/src/infra/credentials.rs`: OAuth issuer metadata and shared atomic writer use.
- Create `crates/adrive/src/domain/oauth.rs`: IDS Auth endpoint request/response types and one-request protocol methods.
- Create `crates/adrive/src/domain/token_manager.rs`: credential source selection, expiry checks, per-profile refresh lock, refresh persistence, and invalid-grant cleanup.
- Modify `crates/adrive/src/domain/client.rs`: mutually exclusive HMAC/Bearer request authentication and one 401 recovery.
- Modify `crates/adrive/src/domain/auth.rs`: stable auth constants and resolved OAuth provider inputs.
- Modify `crates/adrive/src/cli/auth.rs`: login arguments and auth command shapes.
- Modify `crates/adrive/src/handler/auth.rs`: login/status/logout behavior and polling state machine.
- Modify `crates/adrive/src/handler/common.rs`: endpoint/input resolution and AK/SK-or-OAuth client construction.
- Modify `README.md`, CLI help, registry metadata, and configuration inspection output to expose the completed behavior without exposing tokens.

### Task 1: Atomic persistence and OAuth configuration schema

**Files:**
- Create: `crates/tos-core/src/infra/atomic_file.rs`
- Modify: `crates/tos-core/src/infra/mod.rs`
- Modify: `crates/tos-core/src/infra/config.rs`
- Modify: `crates/tos-core/src/infra/credentials.rs`
- Test: inline unit tests in both infrastructure modules

- [ ] **Step 1: Write failing tests for atomic config replacement and OAuth metadata round-trip**

Add tests that save a config twice and verify the second complete TOML replaces the first without a leftover sibling temp file. Extend the OAuth credential round-trip test with this exact metadata shape:

```rust
StoredOAuthCredentials {
    access_token: Some("access".into()),
    refresh_token: Some("refresh".into()),
    expires_at: Some("2026-08-01T12:00:00Z".into()),
    token_type: Some("Bearer".into()),
    scope: vec!["file:read".into()],
    instance_id: Some("inst-1".into()),
    auth_endpoint: Some("https://idsauth.volces.com".into()),
}
```

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test -p tos-core infra::config::tests::config_save_replaces_atomically
cargo test -p tos-core infra::credentials::tests::oauth_metadata_round_trip
```

Expected: the config test fails because `ConfigFile::save_to` still writes directly; the metadata test fails to compile because the fields do not exist.

- [ ] **Step 3: Implement the shared atomic writer and schema fields**

Expose one crate-internal helper with this interface:

```rust
pub(crate) fn write_owner_only_atomic(path: &Path, content: &[u8]) -> Result<(), CliError>;
```

Move the existing credentials temp/write/sync/rename logic into it and call it from both stores. Add `auth_endpoint: Option<String>` to `AdriveOverride` plus `EffectiveProfile`, route `config set auth_endpoint` through `set_adrive_override`, and include the field in redacted config inspection. Add `instance_id` and `auth_endpoint` to `StoredOAuthCredentials`. Resolve `client_id` from `ADRIVE_OAUTH_CLIENT_ID` with the built-in production value as fallback; do not persist it. Preserve schema version 1 because all persisted fields are optional and old files remain readable.

- [ ] **Step 4: Verify GREEN and regression behavior**

Run:

```bash
cargo test -p tos-core infra::config
cargo test -p tos-core infra::credentials
```

Expected: all selected tests pass and no temp file remains after successful or failed replacement.

- [ ] **Step 5: Commit**

```bash
git add crates/tos-core/src/infra
git commit -m "feat: make cli configuration writes atomic"
```

### Task 2: IDS OAuth protocol client

**Files:**
- Create: `crates/adrive/src/domain/oauth.rs`
- Modify: `crates/adrive/src/domain/mod.rs`
- Modify: `crates/adrive/Cargo.toml`
- Modify: `Cargo.toml`
- Test: inline tests in `crates/adrive/src/domain/oauth.rs`

- [ ] **Step 1: Write failing protocol tests against a local TCP server**

Cover exact form bodies and response classification for:

```text
POST /v1/oauth/device_authorization
client_id=<resolved client id>
instance_id=inst-1
device_name=test-device
scope=all

POST /v1/oauth/token
grant_type=urn:ietf:params:oauth:grant-type:device_code
device_code=device-code
client_id=<same resolved client id>

POST /v1/oauth/token
grant_type=refresh_token
refresh_token=refresh-token
client_id=<resolved client id for current process>
```

Tests must prove no `client_secret` is sent and classify `authorization_pending`, `slow_down`, `access_denied`, `expired_token`, `invalid_grant`, `temporarily_unavailable`, and `server_error` without including response token values in error text.

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test -p ve-adrive-cli-core domain::oauth::tests
```

Expected: compilation fails because `domain::oauth` and its public types do not exist.

- [ ] **Step 3: Implement typed protocol methods**

Provide these focused methods:

```rust
impl OAuthClient {
    pub async fn create_device_authorization(
        &self,
        request: &DeviceAuthorizationRequest,
    ) -> Result<DeviceAuthorizationResponse, OAuthClientError>;

    pub async fn poll_device_token(
        &self,
        device_code: &str,
        client_id: &str,
    ) -> Result<DeviceTokenOutcome, OAuthClientError>;

    pub async fn refresh_token(
        &self,
        refresh_token: &str,
        client_id: &str,
    ) -> Result<TokenResponse, OAuthClientError>;
}
```

The client performs one HTTP operation per method. It validates HTTPS endpoints except explicit HTTP loopback test endpoints, bounds response bodies, honors request/connect timeouts, parses `request_id`, and never implements `Debug` for token-bearing structures.

- [ ] **Step 4: Verify GREEN**

Run:

```bash
cargo test -p ve-adrive-cli-core domain::oauth::tests
```

Expected: all protocol tests pass.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/adrive/Cargo.toml crates/adrive/src/domain
git commit -m "feat: add adrive oauth protocol client"
```

### Task 3: Device Authorization login, status, and logout

**Files:**
- Modify: `crates/adrive/src/cli/auth.rs`
- Modify: `crates/adrive/src/cli/mod.rs`
- Modify: `crates/adrive/src/handler/auth.rs`
- Modify: `crates/adrive/src/handler/common.rs`
- Modify: `crates/adrive/src/handler/meta.rs`
- Modify: `crates/adrive/src/registry.rs`
- Test: inline handler/CLI tests and `tests/config_test.rs`

- [ ] **Step 1: Write failing CLI and polling-state tests**

Assert that Clap accepts:

```text
ve-adrive-cli --auth-mode oauth auth login --instance inst-1 \
  --auth-endpoint https://idsauth.volces.com --device-name workstation
```

Add pure polling-state tests proving the first request waits one interval, pending preserves the interval, slow-down chooses at least `current + 5s`, denied/expired stop, and deadline/Ctrl-C never write credentials. Add dry-run tests proving no server is contacted and only the non-sensitive Auth Origin, Instance, Device Name, Client ID, and fixed Scope are emitted. Add status/logout tests proving only `[profile.adrive.oauth]` changes.

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test -p ve-adrive-cli-core cli::
cargo test -p ve-adrive-cli-core handler::auth::tests
```

Expected: login argument parsing and polling types are missing, and login still returns the framework-placeholder error.

- [ ] **Step 3: Implement the foreground login flow**

Use these fixed values and priority rules:

```rust
pub const OAUTH_CLIENT_ID: &str = "ve-adrive-cli-public-client-placeholder";
pub const OAUTH_SCOPE: &str = "all";
pub const DEFAULT_AUTH_ENDPOINT: &str = "https://idsauth.volces.com";
```

Resolve `auth_endpoint` as `--auth-endpoint > [profile.adrive].auth_endpoint > ADRIVE_AUTH_ENDPOINT > default`; resolve Instance as `--instance > config default_instance > ADRIVE_DEFAULT_INSTANCE`; resolve Device Name as flag, environment, hostname environment, then `ve-adrive-cli`. Normalize and validate the Auth Endpoint, reject a non-loopback HTTP URL and an Auth Origin equal to the Resource Origin before network access. Print the opaque verification URI, User Code, and Unicode QR to stderr, then drive the polling loop with `tokio::time::Instant` and `tokio::select!` for interval/deadline/Ctrl-C.

- [ ] **Step 4: Persist only a fully validated token result**

Require non-empty Access/Refresh Token, case-insensitive Bearer token type, positive expiry, non-empty resolved scope, and matching Instance ID. Compute UTC RFC3339 `expires_at`, load the latest credentials file, replace the current Profile OAuth group, and save atomically. Login does not acquire the Refresh lock.

- [ ] **Step 5: Implement status and local-only logout**

Status remains network-free and reports source, expiry state, scope, and Instance without token values. Logout removes only the current Profile OAuth group and reports `local_only`; AK/SK and other profiles stay untouched.

- [ ] **Step 6: Verify GREEN**

Run:

```bash
cargo test -p ve-adrive-cli-core handler::auth::tests
cargo test -p ve-adrive-cli-core cli::
cargo test --test config_test adrive_auth
```

Expected: all selected tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/adrive tests/config_test.rs
git commit -m "feat: implement adrive oauth device login"
```

### Task 4: On-demand token manager and Refresh coordination

**Files:**
- Create: `crates/adrive/src/domain/token_manager.rs`
- Modify: `crates/adrive/src/domain/mod.rs`
- Modify: `crates/adrive/src/domain/auth.rs`
- Modify: `crates/adrive/src/handler/common.rs`
- Modify: `Cargo.toml`
- Modify: `crates/adrive/Cargo.toml`
- Test: inline unit/integration tests in `token_manager.rs`

- [ ] **Step 1: Write failing token-selection and refresh tests**

Cover file-group-over-environment precedence without field mixing, environment Access Token use without refresh, Access Token reuse when more than 60 seconds remain, refresh when at most 60 seconds remain, forced refresh after 401, same-value and rotated Refresh Token replacement, missing issuer metadata, historical Access Token use until its first 401, `invalid_grant` cleanup, and preservation for all other OAuth errors.

- [ ] **Step 2: Write a failing concurrency test**

Start two managers for the same credentials path/Profile and one local Refresh endpoint. Make both observe an expired Access Token and assert the endpoint receives exactly one Refresh request; the second manager must reread and reuse the newly stored Token.

- [ ] **Step 3: Verify RED**

Run:

```bash
cargo test -p ve-adrive-cli-core domain::token_manager::tests
```

Expected: compilation fails because the token manager and lock do not exist.

- [ ] **Step 4: Implement `OAuthTokenManager`**

Expose a cloneable manager with these operations:

```rust
pub async fn access_token(&self) -> Result<String, CliError>;
pub async fn force_refresh(&self, rejected_access_token: &str) -> Result<String, CliError>;
pub fn validate_instance(&self, target_instance: &str) -> Result<(), CliError>;
```

Use a process-local async mutex plus an empty owner-only sibling lock file derived from a SHA-256 digest of the Profile name. Wait at most 10 seconds. After acquiring, reload credentials and reuse a newer valid Access Token before calling Refresh. Hold the lock through the Refresh request and atomic save, then release it on every return path.

- [ ] **Step 5: Implement Refresh error mapping**

Map `invalid_grant` to atomic current-Profile OAuth cleanup plus `login_required`. Preserve credentials for `invalid_request`, `invalid_client`, `unauthorized_client`, `invalid_scope`, `access_denied`, unsupported errors, and exhausted transient failures. Retry only `temporarily_unavailable`, `server_error`, network failures, 429, and 5xx with bounded backoff and legal `Retry-After` handling.

- [ ] **Step 6: Verify GREEN**

Run:

```bash
cargo test -p ve-adrive-cli-core domain::token_manager::tests
```

Expected: all token lifecycle and concurrency tests pass.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/adrive/Cargo.toml crates/adrive/src/domain crates/adrive/src/handler/common.rs
git commit -m "feat: add adrive oauth token refresh manager"
```

### Task 5: Bearer resource requests and one-time 401 recovery

**Files:**
- Modify: `crates/adrive/src/domain/client.rs`
- Modify: `crates/adrive/src/handler/common.rs`
- Test: inline tests in `crates/adrive/src/domain/client.rs`

- [ ] **Step 1: Write failing authentication-boundary tests**

Use local servers to prove:

- AK/SK requests retain their exact signing headers and retry behavior.
- OAuth requests contain `Authorization: Bearer ...` and contain no HMAC Authorization, `x-date`, or `x-security-token` headers.
- A first 401 forces one refresh and replays a replayable request once.
- A second 401 stops.
- 403 never refreshes.
- Concurrent multipart-style requests share one in-process refresh.
- A known target Instance mismatch fails before sending HTTP.

- [ ] **Step 2: Verify RED**

Run:

```bash
cargo test -p ve-adrive-cli-core domain::client::tests::oauth_
```

Expected: OAuth client construction and Bearer request authentication are not implemented.

- [ ] **Step 3: Refactor the client to use a mutually exclusive auth enum**

Keep the existing public AK/SK constructor behavior and add an OAuth constructor. At the central send boundary, branch on:

```rust
enum RequestAuth {
    Aksk(AkskRequestAuth),
    OAuth(OAuthTokenManager),
}
```

The HMAC branch keeps current canonical signing unchanged. The OAuth branch obtains a Token immediately before each HTTP request and attaches only Bearer authentication. On the first 401 it asks the manager to force refresh using the rejected Access Token as the generation check, then replays once. Existing 429/5xx/network retries remain independent of the single authentication retry budget.

- [ ] **Step 4: Verify GREEN and AK/SK regression**

Run:

```bash
cargo test -p ve-adrive-cli-core domain::client::tests
cargo test -p ve-adrive-cli-core handler::common::tests
```

Expected: all existing signing tests and new OAuth tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/adrive/src/domain/client.rs crates/adrive/src/handler/common.rs
git commit -m "feat: authenticate adrive resource requests with bearer tokens"
```

### Task 6: Public documentation, diagnostics, and full regression

**Files:**
- Modify: `README.md`
- Modify: ADrive grouped help and registry descriptions under `crates/adrive/src/`
- Modify: diagnostics tests under `tests/` and crate test modules

- [ ] **Step 1: Write failing public-contract tests**

Assert help, capabilities, `config show`, `auth status`, and machine-readable error output expose `auth_endpoint`, OAuth readiness, `login_required`, and `request_id` while never exposing Access Token, Refresh Token, Device Code, encryption keys, or secret-bearing source lines.

- [ ] **Step 2: Verify RED**

Run the focused integration tests and confirm failures are caused by stale placeholder descriptions or missing output fields.

- [ ] **Step 3: Update public documentation and diagnostics**

Remove all “OAuth integration pending” text, document the exact mode and endpoint precedence, explain foreground Device Code polling and on-demand Refresh, and state that scripts receive `login_required` rather than an automatic interactive login.

- [ ] **Step 4: Run formatting and complete verification**

Run:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
```

Expected: every command exits 0 with no test failures.

- [ ] **Step 5: Perform mandatory Reviewer pass**

Review correctness, token leakage, endpoint validation, cross-process races, retry bounds, request replay safety, AK/SK compatibility, performance, and tests. Fix every Critical and Major finding with numbered `[Review Fix #N]` comments, rerun the complete verification commands, and record any remaining Minor/Suggestion items.

- [ ] **Step 6: Commit**

```bash
git add README.md crates tests Cargo.toml Cargo.lock
git commit -m "docs: document adrive oauth authentication"
```
