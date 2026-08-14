# ADrive Create Parameters and OAuth Space Listing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add authentication-aware ADrive creation parameters, route OAuth Space listings to the IDS user/group APIs, and make device login output URL-only without changing existing AK/SK behavior.

**Architecture:** Keep the three Space listing endpoints as explicit Resource client methods and expose only a read-only authentication-kind query to the handler. Put option compatibility/default decisions in small pure handler helpers, normalize the two OAuth response shapes into one bounded result, and preserve the group root Space separately from pagination.

**Tech Stack:** Rust 2021, clap derive, serde/serde_json, reqwest, tokio, existing in-process TCP test server.

---

## File Map

- `crates/adrive/src/cli/high_level.rs`: public enum flags and command help.
- `crates/adrive/src/domain/types.rs`: IDS request/response wire types.
- `crates/adrive/src/domain/client.rs`: explicit Resource API methods and request-path tests.
- `crates/adrive/src/handler/high_level.rs`: auth-aware defaults, validation, listing selection, pagination, and output.
- `crates/adrive/src/handler/auth.rs`: URL-only login instructions.
- `crates/adrive/src/handler/meta.rs`: describe/help request-plan accuracy.
- `crates/adrive/Cargo.toml`, `Cargo.toml`, `Cargo.lock`: remove the unused QR dependency.

### Task 1: Define CLI and wire contracts

**Files:**
- Modify: `crates/adrive/src/cli/high_level.rs`
- Modify: `crates/adrive/src/domain/types.rs`

- [ ] **Step 1: Write failing enum and serialization tests**

Add tests proving lower-case clap values and exact IDS JSON fields:

```rust
assert_eq!(OwnerType::from_str("user", true).unwrap(), OwnerType::User);
assert!(OwnerType::from_str("User", false).is_err());
assert_eq!(serde_json::to_value(CreateInstanceInput {
    name: "instance".into(),
    service_type: Some("paas".into()),
    ..Default::default()
}).unwrap()["ServiceType"], "paas");
assert_eq!(serde_json::to_value(CreateSpaceInput {
    instance_id: "instance".into(),
    space_name: "space".into(),
    owner_type: Some("group".into()),
    owner_id: Some("owner".into()),
    ..Default::default()
}).unwrap()["OwnerId"], "owner");
```

- [ ] **Step 2: Run the focused tests and verify RED**

Run: `cargo test -p ve-adrive-cli-core owner_type -- --nocapture`

Expected: compilation fails because `OwnerType` and the new wire fields do not exist.

- [ ] **Step 3: Add minimal CLI enums and fields**

Implement documented `ServiceType::{Saas, Paas, Arkclaw}` and `OwnerType::{User, Group}` value enums with `as_str()` methods. Add `service_type`, `owner_type`, and `owner_id` to `CreateArgs`; add `owner_type` to `LsArgs`. Add optional `service_type` to `CreateInstanceInput`, optional `owner_type/owner_id` to `CreateSpaceInput`, and SDK-parity `ListMySpacesInput/Output` plus `ListMyGroupSpacesInput/Output` types.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run: `cargo test -p ve-adrive-cli-core owner_type -- --nocapture`

Expected: all matching tests pass.

### Task 2: Add explicit OAuth Space Resource methods

**Files:**
- Modify: `crates/adrive/src/domain/client.rs`

- [ ] **Step 1: Write failing HTTP contract tests**

Use the existing local TCP server to assert:

```rust
client.list_my_spaces(&ListMySpacesInput {
    instance_id: "inst-1".into(),
    limit: Some(10),
    marker: Some("next marker".into()),
}).await.unwrap();
assert!(request.starts_with("GET /v1/instances/inst-1/myspaces?"));
assert!(request.contains("limit=10"));
assert!(request.contains("marker=next+marker") || request.contains("marker=next%20marker"));
```

Add the equivalent assertion for `/mygroupspaces`, and verify `RootSpace` deserializes separately.

- [ ] **Step 2: Run the client tests and verify RED**

Run: `cargo test -p ve-adrive-cli-core domain::client::tests::list_my -- --nocapture`

Expected: compilation fails because the methods are missing.

- [ ] **Step 3: Implement the minimal client surface**

Add:

```rust
pub(crate) fn uses_oauth(&self) -> bool {
    matches!(&self.inner.auth, RequestAuth::OAuth(_))
}

pub async fn list_my_spaces(
    &self,
    input: &ListMySpacesInput,
) -> Result<ListMySpacesOutput> {
    let query = pagination_query(input.limit, input.marker.as_deref());
    self.do_json(
        Method::GET,
        &format!("/v1/instances/{}/myspaces", input.instance_id),
        optional_query(&query),
        None::<&()>,
    ).await
}

pub async fn list_my_group_spaces(
    &self,
    input: &ListMyGroupSpacesInput,
) -> Result<ListMyGroupSpacesOutput> {
    let query = pagination_query(input.limit, input.marker.as_deref());
    self.do_json(
        Method::GET,
        &format!("/v1/instances/{}/mygroupspaces", input.instance_id),
        optional_query(&query),
        None::<&()>,
    ).await
}
```

Register both outputs with `HasResponseInfo`. Do not alter `list_spaces`.

- [ ] **Step 4: Run client tests and verify GREEN**

Run: `cargo test -p ve-adrive-cli-core domain::client::tests -- --nocapture`

Expected: all client tests pass.

### Task 3: Apply create defaults and validation

