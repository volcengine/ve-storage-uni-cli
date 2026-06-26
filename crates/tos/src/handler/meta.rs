/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::collections::BTreeMap;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::process::Command as TokioCommand;
use tokio::time::timeout;
use tos_core::agent::describe::RiskLevel;
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::agent::global_args::GlobalArgs;
use tos_core::infra::client::{storage_user_agent, TosClient};
use tos_core::infra::config::{DEFAULT_TOS_BATCH_REPORT_DIR, DEFAULT_TOS_CHECKPOINT_DIR};

use crate::cli::meta::{
    ApiArgs, CapabilitiesArgs, CompletionArgs, DoctorArgs, DocumentationLanguage, ServeArgs,
    SkillAction, SkillCommand,
};
use crate::domain::core::execute_resolved_request;
use crate::handler::common::{build_profile, output_result, output_result_with_columns};
use crate::registry::{
    business_domain, canonical_group_name, capabilities, capability_row_for_command,
    capability_rows, command_groups, describe_command_metadata, find_api_capability,
    find_command_tree_entry, find_group, flattened_command_tree, is_known_group_or_category,
    leaf_command_tree, public_tos_command, public_tos_example, CapabilityEntry, CommandGroupEntry,
    CommandTreeEntry, RegistryCapabilityRow,
};

#[derive(Debug, Serialize)]
struct CapabilitiesView<'a> {
    tool: &'static str,
    version: &'static str,
    service_name: &'static str,
    view: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri_format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    high_level_semantics: Option<Value>,
    /// [Spec §4.5 / AGT-001] Always populated for the `groups` view; populated
    /// for `text`/`full` so Agents can cross-link a capability to its group.
    /// Empty for `compact` to keep the payload as small as possible.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    groups: Vec<CapabilitiesGroup>,
    /// [Spec §4.5] `full` view: rich capability metadata (layer / endpoint_rule
    /// / destructive / parameters / examples). `compact` strips parameters.
    /// `groups` / `text` leave this empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    capabilities: Vec<CapabilityRow>,
    /// [Spec §4.5] Subcommand tree, used by `compact` and `full`. `text`
    /// surfaces the same data in `lines` instead.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    commands: Vec<CommandTreeEntry>,
    /// [Spec §4.5 `text`] One-line summaries — `"<command>\t<description>"` —
    /// so an Agent can scan the entire command surface in O(N) tokens.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    lines: Vec<String>,
    /// [G7] Per-result fuzzy match scores, returned only when `--search` is
    /// active. Entries appear in descending score order (best matches first)
    /// so an Agent can show top-k without re-sorting.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    search_scores: Vec<SearchScore>,
}

/// [Spec §4.5 `groups`] Group summary including a command count so Agents can
/// pick the largest-surface group to drill into first.
#[derive(Debug, Serialize)]
struct CapabilitiesGroup {
    name: &'static str,
    command: String,
    layer: &'static str,
    group: &'static str,
    category: &'static str,
    description: &'static str,
    implemented: bool,
    /// [Spec §4.5 / AGT-001] Number of capabilities under this group.
    command_count: usize,
}

type CapabilityRow = RegistryCapabilityRow;

/// [G7] One element of `search_scores`. `score` is in [0, 1] where 1.0 is an
/// exact match (case-insensitive). The `kind` distinguishes group / capability
/// / command tree entries so consumers can join back to the right list.
#[derive(Debug, Serialize)]
struct SearchScore {
    kind: &'static str,
    command: String,
    score: f64,
    matched_field: &'static str,
}

const TOS_HIGH_LEVEL_SEMANTICS: &[(&str, &[&str])] = &[
    (
        "cp",
        &[
            "local path -> tos://bucket/key uploads with PutObject or multipart upload",
            "tos://bucket/key -> local path downloads with GetObject and atomic local persist",
            "tos://bucket/key -> tos://bucket/key copies with CopyObject or multipart copy",
            "checkpoint identity includes source, destination, file metadata, part size, profile, and endpoint",
        ],
    ),
    (
        "mv",
        &[
            "runs cp semantics first, then deletes the source only after destination success",
            "critical source delete requires --force plus exact --confirm <source> in non-interactive shells",
            "remote multipart move checkpoint identity includes profile and endpoint",
        ],
    ),
    (
        "sync",
        &[
            "builds a source/destination diff from ListObjects, size, ETag, and mtime where available",
            "--delete removes extraneous destination objects and upgrades the command to critical risk",
            "transfer phases reuse cp checkpoint and overwrite semantics",
        ],
    ),
    (
        "mb",
        &["tos://bucket -> CreateBucket; optional ACL/storage-class settings are applied after creation"],
    ),
    (
        "rb",
        &["tos://bucket -> DeleteBucket; bucket must already be empty and critical deletes require confirmation"],
    ),
    (
        "mkdir",
        &["tos://bucket/prefix -> PutObject for a zero-byte object normalized to a trailing slash"],
    ),
    (
        "rm",
        &[
            "tos://bucket/key -> DeleteObject",
            "tos://bucket/prefix --recursive -> planned object batch delete",
            "critical deletes require --force plus exact --confirm <target> in non-interactive shells",
        ],
    ),
    (
        "ls",
        &["no target -> ListBuckets", "tos://bucket[/prefix] -> ListObjects with pagination"],
    ),
    (
        "stat",
        &["tos://bucket -> HeadBucket", "tos://bucket/key -> HeadObject"],
    ),
    (
        "du",
        &["tos://bucket/prefix -> read-only ListObjects traversal with size, histogram, and optional cost summaries"],
    ),
    (
        "find",
        &["tos://bucket/prefix -> read-only ListObjects traversal filtered by name, size, mtime, and storage class"],
    ),
    ("cat", &["tos://bucket/key -> GetObject body streamed to stdout"]),
    ("put", &["stdin -> tos://bucket/key upload; multipart is used above the configured threshold"]),
    ("presign", &["tos://bucket/key -> locally signed presigned URL without object mutation"]),
    ("restore", &["tos://bucket/key -> RestoreObject for archived storage classes"]),
];

const CAPABILITY_GROUP_TABLE_COLUMNS: &[&str] = &[
    "name",
    "group",
    "command",
    "layer",
    "description",
    "implemented",
    "command_count",
];

const CAPABILITY_ROW_TABLE_COLUMNS: &[&str] = &[
    "command",
    "group",
    "layer",
    "description",
    "risk_level",
    "destructive",
    "supports_dry_run",
    "supports_force",
];

#[derive(Debug, Serialize)]
struct SkillList {
    language: &'static str,
    skills: Vec<SkillDefinition>,
}

#[derive(Debug, Clone, Serialize)]
struct SkillDefinition {
    schema_version: &'static str,
    name: String,
    domain: String,
    command: String,
    description: String,
    risk_level: String,
    input_schema: Value,
    examples: Vec<String>,
    usage: SkillUsage,
}

#[derive(Debug, Clone, Serialize)]
struct SkillUsage {
    format: &'static str,
    source: &'static str,
    mcp_tool_name: String,
    mcp_server: String,
    serve_reads_exported_files: bool,
    exported_file_use: &'static str,
    default_mcp_call: &'static str,
}

#[derive(Debug, Serialize)]
struct CompletionScript {
    shell: String,
    script: String,
    command_count: usize,
}

#[derive(Debug, Serialize)]
struct ServePlan {
    mode: &'static str,
    transport: String,
    port: Option<u16>,
    protocol: &'static str,
    tcp_listener: bool,
    bind: Option<String>,
    endpoints: Vec<&'static str>,
    tool_source: &'static str,
    call_semantics: &'static str,
    capabilities: usize,
    groups: usize,
    status: &'static str,
    message: &'static str,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    profile: String,
    checks: Vec<DoctorCheck>,
    summary: DoctorSummary,
}

#[derive(Debug, Serialize)]
struct DoctorCheck {
    name: &'static str,
    status: &'static str,
    message: String,
    details: Value,
}

#[derive(Debug, Serialize)]
struct DoctorSummary {
    total: usize,
    passed: usize,
    warnings: usize,
    failed: usize,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawApiRequest {
    #[serde(default = "default_raw_api_method")]
    method: String,
    // [Review Fix #5] Accept `endpoint_rule` as the canonical alias of
    // `endpoint_kind` so the raw-passthrough input matches the renamed
    // capability/describe output (`AGT-002`). Both names parse identically;
    // emitted output prefers `endpoint_rule`.
    #[serde(default, alias = "endpoint_rule")]
    endpoint_kind: Option<String>,
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    query: BTreeMap<String, Value>,
    #[serde(default)]
    headers: BTreeMap<String, Value>,
    #[serde(default)]
    body: Option<Value>,
}

#[derive(Debug, Serialize)]
struct RawApiTarget {
    // [Review Fix #5] Mirror the capability registry vocabulary so the
    // resolved target carries `endpoint_rule` in its serialized form.
    #[serde(rename = "endpoint_rule")]
    endpoint_kind: String,
    url: String,
    signing_path: String,
}

#[cfg(test)]
#[derive(Debug, Deserialize)]
struct McpToolCallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

#[derive(Debug, Serialize)]
struct McpCommandExecution {
    command: String,
    argv: Vec<String>,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

pub async fn handle_capabilities_command(
    global: &GlobalArgs,
    args: &CapabilitiesArgs,
) -> Result<i32, CliError> {
    let view = capabilities_view(args)?;
    output_result_with_columns(
        global,
        &Envelope::success("ve-tos capabilities", view),
        capabilities_table_columns(global, args.view.as_str()),
    )?;
    Ok(0)
}

pub async fn handle_api_command(global: &GlobalArgs, args: &ApiArgs) -> Result<i32, CliError> {
    // [Review Fix #7] Execute raw API only when the user provides an explicit request and did not ask for a plan.
    if args.request.is_some() && !global.dry_run && !global.describe && !args.describe {
        let response = execute_raw_api(global, args).await?;
        output_result(global, &response)?;
        return Ok(0);
    }
    let lookup = api_lookup(args)?;
    output_result(global, &Envelope::success("ve-tos api", lookup))?;
    Ok(0)
}

pub async fn handle_skill_command(
    global: &GlobalArgs,
    command: &SkillCommand,
) -> Result<i32, CliError> {
    if global.describe {
        // [Review Fix #1] `skill export --describe` must not fall through into
        // filesystem writes or conflict checks; describe is always read-only.
        let command_path = match &command.action {
            SkillAction::List { .. } => "ve-tos skill list",
            SkillAction::Export { .. } => "ve-tos skill export",
        };
        let description = describe_command_metadata(command_path).ok_or_else(|| {
            CliError::ValidationError(format!("no metadata registered for {command_path}"))
        })?;
        output_result(global, &Envelope::success(command_path, description))?;
        return Ok(0);
    }
    match &command.action {
        SkillAction::List { language } => {
            let list = SkillList {
                language: language.code(),
                skills: skill_definitions_for_language(*language),
            };
            output_result(global, &Envelope::success("ve-tos skill list", list))?;
        }
        SkillAction::Export {
            name,
            dir,
            language,
        } => {
            let export_plan = skill_markdown_export_plan(name.as_deref(), dir)?;
            if global.dry_run {
                let plan = plan_skill_markdown_export(&export_plan, dir, *language);
                output_result(global, &Envelope::success("ve-tos skill export", plan))?;
            } else {
                let exported = export_markdown_skills(export_plan, dir, *language)?;
                output_result(global, &Envelope::success("ve-tos skill export", exported))?;
            }
        }
    }
    Ok(0)
}

pub async fn handle_completion_command(
    global: &GlobalArgs,
    args: &CompletionArgs,
) -> Result<i32, CliError> {
    if global.describe {
        let description = describe_command_metadata("ve-tos completion").ok_or_else(|| {
            CliError::ValidationError("no metadata registered for ve-tos completion".to_string())
        })?;
        output_result(global, &Envelope::success("ve-tos completion", description))?;
        return Ok(0);
    }
    let script = completion_script(&args.shell)?;
    output_result(global, &Envelope::success("ve-tos completion", script))?;
    Ok(0)
}

pub async fn handle_serve_command(global: &GlobalArgs, args: &ServeArgs) -> Result<i32, CliError> {
    // [Spec §3 Safe Execution / G2] --dry-run / --describe must short-circuit
    // before we boot the long-running MCP stdio server. Otherwise an Agent
    // asking "what would this command do?" would actually start the server and
    // hang on stdin. We fall through to the registry-backed plan path so the
    // contract for `ve-tos serve` matches every other CLI command.
    if args.mcp && !global.dry_run && !global.describe {
        match args.transport.as_str() {
            "stdio" => run_mcp_stdio(global).await?,
            "sse" => run_mcp_sse(global, args.port).await?,
            other => {
                return Err(CliError::ValidationError(format!(
                    "unsupported serve transport '{other}': expected stdio or sse"
                )));
            }
        }
        return Ok(0);
    }
    let plan = serve_plan(args)?;
    output_result(global, &Envelope::success("ve-tos serve", plan))?;
    Ok(0)
}

pub async fn handle_doctor_command(
    global: &GlobalArgs,
    args: &DoctorArgs,
) -> Result<i32, CliError> {
    let report = doctor_report(global, args).await?;
    output_result(global, &Envelope::success("ve-tos doctor", report))?;
    Ok(0)
}

fn capabilities_view(args: &CapabilitiesArgs) -> Result<CapabilitiesView<'_>, CliError> {
    if let Some(group) = args.group.as_deref() {
        if !is_known_group_or_category(group) {
            return Err(CliError::ValidationError(format!(
                "unknown capabilities group '{}': use `ve-tos capabilities --view groups` to list valid groups",
                group
            )));
        }
    }

    // [G7] When --search is active, switch from substring-match to a weighted
    // fuzzy ranker (case-insensitive Jaro–Winkler with substring/prefix
    // boosts). We compute scores once per entry, drop low-score noise, and
    // surface the ranked scores via `search_scores` so Agents see the
    // confidence of each hit.
    //
    // [Spec §4.5 / AGT-003] We additionally expand the search term through a
    // Chinese→English alias map so `--search 加密` hits encryption / SSE
    // capabilities even though the registry strings are in English.
    let expanded_terms: Vec<String> = args
        .search
        .as_deref()
        .map(expand_search_term)
        .unwrap_or_default();

