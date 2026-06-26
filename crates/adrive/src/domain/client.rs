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
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::StreamExt;
use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tos_core::infra::client::storage_user_agent;
use tos_core::infra::config::{
    DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS, DEFAULT_HTTP_MAX_CONNECTIONS,
    DEFAULT_HTTP_MAX_RETRY_COUNT, DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS,
};

use super::rate_limiter::RateLimiter;
use super::types::*;

type HmacSha256 = Hmac<Sha256>;
pub type Result<T> = std::result::Result<T, Error>;

const MAX_RESPONSE_BODY_SIZE: usize = 50 * 1024 * 1024;
const RETRY_BASE_DELAY: Duration = Duration::from_millis(200);

#[derive(Debug)]
pub enum Error {
    Http(reqwest::Error),
    HttpBody(std::io::Error),
    Json(serde_json::Error),
    Server(IdsError),
    Client(String),
    InvalidResponse(String),
}

impl Error {
    fn client(message: impl Into<String>) -> Self {
        Self::Client(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(err) => write!(formatter, "http error: {err}"),
            Self::HttpBody(err) => write!(formatter, "body io error: {err}"),
            Self::Json(err) => write!(formatter, "json error: {err}"),
            Self::Server(err) => write!(formatter, "ids server error: {err}"),
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
            write!(formatter, " (request_id={request_id})")?;
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

struct ClientInner {
    access_key: String,
    secret_key: String,
    security_token: Option<String>,
    endpoint: String,
    region: String,
    http: reqwest::Client,
    max_retry_count: u32,
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
        let (endpoint, region) = resolve_endpoint_and_region(endpoint, region)?;
        let http = reqwest::Client::builder()
            .user_agent(user_agent())
            .tcp_nodelay(true)
            .connect_timeout(Duration::from_secs(
                options
                    .connecttimeout
                    .unwrap_or(DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS),
            ))
            .timeout(Duration::from_secs(
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
                access_key,
                secret_key,
                security_token,
                endpoint,
                region,
                http,
                max_retry_count,
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

        let response = self
            .send_signed(
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
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        self.check_status(status, &headers, &[])?;
        let response_info = Self::build_response_info(status, &headers);
        let stream = response.bytes_stream().map(|chunk| {
            chunk
                .map(|bytes| bytes.to_vec())
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
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
        let response = self
            .send_signed(
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
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        self.check_status(status, &headers, &[])?;
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
        self.do_json(
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
        let request_id = headers
            .get("x-ids-request-id")
            .or_else(|| headers.get("x-request-id"))
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
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
        let body_bytes = match body {
            Some(body) => Some(serde_json::to_vec(body)?),
            None => None,
        };
        // [Review Fix #3] Keep no-body GET/DELETE requests header-compatible with the SDK boundary.
        let content_type = body_bytes.as_ref().map(|_| "application/json");
        let response = self
            .send_signed(method, path, query, headers, body_bytes, content_type, None)
            .await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = read_limited_response_body(response).await?;
        self.check_status(status, &headers, &body)?;
        if body.is_empty() {
            return Err(Error::InvalidResponse("empty response body".to_string()));
        }
        let mut output = serde_json::from_slice::<Resp>(&body)?;
        output.set_response_info(Self::build_response_info(status, &headers));
        Ok(output)
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
        let response = self
            .send_signed(
                method,
                path,
                query,
                headers,
                Some(body),
                content_type,
                content_length,
            )
            .await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = read_limited_response_body(response).await?;
        self.check_status(status, &headers, &body)?;
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
        let response = self
            .send_signed(method, path, query, headers, None, None, None)
            .await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = if status >= 400 {
            read_limited_response_body(response).await?
        } else {
            Vec::new()
        };
        self.check_status(status, &headers, &body)?;
        Ok(Self::build_response_info(status, &headers))
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_signed(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        headers: HeaderMap,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
        content_length: Option<u64>,
    ) -> Result<reqwest::Response> {
        for attempt in 0..=self.inner.max_retry_count {
            let result = self
                .send_signed_once(
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
                Ok(response)
                    if should_retry_status(response.status().as_u16())
                        && attempt < self.inner.max_retry_count =>
                {
                    // [Review Fix #4] Preserve SDK-like retry behavior for transient IDS failures.
                    sleep_before_retry(attempt as usize).await;
                }
                Ok(response) => return Ok(response),
                Err(err) if should_retry_error(&err) && attempt < self.inner.max_retry_count => {
                    // [Review Fix #4] Preserve SDK-like retry behavior for transient transport failures.
                    sleep_before_retry(attempt as usize).await;
                }
                Err(err) => return Err(err),
            }
        }
        Err(Error::InvalidResponse("retry loop exhausted".to_string()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_signed_once(
        &self,
        method: Method,
        path: &str,
        query: Option<&Vec<(String, String)>>,
        mut headers: HeaderMap,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
        content_length: Option<u64>,
    ) -> Result<reqwest::Response> {
        // [Review Fix #2] Build the URL from an encoded path so spaces, '#', and '?' stay in the file key.
        let encoded_path = encode_path(path);
        let mut url = reqwest::Url::parse(&format!("{}{}", self.inner.endpoint, encoded_path))
            .map_err(|err| Error::client(format!("invalid url: {err}")))?;
        if let Some(query) = query {
            let mut pairs = url.query_pairs_mut();
            for (key, value) in query {
                pairs.append_pair(key, value);
            }
        }

        let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
        headers.insert(
            HeaderName::from_static("x-date"),
            HeaderValue::from_str(&timestamp)
                .map_err(|err| Error::client(format!("invalid x-date header: {err}")))?,
        );
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
        if let Some(security_token) = &self.inner.security_token {
            headers.insert(
                HeaderName::from_static("x-security-token"),
                HeaderValue::from_str(security_token)
                    .map_err(|err| Error::client(format!("invalid security token: {err}")))?,
            );
        }
        let host = url_host_with_port(&url);
        headers.insert(
            reqwest::header::HOST,
            HeaderValue::from_str(&host)
                .map_err(|err| Error::client(format!("invalid host header: {err}")))?,
        );

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
            &self.inner.access_key,
            &self.inner.secret_key,
            &self.inner.region,
            "tos",
        );
        headers.insert(
            reqwest::header::AUTHORIZATION,
            HeaderValue::from_str(&authorization)
                .map_err(|err| Error::client(format!("invalid authorization: {err}")))?,
        );

        let mut request = self.inner.http.request(method, url).headers(headers);
        if let Some(body) = body {
            request = request.body(body);
        }
        request.send().await.map_err(Error::Http)
    }

    fn check_status(&self, status: u16, headers: &HeaderMap, body: &[u8]) -> Result<()> {
        if status < 400 {
            return Ok(());
        }

        let request_id = headers
            .get("x-ids-request-id")
            .or_else(|| headers.get("x-request-id"))
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let response_headers = headers
            .iter()
            .filter_map(|(key, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (key.to_string(), value.to_string()))
            })
            .collect::<HashMap<_, _>>();

        if let Ok(envelope) = serde_json::from_slice::<ErrorEnvelope>(body) {
            if let Some(mut ids_error) = envelope.error {
                ids_error.status_code = Some(status);
                if ids_error.request_id.is_none() {
                    ids_error.request_id = request_id;
                }
                ids_error.response_headers = Some(response_headers);
                return Err(Error::Server(ids_error));
            }
        }

        if let Ok(mut ids_error) = serde_json::from_slice::<IdsError>(body) {
            ids_error.status_code = Some(status);
            if ids_error.request_id.is_none() {
                ids_error.request_id = request_id;
            }
            ids_error.response_headers = Some(response_headers);
            return Err(Error::Server(ids_error));
        }

        Err(Error::Server(IdsError {
            code: status.to_string(),
            message: String::from_utf8_lossy(body).to_string(),
            request_id,
            status_code: Some(status),
            response_headers: Some(response_headers),
        }))
    }
}

pub(crate) fn resolve_endpoint_and_region(
    endpoint: Option<String>,
    region: Option<String>,
) -> Result<(String, String)> {
    let endpoint = endpoint.map(|value| normalize_endpoint_scheme(&value));
    let region = region
        .or_else(|| endpoint.as_deref().and_then(derive_region_from_endpoint))
        .ok_or_else(|| {
            Error::client(
                "ADRIVE_REGION is required when ADRIVE_ENDPOINT is not configured or region cannot be derived from it",
            )
        })?;
    let endpoint = endpoint.unwrap_or_else(|| build_ids_endpoint(&region));
    Ok((endpoint, region))
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

fn should_retry_status(status: u16) -> bool {
    matches!(status, 408 | 429 | 500 | 502 | 503 | 504)
}

fn should_retry_error(err: &Error) -> bool {
    matches!(err, Error::Http(http_err) if http_err.is_timeout() || http_err.is_connect())
}

async fn sleep_before_retry(attempt: usize) {
    let factor = 1_u32.checked_shl(attempt as u32).unwrap_or(u32::MAX);
    tokio::time::sleep(RETRY_BASE_DELAY * factor).await;
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

fn normalize_endpoint_scheme(endpoint: &str) -> String {
    let trimmed = endpoint.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

fn derive_region_from_endpoint(endpoint: &str) -> Option<String> {
    let host = reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(ToString::to_string))?;
    host.strip_prefix("ids-")
        .and_then(|rest| rest.split('.').next())
        .map(ToString::to_string)
}

fn build_ids_endpoint(region: &str) -> String {
    format!("https://ids-{region}.volces.com")
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
    fn endpoint_defaults_to_region_scoped_ids_host() {
        let (endpoint, region) =
            resolve_endpoint_and_region(None, Some("cn-beijing".to_string())).unwrap();

        assert_eq!(endpoint, "https://ids-cn-beijing.volces.com");
        assert_eq!(region, "cn-beijing");
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
    fn endpoint_requires_region_when_not_derivable() {
        let err =
            resolve_endpoint_and_region(Some("https://private.example.com".to_string()), None)
                .unwrap_err();

        assert!(err
            .to_string()
            .contains("ADRIVE_REGION is required when ADRIVE_ENDPOINT"));
    }

    #[test]
    fn endpoint_requires_region_or_endpoint() {
        let err = resolve_endpoint_and_region(None, None).unwrap_err();

        assert!(err
            .to_string()
            .contains("ADRIVE_REGION is required when ADRIVE_ENDPOINT"));
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
}
