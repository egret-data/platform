import { describe, it, expect, vi } from "vitest";
import { buildCartesianOption } from "../options/cartesian";
import { buildBoxplotOption } from "../options/boxplot";
import { buildWaterfallOption } from "../options/waterfall";
import { CHART_THEME } from "../palette";
import type { fitXLabels as FitXLabels } from "../axisLabelFit";

/**
 * The wiring between `fitXLabels` and the cartesian builder.
 *
 * WHAT THIS LAYER CAN AND CANNOT ASSERT. `measure-text.ts` returns `{width: 0, height: 0}` under
 * jsdom by design ("Size claims belong in a browser"), and `buildCartesianOption` uses the REAL
 * measurer — there is no seam to inject a stub through, and adding one would be a production
 * parameter that exists only for a test. So every label here measures 0px wide, and the ladder can
 * only be driven through the rungs that depend on `dimensionWidth = plotWidth / labelCount` rather
 * than on a label's own width:
 *
 *   - no overlap (`HORIZONTAL_TICKS_GAP` of 6 fits) -> draw upright;
 *   - overlap, and `dimensionWidth` under `fontSize * 1.2` -> hide the axis.
 *
 * `rotate: 45` and `rotate: 90` sit BETWEEN those two, and reaching them needs `dimensionWidth`
 * to be at once under 6 (to collide with zero-width labels) and at least 15.6 (to clear the 90°
 * rung) — impossible. So no test in this file asserts a MEASURED rotation; that is
 * `axisLabelFit.test.ts`'s job, where the measurer is injected, and the
 * live comparison's. What is asserted here is the wiring: that a size reaches the fitter, that the
 * decision reaches the option, and that an explicit setting stops the measurement entirely.
 *
 * `outerHeight` HAS NO OBSERVABLE EFFECT FROM HERE and is therefore checked on the way IN, by the
 * pass-through spy below: the 70% height guard divides the widest label by it, the widest label is
 * 0px, and 0/anything clears the guard — so passing a grid-adjusted height instead of the
 * container's would silently tighten the guard in a browser and change nothing in this file.
 *
 * The value-axis gutter inside `plotWidth` is not pinned by anything here AT ALL, and cannot be:
 * it is a `measureText` call, so it contributes exactly 0 under jsdom whatever the numbers on the
 * axis, and deleting the term from `estimatePlotWidth` leaves every test in the repo green. It is
 * a browser question and stays one — Task 4's live comparison is where it is answered.
 */

const cols = [{ name: "category", type: "varchar" }, { name: "revenue", type: "double" }];
/** The four categories from the live defect. Four is comfortably clear of the hide threshold at
 *  every width used below — the axis hides iff `plotWidth < 6n`, so 24px, and the narrowest card
 *  here estimates 270px — which is the point: these rows exercise the DRAWN rungs, and
 *  `crowdedRows` exercises the hidden one. */
const rows: unknown[][] = [
  ["Widget", 10], ["Gadget", 20], ["Gizmo", 30], ["Doohickey", 40],
];
const cfg = { kind: "bar" as const, xAxis: "category", yMetrics: ["revenue"] };

/**
 * Enough categories that the plot width divided between them leaves less room than the minimum gap
 * between two labels — the "no room at all" end of the ladder, which is width-driven and so
 * survives a measurer that reports zero.
 *
 * FORTY-SIX, and the exact count is load-bearing. With every label 0px wide the axis hides iff
 * `plotWidth < 6n`, so at n = 46 the threshold is 276px. On a 320px card the estimate is
 * `320 − 8 (grid.left) − 16 (grid.right) − 0 (the gutter, zero under jsdom) − 26 (the value-axis
 * title)` = 270px — under 276, so it hides. Delete any one subtraction and it clears the
 * threshold: without `grid.left` 278, without `grid.right` 286, without the title term 296. So
 * every term of `estimatePlotWidth` that is non-zero under jsdom is genuinely pinned.
 *
 * This was 48 first, which pinned only the title term: the threshold was 288, and dropping
 * `grid.left` (278) or `grid.right` (286) still hid the axis, so both mutations passed. Do not
 * round this number off — check the arithmetic against 6n first. The gutter term stays unpinnable
 * at any n, because `measureText` contributes exactly 0 here; see the header.
 */
const crowdedRows: unknown[][] = Array.from({ length: 46 }, (_, i) => [`Category ${i}`, i]);

type FitArgs = Parameters<typeof FitXLabels>[0];

