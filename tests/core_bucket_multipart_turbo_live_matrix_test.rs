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

use std::collections::{BTreeMap, BTreeSet};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug)]
struct LiveEnv {
    access_key: String,
    secret_key: String,
    endpoint: String,
    region: String,
}

#[derive(Debug)]
struct CoreFixture {
    base_dir: String,
    home_dir: String,
    bucket: String,
    object_key: String,
    source_key: String,
    part_file: String,
    append_file: String,
    upload_id: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum LivePolicy {
    Ready,
    RequiresCapability(&'static str),
}

#[derive(Debug)]
struct MatrixCase {
    command: &'static str,
    args: Vec<String>,
    policy: LivePolicy,
}

fn live_env() -> Option<LiveEnv> {
    let endpoint = std::env::var("TOS_ENDPOINT").ok()?;
    Some(LiveEnv {
        access_key: std::env::var("TOS_ACCESS_KEY").ok()?,
        secret_key: std::env::var("TOS_SECRET_KEY").ok()?,
        region: std::env::var("TOS_REGION")
            .ok()
            .or_else(|| derive_region_from_endpoint(&endpoint))?,
        endpoint,
    })
}

fn derive_region_from_endpoint(endpoint: &str) -> Option<String> {
    let host = endpoint
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()?;
    host.strip_prefix("tos-")
        .and_then(|rest| rest.split('.').next())
        .map(ToString::to_string)
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    format!("{:x}", nanos)
}

fn fixture() -> CoreFixture {
    let suffix = unique_suffix();
    let base_dir = std::env::temp_dir().join(format!("tos-core-matrix-{suffix}"));
    std::fs::create_dir_all(&base_dir).expect("create core matrix temp dir");

    let part_file = base_dir.join("part.bin");
    let append_file = base_dir.join("append.bin");
    std::fs::write(&part_file, format!("part-{suffix}")).expect("write part file");
    std::fs::write(&append_file, format!("append-{suffix}")).expect("write append file");

    CoreFixture {
        base_dir: base_dir.to_string_lossy().into_owned(),
        home_dir: base_dir.join("home").to_string_lossy().into_owned(),
        bucket: format!("ve-tos-cli-core-matrix-{}", &suffix[..16]),
        object_key: format!("matrix/{suffix}.bin"),
        source_key: format!("matrix/{suffix}-source.bin"),
        part_file: part_file.to_string_lossy().into_owned(),
        append_file: append_file.to_string_lossy().into_owned(),
        upload_id: "matrix-upload-id".to_string(),
    }
}

fn cli_live(args: &[String], env: &LiveEnv, home_dir: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env("HOME", home_dir)
        .env_remove("TOS_ACCESS_KEY")
        .env_remove("TOS_SECRET_KEY")
        .env_remove("TOS_SECURITY_TOKEN")
        .env_remove("TOS_CONTROL_ENDPOINT")
        .env("TOS_ACCESS_KEY", &env.access_key)
        .env("TOS_SECRET_KEY", &env.secret_key)
        .env("TOS_ENDPOINT", &env.endpoint)
        .env("TOS_REGION", &env.region)
        .arg("--output")
        .arg("json")
        .args(args)
        .output()
        .expect("execute ve-storage-uni-cli")
}

fn tos_args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

fn expected_bucket_flags() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    BTreeMap::from([
        (
            "create",
            flags(&[
                "--region",
                "--storage-class",
                "--bucket-type",
                "--project-name",
                "--bucket-object-lock-enabled",
                "--acl",
                "--grant-full-control",
                "--grant-read",
                "--grant-read-non-list",
                "--grant-read-acp",
                "--grant-write",
                "--grant-write-acp",
                "--az-redundancy",
                "--tagging",
            ]),
        ),
        ("head", flags(&[])),
        ("delete", flags(&["--force", "--destroy"])),
        ("list", flags(&["--project-name", "--bucket-type"])),
        ("stat", flags(&[])),
        ("info", flags(&[])),
        ("location", flags(&[])),
    ])
}

fn bucket_matrix(fixture: &CoreFixture) -> Vec<MatrixCase> {
    vec![
        MatrixCase {
            command: "create",
            policy: LivePolicy::RequiresCapability(
                "ObjectLock/ACL/grant/tagging bucket creation needs account capability review",
            ),
            args: tos_args(&[
                "ve-tos",
                "bucket",
                "create",
                &fixture.bucket,
                "--region",
                "cn-beijing",
                "--storage-class",
                "STANDARD",
                "--bucket-type",
                "hns",
                "--project-name",
                "matrix-project",
                "--bucket-object-lock-enabled",
                "--acl",
                "private",
                "--grant-full-control",
                "id=matrix",
                "--grant-read",
                "id=matrix",
                "--grant-read-non-list",
                "id=matrix",
                "--grant-read-acp",
                "id=matrix",
                "--grant-write",
                "id=matrix",
                "--grant-write-acp",
                "id=matrix",
                "--az-redundancy",
                "multi-az",
                "--tagging",
                "matrix=bucket",
            ]),
        },
        MatrixCase {
            command: "head",
            policy: LivePolicy::Ready,
            args: tos_args(&["ve-tos", "bucket", "head", &fixture.bucket]),
        },
        MatrixCase {
            command: "delete",
            policy: LivePolicy::RequiresCapability(
                "covered by teardown cleanup; do not run before dependent group cases",
            ),
            args: tos_args(&["ve-tos", "bucket", "delete", &fixture.bucket, "--force"]),
        },
        MatrixCase {
            command: "delete",
            policy: LivePolicy::RequiresCapability(
                "destroy is irreversible and must be confirmed against disposable buckets",
            ),
            args: tos_args(&["ve-tos", "bucket", "delete", &fixture.bucket, "--destroy"]),
        },
        MatrixCase {
            command: "list",
            policy: LivePolicy::Ready,
            args: tos_args(&[
                "ve-tos",
                "bucket",
                "list",
                "--project-name",
                "matrix-project",
                "--bucket-type",
                "hns",
            ]),
        },
        MatrixCase {
            command: "stat",
            policy: LivePolicy::Ready,
            args: tos_args(&["ve-tos", "bucket", "stat", &fixture.bucket]),
        },
        MatrixCase {
            command: "info",
            policy: LivePolicy::Ready,
            args: tos_args(&["ve-tos", "bucket", "info", &fixture.bucket]),
        },
        MatrixCase {
            command: "location",
            policy: LivePolicy::Ready,
            args: tos_args(&["ve-tos", "bucket", "location", &fixture.bucket]),
        },
    ]
}

fn expected_multipart_flags() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    BTreeMap::from([
        (
            "create",
            flags(&[
                "--bucket",
                "--key",
                "--forbid-overwrite",
                "--etag-pattern",
                "--acl",
                "--grant-full-control",
                "--grant-read",
                "--grant-read-non-list",
                "--grant-read-acp",
                "--grant-write",
                "--grant-write-acp",
                "--persistent-headers",
                "--object-lock-mode",
                "--object-lock-retain-until-date",
                "--if-unmodified-since",
                "--if-none-match",
                "--if-match",
                "--tagging",
                "--replicated-from",
                "--crr-source-version-id",
                "--crr-source-last-modify-time",
                "--crr-source-timestamp-nsec",
                "--crr-source-bucket-version-status",
                "--crr-source-upload-id",
                "--from-modular",
            ]),
        ),
        (
            "upload",
            flags(&[
                "--bucket",
                "--key",
                "--upload-id",
                "--part-number",
                "--body",
                "--content-md5",
                "--content-sha256",
                "--hash-crc64ecma",
                "--decoded-content-length",
                "--traffic-limit",
            ]),
        ),
        (
            "complete",
            flags(&[
                "--bucket",
                "--key",
                "--upload-id",
                "--parts",
                "--complete-all",
                "--if-unmodified-since",
                "--if-none-match",
                "--if-match",
                "--server-side-encryption",
                "--from-modular",
            ]),
        ),
        (
            "abort",
            flags(&[
                "--bucket",
                "--key",
                "--upload-id",
                "--force",
                "--from-modular",
            ]),
        ),
        (
            "copy",
            flags(&[
                "--bucket",
                "--key",
                "--upload-id",
                "--part-number",
                "--copy-source",
                "--copy-source-range",
                "--copy-source-part-number",
                "--copy-source-if-modified-since",
                "--copy-source-if-unmodified-since",
                "--etag-pattern",
                "--traffic-limit",
            ]),
        ),
        (
            "list-parts",
            flags(&[
                "--bucket",
                "--key",
                "--upload-id",
                "--part-number-marker",
                "--max-parts",
                "--fetch-from-kv",
            ]),
        ),
        (
            "list",
            flags(&[
                "--bucket",
                "--prefix",
                "--delimiter",
                "--key-marker",
                "--upload-id-marker",
                "--max-uploads",
                "--encoding-type",
                "--fetch-from-kv",
            ]),
        ),
    ])
}

