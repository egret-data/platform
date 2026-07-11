# B2-2 — Object-scope notebooks + dataset preview Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend object-level sharing to notebooks (document + kernel) and add an object-scoped dataset data-preview, reusing the merged B2-1a/B2-1b patterns.

**Architecture:** Migrate notebook + kernel per-object handlers to `require_object`; add notebooks to the object-guest `WsContext` admission; map the share gate to a per-kind manage-action (`notebook:write`); a `POST /datasets/{id}/data` preview endpoint runs the dataset's own source via `run_query`.

**Tech Stack:** Rust, axum, `sqlx`/Postgres, `cedar-policy`; React/orval.

**Reference:** [B2-2 design](../specs/2026-07-11-b2-2-notebooks-dataset-preview-design.md). The merged B2-1a (`require_object`, `shares::resolve`, `has_object_grant_in_workspace`, object-guest admission) and B2-1b (`run_query`, object-scoped data endpoints, `ShareDialog`) are the templates — READ them.

---

## Task 1: Shared plumbing — object-guest admission for notebooks + share-gate per-kind manage-action

**Files:** `crates/egret-workspace/src/rbac.rs`, `crates/egret-bi/src/shares.rs`, tests

- [ ] **Step 1: Admit notebook object-guests.** In `egret-workspace/src/rbac.rs::has_object_grant_in_workspace` (added in B2-1a — it `EXISTS`-joins `access_grants` object rows to `folders/dashboards/charts/datasets` in the workspace), ADD a `notebook` arm: `OR (ag.object_type='notebook' AND ag.scope_id IN (SELECT id FROM analytics.notebooks WHERE workspace_id=$2))`. Without this, a notebook-only-shared non-member is 403'd at the `WsContext` extractor.
- [ ] **Step 2: Per-kind manage-action in the share API.** In `egret-bi/src/shares.rs`: (a) extend the `object_ref`/`object_table` mapping to know `kind == "notebook"` → `analytics.notebooks` (folder_id + workspace_id columns). (b) add `fn manage_action(kind: &str) -> String { if kind == "notebook" { "notebook:write".into() } else { format!("{kind}:manage") } }` and use it in `grant_put`/`grant_delete`/`grant_list`'s `require_object` gate (replacing the hard-coded `{kind}:manage`). So sharing a notebook requires `notebook:write` (Editor), a dashboard/chart/dataset requires `:manage` — unchanged for BI kinds.
- [ ] **Step 3: Tests.** egret-workspace: extend the object-guest admission test — a user with ONLY a notebook object grant in the workspace is admitted (non-403 at WsContext). egret-bi shares: a notebook Editor (ws_editor) can `PUT` a grant on a notebook (2xx); a notebook Viewer (ws_viewer, has notebook:read but not :write) canNOT (403). Reuse the B2-1a share tests as the template.
- [ ] **Step 4: Run + commit.** `export DATABASE_URL=… && rtk proxy cargo test -p egret-workspace -p egret-bi -- --nocapture`; `cargo build --workspace`. Commit `feat(authz): notebook object-guest admission + per-kind share manage-action` (+ trailer).

---

## Task 2: Migrate notebook per-object handlers to object scope

**Files:** `crates/egret-notebook/src/routes.rs` (+ `runtime.rs` if it has a per-notebook gate), tests

