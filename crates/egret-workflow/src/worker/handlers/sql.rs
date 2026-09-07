//! `egret.sql` — run a statement on one of the workspace's registered engines.
//!
//! # This handler ships with NO oracle coverage, and that is measured
//!
//! Every other handler in this module is judged by `tests/executor.rs` against
//! Kestra's own corpus. `egret.sql` reaches **zero** corpus entries — W3b Task 1
//! measured it, and the spec (§3.1) records why: the programme spec's original
//! handler list (`core.log`, `egret.sql`) predates the corpus and names neither
//! `core.debug.Return` (115 entries) nor `core.execution.Fail` (55). Kestra has
//! no `egret.sql`, so no golden entry can ever exercise this file.
//!
//! **A green ratchet therefore says nothing about this handler.** What it rests
//! on instead is `tests/sql_starrocks_live.rs`, which runs it against a real
//! StarRocks, and the unit tests at the bottom of this file. `oracle-exec/README.md`
//! separates the two in its coverage statement.
//!
//! # The two ports this is built on, and why they are ports
//!
//! [`egret_query_api::QueryExecPort`] and nothing else. The programme spec §6.4
//! says *"handlers depend on port traits, never on concrete crates, exactly as
//! `egret-dataapi` holds `Arc<dyn QueryExecPort>` and does not know
//! `egret-query`"*, and this crate follows that literally: `egret-query` appears
//! only in `[dev-dependencies]`, for the live test.
//!
//! The port is **fail-closed on workspace scoping**: it verifies that
//! `database_id` belongs to `ws` before connecting and reports a non-belonging
//! or absent id as `NotFound`, so a flow naming another workspace's connection
//! cannot read it and cannot tell absence from denial. This handler does not
//! repeat that check, and it must not — a second copy is a second thing that can
//! drift. What it does owe is the identity, below.
//!
//! # No value in `sql:` is ever written into the SQL
//!
//! `{{ … }}` inside `sql:` renders through [`crate::runners::env::render_bound`],
//! which is `egret-dataapi`'s `bind_formatter` idiom applied to this crate's
//! engine: the formatter writes `?` and pushes the value onto a bind list, so
//! **there is no path from an expression to SQL text**. That is structural, not
//! an escaping convention — the interpolating code path does not exist for this
//! property. A workflow's `sql:` is deliberately not the third member of the
//! `quote_string` family (`egret-bi/src/chartquery.rs` and
//! `egret-query/src/execution/template.rs` are the other two, and both were call
//! sites of the P1 backslash vector).
//!
//! Two consequences an author can see, both stated rather than left to be found:
//!
//! * **An identifier cannot be bound.** `ORDER BY ?` is a syntax error on both
//!   engines, so `ORDER BY {{ inputs.col }}` does not work and there is no
//!   `| ident` escape hatch here. The Data API has one only because an endpoint
//!   author declares an `allowed` list the render can emit *the author's own
//!   string* from; a flow declares no such list, so the fail-closed answer is
//!   that there is none. A dynamic column list belongs in `{% if %}` branches
//!   over literal SQL.
//! * **`sql:` renders ONCE**, where every other property renders in Kestra's
//!   re-render loop. See [`crate::runners::env::render_bound`] for why: the loop
//!   *is* double evaluation, and binding is what removes it.
//!
//! # Whose identity the statement runs as
//!
//! [`QueryExecPort::execute_as`] takes a `principal`, and a workflow runs with
//! nobody logged in. `egret-automation` already answered this for the same
//! situation and its answer is followed here rather than re-derived: an alert
//! runs as its **owner**, and an owner whose credentials cannot be resolved
//! makes the run **fail rather than fall back** (`evaluator.rs`'s
//! `the_token_is_minted_for_the_owner_and_nobody_else` and
//! `an_owner_with_no_mintable_token_fails_instead_of_falling_back`). Running as
//! the service instead is the escalation that design exists to prevent, and it
//! would look like a flow that simply kept working.
//!
//! So this handler runs as [`TaskInput::principal`], which `worker::claim` reads
//! from the flow's owner, and **refuses when there is none**. See that field for
//! what "the flow's owner" is today and what it should become.
//!
//! # What it produces
//!
//! `Outcome::outputs` is `Some(...)` on every success — never `None` and never
//! an absent key. W3b measured on `kestra/kestra:v2.0.0-rc13` that `{{ outputs.a }}`
//! renders `{}` for a task with no output object, so "no output" is `{}`; a
//! `fetch: NONE` therefore yields `{}` rather than nothing.
//!
//! | `fetch:` | output |
//! |---|---|
//! | `NONE` (default) | `{}` |
//! | `FETCH_ONE` | `{"size": 1, "row": {…}}`, or `{"size": 0}` when the statement returned no rows |
//! | `FETCH` | `{"size": n, "rows": [{…}, …]}` |
//!
//! `size` is defined as **the number of rows this task put into its own
//! output** — 0 or 1 for `FETCH_ONE`, `rows.len()` for `FETCH` — rather than as
//! the engine's row count, so it is derivable from the output beside it and the
//! two cannot disagree. A row is an object keyed by column name; a result with
//! two columns of the same name fails the task rather than silently dropping
//! one, because a JSON object cannot hold both and the loss would be invisible.
//!
//! `NONE` is the default because an output is **persisted**: `task_runs.outputs`
//! is a JSONB column and every later render context in the execution carries it.
//! A flow that only needs the statement to have run should not be paying to
//! store and re-render its rows.

