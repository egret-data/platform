# Chart parity 01 — size-aware option builders — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the cartesian, gauge and sankey option builders see the pixel size of the container they are drawn into, so later PRs can make label decisions from width instead of guessing from label count.

**Architecture:** `EChartsRenderer` already accepts `option` as either a plain object or a function of the measured `EChartsSize`, and already keeps that size current through a ResizeObserver with rAF coalescing. Only pie uses the function form today. This PR adds an optional `size` parameter to three more builders and switches `ChartRenderer` to pass size-functions for them, each behind a memo boundary so the function identity is stable across unrelated re-renders. **No rendering behaviour changes in this PR** — the builders receive the size and ignore it. That keeps the diff reviewable and gives PRs 02, 03 and 05 something to build on.

**Funnel is deliberately not in this PR.** The spec lists it alongside the other three, which is
wrong: there is no `buildFunnelOption`. The funnel is a React component
(`frontend/src/components/chart/FunnelChart.tsx`) drawing Metabase's trapezoid layout from
`funnelSteps()` and `stepOpacity()` — it never goes through `EChartsRenderer`, so there is no option
function to make size-dependent. It must measure its own container with a ref, which is a different
mechanism and belongs with its label-truncation fix in PR 05. The one funnel path that *is* ECharts
is `StepBars`, which renders `funnel.type: "bar"` through `buildCartesianOption` and is therefore
covered by the cartesian change here.

**Tech Stack:** TypeScript, React 18, ECharts (SVG renderer), vitest + @testing-library/react, Tailwind.

**Spec:** `docs/en/specs/2026-08-01-chart-parity-design.md` (workstream 1).

---

## Background an engineer needs before starting

Read these three things first. They are short and they explain why the tasks are shaped this way.

1. **`frontend/src/components/chart/echarts/EChartsRenderer.tsx`** — the type
   `EChartsOptionInput = EChartsCoreOption | ((size: EChartsSize) => EChartsCoreOption)` and the
   `resolveOption` helper. The renderer needs **no changes** in this PR. Its ResizeObserver already
   skips ticks where the rounded size did not change, and coalesces the rest to one rebuild per
   animation frame.

2. **`frontend/src/components/chart/ChartRenderer.tsx`, the `PieChart` component** (around line 46).
   Its doc comment explains why it exists: an inline arrow function passed as `option` is a *new
   function on every render of whatever renders the chart* — a hover, a filter chip, a sibling's
   state change — and `EChartsRenderer`'s `[option]` effect re-applies the whole option whenever that
   identity changes. For pie that means re-running a `measureText` pass per slice. Every builder we
   convert here inherits that trap, so every one gets a memo boundary.

3. **`frontend/src/components/chart/measure-text.ts`** — canvas measurement ported from Metabase, with
   an injectable `measurer`. Under jsdom there is no 2D context, so it deliberately returns
   `{width: 0, height: 0}`. Tests must never assert a pixel outcome; they assert the *decision* a
   builder reached, injecting a stub where a measurement is involved. This PR does not measure
   anything yet, but the constraint governs how its tests are written.

### The one thing that must be untangled first

`ChartRenderer` currently reads two fields back **off the built cartesian option**:

```ts
const foldedCategory = (cartesianOption as { __foldedCategory?: string }).__foldedCategory;
const rawSeriesAt = (i) => (cartesianOption as { series?: { __raw?: string }[] }).series?.[i]?.__raw;
```

Both feed the click handler. If `cartesianOption` becomes a *function*, these reads break — you
cannot index into a function. Both values derive from data and config only, never from size, so
Task 1 extracts them into a small function that `ChartRenderer` can call directly. Do this before
touching any signatures.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `frontend/src/components/chart/echarts/options/cartesianMeta.ts` | Derive the click-handler metadata (folded category name, per-series raw values) from data + config, independently of the built option | **Create** |
| `frontend/src/components/chart/echarts/options/cartesian.ts` | Cartesian option builder | Modify — accept optional `size`, export the meta helper's inputs |
| `frontend/src/components/chart/echarts/options/gauge.ts` | Gauge option builder | Modify — accept optional `size` |
| `frontend/src/components/chart/echarts/options/sankey.ts` | Sankey option builder | Modify — accept optional `size` |
| `frontend/src/components/chart/ChartRenderer.tsx` | Wire size-functions + memo boundaries | Modify |
| `frontend/src/components/chart/echarts/__tests__/sizeAware.test.tsx` | Prove each builder receives a size and that the memo boundary holds | **Create** |
| `frontend/src/components/chart/echarts/__tests__/cartesianMeta.test.ts` | Prove the extracted metadata matches what the option used to carry | **Create** |

