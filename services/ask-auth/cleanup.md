# Authorization-session cleanup design

Design v1.15: one public auth_id, with PK-based authorization checks.

The runtime worker is available through explicit trusted library injection; the
standalone process does not start it. MySQL is the authority for A
(`auth_sessions`) and R (`auth_refresh_requests`). Cleanup is an in-process
worker in every TCE replica, with no separate service, Redis, leader, scheduler
table or MySQL EVENT. Start after the writer pool is ready; on shutdown stop
acquiring work and commit or roll back the current short transaction.

## Budgets and discovery

Initial TCC settings are operational defaults, not capacity evidence:

| Setting | Initial value |
| --- | --- |
| Interval | 10 seconds plus independent 0–3 seconds jitter, including startup; no overlapping passes or missed-tick replay |
| Candidate batch | 100 references, hard maximum 500; do not scan ciphertext |
| Pass budget | 5 seconds, potentially multiple small batches |
| Transaction budget | min(1 second, remaining pass budget), covering all statements and commit; on expiry roll back or discard the connection |
| Rate budget | At most 100 updated/deleted business rows per second per replica, including R; aggregate limits grow with replica count |

Use a separate writer-only UTC/READ COMMITTED cleanup pool with short statement
and lock timeouts. Do not alter API connection isolation. SKIP LOCKED workloads
use row-based replication. Never hold a transaction across HTTP, sleep or ticks;
the existing 3-second DB operation limit does not replace the stricter whole
transaction deadline. Resolve a lost commit acknowledgement by rediscovery and
locked recheck, not replaying old DML.

| Fairly rotated queue | Indexed access | Deadline interpretation |
| --- | --- | --- |
| A login temporary phase | `ix_session_login_expiry(login_expires_at)` then auth_id | pending/exchanging/ready original login deadline; active/refreshing login replay deadline; preauthorization terminal cleanup time |
| A authorization expiry | `ix_session_auth_expiry(auth_expires_at)` then auth_id | Authorization/broker upper bound; now for ended authorization |
| R, one fixed status per scan | `ix_refresh_expiry(status, expires_at)` then auth_id/request_id | Execution lease for pending, replay deadline for completed/failed, now for unknown |

Discover references using nonlocking SELECTs on the writer, stable keyset order
by deadline and PK, with a DB-time pass cutoff. NULL phase deadlines are not due
candidates. No OFFSET, long snapshot or ciphertext reads. Cursors are in-process
hints; advance over skipped rows and reset after a bounded cycle. Small batches
and round-robin queues prevent hot-row/queue starvation. Oldest-overdue probes
start at the index head using LIMIT 1, not a full COUNT every tick.

## Locks and original-operation identity

1. Lock A by PK with `FOR UPDATE SKIP LOCKED`. An empty result means skip, whether
   absent or locked; it never permits independent R deletion.
2. For R work lock exact R next. For parent cleanup use A->R and stable R PK
   order. Candidate references are hints; under locks validate current phase,
   shape, deadline, generation and operation identity using fresh DB time.
3. Preauthorization candidates carry original oauth_state_hash, HMAC version
   and login deadline; a deleted/recreated ID must not inherit an old decision.
   Authorized candidates carry broker digest, HMAC version and generation as
   needed; a refresh/key migration invalidates stale candidate identity. Re-read
   rather than forcing progress from a stale scan.
4. auth_id is only the resource locator. API status/logout use this same A PK
   lookup followed by its stored-version broker proof; no broker index or key-version
   search supplies authority to cleanup. Retain all original-operation fences.
5. Apply parameterized bounded DML with affected-row checks through the same
   current/target-row Store validator as endpoints. Roll back on errors or
   mismatches. An R whose parent is genuinely missing is an integrity incident,
   never an excuse to auto-delete or adopt it. Never delete A with remaining R.

