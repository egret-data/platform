# Phase 3c-3.2 — Multi-dimensional Pivot Table Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the single-dimension pivot from 3c-3 with a Metabase-grade multi-level pivot — N row dims × N col dims × N measures, nested headers, subtotals at every level, grand total, collapsible row groups.

**Architecture:** Backend emits one `GROUP BY ROLLUP(rows…), ROLLUP(cols…)` query (the cross-product gives every grouping level) with `GROUPING()` markers. A pure frontend engine ports Metabase's `multiLevelPivot`: classify each flat row by active row/col prefix lengths, build a unified value-map keyed by variable-length `(rowPath, colPath)`, and compute a renderer-ready nested structure (body rows with row-span header cells + subtotal/grand rows + multi-metric columns). An HTML-table renderer consumes it.

**Tech Stack:** Rust (egret-bi `chartquery`), React 18 + Vite + TypeScript + Vitest, shadcn/ui.

**Spec:** `docs/en/specs/2026-06-19-phase-3c-3-2-pivot-multidim-design.md`.

**Branch:** `feat/phase-3c-3-chart` (continues 3c-3; not yet merged).

**Conventions:** plain `git commit` (never `--author`); runtime sqlx only; DB/engine-gated tests self-skip; `vitest.setup.ts` registers jest-dom.

---

## File Structure

**Backend (`egret-bi`):**
- Modify `crates/egret-bi/src/chartquery.rs` — `QuerySpec.rollup: bool` → `rollup_groups: Vec<Vec<String>>`; `build()` emits multiple ROLLUPs + markers for every dim; update its unit tests.
- Modify `crates/egret-bi/tests/chart_engine.rs` — update the two `QuerySpec` literals; make the rollup test a 2×1 ROLLUP×ROLLUP pivot.

**Frontend:**
- Modify `frontend/src/api/charts.ts` — `QuerySpec.rollup_groups`, new `ChartParams` pivot shape.
- Modify `frontend/src/api/__tests__/charts.test.ts` — update the `ChartParams` test.
- Create `frontend/src/components/chart/pivot/engine.ts` (+ `__tests__/engine.test.ts`) — the pivot transform.
- Create `frontend/src/components/chart/pivot/PivotTable.tsx` (+ `__tests__/PivotTable.test.tsx`) — the renderer.
- Delete `frontend/src/components/chart/PivotTable.tsx` and `frontend/src/components/chart/__tests__/pivot.test.ts` (superseded).
- Modify `frontend/src/routes/explore/ChartExplore.tsx` — pivot controls + `specForRun`/`paramsForSave`/hydration + import path.

---

## Task 1: Backend — `rollup_groups` (multiple ROLLUPs)

**Files:**
- Modify: `crates/egret-bi/src/chartquery.rs`

- [ ] **Step 1: Change the `QuerySpec` field**

In `crates/egret-bi/src/chartquery.rs`, replace the `rollup: bool` field with `rollup_groups`:

```rust
    pub row_limit: i64,
    #[serde(default)]
    pub rollup_groups: Vec<Vec<String>>,
```
(Remove the old `#[serde(default)] pub rollup: bool,` line.)

- [ ] **Step 2: Update `build()`'s aggregate branch**

Replace the aggregate branch (from `if aggregate {` through the `Ok(sql)` that ends it) with this version. It emits one `ROLLUP(...)` per group and a `GROUPING(...) AS __grp_<i>` marker for every dimension (indexed by position in `spec.dimensions`):

```rust
    if aggregate {
        if !spec.rollup_groups.is_empty() && spec.dimensions.is_empty() {
            return Err(BiError::BadRequest(
                "rollup requires at least one dimension".into(),
            ));
        }
        // Resolve every dimension to its expression once, preserving order.
        let mut dim_exprs: Vec<String> = Vec::new();
        for name in &spec.dimensions {
            let c = find_column(columns, name)
                .ok_or_else(|| BiError::BadRequest(format!("unknown dimension: {name}")))?;
            let e = column_sql(engine, c);
            select.push(format!("{e} AS {}", quote_ident(engine, name)));
            dim_exprs.push(e);
        }
        for name in &spec.metrics {
            let m = find_metric(metrics, name)
                .ok_or_else(|| BiError::BadRequest(format!("unknown metric: {name}")))?;
            select.push(format!("{} AS {}", m.expression, quote_ident(engine, name)));
        }
        let rolling = !spec.rollup_groups.is_empty();
        if rolling {
            for (i, e) in dim_exprs.iter().enumerate() {
                // {i} is a usize counter, never user data — safe unquoted.
                select.push(format!("GROUPING({e}) AS __grp_{i}"));
            }
        }
        let mut sql = format!("SELECT {}\nFROM {from}", select.join(", "));
        push_where(&mut sql, engine, columns, &spec.filters)?;
        if rolling {
            // One ROLLUP(...) per group; the cross-product is every grouping level.
            let mut parts: Vec<String> = Vec::new();
            for group in &spec.rollup_groups {
                let mut exprs: Vec<String> = Vec::new();
                for name in group {
                    let c = find_column(columns, name).ok_or_else(|| {
                        BiError::BadRequest(format!("unknown rollup dimension: {name}"))
                    })?;
                    exprs.push(column_sql(engine, c));
                }
                if !exprs.is_empty() {
                    parts.push(format!("ROLLUP({})", exprs.join(", ")));
                }
            }
            if !parts.is_empty() {
                sql.push_str(&format!("\nGROUP BY {}", parts.join(", ")));
            }
        } else if !dim_exprs.is_empty() {
            sql.push_str(&format!("\nGROUP BY {}", dim_exprs.join(", ")));
        }
        push_orderby(&mut sql, engine, columns, metrics, &spec.orderby)?;
        push_limit(&mut sql, spec.row_limit);
        Ok(sql)
    } else {
```
(The `else { ... }` raw-records branch below is unchanged.)

