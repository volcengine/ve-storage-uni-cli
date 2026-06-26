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

fn main() {
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
