# Workflow package 7f — bulk execution operations — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Kestra's `by-ids` / `by-query` bulk operations for the 8 execution ops egret already has single-op routes for (delete, restart, change-status, kill, resume, pause, replay, labels) — 16 endpoints — plus the frontend selection UI. No migration.

**Architecture:** A shared `bulk_apply` helper runs an all-or-nothing protocol (validate every target read-only; if any invalid → 400 with the invalids and apply nothing; else apply every target). Each op supplies a read-only `_check` closure and a mutating `_apply` closure, both factored out of that op's existing single-op handler so the single-op route and the bulk helper share one implementation. `by-query` resolves the existing `ExecutionFilter` to ids (capped) then runs the by-ids path. Synchronous inline (egret has no command queue). Frontend adds row-selection + a bulk-action toolbar + "select all matching filter" on the executions list.

**Tech Stack:** Rust (axum, sqlx, utoipa), React/TypeScript (orval client).

## Global Constraints

- Conventional Commits; `Closes #N` only in the PR description. Commit co-author trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. PR body ends `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Stage by explicit path; never `git add -A`; never stage `target*/`. No secrets/scratch committed.
- **No migration** (bulk reuses single-op data paths).
- egret paths omit Kestra's `/actions/`. Every bulk op gates `workflow:manage` (`WF_MANAGE`).
- **All-or-nothing:** validate all targets first (read-only); if ANY is invalid → `400 BulkErrorResponse { message, invalids: [{id, reason}] }` and apply nothing; else apply all → `200 BulkResponse { count }`.
- A foreign-workspace / non-existent / PDP-denied id is a **404-class invalid** (listed in `invalids`), never a 403 disclosure — matching the single-op 404-not-403 rule.
- **`MAX_BULK = 1000`** cap on the by-ids list length and the by-query match count; over the cap → 400.
- `by-query` reuses the `execution_filter` helper + `ExecutionRepo::list` (workspace-scoped) to resolve ids, then calls the by-ids path.
- Distinct `operation_id` per route (openapi drift gate doesn't run tsc). Regenerate `openapi.json` with `cargo run -p egret-bin -- openapi > openapi.json` and the FE client with `npm run gen:api` (never bare orval).
- **Run `cargo fmt --all --check` before every push** (CI's `rust` job fails fast on fmt; local test/clippy don't run fmt).
- Docs bilingual (egret-data/docs Platform section), EN + VI in sync. Only ONE heavy Rust build at a time.
- Test DB: start a per-worktree Postgres + valkey (see Task 0); env `DATABASE_URL`, `EGRET_TEST_SHARED_STORE_URL`, `EGRET_ALLOW_DOCKER_SKIP=1`, `EGRET_ALLOW_ENGINE_SKIP=1`, unset `EGRET_ALLOW_DB_SKIP`.

## File Structure

- `crates/egret-workflow/src/routes.rs` — the shared bulk machinery, the 16 handlers, DTOs, router + openapi wiring; refactor each single-op handler to expose a `_check` + `_apply` core.
- `crates/egret-workflow/src/bulk.rs` — **new (optional)**: the generic `bulk_apply` helper + `BulkResponse`/`BulkErrorResponse`/`Invalid` types + `MAX_BULK`, if cleaner than inlining in routes.rs.
- `crates/egret-workflow/tests/executions_routes.rs` — bulk HTTP tests.
- `openapi.json`, `frontend/src/api/generated/**` — regenerated.
- `frontend/src/routes/workflows/ExecutionsList.tsx` + `frontend/src/components/workflows/executions/**` — selection + bulk toolbar + confirm dialog.

## Global Interfaces (produced in Task 1, consumed by Tasks 2-5)

- `pub const MAX_BULK: usize = 1000;`
- `BulkResponse { count: usize }` (serialize `{ "count": n }`).
- `BulkErrorResponse { message: String, invalids: Vec<Invalid> }`, `Invalid { id: String, reason: String }`.
- `async fn resolve_ids_by_query(st, ctx, filter) -> Result<Vec<Uuid>, Error>` — workspace-scoped, returns every matching id (capped; over `MAX_BULK` → `Error::InvalidInput("too_many")`).
- `async fn bulk_apply<Chk, ChkFut, Ap, ApFut>(ids: Vec<Uuid>, check: Chk, apply: Ap) -> Result<Json<BulkResponse>, BulkError>` where `check: Fn(Uuid) -> ChkFut` (`ChkFut: Future<Output = Result<(), Invalid>>`, read-only) and `apply: Fn(Uuid) -> ApFut` (`ApFut: Future<Output = Result<(), Error>>`). Protocol: run `check` for every id, collect `Invalid`s; if non-empty → `400 BulkErrorResponse`; else run `apply` for every id and return `200 { count }`. (A rare apply-time race refusal is recorded and the op continues; the common path is clean — document the TOCTOU.)
- `MAX_BULK`-cap on `ids.len()` is enforced in each handler before `bulk_apply`.

---

## Task 0: Baseline

**Files:** none.

- [ ] **Step 1: Start a per-worktree test DB.** Run a Postgres 16 on a free port (e.g. 5466) + valkey 8 (e.g. 6466), create `egret_test`, run `sqlx migrate run --source ./migrations`. Export the test env (above).
- [ ] **Step 2: Green baseline.** Run: `cargo test -p egret-workflow --test executions_routes` → PASS. Stop and report if red.

---

## Task 1: Bulk machinery (DTOs, cap, by-query resolution, `bulk_apply`)

**Files:** Create `crates/egret-workflow/src/bulk.rs` (declare `mod bulk;` in `lib.rs`); Modify `routes.rs` (re-export / use).
**Interfaces:** Produces every item in **Global Interfaces** above.

- [ ] **Step 1: Write failing unit tests** for `bulk_apply` with fake closures:

```rust
// all valid -> applies all, count == n, apply ran n times
// one invalid -> Err(BulkError) carrying that Invalid, apply ran 0 times
// empty ids -> count 0, apply ran 0 times
// MAX_BULK+1 ids at a handler is rejected before bulk_apply (tested at the route in later tasks)
```

- [ ] **Step 2: Run to fail.** `cargo test -p egret-workflow --lib bulk::` → FAIL.
- [ ] **Step 3: Implement** `MAX_BULK`, the DTOs, `resolve_ids_by_query` (reuse `execution_filter` + `ExecutionRepo::list` with a limit of `MAX_BULK + 1` so an over-cap match is detected → `too_many` 400), and `bulk_apply` (validate-all → 400 on any invalid → else apply-all, count). Map `BulkError` to `(StatusCode, Json<BulkErrorResponse>)` via `IntoResponse`.
- [ ] **Step 4: Run to green.** `cargo test -p egret-workflow --lib bulk::` → PASS.
- [ ] **Step 5: Commit** (`src/bulk.rs`, `src/lib.rs`).

---

## Task 2: labels bulk (by-ids + by-query) — proves the end-to-end wiring

**Files:** `routes.rs` (factor `set_execution_labels` ~line 3201 into `labels_check` + `labels_apply`; add 2 handlers + router + openapi); `tests/executions_routes.rs`.

**Interfaces:** Consumes Task 1. Produces the labels `_check`/`_apply` pattern the other ops mirror.

- [ ] **Step 1:** Read `set_execution_labels` (~3201). Factor it into:
  - `async fn labels_check(st, ctx, ws, id) -> Result<(), Invalid>` — existence + ownership + `WF_MANAGE` per-object PDP (reuse `require_exec`/the existing gate), returning `Invalid { id, reason }` on any failure. Read-only.
  - `async fn labels_apply(st, ctx, ws, caller, id, body: &LabelsBody) -> Result<(), Error>` — the label replacement + audit the single-op does.
  The single-op handler is rewritten to call `labels_check(...)?` then `labels_apply(...)` so there is ONE implementation.
- [ ] **Step 2: Write failing HTTP tests:** by-ids applies labels to all when valid (200, count); by-ids with one unknown id → 400 `BulkErrorResponse` and NO labels changed on the valid ones; by-query resolves a filter and applies to the matched set; `MAX_BULK+1` → 400 `too_many`; `workflow:read`-only caller → 403; empty list → 200 count 0.
- [ ] **Step 3: Run to fail.** `cargo test -p egret-workflow --test executions_routes labels_by` → FAIL.
- [ ] **Step 4: Implement** `POST .../executions/labels/by-ids` (op_id `labels_executions_by_ids`, body `{ ids: [uuid], labels: [...] }` — match the single-op's labels body shape alongside the id list) and `POST .../executions/labels/by-query` (op_id `labels_executions_by_query`, labels in body + filter in query). Each gates `WF_MANAGE`, caps at `MAX_BULK`, calls `bulk_apply(ids, labels_check, |id| labels_apply(...))`. Wire router (~6067) + `paths(` (~6216), distinct op_ids.
- [ ] **Step 5: Run to green.** PASS.
- [ ] **Step 6: Commit** (`routes.rs`, `tests/executions_routes.rs`).

---

## Task 3: state-transition ops bulk — change-status, kill, pause, resume

**Files:** `routes.rs` (factor `change_execution_status` (the `/status` handler), `kill_execution` ~5426, `pause_execution` ~4611, `resume_execution` ~4822 into `_check`+`_apply` each; 8 handlers); `tests/executions_routes.rs`.

**Interfaces:** Consumes Task 1 + the Task 2 pattern.

For EACH of the four ops, mirror Task 2:
- [ ] **Step 1:** Factor the single-op handler into `<op>_check` (read-only: existence + ownership + PDP + the op's STATE precondition — e.g. kill requires a non-terminal/running state, resume requires PAUSED, pause requires running, change-status requires a legal source→target) returning `Invalid` on any failure, and `<op>_apply` (the mutation + audit). Rewrite the single-op route to call both.
- [ ] **Step 2:** Failing HTTP tests per op: by-ids all-valid (200 count); one wrong-state id → 400 (nothing applied — assert the valid ones kept their state); by-query matched set; `workflow:read` → 403. Plus ONE cross-op atomicity test: a 3-id batch where #2 is wrong-state leaves #1 and #3 unchanged.
- [ ] **Step 3:** Run to fail.
- [ ] **Step 4:** Implement the 8 routes (`{op}/by-ids`, `{op}/by-query`) for change-status/kill/pause/resume. kill is `DELETE` (Kestra uses `@Delete(uri="/kill/by-ids")`); change-status/pause/resume are `POST`. change-status + resume carry their single-op body (the target status / the resume inputs) alongside the ids/filter. Distinct op_ids. Router + `paths(`.
- [ ] **Step 5:** Run to green.
- [ ] **Step 6: Commit.**

> Note: `resume` bulk carries `onResume` inputs (7c). Applying the SAME inputs to many paused executions is the Kestra semantic; validate each is PAUSED-by-hand/waiting in `resume_check`.

---

## Task 4: restart + replay bulk

**Files:** `routes.rs` (factor `restart_execution` ~6030 and `replay_execution` ~5132 into `_check`+`_apply`; 4 handlers); `tests/executions_routes.rs`.

- [ ] **Step 1:** Factor each. `restart_check`: existence + ownership + rerun_gate (the creator/disabled checks) + the loop-iteration refusal + state (terminal/paused). `restart_apply`: the frozen in-place restart OR the restart-on-source replay the single-op does. `replay_check`/`replay_apply` similarly (replay needs a `taskRunId` per execution — **bulk replay uses the default/frozen replay without a per-id taskRunId**, matching Kestra's `replay/by-ids` which takes only ids + `latestRevision`; confirm against Kestra `replay/by-ids` at ExecutionController ~2139). Rewrite both single-op routes to call their cores.
- [ ] **Step 2:** Failing HTTP tests: restart by-ids all-valid (count; the restarted ones changed state), one non-restartable → 400 nothing applied; restart by-query; replay by-ids creates N new executions (count) / by-query; `latestRevision` query honored; `workflow:read` → 403.
- [ ] **Step 3:** Run to fail.
- [ ] **Step 4:** Implement `restart/by-ids`, `restart/by-query`, `replay/by-ids`, `replay/by-query` (POST; carry `latestRevision`/`revision` query as the single-op does). Distinct op_ids. Router + `paths(`.
- [ ] **Step 5:** Run to green.
- [ ] **Step 6: Commit.**

---

## Task 5: delete bulk

**Files:** `routes.rs` (factor `delete_execution` ~3117 into `delete_check`+`delete_apply`; 2 handlers); `tests/executions_routes.rs`.

- [ ] **Step 1:** Factor `delete_execution` (which handles logs + optional storage + the family-fixpoint lock + the not_terminated/not_a_root/has_dependents/delete_logs_required refusals from 7d). `delete_check`: existence + ownership + PDP + the 7d preconditions (terminated, is-a-root, no dependents, delete_logs required). `delete_apply`: the delete (+logs, optional storage). Rewrite the single-op route to call both. The delete query flags (`delete_storage`, `delete_logs`, `delete_metrics`) apply to the whole batch.
- [ ] **Step 2:** Failing HTTP tests: by-ids deletes all valid (200 count, rows gone); one non-terminated / non-root / has-dependents id → 400 nothing deleted (assert the others still exist); by-query; `delete_logs=false` → 400 `delete_logs_required`; `workflow:read` → 403. Paths: Kestra's delete bulk is the BARE `DELETE .../executions/by-ids` and `.../executions/by-query` (not `delete/by-ids`) — match that.
- [ ] **Step 3:** Run to fail.
- [ ] **Step 4:** Implement `DELETE .../executions/by-ids` (op_id `delete_executions_by_ids`) and `DELETE .../executions/by-query` (op_id `delete_executions_by_query`), carrying the `DeleteExecutionQuery` flags. Router + `paths(`.
- [ ] **Step 5:** Run to green.
- [ ] **Step 6: Commit.**

---

## Task 6: Regenerate openapi.json + frontend client

**Files:** `openapi.json`, `frontend/src/api/generated/**`.

- [ ] **Step 1:** `cargo run -p egret-bin -- openapi > openapi.json`.
- [ ] **Step 2:** `cd frontend && npm run gen:api`.
- [ ] **Step 3:** `cd frontend && npm run typecheck` (catch duplicate-operationId collisions). Confirm the 16 new op_ids appear once each in openapi.json.
- [ ] **Step 4:** Confirm the diff is only `openapi.json` + `frontend/src/api/generated`. Commit.

---

## Task 7: Frontend — row selection, bulk-action toolbar, select-all-matching-filter

**Files:** `frontend/src/routes/workflows/ExecutionsList.tsx` + `frontend/src/components/workflows/executions/**` (+ `__tests__`). Follow `docs/en/frontend-rules.md`.

- [ ] **Step 1 (selection):** failing test → add a checkbox column + a header "select page" checkbox + selection state; implement; green.
- [ ] **Step 2 (toolbar):** failing test → a bulk-action toolbar appears when ≥1 row is selected, with Restart / Kill / Delete / Change status / Replay / Resume / Pause / Edit labels, each ENABLED only when the whole selection is eligible (mirror the single-op button enablement); implement calling the `by-ids` hooks; green.
- [ ] **Step 3 (select-all-matching-filter):** failing test → a "select all N matching the filter" affordance that switches the action to the `by-query` variant (sends the current filter, not the id list); implement; green.
- [ ] **Step 4 (destructive confirm + error surfacing):** failing test → delete/kill open a confirm dialog showing the count; a `400 BulkErrorResponse` surfaces which ids were rejected and why; implement; green.
- [ ] **Step 5: Gates.** `cd frontend && npm run lint && npm run typecheck && npm test -- executions routes/workflows` → green.
- [ ] **Step 6: Commit.**

---

## Task Z: Docs, gates, live-check, merge

- [ ] **Step 1: Docs (bilingual).** egret-data/docs Platform section: document the 16 bulk endpoints + the selection UI, EN + VI, refreshed screenshots. Docs PR merges at once on approval.
- [ ] **Step 2: Rust gates.** `cargo fmt --all --check` ; `cargo clippy --workspace --all-targets -- -D warnings` ; `EGRET_ALLOW_DOCKER_SKIP=1 EGRET_ALLOW_ENGINE_SKIP=1 cargo test -p egret-workflow`.
- [ ] **Step 3: Full-crate test** under the correct gating env (confirm `executions_routes` + `execution_repo` green; the `container_docker_live` binary self-skips with `EGRET_ALLOW_DOCKER_SKIP=1`).
- [ ] **Step 4: Live-check** (recipe from memory): bulk restart/kill/delete/labels a handful of seeded executions in the browser; confirm all-or-nothing (a mixed selection is rejected with the invalids) and select-all-matching-filter.
- [ ] **Step 5: Push + PR.** Branch `feat/workflow-bulk-ops`; green CI → merge (NO `--admin`; standing autonomy). Clean up worktree/containers after merge.

---

## Self-Review notes

- **Spec coverage:** 8 ops × {by-ids, by-query} = 16 endpoints → Tasks 2-5; all-or-nothing + cap + 404-not-403 → Task 1 machinery + each op's `_check`; sync `BulkResponse`/`BulkErrorResponse` → Task 1 DTOs; FE full → Task 7; divergences (sync/cap/deferred unqueue+force-run/no `/actions/`) → Global Constraints. unqueue/force-run intentionally absent (deferred to #8).
- **No migration** anywhere.
- **Type consistency:** `BulkResponse{count}`, `BulkErrorResponse{message,invalids}`, `Invalid{id,reason}`, `MAX_BULK` used identically across Task 1 → all op tasks → tests. Each `<op>_check`/`<op>_apply` is called by BOTH the single-op route (rewritten) and the bulk handler — one implementation, no divergence.
- **Refactor safety:** every single-op route is rewritten to call its new `_check`+`_apply` core; its EXISTING single-op tests must stay green (run them in each op's task), proving the refactor preserved behavior.
