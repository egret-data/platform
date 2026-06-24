# Object Detail Visualization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an "Object detail" visualization that renders one record as a vertical field/value list with prev/next paging through the queried rows.

**Architecture:** A new `"object"` viz type registered in the viz union + gallery; an `ObjectDetail` component that pages through `rows` and formats each field with the table's existing `formatValue`; routed via `ChartRenderer` (so it works in the builder preview and dashboard cards alike); the builder reuses the table column-checkbox picker (`tableCols`) to choose which fields to show.

**Tech Stack:** React 18 + TS + Vite + Vitest + Tailwind v4 + lucide-react.

**Spec:** `docs/en/specs/2026-06-24-object-detail-viz-design.md`
**Branch:** `feat/object-detail-viz` (already checked out; spec committed).

**Conventions (every task):** all commands from `frontend/`. Sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`. Stage explicit paths. Commit footer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. Do NOT push until Task 4.

---

### Task 1: `ObjectDetail` component

**Files:**
- Create: `frontend/src/components/chart/ObjectDetail.tsx`
- Test: `frontend/src/components/chart/__tests__/ObjectDetail.test.tsx`

Background: `ResultColumn` (`{ name: string; type: string }`) is exported from `@/components/chart/ChartTable`. `formatValue(value, type, fmt)` is exported from `@/components/chart/format`; `ColumnFormat` from `@/components/chart/format-types`.

- [ ] **Step 1: Write the failing test**

`frontend/src/components/chart/__tests__/ObjectDetail.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ObjectDetail } from "../ObjectDetail";

const cols = [{ name: "id", type: "int" }, { name: "plan", type: "text" }];
const rows = [[1, "Basic"], [2, "Premium"]];