use std::sync::Arc;

use egret_core::{Error, Result};
use egret_query_api::{ExecFailureKind, Limits, Param, QueryExecPort, TabularResult};
use serde::Deserialize;
use uuid::Uuid;

use super::super::{Outcome, TaskHandler, TaskInput};

/// The `type:` a flow writes. egret's own short spelling, and the one
/// `crate::spec::registry::TASK_TYPES` already publishes under.
pub const TYPE_ID: &str = "egret.sql";

/// The most rows `fetch: FETCH` will put in an output.
///
/// Not a performance number: the output is written to `task_runs.outputs`
/// (JSONB) and read back into the render context of **every** later task in the
/// execution, so an unbounded `FETCH` is an unbounded row in Postgres that every
/// subsequent render pays for. A result that hits the cap **fails the task**
/// rather than being truncated — see [`Sql::run`].
pub const MAX_FETCH_ROWS: usize = 10_000;

/// The wall-clock budget one statement gets when the author asked for none.
///
/// **A fallback now, not a ceiling, and the sentence that used to be here was
/// the hole W4 Task 2 closed.** It read: *"A constant rather than the task's
/// own `timeout:`, and the reason is a seam rather than a choice:
/// `models::Task`'s deserializer takes `timeout` out of a leaf's `spec` before
/// a handler ever sees it, because it belongs to the executor
/// (`executor::advance` reads it). Wiring the two together means carrying it on
/// `TaskInput`, which is W4's business."* Two things in it were wrong and one
/// is now done:
///
/// * the field does **not** belong to the executor — measured on the artifact,
///   `timeout:` is enforced by the WORKER (`WorkerTaskCallable`), and Kestra's
///   own validator says the property "is only relevant for runnable tasks";
///   `executor::plan::PlanTask::timeout` is `None` on every task and
///   `advance()` never reads one;
/// * the wiring is done: [`TaskInput::timeout`] carries the author's rendered
///   budget, and this constant is what a task with no `timeout:` gets.
pub const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// What the task does with the rows.
///
/// The three names are Kestra's JDBC `fetchType` spellings, so an author coming
/// from there is not surprised, under egret's own shorter key. `STORE` is
/// deliberately absent: it writes a result set to internal storage, which W9
/// owns and this crate has no way to reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
pub enum Fetch {
    /// Run it; produce `{}`.
    #[default]
    #[serde(rename = "NONE")]
    None,
    /// Produce the first row under `row`.
    #[serde(rename = "FETCH_ONE")]
    One,
    /// Produce every row under `rows`.
    #[serde(rename = "FETCH")]
    All,
}

/// `egret.sql`'s own properties.
///
/// `deny_unknown_fields` on purpose. A misspelled `fetch_type:` that was simply
/// ignored would run the statement and produce `{}`, and the author would be
/// debugging an empty `{{ outputs.q.rows }}` rather than a typo.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    /// The registered connection, as a UUID.
    ///
    /// **A UUID because `publish` froze it.** `spec::registry::TASK_TYPES` gives
    /// `egret.sql` one `RefField` — `database`, `RefKind::Database` — and
    /// `publish::resolve_task` rewrites the author's name to the resolved id in
    /// place, after refusing an object owned by another workspace. A value that
    /// is not a UUID therefore means this execution is running unresolved
    /// source, and the task fails saying so rather than guessing a lookup this
    /// handler has no port for.
    database: String,
    /// The statement. A template; every `{{ … }}` in it is **bound**.
    sql: String,
    #[serde(default)]
    fetch: Fetch,
}

/// `egret.sql`, over a [`QueryExecPort`].
pub struct Sql {
    port: Arc<dyn QueryExecPort>,
    max_rows: usize,
    timeout: std::time::Duration,
}

