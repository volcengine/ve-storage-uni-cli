//! TOS wire contracts exercised through the public CLI, using only loopback HTTP.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_secs(15);
static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
struct Request {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    fn query(&self, name: &str) -> Option<&str> {
        self.target.split_once('?')?.1.split('&').find_map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key == name).then_some(value)
        })
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("request JSON body")
    }
}

fn read_request(stream: &mut TcpStream) -> Request {
    stream
        .set_nonblocking(false)
        .expect("blocking accepted socket");
    stream
        .set_read_timeout(Some(DEADLINE))
        .expect("HTTP read deadline");
    stream
        .set_write_timeout(Some(DEADLINE))
        .expect("HTTP write deadline");
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    reader.read_line(&mut first).expect("HTTP request line");
    let mut words = first.split_whitespace();
    let method = words.next().expect("request method").to_owned();
    let target = words.next().expect("request target").to_owned();
    let mut headers = Vec::new();
    let mut header_bytes = first.len();
    loop {
        let mut line = String::new();
        let count = reader.read_line(&mut line).expect("HTTP header");
        assert!(count > 0, "truncated HTTP headers");
        header_bytes += count;
        assert!(header_bytes < 65536, "bounded HTTP headers");
        if line == "\r\n" {
            break;
        }
        let (name, value) = line.split_once(':').expect("header colon");
        headers.push((name.to_ascii_lowercase(), value.trim().to_owned()));
    }
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| value.parse::<usize>().expect("content length"))
        .unwrap_or(0);
    assert!(length < 65536, "bounded HTTP body");
    let mut body = vec![0; length];
    reader.read_exact(&mut body).expect("HTTP body");
    Request {
        method,
        target,
        headers,
        body,
    }
}

#[derive(Clone)]
struct ObjectState {
    content_type: String,
    metadata: BTreeMap<String, String>,
    storage_class: String,
    tags: Value,
}

impl Default for ObjectState {
    fn default() -> Self {
        Self {
            content_type: "text/plain".into(),
            metadata: BTreeMap::from([("old".into(), "keep".into())]),
            storage_class: "STANDARD".into(),
            tags: json!({"TagSet":{"Tags":[]}}),
        }
    }
}

fn update_metadata(state: &mut ObjectState, request: &Request) {
    if request.header("x-tos-metadata-directive") == Some("REPLACE") {
        state.metadata.clear();
        state.content_type = "application/octet-stream".into();
    }
    if let Some(content_type) = request.header("content-type") {
        state.content_type = content_type.into();
    }
    for (name, value) in &request.headers {
        if let Some(key) = name.strip_prefix("x-tos-meta-") {
            state.metadata.insert(key.into(), value.clone());
        }
    }
}

fn head_headers(state: &ObjectState) -> Vec<(String, String)> {
    let mut headers = vec![
        ("content-type".into(), state.content_type.clone()),
        ("x-tos-storage-class".into(), state.storage_class.clone()),
    ];
    headers.extend(
        state
            .metadata
            .iter()
            .map(|(key, value)| (format!("x-tos-meta-{key}"), value.clone())),
    );
    headers
}

fn response_for(
    request: &Request,
    versions: &mut BTreeMap<String, ObjectState>,
) -> (Vec<(String, String)>, String) {
    let version = request.query("versionId").unwrap_or("latest").to_owned();
    let state = versions.entry(version.clone()).or_default();
    if request.method == "POST" && request.query("metadata").is_some() {
        update_metadata(state, request);
    }
    if request.method == "POST" && request.query("fetch").is_some() {
        update_metadata(state, request);
        if let Some(class) = request.header("x-tos-storage-class") {
            state.storage_class = class.into();
        }
    }
    if request.query("tagging").is_some() {
        if request.method == "PUT" {
            let body = request.json();
            // The service ignores unsupported fields; success alone must not satisfy the test.
            if body["TagSet"]["Tags"].is_array() {
                state.tags = body;
            }
        }
        return (Vec::new(), state.tags.to_string());
    }
    if request.query("acl").is_some() {
        return (
            Vec::new(),
            json!({"Owner":{"ID":version},"Grants":[]}).to_string(),
        );
    }
    if request.method == "HEAD" {
        return (head_headers(state), String::new());
    }
    (Vec::new(), "{}".into())
}

