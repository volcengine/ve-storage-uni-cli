use super::{ConsulLookupClient, ConsulLookupRequest};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn lookup_request() -> ConsulLookupRequest {
    ConsulLookupRequest {
        psm: "example.tcc.service".into(),
        idc: Some("boe".into()),
        cluster: Some("default".into()),
        addr_family: Some(super::AddrFamily::V6),
    }
}

async fn serve_once(response: String) -> (ConsulLookupClient, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test Consul");
    let address = listener.local_addr().expect("test address");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept lookup");
        let mut request = vec![0; 8192];
        let length = socket.read(&mut request).await.expect("read lookup");
        if let Err(error) = socket.write_all(response.as_bytes()).await {
            assert!(
                matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                ),
                "reply lookup: {error}"
            );
        }
        String::from_utf8(request[..length].to_vec()).expect("HTTP request")
    });
    let mut client = bounded_client(Duration::from_secs(2));
    client.base_url = format!("http://{address}/v1/lookup/name");
    (client, server)
}

async fn lookup_body(
    body: &str,
) -> Result<Vec<super::WeightedEndpoint>, crate::agent::error::CliError> {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (client, server) = serve_once(response).await;
    let result = client.lookup(lookup_request()).await;
    server.await.expect("Consul test server");
    result
}

#[tokio::test]
async fn lookup_rejects_empty_endpoints() {
    assert!(lookup_body("[]").await.is_err());
}

