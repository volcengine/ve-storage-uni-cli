# ask-auth 核心协议与架构参考

当前构建与部署以 [README.md](README.md) 为准。以下保留既有核心协议、存储和生命周期说明；本地无参数入口始终禁用，部署使用同包 `serve`。

## Stage-1 bootstrap and local verification

Run from the workspace root. Add `--offline` after dependencies are cached:

```sh
cargo run -p ask-auth --locked
# Explicit listener override; binding beyond loopback is an operator choice.
ASK_AUTH_LISTEN=127.0.0.1:8081 ASK_AUTH_LOG_LEVEL=info cargo run -p ask-auth --locked
curl -i http://127.0.0.1:8080/healthz
curl -i http://127.0.0.1:8080/readyz
```

Only `ASK_AUTH_LISTEN` (a numeric SocketAddr, default `127.0.0.1:8080`)
and `ASK_AUTH_LOG_LEVEL` (`off`, `error`, `warn`, `info`, `debug`, `trace`,
default `info`) are supported. Other `ASK_AUTH_` settings are rejected with a
fixed error; no App Secret, token, database, TCC or mock-provider configuration
is accepted. Invalid configuration and listener failures exit unsuccessfully
without printing setting values. SIGINT and SIGTERM stop accepting connections
and allow up to five seconds for the entire server to drain, starting when the
shutdown signal arrives. This also bounds connections with incomplete HTTP
headers that have not reached the Router. If draining exceeds that deadline,
the process closes remaining connections through runtime shutdown, prints only
a fixed safe deadline error and exits unsuccessfully; ordinary graceful shutdown
exits successfully. Signal setup and listener failures also exit unsuccessfully.

`GET /healthz` returns 200 `{ "status": "ok" }`; `GET /readyz` always returns
503 `{ "status": "unavailable" }`. Frozen business routes validate the v1.15
request shape and then return 503 `upstream_unavailable`. This bootstrap cannot
create a login, authenticate a credential, exchange a code, issue tokens, call
an upstream provider or persist anything. Its internal injectable mock is
compiled only for tests and cannot be enabled in the binary.

Malformed requests receive sanitized 400 `invalid_request`; missing or malformed
required Bearer proofs receive 401 `authentication_required`. Claim and refresh
require one canonical UUIDv7 `Idempotency-Key`. Status takes only query `auth_id`;
logout takes JSON `auth_id`. auth_id is canonical lowercase RFC-variant UUIDv7;
claim_request_id, refresh request_id and trace request_id are independently
generated UUIDv7. Nonces are canonical
32-byte unpadded Base64URL, JSON rejects unknown and duplicate fields, and bodies
are limited to 32 KiB. A five-second total handler deadline includes body reads.
The callback validates canonical full 48-byte/64-character state with UUIDv7
prefix, exactly one code/error and unique, validly encoded query parameters;
it returns fixed HTML 400/503 with `no-referrer`, never supplied parameter values.

Every application response has `Cache-Control: no-store` and a fresh server
UUIDv7 `X-Request-ID`; JSON errors carry that same ID. Supplied request IDs are
ignored. Unknown routes and unsupported methods return sanitized JSON 404/405.
Request logs contain only the matched route template (or a fixed unmatched
marker), an allowlisted method, generated request ID, status and duration.
The production subscriber admits only the dedicated safe request, cleanup and
configuration event targets, even at debug/trace levels; it excludes dependency
logs. No raw paths, queries,
headers, request bodies, provider responses or credentials are logged.

The frozen contracts live in [`contracts/`](contracts/):
`ask-auth-v1.openapi.json` (OpenAPI API version 1.0.0, design v1.15),
`ask-result-v1.schema.json`, and sanitized `ask-auth-v1.15.fixtures.json`.
Lifecycle fixtures specify the injected login implementation and later refresh/logout behavior.
The service adds no CLI command/help/discovery surface and no `/ask/proxy` route.

Run the focused checks:

```sh
cargo fmt -p ask-auth --check
cargo test -p ask-auth --locked
cargo clippy -p ask-auth --all-targets --locked -- -D warnings
cargo build -p ask-auth --locked
python3 -m venv services/ask-auth/.venv
services/ask-auth/.venv/bin/pip install -r services/ask-auth/tests/requirements.txt
services/ask-auth/.venv/bin/python services/ask-auth/tests/validate_contracts.py -v
```

Rust tests exercise the real Router and middleware, malformed/duplicate input,
canonical encodings, response headers, redacted logs, body limits/deadlines,
startup failures and injected-provider isolation. The Python suite uses pinned
standard validators to check OpenAPI, JSON schemas, sanitized fixtures and real
HTTP responses, then verifies graceful termination. Local socket tests require
permission to bind an isolated loopback listener. `ASK_AUTH_TEST_BINARY` may
select a separately built binary for that Python test process only.

Stage 2 implements versioned HMAC/AEAD, validated rows, writer-only MySQL pools,
explicit migrations and bounded Store transactions. It is a library only: the
HTTP binary still accepts no database/key configuration and always keeps business
routes and readiness at 503. The stage-3 library below enables four login routes
only with explicit trusted adapters. Stage 4a extends that same injected boundary
with refresh, status and logout. The internal host now wires TCC, the fixed
Doubao provider, separate database pools and cleanup controls. Its deployment,
distributed edge limits and CLI integration remain later stages. Health and
local tests do not establish M0/M1/M3, deployment readiness or OAuth success.

## Injected runtime lifecycle

`runtime::AuthorizationRuntime::new(LoginService, CleanupWorker)` validates the
components without I/O or starting work. Cleanup must share the exact immutable
Crypto capability already validated by LoginService against its API Store,
application, deployment and key version. A copied key ring also fails validation.
Validation failure permanently closes both component pools, including external
Store handles retained by the host, without starting any runtime work.
The runtime derives both pools from those components; callers cannot supply
replacement shutdown pools or obtain a raw database/SQL capability.

`serve(listener, shutdown_future)` consumes the runtime and a caller-bound TCP
listener. The signal future returns `std::io::Result<()>`; the host owns OS signal
registration. This entry point serves HTTP/1 only, with TLS terminated upstream.
It caps active connections at 256, headers at 100 and the header buffer at 32 KiB,
with a five-second header-read deadline. Existing Router body limits and handler
deadlines remain in effect. The host must configure its ingress consistently.

