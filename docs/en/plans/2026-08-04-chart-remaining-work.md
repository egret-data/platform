# What is left in the chart programme — a sequencing plan

> **For agentic workers:** this is a **programme plan**, not a task list for one PR. Each section below
> is a separate PR with its own evidence, scope and stop conditions. Take them in the order given, or
> pick one and write its own implementation plan first — do not try to execute this document top to
> bottom in one branch.

**Status:** the Metabase chart-parity programme (PRs 01–07) is merged, plus `#394` nominal-axis order,
`#395` the studio sort control, and `#396` the label-overlap backstop. What follows is everything still
open, with the measurement behind each so nobody has to rediscover it.

**Everything here was re-checked on 2026-08-04.** Three claims that had been carried forward were wrong
and are corrected in place.

---

## Order, and why

| # | item | why here |
|---|---|---|
| 1 | Issue #387 — drill on a timeseries axis | a feature that silently does nothing; the only user-facing **broken** thing left |
| 2 | The mid-decimal keystroke loss | five number fields eat valid input; small, shared, and PR 06 already scoped it |
| 3 | The waterfall's nominal axis order | closes the family `#394` opened; ~one function |
| 4 | The aria description reads the raw bucket | accessibility correctness, and narrower than previously recorded |
| 5 | A vertical value axis crowds its ticks | visible on bar and box plot at card size |
| 6 | PR 06b — panel completeness | features, not defects; largest and least urgent |
| 7 | Re-run the harness seed | cheap, and the harness is drifting from `demo` |
| 8 | Housekeeping | the user's call, not mine |

---

## 1. Issue #387 — drill on a timeseries x-axis filters on an empty string

**The bug, from the issue:** clicking a point on a cartesian chart whose x-axis scale is **Timeseries**
drills to nothing. The filter comes out `<date column> = ''` — an empty string — which matches no row,
so the drill-through opens on an empty result with nothing on screen to say why.

**Shape of the work.** Find where the drill builds its filter from the clicked point. A timeseries axis
carries its value differently from an ordinal one (`#394`/`#395` touched exactly this area:
`sortCategories`, `resolveXScale`, and `drawLabel` all distinguish the scales), so the value the drill
reads is probably the ordinal-path one, which is empty on a timeseries axis.

**Watch for:** a bucketed axis drills to a **half-open range**, not an equality —
`ChartExplore.tsx:250` already says so in a comment about two filters on the same column. Whatever
fixes the empty string must produce that range, not `= <the timestamp>`.

**Acceptance:** click a point on `MB · Line` and land on rows, in the browser. A unit test on the filter
shape is necessary but not sufficient — this is a bug about what reaches the query.

## 2. The mid-decimal keystroke loss, in five fields

**Measured in PR 06** with real key presses (not `fill()`): every fractional `type="number"` field
loses a keystroke mid-decimal in Chrome. Typing `4` then `.` fires an `input` event with `value === ""`
and `validity.badInput === true`, and `Number("")` is 0.

**The five:** `Y min` and `Y max` (`ChartAxesSection` **and** `CartesianDisplaySection`), and
`Goal value` (cartesian, waterfall, box plot). They **clear** rather than corrupt, because they already
map `"" → undefined` — the milder half of the bug. The slice threshold, which also stored `0`, was
fixed in PR 06 (`SliceThresholdInput`: `type="text" inputMode="decimal"`, draft held in state, commit
only when `Number.isFinite` accepts, `"" → undefined`).

**PR 06 stopped here deliberately** — five fields is a shared component, not a local fix. That is this
PR: extract what `SliceThresholdInput` does into one control and adopt it in all five.

**Integer fields are unaffected** (`Decimal places`, tick marks, `Freeze first columns`,
`Max categories`) — they never receive a `.`. **`Freeze first columns` is not a defect** even though it
shares the `Number.isNaN(n) ? 0` shape: `pinnedLeft ?? 0` is its own default, so clearing to 0 tells
the truth. Do not "fix" it.

**Test with `userEvent.type`, not `fireEvent.change`** — the defect lives in the intermediate states and
a test that sets the final value passes against the broken code. jsdom's `user-event` buffers the `.`
itself, so it sees only half of this; say which half your test covers.

## 3. The waterfall's nominal axis draws in engine order

**Confirmed 2026-08-04**, `options/waterfall.ts:115-119`:

```ts
const ordered = isTemporalType(xType)
  ? [...rows].sort(…)
  : isNumericType(xType)
    ? [...rows].sort(…)
    : rows;            // ← nominal: arrival order
```

Same defect `#394` fixed for bar, row and box plot. The harness's waterfall groups by `order_month`, so
it is temporal and hides this.

**The fix is `sortCategories`**, exported from `options/cartesian.ts` for exactly this reason —
`#394`'s own comment says one shared function is what stops the three drifting apart, and the waterfall
is the fourth. It gets the temporal and numeric branches for free.

