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

//! Integration tests for current pagination contracts.
//! [Review Fix #3] Align pagination tests with current PaginationParams and Envelope metadata.

use tos_core::agent::envelope::{Envelope, PaginationInfo};
use tos_core::agent::pagination::PaginationParams;

#[test]
fn test_pagination_params_default() {
    let params = PaginationParams::default();
    assert!(params.page_token.is_none());
    assert_eq!(params.page_size, 100);
}

#[test]
fn test_pagination_params_serialization_omits_absent_page_token() {
    let params = PaginationParams::default();
    let json = serde_json::to_value(&params).unwrap();
    assert!(json.get("page_token").is_none());
    assert_eq!(json["page_size"], 100);
}

#[test]
fn test_pagination_params_with_values() {
    let params = PaginationParams {
        page_token: Some("token-abc".into()),
        page_size: 50,
    };
    assert_eq!(params.page_token.as_deref(), Some("token-abc"));
    assert_eq!(params.page_size, 50);
}

#[test]
fn test_envelope_pagination_info_serialization() {
    let envelope = Envelope::success("tos object list", serde_json::json!({"objects": []}))
        .with_pagination(PaginationInfo {
            next_token: Some("page2-token".into()),
            next_marker: None,
            total_returned: 42,
        });
    let json = serde_json::to_value(&envelope).unwrap();
    assert_eq!(json["pagination"]["next_token"], "page2-token");
    assert_eq!(json["pagination"]["total_returned"], 42);
}
