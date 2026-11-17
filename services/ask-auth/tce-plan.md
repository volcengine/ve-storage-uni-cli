> 历史方案，已由 [tcc-merge-plan.md](tcc-merge-plan.md) 取代。当前仅有 `services/ask-auth` 一个包和二进制，使用根工作区公开依赖及根 Cargo.lock；以下独立工作区/内部 SDK 设想不再适用。

# Ask authorization TCC and TCE implementation plan

> **For agentic workers:** Use `subagent-driven-development`; test first, review specification, then review code quality.

**Goal:** Deliver an internally buildable server entrypoint, secure configuration loading, migration command and deployment instructions without requiring a real App ID during development.

**Architecture:** The portable `services/ask-auth` crate remains in the public CLI workspace. Add an independent, unpublished Cargo workspace at `services/ask-auth-tce` for company SDK integration and a production `ask-auth` binary. Its manifest depends on the core by path and keeps its own lock/config; root CLI builds must not resolve internal SDK packages. Reuse the core runtime, cryptography and migration implementation. Code is locally tested with synthetic sources; BOE connectivity and real OAuth are deployment acceptance, not claimed by local tests.

**Tech stack:** Rust, Tokio, Serde, pinned `byted-bconfig = 0.3.11`, `byted-dps` and `byted-env` versions verified in the SDK lock, plus existing ask-auth core. Preserve `polyval_force_soft` in this independent workspace's build configuration.

## Source contract

Inspected `rust-lang/servery` commit `153533abb652843f2600fa4ded89f7b626369d05`:

- `byted-tcc 0.8.8` supports V3 namespaces through V2. Its normal `get`/`ensure` and watch interfaces do not expose positive remote freshness.
- The same SDK's public lower-level `BConfigClient` explicitly provides uncached requests. `BConfigClient::builder().with_auth(AuthMode::Gdpr).with_timeout(Some(duration)).build()` performs platform discovery/authentication. Do not implement discovery, HTTP authentication or token loading yourself.
- V2 metadata key is `/tcc/v2/meta/{namespace}` with `version_codes: [{ confspace, version_code }]`. Data key is `/tcc/v2/data/{namespace}/{space}` with `version_code`, string-valued `data`, optional `gray_data`.
- Require a nonempty current DPS credential before each read; never disable GDPR auth. Read metadata then data once each under an overall three-second deadline, validate the returned resource keys and matching nonempty version. Reject missing/duplicate target space, version mismatch and nonempty gray overrides rather than accidentally applying the wrong rollout. First release uses explicit user/tenant policy for rollout.
- Raw SDK response logging is blocked by the host's allowlisted core tracing subscriber, installed before SDK use. Install a fixed-message process panic hook before loading configuration. Never print underlying SDK error text, response payloads, identities or secrets.

## Task 1: Independent SDK package and fresh reader

Files: `services/ask-auth-tce/{Cargo.toml,Cargo.lock,.cargo/config.toml,src/lib.rs,src/tcc.rs,src/config.rs,tests/...}`.

- [x] Test first using private fake raw SDK response injection: successful fresh snapshot, target-space/version mismatch, missing/duplicate space, resource-key mismatch, gray data rejection, read failure/timeout and secret-safe errors.
- [x] Implement the SDK reader with an internal raw-read seam for tests, no public endpoint overrides and no cached fallback. Use a single SDK client, not repeated TccClient construction.
- [x] Validate bootstrap namespace/space syntax and explicit expected environment (`boe` or `prod`), service identity availability and SDK environment agreement. Only nonsecret locator/listener settings are accepted as arguments; no application/database secrets in argv or environment.
- [x] Bound fetched snapshot and selected payload sizes, parser depth, lists and counts. Sensitive wrapper types have no Debug and erase owned strings. Treat encrypted values that cannot be read/parsed as errors; never export them through HTTP.
- [x] Keep root CLI workspace metadata free of internal packages. Build and test the isolated package through its own working directory so its registry and crypto build flags apply.
- [x] Independently review and commit this task before proceeding to startup assembly.

## Configuration layout for assembly

Use dedicated namespace `storage.tos.cli`, default space `default`. Values are versioned JSON strings in TCC:

| Key | Protection | Contents |
| --- | --- | --- |
| `ask_auth_runtime` | Ordinary configuration | schema_version=1; is_enabled; app_id; environment; deployment; callback_url; required_scopes; current_key_version; runtime database CA path and API/cleanup pool bounds. |
| `ask_auth_secrets` | Encrypted, approved service PSM only, no CDN | schema_version=1; client_secret; runtime database URL; versioned independent HMAC/AES keys encoded as canonical Base64URL 32-byte values. No migration credentials. |
| `ask_auth_admission` | Ordinary configuration | schema_version=1; is_enabled; allowed_users/allowed_tenants/denied_users/denied_tenants. The reader's remote version and actual read-completion timestamp supply policy version/fetched_at. |
| `ask_auth_cleanup` | Ordinary configuration | schema_version=1; is_enabled; validated interval/batch/pass/transaction/rate limits. |

Disabled runtime configuration can be minimal (`schema_version=1`, `is_enabled=false`) and must start only the disabled health/readiness/business router without requiring OAuth credentials or a database. Active mode strictly requires the full runtime and secret payloads. Unknown/duplicate fields, malformed keys, duplicate key versions, missing current version, unsafe callback, unexpected environment and out-of-range values fail startup with sanitized errors.

