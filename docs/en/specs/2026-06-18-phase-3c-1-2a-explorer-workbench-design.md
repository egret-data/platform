# Phase 3c-1.2a — Explorer Workbench + SQL Lab v2 Design Spec

> **Status:** Approved design · **Date:** 2026-06-18 · **Version:** 0.1
> Canonical (English). Vietnamese reference: [`docs/vi/specs/2026-06-18-phase-3c-1-2a-explorer-workbench-design.md`](../../vi/specs/2026-06-18-phase-3c-1-2a-explorer-workbench-design.md).
> Refines the SQL Lab from [Phase 3c-1](2026-06-18-phase-3c-1-sql-lab-design.md) after manual testing. Based on feedback + a deeper read of Apache Superset's SQL Lab frontend (see §12). Sibling increment **3c-1.2b** adds permission-gated streaming export (deferred here).

---

## 1. Goal & scope

Rebuild SQL Lab as a **multi-tab Explorer Workbench** with backend-persisted tabs, and apply Superset-style UX polish (cascading data selectors, richer autocomplete, SQL formatter, row-limit control, a proper save dialog). Add a **Home** landing surface (shell only) where users will later pin/bookmark items.

The Workbench shell is **generic** (each tab has a `kind`), but only `kind = "sql"` is implemented now; dataset/chart/dashboard tab types plug in during 3c-2/3/4.

**In scope**
- **Backend:** a `workbench_tabs` table (per workspace+user) + CRUD with debounced auto-save; a per-query **row limit** parameter on the engine submit endpoint; a `description` column on `saved_queries`.
- **Frontend:**
  - **Home** page (`/explore/:ws`) — shell/empty-state only ("your bookmarks will appear here") + nav.
  - **Workbench** (`/explore/:ws/workbench`) — tab bar (add/close/rename), per-tab state persisted to the backend (debounced), restore on open.
  - **SQL tab** — the editor + run/poll/results, with results keyed by `queryId` (fixes the stale-results bug robustly).
  - **DataPanel** (left) — DB select + **cascading Catalog → Schema → Table selects** → a **Columns** panel (name + type, click-to-insert); **collapse/expand**.
  - **Toolbar** — Run · Cancel · **Format** (sql-formatter) · **LIMIT** dropdown · Save.
  - **Autocomplete** — SQL keywords + the loaded catalogs/schemas/tables/columns.
  - **Save dialog** — a shadcn `Dialog` (name + description) replacing `window.prompt`; saved-query list loads into a new tab + delete.

**Out of scope (deferred)**
- **Bookmark/pin CRUD** — the Home page is a shell only this increment; saving/opening pinned items is a later increment.
- **Permission-gated streaming export / "no-limit" export** → Phase 3c-1.2b (`query:export` permission + per-user grant + streaming connector path).
- **dataset/chart/dashboard tab types** → 3c-2/3/4 (the shell is built generic to host them).
- Tab session features beyond persist+restore (e.g. tab history "go back", drag-reorder polish), multi-statement execution, query templating/parameters, schema-tree (replaced by selects).

## 2. Decisions

| # | Decision | Choice | Grounding |
|---|---|---|---|
| 1 | Tab persistence | **Backend-persisted** `workbench_tabs` (per workspace+user) + debounced auto-save + restore | User chose cross-device; mirrors Superset `tab_state` |
| 2 | Workbench shell | Generic `kind` column; only `sql` implemented now | Future dataset/chart/dashboard tabs in one workbench |
| 3 | Data selection | **Cascading selects** (Catalog → Schema → Table) + Columns panel; drop the tree | Superset `DatabaseSelector`/`TableSelector`; user request |
| 4 | Row limit | A LIMIT dropdown (10/100/1k/10k/100k); sent per-run to the engine, capped at `engine.max_rows` | Superset `QueryLimitSelect`; "no-limit" is 3c-1.2b export |
| 5 | Formatter | Client-side `sql-formatter`, dialect by engine (`trino`/`mysql`) | Lightest; no backend round-trip |
| 6 | Stale results | Results fetched via a react-query key that includes `queryId` | A new query → new key → old result can never show |
| 7 | Home/Bookmarks | A landing **shell** now; bookmark CRUD deferred | User wants the entry surface; content later |
| 8 | RBAC | All Workbench/SQL routes gated by `require_workspace("query:run")`; tabs + saved-queries owner-scoped | Reuse the engine's existing permission |

