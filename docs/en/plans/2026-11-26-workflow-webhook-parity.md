# Webhook trigger parity (package 11c) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make egret's Webhook trigger match Kestra on `labels`, `stopAfter`, `when`, and the synchronous response (`wait`/`returnOutputs`/`responseContentType`/`responseCode`).

**Architecture:** All five features hook the single webhook fire seam `webhook_routes.rs::handle()` plus the 11b terminal seam `worker/stop_after.rs::on_terminal`. labels/stopAfter/when reuse the 11b patterns; the synchronous response adds a bounded poll of `workflow.executions.state` after the execution starts. Measurement-first: Task 0 pins the Kestra `when` arm, the sync timeout/defaults, the outputs source, and the census deltas before any feature code.

**Tech Stack:** Rust, axum, sqlx/Postgres. `crates/egret-workflow`.

## Global Constraints

- Chat Vietnamese; code/commits/docs English. Conventional Commits; `Closes #N` in the PR body only.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends with the Claude Code line.
- Never commit secrets/scratch. Stage by explicit path; never `git add -A` / `target*/`.
- ONE heavy Rust build at a time, FOREGROUND. Merge on green CI, no `--admin`, `cargo fmt --all --check` first.
- **Scoped `egret-workflow` runs MUST include `--test routes --test execution_timestamps --test webhook_route`** (webhook tests live in `tests/webhook_route.rs`; stopAfter also touches `--test disabled_triggers --test stop_after` — include those for Tasks 2/3/4).
- **LIFT-REFUSAL discipline:** before lifting the sync-key refusals (and the `when` warning), grep code + message + fixture YAML across `src/ tests/ oracle-exec/` (`declared_unsupported`, `Unsupported::`, `WEBHOOK_REFUSED`, `keys.rs:185` `(WEBHOOK,"when")`, the webhook flow corpus).
- Shared migration numbers — only if a column is needed (NOT expected). Next free number at plan time, renumber above main + open PRs.
- `secret()` / package 10a untouched.

## File Structure

- `crates/egret-workflow/src/webhook_routes.rs` — the fire seam `handle()`: trigger-disabled check (T2), webhook label render + carry (T1), `when` gate (T3), sync poll + response (T4).
- `crates/egret-workflow/src/worker/stop_after.rs` — `on_terminal`: generalise the `type` gate to Webhook (T2).
- `crates/egret-workflow/src/trigger/webhook.rs` — `declared_unsupported` (lift sync keys, T4); a webhook label renderer if one belongs here (T1).
- `crates/egret-workflow/src/spec/keys.rs` — `WEBHOOK_REFUSED` (drop sync keys, T4); `lift_of` `(WEBHOOK,"when")` (T3).
- `crates/egret-workflow/src/spec/validate.rs` / `spec/schema/property-treatment.json` + README — census moves (T3/T4).
- `crates/egret-workflow/src/worker/start_execution_with_trigger` (`claim.rs:4085`) — returns the execution id the poll uses (T4, read-only).
- Tests: `tests/webhook_route.rs` (primary), `tests/disabled_triggers.rs`, `tests/stop_after.rs`.

---

### Task 0: Measure — census + pin the unmeasured Kestra semantics (load-bearing)

**Files:** none (report only).

- [ ] **Step 1 — Census + LIFT-REFUSAL grep.** Record the Webhook corpus flows currently refused/warned for the sync keys + `when`. Grep `declared_unsupported`, `Unsupported::`, `WEBHOOK_REFUSED`, `keys.rs:185`, and the webhook flow fixtures across `src/ tests/ oracle-exec/`. Confirm `oracle-exec/baseline.txt` cannot move (the harness never drives the webhook route). Record the current `property-treatment.json` aggregate (1218/3/360/121 after 11b) and which webhook keys sit in which bucket.
- [ ] **Step 2 — Kestra `when` arm.** On the pinned Kestra artifact (`kestra/kestra@sha256:28f59bf1…`, the digest `trigger/webhook.rs` already names), determine whether the webhook controller evaluates the trigger `when` at request time. Drive a real request against a webhook flow with `when: "false"` and `when: "{{ … falsy … }}"`. Record: does a falsy `when` suppress the execution? If yes, what status/body does the request get? This decides **arm (a)** (gate execution creation) vs **arm (b)** (accept-but-inert no-op).
- [ ] **Step 3 — Sync `wait` semantics.** Drive `wait: true` requests. Pin: the wait timeout default + where it is configured; the response when the execution terminates (status source, body); the `responseCode` default and whether it derives from the terminal state; the `returnOutputs: true` body shape; the response when the timeout elapses before terminal. Pin the exact egret read for flow-level `outputs` of a terminal execution (column/source in `workflow.executions` or computed).
- [ ] **Step 4 — Confirm migration-free.** Verify labels/stopAfter/when/sync reuse existing columns/tables (`workflow.triggers.disabled`, `workflow.disabled_triggers`, `workflow.executions.state`, the webhook record). Record "migration-free" or the needed column + next free number.
- [ ] **Step 5 — Write findings** into the task report. No production code. Pin Tasks 1/2/3/4.

