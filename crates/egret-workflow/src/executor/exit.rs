//! `io.kestra.plugin.core.execution.Exit` — spec
//! `docs/en/specs/2026-10-17-workflow-kill-and-exit-design.md` §6.
//!
//! In Kestra it is an `ExecutionUpdatableTask`: the executor runs it, not a
//! worker (`core/…/plugin/core/execution/Exit.java`). Here too: it is a
//! `spec::registry` row with no worker handler, planned
//! [`TaskKind::Exit`](super::TaskKind::Exit), and `advance()` decides it the
//! round after it is started, as it decides an `If`.
//!
//! # What the golden says, and where it is not the spec's first reading
//!
//! The four `valids/exit*` entries are the acceptance. Every one reads the
//! same shape: the `Exit` run `CREATED RUNNING <state>`, each ancestor
//! `CREATED RUNNING <state>`, the execution `CREATED RUNNING <state>`, and no
//! run after the `If` is ever created. **`valids/exit-killed` is the one that
//! moves the design**: its execution reads `CREATED RUNNING KILLED` and its
//! `If` `CREATED RUNNING KILLED`, with no `KILLING` in either, and its `Exit`
//! run is `KILLED` too. So `state: KILLED` is not "put the execution in
//! `KILLING` and let `kill::advance_killing` close it" — that writes `KILLING`
//! into two histories the golden has without it. It is Kestra's own shape
//! (`ExecutorService.handleExecutionUpdatingTasks`, then
//! `ExecutionService.killParentTaskruns`): the `Exit` run and every ancestor
//! are `KILLED` at once, and the execution is `KILLED` at once **when nothing
//! else is live**. When something else is live — a sibling branch of a
//! `Parallel` — the execution becomes `KILLING` and [`super::kill`] closes it,
//! which is the half of Kestra's kill event that the executor can do.
//!
//! # A `KILLED` exit kills its live siblings, and that is the kill route's step
//!
//! Kestra emits a kill event for the execution (`isOnKillCascade(true)`), so
//! a sibling leaf that is running is killed by its worker. The kill route
//! writes the leaves (spec §3.2 steps 1-2). An `Exit` is a kill nobody routed,
//! so this module does the route's two leaf steps itself, for this execution:
//! a `Runnable` the pure state calls running (`RUNNING` or `SUBMITTED`)
//! becomes `KILLING`, and every other live leaf — not started, retrying, a
//! `Subflow` — becomes `KILLED`. Flowables are left to `advance_killing`,
//! exactly as the route leaves them. (`advance_killing` also kills any leaf
//! still live and not `KILLING`, as a backstop: `kill.rs`'s "A leaf nobody
//! killed is killed here".)
//!
//! **The kill is the whole round** (final review C1, 2026-10-18). The kill's
//! leaf steps are computed from the state at the start of the round, but a
//! `Parallel` collects every child's commands in one batch, so the same round
//! could create a sibling branch's next run or start a `CREATED` sibling
//! beside them. `advance()` therefore returns this module's commands alone
//! when an `Exit` decides `KILLED` ([`killed_in`]), as Kestra runs the
//! updating task before the flowables; the next round, under `KILLING`, is
//! `advance_killing`'s.
//!
//! **`advance()` cannot see a lease, so "running" here includes a run no
//! worker holds yet** (`RUNNING` with `claimed_at IS NULL`, waiting to be
//! claimed). Both cases end the same way, by two different doors: if a worker
//! holds the run, that worker sees `KILLING` and stops it (spec §4.2, plan
//! Task 4); if nobody holds it, the next claim takes the `KILLING` row,
//! cancels and writes `KILLED` (spec §4.3, plan Task 5). The execution
//! stays `KILLING` until that claim commits, because `advance_killing` waits
//! for a `KILLING` leaf. That is the spec's path for an unclaimed `KILLING` run, not
//! a separate one, so no DB behaviour is added here (kill-and-exit Task 3 fix
//! round 1, the coordinator's ruling).
//!
//! **A `KILLED` exit also kills the execution's child executions** (`Subflow`
//! and loop children), as Kestra's `isOnKillCascade(true)` does: `claim`'s
//! settle sees this round set an `Exit` run `KILLED` (`claim::exit_kills`)
//! and runs the kill route's cascade (`worker::kill::cascade`, plan Task 6).
//!
//! # Any other state leaves the siblings alone (D5)
//!
//! `SUCCESS`, `WARNING`, `FAILED` and `CANCELLED` set the `Exit` run, its
//! ancestors and the execution to that state and touch nothing else: Kestra's
//! `update` ends "the last not-terminated run, then its parents, then the
//! execution", and documents that *"any other execution with a different
//! state will continue to run"*. A sibling still running reports into a
//! terminal execution, and `advance()` answers a terminal execution with
//! nothing.
//!
//! # `state:` may be an expression
//!
//! A literal one of the five is read off the plan. Anything else — an
//! expression, or a literal outside the five — is offered to the caller's
//! render pass (`worker::render::expression_of`) at `RUNNING`, like an `If`'s
//! condition; the pass fails the run with [`parse_state`]'s message when the
//! rendered value is not one of the five, so the message names the value.