describe("ObjectDetail", () => {
  it("renders the first record's field/value pairs", () => {
    render(<ObjectDetail columns={cols} rows={rows} />);
    expect(screen.getByText("id")).toBeTruthy();
    expect(screen.getByText("plan")).toBeTruthy();
    expect(screen.getByText("Basic")).toBeTruthy();
    expect(screen.getByText("1 / 2")).toBeTruthy();
  });

  it("pages to the next record and clamps the buttons", () => {
    render(<ObjectDetail columns={cols} rows={rows} />);
    const prev = screen.getByLabelText("previous record") as HTMLButtonElement;
    const next = screen.getByLabelText("next record") as HTMLButtonElement;
    expect(prev.disabled).toBe(true);
    fireEvent.click(next);
    expect(screen.getByText("Premium")).toBeTruthy();
    expect(screen.getByText("2 / 2")).toBeTruthy();
    expect((screen.getByLabelText("next record") as HTMLButtonElement).disabled).toBe(true);
  });

  it("hides the nav bar for a single record", () => {
    render(<ObjectDetail columns={cols} rows={[[1, "Basic"]]} />);
    expect(screen.queryByLabelText("next record")).toBeNull();
  });

  it("shows 'No data' for empty rows", () => {
    render(<ObjectDetail columns={cols} rows={[]} />);
    expect(screen.getByText("No data")).toBeTruthy();
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run: `npx vitest run src/components/chart/__tests__/ObjectDetail.test.tsx`
Expected: FAIL — cannot resolve `../ObjectDetail`.

- [ ] **Step 3: Implement the component**

`frontend/src/components/chart/ObjectDetail.tsx`:

```tsx
import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { formatValue } from "@/components/chart/format";
import type { ColumnFormat } from "@/components/chart/format-types";
import type { ResultColumn } from "@/components/chart/ChartTable";

/** Renders a single record as a vertical field/value list, paging through the rows. */
export function ObjectDetail({
  columns,
  rows,
  columnSettings = {},
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  columnSettings?: Record<string, ColumnFormat>;
}) {
  const [index, setIndex] = useState(0);
  // Reset to the first record whenever the data changes (a new query → new rows reference).
  useEffect(() => setIndex(0), [rows]);

  if (rows.length === 0) {
    return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>;
  }
  const i = Math.min(index, rows.length - 1);
  const row = rows[i];

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <dl className="divide-y">
          {columns.map((c, ci) => (
            <div key={c.name} className="flex gap-3 px-3 py-2">
              <dt className="w-2/5 shrink-0 truncate text-xs font-medium uppercase tracking-wide text-muted-foreground">{c.name}</dt>
              <dd className="flex-1 break-words text-sm tabular-nums">{formatValue(row[ci], c.type, columnSettings[c.name])}</dd>
            </div>
          ))}
        </dl>
      </div>
      {rows.length > 1 && (
        <div className="flex items-center justify-between border-t px-3 py-1.5 text-xs text-muted-foreground">
          <button type="button" aria-label="previous record" disabled={i === 0}
            onClick={() => setIndex(i - 1)} className="disabled:opacity-40">
            <ChevronLeft className="h-4 w-4" />
          </button>
          <span>{i + 1} / {rows.length}</span>
          <button type="button" aria-label="next record" disabled={i >= rows.length - 1}
            onClick={() => setIndex(i + 1)} className="disabled:opacity-40">
            <ChevronRight className="h-4 w-4" />
          </button>
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 4: Run tests**

Run: `npx vitest run src/components/chart/__tests__/ObjectDetail.test.tsx`
Expected: PASS (4 tests).

- [ ] **Step 5: Commit**

```bash
git add src/components/chart/ObjectDetail.tsx src/components/chart/__tests__/ObjectDetail.test.tsx
git commit -m "feat(chart): ObjectDetail component (field/value record with prev/next paging)"
```

---

### Task 2: Register the `object` viz type + render route

**Files:**
- Modify: `frontend/src/api/charts.ts` (`VizType` union, lines 5–7)
- Modify: `frontend/src/components/chart/chartTypes.ts` (`CHART_TYPES` array + import)
- Modify: `frontend/src/components/chart/ChartRenderer.tsx` (add a branch + import)

- [ ] **Step 1: Add `"object"` to the VizType union**

In `frontend/src/api/charts.ts`, the second line of the `VizType` union currently ends with `"sankey"`:
```ts
  | "bar" | "row" | "line" | "area" | "scatter" | "combo" | "pie" | "scalar" | "progress" | "gauge" | "funnel" | "waterfall" | "smartscalar" | "boxplot" | "sankey" | "object";
```
(Add `| "object"` at the end. `object` is NOT cartesian, so `CARTESIAN_VIZ` / `isCartesianViz` stay unchanged.)

- [ ] **Step 2: Add the gallery entry**

In `frontend/src/components/chart/chartTypes.ts`, add `FileText` to the lucide import at the top:
```ts
import {
  Table2, LayoutGrid, BarChart3, BarChartHorizontal, LineChart, AreaChart, ScatterChart, Spline, PieChart, Hash, BarChartHorizontalBig, Gauge, Filter, BarChart4, TrendingUp, Box, Workflow, FileText,
  type LucideIcon,
} from "lucide-react";
```
Append to the `CHART_TYPES` array (after the `sankey` entry):
```ts
  { key: "object", name: "Object detail", description: "Inspect one record as field/value pairs, paging through results.", icon: FileText },
```

- [ ] **Step 3: Route `object` in ChartRenderer**

In `frontend/src/components/chart/ChartRenderer.tsx`, add the import near the other component imports:
```ts
import { ObjectDetail } from "@/components/chart/ObjectDetail";
```
Add a branch (place it next to the `table` branch, e.g. immediately before `if (vizType === "table") {`):
```tsx
  if (vizType === "object") {
    return <ObjectDetail columns={columns} rows={rows} columnSettings={columnSettings} />;
  }
```
(`columns`, `rows`, and `columnSettings` are already in scope in `ChartRenderer`.)

- [ ] **Step 4: Sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green. (Object detail is now selectable in the gallery and renders when given a spec; builder controls come next.)

- [ ] **Step 5: Commit**

```bash
git add src/api/charts.ts src/components/chart/chartTypes.ts src/components/chart/ChartRenderer.tsx
git commit -m "feat(chart): register Object detail viz + route it in ChartRenderer"
```

---

### Task 3: Builder wiring in ChartExplore

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

Object detail is a raw-row query over the picked columns; it reuses the table's `tableCols` state and `toggleTableCol`.

- [ ] **Step 1: Add the `specForRun` branch**

In `specForRun()`, immediately BEFORE the final table return (the `const agg = tableMetrics.length > 0;` block), add:
```tsx
    if (viz === "object") {
      return { metrics: [], dimensions: [], columns: tableCols, filters, orderby: [], row_limit: rowLimit, rollup_groups: [] };
    }
```

- [ ] **Step 2: Hydrate a saved object chart**

In the hydration `if/else` chain, the final `else` (line ~127) handles `table`. Insert an `else if` for object BEFORE that final `else`:
```tsx
    } else if (c.viz_type === "object") {
      setTableCols(p.spec?.columns ?? []);
    } else {
```
(So the chain reads `… else if (c.viz_type === "sankey") { … } else if (c.viz_type === "object") { setTableCols(p.spec?.columns ?? []); } else { setTableMetrics(…); setTableCols(…); }`.)

- [ ] **Step 3: Add the run-guard**

In `async function run()` (line ~263), after the existing per-viz guards (e.g. right after the `sankey` guard near line 297, before the build/submit call), add:
```tsx
    if (viz === "object" && tableCols.length === 0) { toast.error("Object detail needs at least one column"); return; }
```
Also add the same guard to the save handler (the second validation chain near line ~347, after its dataset/name checks):
```tsx
    if (viz === "object" && tableCols.length === 0) { toast.error("Object detail needs at least one column"); return; }
```

- [ ] **Step 4: Add the column-picker control block**

The table controls block is `{viz === "table" && ( … )}` (line ~439). Add a sibling block for object that renders ONLY the column-checkbox list (no metrics, no aggregate, no unit list). Place it right after the `{viz === "table" && (…)}` block:
```tsx
            {viz === "object" && (
              <div className="space-y-1">
                <Label className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Fields</Label>
                <div className="space-y-1 pt-1">
                  {columns.length === 0 && <p className="text-xs italic text-muted-foreground">Select a dataset to see columns.</p>}
                  {columns.map((col) => (
                    <div key={col.column_name} className="flex items-center gap-2">
                      <Checkbox id={`obj-${col.column_name}`} checked={tableCols.includes(col.column_name)} onCheckedChange={() => toggleTableCol(col.column_name)} />
                      <Label htmlFor={`obj-${col.column_name}`} className="cursor-pointer text-sm font-normal">{col.column_name}</Label>
                    </div>
                  ))}
                </div>
              </div>
            )}
```
(`Label`, `Checkbox`, `columns`, `tableCols`, `toggleTableCol` are all already in scope/imported in this file.)

- [ ] **Step 5: Sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green. (The preview routes any non-`table`/`pivot_table` viz through `ChartRenderer`, so Object detail renders in the preview with no preview-branch change.)

- [ ] **Step 6: Commit**

```bash
git add src/routes/explore/ChartExplore.tsx
git commit -m "feat(chart): Object detail builder controls (column picker + raw-row query)"
```

---

### Task 4: Live verify + PR

**Files:** none (verification + PR)

- [ ] **Step 1: Full sweep**

Run: `npm run typecheck && npm run lint && npm run build && npx vitest run`
Expected: all green.

- [ ] **Step 2: Live verify (Playwright, dev backend up, throwaway chart, 0 console errors)**

Log in `admin@egret.local` / `Admin12345!`. New chart → pick **Object detail** → dataset `ecom_orders` → check several fields (e.g. `id`, `product`, `category`, `total`) → Run. Confirm:
1. Record 1 renders as field/value pairs; the "1 / N" indicator shows; prev is disabled.
2. Clicking next pages to record 2 (values change); the indicator updates.
3. Save the chart, add it to a throwaway dashboard card → it renders and pages inside the card.
Delete the throwaway chart/dashboard afterward. 0 console errors throughout.

- [ ] **Step 3: Push + open PR**

```bash
git push -u origin feat/object-detail-viz
gh pr create --base main --title "feat(chart): Object detail visualization" --body "<summary + live-verify notes>"
```

---

## Self-Review

**Spec coverage:** `ObjectDetail` component with paging/format/empty/reset (Task 1) ✓; viz registration `VizType` + `CHART_TYPES` + `ChartRenderer` route (Task 2) ✓; builder reuse of the column picker + `specForRun` + hydrate + validation (Task 3) ✓; works in dashboard cards (Task 2's `ChartRenderer` route, verified in Task 4) ✓; live verify of builder + card paging (Task 4) ✓. Non-goals (no backend, no Map viz, no per-record drill) respected.

**Placeholder scan:** every code step shows full code; no TBD/TODO.

**Type consistency:** `ObjectDetail({ columns: ResultColumn[], rows: unknown[][], columnSettings?: Record<string, ColumnFormat> })` ↔ `ChartRenderer` passes `columns`/`rows`/`columnSettings` ↔ `formatValue(value, type, fmt)` ↔ `VizType` includes `"object"` ↔ `CHART_TYPES` key `"object"` ↔ `specForRun` object branch returns `columns: tableCols` (raw mode). All consistent; `ResultColumn`/`ColumnFormat`/`formatValue` are imported from their existing modules.
