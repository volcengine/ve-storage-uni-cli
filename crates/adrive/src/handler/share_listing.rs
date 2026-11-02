//! Bounded current-level share listings preserve every returned continuation boundary.
use super::high_level::normalize_ls_files_and_folders;
use super::share_receive::ShareReceiver;
use crate::cli::share::ShareFilesArgs;
use crate::domain::{share_types::ShareListFilesInput, types::ListFilesOutput};
use std::collections::HashSet;
use tos_core::agent::error::CliError;

const MAX_SERVICE_PAGE_SIZE: usize = 1000;
const MAX_LISTING_PAGES: usize = 10_000;
const MAX_MARKER_BYTES: usize = 4096;

/// Validate the result cap and initial cursor before URL, credential or password I/O.
/// Returns a validation error for a nonpositive cap or an invalid marker.
pub(super) fn validate_arguments(args: &ShareFilesArgs) -> Result<(), CliError> {
    if args.max_keys <= 0 {
        return Err(CliError::ValidationError(
            "--max-keys must be positive".into(),
        ));
    }
    if let Some(marker) = &args.marker {
        validate_marker(marker)?;
    }
    Ok(())
}

/// Aggregate normalized files and folders until the combined cap or true listing end.
/// Renews expired share sessions per page; returns service or invalid-pagination errors.
pub(super) async fn list_files(
    receiver: &mut ShareReceiver,
    args: &ShareFilesArgs,
) -> Result<ListFilesOutput, CliError> {
    let mut listing = Listing::new(args);
    loop {
        let input = listing.next_request(args)?;
        // [Review Fix #ShareFilesSession1] Each page must renew an expired share token while retaining the pinned recipient.
        let page = receiver.list_page(input).await?;
        listing.accept(page, args.path.as_deref().filter(|path| *path != "/"))?;
        if listing.is_finished() {
            return Ok(listing.output);
        }
    }
}

struct Listing {
    output: ListFilesOutput,
    max_keys: usize,
    marker: Option<String>,
    markers: HashSet<String>,
    file_paths: HashSet<String>,
    folder_paths: HashSet<String>,
    pages: usize,
}

impl Listing {
    fn new(args: &ShareFilesArgs) -> Self {
        Self {
            output: ListFilesOutput {
                response_info: Default::default(),
                files: Vec::new(),
                folders: Vec::new(),
                next_marker: String::new(),
                is_truncated: false,
            },
            max_keys: args.max_keys as usize,
            marker: args.marker.clone(),
            markers: args.marker.iter().cloned().collect(),
            file_paths: HashSet::new(),
            folder_paths: HashSet::new(),
            pages: 0,
        }
    }

    fn count(&self) -> usize {
        self.output.files.len() + self.output.folders.len()
    }

    fn page_limit(&self) -> usize {
        (self.max_keys - self.count()).min(MAX_SERVICE_PAGE_SIZE)
    }

    fn next_request(&mut self, args: &ShareFilesArgs) -> Result<ShareListFilesInput, CliError> {
        if self.pages >= MAX_LISTING_PAGES {
            return Err(invalid("listing exceeded the 10000-page safety bound"));
        }
        self.pages += 1;
        Ok(ShareListFilesInput {
            prefix: args.path.clone(),
            delimiter: Some("/".into()),
            marker: self.marker.clone(),
            limit: Some(self.page_limit() as i64),
            list_root: Some(args.path.is_none()),
        })
    }

    fn accept(&mut self, mut page: ListFilesOutput, prefix: Option<&str>) -> Result<(), CliError> {
        self.validate_pagination(&page)?;
        normalize_page(&mut page, prefix.unwrap_or(""));
        page.files
            .retain(|file| self.file_paths.insert(file.file_path.clone()));
        page.folders
            .retain(|folder| self.folder_paths.insert(folder_key(&folder.folder)));
        // [Review Fix #ShareFilesCursor1] Trimming an overfull page would advance beyond entries never returned to the caller.
        if page.files.len() + page.folders.len() > self.page_limit() {
            return Err(invalid(
                "service returned more entries than the requested page size",
            ));
        }
        self.output.files.extend(page.files);
        self.output.folders.extend(page.folders);
        self.marker = page.is_truncated.then(|| page.next_marker.clone());
        self.output.next_marker = page.next_marker;
        self.output.is_truncated = page.is_truncated;
        self.output.response_info = page.response_info;
        Ok(())
    }

    fn validate_pagination(&mut self, page: &ListFilesOutput) -> Result<(), CliError> {
        if page.is_truncated == page.next_marker.is_empty() {
            return Err(invalid("IsTruncated and NextMarker disagree"));
        }
        if !page.is_truncated {
            return Ok(());
        }
        validate_marker(&page.next_marker)?;
        if !self.markers.insert(page.next_marker.clone()) {
            return Err(invalid("service repeated a pagination marker"));
        }
        Ok(())
    }

