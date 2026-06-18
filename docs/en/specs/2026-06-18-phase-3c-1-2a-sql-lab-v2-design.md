# Phase 3c-1.2a — SQL Lab v2 (multi-tab + polish) Design Spec

> **Status:** Approved design · **Date:** 2026-06-18 · **Version:** 0.2
> Canonical (English). Vietnamese reference: [`docs/vi/specs/2026-06-18-phase-3c-1-2a-sql-lab-v2-design.md`](../../vi/specs/2026-06-18-phase-3c-1-2a-sql-lab-v2-design.md).
> Refines the SQL Lab from [Phase 3c-1](2026-06-18-phase-3c-1-sql-lab-design.md) after manual testing. Based on feedback + a deeper read of Apache Superset's SQL Lab frontend (§11). Two sibling increments are deliberately split out: **3c-1.2b** (permission-gated streaming export) and a future **Explorer Workbench** (backend-persisted tabs + Home/bookmarks + a generic multi-artifact tab shell) — see §1 / §12.

---

## 1. Goal & scope

Make SQL Lab a proper tool: **multiple SQL tabs** plus the Superset-style polish surfaced in manual testing — cascading data selectors, richer autocomplete, a SQL formatter, a row-limit control, and a real save dialog. Fix the stale-results bug robustly.

**Deliberately kept lightweight (per the scope review):** tabs persist to **localStorage** (restore on reload), NOT the backend. A heavier "Explorer Workbench" — backend-persisted tabs (cross-device), a Home/bookmarks landing, and a generic shell hosting dataset/chart/dashboard tabs — is a **separate future increment**, designed when there is a real second tab type to generalize from. Building that scaffolding now would be premature (the tab state model would be guessed before chart/dashboard requirements exist) and chatty (auto-save per keystroke, multi-tab write conflicts).

**In scope**
- **Frontend (the bulk):**
  - **Multi-tab SQL** — a tab bar (add / close / rename / switch); per-tab state `{ sql, dbId, limit }` persisted to **localStorage** (`sqllab:tabs:{workspace}` + active tab); restore on reload.
  - **DataPanel** (left) — DB select + **cascading Catalog → Schema → Table selects** → a **Columns** panel (name + type, click-to-insert); **collapse/expand**.
  - **Toolbar** — Run · Cancel · **Format** (sql-formatter) · **LIMIT** dropdown · Save.
  - **Autocomplete** — SQL keywords + the loaded catalogs/schemas/tables/columns.
  - **Save dialog** — a shadcn `Dialog` (name + description) replacing `window.prompt`; the saved-query list opens an item into a new tab + delete.
  - **Results bound to `queryId`** — fixes the "query 2 shows query 1's results" bug.
- **Backend (two small changes):**
  - A per-query **row limit** on the engine submit endpoint (`SubmitQuery.limit`).
  - A `description` column on `saved_queries` (for the save dialog).

**Out of scope (deferred)**
- **Explorer Workbench** = backend-persisted tabs (`workbench_tabs`, auto-save, cross-device restore), a **Home/bookmarks** landing, a generic `kind`-based tab shell hosting dataset/chart/dashboard. A future increment once ≥2 tab types exist.
- **Permission-gated streaming "no-limit" export** → Phase 3c-1.2b.
- Multi-statement execution, query parameters/templating, tab drag-reorder polish, schema-tree (replaced by selects).

## 2. Decisions

| # | Decision | Choice | Grounding |
|---|---|---|---|
| 1 | Tab persistence | **localStorage** (per workspace), restore on reload | 90% of the value at 10% of the cost; no backend table/CRUD/auto-save/conflict; backend persistence is the future Workbench |
| 2 | Data selection | **Cascading selects** (Catalog → Schema → Table) + Columns panel; drop the tree | Superset `DatabaseSelector`/`TableSelector`; user request |
| 3 | Row limit | A LIMIT dropdown (10/100/1k/10k/100k); sent per-run; capped at `engine.max_rows` | Superset `QueryLimitSelect`; "no-limit" export is 3c-1.2b |
| 4 | Formatter | Client-side `sql-formatter`, dialect by engine (`trino`/`mysql`) | Lightest; no backend round-trip |
| 5 | Stale results | Results fetched via a react-query key that includes `queryId` | New query → new key → old result can never render |
| 6 | Save | shadcn `Dialog` (name + description); `saved_queries.description` added | Superset `SaveQuery` modal |
| 7 | Route / IA | Stays at `/explore/:ws/sql-lab`; no Home, no rename, no IA churn | "Explorer Workbench" + Home are the future increment |
| 8 | RBAC | All routes gated by `require_workspace("query:run")`; saved-queries owner-scoped (unchanged) | Reuse the engine permission |

