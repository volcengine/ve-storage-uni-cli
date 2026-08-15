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
use crate::agent::request_id::{sanitize_request_id, ServiceRequestTrace};
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
use crate::infra::retry::{
    should_retry_storage_status, storage_backoff_delay, storage_retry_after_delay,
};
use crate::infra::unified_credentials::{UnifiedCredentialProvider, UnifiedCredentialValue};
use reqwest::{Body, Client, Method, Response, StatusCode};
use serde_json::Value;
use std::collections::BTreeMap;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

pub const USER_AGENT_NAME_ENV: &str = "VE_STORAGE_UNI_USER_AGENT_NAME";
const TOS_CONFIG_BINARY_ENV: &str = "VE_STORAGE_UNI_TOS_CONFIG_BINARY";
const PSM_PLACEHOLDER_ENDPOINT: &str = "http://psm.invalid";
/// Maximum bytes to drain from a retryable (408/429/5xx) response body before
/// giving up and letting the connection close. Error bodies are normally a few
/// hundred bytes; 10 MiB is generous while preventing a misbehaving server from
/// forcing us to buffer an unlimited payload just to reuse the socket.
const MAX_DRAIN_BODY_BYTES: usize = 10 * 1024 * 1024;

fn encoded_object_key_path(key: &str) -> String {
    url_encode_with_safe(key, "/")
}

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

enum RequestAuth {
    Static(TosSigner),
    Unified {
        provider: UnifiedCredentialProvider,
        algorithm: TosSignAlgorithm,
        region: String,
        service: String,
    },
}

enum AttemptSigner<'a> {
    Borrowed(&'a TosSigner),
    Owned(TosSigner),
}

impl AttemptSigner<'_> {
    fn signer(&self) -> &TosSigner {
        match self {
            Self::Borrowed(signer) => signer,
            Self::Owned(signer) => signer,
        }
    }
}

impl RequestAuth {
    async fn resolve_attempt_signer(&self) -> Result<AttemptSigner<'_>, CliError> {
        match self {
            Self::Static(signer) => Ok(AttemptSigner::Borrowed(signer)),
            Self::Unified {
                provider,
                algorithm,
                region,
                service,
            } => {
                let credentials = provider.get().await?;
                Ok(AttemptSigner::Owned(signer_from_unified_credentials(
                    *algorithm,
                    region,
                    service,
                    credentials,
                )))
            }
        }
    }
}

fn signer_from_unified_credentials(
    algorithm: TosSignAlgorithm,
    region: &str,
    service: &str,
    credentials: UnifiedCredentialValue,
) -> TosSigner {
    let mut signer = TosSigner::new(
        algorithm,
        credentials.access_key_id,
        credentials.secret_access_key,
        region.to_string(),
        service.to_string(),
    );
    if !credentials.session_token.is_empty() {
        signer = signer.with_security_token(credentials.session_token);
    }
    signer
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
    request_auth: RequestAuth,
    sign_algorithm: TosSignAlgorithm,
    region: String,
    endpoint: Option<String>,
    psm_resolver: Option<Arc<PsmResolver>>,
    control_endpoint: Option<String>,
    account_id: Option<String>,
    max_retry_count: u32,
    request_trace: Arc<ServiceRequestTrace>,
}

/// One resolved signer used for both PostObject policy preparation and signing.
///
/// The context borrows a static signer or owns one signer built from a single
/// unified credential result. It never exposes credential secrets through
/// `Debug` output.
pub struct FormAuthContext<'a> {
    signer: AttemptSigner<'a>,
}

impl FormAuthContext<'_> {
    /// Build the credential, date, algorithm, and optional token form fields.
    pub fn prepare(&self) -> FormPrepare {
        self.signer.signer().form_prepare()
    }

    /// Sign a base64-encoded PostObject policy with this context's signer.
    pub fn sign(&self, date_short: &str, policy_base64: &str) -> String {
        self.signer.signer().form_sign(date_short, policy_base64)
    }
}

/// Owned metadata for an HTTP request whose byte body can be replayed.
#[derive(Debug, Clone)]
pub struct ReplayableRequest {
    /// HTTP method used for every attempt.
    pub method: Method,
    /// Logical request URL before optional PSM resolution.
    pub url: String,
    /// Canonical signing path.
    pub path: String,
    /// Signed query parameters.
    pub query_params: BTreeMap<String, String>,
    /// Additional signed request headers.
    pub extra_headers: BTreeMap<String, String>,
    /// Optional in-memory request body cloned for every attempt.
    pub body: Option<Vec<u8>>,
}

/// Owned metadata for an HTTP request whose streaming body can be rebuilt.
#[derive(Debug, Clone)]
pub struct ReplayableStreamingRequest {
    /// HTTP method used for every attempt.
    pub method: Method,
    /// Logical object URL before optional PSM resolution.
    pub url: String,
    /// Canonical signing path.
    pub path: String,
    /// Signed query parameters.
    pub query_params: BTreeMap<String, String>,
    /// Additional signed request headers.
    pub extra_headers: BTreeMap<String, String>,
    /// SHA256 payload hash used by the request signer.
    pub payload_hash: String,
}

impl TosClient {
    /// Build a TOS client with an isolated request-ID trace.
    pub fn new(profile: &Profile, service: &str) -> Result<Self, CliError> {
        Self::new_with_sign_algorithm(profile, service, active_tos_sign_algorithm())
    }

    /// Build a TOS client that records response request IDs into an invocation trace.
    pub fn new_with_request_trace(
        profile: &Profile,
        service: &str,
        request_trace: Arc<ServiceRequestTrace>,
    ) -> Result<Self, CliError> {
        Self::new_with_sign_algorithm_and_trace(
            profile,
            service,
            active_tos_sign_algorithm(),
            request_trace,
        )
    }

    /// Build a TOS4 client that resolves unified credentials for each request attempt.
    ///
    /// Network, endpoint, region, and retry options are read from `profile`, but
    /// its static credential fields are deliberately ignored.
    pub fn new_with_unified_credentials(
        profile: &Profile,
        service: &str,
        provider: UnifiedCredentialProvider,
    ) -> Result<Self, CliError> {
        Self::new_with_unified_credentials_and_request_trace(
            profile,
            service,
            provider,
            Arc::new(ServiceRequestTrace::default()),
        )
    }

