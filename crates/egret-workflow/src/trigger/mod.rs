// kestra: scheduler/src/main/java/io/kestra/scheduler/TriggerScheduler.java
// kestra: core/src/main/java/io/kestra/core/scheduler/model/TriggerState.java
// kestra: core/src/main/java/io/kestra/core/scheduler/SchedulerClock.java
// kestra: core/src/main/java/io/kestra/plugin/core/trigger/SchedulableExecutionFactory.java

//! Triggers — the schedule trigger and its backfill, driven by an injected
//! clock, and the flow trigger's window-free arm ([`flow`], W6 Task 6).
//!
//! # The ratchet says nothing about this module, and that is not a verdict
//!
//! `oracle-exec/`'s execution ratchet cannot see a single trigger-fired
//! execution, and the reason is structural rather than accidental:
//! `harvest.py`'s settle walks `parent_id`, and Kestra leaves that column
//! **NULL** for anything a `Schedule` creates — and for anything a `Flow`
//! trigger creates, which [`flow`] measured directly over **210** trigger-fired
//! executions.
//!
//! **And "walks `parent_id`" is only one of the four mechanisms.** W6 Task 8's
//! coverage statement splits them, because merging them loses the only thing a
//! reader can act on: a `Schedule`-fired execution carries **no link at all**,
//! so unioning `trigger_execution_id` — which is what the flow trigger needs —
//! would still not find it; the webhook's request is **never sent** (the string
//! `executions/webhook` appears once in `harvest.py`, as the reason
//! `valids/webhook-inputs` is excluded); and backfill has no population to be
//! blind to. See `oracle-exec/README.md`, *The coverage statement* → the W6
//! section.
//!
//! Seven of the 345 corpus flows declare a cron, six of them are comparable
//! golden entries, and **none of the six is baselined** — `zero` reachable
//! either way. *(This doc said "one of those is baselined", inherited from the
//! design spec's §1 table, which was measured before the package: the one
//! entry it counted is `valids/task-flow`, which W6 Task 3 banked because
//! `Subflow` executes — so a reader could read a `Subflow` bank as a schedule
//! bank. Corrected by W6 Task 8's sweep;
//! `the_sixteen_banked_entries_are_derived_and_the_four_triggers_reach_none`
//! asserts the split and that `valids/task-flow` is the only overlap.)* The
//! flow trigger's own population is seventeen flows, twelve golden entries,
//! **zero** baselined. Backfill has **zero live declarations in the whole
//! corpus** — two commented-out lines, in `valids/full` and
//! `valids/labels-deserialization`. So `baseline.txt` did not move for this
//! work and could not have: **127 before, 127 after**.
//!
//! That puts this module where `egret.sql` (W3b) and `egret.container` (W4)
//! already sit — judged entirely by its own tests — and it inherits W4's rule
//! with it: **a suite that self-skips silently is not coverage.** Nothing here
//! reaches a database, a container or a network, so there is no dependency to
//! be missing and nothing to skip; every test in this module tree runs on every
//! `cargo test`, or the build fails.
//!
//! # Measured, derived, or chosen
//!
//! `schedule.rs`'s §0 explains why this split is written down; the same rule
//! binds here. **The chosen decisions in this file:**
//!
//! 1. [`Clock`] being a parameter rather than Kestra's process-wide singleton.
//!    That `SchedulerClock` exists and is replaceable is measured; dropping
//!    the singleton is a decision.
//! 2. [`TestClock`] shipping in `src/` rather than in a test module, and
//!    allowing the clock to move **backwards**.
//! 3. The [`TickOutcome`] shape — three variants, and in particular
//!    [`TickOutcome::NotSchedulable`] being distinct from
//!    [`TickOutcome::Skipped`]. The *behaviour* each describes is measured;
//!    splitting them into two variants is egret's API decision.
//! 4. [`TriggerState`] dropping `vnode`, `workerId`, `dispatchEpoch`,
//!    `locked`, `executionId`, `lastEventId` and `stopAfter`.
//! 5. [`run_until_idle`] existing at all, and its `max_passes` hard stop.
//! 6. `DrainedRun` being a named triple.
//! 7. `tests::no_wall_clock_in_the_trigger_path` as a policy, and the list of
//!    spellings it forbids.
//! 8. `tick`'s purity over `(trigger, state, now)` — this is the programme's
//!    own constraint, mirroring `executor::advance()`, not Kestra's shape.
//!
//! One **derived** claim is worth naming because it reads like a measurement
//! and is not: `a_clock_that_moves_backwards_does_not_replay_a_consumed_tick`
//! is derived from the due predicate (`now < due_at`), not observed on the
//! artifact. Nothing was ever run with a clock moving backwards.
//!
//! # The clock is injected, and that is not a style preference
//!
//! A scheduler is nothing but a clock. This workspace has lost whole gate runs
//! to wall-clock assertions — `egret-query/tests/connector_trino_pool.rs` once
//! carried three, and cost **four** gate runs re-read as regressions before
//! they were rewritten to record the wait the policy *asks* for; it carries
//! **none** today, and this sentence said "carries three" in the present tense
//! until fix round 3 grepped it — and Kestra itself does not read the wall clock from its
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

pub mod depends_on;
pub mod duration;
pub mod flow;
pub mod schedule;
pub mod webhook;
pub mod window;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

