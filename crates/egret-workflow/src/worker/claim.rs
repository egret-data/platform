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
//! - there is **no `not_before` term on the claim**, and both halves of the
//!   reason that used to be written here were wrong.
//!
//!   The half that reads like a guarantee — *"the delay is `advance()`'s to
//!   enforce and it already does"* — was withdrawn by W3b, which enumerated
//!   why nothing in this worker made a retry's delay elapse. The half that
//!   replaced it — *"a term on it would be a guard nobody could ever see
//!   fail"* — is false too, and W4 Task 1 is what makes it false: the term
//!   exists, it is in [`due_retries`], and
//!   `a_retry_whose_delay_has_elapsed_is_woken_and_one_still_waiting_is_left_alone`
//!   has seen it fail in both directions.
//!
//!   What is true is narrower and is about **which statement** carries the
//!   term. A `RETRYING` run is not runnable: it has to re-enter `CREATED`
//!   first, and only `advance()` may put it there
//!   (`executor::retry::reopen_due`). So the claim — whose whole job is to
//!   hand a run to a handler — cannot take one, and a `not_before` term on
//!   *this* predicate really would be unreachable, because
//!   `TaskRun::not_before` is written only by `Command::ScheduleRetry` and a
//!   run carrying one is `RETRYING` rather than `RUNNING`. The waker is
//!   therefore a **second statement** rather than a widened claim:
//!   [`due_retries`] names the executions holding a run whose deadline has
//!   passed, [`wake`] calls [`resume`] on each, and [`tick`] runs it **ahead
//!   of** the claim so a run this call reopens is claimed by the same call.
//!
//!   **The two clocks are one expression, deliberately.** [`due_retries`]'s
//!   `now` is spelled exactly as [`load_with`] spells it —
//!   `(EXTRACT(EPOCH FROM (now() - e.created_at)) * 1000)::bigint`, the origin
//!   `Tick` is measured from — so an execution the waker names is one
//!   `reopen_due` will actually move. Two different readings of the clock
//!   would give a waker that resumes an execution `advance()` then declines to
//!   advance: a settle that writes nothing, bumps `executions.seq`, and is
//!   selected again on the next tick for ever. `now()` only ever grows between
//!   the two statements, and that is the safe direction — a run the waker
//!   found due is still due by the time `resume` reads the clock.
//!
//!   What this does **not** do is invent a state. The waker moves nothing
//!   itself; it re-reads the clock and hands the execution to the same
//!   [`settle`] every other writer uses, exactly as
//!   `oracle-exec/driver.rs`'s `pass_time` did — except that `pass_time`
//!   *rewinds* `executions.created_at` to fake the delay, and is why this hole
//!   was invisible for a whole package: the only caller `resume` had was one
//!   that had already moved the clock itself.

use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use super::cancel;
use super::render::{self, Pass, Surface};
use super::replay;
use super::spawn::{self, Requested};
use super::stamps::{self, stamp_columns, stamp_set, stamp_values};
use super::subflow;
use super::timeout::{self, Budget};
use super::txn::{transaction, Ended, Fetched, Finish, Gate, InTxn, Isolation, Txn};
use super::{from_wire, Outcome, Registry, TaskInput, WorkerError, MAX_ROUNDS};
use crate::executor::{
    advance, apply, build_plan, Attempt, Command, ExecutionMeta, ExecutionState, Plan, Resume,
    RunIx, RunState, TaskKind, TaskRun, Tick,
};
#[cfg(any(test, feature = "test-util"))]
use crate::storage::prefetch::Prefetch;

/// Keeps the `sqlx::Error` as the source rather than flattening it to text:
/// the Display is unchanged, and `scheduler::fire` downcasts it to tell a
/// duplicate schedule tick (a unique violation) from a transient failure.
fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::Error::new(e))
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
    /// The identity this execution acts as — the flow's owner, or `None`.
    ///
    /// Read in the same statement as everything else, for the reason above, and
    /// through a LEFT JOIN because `executions.flow_id` is nullable
    /// (`ON DELETE SET NULL`, so the record of what a deleted flow did survives
    /// it) and `flows.created_by` is nullable in its turn. Both absences answer
    /// `None`, which is what [`TaskInput::principal`] means and what
    /// [`super::handlers::sql`] refuses on.
    pub principal_id: Option<Uuid>,
    /// **The fencing token: the instant this lease was taken.**
    ///
    /// Returned by the claim's own `UPDATE … SET claimed_at = now()`, so it is
    /// the value that statement wrote and not a value read afterwards. Every
    /// write [`apply_outcome`] makes carries `AND claimed_at = $n` on it, and a
    /// write that matches no row is [`WorkerError::LeaseLost`].
    ///
    /// **Without it there is no value a stale worker could be checked
    /// against**, and that is how the hole shipped: the claim's `RETURNING`
    /// selected `created_at`, so `apply_outcome`'s `WHERE execution_id = $8
    /// AND run_ix = $9` was the whole predicate. [`super::reap`] clears
    /// `claimed_at` and leaves `state = 'RUNNING'` — the claim predicate — so
    /// a run is re-claimable while its first handler is still in flight. Two
    /// terminals landed on one attempt, the second wiping the first's
    /// `ended_seq`. See
    /// `tests/worker.rs::a_worker_whose_lease_was_reaped_cannot_commit`.
    ///
    /// **W4 Task 3's kill path does not close this, and the fence is still the
    /// whole of the answer.** [`super::TaskHandler::cancel`] has a caller now,
    /// but only one: the task-level budget expiring, inside the worker that
    /// owns the run. [`super::reap`] is a bare `UPDATE` run by some other
    /// process and a lease is a `timestamptz`, not a handle to a future, so
    /// nothing can reach across and stop the abandoned handler — it learns it
    /// was reaped when this fence refuses its write and not before.
    /// `a_killed_handler_cannot_commit_over_the_worker_that_replaced_it` drives
    /// that through the shipping `tick`.
    pub claimed_at: chrono::DateTime<chrono::Utc>,
}

impl Claimed {
    /// What the handler is handed, and the budget it is handed under.
    ///
    /// `spec` is this task's own properties, found by walking the parsed flow
    /// for [`Claimed::task_id`]. Task ids are unique across a whole flow —
    /// `spec::validate`'s `duplicate_task_id` is a structural check over the
    /// nested walk, not just `flow.tasks` — so the id is a key and not a
    /// guess.
    ///
    /// **The budget comes back beside the input rather than only on it**,
    /// because a `timeout:` that cannot be used is a *refusal* and a refusal
    /// is not a `None` budget: the run fails without the handler being called
    /// at all. See [`timeout::Budget`], and [`run_one`] for the branch.
    ///
    /// **One walk, not two.** `spec` and `timeout` are two fields of the same
    /// `models::Task`, and a second traversal to fetch the second one could
    /// disagree with the first about which task it found.
    ///
    /// **`log` is this attempt's handle** (W11), built here from the
    /// registry's sink so every path through [`run_one`] has one to finish.
    fn input(
        &self,
        render: crate::runners::RenderContext,
        log: (
            std::sync::Arc<dyn super::handlers::LogSink>,
            crate::logs::LogLimits,
        ),
        script_default: std::time::Duration,
    ) -> Result<(TaskInput, Budget)> {
        let flow = crate::spec::parse(&self.source)
            .map_err(|e| worker_err(WorkerError::Unplannable(self.execution_id, e.message)))?;
        let leaf = leaf(&flow, &self.task_id).ok_or_else(|| {
            worker_err(WorkerError::NotALeaf(
                self.execution_id,
                self.task_id.clone(),
            ))
        })?;
        let budget = timeout::defaulted(
            timeout::resolve(leaf.timeout, &render),
            &self.type_id,
            script_default,
        );
        Ok((
            TaskInput {
                workspace_id: self.workspace_id,
                execution_id: self.execution_id,
                run_id: self.id,
                task_id: self.task_id.clone(),
                type_id: self.type_id.clone(),
                value: self.value.clone(),
                attempt: self.attempt.max(0) as usize,
                spec: leaf.spec.clone(),
                render,
                timeout: budget.duration(),
                principal: self.principal_id.map(egret_core::UserId),
                claimed_at: self.claimed_at,
                log: crate::logs::RunLog::new(
                    crate::logs::RunIdentity {
                        workspace_id: self.workspace_id,
                        execution_id: self.execution_id,
                        task_run_id: self.id,
                        task_id: self.task_id.clone(),
                        attempt: crate::logs::RunIdentity::stored_attempt(
                            self.attempt.max(0) as usize
                        ),
                    },
                    log.0,
                    log.1,
                ),
            },
            budget,
        ))
    }
}

/// The two halves of a leaf task this worker needs: what the handler runs on,
/// and how long it may take.
///
/// A struct rather than a tuple because the second field is easy to mistake
/// for the first's `timeout` key — it is not in `spec` at all, which is the
/// whole seam this task exists to close.
struct Leaf<'a> {
    /// `models::TaskKind::Leaf`'s `spec`.
    spec: &'a serde_json::Value,
    /// `models::Task::timeout`, still unrendered.
    timeout: Option<&'a crate::models::Property<std::time::Duration>>,
}

/// One task's own properties **and its `timeout:`**, by id, from anywhere in
/// the flow.
///
/// `models::TaskKind::Leaf`'s `spec` is the task's map with `id`, `type`,
/// `description`, `retry`, `timeout`, `disabled` and `runIf` already removed
/// by `models::Task`'s deserializer, which is exactly a handler's surface.
///
/// **A non-leaf id answers `None`, and that is a CASE.** This comment used to
/// say the opposite — *"the claim never takes one (`type_id IS NOT NULL`), so
/// reaching that arm is a corruption rather than a case"* — and the corpus
/// says otherwise: `build_plan` plans `AllowFailure`, `WorkingDirectory` and
/// `LoopUntil` opaque, which gives them a `type_id` and therefore
/// makes them claimable, while `models::TaskKind` for the same task is a
/// **flowable** — so the `Leaf` arm below cannot match and this returns `None`.
/// Forty-odd comparable entries
/// reached it when that was measured. **This list named `Subflow` first and
/// that has been false since W6 Task 3**, which is worse than a stale name
/// because the list is the whole argument for the arm being reachable: a
/// `Subflow` is [`crate::executor::TaskKind::Executable`] now, `type_id` below
/// is written only for a `Runnable`, and the claim's own
/// `WHERE type_id IS NOT NULL` therefore cannot take one. The arm is still a
/// case — the **three** named above are still opaque — but the count was
/// measured with `Subflow` in the set and has not been re-derived since.
/// **The subtrahend is already in this crate**, which is better than a figure a
/// reader must distrust: `tests/executor.rs`'s `ENTRIES_LEFT_UNFINISHED` doc
/// (*"21 left at Task 3 and they were the whole of `Subflow`"*) records that
/// **all 21 entries holding an executed top-level `Subflow`
/// were in this population for that reason and none is now**. So: 40-odd, of
/// which 21 left at W6 Task 3.
///
/// **`LoopUntil` only — Kestra's `WaitFor` alias is not egret's.** This list
/// read `WaitFor`/`LoopUntil` until W8 Task 1 fix round 5. `models::kind_for`
/// has no `"waitfor"` row, so a `WaitFor` reaches `models::TaskKind::Leaf` and
/// **this function returns `Some`** for it — an ordinary leaf with an opaque
/// `spec`, by exactly the argument the next paragraph makes for `Pause`, and
/// stopped by the same later gate, [`WorkerError::NoHandler`]. The **count**
/// survives — three constructs still reach the `None` arm — but the
/// **membership** does not, and that is the half a reader uses: **7 of the
/// corpus's 10 flows of that construct take the `WaitFor` spelling**, so
/// someone following this list to find a `WaitFor` among the 40-odd finds
/// none. Fix round 4 measured the model kind and fix round 5 re-measured it;
/// the model divergence itself is the plan's Task 4 Step 0.
///
/// **`Pause` is NOT one of those three either, and it no longer reaches this
/// function at all.** W8 Task 1's first attempt put it among the three; W8 Task
/// 1 fix round 1 corrected that to "it reaches `TaskKind::Leaf` and this
/// function returns `Some`, and the NEXT gate — [`WorkerError::NoHandler`] —
/// stops it"; **W8 Task 3 made both readings false.** A `Pause` is
/// `models::TaskKind::Pause` and `executor::TaskKind::Pause` now, so
/// `persist`'s `type_id` is `None` for it and the claim's own
/// `WHERE type_id IS NOT NULL` never takes the run — the same tier a `Subflow`
/// left this population for at W6 Task 3, reached by the same line of code.
/// `advance()` drives it instead (`executor::flowable::pause`).
///
/// The history is kept rather than replaced with the current sentence because
/// the two refusals it distinguishes are still two different refusals, and that
/// distinction is what the next construct to arrive here will need.
/// `worker::timeout`'s module doc carries the same correction.
///
/// **The arm this function's `None` reaches** — to say it after a paragraph
/// about `Pause`, which reaches neither — is
/// [`WorkerError::NotALeaf`], which aborts the tick for that run exactly as an
/// unregistered type does, and [`WorkerError::Corrupt`] is left meaning what
/// its own doc says.
fn leaf<'a>(flow: &'a crate::models::Flow, task_id: &str) -> Option<Leaf<'a>> {
    fn walk<'a>(task: &'a crate::models::Task, want: &str) -> Option<Leaf<'a>> {
        if task.id == want {
            return match &task.kind {
                crate::models::TaskKind::Leaf { spec } => Some(Leaf {
                    spec,
                    // **Read from the `Task`, not from `spec`.** `timeout` is
                    // one of the seven keys `models::Task`'s deserializer takes
                    // OUT of a leaf's `spec`, which is exactly why a handler
                    // could not see it and `handlers::sql` used a constant.
                    timeout: task.timeout.as_ref(),
                }),
                _ => None,
            };
        }
        // `children()` AND `error_children()`: an error handler nested inside a
        // flowable's own `errors:` is a leaf a worker claims like any other,
        // and a walk that could not reach it aborted the tick with
        // `WorkerError::Corrupt` and left the run `RUNNING` for ever. See
        // `models::Task::error_children` for the three corpus entries that
        // showed it.
        task.children()
            .into_iter()
            .chain(task.error_children())
            .find_map(|c| walk(c, want))
    }
    flow.tasks
        .iter()
        .chain(flow.errors.iter())
        .find_map(|t| walk(t, task_id))
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
///
/// # `ORDER BY created_at` is not an order, and the tiebreak is load-bearing
///
/// One `settle` writes every run of one round in one transaction, and `now()`
/// in Postgres is TRANSACTION time, so sibling runs share `created_at` to the
/// microsecond. With nothing left to compare, the rows came back in whatever
/// order the scan produced — measured, `[0, 2, 4, 3, 1, 5]` for six siblings
/// whose physical order had been perturbed by an ordinary `UPDATE`.
///
/// W3b Task 2 recorded that as benign, and it was: `SKIP LOCKED` already makes
/// the choice arbitrary between two workers, and which of two ready runs goes
/// first is not a correctness property. **W3b Task 5 made it one.** The round a
/// run terminates in is its `ended_seq`, which is an endpoint of the
/// happens-before relation the differential oracle compares, so an arbitrary
/// claim order is an arbitrary relation. W3b Task 7 measured
/// `executor_matches_golden` failing about one run in eight under load, in both
/// directions.
///
/// `(execution_id, run_ix)` is `workflow.task_runs`'s own unique key, so
/// `created_at, execution_id, run_ix` is a **total** order and there is no
/// remaining tie for a scan to decide. `run_ix` is the executor's creation
/// order — `RunIx`, the index into `ExecutionState::runs` — which is the order
/// Kestra's own `taskRunList` carries and the one the oracle compares, so the
/// tiebreak is the executor's own ordering rather than an arbitrary column
/// chosen to break a tie.
///
/// **It has to be on BOTH queries, and the first attempt at this fix put it on
/// one.** The subquery's `ORDER BY` decides *which* rows are taken, which is
/// the whole answer at `LIMIT 1` — the oracle's driver ticks one run at a time.
/// The outer `SELECT` decides the order they come back in, which is the order
/// `tick` hands them to handlers when a caller asks for more than one. With the
/// tiebreak on the subquery alone the returned order was still
/// `[3, 0, 4, 2, 5, 1]`. Driven both ways, and at both batch sizes, by
/// `the_claim_takes_runs_in_run_ix_order_whatever_their_physical_order`.
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
               ORDER BY created_at, execution_id, run_ix \
               FOR UPDATE SKIP LOCKED \
               LIMIT $1 \
            ) \
            RETURNING id, execution_id, run_ix, task_ix, task_id, type_id, value, \
                      created_at, claimed_at, jsonb_array_length(attempts) AS attempt \
         ) \
         SELECT t.id, t.execution_id, e.workspace_id, t.run_ix, t.task_ix, t.task_id, \
                t.type_id, t.value, t.attempt, e.source, f.created_by AS principal_id, \
                t.claimed_at \
           FROM taken t \
           JOIN workflow.executions e ON e.id = t.execution_id \
           LEFT JOIN workflow.flows f ON f.id = e.flow_id \
          ORDER BY t.created_at, t.execution_id, t.run_ix",
    )
    .bind(limit)
    .fetch_all(exec)
    .await
    .map_err(db_err)
}