---

## Task 1: Extract the cartesian click metadata

**Files:**
- Create: `frontend/src/components/chart/echarts/options/cartesianMeta.ts`
- Test: `frontend/src/components/chart/echarts/__tests__/cartesianMeta.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/echarts/__tests__/cartesianMeta.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";
import { cartesianMeta } from "../options/cartesianMeta";

const columns = [
  { name: "category", type: "varchar" },
  { name: "region", type: "varchar" },
  { name: "revenue", type: "double" },
];
const rows: unknown[][] = [
  ["Widget", "North", 10],
  ["Gadget", "South", 20],
  ["Gizmo", "East", 30],
  ["Doohickey", "West", 40],
];
const cfg = {
  kind: "bar" as const,
  xAxis: "category",
  yMetrics: ["revenue"],
  series: "region",
  maxCategories: 2,
};

describe("cartesianMeta", () => {
  /**
   * The metadata used to be read back off the built option (`__foldedCategory`, `series[i].__raw`).
   * That only worked while the option was a value; it becomes a function of the container size in
   * this PR. These assertions pin the extracted helper to the option's own answer, so the two
   * cannot drift apart while both exist.
   */
  it("reports the same folded category the option carries", () => {
    const option = buildCartesianOption(columns, rows, cfg) as { __foldedCategory?: string };
    const meta = cartesianMeta(columns, rows, cfg);
    expect(meta.foldedCategory).toBe(option.__foldedCategory);
  });

  it("reports the same raw series value at each index the option carries", () => {
    const option = buildCartesianOption(columns, rows, cfg) as { series?: { __raw?: string }[] };
    const meta = cartesianMeta(columns, rows, cfg);
    const built = option.series ?? [];
    // A helper that returned undefined for everything would pass an equality-only check, so the
    // series list must be non-trivial before the comparison means anything.
    expect(built.length).toBeGreaterThan(1);
    built.forEach((s, i) => expect(meta.rawSeriesAt(i)).toBe(s.__raw));
  });

  it("returns undefined past the end of the series list rather than throwing", () => {
    const meta = cartesianMeta(columns, rows, cfg);
    expect(meta.rawSeriesAt(999)).toBeUndefined();
    expect(meta.rawSeriesAt(undefined)).toBeUndefined();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/cartesianMeta.test.ts`

Expected: FAIL — `Failed to resolve import "../options/cartesianMeta"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/echarts/options/cartesianMeta.ts`:

```ts
import type { ResultColumn } from "@/components/chart/ChartTable";
import { buildCartesianOption, type CartesianBuildConfig } from "./cartesian";

/**
 * The two things `ChartRenderer`'s click handler needs to know about a cartesian chart:
 * which category (if any) got folded into "Other", and what real column value sits behind
 * each series' display name.
 *
 * Both used to be read straight off the built option. That stopped being possible once the
 * option became a FUNCTION of the measured container size — you cannot index into a function.
 * Neither value depends on size, so both are derived here instead.
 *
 * The option is still what computes them; this is a narrow read of it at a nominal size rather
 * than a second implementation of the folding rules, which would be a copy free to drift.
 */
export interface CartesianMeta {
  /** The category folded into "Other" by `maxCategories`, or undefined when none was. */
  foldedCategory: string | undefined;
  /** The series' real column value at a series index, behind whatever label the legend shows. */
  rawSeriesAt: (i: number | undefined) => string | undefined;
}

export function cartesianMeta(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: CartesianBuildConfig,
): CartesianMeta {
  const option = buildCartesianOption(columns, rows, cfg) as {
    __foldedCategory?: string;
    series?: { __raw?: string }[];
  };
  const series = option.series ?? [];
  return {
    foldedCategory: option.__foldedCategory,
    rawSeriesAt: (i) => (i == null ? undefined : series[i]?.__raw),
  };
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/cartesianMeta.test.ts`

Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/echarts/options/cartesianMeta.ts \
        frontend/src/components/chart/echarts/__tests__/cartesianMeta.test.ts
