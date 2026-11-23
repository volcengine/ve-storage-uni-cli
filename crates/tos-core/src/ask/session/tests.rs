use super::*;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const PROMPT: &str = "如何配置存储？";

fn created() -> String {
    json!({"code":0,"data":{"id":"session-1","status":"in_progress","messages":[{
        "id":"user-1","session_id":"session-1","role":"user","status":"completed",
        "items":[{"id":"input-1","type":"text","seq":1,"text":PROMPT}]
    }]}})
    .to_string()
}

fn lifecycle(kind: &str, seq: u64) -> String {
    let status = if kind == "message.completed" {
        "completed"
    } else {
        "in_progress"
    };
    frame(
        kind,
        json!({"type":kind,"event_id":format!("event-{seq}"),"seq":seq,
        "session_id":"session-1","message_id":"assistant-1","message":{
            "id":"assistant-1","role":"assistant","status":status,"reply_id":"user-1"
        }}),
    )
}

fn frame(kind: &str, body: Value) -> String {
    let id = body
        .get("seq")
        .map(|seq| format!("id: assistant-1:{seq}\n"))
        .unwrap_or_default();
    format!("{id}event: {kind}\ndata: {body}\n\n")
}

fn text(id: &str, seq: u64, text: &str) -> String {
    frame(
        "text",
        json!({"id":id,"type":"text","seq":seq,
        "message_id":"assistant-1","text":text}),
    )
}

fn complete() -> String {
    lifecycle("message.in_progress", 1)
        + &text("item-1", 3, "配置说明。")
        + &lifecycle("message.completed", 4)
}

#[derive(Clone)]
struct Response {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    is_chunked: bool,
    delay: Duration,
}

impl Response {
    fn json(body: String) -> Self {
        Self {
            status: 201,
            content_type: "application/json",
            body: body.into_bytes(),
            is_chunked: false,
            delay: Duration::ZERO,
        }
    }
    fn sse(body: String) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream; charset=utf-8",
            body: body.into_bytes(),
            is_chunked: true,
            delay: Duration::ZERO,
        }
    }
}

struct Fixture {
    client: SessionClient,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn fixture(responses: Vec<Response>, timeout: Duration) -> Fixture {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/open-apis/agent", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let capture = requests.clone();
    let task = tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_request(&mut socket).await;
            capture.lock().unwrap().push(request);
            tokio::time::sleep(response.delay).await;
            if write_response(&mut socket, response).await.is_err() {
                return;
            }
        }
        // Keep accepting to detect forbidden redirect/retry/extra session creation.
        while let Ok((mut socket, _)) = listener.accept().await {
            let request = read_request(&mut socket).await;
            capture.lock().unwrap().push(request);
            let _ = socket
                .write_all(b"HTTP/1.1 500 Error\r\nContent-Length: 0\r\n\r\n")
                .await;
        }
    });
    Fixture {
        client: SessionClient::for_test(base, timeout),
        requests,
        task,
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = socket.read(&mut chunk).await.unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .map(|value| value.parse::<usize>().unwrap())
                .unwrap_or(0);
            if bytes.len() >= end + 4 + length {
                return bytes;
            }
        }
    }
}

async fn write_response(
    socket: &mut tokio::net::TcpStream,
    reply: Response,
) -> std::io::Result<()> {
    let framing = if reply.is_chunked {
        "Transfer-Encoding: chunked".into()
    } else {
        format!("Content-Length: {}", reply.body.len())
    };
    socket.write_all(format!("HTTP/1.1 {} Reply\r\nContent-Type: {}\r\n{framing}\r\nConnection: close\r\nLocation: /redirected\r\n\r\n", reply.status, reply.content_type).as_bytes()).await?;
    if !reply.is_chunked {
        return socket.write_all(&reply.body).await;
    }
    for chunk in reply.body.chunks(7) {
        socket
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await?;
        socket.write_all(chunk).await?;
        socket.write_all(b"\r\n").await?;
    }
    socket.write_all(b"0\r\n\r\n").await
}

