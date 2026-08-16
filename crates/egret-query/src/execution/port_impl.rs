//! `QueryExecPort` over the concrete `EngineState`. The Data API seam holds this
//! as `Arc<dyn QueryExecPort>` and never names `egret-query`.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use egret_core::{ColumnMeta, Error, TabularResult, UserId};

use crate::execution::connector::{build_connector, ConnSpec};
use crate::execution::delegation_for;
use crate::execution::model::Limits;
use crate::execution::{EngineApiError, EngineError, EngineState};

/// `describe` is called directly, with no JobManager wrapping it, so it carries
/// its own timeout — the same reason `Connector::describe` takes one.
const DESCRIBE_TIMEOUT: Duration = Duration::from_secs(30);

/// Map an engine failure onto the core error the endpoint author will see.
///
/// Shared by `describe_as` and `execute_as` — deliberately one table, not two.
/// A blanket `InvalidInput` would be a lie on the exact axis this port exists
/// for: both methods run under the caller's identity precisely so a delegated
/// connection resolves under *their* Ranger policy, and a policy denial arrives
/// as [`EngineError::Forbidden`]. Flattening that to a 400 would tell an author
/// "invalid input: forbidden" when their SQL is perfect and their grant is
/// missing. Likewise `Connect` carries the coordinator's host:port, which has no
/// business in a client-facing 400 body — it is an operator problem (500).
///
/// A pure function over `EngineError` so the distinction is unit-testable
/// without a live engine, and so adding a variant forces a decision here.
///
/// Named for the *input*, not for one caller: it stopped being describe-specific
/// the moment `execute_as` started sharing it, which is why two of its strings
/// are verb-neutral.
fn engine_error(e: EngineError) -> Error {
    let detail = e.to_string();
    match e {
        // The caller's policy denied the table. This is the whole point of
        // running as `principal`; it must stay a 403.
        EngineError::Forbidden => Error::Forbidden,
        // A missing catalog/schema/table the caller named.
        EngineError::NotFound(s) => Error::NotFound(s),
        // The author's SQL (or the shape of it) is the problem — a 400 they can
        // act on. `Timeout` is here because for a `LIMIT 0` describe it means the
        // query is too expensive to plan, and for `execute_as` it means the
        // statement exceeded the budget the caller passed in `Limits` — in both
        // cases the query, not the server, is what has to change.
        EngineError::BadRequest(s) | EngineError::Engine(s) | EngineError::Limit(s) => {
            Error::InvalidInput(s)
        }
        EngineError::Timeout => Error::InvalidInput("query timed out".into()),
        // Infrastructure. Nothing the author can do, and the detail (host:port,
        // SQL state) must not reach them — `Error::Internal` redacts to
        // "internal error" on the wire.
        // `Cancelled` is unreachable today: `describe_as` never triggers its
        // token, and `execute_as` triggers its own only AFTER
        // `tokio::time::timeout` has already dropped the connector future — so
        // the connector is never around to observe it and report `Cancelled`.
        // (That path surfaces as `Timeout` instead, mapped above.) Internal
        // rather than a 400, so if it ever does fire it reads as the bug it
        // would be.
        EngineError::Connect(_)
        | EngineError::Storage(_)
        | EngineError::Db(_)
        | EngineError::Cancelled => Error::Internal(anyhow::anyhow!("query port: {detail}")),
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

/// Everything both port methods do before they touch the engine: scope the
/// database to the workspace, resolve the principal, resolve delegation, open
/// the secret, and build the connection spec.
///
/// **One function on purpose.** `describe_as` and `execute_as` are identical up
/// to the call they make, and the part they share is the security-critical part:
/// the workspace scope check and the delegation resolution that decides which
/// identity the engine sees. A second copy would be free to drift — a bearer
/// wired into one path and not the other, or (the failure P3 found by mutation)
/// a scope check present in one and absent in the other. Adding a third caller
/// means calling this, not copying it.
///
/// Returns a `ConnSpec`; the caller supplies the verb.
async fn connect_as(
    st: &EngineState,
    ws: Uuid,
    database_id: Uuid,
    principal: UserId,
) -> Result<ConnSpec, Error> {
    // Fail closed BEFORE anything else: `database_id` reaches this port from a
    // request body, so an unscoped call would let an author point an endpoint at
    // another workspace's connection — reading its schema via `describe_as`, or
    // its actual rows via `execute_as`. This is the second half of
    // `routes::authorize` (the permission half stays with the caller, which has
    // the WorkspaceContext this port does not). `get_in_workspace` reports a
    // foreign row as NotFound, so absence and denial look identical — the same
    // shape used everywhere else in this feature.
    //
    // Ordering is part of the guarantee: this runs before the principal lookup
    // and before any connection is opened, so a refused id costs no network call
    // and reveals nothing by timing.
    st.databases
        .get_in_workspace(ws, database_id)
        .await
        .map_err(|e| match e {
            crate::registry::DatabaseError::NotFound(s) => Error::NotFound(s),
            other => Error::Internal(anyhow::anyhow!("resolve database: {other}")),
        })?;
    // `delegation_for` needs the caller's email for the non-delegated arm;
    // the port is handed only a user id, so resolve it here.
    let email = st
        .users
        .find_by_id(principal)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("resolve principal: {e}")))?
        .ok_or_else(|| Error::Internal(anyhow::anyhow!("principal not found")))?
        .email;
    let (db, session_user, bearer) = delegation_for(st, principal, &email, database_id)
        .await
        .map_err(delegation_error)?;
    let secret = st
        .databases
        .open_secret(database_id)
        .await
        .map_err(|e| Error::Internal(anyhow::anyhow!("open secret: {e}")))?;
    Ok(ConnSpec::from_database_as(
        &db,
        secret,
        session_user,
        bearer,
    ))
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
        let spec = connect_as(self, ws, database_id, principal).await?;
        build_connector(&spec)
            .describe(sql, DESCRIBE_TIMEOUT)
            .await
            .map_err(engine_error)
    }

    async fn execute_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
        limits: Limits,
    ) -> Result<TabularResult, Error> {
        // Same prelude, same scoping guarantee, same error table as `describe_as`
        // — only the verb differs.
        let spec = connect_as(self, ws, database_id, principal).await?;
        run_bounded(&spec, sql, &[], limits).await
    }

    async fn execute_as_with_params(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
        params: &[egret_query_api::Param],
        limits: Limits,
    ) -> Result<TabularResult, Error> {
        // The SAME prelude as `execute_as`, for the same reason `connect_as`
        // exists at all: the workspace scope check and the delegation resolution
        // are the security-critical half, and a second copy would be free to
        // drift. The only difference between the two methods is the value of
        // `params`, which is why they share `run_bounded` rather than each
        // carrying their own timeout wrapper — the wrapper is the thing that was
        // easiest to forget here (see `run_bounded`'s comment).
        let spec = connect_as(self, ws, database_id, principal).await?;
        run_bounded(&spec, sql, params, limits).await
    }
}

