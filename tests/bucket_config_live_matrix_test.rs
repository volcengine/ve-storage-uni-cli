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
struct Fixture {
    base_dir: String,
    home_dir: String,
    bucket: String,
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

fn fixture() -> Fixture {
    let suffix = unique_suffix();
    let base_dir = std::env::temp_dir().join(format!("tos-bucket-config-matrix-{suffix}"));
    std::fs::create_dir_all(&base_dir).expect("create temp dir");
    Fixture {
        base_dir: base_dir.to_string_lossy().into_owned(),
        home_dir: base_dir.join("home").to_string_lossy().into_owned(),
        bucket: format!("ve-tos-cli-bcfg-{}", &suffix[..16]),
    }
}

fn cli_live(args: &[String], env: &LiveEnv, home_dir: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ve-storage-uni-cli"))
        .env("HOME", home_dir)
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

fn flags(values: &[&'static str]) -> BTreeSet<&'static str> {
    values.iter().copied().collect()
}

fn expected_flags() -> BTreeMap<&'static str, BTreeSet<&'static str>> {
    BTreeMap::from([
        ("quota get", flags(&["--bucket"])),
        ("quota set", flags(&["--bucket", "--config"])),
        ("policy get", flags(&["--bucket"])),
        ("policy set", flags(&["--bucket", "--config"])),
        ("policy delete", flags(&["--bucket", "--force"])),
        ("lifecycle get", flags(&["--bucket"])),
        ("lifecycle set", flags(&["--bucket", "--config"])),
        ("lifecycle delete", flags(&["--bucket", "--force"])),
        ("storageclass set", flags(&["--bucket", "--storage-class"])),
        ("cors get", flags(&["--bucket"])),
        (
            "cors set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("cors delete", flags(&["--bucket", "--force"])),
        ("versioning get", flags(&["--bucket"])),
        ("versioning set", flags(&["--bucket", "--config"])),
        ("replication get", flags(&["--bucket", "--rule-id"])),
        ("replication set", flags(&["--bucket", "--config"])),
        ("replication delete", flags(&["--bucket", "--force"])),
        ("encryption get", flags(&["--bucket"])),
        (
            "encryption set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("encryption delete", flags(&["--bucket", "--force"])),
        (
            "custom-domain set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        (
            "custom-domain delete",
            flags(&["--bucket", "--domain", "--force"]),
        ),
        ("custom-domain list", flags(&["--bucket"])),
        (
            "custom-domain set-token",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("custom-domain get-token", flags(&["--bucket", "--domain"])),
        ("notification get", flags(&["--bucket"])),
        (
            "notification set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("website get", flags(&["--bucket"])),
        (
            "website set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("website delete", flags(&["--bucket", "--force"])),
        ("mirror get", flags(&["--bucket"])),
        (
            "mirror set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("mirror delete", flags(&["--bucket", "--force"])),
        ("inventory get", flags(&["--bucket", "--id"])),
        (
            "inventory set",
            flags(&["--bucket", "--id", "--config", "--content-md5"]),
        ),
        ("inventory delete", flags(&["--bucket", "--id", "--force"])),
        (
            "inventory list",
            flags(&["--bucket", "--continuation-token"]),
        ),
        ("tagging get", flags(&["--bucket"])),
        (
            "tagging set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("tagging delete", flags(&["--bucket", "--force"])),
        ("acl get", flags(&["--bucket"])),
        (
            "acl set",
            flags(&[
                "--bucket",
                "--config",
                "--acl",
                "--grant-full-control",
                "--grant-read",
                "--grant-read-non-list",
                "--grant-read-acp",
                "--grant-write",
                "--grant-write-acp",
            ]),
        ),
        ("rename get", flags(&["--bucket"])),
        ("rename set", flags(&["--bucket", "--config"])),
        ("rename delete", flags(&["--bucket", "--force"])),
        ("worm get", flags(&["--bucket"])),
        (
            "worm set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("real-time-log get", flags(&["--bucket"])),
        (
            "real-time-log set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("real-time-log delete", flags(&["--bucket", "--force"])),
        ("access-monitor get", flags(&["--bucket"])),
        (
            "access-monitor set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("trash get", flags(&["--bucket"])),
        (
            "trash set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("payment get", flags(&["--bucket"])),
        ("payment set", flags(&["--bucket", "--config"])),
        ("logging get", flags(&["--bucket"])),
        (
            "logging set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("intelligent-tiering get", flags(&["--bucket"])),
        (
            "intelligent-tiering set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("transfer-acceleration get", flags(&["--bucket"])),
        (
            "transfer-acceleration set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("cdn-notification get", flags(&["--bucket"])),
        (
            "cdn-notification set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("cdn-notification delete", flags(&["--bucket", "--force"])),
        ("https-config get", flags(&["--bucket"])),
        ("https-config set", flags(&["--bucket", "--config"])),
        ("pay-by-traffic get", flags(&["--bucket"])),
        (
            "pay-by-traffic set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("max-age get", flags(&["--bucket"])),
        (
            "max-age set",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        ("max-age delete", flags(&["--bucket", "--force"])),
        (
            "redundancy-transition create",
            flags(&["--bucket", "--config", "--content-md5"]),
        ),
        (
            "redundancy-transition delete",
            flags(&["--bucket", "--task-id", "--force"]),
        ),
        (
            "redundancy-transition get",
            flags(&["--bucket", "--task-id"]),
        ),
        (
            "redundancy-transition list",
            flags(&["--bucket", "--continuation-token"]),
        ),
        (
            "redundancy-transition get-remaining-time",
            flags(&["--bucket", "--task-id"]),
        ),
    ])
}
fn matrix(fixture: &Fixture) -> Vec<MatrixCase> {
    vec![
        MatrixCase {
            command: "quota get",
            args: tos_args(&["ve-tos", "quota", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "quota set",
            args: tos_args(&[
                "ve-tos",
                "quota",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "policy get",
            args: tos_args(&["ve-tos", "policy", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "policy set",
            args: tos_args(&[
                "ve-tos",
                "policy",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "policy delete",
            args: tos_args(&[
                "ve-tos",
                "policy",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "lifecycle get",
            args: tos_args(&["ve-tos", "lifecycle", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "lifecycle set",
            args: tos_args(&[
                "ve-tos",
                "lifecycle",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "lifecycle delete",
            args: tos_args(&[
                "ve-tos",
                "lifecycle",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "storageclass set",
            args: tos_args(&[
                "ve-tos",
                "storageclass",
                "set",
                "--bucket",
                &fixture.bucket,
                "--storage-class",
                "STANDARD",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "cors get",
            args: tos_args(&["ve-tos", "cors", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "cors set",
            args: tos_args(&[
                "ve-tos",
                "cors",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "cors delete",
            args: tos_args(&[
                "ve-tos",
                "cors",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "versioning get",
            args: tos_args(&["ve-tos", "versioning", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "versioning set",
            args: tos_args(&[
                "ve-tos",
                "versioning",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "replication get",
            args: tos_args(&[
                "ve-tos",
                "replication",
                "get",
                "--bucket",
                &fixture.bucket,
                "--rule-id",
                "rule-1",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "replication set",
            args: tos_args(&[
                "ve-tos",
                "replication",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "replication delete",
            args: tos_args(&[
                "ve-tos",
                "replication",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "encryption get",
            args: tos_args(&["ve-tos", "encryption", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "encryption set",
            args: tos_args(&[
                "ve-tos",
                "encryption",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "encryption delete",
            args: tos_args(&[
                "ve-tos",
                "encryption",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "custom-domain set",
            args: tos_args(&[
                "ve-tos",
                "custom-domain",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "custom-domain delete",
            args: tos_args(&[
                "ve-tos",
                "custom-domain",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--domain",
                "static.example.com",
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "custom-domain list",
            args: tos_args(&[
                "ve-tos",
                "custom-domain",
                "list",
                "--bucket",
                &fixture.bucket,
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "custom-domain set-token",
            args: tos_args(&[
                "ve-tos",
                "custom-domain",
                "set-token",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "custom-domain get-token",
            args: tos_args(&[
                "ve-tos",
                "custom-domain",
                "get-token",
                "--bucket",
                &fixture.bucket,
                "--domain",
                "static.example.com",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "notification get",
            args: tos_args(&["ve-tos", "notification", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "notification set",
            args: tos_args(&[
                "ve-tos",
                "notification",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "website get",
            args: tos_args(&["ve-tos", "website", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "website set",
            args: tos_args(&[
                "ve-tos",
                "website",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "website delete",
            args: tos_args(&[
                "ve-tos",
                "website",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "mirror get",
            args: tos_args(&["ve-tos", "mirror", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "mirror set",
            args: tos_args(&[
                "ve-tos",
                "mirror",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "mirror delete",
            args: tos_args(&[
                "ve-tos",
                "mirror",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "inventory get",
            args: tos_args(&[
                "ve-tos",
                "inventory",
                "get",
                "--bucket",
                &fixture.bucket,
                "--id",
                "daily-report",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "inventory set",
            args: tos_args(&[
                "ve-tos",
                "inventory",
                "set",
                "--bucket",
                &fixture.bucket,
                "--id",
                "daily-report",
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "inventory delete",
            args: tos_args(&[
                "ve-tos",
                "inventory",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--id",
                "daily-report",
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "inventory list",
            args: tos_args(&[
                "ve-tos",
                "inventory",
                "list",
                "--bucket",
                &fixture.bucket,
                "--continuation-token",
                "token-1",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "tagging get",
            args: tos_args(&["ve-tos", "tagging", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "tagging set",
            args: tos_args(&[
                "ve-tos",
                "tagging",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "tagging delete",
            args: tos_args(&[
                "ve-tos",
                "tagging",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "acl get",
            args: tos_args(&["ve-tos", "acl", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "acl set",
            args: tos_args(&[
                "ve-tos",
                "acl",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--acl",
                "public-read",
                "--grant-full-control",
                "id=owner",
                "--grant-read",
                "id=reader",
                "--grant-read-non-list",
                "id=reader",
                "--grant-read-acp",
                "id=reader",
                "--grant-write",
                "id=writer",
                "--grant-write-acp",
                "id=writer",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "rename get",
            args: tos_args(&["ve-tos", "rename", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "rename set",
            args: tos_args(&[
                "ve-tos",
                "rename",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "rename delete",
            args: tos_args(&[
                "ve-tos",
                "rename",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "worm get",
            args: tos_args(&["ve-tos", "worm", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "worm set",
            args: tos_args(&[
                "ve-tos",
                "worm",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "real-time-log get",
            args: tos_args(&[
                "ve-tos",
                "real-time-log",
                "get",
                "--bucket",
                &fixture.bucket,
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "real-time-log set",
            args: tos_args(&[
                "ve-tos",
                "real-time-log",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "real-time-log delete",
            args: tos_args(&[
                "ve-tos",
                "real-time-log",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "access-monitor get",
            args: tos_args(&[
                "ve-tos",
                "access-monitor",
                "get",
                "--bucket",
                &fixture.bucket,
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "access-monitor set",
            args: tos_args(&[
                "ve-tos",
                "access-monitor",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "trash get",
            args: tos_args(&["ve-tos", "trash", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "trash set",
            args: tos_args(&[
                "ve-tos",
                "trash",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "payment get",
            args: tos_args(&["ve-tos", "payment", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "payment set",
            args: tos_args(&[
                "ve-tos",
                "payment",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "logging get",
            args: tos_args(&["ve-tos", "logging", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "logging set",
            args: tos_args(&[
                "ve-tos",
                "logging",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "intelligent-tiering get",
            args: tos_args(&[
                "ve-tos",
                "intelligent-tiering",
                "get",
                "--bucket",
                &fixture.bucket,
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "intelligent-tiering set",
            args: tos_args(&[
                "ve-tos",
                "intelligent-tiering",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "transfer-acceleration get",
            args: tos_args(&[
                "ve-tos",
                "transfer-acceleration",
                "get",
                "--bucket",
                &fixture.bucket,
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "transfer-acceleration set",
            args: tos_args(&[
                "ve-tos",
                "transfer-acceleration",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "cdn-notification get",
            args: tos_args(&[
                "ve-tos",
                "cdn-notification",
                "get",
                "--bucket",
                &fixture.bucket,
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "cdn-notification set",
            args: tos_args(&[
                "ve-tos",
                "cdn-notification",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "cdn-notification delete",
            args: tos_args(&[
                "ve-tos",
                "cdn-notification",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "https-config get",
            args: tos_args(&["ve-tos", "https-config", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "https-config set",
            args: tos_args(&[
                "ve-tos",
                "https-config",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "pay-by-traffic get",
            args: tos_args(&[
                "ve-tos",
                "pay-by-traffic",
                "get",
                "--bucket",
                &fixture.bucket,
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "pay-by-traffic set",
            args: tos_args(&[
                "ve-tos",
                "pay-by-traffic",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "max-age get",
            args: tos_args(&["ve-tos", "max-age", "get", "--bucket", &fixture.bucket]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "max-age set",
            args: tos_args(&[
                "ve-tos",
                "max-age",
                "set",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "max-age delete",
            args: tos_args(&[
                "ve-tos",
                "max-age",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "redundancy-transition create",
            args: tos_args(&[
                "ve-tos",
                "redundancy-transition",
                "create",
                "--bucket",
                &fixture.bucket,
                "--config",
                "{}",
                "--content-md5",
                "dGVzdA==",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "redundancy-transition delete",
            args: tos_args(&[
                "ve-tos",
                "redundancy-transition",
                "delete",
                "--bucket",
                &fixture.bucket,
                "--task-id",
                "task-1",
                "--force",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "redundancy-transition get",
            args: tos_args(&[
                "ve-tos",
                "redundancy-transition",
                "get",
                "--bucket",
                &fixture.bucket,
                "--task-id",
                "task-1",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "redundancy-transition list",
            args: tos_args(&[
                "ve-tos",
                "redundancy-transition",
                "list",
                "--bucket",
                &fixture.bucket,
                "--continuation-token",
                "token-1",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
        MatrixCase {
            command: "redundancy-transition get-remaining-time",
            args: tos_args(&[
                "ve-tos",
                "redundancy-transition",
                "get-remaining-time",
                "--bucket",
                &fixture.bucket,
                "--task-id",
                "task-1",
            ]),
            policy: LivePolicy::RequiresCapability("bucket configuration live validation"),
        },
    ]
}
fn extract_flags(args: &[String]) -> BTreeSet<&str> {
    args.iter()
        .filter_map(|arg| arg.strip_prefix("--"))
        .map(|flag| {
            if flag == "output" {
                "--output"
            } else {
                Box::leak(format!("--{flag}").into_boxed_str())
            }
        })
        .collect()
}

#[test]
fn test_bucket_config_live_matrix_covers_all_parameters() {
    let fixture = fixture();
    let cases = matrix(&fixture);
    let expected = expected_flags();

    assert_eq!(cases.len(), expected.len());

    for case in &cases {
        let actual_flags = extract_flags(&case.args);
        let expected_flags = expected
            .get(case.command)
            .unwrap_or_else(|| panic!("missing expected flags for {}", case.command));
        assert_eq!(
            &actual_flags, expected_flags,
            "flag mismatch for {}",
            case.command
        );
    }
}

#[test]
#[ignore = "requires live TOS credentials and capability review for non-Ready bucket configuration cases"]
fn test_bucket_config_live_ready_matrix_with_teardown() {
    let env = match live_env() {
        Some(env) => env,
        None => {
            eprintln!("skipping live bucket config matrix: missing TOS env vars");
            return;
        }
    };

    let fixture = fixture();
    let create = cli_live(
        &tos_args(&["ve-tos", "bucket", "create", &fixture.bucket]),
        &env,
        &fixture.home_dir,
    );
    if !create.status.success() {
        panic!(
            "failed to create bucket: stdout={}, stderr={}",
            String::from_utf8_lossy(&create.stdout),
            String::from_utf8_lossy(&create.stderr)
        );
    }

    let ready_cases: Vec<MatrixCase> = matrix(&fixture)
        .into_iter()
        .filter(|case| matches!(case.policy, LivePolicy::Ready))
        .collect();
    let mut failure: Option<String> = None;

    for case in ready_cases {
        let output = cli_live(&case.args, &env, &fixture.home_dir);
        if !output.status.success() {
            failure = Some(format!(
                "ready case `{}` failed: stdout={}, stderr={}",
                case.command,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
            break;
        }
    }

    let delete = cli_live(
        &tos_args(&["ve-tos", "bucket", "delete", &fixture.bucket, "--force"]),
        &env,
        &fixture.home_dir,
    );
    if !delete.status.success() && failure.is_none() {
        failure = Some(format!(
            "failed to delete bucket in teardown: stdout={}, stderr={}",
            String::from_utf8_lossy(&delete.stdout),
            String::from_utf8_lossy(&delete.stderr)
        ));
    }

    let _ = std::fs::remove_dir_all(&fixture.base_dir);

    if let Some(message) = failure {
        assert!(false, "{message}");
    }
}