---

### Task 1: Webhook `labels` on the fired execution

**Files:** `webhook_routes.rs` `handle()` (before `start_execution_with_trigger`, `:686`); a label renderer in `trigger/webhook.rs` (mirrors `scheduler::fire::render_trigger_labels` at `fire.rs:627`, but binds the **webhook** record `{body, headers, parameters, uri}` under `trigger`/`webhook`, not a `ScheduledExecution`). Test: `tests/webhook_route.rs`.

**Interfaces:**
- Consumes: `models::Trigger.labels: Vec<Label>` (`models/trigger.rs:51`); the webhook `Output` record built at `handle()` (`build_output`).
- Produces: a `fn render_webhook_labels(record: &Output /* or the serialised trigger Value */, labels: &[Label]) -> Vec<Value>` whose output is stored as the execution's OWN labels (declaration order). **Note: no `Result` — a label that fails to render is OMITTED (Kestra parity, Task 0), not an error.**

**Label ordering (egret model — do NOT invent a prepend):** egret stores an execution's own labels in `workflow.executions.labels`; every reader renders them via `labels::visible_labels(flow, own, replaced)` (`labels.rs:97`) = **flow labels THEN own labels**. Kestra parity is flow-then-trigger, so the webhook trigger labels are the execution's OWN labels (like Schedule/backfill in 11b). Store them in declaration order; do NOT change `visible_labels` and do NOT prepend them before the flow labels.

**Task 0 facts for this task:**
- `start_with` hard-codes `labels: None` (`claim.rs:4155`), so the current `start_execution_with_trigger` path CANNOT carry trigger labels. Thread the rendered labels through: add a labels argument to `start_execution_with_trigger` (or a sibling wrapper) that sets `NewExecution.labels` to the rendered trigger labels (the execution's own labels). Pin the exact field path before writing.
- `render_trigger_labels` (`fire.rs:627`) is private and takes `&ScheduledExecution` — write a webhook variant/shared helper, do NOT call it directly.
- Kestra OMITS a webhook label whose template fails to render and STILL starts the execution — the opposite of the Schedule fail-safe. Do not abort on a bad label.

- [ ] **Step 1: Write the failing test** — a webhook flow declaring `labels: [{env: prod}, {src: "{{ trigger.headers.x }}"}]` fires an execution whose stored own-labels are those two (templated one rendered against the webhook record), in declaration order. Add a second case: a label with an unrenderable template is OMITTED and the execution still starts with the remaining labels.
- [ ] **Step 2: Run it — FAIL** (webhook labels ignored today). Run: `cargo test -p egret-workflow --test webhook_route <name> -- --nocapture`. Expected: FAIL.
- [ ] **Step 3: Implement** — in `handle()`, after `build_output` and before `start_execution_with_trigger`, render the trigger's `labels` against the webhook record (reuse the 11b `render_str` + `RenderContext` shape from `fire.rs:627`, binding the webhook record under `trigger`/`webhook`), **omitting any label that fails to render** (Kestra parity), and carry them into the new execution as its OWN labels via the threaded labels path (see Task 0 facts — `start_with`'s `labels: None` must become the rendered set). Flow labels unchanged; effective order stays flow-then-own via `visible_labels`.
- [ ] **Step 4: Run it — PASS** (+ scoped `--test routes --test execution_timestamps --test webhook_route`).
- [ ] **Step 5: Commit** `feat(workflow): Webhook trigger labels on the fired execution`.

