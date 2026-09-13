//! `Parallel` — and, through [`drive_concurrent`], the half of `Dag` that is
//! not the dependency predicate.
//!
//! Kestra's `Parallel` and `Dag` are one function with one argument different:
//! `Dag.resolveNexts` calls `FlowableUtils.resolveDagNexts`, which calls
//! `resolveParallelNexts` with a filter over the same stream
//! (`core/src/main/java/io/kestra/core/runners/FlowableUtils.java`). This
//! module is that function; `dag.rs` is that filter.

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx};
use crate::executor::state::{ExecutionState, RunIx, RunState, Tick};

use super::{
    aggregate, current_list, descend, is_terminal, latest, open, run_of, sequential, Current, Step,
};

/// Drive one `Parallel` task list: every child is startable, always — until one
/// fails for good, after which its `errors:` list is driven **sequentially**.
///
/// "Sequentially" is Kestra's own word, not an inference:
/// `FlowableUtils.resolveParallelNexts` compares the chosen list against the
/// task list and, when they differ, hands the whole thing to
/// `resolveSequentialNexts` under the comment "errors & finally must be run as
/// sequential tasks". Its `resolveState` is `resolveSequentialState`, the very
/// same function `Sequential` uses, so the *state* half was already shared.
///
/// The still-in-flight siblings are failed first, by the same
/// [`fail_the_rest`] the no-`errors:` path uses. That composition is not
/// exercised by any corpus flow — no `Parallel` in the corpus has both an
/// `errors:` and a sibling still running when one branch fails — so it is
/// stated here rather than claimed to be measured.
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    children: &[TaskIx],
    errors: &[TaskIx],
    parent: Option<RunIx>,
    concurrency: Option<u32>,
    now: Tick,
) -> Step {
    match current_list(plan, state, children, errors, parent, now) {
        // A branch that will be retried holds the whole `Parallel` open, for the
        // reason `sequential::drive` records: `resolveState`'s empty arm
        // returns a state only when `hasFailedNoRetry`.
        Current::RetryPending => return Step::Waiting,
        Current::Errors(list) => {
            let cmds = fail_the_rest(
                state,
                &children
                    .iter()
                    .map(|&task| run_of(state, parent, task))
                    .collect::<Vec<_>>(),
            );
            if !cmds.is_empty() {
                return Step::Commands(cmds);
            }
            return match sequential::walk_errors(plan, state, list, parent, now) {
                Some(step) => step,
                None => Step::Done(aggregate(state, children, parent)),
            };
        }
        // `Ended` falls through on purpose: rules 1 and 4 below are Task 4's
        // measured account of exactly that case, and re-deriving it here would
        // be a second copy of `fail_the_rest`'s three measurements.
        Current::Tasks(_) | Current::Ended => {}
    }
    drive_concurrent(plan, state, children, parent, concurrency, now, |_, _| true)
}