fn multipart_matrix(fixture: &CoreFixture) -> Vec<MatrixCase> {
    vec![
        MatrixCase {
            command: "create",
            policy: LivePolicy::RequiresCapability(
                "full create matrix includes ACL/grant/ObjectLock/CRR/internal headers",
            ),
            args: tos_args(&[
                "ve-tos",
                "multipart",
                "create",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--forbid-overwrite",
                "--etag-pattern",
                "normal",
                "--acl",
                "private",
                "--grant-full-control",
                "id=matrix",
                "--grant-read",
                "id=matrix",
                "--grant-read-non-list",
                "id=matrix",
                "--grant-read-acp",
                "id=matrix",
                "--grant-write",
                "id=matrix",
                "--grant-write-acp",
                "id=matrix",
                "--persistent-headers",
                "content-type",
                "--object-lock-mode",
                "GOVERNANCE",
                "--object-lock-retain-until-date",
                "2099-01-01T00:00:00Z",
                "--if-unmodified-since",
                "Wed, 21 Oct 2099 07:28:00 GMT",
                "--if-none-match",
                "no-such-etag",
                "--if-match",
                "*",
                "--tagging",
                "matrix=multipart",
                "--replicated-from",
                "matrix",
                "--crr-source-version-id",
                "matrix-version",
                "--crr-source-last-modify-time",
                "0",
                "--crr-source-timestamp-nsec",
                "0",
                "--crr-source-bucket-version-status",
                "Enabled",
                "--crr-source-upload-id",
                "matrix-upload",
                "--from-modular",
                "matrix",
            ]),
        },
        MatrixCase {
            command: "upload",
            policy: LivePolicy::RequiresCapability(
                "checksum flags must match the generated part payload",
            ),
            args: tos_args(&[
                "ve-tos",
                "multipart",
                "upload",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--upload-id",
                &fixture.upload_id,
                "--part-number",
                "1",
                "--body",
                &fixture.part_file,
                "--content-md5",
                "matrix-md5",
                "--content-sha256",
                "matrix-sha256",
                "--hash-crc64ecma",
                "0",
                "--decoded-content-length",
                "1",
                "--traffic-limit",
                "1048576",
            ]),
        },
        MatrixCase {
            command: "complete",
            policy: LivePolicy::RequiresCapability("requires a real upload id and uploaded parts"),
            args: tos_args(&[
                "ve-tos",
                "multipart",
                "complete",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--upload-id",
                &fixture.upload_id,
                "--parts",
                r#"[{"part_number":1,"etag":"matrix"}]"#,
                "--complete-all",
                "--if-unmodified-since",
                "Wed, 21 Oct 2099 07:28:00 GMT",
                "--if-none-match",
                "no-such-etag",
                "--if-match",
                "*",
                "--server-side-encryption",
                "AES256",
                "--from-modular",
                "matrix",
            ]),
        },
        MatrixCase {
            command: "abort",
            policy: LivePolicy::RequiresCapability("requires a real disposable upload id"),
            args: tos_args(&[
                "ve-tos",
                "multipart",
                "abort",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--upload-id",
                &fixture.upload_id,
                "--force",
                "--from-modular",
                "matrix",
            ]),
        },
        MatrixCase {
            command: "copy",
            policy: LivePolicy::RequiresCapability("requires a real upload id and source object"),
            args: tos_args(&[
                "ve-tos",
                "multipart",
                "copy",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--upload-id",
                &fixture.upload_id,
                "--part-number",
                "1",
                "--copy-source",
                &format!("/{}/{}", fixture.bucket, fixture.source_key),
                "--copy-source-range",
                "bytes=0-3",
                "--copy-source-part-number",
                "1",
                "--copy-source-if-modified-since",
                "Wed, 21 Oct 2015 07:28:00 GMT",
                "--copy-source-if-unmodified-since",
                "Wed, 21 Oct 2099 07:28:00 GMT",
                "--etag-pattern",
                "normal",
                "--traffic-limit",
                "1048576",
            ]),
        },
        MatrixCase {
            command: "list-parts",
            policy: LivePolicy::RequiresCapability("requires a real upload id"),
            args: tos_args(&[
                "ve-tos",
                "multipart",
                "list-parts",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--upload-id",
                &fixture.upload_id,
                "--part-number-marker",
                "1",
                "--max-parts",
                "100",
                "--fetch-from-kv",
            ]),
        },
        MatrixCase {
            command: "list",
            policy: LivePolicy::Ready,
            args: tos_args(&[
                "ve-tos",
                "multipart",
                "list",
                "--bucket",
                &fixture.bucket,
                "--prefix",
                "matrix/",
                "--delimiter",
                "/",
                "--key-marker",
                "matrix/key",
                "--upload-id-marker",
                "matrix-upload",
                "--max-uploads",
                "100",
                "--encoding-type",
                "url",
                "--fetch-from-kv",
            ]),
        },
    ]
}

