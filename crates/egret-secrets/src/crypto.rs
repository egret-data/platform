//! AES-256-GCM authenticated encryption for credentials.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use rand::RngCore;

use crate::error::SecretsError;

/// Wraps an AES-256-GCM cipher built from a 32-byte master key.
#[derive(Clone)]
pub struct Crypto {
    cipher: Aes256Gcm,
}

impl Crypto {
    /// Build from a key of at least 32 bytes (only the first 32 are used).
    pub fn new(key: &[u8]) -> Result<Self, SecretsError> {
        if key.len() < 32 {
            return Err(SecretsError::Crypto(
                "master key must be >= 32 bytes".into(),
            ));
        }
        let key = Key::<Aes256Gcm>::from_slice(&key[..32]);
        Ok(Self {
            cipher: Aes256Gcm::new(key),
        })
    }

    /// Encrypt `plaintext` with a fresh random 96-bit nonce.
    /// Returns `(nonce[12], ciphertext)` (ciphertext includes the GCM auth tag).
    pub fn seal(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), SecretsError> {
        let mut nonce_bytes = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ct = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|_| SecretsError::Crypto("encryption failed".into()))?;
        Ok((nonce_bytes.to_vec(), ct))
    }

    /// Decrypt; fails on a wrong key or tampered ciphertext (GCM auth-tag check).
    pub fn open(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<String, SecretsError> {
        if nonce.len() != 12 {
            return Err(SecretsError::Crypto("nonce must be 12 bytes".into()));
        }
        let nonce = Nonce::from_slice(nonce);
        let pt = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| SecretsError::Crypto("decryption failed".into()))?;
        String::from_utf8(pt).map_err(|_| SecretsError::Crypto("non-utf8 plaintext".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        let mut k = [0u8; 32];
        for (i, b) in k.iter_mut().enumerate() {
            *b = i as u8;
        }
        k
    }

    #[test]
    fn seal_then_open_roundtrips() {
        let c = Crypto::new(&key()).unwrap();
        let (nonce, ct) = c.seal("hunter2").unwrap();
        assert_eq!(nonce.len(), 12);
        assert_ne!(ct, b"hunter2"); // actually encrypted
        assert_eq!(c.open(&nonce, &ct).unwrap(), "hunter2");
    }

    #[test]
    fn tampered_ciphertext_fails() {
        let c = Crypto::new(&key()).unwrap();
        let (nonce, mut ct) = c.seal("secret").unwrap();
        ct[0] ^= 0xFF;
        assert!(c.open(&nonce, &ct).is_err());
    }

    #[test]
    fn wrong_key_fails_to_open() {
        let c1 = Crypto::new(&key()).unwrap();
        let (nonce, ct) = c1.seal("secret").unwrap();
        let mut k2 = key();
        k2[0] ^= 0xFF;
        let c2 = Crypto::new(&k2).unwrap();
        assert!(c2.open(&nonce, &ct).is_err());
    }

    #[test]
    fn short_key_is_rejected() {
        assert!(Crypto::new(&[0u8; 16]).is_err());
    }
}