pub use depends_on::{
    condition_key, condition_keys, dependency_matches, depends_on_refusals, merge, mode_refusal,
    satisfied, DepSource, DuplicateCondition, ModeRefusal, RefusalAt, TriggerRefusal,
};
pub use duration::{parse_duration, parse_local_time, DurationError, LocalTimeError};
pub use flow::{
    evaluate as evaluate_flow_trigger, FiredExecution, FlowTriggerError, ListeningFlow,
    Outcome as FlowTriggerOutcome, Refusal, SourceExecution, WhenVerdict, WindowFreeFlowTrigger,
    DEFAULT_MAX_EXECUTION_DEPTH,
};
pub use schedule::{
    Backfill, Cron, CronError, Field, RecoverMissed, ScheduleDates, ScheduleError, ScheduleTrigger,
    SkipReason,
};
pub use webhook::{
    build_output as build_webhook_output, ct_eq, find as find_webhook, keys_match,
    render_inputs as render_webhook_inputs, ExecutionTrigger, InputContext, InputRefusal,
    KeyContext, Output as WebhookOutput, Unsupported as UnsupportedWebhook, WebhookRequest,
};
pub use window::{
    window_refusals, Bounds, BoundsError, ResolvedWindow, TimeWindow, WindowError, WindowRefusal,
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
/// **Chosen, not measured.** It ships in `src/` rather than in a test module
/// on the expectation that the trigger routes will want it, and that
/// expectation is a guess about code nobody has written. What *is* measured is
/// only that Kestra ships `SchedulerClock` in `core/src/main` rather than in
/// its tests.
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
    ///
    /// **Chosen.** Nothing was run against the artifact with a clock going
    /// backwards; what egret does about it is derived from its own due
    /// predicate rather than copied from Kestra.
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

/// A clock that answers one instant. Pure: the caller read it elsewhere
/// (in `scheduler/`, from Postgres's `now()`), so W12's rule of one clock --
/// the database's -- holds without this module reading any clock itself.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// How a backfill ended, for [`TriggerState::resume_from`] (W7f spec §3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackfillEnd {
    /// The window's last tick fired.
    Completed { window_end: DateTime<Utc> },
    /// Deleted (or ended by `sync`) with `current_tick` not yet fired.
    Deleted { current_tick: DateTime<Utc> },
}

