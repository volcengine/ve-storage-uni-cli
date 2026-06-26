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

use std::process::{Command, Output};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .args(args)
        .output()
        .expect("failed to execute ve-storage-uni-cli")
}

fn stdout_json(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw: serde_json::Value = serde_json::from_str(&stdout).unwrap_or_else(|err| {
        panic!("stdout is not json: {err}; stdout={stdout}");
    });
    // [Review Fix #1] 自动解包 Envelope，使既有 Agent-faced 断言保持稳定。
    unwrap_envelope(raw)
}

fn stdout_envelope(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|err| {
        panic!("stdout is not json: {err}; stdout={stdout}");
    })
}

fn unwrap_envelope(v: serde_json::Value) -> serde_json::Value {
    if let serde_json::Value::Object(ref map) = v {
        if map
            .get("status")
            .and_then(serde_json::Value::as_str)
            .is_some()
            && map.contains_key("command")
            && map.contains_key("data")
        {
            return map.get("data").cloned().unwrap_or(serde_json::Value::Null);
        }
    }
    v
}

fn unique_temp_dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "ve-storage-uni-cli-{}-{}",
        name,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("create temp dir");
    path
}

fn csv_part_path(base_path: &Path) -> PathBuf {
    base_path.to_path_buf()
}

#[test]
fn test_high_level_describe_exposes_agent_metadata() {
    let output = cli(&[
        "--output",
        "json",
        "--describe",
        "ve-tos",
        "cp",
        "./local",
        "tos://bucket/key",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    assert_eq!(json["command"], "ve-tos cp");
    assert_eq!(json["layer"], "high_level");
    assert_eq!(json["supports_dry_run"], true);
    assert!(json["scenario_routing"]["progress"].is_string());
    assert!(json["parameters"].to_string().contains("checkpoint"));
    assert!(json["parameters"].to_string().contains("manifest-path"));
    assert!(json["parameters"].to_string().contains("no-manifest"));
    assert!(json["parameters"]
        .to_string()
        .contains("report-failures-only"));
}

#[test]
fn test_high_level_describe_does_not_require_positionals() {
    let output = cli(&["--output", "json", "ve-tos", "cp", "--describe"]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = stdout_envelope(&output);
    assert_eq!(envelope["command"], "ve-tos cp");
    assert_eq!(envelope["data"]["command"], "ve-tos cp");
    assert!(envelope["data"]["parameters"]
        .to_string()
        .contains("source"));
}

#[test]
fn test_tos_mb_describe_exposes_bucket_type() {
    let output = cli(&["--output", "json", "ve-tos", "mb", "--describe"]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    assert_eq!(json["command"], "ve-tos mb");
    let parameters = json["parameters"].to_string();
    assert!(parameters.contains("region"));
    assert!(parameters.contains("bucket-type"));
    assert!(parameters.contains("bucket-object-lock-enabled"));
    assert!(parameters.contains("fns"));
    assert!(parameters.contains("hns"));
}

#[test]
fn test_high_level_ls_uses_bounded_non_recursive_pagination_contract() {
    let tos_describe = cli(&[
        "--output",
        "json",
        "--describe",
        "ve-tos",
        "ls",
        "tos://bucket/prefix",
    ]);
    assert!(
        tos_describe.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tos_describe.stderr)
    );
    let tos_json = stdout_json(&tos_describe);
    let tos_params = tos_json["parameters"]
        .as_array()
        .expect("ve-tos ls parameters")
        .iter()
        .filter_map(|param| param["name"].as_str())
        .collect::<Vec<_>>();
    assert!(tos_params.contains(&"max-keys"));
    assert!(tos_params.contains(&"continuation-token"));
    assert!(tos_params.contains(&"columns"));
    assert!(tos_params.contains(&"manifest-path"));
    assert!(!tos_params.contains(&"no-manifest"));
    assert!(!tos_params.contains(&"report-failures-only"));
    assert!(!tos_params.contains(&"recursive"));
    let tos_max_keys = tos_json["parameters"]
        .as_array()
        .expect("ve-tos ls parameters")
        .iter()
        .find(|param| param["name"] == "max-keys")
        .expect("ve-tos ls max-keys parameter");
    assert!(tos_max_keys["description"]
        .as_str()
        .unwrap_or_default()
        .contains("buckets"));

    let tos_dry_run = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-tos",
        "ls",
        "tos://bucket/prefix",
        "--max-keys",
        "100",
        "--continuation-token",
        "token-1",
    ]);
    assert!(
        tos_dry_run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tos_dry_run.stderr)
    );
    let tos_plan = stdout_json(&tos_dry_run);
    assert_eq!(tos_plan["command"], "ve-tos ls");
    assert_eq!(tos_plan["batch"]["enabled"], false);

    let tos_bucket_dry_run = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-tos",
        "ls",
        "--max-keys",
        "2",
    ]);
    assert!(
        tos_bucket_dry_run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tos_bucket_dry_run.stderr)
    );
    let tos_bucket_plan = stdout_json(&tos_bucket_dry_run);
    assert_eq!(tos_bucket_plan["command"], "ve-tos ls");
    assert_eq!(tos_bucket_plan["target"], "all buckets");

    let tos_zero_max_keys = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-tos",
        "ls",
        "--max-keys",
        "0",
    ]);
    assert!(!tos_zero_max_keys.status.success());
    let stderr = String::from_utf8_lossy(&tos_zero_max_keys.stderr);
    assert!(stderr.contains("--max-keys must be greater than 0"));

    let adrive_describe = cli(&[
        "--output",
        "json",
        "--describe",
        "ve-adrive",
        "ls",
        "adrive://inst/space/prefix",
    ]);
    assert!(
        adrive_describe.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&adrive_describe.stderr)
    );
    let adrive_json = stdout_json(&adrive_describe);
    let adrive_params = adrive_json["parameters"]
        .as_array()
        .expect("ve-adrive ls parameters")
        .iter()
        .filter_map(|param| param["name"].as_str())
        .collect::<Vec<_>>();
    assert!(adrive_params.contains(&"by-name"));
    assert!(adrive_params.contains(&"max-keys"));
    assert!(adrive_params.contains(&"marker"));
    assert!(adrive_params.contains(&"manifest-path"));
    assert!(!adrive_params.contains(&"no-manifest"));
    assert!(!adrive_params.contains(&"report-failures-only"));
    assert!(!adrive_params.contains(&"recursive"));

    let adrive_instance_dry_run = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-adrive",
        "ls",
        "--max-keys",
        "2",
    ]);
    assert!(
        adrive_instance_dry_run.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&adrive_instance_dry_run.stderr)
    );
    let adrive_instance_plan = stdout_json(&adrive_instance_dry_run);
    assert_eq!(adrive_instance_plan["command"], "ve-adrive ls");

    let adrive_zero_max_keys = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-adrive",
        "ls",
        "--max-keys",
        "0",
    ]);
    assert!(!adrive_zero_max_keys.status.success());
    let stderr = String::from_utf8_lossy(&adrive_zero_max_keys.stderr);
    assert!(stderr.contains("--max-keys must be greater than 0"));
}