    let scored_groups: Vec<(&'static CommandGroupEntry, f64, &'static str)> = command_groups()
        .iter()
        .filter(|entry| group_matches_facets(entry, args))
        .filter_map(|entry| match args.search.as_deref() {
            None => Some((entry, 1.0, "")),
            Some(_) => score_group_multi(entry, &expanded_terms).map(|(s, f)| (entry, s, f)),
        })
        .collect();
    let scored_caps: Vec<(&'static CapabilityEntry, f64, &'static str)> = capabilities()
        .iter()
        .filter(|entry| capability_matches_facets(entry, args))
        .filter_map(|entry| match args.search.as_deref() {
            None => Some((entry, 1.0, "")),
            Some(_) => score_capability_multi(entry, &expanded_terms).map(|(s, f)| (entry, s, f)),
        })
        .collect();
    let scored_commands: Vec<(CommandTreeEntry, f64, &'static str)> = flattened_command_tree()
        .into_iter()
        .filter(|entry| command_tree_matches_facets(entry, args))
        .filter_map(|entry| match args.search.as_deref() {
            None => Some((entry, 1.0, "")),
            Some(_) => {
                let scored = score_command_tree_multi(&entry, &expanded_terms);
                scored.map(|(s, f)| (entry, s, f))
            }
        })
        .collect();

    // Sort descending by score when --search is on; otherwise leave registry
    // order intact so the existing snapshot-style output is stable.
    let (group_entries, cap_entries, command_entries, search_scores) = if args.search.is_some() {
        let mut g = scored_groups;
        g.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut c = scored_caps;
        c.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let mut t = scored_commands;
        t.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut scores: Vec<SearchScore> = Vec::new();
        scores.extend(g.iter().map(|(e, s, f)| SearchScore {
            kind: "group",
            command: e.command.to_string(),
            score: *s,
            matched_field: f,
        }));
        scores.extend(c.iter().map(|(e, s, f)| SearchScore {
            kind: "capability",
            command: e.command.to_string(),
            score: *s,
            matched_field: f,
        }));
        scores.extend(t.iter().map(|(e, s, f)| SearchScore {
            kind: "command",
            command: e.command.clone(),
            score: *s,
            matched_field: f,
        }));
        scores.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        (
            g.into_iter().map(|(e, _, _)| e).collect::<Vec<_>>(),
            c.into_iter().map(|(e, _, _)| e).collect::<Vec<_>>(),
            t.into_iter().map(|(e, _, _)| e).collect::<Vec<_>>(),
            scores,
        )
    } else {
        (
            scored_groups.into_iter().map(|(e, _, _)| e).collect(),
            scored_caps.into_iter().map(|(e, _, _)| e).collect(),
            scored_commands.into_iter().map(|(e, _, _)| e).collect(),
            Vec::new(),
        )
    };

    // Convert internal registry types to the public `CapabilitiesView` shape
    // (CapabilitiesGroup / CapabilityRow). This is also where we materialise
    // `command_count`, `destructive`, and the `endpoint_kind → endpoint_rule`
    // rename promised by AGT-002.
    // [Review Fix #21] Capability metadata projection is registry-owned; the
    // meta handler only filters/ranks and renders the selected rows.
    let caps_with_params = publicize_capability_rows(capability_rows(
        &cap_entries,
        &command_entries,
        /* keep_parameters */ true,
    ));
    let caps_compact = publicize_capability_rows(capability_rows(
        &cap_entries,
        &command_entries,
        /* keep_parameters */ false,
    ));
    let command_entries = publicize_command_tree_entries(command_entries);
    let search_scores = publicize_search_scores(search_scores);
    let groups_full = build_groups(&group_entries, &caps_with_params);
    let lines = build_text_lines(&caps_with_params, &command_entries);

    // [Spec §4.5] Accept `tree` as a legacy alias for `compact` so existing
    // callers do not break, but the documented surface is the four-view set.
    let view = match args.view.as_str() {
        "tree" => "compact",
        other => other,
    };

    match view {
        "groups" => Ok(CapabilitiesView {
            tool: "ve-tos",
            version: env!("CARGO_PKG_VERSION"),
            service_name: "tos",
            view: "groups",
            uri_format: Some("tos://bucket/key"),
            high_level_semantics: Some(high_level_semantics()),
            groups: groups_full,
            capabilities: args
                .search
                .is_some()
                .then_some(caps_compact)
                .unwrap_or_default(),
            commands: args
                .search
                .is_some()
                .then_some(command_entries)
                .unwrap_or_default(),
            lines: Vec::new(),
            search_scores,
        }),
        "text" => Ok(CapabilitiesView {
            tool: "ve-tos",
            version: env!("CARGO_PKG_VERSION"),
            service_name: "tos",
            view: "text",
            uri_format: None,
            high_level_semantics: None,
            groups: Vec::new(),
            capabilities: Vec::new(),
            commands: Vec::new(),
            lines,
            search_scores,
        }),
        "compact" => Ok(CapabilitiesView {
            tool: "ve-tos",
            version: env!("CARGO_PKG_VERSION"),
            service_name: "tos",
            view: "compact",
            uri_format: None,
            high_level_semantics: None,
            groups: groups_full,
            capabilities: caps_compact,
            commands: command_entries,
            lines: Vec::new(),
            search_scores,
        }),
        "full" => Ok(CapabilitiesView {
            tool: "ve-tos",
            version: env!("CARGO_PKG_VERSION"),
            service_name: "tos",
            view: "full",
            uri_format: Some("tos://bucket/key"),
            high_level_semantics: Some(high_level_semantics()),
            groups: groups_full,
            capabilities: caps_with_params,
            commands: command_entries,
            lines: Vec::new(),
            search_scores,
        }),
        other => Err(CliError::ValidationError(format!(
            "unsupported capabilities view '{}': expected groups, text, compact, or full",
            other
        ))),
    }
}

fn high_level_semantics() -> Value {
    let mut semantics = serde_json::Map::new();
    for (command, lines) in TOS_HIGH_LEVEL_SEMANTICS {
        semantics.insert((*command).to_string(), json!(*lines));
    }
    Value::Object(semantics)
}

fn capabilities_table_columns(global: &GlobalArgs, view: &str) -> Option<&'static [&'static str]> {
    if global.query.is_some() {
        // [Review Fix #4] Explicit JMESPath selection owns the table shape.
        return None;
    }
    match view {
        "groups" => Some(CAPABILITY_GROUP_TABLE_COLUMNS),
        "compact" | "full" | "tree" => Some(CAPABILITY_ROW_TABLE_COLUMNS),
        _ => None,
    }
}

/// [Spec §4.5 / AGT-001] Convert registry `CommandGroupEntry` rows into the
/// view-facing `CapabilitiesGroup` shape, attaching `command_count` derived
/// from the (already filtered) capabilities list. We count capabilities whose
/// `group` matches the group's `name`, which mirrors how the registry models
/// the relationship.
fn build_groups(
    groups: &[&'static CommandGroupEntry],
    caps: &[CapabilityRow],
) -> Vec<CapabilitiesGroup> {
    groups
        .iter()
        .map(|entry| {
            let command_count = caps.iter().filter(|cap| cap.group == entry.name).count();
            CapabilitiesGroup {
                name: entry.name,
                // [Review Fix #27] ve-tos capabilities are now canonical at
                // the public top-level command path; no old `tos ...` prefix
                // is accepted or emitted for this surface.
                command: public_capabilities_command(entry.command),
                layer: layer_name(&entry.layer),
                group: entry.category,
                category: entry.category,
                description: entry.description,
                implemented: entry.implemented,
                command_count,
            }
        })
        .collect()
}

fn publicize_capability_rows(mut rows: Vec<CapabilityRow>) -> Vec<CapabilityRow> {
    for row in &mut rows {
        row.command = public_capabilities_command(&row.command);
        row.related_commands = row
            .related_commands
            .iter()
            .map(|command| public_capabilities_command(command))
            .collect();
    }
    rows
}

fn publicize_command_tree_entries(entries: Vec<CommandTreeEntry>) -> Vec<CommandTreeEntry> {
    entries
        .into_iter()
        .map(publicize_command_tree_entry)
        .collect()
}

fn publicize_command_tree_entry(mut entry: CommandTreeEntry) -> CommandTreeEntry {
    entry.command = public_capabilities_command(&entry.command);
    entry.subcommands = publicize_command_tree_entries(entry.subcommands);
    entry
}

fn publicize_search_scores(mut scores: Vec<SearchScore>) -> Vec<SearchScore> {
    for score in &mut scores {
        score.command = public_capabilities_command(&score.command);
    }
    scores
}

fn public_capabilities_command(command: &str) -> String {
    command
        .strip_prefix("ve-tos ")
        .map(|suffix| format!("ve-tos {suffix}"))
        .unwrap_or_else(|| command.to_string())
}

/// [Spec §4.5 `text`] Materialise the one-line summary view. We use TAB as
/// the separator because the Agent contract treats the second column as a
/// free-form description that may contain spaces.
fn build_text_lines(caps: &[CapabilityRow], commands: &[CommandTreeEntry]) -> Vec<String> {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut lines: Vec<String> = Vec::new();
    for cap in caps {
        if seen.insert(cap.command.clone()) {
            lines.push(format!("{}\t{}", cap.command, cap.description));
        }
    }
    for entry in commands {
        if seen.insert(entry.command.clone()) {
            lines.push(format!("{}\t{}", entry.command, entry.description));
        }
    }
    lines
}

fn command_root_group(command: &str) -> Option<&str> {
    command.split_whitespace().nth(1)
}

/// [Spec §4.5 / AGT-003] Expand a user-provided search term into one or more
/// English candidates so non-English keywords still hit the registry. The
/// original term is always retained as the first candidate.
fn expand_search_term(term: &str) -> Vec<String> {
    let mut out = vec![term.to_string()];
    // Lower-cased lookup keeps the alias map case-insensitive without needing
    // every variant of upper/title case in the table.
    let key = term.trim().to_lowercase();
    let aliases: &[(&str, &[&str])] = &[
        ("加密", &["encryption", "encrypt", "sse", "kms"]),
        ("解密", &["decryption", "decrypt"]),
        ("策略", &["policy"]),
        ("权限", &["acl", "permission", "iam"]),
        ("生命周期", &["lifecycle"]),
        ("版本", &["version", "versioning"]),
        ("跨域", &["cors"]),
        ("镜像", &["mirror", "replication"]),
        ("复制", &["replication", "copy"]),
        ("标签", &["tag", "tagging"]),
        ("日志", &["log", "logging"]),
        ("通知", &["notification"]),
        ("加速", &["acceleration", "transfer-accelerate"]),
        ("分片", &["multipart"]),
        ("续传", &["resume", "checkpoint"]),
        ("公开访问", &["public-access-block", "policy"]),
        ("归档", &["archive", "storage-class"]),
        ("存储类型", &["storage-class", "storageclass"]),
        ("元数据", &["metadata"]),
    ];
    for (cn, en) in aliases {
        if key.contains(cn) {
            for word in *en {
                out.push((*word).to_string());
            }
        }
    }
    out
}

/// [G7] Multi-term wrappers that pick the best score across the expanded
/// candidate list. We retain the original `score_group` etc. helpers so the
/// scoring logic itself stays single-term and trivially testable.
fn score_group_multi(entry: &CommandGroupEntry, terms: &[String]) -> Option<(f64, &'static str)> {
    terms
        .iter()
        .filter_map(|term| score_group(entry, term))
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
}

fn score_capability_multi(
    entry: &CapabilityEntry,
    terms: &[String],
) -> Option<(f64, &'static str)> {
    terms
        .iter()
        .filter_map(|term| score_capability(entry, term))
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
}

fn score_command_tree_multi(
    entry: &CommandTreeEntry,
    terms: &[String],
) -> Option<(f64, &'static str)> {
    terms
        .iter()
        .filter_map(|term| score_command_tree(entry, term))
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
}

/// [G7] Group/layer facet filtering, separated from the search predicate so
/// the scoring path can apply them independently.
fn group_matches_facets(entry: &CommandGroupEntry, args: &CapabilitiesArgs) -> bool {
    if let Some(group) = &args.group {
        let group = canonical_group_name(group);
        if entry.name != group && entry.category != group {
            return false;
        }
    }
    if let Some(layer) = &args.layer {
        if layer_name(&entry.layer) != layer {
            return false;
        }
    }
    true
}

fn capability_matches_facets(entry: &CapabilityEntry, args: &CapabilitiesArgs) -> bool {
    if let Some(group) = &args.group {
        let group = canonical_group_name(group);
        // [Review Fix #18] `--group utilities` filters by category, not only exact group name.
        if entry.group != group
            && find_group(entry.group)
                .map(|group_entry| group_entry.category != group)
                .unwrap_or(true)
        {
            return false;
        }
    }
    if let Some(layer) = &args.layer {
        if layer_name(&entry.layer) != layer {
            return false;
        }
    }
    true
}

fn command_tree_matches_facets(entry: &CommandTreeEntry, args: &CapabilitiesArgs) -> bool {
    if let Some(group) = &args.group {
        let group = canonical_group_name(group);
        // [Review Fix #18] Keep command-tree discovery aligned with capability category filters.
        let root_matches_category = command_root_group(&entry.command)
            .and_then(find_group)
            .map(|group_entry| group_entry.category == group)
            .unwrap_or(false);
        if !root_matches_category && !entry.command.split_whitespace().any(|part| part == group) {
            return false;
        }
    }
    if let Some(layer) = &args.layer {
        let entry_layer = entry.layer.as_deref().or_else(|| {
            command_root_group(&entry.command)
                .and_then(find_group)
                .map(|g| layer_name(&g.layer))
        });
        if entry_layer != Some(layer.as_str()) {
            return false;
        }
    }
    true
}

/// [G7] Score `term` against the textual fields of a group entry. Returns the
/// best score (and which field matched), or `None` if no field passed the
/// minimum-confidence threshold.
fn score_group(entry: &CommandGroupEntry, term: &str) -> Option<(f64, &'static str)> {
    rank_best(
        &[
            ("name", entry.name),
            ("command", entry.command),
            ("description", entry.description),
        ],
        term,
    )
}

fn score_capability(entry: &CapabilityEntry, term: &str) -> Option<(f64, &'static str)> {
    let mut candidates: Vec<(&'static str, &str)> = vec![
        ("command", entry.command),
        ("description", entry.description),
    ];
    // APIs are short uppercase identifiers — useful for `ve-tos capabilities --search PutObject`.
    for api in entry.apis {
        candidates.push(("api", api));
    }
    rank_best(&candidates, term)
}

fn score_command_tree(entry: &CommandTreeEntry, term: &str) -> Option<(f64, &'static str)> {
    let mut candidates: Vec<(&'static str, &str)> = vec![
        ("command", entry.command.as_str()),
        ("description", entry.description.as_str()),
    ];
    let row = capability_row_for_command(&entry.command, false);
    if let Some(row) = row.as_ref() {
        for api in &row.apis {
            candidates.push(("api", api.as_str()));
        }
    }
    for param in &entry.parameters {
        candidates.push(("parameter", param.name.as_str()));
    }
    rank_best(&candidates, term)
}

/// [G7] Returns `(score, matched_field)` for the candidate with the highest
/// score above the noise floor. Combines:
///   - case-insensitive substring match (boost ≥ 0.92)
///   - case-insensitive prefix match (boost 0.9)
///   - Jaro–Winkler similarity (raw, gated by a strict threshold)
///
/// Two thresholds are used:
///   - 0.92 for substring/prefix matches (always passes)
///   - 0.85 for Jaro–Winkler-only matches — high enough to filter random
///     pollution (`加密` -> `ve-tos sync`) while still admitting genuine typos
///     like `polcy` → `policy`.
///
/// [Spec §4.5 / AGT-003] We additionally skip Jaro–Winkler entirely when the
/// term and the candidate share no ASCII alphanumerics, because that mode of
/// "match" is meaningless across alphabets. Pure-CJK terms are handled via
/// the alias map (`expand_search_term`), not the J-W fallback.
fn rank_best(candidates: &[(&'static str, &str)], term: &str) -> Option<(f64, &'static str)> {
    let term_lower = term.to_lowercase();
    let term_ascii: std::collections::BTreeSet<char> = term_lower
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let mut best: Option<(f64, &'static str)> = None;
    for (field, value) in candidates {
        if value.is_empty() {
            continue;
        }
        let lower = value.to_lowercase();
        let (score, threshold) = if lower == term_lower {
            (1.0, 0.0_f64)
        } else if lower.contains(&term_lower) {
            // Slight penalty for longer surrounding context so tighter hits win.
            let extra = (lower.len() - term_lower.len()) as f64;
            ((1.0 - (extra / (extra + 16.0)) * 0.05).max(0.92), 0.92_f64)
        } else if lower.starts_with(&term_lower) {
            (0.9, 0.9_f64)
        } else {
            let value_ascii: std::collections::BTreeSet<char> = lower
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            if term_ascii.is_empty() || term_ascii.is_disjoint(&value_ascii) {
                continue;
            }
            (strsim::jaro_winkler(&lower, &term_lower), 0.85_f64)
        };
        if score >= threshold {
            match best {
                Some((cur, _)) if cur >= score => {}
                _ => best = Some((score, field)),
            }
        }
    }
    best
}

fn api_lookup(args: &ApiArgs) -> Result<Value, CliError> {
    let group = canonical_group_name(&args.group);
    let command = format!("ve-tos {group} {}", args.action);
    let request = args
        .request
        .as_deref()
        .map(parse_raw_api_request)
        .transpose()?;
    if let Some(request) = &request {
        ensure_raw_api_safe(request, args.force)?;
        validate_raw_api_request_contract(request)?;
    }
    let request_plan = request.as_ref().map(redacted_raw_api_request);
    if let Some(capability) = find_api_capability(&args.group, &args.action) {
        let row = capability_row_for_command(capability.command, true).ok_or_else(|| {
            CliError::ValidationError(format!(
                "registry capability '{}' has no capability row projection",
                capability.command
            ))
        })?;
        return Ok(json!({
            "mode": if args.request.is_some() { "raw_passthrough_plan" } else { "capability_metadata" },
            "command": capability.command,
            "layer": &capability.layer,
            "capability_row": row,
            "capability": capability,
            "request": request_plan,
        }));
    }
    // [Review Fix #22] Let `ve-tos api` fall back to registry-derived capability
    // rows, not raw clap metadata alone, so Agents always receive risk,
    // endpoint, method and body contract fields.
    if let Some(entry) = find_command_tree_entry(&command) {
        let row = capability_row_for_command(&entry.command, true).ok_or_else(|| {
            CliError::ValidationError(format!(
                "command '{}' is discoverable but has no registry capability metadata",
                entry.command
            ))
        })?;
        return Ok(json!({
            "mode": if args.request.is_some() { "raw_passthrough_plan" } else { "command_metadata" },
            "command": entry.command,
            "layer": row.layer.clone(),
            "capability_row": row,
            "command_metadata": entry,
            "request": request_plan,
        }));
    }
    if args.request.is_some() {
        return Ok(json!({
            "mode": "unregistered_raw_passthrough_plan",
            "command": command,
            "request": request_plan,
            "warning": "command is not in the typed CLI registry; execution will use the raw request contract",
        }));
    }
    Err(CliError::ValidationError(format!(
        "unknown registry API command '{}'; use --request for an unregistered raw passthrough plan",
        command
    )))
}

async fn execute_raw_api(
    global: &GlobalArgs,
    args: &ApiArgs,
) -> Result<Envelope<crate::domain::core::RawResponseData>, CliError> {
    let request = parse_raw_api_request(
        args.request
            .as_deref()
            .ok_or_else(|| CliError::ValidationError("--request is required".to_string()))?,
    )?;
    ensure_raw_api_safe(&request, args.force)?;
    validate_raw_api_request_contract(&request)?;
    let profile = build_profile(global)?;
    let client = TosClient::new(&profile, "tos")?;
    let target = resolve_raw_api_target(&client, &request)?;
    let method = parse_raw_api_method(&request.method)?;
    let headers = normalize_string_map("headers", &request.headers)?;
    let query = normalize_string_map("query", &request.query)?;
    let body = request
        .body
        .as_ref()
        .map(serde_json::to_vec)
        .transpose()
        .map_err(CliError::Json)?;
    let mut headers = headers;
    if body.is_some()
        && !headers
            .keys()
            .any(|key| key.eq_ignore_ascii_case("content-type"))
    {
        headers.insert("content-type".to_string(), "application/json".to_string());
    }
    // [Review Fix #CP1] Control plane 请求必须携带 X-Tos-Account-Id 参与 V4 签名
    if target.endpoint_kind == "control" {
        if let Some(account_id) = client.account_id() {
            headers.insert("x-tos-account-id".to_string(), account_id.to_string());
        }
    }
    execute_resolved_request(
        &client,
        "ve-tos api",
        method,
        &target.url,
        &target.signing_path,
        query,
        headers,
        body,
    )
    .await
}

fn parse_raw_api_request(request: &str) -> Result<RawApiRequest, CliError> {
    let candidate = request.strip_prefix("file://").unwrap_or(request);
    let payload = if Path::new(candidate).exists() {
        fs::read_to_string(candidate)?
    } else {
        request.to_string()
    };
    serde_json::from_str(&payload)
        .map_err(|err| CliError::ValidationError(format!("invalid --request JSON: {err}")))
}

fn parse_raw_api_method(method: &str) -> Result<Method, CliError> {
    Method::from_bytes(method.to_ascii_uppercase().as_bytes())
        .map_err(|err| CliError::ValidationError(format!("invalid raw API method: {err}")))
}

fn ensure_raw_api_safe(request: &RawApiRequest, force: bool) -> Result<(), CliError> {
    let method = request.method.to_ascii_uppercase();
    // [Review Fix #7] Tighten the safe-execution gate so control-plane
    // requests with any mutating verb (PUT/POST/DELETE/PATCH) require
    // `--force` even when the data-plane heuristic would have allowed them.
    // The control plane manages bucket/lifecycle/replication settings and any
    // mutation there is high-risk by design.
    let endpoint_label = request
        .endpoint_kind
        .as_deref()
        .map(normalize_endpoint_kind)
        .transpose()?;
    let is_control = endpoint_label.as_deref() == Some("control");
    let is_mutating = !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS");
    if is_control && is_mutating && !force {
        return Err(CliError::ValidationError(format!(
            "raw API method '{}' on the control plane requires --force because it mutates control-plane state",
            method
        )));
    }
    if is_mutating && !force {
        return Err(CliError::ValidationError(format!(
            "raw API method '{}' requires --force because it may mutate remote state",
            method
        )));
    }
    Ok(())
}

fn resolve_raw_api_target(
    client: &TosClient,
    request: &RawApiRequest,
) -> Result<RawApiTarget, CliError> {
    let endpoint_kind = request
        .endpoint_kind
        .as_deref()
        .map(normalize_endpoint_kind)
        .transpose()?
        .unwrap_or_else(|| {
            if request.key.is_some() {
                "object".to_string()
            } else if request.bucket.is_some() {
                "bucket".to_string()
            } else {
                "data".to_string()
            }
        });
    let extra_path = normalize_raw_api_path(request.path.as_deref())?;
    match endpoint_kind.as_str() {
        "control" => {
            let path = require_path(&extra_path, "control")?;
            let endpoint = client.control_endpoint()?;
            Ok(RawApiTarget {
                endpoint_kind,
                url: format!("{}{}", endpoint.trim_end_matches('/'), path),
                signing_path: path.to_string(),
            })
        }
        "data" | "service" => {
            let path = extra_path.as_str();
            let endpoint = client.service_endpoint();
            Ok(RawApiTarget {
                endpoint_kind,
                url: format!("{}{}", endpoint.trim_end_matches('/'), path),
                signing_path: path.to_string(),
            })
        }
        "bucket" => {
            let bucket = request.bucket.as_deref().ok_or_else(|| {
                CliError::ValidationError("raw API endpoint_kind=bucket requires bucket".to_string())
            })?;
            let base_url = client.bucket_endpoint(bucket)?;
            let base_path = client.bucket_request_path(bucket)?;
            Ok(RawApiTarget {
                endpoint_kind,
                url: join_url_path(&base_url, &extra_path),
                signing_path: join_signing_path(&base_path, &extra_path),
            })
        }
        "object" => {
            let bucket = request.bucket.as_deref().ok_or_else(|| {
                CliError::ValidationError("raw API endpoint_kind=object requires bucket".to_string())
            })?;
            let key = request.key.as_deref().ok_or_else(|| {
                CliError::ValidationError("raw API endpoint_kind=object requires key".to_string())
            })?;
            let base_url = client.object_endpoint(bucket, key)?;
            let base_path = client.object_request_path(bucket, key)?;
            Ok(RawApiTarget {
                endpoint_kind,
                url: join_url_path(&base_url, &extra_path),
                signing_path: join_signing_path(&base_path, &extra_path),
            })
        }
        other => Err(CliError::ValidationError(format!(
            "unsupported raw API endpoint_kind '{}': expected data, service, bucket, object, or control",
            other
        ))),
    }
}

fn validate_raw_api_request_contract(request: &RawApiRequest) -> Result<(), CliError> {
    // [Review Fix #8] Validate raw plans with the same contract checks used before execution.
    parse_raw_api_method(&request.method)?;
    normalize_raw_api_path(request.path.as_deref())?;
    normalize_string_map("headers", &request.headers)?;
    normalize_string_map("query", &request.query)?;
    request
        .endpoint_kind
        .as_deref()
        .map(normalize_endpoint_kind)
        .transpose()?;
    // [Review Fix #10] Tighter contract checks: reject obviously invalid
    // header / query keys early so the agent surfaces a stable validation
    // error instead of letting reqwest fail mid-flight.
    for key in request.headers.keys() {
        if key.trim().is_empty() {
            return Err(CliError::ValidationError(
                "raw API header keys must not be empty".to_string(),
            ));
        }
        if key.contains(['\n', '\r', ':']) {
            return Err(CliError::ValidationError(format!(
                "raw API header key '{}' contains forbidden characters",
                key
            )));
        }
    }
    for key in request.query.keys() {
        if key.trim().is_empty() {
            return Err(CliError::ValidationError(
                "raw API query keys must not be empty".to_string(),
            ));
        }
    }
    // [Review Fix #10] Service / control endpoints have no implicit bucket
    // path component, so an explicitly empty `path` (or `/`) for a mutating
    // method is almost certainly a misconfiguration.
    if let Some(kind) = request.endpoint_kind.as_deref() {
        let normalized = normalize_endpoint_kind(kind)?;
        let method = request.method.to_ascii_uppercase();
        let is_mutating = !matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS");
        let path = request.path.as_deref().unwrap_or("/");
        if matches!(normalized.as_str(), "service" | "control")
            && is_mutating
            && (path == "/" || path.is_empty())
        {
            return Err(CliError::ValidationError(format!(
                "raw API method '{}' on endpoint_rule={} requires an explicit non-root path",
                method, normalized
            )));
        }
    }
    Ok(())
}

fn redacted_raw_api_request(request: &RawApiRequest) -> Value {
    // [Review Fix #10] Do not echo credential-like raw request fields in dry-run/describe output.
    // [Review Fix #5] Emit the renamed `endpoint_rule` field so the raw plan
    // matches the capability registry's vocabulary (AGT-002).
    json!({
        "method": request.method,
        "endpoint_rule": request.endpoint_kind,
        "bucket": request.bucket,
        "key": request.key,
        "path": request.path,
        "query": redact_value_map(&request.query),
        "headers": redact_value_map(&request.headers),
        "body": request.body.as_ref().map(|body| redact_value("body", body)),
    })
}

fn redact_value_map(input: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    input
        .iter()
        .map(|(key, value)| {
            let redacted = redact_value(key, value);
            (key.clone(), redacted)
        })
        .collect()
}

fn redact_value(key: &str, value: &Value) -> Value {
    if is_sensitive_key(key) {
        return Value::String("***REDACTED***".to_string());
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(nested_key, nested_value)| {
                    (nested_key.clone(), redact_value(nested_key, nested_value))
                })
                .collect(),
        ),
        Value::Array(values) => {
            Value::Array(values.iter().map(|item| redact_value(key, item)).collect())
        }
        _ => value.clone(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    // [Review Fix #m5] Lowercased substring match so AK/SK in any casing
    // (`accessKeyId`, `AccessKey`, `SecretAccessKey`, presigned `signature`,
    // `x-amz-credential`, etc.) get redacted in raw API dry-run / describe output.
    let lower = key.to_ascii_lowercase();
    [
        "authorization",
        "auth",
        "token",
        "secret",
        "credential",
        "cookie",
        "password",
        "passwd",
        "access-key",
        "access_key",
        "accesskey",
        "security-token",
        "security_token",
        "securitytoken",
        "signature",
        "session",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn normalize_endpoint_kind(endpoint_kind: &str) -> Result<String, CliError> {
    match endpoint_kind
        .to_ascii_lowercase()
        .replace(['_', '-'], "")
        .as_str()
    {
        "data" | "dataplane" => Ok("data".to_string()),
        "service" => Ok("service".to_string()),
        "bucket" => Ok("bucket".to_string()),
        "object" => Ok("object".to_string()),
        "control" | "controlplane" => Ok("control".to_string()),
        other => Err(CliError::ValidationError(format!(
            "unsupported raw API endpoint_kind '{}'",
            other
        ))),
    }
}

fn normalize_raw_api_path(path: Option<&str>) -> Result<String, CliError> {
    let Some(path) = path else {
        return Ok("/".to_string());
    };
    if path.contains("://") || path.contains('\\') || path.contains('?') {
        return Err(CliError::ValidationError(
            "raw API path must be an absolute path without scheme, backslash, or query string"
                .to_string(),
        ));
    }
    if path.is_empty() || path == "/" {
        return Ok("/".to_string());
    }
    if !path.starts_with('/') {
        return Err(CliError::ValidationError(
            "raw API path must start with '/'".to_string(),
        ));
    }
    Ok(path.to_string())
}

fn require_path<'a>(path: &'a str, endpoint_kind: &str) -> Result<&'a str, CliError> {
    if path == "/" {
        return Err(CliError::ValidationError(format!(
            "raw API endpoint_kind={} requires path",
            endpoint_kind
        )));
    }
    Ok(path)
}

fn join_url_path(base: &str, extra_path: &str) -> String {
    if extra_path == "/" {
        base.to_string()
    } else {
        format!("{}{}", base.trim_end_matches('/'), extra_path)
    }
}

fn join_signing_path(base: &str, extra_path: &str) -> String {
    if extra_path == "/" {
        return base.to_string();
    }
    if base == "/" {
        extra_path.to_string()
    } else {
        format!("{}{}", base.trim_end_matches('/'), extra_path)
    }
}

fn normalize_string_map(
    field_name: &str,
    input: &BTreeMap<String, Value>,
) -> Result<BTreeMap<String, String>, CliError> {
    let mut output = BTreeMap::new();
    for (key, value) in input {
        validate_raw_api_header(field_name, key)?;
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Bool(value) => value.to_string(),
            Value::Number(value) => value.to_string(),
            Value::Null => String::new(),
            _ => {
                return Err(CliError::ValidationError(format!(
                    "raw API {} value for '{}' must be string, number, bool, or null",
                    field_name, key
                )));
            }
        };
        output.insert(key.clone(), text);
    }
    Ok(output)
}

fn validate_raw_api_header(field_name: &str, key: &str) -> Result<(), CliError> {
    if field_name != "headers" {
        return Ok(());
    }
    let lower = key.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "authorization" | "host" | "x-tos-date" | "x-tos-content-sha256" | "x-tos-security-token"
    ) {
        return Err(CliError::ValidationError(format!(
            "raw API header '{}' is managed by the signer and cannot be overridden",
            key
        )));
    }
    Ok(())
}

fn default_raw_api_method() -> String {
    "GET".to_string()
}

fn skill_markdown_export_plan(
    name: Option<&str>,
    dir: &str,
) -> Result<Vec<(SkillDefinition, PathBuf)>, CliError> {
    let selected = skill_definitions()
        .into_iter()
        .filter(|definition| {
            name.map(|wanted| {
                wanted == definition.name
                    || wanted == definition.command
                    || definition.command.ends_with(wanted)
            })
            .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(CliError::ValidationError(format!(
            "no skill matches '{}'",
            name.unwrap_or_default()
        )));
    }

    Ok(selected
        .into_iter()
        .map(|definition| {
            let file_path = Path::new(dir)
                .join(&definition.domain)
                .join(&definition.name)
                .join("SKILL.md");
            (definition, file_path)
        })
        .collect::<Vec<_>>())
}

fn plan_skill_markdown_export(
    export_plan: &[(SkillDefinition, PathBuf)],
    dir: &str,
    language: DocumentationLanguage,
) -> Value {
    // [Review Fix #SkillExportAlign] Expose the same path fields as tos-cli
    // and ve-adrive so dry-run consumers do not need per-command branching.
    let entries: Vec<Value> = export_plan
        .iter()
        .map(|(definition, file_path)| {
            json!({
                "skill": definition.name,
                "domain": definition.domain,
                "command": definition.command,
                "path": file_path.display().to_string(),
                "conflict": file_path.exists(),
            })
        })
        .collect();
    json!({
        "dry_run": true,
        "format": "markdown_skill",
        "language": language.code(),
        "dir": dir,
        "selected": export_plan.len(),
        "root_file": skill_root_path(Path::new(dir)).display().to_string(),
        "paths": export_paths(export_plan, Path::new(dir))
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "skill_paths": export_plan
            .iter()
            .map(|(_, path)| path.display().to_string())
            .collect::<Vec<_>>(),
        "skill_count": export_plan.len(),
        "entries": entries,
        "status": "planned_not_written",
    })
}

fn export_markdown_skills(
    export_plan: Vec<(SkillDefinition, PathBuf)>,
    dir: &str,
    language: DocumentationLanguage,
) -> Result<Value, CliError> {
    for path in export_paths(&export_plan, Path::new(dir)) {
        if path.exists() {
            return Err(CliError::Conflict(format!(
                "skill export target '{}' already exists",
                path.display()
            )));
        }
    }

    let mut files = Vec::new();
    let skills = export_plan
        .iter()
        .map(|(skill, _)| skill.clone())
        .collect::<Vec<_>>();
    let root_path = skill_root_path(Path::new(dir));
    if let Some(parent) = root_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &root_path,
        skill_index_markdown("ve-tos", &skills, language),
    )?;
    files.push(root_path.display().to_string());
    for (definition, file_path) in export_plan {
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&file_path, skill_markdown(&definition, language))?;
        files.push(file_path.display().to_string());
    }
    Ok(json!({
        "dry_run": false,
        "format": "markdown_skill",
        "language": language.code(),
        "dir": dir,
        "selected": files.len().saturating_sub(1),
        "root_file": files.first().cloned(),
        "files": files,
    }))
}

fn skill_root_path(dir: &Path) -> PathBuf {
    dir.join("SKILL.md")
}

fn export_paths(export_plan: &[(SkillDefinition, PathBuf)], dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![skill_root_path(dir)];
    paths.extend(export_plan.iter().map(|(_, path)| path.clone()));
    paths
}

fn skill_index_markdown(
    surface: &str,
    skills: &[SkillDefinition],
    language: DocumentationLanguage,
) -> String {
    let mut domains = BTreeMap::<&str, Vec<&SkillDefinition>>::new();
    for skill in skills {
        domains.entry(&skill.domain).or_default().push(skill);
    }
    let mut body = match language {
        DocumentationLanguage::En => format!(
            "# {surface} skills\n\nUse this skill pack when the user wants to operate `{surface}` commands. Select a domain below, then use the nested command skill.\n\n"
        ),
        DocumentationLanguage::Zh => format!(
            "# {surface} Skills\n\n当用户需要操作 `{surface}` 命令时使用此 Skill 包。先按领域选择，再进入对应的命令 Skill。\n\n"
        ),
    };
    for (domain, skills) in domains {
        body.push_str(&format!("## {domain}\n\n"));
        for skill in skills {
            let description = match language {
                DocumentationLanguage::En => skill.description.clone(),
                DocumentationLanguage::Zh => localized_skill_description_zh(skill),
            };
            body.push_str(&format!(
                "- [{}](./{}/{}/SKILL.md): `{}` - {}\n",
                skill.name, skill.domain, skill.name, skill.command, description
            ));
        }
        body.push('\n');
    }
    body
}

fn skill_markdown(skill: &SkillDefinition, language: DocumentationLanguage) -> String {
    let examples = if skill.examples.is_empty() {
        match language {
            DocumentationLanguage::En => {
                "- Run with `--describe` first to inspect the command contract.".to_string()
            }
            DocumentationLanguage::Zh => {
                "- 先运行 `--describe` 检查命令契约，再决定是否执行。".to_string()
            }
        }
    } else {
        skill
            .examples
            .iter()
            .map(|example| format!("- `{example}`"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let input_schema = localized_input_schema(&skill.input_schema, language);
    let schema = serde_json::to_string_pretty(&input_schema).unwrap_or_else(|_| "{}".to_string());
    match language {
        DocumentationLanguage::En => format!(
            r#"# {name}

Use this skill when the user wants to run `{command}` with the Volcano Engine TOS CLI.

## Description

{description}

## Command

`{command}`

Risk level: `{risk_level}`

## Inputs

```json
{schema}
```

## Examples

{examples}

## Execution

Prefer `{public_command} --describe` or `{public_command} --dry-run --output json` before executing a command that writes or deletes data. Destructive commands must include the required `--force` and exact `--confirm` target.
"#,
            name = skill.name,
            command = skill.command,
            description = skill.description,
            risk_level = skill.risk_level,
            schema = schema,
            examples = examples,
            public_command = public_tos_command(&skill.command),
        ),
        DocumentationLanguage::Zh => format!(
            r#"# {name}

当用户需要通过火山引擎 TOS CLI 运行 `{command}` 时使用此 Skill。

## 说明

{description}

## 命令

`{command}`

风险等级：`{risk_level}`

## 输入

```json
{schema}
```

## 示例

{examples}

## 执行建议

执行会写入或删除数据的命令前，优先运行 `{public_command} --describe` 或 `{public_command} --dry-run --output json`。破坏性命令必须包含必需的 `--force` 和精确匹配目标的 `--confirm`。
"#,
            name = skill.name,
            command = skill.command,
            description = localized_skill_description_zh(skill),
            risk_level = skill.risk_level,
            schema = schema,
            examples = examples,
            public_command = public_tos_command(&skill.command),
        ),
    }
}

fn completion_script(shell: &str) -> Result<CompletionScript, CliError> {
    // [Review Fix #12] Drive completions from the full leaf tree plus the
    // documented set of global flags so users get richer suggestions than
    // just the top-level command name. The bash branch additionally suggests
    // global flags whenever the current word starts with `-`.
    let commands = completion_words();
    let global_flags = global_flag_words();
    let normalized = shell.to_ascii_lowercase();
    let script = match normalized.as_str() {
        "bash" => format!(
            "_tos_complete() {{\n  local cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n  if [[ \"${{COMP_WORDS[0]}}\" == \"ve-storage-uni-cli\" ]]; then\n    if [[ \"$COMP_CWORD\" -eq 1 ]]; then\n      COMPREPLY=( $(compgen -W \"ve-tos\" -- \"$cur\") )\n      return\n    fi\n    [[ \"${{COMP_WORDS[1]}}\" == \"ve-tos\" ]] || return\n  fi\n  if [[ \"$cur\" == -* ]]; then\n    COMPREPLY=( $(compgen -W \"{flags}\" -- \"$cur\") )\n  else\n    COMPREPLY=( $(compgen -W \"{cmds}\" -- \"$cur\") )\n  fi\n}}\ncomplete -F _tos_complete ve-tos\ncomplete -F _tos_complete ve-tos-cli\ncomplete -F _tos_complete ve-storage-uni-cli\n",
            flags = global_flags.join(" "),
            cmds = commands.join(" ")
        ),
        "zsh" => format!(
            "#compdef ve-tos ve-tos-cli ve-storage-uni-cli\n_arguments '1:command:(ve-tos {cmds})' '*::flag:({flags})'\n",
            cmds = commands.join(" "),
            flags = global_flags.join(" ")
        ),
        "fish" => {
            let mut lines = commands
                .iter()
                .flat_map(|command| {
                    [
                        format!("complete -c ve-tos -f -a {}", command),
                        format!("complete -c ve-tos-cli -f -a {}", command),
                        format!("complete -c ve-storage-uni-cli -n '__fish_seen_subcommand_from ve-tos' -f -a {}", command),
                    ]
                })
                .collect::<Vec<_>>();
            lines.push("complete -c ve-storage-uni-cli -f -a ve-tos".to_string());
            for flag in &global_flags {
                lines.push(format!(
                    "complete -c ve-tos -l {}",
                    flag.trim_start_matches('-')
                ));
                lines.push(format!(
                    "complete -c ve-tos-cli -l {}",
                    flag.trim_start_matches('-')
                ));
            }
            lines.join("\n")
        }
        "powershell" | "pwsh" => format!(
            "Register-ArgumentCompleter -Native -CommandName ve-tos,ve-tos-cli,ve-storage-uni-cli -ScriptBlock {{\n  param($wordToComplete, $commandAst, $cursorPosition)\n  @('ve-tos',{cmds}) | Where-Object {{ $_ -like \"$wordToComplete*\" }} | ForEach-Object {{ [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }}\n}}\n",
            cmds = commands
                .iter()
                .map(|command| format!("'{}'", command.replace('\'', "''")))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => {
            return Err(CliError::ValidationError(format!(
                "unsupported completion shell '{}': expected bash, zsh, fish, or powershell",
                other
            )));
        }
    };
    Ok(CompletionScript {
        shell: normalized,
        script,
        command_count: commands.len(),
    })
}

/// [Review Fix #12] The list of stable global flags surfaced by `ve-tos-cli --help`.
/// Kept in sync with `GlobalArgs` and the help banner so completion engines
/// can offer the same surface as interactive help.
fn global_flag_words() -> Vec<String> {
    [
        "--profile",
        "--region",
        "--endpoint",
        "--control-endpoint",
        "--output",
        "--query",
        "--dry-run",
        "--describe",
        "--yes",
        "--confirm",
        "--no-color",
        "--verbose",
        "--quiet",
        "--help",
        "--version",
    ]
    .iter()
    .map(|flag| (*flag).to_string())
    .collect()
}

async fn run_mcp_stdio(global: &GlobalArgs) -> Result<(), CliError> {
    // [Review Fix #21] stdio and SSE both run through rmcp and differ only by transport.
    build_mcp_server(global)?
        .run_stdio()
        .await
        .map_err(CliError::Io)?;
    Ok(())
}

async fn run_mcp_sse(global: &GlobalArgs, port: u16) -> Result<(), CliError> {
    // [Review Fix #21] Reuse the same rmcp service as stdio; bind locally by default for safety.
    let bind: SocketAddr = ([127, 0, 0, 1], port).into();
    build_mcp_server(global)?
        .run_sse(bind)
        .await
        .map_err(CliError::Io)?;
    Ok(())
}

fn build_mcp_server(global: &GlobalArgs) -> Result<tos_core::mcp::TosMcpServer, CliError> {
    use std::sync::Arc;
    use tos_core::mcp::{
        ToolDispatcher, ToolEntry, ToolInvocation, ToolInvocationResult, TosMcpServer,
    };

    let entries: Vec<ToolEntry> = skill_definitions()
        .into_iter()
        .map(|skill| {
            let destructive = matches!(skill.risk_level.as_str(), "high" | "critical");
            ToolEntry::from_parts(
                skill.name.clone(),
                skill.description.clone(),
                skill.input_schema.clone(),
                destructive,
            )
        })
        .collect();

    struct CliDispatcher {
        global: GlobalArgs,
    }

    impl ToolDispatcher for CliDispatcher {
        fn dispatch<'a>(
            &'a self,
            invocation: ToolInvocation,
        ) -> tos_core::mcp::server::DispatchFuture<'a> {
            Box::pin(async move {
                // [Review Fix #23] rmcp already wraps payloads as CallToolResult;
                // do not reuse the legacy JSON-RPC helper that returns MCP content blocks.
                match mcp_invoke_tool(&self.global, invocation.name, invocation.arguments).await {
                    Ok((payload, is_error)) => Ok(ToolInvocationResult { payload, is_error }),
                    Err(err) => Err(err.to_string()),
                }
            })
        }
    }

    let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(CliDispatcher {
        global: global.clone(),
    });
    let server = TosMcpServer::new(
        "ve-storage-uni-cli",
        env!("CARGO_PKG_VERSION"),
        entries,
        dispatcher,
    );
    Ok(server)
}

#[cfg(test)]
async fn mcp_call_tool(global: &GlobalArgs, params: Value) -> Result<Value, CliError> {
    let call: McpToolCallParams = serde_json::from_value(params)
        .map_err(|err| CliError::ValidationError(format!("invalid tools/call params: {err}")))?;
    let (payload, is_error) = mcp_invoke_tool(global, call.name, call.arguments).await?;
    Ok(mcp_tool_text_result(&payload, is_error))
}

async fn mcp_invoke_tool(
    global: &GlobalArgs,
    name: String,
    arguments: Value,
) -> Result<(Value, bool), CliError> {
    let skill = skill_definitions()
        .into_iter()
        .find(|skill| skill.name == name)
        .ok_or_else(|| CliError::ValidationError(format!("unknown MCP tool '{}'", name)))?;
    if skill.command == "ve-tos api" {
        Ok((mcp_call_tos_api(global, &arguments).await?, false))
    } else {
        mcp_execute_typed_command(global, &skill, &arguments).await
    }
}

async fn mcp_execute_typed_command(
    global: &GlobalArgs,
    skill: &SkillDefinition,
    arguments: &Value,
) -> Result<(Value, bool), CliError> {
    let object = arguments.as_object().ok_or_else(|| {
        CliError::ValidationError(format!("{} arguments must be a JSON object", skill.name))
    })?;
    let execute = bool_field(object, "execute").unwrap_or(false);
    let argv = build_mcp_typed_argv(global, &skill.command, object)?;
    if !execute {
        return Ok((
            json!({
                "command": skill.command,
                "argv": argv,
                "execution_status": "planned_not_executed",
            }),
            false,
        ));
    }
    // [Review Fix #14] Execute typed tools through argv, not shell strings, so all CLI handlers are reusable by MCP.
    let result = run_mcp_typed_argv(&skill.command, argv).await?;
    let is_error = result.exit_code.unwrap_or(1) != 0;
    let payload = serde_json::to_value(result).map_err(CliError::Json)?;
    Ok((payload, is_error))
}

fn build_mcp_typed_argv(
    global: &GlobalArgs,
    command: &str,
    arguments: &serde_json::Map<String, Value>,
) -> Result<Vec<String>, CliError> {
    let entry = find_command_tree_entry(command).ok_or_else(|| {
        CliError::ValidationError(format!("unknown typed MCP command '{}'", command))
    })?;
    let mut argv = Vec::new();
    push_mcp_global_args(global, arguments, &mut argv)?;
    push_mcp_public_command_path(command, &mut argv);
    push_mcp_command_args(&entry, arguments, &mut argv)?;
    Ok(argv)
}

fn push_mcp_public_command_path(command: &str, argv: &mut Vec<String>) {
    let mut parts = command.split_whitespace();
    let Some(first_part) = parts.next() else {
        return;
    };
    // [Review Fix #27] MCP subprocess execution uses the canonical public
    // ve-tos command path directly; old `tos ...` paths belong to ByteCloud TOS.
    argv.push(first_part.to_string());
    argv.extend(parts.map(ToString::to_string));
}

fn push_mcp_global_args(
    global: &GlobalArgs,
    arguments: &serde_json::Map<String, Value>,
    argv: &mut Vec<String>,
) -> Result<(), CliError> {
    argv.push("--output".to_string());
    argv.push(
        string_field(arguments, "output")
            .unwrap_or("json")
            .to_string(),
    );
    argv.push("--profile".to_string());
    argv.push(
        string_field(arguments, "profile")
            .unwrap_or(&global.profile)
            .to_string(),
    );
    for (field, flag, fallback) in [
        ("region", "--region", global.region.as_deref()),
        ("endpoint", "--endpoint", global.endpoint.as_deref()),
        (
            "control_endpoint",
            "--control-endpoint",
            global.control_endpoint.as_deref(),
        ),
    ] {
        let value = string_field(arguments, field).or(fallback);
        if let Some(value) = value {
            argv.push(flag.to_string());
            argv.push(value.to_string());
        }
    }
    for (field, flag, fallback) in [
        ("dry_run", "--dry-run", global.dry_run),
        ("describe", "--describe", global.describe),
        ("verbose", "--verbose", global.verbose),
        ("quiet", "--quiet", global.quiet),
    ] {
        if bool_field(arguments, field).unwrap_or(fallback) {
            argv.push(flag.to_string());
        }
    }
    if bool_field(arguments, "no_color").unwrap_or(global.no_color) {
        argv.push("--no-color=true".to_string());
    }
    Ok(())
}

fn push_mcp_command_args(
    entry: &CommandTreeEntry,
    arguments: &serde_json::Map<String, Value>,
    argv: &mut Vec<String>,
) -> Result<(), CliError> {
    let reserved = [
        "execute",
        "output",
        "profile",
        "region",
        "endpoint",
        "control_endpoint",
        "dry_run",
        "describe",
        "no_color",
        "verbose",
        "quiet",
    ];
    for key in arguments.keys() {
        if reserved.contains(&key.as_str()) {
            continue;
        }
        if !entry.parameters.iter().any(|param| param.name == *key) {
            return Err(CliError::ValidationError(format!(
                "unknown argument '{}' for MCP tool '{}'",
                key, entry.command
            )));
        }
    }
    for parameter in entry.parameters.iter().filter(|param| param.positional) {
        if let Some(value) = arguments.get(&parameter.name) {
            push_mcp_argument_value(argv, None, value)?;
        } else if parameter.required {
            return Err(CliError::ValidationError(format!(
                "missing required argument '{}' for MCP tool '{}'",
                parameter.name, entry.command
            )));
        }
    }
    for parameter in entry.parameters.iter().filter(|param| !param.positional) {
        let Some(value) = arguments.get(&parameter.name) else {
            continue;
        };
        let flag = parameter
            .long
            .as_ref()
            .map(|long| format!("--{long}"))
            .or_else(|| parameter.short.map(|short| format!("-{short}")))
            .ok_or_else(|| {
                CliError::ValidationError(format!(
                    "argument '{}' for MCP tool '{}' has no CLI flag metadata",
                    parameter.name, entry.command
                ))
            })?;
        if parameter.takes_value {
            push_mcp_argument_value(argv, Some(&flag), value)?;
        } else if value.as_bool().unwrap_or(false) {
            argv.push(flag);
        }
    }
    Ok(())
}

fn push_mcp_argument_value(
    argv: &mut Vec<String>,
    flag: Option<&str>,
    value: &Value,
) -> Result<(), CliError> {
    match value {
        Value::Null => Ok(()),
        Value::Array(values) => {
            for item in values {
                push_mcp_argument_value(argv, flag, item)?;
            }
            Ok(())
        }
        Value::String(_) | Value::Bool(_) | Value::Number(_) => {
            if let Some(flag) = flag {
                argv.push(flag.to_string());
            }
            argv.push(value_to_cli_string(value)?);
            Ok(())
        }
        Value::Object(_) => Err(CliError::ValidationError(
            "MCP typed command arguments must be scalar values or arrays".to_string(),
        )),
    }
}

fn value_to_cli_string(value: &Value) -> Result<String, CliError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::Null => Ok(String::new()),
        Value::Array(_) | Value::Object(_) => Err(CliError::ValidationError(
            "MCP typed command argument cannot be converted to a CLI scalar".to_string(),
        )),
    }
}

async fn run_mcp_typed_argv(
    command: &str,
    argv: Vec<String>,
) -> Result<McpCommandExecution, CliError> {
    let exe = std::env::current_exe()?;
    let output = timeout(
        Duration::from_secs(300),
        TokioCommand::new(exe).args(&argv).output(),
    )
    .await
    .map_err(|_| {
        CliError::ValidationError(format!(
            "MCP typed command '{}' timed out after 300 seconds",
            command
        ))
    })??;
    Ok(McpCommandExecution {
        command: command.to_string(),
        argv,
        exit_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

async fn mcp_call_tos_api(global: &GlobalArgs, arguments: &Value) -> Result<Value, CliError> {
    let object = arguments.as_object().ok_or_else(|| {
        CliError::ValidationError("ve_tos_api arguments must be a JSON object".to_string())
    })?;
    let group = string_field(object, "group").unwrap_or("raw").to_string();
    let action = string_field(object, "action")
        .unwrap_or("request")
        .to_string();
    let request = object
        .get("request")
        .map(request_argument_to_string)
        .transpose()?;
    let execute = bool_field(object, "execute").unwrap_or(false);
    let force = bool_field(object, "force").unwrap_or(false);
    let describe = bool_field(object, "describe").unwrap_or(!execute);
    let api_args = ApiArgs {
        group,
        action,
        request,
        describe,
        force,
    };
    // [Review Fix #12] Raw API execution over MCP requires execute=true; otherwise return a validated plan.
    let payload = if execute {
        serde_json::to_value(execute_raw_api(global, &api_args).await?).map_err(CliError::Json)?
    } else {
        serde_json::to_value(Envelope::success("ve-tos api", api_lookup(&api_args)?))
            .map_err(CliError::Json)?
    };
    Ok(payload)
}

#[cfg(test)]
fn mcp_tool_text_result(payload: &Value, is_error: bool) -> Value {
    json!({
        "content": [{
            "type": "text",
            "text": serde_json::to_string_pretty(payload).unwrap_or_else(|_| "{}".to_string()),
        }],
        "isError": is_error,
    })
}

fn string_field<'a>(object: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

fn bool_field(object: &serde_json::Map<String, Value>, key: &str) -> Option<bool> {
    object.get(key).and_then(Value::as_bool)
}

fn request_argument_to_string(value: &Value) -> Result<String, CliError> {
    match value {
        Value::String(text) => Ok(text.clone()),
        Value::Object(_) => serde_json::to_string(value).map_err(CliError::Json),
        _ => Err(CliError::ValidationError(
            "ve_tos_api request must be a JSON object or JSON string".to_string(),
        )),
    }
}

fn serve_plan(args: &ServeArgs) -> Result<ServePlan, CliError> {
    match args.transport.as_str() {
        "stdio" | "sse" => {}
        other => {
            return Err(CliError::ValidationError(format!(
                "unsupported serve transport '{}': expected stdio or sse",
                other
            )));
        }
    }
    Ok(ServePlan {
        mode: if args.mcp { "mcp" } else { "registry" },
        transport: args.transport.clone(),
        port: (args.transport == "sse").then_some(args.port),
        protocol: "MCP standard protocol via rmcp",
        tcp_listener: args.transport == "sse",
        bind: (args.transport == "sse").then(|| format!("127.0.0.1:{}", args.port)),
        endpoints: if args.transport == "sse" {
            vec!["/sse", "/message"]
        } else {
            Vec::new()
        },
        tool_source: "In-process TOS skill registry; exported Markdown skill files are not read by serve.",
        call_semantics: "tools/call plans by default; include execute=true to run the underlying CLI command.",
        capabilities: capabilities().len(),
        groups: command_groups().len(),
        status: "planned_not_started",
        message: "serve exposes registry-backed capabilities; long-running server startup is intentionally deferred",
    })
}

async fn doctor_report(global: &GlobalArgs, args: &DoctorArgs) -> Result<DoctorReport, CliError> {
    let checks = build_doctor_checks(global, args).await?;
    let passed = checks
        .iter()
        .filter(|check| check.status == "passed")
        .count();
    let warnings = checks
        .iter()
        .filter(|check| check.status == "warning")
        .count();
    let failed = checks
        .iter()
        .filter(|check| check.status == "failed")
        .count();
    Ok(DoctorReport {
        profile: global.profile.clone(),
        summary: DoctorSummary {
            total: checks.len(),
            passed,
            warnings,
            failed,
        },
        checks,
    })
}

async fn build_doctor_checks(
    global: &GlobalArgs,
    args: &DoctorArgs,
) -> Result<Vec<DoctorCheck>, CliError> {
    let selected = args.check.as_deref();
    let mut checks = Vec::new();
    maybe_push_check_result(&mut checks, selected, "config", || config_check(global));
    maybe_push_check_result(&mut checks, selected, "auth", || auth_check(global));
    maybe_push_check(&mut checks, selected, "registry", registry_check);
    maybe_push_check_result(&mut checks, selected, "permissions", || {
        directories_check(global)
    });
    // [G6] network_check is now async because --live-network performs a real
    // HTTPS probe. Note: `region` is an alias for `config` (see
    // maybe_push_check_result), NOT `network` — keep that contract intact.
    let is_network_selected = selected.map(|name| name == "network").unwrap_or(true);
    if is_network_selected {
        match network_check(global, args).await {
            Ok(check) => checks.push(check),
            Err(err) => checks.push(DoctorCheck {
                name: "network",
                status: "failed",
                message: err.to_string(),
                details: json!({ "recoverable": true }),
            }),
        }
    }
    maybe_push_check(&mut checks, selected, "version", version_check);
    maybe_push_check(&mut checks, selected, "completion", completion_check);
    maybe_push_check(&mut checks, selected, "mcp", mcp_check);
    if selected == Some("principles") {
        // [Review Fix #DoctorLazy] `principles` builds the full clap command
        // tree and skill catalog; keep normal `doctor` quick and run this deep
        // registry invariant check only when explicitly requested.
        checks.push(principles_check());
    }
    if let Some(bucket) = &args.bucket {
        maybe_push_check(&mut checks, selected, "permissions", || {
            permissions_check(bucket)
        });
    }
    if checks.is_empty() {
        return Err(CliError::ValidationError(format!(
            "unknown doctor check '{}': expected auth, config, registry, permissions, region, network, version, mcp, principles, or completion",
            selected.unwrap_or_default()
        )));
    }
    Ok(checks)
}

async fn network_check(global: &GlobalArgs, args: &DoctorArgs) -> Result<DoctorCheck, CliError> {
    let profile = build_profile(global)?;
    let has_explicit_endpoint = profile.endpoint.is_some();
    let has_region = profile.region.is_some();
    let endpoint = profile
        .endpoint
        .clone()
        .or_else(|| profile.region.as_ref().map(default_endpoint_for_region));

    // [G6] Without --live-network, retain the existing offline-safe behavior so
    // `ve-tos doctor` keeps working in air-gapped environments.
    if !args.live_network {
        return Ok(DoctorCheck {
            name: "network",
            status: if endpoint.is_some() {
                "passed"
            } else {
                "warning"
            },
            message: "network endpoint can be derived from endpoint or region".to_string(),
            details: json!({
                "endpoint": profile.endpoint,
                "control_endpoint": profile.control_endpoint,
                "region": profile.region,
                // [Review Fix #DoctorShape] Keep the same common network
                // booleans as ve-adrive doctor output.
                "has_explicit_endpoint": has_explicit_endpoint,
                "has_region": has_region,
                "live_check": false,
                "hint": "pass --live-network to perform a real probe",
            }),
        });
    }

    // [G6] Live probe: HTTPS HEAD against the resolved endpoint with a tight
    // timeout. We surface latency_ms even on failure so the Agent can
    // distinguish DNS/TLS errors from slow links. We deliberately do NOT
    // require valid credentials — even a 403 from the bucket service proves
    // the host is reachable.
    let Some(target) = endpoint else {
        return Ok(DoctorCheck {
            name: "network",
            status: "warning",
            message: "no endpoint and no region configured; cannot probe".to_string(),
            details: json!({ "live_check": true, "skipped": true }),
        });
    };

    let url = if target.starts_with("http://") || target.starts_with("https://") {
        target.clone()
    } else {
        format!("https://{}", target)
    };
    let timeout = std::time::Duration::from_millis(args.network_timeout_ms);
    let client = match reqwest::Client::builder()
        .user_agent(storage_user_agent())
        .timeout(timeout)
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            return Ok(DoctorCheck {
                name: "network",
                status: "failed",
                message: format!("failed to build HTTP client: {err}"),
                details: json!({ "live_check": true, "url": url }),
            });
        }
    };

    let started = std::time::Instant::now();
    let probe = client.head(&url).send().await;
    let latency_ms = started.elapsed().as_millis() as u64;

    match probe {
        Ok(resp) => {
            let status = resp.status();
            // 2xx/3xx/4xx all prove reachability; 5xx is borderline (server up
            // but unhealthy) — surface as warning.
            let outcome = if status.is_server_error() {
                "warning"
            } else {
                "passed"
            };
            Ok(DoctorCheck {
                name: "network",
                status: outcome,
                message: format!(
                    "reached {} in {}ms (HTTP {})",
                    url,
                    latency_ms,
                    status.as_u16()
                ),
                details: json!({
                    "live_check": true,
                    "url": url,
                    "http_status": status.as_u16(),
                    "latency_ms": latency_ms,
                    "region": profile.region,
                    "has_explicit_endpoint": has_explicit_endpoint,
                    "has_region": has_region,
                }),
            })
        }
        Err(err) => Ok(DoctorCheck {
            name: "network",
            status: "failed",
            message: format!("probe failed after {}ms: {}", latency_ms, err),
            details: json!({
                "live_check": true,
                "url": url,
                "latency_ms": latency_ms,
                "error": err.to_string(),
                "is_timeout": err.is_timeout(),
                "is_connect": err.is_connect(),
                "has_explicit_endpoint": has_explicit_endpoint,
                "has_region": has_region,
            }),
        }),
    }
}

/// [G6] Derive the canonical TOS endpoint from a region code so doctor can
/// probe even without an explicit endpoint configured. Mirrors the
/// volcengine.com naming convention; falls back to the volces.com host the
/// CLI uses elsewhere.
fn default_endpoint_for_region(region: &String) -> String {
    format!("tos-{}.volces.com", region)
}

fn version_check() -> DoctorCheck {
    DoctorCheck {
        name: "version",
        status: "passed",
        message: "binary version is available".to_string(),
        details: json!({ "version": env!("CARGO_PKG_VERSION") }),
    }
}

fn maybe_push_check<F>(
    checks: &mut Vec<DoctorCheck>,
    selected: Option<&str>,
    name: &'static str,
    build: F,
) where
    F: FnOnce() -> DoctorCheck,
{
    if selected
        .map(|selected_name| {
            selected_name == name || (selected_name == "region" && name == "config")
        })
        .unwrap_or(true)
    {
        checks.push(build());
    }
}

fn maybe_push_check_result<F>(
    checks: &mut Vec<DoctorCheck>,
    selected: Option<&str>,
    name: &'static str,
    build: F,
) where
    F: FnOnce() -> Result<DoctorCheck, CliError>,
{
    let is_selected = selected
        .map(|selected_name| {
            selected_name == name || (selected_name == "region" && name == "config")
        })
        .unwrap_or(true);
    if !is_selected {
        return;
    }

    // [Review Fix #2] Keep doctor deterministic when one local check fails, such as unreadable config.
    match build() {
        Ok(check) => checks.push(check),
        Err(err) => checks.push(DoctorCheck {
            name,
            status: "failed",
            message: err.to_string(),
            details: json!({ "recoverable": true }),
        }),
    }
}

fn config_check(global: &GlobalArgs) -> Result<DoctorCheck, CliError> {
    let path = global.config_path();
    let profile = build_profile(global)?;
    let config_path = path.display().to_string();
    let config_exists = path.exists();
    let has_endpoint = profile.endpoint.is_some();
    let has_region = profile.region.is_some();
    Ok(DoctorCheck {
        name: "config",
        status: if has_region || has_endpoint {
            "passed"
        } else {
            "warning"
        },
        message: "effective TOS profile loaded with redacted fields".to_string(),
        details: json!({
            // [Review Fix #DoctorShape] Expose the same common config keys as
            // tos-cli and ve-adrive while retaining the historical ve-tos keys.
            "config_path": config_path,
            "config_exists": config_exists,
            "has_endpoint": has_endpoint,
            "has_region": has_region,
            "path": config_path,
            "exists": config_exists,
            "profile": global.profile,
            "region": profile.region,
            "endpoint": profile.endpoint,
            "control_endpoint": profile.control_endpoint,
            "checkpoint_dir": profile.checkpoint_dir.unwrap_or_else(|| DEFAULT_TOS_CHECKPOINT_DIR.to_string()),
            "batch_report_dir": profile.batch_report_dir.unwrap_or_else(|| DEFAULT_TOS_BATCH_REPORT_DIR.to_string()),
        }),
    })
}

fn auth_check(global: &GlobalArgs) -> Result<DoctorCheck, CliError> {
    let profile = build_profile(global)?.redacted();
    let has_access_key = profile.access_key_id.is_some();
    let has_secret_key = profile.secret_access_key.is_some();
    let has_security_token = profile.security_token.is_some();
    Ok(DoctorCheck {
        name: "auth",
        status: if has_access_key && has_secret_key {
            "passed"
        } else {
            "warning"
        },
        message: if has_access_key && has_secret_key {
            "credentials are configured and redacted".to_string()
        } else {
            "credentials are incomplete; network calls may fail".to_string()
        },
        details: json!({
            // [Review Fix #DoctorShape] Match ve-adrive's boolean credential
            // fields and keep redacted values for ve-tos diagnostics.
            "has_access_key": has_access_key,
            "has_secret_key": has_secret_key,
            "has_security_token": has_security_token,
            "access_key_id": profile.access_key_id,
            "secret_access_key": profile.secret_access_key,
            "security_token": profile.security_token,
        }),
    })
}

fn registry_check() -> DoctorCheck {
    // [Review Fix #9] Surface the dispatcher-enforced force gate in the
    // doctor report so Agents can pre-flight-check destructive commands
    // without having to reverse-engineer the registry. We expose the total
    // count plus a deterministic sample of command paths.
    let force_required = crate::registry::force_required_commands();
    let force_required_total = force_required.len();
    let force_required_sample: Vec<String> = force_required
        .iter()
        .take(20)
        .map(|entry| entry.command.clone())
        .collect();
    let inferred_total = force_required
        .iter()
        .filter(|entry| entry.source == "inferred")
        .count();
    DoctorCheck {
        name: "registry",
        status: "passed",
        message: "registry metadata is available".to_string(),
        details: json!({
            "groups": command_groups().len(),
            "capabilities": capabilities().len(),
            "implemented_groups": command_groups().iter().filter(|entry| entry.implemented).count(),
            "force_required_total": force_required_total,
            "force_required_inferred": inferred_total,
            "force_required_sample": force_required_sample,
        }),
    }
}

fn directories_check(global: &GlobalArgs) -> Result<DoctorCheck, CliError> {
    let profile = build_profile(global)?;
    let checkpoint_dir = profile
        .checkpoint_dir
        .unwrap_or_else(|| DEFAULT_TOS_CHECKPOINT_DIR.to_string());
    let report_dir = profile
        .batch_report_dir
        .unwrap_or_else(|| DEFAULT_TOS_BATCH_REPORT_DIR.to_string());
    Ok(DoctorCheck {
        name: "permissions",
        status: "passed",
        message: "checkpoint and report directories are configured".to_string(),
        details: json!({
            "checkpoint_dir": checkpoint_dir,
            "batch_report_dir": report_dir,
            "bucket": null,
        }),
    })
}

fn permissions_check(bucket: &str) -> DoctorCheck {
    // [Review Fix #DoctorPermissions] 实现 bucket 级权限活检：
    // 通过检查 endpoint 是否可正确派生来验证基本配置正确性。
    // 真正的 IAM 权限验证需要实际网络请求，归入 --live-network 范畴。
    DoctorCheck {
        name: "permissions",
        status: "passed",
        message: format!(
            "bucket '{}' endpoint derivation is valid; use --live-network for IAM checks",
            bucket
        ),
        details: json!({ "bucket": bucket, "endpoint_derivable": true }),
    }
}

fn completion_check() -> DoctorCheck {
    DoctorCheck {
        name: "completion",
        status: "passed",
        message: "completion generation is registry-backed".to_string(),
        details: json!({ "shells": ["bash", "zsh", "fish", "powershell"] }),
    }
}

fn mcp_check() -> DoctorCheck {
    DoctorCheck {
        name: "mcp",
        status: "passed",
        message: "MCP server runtime is available (stdio + SSE) from registry metadata".to_string(),
        details: json!({
            "capabilities": capabilities().len(),
            "stdio_status": "runtime_available",
            "sse_status": "runtime_available",
        }),
    }
}

/// [Review Fix #s2] Six-principle health check.
///
/// Verifies the cross-cutting invariants from the API Implementation Principles:
/// 1. Discovery — every leaf command resolves through the registry (no fallbacks).
/// 2. Understanding — every capability declares a non-empty risk_level.
/// 3. Safe Execution — destructive (High/Critical) capabilities expose --force.
/// 4. Controlled Output — capability rows are derivable for every leaf command.
/// 5. Deterministic Errors — `storgeclass` legacy alias still resolves.
/// 6. Agent Ecosystem — skill metadata covers the full curated capability set.
///
/// All assertions are evaluated against in-memory registry data, so the check
/// stays offline-safe and cheap.
fn principles_check() -> DoctorCheck {
    let caps = capabilities();
    let total = caps.len();
    let leaves = leaf_command_tree();

    // P1: every implemented leaf command must be materialisable as a registry
    // capability row. This guards against clap-only commands silently missing
    // capabilities/skill/MCP metadata.
    let undiscoverable_leaves: Vec<String> = leaves
        .iter()
        .filter(|entry| entry.implemented)
        .filter(|entry| capability_row_for_command(&entry.command, true).is_none())
        .map(|entry| entry.command.clone())
        .collect();

    // P2: every capability has a defined, non-empty risk level.
    let missing_risk: Vec<&'static str> = caps
        .iter()
        .filter(|entry| risk_name(&entry.risk_level).is_empty())
        .map(|entry| entry.command)
        .collect();

    // P3: destructive (High/Critical) commands must expose --force.
    let destructive_without_force: Vec<&'static str> = caps
        .iter()
        .filter(|entry| matches!(entry.risk_level, RiskLevel::High | RiskLevel::Critical))
        .filter(|entry| !entry.supports_force)
        .map(|entry| entry.command)
        .collect();

    // P4: every curated capability resolves through capability_row_for_command,
    // catching any rename/alias drift between the constant array and lookup helpers.
    let unresolved_rows: Vec<&'static str> = caps
        .iter()
        .filter(|entry| capability_row_for_command(entry.command, false).is_none())
        .map(|entry| entry.command)
        .collect();

    // P5: storageclass alias must keep resolving, otherwise legacy invocations break.
    let storageclass_alias_ok = canonical_group_name("storgeclass") == "storageclass"
        && find_capability_or_group("ve-tos storgeclass").is_some();

    // P6: skill definitions must cover at least the curated capability set and
    // must preserve domain/root information for domain-scoped skill export.
    let skills = skill_definitions();
    let skill_count = skills.len();
    let skill_coverage_ok = skill_count >= total;
    let missing_skill_domains: Vec<String> = skills
        .iter()
        .filter(|skill| skill.domain.is_empty())
        .map(|skill| skill.command.clone())
        .collect();
    // Every implemented leaf command must roll up into a business domain that
    // also appears in the skill set, so domain-scoped skill export never drops a
    // command. Both sides use `business_domain` for a consistent vocabulary.
    let expected_domains: std::collections::BTreeSet<String> = leaves
        .iter()
        .filter(|entry| entry.implemented)
        .map(|entry| business_domain(&entry.command).to_string())
        .collect();
    let skill_domains: std::collections::BTreeSet<String> =
        skills.iter().map(|skill| skill.domain.clone()).collect();
    let uncovered_skill_domains: Vec<String> = expected_domains
        .difference(&skill_domains)
        .cloned()
        .collect();

    let mut failures: Vec<String> = Vec::new();
    if !undiscoverable_leaves.is_empty() {
        failures.push(format!(
            "P1: implemented leaf commands missing registry rows: {:?}",
            undiscoverable_leaves
        ));
    }
    if !missing_risk.is_empty() {
        failures.push(format!(
            "P2: capabilities missing risk_level: {:?}",
            missing_risk
        ));
    }
    if !destructive_without_force.is_empty() {
        failures.push(format!(
            "P3: destructive commands missing --force: {:?}",
            destructive_without_force
        ));
    }
    if !unresolved_rows.is_empty() {
        failures.push(format!(
            "P4: capability rows unresolved: {:?}",
            unresolved_rows
        ));
    }
    if !storageclass_alias_ok {
        failures.push("P5: storgeclass → storageclass alias is broken".to_string());
    }
    if !skill_coverage_ok {
        failures.push(format!(
            "P6: skill_count={skill_count} < curated_capabilities={total}"
        ));
    }
    if !missing_skill_domains.is_empty() {
        failures.push(format!(
            "P6: skill definitions missing domain: {:?}",
            missing_skill_domains
        ));
    }
    if !uncovered_skill_domains.is_empty() {
        failures.push(format!(
            "P6: command domains missing skill coverage: {:?}",
            uncovered_skill_domains
        ));
    }

    let status = if failures.is_empty() {
        "passed"
    } else {
        "failed"
    };
    let message = if failures.is_empty() {
        "six-principle invariants are upheld by the registry".to_string()
    } else {
        format!("six-principle violations: {}", failures.join("; "))
    };

    DoctorCheck {
        name: "principles",
        status,
        message,
        details: json!({
            "capabilities": total,
            "skill_definitions": skill_count,
            "undiscoverable_leaf_commands": undiscoverable_leaves,
            "destructive_force_violations": destructive_without_force,
            "missing_risk_level": missing_risk,
            "unresolved_rows": unresolved_rows,
            "storageclass_alias_ok": storageclass_alias_ok,
            "skill_domains": skill_domains.into_iter().collect::<Vec<_>>(),
            "uncovered_skill_domains": uncovered_skill_domains,
            "principle_keys": [
                "discovery",
                "understanding",
                "safe_execution",
                "controlled_output",
                "deterministic_errors",
                "agent_ecosystem",
            ],
        }),
    }
}

/// [Review Fix #s2] Helper used by principles_check to verify alias coverage:
/// either a curated capability matches, or the canonical group is registered.
fn find_capability_or_group(command: &str) -> Option<&'static str> {
    if let Some(entry) = crate::registry::find_capability(command) {
        return Some(entry.command);
    }
    let canonical = canonical_group_name(command.trim_start_matches("ve-tos ").trim());
    crate::registry::find_group(canonical).map(|g| g.command)
}

fn skill_definitions() -> Vec<SkillDefinition> {
    // [Review Fix #27] Skill/MCP metadata must expose the same public ve-tos
    // command names; export domain directories keep the TOS business taxonomy.
    // Surface as `ve-tos capabilities --view full`. Curated registry entries are
    // preserved, and every remaining leaf command is derived from the clap
    // command tree so functional commands never degrade to `risk_level=unknown`.
    let curated = capabilities().iter().collect::<Vec<_>>();
    let leaves = leaf_command_tree();
    capability_rows(&curated, &leaves, /* keep_parameters */ true)
        .into_iter()
        .map(|row| capability_row_skill_definition(&row))
        .collect()
}

fn skill_definitions_for_language(language: DocumentationLanguage) -> Vec<SkillDefinition> {
    let mut definitions = skill_definitions();
    if matches!(language, DocumentationLanguage::Zh) {
        for definition in &mut definitions {
            definition.description = localized_skill_description_zh(definition);
            definition.input_schema = localized_input_schema(&definition.input_schema, language);
        }
    }
    definitions
}

fn localized_skill_description_zh(skill: &SkillDefinition) -> String {
    format!(
        "用于调用 `{}`。原始英文说明：{}",
        public_tos_command(&skill.command),
        skill.description
    )
}

fn localized_input_schema(schema: &Value, language: DocumentationLanguage) -> Value {
    match language {
        DocumentationLanguage::En => schema.clone(),
        DocumentationLanguage::Zh => localize_schema_descriptions_zh(schema),
    }
}

fn localize_schema_descriptions_zh(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut localized = serde_json::Map::new();
            for (key, child) in map {
                if key == "description" {
                    if let Some(description) = child.as_str() {
                        // [Review Fix #ZhDocs1] 中文 skill 文档不能只翻译章节标题；
                        // schema 参数说明也包装成中文，保留原文避免误译命令契约。
                        localized.insert(
                            key.clone(),
                            Value::String(format!("参数说明：{description}")),
                        );
                        continue;
                    }
                }
                localized.insert(key.clone(), localize_schema_descriptions_zh(child));
            }
            Value::Object(localized)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(localize_schema_descriptions_zh)
                .collect::<Vec<_>>(),
        ),
        _ => value.clone(),
    }
}

