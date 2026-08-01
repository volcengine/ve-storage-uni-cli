# TOS Transfer Reliability and Sync Optimization Design

## Context

The current TOS transfer paths have six related correctness and performance
problems:

1. Local-to-TOS sync uploads files serially.
2. Streaming PutObject and UploadPart requests bypass the HTTP retry loop.
3. File SHA256 calculation performs blocking file reads in async call paths.
4. High-level uploads calculate and send CRC64 even though the ByteTOS service
   contract only supports SHA256 payload verification.
5. Object keys are concatenated into HTTP URLs without path encoding.
6. Recursive sync can appear stuck during hierarchical listing and then spend
   substantial time performing sequential per-object HEAD requests.

The observed source prefix contains 28,190 objects and 4,039 directories.
Hierarchical listing completes in approximately one minute with dual-stack
address resolution, while the configured IPv6-only path can fail during a
child-prefix request. Existing partial download files prove that another run
completed listing and reached the transfer stage. There is no evidence of a
logical lock deadlock in the prefix scanner; slow or failed network futures and
sequential HEAD requests explain the observed behavior.

## Goals

- Preserve all existing CLI arguments, sync comparison modes, overwrite
  behavior, manifests, reports, checkpoints, delete safety gates, and output
  envelopes.
- Make local-to-TOS sync file transfers honor `batch_concurrency`.
- Keep the current hierarchical scanner while making scan progress and failures
  observable.
- Use list metadata for sync comparison and avoid sequential source HEAD
  requests.
- Retry replayable file-backed PutObject and UploadPart streams by rebuilding
  the body for every attempt.
- Calculate file and part SHA256 with asynchronous file reads.
- Stop automatic high-level CRC64 calculation, transmission, and verification.
- Encode object keys exactly once at the HTTP path boundary.
- Upgrade all workspace and packaging crate versions from 1.0.2 to 1.0.3.

## Non-Goals

- Replacing the hierarchical recursive-list algorithm with flat listing. The
  `tos` command surface requires delimiter-based traversal.
- Removing public optional CRC64 fields from checkpoint, manifest, response, or
  low-level explicit API models. Retaining these fields avoids format and CLI
  compatibility breaks.
- Retrying non-replayable stdin streams.
- Replacing all transfer code used by cp, mv, and sync with a new framework.
- Removing the existing transfer-time safety HEAD until equivalent conditional
  request behavior is proven by regression tests.

## Architecture

### 1. Inventory-driven sync planning

Existing entry points and command semantics remain unchanged.

- Local-to-TOS keeps its existing destination ListObjects manifest and in-memory
  size/mtime comparison. Its serial upload loop is replaced with the same
  bounded `FuturesUnordered` execution pattern used by recursive transfers.
- TOS-to-local uses source size, ETag, and last-modified values already returned
  by ListObjects. Local metadata is checked before any fallback request.
- TOS-to-TOS builds a destination inventory for the mapped destination prefix and
  compares source and destination entries in memory.
- Missing comparison metadata is handled conservatively by copying the item;
  sync planning does not issue sequential fallback HEAD requests.
- Copy, download, delete, manifest, report, checkpoint, overwrite, include, and
  exclude behavior continue to use the existing implementations.

The optional manifest remains a one-run audit artifact. It is written before
execution, is not accepted as future input, and is not a completion or resume
marker, so this optimization does not introduce a temporary-manifest protocol.

The transfer-time HEAD currently used to obtain a fresh ETag for conditional
copy or download remains in place. It runs inside the bounded transfer future,
so it no longer serializes the entire plan.

### 2. Recursive-list observability and resilience

The current pending-prefix and `FuturesUnordered` scanner remains intact.
Progress state is extended to report:

- completed prefix requests;
- discovered objects;
- currently active scan phase.

Explicit `--list-echo` continues to work outside a TTY. Existing request retry
configuration remains authoritative. Prefix request failures retain the
specific prefix and retryable error context so network failure is not reported
as an unexplained hang. PSM resolution and signing are repeated for each retry.

The change does not silently override an explicitly configured address family.
Operational use of `--addr-family dual-stack` remains available when the
environment's IPv6 endpoints are unhealthy.

### 3. Replayable streaming upload retries

File-backed simple and multipart uploads use an outer retry helper whose
operation creates a fresh body for every attempt:

- PutObject reopens the source file.
- UploadPart reopens the source file, seeks to the part offset, and limits the
  stream to the part length.
