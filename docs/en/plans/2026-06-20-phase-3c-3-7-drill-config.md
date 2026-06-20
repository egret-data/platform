# Phase 3c-3.7 — Drill-through + Pivot Full-Width + Config Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add cell drill-through (Filter-by + See-records) to Table + Pivot, make the pivot fill width, and redesign the config panel (Data + Display) Metabase-style — all frontend-only.

**Architecture:** A pure `drill.ts` filter-builder (TDD) feeds a `DrillMenu` (floating menu at the click point) rendered by `ChartExplore`; "See records" opens a `DrillRecordsDialog` running a raw query via the existing query hooks; a `FilterChips` bar shows/removes active `spec.filters`. The pivot gains `w-full`. A reusable `FieldZone` (dnd-kit chips) replaces `OrderedPicker`, and the Display tab gets shadcn `Switch` toggles + consistent section headers. No backend change — `chartquery::build` already renders `filters` and has a raw-records mode.

**Tech Stack:** React 18 + Vite + TS + Vitest + @tanstack/react-table + @dnd-kit + shadcn/ui (`dialog`/`popover`/`switch`/`dropdown-menu` all already present).

**Spec:** `docs/en/specs/2026-06-20-phase-3c-3-7-drill-config-design.md`.

**Branch:** `feat/phase-3c-3-7-drill-config` (spec committed `fa5946a`). Built on merged main (`e3a9b4e`).

**Conventions:** plain `git commit` (never `--author`); commit body ends `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`; tests under `__tests__/`; npm from `frontend/`, git from repo root; never `git add -A` (gitignored `.claude/` worktree artifact stays untracked). Two tasks (5 pivot-width, 7 Display restyle) are VISUAL — verify by manual screenshot, tune live; don't block on pixel-exactness.

---

## File Structure
- Create `frontend/src/components/chart/drill.ts` (+ test) — pure filter builder.
- Create `frontend/src/components/chart/DrillMenu.tsx` (+ test) — floating action menu.
- Create `frontend/src/components/chart/FilterChips.tsx` (+ test) — active-filter chips bar.
- Create `frontend/src/components/chart/DrillRecordsDialog.tsx` — raw-rows modal.
- Create `frontend/src/components/chart/config/FieldZone.tsx` (+ test) — Metabase-style field chips zone.
- Modify `frontend/src/components/chart/ChartTable.tsx` — cell-drill onClick + `dimensionColumns` prop.
- Modify `frontend/src/components/chart/pivot/PivotTable.tsx` — cell-drill onClick + `w-full`.
- Modify `frontend/src/routes/explore/ChartExplore.tsx` — filters state, drill menu/dialog/chips, FieldZone swap, Display restyle.
- Modify `frontend/src/components/chart/settings/{FormatSection,ConditionalFormattingSection,TableColumnsSection}.tsx` — Display styling (Switch + section headers).

---

## Task 1: Pure drill filter-builder

**Files:**
- Create: `frontend/src/components/chart/drill.ts`
- Test: `frontend/src/components/chart/__tests__/drill.test.ts`

- [ ] **Step 1: Write the failing test**

`frontend/src/components/chart/__tests__/drill.test.ts`:
```ts
import { describe, it, expect } from "vitest";
import { pivotCellDrill, tableCellDrill, drillFilters } from "../drill";

describe("drill filter-builder", () => {
  it("pivotCellDrill: leaf cell yields all row+col dims, skipping nulls", () => {
    const ctx = pivotCellDrill(["region", "seg"], ["EU", "SMB"], ["year"], ["2024"]);
    expect(ctx.dims).toEqual([
      { col: "region", value: "EU" }, { col: "seg", value: "SMB" }, { col: "year", value: "2024" },
    ]);
  });
  it("pivotCellDrill: subtotal (short path) yields partial dims; grand yields none", () => {
    expect(pivotCellDrill(["region", "seg"], ["EU"], [], []).dims).toEqual([{ col: "region", value: "EU" }]);
    expect(pivotCellDrill(["region"], [null], [], []).dims).toEqual([]);
    expect(pivotCellDrill(["region"], [], [], []).dims).toEqual([]);
  });
  it("tableCellDrill: only dimension columns, nulls skipped", () => {
    const ctx = tableCellDrill(["region", "seg"], ["region", "seg", "revenue"], ["EU", null, 100]);
    expect(ctx.dims).toEqual([{ col: "region", value: "EU" }]);
  });
  it("drillFilters: one eq filter per dim", () => {
    expect(drillFilters({ dims: [{ col: "region", value: "EU" }] })).toEqual([
      { col: "region", op: "eq", value: "EU" },
    ]);
  });
});
```

