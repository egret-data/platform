# Phase 3c-3.6 — Pivot interactions (collapse / sticky / dimension-sort) + table column reorder

> Status: approved design. Canonical (English). Vietnamese mirror:
> `docs/vi/specs/2026-06-20-phase-3c-3-6-pivot-interactions-design.md`.
> Frontend-only. Builds on merged 3c-3.5 (`565cb64`). Branch `feat/phase-3c-3-6-pivot-interactions`.

## Goal

Bring the **Pivot** viz up to Metabase parity for interactions, and add the column **drag-reorder**
that was deferred from 3c-3.5 to the **Table** viz. All client-side over the existing ROLLUP result
(no re-query), persisted in chart `params` JSONB ([[metabase-fidelity-preference]]):

**Part A — Pivot:**
1. **Collapse / expand row groups** — `[–]`/`[+]` on subtotal-bearing row-group headers; a collapsed
   group shows only its subtotal row.
2. **Sticky header + sticky left columns** — header and the row-header columns stay while scrolling.
3. **Dimension sort** — sort a row dimension's values ascending/descending (Metabase-faithful: applied
   to the tree order, not a measure sort).

**Part B — Table:**
4. **Column drag-reorder** — drag a header to reorder columns (dnd-kit), persisted by column name.

## Scope

**In scope:**
- Part A on the pivot engine (`engine.ts`) + renderer (`PivotTable.tsx`): wire the already-declared
  `PivotInput.collapsed` into `buildPivot`; add a per-dimension sort input; add toggle UI + sticky CSS.
- Part B on `ChartTable.tsx`: `@dnd-kit/core` + `@dnd-kit/sortable` column reorder driving TanStack
  `columnOrder`, persisted in `tableState.columnOrder`.
- New `ChartParams` fields `collapsedRows` + `pivotSort` (pivot-only) and `TableState.columnOrder`
  (table-only); hydrate/persist wiring in `ChartExplore`.

**Out of scope:** any backend/engine(SQL)/query change (everything is client-side over the ROLLUP
result); **column-dimension** sort (only row-dimension sort has a natural click target — defer);
**measure sort** for the pivot (Metabase doesn't do it; it fights the tree/subtotal/collapse
structure); pivot column resize/pin/pagination (pivot is a custom table, not TanStack — not pursued);
row virtualization. The regular Table already has resize/show-hide/pin/sticky/pagination from 3c-3.5
— this phase only adds reorder to it.

Decisions locked in brainstorming: pivot = **collapse + sticky + dimension sort (row dims, click the
row-dim-name header to cycle none→asc→desc)**; **fold in** the table dnd-kit column reorder.

## Architecture

Same split as prior chart phases: pure logic in `engine.ts` (TDD), UI/CSS in the renderers, and
`ChartExplore` owns the interaction state and persists it in `params`. No re-query: collapse and sort
re-shape the already-fetched ROLLUP rows client-side; the SQL (which always emits every grouping set)
is unchanged.

## 1. Persistence — new fields

`frontend/src/api/charts.ts`:

```ts
export interface TableState {
  // …existing: columnWidths, hiddenColumns, pinnedLeft, pagination…
  columnOrder?: string[];   // table viz only: column names in display order
}

export interface ChartParams {
  // …existing fields…
  collapsedRows?: string[];                       // pivot only: serialized collapsed row-group paths
  pivotSort?: Record<string, "asc" | "desc">;     // pivot only: row-dimension name -> sort direction
}
```

All keyed by **name / serialized path**, reconciled against the live columns/dimensions (drop unknown
entries). No backend change — `params` is JSONB round-tripped verbatim.

## 2. Part A1 — Collapse/expand (engine + renderer)

### Engine (`pivot/engine.ts`)
`PivotInput.collapsed: Set<string>` already exists but `buildPivot`/`emit` ignore it. Wire it:

- A serialized path is `JSON.stringify(path)` where `path` is the row-group's values (e.g.
  `["2-HIGH"]`). This matches the existing `pathKey` convention.
