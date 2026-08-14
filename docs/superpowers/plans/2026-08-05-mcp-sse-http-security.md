# MCP SSE HTTP Security Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Require a fresh startup Bearer token and loopback Host/Origin validation on every local MCP SSE HTTP request without changing stdio behavior.

**Architecture:** Add a focused `http_security` module in `tos-core` that generates the ephemeral credential, validates HTTP headers, strips Authorization before rmcp observes request parts, and wraps an Axum router. Refactor `TosMcpServer::run_sse` to build rmcp's router explicitly, bind it on loopback, apply the security layer, print the connection secret once to stderr, and reuse the existing rmcp service loop. Because all three CLI surfaces use this shared server, one transport fix protects `ve-tos`, ByteTOS `tos`, and `ve-adrive`; their help and dry-run metadata are updated together.

**Tech Stack:** Rust 2021, rmcp 0.8.5, Axum 0.8, Tokio, SHA-256, `subtle` constant-time comparison, Cargo tests.

---

## File structure

- Create `crates/tos-core/src/mcp/http_security.rs`: token generation, fixed-size token digest, Host/Origin/Bearer policy, Axum middleware, sanitized authenticated marker, and unit/router tests.
- Modify `crates/tos-core/src/mcp/mod.rs`: register the private HTTP security module.
- Modify `crates/tos-core/src/mcp/server.rs`: manually compose and serve rmcp's protected SSE router and print the startup credential once.
- Modify `crates/tos-core/Cargo.toml`: add direct runtime dependencies used by the shared transport and the Tower test utility.
- Modify `crates/tos/src/{cli/meta.rs,handler/meta.rs}`, `crates/adrive/src/{cli/meta.rs,handler/meta.rs}`, and `crates/toscli/src/{cli/meta.rs,handler/meta.rs}`: synchronize help and startup-plan metadata.
- Modify `tests/cli_basic.rs`: assert every public CLI describes the Bearer/loopback contract without emitting a token during dry-run/describe.
- Modify `README.md`: document local SSE startup, one-time stderr token output, and Authorization-header use.

### Task 1: HTTP security policy and router middleware

**Files:**
- Create: `crates/tos-core/src/mcp/http_security.rs`
- Modify: `crates/tos-core/src/mcp/mod.rs`
- Modify: `crates/tos-core/Cargo.toml`

- [ ] **Step 1: Add the failing security-policy tests**

Create tests in `http_security.rs` for the wished-for API:

```rust
#[test]
fn generated_token_is_32_random_bytes_in_base64url() {
    let first = generate_bearer_token();
    let second = generate_bearer_token();
    assert_ne!(first, second);
    assert!(!first.contains('='));
    assert_eq!(URL_SAFE_NO_PAD.decode(first).unwrap().len(), 32);
}

#[test]
fn policy_accepts_only_exact_local_authority_and_optional_local_origin() {
    let token = generate_bearer_token();
    let policy = McpHttpSecurity::new(&token, 19090);
    assert!(policy.authorize(&headers("127.0.0.1:19090", None, &token)).is_ok());
    assert!(policy.authorize(&headers("localhost:19090", Some("http://localhost:19090"), &token)).is_ok());
    assert_eq!(policy.authorize(&headers("evil.example:19090", None, &token)), Err(SecurityError::ForbiddenHost));
    assert_eq!(policy.authorize(&headers("127.0.0.1:19090", Some("https://evil.example"), &token)), Err(SecurityError::ForbiddenOrigin));
}

#[tokio::test]
async fn middleware_protects_every_route_and_strips_authorization() {
    let token = generate_bearer_token();
    let app = protect_router(test_router(), McpHttpSecurity::new(&token, 19090));
    assert_eq!(request(&app, Method::GET, "/sse", None).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(request(&app, Method::POST, "/message", Some("wrong")).await.status(), StatusCode::UNAUTHORIZED);
    let response = request(&app, Method::GET, "/sse", Some(&token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_text(response).await, "authorization-stripped");
}
```

The test helper builds `HeaderMap` values with exact Host, optional Origin, and optional Bearer headers. The dummy route returns `authorization-leaked` if it can still see Authorization and `authorization-stripped` otherwise.

- [ ] **Step 2: Run the focused tests and verify RED**

Run:

```bash
cargo test -p tos-core mcp::http_security -- --nocapture
```

Expected: compilation fails because `http_security`, `generate_bearer_token`, `McpHttpSecurity`, `SecurityError`, and `protect_router` do not exist.

- [ ] **Step 3: Add the direct dependencies and minimal implementation**

Add to `crates/tos-core/Cargo.toml`:

```toml
axum = "0.8"
subtle = "2"
tokio-util = "0.7"

[dev-dependencies]
tower = { version = "0.5", features = ["util"] }
```

Register `mod http_security;` in `crates/tos-core/src/mcp/mod.rs`.

