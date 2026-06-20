# Phase 3c-3.7 — Drill-through + pivot full-width + config-panel redesign

> Status: approved design. Canonical (English). Vietnamese mirror:
> `docs/vi/specs/2026-06-20-phase-3c-3-7-drill-config-design.md`.
> Frontend-only. Builds on merged 3c-3.6 (`e3a9b4e`). Branch `feat/phase-3c-3-7-drill-config`.

## Goal

Three improvements to the chart Explore experience, all frontend-only and Metabase-faithful
([[metabase-fidelity-preference]]):

1. **Drill-through** on Table + Pivot — click a value cell → a menu with **"Filter by {dim} is
   {value}"** (adds an `=` filter, re-runs the same viz) and **"See these records"** (opens a drawer
   with the unaggregated rows behind the cell). A **filter-chips bar** shows/removes active filters.
2. **Pivot full-width** — the pivot table fills the preview width (instead of leaving an empty
   bordered area), tuned so it doesn't look sparse.
3. **Config-panel redesign** (both the **Data** and **Display** tabs) — a unified Metabase-style
   visual system: drag-zone field chips, consistent uppercase section headers, switch toggles.

## Scope

**In scope (frontend only):**
- Drill: a pure `cellDrill` filter-builder, a `DrillMenu` popover on value cells of both renderers, a
  `DrillRecordsDialog` (raw-rows query in a side sheet), a `FilterChips` bar; `filters` state wired
  through `specForRun()` and persisted in `params.spec.filters` (already part of `QuerySpec`).
- Pivot full-width CSS.
- Config redesign: a reusable `FieldZone` (dnd-kit chips with reorder) replacing `OrderedPicker` for
  the pivot Rows/Columns/Values; consistent section headers + shadcn `Switch` toggles across the
  Display tab; restyled Format/Conditional/Columns sections.

**Out of scope:** any backend/engine/SQL change — `chartquery::build` already renders `filters`
(eq/ne/gt/ge/lt/le/like/in/not_in/is_null/is_not_null via the security-critical `sql_literal`) into a
`WHERE`, and already has a raw-records mode (`metrics: []` → `SELECT columns … WHERE …`). Drilling
into graphical charts (none exist). Cross-zone field drag (Rows↔Columns) — nice-to-have, deferred.
Drill on raw (non-aggregated) tables — rows are already records, so no drill there. New filter
operators beyond `=` from drill (the chips bar can still show any persisted filter).

Decisions locked in brainstorming: bundle all three in one phase; drill = Metabase-style (both
"Filter by" + "See these records"); see-records → in-context drawer; config redesign covers **both**
tabs.

## Architecture

Same split as prior chart phases: a pure filter-builder in a small module (TDD), UI in components, and
`ChartExplore` owning the `filters` state + persistence. No re-query infra: drill reuses the existing
`useBuildChartData` → `/charts/data` → `useSubmitQuery`/`/queries` polling. `switch.tsx` already exists
(no new primitive). dnd-kit (added in 3c-3.6) drives the FieldZone reorder.

## 1. Drill-through

### 1.1 Pure filter-builder — `frontend/src/components/chart/drill.ts`

```ts
import type { ChartFilter } from "@/api/charts";

// one equality filter per (dimension, value) pair, skipping null path segments
export interface DrillContext { dims: { col: string; value: unknown }[] }

/** Pivot value cell → the dims of its (rowPath, colPath). */
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

/** Build the `=` filters for "Filter by" / "See records" from a drill context. */
export function drillFilters(ctx: DrillContext): ChartFilter[] {
  return ctx.dims.map((d) => ({ col: d.col, op: "eq" as const, value: d.value }));
}
```

