# Group C1: `OutputValues`, `Assert` and `PurgeLogs` — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** `docs/en/specs/2026-10-18-workflow-core-tasks-design.md`. Read §1 and §2 first; this plan builds §2 (C1) only. Its default decisions are D1 and D2. §3 (C2) is a later PR, and nothing here may start it.

**Goal:** three Kestra tasks run on egret's worker, and the oracle entries they unblock bank:
- `io.kestra.plugin.core.output.OutputValues`
- `io.kestra.plugin.core.execution.Assert`
- `io.kestra.plugin.core.log.PurgeLogs`

**Architecture:** each task is a `TaskHandler` in `crates/egret-workflow/src/worker/handlers/`, plus a `spec::registry::TASK_TYPES` row, the same pattern as `Return` (`worker/handlers/ret.rs`).
- `OutputValues` and `Assert` need no connection, so they go into `Handlers::with_core_tasks`.
- `PurgeLogs` needs the pool. It is registered by its own `register` from `runtime::handlers_for`, the way `handlers::storage::register` is.
- The flow JSON schema is re-exported once from the pinned Kestra image, with all six group-C types added, so C2 does not have to repeat the export.

**Tech stack:** Rust (sqlx, minijinja-based renderer in `crate::runners`), Postgres, Python 3 (`trim.py`, `handler_surface.py`), Docker (only to re-export the schema from the local Kestra image).

**Branch and worktree:** `feat/workflow-kestra-core-tasks` in `data-portal/egret-coretasks`, from `origin/main` `15406ad9`. Spec commit `114b8dcd`.

## Global Constraints

- **Type names and spellings, exact.** FQCN only, no short alias:
  - `io.kestra.plugin.core.output.OutputValues`
  - `io.kestra.plugin.core.execution.Assert`
  - `io.kestra.plugin.core.log.PurgeLogs`
- **Messages, verbatim from Kestra:**
  - ``Assertion `<c>` failed!``
  - ``Assertion `<c>` failed, failed to render `<err>` ``
  - `"<n> assertion failed!"` when n = 1, `"<n> assertions failed!"` when n > 1, followed by `"\n" + <rendered errorMessage>` when `errorMessage` is set.
- **`OutputValues` outputs** are `{"values": <map>}`. With no `values`, they are `{"values": {}}`.
- **`PurgeLogs` outputs** are `{"count", "executionLogsCount", "nonExecutionLogsCount"}`, with `nonExecutionLogsCount` always `0` (D2).
- **`PurgeLogs` scope (decided by the user):**
  - Always the running execution's workspace.
  - With no `namespace`, the flow's own namespace and its child namespaces.
  - With a `namespace`, it must pass `storage::nsfiles::access::authorize(…, Access::Purge)` with a new `Resource::Logs`. Otherwise the run is FAILED with the refusal's message, and nothing is deleted.
  - A child namespace means a match on a dot boundary: `a.b` reaches `a.b.c` but not `a.bc`.
- **No metrics (D1).** `Assert` logs one INFO line, `"<s> succeeded, <f> failed"`.
- **`oracle-exec/golden.json` is never edited.** The ratchet `cargo test -p egret-workflow --test executor` stays green after every task. Record the before and after counts in each task report. `valids/purge_logs_full_arguments` becomes a permanent `baseline.txt` line, with the comment giving spec §2.3's reason.
- **Never use the shared dev stack** (`:5432`, `deploy/docker-compose.dev.yml`, `egret-dev-*` containers, shared buckets).
  - DB tests run on a private Postgres container `egret-ct-postgres` on host port **5453**: `DATABASE_URL=postgres://egret:egret@localhost:5453/egret_test`, with `EGRET_ALLOW_DB_SKIP` unset.
  - Valkey, when a suite needs it, is `egret-ct-valkey` on **6453**.
- **One cargo command at a time.** Never `pkill` cargo. `rg` is NOT installed, so use `grep -rn`.
- **Never commit `.superpowers/`.**
- **Commits:** Conventional Commits, ending with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

---

### Task 1: re-export the flow schema with the six group-C types

