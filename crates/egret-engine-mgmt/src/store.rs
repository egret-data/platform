//! Engine-mgmt store: CRUD for the `engine_mgmt.engines` table (this seam owns
//! it). Runtime sqlx queries; schema-qualified.

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::EngineMgmtError;

/// The values `engine_mgmt.engines.status` may hold (`engines_status_check`).
pub mod engine_status {
    /// Created; the cluster has not yet reported every pod ready.
    pub const PROVISIONING: &str = "provisioning";
    /// Live status reported every pod ready at least once.
    pub const RUNNING: &str = "running";
    /// A create failed and its rollback could not remove the row.
    pub const FAILED: &str = "failed";
    /// A delete is in progress (written by the delete handler).
    pub const DELETING: &str = "deleting";
}

/// A provisioned query engine (control-plane, platform-global). Its warehouse
/// is not a column here — it is derived from `registry.engine_catalog_attachments
/// .is_primary` (see `resolve_primary_catalog`). `spec` holds the desired
/// provisioning parameters; `status` tracks the last-known lifecycle phase.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Engine {
    pub id: Uuid,
    pub name: String,
    pub r#type: String,
    pub namespace: String,
    pub spec: serde_json::Value,
    pub status: String,
    pub authz_service_id: Option<i64>,
    pub delegator_secret_id: Option<Uuid>,
    pub root_secret_id: Option<Uuid>,
    pub tls_secret_id: Option<Uuid>,
    pub coordinator_cert: Option<String>,
    pub tls_cert_secret_ref: Option<Uuid>,
    /// Named secret holding `TRINO_INTERNAL_SECRET`, as last applied. `None`:
    /// the pods still carry the legacy id-derived value.
    pub internal_secret_id: Option<Uuid>,
    /// Named secret holding the `egret-monitor` password, as last applied.
    /// `None`: the pods still carry the legacy id-derived value.
    pub monitor_secret_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    /// `provisioned` (egret runs the cluster) or `external` (egret holds a row
    /// for a cluster it never touches). See [`crate::external`]; migration
    /// 0184's `engines_external_shape` makes the other columns below agree.
    pub origin: String,
    /// External only: the address a client dials. `None` for a provisioned row.
    pub external_host: Option<String>,
    pub external_port: Option<i32>,
    /// External only: the Ranger plugin version the operator declared
    /// installed. The one source of an external engine's plugin version for the
    /// equivalence gate and explain (`egret_perm_compile::engine_plugin_version`).
    pub ranger_plugin_version: Option<String>,
    /// Named secret holding the password this engine's Ranger plugin presents
    /// to egret's `/ranger` surface now (spec 2026-10-20-ranger-surface-auth
    /// §4.3). `None`: no credential minted yet. See
    /// [`crate::ranger_credential`].
    pub ranger_secret_id: Option<Uuid>,
    /// The credential the plugin presented before the last rotation, still
    /// accepted until a successful apply retires it (A10).
    pub ranger_secret_prev_id: Option<Uuid>,
}

impl Engine {
    /// True when this is a StarRocks engine. The single source of truth for the
    /// kind check every de-Trino-gated helper below branches on — a helper (not
    /// a repeated `self.r#type == "starrocks"` literal) so a typo can't silently
    /// send a StarRocks engine down the Trino path (which defaults to
    /// impersonate-capable), reintroducing the exact bug class this seam fixes.
    pub fn is_starrocks(&self) -> bool {
        self.r#type == "starrocks"
    }

    /// The declared host of an external engine, `None` for a provisioned one.
    fn external_address_host(&self) -> Option<&str> {
        if self.origin == crate::external::ORIGIN_EXTERNAL {
            self.external_host.as_deref()
        } else {
            None
        }
    }

    fn external_address_port(&self) -> Option<i32> {
        if self.origin == crate::external::ORIGIN_EXTERNAL {
            self.external_port
        } else {
            None
        }
    }

