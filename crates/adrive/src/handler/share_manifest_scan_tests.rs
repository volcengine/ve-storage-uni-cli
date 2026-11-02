//! Loopback fixtures prove directory request bounds and serial marker continuation.
use super::*;
use crate::domain::share_access::fixture::{read_request, reply};
use serde_json::json;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tos_core::agent::global_args::GlobalArgs;

#[derive(Default)]
struct State {
    children: usize,
    is_expiring: bool,
    active: AtomicUsize,
    peak: AtomicUsize,
    tokens: AtomicUsize,
    requests: Mutex<Vec<(String, String, String)>>,
}

struct Server {
    endpoint: String,
    state: Arc<State>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Server {
    fn new(children: usize, is_expiring: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let state = Arc::new(State {
            children,
            is_expiring,
            ..Default::default()
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (server_state, server_stop) = (state.clone(), stop.clone());
        let worker = thread::spawn(move || serve(listener, server_state, server_stop));
        Self {
            endpoint,
            state,
            stop,
            worker: Some(worker),
        }
    }

    async fn receiver(&self) -> ShareReceiver {
        let global = GlobalArgs::default();
        let mut receiver =
            ShareReceiver::new(&global, &format!("{}/share/s", self.endpoint)).unwrap();
        let info = receiver.info().await.unwrap();
        receiver.authorize(&global, &info, false).await.unwrap();
        receiver
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Err(failure) = self.worker.take().unwrap().join() {
            if !thread::panicking() {
                std::panic::resume_unwind(failure);
            }
        }
    }
}

fn serve(listener: TcpListener, state: Arc<State>, stop: Arc<AtomicBool>) {
    let mut handlers = Vec::new();
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let state = state.clone();
                handlers.push(thread::spawn(move || respond(stream, state)));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(2))
            }
            Err(error) => panic!("fixture listener failed: {error}"),
        }
    }
    for handler in handlers {
        handler.join().unwrap();
    }
}

fn respond(mut stream: TcpStream, state: Arc<State>) {
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let request = read_request(&mut stream);
    assert!(!request.to_ascii_lowercase().contains("authorization:"));
    let target = request
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap();
    let url = url::Url::parse(&format!("http://fixture{target}")).unwrap();
    let response = match url.path() {
        "/v1/share_link/get_by_anonymous" => json!({"ShareID":"s"}),
        "/v1/share_link/get_access_token" => {
            let index = state.tokens.fetch_add(1, Ordering::SeqCst);
            json!({"AccessToken":format!("token-{index}"),"ExpiresIn":if state.is_expiring && index == 0 {1} else {60}})
        }
        "/v1/share_file/get" => json!({"FilePath":"root/","FileType":"folder"}),
        "/v1/share_file/list" => listing_response(&state, &request, &url),
        other => panic!("unexpected fixture target: {other}"),
    };
    stream
        .write_all(reply(200, &response.to_string()).as_bytes())
        .unwrap();
}

fn listing_response(state: &State, request: &str, url: &url::Url) -> serde_json::Value {
    let query = url.query_pairs().collect::<BTreeMap<_, _>>();
    let prefix = query
        .get("prefix")
        .map(|value| value.as_ref())
        .unwrap_or("");
    let marker = query
        .get("marker")
        .map(|value| value.as_ref())
        .unwrap_or("");
    let token = request
        .lines()
        .find_map(|line| line.strip_prefix("x-share-token: "))
        .unwrap()
        .to_owned();
    state
        .requests
        .lock()
        .unwrap()
        .push((prefix.into(), marker.into(), token));
    let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.peak.fetch_max(active, Ordering::SeqCst);
    let delay = if state.is_expiring && prefix == "root/" && marker.is_empty() {
        1100
    } else {
        80
    };
    thread::sleep(Duration::from_millis(delay));
    let response = directory_page(state.children, prefix, marker);
    state.active.fetch_sub(1, Ordering::SeqCst);
    response
}

