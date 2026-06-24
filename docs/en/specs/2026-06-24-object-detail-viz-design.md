# Object Detail Visualization (Design)

**Status:** Approved approach, pending spec review
**Canonical:** this file. Vietnamese reference: `docs/vi/specs/2026-06-24-object-detail-viz-design.md`.

## Goal

Add an **Object detail** visualization: render a single record as a vertical list of `field: value`
pairs, with prev/next navigation to page through the queried records. This is Metabase's "Object detail"
and fills the gap where the E-commerce clone currently substitutes a table for "Most recent subscription".

**Frontend only** — the backend already serves raw-row queries (`spec.columns`, no aggregation).

## Decisions (from brainstorming)

- **Field selection:** the user picks which columns to show, reusing the table's column-checkbox picker
  (which writes the existing `tableCols` state).
- **Pagination:** the query returns multiple rows (up to the row limit); the viz shows one record at a
  time with prev/next controls and an "i / N" indicator.

## Components

### 1. Viz registration

- `frontend/src/api/charts.ts` — add `"object"` to the `VizType` union (it is NOT a cartesian viz, so
  `CARTESIAN_VIZ` / `isCartesianViz` are unchanged).
- `frontend/src/components/chart/chartTypes.ts` — add a `CHART_TYPES` entry:
  `{ key: "object", name: "Object detail", description: "Inspect one record as field/value pairs, paging through results.", icon: FileText }` (import `FileText` from lucide-react).

### 2. `ObjectDetail` component — `frontend/src/components/chart/ObjectDetail.tsx` (new)

Props: `columns: ResultColumn[]` (`{ name, type }`, from `ChartTable`), `rows: unknown[][]`,
`columnSettings?: Record<string, ColumnFormat>`.

Behavior:
- Internal `index` state (current record), starting at 0. A `useEffect` on `[rows]` resets `index` to 0
  when the data changes (a new query produces a new `rows` reference), so paging never points past the end.
- Empty `rows` → a centered "No data" message (matching the other viz empty states).
- Renders `rows[index]` as a vertical `<dl>` of rows: each column is `field (label) : value`, the value
  formatted via `formatValue(row[i], column.type, columnSettings[column.name])` (reusing
  `@/components/chart/format`, the same formatter the table uses, so number/date/currency formats match).
- A footer nav bar (only when `rows.length > 1`): a `‹` button (disabled at the first record), the
  indicator `i+1 / rows.length`, and a `›` button (disabled at the last record). `aria-label`
  "previous record" / "next record" for testability.
- Fills its container (`h-full`, scrollable field list) so it sits correctly in a dashboard card (the
  card sizes it; this follows the post-PR-#45 fill-the-card convention).

### 3. Render path — `frontend/src/components/chart/ChartRenderer.tsx`

Add a branch: `if (vizType === "object") return <ObjectDetail columns={columns} rows={rows} columnSettings={columnSettings} />;`
(`columnSettings` is already derived in `ChartRenderer`). Because dashboard cards and the builder preview
both render through `ChartRenderer`, Object detail works in both with no further wiring.

### 4. Builder wiring — `frontend/src/routes/explore/ChartExplore.tsx`

Object detail is a raw-row query over the picked columns, so it reuses the table's column machinery:

- **Controls:** add a control block shown when `viz === "object"` that renders ONLY the column-checkbox
  list (the same `columns.map(... Checkbox checked={tableCols.includes(...)} onCheckedChange={() =>
  toggleTableCol(...)} ...)` markup the table block uses for "Columns"). No metrics picker, no aggregate
  toggle, no temporal unit list — Object detail has none of those.
- **`specForRun()`:** add an `if (viz === "object")` branch BEFORE the table fallback, returning
  `{ metrics: [], dimensions: [], columns: tableCols, filters, orderby: [], row_limit: rowLimit, rollup_groups: [] }`
  (raw mode; `row_limit` controls how many records are pageable).
- **Hydration:** add `else if (c.viz_type === "object") { setTableCols(p.spec?.columns ?? []); }` to the
  hydrate chain (so a saved object chart restores its columns).
- **Validation:** in the run/submit guard chain, add `if (viz === "object" && tableCols.length === 0) {
  toast.error("Object detail needs at least one column"); return; }`.
- **Preview:** the existing preview routes any viz that is not `table`/`pivot_table` through
  `ChartRenderer`, so Object detail renders in the preview automatically (no change needed there).

## Data flow

1. User picks "Object detail", selects a dataset, checks the columns to show, clicks Run.
2. `specForRun()` sends a raw query (`columns: tableCols`, `row_limit`); the backend returns N rows.
3. `ChartRenderer` routes to `ObjectDetail`, which shows record 1 of N as field/value pairs; prev/next
   pages through the records.
4. Saved to a dashboard card, the same `ObjectDetail` renders inside the card (paging works there too).

## Error handling

- No data → "No data". `index` is clamped to `rows.length - 1` defensively in case a stale index outlives
  a smaller result.
- `formatValue` is the table's existing total formatter (never throws); an unknown type falls back to a
  string.

## Testing (Vitest)

- `ObjectDetail.test.tsx`: renders the first record's field/value pairs from `columns`+`rows`; clicking
  "next record" advances to the second record's values; the prev button is disabled on the first record
  and next is disabled on the last; the "i / N" indicator is correct; empty `rows` shows "No data"; the
  nav bar is hidden when there is exactly one row.
- Live verify (Playwright, throwaway chart, 0 console errors): build an Object detail on `ecom_orders`
  with several columns → record 1 shows field/value pairs; prev/next pages records; save it onto a
  throwaway dashboard card and confirm it renders + pages there. Delete the throwaway afterward. Full
  frontend sweep green.

## Files

- Modify: `frontend/src/api/charts.ts` (VizType), `frontend/src/components/chart/chartTypes.ts` (registry),
  `frontend/src/components/chart/ChartRenderer.tsx` (branch), `frontend/src/routes/explore/ChartExplore.tsx`
  (controls + specForRun + hydrate + validation).
- Create: `frontend/src/components/chart/ObjectDetail.tsx`, `…/__tests__/ObjectDetail.test.tsx`.

## Non-goals

- No backend change. No Map viz (separate project). No per-record deep-link / drill, no object-specific
  column formatting UI (reuses any existing `columnSettings`). Object detail reuses `tableCols`; switching
  between Table and Object detail keeps the column selection (acceptable — both are column-based).

## Conventions

Frontend sweep before each commit: `npm run typecheck && npm run lint && npm run build && npx vitest run`
(from `frontend/`). Configured git identity; `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
footer; stage explicit paths.
