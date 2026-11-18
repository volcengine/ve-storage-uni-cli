# Ask authorization platform implementation plan

> **For agentic workers:** Use `subagent-driven-development`, with independent specification review before quality review for each task.

**Goal:** Finish the locally testable platform adapter and deployment code before asking the operator to deploy BOE or provide test application credentials.

**Architecture:** Official documented OAuth wire behavior is implemented and tested locally with synthetic credentials. Real authorization remains disabled until the documented test-application gates pass. TCC uses approved company SDK capabilities, not CDN export or a plain-secret fallback. Internal SDK build dependencies must be isolated from the public CLI workspace.

**Tech stack:** Existing Rust/Tokio/Reqwest, plus a separately locked internal deployment package using pinned company SDKs.

## Confirmed source evidence

- OAuth authorization: https://open.larkoffice.com/document/store-app-development/development-configuration/authenticate-and-authorize/get-authorization-code
- Exchange: https://open.larkoffice.com/document/store-app-development/development-configuration/authenticate-and-authorize/get-user_access_token
- Refresh: https://open.larkoffice.com/document/store-app-development/development-configuration/authenticate-and-authorize/refresh-user_access_token
- Identity: https://open.larkoffice.com/document/store-app-development/development-configuration/authenticate-and-authorize/get-user-information
- TCC SDK overview: https://cloud.bytedance.net/docs/tcc/docs/646447998e0ca902369dc10e/65f1815d6e6bae030cef8e48
- Rust SDK guide: https://bytedance.larkoffice.com/wiki/wikcnGPgb2n9g1kC131drLDDATf
- Inspected SDK source: `rust-lang/servery` commit `153533abb652843f2600fa4ded89f7b626369d05`; published `byted-tcc = 0.8.8` successfully compiled in an isolated temporary project.

Official OAuth pages were read on 2026-11-16. Authorization and token endpoints are respectively `https://accounts.doubao.com/open-apis/authen/v1/authorize` and `https://accounts.doubao.com/oauth/v3/token`; user information uses `https://accounts.doubao.com/open-apis/authen/v1/user_info`. Token POST is form-encoded with client_id/client_secret. Success fields are top-level, including actual scope and second-based expiry durations. Refresh success rotates the RT and immediately invalidates the old RT. Userinfo is a code/msg/data envelope; all identity fields may be omitted. Missing identity or tenant must fail closed, not be inferred from a decoded JWT or treated as a personal account.

## Task 1: Documented OAuth wire client, initially unconnected

Files: add focused modules under `src/doubao/`, export in `src/lib.rs`, reuse workspace Reqwest dependency in `Cargo.toml`, add meaningful wire and parser tests, extend README.

- [x] Test first: constructor validation, exact authorization URL encoding, exact form fields, actual scope parsing, token/userinfo envelope differences, conservative expiry calculation and verified identity.
- [x] Add a documented provider constructor taking trusted application credentials, the fixed HTTPS callback and required scopes. Keep secret wrappers without Debug and erase owned sensitive strings on drop. Require `offline_access` and reject empty/duplicate/malformed scope entries.
- [x] Build an explicit HTTPS-only Reqwest client: redirects disabled, referer disabled, automatic retries disabled, no ambient proxy, bounded connect/read/overall deadlines. Production endpoints are constants, not user-supplied URLs. No request or response body is logged.
- [x] Construct authorization URL through `url::Url`; fixed query is client_id/response_type/scope, then server-owned state/redirect_uri. Validate the existing canonical 64-character state and fixed callback path.
- [x] POST once with `application/x-www-form-urlencoded`; exchange includes grant_type/client_id/client_secret/code/redirect_uri, refresh includes grant_type/client_id/client_secret/refresh_token. No Basic authentication or PKCE behavior is invented.
- [x] Read bodies incrementally with a strict total limit; reject invalid status/content type, oversized/truncated/malformed JSON, duplicate recognized fields and contradictory success/error shapes. Token size is at most the existing 16 KiB bound per credential; scope count at most 100.
- [x] Compute expiries from a whole-millisecond timestamp sampled before token request start, with checked duration arithmetic and supported timestamp range. Both positive durations and both nonempty tokens are required. Omitted refresh_token never falls back to the old value.
- [x] Fetch userinfo once with the new access token; require code zero, nonempty app-scoped open_id and nonempty tenant_key. Map open_id to the broker's provider user_id and tenant_key to tenant_id. Omission/null does not prove tenant absence; unsupported identities fail closed pending platform evidence. Never trust unverified JWT fields, email or CLI claims.
- [x] Implement the existing OAuthProvider trait, keeping the standalone default disabled. No refresh error is classified NoConsumption without specific platform evidence; all uncertain refresh results are OutcomeUnknown. Verify the actual required scopes, not an echoed request.
- [x] Exercise real local test transport for encoding, redirects/no credential forwarding, single-call behavior, timeout/response-size limits and identity follow-up. Test endpoint overrides must be private and compiled only in tests; no HTTP escape in production API.
- [x] Test at least normal exchange+refresh, missing identity, missing/reused RT, scope loss, duration overflow/zero/expiry during userinfo, malformed/oversized body, redirect and lost refresh response. Use synthetic credentials only.
- [x] Run package tests, strict Clippy and formatting; independently review specification then all seven quality categories. Fix Critical/Major with traced Review Fix comments. Commit reviewed files only, no push.

