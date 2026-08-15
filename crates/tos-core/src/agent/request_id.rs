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

use std::sync::Mutex;

use serde_json::{Map, Number, Value};

/// Maximum number of service request IDs retained for one CLI invocation.
pub const SERVICE_REQUEST_ID_LIMIT: usize = 1024;

/// Maximum number of Unicode scalar values accepted in one request ID.
pub const SERVICE_REQUEST_ID_MAX_CHARS: usize = 256;

/// Immutable view of the service responses observed by one CLI invocation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ServiceRequestSnapshot {
    /// Sanitized service request IDs retained in response-completion order.
    pub request_ids: Vec<String>,
    /// Number of additional sanitized IDs omitted after reaching the limit.
    pub request_ids_omitted: usize,
    /// Most recently observed request ID from a successful HTTP response.
    pub last_successful_request_id: Option<String>,
    /// Total HTTP responses observed, including responses without a usable ID.
    pub response_count: usize,
    /// Whether the invocation reached at least one primary-service request attempt.
    pub request_attempted: bool,
    /// Whether the terminal request attempt received an HTTP response.
    pub terminal_response_received: bool,
    /// Sanitized request ID from the terminal response, when one was present.
    pub terminal_response_request_id: Option<String>,
}

#[derive(Debug, Default)]
struct ServiceRequestState {
    request_ids: Vec<String>,
    request_ids_omitted: usize,
    last_successful_request_id: Option<String>,
    response_count: usize,
    request_attempted: bool,
    terminal_response_received: bool,
    terminal_response_request_id: Option<String>,
}

/// Thread-safe, bounded service request-ID trace for one CLI invocation.
#[derive(Debug)]
pub struct ServiceRequestTrace {
    limit: usize,
    state: Mutex<ServiceRequestState>,
}

impl Default for ServiceRequestTrace {
    fn default() -> Self {
        Self::with_limit(SERVICE_REQUEST_ID_LIMIT)
    }
}

impl ServiceRequestTrace {
    /// Create a trace with a caller-supplied retention limit.
    ///
    /// A zero limit retains no individual IDs while still counting omissions
    /// and tracking the last successful response.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            limit,
            state: Mutex::new(ServiceRequestState::default()),
        }
    }

    /// Record one service response, retaining its ID only when it is safe.
    ///
    /// `is_success` reflects the HTTP response status. Invalid or absent IDs
    /// still contribute to the response count but never enter structured output.
    pub fn record_response(&self, raw_request_id: Option<&str>, is_success: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let request_id = raw_request_id.and_then(sanitize_request_id);
        state.response_count = state.response_count.saturating_add(1);
        state.request_attempted = true;
        state.terminal_response_received = true;
        state.terminal_response_request_id = request_id.clone();
        let Some(request_id) = request_id else {
            return;
        };
        if state.request_ids.len() < self.limit {
            state.request_ids.push(request_id.clone());
        } else {
            state.request_ids_omitted = state.request_ids_omitted.saturating_add(1);
        }
        if is_success {
            state.last_successful_request_id = Some(request_id);
        }
    }

    /// Record a terminal HTTP attempt that failed before any response arrived.
    pub fn record_no_response(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.request_attempted = true;
        state.terminal_response_received = false;
        state.terminal_response_request_id = None;
    }

    /// Return a consistent snapshot without exposing the internal mutex.
    pub fn snapshot(&self) -> ServiceRequestSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        ServiceRequestSnapshot {
            request_ids: state.request_ids.clone(),
            request_ids_omitted: state.request_ids_omitted,
            last_successful_request_id: state.last_successful_request_id.clone(),
            response_count: state.response_count,
            request_attempted: state.request_attempted,
            terminal_response_received: state.terminal_response_received,
            terminal_response_request_id: state.terminal_response_request_id.clone(),
        }
    }
}

/// Normalize and validate a service-provided request identifier.
///
/// Returns `None` for empty values, control characters, or values longer than
/// [`SERVICE_REQUEST_ID_MAX_CHARS`] Unicode scalar values.
pub fn sanitize_request_id(raw_request_id: &str) -> Option<String> {
    let request_id = raw_request_id.trim();
    (!request_id.is_empty()
        && request_id.chars().count() <= SERVICE_REQUEST_ID_MAX_CHARS
        && !request_id.chars().any(char::is_control))
    .then(|| request_id.to_string())
}

