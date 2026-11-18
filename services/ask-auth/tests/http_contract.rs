use ask_auth::build_router;
use axum::{
    body::{to_bytes, Body},
    http::{Request, Response, StatusCode},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use tower::ServiceExt;

const AUTH_ID: &str = "11111111-1111-7111-8111-111111111111";
const REQUEST_ID: &str = "44444444-4444-7444-8444-444444444444";
const NONCE: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

fn creation() -> Value {
    json!({"auth_id": AUTH_ID, "login_ticket": NONCE,
        "claim_challenge": NONCE, "device_name": "test-device", "cli_version": "1.0.3"})
}

fn request(method: &str, path: &str, payload: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if payload.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(Body::from(
            payload.map(|value| value.to_string()).unwrap_or_default(),
        ))
        .unwrap()
}

fn authenticated(mut request: Request<Body>) -> Request<Body> {
    request
        .headers_mut()
        .insert("authorization", format!("Bearer {NONCE}").parse().unwrap());
    request
}

async fn execute(request: Request<Body>) -> Response<Body> {
    build_router().oneshot(request).await.unwrap()
}

async fn json_response(response: Response<Body>, expected_status: StatusCode) -> Value {
    assert_eq!(response.status(), expected_status);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let request_id = response.headers()["x-request-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let parsed = uuid::Uuid::parse_str(&request_id).unwrap();
    assert_eq!(parsed.get_version_num(), 7);
    assert_eq!(parsed.to_string(), request_id);
    assert_eq!(parsed.get_variant(), uuid::Variant::RFC4122);
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let payload: Value = serde_json::from_slice(&bytes).unwrap();
    if payload.get("error").is_some() {
        assert_eq!(payload["request_id"], request_id);
    }
    payload
}

async fn error(request: Request<Body>, expected_status: StatusCode, expected_code: &str) {
    let payload = json_response(execute(request).await, expected_status).await;
    assert_eq!(payload["schema_version"], 1);
    assert_eq!(payload["error"]["code"], expected_code);
    assert!(payload["error"]["message"].as_str().unwrap().len() <= 1024);
    assert!(payload["error"]["retry_after_seconds"].is_null());
}

#[tokio::test]
async fn health_reports_running_but_readiness_stays_unavailable() {
    let healthy = json_response(
        execute(request("GET", "/healthz", None)).await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(healthy, json!({"status": "ok"}));
    let ready = json_response(
        execute(request("GET", "/readyz", None)).await,
        StatusCode::SERVICE_UNAVAILABLE,
    )
    .await;
    assert_eq!(ready, json!({"status": "unavailable"}));
}

#[tokio::test]
async fn valid_business_requests_are_unavailable_in_bootstrap() {
    let claims = json!({"auth_id": AUTH_ID, "claim_verifier": NONCE});
    let refresh =
        json!({"auth_id": AUTH_ID, "refresh_token": "example-refresh", "credential_generation": 1});
    let requests = [
        request("POST", "/v1/auth/login-attempts", Some(creation())),
        authenticated(request(
            "GET",
            &format!("/v1/auth/login-attempts/{AUTH_ID}"),
            None,
        )),
        authenticated(request(
            "GET",
            &format!("/v1/auth/status?auth_id={AUTH_ID}"),
            None,
        )),
        authenticated(request(
            "POST",
            "/v1/auth/logout",
            Some(json!({"auth_id": AUTH_ID})),
        )),
        with_idempotency(authenticated(request(
            "POST",
            "/v1/auth/token",
            Some(claims),
        ))),
        with_idempotency(authenticated(request(
            "POST",
            "/v1/auth/refresh",
            Some(refresh),
        ))),
    ];
    for request in requests {
        error(
            request,
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_unavailable",
        )
        .await;
    }
}

fn with_idempotency(mut request: Request<Body>) -> Request<Body> {
    request
        .headers_mut()
        .insert("idempotency-key", REQUEST_ID.parse().unwrap());
    request
}

#[tokio::test]
async fn missing_or_malformed_bearer_returns_only_authentication_required() {
    let path = format!("/v1/auth/status?auth_id={AUTH_ID}");
    for bearer in [
        None,
        Some("Basic abc"),
        Some("Bearer short"),
        Some("Bearer AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB"),
    ] {
        let mut call = request("GET", &path, None);
        if let Some(bearer) = bearer {
            call.headers_mut()
                .insert("authorization", bearer.parse().unwrap());
        }
        error(call, StatusCode::UNAUTHORIZED, "authentication_required").await;
    }
}

#[tokio::test]
async fn missing_or_malformed_identity_and_unknown_queries_are_rejected() {
    for path in [
        "/v1/auth/status",
        "/v1/auth/status?session_id=old",
        "/v1/auth/status?auth_id=no",
        "/v1/auth/status?auth_id=11111111-1111-4111-8111-111111111111&extra=1",
    ] {
        error(
            authenticated(request("GET", path, None)),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        )
        .await;
    }
    for payload in [
        json!({}),
        json!({"auth_id": "bad"}),
        json!({"session_id": AUTH_ID}),
    ] {
        error(
            authenticated(request("POST", "/v1/auth/logout", Some(payload))),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        )
        .await;
    }
}

#[tokio::test]
async fn canonical_identifiers_and_nonce_pad_bits_are_enforced() {
    for auth_id in [
        "11111111-1111-1111-8111-111111111111",
        "AAAAAAAA-AAAA-7AAA-8AAA-AAAAAAAAAAAA",
        "11111111111171118111111111111111",
    ] {
        let mut payload = creation();
        payload["auth_id"] = json!(auth_id);
        error(
            request("POST", "/v1/auth/login-attempts", Some(payload)),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        )
        .await;
    }
    for nonce in [
        format!("{NONCE}="),
        "A".repeat(42) + "B",
        "A".repeat(42),
        "A".repeat(44),
        "!".repeat(43),
    ] {
        let mut payload = creation();
        payload["login_ticket"] = json!(nonce);
        error(
            request("POST", "/v1/auth/login-attempts", Some(payload)),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        )
        .await;
    }
}

#[tokio::test]
async fn duplicate_json_query_and_security_headers_are_rejected() {
    let body = format!(r#"{{"auth_id":"{AUTH_ID}","auth_id":"{AUTH_ID}"}}"#);
    let mut duplicate_json = authenticated(request("POST", "/v1/auth/logout", None));
    duplicate_json
        .headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    *duplicate_json.body_mut() = Body::from(body);
    error(duplicate_json, StatusCode::BAD_REQUEST, "invalid_request").await;
    let duplicate_query = format!("/v1/auth/status?auth_id={AUTH_ID}&auth_id={AUTH_ID}");
    error(
        authenticated(request("GET", &duplicate_query, None)),
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await;
    let mut duplicate_auth = authenticated(request(
        "GET",
        &format!("/v1/auth/status?auth_id={AUTH_ID}"),
        None,
    ));
    duplicate_auth
        .headers_mut()
        .append("authorization", format!("Bearer {NONCE}").parse().unwrap());
    error(
        duplicate_auth,
        StatusCode::UNAUTHORIZED,
        "authentication_required",
    )
    .await;
    let claim = json!({"auth_id": AUTH_ID, "claim_verifier": NONCE});
    let mut duplicate_key = with_idempotency(authenticated(request(
        "POST",
        "/v1/auth/token",
        Some(claim),
    )));
    duplicate_key
        .headers_mut()
        .append("idempotency-key", REQUEST_ID.parse().unwrap());
    error(duplicate_key, StatusCode::BAD_REQUEST, "invalid_request").await;
}

#[tokio::test]
async fn invalid_json_content_type_body_size_and_generation_are_sanitized() {
    for (content_type, body) in [
        ("text/plain", "sensitive-raw-body".to_owned()),
        ("application/json", "{sensitive-invalid-json".to_owned()),
        ("application/json", "x".repeat(32769)),
    ] {
        let mut call = request("POST", "/v1/auth/login-attempts", None);
        call.headers_mut()
            .insert("content-type", content_type.parse().unwrap());
        *call.body_mut() = Body::from(body);
        error(call, StatusCode::BAD_REQUEST, "invalid_request").await;
    }
    for generation in [json!(0), json!(-1), json!(1.2), json!("1"), json!(1e30)] {
        let payload = json!({"auth_id": AUTH_ID, "refresh_token": "fake-token", "credential_generation": generation});
        error(
            with_idempotency(authenticated(request(
                "POST",
                "/v1/auth/refresh",
                Some(payload),
            ))),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        )
        .await;
    }
}

fn callback_state() -> String {
    let mut state = uuid::Uuid::parse_str(AUTH_ID).unwrap().as_bytes().to_vec();
    state.extend([42_u8; 32]);
    URL_SAFE_NO_PAD.encode(state)
}

#[tokio::test]
async fn callback_validates_full_state_and_never_reflects_parameters() {
    let state = callback_state();
    for query in [
        format!("state={state}&code=CALLBACK_SECRET_SENTINEL"),
        format!("state={state}&error=access_denied"),
    ] {
        let response = execute(request(
            "GET",
            &format!("/v1/oauth/doubao/callback?{query}"),
            None,
        ))
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_safe_callback(response).await;
    }
    for query in [
        format!("state={state}&code=a&error=b"),
        format!("state={state}&code=a&state={state}"),
        format!("state={state}&code=a&code=b"),
        format!("state={state}"),
        "state=bad&code=CALLBACK_SECRET_SENTINEL".into(),
        format!("state={}&code=a", "A".repeat(64)),
        format!("state={state}&code="),
        format!("state={state}%3D&code=a"),
    ] {
        let response = execute(request(
            "GET",
            &format!("/v1/oauth/doubao/callback?{query}"),
            None,
        ))
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_safe_callback(response).await;
    }
}

async fn assert_safe_callback(response: Response<Body>) {
    let request_id = response.headers()["x-request-id"].to_str().unwrap();
    let parsed = uuid::Uuid::parse_str(request_id).unwrap();
    assert_eq!(parsed.get_version_num(), 7);
    assert_eq!(parsed.get_variant(), uuid::Variant::RFC4122);
    assert_eq!(parsed.to_string(), request_id);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    let bytes = to_bytes(response.into_body(), 65536).await.unwrap();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(!html.contains("CALLBACK_SECRET_SENTINEL"));
    assert!(!html.contains(&callback_state()));
}

#[tokio::test]
async fn unknown_routes_methods_and_inbound_request_ids_are_sanitized() {
    for call in [
        request("GET", "/ask/proxy?token=secret", None),
        request("POST", "/healthz", None),
    ] {
        let response = execute(call).await;
        assert!(matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ));
        let expected_status = response.status();
        let payload = json_response(response, expected_status).await;
        assert_eq!(payload["error"]["code"], "invalid_request");
    }
    let mut previous_trace = None;
    for supplied_id in [AUTH_ID, REQUEST_ID, "11111111-1111-4111-8111-111111111111"] {
        let mut call = request("POST", "/v1/auth/login-attempts", Some(creation()));
        call.headers_mut()
            .insert("x-request-id", supplied_id.parse().unwrap());
        let response = execute(call).await;
        let trace = response.headers()["x-request-id"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_ne!(trace, supplied_id);
        assert_ne!(previous_trace.as_ref(), Some(&trace));
        previous_trace = Some(trace);
        json_response(response, StatusCode::SERVICE_UNAVAILABLE).await;
    }
}

#[tokio::test(start_paused = true)]
async fn pending_request_body_is_bounded_by_total_request_timeout() {
    let mut call = request("POST", "/v1/auth/login-attempts", Some(creation()));
    *call.body_mut() = Body::from_stream(futures::stream::pending::<
        Result<axum::body::Bytes, std::io::Error>,
    >());
    error(
        call,
        StatusCode::SERVICE_UNAVAILABLE,
        "upstream_unavailable",
    )
    .await;
}

#[tokio::test]
async fn interrupted_body_is_rejected_without_io_error_details() {
    let mut call = request("POST", "/v1/auth/login-attempts", Some(creation()));
    *call.body_mut() = Body::from_stream(futures::stream::once(async {
        Err::<axum::body::Bytes, _>(std::io::Error::other("BODY_IO_SECRET"))
    }));
    let response = execute(call).await;
    let payload = json_response(response, StatusCode::BAD_REQUEST).await;
    assert_eq!(payload["error"]["code"], "invalid_request");
    assert!(!payload.to_string().contains("BODY_IO_SECRET"));
}

#[tokio::test]
async fn malformed_percent_escapes_and_utf8_are_invalid_requests() {
    let state = callback_state();
    for code in ["%", "%0", "%GG", "%FF", "%C0%AF", "%ED%A0%80"] {
        let path = format!("/v1/oauth/doubao/callback?state={state}&code={code}");
        let response = execute(request("GET", &path, None)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_safe_callback(response).await;
    }
}

#[tokio::test]
async fn claim_and_refresh_require_canonical_single_idempotency_keys() {
    let cases = [
        (
            "/v1/auth/token",
            json!({"auth_id": AUTH_ID, "claim_verifier": NONCE}),
        ),
        (
            "/v1/auth/refresh",
            json!({"auth_id": AUTH_ID, "refresh_token": "fake-refresh", "credential_generation": 1}),
        ),
    ];
    for (path, payload) in cases {
        for key in [
            None,
            Some(""),
            Some("AAAAAAAA-AAAA-7AAA-8AAA-AAAAAAAAAAAA"),
            Some("11111111-1111-7111-7111-111111111111"),
            Some("11111111-1111-4111-8111-111111111111"),
            Some("11111111111171118111111111111111"),
        ] {
            let mut call = authenticated(request("POST", path, Some(payload.clone())));
            if let Some(key) = key {
                call.headers_mut()
                    .insert("idempotency-key", key.parse().unwrap());
            }
            error(call, StatusCode::BAD_REQUEST, "invalid_request").await;
        }
    }
}

#[tokio::test]
async fn strict_json_rejects_unknown_fields_for_every_request_dto() {
    let cases = [
        ("/v1/auth/login-attempts", creation()),
        (
            "/v1/auth/token",
            json!({"auth_id": AUTH_ID, "claim_verifier": NONCE}),
        ),
        (
            "/v1/auth/refresh",
            json!({"auth_id": AUTH_ID, "refresh_token": "fake-refresh", "credential_generation": 1}),
        ),
        ("/v1/auth/logout", json!({"auth_id": AUTH_ID})),
    ];
    for (path, mut payload) in cases {
        payload["untrusted_identity"] = json!("untrusted-user");
        let call = with_idempotency(authenticated(request("POST", path, Some(payload))));
        error(call, StatusCode::BAD_REQUEST, "invalid_request").await;
    }
}

#[tokio::test]
async fn creation_text_lengths_enforce_schema_character_boundaries() {
    for (field, maximum) in [("device_name", 128), ("cli_version", 64)] {
        for length in [0, maximum + 1] {
            let mut payload = creation();
            payload[field] = json!("界".repeat(length));
            error(
                request("POST", "/v1/auth/login-attempts", Some(payload)),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            )
            .await;
        }
        let mut payload = creation();
        payload[field] = json!("界".repeat(maximum));
        error(
            request("POST", "/v1/auth/login-attempts", Some(payload)),
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_unavailable",
        )
        .await;
    }
}

#[tokio::test]
async fn exact_body_limit_accepts_valid_json_and_rejects_one_extra_byte() {
    for (length, expected_status) in [
        (32768, StatusCode::SERVICE_UNAVAILABLE),
        (32769, StatusCode::BAD_REQUEST),
    ] {
        let mut bytes = creation().to_string().into_bytes();
        bytes.resize(length, b' ');
        let mut call = request("POST", "/v1/auth/login-attempts", Some(creation()));
        *call.body_mut() = Body::from(bytes);
        let expected_code = if expected_status == StatusCode::BAD_REQUEST {
            "invalid_request"
        } else {
            "upstream_unavailable"
        };
        error(call, expected_status, expected_code).await;
    }
}

#[tokio::test]
async fn callback_enforces_code_error_lengths_and_uuid_variant() {
    let state = callback_state();
    for query in [
        format!("state={state}&code={}", "a".repeat(8193)),
        format!("state={state}&error={}", "a".repeat(129)),
        format!("state={state}&error="),
        format!("state={state}&code=a&extra=b"),
    ] {
        let response = execute(request(
            "GET",
            &format!("/v1/oauth/doubao/callback?{query}"),
            None,
        ))
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_safe_callback(response).await;
    }
    let mut decoded = URL_SAFE_NO_PAD.decode(state).unwrap();
    decoded[8] = 0x11;
    let path = format!(
        "/v1/oauth/doubao/callback?state={}&code=a",
        URL_SAFE_NO_PAD.encode(decoded)
    );
    assert_eq!(
        execute(request("GET", &path, None)).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn auth_and_operation_ids_require_v7() {
    let mut payload = creation();
    payload["auth_id"] = json!("11111111-1111-4111-8111-111111111111");
    error(
        request("POST", "/v1/auth/login-attempts", Some(payload)),
        StatusCode::BAD_REQUEST,
        "invalid_request",
    )
    .await;
    let mut call = with_idempotency(authenticated(request(
        "POST",
        "/v1/auth/token",
        Some(json!({"auth_id": AUTH_ID, "claim_verifier": NONCE})),
    )));
    call.headers_mut().insert(
        "idempotency-key",
        "11111111-1111-4111-8111-111111111111".parse().unwrap(),
    );
    error(call, StatusCode::BAD_REQUEST, "invalid_request").await;
}

#[tokio::test]
async fn v4_auth_ids_are_rejected_in_path_query_and_callback_state() {
    let legacy = "11111111-1111-4111-8111-111111111111";
    for path in [
        format!("/v1/auth/login-attempts/{legacy}"),
        format!("/v1/auth/status?auth_id={legacy}"),
    ] {
        error(
            authenticated(request("GET", &path, None)),
            StatusCode::BAD_REQUEST,
            "invalid_request",
        )
        .await;
    }
    let mut state = uuid::Uuid::parse_str(legacy).unwrap().as_bytes().to_vec();
    state.extend_from_slice(&[42; 32]);
    let query = format!(
        "/v1/oauth/doubao/callback?state={}&code=test",
        URL_SAFE_NO_PAD.encode(state)
    );
    let response = execute(request("GET", &query, None)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
