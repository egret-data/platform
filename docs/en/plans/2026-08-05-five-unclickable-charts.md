# Five charts that do nothing when you click them — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make funnel, gauge, progress, smartscalar and object respond to a click, each in the way its
own shape justifies — and correct a comment that argues they should not.

**Architecture:** `ChartRenderer` receives one outward-facing prop, `onValueClick(ctx: DrillContext,
x, y)`. Each branch adapts its chart's click into a `DrillContext` and calls it. This unit adds five
adapters. It does **not** change `DrillContext`, the drill dialog, or the filter machinery unless
Task 1 proves it must.

**Tech stack:** React 18, ECharts 6.1, vitest.

---

## Where this came from

A code-level audit at `1d9b0d30` walked every branch of `ChartRenderer` and checked its click wiring
against Metabase's equivalent. Wired and correct: cartesian, pie, waterfall, boxplot, sankey, map,
table, pivot, scalar. Not wired:

| branch | egret | Metabase equivalent |
|---|---|---|
| `funnel` `ChartRenderer.tsx:730` | no handler | `FunnelNormal.tsx:197-204` drills a step |
| `gauge` `ChartRenderer.tsx:709-723` | no handler | `Gauge/GaugeArc.tsx:42-60` drills a segment |
| `progress` `ChartRenderer.tsx:683-697` | no handler | `Progress/Progress.tsx:68-75` |
| `smartscalar` `ChartRenderer.tsx:778-794` | no handler | `SmartScalar/SmartScalar.tsx:88-102` |
| `object` `ChartRenderer.tsx:510-512` | no handler | `ObjectDetail/ObjectDetailBody.tsx:16-35` |

**This programme has now forgotten this seam ten times** — five in an earlier round (waterfall,
boxplot, sankey, map, object), five here. The reason it keeps happening is structural and worth
stating: **chart-level tests call `buildXOption` directly and never cross that line**, so a green
suite is not evidence. Only a `ChartRenderer`-seam test or a live click can see it.

`FunnelChart`'s `type: "bar"` mode (`FunnelChart.tsx:242-262`) builds a real cartesian option and
**also** passes no handler, so the funnel is unclickable in both of its modes. Both need wiring.

## A comment that argues the opposite, and is wrong

`ChartRenderer.tsx:316-319`:

> There is no click handler either — a gauge draws a single value, so there is no element on it that
> names a row to drill to; Metabase's gauge has no drill either.

The **first** clause is a real argument and Task 1 has to answer it. The **second is false**:
`Gauge.tsx:186-188` passes `onVisualizationClick` down to `GaugeArc`, which builds a `ClickObject`
from the segment. A comment stating a checked fact that isn't one is worse than no comment — say so
when you replace it, and record what was actually measured.

---

## Task 1: decide what a click means on a chart with no dimension — before writing any adapter

**This is the whole design question and it must be settled first.** Three of the five (gauge,
progress, smartscalar) draw a single aggregate. There is no dimension value under the cursor, so
"filter by what I clicked" has nothing to filter by.

**egret already has a precedent and you must find and follow it rather than invent one.**
`ChartRenderer.tsx:665-681` wires `scalar` — also a single aggregate — by passing `onValueClick`
straight into `BigNumber`. Establish, by reading:

- [ ] **Step 1: What `DrillContext` does `BigNumber` actually emit?** An empty `dims: []`? Something
  else? Read it, and read what the receiving end does with it — `PreviewPane.tsx:133-140` calls the
  prop `onCellDrill`, and `DashboardCardView` is the other caller. **Does the drill dialog handle an
  empty dims list, and what does it show?** Run it if reading is ambiguous.

- [ ] **Step 2: What does Metabase's gauge drill actually do?** Not "does it drill" — the audit
  settled that. What does clicking a segment *produce*: a filter on the segment's range, or an
  underlying-records view with no new filter? Read `GaugeArc.tsx:42-60` and the `ClickObject` it
  builds, and drive the live instance at `http://localhost:3033` (`ref@example.com` /
  `Refuser12345!`) if the source is ambiguous. Do the same for `Progress` and `SmartScalar`.

- [ ] **Step 3: Report the decision before implementing.** For each of the five, state what the click
  emits and why. **If the answer for gauge/progress is "the same nothing-dimensional drill the scalar
  already does", say so — that is a good answer and it makes three of the five nearly free.** If
  instead it needs a range filter on the segment, that is a new `DrillContext` shape and it changes
  the size of this unit; say that too, and stop for a decision rather than expanding scope silently.

