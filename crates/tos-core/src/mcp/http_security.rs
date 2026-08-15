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

use std::str::FromStr;

use axum::{
    extract::{Request, State},
    http::{
        header::{AUTHORIZATION, HOST, ORIGIN, WWW_AUTHENTICATE},
        uri::Authority,
        HeaderMap, HeaderValue, StatusCode, Uri,
    },
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const TOKEN_BYTES: usize = 32;

/// Generates a fresh high-entropy credential for one MCP SSE server process.
pub(super) fn generate_bearer_token() -> String {
    let mut token_bytes = [0_u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut token_bytes);
    URL_SAFE_NO_PAD.encode(token_bytes)
}

/// Header-validation state for one loopback MCP SSE listener.
#[derive(Clone)]
pub(super) struct McpHttpSecurity {
    token_digest: [u8; 32],
    port: u16,
}

/// Marker inserted after all MCP HTTP request checks have succeeded.
#[derive(Clone, Copy, Debug)]
pub(super) struct AuthenticatedMcpHttpRequest;

/// Stable policy outcomes mapped to fail-closed HTTP responses by the middleware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SecurityError {
    MissingHost,
    MalformedHost,
    ForbiddenHost,
    ForbiddenOrigin,
    Unauthorized,
}

impl IntoResponse for SecurityError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::MissingHost => (StatusCode::BAD_REQUEST, "missing Host header"),
            Self::MalformedHost => (StatusCode::BAD_REQUEST, "malformed Host header"),
            Self::ForbiddenHost => (StatusCode::FORBIDDEN, "Host is not allowed"),
            Self::ForbiddenOrigin => (StatusCode::FORBIDDEN, "Origin is not allowed"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "Bearer credential required"),
        };
        let mut response = (status, message).into_response();
        if self == Self::Unauthorized {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        response
    }
}

impl McpHttpSecurity {
    /// Builds policy state without retaining the plaintext startup credential.
    pub(super) fn new(token: &str, port: u16) -> Self {
        Self {
            token_digest: Sha256::digest(token.as_bytes()).into(),
            port,
        }
    }

    fn authorize(&self, headers: &HeaderMap) -> Result<(), SecurityError> {
        self.authorize_host(headers)?;
        self.authorize_origin(headers)?;
        self.authorize_bearer(headers)
    }

    fn authorize_host(&self, headers: &HeaderMap) -> Result<(), SecurityError> {
        let host = single_header(headers, HOST).map_err(|error| match error {
            HeaderLookupError::Missing => SecurityError::MissingHost,
            HeaderLookupError::DuplicateOrInvalid => SecurityError::MalformedHost,
        })?;
        let authority = Authority::from_str(host).map_err(|_| SecurityError::MalformedHost)?;
        // [Review Fix #6] Authority::host() hides userinfo, which is invalid in a Host header.
        if authority.as_str().contains('@') {
            return Err(SecurityError::MalformedHost);
        }
        let port = authority.port_u16().ok_or(SecurityError::MalformedHost)?;
        let has_allowed_name =
            authority.host() == "127.0.0.1" || authority.host().eq_ignore_ascii_case("localhost");
        if !has_allowed_name || port != self.port {
            return Err(SecurityError::ForbiddenHost);
        }
        Ok(())
    }

    fn authorize_origin(&self, headers: &HeaderMap) -> Result<(), SecurityError> {
        let Some(origin) =
            optional_single_header(headers, ORIGIN).map_err(|_| SecurityError::ForbiddenOrigin)?
        else {
            return Ok(());
        };
        let uri = Uri::from_str(origin).map_err(|_| SecurityError::ForbiddenOrigin)?;
        let authority = uri.authority().ok_or(SecurityError::ForbiddenOrigin)?;
        // [Review Fix #1] URI host parsing discards userinfo, so reject it before host comparison.
        let has_userinfo = authority.as_str().contains('@');
        let has_allowed_name =
            authority.host() == "127.0.0.1" || authority.host().eq_ignore_ascii_case("localhost");
        // [Review Fix #8] `http::Uri` normalizes an absent path to `/`; compare the original
        // serialization so a browser Origin tuple cannot carry any path or query text.
        let has_origin_resource = origin.strip_prefix("http://") != Some(authority.as_str());
        if uri.scheme_str() != Some("http")
            || has_userinfo
            || !has_allowed_name
            || authority.port_u16() != Some(self.port)
            || has_origin_resource
        {
            return Err(SecurityError::ForbiddenOrigin);
        }
        Ok(())
    }

    fn authorize_bearer(&self, headers: &HeaderMap) -> Result<(), SecurityError> {
        let value =
            single_header(headers, AUTHORIZATION).map_err(|_| SecurityError::Unauthorized)?;
        let mut parts = value.split_ascii_whitespace();
        let scheme = parts.next().ok_or(SecurityError::Unauthorized)?;
        let candidate = parts.next().ok_or(SecurityError::Unauthorized)?;
        if !scheme.eq_ignore_ascii_case("Bearer") || parts.next().is_some() {
            return Err(SecurityError::Unauthorized);
        }
        let candidate_digest: [u8; 32] = Sha256::digest(candidate.as_bytes()).into();
        if !bool::from(self.token_digest.ct_eq(&candidate_digest)) {
            return Err(SecurityError::Unauthorized);
        }
        Ok(())
    }
}

