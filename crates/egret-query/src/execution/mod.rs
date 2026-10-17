//! Async query engine: connectors (Trino/StarRocks), job manager, result store.
//! Folded in from the former `egret-engine` crate (E4b).

/// DG frontend redesign #1: a catalog level listed on Trino as `egret-lineage`.
pub mod catalog_listing;
pub mod connector;
mod error;
mod job;
/// View governance B1: the `egret-lineage` EXPLAIN runner.
pub mod lineage;
pub mod model;
/// `QueryExecPort` over `EngineState` — the seam the Data API reaches execution
/// through. Private: consumers hold the trait from `egret-query-api`, never this.
mod port_impl;
pub mod routes;
pub mod store;
/// Internal SQL-Lab parameter rendering. Public only so the integration test
/// `tests/connector_starrocks.rs` can call `render` against a live engine — it
/// is not API surface, hence `#[doc(hidden)]`.
#[doc(hidden)]
pub mod template;

pub use error::{EngineApiError, EngineError};
pub use job::{run_query, JobManager};
pub use routes::{engine_router, rows_to_csv, EngineState, QueryExecutionApi};
pub use store::{Query, QueryStore};

use egret_query_api::Database;
use uuid::Uuid;

/// Resolve the per-user delegation credentials for `database_id`.
///
/// Returns `(database, session_user, bearer_token)`. For a delegated connection
/// in impersonate mode this is the caller's username and no JWT; in jwt mode it
/// is their username plus a freshly minted engine token; for a non-delegated
/// connection it is the caller's email and no token. A non-OIDC caller on a
/// delegated jwt connection is refused rather than silently falling back to
/// service credentials.
///
/// Extracted verbatim from the `submit` route so the route and
/// `QueryExecPort::describe_as` cannot drift — this logic decides *whose*
/// Ranger policy applies to a query. It returns `EngineApiError` rather than
/// the bare `EngineError` precisely because the route's original error mapping
/// went through `EngineApiError` (the mint's `NoRefreshToken` → 403, everything
/// else → 500); routing those through `EngineError` instead would silently
/// re-map both to 400.
pub(crate) async fn delegation_for(
    st: &EngineState,
    principal: egret_core::UserId,
    principal_email: &str,
    database_id: Uuid,
) -> Result<(Database, Option<String>, Option<String>), EngineApiError> {
    let db = st
        .databases
        .get(database_id)
        .await
        .map_err(|e| EngineApiError::from(EngineError::from(e)))?;
    let (session_user, bearer) = if db.delegated {
        if db.options.impersonate == Some(true) {
            // Impersonate mode: egret authenticates as the service user (basic-auth
            // from db.secret_id) and runs AS the caller via X-Trino-User. No JWT.
            let username = st
                .users
                .ensure_username(principal)
                .await
                .map_err(EngineApiError::from)?;
            (Some(username), None)
        } else {
            // jwt mode: mint the caller's short-lived engine JWT (403 if non-OIDC).
            let token = st
                .engine_minter
                .mint(principal.0)
                .await
                .map_err(|e| match e {
                    egret_analytics_api::engine_token::BrokerError::NoRefreshToken => {
                        EngineApiError::from(egret_core::Error::Forbidden)
                    }
                    other => EngineApiError::from(egret_core::Error::Internal(anyhow::anyhow!(
                        other.to_string()
                    ))),
                })?;
            let username = st
                .users
                .ensure_username(principal)
                .await
                .map_err(EngineApiError::from)?;
            (Some(username), Some(token.access_token))
        }
    } else {
        (Some(principal_email.to_string()), None)
    };
    Ok((db, session_user, bearer))
}