git commit -m "refactor(charts): derive cartesian click metadata without reading the built option"
```

---

## Task 2: Give the three builders an optional size parameter

**Files:**
- Modify: `frontend/src/components/chart/echarts/options/cartesian.ts:560-564` (`buildCartesianOption`, config type `CartesianBuildConfig`)
- Modify: `frontend/src/components/chart/echarts/options/gauge.ts:83` (`buildGaugeOption`, config type `GaugeBuildConfig`)
- Modify: `frontend/src/components/chart/echarts/options/sankey.ts:119` (`buildSankeyOption`, config type `SankeyBuildConfig`)
- Test: `frontend/src/components/chart/echarts/__tests__/sizeAware.test.tsx`

The parameter is added and **deliberately unused** in this PR. That is not dead code: it is the seam
PRs 02, 03 and 05 attach to, and adding it separately keeps those PRs' diffs about behaviour rather
than about signatures.

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/echarts/__tests__/sizeAware.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import type { EChartsSize } from "../EChartsRenderer";
import { buildCartesianOption } from "../options/cartesian";
import { buildGaugeOption } from "../options/gauge";
import { buildSankeyOption } from "../options/sankey";

const SMALL: EChartsSize = { width: 300, height: 200 };

const cols = [{ name: "k", type: "varchar" }, { name: "v", type: "double" }];
const rows: unknown[][] = [["Widget", 10], ["Gadget", 20]];

describe("builders accept a container size", () => {
  /**
   * The size is not read yet — this PR only opens the seam. What is worth asserting now is that
   * passing one is TYPE-CORRECT and does not change the option, so the later PRs that do read it
   * start from a known-inert baseline rather than from an accidental behaviour change.
   */
  it("cartesian builds identically with and without a size", () => {
    const cfg = { kind: "bar" as const, xAxis: "k", yMetrics: ["v"] };
    expect(buildCartesianOption(cols, rows, cfg, SMALL))
      .toEqual(buildCartesianOption(cols, rows, cfg));
  });

  it("gauge builds identically with and without a size", () => {
    const cfg = { metric: "v", min: 0, max: 100 };
    expect(buildGaugeOption(cols, rows, cfg, SMALL))
      .toEqual(buildGaugeOption(cols, rows, cfg));
  });

  it("sankey builds identically with and without a size", () => {
    const sCols = [{ name: "s", type: "varchar" }, { name: "t", type: "varchar" }, { name: "v", type: "double" }];
    const sRows: unknown[][] = [["A", "X", 1], ["B", "Y", 2]];
    const cfg = { source: "s", target: "t", metric: "v" };
    expect(buildSankeyOption(sCols, sRows, cfg, SMALL))
      .toEqual(buildSankeyOption(sCols, sRows, cfg));
  });

});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/sizeAware.test.tsx`

Expected: FAIL — TypeScript reports "Expected 3 arguments, but got 4" for each builder.

- [ ] **Step 3: Add the parameter to each builder**

In `frontend/src/components/chart/echarts/options/cartesian.ts`, change the signature at line 560:

```ts
export function buildCartesianOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg0: CartesianBuildConfig,
  /** The chart's measured pixel size, when the caller has one. Unread today; this is the seam the
   * axis-label work attaches to, because whether labels fit is a question about WIDTH and this
   * builder has never been able to see it. */
  _size?: EChartsSize,
): EChartsCoreOption {
```

Add the import at the top of the same file:

```ts
import type { EChartsSize } from "@/components/chart/echarts/EChartsRenderer";
```

