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

use crate::agent::error::CliError;
use crate::infra::auth::{
    hash_payload, url_encode, url_encode_with_safe, FormPrepare, TosSignAlgorithm, V1Signer,
    V4Signer, EMPTY_PAYLOAD_HASH,
};
use crate::infra::config::{
    derive_tos_control_endpoint, Binary, Profile, DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS,
    DEFAULT_HTTP_MAX_CONNECTIONS, DEFAULT_HTTP_MAX_RETRY_COUNT,
    DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS,
};
use crate::infra::discovery::{PsmDiscoveryConfig, PsmResolver};
use reqwest::{Body, Client, Method, Response, StatusCode};
use serde_json::Value;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

pub const USER_AGENT_NAME_ENV: &str = "VE_STORAGE_UNI_USER_AGENT_NAME";
const TOS_CONFIG_BINARY_ENV: &str = "VE_STORAGE_UNI_TOS_CONFIG_BINARY";

/// Build the canonical HTTP User-Agent string for the active top-level binary.
pub fn storage_user_agent() -> String {
    let name = std::env::var(USER_AGENT_NAME_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "ve-storage-uni-cli".to_string());
    storage_user_agent_for_name(&name)
}

pub fn storage_user_agent_for_name(name: &str) -> String {
    format!("{}/v{}", name, env!("CARGO_PKG_VERSION"))
}

/// TOS API 的 Endpoint 规则
pub fn build_endpoint(region: &str, service: &str) -> String {
    match service {
        "tos" => format!("https://tos-{}.volces.com", region),
        "tosvectors" => format!("https://tosvectors-{}.volces.com", region),
        "tostables" => format!("https://tostables-{}.volces.com", region),
        "ids" => format!("https://ids-{}.volces.com", region),
        _ => format!("https://{}-{}.volces.com", service, region),
    }
}

/// Validate a TOS bucket name before it is inserted into a host or request path.
///
/// Bucket names must be 3-63 characters, use only lowercase letters, digits, and
/// hyphens, and cannot start or end with a hyphen.
pub fn validate_bucket_name(bucket: &str) -> Result<(), CliError> {
    if bucket.len() < 3 || bucket.len() > 63 {
        return Err(CliError::ValidationError(
            "invalid bucket name, the length must be [3, 63]".to_string(),
        ));
    }
    if bucket.starts_with('-') || bucket.ends_with('-') {
        return Err(CliError::ValidationError(
            "invalid bucket name, the bucket name can be neither starting with '-' nor ending with '-'"
                .to_string(),
        ));
    }
    if !bucket
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(CliError::ValidationError(
            "invalid bucket name, the character set is illegal".to_string(),
        ));
    }
    Ok(())
}

/// Build a bucket-level endpoint after validating the bucket name.
///
/// Returns the virtual-hosted endpoint for the bucket. Returns a
/// `ValidationError` if the bucket name is not a legal TOS bucket name.
pub fn build_bucket_endpoint(bucket: &str, region: &str) -> Result<String, CliError> {
    validate_bucket_name(bucket)?;
    Ok(format!("https://{}.tos-{}.volces.com", bucket, region))
}

enum TosSigner {
    V4(V4Signer),
    V1(V1Signer),
}

impl TosSigner {
    fn new(
        algorithm: TosSignAlgorithm,
        access_key: String,
        secret_key: String,
        region: String,
        service: String,
    ) -> Self {
        match algorithm {
            TosSignAlgorithm::Tos4 => {
                Self::V4(V4Signer::new(access_key, secret_key, region, service))
            }
            TosSignAlgorithm::ByteTosV1 => {
                Self::V1(V1Signer::new(access_key, secret_key, region, service))
            }
        }
    }

    fn with_security_token(self, token: String) -> Self {
        match self {
            Self::V4(signer) => Self::V4(signer.with_security_token(token)),
            Self::V1(signer) => Self::V1(signer.with_security_token(token)),
        }
    }

    fn form_prepare(&self) -> FormPrepare {
        match self {
            Self::V4(signer) => signer.form_prepare(),
            // PostObject is a V4 form-signing contract in this CLI. The new
            // ByteCloud high-level `tos` surface does not expose PostObject.
            Self::V1(_) => FormPrepare {
                credential: String::new(),
                algorithm: "TOS-HMAC-SHA256".to_string(),
                date: String::new(),
                date_short: String::new(),
                security_token: None,
            },
        }
    }

    fn form_sign(&self, date_short: &str, policy_base64: &str) -> String {
        match self {
            Self::V4(signer) => signer.form_sign(date_short, policy_base64),
            Self::V1(_) => String::new(),
        }
    }

    fn presign_query(
        &self,
        method: &str,
        path: &str,
        query: &BTreeMap<String, String>,
        headers: &BTreeMap<String, String>,
        expires: u64,
    ) -> BTreeMap<String, String> {
        match self {
            Self::V4(signer) => signer.presign_query(method, path, query, headers, expires),
            Self::V1(signer) => signer.presign_query(method, path, query, expires),
        }
    }

    fn sign_request(
        &self,
        method: &str,
        path: &str,
        query: &BTreeMap<String, String>,
        headers: &BTreeMap<String, String>,
        payload_hash: &str,
    ) -> ClientSignedRequest {
        match self {
            Self::V4(signer) => {
                let signed = signer.sign_request(method, path, query, headers, payload_hash);
                let mut headers = BTreeMap::from([
                    ("Authorization".to_string(), signed.authorization),
                    ("x-tos-date".to_string(), signed.date),
                    ("x-tos-content-sha256".to_string(), signed.content_sha256),
                ]);
                if let Some(token) = signed.security_token {
                    headers.insert("x-tos-security-token".to_string(), token);
                }
                ClientSignedRequest { headers }
            }
            Self::V1(signer) => {
                let signed = signer.sign_request(method, path, query, headers);
                let mut headers = BTreeMap::from([(signed.signature_header, signed.authorization)]);
                if let Some(token) = signed.security_token {
                    headers.insert("x-tos-security-token".to_string(), token);
                }
                ClientSignedRequest { headers }
            }
        }
    }

