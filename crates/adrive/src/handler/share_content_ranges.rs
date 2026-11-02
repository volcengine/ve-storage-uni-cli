//! Concurrent credential-free ranges into one unpublished pinned file.
use super::{
    download_ranges::{run_ranges, DownloadRange, RangePlan},
    share_content::{failed, record_content_response, validate_etag, BandwidthLimiter},
    share_download_progress::RangeProgress,
    share_local::{SafeDir, StagedFile},
    share_manifest::RemoteFile,
    share_range_checkpoint::RangeCheckpoint,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    time::Duration,
};
use tos_core::agent::error::CliError;
use tos_core::agent::request_id::{
    capture_service_request_id, preserve_operation_failure_request_id, ServiceRequestTrace,
};

pub(super) struct RangeDownload<'a> {
    pub address: &'a url::Url,
    pub remote: &'a RemoteFile,
    pub directory: &'a SafeDir,
    pub local: &'a str,
    pub concurrency: usize,
    pub progress: &'a RangeProgress,
    pub limiter: Option<&'a BandwidthLimiter>,
    pub checkpoint_directory: Option<&'a SafeDir>,
    pub request_trace: &'a ServiceRequestTrace,
}

#[derive(Clone, Copy)]
struct RangeOutput<'a> {
    file: &'a File,
    offset: u64,
}

/// Download verified ranges into an exclusive staged file; configured recovery retains completed, hashed ranges on failure/cancellation.
pub(super) async fn download(
    client: &reqwest::Client,
    request: RangeDownload<'_>,
) -> Result<(StagedFile, String), CliError> {
    let plan = RangePlan::new(request.remote.size)?;
    let checkpoint = request
        .checkpoint_directory
        .map(|root| RangeCheckpoint::open(root, request.remote, request.local))
        .transpose()?;
    let mut staged = StagedFile::new(request.directory, request.local)?;
    staged.file.set_len(request.remote.size)?;
    run_ranges(plan, request.concurrency, |range| {
        let file = &staged.file;
        let request = &request;
        let checkpoint = checkpoint.as_ref();
        run_range(
            client,
            request,
            checkpoint,
            RangeOutput {
                file,
                offset: range.offset,
            },
            range,
        )
    })
    .await?;
    if let Some(checkpoint) = checkpoint.as_ref() {
        assemble(checkpoint, &mut staged.file, request.remote.size).await?;
    }
    let verification = verify_file(&mut staged.file, request.remote).await;
    // [Review Fix #SharePart4] A failed whole-file integrity check invalidates the range set, otherwise every resume would replay the same bad bytes.
    if matches!(&verification, Err(CliError::TransferFailed(_))) {
        checkpoint
            .as_ref()
            .map(RangeCheckpoint::clear)
            .transpose()?;
    }
    let digest = verification?;
    Ok((staged, digest))
}

async fn run_range(
    client: &reqwest::Client,
    request: &RangeDownload<'_>,
    checkpoint: Option<&RangeCheckpoint>,
    output: RangeOutput<'_>,
    range: DownloadRange,
) -> Result<(), CliError> {
    // [Review Fix #ShareRangeId2] Keep buffered transfer futures off nested capture scopes' stack without spawning or changing cancellation.
    let (result, request_id) = capture_service_request_id(Box::pin(async {
        match checkpoint {
            Some(checkpoint) => download_part(client, request, checkpoint, range).await,
            None => transfer_range(client, request, output, range)
                .await
                .map(|_| ()),
        }
    }))
    .await;
    if result.is_err() {
        // [Review Fix #ShareRangeId1] Preserve the failing range's own terminal ID, including an authoritative absence after later sibling responses.
        preserve_operation_failure_request_id(request_id.as_deref());
        return result;
    }
    request.progress.completed(range.size);
    Ok(())
}

