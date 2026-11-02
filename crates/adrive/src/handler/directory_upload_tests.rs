use super::*;
use clap::Parser;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;

#[derive(Parser)]
struct CopyCommand {
    #[command(flatten)]
    arguments: CpArgs,
}

#[derive(Parser)]
struct SyncCommand {
    #[command(flatten)]
    arguments: SyncArgs,
}

#[derive(Parser)]
struct MoveCommand {
    #[command(flatten)]
    arguments: MvArgs,
}

#[derive(Parser)]
struct RemoveCommand {
    #[command(flatten)]
    arguments: RmArgs,
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("adrive-directory-upload-{}", ulid::Ulid::new()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn arguments(&self, flags: &[&str]) -> CpArgs {
        let mut arguments = vec![
            "cp",
            self.0.to_str().unwrap(),
            "adrive://inst/space/dst/",
            "--recursive",
            "--overwrite-strategy",
            "force",
        ];
        arguments.extend_from_slice(flags);
        CopyCommand::parse_from(arguments).arguments
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[derive(Default)]
struct FolderService {
    folders: BTreeSet<String>,
    files: BTreeSet<String>,
    requests: Vec<String>,
    denied_folder: Option<String>,
    conflict_folder: Option<(String, bool)>,
    is_paginated: bool,
    is_marker_invalid: bool,
    is_listing_denied: bool,
    has_later_listing_denial: bool,
    has_forged_error_body_id: bool,
    multipart_failure: Option<&'static str>,
    has_missing_failure_id: bool,
}

struct TestService {
    client: IdsClient,
    state: Arc<Mutex<FolderService>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl TestService {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(FolderService::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Self::spawn(listener, state.clone(), stop.clone());
        let client = IdsClient::new(
            "fake-key".into(),
            "fake-secret".into(),
            None,
            Some(endpoint),
            Some("cn-beijing".into()),
            ClientOptions {
                max_retry_count: Some(0),
                requesttimeout: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        Self {
            client,
            state,
            stop,
            worker: Some(worker),
        }
    }

    fn spawn(
        listener: TcpListener,
        state: Arc<Mutex<FolderService>>,
        stop: Arc<AtomicBool>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // [Review Fix #DirectoryFixture] Accepted sockets can
                        // inherit the listener's nonblocking mode on macOS.
                        stream.set_nonblocking(false).unwrap();
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let request = read_service_request(&mut stream);
                        let (status, mut body) =
                            service_response(&request, &mut state.lock().unwrap());
                        if status == 403 && state.lock().unwrap().has_forged_error_body_id {
                            let mut error: Value = serde_json::from_str(&body).unwrap();
                            error["RequestId"] = json!("untrusted-body-id");
                            body = error.to_string();
                        }
                        let request_id_header = {
                            let state = state.lock().unwrap();
                            if status == 403 && state.has_missing_failure_id {
                                String::new()
                            } else {
                                format!("x-request-id: service-{}\r\n", state.requests.len())
                            }
                        };
                        write!(stream, "HTTP/1.1 {status} Test\r\n{request_id_header}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => panic!("test server accept failed: {error}"),
                }
            }
        })
    }
}

impl Drop for TestService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn read_service_request(stream: &mut std::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0);
        bytes.extend_from_slice(&buffer[..count]);
        let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else {
            continue;
        };
        let content_length = String::from_utf8_lossy(&bytes[..header_end])
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
            .unwrap_or(0);
        if bytes.len() >= header_end + 4 + content_length {
            return String::from_utf8(bytes).unwrap();
        }
    }
}

fn service_response(request: &str, state: &mut FolderService) -> (u16, String) {
    let first = request.lines().next().unwrap();
    state.requests.push(request.to_string());
    if first.contains("initiateMultipart") {
        return (200, r#"{"UploadId":"upload-1"}"#.into());
    }
    if state
        .multipart_failure
        .is_some_and(|stage| first.contains(stage))
    {
        return (403, r#"{"Code":"Forbidden","Message":"denied"}"#.into());
    }
    if first.contains("uploadPart") {
        return (200, r#"{"PartNumber":1,"ETag":"part-etag"}"#.into());
    }
    if first.starts_with("GET ") {
        if state.is_listing_denied
            || (state.has_later_listing_denial && first.contains("marker=next"))
        {
            return (
                403,
                r#"{"Code":"Forbidden","Message":"delete planning denied"}"#.into(),
            );
        }
        if state.is_marker_invalid {
            return (
                200,
                json!({"IsTruncated": true, "NextMarker": ""}).to_string(),
            );
        }
        if state.is_paginated && !state.has_later_listing_denial && !first.contains("marker=next") {
            return (
                200,
                json!({"IsTruncated": true, "NextMarker": "next"}).to_string(),
            );
        }
        let folders = state
            .folders
            .iter()
            .map(|folder| json!({"Folder": folder}))
            .collect::<Vec<_>>();
        let files = state
            .files
            .iter()
            .map(|file| json!({"FilePath": file, "FileType": if file.ends_with('/') {"folder"} else {"file"}}))
            .collect::<Vec<_>>();
        return (
            200,
            json!({"Files": files, "Folders": folders,
            "IsTruncated":state.has_later_listing_denial,"NextMarker":"next"})
            .to_string(),
        );
    }
    if first.contains("/folders ") {
        let body: Value = serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        let folder = body["FolderPath"].as_str().unwrap();
        if state.denied_folder.as_deref() == Some(folder) {
            return (403, r#"{"Code":"Forbidden","Message":"denied"}"#.into());
        }
        if let Some((conflict, has_racing_folder)) = &state.conflict_folder {
            if conflict == folder {
                if *has_racing_folder {
                    state.folders.insert(folder.to_string());
                }
                return (
                    409,
                    r#"{"Code":"Conflict","Message":"concurrent conflict"}"#.into(),
                );
            }
        }
        if state.files.contains(folder) {
            return (409, r#"{"Code":"Conflict","Message":"file exists"}"#.into());
        }
        state.folders.insert(folder.to_string());
        return (200, json!({"FolderPath": folder}).to_string());
    }
    (200, "{}".into())
}

#[tokio::test]
async fn directory_upload_preserves_empty_source_root() {
    let source = TestDirectory::new();
    let service = TestService::new();
    let arguments = source.arguments(&[]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
        .await
        .unwrap();
    assert_eq!(
        report.succeeded, 1,
        "an empty source must create the destination folder"
    );
    let manifest = report.manifest.unwrap();
    assert_eq!(manifest.object_count, 1);
    assert_eq!(manifest.total_size, 0);
    assert!(service.state.lock().unwrap().folders.contains("dst"));
}

#[test]
fn directory_upload_cli_uses_only_canonical_flag() {
    let flag = "--disable-dir-object";
    assert!(
        CopyCommand::try_parse_from([
            "cp",
            "local",
            "adrive://inst/space/dst",
            "--recursive",
            flag
        ])
        .unwrap()
        .arguments
        .disable_dir_object
    );
    assert!(
        SyncCommand::try_parse_from(["sync", "local", "adrive://inst/space/dst", flag])
            .unwrap()
            .arguments
            .disable_dir_object
    );
    assert!(CopyCommand::try_parse_from(["cp", "local", "adrive://inst/space/dst", flag]).is_err());
    for alias in ["--ddo", "-ddo"] {
        let cp_error = CopyCommand::try_parse_from([
            "cp",
            "local",
            "adrive://inst/space/dst",
            "--recursive",
            alias,
        ])
        .err()
        .expect("removed directory alias must fail");
        let sync_error =
            SyncCommand::try_parse_from(["sync", "local", "adrive://inst/space/dst", alias])
                .err()
                .expect("removed directory alias must fail");
        assert_eq!(cp_error.kind(), clap::error::ErrorKind::UnknownArgument);
        assert_eq!(sync_error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}

#[test]
fn directory_upload_describe_parameter_has_boolean_type() {
    assert_eq!(parameter_schema_type("disable-dir-object"), "boolean");
}

#[tokio::test]
async fn directory_upload_streaming_preserves_empty_nested_folders() {
    let source = TestDirectory::new();
    fs::create_dir_all(source.0.join("empty/deep")).unwrap();
    fs::write(source.0.join("zero.txt"), "").unwrap();
    let service = TestService::new();
    let arguments = source.arguments(&["--no-manifest"]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    let (report, scan, drain) = recursive_cp_streaming_no_manifest_report(
        &service.client,
        &arguments,
        runtime,
        "test",
        "test",
        false,
    )
    .await
    .unwrap();
    scan.result.unwrap();
    drain.result.unwrap();
    assert_eq!(report.succeeded, 4);
    assert_eq!(
        service.state.lock().unwrap().folders,
        BTreeSet::from(["dst".into(), "dst/empty".into(), "dst/empty/deep".into()])
    );
    assert!(report.manifest.is_none());
}

#[tokio::test]
async fn directory_upload_repeated_runs_use_typed_existing_folders() {
    let source = TestDirectory::new();
    fs::create_dir_all(source.0.join("empty")).unwrap();
    let service = TestService::new();
    let arguments = source.arguments(&[]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    for _ in 0..2 {
        let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
            .await
            .unwrap();
        assert_eq!(report.failed, 0);
        assert_eq!(report.total, 2);
    }
    let state = service.state.lock().unwrap();
    assert_eq!(
        state
            .requests
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        2
    );
}

#[tokio::test]
async fn directory_upload_reports_file_conflicts_and_forbidden_errors() {
    for is_file_conflict in [true, false] {
        let source = TestDirectory::new();
        let service = TestService::new();
        if is_file_conflict {
            service.state.lock().unwrap().files.insert("dst".into());
        } else {
            service.state.lock().unwrap().denied_folder = Some("dst".into());
        }
        let arguments = source.arguments(&[]);
        let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
        let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
            .await
            .unwrap();
        assert_eq!(report.failed, 1);
        assert_eq!(report.succeeded, 0);
    }
}

#[test]
fn directory_upload_empty_include_parent_root_is_desired_by_sync() {
    let source = TestDirectory::new();
    let desired =
        collect_local_sync_desired_relative_paths(&source.0, Some("src"), None, None).unwrap();
    assert!(desired.folders.contains("src"));
}

#[tokio::test]
async fn directory_upload_dry_run_describes_directory_policy_without_mutation() {
    let source = TestDirectory::new();
    let config = source.0.join("config.toml");
    fs::write(&config, "").unwrap();
    let global = GlobalArgs {
        config_path: Some(config),
        ..Default::default()
    };
    let auth = ADriveAuthArgs {
        auth_mode: Some(AuthMode::Aksk),
    };
    for is_disabled in [false, true] {
        let arguments = source.arguments(if is_disabled {
            &["--disable-dir-object"]
        } else {
            &[]
        });
        let plan = build_plan(&global, &auth, &ADriveCommand::Cp(arguments))
            .await
            .unwrap();
        assert_eq!(plan["directory_upload"]["enabled"], !is_disabled);
        assert_eq!(plan["directory_upload"]["implicit_file_parents"], true);
        assert_eq!(
            plan["request_plan"]
                .as_array()
                .unwrap()
                .iter()
                .any(|action| action == "create_folder"),
            !is_disabled
        );
    }
    assert_eq!(fs::read_dir(&source.0).unwrap().count(), 1);
}

#[tokio::test]
async fn directory_upload_dry_run_rejects_disable_for_remote_sources() {
    let source = TestDirectory::new();
    let config = source.0.join("config.toml");
    fs::write(&config, "").unwrap();
    let global = GlobalArgs {
        config_path: Some(config),
        ..Default::default()
    };
    let auth = ADriveAuthArgs {
        auth_mode: Some(AuthMode::Aksk),
    };
    let mut arguments = source.arguments(&["--disable-dir-object"]);
    arguments.source = "adrive://inst/space/src/".into();
    arguments.destination = source.0.display().to_string();
    assert!(build_plan(&global, &auth, &ADriveCommand::Cp(arguments))
        .await
        .is_err());
}

#[tokio::test]
async fn directory_upload_disable_skips_creation_in_both_manifest_modes() {
    let source = TestDirectory::new();
    fs::create_dir_all(source.0.join("empty")).unwrap();
    fs::write(source.0.join("zero.txt"), "").unwrap();
    let service = TestService::new();
    let arguments = source.arguments(&["--disable-dir-object"]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
        .await
        .unwrap();
    assert_eq!(report.total, 1);
    let (report, scan, drain) = recursive_cp_streaming_no_manifest_report(
        &service.client,
        &arguments,
        runtime,
        "test",
        "test",
        false,
    )
    .await
    .unwrap();
    scan.result.unwrap();
    drain.result.unwrap();
    assert_eq!(report.total, 1);
    let state = service.state.lock().unwrap();
    assert!(state.folders.is_empty());
    assert!(state.requests.iter().all(|request| !request
        .lines()
        .next()
        .unwrap()
        .contains("/folders")));
}

#[tokio::test]
async fn directory_upload_conflict_requires_exact_typed_folder() {
    for has_racing_folder in [false, true] {
        let source = TestDirectory::new();
        let service = TestService::new();
        service.state.lock().unwrap().conflict_folder = Some(("dst".into(), has_racing_folder));
        let arguments = source.arguments(&[]);
        let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
        let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
            .await
            .unwrap();
        assert_eq!(report.failed == 0, has_racing_folder);
    }
}

#[tokio::test]
async fn directory_upload_checkpoint_revalidates_missing_folders() {
    let source = TestDirectory::new();
    let checkpoint = TestDirectory::new();
    let service = TestService::new();
    let arguments = source.arguments(&[
        "--checkpoint",
        "--checkpoint-dir",
        checkpoint.0.to_str().unwrap(),
    ]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    for _ in 0..2 {
        let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
            .await
            .unwrap();
        assert_eq!(report.succeeded, 1);
        assert_eq!(service.state.lock().unwrap().folders.len(), 1);
        service.state.lock().unwrap().folders.clear();
    }
    assert_eq!(
        service
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|request| request.starts_with("POST "))
            .count(),
        2
    );
}

#[tokio::test]
async fn directory_upload_filters_keep_file_ancestors_without_extra_empty_folders() {
    let source = TestDirectory::new();
    fs::create_dir_all(source.0.join("parent/deep")).unwrap();
    fs::create_dir_all(source.0.join("unselected")).unwrap();
    fs::write(source.0.join("parent/deep/keep.txt"), "payload").unwrap();
    let service = TestService::new();
    let arguments = source.arguments(&["--include", "*.txt"]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
        .await
        .unwrap();
    assert_eq!(report.total, 4);
    let (report, scan, drain) = recursive_cp_streaming_no_manifest_report(
        &service.client,
        &arguments,
        runtime,
        "test",
        "test",
        false,
    )
    .await
    .unwrap();
    scan.result.unwrap();
    drain.result.unwrap();
    assert_eq!(report.total, 4);
    assert_eq!(
        service.state.lock().unwrap().folders,
        BTreeSet::from(["dst".into(), "dst/parent".into(), "dst/parent/deep".into()])
    );
}

#[tokio::test]
async fn directory_upload_preserves_include_parent_and_skips_space_root() {
    let source = TestDirectory::new();
    let service = TestService::new();
    let mut arguments = source.arguments(&["--include-parent"]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
        .await
        .unwrap();
    assert_eq!(report.total, 1);
    let parent = source.0.file_name().unwrap().to_str().unwrap();
    assert!(service
        .state
        .lock()
        .unwrap()
        .folders
        .contains(&format!("dst/{parent}")));
    arguments.include_parent = false;
    arguments.destination = "adrive://inst/space".into();
    let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
        .await
        .unwrap();
    assert_eq!(report.total, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn directory_upload_does_not_follow_symlink_descendants() {
    let source = TestDirectory::new();
    std::os::unix::fs::symlink(&source.0, source.0.join("loop")).unwrap();
    let service = TestService::new();
    let arguments = source.arguments(&[]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    let (report, scan, drain) = recursive_cp_streaming_no_manifest_report(
        &service.client,
        &arguments,
        runtime,
        "test",
        "test",
        false,
    )
    .await
    .unwrap();
    scan.result.unwrap();
    drain.result.unwrap();
    assert_eq!(report.total, 1);
    let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
        .await
        .unwrap();
    assert_eq!(report.total, 1);
}

fn sync_arguments(source: &TestDirectory, checkpoint: &TestDirectory, flags: &[&str]) -> SyncArgs {
    let report = checkpoint.0.join("report.csv");
    let mut arguments = vec![
        "sync",
        source.0.to_str().unwrap(),
        "adrive://inst/space/dst/",
        "--delete",
        "--force",
        "--checkpoint-dir",
        checkpoint.0.to_str().unwrap(),
        "--report-path",
        report.to_str().unwrap(),
    ];
    arguments.extend_from_slice(flags);
    SyncCommand::parse_from(arguments).arguments
}

#[tokio::test]
async fn sync_delete_plan_denial_preserves_completed_copy_artifacts() {
    for is_streaming in [false, true] {
        for has_missing_failure_id in [false, true] {
            for failures_only in [false, true] {
                assert_sync_plan_failure_artifacts(
                    is_streaming,
                    has_missing_failure_id,
                    failures_only,
                )
                .await;
            }
        }
    }
}

#[test]
fn rolled_reports_keep_request_id_column_and_each_rows_identity() {
    let artifacts = TestDirectory::new();
    let report_path = artifacts.0.join("中文 report.csv");
    let mut report = BatchReport::new("ve-adrive cp");
    let request_ids = [Some("first-row-id"), None, Some("third-row-id")];
    for (index, request_id) in request_ids.iter().enumerate() {
        let outcome = if index == 1 {
            Err(CliError::PermissionDenied("fixture denied".into()))
        } else {
            Ok(())
        };
        report.push_operation_result(
            format!("中文%?#-{index}.txt"),
            None,
            "upload",
            (outcome, request_id.map(String::from)),
        );
    }
    let mut writer =
        RollingCsvWriter::new(report_path.to_str().unwrap(), ADRIVE_REPORT_COLUMNS).unwrap();
    // [Review Fix #ReportRolloverCoverage] Force real part-file transitions
    // with tiny records while exercising the public report row serializer.
    writer.max_bytes = writer.header_line.len() as u64 + 1;
    for item in &report.items {
        writer
            .write_record(&adrive_report_record(&report, item))
            .unwrap();
    }
    drop(writer);
    for (index, request_id) in request_ids.iter().enumerate() {
        let contents = fs::read_to_string(rolled_csv_path(&report_path, index + 1)).unwrap();
        assert_eq!(contents.lines().count(), 2);
        assert_eq!(
            contents.lines().next().unwrap(),
            "command,operation,source,destination,status,error,request_id"
        );
        let row = contents.lines().nth(1).unwrap();
        assert!(row.contains(&format!("中文%?#-{index}.txt")));
        assert!(row.ends_with(&format!(",{}", request_id.unwrap_or_default())));
        assert!(row.contains(if index == 1 {
            ",failed,"
        } else {
            ",succeeded,"
        }));
    }
    assert!(!rolled_csv_path(&report_path, request_ids.len() + 1).exists());
}

#[tokio::test]
async fn filtered_recursive_move_preserves_unselected_local_source_files() {
    let directory = TestDirectory::new();
    let source = directory.0.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("中文%?#.txt"), "").unwrap();
    fs::write(source.join("unselected.bin"), "preserve me").unwrap();
    let service = TestService::new();
    let arguments = filtered_move_arguments(source.to_str().unwrap(), &directory, false);
    let result = execute_mv(
        &GlobalArgs::default(),
        &Profile::default(),
        &service.client,
        &arguments,
    )
    .await
    .unwrap();
    assert_eq!(result, 0);
    assert!(
        source.join("unselected.bin").exists(),
        "filtered move deleted an unselected file"
    );
    assert!(!source.join("中文%?#.txt").exists());
    let contents =
        fs::read_to_string(rolled_csv_path(&directory.0.join("move-report.csv"), 1)).unwrap();
    assert!(contents
        .lines()
        .any(|row| row.contains(",delete-source,") && row.ends_with(",")));
}

#[tokio::test]
async fn filtered_recursive_move_in_same_space_uses_selected_file_copy_delete() {
    for is_streaming in [false, true] {
        let directory = TestDirectory::new();
        let service = TestService::new();
        service
            .state
            .lock()
            .unwrap()
            .files
            .extend(["src/中文%?#.txt".into(), "src/unselected.bin".into()]);
        let mut arguments =
            filtered_move_arguments("adrive://inst/space/src/", &directory, is_streaming);
        if !is_streaming {
            arguments.report_path = None;
            arguments.manifest_path = None;
        }
        let profile = Profile {
            batch_report_dir: Some(directory.0.to_str().unwrap().into()),
            ..Default::default()
        };
        assert_eq!(
            execute_mv(
                &GlobalArgs::default(),
                &profile,
                &service.client,
                &arguments,
            )
            .await
            .unwrap(),
            0
        );
        let state = service.state.lock().unwrap();
        let requests = state
            .requests
            .iter()
            .map(|request| request.lines().next().unwrap())
            .collect::<Vec<_>>();
        assert!(
            requests.iter().any(|line| line.contains("%3Acopy")),
            "filtered move must copy the selected file: {requests:?}"
        );
        assert!(requests
            .iter()
            .any(|line| line.starts_with("DELETE ") && line.contains("/files/src/")));
        assert!(requests
            .iter()
            .all(|line| !line.contains("/folders/") && !line.contains("unselected.bin")));
        assert_filtered_remote_move_report_ids(&directory, &requests);
    }
}

fn assert_filtered_remote_move_report_ids(artifacts: &TestDirectory, requests: &[&str]) {
    let delete_request_index = requests
        .iter()
        .position(|line| line.starts_with("DELETE "))
        .unwrap();
    let report = fs::read_dir(&artifacts.0)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "csv"))
        .map(|path| fs::read_to_string(path).unwrap())
        .find(|contents| {
            contents.starts_with("command,operation,source,destination,status,error,request_id")
        })
        .unwrap();
    let delete_row = report
        .lines()
        .find(|line| line.contains(",delete-source,"))
        .unwrap();
    assert!(delete_row.contains("src/中文%?#.txt"));
    assert!(delete_row.ends_with(&format!(",service-{}", delete_request_index + 1)));
}

#[tokio::test]
async fn filtered_recursive_move_rejects_self_or_nested_remote_destination() {
    for is_streaming in [false, true] {
        for (destination, include_parent) in [
            ("adrive://inst/space/src/", false),
            ("adrive://inst/space/src/nested/", false),
            ("adrive://inst/space/", true),
            ("adrive://inst/space/dst/../src/", false),
        ] {
            let directory = TestDirectory::new();
            let service = TestService::new();
            let mut arguments =
                filtered_move_arguments("adrive://inst/space/src/", &directory, is_streaming);
            arguments.destination = destination.into();
            arguments.include_parent = include_parent;
            let result = execute_mv(
                &GlobalArgs::default(),
                &Profile::default(),
                &service.client,
                &arguments,
            )
            .await;
            assert!(
                matches!(result, Err(CliError::ValidationError(_))),
                "unsafe filtered move accepted {destination}"
            );
            assert!(service.state.lock().unwrap().requests.is_empty());
        }
    }
}

fn filtered_move_arguments(source: &str, artifacts: &TestDirectory, is_streaming: bool) -> MvArgs {
    let report_path = artifacts.0.join("move-report.csv");
    let manifest_path = artifacts.0.join("move-manifest.csv");
    let mut arguments = vec![
        "mv",
        source,
        "adrive://inst/space/dst/",
        "--recursive",
        "--include",
        "*.txt",
        "--force",
        "--report-path",
        report_path.to_str().unwrap(),
    ];
    if is_streaming {
        arguments.push("--no-manifest");
    } else {
        arguments.extend(["--manifest-path", manifest_path.to_str().unwrap()]);
    }
    MoveCommand::parse_from(arguments).arguments
}

#[tokio::test]
async fn rm_direct_denial_writes_failed_operation_report() {
    for has_missing_failure_id in [false, true] {
        let artifacts = TestDirectory::new();
        let service = TestService::new();
        {
            let mut state = service.state.lock().unwrap();
            state.multipart_failure = Some("DELETE");
            state.has_missing_failure_id = has_missing_failure_id;
        }
        let arguments = batch_remove_arguments(&artifacts, "direct", false);
        let result = execute_rm(
            &GlobalArgs::default(),
            &Profile::default(),
            &service.client,
            &arguments,
        )
        .await;
        let path = rolled_csv_path(&artifacts.0.join("rm-report.csv"), 1);
        assert!(
            path.exists(),
            "direct recursive denial must write its failed operation report"
        );
        let contents = fs::read_to_string(path).unwrap();
        let row = contents.lines().nth(1).unwrap();
        assert!(row.contains(",delete_folder,") && row.contains(",failed,"));
        assert!(row.ends_with(if has_missing_failure_id {
            ","
        } else {
            ",service-1"
        }));
        assert_eq!(result.unwrap(), 5);
    }
}

#[tokio::test]
async fn rm_upload_scan_failure_preserves_completed_delete_report() {
    for (mode, is_streaming) in [("direct", true), ("bottom-up", false), ("bottom-up", true)] {
        let artifacts = TestDirectory::new();
        let service = TestService::new();
        service
            .state
            .lock()
            .unwrap()
            .files
            .insert("dst/中文%?#.txt".into());
        let mut arguments = batch_remove_arguments(&artifacts, mode, is_streaming);
        arguments.include_uploads = true;
        let checkpoint_path = artifacts.0.join("checkpoint-file");
        fs::write(
            &checkpoint_path,
            "ordinary file cannot be scanned as a directory",
        )
        .unwrap();
        arguments.checkpoint_dir = Some(checkpoint_path.to_str().unwrap().into());
        let expected_exit = checkpoint_scan_failure_exit_code(&arguments).await;
        let result = execute_rm(
            &GlobalArgs::default(),
            &Profile::default(),
            &service.client,
            &arguments,
        )
        .await;
        let path = rolled_csv_path(&artifacts.0.join("rm-report.csv"), 1);
        assert!(
            path.exists(),
            "completed deletes must survive local checkpoint scan failure"
        );
        let contents = fs::read_to_string(path).unwrap();
        let scan_row = contents
            .lines()
            .find(|row| row.contains(",scan-uploads,"))
            .unwrap();
        assert!(scan_row.contains(",failed,") && scan_row.ends_with(","));
        assert!(contents
            .lines()
            .any(|row| row.contains(",succeeded,") && row.contains(",service-")));
        assert_eq!(result.unwrap(), expected_exit);
        assert_eq!(
            rolled_csv_path(&artifacts.0.join("rm-manifest.csv"), 1).exists(),
            mode == "bottom-up" && !is_streaming
        );
    }
}

async fn checkpoint_scan_failure_exit_code(arguments: &RmArgs) -> i32 {
    let target = resolve_rm_target(arguments).unwrap();
    list_checkpointed_uploads_for_rm(
        &Profile::default(),
        &target,
        arguments.checkpoint_dir.as_deref(),
    )
    .await
    .err()
    .unwrap()
    .exit_code()
    .as_i32()
}

fn batch_remove_arguments(artifacts: &TestDirectory, mode: &str, is_streaming: bool) -> RmArgs {
    let report_path = artifacts.0.join("rm-report.csv");
    let manifest_path = artifacts.0.join("rm-manifest.csv");
    let mut arguments = vec![
        "rm",
        "adrive://inst/space/dst/",
        "--recursive",
        "--force",
        "--recursive-delete-mode",
        mode,
        "--report-path",
        report_path.to_str().unwrap(),
    ];
    if is_streaming {
        arguments.push("--no-manifest");
    } else if mode == "bottom-up" {
        arguments.extend(["--manifest-path", manifest_path.to_str().unwrap()]);
    }
    RemoveCommand::parse_from(arguments).arguments
}

#[test]
fn batch_failure_envelope_never_borrows_completed_operation_id_for_local_error() {
    let global = GlobalArgs::default();
    global
        .request_trace
        .record_response(Some("previous-delete-id"), true);
    let error = CliError::Io(std::io::Error::new(
        std::io::ErrorKind::NotADirectory,
        "scan failed",
    ));
    let (envelope, exit_code) = adrive_batch_failure_envelope(
        &global,
        "ve-adrive rm",
        None,
        (&error, None),
        json!({"summary":{"succeeded":1,"failed":1}}),
    );
    assert_eq!(exit_code, error.exit_code().as_i32());
    assert_eq!(envelope.request_id, None);
    assert!(envelope.status_code.is_none() && !envelope.success);
}

#[test]
fn batch_failure_envelope_preserves_service_semantics_without_inventing_id() {
    let global = GlobalArgs::default();
    global.request_trace.record_response(None, false);
    let error = CliError::PermissionDenied("HTTP 403 [Forbidden] denied (RequestId: )".into());
    let (envelope, exit_code) = adrive_batch_failure_envelope(
        &global,
        "ve-adrive sync",
        None,
        (&error, None),
        json!({"summary":{"succeeded":1}}),
    );
    assert_eq!(exit_code, 5);
    assert_eq!(envelope.request_id, None);
    assert_eq!(envelope.status_code, Some(403));
    assert_eq!(envelope.ec.as_deref(), Some("Forbidden"));
    assert_eq!(envelope.data.unwrap()["summary"]["succeeded"], 1);
}

#[test]
fn batch_failure_envelope_does_not_trust_body_request_id_without_response_header() {
    let global = GlobalArgs::default();
    let error = CliError::PermissionDenied(
        "HTTP 403 [Forbidden] denied (RequestId: untrusted-body-id)".into(),
    );
    let (envelope, exit_code) = adrive_batch_failure_envelope(
        &global,
        "ve-adrive cp",
        None,
        (&error, None),
        json!({"summary":{"succeeded":1}}),
    );
    assert_eq!(exit_code, 5);
    assert_eq!(envelope.request_id, None);
}

#[tokio::test]
async fn aggregate_http_failure_uses_captured_header_instead_of_body_id() {
    for stage in ["stream", "planning", "direct"] {
        for has_missing_failure_id in [false, true] {
            let artifacts = TestDirectory::new();
            let service = TestService::new();
            {
                let mut state = service.state.lock().unwrap();
                state.has_forged_error_body_id = true;
                state.has_missing_failure_id = has_missing_failure_id;
                state.is_listing_denied = stage == "planning";
                state.has_later_listing_denial = stage == "stream";
                state.multipart_failure = (stage == "direct").then_some("DELETE");
                state.files.insert("src/a.txt".into());
            }
            let failure = run_captured_http_failure(&service.client, &artifacts, stage).await;
            let error = failure.result.unwrap_err();
            let (envelope, exit_code) = adrive_batch_failure_envelope(
                &GlobalArgs::default(),
                "ve-adrive cp",
                None,
                (&error, failure.request_id.as_deref()),
                json!({"summary":{"succeeded":1}}),
            );
            let expected = if has_missing_failure_id {
                None
            } else {
                Some(if stage == "stream" {
                    "service-2"
                } else {
                    "service-1"
                })
            };
            assert_eq!(envelope.request_id.as_deref(), expected, "{stage}");
            assert_eq!(exit_code, 5);
        }
    }
}

async fn run_captured_http_failure(
    client: &IdsClient,
    artifacts: &TestDirectory,
    stage: &str,
) -> ADriveReportedResult<()> {
    match stage {
        "stream" => {
            let mut arguments = artifacts.arguments(&["--no-manifest"]);
            arguments.source = "adrive://inst/space/src/".into();
            let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
            let (_, failure, drain) = recursive_cp_streaming_no_manifest_report(
                client, &arguments, runtime, "test", "test", false,
            )
            .await
            .unwrap();
            drain.result.unwrap();
            failure
        }
        "planning" => {
            let arguments = sync_arguments(artifacts, artifacts, &[]);
            capture_adrive_report_result(build_adrive_sync_delete_plan(client, &arguments, 2))
                .await
                .map(|_| ())
        }
        "direct" => {
            capture_adrive_report_result(async {
                client
                    .delete_folder(&DeleteFolderInput::new("inst", "space", "dst"))
                    .await
                    .map(|_| ())
                    .map_err(map_ids_error)
            })
            .await
        }
        _ => unreachable!(),
    }
}

#[cfg(unix)]
#[tokio::test]
async fn local_stream_walk_failure_does_not_borrow_completed_upload_id() {
    use std::os::unix::fs::PermissionsExt;
    let source = TestDirectory::new();
    fs::write(source.0.join("a.txt"), "").unwrap();
    fs::write(source.0.join("b.txt"), "").unwrap();
    let blocked = source.0.join("z-blocked");
    fs::create_dir(&blocked).unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0)).unwrap();
    let service = TestService::new();
    let arguments = source.arguments(&[
        "--no-manifest",
        "--disable-dir-object",
        "--batch-concurrency",
        "1",
    ]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    let outcome = recursive_cp_streaming_no_manifest_report(
        &service.client,
        &arguments,
        runtime,
        "test",
        "test",
        false,
    )
    .await;
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
    let (report, failure, drain) = outcome.unwrap();
    drain.result.unwrap();
    assert!(matches!(failure.result, Err(CliError::Io(_))));
    assert_eq!(failure.request_id, None);
    assert_eq!(report.succeeded, 2);
    assert!(report.items.iter().all(|item| item.request_id.is_some()));
}

#[test]
fn batch_failure_envelope_keeps_listing_failure_id_after_pending_copy_drain() {
    for request_id in [None, Some("listing-failure-id")] {
        let global = GlobalArgs::default();
        global
            .request_trace
            .record_response(Some("later-copy-success-id"), true);
        let error = CliError::PermissionDenied(format!(
            "HTTP 403 [Forbidden] listing denied (RequestId: {})",
            request_id.unwrap_or_default(),
        ));
        let (envelope, _) = adrive_batch_failure_envelope(
            &global,
            "ve-adrive cp",
            None,
            (&error, request_id),
            json!({"summary":{"succeeded":1}}),
        );
        assert_eq!(envelope.request_id.as_deref(), request_id);
    }
}

#[tokio::test]
async fn streamed_listing_failure_preserves_report_and_aggregate_exit_code() {
    for action in ["cp", "mv", "rm"] {
        assert_streamed_listing_failure(action).await;
    }
}

#[tokio::test]
async fn streamed_remote_copy_ignores_folder_and_self_markers_in_files() {
    for is_download in [false, true] {
        let artifacts = TestDirectory::new();
        let service = TestService::new();
        {
            let mut state = service.state.lock().unwrap();
            state.files.extend(["src/".into(), "src/child/".into()]);
            state.folders.insert("src/".into());
        }
        let mut arguments = artifacts.arguments(&["--no-manifest"]);
        arguments.source = "adrive://inst/space/src/".into();
        if is_download {
            arguments.destination = artifacts.0.join("download").to_str().unwrap().into();
        }
        let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
        let (report, scan, drain) = recursive_cp_streaming_no_manifest_report(
            &service.client,
            &arguments,
            runtime,
            "test",
            "test",
            false,
        )
        .await
        .unwrap();
        scan.result.unwrap();
        drain.result.unwrap();
        assert_eq!(
            report.total, 0,
            "folder markers are not downloadable/copyable files"
        );
        assert_eq!(
            service.state.lock().unwrap().requests.len(),
            2,
            "discover the child prefix once, and ignore the self marker"
        );
    }
}

async fn assert_streamed_listing_failure(action: &str) {
    let artifacts = TestDirectory::new();
    let service = TestService::new();
    {
        let mut state = service.state.lock().unwrap();
        state.has_later_listing_denial = true;
        state.files.insert(format!(
            "{}/中文%?#.txt",
            if action == "rm" { "dst" } else { "src" }
        ));
    }
    let global = GlobalArgs::default();
    let client = service
        .client
        .clone()
        .with_request_trace(global.request_trace.clone());
    let result = execute_streamed_listing_failure(action, &artifacts, &global, &client).await;
    let path = rolled_csv_path(&artifacts.0.join("stream-report.csv"), 1);
    assert!(path.exists());
    let contents = fs::read_to_string(path).unwrap();
    let expected_rows = match action {
        "cp" => 1,
        "mv" => 2,
        "rm" => 0,
        _ => unreachable!(),
    };
    assert_eq!(contents.lines().count(), expected_rows + 1);
    assert_eq!(
        result.unwrap(),
        5,
        "{action} must emit one failed aggregate with the original exit code"
    );
}

async fn execute_streamed_listing_failure(
    action: &str,
    artifacts: &TestDirectory,
    global: &GlobalArgs,
    client: &IdsClient,
) -> Result<i32, CliError> {
    let report_path = artifacts
        .0
        .join("stream-report.csv")
        .to_str()
        .unwrap()
        .to_string();
    match action {
        "cp" => {
            let mut arguments = artifacts.arguments(&["--no-manifest"]);
            arguments.source = "adrive://inst/space/src/".into();
            arguments.report_path = Some(report_path);
            execute_cp(global, &Profile::default(), client, &arguments).await
        }
        "mv" => {
            let mut arguments =
                filtered_move_arguments("adrive://inst/space/src/", artifacts, true);
            arguments.report_path = Some(report_path);
            execute_mv(global, &Profile::default(), client, &arguments).await
        }
        "rm" => {
            let mut arguments = batch_remove_arguments(artifacts, "bottom-up", true);
            arguments.report_path = Some(report_path);
            execute_rm(global, &Profile::default(), client, &arguments).await
        }
        _ => unreachable!(),
    }
}

async fn assert_sync_plan_failure_artifacts(
    is_streaming: bool,
    has_missing_failure_id: bool,
    failures_only: bool,
) {
    let source = TestDirectory::new();
    let artifacts = TestDirectory::new();
    let service = TestService::new();
    {
        let mut state = service.state.lock().unwrap();
        state.is_listing_denied = true;
        state.has_missing_failure_id = has_missing_failure_id;
    }
    fs::write(source.0.join("中文%?#.txt"), "").unwrap();
    let manifest_path = artifacts.0.join("manifest.csv");
    let mut flags = vec!["--disable-dir-object", "--overwrite-strategy", "force"];
    if is_streaming {
        flags.push("--no-manifest");
    } else {
        flags.extend(["--manifest-path", manifest_path.to_str().unwrap()]);
    }
    if failures_only {
        flags.push("--report-failures-only");
    }
    let arguments = sync_arguments(&source, &artifacts, &flags);
    let result = execute_sync(
        &GlobalArgs::default(),
        &Profile::default(),
        &service.client,
        &arguments,
    )
    .await;
    let report = rolled_csv_path(&artifacts.0.join("report.csv"), 1);
    assert!(
        report.exists(),
        "completed upload report must survive delete planning failure"
    );
    let contents = fs::read_to_string(report).unwrap();
    assert_eq!(contents.lines().count(), if failures_only { 1 } else { 2 });
    if !failures_only {
        assert!(contents.contains("中文%?#.txt"));
        assert!(contents.lines().nth(1).unwrap().ends_with(",service-1"));
    }
    assert_eq!(result.unwrap(), 5, "preserve permission-denied exit code");
    assert_eq!(rolled_csv_path(&manifest_path, 1).exists(), !is_streaming);
    let state = service.state.lock().unwrap();
    assert_eq!(state.requests.len(), 2);
    assert!(!state
        .requests
        .iter()
        .any(|request| request.starts_with("DELETE ")));
}

#[tokio::test]
async fn directory_upload_sync_creation_failure_blocks_deletion() {
    let source = TestDirectory::new();
    let checkpoint = TestDirectory::new();
    let service = TestService::new();
    {
        let mut state = service.state.lock().unwrap();
        state.denied_folder = Some("dst".into());
        state.folders.insert("dst/stale".into());
    }
    for flags in [&[][..], &["--no-manifest"][..]] {
        let arguments = sync_arguments(&source, &checkpoint, flags);
        let exit_code = execute_sync(
            &GlobalArgs::default(),
            &Profile::default(),
            &service.client,
            &arguments,
        )
        .await
        .unwrap();
        assert_eq!(exit_code, 1);
    }
    let state = service.state.lock().unwrap();
    assert!(state
        .requests
        .iter()
        .all(|request| !request.starts_with("DELETE ")));
}

#[tokio::test]
async fn directory_upload_sync_disable_protects_source_present_empty_folders() {
    let source = TestDirectory::new();
    fs::create_dir_all(source.0.join("keep")).unwrap();
    let checkpoint = TestDirectory::new();
    let service = TestService::new();
    service
        .state
        .lock()
        .unwrap()
        .folders
        .extend(["dst/keep".into(), "dst/stale".into()]);
    let arguments = sync_arguments(&source, &checkpoint, &["--disable-dir-object"]);
    let plan = build_adrive_sync_delete_plan(&service.client, &arguments, 2)
        .await
        .unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].source, "adrive://inst/space/dst/stale");
}

#[tokio::test]
async fn directory_upload_existing_folder_lookup_handles_pagination() {
    for is_marker_invalid in [false, true] {
        let source = TestDirectory::new();
        let service = TestService::new();
        {
            let mut state = service.state.lock().unwrap();
            state.folders.insert("dst".into());
            state.is_paginated = true;
            state.is_marker_invalid = is_marker_invalid;
        }
        let arguments = source.arguments(&[]);
        let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
        let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
            .await
            .unwrap();
        assert_eq!(report.failed == 1, is_marker_invalid);
        assert_eq!(
            service
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .filter(|request| request.starts_with("POST "))
                .count(),
            0
        );
    }
}

#[tokio::test]
async fn report_request_id_matches_each_upload_after_command_limit() {
    for is_streaming in [false, true] {
        let source = TestDirectory::new();
        for index in 0..105 {
            fs::write(source.0.join(format!("file-{index:03}.txt")), "").unwrap();
        }
        let service = TestService::new();
        let trace = Arc::new(tos_core::agent::request_id::ServiceRequestTrace::default());
        let client = service.client.clone().with_request_trace(trace.clone());
        let flags: &[&str] = if is_streaming {
            &["--no-manifest"]
        } else {
            &[]
        };
        let arguments = source.arguments(flags);
        let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
        let report = if is_streaming {
            let (report, scan, drain) = recursive_cp_streaming_no_manifest_report(
                &client, &arguments, runtime, "test", "test", false,
            )
            .await
            .unwrap();
            scan.result.unwrap();
            drain.result.unwrap();
            report
        } else {
            upload_directory_recursive(&client, &arguments, runtime, false, false)
                .await
                .unwrap()
        };
        assert_eq!(report.succeeded, 106);
        assert_eq!(trace.snapshot().request_ids.len(), 100);
        assert!(trace.snapshot().request_ids_omitted > 0);
        assert_report_upload_request_ids(&report, &service.state.lock().unwrap().requests);
    }
}

fn assert_report_upload_request_ids(report: &BatchReport, requests: &[String]) {
    for item in &report.items {
        let filename = Path::new(&item.source)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let request_index = requests
            .iter()
            .position(|request| {
                if item.operation == "create-folder" {
                    request.starts_with("POST ") && request.contains("\"FolderPath\":\"dst\"")
                } else {
                    request.starts_with("POST ")
                        && request.lines().next().unwrap().contains(filename)
                }
            })
            .unwrap_or_else(|| {
                panic!(
                    "no request for {} ({}) among {:?}",
                    item.source,
                    item.operation,
                    requests
                        .iter()
                        .take(5)
                        .map(|request| request.lines().next())
                        .collect::<Vec<_>>()
                )
            });
        assert_eq!(
            item.request_id,
            Some(format!("service-{}", request_index + 1)),
            "{}",
            item.source
        );
    }
}

#[tokio::test]
async fn report_request_id_is_preserved_for_directory_denial() {
    let source = TestDirectory::new();
    let service = TestService::new();
    service.state.lock().unwrap().denied_folder = Some("dst".into());
    let arguments = source.arguments(&[]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    let report = upload_directory_recursive(&service.client, &arguments, runtime, false, false)
        .await
        .unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(report.items[0].request_id.as_deref(), Some("service-2"));
}

#[tokio::test]
async fn report_request_id_is_preserved_for_sync_delete_rows() {
    let service = TestService::new();
    let mut report = BatchReport::new("ve-adrive sync");
    let items = [SYNC_DELETE_EXTRA_FILE, SYNC_DELETE_EXTRA_FOLDER]
        .into_iter()
        .map(|operation| BatchManifestItem {
            operation: operation.into(),
            source: format!("adrive://inst/space/{operation}"),
            destination: None,
            size: 0,
            etag: None,
            crc64: None,
        })
        .collect();
    let deleted = execute_adrive_sync_delete_plan(&service.client, items, 2, false, &mut report)
        .await
        .unwrap();
    assert_eq!(deleted, 2);
    assert_eq!(report.items[0].request_id.as_deref(), Some("service-1"));
    assert_eq!(report.items[1].request_id.as_deref(), Some("service-2"));
}

#[tokio::test]
async fn report_request_id_preserves_multipart_failure_before_abort_cleanup() {
    for stage in ["uploadPart", "completeMultipart"] {
        for has_missing_failure_id in [false, true] {
            let (report, requests) = failed_multipart_report(stage, has_missing_failure_id).await;
            assert_eq!(report.failed, 1);
            assert!(
                requests.last().unwrap().contains("abortMultipart"),
                "report: {:?}, requests: {:?}",
                report.items,
                requests
                    .iter()
                    .map(|request| request.lines().next())
                    .collect::<Vec<_>>()
            );
            let failed_index = requests
                .iter()
                .position(|request| request.lines().next().unwrap().contains(stage))
                .unwrap();
            let expected =
                (!has_missing_failure_id).then(|| format!("service-{}", failed_index + 1));
            assert_eq!(
                report.items[0].request_id, expected,
                "{stage}, missing ID: {has_missing_failure_id}"
            );
        }
    }
}

async fn failed_multipart_report(
    stage: &'static str,
    has_missing_failure_id: bool,
) -> (BatchReport, Vec<String>) {
    let source = TestDirectory::new();
    let source_file = source.0.join("file.txt");
    fs::write(&source_file, "x").unwrap();
    let service = TestService::new();
    {
        let mut state = service.state.lock().unwrap();
        state.multipart_failure = Some(stage);
        state.has_missing_failure_id = has_missing_failure_id;
    }
    let arguments = source.arguments(&[]);
    let runtime = effective_cp_runtime_config(&Profile::default(), &arguments).unwrap();
    let outcome = capture_service_request_id(upload_adrive_file_multipart_for_batch(
        &service.client,
        "inst",
        "space",
        "file.txt",
        &source_file,
        1,
        false,
        None,
        None,
        runtime,
    ))
    .await;
    let mut report = BatchReport::new("ve-adrive cp");
    report.push_operation_result(source_file.display().to_string(), None, "upload", outcome);
    let requests = service.state.lock().unwrap().requests.clone();
    (report, requests)
}
