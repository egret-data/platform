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
//! | `STORE` | `{"size": n, "uri": "kestra:///…/tasks/{task}/{taskRunId}/result.ion"}` |
//!
//! `size` is defined as **the number of rows this task put into its own
//! output** — 0 or 1 for `FETCH_ONE`, `rows.len()` for `FETCH` — rather than as
//! the engine's row count, so it is derivable from the output beside it and the
//! two cannot disagree. A row is an object keyed by column name; a result with
//! two columns of the same name fails the task rather than silently dropping
//! one, because a JSON object cannot hold both and the loss would be invisible.
//!
//! # `fetch: STORE` (W9a Task 10, spec §9.2)
//!
//! The rows go to internal storage instead of the output: Ion text written by
//! `runners::ionfmt` ([`crate::runners::write_ion_row`]), one struct per line,
//! streamed through a `tokio::io::duplex` into `put_stream` at the run's own
//! task prefix, and handed to the commit through `Outcome::files`, which
//! indexes it in the outcome's transaction (§4.3). The outputs are Kestra's
//! `AbstractJdbcQuery` STORE shape, `{uri, size}` with `size` the ROW count
//! (`w9a-storage.json` `store_outputs`). Kestra names the file randomly
//! (`{n}.ion`, measured: two runs, two names); egret's is the fixed
//! [`STORE_FILE`], `result.ion`, so a re-claim overwrites its own object
//! (§4.3). Kestra's file is binary Ion from `FileSerde`; this one is Ion text,
//! and the test `store_ion_matches_the_artifact_for_every_type_egret_can_carry`
//! records per SQL type where the DECODED values agree and where they do not.
//!
//! **What STORE does not do: reduce memory.** `QueryExecPort` returns a whole
//! `TabularResult`, and neither connector hands a row on before the end
//! (`w9a-storage.json` `connector_streaming`: Trino pages through `nextUri`
//! and StarRocks iterates rows, but both collect every row into a `Vec`, and
//! `port_impl::run_bounded` awaits the whole result). So STORE is capped by
//! [`MAX_FETCH_ROWS`] and fails at the cap exactly as `FETCH` does; what it
//! changes is where the rows go -- a file instead of `task_runs.outputs` --
//! not how many egret holds. `egret.sql` is in no production registry (§1).
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
use crate::storage::{FileStore, StoredFile};

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

/// The name of the file `fetch: STORE` writes under the run's task prefix.
pub const STORE_FILE: &str = "result.ion";

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
/// The four names are Kestra's JDBC `fetchType` spellings, so an author coming
/// from there is not surprised, under egret's own shorter key. `STORE` writes
/// the result set to internal storage (W9a Task 10; see this module's doc).
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
    /// Write every row to internal storage as Ion; produce `{uri, size}`.
    #[serde(rename = "STORE")]
    Store,
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
    /// Internal storage, which `fetch: STORE` writes to; `None` when egret
    /// has no `[storage]`, and `STORE` then fails naming it.
    files: Option<Arc<FileStore>>,
}