/// Apply one invocation trace to a serialized successful CLI Envelope.
///
/// The last successful service ID replaces the generated top-level fallback,
/// unless the command deliberately emitted `request_id: null`. When more than
/// one response was observed, bounded diagnostics are added to object-shaped
/// `data` without changing non-object payloads.
pub fn apply_service_trace_to_success_envelope(
    envelope: &mut Value,
    snapshot: &ServiceRequestSnapshot,
) {
    let Value::Object(envelope_fields) = envelope else {
        return;
    };
    if envelope_fields.get("success").and_then(Value::as_bool) != Some(true) {
        return;
    }

    // [Review Fix #3] Legacy handlers may attach a raw response header before
    // the trace projection runs. Reject unsafe values at the final success
    // boundary so they cannot suppress the generated ULID fallback.
    sanitize_existing_envelope_request_id(envelope_fields);

    let has_explicit_null = matches!(envelope_fields.get("request_id"), Some(Value::Null));
    if !has_explicit_null {
        if let Some(request_id) = &snapshot.last_successful_request_id {
            envelope_fields.insert("request_id".to_string(), Value::String(request_id.clone()));
        }
    }

    if snapshot.response_count <= 1 {
        return;
    }
    let Some(Value::Object(data_fields)) = envelope_fields.get_mut("data") else {
        return;
    };
    insert_multi_request_trace(data_fields, snapshot);
}

/// Select an error Envelope ID from the terminal request attempt.
///
/// Once the invocation attempted a primary-service request, only the terminal
/// response can supply the error ID. A terminal transport failure or a
/// response without a safe ID deliberately returns `None`, allowing the
/// Envelope to keep its generated fallback. The legacy candidate is used only
/// when no traced primary-service attempt exists.
pub fn select_error_request_id(
    snapshot: &ServiceRequestSnapshot,
    legacy_request_id: Option<&str>,
) -> Option<String> {
    if snapshot.request_attempted {
        return snapshot.terminal_response_request_id.clone();
    }
    legacy_request_id.and_then(sanitize_request_id)
}

fn sanitize_existing_envelope_request_id(envelope_fields: &mut Map<String, Value>) {
    let Some(value) = envelope_fields.get("request_id") else {
        return;
    };
    if value.is_null() {
        return;
    }
    let sanitized = value.as_str().and_then(sanitize_request_id);
    match sanitized {
        Some(request_id) => {
            envelope_fields.insert("request_id".to_string(), Value::String(request_id));
        }
        None => {
            envelope_fields.remove("request_id");
        }
    }
}

