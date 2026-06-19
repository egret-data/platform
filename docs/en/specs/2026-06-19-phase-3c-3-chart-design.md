# Phase 3c-3 — Chart (Table + Pivot Table)

> Status: approved design. Canonical (English). Vietnamese mirror:
> `docs/vi/specs/2026-06-19-phase-3c-3-chart-design.md`.

## Goal

Let users build, save, and view **charts** over a dataset, rendered as a **Table** or a
**Pivot Table**. A chart consumes the dataset semantic layer built in 3c-2/3c-2.2:

- **Metrics** → measures (aggregate expressions in `SELECT`).
- **Columns / calculated columns** → dimensions (`GROUP BY` or raw `SELECT`).

This phase deliberately ships **tabular** visualizations only. ECharts visualizations
(bar/line/pie/big-number), time-grain bucketing, and richer formatting are deferred to
**3c-3.2**, reusing the exact same query pipeline.

Validated against **Apache Metabase** (table/pivot UX — the gold standard) and **Apache
Superset** (the `QueryObject → SQL` model). Key adopted decisions and their sources are
called out inline.

## Scope

**In scope:**

- `charts` table + `ChartRepo` CRUD (migration `0014_chart.sql`).
- A pure, unit-tested **query builder** (`chartquery.rs`): `QuerySpec → SQL`, supporting
  aggregate vs raw-records modes, `WHERE` filters, `ORDER BY`, `LIMIT`, and `ROLLUP`
  (for pivot subtotals/grand-totals).
- A type-aware **SQL literal encoder** for filter values (security-critical — connectors
  take raw SQL strings, not bind parameters).
- REST: chart CRUD + a `charts/data` endpoint that builds SQL and submits to the existing
  `JobManager` (reuse async execution + polling).
- Frontend: an **Explore** page (controls + live preview), a **Table** renderer
  (`@tanstack/react-table`), a **Pivot Table** renderer (hand-rolled cross-tab over
  `ROLLUP` rows), and a **Charts list** (replaces the `Charts.tsx` placeholder).

**Out of scope (→ 3c-3.2 unless noted):**

- ECharts visualizations (bar/line/pie/big-number).
- Time-grain bucketing (`date_trunc` day/week/month).
- Metric filters (`HAVING`) — only column filters (`WHERE`) in this phase.
- Conditional/heatmap formatting, column drag-reorder, freeze, mini-bars, drill-through.
- Raw `WHERE` / `HAVING` SQL extras (injection risk — intentionally never supported).
- Advanced filter ops (`ILIKE`, `BETWEEN`, relative-time), series-limit / "top-N + Other".

**No new crate** — extends `egret-bi` (like 3c-2 / 3c-2.2).

## Background: what we adopt from Metabase and Superset

**Superset (`superset/models/helpers.py`, `superset/common/query_object.py`):**

- `need_groupby = metrics present OR groupby` selects **aggregate** vs **raw-records**
  mode (Superset line ~3074). We use `need_groupby = !metrics.is_empty()`.
- **Column filters → `WHERE`; metric filters → `HAVING`** (else `WHERE SUM(x)=5` is
  invalid SQL). MVP supports column filters only; metric filters deferred.
- **`GROUP BY` uses the full column/expression, not the `SELECT` alias** — required so
  calculated-column dimensions group correctly on Trino/StarRocks.
- Pivoting is **post-query** in Superset (pandas `pivot_table` with `margins`). We instead
  push totals into SQL via `ROLLUP` (see §4) so every aggregate (incl. `AVG`/`MIN`/`MAX`)
  is engine-correct.
- Filter operator starter set: `= != > >= < <= LIKE IN NOT IN IS NULL IS NOT NULL`.

**Metabase (`frontend/src/metabase/visualizations/`):**

- Its table is built on **TanStack Table** + virtualization, giving click-to-sort, column
  resize, visibility, and pagination cheaply. We adopt `@tanstack/react-table` (shadcn
  data-table pattern) for the same feel.
- "Feels nice" MVP table features: **sort, resize, column visibility, number/date
  formatting**. Heavier features (heatmap rules, drag-reorder, freeze, mini-bars,
  drill-through) are what make Metabase deep — all deferred.
- Pivot differentiators are **collapsible row groups + subtotals/grand-totals**. Metabase
  computes totals via backend powerset subqueries; we use a single `ROLLUP` query instead.

## Architecture

