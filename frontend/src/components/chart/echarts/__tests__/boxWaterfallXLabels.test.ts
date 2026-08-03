import { describe, it, expect, vi, afterEach } from "vitest";
import type { EChartsSize } from "../EChartsRenderer";
import { buildBoxplotOption } from "../options/boxplot";
import { buildWaterfallOption, type WaterfallBuildConfig } from "../options/waterfall";
import type { fitXLabels as FitXLabels, estimatePlotWidth as EstimatePlotWidth, XLabelFit } from "../axisLabelFit";
import { boundarySteps } from "@/components/chart/temporal-period";

/**
 * The wiring between `fitXLabels` and the two OTHER builders that draw a bottom category axis —
 * the box plot and the waterfall. `cartesianXLabels.test.ts` is the same file for the cartesian
 * family; this one is table-driven over the two, because the argument is identical for each and
 * only the fixture differs.
 *
 * Both carried, verbatim, the heuristic the cartesian builder was written to delete —
 * `longest > 10 && categories.length > 6 ? 30 : 0` beside `interval: "auto"` — so both answered
 * a WIDTH question from label count and character length, and then let ECharts silently drop
 * whatever collided. Neither took a size at all until this change.
 *
 * WHAT THIS LAYER CAN AND CANNOT ASSERT. `measure-text.ts` returns `{width: 0, height: 0}` under
 * jsdom by design ("Size claims belong in a browser"), and these builders use the REAL measurer —
 * there is no seam to inject a stub through, and adding one would be a production parameter that
 * exists only for a test. So every label here measures 0px wide, and the ladder can only be driven
 * through the rungs that depend on `dimensionWidth = plotWidth / labelCount` rather than on a
 * label's own width:
 *
 *   - no overlap (`HORIZONTAL_TICKS_GAP` of 6 fits) -> draw upright;
 *   - overlap, and `dimensionWidth` under `fontSize * 1.2` -> hide the axis.
 *
 * `rotate: 45` and `rotate: 90` sit BETWEEN those two, and reaching them needs `dimensionWidth` to
 * be at once under 6 (to collide with zero-width labels) and at least 15.6 (to clear the 90° rung)
 * — impossible. So no test in this file asserts a MEASURED rotation; that is
 * `axisLabelFit.test.ts`'s job, where the measurer is injected, and the live
 * comparison's. What is asserted here is the wiring: that a size reaches the fitter, that the
 * decision reaches the option, and that an explicit setting stops the measurement entirely.
 *
 * `outerHeight` HAS NO OBSERVABLE EFFECT FROM HERE and is therefore checked on the way IN, by the
 * pass-through spy below: the 70% height guard divides the widest label by it, the widest label is
 * 0px, and 0/anything clears the guard — so passing a grid-adjusted height instead of the
 * container's would silently tighten the guard in a browser and change nothing in this file.
 *
 * The value-axis gutter is pinned by its INPUT and not by its width. Which value string a builder
 * hands `estimatePlotWidth` is a decision, and the block at the foot of this file asserts it. The
 * PIXELS that string turns into are a `measureText` call, so they contribute exactly 0 under jsdom
 * whatever the numbers on the axis, and deleting the whole `gutter` term from `estimatePlotWidth`
 * leaves every test in the repo green. That half is a browser question and stays one — the live
 * comparison against Metabase is where it is answered.
 */

type FitArgs = Parameters<typeof FitXLabels>[0];
type EstimateArgs = Parameters<typeof EstimatePlotWidth>;

/** Every argument a builder hands the fitter and the plot-width estimate, in call order, and an
 *  optional CANNED decision to hand back in place of the real ladder's. `vi.hoisted` because the
 *  `vi.mock` factory below is hoisted above this file's imports and would otherwise touch these
 *  bindings in their temporal dead zone. */
const { fitCalls, gutterCalls, fitResult } = vi.hoisted(() => ({
  fitCalls: [] as FitArgs[],
  /** The VALUE the gutter was measured from and the FORMATTER it was measured through — the two
   *  halves of the decision, since PR 03 stopped this taking a pre-formatted string. Both are
   *  asserted at the foot of this file: the value because it is what the builder chose, and the
   *  formatter because "through the same formatter the axis uses" is the invariant beside it. */
  gutterCalls: [] as { widest: number; format: (v: number) => string }[],
  fitResult: { value: undefined as XLabelFit | undefined },
}));

