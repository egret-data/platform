//! `allowFailure:` / `allowWarning:` — the terminal a task's own two flags
//! decide, and the four sites the artifact reads them at.
//!
//! **This module is the `allowFailure:` FLAG, not the `AllowFailure`
//! flowable.** They are different constructs with the same word in them:
//! `io.kestra.plugin.core.flow.AllowFailure` is a class that `extends
//! Sequential` and is still planned opaque here
//! ([`plan::TaskKind::Runnable`](super::plan::TaskKind)) -- and refused at
//! validate as `unsupported_allow_failure` since no-silent-stall Task 1 --
//! while
//! `allowFailure:` is a `Task.java` property every task in the language may
//! write. Four of `oracle-exec/baseline.txt`'s twelve entries for this row hold
//! the class and are out of this module's reach; the other eight hold the flag.
//!
//! # Step 1: the mapping, measured on the pinned artifact in both directions
//!
//! Decompiled from `/app/kestra` in
//! `kestra/kestra@sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd`
//! with `javap -p -c` under a JDK 25 (the classes are class-file v69).
//!
//! **The artifact spells this table twice, and the two spellings are not the
//! same function.**
//!
//! ## Spelling A — a CONSTRUCTION with no source state
//!
//! `io.kestra.core.models.flows.State$Type.fail(io.kestra.core.models.tasks.Task)`:
//!
//! ```text
//! 1: invokevirtual Task.isAllowFailure:()Z
//! 4: ifeq 26                      // false  -> 26
//! 8: invokevirtual Task.isAllowWarning:()Z
//! 11: ifeq 20                     // false  -> 20
//! 14: getstatic SUCCESS           // both true
//! 20: getstatic WARNING           // allowFailure only
//! 26: getstatic FAILED            // neither
//! ```
//!
//! It takes **no state at all**. `allowWarning` does not map a `WARNING` here —
//! there is no `WARNING` to map *from*; the three-way answer is built out of
//! two booleans. And `isAllowWarning()` is not even read when `isAllowFailure()`
//! is false: `allowWarning:` alone changes nothing at this site.
//!
//! ## Spelling B — a two-step CONVERSION keyed on a state that already exists
//!
//! `io.kestra.worker.processors.WorkerTaskProcessor` (bytecode 994-1055, the
//! nominal runnable path), `io.kestra.core.models.executions.Execution
//! .guessFinalState` (188-235) and `io.kestra.core.runners.ExecutableUtils
//! .guessState` (50-102) all write it as two independent `if`s:
//!
//! ```text
//! if (allowFailure && <no retry pending> && state.isFailed()) state = WARNING;
//! if (allowWarning && WARNING.equals(state))                  state = SUCCESS;
//! ```
//!
//! The two spellings agree for every state B can be handed that A can produce.
//! **They differ on one input and it is the input this package's plan assumed**:
//! handed a state that is ALREADY `WARNING`, spelling B maps it to `SUCCESS`
//! under `allowWarning:` alone, and spelling A cannot see it.
//!
//! ## What that settled
//!
//! The plan says two of this row's entries *"need no new state at all —
//! `allowWarning:` maps `WARNING -> SUCCESS`, which the existing enum already
//! expresses"*. The measurement says the conclusion is right, the count is
//! wrong, and the reason is not the one given:
//!
//! - **THREE, not two**, corpus entries reach a terminal this enum can already
//!   hold: `valids/task-allow-warning-runnable`, `…-flowable` and
//!   `…-executable-flow`. All three declare `allowFailure: true` **and**
//!   `allowWarning: true`.
//! - **None of the three is a member of "the eight"** the residual table names
//!   for this row. Those eight are the eight golden entries that HOLD a
//!   `WARNING` symbol, and an entry holding `WARNING` is by construction one
//!   this enum cannot reproduce. The three above hold none.
//! - **The reason is spelling A, not "the existing enum expresses it".** On the
//!   three flows the incoming state is `FAILED` and both flags are on, so the
//!   answer is `SUCCESS` in one step. The intermediate `WARNING` in spelling B
//!   is a Java local that is never written to any history — which is why the
//!   golden run for `valids/task-allow-warning-executable-flow` reads
//!   `["CREATED","RUNNING","FAILED","SUCCESS"]` with no `WARNING` between them.
//!
//! # Where the artifact reads the flags
//!
//! | site | what it decides | corpus entry that reaches it |
//! |---|---|---|
//! | `WorkerTaskProcessor` nominal path (spelling B) | a **runnable**'s own terminal | `valids/task-allow-{failure,warning}-runnable` |
//! | `Execution.guessFinalState` tail (spelling B) | a **flowable**'s aggregate | `valids/task-allow-{failure,warning}-flowable` |
//! | `ExecutableUtils.guessState` (spelling B) | a **`Subflow`**'s terminal | `valids/task-allow-{failure,warning}-executable-flow` |
//! | `ExecutorService.java:937` (spelling A) | a **`Pause`**'s `timeout:` terminal | `valids/pause-timeout-allow-failure` |
//!
//! The fourth row is the one that would be missed by reading the flag as "a
//! property of failing tasks": `.state(duration != null ? behavior.mapToState()
//! : State.Type.fail(pauseTask))` applies the mapping to a `timeout:` bound and
//! **not** to a `pauseDuration:` one, which takes `behavior.mapToState()`
//! instead. `allowFailure: true` on a `pauseDuration:` pause therefore changes
//! nothing at all.
//!
//! # The retry gate, which is part of the mapping and not an optimisation
//!
//! Spelling B's runnable site is gated on `!taskRun.shouldBeRetried(task
//! .getRetry())`. Without it `valids/task-allow-failure-runnable` — three
//! attempts under `retry: {maxAttempts: 3}` — would soften its FIRST failure,
//! and a softened run is not `FAILED`, so [`retry::scan`](super::retry::scan)
//! would never reopen it: one attempt where the golden has three. The golden is
//! explicit about which attempt is softened: the run reads
//! `… FAILED RETRYING … FAILED RETRYING … WARNING` and **all three attempts end
//! `FAILED`**. Only the run's own state moves.