async fn download_part(
    client: &reqwest::Client,
    request: &RangeDownload<'_>,
    checkpoint: &RangeCheckpoint,
    range: DownloadRange,
) -> Result<(), CliError> {
    if checkpoint.restore(range).await?.is_some() {
        return Ok(());
    }
    let staged = checkpoint.stage(range)?;
    let digest = transfer_range(
        client,
        request,
        RangeOutput {
            file: &staged.file,
            offset: 0,
        },
        range,
    )
    .await?;
    checkpoint.complete(range, staged, &digest)
}

async fn assemble(
    checkpoint: &RangeCheckpoint,
    output: &mut File,
    size: u64,
) -> Result<(), CliError> {
    output.seek(SeekFrom::Start(0))?;
    for range in RangePlan::new(size)? {
        let (mut part, expected_digest) = checkpoint
            .open_completed(range)?
            .ok_or_else(|| failed("verified range is missing during assembly"))?;
        let mut digest = Sha256::new();
        let mut length = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = part.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            length = length.saturating_add(count as u64);
            if length > range.size {
                return Err(failed("verified range changed during assembly"));
            }
            output.write_all(&buffer[..count])?;
            digest.update(&buffer[..count]);
            tokio::task::yield_now().await;
        }
        // [Review Fix #SharePart2] Recheck bytes while copying, closing the window between resume validation and final publication.
        if length != range.size || hex::encode(digest.finalize()) != expected_digest {
            return Err(failed("verified range changed during assembly"));
        }
    }
    Ok(())
}

async fn transfer_range(
    client: &reqwest::Client,
    request: &RangeDownload<'_>,
    output: RangeOutput<'_>,
    range: DownloadRange,
) -> Result<String, CliError> {
    for attempt in 0..3 {
        match attempt_range(client, request, output, range).await {
            Ok(digest) => return Ok(digest),
            Err((_, true)) if attempt < 2 => {
                tokio::time::sleep(Duration::from_millis(200 << attempt)).await
            }
            Err((error, _)) => return Err(error),
        }
    }
    Err(failed("range retry budget exhausted"))
}

async fn attempt_range(
    client: &reqwest::Client,
    request: &RangeDownload<'_>,
    output: RangeOutput<'_>,
    range: DownloadRange,
) -> Result<String, (CliError, bool)> {
    let mut builder = client
        .get(request.address.clone())
        .header(reqwest::header::RANGE, range.header())
        .header(reqwest::header::ACCEPT_ENCODING, "identity");
    if !request.remote.etag.is_empty() && !request.remote.etag.starts_with("W/") {
        builder = builder.header(
            reqwest::header::IF_MATCH,
            format!("\"{}\"", request.remote.etag.trim_matches('"')),
        );
    }
    let response = builder.send().await.map_err(|_| {
        request.request_trace.record_no_response();
        (failed("range connection failed"), true)
    })?;
    record_content_response(request.request_trace, &response);
    let status = response.status();
    if matches!(status.as_u16(), 408 | 429) || status.is_server_error() {
        return Err((
            failed(&format!("range request returned HTTP {}", status.as_u16())),
            true,
        ));
    }
    range
        .validate(
            status.as_u16(),
            response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|value| value.to_str().ok()),
            request.remote.size,
        )
        .map_err(|error| (error, false))?;
    validate_etag(&response, request.remote).map_err(|error| (error, false))?;
    if response
        .content_length()
        .is_some_and(|size| size != range.size)
    {
        return Err((
            failed("range content length differs from planned range"),
            false,
        ));
    }
    receive_range(response, output, range, request.limiter).await
}

async fn receive_range(
    mut response: reqwest::Response,
    output: RangeOutput<'_>,
    range: DownloadRange,
    limiter: Option<&BandwidthLimiter>,
) -> Result<String, (CliError, bool)> {
    let mut written = 0_u64;
    let mut digest = Sha256::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| (failed("range stream interrupted"), true))?
    {
        if chunk.len() as u64 > range.size - written {
            return Err((failed("range exceeds planned length"), false));
        }
        if let Some(limiter) = limiter {
            limiter
                .consume(chunk.len())
                .await
                .map_err(|error| (error, false))?;
        }
        write_at(output.file, &chunk, output.offset + written)
            .map_err(|error| (CliError::Io(error), false))?;
        written += chunk.len() as u64;
        digest.update(&chunk);
    }
    if written != range.size {
        return Err((failed("range body is incomplete"), true));
    }
    Ok(hex::encode(digest.finalize()))
}

