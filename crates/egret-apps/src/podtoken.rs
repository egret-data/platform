//! Pod tokens (spec §5.3): the bearer an app pod presents to egret's pod API.
//!
//! An edit pod's token is bound to (user, workspace); a run pod's to one
//! (app, version). Only the SHA-256 is stored. egret mints the token when it
//! launches the pod and revokes it when the pod stops.

use egret_core::{token, Error, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use sqlx::Row;
use uuid::Uuid;

pub const POD_TOKEN_PREFIX: &str = "egp_";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PodKind {
    Edit,
    Run,
}

impl PodKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Edit => "edit",
            Self::Run => "run",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "edit" => Ok(Self::Edit),
            "run" => Ok(Self::Run),
            other => Err(anyhow::anyhow!("unknown pod token kind {other}").into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PodPrincipal {
    pub id: Uuid,
    pub kind: PodKind,
    /// `Some` exactly for an edit token.
    pub user_id: Option<UserId>,
    pub workspace_id: WorkspaceId,
    /// `Some` exactly for a run token.
    pub app_id: Option<Uuid>,
    pub version_id: Option<Uuid>,
}

#[derive(Clone)]
pub struct PodTokenRepo {
    db: Db,
}

impl PodTokenRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Returns the plaintext exactly once.
    pub async fn mint_edit(&self, user: UserId, ws: WorkspaceId) -> Result<String> {
        self.insert(PodKind::Edit, Some(user.0), ws, None, None)
            .await
    }

    /// The (app, version) pair is enforced by a composite FK, so a version of
    /// another app is refused by the schema, not only by this function.
    pub async fn mint_run(&self, ws: WorkspaceId, app: Uuid, version: Uuid) -> Result<String> {
        self.insert(PodKind::Run, None, ws, Some(app), Some(version))
            .await
    }

    async fn insert(
        &self,
        kind: PodKind,
        user: Option<Uuid>,
        ws: WorkspaceId,
        app: Option<Uuid>,
        version: Option<Uuid>,
    ) -> Result<String> {
        let plaintext = format!("{POD_TOKEN_PREFIX}{}", token::random_token_b64());
        sqlx::query(
            "INSERT INTO analytics.app_pod_tokens \
               (token_hash, kind, user_id, workspace_id, app_id, version_id) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(token::hash_token(&plaintext))
        .bind(kind.as_str())
        .bind(user)
        .bind(ws.0)
        .bind(app)
        .bind(version)
        .execute(&self.db.pool)
        .await
        .map_err(|e| Error::from(anyhow::Error::from(e)))?;
        Ok(plaintext)
    }

    /// `None` for an unknown, foreign-prefixed or revoked token.
    pub async fn resolve(&self, plain: &str) -> Result<Option<PodPrincipal>> {
        if !plain.starts_with(POD_TOKEN_PREFIX) {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT id, kind, user_id, workspace_id, app_id, version_id \
               FROM analytics.app_pod_tokens \
              WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(token::hash_token(plain))
        .fetch_optional(&self.db.pool)
        .await
        .map_err(|e| Error::from(anyhow::Error::from(e)))?;
        row.map(|r| {
            Ok(PodPrincipal {
                id: r.get("id"),
                kind: PodKind::parse(r.get::<&str, _>("kind"))?,
                user_id: r.get::<Option<Uuid>, _>("user_id").map(UserId),
                workspace_id: WorkspaceId(r.get("workspace_id")),
                app_id: r.get("app_id"),
                version_id: r.get("version_id"),
            })
        })
        .transpose()
    }

    pub async fn revoke_for_pod(&self, id: Uuid) -> Result<()> {
        sqlx::query(
            "UPDATE analytics.app_pod_tokens SET revoked_at = now() WHERE id = $1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&self.db.pool)
        .await
        .map_err(|e| Error::from(anyhow::Error::from(e)))?;
        Ok(())
    }

    /// Every live edit token of `user` in `ws`: what stopping that author's pod revokes, so a
    /// pod that is gone cannot be impersonated by a copy of its token. Returns how many.
    pub async fn revoke_edit_for(&self, user: UserId, ws: WorkspaceId) -> Result<u64> {
        let r = sqlx::query(
            "UPDATE analytics.app_pod_tokens SET revoked_at = now() \
              WHERE kind = 'edit' AND user_id = $1 AND workspace_id = $2 AND revoked_at IS NULL",
        )
        .bind(user.0)
        .bind(ws.0)
        .execute(&self.db.pool)
        .await
        .map_err(|e| Error::from(anyhow::Error::from(e)))?;
        Ok(r.rows_affected())
    }
}
