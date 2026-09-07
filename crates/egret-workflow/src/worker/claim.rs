//! The claim, the tick, and the one transaction the tick's result lands in.
//!
//! # The statement is `egret-automation`'s
//!
//! `crates/egret-automation/src/scheduler.rs` already claims work with
//! `UPDATE … WHERE id IN (SELECT … FOR UPDATE SKIP LOCKED LIMIT n) RETURNING …`,
//! and its module doc (`scheduler.rs:1-17`) and its
//! `two_concurrent_claimers_never_take_the_same_row` (`scheduler.rs:265`) give
//! the reasons — both halves of them: what `SKIP LOCKED` buys, and why marking
//! the row in the same statement is the other half of the answer. [`claim`] is
//! that statement over `workflow.task_runs`; the reasoning is not restated
//! here.
//!
//! Two differences from the schedules version, both forced by what a task run
//! is rather than chosen:
//!
//! - it is wrapped in a CTE, because the caller needs `workspace_id`, which
//!   lives on `workflow.executions` and a `RETURNING` cannot reach;
//! - there is **no `not_before` term**. Spec §4.1 writes the predicate as
//!   `state = 'pending' AND not_before <= now()`, and that second half cannot
//!   fire here: `TaskRun::not_before` is written only by
//!   `Command::ScheduleRetry`, `executor::retry::reopen_due` is its only
//!   reader, and a run carrying one is `RETRYING`, never `RUNNING`. A term on
//!   it would be a guard nobody could ever see fail. The delay is `advance()`'s
//!   to enforce and it already does.

use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use super::spawn::{self, Requested};
use super::{from_wire, Outcome, Registry, TaskInput, WorkerError, MAX_ROUNDS};
use crate::executor::{
    advance, apply, build_plan, Attempt, Command, ExecutionState, Plan, RunState, TaskKind,
    TaskRun, Tick,
};

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

fn worker_err(e: WorkerError) -> Error {
    Error::Internal(anyhow::anyhow!(e))
}

/// A task run this worker has taken a lease on.
///
/// Everything a handler needs plus everything [`apply_outcome`] needs, read in
/// the claim itself: a second read to find out what was claimed would be a
/// second chance for the row to have changed.
#[derive(Clone, Debug, PartialEq, Eq, sqlx::FromRow)]
pub struct Claimed {
    /// The `workflow.task_runs` surrogate key.
    pub id: Uuid,
    pub execution_id: Uuid,
    pub workspace_id: Uuid,
    /// The `RunIx` — the index into `ExecutionState::runs`.
    pub run_ix: i32,
    /// The `TaskIx` — the index into `Plan::tasks`.
    pub task_ix: i32,
    pub task_id: String,
    /// Non-null by construction: the claim only ever takes a row whose
    /// `type_id` is set, which is exactly a `TaskKind::Runnable`.
    pub type_id: String,
    pub value: Option<String>,
    /// Which attempt this is, 1-based — the number of attempts the run has,
    /// and the claim reads it as `jsonb_array_length(attempts)` because a
    /// handler that wants to know it is a retry must not have to ask twice.
    pub attempt: i32,
    /// The execution's frozen flow source.
    ///
    /// Read in the claim itself, for [`Claimed`]'s own reason: a handler needs
    /// the task's properties, the `Plan` deliberately does not carry them
    /// (`TaskKind::Runnable` holds a `type_id` and nothing else), and a second
    /// read would be a second chance for the row to have changed.
    pub source: String,
}

impl Claimed {
    /// What the handler is handed.
    ///
    /// `spec` is this task's own properties, found by walking the parsed flow
    /// for [`Claimed::task_id`]. Task ids are unique across a whole flow —
    /// `spec::validate`'s `duplicate_task_id` is a structural check over the
    /// nested walk, not just `flow.tasks` — so the id is a key and not a
    /// guess.
    fn input(&self) -> Result<TaskInput> {
        let flow = crate::spec::parse(&self.source)
            .map_err(|e| worker_err(WorkerError::Unplannable(self.execution_id, e.message)))?;
        let spec = leaf_spec(&flow, &self.task_id).ok_or_else(|| {
            worker_err(WorkerError::Corrupt(
                self.id,
                format!(
                    "task {:?} is not a leaf of the execution's own source",
                    self.task_id
                ),
            ))
        })?;
        Ok(TaskInput {
            workspace_id: self.workspace_id,
            execution_id: self.execution_id,
            run_id: self.id,
            task_id: self.task_id.clone(),
            type_id: self.type_id.clone(),
            value: self.value.clone(),
            attempt: self.attempt.max(0) as usize,
            spec,
            render: render_context_for(self),
        })
    }
}

