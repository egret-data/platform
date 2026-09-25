//! `process_events`: drain `workflow.flow_trigger_events`, one event per
//! transaction, and fire every window-free `Flow` listener that hears it (W13
//! spec §4.4, §4.5). The statement-by-statement argument is in the module doc
//! of [`crate::flow_trigger`].

use chrono::{DateTime, SecondsFormat, Utc};
use egret_core::{Error, Result};
use serde_json::{Map, Value};
use sqlx::{Connection, PgConnection, PgPool};
use uuid::Uuid;

use crate::models::{ExecutionKind, Label, State, TriggerKind};
use crate::runners::{render_str_guarded, GuardedRender, RenderContext};
use crate::scheduler::start::{classify, flow_is_on, Started, WORKFLOWS_FEATURE};
use crate::trigger::flow::{
    evaluate, FiredExecution, ListeningFlow, Outcome, SourceExecution, WhenVerdict,
    WindowFreeFlowTrigger, DEFAULT_MAX_EXECUTION_DEPTH,
};
use crate::trigger::webhook::{render_value, InputRefusal};
use crate::worker::{execution_render_context, start_execution_in, NewExecution};

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

/// The reason a `dependsOn` listener is skipped, logged and counted as
/// `refused`. W13a's `settle` writes events such a listener hears, because the
/// hook's `EXISTS` does not read `depends_on` (spec §4.2).
pub const DEPENDS_ON_IS_W13B: &str =
    "a dependsOn Flow trigger needs a multiple-condition window, which W13b ships; \
     W13a skips it";

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
    /// `dependsOn` listener (W13b's), a refusal by
    /// [`evaluate`](crate::trigger::flow::evaluate) (including a `when` that
    /// does not render), or a fire the unique index says already happened.
    pub refused: usize,
    /// Listeners that failed. Either deterministically -- the flow does not
    /// parse, resolve or plan -- and the event is done with them, or possibly
    /// transiently, and the event is kept for them (spec §4.4 point 1).
    pub errored: usize,
}

/// The claimed event, as the claim read it under its row lock.
#[derive(sqlx::FromRow)]
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
    depends_on: bool,
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
    namespace: String,
    flow_key: String,
    execution_depth: i32,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    source: String,
    last_task_id: Option<String>,
    loop_index: Option<i32>,
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
    /// Deterministic in the listener's source. Done with: only a save helps.
    Broken(String),
    /// Possibly transient. The listener's savepoint rolled back and the event
    /// is kept for it.
    Retry(String),
}

/// One pass: every event in the outbox claimed at most once. Returns when none
/// is left.
///
/// Each event is claimed, evaluated, fired and then deleted (or kept, with its
/// `done` listeners and `attempts + 1`) in **its own** transaction, at
/// `REPEATABLE READ`, so every read of the source -- its row, its task runs,
/// its render surface -- sees one snapshot. `SKIP LOCKED` partitions the
/// outbox between replicas; `seen` makes "at most once per pass" a property of
/// the query, so a kept event -- always the oldest -- cannot take the pass.
///
/// An event whose transaction fails is rolled back -- every fire it made
/// and its own update -- counted as one `errored`, logged at `warn`, and
/// skipped for the rest of the pass, so the next pass retries it and newer
/// events are not held behind it. `Err` is returned only when a transaction
/// cannot even begin or the claim itself fails.
pub async fn process_events(pool: &PgPool) -> Result<EventReport> {
    let mut report = EventReport::default();
    let mut seen: Vec<i64> = Vec::new();
    loop {
        let mut tx = pool
            .begin_with("BEGIN ISOLATION LEVEL REPEATABLE READ")
            .await
            .map_err(db_err)?;
        let claimed: std::result::Result<Option<Event>, sqlx::Error> = sqlx::query_as(
            "SELECT id, source_execution_id, workspace_id, state, done, attempts \
               FROM workflow.flow_trigger_events \
              WHERE id <> ALL ($1) \
              ORDER BY id \
              LIMIT 1 \
              FOR UPDATE SKIP LOCKED",
        )
        .bind(&seen)
        .fetch_optional(&mut *tx)
        .await;
        let ev = match claimed {
            Ok(Some(ev)) => ev,
            Ok(None) => {
                tx.rollback().await.map_err(db_err)?;
                return Ok(report);
            }
            // The one serialization failure this transaction can meet (see
            // the module doc, statement 2): another processor deleted or
            // updated the row our snapshot saw, after the snapshot and before
            // our lock. Nothing was done; a fresh snapshot no longer sees it.
            Err(e) if is_serialization_failure(&e) => {
                tx.rollback().await.map_err(db_err)?;
                continue;
            }
            Err(e) => return Err(db_err(e)),
        };
        seen.push(ev.id);
        // What this event adds is kept only if its transaction commits.
        let before = report.clone();

        let body = match process_one(&mut tx, &ev, &mut report).await {
            Ok((done, pending)) => finish_event(&mut tx, &ev, &done, &pending).await,
            Err(e) => Err(e),
        };
        let committed = match body {
            Ok(()) => tx.commit().await.map_err(db_err),
            Err(e) => {
                // Best effort: dropping the transaction rolls it back too.
                let _ = tx.rollback().await;
                Err(e)
            }
        };
        // A failure of the event's TRANSACTION -- a savepoint, the final
        // write, the COMMIT -- rolls back every fire it made and leaves the
        // event exactly as it was. It does not end the pass: the event is in
        // `seen`, so the pass moves on to newer events, and the next pass
        // claims this one again. One event that always fails therefore cannot
        // block every event behind it (its own `attempts` do not move, since
        // the update that would count them rolled back too).
        if let Err(e) = committed {
            tracing::warn!(
                event_id = ev.id,
                source_execution_id = %ev.source_execution_id,
                error = %e,
                "a flow-trigger event's transaction failed and rolled back; left for the next pass"
            );
            report = before;
            report.errored += 1;
        }
        report.events += 1;
    }
}