async fn ask(fixture: &Fixture) -> Result<Reply> {
    fixture
        .client
        .ask(&Secret::new("borrowed-user-token".into()), PROMPT)
        .await
}

#[tokio::test]
async fn creates_exactly_one_uuid_v7_session_and_streams_by_user_message() {
    let fixture = fixture(
        vec![Response::json(created()), Response::sse(complete())],
        Duration::from_secs(2),
    )
    .await;
    let reply = ask(&fixture).await.unwrap();
    assert_eq!(reply.session_id, "session-1");
    assert_eq!(reply.user_message_id, "user-1");
    assert_eq!(reply.text, "配置说明。");
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let post = String::from_utf8(requests[0].clone()).unwrap();
    let get = String::from_utf8(requests[1].clone()).unwrap();
    assert!(post.starts_with("POST /open-apis/agent/v1/sessions HTTP/1.1\r\n"));
    assert!(post
        .to_lowercase()
        .contains("authorization: bearer borrowed-user-token\r\n"));
    assert!(post
        .to_lowercase()
        .contains("content-type: application/json\r\n"));
    assert!(
        get.starts_with("GET /open-apis/agent/v1/sessions/session-1/events/user-1 HTTP/1.1\r\n")
    );
    assert!(get.to_lowercase().contains("accept: text/event-stream\r\n"));
    assert!(get
        .to_lowercase()
        .contains("authorization: bearer borrowed-user-token\r\n"));
    let body: Value = serde_json::from_str(post.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let uuid = body["uuid"].as_str().unwrap();
    assert_eq!(uuid.len(), 36);
    assert_eq!(&uuid[14..15], "7");
    assert!(["8", "9", "a", "b"].contains(&&uuid[19..20]));
    let mut expected = json!({"uuid":uuid,"name":"Storage CLI assistance","initial_events":[{
        "type":"user.message","contents":[{"type":"text","text":PROMPT}]
    }]});
    assert_eq!(body, expected.take());
}

#[tokio::test]
async fn sse_handles_chunks_crlf_multiline_comments_and_unicode() {
    let multiline = "event: text\ndata: {\ndata: \"id\":\"item-1\",\"type\":\"text\",\"seq\":2,\n\
        data: \"message_id\":\"assistant-1\",\"text\":\"中文🙂\"}\n\n";
    let stream = (": heartbeat\n\n".to_owned()
        + &lifecycle("message.created", 1)
        + multiline
        + &lifecycle("message.completed", 3))
        .replace('\n', "\r\n");
    let fixture = fixture(
        vec![Response::json(created()), Response::sse(stream)],
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(ask(&fixture).await.unwrap().text, "中文🙂");
}

#[tokio::test]
async fn committed_items_replace_previews_deduplicate_and_sort_by_sequence() {
    let preview = frame(
        "text.delta",
        json!({"id":"item-1","message_id":"assistant-1","delta":"preview"}),
    );
    let reasoning = frame(
        "reasoning",
        json!({"id":"thought-1","type":"reasoning","seq":2,"message_id":"assistant-1","summary":["private thought"]}),
    );
    let stream = lifecycle("message.in_progress", 1)
        + &preview
        + &reasoning
        + &text("item-2", 4, "second")
        + &text("item-1", 3, "first")
        + &text("item-1", 3, "first")
        + &lifecycle("message.completed", 5);
    let fixture = fixture(
        vec![Response::json(created()), Response::sse(stream)],
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(ask(&fixture).await.unwrap().text, "firstsecond");
}

#[tokio::test]
async fn rejects_malformed_or_contradictory_creation_without_opening_stream() {
    for body in [
        "bad sensitive provider body".into(),
        created().replace("\"code\":0", "\"code\":9"),
        created().replace("\"id\":\"session-1\"", "\"id\":\"..\""),
        created().replace("\"id\":\"user-1\"", "\"id\":\"\""),
        created().replace("\"session_id\":\"session-1\"", "\"session_id\":\"wrong\""),
        created().replace("\"role\":\"user\"", "\"role\":\"assistant\""),
        created().replace(PROMPT, "different prompt"),
        created().replace("\"messages\":[", "\"messages\":[],\"unused\":["),
    ] {
        let fixture = fixture(vec![Response::json(body)], Duration::from_secs(2)).await;
        let error = ask(&fixture).await.err().unwrap();
        assert!(!format!("{error:?} {error}").contains("sensitive provider body"));
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn rejects_incomplete_malformed_mismatched_or_failed_streams() {
    let streams = [
        String::new(),
        lifecycle("message.in_progress", 1),
        complete().trim_end().to_string(),
        complete().replace("data: {", "data: invalid {"),
        complete().replace("\"session_id\":\"session-1\"", "\"session_id\":\"other\""),
        complete().replace("\"message_id\":\"assistant-1\"", "\"message_id\":\"other\""),
        complete().replace("\"role\":\"assistant\"", "\"role\":\"user\""),
        complete().replace("\"status\":\"completed\"", "\"status\":\"in_progress\""),
        complete().replace("message.completed", "message.failed"),
        complete().replace("message.completed", "message.cancelled"),
        complete().replace("message.completed", "session.failed"),
        complete().replace("message.completed", "error"),
        complete().replace("id: assistant-1:3", "id: different:3"),
        text("item-1", 3, "unanchored") + &lifecycle("message.completed", 4),
        lifecycle("message.in_progress", 1)
            + &text("item-1", 2, "first")
            + &text("item-1", 3, "contradiction")
            + &lifecycle("message.completed", 4),
        lifecycle("message.in_progress", 1)
            + &text("item-1", 2, "first")
            + &text("item-2", 2, "collision")
            + &lifecycle("message.completed", 4),
    ];
    for stream in streams {
        let fixture = fixture(
            vec![Response::json(created()), Response::sse(stream.clone())],
            Duration::from_secs(2),
        )
        .await;
        assert!(ask(&fixture).await.is_err(), "accepted {stream}");
        assert_eq!(fixture.requests.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn rejects_tool_and_artifact_frames_even_after_completion() {
    for kind in ["tool_use", "tool_result", "artifact"] {
        let unsupported = frame(
            kind,
            json!({"type":kind,"id":"tool-1","seq":5,"message_id":"assistant-1"}),
        );
        let fixture = fixture(
            vec![
                Response::json(created()),
                Response::sse(complete() + &unsupported),
            ],
            Duration::from_secs(2),
        )
        .await;
        assert!(ask(&fixture).await.is_err());
    }
}

#[tokio::test]
async fn rejects_redirects_and_wrong_content_types_without_following() {
    for status in [301, 302, 307, 308] {
        let mut response = Response::json(created());
        response.status = status;
        let fixture = fixture(vec![response], Duration::from_secs(2)).await;
        assert!(ask(&fixture).await.is_err());
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
    for content_type in ["application/json", "text/html", "text/event-stream-bogus"] {
        let mut stream = Response::sse(complete());
        stream.content_type = content_type;
        let fixture = fixture(
            vec![Response::json(created()), stream],
            Duration::from_secs(2),
        )
        .await;
        assert!(ask(&fixture).await.is_err());
    }
}

#[tokio::test]
async fn bounds_body_frame_count_frame_size_and_heartbeat_bytes() {
    for response in [
        Response::json(" ".repeat(MAX_CREATE_BYTES + 1)),
        Response::sse(":".to_owned() + &"x".repeat(70 * 1024) + "\n\n"),
        Response::sse(": heartbeat\n\n".repeat(5000)),
        Response::sse((":".to_owned() + &"x".repeat(32 * 1024) + "\n\n").repeat(70)),
    ] {
        let replies = if response.content_type == "application/json" {
            vec![response]
        } else {
            vec![Response::json(created()), response]
        };
        let fixture = fixture(replies, Duration::from_secs(2)).await;
        assert!(ask(&fixture).await.is_err());
    }
}

#[tokio::test]
async fn uncertain_post_is_not_retried_and_does_not_open_stream() {
    let mut response = Response::json(created());
    response.delay = Duration::from_millis(150);
    let fixture = fixture(vec![response], Duration::from_millis(30)).await;
    assert!(ask(&fixture).await.is_err());
    tokio::time::sleep(Duration::from_millis(180)).await;
    assert_eq!(fixture.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn stream_operation_has_deadline_without_reconnect_or_new_post() {
    let mut stream = Response::sse(complete());
    stream.delay = Duration::from_millis(150);
    let fixture = fixture(
        vec![Response::json(created()), stream],
        Duration::from_millis(30),
    )
    .await;
    assert!(ask(&fixture).await.is_err());
    tokio::time::sleep(Duration::from_millis(180)).await;
    assert_eq!(fixture.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn rejects_invalid_inputs_before_any_request() {
    let fixture = fixture(Vec::new(), Duration::from_secs(2)).await;
    for prompt in ["", " \n\t", &"x".repeat(70 * 1024)] {
        assert!(fixture
            .client
            .ask(&Secret::new("token".into()), prompt)
            .await
            .is_err());
    }
    for token in ["", "bad\r\ntoken", &"x".repeat(20 * 1024)] {
        assert!(fixture
            .client
            .ask(&Secret::new(token.into()), PROMPT)
            .await
            .is_err());
    }
    assert!(fixture.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn accepts_ignorable_sse_tails_after_completed_reply() {
    for tail in [
        ": heartbeat\n",
        ": heartbeat",
        "unknown: ignored\n",
        "unknown: ignored",
        "event: text\n",
        "event: text",
    ] {
        let fixture = fixture(
            vec![Response::json(created()), Response::sse(complete() + tail)],
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(ask(&fixture).await.unwrap().text, "配置说明。");
    }
}

#[tokio::test]
async fn rejects_pending_data_after_completed_reply() {
    for tail in ["data: {", "data: {}\n", "event: text\ndata: {}\n"] {
        let fixture = fixture(
            vec![Response::json(created()), Response::sse(complete() + tail)],
            Duration::from_secs(2),
        )
        .await;
        assert!(ask(&fixture).await.is_err(), "accepted pending tail {tail}");
    }
}

#[tokio::test]
async fn accepts_maximum_prompt_with_larger_echoed_create_response() {
    let prompt = "x".repeat(MAX_PROMPT_BYTES);
    let mut creation: Value = serde_json::from_str(&created()).unwrap();
    creation["data"]["messages"][0]["items"][0]["text"] = json!(prompt);
    let body = creation.to_string();
    assert!(body.len() > 64 * 1024);
    let fixture = fixture(
        vec![Response::json(body), Response::sse(complete())],
        Duration::from_secs(2),
    )
    .await;
    let reply = fixture
        .client
        .ask(&Secret::new("borrowed-user-token".into()), &prompt)
        .await
        .unwrap();
    assert_eq!(reply.text, "配置说明。");
    assert_eq!(fixture.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn distinguishes_expired_credentials_permissions_and_quota_without_replay() {
    for (status, expected) in [
        (401, "Doubao access was denied; login may be required"),
        (
            403,
            "Doubao permission was denied; verify application permissions and user scopes",
        ),
        (
            429,
            "Doubao rate limit was reached; the question was not retried",
        ),
    ] {
        let mut response = Response::json("redacted-provider-body".into());
        response.status = status;
        let fixture = fixture(vec![response], Duration::from_secs(2)).await;
        let failure = ask(&fixture).await.err().expect("request should fail");
        assert_eq!(failure.to_string(), expected);
        assert_eq!(fixture.requests.lock().unwrap().len(), 1);
    }
}