    fn sign_copy_source(&self, method: &str, copy_path: &str) -> Option<ClientSignedRequest> {
        match self {
            Self::V4(_) => None,
            Self::V1(signer) => {
                let signed = signer.sign_copy_source(method, copy_path);
                Some(ClientSignedRequest {
                    headers: BTreeMap::from([(signed.signature_header, signed.authorization)]),
                })
            }
        }
    }
}

struct ClientSignedRequest {
    headers: BTreeMap<String, String>,
}

fn active_tos_sign_algorithm() -> TosSignAlgorithm {
    std::env::var(TOS_CONFIG_BINARY_ENV)
        .ok()
        .as_deref()
        .and_then(Binary::parse)
        .map(|binary| match binary {
            Binary::Tos => TosSignAlgorithm::ByteTosV1,
            _ => TosSignAlgorithm::Tos4,
        })
        .unwrap_or(TosSignAlgorithm::Tos4)
}

/// TOS HTTP Client
pub struct TosClient {
    http: Client,
    signer: TosSigner,
    sign_algorithm: TosSignAlgorithm,
    region: String,
    endpoint: Option<String>,
    psm_resolver: Option<Arc<PsmResolver>>,
    control_endpoint: Option<String>,
    account_id: Option<String>,
    service: String,
    max_retry_count: u32,
}

impl TosClient {
    pub fn new(profile: &Profile, service: &str) -> Result<Self, CliError> {
        Self::new_with_sign_algorithm(profile, service, active_tos_sign_algorithm())
    }

