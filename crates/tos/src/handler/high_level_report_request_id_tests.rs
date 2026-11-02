use super::*;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};

fn report_http_fixture(responses: Vec<String>) -> (TosClient, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        for response in responses {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = connection.read(&mut buffer).unwrap();
                assert!(count > 0, "connection closed before request headers");
                request.extend_from_slice(&buffer[..count]);
            }
            connection.write_all(response.as_bytes()).unwrap();
        }
    });
    let client = TosClient::new(
        &Profile {
            endpoint: Some(endpoint),
            region: Some("test-region".into()),
            access_key_id: Some("fixture-ak".into()),
            secret_access_key: Some("fixture-sk".into()),
            max_retry_count: Some(0),
            requesttimeout: Some(5),
            ..Profile::default()
        },
        "tos",
    )
    .unwrap();
    (client, server)
}

fn report_http_response(status: &str, request_id: Option<&str>, body: &str) -> String {
    let identifier_header = request_id
        .map(|identifier| format!("x-tos-request-id: {identifier}\r\n"))
        .unwrap_or_default();
    format!(
        "HTTP/1.1 {status}\r\n{identifier_header}content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn read_report_test_request(connection: &mut TcpStream) -> String {
    connection
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let mut buffer = [0; 1024];
    while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
        let count = connection.read(&mut buffer).unwrap();
        assert!(count > 0, "missing request headers");
        request.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8(request).unwrap()
}

fn delayed_range_failure_fixture(request_id: Option<&str>) -> (TosClient, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let denied_response = report_http_response(
        "403 Forbidden",
        request_id,
        r#"{"Code":"AccessDenied","Message":"denied"}"#,
    );
    let server = thread::spawn(move || {
        let (mut first, _) = listener.accept().unwrap();
        let first_request = read_report_test_request(&mut first);
        let (mut second, _) = listener.accept().unwrap();
        read_report_test_request(&mut second);
        let (mut denied, mut succeeded) = if first_request.contains("bytes=0-") {
            (first, second)
        } else {
            (second, first)
        };
        let (headers, body) = denied_response.split_once("\r\n\r\n").unwrap();
        denied
            .write_all(format!("{headers}\r\n\r\n").as_bytes())
            .unwrap();
        thread::sleep(Duration::from_millis(30));
        succeeded
            .write_all(
                report_http_response("206 Partial Content", Some("other-range-success"), "x")
                    .as_bytes(),
            )
            .unwrap();
        thread::sleep(Duration::from_millis(100));
        denied.write_all(body.as_bytes()).unwrap();
    });
    let client = TosClient::new(
        &Profile {
            endpoint: Some(endpoint),
            region: Some("test-region".into()),
            access_key_id: Some("fixture-ak".into()),
            secret_access_key: Some("fixture-sk".into()),
            max_retry_count: Some(0),
            requesttimeout: Some(5),
            ..Profile::default()
        },
        "tos",
    )
    .unwrap();
    (client, server)
}

#[tokio::test]
async fn parallel_range_failure_keeps_its_own_id_after_another_range_succeeds() {
    for expected_id in [Some("failed-range-id"), None] {
        let (client, server) = delayed_range_failure_fixture(expected_id);
        let directory = std::env::temp_dir().join(format!("tos-range-id-{}", ulid::Ulid::new()));
        fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("download.bin");
        let mut options = local_copy_options(EffectiveOverwriteStrategy::Force);
        options.multipart_concurrency = 2;
        let result = capture_report_result(download_tos_to_local_ranges(
            &GlobalArgs::default(),
            &client,
            "tos://bucket/source.bin",
            "bucket",
            "source.bin",
            destination.to_str().unwrap(),
            &destination,
            multipart_part_size(1) + 1,
            None,
            "etag",
            options,
        ))
        .await;
        server.join().unwrap();
        assert!(result.result.is_err());
        assert_eq!(result.request_id.as_deref(), expected_id);
        fs::remove_dir_all(directory).unwrap();
    }
}

fn streaming_delete_failure_fixture(request_id: Option<&str>) -> (TosClient, JoinHandle<()>) {
    report_http_fixture(vec![
        report_http_response(
            "200 OK",
            Some("list-first"),
            r#"{"Contents":[{"Key":"prefix/a","Size":1}],"IsTruncated":true,"NextContinuationToken":"second"}"#,
        ),
        report_http_response(
            "403 Forbidden",
            request_id,
            r#"{"Code":"AccessDenied","Message":"denied (RequestId: forged-body-id)"}"#,
        ),
        report_http_response("204 No Content", Some("delete-during-drain"), ""),
    ])
}

#[tokio::test]
async fn streaming_list_failure_id_survives_successful_delete_drain_without_body_fallback() {
    for expected_id in [Some("failed-list-id"), None] {
        let (client, server) = streaming_delete_failure_fixture(expected_id);
        let target = parse_tos_uri("tos://bucket/prefix/", true).unwrap();
        let mut report = BatchReport::new(0);
        let captured = capture_report_result(stream_delete_tos_flat_pages(
            &client,
            &target,
            Some("prefix/"),
            &recursive_rm_args(),
            16,
            &None,
            None,
            &mut report,
        ))
        .await;
        server.join().unwrap();
        assert!(captured.result.is_err());
        assert_eq!(report.summary.succeeded, 1);
        assert_eq!(
            report.succeeded[0].request_id.as_deref(),
            Some("delete-during-drain")
        );
        assert_eq!(captured.request_id.as_deref(), expected_id);
    }
}

#[tokio::test]
async fn streaming_delete_returns_partial_report_in_a_failed_aggregate() {
    let (client, server) = streaming_delete_failure_fixture(Some("failed-list-id"));
    let target = parse_tos_uri("tos://bucket/prefix/", true).unwrap();
    let directory = std::env::temp_dir().join(format!("tos-stream-list-id-{}", ulid::Ulid::new()));
    let report_path = directory.join("report.csv");
    let result = execute_rm_streaming_no_manifest(
        &GlobalArgs::default(),
        &client,
        &recursive_rm_args(),
        &target,
        "tos://bucket/prefix/",
        Some("prefix/"),
        None,
        false,
        16,
        TosRecursiveListOptions {
            use_hierarchical_listing: false,
            list_concurrency: 1,
        },
        false,
        report_path.to_str().map(str::to_string),
    )
    .await;
    server.join().unwrap();
    assert_eq!(result.unwrap(), 5);
    let report = fs::read_to_string(&report_path).unwrap();
    assert!(report
        .lines()
        .nth(1)
        .unwrap()
        .ends_with(",delete-during-drain"));
    fs::remove_dir_all(directory).unwrap();
}

fn recursive_rm_args() -> RmArgs {
    use clap::{Args, FromArgMatches};
    let matches = RmArgs::augment_args(clap::Command::new("rm"))
        .try_get_matches_from(["rm", "tos://bucket/prefix/", "--recursive", "--force"])
        .unwrap();
    RmArgs::from_arg_matches(&matches).unwrap()
}

#[cfg(unix)]
fn denied_local_walk_source() -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let source = std::env::temp_dir().join(format!("tos-local-walk-id-{}", ulid::Ulid::new()));
    fs::create_dir_all(source.join("z-denied")).unwrap();
    fs::write(source.join("a.txt"), "a").unwrap();
    fs::write(source.join("b.txt"), "b").unwrap();
    let denied = source.join("z-denied");
    fs::set_permissions(&denied, fs::Permissions::from_mode(0)).unwrap();
    (source, denied)
}

