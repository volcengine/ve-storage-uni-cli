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

//! Integration tests for current transfer primitives.
//! [Review Fix #3] Align transfer tests with current UploadStrategy/Checkpoint primitives.

#[allow(dead_code)]
#[path = "../src/transfer/ratelimit.rs"]
mod ratelimit;

use ratelimit::AtomicTokenBucket;
use tos_core::transfer::checkpoint::{Checkpoint, CompletedPart};
use tos_core::transfer::upload::UploadStrategy;

#[test]
fn test_upload_strategy_small_file_uses_simple_upload() {
    let strategy = UploadStrategy::auto_select(1024 * 1024);
    assert!(matches!(strategy, UploadStrategy::Simple));
}

#[test]
fn test_upload_strategy_large_file_uses_multipart_upload() {
    let strategy = UploadStrategy::auto_select(6 * 1024 * 1024 * 1024);
    match strategy {
        UploadStrategy::Multipart { part_size } => assert!(part_size >= 20 * 1024 * 1024),
        other => panic!("expected multipart strategy, got {other:?}"),
    }
}

#[test]
fn test_create_token_bucket_and_try_acquire() {
    let bucket = AtomicTokenBucket::new(1_000_000, 10_000_000);
    assert!(bucket.try_acquire(1));
}

#[test]
fn test_token_bucket_with_rate_allows_initial_capacity() {
    let bucket = AtomicTokenBucket::with_rate(1_000_000);
    assert!(bucket.try_acquire(1_000_000));
}

#[test]
fn test_token_bucket_try_acquire_rejects_insufficient_tokens() {
    let bucket = AtomicTokenBucket::new(100, 100);
    assert!(!bucket.try_acquire(200));
}

#[test]
fn test_checkpoint_serialization_roundtrip() {
    let checkpoint = Checkpoint {
        bucket: "demo-bucket".into(),
        key: "demo-key".into(),
        source_path: Some("/tmp/file.dat".into()),
        file_size: 2048,
        part_size: 1024,
        upload_id: Some("upload-123".into()),
        completed_parts: vec![CompletedPart {
            part_number: 1,
            etag: "etag-1".into(),
            crc64: Some(123),
        }],
    };

    let json = serde_json::to_string(&checkpoint).unwrap();
    let deserialized: Checkpoint = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.bucket, "demo-bucket");
    assert_eq!(deserialized.key, "demo-key");
    assert_eq!(deserialized.completed_parts.len(), 1);
    assert_eq!(deserialized.completed_parts[0].crc64, Some(123));
}

#[test]
fn test_checkpoint_dir_ends_with_tos_checkpoints() {
    let path = Checkpoint::checkpoint_dir();
    let rendered = path.to_string_lossy();
    assert!(rendered.contains(".tos"));
    assert!(rendered.contains("checkpoints"));
}