## 3. Architecture

```text
crates/egret-bi/src/
  workbench.rs   # NEW: WorkbenchTabRepo (CRUD over workbench_tabs)
  saved.rs       # MODIFY: + description
  model.rs       # MODIFY: WorkbenchTab DTOs, + description on SaveQuery/SavedQueryDto
  routes.rs      # MODIFY: workbench tab routes; description plumbing
crates/egret-engine/src/
  model.rs       # MODIFY: SubmitQuery { …, limit: Option<usize> }
  routes.rs/job.rs  # MODIFY: thread the per-query limit into Limits
migrations/0011_workbench.sql   # NEW: workbench_tabs + saved_queries.description

frontend/src/
  routes/explore/Home.tsx          # NEW: bookmarks shell
  routes/explore/Workbench.tsx      # NEW: tab bar + active tab host (replaces SqlLab page)
  components/sqllab/SqlTab.tsx       # NEW: one SQL tab (editor + run + results)
  components/sqllab/DataPanel.tsx    # NEW: cascading selects + columns + collapse
  components/sqllab/SaveQueryDialog.tsx  # NEW: shadcn dialog
  components/sqllab/SqlEditor.tsx    # MODIFY: keywords + schema-object completion
  components/sqllab/ResultsGrid.tsx  # keep
  api/workbench.ts                   # NEW: tab + limit hooks
  api/sqllab.ts                      # MODIFY: limit param, description, results-by-queryId
  App.tsx + sidebar nav              # MODIFY: Home + Workbench routes/nav
```

`egret-bi` keeps its existing deps. No new crates. Frontend adds `sql-formatter`.

## 4. Data model — `migrations/0011_workbench.sql`

```sql
CREATE TABLE workbench_tabs (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id      uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  kind         text NOT NULL DEFAULT 'sql' CHECK (kind IN ('sql')),
  title        text NOT NULL DEFAULT 'Untitled',
  state        jsonb NOT NULL DEFAULT '{}',   -- { sql, database_id, limit, catalog?, schema? }
  position     int  NOT NULL DEFAULT 0,
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX workbench_tabs_owner_idx ON workbench_tabs (workspace_id, user_id, position);

ALTER TABLE saved_queries ADD COLUMN description text NOT NULL DEFAULT '';
```

The active tab is tracked client-side (localStorage `sqllab:active:{ws}`); only order + content are persisted. `kind` CHECK widens in later phases.

## 5. Backend — workbench tabs + limit

### 5.1 `WorkbenchTabRepo` (`egret-bi/src/workbench.rs`)
`list(ws, user)` (ordered by position), `create(ws, user, kind, title, state) -> Tab`, `update(id, title?, state?, position?) -> Tab`, `delete(id)`, `owner(id) -> Option<Uuid>`. Owner-scoped: routes verify `tab.user_id == caller` (or `platform_admin`).

### 5.2 Routes (`egret-bi`, under `/api/ws/{ws}/workbench`, gated `require_workspace("query:run")`)
| Method + path | Purpose |
|---|---|
| `GET /workbench/tabs` | List the caller's tabs (ordered). |
| `POST /workbench/tabs` `{kind, title, state}` | Create → returns the tab (with id). |
| `PUT /workbench/tabs/{id}` `{title?, state?, position?}` | Auto-save (debounced client-side). Owner-only. |
| `DELETE /workbench/tabs/{id}` | Close a tab. Owner-only. |

Saved-query routes gain `description` (in `SaveQuery` + `SavedQueryDto`).

