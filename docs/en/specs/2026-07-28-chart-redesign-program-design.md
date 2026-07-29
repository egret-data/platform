# Chart Redesign Program — Design Spec

**Status:** approved design, per-chart specs follow
**Date:** 2026-07-28
**Scope:** every one of egret's 19 chart types, one at a time
**Vietnamese reference:** `docs/vi/specs/2026-07-28-chart-redesign-program-design.md`

## Context

The chart platform is structurally finished: A gave the studio and the viz
registry, B one render vocabulary, C validation at the write path. What none of
them touched is whether an individual chart is *good* — its defaults, the shape
of its configuration panel, and what it does when the data is awkward.

This program is that pass, held against what Metabase and Superset actually do.
Both repos are checked out beside this one and were read, not recalled.

### Where egret stands

**Type coverage.** 19 types against Metabase's 20 and Superset's ~25 plus
deck.gl. Even with Metabase (egret lacks pin/grid maps, has everything else).
Behind Superset on distribution (histogram, heatmap), hierarchy (treemap,
sunburst), and relationship (graph, tree, bubble) charts.

**Depth, cartesian.** egret carries a deep cartesian config — stacking,
normalisation, dual axis, log scale, trend line, goal line, per-series colour and
label, line shape/markers/missing-values, ordinal/linear/timeseries x. Of the six
Metabase settings this spec originally listed as missing, **four have since shipped**
in charts 01 and 02:

| Metabase setting | What it does | egret |
|---|---|---|
| `graph.max_categories` | folds a long tail into "Other" | **shipped** #354 (`maxCategories`, opt-in) |
| `graph.series_order` | reorder / hide individual series | **shipped** #354 (`seriesOrder`, `hiddenSeries`) |
| `graph.label_value_frequency` | label every Nth point | **shipped** #354 (`labelDensity`) |
| `graph.label_value_formatting` | compact labels (25.5K) | **shipped** #354 (`compactValues`, auto by magnitude) |
| `graph.tooltip_columns` | extra columns in the tooltip | still absent (verified 2026-07-29) |
| `graph.y_axis.auto_split` | split to a second axis when scales diverge | still manual only (verified 2026-07-29) |

**Depth, pie.** egret is ahead: donut, centre total, slice threshold, per-slice
colour, percent visibility — plus `sortRows`, `showLegend` and `showLabels`, which
this spec previously left out. Metabase adds `pie.decimal_places`.

**Depth, table.** ~~The widest gap.~~ **Corrected 2026-07-28, when chart 03 began:
this row was wrong.** It claimed egret had "no pagination, no freezing". Reading
the code rather than recalling it, `TableState` already carries `pagination`,
`pinnedLeft`, `columnWidths`, `hiddenColumns` and `columnOrder`, and `ChartTable`
implements all five plus a sticky header and drag-to-reorder. The real remaining
gap is far narrower: **`ColumnFormat.column_title` is dead** — the field exists and
the cartesian axis titles read it, but nothing writes it and the table renders
`header: c.name` raw, so renaming a column is something a user cannot do anywhere.

The lesson generalises past table: this comparison was written by reading Metabase
and *recalling* egret. Every remaining per-chart spec must re-check its own row
against the code before designing from it.

**Depth, trend (smartscalar).** Metabase compares against five baselines
(`COMPARISON_TYPES`: previous period, previous value, N periods ago, a static
number, another column) and can invert good/bad with
`scalar.switch_positive_negative`. egret's `SmartScalarConfig` is
`{ dimension, metric }` — no baseline choice at all. Verified both sides
2026-07-29.

### The comparison's real defect: it covered four types, not nineteen

**Audited 2026-07-29, after the table row turned out to be wrong.** The sections
above discuss the cartesian family, pie, table and trend. That is four of nineteen.
For the other fifteen this spec asserted a one-line summary and no detail — and
spot-checking egret's own config types found gaps it never mentioned:

| Type | egret's whole config | Not mentioned above |
|---|---|---|
| scalar | `{ metric }` | — |
| progress | `{ metric, goal }` | Metabase has `progress.color` |
| gauge | `{ metric, min, max }` | Metabase has `gauge.segments` — coloured ranges, the point of a gauge |
| funnel | `{ dimension, metric }` | egret-side verified only |
| waterfall | `{ xAxis, metric, showTotal }` | Metabase has increase / decrease / total colours |
| boxplot | `{ category, value }` | egret-side verified only |
| sankey | `{ source, target, metric }` | egret-side verified only |
| map | `{ mapKey, region, metric }` | egret-side verified only |
| object | `{ columns }` | egret-side verified only |

**Read that table's last column carefully.** Rows marked "egret-side verified only"
mean the egret config was read from `frontend/src/api/charts.ts`, and Metabase was
**not** checked for that type. They are not claims that no gap exists — they are
claims that nobody has looked. The rows naming a Metabase setting were verified in
the Metabase checkout on 2026-07-29.

