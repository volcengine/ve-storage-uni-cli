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

use std::sync::Arc;
use tokio::sync::Semaphore;

/// A pool of concurrent transfer workers.
#[derive(Debug)]
pub struct TransferPool {
    /// Name of this pool (for logging).
    pub name: String,
    /// Semaphore controlling concurrency.
    semaphore: Arc<Semaphore>,
    /// Maximum concurrent tasks.
    pub max_concurrency: usize,
}

impl TransferPool {
    /// Create a new transfer pool with the given concurrency limit.
    pub fn new(name: impl Into<String>, max_concurrency: usize) -> Self {
        Self {
            name: name.into(),
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            max_concurrency,
        }
    }

    /// Acquire a permit from this pool. Blocks until one is available.
    pub async fn acquire(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, tokio::sync::AcquireError> {
        self.semaphore.clone().acquire_owned().await
    }

    /// Get current number of available permits.
    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }
}

/// Dual-pool transfer engine that separates small and large file transfers.
///
/// Small files (< threshold) go to a high-concurrency pool.
/// Large files (>= threshold) go to a lower-concurrency pool that
/// uses multipart upload/download internally.
pub struct DualPoolTransferEngine {
    /// Pool for small file transfers.
    pub small_pool: TransferPool,
    /// Pool for large file transfers.
    pub large_pool: TransferPool,
    /// Size threshold in bytes. Files below this go to small_pool.
    pub threshold: u64,
}

impl DualPoolTransferEngine {
    /// Create a new dual-pool engine with default settings.
    pub fn new() -> Self {
        Self {
            small_pool: TransferPool::new("small-files", 16),
            large_pool: TransferPool::new("large-files", 4),
            threshold: 64 * 1024 * 1024, // 64 MiB
        }
    }

    /// Create a custom dual-pool engine.
    pub fn with_config(small_concurrency: usize, large_concurrency: usize, threshold: u64) -> Self {
        Self {
            small_pool: TransferPool::new("small-files", small_concurrency),
            large_pool: TransferPool::new("large-files", large_concurrency),
            threshold,
        }
    }

    /// Select the appropriate pool based on file size.
    pub fn select_pool(&self, file_size: u64) -> &TransferPool {
        if file_size < self.threshold {
            &self.small_pool
        } else {
            &self.large_pool
        }
    }
}

impl Default for DualPoolTransferEngine {
    fn default() -> Self {
        Self::new()
    }
}
