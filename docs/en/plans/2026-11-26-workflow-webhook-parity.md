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
- Produces: a `fn render_webhook_labels(record: &Output /* or the serialised trigger Value */, labels: &[Label]) -> Result<Vec<Value>, String>` whose output is prepended to the flow labels carried into the new execution.

- [ ] **Step 1: Write the failing test** — a webhook flow declaring `labels: [{env: prod}, {src: "{{ trigger.headers.x }}"}]` fires an execution carrying those labels (templated one rendered against the webhook record), **prepended** before the flow's own labels. Assert order: trigger labels, then flow labels.
- [ ] **Step 2: Run it — FAIL** (webhook labels ignored today). Run: `cargo test -p egret-workflow --test webhook_route <name> -- --nocapture`. Expected: FAIL.
- [ ] **Step 3: Implement** — in `handle()`, after `build_output` and before `start_execution_with_trigger`, render the trigger's `labels` against the webhook record (reuse the 11b `render_str` + `RenderContext` shape from `fire.rs:627`, binding the webhook record under `trigger`) and prepend to the labels the new execution receives. Flow labels unchanged. On render error, fail-safe exactly as 11b (record, do not fabricate), surfaced as the route's `InvalidInput`/`Conflict` per Task 0's 11b-matching convention.
- [ ] **Step 4: Run it — PASS** (+ scoped `--test routes --test execution_timestamps --test webhook_route`).
- [ ] **Step 5: Commit** `feat(workflow): Webhook trigger labels on the fired execution`.

---

### Task 2: `stopAfter` — disable the webhook trigger after a matching terminal

**Files:** `worker/stop_after.rs` `on_terminal` (`:40`, the `type != SCHEDULE_TYPE` gate); `webhook_routes.rs` `handle()` (after the flow-disabled check at `:603`). Tests: `tests/stop_after.rs`, `tests/webhook_route.rs`, `tests/disabled_triggers.rs`.

**Interfaces:**
- Consumes: `on_terminal`'s existing body (reads `execution_trigger`, parses `ctx.source`, finds the declared trigger, checks `stop_after`, writes both disable mechanisms). The webhook type string `io.kestra.plugin.core.trigger.Webhook` (`keys.rs:109` `WEBHOOK`).
- Produces: `on_terminal` now disables Schedule **or** Webhook triggers; the webhook route rejects a disabled webhook trigger with a 409.

- [ ] **Step 1a: Write the failing test (hook)** — an execution started by a **Webhook** trigger reaching a terminal ∈ `stopAfter` (e.g. `[FAILED]`) disables that trigger (`workflow.triggers.disabled = true` + a `workflow.disabled_triggers` row); a non-matching terminal leaves it enabled; a non-trigger execution disables nothing.
- [ ] **Step 2a: Run it — FAIL** (gate rejects non-Schedule). Run: `cargo test -p egret-workflow --test stop_after <name> -- --nocapture`.
- [ ] **Step 3a: Implement** — generalise the `:40` gate from `== Some(SCHEDULE_TYPE)` to accept Schedule or Webhook (add a `WEBHOOK_TYPE`/reuse `keys::WEBHOOK`); the rest of `on_terminal` is type-agnostic (nulling `next_evaluation_date` is a harmless no-op for a webhook). Update the module doc-comment (`:1`) to say Schedule **and** Webhook.
- [ ] **Step 4a: Run it — PASS.**
- [ ] **Step 1b: Write the failing test (route gate)** — after a webhook trigger is disabled (row in `workflow.triggers` with `disabled = true`), a `POST` to its key returns **409** (not a fired execution); an enabled trigger still fires.
- [ ] **Step 2b: Run it — FAIL** (`handle()` checks only flow-disabled today). Run: `cargo test -p egret-workflow --test webhook_route <name> -- --nocapture`.
- [ ] **Step 3b: Implement** — in `handle()`, after the flow-disabled check (`:603`), read `workflow.triggers.disabled` for this `(flow_id, trigger_id)` and return `Error::Conflict` when disabled. Keep the message distinct from the flow-disabled one.
- [ ] **Step 4b: Run it — PASS** (+ scoped `--test routes --test execution_timestamps --test webhook_route --test stop_after --test disabled_triggers`).
- [ ] **Step 5: Commit** `feat(workflow): Webhook stopAfter disables the trigger and the route rejects it`.

---

### Task 3: `when` — honour per the Task 0 arm

**Files:** `webhook_routes.rs` `handle()` (after the key match, before start); `spec/keys.rs` `lift_of` (`:185`); `spec/validate.rs` + `property-treatment.json` + README (census). Test: `tests/webhook_route.rs`.

