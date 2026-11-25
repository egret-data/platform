# `trigger.*` binding in task renders (package 11a) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** When a flow is started by a trigger, bind the trigger's variables into the fired flow's task render context so `{{ trigger.body }}` / `{{ trigger.date }}` / `{{ trigger.outputs.x }}` render.

**Architecture:** Persist each fired execution's trigger output in a new `executions.trigger` jsonb column (migration 0243); each of the three typed fire paths writes its already-computed Output (+ `_context`) into it at execution creation; `build_context` binds a top-level `trigger` render key from it. Binding only — no refusal lifted.

**Tech Stack:** Rust, `crates/egret-workflow`; sqlx/Postgres; migration under `migrations/`.

## Global Constraints

- Chat Vietnamese; code/commits/docs English. Conventional Commits; `Closes #N` in PR body only.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends with the Claude Code line.
- Never commit secrets/scratch. Stage by explicit path; never `git add -A`/`target*/`.
- ONE heavy Rust build at a time, foreground in subagents. Merge on green CI, no `--admin`, `cargo fmt --all --check` first.
- **Scoped `egret-workflow` runs MUST include `--test routes --test execution_timestamps`.**
- **Adding a column to `workflow.executions` means updating EVERY SELECT that loads it into an `Execution`/render context** — Task 0 enumerates them (exhaustiveness: a missed SELECT silently drops the value). A chart/golden change must `cargo test -p egret-config` (the 10b lesson: the exact-RBAC golden).
- Migration next-free = **0243** (latest shipped 0242). Reconfirm at build time and renumber above main + open PRs if taken (migration numbers are a shared namespace).
- `secret()` interaction: NONE — egret trigger outputs are not secret-typed; do not touch 10a machinery.

## File Structure
- `migrations/0243_execution_trigger.sql` (new) — `ALTER TABLE workflow.executions ADD COLUMN trigger jsonb`.
- `crates/egret-workflow/src/worker/render.rs` — `Surface` struct gains `trigger: Value`; `load_with` SELECT + build; `build_context` inserts the `trigger` key.
- `crates/egret-workflow/src/worker/spawn.rs`, `replay.rs`, and the trigger execution-creation seam (`start_execution_in_with` / `NewExecution`) — thread the trigger payload into the INSERT (None for subflow/manual; computed for the 3 triggers; copy for replay).
- `crates/egret-workflow/src/trigger/{webhook,schedule,flow}.rs` + `flow_trigger/process.rs` + `scheduler/` — produce the per-type trigger payload at fire.
- Tests: `tests/trigger_binding.rs` (new); unit tests in render.rs.

---

### Task 0: Measurement (census + exact capture wiring) — load-bearing, mostly no production code

**Why:** pin the facts the later tasks encode, on the post-10b tree.