The supervisor handles a requested shutdown, signal error, cleanup return/panic,
and HTTP exit/failure/panic. It stops accepting connections and signals cleanup,
drains both concurrently for at most five seconds, drops any remaining handler,
socket and cleanup futures, then closes the exact API/cleanup pools concurrently
within three additional seconds. Storage close methods reject new acquisitions
immediately, remain closed if their returned future is cancelled, and may be
awaited repeatedly. Existing handles to either Store share that closed pool.

There are no spawned HTTP/cleanup tasks: owned connection futures also drop when
the caller cancels `serve`. The constructor owns both pool close guards, so
dropping an unused runtime or an unpolled `serve` future closes both pools too.
Cancellation synchronously closes both pool admission
gates; awaiting `serve` to completion additionally awaits connection closure
acknowledgements within the close budget. Trusted adapters and observers must
yield and return promptly as required by their existing contracts.

When several events are ready together, shutdown wins over cleanup, which wins
over HTTP. Clean simultaneous component completion is expected after shutdown;
component failures during draining still fail shutdown. The first initiating
error is preserved over drain/close failures; after a successful signal, cleanup
failure precedes HTTP failure, then drain timeout, then close timeout. Results
contain fixed RuntimeError classes only, with no source errors or panic payloads.
Rust's process panic hook runs before unwinding; the embedding host must install
a sanitized hook if its trusted components can panic with sensitive values.

This library entry point does not change the standalone binary, its five-second
shutdown behavior, environment contract, frozen HTTP schemas or migrations. The
core binary remains disabled. The companion internal host supplies platform
provider/policy/control wiring; deployment and activation gates remain open.
Runtime tests do not prove OAuth success.

## Stage-3 injected login boundary

`build_enabled_router(Arc<LoginService>)` explicitly enables:

- `POST /v1/auth/login-attempts` (201), exact-payload retries and original URL/deadline reuse;
- `GET /v1/auth/login-attempts/{auth_id}`, ticket-authenticated public state without identity/tokens;
- `GET /v1/oauth/doubao/callback`, committed one-winner reservation, one exchange and guarded outcomes;
- `POST /v1/auth/token`, ticket plus SHA256 ASCII verifier authentication, first claim and bounded exact replay.

Construct `EnabledConfig` from `DeploymentIdentity`, an explicit key version,
required scopes and fixed server `OAuthEndpoints`. Callback must be HTTPS at
`/v1/oauth/doubao/callback`, without query/fragment/credentials. Authorization
output must match its approved HTTPS origin/path and fixed query pairs, with
exact server state/redirect_uri; unapproved pairs, credentials, fragments and
undocumented PKCE configuration are rejected. App/deployment and the Store's
actual crypto-ring capability must match. Configuration is immutable and has no
environment fallback. `LoginAdapters` requires trusted `OAuthProvider` and
`AdmissionProvider`. The documented `doubao::DoubaoOAuth` wire adapter described
below is available for explicit injection. This core has no TCC client or mock
enable switch; the companion internal host performs trusted TCC/provider wiring.
Neither binary supplies undocumented PKCE behavior.
A provider adapter must return verified identity/scope and actual token expiries;
it must not log secrets and must promptly return an exchange future without
blocking in its synchronous method. Its authorization-URL builder performs no I/O.

State contains UUIDv7 bytes plus 32 CSPRNG bytes and uses the frozen full-state
HMAC purpose. Context/result caches are authenticated with immutable metadata
and independent version-selected encryption. Callback commits pending->exchanging
before I/O, exchanges at most once with a ten-second deadline, and leaves a
cancelled worker exchanging until original expiry. Current admission is explicitly
enabled, versioned, at most five minutes old, and denies before allowing users or
tenants. It is checked at callback, claim, replay and enabled readiness. Claim
cache expiry is capped by DB now + five minutes and all necessary credential
expiries; authorization expiry remains unchanged. Final credential release locks
and confirms the exact snapshot and DB-clock deadlines. Bounded conflicts reread
and classify refresh/revoke/cleanup without releasing an old result.

The enabled router has a twenty-second total handler budget; JSON body reads
remain bounded to five seconds/32 KiB. The default router keeps the five-second
total budget. Safe fixed HTML includes no reflected input or credentials,
and keeps no-store/no-referrer. `/readyz` on the enabled router requires a writable
validated Store and a fresh enabled admission snapshot; it does not assert live
provider connectivity, release-gate completion or deployment readiness.
`build_router()` and `main` continue to use the disabled bootstrap, including 503
readiness and business routes.

## Documented Doubao OAuth wire adapter

`doubao::AppCredentials::new(app_id, doubao::SecretString::new(secret))` validates
owned trusted credentials. `doubao::DoubaoOAuth::new(credentials, callback_url,
required_scopes)` constructs an adapter without network requests. Required scopes
must include `offline_access`. Use its `endpoints()` in `EnabledConfig` and inject
the adapter through `LoginAdapters.provider` into the existing runtime. No real
App ID or platform resources are needed to develop or test this library.

The closed HTTPS endpoints are:

- Authorization: `https://accounts.doubao.com/open-apis/authen/v1/authorize`.
- Token exchange/refresh: `https://accounts.doubao.com/oauth/v3/token`.
- Verified userinfo: `https://accounts.doubao.com/open-apis/authen/v1/user_info`.

Token POSTs use form fields `client_id`/`client_secret`; success fields are at the
top level, with actual scopes and positive lifetimes. The adapter verifies every
configured scope before requesting userinfo using the new Bearer access token.
`open_id` maps to broker `user_id` and is unique within the registered app;
`tenant_key` maps to `tenant_id`. Missing, null or empty tenant identity fails
closed. External/personal-account support without a verified tenant remains
unverified; email, decoded JWTs and CLI claims never establish identity.

Every operation has a nine-second combined budget (inside the service's ten
seconds), two-second connect, three-second read and eight-second request limits.
Response bodies are read incrementally with 64 KiB token and 32 KiB userinfo
limits. OAuth schema objects and POST bodies reject positional JSON arrays;
container nesting, including ignored additive fields, is capped at 64 levels.
Redirects, proxies, Referer forwarding and automatic retries are disabled.
Credentials, bounded response buffers and parsed tokens erase owned sensitive
storage on drop; raw upstream errors are never logged or exposed. Refresh always
requires a newly rotated RT; omission, reuse, errors or transport uncertainty
return `OutcomeUnknown`, because no documented error proves non-consumption.

