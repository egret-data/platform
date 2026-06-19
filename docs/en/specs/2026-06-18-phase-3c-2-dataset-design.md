# Phase 3c-2 — Dataset (design)

> Status: approved design. Canonical (English). Vietnamese mirror: `docs/vi/specs/2026-06-18-phase-3c-2-dataset-design.md`.

## Goal

Introduce the **Dataset** — a named, governable semantic source backed by a registered
database, carrying a synced snapshot of typed columns. A dataset is the object that
charts (Phase 3c-3) are built on. v1 makes a dataset *usable*: it has a stable column
list with generic types so a chart builder can distinguish dimensions, measures, and
time columns.

## Scope

**In scope (v1):**

- Two dataset kinds under one unified model (mirrors Superset's `SqlaTable`):
  - **Physical** — points at an existing `catalog.schema.table`.
  - **Virtual** — wraps a `SELECT` statement (the natural "promote" target from SQL Lab).
- Column snapshot synced from the source at create time, re-syncable via an explicit
  **Refresh**.
- Per-column `type_generic` classification: `string | numeric | temporal | boolean`,
  plus an `is_temporal` flag — the minimal metadata a chart needs.
- CRUD + Refresh REST API, workspace-scoped, RBAC-gated, owner-or-admin for mutations.
- Frontend: dataset list, create flow (from-table / from-SQL), detail view with columns
  and Refresh, and a **"Save as dataset"** bridge from the SQL Lab editor.

**Deferred (3c-2.2, not in this phase):**

- Metrics (`COUNT(*)`, `SUM(x)`, …).
- Calculated columns (column `expression`).
- `verbose_name` / per-column descriptions / formatting hints.

## Architecture

Extend the existing **`egret-bi`** crate. It already owns the connector-backed
`SchemaService` (physical introspection), the `BiState` wiring (databases, sessions,
workspaces, authz), and the RBAC + owner-gate patterns used by saved queries. A dataset
is the same shape of object as a saved query, so it lives beside it rather than in a new
crate.

The one new piece of lower-layer work lives in **`egret-engine`**: a `describe(sql)`
capability on the connector that returns **typed** columns for an arbitrary `SELECT`.
Today connectors surface column *names* for results but not types for an arbitrary
query; virtual datasets need types.

### New / modified units

| File | Responsibility |
|---|---|
| `crates/egret-bi/src/dataset.rs` (new) | `DatasetRepo` — dataset + column-snapshot CRUD, column reconcile |
| `crates/egret-bi/src/typegen.rs` (new) | Pure `type_generic` mapping (per engine), unit-tested |
| `crates/egret-bi/src/routes.rs` (modify) | Add `dataset_router`; create/list/get/update/refresh/delete handlers |
| `crates/egret-bi/src/model.rs` (modify) | Dataset DTOs + create/update request bodies |
| `crates/egret-bi/src/schema.rs` (reuse) | `columns()` for physical introspection |
| `crates/egret-engine/src/connector.rs` (modify) | Add `describe(sql) -> Vec<DescribedColumn>` to the `Connector` trait |
| `crates/egret-engine/src/trino.rs` (modify) | Implement `describe` via `LIMIT 0` + statement `columns` metadata |
| `crates/egret-engine/src/starrocks.rs` (modify) | Implement `describe` via `LIMIT 0` + result `column_type()` |
| `migrations/0012_dataset.sql` (new) | `datasets` + `dataset_columns` tables + `dataset:read`/`dataset:manage` seed |
| `frontend/src/api/datasets.ts` (new) | react-query hooks |
| `frontend/src/routes/explore/Datasets.tsx` (rewrite stub) | List page |
| `frontend/src/components/dataset/*` (new) | Create flow, detail view, columns table |
| `frontend/src/components/sqllab/SqlTab.tsx` (modify) | "Save as dataset" action |

## Data model

Migration `0012_dataset.sql`:

```sql
CREATE TABLE datasets (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    database_id uuid NOT NULL REFERENCES databases(id) ON DELETE CASCADE,
    name        text NOT NULL,
    description text NOT NULL DEFAULT '',
    -- physical source (all NULL for virtual):
    catalog     text,
    schema_name text,
    table_name  text,
    -- virtual source (NULL for physical):
    sql         text,
    created_by  uuid REFERENCES users(id),
    updated_at  timestamptz NOT NULL DEFAULT now(),
    -- exactly one source kind:
    CONSTRAINT dataset_one_kind CHECK ((table_name IS NOT NULL) <> (sql IS NOT NULL)),
    CONSTRAINT dataset_name_uniq UNIQUE (workspace_id, name)
);

CREATE TABLE dataset_columns (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dataset_id   uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    column_name  text NOT NULL,
    data_type    text NOT NULL,          -- raw engine type, e.g. "varchar(20)"
    type_generic text NOT NULL,          -- string | numeric | temporal | boolean
    is_temporal  boolean NOT NULL DEFAULT false,
    ordinal      int NOT NULL,
    UNIQUE (dataset_id, column_name)
);
CREATE INDEX dataset_columns_dataset_idx ON dataset_columns(dataset_id);

-- RBAC seed (mirrors the query:run grant for ws_editor + ws_admin in 0009_engine.sql)
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'dataset:read'   FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'dataset:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
```

> Note: column/table names (`schema_name`, `table_name`) avoid SQL reserved words. The
> seed follows the `0009_engine.sql` precedent exactly: `role_permissions(role_id,
> permission)` selecting `roles.id` by `scope='workspace'` + `name`.

`kind` is derived, not stored: `sql IS NULL` → physical, else virtual.

## Column introspection

A single `DatasetRepo::sync_columns(spec, source) -> Vec<DescribedColumn>` abstracts both
kinds, exactly like Superset's `external_metadata()`:

- **Physical:** `SchemaService::columns(spec, catalog, schema, table)` (existing DESCRIBE
  path) → `(name, raw_type)`.
- **Virtual:** `connector.describe(sql)` — wraps the SELECT as
  `SELECT * FROM (<sql>) AS _egret_ds LIMIT 0` and reads typed column metadata from the
  engine response:
  - **Trino:** the statement protocol returns a `columns: [{name, type}]` array on the
    first response, present even with zero rows.
  - **StarRocks:** the `mysql_async` result exposes column definitions
    (`column.column_type()`) independent of row count.

`DescribedColumn { name: String, raw_type: String }` is the shared shape. The repo then
maps each through `typegen` and assigns `ordinal` by position.

### `type_generic` mapping (`typegen.rs`)

A pure function `generic_type(engine: Engine, raw_type: &str) -> (GenericType, bool)`
returning the generic class and `is_temporal`. Matching is case-insensitive on the raw
type's leading keyword (strip any `(...)` precision/length suffix):

| Generic | Trino raw types | StarRocks raw types |
|---|---|---|
| `numeric` | tinyint, smallint, integer, int, bigint, real, double, decimal | tinyint, smallint, int, bigint, largeint, float, double, decimal |
| `temporal` | date, time, timestamp, timestamp with time zone | date, datetime |
| `boolean` | boolean | boolean |
| `string` | varchar, char, varbinary, json, uuid, *(fallback)* | char, varchar, string, json, *(fallback)* |

`is_temporal` is `true` iff generic is `temporal`. Unknown types fall back to `string`
(safe default — usable as a dimension). This is the single source of truth charts will
rely on; it is exhaustively unit-tested per engine.

## REST API

Mounted on `BiState` as a `dataset_router`, workspace-scoped under `/api`.

| Method | Path | Gate | Body / returns |
|---|---|---|---|
| POST | `/ws/{ws}/datasets` | `dataset:manage` | `CreateDataset` → `DatasetDetail` |
| GET | `/ws/{ws}/datasets` | `dataset:read` | → `[DatasetSummary]` |
| GET | `/ws/{ws}/datasets/{id}` | `dataset:read` | → `DatasetDetail` (with columns) |
| PUT | `/ws/{ws}/datasets/{id}` | `dataset:manage` + owner-or-admin | `UpdateDataset` → `DatasetDetail` |
| POST | `/ws/{ws}/datasets/{id}/refresh` | `dataset:manage` + owner-or-admin | → `DatasetDetail` |
| DELETE | `/ws/{ws}/datasets/{id}` | `dataset:manage` + owner-or-admin | → `{ ok: true }` |

Request/response shapes:

```rust
// kind is tagged: exactly one of physical / virtual
enum CreateSource {
    Physical { catalog: Option<String>, schema: String, table: String },
    Virtual  { sql: String },
}
struct CreateDataset { name: String, database_id: Uuid, description: String, source: CreateSource }
struct UpdateDataset { name: String, description: String, sql: Option<String> } // sql only meaningful for virtual

struct DatasetColumnDto { column_name: String, data_type: String, type_generic: String, is_temporal: bool, ordinal: i32 }
struct DatasetSummary { id, name, description, database_id, kind: "physical"|"virtual", column_count, updated_at }
struct DatasetDetail  { ..summary, catalog, schema, table, sql, columns: Vec<DatasetColumnDto> }
```

The `database_id` must be granted to the workspace (reuse the existing
`conn_spec`/`list_for_workspace` guard from `routes.rs`).

### Transactions & error handling

- **Create** is introspect-first: build the `ConnSpec`, fetch columns from the source,
  and only then `INSERT` the dataset + columns in a single DB transaction. A bad
  table or invalid SQL surfaces the engine error and creates **nothing**.
- **Refresh** re-introspects, then reconciles `dataset_columns` (insert new, delete
  missing, update changed `data_type`/`type_generic`) in one transaction. If
  introspection fails (source dropped, SQL now invalid), return the error and **keep**
  the existing snapshot untouched.
- **Update** of a virtual dataset's `sql` does **not** auto-refresh columns — the user
  refreshes explicitly (predictable, matches Superset). The response notes columns may be
  stale (frontend shows a "needs refresh" hint when `updated_at` of the dataset is newer
  than the columns — out of scope to persist; v1 just exposes the Refresh button).
- DB secrets are never returned or logged (unchanged invariant).

## Frontend

- **`Datasets.tsx`** — list table: name (link), kind badge, database, column count,
  updated; top-right **Create** button. Empty state with a short explainer + Create.
- **Create flow** (`CreateDatasetDialog`) — two tabs:
  - *From table*: database select → cascading catalog → schema → table (reuse the
    cascading select logic from `DataPanel`), plus name + description.
  - *From SQL*: database select + SQL editor (Monaco) + name + description. Prefilled
    when opened via the SQL Lab bridge.
- **Detail page** (`DatasetDetail`) — header (name, kind, source location/SQL,
  description) + columns table (column_name, data_type, generic-type badge, temporal
  flag) + **Refresh** button (toast on success/failure) + Edit + Delete (owner-or-admin
  only).
- **SQL Lab bridge** — add a **"Save as dataset"** button to the `SqlTab` toolbar (beside
  "Save"); opens the create-from-SQL flow prefilled with the tab's current `sql` and
  `dbId`.
- **`api/datasets.ts`** — react-query hooks: `useDatasets`, `useDataset`,
  `useCreateDataset`, `useUpdateDataset`, `useRefreshDataset`, `useDeleteDataset`. List
  and detail keyed so a refresh invalidates the detail.

## Testing

**Rust**

- `typegen` — exhaustive table-driven unit tests per engine, including precision suffixes
  (`decimal(10,2)` → numeric), `timestamp with time zone` → temporal, and unknown →
  string. Pure, no DB.
- `DatasetRepo` — DB-gated (`#[ignore]`/self-skip when no Postgres): create physical +
  virtual, list, get-with-columns, update, delete cascade, and **reconcile** (add /
  remove / retype a column on refresh).
- `connector.describe` — engine-gated (self-skip when Trino:8090 / StarRocks:9030
  absent): a known `SELECT` returns the expected typed columns on each engine.
- Routes — RBAC denial without `dataset:read`/`dataset:manage`; owner-or-admin gate on
  update/refresh/delete; create rejects an un-granted `database_id`.

**Frontend**

- vitest: create form renders both tabs and validates required fields; list renders rows
  from a mocked `useDatasets`; "Save as dataset" prefills SQL + db.

## Out of scope / non-goals

- No metrics, calculated columns, or verbose names (3c-2.2).
- No dataset-level row/column security beyond the existing Ranger enforcement at query
  time (charts in 3c-3 run through the same engine path).
- No live column fetching — columns are always the stored snapshot.
- No cross-database / join datasets — a dataset belongs to exactly one database.
```
