//! Per-workspace feature entitlements: the code-defined catalog of modules, the
//! request-path matcher that says which module a request belongs to, and the
//! `WorkspaceRepo` queries that read and write the on/off rows.
//!
//! An **absent row means enabled**. Only explicit `enabled = false` rows turn a
//! module off, so a fresh install and every pre-existing workspace behave
//! exactly as they did before this feature existed.

use std::collections::HashSet;

use egret_core::{Error, Result, UserId, WorkspaceId};

use crate::workspaces::{db_err, WorkspaceRepo};

/// Every module key, in the order the UI lists them.
pub const FEATURE_KEYS: &[&str] = &["sql_lab", "datasets", "charts", "dashboards", "notebooks"];

/// True if `key` names a module in the catalog.
pub fn is_known(key: &str) -> bool {
    FEATURE_KEYS.contains(&key)
}

impl WorkspaceRepo {
    /// The module keys explicitly turned OFF for `ws`. Everything not in the
    /// returned set is on.
    pub async fn disabled_features(&self, ws: WorkspaceId) -> Result<HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT feature FROM workspace_features \
             WHERE workspace_id = $1 AND enabled = false",
        )
        .bind(ws.0)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(|(f,)| f).collect())
    }

    /// Upsert `updates` for `ws` in one transaction. Rejects unknown keys before
    /// writing anything, so a batch containing a typo changes nothing.
    pub async fn set_features(
        &self,
        ws: WorkspaceId,
        updates: &[(String, bool)],
        actor: Option<UserId>,
    ) -> Result<()> {
        for (key, _) in updates {
            if !is_known(key) {
                return Err(Error::InvalidInput(format!("unknown feature: {key}")));
            }
        }
        let mut tx = self.db.pool.begin().await.map_err(db_err)?;
        for (key, enabled) in updates {
            sqlx::query(
                "INSERT INTO workspace_features (workspace_id, feature, enabled, updated_by) \
                 VALUES ($1, $2, $3, $4) \
                 ON CONFLICT (workspace_id, feature) DO UPDATE \
                 SET enabled = EXCLUDED.enabled, \
                     updated_at = now(), \
                     updated_by = EXCLUDED.updated_by",
            )
            .bind(ws.0)
            .bind(key)
            .bind(enabled)
            .bind(actor.map(|u| u.0))
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        }
        tx.commit().await.map_err(db_err)?;
        Ok(())
    }
}
