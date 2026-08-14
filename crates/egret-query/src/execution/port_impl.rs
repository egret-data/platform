//! `QueryExecPort` over the concrete `EngineState`. The Data API seam holds this
//! as `Arc<dyn QueryExecPort>` and never names `egret-query`.

use std::time::Duration;

use uuid::Uuid;

use egret_core::{ColumnMeta, Error, UserId};

use crate::execution::connector::{build_connector, ConnSpec};
use crate::execution::delegation_for;
use crate::execution::{EngineApiError, EngineState};

/// `describe` is called directly, with no JobManager wrapping it, so it carries
/// its own timeout — the same reason `Connector::describe` takes one.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Collapse the route-shaped error the shared delegation path returns back to a
/// core `Error`. The `Auth` arm already carries the exact core error the route
/// would have rendered (notably `Forbidden` for a non-OIDC caller on a delegated
/// jwt connection), so it passes through unchanged rather than being flattened
/// into a 500.
fn delegation_error(e: EngineApiError) -> Error {
    match e {
        EngineApiError::Auth(err) => err,
        EngineApiError::Domain(d) => Error::Internal(anyhow::anyhow!("delegation: {d}")),
    }
}

#[async_trait::async_trait]
impl egret_query_api::QueryExecPort for EngineState {
    async fn describe_as(
        &self,
        _ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        // `delegation_for` needs the caller's email for the non-delegated arm;
        // the port is handed only a user id, so resolve it here.
        let email = self
            .users
            .find_by_id(principal)
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("resolve principal: {e}")))?
            .ok_or_else(|| Error::Internal(anyhow::anyhow!("principal not found")))?
            .email;
        let (db, session_user, bearer) = delegation_for(self, principal, &email, database_id)
            .await
            .map_err(delegation_error)?;
        let secret = self
            .databases
            .open_secret(database_id)
            .await
            .map_err(|e| Error::Internal(anyhow::anyhow!("open secret: {e}")))?;
        let spec = ConnSpec::from_database_as(&db, secret, session_user, bearer);
        // A bad SQL string is the endpoint author's mistake, so it must reach
        // them as a 400 (`InvalidInput`). `Error::Internal` would render a 500
        // and tell them nothing.
        build_connector(&spec)
            .describe(sql, DESCRIBE_TIMEOUT)
            .await
            .map_err(|e| Error::InvalidInput(e.to_string()))
    }
}
