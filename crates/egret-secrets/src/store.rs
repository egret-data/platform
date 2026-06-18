//! Postgres-backed store for AES-256-GCM-sealed secrets.

use egret_metastore::Db;
use uuid::Uuid;

use crate::crypto::Crypto;
use crate::error::SecretsError;

/// Stores encrypted secrets. `crypto` is `None` when no master key is configured —
/// any encrypted operation then fails with `NotConfigured`.
#[derive(Clone)]
pub struct SecretStore {
    db: Db,
    crypto: Option<Crypto>,
    key_id: String,
}

impl SecretStore {
    pub fn new(db: Db, crypto: Option<Crypto>) -> Self {
        Self {
            db,
            crypto,
            key_id: "cfg".into(),
        }
    }

    /// Whether a master key is configured (registration with a secret needs this).
    pub fn is_configured(&self) -> bool {
        self.crypto.is_some()
    }

    /// Clone the underlying DB handle (used by callers that need their own store).
    pub fn db_handle(&self) -> Db {
        self.db.clone()
    }

    fn crypto(&self) -> Result<&Crypto, SecretsError> {
        self.crypto.as_ref().ok_or(SecretsError::NotConfigured)
    }

    /// Seal `plaintext` and insert a new secret row; returns its id.
    pub async fn put(&self, kind: &str, plaintext: &str) -> Result<Uuid, SecretsError> {
        let (nonce, ct) = self.crypto()?.seal(plaintext)?;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO secrets (kind, nonce, ciphertext, key_id) \
             VALUES ($1, $2, $3, $4) RETURNING id",
        )
        .bind(kind)
        .bind(nonce)
        .bind(ct)
        .bind(&self.key_id)
        .fetch_one(&self.db.pool)
        .await?;
        Ok(id)
    }

    /// Fetch and decrypt a secret by id.
    pub async fn get(&self, id: Uuid) -> Result<String, SecretsError> {
        let row: Option<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT nonce, ciphertext FROM secrets WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?;
        let (nonce, ct) = row.ok_or(SecretsError::NotFound)?;
        self.crypto()?.open(&nonce, &ct)
    }

    /// Delete a secret by id (idempotent).
    pub async fn delete(&self, id: Uuid) -> Result<(), SecretsError> {
        sqlx::query("DELETE FROM secrets WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }
}