- In `emit`, when about to recurse into a child group node whose path is collapsed AND the group is
  collapsible (has a subtotal — i.e. `showColTotals && children.length>0 && (!condense || children>1)`):
  **do not** emit its subtree; emit a single row of `kind: "subtotal"` for that path (the collapsed
  group's aggregate), and do not also emit the normal trailing subtotal (the collapsed row IS the
  subtotal).
- Mark collapsibility on the group's header cell so the renderer can draw the toggle. Extend
  `RowHeaderCell`:
  ```ts
  export interface RowHeaderCell {
    label: string; depth: number; rowSpan: number; colSpan: number;
    path?: (string | null)[];   // the group path this cell heads (for the collapse toggle)
    collapsible?: boolean;      // group has a subtotal → can be collapsed
    collapsed?: boolean;        // currently collapsed (renderer shows [+] vs [–])
  }
  ```
  For an expanded collapsible group, the group's header cell (the one with `rowSpan = childSpan`) gets
  `collapsible: true, collapsed: false, path`. For a collapsed group, the single emitted row's header
  cell gets `collapsible: true, collapsed: true, path` and the label is the group value.

### Renderer (`pivot/PivotTable.tsx`)
- New prop `collapsed?: Set<string>` (already passed as `input.collapsed`) and
  `onToggleCollapse?: (serializedPath: string) => void`.
- When a `RowHeaderCell.collapsible`, render a small toggle button before the label: `▸` (collapsed)
  / `▾` (expanded), calling `onToggleCollapse(JSON.stringify(hc.path))`. Keep it keyboard-accessible
  (a real `<button>`).
- A small `pivot/RowToggle.tsx` may hold the button if `PivotTable.tsx` grows.

### Wiring (`ChartExplore.tsx`)
- State `const [collapsed, setCollapsed] = useState<Set<string>>(new Set())`.
- Hydrate `setCollapsed(new Set(p.collapsedRows ?? []))`; reset on dataset change.
- Persist (pivot branch) `collapsedRows: [...collapsed]`.
- Pass `collapsed` + `onToggleCollapse={(p) => setCollapsed(prev => { const n = new Set(prev); n.has(p) ? n.delete(p) : n.add(p); return n; })}` to `PivotTable`; the pivot input already takes `collapsed`.

## 3. Part A2 — Sticky header + sticky left columns

In `PivotTable.tsx` (a plain `<table>`), reuse the 3c-3.5 sticky pattern:
- Wrap the table in `max-h-[60vh] overflow-auto` (the existing `overflow-auto rounded-md border`
  frame gains the `max-h`).
- `thead` cells → `sticky top-0` + the existing `bg-muted/40` + `z-10`.
- The first `rowDimNames.length` body row-header `<th>` columns and the top-left corner/row-dim-name
  header cells → `sticky left-0` with computed `left` offsets, an opaque background, `z-20`; the
  top-left corner (sticky on both axes) gets `z-30`.
- Row-header cells use `rowSpan`; `position: sticky` works on them. Offsets: row-dim columns are
  fixed-purpose; use their natural widths via `left: 0` for the first and cumulative offsets for
  subsequent row-dim columns (compute from a fixed row-dim column width, or `left-0` for a single
  row-dim column which is the common case).

## 4. Part A3 — Dimension sort (row dimensions)

