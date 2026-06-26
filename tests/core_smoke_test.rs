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

use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn live_env() -> Option<(String, String, String)> {
    let access_key = std::env::var("TOS_ACCESS_KEY").ok()?;
    let secret_key = std::env::var("TOS_SECRET_KEY").ok()?;
    let endpoint = std::env::var("TOS_ENDPOINT").ok()?;
    Some((access_key, secret_key, endpoint))
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

fn cli_live(
    args: &[&str],
    envs: &(String, String, String),
    home_dir: &std::path::Path,
) -> std::process::Output {
    let region = derive_region_from_endpoint(&envs.2).expect("region from endpoint");
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env("HOME", home_dir)
        .env_remove("TOS_REGION")
        .env_remove("TOS_ACCESS_KEY")
        .env_remove("TOS_SECRET_KEY")
        .env_remove("TOS_SECURITY_TOKEN")
        .env_remove("TOS_CONTROL_ENDPOINT")
        .env("TOS_ACCESS_KEY", &envs.0)
        .env("TOS_SECRET_KEY", &envs.1)
        .env("TOS_ENDPOINT", &envs.2)
        .env("TOS_REGION", region)
        .arg("--output")
        .arg("json")
        .args(args)
        .output()
        .expect("Failed to execute ve-storage-uni-cli")
}

fn parse_json(output: &std::process::Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    serde_json::from_str(stdout.trim())
        .or_else(|_| serde_json::from_str(stderr.trim()))
        .unwrap_or_else(|err| {
            panic!("No valid JSON found: {err}\nstdout: {stdout}\nstderr: {stderr}")
        })
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_nanos();
    format!("{:x}", nanos)
}

fn extract_upload_id(json: &serde_json::Value) -> Option<String> {
    let data = &json["data"];
    data.pointer("/body/UploadId")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
        .or_else(|| {
            data.pointer("/body/upload_id")
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
        })
        .or_else(|| {
            data.pointer("/body/raw")
                .and_then(|v| v.as_str())
                .and_then(|raw| {
                    raw.split("<UploadId>")
                        .nth(1)
                        .and_then(|tail| tail.split("</UploadId>").next())
                        .map(ToString::to_string)
                })
        })
}

#[test]
fn test_core_live_bucket_object_and_multipart_smoke() {
    let Some(envs) = live_env() else {
        eprintln!("skip live smoke: missing TOS_ACCESS_KEY/TOS_SECRET_KEY/TOS_ENDPOINT");
        return;
    };

    let suffix = unique_suffix();
    let bucket = format!("ve-tos-cli-core-{}", &suffix[..16]);
    // [Review Fix #1] Bucket commands require URI-style positional targets in the live smoke path.
    let bucket_uri = format!("tos://{bucket}");
    let object_key = format!("smoke/{suffix}.txt");
    let body = format!("hello-{suffix}");
    let download_path = std::env::temp_dir().join(format!("tos-core-download-{suffix}.txt"));
    let home_dir = std::env::temp_dir().join(format!("tos-core-home-{suffix}"));
    std::fs::create_dir_all(&home_dir).expect("temp home");

    let create = cli_live(
        &["ve-tos", "bucket", "create", &bucket_uri],
        &envs,
        &home_dir,
    );
    assert!(
        create.status.success(),
        "bucket create failed: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    let head = cli_live(&["ve-tos", "bucket", "head", &bucket_uri], &envs, &home_dir);
    assert!(head.status.success(), "bucket head failed");

    let upload = cli_live(
        &[
            "ve-tos",
            "object",
            "upload",
            "--bucket",
            &bucket,
            "--key",
            &object_key,
            "--body",
            &body,
        ],
        &envs,
        &home_dir,
    );
    assert!(
        upload.status.success(),
        "object upload failed: {}",
        String::from_utf8_lossy(&upload.stderr)
    );

    let object_head = cli_live(
        &[
            "ve-tos",
            "object",
            "head",
            "--bucket",
            &bucket,
            "--key",
            &object_key,
        ],
        &envs,
        &home_dir,
    );
    assert!(object_head.status.success(), "object head failed");

    let download = cli_live(
        &[
            "ve-tos",
            "object",
            "download",
            "--bucket",
            &bucket,
            "--key",
            &object_key,
            "--body",
            download_path.to_str().expect("utf8 path"),
        ],
        &envs,
        &home_dir,
    );
    assert!(download.status.success(), "object download failed");
    let downloaded = std::fs::read_to_string(&download_path).expect("downloaded object");
    assert_eq!(downloaded, body);

    let multipart_create = cli_live(
        &[
            "ve-tos",
            "multipart",
            "create",
            "--bucket",
            &bucket,
            "--key",
            "multipart/smoke.bin",
        ],
        &envs,
        &home_dir,
    );
    assert!(
        multipart_create.status.success(),
        "multipart create failed: {}",
        String::from_utf8_lossy(&multipart_create.stderr)
    );
    let multipart_json = parse_json(&multipart_create);
    let upload_id = extract_upload_id(&multipart_json).expect("multipart upload id");

    let multipart_list = cli_live(
        &[
            "ve-tos",
            "multipart",
            "list",
            "--bucket",
            &bucket,
            "--fetch-from-kv",
        ],
        &envs,
        &home_dir,
    );
    assert!(
        multipart_list.status.success(),
        "multipart list failed: {}",
        String::from_utf8_lossy(&multipart_list.stderr)
    );

    let multipart_list_parts = cli_live(
        &[
            "ve-tos",
            "multipart",
            "list-parts",
            "--bucket",
            &bucket,
            "--key",
            "multipart/smoke.bin",
            "--upload-id",
            &upload_id,
            "--fetch-from-kv",
        ],
        &envs,
        &home_dir,
    );
    assert!(
        multipart_list_parts.status.success(),
        "multipart list-parts failed: {}",
        String::from_utf8_lossy(&multipart_list_parts.stderr)
    );

    let multipart_abort = cli_live(
        &[
            "ve-tos",
            "multipart",
            "abort",
            "--bucket",
            &bucket,
            "--key",
            "multipart/smoke.bin",
            "--upload-id",
            &upload_id,
            "--force",
        ],
        &envs,
        &home_dir,
    );
    assert!(multipart_abort.status.success(), "multipart abort failed");

    let delete_object = cli_live(
        &[
            "ve-tos",
            "object",
            "delete",
            "--bucket",
            &bucket,
            "--key",
            &object_key,
            "--force",
            "--confirm",
            &format!("tos://{bucket}/{object_key}"),
        ],
        &envs,
        &home_dir,
    );
    assert!(delete_object.status.success(), "object delete failed");

    let delete_bucket = cli_live(
        &[
            "ve-tos",
            "bucket",
            "delete",
            &bucket_uri,
            "--force",
            "--confirm",
            &bucket_uri,
        ],
        &envs,
        &home_dir,
    );
    if !delete_bucket.status.success() {
        let destroy_bucket = cli_live(
            &[
                "ve-tos",
                "bucket",
                "delete",
                &bucket_uri,
                "--destroy",
                "--confirm",
                &bucket_uri,
            ],
            &envs,
            &home_dir,
        );
        if !destroy_bucket.status.success() {
            eprintln!(
                "best-effort cleanup failed for bucket {bucket}: {}\n{}",
                String::from_utf8_lossy(&delete_bucket.stderr),
                String::from_utf8_lossy(&destroy_bucket.stderr),
            );
        }
    }

    let _ = std::fs::remove_file(download_path);
    let _ = std::fs::remove_dir_all(home_dir);
}
