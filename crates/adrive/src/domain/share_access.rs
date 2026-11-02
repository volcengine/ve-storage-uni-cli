//! Share API transport with anonymous defaults and explicit OAuth session exchange.

#[path = "share_preview.rs"]
pub mod preview;

use super::client::{normalize_endpoint_scheme, Client, ClientOptions, Error, IdsError, Result};
use super::share_types::*;
use super::share_url::safe_origin;
use super::types::{FileInfo, ListFilesOutput, ResponseInfo};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tos_core::agent::request_id::{sanitize_request_id, ServiceRequestTrace};
use tos_core::infra::client::storage_user_agent;
use tos_core::infra::config::{
    DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS, DEFAULT_HTTP_MAX_CONNECTIONS,
    DEFAULT_HTTP_MAX_RETRY_COUNT, DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS,
};
use tos_core::infra::retry::{should_retry_storage_status, storage_backoff_delay};

const MAX_SHARE_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const MAX_SHARE_RETRIES: u32 = 8;
const MAX_OAUTH_REDACTION_VALUES: usize = 64;

/// Share API client with disabled redirects and bounded responses.
/// `new` is anonymous; internal OAuth construction authenticates public metadata/token exchange.
#[derive(Clone)]
pub struct ShareClient {
    inner: Arc<ShareClientInner>,
}

#[cfg(test)]
impl ShareClient {
    pub(crate) fn endpoint(&self) -> String {
        self.inner.endpoint.origin().ascii_serialization()
    }
}

#[derive(Clone)]
struct ShareClientInner {
    endpoint: reqwest::Url,
    http: reqwest::Client,
    max_retry_count: u32,
    request_trace: Arc<ServiceRequestTrace>,
    redactions: Vec<String>,
    oauth: Option<Client>,
    oauth_instance: Option<String>,
    oauth_redactions: Arc<Mutex<Vec<String>>>,
}

/// Ephemeral receiver session. Tokens live only in memory and are redacted in headers.
#[derive(Clone)]
pub struct ShareSession {
    client: ShareClient,
    access_token: String,
    token_header: HeaderValue,
    redactions: Vec<String>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ShareRetryPolicy {
    Metadata,
    NonReplayable,
}

pub(crate) struct ShareRequest {
    pub method: Method,
    pub path: &'static str,
    pub query: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub retry_policy: ShareRetryPolicy,
    pub redactions: Vec<String>,
    pub receiver_instance: Option<String>,
}

impl ShareRequest {
    pub(crate) fn read(path: &'static str, query: Vec<(String, String)>) -> Self {
        Self {
            method: Method::GET,
            path,
            query,
            body: None,
            retry_policy: ShareRetryPolicy::Metadata,
            redactions: Vec::new(),
            receiver_instance: None,
        }
    }

    pub(crate) fn mutation<T: Serialize>(
        method: Method,
        path: &'static str,
        body: &T,
        redactions: Vec<String>,
    ) -> Result<Self> {
        let body = serde_json::to_vec(body)
            .map_err(|_| Error::Client("invalid share request JSON".into()))?;
        Ok(Self {
            method,
            path,
            query: Vec::new(),
            body: Some(body),
            retry_policy: ShareRetryPolicy::NonReplayable,
            redactions,
            receiver_instance: None,
        })
    }
}

pub(crate) struct ShareResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
    redactions: Vec<String>,
    pub(crate) oauth_user_id: Option<String>,
}

impl ShareResponse {
    /// Return the exact secrets used to sanitize this response for a bounded authenticated replay.
    pub(crate) fn redaction_values(&self) -> &[String] {
        &self.redactions
    }

    pub(crate) async fn read(response: reqwest::Response, redactions: &[String]) -> Result<Self> {
        let status = response.status();
        let headers = safe_response_headers(response.headers(), redactions);
        let body = read_bounded_body(response).await?;
        Ok(Self {
            status,
            headers,
            body,
            redactions: redactions.to_vec(),
            oauth_user_id: None,
        })
    }

