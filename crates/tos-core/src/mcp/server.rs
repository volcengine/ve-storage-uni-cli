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

//! rmcp-based MCP server for the TOS unified CLI.
//!
//! This module replaces the previous hand-written JSON-RPC stdio loop with the
//! official `rmcp` 0.8 SDK. The SDK provides:
//! - protocol-version negotiation and capability advertisement,
//! - typed `tools/list` + `tools/call` envelopes (`Tool`, `CallToolResult`),
//! - cancellation, progress, and ping handling for free,
//! - a single transport abstraction shared across stdio, SSE, and child-process
//!   variants, so future transports can be added without rewriting the handler.
//!
//! The CLI keeps its own command dispatch (it already understands clap-derived
//! argv) and exposes that dispatch to the SDK through a [`ToolDispatcher`]
//! callback. Each registered tool advertises a real JSON Schema (built by the
//! caller via `schemars`) instead of the previous empty-object placeholder.
//!
//! # Usage
//!
//! ```ignore
//! use tos_core::mcp::{TosMcpServer, ToolEntry};
//! use rmcp::model::Tool;
//! use std::sync::Arc;
//!
//! let entries = vec![ToolEntry {
//!     tool: Tool::new("tos_ls", "List buckets or objects", Default::default()),
//!     destructive: false,
//! }];
//! let dispatcher: Arc<dyn ToolDispatcher> = Arc::new(my_dispatcher);
//! let server = TosMcpServer::new("ve-storage-uni-cli", env!("CARGO_PKG_VERSION"), entries, dispatcher);
//! server.run_stdio().await?;
//! ```

use std::borrow::Cow;
use std::future::Future;
use std::io::Write;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;

use axum::Router;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Content, Implementation, InitializeResult,
    ListToolsResult, PaginatedRequestParam, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::{io::stdio, sse_server::SseServerConfig, SseServer};
use rmcp::{ErrorData as McpError, ServiceExt};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::http_security::{generate_bearer_token, protect_router, McpHttpSecurity};

/// Boxed future returned by [`ToolDispatcher::dispatch`].
pub type DispatchFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ToolInvocationResult, String>> + Send + 'a>>;

/// Bridge from the SDK's `tools/call` request to the CLI's actual command
/// execution. Implementations are typically a thin `Arc<MyHandler>` wrapper
/// around the existing argv-based dispatcher.
pub trait ToolDispatcher: Send + Sync + 'static {
    fn dispatch<'a>(&'a self, invocation: ToolInvocation) -> DispatchFuture<'a>;
}

/// Inputs handed to a [`ToolDispatcher`] for a single `tools/call`.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    pub name: String,
    pub arguments: Value,
}

/// Outputs returned by a [`ToolDispatcher`]. Mapped 1:1 onto a
/// `CallToolResult` by [`TosMcpServer`].
#[derive(Debug, Clone)]
pub struct ToolInvocationResult {
    /// The structured payload (will be rendered as a JSON text content block).
    pub payload: Value,
    /// `true` if the tool itself reported a logical failure.
    pub is_error: bool,
}

/// One registered tool, comprising the rmcp `Tool` advertisement and a hint
/// flag used to derive `ToolAnnotations.destructiveHint`.
#[derive(Debug, Clone)]
pub struct ToolEntry {
    pub tool: Tool,
    pub destructive: bool,
}

impl ToolEntry {
    /// Convenience constructor that hides the rmcp `Tool` type from callers
    /// outside of `tos-core`. `schema` is treated as a JSON Schema object;
    /// non-object values are silently coerced to an empty schema.
    pub fn from_parts(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
        destructive: bool,
    ) -> Self {
        let schema_obj = match schema {
            Value::Object(map) => map,
            _ => Default::default(),
        };
        let tool = Tool::new(name.into(), description.into(), schema_obj);
        Self { tool, destructive }
    }
}

/// rmcp-backed MCP server for the TOS CLI.
#[derive(Clone)]
pub struct TosMcpServer {
    server_name: String,
    server_version: String,
    tools: Arc<Vec<Tool>>,
    dispatcher: Arc<dyn ToolDispatcher>,
}

struct SseRuntime {
    server: SseServer,
    router: Router,
    token: String,
}