#[test]
fn test_high_level_rb_is_bucket_delete_only() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "rb",
        "tos://example-bucket",
        "--describe",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    let params = json["parameters"]
        .as_array()
        .expect("parameters")
        .iter()
        .filter_map(|parameter| parameter["name"].as_str())
        .collect::<Vec<_>>();

    assert!(params.contains(&"force"));
    assert!(!params.contains(&"all-versions"));
    assert!(!params.contains(&"batch-concurrency"));
    assert!(!params.contains(&"manifest-path"));
    assert!(!params.contains(&"no-manifest"));
    assert!(!params.contains(&"report-path"));
    assert!(!params.contains(&"report-failures-only"));
}

#[test]
fn test_high_level_du_exposes_streaming_profile_parameters() {
    let tos_describe = cli(&[
        "--output",
        "json",
        "--describe",
        "ve-tos",
        "du",
        "tos://bucket/prefix",
    ]);
    assert!(
        tos_describe.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&tos_describe.stderr)
    );
    let tos_json = stdout_json(&tos_describe);
    let tos_params = tos_json["parameters"]
        .as_array()
        .expect("ve-tos du parameters")
        .iter()
        .filter_map(|param| param["name"].as_str())
        .collect::<Vec<_>>();
    assert!(tos_params.contains(&"top-k"));
    assert!(tos_params.contains(&"cost"));
    assert!(tos_params.contains(&"storage-price"));
    assert!(tos_params.contains(&"manifest-path"));
    assert!(!tos_params.contains(&"no-manifest"));
    assert!(!tos_params.contains(&"report-failures-only"));
    assert!(!tos_params.contains(&"max-keys"));
    assert!(!tos_params.contains(&"continuation-token"));

    let adrive_describe = cli(&[
        "--output",
        "json",
        "--describe",
        "ve-adrive",
        "du",
        "adrive://inst/space/prefix",
    ]);
    assert!(
        adrive_describe.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&adrive_describe.stderr)
    );
    let adrive_json = stdout_json(&adrive_describe);
    let adrive_params = adrive_json["parameters"]
        .as_array()
        .expect("ve-adrive du parameters")
        .iter()
        .filter_map(|param| param["name"].as_str())
        .collect::<Vec<_>>();
    assert!(adrive_params.contains(&"top-k"));
    assert!(adrive_params.contains(&"cost"));
    assert!(adrive_params.contains(&"storage-price"));
    assert!(adrive_params.contains(&"manifest-path"));
    assert!(!adrive_params.contains(&"no-manifest"));
    assert!(!adrive_params.contains(&"report-failures-only"));
}