/** Pass-through spies, not stubs, WHILE `fitResult` IS UNSET — which it is for every test above the
 *  temporal block at the foot of this file, so the real ladder and the real estimate still decide
 *  there and those assertions remain assertions about the real module. All the spies add is a view
 *  of what went IN; see the header on `outerHeight`, and the gutter block.
 *
 *  `fitResult` is the one exception and it exists for a reason the header states: no `plotWidth`
 *  reachable from jsdom can make a 0px label collide, so `kind: "thin"` cannot be produced by the
 *  real fitter here. The LADDER is driven with a stub measurer in `axisLabelFit.test.ts`; what a
 *  canned `thin` buys is the BUILDER's half — what that decision turns into on the axis. */
vi.mock("../axisLabelFit", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../axisLabelFit")>();
  return {
    ...actual,
    fitXLabels: (args: FitArgs) => {
      fitCalls.push(args);
      return fitResult.value ?? actual.fitXLabels(args);
    },
    estimatePlotWidth: (...args: EstimateArgs) => {
      gutterCalls.push({ widest: args[1], format: args[2] });
      return actual.estimatePlotWidth(...args);
    },
  };
});

afterEach(() => {
  fitResult.value = undefined;
});

interface XAxisShape {
  axisLabel: { interval: unknown; rotate?: number; show?: boolean; showMinLabel?: boolean; formatter?: (v: unknown) => string };
}
const xAxisOf = (o: unknown) => (o as { xAxis: XAxisShape }).xAxis;
const gridBottomOf = (o: unknown) => (o as { grid: { bottom: number } }).grid.bottom;

/** One builder under test: how to call it, and the two fixtures the assertions share. */
interface Chart {
  what: string;
  /** `rotation` is the user's explicit choice, absent when the axis is left to measure itself. */
  build: (rows: unknown[][], size?: EChartsSize, rotation?: 45 | 90) => unknown;
  /** The four categories from the live defect — comfortably clear of the hide threshold at every
   *  width used below, which is the point: these exercise the DRAWN rungs. */
  rows: unknown[][];
  /** What the axis actually draws for `rows`, and so what the fitter must be handed. */
  labels: string[];
  /** 46 categories — see `CROWDED` below for why that exact number. */
  crowdedRows: unknown[][];
}

/**
 * The crowded fixture's size, and the arithmetic it turns on.
 *
 * With every label 0px wide the overlap test reduces to `HORIZONTAL_TICKS_GAP > plotWidth / n`, so
 * the axis hides iff `plotWidth < 6n`. At n = 46 that threshold is 276px. On a 320px card the
 * estimate is `320 − 8 (grid.left) − 16 (grid.right) − 0 (the gutter, zero under jsdom) − 26 (the
 * value-axis title)` = 270px — under 276, so it hides. Delete any one subtraction and it clears the
 * threshold: without `grid.left` 278, without `grid.right` 286, without the title term 296. So
 * every term of `estimatePlotWidth` that is non-zero under jsdom is genuinely pinned, and each was
 * checked by deleting it rather than by reading the sum.
 *
 * Do not round this count off — check the arithmetic against 6n first. `cartesianXLabels.test.ts`
 * started at 48, where the threshold is 288 and dropping `grid.left` (278) or `grid.right` (286)
 * still hid the axis, so two of the three mutations passed. The gutter term stays unpinnable at any
 * n, because `measureText` contributes exactly 0 here; see the header.
 *
 * At 1200px wide the same 46 categories get `(1200 − 8 − 16 − 26) / 46` = 25px each, clear of the
 * 6px gap, so they are drawn upright — which is what makes the pair of widths an assertion about
 * the WIDTH rather than about the row count.
 */
const CROWDED = 46;
const NARROW: EChartsSize = { width: 320, height: 260 };
const WIDE: EChartsSize = { width: 1200, height: 500 };

const boxCols = [{ name: "c", type: "varchar" }, { name: "v", type: "double" }];
const wfCols = [{ name: "x", type: "varchar" }, { name: "m", type: "double" }];

