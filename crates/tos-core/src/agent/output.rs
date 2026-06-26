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

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::io::IsTerminal;

#[derive(Debug, Clone, Copy, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Json,
    Xml,
    Table,
    Csv,
    Yaml,
    /// [G9] Markdown — emit Envelope as a human-readable Markdown report so
    /// Agents (and humans) can paste CLI responses directly into chat / docs.
    Markdown,
}

impl OutputFormat {
    /// 自动检测：TTY 默认 table，管道默认 json
    pub fn auto_detect() -> Self {
        if std::io::stdout().is_terminal() {
            OutputFormat::Table
        } else {
            OutputFormat::Json
        }
    }
}

/// 格式化 JSON 输出
pub fn format_json<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(value)
}

/// Format serialized data as XML for CLI output.
pub fn format_xml<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let value = serde_json::to_value(value)?;
    Ok(render_xml_document("response", &value))
}

/// [G9] Format an Envelope-shaped value (or any serialisable value) as a
/// Markdown report. The intent is *human readability*, so a top-level Envelope
/// becomes a heading + status callout + table of payload fields, while plain
/// arrays/objects fall back to a generic Markdown rendering.
pub fn format_markdown<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let value = serde_json::to_value(value)?;
    Ok(render_markdown(&value))
}

fn render_markdown(value: &serde_json::Value) -> String {
    if let serde_json::Value::Object(map) = value {
        if map.contains_key("status") && map.contains_key("command") {
            return render_envelope_markdown(map);
        }
    }
    render_value_markdown(value, 0)
}

fn render_envelope_markdown(map: &serde_json::Map<String, serde_json::Value>) -> String {
    let command = map
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or("(unknown command)");
    let status = map
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let request_id = map.get("request_id").and_then(|v| v.as_str()).unwrap_or("");

    let mut out = String::new();
    out.push_str(&format!("# `{}`\n\n", command));
    let badge = match status {
        "success" => "**Status:** ✅ success",
        "error" => "**Status:** ❌ error",
        other => &format!("**Status:** `{}`", other),
    };
    out.push_str(badge);
    out.push_str("\n\n");
    if !request_id.is_empty() {
        out.push_str(&format!("**Request ID:** `{}`\n\n", request_id));
    }

    if let Some(error) = map.get("error") {
        out.push_str("## Error\n\n");
        out.push_str(&render_value_markdown(error, 0));
        out.push_str("\n");
    }

    if let Some(data) = map.get("data") {
        out.push_str("## Data\n\n");
        out.push_str(&render_value_markdown(data, 0));
        out.push_str("\n");
    }

    if let Some(pagination) = map.get("pagination") {
        if !pagination.is_null() {
            out.push_str("## Pagination\n\n");
            out.push_str(&render_value_markdown(pagination, 0));
            out.push_str("\n");
        }
    }

    out
}

fn render_value_markdown(value: &serde_json::Value, depth: usize) -> String {
    match value {
        serde_json::Value::Null => "_null_\n".to_string(),
        serde_json::Value::Bool(b) => format!("`{}`\n", b),
        serde_json::Value::Number(n) => format!("`{}`\n", n),
        serde_json::Value::String(s) => {
            if s.is_empty() {
                "_(empty)_\n".to_string()
            } else if s.contains('\n') {
                format!("```\n{}\n```\n", s)
            } else {
                format!("{}\n", s)
            }
        }
        serde_json::Value::Array(items) => render_array_markdown(items, depth),
        serde_json::Value::Object(map) => render_object_markdown(map, depth),
    }
}

fn render_array_markdown(items: &[serde_json::Value], depth: usize) -> String {
    if items.is_empty() {
        return "_(empty list)_\n".to_string();
    }
    // Tabular shortcut: array of homogeneous flat objects.
    if let Some(table) = try_render_array_as_table(items) {
        return table;
    }
    let mut out = String::new();
    for item in items {
        let rendered = render_value_markdown(item, depth + 1);
        let trimmed = rendered.trim_end();
        if trimmed.contains('\n') {
            out.push_str("- \n");
            for line in trimmed.lines() {
                out.push_str(&format!("  {}\n", line));
            }
        } else {
            out.push_str(&format!("- {}\n", trimmed));
        }
    }
    out
}

fn try_render_array_as_table(items: &[serde_json::Value]) -> Option<String> {
    let mut headers: Vec<String> = Vec::new();
    for item in items {
        let map = item.as_object()?;
        for key in map.keys() {
            if !headers.iter().any(|h| h == key) {
                headers.push(key.clone());
            }
        }
        for value in map.values() {
            if value.is_object() || value.is_array() {
                return None;
            }
        }
    }
    if headers.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push_str("| ");
    out.push_str(&headers.join(" | "));
    out.push_str(" |\n|");
    for _ in &headers {
        out.push_str(" --- |");
    }
    out.push('\n');
    for item in items {
        let map = item.as_object().expect("checked above");
        out.push_str("| ");
        let cells: Vec<String> = headers
            .iter()
            .map(|h| match map.get(h) {
                Some(serde_json::Value::Null) | None => String::new(),
                Some(serde_json::Value::String(s)) => s.replace('|', "\\|"),
                Some(other) => other.to_string(),
            })
            .collect();
        out.push_str(&cells.join(" | "));
        out.push_str(" |\n");
    }
    Some(out)
}