#[test]
fn test_high_level_dry_run_outputs_controlled_plan() {
    let output = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-tos",
        "cp",
        "./local",
        "tos://bucket/key",
        "--recursive",
        "--checkpoint",
        "--checkpoint-dir",
        "/tmp/tos-checkpoints",
        "--report-path",
        "/tmp/tos-cp-report.csv",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    assert_eq!(json["command"], "ve-tos cp");
    assert_eq!(json["dry_run"], true);
    assert_eq!(json["batch"]["enabled"], true);
    assert_eq!(json["checkpoint"]["enabled"], true);
    assert_eq!(json["checkpoint"]["directory"], "/tmp/tos-checkpoints");
    assert_eq!(json["report"]["path"], "/tmp/tos-cp-report.csv");
    assert_eq!(json["list_echo"]["enabled"], false);
    assert_eq!(json["list_echo"]["disabled_reason"], "non_tty");
    assert_eq!(json["progress"]["enabled"], false);
    assert_eq!(json["progress"]["disabled_reason"], "non_tty");
    assert!(json["consistency_guards"].to_string().contains("CRC64"));
}

#[test]
fn test_high_level_output_controls_can_force_list_echo_and_progress() {
    let output = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-tos",
        "cp",
        "./local",
        "tos://bucket/key",
        "--recursive",
        "--list-echo",
        "--progress",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    assert_eq!(json["list_echo"]["enabled"], true);
    assert_eq!(json["progress"]["enabled"], true);
}

#[test]
fn test_high_level_output_controls_are_independent() {
    let output = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-tos",
        "restore",
        "tos://bucket/prefix",
        "--recursive",
        "--force",
        "--list-echo",
        "--no-progress",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    assert_eq!(json["list_echo"]["enabled"], true);
    assert_eq!(json["progress"]["enabled"], false);
    assert_eq!(json["progress"]["disabled_reason"], "no_progress");
}

#[test]
fn test_high_level_no_list_echo_does_not_disable_progress() {
    let output = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-tos",
        "cp",
        "./local",
        "tos://bucket/key",
        "--recursive",
        "--no-list-echo",
        "--progress",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    assert_eq!(json["list_echo"]["enabled"], false);
    assert_eq!(json["list_echo"]["disabled_reason"], "no_list_echo");
    assert_eq!(json["progress"]["enabled"], true);
}

#[test]
fn test_adrive_high_level_output_controls_match_tos() {
    let output = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-adrive",
        "cp",
        "./local",
        "adrive://inst/space/path",
        "--recursive",
        "--list-echo",
        "--progress",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    assert_eq!(json["command"], "ve-adrive cp");
    assert_eq!(json["list_echo"]["enabled"], true);
    assert_eq!(json["progress"]["enabled"], true);
}

#[test]
fn test_high_level_rm_describe_exposes_recursive_delete_mode() {
    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "rm",
        "tos://bucket/prefix/",
        "--describe",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope = stdout_envelope(&output);
    let data = &envelope["data"];
    assert!(data["parameters"]
        .to_string()
        .contains("recursive-delete-mode"));
    assert!(data["low_level_apis"].to_string().contains("HeadBucket"));
    assert!(data.to_string().contains("bottom-up"));
}