### 5.3 Engine row limit
`egret_engine::model::SubmitQuery` gains `pub limit: Option<usize>` (serde `#[serde(default)]`). In `JobManager::submit`, the effective cap = `limit.map(|l| l.clamp(1, cfg_max_rows)).unwrap_or(cfg_max_rows)` and is used as `Limits.max_rows` for that query (instead of the global). `truncated` still flags when the result hit the cap. No new config.

## 6. Frontend

### 6.1 IA / routing
- `/explore/:ws` → **Home** (shell). The Explorer landing redirect (`HomeRedirect`) targets the workspace Home.
- `/explore/:ws/workbench` → **Workbench**. `/explore/:ws/sql-lab` → redirect to `workbench` (back-compat).
- Sidebar nav: **Home**, **Workbench** (was "SQL Lab"), then the existing Datasets/Charts/Dashboards placeholders.

### 6.2 Home (shell)
A simple page: a heading + empty-state card ("Your bookmarks will appear here") + a primary button "Open Workbench". No data fetching. Bookmark CRUD is deferred.

### 6.3 Workbench shell (`Workbench.tsx`)
- Loads tabs via `useWorkbenchTabs(ws)`; if none, creates one default SQL tab.
- A **tab bar**: each tab shows its title; `＋` adds a tab (`POST`), `✕` closes (`DELETE`), double-click renames (`PUT title`). Active tab in client state (+ localStorage).
- Renders the active tab's content by `kind` (only `SqlTab` now).
- **Auto-save:** each tab's state changes (sql, dbId, limit) are debounced (~800ms) → `PUT /workbench/tabs/{id}`. A `useDebouncedSave` hook.

### 6.4 SQL tab (`SqlTab.tsx`)
Per-tab state: `{ sql, dbId, limit, queryId }`. The editor + toolbar + DataPanel + results. The run flow:
- `run()` → clear `queryId`, `POST /queries {database_id, sql, limit}` → set `queryId`.
- Status polled by `useQueryStatus(ws, queryId)` (`refetchInterval` while queued/running).
- **Results bound to queryId:** `useResults(ws, queryId, page)` is a react-query whose key includes `queryId` — switching/rerunning changes the key, so stale results never render (Decision 6). Replaces the manual `setResults` effect.
- Cancel → `POST /queries/{id}/cancel`. Failure → show `error`. Truncation banner from status.

### 6.5 DataPanel (`DataPanel.tsx`) — cascading selects + columns + collapse
- **DB select** (granted databases) → on change, reset catalog/schema/table.
- **Catalog select** (`useCatalogs`; Trino only — for StarRocks this select is hidden, schemas load directly).
- **Schema select** (`useSchemas`).
- **Table select** (`useTables`).
- **Columns panel** — when a table is chosen, `useColumns` lists `name : type`; click a column (or the table) → insert its name into the editor at the cursor.
- **Collapse/expand** — a toggle hides the panel (the editor pane expands); collapsed state in client state.
- As schemas/tables/columns load, they feed the autocomplete hint (Decision: bounded to the active catalog/schema).

### 6.6 Toolbar
- **Run** / **Cancel** (disabled appropriately).
- **Format** — `sql-formatter` `format(sql, { language })` where `language` maps from the DB engine (`trino` → `"trino"`, `starrocks` → `"mysql"`); replaces editor content.
- **LIMIT** dropdown — `10 / 100 / 1,000 / 10,000 / 100,000`; default `1,000`; stored per-tab; sent on run.
- **Save** — opens `SaveQueryDialog`.

### 6.7 Autocomplete (`SqlEditor.tsx`)
The Monaco completion provider merges: a curated **SQL keyword/snippet** list + the DataPanel's loaded **catalogs, schemas, tables, columns**. Items typed as keyword/struct/field kinds. Bounded to the active catalog/schema's objects (avoid loading the whole DB). The provider is disposed on unmount (already fixed in 3c-1).