    fn new_with_sign_algorithm(
        profile: &Profile,
        service: &str,
        sign_algorithm: TosSignAlgorithm,
    ) -> Result<Self, CliError> {
        let region = profile
            .region
            .clone()
            .or_else(|| {
                profile
                    .endpoint
                    .as_deref()
                    .and_then(derive_region_from_endpoint)
            })
            .ok_or_else(|| CliError::ConfigMissing("region is required".to_string()))?;
        let access_key = profile
            .access_key_id
            .as_deref()
            .ok_or_else(|| CliError::ConfigMissing("access_key_id is required".to_string()))?;
        let secret_key = profile
            .secret_access_key
            .as_deref()
            .ok_or_else(|| CliError::ConfigMissing("secret_access_key is required".to_string()))?;

        let mut signer = TosSigner::new(
            sign_algorithm,
            access_key.to_string(),
            secret_key.to_string(),
            region.clone(),
            service.to_string(),
        );
        if let Some(ref token) = profile.security_token {
            signer = signer.with_security_token(token.clone());
        }
        let endpoint = profile.endpoint.as_deref().map(normalize_endpoint_scheme);
        let psm_resolver =
            build_psm_resolver(profile, service, sign_algorithm, endpoint.is_none())?;

        Ok(Self {
            http: Client::builder()
                .user_agent(storage_user_agent())
                .tcp_nodelay(true)
                .connect_timeout(Duration::from_secs(
                    profile
                        .connecttimeout
                        .unwrap_or(DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS),
                ))
                .timeout(Duration::from_secs(
                    profile
                        .requesttimeout
                        .unwrap_or(DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS),
                ))
                .pool_max_idle_per_host(
                    profile
                        .maxconnections
                        .unwrap_or(DEFAULT_HTTP_MAX_CONNECTIONS),
                )
                .build()
                .map_err(CliError::Http)?,
            signer,
            sign_algorithm,
            region,
            endpoint,
            psm_resolver,
            control_endpoint: profile
                .control_endpoint
                .as_deref()
                .map(normalize_endpoint_scheme),
            account_id: profile.account_id.clone(),
            service: service.to_string(),
            max_retry_count: profile
                .max_retry_count
                .unwrap_or(DEFAULT_HTTP_MAX_RETRY_COUNT),
        })
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    /// 获取 account_id（用于 control plane 请求的 X-Tos-Account-Id header）。
    pub fn account_id(&self) -> Option<&str> {
        self.account_id.as_deref()
    }

    /// 获取服务级别 endpoint
    pub fn service_endpoint(&self) -> String {
        self.endpoint
            .clone()
            .unwrap_or_else(|| build_endpoint(&self.region, &self.service))
    }

    /// 获取 control plane endpoint。
    pub fn control_endpoint(&self) -> Result<String, CliError> {
        let base_endpoint = if let Some(endpoint) = self.control_endpoint.clone() {
            endpoint
        } else {
            let data_endpoint = self.service_endpoint();
            derive_tos_control_endpoint(Some(&data_endpoint)).ok_or_else(|| {
                CliError::ConfigMissing(
                    "control_endpoint is required when it cannot be derived from endpoint; \
                     run `ve-tos-cli config set control_endpoint <value>` or pass --control-endpoint"
                        .to_string(),
                )
            })?
        };

        // Prepend account_id as subdomain if provided
        if let Some(ref account_id) = self.account_id {
            if !account_id.is_empty() {
                // Parse the endpoint to insert account_id as subdomain
                // e.g., https://tos-control-cn-beijing.volces.com -> https://200001234.tos-control-cn-beijing.volces.com
                if let Ok(mut url) = url::Url::parse(&base_endpoint) {
                    if let Some(host) = url.host_str() {
                        let new_host = format!("{}.{}", account_id, host);
                        let _ = url.set_host(Some(&new_host));
                        return Ok(url.to_string().trim_end_matches('/').to_string());
                    }
                }
                // Fallback for non-URL format (just hostname)
                return Ok(format!("{}.{}", account_id, base_endpoint));
            }
        }
        Ok(base_endpoint)
    }

    /// 获取桶级别 endpoint。
    ///
    /// Returns a URL suitable for bucket-level requests. Returns a
    /// `ValidationError` when the bucket name is invalid.
    pub fn bucket_endpoint(&self, bucket: &str) -> Result<String, CliError> {
        validate_bucket_name(bucket)?;
        if let Some(ref ep) = self.endpoint {
            if endpoint_uses_virtual_hosted_style(ep) {
                Ok(insert_bucket_into_endpoint(ep, bucket))
            } else {
                Ok(format!("{}/{}", ep.trim_end_matches('/'), bucket))
            }
        } else {
            build_bucket_endpoint(bucket, &self.region)
        }
    }

    /// Prepare form signature fields (credential, date, algorithm) for PostObject policy.
    pub fn form_prepare(&self) -> FormPrepare {
        self.signer.form_prepare()
    }

    /// Compute the HMAC-SHA256 signature over a base64-encoded policy string for PostObject.
    pub fn form_sign(&self, date_short: &str, policy_base64: &str) -> String {
        self.signer.form_sign(date_short, policy_base64)
    }

    /// Send a raw POST request without V4 Authorization header (for PostObject form-based auth).
    pub async fn send_form_post(
        &self,
        url: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<Response, CliError> {
        let resp = self
            .http
            .post(url)
            .header("content-type", content_type)
            .body(body)
            .send()
            .await
            .map_err(CliError::Http)?;
        Ok(resp)
    }

    /// 获取桶级别签名路径。
    ///
    /// Returns the canonical request path for bucket-level signing. Returns a
    /// `ValidationError` when the bucket name is invalid.
    pub fn bucket_request_path(&self, bucket: &str) -> Result<String, CliError> {
        validate_bucket_name(bucket)?;
        if self.sign_algorithm == TosSignAlgorithm::ByteTosV1 {
            return Ok(format!("/{}", bucket));
        }
        let is_path_style = self
            .endpoint
            .as_deref()
            .map(|ep| !endpoint_uses_virtual_hosted_style(ep))
            .unwrap_or(false);
        if is_path_style {
            Ok(format!("/{}", bucket))
        } else {
            Ok("/".to_string())
        }
    }

    /// 获取对象级别 endpoint。
    ///
    /// Returns a URL suitable for object-level requests. Returns a
    /// `ValidationError` when the bucket name is invalid.
    pub fn object_endpoint(&self, bucket: &str, key: &str) -> Result<String, CliError> {
        Ok(format!("{}/{}", self.bucket_endpoint(bucket)?, key))
    }

    /// 获取对象级别签名路径。
    ///
    /// Returns the canonical request path for object-level signing. Returns a
    /// `ValidationError` when the bucket name is invalid.
    pub fn object_request_path(&self, bucket: &str, key: &str) -> Result<String, CliError> {
        validate_bucket_name(bucket)?;
        if self.sign_algorithm == TosSignAlgorithm::ByteTosV1 {
            return Ok(format!("/{}/{}", bucket, key));
        }
        let is_path_style = self
            .endpoint
            .as_deref()
            .map(|ep| !endpoint_uses_virtual_hosted_style(ep))
            .unwrap_or(false);
        if is_path_style {
            Ok(format!("/{}/{}", bucket, key))
        } else {
            Ok(format!("/{}", key))
        }
    }

    /// Build a presigned object URL using the active TOS signing algorithm.
    pub fn presign_object_url(
        &self,
        method: &str,
        bucket: &str,
        key: &str,
        expires: u64,
    ) -> Result<String, CliError> {
        if self.psm_resolver.is_some() {
            return Err(CliError::ValidationError(
                "presign requires endpoint when using PSM; pass --endpoint or remove --psm"
                    .to_string(),
            ));
        }
        if expires == 0 {
            return Err(CliError::ValidationError(
                "presign expires must be greater than 0".to_string(),
            ));
        }
        let endpoint = self.object_endpoint(bucket, key)?;
        let path = self.object_request_path(bucket, key)?;
        let host = url::Url::parse(&endpoint)
            .map_err(|err| CliError::ValidationError(format!("Invalid URL: {}", err)))?
            .host_str()
            .unwrap_or("")
            .to_string();
        let headers = BTreeMap::from([("host".to_string(), host)]);
        let query = self
            .signer
            .presign_query(method, &path, &BTreeMap::new(), &headers, expires);
        let mut url = url::Url::parse(&endpoint)
            .map_err(|err| CliError::ValidationError(format!("Invalid URL: {}", err)))?;
        {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(&key, &value);
            }
        }
        Ok(url.to_string())
    }

    /// 发送签名请求
    pub async fn send_request(
        &self,
        method: Method,
        url: &str,
        path: &str,
        query_params: BTreeMap<String, String>,
        extra_headers: BTreeMap<String, String>,
        body: Option<Vec<u8>>,
    ) -> Result<Response, CliError> {
        for attempt in 0..=self.max_retry_count {
            let result = self
                .send_request_once(
                    method.clone(),
                    url,
                    path,
                    query_params.clone(),
                    extra_headers.clone(),
                    body.clone(),
                )
                .await;
            match result {
                Ok(resp)
                    if should_retry_response(resp.status()) && attempt < self.max_retry_count =>
                {
                    sleep_before_retry(attempt).await;
                }
                Ok(resp) => return Ok(resp),
                Err(CliError::Http(err))
                    if should_retry_reqwest_error(&err) && attempt < self.max_retry_count =>
                {
                    sleep_before_retry(attempt).await;
                }
                Err(err) => return Err(err),
            }
        }
        Err(CliError::TransferFailed(
            "HTTP retry loop exhausted".to_string(),
        ))
    }

    async fn send_request_once(
        &self,
        method: Method,
        url: &str,
        path: &str,
        query_params: BTreeMap<String, String>,
        extra_headers: BTreeMap<String, String>,
        body: Option<Vec<u8>>,
    ) -> Result<Response, CliError> {
        let payload_hash = match &body {
            Some(b) => hash_payload(b),
            None => EMPTY_PAYLOAD_HASH.to_string(),
        };
        let target = self.resolve_request_target(url, path).await?;

        // 从 URL 提取 host
        let host = url::Url::parse(&target.url)
            .map_err(|e| CliError::ValidationError(format!("Invalid URL: {}", e)))?
            .host_str()
            .unwrap_or("")
            .to_string();

        let mut headers = extra_headers.clone();
        headers.insert("host".to_string(), host);

        add_copy_source_signature(&self.signer, method.as_str(), &mut headers);

        let signed = self.signer.sign_request(
            method.as_str(),
            path,
            &query_params,
            &headers,
            &payload_hash,
        );

        // 构建实际请求
        let mut full_url = target.url.clone();
        if !query_params.is_empty() {
            // [Review Fix #1] The signed canonical query uses RFC3986
            // percent-encoding; the actual request URL must use the same
            // encoding so opaque continuation tokens do not invalidate auth.
            full_url = format!(
                "{}?{}",
                target.url,
                self.request_query_string(&query_params)
            );
        }

        let mut req = self.http.request(method, &full_url);
        for (key, value) in &signed.headers {
            req = req.header(key.as_str(), value.as_str());
        }
        // 附加额外 headers
        for (key, value) in &headers {
            if !key.eq_ignore_ascii_case("host")
                && !has_header_case_insensitive(&signed.headers, key.as_str())
            {
                req = req.header(key.as_str(), value.as_str());
            }
        }

        if let Some(body_bytes) = body {
            req = req.body(body_bytes);
        }

        let result = req.send().await;
        self.record_psm_result(&target, result.as_ref().map(|_| ()))
            .await;
        result.map_err(CliError::Http)
    }

    /// Send a signed request with a streaming body.
    ///
    /// `payload_hash` must be computed by the caller before creating the body stream,
    /// so large uploads do not need to be buffered in memory for signing.
    pub async fn send_streaming_request(
        &self,
        method: Method,
        url: &str,
        path: &str,
        query_params: BTreeMap<String, String>,
        extra_headers: BTreeMap<String, String>,
        payload_hash: String,
        body: Body,
    ) -> Result<Response, CliError> {
        self.send_signed_request(
            method,
            url,
            path,
            query_params,
            extra_headers,
            payload_hash,
            Some(body),
        )
        .await
    }

    async fn send_signed_request(
        &self,
        method: Method,
        url: &str,
        path: &str,
        query_params: BTreeMap<String, String>,
        extra_headers: BTreeMap<String, String>,
        payload_hash: String,
        body: Option<Body>,
    ) -> Result<Response, CliError> {
        if body.is_some() {
            return self
                .send_signed_request_once(
                    method,
                    url,
                    path,
                    query_params,
                    extra_headers,
                    payload_hash,
                    body,
                )
                .await;
        }

        for attempt in 0..=self.max_retry_count {
            let result = self
                .send_signed_request_once(
                    method.clone(),
                    url,
                    path,
                    query_params.clone(),
                    extra_headers.clone(),
                    payload_hash.clone(),
                    None,
                )
                .await;
            match result {
                Ok(resp)
                    if should_retry_response(resp.status()) && attempt < self.max_retry_count =>
                {
                    sleep_before_retry(attempt).await;
                }
                Ok(resp) => return Ok(resp),
                Err(CliError::Http(err))
                    if should_retry_reqwest_error(&err) && attempt < self.max_retry_count =>
                {
                    sleep_before_retry(attempt).await;
                }
                Err(err) => return Err(err),
            }
        }
        Err(CliError::TransferFailed(
            "HTTP retry loop exhausted".to_string(),
        ))
    }

    async fn send_signed_request_once(
        &self,
        method: Method,
        url: &str,
        path: &str,
        query_params: BTreeMap<String, String>,
        extra_headers: BTreeMap<String, String>,
        payload_hash: String,
        body: Option<Body>,
    ) -> Result<Response, CliError> {
        let target = self.resolve_request_target(url, path).await?;
        let host = url::Url::parse(&target.url)
            .map_err(|e| CliError::ValidationError(format!("Invalid URL: {}", e)))?
            .host_str()
            .unwrap_or("")
            .to_string();

        let mut headers = extra_headers.clone();
        headers.insert("host".to_string(), host);

        add_copy_source_signature(&self.signer, method.as_str(), &mut headers);

        let signed = self.signer.sign_request(
            method.as_str(),
            path,
            &query_params,
            &headers,
            &payload_hash,
        );

        let mut full_url = target.url.clone();
        if !query_params.is_empty() {
            // [Review Fix #1] Keep streaming requests' URL query rendering
            // consistent with signing for special characters in token values.
            full_url = format!(
                "{}?{}",
                target.url,
                self.request_query_string(&query_params)
            );
        }

        let mut req = self.http.request(method, &full_url);
        for (key, value) in &signed.headers {
            req = req.header(key.as_str(), value.as_str());
        }
        for (key, value) in &headers {
            if !key.eq_ignore_ascii_case("host")
                && !has_header_case_insensitive(&signed.headers, key.as_str())
            {
                req = req.header(key.as_str(), value.as_str());
            }
        }
        if let Some(body) = body {
            req = req.body(body);
        }

        let result = req.send().await;
        self.record_psm_result(&target, result.as_ref().map(|_| ()))
            .await;
        result.map_err(CliError::Http)
    }

    async fn resolve_request_target(
        &self,
        url: &str,
        path: &str,
    ) -> Result<ResolvedRequestTarget, CliError> {
        let Some(resolver) = &self.psm_resolver else {
            return Ok(ResolvedRequestTarget {
                url: url.to_string(),
                psm_selection: None,
            });
        };
        let bucket = bucket_from_bytetos_request_path(path)?;
        let addr = resolver.resolve_addr(&bucket).await?;
        Ok(ResolvedRequestTarget {
            url: bytetos_psm_url(addr, path),
            psm_selection: Some(PsmSelection { bucket, addr }),
        })
    }

    async fn record_psm_result(
        &self,
        target: &ResolvedRequestTarget,
        result: Result<(), &reqwest::Error>,
    ) {
        let (Some(resolver), Some(selection)) = (&self.psm_resolver, &target.psm_selection) else {
            return;
        };
        if result.is_ok() {
            resolver
                .mark_success(&selection.bucket, selection.addr)
                .await;
        } else {
            resolver
                .mark_failure(&selection.bucket, selection.addr)
                .await;
        }
    }

    fn request_query_string(&self, query_params: &BTreeMap<String, String>) -> String {
        match self.sign_algorithm {
            TosSignAlgorithm::Tos4 => encoded_query_string(query_params),
            // [Review Fix #4] ByteTOS V1 signs raw query values, but the HTTP
            // URL still has to escape literal "+" so the server does not parse
            // opaque continuation tokens as spaces during auth recomputation.
            TosSignAlgorithm::ByteTosV1 => bytetos_v1_request_query_string(query_params),
        }
    }

    /// 检查响应状态，提取 TOS 错误
    pub async fn check_response(&self, resp: Response) -> Result<Response, CliError> {
        let status = resp.status();
        // [G8] Capture x-tos-request-id on every response (success or failure)
        // and stash it in TOS_LAST_REQUEST_ID so the handler layer's Envelope
        // wrapper can inject it deterministically without rewiring every call site.
        if let Some(id) = resp
            .headers()
            .get("x-tos-request-id")
            .and_then(|v| v.to_str().ok())
        {
            if !id.is_empty() {
                std::env::set_var("TOS_LAST_REQUEST_ID", id);
            }
        }
        if status.is_success() {
            return Ok(resp);
        }

        let request_id = resp
            .headers()
            .get("x-tos-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let body_text = resp.text().await.unwrap_or_default();

        // TOS error responses are JSON; fall back to the raw body only when the
        // service returns a malformed error payload.
        let (code, message) =
            parse_tos_error(&body_text).unwrap_or_else(|| (status.to_string(), body_text.clone()));

        // [Review Fix #4] Keep legacy exit_code/error.kind aligned with the
        // new Agent categories instead of collapsing common TOS failures to
        // Unknown after the raw service code has been parsed.
        let formatted_error = format_tos_error(status, &code, &message, &request_id);
        match status {
            StatusCode::BAD_REQUEST => Err(CliError::ValidationError(formatted_error)),
            StatusCode::FORBIDDEN => Err(CliError::PermissionDenied(formatted_error)),
            StatusCode::NOT_FOUND => Err(CliError::ResourceNotFound(formatted_error)),
            StatusCode::UNAUTHORIZED => Err(CliError::AuthFailed(formatted_error)),
            StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED => {
                Err(CliError::Conflict(formatted_error))
            }
            StatusCode::REQUEST_TIMEOUT => Err(CliError::TransferFailed(formatted_error)),
            StatusCode::TOO_MANY_REQUESTS => Err(CliError::RateLimited(formatted_error)),
            _ if status.is_server_error() => Err(CliError::TransferFailed(formatted_error)),
            _ => Err(CliError::Unknown(formatted_error)),
        }
    }
}

struct ResolvedRequestTarget {
    url: String,
    psm_selection: Option<PsmSelection>,
}

struct PsmSelection {
    bucket: String,
    addr: SocketAddr,
}

fn build_psm_resolver(
    profile: &Profile,
    service: &str,
    sign_algorithm: TosSignAlgorithm,
    has_no_endpoint: bool,
) -> Result<Option<Arc<PsmResolver>>, CliError> {
    if !has_no_endpoint || service != "tos" || sign_algorithm != TosSignAlgorithm::ByteTosV1 {
        return Ok(None);
    }
    PsmDiscoveryConfig::from_profile(profile)?
        .map(PsmResolver::new)
        .transpose()
        .map(|resolver| resolver.map(Arc::new))
}

fn bucket_from_bytetos_request_path(path: &str) -> Result<String, CliError> {
    let bucket = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("")
        .trim();
    if bucket.is_empty() {
        return Err(CliError::ValidationError(
            "PSM requests require a bucket in the ByteTOS V1 request path".to_string(),
        ));
    }
    Ok(bucket.to_string())
}

fn bytetos_psm_url(addr: SocketAddr, path: &str) -> String {
    if path.starts_with('/') {
        format!("http://{}{}", addr, path)
    } else {
        format!("http://{}/{}", addr, path)
    }
}

fn format_tos_error(status: StatusCode, code: &str, message: &str, request_id: &str) -> String {
    format!(
        "HTTP {} [{}] {} (RequestId: {})",
        status.as_u16(),
        code,
        message,
        request_id
    )
}

fn derive_region_from_endpoint(endpoint: &str) -> Option<String> {
    let raw = endpoint.trim();
    if raw.is_empty() {
        return None;
    }

    let host = if let Ok(url) = url::Url::parse(raw) {
        url.host_str()?.to_string()
    } else {
        raw.trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()?
            .to_string()
    };

    if let Some(rest) = host.strip_prefix("tos-") {
        return rest.split('.').next().map(|s| s.to_string());
    }

    if let Some((_, rest)) = host.split_once(".tos-") {
        return rest.split('.').next().map(|s| s.to_string());
    }

    None
}

/// Normalize a user-supplied endpoint so it always carries an explicit scheme.
///
/// `config init` and hand-written config files frequently store bare hosts like
/// `tos-cn-beijing.volces.com`. reqwest/url require an absolute URL with a
/// scheme, so we default to `https://` when none is present. Inputs that already
/// start with `http://` or `https://` are returned unchanged.
fn normalize_endpoint_scheme(endpoint: &str) -> String {
    let trimmed = endpoint.trim();
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{}", trimmed)
    }
}

fn encoded_query_string(query_params: &BTreeMap<String, String>) -> String {
    query_params
        .iter()
        .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn bytetos_v1_request_query_string(query_params: &BTreeMap<String, String>) -> String {
    query_params
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                url_encode_with_safe(key, ""),
                url_encode_with_safe(value, "/")
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn endpoint_uses_virtual_hosted_style(endpoint: &str) -> bool {
    let raw = endpoint.trim();
    if raw.is_empty() {
        return false;
    }

    let parsed = url::Url::parse(raw).ok();
    let host = parsed
        .as_ref()
        .and_then(|url| url.host_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            raw.trim_start_matches("https://")
                .trim_start_matches("http://")
                .split('/')
                .next()
                .unwrap_or("")
                .to_string()
        });
    let path = parsed.as_ref().map(|url| url.path()).unwrap_or("");
    (path.is_empty() || path == "/") && host.starts_with("tos-")
}

fn insert_bucket_into_endpoint(endpoint: &str, bucket: &str) -> String {
    if let Ok(mut url) = url::Url::parse(endpoint) {
        if let Some(host) = url.host_str() {
            let new_host = format!("{}.{}", bucket, host);
            let _ = url.set_host(Some(&new_host));
            return url.to_string().trim_end_matches('/').to_string();
        }
    }

    let raw = endpoint.trim_end_matches('/');
    format!("https://{}.{}", bucket, raw.trim_start_matches("https://"))
}

/// Parse a TOS JSON error response.
fn parse_tos_error(body: &str) -> Option<(String, String)> {
    let value = serde_json::from_str::<Value>(body).ok()?;
    let code = json_error_string(&value, &["code", "Code", "error_code", "ErrorCode"])?;
    let message = json_error_string(
        &value,
        &["message", "Message", "error_message", "ErrorMessage"],
    )
    .unwrap_or_default();
    Some((code, message))
}

fn json_error_string(value: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        value.get(*name).and_then(|field| {
            field
                .as_str()
                .map(ToString::to_string)
                .or_else(|| field.as_i64().map(|number| number.to_string()))
        })
    })
}

