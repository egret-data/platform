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
/// `folder_id: None` — a folder-scoped grant does NOT cascade to invoke. That is
/// a decision, not an omission: "invoke everything in this folder" widens access
/// and nobody has asked for it. When P5's UI makes endpoint folders reachable,
/// whoever wants that cascade passes the real folder here and accepts what it
/// means.
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
