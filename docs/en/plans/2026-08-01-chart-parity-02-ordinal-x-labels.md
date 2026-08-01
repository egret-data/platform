# Chart parity 02 — ordinal x-axis labels — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decide the x-axis label treatment — draw upright, rotate 45°, rotate 90°, or hide the axis — from the measured plot width, so that bar, row, box plot and waterfall stop silently dropping category labels on a narrow card.

**Architecture:** Port Metabase's ladder from `echarts/cartesian/layout/index.ts` into a pure module that takes the labels and a plot width and returns a decision. `buildCartesianOption` calls it when it has a size (PR 01 gave it one) and keeps today's behaviour when it does not. The load-bearing change is that labels become all-or-nothing: `interval: 0` when they are drawn, `show: false` when they are not — never ECharts' `interval: "auto"`, which draws *some*.

**Tech Stack:** TypeScript, React 18, ECharts (SVG), vitest.

**Spec:** `docs/en/specs/2026-08-01-chart-parity-design.md` (workstream 2). **Depends on PR 01** (#385, merged) for the `_size` parameter.

---

## Background an engineer needs

### What is wrong today

`frontend/src/components/chart/echarts/options/cartesian.ts:899`:

```ts
const rotate = !horizontal && longest > 10 && categories.length > 6 ? 30 : 0;
```

and at `:913`, `axisLabel: { …, interval: "auto", rotate }`.

The heuristic asks about label *length* and *count*; whether labels fit is a question about *width*. With four categories whose longest is "Doohickey" (9 characters) on a one-third-width dashboard card it declines to rotate, and then `interval: "auto"` lets ECharts drop three of the four labels. The axis reads as missing data. This was observed live on the ZZ Chart Parity harness against Metabase on identical numbers.

### Metabase's algorithm, verbatim

From `frontend/src/metabase/visualizations/echarts/cartesian/layout/index.ts` in the sibling `metabase` checkout. Read `getAutoAxisEnabledSetting` (`:296`) before starting.

1. Auto-selection applies **only** when the setting is still its default and the scale is ordinal or histogram. An explicit user choice is honoured verbatim and never overridden.
2. `dimensionWidth = boundaryWidth / xValuesCount` (`:646`).
3. Overlap is tested **per adjacent pair**, on labels padded with one space each side (`getPaddedAxisLabel`, `option/utils.ts:17`) to be pessimistic, with `HORIZONTAL_TICKS_GAP = 6` (`:658`). Three conditions (`:688`):
   - `leftW/2 + rightW/2 + GAP > dimensionWidth`
   - `rightW + GAP/2 > dimensionWidth`
   - `leftW + GAP/2 > dimensionWidth`
4. No overlap → draw all labels upright.
5. Overlap → `dimensionWidth >= fontSize * 2.1` gives 45°; else `dimensionWidth >= fontSize * 1.2` gives 90°; in each case only if the rotated label height stays under 70% of chart height (`checkHeight`, `:280` — `maxW / √2 / outerHeight` for 45°, `maxW / outerHeight` for 90°); otherwise **hide the axis entirely**.
6. At most 50 ticks are measured, sampled at **evenly spaced indices** (`getEvenlySpacedIndices`, `:357`) — not the first 50. On a sorted axis the widest label is as likely to be last as first.

> **Correction to the spec.** `docs/en/specs/2026-08-01-chart-parity-design.md` says "only the first 50 ticks are measured". That is wrong — it is an even sample across the whole dataset. Fix that line as part of Task 1.

### The plot-width problem, and how this plan handles it

Metabase computes its own layout, so `boundaryWidth` is exact. egret does not: `CHART_THEME.grid` (`echarts/palette.ts:39`) sets `containLabel: true`, which makes **ECharts** shrink the plot area to fit the axis labels. So the plot width is not `size.width − grid.left − grid.right`, and it is not knowable exactly before ECharts lays out.

This plan estimates it, and deliberately estimates **low**:

```
plotWidth ≈ size.width − grid.left − grid.right − valueAxisGutter − (yTitle ? 26 : 0)
```

where `valueAxisGutter` is the measured width of the widest value-axis label. Those labels are not known exactly either (ECharts picks the ticks), but they are bounded: format the data's min and max through the same `fmtFor(metrics)` the axis uses and take the wider. A "nice" tick inside the extent cannot format wider than the extremes for any formatter this codebase uses (grouped numbers, compact numbers, currency).

**Underestimating is the safe direction and that is why it is chosen.** Too small a `plotWidth` means we think labels collide when they might not, so we rotate something that would have fitted — mildly ugly, still readable. Too large means we leave labels upright and ECharts drops them, which is the defect this PR exists to remove.

Do not "improve" this by turning off `containLabel` and computing the grid explicitly. That is Metabase's design and it is defensible, but it changes the margins of every cartesian chart in the product and belongs in its own change with its own before/after.

### Testing constraint

`frontend/src/components/chart/measure-text.ts` returns `{width: 0, height: 0}` under jsdom by design — see its doc comment, *"Size claims belong in a browser."* Every test here injects a stub measurer and asserts **the decision** (`rotate: 45`, `show: false`, the chosen bottom padding), never a pixel. A test that asserts a pixel through the real measurer is asserting `0`.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `frontend/src/components/chart/echarts/axisLabelFit.ts` | Pure: labels + plot box → draw upright / rotate 45 / rotate 90 / hide, plus the bottom padding that decision needs | **Create** |
| `frontend/src/components/chart/echarts/__tests__/axisLabelFit.test.ts` | The ladder at its boundaries, including the hide branch | **Create** |
| `frontend/src/components/chart/echarts/options/cartesian.ts` | Call the fitter when a size is present; `interval: 0` or `show: false`, never `"auto"` | Modify |
| `frontend/src/components/chart/echarts/__tests__/cartesianXLabels.test.ts` | The wiring: same data at two widths produces different axis options | **Create** |
| `frontend/src/api/charts.ts` | `xLabelRotation?: 45 \| 90` on `CartesianConfig` | Modify |
| `frontend/src/components/chart/viz/cartesian-codec.ts` | Persist the new key | Modify |
| `frontend/src/components/chart/settings/CartesianDisplaySection.tsx` | Replace the x-ticks toggle with the four-value select | Modify |
| `docs/en/specs/2026-08-01-chart-parity-design.md` | Fix the "first 50 ticks" claim | Modify |

---

## Task 1: The fitter

**Files:**
- Create: `frontend/src/components/chart/echarts/axisLabelFit.ts`
- Create: `frontend/src/components/chart/echarts/__tests__/axisLabelFit.test.ts`
- Modify: `docs/en/specs/2026-08-01-chart-parity-design.md`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/echarts/__tests__/axisLabelFit.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { fitXLabels, type XLabelFit } from "../axisLabelFit";
import type { TextMeasurer } from "@/components/chart/measure-text";

/** Every label is `perChar` wide per character and `fontSize` tall. Injected so the ladder can be
 *  driven to each of its branches exactly; jsdom's real measurer returns zeroes. */
const stub = (perChar: number): TextMeasurer =>
  (text, style) => ({ width: text.length * perChar, height: Number(style.size) });

const fit = (labels: string[], plotWidth: number, outerHeight = 400, perChar = 8): XLabelFit =>
  fitXLabels({ labels, plotWidth, outerHeight, fontSize: 13, measure: stub(perChar) });

describe("fitXLabels", () => {
  it("draws every label upright when they all fit", () => {
    // 4 labels x 80px each = 320 of 800: no adjacent pair collides.
    expect(fit(["Widget", "Gadget", "Gizmo", "Doohickey"], 800)).toEqual({
      kind: "show", rotate: 0, bottomPad: 0,
    });
  });

  /**
   * The defect this module exists for. Same four labels on a one-third-width card: upright they
   * collide, so today's `interval: "auto"` drew ONE of the four. Metabase rotates instead.
   */
  it("rotates 45° when labels collide but there is still room per label", () => {
    const r = fit(["Widget", "Gadget", "Gizmo", "Doohickey"], 240);
    expect(r.kind).toBe("rotate");
    expect(r).toMatchObject({ rotate: 45 });
  });

  it("rotates 90° when there is not even room for 45°", () => {
    // dimensionWidth = 60/4 = 15px. 15 >= 13*2.1 (27.3) is false; 15 >= 13*1.2 (15.6) is false too,
    // so widen slightly to land on the 90 branch rather than the hide branch.
    const r = fit(["Widget", "Gadget", "Gizmo", "Doohickey"], 4 * 13 * 1.3);
    expect(r).toMatchObject({ kind: "rotate", rotate: 90 });
  });

  /**
   * The branch most easily left untested, and the one that changes what a user sees most: when even
   * a rotated label would eat 70% of the chart's height, Metabase hides the axis rather than
   * drawing a wall of text. Asserted with a label long enough that its rotated height exceeds the
   * ratio, so this is the height guard firing and not the width ladder.
   */
  it("hides the axis when a rotated label would be taller than 70% of the chart", () => {
    const long = "Enterprise Widget Assembly, North-Western Distribution Region";
    expect(fit([long, long, long, long], 4 * 13 * 1.3, 120)).toEqual({ kind: "hide" });
  });

  it("measures the widest label even when it is last", () => {
    // An even sample, not the first N: on a sorted axis the widest label is as likely to be last.
    const labels = [...Array(200).fill("a"), "wwwwwwwwwwwwwwwwwwwwwwwwwwwwww"];
    expect(fit(labels, 400).kind).not.toBe("show");
  });

  it("reserves bottom padding proportional to the rotation", () => {
    const upright = fit(["a", "b"], 800);
    const rotated = fit(["Widget", "Gadget", "Gizmo", "Doohickey"], 240);
    expect(upright.kind === "show" && upright.bottomPad).toBe(0);
    expect(rotated.kind === "rotate" && rotated.bottomPad).toBeGreaterThan(0);
  });

  it("draws upright when there are no labels rather than dividing by zero", () => {
    expect(fit([], 800)).toEqual({ kind: "show", rotate: 0, bottomPad: 0 });
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/axisLabelFit.test.ts`

Expected: FAIL — `Failed to resolve import "../axisLabelFit"`.

- [ ] **Step 3: Implement**

Create `frontend/src/components/chart/echarts/axisLabelFit.ts`:

```ts
import { measureText as realMeasureText, type TextMeasurer } from "@/components/chart/measure-text";
import { CHART_THEME } from "./palette";

/**
 * Whether a category axis can draw its labels, and how — ported from Metabase's
 * `getAutoAxisEnabledSetting` (`echarts/cartesian/layout/index.ts`).
 *
 * The property that matters is that the answer is never PARTIAL. egret used ECharts'
 * `interval: "auto"`, which silently drops labels that would collide; four categories on a
 * one-third-width dashboard card came out with one label drawn, and an axis missing three of its
 * four names reads as missing data rather than as a layout decision. Metabase draws all of them,
 * rotates all of them, or hides the axis — and this returns the same three answers.
 */
export type XLabelFit =
  | { kind: "show"; rotate: 0; bottomPad: number }
  | { kind: "rotate"; rotate: 45 | 90; bottomPad: number }
  | { kind: "hide" };

/** Metabase's own constants, kept as its names so the two can be diffed. */
const HORIZONTAL_TICKS_GAP = 6;
const ROTATE_45_MIN_FACTOR = 2.1;
const ROTATE_90_MIN_FACTOR = 1.2;
/** A rotated label may not eat more than this share of the chart's height. */
const MAX_LABEL_HEIGHT_RATIO = 0.7;
/** Formatting and measuring thousands of labels is not free. Metabase samples at EVENLY SPACED
 *  indices rather than taking a prefix, because on a sorted axis the widest label is as likely to
 *  be the last as the first. */
const TICKS_TO_MEASURE = 50;

/** Metabase pads each label with a space per side before measuring, so the estimate is pessimistic
 *  by roughly one space at each end — `getPaddedAxisLabel`, `option/utils.ts`. */
const pad = (label: string) => ` ${label} `;

function sample<T>(items: T[], n: number): T[] {
  if (items.length <= n) return items;
  const step = (items.length - 1) / (n - 1);
  return Array.from({ length: n }, (_, i) => items[Math.round(i * step)]);
}

export function fitXLabels({
  labels, plotWidth, outerHeight, fontSize = CHART_THEME.axisFontSize, measure = realMeasureText,
}: {
  labels: string[];
  /** The drawable width the categories share. See the plan's note on why this is an UNDER-estimate. */
  plotWidth: number;
  outerHeight: number;
  fontSize?: number;
  /** Injectable: jsdom's real measurer returns zeroes, so tests drive the ladder with a stub. */
  measure?: TextMeasurer;
}): XLabelFit {
  const upright: XLabelFit = { kind: "show", rotate: 0, bottomPad: 0 };
  if (labels.length === 0 || plotWidth <= 0) return upright;

  const style = { size: fontSize, family: "inherit", weight: 400 } as const;
  const widths = sample(labels, TICKS_TO_MEASURE).map((l) => measure(pad(l), style).width);
  const dimensionWidth = plotWidth / labels.length;

  // Adjacent PAIRS, not the widest label against the average: two wide neighbours collide even on an
  // axis whose mean label is narrow.
  const overlaps = widths.some((right, i) => {
    if (i === 0) return false;
    const left = widths[i - 1];
    return left / 2 + right / 2 + HORIZONTAL_TICKS_GAP > dimensionWidth
      || right + HORIZONTAL_TICKS_GAP / 2 > dimensionWidth
      || left + HORIZONTAL_TICKS_GAP / 2 > dimensionWidth;
  });
  if (!overlaps) return upright;

  const maxWidth = Math.max(...widths);
  const fitsRotated = (rotate: 45 | 90) =>
    (rotate === 90 ? maxWidth : maxWidth / Math.SQRT2) / outerHeight < MAX_LABEL_HEIGHT_RATIO;

  // A rotated label's footprint below the axis IS its measured width, projected.
  const padFor = (rotate: 45 | 90) => Math.ceil(rotate === 90 ? maxWidth : maxWidth / Math.SQRT2);

  if (dimensionWidth >= fontSize * ROTATE_45_MIN_FACTOR) {
    return fitsRotated(45) ? { kind: "rotate", rotate: 45, bottomPad: padFor(45) } : { kind: "hide" };
  }
  if (dimensionWidth >= fontSize * ROTATE_90_MIN_FACTOR) {
    return fitsRotated(90) ? { kind: "rotate", rotate: 90, bottomPad: padFor(90) } : { kind: "hide" };
  }
  return { kind: "hide" };
}
```

- [ ] **Step 4: Run the test**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/axisLabelFit.test.ts`

Expected: PASS, 7 tests. If the 45°/90° cases land on the wrong branch, adjust the *fixture widths* to sit either side of `fontSize * 2.1` and `fontSize * 1.2` — do not adjust the constants, which are ported deliberately.

- [ ] **Step 5: Prove each branch can fail**

For each of the four outcomes (`show`, `rotate 45`, `rotate 90`, `hide`), make the implementation return a different one unconditionally, run, and record which tests fail. Every outcome must have at least one test that catches it. Paste the four results.

This is not optional. Seven defects in PR 01 looked right on reading and were only caught by running a mutation, two of them in the plan.

- [ ] **Step 6: Fix the spec's sampling claim**

In `docs/en/specs/2026-08-01-chart-parity-design.md`, workstream 2, replace "Only the first 50 ticks are measured, for cost." with a sentence saying the sample is 50 **evenly spaced** ticks, and why: on a sorted axis the widest label is as likely to be last as first. Make the same correction in `docs/vi/specs/2026-08-01-chart-parity-design.md`.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/chart/echarts/axisLabelFit.ts \
        frontend/src/components/chart/echarts/__tests__/axisLabelFit.test.ts \
        docs/en/specs/2026-08-01-chart-parity-design.md \
        docs/vi/specs/2026-08-01-chart-parity-design.md
git commit -m "feat(charts): decide x-axis label treatment from measured width"
```

---

## Task 2: Wire it into the cartesian builder

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts` (the `rotate` computation at `:899`, the `axisLabel` at `:913`, the `grid` at `:988-993`)
- Create: `frontend/src/components/chart/echarts/__tests__/cartesianXLabels.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/echarts/__tests__/cartesianXLabels.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const cols = [{ name: "category", type: "varchar" }, { name: "revenue", type: "double" }];
const rows: unknown[][] = [
  ["Widget", 10], ["Gadget", 20], ["Gizmo", 30], ["Doohickey", 40],
];
const cfg = { kind: "bar" as const, xAxis: "category", yMetrics: ["revenue"] };
const xAxisOf = (o: unknown) => (o as { xAxis: { axisLabel: { interval: unknown; rotate?: number }; show?: boolean } }).xAxis;

describe("cartesian x-axis labels", () => {
  /**
   * The whole point of the change: labels are all-or-nothing. `interval: "auto"` let ECharts drop
   * the ones that collided, which is how four categories came out with one label on a dashboard
   * card. Whatever is decided, the axis never draws a subset.
   */
  it("never leaves the interval to ECharts once it has a size", () => {
    const wide = buildCartesianOption(cols, rows, cfg, { width: 1200, height: 500 });
    const narrow = buildCartesianOption(cols, rows, cfg, { width: 320, height: 260 });
    expect(xAxisOf(wide).axisLabel.interval).toBe(0);
    expect(xAxisOf(narrow).axisLabel.interval).toBe(0);
  });

  it("keeps today's behaviour when no size is available", () => {
    // Unit tests and any caller passing a plain option object have no measurement. Leaving ECharts
    // to thin the labels is worse than rotating, but it is what shipped, and guessing a width here
    // would be the very thing this module was written to stop.
    expect(xAxisOf(buildCartesianOption(cols, rows, cfg)).axisLabel.interval).toBe("auto");
  });

  it("honours an explicit rotation over anything measurement would choose", () => {
    const o = buildCartesianOption(cols, rows, { ...cfg, xLabelRotation: 90 }, { width: 1600, height: 900 });
    expect(xAxisOf(o).axisLabel.rotate).toBe(90);
  });

  it("reserves more room below the axis when the labels rotate", () => {
    const flat = buildCartesianOption(cols, rows, { ...cfg }, { width: 1600, height: 900 }) as { grid: { bottom: number } };
    const turned = buildCartesianOption(cols, rows, { ...cfg, xLabelRotation: 90 }, { width: 1600, height: 900 }) as { grid: { bottom: number } };
    expect(turned.grid.bottom).toBeGreaterThan(flat.grid.bottom);
  });
});
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/cartesianXLabels.test.ts`

Expected: FAIL on the first test — `interval` is `"auto"` at both widths today.

- [ ] **Step 3: Estimate the plot width**

In `cartesian.ts`, just above the current `rotate` computation, add a helper that turns the container size into the width the categories share. Read the plan's "plot-width problem" section before writing this; the under-estimate is deliberate and must be stated in the comment.

```ts
/** The width the categories share, estimated. `CHART_THEME.grid` sets `containLabel: true`, so
 *  ECharts — not this builder — decides the real plot box, and it picks the value-axis ticks too.
 *  Both are bounded: the widest tick cannot format wider than the widest datum, and `widestOf`
 *  already computes that magnitude for the compaction decision just above. Deliberately an
 *  UNDER-estimate; erring small rotates a label that would have fitted, erring large leaves it
 *  upright and ECharts drops it, which is the defect being fixed. */
function estimatePlotWidth(size: EChartsSize, widestValueLabel: string, hasAxisTitle: boolean): number {
  const g = CHART_THEME.grid;
  const gutter = measureText(widestValueLabel, {
    size: CHART_THEME.axisFontSize, family: "inherit", weight: 400,
  }).width;
  return size.width - (g.left as number) - (g.right as number) - gutter - (hasAxisTitle ? 26 : 0);
}
```

**Identifiers verified in scope at that point in `baseOption`** — use these, not invented ones:
`categories` (the labels; `displayCats` is the caller's name for the same array and is *not* in
scope here), `widestOf(metrics)` at `:862` returning the widest absolute datum, `fmtFor(metrics)`
at `:877` returning the value-axis formatter, `leftSide` at `:859`, `yTitle` at `:861`, and
`horizontal`. Note `cfg.yMin` / `cfg.yMax` are the user's manual axis bounds, **not** the data
extent — do not use them for the gutter. `leftTitle` is computed at `:987`, *after* this point;
since this path is already guarded on `!horizontal`, `yTitle` is the left-hand title here.

- [ ] **Step 4a: Thread the size into `baseOption`**

The `rotate` computation lives in `baseOption` (`:829`), not in `buildCartesianOption`, and
`baseOption`'s signature is `(cfg, categories, series)` — the size does not reach it. Add a fourth
optional parameter:

```ts
function baseOption(
  cfg: CartesianBuildConfig,
  categories: (string | number)[],
  series: unknown[],
  /** Absent when the caller has no measurement; see the x-label fit below. */
  size?: EChartsSize,
): EChartsCoreOption {
```

and pass it at **all three** call sites — `:605` (the empty-data path), `:643` (breakout) and `:702`
(the folded path). Missing one is a silent behaviour split where one chart shape measures and
another does not, with nothing failing.

The scatter/value-axis branch (`scatterXY`, which returns before `baseOption`) is out of scope:
its x-axis is a value axis, not a category axis, and nice-tick generation for it is PR 03.

- [ ] **Step 4b: Replace the heuristic**

Replace `:899`'s `const rotate = …` with a call to `fitXLabels`, guarded on having a size and on the axis being the bottom one:

```ts
// `_size` is absent for unit tests and for any caller that passes a plain option object rather
// than a size function; there is nothing to measure against, so the old behaviour stands.
const fit = size && !horizontal
  ? fitXLabels({
      labels: categories.map(String),
      plotWidth: estimatePlotWidth(size, fmtFor(leftSide)(widestOf(leftSide)), Boolean(yTitle)),
      outerHeight: size.height,
    })
  : undefined;
// An explicit choice wins over anything measurement would pick — Metabase's auto-selection also
// only applies while the setting is untouched.
const rotate = cfg.xLabelRotation ?? (fit?.kind === "rotate" ? fit.rotate : 0);
```

Rename the parameter from `_size` to `size` now that it is read, and update its doc comment: it is no longer "unread today". Leave `buildGaugeOption`'s and `buildSankeyOption`'s parameters as `_size` — they are still unread, and PR 05 is where they stop being.

Then in the `axisLabel` at `:913`, replace `interval: "auto" as const` with `interval: fit ? 0 : ("auto" as const)`, and add `show: fit?.kind !== "hide"`. In the `grid` at `:990`, replace `(rotate ? 18 : 0)` with the fit's `bottomPad` when there is one, falling back to `18` for an explicit rotation with no measurement.

> **Corrected after Task 1's review.** An earlier version of this step computed `fit` unconditionally
> and then wrote `show: fit?.kind !== "hide"`. That contradicts this plan's own rule that auto-selection
> applies **only while the setting is untouched**: a user who explicitly picks 45° on a narrow card
> would get their rotation honoured *and their axis hidden anyway*, which is worse than either
> outcome alone and would look like the setting doing nothing.
>
> Skip the measurement entirely when the user has chosen:
>
> ```ts
> const fit = size && !horizontal && cfg.xLabelRotation == null ? fitXLabels({ … }) : undefined;
> const rotate = cfg.xLabelRotation ?? (fit?.kind === "rotate" ? fit.rotate : 0);
> ```
>
> With `fit` undefined on the explicit path, `interval` falls back to `"auto"` — which is wrong for a
> different reason, since the whole point is that labels are never partial. So on the explicit path
> set `interval: 0` too. Write the guard as "did the user choose?", not as "is there a fit?", and add
> a test: an explicit rotation on a card narrow enough that measurement would have said `hide` must
> still draw the axis, rotated as asked.

- [ ] **Step 5: Run both new test files and the whole chart suite**

Run: `cd frontend && npx vitest run src/components/chart`

Expected: PASS. If existing cartesian tests fail because they assert `interval: "auto"`, check whether they pass a size — if they do not, the guard is wrong and the fix is in the builder, not the test.

- [ ] **Step 6: Typecheck**

Run: `cd frontend && npx tsc --noEmit`

vitest does not typecheck in this repo; this is the only gate that catches a signature error.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/chart/echarts/options/cartesian.ts \
        frontend/src/components/chart/echarts/__tests__/cartesianXLabels.test.ts
git commit -m "feat(charts): draw every x-axis label, rotate them, or draw none"
```

---

## Task 3: The setting and its control

**Files:**
- Modify: `frontend/src/api/charts.ts` (`CartesianConfig`)
- Modify: `frontend/src/components/chart/viz/cartesian-codec.ts` (the `KEYS` list)
- Modify: `frontend/src/components/chart/settings/CartesianDisplaySection.tsx:229-230`

- [ ] **Step 1: Add the config field**

In `CartesianConfig`, beside the existing `xAxisTicks?: boolean`:

```ts
/** An explicit rotation, which suppresses the measured choice. Metabase folds this into
 *  `graph.x_axis.axis_enabled` as one five-value key; egret keeps two fields because waterfall,
 *  box plot and sankey already store this shape (`settings/ChartAxesSection.tsx`), and one control
 *  storing two different shapes across the chart module would be worse than differing from
 *  Metabase's key. The panel select is a projection over both. */
xLabelRotation?: 45 | 90;
```

Add `"xLabelRotation"` to the `KEYS` array in `cartesian-codec.ts` so it round-trips through saved settings. **The array is `as const satisfies readonly (keyof CartesianConfig)[]`, so a missing key is not a type error — it is a setting that silently fails to save.** Verify by saving a chart with a rotation and reloading it.

- [ ] **Step 2: Replace the toggle with the select**

`CartesianDisplaySection.tsx:229-230` currently renders:

```tsx
<Toggle id="cz-xticks" label={`${catAx} axis lines and ticks`} checked={config.xAxisTicks !== false}
  onChange={(v) => onChange({ xAxisTicks: v ? undefined : false })} />
```

Replace it with the same four-value select `ChartAxesSection.tsx:95-118` already renders (Hide / Show / Rotate 45° / Rotate 90°), projecting onto the two fields exactly as that component does. Read it and copy its shape rather than inventing a second projection — two controls storing the same pair differently is the failure this is meant to avoid.

Keep the `${catAx}` prefix: on a row chart the category axis is the vertical one, and this section already relabels by the axis the reader sees (see its doc comment at `:120-124`).

- [ ] **Step 3: Test the projection**

Add to the existing settings-panel test file for this section (find it with `ls frontend/src/components/chart/settings/__tests__/`): choosing "Rotate 45°" writes `{ xLabelRotation: 45, xAxisTicks: undefined }`, choosing "Hide" writes `{ xAxisTicks: false }`, and choosing "Show" clears the rotation. Assert the emitted patch, not the DOM.

- [ ] **Step 4: Run, typecheck, commit**

```bash
cd frontend && npx vitest run src/components/chart && npx tsc --noEmit
```

```bash
git add frontend/src/api/charts.ts \
        frontend/src/components/chart/viz/cartesian-codec.ts \
        frontend/src/components/chart/settings/CartesianDisplaySection.tsx \
        frontend/src/components/chart/settings/__tests__/
git commit -m "feat(charts): let the x-axis rotation be chosen, not only measured"
```

---

## Task 4: Verify against Metabase and open the PR

- [ ] **Step 1: Local gates**

```bash
cd frontend && npx tsc --noEmit && npm run lint && npx vitest run
```

All three must pass. GitHub CI is out of quota for this project; local is the gate.

- [ ] **Step 2: Live comparison — this is the point of the PR**

Unlike PR 01, this one **does** change rendering, so the harness comparison is the acceptance test rather than a regression check.

Bring up a build containing these changes and open the **ZZ Chart Parity** dashboard
(`/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`), beside Metabase's
(`localhost:3033/dashboard/2`). Both hold the same 19 chart types on byte-identical numbers.

Confirm on the egret side:
- **bar** draws all four category names, rotated if the card is narrow — not one of four
- **row** is unaffected (its categories run down the left; rotation does not apply)
- **box plot** draws all four category names
- **waterfall** draws its month labels
- widening the browser returns the labels to upright

Then compare each against Metabase's equivalent card and record any remaining difference.

If the dev server cannot be reached from the browser — it could not during PR 01 — say so and leave the check outstanding in the PR body rather than claiming it. Do not describe screenshots as an automated test.

- [ ] **Step 3: Open the PR and stop**

```bash
git push -u origin <branch>
gh pr create --title "feat(charts): draw every x-axis label, rotate it, or draw none" --body "…"
```

The body must state what changed visually, that PR 01's outstanding live check is now subsumed by this one's, and any comparison that could not be run.

**Then stop and wait for review. Do not merge.**

---

## Self-review against the spec

**Spec coverage.** Workstream 2 lists six behaviours; Task 1 implements 2–6 (dimension width, adjacent-pair overlap, upright, the rotate ladder with the height guard, the hide fallback, sampled measurement) and Task 2 implements 1 (auto only while the setting is untouched) via `cfg.xLabelRotation ?? …`. The "never partial" property is Task 2 step 4 and is the first assertion in `cartesianXLabels.test.ts`. The panel control is Task 3, which the spec puts in workstream 6 but which cannot be separated from the setting that suppresses the measurement.

**Placeholders.** None. Task 4 step 3's PR body is left to the author because it must describe what was actually observed.

**Type consistency.** `XLabelFit`, `fitXLabels`, `TextMeasurer`, `EChartsSize` and `CHART_THEME.grid` / `.axisFontSize` are all named as their modules export them. The parameter renamed from `_size` to `size` in Task 2 is cartesian's only; gauge and sankey keep `_size`, and Task 2 step 4b says so.

**Four errors this review caught and fixed inline.** Writing the call against the real file rather than from memory turned up:

1. `cfg.yMin` / `cfg.yMax` are the user's **manual axis bounds**, not the data extent. Using them for the value-axis gutter would have measured `undefined` on every chart that does not set them. The extent is `widestOf(metrics)` at `:862`, which the compaction decision two lines below already computes.
2. `displayCats` is the **caller's** name for the labels; inside `baseOption` the parameter is `categories`.
3. `leftTitle` is computed at `:987`, *after* the point the fit is needed. `yTitle` (`:861`) is the same thing on this path, which is already guarded on `!horizontal`.
4. Most importantly: the `rotate` computation is inside **`baseOption`**, whose signature has no size, and which is called from **three** places. That became step 4a. Without it the plan would have read as though `size` were simply in scope, and an implementer following it would have had to invent the threading — or, worse, wired one call site and left the others silently unmeasured.

**Remaining risk, stated rather than resolved.** The plot-width estimate cannot be exact while `containLabel: true` decides the real box. Task 4 step 2's live comparison against Metabase is what establishes whether the estimate is close enough in practice; if bar labels rotate on a card where Metabase leaves them upright, the gutter estimate is too pessimistic and that is the thing to tune — not the ported thresholds.