---

### Task 2: `stopAfter` — disable the webhook trigger after a matching terminal

**Files:** `worker/stop_after.rs` `on_terminal` (`:40`, the `type != SCHEDULE_TYPE` gate) + module doc (`:1`). Tests: `tests/stop_after.rs`, `tests/webhook_route.rs`, `tests/disabled_triggers.rs`.

**Task 0 facts (SIMPLIFICATION — no route change):** The webhook route **already** rejects a disabled trigger with a 409: it checks the YAML `disabled` flag (`webhook_routes.rs:561`) AND `workflow.disabled_triggers` (`:574-588`). A webhook has **no `workflow.triggers` row**, so `on_terminal`'s `UPDATE workflow.triggers` (`stop_after.rs:74`) is a harmless 0-row no-op; what disables the webhook is `on_terminal`'s `INSERT INTO workflow.disabled_triggers` (`:64`), which is exactly the table the route reads. **So the ONLY change is opening the type gate.** Do NOT add a route check — it already exists and a `triggers.disabled` read would hit no row.

**Interfaces:**
- Consumes: `on_terminal`'s existing body (reads `execution_trigger`, parses `ctx.source`, finds the declared trigger, checks `stop_after`, INSERTs `disabled_triggers` + UPDATEs `triggers`). The webhook type string `io.kestra.plugin.core.trigger.Webhook` (`keys.rs:109` `WEBHOOK`). The webhook `execution_trigger.type` is that same string (set at `webhook_routes.rs` from `trigger.r#type`).
- Produces: `on_terminal` disables Schedule **or** Webhook triggers; no interface change.

- [ ] **Step 1: Write the failing test** — an execution started by a **Webhook** trigger reaching a terminal ∈ `stopAfter` (e.g. `[FAILED]`) writes a `workflow.disabled_triggers` row for that `(flow_id, trigger_id)`, and a subsequent `POST` to the webhook key returns **409**; a non-matching terminal writes no row and the key still fires; a non-trigger execution disables nothing.
- [ ] **Step 2: Run it — FAIL** (gate rejects non-Schedule). Run: `cargo test -p egret-workflow --test stop_after <name> -- --nocapture` (and `--test webhook_route` for the 409 half).
- [ ] **Step 3: Implement** — generalise the `:40` gate from `== Some(SCHEDULE_TYPE)` to accept Schedule **or** Webhook (add a `WEBHOOK_TYPE` const / reuse `keys::WEBHOOK`); the rest of `on_terminal` is type-agnostic (the `triggers` UPDATE is a harmless no-op for a webhook). Update the module doc-comment (`:1`) to say Schedule **and** Webhook.
- [ ] **Step 4: Run it — PASS** (+ scoped `--test routes --test execution_timestamps --test webhook_route --test stop_after --test disabled_triggers`).
- [ ] **Step 5: Commit** `feat(workflow): Webhook stopAfter disables the trigger via on_terminal`.

---

### Task 3: `when` — honour per the Task 0 arm

**Files:** `webhook_routes.rs` `handle()` (after the key match, before start); `spec/keys.rs` `lift_of` (`:185`); `spec/validate.rs` + `property-treatment.json` + README (census). Test: `tests/webhook_route.rs`.

**Task 0 decision — ARM (a), very high confidence.** Kestra's `WebhookService.newExecution` calls `conditionService.isValid(trigger, flow, runContext)`. A falsy `when` returns **HTTP 204 with an empty body and no execution**; a `when` that FAILS to render **also** returns 204 (not an error); `when` is checked **before** the `inputs` render.

**Interfaces:**
- Consumes: `models::Trigger.when: Option<Property<String>>` (`models/trigger.rs:49`, default `"true"`); the webhook record `{body, headers, parameters, uri}` for the render context (the 11a binding).
- Produces: a falsy (or unrenderable) `when` makes `handle()` return `StatusCode::NO_CONTENT` (204, empty body) and start no execution; a truthy `when` proceeds as today.

