//! Per-render credentials.
//!
//! A render token is NOT a session. It authorises reading exactly one object,
//! exactly once, for about a minute, as exactly one user. That envelope is what
//! separates a screenshot service from a data-exfiltration service: compromise
//! the renderer pod and what leaks is one object, briefly, that its owner could
//! already read.

use std::time::Duration;

use egret_metastore::Db;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AutomationError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderGrant {
    pub kind: String,
    pub object_id: Uuid,
    pub owner_id: Uuid,
}

#[derive(Clone)]
pub struct RenderTokenRepo {
    db: Db,
}

fn hash(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    hex::encode(h.finalize())
}

impl RenderTokenRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Mint a token and return it in CLEARTEXT — the only moment it exists in
    /// readable form. Only its hash reaches the database.
    pub async fn mint(
        &self,
        kind: &str,
        object_id: Uuid,
        owner: Uuid,
        ttl: Duration,
    ) -> Result<String, AutomationError> {
        let token = format!("rt_{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let expires_at = chrono::Utc::now()
            + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::seconds(60));
        sqlx::query(
            "INSERT INTO automation.render_tokens (token_hash, kind, object_id, owner_id, expires_at) \
             VALUES ($1,$2,$3,$4,$5)",
        )
        .bind(hash(&token))
        .bind(kind)
        .bind(object_id)
        .bind(owner)
        .bind(expires_at)
        .execute(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;
        Ok(token)
    }

    /// Read the grant WITHOUT spending the token.
    ///
    /// The render is two steps — the page asks what the object is, computes the
    /// dashboard's parameter filters with the same code the interactive
    /// dashboard uses, then asks for the rows — so the first step cannot be the
    /// one that burns the credential. What this step returns is metadata: an
    /// object's title and its charts' saved specs. The ROWS still cost the
    /// token, which is where the envelope in this module's doc comment
    /// actually bites.
    pub async fn peek(&self, token: &str) -> Result<RenderGrant, AutomationError> {
        let row: Option<(String, Uuid, Uuid)> = sqlx::query_as(
            "SELECT kind, object_id, owner_id FROM automation.render_tokens \
             WHERE token_hash = $1 AND expires_at > now()",
        )
        .bind(hash(token))
        .fetch_optional(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;

        row.map(|(kind, object_id, owner_id)| RenderGrant {
            kind,
            object_id,
            owner_id,
        })
        .ok_or_else(|| AutomationError::NotFound("render token".into()))
    }

    /// Redeem once. The row is DELETED by the same statement that reads it, so
    /// two concurrent redemptions cannot both succeed — a `used_at` flag would
    /// leave exactly that race open.
    pub async fn redeem(&self, token: &str) -> Result<RenderGrant, AutomationError> {
        let row: Option<(String, Uuid, Uuid)> = sqlx::query_as(
            "DELETE FROM automation.render_tokens \
             WHERE token_hash = $1 AND expires_at > now() \
             RETURNING kind, object_id, owner_id",
        )
        .bind(hash(token))
        .fetch_optional(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;

        row.map(|(kind, object_id, owner_id)| RenderGrant {
            kind,
            object_id,
            owner_id,
        })
        .ok_or_else(|| AutomationError::NotFound("render token".into()))
    }

    pub async fn prune_expired(&self) -> Result<u64, AutomationError> {
        let done = sqlx::query("DELETE FROM automation.render_tokens WHERE expires_at <= now()")
            .execute(&self.db.pool)
            .await
            .map_err(AutomationError::from_db)?;
        Ok(done.rows_affected())
    }
}
