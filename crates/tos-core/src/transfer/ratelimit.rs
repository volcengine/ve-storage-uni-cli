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

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Atomic token bucket rate limiter for bandwidth control.
///
/// Uses an atomic counter to track available tokens (bytes).
/// Tokens are replenished at a fixed rate (bytes per second).
pub struct AtomicTokenBucket {
    /// Maximum tokens (burst capacity in bytes).
    capacity: u64,
    /// Tokens added per second (rate limit in bytes/sec).
    rate: u64,
    /// Current available tokens.
    tokens: AtomicU64,
    /// Last refill timestamp (stored as nanos since some epoch).
    last_refill: std::sync::Mutex<Instant>,
}

impl AtomicTokenBucket {
    /// Create a new token bucket.
    ///
    /// `rate_bytes_per_sec` - Maximum bytes per second.
    /// `burst_bytes` - Maximum burst size in bytes (bucket capacity).
    pub fn new(rate_bytes_per_sec: u64, burst_bytes: u64) -> Self {
        Self {
            capacity: burst_bytes,
            rate: rate_bytes_per_sec,
            tokens: AtomicU64::new(burst_bytes),
            last_refill: std::sync::Mutex::new(Instant::now()),
        }
    }

    /// Create a token bucket with rate limit only (burst = rate).
    pub fn with_rate(rate_bytes_per_sec: u64) -> Self {
        Self::new(rate_bytes_per_sec, rate_bytes_per_sec)
    }

    /// Refill tokens based on elapsed time.
    fn refill(&self) {
        let mut last = self.last_refill.lock().unwrap();
        let now = Instant::now();
        let elapsed = now.duration_since(*last);
        let new_tokens = (elapsed.as_secs_f64() * self.rate as f64) as u64;
        if new_tokens > 0 {
            let current = self.tokens.load(Ordering::Relaxed);
            let updated = (current + new_tokens).min(self.capacity);
            self.tokens.store(updated, Ordering::Relaxed);
            *last = now;
        }
    }

    /// Acquire `amount` tokens, waiting if necessary.
    ///
    /// Returns when the requested amount of tokens has been acquired.
    pub async fn acquire(&self, amount: u64) {
        loop {
            self.refill();
            let current = self.tokens.load(Ordering::Relaxed);
            if current >= amount {
                // Try to consume tokens
                let new_val = current.saturating_sub(amount);
                if self
                    .tokens
                    .compare_exchange(current, new_val, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
                {
                    return;
                }
                // CAS failed, retry
                continue;
            }
            // Not enough tokens, wait for refill
            let needed = amount - current;
            let wait_secs = needed as f64 / self.rate as f64;
            sleep(Duration::from_secs_f64(wait_secs.max(0.001))).await;
        }
    }

    /// Try to acquire tokens without waiting.
    /// Returns true if tokens were acquired, false otherwise.
    pub fn try_acquire(&self, amount: u64) -> bool {
        self.refill();
        let current = self.tokens.load(Ordering::Relaxed);
        if current >= amount {
            let new_val = current.saturating_sub(amount);
            self.tokens
                .compare_exchange(current, new_val, Ordering::SeqCst, Ordering::Relaxed)
                .is_ok()
        } else {
            false
        }
    }
}
