# Phase 3c-3.6 — Pivot Interactions + Table Reorder Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add pivot collapse/expand + sticky header/left-columns + row-dimension sort (all client-side over the existing ROLLUP result), and table column drag-reorder (dnd-kit), persisted in chart `params` JSONB.

**Architecture:** Pure logic in `pivot/engine.ts` (TDD) gains collapse + dimension-sort; `PivotTable.tsx` gains toggle UI, sticky CSS, and clickable sort headers; `ChartTable.tsx` gains dnd-kit column reorder driving TanStack `columnOrder`; `ChartExplore` owns the new state and persists it. No backend/SQL change — the ROLLUP query already emits every grouping set; collapse/sort reshape it client-side.

**Tech Stack:** React 18 + Vite + TypeScript + Vitest + @tanstack/react-table + **@dnd-kit/core + @dnd-kit/sortable + @dnd-kit/utilities** (new) + shadcn/ui.

**Spec:** `docs/en/specs/2026-06-20-phase-3c-3-6-pivot-interactions-design.md`.

**Branch:** `feat/phase-3c-3-6-pivot-interactions` (spec committed `7fc51f1`). Built on merged main (`565cb64`).

**Conventions:** plain `git commit` (never `--author`); commit body ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`; tests colocated under `__tests__/`; run npm from `frontend/`; git from repo root; never `git add -A` (a gitignored `.claude/` worktree artifact must stay untracked).

---

## File Structure
- Modify `frontend/src/components/chart/pivot/engine.ts` — `sortDims` input, wire `collapsed`, extend `RowHeaderCell`, sort tree.
- Modify `frontend/src/components/chart/pivot/__tests__/engine.test.ts` — collapse + sort cases.
- Modify `frontend/src/components/chart/pivot/PivotTable.tsx` — toggle UI, sticky CSS, sort headers.
- Modify `frontend/src/components/chart/pivot/__tests__/PivotTable.test.tsx` — toggle + sticky.
- Modify `frontend/src/api/charts.ts` — `TableState.columnOrder`, `ChartParams.collapsedRows` + `pivotSort`.
- Modify `frontend/src/components/chart/ChartTable.tsx` — dnd-kit column reorder.
- Modify `frontend/src/components/chart/__tests__/ChartTable.test.tsx` — columnOrder.
- Modify `frontend/src/routes/explore/ChartExplore.tsx` — collapsed/pivotSort state + hydrate/reset/persist + PivotTable props.
- Modify `frontend/package.json` — dnd-kit deps.

---

## Task 1: Pivot engine — collapse + dimension sort

**Files:**
- Modify: `frontend/src/components/chart/pivot/engine.ts`
- Test: `frontend/src/components/chart/pivot/__tests__/engine.test.ts`

- [ ] **Step 1: Write the failing tests**

Append to `frontend/src/components/chart/pivot/__tests__/engine.test.ts` (a new fixture with **2 row dims** so groups exist):

```ts
// 2 row dims (region, seg) × 0 col dims × 1 metric. ROLLUP(region, seg).
const rowGrouped: PivotInput = {
  columns: ["region", "seg", "revenue", "__grp_0", "__grp_1"],
  rows: [
    ["EU", "SMB", 10, 0, 0],
    ["EU", "ENT", 20, 0, 0],
    ["EU", null, 30, 0, 1],   // EU subtotal
    ["US", "SMB", 5, 0, 0],
    ["US", "ENT", 7, 0, 0],
    ["US", null, 12, 0, 1],   // US subtotal
    [null, null, 42, 1, 1],   // grand total
  ],
  rowDims: ["region", "seg"], colDims: [], metrics: ["revenue"],
  showRowTotals: false, showColTotals: true, condense: true, collapsed: new Set<string>(),
};

