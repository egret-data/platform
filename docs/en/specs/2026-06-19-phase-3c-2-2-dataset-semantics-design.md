# Phase 3c-2.2 — Dataset Semantics (metrics, calculated columns, column metadata)

> Status: approved design. Canonical (English). Vietnamese mirror:
> `docs/vi/specs/2026-06-19-phase-3c-2-2-dataset-semantics-design.md`.

## Goal

Complete the dataset semantic layer so charts (3c-3) have rich measures and dimensions:

- **Metrics** — named aggregate expressions (`SUM(totalprice)`, `COUNT(*)`).
- **Calculated columns** — columns defined by a SQL expression (`price * qty`,
  `EXTRACT(year FROM orderdate)`) instead of a physical source column.
- **Column metadata** — editable `verbose_name` (display label) + `description` on every
  column (physical and calculated).

Validated against Apache Superset: the storage model and the refresh-reconcile behavior
mirror Superset's `TableColumn`/`SqlMetric` and `fetch_metadata`. The one deliberate
divergence is expression validation (see §4).

## Scope

**In scope:**

- `dataset_metrics` table + `dataset_columns` gains `expression` / `verbose_name` /
  `description`.
- **Refresh reconcile** rewritten to upsert physical columns by name (preserving
  user-edited metadata) and never touch calculated columns or metrics.
- Expression validation + type inference via the existing `Connector::describe`.
- REST CRUD for metrics and calculated columns + column-metadata edit.
- Tabbed dataset detail page (Columns / Metrics).

**Out of scope (YAGNI / deferred):**

- Metric `metric_type` / `d3format` / currency formatting (chart-time concern).
- Column `groupby` / `filterable` flags (chart-builder concern).
- Certification, tags, advanced data types.

## Architecture

Extends `egret-bi` (no new crate). The detail page becomes tabbed. The one cross-crate
reuse is `egret_engine`'s `Connector::describe` (added in 3c-2) for expression validation.

### New / modified units

| File | Responsibility |
|---|---|
| `migrations/0013_dataset_semantics.sql` (new) | `ALTER dataset_columns` + `dataset_metrics` table |
| `crates/egret-bi/src/dataset.rs` (modify) | `Metric`/`NewMetric` types; metrics CRUD; calc-column CRUD; **reconcile** (replaces `replace_columns`) |
| `crates/egret-bi/src/expr.rs` (new) | `validate_expression` — single-statement guard + `describe`-probe → `(data_type, type_generic)`; per-engine source-ref builder |
| `crates/egret-bi/src/routes.rs` (modify) | metric + calc-column + column-metadata handlers; detail DTO carries metrics + new column fields |
| `crates/egret-bi/src/model.rs` (modify) | `MetricDto`, request bodies, extend `DatasetColumnDto` + `DatasetDetailDto` |
| `crates/egret-bi/tests/dataset.rs` (modify) | reconcile-preserves-calc test; metrics + calc CRUD |
| `crates/egret-bi/tests/expr.rs` (new) | engine-gated expression validation |
| `frontend/src/api/datasets.ts` (modify) | metric/calc-column hooks + types |
| `frontend/src/routes/explore/DatasetDetail.tsx` (modify) | Columns / Metrics tabs |
| `frontend/src/components/dataset/MetricsTab.tsx` (new) | metrics table + add/edit/delete |
| `frontend/src/components/dataset/ColumnsTab.tsx` (new) | columns table + metadata edit + add calculated |

## Data model

Migration `0013_dataset_semantics.sql`:

```sql
ALTER TABLE dataset_columns
  ADD COLUMN expression   text,                       -- NULL = physical; set = calculated
  ADD COLUMN verbose_name text NOT NULL DEFAULT '',
  ADD COLUMN description  text NOT NULL DEFAULT '';

CREATE TABLE dataset_metrics (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    dataset_id   uuid NOT NULL REFERENCES datasets(id) ON DELETE CASCADE,
    metric_name  text NOT NULL,
    expression   text NOT NULL,
    data_type    text NOT NULL,
    type_generic text NOT NULL,
    verbose_name text NOT NULL DEFAULT '',
    description  text NOT NULL DEFAULT '',
    ordinal      int  NOT NULL,
    UNIQUE (dataset_id, metric_name)
);
CREATE INDEX dataset_metrics_dataset_idx ON dataset_metrics (dataset_id);
```