    pub(crate) fn check_status(&self, redactions: &[String]) -> Result<()> {
        if self.status.is_success() {
            return Ok(());
        }
        let redactions = self
            .redactions
            .iter()
            .chain(redactions)
            .cloned()
            .collect::<Vec<_>>();
        let value: serde_json::Value = serde_json::from_slice(&self.body).unwrap_or_default();
        let value = value.get("Error").unwrap_or(&value);
        let code = value
            .get("Code")
            .or_else(|| value.get("code"))
            .and_then(|value| value.as_str());
        let code = code
            .map(|code| redact_exact(code, &redactions))
            .filter(|code| {
                !code.is_empty()
                    && code.len() <= 128
                    && code.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
                    })
            })
            .unwrap_or_else(|| self.status.as_u16().to_string());
        Err(Error::Server(IdsError {
            code,
            message: "share API request failed".into(),
            request_id: share_request_id(&self.headers, &redactions),
            status_code: Some(self.status.as_u16()),
            response_headers: Some(self.info().headers),
        }))
    }

    pub(crate) fn decode<T: DeserializeOwned>(&self, redactions: &[String]) -> Result<T> {
        self.check_status(redactions)?;
        serde_json::from_slice(&self.body)
            .map_err(|_| Error::InvalidResponse("invalid share API JSON response".into()))
    }

    pub(crate) fn info(&self) -> ResponseInfo {
        ResponseInfo {
            request_id: share_request_id(&self.headers, &[]).unwrap_or_default(),
            status_code: self.status.as_u16(),
            headers: self
                .headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value| (name.to_string(), value.to_string()))
                })
                .collect::<HashMap<_, _>>(),
        }
    }
}

impl ShareClient {
    /// Build a credential-independent client for a safe API origin and HTTP options.
    /// Returns an input/build error for unsafe origins; metadata retries are capped at eight.
    pub fn new(endpoint: String, options: ClientOptions) -> Result<Self> {
        // [Review Fix #5] Match account-client endpoint normalization for stored profiles.
        let endpoint = safe_origin(&normalize_endpoint_scheme(&endpoint))?;
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(storage_user_agent())
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
            .map_err(|error| Error::Http(error.without_url()))?;
        Ok(Self {
            inner: Arc::new(ShareClientInner {
                endpoint,
                http,
                max_retry_count: options
                    .max_retry_count
                    .unwrap_or(DEFAULT_HTTP_MAX_RETRY_COUNT)
                    .min(MAX_SHARE_RETRIES),
                request_trace: Arc::new(ServiceRequestTrace::default()),
                redactions: Vec::new(),
                oauth: None,
                oauth_instance: None,
                oauth_redactions: Arc::new(Mutex::new(Vec::new())),
            }),
        })
    }

    /// Reuse the OAuth client's trusted API origin and redirect-free HTTP transport.
    /// Returns sanitized origin errors; account auth is used only before share-session exchange.
    pub(crate) fn from_oauth(
        client: Client,
        http: reqwest::Client,
        max_retry_count: u32,
        oauth_instance: Option<String>,
    ) -> Result<Self> {
        let endpoint = safe_origin(client.endpoint())?;
        Ok(Self {
            inner: Arc::new(ShareClientInner {
                endpoint,
                http,
                max_retry_count: max_retry_count.min(MAX_SHARE_RETRIES),
                request_trace: Arc::new(ServiceRequestTrace::default()),
                redactions: Vec::new(),
                oauth: Some(client),
                oauth_instance,
                oauth_redactions: Arc::new(Mutex::new(Vec::new())),
            }),
        })
    }

    /// Return whether public receiver requests carry an explicitly selected OAuth identity.
    pub(crate) fn has_oauth(&self) -> bool {
        self.inner.oauth.is_some()
    }

    /// Return the non-secret API origin used by receiver requests.
    pub(crate) fn api_origin(&self) -> String {
        self.inner.endpoint.origin().ascii_serialization()
    }

    fn oauth_redactions(&self) -> Result<Vec<String>> {
        self.inner
            .oauth_redactions
            .lock()
            .map(|values| values.clone())
            .map_err(|_| Error::Client("share OAuth redaction state is unavailable".into()))
    }

