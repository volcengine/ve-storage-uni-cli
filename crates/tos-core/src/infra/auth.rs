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

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;

type HmacSha256 = Hmac<Sha256>;

/// Request signing algorithm used by a TOS client surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TosSignAlgorithm {
    Tos4,
    ByteTosV1,
}

/// TOS V4 签名器
pub struct V4Signer {
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
    pub service: String,
    pub security_token: Option<String>,
}

impl V4Signer {
    pub fn new(access_key: String, secret_key: String, region: String, service: String) -> Self {
        Self {
            access_key,
            secret_key,
            region,
            service,
            security_token: None,
        }
    }

    pub fn with_security_token(mut self, token: String) -> Self {
        self.security_token = Some(token);
        self
    }

    /// 对 HTTP 请求进行签名，返回 Authorization header 值
    /// 同时返回需要附加的请求头列表
    pub fn sign_request(
        &self,
        method: &str,
        uri: &str,
        query_params: &BTreeMap<String, String>,
        headers: &BTreeMap<String, String>,
        payload_hash: &str,
    ) -> SignedRequest {
        let now = Utc::now();
        let date_str = now.format("%Y%m%d").to_string();
        let datetime_str = now.format("%Y%m%dT%H%M%SZ").to_string();

        let credential_scope = format!("{}/{}/{}/request", date_str, self.region, self.service);

        // 构建需要签名的 headers（包含 host 和 x-tos-date）
        let mut sign_headers = headers.clone();
        sign_headers.insert("x-tos-date".to_string(), datetime_str.clone());
        if let Some(ref token) = self.security_token {
            sign_headers.insert("x-tos-security-token".to_string(), token.clone());
        }
        sign_headers.insert("x-tos-content-sha256".to_string(), payload_hash.to_string());

        // Canonical Headers（按key排序，小写）
        let signed_header_keys: Vec<String> =
            sign_headers.keys().map(|k| k.to_lowercase()).collect();
        let signed_headers_str = signed_header_keys.join(";");

        let canonical_headers: String = sign_headers
            .iter()
            .map(|(k, v)| format!("{}:{}", k.to_lowercase(), v.trim()))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";

        // Canonical Query String
        let canonical_query = if query_params.is_empty() {
            String::new()
        } else {
            query_params
                .iter()
                .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
                .collect::<Vec<_>>()
                .join("&")
        };

        // Canonical Request
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method,
            uri_encode(uri),
            canonical_query,
            canonical_headers,
            signed_headers_str,
            payload_hash,
        );

        // StringToSign
        let canonical_request_hash = sha256_hex(canonical_request.as_bytes());
        let string_to_sign = format!(
            "TOS4-HMAC-SHA256\n{}\n{}\n{}",
            datetime_str, credential_scope, canonical_request_hash
        );

        // Signing Key
        let signing_key = self.derive_signing_key(&date_str);

        // Signature
        let signature = hmac_sha256_hex(&signing_key, string_to_sign.as_bytes());

        // Authorization Header
        let authorization = format!(
            "TOS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, credential_scope, signed_headers_str, signature
        );