use serde::Deserialize;

use super::command::Command;
use super::flowable::Step;
use super::plan::{Plan, TaskIx, TaskKind};
use super::state::{ExecutionState, RunIx, RunState};

/// The canonical (and only) spelling. Kestra gives it no alias.
pub const TYPE_ID: &str = "io.kestra.plugin.core.execution.Exit";

/// The properties `build_plan` reads off an `Exit`'s leaf spec — the read set
/// `spec::schema::measure` derives its property treatment from.
#[derive(Debug, Deserialize)]
pub struct Spec {
    /// Kestra's `Property<ExitState>`, `@Builder.Default SUCCESS`.
    #[serde(default)]
    pub state: Option<serde_json::Value>,
}

/// The `state:` source an `Exit`'s leaf spec carries, unrendered. `None` is
/// "the author wrote none", which is Kestra's default `SUCCESS`. A non-string
/// value is kept as its JSON text, so the run fails naming it.
pub(crate) fn source_of(spec: &serde_json::Value) -> Option<String> {
    let parsed: Spec = serde_json::from_value(spec.clone()).unwrap_or(Spec { state: None });
    match parsed.state {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) => Some(s),
        Some(other) => Some(other.to_string()),
    }
}

/// Kestra's `ExitState`, by its exact name.
pub fn parse_state(s: &str) -> Result<RunState, String> {
    match s {
        "SUCCESS" => Ok(RunState::Success),
        "WARNING" => Ok(RunState::Warning),
        "KILLED" => Ok(RunState::Killed),
        "FAILED" => Ok(RunState::Failed),
        "CANCELLED" => Ok(RunState::Cancelled),
        other => Err(format!(
            "Exit state '{other}' is not one of SUCCESS, WARNING, KILLED, FAILED, CANCELLED"
        )),
    }
}

/// The source the caller has to render, or `None` when the plan already holds
/// the answer (no `state:`, or a literal one of the five).
pub fn expression(source: Option<&str>) -> Option<&str> {
    source.filter(|s| parse_state(s).is_err())
}

/// A `Running` `Exit` run: its terminal, its ancestors' and the execution's.
pub(crate) fn drive(plan: &Plan, state: &ExecutionState, run: RunIx, task: TaskIx) -> Step {
    let TaskKind::Exit { state: source } = &plan.tasks[task].kind else {
        // `flowable::descend` reaches here only through its `Exit` arm.
        return Step::Waiting;
    };
    let chosen = match source.as_deref() {
        None => Ok(RunState::Success),
        Some(literal) => match parse_state(literal) {
            Ok(s) => Ok(s),
            Err(_) => match state.runs[run].rendered.as_deref() {
                // The caller has not rendered it yet: wait, as an `If` does.
                None => return Step::Waiting,
                Some(rendered) => parse_state(rendered),
            },
        },
    };
    Step::Commands(match chosen {
        // The render pass fails such a run with `parse_state`'s message before
        // `advance` sees it; a rendered value that reaches here anyway fails
        // the run rather than being guessed at.
        Err(_) => vec![Command::SetRunState {
            run,
            state: RunState::Failed,
        }],
        Ok(RunState::Killed) => kill(plan, state, run),
        Ok(s) => end_with(state, run, s),
    })
}