    /// Build a TOS4 unified-credential client with an invocation request trace.
    ///
    /// Returns configuration or HTTP client construction errors before any
    /// credentials are requested. Each later signing attempt resolves exactly
    /// one complete unified credential value.
    pub fn new_with_unified_credentials_and_request_trace(
        profile: &Profile,
        service: &str,
        provider: UnifiedCredentialProvider,
        request_trace: Arc<ServiceRequestTrace>,
    ) -> Result<Self, CliError> {
        Self::new_with_auth(
            profile,
            service,
            TosSignAlgorithm::Tos4,
            request_trace,
            Some(provider),
        )
    }

    fn new_with_sign_algorithm(
        profile: &Profile,
        service: &str,
        sign_algorithm: TosSignAlgorithm,
    ) -> Result<Self, CliError> {
        Self::new_with_sign_algorithm_and_trace(
            profile,
            service,
            sign_algorithm,
            Arc::new(ServiceRequestTrace::default()),
        )
    }

    fn new_with_sign_algorithm_and_trace(
        profile: &Profile,
        service: &str,
        sign_algorithm: TosSignAlgorithm,
        request_trace: Arc<ServiceRequestTrace>,
    ) -> Result<Self, CliError> {
        Self::new_with_auth(profile, service, sign_algorithm, request_trace, None)
    }

    fn new_with_auth(
        profile: &Profile,
        service: &str,
        sign_algorithm: TosSignAlgorithm,
        request_trace: Arc<ServiceRequestTrace>,
        unified_provider: Option<UnifiedCredentialProvider>,
    ) -> Result<Self, CliError> {
        let (endpoint, psm_resolver, region) =
            resolve_client_network(profile, service, sign_algorithm)?;
        let request_auth = match unified_provider {
            Some(provider) => RequestAuth::Unified {
                provider,
                algorithm: sign_algorithm,
                region: region.clone(),
                service: service.to_string(),
            },
            None => RequestAuth::Static(static_signer_from_profile(
                profile,
                service,
                sign_algorithm,
                &region,
            )?),
        };
        Ok(Self {
            http: build_http_client(profile)?,
            request_auth,
            sign_algorithm,
            region,
            endpoint,
            psm_resolver,
            control_endpoint: profile
                .control_endpoint
                .as_deref()
                .map(normalize_endpoint_scheme),
            account_id: profile.account_id.clone(),
            max_retry_count: profile
                .max_retry_count
                .unwrap_or(DEFAULT_HTTP_MAX_RETRY_COUNT),
            request_trace,
        })
    }

    /// Resolve one signer for PostObject preparation and policy signing.
    pub async fn prepare_form_auth(&self) -> Result<FormAuthContext<'_>, CliError> {
        Ok(FormAuthContext {
            signer: self.request_auth.resolve_attempt_signer().await?,
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
            .unwrap_or_else(|| PSM_PLACEHOLDER_ENDPOINT.to_string())
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
        } else if self.psm_resolver.is_some() {
            Ok(format!("{PSM_PLACEHOLDER_ENDPOINT}/{bucket}"))
        } else {
            Err(CliError::ConfigMissing("endpoint is required".to_string()))
        }
    }