Do not put migration credentials in this runtime-readable namespace. A separate migration command accepts a protected operator-provided JSON file with URL and CA path, validates its Unix owner/mode/type/no-symlink properties and bounded contents, then invokes existing MigrationDatabase. It never starts HTTP or automatically repairs schema. No secret appears in command arguments, stdout or errors. The approved provisioning mechanism for that operator file is a deployment input.

## Task 2: Assembly, control publisher, migration and packaging

Files: focused `src/bootstrap.rs`, `src/control.rs`, `src/migration.rs`, `src/main.rs`; `build.sh`, `run.sh`, README and nonsecret JSON examples.

- [x] Parse all four payloads with synthetic fixtures first; separately validate disabled/active mode. Match the exact core API and OAuth provider types produced by the preceding task, sharing one Crypto Arc between both stores and services.
- [x] Active startup validates fixed config/provider/crypto before connecting, opens separate API and cleanup writer pools with mandatory verified TLS, and closes every acquired resource if later startup fails. Never apply migrations automatically during service startup.
- [x] Publish admission and cleanup controls from fresh remote snapshots every 15 seconds with delayed ticks. Failure immediately clears admission and publishes cleanup None; never label a cached read fresh. A later valid read recovers. Config source closure/panic must be observed and stop the server. Immutable runtime/secret changes require restart: revoke controls, drain and exit safely rather than silently retaining mismatched keys or credentials.
- [x] Use whole-millisecond read-completion timestamps and UTC clock validation. Required scopes, identity semantics and external-account restrictions remain those of the strict OAuth client; no relaxed fallback for missing fields.
- [x] Supervise the control publisher and AuthorizationRuntime as owned futures. Signal handling is registered once; normal termination drains and closes resources. Source/publisher/runtime errors and panics produce fixed messages. No detached application tasks survive cancellation.
- [x] Implement `probe` as read-only TCC configuration validation with fixed success/failure output only, no payload/identity/secret values. A disabled minimal configuration permits the initial connectivity check without App ID.
- [x] Implement separate `migrate` command with protected-file configuration, existing checksum/advisory-lock migration, fixed errors and explicit pool close on every result. Test permissions, symlinks, FIFO/nonregular paths, oversized file, malformed input and absence of credential output. Execute destructive migration tests only on explicitly guarded isolated schemas.
- [x] Add scripts that build the actual internal entrypoint with a locked dependency graph and run it with `exec`. Never package private SDK dependencies or configuration into public CLI artifacts. TCE Linux build/target/image, PSM identity/ACL, MySQL CA, HTTPS ingress/TLB, egress, limits/alerts and actual two-instance behavior must be verified at deployment.
- [x] Run package/contract/root-workspace tests appropriate to changes; independent specification and seven-category quality review. Fix all Critical/Major issues and commit locally without pushing.

## Local versus deployment acceptance

No real App ID or database/TCE resource is required to compile and locally test the above. Static tests do not verify a test application's installation, actual identity fields, account coverage, 64-character state roundtrip, actual RT rotation or platform tool restrictions. Activation in BOE uses an isolated approved test application after deployment; production and external rollout require the design's measured release gates.

## Task 1 verification record

- SDK reader and shared depth checker passed independent specification and seven-category quality review, with 0 remaining Critical / Major / Minor findings. The first spec pass found the IgnoredAny depth bypass; a separate deadline review found late synchronous successes. Both were reproduced before fixes #67/#68.
- Root independently ran all 29 child tests, strict all-target Clippy, build and format checks successfully. Evidence: `/private/tmp/ask-auth-tcc-root-verification.json`. Six shared depth tests passed independently; root metadata remains 8 members with no private SDK dependency leakage.
- No real application ID, TCC service credential or BOE API call was needed for development. Runtime ACL/encrypted-read/network verification remains pending deployment. Task 2 assembly is recorded below.


## Task 2 verification record

- Implemented `serve`, read-only `probe`, protected-file `migrate`, typed configuration, fresh control publishing, resource ownership, Linux build/run scripts and deployment instructions. A real application ID remains unnecessary for local development.
- Independent specification review passed. Full quality round 1 found one Major: equal cleanup publications reset the worker timer. Both the notification test and actual-worker scheduling regression reproduced the failure before fix #88. Full quality round 2 passed all seven categories with 0 remaining Critical, Major, Minor or Suggestion findings.
- Root independently verified 124 ordinary core Rust tests, 60 ordinary internal-host Rust tests, 94 guarded core MySQL acceptance tests, 3 production-TLS host tests and 7 Python HTTP/schema contracts. The TLS scheduling regression uses a deliberately closed real store to observe a pass attempt without business writes; it does not claim successful deletion.
- Strict all-target Clippy, local builds, formatting, shell syntax and root workspace check passed. Root workspace remains 8 public members without private SDK dependency leakage. Evidence is recorded under `/private/tmp/ask-auth-stage5-reviewed-child.json`, `/private/tmp/ask-auth-stage5-final-core-allowed.json` and `/private/tmp/ask-auth-stage5-corrected-mysql-verification.json`; the three host TLS passes are in `/private/tmp/ask-auth-stage5-host-tls-green.log`.
- Deployment acceptance remains pending: Linux SCM release assembly, workload PSM/DPS/TCC ACL and encrypted reads, BOE network/TLB/CA/grants, distributed edge limits and alert delivery, real test-application OAuth/M0, two replicas and measured load/capacity. These are required before activating real customer ask; local tests do not establish them.
