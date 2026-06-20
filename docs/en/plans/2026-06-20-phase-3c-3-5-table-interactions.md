# Phase 3c-3.5 — Table Interactions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the chart **Table** viz five Metabase-style interactions — column resize, show/hide, sticky header, pin-first-columns, and optional pagination — all TanStack-table-native + CSS, persisted in the chart `params` JSONB.

**Architecture:** `ChartExplore` owns a `tableState` object (like it already owns `columnSettings`), hydrated/saved with the chart. It is passed to BOTH the `ChartTable` renderer (controlled) and a new `TableColumnsSection` in the Display tab; both call `setTableState`. `ChartTable` maps `tableState` onto TanStack's `columnSizing`/`columnVisibility`/`columnPinning`/`pagination` and emits resize changes back. No new dependency, no backend change.

**Tech Stack:** React 18 + Vite + TypeScript + Vitest + @tanstack/react-table (already a dependency) + shadcn/ui.

**Spec:** `docs/en/specs/2026-06-20-phase-3c-3-5-table-interactions-design.md`.

**Branch:** `feat/phase-3c-3-5-table-interactions` (spec committed `f39a14e`). Built on merged main (`3b740c7`).

**Conventions:** plain `git commit` (never `--author`); commit body ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`; jest-dom configured in `vitest.setup.ts`; shadcn primitives in `@/components/ui/*`; tests colocated under `frontend/src/components/chart/__tests__/`.

---

## File Structure
- Modify `frontend/src/api/charts.ts` — add `TableState` interface + `ChartParams.tableState`.
- Create `frontend/src/components/chart/settings/TableColumnsSection.tsx` (+ test) — Display-tab manager for show/hide + freeze + pagination.
- Modify `frontend/src/components/chart/ChartTable.tsx` — controlled resize/visibility/pin/sticky/pagination.
- Create `frontend/src/components/chart/__tests__/ChartTable.test.tsx` — interaction unit tests.
- Modify `frontend/src/routes/explore/ChartExplore.tsx` — `tableState` state + hydrate + reset + persist + Display section + ChartTable props.

---

## Task 1: `TableState` type + `TableColumnsSection`

**Files:**
- Modify: `frontend/src/api/charts.ts`
- Create: `frontend/src/components/chart/settings/TableColumnsSection.tsx`
- Test: `frontend/src/components/chart/__tests__/TableColumnsSection.test.tsx`

- [ ] **Step 1: Add the `TableState` type**

In `frontend/src/api/charts.ts`, add the interface ABOVE `ChartParams` (after the `ChartOrderBy`/`QuerySpec` block), and add the field to `ChartParams`:

```ts
export interface TableState {
  columnWidths?: Record<string, number>;   // colName -> px (manual resizes only)
  hiddenColumns?: string[];                 // colName[] currently hidden
  pinnedLeft?: number;                      // freeze the first N columns (0 = none)
  pagination?: { enabled: boolean; pageSize: number };  // pageSize in {25, 50, 100}
}
```

Then add `tableState?: TableState;` to the `ChartParams` interface (after `conditionalFormatting?: Rule[];`):

```ts
export interface ChartParams {
  spec: QuerySpec;
  rows?: string[];
  cols?: string[];
  metrics?: string[];
  showRowTotals?: boolean;
  showColTotals?: boolean;
  condense?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
  tableState?: TableState;   // table viz only; ignored for pivot
}
```

- [ ] **Step 2: Write the failing `TableColumnsSection` test**

`frontend/src/components/chart/__tests__/TableColumnsSection.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { TableColumnsSection } from "../settings/TableColumnsSection";

const COLS = [{ name: "a", type: "varchar" }, { name: "b", type: "double" }];

describe("TableColumnsSection", () => {
  it("renders a checked checkbox per visible column", () => {
    render(<TableColumnsSection columns={COLS} value={{}} onChange={() => {}} />);
    expect(screen.getByLabelText("a")).toBeChecked();
    expect(screen.getByLabelText("b")).toBeChecked();
  });

  it("emits hiddenColumns when a column is unchecked", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("a"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ hiddenColumns: ["a"] }));
  });

  it("emits pinnedLeft from the freeze input", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Freeze first columns"), { target: { value: "1" } });
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ pinnedLeft: 1 }));
  });

  it("emits pagination enabled with default page size", () => {
    const onChange = vi.fn();
    render(<TableColumnsSection columns={COLS} value={{}} onChange={onChange} />);
    fireEvent.click(screen.getByLabelText("Paginate"));
    expect(onChange).toHaveBeenCalledWith(expect.objectContaining({ pagination: { enabled: true, pageSize: 25 } }));
  });
});
```

- [ ] **Step 3: Run → FAIL.** `cd frontend && npm test -- TableColumnsSection` → fails (module not found).

- [ ] **Step 4: Implement `TableColumnsSection`**

`frontend/src/components/chart/settings/TableColumnsSection.tsx`:

```tsx
import type { TableState } from "@/api/charts";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Input } from "@/components/ui/input";
import {
  Select, SelectTrigger, SelectValue, SelectContent, SelectItem,
} from "@/components/ui/select";

interface ResultColumn { name: string; type: string }

const PAGE_SIZES = [25, 50, 100];

export function TableColumnsSection({
  columns, value, onChange,
}: {
  columns: ResultColumn[];
  value: TableState;
  onChange: (next: TableState) => void;
}) {
  const hidden = new Set(value.hiddenColumns ?? []);
  const pinnedLeft = value.pinnedLeft ?? 0;
  const pagination = value.pagination ?? { enabled: false, pageSize: 25 };

  function toggleHidden(name: string) {
    const next = new Set(hidden);
    if (next.has(name)) next.delete(name); else next.add(name);
    onChange({ ...value, hiddenColumns: [...next] });
  }

  return (
    <div className="space-y-3 text-sm">
      <div className="space-y-1">
        <div className="text-xs font-medium uppercase text-muted-foreground">Visible columns</div>
        <div className="flex flex-col gap-1">
          {columns.map((c) => (
            <div key={c.name} className="flex items-center gap-2">
              <Checkbox id={`tcol-${c.name}`} checked={!hidden.has(c.name)} onCheckedChange={() => toggleHidden(c.name)} />
              <Label htmlFor={`tcol-${c.name}`} className="cursor-pointer text-sm font-normal">{c.name}</Label>
            </div>
          ))}
        </div>
      </div>

      <div className="space-y-1">
        <Label htmlFor="freeze-left" className="text-xs font-medium uppercase text-muted-foreground">Freeze first columns</Label>
        <Input
          id="freeze-left" type="number" min={0} max={columns.length} value={pinnedLeft}
          onChange={(e) => {
            const n = Number(e.target.value);
            onChange({ ...value, pinnedLeft: Number.isNaN(n) ? 0 : Math.max(0, Math.min(n, columns.length)) });
          }}
          className="h-8 text-sm"
        />
      </div>

      <div className="space-y-2">
        <div className="flex items-center gap-2">
          <Checkbox id="paginate" checked={pagination.enabled}
            onCheckedChange={(v) => onChange({ ...value, pagination: { ...pagination, enabled: !!v } })} />
          <Label htmlFor="paginate" className="cursor-pointer text-sm font-normal">Paginate</Label>
        </div>
        {pagination.enabled && (
          <div className="space-y-1">
            <Label className="text-xs font-medium uppercase text-muted-foreground">Page size</Label>
            <Select value={String(pagination.pageSize)} onValueChange={(v) => onChange({ ...value, pagination: { ...pagination, pageSize: Number(v) } })}>
              <SelectTrigger className="h-8 text-sm"><SelectValue /></SelectTrigger>
              <SelectContent>
                {PAGE_SIZES.map((s) => <SelectItem key={s} value={String(s)}>{s}</SelectItem>)}
              </SelectContent>
            </Select>
          </div>
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 5: Run + gates + commit**

`cd frontend && npm test -- TableColumnsSection` → PASS. Then `npx tsc --noEmit` + `npx eslint src/api/charts.ts src/components/chart/settings/TableColumnsSection.tsx src/components/chart/__tests__/TableColumnsSection.test.tsx` → clean.
```bash
git add frontend/src/api/charts.ts frontend/src/components/chart/settings/TableColumnsSection.tsx frontend/src/components/chart/__tests__/TableColumnsSection.test.tsx
git commit -m "feat(web): TableState type + TableColumnsSection (show/hide, freeze, paginate)"
```
(Run git from repo root `/Users/1pro-l4r8/Documents/github/data-portal/egret`.)

---

## Task 2: `ChartTable.tsx` — controlled resize / visibility / pin / sticky / pagination

**Files:**
- Modify (full rewrite): `frontend/src/components/chart/ChartTable.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartTable.test.tsx`

- [ ] **Step 1: Write the failing interaction test**

`frontend/src/components/chart/__tests__/ChartTable.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen, within, fireEvent } from "@testing-library/react";
import { ChartTable } from "../ChartTable";

const COLS = [{ name: "a", type: "varchar" }, { name: "b", type: "double" }];
const mkRows = (n: number) => Array.from({ length: n }, (_, i) => [`a${i}`, i]);

describe("ChartTable interactions", () => {
  it("hides a column listed in hiddenColumns", () => {
    render(<ChartTable columns={COLS} rows={mkRows(3)} tableState={{ hiddenColumns: ["a"] }} />);
    expect(screen.queryByText("a")).not.toBeInTheDocument();
    expect(screen.getByText("b")).toBeInTheDocument();
  });

  it("ignores stale columnWidths keys for columns that no longer exist", () => {
    render(<ChartTable columns={COLS} rows={mkRows(2)} tableState={{ columnWidths: { zzz: 200 } }} />);
    expect(screen.getByText("a")).toBeInTheDocument();
    expect(screen.getByText("b")).toBeInTheDocument();
  });

  it("marks the first column sticky when pinnedLeft=1", () => {
    render(<ChartTable columns={COLS} rows={mkRows(2)} tableState={{ pinnedLeft: 1 }} />);
    const aHeader = screen.getByText("a").closest("th")!;
    expect(aHeader.className).toContain("sticky");
    expect(aHeader.className).toContain("z-30");
  });

  it("paginates to pageSize rows with a working Next button", () => {
    render(<ChartTable columns={COLS} rows={mkRows(30)} tableState={{ pagination: { enabled: true, pageSize: 25 } }} />);
    expect(screen.getByText("Showing 1–25 of 30")).toBeInTheDocument();
    const body = document.querySelector("tbody")!;
    expect(within(body).getAllByRole("row")).toHaveLength(25);
    fireEvent.click(screen.getByText("Next"));
    expect(screen.getByText("Showing 26–30 of 30")).toBeInTheDocument();
  });
});
```

Note for the implementer: the "Showing …" string uses an en-dash `–` (U+2013), not a hyphen — match it exactly in both the component and the test.

- [ ] **Step 2: Run → FAIL.** `cd frontend && npm test -- ChartTable` → fails (props/behavior absent).

- [ ] **Step 3: Replace `ChartTable.tsx` entirely**

Replace the ENTIRE file `frontend/src/components/chart/ChartTable.tsx` with:

```tsx
import { useEffect, useMemo, useState } from "react";
import {
  flexRender, getCoreRowModel, getPaginationRowModel, getSortedRowModel, useReactTable,
  type ColumnDef, type ColumnSizingState, type SortingState,
} from "@tanstack/react-table";
import { formatValue } from "./format";
import { buildConditional } from "./conditional";
import type { ColumnFormat, Rule } from "./format-types";
import type { TableState } from "@/api/charts";
import { Button } from "@/components/ui/button";

export interface ResultColumn { name: string; type: string }

const NUMERIC = /int|double|decimal|real|numeric|float|bigint/i;
const MIN_COL_WIDTH = 60;

export function ChartTable({
  columns, rows, columnSettings = {}, conditionalFormatting = [],
  tableState = {}, onTableStateChange,
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
  tableState?: TableState;
  onTableStateChange?: (next: TableState) => void;
}) {
  const [sorting, setSorting] = useState<SortingState>([]);

  const colIndexByName = useMemo(
    () => Object.fromEntries(columns.map((c, i) => [c.name, i])),
    [columns],
  );
  const cond = useMemo(
    () => buildConditional(rows, colIndexByName, conditionalFormatting),
    [rows, colIndexByName, conditionalFormatting],
  );

  const cols = useMemo<ColumnDef<unknown[]>[]>(
    () =>
      columns.map((c, i) => {
        const isNum = NUMERIC.test(c.type);
        return {
          id: `col_${i}`,
          accessorFn: (row) => {
            const v = (row as unknown[])[i];
            if (isNum && typeof v === "string" && v.trim() !== "" && Number.isFinite(Number(v))) {
              return Number(v);
            }
            return v;
          },
          header: c.name,
          cell: (ctx) => formatValue(ctx.getValue(), c.type, columnSettings[c.name]),
          enableSorting: true,
          minSize: MIN_COL_WIDTH,
          meta: { isNum, name: c.name },
        };
      }),
    [columns, columnSettings],
  );

  // ── controlled table state derived from the persisted tableState (reconcile stale keys) ──
  const columnSizing = useMemo<ColumnSizingState>(() => {
    const out: ColumnSizingState = {};
    columns.forEach((c, i) => {
      const w = tableState.columnWidths?.[c.name];
      if (typeof w === "number") out[`col_${i}`] = w;
    });
    return out;
  }, [columns, tableState.columnWidths]);

  const columnVisibility = useMemo<Record<string, boolean>>(() => {
    const hidden = new Set(tableState.hiddenColumns ?? []);
    const out: Record<string, boolean> = {};
    columns.forEach((c, i) => { if (hidden.has(c.name)) out[`col_${i}`] = false; });
    return out;
  }, [columns, tableState.hiddenColumns]);

  const pinnedCount = Math.max(0, Math.min(tableState.pinnedLeft ?? 0, columns.length));
  const columnPinning = useMemo(
    () => ({ left: Array.from({ length: pinnedCount }, (_, i) => `col_${i}`) }),
    [pinnedCount],
  );

  const paginationEnabled = !!tableState.pagination?.enabled;
  const pageSize = tableState.pagination?.pageSize ?? 25;
  const [pageIndex, setPageIndex] = useState(0);
  useEffect(() => { setPageIndex(0); }, [paginationEnabled, pageSize, rows]);

  const table = useReactTable({
    data: rows,
    columns: cols,
    state: {
      sorting,
      columnSizing,
      columnVisibility,
      columnPinning,
      ...(paginationEnabled ? { pagination: { pageIndex, pageSize } } : {}),
    },
    columnResizeMode: "onChange",
    enableColumnResizing: true,
    onSortingChange: setSorting,
    onColumnSizingChange: (updater) => {
      const next = typeof updater === "function" ? updater(columnSizing) : updater;
      const widths: Record<string, number> = { ...(tableState.columnWidths ?? {}) };
      Object.entries(next).forEach(([id, px]) => {
        const name = columns[Number(id.slice(4))]?.name;  // "col_3" -> 3
        if (name) widths[name] = px as number;
      });
      onTableStateChange?.({ ...tableState, columnWidths: widths });
    },
    onPaginationChange: (updater) => {
      const next = typeof updater === "function" ? updater({ pageIndex, pageSize }) : updater;
      setPageIndex(next.pageIndex);
    },
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    ...(paginationEnabled ? { getPaginationRowModel: getPaginationRowModel() } : {}),
  });

  const totalRows = rows.length;
  const start = paginationEnabled ? pageIndex * pageSize : 0;

  return (
    <div className="rounded-md border">
      <div className="max-h-[60vh] overflow-auto">
        <table className="w-full border-collapse text-sm">
          <thead className="bg-muted/40">
            {table.getHeaderGroups().map((hg) => (
              <tr key={hg.id} className="border-b border-border/60 text-left text-muted-foreground">
                {hg.headers.map((h) => {
                  const meta = h.column.columnDef.meta as { isNum?: boolean; name?: string } | undefined;
                  const isNum = meta?.isNum ?? false;
                  const sized = tableState.columnWidths?.[meta?.name ?? ""] != null;
                  const pinned = h.column.getIsPinned() === "left";
                  return (
                    <th
                      key={h.id}
                      onClick={h.column.getToggleSortingHandler()}
                      style={{
                        ...(sized ? { width: h.getSize() } : {}),
                        ...(pinned ? { left: h.column.getStart("left") } : {}),
                      }}
                      className={`relative sticky top-0 cursor-pointer select-none px-3 py-2 font-medium whitespace-nowrap ${
                        pinned ? "z-30 border-r border-border/60 bg-muted/40" : "z-10"
                      } ${isNum ? "text-right" : "text-left"}`}
                    >
                      {flexRender(h.column.columnDef.header, h.getContext())}
                      {{ asc: " ▲", desc: " ▼" }[h.column.getIsSorted() as string] ?? ""}
                      {h.column.getCanResize() && (
                        <div
                          onMouseDown={h.getResizeHandler()}
                          onTouchStart={h.getResizeHandler()}
                          onClick={(e) => e.stopPropagation()}
                          className="absolute right-0 top-0 h-full w-1 cursor-col-resize touch-none select-none hover:bg-primary/40"
                        />
                      )}
                    </th>
                  );
                })}
              </tr>
            ))}
          </thead>
          <tbody>
            {table.getRowModel().rows.map((r) => {
              const rowColor = cond.rowBg(r.index);
              return (
                <tr key={r.id} className="border-t border-border/60 hover:bg-muted/20" style={rowColor ? { backgroundColor: rowColor } : undefined}>
                  {r.getVisibleCells().map((cell) => {
                    const meta = cell.column.columnDef.meta as { isNum?: boolean; name?: string } | undefined;
                    const isNum = meta?.isNum ?? false;
                    const bg = cond.bg(cell.getValue(), meta?.name ?? cell.column.id);
                    const sized = tableState.columnWidths?.[meta?.name ?? ""] != null;
                    const pinned = cell.column.getIsPinned() === "left";
                    return (
                      <td
                        key={cell.id}
                        style={{
                          ...(sized ? { width: cell.column.getSize() } : {}),
                          ...(pinned ? { left: cell.column.getStart("left") } : {}),
                          ...(bg ? { backgroundColor: bg } : {}),
                        }}
                        className={`px-3 py-2 ${isNum ? "text-right tabular-nums" : ""} ${
                          pinned ? `sticky z-20 border-r border-border/60${bg ? "" : " bg-background"}` : ""
                        }`}
                      >
                        {flexRender(cell.column.columnDef.cell, cell.getContext())}
                      </td>
                    );
                  })}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {paginationEnabled && (
        <div className="flex items-center justify-between border-t border-border/60 px-3 py-2 text-xs text-muted-foreground">
          <span>
            {totalRows === 0 ? "No rows" : `Showing ${start + 1}–${Math.min(start + pageSize, totalRows)} of ${totalRows}`}
          </span>
          <div className="flex items-center gap-2">
            <Button size="sm" variant="outline" className="h-7" disabled={!table.getCanPreviousPage()} onClick={() => table.previousPage()}>Prev</Button>
            <span>Page {pageIndex + 1} / {Math.max(1, table.getPageCount())}</span>
            <Button size="sm" variant="outline" className="h-7" disabled={!table.getCanNextPage()} onClick={() => table.nextPage()}>Next</Button>
          </div>
        </div>
      )}
    </div>
  );
}
```

NOTES for the implementer:
- Width is applied via `style.width` ONLY for columns the user has explicitly resized (`sized`); unsized columns stay auto so the table still fills width (the 3c-3.4 `w-full` behavior is preserved). First drag of an unsized column may "jump" to the TanStack default size — acceptable.
- The `–` in "Showing X–Y of Z" is an en-dash (U+2013); keep it identical to the test.
- Sticky header uses `position: sticky` on each `th` (border-collapse tables don't reliably stick `thead`); pinned cells stack as header `z-30` > pinned body `z-20` > non-pinned header `z-10`.
- `id.slice(4)` strips the `col_` prefix to recover the column index for name mapping.

- [ ] **Step 4: Run → PASS + gates**

`cd frontend && npm test -- ChartTable` → PASS. `npx tsc --noEmit` + `npx eslint src/components/chart/ChartTable.tsx src/components/chart/__tests__/ChartTable.test.tsx` → clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/ChartTable.tsx frontend/src/components/chart/__tests__/ChartTable.test.tsx
git commit -m "feat(web): ChartTable column resize/show-hide/pin/sticky/pagination"
```

---

## Task 3: Wire `tableState` into `ChartExplore` + sweep + smoke + review

**Files:**
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Imports**

In `frontend/src/routes/explore/ChartExplore.tsx`, add `TableState` to the charts type import and import the new section. Change:

```tsx
import {
  useBuildChartData, useChart, useChartMutations,
  type QuerySpec, type VizType, type ChartParams,
} from "@/api/charts";
```
to:
```tsx
import {
  useBuildChartData, useChart, useChartMutations,
  type QuerySpec, type VizType, type ChartParams, type TableState,
} from "@/api/charts";
```
And add after the `ConditionalFormattingSection` import:
```tsx
import { TableColumnsSection } from "@/components/chart/settings/TableColumnsSection";
```

- [ ] **Step 2: Add `tableState` state**

Right after the `const [conditionalFormatting, setConditionalFormatting] = useState<Rule[]>([]);` line, add:
```tsx
  const [tableState, setTableState] = useState<TableState>({});
```

- [ ] **Step 3: Hydrate + reset**

In the saved-chart hydration `useEffect`, after `setConditionalFormatting(p.conditionalFormatting ?? []);` add:
```tsx
    setTableState(p.tableState ?? {});
```
In `resetControls()`, add `setTableState({});` — change:
```tsx
  function resetControls() {
    setTableMetrics([]); setTableCols([]); setPRows([]); setPCols([]); setPVals([]);
    setColumnSettings({}); setConditionalFormatting([]); setQueryId(null);
  }
```
to:
```tsx
  function resetControls() {
    setTableMetrics([]); setTableCols([]); setPRows([]); setPCols([]); setPVals([]);
    setColumnSettings({}); setConditionalFormatting([]); setTableState({}); setQueryId(null);
  }
```

- [ ] **Step 4: Persist in `paramsForSave`**

Change the table branch of `paramsForSave()`:
```tsx
  function paramsForSave(): ChartParams {
    return viz === "pivot_table"
      ? { spec: specForRun(), rows: pRows, cols: pCols, metrics: pVals, showRowTotals, showColTotals, condense, columnSettings, conditionalFormatting }
      : { spec: specForRun(), columnSettings, conditionalFormatting, tableState };
  }
```

- [ ] **Step 5: Render `TableColumnsSection` in the Display tab**

In the Display tab, inside the `resultColumns.length === 0 ? hint : (<>…</>)` non-empty branch, add the section BEFORE the Formatting block (gated on table viz). Change:
```tsx
            ) : (
              <>
                <div className="space-y-1">
                  <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Formatting</div>
                  <FormatSection columns={resultColumns} value={columnSettings} onChange={setColumnSettings} />
                </div>
```
to:
```tsx
            ) : (
              <>
                {viz === "table" && (
                  <div className="space-y-1">
                    <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Columns</div>
                    <TableColumnsSection columns={resultColumns} value={tableState} onChange={setTableState} />
                  </div>
                )}
                <div className="space-y-1">
                  <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">Formatting</div>
                  <FormatSection columns={resultColumns} value={columnSettings} onChange={setColumnSettings} />
                </div>
```

- [ ] **Step 6: Pass `tableState` to `ChartTable`**

Change the table preview render:
```tsx
        {finished && results.data && viz !== "pivot_table" && (
          <ChartTable columns={results.data.columns} rows={results.data.rows}
            columnSettings={columnSettings} conditionalFormatting={conditionalFormatting} />
        )}
```
to:
```tsx
        {finished && results.data && viz !== "pivot_table" && (
          <ChartTable columns={results.data.columns} rows={results.data.rows}
            columnSettings={columnSettings} conditionalFormatting={conditionalFormatting}
            tableState={tableState} onTableStateChange={setTableState} />
        )}
```

- [ ] **Step 7: Sweep**

`cd frontend && npm run typecheck && npm run lint && npm run build && npm test`
Expected: 0 type errors; lint clean; build ok; all tests pass (existing + the two new test files).

- [ ] **Step 8: Commit**

```bash
git add frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(web): wire tableState (persist/hydrate) + Columns section into chart Explore"
```

- [ ] **Step 9: Manual Playwright smoke** (vite HMR; backend unchanged — no migration)

Login `admin@egret.local` / `Admin12345!`, ws `eng`. New chart → pick **Table** → dataset `orders`, select columns `orderstatus` + `orderpriority` + a metric `revenue` (so there are ≥3 columns) → **Run**:
1. **Resize**: drag a column header's right edge → width changes; **Save** → reopen → width restored.
2. **Show/hide**: Display tab → uncheck a column → it disappears from the preview; persists across Save/reopen.
3. **Freeze**: set "Freeze first columns" = 1 → horizontally scroll (narrow the window or add columns) → the first column stays put.
4. **Pagination**: enable Paginate (page size 25) → footer shows "Showing 1–25 of N" + Prev/Next work; disabling returns to full scroll.
5. **Sticky header**: with enough rows, vertically scroll the preview → the header stays visible.

- [ ] **Step 10: Final review**

Dispatch a final reviewer over the phase `git diff` (base = the spec commit `f39a14e`). Focus: the controlled `tableState` round-trip (hydrate → edit → persist → reload), reconciliation of stale keys when columns change, the id↔name mapping in `onColumnSizingChange`, z-index correctness for pinned + sticky header, that pivot is untouched, and no backend/param-shape regressions (pivot `paramsForSave` branch unchanged).

- [ ] **Step 11: Final commit (if fixes)**

```bash
git add -A && git commit -m "test: phase 3c-3.5 sweep + manual smoke fixes"
```

Then use **superpowers:finishing-a-development-branch**.

---

## Self-Review notes (plan author)
- **Spec coverage:** TableState type + persistence (T1, charts.ts); show/hide + freeze + pagination controls (T1, TableColumnsSection); resize + visibility + pin + sticky + pagination renderer (T2, ChartTable); hydrate/reset/persist + Display section + ChartTable props (T3, ChartExplore); reconciliation of stale keys (T2 `columnSizing`/`columnVisibility`/`pinnedCount` derivations + T2 test); sweep + manual smoke (all five interactions) + final review (T3).
- **Type consistency:** `TableState` shape (`columnWidths`/`hiddenColumns`/`pinnedLeft`/`pagination{enabled,pageSize}`) is identical across charts.ts, TableColumnsSection props, ChartTable derivations, and ChartExplore wiring. `ResultColumn {name,type}` matches the existing local interface used by FormatSection and ChartTable. `onTableStateChange`/`onChange` signatures match call sites.
- **No backend/param-shape break:** pivot `paramsForSave` branch unchanged; `tableState` is additive and JSONB-carried; hydration tolerates its absence (`?? {}`).
- **Scope:** single frontend plan, Table-viz only; pivot, drag-reorder, totals-footer explicitly out (per spec).
