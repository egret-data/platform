//! `Pause` — the wait, its bound, and the resume.
//!
//! `io.kestra.plugin.core.flow.Pause` is the one flowable in this corpus whose
//! whole behaviour is *not running*. It starts like any other flowable, goes
//! straight into `PAUSED`, and stays there until one of three things happens:
//! its `pauseDuration:` elapses, its `timeout:` elapses, or a human resumes it.
//! egret implements the first two. **It must not implement the third by
//! accident**, which is what this module's `None` arm is about.
//!
//! # The two bounds are different constructs with different terminals
//!
//! | written | ends at | the run's terminal | corpus |
//! |---|---|---|---|
//! | `pauseDuration: PT1S` | start + 1 s | `SUCCESS` | `valids/pause-delay` |
//! | `timeout: PT1S` | start + 1 s | `FAILED` | `valids/pause-timeout` |
//! | neither | never | none — it stays `PAUSED` | `valids/pause-test` and six more |
//!
//! **And one thing that is not a bound at all overrides the terminal**: a
//! `behavior:` written as an EXPRESSION, which on the pinned artifact fails at
//! the moment the pause ends and takes the task down whichever bound elapsed.
//! `valids/pause-behavior` is that entry and its golden is byte-identical to
//! `valids/pause-timeout`'s but for the id. See
//! [`TaskKind::Pause::dynamic_behavior`](crate::executor::TaskKind::Pause).
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
//! `onPause:`, a `Pause`'s own `tasks:`, its `errors:` — and a **literal**
//! `behavior:`, whose `WARNING` and `CANCELLED` terminals `RunState` cannot
//! hold and which no corpus flow writes. A `Pause` declaring any of them is
//! not a [`TaskKind::Pause`] at all —
//! [`plan::pause_gap`](crate::executor::plan) keeps it an opaque
//! `TaskKind::Runnable`, so it reaches this module never rather than reaching
//! it and being ignored.

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx};
use crate::executor::retry::parse_duration;
use crate::executor::state::{ExecutionState, RunIx, RunState, Tick};

use super::Step;

/// What one `Pause` run wants, at the round it is `RUNNING`.
///
/// `RUNNING` is the only state this is asked at: [`super::descend`] reaches a
/// child that is running, and a `PAUSED` one goes through [`resume`] instead.
/// The pause is entered in the same round the run is descended into for the
/// first time, which is why the golden never holds a `RUNNING` that lasted a
/// round on its own.
pub(crate) fn drive(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
) -> Step {
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
    let until = match deadline(plan, state, run, task, started) {
        Bound::NotRenderedYet => return Step::Waiting,
        Bound::Unbounded => None,
        Bound::Until(at, _) => Some(at),
    };
    Step::Commands(vec![Command::PauseRun { run, until }])
}

/// What a `PAUSED` run wants, which is nothing at all until its bound elapses.
///
/// Returns the command for the round the wait ends in, or [`Step::Waiting`].
///
/// **The execution's own `RESTARTED` is NOT emitted here**, and neither is the
/// `PAUSED` [`drive`] would otherwise owe. Both are
/// [`advance`](crate::executor::advance)'s, for one reason that is measured
/// rather than stylistic: a `Parallel` of two `Pause`es enters both in the SAME
/// command batch, and a per-run emission writes `PAUSED` twice into a history
/// no golden execution ever repeats a state in. `advance` sees the whole batch
/// and this function sees one run, so only `advance` can answer once for it —
/// see its `pause_mark` for the rule and for what is and is not measured about
/// it. `two_pauses_in_one_parallel_resume_independently` is what made this
/// module's first shape fail.
pub(crate) fn resume(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
    now: Tick,
) -> Step {
    let Some(started) = state.runs[run].attempt_started_at() else {
        return Step::Waiting;
    };
    let Bound::Until(at, ends_with) = deadline(plan, state, run, task, started) else {
        // Unbounded, or an expression the caller has not answered. **Neither
        // may resume**, and they are the same answer for different reasons: an
        // unbounded pause waits for a human this executor has no message from,
        // and an unrendered one has no bound to compare `now` against. Reading
        // either as "due" is the over-acceptance this module exists to refuse.
        return Step::Waiting;
    };
    if now < at {
        return Step::Waiting;
    }
    Step::Commands(vec![Command::SetRunState {
        run,
        state: ends_with,
    }])
}

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
/// re-derives that from all 345 flows — so the precedence below is a **choice
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
fn deadline(
    plan: &Plan,
    state: &ExecutionState,
    run: RunIx,
    task: TaskIx,
    started: Tick,
) -> Bound {
    let crate::executor::plan::TaskKind::Pause {
        duration,
        dynamic_behavior,
    } = &plan.tasks[task].kind
    else {
        // Unreachable for the same reason `drive`'s missing attempt is: both
        // callers match on this kind before they get here.
        return Bound::Unbounded;
    };

    if let Some(source) = duration.as_deref() {
        // An expression goes through the caller, exactly as an `If`'s condition
        // does -- `advance()` is pure. A literal is parsed straight, without
        // waiting for a render that would only hand back the same text:
        // `valids/pause-delay` writes `PT1S` and must not stall on a caller
        // that renders nothing.
        let text = match parse_duration(source) {
            Some(_) => source,
            None => match state.runs[run].rendered.as_deref() {
                Some(rendered) => rendered,
                None => return Bound::NotRenderedYet,
            },
        };
        return match parse_duration(text) {
            Some(d) => Bound::Until(plus(started, d), ends_with(RunState::Success, *dynamic_behavior)),
            None => Bound::Unbounded,
        };
    }

    match plan.tasks[task].timeout {
        Some(d) => Bound::Until(plus(started, d), ends_with(RunState::Failed, *dynamic_behavior)),
        None => Bound::Unbounded,
    }
}

/// The terminal a resume reaches, after the artifact's `behavior:` defect.
///
/// **A dynamic `behavior:` overrides the bound and always FAILS**, because on
/// this artifact the failure happens at the moment the pause ends rather than
/// in place of the wait: the pause still waits, still records `PAUSED`, and
/// then dies rendering the property. See
/// [`TaskKind::Pause::dynamic_behavior`](crate::executor::TaskKind::Pause) for
/// the measurement.
///
/// It is a separate function rather than an `if` at each call site because the
/// two arms above choose DIFFERENT defaults — `SUCCESS` for a `pauseDuration:`
/// and `FAILED` for a `timeout:` — and a rule written twice is a rule that can
/// disagree with itself. `the_dynamic_behavior_defect_overrides_either_bound`
/// drives both arms.
fn ends_with(bound_says: RunState, dynamic_behavior: bool) -> RunState {
    match dynamic_behavior {
        true => RunState::Failed,
        false => bound_says,
    }
}

/// `started + d`, in [`Tick`]s.
///
/// Saturating rather than wrapping: `valids/pause-kill-delay` writes `PT1M` and
/// nothing in the corpus comes near the bound, but a `P100000D` an author can
/// write would wrap an `i64` of milliseconds into the PAST, and a deadline
/// behind `now` is a pause that resumes itself on the round it begins — the one
/// failure this module is written to make impossible.
fn plus(started: Tick, d: std::time::Duration) -> Tick {
    let millis = i64::try_from(d.as_millis()).unwrap_or(i64::MAX);
    Tick(started.0.saturating_add(millis))
}
