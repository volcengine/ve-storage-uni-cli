# ADrive OAuth Root Listing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make OAuth `ve-adrive ls` return the one Instance bound to the selected OAuth credential while preserving all existing AK/SK and explicit-target behavior.

**Architecture:** Expose a small, read-only Instance listing scope from the Resource Client: AK/SK can enumerate all Instances, while OAuth can access exactly the persisted bound Instance. The high-level `ls` handler chooses `list_instances` or `get_instance` only for the existing root target and preserves the current output envelope.

**Tech Stack:** Rust, Tokio, Clap, Reqwest, existing `CredentialsFile` and ADrive domain client test helpers.

---

### Task 1: Expose the selected Instance listing scope

**Files:**
- Modify: `crates/adrive/src/domain/token_manager.rs`
- Modify: `crates/adrive/src/domain/client.rs`

- [x] **Step 1: Write failing Token Manager tests**

Add tests proving that file credentials return their persisted non-empty `instance_id`, while environment credentials return no bound Instance:

```rust
#[test]
fn file_credentials_expose_bound_instance_id() {
    let (directory, credentials_path) = credentials_path();
    save_oauth(
        &credentials_path,
        StoredOAuthCredentials {
            access_token: Some("access".to_string()),
            instance_id: Some("inst-1".to_string()),
            ..StoredOAuthCredentials::default()
        },
    );
    let manager = file_manager(credentials_path);

    assert_eq!(manager.bound_instance_id().unwrap().as_deref(), Some("inst-1"));
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn environment_credentials_have_no_bound_instance_id() {
    let (directory, credentials_path) = credentials_path();
    let manager = OAuthTokenManager::new_with_environment(
        credentials_path,
        "default".to_string(),
        ClientOptions::default(),
        Some("access".to_string()),
        None,
    )
    .unwrap();

    assert_eq!(manager.bound_instance_id().unwrap(), None);
    let _ = std::fs::remove_dir_all(directory);
}
```

- [x] **Step 2: Run tests and verify the missing method failure**

Run: `cargo test -p ve-adrive-cli-core domain::token_manager::tests`

Expected: compilation fails because `OAuthTokenManager::bound_instance_id` does not exist.

- [x] **Step 3: Implement read-only binding lookup**

Add this method to `OAuthTokenManager`; it must never consult ordinary Profile defaults:

```rust
/// Return the persisted OAuth Instance binding, when the selected credential source has one.
pub(crate) fn bound_instance_id(&self) -> Result<Option<String>, CliError> {
    match &self.source {
        CredentialSource::Environment { access_token: None } => {
            return Err(login_required("environment Access Token is missing"));
        }
        CredentialSource::Environment { .. } => return Ok(None),
        CredentialSource::File => {}
    }
    let stored = load_oauth(&self.credentials_path, &self.profile_name)?;
    Ok(nonempty(stored.instance_id))
}
```

The focused tests must also assert that an environment source without an Access Token returns `login_required`, preserving the existing `auth login` remediation before root Instance resolution.

- [x] **Step 4: Add Resource Client scope tests and implementation**

Define the crate-visible scope and method in `domain/client.rs`:

```rust
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InstanceListingScope {
    All,
    Bound(String),
    UnknownOAuthBinding,
}

pub(crate) fn instance_listing_scope(&self) -> Result<InstanceListingScope> {
    match &self.inner.auth {
        RequestAuth::Aksk(_) => Ok(InstanceListingScope::All),
        RequestAuth::OAuth(manager) => manager
            .bound_instance_id()
            .map(|instance_id| {
                instance_id.map_or(
                    InstanceListingScope::UnknownOAuthBinding,
                    InstanceListingScope::Bound,
                )
            })
            .map_err(Error::Cli),
    }
}
```

Add these tests using the existing `oauth_manager`, `no_retry_options`, and temporary credential helpers:

```rust
#[test]
fn aksk_client_can_list_all_instances() {
    let client = Client::new(
        "access-key".to_string(),
        "secret-key".to_string(),
        None,
        Some("https://resource.example.com".to_string()),
        Some("test-region".to_string()),
        no_retry_options(),
    )
    .unwrap();

    assert_eq!(client.instance_listing_scope().unwrap(), InstanceListingScope::All);
}

#[test]
fn oauth_client_lists_only_its_bound_instance() {
    let (directory, manager) = oauth_manager(
        "https://auth.example.com",
        "access-current",
        "refresh-current",
        "inst-1",
    );
    let client = Client::new_oauth(
        manager,
        Some("https://resource.example.com".to_string()),
        Some("test-region".to_string()),
        no_retry_options(),
    )
    .unwrap();

    assert_eq!(
        client.instance_listing_scope().unwrap(),
        InstanceListingScope::Bound("inst-1".to_string())
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn oauth_environment_client_has_unknown_instance_binding() {
    let directory = std::env::temp_dir().join(format!(
        "ve-adrive-resource-oauth-environment-{}-{}",
        std::process::id(),
        ulid::Ulid::new()
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let manager = OAuthTokenManager::new_with_environment(
        directory.join("credentials.toml"),
        "default".to_string(),
        no_retry_options(),
        Some("access-current".to_string()),
        None,
    )
    .unwrap();
    let client = Client::new_oauth(
        manager,
        Some("https://resource.example.com".to_string()),
        Some("test-region".to_string()),
        no_retry_options(),
    )
    .unwrap();

    assert_eq!(
        client.instance_listing_scope().unwrap(),
        InstanceListingScope::UnknownOAuthBinding
    );
    let _ = std::fs::remove_dir_all(directory);
}
```

- [x] **Step 5: Run focused domain tests**

Run: `cargo test -p ve-adrive-cli-core domain::token_manager::tests`

Run: `cargo test -p ve-adrive-cli-core domain::client::tests`

Expected: all Token Manager and Resource Client tests pass.

### Task 2: Route root `ls` without changing explicit targets

**Files:**
- Modify: `crates/adrive/src/handler/high_level.rs`
- Modify: `crates/adrive/src/handler/common.rs`

- [x] **Step 1: Write failing high-level decision tests**

Add a pure decision helper test matrix for root listing scope:

```rust
#[test]
fn oauth_bound_root_listing_selects_single_instance() {
    assert_eq!(
        resolve_root_instance_listing(
            InstanceListingScope::Bound("inst-1".to_string()),
            None,
        )
        .unwrap(),
        RootInstanceListing::Single("inst-1".to_string())
    );
}

#[test]
fn aksk_root_listing_keeps_collection_behavior() {
    assert_eq!(
        resolve_root_instance_listing(InstanceListingScope::All, None).unwrap(),
        RootInstanceListing::Collection
    );
}

#[test]
fn oauth_unknown_binding_and_marker_fail_before_requests() {
    let missing = resolve_root_instance_listing(
        InstanceListingScope::UnknownOAuthBinding,
        None,
    )
    .unwrap_err();
    assert!(missing.to_string().contains("oauth_instance_required"));

    let marker = resolve_root_instance_listing(
        InstanceListingScope::Bound("inst-1".to_string()),
        Some("next"),
    )
    .unwrap_err();
    assert!(marker.to_string().contains("does not support --marker"));
}
```

- [x] **Step 2: Run the focused test and verify it fails**

Run: `cargo test -p ve-adrive-cli-core handler::high_level::tests::oauth_bound_root_listing_selects_single_instance`

Expected: compilation fails because the decision types and helper do not exist.

- [x] **Step 3: Implement the root listing decision and fetch path**

Add `RootInstanceListing`, `resolve_root_instance_listing`, and replace the direct root call with `list_root_instances_bounded`:

```rust
#[derive(Debug, Eq, PartialEq)]
enum RootInstanceListing {
    Collection,
    Single(String),
}

fn resolve_root_instance_listing(
    scope: InstanceListingScope,
    marker: Option<&str>,
) -> Result<RootInstanceListing, CliError> {
    match scope {
        InstanceListingScope::All => Ok(RootInstanceListing::Collection),
        InstanceListingScope::Bound(_) if marker.is_some_and(|value| !value.is_empty()) => {
            Err(CliError::ValidationError(
                "OAuth root ls does not support --marker".to_string(),
            ))
        }
        InstanceListingScope::Bound(instance_id) => Ok(RootInstanceListing::Single(instance_id)),
        InstanceListingScope::UnknownOAuthBinding => Err(CliError::ConfigMissing(
            "[oauth_instance_required] OAuth credentials do not identify the bound ADrive Instance; provide --instance or adrive://<instance-id>".to_string(),
        )),
    }
}

async fn list_root_instances_bounded(
    client: &IdsClient,
    max_keys: i32,
    marker: Option<&str>,
) -> Result<BoundedInstances, CliError> {
    let scope = client.instance_listing_scope().map_err(map_ids_error)?;
    match resolve_root_instance_listing(scope, marker)? {
        RootInstanceListing::Collection => list_instances_bounded(client, max_keys, marker).await,
        RootInstanceListing::Single(instance_id) => {
            let output = client
                .get_instance(&GetInstanceInput::new(instance_id))
                .await
                .map_err(map_ids_error)?;
            Ok(BoundedInstances {
                instances: vec![output.instance],
                next_marker: None,
                is_truncated: false,
                request_id: output.response_info.request_id().to_string(),
            })
        }
    }
}
```

The existing `ADriveTarget::Instances` rendering stays unchanged; only replace `list_instances_bounded` with `list_root_instances_bounded`.

- [x] **Step 4: Make the missing-binding repair command context-aware**

Before generic OAuth authentication guidance, return this guidance when `command_path == "ve-adrive ls"` and the semantic code is `oauth_instance_required`:

```rust
guidance(
    "Specify the ADrive Instance bound to the OAuth Access Token",
    Some("ve-adrive ls --instance <instance_id>"),
    Some("ve-adrive doctor --check auth"),
)
```

Add a test asserting the `fix_command` above, while the existing `auth login` missing-instance guidance remains unchanged.

- [x] **Step 5: Run focused handler tests**

Run: `cargo test -p ve-adrive-cli-core handler::high_level::tests`

Run: `cargo test -p ve-adrive-cli-core handler::common::tests`

Expected: all high-level and error-guidance tests pass.

### Task 3: Review and regression verification

**Files:**
- Review: `crates/adrive/src/domain/token_manager.rs`
- Review: `crates/adrive/src/domain/client.rs`
- Review: `crates/adrive/src/handler/high_level.rs`
- Review: `crates/adrive/src/handler/common.rs`

- [x] **Step 1: Run formatting and static checks**

Run: `cargo fmt --all -- --check`

Expected: exit code 0.

Run: `cargo check --workspace`

Expected: exit code 0.

- [x] **Step 2: Run ADrive and workspace regression tests**

Run: `cargo test -p ve-adrive-cli-core`

Expected: all ADrive tests pass.

Run: `cargo test --workspace`

Expected: all workspace tests pass.

- [x] **Step 3: Perform the mandatory Reviewer pass**

Review correctness, security, performance, maintainability, robustness, testability, and observability. In particular verify that AK/SK root listing still calls `list_instances`, explicit OAuth targets still use their original Space/File paths, Token values never enter output, and missing bindings fail before network access. Fix every Critical or Major finding and rerun the affected checks.

- [ ] **Step 4: Commit only the scoped implementation**

Run:

```bash
git add crates/adrive/src/domain/token_manager.rs \
  crates/adrive/src/domain/client.rs \
  crates/adrive/src/handler/high_level.rs \
  crates/adrive/src/handler/common.rs \
  docs/superpowers/plans/2026-08-01-adrive-oauth-root-ls.md
git commit -m "fix: scope adrive oauth root listing"
```

Expected: one implementation commit; unrelated `docs/development_workflow.md` remains untracked.
