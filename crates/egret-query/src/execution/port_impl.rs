//! `QueryExecPort` over the concrete `EngineState`. The Data API seam holds this
//! as `Arc<dyn QueryExecPort>` and never names `egret-query`.

use std::time::Duration;

use uuid::Uuid;

use egret_core::{ColumnMeta, Error, UserId};

use crate::execution::connector::{build_connector, ConnSpec};
use crate::execution::delegation_for;
use crate::execution::{EngineApiError, EngineError, EngineState};

/// `describe` is called directly, with no JobManager wrapping it, so it carries
/// its own timeout — the same reason `Connector::describe` takes one.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Map an engine failure onto the core error the endpoint author will see.
///
/// A blanket `InvalidInput` would be a lie on the exact axis this port exists
/// for: `describe_as` runs under the caller's identity precisely so a delegated
/// connection resolves under *their* Ranger policy, and a policy denial arrives
/// as [`EngineError::Forbidden`]. Flattening that to a 400 would tell an author
/// "invalid input: forbidden" when their SQL is perfect and their grant is
/// missing. Likewise `Connect` carries the coordinator's host:port, which has no
/// business in a client-facing 400 body — it is an operator problem (500).
///
/// A pure function over `EngineError` so the distinction is unit-testable
/// without a live engine, and so adding a variant forces a decision here.
fn describe_error(e: EngineError) -> Error {
    let detail = e.to_string();
    match e {
        // The caller's policy denied the table. This is the whole point of
        // running as `principal`; it must stay a 403.
        EngineError::Forbidden => Error::Forbidden,
        // A missing catalog/schema/table the caller named.
        EngineError::NotFound(s) => Error::NotFound(s),
        // The author's SQL (or the shape of it) is the problem — a 400 they can
        // act on. `Timeout` is here because for a `LIMIT 0` describe it means the
        // query is too expensive to plan, not that the server is broken.
        EngineError::BadRequest(s) | EngineError::Engine(s) | EngineError::Limit(s) => {
            Error::InvalidInput(s)
        }
        EngineError::Timeout => Error::InvalidInput("describe timed out".into()),
        // Infrastructure. Nothing the author can do, and the detail (host:port,
        // SQL state) must not reach them — `Error::Internal` redacts to
        // "internal error" on the wire.
        // Unreachable today: `describe` builds its own `CancellationToken` and
        // never triggers it. Internal rather than a 400, so if it ever does fire
        // it reads as the bug it would be.
        EngineError::Connect(_)
        | EngineError::Storage(_)
        | EngineError::Db(_)
        | EngineError::Cancelled => Error::Internal(anyhow::anyhow!("describe: {detail}")),
    }
}

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
        ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, Error> {
        // Fail closed BEFORE anything else: `database_id` reaches this port from a
        // request body, so an unscoped describe would let an author point an
        // endpoint at another workspace's connection. This is the second half of
        // `routes::authorize` (the permission half stays with the caller, which
        // has the WorkspaceContext this port does not). `get_in_workspace` reports
        // a foreign row as NotFound, so absence and denial look identical — the
        // same shape used everywhere else in this feature.
        self.databases
            .get_in_workspace(ws, database_id)
            .await
            .map_err(|e| match e {
                crate::registry::DatabaseError::NotFound(s) => Error::NotFound(s),
                other => Error::Internal(anyhow::anyhow!("resolve database: {other}")),
            })?;
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
        build_connector(&spec)
            .describe(sql, DESCRIBE_TIMEOUT)
            .await
            .map_err(describe_error)
    }
}

#[cfg(test)]
mod tests {
    use super::describe_error;
    use crate::execution::EngineError;
    use egret_core::Error;

    /// The finding this exists for: a Ranger deny on the table is a 403, not
    /// "your SQL is bad". `describe_as` runs under the caller's identity so the
    /// engine applies THEIR policy — collapsing the denial into `InvalidInput`
    /// would misreport the one failure the port's `principal` argument exists to
    /// produce.
    #[test]
    fn a_policy_denial_stays_forbidden() {
        assert!(
            matches!(describe_error(EngineError::Forbidden), Error::Forbidden),
            "a Ranger deny must surface as Forbidden, not InvalidInput"
        );
    }

    #[test]
    fn a_missing_relation_stays_not_found() {
        let e = describe_error(EngineError::NotFound("tpch.tiny.nope".into()));
        match e {
            Error::NotFound(s) => assert_eq!(s, "tpch.tiny.nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    /// Bad SQL is the author's mistake and must carry its message through.
    #[test]
    fn engine_and_bad_request_are_the_authors_problem() {
        for e in [
            EngineError::Engine("line 1:8: mismatched input 'FROM'".into()),
            EngineError::BadRequest("nope".into()),
            EngineError::Limit("too many rows".into()),
        ] {
            assert!(
                matches!(describe_error(e), Error::InvalidInput(_)),
                "author-facing engine failures must be InvalidInput"
            );
        }
        assert!(matches!(
            describe_error(EngineError::Timeout),
            Error::InvalidInput(_)
        ));
    }

    /// Infrastructure failures are 500s, and must NOT leak the coordinator's
    /// host:port into a client-facing 400 body.
    #[test]
    fn connect_failures_are_internal_and_do_not_leak_the_host() {
        let e = describe_error(EngineError::Connect(
            "error sending request for url (https://trino.internal:8443/v1/statement)".into(),
        ));
        assert!(
            matches!(e, Error::Internal(_)),
            "a connect failure is an operator problem, not the author's"
        );
        // The variant is what keeps the host out of the response: `Internal`
        // renders 500 + a redacted body, whereas `InvalidInput` would have put
        // this string verbatim into a 400.
        assert_eq!(
            e.status_code(),
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        );
        // The detail survives for the log, just not for the client.
        assert!(e.to_string().contains("trino.internal:8443"));
    }

    #[test]
    fn cancelled_is_internal_not_a_client_error() {
        assert!(matches!(
            describe_error(EngineError::Cancelled),
            Error::Internal(_)
        ));
    }
}