describe("collapse", () => {
  it("marks subtotal-bearing groups collapsible when expanded", () => {
    const p = buildPivot(rowGrouped);
    const euCell = p.bodyRows.flatMap((r) => r.headerCells).find((h) => h.label === "EU");
    expect(euCell?.collapsible).toBe(true);
    expect(euCell?.collapsed).toBe(false);
    expect(euCell?.path).toEqual(["EU"]);
    // expanded: EU's leaf children are present
    expect(p.bodyRows.some((r) => JSON.stringify(r.rowPath) === JSON.stringify(["EU", "SMB"]))).toBe(true);
  });

  it("collapsing a group hides its children and shows a collapsed subtotal row", () => {
    const p = buildPivot({ ...rowGrouped, collapsed: new Set(['["EU"]']) });
    // no EU child rows
    expect(p.bodyRows.some((r) => JSON.stringify(r.rowPath) === JSON.stringify(["EU", "SMB"]))).toBe(false);
    // one collapsed row for ["EU"] carrying the subtotal (30) with a collapsed toggle
    const euRow = p.bodyRows.find((r) => JSON.stringify(r.rowPath) === JSON.stringify(["EU"]) && r.kind === "subtotal");
    expect(euRow).toBeTruthy();
    expect(euRow!.values[0]).toBe(30);
    const euCell = euRow!.headerCells.find((h) => h.label === "EU");
    expect(euCell?.collapsible).toBe(true);
    expect(euCell?.collapsed).toBe(true);
    // US still expanded
    expect(p.bodyRows.some((r) => JSON.stringify(r.rowPath) === JSON.stringify(["US", "SMB"]))).toBe(true);
  });
});

describe("dimension sort", () => {
  it("sorts a row dimension descending when requested", () => {
    const p = buildPivot({ ...rowGrouped, sortDims: { region: "desc" } });
    const firstGroupLabel = p.bodyRows.flatMap((r) => r.headerCells).find((h) => h.depth === 0)?.label;
    expect(firstGroupLabel).toBe("US");
  });

  it("keeps first-appearance order when no sort is set", () => {
    const p = buildPivot(rowGrouped);
    const firstGroupLabel = p.bodyRows.flatMap((r) => r.headerCells).find((h) => h.depth === 0)?.label;
    expect(firstGroupLabel).toBe("EU");
  });
});
```

- [ ] **Step 2: Run → FAIL.** `cd frontend && npm test -- engine` → the new cases fail (`collapsible`/`path`/`sortDims` absent).

- [ ] **Step 3: Edit `engine.ts` — (a) `PivotInput.sortDims`**

In the `PivotInput` interface, add after `collapsed: Set<string>;`:
```ts
  sortDims?: Record<string, "asc" | "desc">;  // row-dimension name -> direction; absent = natural order
```

- [ ] **Step 4: Edit `engine.ts` — (b) extend `RowHeaderCell`**

Replace the `RowHeaderCell` interface with:
```ts
export interface RowHeaderCell {
  label: string;
  depth: number;           // which row-dim column it starts at
  rowSpan: number;
  colSpan: number;
  path?: (string | null)[]; // the group path this cell heads (for the collapse toggle)
  collapsible?: boolean;    // group has a subtotal → can be collapsed
  collapsed?: boolean;      // currently collapsed (renderer shows [+] vs [–])
}
```

- [ ] **Step 5: Edit `engine.ts` — (c) sort helpers + sort the row tree**

Add these two module-level functions right after the `buildTree` function:
```ts
function cmpValues(a: string | null, b: string | null): number {
  if (a === b) return 0;
  if (a === null) return -1;
  if (b === null) return 1;
  const na = Number(a), nb = Number(b);
  if (a.trim() !== "" && b.trim() !== "" && Number.isFinite(na) && Number.isFinite(nb)) return na - nb;
  return a.localeCompare(b);
}

function sortTree(node: TreeNode, depth: number, dims: string[], sortDims: Record<string, "asc" | "desc">) {
  const dir = sortDims[dims[depth]];
  if (dir) {
    const sign = dir === "desc" ? -1 : 1;
    node.children.sort((a, b) => cmpValues(a.value, b.value) * sign);
  }
  for (const ch of node.children) sortTree(ch, depth + 1, dims, sortDims);
}
```
Then in `buildPivot`, right after `const rowTree = buildTree(ix.rowLeafPaths);`, add:
```ts
  sortTree(rowTree, 0, input.rowDims, input.sortDims ?? {});
```

- [ ] **Step 6: Edit `engine.ts` — (d) replace the body-row generation with a collapse-aware unified walk**

Replace the ENTIRE block from the comment `// ----- body rows (depth-first over row tree, inserting subtotals) -----` down to and including the final `return { rowDimNames: input.rowDims, topHeader, colLeaves, bodyRows };` with:

```ts
  // ----- body rows (depth-first over row tree, inserting subtotals; collapse-aware) -----
  const bodyRows: BodyRow[] = [];

  function rowValues(rowPath: (string | null)[]): CellValue[] {
    return colLeaves.map((cl) => ix.cell(rowPath, cl.colPath, cl.metric));
  }

  // emit body rows for `node`'s children, each occupying row-dim column `depth`. returns rows emitted.
  function emitChildren(node: TreeNode, path: (string | null)[], depth: number): number {
    let span = 0;
    for (const ch of node.children) {
      const childPath = [...path, ch.value];
      const isGroup = ch.children.length > 0;
      const hasSubtotal = input.showColTotals && isGroup && (!input.condense || ch.children.length > 1);

      if (!isGroup) {
        const start = bodyRows.length;
        bodyRows.push({ rowPath: childPath, kind: "leaf", headerCells: [], values: rowValues(childPath) });
        bodyRows[start].headerCells.push({ label: String(ch.value ?? ""), depth, rowSpan: 1, colSpan: 1 });
        span += 1;
        continue;
      }

      if (hasSubtotal && input.collapsed.has(JSON.stringify(childPath))) {
        // collapsed group → a single subtotal row carrying the group's aggregate + a collapsed toggle
        bodyRows.push({
          rowPath: childPath,
          kind: "subtotal",
          headerCells: [{ label: String(ch.value ?? ""), depth, rowSpan: 1, colSpan: m - depth, path: childPath, collapsible: true, collapsed: true }],
          values: rowValues(childPath),
        });
        span += 1;
        continue;
      }

      const start = bodyRows.length;
      const childSpan = emitChildren(ch, childPath, depth + 1);
      bodyRows[start].headerCells.push({
        label: String(ch.value ?? ""),
        depth,
        rowSpan: childSpan,
        colSpan: 1,
        ...(hasSubtotal ? { path: childPath, collapsible: true, collapsed: false } : {}),
      });
      span += childSpan;

      if (hasSubtotal) {
        bodyRows.push({
          rowPath: childPath,
          kind: "subtotal",
          headerCells: [{ label: `Totals for ${String(ch.value ?? "")}`, depth, rowSpan: 1, colSpan: m - depth }],
          values: rowValues(childPath),
        });
        span += 1;
      }
    }
    return span;
  }

  emitChildren(rowTree, [], 0);

  if (input.showColTotals) {
    bodyRows.push({
      rowPath: [],
      kind: "grand",
      headerCells: [{ label: "Grand totals", depth: 0, rowSpan: 1, colSpan: m }],
      values: rowValues([]),
    });
  }

  for (const br of bodyRows) br.headerCells.sort((a, b) => a.depth - b.depth);

  return { rowDimNames: input.rowDims, topHeader, colLeaves, bodyRows };
```

