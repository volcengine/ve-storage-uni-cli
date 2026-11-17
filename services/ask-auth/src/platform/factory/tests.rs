use super::*;
use crate::platform::{config::ExpectedEnvironment, control::SourceFuture};
use std::sync::Mutex;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
};
struct CleanupAttempts(std::sync::atomic::AtomicUsize);
impl crate::cleanup::CleanupObserver for CleanupAttempts {
    fn observe(&self, _report: &crate::cleanup::CleanupReport) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
struct PolicyTime;
impl crate::platform::control::PolicyClock for PolicyTime {
    fn now(
        &self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<chrono::NaiveDateTime, AppError>> + Send + '_>,
    > {
        Box::pin(async {
            Ok(chrono::DateTime::from_timestamp_millis(1_800_000_000_000)
                .unwrap()
                .naive_utc())
        })
    }
}
async fn refresh_for_long_interval(
    publisher: &Publisher,
    initial: &HostSnapshot,
    attempts: &CleanupAttempts,
    shutdown: watch::Sender<bool>,
) {
    for _ in 0..10 {
        tokio::time::advance(Duration::from_secs(15)).await;
        tokio::task::yield_now().await;
        publisher.seed(initial).await.unwrap();
        if attempts.0.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            break;
        }
    }
    shutdown.send_replace(true);
}
#[tokio::test]
#[ignore = "requires guarded disposable MySQL URLs and an explicit identity-verifying CA"]
async fn production_tls_equal_control_refresh_does_not_starve_actual_worker() {
    let (runtime_url, _migration_url) = guarded_urls();
    let ca = std::path::PathBuf::from(
        std::env::var("ASK_AUTH_TEST_CA").expect("explicit test CA required"),
    );
    let mut initial = configured_snapshot(&runtime_url, &ca);
    initial.cleanup = Some(zeroize::Zeroizing::new(
        crate::platform::control::tests::snapshot()
            .cleanup
            .unwrap()
            .replace("\"interval_seconds\":10", "\"interval_seconds\":60"),
    ));
    let active = settings::runtime(&initial.runtime, ExpectedEnvironment::Boe)
        .unwrap()
        .unwrap();
    let prepared = Prepared::new(active, &initial).unwrap();
    let database = CleanupDatabase::connect(prepared.cleanup).await.unwrap();
    let store =
        Arc::new(CleanupStore::new(database, &prepared.active.app_id, prepared.crypto).unwrap());
    store.close().await;
    let (_admission, admission_sender) = LiveAdmission::channel();
    let (control, cleanup_sender) = CleanupControl::channel();
    let publisher = Publisher {
        source: Arc::new(Source(Mutex::new(None))),
        clock: Arc::new(PolicyTime),
        controls: Controls {
            admission: admission_sender,
            cleanup: cleanup_sender,
        },
        fingerprint: Fingerprint::capture(&initial),
        environment: ExpectedEnvironment::Boe,
    };
    publisher.seed(&initial).await.unwrap();
    let attempts = Arc::new(CleanupAttempts(std::sync::atomic::AtomicUsize::new(0)));
    let mut worker = CleanupWorker::new(store, control, attempts.clone());
    let (shutdown, receiver) = watch::channel(false);
    tokio::time::pause();
    let refresh = refresh_for_long_interval(&publisher, &initial, &attempts, shutdown);
    tokio::join!(worker.run(receiver), refresh);
    assert!(
        attempts.0.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "actual worker never reached a pass under unchanged 15-second refreshes"
    );
}

fn guarded_urls() -> (String, String) {
    let runtime = std::env::var("ASK_AUTH_TEST_DATABASE_URL")
        .expect("explicit disposable runtime URL required");
    let migration = std::env::var("ASK_AUTH_TEST_MIGRATION_URL")
        .expect("explicit disposable migration URL required");
    for value in [&runtime, &migration] {
        let parsed =
            url::Url::parse(value).unwrap_or_else(|_| panic!("invalid disposable database target"));
        assert!(
            parsed.scheme() == "mysql"
                && parsed.host_str() == Some("127.0.0.1")
                && parsed.path() == "/ask_auth_stage3_test"
                && parsed.query().is_none()
                && parsed.fragment().is_none(),
            "unsafe disposable database target"
        );
    }
    (runtime, migration)
}
fn configured_snapshot(database_url: &str, ca: &std::path::Path) -> HostSnapshot {
    let mut initial = crate::platform::control::tests::snapshot();
    let mut runtime: serde_json::Value = serde_json::from_str(&initial.runtime).unwrap();
    runtime["database_ca_path"] = ca.to_string_lossy().as_ref().into();
    initial.runtime = zeroize::Zeroizing::new(serde_json::to_string(&runtime).unwrap());
    let mut secrets: serde_json::Value =
        serde_json::from_str(initial.secrets.as_ref().unwrap()).unwrap();
    secrets["database_url"] = database_url.into();
    initial.secrets = Some(zeroize::Zeroizing::new(
        serde_json::to_string(&secrets).unwrap(),
    ));
    initial.cleanup = Some(zeroize::Zeroizing::new(
        r#"{"schema_version":1,"is_enabled":false}"#.into(),
    ));
    initial
}
struct Source(Mutex<Option<HostSnapshot>>);
impl ConfigSource for Source {
    fn read(&self) -> SourceFuture<'_> {
        Box::pin(async { self.0.lock().unwrap().take().ok_or(AppError::Source) })
    }
}
async fn status(address: std::net::SocketAddr, path: &str) -> u16 {
    let mut stream = TcpStream::connect(address)
        .await
        .expect("local listener connection failed");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    std::str::from_utf8(&bytes)
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}
#[tokio::test]
#[ignore = "requires guarded disposable MySQL URLs and an explicit identity-verifying CA"]
async fn production_tls_factory_serves_health_readiness_and_closes_exact_pools() {
    let (runtime_url, _migration_url) = guarded_urls();
    let ca = std::path::PathBuf::from(
        std::env::var("ASK_AUTH_TEST_CA").expect("explicit test CA required"),
    );
    assert!(ca.is_absolute() && ca.is_file(), "invalid explicit test CA");
    let initial = configured_snapshot(&runtime_url, &ca);
    let source: Arc<dyn ConfigSource> = Arc::new(Source(Mutex::new(Some(configured_snapshot(
        &runtime_url,
        &ca,
    )))));
    let active = settings::runtime(&initial.runtime, ExpectedEnvironment::Boe)
        .unwrap()
        .unwrap();
    let host = build(active, &initial, source, ExpectedEnvironment::Boe)
        .await
        .unwrap();
    let api = host.runtime;
    let policy_clock = host.publisher.clock.clone();
    assert!(policy_clock.now().await.is_ok());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, receiver) = watch::channel(false);
    let server = api.serve(
        listener,
        crate::platform::lifecycle::runtime_signal(receiver),
    );
    let client = async {
        assert_eq!(status(address, "/healthz").await, 200);
        assert_eq!(status(address, "/readyz").await, 200);
        shutdown.send_replace(true);
    };
    let (result, ()) = tokio::join!(server, client);
    result.unwrap();
    assert_eq!(policy_clock.now().await.err(), Some(AppError::Database));
    assert!(host.publisher.seed(&initial).await.is_err());
}
#[tokio::test]
#[ignore = "requires guarded disposable MySQL URLs and an explicit identity-verifying CA"]
async fn production_tls_migration_command_is_separate_and_idempotent() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let (_runtime_url, migration_url) = guarded_urls();
    let ca = std::env::var("ASK_AUTH_TEST_CA").expect("explicit test CA required");
    let path =
        std::env::temp_dir().join(format!("ask-auth-operator-{}.json", uuid::Uuid::now_v7()));
    let document=zeroize::Zeroizing::new(serde_json::json!({"schema_version":1,"database_url":migration_url,"database_ca_path":ca,"maximum_connections":1}).to_string());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    file.write_all(document.as_bytes()).unwrap();
    drop(file);
    let first = crate::platform::migration::run(&path).await;
    let second = crate::platform::migration::run(&path).await;
    std::fs::remove_file(path).unwrap();
    first.unwrap();
    second.unwrap();
}
