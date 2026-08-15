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

use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::StatusCode;
use std::time::{Duration, SystemTime};

/// Maximum delay accepted from a storage service's `Retry-After` header.
pub const MAX_RETRY_AFTER_DELAY: Duration = Duration::from_secs(600);

/// Return whether a TOS or ADrive response status is safe to retry.
///
/// The covered services guarantee that 429 and every 5xx response means the
/// operation was not accepted or applied. HTTP 408 is an ambiguous timeout and
/// is therefore retryable only when the operation itself is idempotent.
pub fn should_retry_storage_status(status: StatusCode, is_idempotent: bool) -> bool {
    // [Review Fix #3] Keep 408 behind the operation-level idempotency gate;
    // unlike 429/5xx, a timeout does not prove a mutation was rejected.
    status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
        || (is_idempotent && status == StatusCode::REQUEST_TIMEOUT)
}

/// Return the bounded exponential delay for a zero-based retry attempt.
pub fn storage_backoff_delay(attempt: u32) -> Duration {
    let shift = attempt.min(5);
    Duration::from_millis(200_u64.saturating_mul(1_u64 << shift))
}

/// Parse the `Retry-After` delay for a retryable throttling or server response.
///
/// Both delta-seconds and HTTP-date values are accepted. A past HTTP date
/// yields a zero delay, invalid values return `None`, and every valid delay is
/// capped at [`MAX_RETRY_AFTER_DELAY`]. HTTP 408 deliberately uses exponential
/// backoff instead of this header.
pub fn storage_retry_after_delay(
    status: StatusCode,
    headers: &HeaderMap,
    now: SystemTime,
) -> Option<Duration> {
    if status != StatusCode::TOO_MANY_REQUESTS && !status.is_server_error() {
        return None;
    }
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    let delay = value
        .parse::<u64>()
        .map(Duration::from_secs)
        .or_else(|_| {
            httpdate::parse_http_date(value)
                .map(|retry_at| retry_at.duration_since(now).unwrap_or_default())
        })
        .ok()?;
    Some(delay.min(MAX_RETRY_AFTER_DELAY))
}

#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
        }
    }
}

impl RetryConfig {
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let delay = self.base_delay.as_millis() as u64 * 2u64.pow(attempt);
        let jitter = rand_jitter(delay / 4);
        Duration::from_millis((delay + jitter).min(self.max_delay.as_millis() as u64))
    }
}

fn rand_jitter(max: u64) -> u64 {
    // 简单的伪随机抖动
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    seed % (max.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue, RETRY_AFTER};
    use reqwest::StatusCode;
    use std::time::SystemTime;

    #[test]
    fn storage_status_policy_gates_408_by_idempotency() {
        assert!(should_retry_storage_status(
            StatusCode::REQUEST_TIMEOUT,
            true
        ));
        assert!(!should_retry_storage_status(
            StatusCode::REQUEST_TIMEOUT,
            false
        ));
        assert!(should_retry_storage_status(
            StatusCode::TOO_MANY_REQUESTS,
            true
        ));
        assert!(should_retry_storage_status(
            StatusCode::TOO_MANY_REQUESTS,
            false
        ));
        for status in 500..=599 {
            let status = StatusCode::from_u16(status).expect("valid status");
            assert!(should_retry_storage_status(status, true));
            assert!(should_retry_storage_status(status, false));
        }
        assert!(!should_retry_storage_status(StatusCode::BAD_REQUEST, true));
        assert!(!should_retry_storage_status(StatusCode::UNAUTHORIZED, true));
        assert!(!should_retry_storage_status(StatusCode::NOT_FOUND, true));
    }

    #[test]
    fn retry_after_delta_seconds_is_capped_at_ten_minutes() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("900"));

        let delay = storage_retry_after_delay(
            StatusCode::TOO_MANY_REQUESTS,
            &headers,
            SystemTime::UNIX_EPOCH,
        );

        assert_eq!(delay, Some(Duration::from_secs(600)));
    }

    #[test]
    fn retry_after_http_date_uses_remaining_time_for_server_errors() {
        let mut headers = HeaderMap::new();
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Thu, 01 Jan 1970 00:02:00 GMT"),
        );
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(60);

        let delay = storage_retry_after_delay(StatusCode::SERVICE_UNAVAILABLE, &headers, now);

        assert_eq!(delay, Some(Duration::from_secs(60)));
    }

    #[test]
    fn retry_after_past_date_is_immediate() {
        let mut headers = HeaderMap::new();
        headers.insert(
            RETRY_AFTER,
            HeaderValue::from_static("Thu, 01 Jan 1970 00:02:00 GMT"),
        );
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(180);

        let delay = storage_retry_after_delay(StatusCode::BAD_GATEWAY, &headers, now);

        assert_eq!(delay, Some(Duration::ZERO));
    }

    #[test]
    fn retry_after_is_ignored_for_408_and_invalid_values() {
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("5"));
        assert_eq!(
            storage_retry_after_delay(
                StatusCode::REQUEST_TIMEOUT,
                &headers,
                SystemTime::UNIX_EPOCH,
            ),
            None
        );

        headers.insert(RETRY_AFTER, HeaderValue::from_static("invalid"));
        assert_eq!(
            storage_retry_after_delay(
                StatusCode::INTERNAL_SERVER_ERROR,
                &headers,
                SystemTime::UNIX_EPOCH,
            ),
            None
        );
    }

    #[test]
    fn storage_backoff_is_exponential_and_bounded() {
        assert_eq!(storage_backoff_delay(0), Duration::from_millis(200));
        assert_eq!(storage_backoff_delay(1), Duration::from_millis(400));
        assert_eq!(storage_backoff_delay(5), Duration::from_millis(6_400));
        assert_eq!(storage_backoff_delay(30), Duration::from_millis(6_400));
    }
}
