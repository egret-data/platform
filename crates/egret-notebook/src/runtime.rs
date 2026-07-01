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
    /// Short-lived per-user engine JWT the kernel presents to query engines.
    /// `None` when the caller has no stored refresh token (e.g. password/local
    /// users or OIDC logins without an offline token) — minting is best-effort
    /// since no P1 engine consumes this yet.
    pub engine_jwt: Option<String>,
    /// Expiry of `engine_jwt` (RFC3339); kernels re-mint before this time.
    /// `None` whenever `engine_jwt` is `None`.
    pub engine_jwt_expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// The reserved warehouse name that designates a workspace's notebook (read-write)
/// Iceberg catalog. Other warehouses are "shared" (Trino/StarRocks).
const NOTEBOOK_WAREHOUSE_NAME: &str = "notebook";

/// Pick the notebook catalog warehouse: prefer the one named `notebook`, else the
/// first (back-compat). Generic over the name accessor so it is unit-testable
/// without constructing a full `Warehouse`.
pub(crate) fn select_notebook_warehouse<W, F>(warehouses: Vec<W>, name_of: F) -> Option<W>
where
    F: Fn(&W) -> &str,
{
    let idx = warehouses
        .iter()
        .position(|w| name_of(w) == NOTEBOOK_WAREHOUSE_NAME)
        .unwrap_or(0);
    warehouses.into_iter().nth(idx)
}

/// Build the [`NotebookRuntime`] manifest for `(ws, user_id)` without performing
/// any HTTP authorization check. Steps 2–6 of the `runtime` handler.
///
/// The HTTP handler is responsible for calling `require_workspace` before
/// invoking this function. This split allows `egret-kernel` (and future
/// callers) to reuse manifest construction without going through the HTTP
/// layer.
pub async fn build_notebook_runtime(
    st: &NotebookState,
    ws: uuid::Uuid,
    user_id: egret_core::UserId,
) -> Result<NotebookRuntime, NotebookApiError> {
    // 2. Resolve stable username.
    let username = st
        .users
        .ensure_username(user_id)
        .await
        .map_err(NotebookApiError)?;

    // 3. Resolve the workspace's notebook warehouse (prefer the one named "notebook", else first).
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

    let warehouse =
        select_notebook_warehouse(warehouses, |w| w.name.as_str()).ok_or_else(|| {
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

    // 5b. Best-effort mint of a short-lived per-user engine JWT for the kernel.
    // Users without a stored refresh token (password/local logins, OIDC logins
    // without an offline token) cannot mint one; that's not an error here since
    // no P1 engine consumes this JWT yet — leave the field empty in that case.
    let (engine_jwt, engine_jwt_expires_at) = match st.engine_minter.mint(user_id.0).await {
        Ok(tok) => (Some(tok.access_token), Some(tok.expires_at)),
        Err(e) => {
            tracing::warn!("notebook engine token mint skipped: {e}");
            (None, None)
        }
    };

    // 6. Build and return the manifest.
    Ok(NotebookRuntime {
        username,
        catalog: RuntimeCatalog {
            url: st.catalog_base_url.clone(),
            warehouse: warehouse.name,
            namespace: prov.namespace,
            token: prov.token,
        },
        engines,
        catalogs,
        engine_jwt,
        engine_jwt_expires_at,
    })
}

/// `GET /ws/{workspace}/notebook-runtime`
///
/// Steps:
/// 1. Require `notebook:read`.
///    2–6. Delegate to [`build_notebook_runtime`].
///
/// **Warehouse selection:** we use the egret-side `workspace_warehouses` table
/// (via `CatalogStore::list_warehouses`) and select the warehouse named
/// `notebook` (the reserved read-write notebook catalog), falling back to the
/// first row when none is named `notebook`. The `catalog.warehouse` field in the
/// response is set to that row's `name` field (the human-facing warehouse name,
/// which mirrors the lakekeeper warehouse name used in catalog REST paths). An
/// admin designates the notebook catalog by naming a warehouse `notebook`.
pub async fn runtime(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
) -> Result<Json<NotebookRuntime>, NotebookApiError> {
    // 1. Authorization check.
    st.authz
        .require_workspace(&ctx, "notebook:read")
        .await
        .map_err(NotebookApiError::from)?;

    let manifest = build_notebook_runtime(&st, ctx.workspace_id.0, ctx.principal.user_id).await?;

    Ok(Json(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_notebook_named_warehouse() {
        let whs = vec!["shared", "notebook", "other"];
        assert_eq!(select_notebook_warehouse(whs, |s| *s), Some("notebook"));
    }

    #[test]
    fn falls_back_to_first_when_no_notebook_warehouse() {
        let whs = vec!["shared", "other"];
        assert_eq!(select_notebook_warehouse(whs, |s| *s), Some("shared"));
    }

    #[test]
    fn none_when_empty() {
        let whs: Vec<&str> = Vec::new();
        assert_eq!(select_notebook_warehouse(whs, |s| *s), None);
    }
}