Tests use synthetic credentials and private test-only loopback endpoints to check
real HTTP encoding, single calls, redirect denial, lost responses, deadlines and
body bounds. Production exposes no endpoint/client override. These tests do not
prove live BOE OAuth or satisfy M0 activation. The core standalone `main`, environment,
frozen contracts and DDL remain unchanged and disabled.

The wire contract follows the official documentation checked on 2026-11-16:
[authorization code](https://open.larkoffice.com/document/store-app-development/development-configuration/authenticate-and-authorize/get-authorization-code),
[token exchange](https://open.larkoffice.com/document/store-app-development/development-configuration/authenticate-and-authorize/get-user_access_token),
[refresh](https://open.larkoffice.com/document/store-app-development/development-configuration/authenticate-and-authorize/refresh-user_access_token),
and [userinfo](https://open.larkoffice.com/document/store-app-development/development-configuration/authenticate-and-authorize/get-user-information).

## Stage-4a injected authorization lifecycle

The enabled router additionally implements refresh, status and logout using the
unchanged frozen v1.15 schemas. `OAuthProvider::refresh` is mandatory: trusted
adapters receive a zeroizing RT and return effective verified `ProviderTokens`
or `RefreshFailure`. Only `NoConsumption(NoConsumptionReason)` permits a failed
ledger and unchanged active credentials; every uncertain outcome, ten-second
timeout or invalid post-consumption contract closes authorization as unknown.
The documented Doubao wire adapter is available for explicit injection; there is
no default-success adapter. Adapter future construction must return promptly
without blocking; provider I/O belongs inside
the bounded future. An effective RT is required; preserving an omitted RT needs
platform proof inside the trusted adapter, never a service guess.

Exact retained refresh requests authenticate their complete canonical input using
R's fixed key version, including the original broker/RT/generation, even after A
rotates. Successful refresh clears all eight login fields before rotating A's
credential digests/key version/generation. Replay checks output generation,
complete decrypted bundle metadata/digests, current admission/scope and all
credential deadlines, then confirms the exact A->R pair under locks. Concurrent
changes trigger bounded rereads and sanitized classification, never old output.

Status authenticates the selected A's current broker before identity projection.
It enforces natural expiry even without a worker. A fresh versioned disabled or
denied policy returns `is_allowed=false`; unavailable/stale/malformed policy
returns 503. Final locked snapshots prevent old rotated brokers receiving identity.
Logout does not consult admission. It atomically ends A, clears its login group
and settles the bounded pending child as unknown. Existing completed caches
become logically unreleasable and retain their original short TTL until cleanup;
logout does not perform unbounded historical child updates. More than one pending
child is an integrity error and rolls back the entire operation. Retained expired,
reauth_required and revoked rows keep their original terminal status/reason;
`{status:revoked}` acknowledges that they are unusable without rewriting that
terminal history. Repeated current-broker logout succeeds while A is retained;
old rotated brokers fail authentication. This revokes service authorization and
does not promise immediate revocation of an already issued provider access token.

The [lifecycle plan](lifecycle-plan.md) records stage 4a boundaries and the separate
stage 4b cleanup handoff.

Opt-in router tests use fake adapters and the same isolated accounts as the Store
tests. They exercise actual MySQL, callback/claim races, current admission, expiry,
worker cancellation, recreated IDs, authenticated malformed caches and ciphertext
tampering. Python `jsonschema` validates actual successful/error response payloads
against the frozen OpenAPI schemas. Select an installed pinned validator using
`ASK_AUTH_TEST_PYTHON` (defaults to `python3`):

```sh
# Use the isolated runtime/migration URLs described below; never print credentials.
ASK_AUTH_TEST_PYTHON=services/ask-auth/.venv/bin/python \
  cargo test -p ask-auth --locked --test login_mysql -- --ignored --test-threads=1
```

The implementation sequence and explicit scope are recorded in [login-plan.md](login-plan.md).

## Stage-2 library and isolated MySQL verification

Inject `DatabaseConfig`, a `Crypto` key ring and immutable app/deployment identity
through library constructors; there is no environment or production key fallback.
Use `Uuid::now_v7()` for auth_id before durably saving the future CLI request.
Design v1.15 uses canonical RFC-variant UUIDv7 for auth_id, claim_request_id,
refresh request_id, Idempotency-Key, trace request_id and ask-result request_id.
Generate and persist each operation ID independently; never reuse auth_id as an
operation key or trace ID. Doubao session_id remains an unrelated opaque value.
BINARY(16) storage keeps the original RFC byte order without time-part swapping.

`MigrationDatabase` connects using a separate deployment account and applies the
embedded initial DDL with SQLx version/checksum/dirty tracking and a 30-second
budget. `_sqlx_migrations` is a deployment ledger, not a third business record;
the schema still has exactly A (20 columns) and R (9 columns). Never auto-reset a
production schema or bypass checksum/dirty failures. Review partial DDL and repair
it operationally. Migration connections are detached and physically closed on
completion, failure or cancellation so MySQL advisory locks cannot leak to a pool.

The runtime account receives only SELECT/INSERT/UPDATE/DELETE on the two business
tables, with no DDL, migration-ledger access or TRIGGER privilege. Deployment uses
an account scoped to the target schema with DDL and ledger DML privileges. Both
connect only to the writer, using `VerifyIdentity` with an explicit CA remotely.
`LoopbackTlsTestOnly` requires TLS while permitting the isolated server's
self-signed test certificate without CA/hostname checks, only for an explicit
numeric loopback host; it rejects URL sockets. Remote connections always retain
`VerifyIdentity` with an explicit CA. SQLx is pinned to 0.9.0 (Rust >=1.94),
with `mysql-rsa` disabled: caching_sha2 authentication runs inside TLS and does
not depend on an RSA exchange or a warmed authentication cache. Connections enforce UTC, strict SQL mode,
utf8mb4_0900_bin, supported MySQL >=8.0.21 and 16 KiB InnoDB pages. Pools are
bounded to 1–32 connections; connection acquisition and each Store operation
have a three-second total budget, with rollback on errors/cancellation.

Creation assigns its ten-minute deadline from DB time and returns the stored
snapshot for subsequent fences; caller clock/deadline cannot shorten or renew it.
Store operations validate complete phase shapes, immutable identities and
original fences, lock A before R, compare exact snapshots and use database time.
They never hold a transaction across provider HTTP. Callback reserves
pending->exchanging before any ready/denied/error result. Refresh completion
updates A and R atomically; only success increments generation, clears the eight
login fields and changes A's HMAC version. Failed refresh keeps credential
bindings/version/generation. Unknown outcomes end authorization. Reads reject
unknown key versions and ciphertext with invalid tags or immutable AAD. A closed
internal login snapshot validates shape/known key first and defers AEAD inspection
until ticket/proof or full callback state authenticates; public reads, writes and
final credential-release confirmation retain complete cryptographic validation.
The Store accepts already authenticated snapshots/digests from its caller; the
injected login layer verifies ticket/proof, trusted identity and admission first.
It does not interpret decrypted TokenBundle/provider payloads or emit integrity
alert events yet; returned errors are fixed safe enums and the HTTP logger
continues excluding SQLx/dependency logs and all raw row/secret values.

Real MySQL tests are deliberately ignored in the default suite. Configure two
explicit URLs for **one disposable isolated database**, then run:

```sh
# Supply these through your local secret manager or child-process environment.
# ASK_AUTH_TEST_DATABASE_URL: runtime account, two-table DML only.
# ASK_AUTH_TEST_MIGRATION_URL: isolated migration account; TRIGGER additionally
# required only by fault-injection tests (not by production migrations).
cargo test -p ask-auth --locked --test mysql_storage -- --ignored --test-threads=1
```

Missing URLs fail explicitly. Do not print URLs/passwords or point these tests at
shared/production data. Tests inject triggers, corrupt/restore the migration
ledger and mutate synthetic rows, with cleanup before assertions. A server with
binary logging may need an administrator to enable trusted test trigger creation.
Default Rust tests cover cryptographic isolation/tampering/rotation/bounds and
phase invariants without connecting to MySQL.

## Two records and resource identity

<!-- [Review Fix #14] Merge login and authorization without duplicating their resource IDs or states. -->
**A (`auth_sessions`) is one device authorization session**, from anonymous login
through usable authorization and termination. Its primary key is
`auth_id`, a UUIDv7 the CLI generates and durably saves before login creation.
The same field name identifies A in every HTTP request, response and local
credential bundle; it also serves as the login-creation idempotency key.
This authorization session is unrelated to Doubao's question `session_id`.
There is one persisted A `status`, with no separate claimed flag or grant row.
A new device login creates a new A, even for the same trusted user.

**R (`auth_refresh_requests`) is one refresh operation**, identified by
`(auth_id, request_id)`. A:R is 1:N. R briefly retains a refresh result or a
certain error for retries. Deleting R never ends an otherwise usable A.
Deleting A requires that all R children have been removed. Relationships are
logical; no foreign key or cascade performs the cleanup.

| Table | Columns | Physical keys and reads |
| --- | ---: | --- |
| `auth_sessions` | 20 | PK `auth_id`: create retry, callback, poll, claim, refresh, status/logout; `login_expires_at`: short-phase cleanup; `auth_expires_at`: authorization expiry/termination |
| `auth_refresh_requests` | 9 | PK `(auth_id, request_id)`: exact refresh/retry; index `(status, expires_at)`: separate scans per refresh state |

Total: **29 columns, 2 primary keys, 0 secondary unique keys, 3 ordinary secondary
indexes, 0 CHECK constraints and 0 foreign keys**. The SQL column comments are
the field dictionary. No broker secondary index is needed: its credential
comparison follows the auth_id primary-key lookup. This replaces the initial
design before deployment;
it is not a migration from existing live tables.

## Phase shapes and the shared Store

Every write path validates the current row, complete proposed row, legal
transition and original-operation predicates under its locks, then uses
parameterized atomic DML/CAS and checks affected rows. Every credential release
also validates the current policy, row shape, AEAD payload, generation and actual
credential deadlines. Invalid stored rows fail closed without an upstream call,
credential release or automatic repair; emit a sanitized integrity alert.
The database enforces types, NOT NULL and keys, not these business invariants.

The **login temporary group** is `request_hash`, `login_ticket_hash`,
`claim_challenge`, `oauth_state_hash`, `encrypted_oauth_context`,
`claim_request_id`, `encrypted_result`, `login_expires_at`. Clearing this group
means setting **all eight columns to NULL atomically**. Individual phase changes
have the following explicit shapes; tenant_id may legitimately be NULL.

| A phase | Required shape |
| --- | --- |
| `pending` / `exchanging` | Request/ticket/state digests, challenge, context and login deadline present; claim ID/result NULL. Identity, scope, broker/RT digests, generation and auth deadline all NULL. |
| `ready` | Request/ticket/state digests, challenge, login deadline and generation-1 result present; context and claim ID NULL. Trusted user, scope, broker/RT digests, generation=1 and auth deadline present. |
| `active` / `refreshing`, login replay retained | Same login digest/challenge group, first claim ID and generation-1 result present; context NULL. Current generation=1 and authorization fields present. Only `active` may release credentials. |
| `active` / `refreshing`, login group cleared | All eight temporary fields NULL; authorization fields and a positive current generation remain. This is the normal long-lived shape. |
| Preauthorization `denied` / `error` / `expired` | Context/result/claim ID NULL; retain original state hash and HMAC version until deletion, plus request/ticket/challenge for authenticated terminal reads. Login deadline is truncated to now. Authorization fields remain NULL. |
| Authorized `expired` / `revoked` / `reauth_required` | Entire login group NULL; retain trusted identity/scope, broker/RT digests, generation and HMAC version until deletion; auth deadline is truncated to now. No credential release or new R. |

<!-- [Review Fix #20] Login expiry is a public projection; it does not terminate an active authorization. -->
`denied` means a definite user/admission refusal. `error` covers token exchange,
identity or provider-contract errors and `login_outcome_unknown` before
establishing authorization. No A state named `claimed` is stored: first claim
is `ready -> active`. While a complete login replay group is retained, HTTP
login state projects `active`/`refreshing` as `claimed` within the replay
window, and `exchanging` as `pending`. If the retained ticket group authenticates
but login_expires_at has elapsed for active/refreshing, poll returns HTTP 200,
public state `expired` and `reason_code=claim_recovery_expired`. This is only the
login view: it does not change A.status or end valid authorization. The projection
does not create a second state field. A cleared login group cannot authenticate
polling/claiming, so those endpoints return
`401 authentication_required`, even while the authorization remains usable.

Validate known states, positive bounded key versions/generations, no overflow,
trusted immutable app identity and a normalized scope array of at most 100
items within granted permissions. R.pending has no result/error; R.completed
has an encrypted result and no error; R.failed/unknown have a sanitized error
and no result. Context envelopes are 32–32768 bytes; result envelopes are
32–262144 bytes. State/size/expiry/cryptographic checks cover callback, claim,
refresh, logout, replay and cleanup, including malformed persisted rows.

## Configuration, credentials and cryptography

Use MySQL >= 8.0.21, InnoDB 16 KiB pages, DYNAMIC rows,
`utf8mb4_0900_bin`, UTC connections and strict SQL mode. UUID values use RFC
binary byte order without time-part swapping. Provider/environment are immutable
deployment configuration, isolated by database. Stored app_id must match the
configured app; never adopt old records by rewriting it. Authoritative reads
and writes use the writer. Configure DB TLS and separate migration/runtime
accounts with minimum privileges.

TCC encrypted configuration holds App Secret, versioned HMAC keys and independent
AES-256-GCM keys. Access is restricted to the approved service PSM; no CDN,
CLI embedding or config export. Admission uses a validated versioned policy,
deny before allow, default deny; check it after identity verification and before
claim, refresh, cached release and online status. The core rejects snapshots
older than five minutes. The internal host immediately clears policy on a fresh
source/parse/clock failure and never falls back to a cached success; initial
load failure or excessive staleness prevents issuance/refresh. Admission
enabled=false also prevents issuance/refresh. Authenticated
logout remains permitted after policy removal. Trusted identity comes from the
provider's verified userinfo or formal JWT-validation mechanism, never merely
decoding a JWT or trusting CLI-supplied user/tenant claims.

<!-- [Review Fix #15] One A key version is safe only if no old login HMAC survives migration. -->
A has **one HMAC version for all its non-NULL HMAC digests**. Keep it unchanged while
any login temporary group remains. On successful refresh, under A->R locks,
clear that entire group and atomically replace both current credential digests,
generation and A HMAC version using the newly issued bundle. A failed refresh
does not migrate A's HMAC version. Each R retains its own original request-HMAC
version independently, so an exact completed retry can authenticate its old
broker/RT after A has advanced. Read each row's stored version before calculating
its HMAC; use domain separation, length-prefixed canonical inputs and constant-time
comparison. Retain all referenced HMAC keys, including terminal-row fences.

<!-- [Review Fix #27] The supplied auth_id selects one row; only its stored broker digest and key version authenticate it. -->
Status/logout require **auth_id plus BrokerBearer**. Validate the UUIDv7, lock A
by its primary key, and use only that row's hmac_key_version to compute and
constant-time compare the broker HMAC. Revalidate app/deployment, row shape and
already-claimed authorization phase before returning anything. Never enumerate
key versions or locate another A by broker digest. A missing well-formed ID,
wrong broker or mismatched ID/token pair returns the same generic
`401 authentication_required`; auth_id alone grants no access.

Reject ready; do not use claim_request_id as a claimed flag, since normal
temporary-group cleanup clears it. Active/refreshing and retained ended
authorization are identified by status and the current broker binding. Status
checks current admission and may report is_allowed=false for its authenticated
owner. Logout remains permitted after policy removal, ends only the selected A
and may replay for a retained ended A with the same valid broker. It never
reactivates authorization; after deletion it returns 401. No broker unique key,
broker lookup index or user-search/device-list index is required.

| Endpoint | Identity and resource input |
| --- | --- |
| `POST /v1/auth/login-attempts` | JSON `{auth_id, login_ticket, claim_challenge, device_name, cli_version}`; the CLI persists its generated auth_id before send and retries the identical payload. |
| `GET /v1/auth/login-attempts/{auth_id}` | Path auth_id plus LoginTicketBearer; no identity or token returned. |
| `POST /v1/auth/token` | JSON `{auth_id, claim_verifier}` plus LoginTicketBearer and independent Idempotency-Key. |
| `POST /v1/auth/refresh` | JSON `{auth_id, refresh_token, credential_generation}` plus BrokerBearer and independent Idempotency-Key. |
| `GET /v1/auth/status?auth_id=<UUIDv7>` | Required nonsecret resource locator in the query plus BrokerBearer; no broker token in a URL. |
| `POST /v1/auth/logout` | Required JSON `{auth_id}` plus BrokerBearer; proof must match that selected authorization. |

Creation/poll responses and TokenBundle expose auth_id under that name. It is
unrelated to Doubao session_id, R.request_id, first claim_request_id or HTTP
trace request_id; those independent identifiers keep their original roles.

AEAD envelope: `format(1 byte, 0x01) || nonzero key_version(2-byte big endian)
|| nonce(12 bytes) || ciphertext || tag(16 bytes)`, AES-256-GCM, fresh CSPRNG
nonce per encryption. Authenticate versioned purpose, immutable deployment
identity, stored app_id and resource identity using length-prefixed AAD:

- OAuth context: A auth_id and context purpose.
- Login result: A auth_id, login-result purpose and constant generation **1**.
- Refresh result: A auth_id, R request_id, refresh-result purpose and
  `input_generation + 1`.

Reject unsupported format/key/size/tag/AAD. Do not bind mutable status/deadlines
or reconstruct old AAD from current generation. Reencryption preserves original
result generation and deadlines. Retain AEAD decryption keys while referenced
by live ciphertext or valid backups; row deletion does not erase backups.
AES, GHASH and POLYVAL explicitly enable zeroize features in addition to
aes-gcm. Repository `.cargo/config.toml` selects `--cfg polyval_force_soft` for
software GHASH/POLYVAL: the pinned hardware-dispatch wrapper does not forward
Drop to its secret hash state. AES keeps its existing backend selection. TCE
builds that override `RUSTFLAGS` must retain `--cfg polyval_force_soft`.
Production compile-time assertions require GHASH/POLYVAL destruction and AES
expanded round keys implementing ZeroizeOnDrop; selecting a backend without Drop
fails compilation with instructions to restore the flag. This backend choice
preserves AES-GCM, its envelope, HTTP contracts and database schema. Raw tokens are only transiently handled or briefly encrypted in A/R; A's durable
credential fields are digests. No sensitive values, digests, ciphertext,
Authorization, upstream bodies, question text or callback query values in logs.

## Callback and login

The fixed HTTPS callback is `/v1/oauth/doubao/callback`, through TLB to TCE;
configure the exact complete redirect_uri on the provider and server. The CLI
opens the URL and polls/claims; no localhost listener is required. Only the
fixed approved provider endpoints may be called. Enable S256 PKCE only after
M0 verifies actual support; the CLI claim proof is a different protocol.

OAuth state is exactly unpadded Base64URL of
`uuid_v7_bytes(auth_id, 16 RFC-order bytes) || server_random_nonce(32 bytes)`:
48 bytes / 64 ASCII characters. Reject duplicate state parameters, padding,
wrong alphabet/length, noncanonical encoding or UUID version/variant. Extracted
auth_id is only an untrusted row locator. Under the A lock verify HMAC of the
complete original state, purpose `ask.oauth-state.v1`, deployment identity and
stored app_id using A's stored version; also validate pending shape and deadline.
An unknown ID or invalid binding returns a generic safe page, with no state/code
reflection or logs. Tickets, claim verifiers/challenges and brokers separately
use canonical 32-byte / 43-character unpadded Base64URL: decode and reencode
identically, rejecting nonzero unused bits. The challenge is
`SHA-256(ASCII(claim_verifier))`, not a caller-selected identity.

1. The CLI persists auth_id, ticket, claim verifier and full pending request
   before sending. Create A.pending with the full request HMAC and original
   `login_expires_at = DB now + 10 minutes`. A create retry compares the exact
   canonical request and ticket under the stored key version, without extending
   the window or regenerating state. An existing ID with a cleared login group
   is closed: return `409 idempotency_conflict`, never overwrite or recreate it.
2. Every valid callback, including provider refusal, atomically claims
   `pending -> exchanging` under the same A lock. CAS matches captured full
   oauth_state_hash, key version and original login deadline, still unexpired.
   Exactly one admitted code callback calls upstream after commit. An admitted
   `error=access_denied` callback enters denied/access_denied; every other valid
   OAuth error enters error/provider_error without retaining the raw error value.
   Both error paths reserve exchanging first and make no token-endpoint call or retry.
3. Success verifies trusted identity, actual scope and current admission, then
   updates **the same A** to ready with trusted fields, current broker/RT digests,
   generation=1, auth deadline and encrypted result. No separate grant is created.
   Clear context, retain original state/digest/deadline binding. Every success,
   denial, failure or unknown completion must still match captured state/version/
   original deadline and exchanging, before that deadline; otherwise write nothing.
4. A definite refusal enters denied; exchange/identity/contract failure or
   uncertain exchange enters error with a sanitized reason. Clear context/result,
   truncate login deadline to now and retain the original-state fence until
   deletion. A lost worker is never taken over: original expiry ends the attempt.
5. Claim verifies ticket/proof, app, policy, ready, generation=1 and both deadlines.
   First claim changes ready->active and stores claim_request_id, changing only
   login expiry to `min(DB now + 5 minutes, actual access/broker/known RT expiry)`.
   Replay requires active, same first claim key/ticket/proof and valid cached
   credentials; it never extends the window. The window may outlive the original
   login deadline. Refreshing releases no cached credentials.

A ready session cannot refresh before first claim. Its login deadline and auth
deadline serve different purposes: ready login expiry ends the never-claimed
session; active login replay expiry only clears the temporary group, leaving
current authorization and generation intact. An active session with a retained
generation-1 result may refresh; successful refresh clears the obsolete group
in the same transaction rather than releasing a superseded login result.

## Refresh, termination and bounded recovery

All paths lock **A before R**, with stable child PK order. Never keep a DB
transaction open across provider HTTP. Use DB time for all phase decisions.
Initial provider connect/total timeouts are 3/10 seconds, DB operation timeout
3 seconds and refresh execution deadline 45 seconds. CLI authorization-service
HTTP timeout is 20 seconds; bounded recovery is 90 seconds, at most three same-ID,
same-payload attempts with exponential backoff and jitter. Endpoint limits start
at create 5/IP/minute, poll 1/auth_id/second, refresh 6/auth_id/minute; TCC may
adjust them after load tests. Distributed limits are at the trusted TLB boundary;
reject spoofed forwarding headers and never accept arbitrary redirect/upstream URLs.

- Lock A, then read exact R. An existing R authenticates its complete canonical
  request using its fixed version, including old broker/RT/auth_id/generation.
  Do not require its old broker to equal current A's broker before exact replay.
  Cached release additionally requires A.active, allowed policy, output generation
  equal to current A and valid response/credential deadlines. Expired/mismatching
  R is not a new operation. Completed replay compares **output**, not input,
  generation against A.
- On R miss require A.active, current broker/RT digests, correct generation,
  app, admission and auth deadline. Reject overflow. Insert R.pending and set
  A.refreshing atomically; the A lock serializes refresh across all replicas.
  Capture original input generation, execution deadline, A broker digest/key version
  and R request digest/key version, then commit and call the provider once. Never
  replace captured operation identity or lease with values read from a later row.
<!-- [Review Fix #18] A reused authorization ID must not inherit an old refresh worker, even at the same generation. -->
- All outcomes relock A->exact R and require A.refreshing, R.pending, matching
  captured input generation, A broker digest/key version, R request digest/key
  version and original execution deadline. These fences also reject a deleted A
  ID reused for a different authorization. Active workers
  additionally require that original lease unexpired; success also requires
  authorization still valid. Failed guards mean **no writes**, including a late
  failed response after revocation. The sweeper checks the same operation but
  requires its original lease expired; it never resends an RT.
- Success validates actual scope, credentials and expiries, clears the complete
  login temporary group, updates A digests/HMAC version/generation+1/auth expiry/
  active and R.completed/encrypted response in one transaction. R expiry becomes
  `min(DB now + 5 minutes, necessary credential expiries)`. A's auth deadline is
  the common service/broker upper bound, initially capped at 30 days and known
  RT lifetime. Without reliable new RT expiry, do not extend the prior bound.
  Retain omitted RT only if the verified provider contract says it remains valid.
- A detected timeout/lost response/unclassified result makes R.unknown and
  A.reauth_required atomically, clears login payloads and truncates auth expiry
  to now. A crashed worker is recovered at lease expiry. No takeover, replacement
  operation or automatic old-RT retry is permitted.
- R.failed means the provider contract proves **no credential consumption**;
  retain a sanitized error for at most five minutes. Restore A.active only if
  authorization remains valid; natural expiry means A.expired, known credential
  invalidity means A.reauth_required. These ended paths clear the login group
  and truncate auth expiry. Without the no-consumption guarantee use unknown.
- Logout/revocation immediately ends A and truncates auth expiry; natural expiry
  ends it as expired. Settle pending R as unknown before deleting R children;
  already revoked/expired A stays unusable. No outcome resurrects it. Clearing
  admission does not immediately revoke already-issued Doubao AT: direct access
  lasts until provider revocation/expiry. Do not promise global logout.

<!-- [Review Fix #16] Temporary login cleanup changes neither long-lived authorization nor its generation. -->
The [cleanup worker design](cleanup.md) clears active/refreshing login groups
without deleting A or changing its authorization state, generation or digests.
An ended A is retained until all R children are removed. No permanent tombstones
or arbitrary 24-hour/30-day audit retention are introduced.

While evidence remains, authenticated expired first claim can return
`410 login_expired`. For claim, authenticate ticket/proof before exposing state:
a retained ready first-claim deadline has elapsed -> 410; a retained active/
refreshing login-cache deadline has elapsed -> `409 claim_recovery_expired`
**before** reporting refreshing; within its window refreshing returns
`409 refresh_in_progress`, without releasing a result. Exact authenticated R retry can return
`409 refresh_recovery_expired` or generation conflict. After the login group is
cleared, poll/claim return 401; after A deletion, old resources return 401.
R miss accepts only current A credentials; old/invalid credentials or absent A
return 401. A current authenticated generation mismatch is 409. Never infer past
success from a purged row. A retained ID's changed create payload is 409, and a
cleared login group also closes that create ID with 409, exposing no identity.
A failed R may be attempted after its bounded ledger is deleted only if current
A credentials still permit it; failed proves no prior consumption. The CLI must
not reuse creation/refresh IDs for new operations or generate a new ID to retry
old credentials. Replay guarantees are finite, not an audit history.

## Integration and release gates

Implementation proceeds in this order: finish backend code, deploy to BOE with
authorized test resources, collect M0 evidence, then integrate the CLI. A real
AppID or provisioned BOE resource is not needed for local backend development.
M0 and the remaining measured release gates still control activation and rollout.

<!-- [Review Fix #19] Boundary testing may run after tool-configuration evidence; the full V3 pass gates V1 ask closure. -->
M0 permits an authorized isolated test-app probe using the fixed server callback,
approved secret configuration and test accounts to verify authorization, trusted
identity, ask, refresh and another ask. First verify the platform configuration
evidence disabling tools or limiting them to confirmed side-effect-free behavior;
then the isolated probe may run V3 boundary tests. Only after V3 fully passes may
V1 run the real ask closure. Freeze measured endpoints/scopes, external-account
coverage, identity, PKCE, 64-character state roundtrip including denial, RT rotation/omission/error
classification, quota/tool behavior and real TCC/PSM access before production
activation and CLI ask closure. The core standalone binary remains fail-closed.
The login/authorization lifecycle, Doubao wire adapter, cleanup worker and internal
TCC/provider host are implemented. Local tests cover synthetic providers, real
isolated MySQL and the production VerifyIdentity TLS factory; these checks do not
establish BOE authentication, real TCC ACL access or deployment success.

Apply this initial DDL through one migration lock with version/checksum tracking.
MySQL DDL is not a multi-statement transaction: investigate partial failure and
apply reviewed correction, without automatic drops or IF NOT EXISTS masking drift.
Static parser/SQLite reference checks do not prove real MySQL execution, locks,
SKIP LOCKED, cryptography or HTTP correctness. Run the migration on real MySQL,
inspect SHOW CREATE TABLE/EXPLAIN and verify concurrent create/callback, claim
versus cleanup/revoke, two refreshes, late outcomes, ID reuse, key rotation,
unknown/failed windows, malformed persisted rows and the cleanup acceptance cases.
Only measured runtime/platform/DB results can pass M1/M3 and release gates.

[OAuth 2.0](https://www.rfc-editor.org/rfc/rfc6749),
[native-app guidance](https://www.rfc-editor.org/rfc/rfc8252) and
[OAuth security BCP](https://www.rfc-editor.org/rfc/rfc9700) inform protocol
safety; they do not prescribe these tables or their field counts.


## Injected cleanup worker (stage 4b)

`CleanupDatabase::connect` creates a separate writer-only TLS/UTC pool with
READ COMMITTED, one-second lock waits and ROW-binlog validation. API Store
connections retain their existing isolation and three-second method deadline.
`CleanupStore::new` binds the dedicated pool to the same app/crypto validators;
its discovery objects contain metadata fences and expose no pool, transaction or
arbitrary SQL. No provider operation is reachable from cleanup.

Construct `CleanupWorker` only after that pool validates. Its trusted
`CleanupControl::channel` starts disabled; publish `Some(CleanupSettings)` to
allow work and publish `None` on source failure or operational pause. Dropping
the publisher fails closed. The internal TCC host clears enabled settings on
source/parse/clock failure and recovers only after a new valid read. `run` accepts
a shutdown watch receiver; true or sender closure stops new transactions, and current work finishes or
rolls back within the pass/transaction deadline. `run_once` is a bounded manual
integration entrypoint and retains its rate budget across calls.

The six queues use DB-time cutoffs, fixed-status refresh scans, existing indexes,
metadata-only keysets, parent-first SKIP LOCKED locks, exact original fences and
full authenticated current/target validation. Parent cleanup handles one child
per transaction, preserving terminal reasons and deleting parents last. Every
transaction updates/deletes at most two business rows. The worker conservatively
reserves two rows before opening a transaction, retaining that reservation until
one second after completion or an uncertain result; it keeps reservations across
passes. This can run below the configured maximum when operations change one
row. Initial defaults are operational bounds, not measured capacity evidence.

`CleanupObserver` receives sanitized counters, queue age, measured durations and
alert intents; `TracingCleanupObserver` emits only the exact allowed cleanup
target. TCE metric/alert transport, real TCC/PSM access verification, deployment limits,
replica-lag/DB-pressure feedback and load-test capacity evidence remain release
work. No automatic OPTIMIZE, schema change, job table or leader is introduced.


## TCC transport contract

`TccSource::new` requires Tokio, validates platform metadata and creates local
clients. It starts no global workers and performs no network I/O. The host still
installs restricted tracing and a secret-safe panic hook. Namespace syntax is
1–256 ASCII bytes of nonempty dot-separated alphanumeric/underscore/hyphen
segments; space is one such segment of 1–64 bytes.

Runtime metadata is captured once, with these precedence rules:

| Input | Resolution |
| --- | --- |
| Workload PSM | `LOAD_SERVICE_PSM`, then `PSM`, then `TCE_PSM`; required safe dotted identity |
| Environment header | `SERVICE_ENV`, then `TCE_ENV`, then `prod` |
| Sender cluster header | `SERVICE_CLUSTER`, then `TCE_CLUSTER`, then `default` |
| Target IDC | Explicit `RUNTIME_IDC_NAME`, required safe identifier up to 64 bytes |

There is no shell or interface-probing IDC fallback. Missing/unknown IDC fails
closed. BOE IDCs are exactly `boe`, `boe2` and `boei18n`. Production requires a
non-BOE IDC and presence of `IS_PROD_RUNTIME` or `SERVICE_ENV`, matching the SDK's
classification evidence. Expected environment must match; `TCE_HOST_ENV=ppe` and
`TESTING_PREFIX=offline` are rejected. Present higher-precedence values remain
authoritative, including invalid values. Metadata headers are bounded to 256
visible ASCII bytes. Optional `TCE_STAGE`, `MY_POD_NAME`, `MY_HOST_IP` and
`MY_HOST_IPV6` produce `x-stage`, `x-pod` and `x-host-ip` only when present; host
IP must be numeric, with IPv6 preferred. Malformed supplied metadata is rejected.
The client identifies itself as `ask-auth/<version>` rather than an official SDK.

The BConfig V1 service is `bytedance.bconfig.proxy` for `prod`, `canary` or empty
environment; other environments use `bytedance.bconfig.envs_proxy`. Shared
`tos_core::infra::discovery::ConsulLookupClient` resolves weighted numeric socket
addresses with a 500 ms deadline. The target Consul cluster is always `default`;
`x-cluster` describes the sender only. No BNS/CDN route or public endpoint override
exists. Each request is an authenticated HTTP GET to
`http://<discovered-numeric-address>/v1/keys/<resource-without-leading-slash>`.
It sends validated `x-env`, `x-psm`, `x-cluster`, optional metadata, and
`x-dps-token`. Proxy use, redirects, referrers, automatic retries and decompression
are disabled. Connect, read and total request deadlines are bounded; each HTTP
request, including its body, has a one-second timeout.

DPS comes only from platform `SEC_TOKEN_STRING` when present, otherwise absolute
`SEC_TOKEN_PATH`. Invalid static input never falls back to a file. Credentials
must be nonempty visible ASCII header values, at most 16 KiB, and are marked
sensitive in Reqwest. Static values are held in `Zeroizing<String>`. File open
and reads are asynchronous with a 250 ms deadline, `O_NONBLOCK` and regular-file
checks before reading. A fixed wiped 16-KiB-plus-sentinel buffer detects overflow
without plaintext reallocation. Trusted platform symlinks support rotation; no
undocumented 0400/0600 ownership or permission rule is imposed on platform files.
The current file is read before each request and again after its body. Changes
in contents or file device/inode/size/timestamps, disappearance or invalid input
discard the response. File metadata is also checked before/after each read.
File credentials are Unix-only; unsupported platforms compile but fail closed
when file credentials are requested. There is no token cache, JWT identity
assertion, AppSecret fallback or background refresh. Enforce workload-PSM
TCC authorization.

Each `read_snapshot()` reads `/tcc/v2/meta/{namespace}` and
`/tcc/v2/data/{namespace}/{space}` afresh, sharing one three-second deadline over
credentials, discovery, HTTP and validation. Synchronous parsing cannot be
preempted; late completion is rejected. Earlier success is never returned after
a new error, including repeated versions. Responses must be 2xx and contain
object-only `{ "data": { "key": "...", "value": "..." } }`; 401/403, redirects,
other statuses, truncation, malformed JSON and duplicate recognized fields fail
closed. Resource keys must match exactly, with at most one leading slash
normalization. Metadata/content versions must match; the selected space occurs
exactly once. Versions are nonempty, at most 256 bytes, and contain no controls.
Nonempty gray data/code/IP lists are rejected; absent, null and empty gray
configuration are allowed.

Before parsing, each HTTP body is streamed into a wiped buffer allocated once
with a 16 MiB capacity. Both Content-Length and chunked bodies enforce that bound.
This wire limit includes JSON escaping and can reject highly escaped documents
even when their decoded size would fit. Outer and decoded JSON use the core
64-level structural depth check before typed Serde parsing. Schema structures
must be objects; duplicate recognized fields and configuration-map keys are
rejected. Unknown additive fields remain compatible within the same bounds.
Decoded metadata is limited to 2 MiB, content to 8 MiB, spaces/configuration
entries to 128, each selected payload to 256 KiB and selected payloads to 1 MiB.

`Snapshot::required(PayloadKey)` borrows only `ask_auth_runtime`,
`ask_auth_secrets`, `ask_auth_admission` or `ask_auth_cleanup`. Missing payloads
remain explicit errors. `version()` exposes the validated version;
`read_completed_at()` supplies monotonic completion time for host freshness.
Snapshots and credential containers have no Debug/Display output. Owned document
and configuration values are wiped on drop, including discarded unknown map
values. Reqwest header/chunk allocations, parser scratch buffers, original process
environment and downstream parser copies cannot all be wiped by this adapter.
Errors retain only fixed categories, never upstream bodies, errors, paths or tokens.

Wire rules were independently checked against published BConfig V1 SDK source;
no SDK implementation is copied. The
[TCC platform documentation](https://cloud.bytedance.net/docs/tcc/docs/646447998e0ca902369dc10e/65f1815d6e6bae030cef8e48)
provides deployment context. Local tests do not establish real BOE ACL acceptance.