This unifies the old separate depth-0 loop + `emit` into one `emitChildren` (the old code's leaf-label/group-label/subtotal placement is preserved; the final `headerCells.sort` normalizes order exactly as before), and adds the collapse short-circuit + collapsible markers. The `m` and `colLeaves`/`topHeader` above are unchanged.

- [ ] **Step 7: Run → PASS + gates**

`cd frontend && npm test -- engine` → all pass (existing 1×1 fixture cases AND the new collapse/sort cases — the unified walk is output-equivalent for the non-collapsed/non-sorted path). `npx tsc --noEmit` + `npx eslint src/components/chart/pivot/engine.ts src/components/chart/pivot/__tests__/engine.test.ts` → clean.

- [ ] **Step 8: Commit**

```bash
git add frontend/src/components/chart/pivot/engine.ts frontend/src/components/chart/pivot/__tests__/engine.test.ts
git commit -m "feat(web): pivot engine — collapse-aware body rows + dimension sort"
```

---

## Task 2: PivotTable renderer — toggle UI + sticky + sort headers

**Files:**
- Modify (full rewrite): `frontend/src/components/chart/pivot/PivotTable.tsx`
- Test: `frontend/src/components/chart/pivot/__tests__/PivotTable.test.tsx`

- [ ] **Step 1: Write the failing test**

Append to `frontend/src/components/chart/pivot/__tests__/PivotTable.test.tsx` (keep existing tests):

```tsx
import { vi } from "vitest";
import { fireEvent } from "@testing-library/react";

const grouped = {
  columns: ["region", "seg", "revenue", "__grp_0", "__grp_1"],
  rows: [
    ["EU", "SMB", 10, 0, 0], ["EU", "ENT", 20, 0, 0], ["EU", null, 30, 0, 1],
    ["US", "SMB", 5, 0, 0], ["US", "ENT", 7, 0, 0], ["US", null, 12, 0, 1],
    [null, null, 42, 1, 1],
  ],
  rowDims: ["region", "seg"], colDims: [], metrics: ["revenue"],
  showRowTotals: false, showColTotals: true, condense: true, collapsed: new Set<string>(),
};

describe("PivotTable interactions", () => {
  it("renders a collapse toggle on a collapsible group and fires onToggleCollapse", () => {
    const onToggleCollapse = vi.fn();
    render(<PivotTable input={grouped} onToggleCollapse={onToggleCollapse} />);
    const btn = screen.getByRole("button", { name: "Collapse group" });
    fireEvent.click(btn);
    expect(onToggleCollapse).toHaveBeenCalledWith('["EU"]');
  });

  it("fires onCycleSort when a row-dimension header is clicked", () => {
    const onCycleSort = vi.fn();
    render(<PivotTable input={grouped} onCycleSort={onCycleSort} />);
    fireEvent.click(screen.getByText("region"));
    expect(onCycleSort).toHaveBeenCalledWith("region");
  });
});
```

(If the existing `PivotTable.test.tsx` lacks `render`/`screen` imports, ensure `import { render, screen } from "@testing-library/react";` is present.)

- [ ] **Step 2: Run → FAIL.** `cd frontend && npm test -- PivotTable` → new cases fail.

- [ ] **Step 3: Replace `PivotTable.tsx` entirely**

```tsx
import { buildPivot, type PivotInput } from "./engine";
import { formatValue } from "../format";
import { buildConditional } from "../conditional";
import type { ColumnFormat, Rule } from "../format-types";

export function PivotTable({
  input, columnSettings = {}, conditionalFormatting = [], metricTypes = {},
  onToggleCollapse, onCycleSort,
}: {
  input: PivotInput;
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
  metricTypes?: Record<string, string>;
  onToggleCollapse?: (serializedPath: string) => void;
  onCycleSort?: (dimName: string) => void;
}) {
  const p = buildPivot(input);
  const condRows = p.bodyRows.filter((br) => br.kind === "leaf").map((br) => br.values);
  const condColIndex: Record<string, number> = {};
  p.colLeaves.forEach((cl, i) => { if (!(cl.metric in condColIndex)) condColIndex[cl.metric] = i; });
  const cond = buildConditional(condRows, condColIndex, conditionalFormatting, { isPivot: true });
  const rowDimCount = p.rowDimNames.length;
  const levels = p.topHeader.length;
  const sortDirs = input.sortDims ?? {};

  return (
    <div className="max-h-[60vh] overflow-auto rounded-md border">
      <table className="border-collapse text-sm">
        <thead className="bg-muted/40 text-foreground">
          {p.topHeader.map((level, li) => (
            <tr key={li} className="border-b border-border/60">
              {/* empty corner above the row-dim name headers (only when >1 header level) */}
              {li === 0 && levels > 1 && (
                <th className="sticky left-0 top-0 z-30 border-r border-border/60 bg-muted/40" colSpan={rowDimCount} rowSpan={levels - 1} />
              )}
              {/* row-dim names as their own column headers on the last header row (clickable to sort) */}
              {li === levels - 1 &&
                p.rowDimNames.map((name, di) => {
                  const dir = sortDirs[name];
                  return (
                    <th
                      key={`rd-${di}`}
                      onClick={onCycleSort ? () => onCycleSort(name) : undefined}
                      className={`sticky top-0 z-30 border-r border-border/60 bg-muted/40 px-4 py-2 text-left font-semibold whitespace-nowrap ${di === 0 ? "left-0" : ""} ${onCycleSort ? "cursor-pointer select-none" : ""}`}
                    >
                      {name}{dir === "asc" ? " ▲" : dir === "desc" ? " ▼" : ""}
                    </th>
                  );
                })}
              {level.map((c, ci) => (
                <th
                  key={ci}
                  className="sticky top-0 z-10 border-r border-border/60 bg-muted/40 px-4 py-2 text-right font-semibold whitespace-nowrap last:border-r-0"
                  colSpan={c.colSpan}
                  rowSpan={c.rowSpan ?? 1}
                >
                  {c.label}
                </th>
              ))}
            </tr>
          ))}
        </thead>
        <tbody>
          {p.bodyRows.map((br, ri) => (
            <tr
              key={ri}
              className={
                br.kind === "grand"
                  ? "border-t-2 border-border font-semibold"
                  : br.kind === "subtotal"
                    ? "border-t border-border/60 bg-muted/20 font-semibold"
                    : "border-t border-border/60 hover:bg-muted/20"
              }
            >
              {br.headerCells.map((hc, hi) => (
                <th
                  key={hi}
                  scope="row"
                  className={`border-r border-border/60 bg-background px-4 py-2 text-left align-top font-normal whitespace-nowrap ${hc.depth === 0 ? "sticky left-0 z-20" : ""}`}
                  rowSpan={hc.rowSpan}
                  colSpan={hc.colSpan}
                >
                  {hc.collapsible && hc.path && onToggleCollapse && (
                    <button
                      type="button"
                      onClick={() => onToggleCollapse(JSON.stringify(hc.path))}
                      aria-label={hc.collapsed ? "Expand group" : "Collapse group"}
                      className="mr-1 inline-flex h-4 w-4 items-center justify-center rounded text-muted-foreground hover:bg-muted hover:text-foreground"
                    >
                      {hc.collapsed ? "▸" : "▾"}
                    </button>
                  )}
                  {hc.label}
                </th>
              ))}
              {br.values.map((v, vi) => {
                const cl = p.colLeaves[vi];
                const text = formatValue(v, metricTypes[cl.metric] ?? "double", columnSettings[cl.metric]);
                const bg = cond.bg(v, cl.metric);
                return (
                  <td
                    key={vi}
                    className={`border-l border-border/60 px-4 py-2 text-right tabular-nums last:border-r-0 ${
                      br.kind === "subtotal" || br.kind === "grand" || cl.isTotal ? "font-semibold" : ""
                    }`}
                    style={bg ? { backgroundColor: bg } : undefined}
                  >
                    {text}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
```

NOTES: sticky-left freezes the **first** row-dim column (`hc.depth === 0` body cells, `di === 0` header, the corner) — the common single-row-dim case freezes the whole row-header; deeper row-dim columns scroll (acceptable v1). z-index: corner/row-dim-name `z-30` > sticky body header `z-20` > sticky top value header `z-10`. The toggle is a real `<button>` (keyboard-accessible); `▾` expanded / `▸` collapsed.

- [ ] **Step 4: Run → PASS + gates.** `cd frontend && npm test -- PivotTable` → pass; `npx tsc --noEmit`; `npx eslint src/components/chart/pivot/PivotTable.tsx src/components/chart/pivot/__tests__/PivotTable.test.tsx` → clean.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/pivot/PivotTable.tsx frontend/src/components/chart/pivot/__tests__/PivotTable.test.tsx
git commit -m "feat(web): PivotTable — collapse toggle, sticky header/left, clickable sort headers"
```

---

## Task 3: Table column drag-reorder (dnd-kit)

**Files:**
- Modify: `frontend/package.json` (deps)
- Modify: `frontend/src/api/charts.ts` (`TableState.columnOrder`)
- Modify: `frontend/src/components/chart/ChartTable.tsx`
- Test: `frontend/src/components/chart/__tests__/ChartTable.test.tsx`

- [ ] **Step 1: Install dnd-kit**

```bash
cd /Users/1pro-l4r8/Documents/github/data-portal/egret/frontend
npm install @dnd-kit/core @dnd-kit/sortable @dnd-kit/utilities
```
Expected: three packages added to `package.json` dependencies.

- [ ] **Step 2: Add `columnOrder` to `TableState`**

In `frontend/src/api/charts.ts`, add to the `TableState` interface (after `pagination?...`):
```ts
  columnOrder?: string[];   // table viz only: column names in display order
```

- [ ] **Step 3: Write the failing test**

Append to `frontend/src/components/chart/__tests__/ChartTable.test.tsx`:
```tsx
  it("renders columns in tableState.columnOrder", () => {
    render(<ChartTable columns={COLS} rows={mkRows(2)} tableState={{ columnOrder: ["b", "a"] }} />);
    const headers = within(document.querySelector("thead")!).getAllByRole("columnheader");
    expect(headers[0]).toHaveTextContent("b");
    expect(headers[1]).toHaveTextContent("a");
  });
```
(`COLS`, `mkRows`, `within` already exist in this file from 3c-3.5.)

- [ ] **Step 4: Run → FAIL.** `cd frontend && npm test -- ChartTable` → the order test fails (columnOrder not applied).

- [ ] **Step 5: Wire `columnOrder` + dnd-kit into `ChartTable.tsx`**

Add imports at the top:
```tsx
import {
  DndContext, PointerSensor, useSensor, useSensors,
  closestCenter, type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext, horizontalListSortingStrategy, useSortable, arrayMove,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
```

Derive the controlled `columnOrder` (names → ids, reconciled). Add this memo next to the other derived state (after `columnPinning`):
```tsx
  const columnOrder = useMemo<string[]>(() => {
    const byName = new Map(columns.map((c, i) => [c.name, `col_${i}`]));
    const ordered = (tableState.columnOrder ?? [])
      .map((name) => byName.get(name))
      .filter((id): id is string => id != null);
    // append any columns not in the saved order, in natural order
    columns.forEach((_, i) => { if (!ordered.includes(`col_${i}`)) ordered.push(`col_${i}`); });
    return ordered;
  }, [columns, tableState.columnOrder]);
```
Add `columnOrder` to the table `state`:
```tsx
    state: {
      sorting,
      columnSizing: localSizing,
      columnVisibility,
      columnPinning,
      columnOrder,
      ...(paginationEnabled ? { pagination: { pageIndex, pageSize } } : {}),
    },
```
Add a drag-end handler + sensors above the `return`:
```tsx
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 8 } }));
  function handleDragEnd(e: DragEndEvent) {
    const { active, over } = e;
    if (!over || active.id === over.id) return;
    const ids = table.getState().columnOrder.length ? table.getState().columnOrder : columns.map((_, i) => `col_${i}`);
    const from = ids.indexOf(String(active.id));
    const to = ids.indexOf(String(over.id));
    if (from < 0 || to < 0) return;
    const nextIds = arrayMove(ids, from, to);
    const idToName = new Map(columns.map((c, i) => [`col_${i}`, c.name]));
    const names = nextIds.map((id) => idToName.get(id)).filter((n): n is string => n != null);
    onTableStateChange?.({ ...tableState, columnOrder: names });
  }