- [ ] **Step 1: Notebook `ObjectRef` resolver.** Add `async fn notebook_object_ref(db: &Db, ctx: &WorkspaceContext, id: Uuid) -> Result<ObjectRef>` in egret-notebook: `SELECT folder_id, workspace_id FROM analytics.notebooks WHERE id=$1` → `ObjectRef{ kind:"notebook", id, folder_id, workspace_id }`; `NotFound` if absent OR if `workspace_id != ctx.workspace_id` (cross-tenant guard, mirroring `shares::resolve`).
- [ ] **Step 2: Migrate the `{id}` handlers.** `rtk proxy grep -n "async fn \|require(&\*st.pdp, &ctx, \(READ\|WRITE\)\|read_scoped\|write_scoped" crates/egret-notebook/src/routes.rs`. For each per-notebook `{id}` handler (`get_one` READ; `update`/`remove`/`trash`/`restore`/`set_folder`/`add_cell`/`update_cell`/`delete_cell` WRITE; `bookmark_add`/`bookmark_remove` READ), replace `require(&*st.pdp, &ctx, READ|WRITE)` with `require_object(&*st.pdp, &ctx, READ|WRITE, notebook_object_ref(&st.db, &ctx, id).await?)`. `list` (READ) + `create` (WRITE) stay workspace-scope. **Owner gates:** inspect `read_scoped`/`write_scoped` — if they enforce owner-only, REMOVE that (role-based, per B2-1a: a non-owner editor may edit); if they also do an existence/workspace guard, keep that part (or rely on the resolver's guard). Report what each did.
- [ ] **Step 3: Test.** `crates/egret-notebook/tests/notebook_object_scope.rs` (new, real CedarPdp after migrate): an outside `ws_viewer` object-grant on notebook N → `GET /notebooks/{N}` = 200, a non-granted N2 = 403, `PUT`/add-cell = 403 (viewer). An outside `ws_editor` grant → can edit N (2xx), not N2. A non-owner ws_editor member edits another's notebook = 2xx (role-based). A notebook-only-shared non-member is admitted (reaches the handler).
- [ ] **Step 4: Run + commit.** `cargo test -p egret-notebook`; `cargo build --workspace`; `clippy -p egret-notebook -D warnings`. Commit `feat(authz): migrate notebook per-object handlers to object scope (role-based)` (+ trailer).

---

## Task 3: Object-scope the kernel (run cells) for shared notebooks

**Files:** `crates/egret-kernel/src/routes.rs`, tests

- [ ] **Step 1: Notebook resolver in kernel.** egret-kernel doesn't depend on egret-bi/egret-notebook's resolver — add the same small `notebook_object_ref(db, ctx, id)` helper (or, if egret-kernel already depends on egret-notebook, reuse it; check the dep graph). Same SQL + cross-tenant guard as Task 2.
- [ ] **Step 2: Migrate the handlers.** `start`/`status`/`stop`/`connect`/`channels` (+ `runtime.rs`'s per-notebook gate if present) gate `require(&ctx, "notebook:read")` at workspace scope. Replace with `require_object(&*st.pdp, &ctx, "notebook:read", notebook_object_ref(&st.db/&…, &ctx, notebook_id).await?)`. Keep the `notebook:read` action (running in a kernel is read-level today). Confirm the notebook id is in scope for each handler (they operate on a notebook's kernel).
- [ ] **Step 3: Test.** `crates/egret-kernel/tests/kernel_object_scope.rs` (new): an outside `ws_viewer` object-grant on notebook N → `start`/`connect` for N reaches the handler (200/authz-reached — gate live-kernel assertions like the existing kernel tests); a non-granted N2 → 403.
- [ ] **Step 4: Run + commit.** `cargo test -p egret-kernel`; `cargo build --workspace`; `clippy -p egret-kernel -D warnings`. Commit `feat(authz): object-scope kernel handlers for shared notebooks` (+ trailer).

---

## Task 4: Dataset data-preview endpoint

**Files:** `crates/egret-bi/src/routes.rs`, `crates/egret-analytics-api/src/bi.rs` (reuse `ChartRunResponse`), tests

- [ ] **Step 1: The endpoint `POST /ws/{workspace}/datasets/{id}/data`.** Handler (`ds_run`):
  1. `let obj = crate::shares::resolve(&st, &ctx, "dataset", id).await?; require_object(&*st.pdp, &ctx, DS_READ, obj).await.map_err(BiApiError::from)?;`
  2. Load the dataset's source (physical `catalog`/`schema_name`/`table_name` OR virtual `sql`) + `database_id` (`DatasetRepo::get`). Build a bounded preview SQL: physical → `SELECT * FROM <quoted catalog.schema.table> LIMIT n`; virtual → wrap the saved `sql` as `SELECT * FROM (<sql>) t LIMIT n`. **Reuse** the dataset→FROM / identifier-quoting logic the query builder already uses (`schema.rs`/`chartquery`/`compile_chart_sql`'s source handling) — do NOT hand-build quoting. Bound the limit (const `DATASET_PREVIEW_ROWS = 1000`).
  3. `run_query(&st.databases, &DefaultConnectorFactory, database_id, &sql, Some(DATASET_PREVIEW_ROWS), Some(ctx.principal.email.clone()), &chart_run_limits(), CancellationToken::new())` → return `ChartRunResponse { columns, rows }` (reuse the B2-1b DTO). Takes ONLY the path `{id}` — no client SQL. Register the route + OpenAPI.
