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

//! Integration tests for RetryConfig.
//! [Review Fix #3] Align retry tests with current RetryConfig behavior.

use std::time::Duration;
use tos_core::infra::retry::RetryConfig;

#[test]
fn test_retry_config_default_values() {
    let config = RetryConfig::default();
    assert_eq!(config.max_retries, 3);
    assert_eq!(config.base_delay, Duration::from_millis(500));
    assert_eq!(config.max_delay, Duration::from_secs(30));
}

#[test]
fn test_delay_for_attempt_grows_with_attempt_before_cap() {
    let config = RetryConfig {
        max_retries: 3,
        base_delay: Duration::from_millis(500),
        max_delay: Duration::from_secs(30),
    };
    let first = config.delay_for_attempt(0);
    let second = config.delay_for_attempt(1);
    assert!(first >= Duration::from_millis(500));
    assert!(second >= Duration::from_millis(1000));
    assert!(second > first);
}

#[test]
fn test_delay_for_attempt_is_capped_at_max_delay() {
    let config = RetryConfig {
        max_retries: 10,
        base_delay: Duration::from_secs(5),
        max_delay: Duration::from_secs(6),
    };
    assert!(config.delay_for_attempt(10) <= config.max_delay);
}