```
Frontend Explore page
  │  build QuerySpec (metrics, dimensions, columns, filters, orderby, row_limit, rollup)
  ▼
POST /api/ws/{ws}/charts/data  { dataset_id, query_spec }
  │
  ├─ load dataset (source + columns + metrics)         [DatasetRepo]
  ├─ validate names in QuerySpec against the dataset    [chartquery]
  ├─ build SQL                                          [chartquery::build]
  └─ return { sql, database_id }                        [egret-bi]
  ▼
Frontend submits sql → POST /api/ws/{ws}/queries → JobManager   (existing endpoint)
  ▼
Frontend polls GET /api/ws/{ws}/queries/{id}/status + /results  (existing endpoints)
  ▼
Render: Table (tanstack) | Pivot (cross-tab over ROLLUP rows)
```

Chart CRUD (`charts` table) is independent of execution: a saved chart stores `viz_type`
+ `params` (the `QuerySpec` plus pivot row/column split). To view a saved chart, the
frontend posts its `query_spec` to `charts/data` exactly like the live preview, then submits
the returned SQL to the existing `/queries` flow.

`charts/data` returns SQL rather than submitting itself so `BiState` stays decoupled from
`JobManager` (no heavy fixture in the BI http test), and the generated SQL becomes a visible
"View SQL" affordance. Execution still runs through `JobManager` + polling (the chosen model);
only the submit is client-initiated, like SQL Lab. The viewer roles (`ws_editor`/`ws_admin`)
already hold `query:run`, so polling works; fully decoupling chart-view from `query:run` via
chart-owned result endpoints is a 3c-3.2 refinement.

## Components

### 1. Migration — `migrations/0014_chart.sql`

```sql
CREATE TABLE charts (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id uuid NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    dataset_id   uuid NOT NULL REFERENCES datasets(id)   ON DELETE CASCADE,
    name         text NOT NULL,
    description  text NOT NULL DEFAULT '',
    viz_type     text NOT NULL,            -- 'table' | 'pivot_table'
    params       jsonb NOT NULL,           -- QuerySpec + viz layout (pivot rows/cols split)
    created_by   uuid REFERENCES users(id),
    updated_at   timestamptz NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, name)
);
CREATE INDEX charts_workspace_idx ON charts (workspace_id);
CREATE INDEX charts_dataset_idx   ON charts (dataset_id);

-- RBAC seed (mirrors dataset:* in 0012): chart:read / chart:manage for ws_editor, ws_admin.
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'chart:read'   FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
INSERT INTO role_permissions (role_id, permission)
SELECT id, 'chart:manage' FROM roles WHERE scope = 'workspace' AND name IN ('ws_editor','ws_admin');
```

`params` is one JSONB column (the project's "JSONB-then-normalize" convention from the
catalog/dataset work). It holds the `QuerySpec` and the pivot layout (which dimensions are
rows vs columns, and the chosen pivot metric ordering).

### 2. Query builder — `crates/egret-bi/src/chartquery.rs`

Pure functions, no I/O, exhaustively unit-tested. The single most important module.

```rust
pub struct QuerySpec {
    pub metrics:    Vec<String>,    // metric names → resolved to aggregate expressions
    pub dimensions: Vec<String>,    // column names → GROUP BY (calc cols use their expression)
    pub columns:    Vec<String>,    // raw-records mode (used only when metrics is empty)
    pub filters:    Vec<Filter>,    // column filters → WHERE
    pub orderby:    Vec<OrderBy>,   // { field, desc }
    pub row_limit:  i64,
    pub rollup:     bool,           // pivot → GROUP BY ROLLUP(...) + GROUPING() markers
}

pub struct Filter   { pub col: String, pub op: FilterOp, pub value: FilterValue }
pub struct OrderBy  { pub field: String, pub desc: bool }

pub enum FilterOp { Eq, Ne, Gt, Ge, Lt, Le, Like, In, NotIn, IsNull, IsNotNull }

pub enum FilterValue { One(String), Many(Vec<String>), None }  // None for IS [NOT] NULL
```

**Build algorithm** — `build(dataset, columns, metrics, spec) -> Result<String, BiError>`:

1. **Resolve + validate names.** Every `metrics[i]` must be a known metric; every
   `dimensions[i]` / `columns[i]` / filter `col` / orderby `field` must be a known column
   or metric name. Unknown → `BiError::BadRequest` (400). Metric → its `expression`;
   column → its physical name, or its `expression` if calculated. **GROUP BY / SELECT use
   the resolved expression, never an alias.**
