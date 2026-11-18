use std::{
    net::TcpListener,
    process::{Command, Output},
};

fn startup(settings: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ask-auth"));
    command.env_clear();
    for (name, value) in settings {
        command.env(name, value);
    }
    command.output().unwrap()
}

#[test]
fn invalid_configuration_exits_without_exposing_values() {
    for settings in [
        vec![("ASK_AUTH_LISTEN", "PRIVATE_CONFIG_SENTINEL")],
        vec![("ASK_AUTH_LISTEN", "localhost:8080")],
        vec![(
            "ASK_AUTH_LOG_LEVEL",
            "ask_auth=trace,PRIVATE_CONFIG_SENTINEL=debug",
        )],
        vec![("ASK_AUTH_LOG_LEVEL", "")],
        vec![("ASK_AUTH_APP_SECRET", "PRIVATE_CONFIG_SENTINEL")],
        vec![("ASK_AUTH_MOCK_PROVIDER", "true")],
    ] {
        let output = startup(&settings);
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!stderr.contains("PRIVATE_CONFIG_SENTINEL"));
        assert!(stderr.contains("configuration"));
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn occupied_listen_address_exits_with_safe_bind_error() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let listen_address = listener.local_addr().unwrap().to_string();
    let output = startup(&[("ASK_AUTH_LISTEN", &listen_address)]);
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("bind"));
}

#[cfg(unix)]
struct RunningServer(std::process::Child);

#[cfg(unix)]
impl Drop for RunningServer {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) && self.0.kill().is_err() {
            eprintln!("test server cleanup could not terminate process");
        }
        if self.0.wait().is_err() {
            eprintln!("test server cleanup could not reap process");
        }
    }
}

#[cfg(unix)]
fn launch_server() -> (RunningServer, std::net::SocketAddr) {
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let process = Command::new(env!("CARGO_BIN_EXE_ask-auth"))
        .env_clear()
        .env("ASK_AUTH_LISTEN", address.to_string())
        .env("ASK_AUTH_LOG_LEVEL", "off")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    (RunningServer(process), address)
}

#[cfg(unix)]
fn connect_when_listening(address: std::net::SocketAddr) -> std::net::TcpStream {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Ok(connection) =
            std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_millis(100))
        {
            connection
                .set_read_timeout(Some(std::time::Duration::from_secs(1)))
                .unwrap();
            return connection;
        }
        assert!(std::time::Instant::now() < deadline, "server did not start");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn send_termination(process: &std::process::Child) {
    let status = Command::new("/bin/kill")
        .arg("-TERM")
        .arg(process.id().to_string())
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
fn bounded_exit(process: &mut std::process::Child) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(7);
    loop {
        if let Some(status) = process.try_wait().unwrap() {
            return Some(status);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(unix)]
fn confirm_health(address: std::net::SocketAddr) {
    use std::io::{Read, Write};
    let mut connection = connect_when_listening(address);
    connection
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    connection.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
}

#[cfg(unix)]
#[test]
fn shutdown_drain_deadline_bounds_incomplete_http_headers() {
    use std::io::{Read, Write};
    let (mut server, address) = launch_server();
    confirm_health(address);
    let mut connection = connect_when_listening(address);
    connection.write_all(b"POST /v1/auth/login-attempts HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nX-Slow: SHUTDOWN_HEADER_SECRET").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_termination(&server.0);
    let status =
        bounded_exit(&mut server.0).expect("server did not stop within bounded shutdown drain");
    assert!(!status.success());
    let mut stderr = String::new();
    server
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.contains("shutdown drain deadline exceeded"));
    assert!(!stderr.contains("SHUTDOWN_HEADER_SECRET"));
}

#[cfg(unix)]
#[test]
fn shutdown_after_complete_health_request_exits_successfully() {
    let (mut server, address) = launch_server();
    confirm_health(address);
    send_termination(&server.0);
    let status = bounded_exit(&mut server.0).expect("healthy server did not stop gracefully");
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn shutdown_allows_an_in_flight_request_to_finish_within_the_drain_deadline() {
    use std::io::{Read, Write};
    let (mut server, address) = launch_server();
    confirm_health(address);
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../contracts/ask-auth-v1.15.fixtures.json")).unwrap();
    let body = fixtures["creation"].to_string();
    let midpoint = body.len() / 2;
    let headers = format!("POST /v1/auth/login-attempts HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let mut connection = connect_when_listening(address);
    connection.write_all(headers.as_bytes()).unwrap();
    connection.write_all(&body.as_bytes()[..midpoint]).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    send_termination(&server.0);
    std::thread::sleep(std::time::Duration::from_millis(100));
    connection.write_all(&body.as_bytes()[midpoint..]).unwrap();
    let mut response = String::new();
    connection.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 503"));
    assert!(response.contains("upstream_unavailable"));
    let status = bounded_exit(&mut server.0).expect("completed request did not drain");
    assert!(status.success());
}
