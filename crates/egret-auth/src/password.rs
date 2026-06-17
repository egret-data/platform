//! argon2id password hashing.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use egret_core::{Error, Result};

/// Hash a plaintext password with argon2id (PHC string output).
pub fn hash_password(plain: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|e| Error::Internal(anyhow::anyhow!("argon2 hash failed: {e}")))?;
    Ok(hash.to_string())
}

/// Verify a plaintext password against a stored PHC hash. Returns Ok(false) on mismatch.
pub fn verify_password(plain: &str, phc_hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(phc_hash)
        .map_err(|e| Error::Internal(anyhow::anyhow!("invalid password hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_argon2id_and_verifies() {
        let h = hash_password("s3cret!").unwrap();
        assert!(
            h.starts_with("$argon2id$"),
            "expected argon2id PHC, got {h}"
        );
        assert!(verify_password("s3cret!", &h).unwrap());
    }

    #[test]
    fn wrong_password_does_not_verify() {
        let h = hash_password("s3cret!").unwrap();
        assert!(!verify_password("wrong", &h).unwrap());
    }

    #[test]
    fn two_hashes_of_same_password_differ() {
        assert_ne!(hash_password("x").unwrap(), hash_password("x").unwrap());
    }
}