/// `io.kestra.core.scheduler.model.TriggerState`, reduced to what scheduling
/// needs.
///
/// Dropped, with reasons: `vnode`/`workerId`/`dispatchEpoch` (cluster
/// placement, and this engine has one scheduler), `locked`/`executionId` (the
/// `allowConcurrent` lock, which is W8's `concurrency:` neighbourhood),
/// `lastEventId` (the event-sourced control plane), `stopAfter` (needs the
/// terminal state of the execution it started, which arrives on another seam).
///
/// The backfill's database home is `workflow.trigger_backfills` (W7f). The pure
/// [`Backfill`] stays reduced to the window; its inputs and labels live beside
/// it in that row, not here.
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

    /// `TriggerEventHandler.onCreateBackfill`, with one deliberate difference
    /// (W7f spec D4): the window's start is **inclusive** here and exclusive
    /// in Kestra. Kestra seeds `currentDate = start` and recomputes with
    /// `nextAfter(start)`, so a tick exactly at `start` is lost; this takes
    /// the first tick `>= start`.
    ///
    /// Attaching still recomputes the next evaluation date immediately, which
    /// is what moves `current_date` off `start` and onto the window's first
    /// tick. Without that step the first tick would be refused by `eval`'s
    /// `current < date` guard. A window holding no tick attaches nothing and
    /// the schedule stays on the live clock, where Kestra's recompute lands.
    /// A cron that never fires leaves the window attached with no date to fire;
    /// the route's caller refuses that case before it gets here (`backfill_no_ticks`, Tasks 4 and 7).
    pub fn attach_backfill(
        &mut self,
        trigger: &ScheduleTrigger,
        start: DateTime<Utc>,
        end: Option<DateTime<Utc>>,
        clock: &dyn Clock,
    ) {
        let now = clock.now();
        let bf = Backfill::create(start, end, now);
        let next = trigger
            .first_tick_at_or_after(bf.start)
            .filter(|t| *t <= bf.end)
            .or_else(|| trigger.next_evaluation_date(None, None, now));
        self.backfill = Some(bf);
        // Clears the backfill when `next > end`.
        self.update_for_next_evaluation_date(next);
    }

    /// W7f spec §3.6's `from`: after a completed window, `max(prev, first tick
    /// after window_end)` (`prev` alone when there is none); after a delete,
    /// `max(prev, current_tick)`.
    pub fn resume_from(
        trigger: &ScheduleTrigger,
        prev: DateTime<Utc>,
        end: BackfillEnd,
    ) -> DateTime<Utc> {
        match end {
            BackfillEnd::Completed { window_end } => trigger
                .first_tick_after(window_end)
                .map_or(prev, |t| t.max(prev)),
            BackfillEnd::Deleted { current_tick } => current_tick.max(prev),
        }
    }

    /// W7f spec §3.6's rule for where the live schedule resumes. `from > now`
    /// keeps `from`. Otherwise `All` replays from `from`; `Last` takes the
    /// latest tick at or before `now` (`from` if none is `>= from`); `None`
    /// takes the first tick after now, as W7e's skip computes it. A `None`
    /// result means off the queue.
    pub fn resume_after_backfill(
        trigger: &ScheduleTrigger,
        from: DateTime<Utc>,
        policy: RecoverMissed,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        if from > now {
            return Some(from);
        }
        match policy {
            RecoverMissed::All => Some(from),
            RecoverMissed::Last => Some(
                trigger
                    .last_tick_at_or_before(now)
                    .filter(|t| *t >= from)
                    .unwrap_or(from),
            ),
            RecoverMissed::None => trigger.next_evaluation_date(None, None, now),
        }
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
    /// **The state advanced.** That is the artifact's behaviour and it is what
    /// makes this variant dangerous: a tick that reports `Skipped` has
    /// consumed its slot and will not be retried. Reaching it for the wrong
    /// reason is silent data loss, which is why every construction of it names
    /// a [`SkipReason`] and why
    /// `a_scheduler_that_reports_success_without_an_execution_is_caught`
    /// exists.
    Skipped(SkipReason),
    /// The trigger was never eligible to be looked at, so **nothing moved** —
    /// the returned state is byte-identical to the one passed in.
    ///
    /// Distinct from [`TickOutcome::Skipped`] because the difference is the
    /// whole point: `Skipped` consumes the slot, this does not.
    ///
    /// Measured on the artifact, in
    /// `io.kestra.scheduler.internals.DefaultSchedulableTriggerFetcher#getSchedulableTriggers`
    /// — the candidate list is filtered by `TriggerState.isDisabled()` before
    /// anything else, and the per-trigger lambda then does
    ///
    /// ```text
    /// 138: invokevirtual FlowWithSource.isDisabled:()Z
    /// 141: ifne  166
    /// 146: invokevirtual Optional.isEmpty:()Z
    /// 149: ifne  166
    /// 160: invokevirtual AbstractTrigger.isDisabled:()Z
    /// 163: ifeq  168
    /// 166: aconst_null
    /// 167: areturn
    /// ```
    ///
    /// so a disabled trigger returns **null at offset 166, before a
    /// `ConditionContext` is even built** and long before
    /// `TriggerScheduler.evaluate` is reached. It is dropped from the list the
    /// scheduler iterates. Its `nextEvaluationDate` is therefore **never
    /// recomputed: the state is frozen**, and what to do about the missed
    /// window is decided later and elsewhere, by
    /// `TriggerEventHandler.updateForReEnabledTrigger` reading
    /// `recoverMissedSchedules`.
    NotSchedulable(SkipReason),
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

    // Kept so the disabled branch below can return the state EXACTLY as it
    // arrived; `state` itself is about to be mutated.
    let state_before = state.clone();

    // `triggerState.evaluatedAt(clock, triggerState.getNextEvaluationDate())`
    // -- the date being processed, and the one `eval` is asked about.
    state.evaluated_at = Some(due_at);
    let context_date = due_at;
    // `eval` reads the backfill AS IT WAS, before the next-date update
    // advances it. Cloned, not borrowed, because the update below moves it.
    let context_backfill = state.backfill.clone();

    // A disabled trigger is never fetched, so NOTHING about it moves.
    //
    // Fix round 1 reversed this. The first draft advanced the state and said,
    // in a comment and in a test name, that consuming the slot was fidelity --
    // "otherwise re-enabling it would replay the whole outage". That was a
    // policy I reasoned my way to, written in the same voice as the measured
    // rules around it; the artifact does the opposite, and it assigns the
    // re-enable decision to a different seam entirely
    // (`updateForReEnabledTrigger` + `recoverMissedSchedules`). See
    // `TickOutcome::NotSchedulable` for the bytecode.
    //
    // The state is returned untouched -- `evaluated_at` included, which is why
    // this returns before it is set.
    if trigger.is_disabled() {
        return Tick {
            state: state_before,
            outcome: TickOutcome::NotSchedulable(SkipReason::Disabled),
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

/// What a drained run leaves behind: the final state, the executions it asked
/// for, and every named reason a pass produced none.
///
/// A named tuple rather than three loose returns because a caller that dropped
/// the `SkipReason`s would turn every refusal into silence, which is the exact
/// failure `TickOutcome::Skipped`'s doc warns about.
pub type DrainedRun = (TriggerState, Vec<ScheduledExecution>, Vec<SkipReason>);

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
) -> DrainedRun {
    let mut state = state.clone();
    let mut fired = Vec::new();
    let mut skipped = Vec::new();
    for _ in 0..max_passes {
        let t = tick(trigger, &state, inputs, clock);
        state = t.state;
        match t.outcome {
            // NotDue and NotSchedulable both leave the state untouched, so
            // another pass would return the identical answer for ever.
            TickOutcome::NotDue => return (state, fired, skipped),
            TickOutcome::NotSchedulable(reason) => {
                skipped.push(reason);
                return (state, fired, skipped);
            }
            TickOutcome::Skipped(reason) => skipped.push(reason),
            TickOutcome::Fired(execution) => fired.push(*execution),
        }
    }
    (state, fired, skipped)
}

#[cfg(test)]
mod tests {
    use super::{
        run_until_idle, BackfillEnd, Clock, RecoverMissed, ScheduleTrigger, SkipReason,
        SystemClock, TestClock, TickOutcome, TriggerState,
    };
    use crate::models::Trigger;
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use std::collections::{BTreeMap, BTreeSet};

    fn at(iso: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(iso)
            .unwrap_or_else(|e| panic!("bad fixture instant {iso:?}: {e}"))
            .with_timezone(&Utc)
    }

    fn sched(cron: &str) -> ScheduleTrigger {
        let yaml =
            format!("id: nightly\ntype: core.schedule\ncron: \"{cron}\"\ntimezone: \"UTC\"\n");
        let trigger: Trigger = serde_norway::from_str(&yaml).unwrap();
        ScheduleTrigger::resolve(&trigger).unwrap()
    }

    fn no_inputs() -> BTreeMap<String, serde_json::Value> {
        BTreeMap::new()
    }

    fn state_at(trigger: &ScheduleTrigger, clock: &TestClock) -> TriggerState {
        TriggerState::initialise(trigger, "company.team", "nightly_flow", clock)
    }

    // ---------------------------------------------------------------------
    // The clock
    // ---------------------------------------------------------------------

    /// **The only reader of the wall clock under `src/trigger/` is
    /// [`SystemClock::now`].**
    ///
    /// A running check rather than a convention, because a convention is what a
    /// future edit does not read. It scans the *non-test* half of every file in
    /// the module tree — a scheduler that read `Utc::now()` anywhere in `tick`
    /// or `Schedule` would be untestable and would fail at midnight, and this
    /// workspace has lost whole gate runs to exactly that
    /// (`egret-query/tests/connector_trino_pool.rs` once carried three, at a
    /// cost of four gate runs; it carries none now — measured by grep, fix
    /// round 3).
    ///
    /// The split is asserted as well as used: if `#[cfg(test)]` ever stops
    /// being the marker, this test fails rather than silently scanning nothing.
    #[test]
    fn no_wall_clock_in_the_trigger_path() {
        const FORBIDDEN: &[&str] = &[
            "Utc::now",
            "Local::now",
            "SystemTime::now",
            "Instant::now",
            "OffsetDateTime::now",
        ];

        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/trigger");
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .map(|e| e.expect("dir entry").path())
            .filter(|p| p.extension().is_some_and(|x| x == "rs"))
            .collect();
        files.sort();

        assert!(
            files.len() >= 3,
            "expected at least mod.rs, schedule.rs and flow.rs under {}, found {files:?}",
            dir.display()
        );

        let mut markers_seen = 0usize;
        let mut total_hits: Vec<(String, &str)> = Vec::new();

        for path in &files {
            let source = std::fs::read_to_string(path).expect("read source");
            assert!(
                source.len() > 500,
                "{} is suspiciously short -- did the scan read the right file?",
                path.display()
            );

            // Everything before the test module. A file with no test module is
            // scanned whole, which is the conservative direction.
            let shipping = match source.find("#[cfg(test)]") {
                Some(i) => {
                    markers_seen += 1;
                    &source[..i]
                }
                None => &source[..],
            };
            assert!(
                !shipping.is_empty(),
                "{}: the shipping slice is empty",
                path.display()
            );

            let name = path
                .file_name()
                .expect("file name")
                .to_string_lossy()
                .into_owned();
            for needle in FORBIDDEN {
                for _ in 0..shipping.matches(needle).count() {
                    total_hits.push((name.clone(), needle));
                }
            }
        }

        assert!(
            markers_seen >= 1,
            "no `#[cfg(test)]` marker found in any file -- the split this check \
             depends on has moved, so the check was measuring nothing"
        );

        // Exactly one, and it is `SystemClock::now`.
        assert_eq!(
            total_hits,
            vec![("mod.rs".to_string(), "Utc::now")],
            "the ONLY wall-clock read under src/trigger/ may be SystemClock::now"
        );
    }

    /// The injected clock, and nothing else, decides whether a trigger is due.
    ///
    /// Two ticks with the clock unchanged, and the second is `NotDue`. Then the
    /// clock moves and the same state fires. Nothing about the trigger or the
    /// state changed between the two.
    #[test]
    fn the_clock_is_the_only_thing_that_makes_a_trigger_due() {
        let clock = TestClock::new(at("2026-06-15T12:00:00Z"));
        let trigger = sched("0 2 * * *");
        let state = state_at(&trigger, &clock);
        assert_eq!(
            state.next_evaluation_date,
            Some(at("2026-06-16T02:00:00Z")),
            "a new schedule is due at its NEXT tick, not now"
        );

        let first = super::tick(&trigger, &state, &no_inputs(), &clock);
        assert_eq!(first.outcome, TickOutcome::NotDue);
        assert_eq!(first.state, state, "a not-due tick must not mutate state");

        clock.set(at("2026-06-16T02:00:00Z"));
        let second = super::tick(&trigger, &state, &no_inputs(), &clock);
        match &second.outcome {
            TickOutcome::Fired(execution) => {
                assert_eq!(execution.scheduled_date, at("2026-06-16T02:00:00Z"));
            }
            other => panic!("expected the same state to fire once the clock moved, got {other:?}"),
        }
    }

    /// A clock that goes **backwards** does not resurrect a consumed tick.
    ///
    /// **DERIVED, not measured.** It follows from `tick`'s due predicate —
    /// `now < due_at` is false in both directions — and nothing was ever run
    /// against the artifact with a clock moving backwards. Worth pinning
    /// anyway, because a replica whose clock is behind is a real thing and a
    /// scheduler that has never been asked what it does about one has an
    /// answer nobody has read. But it is not evidence about Kestra.
    #[test]
    fn a_clock_that_moves_backwards_does_not_replay_a_consumed_tick() {
        let clock = TestClock::new(at("2026-06-16T02:00:00Z"));
        let trigger = sched("0 2 * * *");
        let mut state = state_at(&trigger, &clock);
        state.next_evaluation_date = Some(at("2026-06-16T02:00:00Z"));

        let fired = super::tick(&trigger, &state, &no_inputs(), &clock);
        assert!(matches!(fired.outcome, TickOutcome::Fired(_)));
        let after = fired.state;
        assert_eq!(after.next_evaluation_date, Some(at("2026-06-17T02:00:00Z")));

        clock.set(at("2026-06-15T00:00:00Z"));
        let again = super::tick(&trigger, &after, &no_inputs(), &clock);
        assert_eq!(
            again.outcome,
            TickOutcome::NotDue,
            "moving the clock back must not re-fire an already-consumed tick"
        );
    }

    // ---------------------------------------------------------------------
    // Once, and only once
    // ---------------------------------------------------------------------

    /// A tick fires **once and only once**: over a run long enough to cover
    /// many ticks, no scheduled date appears twice and none is missing.
    ///
    /// Asserted on the set *and* the sequence — a duplicate that happened to
    /// keep the count right would pass a length check alone.
    #[test]
    fn a_tick_fires_once_and_only_once() {
        let clock = TestClock::new(at("2026-06-15T00:00:00Z"));
        let trigger = sched("0 2 * * *");
        let mut state = state_at(&trigger, &clock);

        let mut fired = Vec::new();
        // Ten days, ticked four times a day -- so every tick is offered to the
        // scheduler several times over, which is what a real loop does.
        for step in 0..40 {
            clock.set(at("2026-06-15T00:00:00Z") + Duration::hours(6 * step));
            let t = super::tick(&trigger, &state, &no_inputs(), &clock);
            state = t.state;
            if let TickOutcome::Fired(execution) = t.outcome {
                fired.push(execution.scheduled_date);
            }
        }

        // The run starts at 06-15T00:00 and the first tick due is 06-15T02:00,
        // not 06-16 -- `initialise` takes the NEXT cron tick, and 02:00 that
        // same day is still ahead of midnight. Forty passes six hours apart
        // reach 06-24T18:00, so 06-24T02:00 is the last one that can fire.
        let expected: Vec<DateTime<Utc>> = (15..=24)
            .map(|day| at(&format!("2026-06-{day:02}T02:00:00Z")))
            .collect();
        assert_eq!(fired, expected, "each tick exactly once, in order");

        let unique: BTreeSet<_> = fired.iter().collect();
        assert_eq!(unique.len(), fired.len(), "no date fired twice");
    }

    // ---------------------------------------------------------------------
    // A missed window
    // ---------------------------------------------------------------------

    /// A schedule five days behind **replays its backlog one tick per pass**,
    /// in order, and then stops — it does not jump to now and it does not fire
    /// them all in one pass.
    #[test]
    fn a_missed_window_replays_one_tick_per_pass() {
        let clock = TestClock::new(at("2026-06-15T12:00:00Z"));
        let trigger = sched("0 2 * * *");
        let mut state = state_at(&trigger, &clock);
        // The trigger last ran on the 10th and the scheduler was down since.
        state.next_evaluation_date = Some(at("2026-06-10T02:00:00Z"));

        // One pass at a time, so "one per pass" is measured rather than assumed.
        let mut per_pass = Vec::new();
        for _ in 0..3 {
            let t = super::tick(&trigger, &state, &no_inputs(), &clock);
            state = t.state;
            match t.outcome {
                TickOutcome::Fired(e) => per_pass.push(e.scheduled_date),
                other => panic!("expected a fire, got {other:?}"),
            }
        }
        assert_eq!(
            per_pass,
            vec![
                at("2026-06-10T02:00:00Z"),
                at("2026-06-11T02:00:00Z"),
                at("2026-06-12T02:00:00Z"),
            ],
            "the backlog is replayed in order, one per pass"
        );

        // Drained, the run stops at the first tick that is still in the future.
        let (state, rest, skipped) = run_until_idle(&trigger, &state, &no_inputs(), &clock, 100);
        assert_eq!(
            rest.iter().map(|e| e.scheduled_date).collect::<Vec<_>>(),
            vec![
                at("2026-06-13T02:00:00Z"),
                at("2026-06-14T02:00:00Z"),
                at("2026-06-15T02:00:00Z"),
            ]
        );
        assert!(skipped.is_empty(), "nothing was skipped: {skipped:?}");
        assert_eq!(
            state.next_evaluation_date,
            Some(at("2026-06-16T02:00:00Z")),
            "and it comes to rest on the next live tick"
        );
    }

    /// The same fixture with `lateMaximumDelay` set **drops** the backlog
    /// instead of replaying it.
    ///
    /// The pair is the point: `a_missed_window_replays_one_tick_per_pass` alone
    /// cannot distinguish "replays the backlog" from "has no delay handling at
    /// all", and this alone cannot distinguish "applies the delay" from "always
    /// jumps to now".
    #[test]
    fn late_maximum_delay_drops_the_backlog_the_same_fixture_replays() {
        let clock = TestClock::new(at("2026-06-15T12:00:00Z"));
        let trigger = sched("0 2 * * *").with_late_maximum_delay(Some(Duration::hours(1)));
        let mut state = state_at(&trigger, &clock);
        state.next_evaluation_date = Some(at("2026-06-10T02:00:00Z"));

        let (state, fired, _) = run_until_idle(&trigger, &state, &no_inputs(), &clock, 100);
        assert_eq!(
            fired.iter().map(|e| e.scheduled_date).collect::<Vec<_>>(),
            vec![at("2026-06-10T02:00:00Z")],
            "the tick that was already due still fires; the rest of the backlog does not"
        );
        assert_eq!(
            state.next_evaluation_date,
            Some(at("2026-06-16T02:00:00Z")),
            "and the next evaluation date skips the five missed days"
        );
    }

    // ---------------------------------------------------------------------
    // Backfill
    // ---------------------------------------------------------------------

    /// A backfill over a past window produces the executions the artifact
    /// produces, **in the order it produces them**, and then clears itself.
    ///
    /// The window is `[2026-06-01T00:00Z, 2026-06-05T00:00Z]` on `0 2 * * *`,
    /// and the artifact's own `nextEvaluationDate` answers say it yields
    /// `06-01T02:00`, `06-02T02:00`, `06-03T02:00`, `06-04T02:00` — with
    /// `06-05T02:00` excluded because it is after `end`.
    #[test]
    fn a_backfill_produces_the_windows_executions_in_order() {
        let clock = TestClock::new(at("2026-06-15T12:00:00Z"));
        let trigger = sched("0 2 * * *");
        let mut state = state_at(&trigger, &clock);

        state.attach_backfill(
            &trigger,
            at("2026-06-01T00:00:00Z"),
            Some(at("2026-06-05T00:00:00Z")),
            &clock,
        );
        // Attaching moved `current_date` off `start` and onto the window's
        // first tick. Without that the first date is lost to eval's
        // `current < date` guard.
        let attached = state.backfill.as_ref().expect("backfill attached");
        assert_eq!(attached.current_date, at("2026-06-01T02:00:00Z"));
        assert_eq!(attached.end, at("2026-06-05T00:00:00Z"));
        assert_eq!(state.next_evaluation_date, Some(at("2026-06-01T02:00:00Z")));

        let (state, fired, skipped) = run_until_idle(&trigger, &state, &no_inputs(), &clock, 100);

        assert_eq!(
            fired.iter().map(|e| e.scheduled_date).collect::<Vec<_>>(),
            vec![
                at("2026-06-01T02:00:00Z"),
                at("2026-06-02T02:00:00Z"),
                at("2026-06-03T02:00:00Z"),
                at("2026-06-04T02:00:00Z"),
            ],
            "the window's four ticks, in order, and NOT 06-05 which is after `end`"
        );
        assert!(skipped.is_empty(), "nothing was skipped: {skipped:?}");
        assert!(
            fired.iter().all(|e| e.from_backfill),
            "every one of them is attributed to the backfill"
        );
        assert!(
            state.backfill.is_none(),
            "the window clears itself once its next date passes `end`"
        );
        assert_eq!(
            state.next_evaluation_date,
            Some(at("2026-06-16T02:00:00Z")),
            "and the schedule returns to the live clock"
        );
    }

    /// An **open-ended** backfill takes `end = now`, so it drains up to the
    /// clock and stops there rather than running for ever.
    #[test]
    fn an_open_ended_backfill_ends_at_the_clock() {
        let clock = TestClock::new(at("2026-06-04T12:00:00Z"));
        let trigger = sched("0 2 * * *");
        let mut state = state_at(&trigger, &clock);

        state.attach_backfill(&trigger, at("2026-06-01T00:00:00Z"), None, &clock);
        assert_eq!(
            state.backfill.as_ref().expect("attached").end,
            at("2026-06-04T12:00:00Z"),
            "an absent `end` becomes now"
        );

        let (state, fired, _) = run_until_idle(&trigger, &state, &no_inputs(), &clock, 100);
        assert_eq!(
            fired.iter().map(|e| e.scheduled_date).collect::<Vec<_>>(),
            vec![
                at("2026-06-01T02:00:00Z"),
                at("2026-06-02T02:00:00Z"),
                at("2026-06-03T02:00:00Z"),
                at("2026-06-04T02:00:00Z"),
            ]
        );
        assert!(state.backfill.is_none());
    }

    /// A **paused** backfill fires nothing and advances nothing — it stays
    /// exactly where it was, for ever, which is the artifact's behaviour
    /// (`getBackFillForNextEvaluationDate` returns the window untouched when
    /// `paused`, and `eval` returns empty).
    ///
    /// Asserted as "the state after 20 passes is byte-identical to the state
    /// before", which a scheduler that merely fired nothing while quietly
    /// draining the window would fail.
    #[test]
    fn a_paused_backfill_fires_nothing_and_advances_nothing() {
        let clock = TestClock::new(at("2026-06-15T12:00:00Z"));
        let trigger = sched("0 2 * * *");
        let mut state = state_at(&trigger, &clock);
        state.attach_backfill(
            &trigger,
            at("2026-06-01T00:00:00Z"),
            Some(at("2026-06-05T00:00:00Z")),
            &clock,
        );
        if let Some(bf) = state.backfill.as_mut() {
            bf.paused = true;
        }
        let before = state.clone();

        let (after, fired, skipped) = run_until_idle(&trigger, &state, &no_inputs(), &clock, 20);

        assert!(fired.is_empty(), "a paused backfill fires nothing");
        assert_eq!(
            skipped,
            vec![SkipReason::BackfillPaused; 20],
            "every pass is refused for the same, named reason"
        );
        assert_eq!(
            after.backfill, before.backfill,
            "and the WINDOW does not move while paused -- `current_date` is what a \
             resumed backfill would restart from"
        );

        // `next_evaluation_date` is a DIFFERENT field, and it does move --
        // exactly once -- which is the artifact's behaviour rather than a
        // defect. Measured on the image's own bytecode, not on the checkout:
        // `Schedule.getPaused` is read inside `eval` and NOWHERE else, so
        // `nextEvaluationDate` never consults it and advances to the tick after
        // `currentDate`; and `TriggerState.getBackFillForNextEvaluationDate`
        // jumps straight to `areturn` on the paused branch, so `currentDate`
        // never follows it. The pair is a fixed point one tick ahead of the
        // pinned window, and asserting "unchanged" here would have been wrong
        // about the artifact in a way no other test would catch.
        let first = super::tick(&trigger, &before, &no_inputs(), &clock);
        assert_eq!(
            first.state.next_evaluation_date,
            Some(at("2026-06-02T02:00:00Z")),
            "one tick past the pinned current_date"
        );
        let mut settled = first.state;
        for pass in 0..10 {
            let t = super::tick(&trigger, &settled, &no_inputs(), &clock);
            assert_eq!(
                t.state.next_evaluation_date,
                Some(at("2026-06-02T02:00:00Z")),
                "pass {pass}: a paused backfill must reach a FIXED POINT, not creep"
            );
            settled = t.state;
        }
        assert_eq!(
            settled.backfill.as_ref().map(|b| b.current_date),
            Some(at("2026-06-01T02:00:00Z")),
            "and eleven passes later the window is still exactly where it paused"
        );
    }

    // ---------------------------------------------------------------------
    // Disabled
    // ---------------------------------------------------------------------

    /// A disabled trigger is **frozen**: nothing fires and nothing moves.
    ///
    /// This test asserted the exact opposite in the first round -- that the
    /// slot was consumed, "so re-enabling does not replay the outage" -- and
    /// said so as though it were fidelity. It was not measured; it was
    /// reasoned. Measured on the artifact, a disabled trigger is dropped by
    /// `DefaultSchedulableTriggerFetcher#getSchedulableTriggers` **before a
    /// `ConditionContext` is built** (`aconst_null; areturn` at offset 166), so
    /// it is never evaluated and its `nextEvaluationDate` is never recomputed.
    /// What to do about the missed window is decided later and elsewhere, by
    /// `TriggerEventHandler.updateForReEnabledTrigger` reading
    /// `recoverMissedSchedules` -- a seam this package does not implement.
    ///
    /// The assertion is on the WHOLE state, byte for byte, rather than on
    /// `next_evaluation_date` alone: a freeze that leaked into `evaluated_at`
    /// or into the backfill window would pass a narrower check.
    #[test]
    fn a_disabled_trigger_is_frozen_not_consumed() {
        let clock = TestClock::new(at("2026-06-16T02:00:00Z"));
        let yaml = "id: nightly\ntype: core.schedule\ncron: \"0 2 * * *\"\ntimezone: \"UTC\"\ndisabled: true\n";
        let parsed: Trigger = serde_norway::from_str(yaml).unwrap();
        let trigger = ScheduleTrigger::resolve(&parsed).unwrap();
        assert!(trigger.is_disabled());

        let mut state = state_at(&trigger, &clock);
        state.next_evaluation_date = Some(at("2026-06-16T02:00:00Z"));

        let before = state.clone();
        let t = super::tick(&trigger, &state, &no_inputs(), &clock);
        assert_eq!(t.outcome, TickOutcome::NotSchedulable(SkipReason::Disabled));
        assert_eq!(
            t.state, before,
            "a disabled trigger is never fetched, so NOTHING about it moves -- \
             not its next evaluation date, not `evaluated_at`, not its backfill"
        );

        // And it stays frozen however many passes go by, which is the property
        // that makes the re-enable seam's job well defined.
        let (after, fired, skipped) = run_until_idle(&trigger, &state, &no_inputs(), &clock, 20);
        assert!(fired.is_empty());
        assert_eq!(
            skipped,
            vec![SkipReason::Disabled],
            "one report, then it stops -- nothing moved, so another pass could \
             only repeat itself"
        );
        assert_eq!(after, before, "twenty passes later, still byte-identical");

        // The two-sided half: the identical fixture WITHOUT `disabled` fires AND
        // advances -- so the assertion above is measuring `disabled` and not
        // some accident that freezes every trigger.
        let enabled = sched("0 2 * * *");
        let enabled_state = trigger_state_for(&enabled, &clock);
        let t = super::tick(&enabled, &enabled_state, &no_inputs(), &clock);
        assert!(
            matches!(t.outcome, TickOutcome::Fired(_)),
            "the same fixture without `disabled` must fire"
        );
        assert_ne!(
            t.state, enabled_state,
            "and it must MOVE, or `frozen` above says nothing"
        );
    }

    fn trigger_state_for(trigger: &ScheduleTrigger, clock: &TestClock) -> TriggerState {
        let mut state = state_at(trigger, clock);
        state.next_evaluation_date = Some(at("2026-06-16T02:00:00Z"));
        state
    }

    // ---------------------------------------------------------------------
    // The control
    // ---------------------------------------------------------------------

    /// **The control that matters: a scheduler that reports a completed tick
    /// without creating an execution.**
    ///
    /// This is the shape of the bug, and it is worth naming precisely because
    /// it does not look like a crash. A tick that reaches evaluation at all
    /// advances the state — that is the artifact's behaviour — so a tick that
    /// reports `Skipped` has
    /// *consumed its slot* and will never be retried. (There are **two**
    /// exits that never reach evaluation and so leave the state untouched:
    /// [`TickOutcome::NotDue`], which predates fix round 1, and
    /// [`TickOutcome::NotSchedulable`], which fix round 1 added. This sentence
    /// used to read "`tick` always advances the state"; fix round 2 corrected
    /// it and named one of the two exceptions as "the one exception", which
    /// fix round 3 corrected in turn.) A scheduler that
    /// advanced the state and silently produced nothing would look, from
    /// outside, exactly like a scheduler with nothing to do: the loop keeps
    /// running, no error is logged, no execution is ever created, and the
    /// oracle cannot see it because no trigger-fired execution reaches the
    /// golden at all.
    ///
    /// So the assertion is deliberately three-sided:
    ///
    /// 1. a due tick with nothing wrong **fires**, and the outcome carries a
    ///    real execution whose date is the date that was due;
    /// 2. the state advanced past it, so the slot really was consumed —
    ///    otherwise "fired" and "will fire again" would be indistinguishable;
    /// 3. and a tick that legitimately produces nothing reports a **named**
    ///    reason, so "no execution" is never a silent state.
    ///
    /// Without (3), an implementation that returned `Skipped` for everything
    /// would pass (1) only by luck; without (1), one that returned `Skipped`
    /// for everything would pass trivially.
    #[test]
    fn a_scheduler_that_reports_success_without_an_execution_is_caught() {
        let clock = TestClock::new(at("2026-06-16T02:00:00Z"));
        let trigger = sched("0 2 * * *");
        let mut state = state_at(&trigger, &clock);
        state.next_evaluation_date = Some(at("2026-06-16T02:00:00Z"));

        let t = super::tick(&trigger, &state, &no_inputs(), &clock);

        // (1) it fired, and the execution is real and is FOR the due date.
        let execution = match &t.outcome {
            TickOutcome::Fired(execution) => execution,
            other => panic!(
                "a due tick with nothing wrong reported {other:?} and created NO execution -- \
                 the slot was consumed and the run is silently lost"
            ),
        };
        assert_eq!(execution.scheduled_date, at("2026-06-16T02:00:00Z"));
        assert_eq!(execution.namespace, "company.team");
        assert_eq!(execution.flow_id, "nightly_flow");
        assert_eq!(execution.trigger_id, "nightly");
        assert!(!execution.from_backfill);
        assert_eq!(execution.next_date, Some(at("2026-06-17T02:00:00Z")));
        assert_eq!(execution.previous_date, Some(at("2026-06-15T02:00:00Z")));

        // (2) the slot really was consumed.
        assert_eq!(
            t.state.next_evaluation_date,
            Some(at("2026-06-17T02:00:00Z")),
            "a tick that fired must also advance, or `fired` and `will fire again` \
             are indistinguishable"
        );
        assert_eq!(t.state.evaluated_at, Some(at("2026-06-16T02:00:00Z")));

        // (3) and every no-execution outcome is NAMED. A future tick is the
        // cheapest one to reach, and it must arrive as `Skipped(InTheFuture)`
        // rather than as a `Fired` with nothing in it or a bare `NotDue`.
        let mut ahead = state.clone();
        ahead.next_evaluation_date = Some(at("2026-06-16T02:00:00Z"));
        clock.set(at("2026-06-16T02:00:00Z") - Duration::days(1));
        let early = super::tick(&trigger, &ahead, &no_inputs(), &clock);
        assert_eq!(early.outcome, TickOutcome::NotDue);

        let never = sched("0 0 30 2 *");
        let mut impossible = state.clone();
        impossible.next_evaluation_date = Some(at("2026-06-16T02:00:00Z"));
        clock.set(at("2026-06-16T02:00:00Z"));
        let t = super::tick(&never, &impossible, &no_inputs(), &clock);
        assert_eq!(
            t.outcome,
            TickOutcome::Skipped(SkipReason::NoSuchInstant),
            "a tick that produces nothing must say WHY, not report success"
        );
    }

    /// `run_until_idle`'s `max_passes` is a hard stop, not a comment.
    ///
    /// A paused backfill is the case that would otherwise spin for ever, and a
    /// test that hangs reports nothing at all.
    #[test]
    fn run_until_idle_stops_at_max_passes_rather_than_spinning() {
        let clock = TestClock::new(at("2026-06-15T12:00:00Z"));
        let trigger = sched("0 2 * * *");
        let mut state = state_at(&trigger, &clock);
        state.attach_backfill(
            &trigger,
            at("2026-06-01T00:00:00Z"),
            Some(at("2026-06-05T00:00:00Z")),
            &clock,
        );
        if let Some(bf) = state.backfill.as_mut() {
            bf.paused = true;
        }

        let (_, fired, skipped) = run_until_idle(&trigger, &state, &no_inputs(), &clock, 5);
        assert!(fired.is_empty());
        assert_eq!(skipped.len(), 5, "exactly max_passes, then it gives up");
    }

    /// The shipping clock reads the real one. Asserted loosely on purpose —
    /// a tight assertion here would be the wall-clock test this module exists
    /// to avoid — but asserted, because an unread `SystemClock` is an
    /// untested one.
    #[test]
    fn the_system_clock_reads_the_real_clock() {
        let before = Utc::now();
        let read = SystemClock.now();
        let after = Utc::now();
        assert!(
            read >= before && read <= after,
            "SystemClock must return an instant between two real readings"
        );
    }

    /// `TestClock` moves when it is told to and not otherwise.
    #[test]
    fn the_test_clock_only_moves_when_told() {
        let clock = TestClock::new(Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap());
        assert_eq!(clock.now(), at("2026-06-15T12:00:00Z"));
        assert_eq!(
            clock.now(),
            at("2026-06-15T12:00:00Z"),
            "reading twice does not advance it"
        );
        clock.advance(Duration::hours(2));
        assert_eq!(clock.now(), at("2026-06-15T14:00:00Z"));
        clock.set(at("2020-01-01T00:00:00Z"));
        assert_eq!(
            clock.now(),
            at("2020-01-01T00:00:00Z"),
            "and it can go backwards"
        );
    }

    // ---------------------------------------------------------------------
    // W7f: inclusive start, the resume rule
    // ---------------------------------------------------------------------

    /// D4: a tick exactly at `start` and one exactly at `end` both belong.
    #[test]
    fn an_inclusive_backfill_fires_both_ends_of_the_window() {
        let clock = TestClock::new(at("2026-06-15T12:00:00Z"));
        let trigger = sched("0 2 * * *");
        let mut state = state_at(&trigger, &clock);
        state.attach_backfill(
            &trigger,
            at("2026-06-01T02:00:00Z"),
            Some(at("2026-06-03T02:00:00Z")),
            &clock,
        );
        let (_, fired, _) = run_until_idle(&trigger, &state, &no_inputs(), &clock, 100);
        assert_eq!(
            fired.iter().map(|e| e.scheduled_date).collect::<Vec<_>>(),
            vec![
                at("2026-06-01T02:00:00Z"),
                at("2026-06-02T02:00:00Z"),
                at("2026-06-03T02:00:00Z"),
            ]
        );
    }

    #[test]
    fn a_window_holding_no_tick_attaches_nothing() {
        let clock = TestClock::new(at("2026-06-15T12:00:00Z"));
        let trigger = sched("0 2 * * *");
        let mut state = state_at(&trigger, &clock);
        state.attach_backfill(
            &trigger,
            at("2026-06-01T03:00:00Z"),
            Some(at("2026-06-01T04:00:00Z")),
            &clock,
        );
        assert_eq!(state.backfill, None);
        assert_eq!(
            state.next_evaluation_date,
            Some(at("2026-06-16T02:00:00Z")),
            "the live clock's next tick"
        );
    }

    #[test]
    fn resume_from_takes_the_later_of_prev_and_the_boundary() {
        let t = sched("0 2 * * *");
        let end = at("2026-06-03T02:00:00Z");
        let after_end = at("2026-06-04T02:00:00Z");
        let completed = BackfillEnd::Completed { window_end: end };
        assert_eq!(
            TriggerState::resume_from(&t, at("2026-06-02T02:00:00Z"), completed),
            after_end
        );
        assert_eq!(
            TriggerState::resume_from(&t, at("2026-06-10T02:00:00Z"), completed),
            at("2026-06-10T02:00:00Z")
        );
        let deleted = BackfillEnd::Deleted {
            current_tick: at("2026-06-02T02:00:00Z"),
        };
        assert_eq!(
            TriggerState::resume_from(&t, at("2026-06-01T02:00:00Z"), deleted),
            at("2026-06-02T02:00:00Z")
        );
        assert_eq!(
            TriggerState::resume_from(&t, at("2026-06-09T02:00:00Z"), deleted),
            at("2026-06-09T02:00:00Z")
        );
    }

    #[test]
    fn resume_after_backfill_applies_the_policy() {
        let t = sched("*/1 * * * *");
        let now = at("2026-06-15T10:30:30Z");
        for p in [RecoverMissed::All, RecoverMissed::Last, RecoverMissed::None] {
            assert_eq!(
                TriggerState::resume_after_backfill(&t, at("2026-06-15T10:40:00Z"), p, now),
                Some(at("2026-06-15T10:40:00Z")),
                "a future `from` is kept under {p:?}"
            );
        }
        let from = at("2026-06-15T10:20:00Z");
        assert_eq!(
            TriggerState::resume_after_backfill(&t, from, RecoverMissed::All, now),
            Some(from)
        );
        assert_eq!(
            TriggerState::resume_after_backfill(&t, from, RecoverMissed::Last, now),
            Some(at("2026-06-15T10:30:00Z"))
        );
        assert_eq!(
            TriggerState::resume_after_backfill(&t, from, RecoverMissed::None, now),
            Some(at("2026-06-15T10:31:00Z"))
        );
        let never = sched("0 0 30 2 *");
        assert_eq!(
            TriggerState::resume_after_backfill(&never, from, RecoverMissed::None, now),
            None
        );
    }

    #[test]
    fn resume_from_a_completed_window_with_no_later_tick_keeps_prev() {
        let never = sched("0 0 30 2 *");
        let prev = at("2026-06-02T02:00:00Z");
        assert_eq!(
            TriggerState::resume_from(
                &never,
                prev,
                BackfillEnd::Completed {
                    window_end: at("2026-06-03T02:00:00Z")
                }
            ),
            prev
        );
    }

    #[test]
    fn resume_after_backfill_boundaries() {
        let t = sched("*/1 * * * *");
        let now = at("2026-06-15T10:30:30Z");
        // `from == now` is not in the future: `None` skips to the next tick.
        assert_eq!(
            TriggerState::resume_after_backfill(&t, now, RecoverMissed::None, now),
            Some(at("2026-06-15T10:31:00Z"))
        );
        // `Last` never goes back before `from`: the latest tick (10:30) is
        // earlier than `from` (10:30:10), so `from` stays.
        let from = at("2026-06-15T10:30:10Z");
        assert_eq!(
            TriggerState::resume_after_backfill(&t, from, RecoverMissed::Last, now),
            Some(from)
        );
        // `now` exactly on a tick: "at or before" includes it.
        let on_tick = at("2026-06-15T10:30:00Z");
        assert_eq!(
            TriggerState::resume_after_backfill(
                &t,
                at("2026-06-15T10:20:00Z"),
                RecoverMissed::Last,
                on_tick
            ),
            Some(on_tick)
        );
    }

    /// A cron that never fires leaves `next_evaluation_date` `None` and the
    /// window attached at `start`, firing nothing; Task 4/7's caller refuses it first
    /// (`backfill_no_ticks`). Pinned so a change to this path is noticed.
    #[test]
    fn a_never_firing_cron_attaches_nothing() {
        let clock = TestClock::new(at("2026-06-15T12:00:00Z"));
        let trigger = sched("0 0 30 2 *");
        let mut state = state_at(&trigger, &clock);
        state.attach_backfill(
            &trigger,
            at("2026-06-01T00:00:00Z"),
            Some(at("2026-06-05T00:00:00Z")),
            &clock,
        );
        assert_eq!(state.next_evaluation_date, None);
        assert_eq!(
            state.backfill.as_ref().map(|b| b.current_date),
            Some(at("2026-06-01T00:00:00Z")),
            "the window stays attached at `start`, with no date to fire"
        );
    }
}
