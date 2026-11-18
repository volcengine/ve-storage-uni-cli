use super::*;
use crate::platform::config::Locator;
use crate::platform::tcc::{read_snapshot, PayloadKey};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};
const META: &str = r#"{"version_codes":[{"confspace":"default","version_code":"42"}]}"#;
const CONTENT: &str = r#"{"version_code":"42","data":{"ask_auth_runtime":"runtime-value"}}"#;
struct Reply {
    bytes: Vec<u8>,
    delay: Duration,
    action: Option<Box<dyn FnOnce() + Send>>,
}
impl Reply {
    fn raw(value: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: value.into(),
            delay: Duration::ZERO,
            action: None,
        }
    }
    fn body(value: &str) -> Self {
        Self::raw(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{value}",
            value.len()
        ))
    }
}
struct Server {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}
impl Server {
    async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            for reply in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 2048];
                while !request.ends_with(b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert!(count > 0 && request.len() < 32 * 1024);
                    request.extend_from_slice(&buffer[..count]);
                }
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                if let Some(action) = reply.action {
                    action();
                }
                tokio::time::sleep(reply.delay).await;
                let _ = stream.write_all(&reply.bytes).await;
            }
        });
        Self {
            addr,
            requests,
            task,
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct FixedResolver(SocketAddr);
impl Resolver for FixedResolver {
    fn resolve<'a>(&'a self, _context: &'a RuntimeContext) -> ResolveFuture<'a> {
        Box::pin(async move { Ok(self.0) })
    }
}
fn runtime() -> RuntimeContext {
    let values: BTreeMap<String, String> = [
        ("PSM", "security.ask_auth"),
        ("RUNTIME_IDC_NAME", "boe"),
        ("SERVICE_ENV", "boe"),
        ("SERVICE_CLUSTER", "sender"),
        ("TCE_STAGE", "canary"),
        ("MY_POD_NAME", "pod-123"),
        ("MY_HOST_IP", "10.0.0.1"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    RuntimeContext::from_map(&values, crate::platform::config::ExpectedEnvironment::Boe)
        .ok()
        .unwrap()
}
fn reader(addr: SocketAddr) -> HttpReader {
    HttpReader {
        http: build_http_client().ok().unwrap(),
        context: runtime(),
        credentials: CredentialSource::from_values(Some("dps-test-token".into()), None)
            .ok()
            .unwrap(),
        resolver: Box::new(FixedResolver(addr)),
    }
}
fn wire(key: &str, value: &str) -> Reply {
    Reply::body(&serde_json::json!({"data":{"key":key,"value":value}}).to_string())
}
fn success() -> Vec<Reply> {
    vec![
        wire("/tcc/v2/meta/security.ask_auth", META),
        wire("/tcc/v2/data/security.ask_auth/default", CONTENT),
    ]
}
fn locator() -> Locator {
    Locator::new(
        "security.ask_auth",
        "default",
        crate::platform::config::ExpectedEnvironment::Boe,
    )
    .ok()
    .unwrap()
}
#[tokio::test]
async fn real_transport_gets_both_exact_resources_with_platform_headers() {
    let server = Server::start(success()).await;
    let snapshot = read_snapshot(&reader(server.addr), &locator())
        .await
        .ok()
        .unwrap();
    assert_eq!(snapshot.required(PayloadKey::Runtime), Ok("runtime-value"));
    let requests = server.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].starts_with("GET /v1/keys/tcc/v2/meta/security.ask_auth HTTP/1.1\r\n"));
    assert!(
        requests[1].starts_with("GET /v1/keys/tcc/v2/data/security.ask_auth/default HTTP/1.1\r\n")
    );
    for request in requests.iter() {
        for header in [
            "x-dps-token: dps-test-token",
            "x-psm: security.ask_auth",
            "x-env: boe",
            "x-cluster: sender",
            "x-stage: canary",
            "x-pod: pod-123",
            "x-host-ip: 10.0.0.1",
            "x-sdk-version: ask-auth/",
        ] {
            assert!(request.contains(header));
        }
        assert!(!request.contains("referer:") && !request.contains("authorization:"));
    }
}
#[tokio::test]
async fn status_failures_and_redirects_are_rejected_without_forwarding_credentials() {
    let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
    for status in [
        "401 Unauthorized",
        "403 Forbidden",
        "500 Error",
        "302 Found",
    ] {
        let reply = Reply::raw(format!("HTTP/1.1 {status}\r\nLocation: http://{}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", destination.local_addr().unwrap()));
        let server = Server::start(vec![reply]).await;
        assert!(matches!(
            read_snapshot(&reader(server.addr), &locator()).await,
            Err(TccError::Unavailable)
        ));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(100), destination.accept())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn malformed_wire_objects_duplicates_and_wrong_resources_fail_closed() {
    for body in [
        "not-json",
        "[]",
        "null",
        r#"{"data":[]}"#,
        r#"{"data":{"key":"a","value":"b","key":"a"}}"#,
        r#"{"data":{"key":"a","value":"b","value":"b"}}"#,
        r#"{"data":{},"data":{}}"#,
        r#"{"data":{"key":"/wrong/resource","value":"{}"}}"#,
    ] {
        let server = Server::start(vec![Reply::body(body), wire("unused", CONTENT)]).await;
        assert!(matches!(
            read_snapshot(&reader(server.addr), &locator()).await,
            Err(TccError::InvalidSnapshot)
        ));
    }
}
#[tokio::test]
async fn transport_rejects_deep_unknown_wire_subtrees_before_serde() {
    let deep = format!("{}0{}", "[".repeat(10_000), "]".repeat(10_000));
    let body = format!(
        r#"{{"data":{{"key":"/tcc/v2/meta/security.ask_auth","value":"{{}}"}},"unknown":{deep}}}"#
    );
    let server = Server::start(vec![Reply::body(&body)]).await;
    assert!(matches!(
        reader(server.addr)
            .read("/tcc/v2/meta/security.ask_auth")
            .await,
        Err(TccError::InvalidSnapshot)
    ));
}
#[tokio::test]
async fn transport_bounds_announced_and_chunked_body_and_rejects_truncation() {
    let oversized = Reply::raw(format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        MAX_WIRE_BYTES + 1
    ));
    let mut chunked = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n",
        MAX_WIRE_BYTES + 1
    )
    .into_bytes();
    chunked.resize(chunked.len() + MAX_WIRE_BYTES + 1, b' ');
    chunked.extend_from_slice(b"\r\n0\r\n\r\n");
    let truncated =
        Reply::raw("HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}");
    for reply in [oversized, Reply::raw(chunked), truncated] {
        let server = Server::start(vec![reply]).await;
        assert!(reader(server.addr)
            .read("/tcc/v2/meta/security.ask_auth")
            .await
            .is_err());
    }
}
#[tokio::test]
async fn delayed_http_response_is_bounded_and_never_retried() {
    let mut replies = success();
    replies[0].delay = Duration::from_secs(2);
    let server = Server::start(replies).await;
    let started = tokio::time::Instant::now();
    assert!(reader(server.addr)
        .read("/tcc/v2/meta/security.ask_auth")
        .await
        .is_err());
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert_eq!(server.requests.lock().unwrap().len(), 1);
}
struct PendingResolver;
impl Resolver for PendingResolver {
    fn resolve<'a>(&'a self, _context: &'a RuntimeContext) -> ResolveFuture<'a> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test]
