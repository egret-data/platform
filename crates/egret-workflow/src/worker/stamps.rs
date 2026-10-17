//! Real start and end times (W7b Task 1, migration 0183) — **the one place
//! the timestamp columns of `workflow.executions` and `workflow.task_runs`
//! are spelled**, and the attempt end beside them.
//!
//! # The rule every state write follows
//!
//! A statement that writes `state` on either table writes `started_at` and
//! `ended_at` in the SAME statement, through one of the macros here:
//!
//! - [`stamp_set!`] in an `UPDATE … SET`;
//! - [`stamp_values!`] in an `INSERT … VALUES`, beside [`stamp_columns!`] in
//!   the column list.
//!
//! `tests/execution_timestamps.rs::every_state_write_stamps_the_timestamps`
//! scans the source for statements that write `state` on either table and
//! fails naming one that does not use these macros. That guard is lexical,
//! and its doc lists what it cannot see. Migration 0183's CHECKs catch only
//! part of a bypass: an end on a live row, or an end before the start. A
//! bypass that writes a terminal state and leaves `ended_at` NULL passes both.
//!
//! # What is written, and when
//!
//! The rule is judged **per transition, across the whole settle**, by
//! [`Entered::of`]. It compares the history the settle LOADED (under the
//! execution's `FOR NO KEY UPDATE`, in this same transaction) with the
//! history it writes, and the `UPDATE` receives two booleans:
//!
//! - **`entered_running`**: this write takes the row into `RUNNING` for the
//!   FIRST time. `started_at` becomes `now()`. Otherwise it is kept while the
//!   new history holds `RUNNING`, so a resumed pause and a `RETRY_FAILED_TASK`
//!   reopen do not move it, and cleared when the history does not hold it.
//! - **`entered_terminal`**: somewhere in this settle the row went from a
//!   non-terminal state to a terminal one, and it ends terminal. `ended_at`
//!   becomes `GREATEST(now(), started_at)`. Otherwise it is kept while the row
//!   stays terminal (an outputs write, a lease release, a terminal-to-terminal
//!   move), and cleared when the row is not terminal (a retry reopen).
//!
//! **Per transition, not "old row against new row".** The first version
//! compared the stored state with the new one. A row that left a terminal
//! state and re-entered one INSIDE one settle then kept its previous end:
//! a `LoopUntil` descendant that `RestartLoop` empties, `RecreateRun` builds
//! back and `runIf` skips again, all in one settle. Here, a history that the
//! write does not merely extend (an emptied and rebuilt one) is judged from
//! scratch.
//!
//! The history alone is not enough for a run. `RestartLoop` can empty a
//! `LoopUntil` descendant and the walk can rebuild it to the SAME history in
//! one settle (`[CREATED, SKIPPED]` both times). So `persist` also passes
//! `reborn`, read from `Marks::created`, which `Marks::observe` records PER
//! ROUND. `Marks::ended` (`$10`, the reviewer's suggestion) is also per
//! round and would give the same end. One rule, [`Entered::of`], serves both
//! columns and both tables instead.
//!
//! **This is not a read-then-write.** The booleans come from the state this
//! transaction loaded under the row lock, the same way `history` and
//! `attempts` themselves are written.
//!
//! **A pre-0183 row is never guessed.** A row that was already running or
//! terminal when the migration ran has an unchanged prefix, so neither
//! boolean fires and its NULLs stay NULL (spec §3.1).
//!
//! **`GREATEST(now(), started_at)`, not `now()`.** `now()` is the
//! TRANSACTION's start. A transaction that began, then waited on the lock
//! while another one started the row, would otherwise write an end before the
//! committed start.
//!
//! The SQL terminal list is `workflow.state_is_terminal`, which matches
//! `models::State::is_terminated` (a superset of [`RunState::is_terminal`]
//! by `RESUBMITTED`, which no `RunState` spells). A test pins it against
//! every variant of both.

use crate::executor::{ExecutionState, RunState, Tick};

/// The two timestamp assignments of an `UPDATE`. The new state is bound at
/// `$state` and the new history (a jsonb array of state names) at `$history`.
/// `$entered_running` and `$entered_terminal` are the two booleans of
/// [`Entered`]. All four are placeholder literals, for example
/// `stamp_set!("$1", "$2", "$4", "$5")`.
macro_rules! stamp_set {
    ($state:literal, $history:literal, $entered_running:literal, $entered_terminal:literal) => {
        concat!(
            "started_at = CASE WHEN (",
            $history,
            ")::jsonb ? 'RUNNING' THEN CASE WHEN ",
            $entered_running,
            "::boolean THEN now() ELSE started_at END END, \
             ended_at = CASE WHEN workflow.state_is_terminal(",
            $state,
            ") THEN CASE WHEN ",
            $entered_terminal,
            "::boolean THEN GREATEST(now(), started_at) ELSE ended_at END END"
        )
    };
}

/// The column names [`stamp_values!`] fills, in its order.
macro_rules! stamp_columns {
    () => {
        "started_at, ended_at"
    };
}

