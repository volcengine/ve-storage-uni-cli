# Service Request ID Propagation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every `tos`, `ve-tos`, and `ve-adrive` command prefer sanitized service request IDs in its top-level Envelope while retaining a bounded trace for multi-request commands and a CLI ULID fallback for local work.

**Architecture:** Add one invocation-scoped `ServiceRequestTrace` to `GlobalArgs` and share it with each runtime HTTP client. Transport boundaries record every observed response ID, while the common output projection replaces generated Envelope IDs with the last successful service ID and adds bounded multi-request diagnostics. Existing explicit `null`, error parsing, command behavior, retry policy, and nested `data.request_id` fields remain compatible.

**Tech Stack:** Rust, Tokio, Reqwest, Clap, Serde JSON, existing `Envelope` and mock TCP-server test utilities.

---

### Task 1: Add the shared bounded request trace

**Files:**
- Create: `crates/tos-core/src/agent/request_id.rs`
- Modify: `crates/tos-core/src/agent/mod.rs`
- Modify: `crates/tos-core/src/agent/global_args.rs`
- Test: `crates/tos-core/src/agent/request_id.rs`

- [x] **Step 1: Write failing sanitizer and bounded-trace tests**

Add tests that require the following API and behavior:

```rust
#[test]
fn sanitizes_request_ids() {
    assert_eq!(sanitize_request_id(" req-1 "), Some("req-1".to_string()));
    assert_eq!(sanitize_request_id(""), None);
    assert_eq!(sanitize_request_id("req\n1"), None);
    assert_eq!(sanitize_request_id(&"x".repeat(257)), None);
}

#[test]
fn bounds_ids_and_tracks_last_success() {
    let trace = ServiceRequestTrace::with_limit(2);
    trace.record_response(Some("retry-1"), false);
    trace.record_response(Some("ok-2"), true);
    trace.record_response(Some("ok-3"), true);
    assert_eq!(
        trace.snapshot(),
        ServiceRequestSnapshot {
            request_ids: vec!["retry-1".into(), "ok-2".into()],
            request_ids_omitted: 1,
            last_successful_request_id: Some("ok-3".into()),
        }
    );
}
```

- [x] **Step 2: Run the focused test and verify RED**

Run:

```bash
cargo test -p tos-core agent::request_id -- --nocapture
```

Expected: compilation fails because `agent::request_id` and the trace types do not exist.

- [x] **Step 3: Implement the trace primitive**

Create a mutex-backed, cloneable shared type with these public contracts:

```rust
pub const SERVICE_REQUEST_ID_LIMIT: usize = 1024;
pub const SERVICE_REQUEST_ID_MAX_CHARS: usize = 256;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ServiceRequestSnapshot {
    pub request_ids: Vec<String>,
    pub request_ids_omitted: usize,
    pub last_successful_request_id: Option<String>,
    pub response_count: usize,
    pub request_attempted: bool,
    pub terminal_response_received: bool,
    pub terminal_response_request_id: Option<String>,
}

#[derive(Debug)]
pub struct ServiceRequestTrace {
    limit: usize,
    state: Mutex<ServiceRequestState>,
}

impl ServiceRequestTrace {
    pub fn record_response(&self, raw_request_id: Option<&str>, is_success: bool);
    pub fn record_no_response(&self);
    pub fn snapshot(&self) -> ServiceRequestSnapshot;
}

pub fn sanitize_request_id(raw_request_id: &str) -> Option<String>;
pub fn select_error_request_id(
    snapshot: &ServiceRequestSnapshot,
    legacy_request_id: Option<&str>,
) -> Option<String>;
```

`sanitize_request_id` trims whitespace, rejects empty values, values over 256
Unicode scalar values, and values containing control characters. Poisoned
mutexes recover with `into_inner()` instead of panicking.

- [x] **Step 4: Add one trace per `GlobalArgs` invocation**

Add the skipped Clap field and initialize it in `Default`:

```rust
#[arg(skip)]
pub request_trace: Arc<ServiceRequestTrace>,
```

Cloning `GlobalArgs` must clone the `Arc`, while separately parsed invocations
must receive separate traces.

- [x] **Step 5: Run focused tests and verify GREEN**

Run:

```bash
cargo test -p tos-core agent::request_id -- --nocapture
cargo test -p tos-core agent::global_args -- --nocapture
```

Expected: all selected tests pass.

