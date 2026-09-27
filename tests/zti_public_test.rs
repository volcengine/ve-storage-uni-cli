//! Public ZTI entry-point contracts with synthetic credentials and loopback HTTP.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TIMEOUT: Duration = Duration::from_secs(20);
const OBJECT_BODY: &str = "local-object\n";
static NEXT_HOME: AtomicU64 = AtomicU64::new(0);

struct Home(PathBuf);

impl Home {
    fn new() -> Self {
        let suffix = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("tos-public-zti-{}-{suffix}", std::process::id()));
        fs::create_dir(&path).expect("create isolated home");
        let config_dir = path.join(".tos");
        fs::create_dir(&config_dir).expect("create isolated configuration");
        fs::write(config_dir.join("credentials.toml"), "invalid = [")
            .expect("write deliberately invalid AKSK configuration");
        Self(path)
    }

    fn command(&self, executable: &Path) -> Command {
        let mut command = Command::new(executable);
        command
            .env_clear()
            .env("HOME", &self.0)
            .env("PATH", "/usr/bin:/bin")
            .env("RUST_LOG", "trace");
        command
    }

    fn root_command(&self) -> Command {
        let mut command = self.command(Path::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli")));
        command.arg("tos");
        command
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn encode_base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        encoded.push(ALPHABET[(first >> 2) as usize] as char);
        encoded.push(ALPHABET[(((first & 3) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[(((second & 15) << 2) | (third >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[(third & 63) as usize] as char);
        }
    }
    encoded
}

fn synthetic_token(is_expired: bool) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs();
    let expiration = if is_expired { now - 3600 } else { now + 3600 };
    let claims = json!({"sub":"spiffe://example.org/ns:test/id:integration","exp":expiration});
    format!(
        "e30.{}.synthetic-signature",
        encode_base64url(claims.to_string().as_bytes())
    )
}

fn assert_redacted(stdout: &str, stderr: &str, token: &str) {
    assert!(!stdout.contains(token), "stdout exposed synthetic token");
    assert!(!stderr.contains(token), "stderr exposed synthetic token");
}

fn failure_json(stderr: &str) -> Value {
    // [Review Fix #2] Trace diagnostics from the fake agent may precede JSON.
    let start = if stderr.starts_with('{') {
        0
    } else {
        stderr.rfind("\n{").expect("structured failure JSON") + 1
    };
    serde_json::from_str(&stderr[start..]).expect("parse structured failure JSON")
}

fn assert_zti_request(request: &str, token: &str) {
    assert!(request.starts_with("GET "), "expected object GET");
    assert!(request.lines().next().unwrap_or("").contains("/key"));
    let headers: Vec<_> = request
        .lines()
        .filter_map(|line| line.split_once(':'))
        .collect();
    let zti_headers: Vec<_> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("x-tos-ztitoken-with-acp"))
        .collect();
    assert_eq!(zti_headers.len(), 1, "expected one ZTI header");
    assert!(zti_headers[0].1.trim() == token, "wrong ZTI token value");
    assert!(
        headers.iter().all(|(name, _)| {
            !name.eq_ignore_ascii_case("authorization")
                && !name.to_ascii_lowercase().contains("security-token")
        }),
        "unexpected signed or security-token header"
    );
}

struct Process {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    errors: Receiver<String>,
}

impl Process {
    fn spawn(command: &mut Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn isolated CLI");
        let stdin = child.stdin.take().expect("child stdin");
        let stdout = child.stdout.take().expect("child stdout");
        let mut stderr = child.stderr.take().expect("child stderr");
        let (line_sender, lines) = mpsc::channel();
        let (error_sender, errors) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if line_sender.send(line.expect("read child stdout")).is_err() {
                    break;
                }
            }
        });
        thread::spawn(move || {
            let mut output = String::new();
            stderr
                .read_to_string(&mut output)
                .expect("read child stderr");
            let _ = error_sender.send(output);
        });
        Self {
            child,
            stdin,
            lines,
            errors,
        }
    }

    fn request(&mut self, request: Value) -> Value {
        writeln!(self.stdin, "{request}").expect("write MCP request");
        self.stdin.flush().expect("flush MCP request");
        let line = self
            .lines
            .recv_timeout(TIMEOUT)
            .expect("MCP response deadline");
        serde_json::from_str(&line).expect("MCP response JSON")
    }

    fn finish(&mut self) -> (ExitStatus, String, String) {
        let deadline = Instant::now() + TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("poll child") {
                break status;
            }
            assert!(Instant::now() < deadline, "CLI exceeded deadline");
            thread::sleep(Duration::from_millis(10));
        };
        let stderr = self.errors.recv_timeout(TIMEOUT).expect("stderr deadline");
        let mut stdout = String::new();
        // [Review Fix #3] A stalled reader must fail instead of returning partial output.
        loop {
            match self.lines.recv_timeout(TIMEOUT) {
                Ok(line) => {
                    stdout.push_str(&line);
                    stdout.push('\n');
                }
                Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => panic!("stdout deadline"),
            }
        }
        (status, stdout, stderr)
    }

    fn stop(&mut self) {
        self.child.kill().expect("stop MCP server");
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.child.try_wait().expect("poll child cleanup").is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn accept_before_deadline(listener: &TcpListener) -> Option<TcpStream> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept mock HTTP: {error}"),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    // Accepted sockets may inherit nonblocking mode on macOS.
    stream
        .set_nonblocking(false)
        .expect("blocking accepted socket");
    stream
        .set_read_timeout(Some(TIMEOUT))
        .expect("read deadline");
    stream
        .set_write_timeout(Some(TIMEOUT))
        .expect("write deadline");
    let mut reader = BufReader::new(stream);
    let mut request = String::new();
    loop {
        let mut line = String::new();
        let count = reader.read_line(&mut line).expect("read HTTP header");
        if count == 0 || line == "\r\n" {
            break;
        }
        request.push_str(&line);
        assert!(request.len() < 65536, "HTTP headers exceeded bound");
    }
    request
}

