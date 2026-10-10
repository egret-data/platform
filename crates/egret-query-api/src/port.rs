//! `QueryExecPort` — the only thing the Data API seam knows about query
//! execution.
//!
//! `egret-dataapi` holds an `Arc<dyn QueryExecPort>` and never depends on
//! `egret-query`, so the Data API can move to its own binary by swapping the
//! implementation. This mirrors `DataApiEndpointPort` in `egret-dataapi-api`.
//!
//! Deliberately minimal, and it grows only alongside its callers: P3 carried
//! `describe_as` alone because validate-before-publish was the only consumer,
//! and P4 added `execute_as` with the serving path that calls it. A trait method
//! with no consumer is untested surface — do not add one ahead of its caller.
//!
//! Both methods carry the SAME scoping obligation (`database_id` must belong to
//! `ws`, refused as `NotFound`) and the same by-variant error mapping. An
//! implementation should share one prelude between them rather than keep two
//! copies of the delegation resolution.

use uuid::Uuid;

use crate::{ColumnMeta, Limits, Param, TabularResult};

#[async_trait::async_trait]
pub trait QueryExecPort: Send + Sync {
    /// Resolve the output column schema of `sql` without fetching rows, running
    /// as `principal` so a delegated connection resolves under that identity's
    /// Ranger policy rather than the service account's.
    ///
    /// Used by validate-before-publish to freeze the output shape into the
    /// snapshot, so the published contract cannot drift when the underlying
    /// table changes.
    ///
    /// # Scoping — an implementation MUST honour this
    ///
    /// `database_id` typically arrives from a request body, so it is
    /// attacker-controlled. Every implementation **must** verify that
    /// `database_id` belongs to `ws` before connecting, and report a
    /// non-belonging or absent id as [`egret_core::Error::NotFound`] — absence
    /// and denial look identical, so the call cannot be used to probe which
    /// connections exist in other workspaces. `ws` is a scoping argument, not a
    /// hint. The port is fail-closed on this by design: a caller that forgets its
    /// own check is still safe.
    ///
    /// The *permission* half is the caller's: the port has no `WorkspaceContext`
    /// and does not evaluate `query:run`.
    ///
    /// # Errors
    ///
    /// The distinction matters to the endpoint author and must be preserved:
    ///
    /// * [`Error::Forbidden`] — the caller's policy denies the table. This is the
    ///   reason `principal` exists; never flatten it into `InvalidInput`.
    /// * [`Error::NotFound`] — the database is not in `ws`, or the relation the
    ///   SQL names does not exist.
    /// * [`Error::InvalidInput`] — the SQL is the problem, with the engine's
    ///   message attached so the author can fix it.
    /// * [`Error::Internal`] — infrastructure. Never carries engine addresses to
    ///   the client.
    ///
    /// [`Error::Forbidden`]: egret_core::Error::Forbidden
    /// [`Error::NotFound`]: egret_core::Error::NotFound
    /// [`Error::InvalidInput`]: egret_core::Error::InvalidInput
    /// [`Error::Internal`]: egret_core::Error::Internal
    async fn describe_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
    ) -> Result<Vec<ColumnMeta>, egret_core::Error>;

    /// [`Self::describe_as`], for a statement that carries `param_count` bind
    /// markers and no values.
    ///
    /// The Data API's publish path renders an endpoint's template before it
    /// describes it, so the statement handed to the engine is the one that will
    /// actually be served — `?` markers and all. That statement cannot go through
    /// [`Self::describe_as`]: **on StarRocks the text path given SQL containing
    /// `?` returns zero columns and NO error** (measured, P4.5 §3.4), so publish
    /// would freeze an empty schema instead of failing. Both engines can type a
    /// parameterised statement without values — Trino via `DESCRIBE OUTPUT`,
    /// StarRocks via `prep()` — which is what this routes to.
    ///
    /// Every obligation on [`Self::describe_as`] holds unchanged: the workspace
    /// scope check, running as `principal`, and the by-variant error mapping.
    ///
    /// # Why it is defaulted, and why the default REFUSES
    ///
    /// The same reasoning as [`Self::execute_as_with_params`], which see: the
    /// trait has 17 implementation sites, 16 of them hand-written test fakes
    /// (`execution::port_impl`'s `EngineState` is the only production one —
    /// counted with `grep -rn "QueryExecPort for" crates/`, W4 Task 3 fix round
    /// 1; the previous "ten … nine" was stale and had been copied outward), and a
    /// default that quietly ignored `param_count` would let every fake answer a
    /// parameterised describe with its canned columns while nothing exercised the
    /// parameter-aware path at all. So it delegates only where delegating is
    /// exactly equivalent — `param_count == 0` — and otherwise refuses.
    async fn describe_as_with_params(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
        param_count: usize,
    ) -> Result<Vec<ColumnMeta>, egret_core::Error> {
        if param_count == 0 {
            self.describe_as(ws, database_id, principal, sql).await
        } else {
            Err(egret_core::Error::Internal(anyhow::anyhow!(
                "this QueryExecPort implementation cannot describe a parameterised statement"
            )))
        }
    }

    /// Run `sql` as `principal` and return the rows, honouring `limits`.
    ///
    /// The serving half of the port: the Data API's request path calls this
    /// after resolving an endpoint snapshot, so the rows come back under the
    /// caller's identity and therefore under *their* engine policy.
    ///
    /// # Scoping — an implementation MUST honour this
    ///
    /// Identical to [`Self::describe_as`], and for the same reason: this refuses
    /// a `database_id` not granted to `ws` with [`egret_core::Error::NotFound`],
    /// **before** resolving the principal and before any network call, so
    /// absence and denial are indistinguishable and the call cannot be used to
    /// probe another workspace's connections. Executing is strictly more
    /// dangerous than describing — a missed check here returns another
    /// workspace's *data*, not just its column names.
    ///
    /// The caller still owes the PERMISSION half: the port has no
    /// `WorkspaceContext` and does not evaluate `query:run`.
    ///
    /// # Truncation
    ///
    /// `limits.max_rows` is reported, not silently applied: a result that hit
    /// the cap comes back with [`TabularResult::truncated`] set, so the caller
    /// can distinguish a complete answer from a prefix of one.
    ///
    /// # Errors
    ///
    /// Engine failures are mapped by variant, not flattened — the same table
    /// [`Self::describe_as`] uses:
    ///
    /// * [`Error::Forbidden`] — the caller's policy denies the data. This is why
    ///   `principal` exists; never flatten it into `InvalidInput`, or an author
    ///   whose grant is missing is sent to debug SQL that is already correct.
    /// * [`Error::NotFound`] — the database is not in `ws`, or the relation the
    ///   SQL names does not exist.
    /// * [`Error::InvalidInput`] — the SQL is the problem, with the engine's
    ///   message attached.
    /// * [`Error::Internal`] — infrastructure. Never carries engine addresses to
    ///   the client.
    ///
    /// [`Error::Forbidden`]: egret_core::Error::Forbidden
    /// [`Error::NotFound`]: egret_core::Error::NotFound
    /// [`Error::InvalidInput`]: egret_core::Error::InvalidInput
    /// [`Error::Internal`]: egret_core::Error::Internal
    async fn execute_as(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
        limits: Limits,
    ) -> Result<TabularResult, egret_core::Error>;

    /// [`Self::execute_as`], with `params` bound by the engine's own parameter
    /// mechanism rather than interpolated into `sql`.
    ///
    /// The Data API serving path calls this for every request once spec §6.2
    /// step 7 has rendered the endpoint's definition: the rendered statement
    /// carries one `?` per value and `params` carries the values, in the order
    /// their markers appear. Caller text never reaches the SQL.
    ///
    /// Every obligation on [`Self::execute_as`] — the workspace scope check, the
    /// by-variant error mapping, reporting truncation rather than applying it —
    /// holds here unchanged.
    ///
    /// # Why it is defaulted, and why the default REFUSES
    ///
    /// Defaulted so the nine hand-written test fakes implementing this trait
    /// keep compiling. But a default that quietly *ignored* `params` and
    /// executed the statement anyway would be the worst of both: a fake would
    /// return its canned rows, every parameterised assertion written against it
    /// would pass, and nothing would have exercised binding at all. So the
    /// default delegates only in the case where delegating is exactly
    /// equivalent — `params.is_empty()` — and otherwise refuses.
    ///
    /// This mirrors `egret_query::execution::connector::Connector::execute_with_params`
    /// deliberately, including its reasoning: this codebase already lost a SEV-0
    /// to a defaulted inherited behaviour in `describe`. A fake that wants to
    /// take part in a parameterised test has to say so by overriding, and one
    /// that does not says so by failing the request loudly.
    ///
    /// # Errors
    ///
    /// The table from [`Self::execute_as`], plus [`egret_core::Error::Internal`]
    /// from the default when `params` is non-empty.
    async fn execute_as_with_params(
        &self,
        ws: Uuid,
        database_id: Uuid,
        principal: egret_core::UserId,
        sql: &str,
        params: &[Param],
        limits: Limits,
    ) -> Result<TabularResult, egret_core::Error> {
        if params.is_empty() {
            self.execute_as(ws, database_id, principal, sql, limits)
                .await
        } else {
            Err(egret_core::Error::Internal(anyhow::anyhow!(
                "this QueryExecPort implementation does not support bound parameters"
            )))
        }
    }
}