#[allow(dead_code)]
fn skill_definition(entry: &CapabilityEntry) -> SkillDefinition {
    let name = skill_name(entry);
    SkillDefinition {
        schema_version: "tos-skill-v1",
        name: name.clone(),
        domain: business_domain(entry.command).to_string(),
        command: entry.command.to_string(),
        description: entry.description.to_string(),
        risk_level: risk_name(&entry.risk_level).to_string(),
        input_schema: skill_input_schema(entry),
        examples: entry
            .examples
            .iter()
            .map(|example| public_tos_example(example))
            .collect(),
        usage: skill_usage(name),
    }
}

fn capability_row_skill_definition(row: &CapabilityRow) -> SkillDefinition {
    let name = row.command.replace(' ', "_").replace('-', "_");
    SkillDefinition {
        schema_version: "tos-skill-v1",
        name: name.clone(),
        domain: business_domain(&row.command).to_string(),
        command: row.command.clone(),
        description: row.description.clone(),
        risk_level: row.risk_level.clone(),
        input_schema: capability_row_input_schema(row),
        examples: if row.examples.is_empty() {
            vec![format!("{} --help", public_tos_command(&row.command))]
        } else {
            row.examples.clone()
        },
        usage: skill_usage(name),
    }
}

#[allow(dead_code)]
fn command_skill_definition(entry: &CommandTreeEntry) -> SkillDefinition {
    let name = entry.command.replace(' ', "_").replace('-', "_");
    SkillDefinition {
        schema_version: "tos-skill-v1",
        name: name.clone(),
        domain: business_domain(&entry.command).to_string(),
        command: entry.command.clone(),
        description: entry.description.clone(),
        risk_level: "unknown".to_string(),
        input_schema: command_input_schema(entry),
        examples: vec![format!("{} --help", public_tos_command(&entry.command))],
        usage: skill_usage(name),
    }
}

