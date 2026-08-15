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

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use chrono::Utc;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tos_core::agent::error::CliError;
use tos_core::agent::request_id::{sanitize_request_id, ServiceRequestTrace};
use tos_core::infra::client::storage_user_agent;
use tos_core::infra::config::{
    DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS, DEFAULT_HTTP_MAX_CONNECTIONS,
    DEFAULT_HTTP_MAX_RETRY_COUNT, DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS,
};
use tos_core::infra::retry::{
    should_retry_storage_status, storage_backoff_delay, storage_retry_after_delay,
};
use tos_core::infra::unified_credentials::{UnifiedCredentialProvider, UnifiedCredentialValue};

use super::rate_limiter::RateLimiter;
use super::token_manager::OAuthTokenManager;
use super::types::*;

type HmacSha256 = Hmac<Sha256>;
pub type Result<T> = std::result::Result<T, Error>;

const MAX_RESPONSE_BODY_SIZE: usize = 50 * 1024 * 1024;
/// Maximum bytes to drain from a retryable (408/429/5xx) response body before
/// giving up and letting the connection close. Error bodies are normally small.
const MAX_DRAIN_BODY_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug)]
pub enum Error {
    Http(reqwest::Error),
    HttpBody(std::io::Error),
    Json(serde_json::Error),
    Server(IdsError),
    Cli(CliError),
    /// Unified-login SDK credential resolution failed before an HTTP request.
    UnifiedCredential(CliError),
    Client(String),
    InvalidResponse(String),
}

impl Error {
    fn client(message: impl Into<String>) -> Self {
        Self::Client(message.into())
    }
}

fn oauth_user_id_required_error() -> Error {
    // [Review Fix #2] Keep the stable error code while making remediation executable.
    Error::Cli(CliError::ValidationError(
        "[oauth_user_id_required] OAuth Space creation requires --owner-id or ve-adrive auth login --instance <instance_id> to load the selected user's ID"
            .to_string(),
    ))
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(err) => write!(formatter, "http error: {err}"),
            Self::HttpBody(err) => write!(formatter, "body io error: {err}"),
            Self::Json(err) => write!(formatter, "json error: {err}"),
            Self::Server(err) => write!(formatter, "ids server error: {err}"),
            Self::Cli(err) => err.fmt(formatter),
            Self::UnifiedCredential(err) => err.fmt(formatter),
            Self::Client(message) => write!(formatter, "client error: {message}"),
            Self::InvalidResponse(message) => write!(formatter, "invalid response: {message}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Self::Http(err)
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

impl From<CliError> for Error {
    fn from(error: CliError) -> Self {
        Self::Cli(error)
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct IdsError {
    #[serde(rename = "Code", alias = "code", default)]
    pub code: String,
    #[serde(rename = "Message", alias = "message", default)]
    pub message: String,
    #[serde(
        rename = "RequestId",
        alias = "RequestID",
        alias = "request_id",
        default
    )]
    pub request_id: Option<String>,
    #[serde(skip)]
    pub status_code: Option<u16>,
    #[serde(skip)]
    pub response_headers: Option<HashMap<String, String>>,
}

impl fmt::Display for IdsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[{}] {}", self.code, self.message)?;
        if let Some(request_id) = &self.request_id {
            // [Review Fix #2] Match the public Agent parser's RequestId syntax
            // so the envelope receives a structured request_id field.
            write!(formatter, " (RequestId: {request_id})")?;
        }
        if let Some(status_code) = self.status_code {
            write!(formatter, " (status={status_code})")?;
        }
        Ok(())
    }
}

#[derive(serde::Deserialize)]
struct ErrorEnvelope {
    #[serde(rename = "Error", alias = "error", default)]
    error: Option<IdsError>,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<ClientInner>,
}

#[derive(Clone, Debug, Default)]
pub struct ClientOptions {
    pub max_retry_count: Option<u32>,
    pub requesttimeout: Option<u64>,
    pub connecttimeout: Option<u64>,
    pub maxconnections: Option<usize>,
}

#[derive(Clone)]
struct ClientInner {
    auth: RequestAuth,
    endpoint: String,
    region: String,
    http: reqwest::Client,
    max_retry_count: u32,
    request_trace: Arc<ServiceRequestTrace>,
}

#[derive(Clone)]
enum RequestAuth {
    Aksk(AkskRequestAuth),
    OAuth(OAuthTokenManager),
    Unified(UnifiedRequestAuth),
}

#[cfg(test)]
type TestUnifiedResolver =
    dyn Fn() -> std::result::Result<UnifiedCredentialValue, CliError> + Send + Sync + 'static;

#[derive(Clone)]
enum UnifiedRequestAuth {
    Provider(UnifiedCredentialProvider),
    #[cfg(test)]
    Resolver(Arc<TestUnifiedResolver>),
}

impl UnifiedRequestAuth {
    async fn get(&self) -> std::result::Result<UnifiedCredentialValue, CliError> {
        match self {
            Self::Provider(provider) => provider.get().await,
            #[cfg(test)]
            Self::Resolver(resolver) => resolver(),
        }
    }
}

/// Instance collection visibility of the selected Resource authentication strategy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum InstanceListingScope {
    /// AK/SK can enumerate the account's visible Instances.
    All,
    /// OAuth can access only the Instance bound during authorization.
    Bound(String),
    /// OAuth credentials exist, but their bound Instance is not available locally.
    UnknownOAuthBinding,
}

#[derive(Clone)]
struct AkskRequestAuth {
    access_key: String,
    secret_key: String,
    security_token: Option<String>,
}

// [Review Fix #1] Keep the applied Token carrier non-Debug so assertion or
// diagnostic formatting cannot accidentally print credential material.
struct AppliedResponse {
    response: reqwest::Response,
    oauth_access_token: Option<String>,
    redaction_context: AppliedRedactionContext,
}

// [Review Fix #5] Keep exact attempt credentials available until response
// parsing, without allowing Debug output to expose their values.
#[derive(Default)]
struct AppliedRedactionContext {
    exact_values: Vec<String>,
}

impl AppliedRedactionContext {
    fn from_values(values: impl IntoIterator<Item = String>) -> Self {
        Self {
            exact_values: values
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect(),
        }
    }

    fn sanitize(&self, message: &str) -> String {
        self.exact_values
            .iter()
            .fold(message.to_string(), |safe, value| {
                safe.replace(value, "***")
            })
    }
}

enum ConsumedAttempt<T> {
    Success(T),
    Unauthorized(AppliedResponse),
}

#[derive(Clone, Copy)]
enum RetrySafety {
    MethodDefault,
    Idempotent,
}

impl RetrySafety {
    fn is_idempotent(self, method: &Method) -> bool {
        // [Review Fix #1] A replayable body is insufficient when repeating the
        // operation itself can create an additional side effect.
        matches!(self, Self::Idempotent) || is_request_retry_safe(method)
    }
}

impl Client {
    pub fn new(
        access_key: String,
        secret_key: String,
        security_token: Option<String>,
        endpoint: Option<String>,
        region: Option<String>,
        options: ClientOptions,
    ) -> Result<Self> {
        Self::new_with_auth(
            RequestAuth::Aksk(AkskRequestAuth {
                access_key,
                secret_key,
                security_token,
            }),
            endpoint,
            region,
            options,
        )
    }

    /// Build an ADrive Resource Client that authenticates with OAuth Bearer Tokens.
    pub fn new_oauth(
        token_manager: OAuthTokenManager,
        endpoint: Option<String>,
        region: Option<String>,
        options: ClientOptions,
    ) -> Result<Self> {
        Self::new_with_auth(RequestAuth::OAuth(token_manager), endpoint, region, options)
    }

    /// Build an ADrive Resource Client that signs each HTTP attempt with Unified credentials.
    ///
    /// The provider is invoked once inside each request-attempt boundary. Credential caching,
    /// refresh, and provider retry behavior remain owned by the unified-login SDK.
    ///
    /// # Parameters
    ///
    /// * `provider` - Unified-login SDK adapter for the selected profile.
    /// * `endpoint` - ADrive Resource Server endpoint.
    /// * `region` - Region used by HMAC request signing.
    /// * `options` - HTTP timeout, connection, and retry settings.
    ///
    /// # Returns
    ///
    /// A client configured for per-attempt Unified HMAC authentication.
    ///
    /// # Errors
    ///
    /// Returns an error when endpoint/region validation or HTTP client construction fails.
    pub fn new_unified(
        provider: UnifiedCredentialProvider,
        endpoint: Option<String>,
        region: Option<String>,
        options: ClientOptions,
    ) -> Result<Self> {
        Self::new_with_auth(
            RequestAuth::Unified(UnifiedRequestAuth::Provider(provider)),
            endpoint,
            region,
            options,
        )
    }

    #[cfg(test)]
    fn new_unified_for_test(
        resolver: impl Fn() -> std::result::Result<UnifiedCredentialValue, CliError>
            + Send
            + Sync
            + 'static,
        endpoint: Option<String>,
        region: Option<String>,
        options: ClientOptions,
    ) -> Result<Self> {
        Self::new_with_auth(
            RequestAuth::Unified(UnifiedRequestAuth::Resolver(Arc::new(resolver))),
            endpoint,
            region,
            options,
        )
    }

    /// Return this Resource client with the invocation-scoped request-ID trace.
    pub(crate) fn with_request_trace(mut self, request_trace: Arc<ServiceRequestTrace>) -> Self {
        Arc::make_mut(&mut self.inner).request_trace = request_trace;
        self
    }

    /// Return the Instance-listing visibility of the selected authentication strategy.
    pub(crate) fn instance_listing_scope(&self) -> Result<InstanceListingScope> {
        match &self.inner.auth {
            RequestAuth::Aksk(_) | RequestAuth::Unified(_) => Ok(InstanceListingScope::All),
            RequestAuth::OAuth(manager) => manager
                .bound_instance_id()
                .map(|instance_id| {
                    instance_id.map_or(
                        InstanceListingScope::UnknownOAuthBinding,
                        InstanceListingScope::Bound,
                    )
                })
                .map_err(Error::Cli),
        }
    }

    /// Return whether this Resource client uses OAuth Bearer authentication.
    pub(crate) fn uses_oauth(&self) -> bool {
        matches!(&self.inner.auth, RequestAuth::OAuth(_))
    }

    /// Return the selected OAuth credential's bound user ID, if OAuth is in use.
    ///
    /// AK/SK clients have no OAuth identity and return `Ok(None)`. OAuth
    /// clients read the selected credential source and normalize its user ID.
    pub(crate) fn oauth_user_id(&self) -> Result<Option<String>> {
        match &self.inner.auth {
            RequestAuth::Aksk(_) | RequestAuth::Unified(_) => Ok(None),
            RequestAuth::OAuth(manager) => manager.bound_user_id().map_err(Error::Cli),
        }
    }

    fn new_with_auth(
        auth: RequestAuth,
        endpoint: Option<String>,
        region: Option<String>,
        options: ClientOptions,
    ) -> Result<Self> {
        let (endpoint, region) = resolve_endpoint_and_region(endpoint, region)?;
        if let RequestAuth::OAuth(manager) = &auth {
            // [Review Fix #5] Recheck issuer/resource origin separation on
            // every process start because Resource config may change post-login.
            manager.validate_resource_origin(&endpoint)?;
        }
        let http = reqwest::Client::builder()
            .user_agent(user_agent())
            .tcp_nodelay(true)
            .connect_timeout(Duration::from_secs(
                options
                    .connecttimeout
                    .unwrap_or(DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS),
            ))
            .read_timeout(Duration::from_secs(
                options
                    .requesttimeout
                    .unwrap_or(DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS),
            ))
            .pool_max_idle_per_host(
                options
                    .maxconnections
                    .unwrap_or(DEFAULT_HTTP_MAX_CONNECTIONS),
            )
            .build()
            .map_err(Error::Http)?;
        let max_retry_count = options
            .max_retry_count
            .unwrap_or(DEFAULT_HTTP_MAX_RETRY_COUNT);

        Ok(Self {
            inner: Arc::new(ClientInner {
                auth,
                endpoint,
                region,
                http,
                max_retry_count,
                request_trace: Arc::new(ServiceRequestTrace::default()),
            }),
        })
    }

    pub async fn create_instance(
        &self,
        input: &CreateInstanceInput,
    ) -> Result<CreateInstanceOutput> {
        self.do_json(Method::POST, "/v1/instances", None, Some(input))
            .await
    }

    pub async fn get_instance(&self, input: &GetInstanceInput) -> Result<GetInstanceOutput> {
        let mut output: GetInstanceOutput = self
            .do_json(
                Method::GET,
                &format!("/v1/instances/{}", input.instance),
                None,
                None::<&()>,
            )
            .await?;
        if output.instance.instance_id.is_empty() {
            output.instance.instance_id = input.instance.clone();
        }
        Ok(output)
    }

    pub async fn get_instance_by_name(
        &self,
        input: &GetInstanceByNameInput,
    ) -> Result<GetInstanceOutput> {
        if input.name.is_empty() {
            return Err(Error::client("Name is required"));
        }
        let query = input.to_query_pairs();
        self.do_json(
            Method::GET,
            "/v1/instances:getByName",
            optional_query(&query),
            None::<&()>,
        )
        .await
    }

    pub async fn list_instances(&self, input: &ListInstancesInput) -> Result<ListInstancesOutput> {
        let query = input.to_query_pairs();
        self.do_json(
            Method::GET,
            "/v1/instances",
            optional_query(&query),
            None::<&()>,
        )
        .await
    }

    pub async fn delete_instance(
        &self,
        input: &DeleteInstanceInput,
    ) -> Result<DeleteInstanceOutput> {
        self.do_json(
            Method::DELETE,
            &format!("/v1/instances/{}", input.instance_id),
            None,
            None::<&()>,
        )
        .await
    }