- [ ] **Step 1: Write the failing test** — a webhook flow with `when: "false"` returns **204** and creates **no** execution; a templated falsy `when` (e.g. `when: "{{ trigger.headers.x == 'no' }}"` with a matching header) returns 204; an unrenderable `when` (`when: "{{ nonexistent.deep.ref }}"`) ALSO returns 204 with no execution; `when: "true"` (and the default absent `when`) fires normally. Place the `when` check so a falsy `when` short-circuits **before** the `inputs` render (Kestra order).
- [ ] **Step 2: Run it — FAIL.** Run: `cargo test -p egret-workflow --test webhook_route <name> -- --nocapture`.
- [ ] **Step 3: Implement** — in `handle()`, after the key matches and the disabled checks, **before** `build_output`/`inputs`, render `when` against the webhook record (confined engine, the 11a binding). If the rendered value is falsy OR the render fails, return `Ok((StatusCode::NO_CONTENT, ()).into_response())` (204, empty body) and start no execution. A truthy `when` (including the `"true"` default) proceeds unchanged. Then update the census: `keys.rs` `lift_of` `(WEBHOOK,"when")` (`:185`) — follow the 11b discipline (a request-time-honoured key: regenerate `property-treatment.json` via the measure tool so the webhook `when` is reclassified, and keep/drop `lift_of` exactly as the regenerated golden + the validate-vs-runtime rule require — mirror how 11b handled Schedule `when`/`labels`). Update `property-treatment.json` + README `### Result` count + the drift-recount note from the measure tool's output, not by hand. LIFT-REFUSAL grep first (the `when` warning + corpus).
- [ ] **Step 4: Run it — PASS** (+ scoped `--test routes --test execution_timestamps --test webhook_route --test flow_schema`).
- [ ] **Step 5: Commit** `feat(workflow): honour Webhook when: (204 on a falsy condition)`.

---

### Task 4: Synchronous response — `wait` / `returnOutputs` / `responseContentType` / `responseCode`

**Files:** `trigger/webhook.rs` `declared_unsupported` (`:348`, lift the four keys); `webhook_routes.rs` `handle()` (`:618` refusal removed; add the poll + response); `spec/keys.rs` `WEBHOOK_REFUSED` (`:435`, drop the four keys) + `lift_of` (`:186`); `property-treatment.json` + README (census). Test: `tests/webhook_route.rs`.

**Task 0 facts (pinned from Kestra rc13 source):**
- **Wait has NO Kestra timeout** (`followExecution().last()`). egret must pick its OWN documented cap (a `const`) and document the divergence — Kestra waits forever, egret bounds it.
- **"Terminal" = `workflow.execution_is_idle(id)`** (SQL function, migration 0210), not a hand-rolled state check.
- **responseCode:** default = **500 if the terminal state is FAILED, else 200** (only FAILED → 500). A declared `responseCode` is rendered against the TERMINAL execution context (so `{{ outputs.<task> }}` resolves — task outputs come from `workflow.task_runs.outputs`). It applies **even without `returnOutputs`**. An invalid rendered code → **500 with an empty body**.
- **returnOutputs body:** the **bare flow-level outputs map** — `workflow.executions.outputs jsonb` (migration 0214; NULL = none; SECRET values stored as `******`). JSON by default; `text/plain` **only** when `responseContentType == "text/plain"` exactly. Without `returnOutputs`, the body is the `WebhookResponse` of the terminal run.
- **egret read:** `SELECT state, outputs, workflow.execution_is_idle(id) FROM workflow.executions WHERE id = $1 AND workspace_id = $2`.

**Interfaces:**
- Consumes: `models::Webhook.{wait, return_outputs, response_content_type, response_code}` (`models/trigger.rs:164-170`); the execution id from `start_execution_with_trigger` (`claim.rs:4085`); the read above; `workflow.task_runs.outputs` for a declared `responseCode` template.
- Produces: `wait: true` holds the response until `execution_is_idle` (or the egret cap) then answers with the execution's outputs/code/content-type; `wait: false` unchanged.

