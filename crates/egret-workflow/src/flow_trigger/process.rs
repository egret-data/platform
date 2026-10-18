//! `process_events`: drain `workflow.flow_trigger_events`, one event per
//! transaction, and fire every `Flow` listener that hears it -- a window-free
//! one at once (W13a, spec §4.4, §4.5), a `dependsOn` one once its window is
//! satisfied (W13b, spec §5.3). `sweep_expired_windows` is the scheduler
//! pass's cleanup of the windows. The statement-by-statement argument is in
//! the module doc of [`crate::flow_trigger`].

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use egret_core::{Error, Result};
use serde_json::{Map, Value};
use sqlx::types::Json;
use sqlx::{Connection, PgConnection, PgPool};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::models::{ExecutionKind, Flow, FlowTrigger, Label, State, Trigger, TriggerKind};
use crate::runners::{render_str_guarded, GuardedRender, RenderContext};
use crate::scheduler::start::{
    classify, flow_is_on, workflows_feature_default, Started, WORKFLOWS_FEATURE,
};
use crate::trigger::flow::{
    evaluate, FiredExecution, ListeningFlow, Outcome, SourceExecution, WhenVerdict,
    WindowFreeFlowTrigger, DEFAULT_MAX_EXECUTION_DEPTH,
};
use crate::trigger::webhook::{render_value, InputRefusal};
use crate::trigger::{
    condition_keys, dependency_matches, depends_on_refusals, merge, satisfied, DepSource,
    ResolvedWindow, TimeWindow,
};
use crate::worker::txn::{transaction, Ended, Fetched, Finish, Isolation, Savepoint, Txn};
use crate::worker::{execution_render_context, start_execution_in_with, NewExecution};

fn db_err(e: sqlx::Error) -> Error {
    Error::Internal(anyhow::anyhow!("{e}"))
}

/// Kestra's `Flow` trigger type name, as stored in `execution_trigger.type`
/// and matched by `workflow_executions_flow_trigger_event_iff_flow` (0157).
pub(crate) const FLOW_TYPE: &str = "io.kestra.plugin.core.trigger.Flow";

/// How many passes an event may take before the listeners still failing on it
/// are given up (spec §4.4 point 1). Counted in `flow_trigger_events.attempts`,
/// one per pass that left a listener undone; the pass that would write this
/// number deletes the event instead and logs the listeners at `warn`.
pub const MAX_ATTEMPTS: i32 = 5;

/// How long a kept event waits before it is claimed again (final review F2):
/// `flow_trigger_events.next_attempt_at` is set this far ahead, and the claim
/// skips an event that is not yet due. One tick of the scheduler at its
/// default interval, so [`MAX_ATTEMPTS`] counts ticks, not passes -- two
/// replicas' passes inside one backoff find the event not due.
pub const DEFAULT_RETRY_BACKOFF: Duration = Duration::from_secs(1);

/// The start of the reason a `dependsOn` listener is skipped with -- logged
/// and counted `refused` -- when its trigger has anything
/// [`depends_on_refusals`] or [`TimeWindow::from_model`] refuses. Drafts save
/// despite validation errors and `scheduler::sync` gives any parseable draft
/// a listener row, so the save-time refusals are applied again here, whole,
/// before the listener's window is touched (W13b Task 4). The refusals
/// follow, `[code] message`.
pub const DEPENDS_ON_REFUSED: &str = "the dependsOn trigger is refused at run time as at save";

/// The reason an event that meets none of a `dependsOn` listener's
/// dependencies is skipped. It touches no window: only a matching event opens
/// or replaces one (user decision D1, spec §5.3 step 1).
pub const NO_DEPENDENCY_MET: &str =
    "the event meets none of the listener's dependencies, so it touches no window";

/// What `render_str_guarded` refuses to reach, for `when`. The source's
/// render context binds no `trigger` root, and this is the artifact's
/// evidence that Kestra itself has none either -- **not, as the name once
/// implied, evidence that a `trigger`-naming expression here refuses.**
/// `listener_run1_FAILED_bad_inputs` (W13a Task 1) is the measured Kestra
/// throw, "Unable to find `trigger`" -- but it is `inputs`, not `when`, that
/// the fixture names, and `inputs` is the *other* fire below: a render
/// failure there is NOT a refusal, it is the born-FAILED execution
/// (`inputs_that_fail_to_render_create_a_failed_execution`). This guard's own
/// refusal path is [`WhenVerdict::Unrenderable`] ->
/// [`crate::trigger::flow::Refusal::WhenUnrenderable`], which no corpus
/// `when` reaches `trigger` to exercise; the citation stays
/// because it is still the only measured "Unable to find `trigger`" this
/// package has, corrected to say what it is evidence of. The guard makes
/// the refusal independent of HOW the expression names it -- `{{ [trigger] }}`
/// would otherwise render `[null]` (W6 Task 7 fix round 3).
const FORBIDDEN_ROOTS: &[&str] = &["trigger"];

/// What one pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EventReport {
    /// Events claimed, each once per pass.
    pub events: usize,
    /// Listeners that fired an execution.
    pub fired: usize,
    /// Listeners that fired an execution born FAILED because their `inputs`
    /// did not render (Kestra's behaviour, measured). Counted apart from
    /// `fired` so a pass that only produced failures does not read as success.
    pub born_failed: usize,
    /// Listeners that heard an event and did not fire, deliberately: a
    /// refusal by [`evaluate`](crate::trigger::flow::evaluate) (including a
    /// `when` that does not render), a fire the unique index says already
    /// happened, or -- for a `dependsOn` listener -- a trigger refused at run
    /// time as at save ([`DEPENDS_ON_REFUSED`]), an event that meets none of
    /// its dependencies ([`NO_DEPENDENCY_MET`]), or one outside its window.
    pub refused: usize,
    /// Listeners that failed. Either deterministically -- the flow does not
    /// parse, resolve or plan, or uses a task type this deployment does not
    /// serve -- and the event is done with them, or possibly
    /// transiently, and the event is kept for them (spec §4.4 point 1).
    pub errored: usize,
    /// `dependsOn` listeners whose window took this event without being
    /// satisfied: stored, not fired (W13b).
    pub accumulated: usize,
    /// `dependsOn` listeners that lost a race for their window row to another
    /// replica (`40001`, or `40P01`) and are retried on a later pass (W13b
    /// Task 4 fix round 1). **Not an error, and not an attempt:** the loser's
    /// failure means a competitor committed a change to that very row.
    pub contended: usize,
}

impl EventReport {
    /// Add what one event's committed transaction did to this pass's count.
    fn absorb(&mut self, d: EventReport) {
        let EventReport {
            events,
            fired,
            born_failed,
            refused,
            errored,
            accumulated,
            contended,
        } = d;
        self.events += events;
        self.fired += fired;
        self.born_failed += born_failed;
        self.refused += refused;
        self.errored += errored;
        self.accumulated += accumulated;
        self.contended += contended;
    }
}

/// The claimed event, as the claim read it under its row lock.
#[derive(Clone, sqlx::FromRow)]
struct Event {
    id: i64,
    source_execution_id: Uuid,
    workspace_id: Uuid,
    state: String,
    done: Vec<String>,
    attempts: i32,
}

/// A listener that is ON (the six off switches are in its `SELECT`).
#[derive(sqlx::FromRow)]
struct Listener {
    flow_id: Uuid,
    trigger_id: String,
    workspace_id: Uuid,
    source: String,
    namespace: String,
    flow_key: String,
}