Apply the identical change — same parameter name `_size`, same doc comment, same import — to
`buildGaugeOption` (`gauge.ts:83`) and `buildSankeyOption` (`sankey.ts:119`). Each already takes
`(columns, rows, cfg)`; the size goes fourth in every case, so the position is uniform across the
three and matches pie's convention of size-last.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/sizeAware.test.tsx`

Expected: PASS, 4 tests.

- [ ] **Step 5: Run the whole chart suite to confirm nothing regressed**

Run: `cd frontend && npx vitest run src/components/chart`

Expected: PASS. An added optional parameter cannot change any existing call, so a failure here means
something else is wrong — stop and investigate rather than adjusting the test.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/echarts/options/cartesian.ts \
        frontend/src/components/chart/echarts/options/gauge.ts \
        frontend/src/components/chart/echarts/options/sankey.ts \
        frontend/src/components/chart/echarts/__tests__/sizeAware.test.tsx
git commit -m "feat(charts): open a size seam on the cartesian, gauge and sankey builders"
```

---

## Task 3: Switch ChartRenderer's cartesian branch to a size-function

**Files:**
- Modify: `frontend/src/components/chart/ChartRenderer.tsx:332-358`

- [ ] **Step 1: Add the memo boundary component**

Directly below the existing `PieChart` component in `ChartRenderer.tsx`, add:

```tsx
/**
 * The cartesian branch, as its own component for the same reason `PieChart` is one: the `option`
 * becomes a FUNCTION of the measured size, and `EChartsRenderer`'s `[option]` effect re-applies
 * the whole option whenever that function's identity changes. Built inline, the arrow would be
 * fresh on every render of whatever renders the chart — a hover, a filter chip, a sibling's state
 * change — and each one would re-run the builder and re-apply the option.
 *
 * Hooks cannot live in `ChartRenderer` itself past its early returns, so this is a component
 * rather than a memo.
 */
function CartesianChart({ columns, rows, cfg, children }: {
  columns: ResultColumn[];
  rows: unknown[][];
  cfg: CartesianBuildConfig;
  children: (meta: CartesianMeta, option: EChartsOptionInput) => React.ReactNode;
}) {
  const option = useMemo(
    () => (size: EChartsSize) => buildCartesianOption(columns, rows, cfg, size),
    [columns, rows, cfg],
  );
  const meta = useMemo(() => cartesianMeta(columns, rows, cfg), [columns, rows, cfg]);
  return <>{children(meta, option)}</>;
}
```

Add the imports it needs:

```ts
import { useMemo } from "react";
import { cartesianMeta, type CartesianMeta } from "@/components/chart/echarts/options/cartesianMeta";
import { type EChartsOptionInput } from "@/components/chart/echarts/EChartsRenderer";
import { type CartesianBuildConfig } from "@/components/chart/echarts/options/cartesian";
```

- [ ] **Step 2: Rewrite the cartesian branch to use it**

Replace the body of the `if (isCartesianViz(vizType))` block (currently lines 332-358, ending at the
`<EChartsRenderer option={cartesianOption}` line) so that the config is built into a variable, the
metadata comes from `meta` rather than from the option, and the renderer receives the function:

```tsx
  if (isCartesianViz(vizType)) {
    const seriesUnit = cartesian.series ? params.spec.temporal_units?.[cartesian.series] : undefined;
    const cartesianCfg: CartesianBuildConfig = {
      // Spread rather than list the fields: this used to be a hand-maintained whitelist, and
      // four settings added in one branch were silently dropped between the panel and the
      // chart because nobody remembered to extend it.
      ...cartesian,
      kind: vizType,
      columnSettings,
      metricTypes,
      xAxisUnit: params.spec.temporal_units?.[cartesian.xAxis ?? ""],
      xAxisType: columns.find((c) => c.name === cartesian.xAxis)?.type,
      seriesUnit,
      seriesType: columns.find((c) => c.name === cartesian.series)?.type,
    };
    return (
      <div className="h-full w-full">
        {rows.length === 0 ? <EmptyState /> : (
          <CartesianChart columns={columns} rows={rows} cfg={cartesianCfg}>
            {(meta, option) => (
              <EChartsRenderer option={option}
                onPointClick={/* unchanged — see step 3 */ undefined}
              />
            )}
          </CartesianChart>
        )}
      </div>
    );
  }
