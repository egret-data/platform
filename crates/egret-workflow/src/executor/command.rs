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
}
