# Retry Idempotency and Commit Split Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prevent automatic replay of non-idempotent HTTP operations, restore formatting compliance, and split the retry and Consul discovery work into focused commits.

**Architecture:** All retry branches share one request-level eligibility decision. Safe HTTP methods are eligible by default; POST and PATCH require an explicit internal idempotent override, used only for known read-only operations such as ADrive search. OAuth 401 recovery remains separate from transient retries.

**Tech Stack:** Rust, Tokio, reqwest, Cargo, Git.

---

### Task 1: Lock down non-idempotent behavior

**Files:**
- Modify: `crates/tos-core/src/infra/client.rs`
- Modify: `crates/adrive/src/domain/client.rs`

- [ ] **Step 1: Write failing TOS tests**

Add behavior tests that send a POST to a one-response server returning HTTP 500 and expect the original response to be returned without a second attempt. Cover byte-body and replayable streaming entry points.

- [ ] **Step 2: Verify the TOS tests fail for the retry reason**

Run:

```bash
cargo test -p tos-core non_idempotent_post_does_not_retry_500 -- --nocapture
```

Expected: FAIL because the current client attempts a second request.

- [ ] **Step 3: Write failing ADrive tests**

Add a POST/500 test around the normal request path and keep the existing response-body test. Assert the POST returns the first 500 response rather than retrying.

- [ ] **Step 4: Verify the ADrive test fails for the retry reason**

Run:

```bash
cargo test -p ve-adrive-cli-core non_idempotent_post_does_not_retry_500 -- --nocapture
```

Expected: FAIL because the current client attempts a second request.

### Task 2: Unify retry eligibility

**Files:**
- Modify: `crates/tos-core/src/infra/client.rs`
- Modify: `crates/adrive/src/domain/client.rs`

- [ ] **Step 1: Gate every TOS retry branch**

Compute one `can_retry_attempt` value from the request method and require it for transient status, send error, body-consumption error, and replayable streaming retries:

```rust
fn is_request_retry_safe(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::PUT | Method::DELETE | Method::OPTIONS
    )
}
```

- [ ] **Step 2: Add an explicit ADrive retry safety policy**

Use an internal policy so POST is conservative by default while a known idempotent POST can opt in:

```rust
#[derive(Clone, Copy)]
enum RetrySafety {
    MethodDefault,
    Idempotent,
}

impl RetrySafety {
    fn allows_retry(self, method: &Method) -> bool {
        matches!(self, Self::Idempotent) || is_request_retry_safe(method)
    }
}
```

Pass the resulting decision through both the response-consuming and response-returning retry loops. Do not apply it to the separate one-time OAuth 401 refresh path.

- [ ] **Step 3: Preserve retry for ADrive read-only search**

Route `search_files`, whose POST operation is read-only, through the explicit `RetrySafety::Idempotent` path. Leave create, rename, copy, and multipart completion on the conservative default.

- [ ] **Step 4: Run the focused tests**

```bash
cargo test -p tos-core non_idempotent_post_does_not_retry_500 -- --nocapture
cargo test -p ve-adrive-cli-core non_idempotent_post_does_not_retry_500 -- --nocapture
cargo test -p ve-adrive-cli-core structured_request_retries_when_success_body_is_truncated -- --nocapture
```

Expected: all PASS.

### Task 3: Format and review

**Files:**
- Modify mechanically: Rust sources touched by `cargo fmt`

- [ ] **Step 1: Apply formatting**

```bash
cargo fmt --all
```

- [ ] **Step 2: Run focused package tests and workspace checks**

```bash
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
```

Expected: all commands exit successfully.

- [ ] **Step 3: Review the complete diff**

Check correctness, security, performance, maintainability, robustness, testability, and observability. Fix all Critical and Major findings, then repeat the checks.

### Task 4: Split and publish commits

**Files:**
- Retry commit: all retry and timeout files except Consul-only hunks
- Discovery commit: Consul address normalization and its tests in `crates/tos-core/src/infra/discovery.rs`

- [ ] **Step 1: Rewrite the latest implementation commit**

Reset the latest implementation commit while preserving its tree, then stage retry-related changes without Consul-only hunks and commit them as:

```text
feat: complete HTTP retry with idempotency safeguards
```

- [ ] **Step 2: Commit Consul discovery separately**

Stage the remaining Consul-only changes and commit them as:

```text
fix: normalize Consul discovery addresses
```

- [ ] **Step 3: Verify commit boundaries and repository state**

```bash
git show --stat --oneline HEAD~1
git show --stat --oneline HEAD
git status --short --branch
```

Expected: two focused commits and a clean worktree.

- [ ] **Step 4: Update the remote safely**

```bash
git push --force-with-lease origin xsj-dev
```

Expected: remote `xsj-dev` points to the rewritten two-commit history.
