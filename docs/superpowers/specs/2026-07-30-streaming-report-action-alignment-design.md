# Streaming Report and Action-Aligned Manifest Design

## Goal

Reduce the memory used by large `cp`, `mv`, and `sync` batches by writing report
rows as actions finish, while making every report row unambiguously traceable to
one manifest action.

The change must preserve existing command behavior, report rollover size, and
the existing CSV column order. The package version is not changed.

## Scope

- Transfer manifests and reports produced by `cp`, `mv`, and `sync`.
- Streaming report writes with the existing 50 MiB part rollover.
- One manifest row per executable action.
- Stable correlation between a manifest action and its report result.

List-only manifests and unrelated commands keep their current schema and
behavior.

## CSV Schema

The current columns remain in their existing order. New columns are appended.

Transfer manifest additions:

- `schema_version`: `2`.
- `action_id`: unique within one command invocation.
- `depends_on_action_id`: optional predecessor action.

Transfer report additions:

- `schema_version`: `2`.
- `action_id`: the corresponding manifest action.

Report physical row order is completion order. It is not required to match
manifest row order.

## Action Model

Each planned operation receives an action ID before execution.

- `cp`: one `copy` action per object.
- `sync`: one `sync-copy` action per copied object and one `delete-extra`
  action per extra destination object.
- `mv`: one `copy` action followed by one conditional `delete-source` action.
  The delete action references the copy action through
  `depends_on_action_id`.

If an `mv` copy fails, its dependent delete action is recorded as skipped. This
keeps the manifest and report action sets reconcilable without relying on
physical line order.

## Streaming Report Writer

`BatchReport` retains summary counters only. An optional report sink owns the
rolling CSV writer and writes each completed action immediately.

- Successful and skipped rows are omitted when `--report-failures-only` is set.
- Counters are updated regardless of row filtering.
- Rows are emitted only by the execution coordinator, not worker futures, so
  the writer does not need cross-task locking.
- Each rolled part starts with the same header.
- CSV records are encoded before rollover selection, so no record is split
  across files.
- Finalization surfaces retained I/O errors and verifies that every manifest
  action was accounted for.

Completed action rows are not accumulated. When a manifest exists, the sink
also keeps a compact fingerprint-to-action-ID index so out-of-order
completions can be correlated without changing every transfer future type.
That index is O(number of planned actions), while the former completed-result
vectors (including repeated source, destination, and error strings) no longer
grow during execution. `--no-manifest` mode does not build this index.

## Failure Semantics

A report write or flush error is retained as a command error. Once reporting is
known to have failed:

- no new destructive follow-up action is started;
- `mv` source deletion and `sync --delete` deletion are blocked;
- already running non-destructive transfers are drained normally;
- the command returns a non-zero result.

A process interruption may leave a partial report, but every visible row is a
complete CSV record. The partial report remains useful together with the
manifest.

## Compatibility

- Existing columns keep their names and positions.
- New schema fields are appended.
- Existing 50 MiB rolling and part naming are retained.
- CLI flags and defaults do not change.
- Package version does not change.
- Exact-column-count consumers must allow the appended fields.

## Testing

Tests cover:

1. report rows are written before finalization and are not retained in vectors;
2. report rolling preserves headers and complete records;
3. `--report-failures-only` filters rows without changing counters;
4. manifest and report rows share action IDs;
5. `mv` emits copy and dependent delete actions;
6. failed `mv` copy emits a skipped delete result;
7. report writer failure blocks destructive follow-up actions;
8. existing CSV fields retain their order;
9. existing transfer and sync regression suites still pass;
10. the package version remains unchanged.