fn is_serialization_failure(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|c| c == "40001")
}

/// Everything that happens to one claimed event before its row is deleted or
/// kept. Returns `(done, pending)`: the listener keys the event is finished
/// with, including the ones an earlier pass finished, and the ones a later
/// pass must retry.
async fn process_one(
    tx: &mut PgConnection,
    ev: &Event,
    report: &mut EventReport,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut done = ev.done.clone();
    let mut pending = Vec::new();

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
        "SELECT l.flow_id, l.trigger_id, l.depends_on, f.workspace_id, f.source, \
                f.namespace, f.flow_id AS flow_key \
           FROM workflow.flow_listeners l JOIN workflow.flows f ON f.id = l.flow_id \
          WHERE l.workspace_id = $1 \
            AND $2 = ANY (l.states) \
            AND NOT l.disabled \
            AND",
        flow_is_on!("$3"),
        "ORDER BY f.namespace, f.flow_id, l.trigger_id",
    ))
    .bind(ev.workspace_id)
    .bind(&ev.state)
    .bind(WORKFLOWS_FEATURE)
    .fetch_all(&mut *tx)
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
        let mut sp = tx.begin().await.map_err(db_err)?;
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
            pending.extend(todo.iter().map(|l| l.key()));
            return Ok((done, pending));
        }
    };

    for l in todo {
        let key = l.key();
        match listen(tx, ev, l, &source, &ctx).await? {
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
                    "a flow-trigger listener cannot fire until its flow is saved again"
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
                pending.push(key);
            }
        }
    }
    Ok((done, pending))
}