2. **Mode.** `aggregate = !spec.metrics.is_empty()`.
   - **Aggregate:** `SELECT <dim-exprs aliased>, <metric-exprs aliased> [, GROUPING(dim)…]
     FROM <src> [WHERE …] GROUP BY <dim-exprs | ROLLUP(<dim-exprs>)> [ORDER BY …]
     LIMIT n`.
   - **Raw records:** `SELECT <column-exprs aliased> FROM <src> [WHERE …] [ORDER BY …]
     LIMIT n` (metrics ignored).
3. **FROM** via existing `expr.rs::source_ref(spec, source)` (physical
   `catalog.schema.table` / virtual `(<sql>) AS _egret_src`).
4. **WHERE** from `filters` (column filters only this phase); each rendered with
   `render_filter` + the literal encoder (§3).
5. **ROLLUP** when `spec.rollup`: emit `GROUP BY ROLLUP(<dim-exprs>)` and add a
   `GROUPING(<dim-expr>) AS __grp_<i>` column per dimension so the frontend can identify
   subtotal/grand-total rows (a grouping marker of `1` means that dimension is rolled up).
6. **ORDER BY** by resolved expression; **LIMIT** = `min(spec.row_limit, MAX_CHART_ROWS)`.

Aliases for output columns are the metric/column **name** (quoted), so the frontend maps
result columns back to dataset names deterministically.

### 3. SQL literal encoder — security-critical