- [ ] **Step 1: Write the failing tests** — (a) a `wait: true` + `returnOutputs: true` webhook whose flow terminates SUCCESS: body = the flow's bare `outputs` map, status 200; (b) a `wait: true` flow that terminates FAILED with no declared `responseCode`: status **500**; (c) a declared `responseCode: "{{ outputs.x.v }}"` rendered against the terminal context → that status, even without `returnOutputs`; (d) `responseContentType: "text/plain"` → `Content-Type: text/plain`; (e) `wait: false` (default) still returns `state: "CREATED"` immediately (fire-and-forget). Drive the executor so the fired execution actually reaches terminal — mirror the executor harness the scheduler/webhook tests use.
- [ ] **Step 2: Run it — FAIL** (409 refusal today). Run: `cargo test -p egret-workflow --test webhook_route <name> -- --nocapture`.
- [ ] **Step 3: Implement** —
  1. Remove `Wait`/`ReturnOutputs`/`ResponseContentType`/`ResponseCode` from `declared_unsupported` (`trigger/webhook.rs:348`) — KEEP `StoreBody`/`Multipart`. Drop them from `WEBHOOK_REFUSED` (`keys.rs:435`) and adjust `lift_of` (`:186`) + regenerate `property-treatment.json` via the measure tool + README count. LIFT-REFUSAL grep first.
  2. In `handle()`, after `start_execution_with_trigger`, branch on `webhook.wait`. `false` → today's `WebhookResponse { state: "CREATED" }`, unchanged. `true` → a bounded poll: loop the read above every FIXED_INTERVAL until `execution_is_idle`, bounded by the egret cap `const`. On terminal: status = rendered `responseCode` against the terminal context if declared (else 500-if-FAILED-else-200; invalid render → 500 empty body); body = the bare `outputs` map when `return_outputs` (NULL → empty), else the terminal `WebhookResponse`; `Content-Type` = `response_content_type` (`text/plain` toggles the outputs serialisation per Task 0). On hitting the egret cap before terminal: document and return a defined response (the execution keeps running) — pin the exact status in the docs (Task 5).
  3. Interval + cap are documented `const`s. No `LISTEN`/`NOTIFY`.
- [ ] **Step 4: Run it — PASS** (+ scoped `--test routes --test execution_timestamps --test webhook_route --test flow_schema`).
- [ ] **Step 5: Commit** `feat(workflow): synchronous Webhook response (wait/returnOutputs/responseContentType/responseCode)`.

---

### Task 5: Docs (repo egret-data/docs, branch docs/workflow-webhook-parity)

**Files:** `platform/workflows/triggers/webhook.md` (+ any editor reference listing refused keys).

- [ ] **Step 1:** Document the five features: webhook labels; stopAfter (disables the trigger; the route then rejects it); `when` (per the Task 0 arm); synchronous response (`wait`/`returnOutputs`/`responseContentType`/`responseCode`), including the **divergences** from §5 of the spec (poll vs reactive stream latency; stopAfter enforced at the route; any `when` arm-(b) no-op).
- [ ] **Step 2:** Build docs (or rely on the PR `build` gate).
- [ ] **Step 3: Commit** `docs(workflow): Webhook trigger parity (labels/stopAfter/when/sync response)`.

---

## Self-Review

- **Spec coverage:** §1.1 labels→T1; §1.2 stopAfter→T2; §1.3 when→T3; §1.4 sync→T4; §6 census→T0+T3+T4; §5 divergences→T5 docs; §7 testing folded into each task. No gap.
- **Placeholders:** the deferred values (Kestra `when` arm, wait timeout/defaults, outputs source, census deltas) are Task 0 **measurement deliverables**, not impl placeholders — every impl step names its file:line and its concrete edit, consuming Task 0's pinned value. `render_trigger_labels` is Schedule-shaped (`ScheduledExecution`); T1 writes a webhook-record variant rather than calling it directly (noted).
- **Type consistency:** `render_webhook_labels` (T1), the `WEBHOOK_TYPE` gate (T2), the `when` arm (T3), and the `wait` poll (T4) use the field names from `models/trigger.rs:49/51/60/164-170` and the existing `on_terminal`/`declared_unsupported`/`WEBHOOK_REFUSED` names verbatim.
- **Order:** measure → easy (labels) → stopAfter → when → sync (heaviest) → docs. Each task is independently testable and reviewable.