<!-- [Review Fix #16] An expired login cache is not an expired authorization. -->
The login temporary group is the eight columns listed in README. Clearing it
sets all eight to NULL in the same UPDATE. Its digest/ciphertext pairs must not
be partially cleared, and its expiry must not be confused with auth_expires_at.

## State actions

| Locked current phase | Action |
| --- | --- |
| A.pending/exchanging, original login deadline elapsed | Match original state/version/deadline, clear OAuth context, end as expired and delete A; no R or authorized identity exists. No code replay or takeover. A preauthorization terminal row kept between transactions retains its state hash/version fence until deletion. |
| A.denied/error/expired without authorization | Context/result/claim ID already cleared; login expiry truncated to now. Recheck original fence and delete; do not erase that fence in a retained row. |
| A.ready, login or authorization deadline elapsed | End the never-claimed session as expired, truncate auth expiry, clear the entire login group and delete A after locked NOT EXISTS R. R cannot legitimately exist for ready; an unexpected child is an integrity error, not permission to drop it. |
| A.active/refreshing, login replay deadline elapsed | Clear the whole login temporary group only; retain A status, identity, broker/RT digests, HMAC version, generation and auth deadline. A claim replay or newer phase deadline invalidates a stale candidate. If authorization itself expired, use the authorization-expiry path instead. |
| R.pending, original execution deadline elapsed | Recheck A.refreshing, R.pending, A generation=input_generation and exact original R request digest/key version/lease. Atomically set R.unknown/error/expiry=now and A.reauth_required/auth expiry=now; clear the login group. Never call or take over the provider. |
| R.completed/failed, current replay deadline elapsed | Under A->R locks recheck phase/current deadline and delete R only. Completed already advanced A; failed proves no consumption. Do not modify a still-usable A. |
| R.unknown | Delete only after verifying A is unusable. If A remains usable, fail closed and alert; do not erase evidence or reactivate it. |
| A authorization expired or explicitly ended | Make A unusable first: expired/revoked/reauth_required as applicable, auth expiry=now, entire login group cleared. Retain broker/RT digests, version and generation as fences. Settle pending R as unknown without provider calls; delete bounded child batches, then A only after locked NOT EXISTS R. |

For parent termination, mark A unusable in the first short transaction; any
remaining R.pending is blocked by that A immediately and must be settled as
unknown before deletion. Preserve an existing revoked/expired/reauth_required
state when settling children, rather than restoring active or disguising the
original reason. A late provider success/failure sees the parent/operation guard
fail and writes nothing. Retain ended A between child batches; new R insertion
locks and rejects it. Final deletion uses locked NOT EXISTS, never an unlocked
count. No orphan is legitimate, and no lock spans transactions.

A successful refresh clears any remaining login temporary group in the same
transaction that migrates A's credential digests and common HMAC version. A
concurrent login-cache sweep therefore either clears the old complete group or
observes it already NULL; it cannot destroy new authorization or change a key
version while old login digests remain. R's request-HMAC version stays fixed.

Cleanup never extends credentials or replay windows. Endpoint checks enforce
expiry even when cleanup is paused. An active login group can persist nearly
15 minutes from creation; clearing it leaves the authorization usable. R can
persist through its execution lease plus at most five minutes of replay. Ended
records have no extra permanent or 24-hour/30-day tombstone requirement. After
evidence is purged, return the authentication/unavailable behavior in README,
not a fabricated historical recovery error.

## Failure handling, observability and capacity

A lock skip is normal contention; advance and retry in a later cycle. On deadlock
or lock timeout roll back the entire transaction, rediscover with jitter, at most
three attempts within the pass budget. DB outage aborts the pass, alerts and uses
bounded reconnect backoff. A closed/discarded connection releases the transaction;
never return an open transaction to the pool. Never replace DB state with local
memory. A TCC kill switch pauses cleanup while endpoint expiry checks remain active.

Measure pass/transaction duration, business rows, lock skips, retries/failures and
oldest overdue age per queue. Warn at >60 seconds overdue for five minutes; page
at >5 minutes overdue or two minutes without a successful pass while work exists.
Integrity incidents alert immediately. These thresholds require load-test tuning.
Logs carry an independently generated UUIDv7 cleanup request_id, queue, operation
and sanitized error;
no token, proof, state, digest, ciphertext, provider body or user-ID metric labels.
Throttle on DB latency, replica lag or purge pressure. Scale aggregate worker
budgets explicitly when replicas change.

Size A by anonymous-login residence **and** all unended device authorizations,
including repeated logins. Size R by refresh arrival rate times lease/replay
residence, including lag and bursts. New login should best-effort revoke the old
A after the new bundle is durably saved; failure stays bounded by auth expiry.
No user/device quota or identity-search index is implied. Clearing BLOBs in A
avoids deleting its live authorization but still produces write/purge workload.
Measure actual envelopes, resident/high-water disk, indexes, write/delete/log
volume, I/O, history/purge backlog, replica lag and API latency under steady churn.
DELETE/NULL makes space reusable after purge, not guaranteed file shrinkage or
healthy page density. No automatic periodic OPTIMIZE/rebuild/partitioning;
maintenance needs measured DBA review, space and an impact window. Sustainable
cleanup capacity must exceed expiry arrivals and demonstrably drain bursts.

## Required real-MySQL acceptance cases

1. Multiple replicas discover one A/R: one processes it, others skip; no R is
   deleted when A is locked or missing. All expected index plans are inspected.
2. Callback/denial versus expiry and same-ID deletion/recreation: original
   state/version/deadline checks prevent an old worker/sweep changing the new A.
3. Claim ready->active changes login deadline after discovery: an old ready
   candidate neither deletes A nor ends its authorization.
4. Active/refreshing login-group expiry clears only temporary fields, without
   changing authorization, generation or usable credential digests.
5. Refresh success races with group cleanup and HMAC rotation: old login digests
   never survive a version migration; exact old R replay still authenticates.
6. Refresh completes before lease sweeper locking: no completed->unknown or
   revocation of the new generation; late workers cannot publish after lease loss.
7. Logout/natural expiry versus late failed/success response: no reactivation,
   release or upstream retry; pending children settle unknown before deletion.
8. Worker crash, deadlock, DB disconnect and lost commit acknowledgement at every
   boundary: rollback or rediscovery is safe; no open transaction or stale DML replay.
9. Ended A with more R than one batch remains until the last guarded deletion;
   no new R is admitted. Hot rows/queues do not starve others; backlog drains.
10. Continuous anonymous login, authorization, refresh, NULL/delete churn and
    replica scaling meet latency/space/purge/lag targets; malformed persisted
    phase shapes fail closed in every affected Store and cached-release path.

Static SQL parsing or SQLite predicate references do not establish MySQL locks,
isolation, SKIP LOCKED, purge, real cryptography or service behavior. These are
release-gate runtime tests, not claims of completed verification.
