//! `WorkingDirectory`: children that share one working directory (spec §4.2,
//! §5.1, §5.3).
//!
//! The block runs its children as a `Sequential` does, so a child that fails
//! without `allowFailure:` stops the rest (V1), and a skipped child gets its
//! `SKIPPED` run (spec §2.1). It adds two claims the worker does for the block,
//! and neither is a run:
//!
//! - the **prepare** claim, before the first child, which stages
//!   `namespaceFiles` and `inputFiles` and restores the cache;
//! - the **finish** claim, after the children are done whether one failed or
//!   not (V2), which collects `outputFiles` and saves the cache.
//!
//! The block's history is Kestra's, so a claim leaves no trace in it. This
//! driver asks for each claim with a command and waits while it is out; the
//! worker answers with [`Command::CompleteWorkingDirectoryClaim`]. A claim that
//! fails is the worker's [`Command::SetRunState`] on the block, which ends the
//! block before any child (V3).

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx};
use crate::executor::state::{ExecutionState, RunIx, Tick, WorkingDirPhase};

use super::{aggregate, sequential, Step};

/// Drive one `WorkingDirectory` block whose run is `RUNNING`.
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    tasks: &[TaskIx],
    now: Tick,
) -> Step {
    match state.runs[run].working_dir {
        None => Step::Commands(vec![Command::BeginWorkingDirectory { run }]),
        Some(WorkingDirPhase::Preparing | WorkingDirPhase::Finishing) => Step::Waiting,
        Some(WorkingDirPhase::Prepared) => {
            match sequential::drive(plan, state, tasks, &[], Some(run), now) {
                // The children are over, whatever state they reached. The
                // finish claim runs before the block takes its terminal state.
                Step::Done(_) => Step::Commands(vec![Command::FinishWorkingDirectory { run }]),
                other => other,
            }
        }
        // The block's terminal is the aggregate of its children, which the
        // finish claim did not change. `descend` applies `allowFailure:` to it.
        Some(WorkingDirPhase::Finished) => Step::Done(aggregate(state, tasks, Some(run))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::state::RunState;
    use crate::executor::{advance, allow_failure, apply, build_plan, TaskKind, TaskRun};

    const T0: Tick = Tick(0);

    /// Two children, `a` then `b`, in one block.
    const TWO: &str = "id: wd\nnamespace: io.kestra.tests\ntasks:\n  - id: block\n    type: io.kestra.plugin.core.flow.WorkingDirectory\n    tasks:\n      - id: a\n        type: io.kestra.plugin.core.debug.Return\n        format: a\n      - id: b\n        type: io.kestra.plugin.core.debug.Return\n        format: b\n";

    /// Where the harness makes the worker fail a claim instead of completing it.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FailClaim {
        Never,
        Prepare,
        Finish,
    }

    /// What one run of the block did, in order: each claim begun, and each
    /// child run created, as the worker and the executor would see them.
    struct Trace {
        plan: Plan,
        state: ExecutionState,
        events: Vec<String>,
    }

    /// Drive one block to a fixed point, playing the worker: a claim is
    /// completed the round after it is begun (or failed, per `fail`), and a
    /// child runs to its `outcomes` entry (default `SUCCESS`), with
    /// `allowFailure:` applied the way the worker applies it to a leaf.
    /// `runIf:` is rendered from its own literal, since this harness has no
    /// render.
    fn drive_block(src: &str, outcomes: &[(&str, RunState)], fail: FailClaim) -> Trace {
        let plan = build_plan(src).expect("the block plans");
        let mut state = ExecutionState::new(&plan);
        let mut events = Vec::new();
        for _ in 0..200 {
            let mut moved = false;
            let known = state.runs.len();

            for cmd in advance(&plan, &state, T0) {
                match cmd {
                    Command::BeginWorkingDirectory { .. } => events.push("prepare".into()),
                    Command::FinishWorkingDirectory { .. } => events.push("finish".into()),
                    _ => {}
                }
                apply(&mut state, &cmd);
                moved = true;
            }

            for ix in 0..state.runs.len() {
                let Some(phase) = state.runs[ix].working_dir else {
                    continue;
                };
                // A block that has ended has no claim left to answer.
                if state.runs[ix]
                    .history
                    .last()
                    .is_some_and(|s| s.is_terminal())
                {
                    continue;
                }
                let (waiting, failing) = match phase {
                    WorkingDirPhase::Preparing => (true, fail == FailClaim::Prepare),
                    WorkingDirPhase::Finishing => (true, fail == FailClaim::Finish),
                    _ => (false, false),
                };
                if waiting {
                    let cmd = if failing {
                        Command::SetRunState {
                            run: ix,
                            state: RunState::Failed,
                        }
                    } else {
                        Command::CompleteWorkingDirectoryClaim { run: ix }
                    };
                    apply(&mut state, &cmd);
                    moved = true;
                }
            }

            for ix in 0..state.runs.len() {
                let task = state.runs[ix].task;
                let def = &plan.tasks[task];
                if let (None, Some(literal)) = (&state.runs[ix].rendered_run_if, &def.run_if) {
                    state.runs[ix].rendered_run_if = Some(literal.clone());
                    moved = true;
                }
                if !matches!(def.kind, TaskKind::Runnable { .. })
                    || state.runs[ix].history.last() != Some(&RunState::Running)
                {
                    continue;
                }
                let want = outcomes
                    .iter()
                    .find(|(id, _)| *id == def.id)
                    .map_or(RunState::Success, |(_, s)| *s);
                let landed = allow_failure::soften(want, def.allow_failure, def.allow_warning);
                apply(
                    &mut state,
                    &Command::SetRunState {
                        run: ix,
                        state: landed,
                    },
                );
                moved = true;
            }

            for ix in known..state.runs.len() {
                if state.runs[ix].parent.is_some() {
                    events.push(format!("run {}", plan.tasks[state.runs[ix].task].id));
                }
            }
            if !moved {
                return Trace {
                    plan,
                    state,
                    events,
                };
            }
        }
        panic!("the block did not settle in 200 rounds");
    }

    /// The block is the first run the executor creates.
    fn block_run(state: &ExecutionState) -> &TaskRun {
        &state.runs[0]
    }

    fn history_of(plan: &Plan, state: &ExecutionState, id: &str) -> Vec<RunState> {
        state
            .runs
            .iter()
            .find(|r| plan.tasks[r.task].id == id)
            .unwrap_or_else(|| panic!("no run for {id}"))
            .history
            .clone()
    }

    /// **A child of a `WorkingDirectory` does not pass through `SUBMITTED`; a
    /// `Runnable` outside one does.**
    ///
    /// Measured, not assumed. In `golden.json` every child of a block reads
    /// `CREATED RUNNING <terminal>` (`valids/working-directory-real-child-fails`,
    /// `-shared-file`, `-cache`, `-input-output-files` and
    /// `valids/task-runif-workingdirectory`), and the raw captured trace of
    /// `working-directory-real-child-fails` has no `SUBMITTED` on its `fails`
    /// run, so the normaliser is not removing it. A top-level `Return` still
    /// reads `CREATED SUBMITTED RUNNING SUCCESS`. The fixture is driven through
    /// [`drive_block`], which plays the worker's claims: a child of a block is
    /// only created once the prepare claim is done.
    #[test]
    fn a_working_directory_child_is_not_submitted_and_a_plain_runnable_is() {
        let src = format!(
            "id: wd\nnamespace: io.kestra.tests\ntasks:\n  - id: outside\n    type: io.kestra.plugin.core.debug.Return\n    format: o\n{}",
            TWO.split_once("tasks:\n").expect("the fixture has a task list").1
        );
        let t = drive_block(&src, &[], FailClaim::Never);
        assert_eq!(
            history_of(&t.plan, &t.state, "outside"),
            [
                RunState::Created,
                RunState::Submitted,
                RunState::Running,
                RunState::Success
            ],
            "a top-level runnable is submitted to the worker"
        );
        for child in ["a", "b"] {
            assert_eq!(
                history_of(&t.plan, &t.state, child),
                [RunState::Created, RunState::Running, RunState::Success],
                "child {child} of a WorkingDirectory must not pass through SUBMITTED"
            );
        }
    }

    /// The §5.3 row V1 and V2 measured on the image: child `a` fails without
    /// `allowFailure:`, so `b` gets no run, and the finish claim still runs.
    #[test]
    fn a_failed_child_stops_the_block_and_the_finish_claim_still_runs() {
        let t = drive_block(TWO, &[("a", RunState::Failed)], FailClaim::Never);
        assert_eq!(
            t.events,
            ["prepare", "run a", "finish"],
            "b must get no run, and the finish claim must run after the failed child"
        );
        assert_eq!(block_run(&t.state).history.last(), Some(&RunState::Failed));
        assert_eq!(
            block_run(&t.state).working_dir,
            Some(WorkingDirPhase::Finished)
        );
    }

    /// The §5.3 row for `allowFailure: true`: the failed child is `WARNING`,
    /// the next child runs, and the block is `WARNING`.
    #[test]
    fn an_allow_failure_child_that_fails_does_not_stop_the_block() {
        let src = TWO.replace("        type: io.kestra.plugin.core.debug.Return\n        format: a\n", "        type: io.kestra.plugin.core.debug.Return\n        format: a\n        allowFailure: true\n");
        let t = drive_block(&src, &[("a", RunState::Failed)], FailClaim::Never);
        assert_eq!(t.events, ["prepare", "run a", "run b", "finish"]);
        assert_eq!(
            history_of(&t.plan, &t.state, "a").last(),
            Some(&RunState::Warning)
        );
        assert_eq!(
            history_of(&t.plan, &t.state, "b").last(),
            Some(&RunState::Success)
        );
        assert_eq!(block_run(&t.state).history.last(), Some(&RunState::Warning));
    }

    /// §5.3: a failing prepare claim fails the block, and no child has a run.
    #[test]
    fn a_failing_prepare_claim_fails_the_block_before_any_child() {
        let t = drive_block(TWO, &[], FailClaim::Prepare);
        assert_eq!(t.events, ["prepare"]);
        assert_eq!(block_run(&t.state).history.last(), Some(&RunState::Failed));
        assert_eq!(t.state.runs.len(), 1, "no child run exists");
    }

    /// §5.3: a failing finish claim fails the block after its children ran.
    #[test]
    fn a_failing_finish_claim_fails_the_block() {
        let t = drive_block(TWO, &[], FailClaim::Finish);
        assert_eq!(t.events, ["prepare", "run a", "run b", "finish"]);
        assert_eq!(block_run(&t.state).history.last(), Some(&RunState::Failed));
        assert_eq!(
            history_of(&t.plan, &t.state, "b").last(),
            Some(&RunState::Success)
        );
    }

    /// A child whose `runIf:` is falsy gets a `SKIPPED` run, and the sibling
    /// after it still runs (spec §2.1, measured as `wd-runif`).
    #[test]
    fn a_child_whose_run_if_is_false_is_skipped_and_the_next_child_runs() {
        let src = TWO.replace(
            "        format: a\n",
            "        format: a\n        runIf: \"false\"\n",
        );
        let t = drive_block(&src, &[], FailClaim::Never);
        assert_eq!(
            history_of(&t.plan, &t.state, "a"),
            [RunState::Created, RunState::Skipped]
        );
        assert_eq!(
            history_of(&t.plan, &t.state, "b").last(),
            Some(&RunState::Success)
        );
        assert_eq!(block_run(&t.state).history.last(), Some(&RunState::Success));
    }
}
