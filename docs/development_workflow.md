# Development Workflow

This project uses different delivery processes for localized bug fixes and larger feature work. The process should match the actual risk and scope of the change.

## Small bug fixes

A change can use the small-bug workflow when the failure, root cause, and affected code are clear; it does not change a public API, command, configuration format, protocol, or persisted data; and focused tests can establish confidence.

Use the following workflow:

1. Reproduce the problem with the smallest practical test.
2. Make the minimum change required by the acceptance criteria.
3. Run focused tests for the affected behavior.
4. Review correctness, security, performance, maintainability, and boundary conditions once.
5. Run crate or workspace regression tests only when shared code, dependencies, build configuration, or cross-module behavior is affected.
6. Prefer one focused commit. A separate design document or implementation plan is not required.

Do not expand a small fix to include unrelated refactoring or pre-existing issues. Record out-of-scope findings separately unless they are critical, cross a security boundary, or prevent the original acceptance criteria from being met.

## Large feature work

Use the large-feature workflow when a change adds or modifies a public API, command, authentication mode, configuration or persistence format, external protocol, cross-module architecture, migration strategy, concurrency model, or other high-risk behavior.

Use the following workflow:

1. Define scope, compatibility requirements, external contracts, and acceptance criteria.
2. Document important design decisions and obtain agreement when multiple viable approaches exist.
3. Break implementation into verifiable stages.
4. Add tests while implementing each stage.
5. Review both specification compliance and code quality.
6. Fix all critical and major issues introduced by or within the scope of the change.
7. Run affected crate tests and workspace regression tests.
8. Document migration, rollout, rollback, and remaining work when applicable.

## User-visible surface synchronization

Any feature that adds or changes a command, option, default, validation rule,
authentication behavior, API routing decision, or output shape must review and
update every applicable user-facing discovery surface in the same change:

1. Clap `--help`, including option descriptions, accepted values, and examples.
2. Structured `--describe` output and the registry that also feeds
   `capabilities` or MCP discovery.
3. The matching installable `skills/*/SKILL.md`, including common workflows
   that an Agent cannot safely infer from generic discovery commands alone.
4. Design, reference, and user documentation that states the affected
   contract.
5. Tests that prevent these surfaces from drifting from runtime behavior.

If a surface is not applicable, record that conclusion in the implementation
review instead of silently skipping it.

CLI-generated error Usage and repair/diagnostic commands must use the active
entrypoint: `ve-adrive-cli`, `ve-tos-cli`, `tos-cli`, or the corresponding unified
invocation. Keep structured command identifiers canonical and preserve quoted
user input, upstream error text, and externally owned commands such as `ve login`.
Verify these boundaries with `tests/error_entrypoint_regression.py` against all
four binaries when changing shared error rendering. Include self-emitted batch
failure envelopes with `tests/batch_transfer_breadth_regression.py`; they can
bypass the root error handler.

Use `tos_core::agent::command_display` for trusted CLI-authored instructions.
Render templates before inserting user paths, server diagnostics, IDs or tokens;
do not replace command names in whole error messages or business JSON. Metadata
prose must be localized before rendering entrypoints. Canonical selectors such
as `--name "ve-tos bucket create"`, schema literals and completion registrations
remain machine values. `tests/command_presentation_regression.py` covers offline
Doctor checks, validation, cross-service remediation, plans and discovery across
all six dedicated/unified service combinations. Keep the presentation root
vocabulary aligned with the Clap tree via the root library consistency test.
Clap hides environment values while help is localized and reformatted; the
final printer restores registered environment values in one pass. Never parse
user values using annotation delimiters, because values can contain brackets,
newlines, command spellings or annotation-shaped text themselves.

## Escalating the process

When the task size is unclear, begin with the small-bug workflow. Escalate only when investigation reveals a public contract change, cross-module impact, security-sensitive behavior, migration requirement, or another concrete risk that the lightweight workflow cannot cover.

Before expanding the scope, state the reason and the additional work required. Avoid duplicate full test runs, unnecessary planning documents, and extra review rounds when there is no evidence that they reduce risk.

## Release verification boundaries

Local fixtures establish CLI behavior without proving every live backend or platform contract. Keep these checks separate from local regression results:

- Directory uploads to real HNS buckets and ADrive spaces, including empty directories and implicit parents.
- HNS direct recursive deletion combined with `--include-uploads`, whose live backend contract remains unverified.
- Native Windows execution, including ZTI authentication and filesystem link handling; cross-compilation or macOS fixtures do not replace it.
- Live share preview routing, tracked with the current [sharing test steps](adrive-share-testing.md).

Persistent local transfer coverage lives in `tests/directory_upload_regression.py`, `tests/batch_transfer_breadth_regression.py`, `tests/concurrent_part_report_regression.py`, `tests/report_request_id_regression.py`, `tests/hns_filtered_move_regression.py` and `tests/tos_unicode_transfer_regression.py`. Report actual commands and results when running them; historical test counts are not evidence for a newer checkout.
