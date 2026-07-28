# Chart Redesign 02 — Line

**Status:** approved design, pending implementation plan
**Date:** 2026-07-28
**Program spec:** `docs/en/specs/2026-07-28-chart-redesign-program-design.md`
**Previous unit:** `docs/en/specs/2026-07-28-chart-redesign-01-bar.md` (shipped, main `1fe27573`)
**Vietnamese reference:** `docs/vi/specs/2026-07-28-chart-redesign-02-line.md`

## What a line chart is for

Showing how one measure moves across an ordered dimension — usually time. Everything
below follows from that: the shape of the line has to be honest about what happened
between the points, gaps in the data must not be silently invented, and when several
lines share a chart they have to be distinguishable from each other.

Line is second because it is the second most used type, and because it inherits
everything bar built — grouped controls, render-time defaults, per-axis compact labels,
series order and visibility — without re-deriving any of it.

## What must be right before any configuration

Line already gets bar's render-time defaults through the shared cartesian builder: the
legend appears by itself, axis titles come from the column names, wide numbers shorten,
and label spacing is left to ECharts. This unit adds nothing there.

What line has of its own is a set of three settings that are booleans when they describe
more than two states:

| | Today | After |
|---|---|---|
| Line shape | two independent booleans, `smooth` and `step` | one choice: linear / smooth / step |
| Missing values | `connectNulls` boolean | interpolate / zero / none |
| Point markers | `showMarkers` boolean, always on | auto / on / off |
| Per-series appearance | colour and label only | + shape, style, size, markers |

## Three defects this fixes on the way

1. **`smooth` and `step` can both be true.** Nothing prevents it and the state means
   nothing — ECharts draws a stepped line and ignores the smoothing. One three-way choice
   removes the state rather than validating it away.

2. **The "Connect gaps" switch contradicts the chart.** The builder reads
   `cfg.connectNulls ?? true`, so an unconfigured chart connects across nulls; the panel
   renders `checked={!!config.connectNulls}`, so the switch reads OFF while the line is
   plainly connected. This is the same defect bar found in its Legend and compact-number
   switches, and it has the same cause: a render-time default forced into a two-state
   control. A three-way `missing` setting makes the default visible.

3. **A comment states the opposite of the code.** `showMarkers` is annotated "off by
   default, like Metabase" while the builder does `cfg.showMarkers !== false`, which shows
   markers unless explicitly disabled. Metabase's own default is `auto`. The code is what
   ships, so the comment goes and the default becomes auto for new charts.

## Settings

Chart-wide, in the shared `cartesian.` namespace:

```
cartesian.lineShape  : "linear" | "smooth" | "step"      default "linear"
cartesian.missing    : "interpolate" | "zero" | "none"   default "interpolate"
cartesian.markers    : "auto" | "on" | "off"             default "auto"
```

Per series, overriding the chart-wide value where present:

```
cartesian.seriesLine : Record<seriesName, {
                         shape?:   "linear" | "smooth" | "step"
                         style?:   "solid" | "dashed" | "dotted"
                         size?:    "S" | "M" | "L"
                         markers?: "auto" | "on" | "off"
                       }>
```

`seriesLine` follows the shape of the existing `seriesColors` and `seriesLabels` maps, so
the per-series panel extends what is already there rather than inventing a second pattern.
`style` and `size` are per-series only: on a single-line chart there is nothing to
distinguish, so a chart-wide dashed-ness would be decoration rather than information.

**What "auto" means for markers.** Points are drawn when there are few enough for them to
read as points rather than as a thick line. The threshold belongs in the option builder,
where the data is, and is tuned against a real chart in the live check — not guessed here.

## Migration 0101

Existing charts carry `cartesian.smooth`, `cartesian.step`, `cartesian.connectNulls` and
`cartesian.showMarkers`. The migration rewrites them and must preserve how each chart
already looks. Two cases are not mechanical:

- **`step` wins over `smooth`.** Both can be true today, and ECharts renders that as a
  stepped line. Mapping `smooth` first would silently restyle those charts.
- **An unset `showMarkers` becomes `"on"`, not `"auto"`.** The builder's current default
  is to show markers, so leaving it unset — or writing `"auto"` — would change the
  appearance of every existing line chart. New charts get `"auto"`; existing ones get the
  explicit value that matches what they already draw.

`connectNulls` maps `false → "none"` and `true`/unset → `"interpolate"`. There is no
existing state that maps to `"zero"`; it is new capability, matching Metabase's
`line.missing`.

**The old keys go away entirely** — removed from the stored settings by the migration, and
from `CartesianConfig`, `CartesianBuildConfig` and the codec's `KEYS`. Leaving them behind
would recreate the state this unit exists to remove: two sources of truth for one shape,
disagreeing after the next edit. Because `smooth` is read by area as well as line, this
means area and combo start reading `lineShape` — which is what "they inherit the migration"
has to mean in practice. Their rendered output is unchanged; only the key it comes from moves.

## Gaps this chart fills

From the program comparison, the line-specific ones: `line.interpolate`, `line.missing`
(including the `zero` option egret has no equivalent for), `line.marker_enabled`,
`line.style` and `line.size`. All five come from Metabase's per-series settings in
`frontend/src/metabase/visualizations/lib/settings/series.ts`, read directly rather than
recalled.

## Deliberately not done

- **Area and combo are not touched.** They share the `cartesian.*` namespace and so inherit
  the migration and the new settings, but their own units come later and this PR changes
  none of their behaviour beyond what the shared shape gives them for free.
- **No chart-wide line style or size** — see above.
- **`graph.tooltip_columns`** stays out, as it did for bar: tooltips are a pass of their own
  across every cartesian kind.
- **Trend line, goal line, dual axis** are already shared cartesian features and are not
  re-opened here.

## Live check

On the Demo Sales dataset:

1. `order_date` (day) × `total_revenue`: switching Line shape between linear, smooth and
   step visibly changes the line, and only one can be active.
2. A series with a genuine gap: all three `missing` modes differ — interpolated bridges it,
   zero drops to the axis, none leaves a break.
3. Markers on auto: dots appear on a short series and disappear on a long one, at the
   threshold the builder chose.
4. Breakout by `category`, then set one series to dashed and another to size L: only those
   series change, and the legend still identifies them.
5. Open **Revenue over time (line)**, saved before this change: it looks exactly as it did,
   with its settings rewritten by the migration.
6. The same chart as a small dashboard card: still legible.

## Verification

The program's gate: lint, typecheck, tests, build, deploy to kind, live check, plus
before/after screenshots at both sizes. Chart 01 established that the live check is where
the seam defects surface — settings that never reach the renderer, controls that
contradict the chart — so it is not optional here either.