#[test]
fn test_high_level_no_progress_disables_progress_plan() {
    let output = cli(&[
        "--output",
        "json",
        "--dry-run",
        "ve-tos",
        "restore",
        "tos://bucket/prefix",
        "--recursive",
        "--force",
        "--no-progress",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json = stdout_json(&output);
    assert_eq!(json["command"], "ve-tos restore");
    assert_eq!(json["batch"]["enabled"], true);
    assert_eq!(json["progress"]["enabled"], false);
    assert_eq!(json["progress"]["disabled_reason"], "no_progress");
}

#[test]
fn test_high_level_destructive_commands_allow_dry_run_without_force() {
    for args in [
        &[
            "--output",
            "json",
            "--dry-run",
            "ve-tos",
            "mv",
            "tos://bucket/a",
            "tos://bucket/b",
        ][..],
        &[
            "--output",
            "json",
            "--dry-run",
            "ve-tos",
            "rm",
            "tos://bucket/key",
        ][..],
        &[
            "--output",
            "json",
            "--dry-run",
            "ve-tos",
            "sync",
            "./local",
            "tos://bucket/prefix",
            "--delete",
        ][..],
    ] {
        let output = cli(args);
        assert!(
            output.status.success(),
            "args={args:?}, stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"dry_run\""), "stdout={stdout}");
    }
}

#[test]
fn test_high_level_rb_requires_force_before_network() {
    let output = cli(&["--output", "json", "ve-tos", "rb", "tos://example-bucket"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // [Review Fix #ErrorEnvelopeContract] Validation failures are reported via
    // the stable error envelope fields rather than the Rust enum variant name.
    assert!(
        stderr.contains("\"ec\": \"InvalidParam\""),
        "stderr={stderr}"
    );
    assert!(stderr.contains("requires --force"), "stderr={stderr}");
    assert!(!stderr.contains("HTTP error"), "stderr={stderr}");
}

#[test]
fn test_high_level_rb_confirm_requires_tos_uri() {
    let output = cli(&[
        "--output",
        "json",
        "--confirm",
        "example-bucket",
        "ve-tos",
        "rb",
        "tos://example-bucket",
        "--force",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("does not match"), "stderr={stderr}");
    assert!(stderr.contains("tos://example-bucket"), "stderr={stderr}");
    assert!(!stderr.contains("HTTP error"), "stderr={stderr}");
}

#[test]
fn test_high_level_mv_confirm_requires_source_tos_uri() {
    let output = cli(&[
        "--output",
        "json",
        "--confirm",
        "tos://example-bucket/dst.txt",
        "ve-tos",
        "mv",
        "tos://example-bucket/src.txt",
        "tos://example-bucket/dst.txt",
        "--force",
    ]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("does not match"), "stderr={stderr}");
    assert!(
        stderr.contains("tos://example-bucket/src.txt"),
        "stderr={stderr}"
    );
    assert!(!stderr.contains("HTTP error"), "stderr={stderr}");
}

#[test]
fn test_high_level_invalid_tos_uri_returns_deterministic_error() {
    let output = cli(&["--output", "json", "--dry-run", "ve-tos", "cat", "./local"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("\"ec\": \"InvalidParam\""),
        "stderr={stderr}"
    );
    assert!(stderr.contains("invalid TOS URI"), "stderr={stderr}");
}

#[test]
fn test_high_level_cp_local_to_local_executes_without_tos_credentials() {
    let temp_dir = unique_temp_dir("cp-local");
    let source = temp_dir.join("source.txt");
    let destination = temp_dir.join("destination.txt");
    fs::write(&source, "hello high level cp").expect("write source");

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "cp",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&destination).expect("read destination"),
        "hello high level cp"
    );
    let json = stdout_envelope(&output);
    assert_eq!(json["status"], "success");
    assert_eq!(json["command"], "ve-tos cp local");
}

#[test]
fn test_high_level_mv_local_to_local_requires_force_then_deletes_source() {
    let temp_dir = unique_temp_dir("mv-local");
    let source = temp_dir.join("source.txt");
    let destination = temp_dir.join("destination.txt");
    fs::write(&source, "hello high level mv").expect("write source");

    let missing_force = cli(&[
        "--output",
        "json",
        "ve-tos",
        "mv",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
    ]);
    assert!(!missing_force.status.success());
    assert!(String::from_utf8_lossy(&missing_force.stderr).contains("requires --force"));

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "mv",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
        "--force",
        "--confirm",
        source.to_str().expect("source path"),
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(&destination).expect("read destination"),
        "hello high level mv"
    );
}

#[test]
fn test_high_level_cp_local_destination_overwrites_by_default() {
    let temp_dir = unique_temp_dir("cp-overwrite");
    let source = temp_dir.join("source.txt");
    let destination = temp_dir.join("destination.txt");
    fs::write(&source, "source").expect("write source");
    fs::write(&destination, "destination").expect("write destination");

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "cp",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(&destination).expect("read destination"),
        "source"
    );
    let json = stdout_envelope(&output);
    assert_eq!(json["status"], "success");
    assert_eq!(json["command"], "ve-tos cp local");
}

#[test]
fn test_high_level_cp_recursive_local_to_local_executes() {
    let temp_dir = unique_temp_dir("cp-recursive-local");
    let source = temp_dir.join("source");
    let nested = source.join("a/b");
    let destination = temp_dir.join("destination");
    fs::create_dir_all(&nested).expect("create nested source");
    fs::write(nested.join("file.txt"), "recursive copy").expect("write nested file");

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "cp",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
        "--recursive",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination.join("a/b/file.txt")).expect("read copied file"),
        "recursive copy"
    );
    let json = stdout_json(&output);
    assert_eq!(json["status"], "succeeded");
    assert_eq!(json["succeeded"], 1);
    assert_eq!(json["failed"], 0);
}