/// The executions holding a run whose **deadline has elapsed and which
/// something will move** — a `RETRYING` one waiting to be reopened, a `PAUSED`
/// one waiting to be resumed, or a `RETRIED` one whose **replay** has not been
/// started yet.
///
/// **The name is narrower than the predicate and is kept on purpose.** Since
/// W8 Task 7 two of the three arms are not retries; `due_retries` is what W4
/// Task 1 called it and what every doc that cites it — this crate's,
/// `oracle-exec/README.md`'s and W8's own record — still calls it, so renaming
/// it would cost a reader the trail for a word. The arms below are the
/// authority on what it selects; this sentence is, too.
///
/// **The state term is the whole predicate's point** and it is not a tidiness:
/// `advance()` never CLEARS a `not_before` — `executor::apply`'s
/// `ScheduleRetry` arm writes one and nothing writes `None` back — so a run
/// that has been retried and has since succeeded still carries the deadline
/// its retry wrote, for ever. Without it this statement would name every
/// finished execution whose retry deadline is in the past, on every
/// tick, and each would take the execution's row lock and a settle that writes
/// nothing but a new `executions.seq`. Driven by
/// `a_retry_whose_delay_has_elapsed_is_woken_and_one_still_waiting_is_left_alone`,
/// whose last assertion is exactly that: nothing is `RETRYING` any more, the
/// deadline is still there and still past, and the waker must name nobody.
///
/// # The `RETRIED` arm needs two more terms, for the same reason
///
/// W6 Task 4. A `CREATE_NEW_EXECUTION` retry leaves the run **`RETRIED`** —
/// terminal — with a deadline, and `reopen_due` will never touch it: what is
/// due is a whole new execution, which `worker::replay::spawn_due` starts from
/// the settle this waker's `resume` produces. `RETRIED` alone would be the
/// unbounded case the paragraph above describes, because nothing ever moves a
/// `RETRIED` run either — so the arm carries both conditions that decide
/// whether a replay is still to come, and each closes a different endless case:
///
/// - **no replay child yet** — otherwise every finished chain is named for
///   ever, once per tick, for a deadline that has already been acted on;
/// - **`e.parent_run_ix IS NULL`** — otherwise a `Loop` sub-execution whose
///   body carries such a retry is named for ever, because `spawn_due` refuses
///   that one permanently (the artifact cannot replay a loop iteration either;
///   see that function's doc) and no child will ever appear to stop it.
///
/// These are not one fence shared with `spawn_due`: this one bounds the
/// **waker's** work, and `spawn_due`'s decides the **spawn**, which is also
/// reached from a `resume` this predicate never saw — the oracle's driver takes
/// exactly that path. Under both sits migration 0138's
/// `workflow_executions_loop_run_idx` with `NULLS NOT DISTINCT`, which makes a
/// second replay unrepresentable rather than merely unasked-for.
/// `a_due_replay_is_woken_and_the_waker_stops_naming_it_once_it_exists` and
/// `a_replay_is_refused_inside_a_loop_sub_execution_and_taken_at_the_top_level`
/// drive the two.
///
/// **The clock is `load_with`'s, spelled the same way**, so an execution
/// named here is one `retry::reopen_due` will actually reopen — see the module
/// doc's `not_before` bullet.
///
/// # `ORDER BY` the shortfall, and the tiebreak for the same reason the claim has one
///
/// `not_before_ms` is measured from each execution's OWN `created_at`, so it
/// is not comparable across executions; the shortfall
/// `not_before_ms - now_ms` is, because both are the same instant minus the
/// same `now()`. It is the quantity `oracle-exec/driver.rs`'s `pass_time`
/// calls by that name, and ordering by it is "the retry that came due first
/// goes first". `execution_id` breaks the remaining tie, for
/// [`claim`]'s reason: without a total order the rows come back in whatever
/// the scan produced, and at `LIMIT 1` that decides *which* execution is
/// woken rather than merely in what order.
/// # The `PAUSED` arm, and the ratchet cannot score it either way
///
/// **W8 Task 2 predicted it, W8 Task 3 made it real, and W8 Task 7 is the one
/// that fixed it.** The gate below read `r.state = 'RETRYING' OR (r.state =
/// 'RETRIED' AND …)` from W4 Task 1 until W8 Task 7. A bounded `Pause` parks a
/// run with a `not_before_ms` and a state of `'PAUSED'`, which matched neither
/// — so in the **product** a bounded pause never resumed, however correct
/// `advance()` was. When that sentence was first written it was a prediction
/// about a construct nothing emitted; `flowable::pause` has emitted it since W8
/// Task 3, `flowable::loop_until` since Task 4, and eight golden entries depend
/// on the wait ending.
///
/// # What the `PAUSED` arm may name, and why it needs no terms of its own
///
/// `'PAUSED'` is added bare, where `'RETRIED'` needed two extra conditions, and
/// the asymmetry is structural rather than an oversight:
///
/// - **The unbounded pause is excluded by a CHECK, not by a term here.**
///   `Resume::Never` writes no deadline (`executor::apply`'s `PauseRun` arm
///   takes `not_before` out of the `Resume`), and migration 0141's
///   `workflow_task_runs_resume_bound_matches` makes `resume_with = 'NEVER'`
///   beside a non-NULL `not_before_ms` a row Postgres refuses. So the
///   pre-existing `r.not_before_ms IS NOT NULL` term above already cannot
///   reach one. **That is the over-acceptance this whole module must not
///   commit** — seven corpus flows write an unbounded `Pause` and the artifact
///   leaves every one of them `PAUSED` for the full harvest — and it is
///   refused in the schema rather than remembered in this `WHERE`. The
///   assertion at this end is the third arm of
///   `a_bounded_pause_whose_wait_has_elapsed_is_woken_and_an_unbounded_one_is_never_woken`.
///   **That citation named
///   `an_unbounded_pause_is_never_named_however_long_its_execution_lives`,
///   which does not exist** — the identifier occurred exactly once in the whole
///   tree, in the citation itself. It is the second dangling citation in this
///   package (W8 Task 4's `every_paused_run_names_its_resumer` was the first,
///   cited by `resume_paused`'s own doc before it was written), which is why
///   the round that fixed it also swept every test name cited in a doc comment
///   across the crate. **A citation is a claim and needs a positive control:
///   the control here is that the name kept below occurs 7 times in the tree,
///   not 1.**
/// - **A `PAUSED` run is never named twice for the same deadline** — but
///   **only because of the hoisted term below, and the first version of this
///   bullet was wrong about why.** It said `flowable::resume_paused` moves a due
///   run on the very settle this waker's `resume` produces, so the run leaves
///   `PAUSED` and the stale deadline lands in a state this predicate does not
///   name. That holds *unless `advance()` returns on its first line*, which it
///   does when the **execution** is terminal. See the section below.
///
/// # The execution-terminality term is ONE term over every arm, and the
/// # exemption is measured
///
/// ```sql
/// AND (r.state = ANY($2) OR NOT (e.state = ANY($2)))
/// ```
///
/// `$2` is the terminal vocabulary, **derived from `RunState::ALL` by
/// `is_terminal()`** rather than written into the SQL —
/// `spawn::terminated_count`'s shape, and the only one a fourteenth terminal
/// variant joins without anybody remembering to.
///
/// **Why it exists.** [`advance`](crate::executor::advance) returns
/// `Vec::new()` on its first line when the execution is terminal, and
/// `RunState::Paused` and `RunState::Retrying` are both **non**-terminal — so
/// `claim::sla_terminal` can `CANCEL` an execution that is still holding a
/// parked bounded `Pause` or a waiting retry. That run keeps its state and its
/// `not_before_ms`; every tick names it; every tick relocks the execution and
/// settles it into writing nothing but a new `executions.seq`. **It is the
/// endless case the `RETRIED` arm needed two extra terms to avoid, reached by a
/// third road.**
///
/// **Why it is ONE term and not one per arm.** W8 Task 7 shipped it on the
/// `'PAUSED'` arm alone and recorded the `'RETRYING'` twin as a finding; *a
/// recorded divergence is not a closed one*, and fixing one member of a family
/// while its sibling stands is this branch's named failure mode in its
/// structural form. Hoisted, it closes both at once and **is unrepresentable
/// rather than remembered for whatever arm comes next**.
///
/// **Why the `r.state = ANY($2)` half is not a hole.** It exempts an arm whose
/// RUN state is itself terminal, and today that is exactly one: `'RETRIED'`.
/// The exemption is **load-bearing and measured, not defensive** — a
/// `CREATE_NEW_EXECUTION` retry ends **this execution** `RETRIED`, so the whole
/// population of that arm lives in terminal executions by construction. The
/// golden says so directly: every `valids/retry-new-execution-*` link reads
/// execution `["CREATED","RUNNING","FAILED","RETRIED"]` beside run
/// `["CREATED","SUBMITTED","RUNNING","FAILED","RETRIED"]`. Dropping the
/// exemption — the naive hoist — is **mutation MX6**.
///
/// **I predicted MX6 would take the ratchet from 109 to 113 and name those four
/// entries. It does not: the ratchet reads 109 under MX6, three runs out of
/// three.** The prediction was a plausible inference written in the voice of a
/// measurement, and the reason it is wrong is *this function's own subject*:
/// the oracle reaches a due replay through `driver::pass_time` → `resume` →
/// `settle` → `replay::spawn_due`, and **never through this query**. A change
/// here cannot move the ratchet for the replay entries any more than it can for
/// the pause entries — the same blindness, one arm further along.
///
/// What MX6 **does** kill, 3 of 3 runs, is
/// `a_due_replay_is_woken_and_the_waker_stops_naming_it_once_it_exists` and
/// `a_replay_is_refused_inside_a_loop_sub_execution_and_taken_at_the_top_level`
/// — **both W6 Task 4's, both written long before this term existed**, so
/// unlike the guard tests of this round they are assertions that **predate what
/// they prove**. That is the strongest available evidence the exemption is
/// load-bearing rather than defensive, and it is stronger than the ratchet
/// figure I had claimed.
///
/// **And the exemption is a proxy for the real rule, soundly.** The guard is
/// about consumers *inside* `advance()`: `'RETRYING'` is consumed by
/// `retry::reopen_due` and `'PAUSED'` by `flowable::resume_paused`, both inside
/// it, while `'RETRIED'` is consumed by `worker::replay::spawn_due`, which runs
/// in the settle **outside** `advance()` and is therefore unaffected by its
/// early return. Keying on *"the run's own state is terminal"* implies that:
/// `advance()` moves no terminal run, so an arm whose run state is terminal
/// **must** have its consumer outside `advance()`. The proxy cannot drift from
/// the rule it stands for.
///
/// `a_terminal_execution_hides_every_arm_except_the_replay` is the 3×2 table —
/// `RETRYING`/`PAUSED`/`RETRIED` × terminal/non-terminal execution — and
/// `a_paused_run_in_a_terminal_execution_is_never_named` drives the pause half
/// end to end.
///
/// # `RESTARTED` is NOT here, and that was measured rather than assumed
///
/// It is the other non-terminal state W8 Task 2 added, so the question has to
/// be answered rather than skipped. Nothing can put a run in it carrying a
/// live deadline: `RunState::Restarted` is pushed by `pause_mark` onto the
/// **execution**'s history and by nothing onto a run's — `0` of the golden's
/// 840 run histories hold it, against 12 execution histories that do — and the
/// one command that ends a `LoopUntil`'s wait, `Command::RestartLoop`, clears
/// `not_before` and `resume_with` in the same arm and pushes `RUNNING`. A
/// state with no producer of due rows is not a gap in this gate.
/// `the_waker_names_exactly_the_states_something_will_move` is the row that
/// says so, and it enumerates `RunState::ALL` so a fourteenth variant cannot be
/// answered by omission.
///
/// # Neither the fix nor the defect is visible to the ratchet
///
/// **The oracle is blind here for a reason that is a property of the
/// instrument.** `oracle-exec/driver.rs`'s `pass_time` selects on
/// `not_before_ms` alone, with no state term anywhere in its SQL, and then
/// calls `worker::resume` directly. It never runs this query. So the driver
/// reaches the same runs through a different door: the eight pause entries
/// banked green with this gate narrow, and they stay green now it is wide.
/// That is the shape W4's item 15 named — a property whose only instrument is an
/// integration test, because the oracle reaches the same runs through a door
/// that cannot fail — and it is why this fix ships with one and no baseline
/// movement.
///
/// **The gloss item 15 used for that shape, "a path no deployment takes", is no
/// longer true of this query.** Since W10 Task 2 `egret-bin` runs the claim loop
/// ([`crate::runtime::spawn`]), so `runtime` → [`tick`] → [`wake`] → this
/// statement is a production path. The ratchet's blindness to it is unchanged;
/// what changed is that it is now blindness about **shipping** behaviour rather
/// than about a driver's, which makes the integration test more load-bearing
/// than it was, not less.
///
/// `executor::retry::reopen_due` is the pure-side twin and its blindness is the
/// **opposite**: the driver does reach it, so a narrowing there fails loudly.
/// Recorded on both because one sentence about "the ratchet cannot see it" is
/// wrong about one of them.
///
/// # Exported for the same reason [`wake`] is
///
/// `pub` so a test can measure the **selection** rather than the settle. The
/// vocabulary row above hand-writes a `task_runs.state` for each member of
/// `RunState::ALL`; going through [`wake`] would resume each one and
/// `claim::persist` recomputes `state` from `history.last()`, so the fixture
/// would erase the very column under test before the next read.
pub async fn due_retries(exec: impl sqlx::PgExecutor<'_>, limit: i64) -> Result<Vec<Uuid>> {
    sqlx::query_scalar(
        "SELECT r.execution_id \
           FROM workflow.task_runs r \
           JOIN workflow.executions e ON e.id = r.execution_id \
          WHERE (r.state = 'RETRYING' \
                 OR r.state = 'PAUSED' \
                 OR (r.state = 'RETRIED' \
                     AND e.parent_run_ix IS NULL \
                     AND NOT EXISTS (SELECT 1 FROM workflow.executions c \
                                      WHERE c.parent_execution_id = e.id \
                                        AND c.parent_run_ix IS NULL))) \
            AND (r.state = ANY($2) OR NOT (e.state = ANY($2))) \
            AND r.not_before_ms IS NOT NULL \
            AND r.not_before_ms <= \
                (EXTRACT(EPOCH FROM (now() - e.created_at)) * 1000)::bigint \
          GROUP BY r.execution_id \
          ORDER BY MIN(r.not_before_ms \
                       - (EXTRACT(EPOCH FROM (now() - e.created_at)) * 1000)::bigint), \
                   r.execution_id \
          LIMIT $1",
    )
    .bind(limit)
    // **Derived from `RunState::ALL`, not listed** -- `spawn::terminated_count`'s
    // shape, and the only one that survives a fourteenth variant. A hand-written
    // `('SUCCESS','FAILED',...)` in the SQL above would be a second copy of
    // `is_terminal` that no compiler and no test compares to the first.
    .bind(
        RunState::ALL
            .iter()
            .filter(|s| s.is_terminal())
            .map(|s| s.wire())
            .collect::<Vec<_>>(),
    )
    .fetch_all(exec)
    .await
    .map_err(db_err)
}