- [ ] **Step 2: Run → FAIL.** `cd frontend && npm test -- drill` → module not found.

- [ ] **Step 3: Implement `drill.ts`**
```ts
import type { ChartFilter } from "@/api/charts";

export interface DrillContext { dims: { col: string; value: unknown }[] }

/** Pivot value cell → the dims of its (rowPath, colPath), skipping null/short segments. */
export function pivotCellDrill(
  rowDims: string[], rowPath: (string | null)[],
  colDims: string[], colPath: (string | null)[],
): DrillContext {
  const dims: { col: string; value: unknown }[] = [];
  rowDims.forEach((c, i) => { if (i < rowPath.length && rowPath[i] != null) dims.push({ col: c, value: rowPath[i] }); });
  colDims.forEach((c, i) => { if (i < colPath.length && colPath[i] != null) dims.push({ col: c, value: colPath[i] }); });
  return { dims };
}

/** Aggregated-table value cell → the dims (dimension columns) of its row. */
export function tableCellDrill(
  dimensionColumns: string[], columns: string[], row: unknown[],
): DrillContext {
  const dims: { col: string; value: unknown }[] = [];
  for (const c of dimensionColumns) {
    const i = columns.indexOf(c);
    if (i >= 0 && row[i] != null) dims.push({ col: c, value: row[i] });
  }
  return { dims };
}

/** Build `=` filters from a drill context. */
export function drillFilters(ctx: DrillContext): ChartFilter[] {
  return ctx.dims.map((d) => ({ col: d.col, op: "eq" as const, value: d.value }));
}
```

- [ ] **Step 4: Run → PASS + gates.** `npm test -- drill`; `npx tsc --noEmit`; `npx eslint src/components/chart/drill.ts src/components/chart/__tests__/drill.test.ts`.

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/chart/drill.ts frontend/src/components/chart/__tests__/drill.test.ts
git commit -m "feat(web): pure drill filter-builder (pivot/table cell → eq filters)"
```

---

## Task 2: FilterChips + DrillMenu

**Files:**
- Create: `frontend/src/components/chart/FilterChips.tsx`, `frontend/src/components/chart/DrillMenu.tsx`
- Test: `frontend/src/components/chart/__tests__/FilterChips.test.tsx`, `frontend/src/components/chart/__tests__/DrillMenu.test.tsx`

- [ ] **Step 1: Write failing tests**

`__tests__/FilterChips.test.tsx`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FilterChips } from "../FilterChips";

describe("FilterChips", () => {
  it("renders a chip per filter and removes on ×", () => {
    const onRemove = vi.fn();
    render(<FilterChips filters={[{ col: "region", op: "eq", value: "EU" }]} onRemove={onRemove} />);
    expect(screen.getByText(/region/)).toBeInTheDocument();
    expect(screen.getByText(/EU/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /remove filter/i }));
    expect(onRemove).toHaveBeenCalledWith(0);
  });
  it("renders nothing when empty", () => {
    const { container } = render(<FilterChips filters={[]} onRemove={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });
});
```
`__tests__/DrillMenu.test.tsx`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DrillMenu } from "../DrillMenu";

const ctx = { dims: [{ col: "region", value: "EU" }] };