Implement these private types and functions in `http_security.rs`:

```rust
const TOKEN_BYTES: usize = 32;

pub(super) struct McpHttpSecurity {
    token_digest: [u8; 32],
    port: u16,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct AuthenticatedMcpHttpRequest;

pub(super) fn generate_bearer_token() -> String;
pub(super) fn protect_router(router: Router, security: McpHttpSecurity) -> Router;

impl McpHttpSecurity {
    pub(super) fn new(token: &str, port: u16) -> Self;
    fn authorize(&self, headers: &HeaderMap) -> Result<(), SecurityError>;
}
```

`generate_bearer_token` fills `[u8; 32]` using `OsRng` and encodes with `URL_SAFE_NO_PAD`. `authorize` parses `Host` as `http::uri::Authority`, requires the configured port and exact `127.0.0.1` or ASCII-case-insensitive `localhost`, validates an optional Origin with `url::Url`, parses a single Bearer value, hashes the candidate with SHA-256, and compares the fixed-size digest using `subtle::ConstantTimeEq`.

The middleware returns `400` for missing/malformed Host, `403` for disallowed Host/Origin, and `401` plus `WWW-Authenticate: Bearer` for Bearer failures. On success it removes Authorization, inserts `AuthenticatedMcpHttpRequest` into request extensions, and calls `next.run(request)`.

Every externally visible type/function receives a documentation comment; the module remains private to `tos-core::mcp`.

- [ ] **Step 4: Run focused tests and verify GREEN**

Run:

```bash
cargo test -p tos-core mcp::http_security -- --nocapture
```

Expected: all token, policy, error-status, and header-stripping tests pass.

- [ ] **Step 5: Commit Task 1**

```bash
git add crates/tos-core/Cargo.toml crates/tos-core/src/mcp/mod.rs crates/tos-core/src/mcp/http_security.rs Cargo.lock
git commit -m "fix: authenticate local MCP SSE requests"
```

### Task 2: Secure rmcp SSE server composition and startup output

**Files:**
- Modify: `crates/tos-core/src/mcp/server.rs`
- Test: `crates/tos-core/src/mcp/server.rs`

- [ ] **Step 1: Add failing server-composition tests**

Add tests for a private `build_sse_runtime` helper:

```rust
#[tokio::test]
async fn prepared_rmcp_router_rejects_anonymous_sse_before_session_creation() {
    let runtime = build_sse_runtime(([127, 0, 0, 1], 19090).into());
    let response = runtime.router.oneshot(
        Request::builder()
            .uri("/sse")
            .header(HOST, "127.0.0.1:19090")
            .body(Body::empty())
            .unwrap(),
    ).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
```

The runtime contains the protected router, rmcp `SseServer`, and generated token. A companion assertion checks that the token is absent from the router's unauthorized response body.

- [ ] **Step 2: Run the server test and verify RED**

Run:

```bash
cargo test -p tos-core prepared_rmcp_router -- --nocapture
```

Expected: compilation fails because `build_sse_runtime` is not implemented.

- [ ] **Step 3: Refactor `run_sse` to serve the protected router**

Replace the direct `SseServer::serve(bind)` call with a helper that:

```rust
let token = generate_bearer_token();
let config = SseServerConfig {
    bind,
    sse_path: "/sse".to_string(),
    post_path: "/message".to_string(),
    ct: CancellationToken::new(),
    sse_keep_alive: None,
};
let (server, router) = SseServer::new(config);
let router = protect_router(router, McpHttpSecurity::new(&token, bind.port()));
```

`run_sse` then binds `tokio::net::TcpListener`, starts `axum::serve` with graceful cancellation, attaches the existing cloned MCP service using `server.with_service`, and only after bind success writes one startup block to stderr:

```text
MCP SSE listening on http://127.0.0.1:<port>/sse
Authorization: Bearer <token>
```

The spawned HTTP task logs only server failures. Ctrl-C cancels both HTTP serving and rmcp service tasks. No function body exceeds 50 lines; listener startup and output formatting are extracted into single-purpose helpers.

- [ ] **Step 4: Run server and full tos-core tests**

Run:

```bash
cargo test -p tos-core prepared_rmcp_router -- --nocapture
cargo test -p tos-core
```

Expected: the anonymous real rmcp route returns `401`; all `tos-core` tests pass.

- [ ] **Step 5: Commit Task 2**

```bash
git add crates/tos-core/src/mcp/server.rs
git commit -m "fix: serve MCP SSE through protected router"
```

### Task 3: Synchronize public CLI contracts and documentation

**Files:**
- Modify: `crates/tos/src/cli/meta.rs`
- Modify: `crates/tos/src/handler/meta.rs`
- Modify: `crates/adrive/src/cli/meta.rs`
- Modify: `crates/adrive/src/handler/meta.rs`
- Modify: `crates/toscli/src/cli/meta.rs`
- Modify: `crates/toscli/src/handler/meta.rs`
- Modify: `tests/cli_basic.rs`
- Modify: `README.md`

