//! `LoopUntil`, and its alias `WaitFor` — the one flowable that runs its
//! children **more than once**.
//!
//! `io.kestra.plugin.core.flow.LoopUntil` runs its `tasks:` as a sequential
//! list, and when every one of them has terminated without failing it asks a
//! single question: *go round again, or stop?* Going round again is a `PAUSED`
//! of `checkFrequency.interval`, after which every run beneath the loop is
//! thrown away and re-created ([`Command::RestartLoop`]). Stopping takes the
//! children's aggregate state, or `FAILED` if a maximum was reached and the
//! author asked for that to fail.
//!
//! # The decision, verbatim from `LoopUntil.java`
//!
//! ```java
//! public Instant nextExecutionDate(…) {
//!     if (!this.reachedMaximums(runContext, execution, parentTaskRun, false)) {
//!         String continueLoop = runContext.render(this.condition).skipCache()…;
//!         if (!TruthUtils.isTruthy(continueLoop)) {
//!             return Instant.now().plus(…getInterval()…);
//!         }
//!     }
//!     return null;
//! }
//!
//! public Optional<State.Type> resolveState(…) {
//!     boolean childTaskExecuted = this.childTaskRunExecuted(execution, parentTaskRun);
//!     if (childTaskExecuted && nextExecutionDate(…) != null) return Optional.empty();
//!     if (childTaskExecuted && this.reachedMaximums(…, true) && failOnMaxReached)
//!         return Optional.of(State.Type.FAILED);
//!     return FlowableUtils.resolveState(…);
//! }
//! ```
//!
//! Three facts fall out of it that a reading of the traces alone would have got
//! wrong, and each is asserted below rather than only written here:
//!
//! 1. **The condition is asked only when no maximum has been reached.** So a
//!    loop at its limit never renders its condition at all, which is why
//!    [`drive`] returns without waiting for a render in that case.
//! 2. **`maxIterations` is `iterationCount > maxIterations`, strictly.** A loop
//!    with `maxIterations: 3` whose condition never becomes true runs **four**
//!    iterations — `valids/waitfor-max-iterations`' golden is
//!    `CREATED RUNNING PAUSED RUNNING PAUSED RUNNING PAUSED RUNNING FAILED`,
//!    three pauses and four `RUNNING`s. The three flows with the same limit
//!    that stop at three do so because their condition became true, not because
//!    of the limit.
//! 3. **`failOnMaxReached` decides only the TERMINAL, never the count.** It is
//!    read after the loop has already stopped.
//!
//! # `iterationCount` is derived from the run's history, not stored
//!
//! Kestra keeps it in the taskrun's outputs and increments it as it schedules
//! each pause. Here it is `history.iter().filter(|s| **s == Running).count()`,
//! which is the same number for a shape the golden pins exactly: a `LoopUntil`
//! run reads `CREATED RUNNING (PAUSED RUNNING)* <terminal>`, so the `RUNNING`s
//! count the iterations and nothing has to be persisted for it. [`iteration`]
//! is the one definition, shared by this module and by
//! [`worker::render`](crate::worker) — which is what puts
//! `{{ outputs.<loop>.iterationCount }}` in front of the children. **Nine of
//! the corpus's ten flows of this construct are written around that
//! expression**; the tenth, `valids/waitfor-child-task-warning`, reads a
//! child's output instead.
//!
//! **Where that derivation is narrower than the stored counter**: a `LoopUntil`
//! that is *retried* re-enters `CREATED` and then `RUNNING` again, and this
//! would count the new attempt's iterations on top of the old ones. Nothing can
//! reach it — `retry::scan` retries a `Runnable` or an `Executable` and a
//! `LoopUntil` is neither — and
//! `no_corpus_loop_until_declares_a_retry_or_inherits_one` is what says so.
//!
//! # What the oracle can and cannot judge here
//!
//! Nine corpus entries reach this construct. **Six of them have an
//! `io.kestra.plugin.core.output.OutputValues` child**, for which this crate
//! ships no handler, so their children cannot run at all and no amount of
//! correctness in this module can move them — they are the strict upper bound
//! on what W8 Task 4 could bank, not failures of it. Of the remaining three,
//! `valids/waitfor-nested` is banked and the other two each differ from the
//! golden on exactly ONE field, for a reason that is not this module:
//!
//! * `valids/loop-until-restart` runs FOUR iterations against a golden of
//!   three. Its iteration count is a function of the **artifact's delay-poller
//!   latency** rather than of the flow: its `PT1S` pauses took **1.17 s and
//!   1.92 s** on the round the golden was taken from, which put the third
//!   iteration past a `PT3S` bound that a clock moving to the earliest deadline
//!   and no further does not reach until the fourth.
//!   `valids/waitfor-max-duration`, the same construct with a finer interval,
//!   is one of only **two** corpus flows Kestra ITSELF could not reproduce
//!   across five rounds, which is the corroboration that this is a property of
//!   the construct and not of one entry.
//! * `valids/loopuntil-failed-flowable`'s loop ends `SUCCESS` where the golden
//!   holds `FAILED`, because its condition is
//!   `{{ outputs.loop.iterationCount < "5" }}`: Pebble widens both operands of
//!   a relational operator to `Number` and throws on a `String`, and minijinja
//!   orders values across kinds and answers `True`. A RENDERER divergence
//!   wearing a loop's name.
//!
//! `oracle-exec/baseline.txt` and `oracle-exec/README.md` carry both, and
//! `handler_surface.py` carries a blocker row for each so the ceiling does not
//! name an entry nothing can bank.

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx, TaskKind};
use crate::executor::state::{ExecutionState, Resume, RunIx, RunState, Tick};