Connectors execute **raw SQL strings** (Trino's REST statement path has no bind params),
so filter **values** are the one untrusted input and MUST be encoded safely:

```rust
fn sql_literal(value: &str, type_generic: &str) -> Result<String, BiError> {
    match type_generic {
        "numeric"  => parse-validate as a number, emit unquoted; reject otherwise,
        "boolean"  => normalize to TRUE/FALSE; reject otherwise,
        _          => single-quote and escape: every ' becomes '' ; wrap in '…',
    }
}
```

- Identifiers and expressions come from the dataset (already validated by `expr.rs` in
  3c-2.2) → trusted; only filter values flow through `sql_literal`.
- `IN` / `NOT IN` encode each element and join with `, `.
- The operator is chosen from the `FilterOp` enum (never user free-text) → no operator
  injection.
- `IS NULL` / `IS NOT NULL` take no value.
- No raw `WHERE`/`HAVING` passthrough is ever accepted.

This module is the **primary code-review focus** of the phase.

### 4. ROLLUP correctness (pivot totals)

Pivot subtotals/grand-totals are produced by the **engine**, not recomputed client-side,
so they are correct for every aggregate (`SUM`, `COUNT`, **and** `AVG`/`MIN`/`MAX`).
Single query, e.g. rows=`[region]`, cols=`[segment]`, metric=`SUM(sales)`:

```sql
SELECT region, segment, SUM(sales) AS "revenue",
       GROUPING(region) AS __grp_0, GROUPING(segment) AS __grp_1
FROM   ...
GROUP BY ROLLUP(region, segment)
```

- `__grp_0 = 0, __grp_1 = 0` → a leaf cell.
- `__grp_1 = 1` → subtotal across `segment` for a fixed `region`.
- `__grp_0 = 1, __grp_1 = 1` → grand total.

Both engines support this: **Trino** `GROUP BY ROLLUP(...)` + `GROUPING(col)`; **StarRocks**
`GROUP BY ROLLUP(...)` + `grouping_id()/grouping()`. The function name difference is
handled per-engine in the builder. The manual Trino integration test is extended to cover
the ROLLUP/GROUPING path (engine-contract-sensitive; unit tests can't catch dialect bugs).

### 5. Repository — `ChartRepo` (in `egret-bi`)

`Chart` row + CRUD: `create`, `get`, `list_by_workspace`, `update`, `delete`,
`chart_workspace(chart_id)` (for the ownership guard). Unique-name violation →
`BiError::Conflict` (409), mirroring `DatasetRepo`. Mutations are owner-or-admin.

### 6. REST routes — `chart_router` (in `egret-bi/src/routes.rs`)

Mounted under the workspace scope alongside datasets/sqllab:

| Method & path                              | Permission       | Notes                          |
|--------------------------------------------|------------------|--------------------------------|
| `GET    /api/ws/{ws}/charts`               | `chart:read`     | list                           |
| `POST   /api/ws/{ws}/charts`               | `chart:manage`   | create                         |
| `GET    /api/ws/{ws}/charts/{id}`          | `chart:read`     | detail                         |
| `PUT    /api/ws/{ws}/charts/{id}`          | `chart:manage`   | update (owner-or-admin)        |
| `DELETE /api/ws/{ws}/charts/{id}`          | `chart:manage`   | delete (owner-or-admin)        |
| `POST   /api/ws/{ws}/charts/data`          | `chart:read`     | build SQL (returns sql)        |

- **Nested-ownership guard:** every `/charts/{id}` handler verifies the chart belongs to
  the path workspace before acting (404 on mismatch) — per the IDOR lesson from 3c-2.2.
- `charts/data` takes `{ dataset_id, query_spec }`, verifies the dataset belongs to the path
  workspace, builds the SQL, and returns `{ sql, database_id }`. The frontend submits that to
  the existing `POST /api/ws/{ws}/queries` endpoint and polls `/queries/{id}`.

### 7. Frontend

- **API client** `frontend/src/api/charts.ts`: `Chart`, `QuerySpec` types; `useCharts(ws)`,
  `useChart(ws,id)`, `useChartMutations(ws)`, and `useBuildChartData(ws, body) → { sql,
  database_id }`, then reuse the existing `useSubmitQuery`/`useQueryStatus`/`useResults` hooks.
- **Explore page** `routes/explore/ChartExplore.tsx`: left rail lists the dataset's metrics
  and columns; a Table/Pivot toggle; a controls panel (dimensions, metrics, column filters,
  sort, row limit; pivot adds a rows-vs-columns split); a **Run** button driving the live
  preview; a **Save** dialog.
- **Table** `components/chart/ChartTable.tsx`: `@tanstack/react-table` — sort, resize,
  column visibility, and number/date formatting keyed off each column's `type_generic`.
- **Pivot** `components/chart/PivotTable.tsx`: a pure `buildPivot(rows, rowDims, colDims,
  metric)` transform that reads the `__grp_*` markers to position subtotal/grand-total
  rows, plus a grid renderer.
- **Charts list** `routes/explore/Charts.tsx` (replaces placeholder): list / create / open /
  delete saved charts.

## Error handling

- Unknown metric/column/filter name in a `QuerySpec` → 400 (`BiError::BadRequest`).
- Invalid filter value for its column type (e.g. non-numeric for a numeric column) → 400.
- Duplicate chart name → 409 (`BiError::Conflict`).
- Chart not in path workspace → 404.
- Engine/SQL failure surfaces through the normal job status (`failed` + message), exactly
  like SQL Lab — the chart layer adds no new execution error path.

## Testing

- **`chartquery` unit tests** (pure, no DB): aggregate SQL, raw-records SQL, ROLLUP SQL +
  grouping markers, every filter operator, `sql_literal` escaping (quote-injection
  attempt stays quoted; numeric/boolean validation), unknown-name rejection, calc-column
  dimension uses its expression in `GROUP BY`.
- **`ChartRepo` tests** (DB-gated, self-skip without `DATABASE_URL`): CRUD + unique-name
  conflict + `chart_workspace` lookup.
- **Engine smoke** (gated, self-skip without `EGRET_TEST_TRINO_URL`): build a Table chart
  and a Pivot (ROLLUP) chart on a seeded dataset, run via `JobManager`, assert rows +
  presence of subtotal/grand-total rows.
- **Frontend tests**: `buildPivot` cross-tab transform (incl. subtotal placement from
  markers) and the Table render.
- **Manual Trino integration test** extended for the ROLLUP/GROUPING path.

## Security checklist

- Filter values pass through `sql_literal` only; identifiers/expressions are dataset-sourced
  and pre-validated.
- Operators come from an enum, never free text.
- No raw `WHERE`/`HAVING` passthrough.
- RBAC on every route; owner-or-admin on mutations; nested-ownership guard on `/charts/{id}`
  and a workspace check on `charts/data`'s `dataset_id` (a dataset from another workspace 404s).
- `row_limit` clamped to `MAX_CHART_ROWS`; execution inherits the engine timeout + row cap.

## Deferred → 3c-3.2 and beyond

- ECharts viz types (bar/line/pie/big-number) reusing this pipeline.
- Time-grain bucketing (per-engine `date_trunc`).
- Metric filters (`HAVING`).
- Heatmap/conditional formatting, column drag-reorder, freeze, mini-bars, drill-through.
- Advanced filter ops, series-limit ("top-N + Other"), pivot collapse/expand interactions.
