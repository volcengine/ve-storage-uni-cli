# TOS Transfer Report Metadata Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make recursive `cp` Report/Manifest rows expose the invoking public command and the concrete transfer direction.

**Architecture:** Keep the shared `ve-tos` handler IDs unchanged internally. Normalize `command` only at persisted CSV boundaries, and derive each `cp` item's `operation` from its source/destination before both manifest construction and report recording so action-ID fingerprints remain identical.

**Tech Stack:** Rust, Cargo integration/unit tests, CSV transfer reports and manifests.

---

### Task 1: Lock the metadata contract with failing tests

**Files:**
- Modify: `crates/tos/src/handler/high_level.rs`
- Modify: `tests/cli_basic.rs`

- [x] **Step 1: Add a unit test for all four `cp` directions**

Build transfer-plan items for local-to-TOS, TOS-to-local, TOS-to-TOS, and
local-to-local mappings. Assert that `build_transfer_manifest` produces
`upload`, `download`, `copy`, and `local-copy` respectively.

- [x] **Step 2: Add an integration test for the public command boundary**

Run a local recursive `tos cp` with explicit report and manifest paths. Assert
that both CSV rows contain `command=tos cp` and `operation=local-copy`.

- [x] **Step 3: Verify RED**

Run:

```bash
cargo test -p ve-tos-cli-core recursive_cp_manifest_uses_directional_operations -- --nocapture
cargo test --test cli_basic test_byted_tos_recursive_cp_persists_public_command_and_operation -- --nocapture
```

Expected: both tests fail because current rows contain `operation=copy`, and
the integration test additionally observes `command=ve-tos cp`.

### Task 2: Implement boundary normalization

**Files:**
- Modify: `crates/tos/src/handler/high_level.rs`

- [x] **Step 1: Reuse one transfer-direction helper**

Use the existing source/destination classification for both single-file output
and recursive manifest/report rows. Do not add a new CSV column.

- [x] **Step 2: Align every recursive `cp` path**

Apply the derived operation in planned recursive execution, `--no-manifest`
streaming execution, and local-to-local recursive execution. Construct
manifest rows with the same value that the report recorder later uses.

- [x] **Step 3: Normalize persisted command names**

Apply `public_high_level_command` when creating streaming report sinks and when
writing collected reports or manifests. `Binary::Tos` maps `ve-tos ...` to
`tos ...`; `Binary::VeTos` remains unchanged.

- [x] **Step 4: Verify GREEN**

Run both focused commands from Task 1 and confirm they pass.

### Task 3: Regression, review, and commit

**Files:**
- Modify: `docs/high_level_commands_plan.md`
- Modify: `docs/superpowers/specs/2026-07-30-streaming-report-action-alignment-design.md`

- [x] **Step 1: Run formatting and focused transfer tests**

```bash
cargo fmt --all -- --check
cargo test -p ve-tos-cli-core
cargo test --test cli_basic
```

- [x] **Step 2: Run the full workspace regression suite**

```bash
cargo test --workspace
```

- [x] **Step 3: Review correctness, compatibility, security, performance, maintainability, robustness, testability, and observability**

Confirm that Manifest/Report fingerprints still resolve, `mv`/`sync` action
models are unchanged, `ve-tos` keeps its public command name, and no network or
credential behavior changed.

- [x] **Step 4: Commit the approved change without pushing**

```bash
git add crates/tos/src/handler/high_level.rs tests/cli_basic.rs docs/high_level_commands_plan.md docs/superpowers/specs/2026-07-30-streaming-report-action-alignment-design.md docs/superpowers/plans/2026-08-06-tos-transfer-report-metadata.md
git commit -m "fix: align TOS transfer report metadata"
```