    fn is_finished(&self) -> bool {
        self.count() >= self.max_keys || !self.output.is_truncated
    }
}

fn normalize_page(page: &mut ListFilesOutput, prefix: &str) {
    // [Review Fix #ShareFilesCount1] Normalize before counting so JSON, queries and table rows use the same logical cap.
    let (files, mut folders) = normalize_ls_files_and_folders(
        std::mem::take(&mut page.files),
        std::mem::take(&mut page.folders),
        prefix,
    );
    let current_folder = folder_key(prefix);
    folders.retain(|folder| {
        let key = folder_key(&folder.folder);
        !key.is_empty() && key != current_folder
    });
    page.files = files;
    page.folders = folders;
}

fn folder_key(path: &str) -> String {
    path.trim_end_matches('/').to_owned()
}

fn validate_marker(marker: &str) -> Result<(), CliError> {
    if marker.is_empty() || marker.len() > MAX_MARKER_BYTES || marker.chars().any(char::is_control)
    {
        return Err(invalid(
            "pagination marker must contain 1..4096 bytes without control characters",
        ));
    }
    Ok(())
}

fn invalid(message: &str) -> CliError {
    CliError::ValidationError(format!("[share_files_pagination] {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn arguments(max_keys: i32) -> ShareFilesArgs {
        ShareFilesArgs {
            share_url: "https://example.test/share/s".into(),
            path: Some("docs/".into()),
            marker: None,
            max_keys,
            human_readable: false,
            sort: None,
            columns: None,
            manifest_path: None,
            is_password_stdin: false,
        }
    }

    fn page(value: serde_json::Value) -> ListFilesOutput {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn short_mixed_pages_consume_the_combined_remaining_allowance() {
        let args = arguments(3);
        let mut listing = Listing::new(&args);
        assert_eq!(listing.next_request(&args).unwrap().limit, Some(3));
        listing
            .accept(
                page(json!({"Files":[{"FilePath":"docs/a"}],
            "Folders":[{"Folder":"docs/b/"}],"NextMarker":"p2","IsTruncated":true})),
                args.path.as_deref(),
            )
            .unwrap();
        assert!(!listing.is_finished());
        let request = listing.next_request(&args).unwrap();
        assert_eq!(request.limit, Some(1));
        assert_eq!(request.marker.as_deref(), Some("p2"));
        listing
            .accept(
                page(json!({"Files":[{"FilePath":"docs/c"}],
            "NextMarker":"p3","IsTruncated":true})),
                args.path.as_deref(),
            )
            .unwrap();
        assert!(listing.is_finished());
        assert_eq!(listing.count(), 3);
        assert_eq!(listing.output.next_marker, "p3");
    }

    #[test]
    fn logical_count_promotes_markers_and_deduplicates_across_pages() {
        let args = arguments(4);
        let mut listing = Listing::new(&args);
        listing
            .accept(
                page(json!({"Files":[{"FilePath":"docs/"},
            {"FilePath":"docs/a/"},{"FilePath":"docs/b"},{"FilePath":"docs/b"}],
            "Folders":[{"Folder":"docs/a/"},{"Folder":"docs/"}],
            "NextMarker":"p2","IsTruncated":true})),
                args.path.as_deref(),
            )
            .unwrap();
        assert_eq!(listing.count(), 2);
        assert_eq!(listing.next_request(&args).unwrap().limit, Some(2));
        listing
            .accept(
                page(json!({"Files":[{"FilePath":"docs/b"}],
            "Folders":[{"Folder":"docs/a"},{"Folder":"docs/c/"}]})),
                args.path.as_deref(),
            )
            .unwrap();
        assert_eq!(listing.output.files.len(), 1);
        assert_eq!(listing.output.folders.len(), 2);
        assert!(listing.is_finished());
        assert!(!listing.output.is_truncated);
        assert!(listing.output.next_marker.is_empty());
    }

    #[test]
    fn overfull_page_fails_without_returning_a_skipping_cursor() {
        let args = arguments(1);
        let mut listing = Listing::new(&args);
        let result = listing.accept(
            page(json!({"Files":[{"FilePath":"docs/a"}],
            "Folders":[{"Folder":"docs/b/"}],"NextMarker":"later","IsTruncated":true})),
            args.path.as_deref(),
        );
        assert!(result.unwrap_err().to_string().contains("more entries"));
        assert_eq!(listing.count(), 0);
        assert!(listing.output.next_marker.is_empty());
    }

    #[test]
    fn invalid_flag_and_marker_combinations_fail_before_returning_results() {
        for value in [
            json!({"IsTruncated":true}),
            json!({"NextMarker":"next","IsTruncated":false}),
            json!({"NextMarker":"bad\nmarker","IsTruncated":true}),
            json!({"NextMarker":"x".repeat(MAX_MARKER_BYTES + 1),"IsTruncated":true}),
        ] {
            let args = arguments(2);
            let mut listing = Listing::new(&args);
            assert!(listing.accept(page(value), None).is_err());
        }
    }

    #[test]
    fn repeated_initial_and_cyclic_markers_fail() {
        let mut args = arguments(4);
        args.marker = Some("initial".into());
        let mut listing = Listing::new(&args);
        let truncated = |marker| page(json!({"NextMarker":marker,"IsTruncated":true}));
        assert!(listing.accept(truncated("initial"), None).is_err());
        let mut listing = Listing::new(&args);
        listing.accept(truncated("p2"), None).unwrap();
        listing.accept(truncated("p3"), None).unwrap();
        assert!(listing.accept(truncated("p2"), None).is_err());
    }

    #[test]
    fn advancing_empty_pages_are_bounded_and_do_not_consume_allowance() {
        let args = arguments(2);
        let mut listing = Listing::new(&args);
        listing
            .accept(page(json!({"NextMarker":"p2","IsTruncated":true})), None)
            .unwrap();
        assert_eq!(listing.count(), 0);
        assert!(!listing.is_finished());
        listing.pages = MAX_LISTING_PAGES;
        assert!(listing
            .next_request(&args)
            .unwrap_err()
            .to_string()
            .contains("safety bound"));
    }

    #[test]
    fn service_requests_are_capped_without_restricting_the_result_cap() {
        let args = arguments(i32::MAX);
        assert!(validate_arguments(&args).is_ok());
        let mut listing = Listing::new(&args);
        assert_eq!(listing.next_request(&args).unwrap().limit, Some(1000));
    }

    #[test]
    fn invalid_cap_or_initial_marker_is_rejected_locally() {
        for cap in [0, -1, i32::MIN] {
            assert!(validate_arguments(&arguments(cap)).is_err());
        }
        for marker in [
            String::new(),
            "line\nfeed".into(),
            "x".repeat(MAX_MARKER_BYTES + 1),
        ] {
            let mut args = arguments(1);
            args.marker = Some(marker);
            assert!(validate_arguments(&args).is_err());
        }
    }

    fn delayed_server(responses: Vec<String>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use crate::domain::share_access::fixture::read_request;
        use std::io::Write;
        use std::net::TcpListener;
        use std::time::{Duration, Instant};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            let deadline = Instant::now() + Duration::from_secs(5);
            while requests.len() < responses.len() && Instant::now() < deadline {
                let Ok((mut stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                requests.push(read_request(&mut stream));
                if requests.len() == 2 {
                    std::thread::sleep(Duration::from_millis(1100));
                }
                stream
                    .write_all(responses[requests.len() - 1].as_bytes())
                    .unwrap();
            }
            requests
        });
        (endpoint, worker)
    }

    #[tokio::test]
    async fn token_expiring_during_a_page_is_renewed_before_the_next_page() {
        use crate::domain::{share_access::fixture::reply, share_types::ShareLinkInfo};
        use tos_core::agent::global_args::GlobalArgs;
        let (endpoint, worker) = delayed_server(vec![
            reply(200, r#"{"AccessToken":"first-share-token","ExpiresIn":1}"#),
            reply(
                200,
                r#"{"Files":[{"FilePath":"docs/a"}],"NextMarker":"p2","IsTruncated":true}"#,
            ),
            reply(
                200,
                r#"{"AccessToken":"renewed-share-token","ExpiresIn":60}"#,
            ),
            reply(
                200,
                r#"{"Folders":[{"Folder":"docs/b/"}],"IsTruncated":false}"#,
            ),
        ]);
        let global = GlobalArgs::default();
        let mut args = arguments(2);
        args.share_url = format!("{endpoint}/share/s?pwd=receiver-password");
        let mut receiver = ShareReceiver::new(&global, &args.share_url).unwrap();
        let info: ShareLinkInfo = serde_json::from_value(json!({"ShareID":"s"})).unwrap();
        receiver.authorize(&global, &info, false).await.unwrap();
        let output = list_files(&mut receiver, &args).await.unwrap();
        assert_eq!(output.files.len() + output.folders.len(), 2);
        let requests = worker.join().unwrap();
        assert_eq!(requests.len(), 4);
        assert!(requests[1].contains("x-share-token: first-share-token"));
        assert!(requests[3].contains("x-share-token: renewed-share-token"));
        assert!(requests[3].contains("marker=p2") && requests[3].contains("limit=1"));
        assert!(requests[2].contains("\"SharePwd\":\"receiver-password\""));
        assert!(!requests
            .iter()
            .any(|request| request.to_ascii_lowercase().contains("authorization:")));
    }
}
