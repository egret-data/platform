//! The render surface: what a `{{ … }}` in a running execution resolves
//! against, and the tier that fills `advance()`'s two render slots.
//!
//! # Why this exists at all
//!
//! `advance()` is pure over `(plan, state, now)` and therefore cannot render.
//! W3a's oracle harness rendered on its behalf and reached exactly two names —
//! `inputs` and `vars` — while Kestra's `RunContext` reaches `outputs`,
//! `taskrun`, `execution`, `flow`, the loop `item` and `tasksWithState()`. The
//! W3b spec (§4.4) calls that gap the reason W3c's `runIf:` fix banks nothing
//! and the reason `loop-outputs-auto` cannot resolve its `values:`.
//!
//! Until this module, **nothing in the worker rendered anything**: Task 2's
//! Concern 2 and Task 3's Concern 4 both name it, and the consequence is not an
//! error but a **stall** — `advance()` returns an empty command vector for ever
//! for any flow with a `runIf:`, an `If`, a `Switch` or a `Loop`, exactly as
//! its own doc warns. So the tier is here, inside
//! [`settle`](super::claim::settle), rather than in the oracle's driver: a
//! render tier that lived only in a test would leave the shipping worker
//! stalling on a quarter of the corpus.
//!
//! # What is supplied, and where each name is measured
//!
//! | name | shape | corpus uses |
//! |---|---|---|
//! | `inputs` | the execution's posted inputs, plus the flow's own declared `defaults:` for the ones it did not post | many |
//! | `vars` | the flow's `variables:` block | many |
//! | `outputs` | `task id -> the handler's output map`, `{}` when it produced none | 14 comparable entries |
//! | `task` | `{id, type}` of the task being rendered | `id` 177, `type` 2 |
//! | `taskrun` | `{id, startDate, value, attemptsCount, parentId}` of the run being rendered | `startDate` 52, `id` 17, `attemptsCount` 9, `value` 8 |
//! | `execution` | `{id, startDate, state, outputs}` | `id` 30, `state` 4, `startDate` 1 |
//! | `flow` | `{id, namespace}` | `id` 10, `namespace` 2 |
//! | `labels` | the flow's own `labels:`, plus `labels.system.correlationId` | 2 |
//! | `item` | `{index, value, key, parent, parents}` inside a loop sub-execution | `valids/loop-serial` writes `{{item.index}} - {{item.value}}`; `loop-map` `{{item.key}}`; `loop-nested` `{{item.parent.index}}` and `{{item.parents[0].value}}` |
//! | `tasks` | `task id -> {state}` — the variable `tasksWithState()` reads | 1 |
//!
//! **`outputs.<id>` is `{}` for a task that produced nothing, never an absent
//! key**, and that is measured on the artifact rather than reasoned from
//! Kestra's Java. W3b's Task 3 brief said the opposite — that three of the four
//! handlers `return null` so the key would be *absent* — and labelled the
//! distinction load-bearing for exactly this module. On
//! `kestra/kestra:v2.0.0-rc13` a flow whose second task renders
//! `{{ outputs.a }}` printed `{}` for `a` = a `Log`, a `Sleep`, a falsy `Fail`
//! **and** a `Return` with no `format:`; `{{ outputs }}` over two such tasks
//! printed `{"a":{},"b":{}}`; and `{{ outputs.a.value }}` FAILED the run in all
//! four. So `Outcome::outputs == None` maps to `{}` here and the key is always
//! written.
//!
//! # A key exists only once the task has TERMINATED
//!
//! `outputs` and `tasks` carry a task that has reached a terminal state and no
//! other. A running task has no output yet, and a task that has not been
//! created has no state — writing either would let an expression read a value
//! Kestra could not have seen, which is the over-acceptance shape spec §7
//! forbids.
//!
//! # What is NOT supplied, and stays a refusal
//!
//! `trigger`, `kv`, `secret`, `envs`, `globals` and the flow-level `outputs:`
//! block. Each is a name a corpus flow writes and this worker cannot resolve;
//! an expression over one of them FAILS the run rather than rendering to
//! nothing, which is a divergence in the safe direction and is visible in the
//! trace instead of silently choosing a branch.

use egret_core::Result;
use serde_json::{json, Map, Value};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::claim::ExecutionCtx;
use crate::executor::{ExecutionState, Plan, RunIx, RunState, TaskKind, Tick};
use crate::models::{Flow, InputType};
use crate::runners::{FileScope, RenderContext};

/// Everything a render needs that lives outside `ExecutionState`, read once
/// per [`settle`](super::claim::settle).
///
/// It is one async read rather than one per expression because none of it can
/// change while `settle` holds the execution's row lock: a handler's outputs
/// are written before `settle` is entered, and no handler runs inside it.
#[derive(Debug, Clone)]
pub(super) struct Surface {
    /// **The flow this execution runs, parsed once** — W8 Task 6 fix round 1.
    ///
    /// `load` already parsed it to build `inputs`, `vars`, `flow` and `labels`
    /// and then threw it away, so `claim::sla_terminal` parsed the same source
    /// a second time on **every settle of every execution** — 339 of the
    /// corpus's 345 flows declare no `sla:` at all, and every one of them paid
    /// for a parse to be told so. Keeping it also fixes a subtler thing: it
    /// lets that guard ask *"is the SLA list empty"* **before** anything else,
    /// which is the order `ExecutorService.handleExecutionChangedSLA` asks its
    /// guards in, instead of parse-then-ask.
    ///
    /// `None` for a source that does not parse. That is not an error here: the
    /// execution is already running, `ExecutionCtx::plan` was built from this
    /// same source, and every reader below already treats an unparseable source
    /// as "no information" rather than as a failure.
    parsed: Option<Flow>,
    /// `inputs`, already merged with the flow's declared defaults.
    inputs: Value,
    /// `vars`.
    vars: Value,
    /// `flow`.
    flow: Value,
    /// `labels`.
    labels: Value,
    /// `execution`.
    execution: Value,
    /// `item`, for a loop sub-execution. `None` at the top level.
    item: Option<Value>,
    /// The outputs of every LOOP ancestor's terminated runs, by task id.
    ///
    /// A loop body renders in its own sub-execution and reads the parent's
    /// outputs: `valids/loop-expression-context`'s body is
    /// `format: "{{outputs.before.value}}"` where `before` is a `Return` in the
    /// PARENT. Kestra passes the parent context down; this is that, read up the
    /// `parent_execution_id` chain.
    ///
    /// **It stops at the first link that is not a loop iteration** -- see
    /// `inheriting` in [`Surface::load`]. Migration 0138 made a second kind of
    /// `parent_execution_id` child representable, the `CREATE_NEW_EXECUTION`
    /// replay, and a replay must not read the outputs of the attempt it is
    /// replacing.
    inherited: Map<String, Value>,
    /// This execution's own stored outputs, by `run_ix`.
    own: std::collections::BTreeMap<usize, Value>,
    /// Each run's `created_at`, by `run_ix`, as `taskrun.startDate` reads it.
    started: std::collections::BTreeMap<usize, String>,
    /// Each run's surrogate id, by `run_ix`, as `taskrun.id` reads it.
    ids: std::collections::BTreeMap<usize, Uuid>,
    /// W9a Task 4: the files the four file functions read, attached by
    /// `settle` after [`Surface::load`] ([`Surface::attach_files`]). `None`
    /// everywhere else -- notably [`execution_render_context`], so a Flow
    /// trigger's `conditions` and `inputs` refuse them (spec §7.2).
    files: Option<FileScope>,
}