fn insert_multi_request_trace(
    data_fields: &mut Map<String, Value>,
    snapshot: &ServiceRequestSnapshot,
) {
    data_fields.insert(
        "service_request_ids".to_string(),
        Value::Array(
            snapshot
                .request_ids
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        ),
    );
    data_fields.insert(
        "service_request_ids_omitted".to_string(),
        Value::Number(Number::from(
            u64::try_from(snapshot.request_ids_omitted).unwrap_or(u64::MAX),
        )),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::envelope::Envelope;
    use crate::agent::global_args::GlobalArgs;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn sanitizes_request_ids() {
        assert_eq!(sanitize_request_id(" req-1 "), Some("req-1".to_string()));
        assert_eq!(sanitize_request_id(""), None);
        assert_eq!(sanitize_request_id("req\n1"), None);
        assert_eq!(sanitize_request_id(&"x".repeat(257)), None);
    }

    #[test]
    fn bounds_ids_and_tracks_last_success() {
        let trace = ServiceRequestTrace::with_limit(2);
        trace.record_response(Some("retry-1"), false);
        trace.record_response(Some("ok-2"), true);
        trace.record_response(Some("ok-3"), true);

        assert_eq!(
            trace.snapshot(),
            ServiceRequestSnapshot {
                request_ids: vec!["retry-1".into(), "ok-2".into()],
                request_ids_omitted: 1,
                last_successful_request_id: Some("ok-3".into()),
                response_count: 3,
                request_attempted: true,
                terminal_response_received: true,
                terminal_response_request_id: Some("ok-3".into()),
            }
        );
    }

    #[test]
    fn default_limit_retains_1024_and_omits_the_1025th_id() {
        let trace = ServiceRequestTrace::default();
        for index in 0..=SERVICE_REQUEST_ID_LIMIT {
            trace.record_response(Some(&format!("request-{index}")), true);
        }

        let snapshot = trace.snapshot();
        assert_eq!(snapshot.request_ids.len(), SERVICE_REQUEST_ID_LIMIT);
        assert_eq!(snapshot.request_ids_omitted, 1);
        assert_eq!(snapshot.response_count, SERVICE_REQUEST_ID_LIMIT + 1);
        assert_eq!(
            snapshot.last_successful_request_id.as_deref(),
            Some("request-1024")
        );
    }

    #[test]
    fn multi_request_detection_counts_responses_without_usable_ids() {
        let trace = ServiceRequestTrace::default();
        trace.record_response(None, true);
        trace.record_response(Some("page-2"), true);
        let mut value =
            serde_json::to_value(Envelope::success("tos ls", json!({}))).expect("envelope");

        apply_service_trace_to_success_envelope(&mut value, &trace.snapshot());

        assert_eq!(value["request_id"], "page-2");
        assert_eq!(value["data"]["service_request_ids"], json!(["page-2"]));
        assert_eq!(value["data"]["service_request_ids_omitted"], 0);
    }

    #[test]
    fn terminal_no_response_does_not_reuse_an_earlier_service_id() {
        let trace = ServiceRequestTrace::default();
        trace.record_response(Some("retry-response"), false);
        trace.record_no_response();

        assert_eq!(
            select_error_request_id(&trace.snapshot(), Some("stale-env-id")),
            None
        );
    }

    #[test]
    fn response_caused_decode_failure_keeps_terminal_response_id() {
        let trace = ServiceRequestTrace::default();
        trace.record_response(Some("decode-response"), true);

        assert_eq!(
            select_error_request_id(&trace.snapshot(), None).as_deref(),
            Some("decode-response")
        );
    }

    #[test]
    fn global_args_clones_share_only_the_same_invocation_trace() {
        let first = GlobalArgs::default();
        let clone = first.clone();
        let second = GlobalArgs::default();

        assert!(Arc::ptr_eq(&first.request_trace, &clone.request_trace));
        assert!(!Arc::ptr_eq(&first.request_trace, &second.request_trace));
    }

    #[test]
    fn service_trace_overrides_generated_success_id() {
        let trace = ServiceRequestTrace::default();
        trace.record_response(Some("service-1"), true);
        let mut value =
            serde_json::to_value(Envelope::success("tos ls", json!({}))).expect("envelope");

        apply_service_trace_to_success_envelope(&mut value, &trace.snapshot());

        assert_eq!(value["request_id"], "service-1");
    }

    #[test]
    fn multi_request_trace_is_added_to_data() {
        let trace = ServiceRequestTrace::with_limit(2);
        trace.record_response(Some("retry"), false);
        trace.record_response(Some("success"), true);
        let mut value =
            serde_json::to_value(Envelope::success("tos ls", json!({}))).expect("envelope");

        apply_service_trace_to_success_envelope(&mut value, &trace.snapshot());

        assert_eq!(
            value["data"]["service_request_ids"],
            json!(["retry", "success"])
        );
        assert_eq!(value["data"]["service_request_ids_omitted"], 0);
    }

    #[test]
    fn explicit_null_request_id_is_preserved() {
        let trace = ServiceRequestTrace::default();
        trace.record_response(Some("service-1"), true);
        let mut value =
            serde_json::to_value(Envelope::success("tos du", json!({})).without_request_id())
                .expect("envelope");

        apply_service_trace_to_success_envelope(&mut value, &trace.snapshot());

        assert!(value["request_id"].is_null());
    }

    #[test]
    fn single_response_does_not_add_multi_request_fields() {
        let trace = ServiceRequestTrace::default();
        trace.record_response(Some("service-1"), true);
        let mut value =
            serde_json::to_value(Envelope::success("tos stat", json!({}))).expect("envelope");

        apply_service_trace_to_success_envelope(&mut value, &trace.snapshot());

        assert!(value["data"].get("service_request_ids").is_none());
        assert!(value["data"].get("service_request_ids_omitted").is_none());
    }

    #[test]
    fn unsafe_existing_success_id_is_removed_for_fallback_injection() {
        let mut value =
            serde_json::to_value(Envelope::success("tos stat", json!({}))).expect("envelope");
        value["request_id"] = Value::String("x".repeat(257));

        apply_service_trace_to_success_envelope(
            &mut value,
            &ServiceRequestTrace::default().snapshot(),
        );

        assert!(value.get("request_id").is_none());
    }
}