/// Drive one concurrently-scheduled task list.
///
/// `startable` decides which not-yet-created children may be created in this
/// round; it is given the child's [`TaskIx`] and a lookup from a sibling's
/// `TaskIx` to its run, if it has one. `Parallel` answers `true` for every
/// child; `Dag` answers with its `dependsOn` predicate.
///
/// # The four rules, in the order they are applied
///
/// **1. A child that ended badly ends the branch.** Nothing further is created,
/// and every run in the branch's subtree that has not terminated is `FAILED`.
/// See [`fail_the_rest`] for the three measurements this rests on, including
/// the one that bounds it — a sibling that had *already* finished keeps the
/// state it finished with.
///
/// **2. Create, and create the whole eligible batch at once.** Kestra's
/// `resolveParallelNexts` returns *every* not-yet-created task in one call, and
/// this is load-bearing: `taskRunList` is **creation** order, proven on a flow
/// whose three branches sleep 4s, 2s and 1s — the list stayed
/// `a_slow, b_mid, c_fast`, which a list ordered by completion would have
/// inverted. A `drive` that created children lazily as earlier ones finished
/// would order them by completion instead.
///
/// **Kestra's other half of that call is deliberately not here, and what makes
/// it unreachable is the CALLER, not this function.**
/// `resolveParallelNexts` also returns nothing at all while any existing child
/// run is still `CREATED` ("first created, leave"). It was ported, and then a
/// mutation that bypassed it (`if !any_created || true`) left the whole crate at
/// 447 passed / 0 failed with the ratchet unmoved — so it was deleted rather
/// than shipped with a comment nobody could check.
///
/// **Deleting it is right for today's only caller and is not a property of this
/// function.** `advance` is a pure function of the state it is handed, and any
/// caller may hand it any state. Instrumenting this exact decision point over
/// all 294 golden entries measures the condition — a batch about to be created
/// while an existing child run of the same branch is still `CREATED` —
/// occurring **0 times** under the oracle's environment, which never applies a
/// worker result while `advance` still has work to do. Under a driver that
/// delivers **one worker result between `advance` calls**, the same
/// instrumentation fires on `valids/dag` (1 batch) and `valids/if` (3). Nothing
/// about a branch has to change for that; only the delivery discipline does.
///
/// So the trigger to watch for is **W4's worker**, which is message-driven by
/// construction — not a later flowable. Whoever gives `advance` a caller that
/// can interleave a completion with a decision has to bring this guard back,
/// and bring the test that makes it fire with it.
///
/// **3. Start every created child, and descend into every running flowable, in
/// the same round.** Also load-bearing, and separately measured: in
/// `valids/parallel-disabled-tasks` two `Sequential` branches of one `Parallel`
/// have their first children created adjacently in `taskRunList`
/// (`... sequence1, sequence2, task1, task3, task2, task4`), which a driver
/// that returned the first branch's work and stopped could not produce — it
/// would finish branch one before creating anything in branch two.
///
/// **4. Terminate when every child run has.** The state is Kestra's
/// `Execution.guessFinalState` priority order, narrowed to what [`RunState`]
/// can hold: `KILLED` beats `FAILED` beats `SUCCESS`.
pub(crate) fn drive_concurrent(
    plan: &Plan,
    state: &ExecutionState,
    children: &[TaskIx],
    parent: Option<RunIx>,
    concurrency: Option<u32>,
    now: Tick,
    startable: impl Fn(TaskIx, &dyn Fn(TaskIx) -> Option<RunIx>) -> bool,
) -> Step {
    let runs: Vec<Option<RunIx>> = children
        .iter()
        .map(|&task| run_of(state, parent, task))
        .collect();
    let run_for = |task: TaskIx| -> Option<RunIx> {
        children
            .iter()
            .position(|&t| t == task)
            .and_then(|i| runs[i])
    };

    // Rule 1.
    //
    // **`SKIPPED` is excluded, and W3c is why.** Through W3a "terminal and not
    // `SUCCESS`" and "terminated in error" were the same set, because every
    // state this enum could hold that was terminal and not `SUCCESS` was a
    // failure. `Skipped` is the first that is not: `State.java:293` lists it in
    // `isTerminatedNoFail()` beside `SUCCESS`, and `Execution.guessFinalState`
    // has no arm for it at all. Without this exclusion a falsy `runIf:` on one
    // branch of a `Parallel` would `fail_the_rest` of its siblings -- egret
    // failing runs Kestra succeeds, which is the divergence class this
    // programme forbids. No corpus flow puts a `runIf:` on a `Parallel` or
    // `Dag` child (`no_corpus_parallel_or_dag_child_declares_a_run_if` derives
    // that on every run), so this is Kestra's Java stated as a rule rather than
    // a live measurement, and `a_skipped_branch_does_not_fail_its_siblings` is
    // what holds it.
    let ended_badly = runs.iter().flatten().any(|&ix| {
        latest(state, ix)
            .is_some_and(|s| s.is_terminal() && !matches!(s, RunState::Success | RunState::Skipped))
    });
    if ended_badly {
        let cmds = fail_the_rest(state, &runs);
        if !cmds.is_empty() {
            return Step::Commands(cmds);
        }
    } else {
        // Rule 2.
        let in_flight = runs
            .iter()
            .flatten()
            .filter(|&&ix| !is_terminal(state, ix))
            .count();
        let room = match concurrency {
            None => usize::MAX,
            Some(cap) => (cap as usize).saturating_sub(in_flight),
        };
        let batch: Vec<Command> = children
            .iter()
            .zip(&runs)
            .filter(|(_, run)| run.is_none())
            .map(|(&task, _)| task)
            .filter(|&task| startable(task, &run_for))
            .take(room)
            .map(|task| Command::CreateRun {
                task,
                parent,
                value: None,
            })
            .collect();
        if !batch.is_empty() {
            return Step::Commands(batch);
        }
    }

    // Rule 3.
    let mut cmds = Vec::new();
    for (&task, run) in children.iter().zip(&runs) {
        let Some(ix) = *run else { continue };
        match latest(state, ix) {
            Some(RunState::Created) => {
                if let Step::Commands(inner) = open(plan, state, ix, task, now) {
                    cmds.extend(inner);
                }
            }
            Some(RunState::Running) => {
                if let Step::Commands(inner) = descend(plan, state, ix, task, now) {
                    cmds.extend(inner);
                }
            }
            // W8 Task 3, and it is the same door `sequential::walk` opens for
            // the same state: a `PAUSED` run is the one non-terminal state no
            // worker is holding, so somebody has to ask it every round whether
            // its bound has elapsed. Without this arm a `Pause` inside a
            // `Parallel` or a `Dag` pauses and is never asked again.
            //
            // **No corpus flow reaches it** -- the six corpus `Pause`es that
            // sit under a `Parallel`, a `Dag` or a `Loop` are all unbounded and
            // none is in `golden.json`
            // (`the_corpus_pauses_that_sit_inside_another_flowable_are_named`).
            // It is here rather than left out because the alternative is not "a
            // guard nobody sees fail" but a stall, and
            // `two_pauses_in_one_parallel_resume_independently` drives it.
            Some(RunState::Paused)
                if matches!(plan.tasks[task].kind, crate::executor::plan::TaskKind::Pause { .. }) =>
            {
                if let Step::Commands(inner) = super::pause::resume(plan, state, ix, task, now) {
                    cmds.extend(inner);
                }
            }
            _ => {}
        }
    }
    if !cmds.is_empty() {
        return Step::Commands(cmds);
    }

    // Rule 4. A child with no run at all leaves the branch unfinished, except
    // when rule 1 has already closed it: a `Dag` whose dependency failed never
    // creates the nodes downstream of it, and a branch that waited for a run
    // that will never exist would be STUCK rather than FAILED.
    let created: Vec<RunIx> = runs.iter().flatten().copied().collect();
    let every_run_terminal = created.iter().all(|&ix| is_terminal(state, ix));
    if every_run_terminal && (created.len() == children.len() || ended_badly) {
        return Step::Done(aggregate_created(state, &created));
    }
    Step::Waiting
}