#[test]
fn test_high_level_cp_recursive_local_partial_failure_outputs_single_summary() {
    let temp_dir = unique_temp_dir("cp-recursive-local-failure");
    let source = temp_dir.join("source");
    let nested = source.join("nested");
    let destination = temp_dir.join("destination");
    let report_path = temp_dir.join("cp-report.csv");
    fs::create_dir_all(&nested).expect("create nested source");
    fs::create_dir_all(&destination).expect("create destination");
    fs::write(nested.join("file.txt"), "copy fails").expect("write source file");
    fs::write(destination.join("nested"), "blocks destination directory")
        .expect("write blocking file");

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "cp",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
        "--recursive",
        "--report-path",
        report_path.to_str().expect("report path"),
    ]);

    assert!(!output.status.success());
    let json = stdout_json(&output);
    assert_eq!(json["status"], "partial_failure");
    assert_eq!(json["failed"], 1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("\"status\": \"failed\""),
        "stderr should not include a second error envelope: {stderr}"
    );
    assert!(
        !stderr.contains("\"kind\": \"transfer_failed\""),
        "stderr should not include a second transfer_failed envelope: {stderr}"
    );
    let report_body = fs::read_to_string(csv_part_path(&report_path)).expect("read csv report");
    assert!(report_body.contains(",copy,"));
    assert!(report_body.contains(",failed,"));
}

#[test]
fn test_high_level_mv_recursive_local_to_local_deletes_source() {
    let temp_dir = unique_temp_dir("mv-recursive-local");
    let source = temp_dir.join("source");
    let nested = source.join("nested");
    let destination = temp_dir.join("destination");
    fs::create_dir_all(&nested).expect("create nested source");
    fs::write(nested.join("file.txt"), "recursive move").expect("write nested file");

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "mv",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
        "--recursive",
        "--force",
        "--confirm",
        source.to_str().expect("source path"),
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!source.exists());
    assert_eq!(
        fs::read_to_string(destination.join("nested/file.txt")).expect("read moved file"),
        "recursive move"
    );
    let json = stdout_json(&output);
    assert_eq!(json["status"], "succeeded");
    assert_eq!(json["failed"], 0);
}

#[test]
fn test_high_level_mv_recursive_writes_two_stage_manifest_and_report() {
    let temp_dir = unique_temp_dir("mv-recursive-report");
    let source = temp_dir.join("source");
    let destination = temp_dir.join("destination");
    let report_path = temp_dir.join("mv-report.csv");
    let manifest_path = temp_dir.join("mv-manifest.csv");
    fs::create_dir_all(&source).expect("create source");
    fs::write(source.join("file.txt"), "recursive move report").expect("write source file");

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "mv",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
        "--recursive",
        "--force",
        "--confirm",
        source.to_str().expect("source path"),
        "--report-path",
        report_path.to_str().expect("report path"),
        "--manifest-path",
        manifest_path.to_str().expect("manifest path"),
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest_body =
        fs::read_to_string(csv_part_path(&manifest_path)).expect("read csv manifest");
    assert!(manifest_body
        .lines()
        .next()
        .unwrap_or_default()
        .contains("operation"));
    assert_eq!(manifest_body.matches("\n").count(), 3);
    assert!(manifest_body.contains(",copy,"));
    assert!(manifest_body.contains(",delete-source,"));

    let report_body = fs::read_to_string(csv_part_path(&report_path)).expect("read csv report");
    assert!(report_body
        .lines()
        .next()
        .unwrap_or_default()
        .contains("status"));
    assert!(report_body.contains(",copy,"));
    assert!(report_body.contains(",delete-source,"));
    assert!(report_body.contains(",succeeded,"));
    assert!(!report_body
        .lines()
        .next()
        .unwrap_or_default()
        .contains("total"));
}