/// The two timestamp values of an `INSERT`, in [`stamp_columns!`]'s order. A
/// new row has no earlier life, so it enters whatever it is inserted in.
macro_rules! stamp_values {
    ($state:literal, $history:literal) => {
        concat!(
            "CASE WHEN (",
            $history,
            ")::jsonb ? 'RUNNING' THEN now() END, \
             CASE WHEN workflow.state_is_terminal(",
            $state,
            ") THEN now() END"
        )
    };
}

/// Which of the two timestamps a write must STAMP, judged per transition
/// across one settle. See this module's doc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entered {
    pub(crate) running: bool,
    pub(crate) terminal: bool,
}

impl Entered {
    /// `before` is the history the settle loaded. `after` is the history it
    /// writes. `reborn` says the settle emptied the row and built it back
    /// (`RestartLoop` then `RecreateRun`). For a run, that is
    /// `Marks::created` holding an EXISTING run. An execution is never
    /// reborn.
    ///
    /// `reborn` is needed because the history alone cannot show it. A
    /// `LoopUntil` descendant that `runIf` skips is `[CREATED, SKIPPED]`
    /// before the settle and `[CREATED, SKIPPED]` after it, yet it was
    /// emptied, rebuilt and skipped again in between. Measured by
    /// `tests/execution_timestamps.rs::a_loop_until_descendant_skipped_again_gets_a_new_end`.
    pub(crate) fn of(before: &[RunState], after: &[RunState], reborn: bool) -> Self {
        // A write that EXTENDS the loaded history only adds the suffix. One
        // that does not, or a reborn row, is a new life judged from scratch.
        let (prior, suffix) = if !reborn && after.starts_with(before) {
            (before, &after[before.len()..])
        } else {
            (&[][..], after)
        };
        let running = suffix.contains(&RunState::Running) && !prior.contains(&RunState::Running);
        let mut was_terminal = prior.last().is_some_and(|s| s.is_terminal());
        let mut entered_terminal = false;
        for s in suffix {
            let is = s.is_terminal();
            entered_terminal |= is && !was_terminal;
            was_terminal = is;
        }
        Self {
            running,
            terminal: entered_terminal && after.last().is_some_and(|s| s.is_terminal()),
        }
    }
}

pub(crate) use {stamp_columns, stamp_set, stamp_values};

