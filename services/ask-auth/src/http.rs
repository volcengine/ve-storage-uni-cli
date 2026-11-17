use axum::{
    body::{to_bytes, Body},
    extract::{MatchedPath, Request},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::any,
    Extension, Json, Router,
};
use serde::de::DeserializeOwned;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    error::ProtocolError,
    provider::OAuthProvider,
    validation::{self, Validate},
};

const MAX_BODY_BYTES: usize = 32 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CALLBACK: &str = "/v1/oauth/doubao/callback";
const ROUTES: [&str; 9] = [
    "/healthz",
    "/readyz",
    "/v1/auth/login-attempts",
    "/v1/auth/login-attempts/{auth_id}",
    "/v1/auth/token",
    "/v1/auth/refresh",
    "/v1/auth/status",
    "/v1/auth/logout",
    CALLBACK,
];

#[derive(Clone)]
struct RequestId(String);

#[derive(Clone)]
struct AppState {
    // Disabled-provider seam remains isolated from the explicitly enabled service.
    _oauth_provider: Arc<dyn OAuthProvider>,
    login: Option<Arc<crate::login::LoginService>>,
}

pub(crate) fn router(provider: Arc<dyn OAuthProvider>) -> Router {
    let mut application = Router::new();
    for route in ROUTES {
        application = application.route(route, any(dispatch));
    }
    application
        .fallback(fallback)
        .layer(Extension(AppState {
            _oauth_provider: provider,
            login: None,
        }))
        .layer(middleware::from_fn(request_scope))
}

pub(crate) fn enabled_router(service: Arc<crate::login::LoginService>) -> Router {
    let mut application = Router::new();
    for route in ROUTES {
        application = application.route(route, any(dispatch));
    }
    application
        .fallback(fallback)
        .layer(Extension(AppState {
            _oauth_provider: Arc::new(crate::provider::DisabledProvider),
            login: Some(service),
        }))
        .layer(middleware::from_fn(enabled_request_scope))
}

async fn request_scope(request: Request, next: Next) -> Response {
    scoped_request(request, next, false).await
}
async fn enabled_request_scope(request: Request, next: Next) -> Response {
    scoped_request(request, next, true).await
}
async fn scoped_request(mut request: Request, next: Next, is_enabled: bool) -> Response {
    // [Review Fix #21] Mint a fresh v7 trace independently of every client-supplied ID.
    let request_id = uuid::Uuid::now_v7().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_else(|| "<unmatched>".into());
    let method = safe_method(request.method());
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));
    let started = Instant::now();
    // [Review Fix #22] Enabled lifecycle uses its total budget; body reads retain a separate cap.
    let timeout = if is_enabled {
        Duration::from_secs(20)
    } else {
        REQUEST_TIMEOUT
    };
    let mut response = match tokio::time::timeout(timeout, next.run(request)).await {
        Ok(response) => response,
        Err(_) if route == CALLBACK => callback_response(StatusCode::SERVICE_UNAVAILABLE),
        Err(_) => ProtocolError::Unavailable.response(&request_id),
    };
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    if let Ok(header) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", header);
    }
    tracing::info!(target: "ask_auth::request", route, method, request_id,
        status = response.status().as_u16(), duration_ms = started.elapsed().as_millis() as u64,
        "request completed");
    response
}

fn safe_method(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::POST => "POST",
        Method::HEAD => "HEAD",
        Method::PUT => "PUT",
        Method::PATCH => "PATCH",
        Method::DELETE => "DELETE",
        Method::OPTIONS => "OPTIONS",
        Method::CONNECT => "CONNECT",
        Method::TRACE => "TRACE",
        _ => "OTHER",
    }
}

async fn fallback(Extension(request_id): Extension<RequestId>) -> Response {
    ProtocolError::InvalidRequest.with_status(&request_id.0, StatusCode::NOT_FOUND)
}

async fn dispatch(request: Request) -> Response {
    let request_id = request
        .extensions()
        .get::<RequestId>()
        .map(|id| id.0.clone())
        .unwrap_or_default();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str())
        .unwrap_or("");
    if !is_allowed_method(route, request.method()) {
        return ProtocolError::InvalidRequest
            .with_status(&request_id, StatusCode::METHOD_NOT_ALLOWED);
    }
    if let Some(service) = enabled_service(&request, route) {
        return service.handle(request, &request_id).await;
    }
    match route {
        "/healthz" => Json(serde_json::json!({"status": "ok"})).into_response(),
        "/readyz" => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"status": "unavailable"})),
        )
            .into_response(),
        CALLBACK => callback_response(
            if validation::validate_callback(request.uri().query()).is_ok() {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::BAD_REQUEST
            },
        ),
        _ => business_response(request, &request_id).await,
    }
}

fn enabled_service(request: &Request, route: &str) -> Option<Arc<crate::login::LoginService>> {
    if !matches!(
        route,
        "/readyz"
            | "/v1/auth/login-attempts"
            | "/v1/auth/login-attempts/{auth_id}"
            | "/v1/auth/token"
            | "/v1/auth/refresh"
            | "/v1/auth/status"
            | "/v1/auth/logout"
            | CALLBACK
    ) {
        return None;
    }
    request
        .extensions()
        .get::<AppState>()
        .and_then(|state| state.login.clone())
}

