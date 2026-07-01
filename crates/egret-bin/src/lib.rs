//! Library surface of the `egret` binary crate: composition-root helpers that
//! need their own unit/integration tests (e.g. the Keycloak engine-token broker).
pub mod engine_broker;

use anyhow::Context;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use egret_workspace::workspaces::WorkspaceRepo;
use uuid::Uuid;

/// Grant an IdP (OIDC) group access to a workspace, idempotently. Upserts the
/// group by name, resolves the workspace by slug and the `added_by` admin by
/// email, then records a group workspace-membership with `role`.
pub async fn grant_group_to_workspace(
    db: &Db,
    workspace_slug: &str,
    group_name: &str,
    role: &str,
    admin_email: &str,
) -> anyhow::Result<()> {
    let group_id: Uuid = sqlx::query_scalar(
        "INSERT INTO groups (name, source) VALUES ($1, 'oidc') \
         ON CONFLICT (name) DO UPDATE SET name = groups.name RETURNING id",
    )
    .bind(group_name)
    .fetch_one(&db.pool)
    .await
    .context("upserting group")?;

    let ws = WorkspaceRepo::new(db.clone())
        .get_by_slug(workspace_slug)
        .await
        .context("looking up workspace")?
        .ok_or_else(|| anyhow::anyhow!("workspace not found: {workspace_slug}"))?;

    let admin = UserRepo::new(db.clone())
        .find_by_email(admin_email)
        .await
        .context("looking up admin")?
        .ok_or_else(|| anyhow::anyhow!("admin user not found: {admin_email}"))?;

    MemberRepo::new(db.clone())
        .add(ws.id, "group", group_id, role, admin.id)
        .await
        .context("adding group workspace-membership")?;
    Ok(())
}
