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
//! [`plan::pause_gap`](crate::executor::plan) keeps it an opaque
//! `TaskKind::Runnable`, so it reaches this module never rather than reaching
//! it and being ignored.

use crate::executor::command::Command;
use crate::executor::plan::{Plan, TaskIx};
use crate::executor::retry::parse_duration;
use crate::executor::state::{ExecutionState, Resume, RunIx, RunState, Tick};

use super::{plus, Step};

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
pub(crate) fn drive(plan: &Plan, state: &ExecutionState, run: RunIx, task: TaskIx) -> Step {
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
    Step::Commands(vec![Command::PauseRun { run, resume }])
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
fn deadline(plan: &Plan, state: &ExecutionState, run: RunIx, task: TaskIx, started: Tick) -> Bound {
    let crate::executor::plan::TaskKind::Pause { duration } = &plan.tasks[task].kind else {
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
            Some(d) => Bound::Until(plus(started, d), RunState::Success),
            None => Bound::Unbounded,
        };
    }

    // **`timeout:` and `pauseDuration:` take DIFFERENT terminals from the two
    // `allow*` flags, and the artifact's own line is what says so** -- W8 Task
    // 5. `ExecutorService.java:937` is
    // `.state(duration != null ? behavior.mapToState() : State.Type.fail(pauseTask))`,
    // so the flags reach the `timeout:` branch and NOT the `pauseDuration:`
    // one above, which keeps `RunState::Success` whatever anybody wrote.
    // `valids/pause-timeout-allow-failure` is the flow that measures it: a
    // `Pause` with `timeout: PT1S` and `allowFailure: true` whose golden run is
    // `["CREATED","RUNNING","PAUSED","WARNING"]`.
    let task = &plan.tasks[task];
    match task.timeout {
        Some(d) => Bound::Until(
            plus(started, d),
            crate::executor::allow_failure::fail(task.allow_failure, task.allow_warning),
        ),
        None => Bound::Unbounded,
    }
}
