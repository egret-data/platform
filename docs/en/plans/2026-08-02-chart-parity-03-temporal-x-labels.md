# Chart parity 03 — temporal and numeric x-axis labels — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a bucketed date axis read as the periods it covers — "January 2026", not "2026-01-01" — and thin its ticks to period boundaries so a narrow card shows a few meaningful labels instead of none. Give a numeric x-axis round ticks. Fix the trend card printing a bucket's first day where it should name the period.

**Architecture:** Two independent pieces. A **period namer** keyed on the truncation unit the chart spec already carries (`xAxisUnit`), shared by the axis and the trend card. Then **boundary thinning**: a temporal category axis may draw every Nth label where the shown ones fall on regular period boundaries, chosen from measured width. Numeric x on a scatter switches to a real value axis, where ECharts already generates round ticks.

**Tech Stack:** TypeScript, React 18, ECharts (SVG), vitest, dayjs.

**Spec:** `docs/en/specs/2026-08-01-chart-parity-design.md` (workstream 3). **Builds on #385 and #386.**

---

## Background an engineer needs

### What is wrong today, and what PR 02 changed about it

Before PR 02, a bar chart grouped by month drew a few arbitrary date labels — ECharts' `interval: "auto"` kept whichever did not collide. PR 02 replaced that with all-or-nothing: draw every label, rotate them all, or hide the axis. On the ZZ Chart Parity harness at a one-third-width card, **line, area, combo, scatter and waterfall now draw no x labels at all.**

That is the rule working as designed, and it is why this PR exists. Their labels are raw bucket values — `"2026-01-01"` for a month bucket, ten characters that mean less than the eight of `"Jan 2026"` — so twelve of them never fit, and "none" is the honest answer to a question nobody should be asking. Metabase draws two: `January 2026` and `July 2026`.

**This PR is what makes PR 02 look like an improvement rather than a loss.** Say so in the PR body.

### What egret already has that Metabase computes

Metabase infers the bucketing interval from the data (`computeTimeseriesDataInterval`). **egret does not have to** — the chart spec carries the grouping unit explicitly:

- `params.spec.temporal_units[<x column>]` reaches the builder as `cfg.xAxisUnit` (`options/cartesian.ts:50`).
- `components/chart/temporal-units.ts` already lists the truncation tokens: `minute`, `hour`, `day`, `week`, `month`, `quarter`, `year`, and separates them from *extraction* tokens (`day_of_week`, `month_of_year`, …) via `isExtractUnit`.

Extraction units are already handled: `cartesian.ts:647` maps them through `temporalLabel` before the axis sees them, so `day_of_week` already draws "Wed". **This PR is about the truncation units**, which today reach the axis as raw bucket values.

### Metabase's tick algorithm, and the one place this plan departs from it

`frontend/src/metabase/visualizations/echarts/cartesian/utils/timeseries.ts` in the sibling checkout:

- `TICKS_TIMESERIES_INTERVALS` (`:45`) — a ladder of candidate intervals from 1 ms to 100 years, including the half-steps (2 weeks, 2 months, 2 quarters, 2/5/10/50/100 years) that only the *tick* chooser uses.
- `computeTimeseriesTicksInterval` (`:216`) — start at the index matching the data's own bucketing (`INTERVAL_INDEX_BY_UNIT`, `:88`), walk **up** until `expectedTickCount <= maxTicksForChartWidth`, and step back one if the result would give fewer than `minTickCount = 2`.
- `maxTicksForChartWidth` (`:280`) — measures the **worst-case label for that unit** using `getRepresentativeDates`, not the actual data, plus `TICK_BUFFER_PIXELS = 10`.
- `expectedTickCount` (`:169`) — aligns to unit boundaries, so quarters land on Jan/Apr/Jul/Oct rather than on a naive division of the range.
- `getFormatter` (`:267`) — when the data is weekly but the chart picked a coarser unit, labels are formatted at the coarser one.

**Where this plan departs:** Metabase puts a temporal x on a real `time` axis. egret plots one category per bucket. Switching axis type would change bar width, stacking and the breakout path for every temporal chart in the product — a much larger change than this PR, and it is not what the visible defect needs. So this plan keeps the category axis and thins labels to every Nth.

**That is not a regression of PR 02's "never partial" rule, and the distinction is the whole justification.** PR 02 banned ECharts' `interval: "auto"` because it drops *whichever labels happen to collide* — an arbitrary subset, which reads as missing data. Every Nth on a bucketed axis is not arbitrary: the drawn labels are regular period boundaries, which is exactly what Metabase's tick interval produces and how a reader expects a date axis to behave. State this in the code, not only here — the next person to read `interval: N` will otherwise think PR 02 was undone.