fn expected_turbo_flags() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    BTreeMap::from([
        (
            "open",
            flags(&[
                "--bucket",
                "--key",
                "--content-type",
                "--content-md5",
                "--hash-crc64ecma",
                "--traffic-limit",
                "--if-match-guard-object",
                "--acl",
                "--grant-full-control",
                "--grant-read",
                "--grant-read-non-list",
                "--grant-read-acp",
                "--grant-write",
                "--grant-write-acp",
            ]),
        ),
        (
            "append",
            flags(&[
                "--bucket",
                "--key",
                "--body",
                "--turbo-token",
                "--content-md5",
                "--hash-crc64ecma",
                "--traffic-limit",
                "--if-match-guard-object",
                "--acl",
                "--grant-full-control",
                "--grant-read",
                "--grant-read-non-list",
                "--grant-read-acp",
                "--grant-write",
                "--grant-write-acp",
            ]),
        ),
        (
            "list",
            flags(&[
                "--bucket",
                "--key",
                "--marker",
                "--max-keys",
                "--prefix",
                "--encoding-type",
            ]),
        ),
        (
            "close",
            flags(&[
                "--bucket",
                "--key",
                "--traffic-limit",
                "--if-match-guard-object",
                "--turbo-token",
                "--acl",
                "--grant-full-control",
                "--grant-read",
                "--grant-read-non-list",
                "--grant-read-acp",
                "--grant-write",
                "--grant-write-acp",
            ]),
        ),
    ])
}