fn mock_server(refresh: Option<(PathBuf, String)>) -> (String, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local mock");
    listener.set_nonblocking(true).expect("nonblocking mock");
    let endpoint = format!("http://{}", listener.local_addr().expect("mock address"));
    let (sender, requests) = mpsc::channel();
    thread::spawn(move || {
        let response_count = if refresh.is_some() { 2 } else { 1 };
        for index in 0..response_count {
            let Some(mut stream) = accept_before_deadline(&listener) else {
                break;
            };
            let request = read_request(&mut stream);
            if index == 0 {
                if let Some((path, token)) = &refresh {
                    let replacement = path.with_extension("next");
                    fs::write(&replacement, token).expect("write refreshed token");
                    fs::rename(replacement, path).expect("atomically refresh token");
                    stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nRetry-After: 1\r\nConnection: close\r\n\r\n").expect("write retry response");
                } else {
                    send_object(&mut stream);
                }
            } else {
                send_object(&mut stream);
            }
            let _ = sender.send(request);
        }
    });
    (endpoint, requests)
}

fn send_object(stream: &mut TcpStream) {
    let response = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nx-tos-request-id: synthetic\r\nConnection: close\r\n\r\n{OBJECT_BODY}", OBJECT_BODY.len());
    stream
        .write_all(response.as_bytes())
        .expect("write object response");
}

fn cat_command(home: &Home, endpoint: &str) -> Command {
    let mut command = home.root_command();
    command.args([
        "--auth-mode",
        "zti",
        "--region",
        "test-region",
        "--endpoint",
        endpoint,
        "cat",
        "tos://bucket/key",
    ]);
    command
}

#[test]
fn embedded_source_takes_priority_and_emits_only_zti_header() {
    let home = Home::new();
    let token = synthetic_token(false);
    let agent_path = home.0.join("agent.sock");
    let file_path = home.0.join("other.jwt");
    fs::write(&agent_path, "not a socket").expect("write fake agent path");
    let file_token = token.replace("synthetic-signature", "lower-priority-signature");
    fs::write(&file_path, &file_token).expect("write lower priority file");
    let (endpoint, requests) = mock_server(None);
    let mut command = cat_command(&home, &endpoint);
    command
        .env("SEC_TOKEN_STRING", &token)
        .env("SEC_TOKEN_PATH", &file_path)
        .env("ZTI_AGENT_SOCKET_PATH", &agent_path);
    let (status, stdout, stderr) = Process::spawn(&mut command).finish();
    assert_redacted(&stdout, &stderr, &token);
    assert_redacted(&stdout, &stderr, &file_token);
    assert!(status.success(), "embedded source failed");
    assert!(stdout.contains(OBJECT_BODY), "missing object body");
    assert_zti_request(
        &requests.recv_timeout(TIMEOUT).expect("HTTP request"),
        &token,
    );
}

