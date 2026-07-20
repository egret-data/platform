//! `GET /ws/{workspace}/notebook-runtime` — per-user kernel manifest (F1→D1).
//!
//! Returns the caller's stable username and the workspace's enabled
//! engines so the Daft kernel can wire itself up in one round-trip.

use axum::extract::State;
use axum::Json;
use egret_workspace::context::WsContext;

use crate::error::NotebookApiError;
use crate::model::EngineOption;
use crate::routes::NotebookState;

// `NotebookRuntime` moved verbatim to the leaf contract crate
// `egret-analytics-api` (E5c-1); re-exported so callers keep using
// `egret_notebook::NotebookRuntime`.
pub use egret_analytics_api::NotebookRuntime;

/// Build the `aud:"catalog"` claim set for a kernel, mirroring the machine-IdP
/// OAuth path (`crates/egret-machine-idp/src/lib.rs:189-198`). Pure (no crypto,
/// no DB) so the claim shape can be unit-tested in isolation. `exp_ts` is a Unix
/// timestamp (seconds).
fn catalog_claims(
    issuer: &str,
    warehouse_lakekeeper_id: &str,
    role: &str,
    exp_ts: u64,
) -> serde_json::Value {
    serde_json::json!({
        "iss": issuer.trim_end_matches('/'),
        "sub": "notebook",
        "aud": "catalog",
        "warehouse": warehouse_lakekeeper_id,
        "roles": [role],
        "exp": exp_ts,
    })
}

/// Mint an `aud:"catalog"` JWT scoped to `warehouse` + `role`, mirroring the
/// machine-IdP OAuth path (`crates/egret-machine-idp/src/lib.rs:189-198`) — a
/// local RS256 sign; no Keycloak/refresh-token dependency (works for any user).
///
/// Returns the signed JWT plus its expiry instant. The `issuer` should match the
/// machine-IdP's `IdpState.issuer` so the token is consistent with OAuth-minted
/// ones (the embedded CatalogGuard does not validate `iss`, so this is
/// best-effort consistency, not a hard requirement).
pub(crate) async fn mint_catalog_jwt(
    keys: &egret_machine_idp::PgSigningKeyStore,
    issuer: &str,
    warehouse_lakekeeper_id: &str,
    role: &str,
) -> Result<(String, chrono::DateTime<chrono::Utc>), NotebookApiError> {
    use egret_machine_idp::SigningKeyStore as _;
    let km = keys
        .active()
        .await
        .map_err(|e| NotebookApiError(egret_core::Error::Internal(anyhow::anyhow!(e))))?;
    let exp_ts = chrono::Utc::now().timestamp() as u64 + 3600;
    let claims = catalog_claims(issuer, warehouse_lakekeeper_id, role, exp_ts);
    let jwt = egret_machine_idp::sign_claims(&km, &claims)
        .map_err(|e| NotebookApiError(egret_core::Error::Internal(anyhow::anyhow!(e))))?;
    let exp = chrono::DateTime::from_timestamp(exp_ts as i64, 0).ok_or_else(|| {
        NotebookApiError(egret_core::Error::Internal(anyhow::anyhow!(
            "catalog JWT exp timestamp out of range"
        )))
    })?;
    Ok((jwt, exp))
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

    // 5. Load enabled engines (filter to enabled==true).
    let engines: Vec<EngineOption> = st
        .store
        .list_engine_options(ws)
        .await?
        .into_iter()
        .filter(|e| e.enabled)
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

    // 5c. Best-effort catalog wiring: if this workspace has a notebook warehouse
    // configured (Task 1/2), mint an `aud:"catalog"` JWT (Task 3) scoped to that
    // warehouse so the kernel's `write_iceberg` helper can write directly to the
    // Iceberg REST catalog. Any missing setting / unresolvable warehouse / mint
    // failure leaves `catalog_jwt = None` (never fails the manifest).
    let (catalog_jwt, catalog_jwt_expires_at, catalog_uri, catalog_warehouse, namespace) =
        match st.store.get_notebook_warehouse(ws).await {
            Ok(Some((wh_id, ns))) => match st.catalog.get_warehouse(wh_id).await {
                Ok(Some(wh)) => match wh.lakekeeper_warehouse_id {
                    Some(lk_id) => {
                        match mint_catalog_jwt(&st.keys, &st.idp_issuer, &lk_id, "notebook").await {
                            Ok((jwt, exp)) => (
                                Some(jwt),
                                Some(exp),
                                Some(format!("{}/catalog", st.catalog_base_url)),
                                Some(lk_id),
                                ns,
                            ),
                            Err(e) => {
                                tracing::warn!("notebook catalog token mint skipped: {}", e.0);
                                (None, None, None, None, ns)
                            }
                        }
                    }
                    None => {
                        tracing::warn!(
                            "notebook warehouse {wh_id} has no lakekeeper id; catalog token skipped"
                        );
                        (None, None, None, None, ns)
                    }
                },
                Ok(None) => (None, None, None, None, ns),
                Err(e) => {
                    tracing::warn!("notebook warehouse lookup skipped: {e}");
                    (None, None, None, None, ns)
                }
            },
            Ok(None) => (None, None, None, None, Vec::new()),
            Err(e) => {
                tracing::warn!("notebook warehouse setting lookup skipped: {e}");
                (None, None, None, None, Vec::new())
            }
        };

    // 6. Build and return the manifest.
    Ok(NotebookRuntime {
        username,
        engines,
        engine_jwt,
        engine_jwt_expires_at,
        catalog_jwt,
        catalog_jwt_expires_at,
        catalog_uri,
        catalog_warehouse,
        namespace,
    })
}

