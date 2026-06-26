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
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Content, Implementation, InitializeResult,
    ListToolsResult, PaginatedRequestParam, ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::{io::stdio, SseServer};
use rmcp::{ErrorData as McpError, ServiceExt};
use serde_json::Value;

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

    /// Run the server through rmcp's HTTP/SSE transport.
    ///
    /// The server listens on `bind` and exposes the default rmcp endpoints:
    /// `/sse` for the event stream and `/message` for client requests.
    pub async fn run_sse(self, bind: SocketAddr) -> std::io::Result<()> {
        // [Review Fix #20] SSE and stdio must share the same rmcp service; only the transport differs.
        let service = self;
        let ct = SseServer::serve(bind)
            .await?
            .with_service(move || service.clone());
        tokio::signal::ctrl_c().await?;
        ct.cancel();
        Ok(())
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