### Testing constraint, carried from PR 02

`components/chart/measure-text.ts` returns `{width: 0, height: 0}` under jsdom by design. Every width-dependent test injects a stub measurer and asserts **the decision**, never a pixel. Two things in PR 02 were honestly reported as unkillable in jsdom for this reason; expect the same here and report rather than invent a test that appears to cover them.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `frontend/src/components/chart/temporal-period.ts` | A truncation unit + a bucket value → the period's name, and the worst-case name at that unit | **Create** |
| `frontend/src/components/chart/__tests__/temporal-period.test.ts` | Every unit, plus the fallbacks | **Create** |
| `frontend/src/components/chart/echarts/axisLabelFit.ts` | Gains the boundary-thinning decision beside the rotate ladder | Modify |
| `frontend/src/components/chart/echarts/options/cartesian.ts` | Name the periods; thin on a temporal axis; auto value-axis for a numeric scatter | Modify |
| `frontend/src/components/chart/SmartScalarChart.tsx` | Name the period instead of printing the bucket's first day | Modify |
| `frontend/src/components/chart/echarts/__tests__/temporalXLabels.test.ts` | The wiring | **Create** |

---

## Task 1: The period namer

**Files:**
- Create: `frontend/src/components/chart/temporal-period.ts`
- Create: `frontend/src/components/chart/__tests__/temporal-period.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect } from "vitest";
import { periodName, widestPeriodName } from "../temporal-period";

describe("periodName", () => {
  /**
   * The defect: a chart grouped by month drew "2026-01-01", the bucket's first DAY. The bucket is
   * January, and a reader who sees a date reasonably concludes the chart is daily. Observed live
   * against Metabase, which draws "January 2026", and on the trend card, which drew
   * "December 1, 2026" for a monthly comparison.
   */
  it("names each truncation unit the way Metabase does", () => {
    expect(periodName("year", "2026-01-01")).toBe("2026");
    expect(periodName("quarter", "2026-07-01")).toBe("Q3 2026");
    expect(periodName("month", "2026-01-01")).toBe("January 2026");
    expect(periodName("day", "2026-01-01")).toBe("January 1, 2026");
  });

  it("keeps the time on sub-day units, where the date alone would be ambiguous", () => {
    expect(periodName("hour", "2026-01-01T13:00:00Z")).toBe("January 1, 2026, 1:00 PM");
    expect(periodName("minute", "2026-01-01T13:05:00Z")).toBe("January 1, 2026, 1:05 PM");
  });

  it("names a week by the days it spans rather than by its number", () => {
    // A week number means nothing without knowing the convention; the span is self-describing.
    expect(periodName("week", "2026-01-05")).toBe("January 5 – 11, 2026");
  });

  /**
   * Total, like `temporalLabel` beside it: an unknown unit, a value that is not a date, and an
   * extraction unit all fall through to the raw string rather than throwing inside a formatter
   * that ECharts calls per label.
   */
  it("falls through rather than throwing", () => {
    expect(periodName(undefined, "2026-01-01")).toBe("2026-01-01");
    expect(periodName("month", "not a date")).toBe("not a date");
    expect(periodName("day_of_week", "3")).toBe("3");
    expect(periodName("month", null)).toBe("");
  });
});

describe("widestPeriodName", () => {
  /**
   * The tick chooser needs the worst case for a unit, not the worst case in this dataset — same
   * reason Metabase measures `getRepresentativeDates` rather than its data. A month axis whose
   * visible range happens to be May–July would otherwise size itself on "May 2026" and clip
   * "September 2026" when the range moves.
   */
  it("returns the longest name a unit can produce, not one from the data", () => {
    expect(widestPeriodName("month")).toBe("September 2026");
    expect(widestPeriodName("quarter").length).toBe("Q1 2026".length);
    expect(widestPeriodName("year").length).toBe("2026".length);
  });

  it("is at least as wide as any name that unit actually produces", () => {
    for (const unit of ["year", "quarter", "month", "day", "week"] as const) {
      const widest = widestPeriodName(unit).length;
      for (let m = 0; m < 12; m++) {
        const v = `2026-${String(m + 1).padStart(2, "0")}-05`;
        expect(periodName(unit, v).length).toBeLessThanOrEqual(widest);
      }
    }
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/temporal-period.test.ts`
Expected: FAIL — `Failed to resolve import "../temporal-period"`.

- [ ] **Step 3: Implement**

