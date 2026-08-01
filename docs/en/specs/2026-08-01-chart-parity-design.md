# Chart parity with Metabase — design

**Date:** 2026-08-01
**Status:** approved, ready for planning
**Scope:** fix what is broken and close clear UX gaps; keep egret's deliberate differences

## Why this exists

The 19-chart redesign programme shipped every chart type, but comparisons up to now were made
against *different data*: egret's demo datasets are synthetic, Metabase's dashboard runs on its
Sample Database. Differences could not be attributed to the chart module because the numbers
themselves differed.

This design rests on a like-for-like harness built on 2026-08-01. Metabase query *results* were
pulled through its own `/api/dataset` and materialised in egret as nine virtual datasets built from
Trino `VALUES` literals, so both products receive byte-identical input. Both sides carry a dashboard
named **ZZ Chart Parity** holding all 19 chart types. Row-for-row equality was verified before any
comparison was made (e.g. `2026-01 → 205 orders / 11,094.70` on both).

Everything below was observed on that harness and then traced to code on both sides.

## The root cause

Twenty-six differences were catalogued. Almost all of them are one architectural gap.

Every decision about labels — rotate or not, how many ticks, where to truncate, how many gauge
graduations, whether a pivot body fits — is a decision *about size*. In egret, the option builders
cannot see their own size. The code says so directly, at
`frontend/src/components/chart/echarts/options/cartesian.ts:874`:

> the thing that decides whether labels fit is the axis WIDTH — which the option builder never sees

So it guesses from label *count* instead:

```ts
const rotate = !horizontal && longest > 10 && categories.length > 6 ? 30 : 0;
```

and delegates the rest to ECharts' `interval: "auto"`, which silently drops labels that collide.
On a one-third-width dashboard card with four short categories the guess declines to rotate, ECharts
drops three of the four labels, and the axis reads as missing data.

Metabase does not guess. It runs a layout pass that computes `boundaryWidth`, measures rendered text,
and only then builds the option.

### egret already has most of the pieces

| Piece | State in egret |
|---|---|
| Canvas text measurement | `components/chart/measure-text.ts` — ported from Metabase, attached to the document so `rem` resolves correctly, with an injectable `measurer` for tests |
| Option as a function of measured size | Working in production — but for **pie only** (`ChartRenderer.tsx`, `PieChart`) |
| Resize coalescing | `EChartsRenderer` already rAF-coalesces, with the performance trap documented |
| Grouped X/Y axis panel with a rotation field | `settings/ChartAxesSection.tsx` already has `xLabelRotation?: 45 \| 90`; used by waterfall, box plot and sankey |

The work is therefore mostly **extending an existing, proven pattern**, not building new machinery.

## Non-goals

Deliberately out of scope, per the agreed framing "fix bugs and close clear UX gaps, keep egret's
own character":

- Absolute setting-for-setting parity. Features Metabase has that egret does not — `graph.split_panels`,
  `graph.tooltip_columns`, pin/heat maps, `table.pivot`, up-to-three trend comparisons — stay out.
- Merging `ChartAxesSection` into cartesian's `AxesOptions`. The former's doc comment argues correctly
  that waterfall and box plot honour neither the x-scale select nor the y-axis split, and rendering
  them would put two settings on screen that do nothing. Only the *presentation* (X-axis / Y-axis
  grouping) is shared, not the field set.
- Places where egret is ahead: per-series colour and reordering inline, the Colors tab, progressive
  disclosure, empty states that name the missing field, and pivot/trend/gauge working on raw SQL where
  Metabase refuses.

## Design

### Workstream 1 — size-aware option builders (infrastructure)

Generalise the pattern `PieChart` already uses: the chart's `option` becomes a function of the
measured `EChartsSize` rather than a value. Extend it to the cartesian family, gauge and sankey.

**Corrected 2026-08-01 while planning:** an earlier draft of this section named funnel as a fourth
builder. It is not one — `options/funnel.ts` exports `funnelSteps` and `stepOpacity` and no option
builder, because the funnel has been a React trapezoid component since PR #378 and never goes through
`EChartsRenderer`. It measures its own container with a ref instead, and that work belongs with its
label-truncation fix in workstream 5. The one funnel path that *is* ECharts is `StepBars`, which
draws `funnel.type: "bar"` through `buildCartesianOption` and is covered by the cartesian change.

The pie precedent also carries the warning: building a size-dependent option can run a `measureText`
pass per element, so the option function's identity must be stable across unrelated re-renders.
`PieChart` exists as a separate component purely for that reason. The generalisation must preserve
it — memoise per viz, not inline an arrow in `ChartRenderer`.

This workstream changes no rendering behaviour. It exists so workstreams 2, 3 and 5 can be small.

**Testing.** Assert that a builder given an explicit size produces different options at 300px and
1200px. `measureText` returns zeroes under jsdom by design (see its doc comment: *"Size claims belong
in a browser"*), so tests inject a stub measurer and assert the *decision* (`rotate: 45`,
`show: false`, chosen interval), never a pixel.

