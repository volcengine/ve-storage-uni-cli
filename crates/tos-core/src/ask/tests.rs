use super::*;
use std::{path::PathBuf, time::Duration};

fn config() -> Config {
    Config::new(
        "test-app".into(),
        "http://127.0.0.1:8080/callback",
        vec!["test:read".into()],
        std::env::temp_dir().join("ask-test"),
    )
    .unwrap()
}

#[test]
fn pkce_matches_rfc7636() {
    assert_eq!(
        callback::challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
    );
}

#[test]
fn configuration_rejects_unsafe_redirect_without_echoing_input() {
    for uri in [
        "https://evil.example/code?secret=hidden",
        "http://0.0.0.0:8080/callback",
        "http://localhost:8080/other",
        "http://u:p@localhost:8080/callback",
        "http://localhost:8080/callback?x=y",
    ] {
        let error = Config::new(
            "test".into(),
            uri,
            vec!["test:read".into()],
            PathBuf::from("/private/tmp/ask"),
        )
        .err()
        .unwrap();
        assert!(!error.to_string().contains(uri));
    }
}

#[test]
fn authorization_url_contains_pkce_and_required_scopes() {
    let pending = callback::authorization_parameters(&config()).unwrap();
    let url = url::Url::parse(&pending.url).unwrap();
    let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(url.host_str(), Some("accounts.doubao.com"));
    assert_eq!(pairs["code_challenge_method"], "S256");
    assert_eq!(
        pairs["code_challenge"],
        callback::challenge(pending.verifier.expose())
    );
    assert!(pairs["scope"]
        .split_whitespace()
        .any(|s| s == "offline_access"));
    assert!(pairs["state"].len() >= 43);
}

#[tokio::test]
async fn callback_rejects_mismatch_denial_and_duplicates() {
    for query in [
        "state=wrong&code=secret",
        "state=expected&error=access_denied",
        "state=expected&state=expected&code=secret",
        "state=expected&code=a&code=b",
    ] {
        assert!(callback::parse_callback(
            &format!("GET /callback?{query} HTTP/1.1\r\nHost: localhost\r\n\r\n"),
            "expected"
        )
        .is_err());
    }
}

#[tokio::test]
async fn callback_times_out() {
    let mut cfg = config();
    cfg.redirect_uri = url::Url::parse("http://127.0.0.1:0/callback").unwrap();
    let pending = callback::PendingLogin::bind(&cfg).await.unwrap();
    assert!(matches!(
        pending.wait(Duration::from_millis(10)).await,
        Err(Error::Timeout)
    ));
}

#[test]
fn response_requires_scopes_rotated_refresh_and_positive_lifetimes() {
    let valid = r#"{"code":0,"access_token":"access","refresh_token":"rotated","expires_in":3600,"refresh_token_expires_in":86400,"token_type":"Bearer","scope":"test:read offline_access"}"#;
    assert!(client::parse_tokens(valid.as_bytes(), &config(), 100).is_ok());
    for response in [
        valid.replace("test:read offline_access", "offline_access"),
        valid.replace("rotated", ""),
        valid.replace("3600", "0"),
        valid.replace("Bearer", "Basic"),
        "secret provider failure".into(),
    ] {
        assert!(client::parse_tokens(response.as_bytes(), &config(), 100).is_err());
    }
}

#[cfg(unix)]
fn directory() -> PathBuf {
    use rand::RngCore;
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let mut suffix = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut suffix);
    root.join(format!("ask-oauth-test-{}", hex::encode(suffix)))
}

#[cfg(unix)]
fn store_config(home: PathBuf) -> Config {
    let mut cfg = config();
    cfg.credentials_path = home.join("credentials.toml");
    cfg.home = home;
    cfg
}

