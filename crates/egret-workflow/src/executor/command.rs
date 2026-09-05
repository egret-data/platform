//! What [`advance`](super::advance) asks the caller to do. The caller does the
//! I/O; nothing here reads a clock.

use super::plan::TaskIx;
use super::state::{RunIx, RunState, Tick};

/// What [`advance`](super::advance) asks the caller to do.
///
/// Two fields diverge from the plan's draft, both deliberately and both
/// recorded in `oracle-exec/README.md` rather than left for a reader to
/// rediscover:
///
/// - `ScheduleRetry::not_before` is a [`Tick`], not a `std::time::Instant` —
///   Task 2's correction, for the reason [`Tick`] gives.
/// - `StartRun` carries `at` — Task 3's, for the reason on the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    CreateRun {
        task: TaskIx,
        parent: Option<RunIx>,
        value: Option<String>,
    },
    StartRun {
        run: RunIx,
        /// The `now` [`advance`](super::advance) held when it took this
        /// decision, stored verbatim on the attempt this command opens.
        ///
        /// **Why the command carries the instant.** Every elapsed-time rule in
        /// the artifact is measured from a run's attempt start —
        /// `RETRY_FAILED_TASK`'s `maxDuration` from
        /// `this.attempts.getFirst().getState().minDate()`
        /// (`TaskRun.java:337`), and `WaitFor`/`Pause` from their own start.
        /// Something therefore has to store that instant, and there were three
        /// places to put it: a `now` parameter on
        /// [`apply`](super::apply), a field the caller sets by hand, or here.
        /// Here is the only one where the stored instant cannot disagree with
        /// the one the decision was taken at.
        at: Tick,
    },
    /// The plan's draft spells `not_before` as an `Instant`; it is a [`Tick`]
    /// for the reason [`Tick`] gives.
    ScheduleRetry {
        run: RunIx,
        not_before: Tick,
    },
    SetRunState {
        run: RunIx,
        state: RunState,
    },
    SetExecutionState {
        state: RunState,
    },
    /// Run one iteration of a [`Loop`](super::TaskKind::Loop) as a **separate
    /// execution**.
    ///
    /// A `Loop` does not iterate inside its own execution: each value produces
    /// a sub-execution carrying `parentId`, and the body's tasks never appear
    /// in the parent's `taskRunList`. Measured over the whole golden: 41 entries
    /// carry 142 sub-executions between them, and across the 39 comparable
    /// entries holding a `Loop` not one of the 82 tasks inside a loop body has a
    /// run in its parent — on egret's side or on Kestra's.
    ///
    /// # `task` is the LOOP, not the body
    ///
    /// A sub-execution runs the loop's **whole** `tasks:` list, which a single
    /// body index cannot name: `valids/loop-serial-multiple-tasks` has two body
    /// tasks and each of its two sub-executions ran both of them (`runs:
    /// ["first", "second"]`, twice, in the golden). So this names the `Loop`
    /// itself and the caller reads its list, which is also what Kestra's
    /// `loopRun` carries — `{"taskId": "1_each", "taskRunId": …, "index": …,
    /// "value": …}` names the loop task, never a body task.
    ///
    /// # Nothing executes this yet, and that is a hole with a count
    ///
    /// W3b/W6 own creating the sub-execution and reporting its outcome back.
    /// Until then [`advance`](super::advance) emits the command and the oracle's
    /// side of `spawned` stays empty, so the 41 golden entries that carry a
    /// sub-execution mismatch on that field whatever else they get right.
    /// `oracle-exec/README.md` records it under *What this oracle does not
    /// prove*; `every_first_batch_egret_spawns_is_a_prefix_of_what_kestra_ran`
    /// is what compares the part of it that can be compared.
    SpawnExecution {
        task: TaskIx,
        /// The iteration value, already resolved to the string Kestra puts on
        /// `loopRun.value` — see
        /// [`iteration_values`](super::flowable::loop_::iteration_values).
        value: String,
        /// The run of the `Loop` this iteration belongs to.
        parent_run: RunIx,
    },
}
