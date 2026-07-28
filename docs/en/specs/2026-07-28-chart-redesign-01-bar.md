# Chart Redesign 01 — Bar

**Status:** approved design, pending implementation plan
**Date:** 2026-07-28
**Program spec:** `docs/en/specs/2026-07-28-chart-redesign-program-design.md`
**Vietnamese reference:** `docs/vi/specs/2026-07-28-chart-redesign-01-bar.md`

## What a bar chart is for

Comparing one measure across a small set of categories. Everything below follows
from that: the categories have to be readable, the values have to be comparable
(a bar axis starts at zero), and a long tail must not turn into two hundred
unreadable slivers.

Bar is first because it is the most used type, and because the four architecture
changes the program needs — grouped controls, per-kind cartesian declarations,
render-time defaults, and the two shared helpers — all land here and every later
chart inherits them.

## What must be right before any configuration

| | Today | After |
|---|---|---|
| Legend | shown only if `showLegend` is set; default off | on when there is a breakout or more than one metric, off otherwise |
| Category labels | drawn at full length, overlapping when crowded | thinned, and rotated when they still collide |
| Value axis | full numbers (`25,468`) | compact (`25.5K`) when the axis is narrow |
| Long tail | every category drawn | unchanged by default — see below |
| Axis titles | empty unless typed | derived from the column and metric names |

**The long tail stays opt-in.** Metabase's `graph.max_categories_enabled`
defaults to `false` and `max_categories` to `MAX_SAFE_INTEGER` — folding is
offered, not imposed. egret matches that: the control appears, the default keeps
every category. Silently hiding data behind "Other" is worse than a crowded axis.

## Control inventory

`group` is the new `VizControl.group`; `advanced` is the collapsed section.

| Control | Group | Default | Change |
|---|---|---|---|
| X-axis | Data | — | unchanged |
| Y-axis (metrics) | Data | — | unchanged |
| Series (breakout) | Data | none | unchanged |
| Max categories + Other | Data | off | **new** |
| Series order / visibility | Series | source order | **new** |
| Series colours | Series | palette | moved from a flat list |
| Series labels | Series | metric name | moved |
| Stacked | Series | off | moved |
| 100% stacked | Series | off | moved, still gated on Stacked |
| Stack totals | Series | off | moved, still gated |
| Horizontal | Axes | off | moved |
| X axis scale | Axes | ordinal | moved |
| X / Y axis title | Axes | derived from column names | **default changed** |
| Y min / Y max | Axes, advanced | auto | moved |
| Log scale | Axes, advanced | off | moved |
| Data labels | Format | off | moved |
| Label density | Format | auto | **new** |
| Compact numbers | Format | auto | **new** |
| Goal line + label | Format, advanced | none | moved |
| Trend line | Format, advanced | off | moved |

Nothing is removed. Six controls move into groups; three are new; one default
changes.

## Gaps this chart fills

From the program comparison, the two that apply to bar:
`graph.max_categories` (+ the "Other" bucket) and `graph.series_order`. Both are
built as shared code — the fold helper and the series-order control — so line,
area and combo inherit them.

`graph.tooltip_columns` and `y_axis.auto_split` are **not** part of this chart:
tooltips belong to a pass of their own, and auto-split matters for combo, where
it will be designed.

## Deliberately not done

- **Legend click-to-hide** — ECharts' legend already toggles a series on click.
  The program's standard 4 is satisfied here without code; it will be verified
  live rather than reimplemented.
- **Per-bar colour by value** (a conditional-formatting-style ramp). Metabase
  does not do it either, and it invites charts that mislead.
- **Bubble size** — scatter's, not bar's.

## Live check

On the Demo Sales dataset:

1. `region` × `total_revenue` with no other configuration: four bars, no legend,
   compact axis labels, both axis titles filled in.
2. Add `category` as a breakout: the legend appears by itself; clicking an entry
   hides that series.
3. Switch to a high-cardinality dimension (`order_date`, day grain): labels thin
   out rather than overlapping; turning on Max categories folds the tail into
   "Other".
4. Reorder two series, then reload the saved chart: the order survives.
5. The same chart as a small dashboard card: still legible, with fewer labels.
6. A bar chart saved before this change still opens and now picks up the new
   defaults (they are render-time, so this is the case that proves it).

## Verification

The program's gate: lint, typecheck, tests, build, deploy to kind, live check,
plus before/after screenshots at both sizes.
