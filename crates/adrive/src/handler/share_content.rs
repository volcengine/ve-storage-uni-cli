//! Credential-free streamed transfer; only content requests reuse an issued URL.
use super::{
    share_local::{SafeDir, StagedFile},
    share_manifest::RemoteFile,
};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tos_core::agent::error::CliError;
use tos_core::agent::request_id::{sanitize_request_id, ServiceRequestTrace};

/// Validate an issued HTTP(S) URL without echoing credentials or signed queries on failure.

pub(crate) fn validate_url(address: &str) -> Result<url::Url, CliError> {
    let parsed = url::Url::parse(address).map_err(|_| failed("invalid download address"))?;
    // [Review Fix #ShareProtocol2] Accept the service-issued HTTP(S) scheme without imposing an undocumented HTTPS requirement.
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(failed(
            "download address must use HTTP(S) without credentials or fragment",
        ));
    }
    Ok(parsed)
}
pub(super) fn failed(message: &str) -> CliError {
    CliError::TransferFailed(message.into())
}
pub(crate) struct ContentClient {
    client: reqwest::Client,
    runtime: ContentRuntime,
    display: super::share_download_progress::RangeDisplay,
    checkpoint_directory: Option<SafeDir>,
    request_trace: Arc<ServiceRequestTrace>,
}
#[derive(Clone)]
pub(super) struct ContentRuntime {
    pub threshold: u64,
    pub concurrency: usize,
    pub limiter: Option<Arc<BandwidthLimiter>>,
}
impl Default for ContentRuntime {
    fn default() -> Self {
        Self {
            threshold: super::high_level::parse_size_bytes(
                tos_core::infra::config::DEFAULT_TRANSFER_CHECKPOINT_THRESHOLD,
            )
            .expect("builtin checkpoint threshold is a valid size"),
            concurrency: tos_core::infra::config::DEFAULT_MULTIPART_CONCURRENCY,
            limiter: None,
        }
    }
}
impl ContentRuntime {
    /// Resolve cp-compatible explicit transfer controls without reading anonymous account configuration.
    pub(super) fn for_args(args: &crate::cli::share::ShareDownloadArgs) -> Result<Self, CliError> {
        let threshold = args
            .checkpoint_threshold
            .as_deref()
            .map(super::high_level::parse_size_bytes)
            .unwrap_or(Some(Self::default().threshold))
            .filter(|size| *size > 0)
            .ok_or_else(|| {
                CliError::ValidationError(
                    "checkpoint-threshold must be a positive size, for example 20MB".into(),
                )
            })?;
        let concurrency = args
            .multipart_concurrency
            .unwrap_or(Self::default().concurrency);
        if concurrency == 0 {
            return Err(CliError::ValidationError(
                "multipart-concurrency must be positive".into(),
            ));
        }
        Ok(Self {
            threshold,
            concurrency,
            limiter: BandwidthLimiter::from_limit(args.bandwidth_limit.as_deref())?,
        })
    }
}
impl ContentClient {
    /// Build an independent credential-free content transport with bounded idle/connect waits; return initialization errors.
    pub(crate) fn new() -> Result<Self, CliError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| failed("cannot initialize content transport"))?;
        Ok(Self {
            client,
            runtime: ContentRuntime::default(),
            display: Default::default(),
            checkpoint_directory: None,
            request_trace: Arc::new(ServiceRequestTrace::default()),
        })
    }
    /// Apply validated transfer controls and stderr-only progress for this download operation.
    pub(super) fn configured(
        mut self,
        runtime: ContentRuntime,
        display: super::share_download_progress::RangeDisplay,
    ) -> Self {
        self.runtime = runtime;
        self.display = display;
        self
    }
    /// Attach the invocation's bounded service-ID trace without adding authentication or request headers.
    pub(super) fn with_request_trace(mut self, request_trace: Arc<ServiceRequestTrace>) -> Self {
        self.request_trace = request_trace;
        self
    }
    /// Pin a no-follow range-checkpoint directory beside the journal; reject unsafe components and I/O errors.
    pub(super) fn with_checkpoint_directory(mut self, path: &Path) -> Result<Self, CliError> {
        self.checkpoint_directory = Some(super::share_range_checkpoint::open_directory(path)?);
        Ok(self)
    }
    /// Remove known range copies after publication and journal persistence; return cleanup errors without altering published bytes.
    pub(super) fn clear_completed_ranges(
        &self,
        remote: &RemoteFile,
        local: &str,
    ) -> Result<(), CliError> {
        if remote.size < self.runtime.threshold || !remote.has_identity() {
            return Ok(());
        }
        if let Some(root) = self.checkpoint_directory.as_ref() {
            super::share_range_checkpoint::RangeCheckpoint::open(root, remote, local)?.clear()?;
        }
        Ok(())
    }
    /// Stream this one issued address into a temporary file and return its SHA-256; retry only recoverable content failures.
    pub(crate) async fn download(
        &self,
        address: &str,
        remote: &RemoteFile,
        directory: &SafeDir,
        local: &str,
    ) -> Result<(StagedFile, String), CliError> {
        let parsed = validate_url(address)?;
        // [Review Fix #ShareRange1] Separate Range requests need a pinned version or final expected checksum; otherwise use one representation from a single GET.
        if remote.size >= self.runtime.threshold && remote.has_identity() {
            let progress = self.display.begin(
                remote
                    .size
                    .div_ceil(super::download_ranges::DOWNLOAD_PART_SIZE),
                remote.size,
            );
            return super::share_content_ranges::download(
                &self.client,
                super::share_content_ranges::RangeDownload {
                    address: &parsed,
                    remote,
                    directory,
                    local,
                    concurrency: self.runtime.concurrency,
                    progress: &progress,
                    limiter: self.runtime.limiter.as_deref(),
                    checkpoint_directory: self.checkpoint_directory.as_ref(),
                    request_trace: &self.request_trace,
                },
            )
            .await;
        }
        let progress = self.display.begin(1, remote.size);
        let result = self
            .download_single(&parsed, remote, directory, local)
            .await;
        if result.is_ok() {
            progress.completed(remote.size);
        }
        result
    }
    async fn download_single(
        &self,
        address: &url::Url,
        remote: &RemoteFile,
        directory: &SafeDir,
        local: &str,
    ) -> Result<(StagedFile, String), CliError> {
        for attempt in 0..3 {
            let result = self.attempt(address, remote, directory, local).await;
            match result {
                Ok(result) => return Ok(result),
                Err((error, true)) if attempt < 2 => {
                    let _ = error;
                    tokio::time::sleep(Duration::from_millis(200 << attempt)).await;
                }
                Err((error, _)) => return Err(error),
            }
        }
        Err(failed("content retry budget exhausted"))
    }
    async fn attempt(
        &self,
        address: &url::Url,
        remote: &RemoteFile,
        directory: &SafeDir,
        local: &str,
    ) -> Result<(StagedFile, String), (CliError, bool)> {
        let mut request = self.client.get(address.clone());
        if !remote.etag.is_empty() && !remote.etag.starts_with("W/") {
            request = request.header(
                reqwest::header::IF_MATCH,
                format!("\"{}\"", remote.etag.trim_matches('"')),
            );
        }
        let response = request.send().await.map_err(|_| {
            self.request_trace.record_no_response();
            (failed("content connection failed"), true)
        })?;
        record_content_response(&self.request_trace, &response);
        let status = response.status();
        if !status.is_success() {
            return Err((
                failed(&format!(
                    "content request returned HTTP {}",
                    status.as_u16()
                )),
                status.is_server_error() || status.as_u16() == 429,
            ));
        }
        validate_response(&response, remote).map_err(|error| (error, false))?;
        let mut staged =
            StagedFile::new(directory, local).map_err(|error| (CliError::Io(error), false))?;
        let digest = receive(
            response,
            &mut staged.file,
            remote,
            self.runtime.limiter.as_deref(),
        )
        .await?;
        Ok((staged, digest))
    }
}
/// Record only a usable service ID and HTTP status; signed addresses and response bodies never enter tracing.
pub(super) fn record_content_response(trace: &ServiceRequestTrace, response: &reqwest::Response) {
    // [Review Fix #ShareContentId1] Issued content URLs bypass the API client; record their own terminal service ID instead of reusing URL-issuance IDs.
    let request_id = ["x-tos-request-id", "x-ids-request-id", "x-request-id"]
        .iter()
        .find_map(|name| {
            response
                .headers()
                .get(*name)
                .and_then(|value| value.to_str().ok())
                .and_then(sanitize_request_id)
        });
    trace.record_response(request_id.as_deref(), response.status().is_success());
}
fn validate_response(response: &reqwest::Response, remote: &RemoteFile) -> Result<(), CliError> {
    if response
        .content_length()
        .is_some_and(|length| length != remote.size)
    {
        return Err(failed("content length differs from manifest"));
    }
    validate_etag(response, remote)
}
pub(super) fn validate_etag(
    response: &reqwest::Response,
    remote: &RemoteFile,
) -> Result<(), CliError> {
    if !remote.etag.is_empty() && !remote.etag.starts_with("W/") {
        let received = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok());
        // [Review Fix #Download3] A server ignoring If-Match must not silently bypass the snapshot ETag.
        if received.is_none_or(|etag| etag.trim_matches('"') != remote.etag.trim_matches('"')) {
            return Err(failed("content ETag differs from manifest"));
        }
    }
    Ok(())
}
async fn receive(
    mut response: reqwest::Response,
    file: &mut std::fs::File,
    remote: &RemoteFile,
    limiter: Option<&BandwidthLimiter>,
) -> Result<String, (CliError, bool)> {
    let mut length = 0_u64;
    let mut sha256 = Sha256::new();
    let mut crc64 = crc64fast::Digest::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| (failed("content stream interrupted"), true))?
    {
        length = length
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| (failed("content length overflow"), false))?;
        if length > remote.size {
            return Err((failed("content exceeds manifest size"), false));
        }
        if let Some(limiter) = limiter {
            limiter
                .consume(chunk.len())
                .await
                .map_err(|error| (error, false))?;
        }
        file.write_all(&chunk)
            .map_err(|error| (CliError::Io(error), false))?;
        sha256.update(&chunk);
        crc64.write(&chunk);
    }
    if length != remote.size || (remote.crc64 != 0 && crc64.sum64() != remote.crc64) {
        return Err((
            failed("content size or CRC64 integrity check failed"),
            false,
        ));
    }
    Ok(hex::encode(sha256.finalize()))
}