#[test]
fn non_utf8_embedded_token_uses_file_source_like_zti_sdk() {
    use std::os::unix::ffi::OsStringExt;

    if Path::new("/run/zti-agent.sock").exists() {
        return;
    }
    let home = Home::new();
    let token = synthetic_token(false);
    let file_path = home.0.join("fallback.jwt");
    fs::write(&file_path, &token).expect("write file token");
    let (endpoint, requests) = mock_server(None);
    let mut command = cat_command(&home, &endpoint);
    command
        .env("SEC_TOKEN_STRING", std::ffi::OsString::from_vec(vec![0xff]))
        .env("SEC_TOKEN_PATH", &file_path)
        .env("ZTI_AGENT_SOCKET_PATH", home.0.join("absent-agent.sock"));

    let (status, stdout, stderr) = Process::spawn(&mut command).finish();
    assert_redacted(&stdout, &stderr, &token);
    assert!(
        status.success(),
        "file source should remain usable: {stderr}"
    );
    assert_zti_request(
        &requests.recv_timeout(TIMEOUT).expect("HTTP request"),
        &token,
    );
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_agent_path_is_ignored_like_zti_sdk() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    // The SDK correctly chooses the default Agent if one is installed on this host.
    if Path::new("/run/zti-agent.sock").exists() {
        return;
    }
    let home = Home::new();
    let token = synthetic_token(false);
    let file_path = home.0.join("fallback.jwt");
    fs::write(&file_path, &token).expect("write file token");
    let mut agent_bytes = home.0.as_os_str().as_bytes().to_vec();
    agent_bytes.extend_from_slice(b"/agent-");
    agent_bytes.push(0xff);
    let agent_path = PathBuf::from(std::ffi::OsString::from_vec(agent_bytes));
    fs::write(&agent_path, "not a socket").expect("write fake agent path");
    let (endpoint, requests) = mock_server(None);
    let mut command = cat_command(&home, &endpoint);
    command
        .env("ZTI_AGENT_SOCKET_PATH", agent_path)
        .env("SEC_TOKEN_PATH", &file_path);

    let (status, stdout, stderr) = Process::spawn(&mut command).finish();
    assert_redacted(&stdout, &stderr, &token);
    assert!(status.success(), "file source should be selected: {stderr}");
    assert_zti_request(
        &requests.recv_timeout(TIMEOUT).expect("HTTP request"),
        &token,
    );
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_file_path_is_ignored_like_zti_sdk() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    if Path::new("/run/zti-agent.sock").exists() {
        return;
    }
    let home = Home::new();
    let token = synthetic_token(false);
    let mut file_bytes = home.0.as_os_str().as_bytes().to_vec();
    file_bytes.extend_from_slice(b"/token-");
    file_bytes.push(0xff);
    let file_path = PathBuf::from(std::ffi::OsString::from_vec(file_bytes));
    fs::write(&file_path, &token).expect("write file token");
    let mut command = cat_command(&home, "http://127.0.0.1:1");
    command
        .env("ZTI_AGENT_SOCKET_PATH", home.0.join("absent-agent.sock"))
        .env("SEC_TOKEN_PATH", file_path);

    let (status, stdout, stderr) = Process::spawn(&mut command).finish();
    assert_redacted(&stdout, &stderr, &token);
    assert!(!status.success());
    assert_eq!(failure_json(&stderr)["ec"], "ZtiTokenUnavailable");
}

