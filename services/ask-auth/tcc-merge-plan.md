# Self-contained TCC client and unified ask-auth implementation plan

> For agentic workers: use subagent-driven-development; test first, then independent specification and seven-category quality review. Reuse the approved current checkout. User approved implementation and merge on 2026-11-16; no further design approval is needed.

**Goal:** Remove every private Rust dependency and the separate ask-auth-tce workspace, while preserving OAuth/HTTP/DDL and deploying one ask-auth binary.

**Architecture:** Reuse the existing tos-core Consul implementation through a small public low-level lookup API; TCC bypasses bucket/BNS routing. Implement direct, authenticated BConfig V1 HTTP reads for the existing TCC V2 resource layout with public reqwest, validated TCE environment metadata and dynamically re-read DPS credentials. Move host assembly into services/ask-auth; preserve no-argument disabled local startup compatibility and put serve/probe/migrate on that same binary. No additional auth service/package, SDK registry or copied SDK source is introduced.

**Tech stack:** Existing root Cargo workspace, reqwest/rustls, Tokio, Serde, zeroize and tos-core. Preserve software-POLYVAL build flags and existing frozen schemas/migrations.

## Task 1: Share the existing Consul lookup

- [x] Add regression tests showing a caller can resolve a plain PSM with explicit IDC/cluster/address family without bucket routing. Cover encoded queries, IPv6, empty/invalid endpoints, non-success/redirect, timeout, bounded body/count and weighted selection.
- [x] Expose documented ConsulLookupClient / ConsulLookupRequest and existing AddrFamily / WeightedEndpoint through tos-core::infra::discovery. Retain existing TOS/BNS behavior and environment address precedence. Offer an explicitly bounded timeout constructor for TCC; no TOS test-endpoint override is used for TCC.
- [x] Fail with handled errors; do not follow discovery redirects or environment proxies. Keep service lookup payload bounded before deserialization. Add no private dependency.
- [x] Run `cargo test -p tos-core --locked --offline discovery` (loopback permission required), strict package Clippy and independent reviews. Commit only after parent verification.

## Task 2: Replace the SDK in place

- [x] In services/ask-auth-tce first, add HTTP and environment/credential regression tests and demonstrate failure before implementing the new transport.
- [x] Implement plain PSM resolution through the shared Consul API, then one HTTP GET per resource at `/v1/keys/tcc/v2/meta/{namespace}` and `/v1/keys/tcc/v2/data/{namespace}/{space}`. Select `bytedance.bconfig.proxy` for prod/canary and `bytedance.bconfig.envs_proxy` for other service environments, matching inspected BConfig V1 wire behavior. Target proxy cluster is default; caller cluster is only an identity header. Never use CDN or disable authentication.
- [x] Derive PSM/environment/IDC/cluster from validated TCE metadata. Preserve BOE/prod mismatch and PPE/offline rejection; unknown runtime metadata fails closed. BOE IDC names are boe, boe2 and boei18n. No shell/ip-discovery fallback or process-global SDK singleton.
- [x] Obtain DPS from platform SEC_TOKEN_PATH (bounded async file read on every request), or SEC_TOKEN_STRING if provided by the platform; validate sensitive HTTP header and recheck the source after the response. Credential disappearance/change rejects the response. Never persist/log credentials or pretend decoded claims establish identity.
- [x] Disable redirects, environment proxy, referer and automatic retry. Bound discovery, connection, request, streamed response and whole-snapshot deadlines; preserve original resource/schema/version/gray/depth/count/selected-payload validation. Allocate wiped bounded secret buffers; sanitize all transport/parser errors. Existing total snapshot acceptance deadline remains three seconds.
- [x] Verify real loopback HTTP wire requests, header propagation, credential rotation/disappearance, denied redirect with no token forwarding, forbidden/unknown/malformed/oversized/deep responses, independent fresh reads and timeout. Existing fake-reader tests remain useful but cannot replace transport tests.
- [x] Remove byted-bconfig, byted-dps, byted-env and private registry configuration. Test/build using only root/public registry resolution. Independent spec and quality review, then parent commit.

## Task 3: Merge and make deployment unambiguous