/// Reopen every retry whose delay has elapsed, up to `limit` executions.
///
/// Returns the executions it resumed, **by id and in the order it took them**,
/// rather than a count: an entry that moved for an unattributed reason is a
/// coincidence, and a caller — a test included — that can only see a number
/// cannot tell which execution moved. [`reap`](super::reap) returns a count
/// because a reclaimed lease is fungible; a woken execution is not.
///
/// Public and callable on its own, for [`reap`](super::reap)'s reason: a real
/// worker pool may want to wake on its own cadence, and a waker reachable only
/// from inside [`tick`] would be one nothing could drive directly. [`tick`]
/// calls it, which is what gives it a caller **inside this crate's own
/// source** — the thing `resume` did not have, its only caller being
/// `oracle-exec/driver.rs`, and the whole reason this hole survived a package.
///
/// **That sentence said "a production caller"; W4 Task 6 measured that it was
/// not one; W10 Task 2 made it one.** Until then [`tick`] had no caller in any
/// `crates/*/src` — a grep for `tick(`, `reap(` and `start_execution(` found no
/// call site at all, and `egret-bin` built a `WorkflowState` (the flow repo and
/// its refs) with no claim loop — so the chain this doc describes ended one link
/// short of a deployment. **It no longer does.**
/// [`crate::runtime::spawn`] is the claim loop, `egret-bin` starts it beside the
/// automation ticker under `[workflow] worker_enabled`, and the waker therefore
/// runs in the product: `runtime` → [`tick`] → [`wake`] → [`resume`].
///
/// The history is kept rather than deleted, because it is the reason this doc
/// is worded as carefully as it is: this was exactly the sentence a reader
/// would quote as evidence the waker was wired, back when it was wired only to
/// [`tick`] and nothing was wired to [`tick`]. The gap survived a whole package
/// behind it.
///
/// `oracle-exec/README.md`'s coverage statement carries the same proposition as
/// its item 15, and W4's own section as its sixth fact. **W10 Task 7 owns that
/// file's sweep.** No count of its instances is given here on purpose: a
/// tally of another file's current state is a sentence that goes false the
/// moment that file is corrected, which is the failure this very paragraph
/// exists to undo.
///
/// **Every named execution is attempted and the first error is returned after
/// the loop**, which is [`tick`]'s own rule for the same reason: one
/// execution's failure to settle must not leave the executions behind it
/// asleep for a reason that concerned a different execution.
pub async fn wake(pool: &PgPool, limit: i64) -> Result<Vec<Uuid>> {
    let due = due_retries(pool, limit).await?;
    let mut woken = Vec::with_capacity(due.len());
    let mut first_err: Option<Error> = None;
    for execution in due {
        match resume(pool, execution).await {
            Ok(()) => woken.push(execution),
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(woken),
    }
}

/// # READ THIS BEFORE WIRING `tick` INTO A BINARY
///
/// Nothing in this workspace calls `tick` in production yet. Whoever first does
/// makes two things reachable that are only latent today, and both are recorded
/// here because this is the function they will be looking at — not at the
/// handler docs where they were first written.
///
/// **1. One unbounded handler can get a later run in the same batch reaped
/// before its handler is invoked, and then run twice.** [`claim`] leases up to
/// `n` rows in a **single** `UPDATE … SET claimed_at = now() … LIMIT $1`, and
/// the loop below is **sequential**, so every lease in a batch is taken at one
/// instant and run *k*'s is already held for the sum of handlers 1..*k*−1.
/// `handlers::sql` bounds itself at 300s when the author writes no `timeout:`;
/// `handlers::container` and `handlers::python` bound themselves at **nothing**,
/// so one container can hold the last lease past
/// [`DEFAULT_WORKING_TIMEOUT`](super::DEFAULT_WORKING_TIMEOUT) before that run
/// starts. Twelve bounded `egret.sql` runs reach the same threshold, so this is
/// **this function's** property and not a handler's. The options are: lease one
/// run at a time; refresh each lease immediately before its handler; or measure
/// the reaper's timeout per run. See W4 Task 5's report and cost 2 above.
///
/// **W5a Task 3 narrows this and does not close it.** [`under_lease`] renews
/// each run's lease *before* its handler is polled, so a run reaped while it
/// waited is found lost and **not run** — no second handler beside the worker
/// that reclaimed it. What is still true: the waiting run's lease is not
/// renewed while earlier members of the batch run, so it can still be reaped
/// and handed to another worker in that window.
///
/// **2. Pick `n` deliberately.** `n = 1` makes the property above unreachable
/// and is what `oracle-exec/driver.rs` uses.
///
/// # Two ways a run HANGS FOR EVER, both shipped and neither bounded
///
/// Recorded by W10 Task 5 fix round 2 at the whole-branch review's request, and
/// **deliberately not fixed there** — they are liveness, not correctness, and
/// the fix for either changes this function's contract.
///
/// **Correctness is fenced; liveness is not.** Everything below is safe in the
/// sense that no wrong value is ever written: `apply_outcome`'s
/// `claimed_at = $3` fence rejects the stale worker every time. What no fence
/// does is make the run finish, or make anything *notice* that it has not.
///
/// **3. A publishable type with no handler is claimed and never completed.**
/// `Handlers::with_core_tasks` registers **11** handlers (`log`, `ret`, `fail`,
/// `sleep` and seven core storage tasks) and `runtime::handlers_for` **2**
/// more (`storage::Delete`, `PurgeCurrentExecutionFiles`);
/// `spec::registry::TASK_TYPES` holds **23** rows. With
/// `workflow.container_runner` set, `handlers_for` adds the **4**
/// script rows (W5a); with `none` it does not, and `publish` refuses those four
/// instead. Either way **6 of 23 publishable types** (`egret.sql`,
/// `egret.dataapi.call`, `egret.dataset.refresh`,
/// `egret.report.send`, `core.http`, `core.slack`) produce a run that
/// `claim` leases, finds no handler for, and leaves `RUNNING` -- as does a
/// script task reached without publish (a draft run) under `none`. *(This
/// read "11 + 2 handlers ... 24 rows ... 7 of 24" until the W9a rebase onto
/// Data Apps P6a (#629), which removed `egret.python`; "4 handlers ... 15
/// rows ... 7 of 15" until W9a Task 12 fix round 1,
/// whose W9a Tasks 11-12 added nine core storage rows and handlers; and
/// "13 rows ... 9 of 13" until W5a Task 6: W5a folded
/// `egret.container`/`egret.script.python` into aliases and added the four
/// script FQCNs; it read "15 rows ... 7 of 15" until Data Apps P6 removed
/// `egret.python`, the notebook-kernel task.)* [`reap`] then
/// clears `claimed_at` and **nothing else** — its statement is
/// `SET claimed_at = NULL, updated_at = now()`, so the state stays `RUNNING` and
/// the attempt stays open — and `claim`'s own predicate is
/// `state = 'RUNNING' AND claimed_at IS NULL`, which that row now satisfies
/// again. It is re-claimed, re-missed, re-reaped, for ever.
///
/// **Nothing bounds it and nothing counts it.** `attempts.push` occurs in
/// exactly **one file** (`executor/state.rs`, 2 sites) and neither is on this
/// path, so **no counter moves**: the run's `attempts` array is the same length
/// on the ten-thousandth cycle as on the first. The only signal in existence is
/// one `warn` per cycle. A fix needs somewhere to put "this has been tried and
/// there is no handler" — which is a schema question, not a patch here.
///
/// **4. A handler that OUTRUNS `working_timeout` does the same thing, and this
/// cause is not the same as 3.** [`DEFAULT_WORKING_TIMEOUT`](super::DEFAULT_WORKING_TIMEOUT)
/// is 3600 s. `Sleep::new()` sets `cap: None` — uncapped — and it is one of the
/// **four registered** handlers, so this is reachable with the shipped registry
/// and a flow anyone may publish. `duration: PT2H`: the run is claimed, sleeps,
/// is reaped at 1 h, is **re-claimed**, starts a *new* two-hour sleep, and when
/// the first handler finally returns it hits the fence with `LeaseLost`. The
/// second is reaped at its own 1 h mark, and so on. The run never completes and
/// each cycle costs a fresh handler.
///
/// **W5a Task 3 closed cause 4.** The worker renews its lease every
/// `working_timeout / 3` while the handler runs ([`under_lease`]), so a live
/// handler's lease never ages past the reaper's threshold; only a worker that
/// stopped renewing (died) has its run reclaimed.
/// `tests/runtime.rs::a_run_longer_than_working_timeout_is_not_reaped_while_its_worker_lives`
/// and `tests/worker.rs::a_run_whose_worker_stopped_renewing_is_reaped` pin
/// the two directions. Cause 3 is untouched.
///
/// The distinction matters for whoever fixes it: cause 3 is a **missing**
/// handler and cause 4 is a **slow** one, so a "give up after N tries" rule
/// would close 3 and quietly convert 4 into a wrongly-failed run. `Sleep` has a
/// bounded constructor already — `Sleep::capped(cap)` — which is the shape of
/// the answer for 4 and does nothing for 3.
///
/// # An `Err` from one run does NOT abandon the others
///
/// Stated because the phrase *"aborts the tick"* is used loosely throughout this
/// crate and overstates it (the whole-branch re-review's Minor 3). What actually
/// happens is in the loop below: the error is kept in `first_err`, **the loop
/// continues**, every other claimed run is still handled, and the first error is
/// returned at the end. What the failing run loses is its **own** outcome — no
/// result is applied, so it stays `RUNNING` and leased until [`reap`] returns
/// it, which is the consequence every finding about this has correctly named.
///
/// [`reap`]: super::reap
///
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
/// # One run's failure does not strand the rest of the batch
///
/// The claim has **already committed** `n` leases by the time the first handler
/// starts, so an early return would leave the runs after it leased and untouched
/// until [`reap`](super::reap) takes them back — one worker timeout of nothing
/// happening, for a reason that concerned a different run. The first draft did
/// exactly that with `?`, and it was invisible because the differential oracle's
/// driver runs at `BATCH == 1`, where there is never a rest of the batch.
///
/// So every claimed run is attempted, and the **first** error is returned after
/// the loop. Returning an error at all is the contract
/// `a_type_with_no_handler_aborts_the_tick` pins: a run this worker could not
/// finish must not be reported as finished. The returned count stays "runs whose
/// result committed", so a partial batch reports what it actually did.
///
/// # The waker runs first, and inside the same call
///
/// A run in `RETRYING` is not claimable — it has to re-enter `CREATED` first,
/// and only `advance()` may put it there. [`wake`] is what re-reads the clock
/// for it, and it is called **ahead of** the claim rather than beside it so
/// that a run this call reopens is a run this call also claims: `resume` drives
/// the reopened run to `RUNNING` inside its own transaction, which is exactly
/// the claim's predicate. A waker after the claim would cost one whole tick of
/// latency per retry for no reason.
///
/// It is `n` executions rather than an unbounded sweep, for the claim's
/// reason: a tick is the unit of work a caller asked for.
///
/// # One result per TRANSACTION — W4 Task 4's ruling, and what it costs
///
/// The loop above hands one result at a time to [`apply_outcome`], which opens
/// its own transaction, applies that one terminal, drives `advance()` to a
/// fixed point and commits. W3b left the question of whether it should stay
/// that way to W4, on the grounds that W4 introduces a real worker pool. **It
/// stays.**
///
/// **"One run per tick" is the wrong name for the discipline and this loop is
/// the proof**: the tick claims `n` and finishes `n`. The discipline is one
/// result per transaction, and `tests/worker.rs`'s
/// `one_result_per_transaction_is_the_ruling_and_this_is_the_step_it_costs`
/// finishes two runs in one tick while asserting it. The two halves are
/// independently measurable and were measured separately:
///
/// | configuration | `executor_matches_golden` |
/// |---|---|
/// | shipping (`driver::BATCH = 1`, one result per transaction) | 143 mismatching |
/// | `BATCH = 32`, one result per transaction | 143 mismatching, `executor.rs` 34/34 (its size at W4 Task 4; **35** since Task 6 added a derivation test — the claim in this row is the 143) |
/// | `BATCH = 1`, a whole batch per transaction | 143 mismatching (**forced**: at `BATCH = 1` a group is one run, so the widened apply IS [`apply_outcome`]) |
/// | `BATCH = 32`, a whole batch per transaction | **140** — banks `valids/dag`, `valids/full`, `valids/parallel-disabled-tasks`; no regression |
///
/// So the batch size buys nothing on its own — the claim orders by
/// `created_at, execution_id, run_ix`, which makes a batch of `n`
/// indistinguishable from `n` successive claims of 1 — and the entries are
/// bought by applying several results **before** calling `advance()`.
///
/// **It was refused for what it costs, not for what it banks**, which is the
/// rule W3b applied when reversing the claim's tiebreak also banked
/// `valids/dag`: choosing the shape of the runner by what it banks is how an
/// oracle gets tuned to its own answer. The costs, in the order of how much
/// weight the ruling puts on them, every one measured on a prototype whose raw
/// output is `oracle-exec/w4t4-widened-tick.txt` — the widened code is not in
/// this checkout, so the numbers below are not otherwise reproducible from it.
///
/// 1. **THE ONE THE RULING RESTS ON: two guards would stop being guards, and
///    [`persist`]'s four-mutation table would start certifying a shape the code
///    no longer has.** That table proves the `outputs` fence and the state
///    fence are ALTERNATIVES, each sufficient alone — and it was taken against
///    a transaction that releases exactly ONE lease. Widened, it releases `n`.
///    Measured (`w4t4-widened-tick.txt`, RUN 4): put a defect in the surviving
///    alternative that can only bite at `n > 1` — delete the `outputs` fence
///    outright, which the table says is survivable, and make `persist` fence
///    only the group's FIRST lease, which is byte-equivalent at `n == 1` — and
///    **the whole crate is green** but for this ruling's own ordering guard.
///    So `a_worker_whose_lease_was_reaped_cannot_commit` and
///    `a_tick_killed_between_the_handler_and_the_commit_applies_nothing` pass
///    with both fences effectively gone for every non-first member of a group,
///    because both drive [`apply_outcome`] with a single `Claimed`. A programme
///    whose first law is *a guard nobody has seen fail is a guard nobody has
///    seen* cannot buy an ordering nicety with two guards that stop being
///    guards.
/// 2. **`working_timeout_s` already does not mean what it says, and this cost
///    was WRONG about the shipping tick.** The sentence here used to read
///    *"Today a run's lease is held for `input_for` plus its own handler.
///    Batched, it is held for the sum over the batch"*, attributing to the
///    widening a property the tick **already has**. The whole-branch review
///    caught it and it is right: [`claim`] leases up to `n` rows in **one**
///    `UPDATE … SET claimed_at = now() … LIMIT $1`, and [`tick`] then walks
///    them with a sequential `for c in &claimed { run_one(…).await }`. So run
///    *k*'s lease is taken at the same instant as run 1's and is already held
///    for the sum of handlers 1..*k*−1 before its own handler is invoked.
///    Widening changes the transaction, not this.
///
///    **What the widening would still cost is a lost lease costing the group**
///    (cost 4), not this — so this row is now an argument for neither side, and
///    is kept because it is a real property of the tick that the operator who
///    sets `working_timeout_s` cannot see.
///
///    **And W4 Task 5 made it bite.** `handlers::sql` bounds itself with
///    `DEFAULT_TIMEOUT` (300s) when the author writes none;
///    `handlers::container` and `handlers::python` have **no default budget at
///    all**, so one `egret.container` with no `timeout:` can hold its own lease
///    — and every later lease in its batch — for longer than
///    [`DEFAULT_WORKING_TIMEOUT`](super::DEFAULT_WORKING_TIMEOUT). The last
///    member of a batch can then be **reaped before its handler is invoked**
///    and run twice. Nothing bounds this today; see
///    [`handlers::container`](super::handlers::container)'s module doc for the
///    proposal and why it was not taken in a bookkeeping round. *(W5a Task 3:
///    script and container tasks with no `timeout:` now run under
///    `container_default_timeout`, and the running run's lease is renewed;
///    the pre-invocation window of cause 1 above remains.)*
/// 3. **It is the coupling this doc's own *One run's failure does not strand
///    the rest of the batch* already refused once** — this time inside the
///    transaction, where an early return cannot be softened into "carry on".
/// 4. **A lost lease costs the whole group rather than the run it named — and
///    this is the WEAKEST of the four, which the first version of this doc got
///    wrong.** A batch is one transaction, so one [`WorkerError::LeaseLost`]
///    aborts every result in it. The first measurement of that used a single
///    adversary — a registry reaping from the SECOND handler, which is the
///    arrangement most favourable to this tick — and reported a live-lock as
///    the widening's. It is not. Five configurations, one scratch database
///    each, six rounds, handler calls / SUCCESS runs
///    (`w4t4-widened-tick.txt`, RUN 3):
///
///    | reap | narrow (shipping) | widened |
///    |---|---|---|
///    | none (control) | 2 / 3 | 2 / 3 |
///    | 2nd handler, every call | 7 / 1 | 12 / 0 |
///    | 2nd handler, once | 3 / 3 | 4 / 3 |
///    | **1st handler, every call** | **12 / 0** | **12 / 0** |
///    | 1st handler, once | 4 / 3 | 4 / 3 |
///
///    The shipping tick live-locks too, with the identical call log, when the
///    reap fires from the first handler. An adversary that reaps on every call
///    starves both. What widening actually costs is the **asymmetry**: this
///    tick survives one of the two placements, the widened one survives
///    neither. Under a transient reap — the realistic case, since [`super::reap`]
///    fires on a lease that has actually expired — both recover fully and the
///    widening costs exactly ONE extra handler re-run.
///
///
/// **A fifth thing the widening changes, derived from the code shape rather
/// than measured, and a BEHAVIOUR change rather than a durability one.** This
/// loop calls [`input_for`], the handler and the commit per run in that order,
/// so the second claimed run's render surface is loaded *after* the first has
/// committed and a sibling writing `{{ outputs.<earlier sibling> }}` resolves
/// it. A widened tick runs every handler before any commit, so no member of a
/// batch can see another member's outputs. Neither is Kestra's guarantee — a
/// `Parallel` sibling reading a sibling's outputs is a race there too — but the
/// widening silently NARROWS what a handler can see, and nothing in this crate
/// or in the corpus would notice.
///
/// **The way back in, named rather than left implicit.** A design that applies
/// a batch's results together **without sharing a failure** — a `SAVEPOINT` per
/// run inside the group transaction, so a lost lease rolls back one run's
/// writes and not the group's — removes cost 4 entirely and shrinks cost 3.
/// Costs 1 and 2 remain, and cost 1 is the binding one: such a design has to
/// bring `n`-lease versions of both fence tests and re-derive [`persist`]'s
/// mutation table against the `n`-row shape. The three entries stay available
/// at that price; this ruling does not spend them.
///
/// **W3a's deleted guard did NOT come back, and that was measured rather than
/// predicted.** W3a removed `resolveParallelNexts`'s "return nothing while any
/// child is still `CREATED`" and named the trigger that would restore it as *"a
/// caller that applies a batch partially — W4's worker"*, on the same three
/// entries. [`CREATE_BESIDE_A_CREATED_SIBLING`] is **0** under the widened
/// prototype exactly as it is today: the guard's state needs a worker result
/// delivered *between two rounds of one `advance()` cascade*, and [`drive`]
/// runs that cascade to a fixed point inside one transaction under either
/// ruling. The overlap of entries is a coincidence of which flows have
/// observable sibling order, not a shared mechanism.
///
/// Returns the number of runs whose result committed. **A tick that only woke
/// something returns 0**, and that is deliberate — the count means "results
/// committed", not "work done", and a caller that looped until zero would
/// otherwise never stop on an execution whose retry it had just reopened. The
/// run it reopened is claimed by this same call, so the count moves on the
/// tick that runs it.
pub async fn tick(pool: &PgPool, n: usize, reg: &dyn Registry) -> Result<usize> {
    tick_with(pool, n, reg, WorkerTiming::default()).await
}

/// The two durations a tick needs and a bare `tick(pool, n, reg)` cannot
/// carry: how long a lease lives, and the budget of a script task with no
/// `timeout:`.
///
/// A struct rather than two more positional `Duration`s on [`tick_with`],
/// because two adjacent `Duration` parameters are the pair a caller swaps
/// without the compiler noticing. [`Default`] is the product default
/// ([`DEFAULT_WORKING_TIMEOUT`](super::DEFAULT_WORKING_TIMEOUT),
/// [`DEFAULT_CONTAINER_TIMEOUT`](super::DEFAULT_CONTAINER_TIMEOUT)), which is
/// what [`tick`] uses; the runtime passes `RuntimeConfig`'s values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerTiming {
    /// The reaper's `working_timeout`: a lease older than this is reclaimed.
    /// The worker renews its lease every third of it while a handler runs.
    pub working_timeout: std::time::Duration,
    /// The budget a script or container task gets when it declares no
    /// `timeout:`. See [`DEFAULT_CONTAINER_TIMEOUT`](super::DEFAULT_CONTAINER_TIMEOUT).
    pub container_default_timeout: std::time::Duration,
}

impl WorkerTiming {
    /// Every third of `working_timeout` (W5a spec §6), so two renewals can be
    /// lost before the reaper takes the run. Floored at 1 ms because
    /// `tokio::time::interval` panics on zero.
    pub fn renew_every(&self) -> std::time::Duration {
        (self.working_timeout / 3).max(std::time::Duration::from_millis(1))
    }
}

impl Default for WorkerTiming {
    fn default() -> Self {
        Self {
            working_timeout: super::DEFAULT_WORKING_TIMEOUT,
            container_default_timeout: super::DEFAULT_CONTAINER_TIMEOUT,
        }
    }
}

/// [`tick`] under an explicit [`WorkerTiming`] — what the runtime calls, with
/// its configured `working_timeout` and `container_default_timeout`.
pub async fn tick_with(
    pool: &PgPool,
    n: usize,
    reg: &dyn Registry,
    timing: WorkerTiming,
) -> Result<usize> {
    let mut first_err: Option<Error> = None;
    // Ahead of the claim, and its failure does not stop the claim: an
    // execution that cannot settle must not keep this worker from running the
    // runs that are already claimable.
    if let Err(e) = wake(pool, n as i64).await {
        first_err = Some(e);
    }
    // Read BEFORE the claim statement is sent, so the self-fence's clock
    // (`under_lease`) can only start earlier than the `claimed_at` it wrote.
    let leased_at = tokio::time::Instant::now();
    let claimed = claim(pool, n as i64).await?;
    let mut done = 0usize;
    for c in &claimed {
        match run_one(pool, c, reg, timing, leased_at).await {
            Ok(Committed::Yes) => done += 1,
            // The fence moved: the run is another worker's and this one
            // committed nothing, so it is not counted.
            Ok(Committed::LeaseLost) => {}
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(done),
    }
}

/// One claimed run: its handler, the budget the handler runs under, and the
/// transaction its result lands in.
///
/// # The budget is the WORKER's, and it is here rather than in a handler
///
/// Measured on the pinned artifact, twice (see [`super::timeout`] for both
/// readings): Kestra enforces `timeout:` in
/// `AbstractWorkerCallable.callWithTimeout`, around the whole of the task's
/// `run`, and a task that outruns it is **FAILED** carrying
/// `Timeout after HH:mm:ss.SSS`. Putting it here rather than inside
/// [`Registry::run`] is what makes it true of *every* registry — the
/// production [`Handlers`](super::Handlers), the oracle driver's, and a test's
/// — instead of only the one that remembered.
///
/// A `timeout:` this run cannot use fails the run **without calling a
/// handler**, which is also the artifact's answer: all three bad spellings
/// produce a FAILED run whose attempt never ran the task body.
///
/// # The attempt's log is finished here, on every path (W11)
///
/// The handler's result is captured as a `Result` rather than `?`-ed, so the
/// `Err` path reaches the same two calls as every other before it returns:
///
/// 1. **the failed-attempt line** (spec §4.4) — if the outcome is `Failed`
///    with a message, that message at ERROR, outside the cap. That is Kestra's
///    `exceptionHandler`, which logs `e.getMessage()` for every task that
///    throws unless it was killed. One rule covers a handler's own
///    `Ok(Failed)`, a refused budget and an expired one (`Timeout after …`),
///    because all three arrive here as the same `Outcome`. A handler `Err` is
///    egret's outage and not the task's, and records no outcome, so it writes
///    no line; a dropped future cannot write one, as Kestra's killed callable
///    does not;
/// 2. **`finish()`** — the cap summary, if anything was withheld.
///
/// Both come after the handler's own lines and before `pool.begin()`, so no
/// log write happens inside the transaction.
///
/// **The fence-lost path (W5a) runs `finish()` and NOT the failed-attempt
/// line.** A worker whose renewal finds the fence moved records no outcome,
/// and a failed-attempt line is the log's half of an outcome; writing one
/// would put `FAILED`'s message beside a run another worker may yet finish.
///
/// # The lease is renewed while the handler runs (W5a)
///
/// See [`under_lease`]: every `working_timeout / 3`, fenced, starting before
/// the handler is polled. The outcome is then fenced on the **latest**
/// renewed lease, which is why a renewed copy of `c` reaches
/// [`apply_outcome`] rather than `c` itself. The handler never sees the lease:
/// what it holds is named by `(run, attempt)` (see [`TaskHandler::cancel`]'s
/// *READ THIS* section), which a renewal does not change.
///
/// # What the run held is released AFTER the commit (W5a final review I3)
///
/// [`Registry::release`] is called only once the outcome's transaction has
/// committed — never on a lost lease, never on a handler `Err`, never on a
/// failed commit. A script task's pod or container is deleted there, not in
/// the handler, so:
///
/// * **the commit failed** (Postgres blip, failover): the run stays RUNNING,
///   its lease expires, and the next claim **re-attaches** to the finished
///   pod and collects the same outcome — the task is not run a second time;
/// * **egret died between the commit and the release**: the run is terminal
///   and its pod is deleted by the orphan sweep on the reaper's tick.
///
/// A release that fails is logged; the committed outcome stands.
///
/// [`TaskHandler::cancel`]: super::TaskHandler::cancel
async fn run_one(
    pool: &PgPool,
    c: &Claimed,
    reg: &dyn Registry,
    timing: WorkerTiming,
    leased_at: tokio::time::Instant,
) -> Result<Committed> {
    let (input, budget) = input_for(pool, c, reg, timing.container_default_timeout).await?;
    let handler = async {
        match budget {
            Budget::Refused(message) => Ok(Outcome::failed_with(message)),
            // `cancel::guarded` rather than `timeout::guard` directly: the
            // budget's expiry is the artifact's only `kill(null)` site, so the
            // kill path hangs off it. A refusal there does not reach this
            // expression — it is logged and the budget's outcome stands. See
            // `super::cancel`.
            other => cancel::guarded(reg, &input, other.duration()).await,
        }
    };
    let (outcome, held, renewed_at) = match under_lease(
        pool,
        c,
        timing.renew_every(),
        timing.working_timeout,
        leased_at,
        handler,
    )
    .await
    {
        Leased::Kept {
            out,
            held,
            renewed_at,
        } => (out, held, renewed_at),
        // **The fence moved: another worker holds this run now, and this one
        // records NOTHING** — no outcome, and no failed-attempt line, because
        // there is no failure to report: the task was not finished by anyone
        // yet. Lines the handler already wrote stay (they were true when
        // written), and `finish()` still runs so a withheld-lines summary is
        // not lost. The handler future was dropped inside `under_lease`.
        Leased::Lost => {
            tracing::warn!(
                execution = %c.execution_id,
                run = %c.id,
                task_id = %c.task_id,
                "this worker no longer holds the run (the fence moved, or no renewal \
                 succeeded for working_timeout); it stopped following the task and \
                 records no outcome"
            );
            input.log.finish().await;
            return Ok(Committed::LeaseLost);
        }
    };
    // Spec §11 (fix round 1, I2): a read this handler's render made met a
    // store OUTAGE. The outcome it produced (a failed render, as a rule) is not
    // the task's: return `Err` and record nothing, as for a handler `Err` --
    // the lease lapses, the run is re-claimed, no attempt is spent. No
    // failed-attempt line either, for the same reason.
    if let Some(outage) = input
        .render
        .files()
        .and_then(|f| f.0.infrastructure_failure())
    {
        input.log.finish().await;
        return Err(Error::Internal(anyhow::anyhow!(
            "run {} of execution {}: {outage}",
            c.id,
            c.execution_id
        )));
    }
    if let Ok(Outcome {
        state: RunState::Failed,
        message: Some(message),
        ..
    }) = &outcome
    {
        input.log.failure(message).await;
    }
    input.log.finish().await;
    let outcome = outcome?;
    // **The fence is the LATEST renewed lease, not the claim's.** Every
    // renewal moved `claimed_at`, so the claim's own value would match no row
    // and a live worker's outcome would be refused as `LeaseLost`.
    //
    // W9c Task 9: the outcome's transaction runs in `txn::transaction!`,
    // so a render in its `settle` that needs a read rolls the attempt back,
    // the read is fetched with no transaction open, and the attempt runs
    // again. **The fetch runs under the lease's renewal**, as the handler
    // did: the run is no longer locked by an open transaction while it
    // waits, so without the renewal a fetch longer than `working_timeout`
    // would let the reaper take it. A renewal that finds the fence moved
    // gives the run up exactly as during the handler.
    let mut lease = (held, renewed_at);
    let ended = transaction!(
        pool,
        Isolation::Default,
        |at| {
            let renewed = Claimed {
                claimed_at: lease.0,
                ..c.clone()
            };
            apply_outcome_with(at, &renewed, &outcome)
                .await
                .map(Finish::Commit)
        },
        |pf| {
            let fetched = under_lease_with(
                |h| renew_lease(pool, c.id, h),
                c.id,
                lease.0,
                timing.renew_every(),
                timing.working_timeout,
                lease.1,
                pf.fetch_pending(),
            )
            .await;
            match fetched {
                Leased::Kept {
                    out,
                    held,
                    renewed_at,
                } => {
                    out?;
                    lease = (held, renewed_at);
                    Ok(Fetched::Again)
                }
                Leased::Lost => Ok(Fetched::GiveUp),
            }
        }
    )?;
    if let Ended::GaveUp = ended {
        tracing::warn!(
            execution = %c.execution_id,
            run = %c.id,
            task_id = %c.task_id,
            "this worker lost the run's lease while it fetched what the run's successors \
             read; it records no outcome"
        );
        return Ok(Committed::LeaseLost);
    }
    // Only now: see "released AFTER the commit" above.
    if let Err(e) = reg.release(&input).await {
        tracing::warn!(
            execution = %c.execution_id,
            run = %c.id,
            task_id = %c.task_id,
            "the run's outcome is committed, but what it held could not be released \
             (the orphan sweep will remove it): {e}"
        );
    }
    Ok(Committed::Yes)
}

/// What [`run_one`] did with a run that did not error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Committed {
    /// Its result committed.
    Yes,
    /// A renewal found the fence moved; nothing was recorded.
    LeaseLost,
}