use super::plan::Plan;
use super::retry;
use super::state::{ExecutionState, RunIx, RunState, Tick};

/// **Spelling B, whole** — W8 Task 5 Step 3 closed the hole Step 2 named.
///
/// Step 2 shipped this with `(Failed, true, false)` returning `Failed`, because
/// `RunState` had no `Warning` to return; Step 3 added the variant and this is
/// the artifact's table with nothing left out.
///
/// Written as one `match` over the triple rather than as Java's two sequential
/// `if`s, and the two are equal by cases rather than by resemblance:
///
/// | state | `allow_failure` | `allow_warning` | Java | here |
/// |---|---|---|---|---|
/// | `FAILED` | false | any | neither `if` fires | `Failed` |
/// | `FAILED` | true | false | first fires, second does not | `Warning` |
/// | `FAILED` | true | true | first fires, then the second | `Success` |
/// | `WARNING` | any | true | only the second fires | `Success` |
/// | `WARNING` | any | false | neither | `Warning` |
/// | anything else | any | any | neither | unchanged |
///
/// The `(Warning, _, true)` row is the one this function cannot be handed by
/// any corpus flow and is implemented anyway, for the reason
/// `worker::subflow::guess_state`'s `transmitFailed: false` row is: a
/// differential oracle's job is the behaviour and not the subset of it one
/// corpus happens to reach. It is also the ONLY row that separates spelling B
/// from spelling A — see this module's doc — so leaving it out would make the
/// two indistinguishable and quietly turn this into [`fail`].
pub(crate) fn soften(state: RunState, allow_failure: bool, allow_warning: bool) -> RunState {
    match (state, allow_failure, allow_warning) {
        // Java's first `if`, then its second on the result.
        (RunState::Failed, true, true) => RunState::Success,
        (RunState::Failed, true, false) => RunState::Warning,
        // Java's second `if` alone, over a `WARNING` that arrived some other
        // way. `allow_failure` is not read: the first `if` is predicated on
        // `state.isFailed()`, which `WARNING` is not.
        (RunState::Warning, _, true) => RunState::Success,
        _ => state,
    }
}

