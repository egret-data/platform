//! Engine-mgmt store: CRUD for the `engine_mgmt.engines` / `engine_mgmt.engine_catalogs`
//! tables (this seam owns them). Runtime sqlx queries; schema-qualified.

use chrono::{DateTime, Utc};
use egret_metastore::Db;
use uuid::Uuid;

use crate::error::EngineMgmtError;

/// A provisioned query engine (control-plane, platform-global) wired to a
/// warehouse. `spec` holds the desired provisioning parameters; `status` tracks
/// the last-known lifecycle phase.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct Engine {
    pub id: Uuid,
    pub name: String,
    pub r#type: String,
    pub namespace: String,
    pub warehouse_id: Uuid,
    pub spec: serde_json::Value,
    pub status: String,
    pub authz_service_id: Option<i64>,
    pub delegator_secret_id: Option<Uuid>,
    pub tls_secret_id: Option<Uuid>,
    pub coordinator_cert: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// A catalog attached to a provisioned engine. Each row renders to one Trino
/// `<name>.properties` file; `kind` selects the connector translation and
/// `config` holds the typed per-kind parameters.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct EngineCatalog {
    pub id: Uuid,
    pub engine_id: Uuid,
    pub name: String,
    pub kind: String,
    pub config: serde_json::Value,
    pub secret_ref: Option<Uuid>,
    pub is_primary: bool,
    pub created_at: DateTime<Utc>,
}

impl Engine {
    /// In-cluster DNS name of this engine's Trino coordinator service
    /// (`{name}-coordinator.{namespace}.svc`).
    pub fn coordinator_host(&self) -> String {
        format!("{}-coordinator.{}.svc", self.name, self.namespace)
    }

    /// The coordinator's HTTP port (fixed by the orchestrator's manifests).
    pub fn coordinator_port(&self) -> i32 {
        8080
    }

    /// Whether the engine accepts a minted per-user engine JWT — a prerequisite
    /// for materializing a delegated (read-only) workspace connection. Mirrors
    /// the orchestrator's `spec.trinoConfig.auth.jwt` object check
    /// (`egret-engine-mgmt/src/engines.rs::resolve_auth`).
    pub fn supports_jwt_delegation(&self) -> bool {
        self.spec
            .get("trinoConfig")
            .and_then(|tc| tc.get("auth"))
            .and_then(|a| a.get("jwt"))
            .map(|j| j.is_object())
            .unwrap_or(false)
    }

    /// Delegation mode from the stored spec (`trinoConfig.delegationMode`).
    /// Defaults to `impersonate` when absent.
    pub fn delegation_mode(&self) -> &str {
        self.spec
            .get("trinoConfig")
            .and_then(|t| t.get("delegationMode"))
            .and_then(|v| v.as_str())
            .unwrap_or("impersonate")
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
    /// Impersonate mode always can; jwt mode requires JWT auth enabled.
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
    fn pool(&self) -> &sqlx::PgPool {
        &self.db.pool
    }

    // engines ------------------------------------------------------------------

    pub async fn create_engine(
        &self,
        name: &str,
        r#type: &str,
        namespace: &str,
        warehouse_id: Uuid,
        spec: &serde_json::Value,
    ) -> Result<Engine, EngineMgmtError> {
        sqlx::query_as::<_, Engine>(
            "INSERT INTO engine_mgmt.engines (name, type, namespace, warehouse_id, spec) VALUES ($1,$2,$3,$4,$5) \
             RETURNING id, name, type, namespace, warehouse_id, spec, status, authz_service_id, delegator_secret_id, tls_secret_id, coordinator_cert, created_at",
        )
        .bind(name)
        .bind(r#type)
        .bind(namespace)
        .bind(warehouse_id)
        .bind(spec)
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
            "SELECT id, name, type, namespace, warehouse_id, spec, status, authz_service_id, delegator_secret_id, tls_secret_id, coordinator_cert, created_at \
             FROM engine_mgmt.engines ORDER BY created_at",
        )
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn get_engine(&self, id: Uuid) -> Result<Option<Engine>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, Engine>(
            "SELECT id, name, type, namespace, warehouse_id, spec, status, authz_service_id, delegator_secret_id, tls_secret_id, coordinator_cert, created_at \
             FROM engine_mgmt.engines WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn update_engine(
        &self,
        id: Uuid,
        spec: &serde_json::Value,
        status: &str,
    ) -> Result<(), EngineMgmtError> {
        sqlx::query("UPDATE engine_mgmt.engines SET spec = $2, status = $3 WHERE id = $1")
            .bind(id)
            .bind(spec)
            .bind(status)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    pub async fn set_engine_status(&self, id: Uuid, status: &str) -> Result<(), EngineMgmtError> {
        sqlx::query("UPDATE engine_mgmt.engines SET status = $2 WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(self.pool())
            .await?;
        Ok(())
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

    pub async fn delete_engine(&self, id: Uuid) -> Result<(), EngineMgmtError> {
        sqlx::query("DELETE FROM engine_mgmt.engines WHERE id = $1")
            .bind(id)
            .execute(self.pool())
            .await?;
        Ok(())
    }

    // engine catalogs ----------------------------------------------------------

    pub async fn create_engine_catalog(
        &self,
        engine_id: Uuid,
        name: &str,
        kind: &str,
        config: &serde_json::Value,
        secret_ref: Option<Uuid>,
        is_primary: bool,
    ) -> Result<EngineCatalog, EngineMgmtError> {
        sqlx::query_as::<_, EngineCatalog>(
            "INSERT INTO engine_mgmt.engine_catalogs (engine_id, name, kind, config, secret_ref, is_primary) \
             VALUES ($1,$2,$3,$4,$5,$6) \
             RETURNING id, engine_id, name, kind, config, secret_ref, is_primary, created_at",
        )
        .bind(engine_id)
        .bind(name)
        .bind(kind)
        .bind(config)
        .bind(secret_ref)
        .bind(is_primary)
        .fetch_one(self.pool())
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d)
                if d.constraint() == Some("engine_catalogs_engine_id_name_key") =>
            {
                EngineMgmtError::AlreadyExists(name.to_string())
            }
            _ => EngineMgmtError::Db(e),
        })
    }

    pub async fn list_engine_catalogs(
        &self,
        engine_id: Uuid,
    ) -> Result<Vec<EngineCatalog>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, EngineCatalog>(
            "SELECT id, engine_id, name, kind, config, secret_ref, is_primary, created_at \
             FROM engine_mgmt.engine_catalogs WHERE engine_id = $1 ORDER BY is_primary DESC, created_at",
        )
        .bind(engine_id)
        .fetch_all(self.pool())
        .await?)
    }

