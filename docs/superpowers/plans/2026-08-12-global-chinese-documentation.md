# Global Chinese Documentation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove untranslated human-facing English prose from Chinese Help, Describe, and generated Skill output for `ve-adrive`, `ve-tos`, and `tos`, with exhaustive coverage tests that fail when new untranslated prose is introduced.

**Architecture:** Keep English Clap and registry metadata as the canonical machine contract. Extend the existing exact-phrase localization catalogs, make each owner module reuse one localizer for Describe and Skill, and add command-tree/catalog coverage tests that distinguish human prose from stable commands, flags, enum values, environment variables, URIs, and examples.

**Tech Stack:** Rust, Clap `CommandFactory`, Serde JSON, Cargo unit and integration tests.

---

### Task 1: Add exhaustive Help coverage auditing

**Files:**
- Modify: `src/lib.rs`
- Modify: `tests/cli_basic.rs`

- [ ] **Step 1: Add RED regressions for the reported mixed top-level output**

Extend the public CLI test so all currently reported English fragments are rejected:

```rust
#[test]
fn test_chinese_grouped_help_has_no_known_english_prose() {
    for surface in ["ve-adrive", "ve-tos", "tos"] {
        let help = successful_cli_stdout(&[surface, "--help", "--language", "zh"]);
        for forbidden in [
            "Move files or folders",
            "execution is unimplemented",
            "Manage ADrive CLI configuration",
            "scripts and installation snippets",
            "Start registry-backed MCP server",
            "external Agents and adapters",
        ] {
            assert!(!help.contains(forbidden), "surface={surface}: {help}");
        }
    }
}
```

- [ ] **Step 2: Run the regression and verify RED**

Run:

```bash
cargo test --test cli_basic chinese_grouped_help_has_no_known_english_prose -- --nocapture --test-threads=1
```

Expected: FAIL for `ve-adrive`, including `Move files or folders by same-space rename or copy plus source delete`.

- [ ] **Step 3: Add an exact metadata coverage helper**

Inside the existing `#[cfg(test)]` module in `src/lib.rs`, traverse the root Clap tree and validate every human help field. Split multiline before/after-help blocks into lines, skip blank lines, section labels, and example command lines, and require every remaining English source line to have an exact catalog entry:

```rust
fn has_exact_help_translation_zh(source: &str) -> bool {
    HELP_TRANSLATIONS_ZH
        .iter()
        .any(|(english, chinese)| *english == source.trim() && *english != *chinese)
}

fn is_cjk(character: char) -> bool {
    matches!(character, '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}')
}

fn is_machine_help_line(line: &str) -> bool {
    let line = line.trim();
    line.is_empty()
        || line.ends_with(':')
        || line.starts_with("ve-storage-uni-cli ")
        || line.starts_with("ve-tos-cli ")
        || line.starts_with("tos-cli ")
        || line.starts_with("ve-adrive-cli ")
        || line.starts_with("curl ")
}

fn assert_help_source_is_covered(path: &str, source: &str) {
    for line in source.lines().map(str::trim) {
        if is_machine_help_line(line) || line.chars().any(is_cjk) {
            continue;
        }
        assert!(
            has_exact_help_translation_zh(line),
            "missing Chinese help translation: command={path}, source={line:?}"
        );
    }
}
```

Walk `Command::get_subcommands`, `get_about`, `get_long_about`, `get_before_help`, `get_after_help`, each `Arg` help/long-help, and possible-value help. Separately validate every description used by `ve_tos_cli::registry::command_groups()`, `tos_cli::registry::capabilities()`, and `ve_adrive_cli::registry::capabilities()`.

- [ ] **Step 4: Run the exhaustive audit and verify RED**

Run:

```bash
cargo test --lib chinese_help_catalog_covers_complete_command_tree -- --nocapture
```

Expected: FAIL with the first exact missing command path and source phrase; no generic assertion message is accepted.

- [ ] **Step 5: Fill the Help catalog using full source phrases**

Add exact full-phrase pairs to `HELP_TRANSLATIONS_ZH`. The reported ADrive entries must include:

