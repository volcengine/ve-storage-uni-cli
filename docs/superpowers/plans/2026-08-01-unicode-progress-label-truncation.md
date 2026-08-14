# Unicode-Safe Progress Label Truncation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound recursive transfer progress labels to 60 terminal columns without panicking or splitting Unicode grapheme clusters.

**Architecture:** Add direct Unicode width and segmentation dependencies, then isolate truncation in one private pure helper beside `OverallProgress`. The progress renderer delegates to that helper; focused unit tests lock the width, suffix, ASCII, CJK, emoji, combining-mark, empty, and boundary behavior.

**Tech Stack:** Rust 2021, `unicode-width` 0.2, `unicode-segmentation` 1, Cargo unit/workspace tests.

---

### Task 1: Unicode-safe progress label helper

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/tos/Cargo.toml`
- Modify: `crates/tos/src/handler/high_level.rs:13750-13764`
- Test: `crates/tos/src/handler/high_level.rs` test module

- [ ] **Step 1: Write failing behavior tests**

Import `UnicodeWidthStr` in the test module and add focused tests that call the
not-yet-defined helper:

```rust
#[test]
fn progress_label_keeps_values_within_display_limit() {
    assert_eq!(truncate_progress_label(""), "");
    assert_eq!(truncate_progress_label("short"), "short");
    let boundary = "a".repeat(60);
    assert_eq!(truncate_progress_label(&boundary), boundary);
    let truncated = truncate_progress_label(&"a".repeat(61));
    assert_eq!(UnicodeWidthStr::width(truncated.as_str()), 60);
    assert_eq!(truncated, format!("…{}", "a".repeat(59)));
}

#[test]
fn progress_label_truncates_cjk_without_panicking() {
    let label = "务".repeat(31);
    let truncated = truncate_progress_label(&label);
    assert!(truncated.starts_with('…'));
    assert!(label.ends_with(truncated.trim_start_matches('…')));
    assert!(UnicodeWidthStr::width(truncated.as_str()) <= 60);
}

#[test]
fn progress_label_preserves_emoji_and_combining_graphemes() {
    for grapheme in ["👩‍💻", "e\u{301}"] {
        let label = format!("{}{}", "a".repeat(60), grapheme);
        let truncated = truncate_progress_label(&label);
        assert!(truncated.ends_with(grapheme));
        assert!(UnicodeWidthStr::width(truncated.as_str()) <= 60);
    }
}
```

- [ ] **Step 2: Run the tests and verify RED**

Run:

```bash
cargo test -p ve-tos-cli-core progress_label_
```

Expected: compilation fails because `truncate_progress_label` and the direct
Unicode dependency are not yet available.

- [ ] **Step 3: Add direct dependencies**

Add to `[workspace.dependencies]` in the root manifest:

```toml
unicode-segmentation = "1"
unicode-width = "0.2"
```

Add to `crates/tos/Cargo.toml`:

```toml
unicode-segmentation = { workspace = true }
unicode-width = { workspace = true }
```

- [ ] **Step 4: Implement the minimal helper and integrate it**

Add imports and the helper near `OverallProgress`:

```rust
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const MAX_PROGRESS_LABEL_WIDTH: usize = 60;
const PROGRESS_LABEL_ELLIPSIS: &str = "…";

fn truncate_progress_label(label: &str) -> String {
    if UnicodeWidthStr::width(label) <= MAX_PROGRESS_LABEL_WIDTH {
        return label.to_string();
    }
    let suffix_budget = MAX_PROGRESS_LABEL_WIDTH
        .saturating_sub(UnicodeWidthStr::width(PROGRESS_LABEL_ELLIPSIS));
    let mut suffix_width = 0;
    let mut suffix_start = label.len();
    for (index, grapheme) in label.grapheme_indices(true).rev() {
        let grapheme_width = UnicodeWidthStr::width(grapheme);
        if suffix_width.saturating_add(grapheme_width) > suffix_budget {
            break;
        }
        suffix_width += grapheme_width;
        suffix_start = index;
    }
    format!("{PROGRESS_LABEL_ELLIPSIS}{}", &label[suffix_start..])
}
```

Replace the byte-slicing block in `set_current_file` with:

```rust
let truncated = truncate_progress_label(label);
```

- [ ] **Step 5: Run focused tests and verify GREEN**

Run:

```bash
cargo test -p ve-tos-cli-core progress_label_
```

Expected: all focused tests pass with no panic.

- [ ] **Step 6: Format and run regressions**

Run:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test -p ve-tos-cli-core
cargo test --workspace --all-targets
```

Expected: all commands exit 0; existing external-service live tests may remain ignored.

- [ ] **Step 7: Perform mandatory Review**

Review correctness, UTF-8 and grapheme boundaries, width invariants, empty and
boundary inputs, allocation behavior, dependency scope, recursive transfer
compatibility, and unrelated regressions. Fix every Critical and Major issue,
add `[Review Fix #N]` comments for those fixes, and rerun Step 6.

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tos/Cargo.toml crates/tos/src/handler/high_level.rs
git commit -m "fix: truncate unicode progress labels safely"
```