```

Wrap the `<table>` in a `DndContext` and the header cells in a `SortableContext`, and make each header a sortable item via a small `SortableHeader` component. Add this component ABOVE `ChartTable`:
```tsx
function SortableHeader({ id, children, className, style, onClick }: {
  id: string; children: React.ReactNode; className?: string;
  style?: React.CSSProperties; onClick?: React.MouseEventHandler;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id });
  return (
    <th
      ref={setNodeRef}
      onClick={onClick}
      style={{ ...style, transform: CSS.Translate.toString(transform), transition, opacity: isDragging ? 0.6 : 1 }}
      className={className}
      {...attributes}
      {...listeners}
    >
      {children}
    </th>
  );
}
```
(Add `import type React from "react";` if not already importing React types; the file already uses React via JSX, so `import { ... } from "react"` is present — add `type React` only if tsc complains about the `React.ReactNode`/`React.CSSProperties`/`React.MouseEventHandler` names; otherwise import them as named: `import { useEffect, useMemo, useRef, useState, type ReactNode, type CSSProperties, type MouseEventHandler } from "react";` and use those bare names.)

Then in the JSX, wrap the existing scroll container's `<table>`: change the header row rendering so each `<th>` becomes `<SortableHeader id={h.column.id} ...>` carrying the SAME className/style/onClick currently on the `<th>`, and wrap the whole `<table>`’s thead area in:
```tsx
<DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
  <SortableContext items={table.getVisibleLeafColumns().map((c) => c.id)} strategy={horizontalListSortingStrategy}>
    {/* the <table> ... thead uses <SortableHeader> per header cell */}
  </SortableContext>
