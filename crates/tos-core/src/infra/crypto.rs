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

//! AES-256-GCM 加解密，用于配置文件中 AK/SK 等敏感字段的本地保护。
//!
//! 密钥派生策略：
//! - 在 `~/.tos/.key` 下存储随机生成的 32 字节主密钥
//! - 首次写入时若文件不存在则自动生成，权限设置为 0600（Unix）
//! - 密文格式：`ENC:<base64(nonce(12) || ciphertext || tag)>`
//!
//! 这不是抵御高能力攻击者的强安全方案（本地盘上任何读到 .key 的人都能解密），
//! 但可以防止 AK/SK 以明文被误分享、误提交到 VCS 或日志，满足设计文档 7.3 中
//! "敏感字段本地 AES-256-GCM 加密存储" 的要求。

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;
use std::path::{Path, PathBuf};

use crate::agent::error::CliError;

/// 加密值前缀。
pub const ENC_PREFIX: &str = "ENC:";

/// 判断一个字符串是否是已经加密的密文。
pub fn is_encrypted(s: &str) -> bool {
    s.starts_with(ENC_PREFIX)
}

/// 计算主密钥文件路径，默认在 `<config_dir>/.key`。
pub fn key_path(config_dir: &Path) -> PathBuf {
    config_dir.join(".key")
}

/// 加载 `config_dir/.key` 中的 32 字节主密钥，缺失时原子初始化并返回。
/// 已有密钥仅执行读取；首次初始化通过独立目录锁协调所有调用方。
/// 返回读写、锁超时或密钥长度校验错误。
pub fn load_or_init_key(config_dir: &Path) -> Result<[u8; 32], CliError> {
    let path = key_path(config_dir);
    if let Some(key) = read_key(&path)? {
        return Ok(key);
    }
    // [Review Fix #CredentialTransaction13] Config, credential and direct crypto callers share one first-init lock; never acquire a credentials transaction from this lock.
    let _guard = super::credentials::transaction::Lock::acquire_key_init(config_dir)?;
    if let Some(key) = read_key(&path)? {
        return Ok(key);
    }
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    // [Review Fix #5] Persist the key before encrypted credentials: interrupted writes must not leave a partial key.
    super::atomic_file::write_owner_only_atomic(&path, &key)?;
    Ok(key)
}

fn read_key(path: &Path) -> Result<Option<[u8; 32]>, CliError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(CliError::Io(std::io::Error::new(
                error.kind(),
                format!("Failed to read master key at {}: {}", path.display(), error),
            )))
        }
    };
    if bytes.len() != 32 {
        return Err(CliError::ValidationError(format!(
            "Master key at {} is corrupted (expected 32 bytes, found {})",
            path.display(),
            bytes.len(),
        )));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(Some(key))
}

/// 用给定 32 字节密钥加密明文。
pub fn encrypt_with_key(key: &[u8; 32], plaintext: &str) -> Result<String, CliError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| CliError::ValidationError(format!("AES-GCM encrypt failed: {}", e)))?;
    let mut buf = Vec::with_capacity(12 + ct.len());
    buf.extend_from_slice(&nonce_bytes);
    buf.extend_from_slice(&ct);
    Ok(format!("{}{}", ENC_PREFIX, B64.encode(buf)))
}

/// 用给定 32 字节密钥解密密文。入参须是 `ENC:...` 形式。
pub fn decrypt_with_key(key: &[u8; 32], ciphertext: &str) -> Result<String, CliError> {
    let body = ciphertext
        .strip_prefix(ENC_PREFIX)
        .ok_or_else(|| CliError::ValidationError("value is not ENC:-prefixed".into()))?;
    let raw = B64
        .decode(body)
        .map_err(|e| CliError::ValidationError(format!("Invalid base64 ciphertext: {}", e)))?;
    if raw.len() < 12 + 16 {
        return Err(CliError::ValidationError(
            "Ciphertext too short (missing nonce/tag)".into(),
        ));
    }
    let (nonce_bytes, ct) = raw.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);
    let pt = cipher
        .decrypt(nonce, ct)
        .map_err(|e| CliError::ValidationError(format!("AES-GCM decrypt failed: {}", e)))?;
    String::from_utf8(pt).map_err(|e| {
        CliError::ValidationError(format!("Decrypted value is not valid UTF-8: {}", e))
    })
}

/// 便捷函数：基于 `<config_dir>/.key` 加密。
pub fn encrypt_in_dir(config_dir: &Path, plaintext: &str) -> Result<String, CliError> {
    let key = load_or_init_key(config_dir)?;
    encrypt_with_key(&key, plaintext)
}

