/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy at http://www.apache.org/licenses/LICENSE-2.0
 * Unless required by law or agreed in writing, software is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND.
 */

use super::*;
use crate::infra::zti_credentials::{ZtiTokenError, ZtiTokenProvider};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn profile(endpoint: String) -> Profile {
    Profile {
        region: Some("cn-beijing".into()),
        endpoint: Some(endpoint),
        max_retry_count: Some(1),
        requesttimeout: Some(2),
        connecttimeout: Some(2),
        ..Default::default()
    }
}

fn provider(calls: Arc<AtomicUsize>) -> ZtiTokenProvider {
    ZtiTokenProvider::new(move || {
        let number = calls.fetch_add(1, Ordering::SeqCst) + 1;
        async move { Ok(format!("test-zti-{number}")) }
    })
}

async fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = stream.read(&mut buffer).await.expect("read request");
        assert!(count > 0, "incomplete HTTP request");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = header(&headers, "content-length")
                .unwrap_or("0")
                .parse::<usize>()
                .expect("body length");
            if bytes.len() >= end + 4 + length {
                return String::from_utf8(bytes).expect("test request UTF-8");
            }
        }
    }
}

async fn server(responses: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let request = tokio::time::timeout(Duration::from_secs(5), async {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let request = read_request(&mut stream).await;
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("respond");
                request
            })
            .await
            .expect("request timeout");
            requests.push(request);
        }
        requests
    });
    (endpoint, task)
}

fn response(status: &str) -> String {
    format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\nx-tos-request-id: zti-test\r\n\r\n")
}

fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then_some(value.trim())
    })
}

fn assert_zti_only(request: &str, number: usize) {
    assert_eq!(
        header(request, "x-tos-ztitoken-with-acp"),
        Some(format!("test-zti-{number}").as_str())
    );
    for forbidden in [
        "authorization",
        "x-tos-signature",
        "x-tos-copy-signature",
        "x-tos-security-token",
    ] {
        assert!(
            header(request, forbidden).is_none(),
            "unexpected {forbidden}"
        );
    }
    assert!(!request.contains("profile-secret"));
}

async fn get(client: &TosClient) -> Result<Response, CliError> {
    client
        .send_request(
            Method::GET,
            &client.object_endpoint("bucket", "key")?,
            &client.object_request_path("bucket", "key")?,
            BTreeMap::new(),
            BTreeMap::new(),
            None,
        )
        .await
}

#[tokio::test]
async fn zti_construction_ignores_aksk_and_defers_provider() {
    let calls = Arc::new(AtomicUsize::new(0));
    let client = TosClient::new_with_zti_credentials(
        &profile("http://127.0.0.1:1".into()),
        provider(calls.clone()),
    )
    .expect("no AK/SK required");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(client.sign_algorithm, TosSignAlgorithm::ByteTosV1);
    assert_eq!(
        client.object_request_path("bucket", "a/b").unwrap(),
        "/bucket/a/b"
    );
}

#[tokio::test]
async fn zti_buffered_retry_reads_current_token_per_attempt() {
    let (endpoint, task) = server(vec![
        response("500 Internal Server Error"),
        response("200 OK"),
    ])
    .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let client =
        TosClient::new_with_zti_credentials(&profile(endpoint), provider(calls.clone())).unwrap();
    assert_eq!(get(&client).await.unwrap().status(), StatusCode::OK);
    let requests = task.await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    for (index, request) in requests.iter().enumerate() {
        assert!(request.starts_with("GET /bucket/key HTTP/1.1"));
        assert_zti_only(request, index + 1);
    }
}

#[tokio::test]
async fn zti_copy_preserves_source_and_removes_supplied_aksk_headers() {
    let (endpoint, task) = server(vec![response("200 OK")]).await;
    let mut settings = profile(endpoint);
    settings.access_key_id = Some("profile-secret-ak".into());
    settings.secret_access_key = Some("profile-secret-sk".into());
    settings.security_token = Some("profile-secret-sts".into());
    let client = TosClient::new_with_zti_credentials(&settings, provider(Arc::default())).unwrap();
    let mut headers = BTreeMap::from([("x-tos-copy-source".into(), "%2Fsource%2Fkey".into())]);
    for name in [
        "Authorization",
        "X-TOS-SIGNATURE",
        "x-tos-copy-signature",
        "X-Tos-Security-Token",
        "X-Tos-ZtiToken-With-Acp",
    ] {
        headers.insert(name.into(), "stale-secret".into());
    }
    client
        .send_request(
            Method::POST,
            &client.object_endpoint("bucket", "copy").unwrap(),
            "/bucket/copy",
            BTreeMap::from([("copyobject".into(), "".into())]),
            headers,
            None,
        )
        .await
        .unwrap();
    let requests = task.await.unwrap();
    assert_zti_only(&requests[0], 1);
    assert_eq!(
        header(&requests[0], "x-tos-copy-source"),
        Some("%2Fsource%2Fkey")
    );
    assert!(requests[0].starts_with("POST /bucket/copy?copyobject= HTTP/1.1"));
    assert!(!requests[0].contains("stale-secret"));
}

