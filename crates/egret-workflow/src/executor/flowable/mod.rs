//! One module per flowable, because each is an independent semantic unit with
//! its own corpus slice — and because a single `executor.rs` is how
//! `dataapi/src/lifecycle.rs` reached 12,091 lines.
//!
//! Task 3 shipped `sequential`; Task 4 added `parallel` and `dag` beside it;
//! Task 5 added `branch` (`If` and `Switch`); Task 6 adds `loop_`, which is the
//! one that creates no runs at all. Tasks 7-8 add the attempt lifecycle.
//!
//! What lives *here* rather than in one of them is the part every task list
//! shares whatever its scheduling rule: the answer type, how a run is looked
//! up, how a run is started, and how a `Running` child is descended into. Task
//! 3 had one flowable and so kept all four in `sequential.rs`; with three they
//! would otherwise be three copies, and three copies of "a `Runnable` gets
//! `SUBMITTED` first" is three chances for one of them to stop being true.

pub(crate) mod allow_failure;
pub(crate) mod branch;
pub(crate) mod dag;
pub(crate) mod loop_;
pub(crate) mod loop_until;
pub(crate) mod parallel;
pub(crate) mod pause;
pub(crate) mod sequential;
pub(crate) mod working_directory;

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx, TaskKind};
use crate::executor::state::{ExecutionState, RunIx, RunState, Tick};

/// `at + d`, in [`Tick`]s — the one definition, shared by
/// [`pause`] and [`loop_until`].
///
/// **Saturating rather than wrapping**, and that is the whole reason it is a
/// function: a `P100000D` an author can legally write overflows an `i64` of
/// milliseconds into the PAST, and a deadline behind `now` is a wait that ends
/// on the round it begins — a pause that resumes itself, or a loop that never
/// waits and whose bound fires immediately.
///
/// It was written twice, byte for byte, until W8 Task 4 fix round 1. Two copies
/// of one saturation rule are two chances for one of them to stop saturating,
/// which is exactly the shape `retry::parse_duration`'s single-parser note
/// records one module over.
pub(crate) fn plus(at: Tick, d: std::time::Duration) -> Tick {
    let millis = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
    Tick(at.0.saturating_add(millis))
}

