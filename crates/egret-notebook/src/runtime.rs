//! `GET /ws/{workspace}/notebook-runtime` — per-user kernel manifest (F1→D1).
//!
//! Returns the caller's stable username, a scoped catalog token for their
//! private namespace, and the workspace's enabled engines/catalogs so the
//! Daft kernel can wire itself up in one round-trip.

use axum::extract::State;
use axum::Json;
use egret_workspace::context::WsContext;

use crate::error::NotebookApiError;
use crate::model::{CatalogOption, EngineOption};
use crate::provisioning::provision_notebook_access;
use crate::routes::NotebookState;

/// The catalog connection parameters for the caller's private namespace.
#[derive(serde::Serialize)]
pub struct RuntimeCatalog {
    /// Base URL of the Iceberg REST catalog (e.g. `http://localhost:8181`).
    pub url: String,
    /// The warehouse name/prefix inside the catalog.
    pub warehouse: String,
    /// The private namespace (`notebook_{username}`).
    pub namespace: String,
    /// A freshly minted, single-use catalog token scoped to this namespace.
    pub token: String,
}

/// Full kernel manifest returned by `GET /ws/{workspace}/notebook-runtime`.
#[derive(serde::Serialize)]
pub struct NotebookRuntime {
    /// The caller's stable username (email local-part, collision-suffixed).
    pub username: String,
    /// Iceberg catalog connection for the caller's private namespace.
    pub catalog: RuntimeCatalog,
    /// Workspace engines (DB connections) enabled for notebooks.
    pub engines: Vec<EngineOption>,
    /// Workspace catalogs (warehouses) enabled for notebooks.
    pub catalogs: Vec<CatalogOption>,
}

/// `GET /ws/{workspace}/notebook-runtime`
///
/// 1. Require `notebook:read`.
/// 2. Resolve (or derive) the caller's stable username.
/// 3. Pick the first warehouse row for the workspace (there is typically only
///    one; if none exists the endpoint returns 404 — the workspace must have a
///    warehouse before notebooks can be provisioned).
/// 4. Provision the private namespace + fresh 1-grant token via
///    `provision_notebook_access`.
/// 5. Load enabled engines and catalogs from the settings store.
/// 6. Return the manifest JSON.
///
/// **Warehouse selection:** we use the egret-side `workspace_warehouses` table
/// (via `CatalogStore::list_warehouses`) and pick the first row ordered by
/// name. The `catalog.warehouse` field in the response is set to that row's
/// `name` field (the human-facing warehouse name, which mirrors the lakekeeper
/// warehouse name used in catalog REST paths). If a workspace ever has multiple
/// warehouses the admin can re-order them via rename to control which one the
/// runtime picks; a future endpoint can expose the selection explicitly.
pub async fn runtime(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
) -> Result<Json<NotebookRuntime>, NotebookApiError> {
    // 1. Authorization check.
    st.authz
        .require_workspace(&ctx, "notebook:read")
        .await
        .map_err(NotebookApiError::from)?;

    let ws = ctx.workspace_id.0;
    let user_id = ctx.principal.user_id;

    // 2. Resolve stable username.
    let username = st
        .users
        .ensure_username(user_id)
        .await
        .map_err(NotebookApiError)?;

    // 3. Resolve the workspace's warehouse row (pick first by name).
    let warehouses = st.catalog.list_warehouses(ws).await.map_err(|e| {
        use egret_catalog::CatalogError as C;
        let inner = match e {
            C::NotFound(m) => egret_core::Error::NotFound(m),
            C::AlreadyExists(m) => egret_core::Error::Conflict(m),
            C::BadRequest(m) => egret_core::Error::InvalidInput(m),
            C::Db(e) => egret_core::Error::Internal(anyhow::anyhow!(e)),
        };
        NotebookApiError(inner)
    })?;

    let warehouse = warehouses.into_iter().next().ok_or_else(|| {
        NotebookApiError(egret_core::Error::NotFound(
            "no warehouse configured for this workspace".into(),
        ))
    })?;

    // 4. Provision private namespace + fresh 1-grant token.
    let prov = provision_notebook_access(
        &st.catalog,
        st.provisioner.as_ref(),
        ws,
        user_id.0,
        warehouse.id,
        &username,
    )
    .await?;

    // 5. Load enabled engines and catalogs (filter to enabled==true).
    let engines: Vec<EngineOption> = st
        .store
        .list_engine_options(ws)
        .await?
        .into_iter()
        .filter(|e| e.enabled)
        .collect();

    let catalogs: Vec<CatalogOption> = st
        .store
        .list_catalog_options(ws)
        .await?
        .into_iter()
        .filter(|c| c.enabled)
        .collect();

    // 6. Build the manifest.
    let manifest = NotebookRuntime {
        username,
        catalog: RuntimeCatalog {
            url: st.catalog_base_url.clone(),
            warehouse: warehouse.name,
            namespace: prov.namespace,
            token: prov.token,
        },
        engines,
        catalogs,
    };

    Ok(Json(manifest))
}