/// One task's own properties, by id, from anywhere in the flow.
///
/// `models::TaskKind::Leaf`'s `spec` is the task's map with `id`, `type`,
/// `description`, `retry`, `timeout`, `disabled` and `runIf` already removed
/// by `models::Task`'s deserializer, which is exactly a handler's surface. A
/// non-leaf id answers `None`: the claim never takes one (`type_id IS NOT
/// NULL`), so reaching that arm is a corruption rather than a case.
fn leaf_spec(flow: &crate::models::Flow, task_id: &str) -> Option<serde_json::Value> {
    fn walk(task: &crate::models::Task, want: &str) -> Option<serde_json::Value> {
        if task.id == want {
            return match &task.kind {
                crate::models::TaskKind::Leaf { spec } => Some(spec.clone()),
                _ => None,
            };
        }
        task.children().into_iter().find_map(|c| walk(c, want))
    }
    flow.tasks
        .iter()
        .chain(flow.errors.iter())
        .find_map(|t| walk(t, task_id))
}

/// What a handler's `{{ … }}` renders against.
///
/// **Empty, and this function exists so that is one edit rather than a
/// search.** `advance()` cannot render (it is pure) and neither can this
/// worker yet: spec §4.4 gives the render surface — `inputs`, `vars`,
/// `outputs`, `taskrun`, `tasksWithState()` — to W3b Task 5, which is also
/// where the widening's consequences have to be attributed. Until then a task
/// whose property references a variable fails to render and the run goes
/// `FAILED`; that is a divergence from Kestra in the safe direction (a
/// refusal, never an over-acceptance) and `tests/worker.rs`'s
/// `a_task_whose_property_needs_a_render_fails_rather_than_succeeding` pins
/// it so the day it changes, something says so.
fn render_context_for(_claimed: &Claimed) -> crate::runners::RenderContext {
    crate::runners::RenderContext::new()
}

/// Take up to `limit` runnable task runs, marking each leased.
///
/// Accepts any executor for the reason `scheduler::claim_due` documents: a test
/// holds the claim open inside a transaction and watches a second claimer skip
/// it; production passes the pool and gets autocommit.
///
/// **`type_id IS NOT NULL` is what separates a runnable from a flowable.** Both
/// reach `RUNNING`, and in this table nothing else tells them apart — see
/// migration 0134's comment on the column, and `executor::flowable::start` for
/// the measurement it comes from. Without it a worker would run a `Parallel`.
pub async fn claim(exec: impl sqlx::PgExecutor<'_>, limit: i64) -> Result<Vec<Claimed>> {
    sqlx::query_as(
        "WITH taken AS ( \
           UPDATE workflow.task_runs \
              SET claimed_at = now(), updated_at = now() \
            WHERE id IN ( \
              SELECT id FROM workflow.task_runs \
               WHERE type_id IS NOT NULL \
                 AND state = 'RUNNING' \
                 AND claimed_at IS NULL \
               ORDER BY created_at \
               FOR UPDATE SKIP LOCKED \
               LIMIT $1 \
            ) \
            RETURNING id, execution_id, run_ix, task_ix, task_id, type_id, value, \
                      jsonb_array_length(attempts) AS attempt \
         ) \
         SELECT t.id, t.execution_id, e.workspace_id, t.run_ix, t.task_ix, t.task_id, \
                t.type_id, t.value, t.attempt, e.source \
           FROM taken t JOIN workflow.executions e ON e.id = t.execution_id",
    )
    .bind(limit)
    .fetch_all(exec)
    .await
    .map_err(db_err)
}