/// What a `PAUSED` run wants — **the one place this executor leaves `PAUSED`,
/// and it reads the RUN rather than the kind.** W8 Task 4 fix round 2.
///
/// There were two: `sequential::walk` and `parallel::drive_concurrent` each
/// carried a `matches!(plan.tasks[task].kind, …)` list behind a catch-all, so
/// a construct missing from either list paused and was never asked again.
/// [`Resume`](crate::executor::Resume) records the answer when the pause is entered, so nothing here
/// needs to know which kinds pause — and a kind that pauses cannot be missing
/// from a list, because there is no list.
///
/// # The terminal is resolved ONCE, at pause time — and what that rests on
///
/// This function re-derives nothing. It reads a [`Resume`](crate::executor::Resume) that
/// [`pause::drive`] computed in the round the pause was **entered**, from
/// `(plan, attempt_started_at, rendered)`, and hands back the terminal that
/// bound chose. Before W8 Task 4 fix round 2 the bound was recomputed on
/// **every** round instead.
///
/// **That is safe only because `pause::Bound::NotRenderedYet` waits rather
/// than pauses**, and the dependency is load-bearing: a `pauseDuration:` that
/// is an expression the caller has not rendered yet makes `drive` return
/// [`Step::Waiting`] *without* emitting a [`Command::PauseRun`] at all, so no
/// run is ever parked carrying a terminal derived from an unrendered bound.
/// Give that arm any other answer — pause with `Resume::Never`, pause with a
/// default duration — and this function becomes **wrong rather than merely
/// stale**: the render that arrives one round later can no longer change the
/// terminal, because nothing re-reads the plan after the run is `PAUSED`.
/// Nothing else in the pause path restores that property, and the two sites
/// are in different modules, so the next change to `deadline`'s arms must come
/// back here.
///
/// **`Waiting` on `None` is unreachable rather than defensive.**
/// [`RunState::Paused`] is pushed by exactly one command,
/// [`Command::PauseRun`], and that command cannot be constructed without a
/// [`Resume`](crate::executor::Resume). It is `Waiting` and not a panic for the reason every other
/// "cannot happen" in this executor is: `advance()` is handed state a caller
/// stored and read back, and a state machine that aborts the process on a
/// surprising row is a worse failure than one that stalls.
///
/// **`every_paused_run_names_its_resumer` is what keeps it unreachable — and
/// until W8 Task 4 fix round 3 that sentence cited a test that did not
/// exist.** It does now, and it is only half the guard: it proves this
/// *executor* never writes such a run. The other half is at the far end of the
/// round trip, where `worker::claim` reads a `TaskRun` back out of Postgres —
/// migration **0142**'s `workflow_task_runs_paused_names_its_resumer`, because
/// migration 0141's two CHECKs are both predicated on `resume_with` itself and
/// so are vacuously true when it is NULL. `state = 'PAUSED'` beside
/// `resume_with IS NULL` was an accepted row, `parse_resume` mapped it to
/// `None`, and this arm stalled it for ever: **the fix round 2 defect,
/// unrepresentable in the type system and still representable in the
/// database.** `a_paused_row_that_names_no_resumer_cannot_be_stored` is that
/// half.
pub(crate) fn resume_paused(state: &ExecutionState, run: RunIx, now: Tick) -> Step {
    let Some(resume) = state.runs[run].resume_with else {
        return Step::Waiting;
    };
    let Some(at) = resume.at() else {
        // `Resume::Never`. **Neither a bound this executor holds nor a clock
        // it can move ends this**, and reading it as "due" is the
        // over-acceptance the variant exists to refuse: an unbounded `Pause`
        // that resumed itself would leave every figure in `baseline.txt`
        // unchanged, because none of the seven corpus flows written that way
        // reaches `golden.json` at all.
        return Step::Waiting;
    };
    if now < at {
        return Step::Waiting;
    }
    match resume {
        crate::executor::Resume::Never => Step::Waiting,
        crate::executor::Resume::Terminal { state: s, .. } => {
            Step::Commands(vec![Command::SetRunState { run, state: s }])
        }
        crate::executor::Resume::Iterate { .. } => {
            Step::Commands(vec![Command::RestartLoop { run }])
        }
    }
}

/// What one task list wants from the caller.
///
/// Three answers, not two, because "nothing to emit" and "finished" are
/// different: the first means a worker is holding a task and the executor must
/// wait, the second means the list has a terminal state to report upward. The
/// oracle's environment distinguishes them too — an execution with nothing to
/// do and nothing in flight that has *not* finished is `STUCK`, a broken
/// instrument rather than a wrong answer.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Work for the caller to apply. Never empty.
    Commands(Vec<Command>),
    /// A task is in flight; the executor has nothing to decide until it lands.
    Waiting,
    /// Every child reached a terminal state, and this is the list's own.
    Done(RunState),
}