/// Run a statement, bounded by `limits.timeout`.
///
/// **The wall clock lives here and nowhere below.** Neither
/// `Connector::execute` nor `Connector::execute_with_params` bounds itself:
/// both connectors' doc comments say so explicitly and name the outer caller as
/// the one that owes it. For the async SQL Lab path that caller is the
/// JobManager; the Data API serving path has no JobManager, so this is it.
///
/// The two engines fail differently, which is why the wrap is not
/// belt-and-braces:
///
/// * StarRocks never reads `limits.timeout` at all, on either path, and its
///   `mysql_async` `Opts` set no read/wait timeout — without this wrap a serving
///   request hangs until the connection dies. StarRocks is in scope for the Data
///   API, so this is the case that matters.
/// * Trino turns `limits.timeout` into a per-request `reqwest` timeout and a
///   `query_max_run_time` session property, but its `nextUri` pagination loop
///   restarts that per-request clock on every poll, so total wall-clock is
///   unbounded without this.
///
/// `describe_as` needs no equivalent only because `Connector::describe` takes a
/// timeout argument and both engines honour it.
///
/// On expiry `tokio::time::timeout` drops the inner future, so the client is
/// released either way; the `cancel()` below is deliberately kept to match
/// `job.rs::run_query` exactly, and is only load-bearing if a future caller
/// holds a clone of the token (as the JobManager does). It is NOT what stops the
/// engine-side query — for Trino that is the `query_max_run_time` session
/// property the connector derives from this same `limits.timeout`.
///
/// `limits.max_rows` is deliberately NOT applied as a SQL `LIMIT` — the
/// connector fetches `max_rows + 1` and sets `truncated`, so the caller can tell
/// a complete answer from a prefix. Rewriting the SQL would erase that
/// distinction.
///
/// `execute_with_params` is called even for an empty `params`, rather than
/// branching to `execute` here: both connectors' `execute_with_params` already
/// delegate to `execute` on an empty list (`connector/trino.rs`,
/// `connector/starrocks.rs`, and the trait's own default), so one call site
/// keeps the unparameterised path from acquiring a second, subtly different
/// spelling.
async fn run_bounded(
    spec: &ConnSpec,
    sql: &str,
    params: &[egret_query_api::Param],
    limits: Limits,
) -> Result<TabularResult, Error> {
    let cancel = CancellationToken::new();
    let connector = build_connector(spec);
    let fut = connector.execute_with_params(sql, params, &limits, cancel.clone());
    match tokio::time::timeout(limits.timeout, fut).await {
        Ok(r) => r.map_err(engine_error),
        Err(_) => {
            cancel.cancel();
            Err(engine_error(EngineError::Timeout))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::engine_error;
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
            matches!(engine_error(EngineError::Forbidden), Error::Forbidden),
            "a Ranger deny must surface as Forbidden, not InvalidInput"
        );
    }

    #[test]
    fn a_missing_relation_stays_not_found() {
        let e = engine_error(EngineError::NotFound("tpch.tiny.nope".into()));
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
                matches!(engine_error(e), Error::InvalidInput(_)),
                "author-facing engine failures must be InvalidInput"
            );
        }
        assert!(matches!(
            engine_error(EngineError::Timeout),
            Error::InvalidInput(_)
        ));
    }

    /// Infrastructure failures are 500s, and must NOT leak the coordinator's
    /// host:port into a client-facing 400 body.
    #[test]
    fn connect_failures_are_internal_and_do_not_leak_the_host() {
        let e = engine_error(EngineError::Connect(
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
            engine_error(EngineError::Cancelled),
            Error::Internal(_)
        ));
    }
}