#[test]
fn file_source_refreshes_by_atomic_rename_before_retry() {
    let home = Home::new();
    let first_token = synthetic_token(false);
    let next_token = first_token.replace("synthetic-signature", "refreshed-signature");
    let file_path = home.0.join("refresh.jwt");
    fs::write(&file_path, &first_token).expect("write initial token");
    let (endpoint, requests) = mock_server(Some((file_path.clone(), next_token.clone())));
    let mut command = cat_command(&home, &endpoint);
    command
        .env("SEC_TOKEN_PATH", &file_path)
        .env("ZTI_AGENT_SOCKET_PATH", home.0.join("absent-agent.sock"));
    let (status, stdout, stderr) = Process::spawn(&mut command).finish();
    assert_redacted(&stdout, &stderr, &first_token);
    assert_redacted(&stdout, &stderr, &next_token);
    assert!(status.success(), "file refresh failed");
    assert!(stdout.contains(OBJECT_BODY), "missing object body");
    assert_zti_request(
        &requests.recv_timeout(TIMEOUT).expect("initial HTTP"),
        &first_token,
    );
    assert_zti_request(
        &requests.recv_timeout(TIMEOUT).expect("retry HTTP"),
        &next_token,
    );
}

#[test]
fn selected_agent_failure_does_not_fall_back_to_file() {
    let home = Home::new();
    let token = synthetic_token(false);
    let file_path = home.0.join("unused.jwt");
    let agent_path = home.0.join("fake-agent.sock");
    fs::write(&file_path, &token).expect("write unused file token");
    fs::write(&agent_path, "not a socket").expect("write fake agent path");
    let (endpoint, requests) = mock_server(None);
    let mut command = cat_command(&home, &endpoint);
    command
        .env("SEC_TOKEN_PATH", &file_path)
        .env("ZTI_AGENT_SOCKET_PATH", &agent_path);
    let (status, stdout, stderr) = Process::spawn(&mut command).finish();
    assert_redacted(&stdout, &stderr, &token);
    assert!(
        !status.success(),
        "selected agent failure unexpectedly succeeded"
    );
    let failure = failure_json(&stderr);
    assert_eq!(failure["ec"], "ZtiTokenUnavailable");
    // [Review Fix #1] Let the listener report a raced request before asserting fail-closed.
    assert!(
        matches!(
            requests.recv_timeout(Duration::from_millis(200)),
            Err(RecvTimeoutError::Timeout)
        ),
        "authentication sent HTTP request"
    );
}

#[test]
fn malformed_and_expired_tokens_have_distinct_redacted_errors() {
    for (token, expected_category) in [
        (
            "malformed-SYNTHETIC-SECRET".to_owned(),
            "ZtiTokenUnavailable",
        ),
        (synthetic_token(true), "ZtiTokenExpired"),
    ] {
        let home = Home::new();
        let mut command = cat_command(&home, "http://127.0.0.1:1");
        command.env("SEC_TOKEN_STRING", &token);
        let (status, stdout, stderr) = Process::spawn(&mut command).finish();
        assert_redacted(&stdout, &stderr, &token);
        assert!(!status.success(), "invalid token unexpectedly succeeded");
        let failure = failure_json(&stderr);
        assert_eq!(failure["ec"], expected_category);
    }
}

#[test]
fn help_describe_and_dry_run_stay_offline_with_malformed_token() {
    let token = "malformed-SYNTHETIC-OFFLINE";
    for args in [
        vec!["--auth-mode", "zti", "--help"],
        vec!["--auth-mode", "zti", "cat", "--describe"],
        vec![
            "--auth-mode",
            "zti",
            "--dry-run",
            "--endpoint",
            "http://127.0.0.1:1",
            "cat",
            "tos://bucket/key",
        ],
    ] {
        let home = Home::new();
        let mut command = home.root_command();
        command.env("SEC_TOKEN_STRING", token).args(args);
        let (status, stdout, stderr) = Process::spawn(&mut command).finish();
        assert_redacted(&stdout, &stderr, token);
        assert!(status.success(), "offline command failed");
    }
}

fn initialize_mcp(process: &mut Process) {
    let initialized = process.request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"public-zti-test","version":"1"}}}));
    assert!(initialized["result"].is_object(), "initialize MCP");
    writeln!(
        process.stdin,
        "{}",
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}})
    )
    .expect("initialized notification");
    process
        .stdin
        .flush()
        .expect("flush initialized notification");
}