```rust
(
    "Move files or folders by same-space rename or copy plus source delete",
    "通过同空间重命名，或复制后删除源文件/文件夹来移动",
),
(
    "Delete a file, folder, or recursively clear a space",
    "删除文件、文件夹，或递归清空空间",
),
(
    "List instances, spaces, or files by target depth",
    "按目标层级列出实例、空间或文件",
),
(
    "Show file or folder metadata",
    "查看文件或文件夹元数据",
),
(
    "Inspect API metadata; execution is unimplemented",
    "查看 API 元数据；暂不支持执行",
),
(
    "Manage ADrive CLI configuration",
    "管理 ADrive CLI 配置",
),
(
    "Start registry-backed MCP server over stdio or local HTTP/SSE",
    "通过 stdio 或本地 HTTP/SSE 启动由 registry 支持的 MCP 服务",
),
(
    "List ADrive skill metadata or export Markdown SKILL.md files for external Agents and adapters",
    "列出 ADrive Skill 元数据，或为外部 Agent 和适配器导出 Markdown SKILL.md 文件",
),
```

For each additional failure, add one exact pair. Preserve technical identifiers such as TOS, ADrive, Bucket, API, MCP, HTTP/SSE, AK/SK, OAuth, Unified, profile, endpoint, JSON, URI, and flag/env names.

- [ ] **Step 6: Verify exhaustive Help GREEN**

Run:

```bash
cargo test --lib chinese_help_catalog_covers_complete_command_tree -- --nocapture
cargo test --test cli_basic chinese_grouped_help_has_no_known_english_prose -- --nocapture --test-threads=1
```

Expected: both commands exit 0.

- [ ] **Step 7: Commit Help coverage**

```bash
git add src/lib.rs tests/cli_basic.rs
git commit -m "docs: complete Chinese CLI help coverage"
```

### Task 2: Complete ADrive Describe and Skill localization

**Files:**
- Modify: `crates/adrive/src/handler/meta.rs`
- Modify: `tests/cli_basic.rs`

- [ ] **Step 1: Replace the fallback expectation with RED full-localization tests**

Add owner-unit tests that enumerate `skill_definitions_for_language(DocumentationLanguage::Zh)` and all ADrive capability/command metadata descriptions. Require Chinese prose and reject both fallback wrappers and known English phrases:

```rust
fn collect_schema_descriptions<'a>(value: &'a Value, descriptions: &mut Vec<&'a str>) {
    match value {
        Value::Object(map) => {
            if let Some(description) = map.get("description").and_then(Value::as_str) {
                descriptions.push(description);
            }
            for child in map.values() {
                collect_schema_descriptions(child, descriptions);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_schema_descriptions(item, descriptions);
            }
        }
        _ => {}
    }
}

#[test]
fn chinese_adrive_documentation_catalog_covers_describe_and_skills() {
    for capability in capabilities() {
        assert!(
            translate_adrive_documentation_text_zh(capability.description).is_some(),
            "command={}, source={:?}",
            capability.command,
            capability.description
        );
        for parameter in &capability.parameters {
            assert!(
                translate_adrive_documentation_text_zh(parameter.description).is_some(),
                "command={}, parameter={}, source={:?}",
                capability.command,
                parameter.name,
                parameter.description
            );
        }
    }
    let english = skill_definitions_for_language(DocumentationLanguage::En);
    let chinese = skill_definitions_for_language(DocumentationLanguage::Zh);
    for (source, localized) in english.iter().zip(&chinese) {
        assert_ne!(localized.description, source.description, "{}", source.name);
        assert!(!localized.description.contains("原始英文说明"), "{}", source.name);
        let mut source_descriptions = Vec::new();
        collect_schema_descriptions(&source.input_schema, &mut source_descriptions);
        for description in source_descriptions {
            assert!(
                translate_adrive_documentation_text_zh(description).is_some(),
                "skill={}, source={description:?}",
                source.name
            );
        }
    }
}
```

Add an integration assertion that `ve-adrive skill export --language zh` produces no `原始英文说明` and no reported English prose.

