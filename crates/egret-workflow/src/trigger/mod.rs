// kestra: scheduler/src/main/java/io/kestra/scheduler/TriggerScheduler.java
// kestra: core/src/main/java/io/kestra/core/scheduler/model/TriggerState.java
// kestra: core/src/main/java/io/kestra/core/scheduler/SchedulerClock.java
// kestra: core/src/main/java/io/kestra/plugin/core/trigger/SchedulableExecutionFactory.java

//! Triggers — the schedule trigger and its backfill, driven by an injected
//! clock.
//!
//! # The ratchet says nothing about this module, and that is not a verdict
//!
//! `oracle-exec/`'s execution ratchet cannot see a single trigger-fired
//! execution, and the reason is structural rather than accidental:
//! `harvest.py`'s settle walks `parent_id`, and Kestra leaves that column
//! **NULL** for anything a `Schedule` creates. Seven of the 345 corpus flows
//! declare a cron and one of those is baselined; **zero are reachable**.
//! Backfill has **zero live declarations in the whole corpus** — two
//! commented-out lines. So `baseline.txt` did not move for this work and could
//! not have: **127 before, 127 after**.
//!
//! That puts this module where `egret.sql` (W3b) and `egret.container` (W4)
//! already sit — judged entirely by its own tests — and it inherits W4's rule
//! with it: **a suite that self-skips silently is not coverage.** Nothing here
//! reaches a database, a container or a network, so there is no dependency to
//! be missing and nothing to skip; every test in this module tree runs on every
//! `cargo test`, or the build fails.
//!
//! # The clock is injected, and that is not a style preference
//!
//! A scheduler is nothing but a clock. This workspace has lost whole gate runs
//! to wall-clock assertions — `egret-query/tests/connector_trino_pool.rs`
//! carries three — and Kestra itself does not read the wall clock from its
//! trigger path either: `SchedulerClock` is a singleton `java.time.Clock` its
//! own tests replace with `Clock.fixed`. [`Clock`] is that, and
//! [`no_wall_clock_in_the_trigger_path`] is a running check that the only
//! reader of the real clock in `src/trigger/` is [`SystemClock::now`].
//!
//! [`no_wall_clock_in_the_trigger_path`]: tests::no_wall_clock_in_the_trigger_path
//!
//! # What a tick is
//!
//! [`tick`] is pure over `(trigger, state, now)` — no I/O and no clock of its
//! own — for the same reason `executor::advance()` is: it is where the
//! semantic difficulty lives, so it has to be testable without standing
//! anything up. Its shape follows `TriggerScheduler.processSchedulableTrigger`
//! exactly, including the order that matters:
//!
//! 1. the state's `evaluated_at` becomes the `next_evaluation_date` it is
//!    about to process, and **that** is the date handed to `eval`;
//! 2. the next evaluation date is recomputed and stored, which is also what
//!    advances or clears a backfill;
//! 3. `eval` runs against the **pre-update** backfill, not the advanced one.
//!
//! Getting 3 backwards would advance a backfill twice per tick and drop every
//! other window date.

pub mod schedule;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

pub use schedule::{
    Backfill, Cron, CronError, Field, ScheduleDates, ScheduleError, ScheduleTrigger, SkipReason,
};

/// The scheduler's clock, injected everywhere.
///
/// `io.kestra.core.scheduler.SchedulerClock` with the singleton removed: a
/// process-wide mutable clock is exactly what makes a scheduler's tests share
/// state, so this is a parameter instead.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// The shipping clock.
///
/// **This is the only reader of the wall clock in `src/trigger/`**, and
/// `tests::no_wall_clock_in_the_trigger_path` fails if a second one appears.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock a test drives.
///
/// Ships in `src/` rather than in a test module because the trigger routes
/// will want it too, and because Kestra ships its own for the same reason.
#[derive(Debug)]
pub struct TestClock {
    now: std::sync::Mutex<DateTime<Utc>>,
}

impl TestClock {
    pub fn new(at: DateTime<Utc>) -> Self {
        Self {
            now: std::sync::Mutex::new(at),
        }
    }

    /// Moves the clock to `at`. Moving it **backwards** is allowed on purpose:
    /// a scheduler that only ever sees time increase has never been asked what
    /// it does when a replica's clock is behind.
    pub fn set(&self, at: DateTime<Utc>) {
        *self.now.lock().expect("test clock poisoned") = at;
    }

    pub fn advance(&self, by: chrono::Duration) {
        let mut guard = self.now.lock().expect("test clock poisoned");
        *guard += by;
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("test clock poisoned")
    }
}

