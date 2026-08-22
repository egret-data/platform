# Data API Endpoint Alerts (package Z2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an alert watch a Data API endpoint's error rate, latency, traffic or error codes, using the scheduler, grace period, recipients, run log and editor that Alerts & Reports already has.

**Architecture:** A new `automation.schedules.type` value, `dataapi_alert`. The scheduler needs no change — its claim query does not read `type`. The comparison needs no change — `validator::should_fire(kind, cfg, Option<f64>)` is already a pure function over an optional scalar. Exactly two things are new: an evaluator that reads `dataapi.access_log` and produces that `Option<f64>`, and a source section in the editor.

**Tech Stack:** Rust (axum, sqlx, Postgres), React 18 + TypeScript, vitest.

**Spec:** `docs/en/specs/2026-08-22-dataapi-alerts-design.md` §5 (English canonical; Vietnamese mirror alongside).

**Branch:** `feat/dataapi-endpoint-alerts`, stacked on `feat/alerts-editor-config-table` (PR #535), which is stacked on `feat/explore-list-primitives-adoption` (PR #534). Do not rebase either parent.

## What was verified before this plan was written

Every architectural claim below was read out of the code, not assumed. Two of the spec's assertions did **not** survive that reading and are corrected here rather than left for an implementer to trip over.

**Confirmed:**

- `validator::should_fire(kind, &config, Option<f64>) -> Result<bool, _>` is **public** (`crates/egret-automation/src/validator.rs:23`) and takes exactly the optional scalar a metric evaluator produces. The spec's claim that "only the source of the scalar is new" holds.
- `Outcome` (`runner.rs:932`) has three variants, each carrying `value: Option<f64>` — the shape a metric run already fits.
- `in_grace` is called at `runner.rs:233`, inside the alert path after evaluation, so a correctly-routed `dataapi_alert` inherits grace handling with no new code.
- `egret-automation`'s route state **already holds a PDP** (`routes.rs:61`) and uses `egret_core::authz::require`, so the save-time authorization check needs no new plumbing.
- `egret-dataapi-api` depends on **only** `egret-core`, so `egret-automation` can depend on it without a cycle. `egret-automation` already consumes `egret-analytics-api` the same way.
- `activity.rs` fixes the metric definitions this plan must copy: errors are `status_code >= 400`; p50/p95 filter to `status_code BETWEEN 200 AND 299`; trials are excluded by `actor_user_id IS NOT NULL`.

**Corrected — the spec asserted these without checking:**

1. **The runner's dispatch is not a match.** `evaluate_and_deliver` (`runner.rs:185-190`) reads `if s.r#type == "report" { return self.render_and_deliver(s).await; }` and then **falls through to the alert path**. A third type routes itself into the alert branch by default. There is a fail-safe — the `let (Some(database_id), Some(sql), Some(validator)) = … else { return Err(EvalError::NotRunnable(…)) }` at `:194-200` — so the failure is a loud run-log error rather than a wrong run, but it is still wrong. Task 3 exists because of this.
2. **The runner has no PDP.** `pdp` appears in `routes.rs` and nowhere in `runner.rs`. The spec's run-time authorization re-check is therefore not "already possible"; it needs a PDP threaded into the runner. Task 4 exists because of this.

## Global Constraints

- **Copy the metric definitions from `activity.rs`, do not re-derive them.** An alert that reports a different number from the statistics panel is a support ticket. The three that matter are named above; read `crates/egret-dataapi/src/activity.rs` and take them verbatim.
- **`actor_user_id IS NULL` is not optional.** It excludes Try-it console trials. Without it, a developer clicking Try it twenty times against a broken draft fires a production alert. It is the filter most likely to be dropped by someone simplifying the query later.
- **The empty-window handling is asymmetric on purpose** and every metric's branch must match §5.6: `error_rate`, `p50_duration_ms` and `p95_duration_ms` return `None` on an empty window; `call_count`, `error_count` and `error_code_count` return `Some(0.0)`. Carry the reason in a comment, or the next refactor will "fix" the inconsistency.
- **Any help or description string that asserts what the system does must be read out of the implementation first**, and the plan must cite where. Six such strings shipped wrong in the preceding package and one of them was a regression over correct text.
- Backend gate before every commit: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and the tests for the crates touched. Frontend gate: `npx tsc --noEmit`, `npx eslint .` at 0 errors / 177 warnings, `npx vitest run`.
- `cargo test --workspace` **stops at the first failing binary**, so a green tail does not mean a green suite. Run the touched crates explicitly.
- Migrations are embedded; touch `metastore/src/lib.rs` if a new migration is not picked up.
- Do not modify `crates/egret-dataapi/src/activity.rs`. Read it.

## File Structure

| File | Responsibility |
|---|---|
| `egret/migrations/0127_dataapi_alerts.sql` | **New.** Four columns, four CHECKs, one index. |
| `crates/egret-dataapi-api/src/lib.rs` | **Modify.** Export the endpoint object-kind and read-permission strings so `egret-automation` can ask its own PDP without depending on `egret-dataapi`. |
| `crates/egret-automation/src/dataapi_metric.rs` | **New.** The evaluator: `(ws, endpoint_id?, metric, error_code?, window) -> Option<f64>`. |
| `crates/egret-automation/src/repo.rs` | **Modify.** Four fields on `Schedule`, and the column list that reads them. |
| `crates/egret-automation/src/model.rs` | **Modify.** The new `ScheduleType`, the request shape, its validation. |
| `crates/egret-automation/src/runner.rs` | **Modify.** Three-way dispatch; the `dataapi_alert` arm; the PDP for the run-time re-check. |
| `crates/egret-automation/src/routes.rs` | **Modify.** Save-time authorization on the selected endpoint. |
| `frontend/src/components/alerts/ScheduleEditor.tsx` | **Modify.** The source section. |

**Task order:** the data model first, because everything else reads it; then the evaluator, which is testable in isolation; then the dispatch that joins them; then authorization; then the API surface and the editor.

---

### Task 1: The migration and the row

**Files:**
- Create: `egret/migrations/0127_dataapi_alerts.sql`
- Modify: `crates/egret-automation/src/repo.rs`

**Interfaces:**
- Produces: four new `Schedule` fields — `dataapi_endpoint_id: Option<Uuid>`, `dataapi_metric: Option<String>`, `dataapi_error_code: Option<String>`, `dataapi_window_s: Option<i32>`. Tasks 2-6 read these names.

- [ ] **Step 1: Write the migration**

The four constraints are in spec §5.4. Copy them from there — they are stated in full and each has its reason. Two things the spec's text calls out and this step must not lose:

- `schedule_type_valid` is the **only existing CHECK that changes**. `schedule_alert_needs_query` and `schedule_report_needs_target` are both written `type <> 'x' OR (…)`, so a new type excludes itself from both. Confirm that by reading `0110_automation_schedules.sql` before you write anything.
- The load-bearing half of the new shape CHECK is `database_id IS NULL AND sql IS NULL`. It makes "a row both evaluators claim as theirs" impossible to write, which is what Task 3's dispatch bug would otherwise depend on being caught at runtime.

Add the index the spec names: `CREATE INDEX access_log_ws_ep_ts_idx ON dataapi.access_log (workspace_id, endpoint_id, ts DESC);`

- [ ] **Step 2: Prove the constraints refuse what they should**

Write a test that INSERTs, against a real database, each of these and asserts the DB rejects it — **not** that Rust rejects it. A Rust-side assertion passes with the CHECK deleted.

```
a `dataapi_alert` row carrying `sql`            → rejected
a `dataapi_alert` row carrying `database_id`    → rejected
a `dataapi_alert` row with a NULL metric        → rejected
a `dataapi_alert` row with `dataapi_metric = 'no_such_metric'` → rejected
a `dataapi_alert` row with `dataapi_error_code` set but metric ≠ 'error_code_count' → rejected
a `dataapi_alert` row with metric = 'error_code_count' and a NULL error_code → rejected
an `alert` row carrying `dataapi_metric`        → rejected
a well-formed `dataapi_alert` row               → accepted
```

Find how the crate's existing DB-backed tests are gated before writing this: a test that silently skips when `DATABASE_URL` is absent reports `ok` while asserting nothing, and only its duration distinguishes the two. Say in your report which gate you used and how you confirmed the test actually ran.

- [ ] **Step 3: Add the four fields to `Schedule` and to the column list**

`repo.rs`'s `Schedule` struct is at `:37`. The column list that populates it is in the same file — find it and extend it; a field added to the struct without the column is a runtime decode error, not a compile error.

- [ ] **Step 4: Backend gate and commit**

```bash
cargo fmt --all -- --check
cargo clippy -p egret-automation --all-targets -- -D warnings
cargo test -p egret-automation
```

Commit the migration and `repo.rs` together — the struct cannot compile against the old schema.

---

### Task 2: The metric evaluator

**Files:**
- Create: `crates/egret-automation/src/dataapi_metric.rs`
- Modify: `crates/egret-automation/src/lib.rs` (declare the module)

**Interfaces:**
- Consumes: nothing from Task 1 except the field names.
- Produces: `pub async fn measure(db: &Db, ws: Uuid, endpoint_id: Option<Uuid>, metric: &str, error_code: Option<&str>, window_s: i32, now: DateTime<Utc>) -> Result<Option<f64>, sqlx::Error>`. Task 3 calls this. **`now` is an argument, not `now()` inside the query** — a time-dependent assertion needs an injectable clock, and two readings that can coincide is a flake this repo has already recorded.

- [ ] **Step 1: Read `activity.rs` and write down the three definitions**

Before writing SQL, read `crates/egret-dataapi/src/activity.rs`'s `summary` query and record, in your report, the exact expressions for: what counts as an error, what p50/p95 filter to, and how trials are excluded. You are copying these.

- [ ] **Step 2: Write the failing tests**

Six metrics × the empty window is the matrix that matters. Seed `dataapi.access_log` rows directly and assert:

- `error_rate` over a window with 3 rows of which 1 is `status_code = 500` → `Some(0.333…)` (assert with a tolerance, not equality).
- `error_rate` over an **empty** window → `None`.
- `call_count` over an **empty** window → `Some(0.0)`. **This asymmetry is the point of the metric** — write the two empty-window tests as separate cases so one passing says nothing about the other.
- `p95_duration_ms` ignores a burst of `401`s: seed several 401 rows with a tiny `duration_ms` and one slow 200, and assert the result reflects the 200. This is the test that dies if someone drops the `BETWEEN 200 AND 299` filter.
- **Trials are excluded**: seed a row with `actor_user_id` set and a row without, and assert only the second is counted. This is the test that dies if someone drops that filter.
- `error_code_count` counts only the named code.
- `endpoint_id = None` aggregates the whole workspace; `Some(id)` narrows to one, and a second endpoint's rows do not leak in.
- A row from **another workspace** never counts.

- [ ] **Step 3: Implement**

One parameterised query per metric, all scoped `WHERE workspace_id = $1 AND ($2::uuid IS NULL OR endpoint_id = $2) AND ts > $3 AND actor_user_id IS NULL`.

- [ ] **Step 4: Verify the two filters are load-bearing**

Delete the `actor_user_id IS NULL` clause, run `cargo build -p egret-automation`, **confirm it compiles**, then run the tests and confirm the trial-exclusion test fails. Restore it. Do the same for `BETWEEN 200 AND 299` against the p95 test. A mutation that does not compile proves nothing, which is why the build check comes first.

- [ ] **Step 5: Backend gate and commit**

---

### Task 3: The dispatch

**Files:**
- Modify: `crates/egret-automation/src/runner.rs`

**Interfaces:**
- Consumes: `dataapi_metric::measure` from Task 2, the `Schedule` fields from Task 1, and `validator::should_fire`.

**This task exists because of a fact the spec got wrong.** `evaluate_and_deliver` at `:185-190` is `if s.r#type == "report" { … }` followed by a fall-through to the alert path. A `dataapi_alert` therefore lands in the alert branch, hits the `NotRunnable("alert is missing its query")` guard at `:194-200`, and errors. Loud, but wrong.

- [ ] **Step 1: Write the failing test**

Assert that a `dataapi_alert` schedule runs its metric and fires — and, before you change the dispatch, confirm the test fails with `NotRunnable`, not with something else. A test that fails for the wrong reason proves nothing about the dispatch.

- [ ] **Step 2: Make the dispatch explicit**

Turn the `if report` into a three-way branch on `s.r#type`. Prefer a form where an unrecognised type is a distinct error rather than silently taking the alert path — the current fall-through is exactly the defect this task fixes, and reintroducing it for a fourth type later is the obvious repeat.

- [ ] **Step 3: Wire the arm**

`measure(...)` → `Option<f64>` → `should_fire(ValidatorType::Operator, &s.validator_config, value)` → the existing `Outcome`. Then:

- **A `None` value must produce a `noop` run, not a fired one.** Read how `should_fire` treats `None` before deciding whether you need a guard ahead of it, and say what you found in your report.
- **A trashed or `disabled` endpoint yields `noop` with the reason in the run log, not `error`** (§5.6). Moving an endpoint to the trash is deliberate; an error every five minutes because of it is noise. Read how the endpoint's state is exposed to `egret-automation` — if it is not, say so rather than inventing a query.

- [ ] **Step 4: Confirm grace still applies**

`in_grace` is called at `:233` inside the alert path. Confirm by test that a `dataapi_alert` which fires twice inside its grace period delivers once. If your dispatch bypassed that call, this test is how you find out.

- [ ] **Step 5: Backend gate and commit**

---

### Task 4: Authorization

**Files:**
- Modify: `crates/egret-dataapi-api/src/lib.rs`, `crates/egret-automation/Cargo.toml`, `crates/egret-automation/src/routes.rs`, `crates/egret-automation/src/runner.rs`

**Interfaces:**
- Produces: the object-kind and read-permission constants, exported from `egret-dataapi-api`.

Creating an alert on endpoint X must require permission to read X. Without it, `alert:manage` becomes a side channel for observing an endpoint you cannot see: its error rate and traffic are information about it.

- [ ] **Step 1: Export the contract strings**

`OBJECT_KIND = "data_api_endpoint"` is `pub` at `crates/egret-dataapi/src/endpoints.rs:348`, and `EP_READ = "data_api_endpoint:read"` is a private `const` at `crates/egret-dataapi/src/routes.rs:109`. `egret-automation` must not depend on `egret-dataapi`. Move or re-export both through `egret-dataapi-api`, which today depends on **only** `egret-core` — verified, so adding it to `egret-automation` creates no cycle. `egret-automation` already consumes `egret-analytics-api` this way.

Leave `egret-dataapi` using the same constants rather than a second copy. Two string literals that must agree and are not the same literal is the drift this avoids.

- [ ] **Step 2: The save-time check**

`egret-automation`'s route state already holds `pdp` (`routes.rs:61`) and the file already calls `egret_core::authz::require`. Add the check on create and on update: the selected `dataapi_endpoint_id` must be readable by the caller. A workspace-scoped alert (`dataapi_endpoint_id IS NULL`) requires workspace-level Data API read.

Write the test as an **authorization** test, not a smoke test: a caller who can manage alerts but cannot read the endpoint must be refused. A test run as an admin proves nothing — an admin passes both checks.

- [ ] **Step 3: The run-time re-check**

**The runner has no PDP.** `pdp` appears in `routes.rs` and nowhere in `runner.rs` — confirm that yourself before starting, then thread one in.

An owner can lose access after the schedule is written. Losing it records `error` with the reason and sends nothing. The precedent is already in the codebase: a schedule whose owners are all deactivated stops running.

- [ ] **Step 4: Backend gate and commit**

---

### Task 5: The API surface

**Files:**
- Modify: `crates/egret-automation/src/model.rs`, `crates/egret-automation/src/routes.rs`, and the generated OpenAPI if this crate contributes to it

**Interfaces:**
- Consumes: the `Schedule` fields from Task 1.

- [ ] **Step 1: Extend `ScheduleType`**

`model.rs:212` reads `let is_alert = body.ty == ScheduleType::Alert;` and `:220-222` use it to null out the alert-only fields. Adding a third variant means every `is_alert` site needs re-reading — a `dataapi_alert` is not an `Alert`, and if it takes the `false` branch its own fields get nulled. **Read all of them before changing any.**

- [ ] **Step 2: Validate the shape in Rust as well as in the database**

The CHECKs from Task 1 are the backstop. The API should refuse a malformed body with a 400 that names the field, not a 500 from a constraint violation. Mirror the four CHECKs. `validate_operator_config` at `model.rs:265` is the existing pattern.

- [ ] **Step 3: Test the pairs the CHECK covers**

For each of the four constraints, assert the API returns a 4xx naming the problem — and separately assert the database still refuses the same row if the Rust check is bypassed. The two layers exist for different reasons and both are worth a test.

- [ ] **Step 4: Backend gate and commit**

---

### Task 6: The editor's source section

**Files:**
- Modify: `frontend/src/components/alerts/ScheduleEditor.tsx`

**Interfaces:**
- Consumes: the API from Task 5.

The editor's four sections are `ConfigTable` rows as of package E. The `what` section already renders two variants chosen by `isAlert`; this adds a third.

- [ ] **Step 1: Read the two existing variants first**

`{section === "what" && (isAlert ? … : …)}`. Follow their shape exactly: `ConfigSection` unchanged on the outside, one `ConfigTable` inside, one `ConfigTableRow` per field carrying its `id` as `htmlFor`.

- [ ] **Step 2: The rows**

Endpoint picker (or "whole workspace"), metric, window, operator, threshold. The operator and threshold controls already exist in the alert variant's `Condition` row — reuse that pattern rather than writing new ones.

- [ ] **Step 3: Every `details` string is a claim, so check it**

This is the constraint that cost the preceding package a whole fix wave. For each metric's help text, read `dataapi_metric.rs` from Task 2 and state what the metric actually measures — including the empty-window behaviour, because "no traffic" and "no errors" are different answers and the user cannot tell them apart from the number alone.

Grep `src/routes/explore/__tests__/ScheduleDetail.test.tsx` for the distinctive words of every string: `ConfigTableRow` renders help text **twice** in the DOM and jsdom sees both, which is how a string containing "paused" broke a test in package E.

- [ ] **Step 4: `grace_period_s`'s default is wrong for this type**

It is 14400 seconds — four hours — inherited from Superset via `0110`. That is reasonable for a business alert and long for an operational one. Give this type its own shorter default rather than inheriting silently, and say in your report what you chose and why.

- [ ] **Step 5: Frontend gate and commit**

```bash
cd frontend
npx tsc --noEmit
npx eslint .          # expect 0 errors, 177 warnings
npx vitest run
```

---

## Browser check, once, at the end

The suite cannot see whether the numbers agree with the statistics panel, which is the whole reason the definitions were copied. After Task 6:

- Create a Data API alert on an endpoint that has recent traffic. Compare the value it evaluates against the same window on the Activity page. They must agree.
- Fire a few Try-it trials against that endpoint and confirm the alert's value **does not move** — that is `actor_user_id IS NULL` working, and it is invisible to any test that does not seed a trial row.
- Put the endpoint in the trash and confirm the next run logs `noop` with a reason, not `error`.
- Confirm an empty window shows `noop` for `error_rate` and a real `0` for `call_count`.

## Self-review notes

Checked against spec §5 before saving:

- §5.1 (shape, and what is reused) — Task 3 wires the arm; the scheduler and delivery layers are untouched by every task.
- §5.2 (why this cannot be configuration) — no task adds a `query.databases` row; the evaluator reads the metastore pool directly.
- §5.3 (cron, not events) — no task subscribes to `egret-events`.
- §5.4 (the migration) — Task 1, with the constraints tested against the database rather than against Rust.
- §5.5 (copied metric definitions) — Task 2 Step 1 makes reading `activity.rs` a step rather than an instruction, and Step 4 mutation-checks both filters.
- §5.6 (the asymmetric empty window, and trashed endpoints) — Task 2 Step 2 splits the two empty-window cases into separate tests; Task 3 Step 3 handles the trashed case and tells the implementer to say so if the endpoint's state is not reachable rather than inventing a query.
- §5.7 (authorization at save and at run) — Task 4, which also corrects the spec's assumption that the runner could already do this.
- §5.8 (the editor, and the grace default) — Task 6.

Two spec claims are corrected in "What was verified" rather than carried into a task, because an implementer who believes them will write the wrong thing: the runner's dispatch is not a match, and the runner has no PDP.
