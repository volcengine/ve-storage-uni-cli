# Complete HTTP Attempt Retry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make replayable HighLevel and LowLevel HTTP operations retry only after their complete response body and semantic validation have succeeded.

**Architecture:** Keep signing and command handlers intact. Add response-consumer-aware retry entry points to each HTTP client, use read-idle rather than total request timeouts, and migrate buffered and file-backed consumers to those entry points. Preserve direct one-shot streaming for stdin and stdout.

**Tech Stack:** Rust, Tokio, reqwest 0.12, existing TOS/ADrive clients and integration-test servers.

---

### Task 1: Lock the shared timeout and retry classification

**Files:**
- Modify: `crates/tos-core/src/infra/config.rs`
- Modify: `crates/tos-core/src/infra/client.rs`
- Test: `crates/tos-core/tests/test_config.rs`
- Test: `crates/tos-core/src/infra/client.rs`

- [ ] Assert the default request timeout is 300 seconds and an explicit value still wins.

```rust
assert_eq!(DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS, 300);
assert_eq!(effective.requesttimeout.value, Some(300));
assert_eq!(explicit.requesttimeout.value, Some(60));
```

- [ ] Add a client test whose response body stalls longer than the configured timeout and assert the failure is classified as a retryable body/read-timeout failure.
- [ ] Replace the total client timeout with reqwest `read_timeout`, retaining the existing connect timeout.

```rust
Client::builder()
    .connect_timeout(Duration::from_secs(connect_timeout))
    .read_timeout(Duration::from_secs(request_timeout))
```

- [ ] Extend the transient reqwest classifier to cover body/decode stream failures while leaving local I/O and deterministic validation errors non-retryable.

```rust
fn should_retry_reqwest_error(error: &reqwest::Error) -> bool {
    error.is_timeout()
        || error.is_connect()
        || error.is_body()
        || error.is_decode()
}
```

- [ ] Run `cargo test -p tos-core` and confirm zero failures.

### Task 2: Add complete-response retry to the TOS client

**Files:**
- Modify: `crates/tos-core/src/infra/client.rs`
- Modify: `crates/tos/src/domain/core.rs`
- Test: `crates/tos-core/src/infra/client.rs`

- [ ] Add a failing mock-server test that sends a successful header and truncated first body, then a complete second body; assert two attempts and one successful parsed result.

```rust
assert_eq!(attempts.load(Ordering::SeqCst), 2);
assert_eq!(consumed_body, b"complete-response");
```

- [ ] Add a generic response-consumer retry entry point whose consumer owns `Response` and returns only after body consumption.

```rust
pub async fn send_request_with_consumer<T, C, CFut>(
    &self,
    request: ReplayableRequest,
    mut consume: C,
) -> Result<T, CliError>
where
    C: FnMut(Response) -> CFut,
    CFut: Future<Output = Result<T, CliError>>,
```

- [ ] Rebuild replayable request bodies for each attempt and keep the existing `max_retry_count` meaning of initial attempt plus N retries.
- [ ] Move buffered success-body parsing inside this retry boundary; retry 408, 429, 5xx, and transient body errors only.

```rust
client
    .send_request_with_consumer(request, |response| async move {
        streaming_response_envelope(client, command, response).await
    })
    .await
```

- [ ] Run the focused client and domain tests.

### Task 3: Migrate TOS HighLevel and LowLevel body consumers

**Files:**
- Modify: `crates/tos/src/domain/object.rs`
- Modify: `crates/tos/src/domain/multipart.rs`
- Modify: `crates/tos/src/handler/high_level.rs`
- Modify: `crates/tos/src/handler/object.rs`
- Test: `crates/tos/src/handler/high_level.rs`

- [ ] Add failing tests for a normal file download and a Range download whose first body terminates early.

```rust
assert_eq!(server_attempts.load(Ordering::SeqCst), 2);
assert_eq!(std::fs::read(destination).unwrap(), expected_bytes);
```

- [ ] Retry a standard file GET by recreating/truncating the temporary file for each attempt.

```rust
|mut response| async move {
    write_response_stream_with_mode(&mut response, &temp_path, false).await
}
```

- [ ] Retry only the failed Range into its own part file and keep completed parts.
- [ ] Route buffered list/config bodies through the complete-response executor.
- [ ] Keep stdout streaming on the existing one-shot path so emitted bytes cannot be duplicated.

```rust
if output == "-" {
    let mut response = core::send_object_request(
        client,
        Method::GET,
        &bucket,
        &key,
        query,
        headers,
        None,
    )
    .await?;
    return stream_response_to_stdout(&mut response).await;
}
```

- [ ] Run `cargo test -p ve-tos-cli-core` and the transfer integration tests.

### Task 4: Complete ADrive buffered-response retries

**Files:**
- Modify: `crates/adrive/src/domain/client.rs`
- Test: `crates/adrive/src/domain/client.rs`

- [ ] Add a failing server test that truncates the first JSON response body and succeeds on the second attempt.

```rust
assert_eq!(attempts.load(Ordering::SeqCst), 2);
assert_eq!(output.response_info().status_code, 200);
```

- [ ] Change the ADrive reqwest client to use read-idle timeout semantics.
- [ ] Move JSON/no-content response consumption inside the transient retry loop.

```rust
self.send_with_transient_retries(request, |applied| async move {
    self.parse_json_response(applied).await
})
.await
```

- [ ] Preserve OAuth 401 refresh as a separate one-time replay and prevent nested retry multiplication.
- [ ] Run the ADrive domain-client tests.

### Task 5: Complete ADrive file-response retries

**Files:**
- Modify: `crates/adrive/src/domain/client.rs`
- Modify: `crates/adrive/src/handler/high_level.rs`
- Test: `crates/adrive/src/handler/high_level.rs`

- [ ] Add failing tests for simple and Range file responses that fail during body streaming.

```rust
assert_eq!(attempts.load(Ordering::SeqCst), 2);
assert_eq!(tokio::fs::read(&part_path).await.unwrap().len() as u64, range_size);
```

- [ ] Retry simple downloads from byte zero into a resettable destination or temporary file.
- [ ] Retry only failed Range part files and retain completed parts.

```rust
retry_get_file(client, input, || async {
    write_download_stream(output, &part_path, false, rate_limiter.clone()).await
})
.await
```

- [ ] Keep stdin uploads and stdout downloads one-shot after streaming starts.
- [ ] Run the focused ADrive transfer tests.

### Task 6: Review and full verification

**Files:**
- Modify only files required by review findings.

- [ ] Review correctness, security, performance, maintainability, robustness, testability, and observability with emphasis on duplicate writes, nested retries, partial files, and secret logging.
- [ ] Fix every Critical and Major finding and repeat the complete review.
- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo test --workspace`.
- [ ] Run `cargo check --workspace`.
- [ ] Run `git diff --check` and inspect the final diff for unrelated changes.