- [x] **Step 6: Commit the primitive**

```bash
git add crates/tos-core/src/agent/request_id.rs crates/tos-core/src/agent/mod.rs crates/tos-core/src/agent/global_args.rs
git commit -m "feat: add invocation request id trace"
```

### Task 2: Project service IDs into successful Envelopes

**Files:**
- Modify: `crates/tos-core/src/agent/request_id.rs`
- Modify: `crates/tos/src/handler/common.rs`
- Modify: `crates/adrive/src/handler/common.rs`
- Test: `crates/tos-core/src/agent/request_id.rs`
- Test: `crates/tos/src/handler/common.rs`
- Test: `crates/adrive/src/handler/common.rs`

- [x] **Step 1: Write failing Envelope projection tests**

Cover these cases with serialized `Envelope<Value>` values:

```rust
#[test]
fn service_trace_overrides_generated_success_id() {
    let trace = ServiceRequestTrace::default();
    trace.record_response(Some("service-1"), true);
    let mut value = serde_json::to_value(Envelope::success("tos ls", json!({}))).unwrap();
    apply_service_trace_to_success_envelope(&mut value, &trace.snapshot());
    assert_eq!(value["request_id"], "service-1");
}

#[test]
fn multi_request_trace_is_added_to_data() {
    let trace = ServiceRequestTrace::with_limit(2);
    trace.record_response(Some("retry"), false);
    trace.record_response(Some("success"), true);
    let mut value = serde_json::to_value(Envelope::success("tos ls", json!({}))).unwrap();
    apply_service_trace_to_success_envelope(&mut value, &trace.snapshot());
    assert_eq!(value["data"]["service_request_ids"], json!(["retry", "success"]));
    assert_eq!(value["data"]["service_request_ids_omitted"], 0);
}
```

Also assert that an explicit top-level `null` remains `null`, a one-ID trace
does not create `data.service_request_ids`, and an empty trace preserves the
generated ULID.

- [x] **Step 2: Run focused projection tests and verify RED**

Run:

```bash
cargo test -p tos-core agent::request_id::tests -- --nocapture
```

Expected: compilation fails because `apply_service_trace_to_success_envelope` is missing.

- [x] **Step 3: Implement the shared projection helper**

Add:

```rust
pub fn apply_service_trace_to_success_envelope(
    envelope: &mut serde_json::Value,
    snapshot: &ServiceRequestSnapshot,
);
```

The helper acts only on object-shaped successful Envelopes, preserves explicit
`null`, replaces a generated or explicit non-null top-level ID when
`last_successful_request_id` exists, and adds the two `data` trace fields only
when the observed response count exceeds one.

- [x] **Step 4: Invoke the helper in both output pipelines**

In both `ensure_envelope` implementations, serialize or wrap first, apply the
invocation trace second, then run the existing fallback injection and command
normalization. Do not remove `TOS_LAST_REQUEST_ID`; it remains the legacy
fallback when the invocation trace is empty.

- [x] **Step 5: Run output tests and verify GREEN**

Run:

```bash
cargo test -p tos-core agent::request_id -- --nocapture
cargo test -p ve-tos-cli-core handler::common -- --nocapture
cargo test -p ve-adrive-cli-core handler::common -- --nocapture
```

Expected: all selected tests pass, including explicit-null and legacy-env tests.

- [x] **Step 6: Commit output projection**

```bash
git add crates/tos-core/src/agent/request_id.rs crates/tos/src/handler/common.rs crates/adrive/src/handler/common.rs
git commit -m "feat: prefer service request ids in output"
```

### Task 3: Record every TOS service response

**Files:**
- Modify: `crates/tos-core/src/infra/client.rs`
- Modify: `crates/tos/src/handler/advanced.rs`
- Modify: `crates/tos/src/handler/bucket.rs`
- Modify: `crates/tos/src/handler/bucket_config.rs`
- Modify: `crates/tos/src/handler/high_level.rs`
- Modify: `crates/tos/src/handler/meta.rs`
- Modify: `crates/tos/src/handler/multipart.rs`
- Modify: `crates/tos/src/handler/object.rs`
- Modify: `crates/tos/src/handler/turbo.rs`
- Test: `crates/tos-core/src/infra/client.rs`

- [x] **Step 1: Write a failing retry trace test**

