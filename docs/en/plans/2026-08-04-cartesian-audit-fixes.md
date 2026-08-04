# Four cartesian findings from the audit — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Close the four cartesian findings a three-way audit turned up: a scale that blanks the
chart, a combo that draws its bars on top of each other, three line settings a combo cannot reach,
and two different Axes vocabularies inside one product.

**Architecture:** Tasks 1–3 are builder/panel correctness and are independent of each other.
Task 4 is a panel consolidation and should land last, because it touches the file Tasks 1 and 3
also edit.

**Tech stack:** React 18, ECharts 6.1, vitest.

---

## Where these came from

Three concurrent audits at `1d9b0d30`: a live panel-parity sweep driving both apps, a code-level
sweep of five recurring defect families, and a mutation + accessibility sweep. Findings labelled
MEASURED below were driven or executed; the rest were read.

---

## Task 1: a scale the column cannot take must not be offered — and must not be honoured

**MEASURED (live, `MB · Bar`).** egret's Axes → X axis scale offers `Ordinal / Linear / Timeseries`
**enabled** on a text column. Picking Timeseries renders an entirely empty plot area — no bars, no
y-axis values, no x tick labels. Metabase, driven side by side on the same card, **disables** the
select and offers only `Ordinal` when the x column is a string. Two clicks destroy the card, and it
persists if the user saves.

**Files:** `settings/CartesianDisplaySection.tsx:231-245`, and whatever the builder consults.

- [ ] **Step 1: Establish where the fix belongs, before writing any of it.** `resolveXScale(kind,
  config.xScale, xColumnType)` already exists and already takes the column type — the panel calls it
  at `:242` to display the RESOLVED scale. So one of two things is true, and you must find out which:
  - if `resolveXScale` already refuses `timeseries` on a text column, the builder is not going
    through it, and the fix is in the builder;
  - if it does not refuse, the fix is in `resolveXScale` and every caller gets it.

  **A panel-only fix is not acceptable here.** #407 established the same lesson on Decimal places:
  fixing the control does not reach a chart that already has the bad value stored. Whatever you do,
  an already-saved chart with `xScale: "timeseries"` on a text column must render.

- [ ] **Step 2: Write the failing test** at whichever layer Step 1 identified — a text x-column with
  `xScale: "timeseries"` produces a drawable option (categories present, series data non-empty).
  Watch it fail and record the text.

- [ ] **Step 3: Fix it.** Then disable the inapplicable items in the select. **The mechanism already
  exists in this file** — `:444` greys out `Rotate 45°/90°` when the scale is not ordinal, with the
  reasoning at `:428-437`. Follow that pattern rather than inventing a second one.

- [ ] **Step 4: Do not silently drop the stored value.** If a user saved `timeseries` and the column
  later became text, decide whether the chart draws as ordinal (resolving) or the panel shows the
  stored value greyed. Say which and why in a comment; the file already argues this class of question
  at `:237-241`.

- [ ] **Step 5: Gates, commit.**

## Task 2: a broken-out combo draws its bars on top of each other

**MEASURED.** `echarts/options/cartesian.ts:868`:

```ts
const barCount = seriesValues.filter((_, si) => seriesDisplayFor(cfg.kind, "", si, cfg.seriesTypes) === "bar").length;
```

The series name is hard-coded `""`, so `cfg.seriesTypes[""]` never matches and `seriesDisplayFor`
always returns the positional default (`index === 0 ? "line" : "bar"`). The real per-band display is
resolved 15 lines later at `:883`, from `groupName(sv)`. The two disagree.

Measured on a 2-band combo with `seriesTypes: {Widget: "bar", Gadget: "bar"}`: both series come out
`type: "bar"` **and both carry `barWidth: "80%"`**, because `grouped` was computed `false`. ECharts
then draws them at the same x and one hides the other.

The un-broken-out path at `:897` passes the real metric and is correct. This is combo-and-breakout
only.

**Files:** `echarts/options/cartesian.ts:862-883`

- [ ] **Step 1: Write the failing test.** A combo with a breakout and **at least two bands set to
  `bar`**; assert `grouped` took effect — the honest assertion is on what ECharts is handed
  (`barWidth` absent / auto-sized), not on an internal. Watch it fail.

- [ ] **Step 2: Fix.** Note `groupName` is defined at `:871`, **after** the broken line at `:868` —
  so this is a small reordering, not just a changed argument. Hoist `groupName` above `barCount` and
  pass `groupName(seriesValues[si])`.

- [ ] **Step 3: Mutate.** Put `""` back and confirm the test dies. Also mutate to
  `String(seriesValues[si])` — the raw value without the `(empty)` mapping — and check whether
  anything catches it, since `decorateSeries` is keyed on `groupName(sv)` and the two must agree.

- [ ] **Step 4: Gates, commit.**

## Task 3: a combo cannot reach three line settings that it honours

**MEASURED.** `settings/CartesianDisplaySection.tsx:77` gates the chart-wide line controls on

