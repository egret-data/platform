//! `egret-workspace` — workspace entity, membership, and RBAC authorization.

pub mod access_requests;
pub mod access_views;
pub mod context;
pub mod mapping;
pub mod members;
pub mod model;
pub mod rbac;
pub mod routes;
pub mod workspaces;

/// The Workspace seam's OpenAPI document (E0·P4b-2): the me, admin-workspace,
/// admin-user, and workspace-member routers merged into one `OpenApi`.
/// `egret-server` merges this into the top-level API doc. This is the per-crate
/// `openapi()` convention every annotated seam exposes.
pub fn openapi() -> utoipa::openapi::OpenApi {
    use utoipa::OpenApi as _;
    let mut doc = routes::MeApi::openapi();
    doc.merge(routes::AdminWorkspaceApi::openapi());
    doc.merge(routes::AdminUserApi::openapi());
    doc.merge(routes::WsMemberApi::openapi());
    doc.merge(routes::AccessRequestApi::openapi());
    doc
}