    pub async fn create_space(&self, input: &CreateSpaceInput) -> Result<CreateSpaceOutput> {
        self.do_json(
            Method::POST,
            &format!("/v1/instances/{}/spaces", input.instance_id),
            None,
            Some(input),
        )
        .await
    }

    /// Create a Space whose OAuth user owner is resolved for each request attempt.
    pub(crate) async fn create_space_with_oauth_default_owner(
        &self,
        input: &CreateSpaceInput,
    ) -> Result<CreateSpaceOutput> {
        let path = format!("/v1/instances/{}/spaces", input.instance_id);
        let mut consume = |applied| self.parse_json_response(applied);
        let first = self
            .send_oauth_default_space_with_consumer(&path, input, true, &mut consume)
            .await?;
        let applied = match first {
            ConsumedAttempt::Success(output) => return Ok(output),
            ConsumedAttempt::Unauthorized(applied) => applied,
        };
        let RequestAuth::OAuth(manager) = &self.inner.auth else {
            return self.create_space(input).await;
        };
        let rejected_access_token = applied.oauth_access_token.as_deref().ok_or_else(|| {
            Error::InvalidResponse("OAuth request did not record its Access Token".to_string())
        })?;
        manager.force_refresh(rejected_access_token).await?;
        // [Review Fix #5] The forced-refresh replay must use the fresh Token
        // until expiry instead of immediately applying the proactive window.
        match self
            .send_oauth_default_space_with_consumer(&path, input, false, &mut consume)
            .await?
        {
            ConsumedAttempt::Success(output) => Ok(output),
            ConsumedAttempt::Unauthorized(applied) => Err(self.oauth_login_required(&applied)),
        }
    }

    pub async fn get_space(&self, input: &GetSpaceInput) -> Result<GetSpaceOutput> {
        let mut output: GetSpaceOutput = self
            .do_json(
                Method::GET,
                &format!("/v1/instances/{}/spaces/{}", input.instance_id, input.space),
                None,
                None::<&()>,
            )
            .await?;
        if output.space.instance_id.is_empty() {
            output.space.instance_id = input.instance_id.clone();
        }
        if output.space.space_id.is_empty() {
            output.space.space_id = input.space.clone();
        }
        Ok(output)
    }

    pub async fn get_space_by_name(&self, input: &GetSpaceByNameInput) -> Result<GetSpaceOutput> {
        if input.space_name.is_empty() {
            return Err(Error::client("SpaceName is required"));
        }
        if input.instance_id.is_empty() == input.instance_name.is_empty() {
            return Err(Error::client(
                "exactly one of InstanceID or InstanceName is required",
            ));
        }
        let query = input.to_query_pairs();
        let mut output: GetSpaceOutput = self
            .do_json(
                Method::GET,
                "/v1/spaces:getByName",
                optional_query(&query),
                None::<&()>,
            )
            .await?;
        if output.space.instance_id.is_empty() {
            output.space.instance_id = if input.instance_id.is_empty() {
                input.instance_name.clone()
            } else {
                input.instance_id.clone()
            };
        }
        if output.space.space_id.is_empty() {
            output.space.space_id = input.space_name.clone();
        }
        Ok(output)
    }

    pub async fn list_spaces(&self, input: &ListSpacesInput) -> Result<ListSpacesOutput> {
        let query = input.to_query_pairs();
        self.do_json(
            Method::GET,
            &format!("/v1/instances/{}/spaces", input.instance_id),
            optional_query(&query),
            None::<&()>,
        )
        .await
    }

    /// List Spaces owned by the current OAuth user.
    pub async fn list_my_spaces(&self, input: &ListMySpacesInput) -> Result<ListMySpacesOutput> {
        let query = input.to_query_pairs();
        self.do_json(
            Method::GET,
            &format!("/v1/instances/{}/myspaces", input.instance_id),
            optional_query(&query),
            None::<&()>,
        )
        .await
    }

    /// List Spaces owned by the current OAuth user's group.
    pub async fn list_my_group_spaces(
        &self,
        input: &ListMyGroupSpacesInput,
    ) -> Result<ListMyGroupSpacesOutput> {
        let query = input.to_query_pairs();
        self.do_json(
            Method::GET,
            &format!("/v1/instances/{}/mygroupspaces", input.instance_id),
            optional_query(&query),
            None::<&()>,
        )
        .await
    }

    pub async fn delete_space(&self, input: &DeleteSpaceInput) -> Result<DeleteSpaceOutput> {
        self.do_json(
            Method::DELETE,
            &format!(
                "/v1/instances/{}/spaces/{}",
                input.instance_id, input.space_id
            ),
            None,
            None::<&()>,
        )
        .await
    }

    pub async fn list_files(&self, input: &ListFilesInput) -> Result<ListFilesOutput> {
        let query = input.to_query_pairs();
        self.do_json(
            Method::GET,
            &format!(
                "/v1/instances/{}/spaces/{}/files",
                input.instance_id, input.space_id
            ),
            optional_query(&query),
            None::<&()>,
        )
        .await
    }

    pub async fn put_file(&self, input: PutFileInput) -> Result<PutFileOutput> {
        let content_length = input.content_length.or_else(|| input.body.content_length());
        let bytes = input.body.into_bytes(content_length).await?;
        throttle_body(input.rate_limiter.as_deref(), bytes.len()).await;

        let mut query = Vec::new();
        if let Some(auto_index) = input.auto_index {
            query.push(("autoIndex".to_string(), auto_index.to_string()));
        }
        let mut headers = HeaderMap::new();
        if let Some(meta) = &input.meta {
            for (key, value) in meta {
                insert_meta_header(&mut headers, key, value)?;
            }
        }
        let mut output: PutFileOutput = self
            .do_body_json(
                Method::POST,
                &format!(
                    "/v1/instances/{}/spaces/{}/files/{}",
                    input.instance_id, input.space_id, input.file_path
                ),
                optional_query(&query),
                headers,
                bytes,
                input
                    .content_type
                    .as_deref()
                    .or(Some("application/octet-stream")),
                content_length,
            )
            .await?;
        if output.instance_id.is_empty() {
            output.instance_id = input.instance_id;
        }
        if output.space_id.is_empty() {
            output.space_id = input.space_id;
        }
        if output.file_path.is_empty() {
            output.file_path = input.file_path;
        }
        if let Some(version_id) = output.response_info.header("x-ids-version-id") {
            output.version_id = version_id.to_string();
        }
        Ok(output)
    }

    pub async fn get_file(&self, input: &GetFileInput) -> Result<GetFileOutput> {
        let headers = get_file_headers(input)?;
        let applied = self
            .send_request(
                Method::GET,
                &format!(
                    "/v1/instances/{}/spaces/{}/files/{}",
                    input.instance_id, input.space_id, input.file_path
                ),
                None,
                headers,
                None,
                None,
                None,
            )
            .await?;
        self.get_file_output(applied)
    }

    /// Download a file through a consumer that finishes reading and validating
    /// the response stream before the HTTP attempt is considered successful.
    ///
    /// The consumer receives a fresh stream for every retry. It must therefore
    /// recreate or roll back any local output before writing the next attempt.
    pub async fn get_file_with_consumer<T, C>(
        &self,
        input: &GetFileInput,
        mut consume: C,
    ) -> Result<T>
    where
        C: FnMut(GetFileOutput) -> Pin<Box<dyn Future<Output = Result<T>> + Send>>,
    {
        let headers = get_file_headers(input)?;
        self.send_request_with_consumer(
            Method::GET,
            &format!(
                "/v1/instances/{}/spaces/{}/files/{}",
                input.instance_id, input.space_id, input.file_path
            ),
            None,
            headers,
            None,
            None,
            None,
            |applied| match self.get_file_output(applied) {
                Ok(output) => consume(output),
                Err(error) => Box::pin(async move { Err(error) }),
            },
        )
        .await
    }

