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

/// 加载或初始化主密钥，长度固定为 32 字节。
pub fn load_or_init_key(config_dir: &Path) -> Result<[u8; 32], CliError> {
    let path = key_path(config_dir);
    if path.exists() {
        let bytes = std::fs::read(&path).map_err(|e| {
            CliError::Io(std::io::Error::new(
                e.kind(),
                format!("Failed to read master key at {}: {}", path.display(), e),
            ))
        })?;
        if bytes.len() != 32 {
            return Err(CliError::ValidationError(format!(
                "Master key at {} is corrupted (expected 32 bytes, found {})",
                path.display(),
                bytes.len()
            )));
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        return Ok(out);
    }

    if !config_dir.exists() {
        std::fs::create_dir_all(config_dir).map_err(|e| {
            CliError::Io(std::io::Error::new(
                e.kind(),
                format!(
                    "Failed to create config dir {}: {}",
                    config_dir.display(),
                    e
                ),
            ))
        })?;
    }

    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    std::fs::write(&path, key).map_err(|e| {
        CliError::Io(std::io::Error::new(
            e.kind(),
            format!("Failed to write master key to {}: {}", path.display(), e),
        ))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&path) {
            let mut perm = meta.permissions();
            perm.set_mode(0o600);
            let _ = std::fs::set_permissions(&path, perm);
        }
    }

    Ok(key)
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
}