</DndContext>
```
Keep the resize-handle `<div>` and the sort `onClick` on the header — the `activationConstraint: { distance: 8 }` ensures a click still sorts and the resize handle still works (drag only starts after 8px). Pinned columns: pass `disabled: h.column.getIsPinned() === "left"` into `useSortable` (add a `disabled?: boolean` prop to `SortableHeader` and forward it) so frozen columns don't reorder.

The implementer should integrate these pieces minimally; the **behavior contract** that the test enforces is: header order follows `columnOrder`, and `handleDragEnd` persists the reordered names. If wrapping the table in `DndContext` while preserving the sticky/resize/pin markup proves fiddly, keep the `<th>`→`SortableHeader` swap purely additive (same classes/style/onClick) and verify all existing ChartTable tests still pass.

- [ ] **Step 6: Run → PASS + full sweep**

`cd frontend && npm test -- ChartTable` → the order test + all prior ChartTable tests pass. Then `npm run typecheck && npm run lint && npm run build && npm test` (full) → green.

- [ ] **Step 7: Commit**

```bash
git add frontend/package.json frontend/package-lock.json frontend/src/api/charts.ts frontend/src/components/chart/ChartTable.tsx frontend/src/components/chart/__tests__/ChartTable.test.tsx
git commit -m "feat(web): table column drag-reorder via dnd-kit (persist tableState.columnOrder)"
```

---

## Task 4: ChartExplore wiring + sweep + smoke + final review

**Files:**
- Modify: `frontend/src/api/charts.ts` (`ChartParams.collapsedRows` + `pivotSort`)
- Modify: `frontend/src/routes/explore/ChartExplore.tsx`

- [ ] **Step 1: Add pivot params**

In `frontend/src/api/charts.ts`, add to `ChartParams` (after `conditionalFormatting?: Rule[];` / `tableState?`):
```ts
  collapsedRows?: string[];                     // pivot only: serialized collapsed row-group paths
  pivotSort?: Record<string, "asc" | "desc">;   // pivot only: row-dimension name -> direction