/// One command-wide byte budget; reservations serialize accounting across all files, ranges and retries.
pub(super) struct BandwidthLimiter {
    bytes_per_second: u64,
    next_available: tokio::sync::Mutex<tokio::time::Instant>,
}
impl BandwidthLimiter {
    fn new(bytes_per_second: u64) -> Self {
        Self {
            bytes_per_second,
            next_available: tokio::sync::Mutex::new(tokio::time::Instant::now()),
        }
    }
    fn from_limit(limit: Option<&str>) -> Result<Option<Arc<Self>>, CliError> {
        let Some(limit) = limit else {
            return Ok(None);
        };
        let bytes_per_second = super::high_level::parse_size_bytes(limit).ok_or_else(|| {
            CliError::ValidationError(
                "bandwidth-limit must be a valid size, for example 100MB".into(),
            )
        })?;
        // [Review Fix #ShareRate1] Match cp: zero disables the limit instead of rejecting a supported configuration.
        Ok((bytes_per_second != 0).then(|| Arc::new(Self::new(bytes_per_second))))
    }
    /// Reserve this chunk against the shared byte rate, yielding until its deadline; report duration overflow.
    pub(super) async fn consume(&self, bytes: usize) -> Result<(), CliError> {
        let seconds = bytes as u64 / self.bytes_per_second;
        let remainder = bytes as u64 % self.bytes_per_second;
        let nanos =
            ((remainder as u128 * 1_000_000_000).div_ceil(self.bytes_per_second as u128)) as u32;
        let duration = Duration::new(seconds, nanos);
        let deadline = {
            let mut next = self.next_available.lock().await;
            *next = (*next)
                .max(tokio::time::Instant::now())
                .checked_add(duration)
                .ok_or_else(|| failed("bandwidth reservation exceeds supported duration"))?;
            *next
        };
        tokio::time::sleep_until(deadline).await;
        Ok(())
    }
}
/// Hash a native no-follow ordinary file within the expected size bound; yield for cancellation and report read/size errors.
pub(crate) async fn local_digest(
    directory: &SafeDir,
    local: &str,
    expected_size: u64,
) -> Result<(u64, String), CliError> {
    let (parent, name) = directory.parent(local, false)?;
    let mut file = parent.read(&name)?;
    let mut length = 0_u64;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        length = length
            .checked_add(count as u64)
            .ok_or_else(|| failed("local size overflow"))?;
        if length > expected_size {
            // [Review Fix #D03] A completed checkpoint no longer matches local bytes; retrying transfer cannot resolve this conflict.
            return Err(CliError::Conflict(
                "completed local file changed since checkpoint".into(),
            ));
        }
        digest.update(&buffer[..count]);
        tokio::task::yield_now().await;
    }
    Ok((length, hex::encode(digest.finalize())))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_http_and_https_but_rejects_credentials_and_non_http_urls() {
        for address in [
            "https://user:pass@example.test/file",
            "http://user:pass@example.test/file",
            "http://example.test/file#secret",
            "file:///etc/passwd",
            "ftp://example.test",
        ] {
            let error = validate_url(address).unwrap_err();
            assert!(!error.to_string().contains("secret"));
            assert!(!error.to_string().contains("pass"));
        }
        assert!(validate_url("http://example.test/file?secret=token").is_ok());
        assert!(validate_url("https://example.test/file?secret=token").is_ok());
        assert!(validate_url("http://127.0.0.1:1234/a").is_ok());
    }
}

