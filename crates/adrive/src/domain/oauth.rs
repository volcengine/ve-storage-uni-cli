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

//! OAuth protocol client for the IDS Authorization Server.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use tos_core::agent::request_id::{sanitize_request_id, ServiceRequestTrace};
use tos_core::infra::client::storage_user_agent;
use tos_core::infra::config::{
    DEFAULT_HTTP_CONNECT_TIMEOUT_SECONDS, DEFAULT_HTTP_MAX_CONNECTIONS,
    DEFAULT_HTTP_REQUEST_TIMEOUT_SECONDS,
};

use super::client::{normalize_endpoint_scheme, ClientOptions};

const MAX_OAUTH_RESPONSE_BODY_SIZE: usize = 1024 * 1024;
const DEVICE_AUTHORIZATION_PATH: &str = "v1/oauth/device_authorization";
const TOKEN_PATH: &str = "v1/oauth/token";

/// One Device Authorization request. Values are encoded as an OAuth form.
#[derive(Serialize)]
pub struct DeviceAuthorizationRequest {
    /// Public Native Client identifier.
    pub client_id: String,
    /// IDS Instance receiving the authorization.
    pub instance_id: String,
    /// Human-readable device name displayed during consent.
    pub device_name: String,
    /// Requested scope shortcut; the CLI currently sends `all`.
    pub scope: String,
}

/// Successful Device Authorization response.
#[derive(Deserialize)]
pub struct DeviceAuthorizationResponse {
    /// High-entropy back-channel credential used only by the polling process.
    pub device_code: String,
    /// Human-readable code displayed for consent confirmation.
    pub user_code: String,
    /// Manual browser verification URI.
    pub verification_uri: String,
    /// Opaque complete URI used to render the QR code.
    pub verification_uri_complete: String,
    /// Remaining Device Grant lifetime in seconds.
    pub expires_in: u64,
    /// Minimum polling interval in seconds.
    pub interval: u64,
}

/// Successful Token Endpoint response for Device Code or Refresh grants.
#[derive(Deserialize)]
pub struct TokenResponse {
    /// Bearer Access Token. This type intentionally does not implement `Debug`.
    pub access_token: String,
    /// Refresh Token returned by the server; it may equal the previous value.
    pub refresh_token: String,
    /// Expected to be `Bearer`, compared case-insensitively by the caller.
    pub token_type: String,
    /// Access Token lifetime in seconds.
    pub expires_in: u64,
    /// Space-delimited resolved Scope returned by IDS.
    #[serde(default)]
    pub scope: String,
    /// Optional OAuth subject persisted in profile credentials as identity metadata.
    ///
    /// Login and refresh responses may supply this value; it can default OAuth
    /// user-owned Space creation without deriving identity from the Token itself.
    #[serde(default)]
    pub user_id: Option<String>,
    /// IDS Instance bound to the returned Token.
    #[serde(default)]
    pub instance_id: String,
}

/// Structured OAuth failure without the potentially sensitive server description.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthFailure {
    /// Stable OAuth error code.
    pub code: String,
    /// Request identifier safe for diagnostics.
    pub request_id: Option<String>,
    /// HTTP response status.
    pub status: u16,
    /// Server-directed polling interval, when present.
    pub interval: Option<u64>,
    /// Retry delay from a legal integer `Retry-After` header.
    pub retry_after_seconds: Option<u64>,
}

impl fmt::Display for OAuthFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OAuth error {} (status={})",
            self.code, self.status
        )?;
        if let Some(request_id) = &self.request_id {
            write!(formatter, " (request_id={request_id})")?;
        }
        Ok(())
    }
}

/// OAuth transport, protocol, or validation error.
#[derive(Debug)]
pub enum OAuthClientError {
    /// Request transport failed.
    Http(reqwest::Error),
    /// Response body read failed or exceeded the safe bound.
    Response(String),
    /// Authorization Server returned an OAuth error.
    OAuth(OAuthFailure),
}

impl fmt::Display for OAuthClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(error) => write!(formatter, "OAuth HTTP request failed: {error}"),
            Self::Response(message) => write!(formatter, "invalid OAuth response: {message}"),
            Self::OAuth(failure) => failure.fmt(formatter),
        }
    }
}

