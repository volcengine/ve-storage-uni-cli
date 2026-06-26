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

use reqwest::{Body, Method, Response};
use serde::Serialize;
use serde_json::{json, Value};
use tos_core::agent::envelope::Envelope;
use tos_core::agent::error::CliError;
use tos_core::infra::client::TosClient;

const MAX_RAW_RESPONSE_BODY_BYTES: u64 = 10 * 1024 * 1024;

/// Serialized view of a low-level raw response.
#[derive(Debug, Serialize)]
pub struct RawResponseData {
    pub status_code: u16,
    pub headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

/// Structured metadata for download-like commands.
#[derive(Debug, Serialize)]
pub struct DownloadResult {
    pub bucket: String,
    pub key: String,
    pub output: String,
    pub bytes_written: u64,
    pub headers: BTreeMap<String, String>,
}

/// Execute a bucket-scoped request whose path is `/{bucket}`.
pub async fn execute_bucket_request(
    client: &TosClient,
    command: &str,
    method: Method,
    bucket: &str,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
) -> Result<Envelope<RawResponseData>, CliError> {
    let url = client.bucket_endpoint(bucket)?;
    let path = client.bucket_request_path(bucket)?;
    execute_request(client, command, method, &url, &path, query, headers, body).await
}

/// Execute an object-scoped request whose path is `/{bucket}/{key}`.
pub async fn execute_object_request(
    client: &TosClient,
    command: &str,
    method: Method,
    bucket: &str,
    key: &str,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
) -> Result<Envelope<RawResponseData>, CliError> {
    let url = client.object_endpoint(bucket, key)?;
    let path = client.object_request_path(bucket, key)?;
    execute_request(client, command, method, &url, &path, query, headers, body).await
}

/// Execute a service/control-plane request whose path is already fully resolved.
pub async fn execute_endpoint_request(
    client: &TosClient,
    command: &str,
    method: Method,
    endpoint: &str,
    path: &str,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
) -> Result<Envelope<RawResponseData>, CliError> {
    let url = format!("{}{}", endpoint.trim_end_matches('/'), path);
    execute_request(client, command, method, &url, path, query, headers, body).await
}

/// Execute a request after the caller has resolved both URL and signing path.
pub async fn execute_resolved_request(
    client: &TosClient,
    command: &str,
    method: Method,
    url: &str,
    path: &str,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
) -> Result<Envelope<RawResponseData>, CliError> {
    execute_request(client, command, method, url, path, query, headers, body).await
}

/// Execute an object-scoped request and return the raw response for download handlers.
pub async fn send_object_request(
    client: &TosClient,
    method: Method,
    bucket: &str,
    key: &str,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
) -> Result<Response, CliError> {
    let url = client.object_endpoint(bucket, key)?;
    let path = client.object_request_path(bucket, key)?;
    client
        .send_request(method, &url, &path, query, headers, body)
        .await
}

/// Execute an object-scoped request with a streaming body and return a structured response.
pub async fn execute_object_streaming_request(
    client: &TosClient,
    command: &str,
    method: Method,
    bucket: &str,
    key: &str,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    payload_hash: String,
    body: Body,
) -> Result<Envelope<RawResponseData>, CliError> {
    let url = client.object_endpoint(bucket, key)?;
    let path = client.object_request_path(bucket, key)?;
    let resp = client
        .send_streaming_request(method, &url, &path, query, headers, payload_hash, body)
        .await?;
    let request_id = extract_request_id(&resp);
    let status_code = resp.status().as_u16();
    let headers = extract_headers(&resp);
    let resp = client.check_response(resp).await?;
    let (body_format, body_value) = read_body(resp).await?;

    Ok(Envelope::success(
        command,
        RawResponseData {
            status_code,
            headers,
            body_format,
            body: body_value,
        },
    )
    .with_request_id(request_id))
}

/// [Review Fix #M4] Bucket-scoped streaming request used by form-upload-style
/// commands (`POST /{bucket}`) so they can honor the Streaming I/O hard
/// constraint when the body resolves to a local file path.
pub async fn execute_bucket_streaming_request(
    client: &TosClient,
    command: &str,
    method: Method,
    bucket: &str,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    payload_hash: String,
    body: Body,
) -> Result<Envelope<RawResponseData>, CliError> {
    let url = client.bucket_endpoint(bucket)?;
    let path = client.bucket_request_path(bucket)?;
    let resp = client
        .send_streaming_request(method, &url, &path, query, headers, payload_hash, body)
        .await?;
    let request_id = extract_request_id(&resp);
    let status_code = resp.status().as_u16();
    let headers = extract_headers(&resp);
    let resp = client.check_response(resp).await?;
    let (body_format, body_value) = read_body(resp).await?;

    Ok(Envelope::success(
        command,
        RawResponseData {
            status_code,
            headers,
            body_format,
            body: body_value,
        },
    )
    .with_request_id(request_id))
}

/// Execute a bucket-scoped request and return the raw response for streaming/list parsing handlers.
pub async fn send_bucket_request(
    client: &TosClient,
    method: Method,
    bucket: &str,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
) -> Result<Response, CliError> {
    let url = client.bucket_endpoint(bucket)?;
    let path = client.bucket_request_path(bucket)?;
    client
        .send_request(method, &url, &path, query, headers, body)
        .await
}

async fn execute_request(
    client: &TosClient,
    command: &str,
    method: Method,
    url: &str,
    path: &str,
    query: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    body: Option<Vec<u8>>,
) -> Result<Envelope<RawResponseData>, CliError> {
    let resp = client
        .send_request(method, url, path, query, headers, body)
        .await?;
    let request_id = extract_request_id(&resp);
    let status_code = resp.status().as_u16();
    let headers = extract_headers(&resp);
    let resp = client.check_response(resp).await?;
    let (body_format, body_value) = read_body(resp).await?;

    Ok(Envelope::success(
        command,
        RawResponseData {
            status_code,
            headers,
            body_format,
            body: body_value,
        },
    )
    .with_request_id(request_id))
}

async fn read_body(resp: Response) -> Result<(Option<String>, Option<Value>), CliError> {
    // [Review Fix #9] Cap raw API inline output so object downloads cannot OOM this fallback path.
    if resp.content_length().unwrap_or(0) > MAX_RAW_RESPONSE_BODY_BYTES {
        return Ok((
            Some("body_omitted".to_string()),
            Some(json!({
                "reason": "response body exceeds raw API inline output limit; use a streaming command for object data",
                "limit_bytes": MAX_RAW_RESPONSE_BODY_BYTES,
            })),
        ));
    }

    let mut resp = resp;
    let mut bytes = Vec::new();
    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => break,
            Err(err) => {
                // [Review Fix #3] Raw low-level commands should preserve a
                // successful service response even when the optional response
                // body cannot be decoded by reqwest.
                return Ok((
                    Some("body_decode_error".to_string()),
                    Some(json!({
                        "error": err.to_string(),
                        "bytes_read": bytes.len(),
                    })),
                ));
            }
        };

