# Workflow package 7e — follow, eval, stats & export — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the remaining read-side of Kestra's ExecutionController/LogController in egret — poll→SSE follow (execution + logs), Pebble eval (secrets sealed on both paths), distinct namespaces/flows, 30-day average-duration, latest-per-flow, and CSV export — plus the frontend for each. No migration.

**Architecture:** New read methods on `ExecutionRepo` (`crates/egret-workflow/src/repo.rs`); new handlers beside the existing execution routes in `crates/egret-workflow/src/routes.rs`; a new eval render-context builder reading *stored* rows (new `crates/egret-workflow/src/eval.rs`), mirroring the worker's live `Surface::context` (`worker/render.rs:799-958`); a new SSE *producer* helper (new `crates/egret-workflow/src/http/follow.rs`) distinct from the existing client-side SSE *parser* (`http/sse.rs`). Frontend adds an eval panel, a CSV export button, an average-duration progress estimate, distinct-backed filter dropdowns, and `EventSource` follow with the existing polling as fallback.

**Tech Stack:** Rust (axum, sqlx, utoipa, tokio-stream), React/TypeScript (orval-generated client), Docusaurus docs.

## Global Constraints

- Conventional Commits; `Closes #N` only in the PR description, never in a commit message.
- Commit co-author trailer: `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`. PR body ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Stage by explicit path; never `git add -A`; never stage `target*/`. Never commit secrets or scratch files.
- **No migration** in this package — every endpoint is read-only.
- egret paths omit Kestra's `/actions/` segment.
- Every route gates `workflow:read` (`WF_READ`, `routes.rs:124`). None runs a task → no `egret.sql` creator gate, no `WF_MANAGE`.
- Eval seals secrets on **both** paths: the stored `inputs` column is already masked, `secret_inputs` is NOT decrypted off the worker, and the live `secret()`/`s()` resolver is NOT wired into the eval context — so `{{ secret('X') }}` fails to render (→ `error`) and a masked SECRET input renders as its mask. Additionally `redact` the mask marker.
- Each new route has a **distinct `operation_id`** (openapi drift gates do not run tsc; a duplicate operationId passes gates but breaks FE typecheck — see memory `openapi-drift-gates-dont-run-tsc`).
- Regenerate the FE client with `npm run gen:api` (never bare orval — it strips `encodeURIComponent`). Regenerate `openapi.json` with `cargo run -p egret-bin -- openapi > openapi.json`.
- Docs are bilingual: any user-visible change updates `egret-data/docs` Platform section; keep EN + VI in sync.
- Only ONE heavy Rust build at a time.
- Test DB containers for this worktree: `egret-execops-postgres` (:5465) and `egret-execops-valkey` (:6465). Start them if down (see Task 0).

---

## File Structure

- `crates/egret-workflow/src/repo.rs` — add `distinct_namespaces`, `distinct_flows`, `average_duration`, `latest_per_flow`, `export_cursor` to `impl ExecutionRepo`.
- `crates/egret-workflow/src/eval.rs` — **new**: `stored_context(detail: &ExecutionDetail, task_run_id: Option<Uuid>) -> Result<RenderContext>`; builds the render context from stored rows.
- `crates/egret-workflow/src/http/follow.rs` — **new**: `sse_follow<T>(...)` poll→SSE producer helper + `FOLLOW_POLL_INTERVAL` const (1s).
- `crates/egret-workflow/src/routes.rs` — add handlers + query/response DTOs + router wiring + `utoipa` registration for all eight endpoints.
- `crates/egret-workflow/src/lib.rs` / `http/mod.rs` — declare the two new modules.
- `crates/egret-workflow/tests/executions_routes.rs` — HTTP tests for every new route.
- `crates/egret-workflow/tests/execution_repo.rs` — repo-level tests for the new read methods.
- `openapi.json` — regenerated.
- `frontend/src/api/generated/**` — regenerated client.
- `frontend/src/components/workflows/executions/**` — eval panel, export button, avg-duration progress, distinct filters, `EventSource` follow.
- `egret-data/docs` (separate repo) — Platform docs for the new endpoints + UI.