impl Listener {
    /// The key `flow_trigger_events.done` records it under.
    fn key(&self) -> String {
        format!("{}/{}", self.flow_id, self.trigger_id)
    }
}

/// What the source execution looked like in the event's snapshot.
struct Source {
    execution: SourceExecution,
    record: SourceRecord,
}

#[derive(sqlx::FromRow)]
struct SourceRow {
    workspace_id: Uuid,
    namespace: String,
    flow_key: String,
    execution_depth: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    source: String,
    last_task_id: Option<String>,
    loop_index: Option<i32>,
    /// The execution's own labels (migration 0192), beyond its flow's.
    labels: Option<serde_json::Value>,
}

/// The source's half of `execution.trigger.variables`, with egret's values
/// already chosen. See [`flow_trigger_record`] for the per-key decisions.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SourceRecord {
    pub execution_id: String,
    pub namespace: String,
    pub flow_id: String,
    /// The EVENT's state -- the transition heard -- not the row's current one.
    pub state: String,
    pub start_date: DateTime<Utc>,
    /// `None` when the event's state is not terminal.
    pub end_date: Option<DateTime<Utc>>,
    pub last_task_id: Option<String>,
    pub labels: Vec<Label>,
}

/// What became of one listener on one event.
enum Fate {
    Fired,
    /// Fired born FAILED: the `inputs` did not render. Done with.
    FiredFailed(String),
    /// Heard and deliberately not fired. Done with.
    Refused(String),
    /// The unique index says this (event, listener) already fired. Done with.
    AlreadyFired(String),
    /// Deterministic in the listener's source and this deployment: only a
    /// save, or a deployment that serves its task types
    /// (`task_type_not_served`, no-silent-stall Task 3), helps. Done with.
    Broken(String),
    /// Possibly transient. The listener's savepoint rolled back and the event
    /// is kept for it.
    Retry(String),
    /// A `dependsOn` listener's window took the event and is not yet
    /// satisfied. Done with.
    Accumulated,
    /// A `dependsOn` listener lost a race for its window row (`40001` or
    /// `40P01`). Its savepoint rolled back; the event is kept for it, due at
    /// once, **without** spending an attempt (see [`finish_event`]).
    Contended(String),
}

/// How much one pass may do (final review I3). A pass runs on the scheduler's
/// ticker right after `fire_due`, so it must not run for as long as the outbox
/// is deep: a backlog -- the scheduler switched on after a while off (spec
/// §4.4 point 6) -- would otherwise hold every cron tick, and shutdown, until
/// it was drained.
#[derive(Debug, Clone)]
pub struct PassLimits {
    /// Stop after this many events. Checked after each event, so a pass
    /// always makes progress.
    pub max_events: usize,
    /// Stop once the pass has run this long, checked after each event (so a
    /// pass overruns by at most one event).
    pub max_duration: Duration,
    /// How far ahead a kept event's `next_attempt_at` is set (F2). The
    /// runtime passes its tick interval.
    pub retry_backoff: Duration,
}

/// [`PassLimits::max_events`]'s default.
pub const MAX_EVENTS_PER_PASS: usize = 500;
/// [`PassLimits::max_duration`]'s default.
pub const MAX_PASS_DURATION: Duration = Duration::from_secs(1);

impl Default for PassLimits {
    fn default() -> Self {
        PassLimits {
            max_events: MAX_EVENTS_PER_PASS,
            max_duration: MAX_PASS_DURATION,
            retry_backoff: DEFAULT_RETRY_BACKOFF,
        }
    }
}

/// Delete every `dependsOn` window whose end has passed (spec §5.3 step 6,
/// user decision D1): the scheduler pass calls it once, after
/// [`process_events_with`]. Returns how many it deleted.
///
/// "Passed" is `window_end <= now()`, the processor's own test for an expired
/// window. Nothing is lost by it: an expired window is one the next matching
/// event would replace anyway. It is one autocommit statement at READ
/// COMMITTED that takes its rows `FOR UPDATE SKIP LOCKED`: a window a
/// processor holds is skipped, not waited on (the processor is replacing or
/// deleting it anyway, and the next pass's sweep sees what it left), so the
/// sweep never waits and cannot close a deadlock cycle with processors that
/// hold one window and want the next (fix round 1). See the module doc,
/// statement 10.
pub async fn sweep_expired_windows(pool: &PgPool) -> Result<u64> {
    let swept = sqlx::query(
        "DELETE FROM workflow.flow_trigger_windows \
          WHERE (flow_id, trigger_id) IN ( \
                SELECT flow_id, trigger_id FROM workflow.flow_trigger_windows \
                 WHERE window_end <= now() \
                   FOR UPDATE SKIP LOCKED)",
    )
    .execute(pool)
    .await
    .map_err(db_err)?;
    Ok(swept.rows_affected())
}

/// One pass with the default [`PassLimits`] and no cancellation: what a test
/// or a one-off caller wants. The runtime calls [`process_events_with`].
pub async fn process_events(pool: &PgPool) -> Result<EventReport> {
    process_events_with(pool, &PassLimits::default(), &CancellationToken::new()).await
}

/// One pass: events claimed oldest first, each at most once, until none is
/// due, the pass reaches a [`PassLimits`] cap, or `cancel` fires -- whichever
/// comes first. What is left waits for the next pass.
///
/// Each event is claimed, evaluated, fired and then deleted (or kept, with its
/// `done` listeners, `attempts + 1` and a `next_attempt_at` one backoff ahead)
/// in **its own** transaction, at `REPEATABLE READ`, so every read of the
/// source -- its row, its task runs, its render surface -- sees one snapshot.
/// `SKIP LOCKED` partitions the outbox between replicas.
///
/// **"At most once per pass" is a cursor** (final review I3): the claim takes
/// the oldest due event with `id > cursor`, and the cursor moves to each event
/// claimed. So neither a kept event nor one whose transaction failed can take
/// the pass again, and the claim binds one integer instead of the growing list
/// of every id seen so far (that list made a pass quadratic in its length). An
/// event that commits with a LOWER id than the cursor after the cursor passed
/// it -- ids are handed out before commit, so commits can land out of order --
/// is simply the next pass's. Events written DURING the pass (a fired
/// execution's first settle) have higher ids and are claimed in it, up to the
/// cap.
///
/// `cancel` is checked before each claim, so a shutdown waits for at most the
/// event in hand.
///
/// An event whose transaction fails is rolled back -- every fire it made
/// and its own update -- counted as one `errored`, logged at `warn`, and its
/// attempt counted apart (F1). `Err` is returned only when a transaction
/// cannot even begin or the claim itself fails.
///
/// # A miss is not a failed attempt (W9c Task 9)
///
/// A fire's first settle renders through the event transaction's cache
/// (`storage::prefetch`). A render that needs a read rolls the WHOLE event
/// transaction back -- the event and window rows' locks with it -- in
/// `worker::txn::transaction!`, which fetches the read with nothing held and
/// claims again from the same cursor, so the same event (or, if another
/// replica took it meanwhile, the next). A listener whose render missed
/// renders a placeholder and the attempt goes on to every other listener
/// (the user's ruling of 2026-10-08, "gather"), so one fetch answers the
/// whole event; whatever the rolled-back attempt computed -- fates, counts,
/// a failed start -- is discarded with it: it is never counted (`errored`,
/// `attempts`), and the cursor moves only past an event whose attempt
/// finished.
pub async fn process_events_with(
    pool: &PgPool,
    limits: &PassLimits,
    cancel: &CancellationToken,
) -> Result<EventReport> {
    let started = Instant::now();
    let mut report = EventReport::default();
    let mut cursor: i64 = 0;
    loop {
        if cancel.is_cancelled() {
            return Ok(report);
        }
        // The event the latest attempt claimed: what a failed transaction
        // is counted against.
        let mut claimed_ev: Option<Event> = None;
        let ended = transaction!(
            pool,
            Isolation::RepeatableRead,
            |at| { claim_and_process(at, cursor, limits, &mut claimed_ev).await },
            |pf| {
                pf.fetch_pending().await?;
                Ok(Fetched::Again)
            }
        );
        match (ended, claimed_ev) {
            (Ok(Ended::Done(Step::Empty)), _) => return Ok(report),
            (Ok(Ended::Done(Step::Again)), _) => continue,
            (Ok(Ended::Done(Step::Event(ev, delta))), _) => {
                cursor = ev.id;
                report.absorb(delta);
            }
            (Ok(Ended::GaveUp), _) => {
                return Err(Error::Internal(anyhow::anyhow!(
                    "a flow-trigger event's plain fetch cannot give up"
                )))
            }
            // A failure before any event was claimed: the begin or the claim.
            (Err(e), None) => return Err(e),
            // A failure of the event's TRANSACTION -- a savepoint, the final
            // write, the COMMIT -- rolls back every fire it made and the
            // event's own write with it. It does not end the pass: the cursor
            // passes the event, so the pass moves on to newer events. The
            // attempt is then counted OUTSIDE that transaction (final review
            // F1), so an event whose transaction always fails is given up like
            // a listener that always fails, instead of being retried for ever.
            (Err(e), Some(ev)) => {
                cursor = ev.id;
                tracing::warn!(
                    event_id = ev.id,
                    source_execution_id = %ev.source_execution_id,
                    error = %e,
                    "a flow-trigger event's transaction failed and rolled back; left for the next pass"
                );
                report.errored += 1;
                count_a_failed_transaction(pool, &ev, limits.retry_backoff).await;
            }
        }
        report.events += 1;
        if report.events >= limits.max_events || started.elapsed() >= limits.max_duration {
            return Ok(report);
        }
    }
}

