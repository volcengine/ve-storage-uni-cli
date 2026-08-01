# TOS Transfer Reliability and Sync Optimization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix replayable upload retries, async SHA256, unsupported automatic CRC64, object-key encoding, and recursive sync concurrency/HEAD bottlenecks without changing existing CLI contracts.

**Architecture:** Keep existing cp/mv/sync entry points and transfer primitives. Add an RFC3986-encoded HTTP object path, a body-factory retry path for replayable file streams, async chunked SHA256, and in-memory sync inventory comparison. Preserve current report, manifest, checkpoint, conditional request, and delete safety behavior.

**Tech Stack:** Rust 2021, Tokio, Reqwest, FuturesUnordered, Clap, SHA2, Cargo workspace tests.

---

## File Map

- `crates/tos-core/src/infra/client.rs`: object HTTP path encoding and replayable streaming retry primitive.
- `crates/tos/src/domain/core.rs`: structured object request wrapper accepting a body factory.
- `crates/tos/src/handler/high_level.rs`: async SHA256, automatic CRC64 removal, high-level retry call sites, sync inventory comparison, local-to-TOS concurrency, and recursive scan counters.
- `crates/tos/src/registry.rs`: correct high-level upload integrity description.
- `docs/high_level_commands_plan.md`: remove obsolete automatic CRC64 guarantees.
- Workspace and packaging `Cargo.toml` files plus `Cargo.lock`: version 1.0.3.

### Task 1: Encode object keys at the HTTP boundary

**Files:**
- Modify: `crates/tos-core/src/infra/client.rs`
- Test: `crates/tos-core/src/infra/client.rs`

- [x] **Step 1: Add failing endpoint and wire-path tests**

Add tests proving literal key bytes are encoded once:

```rust
fn endpoint_test_client(sign_algorithm: TosSignAlgorithm) -> TosClient {
    TosClient::new_with_sign_algorithm(
        &Profile {
            region: Some("cn-beijing".to_string()),
            access_key_id: Some("ak".to_string()),
            secret_access_key: Some("sk".to_string()),
            endpoint: Some("tos-cn-beijing.volces.com".to_string()),
            ..Default::default()
        },
        "tos",
        sign_algorithm,
    )
    .expect("client")
}

#[test]
fn object_endpoint_percent_encodes_key_segments_once() {
    let client = endpoint_test_client(TosSignAlgorithm::ByteTosV1);
    assert!(client
        .object_endpoint("bucket", "dir/create_topic_with_%DLQ%_test.py")
        .unwrap()
        .ends_with("/dir/create_topic_with_%25DLQ%25_test.py"));
    assert!(client
        .object_endpoint("bucket", "literal-%25/a b/中文?#.txt")
        .unwrap()
        .ends_with(
            "/literal-%2525/a%20b/%E4%B8%AD%E6%96%87%3F%23.txt"
        ));
}

#[test]
fn bytetos_v1_signs_the_encoded_wire_path() {
    let client = endpoint_test_client(TosSignAlgorithm::ByteTosV1);
    assert_eq!(
        client.object_request_path("bucket", "dir/%DLQ%").unwrap(),
        "/bucket/dir/%25DLQ%25"
    );
}

#[test]
fn tos4_keeps_raw_path_for_single_canonical_encoding() {
    let client = endpoint_test_client(TosSignAlgorithm::Tos4);
    assert_eq!(
        client.object_request_path("bucket", "dir/%DLQ%").unwrap(),
        "/dir/%DLQ%"
    );
}
```

Extend the existing TCP-listener test to assert that the actual request line
contains `%25DLQ%25` and does not contain raw `%DLQ%`.

- [x] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test -p tos-core infra::client::tests::object_endpoint_percent_encodes_key_segments_once -- --exact
cargo test -p tos-core infra::client::tests::bytetos_v1_signs_the_encoded_wire_path -- --exact
```

Expected: endpoint and V1 path assertions fail because raw `%` remains.

- [x] **Step 3: Implement one-time object path encoding**

Reuse the existing RFC3986 encoder:

```rust
fn encoded_object_key_path(key: &str) -> String {
    url_encode_with_safe(key, "/")
}