/// What [`under_lease`] ended with.
enum Leased<T> {
    /// The work finished while this worker still held the lease; `held` is
    /// the lease's latest value, the one [`apply_outcome`] must fence on, and
    /// `renewed_at` when the renewal that wrote it was sent (the self-fence's
    /// clock, for a later [`under_lease_with`] that carries on from here).
    Kept {
        out: T,
        held: DateTime<Utc>,
        renewed_at: tokio::time::Instant,
    },
    /// A renewal found the fence moved. The work was dropped unfinished.
    Lost,
}

/// Run `work` while renewing `c`'s lease every `every` — **Kestra's worker
/// heartbeat, on egret's claim model** (W5a spec §6).
///
/// Kestra resubmits a task only once its worker's heartbeat is confirmed dead
/// (`kestra.server.liveness`). egret has no worker registry; the lease IS the
/// heartbeat: [`super::reap`] reclaims a lease older than `working_timeout`,
/// so renewing it every third of that keeps a live worker's run from ever
/// being reclaimed, however long its handler — and a dead worker stops
/// renewing, so its run is reclaimed within `working_timeout`. This applies
/// to **every** handler, not only containers: an hour-long `egret.sql` is
/// covered by the same loop.
///
/// # The first renewal happens before the handler is polled
///
/// `interval`'s first tick is immediate and the `select!` is `biased` towards
/// it. So a run whose lease was reaped while it waited behind earlier members
/// of its batch (`tick` claims `n` at one instant and runs them in sequence)
/// is found lost **before its handler starts**, rather than run a second
/// time beside the worker that reclaimed it.
///
/// # Renewal is never cut off mid-statement
///
/// The renewal is awaited inside the `select!` arm's body, so the work
/// future is not polled while one is in flight, and when the work finishes no
/// renewal is outstanding. A renewal run as a concurrently-dropped future
/// could commit on the server after its future was dropped, leaving
/// `claimed_at` at a value this worker never saw and its own outcome refused.
///
/// # A renewal that ERRORS is retried -- until the worker SELF-FENCES
///
/// One failed renewal is logged and the next tick tries again. But a worker
/// cut off from Postgres cannot tell whether the reaper has already taken its
/// run, and -- since W5a names a container or pod by (run, attempt) so that the
/// next holder re-attaches to it -- a worker still inside its handler could
/// then KILL that work when its own budget expires, under the new holder (W5a
/// Task 5 fix round 1, review Important 4; the delete on finish it also
/// feared now happens only after a COMMITTED outcome, which a worker that
/// lost the run cannot make -- final review I3). So **once no renewal has succeeded for `fence_after`
/// (`working_timeout`, the reaper's own threshold), the worker gives the run
/// up exactly as on a moved fence**: the handler future is dropped (no
/// outcome, no delete, no cancel), and [`Leased::Lost`] is returned. Kestra's
/// worker does the same when its heartbeat fails.
///
/// The clock starts when the claim was TAKEN (`leased_at`, read before the
/// claim statement was sent), and each success restarts it from before its
/// renewal was sent -- both earlier than the `claimed_at` the database wrote,
/// so the local deadline always falls at or before the instant the reaper
/// could see the lease as expired. A renewal is awaited (the handler is not
/// polled meanwhile) for up to the later of the deadline and one interval: a
/// renewal that SUCCEEDS is proof the run is still this worker's, however
/// late; anything else once the deadline has passed gives the run up. A
/// renewal cut off by that limit that later commits server-side wrote a lease
/// this worker has already abandoned.
///
/// # What dropping `work` does, per handler
///
/// The four core handlers hold nothing. `egret.sql`'s query keeps running on
/// its engine (a dropped future tells a MySQL-protocol engine nothing).
/// `DockerRunner` (`egret.container`, `egret.script.python`) kills its
/// `docker start --attach` CLI (`kill_on_drop`), which does **not** stop the
/// container, and its `rm --force` never runs: the container is left for the
/// new holder, which is what spec §6 asks (W5a Task 4/5 re-attach to it).
/// [`super::TaskHandler::cancel`] is deliberately **not** called: the run is
/// someone else's now.
async fn under_lease<T>(
    pool: &PgPool,
    c: &Claimed,
    every: std::time::Duration,
    fence_after: std::time::Duration,
    leased_at: tokio::time::Instant,
    work: impl std::future::Future<Output = T>,
) -> Leased<T> {
    under_lease_with(
        |held| renew_lease(pool, c.id, held),
        c.id,
        c.claimed_at,
        every,
        fence_after,
        leased_at,
        work,
    )
    .await
}

/// [`under_lease`] over any renewal, so a test can make renewals fail.
async fn under_lease_with<T, R, F>(
    mut renew: R,
    run: Uuid,
    claimed_at: DateTime<Utc>,
    every: std::time::Duration,
    fence_after: std::time::Duration,
    leased_at: tokio::time::Instant,
    work: impl std::future::Future<Output = T>,
) -> Leased<T>
where
    R: FnMut(DateTime<Utc>) -> F,
    F: std::future::Future<Output = Result<Option<DateTime<Utc>>>>,
{
    let mut held = claimed_at;
    let mut last_ok = leased_at;
    let mut work = std::pin::pin!(work);
    let mut renew_tick = tokio::time::interval(every);
    renew_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let self_fence = |why: &str| {
        tracing::warn!(
            run = %run,
            "{why}: no lease renewal succeeded for {fence_after:?}, so this worker gives the \
             run up (self-fence) and stops following it"
        );
        Leased::Lost
    };
    loop {
        // Wait for the next renewal, the deadline, or the work. The renewal
        // is preferred: a renewal that SUCCEEDS is proof the run is still this
        // worker's, whatever the local clock says (a run that waited behind
        // its batch past the deadline keeps it if nobody reaped it). Reaching
        // the deadline also tries one renewal first, for the same reason. The
        // work future is not polled while a renewal is in flight, so waiting
        // for an answer past the deadline is safe; it is the WORK that must
        // not run on past the deadline unproven.
        tokio::select! {
            biased;
            _ = renew_tick.tick() => {}
            _ = tokio::time::sleep_until(last_ok + fence_after) => {}
            out = &mut work => {
                return Leased::Kept {
                    out,
                    held,
                    renewed_at: last_ok,
                }
            }
        }
        let sent = tokio::time::Instant::now();
        let limit = std::cmp::max(last_ok + fence_after, sent + every);
        match tokio::time::timeout_at(limit, renew(held)).await {
            Ok(Ok(Some(renewed))) => {
                held = renewed;
                last_ok = sent;
            }
            Ok(Ok(None)) => return Leased::Lost,
            Ok(Err(e)) => tracing::warn!(
                run = %run,
                error = %e,
                "could not renew the lease; retrying at the next renewal"
            ),
            Err(_) => tracing::warn!(run = %run, "a lease renewal went unanswered"),
        }
        if tokio::time::Instant::now() >= last_ok + fence_after {
            return self_fence("the last renewal did not succeed");
        }
    }
}

/// Move `run`'s lease to `now()`, **if this worker still holds it** — the
/// fence is `claimed_at = held`. `Some(new lease)` on success; `None` when the
/// fence moved (reaped, or reaped and re-claimed), which means the run is no
/// longer this worker's.
///
/// `RETURNING` the written value rather than reading `now()` separately, for
/// [`Claimed::claimed_at`]'s reason: the token must be exactly what the row
/// holds.
async fn renew_lease(
    pool: &PgPool,
    run: Uuid,
    held: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    sqlx::query_scalar(
        "UPDATE workflow.task_runs SET claimed_at = now() \
          WHERE id = $1 AND claimed_at = $2 \
          RETURNING claimed_at",
    )
    .bind(run)
    .bind(held)
    .fetch_optional(pool)
    .await
    .map_err(db_err)
}

/// What one claimed run's handler is handed, render context included.
///
/// **Read WITHOUT the execution's row lock**, and that is deliberate: this
/// happens before the handler runs and therefore before the tick's transaction
/// is opened, and taking the lock here would hold it across an arbitrarily long
/// handler -- the exact outage the two-commit split exists to prevent (see the
/// `worker` module doc). The run itself is leased, so nothing else will move
/// IT; a sibling of the same execution may finish while the handler runs, and
/// what that costs is that a handler's `outputs` are a snapshot rather than a
/// live view. Kestra's worker renders from a snapshot for the same reason.
async fn input_for(
    pool: &PgPool,
    c: &Claimed,
    reg: &dyn Registry,
    script_default: std::time::Duration,
) -> Result<(TaskInput, Budget)> {
    let mut conn = pool.acquire().await.map_err(db_err)?;
    // A reader: no `settle` runs on this context. The handler's render below
    // is LIVE (`scope_for`, which takes this POOL connection -- never a
    // transaction): it runs after this connection is returned, outside any
    // transaction.
    let (ctx, state, _now) = load_with(&mut conn, c.execution_id, Lock::No, Stack::Reader).await?;
    let mut surface = Surface::load(&mut conn, &ctx).await?;
    // W9a Task 4: the claimed run's OWN execution -- an iteration's id inside
    // a `Loop` (spec §3), never the loop root -- reads through a snapshot of
    // every URI in its source and render context. `task_prefix` is this run's
    // baseUri; a builder error (a namespace or id no URI can carry) leaves it
    // `None` here, and fails the task where a file is written (Task 6).
    let task_prefix = surface.flow().and_then(|f| {
        crate::storage::uri::task_prefix(&f.namespace, &f.id, c.execution_id, &c.task_id, c.id).ok()
    });
    // `taskrun.value` (fix round 1, M5): a loop iteration's value, which a
    // rendered `values:` can make a URI no source text holds.
    let texts: Vec<&str> = std::iter::once(c.source.as_str())
        .chain(c.value.as_deref())
        .collect();
    let files = crate::storage::scope::scope_for(
        &mut conn,
        crate::storage::current(),
        c.workspace_id,
        c.execution_id,
        &texts,
        &surface.snapshot_values(),
        task_prefix,
        surface.flow().map(|f| f.namespace.clone()),
    )
    .await?;
    surface.attach_files(files);
    let run = usize::try_from(c.run_ix).map_err(|_| {
        worker_err(WorkerError::Corrupt(
            c.id,
            format!("negative run_ix {}", c.run_ix),
        ))
    })?;
    c.input(
        surface.context(&ctx.plan, &state, Some(run)),
        reg.log_sink(),
        script_default,
    )
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
///
/// # A caller-owned transaction reads nothing (W9c Task 9)
///
/// **Test-only** (`test-util`; final review m3): production applies an
/// outcome in a transaction `worker::txn` owns ([`tick`]'s `run_one`,
/// [`commit_outcome`]). `settle`'s renders read through a cache filled
/// outside any transaction (`storage::prefetch`), and only an owner that can
/// roll back, fetch and run again fills it. This entry point is the caller's
/// transaction, so its cache stays empty: a render here that needs a read
/// (`kv()`, `read()`, a namespace file, a `Loop`'s values file) renders a
/// placeholder, and this answers `Err` -- the caller must not commit.
#[cfg(any(test, feature = "test-util"))]
pub async fn apply_outcome(
    conn: &mut PgConnection,
    claimed: &Claimed,
    outcome: &Outcome,
) -> Result<()> {
    let pf = Prefetch::new();
    let applied = apply_outcome_on(conn, &Gate::checked_by_caller(&pf), claimed, outcome).await;
    if pf.has_pending() {
        return Err(prefetch_miss());
    }
    applied
}

/// [`apply_outcome`] in a transaction of its own that this function owns:
/// `run_one`'s commit, without the handler and without the lease's renewal
/// (W9c Task 9). A render that needs a read is fetched with the transaction
/// rolled back, and the outcome applied again; `Ok` once it committed.
/// For a caller that holds an outcome but no handler -- a test, a driver.
pub async fn commit_outcome(pool: &PgPool, claimed: &Claimed, outcome: &Outcome) -> Result<()> {
    transaction!(pool, |at| {
        apply_outcome_with(at, claimed, outcome).await
    })
}

/// [`apply_outcome`] in an attempt of `worker::txn`, reading through its
/// gate's cache: the only production entry, so an outcome is applied only
/// where a miss is caught before the commit.
async fn apply_outcome_with(at: &mut Txn, claimed: &Claimed, outcome: &Outcome) -> Result<()> {
    let (conn, gate) = at.parts();
    apply_outcome_on(conn, gate, claimed, outcome).await
}

/// The body of [`apply_outcome_with`] (and of the test-only
/// [`apply_outcome`]). Private: a connection plus a gate is what only those
/// two hand it.
async fn apply_outcome_on(
    conn: &mut PgConnection,
    gate: &Gate,
    claimed: &Claimed,
    outcome: &Outcome,
) -> Result<()> {
    let (ctx, mut state, now) =
        load(conn, claimed.execution_id, Stack::Fresh(gate.clone())).await?;
    let before = state.clone();

    // The output goes down BEFORE `settle` renders, because a successor's
    // `{{ outputs.<this task> }}` has to see it in the same transaction that
    // finished the task. It is a column rather than a field on `ExecutionState`
    // for the reason `advance()` does not carry one: the state machine has no
    // use for it and a plan that knew what a `Return` produces would have
    // stopped being a state machine.
    //
    // **`AND claimed_at = $3` is the fence, and this is the first write it has
    // to be on.** A stale worker reaches here before it reaches `persist`, so
    // a fence only on the state UPDATE would let an outcome from a reaped
    // lease overwrite the live worker's outputs and still leave every other
    // column right.
    //
    // **`message` rides in the same statement, and that is not tidiness.** It
    // is written under the same fence for the same reason `outputs` is — a
    // stale worker that could overwrite one could overwrite the other — and it
    // is set on every outcome including a success, so a run that failed, was
    // retried and then succeeded does not keep the first attempt's reason
    // beside a `SUCCESS` state. See migration 0137.
    let hit = sqlx::query(
        "UPDATE workflow.task_runs SET outputs = $1, message = $4 \
           WHERE id = $2 AND claimed_at = $3",
    )
    .bind(&outcome.outputs)
    .bind(claimed.id)
    .bind(claimed.claimed_at)
    .bind(&outcome.message)
    .execute(&mut *conn)
    .await
    .map_err(db_err)?;
    // **0 rows affected is an error, not a success.** They are the same value
    // to `execute()`, which is exactly how a fence that is written but never
    // read passes every test there is.
    if hit.rows_affected() == 0 {
        return Err(worker_err(WorkerError::LeaseLost(claimed.id)));
    }
    // **W9a Task 6 (spec §4.3): the run's files are indexed HERE** -- after
    // the fence, so a worker whose lease was taken returns above and indexes
    // nothing (its uploads stay orphans for the pruner, §10), and before
    // `settle`, so a successor rendered in this same transaction can read
    // them. A file exists for a flow only once its row does (§4.2), so a run
    // that never commits leaves none. A re-claim writes the same deterministic
    // URIs, and the insert updates its own rows.
    //
    // The fence guards the INDEX, not the objects (fix round 1, M6): uploads
    // happen before this point, so a fenced zombie worker -- one still
    // collecting after its lease was taken -- can overwrite an object that a
    // successor already committed under the same deterministic name. Both
    // read the same container or pod for the same attempt, so the bytes are
    // normally identical; nothing here can make them so.
    crate::storage::index_task_files(conn, claimed.workspace_id, claimed.id, &outcome.files)
        .await?;

    let run = usize::try_from(claimed.run_ix).map_err(|_| {
        worker_err(WorkerError::Corrupt(
            claimed.id,
            format!("negative run_ix {}", claimed.run_ix),
        ))
    })?;
    // **The `allowFailure:`/`allowWarning:` door for a RUNNABLE, and it is the
    // worker's rather than `advance()`'s** -- W8 Task 5. The artifact puts it
    // in the same place, `WorkerTaskProcessor`'s nominal path, between the
    // handler's answer and `taskRun.withState(state)`; `advance()` is pure over
    // `(plan, state, now)` and never sees a handler's outcome, so it could not
    // hold this even if the division of labour were free to move.
    //
    // Only the RUN's state moves. The attempt already carries the handler's own
    // terminal and keeps it, which is exactly what
    // `valids/task-allow-failure-runnable`'s golden holds: a run ending
    // `WARNING` over three attempts that all end `FAILED`.
    let terminal =
        crate::executor::allow_failure::for_runnable(&ctx.plan, &state, run, outcome.state, now);
    // `SetRunState` while the two agree, so every run nobody wrote a flag on
    // takes exactly the path it took before this task; `CompleteRun` only where
    // they differ, because the attempt must keep the handler's own terminal.
    let cmd = if terminal == outcome.state {
        Command::SetRunState {
            run,
            state: terminal,
        }
    } else {
        Command::CompleteRun {
            run,
            run_state: terminal,
            attempt: outcome.state,
        }
    };
    apply(&mut state, &cmd);
    settle(
        conn,
        &ctx,
        &before,
        state,
        now,
        Some(Lease {
            run_ix: claimed.run_ix,
            run_id: claimed.id,
            claimed_at: claimed.claimed_at,
        }),
    )
    .await
}

/// **Whether an execution reaching `s` tells its parent** — the predicate the
/// two links below are gated on, named so that something other than a comment
/// can assert it.
///
/// It is `is_terminal` and the indirection is the point: `worker::subflow`'s
/// [`guess_state`](super::subflow) omits Java's `isPaused()` disjunct from its
/// trigger set, and that omission is sound **only because** a `PAUSED` child
/// never reaches it. That "only because" lived in prose on both sides until W8
/// Task 5 fix round 3. `a_paused_child_cannot_reach_guess_state` calls this
/// function, so a change to the filter here breaks a test rather than a
/// paragraph.
pub(super) fn notifies_parent(s: RunState) -> bool {
    s.is_terminal()
}

/// The lease a writer holds, and the run it releases.
///
/// Carried as one value rather than as a bare `run_ix` because the two halves
/// must not be able to drift: the row whose lease is cleared is the row the
/// fence is checked on, and a signature that took them separately would let a
/// caller release one run while fencing on another's token.
///
/// `None` at [`start_execution`], [`resume`] and every spawn, where nothing is
/// leased and there is therefore nothing to fence.
#[derive(Debug, Clone, Copy)]
pub(super) struct Lease {
    /// The `RunIx` whose lease this write clears.
    pub run_ix: i32,
    /// The `task_runs` surrogate key, carried only so a lost lease can be
    /// reported against the row a production log can look up.
    pub run_id: Uuid,
    /// The fencing token — [`Claimed::claimed_at`].
    pub claimed_at: chrono::DateTime<chrono::Utc>,
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
///
/// `state` is MOVED into the walk's top frame (re-review 3 n1): nothing is
/// written back to the owner, and every owner returns this function's result
/// directly, so no owner reads its state after the settle.
async fn settle(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    before: &ExecutionState,
    state: ExecutionState,
    now: Tick,
    release: Option<Lease>,
) -> Result<()> {
    walk(
        conn,
        Frame {
            ctx: Held::Borrowed(ctx),
            before: before.clone(),
            state,
            now,
            release,
            jobs: VecDeque::new(),
            phase: Phase::Begin,
        },
    )
    .await
}

/// The context a [`Frame`] settles: the owner's own, for the settle an owner
/// starts ([`settle`]), or one the frame owns, for every settle started
/// inside it.
enum Held<'a> {
    Borrowed(&'a ExecutionCtx),
    Owned(Box<ExecutionCtx>),
}

impl std::ops::Deref for Held<'_> {
    type Target = ExecutionCtx;
    fn deref(&self) -> &ExecutionCtx {
        match self {
            Held::Borrowed(ctx) => ctx,
            Held::Owned(ctx) => ctx,
        }
    }
}

/// **One settle, as data** (W9c Task 9 fix round 3, re-review C2/C3): its
/// context and states, the phase it has reached, and the settles it started
/// and has not run yet.
///
/// A settle starts other settles from its HEAD as well as from its tail: a
/// `Loop` iteration's first settle (`spawn::open_batch`), a `Subflow` child's
/// first settle and the completion of a refused or `wait: false` one
/// (`subflow::spawn_one`), a replay's first settle or its refusal
/// (`replay::spawn_due`). Each of those used to run inline, head AND walk, so
/// a chain that came back through one of them -- a sequential `Loop` whose
/// `Subflow` is refused (C2), a sequential `Loop` inside another (C3) --
/// nested one settle per iteration on the Rust stack and aborted the
/// process. Now none of them can run a settle at all: they return [`Job`]s,
/// and only [`walk`] -- private to this module, called only by [`settle`],
/// which only the transaction owners call -- runs one. A frame runs its jobs
/// in order, each to the end of its own tail, before its next phase, which
/// is exactly where the inline call ran it.
pub(super) struct Frame<'a> {
    ctx: Held<'a>,
    before: ExecutionState,
    state: ExecutionState,
    now: Tick,
    release: Option<Lease>,
    /// Settles this one has started and not yet run, in order.
    jobs: VecDeque<Job>,
    phase: Phase,
}