/** Every argument the builder hands the fitter, in call order. `vi.hoisted` because the `vi.mock`
 *  factory below is hoisted above this file's imports and would otherwise touch this binding in
 *  its temporal dead zone. */
const { fitCalls } = vi.hoisted(() => ({ fitCalls: [] as FitArgs[] }));

/** A pass-through spy, not a stub: the real ladder still decides, so every assertion in this file
 *  about what the AXIS ends up as remains an assertion about the real module. All the spy adds is
 *  a view of `outerHeight` — see the header. */
vi.mock("../axisLabelFit", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../axisLabelFit")>();
  return {
    ...actual,
    fitXLabels: (args: FitArgs) => {
      fitCalls.push(args);
      return actual.fitXLabels(args);
    },
  };
});

interface XAxisShape {
  axisLabel: { interval: unknown; rotate?: number; show?: boolean };
}
const xAxisOf = (o: unknown) => (o as { xAxis: XAxisShape }).xAxis;

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
    const o = buildCartesianOption(cols, crowdedRows, cfg, { width: 0, height: 0 });
    expect(fitCalls).toHaveLength(0);
    expect(xAxisOf(o).axisLabel.interval).toBe("auto");
    expect(xAxisOf(o).axisLabel.show).toBe(true);
  });

  it("keeps today's behaviour when no size is available", () => {
    // Unit tests and any caller passing a plain option object have no measurement. Leaving ECharts
    // to thin the labels is worse than rotating, but it is what shipped, and guessing a width here
    // would be the very thing this module was written to stop.
    expect(xAxisOf(buildCartesianOption(cols, rows, cfg)).axisLabel.interval).toBe("auto");
  });

  /**
   * The measurement is a function of the WIDTH, which is the defect's root cause: the old rule
   * asked about label count and character length, so the same data got the same answer on a
   * full-width chart and on a one-third-width card. Same rows, same config, two widths, two
   * different axes.
   */
  it("decides from the width, not from the data", () => {
    const wide = buildCartesianOption(cols, crowdedRows, cfg, { width: 1200, height: 500 });
    const narrow = buildCartesianOption(cols, crowdedRows, cfg, { width: 320, height: 260 });
    expect(xAxisOf(wide).axisLabel.show).toBe(true);
    // Forty-six categories across ~270px of plot leaves 5.87px each — under the 6px minimum gap
    // between two labels, and far under the 90° rung's `fontSize * 1.2`. Metabase hides the axis
    // rather than drawing a wall of text or, as egret did, a few legible names out of forty-six.
    // At 1200px the same rows get 25px each and every name is drawn upright.
    expect(xAxisOf(narrow).axisLabel.show).toBe(false);
  });

  it("honours an explicit rotation over anything measurement would choose", () => {
    const o = buildCartesianOption(cols, rows, { ...cfg, xLabelRotation: 90 }, { width: 1600, height: 900 });
    expect(xAxisOf(o).axisLabel.rotate).toBe(90);
  });

  /**
   * The correction that came out of Task 1's review. Auto-selection applies only while the setting
   * is untouched, so the guard has to be "did the user choose?" and not "is there a fit?" — with
   * the latter, a user who asks for 45° on a card narrow enough that measurement would have said
   * `hide` gets their rotation honoured AND their axis hidden anyway, which reads as the setting
   * doing nothing. The axis must still draw, rotated as asked, and still draw EVERY label.
   */
  it("draws the axis a user asked to rotate, on a card measurement would have hidden", () => {
    const narrow = { width: 320, height: 260 };
    expect(xAxisOf(buildCartesianOption(cols, crowdedRows, cfg, narrow)).axisLabel.show).toBe(false);

    const asked = buildCartesianOption(cols, crowdedRows, { ...cfg, xLabelRotation: 45 }, narrow);
    expect(xAxisOf(asked).axisLabel.show).toBe(true);
    expect(xAxisOf(asked).axisLabel.rotate).toBe(45);
    expect(xAxisOf(asked).axisLabel.interval).toBe(0);
  });

  /**
   * NO BUILDER RESERVES ROOM BELOW THE AXIS FOR A TILTED LABEL — the regression PR 02's live
   * comparison caught, inverted into a test.
   *
   * All three used to add one: the fit's `bottomPad` on the measured path, and a flat 18/44 ladder
   * on the explicit one. `CHART_THEME.grid` sets `containLabel: true` (`palette.ts:39`), and
   * ECharts expands the grid around the axis labels INCLUDING their rotation — so every one of
   * those pixels was reserving a band ECharts had already reserved, and came straight off the plot.
   * On a one-third-width dashboard card the bars ended up about a fifth of the card's height with
   * the bottom 45% of the SVG empty. `containLabelRotation.echarts.test.ts` measures the claim
   * against real ECharts; this pins what the builders may put in `grid.bottom` because of it.
   *
   * Stated as an EQUALITY against the full expected value rather than as "the same for every
   * rotation", so that re-adding any pad fails here whether or not it varies with the angle. The
   * axis-title term is deliberately left in and exercised both ways — deleting that too would also
   * make every rotation agree, and it is a different reservation with a real reason behind it (an
   * unreserved axis name is drawn into the tick labels).
   *
   * This reaches only the EXPLICIT rotation, because a measured one is unreachable from jsdom (see
   * the header). The measured path is guarded from the other end instead, and it takes both halves:
   * `XLabelFit` no longer HAS a pad — so restoring `fit.bottomPad` fails `tsc`, and restoring the
   * field to the type to go with it fails `axisLabelFit.test.ts`, which asserts the fit's shape
   * with `toEqual` rather than `toMatchObject` for exactly this reason.
   */
  it.each([
    ["cartesian", (rotation?: 45 | 90, showXLabel?: boolean) =>
      buildCartesianOption(cols, rows, { ...cfg, xLabelRotation: rotation, showXLabel }, { width: 1600, height: 900 })],
    ["box plot", (rotation?: 45 | 90, showXLabel?: boolean) =>
      buildBoxplotOption(cols, rows, { category: "category", value: "revenue", xLabelRotation: rotation, showXLabel }, { width: 1600, height: 900 })],
    ["waterfall", (rotation?: 45 | 90, showXLabel?: boolean) =>
      buildWaterfallOption(cols, rows, { xAxis: "category", metric: "revenue", xLabelRotation: rotation, showXLabel }, { width: 1600, height: 900 })],
  ] as const)("%s adds nothing to grid.bottom for a rotated axis", (_what, build) => {
    const bottom = (o: unknown) => (o as { grid: { bottom: number } }).grid.bottom;
    const base = CHART_THEME.grid.bottom as number;
    for (const rotation of [undefined, 45, 90] as const) {
      // With the axis title (the default: `xTitle` falls back to the column name), and without it.
      expect(bottom(build(rotation, undefined))).toBe(base + 22);
      expect(bottom(build(rotation, false))).toBe(base);
    }
  });

  /**
   * `baseOption` is reached from THREE places — the empty-data early return, the breakout path and
   * the plain/folded path — and the size has to arrive at all three. Wiring one and missing another
   * is a silent behaviour split, where a plain bar measures its labels and a broken-out bar of the
   * same data does not, with nothing failing to say so. (Both of these caught a real gap: the first
   * draft threaded the size into the folded path only.)
   */
  it("measures a broken-out chart, not only a plain one", () => {
    const bcols = [
      { name: "category", type: "varchar" },
      { name: "series", type: "varchar" },
      { name: "revenue", type: "double" },
    ];
    const brows: unknown[][] = crowdedRows.flatMap((r) => [[r[0], "A", r[1]], [r[0], "B", r[1]]]);
    const bcfg = { kind: "bar" as const, xAxis: "category", series: "series", yMetrics: ["revenue"] };
    expect(xAxisOf(buildCartesianOption(bcols, brows, bcfg, { width: 320, height: 260 })).axisLabel.show).toBe(false);
    expect(xAxisOf(buildCartesianOption(bcols, brows, bcfg, { width: 1200, height: 500 })).axisLabel.show).toBe(true);
  });

  it("decides the empty-data axis too rather than leaving it to ECharts", () => {
    // No x dimension, so the builder returns before it has a single category. There is nothing to
    // lay out, but the axis must still be the DECIDED kind — otherwise two paths out of the same
    // function answer the same question differently for no reason a reader could find.
    const o = buildCartesianOption(cols, rows, { kind: "bar" as const, yMetrics: ["revenue"] }, { width: 320, height: 260 });
    expect(xAxisOf(o).axisLabel.interval).toBe(0);
    expect(xAxisOf(o).axisLabel.show).toBe(true);
  });

  /**
   * The height guard is a share of the CARD (Metabase's `checkHeight` divides by `outerHeight`),
   * so handing it the plot height instead — `size.height` minus the grid's own insets, or minus
   * the room reserved for the legend — would tighten the 70% silently and start hiding axes that
   * fit. Nothing about the option that comes out could show that, so it is checked here.
   */
  it("measures against the container's height, not a grid-adjusted one", () => {
    fitCalls.length = 0;
    buildCartesianOption(cols, rows, cfg, { width: 640, height: 480 });
    expect(fitCalls).toHaveLength(1);
    expect(fitCalls[0].outerHeight).toBe(480);
    // The width, by contrast, IS inset — see `estimatePlotWidth`. How far is a browser question
    // (the gutter measures 0 here); that it is inset at all is not.
    expect(fitCalls[0].plotWidth).toBeLessThan(640);
    expect(fitCalls[0].plotWidth).toBeGreaterThan(0);
    expect(fitCalls[0].labels).toEqual(["Widget", "Gadget", "Gizmo", "Doohickey"]);
  });

  /** A row chart's categories run down the LEFT, where length costs nothing, so there is no
   *  collision for the MEASUREMENT to find. It is not asked. */
  it("does not measure a horizontal chart's category axis", () => {
    fitCalls.length = 0;
    const o = buildCartesianOption(cols, crowdedRows, { ...cfg, horizontal: true }, { width: 320, height: 260 });
    const yAxis = (o as { yAxis: XAxisShape }).yAxis;
    expect(fitCalls).toHaveLength(0);
    expect(yAxis.axisLabel.show).toBe(true);
    expect(yAxis.axisLabel.interval).toBe("auto");
  });

  /**
   * …but an EXPLICIT rotation is a different question, and it is honoured wherever the category
   * axis sits. Not measuring a row chart is a statement about collisions; refusing to rotate one
   * would be the setting doing nothing, and the panel offers this control on row charts (where it
   * names the vertical axis as the category one).
   *
   * ECharts does apply it there — `buildAxisLabel` reads `axisLabel.rotate` for every axis kind,
   * and `innerTextLayout` composes it with the axis's own `PI/2`, so the names turn to run
   * vertically alongside the axis rather than being ignored.
   */
  it("honours an explicit rotation on a row chart's vertical category axis", () => {
    const o = buildCartesianOption(cols, crowdedRows, { ...cfg, horizontal: true, xLabelRotation: 90 }, { width: 320, height: 260 });
    const yAxis = (o as { yAxis: XAxisShape }).yAxis;
    expect(yAxis.axisLabel.rotate).toBe(90);
    // Still all-or-nothing: the user chose, so every label is drawn.
    expect(yAxis.axisLabel.interval).toBe(0);
  });

  /** …and the room it needs is on the LEFT, which `containLabel` reserves. Padding `grid.bottom`
   *  for a rotation that happened on the left axis would open a gap under a chart that has no
   *  bottom labels at all. */
  it("does not pad the bottom for a rotation that happened on the left axis", () => {
    const rowCfg = { ...cfg, horizontal: true };
    const flat = buildCartesianOption(cols, crowdedRows, rowCfg, { width: 320, height: 260 }) as { grid: { bottom: number } };
    const turned = buildCartesianOption(cols, crowdedRows, { ...rowCfg, xLabelRotation: 90 }, { width: 320, height: 260 }) as { grid: { bottom: number } };
    expect(turned.grid.bottom).toBe(flat.grid.bottom);
  });

  /**
   * A linear or timeseries x is a VALUE axis: ECharts generates its tick labels, and `categories`
   * is not what gets drawn. Measuring them anyway would reserve room below the plot for a rotation
   * that is never applied — `scaleAxis` carries no `rotate` — so the fitter is not called at all.
   * Fitting a value axis means generating its ticks first, and no PR owns that: this said "which is
   * PR 03's job", and PR 03 turned out to be about which axis a scatter gets, not about laying out
   * the ticks on it.
   *
   * `xScale: "linear"` here is EXPLICIT and stays that way. `resolveXScale` reaches a value axis on
   * its own only for a scatter, and this is a bar chart, so the case below is the stored one — but
   * an automatically resolved scale lands on the same `usesXScale` and gets the same treatment.
   */
  it("does not measure a value x-axis, whose labels it does not know", () => {
    const ncols = [{ name: "category", type: "double" }, { name: "revenue", type: "double" }];
    const nrows: unknown[][] = Array.from({ length: 48 }, (_, i) => [i, i]);
    fitCalls.length = 0;
    const o = buildCartesianOption(ncols, nrows, { ...cfg, xScale: "linear" }, { width: 320, height: 260 }) as { grid: { bottom: number } };
    const flat = buildCartesianOption(ncols, nrows, { ...cfg, xScale: "linear" }) as { grid: { bottom: number } };
    expect(fitCalls).toHaveLength(0);
    expect(o.grid.bottom).toBe(flat.grid.bottom);
  });
});
