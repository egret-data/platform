# B2-2 — Object-scope for notebooks + dataset data-preview

> **Status:** Design (approved to plan) · **Date:** 2026-07-11 · **Version:** 1.0
>
> **Parent:** [Effort B authorization design](../architecture/2026-07-10-effort-b-authorization-design.md) §2 (B2) · **Predecessors:** [B2-1a object-scope backend](./2026-07-10-b2-1a-object-scope-design.md), [B2-1b sharing UI + data path](./2026-07-11-b2-1b-sharing-ui-datagap-design.md).
>
> Third B2 increment. Extends object-level sharing to **notebooks** (document + live kernel) and adds an object-scoped **dataset data-preview**. Applies the established B2-1a/B2-1b patterns to new resource types.

---

## 1. Goal & scope

Make notebooks shareable at object level like BI content — a user/group granted Viewer/Editor on a notebook (or its folder) can read/edit it and run its cells — and give datasets an object-scoped data-preview endpoint (datasets are already object-grantable from B2-1a but had no way to view their data).

**In scope:** migrate the notebook per-object handlers (`egret-notebook`) + the per-notebook kernel handlers (`egret-kernel`) to `require_object`; notebook object grants + the object-guest admission for notebooks; extend the sharing modal + a Share entry point to notebooks; a `POST /ws/{ws}/datasets/{id}/data` object-scoped dataset preview.
**Out of scope:** warehouses (their data-plane `catalog_roles`/`catalog_grants` model is B3 territory — do NOT object-scope them via `access_grants`); saved-queries (need a folder/model uplift first — separate); the deferred data-model flip; role hierarchy/deny (B2-3); per-user data-plane delegation (B3).

**Roles:** reuse `ws_viewer` (Viewer → `notebook:read` / `dataset:read`) and `ws_editor` (Editor → `notebook:write` / `dataset:manage`). ws_viewer already holds `notebook:read`; B2-1a's migration 0070 gave it `dataset:read`.

## 2. Notebooks — object-scope the document + kernel

### 2.1 Migrate the per-object handlers
`egret-notebook/src/routes.rs` per-notebook `{id}` handlers currently gate `require(&ctx, READ|WRITE)` at **workspace** scope: `get_one` (READ), `update`/`remove`/`trash`/`restore`/`set_folder`/`add_cell`/`update_cell`/`delete_cell` (WRITE), `bookmark_add`/`bookmark_remove` (READ). Migrate each to `require_object(&*st.pdp, &ctx, READ|WRITE, notebook_obj)`. `list` (READ) and `create` (WRITE) stay workspace-scope. Per the B2-1a role-based decision, drop any owner-only gate (`read_scoped`/`write_scoped` owner checks) in favor of `require_object` — a non-owner editor (member or object-Editor share) may edit; note if `read_scoped`/`write_scoped` do more than an owner check (e.g. an existence/workspace guard — keep that part).

### 2.2 Object-scope the kernel (run cells) — the notebook "data-gap" analog
`egret-kernel/src/routes.rs` handlers `start`/`status`/`stop`/`connect`/`channels` gate `require(&ctx, "notebook:read")` at workspace scope. A shared reader must be able to run/interact with the notebook's kernel. Migrate them to `require_object(&*st.pdp, &ctx, "notebook:read", notebook_obj)` (keep the existing `notebook:read` action — running in a kernel is a read-level interaction today; do not change which action each gate uses). So a Viewer-shared user can open + run cells on the shared notebook.

### 2.3 The `ObjectRef` resolver (no egret-bi dependency)
`shares::resolve` lives in `egret-bi`; `egret-notebook`/`egret-kernel` don't depend on it. Add a small resolver in each (or a shared helper): `notebook_object_ref(db, id) -> ObjectRef` = `SELECT folder_id, workspace_id FROM analytics.notebooks WHERE id=$1` → `ObjectRef { kind: "notebook", id, folder_id, workspace_id }`; `NotFound` if absent; and a cross-tenant guard `workspace_id == ctx.workspace_id` (→ NotFound) matching `shares::resolve`. (A minor duplication of the resolve pattern across seams — acceptable; a future refactor could lift a generic resolver into a shared crate.)

### 2.4 Object-guest admission for notebooks (critical)
`egret_workspace::rbac::Authorizer::has_object_grant_in_workspace` (B2-1a, admits a non-member holding an object grant) currently joins `access_grants` object rows to `folders/dashboards/charts/datasets` in the workspace. **Add the `notebook` arm** (`object_type='notebook' AND scope_id IN (SELECT id FROM analytics.notebooks WHERE workspace_id=$2)`). Without it, a user shared ONLY a notebook is rejected at the `WsContext` extractor before any notebook/kernel handler runs (the exact B2-1a gap).

## 3. Notebook sharing — the share API + modal