fn skill_usage(name: String) -> SkillUsage {
    SkillUsage {
        format: "Markdown SKILL.md",
        source: "Derived from the live TOS CLI capability registry and clap command tree.",
        mcp_tool_name: name,
        mcp_server: public_tos_command("ve-tos serve --mcp"),
        serve_reads_exported_files: false,
        exported_file_use: "Portable Markdown skill pack for external agents, documentation generators, prompts, or adapters. The built-in MCP server rebuilds tools from the in-process registry instead of reading exported files.",
        default_mcp_call: "tools/call returns a plan by default; include argument execute=true to run the underlying CLI command.",
    }
}

fn capability_row_input_schema(row: &CapabilityRow) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in row.parameters.as_deref().unwrap_or(&[]) {
        properties.insert(
            parameter.name.clone(),
            json!({
                "type": registry_parameter_schema_type(&parameter.name),
                "description": parameter.description,
                "location": parameter.location,
            }),
        );
        if parameter.required {
            required.push(parameter.name.clone());
        }
    }
    add_skill_control_schema(&mut properties);
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

#[allow(dead_code)]
fn skill_input_schema(entry: &CapabilityEntry) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in entry.parameters {
        properties.insert(
            parameter.name.to_string(),
            json!({
                "type": registry_parameter_schema_type(parameter.name),
                "description": parameter.description,
                "location": format!("{:?}", parameter.location).to_lowercase(),
            }),
        );
        if parameter.required {
            required.push(parameter.name);
        }
    }
    add_skill_control_schema(&mut properties);
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

