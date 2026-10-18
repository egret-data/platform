# Group C2: `SetVariables`, `UnsetVariables` and `Labels` — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** `docs/en/specs/2026-10-18-workflow-core-tasks-design.md`. Read §1 and §3 first; this plan builds §3 (C2) only. Its default decisions are D3 and D4, plus D5–D8 below, which this plan adds and Task 6 writes into the spec.

**Goal:** three Kestra `ExecutionUpdatableTask`s run inside egret's executor, and the oracle entries they unblock bank:
- `io.kestra.plugin.core.execution.SetVariables`
- `io.kestra.plugin.core.execution.UnsetVariables`
- `io.kestra.plugin.core.execution.Labels`

**Architecture:**
- A new plan kind `TaskKind::Update { op, spec }` (`executor/plan.rs`). No worker ever claims one; each type is in `spec::registry::EXECUTOR_LEVEL`, as `Exit` is.
- The pure computation (deep merge, dotted unset, the three label forms, every refusal) lives in a new module `executor/update.rs`, tested without a DB.
- A new worker render pass (`worker/render.rs::render_updates`, run after `render_expressions`) computes the update while the run is `CREATED`, as it does a `Loop`'s `values:`. It updates the in-memory `Surface` so later renders in the same settle see the new `vars`/`labels`, and records the final value in `RenderSide`. `settle` writes it to `workflow.executions` in the same transaction.
- `advance()` turns a computed run into `CREATED RUNNING SUCCESS` (one attempt `CREATED RUNNING SUCCESS`) in one round. A failed computation is `CREATED FAILED` with **one attempt whose history is `[FAILED]`**, through a new command.
- Migration 0195 adds `workflow.executions.variables jsonb` (NULL = the flow's `variables:`).

**Tech stack:** Rust (sqlx, minijinja-based renderer in `crate::runners`), Postgres, Python 3 (`handler_surface.py`).

**Branch and worktree:** `feat/workflow-kestra-executor-tasks` in `data-portal/egret-coretasks`, from `origin/main` `a2850644` (#691, C1). Migration 0195 is free on that main (last is `0194_workflow_task_run_working_dir_snapshot.sql`); re-check with `ls migrations | tail -3` after any rebase.

## Facts measured for this plan (2026-10-18)

Every implementer can rely on these; each was read in the code or the golden, not assumed.

- **Golden shapes** (`oracle-exec/golden.json`):
  - success: run `CREATED RUNNING SUCCESS`, attempts `[["CREATED","RUNNING","SUCCESS"]]` (`valids/set-variables`, `labels-update-task`, …);
  - failure: run `CREATED FAILED`, attempts **`[["FAILED"]]`** (`valids/set-variables-duplicate`, `valids/labels-update-task-empty`). A `Loop` that fails at `CREATED` has attempts `[]` (`valids/loop-null`), so the existing `render::fail` is the WRONG tool for an update: it pushes no attempt (`executor/state.rs:1017-1030`, the `SetRunState` arm only pushes onto an open attempt).
  - falsy `runIf`: `CREATED SKIPPED`, attempt `[CREATED, SKIPPED]` — the existing `Command::SkipRun`.
- **The 12 entries refused today because of these three types** (`executor/mod.rs`, `PUBLISHED_BUT_REFUSED`): `valids/labels-update-nested-value-list`, `labels-update-nested-value-map`, `labels-update-task-deduplicate`, `labels-update-task-empty`, `labels-update-task`, `npe-labels-update-task`, `primitive-labels-flow`, `set-variables-duplicate`, `set-variables`, `task-runif-executionupdating`, `task-runif-taskswithstate`, `unset-variables`. Several are also lines in `baseline.txt` (`:604-608`, `:617`, `:623`, `:645-646`, `:656`).
- **`taskrun.id` is null for a run created in the current settle.** `Surface::ids` is read once from `workflow.task_runs` in `Surface::load` (`worker/render.rs:512-531`), and `persist` inserts new rows with the column default `gen_random_uuid()` (`migrations/0134_workflow_executions.sql:47`). `valids/npe-labels-update-task` and `labels-update-task` render `{{ taskrun.id }}` inside a `Labels` at `CREATED`, so Task 4 reserves the id (below). No foreign key references `workflow.task_runs.id`.
- **`Surface` is fixed for a whole settle today.** `drive` takes `surface: &Surface` (`worker/claim.rs:4459-4505`); `vars` and `labels` are built once in `Surface::load` (`worker/render.rs:168-260`). Task 4 makes `drive` take `&mut Surface`.
- **`TaskKind` derives `Eq`** (`executor/plan.rs:323`), so it cannot hold a `serde_json::Value`. The update's spec is stored as its JSON text.
- **D4 is already enforced by the registry.** `spec/validate.rs:308-310` `is_runnable_child` refuses every `EXECUTOR_LEVEL` type as a `WorkingDirectory` child. Adding the three FQCNs to `EXECUTOR_LEVEL` is the whole change; Task 5 adds the test.
- **Kestra's `MapUtils.deepMerge`** (`kestra/core/src/main/java/io/kestra/core/utils/MapUtils.java:62-133`): a null new value keeps the old one; two maps merge recursively; **two collections concatenate, the new items that are not already in the old list appended in order**; anything else is replaced. A null or empty side returns the other side unchanged.
- **Kestra seeds `execution.variables` from the flow** (`Execution.java:205`, `.variables(flow.getVariables())`), and `RunVariables` reads execution variables first (`RunVariables.java:541-550`). So "NULL column = the flow's `variables:`" is exactly Kestra's starting value.
- **Kestra's `Labels`** (`kestra/core/src/main/java/io/kestra/plugin/core/execution/Labels.java`):
  - string form: the rendered text is read as `Map<String,String>` by Jackson. A duplicate JSON key keeps the LAST value (`labels-update-task-deduplicate`, golden SUCCESS). A non-string JSON value is coerced by Jackson to its text (number, boolean); a nested object or array fails.
  - list form: each item must be a map; `key` rendered, `value` rendered by `renderValue`; a later duplicate key wins.
  - map form: key rendered, value by `renderValue`. Two keys that render to the same text throw (`Collectors.toMap` without a merge function).
  - `renderValue`: a map value is rendered recursively and written as its JSON text (`labels-update-nested-value-map`, `…-list`: `{"name":"done"}`); any other value is `render(value.toString())`.
  - the system-label check runs before the empty-value check, each on the FIRST offending entry in map order.
  - the merge: the execution's current labels to a map, `putAll` the new ones, then drop an entry whose key or value is empty.

## Default decisions this plan adds (2026-10-18)

Recorded in the spec by Task 6, under "Default decisions".
- **D5. `SetVariables` merges exactly as Kestra's `deepMerge`,** including list concatenation-without-duplicates and "a null new value keeps the old one". Spec §3.2 said "any other value replaces", which is not Kestra's rule for lists or nulls; the spec is corrected.
- **D6. A failed update records one attempt `[FAILED]`,** as the golden does, through a new `Command::FailUnstarted`. A `Loop`'s failure at `CREATED` keeps no attempt.
- **D7. A `Labels` map whose two keys render to the same text fails the run** (`labels: two keys render to "<k>"`). Kestra throws an `IllegalStateException` there; a run failure with a message is the same outcome, stated.
- **D8. A whole-expression `variables:` that renders `null` fails the run** (the existing `render_map` error). Kestra's `asMap` returns null and `SetVariables` then throws a `NullPointerException` in `deepMerge`'s caller; a named failure is the same outcome without the crash.

## Global Constraints

- **Type names, exact.** FQCN only, no alias:
  - `io.kestra.plugin.core.execution.SetVariables`
  - `io.kestra.plugin.core.execution.UnsetVariables`
  - `io.kestra.plugin.core.execution.Labels`
- **Messages, verbatim from Kestra:**
  - ``"`overwrite` is set to false and the following variables already exist: <k1>,<k2>"`` (keys in the order of the rendered map, joined by `,` with no space);
  - `"System labels can only be set by Kestra itself, offending label: <k>=<v>"`;
  - `"Label values cannot be empty, offending label: <k>"`.
- **Egret's own messages** (no Kestra counterpart): an `UnsetVariables` key whose parent is missing or not a map: `` "`variables`: cannot unset `<key>`: `<prefix>` is not a map" `` (spec §3.2's deliberate divergence).
- **`vars`** = `workflow.executions.variables` when NOT NULL, else the flow's `variables:`. Subflow and `Loop` children do not inherit the parent's column. A replay copies it (D3).
- **Labels column**: `workflow.executions.labels` stores the full merged list after a `Labels` task (flow labels, then own, then the new ones, the later winning, empty key or value dropped); `crate::labels::execution_labels(flow, own)` keeps reading it correctly because `own` wins.
- **Never write an update while the execution is not `RUNNING`** (a `KILLING` execution creates and starts nothing; an update computed then would still be written by `settle`).
- **`oracle-exec/golden.json` is never edited.** The ratchet `cargo test -p egret-workflow --test executor` stays green after every task. Record before and after counts (mismatching / baselined / refused / LIVE) in each task report.
- **Never use the shared dev stack** (`:5432`, `deploy/docker-compose.dev.yml`, `egret-dev-*` containers, shared buckets).
  - DB tests run on the private Postgres container `egret-ct-postgres` on host port **5453**: `DATABASE_URL=postgres://egret:egret@localhost:5453/egret_test`, with `EGRET_ALLOW_DB_SKIP` unset.
  - Valkey, when a suite needs it, is `egret-ct-valkey` on **6453**.
  - Use a private target dir: `CARGO_TARGET_DIR=/Users/1pro-l4r8/Documents/github/data-portal/egret-coretasks/target` (the worktree's own; never another checkout's).
- **One cargo command at a time.** Never `pkill` cargo. `rg` is NOT installed, so use `grep -rn`.
- **Run only the tests for what you touch.** The full suites run once, at the final gate.
- **Never commit `.superpowers/`.**
- **Commits:** Conventional Commits, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## File map

| File | Responsibility | Task |
|---|---|---|
| `migrations/0195_workflow_execution_variables.sql` | the column and its CHECK | 1 |
| `crates/egret-workflow/src/worker/render.rs` | `vars` rule in `Surface::load`; the `render_updates` pass; `Surface` setters; `RenderSide` fields | 1, 4 |
| `crates/egret-workflow/src/worker/replay.rs` | copy `variables` | 1 |
| `crates/egret-workflow/src/executor/update.rs` (new) | `UpdateOp`, the three `Spec`s, `TYPE_ID`s, `compute` and its pure helpers | 2 |
| `crates/egret-workflow/src/executor/plan.rs` | `TaskKind::Update`, the `build_plan` arm, `direct_task_children` | 3 |
| `crates/egret-workflow/src/executor/command.rs`, `state.rs` | `Command::FailUnstarted` and its `apply` arm | 3 |
| `crates/egret-workflow/src/executor/flowable/mod.rs` | `open`/`descend` arms | 3 |
| `crates/egret-workflow/src/executor/mod.rs` | exhaustive-match arms in tests; `PUBLISHED_BUT_REFUSED` | 3, 5 |
| `crates/egret-workflow/src/worker/claim.rs` | `drive` takes `&mut Surface`; `write_render_side` writes variables, labels, reserved ids | 4 |
| `crates/egret-workflow/src/spec/registry.rs`, `spec/schema/measure.rs`, `property-treatment.json` | rows, `EXECUTOR_LEVEL`, measure arms | 5 |
| `crates/egret-workflow/oracle-exec/{driver.rs,handler_surface.py,handler-surface.txt,baseline.txt,README.md}` | oracle bookkeeping | 5, 6 |
| `docs/{en,vi}/specs/2026-10-18-workflow-core-tasks-design.md` | status, D5–D8, build notes | 6 |

---

### Task 1: migration 0195, the `vars` rule, and replay copies `variables`

**Files:**
- Create: `migrations/0195_workflow_execution_variables.sql`
- Modify: `crates/egret-workflow/src/worker/render.rs:168-190` (`Surface::load`: select `variables`, apply the rule)
- Modify: `crates/egret-workflow/src/worker/replay.rs` (the replay `INSERT … RETURNING` that binds `labels` at about `:275`: add `variables`)
- Test: `crates/egret-workflow/tests/migration_0195_variables.rs` (new; copy the shape of `tests/migration_0192_backfills.rs`), and a test in `tests/worker.rs` for the render rule and the replay copy

**Interfaces:**
- Produces: column `workflow.executions.variables jsonb NULL`, CHECK `variables IS NULL OR jsonb_typeof(variables) = 'object'`. Task 4 writes it; `Surface::load` reads it.

- [ ] **Step 1: write the failing migration test** in `tests/migration_0195_variables.rs`:

```rust
//! Migration 0195: `workflow.executions.variables` holds NULL or a JSON object.

#[sqlx::test(migrations = "../../migrations")]
async fn variables_takes_null_or_an_object_and_refuses_anything_else(pool: sqlx::PgPool) {
    // Arrange: one execution row, made the way migration_0192_backfills.rs makes one.
    let execution = common::seed_execution(&pool).await; // use that file's helper, or copy its INSERT
    // Act + Assert
    for ok in ["NULL", "'{}'::jsonb", "'{\"a\":{\"b\":[1]}}'::jsonb"] {
        sqlx::query(&format!("UPDATE workflow.executions SET variables = {ok} WHERE id = $1"))
            .bind(execution).execute(&pool).await.unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in ["'[]'::jsonb", "'1'::jsonb", "'\"s\"'::jsonb", "'null'::jsonb"] {
        let err = sqlx::query(&format!("UPDATE workflow.executions SET variables = {bad} WHERE id = $1"))
            .bind(execution).execute(&pool).await.expect_err(bad);
        assert!(err.to_string().contains("workflow_executions_variables_is_object"), "{bad}: {err}");
    }
}
```

  Match the attribute and seeding helper that `tests/migration_0192_backfills.rs` actually uses; the snippet's names are illustrative only where that file differs.
- [ ] **Step 2: run it, expect FAIL** (column missing): `DATABASE_URL=… cargo test -p egret-workflow --test migration_0195_variables`.
- [ ] **Step 3: write the migration**:

```sql
-- Group C2 (spec §3.2): the execution's own variables, written by SetVariables
-- and UnsetVariables. NULL means "the flow's `variables:`", which is Kestra's
-- starting value (`Execution.newExecution` copies the flow's).
ALTER TABLE workflow.executions ADD COLUMN variables jsonb;
ALTER TABLE workflow.executions
  ADD CONSTRAINT workflow_executions_variables_is_object
  CHECK (variables IS NULL OR jsonb_typeof(variables) = 'object');
```

- [ ] **Step 4: run, expect PASS.**
- [ ] **Step 5: write the failing render-rule and replay tests** in `tests/worker.rs`, using `start_execution`, `drain` and `run_outputs` as the neighbouring tests do:
  - `vars_reads_the_execution_column_before_the_flows_variables`: flow with `variables: {name: World}` and one `OutputValues` task `values: {m: "{{ vars.name }}"}`. Before draining, `UPDATE workflow.executions SET variables = '{"name":"Loïc"}'`. Expect outputs `{"values":{"m":"Loïc"}}`. A control execution with the column NULL gives `World`.
  - `a_replay_copies_the_execution_variables`: set the column on an execution, replay it through the same path the existing labels-copy test uses (find it with `grep -n "labels" tests/worker.rs | grep -i replay`), and assert the replay's `variables` equals the source's.
- [ ] **Step 6: run, expect FAIL.**
- [ ] **Step 7: implement.** In `Surface::load` add `variables` to the `SELECT`, then:

```rust
// Group C2 (spec §3.2): the execution's own variables first, the flow's as
// the fallback -- Kestra's `RunVariables` rule (`RunVariables.java:541-550`).
let vars = match row.get::<Option<Value>, _>("variables") {
    Some(own) => own,
    None => flow
        .as_ref()
        .and_then(|f| serde_json::to_value(&f.variables).ok())
        .unwrap_or(Value::Null),
};
```

  In `replay.rs` add `variables` to the column list and bind `row.get::<Option<Value>, _>("variables")` with a comment in the style of the `labels` one ("D3: `Execution.childExecution` copies the variables like everything else"). Make sure the `SELECT` that produces `row` reads `variables`.
- [ ] **Step 8: run the three tests, expect PASS.** Run the ratchet: unchanged.
- [ ] **Step 9: commit** `feat(workflow): executions.variables, read before the flow's variables (migration 0195)`.

---

### Task 2: `executor/update.rs`, the pure computation (spec §3.2, §3.3, D5, D7, D8)

**Files:**
- Create: `crates/egret-workflow/src/executor/update.rs`
- Modify: `crates/egret-workflow/src/executor/mod.rs` (declare `pub(crate) mod update;` beside `pub(crate) mod exit;` — match how `exit` is declared)

**Interfaces:**
- Consumes: `crate::models::property::{Property, render_map}`, `crate::runners::RenderContext`, `crate::runners::render_str`, `crate::models::Label`, `crate::labels::execution_labels`.
- Produces (Tasks 3–5 use these exact names):

```rust
pub const SET_VARIABLES: &str = "io.kestra.plugin.core.execution.SetVariables";
pub const UNSET_VARIABLES: &str = "io.kestra.plugin.core.execution.UnsetVariables";
pub const LABELS: &str = "io.kestra.plugin.core.execution.Labels";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateOp { Set, Unset, Labels }
impl UpdateOp {
    pub fn of(type_id: &str) -> Option<UpdateOp>;
    pub fn type_id(self) -> &'static str;
}

/// Read sets for `spec::schema::measure` (serde field names = Kestra's).
#[derive(Debug, Deserialize)] pub struct SetSpec { pub variables: Option<serde_json::Value>, #[serde(default)] pub overwrite: Option<Property<bool>> }
#[derive(Debug, Deserialize)] #[serde(rename_all = "camelCase")] pub struct UnsetSpec { pub variables: Option<Property<Vec<String>>>, #[serde(default)] pub ignore_missing: Option<Property<bool>> }
#[derive(Debug, Deserialize)] pub struct LabelsSpec { pub labels: Option<serde_json::Value> }

/// What one update changes.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// The execution's whole new `variables` object.
    Variables(serde_json::Map<String, serde_json::Value>),
    /// The execution's whole new label list (stored in `executions.labels`).
    Labels(Vec<crate::models::Label>),
}

/// Compute one update. `spec` is the task's leaf spec as JSON text (from the
/// plan), `vars` the current `vars` object (`Value::Null` = none), `labels` the
/// current merged label list (flow then own).
pub fn compute(
    op: UpdateOp,
    spec: &str,
    cx: &RenderContext,
    vars: &serde_json::Value,
    labels: &[crate::models::Label],
) -> Result<Change, String>;

pub fn deep_merge(a: &serde_json::Map<String, Value>, b: &serde_json::Map<String, Value>) -> serde_json::Map<String, Value>;
pub fn unset(vars: &mut serde_json::Map<String, Value>, key: &str, ignore_missing: bool) -> Result<(), String>;
```

- [ ] **Step 1: write the failing unit tests** in `update.rs`'s `#[cfg(test)] mod tests` (AAA, one behaviour each). At least:

```rust
use serde_json::json;
fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> { v.as_object().unwrap().clone() }
fn cx() -> crate::runners::RenderContext {
    let mut cx = crate::runners::RenderContext::new();
    cx.insert("inputs", json!({"k": "key", "v": "val", "n": 3}));
    cx
}

#[test]
fn deep_merge_merges_maps_appends_new_list_items_and_keeps_old_on_null() {
    let a = obj(json!({"m": {"x": 1, "y": 2}, "l": [1, 2], "s": "old", "keep": 1}));
    let b = obj(json!({"m": {"y": 3, "z": 4}, "l": [2, 3], "s": "new", "keep": null}));
    assert_eq!(
        serde_json::Value::Object(deep_merge(&a, &b)),
        json!({"m": {"x": 1, "y": 3, "z": 4}, "l": [1, 2, 3], "s": "new", "keep": 1})
    );
}

#[test]
fn set_variables_on_no_variables_is_the_rendered_map() {
    let change = compute(UpdateOp::Set, r#"{"variables":{"a":"{{ inputs.n }}"}}"#, &cx(), &serde_json::Value::Null, &[]).unwrap();
    assert_eq!(change, Change::Variables(obj(json!({"a": "3"}))));
}

#[test]
fn set_variables_with_overwrite_false_names_every_existing_key_in_order() {
    let err = compute(UpdateOp::Set, r#"{"variables":{"message":"Hello","name":"L","b":1},"overwrite":false}"#,
        &cx(), &json!({"name": "World", "b": 0}), &[]).unwrap_err();
    assert_eq!(err, "`overwrite` is set to false and the following variables already exist: name,b");
}

#[test]
fn set_variables_without_variables_fails() {
    assert!(compute(UpdateOp::Set, r#"{}"#, &cx(), &serde_json::Value::Null, &[]).unwrap_err().contains("variables"));
}

#[test]
fn unset_removes_a_dotted_leaf_and_a_missing_leaf_is_a_no_op() {
    let mut v = obj(json!({"a": {"b": {"c": 1, "d": 2}}, "list": []}));
    unset(&mut v, "a.b.c", false).unwrap();
    unset(&mut v, "list", false).unwrap();
    unset(&mut v, "absent", false).unwrap();
    assert_eq!(serde_json::Value::Object(v), json!({"a": {"b": {"d": 2}}}));
}

#[test]
fn unset_through_a_missing_parent_fails_unless_ignore_missing() {
    let mut v = obj(json!({"a": 1}));
    assert_eq!(unset(&mut v, "a.b", false).unwrap_err(), "`variables`: cannot unset `a.b`: `a` is not a map");
    assert_eq!(unset(&mut v, "x.y", false).unwrap_err(), "`variables`: cannot unset `x.y`: `x` is not a map");
    unset(&mut v, "x.y", true).unwrap();
    assert_eq!(serde_json::Value::Object(v), json!({"a": 1}));
}

#[test]
fn labels_in_three_forms_override_the_current_list() {
    let current = vec![
        crate::models::Label { key: "flowLabelKey".into(), value: "f".into() },
        crate::models::Label { key: "over".into(), value: "old".into() },
    ];
    let from_string = compute(UpdateOp::Labels, r#"{"labels":"{\"a\":\"1\",\"a\":\"2\",\"n\":5}"}"#, &cx(), &serde_json::Value::Null, &current).unwrap();
    let from_list = compute(UpdateOp::Labels, r#"{"labels":[{"key":"{{ inputs.k }}","value":"x"},{"key":"key","value":"{{ inputs.v }}"}]}"#, &cx(), &serde_json::Value::Null, &current).unwrap();
    let from_map = compute(UpdateOp::Labels, r#"{"labels":{"over":"new","nested":{"name":"done"}}}"#, &cx(), &serde_json::Value::Null, &current).unwrap();
    let kv = |c: Change| match c { Change::Labels(l) => l.into_iter().map(|l| (l.key, l.value)).collect::<Vec<_>>(), _ => panic!() };
    assert_eq!(kv(from_string), [("flowLabelKey","f"),("over","old"),("a","2"),("n","5")].map(|(k,v)| (k.to_string(), v.to_string())));
    assert_eq!(kv(from_list), [("flowLabelKey","f"),("over","old"),("key","val")].map(|(k,v)| (k.to_string(), v.to_string())));
    assert_eq!(kv(from_map), [("flowLabelKey","f"),("over","new"),("nested","{\"name\":\"done\"}")].map(|(k,v)| (k.to_string(), v.to_string())));
}

#[test]
fn labels_refuse_a_system_key_before_an_empty_value() {
    let err = compute(UpdateOp::Labels, r#"{"labels":{"e":"","system.x":"v"}}"#, &cx(), &serde_json::Value::Null, &[]).unwrap_err();
    assert_eq!(err, "System labels can only be set by Kestra itself, offending label: system.x=v");
    let err = compute(UpdateOp::Labels, r#"{"labels":"{ \"fromStringKey\": \"\", \"\": \"value2\" }"}"#, &cx(), &serde_json::Value::Null, &[]).unwrap_err();
    assert_eq!(err, "Label values cannot be empty, offending label: fromStringKey");
}

#[test]
fn labels_map_with_two_keys_rendering_alike_fails() {
    let err = compute(UpdateOp::Labels, r#"{"labels":{"key":"a","{{ inputs.k }}":"b"}}"#, &cx(), &serde_json::Value::Null, &[]).unwrap_err();
    assert_eq!(err, "`labels`: two keys render to `key`");
}

#[test]
fn labels_string_form_refuses_a_nested_object() {
    assert!(compute(UpdateOp::Labels, r#"{"labels":"{\"a\":{\"b\":1}}"}"#, &cx(), &serde_json::Value::Null, &[]).is_err());
}
```

  Notes for the tests above:
  - `labels-update-task-deduplicate` is the string form `{ "fromStringKey": "value1", "fromStringKey": "value2" }`. `serde_json::from_str` into a `serde_json::Map` keeps the LAST duplicate (verify this in a test of its own; if it does not, parse with a `Deserializer` that does).
  - In the string form a JSON number or boolean becomes its text (`"n":5` → `"5"`), as Jackson coerces; `null`, arrays and objects fail naming the key.
  - The merged output keeps the FIRST position of an existing key and appends new ones, which is `labels::execution_labels(current, new)`'s order. Reuse it rather than re-implementing; it also drops empty keys/values, which is Kestra's final `getEntryNotEmptyPredicate` filter.
  - The empty-key case from `labels-update-task-empty` (`"": "value2"`) is not refused by Kestra's checks (only empty VALUES are); it is dropped by the final filter. Keep that.
- [ ] **Step 2: run, expect compile failure**: `cargo test -p egret-workflow --lib executor::update`.
- [ ] **Step 3: implement** `update.rs`. Rules:
  - `compute` parses `spec` with `serde_json::from_str::<serde_json::Value>` and then into the op's `Spec`; a parse error is `"<TYPE_ID>: <error>"`.
  - **Set:** `variables` missing or null → `` "`variables`: must not be null" ``. Render with `render_map(raw, "variables", cx)` (D8: a whole expression that renders null is its error). `overwrite` via `Property<bool>::render`, default `true`; on `false`, collect the rendered keys present in the current object, in rendered order, and fail with the exact Kestra message. Current = `vars` as an object, or empty for `Null`. Result `Change::Variables(deep_merge(current, rendered))`. A non-object non-null `vars` is impossible (migration CHECK; the flow's `variables:` is a map) — treat it as empty.
  - **Unset:** `variables` missing → `` "`variables`: must not be null" ``. Render the list and `ignoreMissing` (default `false`). Start from the current object (empty for `Null`), apply `unset` for each key in order, return `Change::Variables`.
  - **`unset`:** split on the FIRST `.`; with a prefix, the prefix must name an object, else the error above (or `Ok(())` with `ignore_missing`); recurse with the suffix. Without a dot, `remove(key)` (absent = no-op).
  - **Labels:** `labels` missing or null → `` "`labels`: must not be null" ``. Build an ordered `Vec<(String, String)>`:
    - string → `render_str`, then parse as an object; a later duplicate wins (same position as its first occurrence is fine, but the value is the last);
    - array → each item an object with `key` and `value`; `key` rendered with `render_str`; value by `render_value`; a later duplicate key replaces the earlier value; a non-object item → `` "`labels[<i>]`: expected a map with `key` and `value`" ``;
    - object → key rendered, value by `render_value`; a rendered key already seen → D7 message;
    - anything else → `` "`labels`: expected a string, a list or a map" ``.
    - `render_value`: an object is rendered with `render_map(v, "labels.<key>", cx)` and serialised with `serde_json::to_string` (compact, source order); a string is `render_str`; a number/boolean is its JSON text rendered (`render_str(&v.to_string())`); null → `` "`labels.<key>`: must not be null" ``.
    - then the system check, then the empty-value check, each over the vector in order; then `Change::Labels(execution_labels(labels, &new_as_labels))`.
- [ ] **Step 4: run, expect PASS.** `cargo clippy -p egret-workflow --lib -- -D warnings` on the touched module.
- [ ] **Step 5: commit** `feat(workflow): the pure computation of SetVariables, UnsetVariables and Labels`.

---

### Task 3: the plan kind, `FailUnstarted`, and `advance()` (spec §3.1, D6)

**Files:**
- Modify: `crates/egret-workflow/src/executor/plan.rs` (`TaskKind::Update` after `Exit` at `:619`; `direct_task_children` at `:119`; the `build_plan` arm beside `Exit`'s at `:1333-1339`)
- Modify: `crates/egret-workflow/src/executor/command.rs` (`Command::FailUnstarted { run: RunIx, at: Tick }`, documented like `SkipRun`)
- Modify: `crates/egret-workflow/src/executor/state.rs` (`apply` arm, beside `SkipRun` at `:922-930`)
- Modify: `crates/egret-workflow/src/executor/flowable/mod.rs` (`open` at `:426-441`; `descend`'s match at about `:480-525`)
- Modify: every exhaustive `match` over `TaskKind` / `Command` the compiler names (at least `executor/mod.rs:753` test fixture, `:3574` `classify`, `:3610` `observed`, `worker/render.rs:787` `task.type`), and wherever `Command` is matched (e.g. `worker/claim.rs` `Requested::of`, `marks`, `stamps`) — let `cargo check -p egret-workflow --all-targets` list them.

**Interfaces:**
- Consumes: `executor::update::{UpdateOp, SET_VARIABLES, UNSET_VARIABLES, LABELS}`.
- Produces:
  - `TaskKind::Update { op: UpdateOp, spec: String }` — `spec` is the leaf spec's JSON text (`serde_json::to_string(spec)`).
  - `Command::FailUnstarted { run: RunIx, at: Tick }` — `apply` pushes `RunState::Failed` onto the run and one `Attempt { history: vec![RunState::Failed], started_at: at, ended_at: None }`.
  - The contract with Task 4: **a run of kind `Update` with `rendered.is_some()` has had its change computed and recorded; `advance()` then completes it.** Task 4 sets `rendered = Some(String::new())`.

- [ ] **Step 1: write the failing `advance()` tests** in `executor/mod.rs`'s test module, next to the `Exit` tests (find them with `grep -n "fn .*exit" src/executor/mod.rs src/executor/exit.rs`). Build plans from YAML with the same helper those tests use; set `rendered` by hand to stand in for the worker:
  - `an_update_waits_at_created_until_rendered_then_ends_created_running_success_in_one_round`: one `SetVariables` task. Round 1 creates it; `advance` again returns no command for the run while `rendered` is `None`; set `rendered = Some(String::new())`; the next round's commands, applied, give run history `[Created, Running, Success]` and one attempt `[Created, Running, Success]`; the execution then succeeds.
  - `fail_unstarted_records_one_failed_attempt`: apply `FailUnstarted` to a `CREATED` run → history `[Created, Failed]`, attempts `[[Failed]]`; the execution then fails (as the golden `set-variables-duplicate` reads `CREATED RUNNING FAILED`).
  - `an_update_with_a_falsy_run_if_is_skipped`: `runIf: "false"` and `rendered_run_if = Some("false")` → `[Created, Skipped]`.
  - `two_updates_in_one_parallel_complete_in_the_same_round`: `Parallel` of two `Labels`, both rendered → both `CREATED RUNNING SUCCESS`.
  - `build_plan_plans_the_three_types_as_updates`: each FQCN → `TaskKind::Update { op, spec }` with `spec` parsing back to the leaf's properties (no `id`/`type` needed either way, but assert `variables`/`labels` is present).
- [ ] **Step 2: run, expect compile failure.**
- [ ] **Step 3: implement.**
  - `plan.rs`: the variant with a doc comment in the `Exit` style (an `ExecutionUpdatableTask`; computed at `CREATED` by the worker's render pass; completed by `advance()` in one round; no children). `build_plan` arm, before the `Subflow` arm: `(ModelKind::Leaf { spec }, _) if let Some(op) = UpdateOp::of(&task.r#type)` → `TaskKind::Update { op, spec: spec.to_string() }` (use a guard that compiles on the crate's edition, e.g. `if super::update::UpdateOp::of(&task.r#type).is_some()` and compute `op` inside). `direct_task_children`: add `| TaskKind::Update { .. }` to the leaf arm.
  - `flowable::open`: after the `runIf` gate,

```rust
TaskKind::Update { .. } => match state.runs[run].rendered {
    // The worker's render pass has not computed it yet (spec §3.1).
    None => Step::Waiting,
    // Computed and recorded: `CREATED RUNNING SUCCESS` in one round, as
    // Kestra records an `ExecutionUpdatableTask` (golden `valids/set-variables`).
    Some(_) => Step::Commands(vec![
        Command::StartRun { run, at: now },
        Command::SetRunState { run, state: RunState::Success },
    ]),
},
```

  - `flowable::descend`: `TaskKind::Update { .. } => return Step::Commands(vec![Command::SetRunState { run, state: RunState::Success }])` with a comment: a `RUNNING` update can only be one whose `StartRun` and `SUCCESS` were split by a reader between the two (none exists; the pair is one round), so it ends as it would have.
  - `FailUnstarted`'s `apply` arm, documented with the golden measurement (`[["FAILED"]]` vs `loop-null`'s `[]`).
  - The other exhaustive matches: `classify`/`observed` → `PlannedAs::Waits` (the `Exit` tier); the `run_to_fixpoint_rendering` fixture arm panics like `Exit`'s; `render.rs:787` `task.type` → `Some(op.type_id().to_string())`. For `Command` matches outside `executor`, `FailUnstarted` behaves like `SkipRun` (it ends a run that never ran: no spawn, no subflow).
- [ ] **Step 4: run** `cargo test -p egret-workflow --lib executor`, expect PASS; `cargo check -p egret-workflow --all-targets` clean. Run the ratchet: counts unchanged (the types are still unregistered, so nothing new publishes).
- [ ] **Step 5: commit** `feat(workflow): plan SetVariables, UnsetVariables and Labels as executor updates`.

---

### Task 4: compute in the render pass, write in `settle` (spec §3.1–§3.3)

**Files:**
- Modify: `crates/egret-workflow/src/worker/render.rs` (`Surface`: `vars`/`labels` setters and the label bookkeeping; `RenderSide`: three fields; a new pass `render_updates`)
- Modify: `crates/egret-workflow/src/worker/claim.rs` (`settle` passes `&mut surface`; `drive` takes `&mut Surface`; `write_render_side` writes the new fields; the "is there anything to write" condition at `:3332`)
- Test: `crates/egret-workflow/tests/worker.rs`

**Interfaces:**
- Consumes: `executor::update::{compute, Change}`, `TaskKind::Update`, `Command::FailUnstarted` (Task 3).
- Produces (all `pub(super)` in `worker`):

```rust
impl Surface {
    /// The current merged labels, flow then own (`labels::execution_labels`).
    pub(super) fn current_labels(&self) -> &[crate::models::Label];
    pub(super) fn vars(&self) -> &serde_json::Value;
    /// Replace `vars` for every later render of this settle.
    pub(super) fn set_vars(&mut self, vars: serde_json::Value);
    /// Replace the merged labels and rebuild the `labels` render value,
    /// keeping the `system` entry `load` built.
    pub(super) fn set_labels(&mut self, labels: Vec<crate::models::Label>);
    /// `taskrun.id` for `run`, reserving a fresh UUID if the run has no row yet.
    pub(super) fn reserve_id(&mut self, run: RunIx) -> (uuid::Uuid, bool /* newly reserved */);
}
pub(super) struct RenderSide {
    // existing: failures, file_values
    pub(super) variables: Option<serde_json::Value>,      // the final object to write
    pub(super) labels: Option<Vec<crate::models::Label>>, // the final list to write
    pub(super) run_ids: Vec<(RunIx, uuid::Uuid)>,         // ids reserved this settle
}
```

- [ ] **Step 1: write the failing DB tests** in `tests/worker.rs` (flows inline, as the `Exit` tests at `:10682` do; `start_execution` + `drain` + `run_state`/`run_outputs`/`execution_history`):
  - `set_variables_then_output_reads_the_new_value`: corpus `valids/set-variables` (flow `variables: {name: World}`; `SetVariables {message: Hello, name: Loïc}`; `OutputValues {message: "{{ vars.message }} {{ vars.name }}"}`) → output `Hello Loïc`; `SELECT variables FROM workflow.executions` = `{"name":"Loïc","message":"Hello"}` (key order as `deep_merge` produces); run `set-vars` history `["CREATED","RUNNING","SUCCESS"]`.
  - `set_variables_overwrite_false_fails_from_created_with_one_failed_attempt`: corpus `valids/set-variables-duplicate` → run history `["CREATED","FAILED"]`, `attempts` JSON has one attempt whose history is `["FAILED"]`, `message` = the Kestra text; execution FAILED; `variables` still NULL.
  - `unset_variables_then_the_default_applies`: corpus `valids/unset-variables` → output `default`.
  - `labels_then_a_later_render_reads_them`: flow with `labels: {flowLabelKey: f}`, a `Labels {labels: {k: "{{ flow.id }}"}}`, then `OutputValues {values: {a: "{{ labels.k }}", b: "{{ labels.flowLabelKey }}"}}` → `a` = flow id, `b` = `f`; `executions.labels` = `[{"key":"flowLabelKey","value":"f"},{"key":"k","value":"<flow id>"}]`.
  - `labels_can_read_its_own_taskrun_id`: corpus `valids/npe-labels-update-task` → the label `someLabel` equals the `id` of the `labels` row in `workflow.task_runs` (so the reserved id was written).
  - `two_updates_in_one_parallel_apply_in_run_order`: `Parallel` of `SetVariables {x: 1}` and `SetVariables {x: 2}` (no `overwrite`), then an `OutputValues {x: "{{ vars.x }}"}` → `2`, and repeat-run five times to show it does not depend on timing.
  - `an_update_under_a_killing_execution_writes_nothing`: start a flow whose first task is a `core.log.Log` and second a `SetVariables`; set the execution `KILLING` the way `tests/kill_route.rs` does before the log run completes; drain; `variables` is NULL and the `SetVariables` run is not `SUCCESS`.
  - `run_if_false_skips_the_update_and_writes_nothing`: `SetVariables` with `runIf: "false"` → `["CREATED","SKIPPED"]`, `variables` NULL.
- [ ] **Step 2: run, expect FAIL** (types not planned/applied). Note: if `start_execution` refuses unregistered types, add the three `TASK_TYPES` rows and `EXECUTOR_LEVEL` entries from Task 5 Step 1 here instead, and move the ratchet bookkeeping (Task 5 Steps 3–5) into this task so the ratchet stays green at this commit. Say so in the report.
- [ ] **Step 3: implement.**
  - `Surface`: keep `own`-merged labels as a `Vec<Label>` field (`labels_list`) built in `load` beside the `labels` value; `set_labels` rebuilds the `labels` object from the list and re-inserts the `system` entry (`load` builds it at `:505-510`; keep that map value in a field so it can be re-inserted unchanged). `reserve_id`: if `self.ids` has `run`, return it with `false`; else insert `Uuid::new_v4()` and return `true`.
  - **A pass of its own, `render_updates`**, not a branch of `render_expressions` (whose population is "one expression per task", `expression_of`, and which keeps `&Surface`). Signature: `pub(super) fn render_updates(surface: &mut Surface, plan: &Plan, state: &mut ExecutionState, now: Tick, side: &mut RenderSide) -> Pass`. `drive` calls it after `render_expressions` returns `Nothing`, with the same `snapshot` / `*seq += 1` / `marks.observe` / `continue` shape. Its population: runs whose kind is `Update`, whose last state is `CREATED`, `rendered.is_none()`, whose task has no `runIf` or a `rendered_run_if` that is `Some` and truthy (`crate::executor::is_truthy`), **and the execution's last state is `RUNNING`** (`state.history.last() == Some(&RunState::Running)`), in run order.
  - For each pending update run, in run order:

```rust
// Group C2 (spec §3.1): computed at CREATED against the CURRENT surface, so
// an update earlier in this pass is seen by this one (two branches of a
// Parallel apply in run order).
let (id, fresh) = surface.reserve_id(ix);
if fresh { side.run_ids.push((ix, id)); }
let cx = surface.context(plan, state, Some(ix));
let TaskKind::Update { op, spec } = &plan.tasks[state.runs[ix].task].kind else { unreachable!() };
match crate::executor::update::compute(*op, spec, &cx, surface.vars(), surface.current_labels()) {
    Ok(Change::Variables(v)) => {
        let v = serde_json::Value::Object(v);
        surface.set_vars(v.clone());
        side.variables = Some(v);
        state.runs[ix].rendered = Some(String::new());
    }
    Ok(Change::Labels(l)) => {
        surface.set_labels(l.clone());
        side.labels = Some(l);
        state.runs[ix].rendered = Some(String::new());
    }
    Err(why) => {
        crate::executor::apply(state, &Command::FailUnstarted { run: ix, at: now });
        side.failures.push((ix, why));
    }
}
```

    Take the limit failure (`surface.take_limit_failure()`) after `compute` and, if any, prefer its message, as the other arms do.
  - A run whose `rendered` is set but whose settle rolls back loses both together (one transaction), so no extra bookkeeping is needed; say so in a comment.
  - `claim.rs`: `drive(…, surface: &mut Surface, …)`; `settle` passes `&mut surface`. `write_render_side`: after the existing loops,

```rust
if let Some(vars) = side.variables {
    sqlx::query("UPDATE workflow.executions SET variables = $2 WHERE id = $1")
        .bind(execution).bind(vars).execute(&mut *conn).await.map_err(db_err)?;
}
if let Some(labels) = side.labels {
    sqlx::query("UPDATE workflow.executions SET labels = $2 WHERE id = $1")
        .bind(execution).bind(serde_json::to_value(&labels).expect("labels serialise"))
        .execute(&mut *conn).await.map_err(db_err)?;
}
for (run, id) in side.run_ids {
    sqlx::query("UPDATE workflow.task_runs SET id = $3 WHERE execution_id = $1 AND run_ix = $2")
        .bind(execution).bind(run as i32).bind(id).execute(&mut *conn).await.map_err(db_err)?;
}
```

    and widen the guard at `:3332` to `!side.is_empty()` (add `RenderSide::is_empty`). Check the `labels` JSON shape matches what `labels::own_labels` reads back (`[{"key":…,"value":…}]`) and what the backfill/replay writers store.
  - `worker/render.rs:787`'s `task.type` arm (Task 3) already names the FQCN.
- [ ] **Step 4: run** the new `tests/worker.rs` tests and the existing `Exit`/`Loop` ones (`cargo test -p egret-workflow --test worker -- exit loop update labels variables`), expect PASS. Ratchet unchanged.
- [ ] **Step 5: commit** `feat(workflow): compute executor updates in the render pass and write them in settle`.

---

### Task 5: register the types, and the oracle bookkeeping

Follow `[[egret-workflow-new-task-type-footprint]]`: the executor-level template is `Exit` in #689 (`15406ad9`).

**Files:**
- Modify: `crates/egret-workflow/src/spec/registry.rs` (three `TASK_TYPES` rows after `Exit`'s at `:206-210`, FQCN only, no alias, no references; `EXECUTOR_LEVEL` at `:356` gets the three constants)
- Modify: `crates/egret-workflow/src/spec/schema/measure.rs:142` (three arms: `update::SET_VARIABLES => fields_of::<update::SetSpec>()`, …), then regenerate with `EGRET_WRITE_PROPERTY_TREATMENT=1 cargo test -p egret-workflow --lib spec::schema::measure`
- Modify: `crates/egret-workflow/src/spec/validate.rs` tests (D4)
- Modify: `crates/egret-workflow/oracle-exec/driver.rs` (`LIVE`, `UNKNOWN_TYPE_REFUSED`, `VALIDATE_REFUSED` with dated doc lines; NO `registry` change — there is no handler)
- Modify: `crates/egret-workflow/oracle-exec/handler_surface.py` (`W3A_EXECUTOR_LEVEL` at `:530`, and the baseline count), then `python3 handler_surface.py` to regenerate `handler-surface.txt`
- Modify: `crates/egret-workflow/oracle-exec/baseline.txt` and `PUBLISHED_BUT_REFUSED` in `crates/egret-workflow/src/executor/mod.rs`
- Check: `runtime::served_for` needs NO key — `Served::serves` already answers true for `EXECUTOR_LEVEL` (`runtime.rs:875-879`). Confirm with a test that publish accepts a flow of the three on a deployment with no handlers (copy the `Exit` one: `grep -n "EXECUTOR_LEVEL\|executor_level" src/runtime.rs src/spec/*.rs tests/*.rs`).

- [ ] **Step 1: write the failing tests:**
  - in `spec/validate.rs`'s tests, `the_three_update_types_are_refused_inside_a_working_directory`, copying the `Exit` one (find it with `grep -n "Exit" src/spec/validate.rs`), one assertion per FQCN;
  - the publish-on-a-bare-deployment test above, for the three FQCNs.
- [ ] **Step 2: register** (rows, `EXECUTOR_LEVEL`, measure arms, regenerate `property-treatment.json`). Run those tests and `cargo test -p egret-workflow --lib spec`, expect PASS.
- [ ] **Step 3: run the ratchet** `cargo test -p egret-workflow --test executor`. The 12 entries in "Facts measured" leave the refused list. For each entry the failures name:
  - matches the golden → remove it from `baseline.txt` and from `PUBLISHED_BUT_REFUSED`;
  - publishes and still mismatches → keep/add it in `baseline.txt` with a `# group C2 Task 5 (2026-10-18): …` comment naming the measured cause (the first differing field the ratchet prints), and remove it from `PUBLISHED_BUT_REFUSED`;
  - for an entry whose `cause:` names a type C2 does not add, keep it and trim the cause to the types still missing.
  Expected to bank (measure, do not assume): `set-variables`, `set-variables-duplicate`, `unset-variables`, `task-runif-executionupdating`, `task-runif-taskswithstate`, the seven `labels*` entries. If one of them does NOT bank, find the cause before baselining it; a mismatch in the attempt shape is a Task 3/4 defect, not a baseline line.
- [ ] **Step 4: update the counts**: `driver.rs` `LIVE` / `UNKNOWN_TYPE_REFUSED` / `VALIDATE_REFUSED`, each with a dated doc line ("group C2 Task 5 (2026-10-18): SetVariables, UnsetVariables and Labels have `TASK_TYPES` rows and are `EXECUTOR_LEVEL`; N flows leave …"). `handler_surface.py`: add the three to `W3A_EXECUTOR_LEVEL`, update its baseline count, run it, commit the regenerated `handler-surface.txt`. Repeat Step 3 until the ratchet is green, 0 broken.
- [ ] **Step 5: record** before/after: mismatching/baselined (expect 84/84 before), refused, LIVE (281 before), handler-surface ceiling (250 before).
- [ ] **Step 6: commit** `feat(workflow): register SetVariables, UnsetVariables and Labels as executor-level tasks`.

---

### Task 6: docs

**Files:**
- Modify: `docs/en/specs/2026-10-18-workflow-core-tasks-design.md` and `docs/vi/specs/2026-10-18-workflow-core-tasks-design.md`:
  - `Status:` "C2 built and verified <date>", with Task 5's ratchet counts;
  - D5–D8 under "Default decisions" (text from this plan);
  - §3.2: replace "where a nested map merges and any other value replaces" with D5's rule;
  - a dated "(2026-10-18, C2 built)" note after §3.3 for every place the build differs from §3: `FailUnstarted` and the `[FAILED]` attempt; `taskrun.id` reserved for an update run created in the same settle (and that every OTHER kind still sees `taskrun.id` null in the settle that creates it — a known gap, unchanged); updates are skipped while the execution is not `RUNNING`; `runtime::served_for` needs no key for executor-level types.
- Modify: `crates/egret-workflow/oracle-exec/README.md`: the coverage counts the ratchet asserts, and a "Group C2" section listing what banked and what was baselined with which cause.

- [ ] **Step 1:** write the edits; every count says where it came from (the ratchet printout or `python3 handler_surface.py`). The Vietnamese spec mirrors the English one section by section.
- [ ] **Step 2:** `cargo test -p egret-workflow --test executor` (the README counts are asserted).
- [ ] **Step 3: commit** `docs(workflow): group C2 built — spec status, D5–D8, oracle README`.

---

## Final gate (controller, after the final whole-branch review)

On a branch rebased onto fresh `origin/main`, on the private containers, one command at a time:
1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo test -p egret-workflow` (all targets)
4. `cargo test -p egret-query`, `cargo test -p egret-dataapi` (needs `EGRET_TEST_SHARED_STORE_URL`), `cargo test -p egret-server`
5. No frontend change, so no vitest/lint/typecheck.

Known and accepted from earlier gates: the 7 `sql_starrocks_live` tests fail without an engine.