#[cfg(unix)]
fn local_walk_cp_args(source: &Path) -> CpArgs {
    use clap::{Args, FromArgMatches};
    let matches = CpArgs::augment_args(clap::Command::new("cp"))
        .try_get_matches_from([
            "cp",
            source.to_str().unwrap(),
            "tos://bucket/base/",
            "--recursive",
            "--no-manifest",
            "--disable-dir-object",
            "--batch-concurrency",
            "1",
            "--overwrite-strategy",
            "force",
        ])
        .unwrap();
    CpArgs::from_arg_matches(&matches).unwrap()
}

#[tokio::test]
#[cfg(unix)]
async fn local_copy_walk_failure_has_no_id_after_successful_uploads() {
    use std::os::unix::fs::PermissionsExt;
    let (client, server) = report_http_fixture(vec![
        report_http_response("200 OK", Some("upload-a"), ""),
        report_http_response("200 OK", Some("upload-b"), ""),
    ]);
    let (source, denied) = denied_local_walk_source();
    let arguments = local_walk_cp_args(&source);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    let global = GlobalArgs::default();
    let mut report = BatchReport::new(0);
    let mut context = TosStreamCopyContext {
        global: &global,
        client: &client,
        args: &arguments,
        runtime,
        write_options: ObjectWriteOptions::default(),
        progress: &None,
        report: &mut report,
        in_flight: FuturesUnordered::new(),
        limit: 1,
    };
    let captured = capture_report_result(stream_cp_local_source_no_manifest(&mut context)).await;
    context.drain_all().await;
    drop(context);
    server.join().unwrap();
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(captured.result.unwrap_err().exit_code().as_i32(), 5);
    assert_eq!(report.summary.succeeded, 2);
    assert!(report
        .succeeded
        .iter()
        .all(|item| item.request_id.is_some()));
    assert_eq!(captured.request_id, None);
    fs::remove_dir_all(source).unwrap();
}