### 1.2 Drill menu — `frontend/src/components/chart/DrillMenu.tsx`
A shadcn `Popover` (or `DropdownMenu`) anchored at the clicked cell. Props: `ctx: DrillContext`,
`onFilterBy: (filter: ChartFilter) => void`, `onSeeRecords: (filters: ChartFilter[]) => void`.
Renders:
- one **"Filter by {col} is {value}"** item per `ctx.dims` → `onFilterBy({col, op:"eq", value})`;
- a **"See these records"** item → `onSeeRecords(drillFilters(ctx))`.
When `ctx.dims` is empty (grand-total cell), show only "See these records" (no extra filters).

Each renderer (ChartTable value `<td>`, PivotTable value `<td>`) gets an `onClick` that computes the
`DrillContext` and opens the menu anchored there. The cell becomes a button-like trigger
(`cursor-pointer`); the renderer accepts an `onCellDrill?(ctx, anchorEl)` callback so the menu state
lives in ChartExplore (or a small local popover wrapper). Pivot only drills **leaf** value cells with
a non-empty `ctx`; subtotal/grand rows drill with the partial dims they carry.

### 1.3 See-records modal — `frontend/src/components/chart/DrillRecordsDialog.tsx`
A shadcn `Dialog` (modal — `dialog.tsx` already exists and is forwardRef-correct; no `Sheet`
primitive is added). Props: `open`, `onOpenChange`, `ws`, `datasetId`, `columns:
ResultColumn[]` (all dataset columns), `filters: ChartFilter[]`. On open it builds a **raw**
`QuerySpec` (`metrics: []`, `columns: <all column names>`, `dimensions: []`, `filters`, `orderby: []`,
`row_limit`, `rollup_groups: []`), runs it via `useBuildChartData` + `useSubmitQuery` +
`useQueryStatus`/`useResults`, and renders a `ChartTable` of the result. Loading / empty / failed
states handled in-drawer. Title summarizes the filters (e.g. "Records where priority = 1-URGENT").

### 1.4 Filter-chips bar — `frontend/src/components/chart/FilterChips.tsx`
Renders the active `filters` (the `ChartFilter[]`) as removable chips ("`{col}` = `{value}` ×"), shown
above the preview. Props: `filters`, `onRemove(index)`. Removing a chip re-runs.

### 1.5 Wiring (`ChartExplore.tsx`)
- New state `const [filters, setFilters] = useState<ChartFilter[]>([])`; hydrate from
  `p.spec?.filters ?? []`; reset on dataset change; **`specForRun()` sets `filters`** (instead of
  `[]`). Persisted automatically via `paramsForSave` (filters live in `spec`).
- **Filter by:** append the filter (dedupe by `col` — replace an existing eq on the same column),
  then `run()`.
- **See records:** open `DrillRecordsDialog` with the drill filters + the current dataset's columns.
- The `FilterChips` bar renders `filters`; removing re-runs.

## 2. Pivot full-width

`PivotTable.tsx`: the `<table>` gains `w-full`; the row-header `<th>` cells (the `scope="row"` cells
and the row-dim-name/corner headers) get `w-px whitespace-nowrap` so they shrink to content and the
slack flows into the value columns (right-aligned numbers expand acceptably). The exact look is tuned
live with screenshots to avoid the 3c-3.4 "sparse stretch" regression; if `w-px` slack distribution
still looks poor, fall back to a trailing flex spacer column. Sticky-left offsets (3c-3.6) are
unaffected (still pinned to the first row-dim column).

## 3. Config-panel redesign (Data + Display)

A shared visual system across both tabs:
- **Section header** component/style: uppercase, small, semibold, muted, with a subtle bottom divider
  — applied to every group (Dataset, Rows, Columns, Values, Totals, Formatting, Conditional, Columns).