```rust
#[test]
fn test_chinese_adrive_documentation_has_no_english_fallback() {
    let markdown = export_skill_markdown(
        "ve-adrive",
        "ve_adrive_cp",
        "adrive-transfer",
        "adrive-global-zh",
    );
    assert!(!markdown.contains("原始英文说明"), "{markdown}");
    assert!(!markdown.contains("Copy local files"), "{markdown}");
    assert!(!markdown.contains("Source path"), "{markdown}");
}
```

- [ ] **Step 2: Run ADrive documentation tests and verify RED**

Run:

```bash
cargo test -p ve-adrive-cli-core chinese_adrive_documentation_catalog -- --nocapture
cargo test --test cli_basic chinese_adrive_documentation -- --nocapture --test-threads=1
```

Expected: FAIL on the first non-auth ADrive Skill or Describe description that still uses the English fallback.

- [ ] **Step 3: Make the ADrive catalog exact and shared**

Change the private translator to return an exact optional translation and make both Describe recursion and Skill generation use it:

```rust
fn translate_adrive_documentation_text_zh(text: &str) -> Option<&'static str> {
    ADRIVE_DOCUMENTATION_TRANSLATIONS_ZH
        .iter()
        .find_map(|(english, chinese)| (*english == text).then_some(*chinese))
}

fn localized_skill_description_zh(skill: &SkillDefinition) -> String {
    translate_adrive_documentation_text_zh(&skill.description)
        .unwrap_or(&skill.description)
        .to_string()
}
```

Remove the `用于调用 ... 原始英文说明：...` fallback. Populate `ADRIVE_DOCUMENTATION_TRANSLATIONS_ZH` from every RED failure until all ADrive command and parameter descriptions are covered. Keep stable strings unchanged.

- [ ] **Step 4: Verify ADrive GREEN**

Run the two Task 2 test commands and expect exit 0.

- [ ] **Step 5: Commit ADrive localization**

```bash
git add crates/adrive/src/handler/meta.rs tests/cli_basic.rs
git commit -m "docs: complete Chinese ADrive metadata"
```

### Task 3: Complete VeTos Describe and Skill localization

**Files:**
- Modify: `crates/tos/src/handler/meta.rs`
- Modify: `tests/cli_basic.rs`

- [ ] **Step 1: Add RED owner coverage for VeTos**

Enumerate the owner Skill definitions plus public Describe/Skill output for `ve-tos`. Reject `原始英文说明` and untranslated human descriptions while preserving service API names, flags, enum literals, URIs, and examples.

```rust
fn collect_schema_descriptions<'a>(value: &'a Value, descriptions: &mut Vec<&'a str>) {
    match value {
        Value::Object(map) => {
            if let Some(description) = map.get("description").and_then(Value::as_str) {
                descriptions.push(description);
            }
            for child in map.values() {
                collect_schema_descriptions(child, descriptions);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_schema_descriptions(item, descriptions);
            }
        }
        _ => {}
    }
}

#[test]
fn chinese_ve_tos_documentation_catalog_covers_describe_and_skills() {
    let english = skill_definitions_for_language(DocumentationLanguage::En);
    let chinese = skill_definitions_for_language(DocumentationLanguage::Zh);
    for (source, localized) in english.iter().zip(&chinese) {
        assert_ne!(localized.description, source.description, "{}", source.name);
        assert!(!localized.description.contains("原始英文说明"), "{}", source.name);
        let mut source_descriptions = Vec::new();
        collect_schema_descriptions(&source.input_schema, &mut source_descriptions);
        for description in source_descriptions {
            assert!(
                translate_tos_documentation_text_zh(description).is_some(),
                "skill={}, source={description:?}",
                source.name
            );
        }
    }
}
```

- [ ] **Step 2: Run TOS owner coverage and verify RED**

Run:

```bash
cargo test -p ve-tos-cli-core chinese_ve_tos_documentation_catalog -- --nocapture --test-threads=1
cargo test --test cli_basic chinese_ve_tos_documentation -- --nocapture --test-threads=1
```

Expected: FAIL on the first non-auth TOS/VeTos description that still uses the English fallback.

