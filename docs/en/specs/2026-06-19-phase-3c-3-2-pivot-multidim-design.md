# Phase 3c-3.2 — Multi-dimensional Pivot Table (Metabase-grade)

> Status: approved design. Canonical (English). Vietnamese mirror:
> `docs/vi/specs/2026-06-19-phase-3c-3-2-pivot-multidim-design.md`.
> Extends Phase 3c-3 (`docs/en/specs/2026-06-19-phase-3c-3-chart-design.md`) on the same
> branch `feat/phase-3c-3-chart`.

## Goal

Replace the single-dimension pivot shipped in 3c-3 with a **Metabase-grade multi-level
pivot**: N row dimensions × N column dimensions × N measures, with nested row/column
groups, subtotals at every level, a grand total, collapsible row groups, and multiple
measures laid out as the innermost column level.

Validated against **Apache Metabase**'s pivot (`metabase/pivot/core.cljc` +
`PivotTable/`): we reuse its data model (row tree / column tree → flattened header items
with `depth`/`offset`/`span`; subtotal + grand-total nodes; multi-measure as the innermost
top-header level; client-side collapse). We diverge on the **query strategy**: Metabase runs
N subqueries (a powerset of breakouts) and marks rows with a bitmask; we use **one
engine-native query** with two ROLLUPs.

## Background — what 3c-3 had, what changes

3c-3's pivot used a single `GROUP BY ROLLUP(rowDim, colDim)` and a flat `buildPivot`,
correct only for one row dim + one col dim. This phase generalizes both the SQL builder and
the frontend transform to arbitrary depth. The backend builder, the `ChartParams` pivot
shape, the frontend `buildPivot` + `PivotTable`, and the explore pivot controls all change.
RBAC, `sql_literal`, name-validation, `MAX_CHART_ROWS`, and the table viz are untouched.

## 1. SQL strategy — ROLLUP × ROLLUP in one query

A pivot over row dims `R = [r1..rm]` and col dims `C = [c1..cn]` with measures `M` emits:

```sql
SELECT <r1..rm exprs>, <c1..cn exprs>, <M exprs>,
       GROUPING(r1) AS __grp_0, …, GROUPING(rm) AS __grp_{m-1},
       GROUPING(c1) AS __grp_{m}, …, GROUPING(cn) AS __grp_{m+n-1}
FROM   <source>
[WHERE …]
GROUP BY ROLLUP(<r1..rm exprs>), ROLLUP(<c1..cn exprs>)
LIMIT n
```

The cross-product of the two ROLLUPs yields exactly `(m+1)·(n+1)` grouping sets in **one
query**:

- both prefixes full → **leaf cells**,
- row prefix shortened, col full → **row subtotals**,
- row full, col prefix shortened → **column subtotals**,
- both shortened → **cross-subtotals**, and `(0,0)` → **grand total**.

Because ROLLUP produces *prefixes*, the active dimensions in any row are always a leading
prefix; a row's `GROUPING()` markers therefore reduce to two integers: the **active
row-prefix length `i`** (0..m) and **active col-prefix length `j`** (0..n), which fully
classify the row. `GROUPING(dim) = 1` means that dim is rolled up (inactive) in that row.

Both **Trino** and **StarRocks** accept multiple grouping elements in one `GROUP BY`
(`GROUP BY ROLLUP(a,b), ROLLUP(c,d)`) and the `GROUPING()` scalar — standard SQL. The
engine-gated test and the manual Trino integration test verify the dialect (the marker
function and ROLLUP syntax are engine-contract-sensitive).

If `C` is empty, emit a single `ROLLUP(R)`; if `R` is empty, a single `ROLLUP(C)`.

## 2. Backend — `egret-bi`

### 2.1 `chartquery` — generalize rollup

Replace the `rollup: bool` field on `QuerySpec` with:

```rust
pub struct QuerySpec {
    pub metrics:    Vec<String>,
    pub dimensions: Vec<String>,        // SELECT'd dims, in order: rows then cols (for pivot)
    pub columns:    Vec<String>,
    pub filters:    Vec<Filter>,
    pub orderby:    Vec<OrderBy>,
    pub row_limit:  i64,
    pub rollup_groups: Vec<Vec<String>>, // each inner Vec = dim names to ROLLUP as one group
}
```