The schema tests use `kestra-flow-schema.json`'s definition of a registry type when it has one (`spec/schema/mod.rs:365`). Re-export once, now, for all six types, so neither C1's later tasks nor C2 have to.

**Files:**
- Modify: `crates/egret-workflow/src/spec/schema/trim.py` (the `KESTRA_TYPES` list; `Exit` is at `:52`)
- Modify: `crates/egret-workflow/src/spec/schema/kestra-flow-schema.json` (regenerated, not hand-edited)
- Modify: `crates/egret-workflow/src/spec/schema/README.md` (§1 table: the export date and the reason)

**Interfaces:**
- Produces: `kestra-flow-schema.json` holds `$defs` for all six FQCNs, with `x-egret-outputs`. Later tasks rely on it.

- [ ] **Step 1: add the six FQCNs to `KESTRA_TYPES`**, after `io.kestra.plugin.core.execution.Exit`, in this order:
  - `io.kestra.plugin.core.output.OutputValues`
  - `io.kestra.plugin.core.execution.Assert`
  - `io.kestra.plugin.core.log.PurgeLogs`
  - `io.kestra.plugin.core.execution.SetVariables`
  - `io.kestra.plugin.core.execution.UnsetVariables`
  - `io.kestra.plugin.core.execution.Labels`
- [ ] **Step 2: re-export**, following the README §1 recipe exactly. The image `28f59bf175e1` (`kestra/kestra:<none>`) is local. Before you start:
  - Use a throwaway container on a free port (not 18093 if it is taken).
  - Name the container `egret-ct-kestra`.
  - Remove it when the export is done.
  
  The final call:

```bash
python3 trim.py raw-flow-schema.json plugin-docs kestra-flow-schema.json \
  2.0.0-rc13 sha256:28f59bf175e1158158022db31c5addebe4801747faf41e8bad83065c787fc4bd 2026-10-18
```

- [ ] **Step 3: check the diff is only additions.** Compare the old and new `kestra-flow-schema.json` definition by definition with a short Python script, as the README's 2026-10-17 note did. Expected: six new type definitions, plus whatever they reach through `$ref`, plus `x-kestra-export`'s `kept_definitions` and `raw_sha256`, and nothing else changed. Put the script's output in the task report. If any existing definition changed, stop and report it (`DONE_WITH_CONCERNS`).
- [ ] **Step 4: update README §1.** Set `Exported` to 2026-10-18, with "re-exported from the same image when group C1 Task 1 added the six group-C types to `KESTRA_TYPES`". Update the `Committed after trim.py` numbers.
- [ ] **Step 5: run the schema tests**: `cargo test -p egret-workflow --lib spec::schema`. Expected: PASS. The six types have no registry row yet, so nothing serves them.
- [ ] **Step 6: commit**: `chore(workflow): re-export the flow schema with the six group-C Kestra types`.

---

### Task 2: `OutputValues`, and the map renderer C2 will reuse (spec §2.1)

