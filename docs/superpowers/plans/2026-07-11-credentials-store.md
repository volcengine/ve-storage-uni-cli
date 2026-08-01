# Unified Credentials Store Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an encrypted `credentials.toml` used by all three public CLI surfaces while preserving legacy `config.toml` credential reads and limiting auth-mode selection to ADrive.

**Architecture:** A new `tos-core` credentials module owns paths, TOML schema, encryption, atomic persistence, and credential overlays. Existing profile loaders consume the overlay before their current config-over-environment merge; config setters route only sensitive keys to the new store. Inspection commands use the same overlay and retain source metadata.

**Tech Stack:** Rust, clap, serde, toml, AES-256-GCM helpers already present in `tos-core`, integration tests via Cargo.

---

### Task 1: Credentials path and TOML store

**Files:**
- Create: `crates/tos-core/src/infra/credentials.rs`
- Modify: `crates/tos-core/src/infra/mod.rs`
- Modify: `crates/tos-core/src/agent/global_args.rs`
- Test: `crates/tos-core/src/infra/credentials.rs`

- [ ] Write failing unit tests for default sibling path derivation, explicit path selection, schema round-trip, encrypted persistence, `0600` permissions, and profile/surface isolation.
- [ ] Run `cargo test -p tos-core infra::credentials` and verify failures identify the missing module/API.
- [ ] Add `CredentialSurface`, `CredentialFile`, `CredentialProfile`, `SurfaceCredentials`, `AkskCredentials`, and `OAuthCredentials` matching the approved schema.
- [ ] Add `GlobalArgs.credentials_path`, `credentials_path()`, and `existing_runtime_credentials_path()` with `--credentials-path` / `TOS_CREDENTIALS_PATH` handling.
- [ ] Implement `CredentialFile::load_from`, encrypted `save_to_path`, atomic sibling rename, get/set/delete methods, and masked inspection helpers.
- [ ] Re-run the focused tests and verify they pass.

### Task 2: Runtime AK/SK resolution for all surfaces

**Files:**
- Modify: `crates/tos/src/handler/common.rs`
- Modify: `crates/adrive/src/handler/common.rs`
- Test: `tests/config_test.rs`

- [ ] Write failing integration tests proving `credentials.toml > legacy config.toml > environment`, shared TOS behavior, per-surface overrides, ADrive isolation, explicit missing-path errors, and legacy-only behavior.
- [ ] Run the new tests and verify the current loaders ignore `credentials.toml`.
- [ ] Add a shared overlay resolver that decrypts the selected profile/surface and overlays only present fields onto the legacy config profile.
- [ ] Integrate it into the TOS and ADrive profile builders without changing non-credential merge order.
- [ ] Re-run focused and existing profile-precedence tests.

### Task 3: Route new credential writes without migration or dual-write

**Files:**
- Modify: `crates/tos/src/handler/config.rs`
- Modify: `crates/adrive/src/handler/meta.rs`
- Test: `tests/config_test.rs`

- [ ] Write failing tests proving `config set access_key_id`, `secret_access_key`, and `security_token` write only encrypted values to `credentials.toml`, preserve an existing `config.toml`, and route profiles/surfaces correctly.
- [ ] Verify dry-run reports the credentials target without writing either file.
- [ ] Route sensitive setters to `CredentialFile`; leave every non-sensitive setter on the existing `ConfigFile` path.
- [ ] Return `credentials_path` and a redacted value for credential writes while preserving command names and exit behavior.
- [ ] Re-run focused config-set tests and legacy non-sensitive config tests.

### Task 4: Dual-file config show and diagnostics

**Files:**
- Modify: `crates/tos-core/src/infra/config.rs`
- Modify: `crates/tos/src/handler/config.rs`
- Modify: `crates/adrive/src/handler/meta.rs`
- Modify: `crates/adrive/src/handler/auth.rs`
- Test: `tests/config_test.rs`

- [ ] Write failing tests for masked credentials-file values, `credentials_path` in JSON, source labels, legacy source fallback, and absence of raw secrets in all output formats.
- [ ] Extend credential source metadata without changing existing non-credential `FieldSource` labels.
- [ ] Overlay masked credential values in both TOS and ADrive `config show`; use the runtime resolver for ADrive status/doctor.
- [ ] Re-run focused inspection and output-redaction tests.

### Task 5: ADrive OAuth store framework and auth-mode precedence

**Files:**
- Modify: `crates/adrive/src/domain/auth.rs`
- Modify: `crates/adrive/src/handler/common.rs`
- Modify: `crates/adrive/src/handler/auth.rs`
- Test: `tests/config_test.rs`

- [ ] Write failing tests for `--auth-mode > config > ADRIVE_AUTH_MODE > aksk`, OAuth token file-over-environment precedence, mode isolation, and offline login/logout behavior.
- [ ] Replace direct environment-only OAuth credentials with the shared store model and source metadata.
- [ ] Keep OAuth network integration unimplemented and ensure no auth command contacts a service endpoint.
- [ ] Re-run all ADrive auth tests.

### Task 6: Help, documentation, review, and verification

**Files:**
- Modify: `README.md`
- Modify: grouped help strings in `crates/tos-core/src/agent/global_args.rs` and `crates/adrive/src/cli/mod.rs`
- Test: `tests/cli_basic.rs`

- [ ] Add help/docs for credentials paths, storage responsibilities, precedence, and compatibility.
- [ ] Run formatter, workspace check, `tos-core`, ADrive core, config, and CLI regression suites.
- [ ] Perform the required independent Reviewer pass across correctness, security, performance, maintainability, robustness, testability, and observability.
- [ ] Fix every Critical/Major finding with review-fix annotations and rerun verification.