/// The `Exit` run this round's `work` decides `KILLED`, if any: the first
/// `SetRunState { KILLED }` onto a still-`RUNNING` `Exit` run, which only
/// [`kill`] emits (its first command is the `Exit` run's own). `advance()`
/// then returns [`kill`]'s commands alone (final review C1).
pub(crate) fn killed_in(plan: &Plan, state: &ExecutionState, work: &[Command]) -> Option<RunIx> {
    work.iter().find_map(|c| match c {
        Command::SetRunState {
            run,
            state: RunState::Killed,
        } => state
            .runs
            .get(*run)
            .filter(|r| {
                matches!(plan.tasks[r.task].kind, TaskKind::Exit { .. })
                    && r.history.last() == Some(&RunState::Running)
            })
            .map(|_| *run),
        _ => None,
    })
}

/// `run` and each of its ancestors, innermost first — Kestra's
/// `parentTaskRunId` walk in `Exit.update` and `killParentTaskruns`. A
/// terminal ancestor is skipped (none can be: it has a running child).
fn line(state: &ExecutionState, run: RunIx) -> Vec<RunIx> {
    let mut out = vec![run];
    let mut cur = state.runs[run].parent;
    while let Some(p) = cur {
        // A run's parent is created before it, so the walk is bounded; the
        // `>=` break holds that for a state a caller stored and read back.
        if p >= *out.last().expect("non-empty") {
            break;
        }
        if !state.runs[p]
            .history
            .last()
            .is_some_and(|s| s.is_terminal())
        {
            out.push(p);
        }
        cur = state.runs[p].parent;
    }
    out
}

/// `SUCCESS`, `WARNING`, `FAILED`, `CANCELLED`: the run, its ancestors and the
/// execution take it; nothing else is touched (D5).
fn end_with(state: &ExecutionState, run: RunIx, s: RunState) -> Vec<Command> {
    let mut cmds: Vec<Command> = line(state, run)
        .into_iter()
        .map(|r| Command::SetRunState { run: r, state: s })
        .collect();
    cmds.push(Command::SetExecutionState { state: s });
    cmds
}

/// `KILLED`: see the module doc. The run and its ancestors are `KILLED`; every
/// other live leaf gets the kill route's leaf step; the execution is `KILLED`
/// when nothing else is live and `KILLING` otherwise.
///
/// A sibling `Runnable` this state calls running becomes `KILLING` whether or
/// not a worker holds it: a holder stops it (plan Task 4), and an unheld one
/// is finished by the next claim, which takes a `KILLING` run only to cancel
/// it and record `KILLED` (plan Task 5, `claim::finish_claimed_kill`). The
/// execution stays `KILLING` until then.
///
/// **These commands are the whole round** (final review C1): `advance()`
/// returns them alone ([`killed_in`]), so no sibling's create or start from
/// the same batch lands beside a kill computed without it.
pub(crate) fn kill(plan: &Plan, state: &ExecutionState, run: RunIx) -> Vec<Command> {
    let line = line(state, run);
    let mut cmds: Vec<Command> = line
        .iter()
        .map(|&r| Command::SetRunState {
            run: r,
            state: RunState::Killed,
        })
        .collect();
    // Whether anything is still live once this round's commands apply.
    let mut live = false;
    for (ix, r) in state.runs.iter().enumerate() {
        let Some(&last) = r.history.last() else {
            continue;
        };
        if line.contains(&ix) || last.is_terminal() {
            continue;
        }
        let leaf = matches!(
            plan.tasks[r.task].kind,
            TaskKind::Runnable { .. } | TaskKind::Executable { .. }
        );
        let held = matches!(plan.tasks[r.task].kind, TaskKind::Runnable { .. })
            && matches!(last, RunState::Running | RunState::Submitted);
        if !leaf || last == RunState::Killing {
            // Step 3: a flowable is `advance_killing`'s; a leaf already being
            // killed is its worker's.
            live = true;
        } else if held {
            // Step 1: a worker may hold it (it stops it, spec §4.2) or it may
            // be waiting for one (the next claim finishes it, spec §4.3).
            cmds.push(Command::SetRunState {
                run: ix,
                state: RunState::Killing,
            });
            live = true;
        } else {
            // Step 2: not started, retrying, or a `Subflow` run.
            cmds.push(Command::SetRunState {
                run: ix,
                state: RunState::Killed,
            });
        }
    }
    cmds.push(Command::SetExecutionState {
        state: if live {
            RunState::Killing
        } else {
            RunState::Killed
        },
    });
    cmds
}