### Workstream 2 — ordinal x-axis labels

Port Metabase's ladder from `echarts/cartesian/layout/index.ts`:

1. Auto-selection applies only while the setting is still its default and the scale is ordinal.
   An explicit user choice is honoured verbatim and never overridden.
2. `dimensionWidth = boundaryWidth / valuesCount`.
3. Overlap is tested **per adjacent pair**, on labels padded to be pessimistic:
   `leftW/2 + rightW/2 + GAP > dimensionWidth`, or either label alone exceeding
   `dimensionWidth - GAP/2`.
4. No overlap → draw all labels upright.
5. Overlap → `dimensionWidth >= fontSize * 2.1` gives 45°, else `>= fontSize * 1.2` gives 90°, in
   each case only if the rotated label height stays under 70% of chart height; otherwise hide the
   axis entirely.
6. At most 50 ticks are measured, for cost — sampled at **evenly spaced** indices
   (`getEvenlySpacedIndices`), not taken as a prefix: on a sorted axis the widest label is as
   likely to be the last as the first, and a prefix would leave it unmeasured.

**The load-bearing property is that labels are never partial.** All, rotated, or none. egret's
current `interval: "auto"` produces "some", which is the one outcome Metabase deliberately avoids.

Add `xLabelRotation?: 45 | 90` to `CartesianConfig`, alongside the existing `xAxisTicks?: boolean`,
and give the cartesian Axes tab the same four-value select `ChartAxesSection` already renders:
Hide / Show / Rotate 45° / Rotate 90°.

Metabase carries all of this in a single key (`graph.x_axis.axis_enabled`, which also has a fifth
"Compact" option). egret deliberately does not follow that here: waterfall, box plot and sankey
already store the two-field shape, and a cartesian family that stored a different shape for the same
control would fragment the settings model to match a foreign one. The select is a UI projection over
the two fields. "Compact" — which shortens labels rather than rotating them — is a separate feature
and stays out of scope.

**Fixes:** bar, row, box plot, waterfall.

### Workstream 3 — temporal and numeric x-axis

Two defects share a cause: egret's `xScale` defaults to `"ordinal"` and is only ever changed by hand,
so a date column is plotted on a category axis with raw string labels.

1. Detect a temporal column and select the timeseries scale automatically, the way Metabase decides
   axis type from the column rather than from a setting.
2. Port the tick-interval ladder from `echarts/cartesian/utils/timeseries.ts`: a fixed list of
   candidate intervals from 1 ms to 100 years; start at the index matching the data's own bucketing;
   walk up until `expectedTickCount <= maxTicksForChartWidth`; step back one if the result would
   yield fewer than two ticks.
3. `maxTicksForChartWidth` measures the **worst-case label for that unit** — via representative
   dates — not the actual data, plus a 10px buffer.
4. Name the period. A month bucket reads "January 2026", not "2026-01-01".
5. Apply the same "nice interval" treatment to a numeric x-axis.

**Fixes:** line, area, combo x-labels; scatter's `15.69 / 48.93 / 84.14` ticks; and the trend card
printing `December 1, 2026` where it should name the period.

### Workstream 4 — colour assignment

The palettes are already identical: egret's `SERIES_PALETTE` is Metabase's eight accents, and
`SUMMARIZE_COLOR = "#689735"` is exactly Metabase's `palm[50]`. The difference is *assignment*.

Add `colorsForKeys(names, existing?)` to `echarts/palette.ts`, mirroring Metabase's
`ui/colors/charts.ts`:

- Colour derives from a **hash of the series/column name**, so a given column keeps its colour across
  every card on a dashboard.
- Semantic names win over the hash: `success`/`profit`/`completed`/`active` → success green;
  `error`/`failed`/`loss`/`cost`/`cancelled` → error red.
- Beyond eight keys, extend the harmony rather than wrapping with `i % 8`, which currently gives two
  different series the same colour.
- Remove the single-series branch at `options/cartesian.ts:526`
  (`groups.length === 1 ? SUMMARIZE_COLOR : seriesColor(si)`). It is why eight of egret's nineteen
  charts render in the same olive green and a dashboard reads as monotone.

All option builders call this one function.

**Accepted consequence:** default colours are computed at render and not stored, so every existing
chart that has not set an explicit colour changes appearance once. This was decided deliberately —
a migration writing current colours into `seriesColors` was rejected as it would inflate stored
settings to preserve an appearance nobody chose.

### Workstream 5 — truncation and three local defects

- **Sankey** node labels truncate to one or two characters on the target side; **funnel** step names
  truncate to `A.` `C.` `P.` and its value row is clipped; **pie** slice labels overlap on the chart.
  All three are size decisions, so they follow workstream 1. `echarts/sliceLabel.ts` is the pie side
  and will be read in full during this workstream.
