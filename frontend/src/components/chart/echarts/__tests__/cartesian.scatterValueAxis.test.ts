import { describe, it, expect } from "vitest";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";

/**
 * THE METRIC-VS-METRIC SCATTER'S OWN AXES NOW SPEAK THE FAMILY'S VOCABULARY.
 *
 * `scatterXY` builds its own pair of value axes rather than falling through to `baseOption`, and it
 * was written knowing none of this panel's axis keys. That was recorded as a deliberate divergence
 * — `settings/CartesianDisplaySection.tsx` and the note on the `yAxis` literal itself both said so —
 * and it stopped being defensible the moment the Axes tab started offering those keys anyway.
 *
 * MEASURED LIVE on `MB · Scatter` with an X metric picked, reading the ECharts instance out of the
 * page rather than rebuilding the option: with "Number of tick marks" typed as `3` the input read
 * `3` and the rendered `yAxis.splitNumber` stayed `5` — ECharts' own default, because nothing was
 * written. `Y show lines and tick marks` and `X show lines and tick marks` were on offer beside it
 * and equally inert (`axisTick.show` and `axisLine.show` both came back `"auto"`, the value ECharts
 * uses when an option says nothing).
 *
 * So this was never "a divergence with the controls hidden to match". Three controls were already
 * being offered and thrown away, which is the save-and-do-not-paint defect this programme has closed
 * repeatedly — and the fix has to be in the BUILDER, because withdrawing three live rows from the
 * one scatter flavour that has an X metric would only move the inconsistency.
 *
 * WHAT IS DELIBERATELY NOT HERE: an x-axis SCALE or x-axis BOUNDS. Metabase's keys are
 * `graph.y_axis.scale` / `.min` / `.max` with no x twin, so an x log axis would be a setting with no
 * parity and no control. `xAxisTicks` is different and is fixed below — it is an existing stored key
 * whose control this chart already showed.
 */

const cols = [
  { name: "d", type: "varchar" },
  { name: "x", type: "double" },
  { name: "v", type: "double" },
];
const rows: unknown[][] = [["a", 1, 10], ["b", 2, 20], ["c", 3, 30]];

type Axis = {
  type?: string; min?: number; max?: number; splitNumber?: number;
  axisTick?: { show?: boolean }; axisLine?: { show?: boolean }; scale?: boolean;
};
const build = (extra: Partial<CartesianBuildConfig>) =>
  buildCartesianOption(cols, rows, {
    kind: "scatter", xAxis: "d", yMetrics: ["v"], ...extra,
  } as CartesianBuildConfig) as unknown as { xAxis: Axis | Axis[]; yAxis: Axis | Axis[] };

const one = (a: Axis | Axis[]) => (Array.isArray(a) ? a[0] : a);
/** With an `xMetric` the chart leaves through `scatterXY`; without one it falls through to
 *  `baseOption` like every other cartesian. Both flavours are asserted on every rule below, which is
 *  what stops a fixture that never reaches the arm from looking like coverage. */
const xy = (extra: Partial<CartesianBuildConfig>) => build({ xMetric: "x", ...extra });
const dim = (extra: Partial<CartesianBuildConfig>) => build(extra);

describe("a scatter's value axis honours the Y scale", () => {
  it("builds a log axis on the metric-vs-metric scatter", () => {
    expect(one(xy({ logScale: true }).yAxis).type).toBe("log");
  });

  it("builds a log axis on the dimension-vs-metric scatter too", () => {
    expect(one(dim({ logScale: true }).yAxis).type).toBe("log");
  });

  it("leaves a plain value axis with no scale stored", () => {
    expect(one(xy({}).yAxis).type).toBe("value");
    expect(one(dim({}).yAxis).type).toBe("value");
  });

  /**
   * THE X AXIS STAYS LINEAR, and this is the assertion rather than a comment because the cheap
   * implementation of the rule above is to hand the same `type` to both axes of a chart that happens
   * to have two value axes. `logScale` is `graph.y_axis.scale`; there is no x twin upstream and no
   * control for one here, so an x log axis would be a scale nobody asked for and nobody could undo.
   */
  it("never turns the x axis into a log axis, which has no key and no control", () => {
    expect(one(xy({ logScale: true }).xAxis).type).toBe("value");
  });
});