describe("DrillMenu", () => {
  it("shows a Filter-by item per dim and See-records, fires callbacks", () => {
    const onFilterBy = vi.fn(); const onSeeRecords = vi.fn(); const onClose = vi.fn();
    render(<DrillMenu ctx={ctx} x={10} y={10} onFilterBy={onFilterBy} onSeeRecords={onSeeRecords} onClose={onClose} />);
    fireEvent.click(screen.getByText(/Filter by region is EU/i));
    expect(onFilterBy).toHaveBeenCalledWith({ col: "region", op: "eq", value: "EU" });
    fireEvent.click(screen.getByText(/See these records/i));
    expect(onSeeRecords).toHaveBeenCalledWith([{ col: "region", op: "eq", value: "EU" }]);
  });
  it("with no dims shows only See-records", () => {
    render(<DrillMenu ctx={{ dims: [] }} x={0} y={0} onFilterBy={() => {}} onSeeRecords={() => {}} onClose={() => {}} />);
    expect(screen.queryByText(/Filter by/i)).not.toBeInTheDocument();
    expect(screen.getByText(/See these records/i)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run → FAIL.** `npm test -- "FilterChips|DrillMenu"`.

- [ ] **Step 3: Implement `FilterChips.tsx`**
```tsx
import type { ChartFilter } from "@/api/charts";

export function FilterChips({
  filters, onRemove,
}: { filters: ChartFilter[]; onRemove: (index: number) => void }) {
  if (filters.length === 0) return null;
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      {filters.map((f, i) => (
        <span key={i} className="inline-flex items-center gap-1 rounded-full border bg-muted/50 px-2.5 py-0.5 text-xs">
          <span className="font-medium">{f.col}</span>
          <span className="text-muted-foreground">{"value" in f ? `= ${String(f.value)}` : f.op}</span>
          <button type="button" aria-label="remove filter" className="ml-0.5 text-muted-foreground hover:text-destructive"
            onClick={() => onRemove(i)}>×</button>
        </span>
      ))}
    </div>
  );
}
```

- [ ] **Step 4: Implement `DrillMenu.tsx`** (a floating menu at the click point + a click-away backdrop; no positioning lib needed)
```tsx
import type { ChartFilter } from "@/api/charts";
import { drillFilters, type DrillContext } from "./drill";

export function DrillMenu({
  ctx, x, y, onFilterBy, onSeeRecords, onClose,
}: {
  ctx: DrillContext; x: number; y: number;
  onFilterBy: (filter: ChartFilter) => void;
  onSeeRecords: (filters: ChartFilter[]) => void;
  onClose: () => void;
}) {
  return (
    <>
      <button type="button" aria-label="close menu" className="fixed inset-0 z-40 cursor-default" onClick={onClose} />
      <div
        className="fixed z-50 min-w-48 overflow-hidden rounded-md border bg-popover p-1 text-sm shadow-md"
        style={{ left: x, top: y }}
      >
        {ctx.dims.map((d, i) => (
          <button
            key={i}
            type="button"
            className="block w-full rounded px-2 py-1.5 text-left hover:bg-muted"
            onClick={() => { onFilterBy({ col: d.col, op: "eq", value: d.value }); onClose(); }}
          >
            Filter by {d.col} is {String(d.value)}
          </button>
        ))}
        {ctx.dims.length > 0 && <div className="my-1 border-t" />}
        <button
          type="button"
          className="block w-full rounded px-2 py-1.5 text-left hover:bg-muted"
          onClick={() => { onSeeRecords(drillFilters(ctx)); onClose(); }}
        >
          See these records
        </button>
      </div>
    </>
  );
}
```

- [ ] **Step 5: Run → PASS + gates** (`npm test -- "FilterChips|DrillMenu"`, tsc, eslint on the 4 files).

- [ ] **Step 6: Commit**
```bash
git add frontend/src/components/chart/FilterChips.tsx frontend/src/components/chart/DrillMenu.tsx frontend/src/components/chart/__tests__/FilterChips.test.tsx frontend/src/components/chart/__tests__/DrillMenu.test.tsx
git commit -m "feat(web): FilterChips bar + DrillMenu (filter-by / see-records)"
```

---

## Task 3: DrillRecordsDialog (raw rows modal)

**Files:**
- Create: `frontend/src/components/chart/DrillRecordsDialog.tsx`

(No unit test — it's an integration wrapper around query hooks; covered by manual smoke. The hooks are: `useBuildChartData(ws)` → `.mutateAsync({dataset_id, query_spec})` → `{sql, database_id}`; `useSubmitQuery(ws)` → `.mutateAsync({database_id, sql, limit})` → `{id}`; `useQueryStatus(ws, id)`; `useResults(ws, queryId, 0, enabled)`.)

- [ ] **Step 1: Implement `DrillRecordsDialog.tsx`**
```tsx
import { useEffect, useState } from "react";
import { useBuildChartData, type ChartFilter, type QuerySpec } from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";
import { ChartTable, type ResultColumn } from "./ChartTable";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";

export function DrillRecordsDialog({
  open, onOpenChange, ws, datasetId, columns, filters, rowLimit = 1000,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  ws: string;
  datasetId: string | null;
  columns: ResultColumn[];
  filters: ChartFilter[];
  rowLimit?: number;
}) {
  const build = useBuildChartData(ws);
  const submit = useSubmitQuery(ws);
  const [queryId, setQueryId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const status = useQueryStatus(ws, queryId);
  const finished = status.data?.status === "finished";
  const results = useResults(ws, queryId, 0, finished);

  useEffect(() => {
    if (!open || !datasetId) return;
    setQueryId(null);
    setError(null);
    const spec: QuerySpec = {
      metrics: [], dimensions: [], columns: columns.map((c) => c.name),
      filters, orderby: [], row_limit: rowLimit, rollup_groups: [],
    };
    (async () => {
      try {
        const { sql, database_id } = await build.mutateAsync({ dataset_id: datasetId, query_spec: spec });
        const ack = await submit.mutateAsync({ database_id, sql, limit: rowLimit });
        setQueryId(ack.id);
      } catch (e) { setError(String(e)); }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const summary = filters.length
    ? filters.map((f) => ("value" in f ? `${f.col} = ${String(f.value)}` : `${f.col} ${f.op}`)).join(", ")
    : "all rows";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-4xl">
        <DialogHeader><DialogTitle>Records where {summary}</DialogTitle></DialogHeader>
        <div className="max-h-[70vh] overflow-auto">
          {error && <p className="rounded bg-destructive/10 p-3 text-sm text-destructive">{error}</p>}
          {status.data?.status === "failed" && <p className="rounded bg-destructive/10 p-3 text-sm text-destructive">{status.data.error}</p>}
          {!error && !finished && <p className="text-sm text-muted-foreground">Loading…</p>}
          {finished && results.data && (
            results.data.rows.length === 0
              ? <p className="text-sm text-muted-foreground">No matching records.</p>
              : <ChartTable columns={results.data.columns} rows={results.data.rows} />
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
```
NOTE: ChartTable's only required props are `columns` + `rows` (the rest default) — verify this against the current ChartTable signature; if `columns`/`rows` are not the first two required props, adapt. The `eslint-disable` is intentional (run only when `open` flips).

- [ ] **Step 2: Gates.** `npx tsc --noEmit`; `npx eslint src/components/chart/DrillRecordsDialog.tsx`; `npm run build`.

- [ ] **Step 3: Commit**
```bash
git add frontend/src/components/chart/DrillRecordsDialog.tsx
git commit -m "feat(web): DrillRecordsDialog — raw-rows modal for see-records"
```

---

## Task 4: Wire drill into renderers + ChartExplore

**Files:**
- Modify: `frontend/src/components/chart/ChartTable.tsx`, `frontend/src/components/chart/pivot/PivotTable.tsx`, `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: ChartTable — add drill props + cell onClick**

In the `ChartTable` props type add (after `onTableStateChange?`):
```tsx
  dimensionColumns?: string[];
  onCellDrill?: (ctx: import("./drill").DrillContext, x: number, y: number) => void;
```
Import at top: `import { tableCellDrill } from "./drill";`
On the value `<td>` (the one rendering `flexRender(cell.column.columnDef.cell, …)`), add an `onClick` that fires drill when `onCellDrill` + `dimensionColumns` are present and the row is aggregated. Add to the `<td>`:
```tsx
                        onClick={onCellDrill && dimensionColumns && dimensionColumns.length > 0
                          ? (e) => onCellDrill(
                              tableCellDrill(dimensionColumns, columns.map((c) => c.name), r.original as unknown[]),
                              e.clientX, e.clientY,
                            )
                          : undefined}
```
and append `${onCellDrill && dimensionColumns?.length ? "cursor-pointer" : ""}` to the `<td>` className. (`r` is the row; use `r.original` — the TanStack row's source array.)

- [ ] **Step 2: PivotTable — add drill props + cell onClick**

In `PivotTable` props add (after `onCycleSort?`):
```tsx
  onCellDrill?: (ctx: import("./drill").DrillContext, x: number, y: number) => void;
```
Import: `import { pivotCellDrill } from "../drill";`
On the value `<td>` (inside `br.values.map`), add `onClick` using the row path + the colLeaf path:
```tsx
                    onClick={onCellDrill && br.kind === "leaf"
                      ? (e) => onCellDrill(
                          pivotCellDrill(input.rowDims, br.rowPath, input.colDims, cl.colPath),
                          e.clientX, e.clientY,
                        )
                      : undefined}
```
and append `${onCellDrill && br.kind === "leaf" ? "cursor-pointer" : ""}` to that `<td>` className. (`cl` is `p.colLeaves[vi]`, already in scope; `br.rowPath` and `input.rowDims`/`input.colDims` are available.)

- [ ] **Step 3: ChartExplore — filters state + specForRun + menu/dialog/chips**

(a) Imports:
```tsx
import { FilterChips } from "@/components/chart/FilterChips";
import { DrillMenu } from "@/components/chart/DrillMenu";
import { DrillRecordsDialog } from "@/components/chart/DrillRecordsDialog";
import type { DrillContext } from "@/components/chart/drill";
import type { ChartFilter } from "@/api/charts";
```
(b) State (after `tableState`):
```tsx
  const [filters, setFilters] = useState<ChartFilter[]>([]);
  const [drillMenu, setDrillMenu] = useState<{ ctx: DrillContext; x: number; y: number } | null>(null);
  const [recordsFilters, setRecordsFilters] = useState<ChartFilter[] | null>(null);
```
(c) Hydrate (after `setTableState(...)`): `setFilters(p.spec?.filters ?? []);`
(d) Reset (in `resetControls`): add `setFilters([]);`
(e) `specForRun` — replace BOTH `filters: []` occurrences with `filters,` (the pivot branch and the table branch).
(f) Drill handlers (near `run`):
```tsx
  function addFilter(f: ChartFilter) {
    setFilters((prev) => [...prev.filter((x) => x.col !== f.col), f]);
  }
  // re-run whenever filters change (after the chart has been run once)
  useEffect(() => { if (queryId) void run(); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [filters]);
```
(g) The drill menu + dialog + chips JSX. Add the chips bar above the preview blocks (inside the main area, before the failed/running blocks):
```tsx
        {filters.length > 0 && (
          <FilterChips filters={filters} onRemove={(i) => setFilters((prev) => prev.filter((_, j) => j !== i))} />
        )}
```
Pass `onCellDrill` to both renderers: add to the PivotTable props
```tsx
            onCellDrill={(ctx, x, y) => setDrillMenu({ ctx, x, y })}
```
and to the ChartTable props
```tsx
            dimensionColumns={tableMetrics.length > 0 ? tableCols : []}
            onCellDrill={(ctx, x, y) => setDrillMenu({ ctx, x, y })}
```
(import `tableCellDrill` is not needed here; `tableCols`/`tableMetrics` are in scope). At the end of the component (next to the dialogs) render:
```tsx
      {drillMenu && (
        <DrillMenu
          ctx={drillMenu.ctx} x={drillMenu.x} y={drillMenu.y}
          onFilterBy={addFilter}
          onSeeRecords={(fs) => setRecordsFilters(fs)}
          onClose={() => setDrillMenu(null)}
        />
      )}
      <DrillRecordsDialog
        open={recordsFilters !== null}
        onOpenChange={(o) => { if (!o) setRecordsFilters(null); }}
        ws={ws} datasetId={datasetId}
        columns={(detail.data?.columns ?? []).map((c) => ({ name: c.column_name, type: c.data_type ?? "varchar" }))}
        filters={recordsFilters ?? []}
        rowLimit={rowLimit}
      />
```
(Verify the dataset column shape: `detail.data.columns[i]` has `.column_name` + `.data_type` — adapt the map if the field names differ.)

- [ ] **Step 4: Gates.** `cd frontend && npm run typecheck && npm run lint && npm run build && npm test` → green (existing suite unaffected; drill wiring compiles).

- [ ] **Step 5: Commit**
```bash
git add frontend/src/components/chart/ChartTable.tsx frontend/src/components/chart/pivot/PivotTable.tsx frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(web): wire cell drill-through (menu + filter-by + see-records) into Explore"
```

---

## Task 5: Pivot full-width (VISUAL — tune live)

**Files:** Modify `frontend/src/components/chart/pivot/PivotTable.tsx`

- [ ] **Step 1: Make the table fill width**
- Change the `<table>` className from `"border-collapse text-sm"` to `"w-full border-collapse text-sm"`.
- On the row-header cells (the `<th scope="row">` body cells AND the row-dim-name/corner header `<th>`s), append `w-px` so they shrink to content and the slack flows to the value columns. (Add `w-px` to those three `<th>` className strings; keep their existing `whitespace-nowrap`.)

- [ ] **Step 2: Verify visually + tune**
Start the dev stack if needed; render a pivot (2 row dims + 1 value). Screenshot. The table should fill the preview width with value columns absorbing the slack (numbers right-aligned). If it looks sparse/ugly (big gaps), fall back: instead of `w-px` on row-headers, append a trailing empty `<th aria-hidden className="w-full" />` spacer to each header row and an empty `<td>` to body rows so the leftover width collects at the far right. Pick whichever looks better.

- [ ] **Step 3: Gates + commit.** `npm run typecheck && npm run lint && npm test` (renderer tests still pass — the change is class-only). Then:
```bash
git add frontend/src/components/chart/pivot/PivotTable.tsx
git commit -m "style(web): pivot table fills full preview width"
```

---

## Task 6: FieldZone — Metabase-style field chips (Data tab)

**Files:**
- Create: `frontend/src/components/chart/config/FieldZone.tsx`
- Test: `frontend/src/components/chart/__tests__/FieldZone.test.tsx`
- Modify: `frontend/src/routes/explore/ChartExplore.tsx` (swap `OrderedPicker` → `FieldZone` for the pivot)

- [ ] **Step 1: Write the failing test**
`__tests__/FieldZone.test.tsx`:
```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FieldZone } from "../config/FieldZone";

describe("FieldZone", () => {
  it("renders a chip per selected field and removes on ×", () => {
    const onChange = vi.fn();
    render(<FieldZone label="Rows" selected={["a", "b"]} options={["a", "b", "c"]} onChange={onChange} />);
    expect(screen.getByText("a")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: /remove/i })[0]);
    expect(onChange).toHaveBeenCalledWith(["b"]);
  });
  it("adds a field via the + Add affordance", () => {
    const onChange = vi.fn();
    render(<FieldZone label="Rows" selected={["a"]} options={["a", "b", "c"]} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: /add rows/i }));
    fireEvent.click(screen.getByText("b"));
    expect(onChange).toHaveBeenCalledWith(["a", "b"]);
  });
});
```

- [ ] **Step 2: Run → FAIL.** `npm test -- FieldZone`.

- [ ] **Step 3: Implement `config/FieldZone.tsx`** (chips with dnd-kit reorder within the zone + a "+ Add" popover of remaining options)
```tsx
import {
  DndContext, PointerSensor, useSensor, useSensors, closestCenter, type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext, verticalListSortingStrategy, useSortable, arrayMove,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical, Plus } from "lucide-react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

function Chip({ id, onRemove }: { id: string; onRemove: () => void }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  return (
    <div
      ref={setNodeRef}
      style={{ transform: CSS.Translate.toString(transform), transition, opacity: isDragging ? 0.6 : 1 }}
      className="flex items-center gap-1.5 rounded-md border bg-muted/40 px-2 py-1 text-sm"
    >
      <button type="button" className="cursor-grab text-muted-foreground" {...attributes} {...listeners} aria-label="drag">
        <GripVertical className="h-3.5 w-3.5" />
      </button>
      <span className="flex-1">{id}</span>
      <button type="button" aria-label={`remove ${id}`} className="text-muted-foreground hover:text-destructive" onClick={onRemove}>×</button>
    </div>
  );
}

export function FieldZone({
  label, selected, options, onChange,
}: { label: string; selected: string[]; options: string[]; onChange: (v: string[]) => void }) {
  const remaining = options.filter((o) => !selected.includes(o));
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }));
  function onDragEnd(e: DragEndEvent) {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const from = selected.indexOf(String(active.id));
    const to = selected.indexOf(String(over.id));
    if (from < 0 || to < 0) return;
    onChange(arrayMove(selected, from, to));
  }
  return (
    <div className="space-y-1.5">
      <div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">{label}</div>
      <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
        <SortableContext items={selected} strategy={verticalListSortingStrategy}>
          <div className="flex flex-col gap-1">
            {selected.map((s) => (
              <Chip key={s} id={s} onRemove={() => onChange(selected.filter((x) => x !== s))} />
            ))}
          </div>
        </SortableContext>
      </DndContext>
      {remaining.length > 0 && (
        <Popover>
          <PopoverTrigger asChild>
            <button type="button" aria-label={`add ${label.toLowerCase()}`}
              className="flex w-full items-center gap-1 rounded-md border border-dashed px-2 py-1 text-xs text-muted-foreground hover:bg-muted/40">
              <Plus className="h-3.5 w-3.5" /> Add {label.toLowerCase()}…
            </button>
          </PopoverTrigger>
          <PopoverContent align="start" className="w-48 p-1">
            <div className="flex flex-col">
              {remaining.map((o) => (
                <button key={o} type="button" className="rounded px-2 py-1.5 text-left text-sm hover:bg-muted"
                  onClick={() => onChange([...selected, o])}>{o}</button>
              ))}
            </div>
          </PopoverContent>
        </Popover>
      )}
    </div>
  );
}
```
(Verify `lucide-react` exports `GripVertical` + `Plus`, and `@/components/ui/popover` exports `Popover`/`PopoverTrigger`/`PopoverContent` — both confirmed present. The PopoverContent stays open across the option click; that's fine, or wrap each option to also close — keep simple.)

- [ ] **Step 4: Run → PASS.** `npm test -- FieldZone`.

- [ ] **Step 5: Swap into ChartExplore Data tab.** Replace the three pivot `OrderedPicker` usages (lines ~301-306) with `FieldZone` (same props shape: `label`/`selected`/`onChange`/`options`):
```tsx
                <FieldZone label="Rows" selected={pRows} onChange={setPRows}
                  options={columns.map((c) => c.column_name).filter((n) => !pCols.includes(n))} />
                <FieldZone label="Columns" selected={pCols} onChange={setPCols}
                  options={columns.map((c) => c.column_name).filter((n) => !pRows.includes(n))} />
                <FieldZone label="Values" selected={pVals} onChange={setPVals}
                  options={metrics.map((m) => m.metric_name)} />
