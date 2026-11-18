//! Service-specific, versioned cryptography. No value containing secrets implements Debug.
use crate::validation::is_uuid_v7;
use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng, Payload},
    Aes256Gcm, Nonce,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::BTreeMap;
use uuid::Uuid;
use zeroize::Zeroizing;

// [Review Fix #20] Production builds must retain zeroizing AES and hash backend destruction.
// RUSTFLAGS can override repository flags; fail compilation if that selects a nondropping backend.
const _: () = {
    fn requires_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
    let _ = requires_zeroize_on_drop::<aes_gcm::aes::Aes256>;
    assert!(
        std::mem::needs_drop::<ghash::GHash>(),
        "ask-auth requires zeroizing GHASH: preserve --cfg polyval_force_soft in RUSTFLAGS"
    );
    assert!(
        std::mem::needs_drop::<polyval::Polyval>(),
        "ask-auth requires zeroizing POLYVAL: preserve --cfg polyval_force_soft in RUSTFLAGS"
    );
};

/// Safe cryptographic failure; contains no supplied value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CryptoError;

/// Opaque bytes erased on drop; deliberately does not implement Debug.
pub struct SecretBytes(Zeroizing<Vec<u8>>);
impl std::ops::Deref for SecretBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}
/// Opaque HMAC digest erased on drop; deliberately does not implement Debug.
pub struct SecretDigest(Zeroizing<[u8; 32]>);
impl std::ops::Deref for SecretDigest {
    type Target = [u8; 32];
    fn deref(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Independent HMAC and encryption material for one nonzero version.
pub struct KeyMaterial {
    version: u16,
    hmac: Zeroizing<[u8; 32]>,
    aes: Zeroizing<[u8; 32]>,
}
impl KeyMaterial {
    /// Construct material. Returns an error for zero version or identical keys.
    pub fn new(version: u16, hmac: [u8; 32], aes: [u8; 32]) -> Result<Self, CryptoError> {
        if version == 0 || hmac == aes {
            return Err(CryptoError);
        }
        Ok(Self {
            version,
            hmac: Zeroizing::new(hmac),
            aes: Zeroizing::new(aes),
        })
    }
}

/// Separate HMAC namespaces; callers must choose the exact semantic purpose.
#[derive(Clone, Copy)]
pub enum Purpose {
    LoginRequest,
    LoginTicket,
    OAuthState,
    Broker,
    RefreshToken,
    RefreshRequest,
}
impl Purpose {
    fn label(self) -> &'static [u8] {
        match self {
            Self::LoginRequest => b"login-request-v1",
            Self::LoginTicket => b"login-ticket-v1",
            // [Review Fix #13] Match the frozen OAuth-state purpose literal; canonical vector prevents drift.
            Self::OAuthState => b"ask.oauth-state.v1",
            Self::Broker => b"broker-v1",
            Self::RefreshToken => b"refresh-token-v1",
            Self::RefreshRequest => b"refresh-request-v1",
        }
    }
}

/// Immutable AEAD resource binding; result generation never comes from current row state.
pub struct Binding {
    purpose: &'static [u8],
    auth_id: Uuid,
    request_id: Option<Uuid>,
    generation: Option<u64>,
    maximum: usize,
}
impl Binding {
    /// Bind OAuth context to its auth ID. Invalid UUIDs are rejected by encryption/decryption.
    pub fn context(auth_id: Uuid) -> Self {
        Self {
            purpose: b"oauth-context-v1",
            auth_id,
            request_id: None,
            generation: None,
            maximum: 32768,
        }
    }
    /// Bind original login result to fixed output generation one.
    pub fn login(auth_id: Uuid) -> Self {
        Self {
            purpose: b"login-result-v1",
            auth_id,
            request_id: None,
            generation: Some(1),
            maximum: 262144,
        }
    }
    /// Bind refresh result to auth/request IDs and checked input+1; invalid IDs or overflow fail.
    pub fn refresh(auth_id: Uuid, request_id: Uuid, input: u64) -> Result<Self, CryptoError> {
        // [Review Fix #21] Refresh AAD accepts only v7 operation IDs, independent of auth_id.
        if !is_uuid_v7(auth_id) || !is_uuid_v7(request_id) || input == 0 {
            return Err(CryptoError);
        }
        Ok(Self {
            purpose: b"refresh-result-v1",
            auth_id,
            request_id: Some(request_id),
            generation: Some(input.checked_add(1).ok_or(CryptoError)?),
            maximum: 262144,
        })
    }
}

/// Immutable deployment-scoped key ring. Old versions are selected explicitly, never tried in turn.
pub struct Crypto {
    deployment: String,
    app: String,
    keys: BTreeMap<u16, KeyMaterial>,
}
impl Crypto {
    /// Construct a ring from injected material; empty identity, duplicate versions or no keys fail.
    pub fn new(deployment: &str, app: &str, keys: Vec<KeyMaterial>) -> Result<Self, CryptoError> {
        if deployment.is_empty() || app.is_empty() || keys.is_empty() {
            return Err(CryptoError);
        }
        let mut ring = BTreeMap::new();
        for key in keys {
            if ring.insert(key.version, key).is_some() {
                return Err(CryptoError);
            }
        }
        Ok(Self {
            deployment: deployment.into(),
            app: app.into(),
            keys: ring,
        })
    }
    /// HMAC canonical length-prefixed fields with an explicit stored version and resource identity.
    /// Returns a secret digest, or a safe error for unknown version/invalid identity/oversize input.
    pub fn digest(
        &self,
        version: u16,
        purpose: Purpose,
        auth_id: Uuid,
        fields: &[&[u8]],
    ) -> Result<SecretDigest, CryptoError> {
        let key = self.keys.get(&version).ok_or(CryptoError)?;
        let encoded = self.canonical(purpose.label(), auth_id, fields)?;
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&*key.hmac).map_err(|_| CryptoError)?;
        mac.update(&encoded);
        Ok(SecretDigest(Zeroizing::new(
            mac.finalize().into_bytes().into(),
        )))
    }
    /// Constant-time proof verification using only the row's stored key version.
    /// Returns false for a mismatching digest and an error for invalid input/unknown key.
    pub fn verify(
        &self,
        version: u16,
        purpose: Purpose,
        auth_id: Uuid,
        fields: &[&[u8]],
        expected: &[u8; 32],
    ) -> Result<bool, CryptoError> {
        let key = self.keys.get(&version).ok_or(CryptoError)?;
        let encoded = self.canonical(purpose.label(), auth_id, fields)?;
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&*key.hmac).map_err(|_| CryptoError)?;
        mac.update(&encoded);
        Ok(mac.verify_slice(expected).is_ok())
    }
    /// Encrypt bounded nonempty plaintext using a fresh CSPRNG nonce. Returns the v1 envelope.
    /// Invalid binding, unknown key, oversize plaintext or encryption failure returns a safe error.
    pub fn encrypt(
        &self,
        version: u16,
        binding: &Binding,
        plaintext: &[u8],
    ) -> Result<SecretBytes, CryptoError> {
        if plaintext.is_empty() || plaintext.len() > binding.maximum - 31 {
            return Err(CryptoError);
        }
        let cipher = self.cipher(version)?;
        let mut nonce_bytes = [0_u8; 12];
        OsRng
            .try_fill_bytes(&mut nonce_bytes)
            .map_err(|_| CryptoError)?;
        let nonce = Nonce::from_slice(&nonce_bytes);
        let aad = self.aad(binding, version)?;
        let encrypted = cipher
            .encrypt(
                nonce,
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError)?;
        let mut envelope = Zeroizing::new(Vec::with_capacity(encrypted.len() + 15));
        envelope.push(1);
        envelope.extend_from_slice(&version.to_be_bytes());
        envelope.extend_from_slice(nonce);
        envelope.extend_from_slice(&encrypted);
        Ok(SecretBytes(envelope))
    }
    /// Authenticate and decrypt a bounded envelope; selects only its encoded version.
    /// Malformed format/length, unknown key, wrong AAD or tag fails without plaintext.
    pub fn decrypt(&self, binding: &Binding, envelope: &[u8]) -> Result<SecretBytes, CryptoError> {
        if !(32..=binding.maximum).contains(&envelope.len()) || envelope[0] != 1 {
            return Err(CryptoError);
        }
        let version = u16::from_be_bytes([envelope[1], envelope[2]]);
        let aad = self.aad(binding, version)?;
        let plaintext = self
            .cipher(version)?
            .decrypt(
                Nonce::from_slice(&envelope[3..15]),
                Payload {
                    msg: &envelope[15..],
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError)?;
        Ok(SecretBytes(Zeroizing::new(plaintext)))
    }
    /// Whether an explicit stored version exists; no key material is returned.
    pub fn has_version(&self, version: u16) -> bool {
        self.keys.contains_key(&version)
    }
    pub(crate) fn deployment(&self) -> &str {
        &self.deployment
    }
    pub(crate) fn app(&self) -> &str {
        &self.app
    }
    fn cipher(&self, version: u16) -> Result<Aes256Gcm, CryptoError> {
        Aes256Gcm::new_from_slice(&*self.keys.get(&version).ok_or(CryptoError)?.aes)
            .map_err(|_| CryptoError)
    }
    fn aad(&self, binding: &Binding, version: u16) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        let request = binding.request_id.map(|id| *id.as_bytes());
        let generation = binding.generation.map(u64::to_be_bytes);
        let mut fields: Vec<&[u8]> = vec![];
        let version_bytes = version.to_be_bytes();
        fields.push(&version_bytes);
        if let Some(ref id) = request {
            fields.push(id);
        }
        if let Some(ref value) = generation {
            fields.push(value);
        }
        self.canonical(binding.purpose, binding.auth_id, &fields)
    }
    fn canonical(
        &self,
        purpose: &[u8],
        auth_id: Uuid,
        fields: &[&[u8]],
    ) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        if !is_uuid_v7(auth_id) || fields.len() > 100 {
            return Err(CryptoError);
        }
        let mut encoded = Zeroizing::new(Vec::new());
        for field in [
            b"ask-auth-v1".as_slice(),
            purpose,
            self.deployment.as_bytes(),
            self.app.as_bytes(),
            auth_id.as_bytes(),
        ]
        .into_iter()
        .chain(fields.iter().copied())
        {
            if field.len() > 262144 || encoded.len() + field.len() + 4 > 1048576 {
                return Err(CryptoError);
            }
            encoded.extend_from_slice(&(field.len() as u32).to_be_bytes());
            encoded.extend_from_slice(field);
        }
        Ok(encoded)
    }
}
