# Dashboard edit UX polish (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-23 · **Branch:** `feat/dashboard-edit-ux` (off main)
**Part of:** the "polish & usability" roadmap phase. This is a self-contained tier addressing six dashboard-edit-mode UX issues raised from screenshots.

## Goal

Make dashboard **edit mode** feel polished and usable: consistent/legible card controls, a non-overwhelming "change chart" picker, a collapsible add-chart sidebar, cleaner tab controls, a Metabase-like resize experience (visible grid + finer steps + more handles), and an auto-height option for content cards. **Frontend-only** — no backend, no migration. Stored card geometry (`grid_col/grid_row/size_x/size_y`) is unchanged on disk; finer vertical granularity is achieved by a render-time transform.

## Scope

**In:** six items (numbered below). **Out / deferred:** auto-height for chart (ECharts) cards (needs a fixed height; excluded by decision); per-axis manual resize locking beyond what RGL's per-item `min/maxH` supports; backend grid migrations; drag-to-reorder changes; theming beyond icon swaps.

## Existing code (verified)

- `src/routes/explore/DashboardView.tsx` (~523 lines): owns edit state, the `react-grid-layout` `<Grid>` (cols 24, `rowHeight ROW_PX=90`, `margin [12,12]`, `draggableHandle .dash-draghandle`, `draggableCancel .dash-nodrag`, `isDraggable/isResizable={editing}`), the `layout` derivation (lines ~184-189), the per-card hover toolbar wrapper (lines ~393-402), the "Replace chart" `Dialog` (lines ~490-501), and the edit sidebar render (line ~423). `editPanel` state drives the sidebar (`kind: add|card|viz|filter`).
- `src/components/chart/dashboard/CardEditToolbar.tsx` (23 lines): emoji buttons (`⚙ ⧉ ↺ ✕`) + one lucide `BarChart3 h-3 w-3`; button class `rounded px-1 text-xs hover:bg-accent`.
- `src/components/chart/dashboard/AddPanel.tsx` (37 lines): search input + chart list; each row uses `ICON_BY_VIZ.get(viz_type) ?? BarChart3` (`h-4 w-4`); plus Text/Heading/Link buttons. `ICON_BY_VIZ` is defined here.
- `src/components/chart/dashboard/EditSidebar.tsx` (~60 lines): `<aside class="… w-[340px] …">`; renders AddPanel / card / viz / filter panels by `panel.kind`.
- `src/components/chart/dashboard/DashboardTabBar.tsx` (62 lines): tabs with emoji `⧉` (duplicate) + `✕` (delete) + `+` (add), shown when `editing`.
- `src/lib/dashboard-grid.ts` (or similar): `GRID_COLS=24`, `GRID_ASPECT_RATIO=10/9`, `GRID_MIN_ROW_HEIGHT=40`, `CARD_DEFAULT_SIZE={w:6,h:4}`, `CARD_MIN_SIZE={w:2,h:2}`, `gridRowHeight()` (defined, unused).
- `src/components/chart/dashboard/DashboardCardView.tsx`: `ChartCardTile` renders header + a `min-h-0 flex-1 overflow-auto` content container (`chartRef`) → ChartRenderer/ChartTable/PivotTable. Card geometry from `DashboardCard` (`api/dashboards.ts`).

## Components & design (per item)

### ① Card hover toolbar — legible, consistent icons
`CardEditToolbar.tsx`: replace the emoji + tiny lucide with a consistent lucide set:
- Settings `Settings`, change-viz `BarChart3` (kept), duplicate `Copy`, replace-chart `RefreshCw`, remove `Trash2`.
- Icons `h-4 w-4` (16px); each button `inline-flex size-7 items-center justify-center rounded hover:bg-accent` (remove text-xs emoji sizing); remove button keeps `text-destructive`.
- Add a `title` attribute per button (tooltip) mirroring the existing `aria-label`. Keep `dash-nodrag` on the toolbar.
- The toolbar wrapper position in DashboardView (`-top-3.5 … group-hover:opacity-100`) is unchanged; only its height grows slightly with the larger buttons (acceptable).