### 3.1 The share gate needs a per-kind "manage" action
The B2-1a share API (`egret-bi/src/shares.rs`) gates grant writes/list on `require_object(&ctx, "{kind}:manage", obj)`. Notebooks have **no** `notebook:manage` action (only `read`/`write`). Add a per-kind mapping `manage_action(kind)`: BI kinds → `"{kind}:manage"`; **`notebook` → `"notebook:write"`** (an Editor can share; a Viewer cannot). Apply it in `grant_put`/`grant_delete`/`grant_list`. Also extend `object_ref`/`object_table` in `shares.rs` to know `notebook` (`analytics.notebooks`) so the share endpoints resolve a notebook object.

### 3.2 Frontend
`ShareDialog` already takes an arbitrary `kind`/`id` — extend the role labels if needed (Viewer/Editor still map to ws_viewer/ws_editor) and add a **"Share…"** entry point on the notebook list/detail surfaces (mirror the chart/dashboard action menu). Inherited-from-folder works via the notebook's `folder_id`.

## 4. Dataset data-preview

Add `POST /ws/{ws}/datasets/{id}/data` (object-scoped) in `egret-bi`:
1. `let obj = shares::resolve(&st, &ctx, "dataset", id)…; require_object(&*st.pdp, &ctx, "dataset:read", obj)…?`
2. Load the dataset's **source** (physical `catalog.schema.table` OR the virtual `sql`) + `database_id`. Build a bounded preview query: `SELECT * FROM <source> LIMIT n` (physical: quote the identifier via the existing `schema.rs`/`chartquery` quoting; virtual: wrap the saved `sql` as a subquery with a LIMIT). Reuse the dataset→FROM logic that `ch_data`/`compile_chart_sql` already uses so quoting/dialect are consistent — do not hand-build the FROM clause.
3. `run_query(database_id, sql, Some(preview_limit), …)` → return the `ChartRunResponse { columns, rows }` DTO (reuse from B2-1b).
- **No client SQL** — the endpoint takes only `{id}` and builds `SELECT * FROM <the dataset's own source>`; a shared reader can only preview the dataset the grant covers. Bound the limit (e.g. 100–1000 rows).

## 5. Security

- Same guarantees as B2-1a/B2-1b: `require_object` gates before load/run; notebook/kernel handlers authorize on the notebook object; the dataset preview runs only the dataset's own source (no client SQL). The kernel object-scope means a shared reader reaches the kernel only for the shared notebook (the resolver + `require_object` bind the notebook id). The object-guest admission addition is scoped to notebooks IN the workspace (same join pattern as B2-1a). The notebook share gate (`notebook:write`) means only an Editor (or admin) can share — a Viewer cannot.
- Regenerate the FE api client for the dataset-preview endpoint.

## 6. Testing

- **Notebook object-scope:** an outside `ws_viewer` grant on a notebook → `GET/run` the notebook (read handlers + kernel start) = 200; a non-granted notebook → 403; an `ws_editor` grant → can edit/add-cell; a non-owner ws_editor member can edit others' notebooks (role-based). A notebook-only-shared non-member is admitted by `WsContext` (regression test for §2.4). A Viewer cannot share (403 on the share PUT); an Editor can.
- **Kernel:** an outside Viewer-shared user reaches `start`/`connect` for the shared notebook (200/authz-reached) but 403 for a non-granted notebook.
- **Dataset preview:** an outside `ws_viewer` grant on a dataset → `POST /datasets/{id}/data` returns rows (200/502-no-engine); non-granted → 403; the endpoint takes no client SQL.
- Full local gate: BE `cargo test --workspace` (pdp parity unchanged), FE `lint/typecheck/test/build`, openapi + client current.

## 7. Open questions (for the plan)

- Whether to lift a generic `object_ref` resolver into a shared crate (egret-core?) instead of duplicating across egret-bi/egret-notebook/egret-kernel — default: small per-seam duplication now, refactor later.
- Kernel `start`/`connect` currently `notebook:read`: confirm a Viewer *should* be able to run cells (read-level) vs requiring Editor (`notebook:write`) for execution. Default: keep `notebook:read` (unchanged behavior); revisit if product wants run=write.
- Notebook `read_scoped`/`write_scoped`: confirm they were owner-gates (drop, role-based) vs existence/workspace guards (keep) — inspect during the plan.
- Dataset preview limit (default 100–1000) + whether the FE gets a "Preview data" affordance in this increment or just the endpoint (default: endpoint + a minimal viewer, full UI later).

---

*Design v1.0 (2026-07-11). Extends object-level sharing to notebooks (document + kernel) and adds an object-scoped dataset data-preview, reusing the B2-1a/B2-1b patterns. Warehouses (data-plane) are B3; saved-queries need a folder uplift.*
