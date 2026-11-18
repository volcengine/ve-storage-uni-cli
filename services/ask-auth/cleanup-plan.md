# Stage 4b cleanup implementation plan

The approved contract is cleanup.md v1.15. A20/R9 and DDL remain frozen.

1. Add a separate validated CleanupDatabase connection capability: writer/TLS/UTC
   baseline, READ COMMITTED, one-second lock wait and ROW binlog validation.
2. Add closed CleanupStore discovery and processing APIs in Store's private
   submodule. Discover metadata only with indexed six-queue keysets and DB cutoff;
   lock A before exact R, revalidate original fences and authenticated full shapes,
   and reuse existing transition/update/delete validators within one transaction.
3. Add injected fail-closed watch controls, validated limits, monotonic affected-row
   rate reservations, independent cryptographic startup/tick jitter, whole-pass and
   whole-transaction deadlines, bounded rediscovery retry and cooperative shutdown.
4. Report sanitized durations, counters, queue age and alert thresholds. Keep
   main/default router disabled and no provider/TCC/environment wiring.
5. TDD against the isolated ask_auth_stage4_cleanup_test schema: expired pending,
   exact fences, lock contention, cache-only cleanup, abandoned refresh, ended
   child batches, malformed persistence and index plans; deterministic scheduling
   and limit unit tests. Never print connection strings or credentials.
6. Separate seven-category Reviewer pass, fix Critical/Major issues with Review
   Fix #40 onward, then formatting, clippy and fresh MySQL verification by root.

No production deployment, actual TCC adapter/alerts transport, or capacity claim
is part of this stage. Defaults require deployment load testing.

## Review record (in progress)

Coder implementation and Reviewer reread are separate passes. Initial real-MySQL
RED observed by root: expired pending discovery returned zero references instead
of one. Root then observed GREEN for the first seven cases, followed by fifteen
cleanup integration cases and the actual EXPLAIN/pool-isolation library case.
No database result is inferred from static SQL or fake time.

First Reviewer pass identified completion-time rate accounting, cancelled-pass
success accounting, connection reuse after uncertain commit, and finite cursor
cycles under continuous expiry arrivals. These are fixed with Review Fixes
#42–#45; normal skips still take the acknowledged read-only commit path and
reuse their connection. Review Fix #40 explicitly guards the original expired
refresh lease after both locks; #41 reserves the maximum two business writes
before acquiring a transaction. The deterministic rate test was observed RED
with start-time reservations and GREEN after completion-time anchoring.

Pending final checks: MySQL 1205/HY000 retry classification (a focused runtime
regression is written), safe alert severity, expanded boundary acceptance tests,
and fresh full formatting/clippy/regression verification by root.

## Final self-review handoff

Second Reviewer pass reread all seven categories after fixes:

| Category | Evidence |
| --- | --- |
| Correctness | Original A phase/state/version/deadlines and broker/generation plus original R request/version/input/lease; fresh locked DB time; ready auth-only expiry; eight-field cache-only changes; ended reason preservation; at most one child per parent transaction and locked final NOT EXISTS. |
| Security | Dedicated writer/TLS policy; authenticated full current/target shapes and AEAD/key validation; no provider, proof release, raw pool/SQL escape, environment fallback, identity labels or secret Debug. SQL structure is static enum-selected QueryBuilder fragments; all values are binds. |
| Performance | Existing deadline indexes and PK ties, metadata-only LIMIT scans, six rotated queues, finite sixteen-batch cursor reset, two-row conservative completion-anchored rolling reservations, no unbounded children/count/OFFSET scan. Actual MySQL index plans were inspected by root. |
| Maintainability | Closed CleanupDatabase/Store/Worker capabilities; existing endpoint validators reused privately; functions <=50 body lines, <=5 parameters and <=3 control nesting; no schema changes. |
| Robustness | Whole pass <=5s and whole transaction <=1s; cancelled/unknown commits destroy the socket; only acknowledged commits reuse connections. Dynamic pause/closed source/shutdown stop new transactions, no transaction spans sleep/control reads/provider calls. Bounded jittered rediscovery retries and outage backoff. |
| Testability | Isolated exact-schema guard, real MySQL A/R fixtures with authentic AEAD, contention/cancellation triggers removed before assertions, actual EXPLAIN and separate pool settings. Fake time is used only for in-process reporting, never as DB-time evidence. |
| Observability | Controlled UUIDv7 pass trace, enum queue/alert/error, measured durations/rows/skips/retries/failures and oldest head ages. Warn/error alert severity; SQLx/dependency secret sentinels remain blocked by the exact target allowlist. |