/// Which of a flowable's two lists is driven right now — Kestra's
/// [`Execution.findTaskDependingFlowState`], with `finally` left out because
/// no W3a task implements it.
///
/// Java returns a `List<ResolvedTask>` and overloads the empty one with two
/// different meanings, which is what this enum separates:
///
/// | Java | here | why they differ |
/// |---|---|---|
/// | `resolvedTasks` | [`Current::Tasks`] | nothing has failed yet |
/// | `resolvedErrors` | [`Current::Errors`] | something failed for good and there is an `errors:` |
/// | `emptyList()` from the retry branch | [`Current::RetryPending`] | a failure that will be retried |
/// | `emptyList()` from the no-errors branch | [`Current::Ended`] | a failure for good with no `errors:` |
///
/// The two empties are not interchangeable and the artifact proves it twice
/// over. `FlowableUtils.resolveState`'s empty arm returns a state **only** if
/// `hasFailedNoRetry`, so a flowable whose child is about to be retried keeps
/// running — `valids/retry-flowable`'s `Sequential` stays `RUNNING` across all
/// three of its child's attempts and only then ends `FAILED`. But
/// `ExecutorService.handleEnd`'s `execution.isTerminated(currentTasks)` is
/// `0 == 0` for the *same* empty list, so the **execution** takes its state on
/// every one of those rounds — which is where `valids/retry-failed`'s
/// `CREATED RUNNING FAILED RETRYING RUNNING FAILED …` comes from. One empty
/// list, two answers, decided by who is asking.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Current<'a> {
    Tasks(&'a [TaskIx]),
    Errors(&'a [TaskIx]),
    RetryPending,
    Ended,
}

/// [`Current`], for one list and its `errors:`.
///
/// # "will be retried" is asked with the FULL precedence, and Kestra asks it
/// two different ways
///
/// Java's `hasFailedNoRetry` calls `taskRun.shouldBeRetried(resolvedTask
/// .getTask().getRetry())` — the failing task's **own** `retry:` and nothing
/// else. That alone would end `valids/retry-flowable`'s `Sequential` on its
/// child's first failure, because the child has no `retry:` of its own; the
/// retry lives on the `Sequential`. Kestra keeps the `Sequential` alive by a
/// *second*, unrelated mechanism — `ExecutorService.java:615-618` filters the
/// parent's own worker result out of the batch whenever a child is being
/// retried — and the measured result is that the `Sequential` stays `RUNNING`.
///
/// This asks once, with [`retry::policy_for`](crate::executor::retry::policy_for)'s
/// full task → ancestor → flow precedence, which reaches the same answer for
/// everything in this corpus without the second mechanism. Where the two
/// constructions *could* differ is a retry owned by a **grand**parent: Java's
/// filter names only `getParentTaskRunId()`, one level. It does not differ in
/// fact, because the grandparent's own state is a function of the parent's and
/// the parent is already held — measured on `valids/retry-flowable-nested-child`,
/// where `flowable` and `flowable2` both stay `RUNNING` for all three attempts.
pub(crate) fn current_list<'a>(
    plan: &Plan,
    state: &ExecutionState,
    tasks: &'a [TaskIx],
    errors: &'a [TaskIx],
    parent: Option<RunIx>,
    now: Tick,
) -> Current<'a> {
    let errors_started = errors
        .iter()
        .any(|&task| run_of(state, parent, task).is_some());
    let failed: Vec<RunIx> = tasks
        .iter()
        .filter_map(|&task| run_of(state, parent, task))
        .filter(|&ix| latest(state, ix) == Some(RunState::Failed))
        .collect();

    if !errors_started && failed.is_empty() {
        return Current::Tasks(tasks);
    }
    // `hasFailedNoRetry`: is there a failure that will NOT be retried?
    let will_retry = |&ix: &RunIx| {
        crate::executor::retry::policy_for(plan, state, ix)
            .and_then(|p| crate::executor::retry::next_retry_at(p, state, ix, now))
            .is_some()
    };
    if !errors_started && failed.iter().all(will_retry) {
        return Current::RetryPending;
    }
    if errors.is_empty() {
        Current::Ended
    } else {
        Current::Errors(errors)
    }
}

/// Kestra's `Execution.guessFinalState` over the runs of `tasks` under
/// `parent`, narrowed to what [`RunState`] can hold.
///
/// **`tasks`, never the list that is actually being driven.** Both
/// `FlowableUtils.resolveState` and `ExecutorService.onEnd` pass the `tasks`
/// list to `guessFinalState` even when `currentTasks` is the `errors` list, and
/// that is why `valids/errors` — whose two error tasks both SUCCEED — ends
/// `FAILED` rather than `SUCCESS`. An aggregate over the driven list would get
/// every `errors:` entry in the corpus exactly backwards.
pub(crate) fn aggregate(
    state: &ExecutionState,
    tasks: &[TaskIx],
    parent: Option<RunIx>,
) -> RunState {
    let mut out = RunState::Success;
    for &task in tasks {
        let Some(ix) = run_of(state, parent, task) else {
            continue;
        };
        match latest(state, ix) {
            Some(RunState::Killed) => return RunState::Killed,
            Some(RunState::Failed) => out = RunState::Failed,
            // **W8 Task 5, and its position in the chain is the artifact's
            // priority order, not an accident of where it was pasted.**
            // `Execution.guessFinalState`'s single pass is
            // `KILLED > FAILED > CANCELLED > WARNING > PAUSED`
            // (`Execution.java:774-789`), so a `WARNING` child loses to a
            // `FAILED` sibling and beats a `SUCCESS` one -- which the `!=`
            // guards are: they upgrade `Success` and refuse to downgrade
            // anything above.
            //
            // **`CANCELLED` arrived at W8 Task 6 and it went ABOVE this arm,
            // which is where Task 5's comment said it belonged.** The Java's
            // own conditions are the guards written out: `CANCELLED` is taken
            // unless the running answer is already `KILLED` or `FAILED`, and
            // `WARNING` unless it is already any of those three. Nothing in the
            // corpus supplies a `CANCELLED` run -- see `RunState::Cancelled`
            // for the derivation -- so `cancelled_outranks_warning_and_loses_to_failed`
            // is the row that supplies it.
            Some(RunState::Cancelled) if out != RunState::Failed => out = RunState::Cancelled,
            Some(RunState::Warning) if out != RunState::Failed && out != RunState::Cancelled => {
                out = RunState::Warning
            }
            _ => {}
        }
    }
    out
}