`build()` aggregate mode:

- SELECT each `dimensions[i]` resolved expression `AS "<name>"`, then each metric
  `AS "<name>"`.
- **GROUP BY:** if `rollup_groups` is empty → `GROUP BY <all dim exprs>` (plain, the table
  case). Otherwise → `GROUP BY ROLLUP(<group0 exprs>), ROLLUP(<group1 exprs>), …`. Every
  name in every group must also appear in `dimensions` (validated; unknown → 400).
- **GROUPING markers:** when `rollup_groups` is non-empty, append
  `GROUPING(<expr>) AS __grp_<i>` for every `dimensions[i]` (index = position in
  `dimensions`), so the frontend maps marker `i` → dimension `i`.
- Pivot callers pass `dimensions = rows ++ cols` and `rollup_groups = [rows, cols]` (drop an
  empty group). Markers `0..m-1` are row dims, `m..m+n-1` are col dims.

The single-dim ROLLUP test from 3c-3 is rewritten for the new field; the aggregate/raw/
filter/`sql_literal` tests are unchanged.

### 2.2 No other backend changes

`charts/data`, `ChartRepo`, routes, RBAC, and the `params` JSONB column are unchanged
(`params` already stores arbitrary JSON). The pivot layout lives entirely in `params`.

## 3. Frontend — pivot engine (`buildPivot` v2)

`src/components/chart/pivot/engine.ts` — pure, exhaustively unit-tested. Port of Metabase's
`multiLevelPivot`. Input: the flat result `{ columns, rows }` + `{ rowDims, colDims,
metrics, showRowTotals, showColTotals, condense, collapsed }`. Output a `Pivot`:

```ts
interface HeaderItem {
  path: (string | null)[];   // raw dim values root→node
  value: string;             // display value
  depth: number;             // 0 = outermost
  span: number;              // leaf rows/cols covered
  offset: number;            // position in the flattened axis
  hasChildren: boolean;
  isSubtotal: boolean;       // a "Totals for X" node
  isGrandTotal: boolean;
  isCollapsed: boolean;
  isMetric?: boolean;        // innermost top-header level when >1 metric
  metric?: string;
}
interface Pivot {
  leftHeaderItems: HeaderItem[];   // flattened row tree
  topHeaderItems: HeaderItem[];    // flattened col tree (+ metric level)
  rowDimNames: string[];
  colDimNames: string[];
  // cell value: (rowLeafPath, colLeafPath, metric) → formatted value (+ isSubtotal flags)
  cell: (rowPath: (string|null)[], colPath: (string|null)[], metric: string) => Cell;
}
```

Algorithm (mirrors the Metabase report):

1. **Index** the marker columns (`__grp_*`) and dim/metric value columns.
2. For each flat row compute `(i, j)` = active row-/col-prefix lengths from markers.
3. **Build the row tree** from `(i=m)` rows (full row prefix) over their row-dim values;
   the **col tree** from `(j=n)` rows. Nodes carry `value`, `children`, a `value→childPos`
   map.
4. **Insert subtotal nodes** (when `showRowTotals`/`showColTotals`) under each non-leaf
   group; suppress when `condense` and the group has a single child. **Append grand total.**
5. **Flatten** each tree depth-first into `HeaderItem[]` computing `depth`, `offset`,
   `span`, `hasChildren`.
6. **Values map:** key leaf cells by `rowPath ++ colPath ++ metric`; key subtotal cells by
   the corresponding partial-prefix grouping rows (the `(i<m)` / `(j<n)` rows). `cell()`
   looks up the right map by the requested paths’ lengths.
7. **Multi-measure:** when `metrics.length > 1`, add a final metric level to the top header
   (one leaf per metric under each column leaf). With one metric, no extra level.
8. **Collapse:** `collapsed` is a set of serialized row-group paths; a collapsed node hides
   its descendant leaves and renders only its subtotal. Pure client state.

### 4. Frontend — renderer (`PivotTable` v2)