        if bytes.len() as u64 + chunk.len() as u64 > MAX_RAW_RESPONSE_BODY_BYTES {
            return Ok((
                Some("body_omitted".to_string()),
                Some(json!({
                    "reason": "response body exceeds raw API inline output limit; use a streaming command for object data",
                    "limit_bytes": MAX_RAW_RESPONSE_BODY_BYTES,
                    "bytes_read": bytes.len(),
                })),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    if bytes.is_empty() {
        return Ok((None, None));
    }

    let text = String::from_utf8_lossy(&bytes).to_string();

    if let Ok(parsed) = serde_json::from_slice::<Value>(&bytes) {
        // [Review Fix #FmtUni-Phase2] JSON 也走 snake_case 归一，避免下游列声明 miss。
        return Ok((Some("json".to_string()), Some(normalize_keys(parsed))));
    }

    // [Review Fix #FmtUni-Phase2] TOS 的 list/get 类 API 大量返回 XML。这里做一次
    // XML→JSON 归一：让 raw API 通道在 table/csv 视图下不再因服务端编码差异漂移；
    // 同时把 PascalCase 键名归一为 snake_case，与 `bucket list` typed handler 对齐
    // project_memory 的 Naming Convention hard constraint。
    if looks_like_xml(&bytes) {
        if let Ok(parsed) = parse_xml_to_json(&bytes) {
            return Ok((Some("xml".to_string()), Some(normalize_keys(parsed))));
        }
    }

    if std::str::from_utf8(&bytes).is_ok() {
        return Ok((
            Some("text".to_string()),
            Some(json!({
                "raw": text,
            })),
        ));
    }

    Ok((
        Some("binary".to_string()),
        Some(json!({
            "bytes": bytes.len(),
        })),
    ))
}

/// [Review Fix #FmtUni-Phase2] Heuristic XML detection that avoids false-positives
/// on UTF-8 prose that just happens to start with `<`. We require the first
/// non-whitespace byte to be `<` and the next significant character to be a
/// known XML structural marker (declaration, comment, doctype, or an ASCII
/// element name start).
fn looks_like_xml(bytes: &[u8]) -> bool {
    let mut iter = bytes
        .iter()
        .copied()
        .skip_while(|b| b.is_ascii_whitespace());
    if iter.next() != Some(b'<') {
        return false;
    }
    match iter.next() {
        Some(b'?') | Some(b'!') => true,
        Some(b) if b.is_ascii_alphabetic() => true,
        _ => false,
    }
}

/// [Review Fix #FmtUni-Phase2] Convert XML payload to a serde_json::Value tree.
/// Repeated child elements are collapsed into a JSON array so list-like XML
/// (`<Buckets><Bucket/>...`) maps cleanly to a renderable JSON array. Element
/// text content collapses to a JSON string when there are no child elements.
pub(crate) fn parse_xml_to_json(bytes: &[u8]) -> Result<Value, CliError> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);

    let mut stack: Vec<XmlNode> = Vec::new();
    let mut root: Option<Value> = None;
    let mut buf = Vec::new();

    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| CliError::Unknown(format!("xml parse error: {}", e)))?
        {
            Event::Start(start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).to_string();
                stack.push(XmlNode {
                    name,
                    children: serde_json::Map::new(),
                    text: String::new(),
                });
            }
            Event::Empty(empty) => {
                let name = String::from_utf8_lossy(empty.name().as_ref()).to_string();
                merge_xml_child(&mut stack, &mut root, name, Value::String(String::new()));
            }
            Event::Text(text) => {
                if let Some(top) = stack.last_mut() {
                    let chunk = text
                        .unescape()
                        .map_err(|e| CliError::Unknown(format!("xml decode: {}", e)))?;
                    top.text.push_str(chunk.as_ref());
                }
            }
            Event::CData(cdata) => {
                if let Some(top) = stack.last_mut() {
                    let chunk = String::from_utf8_lossy(cdata.as_ref()).into_owned();
                    top.text.push_str(&chunk);
                }
            }
            Event::End(_) => {
                let Some(node) = stack.pop() else {
                    break;
                };
                let value = if node.children.is_empty() {
                    Value::String(node.text)
                } else {
                    Value::Object(node.children)
                };
                merge_xml_child(&mut stack, &mut root, node.name, value);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(root.unwrap_or(Value::Null))
}

struct XmlNode {
    name: String,
    children: serde_json::Map<String, Value>,
    text: String,
}

/// [Review Fix #FmtUni-Phase2] Attach a finished element to its parent (or to
/// the document root). Repeated keys collapse into a JSON array so e.g.
/// `<Contents/><Contents/>` becomes `{"Contents":[..,..]}`.
fn merge_xml_child(stack: &mut Vec<XmlNode>, root: &mut Option<Value>, name: String, value: Value) {
    if let Some(parent) = stack.last_mut() {
        match parent.children.get_mut(&name) {
            Some(Value::Array(arr)) => arr.push(value),
            Some(existing) => {
                let prev = std::mem::replace(existing, Value::Null);
                *existing = Value::Array(vec![prev, value]);
            }
            None => {
                parent.children.insert(name, value);
            }
        }
    } else {
        // Document root: TOS responses always have a single root element.
        *root = Some(value);
    }
}

/// [Review Fix #FmtUni-Phase2] Recursively normalize JSON object keys to
/// snake_case so list/get-style raw API outputs stay compliant with the
/// project's Naming Convention hard constraint regardless of whether the
/// upstream API returned PascalCase XML or PascalCase JSON.
pub(crate) fn normalize_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(to_snake_case(&k), normalize_keys(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(normalize_keys).collect()),
        other => other,
    }
}

