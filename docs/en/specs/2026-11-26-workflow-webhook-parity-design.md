# Package 11c — Webhook trigger parity — Design

**Status:** design approved (scope + decisions fixed), formalised on the #11c worktree.
**Date:** 2026-11-26
**Roadmap:** egret-workflow Kestra parity, package **#11 (triggers)**, part **11c**. Program #11: 11a (bind `trigger.*`, MERGED #915) → 11b (Schedule parity, MERGED #916) → **11c** (Webhook parity, this doc) → 11d (Flow-trigger listener conditions + remaining lifecycle). User chose **all Webhook features together in 11c** (sync response included, not split).

## 1. Goal

Make egret's Webhook trigger match Kestra on the five features it currently refuses or drops:

1. **`labels`** — the trigger's own labels on the fired execution (parsed, ignored; `not_honoured` warning today).
2. **`stopAfter`** — disable the webhook trigger after an execution reaches a matching terminal state (parsed, ignored).
3. **`when`** — a truthiness condition evaluated at request time (parsed, `not_honoured` warning via `lift_of` Package 11, ignored at runtime today — **not** hard-refused; **exact Kestra behaviour is a Task 0 measurement**, see §4 D1).
4. **Synchronous response** — `wait` / `returnOutputs` / `responseContentType` / `responseCode`: hold the HTTP response open until the fired execution terminates, then answer with its outputs / content-type / status (all four refused with a 409 today via `trigger::webhook::declared_unsupported`).

Out of scope (stay refused — they need an object store this crate does not depend on, a separate package): `fetchType: STORE` (`Unsupported::StoreBody`) and `multipart/form-data` (`Unsupported::Multipart`).

Built on **11a** (`trigger.*` binding — the webhook record `{body, headers, parameters, uri}` is already bound) and the **11b** patterns (`render_trigger_labels`, the `worker/stop_after.rs on_terminal` hook, the two disable mechanisms).

## 2. Measured facts (egret webhook fire path)

The single webhook fire seam is `webhook_routes.rs::handle()` (the `POST/GET/PUT /webhook/{workspace}/{id}/{key}` handler). Measured flow today:

1. Rate-limit / bulkhead admission (`WebhookGuard`).
2. Resolve workspace slug → id; find the flow + its webhook trigger; render + compare the `key:` (`trigger::webhook::keys_match` / `render_key`, constant-time).
3. Reject if the **flow** is disabled (`parsed.disabled || state == "disabled"`, `webhook_routes.rs:603`). **The trigger's own `disabled` column is NOT checked.**
4. Refuse the parsed-but-unhonoured sync keys by name, 409: `if let Some(unsupported) = declared_unsupported(webhook)` (`webhook_routes.rs:618`).
5. Build the webhook `Output` from the request (`build_output` → `{body, headers, parameters, uri}`); refuse `Multipart`/`StoreBody`.
6. Render `inputs:` (secret cycle; `trigger` is deliberately unbound for the key's private object).
7. Start the execution fire-and-forget: `worker::start_execution_with_trigger(pool, ws_id, flow_uuid, source, inputs, trigger_json)` (`claim.rs:4085`), returns the new execution id.
8. Respond `WebhookResponse { workspace_id, id, namespace, flow_id, trigger, state: "CREATED" }`.

**`when`, `labels`, and `stopAfter` are never read on this path.** The response always returns immediately with `state: "CREATED"`.

- `models::Trigger` carries the generic `when: Option<Property<String>>` (`models/trigger.rs:49`, `default_when()` = `"true"`), `labels: Vec<Label>` (`:51`), `stop_after: Vec<State>` (`:60`) — the same fields 11b honoured for Schedule.
- `models::Webhook` carries `wait: bool` (`:164`), `return_outputs: bool` (`:166`), `response_content_type: Option<String>` (`:168`), `response_code: Option<Property<i32>>` (`:170`).
- `trigger::webhook::declared_unsupported(&Webhook)` returns the first of `Wait` / `ReturnOutputs` / `ResponseContentType` / `ResponseCode` that is declared (`trigger/webhook.rs:348`). `spec::schema` calls it too, so these keys are not offered by the flow editor.
- `workflow.executions.state` (migration, `text NOT NULL`) holds the last state in Kestra's wire spelling. There is **no HTTP-blocking await** anywhere; `worker::subflow` joins children through the internal `settle` recursion, not a request-blocking wait. The sync handler must poll.
- The two disable mechanisms (11b): `workflow.triggers.disabled` column (what a claim/route would read) **and** the `workflow.disabled_triggers` table (source `sync` re-derives the column from it). A disable must write **both** to survive a sync. `worker/stop_after.rs on_terminal` already does this, but is **gated to `type = Schedule`** today.

## 3. Kestra semantics (parity target)

- **labels** — the webhook trigger's `labels:` (rendered against the trigger record) are added to the fired execution, alongside the flow's own labels. Append order per 11b: trigger labels, then the rest.
- **stopAfter** — when an execution started by the trigger reaches an end state in `stopAfter`, the trigger is disabled (no further firing). For a webhook that means subsequent `POST`s to the key are rejected.
- **when** — **Task 0 measures on the pinned artifact** whether Kestra's webhook controller evaluates the trigger `when` at request time, and what it answers when the condition is falsy. `webhook.rs`, which was measured exhaustively on the artifact for the key/inputs path, does **not** mention `when` — a genuine signal that the webhook controller may not evaluate it. Two faithful arms (§4 D1).
- **synchronous response** — with `wait: true`, Kestra holds the HTTP response (a reactive `Mono` subscribed to the execution queue) until the execution terminates or a timeout elapses, then responds. `returnOutputs: true` puts the terminal execution's `outputs` in the body; `responseCode` sets the HTTP status; `responseContentType` sets the `Content-Type`. The default `wait: false` returns immediately (today's behaviour). **Task 0 measures** the wait timeout default, the timeout response, the `responseCode` default and whether it is derived from the terminal state, and the exact outputs source.

