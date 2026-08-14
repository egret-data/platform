//! Data API OAuth2 clients: create, revoke, and the `ClientVerifier` the
//! machine-IdP calls at `POST /oauth/token`.
//!
//! Argon2 is affordable here because it runs once per TOKEN, not once per API
//! request — that is the whole reason this is an OAuth2 client and not a bearer
//! key checked on every call.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chrono::{DateTime, Utc};
use egret_core::{Error, Result, UserId};
use egret_metastore::Db;
use time::OffsetDateTime;
use uuid::Uuid;

/// Data API tokens are short-lived: a stateless token cannot be revoked before
/// it expires, so the exposure window after revoking a client is this long.
pub const DATA_API_TTL_SECS: u64 = 600;

/// `expires_at` is `time::OffsetDateTime` in the public API (matching the
/// `ClientVerifier` contract's expectations), but every other timestamp in this
/// workspace is `chrono::DateTime<Utc>` — sqlx's `chrono` feature is what the
/// whole tree (including the git-pinned lakekeeper crates) is built against, and
/// turning on sqlx's `time` feature alongside it makes `sqlx::query_as!`
/// ambiguous for every OTHER crate's TIMESTAMPTZ columns. So this crate binds
/// `chrono::DateTime<Utc>` to Postgres like everyone else and converts at the
/// boundary instead of asking sqlx to speak `time` at all.
fn offset_to_chrono(o: OffsetDateTime) -> DateTime<Utc> {
    DateTime::from_timestamp_nanos(o.unix_timestamp_nanos() as i64)
}

pub struct NewClient {
    pub id: Uuid,
    pub client_id: String,
    /// Shown once. Not recoverable.
    pub client_secret: String,
}

#[derive(Clone)]
pub struct ClientStore {
    db: Db,
}

impl ClientStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub async fn create(
        &self,
        service_account_id: Uuid,
        name: &str,
        expires_at: Option<OffsetDateTime>,
        by: UserId,
    ) -> Result<NewClient> {
        use base64::Engine as _;
        use rand::RngCore;

        let mut b = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut b);
        let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b);
        let client_id = format!("egret-dataapi-{}", Uuid::new_v4());

        let salt = SaltString::generate(&mut rand::thread_rng());
        let hash = Argon2::default()
            .hash_password(secret.as_bytes(), &salt)
            .map_err(|e| Error::Internal(anyhow::anyhow!("hash client secret: {e}")))?
            .to_string();

        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO dataapi.clients \
               (service_account_id, name, client_id, client_secret_hash, expires_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
        )
        .bind(service_account_id)
        .bind(name)
        .bind(&client_id)
        .bind(hash)
        .bind(expires_at.map(offset_to_chrono))
        .bind(by.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("create dataapi client: {e}")))?;

        Ok(NewClient {
            id,
            client_id,
            client_secret: secret,
        })
    }

    pub async fn revoke(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE dataapi.clients SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&self.db.pool)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("revoke dataapi client: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl egret_machine_idp::ClientVerifier for ClientStore {
    /// Returns `Ok(None)` for EVERY failure — unknown, revoked, expired, wrong
    /// secret. A distinguishing error here would be a free oracle for anyone
    /// probing client ids.
    async fn verify(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> std::result::Result<Option<egret_machine_idp::VerifiedClient>, egret_machine_idp::IdpError>
    {
        let row: Option<(
            Uuid,
            Uuid,
            Uuid,
            String,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
        )> = sqlx::query_as(
            "SELECT c.id, c.service_account_id, sa.workspace_id, c.client_secret_hash, \
                        c.expires_at, c.revoked_at \
                 FROM dataapi.clients c \
                 JOIN dataapi.service_accounts sa ON sa.user_id = c.service_account_id \
                 WHERE c.client_id = $1",
        )
        .bind(client_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(|e| egret_machine_idp::IdpError::Db(e.to_string()))?;

        let Some((id, sa, ws, hash, expires_at, revoked_at)) = row else {
            return Ok(None);
        };
        if revoked_at.is_some() {
            return Ok(None);
        }
        if let Some(exp) = expires_at {
            if exp <= Utc::now() {
                return Ok(None);
            }
        }
        let parsed = PasswordHash::new(&hash)
            .map_err(|e| egret_machine_idp::IdpError::Db(format!("hash parse: {e}")))?;
        if Argon2::default()
            .verify_password(client_secret.as_bytes(), &parsed)
            .is_err()
        {
            return Ok(None);
        }
        let _ = sqlx::query("UPDATE dataapi.clients SET last_used_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.db.pool)
            .await;

        let extra = serde_json::json!({ "ws": ws.to_string(), "egret_user_id": sa.to_string() })
            .as_object()
            .cloned()
            .unwrap_or_default();
        Ok(Some(egret_machine_idp::VerifiedClient {
            client_id: client_id.to_string(),
            audience: "data-api".into(),
            extra_claims: extra,
            ttl_secs: DATA_API_TTL_SECS,
        }))
    }
}