#[cfg(test)]
mod integrity_tests {
    use super::*;
    use crate::domain::share_access::fixture::{reply, Fixture};
    #[tokio::test]
    async fn rejects_missing_changed_etag_and_crc_before_publication() {
        for (response, crc) in [
            (reply(200, "abc"), 0),
            (
                reply(200, "abc").replace("Connection: close", "ETag: xyz\r\nConnection: close"),
                0,
            ),
            (
                reply(200, "abc").replace("Connection: close", "ETag: abc\r\nConnection: close"),
                123,
            ),
        ] {
            let fixture = Fixture::new(vec![response]);
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("share-integrity-{}", ulid::Ulid::new()));
            let directory = SafeDir::open(&root, true).unwrap();
            let remote = RemoteFile {
                path: "a".into(),
                size: 3,
                etag: "abc".into(),
                crc64: crc,
                updated_at: 0,
            };
            assert!(ContentClient::new()
                .unwrap()
                .download(
                    &format!("{}/signed?secret=hidden", fixture.endpoint),
                    &remote,
                    &directory,
                    "a"
                )
                .await
                .is_err());
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
            std::fs::remove_dir_all(root).unwrap();
            fixture.finish();
        }
    }
}

#[cfg(test)]
mod bandwidth_tests {
    use super::*;
    #[tokio::test]
    async fn one_bandwidth_budget_is_shared_by_concurrent_consumers() {
        let limiter = std::sync::Arc::new(BandwidthLimiter::new(100));
        let started = tokio::time::Instant::now();
        let (first, second) = tokio::join!(limiter.consume(5), limiter.consume(5));
        first.unwrap();
        second.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(95));
    }
    #[test]
    fn bandwidth_values_require_valid_size_units_and_zero_means_unlimited() {
        assert!(BandwidthLimiter::from_limit(Some("100KB")).is_ok());
        assert!(BandwidthLimiter::from_limit(None).unwrap().is_none());
        assert!(BandwidthLimiter::from_limit(Some("0")).unwrap().is_none());
        assert!(BandwidthLimiter::from_limit(Some("0B")).unwrap().is_none());
        assert!(BandwidthLimiter::from_limit(Some("not-a-size")).is_err());
    }
    #[tokio::test]
    async fn simultaneous_file_streams_share_the_command_bandwidth_limit() {
        assert_command_bandwidth(false).await;
    }
    #[tokio::test]
    async fn simultaneous_range_streams_share_the_command_bandwidth_limit() {
        assert_command_bandwidth(true).await;
    }
    async fn assert_command_bandwidth(is_ranged: bool) {
        use crate::{
            cli::share::ShareDownloadArgs,
            domain::share_access::fixture::{reply, Fixture},
        };
        let response = reply(if is_ranged { 206 } else { 200 }, "abcde").replace(
            "Connection: close",
            "ETag: version\r\nContent-Range: bytes 0-4/5\r\nConnection: close",
        );
        let fixture = Fixture::new(vec![response.clone(), response]);
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-bandwidth-{}", ulid::Ulid::new()));
        let directory = SafeDir::open(&root, true).unwrap();
        let args = ShareDownloadArgs {
            bandwidth_limit: Some("100B".into()),
            checkpoint_threshold: is_ranged.then(|| "1B".into()),
            ..Default::default()
        };
        let runtime = ContentRuntime::for_args(&args).unwrap();
        let clone = runtime.clone();
        assert!(Arc::ptr_eq(
            runtime.limiter.as_ref().unwrap(),
            clone.limiter.as_ref().unwrap()
        ));
        let content = ContentClient::new()
            .unwrap()
            .configured(runtime, Default::default());
        let remote = RemoteFile {
            path: "remote".into(),
            size: 5,
            etag: "version".into(),
            crc64: 0,
            updated_at: 1,
        };
        let address = format!("{}/content", fixture.endpoint);
        let started = tokio::time::Instant::now();
        let (first, second) = tokio::join!(
            content.download(&address, &remote, &directory, "a"),
            content.download(&address, &remote, &directory, "b")
        );
        assert!(started.elapsed() >= Duration::from_millis(95));
        first.unwrap().0.publish(false).unwrap();
        second.unwrap().0.publish(false).unwrap();
        assert_eq!(fixture.finish().len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod request_trace_tests {
    use super::*;
    use crate::domain::share_access::fixture::{reply, Fixture};
    use tos_core::agent::request_id::capture_service_request_id;

    fn fixture_response(headers: &str) -> String {
        reply(200, "abc")
            .replace("x-ids-request-id: fixture-id\r\n", "")
            .replace(
                "Connection: close",
                &format!("ETag: version\r\n{headers}Connection: close"),
            )
    }
    #[tokio::test]
    async fn each_file_uses_its_own_content_id_with_header_priority_and_authoritative_absence() {
        let cases = [
            ("x-tos-request-id: tos-id\r\nx-ids-request-id: ids-id\r\nx-request-id: generic-id\r\n", Some("tos-id")),
            ("x-ids-request-id: ids-id\r\nx-request-id: generic-id\r\n", Some("ids-id")),
            ("x-request-id: generic-id\r\n", Some("generic-id")),
            ("x-tos-request-id: \r\nx-ids-request-id: usable-fallback\r\n", Some("usable-fallback")),
            ("", None),
        ];
        let fixture = Fixture::new(
            cases
                .iter()
                .map(|(headers, _)| fixture_response(headers))
                .collect(),
        );
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-file-trace-{}", ulid::Ulid::new()));
        let directory = SafeDir::open(&root, true).unwrap();
        let trace = Arc::new(ServiceRequestTrace::default());
        let content = ContentClient::new()
            .unwrap()
            .with_request_trace(trace.clone());
        let remote = RemoteFile {
            path: "remote".into(),
            size: 3,
            etag: "version".into(),
            crc64: 0,
            updated_at: 1,
        };
        for (_, expected_id) in cases {
            let (result, request_id) = capture_service_request_id(async {
                trace.record_response(Some("get-download-url"), true);
                content
                    .download(
                        &format!("{}/signed?secret=hidden", fixture.endpoint),
                        &remote,
                        &directory,
                        "file",
                    )
                    .await
            })
            .await;
            drop(result.unwrap());
            assert_eq!(request_id.as_deref(), expected_id);
        }
        assert_eq!(fixture.finish().len(), cases.len());
        assert_eq!(trace.snapshot().response_count, cases.len() * 2);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn an_attempt_without_a_response_clears_a_previous_service_id() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!(
            "http://{}/signed?token=hidden",
            listener.local_addr().unwrap()
        );
        drop(listener);
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-no-response-trace-{}", ulid::Ulid::new()));
        let directory = SafeDir::open(&root, true).unwrap();
        let trace = Arc::new(ServiceRequestTrace::default());
        let content = ContentClient::new()
            .unwrap()
            .with_request_trace(trace.clone());
        let remote = RemoteFile {
            path: "remote".into(),
            size: 3,
            etag: "version".into(),
            crc64: 0,
            updated_at: 1,
        };
        let (result, request_id) = capture_service_request_id(async {
            trace.record_response(Some("previous-get-url"), true);
            content
                .download(&address, &remote, &directory, "file")
                .await
        })
        .await;
        assert!(result.is_err());
        assert_eq!(request_id, None);
        assert!(!trace.snapshot().terminal_response_received);
        assert_eq!(trace.snapshot().response_count, 1);
        std::fs::remove_dir_all(root).unwrap();
    }
}