#[tokio::test]
async fn lookup_rejects_zero_port() {
    assert!(lookup_body(r#"[{"Host":"127.0.0.1","Port":0}]"#)
        .await
        .is_err());
}

#[tokio::test]
async fn lookup_rejects_unusable_ips() {
    for host in ["0.0.0.0", "::", "224.0.0.1", "ff02::1", "255.255.255.255"] {
        let body = format!(r#"[{{"Host":"{host}","Port":8080}}]"#);
        assert!(lookup_body(&body).await.is_err(), "host {host}");
    }
}

#[tokio::test]
async fn lookup_rejects_oversized_body_before_json_decode() {
    let body = format!("[{}]", " ".repeat(1024 * 1024));
    assert!(lookup_body(&body).await.is_err());
}

#[tokio::test]
async fn lookup_rejects_too_many_endpoints() {
    let endpoint = r#"{"Host":"127.0.0.1","Port":8080}"#;
    let body = format!("[{}]", vec![endpoint; 1025].join(","));
    assert!(lookup_body(&body).await.is_err());
}

fn bounded_client(timeout: Duration) -> ConsulLookupClient {
    let _guard = super::tests::DISCOVERY_ENV_LOCK
        .lock()
        .expect("discovery env lock");
    ConsulLookupClient::from_env_with_timeout(timeout).expect("bounded lookup client")
}

#[tokio::test]
async fn direct_lookup_encodes_parameters_and_returns_weighted_ipv6() {
    let body = r#"[{"Host":"::1","Port":8080,"Tags":{"weight":"42"}}]"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let (client, server) = serve_once(response).await;
    let mut request = lookup_request();
    request.psm = "example.tcc&inject=bad".into();
    request.idc = Some("boe&extra=bad".into());
    request.cluster = Some("default & blue".into());
    let endpoints = client.lookup(request).await.expect("direct lookup");
    assert_eq!(endpoints[0].addr, "[::1]:8080".parse().unwrap());
    assert_eq!(endpoints[0].weight, 42);
    let wire = server.await.expect("Consul server");
    let target = wire
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap();
    let url = url::Url::parse(&format!("http://localhost{target}")).unwrap();
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(url.path(), "/v1/lookup/name");
    assert_eq!(
        query.get("name").unwrap(),
        "example.tcc&inject=bad.service.boe&extra=bad"
    );
    assert_eq!(query.get("cluster").unwrap(), "default & blue");
    assert_eq!(query.get("addr-family").unwrap(), "v6");
    assert!(!query.contains_key("inject"));
    assert!(!query.contains_key("extra"));
    assert!(!wire.to_ascii_lowercase().contains("authorization:"));
    assert!(!wire.to_ascii_lowercase().contains("x-tt-token:"));
}

#[tokio::test]
async fn lookup_rejects_invalid_endpoints_and_malformed_json() {
    for body in [
        r#"[{"Host":"localhost","Port":8080}]"#,
        r#"[{"Host":"127.0.0.1","Port":65536}]"#,
        "not-json",
    ] {
        assert!(lookup_body(body).await.is_err(), "body {body}");
    }
}

#[tokio::test]
async fn lookup_accepts_endpoint_and_body_limits() {
    let endpoint = r#"{"Host":"127.0.0.1","Port":8080}"#;
    let body = format!("[{}]", vec![endpoint; 1024].join(","));
    assert_eq!(lookup_body(&body).await.unwrap().len(), 1024);
    let mut body = format!("[{endpoint}]");
    body.extend(std::iter::repeat_n(' ', 1024 * 1024 - body.len()));
    assert_eq!(lookup_body(&body).await.unwrap().len(), 1);
}

#[tokio::test]
async fn lookup_rejects_oversized_chunked_body() {
    let body = " ".repeat(1024 * 1024 + 1);
    let response = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
        body.len()
    );
    let (client, server) = serve_once(response).await;
    let error = client.lookup(lookup_request()).await.unwrap_err();
    assert!(error.to_string().contains("1 MiB"));
    server.await.expect("Consul server");
}

#[tokio::test]
async fn lookup_rejects_non_success_and_redirect_without_following() {
    for status in ["500 Internal Server Error", "302 Found"] {
        let response =
            format!("HTTP/1.1 {status}\r\nLocation: /redirected\r\nContent-Length: 0\r\n\r\n");
        let (client, server) = serve_once(response).await;
        let error = client.lookup(lookup_request()).await.unwrap_err();
        assert!(error.to_string().contains(&status[..3]), "error {error}");
        server.await.expect("Consul server");
    }
}

#[tokio::test]
async fn bounded_lookup_timeout_covers_response_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 8192];
        // [Review Fix #93] Confirm request arrival before exercising a stalled response body.
        let request_length = socket.read(&mut request).await.unwrap();
        assert!(
            request_length > 0,
            "lookup request must arrive before response"
        );
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
    });
    let mut client = bounded_client(Duration::from_millis(30));
    client.base_url = format!("http://{address}/v1/lookup/name");
    let error = tokio::time::timeout(Duration::from_secs(1), client.lookup(lookup_request()))
        .await
        .expect("bounded timeout")
        .unwrap_err();
    assert!(matches!(error, crate::agent::error::CliError::Http(error) if error.is_timeout()));
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[test]
fn bounded_constructor_rejects_zero_timeout() {
    assert!(ConsulLookupClient::from_env_with_timeout(Duration::ZERO).is_err());
}

#[test]
fn weighted_selection_preserves_zero_weight_and_empty_semantics() {
    assert_eq!(super::choose_weighted_addr(&[]), None);
    let endpoint = super::WeightedEndpoint {
        addr: "127.0.0.1:8080".parse().unwrap(),
        weight: 0,
    };
    assert_eq!(
        super::choose_weighted_addr(std::slice::from_ref(&endpoint)),
        Some(endpoint.addr)
    );
}

#[tokio::test]
async fn direct_lookup_rejects_empty_or_whitespace_psm_before_http() {
    let mut client = bounded_client(Duration::from_millis(500));
    client.base_url = "invalid-url-would-fail-if-http-were-attempted".into();
    for psm in ["", " ", "\t", "\n"] {
        let mut request = lookup_request();
        request.psm = psm.into();
        let error = client.lookup(request).await.unwrap_err();
        assert!(error.to_string().contains("Consul PSM must be nonempty"));
    }
}
