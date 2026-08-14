//! AES-256-GCM authenticated encryption for credentials.

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
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
        // `KeyInit::new_from_slice` rather than `Key::from_slice`: generic-array
        // deprecated `GenericArray::from_slice`, and this constructor never names
        // GenericArray at all. The length is checked above, so it cannot fail.
        let cipher = Aes256Gcm::new_from_slice(&key[..32])
            .map_err(|_| SecretsError::Crypto("master key must be >= 32 bytes".into()))?;
        Ok(Self { cipher })
    }

    /// Encrypt `plaintext` with a fresh random 96-bit nonce.
    /// Returns `(nonce[12], ciphertext)` (ciphertext includes the GCM auth tag).
    pub fn seal(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), SecretsError> {
        let mut nonce_bytes = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| SecretsError::Crypto("encryption failed".into()))?;
        Ok((nonce_bytes.to_vec(), ct))
    }

    /// Decrypt; fails on a wrong key or tampered ciphertext (GCM auth-tag check).
    pub fn open(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<String, SecretsError> {
        // The `try_into` IS the length check — a 12-byte array or an error, so the
        // separate `nonce.len() != 12` guard it replaces cannot drift out of sync
        // with the type. (`Nonce::from_slice` would have panicked on a bad length;
        // it is also deprecated now.)
        let nonce: [u8; 12] = nonce
            .try_into()
            .map_err(|_| SecretsError::Crypto("nonce must be 12 bytes".into()))?;
        let pt = self
            .cipher
            .decrypt(&Nonce::from(nonce), ciphertext)
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
