# Phase 3c-3.8 — ECharts Visualizations (design)

**Status:** approved (brainstorm) · **Date:** 2026-06-21 · **Builds on:** 3c-3.7 (drill-through + config redesign, merged PR #18)

## Goal

Add chart visualizations to the chart Explore UI — **bar, line, area, scatter,
combo, pie, big-number** — rendered with Apache ECharts, matching Metabase
closely in both behaviour and visual style (the same fidelity bar we held for
Table and Pivot).

## Why ECharts / why match Metabase

Metabase renders its charts with **ECharts 6 core** through a thin React
renderer (`init(el, null, { renderer: "svg" })`, `setOption` on change, resize
on size change, `dispose` on unmount, modules registered once). We mirror that
exact integration so we inherit ECharts' strengths (large datasets, rich
cartesian model) and reproduce Metabase's chart look using their published
style constants (palette, fonts, weights, opacities).

## Scope

In scope (one phase, ~12–14 tasks):

- **Cartesian family** — `bar`, `line`, `area`, `scatter`, `combo`. One shared
  config + one shared option builder; the chart kind and per-series type are
  variations on the same model.
- **Pie** — its own small config + builder.
- **Big number** (`scalar`) — a formatted single value; **not** ECharts.

Out of scope (later phases): dashboards (3c-4), trend/goal lines, multiple
y-axes, drill-through *from* charts (charts are display-only this phase; drill
stays a Table/Pivot feature), funnel/map/sankey/waterfall, saved color
overrides per series.

## Architecture

### Dependency