/// The event's last statement: gone when nothing is pending or it has had its
/// [`MAX_ATTEMPTS`], otherwise kept with what is done and one more attempt.
/// Under the claim's row lock, in the claim's transaction (statement 5).
async fn finish_event(
    tx: &mut PgConnection,
    ev: &Event,
    done: &[String],
    pending: &[String],
) -> Result<()> {
    if !pending.is_empty() && ev.attempts + 1 < MAX_ATTEMPTS {
        sqlx::query(
            "UPDATE workflow.flow_trigger_events SET done = $2, attempts = attempts + 1 \
              WHERE id = $1",
        )
        .bind(ev.id)
        .bind(done)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        return Ok(());
    }
    if !pending.is_empty() {
        tracing::warn!(
            event_id = ev.id,
            source_execution_id = %ev.source_execution_id,
            state = %ev.state,
            attempts = ev.attempts + 1,
            listeners = ?pending,
            "gave up on a flow-trigger event after {MAX_ATTEMPTS} attempts; these listeners never fired"
        );
    }
    sqlx::query("DELETE FROM workflow.flow_trigger_events WHERE id = $1")
        .bind(ev.id)
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
        "SELECT e.namespace, e.flow_key, e.execution_depth, e.created_at, e.updated_at, \
                e.source, e.loop_index, \
                (SELECT r.task_id FROM workflow.task_runs r \
                  WHERE r.execution_id = e.id ORDER BY r.run_ix DESC LIMIT 1) AS last_task_id \
           FROM workflow.executions e WHERE e.id = $1",
    )
    .bind(ev.source_execution_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(db_err)?;
    let ctx = execution_render_context(conn, ev.source_execution_id).await?;
    let labels = crate::spec::parse(&row.source)
        .map(|f| f.labels)
        .unwrap_or_default();
    let execution = SourceExecution {
        id: ev.source_execution_id,
        workspace_id: ev.workspace_id,
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
    tx: &mut PgConnection,
    ev: &Event,
    l: &Listener,
    source: &Source,
    ctx: &RenderContext,
) -> Result<Fate> {
    if l.depends_on {
        return Ok(Fate::Refused(DEPENDS_ON_IS_W13B.to_string()));
    }
    let flow = match crate::spec::parse(&l.source) {
        Ok(f) => f,
        Err(e) => {
            return Ok(Fate::Broken(format!(
                "the listener flow does not parse: {}",
                e.message
            )))
        }
    };
    let Some(trigger) = flow
        .triggers
        .iter()
        .find(|t| t.id == l.trigger_id && matches!(t.kind, TriggerKind::Flow(_)))
    else {
        return Ok(Fate::Broken(
            "the listener row names a Flow trigger its flow does not declare".to_string(),
        ));
    };
    let listener = match WindowFreeFlowTrigger::resolve(trigger) {
        Ok(r) => r,
        Err(e) => return Ok(Fate::Broken(e.to_string())),
    };

    // `when` renders against the SOURCE's context (spec §4.4; `trigger::flow`
    // point 3). A render error is `Unrenderable`, which `evaluate` refuses.
    let when = match listener.when_source() {
        None => WhenVerdict::NotDeclared,
        Some(w) => match render_str_guarded(w, ctx, FORBIDDEN_ROOTS) {
            Ok(GuardedRender::Rendered(s)) => WhenVerdict::Rendered(s),
            Ok(GuardedRender::Forbidden(_)) | Err(_) => WhenVerdict::Unrenderable,
        },
    };
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
    let fired = match evaluate(
        &listener,
        &listening,
        &source.execution,
        &when,
        DEFAULT_MAX_EXECUTION_DEPTH,
    ) {
        Outcome::Refused(r) => return Ok(Fate::Refused(format!("{r:?}"))),
        Outcome::Fired(f) => f,
    };

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

    // The listener's savepoint: whatever `start_execution_in` does -- its
    // insert, the first settle, an aborted statement -- rolls back alone, so a
    // sibling's fire in this transaction survives it (spec §4.4 point 1).
    let mut sp = tx.begin().await.map_err(db_err)?;
    let started = start_execution_in(
        &mut sp,
        NewExecution {
            workspace_id: l.workspace_id,
            // Always the listener's uuid: the backstop index keys on it, and
            // NULLs are distinct, so `None` would let a second fire through.
            flow_id: Some(l.flow_id),
            source: &l.source,
            inputs,
            trigger: Some(flow_trigger_record(&fired, &source.record)),
            scheduled_date: None,
            trigger_execution_id: Some(fired.trigger_execution_id),
            execution_depth: i32::try_from(fired.execution_depth)
                .map_err(|e| Error::Internal(e.into()))?,
            flow_trigger_event_id: Some(ev.id),
            born_failed: render_failure.is_some(),
        },
    )
    .await;
    Ok(match started {
        Ok(_) => {
            sp.commit().await.map_err(db_err)?;
            match render_failure {
                None => Fate::Fired,
                Some(why) => Fate::FiredFailed(why),
            }
        }
        Err(err) => {
            sp.rollback().await.map_err(db_err)?;
            match classify(&err) {
                Started::Yes => Fate::Fired,
                Started::Refused(why) => Fate::Broken(why),
                Started::AlreadyFired(why) => Fate::AlreadyFired(why),
                Started::Failed(why) => Fate::Retry(why),
            }
        }
    })
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
/// | `executionLabels` | the source FLOW's declared `labels:`, dotted keys nested, `system.correlationId` excluded | measured: `system.from` nests under `system`, and `correlationId` is absent. egret has no execution labels (spec §4.5), so the flow's own labels -- which Kestra copies onto every execution -- are the nearest value; with none declared it is `{}`, which is what hop 2 measured |
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