#[test]
fn test_high_level_sync_local_to_local_delete_removes_extras() {
    let temp_dir = unique_temp_dir("sync-local");
    let source = temp_dir.join("source");
    let destination = temp_dir.join("destination");
    fs::create_dir_all(&source).expect("create source");
    fs::create_dir_all(&destination).expect("create destination");
    fs::write(source.join("keep.txt"), "keep").expect("write source file");
    fs::write(destination.join("extra.txt"), "extra").expect("write extra file");

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "sync",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
        "--delete",
        "--force",
        "--confirm",
        destination.to_str().expect("destination path"),
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination.join("keep.txt")).expect("read synced file"),
        "keep"
    );
    assert!(!destination.join("extra.txt").exists());
}

#[test]
fn test_high_level_mv_recursive_respects_exclude_on_source_delete() {
    let temp_dir = unique_temp_dir("mv-recursive-exclude");
    let source = temp_dir.join("source");
    let destination = temp_dir.join("destination");
    fs::create_dir_all(&source).expect("create source");
    fs::write(source.join("move.txt"), "move").expect("write moved file");
    fs::write(source.join("keep.log"), "keep").expect("write excluded file");

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "mv",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
        "--recursive",
        "--force",
        "--confirm",
        source.to_str().expect("source path"),
        "--exclude",
        ".log",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(destination.join("move.txt")).expect("read moved file"),
        "move"
    );
    assert_eq!(
        fs::read_to_string(source.join("keep.log")).expect("read excluded file"),
        "keep"
    );
    assert!(!source.join("move.txt").exists());
}

#[test]
fn test_high_level_sync_delete_respects_exclude_filter() {
    let temp_dir = unique_temp_dir("sync-exclude-delete");
    let source = temp_dir.join("source");
    let destination = temp_dir.join("destination");
    fs::create_dir_all(&source).expect("create source");
    fs::create_dir_all(&destination).expect("create destination");
    fs::write(source.join("keep.txt"), "keep").expect("write source file");
    fs::write(destination.join("extra.tmp"), "extra").expect("write managed extra file");
    fs::write(destination.join("preserve.log"), "preserve").expect("write excluded extra file");

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "sync",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
        "--delete",
        "--force",
        "--confirm",
        destination.to_str().expect("destination path"),
        "--exclude",
        ".log",
    ]);

    assert!(
        output.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!destination.join("extra.tmp").exists());
    assert_eq!(
        fs::read_to_string(destination.join("preserve.log")).expect("read excluded extra file"),
        "preserve"
    );
}

#[test]
fn test_high_level_sync_delete_skips_extras_after_copy_failure() {
    let temp_dir = unique_temp_dir("sync-delete-copy-failure");
    let source = temp_dir.join("source");
    let destination = temp_dir.join("destination");
    let report_path = temp_dir.join("sync-report.csv");
    fs::create_dir_all(source.join("nested")).expect("create source");
    fs::create_dir_all(&destination).expect("create destination");
    fs::write(source.join("nested/file.txt"), "copy fails").expect("write source file");
    fs::write(destination.join("nested"), "blocks destination directory")
        .expect("write blocking file");
    fs::write(destination.join("extra.txt"), "must survive").expect("write extra file");

    let output = cli(&[
        "--output",
        "json",
        "ve-tos",
        "sync",
        source.to_str().expect("source path"),
        destination.to_str().expect("destination path"),
        "--delete",
        "--force",
        "--confirm",
        destination.to_str().expect("destination path"),
        "--report-path",
        report_path.to_str().expect("report path"),
    ]);

    assert!(!output.status.success());
    let json = stdout_json(&output);
    assert_eq!(json["status"], "partial_failure");
    assert_eq!(json["failed"], 1);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("\"status\": \"failed\""),
        "stderr should not include a second error envelope: {stderr}"
    );
    assert!(
        !stderr.contains("\"kind\": \"transfer_failed\""),
        "stderr should not include a second transfer_failed envelope: {stderr}"
    );
    assert_eq!(
        fs::read_to_string(destination.join("extra.txt")).expect("read protected extra"),
        "must survive"
    );
    let report_body = fs::read_to_string(csv_part_path(&report_path)).expect("read csv report");
    assert!(report_body.contains(",sync-copy,"));
    assert!(report_body.contains(",failed,"));
    assert!(report_body.contains(",delete-extra,"));
    assert!(report_body.contains(",skipped,"));
}