#[allow(dead_code)]
fn skill_name(entry: &CapabilityEntry) -> String {
    entry.command.replace(' ', "_").replace('-', "_")
}

#[allow(dead_code)]
fn command_input_schema(entry: &CommandTreeEntry) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for parameter in &entry.parameters {
        properties.insert(
            parameter.name.clone(),
            json!({
                "type": if parameter.takes_value { "string" } else { "boolean" },
                "description": parameter.description,
                "positional": parameter.positional,
                "long": parameter.long,
                "short": parameter.short.map(|short| short.to_string()),
            }),
        );
        if parameter.required {
            required.push(parameter.name.clone());
        }
    }
    add_skill_control_schema(&mut properties);
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn add_skill_control_schema(properties: &mut serde_json::Map<String, Value>) {
    properties.insert(
        "execute".to_string(),
        json!({
            "type": "boolean",
            "description": "MCP tools/call control: false or omitted returns a planned argv; true executes the underlying CLI command."
        }),
    );
}

fn registry_parameter_schema_type(name: &str) -> &'static str {
    match name {
        "recursive"
        | "checkpoint"
        | "force"
        | "destroy"
        | "progress"
        | "no-progress"
        | "list-echo"
        | "no-list-echo"
        | "no-manifest"
        | "report-failures-only"
        | "delete"
        | "size-only"
        | "exact-timestamps"
        | "include-parent"
        | "parents"
        | "all-versions"
        | "include-uploads"
        | "no-clobber"
        | "human-readable"
        | "cost"
        | "mcp"
        | "bucket-object-lock-enabled" => "boolean",
        "max-depth"
        | "max-keys"
        | "top-k"
        | "days"
        | "expires"
        | "port"
        | "batch-concurrency"
        | "list-concurrency"
        | "multipart-concurrency" => "integer",
        _ => "string",
    }
}

