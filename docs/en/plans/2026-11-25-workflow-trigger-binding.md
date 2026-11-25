# `trigger.*` binding in task renders (package 11a) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** When a flow is started by a trigger, bind the trigger's variables into the fired flow's task render context so `{{ trigger.body }}` / `{{ trigger.date }}` / `{{ trigger.outputs.x }}` render.

**Architecture (revised after Task 0):** egret ALREADY persists a trigger record in the existing `workflow.executions.execution_trigger` jsonb column (migration 0140), shaped `{ "id": <trigger id>, "type": <fqcn>, "variables": { ...per-type... } }`. Schedule (`fire.rs schedule_trigger_record` → `variables: {date,next,previous}`) and Flow (`flow_trigger/process.rs flow_trigger_record` → `variables: {state,flowId,namespace,executionId,startDate,endDate?,lastTaskId?,executionLabels,outputs?}`) already populate it; subflow/manual/webhook leave it NULL. (CORRECTION, Task 1: the manual-replay path `worker/copy.rs:219` sets `trigger: None`, so replay does NOT yet carry it — Task 2 adds that, for Kestra `Execution.java:422` parity.) The ONLY gaps are: (1) `build_context` never reads it, so tasks can't see `{{ trigger.* }}`; (2) the Webhook route sets `trigger: None`. So 11a = **bind the existing column in build_context + add Webhook capture**. No new column/field; ONE migration is needed (Task 2) only to narrow two fire-uniqueness indexes so a replay may carry the trigger.

**Tech Stack:** Rust, `crates/egret-workflow`; sqlx/Postgres.

## Global Constraints
- Chat Vietnamese; code/commits/docs English. Conventional Commits; `Closes #N` in PR body only.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends with the Claude Code line.
- Never commit secrets/scratch. Stage by explicit path; never `git add -A`/`target*/`.
- ONE heavy Rust build at a time, foreground in subagents. Merge on green CI, no `--admin`, `cargo fmt --all --check` first.
- **Scoped `egret-workflow` runs MUST include `--test routes --test execution_timestamps`.**
- `secret()` interaction: NONE (egret trigger records aren't secret-typed; the Flow record already masks SECRET outputs per #6 D9 — don't touch 10a).
- **BINDING GUARD 1 (gate on fire):** bind `trigger` ONLY when `execution_trigger` is non-null (a genuinely trigger-fired execution). NEVER bind an unconditional/empty `trigger` root. Proof obligation: a STANDALONE (API POST) run of a `{{ trigger.* }}`-reading flow MUST still fail-render. Task 0 measured: the oracle driver runs every corpus flow standalone and never fires a trigger, so `execution_trigger` is NULL there → task fails → matches golden FAILED; under this gate ZERO ratchet entries / census constants (LIVE 299, VALIDATE_REFUSED 21, executor_matches_golden) move. An unconditional/empty bind would regress 5 flows FAILED→SUCCESS vs golden — forbidden.
- **BINDING GUARD 2 (context b only):** bind `trigger` ONLY in the fired flow's TASK render (`build_context` via `load_with`). Do NOT bind it in the Flow-trigger's OWN `inputs:`/`when:` render (render.rs ~1086-1097) — that stays unbound (genuine Kestra parity, w13). Leave that path unchanged.
- **Record→render transform:** Kestra binds `trigger` = the record's `variables` map with `_context={id,type}` added (`RunVariables.java`: `triggerVariables.put("_context", …); builder.put("trigger", triggerVariables)`). So `{{ trigger.date }}` → `variables.date`, `{{ trigger._context.type }}` → the fqcn.

## File Structure
- `crates/egret-workflow/src/worker/render.rs` — `Surface` struct gains `trigger: Value`; `load_with` SELECT adds `execution_trigger` + builds the transformed value; `build_context` inserts the `trigger` key when non-null.
- `crates/egret-workflow/src/routes.rs` (the webhook execution-creation site ~4587) + a new `webhook_trigger_record(...)` helper (next to the webhook Output in `trigger/webhook.rs`) — set `NewExecution.trigger` for webhook fires.
- Tests: `tests/trigger_binding.rs` (new); unit tests in render.rs.

---

### Task 0: Measurement — COMPLETE (recorded in the ledger)
Outcome A — census STABLE under the non-null gate. Discovered the existing `execution_trigger` column + records (this revised plan). No code. (Kept as a task marker; do not re-dispatch.)

---

### Task 1: Bind `trigger` in `build_context` from `execution_trigger`