The focused MySQL 1205/HY000 regression was observed RED by root (zero retries
instead of two). Review Fix #46 classifies typed MySQL 1205/1213 without keeping
server text; #47 counts failed retry attempts as failures. Review Fix #48 emits
integrity/page intents at error level and backlog warning at warn level; its
logging assertion was observed RED then GREEN. No Critical/Major findings remain
in the self-review. Root owns independent Spec then Quality review and fresh
full database/socket verification before committing.

Observed local checks: 55 ordinary tests passed (11 library, 2 cleanup control,
19 HTTP contract, 2 logging, 4 login validation, 17 storage), with the one explicit
MySQL library case ignored locally. Fresh cargo check --tests, all-target clippy
with warnings denied, package formatting and diff checks passed. Root previously
observed fifteen MySQL cleanup cases and the actual EXPLAIN/pool case GREEN.
The final suite contains twenty-two cleanup MySQL cases; its newly added retry,
claim/callback/recreation, terminal retention, rotation/fairness and whole-pass
cases require root's final database run. No unobserved GREEN is claimed.

Remaining release work, outside this injected implementation: real Doubao/TCC
adapters (no Rust TCC SDK exists here), production start/shutdown wiring, TCE
metric/alert delivery and pressure feedback, distributed/replica operational
limits, CLI wiring, and sustained churn/load/space/purge/replication capacity
verification. No fault-injected network lost-COMMIT acknowledgement or real
engine-selected deadlock is claimed: current tests cover cancellation/rollback,
concurrent SKIP LOCKED and typed transient-server-error retries.

## Independent review follow-up: #49 and #50

Independent Spec found a fixture safety issue: its migration/admin connection
was checked against the disposable schema but the independent runtime URL had
not been prechecked. Review Fix #49 adds pure parsing of both targets before any
connection, trigger or DELETE, requiring the exact cleanup schema, numeric
loopback and no socket. The existing server-side admin DATABASE() check remains.
The non-network wrong-runtime-schema regression was observed RED, then both
pure guard tests passed, covering wrong schema, remote/hostname host, socket and
malformed URL without contacting any rejected target. Production Store APIs are
unchanged by this test-only fix.

Root's final run observed a real #50 regression: the whole-pass timeout test
occasionally returned error=None after a failed transaction. Diagnosis traced
this to the retry branch recording counters without retaining its error;
rate/budget/control exit could then return Ok. Review Fix #50 keeps unresolved
retry errors by queue and opaque original cursor. Only a fresh rediscovery of
that same candidate followed by a successfully validated, non-lock-skip
transaction clears its error. Unrelated candidates, other queues and empty
scans do not clear it. Normal rate exhaustion without errors remains a normal
successful pass. Outer cancellation still reports Timeout, resets discovery
hints and conservatively retains possible writes.

The runtime regression now provides 300ms preflight/pass headroom and a 100ms
transaction bound, with a two-row rate reservation. A fixed test trigger acquires
an advisory marker before its delay; a separate read confirms that guarded DML
actually started. The test checks safe transaction/failure/retry/row counters and
retains the Timeout through the subsequent rate-budget exit, verifies rollback
and confirms the reservation survives the next pass. Root owns repeated targeted
MySQL GREEN and final suite reruns. Fresh local check --tests, clippy, format and
diff checks passed after #49/#50. No pending Critical/Major self-review findings
remain; the independent reviewers determine final acceptance.