async fn discovery_is_bounded_within_snapshot_deadline() {
    let mut client = reader("127.0.0.1:1".parse().unwrap());
    client.resolver = Box::new(PendingResolver);
    let started = tokio::time::Instant::now();
    assert!(client.read("/tcc/v2/meta/security.ask_auth").await.is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
}
#[tokio::test]
async fn transport_has_no_snapshot_or_version_cache() {
    let mut replies = success();
    replies.extend(success());
    replies.push(Reply::raw(
        "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    ));
    let server = Server::start(replies).await;
    let client = reader(server.addr);
    assert!(read_snapshot(&client, &locator()).await.is_ok());
    assert!(read_snapshot(&client, &locator()).await.is_ok());
    assert!(read_snapshot(&client, &locator()).await.is_err());
    assert_eq!(server.requests.lock().unwrap().len(), 5);
}
#[tokio::test]
#[cfg(unix)]
async fn credential_file_change_or_disappearance_during_http_discards_response() {
    for is_removed in [false, true] {
        let path = std::env::temp_dir().join(format!("ask-auth-http-dps-{}", uuid::Uuid::now_v7()));
        std::fs::write(&path, b"original-token").unwrap();
        let changed_path = path.clone();
        let mut replies = success();
        replies[0].action = Some(Box::new(move || {
            if is_removed {
                std::fs::remove_file(&changed_path).unwrap();
            } else {
                std::fs::write(&changed_path, b"rotated-token").unwrap();
            }
        }));
        let server = Server::start(replies).await;
        let mut client = reader(server.addr);
        client.credentials = CredentialSource::from_values(None, Some(path.clone()))
            .ok()
            .unwrap();
        assert!(matches!(
            read_snapshot(&client, &locator()).await,
            Err(TccError::Identity)
        ));
        let _ = std::fs::remove_file(path);
    }
}

#[tokio::test]
#[cfg(unix)]
async fn symlink_retarget_discards_current_response_and_fresh_snapshot_recovers() {
    use std::os::unix::fs::symlink;
    let directory = std::env::temp_dir().join(format!("ask-auth-symlink-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&directory).unwrap();
    let initial = directory.join("initial");
    let replacement = directory.join("replacement");
    let link = directory.join("current");
    std::fs::write(&initial, b"same-token").unwrap();
    std::fs::write(&replacement, b"same-token").unwrap();
    symlink(&initial, &link).unwrap();
    let switched_link = link.clone();
    let mut replies = vec![wire("/tcc/v2/meta/security.ask_auth", META)];
    replies[0].action = Some(Box::new(move || {
        std::fs::remove_file(&switched_link).unwrap();
        symlink(&replacement, &switched_link).unwrap();
    }));
    replies.extend(success());
    let server = Server::start(replies).await;
    let mut client = reader(server.addr);
    client.credentials = CredentialSource::from_values(None, Some(link))
        .ok()
        .unwrap();
    assert!(matches!(
        read_snapshot(&client, &locator()).await,
        Err(TccError::Identity)
    ));
    assert!(read_snapshot(&client, &locator()).await.is_ok());
    assert_eq!(server.requests.lock().unwrap().len(), 3);
    std::fs::remove_dir_all(directory).unwrap();
}