Acceptance interface (the implementation may choose focused supporting types):

```rust
let provider = DoubaoOAuth::new(credentials, callback_url, required_scopes)?;
let endpoints = provider.endpoints();
// LoginService continues to own reservation, admission and final-release checks.
let adapters = LoginAdapters { provider: Arc::new(provider), admission };
```

## Remaining tasks and activation boundary

TCC integration must obtain positive remote freshness, not redate a cached SDK value. Inspected V2 `get_raw`/`ensure` retain old values after remote refresh failure; watcher events occur only on value changes. A production control publisher therefore needs an uncached, error-reporting SDK capability or another approved method. `byted-bconfig`, the public lower-level SDK used by byted-tcc, explicitly documents uncached requests; inspect and test that capability before freezing the adapter. Do not recreate TCC auth or discovery HTTP yourself.

After the reader contract is verified, add the isolated internal deployment package, strict ordinary/encrypted TCC payload schemas, startup assembly, a separate migration command and deployment/build/run instructions. Exact TCE cluster addresses, real credentials and resources are deployment inputs and are not prerequisites for the local implementation. Only runtime tests in that environment can verify connectivity and real OAuth behavior. Keep test application/account/permission/64-character-state/RT-rotation/tool-boundary coverage recorded as unverified until measured; do not claim internal/external account support based on static fixtures.

## Task 1 verification record

- Root independently ran 104 ordinary Rust tests, including all 20 OAuth tests, with zero failures. The 94 database-gated cases remain ignored in the ordinary suite; no new database behavior is introduced by this adapter.
- Build, strict all-target Clippy, formatting and root workspace check all passed. Evidence: `/private/tmp/ask-auth-stage5-oauth-root-verification.json`.
- Independent specification and seven-category quality reviews passed with 0 remaining Critical / Major / Minor findings. Coder regression tests cover fixes #54–#58, including reused RT, impossible authorization URL and typed/discarded userinfo msg.
- No real App ID was needed or used. Standalone startup and real-platform acceptance are unchanged and unverified.

## Object and depth enforcement follow-up

Shared TCC review exposed Serde positional-array acceptance and ignored-subtree depth behavior. Seven targeted regressions reproduced acceptance before fixes #69–#72. OAuth envelope/data and both enabled/disabled HTTP seams now enforce object-only schemas and the shared 64-level depth limit while preserving duplicate-field checks and bounded additive fields. Independent specification and quality reviews passed with no remaining Critical/Major/Minor. Root reran the full package (121 passed; 94 database-gated cases ignored), strict Clippy, formatting and workspace check: all passed. Evidence: `/private/tmp/ask-auth-json-hardening-root-verification.json`. This changes enforcement, not the frozen HTTP schema or SQL.
