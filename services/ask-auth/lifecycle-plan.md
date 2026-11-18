# Stage 4 backend lifecycle plan

Scope: extend only the explicitly injected `LoginService` / enabled router. The
standalone process and default router remain unavailable. No migrations, schema,
indexes, environment secrets, real provider transport or production wiring.

## 4a interfaces and implementation

1. Extend trusted OAuth adapter with a required typed refresh operation returning
   effective verified credentials or explicit proven-no-consumption / ambiguous
   outcomes. The service never guesses that an omitted RT remains usable.
2. Add closed Store A->exact-R snapshots with DB time and proof-first shape checks,
   exact locked release confirmation, and bounded atomic authorization termination
   plus at most one pending child settlement. No raw pool/transaction escape.
3. Enable refresh: authenticate exact retained R with its immutable request HMAC
   version before interpreting state; otherwise authenticate current A broker/RT,
   policy/scope/generation, atomically reserve and call once outside a transaction.
   Capture original fences, bound provider time to 10 seconds and execution to
   45 seconds. Publish success or sanitized failure under the original fence.
4. Refresh caches use `Binding::refresh(auth_id, request_id, input_generation)`;
   validate complete schema, metadata, digests and current output generation.
   Final release requires fresh policy and a locked exact A/R snapshot.
5. Enable status with selected-PK current-broker authentication, current policy,
   natural expiry enforcement and locked exact-row privacy confirmation. Enable
   logout independent of policy availability/removal, atomically end A and settle
   pending R. Retained already-ended rows keep their terminal reason/state while
   logout acknowledges `revoked`; no ended row becomes usable.
6. TDD real-MySQL router tests cover full flow, exact retries/concurrency, proof
   ordering, cancellation/late results, policy changes, output validation, key
   versions, expiry and logout; validate actual responses against frozen schemas.
   Run checks/format/clippy and a complete seven-category self-review, fixing all
   Critical/Major findings with traceable Review Fix comments.

## 4b cleanup handoff (separate work)

Implement the existing cleanup.md design with closed bounded Store scans and
mutations: expire short login phases, clear retained login groups independently
of authorization, enforce auth expiry, settle abandoned original refresh leases
without retrying upstream, delete expired completed/failed/unknown R before ended
parents. Lock A before exact R and preserve original operation fences, DB clocks,
a dedicated READ COMMITTED cleanup pool, whole-transaction deadlines of
min(1 second, remaining pass budget), and bounded batches. API Store operations
retain their separate three-second timeout. Test cancellation recovery, races,
corruption, retention and repeated runs. Do not change A20/R9 or schema/indexes.

## 4a self-review record

Coder and Reviewer passes are separate. Reviewer re-read the changed paths from
selected-PK authentication through locked release/termination, without assuming
implementation correctness.

| Category | Evidence checked |
| --- | --- |
| Correctness | Output-generation replay comparison, checked increment, immutable identity, scope narrowing, known/unknown RT deadline bounds, terminal preservation and exact original operation fences. |
| Security | Constant-time row-version proofs before state, canonical length-prefixed full refresh HMAC, independent refresh AEAD binding, required nullable cache fields, complete metadata/digest validation, no broker lookup and no secret Debug/provider text. |
| Performance | Bounded attempts and DB calls, no transactions over external I/O, one upstream call, pending-child query capped at two, no unbounded historical cache updates on logout. |
| Maintainability | Closed Store interfaces and required typed provider outcomes; functions remain at most fifty body lines, at most five parameters and no deep control nesting. |
| Robustness | DB three-second, policy three-second, provider ten-second, body five-second/32 KiB and total handler twenty-second limits; cancellation retains pending evidence and no takeover; transaction rollback owns resources. |
| Testability | Explicit adapters, isolated real MySQL router tests, actual response JSON Schema validation, barriers for races, corruption and immutable-fence tests. |
| Observability | Existing route-template/method/generated-trace/status/duration log only; no raw request/provider text or credentials; credential comparisons avoid assertion formatting. |

Round 1 found four Major issues and one Minor issue, addressed by Review Fixes
#33 (disabled policy status), #34 (active-worker lease check after both locks),
#35 (bounded recovery reclassification), #36 (policy expiry under reservation
lock) and #37 (credential-safe test assertion). The fresh disabled-policy test
was observed RED (503 versus 200), then GREEN with the 44-test router suite.
Round 2 identified the reservation lease clock: it must begin at locked DB now
and return the canonical persisted R to the service fence. Root observed its
real-MySQL regression RED after a delayed policy load: the remaining lease was
not greater than 44.5 seconds. Review Fix #38 now sets the deadline inside the
reservation transaction and returns that persisted R to original-fence capture.
Round 3 rechecked all seven categories after this fix: no remaining Critical,
Major, Minor or Suggestion findings. The root performs fresh MySQL GREEN and
socket verification before accepting the handoff; no unobserved database result
is claimed here.

Observed validation before the final lease change: 44 isolated MySQL router
tests passed, including schema checks; 48 non-socket ordinary tests passed;
`cargo check`, `cargo clippy --all-targets -- -D warnings`, formatting and diff
checks passed. Four ordinary startup socket tests need the same local-network
permission as the MySQL tests; their sandbox-only EPERM is environmental.

Outside 4a: stage 4b cleanup, real provider/TCC adapters, deployment evidence,
distributed limits and CLI wiring. These remain explicit design stages.

After Review Fix #38, fresh `cargo check --locked --offline`,
`cargo clippy --locked --offline --all-targets -- -D warnings`, `cargo fmt --check`
and `git diff --check` passed. The selected non-socket suite again passed all
48 tests (7 library, 19 HTTP contract, 1 logging, 4 login validation, 17 storage).
Root owns the remaining real-MySQL lease GREEN/full database and socket reruns.


## Quality follow-up: Review Fix #39

Independent quality review found one Minor: status computed the policy deadline
before rejecting an invalid future timestamp. A deterministic trusted test source
now returns whole-millisecond `NaiveDateTime::MAX` after a valid login claim.
Root observed genuine RED: one test ran and panicked at the deadline addition.
Review Fix #39 moves the existing `is_current` guard before that arithmetic;
there is no new abstraction or changed valid-policy behavior. The seven review
categories were rechecked for this minimal change: no remaining findings.
Root performs the focused/full HTTP GREEN, clippy/build and quality recheck.
