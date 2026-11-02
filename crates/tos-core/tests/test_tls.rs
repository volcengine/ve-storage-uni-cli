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

//! TLS compatibility checks for the platform verifier and credential SDK transport.

use std::time::Duration;

#[test]
fn platform_and_sdk_clients_can_initialize_in_either_order() {
    // Both crypto providers are enabled by the dependency graph; neither client
    // may rely on rustls inferring a single process-wide default provider.
    let platform_client = reqwest::Client::builder().build().unwrap();
    let sdk_client = reqwest_sdk_compat::blocking::Client::builder()
        .build()
        .unwrap();
    drop((platform_client, sdk_client));
    let sdk_client = reqwest_sdk_compat::blocking::Client::builder()
        .build()
        .unwrap();
    let platform_client = reqwest::Client::builder().build().unwrap();
    drop((sdk_client, platform_client));
}

#[tokio::test]
#[ignore = "requires an explicitly selected HTTPS endpoint and network access"]
async fn platform_and_sdk_clients_accept_system_trusted_https() {
    let endpoint = std::env::var("VE_STORAGE_TLS_TEST_URL")
        .expect("set VE_STORAGE_TLS_TEST_URL to a trusted HTTPS endpoint");
    let url = reqwest::Url::parse(&endpoint).unwrap();
    assert_eq!(url.scheme(), "https");
    assert!(url.username().is_empty() && url.password().is_none());
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap();
    tokio::task::spawn_blocking(move || {
        reqwest_sdk_compat::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap()
            .get(endpoint)
            .send()
            .unwrap();
    })
    .await
    .unwrap();
}
