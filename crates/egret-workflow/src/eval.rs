//! Evaluate one Pebble expression against a STORED execution — package 7e
//! (#1a execution context, #1b task-run context).
//!
//! The worker's live context assembler is [`crate::worker`]'s `Surface::context`
//! (`worker/render.rs`): it inserts `inputs`, `vars`, `flow`, `labels`,
//! `execution`, `outputs`, `tasks` and, for a run, `task` and `taskrun`. This
//! module reproduces the SAME top-level names and shapes, but **from the stored
//! [`ExecutionDetail`]** (its `execution`/`variables`/`outputs`/`runs`) rather
//! than from a live `ExecutionState`/`Surface`.
//!
//! # Secrets are sealed, by construction, on both paths
//!
//! The decision (stricter than Kestra) is that `{{ secret('X') }}` must NOT
//! resolve here, on either the execution or the task-run path. That is achieved
//! by what this module does NOT do:
//!
//! - it never decrypts [`ExecutionDetail::secret_inputs`], so a SECRET input is
//!   only ever the stored [`MASK`] in `inputs`; and
//! - it never attaches a resolving `secret()`/`s()` (the worker's
//!   `runners::secrets::render_with_secrets`), so [`crate::render_str`]'s base
//!   environment keeps its refusing `secret` stub and `{{ secret('X') }}`
//!   returns through the `error` field, not as a value.
//!
//! As a second, belt-and-suspenders measure the mask marker itself is added to
//! the context's redactions and applied to a successful render, so a masked
//! value cannot leave this module unmasked.

use serde::Serialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::repo::ExecutionDetail;
use crate::secret_inputs::MASK;
use crate::RenderContext;

/// What `POST .../eval` answers: exactly one of `result` (a successful render)
/// or `error` (the render's message) is set. Both routes answer **200** even
/// when `error` is set — a broken expression is a result, not a request error.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EvalResultDto {
    /// The rendered string, when the expression rendered.
    pub result: Option<String>,
    /// The render error's message, when it did not. A SECRET expression
    /// (`{{ secret('X') }}`) arrives here, never as a `result`.
    pub error: Option<String>,
}

/// Why [`stored_context`] could not build a context. The route maps it to 404.
#[derive(Debug)]
pub enum EvalError {
    /// No task run with this id belongs to the execution.
    UnknownTaskRun(Uuid),
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::UnknownTaskRun(id) => {
                write!(f, "no task run {id} in this execution")
            }
        }
    }
}

impl std::error::Error for EvalError {}

