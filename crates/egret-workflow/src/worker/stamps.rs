//! Real start and end times (W7b Task 1, migration 0180) — **the one place
//! the timestamp columns of `workflow.executions` and `workflow.task_runs`
//! are spelled**, and the attempt end beside them.
//!
//! # The rule every state write follows
//!
//! A statement that writes `state` on either table writes `started_at` and
//! `ended_at` in the SAME statement, from the same values, through one of the
//! two macros here:
//!
//! - [`stamp_set!`] in an `UPDATE … SET`;
//! - [`stamp_values!`] in an `INSERT … VALUES`, beside [`stamp_columns!`] in
//!   the column list.
//!
//! There is no read-then-write: the decision is a `CASE` over the bound new
//! state and history and the row's OLD `state` and `history` columns, which an
//! `UPDATE`'s right-hand side reads before the assignment.
//!
//! `tests/execution_timestamps.rs::every_state_write_stamps_the_timestamps`
//! scans the source for every `UPDATE`/`INSERT` of either table that writes
//! `state` and fails naming the one that does not use these macros, and
//! migration 0180's two CHECKs make a writer that bypasses them fail at the
//! first wrong write rather than drift quietly.
//!
//! # What the `CASE` says
//!
//! - **`started_at`** is set when the row's history FIRST holds `RUNNING`
//!   (new history has it, old history does not), and kept while it holds it —
//!   so a resumed pause and a `RETRY_FAILED_TASK` reopen do not move it. It is
//!   cleared when the history no longer holds `RUNNING`, which only
//!   `Command::RestartLoop` does, to the descendants it empties.
//! - **`ended_at`** is set when the row ENTERS a terminal state (new state
//!   terminal, old state not), kept while it stays terminal — so an outputs
//!   write or a terminal-to-terminal move does not move it — and cleared when
//!   the row leaves a terminal state (`retry::reopen_due` takes a `FAILED` run
//!   back to `CREATED`).
//!
//! **Entering, not "is", and that is what keeps a pre-0180 row honest.** A row
//! that was already `RUNNING` or already terminal when the migration ran has
//! `NULL`s, and a later write does not stamp it with an instant that is not
//! its start or its end: nothing is guessed (spec §3.1).
//!
//! **`GREATEST(now(), started_at)`, not `now()`.** `now()` is the
//! TRANSACTION's start. A transaction that began, then waited on the
//! execution's `FOR NO KEY UPDATE` while another one started the row, would
//! otherwise write an end before the start the other one committed; migration
//! 0180's `…_ends_after_it_starts` CHECK would then refuse the write.
//!
//! The terminal list is `workflow.state_is_terminal`, migration 0180's one SQL
//! copy of [`RunState::is_terminal`]; a test asserts the two agree for every
//! variant of [`RunState::ALL`].

use crate::executor::{ExecutionState, RunState, Tick};

/// The two timestamp assignments of an `UPDATE`, for a statement whose new
/// state is bound at `$state` and whose new history (a jsonb array of state
/// names) is bound at `$history`. Both are placeholder literals, e.g.
/// `stamp_set!("$1", "$2")`.
macro_rules! stamp_set {
    ($state:literal, $history:literal) => {
        concat!(
            "started_at = CASE WHEN (",
            $history,
            ")::jsonb ? 'RUNNING' THEN \
               CASE WHEN history ? 'RUNNING' THEN started_at ELSE now() END END, \
             ended_at = CASE WHEN workflow.state_is_terminal(",
            $state,
            ") THEN \
               CASE WHEN workflow.state_is_terminal(state) THEN ended_at \
                    ELSE GREATEST(now(), started_at) END END"
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
/// new row has no old state, so it is "entering" whatever it is inserted in.
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
/// pre-0180 attempt, terminal with no `ended_at`, from being given an end it
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
        // Arrange: four runs. 0 ends now; 1 stays open; 2 is a pre-0180
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
        let set = stamp_set!("$1", "$2");
        assert!(set.contains("($2)::jsonb ? 'RUNNING'"), "{set}");
        assert!(set.contains("workflow.state_is_terminal($1)"), "{set}");
        assert!(set.contains("THEN ended_at"), "{set}");
        assert!(set.contains("GREATEST(now(), started_at)"), "{set}");
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
}