- **`FieldZone`** — `frontend/src/components/chart/config/FieldZone.tsx`: replaces `OrderedPicker`.
  A labeled zone of **chips** (a grip icon + field name + `×` remove), drag-to-reorder **within the
  zone** via dnd-kit (`SortableContext` horizontal/vertical), and an **"+ Add"** affordance (a small
  button opening a popover list of the remaining fields, replacing the bare `Select`). Props:
  `label`, `selected: string[]`, `options: string[]`, `onChange`. Used for the pivot Rows / Columns /
  Values. (The table's Metrics/Columns checkbox lists get the same section-header + chip styling where
  it fits, but keep their checkbox semantics.)

### Display tab
- Toggles (Row totals / Column totals / Condense; and the table's Paginate) → shadcn **`Switch`**
  (`ui/switch.tsx` already exists) with a label, in a consistent row layout.
- Format / Conditional / Columns sections re-use the shared section-header style; the Format
  column-selector list is restyled toward Metabase's ColumnItem (data-type icon + name); conditional
  rules render as cleaner cards (color swatch + condition + ×). No behavior change — these wrap the
  existing `FormatSection` / `ConditionalFormattingSection` / `TableColumnsSection` logic.

## 4. Persistence / hydration
`filters` lives in `params.spec.filters` (already in `QuerySpec`) — `paramsForSave`/hydration need
only feed `filters` into `specForRun` and read it back. Pivot full-width and the config restyle are
presentational (no param change). No backend change.

## 5. Error handling
- Drill on a cell with no dims (grand total) → only "See these records"; an empty-filter raw query
  returns all rows (bounded by `row_limit`).
- Duplicate "Filter by" on the same column → replace the prior value (no stacking).
- Drawer query failure → in-drawer error; closing disposes the query state.
- FieldZone with a stale `selected` value not in `options` → still renders the chip (removable);
  options exclude already-selected.

## 6. Testing
Unit (Vitest):
- `drill.ts`: `pivotCellDrill` (leaf → all row+col dims; subtotal → partial; grand → none),
  `tableCellDrill` (dimension columns only, nulls skipped), `drillFilters` (one eq per dim).
- `DrillMenu`: renders a "Filter by" item per dim + "See these records"; fires the right callbacks.
- `FilterChips`: renders a chip per filter; remove fires `onRemove(i)`.
- `FieldZone`: renders a chip per selected; "+ Add" adds; `×` removes; reorder updates order.
- Existing renderer/engine/section tests stay green.

Manual Playwright smoke: pivot → click a leaf cell → "Filter by priority = 1-URGENT" → chip appears +
pivot narrows; click a cell → "See these records" → drawer shows raw rows; remove the chip → restores;
pivot fills full width; Data+Display tabs show the new chips/switches/section headers; Save → reopen →
filters hydrate.

## 7. File structure
- Create `frontend/src/components/chart/drill.ts` (+ test).
- Create `frontend/src/components/chart/DrillMenu.tsx` (+ test).
- Create `frontend/src/components/chart/DrillRecordsDialog.tsx`.
- Create `frontend/src/components/chart/FilterChips.tsx` (+ test).
- Create `frontend/src/components/chart/config/FieldZone.tsx` (+ test).
- Modify `frontend/src/components/chart/ChartTable.tsx` — value-cell drill onClick + `dimensionColumns` prop.
- Modify `frontend/src/components/chart/pivot/PivotTable.tsx` — value-cell drill onClick + `w-full` full-width.
- Modify `frontend/src/routes/explore/ChartExplore.tsx` — filters state + drill wiring + drawer + FilterChips + FieldZone for pivot + Display-tab restyle (Switch + section headers).
- Modify the Display settings sections (`settings/FormatSection.tsx`, `ConditionalFormattingSection.tsx`, `TableColumnsSection.tsx`) — styling only (section headers, Switch, ColumnItem look).
- Reused unchanged: `format.ts`, `conditional.ts`, `engine.ts`, the chart routes/repo/backend, and the existing UI primitives `ui/switch.tsx`, `ui/dialog.tsx`, `ui/popover.tsx`, `ui/dropdown-menu.tsx` (all present — no new shadcn primitive needed).