#[cfg(test)]
mod tests {
    use super::super::state::{apply, ExecutionState};
    use super::super::{advance, build_plan, Tick};
    use super::*;

    use RunState::{
        Cancelled, Created, Failed, Killed, Killing, Retrying, Running, Submitted, Success, Warning,
    };

    const T: Tick = Tick(0);

    fn plan(src: &str) -> Plan {
        build_plan(src).expect("plans")
    }

    fn task(plan: &Plan, id: &str) -> TaskIx {
        plan.tasks
            .iter()
            .position(|t| t.id == id)
            .unwrap_or_else(|| panic!("no task {id}"))
    }

    /// The run of task `id`, by creation order.
    fn run_of(plan: &Plan, s: &ExecutionState, id: &str) -> RunIx {
        let t = task(plan, id);
        s.runs
            .iter()
            .position(|r| r.task == t)
            .unwrap_or_else(|| panic!("no run of {id}: {:?}", s.runs))
    }

    /// Every round until `advance` is quiet, with the caller's half filled in:
    /// an `If` renders `false` (the corpus shape takes `else`), and an `Exit`
    /// whose `state:` is an expression renders `exit_render`.
    fn settle(plan: &Plan, s: &mut ExecutionState, exit_render: &str) {
        for _ in 0..50 {
            for r in s.runs.iter_mut() {
                if r.history.last() != Some(&Running) || r.rendered.is_some() {
                    continue;
                }
                match &plan.tasks[r.task].kind {
                    TaskKind::If { .. } => r.rendered = Some("false".into()),
                    TaskKind::Exit { state } if expression(state.as_deref()).is_some() => {
                        r.rendered = Some(exit_render.into())
                    }
                    _ => {}
                }
            }
            let cmds = advance(plan, s, T);
            if cmds.is_empty() {
                return;
            }
            for c in &cmds {
                apply(s, c);
            }
        }
        panic!("advance did not settle: {s:?}");
    }

    /// The corpus shape: an `Exit` in an `If`'s `else`, and a task after the
    /// `If` that must never be created.
    fn in_if(state_line: &str) -> String {
        format!(
            "id: e\nnamespace: t\ntasks:\n  - id: if\n    type: io.kestra.plugin.core.flow.If\n    condition: \"{{{{ false }}}}\"\n    then:\n      - id: hello\n        type: io.kestra.plugin.core.log.Log\n        message: hi\n    else:\n      - id: exit\n        type: io.kestra.plugin.core.execution.Exit\n{state_line}  - id: end\n    type: io.kestra.plugin.core.log.Log\n    message: end\n"
        )
    }

    /// An `Exit` beside a sibling leaf in a `Parallel`: the sibling is started
    /// and stays `RUNNING` (a worker holds it).
    fn in_parallel(state_line: &str) -> String {
        format!(
            "id: e\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n      - id: sib\n        type: io.kestra.plugin.core.log.Log\n        message: sib\n      - id: exit\n        type: io.kestra.plugin.core.execution.Exit\n{state_line}"
        )
    }

    fn literal(s: &str) -> String {
        format!("        state: {s}\n")
    }

    fn fresh(p: &Plan) -> ExecutionState {
        ExecutionState::new(p)
    }

