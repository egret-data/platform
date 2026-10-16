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

/// Map an engine failure raised by **`describe`** onto the core error the
/// endpoint author will see.
///
/// A blanket `InvalidInput` would be a lie on the exact axis this port exists
/// for: both port verbs run under the caller's identity precisely so a delegated
/// connection resolves under *their* Ranger policy, and a policy denial arrives
/// as [`EngineError::Forbidden`]. Flattening that to a 400 would tell an author
/// "invalid input: forbidden" when their SQL is perfect and their grant is
/// missing. Likewise `Connect` carries the coordinator's host:port, which has no
/// business in a client-facing 400 body — it is an operator problem (500).
///
/// A pure function over `EngineError` so the distinction is unit-testable
/// without a live engine, and so adding a variant forces a decision here.
///
/// # This used to be shared with `execute_as`, and the sharing was the defect
///
/// One table served both verbs until Package 3b. The two rows that could not be
/// shared are `Timeout` and the infrastructure group, and the reason is the same
/// for both: **the sentence they need to produce is addressed to a different
/// person.** For a `LIMIT 0` describe, a timeout means the query is too
/// expensive to plan — the author's problem, a 400 they can act on, which is why
/// this arm is unchanged. For an execution it means the caller's own published
/// budget was exceeded, which is a 504. See [`execute_error`], and
/// `docs/vi/specs/2026-08-14-data-api-design.md` §7, which named this split as
/// requirement (a) of the widening.
fn describe_error(e: EngineError) -> Error {
    let detail = e.to_string();
    match e {
        // The caller's policy denied the table. This is the whole point of
        // running as `principal`; it must stay a 403.
        EngineError::Forbidden => Error::Forbidden,
        // A missing catalog/schema/table the caller named.
        EngineError::NotFound(s) => Error::NotFound(s),
        // The author's SQL (or the shape of it) is the problem — a 400 they can
        // act on.
        EngineError::BadRequest(s) | EngineError::Engine(s) | EngineError::Limit(s) => {
            Error::InvalidInput(s)
        }
        // For a `LIMIT 0` describe this means the query is too expensive to
        // PLAN — the query, not the server, is what has to change. Deliberately
        // NOT what `execute_error` does with the same variant.
        EngineError::Timeout => Error::InvalidInput("query timed out".into()),
        // Infrastructure. Nothing the author can do, and the detail (host:port,
        // SQL state) must not reach them — `Error::Internal` redacts to
        // "internal error" on the wire.
        // `Cancelled` is unreachable on this path: `describe_as` never triggers
        // its token. Internal rather than a 400, so if it ever does fire it reads
        // as the bug it would be.
        EngineError::AuthFailed(_)
        | EngineError::Connect(_)
        | EngineError::Storage(_)
        | EngineError::Db(_)
        | EngineError::Cancelled => Error::Internal(anyhow::anyhow!("query port: {detail}")),
    }
}

