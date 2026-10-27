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
/// fails for good, after which its `errors:` list is driven **sequentially**,
/// and then (on success too) its `finally:`, also sequentially.
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    children: &[TaskIx],
    errors: &[TaskIx],
    finally: &[TaskIx],
    parent: Option<RunIx>,
    concurrency: Option<u32>,
    now: Tick,
) -> Step {
    drive_lists(
        plan,
        state,
        children,
        errors,
        finally,
        parent,
        concurrency,
        now,
        |_, _| true,
    )
}

/// [`drive`] with the `tasks:` list's `startable` predicate as an argument:
/// `Parallel` passes `true`, `Dag` its `dependsOn` gate. Kestra's
/// `resolveDagNexts` is `resolveParallelNexts` with that filter, and both ask
/// `findTaskDependingFlowState` first -- which is [`current_list`] here.
///
/// End hooks A4: before A4 a `Dag` with an `errors:` was handed to [`drive`],
/// whose predicate is `true`, so its `dependsOn` was ignored while `tasks:`
/// ran (no corpus `Dag` had an `errors:`; `valids/finally-dag` has one).
#[allow(clippy::too_many_arguments)]
pub(crate) fn drive_lists(
    plan: &Plan,
    state: &ExecutionState,
    children: &[TaskIx],
    errors: &[TaskIx],
    finally: &[TaskIx],
    parent: Option<RunIx>,
    concurrency: Option<u32>,
    now: Tick,
    startable: impl Fn(TaskIx, &dyn Fn(TaskIx) -> Option<RunIx>) -> bool,
) -> Step {
    match current_list(plan, state, children, errors, finally, parent, now) {
        // A branch that will be retried holds the whole `Parallel` open, for the
        // reason `sequential::drive` records: `resolveState`'s empty arm
        // returns a state only when `hasFailedNoRetry`.
        Current::RetryPending => return Step::Waiting,
        // `errors:` and `finally:` alike: one child at a time, after the
        // still-live siblings of a failure are failed (spec §4.1). On the way
        // into `finally` after a success every LIVE run under the siblings is
        // already terminal; a run a `LoopUntil` restart emptied and never
        // rebuilt is not live, and `fail_the_rest` skips it (review M3), so
        // it has nothing to do there.
        Current::Errors(list) | Current::Finally(list) => {
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
    drive_concurrent(plan, state, children, parent, concurrency, now, startable)
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
/// `Execution.guessFinalState` priority order, which [`RunState`] can now hold
/// in full: `KILLED` beats `FAILED` beats `CANCELLED` beats `WARNING` beats the
/// `SUCCESS` it starts from.
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
    // **The predicate is `isFailed()` -- `FAILED` and nothing else -- and it
    // was an EXCLUSION LIST until W8 Task 5 fix round 2.** It read
    // `s.is_terminal() && !matches!(s, RunState::Success | RunState::Skipped)`,
    // which is the same set only while every OTHER terminal is a failure. It
    // stopped being that twice and nobody noticed either time:
    //
    // * `Retried` (Task 7) is `isTerminatedNoFail()`'s, not a failure, and
    // * `Warning` (W8 Task 5) is `isTerminatedNoFail()`'s FIRST member.
    //
    // So a `WARNING` sibling made this `Parallel` `fail_the_rest` -- **egret
    // failing runs Kestra succeeds**, the divergence class the paragraph this
    // one replaces already forbade, arriving by the exact route that paragraph
    // was written about. Reading it as an allow-list is what makes the next
    // variant inert here instead of a defect: `isFailed()` names what it
    // includes.
    //
    // **Which Java predicate this IS, corrected** -- the first version of this
    // comment named three lambdas and said all three were
    // `getState().isFailed()` and nothing else, which is false for one of them
    // and shipped three times inside the round that wrote it (here, the
    // report, and the new test's doc).
    //
    // `findTaskDependingFlowState` calls TWO predicates, in order
    // (`javap -p -c io/kestra/core/models/executions/Execution.class`):
    //
    // | bci | call | lambda | what it is |
    // |---|---|---|---|
    // | 110 | `hasFailed(List, TaskRun)` | `lambda$hasFailed$1` | `getState().isFailed()`, four instructions |
    // | 120 | `hasFailedNoRetry(List, TaskRun)` | `lambda$hasFailedNoRetry$0` | `isFailed() && shouldNotBeRetried(...)`, **bci 0-24 with a branch** |
    //
    // **This line is `hasFailed`, the one at bci 110**, and that one really is
    // `isFailed()` alone. `State.Type.isFailed()` is `this == FAILED`.
    //
    // **The retry clause is the SECOND gate and it belongs to the CALLER, not
    // here** -- corrected at W8 Task 5 fix round 4, which found that fix round
    // 3's account of it was wrong.
    //
    // Round 3 wrote *"`fail_the_rest` does NOT have that clause ... registered
    // as a divergence"*. The clause was never missing from this rule: it is
    // [`current_list`](super::current_list)'s `Current::RetryPending`, whose
    // `failed.iter().all(will_retry)` is the exact complement of
    // `hasFailedNoRetry`'s `anyMatch`, and [`drive`] above returns
    // `Step::Waiting` on it **before** `drive_concurrent` is ever called. A
    // `Parallel` child mid-retry therefore never reaches this line, which is
    // why probe RP -- adding the clause here -- changed nothing and survived.
    //
    // **What round 3 measured was real and the diagnosis was one layer off.**
    // The path that skipped the gate was [`dag::drive`](super::dag::drive):
    // with an empty `errors:` it called `drive_concurrent` directly, never
    // asking `current_list` at all. That is where the gate went, and
    // `a_dag_node_awaiting_a_retry_does_not_fail_its_siblings` is the row that
    // holds it. The lesson is the shape of the error, not the line: **"this
    // predicate lacks a clause" and "one CALLER of it skips the gate" produce
    // the same symptom and have different fixes**, and a probe that adds the
    // clause to the predicate cannot tell them apart -- it survives under both.
    //
    // `SKIPPED` stays out for W3c's reason, unchanged: a falsy `runIf:` on one
    // branch must not fail the others, and
    // `no_corpus_parallel_or_dag_child_declares_a_run_if` derives that no
    // corpus flow can score it. `a_skipped_branch_does_not_fail_its_siblings`
    // holds that row; `a_warning_branch_does_not_fail_its_siblings` holds this
    // one, and the oracle cannot score either.
    let ended_badly = runs
        .iter()
        .flatten()
        .any(|&ix| latest(state, ix) == Some(RunState::Failed));
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
    //
    // # Every `Step` is accounted for, and none is dropped by an `if let`
    //
    // The three arms below used `if let Step::Commands(inner)`, which silently
    // discards anything else. Measured at W8 Task 3 fix round 1: **none of the
    // three can return [`Step::Done`] today** -- [`open`] returns `Commands` or
    // `Waiting` and its only delegating arm, `loop_::open`, does the same;
    // [`resume_paused`](super::resume_paused) returns `Commands` or `Waiting`;
    // and [`descend`] returns
    // `Done` from none of its paths.
    //
    // **That last clause was stated imprecisely in fix round 1 and the fix
    // round 2 re-review caught it.** It read *"`descend` maps `Done` into a
    // `SetRunState` before it returns"*, which is true of the arms that fall
    // through to its trailing `match` and NOT of the two that `return` early --
    // `TaskKind::Executable`, `TaskKind::Loop` and `TaskKind::Runnable` return
    // `Waiting` directly, and `TaskKind::Pause` returns `pause::drive`'s value
    // directly, neither of them touching the mapping. The conclusion survives
    // because those early returns are themselves `Waiting` or `Commands`, but
    // it needs both halves and the round-1 wording only had one.
    //
    // So the `if let` was dropping `Waiting` and nothing else, which is correct
    // -- a sibling with nothing to ask for must not stop the siblings that
    // have. It is written as an exhaustive `match` anyway, because the fact
    // above is a property of three OTHER functions and nothing here would
    // notice if one of them changed.
    //
    // **The `Done` arm is DEAD CODE, not a guard, and the difference matters
    // here.** A guard is a branch that fires on a condition; this one cannot
    // fire at all. It earns its line only by making the drop explicit rather
    // than implicit -- and it is deliberately not `unreachable!()`, because a
    // panic arm nobody can drive is what this crate has twice paid for
    // (`descend`'s deleted depth guard says so in as many words). No test
    // drives it and none can without faking a `Step` the real callees never
    // produce, which would pin the fake rather than the seam.
    let mut cmds = Vec::new();
    for (&task, run) in children.iter().zip(&runs) {
        let Some(ix) = *run else { continue };
        let step = match latest(state, ix) {
            // W8 Task 4, and the same door `sequential::walk` opens for the
            // same state: a run thrown away by `Command::RestartLoop` is
            // re-created when the walk reaches it. **No corpus flow reaches
            // this arm**, because no corpus `LoopUntil` sits INSIDE a
            // `Parallel` or a `Dag` — the containment runs the other way, and
            // `valids/loopuntil-failed-flowable` is the one instance of it.
            // It is here for the reason the `PAUSED` arm below is: the
            // alternative is not a guard nobody sees fail but a stall.
            //
            // **This sentence said "no corpus `LoopUntil` CONTAINS a
            // `Parallel` or a `Dag`" and was false the day it was written** —
            // thirty lines above the identical one fix round 1 corrected, and
            // missed by that round for the reason the whole branch keeps
            // paying: a text edit fixes the sentence it is looking at. Both
            // instances are compared by a program now
            // (`the_corpus_pauses_that_sit_inside_another_flowable_are_named`),
            // which is the only thing that makes the third instance fail
            // rather than wait to be found.
            None => Step::Commands(vec![Command::RecreateRun { run: ix }]),
            Some(RunState::Created) => open(plan, state, ix, task, now),
            Some(RunState::Running) => descend(plan, state, ix, task, now),
            // **The same one door `sequential::walk` opens**, and for the
            // same reason it is one door: a `PAUSED` run is the only
            // non-terminal state no worker is holding, so somebody has to ask
            // it every round whether its wait is over. W8 Task 4 fix round 2
            // replaced a `TaskKind`-keyed guard here -- which stalled anything
            // not on its list, measured under `Parallel` AND `Dag`, both of
            // which drive through this function -- with
            // `super::resume_paused`, which reads `TaskRun::resume_with`.
            //
            // **No corpus flow reaches this arm** and both halves of that are
            // derived rather than remembered: every corpus `Pause` under
            // another flowable is unbounded and absent from `golden.json`, and
            // no corpus `LoopUntil` sits inside a `Parallel` or a `Dag` at all
            // -- the containment runs the other way, `valids/loopuntil-failed-
            // flowable` putting a `Dag` INSIDE a `LoopUntil`. Both are asserted
            // by `the_corpus_pauses_that_sit_inside_another_flowable_are_named`.
            // The arm is here because the alternative is not a guard nobody
            // sees fail, it is a stall.
            Some(RunState::Paused) => super::resume_paused(state, ix, now),
            _ => continue,
        };
        match step {
            Step::Commands(inner) => cmds.extend(inner),
            // In flight with nothing to ask for. Its siblings still might, so
            // this keeps walking rather than returning.
            Step::Waiting => {}
            // Unreachable today -- see the block above -- and this is
            // `descend`'s own mapping, so a child list that finished takes its
            // terminal rather than being swallowed.
            Step::Done(s) => cmds.push(Command::SetRunState { run: ix, state: s }),
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
        // An empty history is `Command::RestartLoop`'s "this run does not
        // exist at the moment" (a loop iteration that never rebuilt it), not
        // a live run: failing it would persist a ghost `FAILED` (review M3).
        .filter(|&ix| !state.runs[ix].history.is_empty() && !is_terminal(state, ix))
        .map(|run| Command::SetRunState {
            run,
            state: RunState::Failed,
        })
        .collect()
}

/// Kestra's `Execution.guessFinalState`, over the runs a branch created.
///
/// Java's priority is `KILLED > FAILED > CANCELLED > WARNING > PAUSED`, over a
/// single pass that breaks on `KILLED`
/// (`core/src/main/java/io/kestra/core/models/executions/Execution.java:774-789`).
///
/// # This is the SECOND copy of that pass and it was two variants behind the
/// first — W8 Task 6
///
/// [`super::aggregate`] is the other, and this one carried only `KILLED` and
/// `FAILED` because the doc it inherited said `WARNING` and `CANCELLED` were
/// *"states this enum cannot represent"*. That was true when it was written and
/// **stopped being true at W8 Task 5**, which added `RunState::Warning` and
/// updated [`super::aggregate`]'s chain without touching this one — so a
/// `Parallel`/`Dag` branch with a `WARNING` child answered `SUCCESS` where the
/// artifact answers `WARNING`. That is not a state this corpus can put under a
/// branch (`valids/task-allow-failure-flowable` and its three siblings are
/// baselined on the `AllowFailure` FLOWABLE, a different construct), so no
/// figure moved in either direction and nothing could have noticed; it was
/// found by W8 Task 6 enumerating **every** consumer of a terminality
/// judgement before adding `Cancelled` to any of them, which is the method W8
/// Task 5 fix round 2 bought.
///
/// Both variants are here now and the two copies agree, asserted by
/// `the_two_guess_final_state_copies_agree`.
///
/// `PAUSED` is still left out, for a reason that is a property of the CALLER
/// rather than of this order: rule 4 runs this only when every child run is
/// terminal, and `PAUSED` is not. It sits below `WARNING` in the priority
/// anyway.
pub(crate) fn aggregate_created(state: &ExecutionState, runs: &[RunIx]) -> RunState {
    let mut out = RunState::Success;
    for &ix in runs {
        match latest(state, ix) {
            Some(RunState::Killed) => return RunState::Killed,
            Some(RunState::Failed) => out = RunState::Failed,
            Some(RunState::Cancelled) if out != RunState::Failed => out = RunState::Cancelled,
            Some(RunState::Warning) if out != RunState::Failed && out != RunState::Cancelled => {
                out = RunState::Warning
            }
            _ => {}
        }
    }
    out
}