fn turbo_matrix(fixture: &CoreFixture) -> Vec<MatrixCase> {
    vec![
        MatrixCase {
            command: "open",
            policy: LivePolicy::RequiresCapability("Turbo open requires account/bucket Turbo capability and compatible checksum/ACL headers"),
            args: tos_args(&[
                "ve-tos",
                "turbo",
                "open",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--content-type",
                "application/octet-stream",
                "--content-md5",
                "matrix-md5",
                "--hash-crc64ecma",
                "0",
                "--traffic-limit",
                "1048576",
                "--if-match-guard-object",
                "matrix-guard",
                "--acl",
                "private",
                "--grant-full-control",
                "id=matrix",
                "--grant-read",
                "id=matrix",
                "--grant-read-non-list",
                "id=matrix",
                "--grant-read-acp",
                "id=matrix",
                "--grant-write",
                "id=matrix",
                "--grant-write-acp",
                "id=matrix",
            ]),
        },
        MatrixCase {
            command: "append",
            policy: LivePolicy::RequiresCapability("Turbo append requires a valid Turbo token and compatible checksum/ACL headers"),
            args: tos_args(&[
                "ve-tos",
                "turbo",
                "append",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--body",
                &fixture.append_file,
                "--turbo-token",
                "matrix-token",
                "--content-md5",
                "matrix-md5",
                "--hash-crc64ecma",
                "0",
                "--traffic-limit",
                "1048576",
                "--if-match-guard-object",
                "matrix-guard",
                "--acl",
                "private",
                "--grant-full-control",
                "id=matrix",
                "--grant-read",
                "id=matrix",
                "--grant-read-non-list",
                "id=matrix",
                "--grant-read-acp",
                "id=matrix",
                "--grant-write",
                "id=matrix",
                "--grant-write-acp",
                "id=matrix",
            ]),
        },
        MatrixCase {
            command: "list",
            policy: LivePolicy::RequiresCapability("Turbo list requires bucket Turbo capability"),
            args: tos_args(&[
                "ve-tos",
                "turbo",
                "list",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--marker",
                "matrix-marker",
                "--max-keys",
                "100",
                "--prefix",
                "matrix/",
                "--encoding-type",
                "url",
            ]),
        },
        MatrixCase {
            command: "close",
            policy: LivePolicy::RequiresCapability("Turbo close requires an opened Turbo session"),
            args: tos_args(&[
                "ve-tos",
                "turbo",
                "close",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--traffic-limit",
                "1048576",
                "--if-match-guard-object",
                "matrix-guard",
                "--turbo-token",
                "matrix-token",
                "--acl",
                "private",
                "--grant-full-control",
                "id=matrix",
                "--grant-read",
                "id=matrix",
                "--grant-read-non-list",
                "id=matrix",
                "--grant-read-acp",
                "id=matrix",
                "--grant-write",
                "id=matrix",
                "--grant-write-acp",
                "id=matrix",
            ]),
        },
    ]
}

