//! `Pause` — the wait, its bound, and the resume.
//!
//! `io.kestra.plugin.core.flow.Pause` is the one flowable in this corpus whose
//! whole behaviour is *not running*. It starts like any other flowable, goes
//! straight into `PAUSED`, and stays there until one of three things happens:
//! its `pauseDuration:` elapses, its `timeout:` elapses, or a human resumes it.
//! egret implements all three; the third only through [`release_now`], which a
//! person's resume calls (W7b Task 3 fix round 1). **No clock may implement it
//! by accident**, which is what this module's `None` arm is about.
//!
//! # The two bounds are different constructs with different terminals
//!
//! | written | ends at | the run's terminal | corpus |
//! |---|---|---|---|
//! | `pauseDuration: PT1S` | start + 1 s | `SUCCESS` | `valids/pause-delay` |
//! | `timeout: PT1S` | start + 1 s | `FAILED` | `valids/pause-timeout` |
//! | neither | never | none — it stays `PAUSED` | `valids/pause-test` and six more |
//!
//! Both are measured off the golden rather than read from Kestra's docs.
//! `valids/pause-delay`'s run is `["CREATED","RUNNING","PAUSED","SUCCESS"]` and
//! `valids/pause-timeout`'s is `["CREATED","RUNNING","PAUSED","FAILED"]`, and
//! the two flows are otherwise the same flow — a `Pause` and a
//! `core.debug.Return` behind it — so the terminal is the only thing the bound
//! decides. It decides the successor too: `pause-delay`'s `last` runs and
//! `pause-timeout`'s never exists, which is [`sequential::walk`]'s ordinary
//! rule over a failed child and not something this module does.
//!
//! **The third row is the one the oracle cannot check**, and it is the reason
//! this file exists as a module rather than as four lines inside
//! [`super::descend`]. Seven corpus flows write a `Pause` with neither bound —
//! `valids/pause-test`, `valids/pause_on_resume`,
//! `valids/pause_on_resume_optional`, `valids/loop-pause`,
//! `valids/resume-validate`, `valids/pause_no_tasks` and
//! `valids/follow-dependencies-partial-parent` — and **not one of them is in
//! `golden.json`**. They are not there because of what the artifact did with
//! them: `corpus.json` records the same disposition for all seven, *"reached
//! PAUSED and stayed there for the full 90s … it ends only on an explicit
//! resume, which a standalone harvest does not issue"*. So a `Pause` that
//! resumed itself after some default would produce a green ratchet and a wrong
//! product, and the only thing standing between those two is
//! `an_unbounded_pause_never_resumes_itself`.
//!
//! # Where the wait is measured from
//!
//! [`TaskRun::attempt_started_at`] — the instant the attempt now in flight
//! opened, which [`Command::StartRun`] carried in. Kestra measures a flowable's
//! `timeout:` from the same place, and `TaskRun::started_at` (the FIRST
//! attempt) would be the same value for every `Pause` in this corpus because
//! nothing retries one. They are different fields for a run that is retried,
//! and this one is the one that means "since this attempt began".
//!
//! # What is NOT here
//!
//! `behavior:`, `onPause:`, a `Pause`'s own `tasks:` and its `errors:`. A
//! `Pause` declaring any of them is not a [`TaskKind::Pause`] at all —
//! [`plan::pause_gap`](crate::executor::plan) flattens it an opaque
//! `TaskKind::Runnable`, and since the 2026-10-11 ruling `spec::validate`
//! refuses it (`unsupported_pause_property`), so `build_plan` returns no plan
//! for it and it reaches this module never rather than reaching it and being
//! ignored.

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx, TaskKind};
use crate::executor::retry::parse_duration;
use crate::executor::state::{ExecutionState, Resume, RunIx, RunState, Tick};

use super::{plus, run_of, Step};