/// `io.kestra.core.scheduler.model.TriggerState`, reduced to what scheduling
/// needs.
///
/// Dropped, with reasons: `vnode`/`workerId`/`dispatchEpoch` (cluster
/// placement, and this engine has one scheduler), `locked`/`executionId` (the
/// `allowConcurrent` lock, which is W8's `concurrency:` neighbourhood),
/// `lastEventId` (the event-sourced control plane), `stopAfter` (needs the
/// terminal state of the execution it started, which arrives on another seam).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerState {
    pub namespace: String,
    pub flow_id: String,
    pub trigger_id: String,
    /// When this trigger is next due. `None` means "never scheduled" — a cron
    /// that selects no instant at all leaves it `None` (see
    /// [`SkipReason::NoSuchInstant`]).
    pub next_evaluation_date: Option<DateTime<Utc>>,
    /// The date last handed to `eval`. `TriggerState.evaluatedAt`, and what
    /// `TriggerState.context()` puts in `TriggerContext.date`.
    pub evaluated_at: Option<DateTime<Utc>>,
    pub backfill: Option<Backfill>,
}

impl TriggerState {
    /// `TriggerScheduler.onStart`'s new-trigger branch: a schedule's first
    /// evaluation date is its **next** cron tick, not now.
    pub fn initialise(
        trigger: &ScheduleTrigger,
        namespace: &str,
        flow_id: &str,
        clock: &dyn Clock,
    ) -> Self {
        let now = clock.now();
        Self {
            namespace: namespace.to_string(),
            flow_id: flow_id.to_string(),
            trigger_id: trigger.trigger_id.clone(),
            next_evaluation_date: trigger.next_evaluation_date(None, None, now),
            evaluated_at: None,
            backfill: None,
        }
    }

    /// `TriggerEventHandler.onCreateBackfill`.
    ///
    /// Attaching a backfill **immediately recomputes the next evaluation
    /// date**, which is what moves `current_date` off `start` and onto the
    /// window's first cron tick. Without that step the first tick would be
    /// refused by `eval`'s `current < date` guard and the window would lose its
    /// first date.
    pub fn attach_backfill(
        &mut self,
        trigger: &ScheduleTrigger,
        start: DateTime<Utc>,
        end: Option<DateTime<Utc>>,
        clock: &dyn Clock,
    ) {
        let now = clock.now();
        self.backfill = Some(Backfill::create(start, end, now));
        let next = trigger.next_evaluation_date(self.evaluated_at, self.backfill.as_ref(), now);
        self.update_for_next_evaluation_date(next);
    }

    /// `TriggerState.updateForNextEvaluationDate` +
    /// `getBackFillForNextEvaluationDate`.
    ///
    /// The backfill rule lives here rather than beside the tick because it is
    /// the same rule for every path that sets a next evaluation date — attach,
    /// tick, and error recovery — and a window that only ends on one of them
    /// is a window that leaks.
    fn update_for_next_evaluation_date(&mut self, next: Option<DateTime<Utc>>) {
        self.next_evaluation_date = next;
        self.backfill = match (self.backfill.take(), next) {
            (Some(bf), Some(next)) if !bf.paused => {
                if next > bf.end {
                    // The window is finished the moment its next date passes
                    // `end`.
                    None
                } else {
                    Some(Backfill {
                        current_date: next,
                        ..bf
                    })
                }
            }
            (bf, _) => bf,
        };
    }
}

/// The execution a fired tick asks for.
///
/// Deliberately **not** carrying rendered `trigger.*` variables as strings:
/// Kestra builds those with `Schedule.Output.toMap(ZoneId)` through its Jackson
/// mapper, and this package has not measured the exact text that produces. The
/// three instants and the zone are carried instead, so the seam that does
/// measure it can format them without this one having guessed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledExecution {
    pub namespace: String,
    pub flow_id: String,
    pub trigger_id: String,
    /// `Schedule.Output.date` — the tick this execution is *for*, which is not
    /// the same as the instant it was created at.
    pub scheduled_date: DateTime<Utc>,
    pub next_date: Option<DateTime<Utc>>,
    pub previous_date: Option<DateTime<Utc>>,
    /// The zone `scheduled_date` should be read in.
    pub timezone: chrono_tz::Tz,
    /// `Schedule.inputs`, **unrendered**. Kestra renders these through a
    /// `RunContext` (`SchedulableExecutionFactory.getInputs`); rendering is
    /// `runners::render_str`'s job and happens where a render context exists.
    pub inputs: BTreeMap<String, serde_json::Value>,
    /// True when this execution came from a backfill window rather than the
    /// live clock.
    pub from_backfill: bool,
}

