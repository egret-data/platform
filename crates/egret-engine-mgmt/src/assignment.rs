//! `EngineAssignmentService` — orchestrates the two seams behind assigning a
//! provisioned engine to a workspace (P1: assign-engine-to-workspace).
//!
//! Assigning an engine to a workspace is a two-step, cross-seam operation:
//!   1. record the assignment in the engine-provisioning seam
//!      ([`EngineMgmtStore::assign_workspace`]), and
//!   2. materialize a read-only, per-user-delegated connection in the query seam
//!      ([`DatabaseRepo::create_provisioned`]) so the workspace's BI/query tools
//!      can reach the engine's Trino coordinator.
//!
//! This service is the single place that keeps those two seams consistent; the
//! CP endpoints (Task 7) call it rather than touching either store directly.

use egret_query::{Database, DatabaseError, DatabaseRepo};
use uuid::Uuid;

use crate::error::EngineMgmtError;
use crate::store::{Engine, EngineMgmtStore};

/// Orchestrates engine↔workspace assignment across the engine-provisioning and
/// query seams. Cheap to clone (both stores are `Clone` handles over a pool).
#[derive(Clone)]
pub struct EngineAssignmentService {
    engines: EngineMgmtStore,
    databases: DatabaseRepo,
}

impl EngineAssignmentService {
    pub fn new(engines: EngineMgmtStore, databases: DatabaseRepo) -> Self {
        Self { engines, databases }
    }

    /// Assign `engine_id` to `workspace_id` on behalf of user `by`.
    ///
    /// Rejects engines not configured for JWT delegation
    /// ([`EngineMgmtError::NotAssignable`]). Otherwise records the assignment
    /// (idempotent) and — unless one already exists for this engine — materializes
    /// a read-only delegated connection in the workspace.
    pub async fn assign(
        &self,
        engine_id: Uuid,
        workspace_id: Uuid,
        by: Uuid,
    ) -> Result<(), EngineMgmtError> {
        let engine = self
            .engines
            .get_engine(engine_id)
            .await?
            .ok_or_else(|| EngineMgmtError::NotFound(engine_id.to_string()))?;
        if !engine.supports_delegation() {
            return Err(EngineMgmtError::NotAssignable(
                "engine must support delegation before it can be assigned".into(),
            ));
        }
        // Idempotent: `assign_workspace` is `ON CONFLICT DO NOTHING`.
        self.engines
            .assign_workspace(engine_id, workspace_id, by)
            .await?;
        // Materialize the read-only connection — skip if one already exists for
        // this engine (keeps `assign` idempotent end-to-end).
        let existing = self
            .databases
            .list_for_workspace(workspace_id)
            .await
            .map_err(to_mgmt_err)?;
        if existing.iter().any(|d| d.engine_id == Some(engine_id)) {
            return Ok(());
        }
        let name = dedup_name(&engine.name, &existing);
        let (opts, secret_id) = delegated_conn_options(&engine);
        self.databases
            .create_provisioned(
                workspace_id,
                engine_id,
                &name,
                &engine.r#type,
                &engine.coordinator_host(),
                engine.coordinator_port(),
                &opts,
                secret_id,
                Some(egret_core::UserId(by)),
            )
            .await
            .map_err(to_mgmt_err)?;
        Ok(())
    }

    /// Unassign `engine_id` from `workspace_id`: drop the materialized connection
    /// first, then the assignment record. Both steps are idempotent.
    pub async fn unassign(
        &self,
        engine_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<(), EngineMgmtError> {
        self.databases
            .delete_by_engine(workspace_id, engine_id)
            .await
            .map_err(to_mgmt_err)?;
        self.engines
            .unassign_workspace(engine_id, workspace_id)
            .await?;
        Ok(())
    }
}

/// De-duplicate a connection name against the workspace's existing databases,
/// appending `-2`, `-3`, … until it's unique (avoids the `create_provisioned`
/// unique-name violation when two engines share a base name).
fn dedup_name(base: &str, existing: &[Database]) -> String {
    if !existing.iter().any(|d| d.name == base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !existing.iter().any(|d| &d.name == c))
        .expect("infinite range always yields a free name")
}

/// Select the connection `options` JSONB and bound `secret_id` for the
/// materialized delegated connection, by delegation mode:
///   - impersonate → `{ "user": <delegator>, "impersonate": true }` + the engine's
///     global delegator secret, so the query path opens a basic-auth service-user
///     connection and impersonates the caller;
///   - jwt → `{}` + no secret, so the query path mints the caller's engine JWT.
fn delegated_conn_options(engine: &Engine) -> (serde_json::Value, Option<Uuid>) {
    if engine.is_impersonate_delegation() {
        (
            serde_json::json!({
                "user": crate::delegator_policy::DELEGATOR_USERNAME,
                "impersonate": true,
            }),
            engine.delegator_secret_id,
        )
    } else {
        (serde_json::json!({}), None)
    }
}

/// Map a query-seam [`DatabaseError`] into an [`EngineMgmtError`]. The common
/// arms of the materialization flow (transport, unique-name, missing row) carry
/// over directly; the remaining domain arms cannot arise here (no secret is
/// sealed and the engine type always parses) but are preserved as text.
fn to_mgmt_err(e: DatabaseError) -> EngineMgmtError {
    match e {
        DatabaseError::Db(e) => EngineMgmtError::Db(e),
        DatabaseError::AlreadyExists(s) => EngineMgmtError::AlreadyExists(s),
        DatabaseError::NotFound(s) => EngineMgmtError::NotFound(s),
        other => EngineMgmtError::NotAssignable(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Engine;

    fn engine(spec: serde_json::Value, delegator_secret_id: Option<Uuid>) -> Engine {
        Engine {
            id: Uuid::nil(),
            name: "e".into(),
            r#type: "trino".into(),
            namespace: "ns".into(),
            warehouse_id: Uuid::nil(),
            spec,
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn impersonate_engine_binds_delegator_secret_and_impersonate_opts() {
        let s = Uuid::new_v4();
        // Default (no delegationMode) is impersonate.
        let e = engine(serde_json::json!({ "trinoConfig": {} }), Some(s));
        let (opts, secret_id) = delegated_conn_options(&e);
        assert_eq!(
            opts,
            serde_json::json!({ "user": "egret_delegator", "impersonate": true })
        );
        assert_eq!(secret_id, Some(s));
    }

    #[test]
    fn jwt_engine_binds_no_secret_and_empty_opts() {
        let e = engine(
            serde_json::json!({ "trinoConfig": { "delegationMode": "jwt", "auth": { "jwt": {} } } }),
            Some(Uuid::new_v4()),
        );
        let (opts, secret_id) = delegated_conn_options(&e);
        assert_eq!(opts, serde_json::json!({}));
        assert_eq!(secret_id, None);
    }
}