## 4. Design & decisions

- **D4 — labels (small):** in `handle()`, before `start_execution_with_trigger`, render the trigger's `labels` against the webhook record (`{body, headers, parameters, uri}`, the 11a binding) via `render_trigger_labels` (reused from 11b `fire.rs`) and prepend them to the flow labels carried into the new execution. Flow labels unchanged. Fail-safe on render error as 11b (record, do not fabricate a label).
- **D5 — stopAfter (small-medium):** two edits. (a) Open `worker/stop_after.rs on_terminal` to accept **`type = Webhook`** as well as Schedule (it already writes both disable mechanisms + nulls `next_evaluation_date`, which is a no-op for a webhook that has none). (b) In `handle()`, after the flow-disabled check, also reject when the **trigger** is disabled — read `workflow.triggers.disabled` for this `(flow_id, trigger_id)` and return a 409. This route-side check is the webhook analogue of the scheduler's `AND NOT t.disabled` claim filter (a webhook never goes through the scheduler claim, so the gate must live in the route). Divergence note: disabling is keyed by trigger id and enforced at the route, not the scheduler.
- **D1 — when (medium): Task 0 decides the arm.**
  - *Arm (a) — Kestra evaluates `when` at request time:* after the key matches, render `when` against the webhook record; if falsy, do **not** create an execution and answer with whatever Kestra answers (Task 0 pins the status/body). Lift the refusal. Render error → fail-safe (no execution, named 4xx), as `runIf`.
  - *Arm (b) — Kestra does not evaluate webhook `when`:* honour = accept-but-inert. Lift the refusal, keep the key parsing, document it as accepted no-op, and drop it from the refused/warned set so the census reflects reality.
  - Webhook `when` is **not** hard-refused today (it is not in `WEBHOOK_REFUSED` / `declared_unsupported`): it is a `not_honoured` warning via `keys.rs` `lift_of` `(WEBHOOK,"when") => Package(11)` (`keys.rs:185`) and ignored at runtime. Arm (a) = drop the warning + honour; arm (b) = drop the warning + document an accepted no-op. Task 0 pins the exact `lift_of`/census treatment to change. (The sync keys, by contrast, ARE hard-refused and need the LIFT-REFUSAL grep — see D-SYNC.)
