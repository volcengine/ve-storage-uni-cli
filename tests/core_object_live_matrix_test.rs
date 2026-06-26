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

use std::collections::BTreeSet;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const EXPECTED_OBJECT_COMMANDS: &[&str] = &[
    "upload",
    "download",
    "form-upload",
    "copy",
    "delete",
    "batch-delete",
    "list",
    "list-versions",
    "head",
    "stat",
    "set-meta",
    "set-time",
    "set-expires",
    "append",
    "seal-append",
    "modify",
    "rename",
    "restore",
    "status",
    "get-acl",
    "set-acl",
    "get-tagging",
    "set-tagging",
    "delete-tagging",
    "link",
    "get-symlink",
    "create-symlink",
    "get-fetch-task",
    "create-fetch-task",
    "fetch",
    "set-retention",
    "get-retention",
];

#[derive(Debug)]
struct LiveEnv {
    access_key: String,
    secret_key: String,
    endpoint: String,
    region: String,
}

#[derive(Debug)]
struct ObjectFixture {
    base_dir: String,
    bucket: String,
    object_key: String,
    copy_key: String,
    append_key: String,
    link_key: String,
    symlink_key: String,
    renamed_key: String,
    missing_version_id: String,
    task_id: String,
    source_url: String,
    upload_file: String,
    form_file: String,
    append_file: String,
    modify_file: String,
    download_file: String,
    home_dir: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum LivePolicy {
    Ready,
    RequiresCapability(&'static str),
}

#[derive(Debug)]
struct ObjectCommandCase {
    command: &'static str,
    covered_flags: &'static [&'static str],
    policy: LivePolicy,
    args: Vec<String>,
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

fn fixture() -> ObjectFixture {
    let suffix = unique_suffix();
    let base_dir = std::env::temp_dir().join(format!("tos-object-matrix-{suffix}"));
    std::fs::create_dir_all(&base_dir).expect("create object matrix temp dir");

    let upload_file = base_dir.join("upload.txt");
    let form_file = base_dir.join("form.txt");
    let append_file = base_dir.join("append.txt");
    let modify_file = base_dir.join("modify.txt");
    let download_file = base_dir.join("download.txt");
    std::fs::write(&upload_file, format!("upload-{suffix}")).expect("write upload file");
    std::fs::write(&form_file, format!("form-{suffix}")).expect("write form file");
    std::fs::write(&append_file, format!("append-{suffix}")).expect("write append file");
    std::fs::write(&modify_file, "MOD").expect("write modify file");

    ObjectFixture {
        base_dir: base_dir.to_string_lossy().into_owned(),
        bucket: format!("ve-tos-cli-object-{suffix}", suffix = &suffix[..16]),
        object_key: format!("object/{suffix}.txt"),
        copy_key: format!("object/{suffix}-copy.txt"),
        append_key: format!("object/{suffix}-append.txt"),
        link_key: format!("object/{suffix}-link.txt"),
        symlink_key: format!("object/{suffix}-symlink.txt"),
        renamed_key: format!("object/{suffix}-renamed.txt"),
        missing_version_id: "matrix-version-id".to_string(),
        task_id: "matrix-task-id".to_string(),
        source_url: "https://example.com/tos-object-matrix.txt".to_string(),
        upload_file: upload_file.to_string_lossy().into_owned(),
        form_file: form_file.to_string_lossy().into_owned(),
        append_file: append_file.to_string_lossy().into_owned(),
        modify_file: modify_file.to_string_lossy().into_owned(),
        download_file: download_file.to_string_lossy().into_owned(),
        home_dir: base_dir.join("home").to_string_lossy().into_owned(),
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

fn object_matrix(fixture: &ObjectFixture) -> Vec<ObjectCommandCase> {
    vec![
        ObjectCommandCase {
            command: "upload",
            covered_flags: &["--bucket", "--key", "--body", "--content-type", "--storage-class", "--meta", "--net-speed-test"],
            policy: LivePolicy::Ready,
            args: tos_args(&[
                "ve-tos",
                "object",
                "upload",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--body",
                &fixture.upload_file,
                "--content-type",
                "text/plain",
                "--storage-class",
                "STANDARD",
                "--meta",
                "matrix=true",
                "--net-speed-test",
                "matrix",
            ]),
        },
        ObjectCommandCase {
            command: "download",
            covered_flags: &["--bucket", "--key", "--body", "--version-id", "--range", "--if-modified-since", "--if-unmodified-since", "--replicated-from", "--from-modular"],
            policy: LivePolicy::RequiresCapability("conditional headers require service-compatible object timestamps"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "download",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--body",
                &fixture.download_file,
                "--version-id",
                &fixture.missing_version_id,
                "--range",
                "bytes=0-3",
                "--if-modified-since",
                "Wed, 21 Oct 2015 07:28:00 GMT",
                "--if-unmodified-since",
                "Wed, 21 Oct 2099 07:28:00 GMT",
                "--replicated-from",
                "matrix",
                "--from-modular",
                "matrix",
            ]),
        },
        ObjectCommandCase {
            command: "form-upload",
            covered_flags: &["--bucket", "--key", "--body", "--content-type", "--storage-class", "--meta"],
            policy: LivePolicy::Ready,
            args: tos_args(&[
                "ve-tos",
                "object",
                "form-upload",
                "--bucket",
                &fixture.bucket,
                "--key",
                "object/form-upload.txt",
                "--body",
                &fixture.form_file,
                "--content-type",
                "text/plain",
                "--storage-class",
                "STANDARD",
                "--meta",
                "matrix=form",
            ]),
        },
        ObjectCommandCase {
            command: "copy",
            covered_flags: &["--range", "--copy-source-if-modified-since", "--copy-source-if-unmodified-since", "--etag-pattern", "--metadata-directive", "--tagging-directive", "--unique-tag", "--copy-source-last-modified", "--data-id", "--finger-print", "--internal-metadata-directive", "--crr-source-timestamp-nsec", "--crr-proxy", "--crr-source-bucket-version-status", "--traffic-limit", "--object-lock-mode", "--object-lock-retain-until-date", "--if-unmodified-since", "--if-none-match", "--if-match", "--persistent-headers", "--tagging", "--acl", "--grant-full-control", "--grant-read", "--grant-read-non-list", "--grant-read-acp", "--grant-write", "--grant-write-acp"],
            policy: LivePolicy::RequiresCapability("full copy header matrix includes internal/CRR/ObjectLock headers"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "copy",
                &format!("tos://{}/{}", fixture.bucket, fixture.object_key),
                &format!("tos://{}/{}", fixture.bucket, fixture.copy_key),
                "--range",
                "bytes=0-3",
                "--copy-source-if-modified-since",
                "Wed, 21 Oct 2015 07:28:00 GMT",
                "--copy-source-if-unmodified-since",
                "Wed, 21 Oct 2099 07:28:00 GMT",
                "--etag-pattern",
                "normal",
                "--metadata-directive",
                "COPY",
                "--tagging-directive",
                "COPY",
                "--unique-tag",
                "matrix",
                "--copy-source-last-modified",
                "0",
                "--data-id",
                "matrix-data",
                "--finger-print",
                "matrix-fp",
                "--internal-metadata-directive",
                "COPY",
                "--crr-source-timestamp-nsec",
                "0",
                "--crr-proxy",
                "matrix",
                "--crr-source-bucket-version-status",
                "Enabled",
                "--traffic-limit",
                "1048576",
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
                "--persistent-headers",
                "content-type",
                "--tagging",
                "matrix=copy",
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
        ObjectCommandCase {
            command: "delete",
            covered_flags: &["--bucket", "--key", "--version-id", "--force", "--from-modular", "--if-match-expires", "--last-modified", "--if-match-create-time", "--if-match", "--if-match-tags", "--if-match-access-time", "--lifecycle-directly-delete-versions", "--if-match-inode-id", "--parent-inode-id", "--only-put-delete-marker", "--inner-properties-timestamp", "--inner-properties-timestamp-nsec"],
            policy: LivePolicy::RequiresCapability("destructive conditional delete matrix must target disposable objects"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--version-id",
                &fixture.missing_version_id,
                "--force",
                "--from-modular",
                "matrix",
                "--if-match-expires",
                "0",
                "--last-modified",
                "0",
                "--if-match-create-time",
                "0",
                "--if-match",
                "*",
                "--if-match-tags",
                "matrix=true",
                "--if-match-access-time",
                "0",
                "--lifecycle-directly-delete-versions",
                "--if-match-inode-id",
                "1",
                "--parent-inode-id",
                "1",
                "--only-put-delete-marker",
                "--inner-properties-timestamp",
                "0",
                "--inner-properties-timestamp-nsec",
                "0",
            ]),
        },
        ObjectCommandCase {
            command: "batch-delete",
            covered_flags: &["--bucket", "--keys", "--force", "--recursive", "--skip-trash", "--content-md5"],
            policy: LivePolicy::RequiresCapability("content-md5 must match generated delete payload"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "batch-delete",
                "--bucket",
                &fixture.bucket,
                "--keys",
                "object/delete-a.txt,object/delete-b.txt",
                "--force",
                "--recursive",
                "--skip-trash",
                "--content-md5",
                "matrix-md5",
            ]),
        },
        ObjectCommandCase {
            command: "list",
            covered_flags: &["--bucket", "--prefix", "--delimiter", "--max-keys", "--continuation-token"],
            policy: LivePolicy::RequiresCapability("continuation-token must be returned by a previous list response"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "list",
                "--bucket",
                &fixture.bucket,
                "--prefix",
                "object/",
                "--delimiter",
                "/",
                "--max-keys",
                "10",
                "--continuation-token",
                "matrix-token",
            ]),
        },
        ObjectCommandCase {
            command: "list-versions",
            covered_flags: &["--bucket", "--prefix"],
            policy: LivePolicy::Ready,
            args: tos_args(&[
                "ve-tos",
                "object",
                "list-versions",
                "--bucket",
                &fixture.bucket,
                "--prefix",
                "object/",
            ]),
        },
        ObjectCommandCase {
            command: "head",
            covered_flags: &["--bucket", "--key", "--version-id", "--if-modified-since", "--if-unmodified-since", "--replicated-from", "--from-modular"],
            policy: LivePolicy::RequiresCapability("conditional headers require service-compatible object timestamps"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "head",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--version-id",
                &fixture.missing_version_id,
                "--if-modified-since",
                "Wed, 21 Oct 2015 07:28:00 GMT",
                "--if-unmodified-since",
                "Wed, 21 Oct 2099 07:28:00 GMT",
                "--replicated-from",
                "matrix",
                "--from-modular",
                "matrix",
            ]),
        },
        ObjectCommandCase {
            command: "stat",
            covered_flags: &["--bucket", "--key"],
            policy: LivePolicy::Ready,
            args: tos_args(&["ve-tos", "object", "stat", "--bucket", &fixture.bucket, "--key", &fixture.object_key]),
        },
        ObjectCommandCase {
            command: "set-meta",
            covered_flags: &["--bucket", "--key", "--meta", "--version-id", "--unique-tag", "--content-type"],
            policy: LivePolicy::RequiresCapability("metadata update support varies by object state and service capability"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "set-meta",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--meta",
                "matrix=meta",
                "--version-id",
                &fixture.missing_version_id,
                "--unique-tag",
                "matrix",
                "--content-type",
                "text/plain",
            ]),
        },
        ObjectCommandCase {
            command: "set-time",
            covered_flags: &["--bucket", "--key", "--time", "--modify-timestamp", "--modify-timestamp-ns"],
            policy: LivePolicy::RequiresCapability("time mutation is service-capability dependent"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "set-time",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--time",
                "0",
                "--modify-timestamp",
                "0",
                "--modify-timestamp-ns",
                "0",
            ]),
        },
        ObjectCommandCase {
            command: "set-expires",
            covered_flags: &["--bucket", "--key", "--expires", "--version-id"],
            policy: LivePolicy::RequiresCapability("object expiration mutation is service-capability dependent"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "set-expires",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--expires",
                "2099-01-01T00:00:00Z",
                "--version-id",
                &fixture.missing_version_id,
            ]),
        },
        ObjectCommandCase {
            command: "append",
            covered_flags: &["--bucket", "--key", "--body", "--offset", "--append-last-time", "--version-id", "--content-type", "--content-md5", "--content-sha256", "--decoded-content-length", "--object-lock-mode", "--object-lock-retain-until-date", "--acl", "--grant-full-control", "--grant-read", "--grant-read-non-list", "--grant-read-acp", "--grant-write", "--grant-write-acp", "--persistent-headers", "--traffic-limit", "--if-none-match", "--if-match"],
            policy: LivePolicy::RequiresCapability("full append header matrix includes checksum, ACL, ObjectLock, and conditional headers"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "append",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.append_key,
                "--body",
                &fixture.append_file,
                "--offset",
                "0",
                "--append-last-time",
                "0",
                "--version-id",
                &fixture.missing_version_id,
                "--content-type",
                "text/plain",
                "--content-md5",
                "matrix-md5",
                "--content-sha256",
                "matrix-sha256",
                "--decoded-content-length",
                "1",
                "--object-lock-mode",
                "GOVERNANCE",
                "--object-lock-retain-until-date",
                "2099-01-01T00:00:00Z",
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
                "--traffic-limit",
                "1048576",
                "--if-none-match",
                "no-such-etag",
                "--if-match",
                "*",
            ]),
        },
        ObjectCommandCase {
            command: "seal-append",
            covered_flags: &["--bucket", "--key", "--offset", "--version-id", "--acl", "--grant-full-control", "--grant-read", "--grant-read-non-list", "--grant-read-acp", "--grant-write", "--grant-write-acp", "--if-none-match", "--if-match"],
            policy: LivePolicy::RequiresCapability("requires an existing appendable object and compatible ACL/condition headers"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "seal-append",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.append_key,
                "--offset",
                "0",
                "--version-id",
                &fixture.missing_version_id,
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
                "--if-none-match",
                "no-such-etag",
                "--if-match",
                "*",
            ]),
        },
        ObjectCommandCase {
            command: "modify",
            covered_flags: &["--bucket", "--key", "--body", "--offset", "--version-id", "--content-type", "--content-md5", "--traffic-limit", "--if-none-match", "--if-match"],
            policy: LivePolicy::RequiresCapability("modify object support depends on bucket/object type"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "modify",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--body",
                &fixture.modify_file,
                "--offset",
                "0",
                "--version-id",
                &fixture.missing_version_id,
                "--content-type",
                "text/plain",
                "--content-md5",
                "matrix-md5",
                "--traffic-limit",
                "1048576",
                "--if-none-match",
                "no-such-etag",
                "--if-match",
                "*",
            ]),
        },
        ObjectCommandCase {
            command: "rename",
            covered_flags: &["--recursive-mkdir", "--not-update-timestamp", "--forbid-overwrite", "--trace-id"],
            policy: LivePolicy::RequiresCapability("rename support depends on bucket namespace capability"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "rename",
                &format!("tos://{}/{}", fixture.bucket, fixture.object_key),
                &format!("tos://{}/{}", fixture.bucket, fixture.renamed_key),
                "--recursive-mkdir",
                "--not-update-timestamp",
                "--forbid-overwrite",
                "--trace-id",
                "matrix-trace-id",
            ]),
        },
        ObjectCommandCase {
            command: "restore",
            covered_flags: &["--bucket", "--key", "--days", "--version-id", "--content-md5"],
            policy: LivePolicy::RequiresCapability("requires an archived object"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "restore",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--days",
                "1",
                "--version-id",
                &fixture.missing_version_id,
                "--content-md5",
                "matrix-md5",
            ]),
        },
        ObjectCommandCase {
            command: "status",
            covered_flags: &["--bucket", "--key"],
            policy: LivePolicy::Ready,
            args: tos_args(&["ve-tos", "object", "status", "--bucket", &fixture.bucket, "--key", &fixture.object_key]),
        },
        ObjectCommandCase {
            command: "get-acl",
            covered_flags: &["--bucket", "--key", "--version-id"],
            policy: LivePolicy::RequiresCapability("version-id must refer to an existing object version"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "get-acl",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--version-id",
                &fixture.missing_version_id,
            ]),
        },
        ObjectCommandCase {
            command: "set-acl",
            covered_flags: &["--bucket", "--key", "--acl", "--version-id"],
            policy: LivePolicy::RequiresCapability("version-id must refer to an existing object version"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "set-acl",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--acl",
                "private",
                "--version-id",
                &fixture.missing_version_id,
            ]),
        },
        ObjectCommandCase {
            command: "get-tagging",
            covered_flags: &["--bucket", "--key", "--version-id"],
            policy: LivePolicy::RequiresCapability("version-id must refer to an existing object version"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "get-tagging",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--version-id",
                &fixture.missing_version_id,
            ]),
        },
        ObjectCommandCase {
            command: "set-tagging",
            covered_flags: &["--bucket", "--key", "--tags"],
            policy: LivePolicy::Ready,
            args: tos_args(&[
                "ve-tos",
                "object",
                "set-tagging",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--tags",
                "matrix=true",
            ]),
        },
        ObjectCommandCase {
            command: "delete-tagging",
            covered_flags: &["--bucket", "--key"],
            policy: LivePolicy::Ready,
            args: tos_args(&["ve-tos", "object", "delete-tagging", "--bucket", &fixture.bucket, "--key", &fixture.object_key]),
        },
        ObjectCommandCase {
            command: "link",
            covered_flags: &["--bucket", "--key", "--source-key", "--acl", "--grant-full-control", "--grant-read", "--grant-read-non-list", "--grant-read-acp", "--grant-write", "--grant-write-acp", "--tagging"],
            policy: LivePolicy::RequiresCapability("link support depends on bucket namespace capability"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "link",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.link_key,
                "--source-key",
                &fixture.object_key,
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
                "--tagging",
                "matrix=link",
            ]),
        },
        ObjectCommandCase {
            command: "get-symlink",
            covered_flags: &["--bucket", "--key", "--version-id"],
            policy: LivePolicy::RequiresCapability("requires an existing symlink object/version"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "get-symlink",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.symlink_key,
                "--version-id",
                &fixture.missing_version_id,
            ]),
        },
        ObjectCommandCase {
            command: "create-symlink",
            covered_flags: &["--bucket", "--key", "--target-key", "--target-bucket", "--acl", "--grant-full-control", "--grant-read", "--grant-read-non-list", "--grant-read-acp", "--grant-write", "--grant-write-acp", "--tagging"],
            policy: LivePolicy::RequiresCapability("symlink support depends on bucket namespace capability"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "create-symlink",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.symlink_key,
                "--target-key",
                &fixture.object_key,
                "--target-bucket",
                &fixture.bucket,
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
                "--tagging",
                "matrix=symlink",
            ]),
        },
        ObjectCommandCase {
            command: "get-fetch-task",
            covered_flags: &["--bucket", "--task-id"],
            policy: LivePolicy::RequiresCapability("requires a real fetch task id"),
            args: tos_args(&["ve-tos", "object", "get-fetch-task", "--bucket", &fixture.bucket, "--task-id", &fixture.task_id]),
        },
        ObjectCommandCase {
            command: "create-fetch-task",
            covered_flags: &["--bucket", "--key", "--source-url", "--etag-pattern", "--traffic-limit", "--if-unmodified-since", "--if-none-match", "--if-match", "--object-lock-mode", "--object-lock-retain-until-date", "--acl", "--grant-full-control", "--grant-read", "--grant-read-non-list", "--grant-read-acp", "--grant-write", "--grant-write-acp"],
            policy: LivePolicy::RequiresCapability("depends on external source URL, ACL, ObjectLock, and condition header compatibility"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "create-fetch-task",
                "--bucket",
                &fixture.bucket,
                "--key",
                "object/fetch-task.txt",
                "--source-url",
                &fixture.source_url,
                "--etag-pattern",
                "normal",
                "--traffic-limit",
                "1048576",
                "--if-unmodified-since",
                "Wed, 21 Oct 2099 07:28:00 GMT",
                "--if-none-match",
                "no-such-etag",
                "--if-match",
                "*",
                "--object-lock-mode",
                "GOVERNANCE",
                "--object-lock-retain-until-date",
                "2099-01-01T00:00:00Z",
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
        ObjectCommandCase {
            command: "fetch",
            covered_flags: &["--bucket", "--key", "--source-url", "--storage-class", "--meta"],
            policy: LivePolicy::RequiresCapability("depends on external source URL accessibility"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "fetch",
                "--bucket",
                &fixture.bucket,
                "--key",
                "object/fetch.txt",
                "--source-url",
                &fixture.source_url,
                "--storage-class",
                "STANDARD",
                "--meta",
                "matrix=fetch",
            ]),
        },
        ObjectCommandCase {
            command: "set-retention",
            covered_flags: &["--bucket", "--key", "--mode", "--retain-until-date", "--version-id", "--content-md5"],
            policy: LivePolicy::RequiresCapability("requires ObjectLock-enabled bucket/object"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "set-retention",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--mode",
                "GOVERNANCE",
                "--retain-until-date",
                "2099-01-01T00:00:00Z",
                "--version-id",
                &fixture.missing_version_id,
                "--content-md5",
                "matrix-md5",
            ]),
        },
        ObjectCommandCase {
            command: "get-retention",
            covered_flags: &["--bucket", "--key", "--version-id"],
            policy: LivePolicy::RequiresCapability("requires ObjectLock-enabled bucket/object"),
            args: tos_args(&[
                "ve-tos",
                "object",
                "get-retention",
                "--bucket",
                &fixture.bucket,
                "--key",
                &fixture.object_key,
                "--version-id",
                &fixture.missing_version_id,
            ]),
        },
    ]
}