/// **A manual resume** (W7b Task 3 fix round 1, the user's ruling of
/// 2026-10-11): move every `PAUSED` `Pause` run's deadline to `now`, with
/// `SUCCESS` as its terminal, and return which runs were released.
///
/// Kestra's `ExecutionService.resume` marks a paused run's new state at once,
/// whatever its bounds, and `Pause.java` says a pause *"resumed before the
/// duration (for example, from the UI) will not use the behavior property but
/// will always succeed"*. So the terminal is `SUCCESS` for every bound: an
/// unbounded pause, a `pauseDuration:` (which would end `SUCCESS` anyway) and a
/// `timeout:` (which would end `FAILED` had the deadline been reached first).
///
/// **It ends nothing itself.** It rewrites only the `Resume` this module wrote
/// when the pause was entered, so the run then leaves `PAUSED` by the one door
/// every pause leaves by: [`super::resume_paused`] answers
/// `SetRunState { SUCCESS }` at the next `advance()`, `pause_mark` owes the
/// execution `RESTARTED`, and the worker's settle writes the run, its attempt
/// and their timestamps exactly as for a deadline that passed.
///
/// A `Pause` run is told apart by its `Resume`: [`drive`] writes `Never` or
/// `Terminal`; a `LoopUntil` waiting for its next iteration carries `Iterate`
/// and is left alone. `not_before` moves with the deadline, because migration
/// 0141 pairs a `TERMINAL:*` discriminant with a non-NULL `not_before_ms`.
///
/// Idempotent: a run that is no longer `PAUSED` is not touched, which is what
/// makes a resume that loses the race to the waker release nothing.
pub(crate) fn release_now(state: &mut ExecutionState, now: Tick) -> Vec<RunIx> {
    let mut released = Vec::new();
    for (ix, r) in state.runs.iter_mut().enumerate() {
        let paused = r.history.last() == Some(&RunState::Paused);
        let a_pause = matches!(r.resume_with, Some(Resume::Never | Resume::Terminal { .. }));
        if paused && a_pause {
            r.resume_with = Some(Resume::Terminal {
                at: now,
                state: RunState::Success,
            });
            r.not_before = Some(now);
            released.push(ix);
        }
    }
    released
}

