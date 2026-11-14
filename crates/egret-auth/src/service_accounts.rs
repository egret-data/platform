//! Service accounts as principals: the platform-level `service_accounts` table
//! (migration 0229) joined to the `users` row that makes each one a real
//! principal. A service account is either workspace-scoped (bound to one
//! workspace) or platform-scoped (no workspace).
//!
//! Writes go to the BASE table `service_accounts`, never to the
//! `dataapi.service_accounts` view over it.
//!
//! Disable/enable here only flip `users.status`. Session/token revocation and
//! the recompile that a status change forces (`egret_perm_compile`, which
//! depends on this crate) belong to the route layer.

use chrono::{DateTime, Utc};
use egret_core::{Error, Result, UserId};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::users::UserRepo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SaScope {
    Platform,
    Workspace,
}

impl SaScope {
    fn as_str(self) -> &'static str {
        match self {
            SaScope::Platform => "platform",
            SaScope::Workspace => "workspace",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        match s {
            "platform" => Ok(SaScope::Platform),
            "workspace" => Ok(SaScope::Workspace),
            other => Err(Error::Internal(anyhow::anyhow!(
                "unknown service account scope {other:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceAccount {
    pub user_id: Uuid,
    pub scope: SaScope,
    pub workspace_id: Option<Uuid>,
    pub name: String,
    pub description: String,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    /// Mirrors `users.status`.
    pub status: String,
}

pub struct ServiceAccountRepo {
    pool: PgPool,
}

const SELECT: &str = "SELECT sa.user_id, sa.scope, sa.workspace_id, sa.name, sa.description, \
     sa.created_by, sa.created_at, u.status \
     FROM service_accounts sa JOIN users u ON u.id = sa.user_id";

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!(e))
}

/// Same slug rule as the Data API create path, so the synthetic email of a
/// workspace service account is identical whichever path created it.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_dash = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn row_to_sa(r: &sqlx::postgres::PgRow) -> Result<ServiceAccount> {
    let scope: String = r.try_get("scope").map_err(db_err)?;
    Ok(ServiceAccount {
        user_id: r.try_get("user_id").map_err(db_err)?,
        scope: SaScope::parse(&scope)?,
        workspace_id: r.try_get("workspace_id").map_err(db_err)?,
        name: r.try_get("name").map_err(db_err)?,
        description: r.try_get("description").map_err(db_err)?,
        created_by: r.try_get("created_by").map_err(db_err)?,
        created_at: r.try_get("created_at").map_err(db_err)?,
        status: r.try_get("status").map_err(db_err)?,
    })
}

impl ServiceAccountRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create the `users` row and the `service_accounts` row in one
    /// transaction. `(scope, workspace_id)` must agree: platform without a
    /// workspace, workspace with one.
    pub async fn create(
        &self,
        scope: SaScope,
        workspace_id: Option<Uuid>,
        name: &str,
        description: &str,
        created_by: Uuid,
    ) -> Result<ServiceAccount> {
        match (scope, workspace_id) {
            (SaScope::Platform, Some(_)) => {
                return Err(Error::InvalidInput(
                    "a platform service account cannot belong to a workspace".into(),
                ))
            }
            (SaScope::Workspace, None) => {
                return Err(Error::InvalidInput(
                    "a workspace service account needs a workspace".into(),
                ))
            }
            _ => {}
        }
        let slug = slugify(name);
        if slug.is_empty() {
            return Err(Error::InvalidInput(
                "name must contain at least one alphanumeric character".into(),
            ));
        }

        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let email = match workspace_id {
            Some(ws) => {
                let ws_slug: Option<String> =
                    sqlx::query_scalar("SELECT slug FROM workspaces WHERE id = $1")
                        .bind(ws)
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(db_err)?;
                let ws_slug = ws_slug.ok_or_else(|| Error::NotFound(format!("workspace {ws}")))?;
                format!("svc.{slug}@{ws_slug}.service.egret.local")
            }
            None => format!("svc-{slug}@service.egret.local"),
        };

        let conflict = || Error::Conflict(format!("service account already exists: {name}"));
        let user_id = UserRepo::insert_service_with(&mut *tx, &email, name)
            .await
            .map_err(|e| match e {
                Error::Conflict(_) => conflict(),
                other => other,
            })?;
        sqlx::query(
            "INSERT INTO service_accounts (user_id, scope, workspace_id, name, description, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(user_id.0)
        .bind(scope.as_str())
        .bind(workspace_id)
        .bind(name)
        .bind(description)
        .bind(created_by)
        .execute(&mut *tx)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => conflict(),
            _ => db_err(e),
        })?;
        let sa = sqlx::query(sqlx::AssertSqlSafe(format!(
            "{SELECT} WHERE sa.user_id = $1"
        )))
        .bind(user_id.0)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)
        .and_then(|r| row_to_sa(&r))?;
        tx.commit().await.map_err(db_err)?;
        Ok(sa)
    }

    /// Newest first, optionally filtered by scope and/or workspace.
    pub async fn list(
        &self,
        scope: Option<SaScope>,
        workspace_id: Option<Uuid>,
    ) -> Result<Vec<ServiceAccount>> {
        let sql = format!(
            "{SELECT} WHERE ($1::text IS NULL OR sa.scope = $1) \
             AND ($2::uuid IS NULL OR sa.workspace_id = $2) \
             ORDER BY sa.created_at DESC, sa.user_id"
        );
        let rows = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(scope.map(SaScope::as_str))
            .bind(workspace_id)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        rows.iter().map(row_to_sa).collect()
    }

    pub async fn get(&self, user_id: Uuid) -> Result<Option<ServiceAccount>> {
        let sql = format!("{SELECT} WHERE sa.user_id = $1");
        let row = sqlx::query(sqlx::AssertSqlSafe(sql))
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.as_ref().map(row_to_sa).transpose()
    }

    /// Delete the `users` row; the `service_accounts` row goes by cascade.
    pub async fn delete(&self, user_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM users WHERE id = $1 AND provider = 'service'")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn disable(&self, user_id: Uuid) -> Result<()> {
        self.set_status(user_id, "disabled").await
    }

    pub async fn enable(&self, user_id: Uuid) -> Result<()> {
        self.set_status(user_id, "active").await
    }

    async fn set_status(&self, user_id: Uuid, status: &str) -> Result<()> {
        let mut conn = self.pool.acquire().await.map_err(db_err)?;
        UserRepo::set_status_in_tx(&mut conn, UserId(user_id), status).await
    }
}
