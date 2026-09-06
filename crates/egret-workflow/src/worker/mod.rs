//! The worker: the claim loop, the reaper, and the types a handler is handed.
//!
//! W3a built [`advance`](crate::executor::advance) — pure over
//! `(plan, state, now)` — and judged it through a test harness that kept
//! `ExecutionState` in memory. This module is the first **real** caller: it
//! claims a task run out of Postgres, hands it to a handler, and commits the
//! handler's result, `advance()`'s commands and the successors together.
//!
//! # What the ratchet cannot see
//!
//! `tests/executor.rs` compares execution traces. It has no opinion about
//! whether two workers claimed the same run, whether a crash left half a
//! transaction applied, or whether a lease expired on time — so
//! `tests/worker.rs` is the only instrument on any of it, and every property
//! there was written failing first for that reason.
//!
//! # The two commits, and why there are two
//!
//! A tick is **not** one transaction end to end, and the reason is that a
//! handler is arbitrarily long: `core.flow.Sleep` sleeps for an author-chosen
//! duration and `egret.sql` waits on a remote engine. Holding a pool connection
//! and a row lock across either is how a worker takes a database down. So:
//!
//! 1. [`claim`] marks up to `n` runs, **committed**, on
//!    `egret-automation`'s statement — see [`claim`] for the citation.
//! 2. per run: the handler, then [`apply_outcome`] writes the result,
//!    `advance()`'s commands and the successors in **one** transaction.
//!
//! Step 2 is the plan's Global Constraint: no window may exist where a task is
//! finished and the execution has not moved. Step 1 being separate is what
//! gives the lease something to expire, and therefore what makes [`reap`] a
//! live path rather than a guard nobody can see fire — with a single
//! transaction, a crash rolls the claim back too and the reaper could never
//! reclaim anything.
//!
//! The cost of the split is stated rather than hidden: a worker killed between
//! the claim and the commit leaves its run **leased**, and it becomes claimable
//! again only when [`reap`] takes it back. `tests/worker.rs`'s
//! `a_tick_killed_between_the_handler_and_the_commit_applies_nothing` asserts
//! both halves of that, in both directions.
//!
//! # What is NOT here
//!
//! No handler. W3b Task 3 owns `core.log.Log`, `core.debug.Return`,
//! `core.execution.Fail` and `core.flow.Sleep`, and Task 6 owns `egret.sql`;
//! this module takes a [`Registry`] trait object and does not care which. Task 4
//! owns [`Command::SpawnExecution`](crate::executor::Command::SpawnExecution),
//! which [`apply_outcome`] currently **refuses** rather than ignores — see
//! [`WorkerError`].

mod claim;
mod reaper;

pub use claim::{apply_outcome, claim, start_execution, tick, Claimed};
pub use reaper::reap;

use crate::executor::RunState;

/// The claim's `working_timeout_s` equivalent, and the same default
/// `automation.schedules.working_timeout_s` carries: one hour.
///
/// It is a `const` rather than a column because a task run has no per-row
/// timeout to read — [`reap`] takes the timeout as a parameter so a test can
/// drive it, exactly as the plan's interface says, and a caller that wants the
/// product default passes this.
pub const DEFAULT_WORKING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3600);

/// What a handler is handed.
///
/// Deliberately a snapshot rather than a live handle: a handler must not be
/// able to write to the execution, because everything it writes has to land in
/// [`apply_outcome`]'s single transaction and a handler that had its own
/// connection could commit outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskInput {
    pub workspace_id: uuid::Uuid,
    pub execution_id: uuid::Uuid,
    /// The `task_runs` row's surrogate key.
    pub run_id: uuid::Uuid,
    /// The author's task id.
    pub task_id: String,
    /// The task's `type:` — what the [`Registry`] dispatches on.
    pub type_id: String,
    /// A loop iteration's value, if this run has one.
    pub value: Option<String>,
    /// Which attempt this is, 1-based. A retried run re-enters `CREATED` and
    /// opens a second attempt (`executor::apply`'s rule 2), so a handler that
    /// wants to know it is a retry reads this.
    pub attempt: usize,
}