fn directory_page(children: usize, prefix: &str, marker: &str) -> serde_json::Value {
    if children > 0 && prefix == "root/" {
        return json!({"Folders":(0..children).map(|index|json!({"Folder":format!("root/sub{index}/")})).collect::<Vec<_>>()});
    }
    if marker.is_empty() {
        json!({"Files":[{"FilePath":format!("{prefix}a.txt"),"Size":1}],"NextMarker":"next","IsTruncated":true})
    } else {
        assert_eq!(marker, "next");
        json!({"Files":[{"FilePath":format!("{prefix}b.txt"),"Size":1}]})
    }
}

#[tokio::test]
async fn directories_reach_default_or_configured_concurrency_and_each_prefix_stays_serial() {
    for (concurrency, expected) in [(None, 4), (Some(2), 2), (Some(1), 1)] {
        let server = Server::new(6, false);
        let args = crate::cli::share::ShareDownloadArgs {
            path: Some("root/".into()),
            is_recursive: true,
            list_concurrency: concurrency,
            ..Default::default()
        };
        let manifest =
            super::super::enumerate_with_options(&mut server.receiver().await, &args, None)
                .await
                .unwrap();
        assert_eq!(manifest.files.len(), 12);
        assert_eq!(server.state.peak.load(Ordering::SeqCst), expected);
        let requests = server.state.requests.lock().unwrap();
        for index in 0..6 {
            let markers = requests
                .iter()
                .filter(|(prefix, _, _)| prefix == &format!("root/sub{index}/"))
                .map(|(_, marker, _)| marker.as_str())
                .collect::<Vec<_>>();
            assert_eq!(markers, ["", "next"]);
        }
    }
}

#[tokio::test]
async fn expired_session_is_renewed_between_pages_of_one_prefix() {
    let server = Server::new(0, true);
    let args = crate::cli::share::ShareDownloadArgs {
        path: Some("root/".into()),
        is_recursive: true,
        ..Default::default()
    };
    let manifest = super::super::enumerate_with_options(&mut server.receiver().await, &args, None)
        .await
        .unwrap();
    assert_eq!(manifest.files.len(), 2);
    assert_eq!(server.state.tokens.load(Ordering::SeqCst), 2);
    let requests = server.state.requests.lock().unwrap();
    assert_eq!(requests[0].2, "token-0");
    assert_eq!(requests[1].2, "token-1");
}

#[tokio::test]
async fn zero_concurrency_is_rejected_before_the_receiver_needs_authorization() {
    let mut receiver =
        ShareReceiver::new(&GlobalArgs::default(), "https://example.test/share/s").unwrap();
    let args = crate::cli::share::ShareDownloadArgs {
        list_concurrency: Some(0),
        ..Default::default()
    };
    assert!(
        super::super::enumerate_with_options(&mut receiver, &args, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("list-concurrency")
    );
}

#[test]
fn markers_and_scope_checks_reject_cycles_controls_and_traversal() {
    let mut markers = BTreeSet::new();
    let page: ListFilesOutput =
        serde_json::from_value(json!({"NextMarker":"next","IsTruncated":true})).unwrap();
    assert_eq!(
        next_marker(&page, &mut markers).unwrap().as_deref(),
        Some("next")
    );
    assert!(next_marker(&page, &mut markers).is_err());
    for payload in [
        json!({"NextMarker":"bad\nmarker"}),
        json!({"IsTruncated":true}),
    ] {
        let page = serde_json::from_value(payload).unwrap();
        assert!(next_marker(&page, &mut BTreeSet::new()).is_err());
    }
    for path in ["other/file", "root/../file", "root/sub/../../file"] {
        let page = serde_json::from_value(json!({"Files":[{"FilePath":path,"Size":1}]})).unwrap();
        assert!(validate_scope(&page, Some("root/")).is_err());
    }
}

#[test]
fn folder_scope_checks_accept_canonical_self_markers_without_weakening_file_scope() {
    let folder: ListFilesOutput = serde_json::from_value(json!({
        "Folders":[{"Folder":"root"}],
        "Files":[{"FilePath":"root","FileType":"folder"}],
    }))
    .unwrap();
    validate_scope(&folder, Some("root/")).unwrap();
    let file =
        serde_json::from_value(json!({"Files":[{"FilePath":"root","FileType":"file","Size":1}]}))
            .unwrap();
    assert!(validate_scope(&file, Some("root/")).is_err());
}
