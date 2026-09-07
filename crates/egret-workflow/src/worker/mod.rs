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
//! this module takes a [`Registry`] trait object and does not care which.
//!
//! **The renderer is [`render`], and it is inside the tick.** `advance()`'s
//! `rendered` and `rendered_run_if` slots are the caller's to fill, and Task 2
//! shipped this module with nothing filling them: a `runIf:`, an `If`, a
//! `Switch` or a `Loop` got an empty command vector for ever, which is a stall
//! rather than an error. `claim::drive` now renders between rounds of
//! `advance()`, so the shipping worker resolves them and [`resume`] is what a
//! caller uses when something OUTSIDE the execution has changed.

mod claim;
pub mod handlers;
mod reaper;
mod registry;
mod render;
mod spawn;

pub use claim::CREATE_BESIDE_A_CREATED_SIBLING;
pub use claim::{apply_outcome, claim, resume, start_execution, tick, Claimed};
pub use reaper::reap;
pub use registry::Handlers;

use crate::executor::RunState;
use crate::runners::RenderContext;

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
///
/// **No `Eq`.** [`TaskInput::spec`] is a `serde_json::Value`, which is
/// `PartialEq` and not `Eq` because a JSON number may be a float. Nothing
/// compares two `TaskInput`s for equality; the derive is here so a test can
/// `assert_eq!` on one field's worth of it.
#[derive(Clone, Debug, PartialEq)]
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
    /// The task's own properties, exactly as the author wrote them —
    /// `models::TaskKind::Leaf`'s `spec`, i.e. the task's map with `id`,
    /// `type`, `description`, `retry`, `timeout`, `disabled` and `runIf`
    /// already taken out by `models::Task`'s deserializer.
    ///
    /// **The plan cannot supply this.** `executor::TaskKind::Runnable` carries
    /// a `type_id` and nothing else — deliberately, because a state machine
    /// that knows what `egret.sql` is has stopped being a state machine — so
    /// the properties come from the execution's frozen `source`, re-parsed by
    /// [`claim::tick`](tick). That is the same rule the `Plan` itself follows
    /// (rebuilt from `source` every tick rather than stored).
    pub spec: serde_json::Value,
    /// What a `{{ … }}` in one of those properties renders against.
    ///
    /// Built by [`render::Surface`] from the execution's own inputs, its flow's
    /// `variables:`, the outputs of every task that has terminated (its
    /// ancestors' included), and this run's `taskrun`. See that module for what
    /// each name is and what is deliberately still missing.
    pub render: RenderContext,
    /// The identity this execution acts as, and `None` when it has none.
    ///
    /// A workflow runs with nobody logged in, so a handler that reaches an
    /// egret object on the execution's behalf has to name somebody. Four of the
    /// five handlers never do — a `Log`, a `Return`, a `Fail` and a `Sleep`
    /// touch nothing outside the execution — and [`handlers::sql`] is the first
    /// that does.
    ///
    /// **The rule is `egret-automation`'s, followed rather than re-derived.** An
    /// alert runs as its *owner*, and an owner whose credentials cannot be
    /// resolved makes the run fail rather than fall back to the service account
    /// (`evaluator.rs`'s `the_token_is_minted_for_the_owner_and_nobody_else`
    /// and `an_owner_with_no_mintable_token_fails_instead_of_falling_back`).
    /// Falling back is the escalation, and it looks exactly like a flow that
    /// kept working.
    ///
    /// **Today this is `workflow.flows.created_by`**, read by [`claim`] through
    /// the execution's `flow_id`, and that is a stand-in with two known holes,
    /// both stated rather than discovered later:
    ///
    /// * it is the flow's *creator*, not a `run_as` anybody chose, and it is
    ///   `ON DELETE SET NULL`, so deactivating the author silently turns every
    ///   `egret.sql` in their flows into a refusal (loud, but not obviously
    ///   about the user record);
    /// * an execution with no `flow_id` — an ad-hoc run, or one whose flow was
    ///   deleted — has no owner at all and therefore cannot run an `egret.sql`.
    ///
    /// The fix is a `run_as` on `workflow.executions`, chosen when the execution
    /// is started; W6 (triggers) is the package that has to decide what a
    /// scheduled run acts as, so it is left to own that column rather than
    /// guessed at here.
    pub principal: Option<egret_core::UserId>,
}