    fn remember_oauth_redactions(&self, response: &ShareResponse) -> Result<()> {
        let mut values = self
            .inner
            .oauth_redactions
            .lock()
            .map_err(|_| Error::Client("share OAuth redaction state is unavailable".into()))?;
        for value in response.redactions.iter().filter(|value| !value.is_empty()) {
            if !values.contains(value) {
                if values.len() == MAX_OAUTH_REDACTION_VALUES {
                    values.remove(0);
                }
                values.push(value.clone());
            }
        }
        Ok(())
    }

    /// Attach the invocation's service request trace without changing transport credentials.
    pub(crate) fn with_request_trace(mut self, request_trace: Arc<ServiceRequestTrace>) -> Self {
        Arc::make_mut(&mut self.inner).request_trace = request_trace;
        self
    }

    /// Retain receiver URL secrets solely for diagnostics before public receiver requests.
    pub(crate) fn with_redactions(mut self, secrets: Vec<String>) -> Self {
        Arc::make_mut(&mut self.inner).redactions = secrets;
        self
    }

    /// Read public share metadata for one nonempty ID, using bounded metadata retries.
    /// Returns validation, HTTP, service or sanitized JSON errors; explicit OAuth construction adds account auth.
    pub async fn get_share_by_anonymous(&self, share_id: &str) -> Result<ShareLinkInfo> {
        validate_required(share_id, "ShareID")?;
        let request = ShareRequest::read(
            "/v1/share_link/get_by_anonymous",
            vec![("share_id".into(), share_id.into())],
        );
        let response = self.send(request, None).await?;
        let mut info: ShareLinkInfo = response.decode(&[])?;
        info.response_info = response.info();
        Ok(info)
    }

    /// Exchange a share ID/password for a token, without replaying an unknown submission.
    /// Returns sanitized errors for invalid input, rejected exchanges or malformed responses.
    pub async fn get_access_token(
        &self,
        input: &GetShareAccessTokenInput,
    ) -> Result<ShareAccessTokenOutput> {
        self.get_access_token_with_subject(input)
            .await
            .map(|(token, _)| token)
    }

    /// Exchange a share token and return the OAuth subject from its accepted request snapshot.
    /// Returns the same validation/service errors as `get_access_token`; anonymous exchanges have no subject.
    pub(crate) async fn get_access_token_with_subject(
        &self,
        input: &GetShareAccessTokenInput,
    ) -> Result<(ShareAccessTokenOutput, Option<String>)> {
        validate_required(&input.share_id, "ShareID")?;
        if input.expire_sec.is_some_and(|value| value <= 0) {
            return Err(Error::Client("ExpireSec must be positive".into()));
        }
        let redactions = input.share_pwd.iter().cloned().collect();
        let request = ShareRequest::mutation(
            Method::POST,
            "/v1/share_link/get_access_token",
            input,
            redactions,
        )?;
        let response = self.send(request, None).await?;
        let token: ShareAccessTokenOutput = response.decode(&[])?;
        validate_required(&token.access_token, "AccessToken")?;
        if token.expires_in <= 0 {
            return Err(Error::InvalidResponse(
                "share token ExpiresIn must be positive".into(),
            ));
        }
        Ok((token, response.oauth_user_id))
    }

    /// Create a new in-memory session from a freshly exchanged token; refresh by recreating it.
    /// Returns a sanitized input error for empty/invalid tokens or nonpositive token lifetime.
    pub fn session(&self, token: &ShareAccessTokenOutput) -> Result<ShareSession> {
        validate_required(&token.access_token, "AccessToken")?;
        if token.expires_in <= 0 {
            return Err(Error::Client(
                "share token ExpiresIn must be positive".into(),
            ));
        }
        let mut token_header = HeaderValue::from_str(&token.access_token)
            .map_err(|_| Error::Client("invalid share token header".into()))?;
        token_header.set_sensitive(true);
        let mut client = self.clone();
        // [Review Fix #ShareReceiveAuth2] A service can echo earlier OAuth credentials after account headers have been detached.
        Arc::make_mut(&mut client.inner)
            .redactions
            .extend(self.oauth_redactions()?);
        // [Review Fix #ShareReceiveAuth1] Share-file requests must not retain the account authentication path after token exchange.
        Arc::make_mut(&mut client.inner).oauth = None;
        Ok(ShareSession {
            client,
            access_token: token.access_token.clone(),
            token_header,
            redactions: Vec::new(),
        })
    }

