use super::*;
use crate::infra::crypto;
use base64::{engine::general_purpose::STANDARD, Engine};

const MAX_FILE_BYTES: u64 = 128 * 1024;

impl Store {
    fn encryption_key(&self) -> Result<Zeroizing<[u8; 32]>> {
        let bytes = protected::read(&self.config.home.join(".key"), 32)?;
        let mut key = Zeroizing::new([0u8; 32]);
        if bytes.len() != key.len() {
            return Err(Error::Storage);
        }
        key.copy_from_slice(&bytes);
        Ok(key)
    }

    pub(super) fn decrypt_legacy(&self) -> Result<Option<Zeroizing<Vec<u8>>>> {
        if !protected::exists(&self.config.home.join("tokens"))? {
            return Ok(None);
        }
        let bytes = protected::read(&self.config.home.join("tokens"), MAX_FILE_BYTES)?;
        let key = self.encryption_key()?;
        if let Ok(encoded) = std::str::from_utf8(&bytes) {
            if crypto::is_encrypted(encoded) {
                if let Ok(plaintext) = crypto::decrypt_with_key(&key, encoded) {
                    return Ok(Some(Zeroizing::new(plaintext.into_bytes())));
                }
            }
        }
        // [Review Fix #4] Preserve logins from the old nonce+ciphertext format until their next save.
        let encoded = format!(
            "{}{}",
            crypto::ENC_PREFIX,
            STANDARD.encode(bytes.as_slice())
        );
        let plaintext = crypto::decrypt_with_key(&key, &encoded).map_err(|_| Error::Storage)?;
        Ok(Some(Zeroizing::new(plaintext.into_bytes())))
    }
}
