/*
 * Copyright (c) 2025 Beijing Volcano Engine Technology Co., Ltd.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

use std::path::Path;

use crate::infra::zti_credentials::ZtiTokenError;

#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use hyper_util::rt::TokioIo;
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(unix)]
use tonic::{
    codec::ProstCodec,
    transport::{Endpoint, Uri},
    Request,
};

#[cfg(unix)]
#[derive(Clone, PartialEq, prost::Message)]
struct JwtSvidRequest {
    #[prost(string, repeated, tag = "1")]
    audience: Vec<String>,
    #[prost(string, tag = "2")]
    spiffe_id: String,
}

#[cfg(unix)]
// [Review Fix #5] prost otherwise generates Debug output containing JWT-SVIDs.
#[derive(Clone, PartialEq, prost::Message)]
#[prost(skip_debug)]
struct JwtSvidResponse {
    #[prost(message, repeated, tag = "1")]
    svids: Vec<JwtSvid>,
}

#[cfg(unix)]
#[derive(Clone, PartialEq, prost::Message)]
#[prost(skip_debug)]
struct JwtSvid {
    #[prost(string, tag = "1")]
    spiffe_id: String,
    #[prost(string, tag = "2")]
    svid: String,
    #[prost(string, tag = "3")]
    hint: String,
}

#[cfg(unix)]
impl std::fmt::Debug for JwtSvidResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JwtSvidResponse")
            .field("svid_count", &self.svids.len())
            .finish()
    }
}

#[cfg(unix)]
impl std::fmt::Debug for JwtSvid {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("JwtSvid([REDACTED])")
    }
}

#[cfg(unix)]
const FETCH_JWT_SVID_PATH: &str = "/workloadapi.SpiffeWorkloadAPI/FetchJWTSVID";
#[cfg(unix)]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Fetches a JWT-SVID for the `zti` audience from a SPIFFE Workload API Unix socket.
///
/// `socket_path` is the agent socket path. Returns the first SVID token when nonblank,
/// or a closed error category on connection, protocol, or deadline failure.
/// Raw socket paths, agent errors, and tokens are never included in errors.
#[cfg(unix)]
pub(crate) async fn fetch_token(socket_path: &Path) -> Result<String, ZtiTokenError> {
    tokio::time::timeout(REQUEST_TIMEOUT, fetch_token_inner(socket_path))
        .await
        .unwrap_or(Err(ZtiTokenError::Timeout))
}

#[cfg(unix)]
async fn fetch_token_inner(socket_path: &Path) -> Result<String, ZtiTokenError> {
    let path = socket_path.to_path_buf();
    let channel = Endpoint::from_static("http://[::]:50051")
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let path = path.clone();
            async move { UnixStream::connect(path).await.map(TokioIo::new) }
        }))
        .await
        .map_err(|_| ZtiTokenError::Unavailable)?;

    let mut grpc = tonic::client::Grpc::new(channel);
    grpc.ready().await.map_err(|_| ZtiTokenError::Unavailable)?;
    let mut request = Request::new(JwtSvidRequest {
        audience: vec!["zti".to_string()],
        spiffe_id: String::new(),
    });
    request.metadata_mut().insert(
        "workload.spiffe.io",
        tonic::metadata::MetadataValue::from_static("true"),
    );
    let response: tonic::Response<JwtSvidResponse> = grpc
        .unary(
            request,
            http::uri::PathAndQuery::from_static(FETCH_JWT_SVID_PATH),
            ProstCodec::default(),
        )
        .await
        .map_err(|status: tonic::Status| match status.code() {
            tonic::Code::DeadlineExceeded => ZtiTokenError::Timeout,
            _ => ZtiTokenError::Unavailable,
        })?;
    token_from_response(response.into_inner())
}

/// Reports that the Unix Workload API transport is unavailable on this platform.
///
/// `socket_path` is ignored. The function always returns `Unavailable`.
#[cfg(not(unix))]
pub(crate) async fn fetch_token(_socket_path: &Path) -> Result<String, ZtiTokenError> {
    Err(ZtiTokenError::Unavailable)
}

#[cfg(unix)]
fn token_from_response(response: JwtSvidResponse) -> Result<String, ZtiTokenError> {
    let token = response
        .svids
        .into_iter()
        .next()
        .ok_or(ZtiTokenError::Unavailable)?
        .svid;
    if token.trim().is_empty() {
        return Err(ZtiTokenError::Unavailable);
    }
    Ok(token)
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        convert::Infallible,
        path::Path,
        sync::{Arc, Mutex},
        task::{Context, Poll},
        time::Duration,
    };

    use tokio::sync::oneshot;
    use tonic::codegen::{Body, BoxFuture, StdError};

    use super::{fetch_token, token_from_response, JwtSvid, JwtSvidRequest, JwtSvidResponse};
    use crate::infra::zti_credentials::ZtiTokenError;

    #[cfg(unix)]
    #[derive(Debug)]
    struct ObservedRequest {
        path: String,
        audience: Vec<String>,
        spiffe_id: String,
        metadata: Option<String>,
    }

    #[cfg(unix)]
    struct MockState {
        response: Result<JwtSvidResponse, tonic::Status>,
        observed_sender: Mutex<Option<oneshot::Sender<ObservedRequest>>>,
    }

    #[cfg(unix)]
    #[derive(Clone)]
    struct MockWorkloadService {
        state: Arc<MockState>,
    }

    #[cfg(unix)]
    struct MockUnary {
        state: Arc<MockState>,
        path: String,
    }

    #[cfg(unix)]
    impl tonic::server::UnaryService<JwtSvidRequest> for MockUnary {
        type Response = JwtSvidResponse;
        type Future = BoxFuture<tonic::Response<Self::Response>, tonic::Status>;

        fn call(&mut self, request: tonic::Request<JwtSvidRequest>) -> Self::Future {
            let metadata = request
                .metadata()
                .get("workload.spiffe.io")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let body = request.into_inner();
            let observed = ObservedRequest {
                path: self.path.clone(),
                audience: body.audience,
                spiffe_id: body.spiffe_id,
                metadata,
            };
            if let Some(sender) = self.state.observed_sender.lock().unwrap().take() {
                let _ = sender.send(observed);
            }
            let response = self.state.response.clone();
            Box::pin(async move { response.map(tonic::Response::new) })
        }
    }

    #[cfg(unix)]
    impl<B> tonic::codegen::Service<http::Request<B>> for MockWorkloadService
    where
        B: Body + Send + 'static,
        B::Error: Into<StdError> + Send + 'static,
    {
        type Response = http::Response<tonic::body::BoxBody>;
        type Error = Infallible;
        type Future = BoxFuture<Self::Response, Self::Error>;

        fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn call(&mut self, request: http::Request<B>) -> Self::Future {
            let method = MockUnary {
                state: Arc::clone(&self.state),
                path: request.uri().path().to_string(),
            };
            Box::pin(async move {
                let mut grpc = tonic::server::Grpc::new(tonic::codec::ProstCodec::default());
                Ok(grpc.unary(method, request).await)
            })
        }
    }

    #[cfg(unix)]
    impl tonic::server::NamedService for MockWorkloadService {
        const NAME: &'static str = "workloadapi.SpiffeWorkloadAPI";
    }

    #[cfg(unix)]
    async fn fetch_from_mock(
        response: Result<JwtSvidResponse, tonic::Status>,
    ) -> (Result<String, ZtiTokenError>, ObservedRequest) {
        use tokio_stream::wrappers::UnixListenerStream;

        // [Review Fix #1] Keep the mock socket under the Unix path-length limit on macOS.
        let socket_path = Path::new("/tmp").join(format!(
            "tos-zti-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = tokio::net::UnixListener::bind(&socket_path).unwrap();
        let (observed_sender, observed_receiver) = oneshot::channel();
        let (shutdown_sender, shutdown_receiver) = oneshot::channel::<()>();
        let service = MockWorkloadService {
            state: Arc::new(MockState {
                response,
                observed_sender: Mutex::new(Some(observed_sender)),
            }),
        };
        let server = tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(service)
                .serve_with_incoming_shutdown(UnixListenerStream::new(listener), async {
                    let _ = shutdown_receiver.await;
                })
                .await
                .unwrap();
        });
        let result = fetch_token(&socket_path).await;
        // [Review Fix #2] A failed request must fail the test instead of hanging forever.
        let observed = tokio::time::timeout(Duration::from_secs(2), observed_receiver)
            .await
            .unwrap()
            .unwrap();
        let _ = shutdown_sender.send(());
        server.await.unwrap();
        std::fs::remove_file(socket_path).unwrap();
        (result, observed)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_grpc_fetch_uses_spiffe_path_audience_and_metadata() {
        let response = JwtSvidResponse {
            svids: vec![JwtSvid {
                spiffe_id: "spiffe://example.test/workload".into(),
                svid: "SECRET_TOKEN".into(),
                hint: String::new(),
            }],
        };
        let (token, observed) = fetch_from_mock(Ok(response)).await;

        assert_eq!(token, Ok("SECRET_TOKEN".into()));
        assert_eq!(observed.path, "/workloadapi.SpiffeWorkloadAPI/FetchJWTSVID");
        assert_eq!(observed.audience, ["zti"]);
        assert!(observed.spiffe_id.is_empty());
        assert_eq!(observed.metadata.as_deref(), Some("true"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn grpc_status_text_cannot_leak_token() {
        let status = tonic::Status::unavailable("SECRET_TOKEN_FROM_AGENT");
        let (result, _) = fetch_from_mock(Err(status)).await;

        let error = result.unwrap_err();
        assert_eq!(error, ZtiTokenError::Unavailable);
        assert!(!format!("{error:?}").contains("SECRET_TOKEN_FROM_AGENT"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn missing_unix_socket_is_unavailable_and_does_not_reveal_path() {
        let socket_path = Path::new("/tmp").join(format!(
            "zti-agent-secret-missing-{}.sock",
            std::process::id()
        ));
        let error = fetch_token(&socket_path).await.unwrap_err();

        assert_eq!(error, ZtiTokenError::Unavailable);
        assert!(!format!("{error:?}").contains("zti-agent-secret"));
    }

    #[test]
    fn empty_response_is_unavailable() {
        let response = JwtSvidResponse { svids: vec![] };
        assert_eq!(
            token_from_response(response),
            Err(ZtiTokenError::Unavailable)
        );
    }

    #[test]
    fn empty_first_token_is_unavailable_even_if_later_token_exists() {
        let response = JwtSvidResponse {
            svids: vec![
                JwtSvid {
                    spiffe_id: "spiffe://example.test/first".into(),
                    svid: " ".into(),
                    hint: String::new(),
                },
                JwtSvid {
                    spiffe_id: "spiffe://example.test/second".into(),
                    svid: "SECRET_SECOND_TOKEN".into(),
                    hint: String::new(),
                },
            ],
        };
        let error = token_from_response(response).unwrap_err();
        assert_eq!(error, ZtiTokenError::Unavailable);
        assert!(!format!("{error:?}").contains("SECRET_SECOND_TOKEN"));
    }

    #[test]
    fn first_token_is_returned_without_normalization() {
        let response = JwtSvidResponse {
            svids: vec![JwtSvid {
                spiffe_id: "spiffe://example.test/first".into(),
                svid: " SECRET_TOKEN ".into(),
                hint: String::new(),
            }],
        };
        assert_eq!(token_from_response(response), Ok(" SECRET_TOKEN ".into()));
    }

    #[test]
    fn response_debug_redacts_identity_and_token() {
        let response = JwtSvidResponse {
            svids: vec![JwtSvid {
                spiffe_id: "SECRET_SPIFFE_ID".into(),
                svid: "SECRET_TOKEN".into(),
                hint: "SECRET_HINT".into(),
            }],
        };
        let output = format!("{response:?}");
        assert!(!output.contains("SECRET_"));
        assert!(output.contains("svid_count"));
    }
}
