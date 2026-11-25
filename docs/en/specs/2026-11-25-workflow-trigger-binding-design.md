# Package 11a — `trigger.*` binding in task renders — Design

**Status:** approved design, formalized on the #11a worktree.
**Date:** 2026-11-25
**Roadmap:** egret-workflow Kestra parity, package **#11 (triggers)**, part **11a**. #11 is a multi-package program (user-approved decomposition): **11a** = `trigger.*` binding (this doc); 11b = Schedule parity (`when`/`lateMaximumDelay`/`recoverMissedSchedules`/labels/stopAfter); 11c = Webhook parity (synchronous webhooks, webhook `when`, labels, stopAfter); 11d = Flow-trigger listener conditions + cross-cutting lifecycle. 11a is the keystone and is built first.

> **Task 0 correction (2026-11-25):** egret ALREADY persists the trigger record in the existing `workflow.executions.execution_trigger` column (migration 0140), shaped `{id,type,variables:{...}}`; Schedule + Flow already populate it and **replay copies it verbatim**. So §3.1's "new column" is unnecessary — 11a reuses `execution_trigger`. Only gaps: `build_context` doesn't read it, and the Webhook route sets `trigger: None`. The plan reflects this (no migration; bind + webhook-capture only).


## 1. Goal

When a flow is started by a trigger, bind the trigger's variables into **every task's render context** of the fired flow, so an expression like `{{ trigger.body }}`, `{{ trigger.date }}` or `{{ trigger.outputs.x }}` renders. Today it does not: `build_context` (worker/render.rs) binds `inputs/vars/flow/labels/execution/outputs/tasks/task/taskrun` and **no `trigger` root**, so `{{ trigger.* }}` in a task fails to render ("Unable to find `trigger`"). This is roadmap note 13.

**Binding only — 11a lifts NO refusal.** Schedule `when:`, webhook `wait`/`returnOutputs`/`responseContentType`/`responseCode`, partial `dependsOn`, `lateMaximumDelay`, etc. stay refused (11b/11c/11d). 11a surfaces the variables the three supported triggers already produce.

## 2. Measured background

- egret models three typed triggers (`TriggerKind::{Schedule, Webhook, Flow}`); everything else is an opaque `Leaf` (`models/trigger.rs`).
- Each typed trigger already **computes** its output at fire time — Webhook builds `trigger.variables` (an `Output` with `body`/`uri`/`headers`/`parameters`; `trigger/webhook.rs:275,312,647`); Flow builds its Output (`executionId`/.../`outputs`; `trigger/flow.rs`); Schedule has `schedule_date`. But this is used only to render the **trigger's own `inputs:`** — it is **not persisted on the created execution**, and the fired flow's tasks never see it.
- `Execution` (`models/execution.rs`) has **no `trigger` field**. The executions row already has a persisted `variables` jsonb column (→ the `vars` render key, render.rs:220-242) and a `trigger_execution_id` column — but neither carries the trigger's output variables.
- Kestra binds `trigger` from `Execution.getTrigger().getVariables()` into every task via `RunVariables` (`RunVariables.java:492-515`), with `trigger._context = {id, type}` and the per-type Output map. 11a mirrors this.

## 3. Design

### 3.1 Persist: new `executions.trigger` jsonb column (D1, user-approved)

A migration adds `trigger jsonb NULL` to `workflow.executions`. NULL when the execution was not trigger-started (manual/API start, subflow child). The existing `variables` column is already the `vars` key, so the trigger output gets its own column — clean separation, mirrors Kestra's `Execution.trigger`.