```

- [ ] **Step 2: State + hydrate + reset (ChartExplore.tsx)**

After the `conditionalFormatting` state line, add:
```tsx
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [pivotSort, setPivotSort] = useState<Record<string, "asc" | "desc">>({});
```
In the hydration `useEffect`, after `setConditionalFormatting(p.conditionalFormatting ?? []);`, add:
```tsx
    setCollapsed(new Set(p.collapsedRows ?? []));
    setPivotSort(p.pivotSort ?? {});
```
In `resetControls()`, add `setCollapsed(new Set()); setPivotSort({});` to the reset list.

- [ ] **Step 3: Persist (paramsForSave)**

Change the pivot branch of `paramsForSave()` to include the two fields:
```tsx
    return viz === "pivot_table"
      ? { spec: specForRun(), rows: pRows, cols: pCols, metrics: pVals, showRowTotals, showColTotals, condense, columnSettings, conditionalFormatting, collapsedRows: [...collapsed], pivotSort }
      : { spec: specForRun(), columnSettings, conditionalFormatting, tableState };
```

- [ ] **Step 4: Feed the pivot input + callbacks**

Change the PivotTable render to pass `collapsed`/`sortDims` into the input and the two callbacks:
```tsx
        {finished && results.data && viz === "pivot_table" && (
          <PivotTable
            input={{
              columns: results.data.columns.map((c) => c.name),
              rows: results.data.rows,
              rowDims: pRows, colDims: pCols, metrics: pVals,
              showRowTotals, showColTotals, condense, collapsed,
              sortDims: pivotSort,
            }}
            columnSettings={columnSettings}
            conditionalFormatting={conditionalFormatting}
            metricTypes={metricTypes}
            onToggleCollapse={(path) =>
              setCollapsed((prev) => {
                const next = new Set(prev);
                if (next.has(path)) next.delete(path); else next.add(path);
                return next;
              })
            }
            onCycleSort={(dim) =>
              setPivotSort((prev) => {
                const cur = prev[dim];
                const next = { ...prev };
                if (cur === undefined) next[dim] = "asc";
                else if (cur === "asc") next[dim] = "desc";
                else delete next[dim];
                return next;
              })
            }
          />
        )}