fn build_sse_runtime(bind: SocketAddr) -> SseRuntime {
    let token = generate_bearer_token();
    let config = SseServerConfig {
        bind,
        sse_path: "/sse".to_string(),
        post_path: "/message".to_string(),
        ct: CancellationToken::new(),
        sse_keep_alive: None,
    };
    let (server, router) = SseServer::new(config);
    let router = protect_router(router, McpHttpSecurity::new(&token, bind.port()));
    SseRuntime {
        server,
        router,
        token,
    }
}

fn sse_startup_message(bind: SocketAddr, token: &str) -> String {
    format!("MCP SSE listening on http://{bind}/sse\nAuthorization: Bearer {token}\n")
}

fn write_sse_startup(
    writer: &mut impl Write,
    bind: SocketAddr,
    token: &str,
) -> std::io::Result<()> {
    writer.write_all(sse_startup_message(bind, token).as_bytes())?;
    writer.flush()
}

fn validate_sse_bind(bind: SocketAddr) -> std::io::Result<()> {
    if bind.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "MCP SSE only supports 127.0.0.1",
        ));
    }
    Ok(())
}

impl TosMcpServer {
    /// Create a new server with a static set of tools.
    ///
    /// `entries` describe the available tools (with their `inputSchema` already
    /// populated by the caller). `dispatcher` is invoked for every
    /// `tools/call`.
    pub fn new(
        server_name: impl Into<String>,
        server_version: impl Into<String>,
        entries: Vec<ToolEntry>,
        dispatcher: Arc<dyn ToolDispatcher>,
    ) -> Self {
        let tools: Vec<Tool> = entries
            .into_iter()
            .map(|entry| {
                let mut tool = entry.tool;
                let mut annotations = tool
                    .annotations
                    .clone()
                    .unwrap_or_else(ToolAnnotations::new);
                if annotations.destructive_hint.is_none() {
                    annotations.destructive_hint = Some(entry.destructive);
                }
                if annotations.read_only_hint.is_none() {
                    annotations.read_only_hint = Some(!entry.destructive);
                }
                tool.annotations = Some(annotations);
                tool
            })
            .collect();
        Self {
            server_name: server_name.into(),
            server_version: server_version.into(),
            tools: Arc::new(tools),
            dispatcher,
        }
    }

    /// Run the server on the process's stdin/stdout. Blocks the current task
    /// until the peer disconnects or the server is cancelled.
    pub async fn run_stdio(self) -> std::io::Result<()> {
        let service = self
            .serve(stdio())
            .await
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))?;
        service
            .waiting()
            .await
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err.to_string()))?;
        Ok(())
    }

    /// Run an authenticated rmcp HTTP/SSE server on exact IPv4 loopback.
    ///
    /// After the listener binds, a fresh Bearer credential is written once to
    /// stderr. Every `/sse` and `/message` request must carry that credential
    /// in `Authorization`; the credential is never accepted in the URL.
    pub async fn run_sse(self, bind: SocketAddr) -> std::io::Result<()> {
        validate_sse_bind(bind)?;
        let listener = tokio::net::TcpListener::bind(bind).await?;
        let local_bind = listener.local_addr()?;
        let SseRuntime {
            server,
            router,
            token,
        } = build_sse_runtime(local_bind);

        // [Review Fix #20] SSE and stdio must share the same rmcp service; only the transport differs.
        let service = self;
        let http_shutdown = server.config.ct.child_token();
        let cancellation = server.with_service(move || service.clone());
        let http_server = axum::serve(listener, router).with_graceful_shutdown(async move {
            http_shutdown.cancelled().await;
        });
        let http_future = async move { http_server.await };
        tokio::pin!(http_future);

        // [Review Fix #3] Fail closed if the caller cannot receive the only plaintext credential.
        let output_result = write_sse_startup(&mut std::io::stderr().lock(), local_bind, &token);
        if let Err(error) = output_result {
            cancellation.cancel();
            return Err(error);
        }
        drop(token);
        let result = tokio::select! {
            signal = tokio::signal::ctrl_c() => {
                cancellation.cancel();
                signal?;
                http_future.await
            }
            result = &mut http_future => result,
        };
        cancellation.cancel();
        result
    }
}