- [ ] **Step 1 — Census/Banks.** From `crates/egret-workflow`:
```bash
grep -rn "trigger\." tests/ oracle-exec/ | grep -iE "\{\{ *trigger" | head -40
grep -rln "trigger" oracle-exec/ tests/executor.rs
```
Determine whether any corpus/oracle flow references `{{ trigger.* }}` in a TASK (not in a trigger's own `inputs:`) and currently sits in a failed/refused bucket. Record, with file:line, outcome **(A) census stable** (no such flow; expected) or **(B) census moves** (list flows + decision). If (B), surface to the controller before Task 1.
- [ ] **Step 2 — Enumerate every executions SELECT that feeds a render/Execution.** Grep so no reader of the new column is missed:
```bash
grep -rn "FROM workflow.executions" src/ | grep -iE "SELECT" | head -40
```
List each site that must add `trigger` to its column list (the render loader `worker/render.rs load_with` at minimum; note any API/DTO loader that should expose it — but 11a only needs the render path to bind it; API exposure is optional and, if added, must be listed).
- [ ] **Step 3 — Pin the per-type Output shape + fire-path creation seam.** Confirm, with file:line:
  - Webhook `Output` fields (measured: `{body?: Value, uri?: String, headers?: Map<String,Vec<String>>, parameters?: Map<String,Vec<String>>}`, `trigger/webhook.rs`); where the webhook route builds it and where it creates the execution.
  - Flow trigger Output fields + where `flow_trigger::process` creates the fired execution (`start_execution_in_with` / `NewExecution`, imported at `flow_trigger/process.rs:35`).
  - Schedule: the fire site, `schedule_date`, and egret's cron API to compute `next`/`previous`.
  - The shared `NewExecution` struct + `start_execution_in_with` signature, and the INSERT column lists in `spawn.rs` (~417) and `replay.rs` (~244).
- [ ] **Step 4 — Write the findings to the task report.** No source changes (unless a corpus note). This report is the source of exact field names / signatures for Tasks 1-6.

---

### Task 1: Migration + thread the `trigger` column through execution creation (no binding yet)

**Files:** Create `migrations/0243_execution_trigger.sql`; modify the execution-creation seam (`NewExecution`/`start_execution_in_with`) and the INSERTs in `spawn.rs`, `replay.rs`.

**Interfaces — Produces:** executions rows carry a `trigger jsonb NULL`; `NewExecution` gains a `trigger: Option<serde_json::Value>` field (None for subflow/manual).

- [ ] **Step 1: Migration.**
```sql
-- migrations/0243_execution_trigger.sql
ALTER TABLE workflow.executions ADD COLUMN trigger jsonb;
COMMENT ON COLUMN workflow.executions.trigger IS
  'Package 11a: the trigger output + _context bound as {{ trigger.* }} in the fired flow''s task renders; NULL for non-triggered executions.';
```
- [ ] **Step 2: Add `trigger: Option<Value>` to `NewExecution`** (the struct the trigger fire paths pass to `start_execution_in_with`), defaulting to `None`. Thread it into that function's INSERT column list (`trigger`) + bind value (NULL when None).
- [ ] **Step 3: spawn.rs (subflow child) — `trigger` stays NULL** (a subflow child is not trigger-started). Add the column to its INSERT column list with a literal `NULL` (or omit — but add explicitly so the column list is complete and self-documenting).
- [ ] **Step 4: replay.rs — copy the original execution's `trigger`** into the replay INSERT (`(SELECT trigger FROM workflow.executions WHERE id = <original>)`), so a replay keeps `{{ trigger.* }}`.
- [ ] **Step 5: DB-gated test** that a row inserted via each path has the expected `trigger` (NULL for subflow; copied for replay). Run with the worktree DB env.
- [ ] **Step 6: Commit** `feat(workflow): executions.trigger column + thread through execution creation` (migration + the seam).

---

### Task 2: Bind `trigger` in `build_context`

**Files:** `crates/egret-workflow/src/worker/render.rs` (`Surface` struct ~93/111-129; `load_with` ~212; the insert block ~807-812).

**Interfaces — Consumes:** the `trigger` column (Task 1). **Produces:** `{{ trigger.* }}` renders in tasks when the column is non-null.

- [ ] **Step 1: Failing unit test** (render.rs `#[cfg(test)]`): a `Surface` whose `trigger` is `json!({"date":"2026-01-02T03:04:05Z","_context":{"id":"sched","type":"io.kestra.plugin.core.trigger.Schedule"}})` makes `build_context` produce a context where `{{ trigger.date }}` renders; a `Surface` with `trigger: Value::Null` inserts NO `trigger` key (reading `{{ trigger.x }}` fails as today).
- [ ] **Step 2: Run — FAIL** (no `trigger` key today).
- [ ] **Step 3: Add the field + SELECT + insert.**
  - `Surface` (render.rs ~111-129): add `trigger: Value,`.
  - `load_with` (render.rs ~221): add `trigger` to the SELECT column list; after building `vars`, `let trigger = row.get::<Option<Value>, _>("trigger").unwrap_or(Value::Null);` and set it on the `Surface`.
  - Insert (after render.rs:812 `cx.insert("execution", …)`):
    ```rust
    if !self.trigger.is_null() {
        cx.insert("trigger", self.trigger.clone());
    }
    ```
- [ ] **Step 4: Run — PASS.** Also run `--test routes --test execution_timestamps` (the loader SELECT changed).
- [ ] **Step 5: Commit** `feat(workflow): bind trigger.* in task render context`.

---

### Task 3: Schedule capture — `{date, next, previous}` + `_context`

**Files:** the scheduler fire path (Task 0 pins it) + `trigger/schedule.rs`.

- [ ] **Step 1: Integration test** (`tests/trigger_binding.rs`): a schedule-fired execution whose task reads `{{ trigger.date }}` renders the fire date; `{{ trigger._context.type }}` = the Schedule FQCN. (Mirror an existing schedule-fire test harness — Task 0 names it.)
- [ ] **Step 2: Run — FAIL.**
- [ ] **Step 3: Build the payload at fire** — `json!({"date": <schedule_date>, "next": <next cron occurrence>, "previous": <previous>, "_context": {"id": <trigger id>, "type": <fqcn>}})`, computing next/previous from the trigger's cron via egret's existing cron machinery (Task 0 names the API). Pass it as `NewExecution.trigger = Some(..)`.
- [ ] **Step 4: Run — PASS** (+ scoped binaries).
- [ ] **Step 5: Commit** `feat(workflow): bind Schedule trigger.date/next/previous`.

---

### Task 4: Webhook capture — `{body?, headers, parameters}` + `_context`

**Files:** the webhook route/fire path (`trigger/webhook*`, Task 0 pins the creation site).

- [ ] **Step 1: Integration test**: a webhook-fired execution whose task reads `{{ trigger.body }}` renders the posted JSON body; `{{ trigger.headers.x }}` renders a header; a GET with no body → `{{ trigger.body }}` fails to render (no `body` key), Kestra parity.
- [ ] **Step 2: Run — FAIL.**
- [ ] **Step 3: Store the webhook `Output`** (already built to render the trigger's `inputs:`) + `_context` as `NewExecution.trigger`. Reuse the existing `Output` struct (`{body?,uri?,headers?,parameters?}`); serialize with its existing `skip_serializing_if = "Option::is_none"` so absent fields stay absent.
- [ ] **Step 4: Run — PASS** (+ scoped binaries).
- [ ] **Step 5: Commit** `feat(workflow): bind Webhook trigger.body/headers/parameters`.

---

### Task 5: Flow-trigger capture — Flow Output + `_context`

**Files:** `flow_trigger/process.rs` (+ `trigger/flow.rs` Output).

- [ ] **Step 1: Integration test**: a flow-trigger-fired execution whose task reads `{{ trigger.outputs.x }}` and `{{ trigger.executionId }}` renders the source execution's output/id.
- [ ] **Step 2: Run — FAIL.**
- [ ] **Step 3: Store the Flow Output** (`{executionId, namespace, flowId, flowRevision, state, startDate, endDate, firstFailedTaskId, lastTaskId, executionLabels, outputs}`) + `_context` as `NewExecution.trigger` in `start_execution_in_with`'s caller in `process.rs`.
- [ ] **Step 4: Run — PASS** (+ scoped binaries).
- [ ] **Step 5: Commit** `feat(workflow): bind Flow trigger.outputs/executionId/...`.

---

### Task 6: Replay/restart copy + subflow/manual NULL — verify end to end

**Files:** tests only (Task 1 already wired replay-copy and subflow-NULL); this task proves it across the whole path.

- [ ] **Step 1: Tests**: (a) replay of a webhook-fired execution still renders `{{ trigger.body }}` (column copied); (b) a subflow child's task reading `{{ trigger.x }}` fails to render (NULL); (c) a manually-started (API) execution's task reading `{{ trigger.x }}` fails to render (NULL) and no `trigger` key present.
- [ ] **Step 2: Run.** If (a) fails, the replay copy (Task 1 Step 4) is wrong — fix there. If (b)/(c) fail, a creation path is wrongly setting `trigger`.
- [ ] **Step 3: Commit** `test(workflow): trigger.* survives replay, absent on subflow/manual`.

---

### Task 7: Docs (repo egret-data/docs, new branch docs/workflow-trigger-binding)

- [ ] **Step 1:** In `platform/workflows/flows/expressions.md` (or the triggers doc) document `{{ trigger.* }}` availability per type (Schedule `date`/`next`/`previous`; Webhook `body`/`headers`/`parameters`; Flow `outputs`/`executionId`/…; `_context.id`/`_context.type`), and that it is absent for non-triggered executions. Mention replay keeps it.
- [ ] **Step 2:** Build the docs (or rely on the docs PR `build` gate).
- [ ] **Step 3: Commit** `docs(workflow): {{ trigger.* }} in task expressions`.

---

## Self-Review
- **Spec coverage:** persist (new column) → Task 1; capture 3 types → Tasks 3/4/5; bind → Task 2; replay-copy/subflow-NULL → Tasks 1+6; Leaf NULL → Task 1 (None default); no refusal lift → nothing lifts; Task-0 census → Task 0; tests → each task; docs → Task 7. No gap.
- **Placeholder scan:** render-side code is exact; capture-side names the exact structs/sites and defers only the precise field wiring to Task 0's measurement (legitimate — the measurement task), not "TBD".
- **Type consistency:** `NewExecution.trigger: Option<Value>` and `Surface.trigger: Value` used consistently; the per-type payloads are plain `serde_json::Value` objects with `_context`.