    fn get_file_output(&self, applied: AppliedResponse) -> Result<GetFileOutput> {
        let AppliedResponse {
            response,
            redaction_context,
            ..
        } = applied;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        self.check_status(status, &headers, &[], &redaction_context)?;
        let response_info = Self::build_response_info(status, &headers);
        let stream = response.bytes_stream().map(|chunk| {
            chunk
                .map(|bytes| bytes.to_vec())
                .map_err(reqwest_body_io_error)
        });

        Ok(GetFileOutput::new(
            response_info,
            content_length_from_headers(&headers),
            header_str(&headers, "content-type"),
            headers
                .get("content-range")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string),
            header_str(&headers, "x-ids-file-etag"),
            header_u64(&headers, "x-ids-file-hash-crc64-ecma"),
            header_i64(&headers, "x-ids-file-created-at"),
            header_i64(&headers, "x-ids-file-updated-at"),
            header_str(&headers, "x-ids-file-type"),
            header_str(&headers, "x-ids-file-storage-class"),
            metadata_headers(&headers),
            header_str(&headers, "x-ids-file-is-folder") == "true",
            Box::pin(stream),
        ))
    }

    pub async fn head_file(&self, input: &HeadFileInput) -> Result<HeadFileOutput> {
        let applied = self
            .send_request(
                Method::HEAD,
                &format!(
                    "/v1/instances/{}/spaces/{}/files/{}",
                    input.instance_id, input.space_id, input.file_path
                ),
                None,
                HeaderMap::new(),
                None,
                None,
                None,
            )
            .await?;
        let AppliedResponse {
            response,
            redaction_context,
            ..
        } = applied;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        self.check_status(status, &headers, &[], &redaction_context)?;
        Ok(HeadFileOutput {
            response_info: Self::build_response_info(status, &headers),
            content_length: content_length_from_headers(&headers),
            content_type: header_str(&headers, "content-type"),
            etag: header_str(&headers, "x-ids-file-etag"),
            hash_crc64_ecma: header_u64(&headers, "x-ids-file-hash-crc64-ecma"),
            created_at: header_i64(&headers, "x-ids-file-created-at"),
            updated_at: header_i64(&headers, "x-ids-file-updated-at"),
            file_type: header_str(&headers, "x-ids-file-type"),
            storage_class: header_str(&headers, "x-ids-file-storage-class"),
            meta: metadata_headers(&headers),
            is_folder: header_str(&headers, "x-ids-is-folder") == "true"
                || header_str(&headers, "x-ids-file-is-folder") == "true",
        })
    }

    pub async fn delete_file(&self, input: &DeleteFileInput) -> Result<DeleteFileOutput> {
        let mut headers = HeaderMap::new();
        if let Some(if_match) = &input.if_match {
            headers.insert(
                reqwest::header::IF_MATCH,
                HeaderValue::from_str(if_match)
                    .map_err(|err| Error::client(format!("invalid if-match header: {err}")))?,
            );
        }
        let response_info = self
            .do_no_content(
                Method::DELETE,
                &format!(
                    "/v1/instances/{}/spaces/{}/files/{}",
                    input.instance_id, input.space_id, input.file_path
                ),
                headers,
                None,
            )
            .await?;
        let version_id = response_info
            .header("x-ids-version-id")
            .unwrap_or_default()
            .to_string();
        let delete_marker = response_info
            .header("x-ids-delete-marker")
            .unwrap_or_default()
            == "true";
        Ok(DeleteFileOutput {
            response_info,
            version_id,
            delete_marker,
        })
    }

    pub async fn rename_file(&self, input: &RenameFileInput) -> Result<RenameFileOutput> {
        self.do_json(
            Method::POST,
            &format!(
                "/v1/instances/{}/spaces/{}/files/{}:rename",
                input.instance_id, input.space_id, input.file_path
            ),
            None,
            Some(input),
        )
        .await
    }

    pub async fn copy_file(&self, input: &CopyFileInput) -> Result<CopyFileOutput> {
        let mut headers = HeaderMap::new();
        if let Some(if_match) = &input.copy_source_if_match {
            headers.insert(
                HeaderName::from_static("x-ids-copy-source-if-match"),
                HeaderValue::from_str(if_match).map_err(|err| {
                    Error::client(format!("invalid copy-source-if-match header: {err}"))
                })?,
            );
        }
        self.do_json_with_headers(
            Method::POST,
            &format!(
                "/v1/instances/{}/spaces/{}/files/{}:copy",
                input.instance_id, input.space_id, input.file_path
            ),
            None,
            headers,
            Some(input),
        )
        .await
    }

    pub async fn create_folder(&self, input: &CreateFolderInput) -> Result<CreateFolderOutput> {
        let mut output: CreateFolderOutput = self
            .do_json(
                Method::POST,
                &format!(
                    "/v1/instances/{}/spaces/{}/folders",
                    input.instance_id, input.space_id
                ),
                None,
                Some(input),
            )
            .await?;
        if output.instance_id.is_empty() {
            output.instance_id = input.instance_id.clone();
        }
        if output.space_id.is_empty() {
            output.space_id = input.space_id.clone();
        }
        Ok(output)
    }

    pub async fn delete_folder(&self, input: &DeleteFolderInput) -> Result<DeleteFolderOutput> {
        self.do_json(
            Method::DELETE,
            &format!(
                "/v1/instances/{}/spaces/{}/folders/{}",
                input.instance_id, input.space_id, input.folder_path
            ),
            None,
            None::<&()>,
        )
        .await
    }

    pub async fn rename_folder(&self, input: &RenameFolderInput) -> Result<RenameFolderOutput> {
        self.do_json(
            Method::POST,
            &format!(
                "/v1/instances/{}/spaces/{}/folders/{}",
                input.instance_id, input.space_id, input.folder_path
            ),
            None,
            Some(input),
        )
        .await
    }

    pub async fn initiate_multipart_upload(
        &self,
        input: &InitiateMultipartUploadInput,
    ) -> Result<InitiateMultipartUploadOutput> {
        let mut output: InitiateMultipartUploadOutput = self
            .do_json(
                Method::POST,
                &format!(
                    "/v1/instances/{}/spaces/{}/files/{}:initiateMultipart",
                    input.instance_id, input.space_id, input.file_path
                ),
                None,
                Some(input),
            )
            .await?;
        if output.instance_id.is_empty() {
            output.instance_id = input.instance_id.clone();
        }
        if output.space_id.is_empty() {
            output.space_id = input.space_id.clone();
        }
        if output.file_path.is_empty() {
            output.file_path = input.file_path.clone();
        }
        Ok(output)
    }

    pub async fn upload_part(&self, input: UploadPartInput) -> Result<UploadPartOutput> {
        let query = vec![
            ("uploadId".to_string(), input.upload_id.clone()),
            ("partNumber".to_string(), input.part_number.to_string()),
        ];
        let content_length = input.content_length.or_else(|| input.body.content_length());
        let bytes = input.body.into_bytes(content_length).await?;
        throttle_body(input.rate_limiter.as_deref(), bytes.len()).await;
        self.do_body_json(
            Method::PUT,
            &format!(
                "/v1/instances/{}/spaces/{}/files/{}:uploadPart",
                input.instance_id, input.space_id, input.file_path
            ),
            Some(&query),
            HeaderMap::new(),
            bytes,
            Some("application/octet-stream"),
            content_length,
        )
        .await
    }

    pub async fn complete_multipart_upload(
        &self,
        input: &CompleteMultipartUploadInput,
    ) -> Result<CompleteMultipartUploadOutput> {
        let mut output: CompleteMultipartUploadOutput = self
            .do_json(
                Method::POST,
                &format!(
                    "/v1/instances/{}/spaces/{}/files/{}:completeMultipart",
                    input.instance_id, input.space_id, input.file_path
                ),
                None,
                Some(input),
            )
            .await?;
        if output.instance_id.is_empty() {
            output.instance_id = input.instance_id.clone();
        }
        if output.space_id.is_empty() {
            output.space_id = input.space_id.clone();
        }
        if output.file_path.is_empty() {
            output.file_path = input.file_path.clone();
        }
        Ok(output)
    }

    pub async fn abort_multipart_upload(
        &self,
        input: &AbortMultipartUploadInput,
    ) -> Result<ResponseInfo> {
        let query = vec![("uploadId".to_string(), input.upload_id.clone())];
        self.do_no_content(
            Method::DELETE,
            &format!(
                "/v1/instances/{}/spaces/{}/files/{}:abortMultipart",
                input.instance_id, input.space_id, input.file_path
            ),
            HeaderMap::new(),
            Some(&query),
        )
        .await
    }

    pub async fn search_files(&self, input: &SearchFilesInput) -> Result<SearchFilesOutput> {
        self.do_idempotent_json(
            Method::POST,
            &format!(
                "/v1/instances/{}/spaces/{}/search",
                input.instance_id, input.space_id
            ),
            None,
            Some(input),
        )
        .await
    }

    fn build_response_info(status: u16, headers: &HeaderMap) -> ResponseInfo {
        let request_id = resource_request_id(headers).unwrap_or_default();
        if !request_id.is_empty() {
            std::env::set_var("TOS_LAST_REQUEST_ID", &request_id);
        }

        let mut header_map = HashMap::new();
        for (key, value) in headers {
            if let Ok(value) = value.to_str() {
                header_map.insert(key.to_string(), value.to_string());
            }
        }

        ResponseInfo {
            request_id,
            status_code: status,
            headers: header_map,
        }
    }

    async fn do_json<Req, Resp>(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        body: Option<&Req>,
    ) -> Result<Resp>
    where
        Req: Serialize + ?Sized,
        Resp: DeserializeOwned + HasResponseInfo,
    {
        self.do_json_with_headers(method, path, query, HeaderMap::new(), body)
            .await
    }

    async fn do_idempotent_json<Req, Resp>(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        body: Option<&Req>,
    ) -> Result<Resp>
    where
        Req: Serialize + ?Sized,
        Resp: DeserializeOwned + HasResponseInfo,
    {
        self.do_json_with_headers_and_retry_safety(
            method,
            path,
            query,
            HeaderMap::new(),
            body,
            RetrySafety::Idempotent,
        )
        .await
    }

    async fn do_json_with_headers<Req, Resp>(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        headers: HeaderMap,
        body: Option<&Req>,
    ) -> Result<Resp>
    where
        Req: Serialize + ?Sized,
        Resp: DeserializeOwned + HasResponseInfo,
    {
        self.do_json_with_headers_and_retry_safety(
            method,
            path,
            query,
            headers,
            body,
            RetrySafety::MethodDefault,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn do_json_with_headers_and_retry_safety<Req, Resp>(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        headers: HeaderMap,
        body: Option<&Req>,
        retry_safety: RetrySafety,
    ) -> Result<Resp>
    where
        Req: Serialize + ?Sized,
        Resp: DeserializeOwned + HasResponseInfo,
    {
        let body_bytes = match body {
            Some(body) => Some(serde_json::to_vec(body)?),
            None => None,
        };
        // [Review Fix #3] Keep no-body GET/DELETE requests header-compatible with the SDK boundary.
        let content_type = body_bytes.as_ref().map(|_| "application/json");
        self.send_request_with_consumer_and_retry_safety(
            method,
            path,
            query,
            headers,
            body_bytes,
            content_type,
            None,
            retry_safety,
            |response| self.parse_json_response(response),
        )
        .await
    }

    async fn do_body_json<Resp>(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        headers: HeaderMap,
        body: Vec<u8>,
        content_type: Option<&str>,
        content_length: Option<u64>,
    ) -> Result<Resp>
    where
        Resp: DeserializeOwned + HasResponseInfo,
    {
        self.send_request_with_consumer(
            method,
            path,
            query,
            headers,
            Some(body),
            content_type,
            content_length,
            |response| self.parse_json_response(response),
        )
        .await
    }

    async fn parse_json_response<Resp>(&self, applied: AppliedResponse) -> Result<Resp>
    where
        Resp: DeserializeOwned + HasResponseInfo,
    {
        let AppliedResponse {
            response,
            redaction_context,
            ..
        } = applied;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = read_limited_response_body(response).await?;
        self.check_status(status, &headers, &body, &redaction_context)?;
        if body.is_empty() {
            return Err(Error::InvalidResponse("empty response body".to_string()));
        }
        let mut output = serde_json::from_slice::<Resp>(&body)?;
        output.set_response_info(Self::build_response_info(status, &headers));
        Ok(output)
    }

    async fn do_no_content(
        &self,
        method: Method,
        path: &str,
        headers: HeaderMap,
        query: Option<&Vec<(String, String)>>,
    ) -> Result<ResponseInfo> {
        self.send_request_with_consumer(
            method,
            path,
            query,
            headers,
            None,
            None,
            None,
            |applied| async move {
                let AppliedResponse {
                    response,
                    redaction_context,
                    ..
                } = applied;
                let status = response.status().as_u16();
                let headers = response.headers().clone();
                let body = if status >= 400 {
                    read_limited_response_body(response).await?
                } else {
                    Vec::new()
                };
                self.check_status(status, &headers, &body, &redaction_context)?;
                Ok(Self::build_response_info(status, &headers))
            },
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_request_with_consumer<T, C, CFut>(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
        content_length: Option<u64>,
        consume: C,
    ) -> Result<T>
    where
        C: FnMut(AppliedResponse) -> CFut,
        CFut: std::future::Future<Output = Result<T>>,
    {
        self.send_request_with_consumer_and_retry_safety(
            method,
            path,
            query,
            headers,
            body,
            content_type,
            content_length,
            RetrySafety::MethodDefault,
            consume,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_request_with_consumer_and_retry_safety<T, C, CFut>(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
        content_length: Option<u64>,
        retry_safety: RetrySafety,
        mut consume: C,
    ) -> Result<T>
    where
        C: FnMut(AppliedResponse) -> CFut,
        CFut: std::future::Future<Output = Result<T>>,
    {
        let first = self
            .send_with_consumer_retries(
                method.clone(),
                path,
                query,
                headers.clone(),
                body.clone(),
                content_type,
                content_length,
                retry_safety,
                &mut consume,
            )
            .await?;
        let applied = match first {
            ConsumedAttempt::Success(value) => return Ok(value),
            ConsumedAttempt::Unauthorized(applied) => applied,
        };
        let RequestAuth::OAuth(manager) = &self.inner.auth else {
            return consume(applied).await;
        };
        let rejected_access_token = applied.oauth_access_token.as_deref().ok_or_else(|| {
            Error::InvalidResponse("OAuth request did not record its Access Token".to_string())
        })?;
        manager.force_refresh(rejected_access_token).await?;
        match self
            .send_with_consumer_retries(
                method,
                path,
                query,
                headers,
                body,
                content_type,
                content_length,
                retry_safety,
                &mut consume,
            )
            .await?
        {
            ConsumedAttempt::Success(value) => Ok(value),
            ConsumedAttempt::Unauthorized(applied) => Err(self.oauth_login_required(&applied)),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_with_consumer_retries<T, C, CFut>(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
        content_length: Option<u64>,
        retry_safety: RetrySafety,
        consume: &mut C,
    ) -> Result<ConsumedAttempt<T>>
    where
        C: FnMut(AppliedResponse) -> CFut,
        CFut: std::future::Future<Output = Result<T>>,
    {
        let is_idempotent = retry_safety.is_idempotent(&method);
        for attempt in 0..=self.inner.max_retry_count {
            let result = self
                .send_once(
                    method.clone(),
                    path,
                    query,
                    headers.clone(),
                    body.clone(),
                    content_type,
                    content_length,
                )
                .await;
            match result {
                Ok(applied) if applied.response.status().as_u16() == 401 => {
                    return Ok(ConsumedAttempt::Unauthorized(applied));
                }
                Ok(applied)
                    if should_retry_storage_status(applied.response.status(), is_idempotent)
                        && attempt < self.inner.max_retry_count =>
                {
                    sleep_before_response_retry(attempt, applied.response).await;
                }
                Ok(applied) => match consume(applied).await {
                    Ok(value) => return Ok(ConsumedAttempt::Success(value)),
                    Err(error)
                        if is_idempotent
                            && should_retry_error(&error)
                            && attempt < self.inner.max_retry_count =>
                    {
                        sleep_before_retry(attempt).await;
                    }
                    Err(error) => return Err(error),
                },
                Err(error)
                    if should_retry_request_error(&error, is_idempotent)
                        && attempt < self.inner.max_retry_count =>
                {
                    sleep_before_retry(attempt).await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(Error::InvalidResponse("retry loop exhausted".to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_request(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
        content_length: Option<u64>,
    ) -> Result<AppliedResponse> {
        let applied = self
            .send_with_transient_retries(
                method.clone(),
                path,
                query,
                headers.clone(),
                body.clone(),
                content_type,
                content_length,
            )
            .await?;
        if applied.response.status().as_u16() != 401 {
            return Ok(applied);
        }
        let RequestAuth::OAuth(manager) = &self.inner.auth else {
            return Ok(applied);
        };
        let rejected_access_token = applied.oauth_access_token.as_deref().ok_or_else(|| {
            Error::InvalidResponse("OAuth request did not record its Access Token".to_string())
        })?;
        manager.force_refresh(rejected_access_token).await?;
        let replay = self
            .send_with_transient_retries(
                method,
                path,
                query,
                headers,
                body,
                content_type,
                content_length,
            )
            .await?;
        if replay.response.status().as_u16() == 401 {
            return Err(self.oauth_login_required(&replay));
        }
        Ok(replay)
    }

    async fn send_oauth_default_space_with_consumer<T, C, CFut>(
        &self,
        path: &str,
        input: &CreateSpaceInput,
        apply_refresh_window: bool,
        consume: &mut C,
    ) -> Result<ConsumedAttempt<T>>
    where
        C: FnMut(AppliedResponse) -> CFut,
        CFut: Future<Output = Result<T>>,
    {
        // [Review Fix #3] Space creation is non-idempotent, so an ambiguous
        // 408 or local timeout cannot replay the operation.
        let is_idempotent = false;
        // Resolve the OAuth owner again for every attempt so a refreshed Token
        // cannot leave the retried request bound to stale identity metadata.
        for attempt in 0..=self.inner.max_retry_count {
            match self
                .send_oauth_default_space_once(path, input, apply_refresh_window)
                .await
            {
                Ok(applied) if applied.response.status() == StatusCode::UNAUTHORIZED => {
                    return Ok(ConsumedAttempt::Unauthorized(applied));
                }
                Ok(applied)
                    if should_retry_storage_status(applied.response.status(), is_idempotent)
                        && attempt < self.inner.max_retry_count =>
                {
                    sleep_before_response_retry(attempt, applied.response).await;
                }
                Ok(applied) => return consume(applied).await.map(ConsumedAttempt::Success),
                Err(error)
                    if should_retry_request_error(&error, is_idempotent)
                        && attempt < self.inner.max_retry_count =>
                {
                    sleep_before_retry(attempt).await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(Error::InvalidResponse("retry loop exhausted".to_string()))
    }

    async fn send_oauth_default_space_once(
        &self,
        path: &str,
        input: &CreateSpaceInput,
        apply_refresh_window: bool,
    ) -> Result<AppliedResponse> {
        let RequestAuth::OAuth(manager) = &self.inner.auth else {
            return Err(Error::client(
                "OAuth Space owner defaults require OAuth authentication",
            ));
        };
        manager.validate_instance(&input.instance_id)?;
        let (access_token, user_id) = manager
            .access_token_and_user_id(apply_refresh_window)
            .await?;
        let user_id = user_id.ok_or_else(oauth_user_id_required_error)?;
        let mut request_input = input.clone();
        request_input.owner_type = Some("user".to_string());
        request_input.owner_id = Some(user_id);
        let body = serde_json::to_vec(&request_input)?;
        let url = self.request_url(path, None)?;
        let mut headers = HeaderMap::new();
        apply_content_headers(
            &url,
            &mut headers,
            Some("application/json"),
            Some(body.len() as u64),
        )?;
        let mut authorization = HeaderValue::from_str(&format!("Bearer {access_token}"))
            .map_err(|_| Error::client("invalid OAuth Authorization header"))?;
        authorization.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, authorization);
        let response_result = self
            .inner
            .http
            .post(url)
            .headers(headers)
            .body(body)
            .send()
            .await;
        let response = self.finish_request(response_result)?;
        self.record_request_id(&response);
        Ok(AppliedResponse {
            response,
            oauth_access_token: Some(access_token.clone()),
            redaction_context: AppliedRedactionContext::from_values([access_token]),
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_with_transient_retries(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
        content_length: Option<u64>,
    ) -> Result<AppliedResponse> {
        let is_idempotent = is_request_retry_safe(&method);
        for attempt in 0..=self.inner.max_retry_count {
            let result = self
                .send_once(
                    method.clone(),
                    path,
                    query,
                    headers.clone(),
                    body.clone(),
                    content_type,
                    content_length,
                )
                .await;
            match result {
                Ok(applied)
                    if should_retry_storage_status(applied.response.status(), is_idempotent)
                        && attempt < self.inner.max_retry_count =>
                {
                    // [Review Fix #4] Preserve SDK-like retry behavior for transient IDS failures.
                    sleep_before_response_retry(attempt, applied.response).await;
                }
                Ok(applied) => return Ok(applied),
                Err(err)
                    if should_retry_request_error(&err, is_idempotent)
                        && attempt < self.inner.max_retry_count =>
                {
                    // [Review Fix #4] Preserve SDK-like retry behavior for transient transport failures.
                    sleep_before_retry(attempt).await;
                }
                Err(err) => return Err(err),
            }
        }
        Err(Error::InvalidResponse("retry loop exhausted".to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_once(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        mut headers: HeaderMap,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
        content_length: Option<u64>,
    ) -> Result<AppliedResponse> {
        let url = self.request_url(path, query)?;
        headers.remove(reqwest::header::AUTHORIZATION);
        headers.remove(HeaderName::from_static("x-date"));
        headers.remove(HeaderName::from_static("x-security-token"));
        apply_content_headers(&url, &mut headers, content_type, content_length)?;
        let (oauth_access_token, redaction_context) = self
            .apply_request_auth(&method, path, query, &url, &mut headers)
            .await?;
        let mut request = self.inner.http.request(method, url).headers(headers);
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = self.finish_request(request.send().await)?;
        self.record_request_id(&response);
        Ok(AppliedResponse {
            response,
            oauth_access_token,
            redaction_context,
        })
    }

    fn request_url(
        &self,
        path: &str,
        query: Option<&Vec<(String, String)>>,
    ) -> Result<reqwest::Url> {
        let encoded_path = encode_path(path);
        let mut url = reqwest::Url::parse(&format!("{}{}", self.inner.endpoint, encoded_path))
            .map_err(|err| Error::client(format!("invalid url: {err}")))?;
        if let Some(query) = query {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }
        Ok(url)
    }

    fn record_request_id(&self, response: &reqwest::Response) {
        let request_id = resource_request_id(response.headers());
        self.inner
            .request_trace
            .record_response(request_id.as_deref(), response.status().is_success());
    }

    fn finish_request(
        &self,
        result: std::result::Result<reqwest::Response, reqwest::Error>,
    ) -> Result<reqwest::Response> {
        result.map_err(|error| {
            // [Review Fix #10] Error projection is terminal-attempt scoped;
            // do not reuse an ID from a response preceding this transport failure.
            self.inner.request_trace.record_no_response();
            Error::Http(error)
        })
    }

    async fn apply_request_auth(
        &self,
        method: &Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        url: &reqwest::Url,
        headers: &mut HeaderMap,
    ) -> Result<(Option<String>, AppliedRedactionContext)> {
        match &self.inner.auth {
            RequestAuth::Aksk(auth) => {
                self.apply_aksk_auth(auth, method, path, url, headers)?;
                Ok((None, AppliedRedactionContext::default()))
            }
            RequestAuth::Unified(source) => {
                // [Review Fix #1] Keep SDK resolution failures distinct from
                // retryable response/transport errors; the CLI must not add
                // retries around the SDK's own cache/refresh/retry policy.
                let credentials = source.get().await.map_err(Error::UnifiedCredential)?;
                let auth = AkskRequestAuth {
                    access_key: credentials.access_key_id,
                    secret_key: credentials.secret_access_key,
                    // [Review Fix #3] Match static optional-token semantics:
                    // an empty SDK token must not create or sign an empty header.
                    security_token: (!credentials.session_token.is_empty())
                        .then_some(credentials.session_token),
                };
                self.apply_aksk_auth(&auth, method, path, url, headers)?;
                let mut exact_values = Vec::new();
                if let Some(authorization) = headers
                    .get(reqwest::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                {
                    exact_values.push(authorization.to_string());
                }
                exact_values.push(auth.access_key.clone());
                if let Some(security_token) = auth.security_token.clone() {
                    exact_values.push(security_token);
                }
                Ok((None, AppliedRedactionContext::from_values(exact_values)))
            }
            RequestAuth::OAuth(manager) => {
                if let Some(instance_id) = request_instance_id(path, query) {
                    manager.validate_instance(instance_id)?;
                }
                let access_token = manager.access_token().await?;
                let mut authorization = HeaderValue::from_str(&format!("Bearer {access_token}"))
                    .map_err(|_| Error::client("invalid OAuth Authorization header"))?;
                authorization.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, authorization);
                Ok((
                    Some(access_token.clone()),
                    AppliedRedactionContext::from_values([access_token]),
                ))
            }
        }
    }

    fn apply_aksk_auth(
        &self,
        auth: &AkskRequestAuth,
        method: &Method,
        path: &str,
        url: &reqwest::Url,
        headers: &mut HeaderMap,
    ) -> Result<()> {
        let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        headers.insert(
            HeaderName::from_static("x-date"),
            HeaderValue::from_str(&timestamp)
                .map_err(|err| Error::client(format!("invalid x-date header: {err}")))?,
        );
        if let Some(security_token) = &auth.security_token {
            let mut header = HeaderValue::from_str(security_token)
                .map_err(|err| Error::client(format!("invalid security token: {err}")))?;
            header.set_sensitive(true);
            headers.insert(HeaderName::from_static("x-security-token"), header);
        }
        let header_pairs = headers
            .iter()
            .map(|(key, value)| {
                (
                    key.to_string(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect::<Vec<_>>();
        let payload_hash = sha256_hex(b"UNSIGNED-PAYLOAD");
        let authorization = sign_request(
            method.as_str(),
            url.as_str(),
            path,
            &timestamp,
            &header_pairs,
            &payload_hash,
            &auth.access_key,
            &auth.secret_key,
            &self.inner.region,
            "tos",
        );
        let mut authorization = HeaderValue::from_str(&authorization)
            .map_err(|err| Error::client(format!("invalid authorization: {err}")))?;
        authorization.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, authorization);
        Ok(())
    }

    fn check_status(
        &self,
        status: u16,
        headers: &HeaderMap,
        body: &[u8],
        redaction_context: &AppliedRedactionContext,
    ) -> Result<()> {
        if status < 400 {
            return Ok(());
        }

        let request_id = resource_request_id(headers)
            .map(|value| self.sanitize_resource_error(&value, redaction_context));
        let response_headers = headers
            .iter()
            .filter_map(|(key, value)| {
                value.to_str().ok().map(|value| {
                    (
                        key.to_string(),
                        self.sanitize_resource_error(value, redaction_context),
                    )
                })
            })
            .collect::<HashMap<_, _>>();

        if let Some(ids_error) = self.parse_ids_error(
            status,
            request_id.clone(),
            response_headers.clone(),
            body,
            redaction_context,
        ) {
            return Err(Error::Server(ids_error));
        }

        Err(Error::Server(IdsError {
            code: status.to_string(),
            // [Review Fix #5] Unstructured server bodies cannot be safely
            // inspected for credential echoes, so expose only stable metadata.
            message: "unrecognized ADrive error response".to_string(),
            request_id,
            status_code: Some(status),
            response_headers: Some(response_headers),
        }))
    }

    fn parse_ids_error(
        &self,
        status: u16,
        fallback_request_id: Option<String>,
        response_headers: HashMap<String, String>,
        body: &[u8],
        redaction_context: &AppliedRedactionContext,
    ) -> Option<IdsError> {
        let mut error = serde_json::from_slice::<ErrorEnvelope>(body)
            .ok()
            .and_then(|envelope| envelope.error)
            .or_else(|| serde_json::from_slice::<IdsError>(body).ok())?;
        error.status_code = Some(status);
        self.sanitize_ids_error(&mut error, redaction_context);
        // [Review Fix #1] The frozen contract gives the sanitized response
        // header priority; an unsafe body RequestId must not suppress it.
        if fallback_request_id.is_some() {
            error.request_id = fallback_request_id;
        }
        error.response_headers = Some(response_headers);
        Some(error)
    }

    fn sanitize_resource_error(
        &self,
        message: &str,
        redaction_context: &AppliedRedactionContext,
    ) -> String {
        let sanitized = redaction_context.sanitize(message);
        match &self.inner.auth {
            RequestAuth::Aksk(_) | RequestAuth::Unified(_) => sanitized,
            RequestAuth::OAuth(manager) => manager.redact_resource_error(&sanitized),
        }
    }

    fn sanitize_ids_error(
        &self,
        error: &mut IdsError,
        redaction_context: &AppliedRedactionContext,
    ) {
        error.code = self.sanitize_resource_error(&error.code, redaction_context);
        error.message = self.sanitize_resource_error(&error.message, redaction_context);
        if let Some(request_id) = error.request_id.as_mut() {
            *request_id = self.sanitize_resource_error(request_id, redaction_context);
        }
        error.request_id = error.request_id.as_deref().and_then(sanitize_request_id);
    }

    fn oauth_login_required(&self, applied: &AppliedResponse) -> Error {
        let request_id = resource_request_id(applied.response.headers())
            .map(|value| {
                format!(
                    " (RequestId: {})",
                    self.sanitize_resource_error(&value, &applied.redaction_context)
                )
            })
            .unwrap_or_default();
        Error::Cli(CliError::AuthFailed(format!(
            "HTTP 401 [login_required] Access Token remained unauthorized after one refresh{request_id}; run ve-adrive auth login"
        )))
    }
}

fn resource_request_id(headers: &HeaderMap) -> Option<String> {
    ["x-ids-request-id", "x-request-id"]
        .into_iter()
        .find_map(|name| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(sanitize_request_id)
        })
}

pub(crate) fn resolve_endpoint_and_region(
    endpoint: Option<String>,
    region: Option<String>,
) -> Result<(String, String)> {
    // [Review Fix #28] Treat whitespace-only values as missing instead of
    // normalizing them into the invalid resource URL `https://`.
    let endpoint = endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_endpoint_scheme)
        .ok_or_else(|| {
            Error::client(
                "ADRIVE_ENDPOINT is required; configure --endpoint, [profile.adrive].endpoint, or ADRIVE_ENDPOINT",
            )
        })?;
    validate_resource_endpoint(&endpoint)?;
    let region = region
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| derive_region_from_endpoint(&endpoint))
        .ok_or_else(|| {
            Error::client(
                "ADRIVE_REGION is required when region cannot be derived from ADRIVE_ENDPOINT",
            )
        })?;
    Ok((endpoint, region))
}

fn validate_resource_endpoint(endpoint: &str) -> Result<()> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| {
        Error::client("ADrive resource endpoint must use HTTP or HTTPS and include a host")
    })?;
    // [Review Fix #2] Reject opaque/custom-scheme URLs before request signing
    // can derive an empty Host header and reqwest reports a late builder error.
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Error::client(
            "ADrive resource endpoint must use HTTP or HTTPS and include a host",
        ));
    }
    Ok(())
}

pub trait HasResponseInfo {
    fn set_response_info(&mut self, info: ResponseInfo);
}

macro_rules! impl_has_response_info {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl HasResponseInfo for $ty {
                fn set_response_info(&mut self, info: ResponseInfo) {
                    self.response_info = info;
                }
            }
        )+
    };
}

impl_has_response_info!(
    CreateInstanceOutput,
    GetInstanceOutput,
    ListInstancesOutput,
    DeleteInstanceOutput,
    CreateSpaceOutput,
    GetSpaceOutput,
    ListSpacesOutput,
    ListMySpacesOutput,
    ListMyGroupSpacesOutput,
    DeleteSpaceOutput,
    ListFilesOutput,
    PutFileOutput,
    RenameFileOutput,
    CopyFileOutput,
    CreateFolderOutput,
    DeleteFolderOutput,
    RenameFolderOutput,
    InitiateMultipartUploadOutput,
    UploadPartOutput,
    CompleteMultipartUploadOutput,
    SearchFilesOutput,
);

fn optional_query(query: &Vec<(String, String)>) -> Option<&Vec<(String, String)>> {
    if query.is_empty() {
        None
    } else {
        Some(query)
    }
}

fn get_file_headers(input: &GetFileInput) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    if let Some(range) = &input.range_raw {
        headers.insert(
            reqwest::header::RANGE,
            HeaderValue::from_str(range)
                .map_err(|err| Error::client(format!("invalid range header: {err}")))?,
        );
    }
    if let Some(if_match) = &input.if_match {
        headers.insert(
            reqwest::header::IF_MATCH,
            HeaderValue::from_str(if_match)
                .map_err(|err| Error::client(format!("invalid if-match header: {err}")))?,
        );
    }
    Ok(headers)
}

fn request_instance_id<'a>(
    path: &'a str,
    query: Option<&'a Vec<(String, String)>>,
) -> Option<&'a str> {
    let path_instance = path
        .strip_prefix("/v1/instances/")
        .and_then(|remaining| remaining.split('/').next())
        .filter(|instance_id| !instance_id.is_empty());
    path_instance.or_else(|| {
        query?.iter().find_map(|(key, value)| {
            key.eq_ignore_ascii_case("instanceId")
                .then_some(value.as_str())
        })
    })
}

fn apply_content_headers(
    url: &reqwest::Url,
    headers: &mut HeaderMap,
    content_type: Option<&str>,
    content_length: Option<u64>,
) -> Result<()> {
    if let Some(content_type) = content_type {
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_str(content_type)
                .map_err(|err| Error::client(format!("invalid content-type: {err}")))?,
        );
    }
    if let Some(content_length) = content_length {
        headers.insert(
            reqwest::header::CONTENT_LENGTH,
            HeaderValue::from_str(&content_length.to_string())
                .map_err(|err| Error::client(format!("invalid content-length: {err}")))?,
        );
    }
    headers.insert(
        reqwest::header::HOST,
        HeaderValue::from_str(&url_host_with_port(url))
            .map_err(|err| Error::client(format!("invalid host header: {err}")))?,
    );
    Ok(())
}

async fn throttle_body(rate_limiter: Option<&RateLimiter>, bytes: usize) {
    let Some(rate_limiter) = rate_limiter else {
        return;
    };
    let (allowed, wait) = rate_limiter.acquire(bytes);
    if !allowed {
        if let Some(wait) = wait {
            tokio::time::sleep(wait).await;
        }
    }
}

fn insert_meta_header(headers: &mut HeaderMap, key: &str, value: &str) -> Result<()> {
    let name = if key.to_ascii_lowercase().starts_with("x-ids-meta-") {
        key.to_string()
    } else {
        format!("x-ids-meta-{key}")
    };
    headers.insert(
        HeaderName::from_bytes(name.as_bytes())
            .map_err(|err| Error::client(format!("invalid metadata header name: {err}")))?,
        HeaderValue::from_str(value)
            .map_err(|err| Error::client(format!("invalid metadata header value: {err}")))?,
    );
    Ok(())
}

async fn read_limited_response_body(response: reqwest::Response) -> Result<Vec<u8>> {
    if let Some(content_length) = response.content_length() {
        if content_length > MAX_RESPONSE_BODY_SIZE as u64 {
            return Err(Error::InvalidResponse(format!(
                "response body too large: {} bytes",
                content_length
            )));
        }
    }
    let bytes = response.bytes().await?;
    if bytes.len() > MAX_RESPONSE_BODY_SIZE {
        return Err(Error::InvalidResponse(format!(
            "response body too large: {} bytes",
            bytes.len()
        )));
    }
    Ok(bytes.to_vec())
}

fn should_retry_error(err: &Error) -> bool {
    matches!(err, Error::Http(http_err) if http_err.is_timeout() || http_err.is_connect() || http_err.is_body() || http_err.is_decode())
        || matches!(err, Error::HttpBody(io_error) if matches!(io_error.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::InvalidData))
        // [Review Fix #9] JSON decoding is part of the response attempt. The
        // method-level idempotency gate prevents this from replaying POST-like
        // operations after their response has started.
        || matches!(err, Error::Json(_))
        || matches!(err, Error::Cli(CliError::TransferFailed(_)))
}

fn should_retry_request_error(err: &Error, is_idempotent: bool) -> bool {
    matches!(err, Error::Http(http_error) if http_error.is_connect())
        || (is_idempotent && should_retry_error(err))
}

fn is_request_retry_safe(method: &Method) -> bool {
    method == Method::GET
        || method == Method::HEAD
        || method == Method::PUT
        || method == Method::DELETE
        || method == Method::OPTIONS
}

fn reqwest_body_io_error(error: reqwest::Error) -> std::io::Error {
    let kind = if error.is_timeout() {
        std::io::ErrorKind::TimedOut
    } else if error.is_body() || error.is_decode() {
        std::io::ErrorKind::UnexpectedEof
    } else if error.is_connect() {
        std::io::ErrorKind::ConnectionReset
    } else {
        std::io::ErrorKind::Other
    };
    std::io::Error::new(kind, error)
}

async fn sleep_before_retry(attempt: u32) {
    // [Review Fix #2] Keep the configured u32 attempt type through the delay
    // calculation instead of introducing lossy cross-platform casts.
    tokio::time::sleep(storage_backoff_delay(attempt)).await;
}

async fn sleep_before_response_retry(attempt: u32, mut response: reqwest::Response) {
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
async fn drain_response_body_bounded(response: &mut reqwest::Response, max_bytes: usize) -> bool {
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

fn content_length_from_headers(headers: &HeaderMap) -> i64 {
    headers
        .get("x-ids-file-size")
        .or_else(|| headers.get(reqwest::header::CONTENT_LENGTH))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0)
}

fn header_str(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn header_i64(headers: &HeaderMap, name: &str) -> i64 {
    header_str(headers, name).parse::<i64>().unwrap_or(0)
}

fn header_u64(headers: &HeaderMap, name: &str) -> u64 {
    header_str(headers, name).parse::<u64>().unwrap_or(0)
}

fn metadata_headers(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(key, value)| {
            let key = key.as_str();
            if !key.starts_with("x-ids-meta-") {
                return None;
            }
            value.to_str().ok().map(|value| {
                (
                    key.trim_start_matches("x-ids-meta-").to_string(),
                    value.to_string(),
                )
            })
        })
        .collect()
}

fn user_agent() -> String {
    storage_user_agent()
}

pub(crate) fn normalize_endpoint_scheme(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if has_explicit_url_scheme(trimmed) {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

fn has_explicit_url_scheme(value: &str) -> bool {
    let Some((scheme, remainder)) = value.split_once(':') else {
        return false;
    };
    let mut characters = scheme.chars();
    let is_rfc_scheme = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        });
    if !is_rfc_scheme {
        return false;
    }
    // [Review Fix #2] Preserve explicit schemes such as `file:/` so OAuth
    // validation rejects them, while retaining bare domain/localhost ports.
    // [Review Fix #2] A DNS label does not need a dot: Kubernetes and other
    // service-discovery endpoints commonly use forms such as `resource:9000`.
    let looks_like_host_port = remainder
        .split(['/', '?', '#'])
        .next()
        .is_some_and(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()));
    !looks_like_host_port
}

fn derive_region_from_endpoint(endpoint: &str) -> Option<String> {
    let host = reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))?;
    host.strip_prefix("ids-")
        .and_then(|rest| rest.split('.').next())
        .map(ToString::to_string)
}

fn url_host_with_port(url: &reqwest::Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(host), Some(port)) => format!("{host}:{port}"),
        (Some(host), None) => host.to_string(),
        _ => String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
fn sign_request(
    method: &str,
    url: &str,
    path: &str,
    timestamp: &str,
    headers: &[(String, String)],
    payload_hash: &str,
    access_key: &str,
    secret_key: &str,
    region: &str,
    service: &str,
) -> String {
    let date = &timestamp[..8.min(timestamp.len())];
    let (canonical_request, signed_headers) =
        canonical_request(method, url, path, timestamp, headers, payload_hash);
    let canonical_hash = sha256_hex(canonical_request.as_bytes());
    let string_to_sign =
        format!("HMAC-SHA256\n{timestamp}\n{date}/{region}/{service}/request\n{canonical_hash}");
    let signing_key = derive_signing_key(secret_key, date, region, service);
    let signature = hmac_sha256_hex(&signing_key, string_to_sign.as_bytes());
    format!(
        "HMAC-SHA256 Credential={access_key}/{date}/{region}/{service}/request, SignedHeaders={signed_headers}, Signature={signature}"
    )
}

fn canonical_request(
    method: &str,
    url: &str,
    path: &str,
    timestamp: &str,
    headers: &[(String, String)],
    payload_hash: &str,
) -> (String, String) {
    let mut signed_headers = headers
        .iter()
        .filter_map(|(key, value)| {
            let lower = key.to_ascii_lowercase();
            should_sign_header(&lower).then(|| (lower, normalize_header_value(value)))
        })
        .collect::<Vec<_>>();
    if !signed_headers.iter().any(|(key, _)| key == "host") {
        signed_headers.push(("host".to_string(), extract_host(url).to_string()));
    }
    if !signed_headers.iter().any(|(key, _)| key == "x-date") {
        signed_headers.push(("x-date".to_string(), timestamp.to_string()));
    }
    signed_headers.sort_by(|left, right| left.0.cmp(&right.0));
    let signed_header_names = signed_headers
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers = signed_headers
        .iter()
        .map(|(key, value)| format!("{key}:{value}\n"))
        .collect::<String>();
    let canonical_query = canonical_query(url);
    (
        format!(
            "{method}\n{}\n{canonical_query}\n{canonical_headers}\n{signed_header_names}\n{payload_hash}",
            encode_path(path)
        ),
        signed_header_names,
    )
}

fn should_sign_header(key: &str) -> bool {
    key == "host" || key == "x-date" || key == "x-security-token" || key.starts_with("x-ids-")
}

fn normalize_header_value(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_host(url: &str) -> &str {
    let start = url.find("://").map(|position| position + 3).unwrap_or(0);
    let rest = &url[start..];
    let end = rest.find('/').unwrap_or(rest.len());
    &rest[..end]
}

fn canonical_query(url: &str) -> String {
    let Some(query) = url.split_once('?').map(|(_, query)| query) else {
        return String::new();
    };
    let query = query.split('#').next().unwrap_or(query);
    let mut pairs = query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect::<Vec<_>>();
    pairs.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", uri_encode(key, true), uri_encode(value, true)))
        .collect::<Vec<_>>()
        .join("&")
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'+' {
            output.push(b' ');
            index += 1;
        } else if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push((high << 4) | low);
                index += 3;
            } else {
                output.push(bytes[index]);
                index += 1;
            }
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&output).to_string()
}

fn encode_path(path: &str) -> String {
    if path.is_empty() {
        "/".to_string()
    } else {
        uri_encode(path, false)
    }
}

fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut output = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte == b'/' && !encode_slash {
            output.push('/');
        } else if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key size");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

fn hmac_sha256_hex(key: &[u8], message: &[u8]) -> String {
    hex::encode(hmac_sha256(key, message))
}

fn derive_signing_key(secret_key: &str, date: &str, region: &str, service: &str) -> Vec<u8> {
    let date_key = hmac_sha256(secret_key.as_bytes(), date.as_bytes());
    let region_key = hmac_sha256(&date_key, region.as_bytes());
    let service_key = hmac_sha256(&region_key, service.as_bytes());
    hmac_sha256(&service_key, b"request")
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread;

    use chrono::{Duration as ChronoDuration, SecondsFormat};
    use tos_core::agent::request_id::ServiceRequestTrace;
    use tos_core::infra::credentials::{CredentialsFile, StoredOAuthCredentials};
    use tos_core::infra::unified_credentials::UnifiedCredentialValue;

    use crate::domain::token_manager::OAuthTokenManager;

    use super::*;

    #[test]
    fn signs_expected_header_family() {
        let auth = sign_request(
            "GET",
            "https://ids-cn-beijing.volces.com/v1/instances?limit=10",
            "/v1/instances",
            "20230601T120000Z",
            &[
                ("host".to_string(), "ids-cn-beijing.volces.com".to_string()),
                ("x-date".to_string(), "20230601T120000Z".to_string()),
            ],
            &sha256_hex(b"UNSIGNED-PAYLOAD"),
            "ak",
            "sk",
            "cn-beijing",
            "tos",
        );
        assert!(auth.starts_with("HMAC-SHA256 Credential=ak/20230601/cn-beijing/tos/request"));
        assert!(auth.contains("SignedHeaders=host;x-date"));
    }

    #[test]
    fn resource_request_id_prefers_first_safe_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", HeaderValue::from_static("generic-id"));
        headers.insert(
            "x-ids-request-id",
            HeaderValue::from_str(&"x".repeat(257)).unwrap(),
        );

        assert_eq!(resource_request_id(&headers).as_deref(), Some("generic-id"));

        headers.insert("x-ids-request-id", HeaderValue::from_static("ids-id"));
        assert_eq!(resource_request_id(&headers).as_deref(), Some("ids-id"));
    }

    #[test]
    fn resource_error_prefers_safe_response_header_over_unsafe_body_id() {
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some("https://resource.example.com".to_string()),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-ids-request-id", HeaderValue::from_static("header-id"));
        let body = format!(
            r#"{{"Code":"PermissionDenied","Message":"denied","RequestId":"{}"}}"#,
            "x".repeat(257)
        );

        let error = client
            .check_status(
                403,
                &headers,
                body.as_bytes(),
                &AppliedRedactionContext::default(),
            )
            .expect_err("403 error");
        let Error::Server(error) = error else {
            panic!("expected structured service error")
        };
        assert_eq!(error.request_id.as_deref(), Some("header-id"));
    }

    #[test]
    fn oauth_user_id_required_error_recommends_valid_login_command() {
        // [Review Fix #2] Preserve the stable code while requiring login's Instance argument.
        let error = oauth_user_id_required_error();
        let message = error.to_string();

        assert!(message.contains("[oauth_user_id_required]"));
        assert!(message.contains("--owner-id"));
        assert!(message.contains("ve-adrive auth login --instance <instance_id>"));
    }

    #[test]
    fn endpoint_is_required_even_when_region_is_configured() {
        let error = resolve_endpoint_and_region(None, Some("cn-beijing".to_string())).unwrap_err();

        assert!(error.to_string().contains("ADRIVE_ENDPOINT is required"));
    }

    #[test]
    fn blank_endpoint_is_treated_as_missing() {
        let error =
            resolve_endpoint_and_region(Some("   ".to_string()), Some("cn-beijing".to_string()))
                .unwrap_err();

        assert!(error.to_string().contains("ADRIVE_ENDPOINT is required"));
    }

    #[test]
    fn endpoint_can_derive_region_from_ids_host() {
        let (endpoint, region) =
            resolve_endpoint_and_region(Some("ids-cn-shanghai.volces.com".to_string()), None)
                .unwrap();

        assert_eq!(endpoint, "https://ids-cn-shanghai.volces.com");
        assert_eq!(region, "cn-shanghai");
    }

    #[test]
    fn dotless_service_endpoint_with_port_defaults_to_https() {
        let (endpoint, region) = resolve_endpoint_and_region(
            Some("resource:9000".to_string()),
            Some("cn-beijing".to_string()),
        )
        .unwrap();

        assert_eq!(endpoint, "https://resource:9000");
        assert_eq!(region, "cn-beijing");
    }

    #[test]
    fn resource_endpoint_rejects_non_http_scheme_before_request_building() {
        let error = resolve_endpoint_and_region(
            Some("ftp://resource.example.com".to_string()),
            Some("cn-beijing".to_string()),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("ADrive resource endpoint must use HTTP or HTTPS and include a host"));
    }

    #[test]
    fn endpoint_requires_region_when_not_derivable() {
        let err =
            resolve_endpoint_and_region(Some("https://private.example.com".to_string()), None)
                .unwrap_err();

        assert!(err
            .to_string()
            .contains("ADRIVE_REGION is required when region cannot be derived"));
    }

    #[test]
    fn endpoint_requires_region_or_endpoint() {
        let err = resolve_endpoint_and_region(None, None).unwrap_err();

        assert!(err.to_string().contains("ADRIVE_ENDPOINT is required"));
    }

    #[test]
    fn encodes_special_characters_in_canonical_path() {
        let path = "/v1/instances/i/spaces/s/files/a b#c?.txt";
        let encoded_path = encode_path(path);
        let (canonical_request, _) = canonical_request(
            "GET",
            &format!("https://ids-cn-beijing.volces.com{encoded_path}"),
            path,
            "20230601T120000Z",
            &[
                ("host".to_string(), "ids-cn-beijing.volces.com".to_string()),
                ("x-date".to_string(), "20230601T120000Z".to_string()),
            ],
            &sha256_hex(b"UNSIGNED-PAYLOAD"),
        );

        assert_eq!(
            encoded_path,
            "/v1/instances/i/spaces/s/files/a%20b%23c%3F.txt"
        );
        assert!(
            canonical_request.starts_with("GET\n/v1/instances/i/spaces/s/files/a%20b%23c%3F.txt\n")
        );
    }

    #[tokio::test]
    async fn list_my_spaces_uses_sdk_path_and_pagination_query() {
        let (endpoint, requests) =
            serve_responses(vec![(200, r#"{"Spaces":[],"NextMarker":"next-2"}"#)]);
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();
        let mut input = ListMySpacesInput::new("inst-1").with_limit(10);
        input.marker = Some("next marker".to_string());

        let output = client.list_my_spaces(&input).await.unwrap();

        assert_eq!(output.next_marker, "next-2");
        let request = requests.recv().unwrap();
        assert!(request.starts_with("GET /v1/instances/inst-1/myspaces?"));
        assert!(request.contains("limit=10"));
        assert!(request.contains("marker=next+marker") || request.contains("marker=next%20marker"));
    }

    #[tokio::test]
    async fn list_my_group_spaces_preserves_root_space() {
        let (endpoint, requests) = serve_responses(vec![(
            200,
            r#"{"Spaces":[],"RootSpace":{"SpaceID":"root-space"},"NextMarker":""}"#,
        )]);
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        let output = client
            .list_my_group_spaces(&ListMyGroupSpacesInput::new("inst-1").with_limit(20))
            .await
            .unwrap();

        assert_eq!(output.root_space.unwrap().space_id, "root-space");
        let request = requests.recv().unwrap();
        assert!(request.starts_with("GET /v1/instances/inst-1/mygroupspaces?limit=20 "));
    }

    #[test]
    fn retry_status_covers_all_server_errors_and_selected_client_errors() {
        for status in 500..=599 {
            assert!(
                should_retry_storage_status(StatusCode::from_u16(status).unwrap(), true),
                "status {status} must retry"
            );
            assert!(
                should_retry_storage_status(StatusCode::from_u16(status).unwrap(), false),
                "non-idempotent status {status} must retry"
            );
        }
        assert!(should_retry_storage_status(
            StatusCode::REQUEST_TIMEOUT,
            true
        ));
        assert!(!should_retry_storage_status(
            StatusCode::REQUEST_TIMEOUT,
            false
        ));
        assert!(should_retry_storage_status(
            StatusCode::TOO_MANY_REQUESTS,
            false
        ));
        assert!(!should_retry_storage_status(StatusCode::BAD_REQUEST, true));
        assert!(!should_retry_storage_status(StatusCode::UNAUTHORIZED, true));
        assert!(!should_retry_storage_status(
            StatusCode::from_u16(499).unwrap(),
            true
        ));
    }

    #[test]
    fn response_json_decode_errors_are_retryable() {
        let decode_error = serde_json::from_str::<serde_json::Value>("{invalid")
            .expect_err("fixture must be malformed JSON");

        assert!(should_retry_error(&Error::Json(decode_error)));
    }

    #[tokio::test]
    async fn unified_signing_resolves_once_per_retry_attempt() {
        let (endpoint, requests) = serve_responses(vec![
            (500, r#"{"Code":"ServerError"}"#),
            (200, r#"{"Instances":[]}"#),
        ]);
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver_calls = Arc::clone(&calls);
        let client = Client::new_unified_for_test(
            move || {
                let generation = resolver_calls.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(UnifiedCredentialValue::new(
                    format!("unified-ak-{generation}"),
                    format!("unified-sk-{generation}"),
                    format!("unified-token-{generation}"),
                    "test-provider",
                ))
            },
            Some(endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap();

        client
            .list_instances(&ListInstancesInput::new())
            .await
            .expect("second attempt succeeds");

        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let first = requests.recv().unwrap();
        let second = requests.recv().unwrap();
        assert!(first.contains("Credential=unified-ak-1/"));
        assert!(first.contains("x-security-token: unified-token-1"));
        assert!(!first.to_ascii_lowercase().contains("authorization: bearer"));
        assert!(second.contains("Credential=unified-ak-2/"));
        assert!(second.contains("x-security-token: unified-token-2"));
        assert!(!second
            .to_ascii_lowercase()
            .contains("authorization: bearer"));
    }

    #[tokio::test]
    async fn unified_empty_session_token_is_omitted_from_headers_and_signature() {
        let (endpoint, requests) = serve_responses(vec![(200, r#"{"Instances":[]}"#)]);
        let client = Client::new_unified_for_test(
            || {
                Ok(UnifiedCredentialValue::new(
                    "unified-ak",
                    "unified-sk",
                    "",
                    "test-provider",
                ))
            },
            Some(endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        client
            .list_instances(&ListInstancesInput::new())
            .await
            .expect("request without a session token succeeds");

        let request = requests.recv().unwrap().to_ascii_lowercase();
        assert!(!request.contains("x-security-token:"));
        assert!(!request.contains("signedheaders=host;x-date;x-security-token"));
    }

    #[tokio::test]
    async fn unified_error_redacts_attempt_authorization_access_key_and_session_token() {
        const ACCESS_KEY: &str = "UNIFIED_AK_MUST_NOT_LEAK";
        const SECRET_KEY: &str = "UNIFIED_SK_MUST_NOT_LEAK";
        const SESSION_TOKEN: &str = "UNIFIED_TOKEN_MUST_NOT_LEAK";
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            let authorization = request
                .lines()
                .find_map(|line| {
                    line.strip_prefix("authorization: ")
                        .or_else(|| line.strip_prefix("Authorization: "))
                })
                .unwrap()
                .trim()
                .to_string();
            let body = serde_json::json!({
                "Code": format!("Rejected {ACCESS_KEY}"),
                "Message": format!("authorization={authorization}; token={SESSION_TOKEN}"),
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nX-Echo-Token: {SESSION_TOKEN}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let client = Client::new_unified_for_test(
            || {
                Ok(UnifiedCredentialValue::new(
                    ACCESS_KEY,
                    SECRET_KEY,
                    SESSION_TOKEN,
                    "test-provider",
                ))
            },
            Some(endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        let error = client
            .list_instances(&ListInstancesInput::new())
            .await
            .expect_err("fixture response is forbidden");
        server.join().unwrap();
        let rendered = format!("{error:?} {error}");

        assert!(!rendered.contains(ACCESS_KEY));
        assert!(!rendered.contains(SECRET_KEY));
        assert!(!rendered.contains(SESSION_TOKEN));
        assert!(!rendered.contains("HMAC-SHA256"));
        assert!(rendered.contains("***"));
    }

    #[tokio::test]
    async fn unified_provider_error_has_no_local_auth_fallback() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver_calls = Arc::clone(&calls);
        let client = Client::new_unified_for_test(
            move || {
                resolver_calls.fetch_add(1, Ordering::SeqCst);
                Err(CliError::TransferFailed(
                    "[test_unified_unavailable] unified credentials are unavailable".to_string(),
                ))
            },
            Some("http://127.0.0.1:1".to_string()),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(2),
                ..ClientOptions::default()
            },
        )
        .unwrap();

        let error = client
            .list_instances(&ListInstancesInput::new())
            .await
            .expect_err("provider failure must stop before HTTP send");

        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(matches!(
            error,
            Error::UnifiedCredential(CliError::TransferFailed(_))
        ));
        assert!(!error.to_string().contains("fallback"));
    }

    #[test]
    fn unified_client_uses_aksk_business_semantics_without_resolving_credentials() {
        let calls = Arc::new(AtomicUsize::new(0));
        let resolver_calls = Arc::clone(&calls);
        let client = Client::new_unified_for_test(
            move || {
                resolver_calls.fetch_add(1, Ordering::SeqCst);
                Ok(UnifiedCredentialValue::new(
                    "unused-ak",
                    "unused-sk",
                    "unused-token",
                    "test-provider",
                ))
            },
            Some("http://127.0.0.1:1".to_string()),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        assert_eq!(
            client.instance_listing_scope().unwrap(),
            InstanceListingScope::All
        );
        assert!(!client.uses_oauth());
        assert_eq!(client.oauth_user_id().unwrap(), None);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn structured_request_retries_when_success_body_is_truncated() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let complete_body = r#"{"Spaces":[],"NextMarker":"done"}"#;
        let expected_length = complete_body.len();
        let server = thread::spawn(move || {
            for body in ["{", complete_body] {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = read_request(&mut stream);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {expected_length}\r\nConnection: close\r\n\r\n{body}"
                )
                .unwrap();
            }
        });
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap();

        let output = client
            .list_my_spaces(&ListMySpacesInput::new("inst-1"))
            .await
            .expect("retry complete structured response");

        server.join().unwrap();
        assert_eq!(output.next_marker, "done");
    }

    #[tokio::test]
    async fn file_consumer_retries_when_response_stream_is_truncated() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let complete_body = b"complete-file";
        let expected_length = complete_body.len();
        let server = thread::spawn(move || {
            for body in [b"short".as_slice(), complete_body.as_slice()] {
                let (mut stream, _) = listener.accept().unwrap();
                let _ = read_request(&mut stream);
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {expected_length}\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                stream.write_all(body).unwrap();
            }
        });
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap();
        let input = GetFileInput::new("inst-1", "space-1", "file.bin");

        let body = client
            .get_file_with_consumer(&input, |output| Box::pin(output.read_all()))
            .await
            .expect("retry complete file stream");

        server.join().unwrap();
        assert_eq!(body, complete_body);
    }

    #[tokio::test]
    async fn post_consumer_does_not_replay_after_response_body_started() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = read_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 8\r\nConnection: close\r\n\r\nshort"
            )
            .unwrap();
        });
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap();

        let error = client
            .send_request_with_consumer(
                Method::POST,
                "/v1/instances",
                None,
                HeaderMap::new(),
                Some(b"{}".to_vec()),
                Some("application/json"),
                Some(2),
                |applied| read_limited_response_body(applied.response),
            )
            .await
            .expect_err("POST body-consumption failure must not replay the operation");

        server.join().unwrap();
        assert!(matches!(error, Error::Http(_)));
    }

    #[tokio::test]
    async fn non_idempotent_post_retries_500() {
        let (endpoint, requests) = serve_responses(vec![
            (500, r#"{"Code":"ServerError"}"#),
            (200, r#"{"InstanceId":"instance-1"}"#),
        ]);
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap();

        let response = client
            .send_request(
                Method::POST,
                "/v1/instances",
                None,
                HeaderMap::new(),
                Some(b"{}".to_vec()),
                Some("application/json"),
                Some(2),
            )
            .await
            .expect("retry rejected non-idempotent request");

        assert_eq!(response.response.status().as_u16(), 200);
        for _ in 0..2 {
            let request = requests.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(request.starts_with("POST /v1/instances "));
        }
        assert!(requests.try_recv().is_err());
    }

    #[tokio::test]
    async fn non_idempotent_post_does_not_retry_400() {
        let (endpoint, requests) = serve_responses(vec![(400, r#"{"Code":"InvalidArgs"}"#)]);
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(2),
                ..ClientOptions::default()
            },
        )
        .unwrap();

        let response = client
            .send_request(
                Method::POST,
                "/v1/instances",
                None,
                HeaderMap::new(),
                Some(b"{}".to_vec()),
                Some("application/json"),
                Some(2),
            )
            .await
            .expect("return non-retryable client error");

        assert_eq!(response.response.status().as_u16(), 400);
        let request = requests.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(request.starts_with("POST /v1/instances "));
        assert!(requests.try_recv().is_err());
    }

    #[tokio::test]
    async fn non_idempotent_post_does_not_retry_408() {
        let (endpoint, requests) = serve_responses(vec![(408, r#"{"Code":"RequestTimeout"}"#)]);
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap();

        let response = client
            .send_request(
                Method::POST,
                "/v1/instances",
                None,
                HeaderMap::new(),
                Some(b"{}".to_vec()),
                Some("application/json"),
                Some(2),
            )
            .await
            .expect("return ambiguous timeout without replaying POST");

        assert_eq!(response.response.status(), StatusCode::REQUEST_TIMEOUT);
        let request = requests.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(request.starts_with("POST /v1/instances "));
        assert!(requests.try_recv().is_err());
    }

    #[tokio::test]
    async fn idempotent_search_post_retries_500() {
        let (endpoint, requests) = serve_responses(vec![
            (500, r#"{"Code":"ServerError"}"#),
            (200, r#"{"Results":[]}"#),
        ]);
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap();
        let input = SearchFilesInput {
            instance_id: "inst-1".to_string(),
            space_id: "space-1".to_string(),
            query: "needle".to_string(),
            top_k: 10,
            ..Default::default()
        };

        let output = client
            .search_files(&input)
            .await
            .expect("read-only search POST remains retryable");

        assert!(output.results.is_empty());
        for _ in 0..2 {
            let request = requests.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(request.starts_with("POST /v1/instances/inst-1/spaces/space-1/search "));
        }
        assert!(requests.try_recv().is_err());
    }

    #[tokio::test]
    async fn resource_request_trace_records_retries_and_prefers_ids_header() {
        let (endpoint, requests) = serve_responses_with_headers(vec![
            (
                500,
                r#"{"Code":"ServerError"}"#,
                "x-request-id: retry-1\r\n",
            ),
            (
                200,
                r#"{"Results":[]}"#,
                "x-request-id: generic-success\r\nx-ids-request-id: ids-success\r\n",
            ),
        ]);
        let trace = Arc::new(ServiceRequestTrace::default());
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap()
        .with_request_trace(Arc::clone(&trace));
        let input = SearchFilesInput {
            instance_id: "inst-1".to_string(),
            space_id: "space-1".to_string(),
            query: "needle".to_string(),
            top_k: 10,
            ..Default::default()
        };

        client.search_files(&input).await.expect("retry succeeds");

        assert!(requests.recv_timeout(Duration::from_secs(1)).is_ok());
        assert!(requests.recv_timeout(Duration::from_secs(1)).is_ok());
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.request_ids, vec!["retry-1", "ids-success"]);
        assert_eq!(
            snapshot.last_successful_request_id.as_deref(),
            Some("ids-success")
        );
    }

    #[tokio::test]
    async fn terminal_transport_failure_clears_prior_response_id() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            let _ = read_request(&mut first);
            first
                .write_all(
                    b"HTTP/1.1 500 Test\r\nx-ids-request-id: retry-id\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                )
                .unwrap();
            let (mut terminal, _) = listener.accept().unwrap();
            let _ = read_request(&mut terminal);
            drop(terminal);
        });
        let trace = Arc::new(ServiceRequestTrace::default());
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some(endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap()
        .with_request_trace(Arc::clone(&trace));
        let input = SearchFilesInput {
            instance_id: "inst-1".to_string(),
            space_id: "space-1".to_string(),
            query: "needle".to_string(),
            top_k: 10,
            ..Default::default()
        };

        client
            .search_files(&input)
            .await
            .expect_err("terminal transport failure");
        server.join().unwrap();

        let snapshot = trace.snapshot();
        assert_eq!(snapshot.request_ids, vec!["retry-id"]);
        assert!(snapshot.request_attempted);
        assert!(!snapshot.terminal_response_received);
        assert_eq!(snapshot.terminal_response_request_id, None);
        assert_eq!(
            tos_core::agent::request_id::select_error_request_id(&snapshot, Some("stale-id")),
            None
        );
    }

    #[tokio::test]
    async fn oauth_request_uses_only_bearer_authentication_headers() {
        let (resource_endpoint, requests) = serve_responses(vec![(200, "{}")]);
        let (directory, manager) = oauth_manager(
            "http://127.0.0.1:8",
            "access-current",
            "refresh-current",
            "inst-1",
        );
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        let response = client
            .send_request(
                Method::GET,
                "/v1/instances/inst-1",
                None,
                HeaderMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(response.response.status().as_u16(), 200);
        let request = requests.recv().unwrap();
        assert!(request.contains("authorization: Bearer access-current"));
        assert!(!request.to_ascii_lowercase().contains("x-date:"));
        assert!(!request.to_ascii_lowercase().contains("x-security-token:"));
        assert!(!request.contains("HMAC-SHA256"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn aksk_client_can_list_all_instances() {
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some("https://resource.example.com".to_string()),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        assert_eq!(
            client.instance_listing_scope().unwrap(),
            InstanceListingScope::All
        );
    }

    #[test]
    fn oauth_client_lists_only_its_bound_instance() {
        let (directory, manager) = oauth_manager(
            "https://auth.example.com",
            "access-current",
            "refresh-current",
            "inst-1",
        );
        let client = Client::new_oauth(
            manager,
            Some("https://resource.example.com".to_string()),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        assert_eq!(
            client.instance_listing_scope().unwrap(),
            InstanceListingScope::Bound("inst-1".to_string())
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn oauth_environment_client_has_unknown_instance_binding() {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-resource-oauth-environment-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let manager = OAuthTokenManager::new_with_environment(
            directory.join("credentials.toml"),
            "default".to_string(),
            no_retry_options(),
            Some("access-current".to_string()),
            None,
        )
        .unwrap();
        let client = Client::new_oauth(
            manager,
            Some("https://resource.example.com".to_string()),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        assert_eq!(
            client.instance_listing_scope().unwrap(),
            InstanceListingScope::UnknownOAuthBinding
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn oauth_user_id_is_none_for_aksk_and_reads_selected_file_credentials() {
        let aksk_client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            None,
            Some("https://resource.example.com".to_string()),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();
        assert_eq!(aksk_client.oauth_user_id().unwrap(), None);

        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-resource-oauth-user-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let credentials_path = directory.join("credentials.toml");
        let mut credentials = CredentialsFile::default();
        credentials
            .set_adrive_oauth(
                "default",
                StoredOAuthCredentials {
                    access_token: Some("access-current".to_string()),
                    user_id: Some(" user-1 ".to_string()),
                    ..Default::default()
                },
            )
            .unwrap();
        credentials.save_to_path(&credentials_path).unwrap();
        let manager =
            OAuthTokenManager::new(credentials_path, "default".to_string(), no_retry_options())
                .unwrap();
        let oauth_client = Client::new_oauth(
            manager,
            Some("https://resource.example.com".to_string()),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        assert_eq!(
            oauth_client.oauth_user_id().unwrap().as_deref(),
            Some("user-1")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn oauth_first_401_refreshes_and_replays_exactly_once() {
        let (auth_endpoint, refresh_requests) = serve_responses_with_headers(vec![(
            200,
            r#"{"access_token":"access-new","refresh_token":"refresh-new","token_type":"Bearer","expires_in":3600,"scope":"all","instance_id":"inst-1"}"#,
            "x-request-id: oauth-refresh-id\r\n",
        )]);
        let (resource_endpoint, resource_requests) = serve_responses_with_headers(vec![
            (401, "{}", "x-ids-request-id: resource-401\r\n"),
            (200, "{}", "x-ids-request-id: resource-200\r\n"),
        ]);
        let (directory, manager) =
            oauth_manager(&auth_endpoint, "access-old", "refresh-old", "inst-1");
        let trace = Arc::new(ServiceRequestTrace::default());
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap()
        .with_request_trace(Arc::clone(&trace));

        let response = client
            .send_request(
                Method::GET,
                "/v1/instances/inst-1",
                None,
                HeaderMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(response.response.status().as_u16(), 200);
        let first = resource_requests.recv().unwrap();
        let second = resource_requests.recv().unwrap();
        assert!(first.contains("authorization: Bearer access-old"));
        assert!(second.contains("authorization: Bearer access-new"));
        assert!(refresh_requests
            .recv()
            .unwrap()
            .contains("refresh_token=refresh-old"));
        assert!(refresh_requests.try_recv().is_err());
        // [Review Fix #14] Refresh uses its token-manager-local OAuth trace;
        // only primary resource responses reach the command invocation trace.
        assert_eq!(
            trace.snapshot().request_ids,
            vec!["resource-401", "resource-200"]
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn oauth_default_space_owner_reloads_after_401_refresh() {
        // [Review Fix #5] A 401 replay must use a short-lived refreshed Token
        // without immediately attempting a second refresh.
        let (auth_endpoint, refresh_requests) = serve_responses(vec![(
            200,
            r#"{"access_token":"access-new","refresh_token":"refresh-new","token_type":"Bearer","expires_in":30,"scope":"all","instance_id":"inst-1","user_id":"user-new"}"#,
        )]);
        let (resource_endpoint, resource_requests) = serve_responses(vec![
            (401, "{}"),
            (200, r#"{"Space":{"SpaceID":"space-1"}}"#),
        ]);
        let (directory, manager) = oauth_manager_with_user(
            &auth_endpoint,
            "access-old",
            "refresh-old",
            "inst-1",
            Some("user-old"),
        );
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();
        let input = CreateSpaceInput {
            instance_id: "inst-1".to_string(),
            space_name: "space".to_string(),
            owner_type: Some("user".to_string()),
            owner_id: Some("user-old".to_string()),
            ..Default::default()
        };

        client
            .create_space_with_oauth_default_owner(&input)
            .await
            .unwrap();

        let first = resource_requests.recv().unwrap();
        let second = resource_requests.recv().unwrap();
        assert!(first.contains("Bearer access-old"));
        assert!(first.contains(r#""OwnerId":"user-old""#));
        assert!(second.contains("Bearer access-new"));
        assert!(second.contains(r#""OwnerId":"user-new""#));
        refresh_requests.recv().unwrap();
        assert!(refresh_requests.try_recv().is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn oauth_create_space_retries_500() {
        let (resource_endpoint, resource_requests) = serve_responses(vec![
            (500, r#"{"Code":"ServerError"}"#),
            (200, r#"{"Space":{"SpaceID":"space-1"}}"#),
        ]);
        let (directory, manager) = oauth_manager_with_user(
            "http://127.0.0.1:9",
            "access-current",
            "refresh-current",
            "inst-1",
            Some("user-1"),
        );
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap();
        let input = CreateSpaceInput {
            instance_id: "inst-1".to_string(),
            space_name: "space".to_string(),
            ..Default::default()
        };

        let output = client
            .create_space_with_oauth_default_owner(&input)
            .await
            .expect("retry rejected OAuth create");

        assert_eq!(output.space.space_id, "space-1");
        for _ in 0..2 {
            let request = resource_requests
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
            assert!(request.starts_with("POST /v1/instances/inst-1/spaces "));
        }
        assert!(resource_requests.try_recv().is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn oauth_create_space_does_not_retry_408() {
        let (resource_endpoint, resource_requests) =
            serve_responses(vec![(408, r#"{"Code":"RequestTimeout"}"#)]);
        let (directory, manager) = oauth_manager_with_user(
            "http://127.0.0.1:9",
            "access-current",
            "refresh-current",
            "inst-1",
            Some("user-1"),
        );
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            ClientOptions {
                max_retry_count: Some(1),
                ..ClientOptions::default()
            },
        )
        .unwrap();
        let input = CreateSpaceInput {
            instance_id: "inst-1".to_string(),
            space_name: "space".to_string(),
            ..Default::default()
        };

        let error = client
            .create_space_with_oauth_default_owner(&input)
            .await
            .expect_err("return ambiguous timeout without replaying OAuth create");

        assert!(matches!(error, Error::Server(_)));
        let request = resource_requests
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(request.starts_with("POST /v1/instances/inst-1/spaces "));
        assert!(resource_requests.try_recv().is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn oauth_default_space_owner_uses_refreshed_identity_before_first_post() {
        // [Review Fix #4] A short-lived refresh response remains usable for this request.
        let (auth_endpoint, refresh_requests) = serve_responses(vec![(
            200,
            r#"{"access_token":"access-new","refresh_token":"refresh-new","token_type":"Bearer","expires_in":30,"scope":"all","instance_id":"inst-1","user_id":"user-new"}"#,
        )]);
        let (resource_endpoint, resource_requests) =
            serve_responses(vec![(200, r#"{"Space":{"SpaceID":"space-1"}}"#)]);
        let (directory, manager) = oauth_manager_with_user(
            &auth_endpoint,
            "access-old",
            "refresh-old",
            "inst-1",
            Some("user-old"),
        );
        let credentials_path = directory.join("credentials.toml");
        let mut credentials = CredentialsFile::load_from(&credentials_path).unwrap();
        let mut stored = credentials
            .adrive_oauth("default", &credentials_path)
            .unwrap();
        stored.expires_at = Some(
            (Utc::now() - ChronoDuration::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true),
        );
        credentials.set_adrive_oauth("default", stored).unwrap();
        credentials.save_to_path(&credentials_path).unwrap();
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();
        let input = CreateSpaceInput {
            instance_id: "inst-1".to_string(),
            space_name: "space".to_string(),
            owner_type: Some("user".to_string()),
            owner_id: Some("user-old".to_string()),
            ..Default::default()
        };

        client
            .create_space_with_oauth_default_owner(&input)
            .await
            .unwrap();

        let request = resource_requests.recv().unwrap();
        assert!(request.contains("Bearer access-new"));
        assert!(request.contains(r#""OwnerId":"user-new""#));
        refresh_requests.recv().unwrap();
        assert!(refresh_requests.try_recv().is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn oauth_default_space_error_redacts_applied_and_rotated_tokens() {
        // [Review Fix #1] A concurrent login can rotate credentials after this
        // request uses Token A but before its echoed error body is parsed.
        let (directory, error, request) = rotating_oauth_space_error().await;
        assert!(request.contains("Bearer access-request-a"));
        assert_resource_error_is_redacted(&error);
        let cli_error = crate::handler::common::map_ids_error(error);
        let semantics = cli_error.agent_semantics();
        // [Review Fix #2] Resource errors must populate the public structured
        // fields instead of leaving RequestId decoration inside message text.
        assert_eq!(semantics.code, "PermissionDenied");
        assert_eq!(semantics.request_id.as_deref(), Some("req-rotate"));
        assert_eq!(semantics.message, "rejected *** while current ***");
        let envelope = resource_error_envelope(&cli_error, &semantics);
        assert_eq!(envelope["request_id"], "req-rotate");
        assert_eq!(envelope["error"]["code"], "PermissionDenied");
        assert_eq!(
            envelope["error"]["message"],
            "rejected *** while current ***"
        );
        let serialized = envelope.to_string();
        assert!(!serialized.contains("access-request-a"));
        assert!(!serialized.contains("access-current-b"));
        assert!(!serialized.contains("request_id="));
        let _ = std::fs::remove_dir_all(directory);
    }

    fn resource_error_envelope(
        error: &CliError,
        semantics: &tos_core::agent::error::AgentErrorSemantics,
    ) -> serde_json::Value {
        use tos_core::agent::envelope::{Envelope, ErrorDetail, ErrorKind};

        let detail = ErrorDetail {
            status_code: semantics.status_code,
            code: semantics.code.clone(),
            message: semantics.message.clone(),
            exit_code: error.exit_code().as_i32(),
            kind: ErrorKind::PermissionDenied,
            category: semantics.category,
            suggested_action: Some(semantics.suggested_action.clone()),
            fix_command: None,
            doctor_hint: None,
            docs_url: None,
        };
        let envelope = Envelope::<()>::failed("ve-adrive crt", detail)
            .with_request_id(semantics.request_id.clone().unwrap());
        serde_json::to_value(envelope).unwrap()
    }

    #[tokio::test]
    async fn oauth_generic_error_redacts_applied_and_rotated_tokens() {
        // [Review Fix #1] The generic JSON request path carries the same
        // applied-Token context as the default-owner request path.
        let (directory, manager) = oauth_manager(
            "http://127.0.0.1:9",
            "access-request-a",
            "refresh-current",
            "inst-1",
        );
        let credentials_path = directory.join("credentials.toml");
        let (resource_endpoint, requests, server) = serve_rotating_oauth_error(credentials_path);
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        let error = match client.get_instance(&GetInstanceInput::new("inst-1")).await {
            Err(error) => error,
            Ok(_) => panic!("rotating server must reject the generic request"),
        };
        server.join().unwrap();
        assert!(requests.recv().unwrap().contains("Bearer access-request-a"));
        assert_resource_error_is_redacted(&error);
        let _ = std::fs::remove_dir_all(directory);
    }

    async fn rotating_oauth_space_error() -> (PathBuf, Error, String) {
        let (directory, manager) = oauth_manager_with_user(
            "http://127.0.0.1:9",
            "access-request-a",
            "refresh-current",
            "inst-1",
            Some("user-1"),
        );
        let credentials_path = directory.join("credentials.toml");
        let (resource_endpoint, requests, server) = serve_rotating_oauth_error(credentials_path);
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();
        let input = CreateSpaceInput {
            instance_id: "inst-1".to_string(),
            space_name: "space".to_string(),
            ..Default::default()
        };

        let error = client
            .create_space_with_oauth_default_owner(&input)
            .await
            .unwrap_err();
        server.join().unwrap();
        (directory, error, requests.recv().unwrap())
    }

    fn assert_resource_error_is_redacted(error: &Error) {
        let display = error.to_string();
        assert!(!display.contains("access-request-a"), "display={display}");
        assert!(!display.contains("access-current-b"), "display={display}");
        assert!(display.contains("PermissionDenied"), "display={display}");
        assert!(
            display.contains("(RequestId: req-rotate)"),
            "display={display}"
        );
        let Error::Server(server_error) = error else {
            panic!("expected structured server error: {error}");
        };
        let serialized = serde_json::to_string(server_error).unwrap();
        assert!(!serialized.contains("access-request-a"));
        assert!(!serialized.contains("access-current-b"));
    }

    #[tokio::test]
    async fn oauth_403_never_attempts_refresh() {
        let (resource_endpoint, resource_requests) = serve_responses(vec![(403, "{}")]);
        let (directory, manager) = oauth_manager(
            "http://127.0.0.1:9",
            "access-current",
            "refresh-current",
            "inst-1",
        );
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        let response = client
            .send_request(
                Method::GET,
                "/v1/instances/inst-1",
                None,
                HeaderMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(response.response.status().as_u16(), 403);
        resource_requests.recv().unwrap();
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn oauth_instance_mismatch_fails_before_network_access() {
        let (directory, manager) = oauth_manager(
            "http://127.0.0.1:8",
            "access-current",
            "refresh-current",
            "inst-1",
        );
        let client = Client::new_oauth(
            manager,
            Some("http://127.0.0.1:9".to_string()),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        let result = client
            .send_request(
                Method::GET,
                "/v1/instances/inst-2/spaces",
                None,
                HeaderMap::new(),
                None,
                None,
                None,
            )
            .await;
        let error = expect_applied_error(result, "Instance mismatch must fail");

        assert!(error.to_string().contains("login_required"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn oauth_second_401_stops_without_another_refresh() {
        let (auth_endpoint, refresh_requests) = serve_responses(vec![(
            200,
            r#"{"access_token":"access-new","refresh_token":"refresh-new","token_type":"Bearer","expires_in":3600,"scope":"all","instance_id":"inst-1"}"#,
        )]);
        let (resource_endpoint, resource_requests) =
            serve_responses(vec![(401, "{}"), (401, "{}")]);
        let (directory, manager) =
            oauth_manager(&auth_endpoint, "access-old", "refresh-old", "inst-1");
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        let result = client
            .send_request(
                Method::GET,
                "/v1/instances/inst-1",
                None,
                HeaderMap::new(),
                None,
                None,
                None,
            )
            .await;
        let error = expect_applied_error(result, "a second 401 must fail");

        assert!(error.to_string().contains("login_required"));
        resource_requests.recv().unwrap();
        resource_requests.recv().unwrap();
        refresh_requests.recv().unwrap();
        assert!(refresh_requests.try_recv().is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn aksk_request_retains_hmac_headers_after_auth_refactor() {
        let (resource_endpoint, requests) = serve_responses(vec![(200, "{}")]);
        let client = Client::new(
            "access-key".to_string(),
            "secret-key".to_string(),
            Some("session-token".to_string()),
            Some(resource_endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        let response = client
            .send_request(
                Method::GET,
                "/v1/instances/inst-1",
                None,
                HeaderMap::new(),
                None,
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(response.response.status().as_u16(), 200);
        let request = requests.recv().unwrap();
        assert!(request.contains("authorization: HMAC-SHA256"));
        assert!(request.to_ascii_lowercase().contains("x-date:"));
        assert!(request
            .to_ascii_lowercase()
            .contains("x-security-token: session-token"));
    }

    #[test]
    fn oauth_client_rejects_resource_origin_equal_to_stored_auth_origin() {
        let (directory, manager) = oauth_manager(
            "https://same.example.com",
            "access-current",
            "refresh-current",
            "inst-1",
        );

        let error = Client::new_oauth(
            manager,
            Some("https://same.example.com".to_string()),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .err()
        .expect("matching origins must be rejected");

        assert!(error.to_string().contains("different origins"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn oauth_resource_error_never_exposes_the_access_token() {
        let (directory, manager) = oauth_manager(
            "https://auth.example.com",
            "access-must-not-leak",
            "refresh-current",
            "inst-1",
        );
        let client = Client::new_oauth(
            manager,
            Some("https://resource.example.com".to_string()),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();
        let body = br#"{"code":"PermissionDenied","message":"rejected access-must-not-leak"}"#;

        let error = client
            .check_status(
                403,
                &HeaderMap::new(),
                body,
                &AppliedRedactionContext::default(),
            )
            .unwrap_err();

        assert!(!error.to_string().contains("access-must-not-leak"));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[tokio::test]
    async fn concurrent_oauth_requests_share_one_401_refresh() {
        let (auth_endpoint, refresh_requests) = serve_responses(vec![(
            200,
            r#"{"access_token":"access-new","refresh_token":"refresh-new","token_type":"Bearer","expires_in":3600,"scope":"all","instance_id":"inst-1"}"#,
        )]);
        let (resource_endpoint, resource_requests, resource_server) = serve_by_bearer_generation();
        let (directory, manager) =
            oauth_manager(&auth_endpoint, "access-old", "refresh-old", "inst-1");
        let client = Client::new_oauth(
            manager,
            Some(resource_endpoint),
            Some("test-region".to_string()),
            no_retry_options(),
        )
        .unwrap();

        let first = client.send_request(
            Method::GET,
            "/v1/instances/inst-1/spaces/a",
            None,
            HeaderMap::new(),
            None,
            None,
            None,
        );
        let second = client.send_request(
            Method::GET,
            "/v1/instances/inst-1/spaces/b",
            None,
            HeaderMap::new(),
            None,
            None,
            None,
        );
        let (first, second) = tokio::join!(first, second);

        assert_eq!(first.unwrap().response.status().as_u16(), 200);
        assert_eq!(second.unwrap().response.status().as_u16(), 200);
        resource_server.join().unwrap();
        let requests = resource_requests.try_iter().collect::<Vec<_>>();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.contains("Bearer access-new"))
                .count(),
            2
        );
        refresh_requests.recv().unwrap();
        assert!(refresh_requests.try_recv().is_err());
        let _ = std::fs::remove_dir_all(directory);
    }

    fn oauth_manager(
        auth_endpoint: &str,
        access_token: &str,
        refresh_token: &str,
        instance_id: &str,
    ) -> (PathBuf, OAuthTokenManager) {
        oauth_manager_with_user(
            auth_endpoint,
            access_token,
            refresh_token,
            instance_id,
            None,
        )
    }

    fn oauth_manager_with_user(
        auth_endpoint: &str,
        access_token: &str,
        refresh_token: &str,
        instance_id: &str,
        user_id: Option<&str>,
    ) -> (PathBuf, OAuthTokenManager) {
        let directory = std::env::temp_dir().join(format!(
            "ve-adrive-resource-oauth-{}-{}",
            std::process::id(),
            ulid::Ulid::new()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let credentials_path = directory.join("credentials.toml");
        let mut credentials = CredentialsFile::default();
        credentials
            .set_adrive_oauth(
                "default",
                StoredOAuthCredentials {
                    access_token: Some(access_token.to_string()),
                    refresh_token: Some(refresh_token.to_string()),
                    expires_at: Some(
                        (Utc::now() + ChronoDuration::hours(1))
                            .to_rfc3339_opts(SecondsFormat::Secs, true),
                    ),
                    token_type: Some("Bearer".to_string()),
                    scope: vec!["all".to_string()],
                    legacy_client_id: None,
                    instance_id: Some(instance_id.to_string()),
                    user_id: user_id.map(ToString::to_string),
                    auth_endpoint: Some(auth_endpoint.to_string()),
                },
            )
            .unwrap();
        credentials.save_to_path(&credentials_path).unwrap();
        let manager =
            OAuthTokenManager::new(credentials_path, "default".to_string(), no_retry_options())
                .unwrap();
        (directory, manager)
    }

    fn no_retry_options() -> ClientOptions {
        ClientOptions {
            max_retry_count: Some(0),
            ..ClientOptions::default()
        }
    }

    fn expect_applied_error(result: Result<AppliedResponse>, message: &str) -> Error {
        match result {
            Err(error) => error,
            Ok(_) => panic!("{message}"),
        }
    }

    fn serve_responses(responses: Vec<(u16, &'static str)>) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                sender.send(read_request(&mut stream)).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (format!("http://{address}"), receiver)
    }

    fn serve_responses_with_headers(
        responses: Vec<(u16, &'static str, &'static str)>,
    ) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for (status, body, headers) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                sender.send(read_request(&mut stream)).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (format!("http://{address}"), receiver)
    }

    fn serve_rotating_oauth_error(
        credentials_path: PathBuf,
    ) -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            sender.send(read_request(&mut stream)).unwrap();
            let mut credentials = CredentialsFile::load_from(&credentials_path).unwrap();
            let mut oauth = credentials
                .adrive_oauth("default", &credentials_path)
                .unwrap();
            oauth.access_token = Some("access-current-b".to_string());
            credentials.set_adrive_oauth("default", oauth).unwrap();
            credentials.save_to_path(&credentials_path).unwrap();
            let body = r#"{"Code":"PermissionDenied","Message":"rejected access-request-a while current access-current-b","RequestId":"req-rotate"}"#;
            write!(
                stream,
                "HTTP/1.1 403 Test\r\nContent-Type: application/json\r\nX-Ids-Request-Id: req-rotate\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        (format!("http://{address}"), receiver, server)
    }

    fn serve_by_bearer_generation() -> (String, mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let mut successful_responses = 0;
            while successful_responses < 2 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_request(&mut stream);
                let is_current = request.contains("authorization: Bearer access-new");
                let status = if is_current { 200 } else { 401 };
                successful_responses += usize::from(is_current);
                sender.send(request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                )
                .unwrap();
            }
        });
        (format!("http://{address}"), receiver, server)
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                continue;
            };
            let content_length = String::from_utf8_lossy(&bytes[..header_end + 4])
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return String::from_utf8_lossy(&bytes).to_string();
            }
        }
    }
}
