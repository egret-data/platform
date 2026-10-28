//! Postgres-backed database registry CRUD; seals/open credentials via SecretStore.

use egret_core::UserId;
use egret_metastore::Db;
use egret_secrets::{SecretStore, WorkspaceSecretStore};
use uuid::Uuid;

use crate::registry::error::DatabaseError;
use crate::registry::model::{ConnOptions, Database, DatabaseInput, Engine};

#[derive(Clone)]
pub struct DatabaseRepo {
    db: Db,
    secrets: SecretStore,
    workspace_secrets: WorkspaceSecretStore,
}

#[derive(sqlx::FromRow)]
pub(crate) struct Row {
    id: Uuid,
    name: String,
    engine: String,
    host: String,
    port: i32,
    options: serde_json::Value,
    secret_id: Option<Uuid>,
    status: String,
    delegated: bool,
    governed: bool,
    engine_id: Option<Uuid>,
}

/// One engine assigned to a workspace, in the ws-member read-only view (B2).
#[derive(sqlx::FromRow)]
pub struct AssignedEngineRow {
    pub id: Uuid,
    pub name: String,
    /// Engine kind (`trino`/`starrocks`/`risingwave`); aliased from `type`
    /// (a reserved word) in the query.
    pub engine_type: String,
    pub status: String,
}

/// One catalog of an assigned engine, with its storage/secret NAMES resolved
/// (never the secret value). `storage_name`/`secret_name` are best-effort:
/// `None` when the reference is absent or unresolvable (LEFT JOINs) — a jdbc
/// or generic catalog has no warehouse and therefore no storage at all.
#[derive(sqlx::FromRow)]
pub struct EngineCatalogRow {
    pub name: String,
    pub kind: String,
    pub storage_name: Option<String>,
    pub secret_name: Option<String>,
}

impl Row {
    pub(crate) fn into_database(self) -> Result<Database, DatabaseError> {
        let engine = Engine::parse(&self.engine)
            .ok_or_else(|| DatabaseError::BadRequest(format!("unknown engine: {}", self.engine)))?;
        let options: ConnOptions = serde_json::from_value(self.options).unwrap_or_default();
        Ok(Database {
            id: self.id,
            name: self.name,
            engine,
            host: self.host,
            port: self.port,
            options,
            has_secret: self.secret_id.is_some(),
            status: self.status,
            delegated: self.delegated,
            governed: self.governed,
            engine_id: self.engine_id,
        })
    }
}

impl DatabaseRepo {
    pub fn new(db: Db, secrets: SecretStore, workspace_secrets: WorkspaceSecretStore) -> Self {
        Self {
            db,
            secrets,
            workspace_secrets,
        }
    }

    /// Expose the secret store (used by routes to open a stored secret for testing).
    pub fn secrets(&self) -> &SecretStore {
        &self.secrets
    }

    /// Expose the named workspace-secret store (Epic B: routes resolve a
    /// `new_secret`/`secret_id` to a named secret id before create/update).
    pub fn workspace_secrets(&self) -> &WorkspaceSecretStore {
        &self.workspace_secrets
    }