```
Add `import { FieldZone } from "@/components/chart/config/FieldZone";` and remove the now-unused `OrderedPicker` function + its `Select` import usage if nothing else uses it (the table controls still use `Select` for the dataset picker — keep the import; just delete the `OrderedPicker` function).

- [ ] **Step 6: Gates + commit.** `npm run typecheck && npm run lint && npm run build && npm test`.
```bash
git add frontend/src/components/chart/config/FieldZone.tsx frontend/src/components/chart/__tests__/FieldZone.test.tsx frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(web): FieldZone (Metabase-style draggable field chips) for pivot Data tab"
```

---

## Task 7: Display-tab restyle (VISUAL — Switch + section headers; tune live)

**Files:** Modify `frontend/src/routes/explore/ChartExplore.tsx` and `frontend/src/components/chart/settings/{TableColumnsSection,FormatSection,ConditionalFormattingSection}.tsx`

- [ ] **Step 1: Switch toggles in the Display tab (ChartExplore + TableColumnsSection).**
Import `import { Switch } from "@/components/ui/switch";` in both files. Replace the pivot totals `Checkbox`+`Label` rows in ChartExplore's Display tab and the Paginate `Checkbox` in `TableColumnsSection` with a consistent row:
```tsx
<div className="flex items-center justify-between">
  <Label htmlFor="show-row-totals" className="text-sm font-normal">Row totals</Label>
  <Switch id="show-row-totals" checked={showRowTotals} onCheckedChange={(v) => setShowRowTotals(v)} />
