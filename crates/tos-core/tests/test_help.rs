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

//! Integration tests for agent-facing command description metadata.
//! [Review Fix #3] Align help/describe tests with current CommandDescription metadata.

use std::collections::HashMap;
use tos_core::agent::describe::{
    CommandDescription, CommandLayer, CommandParameter, ParameterLocation, RiskLevel,
};

#[test]
fn test_command_description_serializes_core_fields() {
    let description = CommandDescription {
        command: "tos bucket create".into(),
        layer: CommandLayer::LowLevel,
        api: Some("CreateBucket".into()),
        description: "Create a bucket".into(),
        risk_level: RiskLevel::Medium,
        supports_dry_run: true,
        supports_pipe: false,
        parameters: Some(vec![CommandParameter {
            name: "bucket".into(),
            location: ParameterLocation::Path,
            required: true,
            description: "Bucket name".into(),
            ..Default::default()
        }]),
        scenario_routing: Some(HashMap::from([(
            "endpoint_kind".into(),
            "DataPlane".into(),
        )])),
        related_commands: None,
        low_level_apis: None,
        ..Default::default()
    };

    let parsed = serde_json::to_value(&description).unwrap();
    assert_eq!(parsed["command"], "tos bucket create");
    assert_eq!(parsed["layer"], "low_level");
    assert_eq!(parsed["api"], "CreateBucket");
    assert_eq!(parsed["risk_level"], "medium");
    assert_eq!(parsed["supports_dry_run"], true);
    assert_eq!(parsed["parameters"][0]["location"], "path");
    assert_eq!(parsed["scenario_routing"]["endpoint_kind"], "DataPlane");
}

#[test]
fn test_command_description_omits_optional_fields_when_absent() {
    let description = CommandDescription {
        command: "tos config show".into(),
        layer: CommandLayer::Meta,
        api: None,
        description: "Show config".into(),
        risk_level: RiskLevel::Low,
        supports_dry_run: false,
        supports_pipe: false,
        parameters: None,
        scenario_routing: None,
        related_commands: None,
        low_level_apis: None,
        ..Default::default()
    };

    let parsed = serde_json::to_value(&description).unwrap();
    assert!(parsed.get("api").is_none());
    assert!(parsed.get("parameters").is_none());
    assert!(parsed.get("scenario_routing").is_none());
}

#[test]
fn test_parameter_locations_keep_snake_case_contract() {
    let locations = [
        (ParameterLocation::Path, "path"),
        (ParameterLocation::Query, "query"),
        (ParameterLocation::Header, "header"),
        (ParameterLocation::Body, "body"),
        (ParameterLocation::Flag, "flag"),
    ];

    for (location, expected) in locations {
        let value = serde_json::to_value(location).unwrap();
        assert_eq!(value, expected);
    }
}