## 3. Architecture

```text
crates/egret-bi/src/
  saved.rs   # MODIFY: + description
  model.rs   # MODIFY: + description on SaveQuery/SavedQueryDto
  routes.rs  # MODIFY: description plumbing
crates/egret-engine/src/
  model.rs   # MODIFY: SubmitQuery { …, limit: Option<usize> }
  job.rs     # MODIFY: clamp the per-query limit into Limits.max_rows
migrations/0011_sqllab_v2.sql   # NEW: saved_queries.description

frontend/src/
  routes/explore/SqlLab.tsx          # REWRITE: tab bar + active SqlTab host
  components/sqllab/SqlTab.tsx        # NEW: one SQL tab (DataPanel + editor + toolbar + results)
  components/sqllab/DataPanel.tsx     # NEW: cascading selects + columns + collapse
  components/sqllab/SaveQueryDialog.tsx # NEW: shadcn dialog (name + description)
  components/sqllab/SqlEditor.tsx     # MODIFY: keywords + schema-object completion
  components/sqllab/ResultsGrid.tsx   # keep
  hooks/useTabs.ts                    # NEW: localStorage-backed tab array
  api/sqllab.ts                       # MODIFY: limit param, description, results-by-queryId hook
```

No new crates. Frontend adds `sql-formatter`. No new routes/nav.

## 4. Data model — `migrations/0011_sqllab_v2.sql`

```sql
ALTER TABLE saved_queries ADD COLUMN description text NOT NULL DEFAULT '';
```

That is the entire migration. Tab state lives in the browser (localStorage), not Postgres.

## 5. Backend

### 5.1 `saved_queries.description`
`SaveQuery` DTO gains `#[serde(default)] description: String`; `SavedQueryDto` returns `description`; `SavedQueryRepo::{create,update}` accept + persist it; the `FromRow` struct + `COLS` include `description`.

### 5.2 Engine row limit
`egret_engine::model::SubmitQuery` gains `pub limit: Option<usize>` (`#[serde(default)]`). In the submit path (`JobManager`/the submit handler), the effective cap = `limit.map(|l| l.clamp(1, cfg.max_rows)).unwrap_or(cfg.max_rows)`, used as that query's `Limits.max_rows` (instead of the global). `truncated` still flags a capped result. No new config. Existing callers (sending no `limit`) are unchanged (`None` → global max).

## 6. Frontend

### 6.1 Tabs (`hooks/useTabs.ts`, localStorage)
```ts
type SqlTabState = { id: string; title: string; sql: string; dbId: string | null; limit: number };
```
`useTabs(ws)` manages an array + the active id, persisted to `localStorage["sqllab:tabs:" + ws]` and `localStorage["sqllab:active:" + ws]`. API: `tabs`, `activeId`, `setActive`, `addTab()`, `closeTab(id)` (never below one tab), `renameTab(id, title)`, `patchTab(id, partial)` (updates sql/dbId/limit). Loads on mount (or seeds one `Untitled` tab). Runtime state (`queryId`, current page) is held in memory per tab in the parent (NOT persisted) so switching tabs keeps a running query's id; results re-show from the react-query cache.