**Careful:** the waterfall's order is not cosmetic — the running total builds in it. Sorting changes
which bar is the first step. `#394` exempted the funnel's bar mode via `preserveCategoryOrder` for the
same reason; decide whether the waterfall needs the same escape hatch and say why.

## 4. The aria description reads the raw bucket — and the tooltip does NOT

**Corrected.** The carried-forward claim was "the tooltip and the aria description both read the raw
bucket". Checked at `options/cartesian.ts:1495-1520`:

- **Tooltip**: `visible[0].axisValueLabel ?? visible[0].name`. **This entry was wrong — corrected
  by #400.** `axisValueLabel` is built from `axisPointer.label.formatter`, *not* `axisLabel.formatter`,
  over `OrdinalScale.getLabel`, which returns the raw `xAxis.data` entry — so the tooltip says
  `2026-01-01` while the axis says `January 2026`. Established by tracing ECharts **and** by rendering a
  real chart and capturing `showTip`. **#400 left it raw deliberately**: a tooltip sits beside a drill
  and the click filters on the raw bucket. An open product question, not a settled non-defect.
- **Aria**: `points: categories.map((c) => ({ name: String(c) }))` — the **raw** value. So a screen
  reader hears `2026-01-01` where the axis says `January 2026`.

`drawLabel` (the closure the axis formats with) is in scope in the same builder. The fix is to use it
for the aria points too. Check the breakout legend at the same time — the same claim was made about it
and has not been re-verified.

## 5. A vertical value axis crowds its ticks

`#390` gave the **row chart's** value axis an interval fit, deliberately confined to horizontal charts.
Measured on the dashboard at svg 166×184, the vertical case still crowds: `100K/200K` and `200K/300K`
overlap on the bar, `20/40` and `40/60` on the box plot.

**The machinery exists**: `axisLabelFit.ts` has `intervalThatFits`, `plotWidthLessGutter`,
`widestTickLabel` and `nameGapClearOf`, all written for `#390`. This is applying them to the y-axis.

**Watch for:** a vertical axis's constraint is *height*, not width, and its labels are far shorter, so
the arithmetic is not a transposition — `#390` found the horizontal case needed the widest tick charged
for two outermost overhangs. Establish the vertical equivalent by measurement.

**Also open, and probably the same PR:** the axis **title** overlaps the rotated category labels on the
bar and box plot, exactly as it did on the row chart before `#390` fixed it there.

**Related but out of scope:** bar and box plot give 29% of card height to the plot where the line chart
gives 58%, because 90°-rotated labels reserve their full length. That is `#386`'s rotate ladder and
needs its own comparison.

## 6. PR 06b — the panel is incomplete

PR 06 split workstream 6 into *correctness* (shipped, `#392`) and *completeness*. This is the rest,
measured live against Metabase:

- **Axes tab grouping.** `MB · Bar` renders flat: `Horizontal · X axis scale · X label · Y label · Log
  scale · X axis lines and ticks · Y axis lines and ticks · Y tick marks · Y min · Y max`. Metabase
  splits **X-axis** (Show label, Label, Show lines and tick marks, Scale) from **Y-axis** (Show label,
  Label, Auto y-axis range, Scale, Show lines and tick marks, Number of tick marks). `ChartAxesSection`
  already has the headings but only the waterfall and box plot use it, and `StudioPanel` deliberately
  flattens the Axes tab — so the fix belongs in the cartesian controls.
- **`getHidden`.** A single-series bar shows `Stacking = Don't stack`; Metabase hides it
  (`lib/settings/graph.ts:353-365`, `return stackableDisplays.length <= 1`). The row chart too.
- **The table row count.** `ChartTable.tsx:415` renders `Showing {start+1}–{…} of {totalRows}` but it
  sits inside `{paginationEnabled && …}`, which defaults false. Metabase shows "Showing 60 rows"
  unconditionally.
- **Per-series line settings.** egret's Line shape, Missing values and Markers are global; Metabase's
  are per series, plus Line style, Line size, Y-axis position, trend line and values per series.
- **Y scale.** ~~egret offers a binary "Log scale" switch; Metabase offers Linear / Power / Log.~~
  **Half-closed, and the other half is closed as NOT REACHABLE here.** The control is now a Scale
  select on the cartesian panel too, matching the one `#403` gave the waterfall/box plot. **Power is
  not offered, and this is settled rather than deferred**: `pow` is not an ECharts axis type —
  ECharts has `value | category | time | log` and nothing else — and Metabase does not use the `log`
  axis either. Both of its non-linear scales are a TRANSFORM ON THE DATA over a plain `type: "value"`
  axis (`echarts/cartesian/model/transforms.ts`: signed square root for `pow`, symmetric log for
  `log`), with the inverse re-applied in every axis-label, tooltip and data-label formatter.
  Offering Power without that transform would draw a linear axis under a word promising otherwise.
  Doing it honestly is a builder change across cartesian, waterfall and box plot, plus a new stored
  key — `logScale` is a **boolean** and cannot carry a third value, so it would need a migration,
  a codec entry and backend params validation. That is a PR of its own, not a control change.
  Worth knowing if anyone takes it: egret's Log is ECharts' native log axis, which cannot plot zero
  or negatives at all, where Metabase's symlog handles both. Adopting Metabase's `pow` would drag
  that divergence into scope with it.