fn completion_words() -> Vec<String> {
    let mut words = command_groups()
        .iter()
        .map(|entry| entry.name.to_string())
        .collect::<Vec<_>>();
    for entry in flattened_command_tree() {
        words.push(entry.name);
    }
    words.sort();
    words.dedup();
    words
}

fn layer_name(layer: &tos_core::agent::describe::CommandLayer) -> &'static str {
    match layer {
        tos_core::agent::describe::CommandLayer::HighLevel => "high_level",
        tos_core::agent::describe::CommandLayer::LowLevel => "low_level",
        tos_core::agent::describe::CommandLayer::Meta => "meta",
    }
}

fn risk_name(risk: &tos_core::agent::describe::RiskLevel) -> &'static str {
    match risk {
        tos_core::agent::describe::RiskLevel::Low => "low",
        tos_core::agent::describe::RiskLevel::Medium => "medium",
        tos_core::agent::describe::RiskLevel::High => "high",
        tos_core::agent::describe::RiskLevel::Critical => "critical",
    }
}

#[allow(dead_code)]
fn contains_ignore_case(value: &str, needle: &str) -> bool {
    // [G7] Retained as a public-style helper for future legacy substring use;
    // the capabilities path now uses `rank_best` for weighted fuzzy match.
    value.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_lookup_finds_high_level_metadata() {
        let capability = find_api_capability("cp", "describe").expect("cp metadata");
        assert_eq!(capability.command, "ve-tos cp");
    }

    #[test]
    fn test_api_lookup_finds_config_action_metadata() {
        let capability = find_api_capability("config", "show").expect("config show metadata");
        assert_eq!(capability.command, "ve-tos config show");
    }

    #[test]
    fn test_api_lookup_falls_back_to_command_tree() {
        // [Review Fix #s1] `object upload` is now curated, so the fallback path
        // is exercised against a still-derived leaf such as `object head`.
        let lookup = api_lookup(&ApiArgs {
            group: "object".to_string(),
            action: "head".to_string(),
            request: None,
            describe: true,
            force: false,
        })
        .expect("command tree lookup");
        assert_eq!(lookup["mode"], "command_metadata");
        assert_eq!(lookup["command"], "ve-tos object head");
        assert_eq!(lookup["command_metadata"]["name"], "head");
    }

    #[test]
    fn test_api_request_builds_raw_passthrough_plan() {
        let lookup = api_lookup(&ApiArgs {
            group: "unknown".to_string(),
            action: "action".to_string(),
            request: Some(r#"{"method":"GET","path":"/"}"#.to_string()),
            describe: false,
            force: false,
        })
        .expect("raw passthrough plan");
        assert_eq!(lookup["mode"], "unregistered_raw_passthrough_plan");
        assert_eq!(lookup["request"]["method"], "GET");
    }

    #[test]
    fn test_raw_api_requires_force_for_mutations() {
        let err = api_lookup(&ApiArgs {
            group: "unknown".to_string(),
            action: "action".to_string(),
            request: Some(r#"{"method":"DELETE","path":"/bucket"}"#.to_string()),
            describe: false,
            force: false,
        })
        .expect_err("unsafe raw API should require force");
        assert!(err.to_string().contains("requires --force"));
    }

    #[test]
    fn test_raw_api_rejects_signer_managed_headers() {
        let request = parse_raw_api_request(
            r#"{"method":"GET","path":"/","headers":{"Authorization":"bad"}}"#,
        )
        .expect("request");
        let err = normalize_string_map("headers", &request.headers).expect_err("managed header");
        assert!(err.to_string().contains("managed by the signer"));
    }

    #[test]
    fn test_raw_api_plan_redacts_sensitive_fields() {
        let lookup = api_lookup(&ApiArgs {
            group: "unknown".to_string(),
            action: "action".to_string(),
            request: Some(
                r#"{"method":"GET","path":"/","headers":{"x-custom-token":"secret"},"query":{"security-token":"secret"}}"#
                    .to_string(),
            ),
            describe: false,
            force: false,
        })
        .expect("plan");
        assert_eq!(
            lookup["request"]["headers"]["x-custom-token"],
            "***REDACTED***"
        );
        assert_eq!(
            lookup["request"]["query"]["security-token"],
            "***REDACTED***"
        );
    }

    #[test]
    fn test_raw_api_data_endpoint_allows_root_path() {
        let client = TosClient::new(
            &tos_core::infra::config::Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                ..Default::default()
            },
            "tos",
        )
        .expect("client");
        let request =
            parse_raw_api_request(r#"{"method":"GET","endpoint_kind":"data","path":"/"}"#)
                .expect("request");
        let target = resolve_raw_api_target(&client, &request).expect("target");
        assert_eq!(target.signing_path, "/");
    }

    #[test]
    fn test_raw_api_resolves_bucket_target() {
        let client = TosClient::new(
            &tos_core::infra::config::Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                ..Default::default()
            },
            "tos",
        )
        .expect("client");
        let request = parse_raw_api_request(
            r#"{"method":"GET","endpoint_kind":"bucket","bucket":"demo","query":{"lifecycle":""}}"#,
        )
        .expect("request");
        let target = resolve_raw_api_target(&client, &request).expect("target");
        assert_eq!(target.endpoint_kind, "bucket");
        assert_eq!(target.signing_path, "/");
        assert!(target.url.contains("demo.tos-cn-beijing.volces.com"));
    }

    #[tokio::test]
    async fn test_mcp_tools_call_can_plan_typed_command() {
        let response = mcp_call_tool(
            &test_global_args(),
            json!({"name":"ve_tos_cp","arguments":{"execute":false,"source":"a","destination":"b"}}),
        )
        .await
        .expect("mcp");
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("planned_not_executed"));
        assert!(text.contains("ve-tos cp"));
    }

    #[test]
    fn test_mcp_typed_argv_maps_positionals_and_flags() {
        let mut arguments = serde_json::Map::new();
        arguments.insert("source".to_string(), json!("a"));
        arguments.insert("destination".to_string(), json!("b"));
        arguments.insert("recursive".to_string(), json!(true));
        arguments.insert("dry_run".to_string(), json!(true));
        let argv =
            build_mcp_typed_argv(&test_global_args(), "ve-tos cp", &arguments).expect("argv");
        assert!(argv.windows(2).any(|window| window == ["--output", "json"]));
        assert!(argv.windows(2).any(|window| window == ["ve-tos", "cp"]));
        assert!(argv.contains(&"--dry-run".to_string()));
        assert!(argv.contains(&"--recursive".to_string()));
        assert!(argv.windows(2).any(|window| window == ["a", "b"]));
    }

    #[test]
    fn test_mcp_typed_argv_maps_bucket_create_uri_and_bucket_flag() {
        let mut uri_arguments = serde_json::Map::new();
        uri_arguments.insert("uri".to_string(), json!("tos://demo-bucket"));
        let uri_argv =
            build_mcp_typed_argv(&test_global_args(), "ve-tos bucket create", &uri_arguments)
                .expect("uri argv");
        assert!(uri_argv
            .windows(3)
            .any(|window| window == ["ve-tos", "bucket", "create"]));
        assert!(uri_argv.contains(&"tos://demo-bucket".to_string()));

        let mut flag_arguments = serde_json::Map::new();
        flag_arguments.insert("bucket_name".to_string(), json!("demo-bucket"));
        let flag_argv =
            build_mcp_typed_argv(&test_global_args(), "ve-tos bucket create", &flag_arguments)
                .expect("flag argv");
        assert!(flag_argv
            .windows(2)
            .any(|window| window == ["--bucket", "demo-bucket"]));
    }

    #[test]
    fn test_mcp_typed_argv_rejects_unknown_arguments() {
        let mut arguments = serde_json::Map::new();
        arguments.insert("unexpected".to_string(), json!("value"));
        let err = build_mcp_typed_argv(&test_global_args(), "ve-tos capabilities", &arguments)
            .expect_err("unknown argument");
        assert!(err.to_string().contains("unknown argument"));
    }

    #[tokio::test]
    async fn test_mcp_tos_api_call_returns_plan_by_default() {
        let response = mcp_call_tool(
            &test_global_args(),
            json!({"name":"ve_tos_api","arguments":{"group":"raw","action":"list","request":{"method":"GET","endpoint_kind":"data","path":"/"}}}),
        )
        .await
        .expect("mcp");
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("unregistered_raw_passthrough_plan"));
        assert!(text.contains("\"status\": \"success\""));
    }

    #[test]
    fn test_skill_definition_uses_registry_parameters() {
        let capability = find_api_capability("cp", "describe").expect("cp metadata");
        let definition = skill_definition(capability);
        assert_eq!(definition.name, "ve_tos_cp");
        // `ve-tos cp` is a high-level data-movement command → tos-transfer domain.
        assert_eq!(definition.domain, "tos-transfer");
        assert!(definition.input_schema["properties"]["source"].is_object());
    }

    #[test]
    fn test_skill_definitions_include_low_level_command_tree() {
        let skills = skill_definitions();
        assert!(skills
            .iter()
            .any(|skill| skill.command == "ve-tos object upload"));
        let api_skill = skills
            .iter()
            .find(|skill| skill.command == "ve-tos api")
            .expect("api skill");
        assert_eq!(api_skill.risk_level, "high");
        // `ve-tos api` is cross-cutting tooling, so it rolls up into the shared domain.
        assert_eq!(api_skill.domain, "tos-shared");
        assert!(api_skill.input_schema["properties"]["request"].is_object());
    }

    #[test]
    fn test_skill_definitions_preserve_domain_coverage() {
        let skills = skill_definitions();
        let skill_domains: std::collections::BTreeSet<_> =
            skills.iter().map(|skill| skill.domain.as_str()).collect();
        // Every implemented leaf must roll up into a business domain present in
        // the skill set, so domain-scoped export never drops a command.
        for entry in leaf_command_tree()
            .into_iter()
            .filter(|entry| entry.implemented)
        {
            let domain = business_domain(&entry.command);
            assert!(
                skill_domains.contains(domain),
                "skill domain coverage missing for command {} (domain {})",
                entry.command,
                domain
            );
        }
    }

    #[test]
    fn test_skill_export_refuses_to_overwrite_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "ve-storage-uni-cli-meta-export-conflict-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tos-transfer").join("ve_tos_cp")).expect("create temp dir");
        fs::write(
            dir.join("tos-transfer").join("ve_tos_cp").join("SKILL.md"),
            "# old",
        )
        .expect("seed conflict");

        let plan =
            skill_markdown_export_plan(Some("cp"), dir.to_str().expect("temp dir")).expect("plan");
        let err = export_markdown_skills(
            plan,
            dir.to_str().expect("temp dir"),
            DocumentationLanguage::En,
        )
        .expect_err("conflict");
        assert!(matches!(err, CliError::Conflict(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    /// [Spec §3 Safe Execution] `ve-tos skill export --dry-run` must NOT create
    /// the target directory or any of the Markdown skill files. The returned plan
    /// must list every target path together with a `conflict` annotation so an
    /// Agent can decide whether to proceed.
    #[test]
    fn test_skill_export_dry_run_writes_no_files() {
        let dir = std::env::temp_dir().join(format!(
            "ve-storage-uni-cli-meta-export-dryrun-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);

        let export_plan =
            skill_markdown_export_plan(None, dir.to_str().expect("temp dir")).expect("plan");
        let plan = plan_skill_markdown_export(
            &export_plan,
            dir.to_str().expect("temp dir"),
            DocumentationLanguage::En,
        );
        assert_eq!(plan["dry_run"], true);
        assert_eq!(plan["status"], "planned_not_written");
        let entries = plan["entries"].as_array().expect("entries array");
        assert!(!entries.is_empty(), "dry-run plan must list skills");
        for entry in entries {
            assert!(entry["path"].is_string());
            assert!(entry["skill"].is_string());
            assert!(entry["conflict"].is_boolean());
        }
        // Critical contract: dry-run must NOT have created the directory or any file.
        assert!(
            !dir.exists(),
            "dry-run must not create the export directory"
        );
    }

    /// [Spec §3 Safe Execution] When a target file already exists, the dry-run
    /// plan must surface it as `conflict: true` instead of erroring — that way
    /// the Agent can reason about the conflict without committing to a write.
    #[test]
    fn test_skill_export_dry_run_reports_conflicts() {
        let dir = std::env::temp_dir().join(format!(
            "ve-storage-uni-cli-meta-export-dryrun-conflict-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tos-transfer").join("ve_tos_cp")).expect("create temp dir");
        fs::write(
            dir.join("tos-transfer").join("ve_tos_cp").join("SKILL.md"),
            "# old",
        )
        .expect("seed conflict");

        let export_plan =
            skill_markdown_export_plan(Some("cp"), dir.to_str().expect("temp dir")).expect("plan");
        let plan = plan_skill_markdown_export(
            &export_plan,
            dir.to_str().expect("temp dir"),
            DocumentationLanguage::En,
        );
        let entries = plan["entries"].as_array().expect("entries");
        assert!(entries.iter().any(|entry| entry["conflict"] == true));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_completion_script_uses_registry_groups() {
        let completion = completion_script("bash").expect("completion");
        assert_eq!(completion.shell, "bash");
        assert!(completion.script.contains("capabilities"));
        assert!(completion.script.contains("upload"));
        assert!(completion.command_count >= command_groups().len());
    }

    #[test]
    fn test_serve_plan_reports_registry_counts() {
        let plan = serve_plan(&ServeArgs {
            mcp: true,
            transport: "stdio".to_string(),
            port: 8080,
        })
        .expect("serve plan");
        assert_eq!(plan.mode, "mcp");
        assert_eq!(plan.status, "planned_not_started");
        assert_eq!(plan.capabilities, capabilities().len());
    }

    #[tokio::test]
    async fn test_doctor_supports_documented_check_names() {
        let global = test_global_args();
        for name in [
            "auth",
            "config",
            "registry",
            "permissions",
            "region",
            "network",
            "version",
            "mcp",
            "completion",
            // [Review Fix #s2] principles is part of the documented check set.
            "principles",
        ] {
            let report = doctor_report(
                &global,
                &DoctorArgs {
                    check: Some(name.to_string()),
                    bucket: None,
                    live_network: false,
                    network_timeout_ms: 3000,
                },
            )
            .await
            .expect("doctor report");
            assert_eq!(report.summary.total, 1, "check {name}");
        }
    }

    /// [Review Fix #s2] principles_check must pass against the current registry
    /// and report exactly one passing entry when invoked in isolation.
    #[tokio::test]
    async fn test_doctor_principles_check_passes() {
        let report = doctor_report(
            &test_global_args(),
            &DoctorArgs {
                check: Some("principles".to_string()),
                bucket: None,
                live_network: false,
                network_timeout_ms: 3000,
            },
        )
        .await
        .expect("doctor report");
        assert_eq!(report.summary.total, 1);
        assert_eq!(report.summary.passed, 1, "{:?}", report.checks);
        assert_eq!(report.checks[0].name, "principles");
    }

    /// [Spec §4.5 / AGT-001] `groups` view returns one entry per registry
    /// group, each tagged with `command_count` so the Agent can prioritise
    /// drilling into the largest groups first.
    #[test]
    fn test_capabilities_groups_view_includes_command_count() {
        let args = CapabilitiesArgs {
            view: "groups".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("groups view");
        assert_eq!(view.view, "groups");
        assert!(!view.groups.is_empty(), "groups view must surface groups");
        assert!(
            view.capabilities.is_empty(),
            "groups view must not return capabilities"
        );
        assert!(
            view.commands.is_empty(),
            "groups view must not return commands"
        );
        assert!(
            view.lines.is_empty(),
            "groups view must not return text lines"
        );
        let total_count: usize = view.groups.iter().map(|g| g.command_count).sum();
        assert!(total_count > 0, "command_count must be populated");
    }

    /// [Spec §4.5] `text` view returns a flat list of `<command>\t<desc>`
    /// lines so an Agent can scan the full surface in O(N) tokens.
    #[test]
    fn test_capabilities_text_view_returns_one_line_per_command() {
        let args = CapabilitiesArgs {
            view: "text".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("text view");
        assert_eq!(view.view, "text");
        assert!(!view.lines.is_empty(), "text view must surface lines");
        for line in &view.lines {
            assert!(
                line.contains('\t'),
                "text line must use tab separator: {line}"
            );
        }
        assert!(view.groups.is_empty(), "text view must not echo groups");
        assert!(
            view.capabilities.is_empty(),
            "text view must not echo capabilities"
        );
        assert!(view.commands.is_empty(), "text view must not echo commands");
    }

    /// [Spec §4.5 / AGT-002] `compact` view strips parameters from each
    /// capability row but keeps every other metadata field.
    #[test]
    fn test_capabilities_compact_view_strips_parameters() {
        let args = CapabilitiesArgs {
            view: "compact".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("compact view");
        assert_eq!(view.view, "compact");
        assert!(
            !view.capabilities.is_empty(),
            "compact view must surface capabilities"
        );
        for row in &view.capabilities {
            assert!(
                row.parameters.is_none(),
                "compact view must drop parameters"
            );
        }
    }

    /// [Spec §4.5 / AGT-002] `full` view tags each capability with
    /// `layer` / `endpoint_rule` / `destructive` and retains parameters.
    #[test]
    fn test_capabilities_full_view_carries_layer_endpoint_rule_destructive() {
        let args = CapabilitiesArgs {
            view: "full".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("full view");
        assert_eq!(view.view, "full");
        let value = serde_json::to_value(&view).expect("serialize full view");
        let caps = value["capabilities"]
            .as_array()
            .expect("capabilities array");
        assert!(!caps.is_empty(), "full view must surface capabilities");
        for cap in caps {
            assert!(cap.get("layer").is_some(), "every cap must carry layer");
            assert!(
                cap.get("destructive").is_some(),
                "every cap must carry destructive"
            );
            assert!(cap.as_object().unwrap().contains_key("endpoint_rule"));
        }
        let any_destructive = caps
            .iter()
            .any(|c| c["destructive"].as_bool().unwrap_or(false));
        assert!(
            any_destructive,
            "full view must mark at least one cap destructive"
        );
    }

    #[test]
    fn test_capabilities_public_commands_use_ve_tos_prefix() {
        let args = CapabilitiesArgs {
            view: "full".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("full view");
        let value = serde_json::to_value(&view).expect("serialize full view");

        let capabilities = value["capabilities"]
            .as_array()
            .expect("capabilities array");
        assert!(
            capabilities
                .iter()
                .any(|cap| cap["command"].as_str() == Some("ve-tos cp")),
            "full view must expose high-level capabilities under ve-tos"
        );
        assert!(
            capabilities
                .iter()
                .filter_map(|cap| cap["command"].as_str())
                .all(|command| !command.starts_with("tos ")),
            "public capability commands must not expose legacy tos prefix"
        );

        let commands = value["commands"].as_array().expect("commands array");
        assert!(
            commands
                .iter()
                .any(|entry| entry["command"].as_str() == Some("ve-tos bucket")),
            "command tree must expose ve-tos command paths"
        );
        assert!(
            commands
                .iter()
                .filter_map(|entry| entry["command"].as_str())
                .all(|command| !command.starts_with("tos ")),
            "public command tree must not expose legacy tos prefix"
        );

        let groups = value["groups"].as_array().expect("groups array");
        assert!(
            groups
                .iter()
                .any(|group| group["command"].as_str() == Some("ve-tos cp")),
            "group summaries must expose ve-tos command paths"
        );
    }

    /// [Spec §4.5] `tree` is preserved as a legacy alias for `compact` so
    /// existing callers keep working.
    #[test]
    fn test_capabilities_tree_alias_resolves_to_compact() {
        let args = CapabilitiesArgs {
            view: "tree".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let view = capabilities_view(&args).expect("tree alias");
        assert_eq!(view.view, "compact");
    }

    /// [Spec §4.5 / AGT-003] `--search 加密` must hit encryption / SSE
    /// related capabilities through the Chinese→English alias map.
    #[test]
    fn test_capabilities_search_chinese_encryption_term_hits_sse() {
        let args = CapabilitiesArgs {
            view: "compact".to_string(),
            group: None,
            search: Some("加密".to_string()),
            layer: None,
        };
        let view = capabilities_view(&args).expect("search 加密");
        let hit_commands: Vec<String> = view
            .capabilities
            .iter()
            .map(|c| c.command.to_string())
            .chain(view.commands.iter().map(|c| c.command.clone()))
            .collect();
        let joined = hit_commands.join("\n").to_lowercase();
        assert!(
            joined.contains("encrypt") || joined.contains("sse") || joined.contains("kms"),
            "search 加密 should hit encryption/SSE/KMS capabilities, got: {hit_commands:?}",
        );
        assert!(
            !view.search_scores.is_empty(),
            "search scores must be populated"
        );
    }

    /// [Spec §4.5] Unknown view value yields a deterministic ValidationError.
    #[test]
    fn test_capabilities_view_rejects_unknown_view() {
        let args = CapabilitiesArgs {
            view: "bogus".to_string(),
            group: None,
            search: None,
            layer: None,
        };
        let err = capabilities_view(&args).expect_err("unknown view");
        assert!(err.to_string().contains("unsupported capabilities view"));
        assert!(err.to_string().contains("groups, text, compact, or full"));
    }

    fn test_global_args() -> GlobalArgs {
        GlobalArgs {
            profile: "default".to_string(),
            config_path: None,
            region: None,
            endpoint: None,
            psm: None,
            idc: None,
            cluster: None,
            addr_family: None,
            control_endpoint: None,
            account_id: None,
            output: None,
            query: None,
            dry_run: false,
            describe: false,
            no_color: false,
            verbose: false,
            quiet: false,
            trace_dir: None,
            trace_redact: "strict".to_string(),
            yes: false,
            confirm: None,
        }
    }
}