Stored value shape (one object): the per-type variables map **plus** `_context: {id, type}` (the trigger's id and FQCN). Exactly the Kestra per-type shape:
- **Schedule** → `{date, next, previous}` (`date` = the fire date = existing `schedule_date`; `next`/`previous` computed from the cron at fire time using egret's existing cron machinery).
- **Webhook** → `{body?, uri?, headers, parameters}` — the fields egret's webhook `Output` already carries. `body` absent for a GET / empty body (measured). `parts`/`formFields` are **omitted** (multipart is refused today; a later package may add them). `uri` only under `fetchType: STORE` (also refused today) — so in practice `{body?, headers, parameters}`.
- **Flow** → `{executionId, namespace, flowId, flowRevision, state, startDate, endDate, firstFailedTaskId, lastTaskId, executionLabels, outputs}`.

### 3.2 Capture at the three fire paths (D2)

Each fire path writes the computed Output (+ `_context`) into the new `trigger` column at the execution INSERT:
- **Webhook** — the webhook route handler (`trigger/webhook*`), which already builds the `Output`, stores it.
- **Schedule** — the scheduler fire path, storing `{date, next, previous}` + `_context`.
- **Flow trigger** — `flow_trigger::process`, storing the Flow Output + `_context`.

The three INSERT sites are `worker/{subflow,replay,spawn}.rs` (grepped) plus the trigger fire paths; 11a routes the column through the execution-creation seam each uses.

### 3.3 Replay / restart / subflow (D3)

- **Replay / restart** of a trigger-started execution **copies** the original execution's `trigger` column to the new/reset execution, so `{{ trigger.* }}` keeps rendering after a replay (Kestra parity). The capture is immutable per original execution.
- **Subflow child** and **manual/API start** → `trigger` is NULL (no trigger).

### 3.4 Bind in `build_context` (D4)

`build_context` (worker/render.rs ~805) reads the `trigger` column (already selectable alongside `variables` at render.rs:220). When non-null, it inserts a top-level `trigger` render key holding the stored object. When null (or a `Leaf` trigger that stored nothing), no `trigger` key is inserted — `{{ trigger.* }}` then fails to render exactly as today for a non-triggered execution (unchanged behaviour).

### 3.5 Leaf / opaque triggers

`ScheduleOnDates`, `http.Trigger`, plugin triggers are opaque `Leaf` — 11a models no output for them, so `trigger` stays NULL for executions they start. (Their trigger.* binding arrives with their own package, #18 / a later #11 part.)

### 3.6 No secret interaction

egret's trigger outputs are not secret-typed (unlike Kestra EE's secret trigger outputs), so there is no decrypt/mask step. 11a does not touch 10a's secret machinery. (A webhook body is author/caller data, not a `secret()` value.)

## 4. Error handling & edge cases

- **Null trigger:** no `trigger` key (today's behaviour) — no error, `{{ trigger.x }}` fails to render as before.
- **Schedule `next`/`previous`:** computed from the trigger's cron at fire time; if the cron somehow yields none (e.g. `ScheduleOnDates`, which is Leaf anyway), store only `date`.
- **Webhook GET / empty body:** no `body` key (measured Kestra parity).
- **Large webhook body:** stored in the jsonb column as-is (same size envelope as the existing body handling; `fetchType: STORE` which offloads to object storage stays refused in 11a).
- **Trigger output present but a task reads a missing sub-field** (`{{ trigger.outputs.nope }}`): normal render miss, unchanged semantics.

## 5. Testing

- **Task 0 (measure-first, like 10a/10b):** grep the corpus/oracle (`tests/executor.rs`, `oracle-exec/`) for any flow that references `{{ trigger.* }}` in a task and currently sits in a failed/refused bucket. Binding makes it render — if a census/oracle tuple moves, decide (keep census stable, or record the ratchet) before code. Also confirm no existing test asserts the *absence* of `trigger` binding.
- **Integration (one per trigger type):**
  - webhook-started flow whose task reads `{{ trigger.body }}` / `{{ trigger.headers.x }}` renders the posted value;
  - schedule-started flow whose task reads `{{ trigger.date }}` renders the fire date;
  - flow-trigger-started flow whose task reads `{{ trigger.outputs.x }}` / `{{ trigger.executionId }}` renders the source execution's values.
- **Replay:** a replayed trigger-started execution still renders `{{ trigger.* }}` (column copied).
- **Null:** a manually-started execution's task reading `{{ trigger.x }}` fails to render (unchanged), and no `trigger` key is present.
- **Unit:** `build_context` inserts `trigger` iff the column is non-null; `trigger._context` = {id,type}.
- **Migration** applies cleanly; existing rows get NULL.
- **Scoped `egret-workflow` runs MUST include `--test routes --test execution_timestamps`.**

## 6. Docs

- `platform/workflows/flows/expressions.md` (or the triggers doc) — document `{{ trigger.* }}` availability per trigger type (Schedule `date`/`next`/`previous`, Webhook `body`/`headers`/`parameters`, Flow `outputs`/`executionId`/…), and that it is null for non-triggered executions.

## 7. Global constraints

- Chat Vietnamese; code/commits/docs English. Conventional Commits; `Closes #N` in PR body only.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends with the Claude Code line.
- Never commit secrets/scratch. Stage by explicit path; never `git add -A`; never `target*/`.
- ONE heavy Rust build at a time, foreground in subagents. Merge on green CI, no `--admin`, `cargo fmt --all --check` first.
- Migration numbers are a shared namespace — renumber above main AND open PRs at plan time (find the next free number then).
- Scoped `egret-workflow` test runs include `--test routes --test execution_timestamps`.