#[tokio::test]
#[cfg(unix)]
async fn local_move_walk_failure_has_no_id_after_successful_uploads() {
    use std::os::unix::fs::PermissionsExt;
    let (client, server) = report_http_fixture(vec![
        report_http_response("200 OK", Some("upload-a"), ""),
        report_http_response("200 OK", Some("upload-b"), ""),
    ]);
    let (source, denied) = denied_local_walk_source();
    let arguments =
        filtered_recursive_mv_args(source.to_str().unwrap(), "tos://bucket/base/", true);
    let runtime = effective_default_runtime_config(&Profile::default()).unwrap();
    let global = GlobalArgs::default();
    let mut report = BatchReport::new(0);
    let mut context = TosStreamMoveContext {
        global: &global,
        client: &client,
        args: &arguments,
        runtime,
        write_options: ObjectWriteOptions::default(),
        progress: &None,
        report: &mut report,
        in_flight: FuturesUnordered::new(),
        limit: 1,
        source_is_hns: false,
        deferred_directory_deletes: Vec::new(),
    };
    let captured = capture_report_result(stream_mv_local_source_no_manifest(&mut context)).await;
    context.drain_all().await;
    drop(context);
    server.join().unwrap();
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(captured.result.unwrap_err().exit_code().as_i32(), 5);
    assert_eq!(report.summary.succeeded, 4);
    assert!(report
        .succeeded
        .iter()
        .filter(|item| item.operation == "copy")
        .all(|item| item.request_id.is_some()));
    assert_eq!(captured.request_id, None);
    fs::remove_dir_all(source).unwrap();
}

fn filtered_recursive_mv_args(source: &str, destination: &str, is_streaming: bool) -> MvArgs {
    use clap::{Args, FromArgMatches};
    let mut arguments = vec![
        "mv",
        source,
        destination,
        "--recursive",
        "--force",
        "--include",
        "*.txt",
    ];
    if is_streaming {
        arguments.push("--no-manifest");
    }
    let matches = MvArgs::augment_args(clap::Command::new("mv"))
        .try_get_matches_from(arguments)
        .unwrap();
    MvArgs::from_arg_matches(&matches).unwrap()
}

#[test]
fn filtered_recursive_move_rejects_effective_self_destination_in_every_report_mode() {
    for is_streaming in [false, true] {
        for source in ["tos://bucket/src", "tos://bucket/src/"] {
            for destination in ["tos://bucket", "tos://bucket/"] {
                let mut arguments = filtered_recursive_mv_args(source, destination, is_streaming);
                arguments.include_parent = true;
                let failure = mv_operation(&arguments)
                    .err()
                    .expect("effective self move must fail");
                assert_eq!(failure.exit_code().as_i32(), 6);
            }
        }
        let arguments = filtered_recursive_mv_args(
            "tos://bucket/src/",
            "tos://bucket/src/child/",
            is_streaming,
        );
        assert!(mv_operation(&arguments).is_err());
        let safe =
            filtered_recursive_mv_args("tos://bucket/src/", "tos://bucket/other/", is_streaming);
        assert!(mv_operation(&safe).is_ok());
    }
}

#[test]
fn recursive_local_move_rejects_existing_alias_and_missing_nested_destination() {
    let directory =
        std::env::temp_dir().join(format!("tos-local-move-overlap-{}", ulid::Ulid::new()));
    let source = directory.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("keep.txt"), b"only bytes").unwrap();
    for destination in [source.join("."), source.join("missing/nested")] {
        let arguments = filtered_recursive_mv_args(
            source.to_str().unwrap(),
            destination.to_str().unwrap(),
            false,
        );
        assert!(
            mv_operation(&arguments).is_err(),
            "unsafe local destination: {destination:?}"
        );
    }
    assert_eq!(fs::read(source.join("keep.txt")).unwrap(), b"only bytes");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn recursive_move_rejects_dot_segments_before_http_url_normalization() {
    for (source, destination) in [
        ("tos://bucket/src/", "tos://bucket/dst/../src/"),
        ("tos://bucket/src/", "tos://bucket/./src/"),
        ("tos://bucket/dst/../src/", "tos://bucket/other/"),
    ] {
        for is_streaming in [false, true] {
            let mut arguments = filtered_recursive_mv_args(source, destination, is_streaming);
            assert!(mv_operation(&arguments).is_err());
            arguments.include = None;
            assert!(mv_operation(&arguments).is_err());
        }
    }
}