- [x] Move TCC and host modules/tests/scripts into services/ask-auth, use crate-local imports instead of a second ask_auth dependency. Replace the child entry point with commands on the existing ask-auth binary, retaining no-argument disabled local bootstrap/env compatibility and all existing contract tests.
- [x] Remove services/ask-auth-tce and its Cargo.lock/.cargo registry config. Root Cargo.lock is authoritative. Build script enters the root, uses `cargo build -p ask-auth --release --locked`, forces software-POLYVAL and packages one Linux binary plus exec wrapper.
- [x] Add integration regressions: help without identity/credentials, serve/probe/migrate routing, no secret argument output, unchanged no-argument startup/HTTP behavior, no private package/registry anywhere in resolved metadata and only one auth package/bin.
- [x] Consolidate an operator-focused Chinese README with exact root commands, deployment inputs, TCC schemas and DPS/Consul environment requirements. Mark historical plans as superseded and update active Feishu implementation/deployment references without altering diagrams or attachments.
- [x] Run root workspace check, ask-auth and affected tos-core tests, Clippy, formatting, seven Python contracts, guarded actual MySQL/TLS acceptance as appropriate, shell syntax and independent full reviews. Preserve exact runtime ownership, fail-closed controls and equal-cleanup scheduling fix #88.
- [x] Parent inspects staged paths, commits locally without pushing and verifies clean status. Real BOE protocol/ACL/OAuth and Linux artifact acceptance remain pending, with no App ID required for local development.


### Task 1 acceptance

19 discovery tests passed independently in the parent, spec reviewer and quality reviewer. Both reviews found 0 Critical/Major/Minor. The existing tos-core all-target Clippy completes with pre-existing diagnostics outside discovery; strict warnings-as-errors remains blocked by those unchanged files. No private dependencies or Cargo graph changes were introduced. Optional empty/whitespace PSM regression can be added with subsequent transport coverage; rejection is already implemented before I/O.


### Task 2 acceptance

Native TCC transport replaces all internal SDK dependencies. Parent independently ran 75 unit and 3 CLI tests, strict no-dependencies Clippy and a local build. Formatting and host-filtered offline metadata passed; all 381 lock entries use public registry/path sources and contain no byted-* package. Full all-platform offline metadata needs an uncached public target-specific etcetera crate; this is not a private registry dependency. Independent spec and seven-category quality reviews found 0 Critical/Major/Minor. Optional symlink retarget/recovery transport coverage can accompany Task 3 integration. Build verification initially exhausted the disk; deleting only this task's rebuildable temporary caches restored space and the build then passed. Real BOE network/ACL remains unverified.


### Task 3 acceptance

- Removed the old child project and merged its native TCC/host modules under services/ask-auth/src/platform. Exactly one ask-auth package and binary remain among the original eight workspace packages; all 418 lock entries use public registry or local path sources. No private registry configuration or byted-* dependency remains.
- Parent independently verified 207 ordinary service Rust tests, 20 shared discovery tests, 7 live HTTP/schema Python tests and 3 guarded actual VerifyIdentity MySQL tests. The scheduling test observes a real worker pass attempt on a deliberately closed store without business writes. Existing no-argument startup/drain tests and all new command-routing/metadata tests passed.
- Service strict all-target no-dependencies Clippy, build, root workspace check, formatting, shell syntax and diff checks passed. Evidence: /private/tmp/ask-auth-unified-root-proof.json and /private/tmp/ask-auth-unified-root-mysql-tls.log. Existing unrelated tos-core Clippy diagnostics were not changed.
- Independent specification and full integrated seven-category quality review passed. Specification review's two Minor documentation/ignore findings were fixed (#107); final quality review reports 0 Critical/Major/Minor/Suggestion. Prior optional empty-PSM and symlink-retarget/recovery coverage was added.
- Chinese operator README is authoritative; architecture.md preserves core details. Historical tce-plan.md is marked superseded. Native TCC/PSM ACL, real OAuth/M0, actual Linux SCM artifact, Windows cross-compilation, two replicas and measured capacity remain deployment acceptance, not local success claims.