    pub async fn get_engine_catalog(
        &self,
        id: Uuid,
    ) -> Result<Option<EngineCatalog>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, EngineCatalog>(
            "SELECT id, engine_id, name, kind, config, secret_ref, is_primary, created_at \
             FROM engine_mgmt.engine_catalogs WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn update_engine_catalog(
        &self,
        id: Uuid,
        config: &serde_json::Value,
        secret_ref: Option<Uuid>,
    ) -> Result<Option<EngineCatalog>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, EngineCatalog>(
            "UPDATE engine_mgmt.engine_catalogs SET config = $2, secret_ref = $3 WHERE id = $1 \
             RETURNING id, engine_id, name, kind, config, secret_ref, is_primary, created_at",
        )
        .bind(id)
        .bind(config)
        .bind(secret_ref)
        .fetch_optional(self.pool())
        .await?)
    }

    pub async fn delete_engine_catalog(&self, id: Uuid) -> Result<(), EngineMgmtError> {
        sqlx::query("DELETE FROM engine_mgmt.engine_catalogs WHERE id = $1")
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

    /// Engines assigned to a workspace.
    pub async fn list_engines_for_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<Engine>, EngineMgmtError> {
        Ok(sqlx::query_as::<_, Engine>(
            "SELECT e.id, e.name, e.type, e.namespace, e.warehouse_id, e.spec, e.status, e.authz_service_id, e.delegator_secret_id, e.tls_secret_id, e.coordinator_cert, e.created_at \
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
            warehouse_id: Uuid::nil(),
            spec: serde_json::json!({ "trinoConfig": {} }),
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: None,
            created_at: chrono::Utc::now(),
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
            warehouse_id: Uuid::nil(),
            spec: serde_json::json!({ "trinoConfig": { "delegationMode": "jwt", "auth": { "jwt": {} } } }),
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: None,
            created_at: chrono::Utc::now(),
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
            warehouse_id: Uuid::nil(),
            spec: serde_json::json!({ "trinoConfig": { "delegationMode": "jwt" } }),
            status: "provisioning".into(),
            authz_service_id: None,
            delegator_secret_id: None,
            tls_secret_id: None,
            coordinator_cert: None,
            created_at: chrono::Utc::now(),
        };
        assert!(e.is_jwt_delegation());
        assert!(!e.is_impersonate_delegation());

        e.spec = serde_json::json!({ "trinoConfig": {} }); // default = impersonate
        assert!(!e.is_jwt_delegation());
    }
}