/// One row's links upward, as `Surface::load`'s walk reads them.
///
/// A named struct rather than the four-wide tuple it replaced, because migration
/// `0138` gives a row **two** possible causes and the walk has to say which one
/// it is following. `workflow_executions_one_link_kind` makes them mutually
/// exclusive, which is what makes [`Link::cause`] unambiguous — the same
/// guarantee `worker::subflow::chain_holds`'s `COALESCE` rests on, and this is
/// the second consumer to be corrected for the same reason.
struct Link {
    loop_index: Option<i32>,
    loop_value: Option<String>,
    /// A loop iteration's or a `CREATE_NEW_EXECUTION` replay's parent.
    parent: Option<Uuid>,
    parent_run_ix: Option<i32>,
    /// A subflow child's caller. Never set together with [`Link::parent`].
    trigger: Option<Uuid>,
}

impl Link {
    /// The execution that caused this one, **whichever link it carries**.
    ///
    /// `None` for a top-level execution. Not an `or_else` over two independent
    /// possibilities: the CHECK makes at most one of the two present, so this
    /// is a single value read out of two columns.
    fn cause(&self) -> Option<Uuid> {
        self.parent.or(self.trigger)
    }
}

impl Surface {
    /// Read everything the renders of one `settle` will need.
    pub(super) async fn load(conn: &mut PgConnection, ctx: &ExecutionCtx) -> Result<Self> {
        let row = sqlx::query(
            "SELECT inputs, state, created_at, loop_index, loop_value, \
                    parent_execution_id, parent_run_ix, trigger_execution_id \
               FROM workflow.executions WHERE id = $1",
        )
        .bind(ctx.id)
        .fetch_one(&mut *conn)
        .await
        .map_err(db_err)?;

        let flow = crate::spec::parse(&ctx.source).ok();
        let posted: Value = row.get("inputs");
        let inputs = with_declared_defaults(posted, flow.as_ref());
        let vars = flow
            .as_ref()
            .and_then(|f| serde_json::to_value(&f.variables).ok())
            .unwrap_or(Value::Null);
        let flow_value = flow
            .as_ref()
            .map(|f| json!({"id": f.id, "namespace": f.namespace}))
            .unwrap_or(Value::Null);
        // `labels`: the flow's own `labels:` block, plus the ONE system label
        // the corpus reads. Kestra namespaces system labels under `system`.
        //
        // **The artifact's mechanism is a COPY, not a computation, and the
        // gloss this comment used to state is false of the class.** It read
        // *"`correlationId` is the id of the execution at the root of the
        // parent chain"*. Kestra copies the cause's own `system.correlationId`
        // **label** verbatim (`Flow.evaluate`'s `lambda$evaluate$2`;
        // `ExecutionService.replay` for a replay) and `Execution.java:213`
        // fills one in only when the label is **ABSENT** — so the root's id is
        // what the copying *produces* in a chain Kestra generated, and nothing
        // more. Measured on the pinned artifact by W6 Task 6: a root started
        // with an explicit `system.correlationId: ZZZTESTROOT` hands
        // `ZZZTESTROOT` — not an execution id at all — to all three children.
        // The derivation is in `crate::trigger::flow`'s module doc, and
        // `trigger::flow::FiredExecution::correlation_id` owns the gap.
        //
        // `root` below is the **causal** chain's root id, computed by walking
        // whichever link each row carries (see `Link::cause`). **That sentence
        // read "walking `parent_execution_id`" until W6 Task 8's whole-branch
        // review, and the claim attached to it — "agrees with the artifact on
        // every machine-generated chain" — was FALSE**: a subflow chain is
        // machine-generated, and the walk stopped at the first subflow child,
        // whose `parent_execution_id` is NULL. See the `root` paragraph below
        // for the artifact measurement that settled it.
        //
        // **What remains is the seeded case, and nothing in this oracle can
        // reach it** — derived rather than assumed: three corpus flows declare
        // a `system.*` label at all (`invalids/system-labels` twice,
        // `valids/minimal` once) and **none of the three is
        // `system.correlationId`**, while the only two flows whose text names
        // `correlationId` read it and seed nothing. See
        // `the_seeded_correlation_id_divergence_has_no_corpus_population` in
        // `tests/executor.rs`, which compares all three figures — and note what
        // it cannot do: both readers are compared by the oracle **only as root
        // executions**, where every implementation of this walk agrees, which
        // is how the walk defect above survived a test that named this exact
        // subject. The instrument for the walk is
        // `a_subflow_grandchild_correlates_to_the_root_and_inherits_no_item`.
        //
        // Related, same mechanism, same population of zero: the unconditional
        // `labels.insert("system", …)` below **shadows** a declared `system.*`
        // label, so `valids/minimal`'s `system.readOnly` would not render — it
        // matches because the flow never reads it. And `system.from`, which the
        // artifact also sets (measured: `[{"key": "system.from", "value":
        // "api"}, {"key": "system.correlationId", …}]` on every execution of
        // the probe chain), is not supplied here at all.
        //
        // Measured from the two entries that read the label:
        // `valids/follow-dependencies-partial-child`
        // (`{{ labels.system.correlationId }}`) and
        // `valids/subflow-grand-child` (`{{labels.system.correlationId}}`, **no
        // spaces** — the two spellings are why the test that counts them scans
        // for the path and not for a phrase), both in a `Log`, both SUCCESS in
        // the golden. No other system label is supplied, so an expression over
        // one still fails.
        let mut labels = serde_json::Map::new();
        if let Some(f) = &flow {
            for label in &f.labels {
                labels.insert(label.key.clone(), Value::String(label.value.clone()));
            }
        }
        let started_at: chrono::DateTime<chrono::Utc> = row.get("created_at");
        let execution = json!({
            "id": ctx.id.to_string(),
            "startDate": started_at.to_rfc3339(),
            "state": row.get::<String, _>("state"),
            // The flow-level `outputs:` block is not evaluated by anything in
            // W3b, so this is the empty map rather than an absent key: an
            // author's `{{ execution.outputs }}` renders, and
            // `{{ execution.outputs.x }}` fails, which is the same shape the
            // `outputs` rule above has.
            "outputs": {},
        });
        let mut inherited = Map::new();
        let mut root = ctx.id;
        // Each level of the loop nesting, nearest first: this execution's own
        // iteration, then its parent's, and so on. `valids/loop-nested` is
        // three deep and reads `{{ item.parents[1].value }}`.
        let mut levels: Vec<Value> = Vec::new();
        let mut here = Link {
            loop_index: row.get("loop_index"),
            loop_value: row.get("loop_value"),
            parent: row.get("parent_execution_id"),
            parent_run_ix: row.get("parent_run_ix"),
            trigger: row.get("trigger_execution_id"),
        };
        let mut ancestor: Option<Uuid> = here.cause();
        // **Whether the chain walked so far is loops all the way up.**
        //
        // Until migration 0138 this could not be false: 0135's CHECK made a
        // row with a parent and no loop index unrepresentable, so every
        // `parent_execution_id` was a loop iteration's and the walk below could
        // merge unconditionally. 0138 admits the `CREATE_NEW_EXECUTION` replay
        // -- a parent, no loop columns -- and a replay is a NEW execution of
        // the same flow, not a body running inside its parent's context. It
        // must not read `{{ outputs.<task> }}` off the attempt it is replacing:
        // that would render a value from a FAILED run and take the other
        // branch, silently, with no error anywhere. (The sibling consumer,
        // `oracle-exec/driver.rs`'s `spawned` decode, fails loudly on the same
        // row; this one would not, which is why it is guarded rather than
        // documented.)
        //
        // Sticky: once a non-loop link is crossed, everything above it is
        // reachable only THROUGH that link, so nothing above it may contribute
        // either.
        //
        // **The `levels`/`item` walk is gated on this same flag, since W6 Task
        // 8.** A level is pushed only when the row below the link carries
        // `(loop_index, loop_value, parent_run_ix)`, so a replay or a subflow
        // child contributes none of its own -- but the walk now crosses a
        // SUBFLOW link (below), which makes `loop -> subflow child` writable,
        // and without the gate that child would resolve `{{ item.value }}` off
        // the loop iteration that spawned its PARENT. A subflow child is a
        // separate execution whose values arrive as `inputs:`; nothing of the
        // caller's iteration reaches it. The gate is a no-op on every row that
        // existed before, and the reason needs no claim about what is
        // WRITABLE: a replay never carries loop columns at all, so it never
        // pushes a level. (An earlier draft here said "a replay's only writable
        // shape is a replay of a top-level execution". That is false --
        // `worker::replay::spawn_due` refuses only `parent_run_ix IS NOT NULL`,
        // and a SUBFLOW CHILD has it NULL and is replayable; the crate writes
        // that row on purpose in
        // `a_subflow_child_is_replayed_and_egret_cannot_report_it_to_the_parent`.
        // The conclusion held, the premise was narrower than true -- which is
        // the shape this whole finding was about. `spawn_due` does refuse a
        // replay under a `Loop`, because the artifact refuses it too, throwing
        // `Can't find taskrun with taskrunId <the LOOP run's>` out of
        // `markWithTaskRunAs(..., markParents = true)`.) It also closes the
        // asymmetry W6 Task 2's re-review recorded as N-2, where `item` crossed
        // a non-loop link while `outputs` stopped.
        //
        // **`root` walks the whole CAUSAL chain -- whichever link each row
        // carries -- and that is a W6 Task 8 fix, not the original.** It feeds
        // `labels.system.correlationId`.
        //
        // It walked `parent_execution_id` ALONE, which is the same defect W6
        // Task 3's fix round 1 found in `subflow::chain_holds` and did not sweep
        // for: `workflow_executions_one_link_kind` makes a child carry exactly
        // ONE of two links, so a walk keyed on one column stops dead at a
        // subflow child -- and `root` then answered the child's OWN id. Three
        // artefacts of this package said the opposite and called it measured;
        // the whole-branch review found it, and the Kestra half was then
        // measured on the pinned digest rather than argued.
        //
        // **Measured**, `t8.probe/corr-parent -> corr-child -> corr-grand-child`
        // (the corpus's own `subflow-parent -> subflow-child ->
        // subflow-grand-child` shape, renamed), read out of Kestra's Postgres:
        //
        //     flow              id                      parentId  trigger.…executionId  system.correlationId
        //     corr-parent       2cvzjgi7mY6lDfaQxiNa0x  null      (started by hand)     2cvzjgi7mY6lDfaQxiNa0x
        //     corr-child        8yWnkcC1oAvHsunjmTG1Q   null      2cvzjgi7mY6lDfaQxiNa0x  2cvzjgi7mY6lDfaQxiNa0x
        //     corr-grand-child  2Cr8T9lsAM5icJ7KT5NEna  null      8yWnkcC1oAvHsunjmTG1Q    2cvzjgi7mY6lDfaQxiNa0x
        //
        // So the label is the ROOT's id two links down, `parentId` is NULL at
        // every hop, and the cause is `trigger.variables.executionId` -- the
        // same shape `crate::trigger::flow`'s module doc measured for a flow
        // trigger. The corpus reaches this: `valids/subflow-grand-child` logs
        // `{{labels.system.correlationId}}` and is the target of
        // `valids/subflow-child`, which is one of the twelve entries W6 banked.
        // The ratchet cannot see it -- the golden records states, and a `Log`
        // renders either value without failing.
        //
        // A replay still inherits for the reason W6 Task 4 measured
        // (`egret.probe4/replay-correlation`: the replay's label is the
        // ORIGINAL execution's id while `{{ execution.id }}` is its own):
        //
        //     root  cid=3nY5p1H9MmhpWnq4hZsVOD eid=3nY5p1H9MmhpWnq4hZsVOD
        //     replay cid=3nY5p1H9MmhpWnq4hZsVOD eid=5cNdx41IqqhyGSExZsuGLu
        //
        // The mechanism agrees for both kinds of child, which is why one walk
        // serves both: `ExecutionService.replay` and `Flow.evaluate` copy the
        // cause's labels, and `Execution.java:213` fills a `correlationId` in
        // only when one is ABSENT. The gloss is still only a gloss -- see the
        // `labels` comment above for the seeded root that hands a non-id string
        // down a whole chain.
        //
        // No golden entry can report either way, so
        // `a_replay_inherits_the_correlation_id_of_the_execution_it_replaces`
        // and `a_subflow_grandchild_correlates_to_the_root_and_inherits_no_item`
        // in `tests/worker.rs` are the instruments, and the second one carries
        // the loop and standalone controls that tell "the walk works" from "the
        // walk stops one hop early".
        //
        // The chain stays a DAG: `parent_execution_id` and
        // `trigger_execution_id` both name a row that existed before this one
        // and `workflow_executions_no_self_link` refuses the degenerate case,
        // so there is still no depth bound here, for the reason
        // `flowable::descend` gives for not having one either.
        //
        // **Finite is not the same as short, and W13a made it matter**: the
        // walk crosses a Flow-trigger link too (`trigger_execution_id`), so a
        // Flow-trigger chain is walked whole. What keeps that chain short is
        // `execution_depth` (final review I1): a Flow-trigger fire and a
        // `Subflow` child each add one and both refuse past 100, a replay and
        // a `Loop` iteration keep their parent's, and an iteration never fires
        // a Flow trigger. Before that fix a cycle through a `Loop` reset the
        // count and this walk grew with it.
        let mut inheriting = true;
        // Nearest ancestor first, so a nearer scope wins where two ancestors
        // ran a task of the same id. The chain is a DAG by construction --
        // `parent_execution_id` always names a row that existed before this one
        // -- which is why there is no depth bound, for the reason
        // `flowable::descend` gives for not having one either.
        while let Some(id) = ancestor {
            // This level's `item`, built while the parent that holds its
            // resolved `values:` is the row being read. Gated on `inheriting`
            // for the reason above: the walk crosses a subflow link now, and
            // nothing of the caller's iteration may reach a subflow child.
            if let (true, Some(index), Some(value), Some(run_ix)) = (
                inheriting,
                here.loop_index,
                here.loop_value.clone(),
                here.parent_run_ix,
            ) {
                let rendered: Option<String> = sqlx::query_scalar(
                    "SELECT rendered FROM workflow.task_runs \
                      WHERE execution_id = $1 AND run_ix = $2",
                )
                .bind(id)
                .bind(run_ix)
                .fetch_optional(&mut *conn)
                .await
                .map_err(db_err)?
                .flatten();
                levels.push(json!({
                    "index": index,
                    "value": value,
                    // `key` exists only for a MAP `values:` -- `valids/loop-map`
                    // iterates `{key1: "value 1", …}` and its body renders
                    // `{{item.key}} - {{item.value}}`. The values themselves are
                    // the map's VALUES (measured: the golden's `spawned` carries
                    // "value 1", "value 2", "value 3"), so the key is the nth of
                    // the same object and has to be read back from the loop
                    // run's own `rendered`.
                    //
                    // Only an OBJECT is parsed (W9a Task 11): nothing else
                    // has a key. A `Loop` over a file keeps only its URI and
                    // count here (fix round 1), its values being in
                    // `workflow.loop_file_values`.
                    "key": rendered
                        .as_deref()
                        .filter(|r| r.trim_start().starts_with('{'))
                        .and_then(|r| serde_json::from_str::<Value>(r).ok())
                        .and_then(|v| match v {
                            Value::Object(m) => m
                                .keys()
                                .nth(index.max(0) as usize)
                                .map(|k| Value::String(k.clone())),
                            _ => None,
                        }),
                }));
            }
            // `here.loop_index` is the loop index of the row BELOW `id`, i.e.
            // of the link about to be traversed.
            inheriting = inheriting && here.loop_index.is_some();
            if inheriting {
                let rows = sqlx::query(
                    "SELECT task_id, outputs, state FROM workflow.task_runs \
                       WHERE execution_id = $1 ORDER BY run_ix",
                )
                .bind(id)
                .fetch_all(&mut *conn)
                .await
                .map_err(db_err)?;
                for r in &rows {
                    let state: String = r.get("state");
                    if !super::from_wire(&state).is_some_and(|s| s.is_terminal()) {
                        continue;
                    }
                    let task_id: String = r.get("task_id");
                    let outputs: Option<Value> = r.get("outputs");
                    inherited
                        .entry(task_id)
                        .or_insert_with(|| outputs.unwrap_or_else(|| json!({})));
                }
            }
            root = id;
            let up = sqlx::query(
                "SELECT loop_index, loop_value, parent_execution_id, parent_run_ix, \
                        trigger_execution_id \
                   FROM workflow.executions WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .map_err(db_err)?;
            here = Link {
                loop_index: up.get("loop_index"),
                loop_value: up.get("loop_value"),
                parent: up.get("parent_execution_id"),
                parent_run_ix: up.get("parent_run_ix"),
                trigger: up.get("trigger_execution_id"),
            };
            ancestor = here.cause();
        }

        // `item` is this execution's own level, with the enclosing loops
        // attached. `parent` is the nearest and `parents` the whole chain
        // nearest-first — `valids/loop-nested` reads `parents[0]` and
        // `parents[1]` and the golden records only STATES, so which end the
        // list starts from is not observable; what is observable is that both
        // indices resolve.
        let item = levels.first().cloned().map(|mut first| {
            let outer: Vec<Value> = levels[1..].to_vec();
            if let Value::Object(m) = &mut first {
                if let Some(p) = outer.first() {
                    m.insert("parent".to_string(), p.clone());
                }
                m.insert("parents".to_string(), Value::Array(outer));
            }
            first
        });
        labels.insert(
            "system".to_string(),
            json!({"correlationId": root.to_string()}),
        );

        let rows = sqlx::query(
            "SELECT run_ix, id, outputs, created_at FROM workflow.task_runs \
               WHERE execution_id = $1 ORDER BY run_ix",
        )
        .bind(ctx.id)
        .fetch_all(&mut *conn)
        .await
        .map_err(db_err)?;
        let mut own = std::collections::BTreeMap::new();
        let mut started = std::collections::BTreeMap::new();
        let mut ids = std::collections::BTreeMap::new();
        for r in &rows {
            let ix = r.get::<i32, _>("run_ix").max(0) as usize;
            if let Some(o) = r.get::<Option<Value>, _>("outputs") {
                own.insert(ix, o);
            }
            let at: chrono::DateTime<chrono::Utc> = r.get("created_at");
            started.insert(ix, at.to_rfc3339());
            ids.insert(ix, r.get::<Uuid, _>("id"));
        }

        Ok(Surface {
            parsed: flow,
            inputs,
            vars,
            flow: flow_value,
            labels: Value::Object(labels),
            execution,
            item,
            inherited,
            own,
            started,
            ids,
            files: None,
        })
    }

    /// Every JSON value of the render context a `kestra://` URI can arrive in
    /// -- `inputs`, the outputs inherited from loop ancestors, this
    /// execution's own outputs, `execution` and `item` -- for
    /// `storage::scope::Snapshot::take`. The flow source is the caller's.
    ///
    /// `labels` too (fix round 1, M5), though today every label but
    /// `system.correlationId` comes from the flow source the caller already
    /// passes. `vars` and `flow` are the source's own text.
    pub(super) fn snapshot_values(&self) -> Vec<&Value> {
        let mut values = vec![&self.inputs, &self.execution, &self.labels];
        values.extend(self.inherited.values());
        values.extend(self.own.values());
        values.extend(self.item.iter());
        values
    }

    /// The outage a read of this surface's scope met, if any (spec §11):
    /// `settle` turns it into an `Err` so its transaction rolls back.
    pub(super) fn files_outage(&self) -> Option<String> {
        self.files
            .as_ref()
            .and_then(|f| f.0.infrastructure_failure())
    }

    /// W9a Task 11 (spec §9.3): the values of a `Loop` whose `values:`
    /// rendered to `uri`, one per record in file order (no `distinct()`, as
    /// Kestra's URI path) -- or why not, which fails the loop run with that
    /// message. The caller stores `executor::rendered_from_file`'s count and
    /// hands the values to `settle`.
    ///
    /// The file is resolved through this surface's snapshot -- the one
    /// readability decision; `outputs.<task>.uri` is in it because every
    /// terminated task's outputs are -- and read whole, capped by
    /// `workflow.storage.max_read_bytes` (there is no loop-width limit in the
    /// code to reuse). It is read as BYTES, because a plugin's STORE file is
    /// binary Ion and not UTF-8, and parsed by `storage::ion::loop_values`
    /// (the one Ion reader, with Kestra's `ionValueToString` projection), one
    /// top-level value per iteration value. A store OUTAGE is
    /// recorded on the scope, so `settle` turns it into an `Err`.
    ///
    /// Kestra reads the file in batches of `concurrencyLimit` as iterations
    /// end (`Loop.java:404`, `FlowableUtils.java:533-578`); egret reads it
    /// once, before the loop starts, so a `null` record fails the loop before
    /// its first iteration where Kestra fails it at the batch that reaches it
    /// (`loop_over_ion.null_record`: one iteration ran).
    pub(super) fn loop_values_from_file(
        &self,
        uri: &str,
    ) -> std::result::Result<Vec<String>, String> {
        // A `file://` or `nsfile://` URI is refused by the snapshot -- the one
        // readability decision -- as a URI that does not parse, naming it;
        // there is no second check here (mutation j of W9a Task 11 measured
        // one as dead weight).
        let Some(files) = self.files.clone() else {
            return Err(format!(
                "{uri} cannot be read: this render has no file scope"
            ));
        };
        let bytes =
            crate::runners::env::on_a_render_thread(|| files.0.read_bytes(uri, LOOP_READS_WHOLE))??;
        use crate::storage::ion::{loop_values, Stop};
        // The reader's bounds (`ion::for_each_value`) under the same cap the
        // file was read whole with.
        let cap = files.0.max_read_bytes().unwrap_or(bytes.len() as u64);
        let values = match loop_values(std::io::Cursor::new(bytes), usize::MAX, cap) {
            (values, Stop::End | Stop::Full) => values,
            (_, Stop::Failed(e) | Stop::TooDeep(e) | Stop::TooLarge(e) | Stop::Crashed(e)) => {
                return Err(format!("{uri} is not an ION file a Loop can iterate: {e}"))
            }
        };
        // The first record that cannot be a value -- a `null`
        // (`ion::LOOP_NULL_RECORD`, Kestra's message), a `java.time` text that
        // does not parse -- fails the loop, as Kestra's read of it throws.
        values
            .into_iter()
            .collect::<std::result::Result<Vec<String>, String>>()
    }

    /// Attach the file scope every later [`Surface::context`] carries.
    pub(super) fn attach_files(&mut self, files: FileScope) {
        self.files = Some(files);
    }

    /// The flow this execution runs, as `load` parsed it.
    ///
    /// See [`Surface::parsed`] for why it is kept rather than re-parsed.
    pub(super) fn flow(&self) -> Option<&Flow> {
        self.parsed.as_ref()
    }

    /// The context one expression renders against.
    ///
    /// `run` is the run being rendered, which is what `taskrun` names. `None`
    /// is a render with no run of its own — nothing in W3b takes it, and it is
    /// here so that a future caller has to say so rather than pass a run it
    /// invented.
    pub(super) fn context(
        &self,
        plan: &Plan,
        state: &ExecutionState,
        run: Option<RunIx>,
    ) -> RenderContext {
        let mut cx = RenderContext::new();
        cx.insert("inputs", self.inputs.clone());
        cx.insert("vars", self.vars.clone());
        cx.insert("flow", self.flow.clone());
        cx.insert("labels", self.labels.clone());
        cx.insert("execution", self.execution.clone());
        if let Some(item) = &self.item {
            cx.insert("item", item.clone());
        }

        let mut outputs = self.inherited.clone();
        let mut tasks = Map::new();
        for (ix, r) in state.runs.iter().enumerate() {
            let Some(last) = r.history.last().copied() else {
                continue;
            };
            let Some(task) = plan.tasks.get(r.task) else {
                continue;
            };
            // **The one output a RUNNING task publishes, and W8 Task 4 is why
            // the rule above now has an exception.**
            //
            // A `LoopUntil` writes `{iterationCount: n}` onto its own taskrun
            // *before* each iteration's children are created --
            // `ExecutorService.saveFlowableOutput` at creation (Kestra's
            // `INITIAL_LOOP_VALUE`, which is `1`, not `0`) and
            // `taskOutputService.saveOutputs(taskRun, waitFor.outputs(previous))`
            // at every pause. So `{{ outputs.<loop>.iterationCount }}` resolves
            // for a loop that has not terminated, and the corpus is built on
            // that: `valids/waitfor-nested` runs `format: "{{ outputs.loop_1
            // .iterationCount }}"` inside `loop_1` and exits when that value
            // reaches 3, and `valids/loop-until-restart` writes the same
            // expression. Without this arm both children fail on an undefined
            // name -- `runners::env` is `UndefinedBehavior::Strict` -- and the
            // nested flow never terminates at all.
            //
            // **It is `n`, not `n - 1`, and `valids/waitfor-nested` is the row
            // that decides it.** Its three loops each exit on
            // `outputs.<child>.value | number >= 3` where the child returned the
            // loop's own `iterationCount`, and each ran exactly **three**
            // iterations (2 `PAUSED` in each loop run, 26 across the execution
            // -- 2 + 3x2 + 9x2, one per nested restart). A 0-based counter
            // reaches 3 on the fourth iteration, or never.
            //
            // `tasks` is deliberately NOT given the same exception: nothing in
            // the corpus reads `tasks.<id>.state` for a running task, and
            // widening a second name on one construct's evidence is the shape
            // this package keeps paying for.
            if let crate::executor::TaskKind::LoopUntil { .. } = task.kind {
                outputs.insert(
                    task.id.clone(),
                    json!({
                        "iterationCount":
                            crate::executor::loop_iteration(state, ix)
                    }),
                );
            }
            if !last.is_terminal() {
                continue;
            }
            // `None` becomes `{}`: measured on the artifact, an expression
            // cannot tell "no Output object" from an empty one — see the module
            // doc's table.
            if !matches!(task.kind, crate::executor::TaskKind::LoopUntil { .. }) {
                outputs.insert(
                    task.id.clone(),
                    self.own.get(&ix).cloned().unwrap_or_else(|| json!({})),
                );
            }
            tasks.insert(task.id.clone(), json!({"state": last.wire()}));
        }
        cx.insert("outputs", Value::Object(outputs));
        // `tasksWithState(state=…)` reads a `tasks` context variable of exactly
        // this shape — `runners::functions::tasks_with_state` walks
        // `tasks.<id>.state`.
        cx.insert("tasks", Value::Object(tasks));

        if let Some(ix) = run {
            if let Some(r) = state.runs.get(ix) {
                // `task` is the DEFINITION being rendered, `taskrun` the run of
                // it. Two names for two things, and the corpus writes
                // `{{ task.id }}` 177 times -- more than any other context
                // variable -- so leaving it out failed 20 entries whose only
                // expression was that.
                if let Some(t) = plan.tasks.get(r.task) {
                    // **`task.type` is `null` for every kind `build_plan`
                    // descends into, and Kestra's is always the FQCN.** The
                    // divergence set is exactly `{Sequential, Parallel, If,
                    // Switch, Loop, Dag, Subflow}` -- note both ends of it:
                    // `Subflow` is in (it is `Executable`, not a flowable the
                    // renderer sees), and **`Pause` is in too since W8 Task 3**
                    // -- a `TaskKind::Pause` gets no `type_id` either, for the
                    // same reason and by the same line of `claim::persist`.
                    // **`LoopUntil` joined them at W8 Task 4, under BOTH its
                    // spellings** -- a `plan::TaskKind::LoopUntil` gets no
                    // `type_id` either, and `kind_for` now maps
                    // `io.kestra.plugin.core.flow.WaitFor` to the same
                    // `models::TaskKind` as `...LoopUntil`, so the two
                    // spellings no longer disagree here or anywhere else.
                    // (They did through W8 Task 1 fix round 5, when `WaitFor`
                    // reached `models::TaskKind::Leaf`; it changed nothing in
                    // THIS comment even then, because the conclusion is drawn
                    // from the PLAN kind and that was `Runnable` for both.)
                    // The two opaque flowables left, `AllowFailure` and
                    // `WorkingDirectory`, are OUT: they are planned `Runnable`
                    // and so their `type_id` is populated -- as is a `Pause` or
                    // a `LoopUntil` whose own properties `build_plan` cannot
                    // read (`pause_gap`, `loop_until_gap`).
                    //
                    // **Latent, and the reason is nameable rather than
                    // incidental**: only two corpus flows write `{{task.type}}`
                    // (`valids/logs`, `valids/disable-simple`) and both write it
                    // on a `core.log.Log` leaf, so no golden entry can fail on
                    // it and no assertion here could either. The element of the
                    // class this corpus does not contain is **a `{{ task.type }}`
                    // on a flowable**. Recorded here and in
                    // `oracle-exec/README.md`'s unclaimed table rather than only
                    // in a completed task's report, because a finding that lives
                    // in a report is a finding that has to be rediscovered.
                    // Found by W8 Task 1's predicate sweep; not fixed by it.
                    cx.insert(
                        "task",
                        json!({
                            "id": t.id,
                            "type": match &t.kind {
                                TaskKind::Runnable { type_id } => Some(type_id.clone()),
                                _ => None,
                            },
                        }),
                    );
                }
                cx.insert(
                    "taskrun",
                    json!({
                        "id": self.ids.get(&ix).map(Uuid::to_string),
                        "startDate": self.started.get(&ix),
                        "value": r.value,
                        // **The attempts already FINISHED, which is one fewer
                        // than egret holds while the current one is open.**
                        // Measured from the golden rather than reasoned:
                        // `valids/restart-child`'s `failFirstTime` renders
                        // `{{ 1 / taskrun.attemptsCount }}` and Kestra FAILED
                        // it, and `valids/restart_last_failed`'s `c` renders
                        // `{{ taskrun.attemptsCount == 1 ? 'ok' : ko }}` and
                        // Kestra FAILED that too — `ko` is an undefined
                        // variable, so the ternary must have taken the ELSE
                        // branch, so the count was not 1. Both are only
                        // explained by 0 on the first attempt. egret opens the
                        // attempt before the handler runs, so it is one behind.
                        "attemptsCount": r.attempts.len().saturating_sub(1),
                        "parentId": r.parent,
                    }),
                );
            }
        }
        match &self.files {
            Some(files) => cx.with_files(files.clone()),
            None => cx,
        }
    }
}

/// The execution's inputs as an expression sees them: the posted values coerced
/// by their declared `type:`, plus the flow's own `defaults:` for the ones it
/// did not post.
///
/// # The defaults half
///
/// Kestra applies them before it renders anything. Leaving them out made seven
/// planned `If` conditions across five entries fail to render on flows whose
/// input was never missing — `valids/exit` posts `inputs: {}` and declares
/// `state` with `defaults: END`.
///
/// # The coercion half, and it is not only for defaults
///
/// **The declared `type:` decides how a value is typed, not whether its text
/// happens to parse as JSON.** A `BOOL` default of `true` must arrive as a
/// boolean, because `{{ inputs.failed == false }}` is in six corpus flows and
/// compares against one; a `STRING` default written `true` must stay the string
/// `"true"`.
///
/// The same rule applies to a **posted** value, and W3a's harness applied it
/// only to defaults. `valids/empty-variables` is the measurement:
/// `corpus.json` records `harvest.py` posting
/// `emptyKey: "{ \"foo\": \"\" }"` — a JSON **string** — for an input declared
/// `type: JSON`, and the flow renders `{{ inputs['emptyKey']['foo'] }}`. Kestra
/// coerces the string to an object at execution start, so the subscript
/// resolves; without the coercion it is a string and the run FAILS.
fn with_declared_defaults(posted: Value, flow: Option<&Flow>) -> Value {
    let mut inputs = match posted {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    let Some(flow) = flow else {
        return Value::Object(inputs);
    };
    for input in &flow.inputs {
        let structured = matches!(
            input.r#type,
            InputType::Int
                | InputType::Float
                | InputType::Boolean
                | InputType::Json
                | InputType::Array
                | InputType::Multiselect
        );
        match inputs.get(&input.id) {
            // Posted: coerce a STRING carrying a structured type's value into
            // that value. A posted value that is already structured is left
            // alone, and a type whose values are text is never re-read as JSON.
            Some(Value::String(raw)) if structured => {
                if let Ok(v) = serde_json::from_str::<Value>(raw) {
                    inputs.insert(input.id.clone(), v);
                }
            }
            Some(_) => {}
            // Not posted: the flow's own declared default, typed the same way.
            None => {
                let Some(default) = &input.defaults else {
                    continue;
                };
                let raw = default.source();
                let typed = if structured {
                    serde_json::from_str(raw).ok()
                } else {
                    None
                };
                inputs.insert(
                    input.id.clone(),
                    typed.unwrap_or_else(|| Value::String(raw.to_string())),
                );
            }
        }
    }
    Value::Object(inputs)
}

/// The render context a Flow trigger's `when` and `inputs` are evaluated in:
/// the **SOURCE** execution's -- Kestra's `runContextFactory.of(null,
/// execution)`, a context over the execution with no task run of its own.
///
/// It is the context this worker renders the source's own tasks against, built
/// by the same [`Surface::load`] and [`Surface::context`], with `run = None`.
/// So it carries `inputs`, `vars`, `flow`, `labels`, `execution`, `outputs`
/// and `tasks`, and it does **not** carry `trigger`: an expression reading
/// `{{ trigger.executionId }}` fails to render against it, exactly as W13a
/// Task 1 measured on the pinned artifact ("Unable to find `trigger`"). In a
/// `when` that failure refuses the fire; in `inputs` the processor (W13a Task
/// 4) still fires, and the execution is born FAILED, as Kestra's is.
///
/// **Reads without locking the execution** (`Lock::No`): the processor runs
/// in its own transaction and must not serialise behind, or block, a source
/// that is still running -- a `PAUSED` source can be resumed while its event
/// waits. Spec §4.4 point 3 states what that costs.
///
/// `Stack::Fresh`, because the processor is entered from the scheduler ticker
/// with nothing of this crate's below it; nothing `settle`-shaped runs on the
/// context this returns.
pub async fn execution_render_context(
    conn: &mut PgConnection,
    execution_id: Uuid,
) -> Result<RenderContext> {
    let (ctx, state, _now) = super::claim::load_with(
        conn,
        execution_id,
        super::claim::Lock::No,
        super::claim::Stack::Fresh,
    )
    .await?;
    let surface = Surface::load(conn, &ctx).await?;
    Ok(surface.context(&ctx.plan, &state, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(src: &str) -> Flow {
        crate::spec::parse(src).expect("the probe flow parses")
    }

    /// **An input's declared `type:` decides how its value is typed — both a
    /// posted one and a default — and not whether the text happens to parse as
    /// JSON.**
    ///
    /// Four cases in one assertion, because each is the control for the others:
    /// a `STRING` whose text looks like a bool must stay a string, and a `BOOL`
    /// whose text is the same must become one. Zero corpus flows are the first
    /// shape today, which is exactly why it needs a test rather than an entry —
    /// without it the type gate is a rule nothing can see fail.
    #[test]
    fn an_inputs_declared_type_decides_how_its_value_is_typed() {
        let f = flow(
            "id: x\nnamespace: t\ninputs:\n\
             \x20 - id: s\n    type: STRING\n    defaults: true\n\
             \x20 - id: b\n    type: BOOL\n    defaults: true\n\
             \x20 - id: j\n    type: JSON\n\
             \x20 - id: t\n    type: STRING\ntasks:\n\
             \x20 - id: t1\n    type: io.kestra.plugin.core.debug.Return\n    format: x\n",
        );
        let got = with_declared_defaults(
            serde_json::json!({"j": "{\"foo\": \"\"}", "t": "{\"foo\": 1}"}),
            Some(&f),
        );
        assert_eq!(
            got,
            serde_json::json!({
                "s": "true",
                "b": true,
                "j": {"foo": ""},
                "t": "{\"foo\": 1}",
            }),
            "`s` keeps its text because it is declared STRING; `b` becomes a \
             boolean because it is declared BOOL; the POSTED `j` is parsed \
             because it is declared JSON -- which is what `valids/empty-variables` \
             needs and what W3a's harness never did -- and the posted `t` is left \
             alone because a STRING is text whatever it looks like"
        );
    }

    /// **A task that has not TERMINATED is in neither `outputs` nor `tasks`.**
    ///
    /// This is an over-acceptance guard — an expression that could read a
    /// running task's output would render a value Kestra could not have
    /// produced — and **no corpus entry exercises it**: the mutation that
    /// deletes the filter (`if false && !last.is_terminal()`) kills nothing
    /// across the whole crate. That is the shape this programme has shipped
    /// eight times, so the guard gets a driver of its own rather than a comment.
    ///
    /// Both directions, because only the pair distinguishes the filter from
    /// "answer nothing": the terminated task IS present, and the running one is
    /// not.
    #[test]
    fn a_task_that_has_not_terminated_is_in_neither_outputs_nor_tasks() {
        use crate::executor::{build_plan, Attempt, ExecutionState, TaskRun};
        let source = "id: x\nnamespace: t\ntasks:\n\
             \x20 - id: done\n    type: io.kestra.plugin.core.log.Log\n    message: a\n\
             \x20 - id: running\n    type: io.kestra.plugin.core.log.Log\n    message: b\n";
        let plan = build_plan(source).expect("the probe flow plans");
        let run = |task: usize, state: RunState| TaskRun {
            task,
            parent: None,
            value: None,
            rendered: None,
            rendered_run_if: None,
            history: vec![RunState::Created, state],
            attempts: Vec::<Attempt>::new(),
            not_before: None,
            resume_with: None,
        };
        let state = ExecutionState {
            runs: vec![run(0, RunState::Success), run(1, RunState::Running)],
            history: vec![RunState::Running],
            metadata: crate::executor::ExecutionMeta::default(),
        };
        let surface = Surface {
            // This fixture exercises the OUTPUT context only; nothing it drives
            // reads the flow.
            parsed: None,
            inputs: json!({}),
            vars: Value::Null,
            flow: Value::Null,
            labels: json!({}),
            execution: json!({}),
            item: None,
            inherited: Map::new(),
            own: std::collections::BTreeMap::new(),
            started: std::collections::BTreeMap::new(),
            ids: std::collections::BTreeMap::new(),
            files: None,
        };
        let cx = surface.context(&plan, &state, None);
        let read = |name: &str| {
            cx.bindings()
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| format!("{v:?}"))
                .unwrap_or_default()
        };
        assert!(
            read("outputs").contains("done") && read("tasks").contains("done"),
            "the TERMINATED task is present -- without this half the assertion \
             below would pass against a context that answered nothing: {:?}",
            read("outputs")
        );
        assert!(
            !read("outputs").contains("running") && !read("tasks").contains("running"),
            "and the RUNNING one is absent: it has no output yet, and an \
             expression that could read one would render a value Kestra could \
             not have produced -- got {:?} / {:?}",
            read("outputs"),
            read("tasks")
        );
    }

    /// A flow with no `inputs:` gets what it was posted, unchanged — the
    /// control for the case above, which would otherwise pass against a
    /// function that parsed everything.
    #[test]
    fn a_flow_that_declares_no_inputs_gets_what_it_was_posted() {
        let f = flow(
            "id: x\nnamespace: t\ntasks:\n\
             \x20 - id: t1\n    type: io.kestra.plugin.core.debug.Return\n    format: x\n",
        );
        assert_eq!(
            with_declared_defaults(serde_json::json!({"k": "{\"a\":1}"}), Some(&f)),
            serde_json::json!({"k": "{\"a\":1}"})
        );
    }
}

/// What one render pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Pass {
    /// Nothing was left to render at this state.
    Nothing,
    /// At least one slot was filled, or at least one run was failed by a render
    /// that would not resolve. The caller must ask `advance()` again.
    Rendered,
}

/// **Tier A — `runIf:`, and it goes FIRST.**
///
/// Kestra decides a `runIf:` before it resolves anything else about the task,
/// and the order is observable: a `Loop` whose `runIf:` is falsy is `SKIPPED`
/// without its `values:` ever being resolved, so a tier that rendered both at
/// once could fail a run on an expression Kestra never evaluated.
///
/// A `runIf:` that will not render **fails the run, with no attempt** —
/// `valids/task-runif`'s `willfailedtheflow` is `["CREATED","FAILED"]` with
/// `attempts: []` in the golden, which is a different shape from a skip
/// (`["CREATED","SKIPPED"]`, one attempt).
pub(super) fn render_run_if(surface: &Surface, plan: &Plan, state: &mut ExecutionState) -> Pass {
    let pending: Vec<RunIx> = (0..state.runs.len())
        .filter(|&ix| {
            plan.tasks
                .get(state.runs[ix].task)
                .and_then(|t| t.run_if.as_deref())
                .is_some()
                && state.runs[ix].history.last() == Some(&RunState::Created)
                && state.runs[ix].rendered_run_if.is_none()
        })
        .collect();
    if pending.is_empty() {
        return Pass::Nothing;
    }
    for ix in pending {
        let expression = plan.tasks[state.runs[ix].task]
            .run_if
            .clone()
            .expect("filtered on Some above");
        let cx = surface.context(plan, state, Some(ix));
        match crate::render_str(&expression, &cx) {
            Ok(value) => state.runs[ix].rendered_run_if = Some(value),
            Err(_) => fail(state, ix),
        }
    }
    Pass::Rendered
}

/// What a render pass produced beside the state, for `settle` to write in
/// the same transaction after `persist` (W9a Task 11).
#[derive(Debug, Default)]
pub(super) struct RenderSide {
    /// Each run a render failed WITH a message: a `Loop` whose values file
    /// cannot be iterated. Every other render failure fails its run without
    /// one.
    pub(super) failures: Vec<(RunIx, String)>,
    /// Each `Loop` run whose values were read from a file, with those values
    /// in file order: written to `workflow.loop_file_values`, not to the
    /// run's `rendered` (fix round 1; see `executor::rendered_from_file`).
    pub(super) file_values: Vec<(RunIx, Vec<String>)>,
}

/// **Tier B — an `If`'s condition, a `Switch`'s value, a `Loop`'s `values:`, a
/// `Pause`'s `pauseDuration:` and — W8 Task 4 — a `LoopUntil`'s `condition:`.**
///
/// Each is read at its own run state, and that is measured rather than chosen:
/// `CREATED` for a `Loop`, `RUNNING` for the rest. `valids/loop-empty`'s
/// `1_each` reads `CREATED SUCCESS` with no `RUNNING` and no attempt and
/// `valids/loop-null`'s reads `CREATED FAILED`, so on this artifact a `Loop`
/// resolves its values *before* it runs; an `If` is started and then asked.
///
/// **A `LoopUntil` is the one kind whose state is not enough**, and the one
/// asked MANY times for a single run. It is `RUNNING` for the whole of its
/// body, so the state alone would evaluate its `condition:` before the children
/// whose `outputs.<child>.…` it names have run; the third gate below is the
/// executor's own `loop_until::condition_is_asked`, called rather than copied.
/// And `Command::RestartLoop` clears `rendered` at every iteration, so the
/// filter re-arms — which is Kestra's `runContext.render(this.condition)
/// .skipCache()`, and is what stops iteration `k + 1` being decided by
/// iteration `k`'s outputs.
///
/// A render that fails FAILS the run — `valids/if-condition-fail`'s condition
/// names an input the flow does not declare and its golden is one run,
/// `CREATED RUNNING FAILED`.
pub(super) fn render_expressions(
    surface: &Surface,
    plan: &Plan,
    state: &mut ExecutionState,
    now: Tick,
    side: &mut RenderSide,
) -> Pass {
    let pending: Vec<RunIx> = (0..state.runs.len())
        .filter(|&ix| {
            expression_of(plan, state.runs[ix].task).is_some()
                && state.runs[ix].history.last() == Some(&renders_at(plan, state.runs[ix].task))
                && state.runs[ix].rendered.is_none()
                // **A `LoopUntil` has a third gate and the other four kinds do
                // not** -- W8 Task 4. `RUNNING` is when an `If` is asked, and a
                // `LoopUntil` is `RUNNING` for its whole body as well, so the
                // state alone would evaluate the condition before the children
                // that its `outputs.<child>.…` names have run. The predicate is
                // the executor's own, not a copy: see
                // `executor::flowable::loop_until::condition_is_asked` for why
                // rendering early is a terminal the artifact does not produce
                // rather than a harmless extra evaluation.
                && (!matches!(
                    plan.tasks[state.runs[ix].task].kind,
                    crate::executor::TaskKind::LoopUntil { .. }
                ) || crate::executor::loop_condition_is_asked(
                    plan,
                    state,
                    ix,
                    state.runs[ix].task,
                    now,
                ))
        })
        .collect();
    if pending.is_empty() {
        return Pass::Nothing;
    }
    for ix in pending {
        let expression = expression_of(plan, state.runs[ix].task).expect("filtered on Some above");
        let cx = surface.context(plan, state, Some(ix));
        match crate::render_str(&expression, &cx) {
            // W9a Task 11 (spec §9.3): a `Loop` whose `values:` renders to a
            // file URI iterates the FILE. Kestra's test is
            // `URIFetcher.supports` -- the rendered text starts with one of
            // its three schemes -- and its `Loop.initFromUri` reads the file
            // before any iteration exists. Here the worker reads it: the run's
            // `rendered` gets the URI and the count, `advance()` spawns from
            // the count, and the values go to `settle` (`RenderSide`), which
            // writes them where the spawner reads them by index.
            Ok(value)
                if matches!(plan.tasks[state.runs[ix].task].kind, TaskKind::Loop { .. })
                    && crate::executor::values_name_a_file(&value) =>
            {
                match surface.loop_values_from_file(&value) {
                    Ok(values) => {
                        state.runs[ix].rendered =
                            Some(crate::executor::rendered_from_file(&value, values.len()));
                        side.file_values.push((ix, values));
                    }
                    Err(why) => {
                        fail(state, ix);
                        side.failures.push((ix, why));
                    }
                }
            }
            Ok(value) => state.runs[ix].rendered = Some(value),
            Err(_) => fail(state, ix),
        }
    }
    Pass::Rendered
}

/// What a `Loop`'s reading of its values file says when it is not whole:
/// the end of `Snapshot::read_bytes`'s over-cap message.
const LOOP_READS_WHOLE: &str = "a Loop reads its values file whole before its first iteration";

/// A render that will not resolve FAILS the run.
///
/// The endpoint bookkeeping is NOT done here: [`settle`](super::claim::settle)
/// observes the whole state before and after each pass, so a run failed by a
/// render gets its `ended_seq` from the same code path as a run failed by
/// `advance()`. A second place that stamped endpoints would be a second table
/// that can disagree with the one it copies.
fn fail(state: &mut ExecutionState, ix: RunIx) {
    crate::executor::apply(
        state,
        &crate::executor::Command::SetRunState {
            run: ix,
            state: RunState::Failed,
        },
    );
}

/// The unrendered expression a planned task holds, by kind.
///
/// Five kinds hold one: an `If`'s `condition`, a `Switch`'s `value`, a `Loop`'s
/// `values`, a `Pause`'s `pauseDuration:` (W8 Task 3) and a `LoopUntil`'s
/// `condition:` (W8 Task 4).
///
/// **The `Pause` arm is narrower than the other four**, and deliberately: it
/// offers the expression only when the source does NOT already parse as an
/// ISO-8601 literal. Every other kind's expression has to be rendered whatever
/// it says, but `pauseDuration: PT1S` is a complete answer on its own, and
/// `executor::flowable::pause` reads it without waiting for a render. Putting a
/// literal `Pause` into this population would cost nothing on the oracle (the
/// render is the identity for a source with no `{`) and would make the run's
/// `rendered` slot non-`None` for 11 of the 12 baselined `Pause` entries, which
/// is a stored field nothing reads and something can later disagree with.
fn expression_of(plan: &Plan, task: usize) -> Option<String> {
    match &plan.tasks.get(task)?.kind {
        TaskKind::If { condition, .. } => Some(condition.clone()),
        // W8 Task 4. Unlike the other four this one is asked MANY times for one
        // run -- once per iteration -- which is what
        // `Command::RestartLoop` clearing `rendered` arranges, and what Kestra
        // spells `runContext.render(this.condition).skipCache()`.
        TaskKind::LoopUntil { condition, .. } => Some(condition.clone()),
        TaskKind::Switch { value, .. } => Some(value.clone()),
        TaskKind::Loop { values, .. } => Some(values.clone()),
        TaskKind::Pause {
            duration: Some(source),
        } if crate::executor::parse_duration(source).is_none() => Some(source.clone()),
        _ => None,
    }
}

/// The run state at which this task's expression is rendered — see
/// [`render_expressions`] for the measurement.
///
/// A `Pause` takes the `RUNNING` default with the `If` and the `Switch`, and
/// that is forced rather than chosen: its `pauseDuration:` is measured from
/// [`TaskRun::attempt_started_at`](crate::executor::TaskRun::attempt_started_at),
/// so the attempt has to be open before the answer is worth anything.
fn renders_at(plan: &Plan, task: usize) -> RunState {
    match plan.tasks.get(task).map(|t| &t.kind) {
        Some(TaskKind::Loop { .. }) => RunState::Created,
        _ => RunState::Running,
    }
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!("{e}"))
}