`is_calculated` is derived (`expression IS NOT NULL`), not stored. Physical and calculated
columns share `dataset_columns` (Superset's unified model).

## Refresh reconcile (the central change)

The 3c-2 `replace_columns` did DELETE-all + re-insert, which would wipe calculated columns
and user metadata. It is replaced by `reconcile_physical_columns(dataset_id, fresh)`,
mirroring Superset's `fetch_metadata` (diff by `column_name`):

For each freshly-introspected physical column (`fresh`):
- **Upsert by `(dataset_id, column_name)`**: if the row exists, update
  `data_type`, `type_generic`, `is_temporal`, `ordinal`; **preserve** `verbose_name`,
  `description`. If new, insert with empty metadata.

After upserting:
- **Delete** physical columns (`expression IS NULL`) whose `column_name` is no longer in
  `fresh`.
- **Never touch** calculated columns (`expression IS NOT NULL`) or any `dataset_metrics`.

Calculated columns are re-ordinaled to follow physical columns. A calculated column whose
underlying physical column disappears is left in place (it will error at query time — same
as Superset); v1 does not proactively invalidate it.

This runs in one transaction, exactly where `replace_columns` was called by `ds_refresh`.

## Expression validation (`expr.rs`)

`validate_expression(spec, source, expr) -> Result<DescribedExpr, BiError>` where
`DescribedExpr { data_type, type_generic }`:

1. **Guard** — reject if `expr` is empty or contains a statement terminator that would make
   it multi-statement (a `;` outside string literals). This is the only safety gate; the
   acting user already holds `dataset:manage` and can run arbitrary SQL in SQL Lab, and
   Ranger enforces access at query time.
2. **Probe** — build `SELECT <expr> AS _egret_expr FROM <source>` where `source` is:
   - physical: the quoted `catalog.schema.table` (reuse the engine-appropriate identifier
     quoting already in `schema.rs`),
   - virtual: `(<dataset.sql>)` wrapped as a subquery with an alias.

   Call `build_connector(spec).describe(<probe sql>, 15s)`. The wrapping inside `describe`
   yields `SELECT * FROM (SELECT <expr> AS _egret_expr FROM <source>) AS _egret_ds LIMIT 0`,
   which validates the expression and returns its result column with a type.
3. **Result** — take the single returned column's type as `data_type`, map it through
   `typegen::generic_type(spec.engine, &data_type)` for `type_generic`. An engine error
   (invalid expression) propagates as `BiError::Engine` → HTTP 502 with the engine message.

Metrics and calculated columns use the same path (a metric's aggregate expression is valid
in `SELECT <expr> FROM <source>` with no `GROUP BY`).

## REST API

All under the existing `dataset_router`, gated `dataset:manage` + owner-or-admin, using the
existing `conn_spec_perm` to resolve the engine + verify the database grant.

| Method | Path | Body → returns |
|---|---|---|
| POST | `/ws/{ws}/datasets/{id}/metrics` | `{ metric_name, expression, verbose_name?, description? }` → `DatasetDetail` |
| PUT | `/ws/{ws}/datasets/{id}/metrics/{metric_id}` | same body → `DatasetDetail` |
| DELETE | `/ws/{ws}/datasets/{id}/metrics/{metric_id}` | → `DatasetDetail` |
| POST | `/ws/{ws}/datasets/{id}/columns` | `{ column_name, expression, verbose_name?, description? }` (calculated) → `DatasetDetail` |
| PUT | `/ws/{ws}/datasets/{id}/columns/{column_name}` | `{ verbose_name?, description?, expression? }` → `DatasetDetail` |
| DELETE | `/ws/{ws}/datasets/{id}/columns/{column_name}` | → `DatasetDetail` (calculated only; deleting a physical column returns 400) |

Behavior:
- **Add metric / add calculated column / edit a calculated column's expression**: validate
  the expression (§4) first; persist only on success (introspect-first). `data_type` +
  `type_generic` come from the probe.
- **Edit a physical column**: only `verbose_name` / `description` may change; `expression`
  is rejected for a physical column (400).
- **Delete a column**: allowed only when `expression IS NOT NULL` (calculated). Deleting a
  physical column is a 400 — physical columns are managed by refresh.
- Every mutation returns the full refreshed `DatasetDetail` so the UI re-renders from one
  response.

`DatasetDetailDto` gains `metrics: Vec<MetricDto>`; each `DatasetColumnDto` gains
`expression: Option<String>`, `verbose_name: String`, `description: String`, and a derived
`is_calculated: bool`.

## Frontend

`DatasetDetail.tsx` keeps its header (name, kind, source, Refresh/Edit/Delete) and gains a
tab strip below it:

- **Columns tab** (`ColumnsTab.tsx`): table of columns — name, data_type, generic-type
  badge, temporal flag, a **calculated** badge for expression-backed columns, and
  `verbose_name`/`description`. Inline edit of `verbose_name`/`description` for any column;
  edit `expression` for calculated columns. **+ Calculated column** opens a small form
  (column_name + expression + optional label/description); on submit it calls the API,
  which validates server-side — a bad expression surfaces as a toast with the engine error.
  Delete only on calculated columns.
- **Metrics tab** (`MetricsTab.tsx`): table of metrics — name, expression, generic-type
  badge, label. **+ Metric** / edit / delete, same validate-on-submit flow.

`api/datasets.ts` gains: `DatasetMetric` type; `DatasetColumn` gains
`expression`/`verbose_name`/`description`/`is_calculated`; hooks `useAddMetric`,
`useUpdateMetric`, `useDeleteMetric`, `useAddColumn`, `useUpdateColumn`, `useDeleteColumn`
— each invalidating the dataset detail query.

## Error handling

- Invalid expression → engine error surfaced as HTTP 502 with the engine message; nothing
  persisted (introspect-first).
- Duplicate `metric_name` / `column_name` → unique-violation mapped to a clear 4xx (not a
  bare 500) — fix the 3c-2 gap where dataset-name conflicts returned 500.
- Editing/deleting a physical column's structure → 400 with a clear message.
- DB secrets never returned or logged (unchanged).

## Testing

**Rust**

- `expr` unit: the single-statement guard (rejects `a; b`, accepts `SUM(x)` /
  `price*qty`); the per-engine source-ref builder (physical quoting, virtual subquery
  wrap). Pure, no engine.
- `expr` engine-gated (self-skip without Trino): a valid expression returns an inferred
  type; an invalid expression returns `BiError::Engine`.
- `DatasetRepo` DB-gated:
  - **reconcile**: seed a dataset with physical columns + a calculated column + edited
    `verbose_name`; run reconcile with a changed source (one physical column dropped, one
    retyped, one added); assert physical columns upserted, the dropped one removed, the
    **calculated column and the edited `verbose_name` preserved**. (Critical regression for
    the 3c-2 clobber.)
  - metrics CRUD + ordinal; calc-column CRUD; cascade delete with the dataset.
- Routes: RBAC denial without `dataset:manage`; owner-or-admin gate; physical-column delete
  rejected; duplicate metric/column conflict mapped to 4xx.

**Frontend**

- vitest: Columns and Metrics tabs render; the add-metric and add-calculated-column forms
  render and validate required fields; a calculated column shows its badge.

## Migration / compatibility

`0013` is additive (new nullable/defaulted columns + a new table); existing datasets keep
working (no metrics, no calculated columns, empty metadata). The `ds_refresh` handler
swaps `replace_columns` → reconcile with no API change.