pub fn object_endpoint(&self, bucket: &str, key: &str) -> Result<String, CliError> {
    Ok(format!(
        "{}/{}",
        self.bucket_endpoint(bucket)?,
        encoded_object_key_path(key)
    ))
}

pub fn object_request_path(&self, bucket: &str, key: &str) -> Result<String, CliError> {
    validate_bucket_name(bucket)?;
    if self.sign_algorithm == TosSignAlgorithm::ByteTosV1 {
        return Ok(format!("/{}/{}", bucket, encoded_object_key_path(key)));
    }
    // TOS4 signer canonicalizes this raw logical path exactly once.
    let is_path_style = self
        .endpoint
        .as_deref()
        .map(|endpoint| !endpoint_uses_virtual_hosted_style(endpoint))
        .unwrap_or(false);
    if is_path_style {
        Ok(format!("/{}/{}", bucket, key))
    } else {
        Ok(format!("/{}", key))
    }
}
```

- [x] **Step 4: Run focused and crate tests**

Run:

```bash
cargo test -p tos-core infra::client::tests
```

Expected: all client tests pass, including existing ASCII and query encoding
tests.

### Task 2: Add replayable streaming upload retries

**Files:**
- Modify: `crates/tos-core/src/infra/client.rs`
- Modify: `crates/tos/src/domain/core.rs`
- Modify: `crates/tos/src/handler/high_level.rs`
- Test: `crates/tos-core/src/infra/client.rs`

- [x] **Step 1: Add a failing body-factory retry test**

Create a local TCP server that returns 500 for the first request and 200 for
the second, captures both bodies, and assert:

```rust
assert_eq!(attempts.load(Ordering::SeqCst), 2);
assert_eq!(captured_bodies, vec![b"replay".to_vec(), b"replay".to_vec()]);
```

Add a non-retryable 400 test asserting one attempt and a retry-exhaustion test
asserting `max_retry_count + 1` attempts.

- [x] **Step 2: Run the focused retry tests and verify RED**

Run:

```bash
cargo test -p tos-core infra::client::tests::replayable_streaming_request_rebuilds_body_after_500 -- --exact
```

Expected: compilation fails because the replayable streaming API does not
exist.

- [x] **Step 3: Implement a body-factory retry primitive**

Keep the existing one-shot `send_streaming_request` API for compatibility and
add:

```rust
pub async fn send_replayable_streaming_request<F, Fut>(
    &self,
    method: Method,
    url: &str,
    path: &str,
    query_params: BTreeMap<String, String>,
    extra_headers: BTreeMap<String, String>,
    payload_hash: String,
    mut body_factory: F,
) -> Result<Response, CliError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Body, CliError>>,
{
    for attempt in 0..=self.max_retry_count {
        let body = body_factory().await?;
        let result = self
            .send_signed_request_once(
                method.clone(),
                url,
                path,
                query_params.clone(),
                extra_headers.clone(),
                payload_hash.clone(),
                Some(body),
            )
            .await;
        // Apply the same response/error retry predicates and backoff as
        // non-streaming requests.
        match result {
            Ok(response)
                if should_retry_response(response.status())
                    && attempt < self.max_retry_count =>
            {
                drop(response);
                sleep_before_retry(attempt).await;
            }
            Ok(response) => return Ok(response),
            Err(CliError::Http(error))
                if should_retry_reqwest_error(&error)
                    && attempt < self.max_retry_count =>
            {
                sleep_before_retry(attempt).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(CliError::TransferFailed(
        "HTTP retry loop exhausted".to_string(),
    ))
}
```

Each iteration calls `send_signed_request_once`, so signing and PSM resolution
are repeated.

- [x] **Step 4: Add the structured domain wrapper**

Add `execute_object_replayable_streaming_request` in
`crates/tos/src/domain/core.rs`. It constructs URL/path once, delegates retry
attempt creation to `TosClient`, then preserves the existing response envelope
shape.

- [x] **Step 5: Route file-backed PutObject and UploadPart through the factory**

Use closures that reopen the file:

```rust
core::execute_object_replayable_streaming_request(
    client,
    "ve-tos cp upload",
    Method::PUT,
    bucket,
    key,
    query,
    headers,
    payload_hash,
    || file_stream_body(source),
)
.await
```

For UploadPart use:

```rust
|| file_part_stream_body(source, offset, current_size)
```

Do not route non-replayable stdin readers through this API.

- [x] **Step 6: Run retry and high-level tests**

Run:

```bash
cargo test -p tos-core infra::client::tests
cargo test -p ve-tos-cli-core handler::high_level::tests
```

Expected: retry attempt/body tests and existing transfer tests pass.

### Task 3: Make SHA256 reads asynchronous and remove automatic CRC64

**Files:**
- Modify: `crates/tos/src/handler/high_level.rs`
- Modify: `crates/tos/src/registry.rs`
- Modify: `docs/high_level_commands_plan.md`
- Test: `crates/tos/src/handler/high_level.rs`

- [x] **Step 1: Change header tests to require no CRC64**

Replace existing CRC assertions:

```rust
#[test]
fn test_tos_cp_simple_upload_headers_omit_unsupported_crc64() {
    let headers = cp_simple_upload_headers(
        &ObjectWriteOptions::default(),
        42,
        EffectiveOverwriteStrategy::Force,
    );
    assert_eq!(headers.get("content-length").map(String::as_str), Some("42"));
    assert!(!headers.contains_key("x-hash-crc64ecma"));
}

#[test]
fn test_tos_cp_multipart_part_headers_omit_unsupported_crc64() {
    let headers = cp_multipart_upload_part_headers(64);
    assert_eq!(headers.get("content-length").map(String::as_str), Some("64"));
    assert!(!headers.contains_key("x-hash-crc64ecma"));
}
```

Add async hash tests:

```rust
#[tokio::test]
async fn async_file_and_part_sha256_match_in_memory_digest() {
    let payload = vec![0xA5; 3 * 1024 * 1024 + 7];
    fs::write(&path, &payload).unwrap();
    assert_eq!(file_sha256(path_str).await.unwrap(), hash_payload(&payload));
    assert_eq!(
        file_part_sha256(path_str, 17, 2 * 1024 * 1024).await.unwrap(),
        hash_payload(&payload[17..17 + 2 * 1024 * 1024])
    );
}
```

- [x] **Step 2: Run focused tests and verify RED**

Run:

```bash
cargo test -p ve-tos-cli-core test_tos_cp_simple_upload_headers_omit_unsupported_crc64 -- --exact
cargo test -p ve-tos-cli-core async_file_and_part_sha256_match_in_memory_digest -- --exact
```

Expected: header signature/hash future tests fail against the current APIs.

- [x] **Step 3: Implement async SHA256 helpers**

Use a shared async reader helper:

```rust
async fn hash_async_reader<R>(reader: &mut R) -> Result<String, CliError>
where
    R: AsyncRead + Unpin,
{
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}
```

Convert file and part helpers to `tokio::fs::File`, async seek, and async take.
Await both call sites before constructing replayable bodies.

- [x] **Step 4: Remove automatic high-level CRC64**

Remove automatic CRC calculations and header arguments from:

- simple local file upload;
- multipart object and part upload;
- high-level stdin simple and multipart upload;
- response CRC verification.

Set new multipart `CompletedPart.crc64` values to `None`. Retain optional schema
fields and low-level explicit flags for compatibility.

- [x] **Step 5: Retain CRC compatibility-read tests and update upload docs**

Delete high-level CRC helper/vector tests that no longer have production
callers. Update registry and design documentation to describe SHA256-only
automatic high-level uploads.

- [x] **Step 6: Run focused tests**

Run:

```bash
cargo test -p ve-tos-cli-core handler::high_level::tests
cargo test -p tos-cli-core
```

Expected: all tests pass without automatic CRC request headers.

### Task 4: Remove sequential sync HEAD checks and add bounded local upload concurrency

**Files:**
- Modify: `crates/tos/src/handler/high_level.rs`
- Test: `crates/tos/src/handler/high_level.rs`

- [x] **Step 1: Add failing pure inventory comparison tests**

Introduce tests for the desired synchronous comparison API:

```rust
#[test]
fn tos_to_local_sync_compares_list_metadata_without_head() {
    let dir = temp_dir("sync-list-metadata");
    let local_path = dir.join("a");
    fs::write(&local_path, vec![0_u8; 10]).unwrap();
    let item = TransferPlanItem {
        relative_key: "a".to_string(),
        source: "tos://bucket/a".to_string(),
        destination: local_path.to_string_lossy().into_owned(),
        size: 10,
        etag: Some("e1".to_string()),
        crc64: None,
        last_modified: Some("Wed, 29 May 2024 08:37:23 GMT".to_string()),
    };
    let mut args = sync_args("tos://bucket/", dir.to_str().unwrap());
    args.size_only = true;
    assert!(sync_item_should_skip(&item, None, &args).unwrap());
}

#[test]
fn tos_to_tos_sync_uses_destination_inventory() {
    let item = TransferPlanItem {
        relative_key: "a".to_string(),
        source: "tos://src/a".to_string(),
        destination: "tos://dst/a".to_string(),
        size: 10,
        etag: Some("e1".to_string()),
        crc64: None,
        last_modified: Some("Wed, 29 May 2024 08:37:23 GMT".to_string()),
    };
    let mut destination_entry = object_entry("a");
    destination_entry.size = 10;
    destination_entry.etag = Some("e1".to_string());
    let destinations = HashMap::from([(
        "tos://dst/a".to_string(),
        destination_entry,
    )]);
    let args = sync_args("tos://src/", "tos://dst/");
    assert!(sync_item_should_skip(&item, Some(&destinations), &args).unwrap());
}
```

The API receives no `TosClient`, making accidental HEAD requests impossible.

- [x] **Step 2: Run tests and verify RED**

Run:

```bash
cargo test -p ve-tos-cli-core tos_to_local_sync_compares_list_metadata_without_head -- --exact
cargo test -p ve-tos-cli-core tos_to_tos_sync_uses_destination_inventory -- --exact
```

Expected: compilation fails because `sync_item_should_skip` does not exist.

- [x] **Step 3: Build the destination inventory once**

For recursive TOS destinations, parse the destination prefix, call the existing
recursive list helper once, and map entries by their full destination URI:

```rust
type SyncDestinationInventory = HashMap<String, ObjectEntry>;

async fn build_sync_destination_inventory(
    client: &TosClient,
    destination: &str,
    recursive_list_mode: Option<RecursiveListMode>,
    list_concurrency: usize,
) -> Result<SyncDestinationInventory, CliError> {
    let mut target = parse_tos_uri(destination, true)?;
    normalize_recursive_tos_target(&mut target);
    let prefix = target.key.clone().unwrap_or_default();
    let is_hns = bucket_is_hns(client, &target.bucket).await?;
    let entries = list_object_entries_recursive(
        client,
        &target.bucket,
        Some(&prefix),
        resolve_tos_recursive_list_mode(is_hns, recursive_list_mode),
        list_concurrency,
    )
    .await?;
    Ok(entries
        .into_iter()
        .map(|entry| {
            (
                format!("tos://{}/{}", target.bucket, entry.key),
                entry,
            )
        })
        .collect())
}
```

The local-to-TOS specialized path continues using its existing
`destination_manifest`.

- [x] **Step 4: Replace async per-item HEAD comparison**

Replace `sync_mapping_should_skip` with a pure comparison using:

- `TransferPlanItem` source size/ETag/last-modified from ListObjects;
- local filesystem metadata for local destinations;
- the destination inventory for TOS destinations.

Missing destination entries mean copy. Missing source comparison metadata falls
back conservatively to copy rather than skip. Transfer-time HEAD remains inside
existing copy/download futures.

- [x] **Step 5: Make local-to-TOS upload execution bounded-concurrent**

Replace the serial loop with a `FuturesUnordered` queue limited by
`runtime.batch_concurrency`. Preserve:

- current skip decisions from `destination_manifest`;
- progress and report counters;
- per-item warning output;
- delete-extra execution only when all uploads succeed.

Use the existing recursive executor loop shape rather than introducing a new
framework.

- [x] **Step 6: Add scan progress counters**

Extend `RemoteScanProgress` with atomic discovered-prefix and object counters,
and call:

```rust
scan_progress.record_prefix(scan.entries.len() as u64);
```

after each completed prefix future. The spinner message reports both counters.
Disabled progress remains a no-op. Preserve the scanner's existing pending and
in-flight termination logic.

- [x] **Step 7: Add inventory/progress tests and review the bounded concurrency queues**

Use test futures with atomics to prove:

```rust
assert!(peak_active.load(Ordering::SeqCst) > 1);
assert!(peak_active.load(Ordering::SeqCst) <= configured_limit);
assert_eq!(scan_progress.counts(), (expected_prefixes, expected_objects));
```

Also retain existing sync timestamp, delete ordering, include/exclude, report,
and manifest tests unchanged.

- [x] **Step 8: Run focused high-level tests**

Run:

```bash
cargo test -p ve-tos-cli-core handler::high_level::tests
cargo test --test high_level_principles_test
```

Expected: all sync and high-level contract tests pass.

### Task 5: Version, compatibility review, and full regression

**Files:**
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `crates/adrive/Cargo.toml`
- Modify: `crates/tos-core/Cargo.toml`
- Modify: `crates/tos/Cargo.toml`
- Modify: `crates/toscli/Cargo.toml`
- Modify: `crates/tostable/Cargo.toml`
- Modify: `crates/tosvector/Cargo.toml`
- Modify: `packaging/cargo/tos-cli/Cargo.toml`
- Modify: `packaging/cargo/ve-adrive-cli/Cargo.toml`
- Modify: `packaging/cargo/ve-tos-cli/Cargo.toml`

- [x] **Step 1: Run targeted regression before version changes**

Run:

```bash
cargo test -p tos-core
cargo test -p ve-tos-cli-core
cargo test -p tos-cli-core
cargo test --test high_level_principles_test
cargo test --test core_principles_test
```

Expected: all targeted tests pass.

- [x] **Step 2: Update all release versions to 1.0.3**

Change package versions and path dependency constraints from 1.0.2 to 1.0.3
across workspace and packaging manifests. Regenerate matching lock entries with:

```bash
cargo check --workspace
```

- [x] **Step 3: Run formatting and compile checks**

Run:

```bash
cargo fmt --all -- --check
cargo check --workspace
```

Expected: both commands exit 0 with no warnings introduced by this change.

- [x] **Step 4: Run the full workspace regression**

Run:

```bash
cargo test --workspace
```

Expected: all workspace unit and integration tests pass.

- [x] **Step 5: Perform the mandatory Reviewer pass**

Review the complete diff as a first-time reader against:

- correctness and edge cases;
- signing/encoding compatibility;
- retry idempotency and resource cleanup;
- concurrency bounds and delete safety;
- performance and Tokio worker blocking;
- checkpoint/manifest/output compatibility;
- observability and test coverage.

Fix all Critical and Major findings, annotating required fixes with
`// [Review Fix #N]` comments, and rerun the affected tests.

- [x] **Step 6: Verify final repository state**

Run:

```bash
git diff --check
git status --short
git diff --stat
```

Confirm that only the approved implementation, tests, documentation, plan, and
version files changed.
