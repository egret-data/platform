//! Opaque token primitives (feature = "token").
//!
//! Mint a random url-safe opaque token and hash a token for at-rest storage.
//! Shared so the hashing algorithm is defined **once** — session tokens
//! (`egret-auth`) and Data Apps gateway/session tokens (`egret-apps`) both
//! delegate here, keeping their stored hashes byte-identical. Consumers that
//! need a different wire format for the plaintext (e.g. an `egk_` prefix)
//! keep their own minting and reuse [`hash_token`].

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};

/// Mint a fresh 256-bit url-safe opaque token (base64, no padding).
pub fn random_token_b64() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// SHA-256 hex of a token, for at-rest storage and lookup.
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique() {
        assert_ne!(random_token_b64(), random_token_b64());
    }

    #[test]
    fn hash_is_deterministic_and_64_hex() {
        let t = random_token_b64();
        assert_eq!(hash_token(&t), hash_token(&t));
        assert_eq!(hash_token(&t).len(), 64); // sha-256 hex
    }

    #[test]
    fn hash_matches_known_sha256_vector() {
        // Pins the exact algorithm (sha256 hex of the UTF-8 bytes) so the crates
        // that delegate here keep hashes byte-identical to their old inline impl.
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