</div>
```
(repeat for Column totals / Condense / Paginate, each with its own id + state setter). Verify `ui/switch.tsx` exports `Switch` with `checked`/`onCheckedChange`.

- [ ] **Step 2: Consistent section headers.** Factor the repeated `<div className="text-xs font-medium uppercase tracking-wide text-muted-foreground">…</div>` into a small local `SectionLabel` (in ChartExplore) used for every Display group (Totals / Columns / Formatting / Conditional), and add a subtle divider (`border-b pb-1`) for visual separation. Keep wording identical.

- [ ] **Step 3: Restyle the Format column-selector + Conditional rule cards** toward Metabase's ColumnItem look (field name rows with a subtle hover, rule rows as bordered cards with a color swatch + ×). This is presentational only inside `FormatSection.tsx` / `ConditionalFormattingSection.tsx` — DO NOT change their props or behavior (the existing section/renderer/conditional tests must stay green). Tune the spacing/borders live with a screenshot.

- [ ] **Step 4: Verify visually.** Screenshot the Display tab for both viz types — toggles are switches, section headers consistent with dividers, format/conditional sections look cleaner. Iterate.

- [ ] **Step 5: Gates + commit.** `npm run typecheck && npm run lint && npm run build && npm test` (all existing tests green — behavior unchanged).
```bash
git add frontend/src/routes/explore/ChartExplore.tsx frontend/src/components/chart/settings/TableColumnsSection.tsx frontend/src/components/chart/settings/FormatSection.tsx frontend/src/components/chart/settings/ConditionalFormattingSection.tsx
git commit -m "style(web): Metabase-style Display tab (switch toggles, consistent section headers, cleaner format/conditional)"
```

---

## Task 8: Sweep + manual smoke + final review

- [ ] **Step 1: Sweep.** `cd frontend && npm run typecheck && npm run lint && npm run build && npm test` → all green.

- [ ] **Step 2: Manual Playwright smoke** (backend unchanged — no migration). Login `admin@egret.local` / `Admin12345!`, ws `eng`.
  1. **Drill (pivot):** new Pivot → orders, Rows = orderpriority + orderstatus, Values = revenue → Run → click a leaf value cell → menu shows "Filter by orderpriority is …", "Filter by orderstatus is …", "See these records". Click a Filter-by → a chip appears above the preview + the pivot narrows. Remove the chip → restores.
  2. **See records:** click a cell → "See these records" → a modal opens with the raw rows matching that cell's dims; close it.
  3. **Drill (table):** switch to Table (aggregated: a metric + a dimension column) → click a cell → Filter-by / See-records work.
  4. **Pivot full-width:** the pivot fills the preview width (no big empty bordered area).
  5. **Config UX:** Data tab Rows/Columns/Values are draggable chips with "+ Add"; Display tab toggles are switches with consistent section headers.
  6. **Persist:** Save → reopen → the active filters hydrate (chips reappear, pivot still filtered).

- [ ] **Step 3: Final review.** Dispatch a final reviewer over the phase `git diff` (base = spec commit `fa5946a`). Focus: drill filter construction (pivot leaf vs subtotal vs grand; table dimension-only), the filters→specForRun→re-run loop (no infinite loop — the `useEffect([filters])` only re-runs when a query already exists), param isolation (filters live in `spec.filters`, persisted both branches; pivot/table param branches otherwise unchanged), the DrillRecordsDialog query lifecycle (runs on open, disposes on close), `sql_literal` safety is unchanged (drill builds `eq` filters with real values — the backend encodes them, no new injection surface), and that the FieldZone/Display restyle changed no behavior (all prior tests green).

- [ ] **Step 4: Final commit (if fixes).**
```bash
git add -A && git commit -m "test: phase 3c-3.7 sweep + manual smoke fixes"
```
Then use **superpowers:finishing-a-development-branch**.

---

## Self-Review notes (plan author)
- **Spec coverage:** drill.ts pure builder (T1); FilterChips + DrillMenu (T2); DrillRecordsDialog (T3); renderer + ChartExplore wiring incl. filters state/specForRun/re-run/chips/menu/dialog (T4); pivot full-width (T5); FieldZone config redesign Data tab (T6); Display restyle switches/headers/sections (T7); sweep + smoke + review (T8).
- **Type consistency:** `DrillContext {dims:{col,value}[]}` defined in T1, consumed by T2 (DrillMenu), T4 (renderers/ChartExplore). `ChartFilter` is the existing `@/api/charts` type. `drillFilters`/`pivotCellDrill`/`tableCellDrill` signatures match call sites. `FieldZone({label,selected,options,onChange})` matches the OrderedPicker prop shape it replaces (drop-in). `onCellDrill(ctx,x,y)` identical across both renderers and ChartExplore.
- **No backend/param-shape break:** `filters` already exists in `QuerySpec`; `specForRun` just stops hard-coding `[]`; persistence is automatic (filters are inside `spec`). Pivot/table `paramsForSave` branches otherwise unchanged. No new shadcn primitive (dialog/popover/switch/dropdown-menu present).
- **Visual tasks (5, 7)** are explicitly screenshot-tuned, consistent with how 3c-3.4/3c-3.6 visual work was handled.
