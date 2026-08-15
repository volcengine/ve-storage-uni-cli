# Status-Aware HTTP Retry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the frozen TOS and ADrive retry contract so 429 and 5xx responses retry replayable requests regardless of operation idempotency, while 408 and ambiguous transport/body failures remain idempotency-gated.

**Architecture:** Put status classification, bounded exponential delay, and `Retry-After` parsing in `tos-core::infra::retry`, which both clients can use. Keep each client's request loop responsible for distinguishing response-status failures, pre-delivery connection failures, and ambiguous client/body failures. Preserve the existing request factories and response consumers so streams remain replayable only where they already are.

**Tech Stack:** Rust 2021, Tokio, Reqwest, `httpdate`, Cargo unit and integration tests.

---

### Task 1: Shared storage retry policy

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/tos-core/Cargo.toml`
- Modify: `crates/tos-core/src/infra/retry.rs`

- [x] **Step 1: Write failing policy tests**

Add unit tests proving that `should_retry_storage_status` accepts 408 only for idempotent operations, accepts 429 and every 5xx for all operations, and rejects ordinary 4xx; `storage_retry_after_delay` supports delta-seconds and HTTP dates for 429/5xx, caps delays at 600 seconds, treats past dates as zero, and ignores invalid values and 408; and `storage_backoff_delay` grows from 200 milliseconds to a 6.4-second cap.

- [x] **Step 2: Run the policy tests and verify RED**

Run: `cargo test -p tos-core infra::retry::tests`

Expected: compilation fails because the new policy functions do not exist yet.

- [x] **Step 3: Implement the shared policy**

Add the direct `httpdate = "1"` dependency and implement:

```rust
pub const MAX_RETRY_AFTER_DELAY: Duration = Duration::from_secs(600);

pub fn should_retry_storage_status(status: StatusCode, is_idempotent: bool) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
        || (is_idempotent && status == StatusCode::REQUEST_TIMEOUT)
}

pub fn storage_backoff_delay(attempt: u32) -> Duration {
    Duration::from_millis(200_u64.saturating_mul(1_u64 << attempt.min(5)))
}

pub fn storage_retry_after_delay(
    status: StatusCode,
    headers: &HeaderMap,
    now: SystemTime,
) -> Option<Duration> {
    if status != StatusCode::TOO_MANY_REQUESTS && !status.is_server_error() {
        return None;
    }
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    let delay = value.parse::<u64>().map(Duration::from_secs).or_else(|_| {
        httpdate::parse_http_date(value)
            .map(|retry_at| retry_at.duration_since(now).unwrap_or_default())
    }).ok()?;
    Some(delay.min(MAX_RETRY_AFTER_DELAY))
}
```

- [x] **Step 4: Run the policy tests and verify GREEN**

Run: `cargo test -p tos-core infra::retry::tests`

Expected: all shared policy tests pass.

### Task 2: Apply the policy to the TOS HTTP attempt boundary

**Files:**
- Modify: `crates/tos-core/src/infra/client.rs`

- [x] **Step 1: Write failing TOS behavior tests**

Change the non-idempotent POST status tests to serve a 500 followed by a 200 and assert two identical request bodies. Keep the truncated successful POST response test asserting one request so ambiguous response-body failure is not replayed. Add assertions that ordinary 400 and non-idempotent 408 responses remain single-attempt.

- [x] **Step 2: Run the focused TOS tests and verify RED**

Run: `cargo test -p tos-core infra::client::tests::non_idempotent`

Expected: the 500 tests fail because POST status responses are still gated by method idempotency.

- [x] **Step 3: Separate status retry from ambiguous-failure retry**

In every replayable TOS loop:

```rust
Ok(response)
    if should_retry_storage_status(response.status(), is_idempotent)
        && attempt < self.max_retry_count => {
        sleep_before_response_retry(attempt, response).await;
    }
Err(error)
    if should_retry_send_error(&error, is_idempotent)
        && attempt < self.max_retry_count => {
        sleep_before_retry(attempt).await;
    }
```

Make `should_retry_send_error` always accept definite `reqwest::Error::is_connect()` failures and accept timeout/body/decode failures only for idempotent operations. Continue to gate consumer failures entirely on idempotency. Use the shared `Retry-After` parser for 429/5xx after bounded response draining and use the shared backoff delay for fallback.

- [x] **Step 4: Run focused and full TOS tests and verify GREEN**

Run: `cargo test -p tos-core infra::client::tests`

Expected: all TOS client tests pass.

### Task 3: Apply the policy to the ADrive HTTP attempt boundary

**Files:**
- Modify: `crates/adrive/src/domain/client.rs`

- [x] **Step 1: Write failing ADrive behavior tests**

Change AK/SK and OAuth non-idempotent create tests to serve a 500 followed by a success response and assert two POST requests. Add AK/SK and OAuth 408 tests that assert one POST request. Retain the truncated successful POST response test to prove that ambiguous body failure remains single-attempt.

- [x] **Step 2: Run the focused ADrive tests and verify RED**

Run: `cargo test -p ve-adrive-cli-core retries_500`

Expected: both tests fail because POST status responses are still gated by method idempotency.

- [x] **Step 3: Separate status retry from ambiguous-failure retry**

Remove the idempotency gate from 429/5xx response handling in both ADrive retry loops, but retain it for 408. Add a request-error decision that always retries definite connection failures and only retries ambiguous timeout/body/decode errors when `RetrySafety` permits. Continue to gate consumer and JSON/body errors on `RetrySafety`. Replace duplicate status and timing logic with the shared `tos-core::infra::retry` functions.

- [x] **Step 4: Run focused and full ADrive tests and verify GREEN**

Run: `cargo test -p ve-adrive-cli-core domain::client::tests`

Expected: all ADrive client tests pass.

### Task 4: Review, regression verification, and delivery

**Files:**
- Review every modified file from Tasks 1-3.

- [x] **Step 1: Format and perform static checks**

Run: `cargo fmt --all -- --check`

Run: `cargo check --workspace --all-targets`

Expected: both commands exit successfully with no formatting or compiler errors.

- [x] **Step 2: Run the full regression suite**

Run: `cargo test --workspace`

Expected: all workspace tests pass.

- [x] **Step 3: Perform the mandatory fresh-eyes review**

Review correctness, security, performance, maintainability, robustness, testability, and observability. In particular verify that non-idempotent requests cannot replay on ambiguous local timeouts/body failures, stdin/stdout exceptions are unchanged, OAuth 401 remains separate, response draining is bounded, sensitive headers are not logged, and retry counts remain bounded.

- [x] **Step 4: Commit and push**

Run: `git diff --check`, commit the implementation and plan with a focused message, then push the current `xsj-dev` branch to `origin/xsj-dev`.

Expected: the working tree is clean and local `xsj-dev` matches `origin/xsj-dev`.
