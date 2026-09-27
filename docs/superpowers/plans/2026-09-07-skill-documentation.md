# Skill documentation implementation plan

Goal: export usable, entrypoint-aware command skills and improve the three
repository agent skills without changing storage operations.

The user approved three requirements: dedicated and unified examples retain
their invocation context; command references explain behavior beyond schemas,
especially cp; repository skills guide complete tasks using focused references.

- [x] Add regression coverage for direct/unified exports, English/Chinese,
  metadata, command help, copy semantics, and read-only export planning.
- [x] Reuse the existing invocation-prefix configuration in every exported
  command, example, and index. Keep registry IDs and MCP tool names stable.
- [x] Add standard skill frontmatter and parser-derived argument reference;
  enrich transfer workflows with implementation-verified command guidance.
- [x] Improve skills/* and their independent references; distinguish discovery,
  execution, verification, failures, and authorized signed-link delivery.
- [x] Document installation versus export versus MCP in README; explain name
  selection, output layout, language, and update/conflict behavior.
- [x] Run focused Rust and Python tests and review all changes for correctness,
  security, performance, maintainability, resilience, testability, observability.

Validation: test actual CLI subprocesses in isolated temporary directories;
parse exports and inspect their executable prefixes. Use parser-derived help
to keep parameters current. Never execute remote storage mutations in tests.

Results: 179 CLI regression tests, 4 functional agent contract tests, 5 export
integration tests, 15 workspace skill unit tests, and 53 packaging/documentation
tests passed. Exported and checked all 672 English/Chinese command documents
(19 ByteTOS + 296 VeTOS + 21 ADrive, each in two languages).

Review: completed independent review and follow-up review. Corrected the TOS
local-copy include-parent exception, added nested-command/pipeline coverage,
fixed clap's cached usage prefix and dashed schema lookups, and hid ByteTOS's
unsupported storage-class option from the CLI table. No Critical/Major findings
remain. Existing transfer-engine behavior was documented rather than changed.