/// **Spelling A** — `io.kestra.core.models.flows.State$Type.fail(Task)`, the
/// three-way CONSTRUCTION out of two booleans.
///
/// It takes no state because the artifact's does not: see this module's doc for
/// the bytecode. Its one caller here is
/// [`flowable::pause::deadline`](super::flowable::pause), which is
/// `ExecutorService.java:937`'s `State.Type.fail(pauseTask)` — the terminal a
/// `Pause`'s **`timeout:`** ends at. A `pauseDuration:` bound does not go
/// through it and must not: the same ternary gives that branch
/// `behavior.mapToState()` instead.
///
/// `soften(Failed, af, aw)` answers the same thing for every input, and that is
/// a fact about `FAILED` rather than about the two functions. They are kept
/// apart because their *domains* differ — this one has none — and because the
/// call site reads as what the artifact does there.
pub(crate) fn fail(allow_failure: bool, allow_warning: bool) -> RunState {
    match (allow_failure, allow_warning) {
        (false, _) => RunState::Failed,
        (true, false) => RunState::Warning,
        (true, true) => RunState::Success,
    }
}

/// [`soften`] for a **runnable**'s own terminal, with the artifact's retry gate
/// in front of it.
///
/// `WorkerTaskProcessor` reads
/// `!taskRunWithOutput.taskRun().shouldBeRetried(workerTask.getTask()
/// .getRetry())` before it softens anything, and that clause is load-bearing
/// rather than defensive — see this module's doc for what dropping it does to
/// `valids/task-allow-failure-runnable`'s three attempts.
///
/// # It asks the question with egret's OWN retry precedence, and that is a
/// stated divergence
///
/// Java's `shouldBeRetried` takes `task.getRetry()` — the failing task's own
/// `retry:` and nothing else. This asks
/// [`retry::policy_for`](super::retry::policy_for), which is the full
/// task → ancestor → flow precedence that [`retry::scan`](super::retry::scan)
/// itself uses. **The two must agree or a softened run becomes unretryable**:
/// `scan` reopens a `Failed` run, so softening a run `scan` would have retried
/// deletes the retry, and the narrower predicate here would do exactly that for
/// a task whose `retry:` sits on an ancestor. Nothing in this corpus can tell
/// them apart — the four flows that write both an `allowFailure:` and a
/// `retry:` put the `retry:` on the task itself — so this is the answer that
/// cannot be wrong rather than the one that is measured.
///
/// **It is evaluated BEFORE the terminal is recorded**, which is also Java's
/// order (`taskRunWithOutput.taskRun()` is the run as it stands, not
/// `withState(state)`). [`retry::next_retry_at`](super::retry::next_retry_at)
/// counts `attempts`, which a [`Command::SetRunState`](super::Command) does not
/// change, so the two orders would agree here — but only by accident of what
/// that function happens to read today.
pub(crate) fn for_runnable(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    outcome: RunState,
    now: Tick,
) -> RunState {
    let will_retry = retry::policy_for(plan, state, run)
        .and_then(|p| retry::next_retry_at(p, state, run, now))
        .is_some();
    if will_retry {
        return outcome;
    }
    let task = &plan.tasks[state.runs[run].task];
    soften(outcome, task.allow_failure, task.allow_warning)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Spelling A's whole table, as far as this enum reaches it.
    ///
    /// **The rows are named by the artifact's two booleans, and every one of
    /// the four combinations is here** — a fixture that only carried the two
    /// combinations the corpus writes would say nothing about the two it does
    /// not, and the flags are independent in the language whatever the corpus
    /// happens to use.
    #[test]
    fn the_four_flag_combinations_answer_what_the_artifact_answers() {
        // Neither flag: the artifact returns FAILED and so does this.
        assert_eq!(soften(RunState::Failed, false, false), RunState::Failed);
        // `allowWarning:` ALONE. `State$Type.fail` never reads it when
        // `isAllowFailure()` is false (bytecode 4: `ifeq 26`), so a FAILED task
        // stays FAILED -- this is the row a reader who thinks "allowWarning
        // downgrades warnings" gets wrong.
        assert_eq!(soften(RunState::Failed, false, true), RunState::Failed);
        // Both: SUCCESS, in one step.
        assert_eq!(soften(RunState::Failed, true, true), RunState::Success);
        // `allowFailure:` alone is WARNING. This row read `Failed` at Step 2,
        // where the enum had no variant for it.
        assert_eq!(soften(RunState::Failed, true, false), RunState::Warning);
    }

    /// **The row that separates the artifact's two spellings**, and the only
    /// one no corpus flow can supply.
    ///
    /// `State$Type.fail` cannot be handed a `WARNING` at all; spelling B can,
    /// and maps it under `allowWarning:` **whatever `allowFailure:` says** —
    /// its first `if` is predicated on `isFailed()`, which `WARNING` is not.
    /// Both values of `allow_failure` are supplied because a fixture that only
    /// passed `true` would agree with a wrong implementation that read the two
    /// flags as one.
    #[test]
    fn allow_warning_alone_moves_an_existing_warning() {
        assert_eq!(soften(RunState::Warning, false, true), RunState::Success);
        assert_eq!(soften(RunState::Warning, true, true), RunState::Success);
        // Without `allowWarning:` a WARNING stays, however `allowFailure:`
        // reads -- the arm a mutation widening the pattern to `(Warning, _, _)`
        // would break.
        assert_eq!(soften(RunState::Warning, false, false), RunState::Warning);
        assert_eq!(soften(RunState::Warning, true, false), RunState::Warning);
    }

    /// Spelling A's whole table — all four combinations, none of them omitted
    /// because the corpus writes only two.
    ///
    /// **It is asserted beside `soften` over `FAILED`, not instead of it**: the
    /// two agree on every input spelling A has, and the pair is what makes that
    /// agreement a measured fact rather than an assumption the call sites rest
    /// on.
    #[test]
    fn fail_is_the_construction_and_agrees_with_soften_over_failed() {
        assert_eq!(fail(false, false), RunState::Failed);
        assert_eq!(fail(false, true), RunState::Failed);
        assert_eq!(fail(true, false), RunState::Warning);
        assert_eq!(fail(true, true), RunState::Success);
        for (af, aw) in [(false, false), (false, true), (true, false), (true, true)] {
            assert_eq!(
                fail(af, aw),
                soften(RunState::Failed, af, aw),
                "the two spellings must agree over FAILED: af={af} aw={aw}"
            );
        }
    }

    /// **The flags move a `FAILED` and nothing else.** Kestra's two `if`s are
    /// both predicated on the incoming state, so every other terminal passes
    /// through untouched however the flags read -- and this is the arm a
    /// mutation that deleted the `RunState::Failed` pattern would break.
    #[test]
    fn no_other_state_is_touched_by_either_flag() {
        // **Derived from `RunState::ALL` rather than listed**, so a variant
        // added to the enum joins this population without anybody remembering
        // to add it -- which is how the Step 2 version of this test would have
        // stayed green through `Warning` arriving. The two states the flags DO
        // move are excluded by name.
        for state in RunState::ALL
            .iter()
            .copied()
            .filter(|s| !matches!(s, RunState::Failed | RunState::Warning))
        {
            for (af, aw) in [(false, false), (false, true), (true, false), (true, true)] {
                assert_eq!(
                    soften(state, af, aw),
                    state,
                    "{state:?} moved under allowFailure={af} allowWarning={aw}"
                );
            }
        }
    }

    /// The population above is not empty and not the whole enum — **a zero is
    /// unfalsifiable alone**, and a filter that excluded everything would make
    /// the loop above pass by running nothing.
    #[test]
    fn the_untouched_population_is_twelve_of_the_fourteen() {
        let untouched = RunState::ALL
            .iter()
            .filter(|s| !matches!(s, RunState::Failed | RunState::Warning))
            .count();
        // 10 of 12 until W8 Task 6 added `Cancelled`, which `soften` leaves
        // alone: Kestra's two `if`s read `state == FAILED` and `state ==
        // WARNING`, so `allowFailure:` on a task whose subflow child was
        // CANCELLED by an `sla:` does NOT turn that into a success.
        // 12 of 14 since `Killing` (the kill package's Task 1), which is not
        // terminal and which `soften` leaves alone for the same reason.
        assert_eq!((untouched, RunState::ALL.len()), (12, 14));
    }
}