```ts
const isLineish = kind === "line" || kind === "area";
```

which excludes `combo` (used at `:126`, `:139`, `:152`). But `decorateSeries` applies those settings
to **any series whose resolved type is line** (`echarts/options/cartesian.ts:1035-1077`).

Measured on `kind: "combo"` with `missing: "zero"`, `lineShape: "smooth"`, `markers: "off"`: the line
band came back `data: [1, 0, 3]` (null rewritten to 0), `smooth: true`, `showSymbol: false`. **All
three took effect; the panel offers none of them for a combo.**

`shape` and `markers` are still reachable per-series through `SeriesSettingsPopover` (#408).
**`missing` is reachable by nothing** — `LineSeriesStyle` deliberately omits it, with the argument at
`cartesian.ts:1047-1057`, so the chart-wide control is its only door and that door is shut for combo.

This matters beyond appearance: `missing: "zero"` **rewrites the data**, so it silently changes stack
totals, normalisation and trend fits.

**Files:** `settings/CartesianDisplaySection.tsx:77`

- [ ] **Step 1: Write the failing test** — a combo panel offers the missing-values control. Watch it
  fail.

- [ ] **Step 2: Decide the gate and state it.** "Combo is line-ish" is the cheap answer and it is
  wrong when every series is a bar. The honest gate is **does any series resolve to a line** — which
  `seriesDisplayFor` + `echartsTypeOf` already answer, and which #408 already imported into a settings
  component for exactly this kind of question. Prefer that; if it cannot be computed here because the
  series names are not available, say so and fall back, with the reason in a comment.

- [ ] **Step 3: Check the other two while you are here.** `lineShape` and `markers` have a per-series
  door for a combo but no chart-wide one. Once the gate is right they get both, which matches how
  every other kind behaves.

- [ ] **Step 4: Gates, commit.**

## Task 4: one Axes vocabulary, not two

**MEASURED (live, six charts).** Waterfall and box plot render their Axes tab through
`settings/ChartAxesSection.tsx` and match Metabase: an **"Auto y-axis range"** switch (`:177`) that
gates Y min / Y max, and **"Show lines and tick marks"** as a *select*. Bar, line, area, combo, row
and scatter render through `settings/CartesianDisplaySection.tsx` and show a different panel: **no
Auto y-axis range at all** (bare Y min / Y max always visible), and lines/ticks as a *switch*.

**The same stored keys, presented two ways, depending on which chart the user opened.**
`CartesianDisplaySection.tsx:492` already acknowledges the duplicated min/max pair; the gate was never
copied across.

**This costs no stored key.** `ChartAxesSection.tsx:149` derives the switch:
`const autoRange = config.yMin == null && config.yMax == null`. Nothing new is persisted.

- [ ] **Step 1: Write the failing tests** — on a bar chart, Y min / Y max are not visible until
  "Auto y-axis range" is turned off; the lines/ticks control is a select. Watch them fail.

- [ ] **Step 2: Adopt `ChartAxesSection`'s shape** in the cartesian panel. **Prefer reusing the
  component over copying its markup** — two implementations of one control is how this programme got
  four per-series settings spread over three parallel lists. If it cannot be reused, say precisely
  what blocks it.

- [ ] **Step 3: Do not lose the cartesian panel's extras.** It carries things the waterfall's does not
  (per-axis grouping from #403, the Scale select from #406, rotation from the audit's own note that
  rotation is correctly greyed on a value axis). Enumerate what each panel has before merging, and
  report anything that had to move.

- [ ] **Step 4: Gates.** Expect existing tests to move. **Every one that does must be understood and
  explained** — this programme has shipped a passing suite over a dead code path twice.

- [ ] **Step 5: Commit.**

## Task 5: live check and the PR

- [ ] **Step 1: Gates**, then drive the app. If you build, **check the loaded bundle hash** against
  `ls -t frontend/dist/assets/index-*.js` — a stale `index.html` has produced a wrong conclusion twice
  here, and the scratchpad `serve.py` serves it with no cache headers.

- [ ] **Step 2: Reproduce all four findings on the pre-fix bundle and show each gone on the post-fix
  one.** Task 1's is the one to be most careful with: check both the panel (the item is disabled) and
  an already-saved chart carrying the bad value (it renders).

- [ ] **Step 3:** the 19 "ZZ Chart Parity" charts, no regressions.

- [ ] **Step 4: Open the PR. Do NOT merge.** Body carries the failing-assertion text per task,
  mutation results, what was verified live and what was not and why.

---

## Out of scope, deliberately

The audit's other findings, each recorded so they are not lost: the pie's aria description naming
slices the chart is not drawing; `ChartTable`'s mouse-only resize and its "sortable" header whose
Enter starts a drag (the fix already exists at `pivot/PivotTable.tsx:12-51`); the light-theme focus
ring at 2.58:1 against a 3:1 bar; `onPointClick` still missing on funnel/gauge/progress/smartscalar/
object plus a comment at `ChartRenderer.tsx:319` asserting a checked fact that is false; the dead
`view_as`/`link_text`/`link_url` trio on 17 of 18 viz types; four mutation survivors; and the
Metabase-side gaps (scatter's Bubble size and value-axis block, trend's Comparisons, Compact label
mode, Unpin from zero, pie's Show total).

## Self-review

- **Tasks 1–3 are independent; Task 4 is not.** It edits the file Tasks 1 and 3 also touch, so it goes
  last and its diff will be easier to read for it.
- **Task 1 is the only one that can destroy a user's saved card**, which is why its first step is an
  investigation rather than a change, and why a panel-only fix is ruled out up front.
- **Known soft spot:** Task 3 Step 2 assumes the resolved series display is computable inside
  `CartesianDisplaySection`. #408 established that `seriesDisplayFor` is importable from a settings
  component and that `echartsTypeOf(display) === "line"` is the rule that actually governs
  `cfg.seriesLine` — but it had series names to hand and this panel may not. Check before assuming.
- **Task 2's test must assert on what ECharts is handed**, not on the `grouped` local. A test that
  reaches inside would pass on a fix that computed the right boolean and used it wrongly.

---

## Corrections made while executing this plan

Recorded here rather than silently absorbed, because each one was a premise of a step.

- **Task 1, Step 1 — the dichotomy resolved to the second branch, and the rule is wider than the
  finding.** `resolveXScale` did NOT refuse: `if (xScale != null) return xScale` handed the stored
  scale back untouched, so the fix belongs there and every caller gets it. But the measured case
  (`timeseries` on a text column) is one of four, not one of one — `applyXScale` drops any point
  whose coordinate is not finite, so a stored `linear` on a text column and a stored `timeseries` on
  a NUMERIC column blank the chart exactly the same way (`Date.parse("15.69")` is NaN). The refusal
  is therefore a general column-type fit (`xScaleFitsColumn`), which is also Metabase's own rule:
  Timeseries only on a temporal column, Linear only on a numeric one, Ordinal always.

  **The cost this buys, named rather than hidden:** a `varchar` column holding ISO date strings
  coerces fine through `Date.parse` and is now refused, so such a chart falls back to the ordinal
  axis. Every point still draws, in order. The alternative — judging the coerced VALUES — cannot be
  shared with the panel, which holds no rows, and would leave the select greying items out by one
  rule while the chart honoured another.

- **Task 1, Step 4 — the choice, and why it is the opposite of the rotation rows'.** The panel shows
  the RESOLVED scale ("Ordinal"), not the stored word greyed. That follows from the select already
  being bound to `resolveXScale`, and printing "Timeseries" over a chart drawing categories is the
  defect that binding was added to remove. The stored key is never rewritten, so re-pointing the
  chart at a date column brings Timeseries back. The rotation items differ because
  `xTicksChoiceOf` reads the CONFIG; this select reads the resolution.

- **Task 3, Step 2 — the flagged soft spot does not hold.** `SeriesOptions` already takes
  `seriesNames`, threaded in by `viz/cartesian.tsx` for the Stacking count, with a fallback to the
  metrics. The gate is computable here. The one genuinely empty case is a breakout whose query has
  not returned; that falls back to asking the builder what series 0 would be.

- **Task 4, Step 2 — `ChartAxesSection` cannot be reused whole, and the reason is enumerable.** It
  has no per-axis scoping, no x-axis scale select, no left/right metric split, no `autoSplitY`, no
  value-axis rotation greying, and hard-codes "Y" where cartesian must name the axis the reader sees.
  The reuse is therefore four exported sub-components — `CategoryTicksSelect`, `ValueTicksSelect`,
  `ValueRangeRows`, `TickCountRow` — rendered by both panels, with the differences passed in. Every
  cartesian extra was kept; none moved.

- **Task 4 also closed a wording split the finding did not name.** The same rows were labelled
  "X axis lines and ticks" / "Y tick marks" on one tab and "Show lines and tick marks" / "Number of
  tick marks" on the other. Sharing the component settles it on Metabase's wording.

- **One existing test moved and it was pinning a bug.** `scatterValueX.echarts.test.ts` asserted
  that an explicit `timeseries` scale won on a `double` column, checking only `xAxis.type`. All five
  points were being dropped onto a `time` axis with no extent — the very defect Task 1 is about,
  green in the suite.

## Found while executing, not fixed

- **The scatter's aria description reads `NaN` for every y.** Live on `MB · Scatter`: "Scatter chart
  of rating by price. 168 values: 15.69 NaN, 19.87 NaN, …". Pre-existing and untouched by this work
  — that card stores no `xScale`, so `resolveXScale` short-circuits before any new code runs. It is
  the same class as the pie's aria finding already in the out-of-scope list above (a value-axis
  series' data are `{value: [x, y], name}` item objects since #387, and the aria builder reads them
  as if they were bare numbers). Belongs with that group.
