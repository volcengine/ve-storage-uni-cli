use super::*;
use clap::{Args, FromArgMatches};

fn source_tree(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "tos-directory-upload-{label}-{}-{}",
        std::process::id(),
        ulid::Ulid::new()
    ));
    fs::create_dir_all(&root).expect("create isolated source");
    root
}

fn offline_client() -> TosClient {
    TosClient::new(
        &Profile {
            access_key_id: Some("fixture-access".into()),
            secret_access_key: Some("fixture-secret".into()),
            region: Some("fixture-region".into()),
            endpoint: Some("http://127.0.0.1:1".into()),
            ..Default::default()
        },
        "tos",
    )
    .expect("offline client")
}

async fn planned_upload(
    source: &Path,
    destination: &str,
    include_parent: bool,
) -> Vec<TransferPlanItem> {
    build_recursive_copy_mappings(
        &offline_client(),
        source.to_str().expect("source"),
        destination,
        RecursiveMappingOptions {
            include_parent,
            directory_upload_policy: Some(DirectoryUploadPolicy::Preserve),
            recursive_list_mode: None,
            list_concurrency: 1,
            scan_progress: None,
        },
    )
    .await
    .expect("local upload plan")
}

#[tokio::test]
async fn directory_upload_plan_preserves_mixed_tree_and_zero_byte_files() {
    let root = source_tree("mixed");
    fs::create_dir_all(root.join("empty/nested")).unwrap();
    fs::write(root.join("zero.txt"), []).unwrap();
    let planned = planned_upload(&root, "tos://bucket/destination", false).await;
    let destinations = planned
        .iter()
        .map(|item| item.destination.as_str())
        .collect::<Vec<_>>();
    assert!(destinations.contains(&"tos://bucket/destination/"));
    assert!(destinations.contains(&"tos://bucket/destination/empty/"));
    assert!(destinations.contains(&"tos://bucket/destination/empty/nested/"));
    assert!(destinations.contains(&"tos://bucket/destination/zero.txt"));
    assert!(planned.iter().all(|item| item.size == 0));
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn directory_upload_empty_root_respects_include_parent() {
    let root = source_tree("empty");
    let planned = planned_upload(&root, "tos://bucket/destination", true).await;
    assert_eq!(planned.len(), 1);
    assert_eq!(
        planned[0].destination,
        format!(
            "tos://bucket/destination/{}/",
            root.file_name().unwrap().to_str().unwrap()
        )
    );
    assert_eq!(planned[0].size, 0);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_upload_cli_uses_only_canonical_flag() {
    assert!(CpArgs::augment_args(clap::Command::new("cp"))
        .try_get_matches_from([
            "cp",
            ".",
            "tos://bucket/",
            "--recursive",
            "--disable-dir-object"
        ])
        .unwrap()
        .get_flag("disable_dir_object"));
    assert!(SyncArgs::augment_args(clap::Command::new("sync"))
        .try_get_matches_from(["sync", ".", "tos://bucket/", "--disable-dir-object"])
        .unwrap()
        .get_flag("disable_dir_object"));
    for flag in ["--ddo", "-ddo"] {
        let cp_error = CpArgs::augment_args(clap::Command::new("cp"))
            .try_get_matches_from(["cp", ".", "tos://bucket/", "--recursive", flag])
            .expect_err("removed directory alias must fail");
        let sync_error = SyncArgs::augment_args(clap::Command::new("sync"))
            .try_get_matches_from(["sync", ".", "tos://bucket/", flag])
            .expect_err("removed directory alias must fail");
        assert_eq!(cp_error.kind(), clap::error::ErrorKind::UnknownArgument);
        assert_eq!(sync_error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn directory_upload_skips_symlink_descendants() {
    let root = source_tree("symlink");
    fs::write(root.join("file.txt"), "fixture").unwrap();
    std::os::unix::fs::symlink(root.join("file.txt"), root.join("alias.txt")).unwrap();
    let planned = planned_upload(&root, "tos://bucket/destination", false).await;
    assert!(!planned
        .iter()
        .any(|item| item.source.ends_with("alias.txt")));
    fs::remove_dir_all(root).unwrap();
}

fn sync_upload_args(root: &Path) -> SyncArgs {
    let matches = SyncArgs::augment_args(clap::Command::new("sync"))
        .try_get_matches_from(["sync", root.to_str().unwrap(), "tos://bucket/destination"])
        .unwrap();
    SyncArgs::from_arg_matches(&matches).unwrap()
}

fn entry(key: &str) -> ObjectEntry {
    ObjectEntry {
        key: key.into(),
        size: 0,
        last_modified: None,
        etag: None,
        storage_class: None,
    }
}

#[test]
fn directory_upload_sync_disabled_creation_retains_source_present_empty_directories() {
    let root = source_tree("sync-disabled-creation");
    fs::create_dir_all(root.join("empty/nested")).unwrap();
    fs::write(root.join("file.txt"), "file").unwrap();
    let mut args = sync_upload_args(&root);
    args.delete = true;
    args.disable_dir_object = true;
    let (planned, mut desired) = build_local_upload_sync_plan(&args, "destination/").unwrap();
    assert_eq!(planned.len(), 1);
    assert!(desired.contains("destination/empty/nested/"));
    let inventory = [
        "destination/",
        "destination/empty/",
        "destination/empty/nested/",
        "destination/extra/",
    ]
    .into_iter()
    .map(|key| (key.into(), entry(key)))
    .collect();
    let extras =
        plan_local_upload_sync_extras(&args, "destination/", &inventory, &mut desired).unwrap();
    assert_eq!(
        extras
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        ["destination/extra/"]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_upload_sync_protects_selected_file_and_excluded_remote_ancestors() {
    let root = source_tree("sync-parent");
    fs::create_dir_all(root.join("child")).unwrap();
    fs::write(root.join("child/keep.txt"), "file").unwrap();
    let mut args = sync_upload_args(&root);
    args.delete = true;
    args.include = Some("*.txt".into());
    let (_, desired) = build_local_upload_sync_plan(&args, "destination/").unwrap();
    assert!(desired.contains("destination/child/"));
    args.include = None;
    args.exclude = Some("keep.bin".into());
    let (_, mut desired) = build_local_upload_sync_plan(&args, "destination/").unwrap();
    let inventory = [
        "destination/extra/",
        "destination/extra/keep.bin",
        "destination/extra/obsolete.txt",
    ]
    .into_iter()
    .map(|key| (key.into(), entry(key)))
    .collect();
    let extras =
        plan_local_upload_sync_extras(&args, "destination/", &inventory, &mut desired).unwrap();
    assert_eq!(extras.len(), 1);
    assert_eq!(extras[0].key, "destination/extra/obsolete.txt");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_upload_headers_use_zero_body_without_file_storage_class() {
    let headers = directory_upload_headers(&ObjectWriteOptions {
        content_type: Some("text/plain".into()),
        storage_class: Some("IA".into()),
        acl: Some("private".into()),
        ..Default::default()
    });
    assert_eq!(headers["content-length"], "0");
    assert_eq!(headers["content-type"], "application/x-directory");
    assert_eq!(headers["x-tos-acl"], "private");
    assert!(!headers.contains_key("x-tos-storage-class"));
}

#[tokio::test]
async fn directory_upload_bucket_root_never_becomes_an_empty_key() {
    let root = source_tree("bucket-root");
    assert!(planned_upload(&root, "tos://bucket/", false)
        .await
        .is_empty());
    fs::create_dir(root.join("empty")).unwrap();
    let planned = planned_upload(&root, "tos://bucket/", false).await;
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].destination, "tos://bucket/empty/");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_upload_option_rejects_other_transfer_directions() {
    let root = source_tree("validation");
    let path = root.to_str().unwrap();
    for (source, destination) in [
        ("tos://bucket/source/", path),
        (path, path),
        ("tos://bucket/source/", "tos://bucket/target/"),
    ] {
        assert!(validate_directory_upload_option(source, destination, true).is_err());
    }
    assert!(validate_directory_upload_option(path, "tos://bucket/target/", true).is_ok());
    assert!(CpArgs::augment_args(clap::Command::new("cp"))
        .try_get_matches_from(["cp", path, "tos://bucket/target/", "--disable-dir-object"])
        .is_err());
    fs::remove_dir_all(root).unwrap();
}

fn read_fixture_request(stream: &mut std::net::TcpStream) -> String {
    // macOS can inherit listener nonblocking mode on accepted fixture sockets.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).expect("request header");
        request.extend_from_slice(&byte);
        assert!(request.len() < 65536);
    }
    let headers = String::from_utf8(request.clone()).unwrap();
    let size = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    assert!(size < 65536);
    let mut body = vec![0; size];
    stream.read_exact(&mut body).unwrap();
    request.extend(body);
    String::from_utf8(request).unwrap()
}

fn fixture_server(statuses: Vec<u16>) -> (TosClient, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for status in statuses {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "fixture request timeout"
                        );
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            };
            requests.push(read_fixture_request(&mut stream));
            let body = if status == 200 {
                String::new()
            } else {
                json!({"Code":"Conflict", "Message":"fixture rejection"}).to_string()
            };
            write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nx-tos-request-id: directory-fixture\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        }
        requests
    });
    let client = TosClient::new(
        &Profile {
            access_key_id: Some("fixture-access".into()),
            secret_access_key: Some("fixture-secret".into()),
            region: Some("fixture-region".into()),
            endpoint: Some(endpoint),
            max_retry_count: Some(0),
            ..Default::default()
        },
        "tos",
    )
    .unwrap();
    (client, server)
}

fn directory_copy_options(progress: Option<&ProgressBar>) -> CopyOptions<'_> {
    CopyOptions {
        overwrite_strategy: EffectiveOverwriteStrategy::Force,
        report_path: None,
        report_failures_only: false,
        checkpoint_enabled: true,
        checkpoint_dir: None,
        checkpoint_threshold: 1,
        multipart_concurrency: 1,
        progress_granularity: EffectiveProgressGranularity::Byte,
        progress_enabled: false,
        write_options: ObjectWriteOptions {
            storage_class: Some("IA".into()),
            ..Default::default()
        },
        silent_per_file: true,
        overall_bar: progress,
    }
}

#[tokio::test]
async fn directory_upload_executes_idempotent_empty_put_and_propagates_conflict() {
    let root = source_tree("execution");
    let (client, server) = fixture_server(vec![200, 200, 409, 403]);
    let progress = ProgressBar::hidden();
    let options = directory_copy_options(Some(&progress));
    for _ in 0..2 {
        let result = upload_local_to_tos(
            &GlobalArgs::default(),
            &client,
            root.to_str().unwrap(),
            "tos://bucket/destination/",
            options.clone(),
        )
        .await
        .unwrap();
        assert_eq!(result.outcome, CopyOutcome::Transferred);
    }
    for _ in 0..2 {
        assert!(upload_local_to_tos(
            &GlobalArgs::default(),
            &client,
            root.to_str().unwrap(),
            "tos://bucket/destination/",
            options.clone()
        )
        .await
        .is_err());
    }
    assert_eq!(progress.position(), 2);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 4);
    for request in requests {
        assert!(request.starts_with("PUT /bucket/destination/ HTTP/1.1"));
        assert!(request.to_ascii_lowercase().contains("content-length: 0"));
        assert!(!request.contains("uploads"));
        assert!(!request.to_ascii_lowercase().contains("x-tos-storage-class"));
        assert!(request.ends_with("\r\n\r\n"));
    }
    fs::remove_dir_all(root).unwrap();
}