```

- [ ] **Step 5: Sweep**

`cd frontend && npm run typecheck && npm run lint && npm run build && npm test` → 0 type errors, lint clean, build ok, all tests pass.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/api/charts.ts frontend/src/routes/explore/ChartExplore.tsx
git commit -m "feat(web): wire pivot collapse + dimension sort (persist/hydrate) into chart Explore"
```

- [ ] **Step 7: Manual Playwright smoke** (vite HMR; backend unchanged — no migration)

Login `admin@egret.local` / `Admin12345!`, ws `eng`. New chart → **Pivot** → dataset `orders`, Rows = `orderpriority` + `orderstatus`, Values = `revenue` → Run:
1. **Collapse**: a row-group header (e.g. an `orderpriority` value) shows `▾`; click it → its `orderstatus` children vanish, a single subtotal row with `▸` remains; click again → expands.
2. **Sort**: click the `orderpriority` row-dim header → groups reorder ascending (`▲`); click again → descending (`▼`); again → natural order.
3. **Sticky**: with enough rows, scroll vertically → the header sticks; scroll horizontally (add a column dim or narrow the window) → the first row-header column stays.
4. **Table reorder**: switch viz to **Table** (≥2 columns) → drag a column header onto another → order changes.
5. **Persist**: **Save → reopen** → collapsed groups, sort arrows, and table column order all hydrate; 0 console errors.

- [ ] **Step 8: Final review**

Dispatch a final reviewer over the phase `git diff` (base = spec commit `7fc51f1`). Focus: the engine's unified `emitChildren` is output-equivalent to the old code for non-collapsed/non-sorted input (no regression in colSpan/rowSpan/subtotal placement); collapse short-circuit correctness; sort stability + numeric compare; sticky z-index layering; dnd-kit not breaking resize/sort/pin (activation distance) and reconciling stale `columnOrder` names; pivot-only vs table-only param isolation (collapsedRows/pivotSort never written for table, columnOrder never for pivot); persist/hydrate round-trip.

- [ ] **Step 9: Final commit (if fixes)**

```bash
git add -A && git commit -m "test: phase 3c-3.6 sweep + manual smoke fixes"
```
Then use **superpowers:finishing-a-development-branch**.

---

## Self-Review notes (plan author)
- **Spec coverage:** collapse engine + markers (T1), dimension sort engine (T1), toggle UI + sticky + sort headers (T2), table dnd reorder + columnOrder persist (T3), pivot params + collapse/sort wiring + persist/hydrate (T4), sweep + smoke (all four interactions) + final review (T4). TableState.columnOrder (T3); ChartParams.collapsedRows/pivotSort (T4).
- **Type consistency:** `RowHeaderCell` extension (`path`/`collapsible`/`collapsed`) defined in T1, consumed in T2. `PivotInput.sortDims` (T1) ↔ `input.sortDims` in renderer (T2) ↔ `sortDims: pivotSort` in ChartExplore (T4). `onToggleCollapse(serializedPath)` / `onCycleSort(dimName)` signatures match T2 props ↔ T4 call sites. `TableState.columnOrder: string[]` (T3) ↔ derivation + persist (T3) ↔ flows through existing tableState wiring (no T4 change needed). `collapsed: Set<string>` ↔ `collapsedRows: string[]` (array↔Set at the persistence boundary).
- **No backend/param-shape break:** pivot branch of `paramsForSave` gains two additive fields; table branch unchanged from 3c-3.5 except the already-present `tableState` (now carrying `columnOrder`). Hydration tolerates absence (`?? []`, `?? {}`, `new Set(... ?? [])`).
- **Engine risk:** the body-row rewrite is output-equivalent for the existing path; the existing engine tests (exact bodyRows for the 1×1 fixture) gate it, plus the new 2-row-dim collapse/sort cases.
