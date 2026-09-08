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
use crate::executor::{ExecutionState, Plan, RunIx, RunState, TaskKind};
use crate::models::{Flow, InputType};
use crate::runners::RenderContext;

/// Everything a render needs that lives outside `ExecutionState`, read once
/// per [`settle`](super::claim::settle).
///
/// It is one async read rather than one per expression because none of it can
/// change while `settle` holds the execution's row lock: a handler's outputs
/// are written before `settle` is entered, and no handler runs inside it.
#[derive(Debug, Clone)]
pub(super) struct Surface {
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
}

impl Surface {
    /// Read everything the renders of one `settle` will need.
    pub(super) async fn load(conn: &mut PgConnection, ctx: &ExecutionCtx) -> Result<Self> {
        let row = sqlx::query(
            "SELECT inputs, state, created_at, loop_index, loop_value, \
                    parent_execution_id, parent_run_ix \
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
        // the corpus reads. Kestra namespaces system labels under `system`, and
        // `correlationId` is the id of the execution at the root of the parent
        // chain — a sub-execution shares its parent's. Measured from the two
        // entries that read it (`valids/follow-dependencies-partial-child` and
        // `valids/subflow-grand-child`, both `{{ labels.system.correlationId }}`
        // in a `Log`, both SUCCESS in the golden). No other system label is
        // supplied, so an expression over one still fails.
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
        let mut here: (Option<i32>, Option<String>, Option<Uuid>, Option<i32>) = (
            row.get("loop_index"),
            row.get("loop_value"),
            row.get("parent_execution_id"),
            row.get("parent_run_ix"),
        );
        let mut ancestor: Option<Uuid> = here.2;
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
        // **The `levels` walk below is NOT sticky, and W6 Task 4 measured that
        // it cannot reach a non-loop link at all.** A level is pushed only when
        // the row below the link carries `(loop_index, loop_value,
        // parent_run_ix)`, so a replay contributes none of its own; the only
        // way one could cross a replay link is a replay whose PARENT is a loop
        // iteration, and `worker::replay::spawn_due` refuses to create that row
        // -- because the artifact refuses it too, throwing
        // `Can't find taskrun with taskrunId <the LOOP run's>` out of
        // `markWithTaskRunAs(..., markParents = true)` and ending the iteration
        // FAILED with no child. So the state this walk is unguarded against is
        // one no writer can produce, which is why it is left as it is rather
        // than given a second flag nothing could make fire.
        //
        // **`root` deliberately keeps walking the whole chain, and W6 Task 4
        // measured that this is right rather than merely unmeasured.** It feeds
        // `labels.system.correlationId`, and on the pinned artifact
        // (`egret.probe4/replay-correlation`: a flow-level
        // `CREATE_NEW_EXECUTION` retry over a `Log` of
        // `{{ labels.system.correlationId }}` and a `Fail`) the replay's label
        // is the ORIGINAL execution's id while `{{ execution.id }}` is its own:
        //
        //     root  cid=3nY5p1H9MmhpWnq4hZsVOD eid=3nY5p1H9MmhpWnq4hZsVOD
        //     replay cid=3nY5p1H9MmhpWnq4hZsVOD eid=5cNdx41IqqhyGSExZsuGLu
        //
        // The mechanism agrees: `ExecutionService.replay` copies the parent's
        // labels (adding `system.replay`), and `Execution.java:213` fills a
        // `correlationId` in only when one is ABSENT. So a replay inherits, a
        // sticky `root` would not, and stopping this walk at the same link
        // `inheriting` stops at would be the divergence.
        //
        // No golden entry pairs a replay with a `correlationId` read, so the
        // oracle cannot tell either way; the probe is the instrument, and
        // `a_replay_inherits_the_correlation_id_of_the_execution_it_replaces`
        // in `tests/worker.rs` is what keeps it from drifting.
        let mut inheriting = true;
        // Nearest ancestor first, so a nearer scope wins where two ancestors
        // ran a task of the same id. The chain is a DAG by construction --
        // `parent_execution_id` always names a row that existed before this one
        // -- which is why there is no depth bound, for the reason
        // `flowable::descend` gives for not having one either.
        while let Some(id) = ancestor {
            // This level's `item`, built while the parent that holds its
            // resolved `values:` is the row being read.
            if let (Some(index), Some(value), Some(run_ix)) = (here.0, here.1.clone(), here.3) {
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
                    "key": rendered
                        .as_deref()
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
            // `here.0` is the loop index of the row BELOW `id`, i.e. of the
            // link about to be traversed.
            inheriting = inheriting && here.0.is_some();
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
                "SELECT loop_index, loop_value, parent_execution_id, parent_run_ix \
                   FROM workflow.executions WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&mut *conn)
            .await
            .map_err(db_err)?;
            here = (
                up.get("loop_index"),
                up.get("loop_value"),
                up.get("parent_execution_id"),
                up.get("parent_run_ix"),
            );
            ancestor = here.2;
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
        })
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
            if !last.is_terminal() {
                continue;
            }
            let Some(task) = plan.tasks.get(r.task) else {
                continue;
            };
            // `None` becomes `{}`: measured on the artifact, an expression
            // cannot tell "no Output object" from an empty one — see the module
            // doc's table.
            outputs.insert(
                task.id.clone(),
                self.own.get(&ix).cloned().unwrap_or_else(|| json!({})),
            );
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
        cx
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
        };
        let state = ExecutionState {
            runs: vec![run(0, RunState::Success), run(1, RunState::Running)],
            history: vec![RunState::Running],
            metadata: crate::executor::ExecutionMeta::default(),
        };
        let surface = Surface {
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

/// **Tier B — an `If`'s condition, a `Switch`'s value, a `Loop`'s `values:`.**
///
/// Each is read at its own run state, and that is measured rather than chosen:
/// `CREATED` for a `Loop`, `RUNNING` for the other two. `valids/loop-empty`'s
/// `1_each` reads `CREATED SUCCESS` with no `RUNNING` and no attempt and
/// `valids/loop-null`'s reads `CREATED FAILED`, so on this artifact a `Loop`
/// resolves its values *before* it runs; an `If` is started and then asked.
///
/// A render that fails FAILS the run — `valids/if-condition-fail`'s condition
/// names an input the flow does not declare and its golden is one run,
/// `CREATED RUNNING FAILED`.
pub(super) fn render_expressions(
    surface: &Surface,
    plan: &Plan,
    state: &mut ExecutionState,
) -> Pass {
    let pending: Vec<RunIx> = (0..state.runs.len())
        .filter(|&ix| {
            expression_of(plan, state.runs[ix].task).is_some()
                && state.runs[ix].history.last() == Some(&renders_at(plan, state.runs[ix].task))
                && state.runs[ix].rendered.is_none()
        })
        .collect();
    if pending.is_empty() {
        return Pass::Nothing;
    }
    for ix in pending {
        let expression = expression_of(plan, state.runs[ix].task).expect("filtered on Some above");
        let cx = surface.context(plan, state, Some(ix));
        match crate::render_str(&expression, &cx) {
            Ok(value) => state.runs[ix].rendered = Some(value),
            Err(_) => fail(state, ix),
        }
    }
    Pass::Rendered
}

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
/// Three kinds hold one: an `If`'s `condition`, a `Switch`'s `value` and a
/// `Loop`'s `values`.
fn expression_of(plan: &Plan, task: usize) -> Option<String> {
    match &plan.tasks.get(task)?.kind {
        TaskKind::If { condition, .. } => Some(condition.clone()),
        TaskKind::Switch { value, .. } => Some(value.clone()),
        TaskKind::Loop { values, .. } => Some(values.clone()),
        _ => None,
    }
}

/// The run state at which this task's expression is rendered — see
/// [`render_expressions`] for the measurement.
fn renders_at(plan: &Plan, task: usize) -> RunState {
    match plan.tasks.get(task).map(|t| &t.kind) {
        Some(TaskKind::Loop { .. }) => RunState::Created,
        _ => RunState::Running,
    }
}

fn db_err(e: sqlx::Error) -> egret_core::Error {
    egret_core::Error::Internal(anyhow::anyhow!("{e}"))
}
