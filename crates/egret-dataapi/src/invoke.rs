//! The invoke question, asked once.
//!
//! Three callers need to know whether a principal may invoke an endpoint: the
//! serving path (before it runs anything), the OpenAPI generator (before it
//! documents a path for a consumer), and the management UI's Access checklist
//! (before it draws a tick). They must agree, and the only way they can is by
//! being the same expression.
//!
//! # Why this cannot be a query against `access_grants`
//!
//! After migration 0124 the `ws_admin` role carries `data_api_endpoint:invoke`,
//! and the Cedar workspace->object cascade applies a workspace-scoped grant to
//! every object in it. A principal can therefore be fully able to invoke an
//! endpoint while owning no row in `access_grants` that mentions it. A scan
//! answers "no" there, the PDP answers "yes", and the serving path follows the
//! PDP — so a scan-based generator emits a document with paths silently missing,
//! and a scan-based checklist draws a cross next to an endpoint that works.

use crate::routes::DataApiState;
use egret_core::ids::UserId;
use egret_core::WorkspaceId;
use uuid::Uuid;

/// Whether `user_id` may invoke `endpoint_id` in `ws`.
///
/// `folder_id: None` — a folder-scoped grant does NOT cascade to invoke. This is
/// a conservative choice on a path that is now REACHABLE: endpoint folders ship
/// in this branch (create, and move a row between them), so the endpoint passed
/// here may well sit in one. Passing its folder anyway would mean "invoke
/// everything in this folder", which widens access and nobody has asked for it.
///
/// What that leaves open, measured rather than assumed:
///
/// * This crate only ever writes `scope_type = 'object'` grants (`routes.rs`,
///   the endpoint Access seam), so it creates no folder-scoped invoke grant of
///   its own and nothing here is contradicted by its own writes.
/// * `egret-bi`'s shares router DOES accept `kind = "folder"`, and its
///   `object_ref` resolves a folder straight out of `analytics.folders` with no
///   `entity_type` filter (`shares.rs`). `analytics.folders` is polymorphic, so
///   an admin can share a Data API endpoint FOLDER today — and because this
///   call passes `None`, that share will not cascade to invoke.
///
/// The result is a silently INEFFECTIVE grant, not an over-permission: the
/// admin is told nothing, and the grantee still cannot invoke. That is a
/// deferred product decision (either cascade it, or refuse the share), not a
/// bug to fix inside this function.
///
/// An error is an error, never a refusal. A PDP that could not answer is our
/// fault: returning `false` would send an operator to write a grant that already
/// exists.
pub(crate) async fn can_invoke(
    st: &DataApiState,
    user_id: UserId,
    ws: WorkspaceId,
    endpoint_id: Uuid,
) -> Result<bool, egret_core::Error> {
    Ok(st
        .pdp
        .authorize(
            &egret_core::authz::Subject {
                user_id,
                group_ids: Vec::new(),
            },
            crate::routes::EP_INVOKE,
            &egret_core::authz::Resource::Object(egret_core::authz::ObjectRef {
                kind: crate::endpoints::OBJECT_KIND.to_string(),
                id: endpoint_id,
                folder_id: None,
                workspace_id: ws,
            }),
        )
        .await?
        .allow)
}

/// Whether `user_id` reaches invoke on EVERY endpoint of `ws` through the
/// workspace — a membership whose role carries `data_api_endpoint:invoke`
/// (`ws_admin` after migration 0124, `data_api_invoker` as a membership, a
/// custom role), or anything above it — independently of any object-scoped
/// grant on one endpoint.
///
/// The Access tab's "Revoking will not remove access" note asks exactly this
/// (KI-180). It used to answer it from [`can_invoke`] on the endpoint, which a
/// grant on that endpoint makes `true` by itself: every granted account carried
/// the note, including ones with no workspace role, for which it was false.
///
/// Asked of the PDP against the WORKSPACE resource, for the same reason
/// [`can_invoke`] is asked of the PDP and not of `access_grants`: an
/// object-scoped grant links `resource in Object::"…"`, which the workspace is
/// not in, so it cannot satisfy this; a workspace or platform grant can, and it
/// is precisely the one that cascades to every endpoint.
pub(crate) async fn inherits_invoke(
    st: &DataApiState,
    user_id: UserId,
    ws: WorkspaceId,
) -> Result<bool, egret_core::Error> {
    Ok(st
        .pdp
        .authorize(
            &egret_core::authz::Subject {
                user_id,
                group_ids: Vec::new(),
            },
            crate::routes::EP_INVOKE,
            &egret_core::authz::Resource::Workspace(ws),
        )
        .await?
        .allow)
}