**Per-chart drill shapes in this programme have each been argued individually and are worth reading
before you decide** — sankey edges filter BOTH columns; a sankey node picks target-vs-source by
whether anything flows into it; boxplot reads `dataIndex` because a boxplot element has no usable
`name`; map filters the RAW dimension value, never the normalised geojson key; waterfall REFUSES on
its Total bar. **A chart that should refuse a click is a legitimate outcome** — the waterfall's Total
bar is the precedent. If gauge is one of those, say so with the reason, and correct the comment to
give the real one.

## Task 2: funnel — both modes

A funnel's steps come from a dimension, so this one is an ordinary dimensional drill and should look
like the existing ones.

- [ ] **Step 1: Write the failing test at the seam.** A `ChartRenderer`-level test, not a
  builder-level one — the builder tests cannot see this defect, which is exactly why it shipped.

- [ ] **Step 2: Wire the normal mode.** `funnelSteps` already keys on the raw dimension string
  (`funnel.ts:54,57`) — **drill from the raw value, not the label**. This codebase has had seven bugs
  from keying on a display string, the last two caught only by mutation.

- [ ] **Step 3: Wire the `type: "bar"` mode too** (`FunnelChart.tsx:242-262`). It builds a cartesian
  option, so check whether the cartesian adapter can be reused rather than writing a second one.

- [ ] **Step 4: Mutate** display-name → raw-name and watch it die. **A two-step fixture cannot see
  this** — use three steps with the renamed one in the middle.

- [ ] **Step 5: Gates, commit.**

## Task 3: object

- [ ] **Step 1:** Establish what `ObjectDetailBody.tsx:16-35` drills on — a field, a value, or the
  row. Then the failing seam test, then the adapter.

- [ ] **Step 2: Gates, commit.**

## Task 4: gauge, progress, smartscalar

Implement whatever Task 1 decided. If that is "reuse the scalar's path", this task is three small
adapters and its main risk is the comment.

- [ ] **Step 1: Failing seam tests**, one per chart.

- [ ] **Step 2: Implement.**

- [ ] **Step 3: Replace the comment at `ChartRenderer.tsx:316-319`** with what you measured. If a
  chart legitimately refuses the click, the comment must say why, and the reason must be its own —
  not a claim about Metabase that a future reader will have to re-check.

- [ ] **Step 4: Gates, commit.**

## Task 5: close the seam so it cannot happen an eleventh time

Ten misses at one seam is a structural problem, not ten oversights.

- [ ] **Step 1: Add a test that enumerates every `vizType`** and asserts each either passes a click
  path or is on an explicit, commented refusal list. A new chart then fails this test until someone
  makes a decision about it.

- [ ] **Step 2: Verify the test can fail.** Remove one branch's wiring and confirm it goes red. If it
  cannot fail, it is worth nothing — this programme has shipped a passing suite over a dead code path
  twice.

- [ ] **Step 3: Gates, commit.**

## Task 6: live check and the PR

- [ ] **Step 1: Gates**, then drive it. If you build, **check the loaded bundle hash** against
  `ls -t frontend/dist/assets/index-*.js` — the scratchpad `serve.py` serves `index.html` with no
  cache headers, so the first reload after a rebuild keeps the OLD bundle.

- [ ] **Step 2: Click all five on a dashboard.** Drill is a **dashboard-card** interaction — #397
  established that clicking a point in the chart editor's preview opens nothing, and established it
  with a control (switching a chart to a scale where drill demonstrably works, and getting nothing
  there either). Do not repeat that mistake: use a dashboard card.

- [ ] **Step 3: Check the filter each one produces is right**, not just that a dialog opened. The
  most valuable check this programme found was **reconciling numbers**: the records a drill returns
  must total the value that was clicked. That is how a silent off-by-a-period bug was caught on the
  pivot.

- [ ] **Step 4:** the 19 "ZZ Chart Parity" charts, no regressions.

- [ ] **Step 5: Open the PR. Do NOT merge.**

---

## Out of scope, deliberately

The light-theme focus ring at 2.58:1 — #412 established it is **product-wide**: 34 call sites across
26 files, reaching notebook, SQL Lab and the control plane, with only 5 files under
`components/chart/`. It needs its own PR.