        SignedRequest {
            authorization,
            date: datetime_str,
            content_sha256: payload_hash.to_string(),
            security_token: self.security_token.clone(),
            additional_headers: sign_headers,
        }
    }

    /// Build query parameters for a TOS4 presigned URL.
    pub fn presign_query(
        &self,
        method: &str,
        uri: &str,
        query_params: &BTreeMap<String, String>,
        headers: &BTreeMap<String, String>,
        expires: u64,
    ) -> BTreeMap<String, String> {
        let now = Utc::now();
        let date_str = now.format("%Y%m%d").to_string();
        let datetime_str = now.format("%Y%m%dT%H%M%SZ").to_string();
        let credential_scope = format!("{}/{}/{}/request", date_str, self.region, self.service);

        let mut sign_headers = headers.clone();
        if let Some(ref token) = self.security_token {
            sign_headers.insert("x-tos-security-token".to_string(), token.clone());
        }
        let signed_header_keys: Vec<String> =
            sign_headers.keys().map(|key| key.to_lowercase()).collect();
        let signed_headers_str = signed_header_keys.join(";");

        let mut presign_query = query_params.clone();
        presign_query.insert(
            "X-Tos-Algorithm".to_string(),
            "TOS4-HMAC-SHA256".to_string(),
        );
        presign_query.insert(
            "X-Tos-Credential".to_string(),
            format!("{}/{}", self.access_key, credential_scope),
        );
        presign_query.insert("X-Tos-Date".to_string(), datetime_str.clone());
        presign_query.insert("X-Tos-Expires".to_string(), expires.to_string());
        presign_query.insert(
            "X-Tos-SignedHeaders".to_string(),
            signed_headers_str.clone(),
        );
        if let Some(ref token) = self.security_token {
            presign_query.insert("X-Tos-Security-Token".to_string(), token.clone());
        }

        let canonical_headers: String = sign_headers
            .iter()
            .map(|(key, value)| format!("{}:{}", key.to_lowercase(), value.trim()))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let canonical_query = presign_query
            .iter()
            .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method,
            uri_encode(uri),
            canonical_query,
            canonical_headers,
            signed_headers_str,
            "UNSIGNED-PAYLOAD",
        );
        let string_to_sign = format!(
            "TOS4-HMAC-SHA256\n{}\n{}\n{}",
            datetime_str,
            credential_scope,
            sha256_hex(canonical_request.as_bytes())
        );
        let signing_key = self.derive_signing_key(&date_str);
        presign_query.insert(
            "X-Tos-Signature".to_string(),
            hmac_sha256_hex(&signing_key, string_to_sign.as_bytes()),
        );
        presign_query
    }

    /// Prepare form signature fields (credential, date, algorithm) for PostObject policy construction.
    pub fn form_prepare(&self) -> FormPrepare {
        let now = Utc::now();
        let date_str = now.format("%Y%m%d").to_string();
        let datetime_str = now.format("%Y%m%dT%H%M%SZ").to_string();
        let credential = format!(
            "{}/{}/{}/{}/request",
            self.access_key, date_str, self.region, self.service
        );
        FormPrepare {
            credential,
            algorithm: "TOS4-HMAC-SHA256".to_string(),
            date: datetime_str,
            date_short: date_str,
            security_token: self.security_token.clone(),
        }
    }

    /// Compute the actual HMAC-SHA256 signature over a base64-encoded policy string.
    pub fn form_sign(&self, date_short: &str, policy_base64: &str) -> String {
        let signing_key = self.derive_signing_key(date_short);
        hmac_sha256_hex(&signing_key, policy_base64.as_bytes())
    }

    fn derive_signing_key(&self, date: &str) -> Vec<u8> {
        let k_date = hmac_sha256(self.secret_key.as_bytes(), date.as_bytes());
        let k_region = hmac_sha256(&k_date, self.region.as_bytes());
        let k_service = hmac_sha256(&k_region, self.service.as_bytes());
        hmac_sha256(&k_service, b"request")
    }
}

/// ByteCloud TOS V1 signer.
///
/// This follows the local ByteCloud TOS Rust SDK signer semantics:
/// canonical path is `/<bucket>/<key>`, header signing writes
/// `X-Tos-Signature`, and the HMAC digest is URL-safe base64 truncated to
/// 30 bytes.
pub struct V1Signer {
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
    pub service: String,
    pub security_token: Option<String>,
}

