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
use uuid::Uuid;

/// Data API tokens are short-lived: a stateless token cannot be revoked before
/// it expires, so the exposure window after revoking a client is this long.
pub const DATA_API_TTL_SECS: u64 = 600;

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
        expires_at: Option<DateTime<Utc>>,
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
        .bind(expires_at)
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

/// `(id, service_account_id, workspace_id, client_secret_hash, expires_at,
/// revoked_at, users.status, workspaces.status)` — the row before any refusal.
type RawClientRow = (
    Uuid,
    Uuid,
    Uuid,
    String,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    String,
    String,
);

/// A client that may mint right now, as far as anything but its secret can
/// tell. Only [`ClientStore::minting_row`] constructs one.
struct MintingRow {
    id: Uuid,
    service_account_id: Uuid,
    workspace_id: Uuid,
    secret_hash: String,
}

impl ClientStore {
    /// The ONE place that decides whether a client may mint, short of checking
    /// its secret. `verify` and `current` both go through it, so the secret
    /// path and the secret-free path cannot drift apart: a refusal added here
    /// binds both, and there is nowhere else to add one.
    ///
    /// The `users.status` check is what makes the documented kill switch real.
    /// A stateless token cannot be recalled before it expires, so spec §6.1
    /// tells an operator that the way to stop a compromised client INSIDE the
    /// 600s window is to disable the principal. That instruction is only true if
    /// authentication actually reads the principal's status — without this join
    /// a disabled service account would keep minting `data-api` tokens
    /// indefinitely, and the operator would believe they had stopped it.
    ///
    /// `workspaces.status` is read for the same reason, and its absence was
    /// worse: archiving a workspace is the operator's natural decommission
    /// action, and every management route runs through `WsContext`, which 404s
    /// on a non-active workspace. So archiving REMOVED every route that could
    /// revoke a client while leaving the client itself minting tokens — the
    /// credential outliving the only surface that could kill it. Authentication
    /// has to honour the workspace's status itself, because by then nothing else
    /// can.
    async fn minting_row(
        &self,
        client_id: &str,
    ) -> std::result::Result<Option<MintingRow>, egret_machine_idp::IdpError> {
        let row: Option<RawClientRow> = sqlx::query_as(
            "SELECT c.id, c.service_account_id, sa.workspace_id, c.client_secret_hash, \
                        c.expires_at, c.revoked_at, u.status, w.status \
                 FROM dataapi.clients c \
                 JOIN dataapi.service_accounts sa ON sa.user_id = c.service_account_id \
                 JOIN users u ON u.id = sa.user_id \
                 JOIN workspaces w ON w.id = sa.workspace_id \
                 WHERE c.client_id = $1",
        )
        .bind(client_id)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(|e| egret_machine_idp::IdpError::Db(e.to_string()))?;

        let Some((id, sa, ws, hash, expires_at, revoked_at, status, ws_status)) = row else {
            return Ok(None);
        };
        // Checked here rather than in the WHERE clause so they sit together:
        // one place shows every reason a client may not mint, and all of them
        // say it the same way.
        if revoked_at.is_some() {
            return Ok(None);
        }
        if status != "active" {
            return Ok(None);
        }
        if ws_status != "active" {
            return Ok(None);
        }
        if let Some(exp) = expires_at {
            if exp <= Utc::now() {
                return Ok(None);
            }
        }
        Ok(Some(MintingRow {
            id,
            service_account_id: sa,
            workspace_id: ws,
            secret_hash: hash,
        }))
    }
}

/// The token shape a Data API client gets.
fn dataapi_verified_client(client_id: &str, row: &MintingRow) -> egret_machine_idp::VerifiedClient {
    let extra = serde_json::json!({
        "ws": row.workspace_id.to_string(),
        "egret_user_id": row.service_account_id.to_string(),
    })
    .as_object()
    .cloned()
    .unwrap_or_default();
    egret_machine_idp::VerifiedClient {
        client_id: client_id.to_string(),
        // The const, not the literal: this is the MINT side and
        // `serving::verify_serving_token` is the VERIFY side, and a token
        // whose audience the verifier does not recognise is refused with no
        // hint as to why. One definition means the two cannot drift apart.
        audience: crate::serving::DATA_API_AUDIENCE.into(),
        extra_claims: extra,
        ttl_secs: DATA_API_TTL_SECS,
    }
}

#[async_trait::async_trait]
impl egret_machine_idp::ClientVerifier for ClientStore {
    /// Returns `Ok(None)` for EVERY failure — unknown, revoked, expired,
    /// disabled principal, archived workspace, wrong secret. A distinguishing
    /// error here would be a free oracle for anyone probing client ids. Every
    /// reason but the secret lives in [`ClientStore::minting_row`].
    async fn verify(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> std::result::Result<Option<egret_machine_idp::VerifiedClient>, egret_machine_idp::IdpError>
    {
        let Some(row) = self.minting_row(client_id).await? else {
            return Ok(None);
        };
        let parsed = PasswordHash::new(&row.secret_hash)
            .map_err(|e| egret_machine_idp::IdpError::Db(format!("hash parse: {e}")))?;
        if Argon2::default()
            .verify_password(client_secret.as_bytes(), &parsed)
            .is_err()
        {
            return Ok(None);
        }
        let _ = sqlx::query("UPDATE dataapi.clients SET last_used_at = now() WHERE id = $1")
            .bind(row.id)
            .execute(&self.db.pool)
            .await;
        Ok(Some(dataapi_verified_client(client_id, &row)))
    }

    /// Same refusals as `verify` — by construction, through
    /// [`ClientStore::minting_row`] — minus the secret check.
    async fn current(
        &self,
        client_id: &str,
    ) -> std::result::Result<Option<egret_machine_idp::ClientState>, egret_machine_idp::IdpError>
    {
        Ok(self
            .minting_row(client_id)
            .await?
            .map(|row| egret_machine_idp::ClientState {
                verified: dataapi_verified_client(client_id, &row),
                secret_hash: row.secret_hash,
            }))
    }
}