- **Goal line** is buried behind "More options (2)"; Metabase has it top-level.
- **Two table column lists** — Data "Columns" (what to query) and Display "Visible columns" (what to
  show) — where Metabase has one tab. egret's Show all / Hide all, numeric Freeze first columns and
  Paginate are improvements; Metabase's "Freeze rows" is missing.
- **`showLabels` default.** ~~Metabase's `getDefaultShowLabels` returns **false** for a single
  dimension, so its one-ring pie draws nothing on the arcs and everything in the legend; egret's
  `?? true` now draws names on the arcs *and* percentages in the legend.~~ **Done.** Re-checked at
  `shared/settings/pie.ts:51-56` (`getPieDimensions(settings).length <= 1 → false`), wired at
  `PieChart/chart-definition.ts:218`. egret's default is now `resolveShowLabels` — off at one ring,
  on above it, an explicit value always winning. The reasoning it overrides is in the pie spec
  ("Legend and slice labels default to on… a pie's colours say nothing without a key"), which argues
  for the LEGEND — the key, and on by default regardless — and says nothing about repeating the key
  on the arcs. The multi-ring half of the argument does hold, and is kept: the legend lists ring 0
  only. A default one-ring pie is not left blank — the legend names every slice and states its
  share, and the tooltip still answers a hover.
- **The temporal-extraction seam.** For `day_of_week` and friends the aggregation key is
  `temporalLabel(...)` → `"Wed"` while `PieDisplayControl` hands the panel raw `3`. Reproduced in
  PR 06. Closing it needs `dimensionUnits` threaded through `VizControlProps`, `StudioPanel`,
  `CardVizPanel` and `ChartExplore` — a **Chart Studio contract change**, which is why PR 06 stopped.
  `pieSliceRows` is already unit-aware.
- Lower confidence: a `beforeunload` prompt fires on a chart opened but not edited. Confirmed
  pre-existing, cause unknown.

**This is too large for one PR.** Split it: the seam and `getHidden` are behaviour; the grouping, the
row count and the scale options are presentation; per-series line settings are a feature on their own.

## 7. Re-run the harness seed

`scripts/harness/seed.ts` exists (`#393`) and `demo` has drifted from what it emits — `#394` and `#395`
both changed what `buildSpec` produces, and `demo`'s `MB · Scalar` / `Gauge` / `Progress` still store
the nondeterministic per-category spec that `#393` adjudicated.

```bash
cd frontend
npx vite-node --config vitest.config.ts ../scripts/harness/seed.ts -- --workspace=demo
```

**Read `docs/en/guides/chart-parity-harness.md` first** — it records the four ways the harness has lied
and what a fresh workspace needs. Re-measure the dashboard afterwards and update the baseline tables the
parity PRs recorded, or future comparisons will be against stale numbers.

## 8. Housekeeping — the user's call

- **`egret-pg-test`** is running (`postgres:16`, up 8 hours at the time of writing). Nothing in this
  programme uses it; the stack is the kind cluster. **Confirm before removing** — it may belong to
  another workstream.
- **54 stray `.png` files** in `data-portal/` (`egret-boxplot-display.png`, `egret-boxplot-v2.png`, …),
  screenshots from parity work. Playwright writes relative to `data-portal/`, not the scratchpad, which
  is how they accumulated. They are outside any git repo, so `.gitignore` will not help; the durable fix
  is to pass an absolute scratchpad path to every screenshot call.

---

## Self-review

- **Three carried-forward claims were wrong and are corrected here**: the tooltip does *not* read the
  raw bucket (only aria does), the stray screenshots number 54 rather than ~20, and `egret-pg-test` is
  running rather than idle. Anything in this document not re-checked on 2026-08-04 is marked as such.
- **No implementation detail is given for items 5–6**, deliberately. They need their own measurement
  first, and this programme's repeated lesson is that a plan written from a stale note specifies the
  wrong fix — `#394` recommended a client-side sort that reading Metabase's source showed was a
  divergence, and `#396`'s plan named four bundle symbols of which three were mangled.
- **Item 3 carries a real risk the summary would hide**: the waterfall's category order determines its
  running total, so "sort it like the others" is not obviously right. That is flagged rather than
  assumed.
