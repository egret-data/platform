# Notebook C1 — Notebook Core (design)

**Status:** draft · **Date:** 2026-06-29 · **Epic:** Notebooks (modeled on sparklabx, Daft+Spark-Connect engines)

## 1. Context & scope

egret is adding a Jupyter-style notebook per workspace, modeled on **sparklabx**
but adapted to egret's Rust stack and **per-workspace** product model. The full
notebook is decomposed into independent sub-specs, each its own spec → plan →
build (like catalog part A shipped):

- **C1 — Notebook core** *(this spec)*: persistence + CRUD API + editor UI.
  **No execution yet.**
- **F1 — Access foundation**: Trino `X-Trino-User` impersonation (governed reads
  via Ranger, per end-user) + auto-provision the per-user `notebook_{user}`
  namespace + a 1-grant catalog token.
- **D1 — Daft kernel runtime**: `KernelLauncher` trait + subprocess impl, kernel
  registry, idle reaper, WebSocket proxy to the Jupyter protocol.
- **E1 — Execution**: wire cell run → kernel; Python (Daft) · SQL (Trino) ·
  Markdown; stream outputs, render DataFrames, persist `last_output`.
- **Later**: Spark Connect engine picker · StarRocks per-user · docker/k8s
  launchers · per-user MinIO IAM / vended credentials.

The locked access model (see memory `egret-notebook-access-model`) is a **two-plane**
design: governed lakehouse data is **read-only** to notebooks via Trino+Ranger
(per-user), while each user gets a private read/write namespace `notebook_{user}`
on the REST catalog. **C1 builds none of that** — C1 is the notebook document
itself: create/edit/save notebooks and cells, with a Monaco editor. It is
independently usable (you can author and persist a notebook; you just can't run
it until D1/E1).

### In scope (C1)
- Postgres persistence for notebooks and their cells, scoped per workspace.
- CRUD REST API on the workspace session plane (`/api/ws/{workspace}/notebooks…`).
- Ownership model: read = owner **or** public; write/delete = owner only.
- Frontend: notebook list page (create/rename/delete/open) and a notebook editor
  page (ordered cells, Monaco for code, Markdown render, add/reorder/delete cell,
  autosave of cell source + notebook metadata).
- The `engine` field on a notebook (`daft` default, `spark_connect`) is **stored
  and editable** but unused until D1/E1.

### Out of scope (C1 — deferred to later sub-specs)
- Any code execution, kernels, output streaming (D1/E1).
- The `notebook_{user}` namespace, catalog token minting, Trino impersonation (F1).
- Sharing beyond the single `is_public` flag (no per-user ACLs).
- Realtime collaboration, version history, import/export (.ipynb).

## 2. Data model

New migration `migrations/0037_notebooks.sql`. After adding it, **touch
`crates/egret-metastore/src/lib.rs`** so the embedded migrator re-embeds (see
memory `egret-sqlx-migration-staleness`).

```sql
CREATE TABLE notebooks (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id  UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    owner_user_id UUID NOT NULL REFERENCES users(id),
    name          TEXT NOT NULL DEFAULT 'Untitled',
    description   TEXT NOT NULL DEFAULT '',
    engine        TEXT NOT NULL DEFAULT 'daft',   -- 'daft' | 'spark_connect' (unused in C1)
    is_public     BOOLEAN NOT NULL DEFAULT false, -- visible read-only to other workspace members
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_notebooks_ws_owner ON notebooks(workspace_id, owner_user_id);

CREATE TABLE notebook_cells (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    notebook_id  UUID NOT NULL REFERENCES notebooks(id) ON DELETE CASCADE,
    cell_type    TEXT NOT NULL DEFAULT 'code',  -- 'code' | 'markdown' | 'sql'
    source       TEXT NOT NULL DEFAULT '',
    cell_order   INTEGER NOT NULL DEFAULT 0,
    -- execution columns are written by E1; present now so the shape is stable.
    execution_count        INTEGER,
    last_output            JSONB,
    last_execution_time_ms INTEGER,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_notebook_cells_notebook ON notebook_cells(notebook_id, cell_order);
```