impl ServerHandler for TosMcpServer {
    fn get_info(&self) -> ServerInfo {
        InitializeResult {
            protocol_version: ProtocolVersion::default(),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: self.server_name.clone(),
                version: self.server_version.clone(),
                title: None,
                website_url: None,
                icons: None,
            },
            instructions: Some(
                "TOS unified CLI exposed as MCP tools. \
                 All destructive tools require explicit `--force` (and `--confirm` for critical risk). \
                 Use `tos:capabilities` discovery and `--describe` for self-documentation."
                    .to_string(),
            ),
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = (*self.tools).clone();
        async move {
            Ok(ListToolsResult {
                tools,
                next_cursor: None,
            })
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let dispatcher = self.dispatcher.clone();
        let tools = self.tools.clone();
        async move {
            // Reject unknown tool names with a structured MCP error so the
            // caller receives a deterministic exit code (exit 6 in our envelope).
            if !tools.iter().any(|t| t.name == request.name) {
                return Err(McpError::invalid_params(
                    Cow::Owned(format!("unknown tool '{}'", request.name)),
                    None,
                ));
            }
            let arguments = request
                .arguments
                .map(Value::Object)
                .unwrap_or(Value::Object(Default::default()));
            let invocation = ToolInvocation {
                name: request.name.to_string(),
                arguments,
            };
            match dispatcher.dispatch(invocation).await {
                Ok(result) => {
                    let text = serde_json::to_string(&result.payload).unwrap_or_else(|err| {
                        format!("{{\"error\": \"failed to serialize tool result: {err}\"}}")
                    });
                    let content = vec![Content::text(text)];
                    Ok(CallToolResult {
                        content,
                        structured_content: Some(result.payload),
                        is_error: Some(result.is_error),
                        meta: None,
                    })
                }
                Err(err) => {
                    let content = vec![Content::text(err.clone())];
                    Ok(CallToolResult::error(content))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use axum::{
        body::{to_bytes, Body},
        http::{
            header::{AUTHORIZATION, CONTENT_TYPE, HOST},
            Request, StatusCode,
        },
    };
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn prepared_rmcp_router_rejects_anonymous_sse_before_session_creation() {
        let runtime = build_sse_runtime(([127, 0, 0, 1], 19090).into());
        let token = runtime.token.clone();
        let response = runtime
            .router
            .oneshot(
                Request::builder()
                    .uri("/sse")
                    .header(HOST, "127.0.0.1:19090")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&body).contains(&token));
    }

    #[tokio::test]
    async fn prepared_rmcp_router_accepts_authenticated_sse_and_authenticates_messages_first() {
        // [Review Fix #9] Exercise the protected real rmcp routes, not only a dummy router.
        let runtime = build_sse_runtime(([127, 0, 0, 1], 19090).into());
        let authorization = format!("Bearer {}", runtime.token);
        let sse_response = runtime
            .router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/sse")
                    .header(HOST, "127.0.0.1:19090")
                    .header(AUTHORIZATION, &authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(sse_response.status(), StatusCode::OK);
        drop(sse_response);

        let message = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"security-test","version":"1"}}}"#;
        let message_response = runtime
            .router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/message?sessionId=unknown-session")
                    .header(HOST, "127.0.0.1:19090")
                    .header(AUTHORIZATION, authorization)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(message))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(message_response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn startup_message_prints_header_credential_once_without_url_credential() {
        let token = "test-only-token";
        let message = sse_startup_message(([127, 0, 0, 1], 19090).into(), token);

        assert!(message.contains("http://127.0.0.1:19090/sse"));
        assert!(message.contains("Authorization: Bearer test-only-token"));
        assert_eq!(message.matches(token).count(), 1);
        assert!(!message.contains("?token="));
        assert!(!message.contains("access_token="));
    }

    #[test]
    fn startup_writer_propagates_output_failure() {
        struct FailingWriter;

        impl Write for FailingWriter {
            fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "test output failure",
                ))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let error = write_sse_startup(
            &mut FailingWriter,
            ([127, 0, 0, 1], 19090).into(),
            "test-only-token",
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn sse_bind_accepts_only_exact_ipv4_loopback() {
        assert!(validate_sse_bind(([127, 0, 0, 1], 19090).into()).is_ok());
        for bind in [
            ([0, 0, 0, 0], 19090).into(),
            ([127, 0, 0, 2], 19090).into(),
            "[::1]:19090".parse().unwrap(),
        ] {
            let error = validate_sse_bind(bind).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        }
    }
}