/// What one `Pause` run wants, at the round it is `RUNNING`.
///
/// `RUNNING` is the only state this is asked at: [`super::descend`] reaches a
/// child that is running, and a `PAUSED` one goes through
/// [`super::resume_paused`] instead. **That link read `[`resume`]` until fix
/// round 3** — it named an item this module has not had since fix round 2
/// deleted it.
///
/// **Nothing flagged it, and the mechanism is measured rather than guessed:**
/// rustdoc link-checks only the items it documents, and `cargo doc` documents
/// public items only. `drive` is `pub(crate)`. Control — an invented
/// `[`zzz_invented_item_qqq`]` injected into this very doc comment: plain
/// `cargo doc -p egret-workflow --no-deps` reports it **0** times,
/// `--document-private-items` reports it **3**. Since essentially all of
/// `executor/` is `pub(crate)`, **every intra-doc link in this directory is
/// unchecked by the ordinary doc build**, and a dangling one survives until
/// somebody sweeps the deleted name's own population by hand.
/// The pause is entered in the same round the run is descended into for the
/// first time, which is why the golden never holds a `RUNNING` that lasted a
/// round on its own.
///
/// **A `Pause` that has already paused does not pause again** -- package 7a
/// (spec D3). Kestra's `Pause.needPause` pauses only a run whose history holds
/// no `PAUSED`, and its `resolveState` then ends it in the resumed terminal,
/// `SUCCESS` when nothing chose another (`Pause.java:269-272`, `findTerminalState`).
/// The only way a `Pause` is `RUNNING` with `PAUSED` behind it is a restart
/// (`restart::restart`'s `ReopenFlowable`): a resume ends the run straight
/// from `PAUSED`, and a `Pause` is never retried (`retry::is_retried_kind`).
/// It is a run command and not a `Step::Done`, because `descend` returns this
/// step unmapped and a `Done` would end the enclosing list instead.
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
    now: Tick,
) -> Step {
    if state.runs[run].history.contains(&RunState::Paused) {
        return resume_body(plan, state, run, task, now);
    }
    let Some(started) = state.runs[run].attempt_started_at() else {
        // A `Pause` run reaches `RUNNING` only through
        // `flowable::start`'s `Command::StartRun`, which opens an attempt in
        // the same command batch, so this is unreachable rather than a
        // condition. `Waiting` rather than a panic for the reason every other
        // "cannot happen" in this executor is: `advance()` is handed state a
        // caller stored and read back, and a state machine that aborts the
        // process on a surprising row is a worse failure than one that stalls.
        return Step::Waiting;
    };
    // **The terminal is resolved HERE, once, and carried on the command.** It
    // used to be re-derived on every round by a `resume` in this module, from
    // the same `(plan, attempt_started_at, rendered)` -- which was correct and
    // was also the second of the two sites W8 Task 4 fix round 2 collapsed.
    // Nothing it read can change while the run is paused: the attempt does not
    // reopen, and a `pauseDuration:` the caller has not rendered never gets
    // here at all (`Bound::NotRenderedYet` waits instead of pausing).
    let resume = match deadline(plan, state, run, task, started) {
        Bound::NotRenderedYet => return Step::Waiting,
        Bound::Unbounded => Resume::Never,
        Bound::Until(at, state) => Resume::Terminal { at, state },
    };
    // **`onPause:` (package 9a D5): one task, created when the pause begins.**
    // It is a parentless sibling of the pause, as `valids/pause_on_pause`'s
    // golden records it (`parent: null`, overlapping the pause), so it is
    // created here, in the same batch that pauses the run, and driven from
    // there by [`drive_on_pause`]. Once per pause: a run for it already
    // existing means it was created, so a re-entry creates nothing. A
    // `Parallel` of two pauses enters both in one batch; each creates its own
    // distinct `onPause:` task and `advance`'s `pause_mark` still writes the
    // execution's `PAUSED` once.
    let mut cmds = Vec::new();
    if let TaskKind::Pause {
        on_pause: Some(on_pause),
        ..
    } = &plan.tasks[task].kind
    {
        if run_of(state, None, *on_pause).is_none() {
            cmds.push(Command::CreateRun {
                task: *on_pause,
                parent: None,
                value: None,
            });
        }
    }
    cmds.push(Command::PauseRun { run, resume });
    Step::Commands(cmds)
}