### ② "Change chart" picker — searchable icon list (shared component)
- New `src/components/chart/dashboard/ChartPickerList.tsx`: props `{ charts: Chart[]; onPick: (c: Chart) => void; autoFocus?: boolean }`. Renders a search input (`Search charts…`, live filter on `name`) + a scrollable list (`max-h-[60vh] overflow-auto`); each row = viz icon (`ICON_BY_VIZ`) + truncated name, `hover:bg-accent/60`, click → `onPick`. Empty state "No charts".
- Move `ICON_BY_VIZ` to a shared `src/components/chart/dashboard/viz-icons.ts` so both `AddPanel` and `ChartPickerList` import it (DRY). `AddPanel` keeps its Text/Heading/Link footer but delegates the chart list to `ChartPickerList` (its `onAddChart` becomes `onPick`).
- `DashboardView` "Replace chart" `Dialog` body becomes `<ChartPickerList charts={charts ?? []} onPick={(ch) => replaceCard(replacingId!, ch)} autoFocus />`. Bounded height + search removes the wall-of-pills.

### ③ Collapsible add-chart sidebar
- `DashboardView`: add `const [sidebarCollapsed, setSidebarCollapsed] = useState(false)`.
- When `editing && !sidebarCollapsed` → render `EditSidebar` (340px) as today. When collapsed → render a thin rail (`w-9`) with an expand button (`PanelLeftOpen` icon) instead; the grid column flexes to full width.
- `EditSidebar` gains a collapse button (`PanelLeftClose`) in its header → `onCollapse`.
- Auto-expand: any action that sets `editPanel` to `card|viz|filter` (e.g. clicking a card's Settings) also `setSidebarCollapsed(false)` so the requested panel is visible. Opening edit mode keeps current default (`editPanel = {kind:'add'}`), not collapsed.

### ④ Tab controls — lucide icons, hover-revealed
`DashboardTabBar.tsx`: replace `⧉`→`Copy` and `✕`→`X` (lucide, `h-3.5 w-3.5`), `+`→`Plus`. The per-tab duplicate/delete buttons become `opacity-0 group-hover:opacity-100` (wrap each tab item in `group`) so they appear only on hover of that tab — reducing the always-on clutter. Delete stays `disabled` when `< 2` tabs. The active tab may keep its actions visible for discoverability (optional; default: hover-only for all).

### ⑤ Resize — visible grid + finer steps + edge handles
Three coordinated changes, all in `DashboardView` + a small overlay component, **without changing stored geometry**:
- **Render-time vertical scale (finer steps):** introduce `ROW_PX = 30` and `ROW_SCALE = 3`. The `layout` mapping multiplies vertical units by `ROW_SCALE` (`y: c.grid_row*3`, `h: c.size_y*3`, scaled `minH`); `onLayoutChange` divides back (`grid_row: round(y/3)`, `size_y: max(1, round(h/3))`) before persisting. Net: each stored row = 3 drag steps of 30px (was 1 step of 90px), 3× finer vertically; a 6×4 stored card still renders at the same pixel size. Horizontal already fine (24 cols). Guard rounding so a no-op resize doesn't drift.
- **Grid overlay:** new `src/components/chart/dashboard/GridOverlay.tsx` — an absolutely-positioned layer behind `<Grid>`, shown only in edit mode, drawing faint cell lines via `background-image` repeating-linear-gradients at column width (`(containerWidth - 2*pad)/24`) and `ROW_PX` (30) intervals. Measures container width via a ref/ResizeObserver. Non-interactive (`pointer-events-none`).
- **Resize handles:** pass `resizeHandles={['s','e','se']}` to `<Grid>` (adds bottom + right edges to the existing corner). Handles only appear while `isResizable` (edit mode).

### ⑥ Auto-height for content cards (not charts)
- Eligible card types: `table`/`pivot` (via chart `viz_type`) and `text`/`heading`/`link`. Charts (other ECharts viz) are excluded.
- New per-card boolean `card.options.autoHeight`. A toggle appears in the **card settings panel** (`editPanel.kind==='card'`) only for eligible cards.
- When `autoHeight` is on: `DashboardCardView` measures its rendered content height (a `ResizeObserver` on the content container), converts to stored rows `ceil(contentPx / (ROW_PX*ROW_SCALE))` = `ceil(contentPx/90)`, and calls an `onAutoHeight(cardId, rows)` callback (passed from DashboardView) to update the card's `size_y` if it changed by ≥1 row (debounced; only when not actively dragging). The card's layout item gets `minH = maxH = size_y*ROW_SCALE` so RGL locks vertical resize (width still resizable). Manual vertical drag is therefore disabled for that card; turning autoHeight off restores free resize.
- Loop-safety: update only on a real change (≥1 row delta) and never during an active RGL drag/resize (`onLayoutChange` in progress) to avoid feedback oscillation.

## Data flow

```
Geometry (unchanged on disk): grid_col, grid_row, size_x, size_y  [stored units]
  render:  layout = { x, y: row*3, w, h: size_y*3, minH, [maxH if autoHeight] }   (rowHeight 30px)
  persist: onLayoutChange → grid_row = round(y/3), size_y = round(h/3)
autoHeight card: content ResizeObserver → ceil(px/90) rows → onAutoHeight → size_y (if changed) → maxH=minH locks vertical
ChartPickerList: charts → search filter → onPick → (AddPanel: addChart) | (Dialog: replaceCard)
sidebarCollapsed: toggles EditSidebar (340px) ↔ rail (w-9); opening a settings panel forces expanded
```

## Error handling / edge cases

- Render-scale rounding: `size_y` clamped to `≥ 1` stored row; round-trip (×3 then ÷3) of an untouched card must be identity — use `Math.round` and only write back changed items.
- Grid overlay must not capture pointer events (`pointer-events-none`) and must hide outside edit mode.
- Auto-height: guard against measure→resize→measure loops (≥1-row threshold, skip during active drag); a 0-height measurement (card not yet laid out) is ignored. Auto-height cards ignore manual vertical resize (locked via min/maxH).
- Tab delete still disabled at `< 2` tabs. Sidebar collapse must not lose the current `editPanel` (re-expanding shows the same panel).
- `ICON_BY_VIZ` move: keep the same map contents; both importers updated in the same change to avoid a broken import.

## Testing

- **Vitest (unit):**
  - `ChartPickerList`: filters by query; clicking a row calls `onPick` with that chart; empty state renders.
  - render-scale helpers: a pure `toLayoutRow(storedRow)=row*3` / `fromLayoutRow(layoutRow)=round/3` pair (extract to `dashboard-grid.ts`) — round-trip identity for sample values, clamp `≥1`.
  - auto-height helper: `contentRowsFor(px) = ceil(px/90)` (pure), boundary cases (89→1, 90→1, 91→2).
  - `CardEditToolbar`: the five buttons render with their `aria-label`s (icons present).
  - `DashboardTabBar`: duplicate/delete buttons exist per tab; delete disabled when one tab.
- **Live (Playwright MCP, 0 console errors, throwaway dashboard, NOT "a1"):** toolbar icons legible; Replace-chart shows a searchable list (type to filter, pick replaces); sidebar collapse/expand toggles width and a card Settings click re-expands; tab duplicate/delete on hover with lucide icons; resize shows the grid overlay and snaps in ~30px steps with bottom/right/corner handles; an existing dashboard's card pixel sizes are unchanged after the rowHeight/scale change; enabling auto-height on a table card fits its height to content and locks vertical resize.
- **Full sweep** before each commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`.

## Conventions

Metabase fidelity (resize grid, picker, auto-height) · clean-room · TDD · full sweep before each commit · configured git identity + `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage explicit paths · docs bilingual (en canonical + vi reference) · squash-merge · branch `feat/dashboard-edit-ux`.

## Task order (for the plan)

①+④ icon swaps → ② ChartPickerList + Replace dialog (+ ICON_BY_VIZ extract) → ③ sidebar collapse → ⑤ grid overlay + render-scale + handles → ⑥ auto-height → live verify + PR.