### Engine
- `buildPivot` accepts the sort directions (add `sortDims?: Record<string, "asc"|"desc">` to
  `PivotInput`, default `{}`). In `buildTree` (or a post-build sort of each node's `children`), order
  each level's children by the dimension value for that level's dimension using `localeCompare`
  (numbers compared numerically when both parse as numbers), ascending or descending; absent ⇒ keep
  first-appearance order (current behavior).
- The dimension for tree level `d` (0-based) is `rowDims[d]`; look up `sortDims[rowDims[d]]`.

### Renderer + wiring
- The row-dimension-name header cells (the `rowDimNames` `<th>` in the top header) become clickable:
  clicking cycles that dimension `none → asc → desc → none`, showing ` ▲`/` ▼`.
- `ChartExplore` state `const [pivotSort, setPivotSort] = useState<Record<string,"asc"|"desc">>({})`;
  hydrate from `p.pivotSort ?? {}`; reset on dataset change; persist (pivot branch); pass into the
  `PivotTable` input as `sortDims` and an `onCycleSort(dimName)` callback.

## 5. Part B — Table column drag-reorder (dnd-kit)

- Add deps `@dnd-kit/core` + `@dnd-kit/sortable` (+ `@dnd-kit/utilities`).
- In `ChartTable.tsx`: derive TanStack `columnOrder` from `tableState.columnOrder` (names →
  `col_${i}` ids, reconciled: unknown names dropped, missing columns appended in natural order).
  Wrap the header row's cells in a dnd-kit `SortableContext` (horizontal); each header is a sortable
  item dragged by a small grip (or the header itself with a drag sensor that has an activation
  distance so a click still sorts). On drop (`onDragEnd`), compute the new name order via `arrayMove`
  and persist `onTableStateChange({ ...tableState, columnOrder })`.
- Reorder must not break resize/sort/pin: dnd-kit activation constraint (e.g. 8px distance) so a plain
  click still toggles sort and the resize handle still works; pinned columns may be excluded from
  reordering (drag disabled on pinned headers) to keep the freeze block contiguous.

## 6. Error handling

- `collapsedRows` referencing a path that no longer exists (dataset/dimension change) is simply never
  matched during `emit` — harmless; on persist we keep only paths still present (filter against the
  built row paths, or accept harmless extras and let them age out — prefer filtering to avoid blob
  bloat).
- `pivotSort` / `columnOrder` keys for removed dimensions/columns are ignored in the engine/derivation.
- Sticky needs a bounded container (`max-h`); shorter tables simply don't scroll. z-index: corner
  `z-30` > sticky-left `z-20` > sticky-top header `z-10`.
- Collapse only offered on groups that actually have a subtotal (respects `showColTotals`/`condense`).

## 7. Testing

Engine unit (Vitest, `pivot/__tests__/engine.test.ts` — extend existing):
- Collapsing a top-level group emits its subtotal row and **omits** its child + deeper rows; the
  collapsed row's header cell is `collapsible && collapsed`.
- An expanded collapsible group's header cell is `collapsible && !collapsed` with the group `path`.
- `sortDims` asc/desc reorders a level's children correctly (incl. numeric-looking values); absent ⇒
  first-appearance order unchanged.
- Collapse + sort combined: order applies, collapsed subtree still omitted.

Renderer/unit:
- `PivotTable`: a collapsible header renders a toggle button; clicking calls `onToggleCollapse` with
  the serialized path; collapsed header shows `▸`, expanded `▾`; sticky classes present on header +
  first row-header column.
- `ChartTable`: `columnOrder` reorders rendered header order; an `onDragEnd` reorder calls
  `onTableStateChange` with the new name order (test at the handler/state level, not real pointer
  drag).

Manual Playwright smoke: pivot with 2 row dims → collapse a group (children vanish, `[+]` shows) →
expand back; scroll → header + left columns stick; click a row-dim header → ▲/▼ reorders groups;
table → drag a column header to reorder; **Save → reopen → collapse/sort/order all hydrate**.

## 8. File structure

- Modify `frontend/src/api/charts.ts` — `TableState.columnOrder`, `ChartParams.collapsedRows` +
  `pivotSort`.
- Modify `frontend/src/components/chart/pivot/engine.ts` — wire `collapsed`; add `sortDims`; extend
  `RowHeaderCell`.
- Modify `frontend/src/components/chart/pivot/PivotTable.tsx` — toggle UI, sticky CSS, sort headers.
- Optionally create `frontend/src/components/chart/pivot/RowToggle.tsx`.
- Modify `frontend/src/components/chart/ChartTable.tsx` — dnd-kit column reorder.
- Modify `frontend/src/routes/explore/ChartExplore.tsx` — collapsed/pivotSort state + hydrate/reset/
  persist + props to PivotTable; tableState.columnOrder flows through unchanged wiring.
- Modify `frontend/package.json` — add `@dnd-kit/core`, `@dnd-kit/sortable`, `@dnd-kit/utilities`.
- Tests: extend `pivot/__tests__/engine.test.ts`; extend/add `PivotTable` + `ChartTable` tests.
- Reused unchanged: `format.ts`, `conditional.ts`, settings sections, chart routes/repo/backend.