/// Drive a **resumed** `Pause`'s `tasks:` body (package 9a D4/D6).
///
/// Reached once the pause run is `RUNNING` again with `PAUSED` behind it: a
/// RESUME re-opens it ([`flowable::resume_paused`](super::resume_paused) emits
/// a [`Command::ReopenFlowable`] for a `Pause` with a body), and package 7a's
/// restart reaches it too. The body runs as a sequential list with the pause
/// run as its parent, its `errors:`/`finally:` driven by the shared
/// [`sequential::drive`](super::sequential::drive), and the pause run's
/// terminal is `aggregate(tasks)` — the standard flowable rule, so a failing
/// `finally:` child does not change it (spec §0.2, which the sequential driver
/// already enforces).
///
/// An **empty** body ends `SUCCESS`, which is a bare pause's resumed /
/// restarted behaviour unchanged — the one answer this arm gave for every
/// pause before 9a.
///
/// It never returns [`Step::Done`]: [`super::descend`] returns a `Pause`'s
/// step without the `Done` mapping every other flowable goes through, so the
/// body's `Done(aggregate)` is mapped to the run's terminal **here** instead.
fn resume_body(plan: &Plan, state: &ExecutionState, run: RunIx, task: TaskIx, now: Tick) -> Step {
    let TaskKind::Pause { tasks, .. } = &plan.tasks[task].kind else {
        // Unreachable: `descend` matches on this kind before it gets here.
        return Step::Commands(vec![Command::SetRunState {
            run,
            state: RunState::Success,
        }]);
    };
    let errors = plan.tasks[task].errors.as_slice();
    let finally = plan.tasks[task].finally.as_slice();

    // **9a Task 6: a failure deadline re-entered to drive its handlers.** The
    // resumed arm is reached from two re-entries that differ by their command.
    // A RESUME (`ReopenFlowable`) cleared `resume_with`, so a `None`/`SUCCESS`
    // terminal is the RESUME path -- run the `tasks:` body. A
    // `timeout:`/`behavior: FAIL|WARN` deadline (`ReenterPause`) KEPT it, so a
    // non-`SUCCESS` terminal is the failure path -- run no body, drive `errors:`
    // (on a failure) then `finally:`, and end the pause in that deadline
    // terminal. `pause::drive`'s pre-9a answer for a bare pause is the `SUCCESS`
    // + empty-`tasks:` corner of the RESUME path, unchanged.
    let terminal = match state.runs[run].resume_with {
        Some(Resume::Terminal { state: s, .. }) => s,
        _ => RunState::Success,
    };
    if terminal != RunState::Success {
        return drive_terminal(plan, state, run, errors, finally, terminal, now);
    }

    if tasks.is_empty() {
        return Step::Commands(vec![Command::SetRunState {
            run,
            state: RunState::Success,
        }]);
    }
    match super::sequential::drive(plan, state, tasks, errors, finally, Some(run), now) {
        // **The body's `aggregate(tasks)` is softened by the Pause's own
        // `allowFailure:`/`allowWarning:`**, which is the mapping
        // [`super::descend`] applies to every other flowable's `Done`
        // (`Execution.guessFinalState`'s tail, called with the task's flags). A
        // `Pause`'s step is returned from `descend` UNMAPPED, so without this a
        // `Pause { allowFailure: true, tasks: [<failing child>] }` would end
        // `FAILED` where Kestra ends `WARNING`. Only the resumed-BODY terminal:
        // the deadline/`timeout:` terminal is `pause::deadline`'s and already
        // carries its own `allow_failure::fail` mapping (and `behavior:` is Task
        // 4).
        Step::Done(aggregate) => Step::Commands(vec![Command::SetRunState {
            run,
            state: crate::executor::allow_failure::soften(
                aggregate,
                plan.tasks[task].allow_failure,
                plan.tasks[task].allow_warning,
            ),
        }]),
        other => other,
    }
}

/// Drive a failed `Pause`'s `errors:`/`finally:` handlers, then end it in its
/// deadline terminal (9a Task 6).
///
/// Reached from [`resume_body`] when the pause re-entered `RUNNING` on a
/// `timeout:`/`behavior: FAIL|WARN` deadline ([`Command::ReenterPause`]), i.e.
/// with a non-`SUCCESS` `terminal`. This is Kestra's
/// `Execution.findTaskDependingFlowState`'s `terminalState == FAILED` branch
/// (`Execution.java:595-599`), which was left out of
/// [`super::current_list`](super::current_list) until #9 because egret's `Pause`
/// ran no sub-list: a `FAILED` terminal runs the `errors:` handler, then
/// `finally:` runs whatever the terminal, and the pause's own state stays the
/// deadline terminal (never `aggregate`, so a succeeding handler does not
/// upgrade a `FAILED` pause to `SUCCESS`). A `WARNING` deadline is not a failure,
/// so it runs `finally:` alone. The handler runs hang off the pause run as their
/// parent, one at a time, through the shared [`super::sequential::walk_errors`].
fn drive_terminal(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    errors: &[TaskIx],
    finally: &[TaskIx],
    terminal: RunState,
    now: Tick,
) -> Step {
    let started = |list: &[TaskIx]| {
        list.iter().any(|&t| {
            run_of(state, Some(run), t).is_some_and(|ix| !state.runs[ix].history.is_empty())
        })
    };
    let exhausted = |list: &[TaskIx]| {
        list.iter()
            .all(|&t| run_of(state, Some(run), t).is_some_and(|ix| super::is_terminal(state, ix)))
    };
    let end = || {
        Step::Commands(vec![Command::SetRunState {
            run,
            state: terminal,
        }])
    };

    // Once `finally:` has started, continue it to the end -- Kestra's
    // "finally is already started, just continue it" (`Execution.java:589-593`).
    if started(finally) {
        return super::sequential::walk_errors(plan, state, finally, Some(run), now)
            .unwrap_or_else(end);
    }
    // A FAILED pause runs its `errors:` handler first (`:601-613`). A WARNING
    // deadline is not a failure, so it skips straight to `finally:`.
    if terminal == RunState::Failed && !errors.is_empty() && !exhausted(errors) {
        if let Some(step) = super::sequential::walk_errors(plan, state, errors, Some(run), now) {
            return step;
        }
    }
    // Then `finally:`, always (`:615-620`).
    if !finally.is_empty() {
        return super::sequential::walk_errors(plan, state, finally, Some(run), now)
            .unwrap_or_else(end);
    }
    // No `finally:`: once `errors:` (if any) is done, end in the deadline terminal.
    end()
}