So the correction is not "one row was wrong". It is that this was a comparison of
four types wearing the title of nineteen. Each remaining per-chart spec opens by
reading both sides for its own type, and records what it verified and what it did
not.

## Goal

Every chart type is one a person can produce something good with immediately, and
tune without reading documentation.

## Decomposition

Nineteen units, each its own spec → plan → PR, in order of how much they are
used — **except where the audit found a type that is incomplete rather than
merely unpolished**:

> bar · line · table · **gauge** · pie · scalar · pivot · area · combo · scatter ·
> row · trend · progress · funnel · waterfall · boxplot · sankey · map · object

**Gauge moved from thirteenth to fourth on 2026-07-29**, when the audit above found
egret's entire gauge config is `{ metric, min, max }` while Metabase carries
`gauge.segments` — the coloured ranges that are the reason to choose a gauge over a
number. Usage order is the right default, but a type that cannot express its own
premise outranks a type that is merely unpolished. Pie, by contrast, is the row the
audit found egret already **ahead** on, so it stays where it is and will be a UX
unit rather than a gap-filling one.

Each PR covers all three axes **for its chart**: configuration UX, the feature
gaps that chart has, and any split its type warrants. Doing one axis across all
nineteen first would open every chart three times and leave each in a half-state
in between.

Adding chart types egret lacks (histogram, heatmap, treemap, radar, …) is a
separate phase after these nineteen. They are new work, not redesign.

## The standard

Each chart is held to seven things. This list is the yardstick; a per-chart spec
records only where that chart deviates and why.

1. **Good with no configuration.** Choosing a dimension and a metric produces a
   readable chart: formatted numbers, axis titles derived from column names,
   palette colours, a legend only when there is more than one series.
2. **Progressive disclosure.** The Display panel today is a flat list of roughly
   fifteen toggles. Controls become named groups (Data / Series / Axes / Format),
   ordered by how often they are touched, with rare ones behind a collapsed
   "Advanced" — the grouping Metabase gets from `graph.x_axis.*` / `graph.y_axis.*`.
3. **Awkward data handled.** A long tail folds into "Other"; labels thin out when
   they collide; numbers go compact (25.5K) when an axis is narrow.
4. **Direct manipulation where it is cheap.** Clicking a legend entry hides that
   series, without opening a panel.
5. **Empty states that name what this chart needs**, not a generic sentence.
6. **Works at both sizes** — a small dashboard card and the full builder. Today
   one option set serves both with no breakpoint.
7. **Keeps what A, B and C bought**: flat settings, descriptors owning their
   controls, real aria descriptions, and no renderer changes outside the chart
   being worked on.

## Architecture changes, made once before the first chart

**Controls gain a group and a level.** `VizControl` (from A2) takes
`group?: string` and `advanced?: boolean`; `StudioPanel` renders titled groups
with the advanced ones collapsed. Without this, standard 2 is unreachable.

**Cartesian declares controls per kind.** `cartesianDescriptor(kind)` currently
returns nearly the same control list for all six. Each kind declares its own —
bar gets stacking and orientation, line gets smooth/step/markers/gaps, scatter
gets bubble size, combo gets per-series type. The `cartesian.*` namespace stays
shared, so switching bar→line still keeps the axis and metrics, which A2 bought
deliberately.

**Smart defaults live in the option builder, not in stored settings.** "Show the
legend when there is a breakout" is written `cfg.showLegend ?? hasSeries` at
render time. Nothing is written to the database that the user did not choose, so
changing a default later applies to existing charts too.

**Two shared helpers, built with the first chart that needs them:** folding a
long tail into "Other" (generalised from pie's `sliceThreshold`) and compact
number formatting for axis labels.

## Per-chart spec template

One page each. The program spec holds the standard; a chart's spec records only
its own decisions:

- What the chart is for, and therefore what must be right before any configuration.
- Control inventory: a table of `control | group | default | new/moved/removed`.
- Gaps this chart fills, drawn from the comparison above.
- What is deliberately left undone.
- The live check, as concrete steps on the demo dataset.

## Verification, every PR

The gate that A, B and C used — lint, typecheck, tests, build, then a deploy to
kind and a live check — plus two things this kind of work needs:

- **Before/after screenshots** of the same chart on the same data. This is visual
  work; a code diff says nothing about whether it looks better.
- **Both sizes**: the full builder and a small dashboard card, because standard 6
  is otherwise unfalsifiable.

## Out of scope for the whole program

- The params shape. v2 is settled and validated at the API; no chart redesign
  changes it.
- The query pipeline.
- `ChartRenderer` beyond the chart being worked on.

## Risks

- **Nineteen PRs is a long program**, and the standard is only worth anything if
  it is applied consistently. The per-chart spec template exists to make each PR
  answer the same questions rather than drift.
- **Changing defaults changes existing charts.** Because defaults live at render
  time, a chart saved months ago will pick them up. That is intended — it is how
  every chart benefits — but it means each default change needs a look at a
  pre-existing chart, not just a fresh one.