#[cfg(unix)]
#[test]
fn encrypted_store_binds_app_and_pending_marker_survives_reopen() {
    let home = directory();
    let cfg = store_config(home.clone());
    let store = Store::open(&cfg).unwrap();
    assert_eq!(store.status().unwrap(), Status::LoggedOut);
    let tokens = client::parse_tokens(br#"{"code":0,"access_token":"access-secret","refresh_token":"refresh-secret","expires_in":1,"refresh_token_expires_in":86400,"token_type":"Bearer","scope":"test:read offline_access"}"#, &cfg, client::now().unwrap()).unwrap();
    store.save_login(&tokens).unwrap();
    let ciphertext = std::fs::read(&cfg.credentials_path).unwrap();
    assert!(!String::from_utf8_lossy(&ciphertext).contains("refresh-secret"));
    let guard = store.lock().unwrap();
    store.mark_pending(&guard).unwrap();
    drop(guard);
    assert_eq!(
        Store::open(&cfg).unwrap().status().unwrap(),
        Status::LoginRequired
    );
    let mut other = cfg.clone();
    other.app_id = "other-app".into();
    assert!(Store::open(&other).unwrap().status().is_err());
    store.logout().unwrap();
    assert_eq!(store.status().unwrap(), Status::LoggedOut);
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[test]
fn store_rejects_permissions_symlinks_and_busy_lock() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let home = directory();
    let cfg = store_config(home.clone());
    let store = Store::open(&cfg).unwrap();
    let guard = store.lock().unwrap();
    assert!(matches!(store.lock(), Err(Error::Busy)));
    drop(guard);
    crate::infra::crypto::load_or_init_key(&home).unwrap();
    std::fs::set_permissions(home.join(".key"), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Store::open(&cfg).is_err());
    std::fs::set_permissions(home.join(".key"), std::fs::Permissions::from_mode(0o600)).unwrap();
    crate::infra::credentials::CredentialsFile::default()
        .save_to_path(&cfg.credentials_path)
        .unwrap();
    let alias = home.join("credentials-alias.toml");
    symlink(&cfg.credentials_path, &alias).unwrap();
    let mut linked = cfg.clone();
    linked.credentials_path = alias.clone();
    assert!(Store::open(&linked).is_err());
    std::fs::remove_file(alias).unwrap();
    std::fs::remove_dir_all(home).unwrap();
}

async fn server(response: String, delay: Duration) -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/token", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let count = stream.read(&mut chunk).await.unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(position) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..position]).to_ascii_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap_or("0")
                    .parse()
                    .unwrap();
                if bytes.len() >= position + 4 + length {
                    break;
                }
            }
        }
        tokio::time::sleep(delay).await;
        let _ = stream.write_all(response.as_bytes()).await;
        String::from_utf8(bytes).unwrap()
    });
    (endpoint, task)
}
fn response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
fn token_body(access: &str, refresh: &str, lifetime: u64) -> String {
    format!(
        r#"{{"code":0,"access_token":"{access}","refresh_token":"{refresh}","expires_in":{lifetime},"refresh_token_expires_in":86400,"token_type":"Bearer","scope":"test:read offline_access"}}"#
    )
}

#[tokio::test]
async fn exchange_posts_exact_form_and_accepts_rotated_response() {
    let (endpoint, request) = server(
        response(&token_body("access", "new-refresh", 3600)),
        Duration::ZERO,
    )
    .await;
    let client = Client::for_test(config(), endpoint, Duration::from_secs(2));
    let code = AuthorizationCode {
        code: Secret::new("the code&".into()),
        verifier: Secret::new("verifier".into()),
        redirect_uri: config().redirect_uri().into(),
    };
    let tokens = client.finish_code(code).await.unwrap();
    assert_eq!(tokens.refresh_token.expose(), "new-refresh");
    let request = request.await.unwrap();
    let fields: std::collections::HashMap<_, _> =
        url::form_urlencoded::parse(request.split_once("\r\n\r\n").unwrap().1.as_bytes())
            .into_owned()
            .collect();
    assert_eq!(fields.len(), 6);
    assert_eq!(fields["client_secret"], "test-app-secret");
    assert_eq!(fields["code"], "the code&");
    assert_eq!(fields["grant_type"], "authorization_code");
    assert_eq!(fields["code_verifier"], "verifier");
    assert_eq!(fields["redirect_uri"], config().redirect_uri());
}

