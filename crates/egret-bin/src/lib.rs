//! Library surface of the `egret` binary crate: composition-root helpers that
//! need their own unit/integration tests (e.g. the machine-IdP engine-token minter).
pub mod engine_minter;
pub mod identity;
pub mod workflow_secrets;

use anyhow::Context;
use egret_auth::groups::GroupRepo;
use egret_auth::users::UserRepo;
use egret_metastore::Db;
use egret_workspace::members::MemberRepo;
use egret_workspace::workspaces::WorkspaceRepo;

/// Grant an IdP (OIDC) group access to a workspace, idempotently. Resolves the
/// workspace by slug and the `added_by` admin by email (erroring if either is
/// missing, before any write), upserts the group by name, then records a group
/// workspace-membership with `role`.
pub async fn grant_group_to_workspace(
    db: &Db,
    workspace_slug: &str,
    group_name: &str,
    role: &str,
    admin_email: &str,
) -> anyhow::Result<()> {
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

    let group_id = GroupRepo::new(db.clone())
        .upsert_oidc_group(group_name)
        .await
        .context("upserting group")?;

    MemberRepo::new(db.clone())
        .add(ws.id, "group", group_id, role, admin.id)
        .await
        .context("adding group workspace-membership")?;
    Ok(())
}