/// What a handler produces.
///
/// One field today. W3b Task 3 widens it — `core.debug.Return` has an output
/// (`{"value": …}`) and `core.execution.Fail` has a message — and it is a
/// struct rather than a bare [`RunState`] so that widening is a field rather
/// than a signature change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The terminal state this run reached. `advance()` reads it as a
    /// [`Command::SetRunState`](crate::executor::Command::SetRunState).
    pub state: RunState,
}

impl Outcome {
    pub fn success() -> Self {
        Self {
            state: RunState::Success,
        }
    }
    pub fn failed() -> Self {
        Self {
            state: RunState::Failed,
        }
    }
}

/// Type-id → handler dispatch.
///
/// A trait taken as `&dyn Registry` rather than a concrete type, so Task 2's
/// tests can supply a fake one and Task 3 can supply the real one without
/// either touching the other. Task 3's `registry.rs` implements it over a
/// `TaskHandler` per type; nothing here knows that.
#[async_trait::async_trait]
pub trait Registry: Send + Sync {
    /// Run one task. `Err` is a **worker** failure (the type is unknown, the
    /// handler could not be started) and aborts the tick for that run without
    /// moving the execution; a task that ran and failed returns
    /// `Ok(Outcome { state: Failed })`.
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome>;
}

/// The failures this module raises that are not database errors.
#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// A [`Command`](crate::executor::Command) no task in W3b Task 2 executes.
    ///
    /// **A refusal rather than a no-op, and that is the point.**
    /// `Command::SpawnExecution` is W3b Task 4's, and an arm that silently
    /// dropped it would let a `Loop` run to `SUCCESS` having iterated nothing —
    /// which is precisely the over-acceptance shape the spec §7 says must not
    /// ship. Task 4 replaces this with an executor.
    #[error("the worker cannot execute {0} yet (W3b Task 4)")]
    NotYetExecutable(&'static str),

    /// `advance()` asked for more rounds than [`MAX_ROUNDS`] allows.
    ///
    /// `advance()` is a fixed point in every corpus flow; a caller that does not
    /// reach one has either found a cycle or is failing to fill a render slot
    /// (`advance`'s *What the caller must supply*), and both are bugs that must
    /// be loud. Without the bound the tick would spin holding a transaction
    /// open, which is the same outage a long handler inside a transaction
    /// causes.
    #[error("advance() did not reach a fixed point in {MAX_ROUNDS} rounds on execution {0}")]
    NoFixedPoint(uuid::Uuid),

    /// The stored `source` no longer builds a plan.
    #[error("execution {0} holds a source that no longer plans: {1}")]
    Unplannable(uuid::Uuid, String),

    /// A stored state string outside [`RunState::ALL`].
    #[error("{0} holds state {1:?}, which is not one of RunState::ALL")]
    UnknownState(uuid::Uuid, String),

    /// A stored row that cannot be turned back into what `advance()` reads:
    /// a `run_ix` that is not its own position, a `task_ix` the plan does not
    /// hold, an `attempts` entry with no `started_at`.
    ///
    /// **Separate from [`WorkerError::UnknownState`] rather than folded into
    /// it.** The first draft reused that variant for all of these, so a
    /// mis-numbered row was reported as *"holds state "run_ix 3 at position 5",
    /// which is not one of RunState::ALL"* — a message that names the wrong
    /// defect and sends the reader to the wrong table.
    #[error("{0} cannot be read back into an execution state: {1}")]
    Corrupt(uuid::Uuid, String),
}

/// The most rounds of `advance()` one [`apply_outcome`] will drive.
///
/// Derived from the executor's own bound rather than picked: `executor::plan`
/// caps task nesting, and one round retires at most one step of one list, so a
/// flow cannot need more rounds than it has tasks. 1000 is far above every
/// corpus flow (the largest plans 43 tasks) and exists to turn a cycle into an
/// error rather than a hang.
pub const MAX_ROUNDS: usize = 1000;

/// `RunState` from its wire spelling — the inverse of
/// [`RunState::wire`](crate::executor::RunState::wire).
///
/// **Derived from [`RunState::ALL`]**, not a second `match`. A hand-written
/// inverse is a table that can disagree with the one it inverts, and a variant
/// added to the enum would silently fail to load rather than fail to compile.
pub(crate) fn from_wire(s: &str) -> Option<RunState> {
    RunState::ALL.iter().copied().find(|r| r.wire() == s)
}
