// kestra: core/src/main/java/io/kestra/core/models/flows/State.java

use serde::{Deserialize, Serialize};

/// A workflow execution's or task run's state.
///
/// All 17 variants are kept, including those egret's v1 never produces
/// (`Submitted`, `Paused`, `Restarted`, `Queued`, `Breakpoint`, `Resubmitted`),
/// because a missing variant is where semantics drift quietly — and because the
/// predicates below already depend on them. Spec §7 rule 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum State {
    Created,
    Submitted,
    Running,
    Paused,
    Restarted,
    Killing,
    Success,
    Warning,
    Failed,
    Killed,
    Cancelled,
    Queued,
    Retrying,
    Retried,
    Skipped,
    Breakpoint,
    Resubmitted,
}

impl State {
    /// `State.Type.isTerminated()`
    pub const fn is_terminated(self) -> bool {
        matches!(
            self,
            Self::Failed
                | Self::Warning
                | Self::Success
                | Self::Killed
                | Self::Cancelled
                | Self::Retried
                | Self::Skipped
                | Self::Resubmitted
        )
    }

    /// `State.Type.isTerminatedNoFail()` — terminal *and* not an error. Not the
    /// negation of `is_terminated_in_error`: a non-terminal state is neither.
    pub const fn is_terminated_no_fail(self) -> bool {
        matches!(
            self,
            Self::Warning | Self::Success | Self::Retried | Self::Skipped | Self::Resubmitted
        )
    }

    /// `State.Type.isTerminatedInError()`
    pub const fn is_terminated_in_error(self) -> bool {
        matches!(self, Self::Failed | Self::Killed | Self::Cancelled)
    }

    /// `State.Type.isCreated()` — includes `Restarted`.
    pub const fn is_created(self) -> bool {
        matches!(self, Self::Created | Self::Restarted)
    }

    /// `State.Type.isRunning()` — includes `Killing`, which is still executing.
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::Killing)
    }

    /// `State.Type.onlyRunning()` — `Running` and nothing else.
    pub const fn only_running(self) -> bool {
        matches!(self, Self::Running)
    }

    /// `State.Type.isFailed()`
    pub const fn is_failed(self) -> bool {
        matches!(self, Self::Failed)
    }

    /// `State.Type.isPaused()`
    pub const fn is_paused(self) -> bool {
        matches!(self, Self::Paused)
    }

    /// `State.Type.isBreakpoint()`
    pub const fn is_breakpoint(self) -> bool {
        matches!(self, Self::Breakpoint)
    }

    /// `State.Type.isRetrying()` — includes the terminal `Retried`.
    pub const fn is_retrying(self) -> bool {
        matches!(self, Self::Retrying | Self::Retried)
    }

    /// `State.Type.isSuccess()`
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    /// `State.Type.isKilled()`
    pub const fn is_killed(self) -> bool {
        matches!(self, Self::Killed)
    }

    /// `State.Type.isQueued()`
    pub const fn is_queued(self) -> bool {
        matches!(self, Self::Queued)
    }
}

#[cfg(test)]
mod tests {
    use super::State;
    use State::*;

    /// All 17, in Kestra's declaration order.
    const ALL: [State; 17] = [
        Created,
        Submitted,
        Running,
        Paused,
        Restarted,
        Killing,
        Success,
        Warning,
        Failed,
        Killed,
        Cancelled,
        Queued,
        Retrying,
        Retried,
        Skipped,
        Breakpoint,
        Resubmitted,
    ];

    /// The count is asserted on its own because the ONE variant most likely to
    /// be missed is the last: it ends with `;` in the Java enum, not `,`, so a
    /// line-oriented reading of State.java yields 16 and every predicate below
    /// still passes.
    #[test]
    fn has_all_seventeen_variants() {
        assert_eq!(ALL.len(), 17);
        assert!(ALL.contains(&Resubmitted));
    }

    #[test]
    fn predicates_match_kestra_exactly() {
        // State.Type.isTerminated()
        for s in [
            Failed,
            Warning,
            Success,
            Killed,
            Cancelled,
            Retried,
            Skipped,
            Resubmitted,
        ] {
            assert!(s.is_terminated(), "{s:?} must be terminated");
        }
        for s in [
            Created, Submitted, Running, Paused, Restarted, Killing, Queued, Retrying, Breakpoint,
        ] {
            assert!(!s.is_terminated(), "{s:?} must not be terminated");
        }

        // isTerminatedNoFail() -- note it EXCLUDES Failed/Killed/Cancelled.
        for s in [Warning, Success, Retried, Skipped, Resubmitted] {
            assert!(s.is_terminated_no_fail(), "{s:?}");
        }
        for s in [Failed, Killed, Cancelled] {
            assert!(!s.is_terminated_no_fail(), "{s:?}");
        }

        // isTerminatedInError()
        for s in [Failed, Killed, Cancelled] {
            assert!(s.is_terminated_in_error(), "{s:?}");
        }
        assert!(!Warning.is_terminated_in_error());

        // isCreated() covers Restarted too -- not obvious from the name.
        assert!(Created.is_created() && Restarted.is_created());
        assert!(!Submitted.is_created());

        // isRunning() covers Killing; onlyRunning() does not.
        assert!(Running.is_running() && Killing.is_running());
        assert!(Running.only_running() && !Killing.only_running());

        // isRetrying() covers the terminal Retried as well as Retrying.
        assert!(Retrying.is_retrying() && Retried.is_retrying());

        // The single-variant predicates.
        assert!(Failed.is_failed() && Paused.is_paused() && Breakpoint.is_breakpoint());
        assert!(Success.is_success() && Killed.is_killed() && Queued.is_queued());
    }

    #[test]
    fn serialises_as_kestra_spells_it() {
        assert_eq!(serde_json::to_string(&Success).unwrap(), "\"SUCCESS\"");
        assert_eq!(
            serde_json::to_string(&Resubmitted).unwrap(),
            "\"RESUBMITTED\""
        );
        let round: State = serde_json::from_str("\"KILLING\"").unwrap();
        assert_eq!(round, Killing);
    }
}