    async fn send(
        &self,
        mut request: ShareRequest,
        session: Option<&ShareSession>,
    ) -> Result<ShareResponse> {
        // [Review Fix #ShareAnonymousSecret1] Basic-info requests and unused URL passwords need redaction before tracing response headers.
        request
            .redactions
            .extend(self.inner.redactions.iter().cloned());
        request.redactions.extend(self.oauth_redactions()?);
        if let Some(session) = session {
            request.redactions.push(session.access_token.clone());
            // [Review Fix #ShareSecret1] Passwords may be echoed by later metadata endpoints.
            request
                .redactions
                .extend(session.redactions.iter().cloned());
        }
        if session.is_none() {
            if let Some(client) = &self.inner.oauth {
                request.receiver_instance = self.inner.oauth_instance.clone();
                let response = client.send_receiver_share(request).await?;
                self.remember_oauth_redactions(&response)?;
                response.check_status(&[])?;
                return Ok(response);
            }
        }
        self.send_with_retries(&request, session).await
    }

    async fn send_with_retries(
        &self,
        request: &ShareRequest,
        session: Option<&ShareSession>,
    ) -> Result<ShareResponse> {
        for attempt in 0..=self.inner.max_retry_count {
            let result = self.send_once(&request, session).await;
            let can_retry = request.retry_policy == ShareRetryPolicy::Metadata
                && attempt < self.inner.max_retry_count;
            match result {
                Ok(response) if can_retry && should_retry_storage_status(response.status, true) => {
                }
                Ok(response) => {
                    response.check_status(&request.redactions)?;
                    return Ok(response);
                }
                Err(error) if can_retry && is_retryable_share_error(&error) => {}
                Err(error) => return Err(error),
            }
            tokio::time::sleep(storage_backoff_delay(attempt)).await;
        }
        Err(Error::InvalidResponse("share retry loop exhausted".into()))
    }

    async fn send_once(
        &self,
        request: &ShareRequest,
        session: Option<&ShareSession>,
    ) -> Result<ShareResponse> {
        let mut url = self.inner.endpoint.clone();
        url.set_path(request.path);
        if !request.query.is_empty() {
            url.query_pairs_mut().extend_pairs(request.query.iter());
        }
        let mut builder = self.inner.http.request(request.method.clone(), url);
        if let Some(session) = session {
            builder = builder.header("x-share-token", session.token_header.clone());
        }
        if let Some(body) = &request.body {
            builder = builder
                .header("Content-Type", "application/json")
                .body(body.clone());
        }
        let response = builder.send().await.map_err(|error| {
            self.inner.request_trace.record_no_response();
            Error::Http(error.without_url())
        })?;
        let request_id = share_request_id(response.headers(), &request.redactions);
        self.inner
            .request_trace
            .record_response(request_id.as_deref(), response.status().is_success());
        ShareResponse::read(response, &request.redactions).await
    }
}

impl ShareSession {
    /// List a prefix or share root using only this session's sensitive `x-share-token`.
    /// Returns scope validation, HTTP, service or sanitized decoding errors.
    pub async fn list_files(&self, input: &ShareListFilesInput) -> Result<ListFilesOutput> {
        input.validate()?;
        let response = self
            .client
            .send(
                ShareRequest::read("/v1/share_file/list", input.to_query_pairs()),
                Some(self),
            )
            .await?;
        let metadata: super::share_metadata_decode::ShareListMetadata =
            response.decode(&[self.access_token.clone()])?;
        let mut output = metadata.into_output();
        output.response_info = response.info();
        Ok(output)
    }

    /// Retain receiver secrets solely for response and request-trace redaction.
    pub(crate) fn with_redactions(mut self, secrets: Vec<String>) -> Self {
        self.redactions = secrets;
        self
    }