```

- [ ] **Step 3: Move the existing click handler across unchanged**

The `onPointClick` handler currently attached to `<EChartsRenderer option={cartesianOption}` is long
and its logic is not changing. Move it verbatim into the `children` callback above, replacing the
`undefined` placeholder from step 2, and change exactly two references:

- `foldedCategory` → `meta.foldedCategory`
- `rawSeriesAt(...)` → `meta.rawSeriesAt(...)`

Nothing else in the handler changes. Do not reformat it — a diff that reformats the handler hides
whether those two substitutions were the only edits.

- [ ] **Step 4: Typecheck**

Run: `cd frontend && npx tsc --noEmit`

Expected: no errors. If `cartesianCfg` is reported as not assignable to `CartesianBuildConfig`, the
spread is picking up a field the type does not declare — fix the type, not the spread, because the
spread is deliberate (see its comment).

- [ ] **Step 5: Run the chart suite**

Run: `cd frontend && npx vitest run src/components/chart`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/ChartRenderer.tsx
git commit -m "feat(charts): build the cartesian option against its measured size"
```

---

## Task 4: Switch the gauge and sankey branches

**Files:**
- Modify: `frontend/src/components/chart/ChartRenderer.tsx` (gauge at ~line 457, sankey at ~line 477)

- [ ] **Step 1: Add one memo-boundary component per viz**

Neither has metadata read back off the option, so each is simpler than `CartesianChart`.
Add both below it:

```tsx
/** Memo boundary for the gauge's size-function option. See `CartesianChart` for why this is a
 * component rather than an inline arrow. */
function GaugeChart({ columns, rows, cfg }: {
  columns: ResultColumn[]; rows: unknown[][]; cfg: GaugeBuildConfig;
}) {
  const option = useMemo(
    () => (size: EChartsSize) => buildGaugeOption(columns, rows, cfg, size),
    [columns, rows, cfg],
  );
  return <EChartsRenderer option={option} />;
}
```

Write `SankeyChart` the same way, substituting `buildSankeyOption` and `SankeyBuildConfig`. Give it
the same one-line doc comment pointing at `CartesianChart` — do not leave it undocumented on the
grounds that the first one explains it, because the reason is easy to undo and hard to rediscover.

- [ ] **Step 2: Point each branch at its component**

Replace `<EChartsRenderer option={buildGaugeOption(columns, rows, {...})} />` with
`<GaugeChart columns={columns} rows={rows} cfg={gaugeCfg} />`, hoisting the inline config object
into a `const gaugeCfg` above the `return`. Do the same for sankey.

> **Corrected during Task 3.** An earlier version of this plan said to key the memo on
> `[columns, rows, cfg]` and claimed that hoisting the config to a `const` made the reference
> stable. **It does not.** `ChartRenderer` rebuilds its config from scratch on every render — for
> cartesian, `const cartesian = { yMetrics: [], ...cfg }` at `ChartRenderer.tsx:311` — so the object
> is fresh whether it is inline or hoisted, and `[cfg]` memoises nothing. The component would have
> been pure overhead while its own doc comment claimed to prevent re-applies. Task 3 verified this
> with a probe test before fixing it.
>
> Key on a **value**, the way `PieChart` already does in this same file
> (`const settingsKey = JSON.stringify([pie, columnSettings, metricTypes, dimensionUnits])`,
> `ChartRenderer.tsx:80`):
>
> ```ts
> const cfgKey = JSON.stringify(cfg);
> const option = useMemo(
>   () => (size: EChartsSize) => buildGaugeOption(columns, rows, cfg, size),
>   [columns, rows, cfgKey],
> );
> ```
>
> Do not assume the memo works because it compiles. Verify it: render the component inside a parent
> with unrelated state, bump that state, and assert the option's identity is unchanged — then assert
> the negative direction too (a config change DOES produce a new identity), so the test cannot pass
> by memoising everything forever.

- [ ] **Step 3: Typecheck and run the suite**

Run: `cd frontend && npx tsc --noEmit && npx vitest run src/components/chart`