const CHARTS: Chart[] = [
  {
    what: "box plot",
    build: (rows, size, rotation) => buildBoxplotOption(
      boxCols, rows, { category: "c", value: "v", ...(rotation != null ? { xLabelRotation: rotation } : {}) }, size,
    ),
    rows: [["Widget", 10], ["Gadget", 20], ["Gizmo", 30], ["Doohickey", 40]],
    // SORTED, not as the rows arrive: a box plot's nominal axis draws ascending now, and what is
    // MEASURED has to be the labels as DRAWN or the fit is computed for an axis nobody sees.
    labels: ["Doohickey", "Gadget", "Gizmo", "Widget"],
    crowdedRows: Array.from({ length: CROWDED }, (_, i) => [`Category ${i}`, i + 1]),
  },
  {
    what: "waterfall",
    build: (rows, size, rotation) => buildWaterfallOption(
      wfCols, rows, { xAxis: "x", metric: "m", ...(rotation != null ? { xLabelRotation: rotation } : {}) }, size,
    ),
    // ONE FEWER ROW than the box plot's, in both fixtures: `showTotal` defaults on, so the builder
    // appends a "Total" category of its own. It is drawn like any other, so it is measured like any
    // other — the counts below are the categories on the axis, not the rows in the result.
    rows: [["Widget", 10], ["Gadget", 20], ["Gizmo", 30]],
    labels: ["Widget", "Gadget", "Gizmo", "Total"],
    crowdedRows: Array.from({ length: CROWDED - 1 }, (_, i) => [`Category ${i}`, i + 1]),
  },
];