impl V1Signer {
    const MAX_SIGNATURE_LEN: usize = 30;
    const REQUEST: &'static str = "sig_request";
    const SIGNATURE_LIFETIME_SECS: i64 = 3600;
    const HEADER_SIGNATURE: &'static str = "X-Tos-Signature";
    const HEADER_COPY_SIGNATURE: &'static str = "X-Tos-Copy-Signature";
    const SIGNING_HEADERS: &'static [&'static str] =
        &["range", "x-tos-copy-source", "x-tos-copy-destination"];
    const NOT_SIGNING_QUERIES: &'static [&'static str] = &[
        "timeout",
        "tos-algorithm",
        "tos-expiration",
        "tos-signature",
        "tos-signame",
        "tos-signname",
        "tos-credentials",
        "tos-credential",
    ];

    pub fn new(access_key: String, secret_key: String, region: String, service: String) -> Self {
        Self {
            access_key,
            secret_key,
            region,
            service,
            security_token: None,
        }
    }

    pub fn with_security_token(mut self, token: String) -> Self {
        self.security_token = Some(token);
        self
    }

    pub fn sign_request(
        &self,
        method: &str,
        uri: &str,
        query_params: &BTreeMap<String, String>,
        headers: &BTreeMap<String, String>,
    ) -> V1SignedRequest {
        let now = Utc::now();
        let expired_at = now.timestamp() + Self::SIGNATURE_LIFETIME_SECS;
        let signed_headers = self.signed_headers(headers);
        let signed_query = self.signed_query(query_params);
        let signature = self.do_sign(method, uri, "", &signed_headers, &signed_query, expired_at);
        let credential = self.credential(&now.format("%Y%m%d").to_string());
        let authorization = format!(
            "TOS-HMAC-SHA256 expiration={},signame=,signature={},credentials={}",
            expired_at, signature, credential
        );
        V1SignedRequest {
            signature_header: Self::HEADER_SIGNATURE.to_string(),
            authorization,
            security_token: self.security_token.clone(),
        }
    }

    /// Sign a ByteTOS V1 CopyObject source path.
    ///
    /// `method` is the object copy method, and `copy_path` must be the encoded
    /// value that will be sent in `x-tos-copy-source`. The returned signature
    /// is carried in `X-Tos-Copy-Signature`.
    pub fn sign_copy_source(&self, method: &str, copy_path: &str) -> V1SignedRequest {
        let now = Utc::now();
        let expired_at = now.timestamp() + Self::SIGNATURE_LIFETIME_SECS;
        let signature = self.do_sign(method, copy_path, "", &[], &[], expired_at);
        let credential = self.credential(&now.format("%Y%m%d").to_string());
        let authorization = format!(
            "TOS-HMAC-SHA256 expiration={},signame=,signature={},credentials={}",
            expired_at, signature, credential
        );
        V1SignedRequest {
            signature_header: Self::HEADER_COPY_SIGNATURE.to_string(),
            authorization,
            security_token: None,
        }
    }

    pub fn presign_query(
        &self,
        method: &str,
        uri: &str,
        query_params: &BTreeMap<String, String>,
        expires: u64,
    ) -> BTreeMap<String, String> {
        let now = Utc::now();
        let expired_at = now.timestamp() + expires as i64;
        let mut query = query_params.clone();
        query.insert("tos-algorithm".to_string(), "TOS-HMAC-SHA256".to_string());
        query.insert("tos-expiration".to_string(), expired_at.to_string());
        // [Review Fix #5] ByteTOS V1 SignQuery uses the service-recognized
        // `tos-signame`/`tos-credentials` parameters; the similarly named
        // `tos-signname`/`tos-credential` pair is rejected by the server.
        query.insert("tos-signame".to_string(), String::new());
        query.insert(
            "tos-credentials".to_string(),
            self.credential(&now.format("%Y%m%d").to_string()),
        );
        let signed_query = self.signed_query(&query);
        let signature = self.do_sign(method, uri, "", &[], &signed_query, expired_at);
        query.insert("tos-signature".to_string(), signature);
        if let Some(token) = &self.security_token {
            query.insert("x-tos-security-token".to_string(), token.clone());
        }
        query
    }

    fn credential(&self, date: &str) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            self.access_key,
            date,
            self.region,
            self.service,
            Self::REQUEST
        )
    }

    fn signed_headers(&self, headers: &BTreeMap<String, String>) -> Vec<(String, String)> {
        let mut signed = headers
            .iter()
            .filter_map(|(key, value)| {
                let lower_key = key.to_lowercase();
                Self::SIGNING_HEADERS
                    .contains(&lower_key.as_str())
                    .then(|| (lower_key, value.clone()))
            })
            .collect::<Vec<_>>();
        signed.sort_by(|left, right| left.0.cmp(&right.0));
        signed
    }

    fn signed_query(&self, query: &BTreeMap<String, String>) -> Vec<(String, String)> {
        query
            .iter()
            .filter_map(|(key, value)| {
                // [Review Fix #2] ByteTOS V1 signs the raw query values used by
                // the legacy `tos` request path; do not reuse TOS4 query
                // percent-encoding here or basic list requests can fail auth.
                (!Self::NOT_SIGNING_QUERIES.contains(&key.as_str()))
                    .then(|| (key.clone(), value.clone()))
            })
            .collect()
    }

    fn do_sign(
        &self,
        method: &str,
        path: &str,
        name: &str,
        headers: &[(String, String)],
        query: &[(String, String)],
        expired_at: i64,
    ) -> String {
        let mut canonical = Vec::with_capacity(1024);
        canonical.extend_from_slice(method.as_bytes());
        canonical.push(b'\n');
        canonical.extend_from_slice(path.as_bytes());
        canonical.push(b'\n');

        if !query.is_empty() {
            let mut sorted_query = query.to_vec();
            sorted_query.sort_by(|left, right| left.0.cmp(&right.0));
            for (index, (key, value)) in sorted_query.iter().enumerate() {
                if index > 0 {
                    canonical.push(b'&');
                }
                canonical.extend_from_slice(key.as_bytes());
                canonical.push(b'=');
                canonical.extend_from_slice(value.as_bytes());
            }
            canonical.push(b'\n');
        }

        if !headers.is_empty() {
            for (index, (key, value)) in headers.iter().enumerate() {
                if index > 0 {
                    canonical.push(b'\n');
                }
                canonical.extend_from_slice(key.to_lowercase().as_bytes());
                canonical.push(b':');
                canonical.extend_from_slice(value.as_bytes());
            }
            canonical.push(b'\n');
        }

        canonical.extend_from_slice(name.as_bytes());
        canonical.push(b'\n');
        canonical.extend_from_slice(expired_at.to_string().as_bytes());

        let signature = hmac_sha256(self.secret_key.as_bytes(), &canonical);
        let mut encoded = URL_SAFE_NO_PAD.encode(signature);
        if encoded.len() > Self::MAX_SIGNATURE_LEN {
            encoded.truncate(Self::MAX_SIGNATURE_LEN);
        }
        encoded
    }
}