/// Whether a `Pause`'s `onPause:` run exists and has not reached a terminal
/// state. The execution must not terminalize while one is: it is no root, so
/// `aggregate(roots)` cannot see it, and a `CREATED` leaf under a terminal
/// execution is never claimed -- the task would be silently dropped. The golden
/// (`valids/pause_on_pause`) ends the execution only after both runs are done.
pub(crate) fn on_pause_in_flight(plan: &Plan, state: &ExecutionState) -> bool {
    state.runs.iter().any(|r| {
        let TaskKind::Pause {
            on_pause: Some(on_pause),
            ..
        } = &plan.tasks[r.task].kind
        else {
            return false;
        };
        run_of(state, None, *on_pause).is_some_and(|ix| !super::is_terminal(state, ix))
    })
}

/// Drive every `onPause:` run a `Pause` created ([`drive`]): they hang off no
/// list the walk visits, so `advance` asks here, once per round, and gets the
/// ordinary walk of the one-task list (`CREATED` is opened, `RUNNING` is
/// descended into, a terminal run is left).
///
/// A pause whose `onPause:` run does not exist yet is skipped: [`drive`]
/// creates it in the batch that pauses the run, and creating it here too would
/// make a second one in that same round.
pub(crate) fn drive_on_pause(plan: &Plan, state: &ExecutionState, now: Tick) -> Vec<Command> {
    let mut cmds = Vec::new();
    for r in &state.runs {
        let TaskKind::Pause {
            on_pause: Some(on_pause),
            ..
        } = &plan.tasks[r.task].kind
        else {
            continue;
        };
        if run_of(state, None, *on_pause).is_none() {
            continue;
        }
        if let Some(Step::Commands(c)) =
            super::sequential::drive_list(plan, state, &[*on_pause], None, now)
        {
            cmds.extend(c);
        }
    }
    cmds
}

// **`resume` used to live here and is now
// [`flowable::resume_paused`](super::resume_paused).** W8 Task 4 fix round 2.
//
// It re-derived this pause's bound and terminal on every round while
// `loop_until` had a near-copy beside it, and each was reached through its own
// `TaskKind`-keyed guard in two different walks. Both are gone: the answer is
// decided once, by [`drive`] above, and carried on
// [`Command::PauseRun`](crate::executor::Command::PauseRun) as a
// [`Resume`]. What that buys is not tidiness — see [`Resume`] for the silent
// stall it makes unrepresentable.
//
// **The `RESTARTED` this pause's end owes the execution is still not emitted
// per run**, and that is unchanged and still measured:
// [`advance`](crate::executor::advance)'s `pause_mark` answers once per
// command batch, because a `Parallel` of two `Pause`es enters both in ONE
// batch and a per-run emission writes `PAUSED` twice into a history no golden
// execution ever repeats a state in.
// `two_pauses_in_one_parallel_resume_independently` is what made this
// module's first shape fail.