Create `frontend/src/components/chart/temporal-period.ts`. Use `dayjs` (already a dependency — check how `SmartScalarChart` and `format.ts` import it and follow that). Key points to get right:

- Only truncation units name a period. Reuse `isExtractUnit` from `./temporal-units` rather than restating the token list; an extraction unit's value is already a finished word by the time it reaches a formatter.
- The exact strings are the contract. Check them against Metabase before committing: the sibling checkout's date formatting lives under `frontend/src/metabase/lib/formatting/`. If Metabase's month form differs from `"January 2026"`, follow Metabase and fix the test — the point is parity, and the test above is this plan's guess.
- `widestPeriodName(unit)` returns the longest name the unit can produce. For a month that is September (nine letters); derive it rather than hard-coding a month index, so it stays right if the format changes.
- Total: never throw. This runs inside an ECharts `formatter`, once per label.

- [ ] **Step 4: Run the test**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/temporal-period.test.ts`
Expected: PASS.

- [ ] **Step 5: Prove each branch can fail**

For each unit (`year`, `quarter`, `month`, `week`, `day`, `hour`, `minute`) make `periodName` return a different unit's format and confirm a test catches it. Then make `widestPeriodName` return a *narrow* name (e.g. always "May 2026") and confirm the second `widestPeriodName` test fails. Paste the results.

**This is not optional.** Nine defects across PRs 01 and 02 looked right on reading and were caught only by running something, including two assertions that passed against a stubbed implementation.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/temporal-period.ts \
        frontend/src/components/chart/__tests__/temporal-period.test.ts
git commit -m "feat(charts): name the period a bucket covers"
```

---

## Task 2: Name the periods on the axis, and on the trend card

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts` (the category-axis `formatter`, around `:976`)
- Modify: `frontend/src/components/chart/SmartScalarChart.tsx` (`curLabel`, around `:63`)
- Create: `frontend/src/components/chart/echarts/__tests__/temporalXLabels.test.ts`

- [ ] **Step 1: Write the failing test**

Assert through `buildCartesianOption` that a month-bucketed axis formats `"2026-01-01"` as `"January 2026"`, that an explicit column format still wins (a user who set one has said what they want), and that a non-temporal axis is untouched. Then assert the trend card renders the period name rather than the bucket's first day — `SmartScalarChart` is a React component, so render it with `@testing-library/react` as its existing tests do.

Find the trend card's existing test file first and extend it rather than starting a second one.

- [ ] **Step 2: Watch it fail, then implement**

In `cartesian.ts`'s category-axis formatter (`:976-981`), the current order is: a column format if one is set, otherwise the raw string. Insert the period name **between** them — an explicit column format still wins, but a bucketed temporal axis with no format now names its period instead of falling through to raw.

In `SmartScalarChart.tsx`, `curLabel` currently runs the dimension value through `formatValue`. Use `periodName(unit, value)` when `unit` is a truncation unit, keeping the existing behaviour otherwise. **`unit` is already a prop** (`:35`) — it is what `PERIOD_NAME` at `:18` uses for the "vs previous month" caption, so the fix is to use the same input for the label above it.

- [ ] **Step 3: Verify the whole chart suite**

Run: `cd frontend && npx vitest run src/components/chart && npx tsc --noEmit`

Existing tests that assert a raw date on an axis will fail. **Read each before changing it**: a test asserting `"2026-01-01"` on a *month-bucketed* axis was pinning the defect and should be updated; one on an axis with no `xAxisUnit` was pinning correct behaviour and a failure there means the guard is wrong.

- [ ] **Step 4: Mutate and commit**

Make the formatter ignore the unit, and confirm both the axis test and the trend test fail. Paste it.

```bash
git commit -m "feat(charts): a bucketed axis reads as its periods, and so does the trend"
```

---

## Task 3: Thin a temporal axis to period boundaries

**Files:**
- Modify: `frontend/src/components/chart/echarts/axisLabelFit.ts`
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`
- Modify: `frontend/src/components/chart/echarts/__tests__/temporalXLabels.test.ts`

Task 2 makes the labels shorter, which lets more of them fit; this task handles the case where they still do not. Twelve months at "September 2026" width on a 270px card is ~22px each against a ~95px label — every adjacent pair collides, so PR 02's ladder rotates or hides. Metabase draws two upright.

- [ ] **Step 1: Extend the fit with a thinning decision**

`fitXLabels` currently returns `show` / `rotate` / `hide`. Add a fourth outcome for a temporal axis: draw every Nth label upright.

