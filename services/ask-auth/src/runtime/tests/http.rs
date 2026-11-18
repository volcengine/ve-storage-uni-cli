use super::*;
use axum::routing::get;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{oneshot, Notify},
};

async fn request(address: std::net::SocketAddr) -> Vec<u8> {
    let mut stream = TcpStream::connect(address).await.unwrap();
    stream
        .write_all(b"GET /slow HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    response
}

#[tokio::test]
async fn real_http_inflight_handler_finishes_after_shutdown() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let (handler_entered, handler_release) = (entered.clone(), release.clone());
    let router = Router::new().route(
        "/slow",
        get(move || async move {
            handler_entered.notify_one();
            handler_release.notified().await;
            "finished"
        }),
    );
    let (shutdown, receiver) = watch::channel(false);
    let (signal, signal_receiver) = oneshot::channel();
    let serving = supervise(
        serve_http(listener, router, receiver.clone()),
        until_shutdown(receiver),
        async {
            signal_receiver.await.unwrap();
            Ok(())
        },
        shutdown,
        ready(()),
    );
    let client = request(address);
    let controller = async {
        entered.notified().await;
        signal.send(()).unwrap();
        tokio::task::yield_now().await;
        release.notify_one();
    };
    let (outcome, response, ()) = tokio::join!(serving, client, controller);
    assert_eq!(outcome, Ok(()));
    assert!(response.starts_with(b"HTTP/1.1 200"));
    assert!(response.ends_with(b"finished"));
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn real_http_incomplete_headers_close_within_drain_budget() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, receiver) = watch::channel(false);
    let (signal, signal_receiver) = oneshot::channel();
    let serving = supervise(
        serve_http(listener, crate::build_router(), receiver.clone()),
        until_shutdown(receiver),
        async {
            signal_receiver.await.unwrap();
            Ok(())
        },
        shutdown,
        ready(()),
    );
    let client = async {
        let mut stream = TcpStream::connect(address).await.unwrap();
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nX-Slow: PRIVATE_HEADER")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        signal.send(()).unwrap();
        let mut response = Vec::new();
        let outcome = tokio::time::timeout(
            DRAIN_BUDGET + Duration::from_secs(1),
            stream.read_to_end(&mut response),
        )
        .await;
        assert!(outcome.is_ok());
    };
    let (outcome, ()) = tokio::join!(serving, client);
    assert!(matches!(outcome, Ok(()) | Err(RuntimeError::DrainTimeout)));
}

#[tokio::test]
async fn real_http_pending_handler_is_dropped_at_the_drain_deadline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let entered = Arc::new(Notify::new());
    let is_dropped = Arc::new(AtomicBool::new(false));
    let (handler_entered, handler_dropped) = (entered.clone(), is_dropped.clone());
    let router = Router::new().route(
        "/slow",
        get(move || async move {
            let _probe = DropProbe(handler_dropped);
            handler_entered.notify_one();
            pending::<&'static str>().await
        }),
    );
    let (shutdown, receiver) = watch::channel(false);
    let signal = async {
        entered.notified().await;
        Ok(())
    };
    let started = tokio::time::Instant::now();
    let serving = supervise(
        serve_http(listener, router, receiver.clone()),
        until_shutdown(receiver),
        signal,
        shutdown,
        ready(()),
    );
    let (outcome, response) = tokio::join!(serving, request(address));
    assert_eq!(outcome, Err(RuntimeError::DrainTimeout));
    assert!(is_dropped.load(Ordering::SeqCst));
    assert!(response.is_empty());
    assert!(started.elapsed() >= DRAIN_BUDGET);
    assert!(started.elapsed() < DRAIN_BUDGET + Duration::from_secs(2));
}

#[tokio::test]
async fn cancelling_real_http_drops_pending_handler_socket_and_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let entered = Arc::new(Notify::new());
    let is_dropped = Arc::new(AtomicBool::new(false));
    let (handler_entered, handler_dropped) = (entered.clone(), is_dropped.clone());
    let router = Router::new().route(
        "/slow",
        get(move || async move {
            let _probe = DropProbe(handler_dropped);
            handler_entered.notify_one();
            pending::<&'static str>().await
        }),
    );
    let (_shutdown, receiver) = watch::channel(false);
    let mut serving = Box::pin(serve_http(listener, router, receiver));
    let client = request(address);
    tokio::pin!(client);
    tokio::select! { biased;
        _ = entered.notified() => {},
        _ = &mut serving => panic!("HTTP server exited before cancellation"),
        _ = &mut client => panic!("request finished before cancellation"),
    }
    drop(serving);
    assert!(is_dropped.load(Ordering::SeqCst));
    assert!(client.await.is_empty());
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn disabled_host_serves_health_readiness_and_schema_then_shuts_down() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (signal, receiver) = oneshot::channel();
    let serving = serve_disabled(listener, async {
        receiver.await.unwrap();
        Ok(())
    });
    let client = async {
        for (path, status) in [
            ("/healthz", "200"),
            ("/readyz", "503"),
            ("/v1/auth/login-attempts/not-an-id", "400"),
        ] {
            let mut stream = TcpStream::connect(address).await.unwrap();
            stream
                .write_all(
                    format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
            let mut response = Vec::new();
            stream.read_to_end(&mut response).await.unwrap();
            assert!(String::from_utf8(response)
                .unwrap()
                .starts_with(&format!("HTTP/1.1 {status}")));
        }
        signal.send(()).unwrap();
    };
    let (outcome, ()) = tokio::join!(serving, client);
    assert_eq!(outcome, Ok(()));
    assert!(TcpStream::connect(address).await.is_err());
}

#[tokio::test]
async fn cancelled_unpolled_disabled_host_releases_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(serve_disabled(listener, pending()));
    assert!(TcpStream::connect(address).await.is_err());
}