- [ ] **Step 3: Share one exact owner catalog between Describe and Skill**

Add the following exact owner localizer in `crates/tos/src/handler/meta.rs`:

```rust
fn translate_tos_documentation_text_zh(text: &str) -> Option<&'static str> {
    TOS_DOCUMENTATION_TRANSLATIONS_ZH
        .iter()
        .find_map(|(english, chinese)| (*english == text).then_some(*chinese))
}
```

Remove the English fallback wrapper, apply the translator recursively to Chinese VeTos Describe JSON and Skill schemas, and populate exact entries from every RED failure.

- [ ] **Step 4: Verify VeTos GREEN**

Run the two Task 3 commands and expect exit 0.

- [ ] **Step 5: Commit VeTos localization**

```bash
git add crates/tos/src/handler/meta.rs tests/cli_basic.rs
git commit -m "docs: complete Chinese VeTos metadata"
```

### Task 4: Complete TOS Describe and Skill localization

**Files:**
- Modify: `crates/toscli/src/handler/meta.rs`
- Modify: `tests/cli_basic.rs`

- [ ] **Step 1: Add RED owner coverage for TOS**

Add an exact-catalog contract locally in the independent `tos-cli-core` owner.
The test must enumerate `skill_definitions_for_language` in English and
Chinese, recursively collect every `input_schema.description`, and require an
exact TOS translation for each human source description:

```rust
fn collect_schema_descriptions<'a>(value: &'a Value, descriptions: &mut Vec<&'a str>) {
    match value {
        Value::Object(map) => {
            if let Some(description) = map.get("description").and_then(Value::as_str) {
                descriptions.push(description);
            }
            for child in map.values() {
                collect_schema_descriptions(child, descriptions);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_schema_descriptions(item, descriptions);
            }
        }
        _ => {}
    }
}

#[test]
fn chinese_tos_documentation_catalog_covers_describe_and_skills() {
    let english = skill_definitions_for_language(DocumentationLanguage::En);
    let chinese = skill_definitions_for_language(DocumentationLanguage::Zh);
    for (source, localized) in english.iter().zip(&chinese) {
        assert_ne!(localized.description, source.description, "{}", source.name);
        assert!(!localized.description.contains("原始英文说明"), "{}", source.name);
        let mut source_descriptions = Vec::new();
        collect_schema_descriptions(&source.input_schema, &mut source_descriptions);
        for description in source_descriptions {
            assert!(
                translate_byted_tos_documentation_text_zh(description).is_some(),
                "skill={}, source={description:?}",
                source.name
            );
        }
    }
}
```

- [ ] **Step 2: Run TOS owner coverage and verify RED**

```bash
cargo test -p tos-cli-core chinese_tos_documentation_catalog -- --nocapture --test-threads=1
cargo test --test cli_basic chinese_tos_documentation -- --nocapture --test-threads=1
```

Expected: FAIL on the first TOS Skill or Describe description using the English fallback.

- [ ] **Step 3: Add the independent exact TOS owner catalog**

```rust
fn translate_byted_tos_documentation_text_zh(text: &str) -> Option<&'static str> {
    BYTED_TOS_DOCUMENTATION_TRANSLATIONS_ZH
        .iter()
        .find_map(|(english, chinese)| (*english == text).then_some(*chinese))
}
```

Use this function for TOS Describe recursion, Skill descriptions, and Skill
schema descriptions. Remove the `原始英文说明` and `参数说明：<English>` fallbacks.
Keep command names, flags, TOS URI values, enums, and examples unchanged.

- [ ] **Step 4: Verify TOS GREEN**

Run the two Task 4 commands and expect exit 0.

- [ ] **Step 5: Commit TOS localization**

```bash
git add crates/toscli/src/handler/meta.rs tests/cli_basic.rs
git commit -m "docs: complete Chinese TOS metadata"
```

### Task 5: Add public full-tree and compatibility verification

**Files:**
- Modify: `tests/cli_basic.rs`
- Modify: `src/lib.rs`

- [ ] **Step 1: Add public output traversal tests**

