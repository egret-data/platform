# Phase 3c-3.5 — Table interactions (resize / show-hide / sticky / pin / pagination)

> Status: approved design. Canonical (English). Vietnamese mirror:
> `docs/vi/specs/2026-06-20-phase-3c-3-5-table-interactions-design.md`.
> Frontend-only. Builds on merged 3c-3.4 (`3b740c7`). Branch `feat/phase-3c-3-5-table-interactions`.

## Goal

Give the chart **Table** viz the column/table interactions a Metabase user expects, all
TanStack-table-native + CSS (no new dependency, no backend change — [[metabase-fidelity-preference]]):

1. **Column resize** — drag the header edge; widths persist.
2. **Column show/hide** — toggle columns in the Display tab.
3. **Sticky header** — header stays while the body scrolls vertically.
4. **Pin first columns (freeze left)** — first N columns stay while scrolling horizontally.
5. **Pagination** — optional; toggle + page-size selector.

Persisted in the chart `params` JSONB (like `columnSettings`), hydrated/saved by `ChartExplore`.

## Scope

**In scope (Table viz only):** the five interactions above on `ChartTable.tsx` (which already uses
`@tanstack/react-table`), a new `TableColumnsSection` in the Display tab, a `tableState` field on
`ChartParams`, and the hydrate/persist wiring in `ChartExplore`.

**Out of scope:** the **Pivot** viz (untouched — pivot uses a custom HTML table; its own interactions
= collapse/sticky/sort land in **3c-3.6**); column **drag-reorder** (deferred to 3c-3.6 — needs
dnd-kit); a **totals/grand-total footer** for the table (Metabase's regular Table has none — only
the Pivot has totals); any backend/engine/query change; persisting sort or page-index. Row
virtualization (unnecessary at the ≤1000-row cap).

Decisions locked during brainstorming: **Table only**; controls = **direct header resize + a Display
"Columns" manager**; pagination = **toggle, default OFF (scroll + sticky header), page size
25/50/100**.

## Architecture

Controlled state owned by `ChartExplore` (the persistence owner, exactly as it already owns
`columnSettings`/`conditionalFormatting`). A single `tableState` object is passed to **both** the
renderer (`ChartTable`) and the Display-tab manager (`TableColumnsSection`); both call the same
`setTableState`. `ChartTable` stays a controlled renderer — it maps `tableState` onto TanStack table
state and emits changes (resize) back up. No new dependency; persistence is JSONB only.

## 1. Persistence — `ChartParams += tableState`

`frontend/src/api/charts.ts`:

```ts
export interface TableState {
  columnWidths?: Record<string, number>;   // colName -> px (manual resizes only)
  hiddenColumns?: string[];                 // colName[] currently hidden
  pinnedLeft?: number;                      // freeze the first N columns (0 = none)
  pagination?: { enabled: boolean; pageSize: number };  // pageSize in {25, 50, 100}
}

export interface ChartParams {
  // …existing fields…
  tableState?: TableState;   // table viz only; ignored for pivot
}
```

Keys are **column names**, not indexes (robust, matches `columnSettings`/conditional keying). No
backend change — `params` is a JSONB blob round-tripped verbatim.

## 2. `ChartTable.tsx` — controlled renderer

Props add `tableState?: TableState` and `onTableStateChange?: (next: TableState) => void`. The
component keeps its local `sorting` state; everything else derives from `tableState`.

**Reconciliation (defensive):** before building table state, drop keys for columns that no longer
exist and clamp `pinnedLeft` to the column count — so a stale saved `tableState` after a
dataset/column change can never crash or hide the wrong column.

- **Resize** — `columnResizeMode: "onChange"`, `enableColumnResizing: true`; a resize handle on each
  header cell. Derive `columnSizing` from `tableState.columnWidths` (name → px, mapped to the
  `col_${i}` ids). `onColumnSizingChange` writes the new widths back **by column name** via
  `onTableStateChange`. Min width `MIN_COL_WIDTH = 60`.
- **Show/hide** — derive `columnVisibility` from `tableState.hiddenColumns` (hidden = `false`).
  Toggled from the Display manager (not from the header in this phase).
- **Sticky header** — the table sits in a bounded scroll container (`max-h-[60vh] overflow-auto`
  on the existing bordered frame); `thead th` gets `sticky top-0 z-10` + the existing `bg-muted/40`
  so body rows scroll underneath.