**Interfaces:**
- Consumes: `models::Trigger.when: Option<Property<String>>` (`models/trigger.rs:49`, default `"true"`); the webhook record for the render context; Task 0's arm decision.
- Produces (arm a): a falsy rendered `when` suppresses execution creation and returns the Task-0 status/body; (arm b): `when` parsed and documented as an accepted no-op. Either arm drops the `not_honoured` warning.

- [ ] **Step 1: Write the failing test** — **arm (a):** a webhook flow with `when: "false"` (and a templated falsy case) answered per Task 0 **without** creating an execution; `when: "true"` fires normally; a `when` render error fails **safe** (no execution, named 4xx). **arm (b):** `when: "false"` still fires (no-op), asserted against Task 0's measured Kestra behaviour. Pick the arm Task 0 pinned.
- [ ] **Step 2: Run it — FAIL.** Run: `cargo test -p egret-workflow --test webhook_route <name> -- --nocapture`.
- [ ] **Step 3: Implement** — per Task 0's arm. Arm (a): after the key matches, render `when` against the webhook record `{body, headers, parameters, uri}` (confined engine, the 11a binding); if falsy, return the Task-0 response and do not start; render error → fail-safe. Arm (b): leave the runtime untouched, keep parsing. **Both:** change `keys.rs` `lift_of` `(WEBHOOK,"when")` (`:185`) off `Package(11)` (arm a: honoured → drop the lift so it no longer warns, matching how 11b dropped Schedule `when`; arm b: whatever Task 0's census says) and update `property-treatment.json` + README `### Result` count + the drift-recount note. LIFT-REFUSAL grep first.
- [ ] **Step 4: Run it — PASS** (+ scoped `--test routes --test execution_timestamps --test webhook_route --test flow_schema`).
- [ ] **Step 5: Commit** `feat(workflow): honour Webhook when:` (arm a) or `feat(workflow): accept Webhook when as an inert no-op (Kestra parity)` (arm b).

---

### Task 4: Synchronous response — `wait` / `returnOutputs` / `responseContentType` / `responseCode`

**Files:** `trigger/webhook.rs` `declared_unsupported` (`:348`, lift the four keys); `webhook_routes.rs` `handle()` (`:618` refusal removed; add the poll + response); `spec/keys.rs` `WEBHOOK_REFUSED` (`:435`, drop the four keys) + `lift_of` (`:186`); `property-treatment.json` + README (census). Test: `tests/webhook_route.rs`.

**Interfaces:**
- Consumes: `models::Webhook.{wait, return_outputs, response_content_type, response_code}` (`models/trigger.rs:164-170`); the execution id from `start_execution_with_trigger` (`claim.rs:4085`); `workflow.executions.state` + the outputs source Task 0 pinned; `RunState::is_terminal`.
- Produces: `wait: true` holds the response until terminal (or the Task-0 timeout) then answers with the execution's outputs/code/content-type; `wait: false` unchanged.

- [ ] **Step 1: Write the failing tests** — (a) a `wait: true` webhook whose flow terminates: the response body carries the terminal `outputs` when `returnOutputs: true`, the status is the rendered `responseCode` (or the Task-0 default), the `Content-Type` is `responseContentType` when set; (b) `wait: true` whose flow does not terminate within the timeout → the Task-0 timeout response; (c) `wait: false` (default) still returns `state: "CREATED"` immediately, fire-and-forget. (Drive the executor so the fired execution actually reaches terminal — mirror the `tests/webhook_route.rs` + executor harness the scheduler tests use.)
- [ ] **Step 2: Run it — FAIL** (409 refusal today). Run: `cargo test -p egret-workflow --test webhook_route <name> -- --nocapture`.
- [ ] **Step 3: Implement** —
  1. Remove `Wait`/`ReturnOutputs`/`ResponseContentType`/`ResponseCode` from `declared_unsupported` (`trigger/webhook.rs:348`) — keep `StoreBody`/`Multipart`. Drop them from `WEBHOOK_REFUSED` (`keys.rs:435`) and adjust `lift_of` (`:186`). LIFT-REFUSAL grep first.
  2. In `handle()`, after `start_execution_with_trigger`, branch on `webhook.wait`. `false` → today's `WebhookResponse { state: "CREATED" }`, unchanged. `true` → a bounded poll: loop reading `SELECT state[, outputs] FROM workflow.executions WHERE id = $1` every FIXED_INTERVAL until `RunState::is_terminal`, bounded by the Task-0 max-wait. On terminal: build the response with `responseCode` (render `Property<i32>` → status, else Task-0 default), body = terminal `outputs` when `return_outputs` (Task-0 source) else the normal body, `Content-Type` from `response_content_type`. On timeout: the Task-0 response; the execution keeps running.
  3. Interval + max-wait are documented `const`s. No `LISTEN`/`NOTIFY`.
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
