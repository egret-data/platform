# Metabase-faithful dashboard grid (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-23 · **Branch:** `feat/dashboard-edit-ux` (continues; folds into PR #39)
**Supersedes:** the resize item (⑤) of `2026-06-23-dashboard-edit-ux-design.md` — the ×3 render-scale hack is removed and replaced with Metabase's real grid algorithm. The other items of that tier (icons ①④, picker ②, sidebar ③, auto-height ⑥) stay.

## Goal

Make egret's dashboard grid feel like Metabase by copying Metabase's actual grid algorithm: a **dynamic, aspect-ratio row height**, **vertical compaction**, **6px margins**, **per-visualization default/min sizes**, a **Metabase-style SVG grid background**, and a **single SE resize handle**. egret already declares it "mirrors Metabase" and even defines the right formula (`gridRowHeight()`) but never used it. Frontend-only; the stored data model (24 cols, `grid_col/grid_row/size_x/size_y` in grid units) already matches Metabase and is unchanged.

## Reference — how Metabase does it (from `metabase_bk`, verified)

- `frontend/src/metabase/lib/dashboard_grid.js`: `GRID_WIDTH=24`, `GRID_ASPECT_RATIO=10/9`, `MIN_ROW_HEIGHT=40`, `DEFAULT_CARD_SIZE={width:4,height:4}`, `getPositionForNewDashCard(cards, w, h, width)` (scan left→right, top→bottom for the first non-intersecting slot), `intersects(a,b)`.
- `DashboardGrid.tsx` `getRowHeight()`: `rowHeight = max((width / GRID_WIDTH) / GRID_ASPECT_RATIO, MIN_ROW_HEIGHT)` (floor normally; ceil when a scrollbar is present). At 1200px → 45px.
- RGL props (`grid/GridLayout.tsx`): `Responsive` RGL, `cols={desktop:24,mobile:1}`, `margin={desktop:[6,6]}`, `containerPadding=[0,0]`, `compactType="vertical"`, `isDraggable/isResizable={isEditing && !isMobile}`, `draggableCancel="[data-dontdrag]"`, `autoSize={false}`, default resize handle (SE only). `cellSize = { width: (gridWidth - (cols-1)*marginX)/cols, height: rowHeight }`.
- `grid/utils.ts` `generateGridBackground({cellSize, margin, cols, gridWidth, cellStrokeColor})`: builds an SVG `<rect>` per column (`x = i*(cellWidth+marginX)`, size `cellWidth×cellHeight`) into a `url("data:image/svg+xml;utf8,...")`, applied as the grid's `background` only while editing.
- `src/metabase/dashboards/constants.cljc` `card-size-defaults`: per-viz `{:min {...} :default {...}}` (table 4×3 / 12×9, scalar 2×2 / 6×3, bar/line/area/pie/etc. 4×3 / 12×6, heading 1×1 / 24×1, text 1×1 / 12×3, link 1×1 / 8×1, sankey 4×3 / 16×10).
- Resize handle CSS: 40×40 hit area, SE corner, an `::after` border indicator at 6px inset, hidden (`opacity:0.01`) until card hover (`opacity:1`).

## Existing egret code (verified)