impl std::fmt::Debug for Sql {
    /// The bounds, not the port: `dyn QueryExecPort` has no `Debug` and
    /// requiring one would put a bound on every implementation for one line of
    /// diagnostics. Same decision as `Handlers`'s.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sql")
            .field("max_rows", &self.max_rows)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Sql {
    /// The shipping handler: [`MAX_FETCH_ROWS`] and [`DEFAULT_TIMEOUT`].
    pub fn new(port: Arc<dyn QueryExecPort>) -> Self {
        Self {
            port,
            max_rows: MAX_FETCH_ROWS,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Same handler with different bounds, so a test can drive the cap without
    /// materialising ten thousand rows to do it.
    pub fn with_limits(
        port: Arc<dyn QueryExecPort>,
        max_rows: usize,
        timeout: std::time::Duration,
    ) -> Self {
        Self {
            port,
            max_rows,
            timeout,
        }
    }

    /// What one port failure means, by VARIANT rather than by message.
    ///
    /// Three answers, and the split is the port's own (`egret-query-api`'s
    /// `failure` module exists to make it possible without matching on text):
    ///
    /// * **The engine said so** — [`ExecFailureKind`], i.e. unreachable or out
    ///   of time. A **failed task**, so the author's own `retry:` can act. The
    ///   `detail` is deliberately dropped: it may name a coordinator's
    ///   `host:port`, and this message is stored on the run and shown to the
    ///   author, which is exactly why `egret_core` redacts an `Internal` at the
    ///   HTTP boundary.
    /// * **The author said so** — `Forbidden` (their grant), `NotFound` (a
    ///   connection this workspace does not hold, or a relation that is not
    ///   there) and `InvalidInput` (the engine's own words about the statement).
    ///   A **failed task**, message and all: that text is the one useful thing
    ///   in an authoring loop.
    /// * **egret said so** — a bare `Internal` with no [`ExecFailure`] inside
    ///   it: an unopenable secret, a metastore that is down. A **worker
    ///   failure**, which aborts the tick and leaves the execution where it was,
    ///   because the flow is not wrong and failing it would blame the author for
    ///   egret's outage.
    fn mapped(&self, e: Error, budget: std::time::Duration) -> Result<Outcome> {
        if let Some(kind) = egret_query_api::ExecFailure::kind_of(&e) {
            return Ok(failed(match kind {
                ExecFailureKind::EngineUnavailable => {
                    format!("{TYPE_ID}: the engine could not be reached")
                }
                ExecFailureKind::Timeout => format!(
                    "{TYPE_ID}: the statement outran its {}s budget",
                    budget.as_secs()
                ),
            }));
        }
        if matches!(e, Error::Internal(_)) {
            return Err(Error::Internal(anyhow::anyhow!(
                "{TYPE_ID}: the query seam failed for a reason that is not the engine: {e}"
            )));
        }
        Ok(failed(format!("{TYPE_ID}: {e}")))
    }
}

/// A failed task, not a failed worker.
///
/// Everything an author or an operator can act on takes this path: a bad spec,
/// an unrenderable template, a database id the workspace does not hold, a
/// statement the engine refuses, a policy that denies the data, an engine that
/// is down. The alternative — `Err` — aborts the tick and leaves the run
/// `RUNNING` with a lease, which stalls the execution instead of failing it, and
/// removes the author's own `retry:` from the picture. `retry:` is the declared,
/// bounded mechanism for a transient engine; the reaper is not.
fn failed(message: impl Into<String>) -> Outcome {
    Outcome::failed_with(message)
}

#[async_trait::async_trait]
impl TaskHandler for Sql {
    fn type_id(&self) -> &'static str {
        TYPE_ID
    }

    async fn run(&self, task: &TaskInput) -> Result<Outcome> {
        let spec: Spec = match serde_json::from_value(task.spec.clone()) {
            Ok(s) => s,
            Err(e) => return Ok(failed(format!("{TYPE_ID}: {e}"))),
        };

        let Ok(database_id) = Uuid::parse_str(spec.database.trim()) else {
            return Ok(failed(format!(
                "{TYPE_ID}: `database` is {:?}, which is not a UUID. `publish` rewrites \
                 this field to the resolved connection id; an execution whose source \
                 still holds the author's name was started from unresolved source",
                spec.database
            )));
        };

        // The refusal `egret-automation` established for the same situation: an
        // unattended run whose owner cannot be resolved STOPS. Running as the
        // service account instead is the escalation, and on a `governed`
        // connection it is the one that reads another principal's rows.
        let Some(principal) = task.principal else {
            return Ok(failed(format!(
                "{TYPE_ID}: this execution has no principal to run as. A workflow runs \
                 with nobody logged in, so the statement would otherwise run as the \
                 service account and read whatever it may read; the flow's owner is \
                 what supplies it (see `TaskInput::principal`)"
            )));
        };

        let (sql, bound) = match crate::runners::env::render_bound(&spec.sql, &task.render) {
            Ok(pair) => pair,
            Err(e) => return Ok(failed(format!("{TYPE_ID}: `sql` did not render: {e}"))),
        };
        let params: Vec<Param> = bound.iter().map(to_param).collect();

        // **The author's `timeout:` if they wrote one, this handler's
        // constant if they did not.** What it buys over the worker's own
        // budget is that the ENGINE is told: on Trino `Limits::timeout`
        // becomes a `query_max_run_time` session property, so the statement is
        // cancelled server side rather than left running after this future is
        // dropped. `super::super::timeout::guard` is the outer bound and is
        // what reports a budget that expires -- both are the same instant, and
        // the outer one is armed first, so `Timeout after HH:mm:ss.SSS` is the
        // message an author sees for a `timeout:` they wrote. The message
        // below stays reachable for the engine's own deadline (a `NONE`
        // `timeout:` under this constant, or an engine that answers first).
        let budget = task.timeout.unwrap_or(self.timeout);
        let limits = Limits {
            max_rows: self.max_rows,
            timeout: budget,
        };
        let result = self
            .port
            .execute_as_with_params(
                task.workspace_id,
                database_id,
                principal,
                &sql,
                &params,
                limits,
            )
            .await;

        let result = match result {
            Ok(r) => r,
            Err(e) => return self.mapped(e, budget),
        };

        match outputs_for(spec.fetch, &result, self.max_rows) {
            Ok(outputs) => Ok(Outcome::succeeded_with(outputs)),
            Err(message) => Ok(failed(format!("{TYPE_ID}: {message}"))),
        }
    }

    /// **This handler cannot cancel, and says so instead of pretending.**
    ///
    /// `TaskHandler::cancel` has no default body precisely so that `egret.sql`
    /// could not inherit `Ok(())` from the four handlers that genuinely have
    /// nothing to stop. This one holds a statement running on a remote engine,
    /// and [`QueryExecPort`] has no cancellation method — deliberately: that
    /// trait's own doc says a method with no consumer is untested surface and
    /// must not be added ahead of its caller. Adding one here would be adding it
    /// ahead of its caller twice over, because **nothing calls `cancel` at all**
    /// yet (W4 owns the kill path).
    ///
    /// So it refuses. When W4 wires the kill path this fails loudly at the one
    /// place that has to change, rather than reporting a statement as stopped
    /// while it goes on consuming the engine.
    async fn cancel(&self, task: &TaskInput) -> Result<()> {
        Err(Error::Internal(anyhow::anyhow!(
            "{TYPE_ID}: task {} cannot be cancelled — `QueryExecPort` has no cancellation \
             method, so the statement continues on the engine after the run is abandoned. \
             W4 owns the kill path",
            task.task_id
        )))
    }
}

/// A bound JSON scalar as the engine's own bind type.
///
/// `runners::env` deliberately stops at JSON — it is the expression layer and
/// touches no I/O — so this is where the crossing happens, and it is the only
/// place in this crate that names [`Param`].
///
/// **`Param::F64` is never produced.** A JSON number that is not an `i64`
/// becomes a `Param::Decimal`, which both connectors carry as exact text
/// (`DECIMAL '…'` on Trino), so a literal an author wrote cannot arrive at the
/// engine rounded. There is no lossy arm to reach.
fn to_param(value: &serde_json::Value) -> Param {
    match value {
        serde_json::Value::Null => Param::Null,
        serde_json::Value::Bool(b) => Param::Bool(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Param::I64(i),
            None => Param::Decimal(n.to_string()),
        },
        serde_json::Value::String(s) => Param::Str(s.clone()),
        // Unreachable through `render_bound`, which refuses a sequence or a map
        // at the formatter with the `{% for %}` message. Mapped rather than
        // panicked because this function is `pub(crate)`-shaped surface and a
        // panic in a handler aborts a tick.
        other => Param::Str(other.to_string()),
    }
}

/// The output map for one result, or the reason the task must fail.
///
/// Split out of [`Sql::run`] so it can be driven without an engine: everything
/// in the table on this module's doc is decided here.
fn outputs_for(
    fetch: Fetch,
    result: &TabularResult,
    max_rows: usize,
) -> std::result::Result<serde_json::Value, String> {
    if fetch == Fetch::None {
        // The measured rule, not an omission: "no output" is `{}` and never an
        // absent key. `{{ outputs.q }}` must render; `{{ outputs.q.rows }}` must
        // fail, which it does exactly because the key is absent from an object
        // that is present.
        return Ok(serde_json::json!({}));
    }

    let names = column_names(result)?;

    match fetch {
        Fetch::None => unreachable!("returned above"),
        Fetch::One => match result.rows.first() {
            None => Ok(serde_json::json!({ "size": 0 })),
            Some(row) => Ok(serde_json::json!({
                "size": 1,
                "row": row_object(&names, row),
            })),
        },
        Fetch::All => {
            // Checked for `FETCH` alone, and that is precise rather than lazy:
            // `NONE` discards the rows and `FETCH_ONE` takes the first, so
            // neither can be made wrong by a cap the other side of it. Only
            // `FETCH` claims to have produced "the rows", and a claim that is
            // silently a prefix is the over-acceptance shape the spec's fidelity
            // rules forbid.
            if result.truncated {
                return Err(format!(
                    "the statement returned more than the {max_rows} rows `fetch: FETCH` \
                     will put in an output, and a prefix is not an answer. Narrow the \
                     statement, or use `fetch: NONE` if the rows are not needed"
                ));
            }
            let rows: Vec<serde_json::Value> =
                result.rows.iter().map(|r| row_object(&names, r)).collect();
            Ok(serde_json::json!({ "size": rows.len(), "rows": rows }))
        }
    }
}

/// The column names, refusing a duplicate.
///
/// A row is a JSON object and an object cannot hold two members of one name, so
/// `SELECT a.id, b.id FROM …` would silently drop one and the author would be
/// reading the wrong column with nothing to say so. Failing names both the
/// column and the fix.
fn column_names(result: &TabularResult) -> std::result::Result<Vec<&str>, String> {
    let names: Vec<&str> = result.columns.iter().map(|c| c.name.as_str()).collect();
    let mut seen = std::collections::BTreeSet::new();
    for n in &names {
        if !seen.insert(*n) {
            return Err(format!(
                "the result has two columns named {n:?}, and a row is an object that \
                 cannot hold both. Give one of them an alias"
            ));
        }
    }
    Ok(names)
}

/// One row, keyed by column name. `rows` is row-major and aligned to `columns`
/// (`egret_core::TabularResult`), so this zips rather than looks up.
fn row_object(names: &[&str], row: &[serde_json::Value]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (name, cell) in names.iter().zip(row) {
        map.insert((*name).to_string(), cell.clone());
    }
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::RunState;
    use crate::runners::RenderContext;
    use egret_core::{ColumnMeta, UserId};

    fn col(name: &str) -> ColumnMeta {
        ColumnMeta {
            name: name.to_string(),
            ty: "varchar".to_string(),
        }
    }

    fn result(columns: &[&str], rows: Vec<Vec<serde_json::Value>>) -> TabularResult {
        TabularResult {
            columns: columns.iter().map(|c| col(c)).collect(),
            row_count: rows.len() as u64,
            rows,
            bytes_scanned: None,
            truncated: false,
        }
    }

    /// A [`TaskInput`] with a principal and a render context, so the tests below
    /// are about the SQL and not about the two refusals. No `timeout:`, which
    /// is what a task that declares none carries — [`budgeted`] is the other
    /// half.
    fn input(spec: serde_json::Value, render: RenderContext) -> TaskInput {
        TaskInput {
            workspace_id: Uuid::nil(),
            execution_id: Uuid::nil(),
            run_id: Uuid::nil(),
            task_id: "q".to_string(),
            type_id: TYPE_ID.to_string(),
            value: None,
            attempt: 1,
            spec,
            render,
            timeout: None,
            principal: Some(UserId(Uuid::nil())),
        }
    }

    /// The same input, carrying the author's own `timeout:`.
    fn budgeted(
        spec: serde_json::Value,
        render: RenderContext,
        timeout: std::time::Duration,
    ) -> TaskInput {
        TaskInput {
            timeout: Some(timeout),
            ..input(spec, render)
        }
    }

    /// One recorded call: the statement, its binds, and the scope pair the port
    /// was handed. The pair is in here rather than dropped because the port is
    /// fail-closed on it, so handing it the wrong `(ws, database)` is the one
    /// thing this handler can get wrong that no assertion about rows would see.
    type Call = (String, Vec<Param>, Uuid, Uuid);

    /// A [`QueryExecPort`] that records what it was asked and answers with rows
    /// the caller chose.
    #[derive(Default)]
    struct Spy {
        seen: std::sync::Mutex<Vec<Call>>,
        answer: std::sync::Mutex<Option<TabularResult>>,
        /// The `Limits::timeout` each call was made under, recorded beside
        /// [`Call`] rather than inside it so that widening what this port
        /// observes does not rewrite every existing assertion.
        budgets: std::sync::Mutex<Vec<std::time::Duration>>,
        /// What the port answers with instead of rows.
        fails: std::sync::Mutex<Option<ExecFailureKind>>,
    }

    impl Spy {
        fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
        fn answering(r: TabularResult) -> Arc<Self> {
            let s = Self::default();
            *s.answer.lock().unwrap() = Some(r);
            Arc::new(s)
        }
        fn last(&self) -> Call {
            self.seen
                .lock()
                .unwrap()
                .last()
                .cloned()
                .expect("the port was never called")
        }
        fn calls(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
        fn last_budget(&self) -> std::time::Duration {
            *self
                .budgets
                .lock()
                .unwrap()
                .last()
                .expect("the port was never called")
        }
        /// A port that reports the engine's own deadline.
        fn timing_out() -> Arc<Self> {
            let s = Self::default();
            *s.fails.lock().unwrap() = Some(ExecFailureKind::Timeout);
            Arc::new(s)
        }
    }

    #[async_trait::async_trait]
    impl QueryExecPort for Spy {
        async fn describe_as(
            &self,
            _ws: Uuid,
            _database_id: Uuid,
            _principal: UserId,
            _sql: &str,
        ) -> std::result::Result<Vec<ColumnMeta>, Error> {
            unreachable!("this handler never describes")
        }
        async fn execute_as(
            &self,
            _ws: Uuid,
            _database_id: Uuid,
            _principal: UserId,
            _sql: &str,
            _limits: Limits,
        ) -> std::result::Result<TabularResult, Error> {
            // Overriding `execute_as_with_params` below means this is only
            // reached by a caller that bound nothing through it, which this
            // handler never does — it always goes through the params form.
            unreachable!("the handler must go through `execute_as_with_params`")
        }
        async fn execute_as_with_params(
            &self,
            ws: Uuid,
            database_id: Uuid,
            _principal: UserId,
            sql: &str,
            params: &[Param],
            limits: Limits,
        ) -> std::result::Result<TabularResult, Error> {
            self.seen
                .lock()
                .unwrap()
                .push((sql.to_string(), params.to_vec(), ws, database_id));
            self.budgets.lock().unwrap().push(limits.timeout);
            if let Some(kind) = *self.fails.lock().unwrap() {
                return Err(egret_query_api::ExecFailure::new(kind, "engine said so").into_error());
            }
            Ok(self
                .answer
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| result(&[], vec![])))
        }
    }

    /// **The author's `timeout:` is what the engine is given.** W3b measured
    /// the opposite: `models::Task`'s deserializer takes `timeout` out of a
    /// leaf's `spec`, so this handler had nothing to read and used
    /// [`DEFAULT_TIMEOUT`] whatever the author wrote. Both directions are
    /// driven, because a handler that ignored [`TaskInput::timeout`] and one
    /// that ignored the constant look the same from one arm.
    #[tokio::test]
    async fn the_engine_is_given_the_authors_own_budget_and_the_constant_only_without_one() {
        let spy = Spy::new();
        Sql::new(spy.clone())
            .run(&budgeted(
                serde_json::json!({"database": db_id().to_string(), "sql": "SELECT 1"}),
                RenderContext::new(),
                std::time::Duration::from_secs(7),
            ))
            .await
            .unwrap();
        assert_eq!(
            spy.last_budget(),
            std::time::Duration::from_secs(7),
            "the statement must run under the `timeout:` the author wrote, not \
             under this handler's fallback"
        );

        let bare = Spy::new();
        Sql::new(bare.clone())
            .run(&input(
                serde_json::json!({"database": db_id().to_string(), "sql": "SELECT 1"}),
                RenderContext::new(),
            ))
            .await
            .unwrap();
        assert_eq!(
            bare.last_budget(),
            DEFAULT_TIMEOUT,
            "a task that declares no `timeout:` still gets this handler's own \
             ceiling — the constant is a fallback now, not a dead value"
        );
    }

    /// And the engine's own deadline reports the budget it actually ran under,
    /// not the constant. Before W4 Task 2 this sentence always said `300s`.
    #[tokio::test]
    async fn the_engines_timeout_message_names_the_budget_the_statement_ran_under() {
        let out = Sql::new(Spy::timing_out())
            .run(&budgeted(
                serde_json::json!({"database": db_id().to_string(), "sql": "SELECT 1"}),
                RenderContext::new(),
                std::time::Duration::from_secs(7),
            ))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert_eq!(
            out.message.as_deref(),
            Some("egret.sql: the statement outran its 7s budget")
        );
    }

    fn db_id() -> Uuid {
        Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap()
    }

    // -----------------------------------------------------------------
    // The property the whole handler exists for
    // -----------------------------------------------------------------

    /// **The value never reaches the SQL.** Asserted in both directions: the
    /// text is exactly the statement with a `?` where the expression was, AND
    /// the value is in the bind list. A test that only checked the bind list
    /// would pass against a formatter that wrote the value *and* bound it.
    #[tokio::test]
    async fn a_value_in_sql_is_bound_and_never_written_into_the_text() {
        let spy = Spy::new();
        let mut cx = RenderContext::new();
        cx.insert("inputs", serde_json::json!({ "region": "EU' OR '1'='1" }));
        let out = Sql::new(spy.clone())
            .run(&input(
                serde_json::json!({
                    "database": db_id().to_string(),
                    "sql": "SELECT * FROM t WHERE r = {{ inputs.region }}",
                }),
                cx,
            ))
            .await
            .expect("a bound render is not a worker failure");
        assert_eq!(out.state, RunState::Success);

        let (sql, params, _, _) = spy.last();
        assert_eq!(
            sql, "SELECT * FROM t WHERE r = ?",
            "the rendered statement must carry a marker, not the value"
        );
        assert!(
            !sql.contains("EU"),
            "no part of the value may appear in the SQL text: {sql}"
        );
        assert_eq!(
            params,
            vec![Param::Str("EU' OR '1'='1".to_string())],
            "and the value must have been bound, or the statement would be wrong \
             rather than merely safe"
        );
    }

    /// The scope arguments are passed through unchanged. The port is fail-closed
    /// on them, so the one thing this handler can get wrong is handing it the
    /// wrong pair — which no assertion about rows would catch.
    #[tokio::test]
    async fn the_workspace_and_database_reach_the_port_unchanged() {
        let spy = Spy::new();
        let ws = Uuid::new_v4();
        let mut task = input(
            serde_json::json!({ "database": db_id().to_string(), "sql": "SELECT 1" }),
            RenderContext::new(),
        );
        task.workspace_id = ws;
        Sql::new(spy.clone()).run(&task).await.unwrap();
        let (_, _, seen_ws, seen_db) = spy.last();
        assert_eq!(seen_ws, ws);
        assert_eq!(seen_db, db_id());
    }

    /// An execution with no owner refuses, and **the port is never called**.
    /// Asserting the outcome alone would pass against a handler that ran the
    /// statement as the service account and then failed for some other reason.
    #[tokio::test]
    async fn an_execution_with_no_principal_refuses_before_the_engine_is_touched() {
        let spy = Spy::new();
        let mut task = input(
            serde_json::json!({ "database": db_id().to_string(), "sql": "SELECT 1" }),
            RenderContext::new(),
        );
        task.principal = None;
        let out = Sql::new(spy.clone()).run(&task).await.unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message.as_deref().unwrap_or("").contains("principal"),
            "the message must name what is missing: {:?}",
            out.message
        );
        assert_eq!(
            spy.calls(),
            0,
            "nothing may be executed before the identity is known"
        );
    }

    /// An unresolved `database:` is a failed task naming publish, not a lookup.
    #[tokio::test]
    async fn a_database_that_is_not_a_uuid_fails_the_task_and_runs_nothing() {
        let spy = Spy::new();
        let out = Sql::new(spy.clone())
            .run(&input(
                serde_json::json!({ "database": "warehouse", "sql": "SELECT 1" }),
                RenderContext::new(),
            ))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(out.message.as_deref().unwrap_or("").contains("publish"));
        assert_eq!(spy.calls(), 0);
    }

    /// A misspelled property is refused rather than ignored.
    #[tokio::test]
    async fn an_unknown_property_fails_the_task() {
        let spy = Spy::new();
        let out = Sql::new(spy.clone())
            .run(&input(
                serde_json::json!({
                    "database": db_id().to_string(),
                    "sql": "SELECT 1",
                    "fetchType": "FETCH",
                }),
                RenderContext::new(),
            ))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message.as_deref().unwrap_or("").contains("fetchType"),
            "the message must name the property: {:?}",
            out.message
        );
        assert_eq!(spy.calls(), 0);
    }