use super::{aggregate, plus, sequential, Step};

/// Which iteration a `LoopUntil` run is in — Kestra's `iterationCount`,
/// 1-based, `0` before the loop has started.
///
/// See the module doc for why this is derived rather than stored, and for the
/// one shape it is narrower than the artifact's counter for.
pub fn iteration(state: &ExecutionState, run: RunIx) -> usize {
    state.runs[run]
        .history
        .iter()
        .filter(|s| **s == RunState::Running)
        .count()
}

/// Kestra's `LoopUntil.childTaskRunExecuted`: every direct child has a run and
/// every one of those runs is terminated **without having failed**.
///
/// ```java
/// .filter(t -> t.getParentTaskRunId().equals(parentTaskRun.getId())
///           && t.getState().isTerminatedNoFail())
/// .count() == tasks.size()
/// ```
///
/// **The `NoFail` half is what decides `valids/waitfor-multiple-tasks-failed`**,
/// whose second child is a `Fail`: the count never reaches `tasks.size()`, so
/// the loop never asks its condition, never pauses, and takes the children's
/// aggregate — one iteration and `FAILED`, which is exactly its golden.
/// A version that asked "are they all terminal" instead would pause and loop a
/// failing body until its limit.
pub(crate) fn children_executed(state: &ExecutionState, run: RunIx, tasks: &[TaskIx]) -> bool {
    tasks.iter().all(|&task| {
        super::run_of(state, Some(run), task)
            .and_then(|ix| super::latest(state, ix))
            .is_some_and(|s| s.is_terminal() && !matches!(s, RunState::Failed | RunState::Killed))
    })
}

/// What one `LoopUntil` run wants at the round it is `RUNNING`.
///
/// The child list is walked by [`sequential::drive`] — a `LoopUntil` is an
/// `AbstractBranch` and `LoopUntil.tasksTree` builds `GraphUtils.sequential`,
/// and the golden agrees: every multi-child corpus loop records its siblings
/// `before` one another, never `overlapping`.
///
/// Only when that walk answers `Done` is the loop's own question asked.
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
    now: Tick,
) -> Step {
    let TaskKind::LoopUntil {
        tasks,
        interval,
        max_iterations,
        max_duration,
        fail_on_max_reached,
        ..
    } = &plan.tasks[task].kind
    else {
        // Unreachable: `descend` matches on this kind before it gets here.
        return Step::Waiting;
    };
    let errors = plan.tasks[task].errors.as_slice();
    match sequential::drive(plan, state, tasks, errors, Some(run), now) {
        Step::Commands(cmds) => return Step::Commands(cmds),
        Step::Waiting => return Step::Waiting,
        Step::Done(_) => {}
    }

    // `childTaskRunExecuted`. A body that failed never reaches the loop's own
    // question -- see `children_executed`.
    if !children_executed(state, run, tasks) {
        return Step::Done(aggregate(state, tasks, Some(run)));
    }

    let reached = reached_maximums(state, run, *max_iterations, *max_duration, now);
    if !reached {
        // The condition, which the caller renders because `advance()` is pure.
        // It is asked ONLY here: a loop at its maximum never evaluates it, and
        // `worker::render`'s tier is gated on the same two facts so that no
        // expression is evaluated that Kestra would not have evaluated.
        let Some(rendered) = state.runs[run].rendered.as_deref() else {
            return Step::Waiting;
        };
        if !super::branch::is_truthy(rendered) {
            return Step::Commands(vec![Command::PauseRun {
                run,
                // `Iterate`, and naming it is what stops this run from being
                // parked with nobody to wake it -- see `executor::Resume`.
                resume: Resume::Iterate {
                    at: plus(now, *interval),
                },
            }]);
        }
    }
    if reached && *fail_on_max_reached {
        return Step::Done(RunState::Failed);
    }
    Step::Done(aggregate(state, tasks, Some(run)))
}