    /// Read metadata for a nonempty file path with bounded retries and no account auth.
    /// Returns validation, transport, service or sanitized response errors, rejecting missing or different FilePath.
    pub async fn get_file(&self, file_path: &str) -> Result<FileInfo> {
        validate_required(file_path, "file_path")?;
        let request = ShareRequest::read(
            "/v1/share_file/get",
            vec![("file_path".into(), file_path.into())],
        );
        let metadata: super::share_metadata_decode::ShareFileMetadata = self
            .client
            .send(request, Some(self))
            .await?
            .decode(&[self.access_token.clone()])?;
        let file = metadata.into_info();
        // [Review Fix #ShareFileIdentity1] An exact read must never substitute another file or a missing identity.
        if file.file_path.is_empty() || file.file_path != file_path {
            return Err(Error::InvalidResponse(
                "share file response FilePath does not match the requested path".into(),
            ));
        }
        Ok(file)
    }

    /// Issue a download URL once; this may consume quota and is never automatically replayed.
    /// Returns validation, transport, service or sanitized response errors.
    pub async fn get_file_download_url(&self, file_path: &str) -> Result<ShareDownloadUrlOutput> {
        validate_required(file_path, "file_path")?;
        let mut request = ShareRequest::read(
            "/v1/share_file/get_download_url",
            vec![("file_path".into(), file_path.into())],
        );
        request.retry_policy = ShareRetryPolicy::NonReplayable;
        self.client
            .send(request, Some(self))
            .await?
            .decode(&[self.access_token.clone()])
    }
}

