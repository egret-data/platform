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
//! a sibling leaf that is running is killed by its worker. `advance_killing`
//! writes no leaf (`kill.rs`'s "A leaf is never written here"): the kill route
//! does that half (spec §3.2 steps 1-2). An `Exit` is a kill nobody routed, so
//! this module does the route's two leaf steps itself, for this execution:
//! a running `Runnable` becomes `KILLING` (its worker stops it, spec §4), and
//! every other live leaf — not started, retrying, a `Subflow` — becomes
//! `KILLED`. Flowables are left to `advance_killing`, exactly as the route
//! leaves them.
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
fn kill(plan: &Plan, state: &ExecutionState, run: RunIx) -> Vec<Command> {
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
            // Step 1: a worker may hold it; it stops it (spec §4).
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