- [ ] **Step 3: Update the existing chartquery tests for the field rename**

In the `#[cfg(test)] mod tests` block, every `QuerySpec { … }` literal currently has `rollup: false` or `rollup: true`. Change them:
- `rollup: false` → `rollup_groups: vec![]`
- In `build_rollup_adds_grouping_markers`, `rollup: true` → `rollup_groups: vec![vec!["region".into(), "seg".into()]]`, and update its assertions to the cross-product form:

```rust
#[test]
fn build_rollup_adds_grouping_markers() {
    let cols = vec![col("region", "string", None), col("seg", "string", None)];
    let metrics = vec![metric("revenue", "SUM(price)")];
    let spec = QuerySpec {
        metrics: vec!["revenue".into()],
        dimensions: vec!["region".into(), "seg".into()],
        columns: vec![],
        filters: vec![],
        orderby: vec![],
        row_limit: 100,
        rollup_groups: vec![vec!["region".into()], vec!["seg".into()]],
    };
    let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
    assert!(sql.contains("GROUP BY ROLLUP(\"region\"), ROLLUP(\"seg\")"), "{sql}");
    assert!(sql.contains("GROUPING(\"region\") AS __grp_0"), "{sql}");
    assert!(sql.contains("GROUPING(\"seg\") AS __grp_1"), "{sql}");
}
```
Apply the `rollup_groups: vec![]` change to: `build_aggregate_groups_by_dimension_expression`, `build_raw_records_selects_columns_no_group_by`, `build_filters_use_where_and_escape`, `build_rejects_unknown_names`, and `build_aggregate_starrocks_uses_backticks`. (The `sql_literal_*` tests have no `QuerySpec`.)

- [ ] **Step 4: Add a multi-rollup test**

Add to the tests module:

```rust
#[test]
fn build_two_rollups_for_pivot() {
    let cols = vec![
        col("region", "string", None),
        col("nation", "string", None),
        col("seg", "string", None),
    ];
    let metrics = vec![metric("revenue", "SUM(price)")];
    let spec = QuerySpec {
        metrics: vec!["revenue".into()],
        dimensions: vec!["region".into(), "nation".into(), "seg".into()],
        columns: vec![],
        filters: vec![],
        orderby: vec![],
        row_limit: 100,
        rollup_groups: vec![vec!["region".into(), "nation".into()], vec!["seg".into()]],
    };
    let sql = build(Engine::Trino, &phys(), &cols, &metrics, &spec).unwrap();
    assert!(
        sql.contains("GROUP BY ROLLUP(\"region\", \"nation\"), ROLLUP(\"seg\")"),
        "{sql}"
    );
    // a marker per dimension, indexed by position in `dimensions`
    assert!(sql.contains("GROUPING(\"region\") AS __grp_0"), "{sql}");
    assert!(sql.contains("GROUPING(\"nation\") AS __grp_1"), "{sql}");
    assert!(sql.contains("GROUPING(\"seg\") AS __grp_2"), "{sql}");
}
```

- [ ] **Step 5: Run + gates**