/// **Stamp `ended_at` on every attempt this settle ended**, at `now` — the
/// same `Tick` clock as [`Attempt::started_at`](crate::executor::Attempt),
/// milliseconds after the execution's `created_at`.
///
/// Called by `claim::begin` immediately before `persist`, so the end is
/// written by the same `UPDATE` (or `INSERT`) that writes the attempt's
/// terminal state: `attempts` is one jsonb column and `persist` serialises
/// this state whole.
///
/// "Ended in this settle" is: terminal now, no end yet, and not the attempt
/// `before` held at the same position. The last term is what keeps a
/// pre-0183 attempt, terminal with no `ended_at`, from being given an end it
/// did not have — it is unchanged, so it is skipped. `advance()` never writes
/// this field; it is a fact about the environment, not about the executor's
/// decisions.
///
/// `max(now, started_at)`, for [`stamp_set!`]'s `GREATEST` reason: a settle's
/// `now` can be a few milliseconds behind the one that started the attempt.
pub(crate) fn stamp_attempt_ends(before: &ExecutionState, after: &mut ExecutionState, now: Tick) {
    for (ix, run) in after.runs.iter_mut().enumerate() {
        let old = before.runs.get(ix).map(|r| r.attempts.as_slice());
        for (ai, attempt) in run.attempts.iter_mut().enumerate() {
            let ended = attempt
                .history
                .last()
                .is_some_and(|s: &RunState| s.is_terminal());
            let unchanged = old.and_then(|o| o.get(ai)) == Some(&*attempt);
            if ended && attempt.ended_at.is_none() && !unchanged {
                attempt.ended_at = Some(Tick(now.0.max(attempt.started_at.0)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::{Attempt, TaskRun};

    fn run(attempts: Vec<Attempt>) -> TaskRun {
        TaskRun {
            task: 0,
            parent: None,
            value: None,
            rendered: None,
            rendered_run_if: None,
            history: vec![RunState::Created],
            attempts,
            not_before: None,
            resume_with: None,
            working_dir: None,
        }
    }

    fn attempt(history: &[RunState], started: i64, ended: Option<i64>) -> Attempt {
        Attempt {
            history: history.to_vec(),
            started_at: Tick(started),
            ended_at: ended.map(Tick),
        }
    }

    fn state(runs: Vec<TaskRun>) -> ExecutionState {
        ExecutionState {
            runs,
            history: vec![RunState::Running],
            metadata: Default::default(),
        }
    }

    use RunState::{Created as C, Failed as F, Running as R, Skipped as K, Success as S};

    /// Every branch of the rule, each beside its control.
    #[test]
    fn an_attempt_is_stamped_exactly_when_this_settle_ended_it() {
        // Arrange: four runs. 0 ends now; 1 stays open; 2 is a pre-0183
        // terminal attempt with no end, unchanged; 3 already has an end.
        let before = state(vec![
            run(vec![attempt(&[C, R], 10, None)]),
            run(vec![attempt(&[C, R], 10, None)]),
            run(vec![attempt(&[C, R, S], 10, None)]),
            run(vec![attempt(&[C, R, F], 10, Some(20))]),
        ]);
        let mut after = before.clone();
        after.runs[0].attempts[0].history.push(S);
        // A brand-new run whose only attempt is born terminal (`SkipRun`).
        after.runs.push(run(vec![attempt(&[C, K], 40, None)]));

        // Act
        stamp_attempt_ends(&before, &mut after, Tick(50));

        // Assert
        let ends: Vec<Option<Tick>> = after.runs.iter().map(|r| r.attempts[0].ended_at).collect();
        assert_eq!(
            ends,
            vec![Some(Tick(50)), None, None, Some(Tick(20)), Some(Tick(50))]
        );
    }

    /// The end never precedes the start, even when `now` does.
    #[test]
    fn an_end_is_never_before_its_start() {
        let before = state(vec![run(vec![attempt(&[C, R], 100, None)])]);
        let mut after = before.clone();
        after.runs[0].attempts[0].history.push(F);
        stamp_attempt_ends(&before, &mut after, Tick(90));
        assert_eq!(after.runs[0].attempts[0].ended_at, Some(Tick(100)));
    }

    /// The SQL the macros expand to, pinned: the guard in
    /// `tests/execution_timestamps.rs` recognises a writer by the macro name,
    /// so what the macro SAYS has to be checked somewhere.
    #[test]
    fn the_macros_spell_the_rule() {
        let set = stamp_set!("$1", "$2", "$4", "$5");
        assert!(set.contains("($2)::jsonb ? 'RUNNING'"), "{set}");
        assert!(
            set.contains("$4::boolean THEN now() ELSE started_at"),
            "{set}"
        );
        assert!(set.contains("workflow.state_is_terminal($1)"), "{set}");
        assert!(
            set.contains("$5::boolean THEN GREATEST(now(), started_at) ELSE ended_at"),
            "{set}"
        );
        let values = stamp_values!("$10", "$11");
        assert!(
            values.contains("($11)::jsonb ? 'RUNNING' THEN now()"),
            "{values}"
        );
        assert!(
            values.contains("workflow.state_is_terminal($10) THEN now()"),
            "{values}"
        );
        assert_eq!(stamp_columns!(), "started_at, ended_at");
    }

    fn entered(before: &[RunState], after: &[RunState]) -> (bool, bool) {
        let e = Entered::of(before, after, false);
        (e.running, e.terminal)
    }

    use RunState::{Paused as P, Retrying as Y, Warning as W};

    /// Every transition shape the rule distinguishes, by `(running, terminal)`.
    #[test]
    fn entered_is_judged_per_transition() {
        // A first run and a first end.
        assert_eq!(entered(&[C], &[C, R]), (true, false));
        assert_eq!(entered(&[C, R], &[C, R, S]), (false, true));
        assert_eq!(entered(&[C], &[C, R, S]), (true, true));
        // A rewrite with nothing new (lease release, pre-0183 row).
        assert_eq!(entered(&[C, R, S], &[C, R, S]), (false, false));
        assert_eq!(entered(&[C, R], &[C, R]), (false, false));
        // Terminal to terminal does not re-enter.
        assert_eq!(entered(&[C, R, S], &[C, R, S, W]), (false, false));
        // A pause resumed, and a retry reopened then finished: the start is
        // kept, and the end is new.
        assert_eq!(entered(&[C, R, P], &[C, R, P, R]), (false, false));
        assert_eq!(
            entered(&[C, R, F, Y], &[C, R, F, Y, C, R, S]),
            (false, true)
        );
        // Leave and re-enter INSIDE one settle: terminal, reopened and
        // terminal again. The old row-vs-row rule kept the stale end.
        assert_eq!(entered(&[C, R, F], &[C, R, F, Y, C, R, F]), (false, true));
        // `RestartLoop` emptied the history and the walk rebuilt it: a new
        // life, judged from scratch, whether skipped or run.
        assert_eq!(entered(&[C, K], &[C, K]), (false, false));
        assert_eq!(entered(&[C, R, S], &[C, K]), (false, true));
        assert_eq!(entered(&[C, R, S], &[C, R, S][..2]), (true, false));
        assert_eq!(entered(&[C, R, S], &[]), (false, false));
        // Born skipped (an INSERT never asks, but the rule agrees).
        assert_eq!(entered(&[], &[C, K]), (false, true));
        // Reborn with an IDENTICAL history: only `reborn` can tell.
        let reborn = |b: &[RunState], a: &[RunState]| {
            let e = Entered::of(b, a, true);
            (e.running, e.terminal)
        };
        assert_eq!(reborn(&[C, K], &[C, K]), (false, true));
        assert_eq!(reborn(&[C, R, S], &[C, R, S]), (true, true));
        assert_eq!(reborn(&[C, R, S], &[C, R]), (true, false));
    }
}