    #[test]
    fn exit_is_planned_executor_level_with_its_state_source() {
        let p = plan(&in_if(&literal("WARNING")));
        assert_eq!(
            p.tasks[task(&p, "exit")].kind,
            TaskKind::Exit {
                state: Some("WARNING".into())
            }
        );
        let p = plan(&in_if(""));
        assert_eq!(
            p.tasks[task(&p, "exit")].kind,
            TaskKind::Exit { state: None }
        );
    }

    /// Non-`KILLED`, in an `If`: the `Exit` run, the `If` and the execution
    /// take the state; `end` is never created. The golden's `valids/exit`,
    /// `valids/exit-cancelled` shape, for every state and the default.
    #[test]
    fn a_non_killed_exit_in_an_if_ends_the_run_its_parents_and_the_execution() {
        for (line, want) in [
            (String::new(), Success),
            (literal("SUCCESS"), Success),
            (literal("WARNING"), Warning),
            (literal("FAILED"), Failed),
            (literal("CANCELLED"), Cancelled),
        ] {
            let p = plan(&in_if(&line));
            let mut s = fresh(&p);
            settle(&p, &mut s, "");
            let (i, e) = (run_of(&p, &s, "if"), run_of(&p, &s, "exit"));
            assert_eq!(s.runs.len(), 2, "{want:?}: only `if` and `exit` exist");
            assert_eq!(s.runs[e].history, vec![Created, Running, want], "{want:?}");
            assert_eq!(
                s.runs[e].attempts.last().unwrap().history,
                vec![Created, Running, want]
            );
            assert_eq!(s.runs[i].history, vec![Created, Running, want], "{want:?}");
            assert_eq!(
                s.runs[i].attempts.last().unwrap().history,
                vec![Created, Running, want]
            );
            assert_eq!(s.history, vec![Created, Running, want], "{want:?}");
        }
    }

    /// `KILLED`, in an `If`, nothing else live: `valids/exit-killed`'s golden,
    /// which has NO `KILLING` anywhere — so the brief's "`SetExecutionState {
    /// Killing }` and nothing else" is overruled by it.
    #[test]
    fn a_killed_exit_with_nothing_else_live_is_killed_at_once_with_no_killing() {
        let p = plan(&in_if(&literal("KILLED")));
        let mut s = fresh(&p);
        settle(&p, &mut s, "");
        let (i, e) = (run_of(&p, &s, "if"), run_of(&p, &s, "exit"));
        assert_eq!(s.runs.len(), 2);
        assert_eq!(s.runs[e].history, vec![Created, Running, Killed]);
        assert_eq!(s.runs[i].history, vec![Created, Running, Killed]);
        assert_eq!(s.history, vec![Created, Running, Killed]);
    }

    /// D5: a non-`KILLED` exit beside a running sibling leaves the sibling
    /// running; the `Parallel` and the execution take the state.
    #[test]
    fn a_non_killed_exit_in_a_parallel_leaves_the_running_sibling_alone() {
        for want in [Success, Warning, Failed, Cancelled] {
            let p = plan(&in_parallel(&literal(want.wire())));
            let mut s = fresh(&p);
            settle(&p, &mut s, "");
            let (par, sib, e) = (
                run_of(&p, &s, "par"),
                run_of(&p, &s, "sib"),
                run_of(&p, &s, "exit"),
            );
            assert_eq!(s.runs[e].history, vec![Created, Running, want], "{want:?}");
            assert_eq!(
                s.runs[par].history,
                vec![Created, Running, want],
                "{want:?}"
            );
            assert_eq!(
                s.runs[sib].history,
                vec![Created, Submitted, Running],
                "{want:?}: the sibling is untouched"
            );
            assert_eq!(s.history, vec![Created, Running, want], "{want:?}");
            // The sibling ends later; a terminal execution asks for nothing.
            apply(
                &mut s,
                &Command::SetRunState {
                    run: sib,
                    state: Success,
                },
            );
            assert_eq!(advance(&p, &s, T), Vec::<Command>::new());
        }
    }

