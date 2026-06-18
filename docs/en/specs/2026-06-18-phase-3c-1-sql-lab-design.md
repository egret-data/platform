# Phase 3c-1 — SQL Lab Design Spec

> **Status:** Approved design · **Date:** 2026-06-18 · **Version:** 0.1
> Canonical (English). Vietnamese reference: [`docs/vi/specs/2026-06-18-phase-3c-1-sql-lab-design.md`](../../vi/specs/2026-06-18-phase-3c-1-sql-lab-design.md).
> First slice of [Master Architecture](../architecture/2026-06-17-egret-architecture-design.md) §10.5 (`egret-bi`). Consumes the query engine from [Phase 3b-1](2026-06-18-phase-3b-1-query-engine-design.md). Validated against Apache Superset (SQL Lab) and Metabase (native editor) — see §13.

---

## 1. Goal & scope

A working **SQL Lab** in the Explorer SPA: pick a workspace's granted database → browse its schema → write SQL (Monaco, schema-aware autocomplete) → run (async via the existing engine, REST-polled) → see paginated results → export CSV → save/load named queries → see run history.

This is **3c-1 of `egret-bi`**. The remaining BI subsystems — **dataset (3c-2), chart (3c-3), dashboard (3c-4)** — are separate specs that build on this. SQL Lab is the natural first slice: it only needs the query engine, nothing depends on it yet.

**In scope**
- New backend crate **`egret-bi`**: saved-query CRUD (`saved_queries`) + **schema introspection** (list catalogs/schemas/tables/columns of a granted database by running `SHOW`/`DESCRIBE` synchronously through the engine connectors).
- A **server-side CSV export** endpoint added to **`egret-engine`** (`GET …/queries/{id}/results.csv`) — streams the stored JSONL result as CSV (bounded memory). [Decision ①]
- **Frontend SQL Lab** (`frontend/src/routes/explore/SqlLab.tsx` + components): Monaco editor with schema-aware autocomplete, run/cancel via the engine API with REST polling, paginated results grid, CSV download, lazy schema tree (cached + manual refresh), saved-queries + history tabs.
- Run/poll/results/cancel/history stay on the **existing** engine API (`/api/ws/{ws}/queries…`).