pub(crate) async fn read_bounded_body(mut response: reqwest::Response) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SHARE_RESPONSE_BYTES as u64)
    {
        return Err(Error::InvalidResponse(
            "share response body exceeds size limit".into(),
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| Error::Http(error.without_url()))?
    {
        if chunk.len() > MAX_SHARE_RESPONSE_BYTES - body.len() {
            return Err(Error::InvalidResponse(
                "share response body exceeds size limit".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

pub(crate) fn is_retryable_share_error(error: &Error) -> bool {
    matches!(error, Error::Http(error) if error.is_timeout() || error.is_connect() || error.is_body() || error.is_decode())
}

pub(crate) fn share_request_id(headers: &HeaderMap, redactions: &[String]) -> Option<String> {
    ["x-ids-request-id", "x-request-id"]
        .into_iter()
        .find_map(|name| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(|value| redact_exact(value, redactions))
                .and_then(|value| sanitize_request_id(&value))
        })
}

fn safe_response_headers(headers: &HeaderMap, redactions: &[String]) -> HeaderMap {
    let mut safe = HeaderMap::new();
    for name in [
        "x-ids-request-id",
        "x-request-id",
        "content-type",
        "content-length",
        "retry-after",
    ] {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
            if let Ok(value) = HeaderValue::from_str(&redact_exact(value, redactions)) {
                safe.insert(name, value);
            }
        }
    }
    safe
}

fn redact_exact(message: &str, redactions: &[String]) -> String {
    redactions
        .iter()
        .filter(|value| !value.is_empty())
        .fold(message.into(), |safe: String, value| {
            safe.replace(value, "***")
        })
}

#[cfg(test)]
pub(crate) mod fixture {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    pub struct Fixture {
        pub endpoint: String,
        pub requests: Arc<Mutex<Vec<String>>>,
        worker: JoinHandle<()>,
    }

    impl Fixture {
        pub fn new(responses: Vec<String>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let observed = requests.clone();
            let worker = thread::spawn(move || {
                let mut deadline = Instant::now() + Duration::from_secs(5);
                while Instant::now() < deadline {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            // [Review Fix #3] Accepted macOS sockets inherit listener nonblocking mode.
                            stream.set_nonblocking(false).unwrap();
                            stream
                                .set_read_timeout(Some(Duration::from_secs(2)))
                                .unwrap();
                            let request = read_request(&mut stream);
                            let mut requests = observed.lock().unwrap();
                            let response = responses
                                .get(requests.len())
                                .cloned()
                                .unwrap_or_else(|| reply(500, "{}"));
                            requests.push(request);
                            stream.write_all(response.as_bytes()).unwrap();
                            if requests.len() >= responses.len() {
                                deadline = Instant::now() + Duration::from_millis(600);
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(error) => panic!("fixture accept failed: {error}"),
                    }
                }
            });
            Self {
                endpoint,
                requests,
                worker,
            }
        }

        pub fn finish(self) -> Vec<String> {
            self.worker.join().unwrap();
            Arc::try_unwrap(self.requests)
                .unwrap()
                .into_inner()
                .unwrap()
        }
    }

    pub fn reply(status: u16, body: &str) -> String {
        format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\nx-ids-request-id: fixture-id\r\n\r\n{body}", body.len())
    }

    /// Read a bounded fixture request, shared by deterministic authentication barriers.
    pub(crate) fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0_u8; 4096];
            let size = stream.read(&mut buffer).unwrap();
            if size == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..size]);
            assert!(bytes.len() < 1024 * 1024);
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..index]).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("content-length:")
                            .and_then(|value| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= index + 4 + length {
                    break;
                }
            }
        }
        String::from_utf8(bytes).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{reply, Fixture};
    use super::*;
    use serde_json::{json, Value};

    fn options() -> ClientOptions {
        ClientOptions {
            max_retry_count: Some(2),
            requesttimeout: Some(2),
            ..Default::default()
        }
    }

    #[test]
    fn receiver_normalizes_configured_endpoint_without_scheme() {
        assert!(ShareClient::new("api.example.test".into(), options()).is_ok());
    }

    // [Review Fix #8] Keep protocol actions and wire assertions independently reviewable.
    async fn exercise_receiver_routes(client: &ShareClient) {
        assert_eq!(
            client
                .get_share_by_anonymous("s")
                .await
                .unwrap()
                .has_password,
            Some(true)
        );
        let token = client
            .get_access_token(&GetShareAccessTokenInput {
                share_id: "s".into(),
                share_pwd: Some("pw-secret".into()),
                expire_sec: Some(60),
            })
            .await
            .unwrap();
        let session = client.session(&token).unwrap();
        let listed = session
            .list_files(&ShareListFilesInput {
                list_root: Some(true),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(listed.files.len(), 1);
        session.get_file("a.txt").await.unwrap();
        session.get_file_download_url("a.txt").await.unwrap();
    }

    fn assert_receiver_requests(requests: &[String]) {
        assert!(requests[0].starts_with("GET /v1/share_link/get_by_anonymous?share_id=s "));
        assert!(requests[1].starts_with("POST /v1/share_link/get_access_token "));
        let body: Value =
            serde_json::from_str(requests[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            body,
            json!({"ShareID":"s","SharePwd":"pw-secret","ExpireSec":60})
        );
        assert!(requests[2].contains("list_root=true"));
        assert!(requests[3].starts_with("GET /v1/share_file/get?file_path=a.txt "));
        assert!(requests[4].starts_with("GET /v1/share_file/get_download_url?file_path=a.txt "));
        for (index, request) in requests.iter().enumerate() {
            let headers = request
                .split("\r\n\r\n")
                .next()
                .unwrap()
                .to_ascii_lowercase();
            assert!(
                !headers.contains("authorization:")
                    && !headers.contains("x-date:")
                    && !headers.contains("x-security-token:")
            );
            assert_eq!(
                headers.contains("x-share-token: ephemeral-secret"),
                index >= 2
            );
        }
    }

    #[tokio::test]
    async fn receiver_uses_only_ephemeral_share_header_and_sdk_wire_paths() {
        let fixture = Fixture::new(vec![
            reply(200, "{\"HasPassword\":true}"),
            reply(
                200,
                "{\"AccessToken\":\"ephemeral-secret\",\"ExpiresIn\":60}",
            ),
            reply(
                200,
                "{\"Files\":[{\"FilePath\":\"a.txt\"}],\"Folders\":[],\"NextMarker\":\"next\"}",
            ),
            reply(200, "{\"FilePath\":\"a.txt\"}"),
            reply(200, "{\"DownloadURL\":\"https://files.test/a?sig=x\"}"),
        ]);
        let client = ShareClient::new(fixture.endpoint.clone(), options()).unwrap();
        exercise_receiver_routes(&client).await;
        assert_receiver_requests(&fixture.finish());
    }

    #[tokio::test]
    async fn download_url_and_token_exchange_never_replay_on_5xx() {
        for is_exchange in [false, true] {
            let fixture = Fixture::new(vec![reply(500, "{}")]);
            let client = ShareClient::new(fixture.endpoint.clone(), options()).unwrap();
            let result = if is_exchange {
                client
                    .get_access_token(&GetShareAccessTokenInput {
                        share_id: "s".into(),
                        ..Default::default()
                    })
                    .await
                    .map(|_| ())
            } else {
                client
                    .session(&ShareAccessTokenOutput {
                        access_token: "t".into(),
                        expires_in: 60,
                    })
                    .unwrap()
                    .get_file_download_url("a")
                    .await
                    .map(|_| ())
            };
            assert!(result.is_err());
            assert_eq!(fixture.finish().len(), 1);
        }
    }

    #[tokio::test]
    async fn metadata_retries_boundedly_and_records_service_ids() {
        let fixture = Fixture::new(vec![reply(503, "{}"), reply(200, "{\"ShareID\":\"s\"}")]);
        let trace = Arc::new(ServiceRequestTrace::default());
        let client = ShareClient::new(fixture.endpoint.clone(), options())
            .unwrap()
            .with_request_trace(trace.clone());
        client.get_share_by_anonymous("s").await.unwrap();
        assert_eq!(trace.snapshot().response_count, 2);
        assert_eq!(
            trace.snapshot().terminal_response_request_id.as_deref(),
            Some("fixture-id")
        );
        assert_eq!(fixture.finish().len(), 2);
    }

    #[tokio::test]
    async fn receiver_does_not_follow_redirects_or_echo_secrets() {
        let fixture = Fixture::new(vec!["HTTP/1.1 302 Redirect\r\nLocation: http://127.0.0.1:1/token-secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()]);
        let client = ShareClient::new(fixture.endpoint.clone(), options()).unwrap();
        let session = client
            .session(&ShareAccessTokenOutput {
                access_token: "token-secret".into(),
                expires_in: 60,
            })
            .unwrap();
        let error = session.get_file("a").await.unwrap_err();
        assert!(!format!("{error:?} {error}").contains("token-secret"));
        assert_eq!(fixture.finish().len(), 1);
    }

    #[tokio::test]
    async fn server_and_json_errors_hide_password_token_and_urls() {
        let responses = vec![
            reply(403, "{\"Code\":\"pw-secret\",\"Message\":\"pw-secret\"}"),
            reply(403, "{\"Message\":\"token-secret\"}"),
            reply(200, "{\"DownloadURL\":{\"token-secret\":1}}"),
        ];
        let fixture = Fixture::new(responses);
        let client = ShareClient::new(fixture.endpoint.clone(), options()).unwrap();
        let error = client
            .get_access_token(&GetShareAccessTokenInput {
                share_id: "s".into(),
                share_pwd: Some("pw-secret".into()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("pw-secret"));
        let session = client
            .session(&ShareAccessTokenOutput {
                access_token: "token-secret".into(),
                expires_in: 60,
            })
            .unwrap();
        let error = session.get_file_download_url("a").await.unwrap_err();
        assert!(!format!("{error:?} {error}").contains("token-secret"));
        let error = session.get_file_download_url("a").await.unwrap_err();
        assert!(!format!("{error:?} {error}").contains("token-secret"));
        assert_eq!(fixture.finish().len(), 3);
    }
}

#[cfg(test)]
mod receiver_secret_tests {
    use super::fixture::{reply, Fixture};
    use super::*;
    #[tokio::test]
    async fn subsequent_metadata_redacts_password_from_code_and_trace() {
        let fixture = Fixture::new(vec![reply(
            403,
            r#"{"Code":"password-secret","Message":"password-secret"}"#,
        )
        .replace("fixture-id", "password-secret")]);
        let trace = Arc::new(ServiceRequestTrace::default());
        let client = ShareClient::new(
            fixture.endpoint.clone(),
            ClientOptions {
                max_retry_count: Some(0),
                ..Default::default()
            },
        )
        .unwrap()
        .with_request_trace(trace.clone());
        let session = client
            .session(&ShareAccessTokenOutput {
                access_token: "token-secret".into(),
                expires_in: 60,
            })
            .unwrap()
            .with_redactions(vec!["password-secret".into()]);
        let error = session.get_file("a.txt").await.unwrap_err();
        assert!(!error.to_string().contains("password-secret"));
        assert!(!format!("{:?}", trace.snapshot()).contains("password-secret"));
        fixture.finish();
    }
}