#[derive(Debug, Clone)]
pub struct V1SignedRequest {
    pub signature_header: String,
    pub authorization: String,
    pub security_token: Option<String>,
}

/// Result of form_prepare: credential, date, algorithm for building PostObject policy.
#[derive(Debug, Clone)]
pub struct FormPrepare {
    pub credential: String,
    pub algorithm: String,
    pub date: String,
    pub date_short: String,
    pub security_token: Option<String>,
}

#[derive(Debug)]
pub struct SignedRequest {
    pub authorization: String,
    pub date: String,
    pub content_sha256: String,
    pub security_token: Option<String>,
    pub additional_headers: BTreeMap<String, String>,
}

/// URL encode (RFC 3986)
pub(crate) fn url_encode(s: &str) -> String {
    url_encode_with_safe(s, "")
}

/// URL encode while preserving additional ASCII-safe bytes supplied by caller.
pub(crate) fn url_encode_with_safe(s: &str, safe: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ if safe.as_bytes().contains(&byte) => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

/// URI path encode（保留 /）
fn uri_encode(uri: &str) -> String {
    if uri.is_empty() {
        return "/".to_string();
    }
    uri.split('/')
        .map(|segment| url_encode(segment))
        .collect::<Vec<_>>()
        .join("/")
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key length error");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    hex::encode(hmac_sha256(key, data))
}

/// 计算请求体的 SHA256
pub fn hash_payload(payload: &[u8]) -> String {
    sha256_hex(payload)
}

/// 流式计算请求体 SHA256，避免为签名把大文件完整读入内存。
pub fn hash_reader<R: Read>(reader: &mut R) -> Result<String, std::io::Error> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 空请求体的 SHA256 (常量)
pub const EMPTY_PAYLOAD_HASH: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_encode() {
        assert_eq!(url_encode("hello world"), "hello%20world");
        assert_eq!(url_encode("key=value&foo=bar"), "key%3Dvalue%26foo%3Dbar");
        assert_eq!(url_encode("simple"), "simple");
    }

    #[test]
    fn test_uri_encode() {
        assert_eq!(uri_encode("/bucket/key"), "/bucket/key");
        assert_eq!(
            uri_encode("/bucket/path with space"),
            "/bucket/path%20with%20space"
        );
        assert_eq!(uri_encode(""), "/");
    }

    #[test]
    fn test_sha256_hex() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_empty_payload_hash() {
        assert_eq!(hash_payload(b""), EMPTY_PAYLOAD_HASH);
    }

    #[test]
    fn test_sign_request_produces_valid_format() {
        let signer = V4Signer::new(
            "TEST_ACCESS_KEY_ID".to_string(),
            "TEST_SIGNING_KEY".to_string(),
            "cn-beijing".to_string(),
            "tos".to_string(),
        );
        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), "tos-cn-beijing.volces.com".to_string());
        let query = BTreeMap::new();
        let result = signer.sign_request("GET", "/", &query, &headers, EMPTY_PAYLOAD_HASH);
        assert!(result
            .authorization
            .starts_with("TOS4-HMAC-SHA256 Credential=TEST_ACCESS_KEY_ID/"));
        assert!(result.authorization.contains("SignedHeaders="));
        assert!(result.authorization.contains("Signature="));
    }

    #[test]
    fn test_sign_request_with_security_token() {
        let signer = V4Signer::new(
            "TEST_ACCESS_KEY_ID".to_string(),
            "TEST_SIGNING_KEY".to_string(),
            "cn-beijing".to_string(),
            "tos".to_string(),
        )
        .with_security_token("test-token".to_string());
        let mut headers = BTreeMap::new();
        headers.insert("host".to_string(), "tos-cn-beijing.volces.com".to_string());
        let query = BTreeMap::new();
        let result = signer.sign_request("GET", "/", &query, &headers, EMPTY_PAYLOAD_HASH);
        assert_eq!(result.security_token, Some("test-token".to_string()));
    }

    #[test]
    fn test_v1_sign_request_uses_byted_tos_header_format() {
        let signer = V1Signer::new(
            "ak".to_string(),
            "sk".to_string(),
            "cn-boe".to_string(),
            "tos".to_string(),
        );
        let mut query = BTreeMap::new();
        query.insert("list-type".to_string(), "2".to_string());
        let headers = BTreeMap::new();

        let result = signer.sign_request("GET", "/bucket", &query, &headers);

        assert_eq!(result.signature_header, "X-Tos-Signature");
        assert!(result
            .authorization
            .starts_with("TOS-HMAC-SHA256 expiration="));
        assert!(result.authorization.contains("credentials=ak/"));
        assert!(result.authorization.contains("/cn-boe/tos/sig_request"));
    }

    #[test]
    fn test_v1_copy_source_signature_uses_byted_tos_header_format() {
        let signer = V1Signer::new(
            "ak".to_string(),
            "sk".to_string(),
            "cn-boe".to_string(),
            "tos".to_string(),
        );

        let result = signer.sign_copy_source("POST", "%2Fbucket%2Fdir%2Fa.txt");

        assert_eq!(result.signature_header, "X-Tos-Copy-Signature");
        assert!(result
            .authorization
            .starts_with("TOS-HMAC-SHA256 expiration="));
        assert!(result.authorization.contains("credentials=ak/"));
        assert!(result.authorization.contains("/cn-boe/tos/sig_request"));
    }

    #[test]
    fn test_v1_sign_request_preserves_raw_query_values_for_bytetos() {
        let signer = V1Signer::new(
            "ak".to_string(),
            "sk".to_string(),
            "cn-boe".to_string(),
            "tos".to_string(),
        );
        let mut query = BTreeMap::new();
        query.insert("delimiter".to_string(), "/".to_string());
        query.insert("list-type".to_string(), "2".to_string());

        let result = signer.sign_request("GET", "/bucket", &query, &BTreeMap::new());
        let expiration = result
            .authorization
            .split("expiration=")
            .nth(1)
            .and_then(|tail| tail.split(',').next())
            .and_then(|value| value.parse::<i64>().ok())
            .expect("expiration");
        let expected_signature = signer.do_sign(
            "GET",
            "/bucket",
            "",
            &[],
            &[
                ("delimiter".to_string(), "/".to_string()),
                ("list-type".to_string(), "2".to_string()),
            ],
            expiration,
        );

        assert!(
            result
                .authorization
                .contains(&format!("signature={expected_signature}")),
            "authorization={}",
            result.authorization
        );
    }

    #[test]
    fn test_v1_presign_uses_tos_query_names() {
        let signer = V1Signer::new(
            "ak".to_string(),
            "sk".to_string(),
            "cn-boe".to_string(),
            "tos".to_string(),
        );
        let query = signer.presign_query("GET", "/bucket/key", &BTreeMap::new(), 3600);

        assert_eq!(
            query.get("tos-algorithm").map(String::as_str),
            Some("TOS-HMAC-SHA256")
        );
        assert!(query.contains_key("tos-expiration"));
        assert!(query.contains_key("tos-signature"));
        assert!(query.contains_key("tos-signame"));
        assert!(query.contains_key("tos-credentials"));
    }

    #[test]
    fn test_v1_presign_uses_service_sign_query_names() {
        let signer = V1Signer::new(
            "ak".to_string(),
            "sk".to_string(),
            "cn-boe".to_string(),
            "tos".to_string(),
        );
        let query = signer.presign_query("GET", "/bucket/key", &BTreeMap::new(), 3600);

        assert_eq!(
            query.get("tos-algorithm").map(String::as_str),
            Some("TOS-HMAC-SHA256")
        );
        assert!(query.contains_key("tos-expiration"));
        assert!(query.contains_key("tos-signature"));
        assert!(query.contains_key("tos-signame"));
        assert!(query.contains_key("tos-credentials"));
        assert!(!query.contains_key("tos-signname"));
        assert!(!query.contains_key("tos-credential"));
    }
}