fn should_retry_response(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

fn should_retry_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect()
}

async fn sleep_before_retry(attempt: u32) {
    let shift = attempt.min(5);
    let delay_ms = 200_u64.saturating_mul(1_u64 << shift);
    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
}

fn header_value_case_insensitive<'a>(
    headers: &'a BTreeMap<String, String>,
    name: &str,
) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn has_header_case_insensitive(headers: &BTreeMap<String, String>, name: &str) -> bool {
    headers.keys().any(|key| key.eq_ignore_ascii_case(name))
}

fn add_copy_source_signature(
    signer: &TosSigner,
    method: &str,
    headers: &mut BTreeMap<String, String>,
) {
    // [Review Fix #TOS-CopySourceSignature] ByteTOS V1 CopyObject signs the
    // copy source path separately, matching tos-rust-sdk's
    // X-Tos-Copy-Signature behavior. TOS4/ve-tos does not use this header.
    let copy_source =
        header_value_case_insensitive(headers, "x-tos-copy-source").map(ToString::to_string);
    if let Some(copy_source) = copy_source {
        if let Some(signed_copy_source) = signer.sign_copy_source(method, &copy_source) {
            headers.extend(signed_copy_source.headers);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        add_copy_source_signature, derive_region_from_endpoint, storage_user_agent_for_name,
        TosClient, TosSignAlgorithm, TosSigner,
    };
    use crate::infra::config::Profile;
    use reqwest::Method;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{mpsc, Mutex};
    use std::thread;
    use std::time::Duration;

    static TEST_ADDR_ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_test_tosapi_addr<T>(value: Option<String>, run: impl FnOnce() -> T) -> T {
        let _guard = TEST_ADDR_ENV_LOCK.lock().expect("test addr env lock");
        let old_value = std::env::var("TEST_TOSAPI_ADDR").ok();
        match value {
            Some(value) => std::env::set_var("TEST_TOSAPI_ADDR", value),
            None => std::env::remove_var("TEST_TOSAPI_ADDR"),
        }
        let result = run();
        if let Some(old_value) = old_value {
            std::env::set_var("TEST_TOSAPI_ADDR", old_value);
        } else {
            std::env::remove_var("TEST_TOSAPI_ADDR");
        }
        result
    }

    #[test]
    fn derive_region_from_service_endpoint() {
        assert_eq!(
            derive_region_from_endpoint("https://tos-cn-beijing.volces.com"),
            Some("cn-beijing".to_string())
        );
        assert_eq!(
            derive_region_from_endpoint("tos-cn-shanghai.volces.com"),
            Some("cn-shanghai".to_string())
        );
    }

    #[test]
    fn derive_region_from_bucket_endpoint() {
        assert_eq!(
            derive_region_from_endpoint("https://demo.tos-cn-guangzhou.volces.com"),
            Some("cn-guangzhou".to_string())
        );
    }

    #[test]
    fn user_agent_for_name_uses_top_level_binary_name_only() {
        assert_eq!(
            storage_user_agent_for_name("tos-cli"),
            format!("tos-cli/v{}", env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(
            storage_user_agent_for_name("ve-storage-uni-cli"),
            format!("ve-storage-uni-cli/v{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn byted_tos_client_uses_v1_signing_paths() {
        let client = TosClient::new_with_sign_algorithm(
            &Profile {
                region: Some("cn-boe".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some("tos-cn-boe.volces.com".to_string()),
                ..Default::default()
            },
            "tos",
            TosSignAlgorithm::ByteTosV1,
        )
        .expect("client");

        assert_eq!(client.sign_algorithm, TosSignAlgorithm::ByteTosV1);
        assert_eq!(
            client.bucket_request_path("bucket").expect("bucket path"),
            "/bucket"
        );
        assert_eq!(
            client
                .object_request_path("bucket", "key")
                .expect("object path"),
            "/bucket/key"
        );
    }

    #[test]
    fn bytetos_v1_enables_psm_resolver_only_without_endpoint() {
        let profile = Profile {
            region: Some("cn-boe".to_string()),
            access_key_id: Some("ak".to_string()),
            secret_access_key: Some("sk".to_string()),
            psm: Some("tos.example.service".to_string()),
            idc: Some("boe".to_string()),
            cluster: Some("default".to_string()),
            addr_family: Some("v4".to_string()),
            ..Default::default()
        };

        let client = with_test_tosapi_addr(Some("127.0.0.1:1".to_string()), || {
            TosClient::new_with_sign_algorithm(&profile, "tos", TosSignAlgorithm::ByteTosV1)
                .expect("client")
        });

        assert!(client.psm_resolver.is_some());

        let endpoint_profile = Profile {
            endpoint: Some("tos-cn-boe.volces.com".to_string()),
            ..profile
        };
        let endpoint_client = TosClient::new_with_sign_algorithm(
            &endpoint_profile,
            "tos",
            TosSignAlgorithm::ByteTosV1,
        )
        .expect("endpoint client");

        assert!(endpoint_client.psm_resolver.is_none());
    }

    #[test]
    fn tos4_ignores_psm_resolver() {
        let client = with_test_tosapi_addr(Some("127.0.0.1:1".to_string()), || {
            TosClient::new_with_sign_algorithm(
                &Profile {
                    region: Some("cn-beijing".to_string()),
                    access_key_id: Some("ak".to_string()),
                    secret_access_key: Some("sk".to_string()),
                    psm: Some("tos.example.service".to_string()),
                    ..Default::default()
                },
                "tos",
                TosSignAlgorithm::Tos4,
            )
            .expect("client")
        });

        assert!(client.psm_resolver.is_none());
    }

    #[test]
    fn bytetos_v1_presign_requires_endpoint_when_psm_is_active() {
        let client = with_test_tosapi_addr(Some("127.0.0.1:1".to_string()), || {
            TosClient::new_with_sign_algorithm(
                &Profile {
                    region: Some("cn-boe".to_string()),
                    access_key_id: Some("ak".to_string()),
                    secret_access_key: Some("sk".to_string()),
                    psm: Some("tos.example.service".to_string()),
                    ..Default::default()
                },
                "tos",
                TosSignAlgorithm::ByteTosV1,
            )
            .expect("client")
        });

        let err = client
            .presign_object_url("GET", "bucket", "key", 60)
            .expect_err("PSM presign should be rejected");
        assert!(
            err.to_string().contains("presign requires endpoint"),
            "err={err}"
        );
    }

    #[test]
    fn ve_tos_client_keeps_v4_virtual_hosted_signing_paths() {
        let client = TosClient::new_with_sign_algorithm(
            &Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some("tos-cn-beijing.volces.com".to_string()),
                ..Default::default()
            },
            "tos",
            TosSignAlgorithm::Tos4,
        )
        .expect("client");

        assert_eq!(client.sign_algorithm, TosSignAlgorithm::Tos4);
        assert_eq!(
            client.bucket_request_path("bucket").expect("bucket path"),
            "/"
        );
        assert_eq!(
            client
                .object_request_path("bucket", "key")
                .expect("object path"),
            "/key"
        );
    }

    #[test]
    fn bucket_endpoint_rejects_invalid_virtual_hosted_bucket_names() {
        let client = TosClient::new_with_sign_algorithm(
            &Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some("tos-cn-beijing.volces.com".to_string()),
                ..Default::default()
            },
            "tos",
            TosSignAlgorithm::Tos4,
        )
        .expect("client");

        let err = client
            .bucket_endpoint("Bad_Bucket")
            .expect_err("invalid bucket should be rejected before host insertion");
        assert!(err.to_string().contains("invalid bucket name"), "err={err}");
        assert_eq!(
            client
                .bucket_endpoint("demo-bucket")
                .expect("valid bucket endpoint"),
            "https://demo-bucket.tos-cn-beijing.volces.com"
        );
    }

    #[tokio::test]
    async fn bytetos_v1_psm_sends_request_to_resolved_static_address() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let selected_addr = listener.local_addr().expect("local addr");
        let (line_tx, line_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            let request_line = request.lines().next().unwrap_or_default().to_string();
            line_tx.send(request_line).expect("send request line");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .expect("write response");
        });

        let profile = Profile {
            region: Some("cn-boe".to_string()),
            access_key_id: Some("ak".to_string()),
            secret_access_key: Some("sk".to_string()),
            psm: Some("tos.example.service".to_string()),
            max_retry_count: Some(0),
            requesttimeout: Some(5),
            connecttimeout: Some(5),
            ..Default::default()
        };
        let client = with_test_tosapi_addr(Some(selected_addr.to_string()), || {
            TosClient::new_with_sign_algorithm(&profile, "tos", TosSignAlgorithm::ByteTosV1)
                .expect("client")
        });
        assert!(client.psm_resolver.is_some());

        let bucket = "bucket";
        client
            .send_request(
                Method::GET,
                &client.bucket_endpoint(bucket).expect("bucket endpoint"),
                &client.bucket_request_path(bucket).expect("bucket path"),
                BTreeMap::new(),
                BTreeMap::new(),
                None,
            )
            .await
            .expect("send request");

        let request_line = line_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("captured request line");
        server.join().expect("server thread");
        assert_eq!(request_line, "GET /bucket HTTP/1.1");
    }

    #[tokio::test]
    async fn send_request_percent_encodes_signed_query_values() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let (line_tx, line_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            let request_line = request.lines().next().unwrap_or_default().to_string();
            line_tx.send(request_line).expect("send request line");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .expect("write response");
        });

        let profile = Profile {
            region: Some("cn-beijing".to_string()),
            access_key_id: Some("ak".to_string()),
            secret_access_key: Some("sk".to_string()),
            endpoint: Some(endpoint),
            max_retry_count: Some(0),
            requesttimeout: Some(5),
            connecttimeout: Some(5),
            ..Default::default()
        };
        let client = TosClient::new_with_sign_algorithm(&profile, "tos", TosSignAlgorithm::Tos4)
            .expect("client");
        let bucket = "bucket";
        let mut query = BTreeMap::new();
        query.insert("continuation-token".to_string(), "abc+/==".to_string());
        query.insert("list-type".to_string(), "2".to_string());

        client
            .send_request(
                Method::GET,
                &client.bucket_endpoint(bucket).expect("bucket endpoint"),
                &client.bucket_request_path(bucket).expect("bucket path"),
                query,
                BTreeMap::new(),
                None,
            )
            .await
            .expect("send request");

        let request_line = line_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("captured request line");
        server.join().expect("server thread");
        assert!(
            request_line.contains("continuation-token=abc%2B%2F%3D%3D"),
            "request_line={request_line}"
        );
    }

    #[tokio::test]
    async fn bytetos_v1_send_request_preserves_slash_query_values() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let (line_tx, line_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            let request_line = request.lines().next().unwrap_or_default().to_string();
            line_tx.send(request_line).expect("send request line");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .expect("write response");
        });

        let profile = Profile {
            region: Some("cn-boe".to_string()),
            access_key_id: Some("ak".to_string()),
            secret_access_key: Some("sk".to_string()),
            endpoint: Some(endpoint),
            max_retry_count: Some(0),
            requesttimeout: Some(5),
            connecttimeout: Some(5),
            ..Default::default()
        };
        let client =
            TosClient::new_with_sign_algorithm(&profile, "tos", TosSignAlgorithm::ByteTosV1)
                .expect("client");
        let bucket = "bucket";
        let mut query = BTreeMap::new();
        query.insert("delimiter".to_string(), "/".to_string());
        query.insert("list-type".to_string(), "2".to_string());

        client
            .send_request(
                Method::GET,
                &client.bucket_endpoint(bucket).expect("bucket endpoint"),
                &client.bucket_request_path(bucket).expect("bucket path"),
                query,
                BTreeMap::new(),
                None,
            )
            .await
            .expect("send request");

        let request_line = line_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("captured request line");
        server.join().expect("server thread");
        assert!(
            request_line.contains("delimiter=/"),
            "request_line={request_line}"
        );
        assert!(
            !request_line.contains("delimiter=%2F"),
            "request_line={request_line}"
        );
    }

    #[tokio::test]
    async fn bytetos_v1_send_request_escapes_literal_plus_in_query_values() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let (line_tx, line_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            let request_line = request.lines().next().unwrap_or_default().to_string();
            line_tx.send(request_line).expect("send request line");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .expect("write response");
        });

        let profile = Profile {
            region: Some("cn-boe".to_string()),
            access_key_id: Some("ak".to_string()),
            secret_access_key: Some("sk".to_string()),
            endpoint: Some(endpoint),
            max_retry_count: Some(0),
            requesttimeout: Some(5),
            connecttimeout: Some(5),
            ..Default::default()
        };
        let client =
            TosClient::new_with_sign_algorithm(&profile, "tos", TosSignAlgorithm::ByteTosV1)
                .expect("client");
        let bucket = "bucket";
        let mut query = BTreeMap::new();
        query.insert("continuation-token".to_string(), "abc+def/ghi=".to_string());
        query.insert("delimiter".to_string(), "/".to_string());
        query.insert("list-type".to_string(), "2".to_string());

        client
            .send_request(
                Method::GET,
                &client.bucket_endpoint(bucket).expect("bucket endpoint"),
                &client.bucket_request_path(bucket).expect("bucket path"),
                query,
                BTreeMap::new(),
                None,
            )
            .await
            .expect("send request");

        let request_line = line_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("captured request line");
        server.join().expect("server thread");
        assert!(
            request_line.contains("continuation-token=abc%2Bdef/ghi%3D"),
            "request_line={request_line}"
        );
        assert!(
            request_line.contains("delimiter=/"),
            "request_line={request_line}"
        );
    }

    #[test]
    fn bytetos_v1_copy_object_adds_copy_source_signature_header() {
        let signer = TosSigner::new(
            TosSignAlgorithm::ByteTosV1,
            "ak".to_string(),
            "sk".to_string(),
            "cn-boe".to_string(),
            "tos".to_string(),
        );
        let mut headers = BTreeMap::new();
        headers.insert(
            "x-tos-copy-source".to_string(),
            "%2Fbucket%2Fdir%2Fa.txt".to_string(),
        );

        add_copy_source_signature(&signer, "POST", &mut headers);

        let copy_signature = headers
            .get("X-Tos-Copy-Signature")
            .expect("copy signature header");
        assert!(copy_signature.starts_with("TOS-HMAC-SHA256 expiration="));
        assert!(copy_signature.contains("credentials=ak/"));
    }

    #[test]
    fn tos4_copy_object_does_not_add_copy_source_signature_header() {
        let signer = TosSigner::new(
            TosSignAlgorithm::Tos4,
            "ak".to_string(),
            "sk".to_string(),
            "cn-beijing".to_string(),
            "tos".to_string(),
        );
        let mut headers = BTreeMap::new();
        headers.insert(
            "x-tos-copy-source".to_string(),
            "/bucket/dir/a.txt".to_string(),
        );

        add_copy_source_signature(&signer, "PUT", &mut headers);

        assert!(!headers.contains_key("X-Tos-Copy-Signature"));
    }
}