#[tokio::test]
async fn exchange_rejects_malformed_oversized_timeout_and_redirect() {
    let cases = [
        response("secret-invalid-json"),
        response(&"x".repeat(65537)),
        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/leak\r\nContent-Length: 0\r\n\r\n"
            .into(),
    ];
    for fixture in cases {
        let (endpoint, request) = server(fixture, Duration::ZERO).await;
        let client = Client::for_test(config(), endpoint, Duration::from_secs(2));
        let error = client
            .refresh(&Secret::new("refresh-secret".into()))
            .await
            .err()
            .unwrap();
        assert!(!error.to_string().contains("secret"));
        request.await.unwrap();
    }
    let (endpoint, request) = server(
        response(&token_body("a", "b", 3600)),
        Duration::from_millis(100),
    )
    .await;
    let client = Client::for_test(config(), endpoint, Duration::from_millis(10));
    assert!(client
        .refresh(&Secret::new("refresh-secret".into()))
        .await
        .is_err());
    request.await.unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_refresh_performs_one_exchange_and_persists_rotated_credentials() {
    let home = directory();
    let cfg = store_config(home.clone());
    let store = std::sync::Arc::new(Store::open(&cfg).unwrap());
    let old = client::parse_tokens(
        token_body("old-access", "old-refresh", 1).as_bytes(),
        &cfg,
        client::now().unwrap(),
    )
    .unwrap();
    store.save_login(&old).unwrap();
    let (endpoint, request) = server(
        response(&token_body("rotated-access", "rotated-refresh", 3600)),
        Duration::from_millis(50),
    )
    .await;
    let client = std::sync::Arc::new(Client::for_test(
        cfg.clone(),
        endpoint,
        Duration::from_secs(2),
    ));
    let first_store = store.clone();
    let first_client = client.clone();
    let first = tokio::spawn(async move { first_store.access_token(&first_client).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let second_store = store.clone();
    let second_client = client.clone();
    let second = tokio::spawn(async move { second_store.access_token(&second_client).await });
    assert_eq!(first.await.unwrap().unwrap().expose(), "rotated-access");
    assert_eq!(second.await.unwrap().unwrap().expose(), "rotated-access");
    let request = request.await.unwrap();
    assert!(request.contains("refresh_token=old-refresh"));
    let reloaded = Store::open(&cfg).unwrap();
    assert_eq!(
        reloaded.access_token(&client).await.unwrap().expose(),
        "rotated-access"
    );
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn uncertain_refresh_never_replays_old_refresh_token() {
    let home = directory();
    let cfg = store_config(home.clone());
    let store = Store::open(&cfg).unwrap();
    store
        .save_login(
            &client::parse_tokens(
                token_body("old-access", "old-refresh", 1).as_bytes(),
                &cfg,
                client::now().unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let (endpoint, request) = server(response("unknown outcome"), Duration::ZERO).await;
    let client = Client::for_test(cfg.clone(), endpoint, Duration::from_secs(2));
    assert!(store.access_token(&client).await.is_err());
    request.await.unwrap();
    let reopened = Store::open(&cfg).unwrap();
    assert!(matches!(
        reopened.access_token(&client).await,
        Err(Error::LoginRequired)
    ));
    assert_eq!(reopened.status().unwrap(), Status::LoginRequired);
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[test]
fn legacy_abandoned_temporary_file_does_not_block_login() {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let home = directory();
    let cfg = store_config(home.clone());
    let store = Store::open(&cfg).unwrap();
    let mut abandoned = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(home.join(".replace"))
        .unwrap();
    abandoned.write_all(b"incomplete old write").unwrap();
    drop(abandoned);
    let tokens = client::parse_tokens(
        token_body("access", "refresh", 3600).as_bytes(),
        &cfg,
        client::now().unwrap(),
    )
    .unwrap();
    store.save_login(&tokens).unwrap();
    assert!(matches!(store.status().unwrap(), Status::Ready { .. }));
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[test]
fn secret_file_rejects_symlink_insecure_mode_and_oversize() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let home = directory();
    let mut cfg = store_config(home.clone());
    let _store = Store::open(&cfg).unwrap();
    let secret = home.join("secret");
    std::fs::write(&secret, "fixture-secret\n").unwrap();
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
    cfg.secret_file = Some(secret.clone());
    assert_eq!(cfg.load_secret().unwrap().expose(), "fixture-secret");
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(cfg.load_secret().is_err());
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
    let alias = home.join("alias");
    symlink(&secret, &alias).unwrap();
    cfg.secret_file = Some(alias);
    assert!(cfg.load_secret().is_err());
    cfg.secret_file = Some(secret.clone());
    std::fs::write(secret, "s".repeat(4097)).unwrap();
    assert!(cfg.load_secret().is_err());
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn redirect_does_not_forward_secret_to_second_listener() {
    let sink = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect = format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/capture\r\nContent-Length: 0\r\n\r\n", sink.local_addr().unwrap());
    let (endpoint, request) = server(redirect, Duration::ZERO).await;
    let client = Client::for_test(config(), endpoint, Duration::from_secs(2));
    assert!(client
        .refresh(&Secret::new("never-forward".into()))
        .await
        .is_err());
    request.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), sink.accept())
            .await
            .is_err()
    );
}

#[test]
fn strict_callback_encoding_rejects_malformed_percent_and_utf8() {
    for query in [
        "state=expected&code=%",
        "state=expected&code=%0",
        "state=expected&code=%GG",
        "state=expected&code=%FF",
        "state=%FF&code=ok",
    ] {
        assert!(
            callback::parse_callback(
                &format!("GET /callback?{query} HTTP/1.1\r\n\r\n"),
                "expected"
            )
            .is_err(),
            "accepted malformed callback encoding"
        );
    }
}

#[test]
fn token_response_rejects_array_error_and_unsafe_token_text() {
    let valid = token_body("access", "refresh", 3600);
    let cases = [
        r#"[0,"access","refresh",3600,86400,"Bearer","test:read offline_access"]"#.to_string(),
        valid.replacen("\"code\":0", "\"code\":0,\"error\":\"denied\"", 1),
        valid.replacen(
            "\"code\":0",
            "\"code\":0,\"error_description\":\"denied\"",
            1,
        ),
        token_body(" ", "refresh", 3600),
        token_body("access", "ref resh", 3600),
        token_body("access\\n", "refresh", 3600),
    ];
    for body in cases {
        assert!(
            client::parse_tokens(body.as_bytes(), &config(), 100).is_err(),
            "accepted invalid token shape/text"
        );
    }
}

#[test]
fn scope_syntax_and_duplicates_are_rejected() {
    for scope in [
        "test:read\toffline_access",
        "test:read  offline_access",
        "test:read offline_access test:read",
        "test:read offline_access bad\"scope",
        "test:read offline_access bad\\scope",
        "test:read offline_access ☃",
    ] {
        let body = serde_json::json!({"code":0,"access_token":"access","refresh_token":"refresh","expires_in":3600,"refresh_token_expires_in":86400,"token_type":"Bearer","scope":scope});
        assert!(client::parse_tokens(&serde_json::to_vec(&body).unwrap(), &config(), 100).is_err());
    }
    for scopes in [
        vec!["bad\"scope".into()],
        vec!["bad\\scope".into()],
        vec!["test:read".into(), "test:read".into()],
    ] {
        assert!(Config::new(
            "test-app".into(),
            "http://localhost:8080/callback",
            scopes,
            PathBuf::from("/private/tmp/ask")
        )
        .is_err());
    }
}

#[cfg(unix)]
#[test]
fn protected_readonly_secret_is_accepted() {
    use std::os::unix::fs::PermissionsExt;
    let home = directory();
    let mut cfg = store_config(home.clone());
    let _store = Store::open(&cfg).unwrap();
    let secret = home.join("readonly-secret");
    std::fs::write(&secret, "read-only-fixture").unwrap();
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o400)).unwrap();
    cfg.secret_file = Some(secret);
    assert_eq!(cfg.load_secret().unwrap().expose(), "read-only-fixture");
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[test]
fn nonunicode_callback_environment_is_rejected() {
    use std::os::unix::ffi::OsStringExt;
    let old_app = std::env::var_os("STORAGE_CLI_DOUBAO_APP_ID");
    let old_callback = std::env::var_os("STORAGE_CLI_DOUBAO_CALLBACK");
    std::env::set_var("STORAGE_CLI_DOUBAO_APP_ID", "test-app");
    std::env::set_var(
        "STORAGE_CLI_DOUBAO_CALLBACK",
        std::ffi::OsString::from_vec(vec![0xff]),
    );
    let is_rejected = Config::from_env(&["test:read"]).is_err();
    if let Some(value) = old_app {
        std::env::set_var("STORAGE_CLI_DOUBAO_APP_ID", value);
    } else {
        std::env::remove_var("STORAGE_CLI_DOUBAO_APP_ID");
    }
    if let Some(value) = old_callback {
        std::env::set_var("STORAGE_CLI_DOUBAO_CALLBACK", value);
    } else {
        std::env::remove_var("STORAGE_CLI_DOUBAO_CALLBACK");
    }
    assert!(is_rejected);
}

#[cfg(unix)]
#[test]
fn child_refresh_lock_fixture() {
    let Some(home) = std::env::var_os("TOS_ASK_CHILD_LOCK_HOME") else {
        return;
    };
    let store = Store::open(&store_config(PathBuf::from(&home))).unwrap();
    let guard = store.lock().unwrap();
    store.mark_pending(&guard).unwrap();
    std::fs::write(PathBuf::from(home).join("child-ready"), b"ready").unwrap();
    std::thread::sleep(Duration::from_secs(20));
}

#[cfg(unix)]
#[tokio::test]
async fn killed_process_releases_lock_and_unresolved_refresh_cannot_replay() {
    let home = directory();
    let cfg = store_config(home.clone());
    let store = Store::open(&cfg).unwrap();
    store
        .save_login(
            &client::parse_tokens(
                token_body("old-access", "old-refresh", 1).as_bytes(),
                &cfg,
                client::now().unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("ask::tests::child_refresh_lock_fixture")
        .env("TOS_ASK_CHILD_LOCK_HOME", &home)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while !home.join("child-ready").exists() && started.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(10));
    }
    let is_ready = home.join("child-ready").exists();
    let is_busy = matches!(store.lock(), Err(Error::Busy));
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(is_ready && is_busy);
    let reopened = Store::open(&cfg).unwrap();
    let client = Client::for_test(
        cfg,
        "http://127.0.0.1:1/must-not-send".into(),
        Duration::from_millis(100),
    );
    assert!(matches!(
        reopened.access_token(&client).await,
        Err(Error::LoginRequired)
    ));
    std::fs::remove_dir_all(home).unwrap();
}

#[cfg(unix)]
#[test]
fn invalid_utf8_secret_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let home = directory();
    let mut cfg = store_config(home.clone());
    let _store = Store::open(&cfg).unwrap();
    let secret = home.join("invalid-secret");
    std::fs::write(&secret, [0xff]).unwrap();
    std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
    cfg.secret_file = Some(secret);
    assert!(cfg.load_secret().is_err());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn callback_rejects_decoded_control_and_whitespace_codes() {
    for code in ["%00", "%0D%0A", "white+space", "%09", "%7F", "%E2%98%83"] {
        assert!(callback::parse_callback(
            &format!("GET /callback?state=expected&code={code} HTTP/1.1\r\n\r\n"),
            "expected"
        )
        .is_err());
    }
}

#[test]
fn bearer_and_refresh_tokens_reject_non_ascii_text() {
    for body in [
        token_body("access-☃", "refresh", 3600),
        token_body("access", "refresh-😀", 3600),
    ] {
        assert!(client::parse_tokens(body.as_bytes(), &config(), 100).is_err());
    }
}

#[cfg(unix)]
#[test]
fn child_fifo_reader_fixture() {
    let Some(home) = std::env::var_os("TOS_ASK_FIFO_HOME") else {
        return;
    };
    let node = std::env::var("TOS_ASK_FIFO_NODE").unwrap();
    let mut cfg = store_config(PathBuf::from(home));
    let outcome = if node == "secret" {
        cfg.secret_file = Some(cfg.home.join("secret"));
        cfg.load_secret().map(|_| ())
    } else {
        Store::open(&cfg).and_then(|store| store.status().map(|_| ()))
    };
    assert!(matches!(outcome, Err(Error::Storage)));
}

#[cfg(unix)]
fn assert_fifo_reader_returns(home: &std::path::Path, node: &str) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "ask::tests::child_fifo_reader_fixture"])
        .env("TOS_ASK_FIFO_HOME", home)
        .env("TOS_ASK_FIFO_NODE", node)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            return;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("FIFO {node} blocked protected storage read");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn fifo_secrets_keys_tokens_and_markers_fail_without_blocking() {
    use std::os::unix::ffi::OsStrExt;
    for node in ["secret", ".key", "credentials.toml", "pending"] {
        let home = directory();
        let cfg = store_config(home.clone());
        let store = Store::open(&cfg).unwrap();
        store
            .save_login(
                &client::parse_tokens(
                    token_body("access", "refresh", 3600).as_bytes(),
                    &cfg,
                    client::now().unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        let path = if node == "pending" {
            store.state_path("pending").unwrap()
        } else {
            home.join(node)
        };
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_fifo_reader_returns(&home, node);
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn current_thread_concurrent_refresh_yields_while_waiting_for_lock() {
    let home = directory();
    let cfg = store_config(home.clone());
    let store = std::sync::Arc::new(Store::open(&cfg).unwrap());
    store
        .save_login(
            &client::parse_tokens(
                token_body("old-access", "old-refresh", 1).as_bytes(),
                &cfg,
                client::now().unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
    let (endpoint, request) = server(
        response(&token_body("new-access", "new-refresh", 3600)),
        Duration::from_millis(40),
    )
    .await;
    let client = std::sync::Arc::new(Client::for_test(cfg, endpoint, Duration::from_secs(2)));
    let first_store = store.clone();
    let first_client = client.clone();
    let first = tokio::spawn(async move { first_store.access_token(&first_client).await });
    tokio::time::sleep(Duration::from_millis(10)).await;
    let second = store.access_token(&client).await;
    assert_eq!(first.await.unwrap().unwrap().expose(), "new-access");
    assert_eq!(second.unwrap().expose(), "new-access");
    request.await.unwrap();
    std::fs::remove_dir_all(home).unwrap();
}