fn render_object_markdown(
    map: &serde_json::Map<String, serde_json::Value>,
    depth: usize,
) -> String {
    if map.is_empty() {
        return "_(empty object)_\n".to_string();
    }
    let all_scalar = map.values().all(|v| !v.is_object() && !v.is_array());
    if all_scalar && depth == 0 {
        let mut out = String::from("| Field | Value |\n| --- | --- |\n");
        for (k, v) in map {
            let rendered = match v {
                serde_json::Value::Null => String::new(),
                serde_json::Value::String(s) => s.replace('|', "\\|"),
                other => other.to_string(),
            };
            out.push_str(&format!("| `{}` | {} |\n", k, rendered));
        }
        return out;
    }
    let mut out = String::new();
    for (k, v) in map {
        let header_level = (depth + 3).min(6);
        let hashes = "#".repeat(header_level);
        out.push_str(&format!("{} {}\n\n", hashes, k));
        out.push_str(&render_value_markdown(v, depth + 1));
        out.push('\n');
    }
    out
}

/// 格式化表格输出（使用 comfy-table）
pub fn format_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut table = comfy_table::Table::new();
    table.set_header(headers.iter().map(|h| h.to_string()));
    for row in rows {
        table.add_row(row.clone());
    }
    table.to_string()
}

fn render_xml_document(root: &str, value: &serde_json::Value) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{}",
        render_xml_value(root, value)
    )
}

fn render_xml_value(name: &str, value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => format!("<{} />", escape_xml_name(name)),
        serde_json::Value::Bool(v) => render_xml_text(name, &v.to_string()),
        serde_json::Value::Number(v) => render_xml_text(name, &v.to_string()),
        serde_json::Value::String(v) => render_xml_text(name, v),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| render_xml_value(name, item))
            .collect::<Vec<_>>()
            .join(""),
        serde_json::Value::Object(map) => {
            let tag = escape_xml_name(name);
            let children = map
                .iter()
                .map(|(key, value)| render_xml_value(key, value))
                .collect::<Vec<_>>()
                .join("");
            format!("<{}>{}</{}>", tag, children, tag)
        }
    }
}

fn render_xml_text(name: &str, text: &str) -> String {
    let tag = escape_xml_name(name);
    format!("<{}>{}</{}>", tag, escape_xml_text(text), tag)
}

fn escape_xml_name(name: &str) -> String {
    let mut output = String::new();
    for (idx, ch) in name.chars().enumerate() {
        let valid = ch.is_ascii_alphanumeric() || ch == '_' || ch == '-';
        if valid && !(idx == 0 && ch.is_ascii_digit()) {
            output.push(ch);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "value".to_string()
    } else {
        output
    }
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_json() {
        let data = serde_json::json!({"key": "value"});
        let result = format_json(&data).unwrap();
        assert!(result.contains("key"));
        assert!(result.contains("value"));
    }

    #[test]
    fn test_format_table() {
        let headers = &["Name", "Size", "Type"];
        let rows = vec![
            vec![
                "file1.txt".to_string(),
                "1024".to_string(),
                "STANDARD".to_string(),
            ],
            vec![
                "file2.txt".to_string(),
                "2048".to_string(),
                "IA".to_string(),
            ],
        ];
        let result = format_table(headers, &rows);
        assert!(result.contains("file1.txt"));
        assert!(result.contains("1024"));
        assert!(result.contains("STANDARD"));
    }

    // [G9] Markdown renderer tests.
    #[test]
    fn markdown_renders_envelope_with_heading_and_status() {
        let env = serde_json::json!({
            "status": "success",
            "command": "tos object list",
            "request_id": "01HXYZ",
            "data": {"objects": []},
        });
        let md = format_markdown(&env).unwrap();
        assert!(md.contains("# `tos object list`"));
        assert!(md.contains("✅ success"));
        assert!(md.contains("`01HXYZ`"));
        assert!(md.contains("## Data"));
    }

    #[test]
    fn markdown_renders_homogeneous_array_as_table() {
        let value = serde_json::json!([
            {"name": "a", "size": 1},
            {"name": "b", "size": 2},
        ]);
        let md = format_markdown(&value).unwrap();
        assert!(md.contains("| name | size |"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| a | 1 |"));
        assert!(md.contains("| b | 2 |"));
    }

    #[test]
    fn markdown_renders_error_envelope() {
        let env = serde_json::json!({
            "status": "error",
            "command": "tos object get",
            "request_id": "01HERR",
            "error": {"code": "NoSuchKey", "message": "missing"},
        });
        let md = format_markdown(&env).unwrap();
        assert!(md.contains("❌ error"));
        assert!(md.contains("## Error"));
        assert!(md.contains("NoSuchKey"));
    }
}