    /// A list cannot be one bound value, and the refusal names the spelling that
    /// works. This is the one place binding is less convenient than
    /// interpolation, so the message is the feature.
    #[tokio::test]
    async fn a_list_cannot_be_bound_as_one_value() {
        let spy = Spy::new();
        let mut cx = RenderContext::new();
        cx.insert("inputs", serde_json::json!({ "rs": ["EU", "US"] }));
        let out = Sql::new(spy.clone())
            .run(&input(
                serde_json::json!({
                    "database": db_id().to_string(),
                    "sql": "SELECT * FROM t WHERE r IN ({{ inputs.rs }})",
                }),
                cx,
            ))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message.as_deref().unwrap_or("").contains("{% for %}"),
            "{:?}",
            out.message
        );
        assert_eq!(spy.calls(), 0);
    }

    /// …and the spelling the refusal names actually works: one marker and one
    /// bind per element. Without this the message above would be advice nobody
    /// had followed.
    #[tokio::test]
    async fn a_for_loop_binds_one_value_per_element() {
        let spy = Spy::new();
        let mut cx = RenderContext::new();
        cx.insert("inputs", serde_json::json!({ "rs": ["EU", "US"] }));
        Sql::new(spy.clone())
            .run(&input(
                serde_json::json!({
                    "database": db_id().to_string(),
                    "sql": "SELECT 1 FROM t WHERE r IN (\
                            {% for r in inputs.rs %}{{ r }}{% if not loop.last %},{% endif %}\
                            {% endfor %})",
                }),
                cx,
            ))
            .await
            .unwrap();
        let (sql, params, _, _) = spy.last();
        assert_eq!(sql, "SELECT 1 FROM t WHERE r IN (?,?)");
        assert_eq!(
            params,
            vec![Param::Str("EU".to_string()), Param::Str("US".to_string())]
        );
    }

    /// A comparison against a parameter is NOT silently false here, which is the
    /// whole difference from the Data API's opaque `Cell`. Asserted on the
    /// branch that was taken — the `else` arm binds a different value — so it
    /// cannot pass by both arms happening to agree.
    #[tokio::test]
    async fn a_comparison_against_a_bound_name_takes_the_right_branch() {
        let spy = Spy::new();
        let mut cx = RenderContext::new();
        cx.insert("inputs", serde_json::json!({ "mode": "fast" }));
        Sql::new(spy.clone())
            .run(&input(
                serde_json::json!({
                    "database": db_id().to_string(),
                    "sql": "SELECT {% if inputs.mode == 'fast' %}1{% else %}2{% endif %}",
                }),
                cx,
            ))
            .await
            .unwrap();
        assert_eq!(
            spy.last().0,
            "SELECT 1",
            "`==` against a context value must work; in `egret-dataapi` the same \
             expression is silently false for ever, because every parameter there is an \
             opaque object"
        );
    }

    // -----------------------------------------------------------------
    // The output shapes
    // -----------------------------------------------------------------

    /// `fetch:` defaults to `NONE`, and `NONE` is `{}` — present and empty,
    /// never `None`. That is the measured rule (`{{ outputs.a }}` renders `{}`
    /// on the artifact), and this is the guard on it.
    #[tokio::test]
    async fn the_default_fetch_runs_the_statement_and_produces_an_empty_map() {
        let spy = Spy::answering(result(&["n"], vec![vec![serde_json::json!(1)]]));
        let out = Sql::new(spy.clone())
            .run(&input(
                serde_json::json!({ "database": db_id().to_string(), "sql": "SELECT 1 AS n" }),
                RenderContext::new(),
            ))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Success);
        assert_eq!(
            out.outputs,
            Some(serde_json::json!({})),
            "`{{{{ outputs.q }}}}` has to render, so the key is present and the map is \
             empty — an absent output is not the same thing"
        );
        assert_eq!(spy.calls(), 1, "and the statement still ran");
    }

    #[test]
    fn fetch_one_produces_the_first_row_and_a_size_of_one() {
        let r = result(
            &["region", "units"],
            vec![
                vec![serde_json::json!("EU"), serde_json::json!(42)],
                vec![serde_json::json!("US"), serde_json::json!(7)],
            ],
        );
        assert_eq!(
            outputs_for(Fetch::One, &r, 10).unwrap(),
            serde_json::json!({"size": 1, "row": {"region": "EU", "units": 42}})
        );
    }

    /// No rows is `{"size": 0}` with **no** `row` key, so `{{ outputs.q.row }}`
    /// fails rather than rendering a null an author would compare against.
    #[test]
    fn fetch_one_over_no_rows_is_a_size_of_zero_with_no_row_key() {
        let r = result(&["n"], vec![]);
        let out = outputs_for(Fetch::One, &r, 10).unwrap();
        assert_eq!(out, serde_json::json!({"size": 0}));
        assert!(out.get("row").is_none());
    }

    #[test]
    fn fetch_produces_every_row_and_a_size_derived_from_them() {
        let r = result(
            &["region", "units"],
            vec![
                vec![serde_json::json!("EU"), serde_json::json!(42)],
                vec![serde_json::json!("US"), serde_json::json!(7)],
            ],
        );
        let out = outputs_for(Fetch::All, &r, 10).unwrap();
        assert_eq!(
            out,
            serde_json::json!({
                "size": 2,
                "rows": [
                    {"region": "EU", "units": 42},
                    {"region": "US", "units": 7}
                ]
            })
        );
        assert_eq!(
            out["size"].as_u64().unwrap() as usize,
            out["rows"].as_array().unwrap().len(),
            "`size` is defined as the number of rows in the output beside it, so the \
             two cannot disagree"
        );
    }

    /// A truncated `FETCH` is a failure, not a prefix. Both directions: the same
    /// result with the flag clear succeeds, so this cannot pass against a
    /// function that simply refuses everything.
    #[test]
    fn a_truncated_fetch_fails_and_an_untruncated_one_does_not() {
        let mut r = result(&["n"], vec![vec![serde_json::json!(1)]]);
        assert!(outputs_for(Fetch::All, &r, 10).is_ok(), "the control");
        r.truncated = true;
        let msg = outputs_for(Fetch::All, &r, 10).expect_err("a prefix is not an answer");
        assert!(msg.contains("10"), "the cap must be named: {msg}");
    }

    /// `NONE` and `FETCH_ONE` are unaffected by the flag, and that is the
    /// precision the check is written for rather than an oversight.
    #[test]
    fn truncation_does_not_fail_the_two_fetches_that_do_not_claim_all_the_rows() {
        let mut r = result(&["n"], vec![vec![serde_json::json!(1)]]);
        r.truncated = true;
        assert_eq!(
            outputs_for(Fetch::None, &r, 10).unwrap(),
            serde_json::json!({})
        );
        assert_eq!(
            outputs_for(Fetch::One, &r, 10).unwrap(),
            serde_json::json!({"size": 1, "row": {"n": 1}})
        );
    }

    /// Two columns of one name would silently lose a value; it fails instead,
    /// naming the column. The `NONE` arm is unaffected, because it builds no row.
    #[test]
    fn a_duplicate_column_name_fails_rather_than_dropping_a_value() {
        let r = result(
            &["id", "id"],
            vec![vec![serde_json::json!(1), serde_json::json!(2)]],
        );
        let msg = outputs_for(Fetch::All, &r, 10).expect_err("an object cannot hold both");
        assert!(msg.contains("\"id\""), "{msg}");
        assert_eq!(
            outputs_for(Fetch::None, &r, 10).unwrap(),
            serde_json::json!({}),
            "`NONE` builds no row and must not be refused by a rule about rows"
        );
    }

    /// A number binds exactly. `Param::F64` is unreachable by construction, so
    /// a decimal an author wrote cannot arrive rounded.
    #[test]
    fn a_non_integer_number_binds_as_an_exact_decimal_and_never_as_a_float() {
        assert_eq!(to_param(&serde_json::json!(7)), Param::I64(7));
        let big: serde_json::Value = serde_json::from_str("1234567.875").unwrap();
        assert_eq!(to_param(&big), Param::Decimal("1234567.875".to_string()));
        assert_eq!(to_param(&serde_json::Value::Null), Param::Null);
        assert_eq!(to_param(&serde_json::json!(true)), Param::Bool(true));
    }

    /// `cancel` refuses. The four core handlers answer `Ok(())` because they
    /// have nothing to stop; this one holds a remote statement, and a cheerful
    /// `Ok(())` would report it as stopped while it went on running.
    #[tokio::test]
    async fn cancel_refuses_rather_than_claiming_to_have_stopped_the_statement() {
        let err = Sql::new(Spy::new())
            .cancel(&input(serde_json::json!({}), RenderContext::new()))
            .await
            .expect_err("this handler cannot cancel and must not say it can");
        let msg = err.to_string();
        assert!(
            msg.contains("QueryExecPort") && msg.contains("kill path"),
            "the refusal must name what is missing and who owns it: {msg}"
        );
    }
}