CHARTS.forEach((c) => {
  describe(`${c.what} x-axis labels`, () => {
    /**
     * The whole point of the change: labels are all-or-nothing. `interval: "auto"` let ECharts drop
     * the ones that collided, which is how four categories came out with one label on a dashboard
     * card. Whatever is decided, the axis never draws a subset.
     */
    it("never leaves the interval to ECharts once it has a size", () => {
      expect(xAxisOf(c.build(c.rows, WIDE)).axisLabel.interval).toBe(0);
      expect(xAxisOf(c.build(c.rows, NARROW)).axisLabel.interval).toBe(0);
    });

    it("keeps today's behaviour when no size is available", () => {
      // Unit tests and any caller passing a plain option object have no measurement. Leaving ECharts
      // to thin the labels is worse than rotating, but it is what shipped, and guessing a width here
      // would be the very thing this module was written to stop.
      const o = c.build(c.rows);
      expect(xAxisOf(o).axisLabel.interval).toBe("auto");
      expect(xAxisOf(o).axisLabel.show).toBe(true);
    });

    /**
     * A zero size is not a very small chart, it is an UNMEASURED one — `EChartsRenderer` reads
     * `getBoundingClientRect()` before `init`, so a hidden or not-yet-laid-out container reports
     * `{0, 0}`. Measured anyway, `plotWidth` comes out negative and the fitter answers "draw every
     * label upright" for a container nobody could see, while a zero height would make a rotated
     * label's projected share infinite and hide the axis instead. Either way it is a decision taken
     * against a width that was never observed, so it is declined and the size-blind behaviour stands.
     */
    it("treats a zero size as no measurement rather than as a tiny chart", () => {
      fitCalls.length = 0;
      const o = c.build(c.crowdedRows, { width: 0, height: 0 });
      expect(fitCalls).toHaveLength(0);
      expect(xAxisOf(o).axisLabel.interval).toBe("auto");
      expect(xAxisOf(o).axisLabel.show).toBe(true);
    });

    /**
     * The measurement is a function of the WIDTH, which is the defect's root cause: the deleted rule
     * asked about label count and character length, so the same data got the same answer on a
     * full-width chart and on a one-third-width card. Same rows, same config, two widths, two
     * different axes.
     */
    it("decides from the width, not from the data", () => {
      expect(xAxisOf(c.build(c.crowdedRows, WIDE)).axisLabel.show).toBe(true);
      // 46 categories across ~270px of plot leaves 5.87px each — under the 6px minimum gap between
      // two labels, and far under the 90° rung's `fontSize * 1.2`. Metabase hides the axis rather
      // than drawing a wall of text or, as egret did, a few legible names out of forty-six.
      expect(xAxisOf(c.build(c.crowdedRows, NARROW)).axisLabel.show).toBe(false);
    });

    it("honours an explicit rotation over anything measurement would choose", () => {
      const o = c.build(c.rows, { width: 1600, height: 900 }, 90);
      expect(xAxisOf(o).axisLabel.rotate).toBe(90);
    });

    /**
     * Auto-selection applies only while the setting is untouched, so the guard has to be "did the
     * user choose?" and not "is there a fit?" — with the latter, a user who asks for 45° on a card
     * narrow enough that measurement would have said `hide` gets their rotation honoured AND their
     * axis hidden anyway, which reads as the setting doing nothing. The axis must still draw,
     * rotated as asked, and still draw EVERY label. The same correction Task 1's review made to the
     * cartesian wiring, which these two mirror.
     */
    it("skips the measurement for a user's own rotation, on a card it would have hidden", () => {
      expect(xAxisOf(c.build(c.crowdedRows, NARROW)).axisLabel.show).toBe(false);

      fitCalls.length = 0;
      const asked = c.build(c.crowdedRows, NARROW, 45);
      expect(fitCalls).toHaveLength(0);
      expect(xAxisOf(asked).axisLabel.show).toBe(true);
      expect(xAxisOf(asked).axisLabel.rotate).toBe(45);
      expect(xAxisOf(asked).axisLabel.interval).toBe(0);
    });

    /**
     * Both builders used to add a flat 18/44 to `grid.bottom` for a tilted label. They no longer
     * add anything, at any angle: `containLabel: true` already expands the grid around the rotated
     * labels, so that ladder reserved a band ECharts had reserved and took it off the plot instead
     * — see `cartesianXLabels.test.ts`, which pins the same contract across all three builders and
     * carries the argument, and `containLabelRotation.echarts.test.ts`, which measures it.
     *
     * Kept here as well, on the fixture each builder is table-driven over, because this is the file
     * a reader lands in when they wonder why the box plot's tilted axis has no room reserved.
     */
    it("adds nothing to grid.bottom the further the labels turn", () => {
      const big = { width: 1600, height: 900 };
      const flat = gridBottomOf(c.build(c.rows, big));
      expect(gridBottomOf(c.build(c.rows, big, 45))).toBe(flat);
      expect(gridBottomOf(c.build(c.rows, big, 90))).toBe(flat);
    });

    /**
     * The height guard is a share of the CARD (Metabase's `checkHeight` divides by `outerHeight`),
     * so handing it the plot height instead — `size.height` minus the grid's own insets, or minus
     * the room reserved for an axis title — would tighten the 70% silently and start hiding axes
     * that fit. Nothing about the option that comes out could show that, so it is checked here.
     */
    it("measures the drawn labels, against the container's height rather than a grid-adjusted one", () => {
      fitCalls.length = 0;
      c.build(c.rows, { width: 640, height: 480 });
      expect(fitCalls).toHaveLength(1);
      expect(fitCalls[0].outerHeight).toBe(480);
      // The width, by contrast, IS inset — see `estimatePlotWidth`. How far is a browser question
      // (the gutter measures 0 here); that it is inset at all is not.
      expect(fitCalls[0].plotWidth).toBeLessThan(640);
      expect(fitCalls[0].plotWidth).toBeGreaterThan(0);
      expect(fitCalls[0].labels).toEqual(c.labels);
    });
  });
});

/**
 * THE WATERFALL'S STEPS READ AS PERIODS — the cartesian axis's treatment, on the other builder the
 * plan named and no task had touched.
 *
 * The defect: `WaterfallBuildConfig` carried no `xAxisUnit` and `ChartRenderer` passed none, so a
 * waterfall grouped by month labelled its steps "2026-01-01" — the bucket's first DAY — and after
 * the axis became all-or-nothing, twelve of those ten-character labels on a one-third-width card
 * meant NO x labels at all. `temporalXLabels.test.ts` is this file's opposite number for the
 * cartesian family; the arguments are the same because the implementation is deliberately the same
 * closure shape, and only the fixture and the appended "Total" differ.
 *
 * THE BOX PLOT IS NOT HERE, on purpose: its categories are group names, not temporal buckets.
 *
 * WHAT THIS BLOCK CAN AND CANNOT REACH is the same split the file header states, applied one level
 * up. The naming arms and the fitter's ARGUMENTS are pure decisions and are asserted directly. The
 * `thin` DECISION is not reachable through the real fitter under jsdom — every label measures 0px,
 * so nothing collides — so the stride ladder is driven with a stub measurer in `axisLabelFit.test.ts`
 * and what is left here is the builder's half: the precondition it enforces before a stride is
 * offered, the candidate it sizes one against, and the `interval` a `thin` turns into.
 */
