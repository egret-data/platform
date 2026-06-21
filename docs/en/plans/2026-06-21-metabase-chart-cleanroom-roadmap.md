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
- [x] **1b. Pie donut polish** — centre total (donut + Show total), percent-visibility
  modes (off / inside / both). Done in `b99a902` (registers GraphicComponent). Verified
  live. (legend-mode % + hover-swap of centre value deferred.)
- [x] **1c. Cartesian data shaping** — sort x by axis type (temporal chronological,
  numeric ascending, nominal preserved); drop null-dimension rows. Auto by column type,
  no new config. (mb: cartesian/model/dataset.ts) — per-series missing-value strategy
  (gap vs zero) deferred to 2a.
- [x] **1d. Cartesian axes** — dual Y-axis (per-metric left/right, second axis on the
  right with its own formatter; series get yAxisIndex) + log scale toggle. Wired into
  the Display tab. Verified live (combo: order_count line now legible on the right axis).
  (mb: cartesian/model/axis.ts, option/axis.ts) — pow scale + auto-range heuristics deferred.
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