describe("a scatter's value axis honours an explicit range", () => {
  it("takes both bounds on the metric-vs-metric scatter", () => {
    const y = one(xy({ yMin: 5, yMax: 99 }).yAxis);
    expect(y.min).toBe(5);
    expect(y.max).toBe(99);
  });

  it("takes both bounds on the dimension-vs-metric scatter too", () => {
    const y = one(dim({ yMin: 5, yMax: 99 }).yAxis);
    expect(y.min).toBe(5);
    expect(y.max).toBe(99);
  });

  /** One bound alone is a real state — "Auto y-axis range" off seeds `yMin: 0` and leaves the max
   *  empty — so writing the pair together would pin the far end to `undefined` and lose the fit. */
  it("takes a lower bound on its own without inventing an upper one", () => {
    const y = one(xy({ yMin: 5 }).yAxis);
    expect(y.min).toBe(5);
    expect(y.max).toBeUndefined();
  });

  /**
   * `yMin: 0` is the one bound a truthiness test drops, and it is the pinned axis spelled out by
   * hand — the same argument `unpinFromZeroApplies` makes about the identical pair.
   */
  it("keeps a zero minimum, which a truthiness test would silently drop", () => {
    expect(one(xy({ yMin: 0 }).yAxis).min).toBe(0);
  });

  /**
   * AND THE RANGE OUTRANKS THE UNPIN, which is the interaction that makes the two controls
   * consistent rather than merely both present: a scatter is unpinned by default, so without this
   * the axis would carry `scale: true` under a minimum the user typed. `unpinFromZeroApplies`
   * already refuses on an explicit bound, so the panel hides the switch in the same state — this
   * asserts the builder agrees with the panel it is now paired with.
   */
  it("stops unpinning once a bound is set, as the panel's own gate does", () => {
    expect(one(xy({}).yAxis).scale).toBe(true);
    expect(one(xy({ yMin: 5 }).yAxis).scale).toBe(false);
  });
});

describe("a scatter's axes honour the lines-and-ticks selects the panel already showed", () => {
  it("hides the value axis' line and ticks on the metric-vs-metric scatter", () => {
    const y = one(xy({ yAxisTicks: false }).yAxis);
    expect(y.axisTick?.show).toBe(false);
    expect(y.axisLine?.show).toBe(false);
  });

  it("shows them again with nothing stored", () => {
    const y = one(xy({}).yAxis);
    expect(y.axisTick?.show).toBe(true);
    expect(y.axisLine?.show).toBe(true);
  });

  /**
   * THE CATEGORY-AXIS TWIN, which was inert on this one path and on no other: `xCategory` and
   * `scaleAxis` have both keyed their `axisLine`/`axisTick` on `xAxisTicks` all along, so a
   * metric-vs-metric scatter was the single chart in the family whose "X show lines and tick marks"
   * select did nothing. Found while measuring the Y ones rather than reported, and fixed here rather
   * than left for whoever noticed second.
   */
  it("hides the x axis' line and ticks too, which no other cartesian path ignored", () => {
    const x = one(xy({ xAxisTicks: false }).xAxis);
    expect(x.axisTick?.show).toBe(false);
    expect(x.axisLine?.show).toBe(false);
  });

  it("takes the tick count on the metric-vs-metric scatter", () => {
    expect(one(xy({ yTickCount: 3 }).yAxis).splitNumber).toBe(3);
  });

  it("writes no tick count when none is asked for, leaving ECharts its own default", () => {
    expect(one(xy({}).yAxis).splitNumber).toBeUndefined();
  });

  it("takes the tick count on the dimension-vs-metric scatter too", () => {
    expect(one(dim({ yTickCount: 3 }).yAxis).splitNumber).toBe(3);
  });
});

/**
 * KI-163: the metric-vs-metric scatter's axis TITLES were drawn in ECharts' default pale grey —
 * almost invisible on the plot — while bar, line and combo titles take the axis-label ink. The
 * same ink here, read off a bar chart built from the same rows rather than restated.
 */
describe("a scatter's axis titles are drawn in the family's axis-title colour", () => {
  type Named = { name?: string; nameTextStyle?: { color?: string } };
  const bar = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "d", yMetrics: ["v"] } as CartesianBuildConfig) as unknown as { xAxis: Named | Named[] };
  const ink = (one(bar.xAxis as Axis) as Named).nameTextStyle?.color;

  it("colours both titles of the metric-vs-metric scatter", () => {
    const o = xy({});
    expect(ink).toBeTruthy();
    expect((one(o.xAxis) as Named).nameTextStyle?.color).toBe(ink);
    expect((one(o.yAxis) as Named).nameTextStyle?.color).toBe(ink);
  });
});