impl std::fmt::Debug for Sql {
    /// The bounds, not the port: `dyn QueryExecPort` has no `Debug` and
    /// requiring one would put a bound on every implementation for one line of
    /// diagnostics. Same decision as `Handlers`'s.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sql")
            .field("max_rows", &self.max_rows)
            .field("timeout", &self.timeout)
            .field("files", &self.files.is_some())
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
            files: None,
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
            files: None,
        }
    }

    /// The same handler, writing `fetch: STORE` files to `files`.
    pub fn with_files(mut self, files: Option<Arc<FileStore>>) -> Self {
        self.files = files;
        self
    }

    /// Where `fetch: STORE` writes, or the task's failure: the store, and the
    /// run's own task prefix -- the render scope's (the writing execution's,
    /// an iteration's inside a `Loop`, spec §3), else built from `flow` the
    /// way `script.rs`'s `collect_to` builds it. A namespace, flow or task id
    /// no URI can carry is the author's error, never `Internal`.
    fn store_target(
        &self,
        task: &TaskInput,
    ) -> std::result::Result<(Arc<FileStore>, String), String> {
        let Some(fs) = &self.files else {
            return Err(
                "`fetch: STORE` needs internal storage, and egret has none configured \
                 (the `[storage]` section of its configuration)"
                    .to_string(),
            );
        };
        if let Some(prefix) = task.render.files().and_then(|f| f.0.task_prefix()) {
            return Ok((fs.clone(), prefix.to_string()));
        }
        let flow = (
            crate::runners::render_str("{{ flow.namespace }}", &task.render),
            crate::runners::render_str("{{ flow.id }}", &task.render),
        );
        let (Ok(namespace), Ok(flow_id)) = flow else {
            return Err(
                "this run has no storage prefix for the file `fetch: STORE` writes: it is not \
                 a task run of a flow's execution"
                    .to_string(),
            );
        };
        let prefix = crate::storage::uri::task_prefix(
            &namespace,
            &flow_id,
            task.execution_id,
            &task.task_id,
            task.run_id,
        )?;
        Ok((fs.clone(), prefix))
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

        // `fetch: STORE` needs somewhere to put the rows, and that is decided
        // BEFORE the statement runs: a statement whose rows have nowhere to
        // go would cost the engine for nothing.
        let store_to = match spec.fetch {
            Fetch::Store => match self.store_target(task) {
                Ok(target) => Some(target),
                Err(why) => return Ok(failed(format!("{TYPE_ID}: {why}"))),
            },
            Fetch::None | Fetch::One | Fetch::All => None,
        };

        let rendered = crate::runners::env::render_bound(&spec.sql, &task.render);
        // W9a Task 4: a `read()` in `sql:` that met a store OUTAGE failed the
        // render, and that failure is not the author's (spec §11): `Err`,
        // before the statement runs.
        if let Some(outage) = task
            .render
            .files()
            .and_then(|f| f.0.infrastructure_failure())
        {
            return Err(Error::Internal(anyhow::anyhow!("{TYPE_ID}: {outage}")));
        }
        let (sql, bound) = match rendered {
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

        let outputs = match outputs_for(spec.fetch, &result, self.max_rows) {
            Ok(outputs) => outputs,
            Err(message) => return Ok(failed(format!("{TYPE_ID}: {message}"))),
        };
        match store_to {
            None => Ok(Outcome::succeeded_with(outputs)),
            Some((fs, prefix)) => store(&fs, task.workspace_id, &prefix, &result, outputs).await,
        }
    }

    /// **This handler cannot cancel, and says so instead of pretending.**
    ///
    /// `TaskHandler::cancel` has no default body precisely so that `egret.sql`
    /// could not inherit `Ok(())` from the four handlers that genuinely have
    /// nothing to stop. This one holds a statement running on a remote engine,
    /// and dropping the future does not stop it.
    ///
    /// # The refusal was a tripwire and W4 Task 3 tripped it
    ///
    /// W3b wrote *"nothing calls `cancel` at all yet"*. That is no longer true:
    /// [`cancel::guarded`](crate::worker) calls it whenever a task-level
    /// `timeout:` expires, which is the artifact's only `kill(null)` site. This
    /// is therefore a refusal that is **reached**, logged with its own reason,
    /// and asserted in both this module's tests and `tests/worker.rs`'s
    /// `a_handler_that_cannot_be_stopped_still_fails_on_its_budget_and_says_so`.
    ///
    /// # THIS IS A MEASURED DIVERGENCE FROM THE ARTIFACT, not a gap the
    /// # artifact shares
    ///
    /// Fix round 1 corrected the sentence that used to stand here. W4 Task 3's
    /// first reading stopped at `WorkerJobLifecycle`, whose `kill()` is
    /// `default` and empty, and concluded that a Kestra task which cannot stop
    /// itself is indistinguishable from one that did. That is true of the
    /// *interface* and false of the task this handler corresponds to.
    /// Disassembled from the pinned image's plugin jars — `/app/plugins/`
    /// `io_kestra_plugin__plugin-jdbc-trino__2_0_0.jar` and
    /// `…plugin-jdbc-mysql__2_0_0.jar`, extracted with `docker cp` and read
    /// with `javap -c`:
    ///
    /// * `io.kestra.plugin.jdbc.trino.Query` and
    ///   `io.kestra.plugin.jdbc.mysql.Query` both `extends
    ///   io.kestra.plugin.jdbc.AbstractJdbcQuery`. (Kestra ships no StarRocks
    ///   task; StarRocks speaks the MySQL wire protocol and users point the
    ///   MySQL task at it.)
    /// * `AbstractJdbcQuery` holds `private volatile transient
    ///   java.sql.Statement runningStatement` and `java.sql.Connection
    ///   runningConnection`, both assigned during `run`.
    /// * It **overrides** `public void kill()`, which calls
    ///   `AbstractJdbcBaseQuery.kill(Statement)` then `kill(Connection)`.
    /// * `kill(Statement)`: if non-null and `!isClosed()` →
    ///   `Statement.cancel()`, then `Statement.close()`. A `SQLException` is
    ///   wrapped in a `RuntimeException` and **thrown**, landing in
    ///   `WorkerTaskCallable.kill`'s catch/WARN.
    /// * `kill(Connection)`: if non-null and `!isClosed()` → `close()`.
    ///
    /// **So Kestra's own SQL task stops the remote statement, and `egret.sql`
    /// does not.** A `timeout: PT10S` on a one-hour query is cancelled at ten
    /// seconds there and runs the full hour here on StarRocks. Whatever
    /// justifies this refusal, it cannot be *"the artifact has no better
    /// answer"* — it has one, and it is `Statement.cancel()`.
    ///
    /// # Why it still refuses: cost and scope, and one fact that decides it
    ///
    /// The cancellation channel exists **below** this handler and stops one
    /// layer short of it:
    ///
    /// * `egret_query`'s `Connector::execute(sql, limits, CancellationToken)`
    ///   takes a token. `TrinoConnector` honours it with a real server-side
    ///   cancel — it `DELETE`s the statement's `nextUri`.
    ///   `JobManager::cancel` already drives that.
    /// * `execution::port_impl::run_bounded` — the implementation behind
    ///   [`QueryExecPort`] — builds a token **privately**, cancels it on its own
    ///   `tokio::time::timeout(limits.timeout, …)`, and never lets a caller hold
    ///   it. When the worker's outer budget drops the future first (it does:
    ///   same instant, armed first) that inner timeout never fires, so the
    ///   DELETE never happens.
    /// * [`QueryExecPort`] itself has no cancel verb, and that trait's doc gives
    ///   the rule for adding one: *"A trait method with no consumer is untested
    ///   surface — do not add one ahead of its caller."* It has **17
    ///   implementation sites, 16 of them hand-written test fakes** —
    ///   `execution::port_impl`'s `EngineState` is the only production one.
    ///
    /// So honouring `cancel` means a new port verb, a per-run handle threaded
    /// out of `execute_as_with_params` and held by this handler, and 16 fakes
    /// updated across `egret-dataapi`, `egret-server` and this crate.
    ///
    /// **The fact that decides it is that this would not fix the engine where
    /// the hazard is.** On StarRocks the token is honoured only *cooperatively
    /// inside the row drain* — `connector/starrocks.rs`'s `drain_rows` checks
    /// `cancel.is_cancelled()` between rows — so a query still executing before
    /// its first row is untouched, and breaking out of the loop tells StarRocks
    /// nothing. A real cancel there needs `KILL QUERY <id>` issued on a **second
    /// connection**, which no connector does. So the change with the widest
    /// blast radius would deliver a true cancel only on Trino, which is
    /// **already** bounded by the `query_max_run_time` this handler sends. That
    /// is the wrong trade for this task, and it is a scope judgement rather than
    /// a claim about what is possible.
    ///
    /// **Whoever picks this up needs all three, not the first:** the port verb,
    /// a `KILL QUERY` path in the StarRocks connector, and a live assertion in
    /// `tests/sql_starrocks_live.rs` that the statement actually stops — the
    /// default gate cannot see it, and a Trino-only half looks finished from
    /// inside the unit tests.
    ///
    /// # What the refusal costs today, per engine
    ///
    /// * **Trino** — [`Limits::timeout`] is sent as a `query_max_run_time`
    ///   session property (`connector/trino.rs`), so the coordinator kills the
    ///   statement at the same budget the worker used. The abandoned statement
    ///   costs at most one budget of engine time.
    /// * **StarRocks** — `limits.timeout` is **unread** by that connector, which
    ///   says so in its own doc: it *"relies entirely on an outer caller … to
    ///   enforce `limits.timeout` by dropping the future"*. The worker's drop is
    ///   that outer caller, and dropping a future tells MySQL-protocol nothing.
    ///   A timed-out `egret.sql` on StarRocks runs to completion on the engine.
    ///   The cheapest real fix is a `query_timeout` session variable set the way
    ///   `connector/trino.rs` already sets `query_max_run_time`; it is not done
    ///   here because that connector is shared with the Data API serving path
    ///   and would begin failing queries that succeed today.
    ///
    /// **This divergence is not in the oracle's runtime over-acceptance
    /// register and cannot be.** That register is keyed by corpus id and derived
    /// from the two traces by
    /// `runtime_over_acceptance_is_registered_and_owned`; `egret.sql` appears in
    /// **zero** corpus entries, so it has no id to register under. The earlier
    /// sentence here claiming a row was false and is withdrawn. This doc
    /// comment and the task report are the record.
    ///
    /// [`Limits::timeout`]: egret_query_api::Limits
    async fn cancel(&self, task: &TaskInput) -> Result<()> {
        Err(Error::Internal(anyhow::anyhow!(
            "{TYPE_ID}: task {} cannot be cancelled — `QueryExecPort` has no cancellation \
             verb, so the statement continues on the engine after the run is abandoned. \
             On Trino it is still bounded by the `query_max_run_time` this task sent with \
             it; on StarRocks that connector does not read `Limits::timeout` at all, so it \
             is not bounded by anything",
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
            // Checked for `FETCH` and `STORE` alone, and that is precise rather
            // than lazy: `NONE` discards the rows and `FETCH_ONE` takes the
            // first, so neither can be made wrong by a cap the other side of
            // it. Only `FETCH` and `STORE` claim to have produced "the rows",
            // and a claim that is silently a prefix is the over-acceptance
            // shape the spec's fidelity rules forbid.
            complete("FETCH", result, max_rows)?;
            let rows: Vec<serde_json::Value> =
                result.rows.iter().map(|r| row_object(&names, r)).collect();
            Ok(serde_json::json!({ "size": rows.len(), "rows": rows }))
        }
        // The rows go to a file, not here: `size` is decided now (the ROW
        // count, `w9a-storage.json` `store_outputs.size_counts`) and `uri` is
        // added by [`store`] once the file is written.
        Fetch::Store => {
            complete("STORE", result, max_rows)?;
            Ok(serde_json::json!({ "size": result.rows.len() }))
        }
    }
}

/// The cap, for the two fetches that claim every row: a truncated result is
/// the task's failure, never a prefix.
fn complete(
    fetch: &str,
    result: &TabularResult,
    max_rows: usize,
) -> std::result::Result<(), String> {
    if result.truncated {
        return Err(format!(
            "the statement returned more than the {max_rows} rows `fetch: {fetch}` \
             will put in an output, and a prefix is not an answer. Narrow the \
             statement, or use `fetch: NONE` if the rows are not needed"
        ));
    }
    Ok(())
}

/// How many bytes of rows go through the pipe between the writer and the
/// upload at once.
const STORE_PIPE_BYTES: usize = 64 * 1024;

/// One row and its newline: the one spelling both of [`store`]'s passes use.
fn store_line(line: &mut String, columns: &[String], row: &[serde_json::Value]) {
    line.clear();
    crate::runners::write_ion_row(line, columns, row);
    line.push('\n');
}

/// `fetch: STORE`'s file: every row as Ion text, one per line, streamed into
/// the store at `{prefix}`[`STORE_FILE`]; `outputs` gains its `uri`, and the
/// outcome carries the file so the commit indexes it (spec §4.3).
///
/// Two passes over the rows the port already holds, both through
/// [`store_line`]: the first only counts bytes, so the caps are judged before
/// a byte is uploaded and the upload is told its exact size; the second
/// writes into a `tokio::io::duplex` that `put_stream` reads from, so the
/// file is never assembled whole beside the rows.
///
/// * The author's: a name no URI can carry, a crossed cap -- a failed task.
/// * egret's: the store failing, or holding other than what was written --
///   `Err` (spec §11), so the lease lapses and the run is re-claimed with no
///   attempt spent; the re-claim writes the same object again.
async fn store(
    fs: &FileStore,
    workspace: Uuid,
    prefix: &str,
    result: &TabularResult,
    mut outputs: serde_json::Value,
) -> Result<Outcome> {
    use tokio::io::AsyncWriteExt;
    let fail = |why: String| -> Result<Outcome> { Ok(failed(format!("{TYPE_ID}: {why}"))) };
    let file = match crate::storage::uri::verbatim_file_uri(prefix, STORE_FILE) {
        Ok(file) => file,
        Err(why) => return fail(why),
    };
    let object = match fs.object_uri(workspace, &file) {
        Ok(object) => object,
        Err(why) => return fail(why),
    };
    let columns: Vec<String> = result.columns.iter().map(|c| c.name.clone()).collect();

    let mut line = String::new();
    let mut size: u64 = 0;
    for row in &result.rows {
        store_line(&mut line, &columns, row);
        size += line.len() as u64;
    }
    let caps = fs.caps;
    if size > caps.max_file_bytes {
        return fail(format!(
            "the file `fetch: STORE` writes, {file}, is {size} bytes, over the {} one file \
             may be (workflow.storage.max_file_bytes)",
            caps.max_file_bytes
        ));
    }
    if size > caps.max_task_bytes {
        return fail(format!(
            "the file `fetch: STORE` writes, {file}, is {size} bytes, over the {} one task \
             run may store (workflow.storage.max_task_bytes)",
            caps.max_task_bytes
        ));
    }

    let (mut tx, rx) = tokio::io::duplex(STORE_PIPE_BYTES);
    let rows = &result.rows;
    let columns = &columns;
    let write = async move {
        let mut line = String::new();
        for row in rows {
            store_line(&mut line, columns, row);
            tx.write_all(line.as_bytes()).await?;
        }
        tx.shutdown().await
    };
    // `rx` is MOVED into the upload, so an upload that gives up drops it and
    // the writer's next write fails instead of waiting for a reader for ever.
    let put = async {
        let mut rx = rx;
        fs.store.put_stream(&object, &mut rx, Some(size)).await
    };
    let (written, put) = tokio::join!(write, put);
    let outage = |what: String| -> Result<Outcome> {
        Err(Error::Internal(anyhow::anyhow!("{TYPE_ID}: {what}")))
    };
    let uploaded = match put {
        Ok(n) => n,
        Err(e) => {
            return outage(format!(
                "{file} could not be stored: internal storage is unavailable: {e}"
            ))
        }
    };
    if let Err(e) = written {
        return outage(format!(
            "{file} could not be written to internal storage: {e}"
        ));
    }
    // `head` for the real size (spec §4.2), as `collect_one` does: what the
    // index row will hold.
    match fs.store.head(&object).await {
        Ok(Some(held)) if held == size && uploaded == size => {}
        Ok(Some(held)) => {
            return outage(format!(
                "{file} holds {held} bytes in internal storage after {size} were written \
                 ({uploaded} uploaded)"
            ))
        }
        Ok(None) => {
            return outage(format!(
                "{file} was written ({uploaded} bytes) but internal storage does not have it"
            ))
        }
        Err(e) => {
            return outage(format!(
                "{file} could not be checked: internal storage is unavailable: {e}"
            ))
        }
    }

    outputs["uri"] = serde_json::Value::String(file.clone());
    Ok(Outcome {
        files: vec![StoredFile { uri: file, size }],
        ..Outcome::succeeded_with(outputs)
    })
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
            // `egret.sql` holds a remote statement and cannot cancel it at
            // all, so the lease token has no reader here. A fixed instant
            // rather than a clock reading, for `handlers::testing::input`'s
            // reason.
            log: crate::logs::RunLog::discard(),
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
            msg.contains("QueryExecPort") && msg.contains("StarRocks"),
            "the refusal must name the verb that is missing and the engine where \
             the consequence is unbounded: {msg}"
        );
    }

    /// **The tripwire, tripped.** W3b made this handler refuse so that wiring
    /// the kill path would fail loudly at the one site that had to change; this
    /// is that site, driven through the worker's own dispatch rather than by
    /// calling the method directly.
    ///
    /// It is here rather than in `worker::cancel` because the fake
    /// [`QueryExecPort`] this handler needs already exists in this module, and a
    /// second one written next to the kill path could drift from it.
    #[tokio::test]
    async fn the_kill_path_reaches_this_refusal_through_the_registry() {
        let mut reg = crate::worker::Handlers::new();
        reg.add(Arc::new(Sql::new(Spy::new())));
        let killed =
            crate::worker::cancel::kill(&reg, &input(serde_json::json!({}), RenderContext::new()))
                .await;
        let crate::worker::cancel::Killed::Refused(reason) = killed else {
            panic!(
                "the kill path must not report `egret.sql` as stopped; if this \
                 arm is reached, the path wired by W4 Task 3 does not reach \
                 `TaskHandler::cancel` at all"
            );
        };
        assert!(
            reason.contains("QueryExecPort"),
            "the handler's own reason must survive the trip through the \
             registry rather than being flattened: {reason}"
        );
    }

    // -----------------------------------------------------------------
    // `fetch: STORE` (W9a Task 10, spec §9.2)
    // -----------------------------------------------------------------

    use crate::storage::{self, uri, Caps, StoredFile};
    use egret_storage::{ObjectStore, StorageError};

    const NS: &str = "w9a.t10";
    const FLOW: &str = "pg_store";

    /// A `FileStore` over a `MemStore` the test keeps a handle on, so it can
    /// read back what the handler wrote and see what it did not.
    fn mem_files(caps: Caps) -> (Arc<FileStore>, egret_storage::MemStore) {
        let mem = egret_storage::MemStore::new();
        let fs = Arc::new(FileStore {
            store: Arc::new(mem.clone()),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps,
            retention_days: 30,
        });
        (fs, mem)
    }

    /// A STORE input as the worker hands it over: ids of its own, and the
    /// render scope carrying the run's `task_prefix` (what `claim` builds).
    struct Stored {
        task: TaskInput,
        prefix: String,
    }

    fn store_input(fs: &Arc<FileStore>, sql: &str) -> Stored {
        let ws = Uuid::new_v4();
        let ex = Uuid::new_v4();
        let run = Uuid::new_v4();
        let prefix = uri::task_prefix(NS, FLOW, ex, "q", run).unwrap();
        let scope = storage::scope::Snapshot::from_parts(
            std::collections::BTreeMap::new(),
            fs.clone(),
            ws,
            Some(prefix.clone()),
        )
        .into_scope();
        let mut task = input(
            serde_json::json!({"database": db_id().to_string(), "sql": sql, "fetch": "STORE"}),
            RenderContext::new().with_files(scope),
        );
        task.workspace_id = ws;
        task.execution_id = ex;
        task.run_id = run;
        Stored { task, prefix }
    }

    async fn object(fs: &FileStore, ws: Uuid, uri: &str) -> Vec<u8> {
        fs.store
            .get(&fs.object_uri(ws, uri).unwrap())
            .await
            .expect("the file is in the store")
    }

    fn decoded(bytes: Vec<u8>) -> Vec<serde_json::Value> {
        let (rows, more) =
            storage::ion::values_to_json(std::io::Cursor::new(bytes), 100_000).expect("Ion");
        assert!(!more);
        rows
    }

    /// **The output shape is Kestra's** (`w9a-storage.json` `store_outputs`:
    /// keys `size` and `uri`, `size` the ROW count), the file is under the
    /// run's own prefix with the fixed name, every row is in it as one Ion
    /// struct per line, and **the file is handed to the commit through
    /// `Outcome.files`** with its BYTE size -- which is what indexes it.
    /// Zero rows still writes and indexes a 0-byte file (measured: "0 rows ->
    /// size 0 (a 0-byte file is still written and returned)").
    #[tokio::test]
    async fn store_writes_every_row_as_ion_and_outputs_uri_and_row_count() {
        let (fs, _mem) = mem_files(Caps::default());
        let spy = Spy::answering(result(
            &["region", "units", "note"],
            vec![
                vec![
                    serde_json::json!("EU"),
                    serde_json::json!(40),
                    serde_json::json!("a\nb"),
                ],
                vec![
                    serde_json::json!("US"),
                    serde_json::json!(7),
                    serde_json::Value::Null,
                ],
                vec![
                    serde_json::json!("EU"),
                    serde_json::json!(51),
                    serde_json::json!("say \"hi\""),
                ],
            ],
        ));
        let st = store_input(&fs, "SELECT region, units, note FROM t");
        let out = Sql::new(spy.clone())
            .with_files(Some(fs.clone()))
            .run(&st.task)
            .await
            .expect("a stored result is not a worker failure");
        assert_eq!(out.state, RunState::Success, "{:?}", out.message);

        let want_uri = format!("{}{STORE_FILE}", st.prefix);
        assert_eq!(
            STORE_FILE, "result.ion",
            "w9a-storage.json store_outputs.egret.name"
        );
        assert_eq!(
            out.outputs,
            Some(serde_json::json!({"uri": want_uri, "size": 3})),
            "Kestra's STORE outputs: {{uri, size}} with size the row count"
        );

        let bytes = object(&fs, st.task.workspace_id, &want_uri).await;
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert_eq!(
            text.lines().count(),
            3,
            "one row per line, and a newline inside a value is escaped: {text:?}"
        );
        assert_eq!(
            decoded(bytes.clone()),
            vec![
                serde_json::json!({"region": "EU", "units": 40, "note": "a\nb"}),
                // The reader drops a null field, as Kestra's preview does.
                serde_json::json!({"region": "US", "units": 7}),
                serde_json::json!({"region": "EU", "units": 51, "note": "say \"hi\""}),
            ]
        );
        assert!(
            text.contains("note:null"),
            "the null is written, not omitted (ofIon writes ALWAYS): {text}"
        );
        assert_eq!(
            out.files,
            vec![StoredFile {
                uri: want_uri.clone(),
                size: bytes.len() as u64,
            }],
            "the file reaches the outcome-commit transaction through Outcome.files, \
             with its size in BYTES (the index row), not in rows"
        );
        crate::storage::collect::check_outputs_collected(
            out.outputs.as_ref().unwrap(),
            &st.prefix,
            &out.files,
        )
        .expect("§5: every URI under the run's prefix in the outputs is collected");

        // Zero rows: still a file, still indexed, size 0.
        let empty = Spy::answering(result(&["n"], vec![]));
        let st = store_input(&fs, "SELECT n FROM t WHERE false");
        let out = Sql::new(empty)
            .with_files(Some(fs.clone()))
            .run(&st.task)
            .await
            .unwrap();
        let want_uri = format!("{}{STORE_FILE}", st.prefix);
        assert_eq!(
            out.outputs,
            Some(serde_json::json!({"uri": want_uri, "size": 0}))
        );
        assert!(object(&fs, st.task.workspace_id, &want_uri)
            .await
            .is_empty());
        assert_eq!(
            out.files,
            vec![StoredFile {
                uri: want_uri,
                size: 0
            }]
        );
    }

    /// How a value of each SQL type decodes, against the artifact's own file.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Ion {
        /// Decodes to the same JSON and is spelled the same Ion text as the
        /// artifact's (`ion_text`, Kestra's file decoded by its own ion-java).
        Same,
        /// Decodes to the same JSON through `storage::ion`, but the Ion TYPE
        /// differs (a string where Kestra wrote a timestamp or a blob); the
        /// reader's JSON projection cannot see it, the text can.
        SameJsonOtherType,
        /// Decodes to different JSON: a divergence, recorded, not faked.
        Diverges,
    }

    /// **Every type in `ion_rows`, compared as DECODED values** (spec §1,
    /// §9.2): Kestra's file is binary Ion from `FileSerde`, egret's is Ion
    /// text from `ionfmt`, so bytes never match. Both go through the one Ion
    /// reader, `storage::ion::values_to_json`.
    ///
    /// The value egret writes is the JSON its `TabularResult` CARRIES for that
    /// type, which is each connector's, read from its code:
    /// * Trino (`connector/trino.rs:390-393`) passes the REST response's JSON
    ///   through: a boolean, a decimal as a string, a timestamp as
    ///   `yyyy-MM-dd HH:mm:ss.ffffff`, a varbinary as base64.
    /// * StarRocks (`connector/starrocks.rs`, `bytes_to_json`) types only ints
    ///   and floats; DECIMAL, dates and times are strings, a BOOLEAN is a
    ///   TINYINT (`1`), a non-UTF-8 VARBINARY is hex. No timestamptz type.
    #[test]
    fn store_ion_matches_the_artifact_for_every_type_egret_can_carry() {
        use base64::Engine as _;
        use serde_json::json;
        use Ion::*;
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../oracle-exec/w9a-storage.json")).unwrap();
        let rows = fixture["egret_expected"]["ion_rows"].as_array().unwrap();

        // (sql_type, Trino carries, StarRocks carries (None: no such type))
        type Carried = (serde_json::Value, Ion);
        let carried: Vec<(&str, Carried, Option<Carried>)> = vec![
            ("int", (json!(42), Same), Some((json!(42), Same))),
            (
                "bigint",
                (json!(9007199254740993_i64), Same),
                Some((json!(9007199254740993_i64), Same)),
            ),
            (
                "decimal",
                (json!("12.340"), Diverges),
                Some((json!("12.340"), Diverges)),
            ),
            ("float", (json!(1.5), Same), Some((json!(1.5), Same))),
            ("bool", (json!(true), Same), Some((json!(1), Diverges))),
            (
                "text_with_quote",
                (json!("say \"hi\""), Same),
                Some((json!("say \"hi\""), Same)),
            ),
            (
                "timestamp",
                (json!("2026-01-02 03:04:05.123456"), Diverges),
                Some((json!("2026-01-02 03:04:05.123456"), Diverges)),
            ),
            (
                "timestamptz",
                (json!("2026-01-01 20:04:05.000 UTC"), Diverges),
                None,
            ),
            (
                "date",
                (json!("2026-01-02"), SameJsonOtherType),
                Some((json!("2026-01-02"), SameJsonOtherType)),
            ),
            ("null", (json!(null), Same), Some((json!(null), Same))),
            (
                "bytea",
                (json!("3q2+7w=="), SameJsonOtherType),
                Some((json!("deadbeef"), Diverges)),
            ),
        ];
        assert_eq!(
            carried.len(),
            rows.len(),
            "every ion_rows entry is classified, none skipped"
        );

        let mut seen = std::collections::BTreeMap::new();
        for row in rows {
            let ty = row["sql_type"].as_str().unwrap();
            let (_, trino, starrocks) = carried
                .iter()
                .find(|(t, _, _)| *t == ty)
                .unwrap_or_else(|| panic!("{ty} is not classified"));
            let kestra_bytes = base64::engine::general_purpose::STANDARD
                .decode(row["bytes_b64"].as_str().unwrap())
                .unwrap();
            let kestra = decoded(kestra_bytes);
            let kestra_text = row["ion_text"].as_str().unwrap();
            for (engine, (value, want)) in
                [("trino", Some(trino)), ("starrocks", starrocks.as_ref())]
                    .into_iter()
                    .filter_map(|(e, c)| c.map(|c| (e, c)))
            {
                let mut ours = String::new();
                crate::runners::write_ion_row(
                    &mut ours,
                    &["v".to_string()],
                    std::slice::from_ref(value),
                );
                let got = decoded(ours.clone().into_bytes());
                let class = match (got == kestra, ours == kestra_text) {
                    (true, true) => Same,
                    (true, false) => SameJsonOtherType,
                    (false, _) => Diverges,
                };
                assert_eq!(
                    class, *want,
                    "{ty} on {engine}: egret wrote {ours} -> {got:?}; Kestra {kestra_text} -> {kestra:?}"
                );
                seen.insert((ty, engine), class);
            }
        }
        // The divergences, named so a change in either direction is seen.
        let diverging: Vec<_> = seen
            .iter()
            .filter(|(_, c)| **c != Same)
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(
            diverging,
            vec![
                ("bool", "starrocks"),
                ("bytea", "starrocks"),
                ("bytea", "trino"),
                ("date", "starrocks"),
                ("date", "trino"),
                ("decimal", "starrocks"),
                ("decimal", "trino"),
                ("timestamp", "starrocks"),
                ("timestamp", "trino"),
                ("timestamptz", "trino"),
            ]
        );
    }

    /// **At the cap STORE fails, as `FETCH` does, and writes nothing.** A
    /// stored prefix would be a file that claims to be the result. The same
    /// result with the flag clear stores, so this cannot pass against a
    /// handler that refuses everything.
    #[tokio::test]
    async fn store_over_the_row_cap_fails_like_fetch_does() {
        let (fs, mem) = mem_files(Caps::default());
        let mut r = result(&["n"], vec![vec![serde_json::json!(1)]]);
        r.truncated = true;
        let st = store_input(&fs, "SELECT n FROM t");
        let out = Sql::with_limits(Spy::answering(r.clone()), 10, DEFAULT_TIMEOUT)
            .with_files(Some(fs.clone()))
            .run(&st.task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        let msg = out.message.unwrap();
        assert_eq!(
            msg,
            "egret.sql: the statement returned more than the 10 rows `fetch: STORE` \
             will put in an output, and a prefix is not an answer. Narrow the \
             statement, or use `fetch: NONE` if the rows are not needed"
        );
        let fetch = outputs_for(Fetch::All, &r, 10).unwrap_err();
        assert_eq!(
            msg.replace("`fetch: STORE`", "`fetch: FETCH`"),
            format!("egret.sql: {fetch}"),
            "the same message shape as FETCH's"
        );
        assert!(
            mem.keys().is_empty(),
            "nothing is written: {:?}",
            mem.keys()
        );
        assert!(out.files.is_empty());

        r.truncated = false;
        let out = Sql::with_limits(Spy::answering(r), 10, DEFAULT_TIMEOUT)
            .with_files(Some(fs))
            .run(&st.task)
            .await
            .unwrap();
        assert_eq!(
            out.state,
            RunState::Success,
            "the control: {:?}",
            out.message
        );
    }

    /// **No internal storage: the task fails naming `[storage]`, and the
    /// statement never runs** -- a statement whose rows have nowhere to go
    /// would cost the engine for nothing. The control stores.
    #[tokio::test]
    async fn store_without_a_store_fails_naming_storage_before_the_statement_runs() {
        let (fs, _mem) = mem_files(Caps::default());
        let spy = Spy::answering(result(&["n"], vec![vec![serde_json::json!(1)]]));
        let st = store_input(&fs, "SELECT 1 AS n");
        let out = Sql::new(spy.clone()).run(&st.task).await.unwrap();
        assert_eq!(out.state, RunState::Failed);
        let msg = out.message.unwrap_or_default();
        assert!(msg.contains("[storage]"), "{msg}");
        assert_eq!(spy.calls(), 0, "the statement must not run");

        let out = Sql::new(spy.clone())
            .with_files(Some(fs))
            .run(&st.task)
            .await
            .unwrap();
        assert_eq!(
            out.state,
            RunState::Success,
            "the control: {:?}",
            out.message
        );
        assert_eq!(spy.calls(), 1);
    }

    /// No prefix to write under (not a task run of a flow's execution: no
    /// scope, no `flow`) is the task's failure, before the statement runs.
    /// With `flow` in the context the prefix is built from it instead.
    #[tokio::test]
    async fn store_with_no_task_prefix_fails_the_task_and_runs_nothing() {
        let (fs, _mem) = mem_files(Caps::default());
        let spy = Spy::new();
        let spec = serde_json::json!({"database": db_id().to_string(), "sql": "SELECT 1", "fetch": "STORE"});
        let out = Sql::new(spy.clone())
            .with_files(Some(fs.clone()))
            .run(&input(spec.clone(), RenderContext::new()))
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        assert!(
            out.message
                .as_deref()
                .unwrap_or("")
                .contains("no storage prefix"),
            "{:?}",
            out.message
        );
        assert_eq!(spy.calls(), 0);

        let mut cx = RenderContext::new();
        cx.insert("flow", serde_json::json!({"namespace": NS, "id": FLOW}));
        let mut task = input(spec, cx);
        task.execution_id = Uuid::new_v4();
        task.run_id = Uuid::new_v4();
        let out = Sql::new(spy.clone())
            .with_files(Some(fs))
            .run(&task)
            .await
            .unwrap();
        let prefix = uri::task_prefix(NS, FLOW, task.execution_id, "q", task.run_id).unwrap();
        assert_eq!(
            out.outputs.as_ref().unwrap()["uri"],
            serde_json::json!(format!("{prefix}{STORE_FILE}")),
            "{:?}",
            out.message
        );
    }

    /// `workflow.storage.max_file_bytes` is the author's cap: a failed task
    /// naming it, nothing written. Judged before a byte is uploaded.
    #[tokio::test]
    async fn store_over_the_file_cap_fails_naming_it_and_writes_nothing() {
        let (fs, mem) = mem_files(Caps {
            max_file_bytes: 10,
            ..Caps::default()
        });
        let spy = Spy::answering(result(
            &["n"],
            vec![vec![serde_json::json!(1)], vec![serde_json::json!(2)]],
        ));
        let st = store_input(&fs, "SELECT n FROM t");
        let out = Sql::new(spy)
            .with_files(Some(fs))
            .run(&st.task)
            .await
            .unwrap();
        assert_eq!(out.state, RunState::Failed);
        let msg = out.message.unwrap_or_default();
        assert!(
            msg.contains("workflow.storage.max_file_bytes") && msg.contains("12"),
            "{msg}"
        );
        assert!(mem.keys().is_empty());
    }

    /// A store that fails while the file is written.
    struct Down;

    #[async_trait::async_trait]
    impl ObjectStore for Down {
        async fn put(&self, uri: &str, _: Vec<u8>) -> std::result::Result<(), StorageError> {
            Err(StorageError::S3(format!("{uri}: connection refused")))
        }
        async fn get(&self, uri: &str) -> std::result::Result<Vec<u8>, StorageError> {
            Err(StorageError::S3(format!("{uri}: connection refused")))
        }
        async fn delete(&self, _: &str) -> std::result::Result<(), StorageError> {
            Ok(())
        }
        async fn copy(&self, _: &str, _: &str) -> std::result::Result<(), StorageError> {
            Ok(())
        }
        async fn list(
            &self,
            _: &str,
            _: Option<&str>,
        ) -> std::result::Result<egret_storage::Listing, StorageError> {
            unreachable!()
        }
        async fn put_stream(
            &self,
            uri: &str,
            body: &mut (dyn tokio::io::AsyncRead + Send + Unpin),
            _: Option<u64>,
        ) -> std::result::Result<u64, StorageError> {
            // Read a little, then fail mid-upload, as a dropped connection does.
            let mut buf = [0u8; 4];
            let _ = tokio::io::AsyncReadExt::read(body, &mut buf).await;
            Err(StorageError::S3(format!(
                "{uri}: dispatch failure: connection refused"
            )))
        }
        async fn get_stream(
            &self,
            uri: &str,
        ) -> std::result::Result<Box<dyn tokio::io::AsyncBufRead + Send + Unpin>, StorageError>
        {
            Err(StorageError::S3(format!(
                "{uri}: dispatch failure: connection refused"
            )))
        }
    }

    fn down() -> Arc<FileStore> {
        Arc::new(FileStore {
            store: Arc::new(Down),
            bucket: "egret-test".to_string(),
            prefix: "workflow/".to_string(),
            caps: Caps::default(),
            retention_days: 30,
        })
    }

    /// **A store outage while writing is egret's, not the author's** (spec
    /// §11): `Err`, so the lease lapses and the run is re-claimed with no
    /// attempt spent -- never a failed task. The rows are many so the writer
    /// is still writing when the upload fails (it must not hang).
    #[tokio::test]
    async fn a_store_outage_while_writing_is_a_worker_failure_naming_the_uri() {
        let fs = down();
        let rows = (0..50_000).map(|i| vec![serde_json::json!(i)]).collect();
        let st = store_input(&fs, "SELECT n FROM t");
        let err = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            Sql::with_limits(
                Spy::answering(result(&["n"], rows)),
                100_000,
                DEFAULT_TIMEOUT,
            )
            .with_files(Some(fs))
            .run(&st.task),
        )
        .await
        .expect("the writer must not hang when the upload fails")
        .expect_err("an outage is not the author's error");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        let msg = err.to_string();
        assert!(
            msg.contains(&format!("{}{STORE_FILE}", st.prefix)) && msg.contains("unavailable"),
            "{msg}"
        );
    }

    /// **A store outage met while `sql:` rendered** (a `read()` in it) is
    /// egret's too (Task 4: a render with a file scope checks
    /// `infrastructure_failure`): `Err`, and the statement does not run.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_store_outage_in_the_sql_render_is_a_worker_failure_and_runs_nothing() {
        let fs = down();
        let ws = Uuid::new_v4();
        let ex = Uuid::new_v4();
        let file = uri::execution_file_uri(NS, FLOW, ex, "x.txt").unwrap();
        let mut files = std::collections::BTreeMap::new();
        files.insert(
            file.clone(),
            Ok(storage::IndexedFile {
                uri: file.clone(),
                execution_id: ex,
                task_run_id: None,
                size: 3,
            }),
        );
        let scope = storage::scope::Snapshot::from_parts(files, fs, ws, None).into_scope();
        let spy = Spy::new();
        let mut task = input(
            serde_json::json!({
                "database": db_id().to_string(),
                "sql": format!("SELECT {{{{ read('{file}') }}}}"),
            }),
            RenderContext::new().with_files(scope),
        );
        task.workspace_id = ws;
        let err = Sql::new(spy.clone())
            .run(&task)
            .await
            .expect_err("an outage in the render is not the author's error");
        assert!(matches!(err, Error::Internal(_)), "{err:?}");
        assert!(err.to_string().contains(&file), "{err}");
        assert_eq!(spy.calls(), 0);
    }

    #[test]
    fn the_fetch_spellings_include_store() {
        assert_eq!(
            serde_json::from_value::<Fetch>(serde_json::json!("STORE")).unwrap(),
            Fetch::Store
        );
        assert!(serde_json::from_value::<Fetch>(serde_json::json!("store")).is_err());
    }
}