/// The run a list created for `task` under `parent`, if it has created one.
///
/// Every list this executor drives creates at most one run per child — a
/// `Loop`, which does not, spawns sub-executions instead (Task 6) — so the
/// first match is the only match.
pub(crate) fn run_of(state: &ExecutionState, parent: Option<RunIx>, task: TaskIx) -> Option<RunIx> {
    (0..state.runs.len()).find(|&ix| state.runs[ix].parent == parent && state.runs[ix].task == task)
}

/// This run's latest state, or `None` if it somehow has no history.
pub(crate) fn latest(state: &ExecutionState, run: RunIx) -> Option<RunState> {
    state.runs[run].history.last().copied()
}

/// Whether this run has reached a terminal state.
pub(crate) fn is_terminal(state: &ExecutionState, run: RunIx) -> bool {
    latest(state, run).is_some_and(RunState::is_terminal)
}

/// Start a created run.
///
/// **A `Runnable` gets `SUBMITTED` first and a flowable does not**, and that is
/// measured rather than stylistic: on `valids/parallel`, `parent` (the
/// `Parallel`) goes `CREATED RUNNING SUCCESS` while each of `t1`..`t6` goes
/// `CREATED SUBMITTED RUNNING SUCCESS`. `SUBMITTED` is in 580 of the golden's
/// 840 run histories and is the only thing in Kestra's record that separates a
/// task the worker ran from a flowable the executor ran, so a state machine
/// that omits it reproduces neither.
///
/// It is a separate `SetRunState` rather than something
/// [`apply`](crate::executor::apply) infers from `StartRun`, because `apply`
/// does not see the [`Plan`] and so cannot tell the two apart — and because
/// `apply` carrying that inference would be `apply` taking a decision, which is
/// exactly what it is documented not to do.
pub(crate) fn start(plan: &Plan, run: RunIx, task: TaskIx, now: Tick) -> Vec<Command> {
    let mut cmds = Vec::new();
    if matches!(plan.tasks[task].kind, TaskKind::Runnable { .. }) {
        cmds.push(Command::SetRunState {
            run,
            state: RunState::Submitted,
        });
    }
    cmds.push(Command::StartRun { run, at: now });
    // **A [`TaskKind::Executable`] is the third case, and it is why W6 opened
    // this directory.** It is not `SUBMITTED` -- no worker ever sees it, and
    // measured over `golden.json` not one of the 24 executed `Subflow` runs
    // carries that state -- and it is not merely `StartRun` either: something
    // has to create the child execution, and the only round in which that can
    // be asked for is the one that starts the run. See
    // [`Command::SpawnSubflow`] for why it cannot be asked for on its own.
    if matches!(plan.tasks[task].kind, TaskKind::Executable { .. }) {
        cmds.push(Command::SpawnSubflow { task, run });
    }
    cmds
}