- **D-SYNC — synchronous response (large, cross-seam):**
  - Lift the four sync keys from `declared_unsupported` (`trigger/webhook.rs:348`) and the route refusal (`webhook_routes.rs:618`), and from `WEBHOOK_REFUSED` / the schema measurement.
  - `wait: false` (default): unchanged fire-and-forget.
  - `wait: true`: after `start_execution_with_trigger`, **poll** `workflow.executions.state` for this execution id on a bounded loop (fixed interval + a max-wait timeout) until the state is terminal (`RunState::is_terminal`). On terminal:
    - `responseCode` (if declared) → rendered `Property<i32>` → HTTP status; otherwise the Task 0 default (likely 200, or a state-derived code — Task 0 pins).
    - `returnOutputs: true` → body = the execution's flow-level `outputs` (Task 0 pins the exact read / source column); otherwise the current `WebhookResponse` body.
    - `responseContentType` (if declared) → `Content-Type` header.
    - On timeout before terminal → Task 0 pins Kestra's answer (the execution as-is, or a 4xx). The execution keeps running regardless.
  - **No `LISTEN`/`NOTIFY`** (egret has none wired): a bounded poll. Interval and max-wait are implementation constants, documented, and chosen so a slow execution does not hold a connection indefinitely. This is a latency divergence from Kestra's reactive stream, not a semantic one (§5).

## 5. Divergences (to document in docs PR)

- **Poll vs reactive stream:** egret polls the execution state on an interval rather than subscribing to a queue, so a waited response may lag the terminal by up to one poll interval. Semantics (which outputs/code/content-type) match.
- **Webhook stopAfter enforced at the route:** the disable is checked in `webhook_routes.rs`, not the scheduler, because a webhook never goes through a scheduler claim.
- Any `when` arm-(b) outcome (accepted no-op) if Task 0 finds Kestra does not evaluate webhook `when`.

## 6. Census / Banks (Task 0, measure first)

Lifting the sync keys + (arm a) `when` translates Webhook corpus flows from refused/warned toward valid. Task 0 re-measures `property-treatment.json` (probe-generated + golden-tested, as 11b) and records the exact honoured/warned/ignored/refused deltas and the new aggregate (today's census after 11b: 1218 honoured / 3 warned / 360 ignored / 121 refused / 1702 total). `labels`/`stopAfter` are runtime/scheduler behaviour and should not move the validate/publish census (confirm). The oracle executor never drives the webhook route (`oracle-exec/harvest.py` never calls `POST …/executions/webhook/…`), so `oracle-exec/baseline.txt` does not move — every claim is judged by this crate's own tests.

## 7. Testing

Task 0 pins: the `when` arm (a/b) + falsy response, the `wait` timeout default + timeout response, the `responseCode` default + source, the `returnOutputs` body/outputs source, the census deltas, the exact `when` treatment site (`keys.rs` `lift_of` + census). Per-feature tests: labels order; stopAfter disables + route rejects a disabled webhook trigger + a non-matching terminal leaves it enabled; `when` gate per the Task 0 arm; `wait` terminal → outputs/code/content-type; `wait` timeout; `wait: false` fire-and-forget unchanged. Scoped `egret-workflow` runs **must** include `--test routes --test execution_timestamps`. Migration-free expected (reuses existing columns/tables); Task 0 confirms.

## 8. Global constraints

- Chat Vietnamese; code/commits/docs English. Conventional Commits; `Closes #N` only in the PR body.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`; PR body ends with the Claude Code line.
- Never commit secrets/scratch. Stage by explicit path; never `git add -A` / `target*/`.
- ONE heavy Rust build at a time, foreground. Merge on green CI, no `--admin`, `cargo fmt --all --check` first.
- **LIFT-REFUSAL discipline:** before lifting the `when` / sync refusals, grep code + message + fixture YAML across `src/ tests/ oracle-exec/` (`declared_unsupported`, `Unsupported::`, `WEBHOOK_REFUSED`, the `when` refusal site, the webhook flow corpus).
- Shared migration numbers — only if a column is needed (not expected); take the next free number at plan time and renumber above main + open PRs.
- Scoped `egret-workflow` runs include `--test routes --test execution_timestamps`.