/// What one tick did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TickOutcome {
    /// The trigger was not due at `now`, or has no next evaluation date at all.
    NotDue,
    /// The trigger was due and was evaluated, and the evaluation produced no
    /// execution.
    ///
    /// The state still advanced — that is Kestra's behaviour and it is what
    /// makes this variant dangerous: a tick that reports `Skipped` has
    /// consumed its slot and will not be retried. Reaching it for the wrong
    /// reason is silent data loss, which is why every construction of it names
    /// a [`SkipReason`] and why `a_scheduler_that_reports_success_without_an_execution_is_caught`
    /// exists.
    Skipped(SkipReason),
    /// The trigger was due and produced an execution.
    Fired(Box<ScheduledExecution>),
}

/// The result of one tick: the state to store, and what happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tick {
    pub state: TriggerState,
    pub outcome: TickOutcome,
}

/// One scheduler tick — `TriggerScheduler.evaluate` +
/// `processSchedulableTrigger`, pure over `(trigger, state, now)`.
///
/// See this module's doc for why step order matters. `now` comes from the
/// injected [`Clock`] and nowhere else.
pub fn tick(
    trigger: &ScheduleTrigger,
    state: &TriggerState,
    inputs: &BTreeMap<String, serde_json::Value>,
    clock: &dyn Clock,
) -> Tick {
    let now = clock.now();
    let mut state = state.clone();

    let Some(due_at) = state.next_evaluation_date else {
        return Tick {
            state,
            outcome: TickOutcome::NotDue,
        };
    };
    if now < due_at {
        return Tick {
            state,
            outcome: TickOutcome::NotDue,
        };
    }

    // `triggerState.evaluatedAt(clock, triggerState.getNextEvaluationDate())`
    // -- the date being processed, and the one `eval` is asked about.
    state.evaluated_at = Some(due_at);
    let context_date = due_at;
    // `eval` reads the backfill AS IT WAS, before the next-date update
    // advances it. Cloned, not borrowed, because the update below moves it.
    let context_backfill = state.backfill.clone();

    // A disabled trigger is not evaluated at all, but its slot is still
    // consumed -- otherwise re-enabling it would replay the whole outage.
    if trigger.is_disabled() {
        let next = trigger.next_evaluation_date(Some(context_date), context_backfill.as_ref(), now);
        state.update_for_next_evaluation_date(next);
        return Tick {
            state,
            outcome: TickOutcome::Skipped(SkipReason::Disabled),
        };
    }

    let next = trigger.next_evaluation_date(Some(context_date), context_backfill.as_ref(), now);
    state.update_for_next_evaluation_date(next);

    match trigger.eval(context_date, context_backfill.as_ref(), now) {
        Err(reason) => Tick {
            state,
            outcome: TickOutcome::Skipped(reason),
        },
        Ok(dates) => {
            let execution = ScheduledExecution {
                namespace: state.namespace.clone(),
                flow_id: state.flow_id.clone(),
                trigger_id: state.trigger_id.clone(),
                scheduled_date: dates.date,
                next_date: dates.next,
                previous_date: dates.previous,
                timezone: trigger.timezone(),
                inputs: inputs.clone(),
                from_backfill: context_backfill.is_some(),
            };
            Tick {
                state,
                outcome: TickOutcome::Fired(Box::new(execution)),
            }
        }
    }
}

/// Ticks until the trigger is no longer due, collecting what it fired.
///
/// The scheduling loop applies **one** tick per pass — `TriggerScheduler`'s own
/// `onSchedule` evaluates each trigger once per cycle — so catching up on a
/// backlog takes as many passes as there are missed ticks. This helper is that
/// loop, and `max_passes` is a hard stop rather than a `while true`: a cron
/// whose next date does not advance would otherwise spin for ever, and a test
/// that hangs reports nothing.
pub fn run_until_idle(
    trigger: &ScheduleTrigger,
    state: &TriggerState,
    inputs: &BTreeMap<String, serde_json::Value>,
    clock: &dyn Clock,
    max_passes: usize,
) -> (TriggerState, Vec<ScheduledExecution>, Vec<SkipReason>) {
    let mut state = state.clone();
    let mut fired = Vec::new();
    let mut skipped = Vec::new();
    for _ in 0..max_passes {
        let t = tick(trigger, &state, inputs, clock);
        state = t.state;
        match t.outcome {
            TickOutcome::NotDue => return (state, fired, skipped),
            TickOutcome::Skipped(reason) => skipped.push(reason),
            TickOutcome::Fired(execution) => fired.push(*execution),
        }
    }
    (state, fired, skipped)
}
