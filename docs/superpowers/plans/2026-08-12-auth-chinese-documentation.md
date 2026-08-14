# Auth Chinese Documentation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fully localize the new ADrive OAuth/Unified and VeTos Unified authentication prose on Chinese help, describe, and generated Skill surfaces.

**Architecture:** Keep Clap help localization in the root dispatcher's existing exact-phrase table. Add exact authentication-only metadata localizers to the owning ADrive and VeTos meta modules, reuse them for describe and Skill output, and preserve all machine-readable identifiers and English behavior.

**Tech Stack:** Rust, Clap, Serde JSON, Cargo integration tests.

---

### Task 1: Localize authentication help

**Files:**
- Modify: `tests/cli_basic.rs`
- Modify: `src/lib.rs`

- [x] **Step 1: Write failing public help tests**

Add one ADrive test that runs both `ve-adrive auth --help --language zh` and
`ve-adrive auth login --help --language zh`. Assert Chinese descriptions for
the command actions, `--auth-mode`, `--instance`, `--auth-endpoint`, and
`--device-name`; assert the corresponding English authentication sentences are
absent. Add one VeTos test that runs `ve-tos ls --help --language zh`, asserts a
Chinese Unified authentication explanation, and rejects its English sentence.

- [x] **Step 2: Run the tests and verify RED**

Run:

```bash
cargo test --test cli_basic chinese_auth -- --nocapture --test-threads=1
```

Expected: the new assertions fail because the authentication prose remains
English.

- [x] **Step 3: Add exact authentication translations**

Extend `HELP_TRANSLATIONS_ZH` in `src/lib.rs` with exact mappings for:

```text
Inspect or manage ADrive authentication
Show the effective mode and credential availability without exposing secrets
Start OAuth Device Authorization login
Clear the current profile's locally persisted OAuth state; --dry-run only previews it
Authentication mode for this invocation: ...
Sign IDS requests with the existing access-key/secret-key mechanism
Use OAuth credentials and Bearer-authenticated Resource requests
Use credentials managed by the unified authentication integration
ADrive Instance ID to authorize. ...
OAuth Authorization Server base URL. ...
Human-readable device name shown during authorization
Unified login and logout are owned by the external framework; ...
```

Add the corresponding VeTos `aksk or unified` option and enum-description
mappings. Keep flags, environment variables, profile keys, mode values, and
example commands literal.

- [x] **Step 4: Run GREEN help tests**

Run the Task 1 focused command and expect all selected tests to pass.

### Task 2: Localize authentication describe metadata

**Files:**
- Modify: `tests/cli_basic.rs`
- Modify: `src/lib.rs`
- Modify: `crates/adrive/src/handler/meta.rs`
- Modify: `crates/tos/src/handler/meta.rs`

- [x] **Step 1: Write failing describe tests**

Run ADrive Auth describe with `--language zh --output json`. Assert its command
description, each Auth parameter description, authentication scenario routing,
and shell guidance are Chinese. Also assert stable values remain exact:
`auth-mode`, `ADRIVE_AUTH_MODE`, `aksk`, `oauth`, `unified`, `ve login`, and
`ve logout`. Add a VeTos describe case whose metadata contains `--auth-mode`
and assert only its explanatory prose is localized. Verify `--language en`
still returns the original English descriptions.

- [x] **Step 2: Run the tests and verify RED**

Run:

```bash
cargo test --test cli_basic chinese_auth_describe -- --nocapture --test-threads=1
```

Expected: Chinese describe assertions fail against English registry metadata.

- [x] **Step 3: Implement owner-scoped metadata localizers**

In each owning meta module, add a documented public function that recursively
visits JSON values and replaces only exact, frozen authentication prose. Its
private string helper must return unrelated text unchanged. Reuse the same
helper for schema descriptions.

In `src/lib.rs`, detect `requested_help_language(effective_args) == Some(HelpLanguage::Zh)`
inside the describe recovery functions and call the appropriate owner localizer
before constructing the Envelope. Do not translate keys, command examples,
enum literals, environment variables, or schema types.

- [x] **Step 4: Run GREEN describe tests**

Run the Task 2 focused command and expect all selected tests to pass.

### Task 3: Localize generated authentication Skills

**Files:**
- Modify: `tests/cli_basic.rs`
- Modify: `crates/adrive/src/handler/meta.rs`
- Modify: `crates/tos/src/handler/meta.rs`

- [x] **Step 1: Write failing Skill tests**

Use `skill list --language zh --output json` to select `ve_adrive_auth` and
`ve_tos_config_set`. Assert authentication descriptions and `auth-mode` schema
descriptions are Chinese and do not contain their corresponding English prose.
Run the same commands with `--language en` and assert existing English metadata
is unchanged.

- [x] **Step 2: Run the tests and verify RED**

Run:

```bash
cargo test --test cli_basic chinese_auth_skill -- --nocapture --test-threads=1
```

Expected: selected Chinese Skill metadata still contains unchanged English
authentication descriptions.

- [x] **Step 3: Reuse metadata localization in Skill generation**

Update `skill_definitions_for_language` and schema localization in both owner
modules. For Chinese output, use the exact authentication translator before the
existing fallback wrapper. Auth-specific descriptions become Chinese; unrelated
legacy Skill descriptions keep their current behavior. English generation is
untouched.

- [x] **Step 4: Run GREEN Skill tests**

Run the Task 3 focused command and expect all selected tests to pass.

### Task 4: Review, verify, and commit

**Files:**
- Verify: `src/lib.rs`
- Verify: `crates/adrive/src/handler/meta.rs`
- Verify: `crates/tos/src/handler/meta.rs`
- Verify: `tests/cli_basic.rs`

- [x] **Step 1: Perform the required Reviewer pass**

Check correctness, exact localization scope, stable machine values, English
compatibility, output safety, and maintainability. Fix every Critical/Major
finding and mark required review corrections with `// [Review Fix #N]`.

- [x] **Step 2: Run final verification**

```bash
cargo test --test cli_basic -- --test-threads=1
cargo test -p ve-adrive-cli-core -- --test-threads=1
cargo test -p ve-tos-cli-core -- --test-threads=1
cargo fmt --all -- --check
git diff --check
```

Expected: all commands exit 0. Loopback-binding tests may require the approved
test sandbox permission; rerun the same command with that permission rather
than altering tests.

- [x] **Step 3: Commit the implementation**

```bash
git add src/lib.rs crates/adrive/src/handler/meta.rs crates/tos/src/handler/meta.rs tests/cli_basic.rs docs/superpowers/plans/2026-08-12-auth-chinese-documentation.md
git commit -m "docs: localize authentication documentation"
```