Choose N the way Metabase chooses its interval: the smallest step that makes the drawn labels fit, measured against `widestPeriodName(unit)` rather than against this dataset's labels. Prefer steps that land on natural boundaries for the unit — for months, 1, 2, 3, 6, 12; for quarters, 1, 2, 4 — so the drawn labels are Jan/Jul rather than an arbitrary stride. Metabase's `TICKS_TIMESERIES_INTERVALS` is the list to follow; take the entries at and above the data's own unit.

**The rule for when thinning is allowed, and it must be enforced in code rather than trusted:** only when the axis is a bucketed temporal one — `cfg.xAxisUnit` is a truncation unit. An ordinal axis of product names keeps PR 02's all-or-nothing behaviour, because there is no such thing as "every second product".

- [ ] **Step 2: Apply it**

Where PR 02 writes `interval: 0`, a thinned axis writes `interval: N - 1` (ECharts counts skipped labels, not stride — verify this against the vendored source before relying on it).

**Write the justification at the `interval` line**, not only in this plan: PR 02 banned `interval: "auto"` for dropping an arbitrary subset; a regular stride on period boundaries is what a date axis is expected to do and is what Metabase produces. Without that comment the next reader will read `interval: N` as PR 02 being undone.

- [ ] **Step 3: Test the ladder and prove each rung fails**

Twelve months wide enough for all → every label. Narrow → every 6th (Jan, Jul). Narrower → every 12th. Too narrow for even one → fall through to PR 02's rotate/hide, unchanged. Assert the *decision*, injecting a stub measurer.

Also assert the negative: a **non-temporal** axis with the same label count and width still gets all-or-nothing, never a stride.

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(charts): a date axis draws every Nth period rather than none"
```

---

## Task 3b: The waterfall

**Added after Task 3's implementer found a scope hole in this plan** — the same class of hole PR 02
had, where the goal and the acceptance list named a chart no task touched.

This plan's Background names the waterfall among the charts that now draw nothing, and Task 5's
acceptance list says "**waterfall** draws its month labels". But `WaterfallBuildConfig` has **no
`xAxisUnit` at all** and `ChartRenderer` never passes one, so Task 2 could not name its periods and
Task 3 cannot thin it. A month-bucketed waterfall still draws raw `"2026-01-01"` and gets PR 02's
rotate-or-hide on a narrow card. Verified before adding this task.

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/waterfall.ts`
- Modify: `frontend/src/components/chart/ChartRenderer.tsx`
- Test: extend `frontend/src/components/chart/echarts/__tests__/boxWaterfallXLabels.test.ts`

- [x] **Step 1: Thread the unit in**

Add `xAxisUnit?: string` to `WaterfallBuildConfig` and pass it from `ChartRenderer`'s waterfall
branch, from `params.spec.temporal_units[<the x column>]` — the same source cartesian reads
(`ChartRenderer.tsx`, the cartesian branch, is the shape to copy).

- [x] **Step 2: Name the periods and allow thinning**

Mirror what Task 2 and Task 3 did for cartesian, and mirror it *exactly* — the same three ordered
arms (column format if it **specifies date rendering** → `periodName` → the previous fallback), and
the same single `drawLabel` closure feeding both the fitter's `labels` and `axisLabel.formatter`.

Two traps, both already paid for once:

- **Arm 1 must use `specifiesDateRendering(fmt)`, not `fmt` truthiness.** Task 2 shipped truthiness
  and a reviewer found the panel mints date-silent format objects today (`FormatSection.tsx:104`
  writes `{ column_title }`), so renaming a column silently reverted the axis. The predicate lives
  in `format.ts` beside `formatDate`.
- **One `drawLabel`, not two.** Task 2 first measured the raw category while drawing the period name,
  which made a `year` axis over-measure 10 characters against 4 and rotate labels that would have sat
  upright.

- [x] **Step 3: Same verification bar**

Every rung mutated and seen to fail; the negative case (a non-bucketed waterfall is never thinned)
asserted; and the `ChartRenderer` → builder seam covered, because PR 02 found that seam untested on
all three charts and only a mutation caught it.

- [x] **Step 4: Commit**

```bash
git commit -m "feat(charts): the waterfall's steps read as periods too"
```

---