/// What one attempt of [`process_events_with`]'s transaction came to.
enum Step {
    /// Nothing is due.
    Empty,
    /// The claim met a serialization failure: claim again.
    Again,
    /// This event was processed; what it adds to the pass's report.
    Event(Event, EventReport),
}

/// One attempt of [`process_events_with`]'s transaction: claim the oldest due
/// event past `cursor`, process it, and finish it. `claimed` is set to the
/// event as soon as it is claimed.
async fn claim_and_process(
    at: &mut Txn,
    cursor: i64,
    limits: &PassLimits,
    claimed: &mut Option<Event>,
) -> Result<Finish<Step>> {
    *claimed = None;
    {
        let claimed_row: std::result::Result<Option<Event>, sqlx::Error> = sqlx::query_as(
            "SELECT id, source_execution_id, workspace_id, state, done, attempts \
               FROM workflow.flow_trigger_events \
              WHERE id > $1 \
                AND next_attempt_at <= now() \
              ORDER BY id \
              LIMIT 1 \
              FOR UPDATE SKIP LOCKED",
        )
        .bind(cursor)
        .fetch_optional(at.conn())
        .await;
        let ev = match claimed_row {
            Ok(Some(ev)) => ev,
            Ok(None) => return Ok(Finish::Rollback(Step::Empty)),
            // The one serialization failure this transaction can meet (see
            // the module doc, statement 2): another processor deleted or
            // updated the row our snapshot saw, after the snapshot and before
            // our lock. Nothing was done; a fresh snapshot no longer sees it.
            Err(e) if is_serialization_failure(&e) => return Ok(Finish::Rollback(Step::Again)),
            Err(e) => return Err(db_err(e)),
        };
        // The cache is this event's: a retry that claims another one (this
        // one was taken by another replica meanwhile) starts it over.
        at.gate().prefetch().bind(&ev.id.to_string());
        *claimed = Some(ev.clone());
        // What this event adds is kept only if its transaction commits.
        let mut delta = EventReport::default();
        let (done, pending) = process_one(at, &ev, &mut delta).await?;
        finish_event(at.conn(), &ev, done, pending, limits.retry_backoff).await?;
        Ok(Finish::Commit(Step::Event(ev, delta)))
    }
}

/// The failure path's own `attempts + 1` (final review F1): one autocommit
/// statement, after the event's transaction rolled back. **Best effort** -- a
/// failure here is logged and the event is simply claimed again, as before.
///
/// It takes the row `FOR UPDATE SKIP LOCKED` so it never waits on another
/// replica that has claimed the event since: that replica's own pass counts
/// its own attempt. The event is due again one `backoff` later (F2), and at
/// [`MAX_ATTEMPTS`] it is deleted with the same `warn` as a listener given up.
async fn count_a_failed_transaction(pool: &PgPool, ev: &Event, backoff: Duration) {
    let counted: std::result::Result<Option<i32>, sqlx::Error> = sqlx::query_scalar(
        "UPDATE workflow.flow_trigger_events \
            SET attempts = attempts + 1, \
                next_attempt_at = now() + $2::bigint * interval '1 millisecond' \
          WHERE id = (SELECT id FROM workflow.flow_trigger_events \
                       WHERE id = $1 FOR UPDATE SKIP LOCKED) \
         RETURNING attempts",
    )
    .bind(ev.id)
    .bind(backoff_ms(backoff))
    .fetch_optional(pool)
    .await;
    let attempts = match counted {
        Ok(Some(n)) => n,
        // Gone, or held by another pass: nothing to count here.
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(
                event_id = ev.id,
                error = %e,
                "could not count a failed flow-trigger event's attempt; it is claimed again as it is"
            );
            return;
        }
    };
    if attempts < MAX_ATTEMPTS {
        return;
    }
    let deleted = sqlx::query(
        "DELETE FROM workflow.flow_trigger_events \
          WHERE id = (SELECT id FROM workflow.flow_trigger_events \
                       WHERE id = $1 FOR UPDATE SKIP LOCKED)",
    )
    .bind(ev.id)
    .execute(pool)
    .await;
    match deleted {
        Ok(r) if r.rows_affected() == 1 => tracing::warn!(
            event_id = ev.id,
            source_execution_id = %ev.source_execution_id,
            state = %ev.state,
            attempts,
            "gave up on a flow-trigger event after {MAX_ATTEMPTS} attempts; its transaction \
             failed every time, so none of the listeners it still owed fired"
        ),
        Ok(_) => {}
        Err(e) => tracing::warn!(
            event_id = ev.id,
            error = %e,
            "could not give up a failed flow-trigger event; it is claimed again"
        ),
    }
}

fn backoff_ms(backoff: Duration) -> i64 {
    i64::try_from(backoff.as_millis()).unwrap_or(i64::MAX)
}

fn is_serialization_failure(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| c == "40001")
}

/// The listener keys a later pass must retry, by why.
#[derive(Default)]
struct Pending {
    /// Possibly transient failures ([`Fate::Retry`]): each pass that leaves
    /// one spends an attempt.
    failed: Vec<String>,
    /// Lost window races ([`Fate::Contended`]): retried, free.
    contended: Vec<String>,
}