/// [Review Fix #FmtUni-Phase2] PascalCase / camelCase / kebab-case → snake_case.
/// Idempotent on already-snake_case keys. Preserves all-uppercase acronyms
/// (e.g. `ID` → `id`, `ETag` → `e_tag` is acceptable; we keep it deterministic
/// rather than try to be clever about acronym detection).
fn to_snake_case(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(input.len() + 4);
    let mut chars = input.chars().peekable();
    let mut prev_lower_or_digit = false;
    while let Some(c) = chars.next() {
        if c == '-' || c == ' ' {
            if !out.ends_with('_') {
                out.push('_');
            }
            prev_lower_or_digit = false;
            continue;
        }
        if c.is_uppercase() {
            // Insert separator if previous was lowercase/digit, or if a lowercase
            // follows (handles "URLPath" → "url_path").
            let next_is_lower = chars.peek().map(|n| n.is_lowercase()).unwrap_or(false);
            if !out.is_empty() && !out.ends_with('_') && (prev_lower_or_digit || next_is_lower) {
                out.push('_');
            }
            for lc in c.to_lowercase() {
                out.push(lc);
            }
            prev_lower_or_digit = false;
        } else {
            out.push(c);
            prev_lower_or_digit = c.is_lowercase() || c.is_ascii_digit();
        }
    }
    out
}

