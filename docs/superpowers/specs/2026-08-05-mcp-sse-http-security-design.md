# MCP SSE HTTP Security Design

## Goal

Harden the existing local HTTP/SSE MCP transport against unauthenticated
browser access and DNS rebinding without changing stdio behavior or exposing a
remote MCP service. The SSE transport remains a loopback-only integration for
an Agent running on the same host and network namespace as the CLI.

This design applies to every CLI surface that reuses
`tos_core::mcp::TosMcpServer`: `ve-tos`, ByteTOS `tos`, and `ve-adrive`.

## Deployment and trust model

The supported SSE topology is:

```text
one host / one network namespace
├── local MCP Agent
└── storage CLI listening on 127.0.0.1:<port>
```

Binding to `127.0.0.1` prevents direct connections from other machines, but it
does not distinguish a trusted local Agent from a browser running on the same
host. The HTTP layer therefore authenticates every MCP request and validates
the logical HTTP destination before rmcp creates or uses a session.

Remote Agents, containers in a different network namespace, public listeners,
TLS termination, reverse proxies, OAuth, and `0.0.0.0` binding are outside this
design. Those deployments require a separate remote-MCP security contract.

## Bearer token lifecycle

Every real SSE startup generates a fresh 32-byte token using the operating
system cryptographic random source and encodes it as unpadded base64url. Dry
run, describe, and stdio do not generate a token.

After the loopback listener binds successfully, the CLI prints the complete
connection URL and Bearer token exactly once to stderr. Stderr is used because
it is the CLI diagnostic/startup stream and does not corrupt structured stdout.
The output clearly states that the token must be supplied as:

```http
Authorization: Bearer <token>
```

The token is never placed in an endpoint URL, query parameter, MCP payload,
success envelope, trace field, or subsequent log message. The authentication
state stores only a fixed-size SHA-256 digest of the token. Candidate digests
are compared in constant time. Restarting the server invalidates the token and
all sessions from the previous process.

The initial implementation intentionally has no caller-supplied token flag,
environment variable, or persistent token file. A user who starts SSE manually
copies the one-time token into the local Agent's Authorization-header
configuration. Automated token exchange is a separate integration problem and
must not be solved by putting credentials in URLs.

## HTTP request policy

The security layer wraps the complete rmcp SSE router, so current and future
routes are protected by default rather than relying on each handler to remember
authentication. Validation happens before session lookup, session creation,
JSON parsing, or tool dispatch, in this order:

1. Validate `Host`.
2. Validate `Origin` when present.
3. Validate `Authorization: Bearer <token>`.
4. Remove the Authorization header before forwarding the request to rmcp.

Removing the header after successful validation is required because rmcp 0.8.5
logs and attaches HTTP request parts to MCP messages. Downstream code receives a
non-secret authenticated marker instead of the credential.

### Host validation

For a listener on `127.0.0.1:<port>`, the accepted HTTP authorities are exactly:

- `127.0.0.1:<port>`
- `localhost:<port>` with ASCII case-insensitive hostname comparison

The configured non-default port is required. Missing, malformed, userinfo,
non-loopback, suffix-matched, wildcard, and DNS-derived aliases such as
`127.0.0.1.nip.io:<port>` are rejected. The current listener is IPv4-only, so
`[::1]` is not advertised or accepted until the bind contract supports IPv6.

This check stops DNS rebinding because the browser connects to the loopback IP
while retaining the attacker's domain in the HTTP `Host` header. Host is a
destination integrity check, not client authentication; the Bearer token
remains mandatory.

### Origin validation

Native MCP clients normally omit `Origin`. A request without `Origin` may
continue only if its Host and Bearer token are valid.

When `Origin` is present, it must be an exact HTTP origin using `127.0.0.1` or
`localhost` and the listener port. External domains, opaque `null` origins,
wildcards, suffix matches, HTTPS origins for the plain-HTTP listener, malformed
values, and origins with a different port are rejected.

Origin is an additional browser boundary, not authentication. It prevents a
website from directly targeting an otherwise valid loopback Host and provides
defense in depth if browser or CORS behavior changes. It does not block native
Agents because their missing-Origin requests remain valid after Bearer
authentication.

### Bearer validation

Both `GET /sse` and `POST /message` require the generated token, as do any
future routes added to the protected router. The authentication scheme is
matched ASCII case-insensitively; the token is matched case-sensitively. Missing,
duplicated, malformed, empty, and incorrect credentials are rejected.

The process has one local authentication principal. Requiring the same Bearer
credential on every request implicitly binds all rmcp sessions to that
principal: possession of a session ID alone never authorizes `/message`.

## HTTP errors and observability

The security layer returns errors without MCP dispatch:

- missing or malformed Host: `400 Bad Request`;
- syntactically valid but disallowed Host or Origin: `403 Forbidden`;
- missing, malformed, or incorrect Bearer token: `401 Unauthorized` with
  `WWW-Authenticate: Bearer`.

Responses and logs never echo the supplied credential. Rejection logs may
include the reason category, method, path, Host, and Origin, but not
Authorization or MCP request bodies. Authorization is stripped before rmcp's
existing request-parts logging can observe it.

## Public CLI and documentation behavior

The `serve --help`, dry-run/describe startup plan, README, and all three CLI
surfaces describe the same contract:

- stdio remains the default and has no TCP listener;
- SSE remains loopback-only;
- real SSE startup generates a temporary Bearer token and prints it once to
  stderr;
- every SSE HTTP request must carry the token in the Authorization header;
- the token is never accepted in a URL.

No token appears in dry-run or describe output because those modes do not start
a listener or create a credential.

## Compatibility and non-goals

- MCP tool schemas, `execute`/`force` semantics, TOS credentials, and tool
  dispatch are unchanged.
- stdio initialization and tool calls are unchanged.
- Existing SSE clients must add Authorization-header support. A client that can
  only use browser-native `EventSource` without custom headers is intentionally
  incompatible; query-string credentials are not an acceptable fallback.
- There is no CORS wildcard and no browser-facing public API in local mode.
- The legacy rmcp transport is hardened in place; migration to Streamable HTTP
  remains separate work.

## Verification

Tests cover at least:

1. Generated tokens decode to 32 random bytes, use base64url without padding,
   and independent generations differ.
2. `GET /sse` and `POST /message` reject missing and incorrect Bearer tokens
   with `401` and `WWW-Authenticate: Bearer`.
3. Both routes accept the correct token with an exact loopback Host.
4. A valid token with `Host: evil.example:<port>` or a DNS alias is rejected
   with `403` before the route handler runs.
5. A valid Host and token with an external, `null`, malformed, wrong-scheme, or
   wrong-port Origin is rejected; missing Origin and exact loopback Origin pass.
6. The downstream route cannot observe the Authorization header after successful
   authentication.
7. A session ID without the Bearer token cannot submit `/message` requests.
8. SSE startup documentation and dry-run metadata describe authentication but
   contain no generated token.
9. Existing stdio MCP initialization, tools/list, and approved tool execution
   tests continue to pass.
