//! Postgres-backed workspace CRUD.

use egret_core::{Error, Pagination, Result, UserId, WorkspaceId};
use egret_metastore::Db;
use uuid::Uuid;

use crate::model::{is_valid_slug, Workspace};

#[derive(Clone)]
pub struct WorkspaceRepo {
    db: Db,
}

impl WorkspaceRepo {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Insert a workspace row.
    ///
    /// Warehouse provisioning is **no longer done here** (CE-2 T4). The legacy
    /// A-2 path inserted a `catalogs` row in the same transaction; under the
    /// embedded-lakekeeper architecture the workspace's catalog is a lakekeeper
    /// *warehouse*, provisioned out-of-band by
    /// `egret_catalog_engine::provision_workspace_warehouse` from the
    /// server/handler layer (which holds the engine state + storage cfg +
    /// catalog store). Keeping that out of `WorkspaceRepo` avoids a circular
    /// crate dependency (egret-workspace must not depend on egret-catalog-engine)
    /// and keeps the warehouse round-trip (a live S3 `validate_access`) out of
    /// this DB transaction.
    pub async fn create(&self, slug: &str, name: &str, created_by: UserId) -> Result<Workspace> {
        if !is_valid_slug(slug) {
            return Err(Error::InvalidInput(format!("invalid slug: {slug}")));
        }

        let row = sqlx::query_as::<_, WorkspaceRow>(
            "INSERT INTO workspaces (slug, name, created_by) VALUES ($1, $2, $3) \
             RETURNING id, slug, name, status",
        )
        .bind(slug)
        .bind(name)
        .bind(created_by.0)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db) if db.is_unique_violation() => {
                Error::Conflict(format!("workspace slug already exists: {slug}"))
            }
            other => db_err(other),
        })?;

        Ok(row.into())
    }

    pub async fn list(&self, page: Pagination) -> Result<Vec<Workspace>> {
        let rows = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, slug, name, status FROM workspaces ORDER BY created_at DESC \
             LIMIT $1 OFFSET $2",
        )
        .bind(page.limit() as i64)
        .bind(page.offset() as i64)
        .fetch_all(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn get_by_slug(&self, slug: &str) -> Result<Option<Workspace>> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, slug, name, status FROM workspaces WHERE slug = $1",
        )
        .bind(slug)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }

    pub async fn get_by_id(&self, id: WorkspaceId) -> Result<Option<Workspace>> {
        let row = sqlx::query_as::<_, WorkspaceRow>(
            "SELECT id, slug, name, status FROM workspaces WHERE id = $1",
        )
        .bind(id.0)
        .fetch_optional(&self.db.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(Into::into))
    }

    pub async fn update_name(&self, id: WorkspaceId, name: &str) -> Result<()> {
        sqlx::query("UPDATE workspaces SET name = $1 WHERE id = $2")
            .bind(name)
            .bind(id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    pub async fn archive(&self, id: WorkspaceId) -> Result<()> {
        sqlx::query("UPDATE workspaces SET status = 'archived' WHERE id = $1")
            .bind(id.0)
            .execute(&self.db.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Workspaces the principal can access: all active if `include_all` (platform_admin),
    /// else those where the user is a member directly or via a group.
    pub async fn list_for_principal(
        &self,
        user_id: UserId,
        include_all: bool,
    ) -> Result<Vec<Workspace>> {
        let rows = if include_all {
            sqlx::query_as::<_, WorkspaceRow>(
                "SELECT id, slug, name, status FROM workspaces WHERE status = 'active' ORDER BY name",
            )
            .fetch_all(&self.db.pool)
            .await
        } else {
            sqlx::query_as::<_, WorkspaceRow>(
                "SELECT DISTINCT w.id, w.slug, w.name, w.status FROM workspaces w \
                 JOIN workspace_members wm ON wm.workspace_id = w.id \
                 WHERE w.status = 'active' \
                   AND ( (wm.principal_type = 'user'  AND wm.principal_id = $1) \
                      OR (wm.principal_type = 'group' AND wm.principal_id IN \
                             (SELECT group_id FROM user_groups WHERE user_id = $1)) ) \
                 ORDER BY w.name",
            )
            .bind(user_id.0)
            .fetch_all(&self.db.pool)
            .await
        };
        Ok(rows.map_err(db_err)?.into_iter().map(Into::into).collect())
    }
}

#[derive(sqlx::FromRow)]
struct WorkspaceRow {
    id: Uuid,
    slug: String,
    name: String,
    status: String,
}

impl From<WorkspaceRow> for Workspace {
    fn from(r: WorkspaceRow) -> Self {
        Workspace {
            id: WorkspaceId(r.id),
            slug: r.slug,
            name: r.name,
            status: r.status,
        }
    }
}

pub(crate) fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!(e))
}