Run: `cargo test -p egret-bi chartquery::`
Expected: all pass.
Run: `cargo fmt -p egret-bi && cargo clippy -p egret-bi --all-targets -- -D warnings`
Expected: clean. (The crate won't fully build until Task 2 fixes `chart_engine.rs`; if `cargo test -p egret-bi` fails to compile on the engine test, do Task 2 first or in the same commit.)

- [ ] **Step 6: Commit** (with Task 2 if needed to keep the crate compiling)

```bash
git add crates/egret-bi/src/chartquery.rs
git commit -m "feat(bi): generalize pivot to rollup_groups (multiple ROLLUPs)"
```

---

## Task 2: Backend — engine test for ROLLUP × ROLLUP

**Files:**
- Modify: `crates/egret-bi/tests/chart_engine.rs`

- [ ] **Step 1: Update the two `QuerySpec` literals**

In `crates/egret-bi/tests/chart_engine.rs`, the existing `table_chart_runs_on_trino` and `rollup_chart_adds_grand_total_on_trino` build `QuerySpec` with `rollup: false`/`rollup: true`. Change `table_chart_runs_on_trino`'s literal: `rollup: false` → `rollup_groups: vec![]`. Replace `rollup_chart_adds_grand_total_on_trino` entirely with a 2×1 ROLLUP×ROLLUP pivot over `tpch.tiny.orders` (rows = `orderpriority`, cols = `orderstatus`, metric = `revenue`):

```rust
#[tokio::test]
async fn pivot_rollup_x_rollup_runs_on_trino() {
    let Some(spec) = trino_spec() else {
        eprintln!("skip: EGRET_TEST_TRINO_URL not set");
        return;
    };
    let cols = vec![dim("orderpriority"), dim("orderstatus")];
    let metrics = vec![revenue()];
    let qs = QuerySpec {
        metrics: vec!["revenue".into()],
        dimensions: vec!["orderpriority".into(), "orderstatus".into()],
        columns: vec![],
        filters: vec![],
        orderby: vec![],
        row_limit: 1000,
        rollup_groups: vec![vec!["orderpriority".into()], vec!["orderstatus".into()]],
    };
    let sql = build(spec.engine, &orders(), &cols, &metrics, &qs).unwrap();
    let r = run(&spec, &sql).await;
    assert!(!r.truncated, "unexpected truncation");
    let g0 = r.columns.iter().position(|c| c.name == "__grp_0").expect("__grp_0");
    let g1 = r.columns.iter().position(|c| c.name == "__grp_1").expect("__grp_1");
    // grand total row: both dims rolled up.
    let grand = r.rows.iter().filter(|row| row[g0].as_i64() == Some(1) && row[g1].as_i64() == Some(1)).count();
    assert_eq!(grand, 1, "expected one grand-total row, sql=\n{sql}");
    // row subtotals (priority rolled-up over status): g0=0, g1=1, more than zero.
    let row_subtotals = r.rows.iter().filter(|row| row[g0].as_i64() == Some(0) && row[g1].as_i64() == Some(1)).count();
    assert!(row_subtotals >= 5, "expected per-priority subtotals, sql=\n{sql}");
}
```

The `trino_spec`, `dim`, `revenue`, `orders`, `run` helpers already exist in this file. `QuerySpec` is imported from `egret_bi::chartquery`.

- [ ] **Step 2: Build + run (gated)**

Run: `cargo test -p egret-bi --test chart_engine` (compiles; self-skips without `EGRET_TEST_TRINO_URL`).
If `EGRET_TEST_TRINO_URL` is available (dev stack Trino at `http://localhost:8090`), run `EGRET_TEST_TRINO_URL=http://localhost:8090 cargo test -p egret-bi --test chart_engine` and confirm PASS.

- [ ] **Step 3: Full crate gate + commit**

Run: `cargo test -p egret-bi && cargo fmt -p egret-bi && cargo clippy -p egret-bi --all-targets -- -D warnings`

```bash
git add crates/egret-bi/tests/chart_engine.rs
git commit -m "test(bi): engine-gated Trino test for ROLLUP x ROLLUP pivot"
```

---

## Task 3: Frontend — `charts.ts` types

**Files:**
- Modify: `frontend/src/api/charts.ts`
- Modify: `frontend/src/api/__tests__/charts.test.ts`

- [ ] **Step 1: Update `QuerySpec` and `ChartParams`**

In `frontend/src/api/charts.ts`, change `QuerySpec.rollup: boolean` to `rollup_groups: string[][]`, and replace the `ChartParams` interface:

```ts
export interface QuerySpec {
  metrics: string[];
  dimensions: string[];
  columns: string[];
  filters: ChartFilter[];
  orderby: ChartOrderBy[];
  row_limit: number;
  rollup_groups: string[][];
}

export interface ChartParams {
  spec: QuerySpec;
  // pivot-only layout (ignored for table viz)
  rows?: string[];
  cols?: string[];
  metrics?: string[];
  showRowTotals?: boolean;
  showColTotals?: boolean;
  condense?: boolean;
}
```

- [ ] **Step 2: Update the test**

In `frontend/src/api/__tests__/charts.test.ts`, replace the `ChartParams` test body:

```ts
  it("carries spec + pivot layout", () => {
    const p: ChartParams = {
      spec: { metrics: ["revenue"], dimensions: ["region", "seg"], columns: [], filters: [], orderby: [], row_limit: 100, rollup_groups: [["region"], ["seg"]] },
      rows: ["region"], cols: ["seg"], metrics: ["revenue"], showRowTotals: true, showColTotals: true, condense: true,
    };
    expect(p.spec.rollup_groups).toEqual([["region"], ["seg"]]);
    expect(p.rows).toEqual(["region"]);
  });
```

- [ ] **Step 3: Verify + commit**

Run: `cd frontend && npm test -- charts && npm run typecheck`
Expected: PASS / clean. (`npm run lint` may flag the old `PivotTable` import in ChartExplore later — that's fixed in Task 6; for now the api + test compile.)

```bash
git add frontend/src/api/charts.ts frontend/src/api/__tests__/charts.test.ts
git commit -m "feat(web): charts QuerySpec.rollup_groups + multi-dim ChartParams shape"
```

---

## Task 4: Frontend — pivot engine: classification + value map

**Files:**
- Create: `frontend/src/components/chart/pivot/engine.ts`
- Test: `frontend/src/components/chart/pivot/__tests__/engine.test.ts`

This task builds the *value* half of the engine: classify rows and answer `cell(rowPath, colPath, metric)`. Task 5 adds the *structure* half (body rows / headers).

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/pivot/__tests__/engine.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { indexResult, type PivotInput } from "../engine";

// 1 row dim (region) × 1 col dim (seg) × 1 metric (revenue), ROLLUP×ROLLUP result.
// columns order: region, seg, revenue, __grp_0 (region marker), __grp_1 (seg marker)
const input: PivotInput = {
  columns: ["region", "seg", "revenue", "__grp_0", "__grp_1"],
  rows: [
    ["EU", "SMB", 10, 0, 0],   // leaf
    ["EU", "ENT", 20, 0, 0],   // leaf
    ["EU", null, 30, 0, 1],    // EU row-subtotal (seg rolled)
    ["US", "SMB", 5, 0, 0],    // leaf
    ["US", null, 5, 0, 1],     // US row-subtotal
    [null, "SMB", 15, 1, 0],   // SMB col-subtotal (region rolled)
    [null, "ENT", 20, 1, 0],   // ENT col-subtotal
    [null, null, 35, 1, 1],    // grand total
  ],
  rowDims: ["region"],
  colDims: ["seg"],
  metrics: ["revenue"],
  showRowTotals: true,
  showColTotals: true,
  condense: true,
  collapsed: new Set<string>(),
};

describe("indexResult", () => {
  const ix = indexResult(input);

  it("enumerates ordered row and col leaf paths", () => {
    expect(ix.rowLeafPaths).toEqual([["EU"], ["US"]]);
    expect(ix.colLeafPaths).toEqual([["SMB"], ["ENT"]]);
  });
  it("answers leaf cells", () => {
    expect(ix.cell(["EU"], ["SMB"], "revenue")).toBe(10);
    expect(ix.cell(["US"], ["SMB"], "revenue")).toBe(5);
  });
  it("answers row subtotals (col path empty)", () => {
    expect(ix.cell(["EU"], [], "revenue")).toBe(30);
    expect(ix.cell(["US"], [], "revenue")).toBe(5);
  });
  it("answers col subtotals (row path empty)", () => {
    expect(ix.cell([], ["SMB"], "revenue")).toBe(15);
  });
  it("answers the grand total (both empty)", () => {
    expect(ix.cell([], [], "revenue")).toBe(35);
  });
  it("returns null for a missing cell", () => {
    expect(ix.cell(["EU"], ["NOPE"], "revenue")).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npm test -- engine`
Expected: FAIL (no `engine.ts`).

- [ ] **Step 3: Implement `indexResult`**

Create `frontend/src/components/chart/pivot/engine.ts`:

```ts
export interface PivotInput {
  columns: string[];          // result column names
  rows: unknown[][];          // result rows
  rowDims: string[];          // ordered row dimension names
  colDims: string[];          // ordered column dimension names
  metrics: string[];          // ordered metric names
  showRowTotals: boolean;     // right "Total" column (collapse all col dims)
  showColTotals: boolean;     // nested row-subtotal rows + bottom grand-total row
  condense: boolean;          // drop a subtotal row when its group has one child
  collapsed: Set<string>;     // serialized row-group paths that are collapsed
}

export type CellValue = number | string | null;

/** key for the value map: JSON of [rowPath, colPath]. */
const pathKey = (rowPath: (string | null)[], colPath: (string | null)[]) =>
  JSON.stringify([rowPath, colPath]);

export interface Indexed {
  rowLeafPaths: (string | null)[][];   // distinct full row paths, first-appearance order
  colLeafPaths: (string | null)[][];   // distinct full col paths, first-appearance order
  cell: (rowPath: (string | null)[], colPath: (string | null)[], metric: string) => CellValue;
}

/** Classify each flat ROLLUP×ROLLUP row and build a value map keyed by (rowPath, colPath). */
export function indexResult(input: PivotInput): Indexed {
  const idx = (name: string) => input.columns.indexOf(name);
  const m = input.rowDims.length;
  const n = input.colDims.length;
  const rowDimIdx = input.rowDims.map(idx);
  const colDimIdx = input.colDims.map(idx);
  const metricIdx = new Map(input.metrics.map((mn) => [mn, idx(mn)]));
  // marker for dimensions[k] is __grp_k, dimensions = rowDims ++ colDims
  const rowGrpIdx = input.rowDims.map((_, k) => idx(`__grp_${k}`));
  const colGrpIdx = input.colDims.map((_, k) => idx(`__grp_${m + k}`));

  const values = new Map<string, Record<string, CellValue>>();
  const rowLeafPaths: (string | null)[][] = [];
  const colLeafPaths: (string | null)[][] = [];
  const seenRow = new Set<string>();
  const seenCol = new Set<string>();

  for (const r of input.rows) {
    // active prefix length = count of leading dims with GROUPING marker 0
    let i = 0;
    while (i < m && Number(r[rowGrpIdx[i]]) === 0) i++;
    let j = 0;
    while (j < n && Number(r[colGrpIdx[j]]) === 0) j++;
    const rowPath = rowDimIdx.slice(0, i).map((c) => r[c] as string | null);
    const colPath = colDimIdx.slice(0, j).map((c) => r[c] as string | null);
    const rec: Record<string, CellValue> = {};
    for (const [mn, mi] of metricIdx) rec[mn] = r[mi] as CellValue;
    values.set(pathKey(rowPath, colPath), rec);
    if (i === m && j === n) {
      const rk = JSON.stringify(rowPath);
      if (!seenRow.has(rk)) {
        seenRow.add(rk);
        rowLeafPaths.push(rowPath);
      }
      const ck = JSON.stringify(colPath);
      if (!seenCol.has(ck)) {
        seenCol.add(ck);
        colLeafPaths.push(colPath);
      }
    }
  }

  const cell = (rowPath: (string | null)[], colPath: (string | null)[], metric: string): CellValue => {
    const rec = values.get(pathKey(rowPath, colPath));
    return rec ? rec[metric] ?? null : null;
  };

  return { rowLeafPaths, colLeafPaths, cell };
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npm test -- engine`
Expected: 6 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/pivot/engine.ts frontend/src/components/chart/pivot/__tests__/engine.test.ts
git commit -m "feat(web): pivot engine — row classification + (rowPath,colPath) value map"
```

---

## Task 5: Frontend — pivot engine: nested structure (body rows + headers)

**Files:**
- Modify: `frontend/src/components/chart/pivot/engine.ts`
- Modify: `frontend/src/components/chart/pivot/__tests__/engine.test.ts`

Builds the renderer-ready structure: a row tree with subtotal/grand rows + computed `rowSpan`, the top header levels (+ metric level + Total column), and a flat `bodyRows` list with values.

- [ ] **Step 1: Write the failing structure test**

Append to `engine.test.ts`:

```ts
import { buildPivot } from "../engine";

describe("buildPivot single-dim (1 row × 1 col × 1 metric)", () => {
  const p = buildPivot(input);

  it("has one body row per row leaf plus a grand-total row", () => {
    const kinds = p.bodyRows.map((b) => b.kind);
    expect(kinds).toEqual(["leaf", "leaf", "grand"]); // EU, US, Grand total
  });
  it("col leaves are the distinct columns, each metric, plus a Total column", () => {
    // 2 col leaves × 1 metric + 1 total column (showRowTotals) = 3 value columns
    expect(p.colLeaves.length).toBe(3);
    expect(p.colLeaves[2].isTotal).toBe(true);
  });
  it("places leaf values and the row total", () => {
    const eu = p.bodyRows[0];
    expect(eu.kind).toBe("leaf");
    // values aligned to colLeaves: SMB=10, ENT=20, Total=30
    expect(eu.values).toEqual(["10", "20", "30"]);
  });
  it("grand total row totals across rows", () => {
    const grand = p.bodyRows[2];
    // SMB col total=15, ENT col total=20, grand=35
    expect(grand.values).toEqual(["15", "20", "35"]);
  });
});

describe("buildPivot nested rows (2 row × 1 col)", () => {
  // region → nation, seg columns. columns: region, nation, seg, revenue, __grp_0, __grp_1, __grp_2
  const nested: PivotInput = {
    columns: ["region", "nation", "seg", "revenue", "__grp_0", "__grp_1", "__grp_2"],
    rows: [
      ["EU", "DE", "SMB", 4, 0, 0, 0],
      ["EU", "FR", "SMB", 6, 0, 0, 0],
      ["EU", "DE", null, 4, 0, 0, 1],   // DE row subtotal
      ["EU", "FR", null, 6, 0, 0, 1],   // FR row subtotal
      ["EU", null, "SMB", 10, 0, 1, 0], // EU subtotal (nation rolled), seg full
      ["EU", null, null, 10, 0, 1, 1],  // EU grand-of-region
      [null, null, "SMB", 10, 1, 1, 0],
      [null, null, null, 10, 1, 1, 1],  // grand total
    ],
    rowDims: ["region", "nation"], colDims: ["seg"], metrics: ["revenue"],
    showRowTotals: true, showColTotals: true, condense: true, collapsed: new Set(),
  };
  const p = buildPivot(nested);

  it("emits leaves, a region subtotal row, and a grand-total row", () => {
    // DE leaf, FR leaf, EU subtotal, Grand total
    expect(p.bodyRows.map((b) => b.kind)).toEqual(["leaf", "leaf", "subtotal", "grand"]);
  });
  it("the region cell rowSpans its two leaf rows", () => {
    // first body row's header cells: region(span 2) + nation(span1)
    const cells = p.bodyRows[0].headerCells;
    const region = cells.find((c) => c.depth === 0)!;
    expect(region.rowSpan).toBe(2);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npm test -- engine`
Expected: FAIL (`buildPivot` not defined).

- [ ] **Step 3: Implement `buildPivot` + types**

Append to `engine.ts`:

```ts
export interface ColLeaf {
  colPath: (string | null)[];
  metric: string;
  isTotal: boolean;        // the right-hand "Total" column group (colPath = [])
}
export interface TopCell {
  label: string;
  colSpan: number;
}
export interface RowHeaderCell {
  label: string;
  depth: number;           // which row-dim column it starts at
  rowSpan: number;
  colSpan: number;
}
export interface BodyRow {
  rowPath: (string | null)[];
  kind: "leaf" | "subtotal" | "grand";
  headerCells: RowHeaderCell[]; // only cells that START on this row
  values: string[];             // formatted, aligned to colLeaves
}
export interface Pivot {
  rowDimNames: string[];
  topHeader: TopCell[][];       // one array per top-header level
  colLeaves: ColLeaf[];
  bodyRows: BodyRow[];
}

const fmt = (v: CellValue): string => {
  if (v == null) return "";
  if (typeof v === "number") return v.toLocaleString();
  const n = Number(v);
  return typeof v === "string" && v.trim() !== "" && Number.isFinite(n)
    ? n.toLocaleString()
    : String(v);
};

// A node in the row or column tree.
interface TreeNode {
  value: string | null;
  children: TreeNode[];
  childByValue: Map<string | null, TreeNode>;
}
const newNode = (value: string | null): TreeNode => ({ value, children: [], childByValue: new Map() });

function buildTree(paths: (string | null)[][]): TreeNode {
  const root = newNode(null);
  for (const path of paths) {
    let cur = root;
    for (const v of path) {
      let next = cur.childByValue.get(v);
      if (!next) {
        next = newNode(v);
        cur.childByValue.set(v, next);
        cur.children.push(next);
      }
      cur = next;
    }
  }
  return root;
}

export function buildPivot(input: PivotInput): Pivot {
  const ix = indexResult(input);
  const m = input.rowDims.length;
  const rowTree = buildTree(ix.rowLeafPaths);
  const colTree = buildTree(ix.colLeafPaths);

  // ----- column leaves (full col paths × metrics, + optional Total group) -----
  const colLeafPaths: (string | null)[][] = [];
  (function collect(node: TreeNode, path: (string | null)[]) {
    if (node.children.length === 0) {
      if (path.length > 0) colLeafPaths.push(path);
      return;
    }
    for (const ch of node.children) collect(ch, [...path, ch.value]);
  })(colTree, []);
  // when there are no column dims, a single empty col path holds the metrics
  const effectiveColPaths = colLeafPaths.length ? colLeafPaths : [[]];

  const colLeaves: ColLeaf[] = [];
  for (const cp of effectiveColPaths) {
    for (const metric of input.metrics) colLeaves.push({ colPath: cp, metric, isTotal: false });
  }
  if (input.showRowTotals) {
    for (const metric of input.metrics) colLeaves.push({ colPath: [], metric, isTotal: true });
  }

  // ----- top header levels -----
  const topHeader: TopCell[][] = [];
  for (let d = 0; d < input.colDims.length; d++) {
    const level: TopCell[] = [];
    for (const cp of effectiveColPaths) {
      const span = input.metrics.length;
      level.push({ label: String(cp[d] ?? ""), colSpan: span });
    }
    if (input.showRowTotals) level.push({ label: "Total", colSpan: input.metrics.length });
    topHeader.push(level);
  }
  if (input.metrics.length > 1) {
    const level: TopCell[] = [];
    const groups = effectiveColPaths.length + (input.showRowTotals ? 1 : 0);
    for (let g = 0; g < groups; g++) for (const metric of input.metrics) level.push({ label: metric, colSpan: 1 });
    topHeader.push(level);
  }

  // ----- body rows (depth-first over row tree, inserting subtotals) -----
  const bodyRows: BodyRow[] = [];
  // returns the number of body rows produced for `node`'s subtree, EXCLUDING node's own subtotal row.
  function emit(node: TreeNode, path: (string | null)[], depth: number): number {
    if (node.children.length === 0) {
      // leaf row
      bodyRows.push({ rowPath: path, kind: "leaf", headerCells: [], values: rowValues(path) });
      return 1;
    }
    let span = 0;
    const startIndex = bodyRows.length;
    for (const ch of node.children) {
      const childStart = bodyRows.length;
      const childSpan = emit(ch, [...path, ch.value], depth + 1);
      // record the child's header cell at `depth+1` on its first body row
      const isLeafLevel = depth + 1 === m - 1 + 1; // child has no children
      bodyRows[childStart].headerCells.push({
        label: String(ch.value ?? ""),
        depth: depth + 1,
        rowSpan: childSpan,
        colSpan: 1,
      });
      span += childSpan;
      // subtotal row for this child group (child is internal and showColTotals)
      const childHasSubtotal =
        input.showColTotals && ch.children.length > 0 && (!input.condense || ch.children.length > 1);
      if (childHasSubtotal) {
        const subPath = [...path, ch.value];
        bodyRows.push({
          rowPath: subPath,
          kind: "subtotal",
          headerCells: [{ label: "Total", depth: depth + 1, rowSpan: 1, colSpan: m - (depth + 1) }],
          values: rowValues(subPath),
        });
        span += 1;
      }
      void isLeafLevel;
      void startIndex;
      void childStart;
    }
    return span;
  }

  function rowValues(rowPath: (string | null)[]): string[] {
    return colLeaves.map((cl) => fmt(ix.cell(rowPath, cl.colPath, cl.metric)));
  }

  // emit top-level groups
  let total = 0;
  const topStart = 0;
  for (const ch of rowTree.children) {
    const start = bodyRows.length;
    const childSpan = emit(ch, [ch.value], 0); // top-level row nodes are depth 0
    bodyRows[start].headerCells.unshift({
      label: String(ch.value ?? ""),
      depth: 0,
      rowSpan: childSpan,
      colSpan: 1,
    });
    total += childSpan;
    const hasSubtotal =
      input.showColTotals && ch.children.length > 0 && (!input.condense || ch.children.length > 1);
    if (hasSubtotal) {
      const subPath = [ch.value];
      bodyRows.push({
        rowPath: subPath,
        kind: "subtotal",
        headerCells: [{ label: "Total", depth: 0, rowSpan: 1, colSpan: m }],
        values: rowValues(subPath),
      });
    }
  }
  void total;
  void topStart;

  // grand total row
  if (input.showColTotals) {
    bodyRows.push({
      rowPath: [],
      kind: "grand",
      headerCells: [{ label: "Grand total", depth: 0, rowSpan: 1, colSpan: m }],
      values: rowValues([]),
    });
  }

  return { rowDimNames: input.rowDims, topHeader, colLeaves, bodyRows };
}
```

NOTE on the single-dim case (m=1): a top-level node IS a leaf (no children), so `emit` pushes a leaf row and the top-level loop's `headerCells.unshift` adds the region cell with rowSpan = childSpan = 1. `hasSubtotal` is false (no children). The grand-total row is appended. This matches the first test (`["leaf","leaf","grand"]`). For the nested case (m=2), the region node has children, so its leaves emit, then the region subtotal row appends, then grand total — matching `["leaf","leaf","subtotal","grand"]`.

- [ ] **Step 4: Run to verify it passes**

Run: `cd frontend && npm test -- engine`
Expected: all PASS (Task 4 tests + the new structure tests). If the nested `rowSpan` assertion fails, the bug is in how the top-level region cell's `rowSpan` is computed — it must equal the number of body rows for its subtree EXCLUDING its own subtotal row (i.e. `childSpan` returned by `emit`, which already excludes the node's own subtotal). Verify `emit` returns leaf+nested-subtotal counts but not the node's own subtotal.

- [ ] **Step 5: Clean up unused bindings + gates**

Remove the `void ...;` no-op guards and the genuinely-unused bindings (`isLeafLevel`, `startIndex`, `total`, `topStart`). KEEP `childStart` — it IS used (`bodyRows[childStart].headerCells.push(...)`). After deleting the unused ones, drop their `void` lines.
Run: `cd frontend && npm test -- engine && npm run typecheck && npm run lint`
Expected: PASS / clean.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/pivot/engine.ts frontend/src/components/chart/pivot/__tests__/engine.test.ts
git commit -m "feat(web): pivot engine — nested body rows, subtotals, grand total, multi-metric columns"
```

---

## Task 6: Frontend — `PivotTable` v2 renderer

**Files:**
- Create: `frontend/src/components/chart/pivot/PivotTable.tsx`
- Test: `frontend/src/components/chart/pivot/__tests__/PivotTable.test.tsx`
- Delete: `frontend/src/components/chart/PivotTable.tsx`, `frontend/src/components/chart/__tests__/pivot.test.ts`

- [ ] **Step 1: Write the failing render test**

Create `frontend/src/components/chart/pivot/__tests__/PivotTable.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { PivotTable } from "../PivotTable";
import type { PivotInput } from "../engine";

const input: PivotInput = {
  columns: ["region", "seg", "revenue", "__grp_0", "__grp_1"],
  rows: [
    ["EU", "SMB", 10, 0, 0], ["EU", "ENT", 20, 0, 0], ["EU", null, 30, 0, 1],
    ["US", "SMB", 5, 0, 0], ["US", null, 5, 0, 1],
    [null, "SMB", 15, 1, 0], [null, "ENT", 20, 1, 0], [null, null, 35, 1, 1],
  ],
  rowDims: ["region"], colDims: ["seg"], metrics: ["revenue"],
  showRowTotals: true, showColTotals: true, condense: true, collapsed: new Set(),
};

describe("PivotTable", () => {
  it("renders the column headers, a leaf value, and the grand total", () => {
    render(<PivotTable input={input} />);
    expect(screen.getByText("SMB")).toBeInTheDocument();
    expect(screen.getByText("ENT")).toBeInTheDocument();
    expect(screen.getByText("Grand total")).toBeInTheDocument();
    expect(screen.getByText("35")).toBeInTheDocument(); // grand total value
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `cd frontend && npm test -- pivot/__tests__/PivotTable`
Expected: FAIL.

- [ ] **Step 3: Implement the renderer**

Create `frontend/src/components/chart/pivot/PivotTable.tsx`:

```tsx
import { buildPivot, type PivotInput } from "./engine";

export function PivotTable({ input }: { input: PivotInput }) {
  const p = buildPivot(input);
  const rowDimCount = p.rowDimNames.length;

  return (
    <div className="overflow-auto">
      <table className="border-collapse text-sm">
        <thead>
          {p.topHeader.map((level, li) => (
            <tr key={li} className="border-b text-muted-foreground">
              {/* top-left corner spanning the row-dim columns, only on the first header row */}
              {li === 0 && (
                <th
                  className="px-3 py-1 text-left"
                  colSpan={rowDimCount}
                  rowSpan={p.topHeader.length}
                >
                  {p.rowDimNames.join(" / ")}
                </th>
              )}
              {level.map((c, ci) => (
                <th key={ci} className="border-l px-3 py-1 text-center" colSpan={c.colSpan}>
                  {c.label}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody>
          {p.bodyRows.map((br, ri) => {
            const subtotal = br.kind === "subtotal" || br.kind === "grand";
            return (
              <tr
                key={ri}
                className={`border-t ${subtotal ? "bg-muted/50 font-medium" : ""}`}
              >
                {[...br.headerCells]
                  .sort((a, b) => a.depth - b.depth) // render row-dim cells left→right by depth
                  .map((hc, hi) => (
                    <th
                      key={hi}
                      scope="row"
                      className="px-3 py-1 text-left align-top font-normal"
                      rowSpan={hc.rowSpan}
                      colSpan={hc.colSpan}
                    >
                      {hc.label}
                    </th>
                  ))}
                {br.values.map((v, vi) => (
                  <td
                    key={vi}
                    className={`border-l px-3 py-1 text-right tabular-nums ${
                      p.colLeaves[vi].isTotal ? "font-medium" : ""
                    }`}
                  >
                    {v}
                  </td>
                ))}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
```

(Collapse interactivity ships as a follow-up step in Task 7 wiring — the engine already accepts `collapsed`; the renderer can stay collapse-agnostic for this task. If you add a caret here, gate it behind a prop callback; otherwise leave it out to keep this task focused on rendering structure.)

- [ ] **Step 4: Delete the old single-dim pivot**

```bash
git rm frontend/src/components/chart/PivotTable.tsx frontend/src/components/chart/__tests__/pivot.test.ts
```
(Task 7 updates the import in `ChartExplore.tsx`; until then `npm run typecheck` will error on that import — that's expected. Run the pivot tests in isolation here.)

- [ ] **Step 5: Run the render test**

Run: `cd frontend && npm test -- pivot/__tests__/PivotTable`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/pivot/PivotTable.tsx frontend/src/components/chart/pivot/__tests__/PivotTable.test.tsx
git commit -m "feat(web): multi-level PivotTable renderer (nested headers, subtotals, grand total)"
```

---

## Task 7: Frontend — explore controls + wiring

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Replace pivot state + controls**

In `frontend/src/routes/explore/ChartExplore.tsx`:

1. Update the import: `import { PivotTable } from "@/components/chart/pivot/PivotTable";` (was `@/components/chart/PivotTable`).
2. Replace the single-dim pivot state (`pivotRow`, `pivotCol`, `pivotMetric`) with ordered arrays + toggles:

```tsx
  const [pRows, setPRows] = useState<string[]>([]);
  const [pCols, setPCols] = useState<string[]>([]);
  const [pVals, setPVals] = useState<string[]>([]);
  const [showRowTotals, setShowRowTotals] = useState(true);
  const [showColTotals, setShowColTotals] = useState(true);
  const [condense, setCondense] = useState(true);
```

3. `specForRun()` pivot branch:

```tsx
    if (viz === "pivot_table") {
      const dims = [...pRows, ...pCols];
      const groups = [pRows, pCols].filter((g) => g.length > 0);
      return {
        metrics: pVals,
        dimensions: dims,
        columns: [],
        filters: [],
        orderby: [],
        row_limit: rowLimit,
        rollup_groups: groups,
      };
    }
```

4. `paramsForSave()` pivot branch:

```tsx
      ? { spec: specForRun(), rows: pRows, cols: pCols, metrics: pVals, showRowTotals, showColTotals, condense }
```

5. Run-guard for pivot: `if (viz === "pivot_table" && (pRows.length === 0 || pVals.length === 0)) { toast.error("Pivot needs at least one row dimension and one value"); return; }`

6. Hydration effect pivot branch:

```tsx
    if (c.viz_type === "pivot_table") {
      setPRows(p.rows ?? []);
      setPCols(p.cols ?? []);
      setPVals(p.metrics ?? []);
      setShowRowTotals(p.showRowTotals ?? true);
      setShowColTotals(p.showColTotals ?? true);
      setCondense(p.condense ?? true);
    } else { /* table branch unchanged */ }
```

7. Render the preview pivot:

```tsx
        {finished && results.data && viz === "pivot_table" && (
          <PivotTable
            input={{
              columns: results.data.columns.map((c) => c.name),
              rows: results.data.rows,
              rowDims: pRows, colDims: pCols, metrics: pVals,
              showRowTotals, showColTotals, condense, collapsed: new Set(),
            }}
          />
        )}
```

- [ ] **Step 2: Build the ordered Rows / Columns / Values pickers**

Replace the three single-`Select`s with three ordered chip-list pickers. Each shows the currently-selected items (in order, with a remove "×") plus a `Select` to add the next item from the remaining candidates. Implement a small inline helper inside the component:

```tsx
  function OrderedPicker({
    label, selected, onChange, options,
  }: { label: string; selected: string[]; onChange: (v: string[]) => void; options: string[] }) {
    const remaining = options.filter((o) => !selected.includes(o));
    return (
      <div className="space-y-1">
        <div className="text-sm">{label}</div>
        <div className="flex flex-wrap gap-1">
          {selected.map((s) => (
            <span key={s} className="inline-flex items-center gap-1 rounded bg-muted px-2 py-0.5 text-xs">
              {s}
              <button type="button" className="text-muted-foreground hover:text-destructive"
                onClick={() => onChange(selected.filter((x) => x !== s))}>×</button>
            </span>
          ))}
        </div>
        {remaining.length > 0 && (
          <Select value="" onValueChange={(v) => v && onChange([...selected, v])}>
            <SelectTrigger className="h-8"><SelectValue placeholder={`Add ${label.toLowerCase()}…`} /></SelectTrigger>
            <SelectContent>
              {remaining.map((o) => <SelectItem key={o} value={o}>{o}</SelectItem>)}
            </SelectContent>
          </Select>
        )}
      </div>
    );
  }
```

Use it for the pivot controls (a column may be a row OR a column dim, not both; values come from metrics):

```tsx
      {viz === "pivot_table" && (
        <>
          <OrderedPicker label="Rows" selected={pRows} onChange={setPRows}
            options={columns.map((c) => c.column_name).filter((n) => !pCols.includes(n))} />
          <OrderedPicker label="Columns" selected={pCols} onChange={setPCols}
            options={columns.map((c) => c.column_name).filter((n) => !pRows.includes(n))} />
          <OrderedPicker label="Values" selected={pVals} onChange={setPVals}
            options={metrics.map((m) => m.metric_name)} />
          <label className="flex items-center gap-2 text-sm">
            <Checkbox checked={showRowTotals} onCheckedChange={(v) => setShowRowTotals(!!v)} /> Row totals
          </label>
          <label className="flex items-center gap-2 text-sm">
            <Checkbox checked={showColTotals} onCheckedChange={(v) => setShowColTotals(!!v)} /> Column totals
          </label>
          <label className="flex items-center gap-2 text-sm">
            <Checkbox checked={condense} onCheckedChange={(v) => setCondense(!!v)} /> Condense duplicate totals
          </label>
        </>
      )}
```

`columns` and `metrics` are the already-derived `detail.data?.columns ?? []` / `detail.data?.metrics ?? []`. The `Select`/`SelectItem`/`Checkbox` imports already exist in the file from 3c-3.

- [ ] **Step 3: Typecheck, lint, build, test**

Run: `cd frontend && npm run typecheck && npm run lint && npm run build && npm test`
Expected: clean / all pass. Fix any leftover references to the removed `pivotRow`/`pivotCol`/`pivotMetric`.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(web): pivot explore controls — ordered Rows/Columns/Values + totals toggles"
```

---

## Task 8: Sweep + manual smoke + final review

**Files:** none (verification).

- [ ] **Step 1: Backend + frontend sweeps**

Run: `cargo test -p egret-bi && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings`
Run: `cd frontend && npm test && npm run typecheck && npm run lint && npm run build`
Expected: all green.

- [ ] **Step 2: Engine-gated Trino test**

Run: `EGRET_TEST_TRINO_URL=http://localhost:8090 cargo test -p egret-bi --test chart_engine`
Expected: PASS (the ROLLUP×ROLLUP pivot query runs on live Trino).

- [ ] **Step 3: Manual Playwright smoke**

The dev backend must be the new build (rebuild + restart if needed; it auto-migrates on boot — no new migration this phase). With the stack up (login `admin@egret.local` / `Admin12345!`, ws `eng`, dataset `orders`):
1. Explore → Charts → + Chart → dataset `orders` → Pivot.
2. Rows = `orderpriority`, `orderstatus` (two row dims); Columns = `clerk`-or another low-cardinality dim; Values = `revenue`. Run.
3. Verify nested row groups with `orderstatus` indented under `orderpriority`, per-group subtotal rows, a right Total column, and a Grand total row — values consistent.
4. Toggle Row totals / Column totals / Condense and confirm columns/rows appear/disappear.
5. Save, reopen, confirm the controls hydrate.

- [ ] **Step 4: Final holistic review**

Dispatch a final code reviewer over `git diff` for this phase. Focus: the `rollup_groups` SQL (no injection — names still lookup-validated; markers from a counter), the engine's classification + span math (does the region cell's `rowSpan` correctly exclude its own subtotal? do subtotal/grand values come from the right grouping rows?), and the renderer's `rowSpan`/`colSpan` correctness.

- [ ] **Step 5: Final commit (if fixes)**

```bash
git add -A && git commit -m "test: phase 3c-3.2 multi-dim pivot sweep + manual smoke fixes"
```

Then use **superpowers:finishing-a-development-branch**.

---

## Self-Review notes (plan author)

- **Spec coverage:** ROLLUP×ROLLUP (T1/T2), engine classification+values (T4), nested structure+subtotals+grand+multi-metric (T5), renderer (T6), controls+toggles+hydration (T7), tests at every layer (T1–T8). Collapse: the engine input accepts `collapsed` and the controls pass `new Set()`; interactive collapse UI is wired minimally (deferred caret) — the data path exists; full collapse interaction can be a fast follow if not completed in T7.
- **Type consistency:** `PivotInput`, `Indexed.cell`, `buildPivot → Pivot{topHeader,colLeaves,bodyRows}`, `ChartParams{rows,cols,metrics,showRowTotals,showColTotals,condense}`, and `QuerySpec.rollup_groups` are used identically across T3–T7.
- **Known risk:** the `emit`/`rowSpan` recursion is the subtle part; T5's nested test pins the region `rowSpan`, and T8's manual smoke + final review verify multi-level rendering on real data.