Use the root Clap tree to derive every public command path. For each leaf, render Chinese Help and run the prose-residue assertion. For each surface, request Chinese Describe/Skill metadata and validate human fields. Error messages must include the surface, command path, JSON key or Markdown path, and untranslated text.

```rust
fn assert_rendered_help_uses_catalog(path: &str, english_help: &str) {
    let chinese_help = localize_clap_help_zh(english_help);
    for (english, chinese) in HELP_TRANSLATIONS_ZH {
        if english_help.contains(english) {
            assert!(
                chinese_help.contains(chinese),
                "command={path}, source={english:?}, expected={chinese:?}"
            );
        }
    }
}

fn assert_localized_command_tree(command: &Command, path: &str) {
    let mut rendered = command.clone();
    let english_help = rendered.render_long_help().to_string();
    assert_rendered_help_uses_catalog(path, &english_help);
    for child in command.get_subcommands() {
        let child_path = format!("{path} {}", child.get_name());
        assert_localized_command_tree(child, &child_path);
    }
}

#[test]
fn chinese_help_renders_complete_root_command_tree_without_partial_matches() {
    let command = Cli::command();
    assert_localized_command_tree(&command, "ve-storage-uni-cli");
    for grouped in [
        tos_grouped_help_zh(),
        byted_tos_grouped_help_zh(),
        adrive_grouped_help_zh(),
    ] {
        assert!(!grouped.contains("原始英文说明"), "{grouped}");
    }
}
```

The integration test then exercises one grouped output, one nested leaf, one Describe result, and one exported Skill for each of `ve-adrive`, `ve-tos`, and `tos`; exhaustive source and rendering coverage stays in the in-process command-tree tests so the suite does not spawn hundreds of processes.

- [ ] **Step 2: Add English compatibility tests**

For representative grouped, leaf, Describe, and Skill outputs, assert the English source phrases remain present under `--language en` and when no language is supplied:

```rust
#[test]
fn global_chinese_localization_does_not_change_english_documentation() {
    for args in [
        vec!["ve-adrive", "--help", "--language", "en"],
        vec!["ve-tos", "cp", "--help", "--language", "en"],
        vec!["tos", "skill", "export", "--help", "--language", "en"],
    ] {
        let output = successful_cli_stdout(&args);
        assert!(output.contains("Usage:") || output.contains("ADrive CLI"));
        assert!(!output.contains("说明:"));
    }
}
```

- [ ] **Step 3: Run global documentation coverage**

Run:

```bash
cargo test --lib chinese_help -- --nocapture
cargo test --test cli_basic chinese_documentation -- --nocapture --test-threads=1
```

Expected: all selected tests pass with no ignored command path.

- [ ] **Step 4: Commit global coverage tests**

```bash
git add src/lib.rs tests/cli_basic.rs
git commit -m "test: enforce global Chinese documentation coverage"
```

### Task 6: Review, verify, and deliver

**Files:**
- Verify: `src/lib.rs`
- Verify: `crates/adrive/src/handler/meta.rs`
- Verify: `crates/tos/src/handler/meta.rs`
- Verify: `crates/toscli/src/handler/meta.rs`
- Verify: `tests/cli_basic.rs`

- [ ] **Step 1: Perform the required first-reader Reviewer pass**

Review correctness, catalog completeness, exact machine-field preservation, English compatibility, translation consistency, performance, and test failure diagnostics. Fix every Critical/Major and mark each required correction with `// [Review Fix #N]`.

- [ ] **Step 2: Run final verification**

```bash
cargo test --test cli_basic -- --test-threads=1
cargo test -p ve-adrive-cli-core -- --test-threads=1
cargo test -p ve-tos-cli-core -- --test-threads=1
cargo test -p tos-cli-core -- --test-threads=1
cargo fmt --all -- --check
git diff --check
```

Expected: every command exits 0. If loopback tests are denied by the sandbox, rerun the identical test command with the already approved test permission.

- [ ] **Step 3: Confirm clean scoped history**

```bash
git status --short --branch
git log -6 --oneline --decorate
```

Expected: no unstaged or untracked implementation files; only the planned Chinese documentation commits are ahead of `origin/xsj-dev`.
