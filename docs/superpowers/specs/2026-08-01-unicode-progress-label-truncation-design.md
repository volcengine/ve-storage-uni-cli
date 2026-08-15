# Unicode-Safe Progress Label Truncation Design

## Goal

Fix recursive TOS transfer progress rendering so a long UTF-8 path is
truncated to a bounded terminal width without panicking or splitting a user-
perceived character.

## Scope

The change applies only to the `OverallProgress` current-file label shared by
recursive `cp`, `mv`, and `sync` paths in `ve-tos-cli-core`. It does not change
transfer behavior, path handling, progress-bar templates, or the existing
`--no-progress` option.

## Decision

Define a named maximum of 60 terminal columns for the current-file label. If
the label already fits, return it unchanged. Otherwise, reserve the measured
width of the leading ellipsis and retain the longest suffix whose measured
width fits in the remaining budget.

Measure width with `unicode-width` and choose suffix boundaries with
`unicode-segmentation` extended grapheme clusters. Both crates become direct
workspace dependencies of `ve-tos-cli-core`; their versions are already
present transitively in the lockfile.

The suffix budget is derived from the maximum label width and ellipsis width;
there is no independent hard-coded `59`. The result must always satisfy:

```text
display_width(result) <= MAX_PROGRESS_LABEL_WIDTH
```

## Alternatives Considered

1. Byte-boundary correction with `is_char_boundary`: prevents the panic but
   still measures the wrong quantity for terminal layout.
2. Unicode scalar counting with `chars()`: UTF-8-safe, but treats a two-column
   CJK character as one column and can split an emoji or combining sequence.
3. Display-width measurement plus grapheme boundaries: preserves the intended
   terminal-width limit and avoids invalid or visibly broken truncation. This
   is the selected approach.

## API and Integration

Extract a private pure helper in `crates/tos/src/handler/high_level.rs`:

```rust
fn truncate_progress_label(label: &str) -> String
```

`OverallProgress::set_current_file` passes the source label through the helper
before formatting the progress message. No public CLI or library API changes.

## Test Contract

Unit tests cover:

- ASCII below and exactly at the limit remains unchanged.
- Long ASCII is prefixed with an ellipsis and bounded to 60 columns.
- Long CJK input is truncated by terminal width without panic.
- Emoji ZWJ and combining-character sequences are not split.
- The retained suffix is the end of the original label.
- Empty and very short labels remain unchanged.

After focused tests pass, run formatting, the complete `ve-tos-cli-core` test
suite, and the full workspace tests. Reviewer checks correctness, Unicode
boundaries, width invariants, allocations, and regression risk.
