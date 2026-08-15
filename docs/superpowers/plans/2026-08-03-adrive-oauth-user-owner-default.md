# ADrive OAuth User Owner Default Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Persist the OAuth Token response `user_id` and use it as the default owner ID for OAuth user-owned Space creation, while synchronizing all user-facing discovery surfaces.

**Architecture:** Extend the existing profile-scoped OAuth credential record with optional identity metadata. Validation and dry-run use a pure owner resolver, while real OAuth Space creation rebuilds its request body from one refresh-aware Token/user snapshot on every attempt. Normal requests use the proactive refresh window; a just-refreshed or 401-replay request accepts the paired Token and user ID until actual expiry. Keep environment-only OAuth and AK/SK semantics unchanged.

**Tech Stack:** Rust, Serde/TOML, Clap, Reqwest OAuth client, Cargo tests, Pytest packaging checks.

---

### Task 1: Persist OAuth user identity through login and refresh

**Files:**
- Modify: `crates/tos-core/src/infra/credentials.rs`
- Modify: `crates/adrive/src/handler/auth.rs`
- Modify: `crates/adrive/src/domain/token_manager.rs`
- Modify: `crates/adrive/src/domain/client.rs` (explicit test initializer only)
- Modify: `crates/adrive/src/handler/common.rs` (explicit test initializer only)

- [x] **Step 1: Write failing credential and login tests**

Extend `oauth_metadata_round_trip` to set and assert `user_id`, and extend the
device-login success test to load the written profile and assert:

```rust
assert_eq!(loaded.user_id.as_deref(), Some("user-1"));
```

Also load a hand-written schema-version-1 OAuth table without `user_id` and
assert `loaded.user_id == None`.

- [x] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test -p tos-core oauth_metadata_round_trip
cargo test -p ve-adrive-cli-core handler::auth::tests::login_polls_and_persists_complete_token_pair
```

Expected: compilation or assertion failure because `StoredOAuthCredentials`
does not yet expose or persist `user_id`.

- [x] **Step 3: Add optional credential metadata and login persistence**

Add this field beside `instance_id`:

```rust
/// OAuth subject returned by the Token endpoint.
#[serde(skip_serializing_if = "Option::is_none")]
pub user_id: Option<String>,
```

Include it in `StoredOAuthCredentials::merge`. In `validated_login`, normalize
the response value once and reuse it for both saved output and credentials:

```rust
let user_id = token.user_id.and_then(nonempty_string);
let saved = SavedLogin {
    expires_at: expires_at.clone(),
    scope: scope.clone(),
    user_id: user_id.clone(),
};
// StoredOAuthCredentials { user_id, ... }
```

Use a small local helper that trims strings and maps blank values to `None`.

- [x] **Step 4: Write failing refresh lifecycle tests**

Add one refresh response containing `"user_id":"user-new"` and assert that it
replaces `user-old`. Add another response without `user_id` and assert that
`user-old` remains stored. Add file/environment source tests for a new
`bound_user_id()` method.

- [x] **Step 5: Run refresh tests and verify RED**

Run:

```bash
cargo test -p ve-adrive-cli-core token_manager::tests::refresh
cargo test -p ve-adrive-cli-core token_manager::tests::file_credentials_expose_bound_user_id
```

Expected: failure because refresh reconstruction drops identity metadata and
the accessor does not exist.

- [x] **Step 6: Preserve and expose user identity during refresh**

Add `OAuthTokenManager::bound_user_id()` with the same source-selection rules
as `bound_instance_id()`: file credentials return normalized metadata,
environment credentials return `None`, and a missing environment Access Token
keeps the existing login-required precedence.

Carry `user_id` through `ValidatedRefreshToken`. When saving refreshed tokens,
select:

```rust
let user_id = validated.user_id.or(stored_user_id);
```

Pass the previous stored user ID into `save_refreshed_token` so omission by the
Refresh response cannot erase it.

- [x] **Step 7: Run Task 1 tests and verify GREEN**

Run:

```bash
cargo test -p tos-core infra::credentials
cargo test -p ve-adrive-cli-core handler::auth
cargo test -p ve-adrive-cli-core token_manager
```

Expected: all selected tests pass.

### Task 2: Resolve OAuth Space ownership from credentials

**Files:**
- Modify: `crates/adrive/src/domain/client.rs`
- Modify: `crates/adrive/src/domain/token_manager.rs`
- Modify: `crates/adrive/src/handler/high_level.rs`

- [x] **Step 1: Write failing pure owner-resolution tests**

Replace the old unconditional-owner test with focused assertions covering:

```rust
let input = build_create_space_input("inst", "space", &args, true, Some("user-1"))?;
assert_eq!(input.owner_type.as_deref(), Some("user"));
assert_eq!(input.owner_id.as_deref(), Some("user-1"));
```

Add separate tests for omitted owner type, explicit `user`, explicit owner ID
override, `group` without an ID, missing stored user metadata, blank IDs, and
unchanged AK/SK omission.

- [x] **Step 2: Run the owner tests and verify RED**

Run:

```bash
cargo test -p ve-adrive-cli-core handler::high_level::tests::create_space
```

Expected: compilation or assertion failure because the builder does not accept
OAuth user metadata and still rejects every missing owner ID.

- [x] **Step 3: Add the client metadata boundary and owner resolver**

Expose selected metadata without exposing the authentication enum:

```rust
pub(crate) fn oauth_user_id(&self) -> Result<Option<String>> {
    match &self.inner.auth {
        RequestAuth::Aksk(_) => Ok(None),
        RequestAuth::OAuth(manager) => manager.bound_user_id().map_err(Error::Cli),
    }
}
```

Change `build_create_space_input` and `prevalidate_create_options` to accept
`oauth_user_id: Option<&str>`. Resolve effective ownership as follows:

```rust
let owner_type = args.owner_type
    .map(OwnerType::as_str)
    .or_else(|| is_oauth.then_some("user"));