#[test]
fn bytetos_copy_source_encodes_full_path_once_for_chinese_and_reserved_characters() {
    for key in ["dir/中文.txt", "dir/中文 空+%25%2F?#.txt"] {
        let header = copy_source_header_value("bucket", key);
        assert!(header.starts_with("%2Fbucket%2Fdir%2F"));
        let encoded_url = reqwest::Url::parse(&format!("http://fixture/?source={header}")).unwrap();
        let decoded = encoded_url.query_pairs().next().unwrap().1.into_owned();
        assert_eq!(decoded, format!("/bucket/{key}"));
    }
}

#[tokio::test]
async fn hns_direct_delete_failure_writes_its_service_id_and_retains_the_exit_code() {
    let (client, server) = report_http_fixture(vec![report_http_response(
        "403 Forbidden",
        Some("direct-denied"),
        r#"{"Code":"AccessDenied","Message":"fixture denied"}"#,
    )]);
    let directory = std::env::temp_dir().join(format!("tos-direct-id-{}", ulid::Ulid::new()));
    let report_path = directory.join("report.csv");
    let manifest_path = directory.join("manifest.csv");
    let mut arguments = recursive_rm_args();
    arguments.manifest_path = Some(manifest_path.to_string_lossy().into_owned());
    let target = parse_tos_uri("tos://bucket/prefix/", true).unwrap();
    let exit_code = execute_rm_hns_direct(
        &GlobalArgs::default(),
        &client,
        &arguments,
        &target,
        TosBatchArtifactPaths {
            report_path: report_path.to_str().map(str::to_string),
            manifest_path: arguments.manifest_path.clone(),
        },
    )
    .await
    .unwrap();
    server.join().unwrap();
    assert_eq!(exit_code, 5);
    let report = fs::read_to_string(&report_path).expect("failed direct delete report");
    assert!(report.lines().nth(1).unwrap().ends_with(",direct-denied"));
    assert!(report.contains(",failed,permission_denied,"));
    let manifest = fs::read_to_string(&manifest_path).expect("direct directory manifest");
    assert!(manifest.contains(",directory,tos://bucket/prefix/,"));
    fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn abort_uploads_count_each_planned_action_and_preserve_its_own_response_id() {
    let (client, server) = report_http_fixture(vec![
        report_http_response(
            "200 OK",
            Some("uploads-list"),
            r#"{"Uploads":[{"Key":"prefix/a","UploadId":"first"},{"Key":"prefix/b","UploadId":"second"}],"IsTruncated":false}"#,
        ),
        report_http_response("204 No Content", Some("abort-first"), ""),
        report_http_response("403 Forbidden", None, r#"{"Code":"AccessDenied"}"#),
    ]);
    let target = parse_tos_uri("tos://bucket/prefix/", true).unwrap();
    let mut report = BatchReport::new(0);
    abort_multipart_uploads_for_rm(
        &client,
        &target,
        Some("prefix/"),
        TosAbortReportOptions {
            progress_enabled: false,
            report_path: None,
            report_failures_only: false,
        },
        &mut report,
    )
    .await
    .unwrap();
    server.join().unwrap();
    assert_eq!(report.summary.planned, 2);
    assert_eq!(report.summary.succeeded, 1);
    assert_eq!(report.summary.failed, 1);
    assert_eq!(
        report.succeeded[0].request_id.as_deref(),
        Some("abort-first")
    );
    assert_eq!(report.failed[0].request_id, None);
}

#[tokio::test]
async fn rm_upload_plan_keeps_manifest_and_abort_results_consistent() {
    let listing = r#"{"Contents":[],"Versions":[],"Uploads":[{"Key":"prefix/a","UploadId":"first"}],"IsTruncated":false}"#;
    for is_all_versions in [false, true] {
        let (client, server) = report_http_fixture(vec![
            report_http_response("200 OK", Some("bucket-head"), ""),
            report_http_response("200 OK", Some("first-list"), listing),
            report_http_response("200 OK", Some("second-list"), listing),
            report_http_response("204 No Content", Some("abort-first"), ""),
        ]);
        let directory = std::env::temp_dir().join(format!("tos-upload-plan-{}", ulid::Ulid::new()));
        let mut arguments = recursive_rm_args();
        arguments.include_uploads = true;
        arguments.all_versions = is_all_versions;
        arguments.report_path = Some(directory.join("report.csv").to_string_lossy().into_owned());
        arguments.manifest_path = Some(
            directory
                .join("manifest.csv")
                .to_string_lossy()
                .into_owned(),
        );
        let result = execute_rm(
            &GlobalArgs::default(),
            &Profile::default(),
            &client,
            &arguments,
        )
        .await
        .unwrap();
        assert_eq!(result, 0);
        let manifest = fs::read_to_string(arguments.manifest_path.as_ref().unwrap()).unwrap();
        assert!(manifest.contains(",multipart_upload,"), "{manifest}");
        let report = fs::read_to_string(arguments.report_path.as_ref().unwrap()).unwrap();
        assert!(manifest.contains("prefix/a?"));
        assert!(report.contains("prefix/a?"));
        assert!(report.lines().nth(1).unwrap().ends_with(",abort-first"));
        server.join().unwrap();
        fs::remove_dir_all(directory).unwrap();
    }
}

#[tokio::test]
async fn active_core_scope_without_header_suppresses_legacy_error_body_ids() {
    let directory =
        std::env::temp_dir().join(format!("tos-no-header-report-{}", ulid::Ulid::new()));
    let path = directory.join("report.csv");
    let trace = tos_core::agent::request_id::ServiceRequestTrace::default();
    let error = CliError::TransferFailed("HTTP 400 body (RequestId: forged-id)".into());
    let (item, _) = capture_service_request_id(async {
        trace.record_response(None, false);
        write_error_report(path.to_str(), "copy", "source", None, &error).unwrap();
        BatchItemResult::failure("copy", "source", None, &error)
    })
    .await;
    assert_eq!(item.request_id, None);
    let report = fs::read_to_string(&path).expect("read report");
    assert!(report.lines().nth(1).unwrap().ends_with(','));
    fs::remove_dir_all(directory).expect("remove report directory");
}

#[tokio::test]
async fn active_core_header_overrides_legacy_error_body_ids() {
    let trace = tos_core::agent::request_id::ServiceRequestTrace::default();
    let error = CliError::TransferFailed("HTTP 400 body (RequestId: forged-id)".into());
    let (item, _) = capture_service_request_id(async {
        trace.record_response(Some("real-header-id"), false);
        BatchItemResult::failure("copy", "source", None, &error)
    })
    .await;
    assert_eq!(item.request_id.as_deref(), Some("real-header-id"));
}

#[test]
fn captured_header_id_takes_precedence_over_error_body_ids() {
    let error = CliError::TransferFailed(
        "HTTP 400 body (RequestId: forged-id) (RequestId: header-id)".into(),
    );
    let item = with_report_request_id(Some("header-id".into()), || {
        BatchItemResult::failure("copy", "source", None, &error)
    });
    assert_eq!(item.request_id.as_deref(), Some("header-id"));
}

#[test]
fn captured_missing_id_does_not_fall_back_to_error_body_ids() {
    let error = CliError::TransferFailed("HTTP 400 body (RequestId: forged-id)".into());
    let item = with_report_request_id(None, || {
        BatchItemResult::failure("copy", "source", None, &error)
    });
    assert_eq!(item.request_id, None);
}

#[test]
fn local_error_text_cannot_supply_a_service_id() {
    let error = CliError::ValidationError("invalid local path (RequestId: forged-id)".into());
    let item = BatchItemResult::failure("copy", "source", None, &error);
    assert_eq!(item.request_id, None);
}

#[tokio::test]
async fn response_without_id_cannot_reuse_another_file_id() {
    let trace = tos_core::agent::request_id::ServiceRequestTrace::default();
    trace.record_response(Some("unrelated-file"), true);
    let captured = capture_report_result(async {
        trace.record_response(None, true);
        Ok(transferred(None))
    })
    .await;
    let mut report = BatchReport::new(1);
    record_tos_stream_copy_result(
        &None,
        &mut report,
        "source".into(),
        "destination".into(),
        1,
        captured,
    );
    assert_eq!(report.succeeded[0].request_id, None);
}

#[tokio::test]
async fn legacy_single_row_hooks_preserve_scoped_ids_and_csv_escaping() {
    let directory = std::env::temp_dir().join(format!("tos-single-report-{}", ulid::Ulid::new()));
    let path = directory.join("report.csv");
    let trace = tos_core::agent::request_id::ServiceRequestTrace::default();
    capture_service_request_id(async {
        trace.record_response(Some("id,\"quoted\""), true);
        write_single_report(path.to_str(), "restore", "source", None, "succeeded").unwrap();
        write_error_report(
            path.to_str(),
            "restore",
            "source",
            None,
            &CliError::ValidationError("decode failed".into()),
        )
        .unwrap();
    })
    .await;
    let report = fs::read_to_string(&path).expect("read single report");
    assert!(report.lines().next().unwrap().ends_with(",request_id"));
    assert!(report
        .lines()
        .skip(1)
        .all(|row| row.ends_with(",\"id,\"\"quoted\"\"\"")));
    fs::remove_dir_all(directory).expect("remove report directory");
}

#[test]
fn manifested_csv_matches_each_action_to_its_service_id() {
    let directory = std::env::temp_dir().join(format!("tos-action-report-{}", ulid::Ulid::new()));
    let path = directory.join("report.csv");
    let mappings = ["a", "b"].map(|name| TransferPlanItem {
        relative_key: name.into(),
        source: format!("tos://source/{name}"),
        destination: format!("tos://destination/{name}"),
        size: 1,
        etag: None,
        crc64: None,
        last_modified: None,
    });
    let manifest = build_transfer_manifest(&mappings);
    let mut report =
        BatchReport::new_streaming(2, path.to_str(), "tos cp", false, Some(&manifest)).unwrap();
    for (index, item) in mappings.iter().enumerate().rev() {
        report.record_copy_result(
            "copy",
            &item.source,
            &item.destination,
            &transferred(Some(&format!("service-{index}"))),
        );
    }
    write_tos_batch_report(path.to_str(), "tos cp", &report, false).unwrap();
    let body = fs::read_to_string(&path).expect("read manifested report");
    let fields = body
        .lines()
        .skip(1)
        .map(|row| row.split(',').collect::<Vec<_>>())
        .collect::<Vec<_>>();
    assert_eq!(
        (&fields[0][9], &fields[0][10], &fields[0][11]),
        (&"3", &"2", &"service-1")
    );
    assert_eq!(
        (&fields[1][9], &fields[1][10], &fields[1][11]),
        (&"3", &"1", &"service-0")
    );
    fs::remove_dir_all(directory).expect("remove report directory");
}

fn transferred(request_id: Option<&str>) -> CopyTransferResult {
    CopyTransferResult {
        outcome: CopyOutcome::Transferred,
        response_data: None,
        request_id: request_id.map(str::to_string),
        status_code: Some(200),
        ec: None,
    }
}

#[test]
fn report_appends_request_id_after_existing_columns() {
    assert_eq!(TOS_REPORT_COLUMNS.last(), Some(&"request_id"));
    assert_eq!(
        &TOS_REPORT_COLUMNS[..11],
        &[
            "command",
            "operation",
            "source",
            "destination",
            "status",
            "error_kind",
            "error_code",
            "message",
            "retryable",
            "schema_version",
            "action_id",
        ]
    );
}

#[test]
fn unmanifested_copy_report_keeps_its_response_id() {
    let mut report = BatchReport::new(1);
    record_tos_stream_copy_result(
        &None,
        &mut report,
        "source".into(),
        "tos://bucket/destination".into(),
        1,
        Ok(transferred(Some("copy-response"))),
    );
    let row = tos_report_record("tos cp", &report.succeeded[0], None);
    assert_eq!(row.last().map(String::as_str), Some("copy-response"));
}

#[test]
fn failed_report_reads_only_the_error_own_service_id() {
    let error = CliError::TransferFailed("HTTP 403 Forbidden (RequestId: denied-response)".into());
    let item = BatchItemResult::failure("copy", "source", None, &error);
    let row = tos_report_record("tos cp", &item, None);
    assert_eq!(row.last().map(String::as_str), Some("denied-response"));
}

#[test]
fn local_failure_report_keeps_request_id_empty() {
    let error = CliError::ValidationError("invalid local input".into());
    let item = BatchItemResult::failure("copy", "source", None, &error);
    let row = tos_report_record("tos cp", &item, None);
    assert_eq!(row.last().map(String::as_str), Some(""));
}

fn local_copy_options(overwrite_strategy: EffectiveOverwriteStrategy) -> CopyOptions<'static> {
    let mut runtime = effective_default_runtime_config(&Profile::default()).unwrap();
    runtime.overwrite_strategy = overwrite_strategy;
    runtime.copy_options(
        None,
        false,
        false,
        None,
        ObjectWriteOptions::default(),
        false,
        true,
        None,
    )
}

#[test]
fn local_copy_success_report_omits_generated_envelope_request_id() {
    let directory = std::env::temp_dir().join(format!("tos-local-copy-id-{}", ulid::Ulid::new()));
    fs::create_dir_all(&directory).unwrap();
    let source = directory.join("source.txt");
    let destination = directory.join("destination.txt");
    fs::write(&source, b"local bytes").unwrap();
    let result = copy_local_to_local(
        &GlobalArgs::default(),
        source.to_str().unwrap(),
        destination.to_str().unwrap(),
        local_copy_options(EffectiveOverwriteStrategy::Force),
    )
    .unwrap();
    let mut report = BatchReport::new(1);
    report.record_copy_result("local-copy", "source", "destination", &result);
    assert_eq!(fs::read(&destination).unwrap(), b"local bytes");
    let row = tos_report_record("tos cp", &report.succeeded[0], None);
    assert_eq!(row.last().map(String::as_str), Some(""));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn local_copy_skip_report_has_no_service_request_id() {
    let directory = std::env::temp_dir().join(format!("tos-local-skip-id-{}", ulid::Ulid::new()));
    fs::create_dir_all(&directory).unwrap();
    let destination = directory.join("destination.txt");
    fs::write(&destination, b"keep destination").unwrap();
    let result = copy_local_to_local(
        &GlobalArgs::default(),
        "missing-source.txt",
        destination.to_str().unwrap(),
        local_copy_options(EffectiveOverwriteStrategy::NoClobber),
    )
    .unwrap();
    let mut report = BatchReport::new(1);
    report.record_copy_result("local-copy", "source", "destination", &result);
    assert!(result.is_skipped());
    assert_eq!(fs::read(&destination).unwrap(), b"keep destination");
    let row = tos_report_record("tos cp", &report.skipped[0], None);
    assert_eq!(row.last().map(String::as_str), Some(""));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rolled_csv_reports_keep_each_items_present_or_missing_service_id() {
    let directory =
        std::env::temp_dir().join(format!("tos-rolled-report-id-{}", ulid::Ulid::new()));
    let path = directory.join("report.csv");
    let mut report = BatchReport::new_streaming(2, path.to_str(), "tos cp", false, None).unwrap();
    with_report_request_id(Some("first-service-id".into()), || {
        report.record_success("copy", "first-source", Some("first-destination"));
    });
    let sink = report.streaming_sink.as_mut().unwrap();
    sink.writer.max_bytes = sink.writer.current_bytes + 1;
    with_report_request_id(None, || {
        report.record_success("copy", "second-source", Some("second-destination"));
    });
    write_tos_batch_report(path.to_str(), "tos cp", &report, false).unwrap();
    let first = fs::read_to_string(&path).unwrap();
    let second = fs::read_to_string(rolled_csv_path(&path, 2)).unwrap();
    assert_eq!(first.lines().next(), second.lines().next());
    assert!(first.lines().next().unwrap().ends_with(",request_id"));
    assert_eq!(first.lines().count(), 2);
    assert_eq!(second.lines().count(), 2);
    assert!(first.lines().nth(1).unwrap().ends_with(",first-service-id"));
    assert!(second.lines().nth(1).unwrap().ends_with(','));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn unsafe_error_service_id_is_omitted() {
    let error = CliError::TransferFailed("HTTP 500 (RequestId: unsafe\nvalue)".into());
    let item = BatchItemResult::failure("copy", "source", None, &error);
    let row = tos_report_record("tos cp", &item, None);
    assert_eq!(row.last().map(String::as_str), Some(""));
}

#[test]
fn unsafe_copy_response_id_is_omitted() {
    let mut report = BatchReport::new(1);
    report.record_copy_result(
        "copy",
        "source",
        "destination",
        &transferred(Some("bad\nid")),
    );
    assert_eq!(report.succeeded[0].request_id, None);
}

#[test]
fn du_uses_the_shared_command_diagnostic_limit() {
    assert_eq!(
        DU_REQUEST_ID_LIMIT,
        tos_core::agent::request_id::SERVICE_REQUEST_ID_LIMIT
    );
}

#[test]
fn legacy_report_header_is_not_mixed_with_new_rows() {
    let directory = std::env::temp_dir().join(format!("tos-report-header-{}", ulid::Ulid::new()));
    fs::create_dir_all(&directory).expect("create report directory");
    let path = directory.join("report.csv");
    let legacy = "command,operation,source,destination,status,error_kind,error_code,message,retryable,schema_version,action_id\n";
    fs::write(&path, legacy).expect("write legacy header");
    let result = write_single_report(path.to_str(), "copy", "source", None, "succeeded");
    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(&path).expect("read unchanged report"),
        legacy
    );
    fs::remove_dir_all(directory).expect("remove report directory");
}

#[tokio::test]
async fn response_caused_local_failure_keeps_the_file_service_id() {
    let trace = tos_core::agent::request_id::ServiceRequestTrace::default();
    let captured = capture_report_result(async {
        trace.record_response(Some("decode-response"), true);
        Err::<(), _>(CliError::ValidationError("invalid response payload".into()))
    })
    .await;
    let mut report = BatchReport::new(1);
    captured.record(|result| report.record_failure("copy", "source", None, &result.unwrap_err()));
    assert_eq!(
        report.failed[0].request_id.as_deref(),
        Some("decode-response")
    );
}

#[tokio::test]
async fn terminal_transport_failure_leaves_the_file_id_empty() {
    let trace = tos_core::agent::request_id::ServiceRequestTrace::default();
    let captured = capture_report_result(async {
        trace.record_response(Some("retry-response"), false);
        trace.record_no_response();
        Err::<(), _>(CliError::TransferFailed("transport failed".into()))
    })
    .await;
    let mut report = BatchReport::new(1);
    captured.record(|result| report.record_failure("copy", "source", None, &result.unwrap_err()));
    assert_eq!(report.failed[0].request_id, None);
}

#[tokio::test]
async fn report_rows_keep_ids_after_the_command_diagnostic_limit() {
    let trace = tos_core::agent::request_id::ServiceRequestTrace::with_limit(100);
    let mut report = BatchReport::new(105);
    for index in 0..105 {
        let expected_id = format!("file-{index}");
        let captured = capture_report_result(async {
            trace.record_response(Some(&expected_id), true);
            Ok(())
        })
        .await;
        captured.record(|_| report.record_success("copy", &expected_id, None));
    }
    assert_eq!(trace.snapshot().request_ids_omitted, 5);
    assert!(report
        .succeeded
        .iter()
        .enumerate()
        .all(|(index, item)| item.request_id.as_deref() == Some(format!("file-{index}").as_str())));
}

#[tokio::test]
async fn concurrent_file_captures_do_not_exchange_report_ids() {
    let trace = tos_core::agent::request_id::ServiceRequestTrace::default();
    let (first, second) = tokio::join!(
        capture_report_result(async {
            trace.record_response(Some("first-retry"), false);
            tokio::task::yield_now().await;
            trace.record_response(Some("first-final"), true);
            Ok(())
        }),
        capture_report_result(async {
            trace.record_response(Some("second-final"), true);
            Ok(())
        }),
    );
    let mut report = BatchReport::new(2);
    second.record(|_| report.record_success("delete", "second", None));
    first.record(|_| report.record_success("delete", "first", None));
    assert_eq!(
        report.succeeded[0].request_id.as_deref(),
        Some("second-final")
    );
    assert_eq!(
        report.succeeded[1].request_id.as_deref(),
        Some("first-final")
    );
}

#[test]
fn filtered_hns_move_does_not_plan_source_root_deletion() {
    for is_streaming in [false, true] {
        for is_exclusion in [false, true] {
            let mut arguments =
                filtered_recursive_mv_args("tos://bucket/src/", "tos://bucket/dst/", is_streaming);
            if is_exclusion {
                arguments.include = None;
                arguments.exclude = Some("*.bin".into());
            }
            assert!(
                tos_recursive_move_source_root(&arguments).is_none(),
                "a filtered move must leave its source directory in place"
            );
            let plan = append_move_source_root_action(
                build_move_transfer_manifest(&[]),
                &arguments,
                &[],
                true,
            );
            assert_eq!(
                plan.object_count, 0,
                "do not append an unselected directory action"
            );
        }
    }
}
