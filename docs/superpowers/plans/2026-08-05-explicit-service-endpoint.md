# Explicit Service Endpoint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Preserve endpoint-to-region parsing while removing every runtime region-to-endpoint fallback, retaining only the explicit `ve-tos config init` Beijing pair.

**Architecture:** Keep source precedence in the existing profile builders. Enforce connection-mode completeness when constructing clients: a static resource connection needs endpoint plus an explicit or parsed region, while ByteTOS PSM remains an endpoint-free alternative with explicit region and PSM. Resolve the OAuth Authorization Server independently and require one of its three explicit sources.

**Tech Stack:** Rust 2021, clap, serde/TOML configuration, reqwest clients, Cargo unit and integration tests.

---

## File map

- `crates/tos/src/handler/config.rs`: surface-specific `config init` and dry-run output.
- `crates/tos-core/src/infra/client.rs`: TOS endpoint/region validation and PSM-only endpoint-free path.
- `crates/adrive/src/domain/client.rs`: IDS resource endpoint/region validation.
- `crates/adrive/src/handler/common.rs`: ADrive configuration-error classification and fix commands.
- `crates/adrive/src/handler/auth.rs`: explicit OAuth auth endpoint resolution.
- `crates/tos/src/handler/meta.rs`, `crates/adrive/src/handler/meta.rs`: Doctor output aligned with explicit endpoints.
- `tests/config_test.rs`: public CLI regression coverage.
- `README.md`, `skills/*/SKILL.md`, and relevant CLI help: user- and Agent-facing contract.

### Task 1: Make `config init` defaults surface-specific

**Files:**
- Modify: `tests/config_test.rs`
- Modify: `crates/tos/src/handler/config.rs`

- [x] **Step 1: Write failing integration tests**

Add a `tos config init` test that reads `~/.tos/config.toml` and asserts that
`[default.tos]` exists while `region`, `endpoint`, and `psm` are absent. Extend
the existing `ve-tos config init` test to assert the exact Beijing region and
endpoint. Add a `tos config init --dry-run` assertion that its plan does not
claim to write network defaults.

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test --test config_test test_tos_config_init_omits_network_defaults -- --exact
```

Expected: FAIL because `tos config init` currently writes `cn-beijing` and
`tos-cn-beijing.volces.com`.

- [x] **Step 3: Implement the minimal surface branch**

In `handle_init`, set shared region and the binary endpoint only inside the
`Binary::VeTos` branch. Always create the active binary override and retain its
non-network operational defaults. Apply the same branch to the dry-run plan:

```rust
let active_binary = active_tos_config_binary();
if active_binary == Binary::VeTos {
    if p.region.is_none() {
        p.region = Some("cn-beijing".to_string());
    }
}
let tos_override = match active_binary {
    Binary::VeTos => p.ve_tos.get_or_insert_with(TosOverride::default),
    _ => p.tos.get_or_insert_with(TosOverride::default),
};
if active_binary == Binary::VeTos && tos_override.endpoint.is_none() {
    tos_override.endpoint = Some("tos-cn-beijing.volces.com".to_string());
}
```

- [x] **Step 4: Verify GREEN**

Run the three focused config-init tests, then `cargo test --test config_test
config_init` and expect PASS.

### Task 2: Remove the TOS region-to-endpoint runtime fallback

**Files:**
- Modify: `crates/tos-core/src/infra/client.rs`
- Modify: `crates/tos/src/handler/meta.rs`

- [x] **Step 1: Write failing client tests**

Add tests proving that a TOS4 client with only region returns
`ConfigMissing("endpoint is required...")`, a recognizable endpoint still
supplies region, and a ByteTOS V1 profile with explicit region plus PSM still
constructs successfully without endpoint.

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test -p tos-core infra::client::tests::tos4_requires_explicit_endpoint -- --exact
```

Expected: FAIL because `service_endpoint()` currently calls
`build_endpoint(region, service)`.

- [x] **Step 3: Enforce static endpoint or PSM**

Normalize the explicit endpoint, build the PSM resolver, and reject a missing
endpoint when no PSM resolver exists:

```rust
let endpoint = profile.endpoint.as_deref().map(normalize_endpoint_scheme);
let psm_resolver =
    build_psm_resolver(profile, service, sign_algorithm, endpoint.is_none())?;
if endpoint.is_none() && psm_resolver.is_none() {
    return Err(CliError::ConfigMissing(
        "endpoint is required; configure --endpoint, the active profile endpoint, or the surface-specific endpoint environment variable"
            .to_string(),
    ));
}
```

Use a non-routable internal URL such as `http://psm.invalid` only as the
pre-resolution request shape for an active PSM resolver. It must never be used
when the resolver is absent and must never be sent over the network.

- [x] **Step 4: Align TOS Doctor**

Remove `default_endpoint_for_region`. Offline and live network checks pass only
with an explicit endpoint; ByteTOS PSM mode reports that no static endpoint
probe is available instead of inventing one. Config checks treat endpoint mode
as complete only when endpoint exists and region is explicit or parseable.

- [x] **Step 5: Verify GREEN**

Run:

```bash
cargo test -p tos-core infra::client::tests
cargo test -p ve-tos-cli-core handler::meta::tests
```

Expected: PASS with endpoint-only parsing and PSM behavior unchanged.

### Task 3: Require an explicit ADrive resource endpoint

**Files:**
- Modify: `crates/adrive/src/domain/client.rs`
- Modify: `crates/adrive/src/handler/common.rs`
- Modify: `crates/adrive/src/handler/meta.rs`
- Modify: `tests/config_test.rs`