/// Claim up to `n` runnable task runs, hand each to `reg`, and commit each
/// result with `advance()`'s commands and the successors.
///
/// # Two commits, not one
///
/// The claim commits on its own; the result commits in its own transaction.
/// The `worker` module doc gives the reason (a handler is arbitrarily long and
/// must not hold a row lock) and the cost (a killed worker leaves a lease for
/// [`reap`](super::reap)). What is **not** split is the thing the plan's Global
/// Constraint names: the handler's result, `advance()`'s commands and the
/// successors are one transaction, so no window exists in which a task is
/// finished and the execution has not moved.
///
/// Returns the number of runs whose result committed.
pub async fn tick(pool: &PgPool, n: usize, reg: &dyn Registry) -> Result<usize> {
    let claimed = claim(pool, n as i64).await?;
    let mut done = 0usize;
    for c in &claimed {
        let outcome = reg.run(&c.input()?).await?;
        let mut tx = pool.begin().await.map_err(db_err)?;
        apply_outcome(&mut tx, c, &outcome).await?;
        tx.commit().await.map_err(db_err)?;
        done += 1;
    }
    Ok(done)
}

/// Record one handler's result, drive `advance()` to a fixed point, and write
/// everything it asked for.
///
/// **The caller owns the transaction and nothing here commits**, which is what
/// lets `tests/worker.rs` drive a killed process by dropping it. Every
/// statement runs on the connection it is given, so there is no way to write
/// half of this outside the caller's transaction without changing this
/// signature.
///
/// Order matters and is not incidental: the claimed run's row is written
/// **before** the successors are inserted, so a failure on a successor leaves a
/// visibly finished run behind if — and only if — these statements are not in
/// one transaction. That is the window
/// `a_failure_partway_through_a_tick_leaves_nothing_half_applied` looks for.
pub async fn apply_outcome(
    conn: &mut PgConnection,
    claimed: &Claimed,
    outcome: &Outcome,
) -> Result<()> {
    let (ctx, mut state, now) = load(conn, claimed.execution_id).await?;
    let before = state.clone();

    let run = usize::try_from(claimed.run_ix).map_err(|_| {
        worker_err(WorkerError::Corrupt(
            claimed.id,
            format!("negative run_ix {}", claimed.run_ix),
        ))
    })?;
    apply(
        &mut state,
        &Command::SetRunState {
            run,
            state: outcome.state,
        },
    );
    settle(conn, &ctx, &before, &mut state, now, Some(claimed.run_ix)).await
}

/// Everything one round of `advance()` implies, written on one connection.
///
/// Four things, in this order and for this reason:
///
/// 1. **drive** `advance()` to a fixed point, collecting the
///    `Command::SpawnExecution`s — nothing in `executor::apply` records a
///    spawn, because `ExecutionState` has nowhere to put one;
/// 2. **persist** the runs that changed and the execution's own state;
/// 3. **spawn** the sub-executions, which read the run rows step 2 has just
///    written;
/// 4. if this execution has itself just reached a terminal state, **tell its
///    parent**, which is where a `Loop` starts its next iteration or finishes.
///
/// The caller owns the transaction and nothing here commits, so all four land
/// together or none does. That is the plan's Global Constraint one level up
/// from where Task 2 left it: with a spawner, "the execution has not moved"
/// includes "the iteration it started does not exist".
pub(super) async fn settle(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    before: &ExecutionState,
    state: &mut ExecutionState,
    now: Tick,
    release: Option<i32>,
) -> Result<()> {
    let spawns = drive(&ctx.plan, state, now, ctx.id)?;
    persist(conn, ctx, before, state, release).await?;
    spawn::spawn_all(conn, ctx, state, &spawns).await?;

    let was = before.history.last().copied();
    let is = state.history.last().copied();
    if was != is {
        if let Some(terminal) = is.filter(|s| s.is_terminal()) {
            spawn::on_execution_terminal(conn, ctx.id, terminal).await?;
        }
    }
    Ok(())
}

