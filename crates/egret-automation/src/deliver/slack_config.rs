//! Per-workspace Slack, mirroring [`super::smtp_config`].
//!
//! A Slack app belongs to the workspace, not to each destination. Before this
//! every slack recipient carried its own bot token, so posting to three
//! channels meant pasting the same credential three times, rotating it in three
//! places, and leaving three vault rows to leak.
//!
//! The token is never a column and never leaves this module in plaintext except
//! through [`SlackConfigRepo::resolve_token`], called at delivery time.

use egret_metastore::Db;
use egret_secrets::SecretStore;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::AutomationError;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SlackConfig {
    pub workspace_id: Uuid,
    pub token_secret_id: Uuid,
    pub default_channel: Option<String>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// What a client may see. There is no token field — not a redacted one, an
/// ABSENT one, so no future serializer can start including it by accident.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct SlackConfigDto {
    pub default_channel: Option<String>,
    /// Always true when a row exists: `token_secret_id` is NOT NULL. It is
    /// reported anyway so the editor's "a token is stored" line reads the same
    /// field on both credential paths — see `redact_recipient_config`.
    pub has_token: bool,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct SlackConfigBody {
    /// Three-way, like SMTP's password: absent keeps the stored token, `""` is
    /// rejected (a config with no token cannot deliver, so there is nothing
    /// sensible to store), a value replaces it. Required on first write.
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub default_channel: Option<String>,
}

pub fn to_dto(c: &SlackConfig) -> SlackConfigDto {
    SlackConfigDto {
        default_channel: c.default_channel.clone(),
        has_token: true,
        updated_at: c.updated_at,
    }
}

#[derive(Clone)]
pub struct SlackConfigRepo {
    db: Db,
    secrets: SecretStore,
}

const COLS: &str = "workspace_id, token_secret_id, default_channel, updated_at";

/// The vault `kind`, so a Slack token is identifiable in the store. Distinct
/// from the per-recipient kind that predates this table, so the two eras are
/// tellable apart during the migration window.
const SECRET_KIND: &str = "automation_slack_workspace_token";

impl SlackConfigRepo {
    pub fn new(db: Db, secrets: SecretStore) -> Self {
        Self { db, secrets }
    }

    pub async fn get(&self, ws: Uuid) -> Result<Option<SlackConfig>, AutomationError> {
        sqlx::query_as(&format!(
            "SELECT {COLS} FROM automation.slack_config WHERE workspace_id = $1"
        ))
        .bind(ws)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)
    }

    /// Upsert the workspace's Slack app.
    ///
    /// A blank token on a workspace that has none is refused rather than
    /// stored: the row's whole purpose is to hold a credential, and an empty
    /// one is a workspace that looks configured and silently cannot deliver.
    pub async fn upsert(
        &self,
        ws: Uuid,
        body: &SlackConfigBody,
        actor: Option<Uuid>,
    ) -> Result<SlackConfig, AutomationError> {
        let existing = self.get(ws).await?;
        let old_secret = existing.as_ref().map(|c| c.token_secret_id);

        let new_secret = match body.token.as_deref().map(str::trim) {
            Some(t) if !t.is_empty() => Some(
                self.secrets
                    .put(SECRET_KIND, t)
                    .await
                    .map_err(|e| AutomationError::BadRequest(e.to_string()))?,
            ),
            // Absent (or blank) keeps what is stored — the editor cannot render
            // a token, so its empty field must not mean "delete".
            _ => old_secret,
        };
        let Some(token_secret_id) = new_secret else {
            return Err(AutomationError::BadRequest(
                "a slack bot token is required".into(),
            ));
        };

        let channel = body
            .default_channel
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_string);

        let row: SlackConfig = sqlx::query_as(&format!(
            "INSERT INTO automation.slack_config \
               (workspace_id, token_secret_id, default_channel, updated_by, updated_at) \
             VALUES ($1,$2,$3,$4, now()) \
             ON CONFLICT (workspace_id) DO UPDATE SET \
               token_secret_id = EXCLUDED.token_secret_id, \
               default_channel = EXCLUDED.default_channel, \
               updated_by = EXCLUDED.updated_by, updated_at = now() \
             RETURNING {COLS}"
        ))
        .bind(ws)
        .bind(token_secret_id)
        .bind(&channel)
        .bind(actor)
        .fetch_one(&self.db.pool)
        .await
        .map_err(AutomationError::from_db)?;

        // Only once the row committed: if the write failed, the old token is
        // still the live one.
        if let (Some(old), true) = (old_secret, old_secret != new_secret) {
            let _ = self.secrets.delete(old).await;
        }
        Ok(row)
    }

    pub async fn delete(&self, ws: Uuid) -> Result<bool, AutomationError> {
        let existing = self.get(ws).await?;
        // The row first: `token_secret_id` is `ON DELETE RESTRICT`, so the
        // secret cannot go while anything still references it.
        let done = sqlx::query("DELETE FROM automation.slack_config WHERE workspace_id = $1")
            .bind(ws)
            .execute(&self.db.pool)
            .await
            .map_err(AutomationError::from_db)?;
        if let Some(c) = existing {
            let _ = self.secrets.delete(c.token_secret_id).await;
        }
        Ok(done.rows_affected() > 0)
    }

    /// The one place plaintext is produced, called at delivery time.
    pub async fn resolve_token(&self, ws: Uuid) -> Option<String> {
        let config = self.get(ws).await.ok().flatten()?;
        self.secrets.get(config.token_secret_id).await.ok()
    }
}