#[tokio::test]
async fn zti_streaming_retry_replays_body_with_current_token() {
    let (endpoint, task) = server(vec![
        response("503 Service Unavailable"),
        response("200 OK"),
    ])
    .await;
    let calls = Arc::new(AtomicUsize::new(0));
    let client =
        TosClient::new_with_zti_credentials(&profile(endpoint), provider(calls.clone())).unwrap();
    let request = ReplayableStreamingRequest {
        method: Method::PUT,
        url: client.object_endpoint("bucket", "key").unwrap(),
        path: "/bucket/key".into(),
        query_params: BTreeMap::from([
            ("uploadID".into(), "opaque+/=".into()),
            ("partNumber".into(), "1".into()),
        ]),
        extra_headers: BTreeMap::new(),
        payload_hash: "unused-by-zti".into(),
    };
    client
        .send_replayable_streaming_request(request, || async { Ok(Body::from("payload")) })
        .await
        .unwrap();
    let requests = task.await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    for (index, request) in requests.iter().enumerate() {
        assert_zti_only(request, index + 1);
        assert!(request.ends_with("payload"));
        // ByteTOS V1 preserves slashes in query values, including opaque upload IDs.
        assert!(request.contains("uploadID=opaque%2B/%3D"));
    }
}

#[tokio::test]
async fn zti_presign_and_form_reject_without_resolving_token() {
    let calls = Arc::new(AtomicUsize::new(0));
    let client = TosClient::new_with_zti_credentials(
        &profile("http://127.0.0.1:1".into()),
        provider(calls.clone()),
    )
    .unwrap();
    let error = client
        .presign_object_url("GET", "bucket", "key", 60)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ZTI"));
    assert!(client.prepare_form_auth().await.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn zti_provider_failure_stops_without_fallback_or_retry() {
    let calls = Arc::new(AtomicUsize::new(0));
    let attempts = calls.clone();
    let provider = ZtiTokenProvider::new(move || {
        attempts.fetch_add(1, Ordering::SeqCst);
        async { Err(ZtiTokenError::Unavailable) }
    });
    let client =
        TosClient::new_with_zti_credentials(&profile("http://127.0.0.1:1".into()), provider)
            .unwrap();
    assert!(matches!(get(&client).await, Err(CliError::AuthFailed(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn zti_does_not_forward_token_on_redirect() {
    let redirected = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let destination = format!("http://{}/leak", redirected.local_addr().unwrap());
    let redirect = format!("HTTP/1.1 302 Found\r\nlocation: {destination}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
    let (endpoint, task) = server(vec![redirect]).await;
    let client =
        TosClient::new_with_zti_credentials(&profile(endpoint), provider(Arc::default())).unwrap();
    assert_eq!(get(&client).await.unwrap().status(), StatusCode::FOUND);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), redirected.accept())
            .await
            .is_err()
    );
    task.await.unwrap();
}

#[tokio::test]
async fn zti_request_keeps_header_sensitive() {
    let client = TosClient::new_with_zti_credentials(
        &profile("http://127.0.0.1:1".into()),
        provider(Arc::default()),
    )
    .unwrap();
    let query = BTreeMap::new();
    let signing = SigningInput {
        method: &Method::GET,
        path: "/bucket/key",
        query: &query,
        payload_hash: "unused",
    };
    let request = client
        .authenticate_request(
            client.http.get("http://127.0.0.1:1/bucket/key"),
            signing,
            BTreeMap::new(),
        )
        .await
        .unwrap()
        .build()
        .unwrap();
    assert!(request.headers()[ZTI_TOKEN_HEADER].is_sensitive());
    assert!(!format!("{request:?}").contains("test-zti-1"));
}

#[tokio::test]
async fn zti_invalid_token_fails_before_network() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let provider = ZtiTokenProvider::new(|| async { Ok("SECRET\r\nInjected: value".into()) });
    let client = TosClient::new_with_zti_credentials(&profile(endpoint), provider).unwrap();
    let error = get(&client).await.unwrap_err();
    assert!(matches!(error, CliError::ValidationError(_)));
    assert!(!error.to_string().contains("SECRET"));
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn zti_auth_rejection_is_not_retried() {
    let (endpoint, task) = server(vec![response("401 Unauthorized")]).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let client =
        TosClient::new_with_zti_credentials(&profile(endpoint), provider(calls.clone())).unwrap();
    let response = get(&client).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(matches!(
        client.check_response(response).await,
        Err(CliError::AuthFailed(_))
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    task.await.unwrap();
}

#[tokio::test]
async fn zti_token_failure_on_retry_clears_terminal_response_id() {
    let (endpoint, task) = server(vec![response("503 Service Unavailable")]).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = ZtiTokenProvider::new(move || {
        let sequence = calls.fetch_add(1, Ordering::SeqCst);
        async move {
            if sequence == 0 {
                Ok("test-token".into())
            } else {
                Err(ZtiTokenError::Expired)
            }
        }
    });
    let trace = Arc::new(ServiceRequestTrace::default());
    let client = TosClient::new_with_zti_credentials_and_request_trace(
        &profile(endpoint),
        provider,
        trace.clone(),
    )
    .unwrap();
    assert!(matches!(get(&client).await, Err(CliError::AuthFailed(_))));
    let snapshot = trace.snapshot();
    assert_eq!(snapshot.response_count, 1);
    assert_eq!(snapshot.request_ids, vec!["zti-test"]);
    assert!(!snapshot.terminal_response_received);
    assert_eq!(snapshot.terminal_response_request_id, None);
    task.await.unwrap();
}

#[tokio::test]
async fn zti_removes_basic_auth_from_url_in_buffered_and_streaming_requests() {
    let (endpoint, task) = server(vec![response("200 OK"), response("200 OK")]).await;
    let client =
        TosClient::new_with_zti_credentials(&profile(endpoint.clone()), provider(Arc::default()))
            .unwrap();
    let destination = endpoint.replacen("http://", "http://user:password@", 1) + "/bucket/key";
    client
        .send_request(
            Method::GET,
            &destination,
            "/bucket/key",
            BTreeMap::new(),
            BTreeMap::new(),
            None,
        )
        .await
        .unwrap();
    client
        .send_streaming_request(
            Method::PUT,
            &destination,
            "/bucket/key",
            BTreeMap::new(),
            BTreeMap::new(),
            "unused".into(),
            Body::from("payload"),
        )
        .await
        .unwrap();
    let requests = task.await.unwrap();
    assert_zti_only(&requests[0], 1);
    assert_zti_only(&requests[1], 2);
}

#[tokio::test]
async fn zti_response_body_retry_reads_current_token() {
    let truncated =
        "HTTP/1.1 200 OK\r\ncontent-length: 10\r\nconnection: close\r\n\r\nshort".to_string();
    let (endpoint, task) = server(vec![truncated, response("200 OK")]).await;
    let client =
        TosClient::new_with_zti_credentials(&profile(endpoint), provider(Arc::default())).unwrap();
    let request = ReplayableRequest {
        method: Method::GET,
        url: client.object_endpoint("bucket", "key").unwrap(),
        path: "/bucket/key".into(),
        query_params: BTreeMap::new(),
        extra_headers: BTreeMap::new(),
        body: None,
    };
    client
        .send_request_with_consumer(request, |response| async move {
            response.bytes().await.map_err(CliError::Http)
        })
        .await
        .unwrap();
    let requests = task.await.unwrap();
    assert_zti_only(&requests[0], 1);
    assert_zti_only(&requests[1], 2);
}

#[tokio::test]
async fn zti_invalid_extra_header_clears_previous_response_trace() {
    let (endpoint, task) = server(vec![response("200 OK")]).await;
    let trace = Arc::new(ServiceRequestTrace::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let client = TosClient::new_with_zti_credentials_and_request_trace(
        &profile(endpoint),
        provider(calls.clone()),
        trace.clone(),
    )
    .unwrap();
    get(&client).await.unwrap();
    task.await.unwrap();
    let error = client
        .send_request(
            Method::GET,
            &client.object_endpoint("bucket", "key").unwrap(),
            "/bucket/key",
            BTreeMap::new(),
            BTreeMap::from([("x-invalid".into(), "SECRET\r\nvalue".into())]),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, CliError::Http(_)));
    assert!(!error.to_string().contains("SECRET"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let snapshot = trace.snapshot();
    assert_eq!(snapshot.response_count, 1);
    assert!(!snapshot.terminal_response_received);
    assert_eq!(snapshot.terminal_response_request_id, None);
}