Also out: the dead `view_as`/`link_text`/`link_url` trio on 17 of 18 viz types; the four mutation
survivors from the audit; a combo's left axis title reading a hidden series' name after a legend click
(found in #412); and the Metabase-side gaps (scatter's Bubble size and value-axis block, trend's
Comparisons, the Compact label mode, Unpin from zero, pie's Show total).

## Self-review

- **Task 1 is investigation and ends in a decision, not a diff.** Three of the five charts have no
  dimension under the cursor, and getting that wrong means either a drill that filters on nothing or a
  new `DrillContext` shape smuggled in without review.
- **A refusal is a legitimate outcome for any of the five.** The waterfall already refuses on its
  Total bar. What is not acceptable is the current state: no handler and a comment justifying it with
  a false claim.
- **Task 5 is the one that pays for itself.** Without it this seam gets missed again; the same defect
  has now shipped ten times and every previous fix was per-chart.
- **Known soft spot:** the plan assumes `DrillContext` can express whatever the five need. If gauge or
  smartscalar wants a range filter (a segment's bounds, or a comparison period), that is a shape
  `dims: [{col, value, unit?}]` may not carry. Task 1 Step 3 is where that surfaces — surface it there
  rather than bending the type quietly.

---

## Outcome — Task 1's decision, and where this plan was wrong

Recorded here rather than only in the PR, because the plan's own premise turned out to be false in
one place and the next reader of this file should not inherit it.

### The decision, per chart

Everything below was settled by reading Metabase's source before any adapter was written. **No new
`DrillContext` shape is needed anywhere** — the plan's known soft spot did not materialise.

| chart | emits | why |
|---|---|---|
| funnel (both modes) | `{dims: [{col: dimension, value: RAW step name, unit?}]}` | `components/FunnelNormal.tsx:183-193` ships `dimensions: [{column: dimCol, value: raw step value}]`; bar mode reaches the identical filter through `funnelToBarTransform` + the cartesian click object. |
| object | `{dims: [{col: field, value: RAW field value}]}`, value cell only | `components/ObjectDetail/ObjectDetailsTable.tsx:209-238` gives a `clicked` to the value half only; the label half gets none and is inert. Resolves to quick-filter, `column = value`. |
| gauge | `{dims: []}` | `Gauge/GaugeArc.tsx:47-61` builds `{value: segment.min, column: cols[0], settings, event}` with **no `dimensions`**. `underlying_records.cljc:95-105,183-188` then iterates an empty dimensions seq — **no filter is added**. quick-filter/automatic-insights/column-filter all decline it. **A gauge segment is not a range filter.** |
| progress | `{dims: []}` | `Progress/Progress.tsx:72-77`, same object, handler on the whole bar — no "clicked at 60% of the track" behaviour exists to copy. |
| smartscalar | `{dims: [{col: dimension, value: LATEST period, unit?}]}` | **See the correction below.** |

### Correction 1 — the trend is not a dimension-less chart

This plan says "Three of the five (gauge, progress, smartscalar) draw a single aggregate. There is
no dimension value under the cursor." That is true of gauge and progress and **false of the trend**.
A trend is grouped — dimension + metric — and the number it draws is the latest row of that
grouping. `SmartScalar/compute.ts:311-325` says so explicitly, shipping
`dimensions: [{value: rows[latestRowIndex][dimensionColIndex], column: cols[dimensionColIndex]}]`.

So only **two** of the five are dimension-less. The trend drills to the latest period, as a
half-open range when the dimension is bucketed. Its previous-period comparison is deliberately NOT
a click target: nothing under Metabase's `PreviousValueComparison/` carries an `onClick`, and the
comparison is a derived reading rather than a period of its own.

### Correction 2 — the file references in "Where this came from" had drifted

`FunnelNormal.tsx` and `ObjectDetailBody.tsx` are under `visualizations/**components**/`, not
`visualizations/visualizations/`. `ObjectDetailBody.tsx` is a pure pass-through — the click object
is built in `ObjectDetailsTable.tsx`. SmartScalar's is built in `SmartScalar/compute.ts`, not in
`SmartScalar.tsx`.

### Correction 3 — one deliberate divergence from Metabase, in the funnel

**Metabase's first funnel step is not clickable.** `FunnelNormal.tsx:140` builds its click infos
from `funnelSteps.slice(1)` and hangs the handler on the band section, which the first step does not
have. That is an artifact of the first step having no band to draw, not a decision about what the
step means — it is a row of the result with a dimension value like every other one, and it holds the
number every later step is a share of. egret's is clickable. (This file has form here: it already
declines to reproduce Metabase's mismatched first-column spacer.)

### A second defect, found by the drill rather than by the plan

`SmartScalarChart`'s `valid` filter asked `Number.isFinite(Number(r[mi]))`. `Number(null)` and
`Number("")` are both `0`, and `0` is finite — so a trailing period with no data passed for a real
one: the card read **0**, labelled itself with that period, and said "▼ 100.00% vs previous month".
Found because the new drill pointed at that empty period. Fixed, with its own tests; the fix is
independent of the drill and changes what the chart DRAWS.

### Task 5, as built

`__tests__/ChartRenderer.clickCoverage.test.tsx` renders every `VizType` twice — with and without
`onValueClick` — and requires the first to grow strictly more click affordance than the second. One
generic assertion, not nineteen hand-maintained ones. `REFUSES_CLICK` is the escape hatch and is
deliberately empty today; anything listed in it is checked both ways. Proven able to fail by
unwiring each of the **fourteen** branches in turn: all fourteen went red.