/// Create an execution for `source` and drive `advance()` until it is waiting
/// on a worker.
///
/// The counterpart to [`tick`]: nothing can be claimed until something has been
/// started, and both halves of that go through the same [`drive`] and
/// [`persist`], so a run created at start and a run created as a successor are
/// the same row written by the same code.
pub async fn start_execution(
    pool: &PgPool,
    workspace_id: Uuid,
    flow_id: Option<Uuid>,
    source: &str,
) -> Result<Uuid> {
    let flow = crate::spec::parse(source)
        .map_err(|e| Error::InvalidInput(format!("the flow does not parse: {}", e.message)))?;
    let plan = build_plan(source)
        .map_err(|e| Error::InvalidInput(format!("the flow does not plan: {e}")))?;

    let mut tx = pool.begin().await.map_err(db_err)?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history) \
         VALUES ($1, $2, $3, $4, $5, 'CREATED', '[\"CREATED\"]'::jsonb) RETURNING id",
    )
    .bind(workspace_id)
    .bind(flow_id)
    .bind(&flow.namespace)
    .bind(&flow.id)
    .bind(source)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;

    let ctx = ExecutionCtx {
        id,
        workspace_id,
        flow_id,
        source: source.to_string(),
        plan,
    };
    let empty = ExecutionState::new(&ctx.plan);
    let mut state = empty.clone();
    // `Tick(0)` is the execution's own origin — `Tick` is milliseconds since
    // the execution was created (`executor::state::Tick`), and this is that
    // instant.
    settle(&mut tx, &ctx, &empty, &mut state, Tick(0), None).await?;
    tx.commit().await.map_err(db_err)?;
    Ok(id)
}

/// Drive an execution forward from whatever the **caller** has just supplied.
///
/// [`tick`] drives an execution when a task finishes; this is the entry point
/// for everything else that changes what `advance()` can see. Today that is
/// the render slots — `TaskRun::rendered` and `rendered_run_if`, which
/// `advance()` cannot fill because it is pure and which this worker does not
/// fill either (W3b Task 5 owns the render surface). A `Loop`, an `If`, a
/// `Switch` or a `runIf:` therefore waits until a caller writes the slot and
/// calls this.
///
/// It takes the same lock and runs the same [`settle`] as every other writer,
/// so a run created here and a run created by a finishing task are the same
/// row written by the same code.
pub async fn resume(pool: &PgPool, execution_id: Uuid) -> Result<()> {
    let mut tx = pool.begin().await.map_err(db_err)?;
    let (ctx, mut state, now) = load(&mut tx, execution_id).await?;
    let before = state.clone();
    settle(&mut tx, &ctx, &before, &mut state, now, None).await?;
    tx.commit().await.map_err(db_err)
}

/// Everything a writer needs to know about the execution it is writing.
///
/// The `Plan` is rebuilt from `source` rather than stored — `build_plan` is
/// pure and deterministic over it, so a stored plan is a second copy of a
/// derivable thing that can disagree with the source beside it — and
/// `workspace_id`/`flow_id` are carried because a spawn creates a *new*
/// execution and has to give it both.
pub(super) struct ExecutionCtx {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub flow_id: Option<Uuid>,
    pub source: String,
    pub plan: Plan,
}

