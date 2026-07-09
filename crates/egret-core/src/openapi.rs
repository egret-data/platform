//! Shared OpenAPI vocabulary reused by every annotated seam (E0·P4).
//!
//! These are the pattern-locking primitives the per-crate `#[utoipa::path]`
//! annotations reference so ~10 seams describe the same recurring shapes once,
//! consistently, instead of re-spelling them per handler:
//!
//! - [`WorkspacePath`] — the `{workspace}` path parameter present on every
//!   workspace-scoped route. Use `params(WorkspacePath, ("id" = Uuid, Path, …))`.
//! - [`Ack`] — the `{ "ok": true }` body returned by delete / cancel / ack
//!   endpoints. Reference as `body = Ack` (the shape matches the handlers'
//!   `Json(json!({ "ok": true }))`).
//!
//! utoipa is an unconditional dependency of `egret-core`, so these are available
//! to the light `*-api` contract crates without pulling the `http`/axum stack.

/// The `{workspace}` path parameter shared by every workspace-scoped handler.
///
/// Reference it in a `#[utoipa::path]` via `params(WorkspacePath, …)` instead of
/// re-spelling `("workspace" = String, Path, description = "Workspace slug")` on
/// each handler.
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Path)]
pub struct WorkspacePath {
    /// Workspace slug
    pub workspace: String,
}

/// The uniform acknowledgement body `{ "ok": true }` returned by mutating
/// endpoints that have nothing else to say (delete, cancel, …).
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct Ack {
    pub ok: bool,
}

impl Ack {
    /// Construct an `ok: true` acknowledgement.
    pub fn ok() -> Self {
        Self { ok: true }
    }
}