describe("a bucketed temporal waterfall", () => {
  const COLS = [{ name: "d", type: "date" }, { name: "m", type: "double" }];
  const ROWS: unknown[][] = [["2026-01-01", 10], ["2026-02-01", 4]];
  const SIZE: EChartsSize = { width: 320, height: 200 };
  // `satisfies`, not a bare object: spread properties escape excess-property checking, so `xAxs`
  // here would compile clean and silently leave every case below without an x column.
  const base = { xAxis: "d", metric: "m" } satisfies Partial<WaterfallBuildConfig>;
  const build = (cfg: Partial<WaterfallBuildConfig>, size?: EChartsSize) =>
    buildWaterfallOption(COLS, ROWS, { ...base, ...cfg }, size);
  const label = (cfg: Partial<WaterfallBuildConfig>, v: unknown = "2026-01-01") =>
    xAxisOf(build(cfg)).axisLabel.formatter?.(v);

  it("names the period at each truncation unit the chart can be grouped by", () => {
    expect(label({ xAxisUnit: "year" })).toBe("2026");
    expect(label({ xAxisUnit: "quarter" }, "2026-07-01")).toBe("Q3 2026");
    expect(label({ xAxisUnit: "month" })).toBe("January 2026");
    expect(label({ xAxisUnit: "day" })).toBe("January 1, 2026");
    expect(label({ xAxisUnit: "hour" }, "2026-01-01T13:00:00Z")).toBe("January 1, 2026, 1:00 PM");
  });

  /**
   * The guard, and the negative half matters more than the positive. A column that was NOT bucketed
   * reaches the axis as whatever the engine returned, and naming a period from it would invent a
   * granularity the query never asked for. An extraction unit is the same case from the other side:
   * its value is a finished word, and `dayjs("3")` for a `day_of_week` of 3 is VALID and formats as
   * "March 2001", so the arm has to be refused rather than left to fail on its own.
   */
  it("leaves an unbucketed column, and an extraction unit's value, raw", () => {
    expect(label({})).toBe("2026-01-01");
    expect(label({ xAxisUnit: "day_of_week" }, "3")).toBe("3");
  });

  /** The Total is a bar the CHART invented by summing the steps; it is not a bucket and there is no
   *  period to name from it. It survives arm 3 because `periodName` is total — `dayjs("Total")` is
   *  invalid — rather than because anything special-cases it. */
  it("leaves the invented Total step alone on a bucketed axis", () => {
    expect(label({ xAxisUnit: "month" }, "Total")).toBe("Total");
  });

  it("draws nothing for an empty step rather than naming a period from it", () => {
    expect(label({ xAxisUnit: "month" }, null)).toBe("");
  });

  /** A user who set a date style has SAID what they want, and a stored-then-ignored setting is the
   *  defect the pivot, the pie and the breakout legend each shipped with in turn. */
  it("lets the column's own format win over the period name", () => {
    expect(label({ xAxisUnit: "month", columnSettings: { d: { date_style: "YYYY-MM-DD" } } }))
      .toBe("2026-01-01");
  });

  it("still names the period when the format is on a DIFFERENT column", () => {
    expect(label({ xAxisUnit: "month", columnSettings: { m: { date_style: "YYYY-MM-DD" } } }))
      .toBe("January 2026");
  });

  /**
   * A format object SILENT ABOUT DATES does not outrank the period name, and this is not
   * hypothetical: `FormatSection`'s `patch` spreads into `{...fmt, ...p}` (`settings/
   * FormatSection.tsx:104`), so renaming the step column to "Month" stores `{column_title: "Month"}`
   * — truthy, which the cartesian axis took as arm 1 until a reviewer caught it, reverting that axis
   * from "January 2026" to `formatDate`'s default "January 1, 2026" with nothing in the panel to
   * explain it. Shipping truthiness here would have been the same defect a second time.
   */
  it("is not displaced by a format that says nothing about dates", () => {
    expect(label({ xAxisUnit: "month", columnSettings: { d: { column_title: "Month" } } }))
      .toBe("January 2026");
    // A title typed and then deleted persists the key with no value. Still a truthy object.
    expect(label({ xAxisUnit: "month", columnSettings: { d: { column_title: undefined } } }))
      .toBe("January 2026");
    // A date switch toggled on and back off leaves the falsy value, which means "not set".
    expect(label({ xAxisUnit: "month", columnSettings: { d: { date_abbreviate: false } } }))
      .toBe("January 2026");
    // …but the same switch left ON is a date instruction, and takes arm 1 with the default style.
    expect(label({ xAxisUnit: "month", columnSettings: { d: { date_abbreviate: true } } }))
      .toBe("Jan 1, 2026");
  });

  /** The higher bar applies ONLY where a period name is in play. On an unbucketed axis there is
   *  nothing for the format to outrank, so it keeps winning on existence — anything else would be
   *  this task changing behaviour it was not sent to change. */
  it("keeps the old existence test where there is no period name to outrank", () => {
    expect(label({ columnSettings: { d: { column_title: "When" } } })).toBe("January 1, 2026");
  });

  /**
   * THE FITTER MEASURES THE STRING THE AXIS DRAWS.
   *
   * It measured `categories`, the raw bucket, while the formatter now draws the period name, and the
   * comment there justified the gap as "the formatted label is wider, so this under-measures".
   * Naming the periods falsifies both halves at once: the two differ whenever the axis is bucketed,
   * and the period name is often NARROWER — a year measured "2026-01-01" at ten characters to draw
   * "2026" at four. Over-measuring rotates labels that would have sat upright, in the change that
   * exists because labels did not fit.
   *
   * Asserted as the ARGUMENT rather than as a rotation, because no rotation is reachable from here:
   * every label is 0px wide under jsdom, so both strings produce the same decision.
   */
  it("hands the fitter the drawn labels, not the raw buckets", () => {
    fitCalls.length = 0;
    buildWaterfallOption(
      COLS, [["2025-01-01", 1], ["2026-01-01", 2]], { ...base, xAxisUnit: "year" }, SIZE,
    );
    expect(fitCalls).toHaveLength(1);
    // "Total" included: it is drawn like any other category, so it is measured like any other.
    expect(fitCalls[0].labels).toEqual(["2025", "2026", "Total"]);
  });

  it("measures the column's format too, where that is what wins", () => {
    fitCalls.length = 0;
    build({ xAxisUnit: "month", columnSettings: { d: { date_style: "YYYY-MM-DD" } } }, SIZE);
    expect(fitCalls[0].labels).toEqual(["2026-01-01", "2026-02-01", "Total"]);
  });
});