#[test]
fn test_object_live_matrix_covers_all_commands_and_parameters() {
    let fixture = fixture();
    let cases = object_matrix(&fixture);
    let actual_commands = cases
        .iter()
        .map(|case| case.command)
        .collect::<BTreeSet<_>>();
    let expected_commands = EXPECTED_OBJECT_COMMANDS
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_commands, expected_commands);

    for case in cases {
        assert!(
            !case.covered_flags.is_empty(),
            "{} must declare covered flags",
            case.command
        );
        for flag in case.covered_flags {
            assert!(
                case.args.iter().any(|arg| arg == flag),
                "{} does not pass declared flag {} in matrix args {:?}",
                case.command,
                flag,
                case.args
            );
        }
    }
    let _ = std::fs::remove_dir_all(&fixture.base_dir);
}

#[test]
#[ignore = "requires live TOS credentials and service capability review for every RequiresCapability case"]
fn test_object_live_full_command_parameter_matrix() {
    let Some(env) = live_env() else {
        eprintln!("skip object live matrix: missing TOS_ACCESS_KEY/TOS_SECRET_KEY/TOS_ENDPOINT/TOS_REGION");
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

    let matrix = object_matrix(&fixture);
    let ready_cases = matrix
        .iter()
        .filter(|case| case.policy == LivePolicy::Ready);
    for case in ready_cases {
        let output = cli_live(&case.args, &env, &fixture.home_dir);
        assert_success(case.command, &output);
    }

    for case in matrix
        .iter()
        .filter(|case| case.policy != LivePolicy::Ready)
    {
        if let LivePolicy::RequiresCapability(reason) = case.policy {
            eprintln!("manual live case pending: {} ({reason})", case.command);
        }
    }

    let cleanup_cases = [
        tos_args(&[
            "ve-tos",
            "object",
            "delete",
            "--bucket",
            &fixture.bucket,
            "--key",
            &fixture.object_key,
            "--force",
        ]),
        tos_args(&[
            "ve-tos",
            "object",
            "delete",
            "--bucket",
            &fixture.bucket,
            "--key",
            "object/form-upload.txt",
            "--force",
        ]),
    ];
    for args in cleanup_cases {
        let _ = cli_live(&args, &env, &fixture.home_dir);
    }
    let delete_bucket = cli_live(
        &tos_args(&["ve-tos", "bucket", "delete", &fixture.bucket, "--force"]),
        &env,
        &fixture.home_dir,
    );
    assert_success("bucket delete", &delete_bucket);
    let _ = std::fs::remove_dir_all(&fixture.base_dir);
}

fn assert_success(command: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{command} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