    /// `KILLED` beside a running sibling: the `Exit` run and the `Parallel` are
    /// `KILLED` at once (Kestra's `killParentTaskruns`), the running sibling
    /// becomes `KILLING` for its worker to stop (spec §3.2 step 1, done here
    /// because no route ran), and the execution is `KILLING` until it ends.
    #[test]
    fn a_killed_exit_in_a_parallel_kills_the_sibling_and_waits_for_it() {
        let p = plan(&in_parallel(&literal("KILLED")));
        let mut s = fresh(&p);
        settle(&p, &mut s, "");
        let (par, sib, e) = (
            run_of(&p, &s, "par"),
            run_of(&p, &s, "sib"),
            run_of(&p, &s, "exit"),
        );
        assert_eq!(s.runs[e].history, vec![Created, Running, Killed]);
        assert_eq!(s.runs[par].history, vec![Created, Running, Killed]);
        assert_eq!(
            s.runs[sib].history,
            vec![Created, Submitted, Running, Killing]
        );
        assert_eq!(s.history, vec![Created, Running, Killing]);
        // The worker's half (spec §4.2), then `advance_killing` closes it.
        apply(
            &mut s,
            &Command::SetRunState {
                run: sib,
                state: Killed,
            },
        );
        settle(&p, &mut s, "");
        assert_eq!(s.history, vec![Created, Running, Killing, Killed]);
    }

    /// A live sibling leaf that no worker holds -- here one waiting for its
    /// retry -- is `KILLED` directly (spec §3.2 step 2), and with nothing
    /// else live the execution is `KILLED` in the same round.
    #[test]
    fn a_killed_exit_kills_a_sibling_no_worker_holds() {
        let p = plan(&in_parallel(&literal("KILLED")));
        let mut s = fresh(&p);
        // Up to the round where `exit` is RUNNING and not yet decided.
        for _ in 0..10 {
            let started = s
                .runs
                .iter()
                .any(|r| r.task == task(&p, "exit") && r.history.last() == Some(&Running));
            if started {
                break;
            }
            for c in &advance(&p, &s, T) {
                apply(&mut s, c);
            }
        }
        let (sib, e) = (run_of(&p, &s, "sib"), run_of(&p, &s, "exit"));
        assert_eq!(s.runs[e].history, vec![Created, Running]);
        s.runs[sib].history = vec![Created, Submitted, Running, Failed, Retrying];
        let cmds = advance(&p, &s, T);
        assert!(
            cmds.contains(&Command::SetRunState {
                run: sib,
                state: Killed
            }),
            "{cmds:?}"
        );
        assert_eq!(
            cmds.last(),
            Some(&Command::SetExecutionState { state: Killed })
        );
    }

    /// Nested, `valids/exit-nested`'s shape: every ancestor takes the state.
    #[test]
    fn a_nested_exit_ends_every_ancestor() {
        let p = plan(
            "id: e\nnamespace: t\ntasks:\n  - id: outer\n    type: io.kestra.plugin.core.flow.Sequential\n    tasks:\n      - id: inner\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: exit\n            type: io.kestra.plugin.core.execution.Exit\n            state: FAILED\n  - id: after\n    type: io.kestra.plugin.core.log.Log\n    message: no\n",
        );
        let mut s = fresh(&p);
        settle(&p, &mut s, "");
        assert_eq!(s.runs.len(), 3, "`after` is never created");
        for id in ["outer", "inner", "exit"] {
            assert_eq!(
                s.runs[run_of(&p, &s, id)].history,
                vec![Created, Running, Failed],
                "{id}"
            );
        }
        assert_eq!(s.history, vec![Created, Running, Failed]);
    }