#[test]
fn mcp_stdio_child_uses_builtin_zti_source() {
    let home = Home::new();
    let token = synthetic_token(false);
    let (endpoint, requests) = mock_server(None);
    let mut command = home.root_command();
    command.env("SEC_TOKEN_STRING", &token).args([
        "--auth-mode",
        "zti",
        "--region",
        "test-region",
        "--endpoint",
        &endpoint,
        "serve",
        "--mcp",
        "--transport",
        "stdio",
    ]);
    let mut process = Process::spawn(&mut command);
    initialize_mcp(&mut process);
    let response = process.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tos_cat","arguments":{"path":"tos://bucket/key","execute":true}}}));
    assert_redacted(&response.to_string(), "", &token);
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("MCP execution content");
    let execution: Value = serde_json::from_str(text).expect("MCP execution JSON");
    assert_eq!(execution["exit_code"], 0, "MCP child execution failed");
    assert!(execution["stdout"]
        .as_str()
        .unwrap_or("")
        .contains(OBJECT_BODY));
    assert_zti_request(&requests.recv_timeout(TIMEOUT).expect("child HTTP"), &token);
    process.stop();
    let (_, stdout, stderr) = process.finish();
    assert_redacted(&stdout, &stderr, &token);
}

#[test]
fn mcp_stdio_child_refreshes_file_token_before_retry() {
    let home = Home::new();
    let first_token = synthetic_token(false);
    let next_token = first_token.replace("synthetic-signature", "refreshed-signature");
    let file_path = home.0.join("mcp-refresh.jwt");
    fs::write(&file_path, &first_token).expect("write initial token");
    let (endpoint, requests) = mock_server(Some((file_path.clone(), next_token.clone())));
    let mut command = home.root_command();
    command.env("SEC_TOKEN_PATH", &file_path).args([
        "--auth-mode",
        "zti",
        "--region",
        "test-region",
        "--endpoint",
        &endpoint,
        "serve",
        "--mcp",
        "--transport",
        "stdio",
    ]);
    let mut process = Process::spawn(&mut command);
    initialize_mcp(&mut process);
    let response = process.request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"tos_cat","arguments":{"path":"tos://bucket/key","execute":true}}}));
    assert_redacted(&response.to_string(), "", &first_token);
    assert_redacted(&response.to_string(), "", &next_token);
    let text = response["result"]["content"][0]["text"]
        .as_str()
        .expect("MCP execution content");
    let execution: Value = serde_json::from_str(text).expect("MCP execution JSON");
    assert_eq!(execution["exit_code"], 0, "MCP child execution failed");
    assert_zti_request(
        &requests.recv_timeout(TIMEOUT).expect("first HTTP"),
        &first_token,
    );
    assert_zti_request(
        &requests.recv_timeout(TIMEOUT).expect("retried HTTP"),
        &next_token,
    );
    process.stop();
    let (_, stdout, stderr) = process.finish();
    assert_redacted(&stdout, &stderr, &first_token);
    assert_redacted(&stdout, &stderr, &next_token);
}

#[test]
fn direct_tos_cli_alias_uses_builtin_zti_provider() {
    use std::os::unix::fs::symlink;
    let home = Home::new();
    let token = synthetic_token(false);
    let alias = home.0.join("tos-cli");
    symlink(env!("CARGO_BIN_EXE_ve-storage-uni-cli"), &alias).expect("create direct alias");
    let (endpoint, requests) = mock_server(None);
    let mut command = home.command(&alias);
    command.env("SEC_TOKEN_STRING", &token).args([
        "--auth-mode",
        "zti",
        "--region",
        "test-region",
        "--endpoint",
        &endpoint,
        "cat",
        "tos://bucket/key",
    ]);
    let (status, stdout, stderr) = Process::spawn(&mut command).finish();
    assert_redacted(&stdout, &stderr, &token);
    assert!(status.success(), "direct alias failed");
    assert!(stdout.contains(OBJECT_BODY));
    assert_zti_request(&requests.recv_timeout(TIMEOUT).expect("alias HTTP"), &token);
}
