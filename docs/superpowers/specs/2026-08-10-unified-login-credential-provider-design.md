# Unified Login Credential Provider Design

## 1. Status and scope

- Status: **Frozen**
- Frozen date: 2026-08-10
- Supported surfaces: `ve-tos-cli` and `ve-adrive-cli`
- Unsupported surface: `tos-cli`
- Credential SDK: `volcengine-rust-sdk-auth` 0.1.0 (Rust import path:
  `volcengine_rust_sdk_auth`), published on crates.io

This design adds a third explicit authentication source backed by the unified
Volcengine login framework. The framework remains the sole owner of its config
and login cache. The storage CLI reads credentials through the SDK and never
writes, migrates, or duplicates unified-login state.

The SDK returns a temporary `access_key_id`, `secret_access_key`, and
`session_token`. Unified login therefore reuses the existing HMAC resource
authentication protocol; it is not another Bearer Token protocol. Existing
ADrive OAuth continues to use Bearer Tokens and remains independent.

## 2. Authentication modes

Authentication mode is always explicit. Missing credentials never cause the
CLI to probe or fall back to another mode.

| Surface | Accepted modes | Compatibility default |
|---|---|---|
| `tos-cli` | Existing AK/SK behavior only; no `auth_mode` option | Existing behavior |
| `ve-tos-cli` | `aksk`, `unified` | `aksk` |
| `ve-adrive-cli` | `aksk`, `oauth`, `unified` | `aksk` |

Mode resolution uses the existing project-wide precedence rule:

```text
command-line option > selected config profile > environment variable > compatibility default
```

The concrete sources are:

| Surface | Command line | Config | Environment |
|---|---|---|---|
| `ve-tos-cli` | `--auth-mode` | `[profile.ve-tos].auth_mode` | `TOS_AUTH_MODE` |
| `ve-adrive-cli` | `--auth-mode` | `[profile.adrive].auth_mode` | `ADRIVE_AUTH_MODE` |

`--auth-mode` remains tool-scoped. `tos-cli` must not expose or accept it.
`config set auth_mode unified` writes only the active surface section. No
existing config or credential field is migrated or double-written.

## 3. Profile and configuration ownership

The selected storage CLI profile and unified-login profile use the same name.
For example:

```text
ve-tos-cli --profile test --auth-mode unified ls
```

uses the local `test` profile for resource settings and explicitly asks the SDK
for unified-login profile `test`. A missing unified-login profile is an error;
the CLI must not silently use the unified config's `current` or `default`
profile.

The SDK owns unified-login path resolution. The storage CLI does not add a
config-file option or copy unified credentials into `config.toml` or
`credentials.toml`. The SDK currently defaults to
`~/.volcengine/config.json` and may honor SDK-defined environment overrides.

When `unified` is selected:

- local resource settings such as endpoint, region, PSM, account ID, timeouts,
  and retry policy continue to use the storage CLI config;
- local AK/SK fields, ADrive OAuth Tokens, and `auth_endpoint` are unselected
  credential families and must be ignored;
- invalid or undecryptable unselected local credentials must not block a
  unified-login command;
- failure to resolve unified credentials is returned directly and never falls
  back to local AK/SK or OAuth.

## 4. SDK boundary

The CLI creates one `Arc<CliCredentials>` for the selected invocation and
constructs it with the same profile name:

```rust
CliCredentials::new(None, Some(global.profile.clone()))
```

The inspected SDK API is synchronous:

```rust
pub fn get(&self) -> Result<CredentialValue>;
```

It may perform file I/O, synchronous network requests, refreshes, retries, and
sleeps. Async storage request paths therefore invoke it through
`tokio::task::spawn_blocking`. The blocking wrapper is only runtime isolation;
it does not add credential lifecycle behavior.

The storage CLI deliberately does not implement:

- a second credential cache;
- expiration parsing or refresh windows;
- refresh locks or refresh single-flight logic;
- refresh retries;
- writes to unified-login files.

All caching, expiration decisions, refreshes, and refresh concurrency belong
to the SDK.

## 5. Request signing lifecycle

Every HTTP request attempt resolves one complete credential triple from the
SDK immediately before signing:

```text
HTTP attempt
  -> spawn_blocking(CliCredentials::get)
  -> one CredentialValue(AK, SK, SessionToken)
  -> construct the request signer
  -> sign and send the attempt
```

The AK, SK, and Session Token used by one attempt must come from the same SDK
result. A retry is a new HTTP attempt and calls the SDK again. This lets the SDK
return refreshed credentials without coupling the CLI to refresh semantics.

Special operations follow the same request-scoped rule:

- multipart upload resolves credentials independently for every part request;
- a presigned URL generation resolves credentials once for that URL;
- PostObject resolves credentials once and uses the same result for form
  preparation and policy signing;
- long-running MCP processes reuse the SDK provider object but call `get()` for
  every resource HTTP attempt.

`spawn_blocking` join failures and SDK credential failures are distinct error
paths. Credentials and SDK cache contents must never be included in either
error.

## 6. Client integration

### 6.1 Shared provider adapter