---

## Task 0: Baseline

**Files:** none (environment only).

- [ ] **Step 1: Confirm worktree + branch.**
Run: `cd /Users/1pro-l4r8/Documents/github/data-portal/egret-execops && git rev-parse --abbrev-ref HEAD`
Expected: `feat/workflow-follow-eval-export`, tip at or above origin/main `7fd2e5e59`.

- [ ] **Step 2: Start test DB containers if down.**
Run: `docker ps --format '{{.Names}}' | grep -E 'egret-execops-(postgres|valkey)'`
If missing, start the per-worktree Postgres on :5465 and valkey on :6465 (same images the prior 7a–7d packages used), create the `egret_test` DB, and run migrations: `export DATABASE_URL=postgres://egret:egret@localhost:5465/egret_test` then `cargo sqlx migrate run` (or the repo's `just` recipe used by earlier packages).

- [ ] **Step 3: Green baseline.**
Run: `cargo test -p egret-workflow --test executions_routes --test execution_repo`
Expected: PASS. If anything fails, stop and report before implementing.

---

## Task 1: ExecutionRepo read methods (distinct, average-duration, latest, export)

**Files:**
- Modify: `crates/egret-workflow/src/repo.rs` (`impl ExecutionRepo`, near `list`/`get` ~1396-1568)
- Test: `crates/egret-workflow/tests/execution_repo.rs`

**Interfaces:**
- Consumes: `self.db.pool`, `ExecutionFilter`, `Page`, `execution_cols!`.
- Produces:
  - `pub async fn distinct_namespaces(&self, ws: Uuid) -> Result<Vec<String>>`
  - `pub async fn distinct_flows(&self, ws: Uuid, namespace: &str) -> Result<Vec<String>>`
  - `pub async fn average_duration(&self, ws: Uuid, namespace: &str, flow_id: &str) -> Result<AvgDuration>` where `pub struct AvgDuration { pub avg_duration_ms: Option<i64>, pub count: i64 }`
  - `pub async fn latest_per_flow(&self, ws: Uuid, flows: &[(String, String)]) -> Result<Vec<LastExecution>>` where `pub struct LastExecution { pub id: Uuid, pub flow_id: String, pub namespace: String, pub start_date: DateTime<Utc>, pub status: String }` (`flows` = `(namespace, flow_id)` pairs)
  - `pub fn export_cursor(&self, ws: Uuid, filter: &ExecutionFilter) -> impl Stream<Item = Result<ExecutionListRow>>` (sqlx `.fetch(...)` cursor, same row type `list` maps from)

- [ ] **Step 1: Read the executions table columns.**
Run: `grep -n 'execution_cols!' crates/egret-workflow/src/repo.rs` then read the macro and the `workflow.executions` columns (confirm the start/end timestamp column names used below; the schema is the source of truth, not this plan). Confirm which column holds the terminal state and the created/updated timestamps.

- [ ] **Step 2: Write the failing repo tests.**
Add to `tests/execution_repo.rs` (mirror the file's existing seeding helpers):

```rust
#[sqlx::test]
async fn distinct_namespaces_and_flows_are_workspace_scoped_and_ordered(pool: PgPool) {
    // Arrange: seed executions across two namespaces in ws A, one in ws B.
    // Act: distinct_namespaces(A) ; distinct_flows(A, "ns1")
    // Assert: only ws A's namespaces, sorted, deduped; only ns1's flows.
}

#[sqlx::test]
async fn average_duration_is_null_without_a_terminated_run_and_weighted_with_them(pool: PgPool) {
    // Arrange: a flow with no terminal execution -> count 0, avg None.
    //          two terminal executions with known durations in-window -> avg = mean.
    //          one terminal execution older than 30 days -> excluded.
}

#[sqlx::test]
async fn latest_per_flow_returns_the_newest_per_flow_and_omits_flows_with_none(pool: PgPool) {
    // Arrange: flow X with two executions, flow Y with one, flow Z with none.
    // Act: latest_per_flow(ws, [X, Y, Z])
    // Assert: newest of X, the one of Y, Z absent. Empty input -> empty.
}

#[sqlx::test]
async fn export_cursor_streams_every_matching_row(pool: PgPool) {
    // Arrange: N executions, a filter that narrows to M.
    // Act: collect export_cursor(ws, filter)
    // Assert: exactly M rows, newest-first, same shape list() returns.
}
```

- [ ] **Step 3: Run the tests to see them fail.**
Run: `cargo test -p egret-workflow --test execution_repo distinct_ average_duration latest_per_flow export_cursor`
Expected: FAIL (methods not defined).

- [ ] **Step 4: Implement the methods.** Plain `SELECT`s, all `WHERE workspace_id = $1`:
  - `distinct_namespaces`: `SELECT DISTINCT namespace FROM workflow.executions WHERE workspace_id = $1 ORDER BY namespace`.
  - `distinct_flows`: `... WHERE workspace_id = $1 AND namespace = $2 ORDER BY flow_id` over `DISTINCT flow_id`.
  - `average_duration`: over executions in `ws` whose state is terminal and whose start+end are set and whose created_at ≥ now() − interval '30 days':
    `SELECT (EXTRACT(EPOCH FROM AVG(<end> - <start>)) * 1000)::bigint AS avg_ms, COUNT(*) AS n` — map `avg_ms` to `None` when `n = 0`. Use the exact column names confirmed in Step 1.
  - `latest_per_flow`: build `(namespace, flow_id)` pairs; `SELECT DISTINCT ON (namespace, flow_id) id, flow_id, namespace, <start> AS start_date, state FROM workflow.executions WHERE workspace_id = $1 AND (namespace, flow_id) IN (…) ORDER BY namespace, flow_id, created_at DESC`. Empty input → return `vec![]` without a query.
  - `export_cursor`: the same `SELECT`/`WHERE`/`ORDER BY` as `list` but without `LIMIT/OFFSET`, returned via `sqlx::query_as(...).fetch(&self.db.pool)` so rows stream.

- [ ] **Step 5: Run the tests to green.**
Run: `cargo test -p egret-workflow --test execution_repo distinct_ average_duration latest_per_flow export_cursor`
Expected: PASS.

- [ ] **Step 6: Commit.**
```bash
git add crates/egret-workflow/src/repo.rs crates/egret-workflow/tests/execution_repo.rs
git commit -m "feat(workflow): execution read methods — distinct, average-duration, latest, export cursor"
```

---

## Task 2: Stats/listing/export routes (#5a, #5b, #6, #7, #8)

**Files:**
- Modify: `crates/egret-workflow/src/routes.rs` (handlers, DTOs, router block ~6067, `utoipa::path` + `paths(` list ~6216)
- Test: `crates/egret-workflow/tests/executions_routes.rs`

**Interfaces:**
- Consumes: Task 1 repo methods; `require_exec`, `egret_core::authz::require`, `WF_READ`, `WsContext`, `ExecutionFilter`, `parse_state_filter`.
- Produces routes:
  - `GET /api/ws/{workspace}/workflows/executions/namespaces` → `Json<Vec<String>>` (op_id `list_execution_namespaces`)
  - `GET /api/ws/{workspace}/workflows/executions/namespaces/{namespace}/flows` → `Json<Vec<String>>` (op_id `list_execution_namespace_flows`)
  - `GET /api/ws/{workspace}/workflows/executions/namespaces/{namespace}/flows/{flowId}/average-duration` → `Json<AvgDurationDto>` where `AvgDurationDto { avg_duration_ms: Option<i64>, count: i64 }` (op_id `execution_average_duration`)
  - `POST /api/ws/{workspace}/workflows/executions/latest` body `Json<Vec<FlowFilterDto>>` (`FlowFilterDto { namespace: String, id: String }`) → `Json<Vec<LastExecutionDto>>` (`LastExecutionDto { id, flow_id, namespace, start_date, status }`) (op_id `latest_executions`)
  - `GET /api/ws/{workspace}/workflows/executions/export.csv` with the `ListExecutionsQuery` filter (minus `limit`/`offset`) → `text/csv` streamed, `Content-Disposition: attachment; filename=executions.csv` (op_id `export_executions_csv`)

- [ ] **Step 1: Write the failing HTTP tests** in `executions_routes.rs` (mirror the existing 401/403/404/disabled-module patterns at the top of the file, using `common::get`, `common::post`, `common::json`, `insert_execution`):

```rust
#[tokio::test]
async fn distinct_namespaces_and_flows_are_workflow_read_gated_and_scoped() { /* 403 without WF_READ; only own ws */ }

#[tokio::test]
async fn average_duration_is_null_without_terminated_and_a_number_with() { /* {avgDurationMs:null,count:0} then a value */ }

#[tokio::test]
async fn latest_returns_the_newest_execution_per_requested_flow() { /* omits flows with none; [] for [] */ }

#[tokio::test]
async fn export_csv_has_a_header_and_one_row_per_matching_execution() {
    // assert content-type text/csv, Content-Disposition attachment,
    // header line + N data rows, filter narrows them.
}

#[tokio::test]
async fn the_new_read_routes_are_404_for_a_foreign_workspace_execution() { /* avg-duration/flows never 403-disclose */ }
```

- [ ] **Step 2: Run to see them fail.**
Run: `cargo test -p egret-workflow --test executions_routes distinct_ average_duration latest_ export_csv`
Expected: FAIL (routes 404/handler missing).

- [ ] **Step 3: Implement the handlers + DTOs**, each gating `egret_core::authz::require(&*st.pdp, &ctx, WF_READ).await?` first, each with a `#[utoipa::path]` doc block modelled on `list_executions`. For export, build a `StreamBody`/`axum::body::Body::from_stream` over `st.executions.export_cursor(...)` mapping each row to one CSV line (header first), setting `content-type: text/csv` and the `content-disposition` header. Reuse `ExecutionFilter` assembly from `list_executions` (extract a shared `filter_from_query` helper if cleaner).

- [ ] **Step 4: Wire the router + openapi.** Add `.route(...)` lines in the execution block (~6067) and the handler names to the `paths(` macro (~6216). Confirm each `operation_id` is unique.

- [ ] **Step 5: Run to green.**
Run: `cargo test -p egret-workflow --test executions_routes distinct_ average_duration latest_ export_csv`
Expected: PASS.

- [ ] **Step 6: Commit.**
```bash
git add crates/egret-workflow/src/routes.rs crates/egret-workflow/tests/executions_routes.rs
git commit -m "feat(workflow): distinct namespaces/flows, average-duration, latest, CSV export routes"
```

---

## Task 3: Eval — stored render context + routes (#1a, #1b)

**Files:**
- Create: `crates/egret-workflow/src/eval.rs`
- Modify: `crates/egret-workflow/src/lib.rs` (declare `pub(crate) mod eval;`), `crates/egret-workflow/src/routes.rs` (two handlers + DTO + wiring)
- Test: unit tests in `eval.rs`; HTTP tests in `tests/executions_routes.rs`

**Interfaces:**
- Consumes: `ExecutionDetail { execution, source, runs, outputs, variables, secret_inputs }` (`repo.rs` `ExecutionRepo::get`), `crate::render_str`, `RenderContext` (`runners/env.rs`), the `Flow` parse of `source`, `worker/render.rs:805-951` as the binding reference.
- Produces:
  - `eval.rs`: `pub fn stored_context(detail: &ExecutionDetail, task_run_id: Option<Uuid>) -> Result<RenderContext, EvalError>` and `pub fn eval_expression(detail: &ExecutionDetail, task_run_id: Option<Uuid>, expr: &str) -> EvalResultDto`.
  - routes: `POST .../executions/{id}/eval` (op_id `eval_execution_expression`) and `POST .../executions/{id}/task-runs/{taskRunId}/eval` (op_id `eval_task_run_expression`), both `consumes = text/plain`, body the raw expression, → `Json<EvalResultDto>` `{ result: Option<String>, error: Option<String> }`, 200 even on render error.

- [ ] **Step 1: Write failing unit tests in `eval.rs`** for `stored_context`/`eval_expression`:

```rust
// a literal renders:         "{{ 1 + 1 }}" -> result "2", error None
// execution context binds:   "{{ execution.id }}" -> the id
// a broken expression:       "{{ nope.nope }}" -> result None, error Some(_)
// a SECRET input is masked:  inputs.pw stored as the mask -> renders the mask, never the plaintext
// secret() is not wired:     "{{ secret('X') }}" -> error (not a value) on BOTH context kinds
// taskrun context binds:     with a task_run_id -> "{{ taskrun.id }}" resolves
// unknown task_run_id:       stored_context(..., Some(bad)) -> Err (maps to 404 at the route)
```

- [ ] **Step 2: Run to see them fail.**
Run: `cargo test -p egret-workflow --lib eval::`
Expected: FAIL.

- [ ] **Step 3: Implement `stored_context`.** Parse `detail.source` into a `Flow`; build a `RenderContext::new()` and insert, from stored rows, the same top-level names the worker sets (`worker/render.rs:805-951`): `inputs` (from the execution's masked `inputs`), `vars` (`detail.variables`), `flow`, `labels`, `execution`, `outputs` (from `detail.outputs` / per-task-run stored outputs), `tasks` (`{id:{state}}` from `detail.runs`). When `task_run_id` is `Some`, find the matching run in `detail.runs` (else `Err` → 404 at route) and insert `task` (`{id,type}`) and `taskrun` (`{id,startDate,attemptsCount,parentId?,value?,iteration?}`) exactly as `render.rs` shapes them. **Do not** decrypt `secret_inputs` and **do not** attach a live `secret()`/`s()` resolver; `redact` the mask marker so any masked value stays masked in output. Then `eval_expression` calls `render_str(expr, &cx)` and maps `Ok`→`result`, `Err`→`error`.

- [ ] **Step 4: Run unit tests to green.**
Run: `cargo test -p egret-workflow --lib eval::`
Expected: PASS.

- [ ] **Step 5: Write failing HTTP tests** in `executions_routes.rs`:

```rust
#[tokio::test] async fn eval_renders_in_execution_and_task_run_context() { /* 200 result for both */ }
#[tokio::test] async fn eval_returns_the_render_error_in_band_with_200() { /* {result:null,error:...} */ }
#[tokio::test] async fn eval_seals_secrets_on_both_paths() { /* secret() -> error; masked input -> mask */ }
#[tokio::test] async fn eval_is_404_for_unknown_execution_or_task_run() {}
#[tokio::test] async fn eval_requires_workflow_read() { /* 403 without WF_READ */ }
```

- [ ] **Step 6: Implement the two handlers** (load `ExecutionDetail` via `require_exec`+`st.executions.get`, call `eval::eval_expression`), wire router + `paths(`, unique op_ids, `consumes = text/plain`.

- [ ] **Step 7: Run to green.**
Run: `cargo test -p egret-workflow --test executions_routes eval_`
Expected: PASS.

- [ ] **Step 8: Commit.**
```bash
git add crates/egret-workflow/src/eval.rs crates/egret-workflow/src/lib.rs crates/egret-workflow/src/routes.rs crates/egret-workflow/tests/executions_routes.rs
git commit -m "feat(workflow): eval a Pebble expression in an execution or task-run context (secrets sealed)"
```

---

## Task 4: Poll→SSE follow (#2 execution, #3 logs)

**Files:**
- Create: `crates/egret-workflow/src/http/follow.rs`
- Modify: `crates/egret-workflow/src/http/mod.rs` (declare module), `crates/egret-workflow/src/routes.rs` (two handlers + wiring)
- Test: `crates/egret-workflow/tests/executions_routes.rs`

**Interfaces:**
- Consumes: `axum::response::sse::{Sse, Event, KeepAlive}`, `tokio_stream`, `require_exec`, `st.executions.get`/`log_status`, `st.logs.page`, `LogQuery`.
- Produces:
  - `follow.rs`: `pub const FOLLOW_POLL_INTERVAL: Duration = Duration::from_secs(1);` and a generic helper `pub fn sse_from_poll<S>(stream: S) -> Sse<S>` (sets `KeepAlive`), or a focused builder per stream if cleaner.
  - routes: `GET .../executions/{id}/follow` (op_id `follow_execution`) and `GET .../executions/{id}/logs/follow` (op_id `follow_execution_logs`), both `produces = text/event-stream`.

- [ ] **Step 1: Write failing HTTP tests** (drive the SSE as a streamed body; read events until the stream closes):

```rust
#[tokio::test]
async fn follow_execution_emits_a_snapshot_then_closes_on_terminal() {
    // Arrange: seed a RUNNING execution; after the first event, flip it to a terminal
    //          state; Assert: an initial event, a changed event, a final event, stream closed.
}
#[tokio::test]
async fn follow_logs_emits_appended_lines_then_closes_when_drained() {
    // Arrange: seed some settled lines; Assert events carry lines after the watermark,
    //          stream closes once terminal AND the tail is drained.
}
#[tokio::test]
async fn follow_is_404_before_the_stream_opens_for_a_foreign_execution() {}
#[tokio::test]
async fn follow_requires_workflow_read() {}
```

- [ ] **Step 2: Run to see them fail.**
Run: `cargo test -p egret-workflow --test executions_routes follow_`
Expected: FAIL.

- [ ] **Step 3: Implement the SSE helper + handlers.** Each handler: `require_exec(...WF_READ)` → **404 before opening the stream**; then build an `async_stream`/`tokio_stream` that:
  - **execution follow:** emit the current execution DTO (same shape as `get_execution`), then every `FOLLOW_POLL_INTERVAL` re-read via `st.executions.get`; emit only when the DTO changed; after a terminal state, emit one final event and end the stream.
  - **logs follow:** keep an `after` watermark; each tick call `st.logs.page(... after ...)`, emit any new lines, advance the watermark; also read `st.executions.log_status`; end when `complete` (terminal **and** tail drained, the 7d settle-watermark rule).
  Wrap with `Sse::new(stream).keep_alive(KeepAlive::default())`. Register router + `paths(`.

- [ ] **Step 4: Run to green.**
Run: `cargo test -p egret-workflow --test executions_routes follow_`
Expected: PASS.

- [ ] **Step 5: Commit.**
```bash
git add crates/egret-workflow/src/http/follow.rs crates/egret-workflow/src/http/mod.rs crates/egret-workflow/src/routes.rs crates/egret-workflow/tests/executions_routes.rs
git commit -m "feat(workflow): poll->SSE follow for an execution and its logs"
```

---

## Task 5: Regenerate openapi.json + frontend client

**Files:** `openapi.json`, `frontend/src/api/generated/**`

- [ ] **Step 1: Regenerate the spec.**
Run: `cargo run -p egret-bin -- openapi > openapi.json`

- [ ] **Step 2: Regenerate the client.**
Run: `cd frontend && npm run gen:api` (never bare orval).

- [ ] **Step 3: Typecheck.**
Run: `cd frontend && npm run typecheck` (or `tsc --noEmit`). Expected: no duplicate-operationId collisions (confirms Global Constraint).

- [ ] **Step 4: Commit.**
```bash
git add openapi.json frontend/src/api/generated
git commit -m "chore(api): regenerate openapi + client for package 7e endpoints"
```

---

## Task 6: Frontend — eval panel, CSV export, avg-duration progress, distinct filters, EventSource follow

**Files:** `frontend/src/components/workflows/executions/**` (+ tests under `__tests__`)

**Interfaces:** the generated hooks from Task 5; existing `useLogFollow` (`.../executions/logs/useLogFollow.ts`) stays as the polling fallback.

- [ ] **Step 1 (eval panel):** write a failing component test for an expression input that posts to `eval` and shows `result`/`error`; implement an `EvalPanel` mounted in the execution view and the task-run view; run the test to green.

- [ ] **Step 2 (CSV export button):** write a failing test asserting the button's href/stream target carries the current execution-list filter into `export.csv`; implement; green.

- [ ] **Step 3 (average-duration progress):** write a failing test that a running execution with a known `avgDurationMs` renders a bounded progress estimate; implement a small hook/component calling `#6`; green.

- [ ] **Step 4 (distinct filters):** write a failing test that the namespace/flow filter dropdowns are populated from `#5`; implement; green.

- [ ] **Step 5 (EventSource follow):** write a failing test that the execution view and log view open an `EventSource` to `#2`/`#3` and fall back to `useLogFollow` polling on error; implement; green.

- [ ] **Step 6: Frontend gates.**
Run: `cd frontend && npm run lint && npm run typecheck && npm test -- executions`
Expected: PASS.

- [ ] **Step 7: Commit.**
```bash
git add frontend/src/components/workflows/executions
git commit -m "feat(workflow): execution eval panel, CSV export, avg-duration progress, distinct filters, SSE follow"
```

---

## Task Z: Docs, gates, live-check

**Files:** `egret-data/docs` (separate repo), code gates.

- [ ] **Step 1: Docs (bilingual).** In `egret-data/docs` Platform section, document the eight endpoints and the new UI (eval panel, CSV export, progress estimate, filters, live follow), EN + VI, with refreshed screenshots for the UI. Open a docs PR (merge at once on approval per standing rule).

- [ ] **Step 2: Rust gates.**
Run: `cargo fmt --all -- --check` ; `cargo clippy -p egret-workflow --all-targets -- -D warnings` ; `cargo test -p egret-workflow`
Expected: clean.

- [ ] **Step 3: Full local CI** (the repo's standard pre-merge recipe; memory `egret-run-full-ci-local-before-merge`). Only one heavy Rust build at a time.

- [ ] **Step 4: Live-check** (recipe from memory `egret-workflow-parity-roadmap` 2026-11-14 note): run `target/debug/egret serve` on its own port with a scratch env (own DB, dummy storage creds) + a scratch `vite.<name>.config.ts` proxying `/api` (never commit it); with Playwright, start an execution, open the follow stream (watch it push then close), run an eval expression, confirm a secret expression returns an error (not a value), export CSV, and read a flow's average-duration. Record findings in the spec §findings.

- [ ] **Step 5: Push + PR.** Branch `feat/workflow-follow-eval-export`; PR body references `Closes #<platform issue if any>` and ends with the attribution line. Wait for green CI; merge under standing authority ("tự làm tiếp"). Renumber nothing (no migration). After merge, clean up per the package convention (SDD workspace, `target/`, scratch vite config, test containers as decided).

---

## Self-Review notes

- **Spec coverage:** #1a/1b eval → Task 3; #2/#3 follow → Task 4; #5a/5b distinct → Tasks 1-2; #6 avg-duration → Tasks 1-2; #7 latest → Tasks 1-2; #8 export → Tasks 1-2; FE 5 items → Task 6; divergences enforced via Global Constraints + Task 3 sealing + Task 4 poll→SSE + Task 1 30-day AVG. `#4` follow-dependencies intentionally absent (deferred).
- **No migration** anywhere (read-only), matching the spec.
- **Type consistency:** `AvgDuration`/`AvgDurationDto`, `LastExecution`/`LastExecutionDto`, `FlowFilterDto{namespace,id}`, `EvalResultDto{result,error}` are used identically across repo → route → test.
- **Exact DB column names** for average-duration/latest are confirmed against the schema in Task 1 Step 1 before the SQL is written (the one place this plan defers to the schema rather than asserting a column).