// **`resume` used to live here too**, reading `TaskRun::not_before` where
// [`pause`](super::pause)'s read the plan — two functions, two
// `TaskKind`-keyed guards, two walks. W8 Task 4 fix round 2 collapsed both
// into [`flowable::resume_paused`](super::resume_paused), which reads the
// `Resume` this module's [`drive`] wrote. See [`Resume`] for what that makes
// unrepresentable.

/// Kestra's `LoopUntil.reachedMaximums`, minus the logging.
///
/// ```java
/// if (maxIterations.isPresent() && iterationCount > maxIterations.get()) return true;
/// Instant creationDate = parentTaskRun.getState().getHistories().reversed().stream()
///     .filter(h -> h.getState().isCreated()).findFirst().get().getDate();
/// if (maxDuration.isPresent() && creationDate.plus(maxDuration.get()).isBefore(Instant.now()))
///     return true;
/// ```
///
/// **`>` and not `>=`, and that is measured, not transcribed.** The corpus has
/// **five** flows with a `maxIterations:` and only ONE of them can tell the two
/// readings apart:
///
/// | flow | limit | iterations in the golden | what stopped it |
/// |---|---|---|---|
/// | `valids/waitfor-max-iterations` | 3 | **4** | **the limit** |
/// | `valids/waitfor-multiple-tasks` | 3 | 3 | the condition |
/// | `valids/waitfor-no-success` | 5 | 5 | the condition |
/// | `valids/waitfor` | 3 | 1 | the condition |
/// | `valids/waitfor-multiple-tasks-failed` | 5 | 1 | a child FAILED |
///
/// A `>=` reading agrees with the last four and is wrong about the first, so
/// the first is the whole instrument — and it is one of the six entries held by
/// the `OutputValues` handler, which is why the ratchet cannot score this line
/// either way (mutating `>` to `>=` leaves it green) and
/// `a_limit_of_n_runs_n_plus_one_iterations_and_fail_on_max_decides_only_the_terminal`
/// is what holds it.
///
/// **`maxDuration` is measured from the loop run's CREATION**, not from the
/// current iteration and not from the attempt: `resetAttempts()` rewinds
/// `current` and keeps the histories, so `reversed().filter(isCreated())` finds
/// the run's original `CREATED` however many iterations have gone by. Here that
/// instant is [`TaskRun::attempt_started_at`], because a `LoopUntil` opens
/// exactly one attempt and never reopens it ([`Command::RestartLoop`] writes
/// the run's history and not the attempt's) — the same instant to within the
/// round that created the run.
///
/// [`TaskRun::attempt_started_at`]: crate::executor::TaskRun::attempt_started_at
pub(crate) fn reached_maximums(
    state: &ExecutionState,
    run: RunIx,
    max_iterations: Option<i64>,
    max_duration: Option<std::time::Duration>,
    now: Tick,
) -> bool {
    if let Some(max) = max_iterations {
        if iteration(state, run) as i64 > max {
            return true;
        }
    }
    if let Some(d) = max_duration {
        if let Some(created) = state.runs[run].attempt_started_at() {
            if plus(created, d) < now {
                return true;
            }
        }
    }
    false
}

/// Whether the caller should render this `LoopUntil`'s `condition:` — the gate
/// [`worker::render`](crate::worker)'s tier B asks, and the same two facts
/// [`drive`] decides on.
///
/// **A shared predicate, for `pause_gap`'s reason.** Kestra evaluates the
/// condition in exactly one place, `nextExecutionDate`, behind exactly two
/// guards: the children have all terminated without failing, and no maximum has
/// been reached. A render tier that asked at any other moment would evaluate an
/// expression the artifact never evaluates — and since a render that will not
/// resolve FAILS the run, that is not a harmless extra evaluation but a
/// terminal the artifact does not produce. The corpus writes conditions over
/// `outputs.<child>.…`, which is precisely the name that does not exist yet
/// before the children terminate.
///
/// `false` for any task that is not a `LoopUntil`, so the caller can ask it of
/// every run it is about to render.
pub fn condition_is_asked(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
    now: Tick,
) -> bool {
    let TaskKind::LoopUntil {
        tasks,
        max_iterations,
        max_duration,
        ..
    } = &plan.tasks[task].kind
    else {
        return false;
    };
    children_executed(state, run, tasks)
        && !reached_maximums(state, run, *max_iterations, *max_duration, now)
}