/// Map an engine failure raised by **`execute`** onto the core error.
///
/// Identical to [`describe_error`] except for the two groups whose cause is not
/// the author's SQL, which are the two the Data API's serving surface could
/// never distinguish:
///
/// * **`Timeout`** — the statement outran the budget its caller passed in
///   `Limits`. Reported as a typed [`ExecFailureKind::Timeout`] rather than as
///   `InvalidInput("query timed out")`, so a consumer classifies it by value.
///   `describe_error` keeps the old mapping, for the reason written there.
/// * **`Connect`** — the engine could not be reached. Reported as a typed
///   [`ExecFailureKind::EngineUnavailable`]. It is the ONLY variant that earns
///   that tag: it is produced by the connectors themselves, from a failed
///   socket, TLS handshake or HTTP send to the coordinator.
///
/// Both stay `Error::Internal` on the wire, so **no existing consumer changes
/// shape**: still a 500, still redacted to `"internal error"`, still logged with
/// the full detail. The type is recovered only by a consumer that asks for it
/// (`ExecFailure::kind_of`). See `egret_query_api::failure` for why the widening
/// is additive rather than a new port error type.
///
/// # What is deliberately NOT typed
///
/// `Db`, `Storage` and `Cancelled` are egret's own faults, not the engine's, so
/// they stay an untyped `Internal` — a consumer that turned them into
/// `engine_unavailable` would send an operator to look at Trino for a bug in
/// egret. `Storage` used to be tagged with `Connect`, and it does not belong
/// there: it wraps `egret_storage::StorageError`, egret's own object-store
/// client, so a failing S3 would have answered `502 engine_unavailable` and sent
/// an operator to a query engine that was healthy. That is exactly the claim the
/// 502 is supposed to make honest. And
/// `Engine`/`BadRequest`/`Limit` stay `InvalidInput` carrying the engine's own
/// words: that message is the one thing the endpoint author can act on, the
/// preview loop shows it to them, and at the serving seam `InvalidInput` is
/// provably the only variant whose text came from the engine — which is what
/// lets the serving router classify it without a typed marker.
fn execute_error(e: EngineError) -> Error {
    let detail = e.to_string();
    match e {
        EngineError::Forbidden => Error::Forbidden,
        EngineError::NotFound(s) => Error::NotFound(s),
        EngineError::BadRequest(s) | EngineError::Engine(s) | EngineError::Limit(s) => {
            Error::InvalidInput(s)
        }
        EngineError::Timeout => {
            egret_query_api::ExecFailure::new(egret_query_api::ExecFailureKind::Timeout, detail)
                .into_error()
        }
        EngineError::Connect(_) => egret_query_api::ExecFailure::new(
            egret_query_api::ExecFailureKind::EngineUnavailable,
            detail,
        )
        .into_error(),
        // egret's own faults. `Storage` is egret's object-store client, not the
        // engine — see the doc comment. `Cancelled` is unreachable today:
        // `execute_as` triggers its own token only AFTER `tokio::time::timeout`
        // has already dropped the connector future — so the connector is never
        // around to observe it and report `Cancelled`. (That path surfaces as
        // `Timeout` instead, mapped above.)
        // `AuthFailed`: egret's credential to the engine was refused. An operator
        // problem, not the author's SQL and not a policy denial; the engine's
        // reason is in `detail`, which `Internal` logs and redacts on the wire.
        EngineError::AuthFailed(_)
        | EngineError::Db(_)
        | EngineError::Storage(_)
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
        let connector = build_connector(&spec);
        describe_bounded(connector.describe(sql, DESCRIBE_TIMEOUT)).await
    }

    async fn describe_as_with_params(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: UserId,
        sql: &str,
        param_count: usize,
    ) -> Result<Vec<ColumnMeta>, Error> {
        // Same prelude, same scoping guarantee, same error table as `describe_as`
        // — only the connector call differs, and it differs because the text path
        // given a `?` answers with zero columns and no error on StarRocks.
        let spec = connect_as(self, ws, database_id, principal).await?;
        let connector = build_connector(&spec);
        describe_bounded(connector.describe_with_params(sql, param_count, DESCRIBE_TIMEOUT)).await
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
/// `describe_as` has its own, [`describe_bounded`], for the reason written
/// there — the sentence that used to sit here (that `Connector::describe` taking
/// a timeout argument was enough) was false for Trino.
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
/// Bound a `describe`, the way [`run_bounded`] bounds an execution.
///
/// **The `timeout` argument `Connector::describe` takes is not a wall clock on
/// this side of Trino.** `TrinoConnector` turns it into a per-REQUEST `reqwest`
/// timeout, and one describe is several requests: the statement POST, one
/// `nextUri` poll per page, and — since this package — a sleep between 503
/// retries. Each is individually bounded and the total was not, so a coordinator
/// answering `503, Retry-After: <n>` to every attempt could hold a
/// publish-validate for as long as it liked. StarRocks' own `describe` already
/// wraps itself in `tokio::time::timeout`; this is the same bound applied on the
/// caller's side, where it covers both engines and both describe verbs.
///
/// The value is [`DESCRIBE_TIMEOUT`] — the same number already handed to the
/// connector, so nothing that finishes today starts failing: this only ends the
/// case where the sum of the parts outran the budget every part was measured
/// against. It is exactly what `run_bounded` does with `limits.timeout`.
///
/// The error is mapped through [`describe_error`], so an expiry reads as the
/// author's `400 "query timed out"` here and NOT as the serving surface's 504 —
/// `describe` runs at publish time, under a human, and that mapping is the one
/// thing this package deliberately left alone.
async fn describe_bounded(
    fut: impl std::future::Future<Output = Result<Vec<ColumnMeta>, EngineError>>,
) -> Result<Vec<ColumnMeta>, Error> {
    match tokio::time::timeout(DESCRIBE_TIMEOUT, fut).await {
        Ok(r) => r.map_err(describe_error),
        Err(_) => Err(describe_error(EngineError::Timeout)),
    }
}

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
        Ok(r) => r.map_err(execute_error),
        Err(_) => {
            cancel.cancel();
            Err(execute_error(EngineError::Timeout))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{describe_error, execute_error};
    use crate::execution::EngineError;
    use egret_core::{ColumnMeta, Error};
    use egret_query_api::{ExecFailure, ExecFailureKind};

    /// The finding this exists for: a Ranger deny on the table is a 403, not
    /// "your SQL is bad". Both port verbs run under the caller's identity so the
    /// engine applies THEIR policy — collapsing the denial into `InvalidInput`
    /// would misreport the one failure the port's `principal` argument exists to
    /// produce.
    #[test]
    fn a_policy_denial_stays_forbidden() {
        assert!(
            matches!(describe_error(EngineError::Forbidden), Error::Forbidden),
            "a Ranger deny must surface as Forbidden, not InvalidInput"
        );
        assert!(
            matches!(execute_error(EngineError::Forbidden), Error::Forbidden),
            "and the split must not have changed that on the execute path"
        );
    }

    #[test]
    fn an_engine_login_failure_is_not_a_policy_denial() {
        for e in [
            describe_error(EngineError::AuthFailed("jwks".into())),
            execute_error(EngineError::AuthFailed("jwks".into())),
        ] {
            assert!(
                matches!(e, Error::Internal(_)),
                "engine 401 must not read as Forbidden: {e:?}"
            );
        }
    }

    #[test]
    fn a_missing_relation_stays_not_found() {
        for e in [
            describe_error(EngineError::NotFound("tpch.tiny.nope".into())),
            execute_error(EngineError::NotFound("tpch.tiny.nope".into())),
        ] {
            match e {
                Error::NotFound(s) => assert_eq!(s, "tpch.tiny.nope"),
                other => panic!("expected NotFound, got {other:?}"),
            }
        }
    }

    /// Bad SQL is the author's mistake and must carry its message through — on
    /// BOTH verbs. The serving router's `endpoint_query_failed` classification
    /// depends on this staying `InvalidInput`: at that seam it is provably the
    /// only variant whose text came from the engine.
    #[test]
    fn engine_and_bad_request_are_the_authors_problem() {
        for build in [
            describe_error as fn(EngineError) -> Error,
            execute_error as fn(EngineError) -> Error,
        ] {
            for e in [
                EngineError::Engine("line 1:8: mismatched input 'FROM'".into()),
                EngineError::BadRequest("nope".into()),
                EngineError::Limit("too many rows".into()),
            ] {
                assert!(
                    matches!(build(e), Error::InvalidInput(_)),
                    "author-facing engine failures must be InvalidInput"
                );
            }
        }
    }

    /// **Requirement (a) of the widening: the two verbs answer a timeout
    /// differently, and they must.**
    ///
    /// For a `LIMIT 0` describe a timeout means the query is too expensive to
    /// plan — the author's problem, a 400 carrying a message they can act on.
    /// For an execution it means the caller's own published budget was exceeded,
    /// which is nobody's input error. Sharing one table made 504 undecidable at
    /// the serving seam forever; this is the test that keeps them apart.
    ///
    /// Mutating `execute_error`'s `Timeout` arm back to
    /// `Error::InvalidInput("query timed out")` fails the second half.
    #[test]
    fn a_timeout_means_different_things_to_describe_and_to_execute() {
        assert!(
            matches!(describe_error(EngineError::Timeout), Error::InvalidInput(_)),
            "describe's timeout is still the author's 400"
        );

        let e = execute_error(EngineError::Timeout);
        assert_eq!(
            ExecFailure::kind_of(&e),
            Some(ExecFailureKind::Timeout),
            "execute's timeout must be recoverable as a value, not as a message"
        );
        // And it stays `Internal` on the wire, so nothing that does not ask for
        // the kind changes behaviour.
        assert!(matches!(e, Error::Internal(_)));
    }

    /// Infrastructure failures are 500s, and must NOT leak the coordinator's
    /// host:port into a client-facing 400 body.
    #[test]
    fn connect_failures_are_internal_and_do_not_leak_the_host() {
        for e in [
            describe_error(EngineError::Connect(
                "error sending request for url (https://trino.internal:8443/v1/statement)".into(),
            )),
            execute_error(EngineError::Connect(
                "error sending request for url (https://trino.internal:8443/v1/statement)".into(),
            )),
        ] {
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
    }

    /// The execute path additionally says *which* infrastructure failed, so an
    /// operator can be told "the engine is unreachable" rather than "internal".
    ///
    /// `Connect` and nothing else: it is the variant the connectors raise from a
    /// failed socket, TLS handshake or HTTP send to the coordinator, which is
    /// the only thing `engine_unavailable` claims.
    #[test]
    fn an_unreachable_engine_is_typed_on_the_execute_path() {
        assert_eq!(
            ExecFailure::kind_of(&execute_error(EngineError::Connect(
                "connection refused".into()
            ))),
            Some(ExecFailureKind::EngineUnavailable)
        );
    }

    /// **A failing object store is not the query engine.**
    ///
    /// `EngineError::Storage` wraps `egret_storage::StorageError` — egret's own
    /// object-store client — and it used to share the `Connect` arm, so an
    /// unreachable S3 answered the serving surface's `502 engine_unavailable`.
    /// That sends an operator to a Trino that is fine, and it is the one arm
    /// that made "the 502 is honest" untrue. It is an untyped `Internal` now,
    /// like every other egret-side fault.
    #[test]
    fn a_failing_object_store_is_not_reported_as_the_engine_being_down() {
        let mapped = execute_error(EngineError::Storage(egret_storage::StorageError::S3(
            "object store unreachable".into(),
        )));
        assert_eq!(
            ExecFailure::kind_of(&mapped),
            None,
            "egret's object store is not the query engine; a 502 here would page \
             the wrong team"
        );
        assert!(matches!(mapped, Error::Internal(_)));
    }

    /// **The describe path has a wall clock, not just a per-request one.**
    ///
    /// Driven with a future that never completes, under a paused clock, so the
    /// assertion is about the bound rather than about waiting 30 seconds for it.
    /// `tokio`'s auto-advance moves time only while the runtime is idle, which
    /// is precisely the state a hung engine leaves it in.
    ///
    /// Mutation: `fut.await.map_err(describe_error)` in place of the
    /// `tokio::time::timeout` and this test never returns — a publish-validate
    /// against a coordinator that never answers is exactly that hang, one
    /// process out.
    #[tokio::test(start_paused = true)]
    async fn a_describe_that_never_answers_is_still_bounded() {
        let started = tokio::time::Instant::now();
        let never = std::future::pending::<Result<Vec<ColumnMeta>, EngineError>>();
        let out = super::describe_bounded(never).await;
        match out {
            Err(Error::InvalidInput(m)) => assert!(
                m.contains("timed out"),
                "a describe that outran its budget must read as the author's \
                 timeout, got {m:?}"
            ),
            other => panic!("expected the describe timeout, got {other:?}"),
        }
        assert!(
            started.elapsed() >= super::DESCRIBE_TIMEOUT,
            "the bound must be DESCRIBE_TIMEOUT, not something shorter"
        );
    }

    /// **The half that keeps the 502 honest.** egret's OWN faults must stay an
    /// untyped `Internal`: a consumer that read them as `engine_unavailable`
    /// would page an operator about Trino when the metastore is what broke.
    /// `describe_error` types nothing at all, for the same reason.
    #[test]
    fn egrets_own_faults_are_never_typed_as_an_engine_failure() {
        for e in [
            EngineError::Db(sqlx::Error::PoolTimedOut),
            EngineError::Cancelled,
        ] {
            let mapped = execute_error(e);
            assert!(matches!(mapped, Error::Internal(_)));
            assert_eq!(
                ExecFailure::kind_of(&mapped),
                None,
                "an egret-side fault must not be readable as an engine failure"
            );
        }
        assert_eq!(
            ExecFailure::kind_of(&describe_error(EngineError::Connect("refused".into()))),
            None,
            "describe types nothing: only the execute path has a consumer for it"
        );
    }

    #[test]
    fn cancelled_is_internal_not_a_client_error() {
        assert!(matches!(
            describe_error(EngineError::Cancelled),
            Error::Internal(_)
        ));
        assert!(matches!(
            execute_error(EngineError::Cancelled),
            Error::Internal(_)
        ));
    }
}