/**
 * The waterfall's half of the thinning contract. `temporalXLabels.test.ts` makes these same claims
 * about the cartesian axis; they are repeated rather than shared because the two builders hold
 * their own copy of this wiring, which is precisely how the waterfall came to be missing it.
 */
describe("a bucketed temporal waterfall thins to period boundaries", () => {
  const COLS = [{ name: "d", type: "date" }, { name: "m", type: "double" }];
  const ROWS: unknown[][] = [["2026-01-01", 10], ["2026-02-01", 4]];
  const SIZE: EChartsSize = { width: 320, height: 200 };
  const base = { xAxis: "d", metric: "m" } satisfies Partial<WaterfallBuildConfig>;
  const build = (cfg: Partial<WaterfallBuildConfig>) =>
    buildWaterfallOption(COLS, ROWS, { ...base, ...cfg }, SIZE);
  /** Clears the spy's log first, so a test that builds more than once reads THIS build's argument
   *  and not the first one it happened to make. */
  const boundaryOf = (cfg: Partial<WaterfallBuildConfig>) => {
    fitCalls.length = 0;
    build(cfg);
    expect(fitCalls, "the fitter was not consulted at all").toHaveLength(1);
    return fitCalls[0].boundary;
  };

  /** The boundary is the LADDER AND NOTHING ELSE — Metabase's `TICKS_TIMESERIES_INTERVALS` at and
   *  above `month`, read from `temporal-period.ts`, so this asserts the wiring rather than restating
   *  the values, which `temporal-period.test.ts` pins.
   *
   *  It carried a `widestLabel: widestPeriodName(cfg.xAxisUnit, drawLabel)` too — the widest name
   *  the unit could produce, threaded through this builder's label closure. `strideThatFits` slices
   *  each rung's DRAWN labels out of `labels:` now, and the two `measures ...` cases above pin that
   *  `labels:` is this builder's own drawn output ("Total" and column format included), so what the
   *  field was protecting is still true without it. */
  it("offers the stride ladder for the unit and nothing else", () => {
    expect(boundaryOf({ xAxisUnit: "month" })).toEqual({ steps: boundarySteps("month") });
    expect(boundaryOf({ xAxisUnit: "quarter" })).toEqual({ steps: boundarySteps("quarter") });
  });

  /**
   * THE NEGATIVE, and the one the whole design rests on. A waterfall of named stages gets no
   * `boundary` at all, so `fitXLabels` cannot reach its stride arm — the all-or-nothing ladder is
   * what a non-bucketed axis still gets, because there is no such thing as every second stage. An
   * unbucketed DATE column is the same case: its values are whatever the engine returned. An
   * EXTRACTION unit is the third: "Mon"/"Tue" are seven finished words, not a period each.
   */
  it("offers no stride to a waterfall that is not bucketed", () => {
    fitCalls.length = 0;
    buildWaterfallOption(
      [{ name: "stage", type: "varchar" }, { name: "m", type: "double" }],
      [["Leads", 10], ["Demos", 4]], { xAxis: "stage", metric: "m" }, SIZE,
    );
    expect(fitCalls).toHaveLength(1);
    expect(fitCalls[0].boundary).toBeUndefined();

    expect(boundaryOf({})).toBeUndefined();
    expect(boundaryOf({ xAxisUnit: "day_of_week" })).toBeUndefined();
  });

  /**
   * `interval` COUNTS THE LABELS SKIPPED, NOT THE STRIDE. ECharts reads it as
   * `step = max((interval || 0) + 1, 1)` (`echarts/lib/scale/helper.js:235`), so every 6th is 5. An
   * off-by-one draws January, June and November instead of January and July, which still looks like
   * a plausible date axis in a screenshot — hence an assertion rather than an eyeball. The waterfall
   * had `interval: decided ? 0 : "auto"` and nothing else, so wiring a `boundary` in without this
   * line would have drawn every label under a `thin` decision.
   */
  it("writes the stride as the count of labels ECharts should skip", () => {
    fitResult.value = { kind: "thin", rotate: 0, every: 6 };
    const axis = xAxisOf(build({ xAxisUnit: "month" })).axisLabel;
    expect(axis.interval).toBe(5);
    // Upright and shown: thinning is what lets the labels stay horizontal, and it is not a hide.
    expect(axis.rotate).toBe(0);
    expect(axis.show).toBe(true);
  });

  /** AND IT DECLARES THE FIRST BUCKET REQUIRED, without which `AxisBuilder.fixMinMaxLabelShow`
   *  drops index 0 — the label the stride exists to draw — in favour of the off-interval end tick
   *  ECharts appends after it. The mechanism is at the option line in `options/cartesian.ts`; this
   *  pins that the waterfall writes the flag too, since the two builders hold their own copy of this
   *  wiring and that is exactly how the waterfall came to be missing the last one. */
  it("declares the first bucket a required label on a thinned axis", () => {
    fitResult.value = { kind: "thin", rotate: 0, every: 6 };
    expect(xAxisOf(build({ xAxisUnit: "month" })).axisLabel.showMinLabel).toBe(true);
  });

  /** The other three arms are untouched, which is the claim "the all-or-nothing rule was not undone"
   *  has to survive: a decided axis still writes 0 — every label or none — and only `thin` writes
   *  anything else, `showMinLabel` included. */
  it("still writes every-label-or-none for every other decision", () => {
    for (const value of [
      { kind: "show", rotate: 0 },
      { kind: "rotate", rotate: 45 },
      { kind: "hide" },
    ] as XLabelFit[]) {
      fitResult.value = value;
      const axis = xAxisOf(build({ xAxisUnit: "month" })).axisLabel;
      expect(axis.interval, value.kind).toBe(0);
      expect(axis.showMinLabel, value.kind).toBeUndefined();
    }
  });

  /** A stride of 1 is every label, so it must write 0 and not -1. ECharts would render the two the
   *  same — `-1` is truthy, so the `|| 0` never fires; it is the `Math.max(…, 1)` in the SAME
   *  expression that clamps the step back to 1. So the behaviour is safe either way and only the
   *  arithmetic is wrong, which is exactly the kind of error that survives a screenshot and then
   *  stops being harmless the day the expression changes. */
  it("writes 0 for a stride of one rather than a negative interval", () => {
    fitResult.value = { kind: "thin", rotate: 0, every: 1 };
    expect(xAxisOf(build({ xAxisUnit: "month" })).axisLabel.interval).toBe(0);
  });
});

