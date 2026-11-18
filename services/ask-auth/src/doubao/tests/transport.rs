use super::{parsing::token_json, *};
use crate::login::{ExchangeRequest, ProviderRefreshRequest, RefreshFailure};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

struct Reply {
    status: &'static str,
    content_type: &'static str,
    body: String,
    delay: Duration,
    is_lost: bool,
    is_raw: bool,
    extra_header: String,
}
impl Reply {
    fn json(body: String) -> Self {
        Self {
            status: "200 OK",
            content_type: "application/json; charset=utf-8",
            body,
            delay: Duration::ZERO,
            is_lost: false,
            is_raw: false,
            extra_header: String::new(),
        }
    }
}
type Captures = Arc<Mutex<Vec<String>>>;
struct Server {
    base: String,
    captures: Captures,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn server(replies: Vec<Reply>) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let captures: Captures = Arc::default();
    let recorded = captures.clone();
    let task = tokio::spawn(async move {
        for reply in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            recorded.lock().unwrap().push(request);
            tokio::time::sleep(reply.delay).await;
            if reply.is_lost {
                continue;
            }
            let response = if reply.is_raw {
                reply.body
            } else {
                format!("HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    reply.status, reply.content_type, reply.body.len(), reply.extra_header, reply.body)
            };
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    Server {
        base,
        captures,
        task,
    }
}
async fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = stream.read(&mut chunk).await.unwrap();
        assert!(count > 0 && bytes.len() < 65536);
        bytes.extend_from_slice(&chunk[..count]);
        if is_complete(&bytes) {
            return String::from_utf8(bytes).unwrap();
        }
    }
}
fn is_complete(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    let Some((headers, body)) = text.split_once("\r\n\r\n") else {
        return false;
    };
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    body.len() >= length
}
fn local_adapter(server: &Server) -> DoubaoOAuth {
    let mut oauth = adapter();
    oauth.token_url = format!("{}/token", server.base);
    oauth.userinfo_url = format!("{}/user", server.base);
    oauth.client = client_builder()
        .https_only(false)
        .timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    oauth
}
fn exchange() -> ExchangeRequest {
    ExchangeRequest {
        code: "code with+&=symbols".into(),
        callback_url: CALLBACK.into(),
    }
}
fn userinfo() -> Reply {
    Reply::json(
        r#"{"code":0,"msg":"success","data":{"open_id":"app-user","tenant_key":"tenant"}}"#.into(),
    )
}

#[tokio::test]
async fn exchange_encodes_form_and_verifies_userinfo_once() {
    let server = server(vec![Reply::json(token_json()), userinfo()]).await;
    let tokens = local_adapter(&server).exchange(exchange()).await.unwrap();
    assert_eq!(tokens.identity.user_id, "app-user");
    assert_eq!(tokens.identity.tenant_id.as_deref(), Some("tenant"));
    assert_eq!(tokens.refresh_token, "new-refresh");
    let captures = server.captures.lock().unwrap();
    assert_eq!(captures.len(), 2);
    assert!(captures[0].starts_with("POST /token HTTP/1.1"));
    let form: std::collections::BTreeMap<_, _> =
        url::form_urlencoded::parse(captures[0].split_once("\r\n\r\n").unwrap().1.as_bytes())
            .collect();
    assert_eq!(form.get("code").unwrap(), "code with+&=symbols");
    assert_eq!(form.get("client_secret").unwrap(), "synthetic-secret+&=");
    assert_eq!(form.get("grant_type").unwrap(), "authorization_code");
    assert_eq!(form.get("redirect_uri").unwrap(), CALLBACK);
    assert!(!captures[0].to_lowercase().contains("authorization:"));
    assert!(captures[1].starts_with("GET /user HTTP/1.1"));
    assert!(captures[1]
        .to_lowercase()
        .contains("authorization: bearer new-access"));
    assert!(!captures[1].contains("synthetic-secret"));
}

#[tokio::test]
async fn refresh_rotates_and_never_sends_redirect_or_old_access_token() {
    let server = server(vec![Reply::json(token_json()), userinfo()]).await;
    let result = local_adapter(&server)
        .refresh(ProviderRefreshRequest {
            refresh_token: "old+&=refresh".into(),
        })
        .await;
    assert_eq!(result.ok().unwrap().refresh_token, "new-refresh");
    let captures = server.captures.lock().unwrap();
    assert_eq!(captures.len(), 2);
    let form: std::collections::BTreeMap<_, _> =
        url::form_urlencoded::parse(captures[0].split_once("\r\n\r\n").unwrap().1.as_bytes())
            .collect();
    assert_eq!(form.get("grant_type").unwrap(), "refresh_token");
    assert_eq!(form.get("refresh_token").unwrap(), "old+&=refresh");
    assert!(!form.contains_key("redirect_uri"));
}

#[tokio::test]
async fn redirect_is_denied_without_forwarding_secrets() {
    let destination = server(vec![Reply::json(token_json())]).await;
    let mut redirect = Reply::json(String::new());
    redirect.status = "307 Temporary Redirect";
    redirect.extra_header = format!("Location: {}/stolen\r\n", destination.base);
    let source = server(vec![redirect]).await;
    assert!(local_adapter(&source).exchange(exchange()).await.is_err());
    assert_eq!(source.captures.lock().unwrap().len(), 1);
    assert!(destination.captures.lock().unwrap().is_empty());
}

#[tokio::test]
async fn refresh_loss_timeout_and_platform_errors_are_unknown_without_retry() {
    let mut lost = Reply::json(String::new());
    lost.is_lost = true;
    let mut delayed = Reply::json(token_json());
    delayed.delay = Duration::from_millis(400);
    for reply in [
        lost,
        delayed,
        Reply::json(r#"{"code":20073,"error":"invalid_grant"}"#.into()),
        Reply::json(token_json().replace("\"refresh_token\":\"new-refresh\",", "")),
    ] {
        let server = server(vec![reply]).await;
        let result = local_adapter(&server)
            .refresh(ProviderRefreshRequest {
                refresh_token: "old-refresh".into(),
            })
            .await;
        assert!(matches!(result, Err(RefreshFailure::OutcomeUnknown)));
        assert_eq!(server.captures.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn http_content_type_json_and_body_limits_fail_before_userinfo() {
    let mut status = Reply::json(token_json());
    status.status = "500 Internal Server Error";
    let mut html = Reply::json(token_json());
    html.content_type = "text/html";
    for reply in [
        status,
        html,
        Reply::json("{".into()),
        Reply::json(" ".repeat(65537)),
        Reply::json(token_json().replace("read:user offline_access", "offline_access")),
    ] {
        let server = server(vec![reply]).await;
        assert!(local_adapter(&server).exchange(exchange()).await.is_err());
        assert_eq!(server.captures.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn invalid_userinfo_and_oversized_unknown_fields_fail_closed() {
    for reply in [
        Reply::json(" ".repeat(32769)),
        Reply::json(r#"{"code":0,"data":{"open_id":"u","tenant_key":null}}"#.into()),
        Reply::json(r#"{"code":999,"msg":"denied"}"#.into()),
    ] {
        let server = server(vec![Reply::json(token_json()), reply]).await;
        assert!(local_adapter(&server).exchange(exchange()).await.is_err());
        assert_eq!(server.captures.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn refresh_rejects_reused_rotated_token() {
    let server = server(vec![Reply::json(token_json()), userinfo()]).await;
    let result = local_adapter(&server)
        .refresh(ProviderRefreshRequest {
            refresh_token: "new-refresh".into(),
        })
        .await;
    assert!(matches!(result, Err(RefreshFailure::OutcomeUnknown)));
}

fn raw_reply(response: String) -> Reply {
    let mut reply = Reply::json(response);
    reply.is_raw = true;
    reply
}
fn chunked(body: String) -> Reply {
    raw_reply(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n", body.len(), body))
}

#[tokio::test]
async fn streamed_limits_and_truncated_response_are_rejected() {
    let truncated = raw_reply("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{}".into());
    for reply in [chunked(" ".repeat(TOKEN_BODY_LIMIT + 1)), truncated] {
        let server = server(vec![reply]).await;
        assert!(local_adapter(&server).exchange(exchange()).await.is_err());
        assert_eq!(server.captures.lock().unwrap().len(), 1);
    }
    let server = server(vec![
        Reply::json(token_json()),
        chunked(" ".repeat(USERINFO_BODY_LIMIT + 1)),
    ])
    .await;
    assert!(local_adapter(&server).exchange(exchange()).await.is_err());
    assert_eq!(server.captures.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn credentials_expired_during_userinfo_are_not_released() {
    let mut delayed = userinfo();
    delayed.delay = Duration::from_millis(1100);
    let server = server(vec![
        Reply::json(token_json().replace("3600", "1")),
        delayed,
    ])
    .await;
    let mut oauth = local_adapter(&server);
    oauth.client = client_builder()
        .https_only(false)
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    assert!(matches!(
        oauth.exchange(exchange()).await,
        Err(ProviderError::Contract)
    ));
    assert_eq!(server.captures.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn additive_unknown_data_within_body_limit_is_accepted() {
    let token = token_json().replace(
        "\"code\":0",
        &format!("\"code\":0,\"future\":\"{}\"", "x".repeat(20000)),
    );
    let server = server(vec![chunked(token), userinfo()]).await;
    assert!(local_adapter(&server).exchange(exchange()).await.is_ok());
}

#[tokio::test]
async fn malformed_or_drifted_requests_perform_no_io() {
    let server = server(vec![]).await;
    let oauth = local_adapter(&server);
    let mut request = exchange();
    request.callback_url = "https://attacker.example/callback".into();
    assert!(oauth.exchange(request).await.is_err());
    assert!(oauth
        .exchange(ExchangeRequest {
            code: String::new(),
            callback_url: CALLBACK.into()
        })
        .await
        .is_err());
    assert!(matches!(
        oauth
            .refresh(ProviderRefreshRequest {
                refresh_token: String::new()
            })
            .await,
        Err(RefreshFailure::OutcomeUnknown)
    ));
    assert!(server.captures.lock().unwrap().is_empty());
}