async fn stream_upload_for_test(root: &Path, client: &TosClient, flags: &[&str]) -> BatchReport {
    let mut arguments = vec![
        "cp",
        root.to_str().unwrap(),
        "tos://bucket/destination",
        "--recursive",
        "--no-manifest",
    ];
    arguments.extend_from_slice(flags);
    let matches = CpArgs::augment_args(clap::Command::new("cp"))
        .try_get_matches_from(arguments)
        .unwrap();
    let args = CpArgs::from_arg_matches(&matches).unwrap();
    let runtime = effective_cp_runtime_config(&Profile::default(), &args).unwrap();
    let mut report = BatchReport::new(0);
    let progress = None;
    let global = GlobalArgs::default();
    {
        let mut context = TosStreamCopyContext {
            global: &global,
            client,
            args: &args,
            runtime,
            write_options: ObjectWriteOptions::default(),
            progress: &progress,
            report: &mut report,
            in_flight: FuturesUnordered::new(),
            limit: 2,
        };
        stream_cp_local_source_no_manifest(&mut context)
            .await
            .unwrap();
        context.drain_all().await;
    }
    report
}

#[tokio::test]
async fn directory_upload_streaming_matches_manifest_actions() {
    let root = source_tree("streaming");
    fs::create_dir_all(root.join("empty/nested")).unwrap();
    fs::write(root.join("file.txt"), "fixture").unwrap();
    let planned = planned_upload(&root, "tos://bucket/destination", false).await;
    let manifest = build_transfer_manifest(&planned);
    let (client, server) = fixture_server(vec![200; planned.len()]);
    let report = stream_upload_for_test(&root, &client, &[]).await;
    assert_eq!(report.summary.failed, 0);
    assert_eq!(report.summary.succeeded, manifest.object_count);
    assert_eq!(report.summary.planned, manifest.object_count);
    let mut completed = report
        .succeeded
        .iter()
        .map(|item| item.destination.clone().unwrap())
        .collect::<Vec<_>>();
    let mut expected = planned
        .iter()
        .map(|item| item.destination.clone())
        .collect::<Vec<_>>();
    completed.sort();
    expected.sort();
    assert_eq!(completed, expected);
    assert!(report
        .succeeded
        .iter()
        .all(|item| item.operation == "upload"));
    assert_eq!(server.join().unwrap().len(), planned.len());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_upload_single_directory_requires_recursive() {
    let root = source_tree("single-validation");
    let matches = CpArgs::augment_args(clap::Command::new("cp"))
        .try_get_matches_from(["cp", root.to_str().unwrap(), "tos://bucket/destination/"])
        .unwrap();
    let args = CpArgs::from_arg_matches(&matches).unwrap();
    assert!(cp_operation(&args).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn directory_upload_no_clobber_keeps_atomic_precondition() {
    let root = source_tree("no-clobber");
    let (client, server) = fixture_server(vec![404, 200]);
    let mut options = directory_copy_options(None);
    options.overwrite_strategy = EffectiveOverwriteStrategy::NoClobber;
    upload_local_to_tos(
        &GlobalArgs::default(),
        &client,
        root.to_str().unwrap(),
        "tos://bucket/destination/",
        options,
    )
    .await
    .unwrap();
    let requests = server.join().unwrap();
    assert!(requests[0].starts_with("HEAD "));
    assert!(requests[1]
        .to_ascii_lowercase()
        .contains("if-none-match: *"));
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn directory_upload_no_clobber_skips_a_concurrently_created_hns_parent() {
    let root = source_tree("no-clobber-race");
    let (client, server) = fixture_server(vec![404, 412]);
    let mut options = directory_copy_options(None);
    options.overwrite_strategy = EffectiveOverwriteStrategy::NoClobber;
    let result = upload_local_to_tos(
        &GlobalArgs::default(),
        &client,
        root.to_str().unwrap(),
        "tos://bucket/destination/",
        options,
    )
    .await
    .unwrap();
    assert_eq!(result.outcome, CopyOutcome::Skipped);
    assert_eq!(server.join().unwrap().len(), 2);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn directory_upload_cp_include_parent_filters_keep_existing_file_contract() {
    let root = source_tree("parent-filter");
    let source = root.join("src");
    fs::create_dir_all(source.join("empty")).unwrap();
    fs::write(source.join("file.txt"), "fixture").unwrap();
    let (client, server) = fixture_server(vec![200; 3]);
    let report = stream_upload_for_test(
        &source,
        &client,
        &["--include-parent", "--include", "src/*"],
    )
    .await;
    assert_eq!(report.summary.succeeded, 3);
    assert!(report
        .succeeded
        .iter()
        .any(|item| item.destination.as_deref() == Some("tos://bucket/destination/src/file.txt")));
    assert_eq!(server.join().unwrap().len(), 3);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directory_upload_sync_root_container_survives_active_filters() {
    let root = source_tree("filtered-root");
    let mut args = sync_upload_args(&root);
    args.include = Some("*.txt".into());
    let (planned, desired) = build_local_upload_sync_plan(&args, "destination/").unwrap();
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].1, "destination/");
    assert!(desired.contains("destination/"));
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn directory_upload_selected_files_create_required_ancestors() {
    let root = source_tree("filtered-ancestors");
    fs::create_dir_all(root.join("populated/deep")).unwrap();
    fs::create_dir(root.join("unmatched-empty")).unwrap();
    fs::write(root.join("populated/deep/keep.txt"), "fixture").unwrap();
    let (client, server) = fixture_server(vec![200; 4]);
    let report = stream_upload_for_test(&root, &client, &["--include", "*.txt"]).await;
    assert_eq!(report.summary.succeeded, 4);
    assert_eq!(server.join().unwrap().len(), 4);
    let matches = CpArgs::augment_args(clap::Command::new("cp"))
        .try_get_matches_from([
            "cp",
            root.to_str().unwrap(),
            "tos://bucket/destination",
            "--recursive",
            "--include",
            "*.txt",
        ])
        .unwrap();
    let mut cp_args = CpArgs::from_arg_matches(&matches).unwrap();
    let raw = planned_upload(&root, "tos://bucket/destination", false).await;
    let selected = filter_local_cp_upload_mappings(&cp_args, raw.clone()).unwrap();
    assert_eq!(build_transfer_manifest(&selected).object_count, 4);
    assert!(selected
        .iter()
        .any(|item| item.destination == "tos://bucket/destination/populated/deep/"));
    cp_args.disable_dir_object = true;
    assert_eq!(
        filter_local_cp_upload_mappings(&cp_args, raw)
            .unwrap()
            .len(),
        1
    );
    let mut args = sync_upload_args(&root);
    args.include = Some("*.txt".into());
    let (planned, _) = build_local_upload_sync_plan(&args, "destination/").unwrap();
    assert_eq!(planned.len(), 4);
    assert!(planned
        .iter()
        .any(|(_, key, _)| key == "destination/populated/deep/"));
    assert!(!planned
        .iter()
        .any(|(_, key, _)| key.contains("unmatched-empty")));
    fs::remove_dir_all(root).unwrap();
}