pub fn extract_request_id(resp: &Response) -> String {
    resp.headers()
        .get("x-tos-request-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string()
}

pub fn extract_headers(resp: &Response) -> BTreeMap<String, String> {
    resp.headers()
        .iter()
        .filter_map(|(key, value)| {
            value.to_str().ok().map(|text| {
                let key = key.to_string();
                let value = if is_sensitive_header(&key) {
                    "***REDACTED***".to_string()
                } else {
                    text.to_string()
                };
                (key, value)
            })
        })
        .collect()
}

fn is_sensitive_header(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    if lower == "x-tos-turbo-token" {
        // [Review Fix #TurboTokenOutput] OpenTurbo returns this short-lived
        // session token as the public handoff value for AppendTurbo. Keep
        // broader credential/token redaction below for all other headers.
        return false;
    }
    // [Review Fix #m5] Expanded coverage so AK/SK and presigned-URL signature
    // material never leak through `extract_headers`. We deliberately err on
    // the side of redacting any header whose name hints at credentials,
    // tokens, signatures, or session material — false positives only mask
    // values, never expose them.
    [
        "authorization",
        "token",
        "secret",
        "credential",
        "cookie",
        "access-key",
        "access_key",
        "accesskey",
        "session",
        "signature",
        "x-amz-signature",
        "x-tos-signature",
        "x-amz-security-token",
        "x-tos-security-token",
        "x-amz-credential",
        "password",
        "passwd",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn to_snake_case_handles_pascal_camel_kebab_and_idempotent_snake() {
        assert_eq!(to_snake_case("PascalCase"), "pascal_case");
        assert_eq!(to_snake_case("camelCase"), "camel_case");
        assert_eq!(to_snake_case("kebab-case"), "kebab_case");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
        assert_eq!(to_snake_case("URLPath"), "url_path");
        assert_eq!(to_snake_case("ID"), "id");
    }

    #[test]
    fn normalize_keys_recursively_snake_cases_object_and_array_keys() {
        let input = json!({
            "BucketName": "b",
            "ContentList": [
                { "ObjectKey": "k1", "SizeBytes": 10 },
                { "ObjectKey": "k2", "SizeBytes": 20 },
            ],
        });
        let out = normalize_keys(input);
        assert_eq!(out["bucket_name"], "b");
        assert_eq!(out["content_list"][0]["object_key"], "k1");
        assert_eq!(out["content_list"][1]["size_bytes"], 20);
    }

    #[test]
    fn parse_xml_to_json_collapses_repeated_children_into_array() {
        let xml = br#"<?xml version="1.0"?>
<ListBucketResult>
    <Name>bucket-a</Name>
    <Contents><Key>k1</Key><Size>10</Size></Contents>
    <Contents><Key>k2</Key><Size>20</Size></Contents>
</ListBucketResult>"#;
        let v = parse_xml_to_json(xml).expect("parse");
        let arr = v["Contents"].as_array().expect("Contents is array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["Key"], "k1");
        assert_eq!(v["Name"], "bucket-a");
    }

    #[test]
    fn looks_like_xml_recognizes_decl_and_element_starts_only() {
        assert!(looks_like_xml(b"<?xml version=\"1.0\"?><Root/>"));
        assert!(looks_like_xml(b"   <Root/>"));
        assert!(!looks_like_xml(b"{\"k\":1}"));
        assert!(!looks_like_xml(b"plain text < still text"));
        assert!(!looks_like_xml(b""));
    }

    #[test]
    fn sensitive_header_redaction_keeps_turbo_session_token_usable() {
        assert!(!is_sensitive_header("x-tos-turbo-token"));
        assert!(is_sensitive_header("x-tos-security-token"));
        assert!(is_sensitive_header("authorization"));
    }
}