/// Read the execution, **locking it**, and rebuild what `advance()` reads.
///
/// `FOR UPDATE` is the serialisation two workers finishing two siblings of the
/// same `Parallel` need: without it both would read the same `ExecutionState`,
/// both would compute the same successor, and both would try to create it. The
/// `UNIQUE (execution_id, run_ix)` in migration 0134 is what happens if this
/// lock is ever lost — a collision rather than a duplicate run.
pub(super) async fn load(
    conn: &mut PgConnection,
    execution_id: Uuid,
) -> Result<(ExecutionCtx, ExecutionState, Tick)> {
    let row = sqlx::query(
        "SELECT source, history, workspace_id, flow_id, \
                (EXTRACT(EPOCH FROM (now() - created_at)) * 1000)::bigint AS now_ms \
           FROM workflow.executions WHERE id = $1 FOR UPDATE",
    )
    .bind(execution_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db_err)?
    .ok_or_else(|| Error::NotFound(format!("execution {execution_id}")))?;

    let source: String = row.get("source");
    let history: serde_json::Value = row.get("history");
    let now_ms: i64 = row.get("now_ms");
    let plan = build_plan(&source)
        .map_err(|e| worker_err(WorkerError::Unplannable(execution_id, e.to_string())))?;
    let ctx = ExecutionCtx {
        id: execution_id,
        workspace_id: row.get("workspace_id"),
        flow_id: row.get("flow_id"),
        source,
        plan,
    };

    let rows = sqlx::query(
        "SELECT id, run_ix, task_ix, parent_ix, value, rendered, rendered_run_if, \
                history, attempts, not_before_ms \
           FROM workflow.task_runs WHERE execution_id = $1 ORDER BY run_ix",
    )
    .bind(execution_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db_err)?;

    let mut runs = Vec::with_capacity(rows.len());
    for (ix, r) in rows.iter().enumerate() {
        let run_ix: i32 = r.get("run_ix");
        let id: Uuid = r.get("id");
        // `RunIx` IS the position in this vector, and `advance()` addresses
        // runs by it. A gap would silently hand the executor a different state
        // from the one it wrote, so it is an error rather than a renumbering.
        if run_ix as usize != ix {
            return Err(worker_err(WorkerError::Corrupt(
                id,
                format!("run_ix {run_ix} at position {ix}"),
            )));
        }
        runs.push(TaskRun {
            task: r.get::<i32, _>("task_ix") as usize,
            parent: r.get::<Option<i32>, _>("parent_ix").map(|p| p as usize),
            value: r.get("value"),
            rendered: r.get("rendered"),
            rendered_run_if: r.get("rendered_run_if"),
            history: parse_states(id, &r.get::<serde_json::Value, _>("history"))?,
            attempts: parse_attempts(id, &r.get::<serde_json::Value, _>("attempts"))?,
            not_before: r.get::<Option<i64>, _>("not_before_ms").map(Tick),
        });
    }

    let state = ExecutionState {
        runs,
        history: parse_states(execution_id, &history)?,
    };
    Ok((ctx, state, Tick(now_ms)))
}

/// Drive `advance()` to a fixed point, applying every command it asks for and
/// **collecting** the spawns.
///
/// `Command::SpawnExecution` is the one command `executor::apply` has no arm
/// for, and that is not an omission: [`ExecutionState`] holds nothing about a
/// sub-execution, so there is nothing for `apply` to record. It comes back to
/// the caller instead, which is the only thing that can write one — see
/// [`spawn::spawn_all`].
///
/// Bounded by [`MAX_ROUNDS`]: a caller that does not converge has found a cycle
/// or is failing to fill a render slot, and both must be loud rather than spin
/// with a transaction open.
fn drive(
    plan: &Plan,
    state: &mut ExecutionState,
    now: Tick,
    execution_id: Uuid,
) -> Result<Vec<Requested>> {
    let mut spawns = Vec::new();
    for _ in 0..MAX_ROUNDS {
        let cmds = advance(plan, state, now);
        if cmds.is_empty() {
            return Ok(spawns);
        }
        for cmd in &cmds {
            spawns.extend(Requested::of(cmd));
            apply(state, cmd);
        }
    }
    Err(worker_err(WorkerError::NoFixedPoint(execution_id)))
}