- [ ] **Step 1: Add failing CLI contract assertions**

Extend the existing serve describe/dry-run tests for all three surfaces:

```rust
assert_eq!(parsed["data"]["authentication"], "ephemeral_bearer");
assert_eq!(parsed["data"]["token_output"], "stderr_once_after_bind");
assert_eq!(parsed["data"]["authorization_header_required"], true);
assert_eq!(parsed["data"]["allowed_hosts"], json!(["127.0.0.1:<port>", "localhost:<port>"]));
assert!(!stdout.contains("Bearer "));
```

For stdio plans, assert `authentication == "process_stdio"`, `authorization_header_required == false`, and no token-output field contains a credential.

- [ ] **Step 2: Run CLI tests and verify RED**

Run:

```bash
cargo test --test cli_basic serve_describe_documents_mcp_transport_details -- --nocapture
cargo test --test cli_basic byted_tos_serve_and_skill_schema_are_registry_backed -- --nocapture
```

Expected: assertions fail because the new authentication fields and help text are missing.

- [ ] **Step 3: Add consistent metadata and help text**

For SSE plans, add these non-secret fields:

```json
{
  "authentication": "ephemeral_bearer",
  "token_output": "stderr_once_after_bind",
  "authorization_header_required": true,
  "allowed_hosts": ["127.0.0.1:<port>", "localhost:<port>"],
  "origin_policy": "missing or exact loopback origin on the configured port"
}
```

For stdio, report process-pipe authentication and `authorization_header_required: false`. Update all `serve --help` long text to explain that SSE is same-host only, prints a fresh token once, and requires `Authorization: Bearer` on every HTTP request. Update README's MCP section with a local SSE example and explicitly prohibit URL query credentials.

- [ ] **Step 4: Run focused CLI tests and verify GREEN**

Run:

```bash
cargo test --test cli_basic serve -- --nocapture
cargo test --test cli_basic mcp_stdio -- --nocapture
```

Expected: serve metadata/help tests and existing stdio runtime tests pass.

- [ ] **Step 5: Commit Task 3**

```bash
git add README.md tests/cli_basic.rs crates/tos/src/cli/meta.rs crates/tos/src/handler/meta.rs crates/adrive/src/cli/meta.rs crates/adrive/src/handler/meta.rs crates/toscli/src/cli/meta.rs crates/toscli/src/handler/meta.rs
git commit -m "docs: describe authenticated MCP SSE startup"
```

### Task 4: Full verification and security review

**Files:**
- Review every file changed since commit `f82e726`

- [ ] **Step 1: Format and compile**

Run:

```bash
cargo fmt --all -- --check
cargo check --workspace
```

Expected: formatting and compilation succeed without warnings introduced by this change.

- [ ] **Step 2: Run focused security and CLI regression suites**

Run:

```bash
cargo test -p tos-core mcp::http_security -- --nocapture
cargo test -p tos-core prepared_rmcp_router -- --nocapture
cargo test --test cli_basic serve -- --nocapture
cargo test --test cli_basic mcp_stdio -- --nocapture
```

Expected: all focused tests pass with zero failures.

- [ ] **Step 3: Run the complete workspace suite**

Run:

```bash
cargo test --workspace
```

Expected: all workspace unit and integration tests pass.

- [ ] **Step 4: Perform mandatory Reviewer pass**

Review the complete diff against the design by category: correctness, security,
performance, maintainability, robustness, testability, and observability. In
particular verify fail-closed middleware placement, credential stripping before
rmcp logging, constant-time comparison, no token in URL/JSON/traces, exact
Host/Origin parsing, all-route coverage, clean cancellation, and unchanged
stdio behavior. Fix every Critical or Major issue with `[Review Fix #N]`
comments explaining why, then rerun the entire review checklist.

- [ ] **Step 5: Verify the original attack preconditions are rejected**

Start the debug SSE server and send anonymous and hostile-header requests:

```bash
target/debug/ve-storage-uni-cli ve-tos serve --mcp --transport sse --port 19191
curl -i -H 'Host: evil.example:19191' -H 'Origin: http://evil.example:19191' http://127.0.0.1:19191/sse
curl -i -H 'Host: 127.0.0.1:19191' http://127.0.0.1:19191/sse
```

Expected: hostile Host returns `403`; exact loopback without Authorization returns `401`; a request carrying the printed token and exact loopback Host opens the SSE stream.

- [ ] **Step 6: Commit review fixes if any**

```bash
git add Cargo.toml Cargo.lock README.md crates tests docs/superpowers/plans/2026-08-05-mcp-sse-http-security.md
git commit -m "fix: address MCP SSE security review"
```