/// Everything that happens to one claimed event before its row is deleted or
/// kept. Returns `(done, pending)`: the listener keys the event is finished
/// with, including the ones an earlier pass finished, and the ones a later
/// pass must retry.
async fn process_one(
    at: &mut Txn,
    ev: &Event,
    report: &mut EventReport,
) -> Result<(Vec<String>, Pending)> {
    let mut done = ev.done.clone();
    let mut pending = Pending::default();

    let Ok(state) = serde_json::from_value::<State>(Value::String(ev.state.clone())) else {
        // `settle` writes `RunState::wire()`, pinned to `State`'s serde
        // spelling by `run_state_wire_is_the_model_states_serde_spelling`, so
        // this is a corrupt row. Nothing can ever hear it: drop it, loudly.
        tracing::warn!(
            event_id = ev.id,
            state = %ev.state,
            "a flow-trigger event names a state no listener can hear; dropped"
        );
        report.errored += 1;
        return Ok((done, pending));
    };

    // The listeners that are ON: same workspace, the event's state in their
    // `states`, and the six off switches (spec §6). No lock: see statement 3.
    let listeners: Vec<Listener> = sqlx::query_as(concat!(
        "SELECT l.flow_id, l.trigger_id, f.workspace_id, f.source, \
                f.namespace, f.flow_id AS flow_key \
           FROM workflow.flow_listeners l JOIN workflow.flows f ON f.id = l.flow_id \
          WHERE l.workspace_id = $1 \
            AND $2 = ANY (l.states) \
            AND NOT l.disabled \
            AND",
        flow_is_on!("$3", "$4"),
        "ORDER BY f.namespace, f.flow_id, l.trigger_id",
    ))
    .bind(ev.workspace_id)
    .bind(&ev.state)
    .bind(WORKFLOWS_FEATURE)
    .bind(workflows_feature_default())
    .fetch_all(at.conn())
    .await
    .map_err(db_err)?;
    let todo: Vec<&Listener> = listeners
        .iter()
        .filter(|l| !done.contains(&l.key()))
        .collect();
    if todo.is_empty() {
        return Ok((done, pending));
    }

    // The source and its render context, in a savepoint of their own: a
    // failure here must leave the transaction able to keep the event.
    let loaded = {
        let mut sp = at.conn().begin().await.map_err(db_err)?;
        match load_source(&mut sp, ev, state).await {
            Ok(v) => {
                sp.commit().await.map_err(db_err)?;
                Ok(v)
            }
            Err(e) => {
                sp.rollback().await.map_err(db_err)?;
                Err(e)
            }
        }
    };
    let (source, ctx) = match loaded {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                event_id = ev.id,
                source_execution_id = %ev.source_execution_id,
                error = %e,
                "the source of a flow-trigger event could not be read; kept for every listener"
            );
            report.errored += todo.len();
            pending.failed.extend(todo.iter().map(|l| l.key()));
            return Ok((done, pending));
        }
    };

    for l in todo {
        let key = l.key();
        match listen(at, ev, l, &source, &ctx).await? {
            Fate::Fired => {
                report.fired += 1;
                done.push(key);
            }
            Fate::FiredFailed(why) => {
                report.born_failed += 1;
                tracing::warn!(
                    event_id = ev.id,
                    listener = %key,
                    reason = %why,
                    "a flow-trigger listener's inputs did not render; its execution was created FAILED, as Kestra does"
                );
                done.push(key);
            }
            Fate::Refused(why) => {
                report.refused += 1;
                tracing::info!(
                    event_id = ev.id,
                    listener = %key,
                    reason = %why,
                    "a flow-trigger listener did not fire"
                );
                done.push(key);
            }
            Fate::AlreadyFired(why) => {
                report.refused += 1;
                tracing::warn!(
                    event_id = ev.id,
                    listener = %key,
                    reason = %why,
                    "a flow-trigger event that already fired this listener was claimed again; consumed"
                );
                done.push(key);
            }
            Fate::Broken(why) => {
                report.errored += 1;
                tracing::warn!(
                    event_id = ev.id,
                    listener = %key,
                    reason = %why,
                    "a flow-trigger listener cannot fire until its flow is saved again \
                     or this deployment serves its task types (see reason)"
                );
                done.push(key);
            }
            Fate::Accumulated => {
                report.accumulated += 1;
                tracing::debug!(
                    event_id = ev.id,
                    listener = %key,
                    "a flow-trigger event met a dependsOn listener's dependency; stored in its window"
                );
                done.push(key);
            }
            Fate::Retry(why) => {
                report.errored += 1;
                tracing::warn!(
                    event_id = ev.id,
                    listener = %key,
                    reason = %why,
                    "a flow-trigger listener failed; the event is kept for it"
                );
                pending.failed.push(key);
            }
            Fate::Contended(why) => {
                report.contended += 1;
                tracing::debug!(
                    event_id = ev.id,
                    listener = %key,
                    reason = %why,
                    "a flow-trigger listener lost a race for its window; the event is kept for it"
                );
                pending.contended.push(key);
            }
        }
    }
    Ok((done, pending))
}

/// The event's last statement (statement 5), under the claim's row lock, in
/// the claim's transaction.
///
/// - **Failed listeners spend attempts.** A pass that leaves one counts
///   `attempts + 1` and sets the event one `backoff` ahead; the pass that
///   would reach [`MAX_ATTEMPTS`] gives those listeners up -- logged at
///   `warn`, recorded as done -- instead of keeping them.
/// - **Contended listeners do not** (W13b Task 4 fix round 1). A `dependsOn`
///   listener that lost a race for its window row is kept, the event due at
///   once (`next_attempt_at = now()`, the pass's own start, so the NEXT pass
///   claims it: this pass's cursor is already past it), and `attempts` is
///   untouched. Losing is not a fault: the `40001` exists only because a
///   competitor committed a change to that row, so every free retry is paired
///   with another transaction's progress, and the next pass reads the
///   winner's result (spec §5.3 *Concurrency*). Before this fix, five lost
///   races gave the event up and the listener's dependency result was lost.
///   A pass still claims an event at most once, so this cannot spin a pass
///   or hold `fire_due` back (W13a's cursor and caps).
/// - Nothing left: the event is deleted.
async fn finish_event(
    tx: &mut PgConnection,
    ev: &Event,
    mut done: Vec<String>,
    pending: Pending,
    backoff: Duration,
) -> Result<()> {
    let Pending { failed, contended } = pending;
    let failing = !failed.is_empty();
    let mut retry_failed = failing;
    if failing && ev.attempts + 1 >= MAX_ATTEMPTS {
        tracing::warn!(
            event_id = ev.id,
            source_execution_id = %ev.source_execution_id,
            state = %ev.state,
            attempts = ev.attempts + 1,
            listeners = ?failed,
            "gave up on a flow-trigger event after {MAX_ATTEMPTS} attempts; these listeners never fired"
        );
        done.extend(failed);
        retry_failed = false;
    }
    if !retry_failed && contended.is_empty() {
        sqlx::query("DELETE FROM workflow.flow_trigger_events WHERE id = $1")
            .bind(ev.id)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
        return Ok(());
    }
    let delay = if retry_failed { backoff_ms(backoff) } else { 0 };
    sqlx::query(
        "UPDATE workflow.flow_trigger_events \
            SET done = $2, attempts = attempts + $4, \
                next_attempt_at = now() + $3::bigint * interval '1 millisecond' \
          WHERE id = $1",
    )
    .bind(ev.id)
    .bind(&done)
    .bind(delay)
    .bind(i32::from(failing))
    .execute(&mut *tx)
    .await
    .map_err(db_err)?;
    Ok(())
}

