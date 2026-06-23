# Tier P1 — Export chart (CSV + PNG) (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-23 · **Branch:** `feat/chart-export` (off main)
**Part of:** the "polish & usability" roadmap phase (P1 of P1 export → P2 drill-to-dashboard/question + per-column click → P3 date Exclude tab; time range/grain deferred to a later time-series phase).

## Goal

Let users download a chart's data as **CSV** and its rendered image as **PNG**, from both the **chart builder** and **dashboard cards** — closing a gap vs Metabase/Superset (egret only exports from SQL Lab today). Frontend-only: the data is already client-side and the SVG is rasterized in-browser; no backend or migration.

## Scope

**In:** (1) a `rowsToCsv` + download util; (2) an `svgToPngBlob` rasteriser; (3) a `ChartExportMenu` (CSV always; PNG only for ECharts viz); (4) wiring into the chart builder (`ChartExplore`, next to Run/Save) and dashboard cards (`DashboardCardView`, view-mode hover affordance).

**Out / deferred:** an "export full result" re-run beyond the query's `row_limit` (MVP exports the displayed rows); PNG for table/pivot (they have no `<svg>` — CSV only); Excel/XLSX; server-side export; scheduled/emailed exports.

## Existing code (verified)

- `frontend/src/components/chart/ChartRenderer.tsx`: `ChartRenderer({ vizType, params, columns, rows, ... })` where `columns: ResultColumn[]` (`{name, type}`) and `rows: unknown[][]`. For chart viz it renders `EChartsRenderer`; table → `ChartTable`; pivot → `PivotTable`.
- `frontend/src/components/chart/echarts/EChartsRenderer.tsx`: `init(el, null, { renderer: "svg" })` — renders an `<svg>` into a `<div ref>`; no `foreignObject`.
- `frontend/src/routes/explore/ChartExplore.tsx`: the builder; has a Run button + result `columns`/`rows` state + a Save button; renders the preview pane.
- `frontend/src/components/chart/dashboard/DashboardCardView.tsx`: `ChartCardTile` fetches data via `useChartData` → `{ columns, rows, isLoading, error }` and renders `ChartRenderer`.
- SQL Lab already does a CSV download via an `<a href={csvUrl} download>` (`ResultsGrid.tsx`) — a precedent for the download affordance, but that is query-id/server-based; chart export is client-side.

## Components

### 1. `frontend/src/components/chart/export-chart.ts`
```ts
import type { ResultColumn } from "@/api/charts"; // {name: string; type: string}

/** RFC-4180 CSV: header row + one row per data row; quote fields containing , " or newline. */
export function rowsToCsv(columns: ResultColumn[], rows: unknown[][]): string { … }

/** Sanitize a title into a safe filename stem (fallback "chart"). */
export function exportFilename(title: string | undefined, ext: string): string { … }

/** Trigger a client-side download of a Blob. */
export function downloadBlob(filename: string, blob: Blob): void { … }

export function downloadCsv(title: string | undefined, columns: ResultColumn[], rows: unknown[][]): void {
  downloadBlob(exportFilename(title, "csv"), new Blob([rowsToCsv(columns, rows)], { type: "text/csv;charset=utf-8" }));
}

/** Rasterise an <svg> element to a PNG Blob via an Image → canvas (ECharts SVG has no foreignObject → no taint). */
export function svgToPngBlob(svg: SVGElement, scale?: number): Promise<Blob> { … }

export async function downloadPng(title: string | undefined, svg: SVGElement): Promise<void> {
  downloadBlob(exportFilename(title, "png"), await svgToPngBlob(svg));
}
```
`rowsToCsv` rules: header = `columns.map(c => c.name)`; each cell: `null`/`undefined` → empty; otherwise `String(v)`; quote (and double internal `"`) when the cell contains `,`, `"`, or `\n`. Join cells with `,`, rows with `\r\n`.