- The precomputed SHA256 value is reused.
- Request signing and PSM resolution occur again on every attempt.
- Connection errors, request timeouts, HTTP 408, HTTP 429, and HTTP 5xx are
  retryable.
- Validation errors, authentication failures, conditional conflicts, and other
  HTTP 4xx responses are not retried.

PutObject writes the same bytes to the same key and UploadPart writes the same
bytes to the same upload ID and part number, making these retries replayable.
For an ambiguous `If-None-Match: *` timeout, an eventual conditional conflict
is preserved as a conflict rather than being converted into success.

### 4. Asynchronous SHA256

File and part SHA256 helpers become async and use `tokio::fs::File`,
`AsyncSeekExt`, and bounded `AsyncReadExt` reads. SHA state is updated in 1 MiB
chunks. This removes blocking disk reads from Tokio worker threads without
changing the digest or loading the file into memory.

File-level transfer concurrency remains the bound on simultaneous hashing.
No unbounded `spawn_blocking` jobs are introduced.

### 5. CRC64 compatibility boundary

Automatic high-level TOS upload flows stop:

- scanning files or parts for CRC64;
- sending `x-hash-crc64ecma`;
- comparing upload or complete responses with a locally computed CRC64;
- writing newly computed CRC64 values into multipart completion entries.

Existing optional CRC64 fields remain readable and serializable to preserve old
checkpoint and manifest compatibility. Low-level explicit API flags are not
removed in this change. Registry text, comments, and tests are updated so they
no longer claim that high-level ByteTOS uploads automatically use CRC64.

### 6. Object-key HTTP encoding

Object keys remain raw logical strings inside CLI parsing and domain models.
Only the HTTP transport boundary encodes the key:

- each UTF-8 path segment is RFC3986 percent-encoded;
- slash separators remain separators;
- literal percent signs become `%25`;
- query strings continue to use their existing independent encoder.

ByteTOS V1 signs the same encoded path that is placed on the wire. TOS4
continues to canonicalize the raw logical path once, avoiding double encoding.
ASCII unreserved keys therefore produce the same URL and signature as before.

## Error Handling

- Retry exhaustion returns the last meaningful transfer error and preserves the
  existing retryable classification.
- A failed sync item is recorded in the batch report and does not trigger
  delete-extra execution.
- Listing failures identify the prefix being scanned.
- Upload retries recreate file resources on every attempt; opened files and
  response bodies are dropped between attempts.
- SHA256 read or seek errors remain I/O errors and are not converted into HTTP
  retries.

## Compatibility Constraints

- No CLI flag is removed or renamed.
- JSON, table, CSV, YAML, and Markdown envelope shapes remain unchanged.
- Existing checkpoint and manifest files containing CRC64 remain parseable.
- Existing overwrite and conditional-write behavior remains unchanged.
- Sync delete actions still execute only after all copy actions succeed.
- Existing single-object cp and mv semantics remain unchanged apart from
  corrected key encoding, SHA I/O scheduling, removal of the unsupported
  automatic CRC64 header, and retry of replayable transient failures.

## Test Strategy

Tests are added before implementation and must fail for the expected missing
behavior.

1. HTTP object-path tests cover `%DLQ%`, literal `%25`, spaces, plus signs,
   question marks, fragments, Unicode, and slash-separated segments for both
   ByteTOS V1 and TOS4 without double encoding.
2. Local mock-server tests force a transient PutObject or UploadPart failure,
   verify the configured attempt count, and verify identical bytes and part
   offsets on the successful replay.
3. Async SHA256 tests compare file and part hashes against known in-memory
   digests across multiple read buffers.
4. Header tests verify that automatic simple, multipart, and stdin high-level
   uploads no longer send `x-hash-crc64ecma`.
5. Sync planning tests verify that list metadata avoids source HEAD requests,
   destination inventories are reused, missing metadata causes a conservative
   copy, and local-to-TOS active transfers never exceed `batch_concurrency`.
6. Recursive-list tests cover multi-level prefix termination, progress counts,
   and propagation of the failing prefix.
7. Existing targeted crate tests, workspace tests, `cargo check --workspace`,
   and formatting checks run after implementation.

## Versioning

All workspace crate package versions, path dependency version constraints,
packaging crate versions, and matching `Cargo.lock` entries are updated from
1.0.2 to 1.0.3 after behavioral tests pass.
