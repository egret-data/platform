import { describe, it, expect, vi } from "vitest";
import type { EChartsSize } from "../EChartsRenderer";
import { buildBoxplotOption } from "../options/boxplot";
import { buildWaterfallOption } from "../options/waterfall";
import type { fitXLabels as FitXLabels, estimatePlotWidth as EstimatePlotWidth } from "../axisLabelFit";

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

/** Every argument a builder hands the fitter and the plot-width estimate, in call order.
 *  `vi.hoisted` because the `vi.mock` factory below is hoisted above this file's imports and would
 *  otherwise touch these bindings in their temporal dead zone. */
const { fitCalls, gutterLabels } = vi.hoisted(() => ({
  fitCalls: [] as FitArgs[],
  gutterLabels: [] as string[],
}));

/** Pass-through spies, not stubs: the real ladder and the real estimate still decide, so every
 *  assertion in this file about what the AXIS ends up as remains an assertion about the real
 *  module. All they add is a view of what went IN — see the header on `outerHeight`, and the
 *  gutter block at the foot of the file. */
vi.mock("../axisLabelFit", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../axisLabelFit")>();
  return {
    ...actual,
    fitXLabels: (args: FitArgs) => {
      fitCalls.push(args);
      return actual.fitXLabels(args);
    },
    estimatePlotWidth: (...args: EstimateArgs) => {
      gutterLabels.push(args[1]);
      return actual.estimatePlotWidth(...args);
    },
  };
});

interface XAxisShape {
  axisLabel: { interval: unknown; rotate?: number; show?: boolean };
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
    labels: ["Widget", "Gadget", "Gizmo", "Doohickey"],
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
 * Compared against the axis's OWN formatter rather than against a literal, so this pins the
 * invariant the comment states — "through the same formatter the axis uses" — without hard-coding
 * this repo's number format, which would make the test fail on an unrelated formatting change.
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
    gutterLabels.length = 0;
    const o = buildBoxplotOption(cols, rows, { category: "c", value: "v" }, SIZE);
    expect(gutterLabels).toHaveLength(1);
    expect(gutterLabels[0]).toBe(yFmtOf(o)(1000));
    // Named explicitly: this is the value the gutter WAS measured from, and the assertion above
    // would still read plausibly if the two happened to format alike.
    expect(gutterLabels[0]).not.toBe(yFmtOf(o)(1));
  });

  it("counts a point series only when that series is actually drawn", () => {
    // With no points and no mean, nothing but the boxes reaches the axis — so the gutter is back to
    // the boxes, and `axisWidest` must not be "the extent of the data" in general.
    gutterLabels.length = 0;
    const o = buildBoxplotOption(
      cols, rows, { category: "c", value: "v", points: "none", showMean: false }, SIZE,
    );
    expect(gutterLabels).toHaveLength(1);
    expect(gutterLabels[0]).toBe(yFmtOf(o)(1));
  });
});