/// When this pause ends, and what it ends as.
enum Bound {
    /// `pauseDuration:` is an expression and the caller has not rendered it.
    NotRenderedYet,
    /// Neither `pauseDuration:` nor `timeout:`. Nothing this executor holds
    /// ends it.
    Unbounded,
    Until(Tick, RunState),
}

/// [`Bound`], from the plan and from whatever the caller has rendered.
///
/// # `pauseDuration` wins over `timeout`, and nothing in this corpus can tell
///
/// No corpus `Pause` writes both — `every_corpus_pause_bound_is_one_or_none`
/// re-derives that from all 351 flows — so the precedence below is a **choice
/// with no measurement behind it**, stated rather than implied. It is Kestra's
/// shape in the narrow sense that `pauseDuration` is `Pause`'s own field and
/// `timeout` is inherited from `Task`, and the more specific one is the one the
/// author wrote about pausing. Whoever gets a flow that writes both should
/// measure it on the artifact before trusting this line.
///
/// # A `pauseDuration:` that will not parse is UNBOUNDED, not an error
///
/// Measured unreachable from this corpus rather than argued:
/// `every_corpus_pause_bound_is_one_or_none` also asserts that every literal
/// `pauseDuration:` and every `timeout:` on a corpus `Pause` parses. The
/// alternative — failing the run — would be inventing a terminal the artifact
/// has not been measured to produce, and an unbounded pause is the one answer
/// that can never be an over-acceptance.
fn deadline(plan: &Plan, state: &ExecutionState, run: RunIx, task: TaskIx, started: Tick) -> Bound {
    let crate::executor::plan::TaskKind::Pause {
        duration, behavior, ..
    } = &plan.tasks[task].kind
    else {
        // Unreachable for the same reason `drive`'s missing attempt is: both
        // callers match on this kind before they get here.
        return Bound::Unbounded;
    };

    // The caller's render, split back into the two expressions it answers
    // ([`rendered_expression`]). `None` while an expression is unanswered.
    let rendered = state.runs[run].rendered.as_deref();
    let (duration_expr, behavior_expr) = expressions(duration.as_deref(), behavior.as_deref());
    let rendered_parts = match rendered {
        None => None,
        Some(r) => match split_rendered(r, duration_expr, behavior_expr) {
            Some(parts) => Some(parts),
            // Both were expected and the separator is missing (D2): fail.
            None => return Bound::Until(started, RunState::Failed),
        },
    };

    let Some(source) = duration.as_deref() else {
        // **`timeout:` takes the `allow*` flags and NOT `behavior:`** -- W8
        // Task 5. `ExecutorService.java:937` is
        // `.state(duration != null ? behavior.mapToState() : State.Type.fail(pauseTask))`,
        // so only the `pauseDuration:` branch reads `behavior:` and the
        // `timeout:` one is `fail(allowFailure, allowWarning)` whatever
        // `behavior:` says. `valids/pause-timeout-allow-failure` measures it: a
        // `Pause` with `timeout: PT1S` and `allowFailure: true` whose golden run
        // is `["CREATED","RUNNING","PAUSED","WARNING"]`.
        let task = &plan.tasks[task];
        return match task.timeout {
            Some(d) => Bound::Until(
                plus(started, d),
                crate::executor::allow_failure::fail(task.allow_failure, task.allow_warning),
            ),
            None => Bound::Unbounded,
        };
    };

    // An expression goes through the caller, exactly as an `If`'s condition
    // does -- `advance()` is pure. A literal is parsed straight, without
    // waiting for a render that would only hand back the same text:
    // `valids/pause-delay` writes `PT1S` and must not stall on a caller
    // that renders nothing.
    let text = if duration_expr {
        match rendered_parts {
            Some((Some(d), _)) => d,
            _ => return Bound::NotRenderedYet,
        }
    } else {
        source
    };
    let Some(d) = parse_duration(text) else {
        return Bound::Unbounded;
    };
    let at = plus(started, d);

    // **`behavior:` (package 9a D2/D3), applied at the `pauseDuration:`
    // deadline only.** A manual resume never gets here ([`release_now`]), so a
    // Pause resumed before its deadline succeeds whatever it says. Unset is
    // `RESUME`, `SUCCESS`: every pre-9a figure unchanged.
    let Some(source) = behavior.as_deref() else {
        return Bound::Until(at, RunState::Success);
    };
    let text = if behavior_expr {
        match rendered_parts {
            Some((_, Some(b))) => b,
            _ => return Bound::NotRenderedYet,
        }
    } else {
        source
    };
    Bound::Until(at, behavior_terminal(text))
}