- **Gauge**: `options/gauge.ts:142` sets `axisLabel` without `splitNumber`, so ECharts uses its
  default of ten and draws eleven six-character labels around a small arc. Derive the graduation
  count from the measured radius.
- **Pivot in a small card**: renders correctly in the editor (verified: full 12×4 grid with grand
  totals matching Metabase) but collapses to header plus grand-totals row on a dashboard card. Needs
  to scroll within its card rather than shed its body.
- **Row chart**: the rotated axis title overlaps the category labels, and the value-axis ticks
  collide into `100K200K300K400K`.

### Workstream 6 — settings panel

- Group cartesian's Axes tab under X-axis / Y-axis headings, as `ChartAxesSection` already does.
  Presentation only; the field sets stay separate for the reason given under Non-goals.
- Expose pie's `showTotal`. The setting exists and defaults to on for multi-ring pies and off for
  single-ring — a deliberate choice, documented — but has no control in the panel, so a one-ring
  donut can never be given the centre total that Metabase shows by default.
- Replace pie's single "Percentages" select with explicit options matching Metabase's
  Off / In legend / On the chart / Both. The current single control is why slice labels and the
  legend both draw.
- Hide settings that do not apply, the way Metabase's `getHidden` does — stacking should not appear
  for a single series.
- Fix the numeric input rendering `2,5` for 2.5 in "Group slices below".
- Add the row count to the table footer, as Metabase's "Showing 4 rows" does.

### Workstream 7 — harness seed script

Commit a script that rebuilds the nine datasets and the 19-chart **ZZ Chart Parity** dashboard, so
the comparison survives a database reset and anyone can reproduce it.

It must encode the traps found while building it by hand:

- Chart metrics must be **declared dataset metrics** (`POST .../datasets/{id}/metrics` with an
  expression such as `SUM(revenue)`), not raw column names, or the chart returns
  `bad request: unknown metric`.
- Settings keys fail *silently* as an empty chart when wrong: pie takes `pie.dimensions` (a plural
  array), waterfall takes `waterfall.xAxis`, and box plot takes `boxplot.category` + `boxplot.value`
  over raw `columns` with no metric, because its quartiles are computed client-side and an aggregate
  collapses the distribution to a single point.
- Dataset creation takes `source: {kind: "virtual", sql}`.
- The dashboard PUT is a full replace and needs `name` alongside `cards`, whose geometry fields are
  `grid_col` / `grid_row` / `size_x` / `size_y`.

## Sequencing

Workstream 1 blocks 2, 3 and 5. Workstreams 4, 6 and 7 are independent.

| PR | Workstream | Blocked by |
|---|---|---|
| 1 | Size-aware option builders | — |
| 2 | Ordinal x-axis | 1 |
| 3 | Temporal / numeric x-axis | 1 |
| 4 | Colour assignment | — |
| 5 | Truncation + gauge, pivot, row | 1 |
| 6 | Settings panel | — |
| 7 | Harness seed script | — |

## Testing

Every fix carries a vitest that asserts the *rule*, not a rendered pixel:

- Size-dependent behaviour is tested by injecting a stub measurer and asserting the decision the
  builder reaches. jsdom runs no layout; `measure-text.ts` deliberately reports zeroes there so a
  test cannot accidentally appear to verify a fit.
- Colour is tested by asserting that the same name yields the same accent across two different chart
  builds, that a semantic name beats the hash, and that a ninth series does not repeat the first.
- The axis ladder is tested at its boundaries: just-fitting, needing 45°, needing 90°, and too tall
  for either, so the "hide the whole axis" branch is exercised rather than assumed.

Live verification runs on the seeded harness, side by side against Metabase. Screenshots are evidence
of a manual check, not an automated test, and will be described as such.

## Risks

- **Colours change for existing saved charts.** Accepted and decided; worth calling out in the PR
  description so it is not read as a regression.
- **Threading size through the builders can regress performance.** The pie precedent documents this
  exact trap. Mitigation is to keep the per-viz memo boundary rather than building option functions
  inline.
- **Auto-selecting a timeseries scale changes existing line and area charts** that currently render
  on a category axis. Ordering and spacing can shift where dates are irregular. Needs checking on the
  harness before merge.

## Corrections to earlier findings

Recorded so the earlier list is not cited as-is:

1. **"Pie has no Show total."** Wrong — `cfg.showTotal` exists and is defaulted deliberately. The
   gap is that the panel does not expose it.
2. **"`pie.slice_threshold` is a constant, not a setting."** Wrong — the panel has
   "Group slices below … %". The claim came from reading code without opening the UI.
3. **"egret has no label rotation at all."** Wrong — cartesian auto-rotates 30°, but only when the
   longest label exceeds ten characters *and* there are more than six categories, which is blind to
   the width that actually decides the question.
4. **Four charts initially rendered blank** in the harness (pie, waterfall, box plot, pivot). That
   was a settings-key error while authoring the comparison, not an egret defect; all four render once
   the correct keys are used.