Notes:
- `workspace_id` makes notebooks per-workspace (egret's model; sparklabx is global).
- The execution columns exist now (cheap, avoids a later migration) but are only
  populated by E1.
- `engine`, `cell_type` are validated in the API layer to the enumerated values
  (reject unknown → 400), not via DB CHECK, to keep adding engines/cell-types a
  code-only change.

## 3. Backend — new crate `egret-notebook`

Mirror `egret-engine`'s layout and `Cargo.toml` (workspace deps:
`egret-core`, `egret-metastore`, `egret-auth`, `egret-workspace`, axum, sqlx,
serde, uuid, chrono, thiserror, tracing). Modules:

- `model.rs` — domain types + DTOs: `Notebook`, `Cell`, `CellType`, `Engine`,
  `CreateNotebook`, `UpdateNotebook`, `CreateCell`, `UpdateCell`, `ReorderCells`.
- `store.rs` — `NotebookStore { db }` mirroring `QueryStore`: row structs +
  `into_*` mappers, parameterized sqlx, no string interpolation of user input.
- `routes.rs` — `notebook_router() -> Router<NotebookState>` + handlers.
- `error.rs` — `NotebookApiError` mapping to status codes (mirror `EngineApiError`).
- `lib.rs` — re-exports + `notebook_router`.

`NotebookState` holds `store: NotebookStore`, `sessions: SessionService`,
`workspaces: WorkspaceRepo`, `authz: Authorizer`, with `FromRef` impls (mirror
`EngineState`).

### API (workspace session plane)

All under `/ws/{workspace}/notebooks`, mounted by egret-server as
`.nest("/api", egret_notebook::notebook_router())` with the `csrf_layer` and
`.with_state(notebook)` (mirror the engine/bi blocks in
`egret-server/src/lib.rs`). Every handler uses `WsContext(ctx)` and
`require_workspace(ctx, PERM)`.

| Method & path | Perm | Behavior |
|---|---|---|
| `GET    /ws/{ws}/notebooks` | `notebook:read` | List notebooks visible to the caller: own + public, in this workspace. |
| `POST   /ws/{ws}/notebooks` | `notebook:write` | Create (owner = caller). Returns the notebook with an empty cell list (or one empty code cell — see §6). |
| `GET    /ws/{ws}/notebooks/{id}` | `notebook:read` | Get one notebook **with its ordered cells**. 404 if not in ws; 403 if not own and not public. |
| `PATCH  /ws/{ws}/notebooks/{id}` | `notebook:write` | Update name/description/engine/is_public. Owner only. |
| `DELETE /ws/{ws}/notebooks/{id}` | `notebook:write` | Delete (cascades cells). Owner only. |
| `POST   /ws/{ws}/notebooks/{id}/cells` | `notebook:write` | Append a cell (type + source + order). Owner only. |
| `PATCH  /ws/{ws}/notebooks/{id}/cells/{cid}` | `notebook:write` | Update a cell's source/type. Owner only. |
| `DELETE /ws/{ws}/notebooks/{id}/cells/{cid}` | `notebook:write` | Delete a cell. Owner only. |
| `PUT    /ws/{ws}/notebooks/{id}/cells/order` | `notebook:write` | Reorder: body is the full ordered list of cell ids; sets `cell_order`. Owner only. |

### Ownership / authorization

Two RBAC perms seeded in the migration and granted to workspace roles, mirroring
how `catalog:read|write` were seeded (migration 0024): `notebook:read`,
`notebook:write`. The coarse `require_workspace(ctx, PERM)` gate ensures the
caller is a member with the perm; per-record ownership is enforced by an `owned`
helper (mirror `egret-engine::routes::owned`) with this notebook-specific policy:

- **Read** (`GET` one): allowed if `notebook.workspace_id == ctx.workspace_id`
  **and** (`owner_user_id == caller` **or** `is_public`). Else 404 (cross-ws) / 403.
