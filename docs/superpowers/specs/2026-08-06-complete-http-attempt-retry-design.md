# Complete HTTP Attempt Retry Design

## Scope

This change applies the same retry contract to fixed HighLevel and LowLevel
storage operations implemented by the TOS and ADrive clients. Raw calls whose
semantics are unknown remain conservative. Existing command arguments, output
envelopes, status-code mappings, overwrite rules, authentication refresh, and
checkpoint formats remain unchanged.

The service contract guarantees that HTTP 429 and every HTTP 5xx response from
these storage APIs means the operation was not accepted or applied. Those
status responses are therefore safe to retry regardless of the HTTP method or
the operation's idempotency. HTTP 408 is treated as an ambiguous timeout and is
retryable only for idempotent operations. These rules are specific to the TOS
and ADrive storage APIs covered here; they must not be generalized to an
unknown endpoint.

## Attempt boundary

One HTTP attempt owns the complete request lifecycle:

1. construct and sign the request;
2. send all replayable request headers and body;
3. receive and validate the response status and headers;
4. when a response body exists, consume, parse, and validate it completely;
5. return success only after every required step completes.

Receiving response headers alone is not success for a body-bearing response.
A mid-body timeout, disconnect, premature EOF, decode error, length mismatch,
or checksum mismatch fails the attempt.

## Retry eligibility

Retry eligibility is decided from both the failure event and the operation's
semantics:

| Failure event | Idempotent operation | Non-idempotent operation |
| --- | --- | --- |
| Definite connection failure before delivery | Retry | Retry |
| HTTP 408 | Retry | Do not retry |
| HTTP 429 | Retry | Retry |
| HTTP 5xx | Retry | Retry |
| Client-side timeout after delivery may have started | Retry | Do not retry |
| Response body stream/decode/length/checksum failure | Retry | Do not retry |
| Other HTTP 4xx | Do not retry | Do not retry |
| Local validation or local I/O failure | Do not retry | Do not retry |

GET, HEAD, PUT, DELETE, and OPTIONS are idempotent by default. POST and PATCH
require an internal explicit idempotent declaration for fixed operations whose
semantics are known. For example, ADrive file search is a read-only POST and is
explicitly idempotent. Create, rename, copy, and multipart completion remain
non-idempotent for ambiguous client-side failures, but still retry the service's
explicit 429 and 5xx rejection responses.

An eligible retry additionally requires that its request body can be reproduced
and its response sink can be reset:

- In-memory JSON and byte bodies are replayable.
- Local files and file ranges are replayable by reopening them per attempt.
- Multipart upload/copy retries reuse the same upload ID and part number.
- Standard downloads restart their temporary file from byte zero.
- Range downloads retry only the failed range and retain completed ranges.
- Stdin is not replayable and is never retried after transmission starts.
- Stdout cannot be rolled back and is never retried after output starts.

## Retry decisions

Apply the decision table above to the complete attempt, including response-body
consumption. An idempotent body-bearing request is retried from request headers
through the complete response body when a body stream, decode, length, or
checksum failure occurs. A partial download resets only the current attempt's
sink before retrying; already completed multipart ranges remain intact.

A non-idempotent operation retries only failures that prove the operation was
not accepted: a definite pre-delivery connection failure or an HTTP 429 or 5xx
response. It does not retry HTTP 408, an ambiguous client-side timeout,
send/body failure after delivery may have begun, or response-body failure.

Do not retry other HTTP 4xx, local I/O errors, validation errors,
authentication/authorization failures, or non-replayable operations.

OAuth 401 refresh remains a separate one-time authentication recovery step and
does not expand the operation's retry eligibility.

## Retry timing and limits

The default `max_retry_count` remains 3, so a command makes at most four HTTP
attempts unless the user explicitly configures another value.

For HTTP 429 and HTTP 5xx, honor a valid `Retry-After` response header before
falling back to exponential backoff. Support both integer delta-seconds and an
HTTP date. A future HTTP date uses its remaining duration; a past date permits
an immediate retry. An invalid value falls back to exponential backoff. Clamp
every `Retry-After` sleep to 600 seconds.

Retryable HTTP 408 responses and retryable transport/body failures use
exponential backoff starting at 200 milliseconds and doubling per retry, capped
at 6.4 seconds per sleep. Backoff waits do not replace the attempt-count limit.
There is no total deadline for a healthy transfer.

## Timeout semantics

`requesttimeout` defaults to 300 seconds and is a read-idle timeout for streamed
responses, not a total transfer deadline. Each successful body read resets the
idle timer, allowing healthy large transfers to run longer than 300 seconds.
`connecttimeout` continues to bound connection establishment.

## Compatibility

Explicit existing `requesttimeout` values override the new default. Public CLI
arguments and output schemas do not change. Retry attempt details remain in
verbose diagnostics and must not expose credentials, authorization headers, or
copy signatures.