impl std::error::Error for OAuthClientError {}

/// Stable non-secret classification of a Device Token polling result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceTokenOutcomeKind {
    /// A complete Token pair was returned.
    Success,
    /// User authorization has not completed.
    Pending,
    /// Polling must permanently slow down.
    SlowDown,
    /// User denied authorization.
    Denied,
    /// Device Grant expired or was consumed.
    Expired,
    /// Server reported a retryable temporary failure.
    Temporary,
    /// Any other terminal OAuth failure.
    Terminal,
}

/// Result of one Device Code Token request.
pub enum DeviceTokenOutcome {
    /// Token issuance succeeded.
    Success(TokenResponse),
    /// Authorization remains pending.
    Pending(OAuthFailure),
    /// Client must use the supplied/new minimum interval.
    SlowDown(OAuthFailure),
    /// User denied the grant.
    Denied(OAuthFailure),
    /// Grant expired or was consumed.
    Expired(OAuthFailure),
    /// Retryable temporary server condition.
    Temporary(OAuthFailure),
    /// Non-retryable OAuth failure.
    Terminal(OAuthFailure),
}

impl DeviceTokenOutcome {
    /// Return a stable non-secret result classification.
    pub fn kind(&self) -> DeviceTokenOutcomeKind {
        match self {
            Self::Success(_) => DeviceTokenOutcomeKind::Success,
            Self::Pending(_) => DeviceTokenOutcomeKind::Pending,
            Self::SlowDown(_) => DeviceTokenOutcomeKind::SlowDown,
            Self::Denied(_) => DeviceTokenOutcomeKind::Denied,
            Self::Expired(_) => DeviceTokenOutcomeKind::Expired,
            Self::Temporary(_) => DeviceTokenOutcomeKind::Temporary,
            Self::Terminal(_) => DeviceTokenOutcomeKind::Terminal,
        }
    }

    /// Return the server interval carried by a `slow_down` response.
    pub fn interval(&self) -> Option<u64> {
        match self {
            Self::SlowDown(failure) => failure.interval,
            _ => None,
        }
    }
}

impl fmt::Display for DeviceTokenOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Success(_) => formatter.write_str("OAuth token issued"),
            Self::Pending(failure)
            | Self::SlowDown(failure)
            | Self::Denied(failure)
            | Self::Expired(failure)
            | Self::Temporary(failure)
            | Self::Terminal(failure) => failure.fmt(formatter),
        }
    }
}

/// IDS OAuth Authorization Server client.
#[derive(Clone)]
pub struct OAuthClient {
    endpoint: reqwest::Url,
    http: reqwest::Client,
    request_trace: Arc<ServiceRequestTrace>,
}

impl OAuthClient {
    /// Build an OAuth client after validating and normalizing its base URL.
    pub fn new(endpoint: String, options: ClientOptions) -> Result<Self, OAuthClientError> {
        Self::new_with_request_trace(endpoint, options, Arc::new(ServiceRequestTrace::default()))
    }

    /// Build an OAuth client that records response IDs into an invocation trace.
    pub fn new_with_request_trace(
        endpoint: String,
        options: ClientOptions,
        request_trace: Arc<ServiceRequestTrace>,
    ) -> Result<Self, OAuthClientError> {
        let endpoint = normalize_auth_endpoint(&endpoint)?;
        let http = reqwest::Client::builder()
            .user_agent(storage_user_agent())
            // [Review Fix #2] Never forward Device Codes or Refresh Tokens to
            // a redirect target selected by an Authorization Server response.
            .redirect(reqwest::redirect::Policy::none())
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
            .map_err(OAuthClientError::Http)?;
        Ok(Self {
            endpoint,
            http,
            request_trace,
        })
    }

    /// Return the normalized Authorization Server base URL.
    pub fn endpoint(&self) -> &reqwest::Url {
        &self.endpoint
    }