/// Rule 1's commands: fail everything under this branch that is still going.
///
/// **Measured, and narrower than the sentence it started as.** The design doc
/// records "one bad sibling failed all three" from a spike whose three parallel
/// branches slept 4s, 2s and 1s and whose fastest branch had a validation
/// error: the other two ended `FAILED`. The golden reproduces that shape on
/// `valids/parallel-fail-with-flowable`, where the branch holding a
/// `Sleep: PT1S` ends `FAILED` — a `Sleep` that succeeds in every other flow
/// in the corpus, `PT30S` included.
///
/// But it is **not** true that a bad sibling fails every sibling, and the same
/// golden says so: in `valids/finally-parallel`, driven with `failed: true`,
/// the `Parallel` has four branches and ends `FAILED` while `p1`, `p2` and
/// `p3` all end `SUCCESS`. What separates the two is *when*: a sibling that had
/// already terminated keeps the state it terminated with, and a sibling still
/// in flight does not survive. So the rule is about the runs still going, which
/// is what this function collects.
///
/// It reaches the whole subtree and not only the direct children, because
/// `parallel-fail-with-flowable`'s `sleep` is a **grandchild** — the failure
/// was in the other branch entirely, and both the branch flowable `if-1` and
/// the `sleep` under it end `FAILED`.
fn fail_the_rest(state: &ExecutionState, runs: &[Option<RunIx>]) -> Vec<Command> {
    let mut frontier: Vec<RunIx> = runs.iter().flatten().copied().collect();
    let mut doomed: Vec<RunIx> = Vec::new();
    // A run's parent is fixed at creation to a run that already existed, so
    // parent indices strictly decrease and this walk visits each run at most
    // once for any state at all.
    while let Some(ix) = frontier.pop() {
        if !doomed.contains(&ix) {
            doomed.push(ix);
            frontier.extend(
                (0..state.runs.len()).filter(|&child| state.runs[child].parent == Some(ix)),
            );
        }
    }
    doomed.sort_unstable();
    doomed
        .into_iter()
        .filter(|&ix| !is_terminal(state, ix))
        .map(|run| Command::SetRunState {
            run,
            state: RunState::Failed,
        })
        .collect()
}

/// Kestra's `Execution.guessFinalState`, narrowed to the states [`RunState`]
/// can hold.
///
/// Java's priority is `KILLED > FAILED > CANCELLED > WARNING > PAUSED`, over a
/// single pass that breaks on `KILLED`
/// (`core/src/main/java/io/kestra/core/models/executions/Execution.java:769`).
///
/// **This said `CANCELLED`, `WARNING` and `PAUSED` were "three of the six
/// states this enum still cannot represent", and by W8 Task 2 both halves of
/// that were stale.** The count was the enum's seven-member era (13 golden
/// states minus 7); it had been 4 since W3c and is **2** now — `WARNING` (10
/// entries) and `CANCELLED` (3), owned by `AllowFailure`/`allowFailure:` and by
/// `sla:`/`Exit`. `PAUSED` **is** representable from W8 Task 2, so it is left
/// out of this aggregate for a different reason, and a measured one: it is not
/// terminal, `aggregate_created` runs only over runs that are, and `PAUSED`
/// sits below `WARNING` in the priority above anyway. What is left of the order
/// here is `KILLED`, then `FAILED`, then the `SUCCESS` it starts from.
fn aggregate_created(state: &ExecutionState, runs: &[RunIx]) -> RunState {
    let mut out = RunState::Success;
    for &ix in runs {
        match latest(state, ix) {
            Some(RunState::Killed) => return RunState::Killed,
            Some(RunState::Failed) => out = RunState::Failed,
            _ => {}
        }
    }
    out
}
