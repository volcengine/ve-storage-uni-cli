# TOS protocol corrections v1.0.4

User-approved scope: fix the preceding protocol audit on a separate branch/MR based on latest master; bump the affected release to 1.0.4; preserve ADrive changes already merged in MR94.

- [x] Fetch master and start codex/tos-protocol-v1.0.4 directly at origin/master, avoiding unrelated xsj-dev commits.
- [x] Verify request contracts against official SDK and available server source, including versionId, recursive/skipTrash, TTL days and middleware header normalization.
- [x] Add failing HTTP regression tests for metadata/time/expires/fetch/tagging/version selection/status, then correct object command serialization and discoverability. Include metadata escaping, replacement directives, Content-Type preservation and explicit overrides.
- [x] Add failing advanced request tests, then correct job type/job id, ObjectSet selectors and dataset binding body/query selectors.
- [x] Add opt-in real-service readback tests for metadata/time/expiry/tagging/fetch and version isolation; run local protocol/readback tests without touching real resources by default.
- [x] Update affected package/installer versions and lockfiles to 1.0.4, release notes and protocol reference documentation. Establish whether ByteTOS production paths change.
- [x] Run targeted tests, complete independent review and address all Critical/Major findings, then run relevant workspace and packaging checks.
Delivery gate: refreshed master is an ancestor of this branch and the final diff/tests are verified. Commit/push and the new master-targeting MR are recorded in the delivery message and MR metadata.

Implementation boundaries: object code and typed CLI arguments; advanced operation mapping; regression fixtures; release metadata. Production changes must carry Review Fix explanations. All protocol assertions derive from server/official sources rather than mirroring the implementation. No deployment, merge or release publication is authorized or needed.