**Files:** `crates/egret-workflow/src/worker/render.rs` (`Surface` struct ~93/111-129; `load_with` SELECT ~220; insert block ~807-812). Test: `tests/trigger_binding.rs`.

**Interfaces — Produces:** `{{ trigger.* }}` renders in a fired flow's tasks (Schedule + Flow immediately, since they already populate the column; Webhook after Task 2).

**Record→render transform** (put in a small helper, e.g. `fn trigger_binding(record: Value) -> Value`):
- input: the `execution_trigger` record `{"id":..,"type":..,"variables":{..}}` (or `Value::Null`).
- output: `Value::Null` when input is null/missing `variables`; else the `variables` object with an added key `_context` = `{"id": record.id, "type": record.type}`.

- [ ] **Step 1: Failing tests** in `tests/trigger_binding.rs` (DB-gated; mirror the existing schedule-fire + flow-trigger test harnesses — grep `schedule_trigger_record`/`flow_trigger_record` tests + `w12-schedule-trigger.json`/`w13-flow-trigger.json` usage for how a fired execution is set up):
  - a Schedule-fired execution whose task reads `{{ trigger.date }}` renders the fire date and `{{ trigger._context.type }}` = the Schedule fqcn;
  - a Flow-trigger-fired execution whose task reads `{{ trigger.outputs.x }}` / `{{ trigger.executionId }}` renders the source values;
  - a replay of a Schedule-fired execution still renders `{{ trigger.date }}` (column copied by replay — should pass once binding exists, proving replay-survival);
  - a standalone (manual/API) execution whose task reads `{{ trigger.x }}` FAILS to render (execution_trigger NULL → no key) — the GUARD-1 proof.
  Also a render.rs unit test for `trigger_binding`: a null record → Null (no key inserted); a `{id,type,variables:{date}}` record → `{date, _context:{id,type}}`.
- [ ] **Step 2: Run — FAIL** (`{{ trigger.* }}` unbound today).
- [ ] **Step 3: Implement.**
  - `Surface` (render.rs ~111-129): add `trigger: Value,`.
  - `load_with` (render.rs ~220): add `execution_trigger` to the SELECT column list; after building `vars`, `let trigger = trigger_binding(row.get::<Option<Value>, _>("execution_trigger").unwrap_or(Value::Null));` and set it on the `Surface`.
  - Insert (after render.rs:812 `cx.insert("execution", …)`):
    ```rust
    if !self.trigger.is_null() {
        cx.insert("trigger", self.trigger.clone());
    }
    ```
  - Add the `trigger_binding` helper (same file).
  - Do NOT change the Flow-trigger own-inputs/when render (render.rs ~1086-1097).
- [ ] **Step 4: Run — PASS** (+ `--test routes --test execution_timestamps` since the loader SELECT changed).
- [ ] **Step 5: Commit** `feat(workflow): bind trigger.* in task render context from execution_trigger`.

---

### Task 2: Replay copies `execution_trigger` (replay-survival parity)

**Files:** `crates/egret-workflow/src/worker/copy.rs` (~219, the replay `NewExecution`, currently `trigger: None`). Un-ignore the replay test in `tests/trigger_binding.rs`.

**Why:** Task 1 found manual replay sets `trigger: None`, so a replayed trigger-fired execution loses `{{ trigger.* }}`. Kestra carries the trigger verbatim on replay (`Execution.java:422`). The `execution_trigger` column is coupled by a CHECK (migrations 0140/0157) to `scheduled_date` and `flow_trigger_event_id`, so they must be copied together.

- [ ] **Step 1: Un-ignore** the `#[ignore]`d replay-survival test (replay of a Schedule-fired execution still renders `{{ trigger.date }}`). Add a Flow-replay case too (`{{ trigger.outputs.x }}` survives).
- [ ] **Step 2: Run — FAIL** (replay drops the trigger).
- [ ] **Step 3a: Migration (next-free, ~0243).** Recreate `workflow_executions_one_per_schedule_tick` (current def in migration 0192) and `workflow_executions_one_per_flow_trigger_event` (0157) with an added `AND replayed_from_execution_id IS NULL` in their WHERE, preserving all existing columns/conditions. Rationale: these are one-per-FIRE guarantees; a replay is an intentional re-run (mirrors 0192 excluding backfill_id). Without this, copying the coupled columns collides with these uniques.
- [ ] **Step 3: Implement.** In the replay `NewExecution` (`copy.rs` `replay_locked` ~219), copy the ORIGINAL execution's `execution_trigger`, `scheduled_date`, and `flow_trigger_event_id` together (read them from the original row the replay already loads, or add them to that load). Set `trigger: <original execution_trigger>`, `scheduled_date: <original>`, `flow_trigger_event_id: <original>`. Verify the CHECK constraint is satisfied (copying all three coupled fields together keeps it valid — if the CHECK requires a specific combination, match what the original row had).
- [ ] **Step 4: Run — PASS** (+ `--test routes --test execution_timestamps`). Confirm a NON-triggered execution's replay still has NULL trigger (don't fabricate one).
- [ ] **Step 5: Commit** `feat(workflow): replay carries execution_trigger (trigger.* survives replay)`.