### 6.2 SqlLab page (rewrite)
A **tab bar** (using the shadcn `tabs` or a simple custom bar): each tab title; `＋` add, `✕` close, double-click rename. Renders the active **`SqlTab`** (keyed by `id`, with its runtime `queryId` threaded from the parent so it isn't lost on switch). Tab edits call `patchTab`.

### 6.3 SqlTab (`SqlTab.tsx`)
Holds one tab's editor + DataPanel + toolbar + results. Run flow:
- `run()` → clear this tab's `queryId`, `POST /queries { database_id, sql, limit }` → set `queryId` (parent runtime state).
- Status: `useQueryStatus(ws, queryId)` (`refetchInterval` while queued/running).
- **Results bound to `queryId`:** `useResults(ws, queryId, page)` — a react-query keyed by `["sqllab", ws, "results", queryId, page]`. Switching/rerunning changes the key, so stale results never render (Decision 5). This replaces the manual `setResults` effect.
- Cancel → `POST /queries/{id}/cancel`. Failure → show `error`. Truncation banner from status.

### 6.4 DataPanel (`DataPanel.tsx`) — cascading selects + columns + collapse
- **DB select** (granted databases) → resets catalog/schema/table on change.
- **Catalog select** (`useCatalogs`; Trino only — hidden for StarRocks, schemas load directly).
- **Schema select** (`useSchemas`).
- **Table select** (`useTables`).
- **Columns panel** — for the chosen table, `useColumns` lists `name : type`; click a column (or the table) → insert its name into the editor at the cursor.
- **Collapse/expand** — a toggle hides the panel (editor pane expands); collapsed state in client state.
- Loaded catalogs/schemas/tables/columns feed the autocomplete hint (bounded to the active catalog/schema).

### 6.5 Toolbar
- **Run** / **Cancel**.
- **Format** — `sql-formatter` `format(sql, { language })`, `language` from the engine (`trino` → `"trino"`, `starrocks` → `"mysql"`); replaces editor content.
- **LIMIT** dropdown — `10 / 100 / 1,000 / 10,000 / 100,000`; default `1,000`; per-tab; sent on run.
- **Save** — opens `SaveQueryDialog`.

### 6.6 Autocomplete (`SqlEditor.tsx`)
The Monaco completion provider merges a curated **SQL keyword/snippet** list + the DataPanel's loaded **catalogs, schemas, tables, columns** (kinds keyword/struct/field), bounded to the active catalog/schema. Provider disposed on unmount (fixed in 3c-1).

### 6.7 Save dialog + saved/history
- `SaveQueryDialog` (shadcn `Dialog`): **Name** (required) + **Description** (textarea) → `POST /sqllab/queries { database_id, name, sql, description }`. Toast on success.
- **Saved** list: name (+ description tooltip); click → open in a **new tab**; delete (own).
- **History** list: recent runs (`GET /queries`); click → load sql into the current tab.

## 7. Error handling
Reuses `BiApiError` / `EngineApiError`. Frontend run/format/save errors via `sonner` toasts. localStorage tab state is best-effort (a parse failure falls back to one fresh tab).

## 8. Config + wiring
- `egret-bi` `SaveQueryRepo`/routes carry `description` (no new sub-state). `egret-engine` submit accepts `limit`. No `egret-server`/`egret-bin` wiring changes.
- Frontend: add `sql-formatter` (`npm i sql-formatter`). No new routes/nav.

## 9. Testing
- **Backend:** `saved_queries.description` round-trip (DB-gated, extend the saved repo test); engine **limit param** (engine-gated: submit a generated-series `SELECT` with `limit=5` → `row_count==5`, `truncated`).
- **Frontend (Vitest):** `useTabs` add/close/rename/switch + localStorage round-trip; the run-flow with results bound to `queryId` (**run query A then query B → B's results, never A's** — the reported regression); DataPanel cascading selects load on change; Format transforms SQL; autocomplete returns a keyword + a table; `SaveQueryDialog` posts name + description; "open saved query in a new tab".

## 10. Decomposition for the plan
Suggested tasks (~7): (1) `migrations/0011` + `saved_queries.description` (repo/model/routes + DB-gated test); (2) engine `SubmitQuery.limit` + clamp (engine-gated); (3) frontend `api/sqllab.ts` (limit, description, `useResults`-by-queryId) + `useTabs` hook + tests; (4) `DataPanel` (cascading selects + columns + collapse + autocomplete feed); (5) `SqlEditor` autocomplete (keywords + schema objects); (6) `SqlTab` (run/poll, results-by-queryId, cancel) + `SqlLab` tab bar (add/close/rename) + `SaveQueryDialog` + saved/history-open-in-tab + `sql-formatter` toolbar; (7) component tests + final verification.

## 11. References
- **Apache Superset SQL Lab frontend** (validated): `TabbedSqlEditors` (per-tab editor state, add/remove/rename), `DatabaseSelector`/`TableSelector` cascading selects + column metadata, `QueryLimitSelect` (powers-of-ten, sent per-run), `SqlEditorLeftBar` collapse, `SaveQuery` modal (name + description), autocomplete from keywords + schema objects, results bound to the active query's id. **Adopted:** multi-tab, cascading selects, limit-per-run, formatter, results-by-query-id, save modal. **Differed:** tabs in localStorage (Superset persists to `tab_state` server-side — that's our future Workbench, not now).
- **Existing patterns:** `egret-bi`/`egret-engine` (3c-1/3b-1 — `WsContext` + `require_workspace`, `BiApiError`/`EngineApiError`, DB-/engine-gated tests), `frontend/src/api/{client,hooks}.ts`, the shadcn kit (`dialog`, `select`, `tabs`, `resizable`), `react-resizable-panels`, Vitest `__tests__`.

## 12. The future "Explorer Workbench" (recorded, not built here)
When a second tab type exists (after dataset/chart), generalize SQL Lab's tabs into an Explorer Workbench: a generic `kind`-based tab shell, optional backend-persisted tabs (cross-device restore) with debounced auto-save + conflict handling, and a Home/bookmarks landing where users pin artifacts. Designing it then — with real multi-artifact requirements — avoids the premature, rework-prone scaffolding this increment deliberately skips.