`src/components/chart/pivot/PivotTable.tsx` — an HTML `<table>` driven by the flattened
header items (no virtualization this phase):

- **Top headers:** one `<tr>` per column-dim level (+ a metric row when >1 metric); each
  cell `colSpan = item.span`; a trailing "Row totals" group when `showRowTotals`.
- **Left headers:** the leaf body rows carry the row-dim cells using `rowSpan = item.span`
  for grouping; subtotal rows span the inner row-dim columns and are styled distinctly; a
  trailing "Grand total" row when `showColTotals`. A collapse caret on groups with children
  toggles `collapsed`.
- **Body cells:** `cell(rowLeafPath, colLeafPath, metric)`, right-aligned + locale-formatted
  for numeric metrics (string-encoded numerics coerced, as in `ChartTable`).

### 5. Explore controls

`ChartExplore` pivot controls become three **ordered multi-selects** (Metabase
Rows/Columns/Values): **Rows** (ordered row dims), **Columns** (ordered col dims),
**Values** (ordered metrics), plus checkboxes **Row totals**, **Column totals**, **Condense
duplicate totals**. Implemented as add/remove ordered chip lists over the dataset's columns
and metrics (a column may appear in Rows or Columns, not both; a calc column is a valid
dim). `specForRun` builds `dimensions = rows ++ cols`, `rollup_groups = [rows, cols]`
(dropping empties), `metrics = values`. Guard: a pivot needs ≥1 row dim and ≥1 metric.

`ChartParams` pivot shape:

```ts
{ spec: QuerySpec, rows: string[], cols: string[], metrics: string[],
  showRowTotals: boolean, showColTotals: boolean, condense: boolean }
```

(Collapse state is session-local, not persisted this phase.)

## 6. Scope

**In:** N×N×N row/col/metric pivots; nested multi-level headers; subtotals at every level +
grand total with `showRowTotals`/`showColTotals`/`condense` toggles; collapsible row groups;
multi-measure as innermost top-header level; numeric formatting.

**Out (→ 3c-3.3):** virtualized rendering for very large pivots; column resize; cell
conditional-formatting/heatmap; drill-through on cells; persisted collapse state; pivot
CSV/XLSX export; column subtotals collapse (only row groups collapse this phase).

## 7. Error handling

- Pivot with no row dim or no metric → blocked client-side (toast) and, defensively, the
  builder still produces valid SQL.
- Unknown dim/metric name in `rollup_groups`/`dimensions` → 400 (existing validation).
- A degenerate result (no `__grp_*` columns) renders as a flat single-group table — no crash.

## 8. Testing

- **`chartquery` unit tests:** `rollup_groups = [[r1,r2],[c1]]` emits
  `GROUP BY ROLLUP(...), ROLLUP(...)` with correct `__grp_<i>` markers for every dim;
  empty `rollup_groups` → plain GROUP BY; StarRocks backtick quoting through the path.
- **`engine.ts` unit tests** (the core): 2×1, 1×2, 2×2 pivots, multi-metric, subtotal node
  insertion + condense, grand total, collapse hides descendants, marker→(i,j) classification,
  null dim values.
- **Engine-gated Trino test:** run a 2×1 ROLLUP×ROLLUP pivot on `tpch.tiny.orders`, assert
  the expected leaf + subtotal + grand-total rows appear (by markers).
- **Frontend render test:** a 2×1 pivot renders nested left headers (rowSpan), a subtotal
  row, and a grand-total row.
- **Manual Playwright smoke** on the live stack; **manual Trino integration test** extended
  for the ROLLUP×ROLLUP path.

## 9. File structure

- Modify `crates/egret-bi/src/chartquery.rs` (rollup_groups), its tests, and the engine test.
- New `frontend/src/components/chart/pivot/engine.ts` (+ tests) and
  `frontend/src/components/chart/pivot/PivotTable.tsx` (+ test); delete the old
  `frontend/src/components/chart/PivotTable.tsx` and its test (superseded).
- Modify `frontend/src/api/charts.ts` (`ChartParams` pivot shape, `rollup_groups` on
  `QuerySpec`) and `frontend/src/routes/explore/ChartExplore.tsx` (controls + `specForRun`
  + hydration).