- **Write** (`PATCH`/`DELETE`/cell mutations): allowed only if
  `owner_user_id == caller`. **No platform-admin override** — a notebook is a
  private workspace, matching sparklabx's deliberate choice. (This intentionally
  differs from `egret-engine`'s `owned`, which lets platform_admin read any
  query; we do **not** widen notebooks that way.)
- List: store-side filter `WHERE workspace_id=$1 AND (owner_user_id=$2 OR is_public)`.

All cell routes resolve the parent notebook first and verify it belongs to the
path workspace and the caller may write it — closing the nested-resource IDOR
(see memory `api-nested-resource-ownership-scoping`).

## 4. Frontend

Reuses the existing Explore placement — the **route and sidebar item already
exist** (`App.tsx` `notebooks`; `Sidebar.tsx` Explore group "Notebooks").

- `src/api/notebooks.ts` — react-query client mirroring `api/sqllab.ts`:
  `qk` keys, `useNotebooks(ws)`, `useNotebook(ws, id)`, `useCreateNotebook`,
  `useUpdateNotebook`, `useDeleteNotebook`, `useCreateCell`, `useUpdateCell`,
  `useDeleteCell`, `useReorderCells`. Types: `NotebookListItem`, `Notebook`,
  `Cell`, `CellType`, `Engine`.
- `src/routes/explore/Notebooks.tsx` — replace the placeholder with the **list
  page**: grid/list of the caller's notebooks (+ public, badged), New-notebook
  button (name + engine), rename, delete (typed-confirm like DatabasesPage),
  click → open. Empty state.
- `src/routes/explore/NotebookPage.tsx` *(new)* — the **editor**, route
  `notebooks/:id` (add to `App.tsx`). Renders ordered cells:
  - Code/SQL cells → Monaco (`@monaco-editor/react`, already a dep; follow
    `sqllab/SqlEditor.tsx`). Language = `python` for code, `sql` for sql.
  - Markdown cells → an edit textarea with a rendered preview via the already-
    bundled `react-markdown` + `remark-gfm` (see usage in
    `components/chart/dashboard/TextCard.tsx`).
  - Toolbar per cell: change type, move up/down, delete. Notebook header: rename,
    engine select, public toggle.
  - **Autosave**: debounce cell `source` edits (~600ms) → `useUpdateCell`;
    notebook metadata saves on blur/change. No execution affordance in C1 (the
    Run button is added in E1; optionally render it disabled with a "coming
    soon" title to set expectations).

## 5. Testing

- **Backend** (`crates/egret-notebook`, sqlx `#[sqlx::test]` like egret-engine):
  - create → get-with-cells → list (own vs public visibility) → update → delete.
  - ownership: non-owner read of private = 403; non-owner read of public = ok;
    non-owner write = 403; **platform_admin write of another's notebook = 403**
    (asserts the no-admin-override decision).
  - cross-workspace: notebook id from ws A requested under ws B = 404.
  - cell CRUD + reorder; nested IDOR (cell of notebook X under wrong ws/notebook).
  - validation: unknown `engine`/`cell_type` = 400.
- **Frontend** (vitest, mirror `DatabasesPage.test.tsx`): list render + empty
  state, create/rename/delete flows (mocked api), editor renders cells, autosave
  fires `useUpdateCell`, add/reorder/delete cell, markdown preview renders.
- **Local CI gate before merge** (memory `egret-run-full-ci-local-before-merge`):
  full workspace `fmt` + `clippy` + `test`, and frontend `lint`/`typecheck`/
  `test`/`build`. GitHub Actions is out of quota.

## 6. Open questions (resolve during planning)
1. On create, return an empty cell list or seed **one empty code cell**?
   *Recommend: seed one code cell* so the editor isn't blank.
2. Exact role→perm grants for `notebook:read|write` — mirror the role rows used
   for `catalog:read|write` in migration 0024.

## 7. Non-goals / risks
- C1 deliberately ships a notebook you can't run; that's the point of slicing.
  Communicate the disabled Run state in the UI so it doesn't read as broken.
- Keep `egret-notebook` free of any engine/kernel/storage dependency — those
  arrive as separate crates in D1/E1, keeping C1's boundary clean.