/// What a handler produces.
///
/// Task 2 left this a single [`RunState`] and said Task 3 would widen it;
/// this is that widening, and both new fields are measured rather than
/// guessed — see [`handlers`] for the per-class citations.
///
/// **No `Eq`**, for [`TaskInput`]'s reason: `serde_json::Value` has none.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    /// The terminal state this run reached. `advance()` reads it as a
    /// [`Command::SetRunState`](crate::executor::Command::SetRunState).
    pub state: RunState,
    /// The task's output map — what `{{ outputs.<task id> }}` reads.
    ///
    /// **`None` means the task produced no `Output` object at all**, which is
    /// what three of the four handlers do: Kestra's `Log`, `Sleep` and a
    /// falsy `Fail` all `return null` from `run` (`Log.java:109`,
    /// `Sleep.java:62`, `Fail.java:137`), and `VoidOutput` is an empty class
    /// (`VoidOutput.java:3`) none of them ever instantiates.
    ///
    /// **`None` and `Some({})` are not distinguishable to an expression, and
    /// that was measured rather than reasoned.** On `kestra/kestra:v2.0.0-rc13`
    /// a flow whose second task renders `{{ outputs.a }}` printed `{}` for `a`
    /// = a `Log`, a `Sleep`, a falsy `Fail` **and** a `Return` with no
    /// `format:` — and `{{ outputs }}` printed `{"a":{},"b":{}}`, so the key
    /// is present and empty rather than absent. The renderer W3b Task 5 builds
    /// must therefore map `None` to `{}`, not omit the key: an author's
    /// `{{ outputs.a }}` has to render, while `{{ outputs.a.value }}` has to
    /// fail (measured: *"Unable to find `value` used in the expression"*, and
    /// the run goes `FAILED`).
    pub outputs: Option<serde_json::Value>,
    /// The message a failure carries.
    ///
    /// It exists because `core.execution.Fail` has **two** throwing paths with
    /// **different** messages — the literal `"Fail on a condition"` when a
    /// `condition:` is truthy (`Fail.java:135`) and the rendered
    /// `errorMessage` when there is no condition at all (`:140`, default
    /// `"Task failure"` at `:127`) — and **nothing in `golden.json` records a
    /// failure message**, so the ratchet cannot tell them apart. This field is
    /// what `handlers::fail`'s own tests assert on instead.
    ///
    /// `None` on every success. Nothing persists it yet; see the task report.
    pub message: Option<String>,
}

impl Outcome {
    /// `SUCCESS`, with no output — the three void handlers' answer.
    pub fn success() -> Self {
        Self {
            state: RunState::Success,
            outputs: None,
            message: None,
        }
    }
    /// `SUCCESS`, carrying an output map.
    pub fn succeeded_with(outputs: serde_json::Value) -> Self {
        Self {
            state: RunState::Success,
            outputs: Some(outputs),
            message: None,
        }
    }
    /// `FAILED`, with no message. Kept because Task 2's tests use it.
    pub fn failed() -> Self {
        Self {
            state: RunState::Failed,
            outputs: None,
            message: None,
        }
    }
    /// `FAILED`, carrying the message the task failed with.
    pub fn failed_with(message: impl Into<String>) -> Self {
        Self {
            state: RunState::Failed,
            outputs: None,
            message: Some(message.into()),
        }
    }
}

/// One task type's implementation.
///
/// **Named `TaskHandler`, deliberately not `TaskRunner`** — the programme spec
/// §6.4 names it and gives the reason: in Kestra `TaskRunner` means a
/// *shell-command executor* (`TaskRunner.run(RunContext, TaskCommands,
/// filesToDownload)`, whose javadoc says "will be called by the script
/// plugin"), and reusing the name would recreate the exact confusion this
/// programme had to clear up.
///
/// Three methods, which is §6.4's list minus one: that signature also carries
/// `resolve(spec, PublishCx)`, the publish-time half, and W3b does not own
/// publish. `crate::spec::registry::TASK_TYPES` is publish's half of the seam
/// today — see [`Handlers`] for what it and this registry currently disagree
/// about.
#[async_trait::async_trait]
pub trait TaskHandler: Send + Sync {
    /// The `type:` this handler serves, spelled as a flow writes it.
    fn type_id(&self) -> &'static str;

    /// Run one task.
    ///
    /// The return contract is [`Registry::run`]'s: `Err` is a **worker**
    /// failure and aborts the tick without moving the execution, while a task
    /// that ran and failed is `Ok(Outcome { state: Failed, .. })`. A handler
    /// that turned a task failure into `Err` would stall the execution instead
    /// of failing it.
    async fn run(&self, task: &TaskInput) -> egret_core::Result<Outcome>;

    /// Stop a run that is still in flight.
    ///
    /// **No default body on purpose.** All four of W3b's handlers have nothing
    /// to cancel — they hold no pod, no remote query and no external handle —
    /// but a default `Ok(())` would hand that same answer to `egret.sql` and
    /// to W4's container handlers, where it is wrong and where nothing would
    /// say so. Each implementation states its own reason.
    ///
    /// **Nothing calls this yet.** W4 owns the kill path (the programme spec's
    /// *"the reclaim must kill the pod before retrying, or two pods will write
    /// at once"*), and until it lands this is a shape with no caller — which
    /// is stated here rather than left to be discovered.
    async fn cancel(&self, task: &TaskInput) -> egret_core::Result<()>;
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

    /// No handler is registered for a claimed run's `type_id`.
    ///
    /// **A refusal, and it aborts the tick for that run** rather than
    /// completing it in some state. An unknown type that "succeeded" would be
    /// the over-acceptance shape spec §7 forbids, and an unknown type that
    /// "failed" would be worse: it would look like the author's flow was
    /// wrong.
    #[error("no handler is registered for task type {0:?}")]
    NoHandler(String),

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