- [x] **Step 1: Replace the old resolver expectation with a failing test**

Change `endpoint_defaults_to_region_scoped_ids_host` into
`endpoint_is_required_even_when_region_is_configured`. Assert that
`resolve_endpoint_and_region(None, Some("cn-beijing"))` returns an error naming
`ADRIVE_ENDPOINT`. Retain the endpoint-only region-parsing test.

- [x] **Step 2: Verify RED**

Run:

```bash
cargo test -p ve-adrive-cli-core domain::client::tests::endpoint_is_required_even_when_region_is_configured -- --exact
```

Expected: FAIL because the resolver currently builds
`https://ids-cn-beijing.volces.com`.

- [x] **Step 3: Implement endpoint-first resolution**

Resolve endpoint before region and remove `build_ids_endpoint` from runtime:

```rust
let endpoint = endpoint
    .map(|value| normalize_endpoint_scheme(&value))
    .ok_or_else(|| Error::client(
        "ADRIVE_ENDPOINT is required; configure --endpoint, [profile.adrive].endpoint, or ADRIVE_ENDPOINT",
    ))?;
let region = region
    .or_else(|| derive_region_from_endpoint(&endpoint))
    .ok_or_else(|| Error::client(
        "ADRIVE_REGION is required when region cannot be derived from ADRIVE_ENDPOINT",
    ))?;
Ok((endpoint, region))
```

Map both missing endpoint and missing region to `config_missing`. Update ADrive
Doctor to recommend `ve-adrive config set endpoint <endpoint>` and stop saying
the endpoint was derived from region.

- [x] **Step 4: Verify GREEN**

Run the ADrive client tests and focused Doctor/config integration tests. Expect
endpoint-only parsing to pass and region-only configuration to fail with the
endpoint-specific fix command.

### Task 4: Require an explicit OAuth Authorization Server

**Files:**
- Modify: `crates/adrive/src/handler/auth.rs`
- Modify: `crates/adrive/src/domain/auth.rs`
- Modify: `crates/adrive/src/handler/common.rs`
- Modify: `crates/adrive/src/cli/auth.rs`
- Modify: `tests/config_test.rs`

- [x] **Step 1: Write failing resolver and guidance tests**

Add a login-settings test with Instance configured but no CLI/config/env auth
endpoint. Assert `oauth_auth_endpoint_required`, a fix command of
`ve-adrive config set auth_endpoint <url>`, and Doctor hint
`ve-adrive doctor --check config`. Keep tests for each explicit source and its
existing precedence.

- [x] **Step 2: Verify RED**

Run the focused handler tests. Expected: FAIL because the built-in IDS Auth URL
currently makes the resolution succeed.

- [x] **Step 3: Remove the fallback and add actionable guidance**

Remove `DEFAULT_AUTH_ENDPOINT` from login resolution and return:

```rust
CliError::ConfigMissing(
    "[oauth_auth_endpoint_required] ADrive OAuth auth endpoint is required; use --auth-endpoint, [profile.adrive].auth_endpoint, or ADRIVE_AUTH_ENDPOINT"
        .to_string(),
)
```

Teach `adrive_error_guidance` the stable code and update `--help` text to say
the option is required unless config or environment supplies it. Do not change
refresh: it continues using the endpoint stored with the OAuth credential.

- [x] **Step 4: Verify GREEN**

Run all `handler::auth` tests and the error-envelope integration test. Expect
new login without auth endpoint to fail locally before network access, while a
stored endpoint still supports refresh.

### Task 5: Synchronize public documentation and run full review

**Files:**
- Modify: `README.md`
- Modify: `skills/tos-cli/SKILL.md`
- Modify: `skills/ve-tos-cli/SKILL.md`
- Modify: `skills/ve-adrive-cli/SKILL.md`
- Modify: `docs/superpowers/specs/2026-07-31-ve-adrive-cli-oauth-contract-design.md`

- [x] **Step 1: Update documentation assertions or text checks first**

Where automated metadata checks exist, assert that help/describe no longer
advertise an implicit endpoint or auth endpoint. Confirm the three skills show
the required explicit setup commands and the `ve-tos` initialization exception.

- [x] **Step 2: Update user-facing text**

Remove README statements that `ADRIVE_REGION` derives an endpoint and that Auth
Endpoint has a built-in fallback. Mark the earlier OAuth design's default as
superseded by the explicit-endpoint contract. Add concise setup examples:

```bash
tos-cli config set endpoint https://service-host.example
ve-adrive-cli config set endpoint https://ids-cn-beijing.volces.com
ve-adrive-cli config set auth_endpoint https://idsauth.volces.com
```

- [x] **Step 3: Run verification**

Run:

```bash
cargo fmt --all -- --check
cargo test -p tos-core
cargo test -p ve-tos-cli-core
cargo test -p ve-adrive-cli-core
cargo test --test config_test
cargo check --workspace
```

Expected: all commands exit successfully with no new warnings.

- [x] **Step 4: Reviewer pass**

Review the complete diff for correctness, security, performance,
maintainability, robustness, testability, observability, and backward-compatibility
boundaries. Fix every Critical or Major issue, add `[Review Fix #N]` comments
only to changes made because of that review, rerun affected tests, and record
remaining Minor/Suggestion items in the final handoff.

- [x] **Step 5: Commit**

Stage only files belonging to this contract and create a dedicated feature
commit after all verification succeeds.