    /// Register a new database owned by `workspace_id`, binding the already-resolved
    /// named secret id (Epic B: the route resolves `new_secret`/`secret_id` to a
    /// `workspace_secrets` id; the repo just binds it — no sealing here).
    ///
    /// ADR-19 per-workspace model: the row is stamped with its owning workspace
    /// so workspace-scoped reads can filter it.
    pub async fn create_in_workspace(
        &self,
        workspace_id: Uuid,
        input: &DatabaseInput,
        secret_id: Option<Uuid>,
        created_by: Option<UserId>,
    ) -> Result<Database, DatabaseError> {
        let opts = serde_json::to_value(&input.options).unwrap_or(serde_json::json!({}));
        sqlx::query_as::<_, Row>(
            "INSERT INTO query.databases (name, engine, host, port, options, secret_id, created_by, workspace_id, delegated, governed) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             RETURNING id, name, engine, host, port, options, secret_id, status, delegated, governed, engine_id",
        )
        .bind(&input.name)
        .bind(input.engine.as_str())
        .bind(&input.host)
        .bind(input.port)
        .bind(opts)
        .bind(secret_id)
        .bind(created_by.map(|u| u.0))
        .bind(workspace_id)
        .bind(input.delegated)
        // CREATE has nothing to keep, so an absent `governed` is `false` — see
        // [`DatabaseInput::governed`] for why absence means something different
        // on UPDATE.
        .bind(input.governed.unwrap_or(false))
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                DatabaseError::AlreadyExists(input.name.clone())
            }
            _ => DatabaseError::Db(e),
        })?
        .into_database()
    }

    /// Materialize a read-only, per-user-delegated connection for a provisioned
    /// engine assigned to a workspace. `delegated = true` so the query path mints
    /// the caller's engine JWT (jwt mode) or impersonates via the delegator secret
    /// (impersonate mode); `engine_id` marks the row read-only + links the source
    /// engine. `secret_id` binds the (global) delegator credential for impersonate
    /// mode, or `None` for jwt mode.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_provisioned(
        &self,
        workspace_id: Uuid,
        engine_id: Uuid,
        name: &str,
        engine: &str,
        host: &str,
        port: i32,
        options: &serde_json::Value,
        secret_id: Option<Uuid>,
        created_by: Option<UserId>,
    ) -> Result<Database, DatabaseError> {
        sqlx::query_as::<_, Row>(
            "INSERT INTO query.databases \
               (name, engine, host, port, options, secret_id, created_by, workspace_id, delegated, engine_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, true, $9) \
             RETURNING id, name, engine, host, port, options, secret_id, status, delegated, governed, engine_id",
        )
        .bind(name)
        .bind(engine)
        .bind(host)
        .bind(port)
        .bind(options)
        .bind(secret_id)
        .bind(created_by.map(|u| u.0))
        .bind(workspace_id)
        .bind(engine_id)
        .fetch_one(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                DatabaseError::AlreadyExists(name.to_string())
            }
            _ => DatabaseError::Db(e),
        })?
        .into_database()
    }

    /// Refresh the materialized delegated connection(s) for an engine
    /// assignment in a workspace *in place* — used when the engine's delegation
    /// mode / TLS changes, so each workspace's stored `host`/`port`/`options`/
    /// `secret_id` match the new mode. Updating the existing row (rather than
    /// delete + recreate) preserves the row id, so chart/saved-query references
    /// to the connection survive the change. Idempotent; a no-op when no such
    /// row exists.
    ///
    /// The whole `options` JSONB is REPLACED (not merged) by design: these rows
    /// are system-owned (delegated, `engine_id` set) and their options are fully
    /// derived from the engine's delegation mode, so there is no user-set option
    /// to preserve.
    pub async fn update_provisioned_by_engine(
        &self,
        workspace_id: Uuid,
        engine_id: Uuid,
        host: &str,
        port: i32,
        options: &serde_json::Value,
        secret_id: Option<Uuid>,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE query.databases \
             SET host = $3, port = $4, options = $5, secret_id = $6 \
             WHERE workspace_id = $1 AND engine_id = $2",
        )
        .bind(workspace_id)
        .bind(engine_id)
        .bind(host)
        .bind(port)
        .bind(options)
        .bind(secret_id)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }

    /// Remove the materialized connection(s) for an engine assignment in a
    /// workspace (used when unassigning the engine). Idempotent.
    pub async fn delete_by_engine(
        &self,
        workspace_id: Uuid,
        engine_id: Uuid,
    ) -> Result<(), DatabaseError> {
        sqlx::query("DELETE FROM query.databases WHERE workspace_id = $1 AND engine_id = $2")
            .bind(workspace_id)
            .bind(engine_id)
            .execute(&self.db.pool)
            .await?;
        Ok(())
    }

    pub async fn get(&self, id: Uuid) -> Result<Database, DatabaseError> {
        sqlx::query_as::<_, Row>(
            "SELECT id, name, engine, host, port, options, secret_id, status, delegated, governed, engine_id \
             FROM query.databases WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| DatabaseError::NotFound(id.to_string()))?
        .into_database()
    }

    /// The workspace that owns database `id`, or `None` when no row has that
    /// id. Unscoped on purpose: the caller compares the answer with its own
    /// workspace (see [`egret_query_api::DatabaseOwnerPort`]).
    pub async fn workspace_of(&self, id: Uuid) -> Result<Option<Uuid>, DatabaseError> {
        Ok(
            sqlx::query_scalar("SELECT workspace_id FROM query.databases WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?,
        )
    }

    /// Databases owned by a workspace (ordered by name).
    ///
    /// Ownership query (ADR-19 per-workspace model): rows are filtered by their
    /// own `workspace_id` column. Used by the workspace router and the query
    /// engine to scope visibility.
    pub async fn list_for_workspace(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<Database>, DatabaseError> {
        let rows = sqlx::query_as::<_, Row>(
            "SELECT id, name, engine, host, port, options, secret_id, status, delegated, governed, engine_id \
             FROM query.databases WHERE workspace_id = $1 ORDER BY name",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?;
        rows.into_iter().map(Row::into_database).collect()
    }

    /// Open the stored plaintext secret for a database (used by the connection
    /// tester and the delegated impersonate query path). The named secret (Epic B)
    /// is resolved from the row's owning `workspace_id`, accepting either a
    /// workspace-scoped secret OR a GLOBAL (`workspace_id NULL`) one — the delegator
    /// credential is provisioned globally but referenced by a workspace database.
    /// NULL `secret_id` (or no row) → `Ok(None)`.
    pub async fn open_secret(&self, id: Uuid) -> Result<Option<String>, DatabaseError> {
        let row: Option<(Option<Uuid>, Uuid)> =
            sqlx::query_as("SELECT secret_id, workspace_id FROM query.databases WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.db.pool)
                .await?;
        match row {
            Some((Some(secret_id), workspace_id)) => {
                let (_user, pw) = self
                    .workspace_secrets
                    .resolve_basic_auth_ws_or_global(workspace_id, secret_id)
                    .await?;
                Ok(Some(pw))
            }
            _ => Ok(None),
        }
    }

    /// The current `secret_id` bound to a database owned by `workspace_id`
    /// (used by the update route to keep the existing named secret when neither
    /// `new_secret` nor `secret_id` is supplied). `NotFound` if the row is absent
    /// or owned by another workspace (IDOR closed); `Ok(None)` if it has no secret.
    pub async fn current_secret_id(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<Option<Uuid>, DatabaseError> {
        let row: Option<Option<Uuid>> = sqlx::query_scalar(
            "SELECT secret_id FROM query.databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?;
        row.ok_or_else(|| DatabaseError::NotFound(id.to_string()))
    }

    /// Fetch a database only if it belongs to `workspace_id`.
    ///
    /// IDOR-safe: a row owned by another workspace (or absent) is reported as
    /// `NotFound`, never leaked.
    pub async fn get_in_workspace(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<Database, DatabaseError> {
        sqlx::query_as::<_, Row>(
            "SELECT id, name, engine, host, port, options, secret_id, status, delegated, governed, engine_id \
             FROM query.databases WHERE id = $1 AND workspace_id = $2",
        )
        .bind(id)
        .bind(workspace_id)
        .fetch_optional(&self.db.pool)
        .await?
        .ok_or_else(|| DatabaseError::NotFound(id.to_string()))?
        .into_database()
    }

    /// Update a database scoped to `workspace_id`, binding the already-resolved
    /// named secret id (Epic B: the route resolves `new_secret`/`secret_id`,
    /// defaulting to the existing `secret_ref` for keep-current; the repo just
    /// binds the resulting id — no sealing or rotation cleanup here, since named
    /// secrets are shared and owned by `WorkspaceSecretStore`).
    ///
    /// Ownership is checked first (404 before any mutation), and every statement
    /// is constrained to the owning workspace.
    pub async fn update_in_workspace(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        input: &DatabaseInput,
        secret_id: Option<Uuid>,
        _actor: Option<UserId>,
    ) -> Result<Database, DatabaseError> {
        self.get_in_workspace(workspace_id, id).await?; // 404 if not owned, before any mutation
        let opts = serde_json::to_value(&input.options).unwrap_or(serde_json::json!({}));

        // `governed = COALESCE($10, governed)` and NOT `governed = $10`. This is
        // a whole-row update, so every other column here is overwritten by
        // whatever the body carried — including its `serde` default when the
        // body omitted it. For a security flag that is a control a client which
        // predates the field CLEARS by accident, editing a port; see
        // [`crate::registry::model::DatabaseInput::governed`]. NULL means "leave
        // it", so only an explicit `false` turns it off, and the keep-current
        // value is read and written in ONE statement rather than across a
        // read-modify-write with a window in it.
        sqlx::query(
            "UPDATE query.databases SET name=$2, engine=$3, host=$4, port=$5, options=$6, \
                 secret_id=$7, delegated=$9, governed=COALESCE($10, governed), updated_at=now() \
               WHERE id=$1 AND workspace_id=$8",
        )
        .bind(id)
        .bind(&input.name)
        .bind(input.engine.as_str())
        .bind(&input.host)
        .bind(input.port)
        .bind(opts)
        .bind(secret_id)
        .bind(workspace_id)
        .bind(input.delegated)
        .bind(input.governed)
        .execute(&self.db.pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => {
                DatabaseError::AlreadyExists(input.name.clone())
            }
            _ => DatabaseError::Db(e),
        })?;

        self.get_in_workspace(workspace_id, id).await
    }

    /// Delete a database scoped to `workspace_id`.
    ///
    /// Ownership is checked first (404 if not owned), then the row is deleted
    /// under the same workspace constraint. The referenced NAMED secret (Epic B)
    /// is left intact — it is shared and owned by `WorkspaceSecretStore`; removing
    /// a connection only drops its reference (decrementing the secret's
    /// `used_by`), it does not delete the secret.
    pub async fn delete_in_workspace(
        &self,
        workspace_id: Uuid,
        id: Uuid,
    ) -> Result<(), DatabaseError> {
        self.get_in_workspace(workspace_id, id).await?; // 404 if not owned
        sqlx::query("DELETE FROM query.databases WHERE id = $1 AND workspace_id = $2")
            .bind(id)
            .bind(workspace_id)
            .execute(&self.db.pool)
            .await
            .map_err(|e| match &e {
                // The Data API (P3) FK's dataapi.endpoints.database_id to this
                // table with ON DELETE RESTRICT, deliberately: a published
                // endpoint's database vanishing silently is worse than the
                // delete being refused. Map that violation to a 409 the caller
                // can act on, rather than letting it fall through to `Db(_)`'s
                // generic 500 — the constraint name is a implementation detail,
                // not the message.
                sqlx::Error::Database(d) if d.is_foreign_key_violation() => {
                    DatabaseError::Conflict(
                        "database is in use by one or more Data API endpoints; \
                         remove those endpoints before deleting this database"
                            .to_string(),
                    )
                }
                _ => DatabaseError::Db(e),
            })?;
        Ok(())
    }

    // ── B2: ws-member read-only view of assigned engines ────────────────────
    //
    // A workspace member may see the egret-provisioned engines assigned to its
    // workspace, read-only, with the storage/secret NAME each catalog maps to.
    // These are cross-seam reads (engine_mgmt / catalog / storage / secrets):
    // raw sqlx here avoids taking a dependency on those crates from egret-query.

    /// Engines assigned to `workspace_id` (via `engine_mgmt.engine_workspaces`),
    /// ordered by name. `type` is aliased to `engine_type` (reserved word).
    pub async fn list_assigned_engines(
        &self,
        workspace_id: Uuid,
    ) -> Result<Vec<AssignedEngineRow>, DatabaseError> {
        Ok(sqlx::query_as::<_, AssignedEngineRow>(
            "SELECT e.id, e.name, e.type AS engine_type, e.status \
               FROM engine_mgmt.engines e \
               JOIN engine_mgmt.engine_workspaces ew ON ew.engine_id = e.id \
              WHERE ew.workspace_id = $1 \
              ORDER BY e.name",
        )
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// A single engine, but ONLY if it is assigned to `workspace_id`
    /// (`Ok(None)` → the route returns 404, never leaking an unassigned engine).
    pub async fn get_assigned_engine(
        &self,
        workspace_id: Uuid,
        engine_id: Uuid,
    ) -> Result<Option<AssignedEngineRow>, DatabaseError> {
        Ok(sqlx::query_as::<_, AssignedEngineRow>(
            "SELECT e.id, e.name, e.type AS engine_type, e.status \
               FROM engine_mgmt.engines e \
               JOIN engine_mgmt.engine_workspaces ew ON ew.engine_id = e.id \
              WHERE ew.workspace_id = $1 AND e.id = $2",
        )
        .bind(workspace_id)
        .bind(engine_id)
        .fetch_optional(&self.db.pool)
        .await?)
    }

    /// Catalogs attached to an engine (SP3 registry) that are VISIBLE to
    /// `workspace_id` (SP3 §4.6, catalog-registry review N2), with the legacy
    /// `kind` label the workspace view shows (`egret-warehouse` or the Trino
    /// connector name), the NAME of the storage connection THIS catalog's own
    /// warehouse writes to, and the NAME of one secret the catalog references
    /// (`None` when none; the value is never read). Ordered by name.
    ///
    /// `storage_name` is resolved per catalog, through that catalog's mirrored
    /// `warehouse_id` — not from the engine's primary warehouse. Copying the
    /// engine's storage onto every row was true when an engine had exactly one
    /// warehouse catalog, and became wrong the moment this list started
    /// returning other warehouses and jdbc/generic catalogs (review F-8): a
    /// non-primary row showed a confidently wrong storage, and a jdbc catalog
    /// — which has no warehouse and no storage — showed one anyway.
    ///
    /// Visibility mirrors the two-branch shape
    /// `RegistryStore::warehouse_visible_to_workspace` encodes (assigned OR
    /// owned-and-not-yet-assigned), adapted for a catalog row that is already
    /// known to exist here (unlike that function's own "no registry catalog
    /// yet" branch, which can never fire for a row we are already iterating —
    /// calling it verbatim per-row would have made the fallback dead code):
    ///   (a) assigned — a `registry.catalog_workspaces` row for (this catalog,
    ///       this workspace); or
    ///   (b) NOT (a) for anyone yet, AND this is a warehouse catalog
    ///       (`c.warehouse_id IS NOT NULL`) whose warehouse is owned by this
    ///       workspace (`catalog.warehouses.workspace_id`) — the fallback for
    ///       a warehouse catalog created before its `catalog_workspaces` row
    ///       was written (the backfill/creation path has not reached that
    ///       step for it yet).
    /// A catalog assigned to some OTHER workspace only (not this one, and not
    /// unassigned) matches neither branch and is absent, even though it is
    /// still attached to the engine. This is READ VISIBILITY only, not
    /// enforcement — the engine itself still answers a direct query for it.
    ///
    /// The `kind` CASE re-implements, in SQL, the same connector-name mapping
    /// `egret_catalog_registry::CatalogDef::trino_connector_name` computes in
    /// Rust for rendering (`crates/egret-catalog-registry/src/config.rs`) — this
    /// is a DISPLAY-ONLY read path with no `CatalogDef` at hand, not a second
    /// source of truth. If `trino_connector_name`'s mapping changes, this CASE
    /// must change with it (SP3 catalog-registry review M6).
    pub async fn list_engine_catalogs(
        &self,
        engine_id: Uuid,
        workspace_id: Uuid,
    ) -> Result<Vec<EngineCatalogRow>, DatabaseError> {
        Ok(sqlx::query_as::<_, EngineCatalogRow>(
            "SELECT c.name, \
                    CASE WHEN c.warehouse_id IS NOT NULL THEN 'egret-warehouse' \
                         WHEN c.kind = 'jdbc' THEN c.config->>'dialect' \
                         WHEN c.kind = 'generic' THEN COALESCE(c.advanced->'trino'->>'connector.name', 'generic') \
                         ELSE c.kind END AS kind, \
                    sc.name AS storage_name, \
                    (SELECT min(ws.name) FROM registry.catalog_secret_refs r \
                       JOIN workspace_secrets ws ON ws.id = r.secret_id \
                      WHERE r.catalog_id = c.id) AS secret_name \
               FROM registry.engine_catalog_attachments a \
               JOIN registry.catalogs c ON c.id = a.catalog_id \
               LEFT JOIN catalog.warehouses wh ON wh.id = c.warehouse_id \
               LEFT JOIN storage.storage_connections sc ON sc.id = wh.storage_conn_id \
              WHERE a.engine_id = $1 \
                AND ( \
                     EXISTS (SELECT 1 FROM registry.catalog_workspaces cw \
                              WHERE cw.catalog_id = c.id AND cw.workspace_id = $2) \
                  OR (c.warehouse_id IS NOT NULL \
                      AND NOT EXISTS (SELECT 1 FROM registry.catalog_workspaces cw2 \
                                       WHERE cw2.catalog_id = c.id) \
                      AND EXISTS (SELECT 1 FROM catalog.warehouses w \
                                   WHERE w.id = c.warehouse_id AND w.workspace_id = $2)) \
                ) \
              ORDER BY c.name",
        )
        .bind(engine_id)
        .bind(workspace_id)
        .fetch_all(&self.db.pool)
        .await?)
    }

    /// Record the last connection-test result, scoped to the owning workspace
    /// (defense-in-depth: the status write cannot touch another workspace's row).
    pub async fn set_status(
        &self,
        workspace_id: Uuid,
        id: Uuid,
        status: &str,
    ) -> Result<(), DatabaseError> {
        sqlx::query(
            "UPDATE query.databases SET status=$3, updated_at=now() WHERE id=$1 AND workspace_id=$2",
        )
        .bind(id)
        .bind(workspace_id)
        .bind(status)
        .execute(&self.db.pool)
        .await?;
        Ok(())
    }
}

/// The Workflow seam's `RefKind::Database` resolver reaches this through the
/// leaf port, never through `egret-query` itself. `self.workspace_of(id)`
/// below is the INHERENT method (inherent methods win over trait methods), so
/// this is a delegation, not a recursion.
#[async_trait::async_trait]
impl egret_query_api::DatabaseOwnerPort for DatabaseRepo {
    async fn workspace_of(&self, id: Uuid) -> Result<Option<Uuid>, egret_core::Error> {
        DatabaseRepo::workspace_of(self, id)
            .await
            .map_err(|e| egret_core::Error::Internal(anyhow::anyhow!("query.databases: {e}")))
    }
}
