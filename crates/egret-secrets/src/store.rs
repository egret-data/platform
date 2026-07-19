//! Postgres-backed store for AES-256-GCM-sealed secrets.

use std::collections::BTreeMap;

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

/// A typed, multi-field secret: a k8s-style `type` plus a plaintext `data` map.
/// Returned only on read; values are never logged.
pub struct TypedSecret {
    pub r#type: String,
    pub data: BTreeMap<String, String>,
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

    /// Create a typed secret: validate, insert the `secrets` row, seal + insert one
    /// `secret_data` row per key (atomically). Returns the new id.
    pub async fn put_typed(
        &self,
        kind: &str,
        r#type: &str,
        data: &BTreeMap<String, String>,
    ) -> Result<Uuid, SecretsError> {
        crate::secret_type::validate(r#type, data)?;
        let crypto = self.crypto()?;
        let mut tx = self.db.pool.begin().await?;
        let id: Uuid =
            sqlx::query_scalar("INSERT INTO secrets (kind, type) VALUES ($1, $2) RETURNING id")
                .bind(kind)
                .bind(r#type)
                .fetch_one(&mut *tx)
                .await?;
        for (key, value) in data {
            let (nonce, ct) = crypto.seal(value)?;
            sqlx::query(
                "INSERT INTO secret_data (secret_id, key, nonce, ciphertext, key_id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(key)
            .bind(nonce)
            .bind(ct)
            .bind(&self.key_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    /// Fetch + decrypt a typed secret (its type and full data map).
    pub async fn get_typed(&self, id: Uuid) -> Result<TypedSecret, SecretsError> {
        let r#type: String = sqlx::query_scalar("SELECT type FROM secrets WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?
            .ok_or(SecretsError::NotFound)?;
        let rows: Vec<(String, Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT key, nonce, ciphertext FROM secret_data WHERE secret_id = $1")
                .bind(id)
                .fetch_all(&self.db.pool)
                .await?;
        let crypto = self.crypto()?;
        let mut data = BTreeMap::new();
        for (key, nonce, ct) in rows {
            data.insert(key, crypto.open(&nonce, &ct)?);
        }
        Ok(TypedSecret { r#type, data })
    }

    /// Fetch + decrypt a single key of a secret.
    pub async fn get_key(&self, id: Uuid, key: &str) -> Result<String, SecretsError> {
        let row: Option<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
            "SELECT nonce, ciphertext FROM secret_data WHERE secret_id = $1 AND key = $2",
        )
        .bind(id)
        .bind(key)
        .fetch_optional(&self.db.pool)
        .await?;
        let (nonce, ct) = row.ok_or(SecretsError::NotFound)?;
        self.crypto()?.open(&nonce, &ct)
    }

    /// Replace a typed secret's type + data map (re-validated, atomically).
    pub async fn update_typed(
        &self,
        id: Uuid,
        r#type: &str,
        data: &BTreeMap<String, String>,
    ) -> Result<(), SecretsError> {
        crate::secret_type::validate(r#type, data)?;
        let crypto = self.crypto()?;
        let mut tx = self.db.pool.begin().await?;
        let n = sqlx::query("UPDATE secrets SET type = $2 WHERE id = $1")
            .bind(id)
            .bind(r#type)
            .execute(&mut *tx)
            .await?;
        if n.rows_affected() == 0 {
            return Err(SecretsError::NotFound);
        }
        sqlx::query("DELETE FROM secret_data WHERE secret_id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        for (key, value) in data {
            let (nonce, ct) = crypto.seal(value)?;
            sqlx::query(
                "INSERT INTO secret_data (secret_id, key, nonce, ciphertext, key_id) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(key)
            .bind(nonce)
            .bind(ct)
            .bind(&self.key_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Backward-compat: create an `Opaque` secret with a single `value` key.
    pub async fn put(&self, kind: &str, plaintext: &str) -> Result<Uuid, SecretsError> {
        let mut data = BTreeMap::new();
        data.insert("value".to_string(), plaintext.to_string());
        self.put_typed(kind, crate::secret_type::OPAQUE, &data)
            .await
    }

    /// Backward-compat: re-seal the `value` key (preserves the row + type).
    pub async fn update(&self, id: Uuid, plaintext: &str) -> Result<(), SecretsError> {
        let exists: Option<Uuid> = sqlx::query_scalar("SELECT id FROM secrets WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.db.pool)
            .await?;
        if exists.is_none() {
            return Err(SecretsError::NotFound);
        }
        let (nonce, ct) = self.crypto()?.seal(plaintext)?;
        sqlx::query(
            "INSERT INTO secret_data (secret_id, key, nonce, ciphertext, key_id) \
             VALUES ($1, 'value', $2, $3, $4) \
             ON CONFLICT (secret_id, key) DO UPDATE \
             SET nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext, key_id = EXCLUDED.key_id",
        )
        .bind(id)
        .bind(nonce)
        .bind(ct)
        .bind(&self.key_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Backward-compat single-value read: decrypt the `value` key, falling back to
    /// `password` for a `kubernetes.io/basic-auth` secret (P1b). This lets every
    /// legacy single-value reader (`resolve_plaintext` → `get`) transparently read
    /// the password of a connection credential that has been migrated to basic-auth,
    /// without changing each call site.
    pub async fn get(&self, id: Uuid) -> Result<String, SecretsError> {
        match self.get_key(id, "value").await {
            Err(SecretsError::NotFound) => self.get_key(id, "password").await,
            other => other,
        }
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