    /// In-cluster DNS name of this engine's coordinator/FE service. For an
    /// external engine, the host its operator declared instead. For Trino,
    /// egret provisions plain Deployments plus a single Service named
    /// `{name}-trino.{namespace}.svc` (the coordinator selector) — so that is
    /// the host we dial. StarRocks' kube-starrocks operator instead exposes the
    /// FE (the MySQL-protocol query endpoint) as `{name}-fe-service.{namespace}.svc`.
    pub fn coordinator_host(&self) -> String {
        if let Some(host) = self.external_address_host() {
            return host.to_string();
        }
        if self.is_starrocks() {
            format!("{}-fe-service.{}.svc", self.name, self.namespace)
        } else {
            format!("{}-trino.{}.svc", self.name, self.namespace)
        }
    }

    /// The coordinator/FE's query port (fixed by each operator's manifests):
    /// Trino's HTTP listener is `8080`; StarRocks' FE MySQL-protocol
    /// (`query_port`) listener is `9030`.
    pub fn coordinator_port(&self) -> i32 {
        if let Some(port) = self.external_address_port() {
            return port;
        }
        if self.is_starrocks() {
            9030
        } else {
            8080
        }
    }

    /// Whether the engine accepts a minted per-user engine JWT — a prerequisite
    /// for materializing a delegated (read-only) workspace connection.
    ///
    /// Trino: mirrors the provisioner's `spec.trinoConfig.auth.jwt` object
    /// check (`egret-engine-mgmt/src/engines.rs::resolve_auth`).
    /// StarRocks: has no `trinoConfig.auth.jwt` concept — jwt-capability is
    /// simply "the stored delegation mode is `jwt`" (see [`Self::delegation_mode`]).
    pub fn supports_jwt_delegation(&self) -> bool {
        if self.is_starrocks() {
            self.is_jwt_delegation()
        } else {
            self.spec
                .get("trinoConfig")
                .and_then(|tc| tc.get("auth"))
                .and_then(|a| a.get("jwt"))
                .map(|j| j.is_object())
                .unwrap_or(false)
        }
    }

    /// Delegation mode from the stored spec.
    ///
    /// Trino: `trinoConfig.delegationMode`, defaulting to `impersonate` when
    /// absent (back-compat: pre-delegation Trino engines default to the
    /// impersonate path).
    ///
    /// StarRocks: `starrocksConfig.delegation.mode`. CRITICAL: unlike Trino,
    /// this does NOT default to `impersonate` when absent — a StarRocks
    /// engine's spec has no `trinoConfig` at all, so falling through to
    /// Trino's `impersonate` default would make every un-configured StarRocks
    /// engine silently report itself delegation-capable and cause `assign()`
    /// to materialize a broken connection (wrong delegator user, no secret).
    /// Absent/unset delegation on a StarRocks engine reports `"none"`, which
    /// none of `is_impersonate_delegation`/`is_jwt_delegation` match.
    pub fn delegation_mode(&self) -> &str {
        if self.is_starrocks() {
            self.spec
                .get("starrocksConfig")
                .and_then(|c| c.get("delegation"))
                .and_then(|d| d.get("mode"))
                .and_then(|v| v.as_str())
                .unwrap_or("none")
        } else {
            self.spec
                .get("trinoConfig")
                .and_then(|t| t.get("delegationMode"))
                .and_then(|v| v.as_str())
                .unwrap_or("impersonate")
        }
    }

    /// True when delegated queries run via basic-auth service-user impersonation.
    pub fn is_impersonate_delegation(&self) -> bool {
        self.delegation_mode() == "impersonate"
    }

    /// True when delegated queries authenticate with the caller's own JWT over TLS.
    pub fn is_jwt_delegation(&self) -> bool {
        self.delegation_mode() == "jwt"
    }

    /// True when the engine can back a delegated (per-user) connection at all.
    /// Impersonate mode always can; jwt mode requires JWT auth enabled. A
    /// StarRocks engine with no `starrocksConfig.delegation` at all reports
    /// `false` (see [`Self::delegation_mode`]'s doc) rather than silently
    /// defaulting to impersonate.
    pub fn supports_delegation(&self) -> bool {
        self.is_impersonate_delegation() || self.supports_jwt_delegation()
    }
}