**Out of scope (deferred)**
- Dataset / chart / dashboard → 3c-2/3/4.
- WebSocket live status → Phase 3b-2 (the UI uses REST polling).
- Metabase-style **schema sync** into Egret's own metadata tables — overkill; live-introspection-with-caching is the MVP call (Superset does exactly this). Adopt sync only when data-modeling/governance needs it.
- Editor session restore (Superset's `tab_state`), multi-tab editor, SQL snippets, per-table autocomplete permission filtering.
- Schema-tree virtualization (react-arborist) — a perf follow-up; lazy loading covers MVP.
- A dedicated prefix-search autocomplete endpoint — autocomplete v1 feeds from the cached schema of the active catalog/schema.

## 2. Decisions

| # | Decision | Choice | Grounding |
|---|---|---|---|
| 1 | First BI slice | SQL Lab (3c-1); dataset/chart/dashboard later | Standalone — only needs the engine |
| 2 | Backend shape | `egret-bi`: saved queries + schema introspection; run/poll/results reuse the engine | A real SQL Lab without duplicating the engine |
| 3 | Schema source | **Live introspection** via engine `SHOW`/`DESCRIBE` connectors, lazy per level | Superset does exactly this; the Iceberg catalog covers only Iceberg tables |
| 4 | Status delivery | **REST polling** (`refetchInterval`) on the existing engine endpoints | Superset's exact model; WS hub deferred to 3b-2 |
| 5 | Autocomplete | Schema-aware, fed from the cached schema of the active catalog/schema | Superset/Metabase feed completion from schema metadata |
| ① | CSV export | **Server-side** streaming endpoint in `egret-engine` (JSONL → CSV) | Superset + Metabase both export server-side; client-side risks browser OOM at 100k rows |
| ② | Schema caching | Frontend react-query cache (`staleTime ≈ 5 min`) + a **Refresh schema** button | Superset = RTK-Query cache + `forceRefresh` |
| ③ | Autocomplete scope | Bounded to the active catalog/schema's cached tables/columns | Avoids loading a whole DB; prefix-search endpoint is a later scale step |
| 4b | RBAC | Reuse `query:run` to gate all of SQL Lab; saved-query edit/delete owner-or-`platform_admin` | If you can run queries you can browse schema + save; mirrors the engine's ownership rule |

## 3. Architecture

```text
crates/
├── egret-bi/                      # NEW
│   └── src/
│       ├── lib.rs
│       ├── model.rs               # SavedQuery, SchemaNode DTOs
│       ├── error.rs               # BiError + BiApiError (IntoResponse)
│       ├── saved.rs               # SavedQueryRepo (CRUD over saved_queries)
│       ├── schema.rs              # SchemaService: per-engine SHOW/DESCRIBE via egret-engine connectors
│       └── routes.rs              # BiState + /api/ws/{ws}/sqllab routers
└── egret-engine/                  # MODIFIED: add the CSV export route
    └── src/routes.rs              # GET /api/ws/{ws}/queries/{id}/results.csv

frontend/src/
├── routes/explore/SqlLab.tsx      # the page (replaces the placeholder)
├── components/sqllab/             # NEW: SchemaTree, ResultsGrid, SavedQueries, History, editor wrapper
└── api/sqllab.ts (+ hooks)        # API client + react-query hooks
```

**Backend deps:** `egret-bi` → `egret-{core,config,metastore,auth,workspace,database,engine}`. It uses `egret_engine::connector::{build_connector, ConnSpec}` + `model::Limits` to run introspection queries synchronously, and `egret_database::DatabaseRepo` to resolve + grant-check the database. `egret-server` mounts `egret-bi`'s router; `egret-bin` builds `BiState`.

**Frontend deps:** add `@monaco-editor/react` (Monaco). `@tanstack/react-query` is already present; the shadcn kit (`table`, `tabs`, `resizable`, `select`, `scroll-area`, `sonner`, `button`, `dialog`) is already present.

## 4. Backend — `egret-bi`

### 4.1 Saved queries (`saved.rs`)
`SavedQueryRepo` over `saved_queries`: `create(ws, db_id, name, sql, user)`, `list(ws)`, `get(id)`, `update(id, name, sql, db_id)`, `delete(id)`. Edit/delete enforce owner-or-`platform_admin` at the route layer (like the engine's query ownership). The list returns all saved queries in the workspace with their owner (a shared workspace asset).

### 4.2 Schema introspection (`schema.rs`)
`SchemaService` resolves a database via `DatabaseRepo` (with grant check), opens its secret, builds a connector (`egret_engine::connector::build_connector(&ConnSpec)`), and runs **small, synchronous** meta-queries with a tight `Limits` (e.g. `max_rows = 10_000`, `timeout = 15s`) — these are interactive and fast, so they **bypass the async `JobManager`**. Per-engine logic (`SchemaIntrospector`):

| Level | Trino | StarRocks |
|---|---|---|
| catalogs | `SHOW CATALOGS` | *(none — returns empty; FE skips to schemas)* |
| schemas | `SHOW SCHEMAS FROM "{catalog}"` | `SHOW DATABASES` |
| tables | `SHOW TABLES FROM "{catalog}"."{schema}"` | `SHOW TABLES FROM `{schema}` ` |
| columns | `DESCRIBE "{catalog}"."{schema}"."{table}"` → (name, type) | `DESCRIBE `{schema}`.`{table}` ` → (Field, Type) |

Identifiers are quoted per engine to resist injection via schema/table names. Each call returns a flat list (lazy — one level per request). The frontend caches and assembles the tree.

## 5. Data model — `migrations/0010_sqllab.sql`

```sql
CREATE TABLE saved_queries (
  id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  database_id  uuid REFERENCES databases(id) ON DELETE SET NULL,
  name         text NOT NULL,
  sql          text NOT NULL,
  created_by   uuid REFERENCES users(id),
  created_at   timestamptz NOT NULL DEFAULT now(),
  updated_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX saved_queries_ws_idx ON saved_queries (workspace_id, updated_at DESC);
```

No new RBAC permission — SQL Lab reuses `query:run` (seeded in `0009`).

## 6. CSV export (Decision ①) — `egret-engine` addition

A new route on the engine (it owns query results): `GET /api/ws/{workspace}/queries/{id}/results.csv`, gated by the same `require_workspace("query:run")` + the engine's existing `owned()` ownership check. It loads the query's `result_ref` JSONL object from the object store (bounded by `max_rows`), writes the header row from `columns_jsonb`, then one CSV line per JSONL row, and returns it as `text/csv` with `Content-Disposition: attachment; filename="query-<id>.csv"`. CSV-escaping (quotes, commas, newlines) per RFC 4180. Memory is bounded by `max_rows`; true S3-range streaming is a later optimization (same note as results pagination in 3b-1). This replaces a client-side export, which would risk browser OOM at 100k rows (the reason Superset and Metabase both export server-side).

## 7. REST surface

**SQL Lab (`egret-bi`, under `/api/ws/{workspace}/sqllab`)** — every route requires `require_workspace("query:run")` + the database granted to the workspace (for the `databases/{id}/…` routes).

| Method + path | Purpose |
|---|---|
| `POST /sqllab/queries` `{database_id, name, sql}` | Save a named query. |
| `GET /sqllab/queries` | List this workspace's saved queries (with owner). |
| `GET /sqllab/queries/{id}` | Load one. |
| `PUT /sqllab/queries/{id}` `{name, sql, database_id}` | Update (owner or `platform_admin`). |
| `DELETE /sqllab/queries/{id}` | Delete (owner or `platform_admin`). |
| `GET /sqllab/databases/{id}/catalogs` | Trino catalogs (StarRocks → empty list). |
| `GET /sqllab/databases/{id}/schemas?catalog=` | Schemas / databases. |
| `GET /sqllab/databases/{id}/tables?catalog=&schema=` | Tables. |
| `GET /sqllab/databases/{id}/columns?catalog=&schema=&table=` | `[{name, type}]`. |

**Engine (existing + the one CSV addition):** `POST /queries`, `GET /queries/{id}`, `GET /queries/{id}/results`, `POST /queries/{id}/cancel`, `GET /queries`, and **new** `GET /queries/{id}/results.csv`.

## 8. Frontend — SQL Lab

### 8.1 Layout (resizable panes; shadcn kit already present)
```
┌──────────────┬───────────────────────────────────────────┐
│ DB picker    │  Monaco SQL editor       [Run][Cancel][Save]│
│ schema tree  ├───────────────────────────────────────────┤
│ (lazy,cached;│  [Results] [History] [Saved]               │
│  Refresh ⟳;  │  results grid (paginated) · truncated? ·   │
│  click→insert)│  [Download CSV] · error panel             │
└──────────────┴───────────────────────────────────────────┘
```

### 8.2 Run state machine (react-query)
`POST /queries` → `{id}` → poll `GET /queries/{id}` with `refetchInterval: 1000` until `status ∈ {finished, failed, cancelled}` → on `finished`, `GET /queries/{id}/results?page=`. `running` → spinner + enabled Cancel (`POST /queries/{id}/cancel`). `failed` → show `error`. Truncation banner when `truncated`.

### 8.3 Schema tree + caching (Decision ②)
Lazy react-query per level (`catalogs` → `schemas` → `tables` → `columns`), `staleTime ≈ 5 min` so repeated expansion doesn't re-hit the engine. A **Refresh schema** button invalidates the DB's schema queries (Superset's `forceRefresh`). Click a table → insert its qualified name into the editor at the cursor.

### 8.4 Autocomplete (Decisions 5, ③)
A Monaco `registerCompletionItemProvider` for SQL: built-in keywords/snippets + **table/column suggestions from the cached schema of the active catalog/schema** (bounded — not the whole DB). Columns for a table are pulled (and cached) when that table is referenced/expanded. A prefix-search endpoint is a later scale step.

### 8.5 Results grid + CSV
Paginated read-only `table` (page via the engine results endpoint); page-size selector; truncation banner. **Download CSV** is an `<a href>` to the engine's `…/results.csv` endpoint (server-side; the browser just downloads the file).

### 8.6 Saved / History tabs
- **Saved:** `GET /sqllab/queries` list; click to load into the editor; delete (own). Save dialog (`name`) → `POST /sqllab/queries`.
- **History:** `GET /api/ws/{ws}/queries` (engine); click a row to reload its SQL into the editor.

Single editor; multi-tab + session restore are deferred.

## 9. Config + wiring

- **`egret-server`:** `AppState.bi: Option<egret_bi::BiState>`; mount the `sqllab` router under `/api/ws` behind the session + CSRF stack. `AppState::without_db()` → `bi: None`.
- **`egret-bin`:** build `BiState { saved: SavedQueryRepo, schema: SchemaService, databases: DatabaseRepo, sessions, workspaces, authz }` from config + DB. `SchemaService` holds a small `Limits` (10k rows / 15s).
- The engine CSV route ships inside the existing `EngineState` mount (no new wiring).
- **Frontend:** add `@monaco-editor/react`; new route already exists (`SqlLab.tsx` placeholder) under `/explore`.

## 10. Error handling

`BiError`: `NotFound`, `Forbidden`, `BadRequest`, `Engine(String)` (introspection failure surfaced from the connector), `Db(sqlx::Error)`. `BiApiError` (`IntoResponse`, `DbApiError`-style) maps authz/validation to HTTP; internal DB errors → 500. Schema-introspection failures (bad DB, unreachable engine) return `502/400` with the engine message (non-secret). The engine CSV route reuses the engine's `EngineApiError`.

## 11. Security

- SQL Lab is gated by `require_workspace("query:run")` (ws_editor/ws_admin) + the database granted to the workspace. The engine + Ranger still govern actual data visibility at query time.
- Saved-query edit/delete are owner-or-`platform_admin`. The CSV route reuses the engine's `owned()` check (a member can't export another member's result).
- Schema introspection quotes identifiers per engine; the DB secret travels only inside connector opts/headers, never into a response, log, `saved_queries.sql`, or the CSV.

## 12. Testing

- **`egret-bi` unit:** DTO serde; per-engine `SHOW`/`DESCRIBE` → `SchemaNode` parsing (table-driven, no DB).
- **DB-gated:** `SavedQueryRepo` CRUD + ownership (create/list/get/update/delete; cross-user edit forbidden).
- **Engine-gated** (live Trino :8090 + StarRocks :9030): `SchemaService` lists catalogs/schemas/tables/columns for a real database (create an Iceberg table via the catalog or use a system schema); assert a known table/column appears.
- **HTTP** (oneshot, DB-gated): unauth → 401; non-member → 403/404; saved-query CRUD happy path; the engine `results.csv` route returns `text/csv` + correct header row (DB-gated, using a finished query row).
- **Frontend (Vitest, matching `frontend/src/**/__tests__`):** the run state machine (submit → poll → results, mocked client); schema-tree lazy load + Refresh invalidation; autocomplete provider returns table/column items from a mocked schema; saved-query load/delete; CSV link points at the right URL.

## 13. References

- **Architecture:** [Master design](../architecture/2026-06-17-egret-architecture-design.md) §10.5 (`egret-bi` — SQL Lab), §9.6 (`saved_queries`), §10.2 (engine — consumed here).
- **Apache Superset SQL Lab** (validated, the closest reference — also an async-job + REST-polling BI): live schema introspection per request (`/api/v1/database/{id}/{schemas,tables,table_metadata}`) with **RTK-Query frontend caching + `forceRefresh`** (no metadata sync); autocomplete fed from the same schema data; `saved_query` model; **server-side CSV/streaming export** (threshold ~1000 rows → streaming endpoint); run-flow = POST execute → **poll** results by key; 3-pane layout (left schema browser, center editor, results), virtualized tree (react-arborist). **Adopted:** live-introspection + FE cache + refresh (②), polling (④), saved_queries model, server-side CSV (①), layout. **Deferred:** `tab_state` session restore, tree virtualization.
- **Metabase native editor** (validated, contrasting model): **syncs** schema into its own `metabase_table`/`metabase_field` tables (Quartz, hourly) → instant autocomplete from synced metadata (endpoint capped at 50, 60s cache); results **stream in one request** (`/api/dataset/:format`), no job/poll. **Adopted:** the autocomplete-cap idea (③, as a later prefix-search endpoint), server-side export. **Deliberately NOT adopted:** schema sync infra (overkill for MVP — only needed for data-modeling/governance); the single-request streaming model does not transfer to our async-job engine (a fundamental difference — our cancellable job + polling is the right fit, which Superset confirms).
- **Existing patterns:** `egret-database`/`egret-engine` (Phase 3a/3b-1 — `Option<_>` sub-state, DB-/engine-gated tests, `DbApiError`/`EngineApiError`, `WsContext` + `require_workspace`, the connector API), `frontend/src/api/{client,hooks}.ts` + the shadcn UI kit + Vitest `__tests__` (Phase 0b).

## 14. Decomposition for the plan

Suggested tasks (~9): (1) scaffold `egret-bi` crate + frontend `@monaco-editor/react` dep; (2) `model` + `BiError`/`BiApiError`; (3) `migrations/0010_sqllab.sql` + `SavedQueryRepo` (DB-gated); (4) `SchemaService` per-engine introspection (engine-gated); (5) `egret-bi` routes + RBAC + ownership + HTTP tests + wiring (`egret-server`/`egret-bin`); (6) `egret-engine` `results.csv` route + test; (7) frontend API hooks (`sqllab.ts`) + run state machine; (8) frontend SQL Lab page — editor + run/cancel + results grid + CSV + schema tree (cached + refresh) + autocomplete; (9) frontend saved/history tabs + component tests + final verification.