/// Build the render context one expression evaluates against, from the stored
/// [`ExecutionDetail`].
///
/// `task_run_id` is `None` for the execution context (#1a) and `Some` for a
/// task-run context (#1b); an id that names no run of this execution is an
/// [`EvalError::UnknownTaskRun`], which the route turns into a 404.
///
/// The names and shapes mirror `Surface::context` (`worker/render.rs`):
/// `inputs`, `vars`, `flow` (`{id, namespace}`), `labels` (the visible labels
/// flat, plus `system.correlationId`), `execution`
/// (`{id, startDate, state, outputs}`), `outputs` (`{taskId: outputObj}` for the
/// terminal runs), `tasks` (`{taskId: {state}}` for the terminal runs) and,
/// with a run, `task` (`{id, type}`) and `taskrun`
/// (`{id, startDate, attemptsCount, parentId?, value?}`).
pub fn stored_context(
    detail: &ExecutionDetail,
    task_run_id: Option<Uuid>,
) -> Result<RenderContext, EvalError> {
    // Resolve the named run FIRST, so an unknown id is an error before any
    // other work and never a silently-empty `taskrun`.
    let run = match task_run_id {
        Some(id) => Some(
            detail
                .runs
                .iter()
                .find(|r| r.id == id)
                .ok_or(EvalError::UnknownTaskRun(id))?,
        ),
        None => None,
    };

    // The frozen source, parsed; tolerated as `None`, exactly as the worker
    // tolerates it (`crate::spec::parse(..).ok()`).
    let flow = crate::spec::parse(&detail.source).ok();
    // Built only for `task.type` below — the stored run carries `type_id`, which
    // is `NULL` for a flowable, where the plan's `type_name` is the spelling.
    let plan = crate::executor::build_plan(&detail.source).ok();

    let mut cx = RenderContext::new();

    // `inputs`: the execution's stored, already-masked inputs (a SECRET value
    // is the mask; the plaintext lives sealed in `secret_inputs`, unread here).
    cx.insert("inputs", detail.execution.inputs.clone());
    // Seal, second measure: the mask marker is the one redaction, so a masked
    // value stays masked in a successful render (see `eval_expression`).
    cx.redact(&[MASK.to_string()]);

    // `vars`: the execution's own `variables` (group C2), else the flow's.
    let vars = match &detail.variables {
        Some(v) => v.clone(),
        None => flow
            .as_ref()
            .and_then(|f| serde_json::to_value(&f.variables).ok())
            .unwrap_or(Value::Null),
    };
    cx.insert("vars", vars);

    // `flow`: `{id, namespace}`, or `Null` when the source would not parse.
    let flow_value = flow
        .as_ref()
        .map(|f| json!({"id": f.id, "namespace": f.namespace}))
        .unwrap_or(Value::Null);
    cx.insert("flow", flow_value);

    // `labels`: the visible labels as a flat map, plus the one system label the
    // corpus reads (`labels_value` in `worker/render.rs`). `correlationId` is a
    // best-effort here: with no causal-chain walk in `stored_context`, it is the
    // execution's own id — exact for a root execution, which is Kestra's value
    // when the label is absent.
    let own = crate::labels::own_labels(detail.execution.labels.as_ref());
    let flow_labels = flow.as_ref().map(|f| f.labels.as_slice()).unwrap_or(&[]);
    let visible =
        crate::labels::visible_labels(flow_labels, &own, detail.execution.labels_replaced);
    let mut labels = Map::new();
    for label in &visible {
        labels.insert(label.key.clone(), Value::String(label.value.clone()));
    }
    labels.insert(
        "system".to_string(),
        json!({ "correlationId": detail.execution.id.to_string() }),
    );
    cx.insert("labels", Value::Object(labels));

    // `execution`: `{id, startDate, state, outputs}`. `startDate` is the
    // execution's `created_at`, as `Surface` uses, and `outputs` is the stored
    // (masked) flow outputs, `{}` when none were stored.
    cx.insert(
        "execution",
        json!({
            "id": detail.execution.id.to_string(),
            "startDate": detail.execution.created_at.to_rfc3339(),
            "state": detail.execution.state,
            "outputs": detail.outputs.clone().unwrap_or_else(|| json!({})),
        }),
    );

    // `outputs` (`{taskId: outputObj}`) and `tasks` (`{taskId: {state}}`), from
    // the TERMINAL stored runs only — the same partition `Surface::context`
    // applies (`if !last.is_terminal() { continue }`). A run's stored `outputs`
    // is already masked; `None` becomes `{}`, as it does live.
    let mut outputs = Map::new();
    let mut tasks = Map::new();
    for r in &detail.runs {
        let terminal = crate::worker::from_wire(&r.state).is_some_and(|s| s.is_terminal());
        if !terminal {
            continue;
        }
        outputs.insert(
            r.task_id.clone(),
            r.outputs.clone().unwrap_or_else(|| json!({})),
        );
        tasks.insert(r.task_id.clone(), json!({ "state": r.state }));
    }
    cx.insert("outputs", Value::Object(outputs));
    cx.insert("tasks", Value::Object(tasks));

    // `task` and `taskrun` when a run was named, shaped as `Surface::context`
    // shapes them from `RunVariables.of(Task)`/`of(TaskRun)`.
    if let Some(r) = run {
        // `task.type` is the type AS WRITTEN for every kind — the plan's
        // `type_name`; the stored `type_id` (`NULL` for a flowable) is the
        // fallback when the source would not build a plan.
        let type_value = plan
            .as_ref()
            .and_then(|p| p.tasks.get(r.task_ix.max(0) as usize))
            .map(|t| t.type_name.clone())
            .or_else(|| r.type_id.clone());
        cx.insert("task", json!({ "id": r.task_id, "type": type_value }));

        // `id`, `startDate` and `attemptsCount` always; `parentId` and `value`
        // ONLY when non-null. `attemptsCount` is the attempts already FINISHED,
        // one fewer than the stored array holds while the current one is open.
        let attempts = r.attempts.as_array().map_or(0, |a| a.len());
        let mut taskrun = json!({
            "id": r.id.to_string(),
            "startDate": r.created_at.to_rfc3339(),
            "attemptsCount": attempts.saturating_sub(1),
        });
        let fields = taskrun.as_object_mut().expect("built as an object");
        if let Some(parent_ix) = r.parent_ix {
            // The parent's task-run id, by its `run_ix`; `null` when the parent
            // row is absent, exactly as the live `self.ids.get(&parent)` reads.
            let parent_id = detail
                .runs
                .iter()
                .find(|p| p.run_ix == parent_ix)
                .map(|p| p.id.to_string());
            fields.insert("parentId".to_string(), json!(parent_id));
        }
        if let Some(value) = &r.value {
            fields.insert("value".to_string(), json!(value));
        }
        // `iteration` is deliberately NOT reproduced: it is a `LoopUntil`-only
        // field the worker derives from live `ExecutionState`, which a stored
        // read does not carry.
        cx.insert("taskrun", taskrun);
    }

    Ok(cx)
}