/// The source execution in the claim's snapshot, and the context its
/// listeners' `when` and `inputs` render against.
async fn load_source(
    conn: &mut PgConnection,
    ev: &Event,
    state: State,
) -> Result<(Source, RenderContext)> {
    let row: SourceRow = sqlx::query_as(
        "SELECT e.workspace_id, e.namespace, e.flow_key, e.execution_depth, e.created_at, e.updated_at, \
                e.source, e.loop_index, e.labels, \
                (SELECT r.task_id FROM workflow.task_runs r \
                  WHERE r.execution_id = e.id ORDER BY r.run_ix DESC LIMIT 1) AS last_task_id \
           FROM workflow.executions e WHERE e.id = $1",
    )
    .bind(ev.source_execution_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    let ctx = execution_render_context(conn, ev.source_execution_id).await?;
    // The EXECUTION's labels: its flow's, then its own (W7f spec §3.3).
    let flow_labels = crate::spec::parse(&row.source)
        .map(|f| f.labels)
        .unwrap_or_default();
    let labels = crate::labels::execution_labels(
        &flow_labels,
        &crate::labels::own_labels(row.labels.as_ref()),
    );
    let execution = SourceExecution {
        id: ev.source_execution_id,
        // The EXECUTION's workspace, from its row (final review M-a). The
        // event's copy is what the listener `SELECT` narrows by, and `settle`
        // writes the two equal; reading the row means a disagreeing event is
        // refused `DifferentWorkspace` instead of reaching the fire, where
        // 0138's `(trigger_execution_id, workspace_id)` key would fail it and
        // the event would be retried until given up.
        workspace_id: row.workspace_id,
        namespace: row.namespace.clone(),
        flow_id: row.flow_key.clone(),
        state,
        // egret has no `kind` column, but one kind IS identifiable: a `Loop`
        // iteration, whose row carries a `loop_index`. Kestra builds an
        // iteration with `ExecutionKind.LOOP` (`Execution.loopExecution`,
        // javap) and `computeFlowTriggers` answers an empty list for a kind
        // that is not normal (offset 12), so an iteration is heard and
        // refused `NotNormalKind` (W13a final review I1). Every other egret
        // execution is normal, which is what `isNormal` answers for a null
        // kind; `Test` and `Playground` have no egret row.
        kind: row.loop_index.map(|_| ExecutionKind::Loop),
        execution_depth: Some(u32::try_from(row.execution_depth).unwrap_or(0)),
    };
    let record = SourceRecord {
        execution_id: ev.source_execution_id.to_string(),
        namespace: row.namespace,
        flow_id: row.flow_key,
        state: ev.state.clone(),
        start_date: row.created_at,
        end_date: state.is_terminated().then_some(row.updated_at),
        last_task_id: row.last_task_id,
        labels,
    };
    Ok((Source { execution, record }, ctx))
}

/// One listener on one event. `Err` is a failure of the event's transaction
/// itself; everything the listener can get wrong is a [`Fate`].
async fn listen(
    at: &mut Txn,
    ev: &Event,
    l: &Listener,
    source: &Source,
    ctx: &RenderContext,
) -> Result<Fate> {
    let flow = match crate::spec::parse(&l.source) {
        Ok(f) => f,
        Err(e) => {
            return Ok(Fate::Broken(format!(
                "the listener flow does not parse: {}",
                e.message
            )))
        }
    };
    let Some((trigger, spec)) = flow.triggers.iter().find_map(|t| match &t.kind {
        TriggerKind::Flow(spec) if t.id == l.trigger_id => Some((t, spec)),
        _ => None,
    }) else {
        return Ok(Fate::Broken(
            "the listener row names a Flow trigger its flow does not declare".to_string(),
        ));
    };
    // The arm is read off the YAML the listener is evaluated from, not off
    // the row's `depends_on` copy: the two are written together by `sync`,
    // and the YAML is what the rest of this function obeys.
    if !spec.depends_on.is_empty() {
        return listen_depends_on(at, ev, l, &flow, trigger, spec, source, ctx).await;
    }
    let listener = match WindowFreeFlowTrigger::resolve(trigger) {
        Ok(r) => r,
        Err(e) => return Ok(Fate::Broken(e.to_string())),
    };
    let fired = match filter(&listener, l, &flow, source, ctx) {
        Ok(f) => f,
        Err(why) => return Ok(Fate::Refused(why)),
    };

    // The listener's savepoint: whatever `start_execution_in` does -- its
    // insert, the first settle, an aborted statement -- rolls back alone, so a
    // sibling's fire in this transaction survives it (spec §4.4 point 1).
    let mut sp = at.savepoint().await?;
    let started = start_fire(&mut sp, ev, l, source, ctx, &fired).await?;
    Ok(match started {
        Ok(render_failure) => {
            sp.release().await?;
            fired_fate(render_failure)
        }
        Err(err) => {
            sp.rollback().await?;
            failed_start_fate(&err)
        }
    })
}

/// `computeFlowTriggers`' filter, the same on both arms: `when` rendered
/// against the SOURCE's context (spec §4.4; `trigger::flow` point 3), then
/// [`evaluate`]. `Err` is the refusal, named.
fn filter(
    listener: &WindowFreeFlowTrigger,
    l: &Listener,
    flow: &Flow,
    source: &Source,
    ctx: &RenderContext,
) -> std::result::Result<FiredExecution, String> {
    let when = render_when(listener.when_source(), ctx);
    let listening = ListeningFlow {
        workspace_id: l.workspace_id,
        namespace: l.namespace.clone(),
        flow_id: l.flow_key.clone(),
        disabled: flow.disabled,
        // **A draft listener fires** (the user's decision of 2026-09-25, spec
        // header): cron, webhooks and API start already run the draft, so
        // egret never reports a flow as Kestra's `isDraft`.
        draft: false,
    };
    match evaluate(
        listener,
        &listening,
        &source.execution,
        &when,
        DEFAULT_MAX_EXECUTION_DEPTH,
    ) {
        Outcome::Refused(r) => Err(format!("{r:?}")),
        Outcome::Fired(f) => Ok(*f),
    }
}

/// A `when` -- the trigger's or a dependency's -- rendered against the
/// source's context. A render error is `Unrenderable`, which both
/// `evaluate` and `dependency_matches` refuse.
fn render_when(source: Option<&str>, ctx: &RenderContext) -> WhenVerdict {
    match source {
        None => WhenVerdict::NotDeclared,
        Some(w) => match render_str_guarded(w, ctx, FORBIDDEN_ROOTS) {
            Ok(GuardedRender::Rendered(s)) => WhenVerdict::Rendered(s),
            Ok(GuardedRender::Forbidden(_)) | Err(_) => WhenVerdict::Unrenderable,
        },
    }
}

fn fired_fate(render_failure: Option<String>) -> Fate {
    match render_failure {
        None => Fate::Fired,
        Some(why) => Fate::FiredFailed(why),
    }
}

fn failed_start_fate(err: &Error) -> Fate {
    match classify(err) {
        Started::Yes => Fate::Fired,
        Started::Refused(why) => Fate::Broken(why),
        Started::AlreadyFired(why) => Fate::AlreadyFired(why),
        Started::Failed(why) => Fate::Retry(why),
    }
}

/// Render the fire's `inputs` and start the listener's execution on `conn`,
/// which the caller has made a savepoint. `Ok(Ok(render_failure))` started
/// it (born FAILED when `render_failure` is `Some`); `Ok(Err(e))` is
/// `start_execution_in`'s failure, for the caller to roll its savepoint back
/// and [`failed_start_fate`] it.
///
/// W9c Task 9: `sp` is a savepoint of the event's attempt, so the first
/// settle renders under `process_events_with`'s gate; a start whose render
/// missed is classified like any other, and the gate rolls the whole event
/// back, its fate and counts with it.
async fn start_fire(
    sp: &mut Savepoint<'_>,
    ev: &Event,
    l: &Listener,
    source: &Source,
    ctx: &RenderContext,
    fired: &FiredExecution,
) -> Result<std::result::Result<Option<String>, Error>> {
    // `inputs` render against the same context, by the same
    // `runContext.render(Map)` the webhook uses. **A failure is NOT a
    // refusal**, unlike an unrenderable `when`: Kestra, measured, still creates
    // the listener's execution, `CREATED -> FAILED` with no task run and no
    // `inputs` (W13a Task 1, `listener_run1_FAILED_bad_inputs`). So the fire
    // goes ahead born FAILED with `inputs = {}` -- the column is `NOT NULL
    // DEFAULT '{}'` -- and the render error, which egret has no
    // execution-level column to keep, is logged.
    let declared = Value::Object(fired.inputs.clone().into_iter().collect());
    let (inputs, render_failure) = match render_value(&declared, ctx, "") {
        Ok(v) => (v, None),
        Err(InputRefusal::NeedsTriggerContext { key }) => (
            Value::Object(Map::new()),
            Some(format!(
                "inputs.{key} reads `trigger`, which a Flow trigger's render context does not bind"
            )),
        ),
        Err(InputRefusal::Unrenderable { key, error }) => (
            Value::Object(Map::new()),
            Some(format!("inputs.{key} does not render: {error}")),
        ),
    };
    let started = start_execution_in_with(
        sp,
        NewExecution {
            workspace_id: l.workspace_id,
            // Always the listener's uuid: the backstop index keys on it, and
            // NULLs are distinct, so `None` would let a second fire through.
            flow_id: Some(l.flow_id),
            source: &l.source,
            inputs,
            trigger: Some(flow_trigger_record(fired, &source.record)),
            scheduled_date: None,
            trigger_execution_id: Some(fired.trigger_execution_id),
            execution_depth: i32::try_from(fired.execution_depth)
                .map_err(|e| Error::Internal(e.into()))?,
            flow_trigger_event_id: Some(ev.id),
            born_failed: render_failure.is_some(),
            replayed_from_execution_id: None,
            id: None,
            input_files: Vec::new(),
            labels: None,
            backfill_id: None,
            variables: None,
        },
    )
    .await;
    Ok(started.map(|_| render_failure))
}

/// A `dependsOn` listener on one event (W13b, spec §5.3, user decision D1).
/// In order:
///
/// 1. **The run-time gate**: [`depends_on_refusals`] (the whole list),
///    [`TimeWindow::from_model`] and [`condition_keys`]. Anything refused is
///    skipped with [`DEPENDS_ON_REFUSED`] and never touches a window.
/// 2. **The filter**, [`filter`] -- the same as the window-free arm's, the
///    trigger's own `when` included (`computeFlowTriggers` runs before the
///    split). A refused event touches no window.
/// 3. **The dependencies**, each `when` rendered on the SOURCE. None met:
///    [`NO_DEPENDENCY_MET`], and no window is opened or replaced (D1).
/// 4. **The window**, in a savepoint of its own ([`window_step`]), so a
///    failure anywhere in it -- the row lock's `40001`, the fire -- rolls back
///    the window's changes with it and the event is retried for this
///    listener.
#[allow(clippy::too_many_arguments)]
async fn listen_depends_on(
    at: &mut Txn,
    ev: &Event,
    l: &Listener,
    flow: &Flow,
    trigger: &Trigger,
    spec: &FlowTrigger,
    source: &Source,
    ctx: &RenderContext,
) -> Result<Fate> {
    let refusals = depends_on_refusals(spec);
    if !refusals.is_empty() {
        let each: Vec<String> = refusals
            .iter()
            .map(|r| format!("[{}] {}", r.code, r.message))
            .collect();
        return Ok(Fate::Refused(format!(
            "{DEPENDS_ON_REFUSED}: {}",
            each.join("; ")
        )));
    }
    let window = match TimeWindow::from_model(spec.window.as_ref()) {
        Ok(w) => w,
        Err(e) => return Ok(Fate::Refused(format!("{DEPENDS_ON_REFUSED}: {e}"))),
    };
    let keys = match condition_keys(&spec.depends_on) {
        Ok(k) => k,
        Err(e) => return Ok(Fate::Refused(format!("{DEPENDS_ON_REFUSED}: {e}"))),
    };
    let Some(listener) = WindowFreeFlowTrigger::depends_on_filter(trigger) else {
        return Ok(Fate::Broken(
            "the dependsOn trigger has no listener filter".to_string(),
        ));
    };
    let fired = match filter(&listener, l, flow, source, ctx) {
        Ok(f) => f,
        Err(why) => return Ok(Fate::Refused(why)),
    };

    // `Flow.DependencyCondition.test` per dependency. **Its `when` renders on
    // the SOURCE** (`runContextFactory.of(null, execution)`), like the
    // trigger's. `models::Dependency` fills an absent `when` with `"true"`, so
    // this renders it and passes `Rendered`; only an explicit `when: null` is
    // `NotDeclared` (which `dependency_matches` accepts for exactly that).
    // Labels are the source EXECUTION's: its flow's declared ones, then its
    // own (a backfill's, a replay's copy; W7f spec §3.3), as Kestra matches.
    // An execution without own labels still has none beyond its flow's, and a
    // label that exists on neither never matches (fails closed).
    let dep_source = DepSource {
        namespace: &source.record.namespace,
        flow_id: &source.record.flow_id,
        state: source.execution.state,
        labels: &source.record.labels,
    };
    let met: BTreeMap<String, bool> = spec
        .depends_on
        .iter()
        .zip(&keys)
        .filter(|(dep, _)| {
            let when = render_when(dep.when.as_ref().map(|w| w.source()), ctx);
            dependency_matches(dep, &dep_source, &when)
        })
        .map(|(_, key)| (key.clone(), true))
        .collect();
    if met.is_empty() {
        return Ok(Fate::Refused(NO_DEPENDENCY_MET.to_string()));
    }

    let mut sp = at.savepoint().await?;
    let step = window_step(
        &mut sp, ev, l, spec, &window, &keys, &met, source, ctx, &fired,
    )
    .await;
    Ok(match step {
        Ok(WindowStep::Done(fate)) => {
            sp.release().await?;
            fate
        }
        Ok(WindowStep::StartFailed(err)) => {
            sp.rollback().await?;
            failed_start_fate(&err)
        }
        Err(StepError::Contended(why)) => {
            sp.rollback().await?;
            Fate::Contended(why)
        }
        Err(StepError::Failed(e)) => {
            sp.rollback().await?;
            Fate::Retry(format!("the listener's window: {e}"))
        }
    })
}

/// What [`window_step`] did.
enum WindowStep {
    /// Commit the savepoint: this is the listener's fate.
    Done(Fate),
    /// The fire's `start_execution_in` failed: roll everything back.
    StartFailed(Error),
}

/// Why [`window_step`] failed.
enum StepError {
    /// A serialization failure (`40001`) or a deadlock (`40P01`) on the
    /// window: another transaction committed a change to the row after this
    /// one's snapshot (or won the deadlock). See [`finish_event`].
    Contended(String),
    /// Anything else: possibly transient, retried against `attempts`.
    Failed(Error),
}

impl From<Error> for StepError {
    fn from(e: Error) -> Self {
        StepError::Failed(e)
    }
}

/// A window statement's error, classified.
///
/// **`40P01` is not reachable among the window statements** (derived, not
/// staged): processors lock window rows in the listener `SELECT`'s order
/// (`ORDER BY namespace, flow_id, trigger_id`), one row per listener, and the
/// sweep takes its rows `SKIP LOCKED`, so it never waits and closes no cycle.
/// It is classified as contention anyway -- a deadlock victim's competitor
/// committed too -- so that a cycle closed through some other lock by a
/// WINDOW statement cannot spend attempts either.
///
/// **This does not cover the fire.** A fire's errors go through
/// `start_fire(..)?`, `StartFailed` and [`failed_start_fate`], never through
/// here, so a deadlock in the fire (for example a flow delete cascading into
/// the window row while the fire's foreign-key check takes `KEY SHARE` on
/// `workflow.flows`) is a failed start and spends an attempt. That is safe --
/// the savepoint rolls the window back with it and the event is retried --
/// but it is an attempt, not free contention. (Final review, RR-2: an
/// earlier version of this comment said the opposite.)
fn step_err(e: sqlx::Error) -> StepError {
    let code = e.as_database_error().and_then(|d| d.code());
    match code.as_deref() {
        Some("40001") | Some("40P01") => {
            StepError::Contended(format!("the listener's window: {e}"))
        }
        _ => StepError::Failed(db_err(e)),
    }
}

/// Spec §5.3 steps 2-5 on the listener's window row, inside the listener's
/// savepoint. Every statement is one of the module doc's statements 6-9.
#[allow(clippy::too_many_arguments)]
async fn window_step(
    sp: &mut Savepoint<'_>,
    ev: &Event,
    l: &Listener,
    spec: &FlowTrigger,
    window: &ResolvedWindow,
    keys: &[String],
    met: &BTreeMap<String, bool>,
    source: &Source,
    ctx: &RenderContext,
    fired: &FiredExecution,
) -> std::result::Result<WindowStep, StepError> {
    // The window's clock is the event transaction's `now()`, never the wall
    // clock (W13b global constraint). At `REPEATABLE READ` it is the
    // transaction's start, the same instant for every listener of the event.
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(sp.conn())
        .await
        .map_err(step_err)?;
    let (start, end) = match window.bounds(now) {
        Ok(b) => b,
        Err(e) => {
            return Ok(WindowStep::Done(Fate::Refused(format!(
                "the listener's window has no bounds at {now}: {e}"
            ))))
        }
    };

    // Statement 6, insert-if-absent. A missing row cannot be created twice.
    sqlx::query(
        "INSERT INTO workflow.flow_trigger_windows \
           (flow_id, trigger_id, window_start, window_end, results) \
         VALUES ($1, $2, $3, $4, '{}'::jsonb) \
         ON CONFLICT (flow_id, trigger_id) DO NOTHING",
    )
    .bind(l.flow_id)
    .bind(&l.trigger_id)
    .bind(start)
    .bind(end)
    .execute(sp.conn())
    .await
    .map_err(step_err)?;

    // Statement 7, the row lock: what makes "fire once" hold.
    let (mut w_start, mut w_end, Json(mut results)): (
        DateTime<Utc>,
        DateTime<Utc>,
        Json<BTreeMap<String, bool>>,
    ) = sqlx::query_as(
        "SELECT window_start, window_end, results FROM workflow.flow_trigger_windows \
          WHERE flow_id = $1 AND trigger_id = $2 \
            FOR UPDATE",
    )
    .bind(l.flow_id)
    .bind(&l.trigger_id)
    .fetch_one(sp.conn())
    .await
    .map_err(step_err)?;

    // D1: an expired window is REPLACED -- fresh bounds, empty results -- and
    // the event is then evaluated against the fresh one (Kestra, measured,
    // ignores the event instead; spec §5.3 "Kestra, as measured").
    if w_end <= now {
        sqlx::query(
            "UPDATE workflow.flow_trigger_windows \
                SET window_start = $3, window_end = $4, results = '{}'::jsonb, outputs = NULL \
              WHERE flow_id = $1 AND trigger_id = $2",
        )
        .bind(l.flow_id)
        .bind(&l.trigger_id)
        .bind(start)
        .bind(end)
        .execute(sp.conn())
        .await
        .map_err(step_err)?;
        (w_start, w_end) = (start, end);
        results.clear();
    }

    // Step 3: outside the window (a daily window's off-hours, a future
    // `offset` start, a passed deadline), the event is ignored and not kept.
    if !TimeWindow::is_valid(w_start, w_end, now) {
        return Ok(WindowStep::Done(Fate::Refused(format!(
            "the event at {now} is outside the listener's window ({w_start} .. {w_end})"
        ))));
    }

    let merged = merge(&results, met);
    if !satisfied(spec.mode, spec.min_satisfied, keys, &merged) {
        // Statement 8, the update.
        sqlx::query(
            "UPDATE workflow.flow_trigger_windows SET results = $3 \
              WHERE flow_id = $1 AND trigger_id = $2",
        )
        .bind(l.flow_id)
        .bind(&l.trigger_id)
        .bind(Json(&merged))
        .execute(sp.conn())
        .await
        .map_err(step_err)?;
        return Ok(WindowStep::Done(Fate::Accumulated));
    }

    // Satisfied: fire through the window-free arm's path, then delete the
    // window (statement 9) under every mode -- no `resetOnSuccess` exists
    // (D1; measured for ALL, ANY and AT_LEAST). The merged results are not
    // written first: the row is deleted in this savepoint either way.
    match start_fire(sp, ev, l, source, ctx, fired).await? {
        Ok(render_failure) => {
            sqlx::query(
                "DELETE FROM workflow.flow_trigger_windows WHERE flow_id = $1 AND trigger_id = $2",
            )
            .bind(l.flow_id)
            .bind(&l.trigger_id)
            .execute(sp.conn())
            .await
            .map_err(step_err)?;
            Ok(WindowStep::Done(fired_fate(render_failure)))
        }
        Err(e) => Ok(WindowStep::StartFailed(e)),
    }
}

/// `execution.trigger` for a Flow-fired execution, in the shape W13a Task 1
/// measured on the pinned artifact (`oracle-exec/w13-flow-trigger.json`):
/// `{id, type, variables}` with exactly nine `variables` keys. Each key, and
/// where egret's value comes from:
///
/// | key | egret's value | basis |
/// |---|---|---|
/// | `executionId` | the source's id | measured key; egret ids are uuids, Kestra's are short ids |
/// | `namespace` | the source's namespace | measured |
/// | `flowId` | the source's flow key | measured |
/// | `state` | the EVENT's state, the bare enum | measured (`"SUCCESS"`) |
/// | `startDate` | the source's `created_at` | measured: equals the source's `CREATED` history date (hop 2) |
/// | `endDate` | the source's `updated_at`, **only** when the event's state is terminal | measured for SUCCESS: equals the terminal history date. For a non-terminal state (`PAUSED`) it is **omitted** -- derived from `State.getEndDate()` being empty until termination, not measured |
/// | `lastTaskId` | the source's highest-`run_ix` task run; omitted when it has none | measured on one-task flows only |
/// | `executionLabels` | the source EXECUTION's labels (its flow's, then its own), dotted keys nested, `system.correlationId` excluded | measured: `system.from` nests under `system`, and `correlationId` is absent. The source EXECUTION's labels: its flow's declared ones (which Kestra copies onto every execution), then the execution's own (W7f); egret sets no `system.*` label, so with none of either it is `{}`, which is what hop 2 measured |
/// | `flowRevision` | **omitted** | egret records no revision on an execution. `flow_versions` exists, but nothing says which version an execution ran, and a guessed number would be an invented value |
///
/// Dates carry six fractional digits and `Z`, the measured shape. A date whose
/// microseconds end in zeros was not measured.
///
/// `endDate` and `lastTaskId` are read when the event is PROCESSED, not when
/// the transition happened (spec §4.4 point 3). For a terminal source they
/// agree unless the source was replayed or restarted before its event was
/// processed; then they describe the later run.
pub(crate) fn flow_trigger_record(fired: &FiredExecution, source: &SourceRecord) -> Value {
    let fmt = |at: DateTime<Utc>| at.to_rfc3339_opts(SecondsFormat::Micros, true);
    let mut variables = Map::new();
    variables.insert("state".into(), Value::String(source.state.clone()));
    variables.insert("flowId".into(), Value::String(source.flow_id.clone()));
    variables.insert("namespace".into(), Value::String(source.namespace.clone()));
    variables.insert(
        "executionId".into(),
        Value::String(source.execution_id.clone()),
    );
    variables.insert("startDate".into(), Value::String(fmt(source.start_date)));
    if let Some(end) = source.end_date {
        variables.insert("endDate".into(), Value::String(fmt(end)));
    }
    if let Some(task) = &source.last_task_id {
        variables.insert("lastTaskId".into(), Value::String(task.clone()));
    }
    variables.insert("executionLabels".into(), execution_labels(&source.labels));
    serde_json::json!({
        "id": fired.trigger_id,
        "type": FLOW_TYPE,
        "variables": variables,
    })
}

/// Labels as Kestra writes `executionLabels`: a dotted key becomes nested
/// objects (`system.from = api` -> `{"system": {"from": "api"}}`, measured),
/// and `system.correlationId` is left out (measured: absent from all 204). Of
/// two labels where one key is a prefix of the other, the later one wins --
/// not measured.
fn execution_labels(labels: &[Label]) -> Value {
    let mut root = Map::new();
    for label in labels {
        if label.key == "system.correlationId" {
            continue;
        }
        let mut parts: Vec<&str> = label.key.split('.').collect();
        let Some(last) = parts.pop() else { continue };
        let mut node: &mut Map<String, Value> = &mut root;
        for part in parts {
            let slot = node
                .entry(part.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if !slot.is_object() {
                *slot = Value::Object(Map::new());
            }
            node = slot.as_object_mut().expect("just made an object");
        }
        node.insert(last.to_string(), Value::String(label.value.clone()));
    }
    Value::Object(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn oracle() -> Value {
        serde_json::from_str(include_str!("../../oracle-exec/w13-flow-trigger.json"))
            .expect("the Task 1 measurement parses")
    }

    fn labels_of(execution: &Value) -> Vec<Label> {
        execution["labels"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|l| Label {
                        key: l["key"].as_str().unwrap().into(),
                        value: l["value"].as_str().unwrap().into(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn date(v: &Value) -> DateTime<Utc> {
        v.as_str()
            .expect("a date string")
            .parse()
            .expect("RFC 3339")
    }

    fn fired(trigger_id: &str) -> FiredExecution {
        FiredExecution {
            namespace: "w13".into(),
            flow_id: "listener".into(),
            trigger_id: trigger_id.into(),
            trigger_execution_id: Uuid::nil(),
            execution_depth: 1,
            inputs: BTreeMap::new(),
            labels: Vec::new(),
        }
    }

    /// The keys egret deliberately does not write. Asserted as a SET, so a key
    /// the measurement has and the record lacks cannot pass unnoticed.
    const OMITTED: &[&str] = &["flowRevision"];

    fn assert_matches_measured(got: &Value, captured: &Value) {
        let mut want = captured.clone();
        let vars = want["variables"].as_object_mut().unwrap();
        for k in OMITTED {
            assert!(vars.remove(*k).is_some(), "the measurement has `{k}`");
        }
        assert_eq!(got, &want);
    }

    /// Hop 2 is reproduced from the SOURCE's own record in the file -- its
    /// id, flow, final state, history dates and labels -- and the flow that
    /// ran it, not from the variables under test.
    #[test]
    fn the_hop_2_record_is_rebuilt_from_its_source_execution() {
        let o = oracle();
        let captured = &o["executions"]["listener2_hop2_SUCCESS"]["trigger"];
        let src = &o["executions"]["listener_hop1_SUCCESS"];
        assert_eq!(captured["variables"]["executionId"], src["id"], "control");
        let histories = src["state"]["histories"].as_array().unwrap();
        let record = SourceRecord {
            execution_id: src["id"].as_str().unwrap().into(),
            namespace: o["flows"]["listener"]["namespace"].as_str().unwrap().into(),
            flow_id: src["flowId"].as_str().unwrap().into(),
            state: src["state"]["current"].as_str().unwrap().into(),
            start_date: date(&histories[0]["date"]),
            end_date: Some(date(&histories[histories.len() - 1]["date"])),
            last_task_id: o["flows"]["listener"]["revision_2"]["tasks"]
                .as_array()
                .unwrap()
                .last()
                .map(|t| t["id"].as_str().unwrap().to_string()),
            labels: labels_of(src),
        };
        let got = flow_trigger_record(&fired(captured["id"].as_str().unwrap()), &record);
        assert_matches_measured(&got, captured);
    }

    /// The two fires whose source is a `producer` run. The file keeps the
    /// producer's id, flow, state and labels but not its history, so the two
    /// dates are taken from the measured variables; everything else comes
    /// from the producer's record and flow.
    #[test]
    fn the_records_fired_by_a_producer_run_match_the_measurement() {
        let o = oracle();
        for (fired_key, source_key) in [
            ("listener_hop1_SUCCESS", "producer_run2"),
            ("listener_run1_FAILED_bad_inputs", "producer_run1"),
        ] {
            let captured = &o["executions"][fired_key]["trigger"];
            let src = &o["executions"][source_key];
            assert_eq!(captured["variables"]["executionId"], src["id"], "control");
            let record = SourceRecord {
                execution_id: src["id"].as_str().unwrap().into(),
                namespace: o["flows"]["producer"]["namespace"].as_str().unwrap().into(),
                flow_id: src["flowId"].as_str().unwrap().into(),
                state: src["state"]["current"].as_str().unwrap().into(),
                start_date: date(&captured["variables"]["startDate"]),
                end_date: Some(date(&captured["variables"]["endDate"])),
                last_task_id: o["flows"]["producer"]["tasks"]
                    .as_array()
                    .unwrap()
                    .last()
                    .map(|t| t["id"].as_str().unwrap().to_string()),
                labels: labels_of(src),
            };
            let got = flow_trigger_record(&fired(captured["id"].as_str().unwrap()), &record);
            assert_matches_measured(&got, captured);
        }
    }

    /// A non-terminal state has no `endDate`, and a source with no task run
    /// has no `lastTaskId` -- both omitted rather than written null.
    #[test]
    fn a_paused_source_with_no_task_run_omits_end_date_and_last_task() {
        let at: DateTime<Utc> = "2026-09-25T03:17:27.099830Z".parse().unwrap();
        let record = SourceRecord {
            execution_id: "x".into(),
            namespace: "n".into(),
            flow_id: "f".into(),
            state: "PAUSED".into(),
            start_date: at,
            end_date: None,
            last_task_id: None,
            labels: vec![Label {
                key: "team".into(),
                value: "data".into(),
            }],
        };
        let got = flow_trigger_record(&fired("t"), &record);
        assert_eq!(
            got["variables"],
            serde_json::json!({
                "state": "PAUSED",
                "flowId": "f",
                "namespace": "n",
                "executionId": "x",
                "startDate": "2026-09-25T03:17:27.099830Z",
                "executionLabels": {"team": "data"},
            })
        );
    }
}