## Task 4: Round ticks for a numeric x-axis

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts`

The scatter on the harness plots `price` — 196 numeric values — as 196 categories. Before PR 02 it drew three arbitrary labels (`15.69`, `48.93`, `84.14`); now it draws none. Metabase draws `0 20 40 60 80 100`, because its x is a **value axis** and ECharts generates round ticks for one.

egret already supports this: `xScale: "linear"` routes through `scaleAxis` (`cartesian.ts:400`, `:1002`). It is only ever set by hand.

- [ ] **Step 1: Auto-select a value axis for a numeric scatter**

When the kind is `scatter`, the x column's type is numeric, and the user has not chosen an `xScale`, default to `"linear"`. **Scatter only.** A bar chart's categories are discrete even when they are numbers — bar width and stacking come off the axis type, and changing it for bar charts is a much larger change than this defect needs.

Do not write the chosen scale into stored settings; resolve it at render time. That is this programme's standing rule, and it is why a chart saved before this change picks the new behaviour up.

- [ ] **Step 2: Test and mutate**

Assert a numeric-x scatter builds a `value` x-axis and a varchar-x scatter still builds a category axis; assert an explicit `xScale` still wins. Mutate the type check and confirm failure.

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(charts): a numeric scatter gets a value axis, and round ticks with it"
```

---

## Task 5: Live comparison and PR

- [ ] **Step 1: Gates**

```bash
cd frontend && npx tsc --noEmit && npm run lint && npx vitest run
```

- [ ] **Step 2: The live comparison — this is the acceptance test**

PR 02 established how to do this, and it caught a regression four review rounds had missed. Follow it:

```bash
cd frontend && npm run build
```

then serve `dist` on a spare port with a small Python static server that proxies `/api`, `/catalog` and `/oauth` to `127.0.0.1:30080` and falls back to `index.html` for SPA routes. Log in through the UI — it is a different origin, so there is no cookie. The pattern is recorded in the parity-harness memory; do not try the Vite dev server, which Playwright could not reach.

Open **ZZ Chart Parity** (`/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`) beside Metabase's (`localhost:3033/dashboard/2`). Confirm:

- **line, area, combo** draw named periods — "January 2026", "July 2026" — where they drew nothing after PR 02
- **waterfall** draws its month labels
- **scatter** draws round price ticks
- **bar** and **box plot** still draw all four category names (PR 02's fix must survive)
- the **trend** card names its period instead of printing a first day

**Measure in the DOM, not by eye.** PR 02's regression was a crushed plot area that looked plausible in a thumbnail; a probe counting text nodes and the tallest drawn shape as a share of card height is what exposed it.

- [ ] **Step 3: Open the PR and stop**

The body must state what changed visually, that this is what makes PR 02 read as an improvement, and any comparison that could not be run. **Do not merge.**

---

## Self-review against the spec

**Spec coverage.** Workstream 3 lists five items: auto-detect a temporal column (Task 4 does this for numeric scatter; **the temporal case is deliberately not done** — see below), port the interval ladder (Task 3, adapted to a category axis), measure the worst-case label for the unit (Task 1's `widestPeriodName`, used by Task 3), name the period (Tasks 1 and 2), and the same treatment for a numeric x (Task 4). The trend card's period label, which the spec lists under workstream 3's outcomes, is Task 2.

**A deliberate departure from the spec, stated rather than buried.** The spec says to "detect a temporal column and select the timeseries scale automatically, the way Metabase decides axis type from the column rather than from a setting." This plan does **not** do that. Switching a temporal x to a real `time` axis changes bar width, stacking and the breakout path for every temporal chart in the product; the visible defect — unreadable and then absent labels — is fixed by naming the periods and thinning to boundaries, on the axis egret already uses. If a later change wants the time axis for its own reasons, `applyXScale` and `scaleAxis` are already there. **Update the spec to record this, as part of Task 3**, so the next reader does not treat it as an oversight.

**Placeholders.** Task 1 step 3 deliberately does not fix the exact label strings: they are a parity question and the answer is in the Metabase checkout, not in my head. The step says to check and to fix the test if it guessed wrong, which is the honest shape for a contract this plan cannot verify without reading a file it has not read.

**Type consistency.** `periodName` / `widestPeriodName` are defined in Task 1 and used with those names in Tasks 2 and 3. `isExtractUnit` and `temporalLabel` are existing exports of `components/chart/temporal-units.ts`. `cfg.xAxisUnit` is `options/cartesian.ts:50`. `fitXLabels` and its `XLabelFit` union are from `echarts/axisLabelFit.ts`, and Task 3 adds a fourth arm to that union — every existing `switch`/narrow on it will need the new case, which `tsc` will find.

**Known risk.** Task 3 changes what `interval` may hold, three weeks after PR 02 made "never a subset" the rule. If the justification comment is weak, a future reader will "restore" `interval: 0` and silently undo this. The plan asks for that comment twice, in two places, on purpose.
