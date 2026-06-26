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

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(args)
        .output()
        .expect("Failed to execute ve-storage-uni-cli")
}

fn assert_no_cjk(stdout: &str) {
    assert!(
        !stdout
            .chars()
            .any(|ch| ('\u{4e00}'..='\u{9fff}').contains(&ch)),
        "help output should be English-only, got:\n{stdout}"
    );
}

#[test]
fn test_object_upload_help_is_self_descriptive_and_english_only() {
    let output = cli(&["ve-tos", "object", "upload", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Upload a single object"));
    assert!(stdout.contains("Object path"));
    assert!(stdout.contains("--bucket"));
    assert!(stdout.contains("--key"));
    assert!(stdout.contains("--body"));
    assert!(stdout.contains("--net-speed-test"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_object_download_help_exposes_conditional_headers() {
    let output = cli(&["ve-tos", "object", "download", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--if-modified-since"));
    assert!(stdout.contains("--if-unmodified-since"));
    assert!(stdout.contains("--replicated-from"));
    assert!(stdout.contains("--from-modular"));
}

#[test]
fn test_object_delete_help_exposes_conditional_delete_headers() {
    let output = cli(&["ve-tos", "object", "delete", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--if-match-expires"));
    assert!(stdout.contains("--if-match-create-time"));
    assert!(stdout.contains("--lifecycle-directly-delete-versions"));
    assert!(stdout.contains("--only-put-delete-marker"));
}

#[test]
fn test_object_batch_delete_help_exposes_recursive_and_md5() {
    let output = cli(&["ve-tos", "object", "batch-delete", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--recursive"));
    assert!(stdout.contains("--skip-trash"));
    assert!(stdout.contains("--content-md5"));
}

#[test]
fn test_multipart_upload_help_is_self_descriptive_and_english_only() {
    let output = cli(&["ve-tos", "multipart", "upload", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Upload a part"));
    assert!(stdout.contains("Upload ID"));
    assert!(stdout.contains("Part number"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_turbo_append_help_is_self_descriptive_and_english_only() {
    let output = cli(&["ve-tos", "turbo", "append", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Append data to a turbo object"));
    assert!(stdout.contains("Object path"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_bucket_delete_help_exposes_force_and_destroy() {
    let output = cli(&["ve-tos", "bucket", "delete", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--force"));
    assert!(stdout.contains("--destroy"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_object_delete_help_exposes_force() {
    let output = cli(&["ve-tos", "object", "delete", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--force"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_multipart_abort_help_exposes_force() {
    let output = cli(&["ve-tos", "multipart", "abort", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--force"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_bucket_create_help_exposes_region_override_in_english() {
    let output = cli(&["ve-tos", "bucket", "create", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Create a new bucket"));
    assert!(stdout.contains("--region"));
    assert!(stdout.contains("--bucket-type"));
    assert!(stdout.contains("--project-name"));
    assert!(stdout.contains("--bucket-object-lock-enabled"));
    assert!(stdout.contains("--acl"));
    assert!(stdout.contains("public-read-write"));
    assert!(stdout.contains("--grant-full-control"));
    assert!(stdout.contains("INTELLIGENT_TIERING"));
    assert!(stdout.contains("--az-redundancy"));
    assert!(stdout.contains("multi-az"));
    assert!(stdout.contains("--tagging"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_bucket_list_help_exposes_bucket_type_filter() {
    let output = cli(&["ve-tos", "bucket", "list", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--project-name"));
    assert!(stdout.contains("--bucket-type"));
}

#[test]
fn test_object_get_fetch_task_help_is_english_only() {
    let output = cli(&["ve-tos", "object", "get-fetch-task", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Get async fetch task status"));
    assert!(stdout.contains("--bucket"));
    assert!(stdout.contains("--task-id"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_multipart_complete_help_is_english_only() {
    let output = cli(&["ve-tos", "multipart", "complete", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Complete a multipart upload"));
    assert!(stdout.contains("--upload-id"));
    assert!(stdout.contains("--complete-all"));
    assert!(stdout.contains("--if-unmodified-since"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_turbo_open_help_is_english_only() {
    let output = cli(&["ve-tos", "turbo", "open", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Open a turbo channel for an object"));
    assert!(stdout.contains("--bucket"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_object_form_upload_help_exposes_meta() {
    let output = cli(&["ve-tos", "object", "form-upload", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--meta"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_object_set_acl_help_exposes_version_id() {
    let output = cli(&["ve-tos", "object", "set-acl", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--version-id"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_turbo_list_help_exposes_key() {
    let output = cli(&["ve-tos", "turbo", "list", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--key"));
    assert!(stdout.contains("--prefix"));
    assert_no_cjk(&stdout);
}

#[test]
fn test_object_link_help_exposes_acl_and_tagging_headers() {
    let output = cli(&["ve-tos", "object", "link", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--acl"));
    assert!(stdout.contains("--grant-read-non-list"));
    assert!(stdout.contains("--tagging"));
}

#[test]
fn test_object_copy_help_exposes_extended_headers() {
    let output = cli(&["ve-tos", "object", "copy", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--unique-tag"));
    assert!(stdout.contains("--copy-source-last-modified"));
    assert!(stdout.contains("--traffic-limit"));
    assert!(stdout.contains("--object-lock-mode"));
    assert!(stdout.contains("--persistent-headers"));
    assert!(stdout.contains("--tagging"));
    assert!(stdout.contains("--grant-write-acp"));
}

#[test]
fn test_object_append_help_exposes_conditional_and_acl_headers() {
    let output = cli(&["ve-tos", "object", "append", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--append-last-time"));
    assert!(stdout.contains("--content-md5"));
    assert!(stdout.contains("--object-lock-mode"));
    assert!(stdout.contains("--grant-write-acp"));
    assert!(stdout.contains("--traffic-limit"));
}

#[test]
fn test_object_rename_help_exposes_forbid_overwrite_headers() {
    let output = cli(&["ve-tos", "object", "rename", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--recursive-mkdir"));
    assert!(stdout.contains("--not-update-timestamp"));
    assert!(stdout.contains("--forbid-overwrite"));
    assert!(stdout.contains("--trace-id"));
}

#[test]
fn test_object_create_symlink_help_exposes_header_driven_fields() {
    let output = cli(&["ve-tos", "object", "create-symlink", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--target-key"));
    assert!(stdout.contains("--target-bucket"));
    assert!(stdout.contains("--grant-write-acp"));
    assert!(stdout.contains("--tagging"));
}

#[test]
fn test_object_create_fetch_task_help_exposes_acl_headers() {
    let output = cli(&["ve-tos", "object", "create-fetch-task", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--acl"));
    assert!(stdout.contains("--grant-full-control"));
    assert!(stdout.contains("--grant-read-non-list"));
}

#[test]
fn test_multipart_list_help_exposes_pagination_fields() {
    let output = cli(&["ve-tos", "multipart", "list", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--delimiter"));
    assert!(stdout.contains("--key-marker"));
    assert!(stdout.contains("--upload-id-marker"));
    assert!(stdout.contains("--max-uploads"));
    assert!(stdout.contains("--encoding-type"));
}

#[test]
fn test_multipart_list_parts_help_exposes_pagination_fields() {
    let output = cli(&["ve-tos", "multipart", "list-parts", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--part-number-marker"));
    assert!(stdout.contains("--max-parts"));
    assert!(stdout.contains("--fetch-from-kv"));
}

#[test]
fn test_multipart_list_help_exposes_fetch_from_kv() {
    let output = cli(&["ve-tos", "multipart", "list", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--fetch-from-kv"));
}

#[test]
fn test_turbo_open_help_exposes_acl_headers() {
    let output = cli(&["ve-tos", "turbo", "open", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--grant-read-non-list"));
    assert!(stdout.contains("--grant-write-acp"));
}
