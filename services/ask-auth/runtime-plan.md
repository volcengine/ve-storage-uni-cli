# Ask authorization runtime implementation plan

> **For agentic workers:** Use `subagent-driven-development` to implement the task and run specification review before quality review.

**Goal:** Give the already implemented authorization service one owner for HTTP, cleanup and database shutdown, ready for subsequent trusted platform wiring.

**Architecture:** Retain the approved TCC/OAuth architecture and the disabled standalone default. An explicitly constructed runtime owns a validated `LoginService` and `CleanupWorker`; it derives resource handles from those exact components, rather than accepting unrelated pools. It supervises both futures, stops new work when either exits or shutdown arrives, and closes storage within bounded shutdown time. This step makes no claims about platform connectivity or M0 acceptance.

**Tech stack:** Existing Rust, Tokio, Axum and SQLx dependencies only.

## Task 1: Storage shutdown and injected runtime

Files:

- Add `src/runtime.rs` and focused private runtime helpers if necessary.
- Export `runtime` in `src/lib.rs`.
- Extend `src/storage/mysql.rs`, `src/storage/store.rs` and `src/storage/store/cleanup.rs` only with closed shutdown capabilities.
- Add crate-private accessors in `src/login/mod.rs` and `src/cleanup.rs` for the runtime to derive its exact resources.
- Add focused runtime tests and actual-MySQL closure tests.
- Extend `README.md` with assembly order and the remaining platform boundary.

Implementation checklist:

- [x] Write failing storage closure tests: close twice is safe; subsequent API/cleanup operations fail; outstanding borrows do not allow new acquisitions.
- [x] Run those tests and retain the expected missing-capability failure.
- [x] Add documented asynchronous close capabilities without exposing a pool, connection, SQL or mutation escape hatch.
- [x] Write failing runtime tests for normal shutdown, worker exit, worker panic/cancellation, HTTP failure, in-flight HTTP drain, incomplete headers and shutdown timeout. Internal dependency injection may simulate task completion; production entrypoints must accept only validated service components.
- [x] Implement `AuthorizationRuntime::new(service, worker)` and `serve(listener, shutdown)` with at most five arguments per function. A mismatched application/deployment/crypto binding must fail construction before listening. Extract the exact owned Store handles internally.
- [x] Observe cleanup completion and panic, stop accepting HTTP, signal cleanup and cancel in-flight work after a five-second drain. Close the API and cleanup pools concurrently with a bounded additional close budget. Every spawned task must be awaited or aborted and awaited; future cancellation must not leave detached HTTP/cleanup tasks.
- [x] Give shutdown and simultaneous worker completion deterministic precedence: a requested shutdown is normal, unexpected worker/server exit is an error. Errors must be fixed sanitized enum/display text and retain no underlying error bodies.
- [x] Ensure partial constructor/startup failure and cancellation cannot leave a background cleanup task. Retained external Arc handles must not keep a closed pool usable.
- [x] Keep `main.rs`, environment allowlist, SQL migrations and frozen OpenAPI contracts unchanged; do not add a fake production adapter or implicit allow policy.
- [x] Run focused tests, `cargo test -p ask-auth --locked`, `cargo clippy -p ask-auth --all-targets --locked -- -D warnings` and formatting.
- [x] Run independent specification review, then independent seven-category quality review; fix all Critical/Major findings with `// [Review Fix #N]` explanations and re-review.
- [x] Commit only reviewed task files; no push.

Runtime commit: `b1fbf71`.

Acceptance examples:

```rust
// The actual constructor must derive its resources from these components.
let runtime = AuthorizationRuntime::new(login_service, cleanup_worker)?;
runtime.serve(listener, shutdown_signal).await?;
```

```rust
// Representative real database acceptance, with synthetic test credentials only.
store.close().await;
store.close().await;
assert!(store.session(uuid::Uuid::now_v7()).await.is_err());
cleanup_store.close().await;
assert!(cleanup_store.cutoff().await.is_err());
```

## Platform work tracked separately

The TCC documentation now points to company-maintained `byted-tcc` (V2 SDK mode for V3 data). Inspect its current implementation before choosing SDK configuration, authentication and freshness behavior; cached `get`/watch events alone are not evidence of a healthy remote source. The CDN HTTP export is unsuitable for secret configuration. Official OAuth pages establish wire shapes, but test-application identity, omitted refresh-token handling, actual state roundtrip and account coverage remain M0 evidence. Do not silently enable real authorization while those are unverified.

This plan implements Task 1 now while platform investigation continues independently. It does not declare the final binary deployable.

## Validation and review record

- Independent specification review: initial 2 Major findings (unpolled cancellation and ambiguous closure assertions), both fixed with Review Fix #53 and explicit pool-state tests; final 0 Critical / 0 Major / 0 Minor.
- Independent seven-category quality review: passed, 0 actionable findings. Coder self-review also fixed rejected-constructor closure and drain error precedence (#51/#52).
- Root verification: 84 ordinary Rust tests passed, 2 additional actual MySQL closure tests passed; 94 database-dependent cases remain ignored in the ordinary suite. Build, strict all-target Clippy, formatting and workspace check passed.
- Evidence: `/private/tmp/ask-auth-stage5-runtime-verification.json` and `/private/tmp/ask-auth-stage5-{mysql_storage,cleanup_mysql}-close.log`.
- Standalone main and real platform activation remain unchanged; this unit is an injected runtime owner, not proof of production readiness.