**Files:**
- Modify: `crates/egret-workflow/src/models/property.rs` (add `render_map`)
- Create: `crates/egret-workflow/src/worker/handlers/output_values.rs`
- Modify: `crates/egret-workflow/src/worker/handlers/mod.rs` (declare the module)
- Modify: `crates/egret-workflow/src/worker/registry.rs:126-152` (`with_core_tasks`: `add_publishable`)
- Modify: `crates/egret-workflow/src/spec/registry.rs` (a `TASK_TYPES` row after `Exit`'s, at `:206-210`)
- Modify: `crates/egret-workflow/src/spec/schema/measure.rs:110-136` (a `fields_of::<…::Spec>()` arm), and regenerate `property-treatment.json` as `measure.rs`'s doc says
- Modify: `crates/egret-workflow/oracle-exec/driver.rs` (the `registry` fn, `:142`: `r.add(…OutputValues)`; the `LIVE` and `UNKNOWN_TYPE_REFUSED` constants)
- Modify: `crates/egret-workflow/oracle-exec/handler_surface.py` (`W3B_HANDLERS`, `:431`, and the `baseline.txt` count check), then regenerate `handler-surface.txt`
- Modify: `crates/egret-workflow/oracle-exec/baseline.txt`, and the `PublishedButRefused` list in `crates/egret-workflow/src/executor/mod.rs` (`:4240` onwards)

**Interfaces:**
- Produces:
  - `pub fn render_map(raw: &serde_json::Value, cx: &RenderContext) -> Result<serde_json::Map<String, serde_json::Value>, String>` in `crate::models::property`. C2's `SetVariables` and `Labels` use it.
  - `crate::worker::handlers::output_values::{TYPE_ID, OutputValues, Spec}`.

- [ ] **Step 1: write the `render_map` tests**, in `property.rs`'s test module. Kestra's rule is `Property.asMap` (`kestra/core/src/main/java/io/kestra/core/models/property/Property.java:264-287`):
  - If the source is a string that, trimmed, starts with `{{` and ends with `}}`, it is rendered, and the text is parsed as a JSON object.
  - Otherwise a map is rendered entry by entry (`VariableRenderer.render(Map)`, `core/…/runners/VariableRenderer.java:187-221`). Keys are rendered. String leaves are rendered to strings. Lists and maps are walked. Numbers, booleans and null are kept. When two keys render to the same text, the FIRST one wins (`putIfAbsent`).
  - A string that is not an expression is parsed as a JSON object (`MAPPER.readValue(expression, Map.class)`). If that fails, it is an error.

```rust
#[test]
fn render_map_renders_keys_and_string_leaves_and_keeps_other_scalars() {
    let mut cx = crate::runners::RenderContext::new();
    cx.insert("n", serde_json::json!(2)).insert("k", serde_json::json!("key"));
    let raw = serde_json::json!({"a": "{{ n + 1 }}", "{{ k }}": [1, "{{ n }}", {"x": true}], "z": null});
    let out = render_map(&raw, &cx).unwrap();
    assert_eq!(serde_json::Value::Object(out), serde_json::json!({"a": "3", "key": [1, "2", {"x": true}], "z": null}));
}

#[test]
fn render_map_parses_a_whole_expression_as_a_json_object() {
    let mut cx = crate::runners::RenderContext::new();
    cx.insert("m", serde_json::json!({"p": 1}));
    let out = render_map(&serde_json::json!("{{ m | toJson }}"), &cx).unwrap();
    assert_eq!(serde_json::Value::Object(out), serde_json::json!({"p": 1}));
    assert!(render_map(&serde_json::json!("{{ 'not a map' }}"), &cx).is_err());
}

#[test]
fn render_map_keeps_the_first_of_two_keys_that_render_alike() {
    let mut cx = crate::runners::RenderContext::new();
    cx.insert("k", serde_json::json!("a"));
    let raw = serde_json::json!({"a": "first", "{{ k }}": "second"});
    assert_eq!(render_map(&raw, &cx).unwrap()["a"], serde_json::json!("first"));
}
```

  `RenderContext::new()` and `insert(key, JsonValue) -> &mut Self` are at `runners/env.rs:342-350`; `toJson` is registered at `runners/filters.rs:183`. These tests show `render_map` without its `field` parameter (Step 3 adds it), so pass `"values"` when you add it. Note: `serde_json::Map` keeps insertion order here (`preserve_order` is on), so the map's iteration order is the source order.
- [ ] **Step 2: run, and expect a compile failure**: `cargo test -p egret-workflow --lib models::property`.
- [ ] **Step 3: implement `render_map`** with `crate::runners::render_str` for every string, recursing through arrays and objects. On a collision, keep the first key (`entry().or_insert`). Error messages name where they happened: `` `values.<key>`: <render error> ``, and for a whole expression whose text is not a JSON object, `` `values`: the expression rendered `<text>`, which is not a JSON object ``. The caller passes the property name, so give the function a `field: &str` parameter for the message prefix. Update the `Produces` signature in your report.
- [ ] **Step 4: write the handler tests**, in `output_values.rs`, with `super::super::testing::input`, as `ret.rs` does:
  - `{"values": {"a": "x", "b": "{{ 1 + 1 }}"}}` → SUCCESS, with outputs `{"values": {"a": "x", "b": "2"}}`;
  - no `values` → SUCCESS, with outputs `{"values": {}}`;
  - a value that fails to render (`"{{ nope.nope }}"`) → FAILED, with a message containing `values.`;
  - a non-map `values` (`[1, 2]`) → FAILED.
- [ ] **Step 5: implement `OutputValues`**: `TYPE_ID = "io.kestra.plugin.core.output.OutputValues"`, and `pub(crate) struct Spec { values: Option<serde_json::Value> }`. Read `values` raw, not as a `Property`, because `render_map` takes the raw value. `cancel` is a no-op. Register it in `with_core_tasks`, add its `TASK_TYPES` row (FQCN, no alias, no references), add its `measure.rs` arm, and regenerate `property-treatment.json`.
- [ ] **Step 6: register it in the oracle driver** (`driver.rs`'s `registry`, beside `Size`) and in `handler_surface.py`'s `W3B_HANDLERS`.
- [ ] **Step 7: run the ratchet**: `cargo test -p egret-workflow --test executor`. The entries whose only unported type was `OutputValues` now publish. The failures name each entry that left the refused list or that now matches. For each one:
  - remove its `PublishedButRefused` row in `executor/mod.rs`;
  - if it now matches the golden, remove it from `baseline.txt`;
  - if it publishes and still mismatches, add it to `baseline.txt` with a one-line comment naming the measured cause (the first differing field the ratchet prints);
  - for an entry that still has another group-C type (`assert`, `set-variables`, …), update its `cause:` text to name only the types still missing, if the ratchet asks for it.
  
  Then update `driver.rs`'s `LIVE` and `UNKNOWN_TYPE_REFUSED` with dated doc lines (the kill-and-exit Task 3 style), and `handler_surface.py`'s baseline count. Regenerate `handler-surface.txt` by running `python3 handler_surface.py` as its docstring says. Repeat until the ratchet is green. Record before and after: mismatching, baselined, refused, live.
- [ ] **Step 8: run the whole crate once**: `cargo test -p egret-workflow` on the private Postgres. Tests that pin the registry or the served set (`tests/served.rs`, `tests/not_served.rs`, `spec::schema`, `runtime`) may need the new type. Fix each by adding the type where the test enumerates types, never by loosening an assertion.
- [ ] **Step 9: commit**: `feat(workflow): the OutputValues task`.

---

### Task 3: `Assert` (spec §2.2)

**Files:**
- Create: `crates/egret-workflow/src/worker/handlers/assert.rs`
- Modify: the same registration files as Task 2: `handlers/mod.rs`, `worker/registry.rs` `with_core_tasks`, `spec/registry.rs`, `measure.rs` + `property-treatment.json`, `driver.rs`, `handler_surface.py` + `handler-surface.txt`, `baseline.txt`, `executor/mod.rs`'s `PublishedButRefused`

**Interfaces:**
- Consumes: `crate::executor::flowable::branch::is_truthy(&str) -> bool` (`executor/flowable/branch.rs:93`); `TaskInput::log.line(LogLevel, &str)` (used by `handlers/log.rs`).
- Produces: `crate::worker::handlers::assert::{TYPE_ID, Assert, Spec}`.

- [ ] **Step 1: write the tests**, in `assert.rs`. Read the log lines back with `super::testing::recorded(TYPE_ID, spec) -> (TaskInput, Arc<Recorder>)` and `Recorder::texts() -> Vec<(LogLevel, String)>` (`worker/handlers/mod.rs:142-200`).
  - All true (`["{{ 1 == 1 }}", "yes"]`) → SUCCESS, no outputs, one INFO line `2 succeeded, 0 failed`.
  - One false (`["{{ 1 == 2 }}", "x"]`) → FAILED with the message exactly `1 assertion failed!`, one ERROR line ``Assertion `{{ 1 == 2 }}` failed!``, and an INFO line `1 succeeded, 1 failed`.
  - Two false, with `errorMessage: "because {{ 'x' }}"` → FAILED with the message exactly `"2 assertions failed!\nbecause x"`.
  - A condition that fails to render (`"{{ nope.nope }}"`) → counts as failed, with an ERROR line starting ``Assertion `{{ nope.nope }}` failed, failed to render ` ``.
  - The falsy set is `is_truthy`'s: `"false"`, `"0"`, `"-0"`, `""` and whitespace-only are falsy; `"False"` is truthy. Assert one falsy and one truthy edge.
  - No `conditions` → FAILED with a message naming `conditions`.
- [ ] **Step 2: run, and expect a compile failure**: `cargo test -p egret-workflow --lib worker::handlers::assert`.
- [ ] **Step 3: implement.** `Spec { conditions: Option<Vec<String>>, error_message: Option<Property<String>> }`, with `#[serde(rename = "errorMessage")]`. Kestra's `conditions` is `@PluginProperty(dynamic = true) List<String>`, so each entry is rendered with `render_str`. Evaluate every condition, collecting counts. Log each failure as it happens, then the INFO count line, then return. If `errorMessage` fails to render, the run is FAILED with that render error.
- [ ] **Step 4: register it** in the same places as Task 2 Steps 5-6.
- [ ] **Step 5: ratchet and bookkeeping**, exactly as Task 2 Step 7. `valids/assert` and `valids/if-in-parallel` are the entries to watch. Record before and after.
- [ ] **Step 6: whole crate once**, as Task 2 Step 8.
- [ ] **Step 7: commit**: `feat(workflow): the Assert task`.

---

### Task 4: `PurgeLogs` (spec §2.3)

**Files:**
- Modify: `crates/egret-workflow/src/storage/nsfiles/access.rs` (`Resource::Logs`: `noun()` → `"logs"`, `own_noun()` → `"logs"`; a unit test beside `a_flow_purges_its_own_and_child_namespaces_on_a_dot_boundary`, `:336`)
- Modify: `crates/egret-workflow/src/logs/repo.rs` (`LogRepo::purge`)
- Create: `crates/egret-workflow/src/worker/handlers/purge_logs.rs` (handler plus `pub fn register(handlers: &mut Handlers, pool: PgPool)`)
- Modify: `crates/egret-workflow/src/worker/handlers/mod.rs`, `crates/egret-workflow/src/runtime.rs` (`handlers_for`, beside `kv::register` at about `:735`), `spec/registry.rs`, `measure.rs` + `property-treatment.json`, `oracle-exec/driver.rs` (`registry`: call `purge_logs::register`), `handler_surface.py` + `handler-surface.txt`, `baseline.txt`, `executor/mod.rs`'s `PublishedButRefused`
- Test: `crates/egret-workflow/tests/purge_logs.rs` (new; DB)

**Interfaces:**
- Consumes:
  - `storage::nsfiles::access::{authorize, Actor, Resource, Access}`;
  - `worker::handlers::namespace::flow_namespace(&TaskInput) -> Option<String>`, which is `pub(super)`: widen it to `pub(crate)`, or move it to `handlers/mod.rs`;
  - `logs::LogLevel::{parse, as_i16}`.
- Produces: `LogRepo::purge(&self, q: &PurgeQuery) -> egret_core::Result<u64>`, where

```rust
pub struct PurgeQuery {
    pub workspace_id: Uuid,
    /// Each grant's namespace; a row matches when its execution's namespace is
    /// one of these or a dot-child of one (`ns = g OR ns LIKE g || '.%'` with
    /// `g` escaped for LIKE: `_` and `%` and `\`).
    pub namespaces: Vec<String>,
    pub flow_key: Option<String>,
    pub execution_id: Option<Uuid>,
    pub levels: Option<Vec<LogLevel>>,
    pub start: Option<chrono::DateTime<chrono::Utc>>,
    pub end: chrono::DateTime<chrono::Utc>,
    /// `None`: one statement. `Some(n)`: repeat `LIMIT n` batches until one deletes fewer than n.
    pub batch: Option<i64>,
}
```

- [ ] **Step 1: the access rule test first.** In `access.rs`, `authorize(ws, Actor::Flow { namespace: "a.b" }, t, Resource::Logs, Access::Purge)`:
  - is `Ok` for `a.b`, `a.b.c` and `a.b.c.d`;
  - is `Err` for `a.bc`, `a` and `b`, with a message containing `may not purge logs of namespace '<t>'`.
  
  Run it (`cargo test -p egret-workflow --lib storage::nsfiles::access`) and expect a compile failure. Then add the variant, fix the two matches, and run again: PASS. Check that `storage/mod.rs:170` (`grant.resource() == Resource::Kv`) needs no change.
- [ ] **Step 2: the repo tests**, in `tests/purge_logs.rs`, on the private Postgres. Seed two workspaces. In workspace W1, add executions in namespaces `a.b`, `a.b.c`, `a.bc` and `z`, with flow keys `f1` and `f2`, and log lines at every level, with `ts` spread over three hours. In W2, add an execution in `a.b`. Use the existing helpers that `tests/repo.rs` or `tests/execution_repo.rs` use to create a workspace, a flow and an execution. Cases:
  - `namespaces = ["a.b"]` deletes `a.b` and `a.b.c` lines and leaves `a.bc`, `z` and W2's untouched. Count each remainder.
  - A namespace containing `_` (`a_b`) does not match `axb` (the LIKE escape: the `sql-like-underscore` lesson).
  - `flow_key`, `execution_id` and `levels` each narrow the delete.
  - `[start, end)`: a line at exactly `end` survives, and a line at exactly `start` is deleted.
  - `batch = Some(2)` deletes the same rows as `None` and returns the same total.
  - The return value equals the rows actually gone.
- [ ] **Step 3: run, and expect failure**: `DATABASE_URL=… cargo test -p egret-workflow --test purge_logs`.
- [ ] **Step 4: implement `LogRepo::purge`.**
  - The statement: `DELETE FROM workflow.task_logs l USING workflow.executions e WHERE l.execution_id = e.id AND e.workspace_id = $1 AND (…namespace any-of…) AND ($3::text IS NULL OR e.flow_key = $3) …`, with `ts >= start` (when set) and `ts < end`, and `level = ANY($n::int2[])` when levels are set.
  - The batch form: `DELETE … WHERE l.id IN (SELECT l.id … LIMIT $n)` in a loop, the way `prune_batch` does (`logs/repo.rs:291`).
  - Bind every value. Build no SQL from strings: this crate's `query()` takes `&'static str` (the sqlx 0.9 rule). Express the namespace list as `EXISTS (SELECT 1 FROM unnest($2::text[]) g WHERE e.namespace = g OR e.namespace LIKE <escaped g> || '.%' ESCAPE '\')`, where the escaping is done in SQL with `replace(replace(replace(g, '\', '\\'), '%', '\%'), '_', '\_')`.
  - Run the tests: PASS.
- [ ] **Step 5: the handler tests**, in `purge_logs.rs` (unit, no DB, through a fake or by testing a pure `plan(spec, own_namespace) -> Result<PurgeQuery-without-workspace, String>` function, so that the scope decision is testable without Postgres):
  - no `namespace` → `namespaces = [own]`;
  - `namespace: own.child` → `[own.child]`;
  - `namespace: other` → `Err` naming `may not purge logs`;
  - no `endDate` → `Err` naming `endDate`;
  - an unparseable date → `Err` naming the property;
  - `logLevels: [INFO, ERROR]` → those two levels, and `[info]` → `Err` (case-sensitive, as `LogLevel::parse`);
  - `executionId` not a UUID → `Err` naming `executionId`;
  - `purgeExecutionLogs: false` → no query (`Ok(None)`): nothing is deleted, and the counts are 0;
  - `batchSize: 0` or negative → `Err` naming `batchSize`.
  
  **Dates** are rendered strings parsed as Java's `ZonedDateTime.parse` reads them, which is RFC 3339 with an optional trailing `[Region/City]`. Strip a trailing `[...]`, then `DateTime::parse_from_rfc3339`. Test it with the text egret's own `{{ now() | dateAdd(2, 'HOURS') }}` renders: render it in the test through `render_str`, and do not hand-write it.
- [ ] **Step 6: implement the handler.**
  - `TYPE_ID = "io.kestra.plugin.core.log.PurgeLogs"`.
  - `Spec` has `namespace`, `flow_id` (rename `flowId`), `execution_id` (rename `executionId`), `log_levels` (rename `logLevels`, `Option<Property<Vec<String>>>`), `start_date`, `end_date`, `purge_execution_logs`, `purge_non_execution_logs` and `batch_size`, all `Option<Property<…>>` with the camelCase renames.
  - The handler takes the flow's namespace through `flow_namespace(task)`. With none, it FAILs: "this run is not a task run of a flow's execution, so it has no namespace to purge logs from".
  - It authorizes the target with `Resource::Logs`, builds the `PurgeQuery`, calls `LogRepo::purge`, and returns `Outcome::succeeded_with(json!({"count": n, "executionLogsCount": n, "nonExecutionLogsCount": 0}))`.
  - It writes one INFO line naming the count and the scope.
  - Register it through `purge_logs::register` in `runtime::handlers_for`, with a comment in the style of the W9 ones there, and in `driver.rs`'s `registry`. Add the `TASK_TYPES` row, the `measure.rs` arm, and `handler_surface.py`.
- [ ] **Step 7: one DB test end to end through the worker.** Add a case to `tests/purge_logs.rs` that runs a one-task flow (`PurgeLogs` with `endDate: "{{ now() | dateAdd(1, 'HOURS') }}"`) through the same harness `tests/worker.rs` uses to run a flow to a terminal state. Before it runs, seed old lines on another execution of the same namespace. Assert:
  - the run is SUCCESS;
  - `outputs.count` equals the seeded lines (plus the purging execution's own lines written before the delete, if any: measure and state which);
  - a sibling namespace's lines are untouched.
- [ ] **Step 8: ratchet and bookkeeping**, as Task 2 Step 7. Expected:
  - `purge_logs_execution_only`, `purge_logs_no_arguments`, `purge_logs_trigger_only` and `purge_logs_with_batch_size` publish and match, or are baselined with their measured cause;
  - `purge_logs_full_arguments` FAILS (namespace `purge.namespace` from `io.kestra.tests`) and goes into `baseline.txt` with this comment, verbatim:

```text
# Permanent, not a gap (group C spec §2.3, the user's ruling 2026-10-18): a flow
# purges logs only of its own namespace and its children; Kestra OSS checks nothing.
```

- [ ] **Step 9: whole crate once**, as Task 2 Step 8.
- [ ] **Step 10: commit**: `feat(workflow): the PurgeLogs task, scoped to the flow's namespace`.

---

### Task 5: documentation and status

**Files:**
- Modify: `docs/en/specs/2026-10-18-workflow-core-tasks-design.md` and `docs/vi/specs/2026-10-18-workflow-core-tasks-design.md` (the `Status:` line: "C1 built and verified <date>", with the ratchet before/after counts from Tasks 2-4, and a dated note in §2 for every place the build differs from the spec)
- Modify: `crates/egret-workflow/oracle-exec/README.md` (the coverage statement's counts, which the ratchet asserts, and a short "Group C1" section listing what banked and what was baselined with which cause)
- Modify: `docs/en/specs/2026-09-01-workflow-programme-design.md` §9 and its VI copy: one dated line, "2026-10-18, group C1: OutputValues, Assert, PurgeLogs (#<PR>)". The VI §9 is also missing the 10-16 and 10-17 notes the EN copy has. Copy them across, translated.

- [ ] **Step 1:** write the edits above. In every count, state where it came from: the ratchet's printout, or `python3 handler_surface.py`.
- [ ] **Step 2:** run `cargo test -p egret-workflow --test executor` once more. README numbers that the suite asserts must match.
- [ ] **Step 3: commit**: `docs(workflow): group C1 status, oracle coverage and programme notes`.