**Files:**
- Modify: `crates/adrive/src/handler/high_level.rs`

- [ ] **Step 1: Write failing pure policy tests**

Cover these exact cases:

```rust
assert_eq!(effective_service_type(false, None), "arkclaw");
assert_eq!(effective_service_type(true, None), "paas");
assert_eq!(effective_owner_type(true, None).unwrap(), Some("user"));
assert!(validate_oauth_space_owner(true, None).is_err());
assert_eq!(effective_owner_type(false, None).unwrap(), None);
```

Also assert Instance targets reject owner flags and Space targets reject `--service-type` before any client call.

- [ ] **Step 2: Run policy tests and verify RED**

Run: `cargo test -p ve-adrive-cli-core handler::high_level::tests::create_ -- --nocapture`

Expected: compilation fails because the policy helpers are missing.

- [ ] **Step 3: Implement mode-aware request construction**

Use `client.uses_oauth()` after the existing provider is constructed. Set `CreateInstanceInput.service_type` to the explicit value or mode default. For Space creation, preserve omitted AK/SK fields, default OAuth owner type to `user`, trim/reject an empty OAuth owner ID, and serialize explicit lower-case enum values.

- [ ] **Step 4: Run policy and serialization tests and verify GREEN**

Run: `cargo test -p ve-adrive-cli-core create_ -- --nocapture`

Expected: all matching tests pass.

### Task 4: Route OAuth Space listings

**Files:**
- Modify: `crates/adrive/src/handler/high_level.rs`
- Modify: `crates/adrive/src/handler/meta.rs`

- [ ] **Step 1: Write failing selection and pagination tests**

Define a pure selection enum and assert:

```rust
assert_eq!(space_listing_kind(false, None).unwrap(), SpaceListingKind::All);
assert_eq!(space_listing_kind(true, None).unwrap(), SpaceListingKind::User);
assert_eq!(space_listing_kind(true, Some(OwnerType::Group)).unwrap(), SpaceListingKind::Group);
assert!(space_listing_kind(false, Some(OwnerType::User)).is_err());
```

Add async tests proving an OAuth response with only `NextMarker` continues, and `RootSpace` remains separate from the bounded `spaces` count.

- [ ] **Step 2: Run listing tests and verify RED**

Run: `cargo test -p ve-adrive-cli-core space_listing -- --nocapture`

Expected: compilation fails because selection and OAuth pagination do not exist.

- [ ] **Step 3: Implement normalized page fetching**

Add `SpaceListingKind::{All, User, Group}` and a small `fetch_space_page` function. `All` calls the unchanged `list_spaces` and honors its existing `IsTruncated`; `User` and `Group` call the SDK-parity methods and continue only when `NextMarker` is non-empty. Carry the first group `RootSpace` separately in `BoundedSpaces` and expose it as `root_space` in structured output.

Reject `--owner-type` for non-Instance targets before listing calls. Update dry-run/describe request plans so OAuth endpoints are discoverable.

- [ ] **Step 4: Run listing tests and verify GREEN**

Run: `cargo test -p ve-adrive-cli-core space_listing -- --nocapture`

Expected: all matching tests pass.

### Task 5: Make OAuth login URL-only

**Files:**
- Modify: `crates/adrive/src/handler/auth.rs`
- Modify: `crates/adrive/Cargo.toml`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write a failing instruction-format test**

Extract a pure formatter and assert its output contains only the complete URL and waiting line:

```rust
let lines = login_instruction_lines(&authorization);
assert_eq!(lines, vec![
    "Open this URL to authorize ve-adrive-cli:".to_string(),
    authorization.verification_uri_complete.clone(),
    "Waiting for authorization…".to_string(),
]);
assert!(!lines.join("\n").contains(&authorization.user_code));
```

- [ ] **Step 2: Run auth tests and verify RED**

Run: `cargo test -p ve-adrive-cli-core login_instruction -- --nocapture`

Expected: compilation fails because the pure formatter is missing.

- [ ] **Step 3: Implement URL-only output and remove QR dependency**

Print the formatter lines from `show_login_instructions`, remove `QrCode`, TTY branching, and the now-unused `global` parameter. Remove `qrcode` from the crate/workspace dependency lists and regenerate the lockfile with Cargo.

- [ ] **Step 4: Run auth tests and verify GREEN**

Run: `cargo test -p ve-adrive-cli-core handler::auth::tests -- --nocapture`

Expected: all auth tests pass and no QR dependency is compiled for ADrive.

### Task 6: Regression, review, and commit

**Files:**
- Verify all modified files above.

- [ ] **Step 1: Run formatting and focused regression**

Run:

```bash
cargo fmt --all -- --check
cargo test -p ve-adrive-cli-core
cargo check --workspace
```

Expected: all commands exit 0 without new warnings.

- [ ] **Step 2: Perform the mandatory fresh-eyes review**

Review correctness, security, performance, maintainability, robustness, testability, and observability. In particular verify no token/owner identifiers are logged, no AK/SK endpoint changed, and no new option is accepted in a silently ignored context.

- [ ] **Step 3: Fix every Critical/Major finding and rerun verification**

Any required review fix receives a `// [Review Fix #N]` why-comment per repository instructions, followed by the full commands from Step 1.

- [ ] **Step 4: Commit the implementation independently**

```bash
git add Cargo.toml Cargo.lock crates/adrive docs/superpowers/plans/2026-08-03-adrive-create-and-oauth-space-listing.md
git commit -m "feat: add adrive oauth space ownership flows"
```