/// What a **`CREATED`** child run needs, which is [`start`] for every kind but
/// one.
///
/// A [`TaskKind::Loop`] is the exception, and it is the artifact's shape rather
/// than a convenience: a `Loop` decides at `CREATED` whether it starts at all.
/// `valids/loop-empty`'s `1_each` reads `CREATED SUCCESS` with **no `RUNNING`
/// and no attempt**, and `valids/loop-null`'s reads `CREATED FAILED` the same
/// way, so the values have to be resolved before the run is started rather
/// than after — which is also why the caller renders a `Loop` while it is
/// `CREATED` and an `If` while it is `RUNNING`.
///
/// It returns a [`Step`] rather than a `Vec<Command>` because a `Loop` whose
/// `values` the caller has not rendered yet has nothing to ask for, and
/// [`Step::Commands`] is documented never to be empty.
/// A falsy `runIf:` skips the run, and this is where W3c puts that — **before**
/// anything else `open` does.
///
/// # One door, for every kind
///
/// Kestra asks the question in one place for runnables and flowables alike:
/// `ExecutionEventMessageHandler.java:269` tests
/// `TruthUtils.isTruthy(render(task.getRunIf()))` before it emits `SUBMITTED`
/// for a runnable, before it moves a flowable to `RUNNING`, and before either
/// gets an attempt. So the run reads `CREATED SKIPPED` and nothing else — which
/// is exactly what the golden holds for all six skipped runs, `Subflow`
/// (`valids/task-runif`'s `notexecutedflowable`) included.
///
/// [`open`] is this executor's matching single door: every created child run
/// reaches it, from [`sequential::walk`](super::sequential) and from
/// [`parallel::drive_concurrent`](super::parallel)'s rule 3, and a `Loop` goes
/// through it before it resolves its values. Putting the check in `start`
/// instead would miss the `Loop`, which never calls `start`; putting it in each
/// flowable would be six copies of one rule.
///
/// # `disabled:` and `runIf:` are the same shape and NOT the same rule
///
/// Both are task-level properties that stop a task running, and Kestra
/// implements them at opposite ends. `disabled:` is `Execution.removeDisabled`,
/// a *list* filter applied before anything is resolved — no taskrun at all —
/// and lives in [`flatten_list`](crate::executor::build_plan). `runIf:` is
/// rendered *per run*, so the run must exist to be skipped. A `runIf:`
/// implemented as a plan-time deletion would produce five taskruns for
/// `valids/task-runif` where Kestra produces five and three where it produces
/// three, and would still be wrong on every one of them.
enum RunIfVerdict {
    /// No `runIf:`, or one that rendered truthy.
    Run,
    /// Rendered falsy: `CREATED → SKIPPED`.
    Skip,
    /// There is a `runIf:` and the caller has not rendered it yet.
    NotRenderedYet,
}

fn run_if_verdict(plan: &Plan, state: &ExecutionState, run: RunIx, task: TaskIx) -> RunIfVerdict {
    let Some(_) = plan.tasks[task].run_if.as_deref() else {
        return RunIfVerdict::Run;
    };
    match state.runs[run].rendered_run_if.as_deref() {
        None => RunIfVerdict::NotRenderedYet,
        Some(rendered) if branch::is_truthy(rendered) => RunIfVerdict::Run,
        Some(_) => RunIfVerdict::Skip,
    }
}

pub(crate) fn open(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
    now: Tick,
) -> Step {
    match run_if_verdict(plan, state, run, task) {
        RunIfVerdict::Skip => return Step::Commands(vec![Command::SkipRun { run, at: now }]),
        RunIfVerdict::NotRenderedYet => return Step::Waiting,
        RunIfVerdict::Run => {}
    }
    match plan.tasks[task].kind {
        TaskKind::Loop { .. } => loop_::open(plan, state, run, task, now),
        _ => Step::Commands(start(plan, run, task, now)),
    }
}

