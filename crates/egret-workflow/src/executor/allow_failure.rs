//! `allowFailure:` / `allowWarning:` — the terminal a task's own two flags
//! decide, and the four sites the artifact reads them at.
//!
//! **This module is the `allowFailure:` FLAG, not the `AllowFailure`
//! flowable.** They are different constructs with the same word in them:
//! `io.kestra.plugin.core.flow.AllowFailure` is a class that `extends
//! Sequential` and is still planned opaque here
//! ([`plan::TaskKind::Runnable`](super::plan::TaskKind)), while
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

/// Spelling B, restricted to the half this enum can currently express — W8
/// Task 5 Step 2.
///
/// # The hole is named rather than approximated
///
/// `(Failed, allow_failure = true, allow_warning = false)` is `WARNING` on the
/// artifact and there is **no [`RunState`] variant for it**, so that input is
/// left alone here and returns `Failed`. That is a DIVERGENCE, not a rule: it
/// is wrong for the four entries Step 3 takes, and it is right for nothing —
/// the three entries this step banks never reach it, because all three write
/// both flags.
///
/// It is written as an explicit arm rather than as a catch-all so that the next
/// step widens a `match` the compiler has already laid out, and so that a
/// reader cannot mistake the gap for a decision.
pub(crate) fn soften(state: RunState, allow_failure: bool, allow_warning: bool) -> RunState {
    match (state, allow_failure, allow_warning) {
        // Both flags: `FAILED -> WARNING -> SUCCESS` on the artifact, and the
        // intermediate is a local that never reaches a history.
        (RunState::Failed, true, true) => RunState::Success,
        // `(Failed, true, false)` is the hole above.
        // `(Warning, _, true)` is spelling B's second `if` and is
        // unrepresentable for the same reason.
        _ => state,
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
        // `allowFailure:` alone is WARNING on the artifact and is the hole this
        // step names. Asserted as `Failed` on purpose: an assertion that said
        // `Success` here would ship the over-acceptance, and one that said
        // nothing would let Step 3 change it without anything going red.
        assert_eq!(soften(RunState::Failed, true, false), RunState::Failed);
    }

    /// **The flags move a `FAILED` and nothing else.** Kestra's two `if`s are
    /// both predicated on the incoming state, so every other terminal passes
    /// through untouched however the flags read -- and this is the arm a
    /// mutation that deleted the `RunState::Failed` pattern would break.
    #[test]
    fn no_other_state_is_touched_by_either_flag() {
        for state in [
            RunState::Created,
            RunState::Submitted,
            RunState::Running,
            RunState::Success,
            RunState::Retrying,
            RunState::Retried,
            RunState::Killed,
            RunState::Skipped,
            RunState::Paused,
            RunState::Restarted,
        ] {
            for (af, aw) in [(false, false), (false, true), (true, false), (true, true)] {
                assert_eq!(
                    soften(state, af, aw),
                    state,
                    "{state:?} moved under allowFailure={af} allowWarning={aw}"
                );
            }
        }
    }
}