fn send_response(
    stream: &mut TcpStream,
    request: &Request,
    versions: &mut BTreeMap<String, ObjectState>,
) {
    let (headers, body) = response_for(request, versions);
    let mut response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nx-tos-request-id: synthetic-protocol\r\nConnection: close\r\n", body.len());
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    response.push_str(&body);
    stream
        .write_all(response.as_bytes())
        .expect("HTTP response");
}

struct Server {
    endpoint: String,
    requests: Receiver<Request>,
    is_stopped: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let endpoint = format!(
            "http://{}",
            listener.local_addr().expect("listener address")
        );
        let (sender, requests) = mpsc::channel();
        let is_stopped = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&is_stopped);
        let worker = thread::spawn(move || {
            let mut versions = BTreeMap::new();
            while !worker_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_request(&mut stream);
                        send_response(&mut stream, &request, &mut versions);
                        if sender.send(request).is_err() {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("accept loopback request: {error}"),
                }
            }
        });
        Self {
            endpoint,
            requests,
            is_stopped,
            worker: Some(worker),
        }
    }

    fn request(&self) -> Request {
        self.requests
            .recv_timeout(DEADLINE)
            .expect("CLI HTTP request deadline")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.is_stopped.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Process(Child);

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Fixture {
    directory: PathBuf,
    server: Server,
}

impl Fixture {
    fn new() -> Self {
        let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("tos-protocol-{}-{sequence}", std::process::id()));
        fs::create_dir(&directory).expect("isolated fixture directory");
        let server = Server::new();
        let config = format!("[default.ve-tos]\nauth_mode='aksk'\nendpoint='{}'\ncontrol_endpoint='{}'\nregion='test-region'\naccess_key_id='fixture-ak'\nsecret_access_key='fixture-sk'\nmax_retry_count=0\n", server.endpoint, server.endpoint);
        fs::write(directory.join("config"), config).expect("fake configuration");
        fs::write(directory.join("credentials"), "schema_version=1\n").expect("fake credentials");
        Self { directory, server }
    }

    fn run(&self, args: &[&str]) -> String {
        let binary = std::env::var_os("TOS_PROTOCOL_TEST_BINARY")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_ve-storage-uni-cli").into());
        let stdout_path = self.directory.join("stdout");
        let stderr_path = self.directory.join("stderr");
        let mut command = Command::new(binary);
        command
            .env_clear()
            .env("HOME", &self.directory)
            .env("PATH", "/usr/bin:/bin")
            // [Review Fix #1] Low-level APIs belong to the Volcengine entry point.
            .arg("ve-tos")
            .arg("--config-path")
            .arg(self.directory.join("config"))
            .arg("--credentials-path")
            .arg(self.directory.join("credentials"))
            .args(["--output", "json", "--quiet"])
            .args(args)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout_path).expect("stdout file"))
            .stderr(fs::File::create(&stderr_path).expect("stderr file"));
        let mut process = Process(command.spawn().expect("spawn isolated CLI"));
        let deadline = Instant::now() + DEADLINE;
        let status = loop {
            if let Some(status) = process.0.try_wait().expect("poll CLI") {
                break status;
            }
            assert!(Instant::now() < deadline, "CLI command deadline: {args:?}");
            thread::sleep(Duration::from_millis(5));
        };
        let stdout = fs::read_to_string(stdout_path).expect("read stdout");
        let stderr = fs::read_to_string(stderr_path).expect("read stderr");
        assert!(status.success(), "CLI failed {args:?}: {stdout}\n{stderr}");
        stdout
    }

    fn object(&self, action: &str, extra: &[&str]) -> (Request, String) {
        let mut args = vec![
            "object",
            action,
            "--bucket",
            "bucket",
            "--key",
            "object.txt",
        ];
        args.extend_from_slice(extra);
        let output = self.run(&args);
        (self.server.request(), output)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn set_meta_merges_headers_without_corrupting_existing_content_type() {
    let fixture = Fixture::new();
    let (request, _) = fixture.object("set-meta", &["--meta", "owner=xsh"]);
    let (_, readback) = fixture.object("head", &[]);
    assert!(
        request.body.is_empty(),
        "metadata must be headers, not a JSON body"
    );
    assert_eq!(
        request.header("x-tos-metadata-directive"),
        Some("REPLACE_NEW")
    );
    assert_eq!(request.header("x-tos-meta-owner"), Some("xsh"));
    assert_eq!(
        request.header("content-type"),
        None,
        "do not overwrite stored content type"
    );
    assert!(
        readback.contains("text/plain")
            && readback.contains("x-tos-meta-old")
            && readback.contains("x-tos-meta-owner"),
        "merged HEAD state: {readback}"
    );
}

#[test]
fn replace_metadata_removes_old_keys_and_explicit_content_type_is_unique() {
    let fixture = Fixture::new();
    let (request, _) = fixture.object(
        "set-meta",
        &[
            "--meta",
            "owner=xsh",
            "--metadata-directive",
            "REPLACE",
            "--content-type",
            "image/png",
        ],
    );
    let (_, readback) = fixture.object("head", &[]);
    assert_eq!(request.header("x-tos-metadata-directive"), Some("REPLACE"));
    assert_eq!(
        request
            .headers
            .iter()
            .filter(|(name, _)| name == "content-type")
            .count(),
        1
    );
    assert_eq!(request.header("content-type"), Some("image/png"));
    assert!(
        readback.contains("image/png") && readback.contains("x-tos-meta-owner"),
        "replacement HEAD state: {readback}"
    );
    assert!(
        !readback.contains("x-tos-meta-old"),
        "REPLACE must remove old user metadata"
    );
}

#[test]
fn metadata_literals_and_non_ascii_are_percent_encoded_exactly_once() {
    let fixture = Fixture::new();
    let (request, _) = fixture.object(
        "set-meta",
        &[
            "--meta",
            r#"{"literal":"%20","percentage":"50%","city":"北京"}"#,
        ],
    );
    assert_eq!(request.header("x-tos-meta-literal"), Some("%2520"));
    assert_eq!(request.header("x-tos-meta-percentage"), Some("50%25"));
    assert_eq!(
        request.header("x-tos-meta-city"),
        Some("%E5%8C%97%E4%BA%AC")
    );
}

#[test]
fn timestamps_use_seconds_and_nanosecond_headers_without_json_body() {
    let fixture = Fixture::new();
    for (input, nanoseconds) in [
        ("1735689600", "0"),
        ("2025-01-01T00:00:00.123456789Z", "123456789"),
    ] {
        let (request, _) = fixture.object("set-time", &["--time", input]);
        // [Review Fix #2] Assert external SDK headers before server middleware strips x-tos-.
        assert_eq!(request.header("x-tos-modify-timestamp"), Some("1735689600"));
        assert_eq!(
            request.header("x-tos-modify-timestamp-ns"),
            Some(nanoseconds)
        );
        assert!(request.body.is_empty());
    }
}

#[test]
fn expiry_uses_numeric_object_expires_in_ttl_days() {
    let fixture = Fixture::new();
    // [Review Fix #ProtocolExpiresDays] The integer is a day count, not an absolute timestamp.
    for input in ["7", "0"] {
        let (request, _) = fixture.object("set-expires", &["--expires", input]);
        assert_eq!(request.query("objectExpires"), Some(""));
        assert_eq!(
            request.json(),
            json!({"ObjectExpires": input.parse::<u64>().unwrap()})
        );
    }
}

#[test]
fn key_value_tagging_has_tagset_envelope_and_survives_get() {
    let fixture = Fixture::new();
    let (request, _) = fixture.object("set-tagging", &["--tags", "owner=xsh&purpose=regression"]);
    let (_, readback) = fixture.object("get-tagging", &[]);
    assert_eq!(
        request.json(),
        json!({"TagSet":{"Tags":[{"Key":"owner","Value":"xsh"},{"Key":"purpose","Value":"regression"}]}})
    );
    assert!(
        readback.contains("xsh") && readback.contains("regression"),
        "GET tagging state: {readback}"
    );
}

#[test]
fn fetch_keeps_url_in_body_and_metadata_storage_class_in_headers() {
    let fixture = Fixture::new();
    let (request, _) = fixture.object(
        "fetch",
        &[
            "--source-url",
            "https://example.invalid/source",
            "--storage-class",
            "IA",
            "--meta",
            "owner=xsh",
        ],
    );
    let (_, readback) = fixture.object("head", &[]);
    assert_eq!(
        request.json(),
        json!({"URL":"https://example.invalid/source"})
    );
    assert_eq!(request.header("x-tos-meta-owner"), Some("xsh"));
    assert_eq!(request.header("x-tos-storage-class"), Some("IA"));
    assert!(
        readback.contains("xsh") && readback.contains("IA"),
        "fetched HEAD state: {readback}"
    );
}

#[test]
fn versioned_metadata_and_acl_select_only_requested_version() {
    let fixture = Fixture::new();
    let (mutation, _) = fixture.object(
        "set-meta",
        &["--meta", "owner=selected", "--version-id", "version-one"],
    );
    let (_, selected) = fixture.object("head", &["--version-id", "version-one"]);
    let (_, latest) = fixture.object("head", &[]);
    let (acl, acl_output) = fixture.object("get-acl", &["--version-id", "version-one"]);
    assert_eq!(mutation.query("versionId"), Some("version-one"));
    assert_eq!(mutation.query("queryVersionID"), None);
    assert!(
        selected.contains("selected"),
        "selected version HEAD: {selected}"
    );
    assert!(
        !latest.contains("selected"),
        "unselected version was modified: {latest}"
    );
    assert_eq!(acl.query("versionId"), Some("version-one"));
    assert!(
        acl_output.contains("version-one"),
        "ACL selected wrong version: {acl_output}"
    );
}

#[test]
fn object_status_uses_stat_marker() {
    let fixture = Fixture::new();
    let (request, _) = fixture.object("status", &[]);
    assert_eq!(request.query("stat"), Some(""));
    assert_eq!(request.query("status"), None);
}

#[test]
fn data_process_job_uses_job_id_and_job_type_values() {
    let fixture = Fixture::new();
    fixture.run(&[
        "data-process",
        "get-job",
        "--bucket",
        "bucket",
        "--job-id",
        "jobA",
        "--job-type",
        "Transcode",
    ]);
    let request = fixture.server.request();
    assert_eq!(request.query("job_type"), Some("Transcode"));
    assert_eq!(request.query("job_id"), Some("jobA"));
}

#[test]
fn object_set_selectors_use_named_parameter_for_normal_and_lifecycle_calls() {
    let fixture = Fixture::new();
    for action in ["get", "get-lifecycle"] {
        fixture.run(&[
            "object-set",
            action,
            "--bucket",
            "bucket",
            "--object-set-name",
            "groupA",
        ]);
        let request = fixture.server.request();
        assert_eq!(
            request.query("ObjectSetName"),
            Some("groupA"),
            "{action}: {request:?}"
        );
        assert_eq!(
            request.query("groupA"),
            None,
            "name must not become a query key"
        );
    }
}

#[test]
fn dataset_unbind_preserves_delete_json_body() {
    let fixture = Fixture::new();
    let config = r#"{"DatasetName":"datasetA","URI":"tos://bucket"}"#;
    fixture.run(&[
        "dataset",
        "delete-binding",
        "--name",
        "datasetA",
        "--config",
        config,
        "--force",
        "--confirm",
        "tos://datasetA",
    ]);
    let request = fixture.server.request();
    assert_eq!(request.method, "DELETE");
    assert!(request.target.starts_with("/datasetbinding"));
    assert_eq!(
        request.json(),
        json!({"DatasetName":"datasetA","URI":"tos://bucket"})
    );
}

#[test]
fn batch_delete_uses_resolved_recursive_and_skip_trash_query_names() {
    let fixture = Fixture::new();
    fixture.run(&[
        "object",
        "batch-delete",
        "--bucket",
        "bucket",
        "--keys",
        "object.txt",
        "--recursive",
        "--skip-trash",
        "--force",
        "--confirm",
        "tos://bucket",
    ]);
    let request = fixture.server.request();
    // [Review Fix #ProtocolQueryConstants] Go constant identifiers are not wire names.
    assert_eq!(request.query("recursive"), Some("true"));
    assert_eq!(request.query("skipTrash"), Some("true"));
    assert_eq!(request.query("queryRecursive"), None);
    assert_eq!(request.query("querySkipTrash"), None);
}
