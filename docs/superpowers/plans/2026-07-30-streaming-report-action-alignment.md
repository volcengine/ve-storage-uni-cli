# Streaming Report and Action-Aligned Manifest Implementation Plan

> **For Codex:** Execute this plan in the current task with test-driven
> development. Do not change package versions.

**Goal:** Stream `cp`/`mv`/`sync` report rows to rolling CSV files and correlate
each row with one action-level manifest row.

**Architecture:** Keep `BatchReport` as the existing summary and compatibility
abstraction, but give transfer batches an optional `StreamingReportSink`.
The sink owns `RollingCsvWriter` plus an action resolver built from the
manifest. Non-transfer commands continue using the existing collected report
mode. Transfer manifests assign monotonic action IDs centrally, so execution
callers can resolve IDs from operation/source/destination without threading IDs
through every future type.

**Tech Stack:** Rust, Tokio/futures, standard library file I/O, Cargo unit and
integration tests.

---

## Task 1: Specify action-level CSV behavior with failing tests

**Files:**

- Modify: `crates/tos/src/handler/high_level.rs` test module

1. Add a test that constructs a copy manifest and verifies the existing
   manifest columns remain first while `schema_version`, `action_id`, and
   `depends_on_action_id` are appended.
2. Add a test that verifies a move manifest contains one copy and one
   `delete-source` row per input, with delete rows depending on the matching
   copy action.
3. Add a test that starts a streaming report, records one result, reads the
   report before finalization, and verifies that the result already exists.
4. Add a test that records results in reverse manifest order and joins them by
   `action_id`.
5. Add a failures-only test proving counters include successful actions while
   the CSV contains only failures.
6. Run the focused tests and confirm they fail for the missing schema and
   streaming behavior.

## Task 2: Add manifest action identities

**Files:**

- Modify: `crates/tos/src/handler/high_level.rs:870-975`
- Modify: `crates/tos/src/handler/high_level.rs:15216-15255`

1. Add `schema_version`, `action_id`, and `depends_on_action_id` to transfer
   manifest items.
2. Assign monotonic IDs in manifest construction after final action ordering is
   known.
3. Build move manifests in per-object action order and link `delete-source` to
   its copy action.
4. Normalize operation names so sync manifest rows use the same
   `sync-copy`/`delete-extra` values as report rows.
5. Append the new fields to the transfer manifest CSV and keep all old fields
   in their existing positions.
6. Run the manifest tests.

## Task 3: Implement rolling streaming report output

**Files:**

- Modify: `crates/tos/src/handler/high_level.rs:181-280`
- Modify: `crates/tos/src/handler/high_level.rs:15043-15320`

1. Add `StreamingReportSink` with a `RollingCsvWriter`, command name,
   failures-only setting, action resolver, and retained first write error.
2. Append `schema_version` and `action_id` to transfer report columns.
3. Make streaming-mode `record_success`, `record_failure`, and
   `record_skipped` encode and write one complete CSV row immediately instead
   of pushing it into a result vector.
4. Preserve collected mode for commands outside `cp`/`mv`/`sync`.
5. Surface retained writer errors and unresolved manifest actions from
   `write_tos_batch_report`; do not rewrite an already-streamed report.
6. Expose a report health predicate for destructive-action safety gates.
7. Run focused streaming and rollover tests.

## Task 4: Enable streaming for planned cp/mv/sync paths

**Files:**

- Modify: `crates/tos/src/handler/high_level.rs` recursive/local cp execution
- Modify: `crates/tos/src/handler/high_level.rs` recursive/local/HNS mv execution
- Modify: `crates/tos/src/handler/high_level.rs` recursive/local sync execution

1. Construct transfer reports from the final manifest, report path, command,
   and failures-only setting.
2. Remove legacy per-item report-path propagation for these batch copy
   operations so one action is never written twice.
3. Replace copy-phase `summary.failed == 0` destructive gates with the report
   health predicate.
4. Stop scheduling new delete actions after a report writer failure; mark
   remaining planned deletes skipped in the in-memory summary.
5. Keep final envelopes, exit codes, progress, checkpoint behavior, and report
   path defaults unchanged.
6. Run the existing cp/mv/sync unit tests.

## Task 5: Handle no-manifest compatibility paths

**Files:**

- Modify: `crates/tos/src/handler/high_level.rs` no-manifest streaming contexts

1. Keep `--no-manifest` discovery streaming and bounded.
2. Allocate action IDs monotonically as work is discovered because no manifest
   is emitted.
3. Route completion records through the same streaming report sink.
4. Ensure `mv --no-manifest` never deletes a source after report failure.
5. Add focused tests for generated action IDs and failures-only behavior.

## Task 6: Review and regression verification

**Files:**

- Review all modified files and both design/plan documents.

1. Run `cargo fmt --all -- --check`.
2. Run `cargo test -p ve-tos-cli-core`.
3. Run the repository transfer-related integration tests identified by
   `cargo test --workspace --no-run`/test discovery.
4. Run `cargo check --workspace`.
5. Verify Cargo package versions remain `1.0.3`.
6. Perform a full reviewer pass for correctness, safety, performance,
   maintainability, robustness, testability, and observability.
7. Fix all Critical and Major findings, mark review fixes in code, and rerun
   affected tests.
8. Request an independent code review and address its Critical/Major findings.
9. Inspect the final diff, commit the implementation separately, and report
   both commit IDs.