- [ ] **Step 2: Test.** `crates/egret-bi/tests/dataset_data_object_scope.rs`: an outside `ws_viewer` grant on dataset DS → `POST /datasets/{DS}/data` reaches execution (200/502-no-engine); a non-granted DS2 → 403; no client-SQL accepted (path id only).
- [ ] **Step 3: Run + commit.** `cargo test -p egret-bi`; `cargo build --workspace`; `clippy -p egret-bi -D warnings`. Commit `feat(authz): object-scoped POST /datasets/{id}/data preview (runs the dataset's own source)` (+ trailer).

---

## Task 5: Frontend — notebook sharing + dataset-preview client

**Files:** `frontend/src/…` (notebook surfaces + ShareDialog wiring), regen client

- [ ] **Step 1: Regen the client** for the new `ds_run` (dataset data) endpoint: `cargo run -p egret-bin -- openapi > openapi.json && cd frontend && npm run gen:api`.
- [ ] **Step 2: Notebook Share entry.** `ShareDialog` already takes an arbitrary `kind`/`id` — add a **"Share…"** action on the notebook list/detail surfaces (mirror the chart/dashboard action-menu wiring from B2-1b: an `onShare` prop + a `<ShareDialog kind="notebook" id={nb.id} folderId={nb.folder_id} …>`). Roles stay Viewer/Editor. Confirm the modal works for `kind="notebook"` (the share API now maps its manage-gate to `notebook:write`).
- [ ] **Step 3: (Optional, minimal) dataset preview affordance.** If a dataset detail page exists, add a minimal "Preview data" that calls the new `dsRun` hook and renders rows in a simple table (reuse `ChartTable`/a basic grid). If it's large, just ship the generated hook + note the UI is a follow-up. Prefer a minimal viewer so dataset shares are usable.
- [ ] **Step 4: Tests (Vitest).** The notebook Share action opens `ShareDialog` with `kind="notebook"`; if you added the preview, mock the `dsRun` hook and assert it renders rows. Keep existing notebook tests green.
- [ ] **Step 5: Run + commit.** `cd frontend && npm run typecheck && npm run test -- --run && npm run lint && npm run build`. Commit `feat(bi): notebook sharing entry + dataset-preview client` (+ trailer).

---

## Task 6: Verify + full gate

- [ ] **Step 1: Verify** every notebook `{id}` handler + kernel per-notebook handler uses `require_object` (list/create stay workspace-scope); `has_object_grant_in_workspace` includes the notebook arm; the share gate uses `notebook:write` for notebooks; `ds_run` takes only the path id. Grep-confirm; report.
- [ ] **Step 2: Full local CI gate (sequential — never two `cargo test --workspace` at once):**
  - `rtk proxy cargo fmt --all --check`
  - `rtk proxy cargo clippy --workspace --all-targets -- -D warnings` → clean
  - `export DATABASE_URL=… && rtk proxy cargo test --workspace` → 0 failures; `pdp_regression` unchanged; the new object-scope tests pass; `committed_openapi_json_is_current` green.
  - `cd frontend && npm run lint && npm run typecheck && npm run test -- --run && npm run build` → green.
- [ ] **Step 3: Commit** any fmt/openapi fixups → `chore(authz): B2-2 verify + gate green` (+ trailer).

---

## Self-Review Notes

- **Spec coverage:** §2.4 object-guest + §3.1 share-gate → Task 1; §2.1 notebook handlers → Task 2; §2.2 kernel → Task 3; §4 dataset preview → Task 4; §3.2 FE → Task 5.
- **Security:** notebook/kernel/dataset endpoints gate via `require_object` before load/run; notebook-only-shared non-members admitted via the new `has_object_grant_in_workspace` arm (scoped to notebooks IN the workspace); dataset preview runs only the dataset's own source (no client SQL); notebook share requires `notebook:write`.
- **Behavior preservation:** role-based edit (drop owner gates, per B2-1a); kernel keeps `notebook:read`; `pdp_regression` untouched (no PDP change).
- **No placeholders:** the resolver duplication + owner-gate inspection are explicit in-task investigation steps.

---

## Next: B2-3 / B2-flip / B3

B2-3 role hierarchy + explicit deny / allow-exceptions (Ranger-parity, forbid infra). B2-flip: the deferred data-model flip (writers → access_grants, drop legacy tables/triggers). B3 per-user data-plane delegation — replaces the machine-JWT/workspace-connection data path with per-user downscoped tokens (also downscopes the B2-1b/B2-2 data-run endpoints from the workspace connection) + federates control-plane grants into Ranger; **warehouses' object authz belongs here** (their `catalog_grants` model). B4 audit_log.
