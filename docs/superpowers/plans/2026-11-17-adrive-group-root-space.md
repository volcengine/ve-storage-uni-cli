# ADrive group RootSpace listing implementation plan

> Execute the approved design in this session. Deliver one focused commit.

**Goal:** Include RootSpace exactly once in group Space listing rows, within the existing result cap, with lowercase `root` in tabular presentation.

**Architecture:** Keep SDK response types unchanged. Isolate group pagination and versioned opaque cursor state in `handler/group_spaces.rs`. Normalize rows before manifest/count generation; apply the root owner label only to unqueried tabular output. Preserve raw ownership and the compatibility `root_space` field.

**Tech stack:** Rust, existing IDS V1 client, serde/hex, local HTTP fixtures.

- [x] Add `tests/group_space_listing_regression.py` using an OAuth fixture token and loopback server; verify the current binary fails because RootSpace is absent from `spaces`.
- [x] Implement a scoped group cursor carrying Instance, service marker, replay offset/limit, and the emitted root ID; accept legacy raw service markers, reject malformed/cross-scope CLI markers, and detect cyclic service pagination.
- [x] Merge RootSpace before ordinary entries; prefer an existing matching Space record, deduplicate the root, preserve unconsumed service-page positions and the strict `--max-keys` cap, including cap one and backend overfull pages.
- [x] Wire group listing into `high_level.rs`; add `is_root` to normalized Space rows and include merged rows in counts and explicit manifests. Keep user/AKSK list routes intact.
- [x] Render root ownership as `root` only for unqueried table/CSV/Markdown; JSON/YAML/XML and explicit query projections retain backend ownership.
- [x] Update CLI help, both localization catalogs, registry/Describe/Skill/MCP metadata, installed Skill, user documentation and changelog together.
- [x] Validate both actual entrypoints with root-only, missing/duplicate root, cap one, legacy/CLI continuation, changed caps, overfull/short pages, scope errors, marker cycles, raw versus tabular fields and manifest rows. Run affected library/discovery tests and perform independent review.
- [x] Resolve review findings, confirm a focused diff and rebuild debug binaries for the single-commit delivery.


## Verification and review

- RED: the original standalone binary failed `tests/group_space_listing_regression.py` because the first logical page contained ordinary Spaces but omitted RootSpace.
- GREEN: actual unified and standalone binaries pass the fixture. Coverage includes root-only/missing/duplicate/late root, sparse field fallback, cap one, changed caps, short/overfull pages, legacy and CLI markers, invalid/cross-scope markers, within-call and cross-call cycles, manifest rows, tabular ownership, raw query fields, bilingual help/Describe/Skill export and MCP schema discovery.
- ADrive library: 639 passed, 0 failed (serial, local fixture sockets enabled).
- Root library: 23 passed, 0 failed.
- Installed Skill packaging: 12 passed, 0 failed.
- Formatting and diff whitespace checks pass. These are local contract tests; no live backend request was made.
- Independent review round 1 identified two Major findings: sparse normal records overwrote complete root fields, and cycle detection did not span CLI calls. Both were fixed with field fallback and constant-memory Brent cursor state, with regressions added.
- Independent review round 2 plus final mechanical-conversion check passed: Critical 0, Major 0, remaining Minor 0. Review covered correctness, security, performance, maintainability, robustness, testability and observability.
- Non-snapshot limitation documented: an ID already returned as an ordinary Space before RootSpace first identifies it on a later logical page cannot be retroactively reclassified.
