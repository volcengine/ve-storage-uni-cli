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

#[derive(Debug, Clone, Copy)]
pub enum UploadStrategy {
    Simple,
    Multipart { part_size: u64 },
    Stream,
}

impl UploadStrategy {
    pub fn auto_select(file_size: u64) -> Self {
        if file_size <= 5 * 1024 * 1024 * 1024 {
            Self::Simple
        } else {
            let part_size = Self::optimal_part_size(file_size);
            Self::Multipart { part_size }
        }
    }

    fn optimal_part_size(file_size: u64) -> u64 {
        if file_size < 1024 * 1024 * 1024 {
            20 * 1024 * 1024 // 20MB
        } else if file_size < 10 * 1024 * 1024 * 1024 {
            50 * 1024 * 1024 // 50MB
        } else {
            100 * 1024 * 1024 // 100MB
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upload_strategy_small_file() {
        let strategy = UploadStrategy::auto_select(1024 * 1024); // 1MB
        assert!(matches!(strategy, UploadStrategy::Simple));
    }

    #[test]
    fn test_upload_strategy_large_file() {
        let strategy = UploadStrategy::auto_select(10 * 1024 * 1024 * 1024); // 10GB
        assert!(matches!(strategy, UploadStrategy::Multipart { .. }));
    }
}