Expected: no type errors, tests PASS.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/components/chart/ChartRenderer.tsx
git commit -m "feat(charts): build the gauge and sankey options against their measured size"
```

---

## Task 5: Prove the memo boundary actually holds

**Files:**
- Modify: `frontend/src/components/chart/echarts/__tests__/sizeAware.test.tsx`

A memo boundary that silently stopped working would cost performance with nothing visibly wrong —
exactly the failure mode the pie comment describes. This test fails if someone later inlines the
arrow.

- [ ] **Step 1: Write the failing test**

Append to `sizeAware.test.tsx`:

```tsx
import { render } from "@testing-library/react";
import { useState } from "react";
import { ChartRenderer } from "@/components/chart/ChartRenderer";

describe("cartesian option identity", () => {
  /**
   * A parent re-render that changes nothing the chart depends on must NOT produce a new option
   * function. If it does, EChartsRenderer re-applies the whole option — the cost the PieChart
   * boundary was introduced to avoid, now inherited by every converted builder.
   */
  it("does not rebuild the option when an unrelated parent state changes", () => {
    const builds: number[] = [];
    function Harness() {
      const [n, setN] = useState(0);
      builds.push(n);
      return (
        <div>
          <button onClick={() => setN((v) => v + 1)}>bump</button>
          <ChartRenderer
            params={{ version: 2, spec: {
              dimensions: ["k"], metrics: ["v"], columns: [], filters: [],
              orderby: [], row_limit: 1000, rollup_groups: [], temporal_units: {},
            }, settings: { "cartesian.xAxis": "k", "cartesian.yMetrics": ["v"] } }}
            vizType="bar"
            columns={cols}
            rows={rows}
          />
        </div>
      );
    }
    const { getByText, container } = render(<Harness />);
    const before = container.querySelector("[data-echarts-root]")?.getAttribute("data-option-id");
    getByText("bump").click();
    const after = container.querySelector("[data-echarts-root]")?.getAttribute("data-option-id");
    expect(builds.length).toBeGreaterThan(1); // the parent really did re-render
    expect(after).toBe(before);
  });
});
```

- [ ] **Step 2: Run it to see how it fails**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/sizeAware.test.tsx -t "unrelated parent"`

Expected: FAIL — `data-option-id` does not exist yet, so both reads are `null` and
`expect(after).toBe(before)` passes vacuously while `builds.length` may also mislead. **Treat a pass
here as a failure of the test, not a success of the code**, and continue to step 3, which gives the
attribute something real to report.

- [ ] **Step 3: Emit the identity marker from EChartsRenderer**

In `EChartsRenderer.tsx`, on the root `<div ref={elRef}>`, add a stable id derived from the option's
identity:

```tsx
const optionIdRef = useRef(0);
const lastOptionRef = useRef<EChartsOptionInput | null>(null);
if (lastOptionRef.current !== option) {
  lastOptionRef.current = option;
  optionIdRef.current += 1;
}
```

and render `data-echarts-root="" data-option-id={optionIdRef.current}` on that div.

This is a test seam and should say so in a comment: it counts how many *distinct* option identities
this renderer has seen, which is exactly the number of full re-applies it has been asked to do.

- [ ] **Step 4: Run the test to verify it now passes meaningfully**

Run: `cd frontend && npx vitest run src/components/chart/echarts/__tests__/sizeAware.test.tsx -t "unrelated parent"`

Expected: PASS, with `before` and `after` both being a real number string such as `"1"`.

- [ ] **Step 5: Confirm the test can fail**

Temporarily replace `<CartesianChart …>` usage in `ChartRenderer.tsx` with an inline
`option={(size) => buildCartesianOption(columns, rows, cartesianCfg, size)}` and re-run the test.

Expected: FAIL — `after` is `"2"` where `before` was `"1"`. Restore the memo boundary and confirm it
passes again. **Do not skip this step.** A test that has never been seen to fail has not been shown
to test anything, and four assertions in this programme have already survived their own mutation.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/echarts/EChartsRenderer.tsx \
        frontend/src/components/chart/echarts/__tests__/sizeAware.test.tsx
