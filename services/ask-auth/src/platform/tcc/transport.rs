//! Authenticated BConfig V1 wire transport over shared Consul discovery.
use super::{
    credentials::CredentialSource, ObjectOnly, RawDocument, RawReader, ReadFuture, SensitiveText,
    TccError, MAX_CONTENT_BYTES, MAX_META_BYTES,
};
use crate::platform::config::runtime::RuntimeContext;
use reqwest::Client;
use serde::Deserialize;
use std::{future::Future, net::SocketAddr, pin::Pin, time::Duration};
use tos_core::infra::discovery::{choose_weighted_addr, ConsulLookupClient};
use zeroize::Zeroizing;

const DISCOVERY_TIMEOUT: Duration = Duration::from_millis(500);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_WIRE_BYTES: usize = 16 * 1024 * 1024;
type ResolveFuture<'a> = Pin<Box<dyn Future<Output = Result<SocketAddr, TccError>> + Send + 'a>>;
trait Resolver: Send + Sync {
    fn resolve<'a>(&'a self, context: &'a RuntimeContext) -> ResolveFuture<'a>;
}
struct ConsulResolver(ConsulLookupClient);
impl Resolver for ConsulResolver {
    fn resolve<'a>(&'a self, context: &'a RuntimeContext) -> ResolveFuture<'a> {
        Box::pin(async move {
            let endpoints = self
                .0
                .lookup(context.discovery_request())
                .await
                .map_err(|_| TccError::Unavailable)?;
            choose_weighted_addr(&endpoints).ok_or(TccError::Unavailable)
        })
    }
}
pub(super) struct HttpReader {
    http: Client,
    context: RuntimeContext,
    credentials: CredentialSource,
    resolver: Box<dyn Resolver>,
}
impl HttpReader {
    pub(super) fn new(context: RuntimeContext) -> Result<Self, TccError> {
        Ok(Self {
            http: build_http_client()?,
            context,
            credentials: CredentialSource::from_env()?,
            resolver: Box::new(ConsulResolver(
                ConsulLookupClient::from_env_with_timeout(DISCOVERY_TIMEOUT)
                    .map_err(|_| TccError::Unavailable)?,
            )),
        })
    }
    async fn fetch(&self, key: &str) -> Result<RawDocument, TccError> {
        let initial = self.credentials.load().await?;
        let address = tokio::time::timeout(DISCOVERY_TIMEOUT, self.resolver.resolve(&self.context))
            .await
            .map_err(|_| TccError::Unavailable)??;
        let url = format!("http://{address}/v1/keys/{}", key.trim_start_matches('/'));
        let response = self
            .http
            .get(url)
            .headers(self.context.headers())
            .header("x-dps-token", initial.header()?)
            .send()
            .await
            .map_err(|_| TccError::Unavailable)?;
        if !response.status().is_success() {
            return Err(TccError::Unavailable);
        }
        let bytes = bounded_body(response).await?;
        self.credentials.check_current(&initial).await?;
        decode_wire(bytes.as_slice(), key)
    }
}
impl RawReader for HttpReader {
    fn read<'a>(&'a self, key: &'a str) -> ReadFuture<'a> {
        Box::pin(async move { self.fetch(key).await })
    }
}
fn build_http_client() -> Result<Client, TccError> {
    // [Review Fix #99] Never forward workload credentials through proxies, redirects, referrers, or automatic retry paths.
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .retry(reqwest::retry::never())
        .connect_timeout(DISCOVERY_TIMEOUT)
        .read_timeout(REQUEST_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .user_agent(concat!("ask-auth/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| TccError::Unavailable)
}
async fn bounded_body(mut response: reqwest::Response) -> Result<Zeroizing<Vec<u8>>, TccError> {
    if response
        .content_length()
        .is_some_and(|size| size > MAX_WIRE_BYTES as u64)
    {
        return Err(TccError::InvalidSnapshot);
    }
    // [Review Fix #100] Allocate once before plaintext arrives; enforce the same bound for chunked or unannounced bodies.
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_WIRE_BYTES));
    while let Some(chunk) = response.chunk().await.map_err(|_| TccError::Unavailable)? {
        if chunk.len() > MAX_WIRE_BYTES - bytes.len() {
            return Err(TccError::InvalidSnapshot);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
#[derive(Deserialize)]
struct WireRoot {
    data: ObjectOnly<WireDocument>,
}
#[derive(Deserialize)]
struct WireDocument {
    key: String,
    value: SensitiveText,
}
fn decode_wire(bytes: &[u8], key: &str) -> Result<RawDocument, TccError> {
    // [Review Fix #101] Prevalidate outer JSON too: unknown fields cannot hide unbounded depth or positional schemas.
    crate::json::check_json_depth(bytes).map_err(|_| TccError::InvalidSnapshot)?;
    let root: ObjectOnly<WireRoot> =
        serde_json::from_slice(bytes).map_err(|_| TccError::InvalidSnapshot)?;
    let document = RawDocument {
        key: root.0.data.0.key,
        value: root.0.data.0.value,
    };
    let limit = if key.starts_with("/tcc/v2/meta/") {
        MAX_META_BYTES
    } else {
        MAX_CONTENT_BYTES
    };
    super::check_document(&document, key, limit)?;
    Ok(document)
}
#[cfg(test)]
mod tests;
