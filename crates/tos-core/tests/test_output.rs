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

//! Integration tests for current output formatting helpers.
//! [Review Fix #3] Align output tests with current format_json/format_table helpers.

use clap::ValueEnum;
use serde::Serialize;
use tos_core::agent::output::{format_json, format_table, format_xml, OutputFormat};

#[derive(Debug, Clone, Serialize)]
struct TestItem {
    name: String,
    value: u64,
}

#[test]
fn test_output_format_value_enum_parses_supported_formats() {
    assert!(matches!(
        OutputFormat::from_str("json", true).unwrap(),
        OutputFormat::Json
    ));
    assert!(matches!(
        OutputFormat::from_str("table", true).unwrap(),
        OutputFormat::Table
    ));
    assert!(matches!(
        OutputFormat::from_str("csv", true).unwrap(),
        OutputFormat::Csv
    ));
    assert!(matches!(
        OutputFormat::from_str("yaml", true).unwrap(),
        OutputFormat::Yaml
    ));
    assert!(matches!(
        OutputFormat::from_str("xml", true).unwrap(),
        OutputFormat::Xml
    ));
    // [Review Fix #11] Markdown is now a first-class output format.
    assert!(matches!(
        OutputFormat::from_str("markdown", true).unwrap(),
        OutputFormat::Markdown
    ));
}

#[test]
fn test_output_format_rejects_unsupported_format() {
    // [Review Fix #11] `markdown` is supported now; verify the enum still
    // rejects clearly unsupported values like `pdf`.
    assert!(OutputFormat::from_str("pdf", true).is_err());
}

#[test]
fn test_auto_detect_returns_a_supported_format() {
    let format = OutputFormat::auto_detect();
    assert!(matches!(format, OutputFormat::Json | OutputFormat::Table));
}

#[test]
fn test_format_json_pretty_prints_serializable_value() {
    let item = TestItem {
        name: "alpha".into(),
        value: 7,
    };
    let output = format_json(&item).unwrap();
    assert!(output.contains("\"name\": \"alpha\""));
    assert!(output.contains("\"value\": 7"));
}

#[test]
fn test_format_xml_renders_controlled_output_not_service_protocol() {
    let item = TestItem {
        name: "alpha&beta".into(),
        value: 7,
    };
    let output = format_xml(&item).unwrap();

    assert!(output.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    assert!(output.contains("<name>alpha&amp;beta</name>"));
    assert!(output.contains("<value>7</value>"));
}

#[test]
fn test_format_table_contains_headers_and_rows() {
    let rows = vec![
        vec!["alpha".to_string(), "1".to_string()],
        vec!["beta".to_string(), "2".to_string()],
    ];
    let output = format_table(&["Name", "Value"], &rows);
    assert!(output.contains("Name"));
    assert!(output.contains("Value"));
    assert!(output.contains("alpha"));
    assert!(output.contains("beta"));
}