git commit -m "test(charts): pin the memo boundary that keeps size-built options from re-applying"
```

---

## Task 6: Full local verification and PR

Per `docs/en/plans/2026-07-30-chart-redesign-metabase-parity-rules.md` and standing project practice,
GitHub CI is out of quota — everything runs locally before the PR opens.

- [ ] **Step 1: Frontend gates**

Run: `cd frontend && npx tsc --noEmit && npm run lint && npx vitest run`

Expected: all pass.

- [ ] **Step 2: Live check on the parity harness**

Open the **ZZ Chart Parity** dashboard at
`http://localhost:30080/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`
and confirm all 19 cards still render exactly as they did before this PR. This PR changes no
rendering, so **any** visual difference is a regression, not an improvement.

Resize the browser and confirm charts still redraw smoothly — the ResizeObserver now rebuilds four
more option types per frame, and a visible stutter while dragging means a memo boundary was missed.

Record this as a manual check. It is not an automated test and must not be described as one.

- [ ] **Step 3: Open the PR and stop**

```bash
git push -u origin <branch>
gh pr create --title "feat(charts): build options against the measured container size" --body "$(cat <<'EOF'
## What

Opens a `size` seam on the cartesian, gauge and sankey option builders and switches
`ChartRenderer` to pass size-functions for them, each behind a memo boundary.

The spec lists funnel alongside these; it does not belong. There is no `buildFunnelOption` — the
funnel is a React trapezoid component that never touches `EChartsRenderer`, so it measures its own
container instead, in PR 05 with its label fix.

**No rendering changes.** The builders receive the size and ignore it. This is the foundation for
PRs 02, 03 and 05, which make label decisions from width instead of guessing from label count —
see `docs/en/specs/2026-08-01-chart-parity-design.md`.

## Why it is shaped this way

`EChartsRenderer` already accepted a function-valued option and already coalesced resize rebuilds;
only pie used it. The one untangling needed first was `ChartRenderer` reading `__foldedCategory` and
`series[i].__raw` back off the built cartesian option — impossible once the option is a function —
so that metadata moved into `cartesianMeta`.

## Verification

- `tsc --noEmit`, `npm run lint`, `vitest run` all pass locally
- Manually checked all 19 cards on the ZZ Chart Parity dashboard: unchanged, as intended
- The memo-boundary test was confirmed to fail when the boundary is removed

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Then **stop and wait for review**. Do not merge.

---

## Self-review against the spec

**Spec coverage.** Workstream 1 asks for three things: generalise the `PieChart` pattern to
cartesian/gauge/sankey/funnel (Tasks 3 and 4 — **minus funnel, which the spec gets wrong**; see the
Architecture note and the correction filed against the spec), preserve the per-viz memo boundary
(Tasks 3, 4 and 5), and change no rendering behaviour (asserted in Task 2 step 1, checked in Task 6
step 2). The spec's
testing note — inject a stub measurer, assert the decision not the pixel — does not bind this PR
because nothing is measured yet; it binds PRs 02, 03 and 05 and is restated in this plan's
background section so the next author meets it before writing those.

**Placeholders.** None. Task 3 step 2 contains a deliberate `undefined` placeholder that step 3
replaces, and the step says so explicitly rather than leaving the reader to guess.

**Type consistency.** All type names used here were checked against the modules that export them:
`CartesianBuildConfig` (`cartesian.ts:12`), `GaugeBuildConfig` (`gauge.ts:10`), `SankeyBuildConfig`
(`sankey.ts:14`), `EChartsSize` and `EChartsOptionInput` (`EChartsRenderer.tsx:56,64`).
`CartesianMeta` and `cartesianMeta` are defined in Task 1 and used with those exact names in Task 3.
The size parameter is named `_size` and sits fourth in all three builders.

**A spec error this review caught.** The spec names funnel as a fourth builder to convert. Checking
the module showed `funnel.ts` exports `funnelSteps` and `stepOpacity` and **no option builder at
all** — the funnel has been a React trapezoid component since PR #378. The spec needs a correction
noting that funnel measures its own container in PR 05 instead.