- **Pin left** — `columnPinning.left` = the first `pinnedLeft` column ids. Pinned cells get
  `sticky left-0 z-20` with a computed `left` offset (sum of preceding pinned widths), an opaque
  background, and a right border to delineate the frozen block. The top-left cell (pinned column in
  the sticky header) needs the highest z-index (`z-30`).
- **Pagination** — when `tableState.pagination?.enabled`, add `getPaginationRowModel()` and a
  controlled `pagination` state seeded from `pageSize`; render a footer below the table with
  "Showing X–Y of Z", Prev/Next buttons, and the page-size value. Page index is **local** (resets to
  0 on change; not persisted). When disabled, render all rows (current behavior).

If `ChartTable` grows unwieldy, extract the table-instance setup into a `useChartTableInstance` hook
in the same folder — keep the JSX renderer focused.

## 3. `settings/TableColumnsSection.tsx` — new (Display tab)

A controlled section mirroring `FormatSection`/`ConditionalFormattingSection`:

```tsx
export function TableColumnsSection({
  columns, value, onChange,
}: { columns: ResultColumn[]; value: TableState; onChange: (next: TableState) => void }) { … }
```

- **Columns** — one checkbox per result column (checked = visible); toggling updates
  `hiddenColumns`.
- **Freeze first N columns** — a small number input (0…columns.length) → `pinnedLeft`.
- **Pagination** — a toggle (`pagination.enabled`); when on, a page-size `Select` (25/50/100) →
  `pagination.pageSize`.
- Empty-state hint ("Run the chart to configure columns") when there are no result columns, same as
  the other Display sections.

## 4. `ChartExplore.tsx` wiring

- New state `const [tableState, setTableState] = useState<TableState>({})`.
- Hydrate: in the saved-chart effect, `setTableState(p.tableState ?? {})`.
- Reset: `resetControls()` also clears `tableState` to `{}` (dataset change).
- Persist: `paramsForSave()` includes `tableState` for the **table** branch
  (`{ spec, columnSettings, conditionalFormatting, tableState }`); the pivot branch is unchanged.
- Display tab: when `viz === "table"`, render `<TableColumnsSection columns={resultColumns}
  value={tableState} onChange={setTableState} />` above the Format/Conditional sections.
- Preview: pass `tableState={tableState}` and `onTableStateChange={setTableState}` to `<ChartTable>`.

## 5. Error handling

- Stale `tableState` after a column/dataset change → reconciled in `ChartTable` (drop unknown keys,
  clamp `pinnedLeft`); `TableColumnsSection` only lists current columns.
- Sticky requires a bounded container → the frame carries `max-h-[60vh]`; if the table is shorter it
  simply doesn't scroll.
- Pinned-column + sticky-header z-index handled explicitly (header z-10, pinned z-20, pinned-header
  z-30).
- Pagination page-size always one of {25, 50, 100}; page index clamped by TanStack.

## 6. Testing

Unit (Vitest + jsdom, colocated under `__tests__`):
- **ChartTable**: a hidden column does not render; `pinnedLeft=1` puts a sticky class on the first
  column's cells; with pagination enabled + pageSize 25 only 25 body rows render and the footer shows
  the range; clicking Next advances the page; a simulated column-sizing change calls
  `onTableStateChange` with the width keyed by column name; stale `tableState` keys (unknown column)
  are ignored without error.
- **TableColumnsSection**: renders a checkbox per column; unchecking emits `hiddenColumns`; changing
  freeze-N emits `pinnedLeft`; toggling pagination + page size emits `pagination`.
- Existing renderer/section/engine/gallery tests stay green.

Manual Playwright smoke: resize a column → Save → reopen → width restored; hide a column → gone and
persists; freeze the first column → it stays on horizontal scroll; enable pagination (page size 25)
→ footer + Next works; vertical scroll → header stays stuck.

## 7. File structure

- Modify `frontend/src/api/charts.ts` — add `TableState` + `ChartParams.tableState`.
- Modify `frontend/src/components/chart/ChartTable.tsx` — resize/visibility/pin/sticky/pagination
  (optionally extract `useChartTableInstance.ts`).
- Create `frontend/src/components/chart/settings/TableColumnsSection.tsx` (+ test).
- Create/extend `frontend/src/components/chart/__tests__/ChartTable.test.tsx`.
- Modify `frontend/src/routes/explore/ChartExplore.tsx` — `tableState` state + hydrate + persist +
  Display-tab section + ChartTable props.
- Reused unchanged: `format.ts`, `conditional.ts`, `PivotTable`, `FormatSection`,
  `ConditionalFormattingSection`, the chart routes/repo/backend.
