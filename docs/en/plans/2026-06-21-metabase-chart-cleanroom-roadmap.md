# Metabase Chart Clean-Room Roadmap

**Goal:** bring egret's charts to Metabase fidelity (look + features) by *clean-room
reimplementation* — study Metabase's behavior, write fresh egret code. **No Metabase
(AGPL) source is copied**; only behaviors/look/UX are reproduced, so egret stays
Apache-licensable. Tests are adapted from the *intent* of Metabase's `*.unit.spec.ts`,
not copied.

Reference (read-to-understand only): `metabase/frontend/src/metabase/visualizations/echarts/`.

## Status legend
- [x] done · [~] in progress · [ ] todo

## Phase group 1 — Close fidelity gaps on existing cartesian + pie (Scope A)

- [x] **1a. Pie data robustness** — sort by value desc; drop zeros; negative handling
  (all-negative → abs, mixed → drop negatives); group sub-threshold slices into a
  single "Other" (≥2 only), muted color. (mb: pie/model/index.ts, constants.ts SLICE_THRESHOLD=2.5%)
  Done in `3cce13c`. Also fixed a latent bug (`b0e649a`): CSS-var chart colors don't
  resolve in ECharts' SVG attributes — added `resolveColor()` (gridlines/axis/Other
  were rendering black). Verified live.
- [ ] **1b. Pie donut polish** — centre total (show on hover / always), percent-visibility
  modes (inside / legend / both), auto decimal places for %. (mb: pie/option.ts, format.ts)
- [ ] **1c. Cartesian data shaping** — sort dimension (numeric/time) before plotting;
  drop null-dimension rows; per-series missing-value strategy (gap vs zero). (mb: cartesian/model/dataset.ts)
- [ ] **1d. Cartesian axes** — log / pow y-scale; dual Y-axis (left/right per metric);
  auto axis range. (mb: cartesian/model/axis.ts, option/axis.ts)
- [ ] **1e. Cartesian overlays** — goal line, trend line, stack totals labels,
  100%-normalized stacking. (mb: cartesian/option/{goal-line,trend-line}.ts, model/stack.ts)

## Phase group 2 — Full parity on existing families (Scope B)

- [ ] **2a. Series polish** — barMinHeight, connectNulls/step interpolation, emphasis/blur,
  negative-bar label placement, smart label density. (mb: cartesian/option/series.ts)
- [ ] **2b. Tooltips** — rich multi-series tooltip with formatted rows + % of total.
  (mb: echarts/tooltip/)
- [ ] **2c. Legend** — hidden-series toggle, legend values/percent. (mb: cartesian/model/legend.ts)

## Phase group 3 — New chart types Metabase has, egret lacks (Scope C)

One phase each, simplest first:
- [ ] 3a. Row chart (horizontal bar variant)
- [ ] 3b. Waterfall
- [ ] 3c. Funnel
- [ ] 3d. Progress
- [ ] 3e. Gauge
- [ ] 3f. SmartScalar (trend vs previous period)
- [ ] 3g. BoxPlot
- [ ] 3h. Sankey
- [ ] 3i. Map (region/pin) — largest; needs geo data

## Working rules
- TDD per phase: adapt Metabase spec intent → egret vitest (red) → implement → green.
- Each phase: `npm run typecheck && lint && build && test` green, then commit.
- Builder behavior first, then wire config into PieConfig/CartesianConfig + Display UI + ChartExplore.