/// What `behavior:` ends the pause as at its deadline. `RESUME` is `SUCCESS`,
/// which the resume gate ([`super::resume_paused`]) answers by running the
/// body; the other three are non-`SUCCESS`, so that gate runs no body.
///
/// **A value outside the four fails the run** (spec D2) where Kestra's
/// `Behavior.valueOf` throws inside the executor and leaves the pause
/// `RUNNING` for ever. A literal never gets here with one -- `pause_plan_gap`
/// refuses it -- so this is the rendered-expression case.
fn behavior_terminal(text: &str) -> RunState {
    match text.trim() {
        "RESUME" => RunState::Success,
        "WARN" => RunState::Warning,
        "CANCEL" => RunState::Cancelled,
        _ => RunState::Failed,
    }
}

/// Which of a `Pause`'s two expressions need the caller: `(pauseDuration,
/// behavior)`. A `pauseDuration:` that already parses is a complete answer; a
/// `behavior:` needs a render only when it contains a template tag.
fn expressions(duration: Option<&str>, behavior: Option<&str>) -> (bool, bool) {
    (
        duration.is_some_and(|d| parse_duration(d).is_none()),
        // `behavior:` is read only at a `pauseDuration:` deadline
        // (`ExecutorService.java:937`), so without one there is nothing to
        // render.
        duration.is_some() && behavior.is_some_and(crate::executor::plan::behavior_is_expression),
    )
}

/// Joins the two renders in the single `rendered` slot when both are needed.
const RENDER_SEP: char = '\u{1f}';

/// The one template the caller renders for a `Pause`, or `None` when nothing
/// needs it. A lone expression is rendered as itself (so `pauseDuration:`'s
/// `rendered` is unchanged from before 9a); both are joined by [`RENDER_SEP`]
/// and read back by [`split_rendered`]. One slot rather than a second column:
/// `rendered` is stored, and a new field would be a migration.
pub(crate) fn rendered_expression(kind: &TaskKind) -> Option<String> {
    let TaskKind::Pause {
        duration, behavior, ..
    } = kind
    else {
        return None;
    };
    let (d, b) = expressions(duration.as_deref(), behavior.as_deref());
    match (d, b) {
        (false, false) => None,
        (true, false) => duration.clone(),
        (false, true) => behavior.clone(),
        (true, true) => Some(format!(
            "{}{RENDER_SEP}{}",
            duration.as_deref().unwrap_or_default(),
            behavior.as_deref().unwrap_or_default()
        )),
    }
}

/// [`rendered_expression`]'s answer split back: `(duration, behavior)`, each
/// `Some` only for an expression that was rendered.
///
/// `None` is a rendered slot that should hold both but has no separator, which
/// the caller cannot have written: [`deadline`] fails the pause on it (spec
/// D2, never a hang) instead of waiting for an answer that will not come.
fn split_rendered(rendered: &str, d: bool, b: bool) -> Option<(Option<&str>, Option<&str>)> {
    Some(match (d, b) {
        (true, true) => {
            let (d, b) = rendered.split_once(RENDER_SEP)?;
            (Some(d), Some(b))
        }
        (true, false) => (Some(rendered), None),
        (false, true) => (None, Some(rendered)),
        (false, false) => (None, None),
    })
}