Extend the existing mock TCP server test so the first response is retryable
with `x-tos-request-id: retry-1`, the second is successful with
`x-tos-request-id: success-2`, and the client shares a test trace. Assert:

```rust
assert_eq!(snapshot.request_ids, vec!["retry-1", "success-2"]);
assert_eq!(snapshot.last_successful_request_id.as_deref(), Some("success-2"));
```

- [x] **Step 2: Run the TOS client test and verify RED**

Run the exact new test by name with:

```bash
cargo test -p tos-core tos_client_records_retry_and_success_request_ids -- --nocapture
```

Expected: compilation fails because no traced constructor exists.

- [x] **Step 3: Add a traced TOS client constructor**

Keep `TosClient::new` source-compatible and add:

```rust
pub fn new_with_request_trace(
    profile: &Profile,
    service: &str,
    request_trace: Arc<ServiceRequestTrace>,
) -> Result<Self, CliError>;
```

Store the trace in `TosClient`. Both `send_request_once` and
`send_signed_request_once` must record every received response exactly once,
before retry classification consumes it. Use `response.status().is_success()`
to update the last successful ID.

- [x] **Step 4: Share the invocation trace at every TOS runtime constructor**

Replace handler-side runtime construction with:

```rust
TosClient::new_with_request_trace(&profile, "tos", Arc::clone(&global.request_trace))
```

Do this for ordinary clients and region-override clients. Test-only direct
constructors may keep `TosClient::new` and receive an isolated trace.

- [x] **Step 5: Sanitize the legacy environment fallback**

When `check_response` mirrors `x-tos-request-id` into
`TOS_LAST_REQUEST_ID`, use `sanitize_request_id` and do not write rejected
values.

- [x] **Step 6: Run TOS tests and verify GREEN**

Run:

```bash
cargo test -p tos-core tos_client_records_retry_and_success_request_ids -- --nocapture
cargo test -p ve-tos-cli-core --lib -- --nocapture
```

Expected: all tests pass and retry policy assertions remain unchanged.

- [x] **Step 7: Commit TOS transport wiring**

```bash
git add crates/tos-core/src/infra/client.rs crates/tos/src/handler
git commit -m "feat: trace TOS service request ids"
```

### Task 4: Record ADrive Resource and interactive OAuth responses

**Files:**
- Modify: `crates/adrive/src/domain/client.rs`
- Modify: `crates/adrive/src/domain/oauth.rs`
- Modify: `crates/adrive/src/handler/common.rs`
- Modify: `crates/adrive/src/handler/auth.rs`
- Test: `crates/adrive/src/domain/client.rs`
- Test: `crates/adrive/src/domain/oauth.rs`

- [x] **Step 1: Write failing header-priority and retry trace tests**

For the Resource client, return both headers on the successful response and
assert `x-ids-request-id` wins over `x-request-id`. Return a retryable response
before it and assert both IDs are retained while the successful IDS value is
last.

For the interactive OAuth client, return `x-request-id` from device
authorization and token polling responses and assert the shared trace records
them. Do not share this trace with `OAuthTokenManager` refresh clients.

- [x] **Step 2: Run focused ADrive tests and verify RED**

Run:

```bash
cargo test -p ve-adrive-cli-core request_trace -- --nocapture
```

Expected: compilation fails because traced constructors are absent.

- [x] **Step 3: Add traced Resource constructors and response recording**

Keep existing constructors for compatibility and add crate-visible traced
constructors for AK/SK and OAuth. Store the shared trace on `ClientInner` and
record the prioritized header in `send_once` immediately after `send()`
returns, before retry or 401-refresh handling.

Change successful `ResponseInfo` construction to reuse the same sanitizer.
Rejected IDs remain absent and never replace the Envelope fallback.

- [x] **Step 4: Wire `build_ids_client` to the invocation trace**

Pass `Arc::clone(&global.request_trace)` for both AK/SK and OAuth Resource
clients. OAuth Token refresh clients continue using an isolated trace so an
authorization-server response cannot overwrite a successful resource API ID.

- [x] **Step 5: Trace only interactive Auth commands**

Add an `OAuthClient::new_with_request_trace` constructor. Use it in
`handle_login` and other direct `ve-adrive auth` request paths that receive
`GlobalArgs`; keep token-manager construction on the original isolated
constructor.

- [x] **Step 6: Run ADrive tests and verify GREEN**

Run:

```bash
cargo test -p ve-adrive-cli-core request_trace -- --nocapture
cargo test -p ve-adrive-cli-core --lib -- --nocapture
```

Expected: all selected tests pass, OAuth refresh tests still show one Resource
retry, and credential values remain redacted.

- [x] **Step 7: Commit ADrive transport wiring**

```bash
git add crates/adrive/src/domain/client.rs crates/adrive/src/domain/oauth.rs crates/adrive/src/handler/common.rs crates/adrive/src/handler/auth.rs
git commit -m "feat: trace ADrive service request ids"
```

### Task 5: Add command-level regressions and complete the global audit

**Files:**
- Modify: `crates/tos/src/handler/high_level.rs`
- Modify: `crates/adrive/src/handler/high_level.rs`
- Modify: `tests/cli_basic.rs`
- Modify: `crates/tos-core/tests/test_agent_output.rs`

- [x] **Step 1: Write command-level failing regressions**

Add mock-response tests for:

- `tos ls tos://bucket/` returning the service header at top level;
- `ve-adrive ls` Instances, Spaces, and Files scopes returning the service
  header at top level while retaining existing nested `data.request_id`;
- a two-page listing returning the second page ID at the top level and both IDs
  in `data.service_request_ids`;
- a pure local or describe command retaining a 26-character ULID;
- an aggregate explicit-null response remaining null.

- [x] **Step 2: Run the regressions and verify RED where audit gaps remain**

Run each new test by exact name. Any test that passes before a production
change must be retained as coverage but does not justify modifying that path.

- [x] **Step 3: Audit every runtime client constructor and Envelope output**

Use:

```bash
rg -n "TosClient::new|IdsClient::new|OAuthClient::new" crates/tos/src crates/adrive/src
rg -n "Envelope::success|high_level_success_envelope|Envelope::failed" crates/tos/src crates/adrive/src src
rg -n '"request_id"' crates/tos/src crates/adrive/src
```

Classify each result as local, single-response, or multi-response. Wire missed
runtime clients to `global.request_trace`. Do not change pure local commands or
remove nested compatibility fields.

- [x] **Step 4: Implement only the remaining propagation fixes**

Where an HTTP helper still discards response metadata before it reaches the
shared trace, retain and record that metadata. Where a command intentionally
uses explicit `null`, preserve it. Do not change request payloads, endpoint
selection, authentication, or retry decisions.

- [x] **Step 5: Run focused command regressions and verify GREEN**

Run the exact new test names, then:

```bash
cargo test -p ve-tos-cli-core --lib -- --nocapture
cargo test -p ve-adrive-cli-core --lib -- --nocapture
cargo test --test cli_basic -- --nocapture
```

Expected: all tests pass.

- [x] **Step 6: Commit audit fixes**

```bash
git add crates/tos/src/handler/high_level.rs crates/adrive/src/handler/high_level.rs tests/cli_basic.rs crates/tos-core/tests/test_agent_output.rs
git commit -m "fix: propagate service request ids globally"
```

### Task 6: Review and full verification

**Files:**
- Review: all files changed since design commit `182a5f9`

- [x] **Step 1: Format and inspect the complete diff**

Run:

```bash
cargo fmt --all -- --check
git diff --check 182a5f9..HEAD
git diff --stat 182a5f9..HEAD
```

Expected: formatting and whitespace checks succeed; the diff contains only
request-ID propagation code, tests, and its implementation plan.

- [x] **Step 2: Perform the mandatory Reviewer pass**

Review correctness, control-character rejection, 1024-entry memory bound,
mutex poisoning, concurrent completion order, OAuth/resource isolation,
fallback compatibility, error request IDs, and secret redaction. Fix every
Critical or Major issue and add a regression test before each production fix.

- [x] **Step 3: Run complete verification**

Run:

```bash
cargo fmt --all -- --check
cargo test --workspace
```

Expected: both commands exit zero. Existing explicitly ignored live-service
tests may remain ignored; no new ignored tests are introduced.

- [x] **Step 4: Verify repository state and commit any review fixes**

```bash
git status --short
git log --oneline --decorate -8
```

If Reviewer fixes changed files, stage the files listed by `git status --short`
and commit them as:

```bash
git add crates/tos-core crates/tos crates/adrive src tests docs/superpowers/plans/2026-08-11-service-request-id-propagation.md
git commit -m "fix: harden service request id propagation"
```

Expected: no uncommitted changes remain.
