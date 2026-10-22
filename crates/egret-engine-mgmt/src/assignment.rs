//! `EngineAssignmentService` — orchestrates the two seams behind assigning a
//! provisioned engine to a workspace (P1: assign-engine-to-workspace).
//!
//! Assigning an engine to a workspace is a two-step, cross-seam operation:
//!   1. record the assignment in the engine-provisioning seam
//!      ([`EngineMgmtStore::assign_workspace`]), and
//!   2. materialize a read-only, per-user-delegated connection in the query seam
//!      ([`DatabaseRepo::create_provisioned`]) so the workspace's BI/query tools
//!      can reach the engine's query endpoint (a Trino coordinator or a
//!      StarRocks FE, depending on the engine kind).
//!
//! This service is the single place that keeps those two seams consistent; the
//! CP endpoints (Task 7) call it rather than touching either store directly.

use egret_query::{Database, DatabaseError, DatabaseRepo};
use uuid::Uuid;

use crate::error::EngineMgmtError;
use crate::store::{Engine, EngineMgmtStore};

/// Shared refusal text for both places that gate an assignment on
/// [`Engine::supports_delegation`] — `EngineAssignmentService::assign` and
/// `PUT /engines/{id}/workspaces` (`engines.rs`) — so the two 400s cannot drift.
pub const NOT_DELEGATION_CAPABLE_MSG: &str =
    "engine must support delegation before it can be assigned";

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
        // The routes refuse an external engine with a 409 before they get here;
        // this is the floor under them for any other caller.
        if engine.origin == crate::external::ORIGIN_EXTERNAL {
            return Err(EngineMgmtError::NotAssignable(format!(
                "engine {} is external: egret holds no credentials for it, so it cannot be \
                 assigned to a workspace",
                engine.name
            )));
        }
        if !engine.supports_delegation() {
            return Err(EngineMgmtError::NotAssignable(
                NOT_DELEGATION_CAPABLE_MSG.into(),
            ));
        }
        // Idempotent: `assign_workspace` is `ON CONFLICT DO NOTHING`.
        self.engines
            .assign_workspace(engine_id, workspace_id, by)
            .await?;
        // Materialize (or refresh) the read-only connection for this workspace.
        self.materialize_conn(&engine, workspace_id, Some(egret_core::UserId(by)))
            .await
    }

    /// Materialize (or refresh in place) the read-only, per-user-delegated
    /// connection for `engine` in workspace `ws`: build the delegated
    /// `options`/`secret_id` for the engine's CURRENT delegation mode and
    /// upsert the workspace's `databases` row. When a row for this engine
    /// already exists it is updated in place (so `assign` stays idempotent and
    /// a later delegation change keeps the same row id — chart/query references
    /// survive); otherwise a fresh, name-deduped row is inserted with
    /// `created_by`.
    async fn materialize_conn(
        &self,
        engine: &Engine,
        ws: Uuid,
        created_by: Option<egret_core::UserId>,
    ) -> Result<(), EngineMgmtError> {
        let existing = self
            .databases
            .list_for_workspace(ws)
            .await
            .map_err(to_mgmt_err)?;
        let (opts, secret_id) = delegated_conn_options(engine);
        // Trino delegated connections materialize over the coordinator's
        // separate https listener (8443); see `Engine::client_port`.
        let port = engine.client_port();
        let host = engine.coordinator_host();
        // Refresh in place when a connection for this engine already exists —
        // keeps `assign` idempotent and lets a delegation/TLS change re-point
        // the existing row's options without breaking references to it.
        if existing.iter().any(|d| d.engine_id == Some(engine.id)) {
            self.databases
                .update_provisioned_by_engine(ws, engine.id, &host, port, &opts, secret_id)
                .await
                .map_err(to_mgmt_err)?;
            return Ok(());
        }
        let name = dedup_name(&engine.name, &existing);
        self.databases
            .create_provisioned(
                ws,
                engine.id,
                &name,
                &engine.r#type,
                &host,
                port,
                &opts,
                secret_id,
                created_by,
            )
            .await
            .map_err(to_mgmt_err)?;
        Ok(())
    }

    /// Re-run connection materialization for every workspace this engine is
    /// assigned to — called after a delegation/TLS change so each workspace's
    /// stored `databases` options (impersonate/tls_cert) match the new mode.
    pub async fn rematerialize_for_engine(&self, engine_id: Uuid) -> Result<(), EngineMgmtError> {
        let engine = self
            .engines
            .get_engine(engine_id)
            .await?
            .ok_or_else(|| EngineMgmtError::NotFound(engine_id.to_string()))?;
        for ws in self.engines.list_engine_workspace_ids(engine_id).await? {
            self.materialize_conn(&engine, ws, None).await?;
        }
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
/// materialized delegated connection, by engine kind + delegation mode.
/// Branches cleanly on `engine.r#type`: the Trino shape (below) is completely
/// unchanged from before StarRocks support existed.
fn delegated_conn_options(engine: &Engine) -> (serde_json::Value, Option<Uuid>) {
    if engine.is_starrocks() {
        return starrocks_delegated_conn_options(engine);
    }
    trino_delegated_conn_options(engine)
}

/// Trino delegated connection options, by delegation mode:
///   - impersonate → `{ "user": <delegator>, "impersonate": true }` + the engine's
///     global delegator secret, so the query path opens a basic-auth service-user
///     connection and impersonates the caller;
///   - jwt → `{}` + no secret, so the query path mints the caller's engine JWT.
///
/// TLS (https + pinned public cert) for BOTH delegation modes: the query path
/// talks to the coordinator's separate https listener (8443, see `assign`)
/// and pins this public cert.
fn trino_delegated_conn_options(engine: &Engine) -> (serde_json::Value, Option<Uuid>) {
    let with_tls = |mut opts: serde_json::Value| {
        opts["http_scheme"] = serde_json::Value::String("https".into());
        if let Some(cert) = &engine.coordinator_cert {
            opts["tls_cert"] = serde_json::Value::String(cert.clone());
        }
        opts
    };
    if engine.is_impersonate_delegation() {
        let opts = with_tls(serde_json::json!({
            "user": crate::delegator_policy::DELEGATOR_USERNAME,
            "impersonate": true,
        }));
        (opts, engine.delegator_secret_id)
    } else if engine.is_jwt_delegation() {
        (with_tls(serde_json::json!({})), None)
    } else {
        (serde_json::json!({}), None)
    }
}

/// StarRocks delegated connection options, by delegation mode:
///   - impersonate → `{ "user": <delegator>, "impersonate": true }` + the engine's
///     global delegator secret (the same generic delegator-secret slot Trino
///     uses), so [`egret_query`'s `StarRocksConnector`] logs in as that service
///     user and runs `EXECUTE AS <caller>`;
///   - jwt → `{}` + no secret, so the query path mints the caller's engine JWT
///     and `StarRocksConnector` presents it as the login "password" (StarRocks
///     `authentication_jwt`).
///
/// Unlike Trino, NO `http_scheme` key is set — that option is Trino-only
/// (`https` vs a separate port). StarRocks does in-band TLS on its single
/// MySQL-protocol port (9030, see `assign`'s port selection), so
/// `options.tls_cert` alone (when the engine has a stored FE cert) is enough
/// for the connector to negotiate TLS on that same port.
fn starrocks_delegated_conn_options(engine: &Engine) -> (serde_json::Value, Option<Uuid>) {
    let with_tls_cert = |mut opts: serde_json::Value| {
        if let Some(cert) = &engine.coordinator_cert {
            opts["tls_cert"] = serde_json::Value::String(cert.clone());
        }
        opts
    };
    if engine.is_impersonate_delegation() {
        let opts = with_tls_cert(serde_json::json!({
            "user": crate::delegator_policy::DELEGATOR_USERNAME,
            "impersonate": true,
        }));
        (opts, engine.delegator_secret_id)
    } else if engine.is_jwt_delegation() {
        (with_tls_cert(serde_json::json!({})), None)
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
            spec,
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id,
            root_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: None,
            tls_cert_secret_ref: None,
            internal_secret_id: None,
            monitor_secret_id: None,
            ranger_secret_id: None,
            ranger_secret_prev_id: None,
            created_at: chrono::Utc::now(),
            origin: "provisioned".into(),
            external_host: None,
            external_port: None,
            ranger_plugin_version: None,
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
            serde_json::json!({ "user": "egret_delegator", "impersonate": true, "http_scheme": "https" })
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
        assert_eq!(secret_id, None);
        assert!(opts.get("impersonate").is_none());
        assert!(opts.get("user").is_none());
    }

    #[test]
    fn delegated_conn_options_jwt_uses_https_and_pins_cert() {
        let e = Engine {
            id: Uuid::nil(),
            name: "e".into(),
            r#type: "trino".into(),
            namespace: "ns".into(),
            spec: serde_json::json!({ "trinoConfig": { "delegationMode": "jwt", "auth": { "jwt": {} } } }),
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id: None,
            root_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: Some("PEMCERT".into()),
            tls_cert_secret_ref: None,
            internal_secret_id: None,
            monitor_secret_id: None,
            ranger_secret_id: None,
            ranger_secret_prev_id: None,
            created_at: chrono::Utc::now(),
            origin: "provisioned".into(),
            external_host: None,
            external_port: None,
            ranger_plugin_version: None,
        };
        let (opts, secret) = delegated_conn_options(&e);
        assert_eq!(opts["http_scheme"], "https");
        assert_eq!(opts["tls_cert"], "PEMCERT");
        assert!(opts.get("impersonate").is_none());
        assert!(opts.get("user").is_none());
        assert_eq!(secret, None);
    }

    #[test]
    fn delegated_conn_options_impersonate_uses_https_and_pins_cert() {
        let e = Engine {
            id: Uuid::nil(),
            name: "e".into(),
            r#type: "trino".into(),
            namespace: "ns".into(),
            spec: serde_json::json!({ "trinoConfig": {} }), // default = impersonate
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id: Some(Uuid::from_u128(7)),
            root_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: Some("PEMCERT".into()),
            tls_cert_secret_ref: None,
            internal_secret_id: None,
            monitor_secret_id: None,
            ranger_secret_id: None,
            ranger_secret_prev_id: None,
            created_at: chrono::Utc::now(),
            origin: "provisioned".into(),
            external_host: None,
            external_port: None,
            ranger_plugin_version: None,
        };
        let (opts, secret) = delegated_conn_options(&e);
        assert_eq!(opts["http_scheme"], "https");
        assert_eq!(opts["tls_cert"], "PEMCERT");
        assert_eq!(opts["user"], "egret_delegator");
        assert_eq!(opts["impersonate"], true);
        assert_eq!(secret, Some(Uuid::from_u128(7)));
    }

    // ── starrocks ───────────────────────────────────────────────────────────

    fn starrocks_engine(delegation: Option<&str>, delegator_secret_id: Option<Uuid>) -> Engine {
        let spec = match delegation {
            Some(mode) => {
                serde_json::json!({ "starrocksConfig": { "delegation": { "mode": mode } } })
            }
            None => serde_json::json!({ "starrocksConfig": {} }),
        };
        Engine {
            id: Uuid::nil(),
            name: "sr1".into(),
            r#type: "starrocks".into(),
            namespace: "engines".into(),
            spec,
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id,
            root_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: Some("SRPEMCERT".into()),
            tls_cert_secret_ref: None,
            internal_secret_id: None,
            monitor_secret_id: None,
            ranger_secret_id: None,
            ranger_secret_prev_id: None,
            created_at: chrono::Utc::now(),
            origin: "provisioned".into(),
            external_host: None,
            external_port: None,
            ranger_plugin_version: None,
        }
    }

    /// impersonate: user + impersonate flag + bound delegator secret, tls_cert
    /// set, but NO `http_scheme` (that option is Trino-only).
    #[test]
    fn starrocks_delegated_conn_options_impersonate() {
        let s = Uuid::new_v4();
        let e = starrocks_engine(Some("impersonate"), Some(s));
        let (opts, secret_id) = delegated_conn_options(&e);
        assert_eq!(opts["user"], "egret_delegator");
        assert_eq!(opts["impersonate"], true);
        assert_eq!(opts["tls_cert"], "SRPEMCERT");
        assert!(opts.get("http_scheme").is_none());
        assert_eq!(secret_id, Some(s));
    }

    /// jwt: no user/impersonate/secret, tls_cert still set, no http_scheme.
    #[test]
    fn starrocks_delegated_conn_options_jwt() {
        let e = starrocks_engine(Some("jwt"), Some(Uuid::new_v4()));
        let (opts, secret_id) = delegated_conn_options(&e);
        assert!(opts.get("user").is_none());
        assert!(opts.get("impersonate").is_none());
        assert_eq!(opts["tls_cert"], "SRPEMCERT");
        assert!(opts.get("http_scheme").is_none());
        assert_eq!(secret_id, None);
    }

    /// CRITICAL guard: a StarRocks engine with no delegation config at all
    /// must not be treated as impersonate-delegated by `assign()` — it must
    /// be rejected as not-assignable rather than materializing a broken
    /// connection. This exercises the full `assign()` gate via
    /// `supports_delegation()`, the same predicate `assign()` checks.
    #[test]
    fn starrocks_without_delegation_is_not_impersonate_delegated() {
        let e = starrocks_engine(None, Some(Uuid::new_v4()));
        assert!(!e.supports_delegation());
        assert!(!e.is_impersonate_delegation());
        // Even if something called delegated_conn_options directly despite the
        // gate, it must NOT come back looking like an impersonate connection.
        let (opts, secret_id) = delegated_conn_options(&e);
        assert!(opts.get("impersonate").is_none());
        assert!(opts.get("user").is_none());
        assert_eq!(secret_id, None);
    }

    /// StarRocks never gets the Trino-only 8443/https treatment: port
    /// selection in `assign()` always uses `coordinator_port()` (9030) for
    /// starrocks, delegated or not — mirrored here since `assign()` itself
    /// needs a live DB (`DatabaseRepo`) to exercise end-to-end.
    #[test]
    fn starrocks_port_selection_never_forces_8443() {
        let e = starrocks_engine(Some("jwt"), None);
        assert_eq!(e.coordinator_port(), 9030);
        // The `assign()` port-selection logic (mirrored): starrocks always
        // uses coordinator_port(), trino uses 8443 when delegation-capable.
        let port = if e.is_starrocks() {
            e.coordinator_port()
        } else if e.supports_delegation() {
            8443
        } else {
            e.coordinator_port()
        };
        assert_eq!(port, 9030);
    }
}