/**
 * WHICH VALUE the box plot's gutter is measured from — an assertion about the estimate's INPUT, not
 * about a pixel. The width that value turns into is still `measureText`'s, still 0 under jsdom, and
 * still unpinnable here (see this file's header); what is pinnable, and what was wrong, is which
 * number gets handed over.
 *
 * The bug: `widest` is the max over the BOXES, and with Tukey whiskers a box deliberately stops at
 * the largest value inside the fence — `boxStats`' own rule, so that a whisker never reaches out to
 * the outlier it just excluded. Those outliers are still drawn, on a scatter series sharing this
 * axis, and so is the mean, which an outlier drags past its own box. So the axis extends past
 * `widest` and prints a wider top tick than the boxes ever suggested, and the gutter under-reserved
 * by that difference — which over-estimates `plotWidth` and leaves labels upright and overlapping.
 * At ~40px on a ~270px card that is larger than any case `estimatePlotWidth`'s doc names.
 *
 * The VALUE is asserted directly and the FORMATTER through its own output. Until PR 03 the builder
 * handed over a finished string, so both halves had to be inferred from it and the first assertion
 * would have read plausibly if the two numbers happened to format alike — the reason the second
 * assertion existed. Now the estimate takes `(widest, format)`, so which number was chosen is
 * asserted as a number, and "through the same formatter the axis uses" is asserted by running the
 * captured closure and the axis's own on the same input. Neither hard-codes this repo's number
 * format, which would make these fail on an unrelated formatting change.
 */