    /// Create a Device Grant using a form-encoded public-client request.
    pub async fn create_device_authorization(
        &self,
        request: &DeviceAuthorizationRequest,
    ) -> Result<DeviceAuthorizationResponse, OAuthClientError> {
        let response_result = self
            .http
            .post(self.url(DEVICE_AUTHORIZATION_PATH)?)
            .form(request)
            .send()
            .await;
        let response = self.finish_request(response_result)?;
        self.record_request_id(&response);
        parse_success_or_oauth_error(response).await
    }

    /// Perform one Device Code Token request; this method never loops.
    pub async fn poll_device_token(
        &self,
        device_code: &str,
        client_id: &str,
    ) -> Result<DeviceTokenOutcome, OAuthClientError> {
        let response_result = self
            .http
            .post(self.url(TOKEN_PATH)?)
            .form(&DeviceTokenRequest {
                grant_type: "urn:ietf:params:oauth:grant-type:device_code",
                device_code,
                client_id,
            })
            .send()
            .await;
        let response = self.finish_request(response_result)?;
        self.record_request_id(&response);
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = read_bounded_body(response).await?;
        if (200..300).contains(&status) {
            let token = deserialize_response(&body)?;
            return Ok(DeviceTokenOutcome::Success(token));
        }
        let failure = parse_oauth_failure(status, &headers, &body);
        Ok(match failure.code.as_str() {
            "authorization_pending" => DeviceTokenOutcome::Pending(failure),
            "slow_down" => DeviceTokenOutcome::SlowDown(failure),
            "access_denied" => DeviceTokenOutcome::Denied(failure),
            "expired_token" => DeviceTokenOutcome::Expired(failure),
            "temporarily_unavailable" | "server_error" => DeviceTokenOutcome::Temporary(failure),
            _ => DeviceTokenOutcome::Terminal(failure),
        })
    }

    /// Exchange one Refresh Token for the next complete Token pair.
    pub async fn refresh_token(
        &self,
        refresh_token: &str,
        client_id: &str,
    ) -> Result<TokenResponse, OAuthClientError> {
        let response_result = self
            .http
            .post(self.url(TOKEN_PATH)?)
            .form(&RefreshTokenRequest {
                grant_type: "refresh_token",
                refresh_token,
                client_id,
            })
            .send()
            .await;
        let response = self.finish_request(response_result)?;
        self.record_request_id(&response);
        parse_success_or_oauth_error(response).await
    }

    fn record_request_id(&self, response: &reqwest::Response) {
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok());
        self.request_trace
            .record_response(request_id, response.status().is_success());
    }

    fn finish_request(
        &self,
        result: Result<reqwest::Response, reqwest::Error>,
    ) -> Result<reqwest::Response, OAuthClientError> {
        result.map_err(|error| {
            // [Review Fix #11] Device-login errors follow the same terminal
            // response rule as resource requests.
            self.request_trace.record_no_response();
            OAuthClientError::Http(error)
        })
    }

    fn url(&self, relative_path: &str) -> Result<reqwest::Url, OAuthClientError> {
        self.endpoint.join(relative_path).map_err(|_| {
            OAuthClientError::Response("failed to construct OAuth endpoint URL".to_string())
        })
    }
}

#[derive(Serialize)]
struct DeviceTokenRequest<'a> {
    grant_type: &'static str,
    device_code: &'a str,
    client_id: &'a str,
}

#[derive(Serialize)]
struct RefreshTokenRequest<'a> {
    grant_type: &'static str,
    refresh_token: &'a str,
    client_id: &'a str,
}

#[derive(Deserialize)]
struct OAuthErrorBody {
    #[serde(default)]
    error: String,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    interval: Option<u64>,
}

async fn parse_success_or_oauth_error<T: for<'de> Deserialize<'de>>(
    response: reqwest::Response,
) -> Result<T, OAuthClientError> {
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let body = read_bounded_body(response).await?;
    if !(200..300).contains(&status) {
        return Err(OAuthClientError::OAuth(parse_oauth_failure(
            status, &headers, &body,
        )));
    }
    deserialize_response(&body)
}

fn deserialize_response<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, OAuthClientError> {
    serde_json::from_slice(body)
        .map_err(|_| OAuthClientError::Response("response JSON does not match contract".into()))
}