/// Write the difference between `before` and `after`.
///
/// `release` names the run whose lease this write clears — `None` at
/// [`start_execution`], where nothing is leased. Only that run's lease is
/// touched: another worker may hold a lease on a sibling of the same execution,
/// and clearing it would hand that sibling to a second handler.
async fn persist(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    before: &ExecutionState,
    after: &ExecutionState,
    release: Option<i32>,
) -> Result<()> {
    let execution_id = ctx.id;
    let plan = &ctx.plan;
    for (ix, run) in after.runs.iter().enumerate() {
        let history = states_json(&run.history);
        let attempts = attempts_json(&run.attempts);
        let state = run
            .history
            .last()
            .copied()
            .unwrap_or(RunState::Created)
            .wire();
        if ix < before.runs.len() {
            if before.runs[ix] == *run && release != Some(ix as i32) {
                continue;
            }
            sqlx::query(
                "UPDATE workflow.task_runs \
                    SET state = $1, history = $2, attempts = $3, rendered = $4, \
                        rendered_run_if = $5, not_before_ms = $6, \
                        claimed_at = CASE WHEN $7 THEN NULL ELSE claimed_at END, \
                        updated_at = now() \
                  WHERE execution_id = $8 AND run_ix = $9",
            )
            .bind(state)
            .bind(&history)
            .bind(&attempts)
            .bind(run.rendered.as_deref())
            .bind(run.rendered_run_if.as_deref())
            .bind(run.not_before.map(|t| t.0))
            .bind(release == Some(ix as i32))
            .bind(execution_id)
            .bind(ix as i32)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        } else {
            // A plain INSERT, never an upsert: a `CreateRun` for a `run_ix`
            // that already exists is two workers having advanced the same
            // execution, and `UNIQUE (execution_id, run_ix)` turning that into
            // a failed transaction is the point (migration 0134).
            let task = plan.tasks.get(run.task).ok_or_else(|| {
                worker_err(WorkerError::Corrupt(
                    execution_id,
                    format!(
                        "run {ix} names task {} which the plan does not hold",
                        run.task
                    ),
                ))
            })?;
            let type_id = match &task.kind {
                TaskKind::Runnable { type_id } => Some(type_id.as_str()),
                _ => None,
            };
            sqlx::query(
                "INSERT INTO workflow.task_runs \
                   (execution_id, run_ix, task_ix, task_id, type_id, parent_ix, value, \
                    rendered, rendered_run_if, state, history, attempts, not_before_ms) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
            )
            .bind(execution_id)
            .bind(ix as i32)
            .bind(run.task as i32)
            .bind(&task.id)
            .bind(type_id)
            .bind(run.parent.map(|p| p as i32))
            .bind(run.value.as_deref())
            .bind(run.rendered.as_deref())
            .bind(run.rendered_run_if.as_deref())
            .bind(state)
            .bind(&history)
            .bind(&attempts)
            .bind(run.not_before.map(|t| t.0))
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }
    }

    if before.history != after.history {
        sqlx::query(
            "UPDATE workflow.executions SET state = $1, history = $2, updated_at = now() \
              WHERE id = $3",
        )
        .bind(
            after
                .history
                .last()
                .copied()
                .unwrap_or(RunState::Created)
                .wire(),
        )
        .bind(states_json(&after.history))
        .bind(execution_id)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
    }
    Ok(())
}

fn states_json(states: &[RunState]) -> serde_json::Value {
    serde_json::Value::Array(
        states
            .iter()
            .map(|s| serde_json::Value::String(s.wire().to_string()))
            .collect(),
    )
}

fn attempts_json(attempts: &[Attempt]) -> serde_json::Value {
    serde_json::Value::Array(
        attempts
            .iter()
            .map(|a| {
                serde_json::json!({
                    "history": states_json(&a.history),
                    "started_at": a.started_at.0,
                })
            })
            .collect(),
    )
}

fn parse_states(owner: Uuid, v: &serde_json::Value) -> Result<Vec<RunState>> {
    let arr = v.as_array().ok_or_else(|| {
        worker_err(WorkerError::Corrupt(
            owner,
            format!("a state history must be a list, not {v}"),
        ))
    })?;
    arr.iter()
        .map(|s| {
            s.as_str()
                .and_then(from_wire)
                .ok_or_else(|| worker_err(WorkerError::UnknownState(owner, s.to_string())))
        })
        .collect()
}

fn parse_attempts(owner: Uuid, v: &serde_json::Value) -> Result<Vec<Attempt>> {
    let arr = v.as_array().ok_or_else(|| {
        worker_err(WorkerError::Corrupt(
            owner,
            format!("an attempt list must be a list, not {v}"),
        ))
    })?;
    arr.iter()
        .map(|a| {
            let history =
                parse_states(owner, a.get("history").unwrap_or(&serde_json::Value::Null))?;
            let started_at = a
                .get("started_at")
                .and_then(|t| t.as_i64())
                .ok_or_else(|| {
                    worker_err(WorkerError::Corrupt(
                        owner,
                        format!("attempt {a} has no started_at"),
                    ))
                })?;
            Ok(Attempt {
                history,
                started_at: Tick(started_at),
            })
        })
        .collect()
}