fn is_allowed_method(route: &str, method: &Method) -> bool {
    let is_get = matches!(
        route,
        "/healthz" | "/readyz" | "/v1/auth/status" | "/v1/auth/login-attempts/{auth_id}" | CALLBACK
    );
    *method == if is_get { Method::GET } else { Method::POST }
}

async fn business_response(request: Request, request_id: &str) -> Response {
    match validate_business(request).await {
        Ok(()) => ProtocolError::Unavailable.response(request_id),
        Err(error) => error.response(request_id),
    }
}

async fn validate_business(request: Request) -> Result<(), ProtocolError> {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str())
        .unwrap_or("");
    validate_query(&request, route)?;
    if route != "/v1/auth/login-attempts" {
        validate_bearer(request.headers())?;
    }
    if matches!(route, "/v1/auth/token" | "/v1/auth/refresh") {
        let key = single_header(request.headers(), "idempotency-key")
            .ok_or(ProtocolError::InvalidRequest)?;
        if !validation::is_request_id(key) {
            return Err(ProtocolError::InvalidRequest);
        }
    }
    match route {
        "/v1/auth/login-attempts" => validate_json::<validation::LoginCreateRequest>(request).await,
        "/v1/auth/token" => validate_json::<validation::ClaimRequest>(request).await,
        "/v1/auth/refresh" => validate_json::<validation::RefreshRequest>(request).await,
        "/v1/auth/logout" => validate_json::<validation::LogoutRequest>(request).await,
        _ => Ok(()),
    }
}

fn validate_query(request: &Request, route: &str) -> Result<(), ProtocolError> {
    if route == "/v1/auth/status" {
        return validation::validate_status(request.uri().query());
    }
    if !validation::query_fields(request.uri().query())?.is_empty() {
        return Err(ProtocolError::InvalidRequest);
    }
    if route == "/v1/auth/login-attempts/{auth_id}" {
        let identifier = request.uri().path().rsplit('/').next().unwrap_or_default();
        if !validation::is_auth_id(identifier) {
            return Err(ProtocolError::InvalidRequest);
        }
    }
    Ok(())
}

fn single_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        None
    } else {
        Some(first)
    }
}

fn validate_bearer(headers: &HeaderMap) -> Result<(), ProtocolError> {
    let token = single_header(headers, "authorization")
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
        .map(|(_, token)| token)
        .filter(|token| validation::is_nonce(token));
    token
        .map(|_| ())
        .ok_or(ProtocolError::AuthenticationRequired)
}

async fn validate_json<T: DeserializeOwned + Validate>(
    request: Request<Body>,
) -> Result<(), ProtocolError> {
    let is_json = single_header(request.headers(), "content-type")
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"));
    if !is_json {
        return Err(ProtocolError::InvalidRequest);
    }
    let bytes = to_bytes(request.into_body(), MAX_BODY_BYTES)
        .await
        .map_err(|_| ProtocolError::InvalidRequest)?;
    // [Review Fix #72] Disabled routes enforce the same frozen object shape and depth bound as enabled routes.
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return Err(ProtocolError::InvalidRequest);
    }
    crate::json::check_json_depth(&bytes).map_err(|_| ProtocolError::InvalidRequest)?;
    let payload: T = serde_json::from_slice(&bytes).map_err(|_| ProtocolError::InvalidRequest)?;
    payload.validate()
}

pub(crate) fn callback_response(status: StatusCode) -> Response {
    // [Review Fix #28] A completed callback uses neutral fixed HTML instead of reporting failure.
    let html = if status == StatusCode::OK {
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Authorization processed</title><p>Return to the CLI for the authorization result.</p></html>"
    } else {
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Authorization unavailable</title><p>Authorization could not be completed. Return to the CLI.</p></html>"
    };
    let mut response = (status, Html(html)).into_response();
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    #[tokio::test]
    async fn successful_callback_html_is_fixed_and_does_not_report_failure() {
        let response = callback_response(StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let html = std::str::from_utf8(&body).unwrap();
        assert!(html.contains("Authorization processed"));
        assert!(!html.contains("could not be completed"));
    }
}

#[cfg(test)]
mod json_tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use serde_json::{json, Value};
    use tower::ServiceExt;

    fn create_fields() -> Value {
        let nonce = URL_SAFE_NO_PAD.encode([1; 32]);
        json!({
            "auth_id": uuid::Uuid::now_v7().to_string(),
            "login_ticket": nonce,
            "claim_challenge": nonce,
            "device_name": "workstation",
            "cli_version": "0.1.0",
        })
    }
    fn request(body: String) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/auth/login-attempts")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn disabled_create_rejects_positional_json_packets() {
        let fields = create_fields();
        let positional: Vec<Value> = [
            "auth_id",
            "login_ticket",
            "claim_challenge",
            "device_name",
            "cli_version",
        ]
        .iter()
        .map(|field| fields[field].clone())
        .collect();
        let body = json!(positional).to_string();
        assert!(matches!(
            validate_json::<validation::LoginCreateRequest>(request(body.clone())).await,
            Err(ProtocolError::InvalidRequest)
        ));
        let response = crate::build_router().oneshot(request(body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn disabled_create_accepts_literal_brackets_and_escaped_quotes_in_objects() {
        let mut fields = create_fields();
        fields["device_name"] = json!(r#"work [station] {\"quoted\"}"#);
        let body = format!(" \n\r\t{}", fields);
        assert!(
            validate_json::<validation::LoginCreateRequest>(request(body.clone()))
                .await
                .is_ok()
        );
        let response = crate::build_router().oneshot(request(body)).await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