---

### Task 3: Webhook binding — CONFIRMATION TESTS (capture already exists)

> Task-3 correction: the real webhook route is `src/webhook_routes.rs`, which ALREADY builds `Output` -> `ExecutionTrigger{id,type,variables}` -> `start_execution_with_trigger(Some(..))`, so `execution_trigger` is already populated for webhook fires (existing test `a_fired_webhook_stores_the_trigger_record`). `routes.rs:4587` was the multipart start, not the main route. So Task 3 adds CONFIRMATION tests only — no production code — proving the Task-1 binding renders webhook `trigger.*`.

### Task 3 (original, superseded): Webhook capture

**Files:** `crates/egret-workflow/src/trigger/webhook.rs` (add `webhook_trigger_record`); `crates/egret-workflow/src/routes.rs` (the webhook execution creation ~4587, currently `trigger: None`).

**Interfaces — Consumes:** the `execution_trigger` column + Task 1 binding. **Produces:** `{{ trigger.body }}` etc. render for webhook-fired executions.

- [ ] **Step 1: Failing integration test**: a webhook-fired execution whose task reads `{{ trigger.body }}` renders the posted JSON body; `{{ trigger.headers.x }}` renders a header; a GET with no body → `{{ trigger.body }}` fails to render (no `body` key), Kestra parity. (Mirror the existing webhook-fire test harness — grep webhook route tests.)
- [ ] **Step 2: Run — FAIL** (webhook sets `trigger: None`).
- [ ] **Step 3: Implement.** Add `pub(crate) fn webhook_trigger_record(trigger_id: &str, out: &Output) -> serde_json::Value` returning `{"id": trigger_id, "type": WEBHOOK_TYPE, "variables": <out>}` where `<out>` is the existing webhook `Output` serialized (its `skip_serializing_if = "Option::is_none"` keeps absent fields out, so a GET has no `body`). At routes.rs ~4587 replace `trigger: None` with `trigger: Some(webhook_trigger_record(<trigger id>, <the Output already built for the webhook inputs render>))`. (Grep the webhook route for where it builds the `Output` — reuse it; do not recompute.)
- [ ] **Step 4: Run — PASS** (+ scoped binaries).
- [ ] **Step 5: Commit** `feat(workflow): populate execution_trigger for webhook fires (trigger.body/headers/parameters)`.

---

### Task 4: Docs (repo egret-data/docs, new branch docs/workflow-trigger-binding)

- [ ] **Step 1:** In `platform/workflows/flows/expressions.md` (or the triggers doc) document `{{ trigger.* }}` availability per type (Schedule `date`/`next`/`previous`; Webhook `body`/`headers`/`parameters`; Flow `outputs`/`executionId`/`state`/…; `_context.id`/`_context.type`), that it is absent for non-triggered executions, and that replay keeps it.
- [ ] **Step 2:** Build docs (or rely on the docs PR `build` gate).
- [ ] **Step 3: Commit** `docs(workflow): {{ trigger.* }} in task expressions`.

---

## Self-Review
- **Spec coverage:** persist → already exists (execution_trigger, 0140); Schedule/Flow capture → already exist; Webhook capture → Task 2; bind → Task 1; replay-survival → Task 2 (copy.rs copies execution_trigger + coupled fields); subflow/manual NULL → free (they set None; Task 1 tests it); Leaf NULL → free; no refusal lift → nothing lifts; Task-0 census → done; docs → Task 3. No gap.
- **Placeholder scan:** render-side code exact; the webhook `Output` reuse points at the existing route-built value (grep-located), not "TBD".
- **Type consistency:** `Surface.trigger: Value`; `trigger_binding(Value)->Value`; `webhook_trigger_record(&str,&Output)->Value`; the record shape `{id,type,variables}` is consistent with the existing schedule/flow records.