Add `echarts@^6` (matches Metabase's major). Import from `echarts/core` and
register only the modules we use — never the `echarts` umbrella — so the bundle
stays tree-shaken.

### `EChartsRenderer` (thin React wrapper)

`src/components/chart/echarts/EChartsRenderer.tsx`, mirroring Metabase's
`EChartsRenderer`:

- `init(el, null, { renderer: "svg" })` on mount.
- `setOption(option, true)` (notMerge) on mount and whenever `option` changes.
- A `ResizeObserver` on the container calls `chart.resize()` (Metabase passes
  explicit width/height; we use ResizeObserver so the chart fills its flex
  parent responsively).
- `dispose()` on unmount; guard against double-dispose so React 18 StrictMode's
  double mount is safe.
- Props: `{ option: EChartsCoreOption; className?: string }`. Event handling is
  out of scope this phase (charts are display-only).

### Module registration

`src/components/chart/echarts/register.ts` exports `registerEChartsModules()`
called once at renderer module load. Registers: `SVGRenderer`;
`BarChart, LineChart, ScatterChart, PieChart`; `GridComponent, TooltipComponent,
LegendComponent, DatasetComponent, MarkLineComponent` (markline reserved, unused
this phase). Area is `LineChart` + `areaStyle`; combo is per-series
`bar`/`line`.

## Data model

### VizType

Extend `VizType` in `src/api/charts.ts`:

```ts
export type VizType =
  | "table" | "pivot_table"
  | "bar" | "line" | "area" | "scatter" | "combo" | "pie" | "scalar";
```

Add gallery entries to `chartTypes.ts` with lucide icons: `BarChart3` (bar),
`LineChart` (line), `AreaChart` (area), `ScatterChart` (scatter), `Spline`
(combo), `PieChart` (pie), `Hash` (scalar). Each entry keeps the existing
`{ key, name, description, icon }` shape so `ChartTypeGallery` needs no change.

### ChartParams

Add three optional, viz-specific sub-objects (parallel to how pivot fields hang
off `ChartParams` today). Only the one matching the active viz is populated:

```ts
export interface CartesianConfig {
  xAxis?: string;            // single dimension (category / x)
  yMetrics: string[];        // one or more metrics (y)
  series?: string;           // optional 2nd dimension (breakout → one series each)
  stacked?: boolean;         // bar/area
  horizontal?: boolean;      // bar
  smooth?: boolean;          // line/area
  showLabels?: boolean;      // data labels on points/bars
  showLegend?: boolean;      // default true when series or >1 metric
  xLabel?: string; yLabel?: string;
  seriesTypes?: Record<string, "bar" | "line">;  // combo only: metric → type
}
export interface PieConfig {
  dimension?: string;        // category
  metric?: string;           // value
  donut?: boolean;
  showLegend?: boolean;      // default true
  showLabels?: boolean;      // slice % labels
}
export interface ScalarConfig {
  metric?: string;
}
// on ChartParams:
cartesian?: CartesianConfig;
pie?: PieConfig;
scalar?: ScalarConfig;
```

### Query derivation (`specForRun`)

`ChartExplore.specForRun()` derives the `QuerySpec` from the active viz config:

- **Cartesian:** `dimensions = [xAxis, series].filter(Boolean)`,
  `metrics = yMetrics`, plus existing `filters`/`row_limit`.
- **Pie:** `dimensions = [dimension]`, `metrics = [metric]`.
- **Scalar:** `dimensions = []`, `metrics = [metric]` (single aggregated row).

The backend `chartquery::build` already supports this (group-by + aggregates);
no backend change.

## Option builders (pure, unit-tested)

Pure functions taking `(columns, rows, config)` and returning an
`EChartsCoreOption` — no React, no ECharts side effects — so they unit-test like
the pivot engine and drill builders.

### `buildCartesianOption(columns, rows, cfg)`

`src/components/chart/echarts/options/cartesian.ts`:

- Resolve column indices for `xAxis`, each `yMetric`, and `series`.
- **No series dim:** one ECharts series per y-metric; category axis = distinct
  x values in row order.
- **Series dim set:** pivot rows into one ECharts series per distinct series
  value (for the first y-metric; multiple metrics × series is out of scope —
  when `series` is set we use a single y-metric, documented in the config
  guard). Category axis = distinct x values; missing (x, series) cells → null.
- `kind` maps to series `type`: bar/line/scatter; `area` = line + `areaStyle`
  (opacity 0.3); `combo` = per-metric type from `seriesTypes` (default bar).
- `stacked` → set `stack: "total"` on bar/area series. `horizontal` → swap
  x/y axis (category on y). `smooth` → line smoothing. `showLabels` →
  `label.show` with series-label style.
- Tooltip `trigger: "axis"` (item for scatter), legend bottom when
  `showLegend`. Value formatting via `columnSettings[metric]` + `formatValue`
  reused in axis/tooltip/label formatters.

### `buildPieOption(columns, rows, cfg)`

`src/components/chart/echarts/options/pie.ts`: one pie series, `data` = rows
mapped to `{ name: dimensionValue, value: metricValue }`; `donut` →
`radius: ["45%", "70%"]` else `["0%", "70%"]`; labels show name + percent when
`showLabels`; legend bottom.

### Big number

`src/components/chart/BigNumber.tsx` — not ECharts. Renders the single
aggregated value large and centered, formatted via the metric's `ColumnFormat`
(`formatValue`), with the metric name as a caption. Mirrors Metabase's Number
viz layout (value 2–2.5rem bold, caption muted).

## Styling — Metabase fidelity (concrete values)

`src/components/chart/echarts/palette.ts` + a shared `chartTheme` object feed
the builders. Values lifted from Metabase:

- **Series palette** (cycled): `#509EE3, #88BF4D, #A989C5, #EF8C8C, #F9D45C,
  #F2A86F, #98D9D9, #7172AD`.
- **Fonts:** axis ticks & names weight 400; axis label font 13px; data/series
  labels weight 700, size 13, offset 4.
- **Bars:** `barWidth` ratio 0.8; bar series z below line/area.
- **Scatter:** `symbolSize` 6, opacity 0.8. **Area:** fill opacity 0.3.
- **Line size:** default M = 2px.
- **Grid padding:** x 8, y 12; axis-name margin 12.
- **Grid/axis lines:** use the egret theme border token (light hairline,
  matching Metabase's `--mb-color-cartesian-grid-line`); splitLine only on the
  value axis (Metabase shows horizontal gridlines, hides vertical for bar/line).
- **Tooltip/legend:** SVG renderer; legend at the bottom; tooltip default
  ECharts card with themed text.

Container: chart fills a fixed-height responsive area in the Explore preview
(same `max-h` region the table uses), `rounded-md border` to match Table/Pivot.

## Display tab (per viz)

Reuse the existing settings panel patterns (SectionLabel, Switch, Select,
FormatSection):

- **Cartesian:** Switches — Legend, Stacked (bar/area), Horizontal (bar), Smooth
  (line/area), Data labels. Inputs — X label, Y label. Combo: a small
  per-metric bar/line picker. Number formatting of metrics via `FormatSection`
  (drives tooltip/axis/labels).
- **Pie:** Switches — Legend, Donut, Show labels.
- **Scalar:** the metric's `FormatSection` (prefix/suffix/decimals/compact).

## Guards, empty & error states

A viz is renderable only when its required fields are set; otherwise show the
existing "Configure the controls on the left and click Run." hint (same pattern
as the pivot guard in `ChartExplore`):

- Cartesian: needs `xAxis` and ≥1 `yMetrics`.
- Pie: needs `dimension` and `metric`.
- Scalar: needs `metric`.

Query/loading/failed states reuse the existing run pipeline
(`useBuildChartData` → submit → poll → results). Empty result set → a centered
"No data" message inside the chart frame.

## Testing

- **Unit (Vitest):** `buildCartesianOption` (no-series multi-metric; series
  breakout; stacked; horizontal; area/scatter/combo type mapping; value
  formatting) and `buildPieOption` (slices, donut, percent) — pure, no DOM.
  `BigNumber` formatting. Config reducers / `specForRun` derivation per viz.
- **Component smoke:** `EChartsRenderer` mounts, calls `setOption`, disposes on
  unmount — with `echarts/core` mocked (no real canvas/svg measure in jsdom).
- **Live (Playwright):** one smoke per viz type — configure axes, Run, assert an
  SVG chart renders; toggle a Display option and see it reflected.

## File structure

```
src/components/chart/echarts/
  EChartsRenderer.tsx          # thin wrapper (init/setOption/resize/dispose)
  register.ts                  # registerEChartsModules() — one-time
  palette.ts                   # Metabase series palette + chartTheme constants
  options/cartesian.ts         # buildCartesianOption (+ __tests__)
  options/pie.ts               # buildPieOption (+ __tests__)
src/components/chart/BigNumber.tsx
src/components/chart/settings/   # new chart Display sections (cartesian/pie)
# modified:
src/api/charts.ts              # VizType + CartesianConfig/PieConfig/ScalarConfig
src/components/chart/chartTypes.ts   # gallery entries
src/routes/explore/ChartExplore.tsx  # config zones, specForRun, render switch
```

## Risks / notes

- **jsdom + ECharts:** ECharts needs real layout measurement; unit tests mock
  the renderer and test the pure builders instead. This is why builders are
  pure.
- **StrictMode double-mount:** dispose guard prevents "instance already
  disposed" warnings.
- **series + multi-metric:** when a breakout `series` is chosen we restrict to a
  single y-metric (documented guard) to keep the series model unambiguous.