/// Drive the flowable that a **`Running`** child run is, and turn its `Done`
/// into the command that terminates that child.
///
/// A `Runnable` is [`Step::Waiting`]: the worker is holding it and the executor
/// has nothing to decide until it lands.
///
/// # Recursion, and why there is no depth guard here
///
/// This descends only through a child that is **already `Running`**, and a
/// running child is a run in `state.runs`. Each level therefore descends from
/// one existing run to a *different* existing run — the next level searches for
/// runs whose `parent` is the level above, and a run's parent is fixed at
/// creation to a run that already existed, so parent indices strictly decrease
/// going up. Recursion depth is at most `state.runs.len()`, for **any** `Plan`,
/// including a hand-built cyclic one.
///
/// A `if depth > state.runs.len()` guard would therefore be unreachable code —
/// a guard nobody could ever see fire, which this package has already paid for
/// twice. The bound over *author* input, which is the one the programme's
/// constraint is about, is enforced once at
/// [`build_plan`](crate::executor::build_plan).
pub(crate) fn descend(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
    now: Tick,
) -> Step {
    let errors = plan.tasks[task].errors.as_slice();
    let step = match &plan.tasks[task].kind {
        TaskKind::Runnable { .. } => return Step::Waiting,
        // Waiting for the same reason a `Runnable` is, on a different thing:
        // its terminal comes from the child execution reaching one, which
        // `worker::subflow::on_child_terminal` reports. `advance()` cannot see
        // a sub-execution at all -- it is pure over `(plan, state, now)` and
        // `ExecutionState` holds nothing about one -- which is the same
        // division of labour `TaskKind::Loop` already has.
        TaskKind::Executable { .. } => return Step::Waiting,
        TaskKind::Sequential { tasks } => {
            sequential::drive(plan, state, tasks, errors, Some(run), now)
        }
        // Returned without the `Done` mapping below, for the reason
        // `allow_failure::drive` gives: its terminal is already the block's.
        TaskKind::AllowFailure { tasks, finally } => {
            return allow_failure::drive(plan, state, run, task, tasks, finally, now)
        }
        TaskKind::Parallel { tasks, concurrency } => {
            parallel::drive(plan, state, tasks, errors, Some(run), *concurrency, now)
        }
        TaskKind::Dag { nodes } => dag::drive(plan, state, nodes, errors, Some(run), now),
        TaskKind::If { then_, else_, .. } => {
            branch::drive_if(plan, state, run, then_, else_, errors, now)
        }
        TaskKind::Switch { cases, default, .. } => {
            branch::drive_switch(plan, state, run, cases, default, errors, now)
        }
        TaskKind::Loop { .. } => return Step::Waiting,
        // Not returned: its `Done` is the block's, and it takes the same
        // `allowFailure:` mapping below as every other flowable.
        TaskKind::WorkingDirectory { tasks } => {
            working_directory::drive(plan, state, run, tasks, now)
        }
        // W8 Task 3. It is neither `Waiting` (there is a command to emit, and
        // the executor is the only thing that can emit it) nor a list to walk:
        // the whole of a `Pause` is the transition into `PAUSED`, which
        // `pause::drive` decides from the plan and the clock alone.
        //
        // Its `Step` is returned WITHOUT going through the `Done` mapping
        // below, because a pause does not terminate in the round it starts and
        // never returns `Done` at all -- the terminal is
        // [`resume_paused`]'s, one clock move later.
        TaskKind::Pause { .. } => return pause::drive(plan, state, run, task),
        // W8 Task 4. It DOES go through the `Done` mapping below, unlike the
        // `Pause` above: a `LoopUntil` terminates in the round its last
        // iteration ends, and the `Step::Commands` it returns instead is a
        // `PauseRun` that must not be mapped either -- which is what the match
        // below already gets right for every other flowable.
        TaskKind::LoopUntil { .. } => loop_until::drive(plan, state, run, task, now),
    };
    match step {
        // **The single door `allowFailure:`/`allowWarning:` go through for a
        // flowable** -- W8 Task 5. In the artifact the mapping is the tail of
        // `Execution.guessFinalState`, which every `FlowableTask.resolveState`
        // calls with its own `isAllowFailure()`/`isAllowWarning()`; here every
        // one of those calls has already come back as a `Step::Done`, so one
        // application covers `Sequential`, `Parallel`, `Dag`, `If`, `Switch`
        // and `LoopUntil` alike -- including `If`/`Switch`'s no-branch answer,
        // which Java routes through `guessFinalState(null, ...)` rather than
        // around it.
        //
        // It is NOT applied to the top-level list: `advance` drives
        // `plan.roots` with no parent run and turns its `Done` into a
        // `SetExecutionState`, and Kestra's flow-level call is
        // `guessFinalState(flow)` -- `(false, false)`, a flow has no task
        // flags. That asymmetry is why this lives here and not in
        // [`aggregate`].
        Step::Done(s) => {
            let t = &plan.tasks[task];
            let state = crate::executor::allow_failure::soften(s, t.allow_failure, t.allow_warning);
            Step::Commands(vec![Command::SetRunState { run, state }])
        }
        other => other,
    }
}
