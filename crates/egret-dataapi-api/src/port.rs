//! `DataApiEndpointPort` — the minimal Data API endpoint surface
//! `egret-automation` needs to authorize a `dataapi_alert`, the same leaf-port
//! shape as `egret_query_api::QueryExecPort`: `egret-automation` holds an
//! `Arc<dyn DataApiEndpointPort>`, never concrete `egret-dataapi`, and
//! `egret-dataapi` implements this over its concrete `EndpointStore`.
//!
//! Added in Task 5's fix round 2, to close a hole `require_dataapi_endpoint_
//! read` had from the moment it could take an `id`: `dataapi_endpoint_id` is a
//! plain FK (existence only, no workspace predicate — migration 0128) and the
//! Cedar object check that follows it is told the CALLER's workspace, never
//! the object's, so it never noticed a foreign or fictional id either. This
//! trait exists so that check can be made BEFORE asking the PDP anything.

use uuid::Uuid;

#[async_trait::async_trait]
pub trait DataApiEndpointPort: Send + Sync {
    /// The workspace `id` belongs to, or `None` if no such `dataapi.endpoints`
    /// row exists at all.
    ///
    /// Trashed and live rows both answer with their real workspace — a
    /// trashed endpoint still belongs to the workspace that trashed it, and
    /// `automation.schedules.dataapi_endpoint_id`'s FK is `ON DELETE CASCADE`,
    /// not `ON DELETE SET NULL`, so a trashed row is never the reason this
    /// returns `None`; only a never-existed or hard-deleted id is.
    ///
    /// Deliberately UNSCOPED by any caller-supplied workspace — the caller
    /// compares this answer against its OWN workspace and treats a mismatch
    /// exactly like `None`. See `egret-automation`'s
    /// `require_dataapi_endpoint_read`, whose own comment explains why a
    /// workspace mismatch must be refused the same way as "no such id" (404,
    /// not 403) — a distinguishable response would let one workspace probe
    /// whether a uuid names a live endpoint anywhere in the deployment.
    async fn workspace_of(&self, id: Uuid) -> Result<Option<Uuid>, egret_core::Error>;
}
