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

use egret_core::{Error, Result};
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use super::cancel;
use super::render::{self, Pass, Surface};
use super::replay;
use super::spawn::{self, Requested};
use super::subflow;
use super::timeout::{self, Budget};
use super::{from_wire, Outcome, Registry, TaskInput, WorkerError, MAX_ROUNDS};
use crate::executor::{
    advance, apply, build_plan, Attempt, Command, ExecutionMeta, ExecutionState, Plan, Resume,
    RunIx, RunState, TaskKind, TaskRun, Tick,
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
    fn input(&self, render: crate::runners::RenderContext) -> Result<(TaskInput, Budget)> {
        let flow = crate::spec::parse(&self.source)
            .map_err(|e| worker_err(WorkerError::Unplannable(self.execution_id, e.message)))?;
        let leaf = leaf(&flow, &self.task_id).ok_or_else(|| {
            worker_err(WorkerError::NotALeaf(
                self.execution_id,
                self.task_id.clone(),
            ))
        })?;
        let budget = timeout::resolve(leaf.timeout, &render);
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
                // **The lease token, handed to the handler.** See
                // `TaskInput::claimed_at`: a handler that holds an external
                // resource names it after this and not after `run_id` alone,
                // so a reaped worker cannot reach its replacement's resource.
                claimed_at: self.claimed_at,
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

/// The executions holding a run whose retry delay has elapsed — a `RETRYING`
/// one waiting to be reopened, or a `RETRIED` one whose **replay** has not been
/// started yet.
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
/// due is a whole new execution, which [`super::replay::spawn_due`] starts from
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
/// **The clock is [`load_with`]'s, spelled the same way**, so an execution
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
/// # It will never wake a `PAUSED` run, and the ratchet cannot tell you
///
/// **W8 Task 2, and W8 Task 3 made the hypothetical real.** The gate below is
/// `r.state = 'RETRYING' OR (r.state = 'RETRIED' AND …)`. A bounded `Pause`
/// parks a run with a `not_before_ms` and a state of `'PAUSED'`, which matches
/// neither — so in the **product** a bounded pause never resumes, however
/// correct `advance()` is. That was a prediction about a construct nothing
/// emitted; since W8 Task 3 `flowable::pause` emits it, eight golden entries
/// depend on the wait ending, and every one of them ends it through
/// `driver::pass_time` rather than through this query. **The defect is live and
/// it is Task 7's**, per the package's own assignment.
///
/// **This is the half the oracle is blind to, and it is blind for a reason that
/// is a property of the instrument.** `oracle-exec/driver.rs`'s `pass_time`
/// selects on `not_before_ms` alone, with no state term anywhere in its SQL,
/// and then calls `worker::resume` directly. It never runs this query. So the
/// driver reaches the same runs through a different door and a green ratchet
/// says nothing about this predicate at all — the shape W4's item 15 named, a
/// property proved about a path no deployment takes.
///
/// `executor::retry::reopen_due` is the pure-side twin and its blindness is the
/// **opposite**: the driver does reach it, so it fails loudly. Recorded on both
/// because one sentence about "the ratchet cannot see it" is wrong about one of
/// them.
pub(super) async fn due_retries(exec: impl sqlx::PgExecutor<'_>, limit: i64) -> Result<Vec<Uuid>> {
    sqlx::query_scalar(
        "SELECT r.execution_id \
           FROM workflow.task_runs r \
           JOIN workflow.executions e ON e.id = r.execution_id \
          WHERE (r.state = 'RETRYING' \
                 OR (r.state = 'RETRIED' \
                     AND e.parent_run_ix IS NULL \
                     AND NOT EXISTS (SELECT 1 FROM workflow.executions c \
                                      WHERE c.parent_execution_id = e.id \
                                        AND c.parent_run_ix IS NULL))) \
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
/// **That sentence said "a production caller", and W4 Task 6 measured that it
/// is not one yet.** [`tick`] itself has no production caller: a grep for
/// `tick(`, `reap(` and `start_execution(` over every `crates/*/src` finds no
/// call site at all, and `egret-bin` builds a `WorkflowState` — the flow repo
/// and its refs — with no claim loop. So the chain this doc describes is real
/// and ends one link short of a deployment, and this is exactly the sentence a
/// reader would quote as evidence the waker is wired. It is wired to
/// **`tick`**; nothing is wired to `tick`. `oracle-exec/README.md`'s coverage
/// statement carries it as item 15, and W4's own section as its sixth fact.
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
/// **2. Pick `n` deliberately.** `n = 1` makes the property above unreachable
/// and is what `oracle-exec/driver.rs` uses.
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
///    proposal and why it was not taken in a bookkeeping round.
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
    let mut first_err: Option<Error> = None;
    // Ahead of the claim, and its failure does not stop the claim: an
    // execution that cannot settle must not keep this worker from running the
    // runs that are already claimable.
    if let Err(e) = wake(pool, n as i64).await {
        first_err = Some(e);
    }
    let claimed = claim(pool, n as i64).await?;
    let mut done = 0usize;
    for c in &claimed {
        match run_one(pool, c, reg).await {
            Ok(()) => done += 1,
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
async fn run_one(pool: &PgPool, c: &Claimed, reg: &dyn Registry) -> Result<()> {
    let (input, budget) = input_for(pool, c).await?;
    let outcome = match budget {
        Budget::Refused(message) => Outcome::failed_with(message),
        // `cancel::guarded` rather than `timeout::guard` directly: the budget's
        // expiry is the artifact's only `kill(null)` site, so the kill path
        // hangs off it. A refusal there does not reach this expression — it is
        // logged and the budget's outcome stands. See `super::cancel`.
        other => cancel::guarded(reg, &input, other.duration()).await?,
    };
    let mut tx = pool.begin().await.map_err(db_err)?;
    apply_outcome(&mut tx, c, &outcome).await?;
    tx.commit().await.map_err(db_err)?;
    Ok(())
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
async fn input_for(pool: &PgPool, c: &Claimed) -> Result<(TaskInput, Budget)> {
    let mut conn = pool.acquire().await.map_err(db_err)?;
    let (ctx, state, _now) = load_with(&mut conn, c.execution_id, Lock::No, Stack::Fresh).await?;
    let surface = Surface::load(&mut conn, &ctx).await?;
    let run = usize::try_from(c.run_ix).map_err(|_| {
        worker_err(WorkerError::Corrupt(
            c.id,
            format!("negative run_ix {}", c.run_ix),
        ))
    })?;
    c.input(surface.context(&ctx.plan, &state, Some(run)))
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
    let (ctx, mut state, now) = load(conn, claimed.execution_id, Stack::Fresh).await?;
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
        &mut state,
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
pub(super) async fn settle(
    conn: &mut PgConnection,
    ctx: &ExecutionCtx,
    before: &ExecutionState,
    state: &mut ExecutionState,
    now: Tick,
    release: Option<Lease>,
) -> Result<()> {
    let surface = Surface::load(conn, ctx).await?;
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
    let (spawns, subflows) = drive(ctx, &surface, state, now, &mut marks, &mut seq)?;
    persist(conn, ctx, before, state, release, &marks, seq).await?;
    spawn::spawn_all(conn, ctx, state, &spawns).await?;
    // **After `persist`, exactly like the loop spawner, and for the same
    // reason**: the child's row names `(execution, run_ix)` and that run has to
    // exist. It differs in one way that matters for what comes next --
    // `subflow::spawn_one` may complete the parent's own run (a `wait: false`
    // subflow, or a target that does not resolve), and it does so through a
    // nested `settle` on this same execution. That is the same re-entrancy
    // `spawn::spawn_all` already has via `on_execution_terminal`, and it is why
    // the terminal check below reads `state` -- this transaction's in-memory
    // copy, which a nested settle deliberately does not update -- rather than
    // the row: a nested settle that terminated the execution has ALREADY told
    // its parent, and firing again from here would report one terminal twice.
    subflow::spawn_all(conn, ctx, &surface, state, &subflows).await?;
    // **The execution-delay loop, and it is a different loop in the artifact
    // too.** A `CREATE_NEW_EXECUTION` retry ends THIS execution `RETRIED` and
    // leaves an `ExecutionDelay` behind (`retry::scan`'s `ScheduleRetry`); the
    // replacement execution is started later, by whoever notices the deadline.
    // Here that is every `settle`, because `resume` -- from `worker::wake` in
    // production and from `pass_time` in the oracle's driver -- is the settle a
    // due deadline produces. It reads `state`, so the round that scheduled the
    // delay can see it too, and refuses on `not_before > now`.
    replay::spawn_due(conn, ctx, state, now).await?;

    let was = before.history.last().copied();
    let is = state.history.last().copied();
    if was != is {
        if let Some(terminal) = is.filter(|s| notifies_parent(*s)) {
            spawn::on_execution_terminal(conn, ctx.id, terminal, ctx.depth).await?;
            // The other kind of link. `on_execution_terminal` reads
            // `parent_execution_id` and this reads `trigger_execution_id`, and
            // `workflow_executions_one_link_kind` makes it impossible for both
            // to find something -- so the pair is a dispatch, not two chances
            // to report the same terminal.
            //
            // **For a `CREATE_NEW_EXECUTION` replay NEITHER finds anything**,
            // and that is why egret's answer for a replayed `Subflow` child is
            // invariant rather than timing-dependent: `trigger_of` zips two
            // NULL trigger columns, and `parent_of` zips a set
            // `parent_execution_id` with a NULL `parent_run_ix`, which
            // `workflow_executions_loop_run_whole_or_absent` keeps NULL for a
            // row with no loop index. Registered as a divergence in
            // `oracle-exec/README.md`'s coverage statement, item 16.
            subflow::on_child_terminal(conn, ctx.id, terminal, ctx.depth).await?;
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
    inputs: serde_json::Value,
) -> Result<Uuid> {
    start_execution_with_trigger(pool, workspace_id, flow_id, source, inputs, None).await
}

/// [`start_execution`] with Kestra's `Execution.trigger` supplied.
///
/// A **sibling** rather than a sixth parameter on `start_execution`, because
/// that function has **65** call sites across `crates/` and all but one of them
/// have no trigger: widening it would mean writing `None` sixty-four times to
/// serve one caller, and sixty-four uniform `None`s are the kind of argument
/// that hides a mutation (W6 Task 6's `max_depth` survived a two-sided mutation
/// for exactly that reason -- every fixture passed the default).
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
    let flow = crate::spec::parse(source)
        .map_err(|e| Error::InvalidInput(format!("the flow does not parse: {}", e.message)))?;
    let plan = build_plan(source)
        .map_err(|e| Error::InvalidInput(format!("the flow does not plan: {e}")))?;

    let mut tx = pool.begin().await.map_err(db_err)?;
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO workflow.executions \
           (workspace_id, flow_id, namespace, flow_key, source, state, history, inputs, \
            execution_trigger) \
         VALUES ($1, $2, $3, $4, $5, 'CREATED', '[\"CREATED\"]'::jsonb, $6, $7) RETURNING id",
    )
    .bind(workspace_id)
    .bind(flow_id)
    .bind(&flow.namespace)
    .bind(&flow.id)
    .bind(source)
    .bind(if inputs.is_object() {
        inputs
    } else {
        serde_json::json!({})
    })
    .bind(trigger)
    .fetch_one(&mut *tx)
    .await
    .map_err(db_err)?;

    let ctx = ExecutionCtx {
        id,
        workspace_id,
        flow_id,
        source: source.to_string(),
        plan,
        seq: 0,
        // A caller that starts an execution is the bottom of the stack.
        depth: 0,
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
    let (ctx, mut state, now) = load(&mut tx, execution_id, Stack::Fresh).await?;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stack {
    /// **This caller begins a fresh call stack.** The tick (`apply_outcome`),
    /// [`resume`], and `input_for`, which runs before the tick's transaction
    /// is even opened. Each is entered from `worker::run`, with nothing of
    /// this crate's below it.
    Fresh,
    /// **This load happens inside a `settle` that is already `.0` deep**, and
    /// the `settle` it is about to run nests inside that one — so the frame it
    /// produces is `.0 + 1`.
    Nested(usize),
}

impl Stack {
    /// [`ExecutionCtx::depth`] for the context this load is building.
    fn depth(self) -> usize {
        match self {
            Stack::Fresh => 0,
            Stack::Nested(caller) => caller + 1,
        }
    }
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
           FROM workflow.executions WHERE id = $1 FOR UPDATE"
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
/// # The three guards, each of them Kestra's
///
/// 1. `executor.getExecution().getState().isTerminated()` — an execution that
///    has already finished is not re-judged. It is what makes this idempotent
///    across the many `settle`s one execution has: the violation fires once,
///    terminates the execution, and every later call returns here.
/// 2. `ListUtils.isEmpty(flow.getSla())` — no `sla:`, no work and no parse of
///    a render context. 339 of the corpus's 345 flows take this arm.
/// 3. `violation.behavior()` — `NONE` changes no state
///    ([`crate::sla::state_for`]).
///
/// A source that no longer parses yields `None` rather than an error: this
/// execution is already running, `ctx.plan` was built from that same source,
/// and turning an SLA check into the thing that fails a running execution would
/// be a new failure mode for every flow that has no `sla:` at all.
fn sla_terminal(surface: &Surface, ctx: &ExecutionCtx, state: &ExecutionState) -> Option<RunState> {
    if state.history.last().is_some_and(|s| s.is_terminal()) {
        return None;
    }
    let flow = crate::spec::parse(&ctx.source).ok()?;
    if flow.sla.is_empty() {
        return None;
    }
    // `run: None` -- an execution-level render with no run of its own, which is
    // exactly what `Surface::context`'s `Option<RunIx>` was left open for.
    // Kestra builds it the same way: `runContextFactory.of(flow, execution)`,
    // no taskrun.
    let cx = surface.context(&ctx.plan, state, None);
    let violation =
        crate::sla::first_execution_changed_violation(&flow, &mut |p| p.render(&cx).ok())?;
    crate::sla::state_for(violation.behavior)
}

fn drive(
    ctx: &ExecutionCtx,
    surface: &Surface,
    state: &mut ExecutionState,
    now: Tick,
    marks: &mut Marks,
    seq: &mut i64,
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

        let snapshot = state.clone();
        if render::render_run_if(surface, plan, state) == Pass::Rendered {
            *seq += 1;
            marks.observe(&snapshot, state, *seq);
            continue;
        }

        let snapshot = state.clone();
        if render::render_expressions(surface, plan, state, now) == Pass::Rendered {
            *seq += 1;
            marks.observe(&snapshot, state, *seq);
            continue;
        }

        return Ok((spawns, subflows));
    }
    Err(worker_err(WorkerError::NoFixedPoint(ctx.id)))
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
/// execution's `FOR UPDATE` lock, which is what serialises two workers there;
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
            let hit = sqlx::query(
                "UPDATE workflow.task_runs \
                    SET state = $1, history = $2, attempts = $3, rendered = $4, \
                        rendered_run_if = $5, not_before_ms = $6, \
                        claimed_at = CASE WHEN $7 THEN NULL ELSE claimed_at END, \
                        ended_seq = $10, \
                        created_seq = COALESCE($12, created_seq), \
                        resume_with = $13, \
                        updated_at = now() \
                  WHERE execution_id = $8 AND run_ix = $9 \
                    AND ($11::timestamptz IS NULL OR claimed_at = $11)",
            )
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
            sqlx::query(
                "INSERT INTO workflow.task_runs \
                   (execution_id, run_ix, task_ix, task_id, type_id, parent_ix, value, \
                    rendered, rendered_run_if, state, history, attempts, not_before_ms, \
                    created_seq, ended_seq, resume_with) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
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
            Ok(Attempt {
                history,
                started_at: Tick(started_at),
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
}