/// 便捷函数：基于 `<config_dir>/.key` 解密。
pub fn decrypt_in_dir(config_dir: &Path, ciphertext: &str) -> Result<String, CliError> {
    let key = load_or_init_key(config_dir)?;
    decrypt_with_key(&key, ciphertext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = [7u8; 32];
        let ct = encrypt_with_key(&key, "hello AKXXX").unwrap();
        assert!(is_encrypted(&ct));
        let pt = decrypt_with_key(&key, &ct).unwrap();
        assert_eq!(pt, "hello AKXXX");
    }

    #[test]
    fn wrong_key_fails() {
        let key = [7u8; 32];
        let bad = [8u8; 32];
        let ct = encrypt_with_key(&key, "secret").unwrap();
        assert!(decrypt_with_key(&bad, &ct).is_err());
    }

    #[test]
    fn prefix_detection() {
        assert!(is_encrypted("ENC:abcdef"));
        assert!(!is_encrypted("plaintext"));
    }

    #[test]
    fn concurrent_direct_initializers_return_one_persisted_master_key() {
        use std::sync::{Arc, Barrier};
        let directory = key_test_directory("concurrent");
        let barrier = Arc::new(Barrier::new(16));
        let writers: Vec<_> = (0..16)
            .map(|_| {
                let directory = directory.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    load_or_init_key(&directory).unwrap()
                })
            })
            .collect();
        let returned_keys: Vec<_> = writers
            .into_iter()
            .map(|writer| writer.join().unwrap())
            .collect();
        let persisted = std::fs::read(key_path(&directory)).unwrap();
        assert!(
            returned_keys.iter().all(|key| key.as_slice() == persisted),
            "all callers must receive the one persisted key"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn key_initializer_rechecks_after_waiting_for_another_publisher() {
        use fs2::FileExt;
        let directory = key_test_directory("double-check");
        std::fs::create_dir_all(&directory).unwrap();
        let blocker = std::fs::File::create(directory.join(".key.lock")).unwrap();
        FileExt::lock_exclusive(&blocker).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker_directory = directory.clone();
        let writer = std::thread::spawn(move || {
            sender.send(load_or_init_key(&worker_directory)).unwrap();
        });
        let premature = receiver
            .recv_timeout(std::time::Duration::from_millis(200))
            .ok();
        let published = [11_u8; 32];
        std::fs::write(key_path(&directory), published).unwrap();
        FileExt::unlock(&blocker).unwrap();
        let returned = match premature {
            Some(result) => result.unwrap(),
            None => receiver.recv().unwrap().unwrap(),
        };
        writer.join().unwrap();
        assert!(
            returned == published,
            "a waiting initializer must reuse the key published by the lock holder"
        );
        assert!(std::fs::read(key_path(&directory)).unwrap() == published);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn existing_key_is_read_without_touching_the_initialization_lock() {
        let directory = key_test_directory("existing-read-only");
        std::fs::create_dir_all(&directory).unwrap();
        let original_key = [7_u8; 32];
        std::fs::write(key_path(&directory), original_key).unwrap();
        // A directory cannot be opened as an initialization lock; existing-key reads must bypass it.
        std::fs::create_dir(directory.join(".key.lock")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o500)).unwrap();
        }
        assert!(load_or_init_key(&directory).unwrap() == original_key);
        let ciphertext = encrypt_in_dir(&directory, "synthetic-read-only").unwrap();
        assert_eq!(
            decrypt_in_dir(&directory, &ciphertext).unwrap(),
            "synthetic-read-only"
        );
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        assert!(directory.join(".key.lock").is_dir());
        assert!(!directory.join(".credentials.lock").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupted_existing_key_is_rejected_without_initialization_writes() {
        let directory = key_test_directory("corrupted-read-only");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(key_path(&directory), [7_u8; 31]).unwrap();
        assert!(matches!(
            load_or_init_key(&directory),
            Err(CliError::ValidationError(_))
        ));
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        assert_eq!(std::fs::metadata(key_path(&directory)).unwrap().len(), 31);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn key_test_directory(label: &str) -> PathBuf {
        std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("credential-key-{label}-{}", uuid::Uuid::now_v7()))
    }

    #[cfg(unix)]
    #[test]
    fn failed_key_creation_leaves_no_partial_key_and_can_be_retried() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // Root bypasses the permission failure used by this fixture.
        }
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("credential-key-failure-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&directory).unwrap();
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o500)).unwrap();
        let outcome = load_or_init_key(&directory);
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(outcome.is_err());
        assert!(!key_path(&directory).exists());
        let key = load_or_init_key(&directory).unwrap();
        assert_eq!(load_or_init_key(&directory).unwrap(), key);
        // [Review Fix #CredentialTransaction14] The retained empty initialization lock is coordination metadata, not a second master key.
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        assert_eq!(
            std::fs::metadata(directory.join(".key.lock"))
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            std::fs::metadata(key_path(&directory))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
}