- `src/components/chart/dashboard-grid.ts`: `GRID_COLS=24`, `GRID_ASPECT_RATIO=10/9`, `GRID_MIN_ROW_HEIGHT=40`, `CARD_DEFAULT_SIZE={w:6,h:4}`, `CARD_MIN_SIZE={w:2,h:2}`, `gridRowHeight(containerWidth)=max(floor(width/24/aspect),40)` (defined, **unused**). PR #39 added `ROW_PX=30`, `ROW_SCALE=3`, `GRID_MARGIN_X=12`, `GRID_MARGIN_Y=4`, `toLayoutRows`, `fromLayoutRows`, `contentRowsFor` — **these scale-helpers are removed by this spec** (except `contentRowsFor`, which is reworked for the dynamic rowHeight).
- `src/routes/explore/DashboardView.tsx`: `const Grid = WidthProvider(RGL)`; the `layout` map (scaled via `toLayoutRows`); `onLayoutChange` (un-scaled via `fromLayoutRows`); `<Grid cols={GRID_COLS} rowHeight={ROW_PX} margin={[GRID_MARGIN_X,GRID_MARGIN_Y]} resizeHandles={['s','e','se']} draggableHandle=".dash-draghandle" draggableCancel=".dash-nodrag" …>` wrapped in `<div className="relative">` with `<GridOverlay>`; `interactingRef` + `applyAutoHeight`; `addCard`/`addVirtualCard` create cards (current default `CARD_DEFAULT_SIZE` 6×4 + a placement routine). `draggableHandle=".dash-draghandle"` (a top strip) — KEEP egret's drag-handle + `.dash-nodrag` approach (egret's analog of Metabase's `[data-dontdrag]`).
- `src/components/chart/dashboard/GridOverlay.tsx`: CSS repeating-gradient overlay (PR #39) — **replaced** by the SVG-pattern background.
- `src/components/chart/dashboard/AutoHeightMeasure.tsx`: ResizeObserver → `contentRowsFor(px)` (fixed 90) → `onRows`. **Reworked** to take the live rowHeight.
- `src/components/chart/chartTypes.ts`: `CHART_TYPES` with `key` + `icon`. egret viz keys include `table`, `pivot_table`, `bar`, `line`, `area`, `pie`, `scatter`, `row`, `scalar`, `smartscalar`, `gauge`, `progress`, `funnel`, `waterfall`, `combo`, `sankey`, `boxplot`, `trend`; plus card types `text`/`heading`/`link`.

## Components & design

### 1. `dashboard-grid.ts` — adopt Metabase math, drop the scale hack
- Keep `GRID_COLS=24`, `GRID_ASPECT_RATIO=10/9`, `GRID_MIN_ROW_HEIGHT=40`. Set `GRID_MARGIN: [number, number] = [6, 6]` and `GRID_PADDING: [number, number] = [0, 0]`.
- `gridRowHeight(width)` stays (already correct). Add `cellWidth(width) = (width - (GRID_COLS-1)*GRID_MARGIN[0]) / GRID_COLS`.
- **Remove** `ROW_PX`, `ROW_SCALE`, `GRID_MARGIN_X`, `GRID_MARGIN_Y`, `toLayoutRows`, `fromLayoutRows`.
- `contentRowsFor(px, rowHeight)` — rework to take the live rowHeight: `Math.max(1, Math.ceil(px / (rowHeight + GRID_MARGIN[1])))` (a card of `n` rows occupies `n*rowHeight + (n-1)*marginY` px; dividing by `rowHeight+marginY` is the correct inverse up to the edge term).
- Add `getPositionForNewDashCard(cards, size_x, size_y, width=GRID_COLS)` and `intersects(a,b)` — direct port of Metabase's `dashboard_grid.js` (returns `{col,row,size_x,size_y}`).
- Add `CARD_SIZES_BY_VIZ: Record<string, { default: {w,h}; min: {w,h} }>` — port Metabase's `card-size-defaults`, keyed by egret viz/card keys. Mapping notes: Metabase `pivot` → egret `pivot_table`; egret `trend`/`smartscalar` → scalar-like (min 2×2, default 6×3); egret `boxplot` (no Metabase entry) → generic chart (min 4×3, default 12×6). Card types `text`/`heading`/`link` included. A `sizeForViz(key)` helper returns the entry or a generic-chart fallback (`{default:{w:12,h:6}, min:{w:4,h:3}}`).

### 2. `DashboardGridLayout.tsx` (new) — the Metabase-style grid wrapper
A component that owns width measurement, rowHeight, the SVG background, and RGL — mirroring Metabase's `GridLayout`.
- Props: `{ editing, layout, onLayoutChange, onResizeStart/Stop, onDragStart/Stop, children }`.
- Measures container width via a ref + `ResizeObserver` (jsdom-guarded). `rowHeight = gridRowHeight(width)`; `cellW = cellWidth(width)`.
- Renders, behind the grid when `editing`, a `<div>` whose `background` is the generated SVG pattern (see §3); `position:absolute inset-0 pointer-events-none`.
- Renders plain `RGL` (NOT WidthProvider — we supply `width`) with: `width`, `cols={GRID_COLS}`, `rowHeight`, `margin={GRID_MARGIN}`, `containerPadding={GRID_PADDING}`, `compactType="vertical"`, `isDraggable={editing}`, `isResizable={editing}`, `draggableHandle=".dash-draghandle"`, `draggableCancel=".dash-nodrag"`, `layout`, `onLayoutChange`, and the four drag/resize start/stop handlers. Default resize handle (SE only) — do not pass `resizeHandles`.
- Exposes the live `rowHeight` to the parent (callback or render-prop) so auto-height can convert px→rows with the current rowHeight.

### 3. SVG grid background (replaces `GridOverlay.tsx`)
Add `generateGridBackground({ cellWidth, rowHeight, cols, marginX, marginY, stroke })` to `dashboard/grid-background.ts` (port of Metabase `grid/utils.ts`): one `<rect stroke=… fill=none x=i*(cellWidth+marginX) y=0 width=cellWidth height=rowHeight/>` per column, wrapped in an `<svg width=totalWidth height=rowHeight+marginY>` → `url("data:image/svg+xml;utf8,"+encodeURIComponent(svg))`. `stroke` uses an egret token via the **OKLCH-safe** form `var(--border)` (NOT `hsl(var(--border))`). Delete the old CSS-gradient `GridOverlay`.

### 4. `DashboardView.tsx` — use the wrapper, drop scaling
- Replace `const Grid = WidthProvider(RGL)` + the inline `<Grid>` with `<DashboardGridLayout editing={editing} layout={layout} onLayoutChange={…} onDrag/Resize…>{cards}</DashboardGridLayout>`.
- `layout` map: raw grid units — `{ i, x: c.grid_col, y: c.grid_row, w: c.size_x, h: c.size_y, minW: sizeForViz(vizKey).min.w (clamped ≤ size_x), minH: sizeForViz(vizKey).min.h (clamped ≤ size_y), ...(autoHeight ? {maxH: c.size_y} : {}) }`. (vizKey = chart viz_type for chart cards, else card_type.)
- `onLayoutChange`: write back raw `grid_col:l.x, grid_row:l.y, size_x:l.w, size_y:l.h` (no scaling), keeping the same-ref no-op short-circuit.
- `addCard(chart)`: size from `sizeForViz(chart.viz_type).default`, placed via `getPositionForNewDashCard(existingCards, w, h)`. `addVirtualCard(type)`: size from `sizeForViz(type).default` (heading 24×1, text 12×3, link 8×1), placed the same way.
- Keep `draggableHandle`/`.dash-nodrag`, the card hover toolbar (①), and the `interactingRef`/`applyAutoHeight` wiring (⑥).

### 5. Auto-height (⑥) on dynamic rowHeight
- `AutoHeightMeasure` takes a `rowHeight` prop and calls `contentRowsFor(px, rowHeight)`. `DashboardView` passes the live rowHeight (from the wrapper). The vertical lock (`maxH===minH===size_y`) is unchanged.

### 6. Resize handle styling (Metabase look)
Add CSS so the default SE handle is a subtle corner indicator: hidden (`opacity:0`) until the card is hovered in edit mode, a small bottom-right border glyph, `cursor: nwse-resize`. Scope to edit mode + the dashboard card. (egret has no global RGL handle styling today; add a small CSS module or Tailwind on the card wrapper.)

## Data flow

```
container width (ResizeObserver) → rowHeight = max((width/24)/(10/9), 40), cellW = (width-23*6)/24
layout (raw grid units) → RGL(width, rowHeight, margin[6,6], padding[0,0], compactType:vertical) → cards
onLayoutChange → grid_col/grid_row/size_x/size_y (raw)   [compaction may shift y on load — accepted]
new card → sizeForViz(viz).default + getPositionForNewDashCard → addCard
auto-height card → ResizeObserver px → contentRowsFor(px, rowHeight) → size_y (locked maxH=minH)
edit mode → SVG grid background (cellW × rowHeight cells) behind RGL
```

## Backward compatibility / risk

- **Existing dashboards (incl. a1) re-flow**: dynamic rowHeight makes cards aspect-proportioned (a 4-row card ≈ 4×45=180px vs the old 360px), and `compactType:"vertical"` packs cards upward on load (gaps collapse). This is the intended Metabase behavior and was explicitly accepted. Stored geometry on disk is unchanged until the user saves; positions only persist when they edit + save.
- `minW/minH` are clamped to never exceed the card's current `size_x/size_y` (RGL warns otherwise) — a card stored smaller than its viz min keeps its size but can't be shrunk further.
- jsdom has no `ResizeObserver`/layout: guard measurement (width 0 → rowHeight clamps to 40); the grid renders without the SVG bg in tests. SVG-bg correctness is live-verified.

## Testing

- **Vitest (unit):**
  - `gridRowHeight`: `gridRowHeight(1200)===45`, clamps to 40 for small widths, monotonic.
  - `cellWidth`: `(1200 - 23*6)/24` matches.
  - `contentRowsFor(px, rowHeight)`: boundaries at `rowHeight+marginY` (e.g. rh=45,my=6 → 51px→1, 52px→2, 0→1).
  - `getPositionForNewDashCard`/`intersects`: first-free-slot placement (empty → {0,0}; fills row; wraps).
  - `sizeForViz`: known viz returns Metabase size; unknown (`boxplot`) → generic fallback; `pivot_table`/`heading`/`text`/`link` mapped.
- **Live (Playwright MCP, 0 console errors, throwaway dashboard, NOT a1):** measure rowHeight ≈ `(gridWidth/24)/(10/9)` at the live width; margins 6px; a chart card's pixel proportions match a Metabase card of the same grid size (open both side-by-side); vertical compaction packs cards up; the SVG grid background shows in edit mode and aligns to cards; SE resize handle appears on hover and resizes; a new Table card is 12×9 and a new Scalar is 6×3; auto-height still fits + locks on the dynamic rowHeight. Confirm an existing dashboard (PARITY, read-only) re-flows but renders coherently.
- **Full sweep** before each commit (from `frontend/`): `npm run typecheck && npm run lint && npm run build && npx vitest run`. Update/remove tests that asserted the ×3 scale (the `dashboard-grid.test.ts` render-scale + pixel-identity cases are deleted; replaced by the formulas above).

## Conventions

Metabase fidelity (the explicit goal here) · clean-room port of Metabase's public algorithm (no copied proprietary code — reimplement the documented math) · TDD · full sweep before each commit · configured git identity + `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` · stage explicit paths · docs bilingual · folds into PR #39 · squash-merge.

## Task order (for the plan)

1. `dashboard-grid.ts`: Metabase constants/helpers (`gridRowHeight` wired, `cellWidth`, `contentRowsFor(px,rh)`, `getPositionForNewDashCard`/`intersects`, `CARD_SIZES_BY_VIZ`+`sizeForViz`); remove scale helpers; update/replace tests.
2. `grid-background.ts` (`generateGridBackground`) + delete `GridOverlay.tsx`.
3. `DashboardGridLayout.tsx` wrapper (width/rowHeight/SVG-bg/RGL, compactType, margins, SE handle, exposes rowHeight).
4. `DashboardView.tsx`: use the wrapper; raw-unit layout/onLayoutChange; per-viz new-card sizing + placement; auto-height on live rowHeight.
5. Resize-handle CSS (Metabase look).
6. Live verify vs Metabase + (the tier's) PR.
