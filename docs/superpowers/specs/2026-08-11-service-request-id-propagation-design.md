# Service Request ID Propagation Design

## 1. Status and scope

- Status: **Frozen**
- Frozen date: 2026-08-11
- Supported surfaces: `tos-cli`, `ve-tos-cli`, and `ve-adrive-cli`
- Scope: successful responses, failed responses, pagination, retries, recursive
  operations, and batch operations

This design separates the service request identifier from the CLI-generated
fallback identifier. Whenever a command observes a usable request ID from its
primary service, the top-level Envelope `request_id` must use that service
value. A CLI-generated ULID is used only when no service request ID exists.

## 2. Public output contract

### 2.1 Single service request

For a command whose result is produced by one service response:

- the top-level `request_id` is read from the service response;
- the command does not add `data.service_request_ids` merely to repeat that
  single value;
- when the service omits or returns an unusable request ID, the command remains
  successful and the top-level field falls back to a CLI-generated ULID.

The recognized resource-service headers are:

| Surface | Header priority |
|---|---|
| `tos-cli`, `ve-tos-cli` | `x-tos-request-id` |
| `ve-adrive-cli` | `x-ids-request-id`, then `x-request-id` |

OAuth endpoints are the primary service only for `ve-adrive auth` commands.
An OAuth refresh performed before a resource request must not replace the
resource API request ID in a successful resource-command Envelope.

### 2.2 Multiple service requests

Pagination, retries, recursive traversal, multipart work, and batch operations
may observe more than one resource-service request ID. Their output follows
this contract:

- top-level `request_id` is the last successfully completed primary-service
  request ID;
- for concurrent work, "last" means the last successful response recorded by
  the command, not submission order;
- `data.service_request_ids` contains the observed non-empty service request
  IDs in completion order;
- aggregate commands covered by this contract return an object-shaped `data`
  payload so the trace fields have one stable location;
- the list is bounded to 1024 entries, matching the existing `du` diagnostic
  limit;
- `data.service_request_ids_omitted` reports how many additional IDs were not
  retained;
- duplicate IDs are retained because each position represents an observed
  response and the CLI must not assume service-side uniqueness.

Retry responses that contain a service request ID are recorded even when a
later attempt succeeds. Connection failures that never receive an HTTP
response cannot contribute an ID.

### 2.3 Failed commands

When a service response causes the command to fail, the top-level `request_id`
uses the terminal failed response's service request ID. If the failure occurs
without a service response, the existing CLI-generated fallback remains.

For multi-request commands that fail after partial progress, the failed
Envelope keeps the terminal service ID at the top level. Existing report or
diagnostic payloads may expose the bounded list of earlier IDs when they
already support partial-result metadata; this change does not introduce a new
partial-result schema for every error.

### 2.4 Commands without a primary service request

Pure local commands, validation failures, `--describe`, `--dry-run`, local
configuration operations, and commands whose network call receives no service
request ID keep a CLI-generated ULID. An explicit `null` request ID remains
valid for aggregate commands that intentionally declare that no single
request ID exists.

## 3. Internal data flow

Service request IDs are propagated explicitly from the HTTP response boundary
to the command Envelope. The process-wide `TOS_LAST_REQUEST_ID` environment
variable remains a compatibility fallback for legacy paths and error parsing;
it is not an authoritative source for successful command output because it can
be stale or overwritten by concurrent work.

The implementation uses a shared bounded request-trace abstraction for
multi-request commands. It provides:

- the total number of received HTTP responses, including responses without a
  usable request ID;
- validation and recording of non-empty service request IDs;
- the last successfully recorded ID;
- terminal-attempt metadata so an error caused by a response keeps that
  response's ID, while a terminal transport failure keeps the CLI fallback;
- at most 1024 retained IDs;
- an omitted counter;
- a snapshot suitable for attaching to JSON payloads and Envelopes.

HTTP helpers and typed SDK-like response structures must preserve response
metadata instead of returning only decoded business data. Single-request
handlers attach the response ID with `Envelope::with_request_id`. Aggregate
handlers merge request-trace snapshots and attach the last service ID to the
top level.

The output layer still guarantees that every ordinary Envelope has a non-empty
fallback ID. It must not overwrite an explicitly supplied service ID or an
explicit `null`.

## 4. Audit scope

The implementation audit covers all three command surfaces and classifies each
Envelope-producing path as one of:

1. pure local, with a generated CLI ID;
2. one service response, with an explicitly propagated service ID;
3. multiple service responses, with a bounded trace and last service ID;
4. no meaningful single service response, with explicit `null` where already
   required by the command contract.

The audit includes:

- high-level commands such as `ls`, `cp`, `mv`, `sync`, `du`, `find`, and
  recursive `rm`;
- low-level bucket, object, multipart, and ADrive resource commands;
- download and streaming-body completion paths;
- retry loops, including retryable HTTP responses;
- ADrive OAuth command responses and resource responses;
- success and error Envelope construction.

Existing nested `data.request_id` fields are retained during this change to
avoid breaking consumers. They no longer substitute for the correct top-level
field. Removing redundant nested fields requires a separate compatibility
decision.

## 5. Safety and compatibility

- Command behavior, authentication, signing, endpoint selection, retry policy,
  and payload data remain unchanged.
- No request ID is used as a credential, authorization decision, retry key, or
  idempotency token.
- Request IDs are trimmed and accepted only when they contain at most 256
  Unicode scalar values and no control characters. Empty or rejected service
  values are treated as absent before they enter structured output.
- Logs and errors must not interpolate response headers other than the
  sanitized request ID.
- The new bounded list prevents unbounded memory and output growth during large
  recursive or batch operations.
- Existing clients that read only top-level `request_id` gain the service value
  without a schema change. Existing clients that read `data.request_id` remain
  compatible.

## 6. Verification requirements

Implementation is complete only when automated tests demonstrate:

1. one TOS response places `x-tos-request-id` at the top level;
2. one ADrive response honors `x-ids-request-id` before `x-request-id`;
3. `tos ls tos://...` no longer discards parsed response metadata;
4. every `ve-adrive ls` scope places its service ID at the top level;
5. multi-page listing uses the last successful page ID and emits all observed
   IDs in `data.service_request_ids`;
6. retryable responses contribute IDs and a later success becomes the top-level
   ID;
7. the 1024-entry limit and omitted counter are enforced;
8. failed service responses preserve their terminal service request ID;
9. local commands and responses without a request ID still receive a ULID;
10. explicit `null` remains unchanged;
11. OAuth refresh IDs do not replace successful ADrive resource IDs;
12. existing JSON, table, CSV, YAML, XML, and Markdown rendering tests remain
    compatible.