/// A workspace an engine is assigned to (join of `engine_mgmt.engine_workspaces`
/// with `workspaces` for the display name/slug).
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct EngineWorkspace {
    pub engine_id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub slug: String,
    pub assigned_at: DateTime<Utc>,
}

#[derive(Clone)]
pub struct EngineMgmtStore {
    db: Db,
}

impl EngineMgmtStore {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
    pub(crate) fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    /// The catalog registry on the same database (SP3).
    pub fn registry(&self) -> egret_catalog_registry::RegistryStore {
        egret_catalog_registry::RegistryStore::new(self.db.clone())
    }

    // engines ------------------------------------------------------------------

    pub async fn create_engine(
        &self,
        name: &str,
        r#type: &str,
        namespace: &str,
        spec: &serde_json::Value,
        tls_cert_secret_ref: Option<Uuid>,
    ) -> Result<Engine, EngineMgmtError> {
        sqlx::query_as::<_, Engine>(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, spec, tls_cert_secret_ref) VALUES ($1,$2,$3,$4,$5) \
             RETURNING id, name, type, namespace, spec, status, authz_service_id, delegator_secret_id, root_secret_id, tls_secret_id, coordinator_cert, tls_cert_secret_ref, internal_secret_id, monitor_secret_id, created_at, origin, external_host, external_port, ranger_plugin_version, ranger_secret_id, ranger_secret_prev_id",
        )
        .bind(name)
        .bind(r#type)
        .bind(namespace)
        .bind(spec)
        .bind(tls_cert_secret_ref)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.constraint() == Some("engines_name_key") => {
                EngineMgmtError::AlreadyExists(name.to_string())
            }
            _ => EngineMgmtError::Db(e),
        })
    }

    pub async fn list_engines(&self) -> Result<Vec<Engine>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, Engine>(
            "SELECT id, name, type, namespace, spec, status, authz_service_id, delegator_secret_id, root_secret_id, tls_secret_id, coordinator_cert, tls_cert_secret_ref, internal_secret_id, monitor_secret_id, created_at, origin, external_host, external_port, ranger_plugin_version, ranger_secret_id, ranger_secret_prev_id \
             FROM engine_mgmt.engines ORDER BY created_at",
        )
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn get_engine(&self, id: Uuid) -> Result<Option<Engine>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, Engine>(
            "SELECT id, name, type, namespace, spec, status, authz_service_id, delegator_secret_id, root_secret_id, tls_secret_id, coordinator_cert, tls_cert_secret_ref, internal_secret_id, monitor_secret_id, created_at, origin, external_host, external_port, ranger_plugin_version, ranger_secret_id, ranger_secret_prev_id \
             FROM engine_mgmt.engines WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }

    /// Persist a new spec, gated on the row still being at the caller's last
    /// observed `status` — a compare-and-set (mirrors
    /// [`Self::transition_engine_status`]'s `Ok(bool)`) that does NOT itself
    /// write `status`. Only [`Self::set_engine_status`] and
    /// [`Self::transition_engine_status`] own that column. Returns whether the
    /// row was still at `status` (and so was updated); the caller MUST check
    /// this — a `false` means something else changed the row (typically a
    /// concurrent delete) since it was read, and the new spec was NOT written.
    ///
    /// Every caller reads an [`Engine`], derives a new spec from it, then calls
    /// this passing the status it just read. Without the `AND status = $3`
    /// guard, a PUT racing a concurrent DELETE could read `running`, and DELETE
    /// could flip the row to `deleting` before the PUT's write lands — silently
    /// erasing the `deleting` retry marker `delete_engine` depends on. With the
    /// guard, that write matches zero rows and is a no-op: the row stays
    /// exactly as DELETE left it.
    pub async fn update_engine(
        &self,
        id: Uuid,
        spec: &serde_json::Value,
        status: &str,
    ) -> Result<bool, EngineMgmtError> {
        let res =
            sqlx::query("UPDATE engine_mgmt.engines SET spec = $2 WHERE id = $1 AND status = $3")
                .bind(id)
                .bind(spec)
                .bind(status)
                .execute(self.pool())
                .await?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn set_engine_status(&self, id: Uuid, status: &str) -> Result<(), EngineMgmtError> {
        sqlx::query("UPDATE engine_mgmt.engines SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Move `status` from `from` to `to` only if it is still `from`; returns
    /// whether a row moved. Conditional so a reader recording `running` can
    /// never overwrite a concurrent `deleting`.
    pub async fn transition_engine_status(
        &self,
        id: Uuid,
        from: &str,
        to: &str,
    ) -> Result<bool, EngineMgmtError> {
        let res =
            sqlx::query("UPDATE engine_mgmt.engines SET status = $3 WHERE id = $1 AND status = $2")
                .bind(id)
                .bind(from)
                .bind(to)
                .execute(self.pool())
                .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Bind (or detach with `None`) an engine to a Ranger policy profile.
    pub async fn set_engine_authz_service(
        &self,
        id: Uuid,
        authz_service_id: Option<i64>,
    ) -> Result<(), EngineMgmtError> {
        sqlx::query("UPDATE engine_mgmt.engines SET authz_service_id = $2 WHERE id = $1")
            .bind(id)
            .bind(authz_service_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// [`set_engine_authz_service`](Self::set_engine_authz_service) on the
    /// CALLER's open transaction.
    ///
    /// The update path binds and RECOMPILES atomically: a profile an engine
    /// now binds but whose compiled policy set predates that binding is
    /// exactly the staleness SP4a's engine-create recompile exists to remove,
    /// and committing the binding without the compile would reintroduce it in
    /// the one place a caller cannot see it. Does not commit; the caller does.
    ///
    /// `None` DETACHES the engine. It never touches `policy.authz_services` —
    /// unbinding must not delete a profile row.
    ///
    /// **Compare-and-set on `expect_status`**, and `false` when it missed —
    /// the SAME guard [`update_engine`](Self::update_engine) puts on the spec
    /// write, for the same reason. A PUT racing a concurrent DELETE could
    /// otherwise read `running`, have DELETE flip the row to `deleting`, and
    /// still bind (and compile) a profile onto an engine that is being torn
    /// down — a write the caller is about to be told failed with a 409.
    pub async fn set_engine_authz_service_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        id: Uuid,
        authz_service_id: Option<i64>,
        expect_status: &str,
    ) -> Result<bool, EngineMgmtError> {
        let res = sqlx::query(
            "UPDATE engine_mgmt.engines SET authz_service_id = $2 \
             WHERE id = $1 AND status = $3",
        )
        .bind(id)
        .bind(authz_service_id)
        .bind(expect_status)
        .execute(&mut **tx)
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// How many engines are bound to a Ranger profile.
    pub async fn count_engines_bound_to_authz_service(
        &self,
        service_id: i64,
    ) -> Result<i64, EngineMgmtError> {
        Ok(sqlx::query_scalar(
            "SELECT count(*) FROM engine_mgmt.engines WHERE authz_service_id = $1",
        )
        .bind(service_id)
        .fetch_one(self.pool())
        .await?)
    }

    /// Persist the generated delegator secret id for an engine (idempotent bookkeeping).
    pub async fn set_delegator_secret(
        &self,
        engine_id: Uuid,
        secret_id: Uuid,
    ) -> Result<(), EngineMgmtError> {
        sqlx::query("UPDATE engine_mgmt.engines SET delegator_secret_id = $2 WHERE id = $1")
            .bind(engine_id)
            .bind(secret_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Persist the generated StarRocks FE `root` password secret id for an engine
    /// (idempotent bookkeeping; mirrors [`Self::set_delegator_secret`]).
    pub async fn set_root_secret(
        &self,
        engine_id: Uuid,
        secret_id: Uuid,
    ) -> Result<(), EngineMgmtError> {
        sqlx::query("UPDATE engine_mgmt.engines SET root_secret_id = $2 WHERE id = $1")
            .bind(engine_id)
            .bind(secret_id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    /// Persist the generated TLS keystore secret id + public cert PEM for an engine.
    pub async fn set_tls(
        &self,
        engine_id: Uuid,
        secret_id: Uuid,
        cert_pem: &str,
    ) -> Result<(), EngineMgmtError> {
        sqlx::query(
            "UPDATE engine_mgmt.engines SET tls_secret_id = $2, coordinator_cert = $3 WHERE id = $1",
        )
        .bind(engine_id)
        .bind(secret_id)
        .bind(cert_pem)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Record the internal/monitor secrets an apply attempted to render,
    /// compare-and-set against the pair the caller's build read
    /// (`prev_internal_secret_id`/`prev_monitor_secret_id`, i.e. `Engine::{internal_secret_id,monitor_secret_id}`
    /// as loaded BEFORE that build). Writes only when the row still names that
    /// exact pair (`IS NOT DISTINCT FROM` so two `NULL`s also match); returns
    /// `false` and writes nothing otherwise. This closes the race between two
    /// concurrent applies on the same engine: whichever commits first wins,
    /// and the loser does not clobber it with its own (possibly stale) pair.
    pub async fn set_internal_secret_ids(
        &self,
        engine_id: Uuid,
        prev_internal_secret_id: Option<Uuid>,
        prev_monitor_secret_id: Option<Uuid>,
        internal_secret_id: Uuid,
        monitor_secret_id: Uuid,
    ) -> Result<bool, EngineMgmtError> {
        let res = sqlx::query(
            "UPDATE engine_mgmt.engines SET internal_secret_id = $4, monitor_secret_id = $5 \
             WHERE id = $1 AND internal_secret_id IS NOT DISTINCT FROM $2 \
             AND monitor_secret_id IS NOT DISTINCT FROM $3",
        )
        .bind(engine_id)
        .bind(prev_internal_secret_id)
        .bind(prev_monitor_secret_id)
        .bind(internal_secret_id)
        .bind(monitor_secret_id)
        .execute(self.pool())
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Record a Ranger plugin credential pair, compare-and-set against the
    /// CURRENT id the caller read (`expected_current`, `IS NOT DISTINCT FROM`
    /// so two `NULL`s match). Returns `false` and writes nothing when a
    /// concurrent writer moved the column first — the same shape as
    /// [`Self::set_internal_secret_ids`].
    pub async fn set_ranger_secret_ids(
        &self,
        engine_id: Uuid,
        expected_current: Option<Uuid>,
        current: Uuid,
        previous: Option<Uuid>,
    ) -> Result<bool, EngineMgmtError> {
        let res = sqlx::query(
            "UPDATE engine_mgmt.engines SET ranger_secret_id = $3, ranger_secret_prev_id = $4 \
             WHERE id = $1 AND ranger_secret_id IS NOT DISTINCT FROM $2",
        )
        .bind(engine_id)
        .bind(expected_current)
        .bind(current)
        .bind(previous)
        .execute(self.pool())
        .await?;
        Ok(res.rows_affected() == 1)
    }

    /// Clear the previous Ranger credential, but only while the row still
    /// names `expected_previous` there. `false`: a rotation has moved the
    /// column since the caller read it, and the caller must not delete the
    /// secret it read.
    pub async fn clear_ranger_prev(
        &self,
        engine_id: Uuid,
        expected_previous: Uuid,
    ) -> Result<bool, EngineMgmtError> {
        let res = sqlx::query(
            "UPDATE engine_mgmt.engines SET ranger_secret_prev_id = NULL \
             WHERE id = $1 AND ranger_secret_prev_id = $2",
        )
        .bind(engine_id)
        .bind(expected_previous)
        .execute(self.pool())
        .await?;
        Ok(res.rows_affected() == 1)
    }

    pub async fn delete_engine(&self, id: Uuid) -> Result<(), EngineMgmtError> {
        sqlx::query("DELETE FROM engine_mgmt.engines WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // engine ↔ workspace assignments -------------------------------------------

    /// Assign an engine to a workspace. Idempotent: re-assigning is a no-op.
    pub async fn assign_workspace(
        &self,
        engine_id: Uuid,
        workspace_id: Uuid,
        by: Uuid,
    ) -> Result<(), EngineMgmtError> {
        sqlx::query(
            "INSERT INTO engine_mgmt.engine_workspaces (engine_id, workspace_id, assigned_by) \
             VALUES ($1,$2,$3) ON CONFLICT (engine_id, workspace_id) DO NOTHING",
        )
        .bind(engine_id)
        .bind(workspace_id)
        .bind(by)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Remove an engine↔workspace assignment. No-op if it doesn't exist.
    pub async fn unassign_workspace(
        &self,
        engine_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<(), EngineMgmtError> {
        sqlx::query(
            "DELETE FROM engine_mgmt.engine_workspaces WHERE engine_id=$1 AND workspace_id=$2",
        )
        .bind(engine_id)
        .bind(workspace_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// Workspaces this engine is assigned to (with display name/slug).
    pub async fn list_workspaces_for_engine(
        &self,
        engine_id: Uuid,
    ) -> Result<Vec<EngineWorkspace>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, EngineWorkspace>(
            "SELECT ew.engine_id, ew.workspace_id, w.name, w.slug, ew.assigned_at \
             FROM engine_mgmt.engine_workspaces ew JOIN workspaces w ON w.id = ew.workspace_id \
             WHERE ew.engine_id = $1 ORDER BY w.name",
        )
        .bind(engine_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// The workspace ids this engine is assigned to. Thin variant of
    /// [`Self::list_workspaces_for_engine`] used by re-materialization (a
    /// delegation/TLS change re-runs the per-workspace connection upsert for
    /// every assigned workspace), which needs only the ids.
    pub async fn list_engine_workspace_ids(
        &self,
        engine_id: Uuid,
    ) -> Result<Vec<Uuid>, EngineMgmtError> {
        Ok(sqlx::query_scalar(
            "SELECT workspace_id FROM engine_mgmt.engine_workspaces WHERE engine_id = $1",
        )
        .bind(engine_id)
        .fetch_all(self.pool())
        .await?)
    }

    /// The ids in `ids` that name no workspace.
    pub async fn missing_workspace_ids(&self, ids: &[Uuid]) -> Result<Vec<Uuid>, EngineMgmtError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(sqlx::query_scalar(
            "SELECT x FROM unnest($1::uuid[]) AS x \
             WHERE NOT EXISTS (SELECT 1 FROM workspaces w WHERE w.id = x)",
        )
        .bind(ids.to_vec())
        .fetch_all(self.pool())
        .await?)
    }

    /// Engines assigned to a workspace.
    pub async fn list_engines_for_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<Engine>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, Engine>(
            "SELECT e.id, e.name, e.type, e.namespace, e.spec, e.status, e.authz_service_id, e.delegator_secret_id, e.root_secret_id, e.tls_secret_id, e.coordinator_cert, e.tls_cert_secret_ref, e.internal_secret_id, e.monitor_secret_id, e.created_at, e.origin, e.external_host, e.external_port, e.ranger_plugin_version, e.ranger_secret_id, e.ranger_secret_prev_id \
             FROM engine_mgmt.engines e JOIN engine_mgmt.engine_workspaces ew ON ew.engine_id = e.id \
             WHERE ew.workspace_id = $1 ORDER BY e.name",
        )
        .bind(workspace_id)
        .fetch_all(self.pool())
        .await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delegation_mode_defaults_to_impersonate() {
        let e = Engine {
            id: Uuid::nil(),
            name: "e".into(),
            r#type: "trino".into(),
            namespace: "ns".into(),
            spec: serde_json::json!({ "trinoConfig": {} }),
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id: None,
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
        };
        assert_eq!(e.delegation_mode(), "impersonate");
        assert!(e.is_impersonate_delegation());
        assert!(e.supports_delegation());
    }

    #[test]
    fn delegation_mode_jwt_is_not_impersonate() {
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
        };
        assert_eq!(e.delegation_mode(), "jwt");
        assert!(!e.is_impersonate_delegation());
        assert!(e.supports_delegation()); // jwt engine with JWT auth also supports delegation
    }

    #[test]
    fn is_jwt_delegation_true_only_for_jwt_mode() {
        let mut e = Engine {
            id: Uuid::nil(),
            name: "e".into(),
            r#type: "trino".into(),
            namespace: "ns".into(),
            spec: serde_json::json!({ "trinoConfig": { "delegationMode": "jwt" } }),
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id: None,
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
        };
        assert!(e.is_jwt_delegation());
        assert!(!e.is_impersonate_delegation());

        e.spec = serde_json::json!({ "trinoConfig": {} }); // default = impersonate
        assert!(!e.is_jwt_delegation());
    }

    /// Fixture builder for the starrocks tests below.
    fn starrocks_engine(spec: serde_json::Value) -> Engine {
        Engine {
            id: Uuid::nil(),
            name: "sr1".into(),
            r#type: "starrocks".into(),
            namespace: "engines".into(),
            spec,
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id: None,
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
    fn starrocks_coordinator_host_is_fe_service_not_coordinator() {
        let e = starrocks_engine(serde_json::json!({}));
        assert_eq!(e.coordinator_host(), "sr1-fe-service.engines.svc");
    }

    #[test]
    fn starrocks_coordinator_port_is_9030_not_8080() {
        let e = starrocks_engine(serde_json::json!({}));
        assert_eq!(e.coordinator_port(), 9030);
    }

    /// Trino's coordinator_host must dial the `{name}-trino` Service that the
    /// provisioner actually creates, and its port must stay 8080.
    #[test]
    fn trino_coordinator_host_matches_provisioned_service() {
        let e = Engine {
            id: Uuid::nil(),
            name: "trino1".into(),
            r#type: "trino".into(),
            namespace: "engines".into(),
            spec: serde_json::json!({}),
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id: None,
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
        };
        assert_eq!(e.coordinator_host(), "trino1-trino.engines.svc");
        assert_eq!(e.coordinator_port(), 8080);
    }

    /// CRITICAL (the latent bug this task fixes): a StarRocks engine with NO
    /// `starrocksConfig.delegation` at all — the state every StarRocks engine
    /// is in immediately after `create_starrocks_engine` unless the admin
    /// opted into delegation — must NOT be reported as delegation-capable.
    /// Before this fix `delegation_mode()` unconditionally defaulted to
    /// `"impersonate"`, so `assign()` would have materialized a broken
    /// impersonate connection (wrong delegator user, no bound secret).
    #[test]
    fn starrocks_no_delegation_config_is_not_delegation_capable() {
        // No starrocksConfig key at all.
        let e = starrocks_engine(serde_json::json!({}));
        assert_eq!(e.delegation_mode(), "none");
        assert!(!e.is_impersonate_delegation());
        assert!(!e.is_jwt_delegation());
        assert!(!e.supports_jwt_delegation());
        assert!(!e.supports_delegation());

        // starrocksConfig present but no delegation sub-object (the shape
        // `create_starrocks_engine` persists when the body omits delegation).
        let e2 = starrocks_engine(serde_json::json!({ "starrocksConfig": { "feReplicas": 1 } }));
        assert!(!e2.supports_delegation());
    }

    #[test]
    fn starrocks_delegation_mode_impersonate() {
        let e = starrocks_engine(serde_json::json!({
            "starrocksConfig": { "delegation": { "mode": "impersonate" } }
        }));
        assert_eq!(e.delegation_mode(), "impersonate");
        assert!(e.is_impersonate_delegation());
        assert!(!e.is_jwt_delegation());
        assert!(!e.supports_jwt_delegation());
        assert!(e.supports_delegation());
    }

    #[test]
    fn starrocks_delegation_mode_jwt() {
        let e = starrocks_engine(serde_json::json!({
            "starrocksConfig": { "delegation": { "mode": "jwt" } }
        }));
        assert_eq!(e.delegation_mode(), "jwt");
        assert!(!e.is_impersonate_delegation());
        assert!(e.is_jwt_delegation());
        assert!(e.supports_jwt_delegation());
        assert!(e.supports_delegation());
    }
}