/// Applies fail-closed MCP HTTP checks to every route in `router`.
pub(super) fn protect_router(router: Router, security: McpHttpSecurity) -> Router {
    router.layer(middleware::from_fn_with_state(
        security,
        enforce_http_security,
    ))
}

async fn enforce_http_security(
    State(security): State<McpHttpSecurity>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Err(error) = security.authorize(request.headers()) {
        return error.into_response();
    }
    request.headers_mut().remove(AUTHORIZATION);
    request.extensions_mut().insert(AuthenticatedMcpHttpRequest);
    next.run(request).await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeaderLookupError {
    Missing,
    DuplicateOrInvalid,
}

fn single_header(
    headers: &HeaderMap,
    name: axum::http::header::HeaderName,
) -> Result<&str, HeaderLookupError> {
    optional_single_header(headers, name)?.ok_or(HeaderLookupError::Missing)
}

fn optional_single_header(
    headers: &HeaderMap,
    name: axum::http::header::HeaderName,
) -> Result<Option<&str>, HeaderLookupError> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(HeaderLookupError::DuplicateOrInvalid);
    }
    value
        .to_str()
        .map(Some)
        .map_err(|_| HeaderLookupError::DuplicateOrInvalid)
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        extract::Extension,
        http::{
            header::{AUTHORIZATION, HOST, ORIGIN, WWW_AUTHENTICATE},
            HeaderMap, HeaderValue, Method, Request, StatusCode,
        },
        response::Response,
        routing::{get, post},
        Router,
    };
    use tower::ServiceExt;

    use super::*;

    fn request_headers(
        host: Option<&str>,
        origin: Option<&str>,
        bearer: Option<&str>,
    ) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(host) = host {
            headers.insert(HOST, HeaderValue::from_str(host).unwrap());
        }
        if let Some(origin) = origin {
            headers.insert(ORIGIN, HeaderValue::from_str(origin).unwrap());
        }
        if let Some(bearer) = bearer {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {bearer}")).unwrap(),
            );
        }
        headers
    }

    fn test_router() -> Router {
        async fn inspect_request(
            headers: HeaderMap,
            Extension(_authenticated): Extension<AuthenticatedMcpHttpRequest>,
        ) -> &'static str {
            if headers.contains_key(AUTHORIZATION) {
                "authorization-leaked"
            } else {
                "authorization-stripped"
            }
        }

        Router::new()
            .route("/sse", get(inspect_request))
            .route("/message", post(inspect_request))
    }

    // [Review Fix #2] Bundle request inputs so the test helper stays within the five-parameter limit.
    struct TestRequest<'a> {
        method: Method,
        path: &'a str,
        host: Option<&'a str>,
        origin: Option<&'a str>,
        bearer: Option<&'a str>,
    }

    impl<'a> TestRequest<'a> {
        fn new(
            method: Method,
            path: &'a str,
            host: Option<&'a str>,
            origin: Option<&'a str>,
            bearer: Option<&'a str>,
        ) -> Self {
            Self {
                method,
                path,
                host,
                origin,
                bearer,
            }
        }
    }

    async fn send_request(app: &Router, input: TestRequest<'_>) -> Response {
        let mut request = Request::builder().method(input.method).uri(input.path);
        for (name, value) in request_headers(input.host, input.origin, input.bearer) {
            if let Some(name) = name {
                request = request.header(name, value);
            }
        }
        app.clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn response_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn generated_token_contains_32_random_bytes_in_base64url() {
        let first = generate_bearer_token();
        let second = generate_bearer_token();

        assert_ne!(first, second);
        assert!(!first.contains('='));
        assert_eq!(URL_SAFE_NO_PAD.decode(first).unwrap().len(), 32);
    }

    #[test]
    fn policy_accepts_only_exact_loopback_authorities() {
        let token = generate_bearer_token();
        let security = McpHttpSecurity::new(&token, 19090);

        for host in ["127.0.0.1:19090", "localhost:19090", "LOCALHOST:19090"] {
            assert_eq!(
                security.authorize(&request_headers(Some(host), None, Some(&token))),
                Ok(())
            );
        }
        assert_eq!(
            security.authorize(&request_headers(None, None, Some(&token))),
            Err(SecurityError::MissingHost)
        );
        assert_eq!(
            security.authorize(&request_headers(Some("localhost"), None, Some(&token))),
            Err(SecurityError::MalformedHost)
        );
        // [Review Fix #6] Host authority parsing must not hide userinfo before comparison.
        assert_eq!(
            security.authorize(&request_headers(
                Some("attacker@localhost:19090"),
                None,
                Some(&token),
            )),
            Err(SecurityError::MalformedHost)
        );
        for host in [
            "evil.example:19090",
            "127.0.0.2:19090",
            "127.0.0.1:19091",
            "[::1]:19090",
            // [Review Fix #4] DNS aliases that resolve to loopback must retain their hostile Host.
            "127.0.0.1.nip.io:19090",
        ] {
            assert_eq!(
                security.authorize(&request_headers(Some(host), None, Some(&token))),
                Err(SecurityError::ForbiddenHost)
            );
        }
    }

    #[test]
    fn policy_allows_missing_origin_or_exact_http_loopback_origin() {
        let token = generate_bearer_token();
        let security = McpHttpSecurity::new(&token, 19090);

        for origin in [
            None,
            Some("http://127.0.0.1:19090"),
            Some("http://localhost:19090"),
            Some("http://LOCALHOST:19090"),
        ] {
            assert_eq!(
                security.authorize(&request_headers(
                    Some("127.0.0.1:19090"),
                    origin,
                    Some(&token),
                )),
                Ok(())
            );
        }
        for origin in [
            "null",
            "https://localhost:19090",
            "http://evil.example:19090",
            "http://127.0.0.1:19091",
            // [Review Fix #1] An Origin must be an origin tuple, never a URL with userinfo or resources.
            "http://attacker@localhost:19090",
            // [Review Fix #8] RFC Origin serialization has no path, including a trailing slash.
            "http://localhost:19090/",
            "http://localhost:19090/path",
            "http://localhost:19090?query",
            "not a URL",
        ] {
            assert_eq!(
                security.authorize(&request_headers(
                    Some("127.0.0.1:19090"),
                    Some(origin),
                    Some(&token),
                )),
                Err(SecurityError::ForbiddenOrigin),
                "unexpected Origin result for {origin}"
            );
        }
    }

    #[test]
    fn policy_requires_one_matching_bearer_credential() {
        let token = generate_bearer_token();
        let security = McpHttpSecurity::new(&token, 19090);

        for bearer in [None, Some(""), Some("wrong-token")] {
            assert_eq!(
                security.authorize(&request_headers(Some("127.0.0.1:19090"), None, bearer,)),
                Err(SecurityError::Unauthorized)
            );
        }

        let mut duplicate = request_headers(Some("127.0.0.1:19090"), None, Some(&token));
        duplicate.append(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer second-token"),
        );
        assert_eq!(
            security.authorize(&duplicate),
            Err(SecurityError::Unauthorized)
        );
    }

    #[tokio::test]
    async fn middleware_protects_every_mcp_route_and_strips_authorization() {
        let token = generate_bearer_token();
        let app = protect_router(test_router(), McpHttpSecurity::new(&token, 19090));

        // [Review Fix #5] A session identifier never substitutes for authentication on POST.
        for (method, path) in [
            (Method::GET, "/sse"),
            (Method::POST, "/message?sessionId=known-session"),
        ] {
            let anonymous = send_request(
                &app,
                TestRequest::new(method.clone(), path, Some("127.0.0.1:19090"), None, None),
            )
            .await;
            assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(anonymous.headers().get(WWW_AUTHENTICATE).unwrap(), "Bearer");

            let incorrect = send_request(
                &app,
                TestRequest::new(
                    method.clone(),
                    path,
                    Some("127.0.0.1:19090"),
                    None,
                    Some("wrong-token"),
                ),
            )
            .await;
            assert_eq!(incorrect.status(), StatusCode::UNAUTHORIZED);

            let authenticated = send_request(
                &app,
                TestRequest::new(method, path, Some("127.0.0.1:19090"), None, Some(&token)),
            )
            .await;
            assert_eq!(authenticated.status(), StatusCode::OK);
            assert_eq!(response_text(authenticated).await, "authorization-stripped");
        }
    }

    #[tokio::test]
    async fn middleware_maps_host_origin_and_bearer_failures_without_leaking_token() {
        let token = generate_bearer_token();
        let app = protect_router(test_router(), McpHttpSecurity::new(&token, 19090));
        let cases = [
            (None, None, Some(token.as_str()), StatusCode::BAD_REQUEST),
            (
                Some("evil.example:19090"),
                None,
                Some(token.as_str()),
                StatusCode::FORBIDDEN,
            ),
            (
                Some("127.0.0.1:19090"),
                Some("http://evil.example:19090"),
                Some(token.as_str()),
                StatusCode::FORBIDDEN,
            ),
            (
                Some("127.0.0.1:19090"),
                None,
                Some("wrong-token"),
                StatusCode::UNAUTHORIZED,
            ),
        ];

        for (host, origin, bearer, status) in cases {
            let response = send_request(
                &app,
                TestRequest::new(Method::GET, "/sse", host, origin, bearer),
            )
            .await;
            assert_eq!(response.status(), status);
            assert!(!response_text(response).await.contains(&token));
        }
    }
}