describe("the box plot's value-axis gutter", () => {
  const cols = [{ name: "c", type: "varchar" }, { name: "v", type: "double" }];
  /** Four 1s and a 1000. The quartiles all land on 1, so the IQR is zero, the Tukey fence is
   *  [1, 1], and 1000 is an OUTLIER: the box tops out at 1 while the point is drawn separately on
   *  this same axis. The mean (200.8) is dragged out with it — the same defect, one series over. */
  const rows: unknown[][] = [["A", 1], ["A", 1], ["A", 1], ["A", 1], ["A", 1000]];
  const SIZE = { width: 640, height: 480 };
  const yFmtOf = (o: unknown) =>
    (o as { yAxis: { axisLabel: { formatter: (v: number) => string } } }).yAxis.axisLabel.formatter;

  it("measures every value the axis can print, not only the boxes", () => {
    gutterCalls.length = 0;
    const o = buildBoxplotOption(cols, rows, { category: "c", value: "v" }, SIZE);
    expect(gutterCalls).toHaveLength(1);
    expect(gutterCalls[0].widest).toBe(1000);
    // And through the axis's own formatter, which is the other half of the decision: a candidate
    // measured in a format the axis does not draw in is measured against the wrong string.
    expect(gutterCalls[0].format(1000)).toBe(yFmtOf(o)(1000));
  });

  it("counts a point series only when that series is actually drawn", () => {
    // With no points and no mean, nothing but the boxes reaches the axis — so the gutter is back to
    // the boxes, and `axisWidest` must not be "the extent of the data" in general.
    gutterCalls.length = 0;
    const o = buildBoxplotOption(
      cols, rows, { category: "c", value: "v", points: "none", showMean: false }, SIZE,
    );
    expect(gutterCalls).toHaveLength(1);
    expect(gutterCalls[0].widest).toBe(1);
    expect(gutterCalls[0].format(1)).toBe(yFmtOf(o)(1));
  });
});