### 6.8 Save dialog + saved/history
- `SaveQueryDialog` (shadcn `Dialog`): **Name** (required) + **Description** (textarea) → `POST /sqllab/queries {database_id, name, sql, description}`. Toast on success.
- **Saved** list: each item shows name (+ description tooltip); click → open in a **new workbench tab** (create a tab with that sql/db); delete (own).
- **History** list: recent runs (`GET /queries`); click → load sql into the current tab.

## 7. Error handling
Reuses `BiApiError` / `EngineApiError`. Tab routes: 401 unauth, 403 non-member-or-non-owner (PUT/DELETE on another user's tab), 404 missing. Auto-save failures are non-fatal (toast a subtle warning; the client keeps the local edit). The frontend run/format/save errors surface via `sonner` toasts.

## 8. Config + wiring
- **`egret-server`:** the workbench routes ship inside the existing `BiState` mount (no new sub-state). 
- **`egret-bin`:** `BiState` gains `tabs: WorkbenchTabRepo` (built from `db`).
- **Frontend:** add `sql-formatter` (`npm i sql-formatter`). New routes in `App.tsx`; new nav entries.

## 9. Testing
- **Backend:** `WorkbenchTabRepo` CRUD + ordering (DB-gated); tab routes auth/ownership (HTTP, DB-gated — 401, and a non-owner PUT → 403); `saved_queries.description` round-trip; engine **limit param** (engine-gated: submit `SELECT` over a generated series with `limit=5` → `row_count==5`, `truncated`).
- **Frontend (Vitest):** tab bar add/switch/close/rename (mocked API); the run-flow per tab with results bound to `queryId` (run query A then B → B's results, never A's — the regression for the reported bug); DataPanel cascading selects load on change; Format button transforms SQL; autocomplete provider returns a keyword + a table; SaveQueryDialog posts name+description.

## 10. Decomposition for the plan
Suggested tasks (~10): (1) `migrations/0011` + `WorkbenchTabRepo` + `saved_queries.description` (DB-gated); (2) workbench tab routes + ownership + `description` plumbing + `BiState`/bin wiring (HTTP tests); (3) engine `SubmitQuery.limit` + JobManager clamp (engine-gated); (4) frontend `api/workbench.ts` + `api/sqllab.ts` updates (limit, description, results-by-queryId) + hooks tests; (5) `Home` shell + routing/nav (Home + Workbench, sql-lab redirect); (6) `Workbench` shell + tab bar + debounced auto-save; (7) `SqlTab` (run/poll, results bound to queryId, cancel); (8) `DataPanel` cascading selects + columns + collapse + autocomplete feed; (9) toolbar Format (`sql-formatter`) + LIMIT + `SaveQueryDialog` + saved/history-open-in-tab; (10) component tests + final verification.

## 11. References
- **Apache Superset SQL Lab frontend** (validated): `TabbedSqlEditors` (per-tab `queryEditor` state, `tabHistory`, add/remove/rename), `tab_state` backend persistence; `DatabaseSelector`/`TableSelector` cascading selects + column metadata; `QueryLimitSelect` (powers-of-ten up to server max, sent per-run); `SqlEditorLeftBar` collapse (`hideLeftBar` + splitter); `SaveQuery` modal (name + description); autocomplete from `useKeywords` + schema objects; results bound to the active query's `latestQueryId` (the stale-results fix). **Adopted:** backend-persisted tabs, cascading selects, limit-per-run, formatter, results-by-query-id, save modal. **Deferred:** streaming export (3c-1.2b), bookmark/pin Home content.
- **Existing patterns:** `egret-bi`/`egret-engine` (Phase 3c-1/3b-1 — `WsContext` + `require_workspace`, `BiApiError`/`EngineApiError`, DB-/engine-gated tests), `frontend/src/api/{client,hooks}.ts`, the shadcn UI kit (`dialog`, `select`, `tabs`, `resizable`), `react-resizable-panels`, Vitest `__tests__` (Phase 0b).
