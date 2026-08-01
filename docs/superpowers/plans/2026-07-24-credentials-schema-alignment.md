# Credentials Schema Alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `credentials.toml` use the same root profile hierarchy as `config.toml`, while preserving encryption, precedence, CLI behavior, and strict ADrive auth-mode selection.

**Architecture:** `CredentialsFile` keeps `schema_version` as a reserved scalar and flattens profile names into the TOML root. Each credential profile stores shared AK/SK fields directly, TOS and ve-tos AK/SK in matching child tables, and ADrive AK/SK plus an OAuth child table. The unreleased `[profiles.<name>.<surface>.<family>]` representation is rejected without migration or fallback.

**Tech Stack:** Rust, serde, toml, existing AES-256-GCM helpers, Cargo unit and integration tests.

---

### Task 1: Lock the canonical on-disk contract with failing tests

**Files:**
- Modify: `crates/tos-core/src/infra/credentials.rs`
- Modify: `tests/credentials_test.rs`
- Modify: `tests/config_test.rs`

- [x] **Step 1: Change the encrypted round-trip assertions to require config-aligned tables**

Add assertions equivalent to:

```rust
assert!(raw.contains("[default.tos]"));
assert!(raw.contains("[default.ve-tos]"));
assert!(raw.contains("[default.adrive]"));
assert!(!raw.contains("[profiles."));
assert!(!raw.contains(".aksk]"));
```

- [x] **Step 2: Add a test proving the old private schema is rejected**

```rust
#[test]
fn legacy_private_credentials_schema_is_not_accepted() {
    let path = write_credentials(
        "schema_version = 1\n[profiles.default.adrive.aksk]\naccess_key_id = \"ak\"\n",
    );
    assert!(CredentialsFile::load_from(&path).is_err());
}
```

- [x] **Step 3: Update OAuth fixtures to the canonical child table**

Use:

```toml
schema_version = 1
[default.adrive.oauth]
access_token = "FILE_ACCESS_TOKEN"
refresh_token = "FILE_REFRESH_TOKEN"
```

- [x] **Step 4: Run focused tests and verify RED**

Run:

```bash
cargo test -p tos-core --lib infra::credentials::tests
cargo test --test credentials_test
```

Expected: failures show the serializer still emits `[profiles.default.*.aksk]` and the loader still accepts the old representation.

### Task 2: Refactor the credentials data model

**Files:**
- Modify: `crates/tos-core/src/infra/credentials.rs`

- [x] **Step 1: Replace the nested family wrappers with config-aligned structures**

Use explicit fields so `deny_unknown_fields` continues catching misspellings:

```rust
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AdriveCredentials {
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    security_token: Option<String>,
    oauth: Option<StoredOAuthCredentials>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CredentialProfile {
    access_key_id: Option<String>,
    secret_access_key: Option<String>,
    security_token: Option<String>,
    tos: Option<StoredAkskCredentials>,
    #[serde(rename = "ve-tos")]
    ve_tos: Option<StoredAkskCredentials>,
    adrive: Option<AdriveCredentials>,
}
```

- [x] **Step 2: Flatten profiles beside the reserved schema version**

```rust
#[derive(Clone, Deserialize, Serialize)]
pub struct CredentialsFile {
    schema_version: u32,
    #[serde(flatten)]
    profiles: BTreeMap<String, CredentialProfile>,
}
```

Reject the reserved profile name `schema_version` in setters and reject the old `profiles` key during parsing.

- [x] **Step 3: Adapt get/set/encrypt helpers**

Keep the existing public APIs:

```rust
set_aksk_field(profile_name, section, field, value)
effective_aksk(profile_name, section, path)
adrive_oauth(profile_name, path)
set_adrive_oauth(profile_name, credentials)
clear_adrive_oauth(profile_name)
```

Shared AK/SK remains inherited only by TOS and ve-tos. ADrive AK/SK and OAuth remain independent and can coexist.

- [x] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
cargo test -p tos-core --lib infra::credentials::tests
cargo test --test credentials_test
```

Expected: all focused tests pass with no plaintext credential output.

### Task 3: Align CLI inspection and dry-run paths

**Files:**
- Modify: `crates/tos/src/handler/config.rs`
- Modify: `crates/adrive/src/handler/meta.rs`
- Modify: `tests/config_test.rs`
- Modify: `tests/credentials_test.rs`

- [x] **Step 1: Update expected section names**

Map sections as follows:

```text
Shared  -> [<profile>]
Tos     -> [<profile>.tos]
VeTos   -> [<profile>.ve-tos]
ADrive  -> [<profile>.adrive]
OAuth   -> [<profile>.adrive.oauth]
```

- [x] **Step 2: Update successful write and dry-run responses**

Remove the `profiles` and `aksk` path components from `section` and plan strings without changing command names, redaction, output envelopes, or target file paths.

- [x] **Step 3: Run config and CLI regression tests**

Run:

```bash
cargo test --test config_test
cargo test --test credentials_test
cargo test --test cli_basic
```

Expected: all tests pass and no output contains raw AK/SK or OAuth tokens.

### Task 4: Review and full verification

**Files:**
- Review all files changed by Tasks 1–3.

- [x] **Step 1: Run formatting and compilation checks**

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
git diff --check
```

- [x] **Step 2: Run the complete workspace test suite**

```bash
cargo test --workspace --all-targets
```

- [x] **Step 3: Perform the mandatory Reviewer pass**

Check correctness, malformed TOML handling, reserved-name validation, secret leakage, file permissions, atomic writes, precedence, ADrive mode isolation, maintainability, and test coverage. Fix every Critical or Major issue with a `// [Review Fix #N]` explanation and rerun the relevant tests.

- [x] **Step 4: Confirm intentional non-compatibility**

Verify a file containing `[profiles.default.adrive.aksk]` fails deterministically and is neither rewritten nor migrated.
