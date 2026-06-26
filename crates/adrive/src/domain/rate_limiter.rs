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

use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RateLimiter {
    capacity: u64,
    rate: u64,
    tokens_checkpoint: Mutex<(u64, Instant)>,
}

impl PartialEq for RateLimiter {
    fn eq(&self, other: &Self) -> bool {
        self.capacity == other.capacity && self.rate == other.rate
    }
}

impl RateLimiter {
    pub fn new(capacity: i64, rate: i64) -> Self {
        let rate = rate.max(0) as u64;
        let mut capacity = capacity.max(0) as u64;
        if capacity < rate {
            capacity = rate;
        }

        Self {
            capacity,
            rate,
            tokens_checkpoint: Mutex::new((0, Instant::now())),
        }
    }

    fn is_unlimited(&self) -> bool {
        self.capacity == 0 || self.rate == 0
    }

    pub fn acquire(&self, want: usize) -> (bool, Option<Duration>) {
        if self.is_unlimited() || want == 0 {
            return (true, None);
        }

        let want = (want as u64).min(self.capacity);
        let mut checkpoint = self.tokens_checkpoint.lock().unwrap();
        let now = Instant::now();
        let elapsed_ms = now.duration_since(checkpoint.1).as_millis() as u64;
        let delta = elapsed_ms.saturating_mul(self.rate) / 1000 + 1;
        let tokens = checkpoint.0.saturating_add(delta).min(self.capacity);

        if tokens >= want {
            checkpoint.0 = tokens - want;
            checkpoint.1 = now;
            return (true, None);
        }

        checkpoint.0 = tokens;
        checkpoint.1 = now;

        let missing = want - tokens;
        let wait_ms =
            ((missing.saturating_mul(1000)).saturating_add(self.rate - 1) / self.rate).max(10);
        (false, Some(Duration::from_millis(wait_ms)))
    }
}