    /// A templated `state:` waits for the caller's render, then takes it.
    #[test]
    fn a_templated_state_is_rendered_by_the_caller() {
        let p = plan(&in_if("        state: \"{{ inputs.s }}\"\n"));
        assert_eq!(
            p.tasks[task(&p, "exit")].kind,
            TaskKind::Exit {
                state: Some("{{ inputs.s }}".into())
            }
        );
        // Unrendered: the Exit run is started and then waited on.
        let mut s = fresh(&p);
        for _ in 0..10 {
            for r in s.runs.iter_mut() {
                if matches!(p.tasks[r.task].kind, TaskKind::If { .. })
                    && r.history.last() == Some(&Running)
                {
                    r.rendered = Some("false".into());
                }
            }
            let cmds = advance(&p, &s, T);
            if cmds.is_empty() {
                break;
            }
            for c in &cmds {
                apply(&mut s, c);
            }
        }
        let e = run_of(&p, &s, "exit");
        assert_eq!(
            s.runs[e].history,
            vec![Created, Running],
            "waits for the render"
        );
        assert_eq!(s.history, vec![Created, Running]);
        // Rendered.
        settle(&p, &mut s, "CANCELLED");
        assert_eq!(s.runs[e].history, vec![Created, Running, Cancelled]);
        assert_eq!(s.history, vec![Created, Running, Cancelled]);
    }

    /// `state: NOPE` is a run failure naming `NOPE`: the plan offers it for
    /// rendering (it is not one of the five), the render pass fails the run
    /// with [`parse_state`]'s message, and a rendered `NOPE` reaching
    /// `advance` anyway fails the run rather than guessing.
    #[test]
    fn an_unknown_state_fails_the_run_naming_the_value() {
        let p = plan(&in_if(&literal("NOPE")));
        assert_eq!(expression(Some("NOPE")), Some("NOPE"));
        assert_eq!(expression(Some("KILLED")), None);
        assert_eq!(expression(None), None);
        let why = parse_state("NOPE").unwrap_err();
        assert!(why.contains("NOPE"), "{why}");

        let mut s = fresh(&p);
        settle(&p, &mut s, "NOPE");
        let (i, e) = (run_of(&p, &s, "if"), run_of(&p, &s, "exit"));
        assert_eq!(s.runs[e].history, vec![Created, Running, Failed]);
        assert_eq!(s.runs[i].history, vec![Created, Running, Failed]);
        assert_eq!(s.history, vec![Created, Running, Failed]);
    }

    // ---- final review C1: nothing is started in the round an Exit kills ----

    /// Every round until `advance` is quiet, with a worker's half for a
    /// `KILLING` leaf (it writes `KILLED`, spec §4.2), so the only way this
    /// stops short of a terminal execution is a run nobody will ever move.
    fn settle_with_worker(plan: &Plan, s: &mut ExecutionState) {
        for _ in 0..50 {
            let mut cmds = advance(plan, s, T);
            for (ix, r) in s.runs.iter().enumerate() {
                if r.history.last() == Some(&Killing)
                    && matches!(plan.tasks[r.task].kind, TaskKind::Runnable { .. })
                {
                    cmds.push(Command::SetRunState {
                        run: ix,
                        state: Killed,
                    });
                }
            }
            if cmds.is_empty() {
                return;
            }
            for c in &cmds {
                apply(s, c);
            }
        }
        panic!("advance did not settle: {s:?}");
    }

    /// The execution ended `KILLED` and no run is left live: in particular no
    /// run is `CREATED` (claim never takes one) or `RUNNING`/`SUBMITTED` (a
    /// worker would claim it and run a killed execution's task).
    fn assert_killed_and_nothing_live(p: &Plan, s: &ExecutionState, what: &str) {
        assert_eq!(s.history.last(), Some(&Killed), "{what}: {:?}", s.history);
        for r in &s.runs {
            assert!(
                r.history.last().is_some_and(|st| st.is_terminal()),
                "{what}: {} is left {:?}",
                p.tasks[r.task].id,
                r.history
            );
        }
    }