#[test]
fn test_bucket_multipart_turbo_live_matrices_cover_all_parameters() {
    let fixture = fixture();
    assert_matrix("bucket", expected_bucket_flags(), &bucket_matrix(&fixture));
    assert_matrix(
        "multipart",
        expected_multipart_flags(),
        &multipart_matrix(&fixture),
    );
    assert_matrix("turbo", expected_turbo_flags(), &turbo_matrix(&fixture));
    let _ = std::fs::remove_dir_all(&fixture.base_dir);
}

#[test]
#[ignore = "requires live TOS credentials and capability review for every non-Ready case"]
fn test_bucket_multipart_turbo_live_ready_matrix_with_teardown() {
    let Some(env) = live_env() else {
        eprintln!(
            "skip core live matrix: missing TOS_ACCESS_KEY/TOS_SECRET_KEY/TOS_ENDPOINT/TOS_REGION"
        );
        return;
    };
    let fixture = fixture();
    std::fs::create_dir_all(&fixture.home_dir).expect("create temp home");

    let create_bucket = cli_live(
        &tos_args(&["ve-tos", "bucket", "create", &fixture.bucket]),
        &env,
        &fixture.home_dir,
    );
    assert_success("bucket create", &create_bucket);

    for case in bucket_matrix(&fixture)
        .into_iter()
        .chain(multipart_matrix(&fixture))
        .chain(turbo_matrix(&fixture))
    {
        match case.policy {
            LivePolicy::Ready => {
                assert_success(case.command, &cli_live(&case.args, &env, &fixture.home_dir))
            }
            LivePolicy::RequiresCapability(reason) => {
                eprintln!("manual live case pending: {} ({reason})", case.command);
            }
        }
    }

    let delete_bucket = cli_live(
        &tos_args(&["ve-tos", "bucket", "delete", &fixture.bucket, "--force"]),
        &env,
        &fixture.home_dir,
    );
    if !delete_bucket.status.success() {
        let destroy_bucket = cli_live(
            &tos_args(&["ve-tos", "bucket", "delete", &fixture.bucket, "--destroy"]),
            &env,
            &fixture.home_dir,
        );
        assert_success("bucket destroy cleanup", &destroy_bucket);
    }
    let _ = std::fs::remove_dir_all(&fixture.base_dir);
}

fn flags(values: &[&'static str]) -> BTreeSet<&'static str> {
    values.iter().copied().collect()
}

fn assert_matrix(
    group: &str,
    expected: BTreeMap<&'static str, BTreeSet<&'static str>>,
    cases: &[MatrixCase],
) {
    let mut actual = BTreeMap::<&str, BTreeSet<&str>>::new();
    for case in cases {
        let entry = actual.entry(case.command).or_default();
        for arg in &case.args {
            if arg.starts_with("--") {
                entry.insert(arg);
            }
        }
    }
    assert_eq!(
        actual.keys().copied().collect::<BTreeSet<_>>(),
        expected.keys().copied().collect::<BTreeSet<_>>(),
        "{group} command coverage mismatch",
    );
    for (command, expected_flags) in expected {
        let actual_flags = actual.get(command).cloned().unwrap_or_default();
        assert_eq!(
            actual_flags, expected_flags,
            "{group} {command} flag coverage mismatch",
        );
    }
}

fn assert_success(command: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{command} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