### 2. `frontend/src/components/chart/ChartExportMenu.tsx`
A small **Download** control (a dropdown/menu, lucide `Download` icon):
```ts
interface Props {
  title: string | undefined;
  columns: ResultColumn[];
  rows: unknown[][];
  containerRef: React.RefObject<HTMLElement>;   // the element wrapping the chart render
  disabled?: boolean;
}
```
- Renders a **Download CSV** item → `downloadCsv(title, columns, rows)`.
- Renders a **Download PNG** item **only** when `containerRef.current?.querySelector("svg")` exists (i.e. an ECharts viz is rendered) → `downloadPng(title, svg)`. For table/pivot (no `<svg>`) the PNG item is hidden.
- `disabled` (no result yet / loading / blocked) → the trigger is disabled.
- PNG availability is computed on open (read the DOM then), so it reflects the currently-rendered viz.

## Wiring

### Chart builder (`ChartExplore.tsx`)
- Wrap the preview/chart area in a ref'd container (or reuse an existing wrapper) — `previewRef`.
- Render `<ChartExportMenu title={name} columns={resultColumns} rows={results.data?.rows ?? []} containerRef={previewRef} disabled={!hasResults} />` next to the Run/Save controls.

### Dashboard card (`DashboardCardView.tsx` / `ChartCardTile`)
- In view mode (not editing), show a hover affordance in the card corner: `<ChartExportMenu title={cardTitle} columns={data.columns} rows={data.rows} containerRef={tileRef} disabled={data.isLoading || !!data.error || blocked} />`, where `tileRef` wraps the `ChartRenderer`. `cardTitle` = the card's title override or the chart name.
- The affordance is hidden while loading/blocked; it must carry the `dash-nodrag` class so react-grid-layout doesn't hijack the click (consistent with the existing edit toolbar).

## Data flow

```
CSV  : ChartRenderer's columns + rows → rowsToCsv → Blob('text/csv') → downloadBlob
PNG  : containerRef.querySelector('svg') → svgToPngBlob (serialize → Image → canvas → toBlob('image/png')) → downloadBlob
table/pivot (no <svg>) → CSV only
```

## Error handling

- No rows → CSV is header-only (still downloads). Export trigger disabled while `isLoading`/`error`/`blocked` or before the first result.
- Table/pivot (no `<svg>`) → PNG item not shown; CSV always shown.
- `svgToPngBlob` rejects (Image load / `toBlob` null) → surface a toast (`toast.error`), and never block the CSV path.
- The download affordance on dashboard cards uses `dash-nodrag` to avoid RGL drag hijack.

## Testing

- **Vitest:**
  - `rowsToCsv`: header row; comma/quote/newline quoting (`a,b` → `"a,b"`, `he"llo` → `"he""llo"`); null/undefined → empty; numbers/strings stringified; CRLF row separator.
  - `exportFilename`: sanitises unsafe chars, applies the extension, falls back to `"chart"`.
  - `ChartExportMenu`: with an `<svg>` in the container, both CSV + PNG items render; without an `<svg>`, only CSV; `disabled` disables the trigger; clicking CSV calls the download path (mock `downloadBlob`/URL.createObjectURL).
  - (`svgToPngBlob`/PNG rasterisation needs canvas+Image, unsupported in jsdom → covered by the live test, not Vitest.)
- **Live (Playwright MCP, 0 console errors, throwaway dashboard, NOT "a1"):** chart builder → Download CSV (correct headers+rows) + Download PNG (a `.png` downloads) on an ECharts chart; a **table** chart → CSV only (no PNG item); a dashboard chart card (view-mode hover) → CSV + PNG; a table card → CSV only.
- **Full sweep** before each commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Conventions

Metabase fidelity (PNG + CSV from chart + card) · clean-room · TDD · full sweep before each commit · configured git identity + `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage explicit paths · docs bilingual (en canonical + vi reference) · squash-merge · branch `feat/chart-export`.
