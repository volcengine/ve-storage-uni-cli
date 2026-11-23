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
#[path = "src/ask/application_values.rs"]
mod application_values;

const BUILD_ENV_NAMES: [&str; 2] = [
    "STORAGE_CLI_BUILD_DOUBAO_APP_ID",
    "STORAGE_CLI_BUILD_DOUBAO_APP_SECRET",
];

const BUNDLE_ERROR: &str = "invalid bundled Doubao application configuration; provide both STORAGE_CLI_BUILD_DOUBAO_APP_ID and STORAGE_CLI_BUILD_DOUBAO_APP_SECRET inputs with valid values, or leave both absent";

fn application_value(name: &str) -> Result<Option<String>, ()> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(_) => Err(()),
    }
}

fn validate_application_bundle() -> Result<bool, ()> {
    for name in BUILD_ENV_NAMES {
        println!("cargo:rerun-if-env-changed={name}");
    }
    println!("cargo:rerun-if-changed=src/ask/application_values.rs");
    // [Review Fix #14] Never format VarError or field values: Unicode failures may own secrets.
    let app_id = application_value(BUILD_ENV_NAMES[0])?;
    let secret = application_value(BUILD_ENV_NAMES[1])?;
    application_values::parse_bundle(app_id.as_deref(), secret.as_deref())
        .map(|bundle| bundle.is_some())
        .map_err(|_| ())
}

fn main() {
    match validate_application_bundle() {
        Ok(true) => println!("cargo:warning=Doubao ask application: bundled (build inputs received)"),
        Ok(false) => println!("cargo:warning=Doubao ask application: not bundled (both build environment variables absent; export them before invoking Cargo)"),
        Err(()) => {
            eprintln!("{BUNDLE_ERROR}");
            std::process::exit(1);
        }
    }
    // Capture the rustc version at build time and expose it to the crate via
    // `option_env!("RUSTC_VERSION")`. Used to build the HTTP User-Agent string.
    println!("cargo:rerun-if-changed=build.rs");
    let version = Command::new(std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()))
        .arg("--version")
        .output()
        .ok()
        .and_then(|out| {
            if out.status.success() {
                String::from_utf8(out.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "rustc unknown".into());

    // The full output looks like "rustc 1.78.0 (9b00956e5 2024-04-29)"; we keep
    // it as-is for transparency, callers may strip if they want a shorter form.
    println!("cargo:rustc-env=RUSTC_VERSION={}", version);
}