impl Frame<'static> {
    /// A new execution's first settle: from the empty state, at `Tick(0)` --
    /// a `Tick` is milliseconds since the EXECUTION was created and this is
    /// that instant, for the execution's own clock.
    pub(super) fn first(ctx: ExecutionCtx) -> Self {
        let empty = ExecutionState::new(&ctx.plan);
        Frame {
            state: empty.clone(),
            before: empty,
            ctx: Held::Owned(Box::new(ctx)),
            now: Tick(0),
            release: None,
            jobs: VecDeque::new(),
            phase: Phase::Begin,
        }
    }

    /// A settle of a loaded execution, `state` being `before` with the
    /// caller's commands applied.
    pub(super) fn loaded(
        ctx: ExecutionCtx,
        before: ExecutionState,
        state: ExecutionState,
        now: Tick,
    ) -> Self {
        Frame {
            ctx: Held::Owned(Box::new(ctx)),
            before,
            state,
            now,
            release: None,
            jobs: VecDeque::new(),
            phase: Phase::Begin,
        }
    }
}

/// A settle some code started, for [`walk`] to run: what every function that
/// used to call `settle` returns instead.
pub(super) enum Job {
    /// A settle whose context and state are built.
    Settle(Box<Frame<'static>>),
    /// `subflow::complete`: load the execution, write the run's outputs and
    /// message, apply the states, and settle -- opened when the walk reaches
    /// it, which is where the call used to run.
    Complete(subflow::Completion),
}

/// Where a [`Frame`]'s head has got to: steps 1-3 of the settle and the
/// outbox, cut at every point that can start another settle.
enum Phase {
    /// Steps 1-2: render, drive, persist, the render side.
    Begin,
    /// Step 3, the loop spawner: one loop run's batch per advance, in the
    /// order the runs were first asked for.
    Loops {
        surface: Surface,
        runs: VecDeque<RunIx>,
        spawns: Vec<Requested>,
        subflows: Vec<subflow::Requested>,
    },
    /// Step 3, the subflow spawner: one request per advance.
    Subflows {
        surface: Surface,
        subflows: Vec<subflow::Requested>,
        next: usize,
    },
    /// The execution-delay loop.
    Replay { surface: Surface },
    /// The Flow-trigger outbox, then the tail.
    Outbox { surface: Surface },
}

/// What a settle leaves to do once its head is done: step 4 (tell a parent)
/// and the outage check -- its tail, carried as data so [`walk`] runs a chain
/// of them without nesting one inside the next.
#[derive(Debug)]
pub(super) struct Settled {
    /// The execution that settled.
    id: Uuid,
    /// The terminal it reached in this settle, if it reached one that
    /// [`notifies_parent`].
    terminal: Option<RunState>,
    /// [`ExecutionCtx::caller`] of the settle: what a nested load passes.
    caller: Caller,
    /// The store outage a render of this settle met (spec §11).
    outage: Option<String>,
}

/// One step of [`walk`].
enum Step<'a> {
    /// Advance a settle's head by one phase, or run its next job.
    Frame(Box<Frame<'a>>),
    /// Open a settle some code started.
    Job(Job),
    /// Tell the loop that spawned it (`spawn::on_execution_terminal`).
    Loop(Settled),
    /// Tell the run that started it (`subflow::on_child_terminal`).
    Child(Settled),
    /// The outage check.
    Outage(Settled),
}

/// **THE walk: every settle of one transaction, run from one explicit
/// stack**, so the Rust stack this uses is the same at any iteration count
/// and any nesting (W9c Task 9 fix rounds 2 and 3; re-review N1, C2, C3).
///
/// The order is the order the recursion ran things in, because a step that
/// starts a settle pushes it ABOVE what remains of its starter:
///
/// - a [`Frame`] runs its pending [`Job`]s one at a time -- each to the end
///   of its own tail -- before its next phase, which is where the inline call
///   ran it, and its next job only after that;
/// - a finished head becomes its tail: `on_execution_terminal(s)` with the
///   whole of what it started, then `on_child_terminal(s)` with the whole of
///   what it started, then `s`'s outage check.
///
/// An `Err` returns at once, as the `?` of the recursion did.
///
/// Private, and called only by [`settle`]: no other function can run a
/// settle, so nothing a settle calls can nest one.
async fn walk<'a>(conn: &mut PgConnection, top: Frame<'a>) -> Result<()> {
    let mut steps: Vec<Step<'a>> = vec![Step::Frame(Box::new(top))];
    while let Some(step) = steps.pop() {
        match step {
            Step::Frame(mut frame) => {
                if let Some(job) = frame.jobs.pop_front() {
                    steps.push(Step::Frame(frame));
                    steps.push(Step::Job(job));
                    continue;
                }
                match Box::pin(advance_frame(conn, &mut frame)).await? {
                    Some(settled) => steps.push(Step::Loop(settled)),
                    None => steps.push(Step::Frame(frame)),
                }
            }
            Step::Job(Job::Settle(frame)) => steps.push(Step::Frame(frame)),
            Step::Job(Job::Complete(completion)) => {
                if let Some(frame) = Box::pin(subflow::open_completion(conn, completion)).await? {
                    steps.push(Step::Frame(Box::new(frame)));
                }
            }
            Step::Loop(s) => {
                let started = match s.terminal {
                    Some(terminal) => {
                        Box::pin(spawn::on_execution_terminal(
                            conn,
                            s.id,
                            terminal,
                            s.caller.clone(),
                        ))
                        .await?
                    }
                    None => None,
                };
                steps.push(Step::Child(s));
                if let Some(job) = started {
                    steps.push(Step::Job(job));
                }
            }
            Step::Child(s) => {
                // The other kind of link. `on_execution_terminal` reads
                // `parent_execution_id` and this reads `trigger_execution_id`,
                // and `workflow_executions_one_link_kind` makes it impossible
                // for both to find something -- so the pair is a dispatch, not
                // two chances to report the same terminal.
                //
                // **For a `CREATE_NEW_EXECUTION` replay NEITHER finds
                // anything**, and that is why egret's answer for a replayed
                // `Subflow` child is invariant rather than timing-dependent:
                // `trigger_of` zips two NULL trigger columns, and `parent_of`
                // zips a set `parent_execution_id` with a NULL `parent_run_ix`,
                // which `workflow_executions_loop_run_whole_or_absent` keeps
                // NULL for a row with no loop index. Registered as a
                // divergence in `oracle-exec/README.md`'s coverage statement,
                // item 16.
                let started = match s.terminal {
                    Some(terminal) => {
                        Box::pin(subflow::on_child_terminal(
                            conn,
                            s.id,
                            terminal,
                            s.caller.clone(),
                        ))
                        .await?
                    }
                    None => None,
                };
                steps.push(Step::Outage(s));
                if let Some(job) = started {
                    steps.push(Step::Job(job));
                }
            }
            // Spec §11 (fix round 1, I2): a render this settle ran met a store
            // OUTAGE. It failed a run, and that failure is not the author's:
            // an `Err` here rolls the caller's whole transaction back --
            // `apply_outcome`'s write included -- so the lease lapses and the
            // run is re-claimed with no attempt spent, exactly as a handler
            // `Err` is.
            Step::Outage(s) => {
                if let Some(outage) = s.outage {
                    return Err(Error::Internal(anyhow::anyhow!(
                        "execution {}: {outage}",
                        s.id
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Run one phase of `frame`'s head: the settle's tail when the head is done,
/// otherwise `None` (with any settles the phase started queued as jobs).
async fn advance_frame(conn: &mut PgConnection, frame: &mut Frame<'_>) -> Result<Option<Settled>> {
    let Frame {
        ctx,
        before,
        state,
        now,
        release,
        jobs,
        phase,
    } = frame;
    let ctx: &ExecutionCtx = ctx;
    match std::mem::replace(phase, Phase::Begin) {
        Phase::Begin => {
            let (surface, spawns, subflows) =
                begin(conn, ctx, before, state, *now, *release).await?;
            // Grouped by the loop run, because the index is per loop run and
            // two `Loop`s can be in flight in one execution.
            let mut runs = VecDeque::new();
            for r in &spawns {
                if !runs.contains(&r.parent_run) {
                    runs.push_back(r.parent_run);
                }
            }
            *phase = Phase::Loops {
                surface,
                runs,
                spawns,
                subflows,
            };
        }
        Phase::Loops {
            surface,
            mut runs,
            spawns,
            subflows,
        } => match runs.pop_front() {
            Some(run) => {
                let batch: Vec<Requested> = spawns
                    .iter()
                    .filter(|r| r.parent_run == run)
                    .cloned()
                    .collect();
                jobs.extend(spawn::open_batch(conn, ctx, state, run, &batch).await?);
                *phase = Phase::Loops {
                    surface,
                    runs,
                    spawns,
                    subflows,
                };
            }
            // **After `persist`, exactly like the loop spawner, and for the
            // same reason**: the child's row names `(execution, run_ix)` and
            // that run has to exist. `subflow::spawn_one` may complete the
            // parent's own run (a `wait: false` subflow, or a target that does
            // not resolve), and it does so through a nested settle on this
            // same execution -- the same re-entrancy the loop spawner has via
            // `on_execution_terminal` -- which is why the terminal check in
            // the outbox phase reads `state`, this settle's in-memory copy,
            // which a nested settle deliberately does not update, rather than
            // the row: a nested settle that terminated the execution has
            // ALREADY told its parent, and firing again from here would report
            // one terminal twice.
            None => {
                *phase = Phase::Subflows {
                    surface,
                    subflows,
                    next: 0,
                }
            }
        },
        Phase::Subflows {
            surface,
            subflows,
            next,
        } => match subflows.get(next) {
            Some(r) => {
                jobs.extend(subflow::spawn_one(conn, ctx, &surface, state, r).await?);
                *phase = Phase::Subflows {
                    surface,
                    subflows,
                    next: next + 1,
                };
            }
            None => *phase = Phase::Replay { surface },
        },
        // **The execution-delay loop, and it is a different loop in the
        // artifact too.** A `CREATE_NEW_EXECUTION` retry ends THIS execution
        // `RETRIED` and leaves an `ExecutionDelay` behind (`retry::scan`'s
        // `ScheduleRetry`); the replacement execution is started later, by
        // whoever notices the deadline. Here that is every `settle`, because
        // `resume` -- from `worker::wake` in production and from `pass_time` in
        // the oracle's driver -- is the settle a due deadline produces. It
        // reads `state`, so the round that scheduled the delay can see it too,
        // and refuses on `not_before > now`.
        Phase::Replay { surface } => {
            jobs.extend(replay::spawn_due(conn, ctx, state, *now).await?);
            *phase = Phase::Outbox { surface };
        }
        Phase::Outbox { surface } => {
            outbox(conn, ctx, before, state).await?;
            // Step 4 is the tail ([`walk`]): if this settle reached a
            // terminal, tell the parent. `was != is` reads `state` -- this
            // settle's in-memory copy, which a nested settle deliberately does
            // not update -- so a nested settle that terminated the execution
            // has ALREADY told its parent, and this one does not report the
            // terminal twice.
            let was = before.history.last().copied();
            let is = state.history.last().copied();
            return Ok(Some(Settled {
                id: ctx.id,
                terminal: is.filter(|s| was != Some(*s) && notifies_parent(*s)),
                caller: ctx.caller(),
                outage: surface.files_outage(),
            }));
        }
    }
    Ok(None)
}

/// Steps 1-2 of a settle: the render scope, the SLA, `drive`, `persist` and
/// the render side. Starts no settle. Returns the surface and what `drive`
/// asked to spawn.
async fn begin(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    before: &ExecutionState,
    state: &mut ExecutionState,
    now: Tick,
    release: Option<Lease>,
) -> Result<(Surface, Vec<Requested>, Vec<subflow::Requested>)> {
    let mut surface = Surface::load(conn, ctx).await?;
    // W9a Task 4: the snapshot every render of this settle reads files
    // through -- `runIf`, the five tier-B expressions, the SLA, a subflow's
    // inputs -- taken on `conn`, i.e. INSIDE the caller's transaction, which
    // is the only connection that sees the index rows of the task this
    // transaction has just finished.
    //
    // W9c Task 9: and it is the IN-TRANSACTION scope, which holds no store
    // and no connect options. Every body, namespace-file and KV read is
    // answered from the owner's cache (`ctx.gate`); a miss renders a
    // placeholder and is fetched by the owner with this transaction rolled
    // back (`worker::txn`). A context with no gate is a reader's, and a
    // reader never settles.
    let gate = ctx.gate.as_ref().ok_or_else(|| {
        Error::Internal(anyhow::anyhow!(
            "execution {}: a reader's context reached settle, which renders only under an \
             attempt's gate",
            ctx.id
        ))
    })?;
    let texts: Vec<&str> = std::iter::once(ctx.source.as_str())
        .chain(state.runs.iter().filter_map(|r| r.value.as_deref()))
        .collect();
    let files = crate::storage::scope::scope_in_transaction(
        conn,
        crate::storage::current(),
        ctx.workspace_id,
        ctx.id,
        &texts,
        &surface.snapshot_values(),
        None,
        surface.flow().map(|f| f.namespace.clone()),
        gate,
    )
    .await?;
    surface.attach_files(files);
    let mut marks = Marks::default();
    // The caller's own command -- a handler's terminal, or the `SetRunState` a
    // loop's completion applies -- is step `ctx.seq + 1`, and every round below
    // is one more. Nothing shares a step with a round of the PREVIOUS `settle`,
    // which is what makes "created strictly after the other ended" mean the
    // same thing across a transaction boundary as it does inside one.
    let mut seq = ctx.seq + 1;
    marks.observe(before, state, seq);
    // **`ExecutorService.handleExecutionChangedSLA`, and it is here for the
    // same reason it is there**: the artifact evaluates it on every execution
    // message, immediately BEFORE `executorService.process(executor)`
    // (`ExecutionEventMessageHandler.java:244`). `drive` below is `process`, so
    // this is that edge. An execution the SLA terminates is terminal when
    // `advance()` is first asked, and `advance()` returns nothing for a
    // terminal execution -- which is what produces the golden's
    // `["CREATED","CANCELLED"]` with no runs, rather than a `RUNNING` that has
    // to be taken back.
    if let Some(terminal) = sla_terminal(&surface, ctx, state) {
        let snapshot = state.clone();
        seq += 1;
        apply(state, &Command::SetExecutionState { state: terminal });
        marks.observe(&snapshot, state, seq);
    }
    // W9a Task 11: what a render produced beside the state -- the runs it
    // failed WITH a message, and a file `Loop`'s values -- written after the
    // state `persist` writes and BEFORE `spawn::open_batch`, which reads the values.
    let mut side = render::RenderSide::default();
    let (spawns, subflows) = drive(ctx, &surface, state, now, &mut marks, &mut seq, &mut side)?;
    // W7b Task 1: the attempt ends this settle produced, stamped into the state
    // `persist` serialises, so they ride the same UPDATE as the terminal state.
    stamps::stamp_attempt_ends(before, state, now);
    persist(conn, ctx, before, state, release, &marks, seq).await?;
    if !side.failures.is_empty() || !side.file_values.is_empty() {
        // Built and boxed in a SYNC callee: `settle` re-enters itself through
        // a `Subflow` chain, and a statement future built inline (even one
        // boxed here) grows every level's poll frame in a debug build -- the
        // worker suite's `a_chain_of_subflows_deeper_than_the_bound…`
        // overflowed with either.
        write_render_side(conn, ctx.id, side).await?;
    }
    Ok((surface, spawns, subflows))
}

/// The Flow-trigger outbox of one settle.
async fn outbox(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    before: &ExecutionState,
    state: &ExecutionState,
) -> Result<()> {
    // **The Flow-trigger outbox (W13 spec §4.3), and it is the ONE statement
    // W13a adds to this transaction.** One event per state this settle
    // APPENDED to the execution's history, except `CREATED`, in the order they
    // were appended -- Kestra's `processFlowTriggers` runs once per
    // transition. Not only the last state: a bounded `Pause` start goes
    // `CREATED -> RUNNING -> PAUSED` inside one settle, and the settle that
    // ends one pause and enters the next goes `PAUSED -> RESTARTED -> RUNNING
    // -> PAUSED`, whose first and last states are equal. And not only terminal
    // states, which is why this is not inside `notifies_parent`'s branch:
    // `PAUSED` is in a Flow trigger's default `states` and is not terminal.
    //
    // The slice is sound because execution history is append-only across a
    // settle: every caller builds `before` as a clone of `state` and then only
    // `apply`s commands, and `Command::SetExecutionState` -- a `push` -- is the
    // one writer of the execution's history. The `debug_assert` pins that.
    //
    // ONE statement whatever the count: `unnest ... WITH ORDINALITY` keeps the
    // order, so the bigserial `id` carries it, and the `EXISTS` is evaluated
    // per state, so the outbox stays empty for a workspace with no listener
    // that could hear any of them. It runs on `conn`, inside the caller's
    // transaction: a transition that rolls back leaves no event.
    //
    // `wire()` is the spelling `flow_listeners.states` holds;
    // `run_state_wire_is_the_model_states_serde_spelling` pins it to
    // `models::State`'s serde spelling.
    debug_assert!(
        state.history.starts_with(&before.history),
        "execution history must be append-only across a settle: {:?} -> {:?}",
        before.history,
        state.history
    );
    let appended: Vec<&str> = state
        .history
        .get(before.history.len()..)
        .unwrap_or_default()
        .iter()
        .filter(|s| **s != RunState::Created)
        .map(|s| s.wire())
        .collect();
    if !appended.is_empty() {
        sqlx::query(
            "INSERT INTO workflow.flow_trigger_events (source_execution_id, workspace_id, state) \
             SELECT $1, $2, t.s \
               FROM unnest($3::text[]) WITH ORDINALITY AS t(s, n) \
              WHERE EXISTS (SELECT 1 FROM workflow.flow_listeners l \
                             WHERE l.workspace_id = $2 AND NOT l.disabled \
                               AND t.s = ANY (l.states)) \
              ORDER BY t.n",
        )
        .bind(ctx.id)
        .bind(ctx.workspace_id)
        .bind(&appended)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;
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
    inputs: serde_json::Value,
) -> Result<Uuid> {
    start_execution_with_trigger(pool, workspace_id, flow_id, source, inputs, None).await
}

/// [`start_execution`] with Kestra's `Execution.trigger` supplied.
///
/// A **sibling** rather than a sixth parameter on `start_execution`, because
/// all but one call site of that function has no trigger: widening it would
/// mean writing `None` at every one of them to serve a single caller, and a
/// long run of uniform `None`s is the kind of argument that hides a mutation
/// (W6 Task 6's `max_depth` survived a two-sided mutation for exactly that
/// reason -- every fixture passed the default). Every call site is a test or
/// `oracle-exec/driver.rs`; `crates/*/src` holds none.
///
/// # The count this line used to carry is GONE, and that is the fix
///
/// It said **65**, with the ordinal **"sixty-four times"** beside it. Both were
/// true when W6 Task 6 wrote them and neither was true afterwards: the figure
/// had already decayed before W10 touched anything, and W10 Task 2's own tests
/// moved it again. Two things were wrong at once, and only one of them is about
/// being out of date:
///
/// 1. **An ordinal shares no letters with the number it names.** A sweep that
///    grepped `65` would have corrected the count and left "sixty-four"
///    disagreeing with it.
/// 2. **A count needs a recipe, and a recipe is a second representation.**
///    Whether the definition line counts, whether `start_execution_with_trigger`
///    counts, whether doc mentions count — each answer gives a different number,
///    so any figure here is only reproducible beside the exact grep that made
///    it, and then there are two things to keep in step instead of one.
///
/// The argument never needed a number: "all but one" is the whole of it, and it
/// is true at every count. **Removing the figure is the structural fix; dating
/// it would only have been the editorial one.**
///
/// The figure is counted, not estimated. This doc said "twenty" until fix round
/// 1's sweep ran
/// `grep -rn "start_execution(" crates/ --include="*.rs" | grep -v with_trigger | grep -v "///"`
/// on 2026-09-12 and got 65. A number nobody has compared to a running command
/// is the same shape of claim as an untested guard, which is the point of the
/// sentence it appears in.
///
/// `trigger` is `{id, type, variables}` and lands in
/// `workflow.executions.execution_trigger` (migration 0140), whose CHECK
/// refuses a record missing any of the three. It is stored and **not**
/// published into the render context; see that migration for the measurement
/// behind that split.
///
/// It is written in the SAME transaction as the execution row and its first
/// settle, so an execution can never be visible without the record of what
/// started it.
pub async fn start_execution_with_trigger(
    pool: &PgPool,
    workspace_id: Uuid,
    flow_id: Option<Uuid>,
    source: &str,
    inputs: serde_json::Value,
    trigger: Option<serde_json::Value>,
) -> Result<Uuid> {
    transaction!(pool, |at| {
        start_execution_in_with(
            at,
            NewExecution {
                workspace_id,
                flow_id,
                source,
                inputs: inputs.clone(),
                trigger: trigger.clone(),
                // Only the scheduler (W12 Task 4) fires a `Schedule` tick, and it
                // calls `start_execution_in` directly on the transaction that
                // advances `next_evaluation_date` -- this wrapper is never that
                // caller.
                scheduled_date: None,
                // Likewise only the Flow-trigger processor (W13a Task 4) starts an
                // execution because another one reached a state; this wrapper's
                // callers -- the API, a webhook, the tests -- start at the bottom.
                trigger_execution_id: None,
                execution_depth: 0,
                flow_trigger_event_id: None,
                born_failed: false,
                // A manual replay calls `start_execution_in` itself with the link.
                replayed_from_execution_id: None,
                // Only the start route uploads FILE inputs, and it calls
                // `start_execution_in` itself with the id it uploaded under.
                id: None,
                input_files: Vec::new(),
            },
        )
        .await
    })
}

/// Everything a caller supplies to create an execution. A struct because the
/// alternative is a seventh positional argument, and a run of uniform `None`s
/// is the shape that hid W6 Task 6's surviving mutant.
pub struct NewExecution<'a> {
    pub workspace_id: Uuid,
    pub flow_id: Option<Uuid>,
    pub source: &'a str,
    pub inputs: serde_json::Value,
    pub trigger: Option<serde_json::Value>,
    pub scheduled_date: Option<DateTime<Utc>>,
    /// The SOURCE execution a Flow trigger fired on (W13 spec §4.5) -- the
    /// same column a `Subflow` child's caller is written to, but with no
    /// `trigger_run_ix`, which is what keeps `subflow::on_child_terminal`
    /// (it zips the two) from reporting a flow-fired execution's terminal to
    /// its source. `None` for every other start.
    pub trigger_execution_id: Option<Uuid>,
    /// Kestra's `metadata.executionDepth`: the source's own depth plus one
    /// for a Flow-trigger fire, `0` for every other start. Persisted so a
    /// chain's depth is data, not stack (spec §4.4 point 4).
    pub execution_depth: i32,
    /// The outbox event (`workflow.flow_trigger_events.id`) a Flow-trigger
    /// fire consumed. Set iff `trigger` is a `Flow` record -- 0157's
    /// `workflow_executions_flow_trigger_event_iff_flow` -- and keyed by
    /// `workflow_executions_one_per_flow_trigger_event`, so one event fires
    /// one listener at most once.
    pub flow_trigger_event_id: Option<i64>,
    /// **Start the execution already FAILED, with no task run** (W13a fix
    /// round 1). A Flow trigger whose `inputs:` do not render against the
    /// source's context is not a refusal on the artifact: Kestra creates the
    /// listener's execution `CREATED -> FAILED`, `taskRunList: []`, no
    /// `inputs` (W13a Task 1, `listener_run1_FAILED_bad_inputs`). `false`
    /// for every other start.
    ///
    /// A `bool`, not the reason: `workflow.executions` has no
    /// execution-level message column, so there is nowhere to keep it. The
    /// processor logs the render error at `warn` instead.
    pub born_failed: bool,
    /// The manual replay's link (W9a spec §4.4 item 3): the execution
    /// `POST .../executions/{id}/replay` replays, written to
    /// `workflow.executions.replayed_from_execution_id` (migration 0166).
    /// `None` for every other start. A replay reads its source chain's files.
    pub replayed_from_execution_id: Option<Uuid>,
    /// The execution's id, when the caller had to know it before the row
    /// existed (W9a Task 7: the start route streams uploaded FILE inputs to
    /// `inputs/{input}/{file}` of this id first). `None` is the database's
    /// `gen_random_uuid()`, as for every other start.
    pub id: Option<Uuid>,
    /// The FILE inputs uploaded for THIS execution (spec §7.1), indexed here
    /// in the transaction that inserts the row. Empty for every start that
    /// uploads nothing.
    pub input_files: Vec<crate::storage::StoredFile>,
}

/// [`start_execution_with_trigger`]'s body, taking the caller's own `conn`
/// rather than a pool -- the point being that a caller already inside a
/// transaction (W12 Task 4's scheduler, advancing a `Schedule` trigger's
/// `next_evaluation_date`) can create the execution in that SAME transaction,
/// so the trigger's advance and the execution it fires commit or roll back
/// together. `start_execution_with_trigger` is now the thin wrapper: begin,
/// call this, commit.
///
/// # A caller-owned transaction reads nothing (W9c Task 9)
///
/// **Test-only** (`test-util`; final review m3), as [`apply_outcome`]: the
/// first settle renders through a cache only an owner that can roll back and
/// fetch fills, so here it stays empty, a render that needs a read renders a
/// placeholder, and this answers `Err` -- the caller must not commit. The
/// scheduler, the Flow-trigger processor, the routes and
/// [`start_execution_with_trigger`] call [`start_execution_in_with`] inside
/// `worker::txn`.
#[cfg(any(test, feature = "test-util"))]
pub async fn start_execution_in(conn: &mut PgConnection, new: NewExecution<'_>) -> Result<Uuid> {
    let pf = Prefetch::new();
    let started = start_execution_in_on(conn, &Gate::checked_by_caller(&pf), new).await;
    if pf.has_pending() {
        return Err(prefetch_miss());
    }
    started
}

/// [`start_execution_in`] in an attempt of `worker::txn` (or a savepoint of
/// one), reading through its gate's cache: the only production entry.
pub(crate) async fn start_execution_in_with(
    at: &mut impl InTxn,
    new: NewExecution<'_>,
) -> Result<Uuid> {
    let (conn, gate) = at.parts();
    start_execution_in_on(conn, gate, new).await
}

/// The body of [`start_execution_in_with`] (and of the test-only
/// [`start_execution_in`]). Private, as [`apply_outcome_on`].
async fn start_execution_in_on(
    conn: &mut PgConnection,
    gate: &Gate,
    new: NewExecution<'_>,
) -> Result<Uuid> {
    let NewExecution {
        workspace_id,
        flow_id,
        source,
        inputs,
        trigger,
        scheduled_date,
        trigger_execution_id,
        execution_depth,
        flow_trigger_event_id,
        born_failed,
        replayed_from_execution_id,
        id,
        input_files,
    } = new;

    let flow = crate::spec::parse(source)
        .map_err(|e| Error::InvalidInput(format!("the flow does not parse: {}", e.message)))?;
    let plan = build_plan(source)
        .map_err(|e| Error::InvalidInput(format!("the flow does not plan: {e}")))?;

    let inputs = if inputs.is_object() {
        inputs
    } else {
        serde_json::json!({})
    };
    let id: Uuid = sqlx::query_scalar(concat!(
        "INSERT INTO workflow.executions \
           (id, workspace_id, flow_id, namespace, flow_key, source, state, history, inputs, \
            execution_trigger, scheduled_date, trigger_execution_id, execution_depth, \
            flow_trigger_event_id, replayed_from_execution_id, ",
        stamp_columns!(),
        ") VALUES (COALESCE($13, gen_random_uuid()), $1, $2, $3, $4, $5, 'CREATED', \
                 '[\"CREATED\"]'::jsonb, $6, $7, $8, $9, $10, $11, $12, ",
        // Both NULL for a row born `CREATED`; spelled through the helper so the
        // rule has no exception for the reader to remember (`stamps`).
        stamp_values!("'CREATED'", "'[\"CREATED\"]'"),
        ") RETURNING id",
    ))
    .bind(workspace_id)
    .bind(flow_id)
    .bind(&flow.namespace)
    .bind(&flow.id)
    .bind(source)
    .bind(&inputs)
    .bind(trigger)
    .bind(scheduled_date)
    .bind(trigger_execution_id)
    .bind(execution_depth)
    .bind(flow_trigger_event_id)
    .bind(replayed_from_execution_id)
    .bind(id)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;

    // W9a spec §7.1: a FILE input is a URI readable from THIS execution --
    // checked now that the row exists (the readable set walks from it), in
    // its transaction. A refusal is the caller's: the start route answers it,
    // and a trigger's start is `scheduler::start::classify`'s `Refused`.
    if let Err(why) = crate::storage::inputs::check_file_inputs(
        conn,
        workspace_id,
        id,
        &flow,
        &inputs,
        &input_files,
    )
    .await?
    {
        return Err(Error::InvalidInput(why));
    }
    crate::storage::index_input_files(conn, workspace_id, id, &input_files).await?;

    let ctx = ExecutionCtx {
        id,
        workspace_id,
        flow_id,
        source: source.to_string(),
        plan,
        seq: 0,
        // A caller that starts an execution is the bottom of the stack.
        depth: 0,
        gate: Some(gate.clone()),
    };
    let empty = ExecutionState::new(&ctx.plan);
    let mut state = empty.clone();
    // Born FAILED: the caller's own command, applied before the first settle
    // exactly as `settle` applies an SLA's terminal before `drive` -- and for
    // the same effect: `advance()` returns nothing for a terminal execution, so
    // no run is created, the history is `[CREATED, FAILED]`, and the settle's
    // outbox hook, parent notification and every other terminal path see an
    // ordinary FAILED transition.
    if born_failed {
        apply(
            &mut state,
            &Command::SetExecutionState {
                state: RunState::Failed,
            },
        );
    }
    // `Tick(0)` is the execution's own origin — `Tick` is milliseconds since
    // the execution was created (`executor::state::Tick`), and this is that
    // instant.
    settle(conn, &ctx, &empty, state, Tick(0), None).await?;
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
///
/// W9c Task 9: in `txn::transaction!`, so a render that needs a read is
/// fetched with this transaction rolled back, and the settle runs again.
pub async fn resume(pool: &PgPool, execution_id: Uuid) -> Result<()> {
    transaction!(pool, |at| {
        let (conn, gate) = at.parts();
        let (ctx, state, now) = load(conn, execution_id, Stack::Fresh(gate.clone())).await?;
        let before = state.clone();
        settle(conn, &ctx, &before, state, now, None).await
    })
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
    /// The execution's logical clock, as [`Marks`] describes it: the last step
    /// a `settle` consumed. Carried on the context rather than read again in
    /// `persist` because the whole of one settle has to agree about it.
    pub seq: i64,
    /// **How many nested `settle`s deep this one is** — the recursion depth on
    /// the *stack*, not a property of the execution.
    ///
    /// # Why it is carried rather than derived, and it was derived first
    ///
    /// `spawn_one` drives a child's first settle **inside its caller's
    /// transaction**, so a chain of child executions is a chain of nested
    /// `settle` frames and depth in the data is depth on the stack. W6 Task 3's
    /// fix round bounded that with a recursive CTE over the link columns — and
    /// mutation **M9 showed the bound was worthless against the defect it was
    /// meant to backstop**: it shared its walk with the cycle guard, so
    /// breaking the walk broke both and the mutation aborted the test binary
    /// with SIGABRT instead of failing an assertion.
    ///
    /// This is the same fix one level down: a counter that is **not derived
    /// from anything that can be wrong**. It is `0` only where a fresh call
    /// stack begins — [`start_execution`], and every [`load_with`] whose caller
    /// passes [`Stack::Fresh`] — and it is the caller's plus one everywhere a
    /// frame nests inside another: the two spawners (`spawn::spawn_one` and
    /// `subflow::spawn_one`) **and the three re-entrant loads**
    /// ([`Stack::Nested`]).
    ///
    /// # The descent was right and the ASCENT was the hole, and it was reachable
    ///
    /// Fix round 1 shipped this counter with `load_with` writing `0`
    /// unconditionally, on a written-down claim that no caller that loads an
    /// execution is inside another `settle`. Two of them are, and they are the
    /// two that a *completing* chain goes through — so the counter went
    /// `0,1,2,0,1,2,…` up a chain that was climbing the stack the whole time,
    /// and **nine flows filed in one workspace aborted the worker process**.
    /// [`Stack`] is why that can no longer be written by omission.
    ///
    /// **Only `subflow::spawn_one` refuses on it, and the recorded reason for
    /// that used to be false.** It said *"a `Loop` cannot recurse without
    /// bound — its body is its own parent's `tasks:`, strictly smaller each
    /// time — so bounding the loop spawner would be a behaviour change to a
    /// construct that cannot reach it."* **It reaches it**: 12 statically
    /// nested one-value `Loop`s, with no `Subflow` anywhere, run to completion
    /// at depth 12 — half again past [`super::subflow::MAX_SPAWN_CHAIN`] — and
    /// 47 of them abort the process. The true reason the loop spawner does not
    /// refuse here is narrower and is not a safety argument at all:
    /// `build_plan` accepts a nesting up to `plan::MAX_TASK_DEPTH` (48), so
    /// refusing at 8 would refuse flows the planner accepts, and the artifact
    /// accepts far more than either (measured: 498). See
    /// [`super::subflow::MAX_SPAWN_CHAIN`]'s *"What neither bound closes"* for
    /// the whole measurement and who owns it. Loop links are counted here,
    /// because each one is a real frame.
    pub depth: usize,
    /// W9c Task 9: the gate of the attempt this execution's `settle` renders
    /// in, whose cache every render reads through (`storage::prefetch`) --
    /// the transaction OWNER's, handed down by [`Stack`] to every nested
    /// settle, so all the executions one transaction touches read through one
    /// cache and the owner's check before its commit sees every miss.
    /// `None` for a reader's context ([`Stack::Reader`]), which `settle`
    /// refuses.
    pub gate: Option<Gate>,
}

impl ExecutionCtx {
    /// What a load nested inside this execution's `settle` passes as
    /// [`Stack::Nested`]: this frame's depth, and the owner's cache.
    pub(super) fn caller(&self) -> Caller {
        Caller {
            depth: self.depth,
            gate: self.gate.clone(),
        }
    }
}

/// The frame a nested load sits inside: [`Stack::Nested`]'s payload.
#[derive(Debug, Clone)]
pub(super) struct Caller {
    /// The calling `settle`'s [`ExecutionCtx::depth`].
    pub depth: usize,
    /// The calling `settle`'s [`ExecutionCtx::gate`].
    pub gate: Option<Gate>,
}

/// Whether [`load_with`] takes the execution's row lock.
///
/// Every WRITER takes it -- it is the serialisation two workers finishing two
/// siblings of one `Parallel` need. The one reader that does not is
/// [`input_for`], which runs before the tick's transaction is opened and must
/// not hold a lock across a handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lock {
    Yes,
    No,
}

/// **Where on the CALL STACK the caller loading this execution already is** —
/// the one thing [`load_with`] cannot read off the row it reads.
///
/// # It is an enum, and a `usize` with a default is what it replaces
///
/// `load_with` used to write `depth: 0` under a comment asserting that every
/// caller that loads an execution *"is starting a fresh call stack"*. **Two of
/// them are not**: `claim::settle` step 4 calls
/// [`spawn::on_execution_terminal`](super::spawn::on_execution_terminal) and
/// [`subflow::on_child_terminal`](super::subflow::on_child_terminal), and
/// `settle` step 3 reaches `subflow::complete` through
/// `subflow::spawn_all`. Each re-enters `load_with` **inside the caller's own
/// frame**, so the counter was reset once per level of a completing chain and
/// [`super::subflow::MAX_SPAWN_CHAIN`] never fired: nine flows filed in one
/// workspace, plain `wait: true`, aborted the worker process with
/// `fatal runtime error: stack overflow`.
///
/// The fix is not a better default — it is **having no default**. A caller
/// must say which of the two it is, at the call site, and the two answers are
/// different *kinds* rather than two numbers, so [`Stack::Fresh`] cannot be
/// written by forgetting.
///
/// `tests/worker.rs`'s
/// `a_chain_of_subflows_that_COMPLETES_is_bounded_by_the_same_counter` is the
/// control: it is the ascent, where the reset lived, as distinct from the
/// descent the sibling test covers.
///
/// # It carries the gate too (W9c Task 9)
///
/// For the same reason: a nested settle's renders must read through the
/// transaction owner's cache, or the owner's check before its commit would
/// not see their misses. `Fresh` takes the attempt's [`Gate`] (which only
/// `worker::txn` makes) and `Nested` the caller's, so a load that settles
/// cannot be written without one; `Reader` is the load that never settles,
/// and `settle` refuses a context built by it.
#[derive(Debug, Clone)]
pub(super) enum Stack {
    /// **This caller begins a fresh call stack**, inside an attempt of
    /// `worker::txn`: the tick (`apply_outcome_with`) and [`resume`]. Each is
    /// entered from `worker::run`, with nothing of this crate's below it.
    Fresh(Gate),
    /// **This load happens inside a `settle` that is already `.0.depth`
    /// deep**, and the `settle` it is about to run nests inside that one — so
    /// the frame it produces is `.0.depth + 1`, reading through the same cache.
    Nested(Caller),
    /// **A reader**: `input_for` and `execution_render_context`, which build
    /// a render context and run no `settle`. A fresh stack, and no gate.
    Reader,
}

impl Stack {
    /// [`ExecutionCtx::depth`] for the context this load is building.
    fn depth(&self) -> usize {
        match self {
            Stack::Fresh(_) | Stack::Reader => 0,
            Stack::Nested(caller) => caller.depth + 1,
        }
    }

    /// [`ExecutionCtx::gate`] for the context this load is building.
    fn gate(&self) -> Option<Gate> {
        match self {
            Stack::Fresh(gate) => Some(gate.clone()),
            Stack::Nested(caller) => caller.gate.clone(),
            Stack::Reader => None,
        }
    }
}

/// Read the execution, **locking it**, and rebuild what `advance()` reads.
///
/// The lock is the serialisation two workers finishing two siblings of the
/// same `Parallel` need: without it both would read the same `ExecutionState`,
/// both would compute the same successor, and both would try to create it. The
/// `UNIQUE (execution_id, run_ix)` in migration 0134 is what happens if this
/// lock is ever lost — a collision rather than a duplicate run.
///
/// # `FOR NO KEY UPDATE`, not `FOR UPDATE` (W13a final review I2)
///
/// Every writer of an execution comes through here, so the lock only has to
/// exclude other writers, and `FOR NO KEY UPDATE` conflicts with itself exactly
/// as `FOR UPDATE` does: two loads of one execution still queue, and `resume`'s
/// lost-update argument (`runtime`'s statement 3) is unchanged. What the
/// weaker mode stops blocking is `FOR KEY SHARE` -- the lock a foreign-key
/// check takes on the row it references -- and nothing on this path changes a
/// key: `settle` writes `state`, `history`, `seq` and `updated_at`, never `id`
/// or `workspace_id`.
///
/// That matters to one reader. The Flow-trigger processor inserts an
/// execution whose `trigger_execution_id` references the SOURCE, at
/// `REPEATABLE READ`. Under `FOR UPDATE` a source that any worker settled
/// after the processor's snapshot made that check fail `40001` (the reviewer
/// measured it on Postgres 16), so a busy source's listeners failed and were
/// given up. Every other `KEY SHARE` taker -- a task run, a loop iteration, a
/// `Subflow` child, a replay, an outbox event -- is inserted by the writer that
/// already holds this row, in its own transaction, where no lock conflicts
/// with itself. So no serialisation anywhere relied on blocking one.
/// `tests/flow_trigger_fire.rs`'s
/// `a_source_settled_between_the_claim_and_the_fire_still_fires` is the
/// reproduction.
pub(super) async fn load(
    conn: &mut PgConnection,
    execution_id: Uuid,
    stack: Stack,
) -> Result<(ExecutionCtx, ExecutionState, Tick)> {
    load_with(conn, execution_id, Lock::Yes, stack).await
}

pub(super) async fn load_with(
    conn: &mut PgConnection,
    execution_id: Uuid,
    lock: Lock,
    stack: Stack,
) -> Result<(ExecutionCtx, ExecutionState, Tick)> {
    let row = sqlx::query(if lock == Lock::Yes {
        "SELECT source, history, workspace_id, flow_id, seq, \
                attempt_number, origin_offset_ms, \
                (EXTRACT(EPOCH FROM (now() - created_at)) * 1000)::bigint AS now_ms \
           FROM workflow.executions WHERE id = $1 FOR NO KEY UPDATE"
    } else {
        "SELECT source, history, workspace_id, flow_id, seq, \
                attempt_number, origin_offset_ms, \
                (EXTRACT(EPOCH FROM (now() - created_at)) * 1000)::bigint AS now_ms \
           FROM workflow.executions WHERE id = $1"
    })
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
        seq: row.get("seq"),
        // **The CALLER says, because only the caller knows.** Depth is a fact
        // about the stack and a row read off the database cannot carry it --
        // but "therefore it is zero" was the fix round 1 defect: two of the
        // callers here are re-entered from *inside* another `settle`
        // (`subflow::complete` and `spawn::on_execution_terminal`), so zeroing
        // it made the counter walk in place while the stack climbed. See
        // [`Stack`].
        depth: stack.depth(),
        gate: stack.gate(),
    };

    let rows = sqlx::query(
        "SELECT id, run_ix, task_ix, parent_ix, value, rendered, rendered_run_if, \
                history, attempts, not_before_ms, resume_with \
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
            // **The instant is NOT in this column** -- it is `not_before_ms`,
            // read one line up, so there is one clock rather than two that can
            // disagree. Migration 0141's `workflow_task_runs_resume_bound_
            // matches` is what makes the pairing structural; a row that
            // violates it cannot be written, so a `Some` discriminant with no
            // deadline here is storage corruption rather than a state.
            resume_with: parse_resume(
                id,
                r.get::<Option<String>, _>("resume_with").as_deref(),
                r.get::<Option<i64>, _>("not_before_ms").map(Tick),
            )?,
        });
    }

    let state = ExecutionState {
        runs,
        history: parse_states(execution_id, &history)?,
        // Kestra's `Execution.metadata` (migration 0139). `now` is deliberately
        // NOT shifted by the offset: every other `Tick` in this execution --
        // `TaskRun::not_before`, `Attempt::started_at` -- is measured from THIS
        // execution's `created_at`, and so is `oracle-exec/driver.rs`'s
        // `pass_time`, which computes a shortfall against the same column. A
        // clock shifted here would be read unshifted there and the driver would
        // travel a chain's whole accumulated time on every hop. The offset is
        // carried where its one reader is -- `retry::next_retry_at`'s
        // `CREATE_NEW_EXECUTION` origin -- as a negative tick.
        metadata: ExecutionMeta {
            attempt: u32::try_from(row.get::<i32, _>("attempt_number")).map_err(|_| {
                worker_err(WorkerError::Corrupt(
                    execution_id,
                    format!(
                        "negative attempt_number {}",
                        row.get::<i32, _>("attempt_number")
                    ),
                ))
            })?,
            origin: Tick(-row.get::<i64, _>("origin_offset_ms")),
        },
    };
    Ok((ctx, state, Tick(now_ms)))
}

/// **How many times this process has asked `advance()` to create a run while an
/// existing child of the same parent was still `CREATED`.**
///
/// A diagnostic, not a guard, and it exists because W3a *deleted* a guard on
/// exactly this state and named the caller that would bring it back. Kestra's
/// `resolveParallelNexts` returns nothing while any existing child run is still
/// `CREATED` — "first created, leave". W3a ported it, found that the mutation
/// bypassing it (`if !any_created || true`) left the whole crate green and the
/// ratchet unmoved, and deleted it with the reason written down: *"Deleting it
/// is right for today's only caller. It is not a property of `advance`… the
/// trigger to watch for is W4's worker, which is message-driven by
/// construction."* It measured the decision point at **0** occurrences under
/// the oracle's harness and at `valids/dag` 1 batch / `valids/if` 3 under a
/// driver that applies one worker result BETWEEN `advance()` calls.
///
/// This worker is that driver. The counter is how W3b answers the question with
/// a measurement rather than with a prediction — see
/// `tests/executor.rs`'s
/// `the_guard_w3a_deleted_did_not_come_back_and_the_reason_is_the_transaction`.
/// (That citation read `…_is_reachable_again_and_what_it_costs` until W4 Task
/// 6's fourth sweep pass: **no test has ever had that name.** A pointer into a
/// test file, from product source, that resolves to nothing — the shape
/// `oracle-exec/README.md` calls the worse one, in a surface its own table did
/// not cover.)
/// It is deliberately **not** a guard: `advance()` may not change (the W3b
/// plan's first Global Constraint), and the three entries the state occurs on
/// reproduce Kestra's run list exactly, so nothing observable is wrong.
pub static CREATE_BESIDE_A_CREATED_SIBLING: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// The happens-before endpoints one `settle` stamped, by run index.
///
/// **Why a logical clock rather than a timestamp.** The oracle compares a
/// relation over `(created, ended)`, and a `timestamptz` cannot carry it: every
/// row a `settle` writes shares one `now()`, because `now()` in Postgres is
/// TRANSACTION time. That is not a suspicion — it is the same shape W3b Task 4
/// measured on Kestra's side, where all 670 loop sub-executions carry their
/// parent execution's `startDate` and all 220 multi-sibling groups share one
/// instant, so the instant does not order siblings badly, it does not order
/// them at all.
///
/// **The left endpoint is stamped once and the right one is not**, and the
/// asymmetry is `normalise.py`'s rather than a choice: its `relation` reads
/// `state.startDate` and `state.endDate`, which for a RETRIED run are the first
/// creation and the LAST termination. So `created` is `or_insert` — a run that
/// re-enters `CREATED` on a retry must not look created after tasks it preceded
/// — and `ended` is a plain insert.
///
/// A first draft had both as `or_insert` plus a `COALESCE` in the UPDATE to
/// enforce it, and the mutation removing that guard killed **nothing** across
/// the whole crate: no compared entry's relation depends on which termination
/// of a retried run is taken. Rather than ship a guard nobody has seen fail,
/// the rule was changed to the one `normalise.py` actually uses.
#[derive(Debug, Default, Clone)]
pub(super) struct Marks {
    created: std::collections::BTreeMap<usize, i64>,
    ended: std::collections::BTreeMap<usize, i64>,
}

impl Marks {
    /// Record everything that changed between two snapshots as having happened
    /// at step `seq`.
    ///
    /// Diffing the state rather than reading the commands is deliberate: a
    /// `SkipRun` terminates a run just as a `SetRunState` does, and W3a shipped
    /// an arm-per-command version whose `SkipRun` arm nothing in the corpus
    /// could exercise. A diff has no arms to forget.
    pub(super) fn observe(&mut self, before: &ExecutionState, after: &ExecutionState, seq: i64) {
        for ix in before.runs.len()..after.runs.len() {
            self.created.entry(ix).or_insert(seq);
        }
        // **A run that goes from NO history to a `CREATED` was created now**,
        // and its left endpoint moves with it — W8 Task 4.
        //
        // An empty history is `Command::RestartLoop`'s spelling of Kestra's
        // "removed from `taskRunList`", and `Command::RecreateRun` is the walk
        // building it back — one child per round, in list order, which is what
        // `valids/waitfor-nested`'s `loop_1_return` **before** `loop_2` and
        // `loop_3_return` **before** `loop_3_log` measure. An iteration that
        // re-stamped every sibling at once made all three pairs `overlapping`.
        //
        // **A `plain` insert, not `or_insert`**, and the contrast with the loop
        // above is the whole rule: a retried run re-enters `CREATED` by
        // APPENDING and must keep its first endpoint (this struct's doc says
        // why — `normalise.py`'s `relation` reads a retried run's FIRST
        // `startDate`), while a re-created one has no earlier life to keep.
        // Nothing but `RestartLoop` empties a history.
        for (ix, run) in after.runs.iter().enumerate() {
            let was_removed = before.runs.get(ix).is_some_and(|b| b.history.is_empty());
            if was_removed && !run.history.is_empty() {
                self.created.insert(ix, seq);
            }
        }
        for (ix, run) in after.runs.iter().enumerate() {
            let was = before
                .runs
                .get(ix)
                .and_then(|b| b.history.last())
                .is_some_and(|s| s.is_terminal());
            let is = run.history.last().is_some_and(|s| s.is_terminal());
            if is && !was {
                self.ended.insert(ix, seq);
            }
        }
    }
}

/// Drive `advance()` and the render tier to a joint fixed point.
///
/// # Three things happen here, in this order, and the order is measured
///
/// 1. **`advance()`**, applying every command it asks for and **collecting**
///    the spawns. `Command::SpawnExecution` is the one command
///    `executor::apply` has no arm for, and that is not an omission:
///    `ExecutionState` holds nothing about a sub-execution, so there is nothing
///    for `apply` to record. It comes back to the caller instead.
/// 2. **`runIf:`**, for every run still `CREATED` whose task declares one.
/// 3. **an `If`/`Switch`/`Loop` expression**, at that kind's own render state.
///
/// Each is a step of its own, and that uniformity is a definition rather than a
/// guard: the mutation removing the increment from BOTH render passes killed
/// nothing across the whole crate, because a render neither creates nor
/// terminates a run and the round after it is `seq + 1` either way. It is kept
/// so the clock means "one step per pass that can change what `advance()`
/// sees", with the measurement recorded rather than a comment claiming a cost
/// nobody has seen.
///
/// 2 before 3 because Kestra decides a `runIf:`
/// before it resolves anything else about the task and the order is observable:
/// a `Loop` whose `runIf:` is falsy is `SKIPPED` without its `values:` ever
/// being resolved, so a tier that did both at once could fail a run on an
/// expression Kestra never evaluated.
///
/// **The render tier is inside this loop, not outside it**, because a render
/// unblocks `advance()` and `advance()` creates the runs the next render needs.
/// Task 2 shipped this loop without one and named the consequence: a flow with
/// a `runIf:`, an `If`, a `Switch` or a `Loop` got an empty command vector for
/// ever -- a stall, not an error.
///
/// Bounded by [`MAX_ROUNDS`]: a caller that does not converge has found a cycle
/// or a render that keeps re-arming, and both must be loud rather than spin
/// with a transaction open.
/// **The state a violated `ExecutionChangedSLA` puts this execution into**, or
/// `None` — `ExecutorService.handleExecutionChangedSLA` plus the state half of
/// `processViolation`, over [`crate::sla`]'s decision.
///
/// # Why it is here and not in `advance()`
///
/// `EXECUTION_ASSERTION`'s `assert:` is `runContext.render`ed
/// (`ExecutionAssertionSLA.evaluate`), and the corpus depends on that:
/// `valids/sla-execution-condition`'s is `{{inputs.string != 'CANCEL'}}`.
/// [`advance`](crate::executor::advance) is pure over `(plan, state, now)` and
/// has no renderer and no access to `inputs`, so evaluating an SLA there would
/// mean widening its contract. It does not need to be there: the decision is a
/// function of the flow and the render surface, both of which this `settle`
/// already holds, and Kestra takes it outside `process` too. **A change to
/// `advance()`'s contract is a finding, not a step**, and this is the finding
/// ruled on rather than taken.
///
/// # Kestra has FOUR guards, this has three, and the fourth is closed
/// elsewhere — W8 Task 6 fix round 1
///
/// `handleExecutionChangedSLA` returns early on
/// `flow == null || ListUtils.isEmpty(flow.getSla()) ||
/// executor.getExecution().getKind() == ExecutionKind.LOOP ||
/// executor.getExecution().getState().isTerminated()`. Three of the four are
/// here, **in Kestra's order**, and the fourth is not a guard in this crate at
/// all:
///
/// 1. **the flow parses and its `sla:` is non-empty** — `flow == null ||
///    isEmpty(getSla())`. Read off [`Surface::flow`], which `Surface::load`
///    already parsed: W8 Task 6 parsed `ctx.source` a SECOND time here, on
///    every settle of every execution, and 339 of the corpus's 345 flows paid
///    for a parse only to be told they have no `sla:`. An unparseable source
///    yields `None` rather than an error — this execution is already running
///    and `ctx.plan` was built from that same source, so failing it here would
///    be a new failure mode for every flow that declares no `sla:`.
/// 2. **the execution is not terminated** — what makes this idempotent across
///    the many `settle`s one execution has: the violation fires once,
///    terminates the execution, and every later call returns here.
/// 3. `violation.behavior()` — `NONE` changes no state
///    ([`crate::sla::state_for`]).
///
/// # The fourth guard is `ExecutionKind.LOOP`, and egret closes it at the source
///
/// Kestra runs a `Loop` iteration as a sub-execution **of the same flow**, so
/// the flow-level `sla:` is right there and the guard is what stops it being
/// evaluated once per iteration. egret does not: `worker::spawn::sub_flow_source`
/// synthesises a *different* flow from the loop body. **It did not clear
/// `sla:`**, so the parent's SLA was inherited by every iteration and this
/// function would have evaluated it there — an over-acceptance no corpus flow
/// reaches (none declares both an `sla:` and a `Loop`) and the ratchet cannot
/// see. Fix round 1 clears it beside `triggers:`, so the condition is
/// **unrepresentable** rather than guarded. See
/// `no_flow_level_sla_is_inherited_by_a_loop_sub_execution`.
///
/// **Fix round 2 corrects the reason this paragraph gave.** It said *"egret has
/// no `ExecutionKind`"*. It has one — [`crate::models::ExecutionKind`], with a
/// `Loop` variant, read by `trigger::flow` — and what is true is narrower:
/// **`workflow.executions` has no `kind` column.** The correction matters
/// because the false version implied no alternative existed, and one did: a
/// loop iteration's row carries a non-NULL `loop_index`, which
/// [`Surface::load`] already selects, so this function COULD have asked. The
/// clear at the source is a choice between two available sites and not a
/// last resort.
fn sla_terminal(surface: &Surface, ctx: &ExecutionCtx, state: &ExecutionState) -> Option<RunState> {
    let flow = surface.flow()?;
    if flow.sla.is_empty() {
        return None;
    }
    if state.history.last().is_some_and(|s| s.is_terminal()) {
        return None;
    }
    // `run: None` -- an execution-level render with no run of its own, which is
    // exactly what `Surface::context`'s `Option<RunIx>` was left open for.
    // Kestra builds it the same way: `runContextFactory.of(flow, execution)`,
    // no taskrun.
    let cx = surface.context(&ctx.plan, state, None);
    // `Err` is a render that will not resolve, and the artifact treats that as
    // a VIOLATION rather than as a skip -- see `crate::sla`'s module doc for
    // the bytecode, and for the assertion W8 Task 6 shipped saying the
    // opposite.
    let violation = crate::sla::first_execution_changed_violation(flow, &mut |p| {
        let rendered = p.render(&cx).map_err(|e| e.to_string());
        // W9c Task 9 (final review I2): an SLA condition belongs to no run,
        // so a per-call read limit it met has no run to carry its message,
        // and `workflow.executions` has no message column; it is logged.
        let limit = surface.take_limit_failure();
        if let (Err(_), Some(limit)) = (&rendered, limit) {
            tracing::warn!(
                execution = %ctx.id,
                "an SLA condition did not render, and counts as violated: {limit}"
            );
        }
        rendered
    })?;
    crate::sla::state_for(violation.behavior)
}

/// W9a Task 11: what a render produced beside the state (see `settle`): the
/// message of each run a render failed with one, and each file `Loop`'s
/// values, one row per record in file order (migration 0167) -- replacing
/// any rows a previous rendering of the same run left, so a run's values
/// are the ones its current `rendered` counts.
fn write_render_side(
    conn: &mut PgConnection,
    execution: Uuid,
    side: render::RenderSide,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + '_>> {
    Box::pin(async move {
        for (run, message) in side.failures {
            sqlx::query(
                "UPDATE workflow.task_runs SET message = $3 \
                  WHERE execution_id = $1 AND run_ix = $2",
            )
            .bind(execution)
            .bind(run as i32)
            .bind(message)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }
        for (run, values) in side.file_values {
            sqlx::query(
                "DELETE FROM workflow.loop_file_values WHERE execution_id = $1 AND run_ix = $2",
            )
            .bind(execution)
            .bind(run as i32)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
            sqlx::query(
                "INSERT INTO workflow.loop_file_values (execution_id, run_ix, idx, value) \
                 SELECT $1, $2, v.ord::int - 1, v.value \
                   FROM unnest($3::text[]) WITH ORDINALITY AS v(value, ord)",
            )
            .bind(execution)
            .bind(run as i32)
            .bind(&values)
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }
        Ok(())
    })
}

fn drive(
    ctx: &ExecutionCtx,
    surface: &Surface,
    state: &mut ExecutionState,
    now: Tick,
    marks: &mut Marks,
    seq: &mut i64,
    side: &mut render::RenderSide,
) -> Result<(Vec<Requested>, Vec<subflow::Requested>)> {
    let plan = &ctx.plan;
    let mut spawns = Vec::new();
    let mut subflows = Vec::new();
    for _ in 0..MAX_ROUNDS {
        let snapshot = state.clone();
        let cmds = advance(plan, state, now);
        if !cmds.is_empty() {
            note_create_beside_a_created_sibling(state, &cmds);
            *seq += 1;
            for cmd in &cmds {
                spawns.extend(Requested::of(cmd));
                subflows.extend(subflow::Requested::of(cmd));
                apply(state, cmd);
            }
            marks.observe(&snapshot, state, *seq);
            continue;
        }

        // W9c Task 9: a render that missed rendered a placeholder and carried
        // on (the user's ruling of 2026-10-08, "gather"): this attempt goes on
        // driving, so every read it can reach is recorded, and the owner's
        // gate rolls it back before any of it commits.
        let snapshot = state.clone();
        if render::render_run_if(surface, plan, state, side) == Pass::Rendered {
            *seq += 1;
            marks.observe(&snapshot, state, *seq);
            continue;
        }

        let snapshot = state.clone();
        if render::render_expressions(surface, plan, state, now, side) == Pass::Rendered {
            *seq += 1;
            marks.observe(&snapshot, state, *seq);
            continue;
        }

        return Ok((spawns, subflows));
    }
    Err(worker_err(WorkerError::NoFixedPoint(ctx.id)))
}

/// The `Err` the test-only caller-owned wrappers ([`apply_outcome`],
/// [`start_execution_in`]) answer when a render needed a read.
#[cfg(any(test, feature = "test-util"))]
fn prefetch_miss() -> Error {
    worker_err(WorkerError::PrefetchMiss)
}

/// The decision point [`CREATE_BESIDE_A_CREATED_SIBLING`] counts: this round
/// creates a child of some parent, and a child of that same parent already
/// exists and is still `CREATED`.
fn note_create_beside_a_created_sibling(state: &ExecutionState, cmds: &[Command]) {
    let created_parents: Vec<Option<RunIx>> = state
        .runs
        .iter()
        .filter(|r| r.history.last() == Some(&RunState::Created))
        .map(|r| r.parent)
        .collect();
    let hits = cmds
        .iter()
        .filter(|c| match c {
            Command::CreateRun { parent, .. } => created_parents.contains(parent),
            _ => false,
        })
        .count();
    if hits > 0 {
        CREATE_BESIDE_A_CREATED_SIBLING.fetch_add(hits, std::sync::atomic::Ordering::Relaxed);
    }
}

/// `persist`'s write of an EXISTING run, `$1`..`$15` as bound there.
/// `$14`/`$15` are [`stamps::Entered`]'s two booleans.
///
/// A `const` rather than inline for one reader besides `persist`:
/// `tests/execution_timestamps.rs` drives this exact text against a terminal
/// row to show that a later write does not move its `ended_at`. It is
/// exported under `test-util`.
pub const RUN_STATE_UPDATE: &str = concat!(
    "UPDATE workflow.task_runs \
        SET state = $1, history = $2, attempts = $3, rendered = $4, \
            rendered_run_if = $5, not_before_ms = $6, \
            claimed_at = CASE WHEN $7 THEN NULL ELSE claimed_at END, \
            ended_seq = $10, \
            created_seq = COALESCE($12, created_seq), \
            resume_with = $13, ",
    stamp_set!("$1", "$2", "$14", "$15"),
    ", updated_at = now() \
      WHERE execution_id = $8 AND run_ix = $9 \
        AND ($11::timestamptz IS NULL OR claimed_at = $11)",
);

/// `persist`'s write of the execution's state: `$1` the state, `$2` the
/// history, `$3` the id, and `$4`/`$5` [`stamps::Entered`]'s booleans.
/// Exported under `test-util` for [`RUN_STATE_UPDATE`]'s reason.
pub const EXECUTION_STATE_UPDATE: &str = concat!(
    "UPDATE workflow.executions SET state = $1, history = $2, ",
    stamp_set!("$1", "$2", "$4", "$5"),
    ", updated_at = now() WHERE id = $3",
);

/// Write the difference between `before` and `after`.
///
/// `release` names the run whose lease this write clears — `None` at
/// [`start_execution`], where nothing is leased. Only that run's lease is
/// touched: another worker may hold a lease on a sibling of the same execution,
/// and clearing it would hand that sibling to a second handler.
///
/// **The released run's UPDATE is fenced on that lease and every other run's is
/// not**, which is the narrowest form of the rule: the only row this writer has
/// a token for is the one it claimed. A sibling's row is written under the
/// execution's `FOR NO KEY UPDATE` lock, which is what serialises two workers there;
/// fencing it on a token this writer does not hold would refuse every legitimate
/// successor write.
///
/// **Measured: this fence and [`apply_outcome`]'s are ALTERNATIVES, and each is
/// sufficient on its own.** Four mutations against
/// `a_worker_whose_lease_was_reaped_cannot_commit`: making both `rows_affected`
/// checks unread FIRES (`["CREATED","RUNNING","FAILED"]` where the test wants
/// `["CREATED","RUNNING"]`); deleting both `claimed_at` terms FIRES (`A's
/// FAILED must not overwrite B's terminal`); deleting either term ALONE leaves
/// the test green, because the surviving one aborts the same transaction. So
/// neither is individually necessary today, and that is written down rather
/// than left for the next reader to discover by deleting one and seeing green.
/// Both are kept because they answer different questions — "may this worker
/// write this run's output" and "may it write this run's state" — and W4's
/// worker pool is the caller that reaches `settle` with a lease and no
/// `outputs` write in front of it.
#[allow(clippy::too_many_arguments)]
async fn persist(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    before: &ExecutionState,
    after: &ExecutionState,
    release: Option<Lease>,
    marks: &Marks,
    seq: i64,
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
            let lease = release.filter(|l| l.run_ix == ix as i32);
            if before.runs[ix] == *run && lease.is_none() {
                continue;
            }
            // The fence, on the one row this writer holds a token for. `$11`
            // is NULL for every other run, and `claimed_at IS NOT DISTINCT
            // FROM NULL` would be the wrong reading of that — so the term is
            // written as "`$11` is null, OR it equals the row's lease".
            // An EXISTING run in `marks.created` was emptied and built back in
            // this settle (`Marks::observe`'s second loop). See `stamps`.
            let entered = stamps::Entered::of(
                &before.runs[ix].history,
                &run.history,
                marks.created.contains_key(&ix),
            );
            let hit = sqlx::query(RUN_STATE_UPDATE)
                .bind(state)
                .bind(&history)
                .bind(&attempts)
                .bind(run.rendered.as_deref())
                .bind(run.rendered_run_if.as_deref())
                .bind(run.not_before.map(|t| t.0))
                .bind(lease.is_some())
                .bind(execution_id)
                .bind(ix as i32)
                // **No `COALESCE`, and that is a measurement.** Two guards were
                // written here and neither could be seen to fail: keeping the FIRST
                // termination (`COALESCE(ended_seq, $10)`) killed nothing across
                // the whole crate, and so did keeping any EXISTING value
                // (`COALESCE($10, ended_seq)`). The second is not merely unkillable
                // but wrong: `retry::reopen_due` puts a terminated run back to
                // `CREATED`, and a run that is running again must not keep a right
                // endpoint. So the plain assignment is both the simplest and the
                // one `normalise.py` describes -- its `relation` reads
                // `state.endDate`, the run's LAST termination.
                .bind(marks.ended.get(&ix).copied())
                .bind(lease.map(|l| l.claimed_at))
                // **`COALESCE` here and NOT on `ended_seq` above, and the asymmetry
                // is the two fields' rules rather than caution** — W8 Task 4.
                // `Marks` is built per `settle`, so it holds a `created` entry only
                // for a run this settle created or RESET; every other run must keep
                // the column it already has, which a bare `$12` would null. The
                // right endpoint has no such case: `marks.ended` is `None` exactly
                // when nothing terminated, and the plain assignment is what
                // `retry::reopen_due` needs (a run that is running again must not
                // keep a right endpoint).
                .bind(marks.created.get(&ix).copied())
                // A plain assignment, like `ended_seq` and unlike `created_seq`:
                // the state's value IS the answer, including when it goes back to
                // `NULL` because `Command::RestartLoop` took the run out of
                // `PAUSED`. A `COALESCE` here would leave a stale discriminant
                // beside a NULL deadline, which migration 0141 refuses.
                .bind(run.resume_with.as_ref().map(Resume::wire))
                .bind(entered.running)
                .bind(entered.terminal)
                .execute(&mut *conn)
                .await
                .map_err(db_err)?;
            // Same rule as the `outputs` write: 0 rows affected and 1 are the
            // same value to `execute()`, so the fence has to be read.
            if let Some(l) = lease {
                if hit.rows_affected() == 0 {
                    return Err(worker_err(WorkerError::LeaseLost(l.run_id)));
                }
            }
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
            sqlx::query(concat!(
                "INSERT INTO workflow.task_runs \
                   (execution_id, run_ix, task_ix, task_id, type_id, parent_ix, value, \
                    rendered, rendered_run_if, state, history, attempts, not_before_ms, \
                    created_seq, ended_seq, resume_with, ",
                stamp_columns!(),
                ") VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16, ",
                stamp_values!("$10", "$11"),
                ")",
            ))
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
            .bind(marks.created.get(&ix).copied())
            .bind(marks.ended.get(&ix).copied())
            // Always `NULL` on an INSERT today -- `Command::CreateRun` cannot
            // pause a run it is creating -- and bound anyway, because a column
            // this writer never mentions is a column the next author has to
            // discover.
            .bind(run.resume_with.as_ref().map(Resume::wire))
            .execute(&mut *conn)
            .await
            .map_err(db_err)?;
        }
    }

    // The clock, written once per `settle` whatever else moved. It is NOT
    // conditional on `before.history != after.history`: a settle that only
    // filled a render slot still consumed steps, and a clock that stood still
    // there would let the NEXT settle stamp an endpoint at a step already used.
    sqlx::query("UPDATE workflow.executions SET seq = $1 WHERE id = $2")
        .bind(seq)
        .bind(execution_id)
        .execute(&mut *conn)
        .await
        .map_err(db_err)?;

    if before.history != after.history {
        let entered = stamps::Entered::of(&before.history, &after.history, false);
        sqlx::query(EXECUTION_STATE_UPDATE)
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
            .bind(entered.running)
            .bind(entered.terminal)
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
                // `ended_at` only once there is one: an attempt written before
                // migration 0180, or one still open, has no key rather than a
                // `null` that reads like a measurement.
                let mut o = serde_json::json!({
                    "history": states_json(&a.history),
                    "started_at": a.started_at.0,
                });
                if let Some(end) = a.ended_at {
                    o["ended_at"] = serde_json::Value::from(end.0);
                }
                o
            })
            .collect(),
    )
}

/// [`Resume`] from migration 0141's discriminant and the run's own deadline.
///
/// The two halves are checked against each other rather than trusted: 0141's
/// CHECK pairs them at write time, so a mismatch read back here is the storage
/// being wrong -- which is `WorkerError::Corrupt`'s whole class, and the one
/// class `oracle-exec/driver.rs` reports as `Broken` rather than swallowing as
/// an ordinary divergence.
fn parse_resume(owner: Uuid, wire: Option<&str>, at: Option<Tick>) -> Result<Option<Resume>> {
    let corrupt = |what: String| worker_err(WorkerError::Corrupt(owner, what));
    Ok(match (wire, at) {
        (None, _) => None,
        (Some("NEVER"), None) => Some(Resume::Never),
        (Some("ITERATE"), Some(at)) => Some(Resume::Iterate { at }),
        (Some(s), Some(at)) if s.starts_with("TERMINAL:") => Some(Resume::Terminal {
            at,
            state: from_wire(&s["TERMINAL:".len()..])
                .ok_or_else(|| worker_err(WorkerError::UnknownState(owner, s.to_string())))?,
        }),
        (Some(s), at) => {
            return Err(corrupt(format!(
                "resume_with {s:?} with not_before_ms {:?}: migration 0141 pairs the \
                 discriminant with the deadline and this row has neither pairing",
                at.map(|t| t.0)
            )))
        }
    })
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
            // Absent on an open attempt and on one written before
            // migration 0180; a present key that is not an integer is
            // corruption, like a missing `started_at`.
            let ended_at = match a.get("ended_at") {
                None | Some(serde_json::Value::Null) => None,
                Some(t) => Some(Tick(t.as_i64().ok_or_else(|| {
                    worker_err(WorkerError::Corrupt(
                        owner,
                        format!("attempt {a} has a non-integer ended_at"),
                    ))
                })?)),
            };
            Ok(Attempt {
                history,
                started_at: Tick(started_at),
                ended_at,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{Attempt as _Attempt, TaskRun};

    /// **The positive control for [`CREATE_BESIDE_A_CREATED_SIBLING`].**
    ///
    /// The corpus measures that counter at **zero**, and a counter that is zero
    /// because nothing can ever increment it says nothing at all — which is the
    /// exact shape W3a's deleted guard had (a guard nobody has seen fail is a
    /// guard nobody has seen). This drives the decision point directly and
    /// asserts the counter moves, so `tests/executor.rs`'s corpus-wide zero is
    /// a measurement rather than a broken instrument.
    #[test]
    fn the_create_beside_a_created_sibling_counter_can_fire() {
        let run = |parent: Option<usize>, state: RunState| TaskRun {
            task: 0,
            parent,
            value: None,
            rendered: None,
            rendered_run_if: None,
            history: vec![state],
            attempts: Vec::<_Attempt>::new(),
            not_before: None,
            resume_with: None,
        };
        let state = ExecutionState {
            runs: vec![
                run(None, RunState::Running),
                run(Some(0), RunState::Created),
            ],
            history: vec![RunState::Running],
            metadata: crate::executor::ExecutionMeta::default(),
        };
        let before = CREATE_BESIDE_A_CREATED_SIBLING.load(std::sync::atomic::Ordering::Relaxed);
        // A sibling of run 1 being created while run 1 is still CREATED: the
        // state Kestra's `resolveParallelNexts` refuses to be in.
        note_create_beside_a_created_sibling(
            &state,
            &[Command::CreateRun {
                task: 1,
                parent: Some(0),
                value: None,
            }],
        );
        let after = CREATE_BESIDE_A_CREATED_SIBLING.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(after, before + 1, "the decision point must be countable");

        // And the negative half, on the SAME state: creating a child of a
        // DIFFERENT parent is not the decision point, so the counter must not
        // move. Without this the counter would pass by counting everything.
        note_create_beside_a_created_sibling(
            &state,
            &[Command::CreateRun {
                task: 2,
                parent: Some(1),
                value: None,
            }],
        );
        assert_eq!(
            CREATE_BESIDE_A_CREATED_SIBLING.load(std::sync::atomic::Ordering::Relaxed),
            after,
            "a child of a parent with no CREATED child is not the decision point"
        );
    }

    // ---- the self-fence (W5a Task 5 fix round 1) ------------------------------

    /// Work that never finishes on its own and records whether it was dropped.
    struct Dropped(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    const EVERY: std::time::Duration = std::time::Duration::from_millis(60);
    const FENCE: std::time::Duration = std::time::Duration::from_millis(200);

    fn now_utc() -> DateTime<Utc> {
        Utc::now()
    }

    /// **A worker cut off from Postgres gives the run up once no renewal has
    /// succeeded for `working_timeout`** -- exactly as on a moved fence: the
    /// handler future is DROPPED (so the budget's `cancel`, inside it, never
    /// runs) and `Lost` is returned.
    /// Every renewal here errors, as against an unreachable database.
    #[tokio::test]
    async fn a_worker_whose_renewals_keep_failing_self_fences_and_drops_the_handler() {
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let guard = Dropped(dropped.clone());
        let work = async move {
            let _g = guard;
            std::future::pending::<()>().await
        };
        let n = calls.clone();
        let started = tokio::time::Instant::now();
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            under_lease_with(
                move |_held| {
                    n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async { Err(Error::Internal(anyhow::anyhow!("database unreachable"))) }
                },
                Uuid::nil(),
                now_utc(),
                EVERY,
                FENCE,
                started,
                work,
            ),
        )
        .await
        .expect("a worker that cannot renew must NOT keep following the task for ever");
        assert!(matches!(out, Leased::Lost));
        assert!(
            started.elapsed() >= FENCE,
            "not before the deadline: {:?}",
            started.elapsed()
        );
        assert!(
            dropped.load(std::sync::atomic::Ordering::SeqCst),
            "the handler was dropped"
        );
        assert!(
            calls.load(std::sync::atomic::Ordering::SeqCst) >= 2,
            "it did retry first"
        );
    }

    /// A renewal that never ANSWERS (a partition that hangs the connection)
    /// self-fences too; the work is not polled on past the deadline.
    #[tokio::test]
    async fn a_renewal_that_never_answers_self_fences() {
        let started = tokio::time::Instant::now();
        let out = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            under_lease_with(
                |_held| std::future::pending::<Result<Option<DateTime<Utc>>>>(),
                Uuid::nil(),
                now_utc(),
                EVERY,
                FENCE,
                started,
                std::future::pending::<()>(),
            ),
        )
        .await
        .expect("a hung renewal must not hold the worker in the handler for ever");
        assert!(matches!(out, Leased::Lost));
    }

    /// **The controls.** Renewals that succeed keep a handler running well
    /// past `working_timeout`; errors that recover before the deadline do not
    /// fence; and a successful renewal keeps a run whose deadline had already
    /// passed at entry (it waited behind its batch) -- success is proof.
    #[tokio::test]
    async fn succeeding_renewals_keep_the_run_past_the_deadline() {
        let ok = |_held: DateTime<Utc>| async { Ok(Some(Utc::now())) };
        let out = under_lease_with(
            ok,
            Uuid::nil(),
            now_utc(),
            EVERY,
            FENCE,
            tokio::time::Instant::now(),
            async {
                tokio::time::sleep(FENCE * 3).await;
                7
            },
        )
        .await;
        assert!(matches!(out, Leased::Kept { out: 7, .. }));

        let n = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let m = n.clone();
        let flaky = move |_held: DateTime<Utc>| {
            let i = m.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                if i % 2 == 1 {
                    Err(Error::Internal(anyhow::anyhow!("blip")))
                } else {
                    Ok(Some(Utc::now()))
                }
            }
        };
        let out = under_lease_with(
            flaky,
            Uuid::nil(),
            now_utc(),
            EVERY,
            FENCE,
            tokio::time::Instant::now(),
            async {
                tokio::time::sleep(FENCE * 3).await;
                8
            },
        )
        .await;
        assert!(
            matches!(out, Leased::Kept { out: 8, .. }),
            "one error in two is not a fence"
        );

        let late = tokio::time::Instant::now() - FENCE * 2;
        let out = under_lease_with(ok, Uuid::nil(), now_utc(), EVERY, FENCE, late, async {
            tokio::time::sleep(EVERY).await;
            9
        })
        .await;
        assert!(matches!(out, Leased::Kept { out: 9, .. }));
    }
}
