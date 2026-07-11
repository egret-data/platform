# B2-1b — Sharing UI + object-scoped data path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make object-sharing functionally complete + usable: object-scoped server-run data endpoints so a shared user sees a shared dashboard/chart's DATA, plus the Metabase-style sharing modal.

**Architecture:** `POST /ws/{ws}/charts/{id}/data` and `POST /ws/{ws}/dashboards/{id}/cards/{card_id}/data` authorize via `require_object` then run the SAVED object's own `params.spec` (which IS a `QuerySpec`) server-side, returning rows. The FE view/render pipeline switches onto them (editor keeps `ch_data`); a sharing modal drives the B2-1a share API.

**Tech Stack:** Rust, axum, `sqlx`/Postgres, `cedar-policy`; React 18 + TS + shadcn + orval.

**Reference:** [B2-1b design](../specs/2026-07-11-b2-1b-sharing-ui-datagap-design.md). Object-scope PEP + share API are from the merged B2-1a.

---

## Task 1: Server-side "run SQL to completion" helper

**Files:** `crates/egret-query/src/execution/` (+ tests)

The chart-data endpoints must run SQL and return the completed result rows (not a client-polled handle). Trino is async (submit → poll). Provide a reusable synchronous-run function.

- [ ] **Step 1: Investigate the existing submit/poll path.** Read `crates/egret-query/src/execution/routes.rs` (`submit` handler + how results are fetched; `egret_core::TabularResult`). Determine whether there is already a function that runs a SQL string on a `database_id` to completion and returns `TabularResult` (columns+rows), or whether `submit` returns a handle the client polls.
- [ ] **Step 2: Provide `run_to_completion`.** Add (or expose) `pub async fn run_query(state/db, database_id: Uuid, sql: &str, row_limit: i64) -> Result<TabularResult>` in `egret-query` — build the connector (as the existing execution code does), execute, poll to completion for async engines, coerce rows by column type (reuse the existing coercion — see the StarRocks/Trino handling), cap at `row_limit`. Factor it out of the `submit` handler if that logic is currently inline, so both `submit` and the new BI endpoints share ONE execution path (don't duplicate connector/coercion/limit logic).
- [ ] **Step 3: Test.** A DB-gated test (or reuse the query execution test harness) that runs a trivial `SELECT 1` against a seeded database connection and asserts `TabularResult` columns/rows. If the test infra needs a live Trino/StarRocks, gate/skip like the existing execution tests and rely on a unit-level assertion of the coercion path.
- [ ] **Step 4: Commit.** `git add crates/egret-query/` → `feat(query): reusable run_query(database_id, sql) -> TabularResult` (+ trailer).

---

## Task 2: `POST /ws/{ws}/charts/{id}/data` — object-scoped chart data

**Files:** `crates/egret-bi/src/routes.rs` (or `shares.rs`), `crates/egret-analytics-api/src/bi.rs` (DTO), tests

- [ ] **Step 1: The endpoint.** Register `POST /ws/{workspace}/charts/{id}/data` on the BI chart router. Handler:
  1. `let obj = crate::shares::resolve(&st, &ctx, "chart", id).await?; egret_core::authz::require_object(&*st.pdp, &ctx, "chart:read", obj).await.map_err(BiApiError::from)?;`
  2. Load the saved chart → `dataset_id`, `params` (JSONB). Extract `params.spec` and deserialize into `chartquery::QuerySpec` (`serde_json::from_value(params["spec"].clone())`); if absent/invalid → `Error::InvalidInput("chart has no runnable query")`.
  3. Load the dataset columns + metrics (as `ch_data` does); `chartquery::build(&spec, …)` → `(sql, database_id)`.
  4. `let result = egret_query::execution::run_query(&st.<engine/db handle>, database_id, &sql, spec.row_limit).await?;` (use the Task 1 helper; egret-bi already depends on egret-query — thread the needed state handle onto `BiState` if not present).
  5. Return a `ChartRunResponse { columns, rows }` DTO (add it to `egret-analytics-api::bi` with `ToSchema`; shape it like the FE's `ChartData` expects — columns + rows).
  - The client supplies NO spec — only `{id}` — so only the saved query runs.
- [ ] **Step 2: Test** (`crates/egret-bi/tests/chart_data_object_scope.rs`, real CedarPdp after migrate, real dataset+chart+database or a mock engine): an outside `ws_viewer` object-grant on chart C → `POST /charts/{C}/data` = **200** with rows; a non-granted chart → **403**; assert the response reflects the SAVED chart's spec (there is no request body spec to spoof — the endpoint takes only the path id, structurally preventing arbitrary SQL). If a live engine isn't available in CI, assert the authz + the built SQL (return sql in a debug field or test `chartquery::build` separately) and skip the live-rows assertion with the existing engine-test gating.
- [ ] **Step 3: Commit.** `git add crates/egret-bi/ crates/egret-analytics-api/` → `feat(authz): object-scoped POST /charts/{id}/data (runs the saved chart's query)` (+ trailer).

---

## Task 3: `POST /ws/{ws}/dashboards/{id}/cards/{card_id}/data` — dashboard card data

**Files:** `crates/egret-bi/src/routes.rs`, tests

- [ ] **Step 1: The endpoint.** Register `POST /ws/{workspace}/dashboards/{id}/cards/{card_id}/data`. Handler:
  1. `require_object(&*st.pdp, &ctx, "dashboard:read", dashboard_obj)` (authorize on the DASHBOARD — transitive; NO per-chart grant needed).
  2. Verify the card belongs to the dashboard: `SELECT chart_id FROM analytics.dashboard_cards WHERE id=$card_id AND dashboard_id=$id` → else `Error::NotFound`.
  3. Load the card's chart → `params.spec` + dataset; build SQL; `run_query`; return `ChartRunResponse { columns, rows }` (same DTO as Task 2).
  4. **Dashboard-parameter filters (optional):** accept a request body `{ filters?: [...] }` of resolved dashboard-param values; if present, VALIDATE each filter column against the dataset columns (reuse the existing filter/expression validation) and merge into the spec's filters before building SQL. If you cannot safely validate in this task, omit the body and run the card unfiltered — note it (dashboard-param interactivity for shared viewers can be a follow-up). Prefer accepting validated filters so shared dashboards are interactive.
- [ ] **Step 2: Test:** a `ws_viewer` object-grant on dashboard D (NOT on the card's chart) → `POST /dashboards/{D}/cards/{card}/data` = **200** rows (transitive auth works); a `card_id` not belonging to D → **404**; a user with no grant on D → **403**.
- [ ] **Step 3: Commit.** `feat(authz): object-scoped dashboard card data (transitive on the dashboard grant)` (+ trailer).

---

## Task 4: Frontend — switch the render pipeline onto the object endpoints

**Files:** `frontend/src/api/useChartData.ts`, `frontend/src/components/chart/dashboard/DashboardCardView.tsx`, the saved-chart viewer, tests

- [ ] **Step 1: Regenerate the client** for the two new endpoints first: `cargo run -p egret-bin -- openapi > openapi.json` then `cd frontend && npm run gen:api`. The `chartRunData`/`dashboardCardData` hooks + `ChartRunResponse` type appear in `frontend/src/api/generated/`.
- [ ] **Step 2: `useChartData` gains a saved-object mode.** Today it does `build → submit → poll` (editor path). Add a variant that calls the object-scoped endpoint and returns rows in one call. Simplest: a new `useDashboardCardData(ws, dashboardId, cardId, nonce, enabled)` and `useSavedChartData(ws, chartId, nonce, enabled)` that call the generated hooks and map `ChartRunResponse` → the existing `ChartData` shape. Keep `useChartData` (build→submit) for the editor.
- [ ] **Step 3: Point the dashboard card + shared-chart view at them.** `DashboardCardView` uses `useDashboardCardData(ws, dashboardId, card.id, …)` instead of `useChartData(ws, chart)`. A read-only saved-chart view (if one exists separate from the editor) uses `useSavedChartData`. The **editor** (`ChartExplore`) is unchanged (still `ch_data`+submit). `ChartRenderer` stays the single viz dispatch.
- [ ] **Step 4: Tests** (Vitest): mock the generated hooks; assert `DashboardCardView` fetches via the card-data hook and renders rows; the editor path is untouched. Update the existing `DashboardCardView` tests (they mock `useChartData`) to the new hook.
- [ ] **Step 5: Run + commit.** `cd frontend && npm run typecheck && npm run test` (BI area) green. `git add frontend/ openapi.json` → `feat(bi): dashboard cards + shared charts fetch data via object-scoped endpoints` (+ trailer).

---

## Task 5: Frontend — the sharing modal

**Files:** `frontend/src/components/…/ShareDialog.tsx` (new) + wiring on dashboards/charts/datasets/folders, tests

- [ ] **Step 1: The modal.** A shadcn `Dialog` "Share" opened from dashboards/charts/datasets/folders. It:
  - Lists direct grants via the generated `GET /objects/{kind}/{id}/grants` hook (grantee + role Viewer/Editor).
  - Shows the item's FOLDER grants as **inherited** (read-only), labeled, by calling the folder's grants endpoint (if the item has a `folder_id`).
  - **Add**: a people/group picker (reuse the workspace-members picker component if one exists; else a simple user/group search) + a Viewer/Editor select → `PUT …/grants`.
  - **Remove**: a per-row delete → `DELETE …/grants/{grant_id}`.
  - Uses OKLCH tokens + shadcn; optimistic or refetch-on-mutate via the generated query keys.
- [ ] **Step 2: Manager-only.** The list endpoint is manage-gated (B2-1a), so a non-manager gets 403 — hide/disable the Share action when the user lacks manage (derive from the object's own permission the page already knows, or gracefully handle the 403). Do NOT show the ACL to viewers.
- [ ] **Step 3: Tests** (Vitest, mock the generated hooks): the modal lists grants, adds (calls PUT), removes (calls DELETE), renders inherited vs direct sections, and is hidden/disabled for a non-manager.
- [ ] **Step 4: Run + commit.** `cd frontend && npm run typecheck && npm run test` green. `git add frontend/` → `feat(bi): Metabase-style sharing modal (object grants)` (+ trailer).

---

## Task 6: Verify + full gate

- [ ] **Step 1: Verify** the data endpoints only accept the object id (no client spec) and are `require_object`-gated; the card endpoint authorizes on the dashboard + checks card membership. Grep-confirm no new workspace-scoped hole.
- [ ] **Step 2: Full local CI gate (sequential — never two `cargo test --workspace` at once):**
  - `rtk proxy cargo fmt --all --check`
  - `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` → clean
  - `export DATABASE_URL=postgres://egret:egret@localhost:5432/egret && rtk proxy cargo test --workspace` → 0 failures; `pdp_regression` unchanged; the new `chart_data_object_scope` / dashboard-card tests pass; `committed_openapi_json_is_current` green (openapi regenerated).
  - `cd frontend && npm run lint && npm run typecheck && npm run test && npm run build` → green.
- [ ] **Step 3: Commit** any fmt/openapi fixups → `chore(bi): B2-1b verify + gate green` (+ trailer).

---

## Self-Review Notes

- **Spec coverage:** §2.3 chart-data → Task 2; §2.4 dashboard-card-data → Task 3; §2.2 params.spec reuse → Tasks 2/3; execution helper (§6 open q) → Task 1; §3.1 render switch → Task 4; §3.2 sharing modal → Task 5.
- **Security:** data endpoints take only `{id}` (no client spec) → no arbitrary SQL; dashboard-card authorizes on the dashboard + verifies card membership; dashboard-param filters (if accepted) validated against dataset columns.
- **Behavior preservation:** the editor keeps `ch_data`+submit; only the view/card render path moves to object endpoints. `pdp_regression` untouched (no PDP change in B2-1b).
- **No placeholders:** the one genuinely-open implementation detail (the exact egret-query execution reuse point) is Task 1's explicit investigation step; the rest is concrete.

---

## Next: B2-2 / B2-3 / B2-flip

B2-2 object-scope other resource types (notebooks, saved-queries, warehouses) + dataset data-preview. B2-3 role hierarchy + explicit deny / allow-exceptions (Ranger-parity, using the forbid infra). B2-flip: the deferred data-model flip (writers → access_grants, drop the legacy tables/triggers). Then B3 (per-user data-plane delegation — which also downscopes the data-run path from the workspace connection to per-user tokens) and B4 (audit_log).