/// Render `expr` against the stored context for `detail` (and `task_run_id`),
/// into the wire DTO. A render error — including a refused `secret()` — is the
/// `error` field, never a thrown error, so the route can answer 200.
pub fn eval_expression(
    detail: &ExecutionDetail,
    task_run_id: Option<Uuid>,
    expr: &str,
) -> EvalResultDto {
    let cx = match stored_context(detail, task_run_id) {
        Ok(cx) => cx,
        Err(e) => {
            return EvalResultDto {
                result: None,
                error: Some(e.to_string()),
            }
        }
    };
    match crate::render_str(expr, &cx) {
        // Belt-and-suspenders: mask the mask marker in the output too, so a
        // masked value that reached the string cannot leave unmasked.
        Ok(text) => EvalResultDto {
            result: Some(crate::logs::mask(&text, cx.redactions()).into_owned()),
            error: None,
        },
        Err(e) => EvalResultDto {
            result: None,
            error: Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::{ExecutionRow, TaskRunRow};
    use chrono::{DateTime, TimeZone, Utc};

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    /// A flow with one SECRET input and one Log task, so a plan builds.
    const SRC: &str = "id: s\nnamespace: a.b\ninputs:\n  - id: pw\n    type: SECRET\ntasks:\n  - id: t\n    type: io.kestra.plugin.core.log.Log\n    message: hi\n";

    fn exec_row(id: Uuid, inputs: Value) -> ExecutionRow {
        ExecutionRow {
            id,
            workspace_id: Uuid::new_v4(),
            flow_id: None,
            namespace: "a.b".into(),
            flow_key: "s".into(),
            state: "SUCCESS".into(),
            history: json!(["CREATED", "RUNNING", "SUCCESS"]),
            inputs,
            seq: 0,
            execution_trigger: None,
            trigger_execution_id: None,
            trigger_run_ix: None,
            parent_execution_id: None,
            parent_run_ix: None,
            loop_index: None,
            loop_value: None,
            attempt_number: 0,
            origin_offset_ms: 0,
            replayed_from_execution_id: None,
            started_at: None,
            ended_at: None,
            created_at: ts(),
            updated_at: ts(),
            labels: None,
            labels_replaced: false,
            backfill_id: None,
        }
    }

    fn run_row(execution_id: Uuid, run_ix: i32, task_id: &str) -> TaskRunRow {
        TaskRunRow {
            id: Uuid::new_v4(),
            execution_id,
            run_ix,
            task_ix: 0,
            task_id: task_id.into(),
            type_id: Some("io.kestra.plugin.core.log.Log".into()),
            parent_ix: None,
            value: None,
            rendered: None,
            rendered_run_if: None,
            state: "SUCCESS".into(),
            history: json!(["CREATED", "RUNNING", "SUCCESS"]),
            attempts: json!([{"state": "SUCCESS"}]),
            not_before_ms: None,
            claimed_at: None,
            outputs: Some(json!({"value": "ok"})),
            message: None,
            created_seq: None,
            ended_seq: None,
            resume_with: None,
            started_at: Some(ts()),
            ended_at: Some(ts()),
            created_at: ts(),
            updated_at: ts(),
        }
    }

    fn detail(id: Uuid, source: &str, inputs: Value, runs: Vec<TaskRunRow>) -> ExecutionDetail {
        ExecutionDetail {
            execution: exec_row(id, inputs),
            source: source.into(),
            runs,
            outputs: None,
            variables: None,
            secret_inputs: None,
        }
    }

    #[test]
    fn a_literal_renders() {
        let d = detail(Uuid::new_v4(), SRC, json!({}), vec![]);
        let r = eval_expression(&d, None, "{{ 1 + 1 }}");
        assert_eq!(r.result.as_deref(), Some("2"));
        assert!(r.error.is_none(), "{r:?}");
    }

    #[test]
    fn the_execution_context_binds() {
        let id = Uuid::new_v4();
        let d = detail(id, SRC, json!({}), vec![]);
        let r = eval_expression(&d, None, "{{ execution.id }}");
        assert_eq!(r.result.as_deref(), Some(id.to_string().as_str()));
        assert!(r.error.is_none(), "{r:?}");
    }

    #[test]
    fn a_broken_expression_is_an_error_not_a_value() {
        let d = detail(Uuid::new_v4(), SRC, json!({}), vec![]);
        let r = eval_expression(&d, None, "{{ nope.nope }}");
        assert!(r.result.is_none(), "{r:?}");
        assert!(r.error.is_some(), "{r:?}");
    }

    #[test]
    fn a_masked_secret_input_renders_the_mask_never_the_plaintext() {
        let d = detail(Uuid::new_v4(), SRC, json!({ "pw": MASK }), vec![]);
        let r = eval_expression(&d, None, "{{ inputs.pw }}");
        assert_eq!(r.result.as_deref(), Some(MASK));
        assert!(r.error.is_none(), "{r:?}");
    }

    #[test]
    fn secret_is_sealed_on_both_the_execution_and_the_task_run_path() {
        // A distinctive plaintext that is REALLY sealed into `secret_inputs`, so
        // this test fails if the code ever decrypts it: `{{ inputs.pw }}` would
        // then render the sentinel instead of the mask. Masking alone — the old
        // version of this test — would pass even against a decrypting bug.
        const SENTINEL: &str = "SENTINEL-PLAINTEXT-d3adb33f";
        crate::secret_inputs::install_for_tests();
        let flow = crate::spec::parse(SRC).expect("parse");
        let (masked, sealed) =
            crate::secret_inputs::split(&flow, json!({ "pw": SENTINEL })).expect("seal");
        assert_eq!(masked["pw"], json!(MASK), "split masks the input");
        assert!(sealed.is_some(), "split seals the plaintext");

        let id = Uuid::new_v4();
        let run = run_row(id, 0, "t");
        let trid = run.id;
        let d = ExecutionDetail {
            execution: exec_row(id, masked),
            source: SRC.into(),
            runs: vec![run],
            outputs: None,
            variables: None,
            secret_inputs: sealed,
        };

        // #1a (None) and #1b (Some(trid)): the sealed plaintext surfaces in NO
        // rendered surface, and `secret()` is an error while the masked input is
        // the mask.
        for task_run in [None, Some(trid)] {
            for expr in ["{{ secret('pw') }}", "{{ inputs.pw }}", "{{ inputs }}", "{{ outputs }}"] {
                let r = eval_expression(&d, task_run, expr);
                let seen = format!("{r:?}");
                assert!(!seen.contains(SENTINEL), "{expr} leaked the sentinel: {seen}");
            }
            let secret = eval_expression(&d, task_run, "{{ secret('pw') }}");
            assert!(secret.result.is_none(), "secret() leaked a value: {secret:?}");
            assert!(secret.error.is_some(), "{secret:?}");
            let input = eval_expression(&d, task_run, "{{ inputs.pw }}");
            assert_eq!(input.result.as_deref(), Some(MASK), "{input:?}");
        }
    }

    #[test]
    fn the_task_run_context_binds() {
        let id = Uuid::new_v4();
        let run = run_row(id, 0, "t");
        let trid = run.id;
        let d = detail(id, SRC, json!({}), vec![run]);
        let r = eval_expression(&d, Some(trid), "{{ taskrun.id }}");
        assert_eq!(r.result.as_deref(), Some(trid.to_string().as_str()));
        assert!(r.error.is_none(), "{r:?}");

        // `task.type` binds too, from the plan's spelling.
        let t = eval_expression(&d, Some(trid), "{{ task.type }}");
        assert_eq!(t.result.as_deref(), Some("io.kestra.plugin.core.log.Log"));
    }

    #[test]
    fn an_unknown_task_run_id_is_an_error() {
        let d = detail(Uuid::new_v4(), SRC, json!({}), vec![]);
        let err = stored_context(&d, Some(Uuid::new_v4()));
        assert!(matches!(err, Err(EvalError::UnknownTaskRun(_))), "{err:?}");
    }

    #[test]
    fn a_terminal_run_binds_tasks_and_outputs() {
        let id = Uuid::new_v4();
        let run = run_row(id, 0, "t");
        let d = detail(id, SRC, json!({}), vec![run]);
        let state = eval_expression(&d, None, "{{ tasks.t.state }}");
        assert_eq!(state.result.as_deref(), Some("SUCCESS"), "{state:?}");
        let out = eval_expression(&d, None, "{{ outputs.t.value }}");
        assert_eq!(out.result.as_deref(), Some("ok"), "{out:?}");
    }
}