let owner_id = match (normalized_owner_id(args.owner_id.as_deref())?, owner_type) {
    (Some(explicit), _) => Some(explicit),
    (None, Some("user")) if is_oauth => normalized_stored_user_id(oauth_user_id),
    (None, Some("group")) if is_oauth => return Err(group_owner_id_required()),
    (None, _) => None,
};
```

If OAuth user ownership still has no ID, return a validation error that names
both remedies: `--owner-id` and `ve-adrive auth login`.

- [x] **Step 4: Use the same metadata in real execution and dry-run**

Add a pure predicate that requests metadata only for a structurally valid
Space target whose effective OAuth owner type is `user` and whose explicit
owner ID is absent. Dry-run constructs an `OAuthTokenManager` only for that
predicate, then calls `bound_user_id()` without performing network I/O. Real
execution builds the Space request body on every attempt from one persisted
Token/user snapshot, so a refresh cannot pair a Token with stale ownership
metadata. Initial requests apply the proactive refresh window; snapshots loaded
immediately after refresh and for the one forced-401 replay only need to be
unexpired. Explicit or blank owner IDs, group ownership, AK/SK, Instance
creation, and invalid target-specific options do not load identity metadata and
retain their existing validation precedence.

- [x] **Step 5: Add and run dry-run parity tests**

Add tests using a temporary credentials file that assert OAuth user dry-run
succeeds with stored `user_id`, and that group/missing-user errors match real
validation. The pure builder tests from Step 1 assert the exact `OwnerType` and
`OwnerId` request fields because the existing dry-run envelope does not expose
a request body.

Run:

```bash
cargo test -p ve-adrive-cli-core handler::high_level::tests::oauth_dry_run_space_create
cargo test -p ve-adrive-cli-core handler::high_level::tests::create_space
```

Expected: all selected tests pass and no Resource request is made during
validation.

### Task 3: Synchronize help, describe, skill, and contract documentation

**Files:**
- Modify: `crates/adrive/src/cli/high_level.rs`
- Modify: `crates/adrive/src/domain/oauth.rs`
- Modify: `crates/adrive/src/handler/high_level.rs`
- Modify: `crates/adrive/src/registry.rs`
- Modify: `crates/adrive/src/handler/meta.rs`
- Modify: `README.md`
- Modify: `skills/ve-adrive-cli/SKILL.md`
- Modify: `packaging/tests/test_skills.py`
- Modify: `tests/cli_basic.rs`

- [x] **Step 1: Write failing discovery-surface tests**

Add CLI tests asserting `ve-adrive crt --help` mentions the OAuth user default
and group requirement. Add a describe assertion that the `owner-id` parameter
description contains conditional user/group semantics. Add a packaging test
asserting the public ADrive skill contains `auth login`, `--owner-type user`,
`--owner-type group`, `--service-type`, and user/group Space-listing examples.

- [x] **Step 2: Run discovery tests and verify RED**

Run:

```bash
cargo test --test cli_basic adrive_create_
python3 -m pytest packaging/tests/test_skills.py -q
```

Expected: assertions fail against the current incomplete help text and generic
ADrive skill.

- [x] **Step 3: Update every discovery surface**

Update the Clap owner-ID documentation and `crt` examples. Change the registry
description from “required for OAuth Space creation” to language equivalent to:

```text
Space owner identifier; OAuth user ownership defaults to the logged-in user,
while OAuth group ownership requires this option
```

Update `handler/meta.rs` routing notes with user-default/group-required
semantics. Extend `skills/ve-adrive-cli/SKILL.md` with concrete OAuth login,
Instance/Space creation, and user/group list commands while retaining the
existing installation and safety instructions.

Update `TokenResponse::user_id` documentation so it states that the field is
persisted as OAuth identity metadata instead of being output-only.

- [x] **Step 4: Run discovery tests and verify GREEN**

Run:

```bash
cargo test --test cli_basic adrive_create_
python3 -m pytest packaging/tests/test_skills.py -q
```

Expected: all selected tests pass.

### Task 4: Review, regression verification, and focused commit

**Files:**
- Review every file changed in Tasks 1-3.

- [x] **Step 1: Format and run focused crate tests**

```bash
cargo fmt --all -- --check
cargo test -p tos-core
cargo test -p ve-adrive-cli-core
python3 -m pytest packaging/tests/test_skills.py -q
```

Expected: all commands exit successfully.

- [x] **Step 2: Run workspace regression tests**

```bash
cargo test --workspace
```

Expected: all workspace tests pass.

- [x] **Step 3: Perform the mandatory Reviewer pass**

Review correctness, credential confidentiality, backward reading, error
precedence, refresh rotation, AK/SK compatibility, environment-only OAuth,
dry-run parity, user-facing discovery drift, performance, and test coverage.
Fix every Critical or Major finding and rerun the affected tests.

- [x] **Step 4: Commit the implementation separately**

The implementation was committed as focused documentation, persistence,
runtime, regression-test, and review-fix commits ending at `51ec156`.