fn write_at(file: &File, mut bytes: &[u8], mut offset: u64) -> std::io::Result<()> {
    while !bytes.is_empty() {
        #[cfg(unix)]
        let written = std::os::unix::fs::FileExt::write_at(file, bytes, offset)?;
        #[cfg(windows)]
        let written = std::os::windows::fs::FileExt::seek_write(file, bytes, offset)?;
        if written == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        bytes = &bytes[written..];
        offset += written as u64;
    }
    Ok(())
}

async fn verify_file(file: &mut File, remote: &RemoteFile) -> Result<String, CliError> {
    file.seek(SeekFrom::Start(0))?;
    let mut sha256 = Sha256::new();
    let mut crc64 = crc64fast::Digest::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut length = 0_u64;
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        length += count as u64;
        sha256.update(&buffer[..count]);
        crc64.write(&buffer[..count]);
        tokio::task::yield_now().await;
    }
    if length != remote.size || (remote.crc64 != 0 && crc64.sum64() != remote.crc64) {
        return Err(failed("content size or CRC64 integrity check failed"));
    }
    Ok(hex::encode(sha256.finalize()))
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::{
        domain::share_access::fixture::{reply, Fixture},
        handler::{
            download_ranges::DOWNLOAD_PART_SIZE,
            share_content::{ContentClient, ContentRuntime},
        },
    };
    fn range_response(offset: u64, total: u64, body: &str) -> String {
        format!("HTTP/1.1 206 Fixture\r\nETag: snapshot\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", offset, offset + body.len() as u64 - 1, total, body.len(), body)
    }
    fn root() -> std::path::PathBuf {
        std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("share-range-recovery-{}", ulid::Ulid::new()))
    }
    fn remote() -> RemoteFile {
        RemoteFile {
            path: "remote".into(),
            size: DOWNLOAD_PART_SIZE + 3,
            etag: "snapshot".into(),
            crc64: 0,
            updated_at: 1,
        }
    }
    fn client(root: &std::path::Path) -> ContentClient {
        ContentClient::new()
            .unwrap()
            .configured(
                ContentRuntime {
                    threshold: 1,
                    concurrency: 1,
                    ..Default::default()
                },
                Default::default(),
            )
            .with_checkpoint_directory(&root.join("parts"))
            .unwrap()
    }
    #[tokio::test]
    async fn interrupted_transfer_restores_only_verified_ranges_using_a_fresh_signed_url() {
        let root = root();
        let directory = SafeDir::open(&root.join("out"), true).unwrap();
        let remote = remote();
        let initial = Fixture::new(vec![
            range_response(0, remote.size, &"a".repeat(DOWNLOAD_PART_SIZE as usize)),
            reply(400, "failed"),
        ]);
        assert!(client(&root)
            .download(
                &format!("{}/old?secret=first", initial.endpoint),
                &remote,
                &directory,
                "file"
            )
            .await
            .is_err());
        assert!(!root.join("out/file").exists());
        assert_eq!(initial.finish().len(), 2);
        let resumed = Fixture::new(vec![range_response(DOWNLOAD_PART_SIZE, remote.size, "xyz")]);
        let (staged, digest) = client(&root)
            .download(
                &format!("{}/new?secret=second", resumed.endpoint),
                &remote,
                &directory,
                "file",
            )
            .await
            .unwrap();
        staged.publish(false).unwrap();
        assert_eq!(
            digest,
            hex::encode(Sha256::digest(
                std::fs::read(root.join("out/file")).unwrap()
            ))
        );
        let requests = resumed.finish();
        assert_eq!(requests.len(), 1);
        assert!(requests[0]
            .to_ascii_lowercase()
            .contains("range: bytes=16777216-16777218"));
        assert!(requests[0]
            .to_ascii_lowercase()
            .contains("if-match: \"snapshot\""));
        std::fs::remove_dir_all(root).unwrap();
    }
    async fn read_headers(stream: &mut tokio::net::TcpStream) {
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(count > 0 && bytes.len() < 16 * 1024);
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
    #[tokio::test]
    async fn cancelling_an_active_range_preserves_previously_verified_parts() {
        use tokio::io::AsyncWriteExt;
        let root = root();
        let directory = SafeDir::open(&root.join("out"), true).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!(
            "http://{}/cancel?secret=hidden",
            listener.local_addr().unwrap()
        );
        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let notification = started.clone();
        let remote = remote();
        let response = range_response(0, remote.size, &"a".repeat(DOWNLOAD_PART_SIZE as usize));
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            read_headers(&mut first).await;
            first.write_all(response.as_bytes()).await.unwrap();
            drop(first);
            let (mut second, _) = listener.accept().await.unwrap();
            read_headers(&mut second).await;
            notification.notify_one();
            std::future::pending::<()>().await;
            drop(second);
        });
        let content = client(&root);
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::select! {
                result = content.download(&address, &remote, &directory, "file") => panic!("transfer ended before cancellation: {}", result.is_ok()),
                _ = started.notified() => {}
            }
        }).await.unwrap();
        server.abort();
        assert!(server.await.unwrap_err().is_cancelled());
        assert!(!root.join("out/file").exists());
        let resumed = Fixture::new(vec![range_response(DOWNLOAD_PART_SIZE, remote.size, "xyz")]);
        let (staged, _) = content
            .download(
                &format!("{}/resume", resumed.endpoint),
                &remote,
                &directory,
                "file",
            )
            .await
            .unwrap();
        staged.publish(false).unwrap();
        assert_eq!(resumed.finish().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn a_final_crc_failure_invalidates_all_cached_ranges_before_retry() {
        let root = root();
        let directory = SafeDir::open(&root.join("out"), true).unwrap();
        let mut remote = remote();
        remote.size = 3;
        remote.crc64 = 123;
        for _ in 0..2 {
            let fixture = Fixture::new(vec![range_response(0, 3, "abc")]);
            let result = client(&root)
                .download(
                    &format!("{}/content", fixture.endpoint),
                    &remote,
                    &directory,
                    "file",
                )
                .await;
            assert!(result.is_err());
            assert_eq!(
                fixture.finish().len(),
                1,
                "a failed global checksum must not permanently reuse invalid cached bytes"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod request_trace_tests {
    use super::*;
    use crate::handler::share_content::{ContentClient, ContentRuntime};
    use std::sync::Arc;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };
    use tos_core::agent::request_id::{capture_service_request_id, ServiceRequestTrace};

    fn remote() -> RemoteFile {
        RemoteFile {
            path: "remote".into(),
            size: super::super::download_ranges::DOWNLOAD_PART_SIZE + 3,
            etag: "snapshot".into(),
            crc64: 0,
            updated_at: 1,
        }
    }
    fn traced_client(trace: Arc<ServiceRequestTrace>, concurrency: usize) -> ContentClient {
        ContentClient::new()
            .unwrap()
            .with_request_trace(trace)
            .configured(
                ContentRuntime {
                    threshold: 1,
                    concurrency,
                    ..Default::default()
                },
                Default::default(),
            )
    }
    async fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(count > 0 && bytes.len() < 16 * 1024);
            bytes.extend_from_slice(&buffer[..count]);
        }
        String::from_utf8(bytes).unwrap().to_ascii_lowercase()
    }
    async fn wait_for_responses(trace: &ServiceRequestTrace, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while trace.snapshot().response_count < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("content response was not recorded");
    }
    fn response_headers(is_failed: bool, request_id: Option<&str>) -> String {
        let part = super::super::download_ranges::DOWNLOAD_PART_SIZE;
        let range = if is_failed {
            format!("{part}-{}/{}", part + 2, part + 3)
        } else {
            format!("0-{}/{}", part - 1, part + 3)
        };
        let length = if is_failed {
            "Transfer-Encoding: chunked".into()
        } else {
            format!("Content-Length: {part}")
        };
        let id = request_id
            .map(|id| format!("x-tos-request-id: {id}\r\n"))
            .unwrap_or_default();
        format!("HTTP/1.1 206 Fixture\r\nETag: snapshot\r\nContent-Range: bytes {range}\r\n{length}\r\n{id}Connection: close\r\n\r\n")
    }
    async fn server(
        trace: Arc<ServiceRequestTrace>,
        failure_id: Option<&'static str>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!(
            "http://{}/signed?token=secret",
            listener.local_addr().unwrap()
        );
        let worker = tokio::spawn(async move {
            let mut connections = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request(&mut stream).await;
                connections.push((stream, request));
            }
            connections.sort_by_key(|(_, request)| request.contains("range: bytes=16777216-"));
            let (mut success, _) = connections.remove(0);
            let (mut failed, _) = connections.remove(0);
            let baseline = trace.snapshot().response_count;
            failed
                .write_all(response_headers(true, failure_id).as_bytes())
                .await
                .unwrap();
            wait_for_responses(&trace, baseline + 1).await;
            success
                .write_all(response_headers(false, Some("successful-sibling")).as_bytes())
                .await
                .unwrap();
            wait_for_responses(&trace, baseline + 2).await;
            failed.write_all(b"4\r\nabcd\r\n0\r\n\r\n").await.unwrap();
        });
        (address, worker)
    }
    #[tokio::test]
    async fn failed_range_keeps_its_own_id_after_a_later_successful_sibling_response() {
        for failure_id in [Some("failed-range"), None] {
            let trace = Arc::new(ServiceRequestTrace::default());
            trace.record_response(Some("unrelated-file"), true);
            let (address, worker) = server(trace.clone(), failure_id).await;
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("share-range-trace-{}", ulid::Ulid::new()));
            let directory = SafeDir::open(&root, true).unwrap();
            let content = traced_client(trace.clone(), 2);
            let remote = remote();
            let (result, request_id) = capture_service_request_id(async {
                trace.record_response(Some("get-download-url"), true);
                content
                    .download(&address, &remote, &directory, "file")
                    .await
            })
            .await;
            worker.await.unwrap();
            assert!(result.is_err());
            assert_eq!(request_id.as_deref(), failure_id);
            assert_eq!(
                trace.snapshot().terminal_response_request_id.as_deref(),
                Some("successful-sibling")
            );
            assert_eq!(trace.snapshot().response_count, 4);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[tokio::test]
    async fn successful_two_range_files_use_the_last_content_response_even_without_an_id() {
        use crate::domain::share_access::fixture::Fixture;
        let part = super::super::download_ranges::DOWNLOAD_PART_SIZE;
        for final_id in [Some("range-final"), None] {
            let responses = vec![
                format!(
                    "{}{}",
                    response_headers(false, Some("range-first")),
                    "a".repeat(part as usize)
                ),
                format!("{}3\r\nxyz\r\n0\r\n\r\n", response_headers(true, final_id)),
            ];
            let fixture = Fixture::new(responses);
            let root = std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("share-range-success-trace-{}", ulid::Ulid::new()));
            let directory = SafeDir::open(&root, true).unwrap();
            let trace = Arc::new(ServiceRequestTrace::default());
            let content = traced_client(trace.clone(), 1);
            let remote = remote();
            let (result, request_id) = capture_service_request_id(async {
                trace.record_response(Some("get-download-url"), true);
                content
                    .download(
                        &format!("{}/signed?token=hidden", fixture.endpoint),
                        &remote,
                        &directory,
                        "file",
                    )
                    .await
            })
            .await;
            drop(result.unwrap());
            assert_eq!(request_id.as_deref(), final_id);
            assert_eq!(fixture.finish().len(), 2);
            assert_eq!(trace.snapshot().response_count, 3);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
