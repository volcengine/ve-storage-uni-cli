# Injected login lifecycle implementation plan

Scope: implement create, ticket-authenticated poll, fenced callback and proof-bound first claim/replay against the existing writer Store and cryptographic ring. Keep bootstrap and refresh/status/logout fail-closed. No provider HTTP adapter, TCC wiring, schema or index changes.

1. Add tests for immutable deployment configuration, approved authorization URL validation, policy freshness and deny precedence, strict request validation and disabled bootstrap isolation.
2. Add documented provider/admission injection APIs, typed encrypted OAuth context and exact token bundle validation. Bind all metadata to configured deployment/application/environment and explicit key versions.
3. Extend Store with closed DB-clock snapshot and exact-snapshot credential-release confirmation; retain bounded A-first transactions and callback/claim CAS.
4. Implement creation retry, poll projection, committed callback reservation before provider I/O, guarded outcomes, current admission and scope validation, bounded claim/replay with final locked release check.
5. Add enabled HTTP routing with fixed safe HTML, bounded body reads, sanitized errors and separate callback deadline; retain bootstrap timeout and disabled routes.
6. Add opt-in real-MySQL integration coverage for lifecycle, retry/conflict, admission, failures, expiry, callback concurrency, claim concurrency and stale-write/ciphertext races. Run unit, contract and clippy checks and perform a complete correctness/security/performance/maintainability/robustness/testability/observability review.

The injected provider is trusted to implement documented platform behavior only after M0 evidence exists. Tests supply a deterministic fake; production has no wire adapter and main never constructs the enabled router.
