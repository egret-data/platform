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

**Depth, cartesian.** egret already carries 27 settings — stacking,
normalisation, dual axis, log scale, trend line, goal line, per-series colour and
label, step/smooth/markers, ordinal/linear/timeseries x. Metabase has six things
egret does not, all of which matter when real data is messy:

| Metabase setting | What it does | egret |
|---|---|---|
| `graph.max_categories` + `other_category_aggregation_fn` | folds a long tail into "Other" | pie only (`sliceThreshold`) |
| `graph.series_order` | reorder / hide individual series | — |
| `graph.tooltip_columns` | extra columns in the tooltip | — |
| `graph.y_axis.auto_split` | split to a second axis when scales diverge | manual only |
| `graph.label_value_frequency` | label every Nth point | — |
| `graph.label_value_formatting` | compact labels (25.5K) | — |

**Depth, pie.** egret is ahead: donut, centre total, slice threshold, per-slice
colour, percent visibility. Metabase adds `pie.decimal_places`.

**Depth, table.** The widest gap. Metabase has pagination, frozen columns and
rows, column widths, column show/hide and reorder. egret has sort, width and
conditional formatting — no pagination, no freezing.

**Depth, trend (smartscalar).** Metabase compares against several baselines
(previous period, same period last year, a fixed value); egret only compares with
the immediately previous period.

## Goal

Every chart type is one a person can produce something good with immediately, and
tune without reading documentation.

## Decomposition

Nineteen units, each its own spec → plan → PR, in order of how much they are
used:

> bar · line · table · pie · scalar · pivot · area · combo · scatter · row ·
> trend · progress · gauge · funnel · waterfall · boxplot · sankey · map · object

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