`tos-core` owns a small unified credential adapter shared by `ve-tos-cli` and
`ve-adrive-cli`. It stores the `Arc<CliCredentials>`, invokes `get()` through
`spawn_blocking`, and converts the SDK result into the existing internal AK/SK
plus Session Token value. It contains no cache or refresh policy.

The crates.io dependency must use a released, pinned compatible version rather
than the private Git repository. The published crate must declare its license
and supported Rust version before this open-source project depends on it.

### 6.2 `ve-tos-cli`

The TOS client keeps the existing static signer path unchanged for `aksk`.
Under `unified`, the request-send boundary obtains SDK credentials and creates
the signer for that attempt. Copy-source signing and normal request signing in
one attempt must share that signer.

The new mode resolver and CLI arguments belong only to the `ve-tos` command
surface. Shared `tos-cli` parsing and its ByteCloud credential behavior remain
unchanged.

### 6.3 `ve-adrive-cli`

ADrive extends its request authentication enum with `Unified`. Its wire behavior
matches `Aksk`: HMAC Authorization plus the temporary Session Token header.
OAuth remains the only Bearer path.

Unified credentials are not bound to the ADrive OAuth `instance_id`. Instance
listing and resource targeting therefore use existing AK/SK semantics, subject
to the permissions carried by the temporary credentials. OAuth-only validation,
user ID defaults, and bound-instance behavior must not run in unified mode.

## 7. Auth commands and diagnostics

The unified-login framework owns interactive login and logout. The storage CLI
must not spawn its executable or edit its state.

| Command | Unified-mode behavior |
|---|---|
| `ve-adrive auth login` | Does not start ADrive OAuth or mutate files; returns an actionable instruction to use `ve login` |
| `ve-adrive auth logout` | Does not delete local OAuth or unified state; returns an actionable instruction to use the unified-login framework |
| `ve-adrive auth status` | Calls SDK `get()` for the selected same-name profile and reports readiness |
| `doctor --check auth` | Calls SDK `get()` once and maps the stable SDK error code |

Successful status and Doctor output may include only:

- resolved auth mode and its source;
- selected profile name;
- SDK `provider_name`;
- whether a non-empty Session Token is present;
- readiness and a sanitized SDK error code when not ready.

They must not output AK, SK, Session Token, cached access/refresh tokens, or the
contents of unified-login files. `config show`, `--help`, `--describe`, command
registries, and applicable `SKILL.md` files must be updated with the new mode
without exposing credentials.

## 8. Error mapping

SDK `CredentialError::code()` is the stable classification input. Message text
may be retained only after normal CLI secret redaction.

| SDK failure family | CLI category | Suggested action |
|---|---|---|
| Missing config, profile, or login cache | `config_missing` | Run `ve login` for the selected profile |
| Expired/rejected refresh, `invalid_grant`, invalid login token | `auth_failed` | Run `ve login` again |
| Invalid unified config mode or malformed config | `validation_error` | Repair the unified-login configuration |
| Authentication service, STS, or IMDS transport failure | `transfer_failed` | Retry and run `doctor --check auth` |
| SDK lock poisoning or unclassified internal failure | `unknown` | Preserve the SDK code and sanitized message for diagnosis |

Unified errors must produce a unified-login repair command. They must not
recommend `ve-tos config init`, `ve-adrive config init`, `ve-adrive auth login`,
or local AK/SK configuration.

## 9. Compatibility and safety

- The default remains `aksk`, so existing invocations and scripts keep their
  current behavior.
- Existing AK/SK and OAuth resolution, signing, retry, endpoint, and credential
  persistence code paths are unchanged unless their mode is selected.
- No automatic migration, deletion, or double-write is introduced.
- The SDK value must not be formatted with user-visible `Debug`; sensitive
  request headers remain marked sensitive and redacted from traces.
- Unified-login failures never trigger an automatic interactive login.
- `tos-cli` help, config schema, environment handling, and runtime behavior do
  not gain unified authentication.

## 10. Verification requirements

Implementation is complete only when tests cover at least:

1. Existing `ve-tos-cli` and `ve-adrive-cli` invocations with no mode still
   select `aksk` and preserve current behavior.
2. `tos-cli --auth-mode unified` is rejected as an unknown option and never
   invokes the SDK.
3. CLI, config, environment, and compatibility-default mode precedence for
   both supported surfaces.
4. The storage `--profile` name is passed explicitly to the SDK, including a
   missing-profile failure with no fallback to unified `current`.
5. Unified mode ignores invalid local AK/SK and ADrive OAuth credentials.
6. Every initial request and retry attempt calls SDK `get()` exactly once and
   signs with one internally consistent credential triple.
7. Concurrent multipart requests can call the SDK safely through
   `spawn_blocking` without blocking Tokio worker threads.
8. Session Token headers are present and signed correctly for both TOS and
   ADrive; OAuth remains Bearer-only.
9. Presign and PostObject use a single credential result per logical signing
   operation.
10. `auth login`, `auth logout`, status, Doctor, `--dry-run`, help, describe,
    registries, and skills follow the unified-mode contract and never mutate or
    reveal unified credentials.
11. SDK join failures, stable credential errors, and transport failures map to
    the specified CLI categories and repair hints.
12. Full workspace regression tests pass, including existing AK/SK, OAuth,
    retry, transfer, config, and agent-output suites.