fn parse_oauth_failure(status: u16, headers: &HeaderMap, body: &[u8]) -> OAuthFailure {
    let parsed = serde_json::from_slice::<OAuthErrorBody>(body).ok();
    let code = parsed
        .as_ref()
        .map(|error| error.error.trim())
        // [Review Fix #8] OAuth diagnostics are rendered to stderr. Only
        // accept the RFC-style token alphabet for the error identifier.
        .filter(|code| is_safe_error_code(code))
        .map(ToString::to_string)
        .unwrap_or_else(|| default_error_code(status).to_string());
    let request_id = parsed
        .as_ref()
        .and_then(|error| error.request_id.clone())
        .and_then(|request_id| sanitize_request_id(&request_id))
        // [Review Fix #2] An unsafe optional body field must not suppress the
        // independently usable Authorization Server response header.
        .or_else(|| {
            header_string(headers, "x-request-id")
                .and_then(|request_id| sanitize_request_id(&request_id))
        });
    OAuthFailure {
        code,
        request_id,
        status,
        interval: parsed.and_then(|error| error.interval),
        retry_after_seconds: header_string(headers, "retry-after")
            .and_then(|value| value.parse::<u64>().ok()),
    }
}

fn is_safe_error_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn default_error_code(status: u16) -> &'static str {
    match status {
        429 | 503 => "temporarily_unavailable",
        500..=599 => "server_error",
        _ => "invalid_response",
    }
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>, OAuthClientError> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(OAuthClientError::Http)?;
        if body.len().saturating_add(chunk.len()) > MAX_OAUTH_RESPONSE_BODY_SIZE {
            return Err(OAuthClientError::Response(
                "response body exceeded 1 MiB".to_string(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn normalize_auth_endpoint(endpoint: &str) -> Result<reqwest::Url, OAuthClientError> {
    let endpoint = normalize_endpoint_scheme(endpoint);
    let mut url = reqwest::Url::parse(&endpoint)
        .map_err(|_| OAuthClientError::Response("auth endpoint must be an absolute URL".into()))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        // [Review Fix #2] Endpoint fragments are configuration errors; silently
        // discarding them would hide a different effective Auth URL.
        return Err(OAuthClientError::Response(
            "auth endpoint cannot contain userinfo, query, or fragment".to_string(),
        ));
    }
    let is_loopback = url.host().is_some_and(|host| match host {
        url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    });
    if url.scheme() != "https" && !(url.scheme() == "http" && is_loopback) {
        return Err(OAuthClientError::Response(
            "auth endpoint must use HTTPS; HTTP is allowed only for loopback tests".to_string(),
        ));
    }
    if !url.path().ends_with('/') {
        let normalized_path = format!("{}/", url.path());
        url.set_path(&normalized_path);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{mpsc, Arc};
    use std::thread;

    use super::*;
    use crate::domain::auth::OAUTH_CLIENT_ID;
    use crate::domain::client::ClientOptions;
    use tos_core::agent::request_id::ServiceRequestTrace;

    #[test]
    fn auth_endpoint_defaults_to_https_when_scheme_is_omitted() {
        let client = OAuthClient::new("idsauth.volces.com".to_string(), ClientOptions::default())
            .expect("bare auth endpoint should default to HTTPS");

        assert_eq!(client.endpoint().as_str(), "https://idsauth.volces.com/");
    }

    #[test]
    fn auth_endpoint_rejects_non_http_scheme() {
        let error = OAuthClient::new(
            "ftp://idsauth.volces.com".to_string(),
            ClientOptions::default(),
        )
        .err()
        .expect("non-HTTP auth endpoint must be rejected");

        assert!(error.to_string().contains("must use HTTPS"));
    }

    #[test]
    fn auth_endpoint_rejects_explicit_non_http_scheme_without_authority() {
        let error = OAuthClient::new("file:/tmp/auth".to_string(), ClientOptions::default())
            .err()
            .expect("explicit file scheme must be rejected");

        assert!(error.to_string().contains("must use HTTPS"));
    }

    #[test]
    fn auth_endpoint_rejects_fragment_instead_of_silently_discarding_it() {
        let error = OAuthClient::new(
            "https://idsauth.volces.com#unexpected".to_string(),
            ClientOptions::default(),
        )
        .err()
        .expect("fragment must be rejected");

        assert!(error.to_string().contains("fragment"));
    }

    #[test]
    fn oauth_failure_rejects_unsafe_diagnostic_fields() {
        let failure = parse_oauth_failure(
            400,
            &HeaderMap::new(),
            br#"{"error":"invalid_grant\u001b[31m","request_id":"req\u001b[31m"}"#,
        );

        assert_eq!(failure.code, "invalid_response");
        assert_eq!(failure.request_id, None);
    }

    #[test]
    fn oauth_failure_uses_safe_header_when_body_request_id_is_unsafe() {
        let mut headers = HeaderMap::new();
        headers.insert("x-request-id", "header-id".parse().unwrap());
        let failure = parse_oauth_failure(
            400,
            &headers,
            br#"{"error":"invalid_grant","request_id":"req\u001b[31m"}"#,
        );

        assert_eq!(failure.request_id.as_deref(), Some("header-id"));
    }

    #[tokio::test]
    async fn refresh_does_not_follow_cross_origin_redirects() {
        let client = OAuthClient::new(serve_redirect_once(), ClientOptions::default()).unwrap();

        let error = client
            .refresh_token("refresh-must-not-leak", "client-id")
            .await
            .err()
            .expect("redirect must be terminal");

        assert!(matches!(
            error,
            OAuthClientError::OAuth(OAuthFailure { status: 307, .. })
        ));
    }

    fn serve_once(status: u16, response_body: &str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind OAuth test listener");
        let address = listener.local_addr().expect("OAuth test listener address");
        let body = response_body.to_string();
        let (request_sender, request_receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept OAuth request");
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let count = stream.read(&mut buffer).expect("read OAuth request");
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
                let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&bytes[..header_end + 4]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                if bytes.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            request_sender
                .send(String::from_utf8_lossy(&bytes).to_string())
                .expect("send captured OAuth request");
            let reason = if status == 200 { "OK" } else { "Bad Request" };
            write!(
                stream,
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("write OAuth response");
        });
        (format!("http://{address}"), request_receiver)
    }

    fn serve_redirect_once() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind redirect test listener");
        let address = listener
            .local_addr()
            .expect("redirect test listener address");
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept redirect request");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).expect("read redirect request");
            write!(
                stream,
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:9/token-steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .expect("write redirect response");
        });
        format!("http://{address}")
    }

    fn serve_oauth_responses(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind OAuth test listener");
        let address = listener.local_addr().expect("OAuth test listener address");
        let (request_sender, request_receiver) = mpsc::channel();
        thread::spawn(move || {
            for (request_id, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept OAuth request");
                let mut request = [0_u8; 4096];
                let count = stream.read(&mut request).expect("read OAuth request");
                request_sender
                    .send(String::from_utf8_lossy(&request[..count]).to_string())
                    .expect("send captured OAuth request");
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nx-request-id: {request_id}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .expect("write OAuth response");
            }
        });
        (format!("http://{address}"), request_receiver)
    }

    #[tokio::test]
    async fn device_authorization_uses_exact_form_and_no_client_secret() {
        let (endpoint, captured) = serve_once(
            200,
            r#"{"device_code":"device-code","user_code":"USER-CODE","verification_uri":"https://idsauth.volces.com/oauth/device","verification_uri_complete":"https://idsauth.volces.com/oauth/device#user_code=USER-CODE","expires_in":600,"interval":5}"#,
        );
        let client = OAuthClient::new(endpoint, ClientOptions::default()).unwrap();

        let response = client
            .create_device_authorization(&DeviceAuthorizationRequest {
                client_id: OAUTH_CLIENT_ID.to_string(),
                instance_id: "inst-1".to_string(),
                device_name: "test-device".to_string(),
                scope: "all".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(response.device_code, "device-code");
        assert_eq!(response.interval, 5);
        let request = captured.recv().unwrap();
        assert!(request.starts_with("POST /v1/oauth/device_authorization "));
        assert!(request
            .to_ascii_lowercase()
            .contains("content-type: application/x-www-form-urlencoded"));
        assert!(request.contains("client_id=global_74c584"));
        assert!(request.contains("instance_id=inst-1"));
        assert!(request.contains("device_name=test-device"));
        assert!(request.contains("scope=all"));
        assert!(!request.contains("client_secret"));
    }

    #[tokio::test]
    async fn interactive_oauth_request_trace_records_authorization_and_polling() {
        let (endpoint, captured) = serve_oauth_responses(vec![
            (
                "authorize-1",
                r#"{"device_code":"device-code","user_code":"USER-CODE","verification_uri":"https://idsauth.volces.com/oauth/device","verification_uri_complete":"https://idsauth.volces.com/oauth/device#user_code=USER-CODE","expires_in":600,"interval":5}"#,
            ),
            (
                "poll-2",
                r#"{"access_token":"access-1","refresh_token":"refresh-1","token_type":"Bearer","expires_in":3600,"scope":"all","instance_id":"inst-1"}"#,
            ),
        ]);
        let trace = Arc::new(ServiceRequestTrace::default());
        let client = OAuthClient::new_with_request_trace(
            endpoint,
            ClientOptions::default(),
            Arc::clone(&trace),
        )
        .unwrap();

        client
            .create_device_authorization(&DeviceAuthorizationRequest {
                client_id: "client-id".to_string(),
                instance_id: "inst-1".to_string(),
                device_name: "test-device".to_string(),
                scope: "all".to_string(),
            })
            .await
            .unwrap();
        let outcome = client
            .poll_device_token("device-code", "client-id")
            .await
            .unwrap();

        assert!(matches!(outcome, DeviceTokenOutcome::Success(_)));
        assert!(captured.recv().is_ok());
        assert!(captured.recv().is_ok());
        let snapshot = trace.snapshot();
        assert_eq!(snapshot.request_ids, vec!["authorize-1", "poll-2"]);
        assert_eq!(
            snapshot.last_successful_request_id.as_deref(),
            Some("poll-2")
        );
    }

    #[tokio::test]
    async fn device_token_errors_are_classified_without_sensitive_description() {
        let cases = [
            ("authorization_pending", DeviceTokenOutcomeKind::Pending),
            ("slow_down", DeviceTokenOutcomeKind::SlowDown),
            ("access_denied", DeviceTokenOutcomeKind::Denied),
            ("expired_token", DeviceTokenOutcomeKind::Expired),
            ("invalid_grant", DeviceTokenOutcomeKind::Terminal),
        ];
        for (error_code, expected_kind) in cases {
            let body = format!(
                r#"{{"error":"{error_code}","error_description":"must-not-leak refresh-token-value","interval":10,"request_id":"req-1"}}"#
            );
            let (endpoint, captured) = serve_once(400, &body);
            let client = OAuthClient::new(endpoint, ClientOptions::default()).unwrap();

            let outcome = client
                .poll_device_token("device-code", "client-id")
                .await
                .unwrap();

            assert_eq!(outcome.kind(), expected_kind);
            assert!(!outcome.to_string().contains("refresh-token-value"));
            assert!(captured.recv().unwrap().contains("device_code=device-code"));
        }
    }

    #[tokio::test]
    async fn refresh_uses_one_form_request_and_parses_token_pair() {
        let (endpoint, captured) = serve_once(
            200,
            r#"{"access_token":"access-2","refresh_token":"refresh-2","token_type":"Bearer","expires_in":7200,"scope":"file:read file:list","instance_id":"inst-1"}"#,
        );
        let client = OAuthClient::new(endpoint, ClientOptions::default()).unwrap();

        let response = client
            .refresh_token("refresh-1", "client-id")
            .await
            .unwrap();

        assert_eq!(response.access_token, "access-2");
        assert_eq!(response.refresh_token, "refresh-2");
        let request = captured.recv().unwrap();
        assert!(request.starts_with("POST /v1/oauth/token "));
        assert!(request.contains("grant_type=refresh_token"));
        assert!(request.contains("refresh_token=refresh-1"));
        assert!(request.contains("client_id=client-id"));
        assert!(!request.contains("client_secret"));
    }
}