/// `GET /ws/{workspace}/notebook-runtime`
///
/// Steps:
/// 1. Require `notebook:read`.
///    2–6. Delegate to [`build_notebook_runtime`].
#[utoipa::path(
    get,
    path = "/api/ws/{workspace}/notebook-runtime",
    tag = "notebooks",
    params(egret_core::WorkspacePath),
    responses(
        (status = 200, description = "Per-user kernel runtime manifest (username, engines, engine JWT)", body = NotebookRuntime),
        (status = 401, description = "Not authenticated", body = egret_core::http::ErrorBody),
        (status = 403, description = "Missing notebook:read permission", body = egret_core::http::ErrorBody),
        (status = 500, description = "Internal error", body = egret_core::http::ErrorBody),
    ),
)]
pub async fn runtime(
    State(st): State<NotebookState>,
    WsContext(ctx): WsContext,
) -> Result<Json<NotebookRuntime>, NotebookApiError> {
    // 1. Authorization check.
    egret_core::authz::require(&*st.pdp, &ctx, "notebook:read")
        .await
        .map_err(NotebookApiError::from)?;

    let manifest = build_notebook_runtime(&st, ctx.workspace_id.0, ctx.principal.user_id).await?;

    Ok(Json(manifest))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pure claim-shape check (no DB, no crypto): the catalog claim set mirrors
    /// the machine-IdP OAuth path exactly.
    #[test]
    fn catalog_claims_shape() {
        let c = catalog_claims("http://egret.local/", "wh-lakekeeper-id", "notebook", 12345);
        assert_eq!(c["iss"], "http://egret.local"); // trailing slash trimmed
        assert_eq!(c["sub"], "notebook");
        assert_eq!(c["aud"], "catalog");
        assert_eq!(c["warehouse"], "wh-lakekeeper-id");
        assert_eq!(c["roles"], serde_json::json!(["notebook"]));
        assert_eq!(c["exp"], 12345);
    }

    /// End-to-end mint: `active()` self-creates a signing key in the test DB, so
    /// no manual seeding is needed. The minted JWT verifies against that key's
    /// public JWK and decodes to the expected catalog claims.
    #[sqlx::test(migrations = "../../migrations")]
    async fn mint_catalog_jwt_signs_verifiable_scoped_token(pool: sqlx::PgPool) {
        use egret_machine_idp::SigningKeyStore as _;

        let db = egret_metastore::Db { pool };
        let keys = egret_machine_idp::PgSigningKeyStore::new(db);

        let (jwt, exp) =
            mint_catalog_jwt(&keys, "http://egret.local/", "wh-lakekeeper-id", "notebook")
                .await
                .expect("mint catalog jwt");

        let km = keys.active().await.expect("active key");
        let claims =
            egret_machine_idp::verify_with_jwk(&jwt, &km.public_jwk).expect("verify catalog jwt");

        assert_eq!(claims["aud"], "catalog");
        assert_eq!(claims["warehouse"], "wh-lakekeeper-id");
        assert_eq!(claims["roles"], serde_json::json!(["notebook"]));
        assert_eq!(claims["iss"], "http://egret.local");
        assert_eq!(claims["sub"], "notebook");
        let exp_claim = claims["exp"].as_i64().expect("exp is a number");
        assert!(
            exp_claim > chrono::Utc::now().timestamp(),
            "exp must be in the future"
        );
        assert_eq!(exp_claim, exp.timestamp());
        assert!(exp > chrono::Utc::now());
    }
}