    /// Send a raw POST request without V4 Authorization header (for PostObject form-based auth).
    ///
    /// ByteTOS PSM profiles resolve the bucket target before the request is
    /// sent, just like signed requests.
    pub async fn send_form_post(
        &self,
        url: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> Result<Response, CliError> {
        let path = url::Url::parse(url)
            .map_err(|error| CliError::ValidationError(format!("Invalid URL: {error}")))?
            .path()
            .to_string();
        // [Review Fix #26] `service_endpoint` uses an internal placeholder in
        // PSM mode; resolve it here so form uploads never contact that host.
        let target = self.resolve_request_target(url, &path).await?;
        let result = self
            .http
            .post(&target.url)
            .header("content-type", content_type)
            .body(body)
            .send()
            .await;
        self.record_psm_result(&target, result.as_ref().map(|_| ()))
            .await;
        match &result {
            Ok(response) => self.record_request_id(response),
            // [Review Fix #9] A terminal transport failure must clear any
            // earlier response as the source of the public error request ID.
            Err(_) => self.request_trace.record_no_response(),
        }
        result.map_err(CliError::Http)
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
        Ok(format!(
            "{}/{}",
            self.bucket_endpoint(bucket)?,
            encoded_object_key_path(key)
        ))
    }

    /// 获取对象级别签名路径。
    ///
    /// Returns the canonical request path for object-level signing. Returns a
    /// `ValidationError` when the bucket name is invalid.
    pub fn object_request_path(&self, bucket: &str, key: &str) -> Result<String, CliError> {
        validate_bucket_name(bucket)?;
        if self.sign_algorithm == TosSignAlgorithm::ByteTosV1 {
            return Ok(format!("/{}/{}", bucket, encoded_object_key_path(key)));
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
    pub async fn presign_object_url(
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
        let signer = self.request_auth.resolve_attempt_signer().await?;
        let query =
            signer
                .signer()
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
        let is_idempotent = is_request_retry_safe(&method);
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
                    if should_retry_storage_status(resp.status(), is_idempotent)
                        && attempt < self.max_retry_count =>
                {
                    sleep_before_response_retry(attempt, resp).await;
                }
                Ok(resp) => return Ok(resp),
                Err(CliError::Http(err))
                    if should_retry_send_error(&err, is_idempotent)
                        && attempt < self.max_retry_count =>
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

    /// Send a replayable request and consume its complete response inside the
    /// retry boundary.
    ///
    /// The consumer owns the response and must return only after every required
    /// response-body byte has been read and validated. Transient transport or
    /// response-body failures rebuild the signed request and invoke the consumer
    /// again, up to the configured retry limit.
    pub async fn send_request_with_consumer<T, C, CFut>(
        &self,
        request: ReplayableRequest,
        mut consume: C,
    ) -> Result<T, CliError>
    where
        C: FnMut(Response) -> CFut,
        CFut: Future<Output = Result<T, CliError>>,
    {
        // [Review Fix #1] Rebuildable bytes are not enough to make POST-like
        // operations idempotent, so response-consumption failures are replayed
        // only for methods whose operation semantics are safe to repeat.
        let is_idempotent = is_request_retry_safe(&request.method);
        for attempt in 0..=self.max_retry_count {
            let result = self
                .send_request_once(
                    request.method.clone(),
                    &request.url,
                    &request.path,
                    request.query_params.clone(),
                    request.extra_headers.clone(),
                    request.body.clone(),
                )
                .await;
            match result {
                Ok(response)
                    if should_retry_storage_status(response.status(), is_idempotent)
                        && attempt < self.max_retry_count =>
                {
                    sleep_before_response_retry(attempt, response).await;
                }
                Ok(response) => match consume(response).await {
                    Ok(value) => return Ok(value),
                    Err(error)
                        if is_idempotent
                            && should_retry_cli_error(&error)
                            && attempt < self.max_retry_count =>
                    {
                        sleep_before_retry(attempt).await;
                    }
                    Err(error) => return Err(error),
                },
                Err(error)
                    if should_retry_request_error(&error, is_idempotent)
                        && attempt < self.max_retry_count =>
                {
                    sleep_before_retry(attempt).await;
                }
                Err(error) => return Err(error),
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

        let signer = self.request_auth.resolve_attempt_signer().await?;
        add_copy_source_signature(signer.signer(), method.as_str(), &mut headers);

        let signed = signer.signer().sign_request(
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
        match &result {
            Ok(response) => self.record_request_id(response),
            // [Review Fix #9] Keep terminal-attempt metadata aligned across
            // buffered and streaming TOS request paths.
            Err(_) => self.request_trace.record_no_response(),
        }
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

    /// Send a signed streaming request using the normal HTTP retry policy.
    ///
    /// The body factory is invoked once per attempt, so an already-consumed
    /// file stream is never reused. Factory errors and non-retryable responses
    /// are returned immediately.
    pub async fn send_replayable_streaming_request<F, Fut>(
        &self,
        request: ReplayableStreamingRequest,
        mut body_factory: F,
    ) -> Result<Response, CliError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Body, CliError>>,
    {
        let is_idempotent = is_request_retry_safe(&request.method);
        for attempt in 0..=self.max_retry_count {
            let body = body_factory().await?;
            let result = self
                .send_signed_request_once(
                    request.method.clone(),
                    &request.url,
                    &request.path,
                    request.query_params.clone(),
                    request.extra_headers.clone(),
                    request.payload_hash.clone(),
                    Some(body),
                )
                .await;
            match result {
                Ok(response)
                    if should_retry_storage_status(response.status(), is_idempotent)
                        && attempt < self.max_retry_count =>
                {
                    sleep_before_response_retry(attempt, response).await;
                }
                Ok(response) => return Ok(response),
                Err(CliError::Http(error))
                    if should_retry_send_error(&error, is_idempotent)
                        && attempt < self.max_retry_count =>
                {
                    sleep_before_retry(attempt).await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(CliError::TransferFailed(
            "HTTP retry loop exhausted".to_string(),
        ))
    }

    /// Send a replayable streaming request and consume the complete response
    /// inside the same retry boundary.
    ///
    /// Both factories are invoked once per attempt. The body factory must
    /// reopen or reconstruct the request body, while the consumer must finish
    /// reading and validating the response before returning.
    pub async fn send_replayable_streaming_request_with_consumer<T, F, Fut, C, CFut>(
        &self,
        request: ReplayableStreamingRequest,
        mut body_factory: F,
        mut consume: C,
    ) -> Result<T, CliError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<Body, CliError>>,
        C: FnMut(Response) -> CFut,
        CFut: Future<Output = Result<T, CliError>>,
    {
        let is_idempotent = is_request_retry_safe(&request.method);
        for attempt in 0..=self.max_retry_count {
            let body = body_factory().await?;
            let result = self
                .send_signed_request_once(
                    request.method.clone(),
                    &request.url,
                    &request.path,
                    request.query_params.clone(),
                    request.extra_headers.clone(),
                    request.payload_hash.clone(),
                    Some(body),
                )
                .await;
            match result {
                Ok(response)
                    if should_retry_storage_status(response.status(), is_idempotent)
                        && attempt < self.max_retry_count =>
                {
                    sleep_before_response_retry(attempt, response).await;
                }
                Ok(response) => match consume(response).await {
                    Ok(value) => return Ok(value),
                    Err(error)
                        if is_idempotent
                            && should_retry_cli_error(&error)
                            && attempt < self.max_retry_count =>
                    {
                        sleep_before_retry(attempt).await;
                    }
                    Err(error) => return Err(error),
                },
                Err(error)
                    if should_retry_request_error(&error, is_idempotent)
                        && attempt < self.max_retry_count =>
                {
                    sleep_before_retry(attempt).await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(CliError::TransferFailed(
            "HTTP retry loop exhausted".to_string(),
        ))
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

        let is_idempotent = is_request_retry_safe(&method);
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
                    if should_retry_storage_status(resp.status(), is_idempotent)
                        && attempt < self.max_retry_count =>
                {
                    sleep_before_response_retry(attempt, resp).await;
                }
                Ok(resp) => return Ok(resp),
                Err(CliError::Http(err))
                    if should_retry_send_error(&err, is_idempotent)
                        && attempt < self.max_retry_count =>
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

        let signer = self.request_auth.resolve_attempt_signer().await?;
        add_copy_source_signature(signer.signer(), method.as_str(), &mut headers);

        let signed = signer.signer().sign_request(
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
        match &result {
            Ok(response) => self.record_request_id(response),
            // [Review Fix #9] Signed requests must also clear any earlier
            // response when their terminal attempt receives no response.
            Err(_) => self.request_trace.record_no_response(),
        }
        result.map_err(CliError::Http)
    }

    fn record_request_id(&self, response: &Response) {
        let request_id = response
            .headers()
            .get("x-tos-request-id")
            .and_then(|value| value.to_str().ok());
        self.request_trace
            .record_response(request_id, response.status().is_success());
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
            .and_then(sanitize_request_id)
        {
            std::env::set_var("TOS_LAST_REQUEST_ID", id);
        }
        if status.is_success() {
            return Ok(resp);
        }

        let request_id = resp
            .headers()
            .get("x-tos-request-id")
            .and_then(|v| v.to_str().ok())
            .and_then(sanitize_request_id)
            .unwrap_or_default();

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

type ClientNetwork = (Option<String>, Option<Arc<PsmResolver>>, String);

fn resolve_client_network(
    profile: &Profile,
    service: &str,
    sign_algorithm: TosSignAlgorithm,
) -> Result<ClientNetwork, CliError> {
    // Whitespace-only values are not explicit network configuration and must
    // not turn into the invalid URL `https://`.
    let endpoint = profile
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_endpoint_scheme);
    let psm_resolver = build_psm_resolver(profile, service, sign_algorithm, endpoint.is_none())?;
    if endpoint.is_none() && psm_resolver.is_none() {
        let message = if service == "tos" && sign_algorithm == TosSignAlgorithm::ByteTosV1 {
            "endpoint or psm is required; configure the active profile or a BYTE_TOS_* environment variable"
        } else {
            "endpoint is required; configure --endpoint, the active profile endpoint, or TOS_ENDPOINT"
        };
        return Err(CliError::ConfigMissing(message.to_string()));
    }
    let region = profile
        .region
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| endpoint.as_deref().and_then(derive_region_from_endpoint))
        .ok_or_else(|| CliError::ConfigMissing("region is required".to_string()))?;
    Ok((endpoint, psm_resolver, region))
}

fn static_signer_from_profile(
    profile: &Profile,
    service: &str,
    sign_algorithm: TosSignAlgorithm,
    region: &str,
) -> Result<TosSigner, CliError> {
    let access_key = profile
        .access_key_id
        .as_deref()
        .ok_or_else(|| CliError::ConfigMissing("access_key_id is required".to_string()))?;
    let secret_key = profile
        .secret_access_key
        .as_deref()
        .ok_or_else(|| CliError::ConfigMissing("secret_access_key is required".to_string()))?;
    let signer = TosSigner::new(
        sign_algorithm,
        access_key.to_string(),
        secret_key.to_string(),
        region.to_string(),
        service.to_string(),
    );
    Ok(match &profile.security_token {
        Some(token) => signer.with_security_token(token.clone()),
        None => signer,
    })
}

fn build_http_client(profile: &Profile) -> Result<Client, CliError> {
    Client::builder()
        .user_agent(storage_user_agent())
        .tcp_nodelay(true)
        .connect_timeout(Duration::from_secs(
            profile
                .connecttimeout
                .unwrap_or(DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS),
        ))
        .read_timeout(Duration::from_secs(
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
        .map_err(CliError::Http)
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

/// Parse a TOS signing region from a recognizable service or bucket endpoint.
///
/// Custom endpoints that do not contain the `tos-<region>` host component
/// return `None` and require an explicit region.
pub fn derive_region_from_endpoint(endpoint: &str) -> Option<String> {
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

fn is_transient_reqwest_error(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_body() || err.is_decode()
}

fn should_retry_send_error(err: &reqwest::Error, is_idempotent: bool) -> bool {
    err.is_connect() || (is_idempotent && is_transient_reqwest_error(err))
}

fn should_retry_request_error(error: &CliError, is_idempotent: bool) -> bool {
    matches!(error, CliError::Http(http_error) if should_retry_send_error(http_error, is_idempotent))
}

fn should_retry_cli_error(error: &CliError) -> bool {
    matches!(error, CliError::Http(http_error) if is_transient_reqwest_error(http_error))
        || matches!(error, CliError::TransferFailed(_))
}

fn is_request_retry_safe(method: &Method) -> bool {
    // [Review Fix #1] Apply one operation-level eligibility decision to send,
    // status, and response-consumption failures. Replayable bytes alone do not
    // make POST/PATCH side effects safe to repeat.
    method == Method::GET
        || method == Method::HEAD
        || method == Method::PUT
        || method == Method::DELETE
        || method == Method::OPTIONS
}

async fn sleep_before_retry(attempt: u32) {
    tokio::time::sleep(storage_backoff_delay(attempt)).await;
}

async fn sleep_before_response_retry(attempt: u32, mut response: Response) {
    // Drain the body so the underlying connection can be reused for the retry.
    // Cap the drain at MAX_DRAIN_BODY_BYTES; if the body is larger we stop
    // reading and let the connection close rather than buffer it all.
    drain_response_body_bounded(&mut response, MAX_DRAIN_BODY_BYTES).await;
    // [Review Fix #1] Resolve HTTP-date relative to the time sleeping begins,
    // so response draining does not make the client wait past the server date.
    let delay = storage_retry_after_delay(response.status(), response.headers(), SystemTime::now());
    match delay {
        Some(duration) => tokio::time::sleep(duration).await,
        None => sleep_before_retry(attempt).await,
    }
}

/// Read a response body up to `max_bytes` so the connection can be returned to
/// the keep-alive pool. Returns `true` if the body was fully consumed (the
/// connection is reusable), `false` if the limit was exceeded or a read error
/// occurred (the connection will be closed on drop).
async fn drain_response_body_bounded(response: &mut Response, max_bytes: usize) -> bool {
    if let Some(len) = response.content_length() {
        if len as usize > max_bytes {
            return false;
        }
    }
    let mut total = 0usize;
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                total += chunk.len();
                if total > max_bytes {
                    return false;
                }
            }
            Ok(None) => return true,
            Err(_) => return false,
        }
    }
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
        ReplayableRequest, ReplayableStreamingRequest, TosClient, TosSignAlgorithm, TosSigner,
    };
    use crate::agent::error::CliError;
    use crate::agent::request_id::ServiceRequestTrace;
    use crate::infra::config::Profile;
    use crate::infra::unified_credentials::{UnifiedCredentialProvider, UnifiedCredentialValue};
    use reqwest::{Method, StatusCode};
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    };
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

    fn endpoint_test_client(sign_algorithm: TosSignAlgorithm) -> TosClient {
        TosClient::new_with_sign_algorithm(
            &Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some("tos-cn-beijing.volces.com".to_string()),
                ..Default::default()
            },
            "tos",
            sign_algorithm,
        )
        .expect("client")
    }

    fn read_request_body(stream: &mut TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        let header_end = loop {
            let bytes_read = stream.read(&mut buffer).expect("read request");
            assert!(bytes_read > 0, "connection closed before request headers");
            request.extend_from_slice(&buffer[..bytes_read]);
            if let Some(offset) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break offset + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("content length"))
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let bytes_read = stream.read(&mut buffer).expect("read request body");
            assert!(bytes_read > 0, "connection closed before request body");
            request.extend_from_slice(&buffer[..bytes_read]);
        }
        request[header_end..header_end + content_length].to_vec()
    }

    fn read_request_headers(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set read timeout");
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let bytes_read = stream.read(&mut buffer).expect("read request");
            assert!(bytes_read > 0, "connection closed before request headers");
            request.extend_from_slice(&buffer[..bytes_read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                return String::from_utf8(request).expect("HTTP request is UTF-8");
            }
        }
    }

    fn request_header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
        request.lines().find_map(|line| {
            let (header_name, value) = line.split_once(':')?;
            header_name
                .eq_ignore_ascii_case(name)
                .then_some(value.trim())
        })
    }

    fn counting_unified_provider(calls: Arc<AtomicUsize>) -> UnifiedCredentialProvider {
        UnifiedCredentialProvider::from_resolver(move || {
            let call = calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(UnifiedCredentialValue::new(
                format!("unified-ak-{call}"),
                format!("unified-sk-{call}"),
                format!("unified-token-{call}"),
                "test-provider",
            ))
        })
    }

    fn unified_test_profile(endpoint: String, max_retry_count: u32) -> Profile {
        Profile {
            region: Some("cn-beijing".to_string()),
            endpoint: Some(endpoint),
            max_retry_count: Some(max_retry_count),
            requesttimeout: Some(5),
            connecttimeout: Some(5),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn unified_signing_resolves_credentials_once_per_retry_attempt() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            ["500 Internal Server Error", "200 OK"]
                .into_iter()
                .map(|status| {
                    let (mut stream, _) = listener.accept().expect("accept request");
                    let request = read_request_headers(&mut stream);
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    )
                    .expect("write response");
                    request
                })
                .collect::<Vec<_>>()
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let client = TosClient::new_with_unified_credentials(
            &unified_test_profile(endpoint, 1),
            "tos",
            counting_unified_provider(Arc::clone(&calls)),
        )
        .expect("unified client");

        let response = client
            .send_request(
                Method::GET,
                &client.bucket_endpoint("bucket").expect("bucket endpoint"),
                &client.bucket_request_path("bucket").expect("bucket path"),
                BTreeMap::new(),
                BTreeMap::new(),
                None,
            )
            .await
            .expect("retry succeeds");

        assert_eq!(response.status(), StatusCode::OK);
        let requests = server.join().expect("server thread");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_unified_request_credential(&requests[0], 1);
        assert_unified_request_credential(&requests[1], 2);
    }

    fn assert_unified_request_credential(request: &str, call: usize) {
        let authorization = request_header(request, "authorization").expect("authorization");
        assert!(authorization.contains(&format!("Credential=unified-ak-{call}/")));
        let expected_token = format!("unified-token-{call}");
        assert_eq!(
            request_header(request, "x-tos-security-token"),
            Some(expected_token.as_str())
        );
    }

    #[tokio::test]
    async fn unified_signing_copy_object_uses_one_atomic_attempt_credential() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let request = read_request_headers(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .expect("write response");
            request
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let client = TosClient::new_with_unified_credentials(
            &unified_test_profile(endpoint, 0),
            "tos",
            counting_unified_provider(Arc::clone(&calls)),
        )
        .expect("unified client");
        let headers = BTreeMap::from([(
            "x-tos-copy-source".to_string(),
            "/source-bucket/source-key".to_string(),
        )]);

        client
            .send_request(
                Method::PUT,
                &client
                    .object_endpoint("bucket", "destination")
                    .expect("object endpoint"),
                &client
                    .object_request_path("bucket", "destination")
                    .expect("object path"),
                BTreeMap::new(),
                headers,
                None,
            )
            .await
            .expect("copy response");

        let request = server.join().expect("server thread");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_unified_request_credential(&request, 1);
        assert!(request_header(&request, "x-tos-copy-signature").is_none());
    }

    #[tokio::test]
    async fn unified_signing_presign_resolves_once_with_atomic_query_credentials() {
        let calls = Arc::new(AtomicUsize::new(0));
        let client = TosClient::new_with_unified_credentials(
            &unified_test_profile("https://tos-cn-beijing.volces.com".to_string(), 0),
            "tos",
            counting_unified_provider(Arc::clone(&calls)),
        )
        .expect("unified client");

        let presigned = client
            .presign_object_url("GET", "bucket", "key", 60)
            .await
            .expect("presigned URL");

        let url = url::Url::parse(&presigned).expect("valid URL");
        let query = url.query_pairs().collect::<BTreeMap<_, _>>();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(query
            .get("X-Tos-Credential")
            .is_some_and(|value| value.starts_with("unified-ak-1/")));
        assert_eq!(
            query
                .get("X-Tos-Security-Token")
                .map(|value| value.as_ref()),
            Some("unified-token-1")
        );
    }

    #[tokio::test]
    async fn unified_signing_form_context_resolves_once_for_prepare_and_sign() {
        let calls = Arc::new(AtomicUsize::new(0));
        let client = TosClient::new_with_unified_credentials(
            &unified_test_profile("https://tos-cn-beijing.volces.com".to_string(), 0),
            "tos",
            counting_unified_provider(Arc::clone(&calls)),
        )
        .expect("unified client");

        let context = client.prepare_form_auth().await.expect("form auth context");
        let prepare = context.prepare();
        let signature = context.sign(&prepare.date_short, "cG9saWN5");
        let expected = TosSigner::new(
            TosSignAlgorithm::Tos4,
            "unified-ak-1".to_string(),
            "unified-sk-1".to_string(),
            "cn-beijing".to_string(),
            "tos".to_string(),
        )
        .with_security_token("unified-token-1".to_string())
        .form_sign(&prepare.date_short, "cG9saWN5");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(prepare.credential.starts_with("unified-ak-1/"));
        assert_eq!(prepare.security_token.as_deref(), Some("unified-token-1"));
        assert_eq!(signature, expected);
    }

    #[tokio::test]
    async fn unified_signing_static_client_keeps_static_credentials_without_provider_calls() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let request = read_request_headers(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .expect("write response");
            request
        });
        let calls = Arc::new(AtomicUsize::new(0));
        let _unused_provider = counting_unified_provider(Arc::clone(&calls));
        let client = TosClient::new_with_sign_algorithm(
            &Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("static-ak".to_string()),
                secret_access_key: Some("static-sk".to_string()),
                security_token: Some("static-token".to_string()),
                endpoint: Some(endpoint),
                max_retry_count: Some(0),
                ..Default::default()
            },
            "tos",
            TosSignAlgorithm::Tos4,
        )
        .expect("static client");

        client
            .send_request(
                Method::GET,
                &client.bucket_endpoint("bucket").expect("bucket endpoint"),
                &client.bucket_request_path("bucket").expect("bucket path"),
                BTreeMap::new(),
                BTreeMap::new(),
                None,
            )
            .await
            .expect("static response");

        let request = server.join().expect("server thread");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(request_header(&request, "authorization")
            .is_some_and(|value| value.contains("Credential=static-ak/")));
        assert_eq!(
            request_header(&request, "x-tos-security-token"),
            Some("static-token")
        );
    }

    fn spawn_streaming_response_server(
        statuses: Vec<&'static str>,
    ) -> (String, thread::JoinHandle<Vec<Vec<u8>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            let mut bodies = Vec::new();
            for status in statuses {
                let (mut stream, _) = listener.accept().expect("accept request");
                bodies.push(read_request_body(&mut stream));
                let response =
                    format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
            bodies
        });
        (endpoint, server)
    }

    fn streaming_test_client(endpoint: String, max_retry_count: u32) -> TosClient {
        TosClient::new_with_sign_algorithm(
            &Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some(endpoint),
                max_retry_count: Some(max_retry_count),
                requesttimeout: Some(5),
                connecttimeout: Some(5),
                ..Default::default()
            },
            "tos",
            TosSignAlgorithm::Tos4,
        )
        .expect("client")
    }

    fn replayable_test_request(client: &TosClient) -> ReplayableStreamingRequest {
        ReplayableStreamingRequest {
            method: Method::PUT,
            url: client
                .object_endpoint("bucket", "retry.bin")
                .expect("object endpoint"),
            path: client
                .object_request_path("bucket", "retry.bin")
                .expect("object path"),
            query_params: BTreeMap::new(),
            extra_headers: BTreeMap::from([("content-length".to_string(), "6".to_string())]),
            payload_hash: "payload-hash".to_string(),
        }
    }

    #[tokio::test]
    async fn tos_client_records_retry_and_success_request_ids() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            for response in [
                "HTTP/1.1 500 Internal Server Error\r\nx-tos-request-id: retry-1\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                "HTTP/1.1 200 OK\r\nx-tos-request-id: success-2\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            ] {
                let (mut stream, _) = listener.accept().expect("accept request");
                let _ = read_request_body(&mut stream);
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });
        let trace = Arc::new(ServiceRequestTrace::default());
        let client = TosClient::new_with_request_trace(
            &Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some(endpoint),
                max_retry_count: Some(1),
                requesttimeout: Some(5),
                connecttimeout: Some(5),
                ..Default::default()
            },
            "tos",
            Arc::clone(&trace),
        )
        .expect("client");

        let response = client
            .send_request(
                Method::GET,
                &client.bucket_endpoint("bucket").expect("bucket endpoint"),
                &client.bucket_request_path("bucket").expect("bucket path"),
                BTreeMap::new(),
                BTreeMap::new(),
                None,
            )
            .await
            .expect("retry succeeds");

        assert_eq!(response.status(), StatusCode::OK);
        server.join().expect("server thread");
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.request_ids, vec!["retry-1", "success-2"]);
        assert_eq!(
            snapshot.last_successful_request_id.as_deref(),
            Some("success-2")
        );
    }

    #[tokio::test]
    async fn tos_client_rejects_unsafe_error_request_id() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let _ = read_request_body(&mut stream);
            let request_id = "x".repeat(257);
            let body = r#"{"Code":"InvalidArgument","Message":"bad request"}"#;
            write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\nx-tos-request-id: {request_id}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("write response");
        });
        let trace = Arc::new(ServiceRequestTrace::default());
        let client = TosClient::new_with_request_trace(
            &Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some(endpoint),
                max_retry_count: Some(0),
                ..Default::default()
            },
            "tos",
            Arc::clone(&trace),
        )
        .expect("client");
        let response = client
            .send_request(
                Method::GET,
                &client.bucket_endpoint("bucket").expect("bucket endpoint"),
                &client.bucket_request_path("bucket").expect("bucket path"),
                BTreeMap::new(),
                BTreeMap::new(),
                None,
            )
            .await
            .expect("HTTP response");

        let error = client
            .check_response(response)
            .await
            .expect_err("400 error");

        server.join().expect("server thread");
        assert_eq!(error.agent_semantics().request_id, None);
        assert!(trace.snapshot().request_ids.is_empty());
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
    fn tos4_requires_explicit_endpoint_when_only_region_is_configured() {
        let result = TosClient::new_with_sign_algorithm(
            &Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                ..Default::default()
            },
            "tos",
            TosSignAlgorithm::Tos4,
        );
        let Err(error) = result else {
            panic!("region must not be converted into an endpoint");
        };

        assert!(error.to_string().contains("endpoint is required"));
    }

    #[test]
    fn tos4_rejects_blank_endpoint_as_missing() {
        let result = TosClient::new_with_sign_algorithm(
            &Profile {
                region: Some("cn-beijing".to_string()),
                endpoint: Some("   ".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                ..Default::default()
            },
            "tos",
            TosSignAlgorithm::Tos4,
        );
        let Err(error) = result else {
            panic!("blank endpoint must not count as explicit configuration");
        };

        assert!(error.to_string().contains("endpoint is required"));
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
    fn tos4_rejects_psm_without_explicit_endpoint() {
        let result = with_test_tosapi_addr(Some("127.0.0.1:1".to_string()), || {
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
        });
        let Err(error) = result else {
            panic!("TOS4 must not treat ByteTOS PSM as a resource endpoint");
        };

        assert!(error.to_string().contains("endpoint is required"));
    }

    #[tokio::test]
    async fn bytetos_v1_presign_requires_endpoint_when_psm_is_active() {
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
            .await
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
    fn object_endpoint_percent_encodes_key_segments_once() {
        let client = endpoint_test_client(TosSignAlgorithm::ByteTosV1);
        assert!(client
            .object_endpoint("bucket", "dir/create_topic_with_%DLQ%_test.py")
            .expect("object endpoint")
            .ends_with("/dir/create_topic_with_%25DLQ%25_test.py"));
        assert!(client
            .object_endpoint("bucket", "literal-%25/a b/plus+sign/中文?#.txt")
            .expect("object endpoint")
            .ends_with("/literal-%2525/a%20b/plus%2Bsign/%E4%B8%AD%E6%96%87%3F%23.txt"));
    }

    #[test]
    fn bytetos_v1_signs_the_encoded_wire_path() {
        let client = endpoint_test_client(TosSignAlgorithm::ByteTosV1);
        assert_eq!(
            client
                .object_request_path("bucket", "dir/%DLQ%")
                .expect("object path"),
            "/bucket/dir/%25DLQ%25"
        );
    }

    #[test]
    fn tos4_keeps_raw_path_for_single_canonical_encoding() {
        let client = endpoint_test_client(TosSignAlgorithm::Tos4);
        assert_eq!(
            client
                .object_request_path("bucket", "dir/%DLQ%")
                .expect("object path"),
            "/dir/%DLQ%"
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
    async fn bytetos_v1_psm_form_post_uses_resolved_static_address() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let selected_addr = listener.local_addr().expect("local addr");
        let (line_tx, line_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            line_tx
                .send(request.lines().next().unwrap_or_default().to_string())
                .expect("send request line");
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
        let url = format!(
            "{}/",
            client.bucket_endpoint("bucket").expect("bucket endpoint")
        );

        client
            .send_form_post(&url, "multipart/form-data; boundary=test", b"body".to_vec())
            .await
            .expect("send form request");

        let request_line = line_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("captured request line");
        server.join().expect("server thread");
        assert_eq!(request_line, "POST /bucket/ HTTP/1.1");
    }

    #[tokio::test]
    async fn bytetos_v1_wire_path_escapes_literal_percent() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let (line_tx, line_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            line_tx
                .send(request.lines().next().unwrap_or_default().to_string())
                .expect("send request line");
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .expect("write response");
        });
        let client = TosClient::new_with_sign_algorithm(
            &Profile {
                region: Some("cn-boe".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some(endpoint),
                max_retry_count: Some(0),
                ..Default::default()
            },
            "tos",
            TosSignAlgorithm::ByteTosV1,
        )
        .expect("client");
        let key = "dir/create_topic_with_%DLQ%_test.py";

        client
            .send_request(
                Method::GET,
                &client.object_endpoint("bucket", key).expect("endpoint"),
                &client.object_request_path("bucket", key).expect("path"),
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
        assert_eq!(
            request_line,
            "GET /bucket/dir/create_topic_with_%25DLQ%25_test.py HTTP/1.1"
        );
    }

    #[tokio::test]
    async fn replayable_streaming_request_rebuilds_body_after_500() {
        let (endpoint, server) =
            spawn_streaming_response_server(vec!["500 Internal Server Error", "200 OK"]);
        let client = streaming_test_client(endpoint, 1);
        let response = client
            .send_replayable_streaming_request(replayable_test_request(&client), || async {
                Ok(reqwest::Body::from("replay"))
            })
            .await
            .expect("replayed request");

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            server.join().expect("server thread"),
            vec![b"replay".to_vec(), b"replay".to_vec()]
        );
    }

    #[tokio::test]
    async fn non_idempotent_post_retries_500() {
        let (endpoint, server) =
            spawn_streaming_response_server(vec!["500 Internal Server Error", "200 OK"]);
        let client = streaming_test_client(endpoint, 1);
        let response = client
            .send_request(
                Method::POST,
                &client
                    .object_endpoint("bucket", "create")
                    .expect("object endpoint"),
                &client
                    .object_request_path("bucket", "create")
                    .expect("object path"),
                BTreeMap::new(),
                BTreeMap::new(),
                Some(b"request".to_vec()),
            )
            .await
            .expect("retry rejected non-idempotent request");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            server.join().expect("server thread"),
            vec![b"request".to_vec(), b"request".to_vec()]
        );
    }

    #[tokio::test]
    async fn non_idempotent_streaming_post_retries_500() {
        let (endpoint, server) =
            spawn_streaming_response_server(vec!["500 Internal Server Error", "200 OK"]);
        let client = streaming_test_client(endpoint, 1);
        let mut request = replayable_test_request(&client);
        request.method = Method::POST;
        let response = client
            .send_replayable_streaming_request(request, || async {
                Ok(reqwest::Body::from("replay"))
            })
            .await
            .expect("retry rejected non-idempotent streaming request");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            server.join().expect("server thread"),
            vec![b"replay".to_vec(), b"replay".to_vec()]
        );
    }

    #[tokio::test]
    async fn non_idempotent_post_consumer_retries_500() {
        let (endpoint, server) =
            spawn_streaming_response_server(vec!["500 Internal Server Error", "200 OK"]);
        let client = streaming_test_client(endpoint, 1);
        let request = ReplayableRequest {
            method: Method::POST,
            url: client
                .object_endpoint("bucket", "create")
                .expect("object endpoint"),
            path: client
                .object_request_path("bucket", "create")
                .expect("object path"),
            query_params: BTreeMap::new(),
            extra_headers: BTreeMap::new(),
            body: Some(b"request".to_vec()),
        };

        let status = client
            .send_request_with_consumer(request, |response| async move { Ok(response.status()) })
            .await
            .expect("retry rejected response before consuming success");

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            server.join().expect("server thread"),
            vec![b"request".to_vec(), b"request".to_vec()]
        );
    }

    #[tokio::test]
    async fn non_idempotent_streaming_post_consumer_retries_500() {
        let (endpoint, server) =
            spawn_streaming_response_server(vec!["500 Internal Server Error", "200 OK"]);
        let client = streaming_test_client(endpoint, 1);
        let mut request = replayable_test_request(&client);
        request.method = Method::POST;

        let status = client
            .send_replayable_streaming_request_with_consumer(
                request,
                || async { Ok(reqwest::Body::from("replay")) },
                |response| async move { Ok(response.status()) },
            )
            .await
            .expect("retry rejected streaming response before consuming success");

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            server.join().expect("server thread"),
            vec![b"replay".to_vec(), b"replay".to_vec()]
        );
    }

    #[tokio::test]
    async fn non_idempotent_post_does_not_retry_400() {
        let (endpoint, server) = spawn_streaming_response_server(vec!["400 Bad Request"]);
        let client = streaming_test_client(endpoint, 2);
        let response = client
            .send_request(
                Method::POST,
                &client
                    .object_endpoint("bucket", "create")
                    .expect("object endpoint"),
                &client
                    .object_request_path("bucket", "create")
                    .expect("object path"),
                BTreeMap::new(),
                BTreeMap::new(),
                Some(b"request".to_vec()),
            )
            .await
            .expect("return non-retryable client error");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(server.join().expect("server thread"), vec![b"request"]);
    }

    #[tokio::test]
    async fn non_idempotent_post_does_not_retry_408() {
        let (endpoint, server) = spawn_streaming_response_server(vec!["408 Request Timeout"]);
        let client = streaming_test_client(endpoint, 1);
        let response = client
            .send_request(
                Method::POST,
                &client
                    .object_endpoint("bucket", "create")
                    .expect("object endpoint"),
                &client
                    .object_request_path("bucket", "create")
                    .expect("object path"),
                BTreeMap::new(),
                BTreeMap::new(),
                Some(b"request".to_vec()),
            )
            .await
            .expect("return ambiguous timeout without replaying POST");

        assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
        assert_eq!(server.join().expect("server thread"), vec![b"request"]);
    }

    #[tokio::test]
    async fn replayable_streaming_request_does_not_retry_400() {
        let (endpoint, server) = spawn_streaming_response_server(vec!["400 Bad Request"]);
        let client = streaming_test_client(endpoint, 2);
        let response = client
            .send_replayable_streaming_request(replayable_test_request(&client), || async {
                Ok(reqwest::Body::from("replay"))
            })
            .await
            .expect("non-retryable response");

        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(server.join().expect("server thread").len(), 1);
    }

    #[tokio::test]
    async fn replayable_streaming_request_stops_after_retry_limit() {
        let (endpoint, server) = spawn_streaming_response_server(vec![
            "500 Internal Server Error",
            "500 Internal Server Error",
            "500 Internal Server Error",
        ]);
        let client = streaming_test_client(endpoint, 2);
        let response = client
            .send_replayable_streaming_request(replayable_test_request(&client), || async {
                Ok(reqwest::Body::from("replay"))
            })
            .await
            .expect("last retry response");

        assert_eq!(
            response.status(),
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(server.join().expect("server thread").len(), 3);
    }

    #[tokio::test]
    async fn replayable_request_retries_when_success_body_is_truncated() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            for body in [b"short".as_slice(), b"complete-response".as_slice()] {
                let (mut stream, _) = listener.accept().expect("accept request");
                let _ = read_request_body(&mut stream);
                let response = "HTTP/1.1 200 OK\r\ncontent-length: 17\r\nconnection: close\r\n\r\n";
                stream
                    .write_all(response.as_bytes())
                    .expect("write response headers");
                stream.write_all(body).expect("write response body");
            }
        });
        let client = streaming_test_client(endpoint, 1);
        let request = ReplayableRequest {
            method: Method::GET,
            url: client
                .object_endpoint("bucket", "retry.bin")
                .expect("object endpoint"),
            path: client
                .object_request_path("bucket", "retry.bin")
                .expect("object path"),
            query_params: BTreeMap::new(),
            extra_headers: BTreeMap::new(),
            body: None,
        };

        let body = client
            .send_request_with_consumer(request, |response| async move {
                response
                    .bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .map_err(CliError::Http)
            })
            .await
            .expect("retry complete response body");

        server.join().expect("server thread");
        assert_eq!(body, b"complete-response");
    }

    #[tokio::test]
    async fn replayable_request_retries_response_validation_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept request");
                let _ = read_request_body(&mut stream);
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                    )
                    .expect("write response");
            }
        });
        let client = streaming_test_client(endpoint, 1);
        let request = ReplayableRequest {
            method: Method::GET,
            url: client
                .object_endpoint("bucket", "validated.json")
                .expect("object endpoint"),
            path: client
                .object_request_path("bucket", "validated.json")
                .expect("object path"),
            query_params: BTreeMap::new(),
            extra_headers: BTreeMap::new(),
            body: None,
        };
        let mut attempts = 0_u32;

        let body = client
            .send_request_with_consumer(request, |response| {
                attempts += 1;
                let current_attempt = attempts;
                async move {
                    let body = response.bytes().await.map_err(CliError::Http)?;
                    if current_attempt == 1 {
                        return Err(CliError::TransferFailed(
                            "simulated decode validation failure".to_string(),
                        ));
                    }
                    Ok(body)
                }
            })
            .await
            .expect("retry response validation failure");

        server.join().expect("server thread");
        assert_eq!(attempts, 2);
        assert_eq!(body, b"{}".as_slice());
    }

    #[tokio::test]
    async fn replayable_request_does_not_retry_non_transient_400() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let _ = read_request_body(&mut stream);
            stream
                .write_all(
                    b"HTTP/1.1 400 Bad Request\r\ncontent-length: 3\r\nconnection: close\r\n\r\nbad",
                )
                .expect("write response");
        });
        let client = streaming_test_client(endpoint, 2);
        let request = ReplayableRequest {
            method: Method::GET,
            url: client
                .object_endpoint("bucket", "bad.bin")
                .expect("object endpoint"),
            path: client
                .object_request_path("bucket", "bad.bin")
                .expect("object path"),
            query_params: BTreeMap::new(),
            extra_headers: BTreeMap::new(),
            body: None,
        };

        let (status, body) = client
            .send_request_with_consumer(request, |response| async move {
                let status = response.status();
                let body = response.bytes().await.map_err(CliError::Http)?;
                Ok((status, body))
            })
            .await
            .expect("return non-retryable response");

        server.join().expect("server thread");
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, b"bad".as_slice());
    }

    #[tokio::test]
    async fn non_idempotent_post_does_not_retry_after_response_body_started() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let _ = read_request_body(&mut stream);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 8\r\nconnection: close\r\n\r\nshort",
                )
                .expect("write truncated response");
        });
        let client = streaming_test_client(endpoint, 1);
        let request = ReplayableRequest {
            method: Method::POST,
            url: client
                .object_endpoint("bucket", "create")
                .expect("object endpoint"),
            path: client
                .object_request_path("bucket", "create")
                .expect("object path"),
            query_params: BTreeMap::new(),
            extra_headers: BTreeMap::new(),
            body: Some(b"request".to_vec()),
        };

        let error = client
            .send_request_with_consumer(request, |response| async move {
                response.bytes().await.map_err(CliError::Http)
            })
            .await
            .expect_err("POST response consumption must not be replayed");

        server.join().expect("server thread");
        assert!(matches!(error, CliError::Http(_)));
    }

    #[tokio::test]
    async fn request_timeout_is_read_idle_timeout_not_total_body_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let _ = read_request_body(&mut stream);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\nconnection: close\r\n\r\na")
                .expect("write response start");
            thread::sleep(Duration::from_millis(700));
            let _ = stream.write_all(b"b");
            thread::sleep(Duration::from_millis(700));
            let _ = stream.write_all(b"c");
        });
        let client = TosClient::new_with_sign_algorithm(
            &Profile {
                region: Some("cn-beijing".to_string()),
                access_key_id: Some("ak".to_string()),
                secret_access_key: Some("sk".to_string()),
                endpoint: Some(endpoint),
                max_retry_count: Some(0),
                requesttimeout: Some(1),
                connecttimeout: Some(1),
                ..Default::default()
            },
            "tos",
            TosSignAlgorithm::Tos4,
        )
        .expect("client");
        let request = ReplayableRequest {
            method: Method::GET,
            url: client
                .object_endpoint("bucket", "slow.bin")
                .expect("object endpoint"),
            path: client
                .object_request_path("bucket", "slow.bin")
                .expect("object path"),
            query_params: BTreeMap::new(),
            extra_headers: BTreeMap::new(),
            body: None,
        };

        let body = client
            .send_request_with_consumer(request, |response| async move {
                response
                    .bytes()
                    .await
                    .map(|bytes| bytes.to_vec())
                    .map_err(CliError::Http)
            })
            .await
            .expect("steady body progress must not hit a total deadline");

        server.join().expect("server thread");
        assert_eq!(body, b"abc");
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