    /// `Parallel{ exit(KILLED), seq: Sequential[log] }`, in both child orders:
    /// the round the `Exit` decides in is the round `seq` would create `log`.
    /// That `CreateRun` must not be applied: under a `KILLING` execution a
    /// `CREATED` leaf is never claimed and the execution would wait for ever.
    #[test]
    fn a_killed_exit_creates_no_leaf_in_its_own_round_in_either_order() {
        let exit = "      - id: exit\n        type: io.kestra.plugin.core.execution.Exit\n        state: KILLED\n";
        let seq = "      - id: seq\n        type: io.kestra.plugin.core.flow.Sequential\n        tasks:\n          - id: log\n            type: io.kestra.plugin.core.log.Log\n            message: hi\n";
        for (order, children) in [("exit first", [exit, seq]), ("seq first", [seq, exit])] {
            let p = plan(&format!(
                "id: e\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n{}{}",
                children[0], children[1]
            ));
            let mut s = fresh(&p);
            // Up to the round where `exit` is RUNNING and undecided.
            for _ in 0..10 {
                let e = task(&p, "exit");
                if s.runs
                    .iter()
                    .any(|r| r.task == e && r.history.last() == Some(&Running))
                {
                    break;
                }
                for c in &advance(&p, &s, T) {
                    apply(&mut s, c);
                }
            }
            assert_eq!(
                s.runs[run_of(&p, &s, "seq")].history.last(),
                Some(&Running),
                "{order}: the fixture has seq live beside the exit"
            );
            let deciding = advance(&p, &s, T);
            assert!(
                !deciding
                    .iter()
                    .any(|c| matches!(c, Command::CreateRun { .. } | Command::StartRun { .. })),
                "{order}: the Exit's round creates and starts nothing: {deciding:?}"
            );
            for c in &deciding {
                apply(&mut s, c);
            }
            settle_with_worker(&p, &mut s);
            assert_killed_and_nothing_live(&p, &s, order);
        }
    }

    /// The `SUBMITTED`→`RUNNING` variant: a sibling leaf still `CREATED` when
    /// the `Exit` decides is started by the same round. With the `Exit`
    /// listed first, the kill's `KILLED` is applied before the start, which
    /// would leave the run `RUNNING` and unleased under a killed execution.
    #[test]
    fn a_killed_exit_starts_no_sibling_in_its_own_round_in_either_order() {
        let exit = "      - id: exit\n        type: io.kestra.plugin.core.execution.Exit\n        state: KILLED\n";
        let sib =
            "      - id: sib\n        type: io.kestra.plugin.core.log.Log\n        message: sib\n";
        for (order, children) in [("exit first", [exit, sib]), ("sib first", [sib, exit])] {
            let p = plan(&format!(
                "id: e\nnamespace: t\ntasks:\n  - id: par\n    type: io.kestra.plugin.core.flow.Parallel\n    tasks:\n{}{}",
                children[0], children[1]
            ));
            let mut s = fresh(&p);
            for _ in 0..10 {
                let e = task(&p, "exit");
                if s.runs
                    .iter()
                    .any(|r| r.task == e && r.history.last() == Some(&Running))
                {
                    break;
                }
                for c in &advance(&p, &s, T) {
                    apply(&mut s, c);
                }
            }
            // `sib` has not been started yet (a `runIf:` rendered late, a
            // `concurrent:` slot): the next round both decides the Exit and
            // starts `sib`.
            let sib_ix = run_of(&p, &s, "sib");
            s.runs[sib_ix].history = vec![Created];
            s.runs[sib_ix].attempts.clear();
            let deciding = advance(&p, &s, T);
            assert!(
                !deciding.iter().any(|c| matches!(
                    c,
                    Command::StartRun { .. }
                        | Command::SetRunState {
                            state: Submitted | Running,
                            ..
                        }
                )),
                "{order}: the Exit's round starts nothing: {deciding:?}"
            );
            for c in &deciding {
                apply(&mut s, c);
            }
            settle_with_worker(&p, &mut s);
            assert_killed_and_nothing_live(&p, &s, order);
        }
    }

    #[test]
    fn parse_state_reads_the_five_by_exact_name() {
        assert_eq!(parse_state("SUCCESS"), Ok(Success));
        assert_eq!(parse_state("WARNING"), Ok(Warning));
        assert_eq!(parse_state("KILLED"), Ok(Killed));
        assert_eq!(parse_state("FAILED"), Ok(Failed));
        assert_eq!(parse_state("CANCELLED"), Ok(Cancelled));
        assert!(parse_state("success").is_err());
    }
}
