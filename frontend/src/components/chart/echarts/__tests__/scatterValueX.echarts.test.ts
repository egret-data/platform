import { describe, it, expect } from "vitest";
import * as echarts from "echarts/core";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";
import { registerEChartsModules } from "../register";

/**
 * A NUMERIC SCATTER'S X-AXIS: the axis type this builder chooses, and — for the half no option
 * assertion can reach — the labels real ECharts then draws on it.
 *
 * The defect: the parity harness's scatter plots `price`, 196 distinct numbers, and egret handed
 * them to a CATEGORY axis as 196 evenly-spaced categories each carrying its own label. ECharts kept
 * whichever labels did not collide — three arbitrary prices, `15.69 48.93 84.14`, which read as a
 * chart that had picked three rows out of 196 — and once PR 02 made the axis all-or-nothing it drew
 * none at all. Metabase draws `0 20 40 60 80 100` because its x is a VALUE axis, where the labels
 * are ticks computed from the extent rather than a subset of the data.
 *
 * The two halves are tested at the two levels they live at, and the second file-half exists because
 * the first cannot see it: `xAxis.type === "value"` is a fact about the option, while "and therefore
 * the labels come out round" is a fact about ECharts' interval scale. PR 02's worst defect was
 * invisible to every option assertion in the repo for exactly this reason.
 */

/** The harness's shape: a numeric dimension on x, a measure on y. Prices chosen off the real card. */
const priceCols = [{ name: "price", type: "double" }, { name: "rating", type: "double" }];
const priceRows = [[15.69, 3], [48.93, 7], [84.14, 2], [99.5, 1], [0.2, 4]];

/** The same chart over a NOMINAL dimension — the control for every "numeric" assertion below. */
const nameCols = [{ name: "category", type: "varchar" }, { name: "rating", type: "double" }];
const nameRows = [["Doohickey", 3], ["Gadget", 7], ["Gizmo", 2]];

/**
 * The built option, opened up for reading. The RESULT is loose and `cfg` is not, which is the whole
 * point of the helper: an `as never` at the call site, or a `Partial`/untyped fixture spread into
 * it, silences the compiler for the entire argument — `xAxsi: "price"` then compiles, builds a chart
 * with no x column at all, and every assertion below is about a different chart than its name says.
 * Task 2 of this PR shipped that hole twice before a reviewer found the second one. Checked here
 * rather than assumed: misspelling `xAxis` in the first test is a `tsc` error (TS2561).
 */
const build = (cols: { name: string; type: string }[], rows: unknown[][], cfg: CartesianBuildConfig): any =>
  buildCartesianOption(cols, rows, cfg);

describe("resolveXScale — which axis a scatter gets", () => {
  it("puts a scatter over a NUMERIC dimension on a value axis, with its points at real coordinates", () => {
    const opt = build(priceCols, priceRows, { kind: "scatter", xAxis: "price", yMetrics: ["rating"] });
    expect(opt.xAxis.type).toBe("value");
    // `applyXScale` re-keys each point to an item object at [x, y]; sorted ascending by the numeric
    // x. The `name` is the raw category — see the click-payload block below for why it is there and
    // what it fixed; before #387 these were bare `[x, y]` arrays and the points had no name at all.
    expect(opt.series[0].data).toEqual([
      { value: [0.2, 4], name: "0.2" },
      { value: [15.69, 3], name: "15.69" },
      { value: [48.93, 7], name: "48.93" },
      { value: [84.14, 2], name: "84.14" },
      { value: [99.5, 1], name: "99.5" },
    ]);
  });

  it("leaves a scatter over a VARCHAR dimension on the category axis it always had", () => {
    const opt = build(nameCols, nameRows, { kind: "scatter", xAxis: "category", yMetrics: ["rating"] });
    expect(opt.xAxis.type).toBe("category");
    expect(opt.xAxis.data).toEqual(["Doohickey", "Gadget", "Gizmo"]);
    expect(opt.series[0].data).toEqual([3, 7, 2]);
  });

  /**
   * A stored scale is a statement, and the automatic one is only for charts that have not made it.
   * `"ordinal"` is the case that matters: it is the value the panel writes when a user picks the
   * category axis BACK, and treating it as "nothing chosen" would make that choice unclickable.
   *
   * `"timeseries"` USED TO BE IN THIS LIST and was moved out below, because on this fixture it was
   * pinning a blank chart rather than a stored statement — see the test that replaced it.
   */
  it.each(["ordinal", "linear"] as const)("lets an explicit %s scale win", (xScale) => {
    const opt = build(priceCols, priceRows, { kind: "scatter", xAxis: "price", yMetrics: ["rating"], xScale });
    expect(opt.xAxis.type).toBe(xScale === "ordinal" ? "category" : "value");
  });

  /**
   * …BUT ONLY WHERE THE COLUMN CAN CARRY IT. `price` is a `double`, and `applyXScale` reaches a
   * timeseries coordinate through `Date.parse(String(c))` — which is NaN for "15.69", so every one
   * of these five points was dropped and the chart came out empty on a `time` axis with no extent.
   * The assertion above asserted `type === "time"` and never looked at the data, so it passed on
   * exactly that.
   *
   * `resolveXScale` now refuses the pair (`xScaleFitsColumn`), and a refused scale falls through to
   * the automatic answer rather than to `ordinal` outright — so this scatter keeps the value axis
   * this whole file exists to give it. Both halves are asserted, because the axis type alone is
   * what let the blank chart through the first time.
   */
  it("refuses a timeseries scale on a numeric column instead of dropping every point", () => {
    const opt = build(priceCols, priceRows, { kind: "scatter", xAxis: "price", yMetrics: ["rating"], xScale: "timeseries" });
    expect(opt.series[0].data).toHaveLength(priceRows.length);
    expect(opt.xAxis.type).toBe("value");
  });

  /**
   * SCATTER ONLY. A bar chart's categories are discrete even when they are numbers, and bar width,
   * grouping and stacking are all computed off the axis type — putting bars on a value axis is a
   * far larger change than the defect above asks for. The other three vertical kinds are here
   * because the rule is `kind !== "scatter"`, and a rule that only bar was checked against would
   * pass while being written the other way round.
   *
   * `row` IS DIFFERENT AND IS KEPT DELIBERATELY. Mutating `resolveXScale`'s `kind !== "scatter"`
   * guard away fails bar, line, area and combo — and NOT row, because a row chart is caught one
   * layer further down by `usesXScale`'s `!isHorizontal` guard, which is the only thing in the suite
   * pinning that guard for a numeric x. So this case is inert against the rule its name describes
   * and load-bearing for a different one. Do not delete it as redundant, and do not read a pass here
   * as evidence about the scatter rule.
   */
  it.each(["bar", "line", "area", "combo", "row"] as const)("does not change the axis under a numeric %s", (kind) => {
    const opt = build(priceCols, priceRows, { kind, xAxis: "price", yMetrics: ["rating"] });
    // A row chart's category axis is the vertical one; either way it stays a category axis.
    const cat = kind === "row" ? opt.yAxis : opt.xAxis;
    expect(cat.type).toBe("category");
    expect(cat.data).toEqual([0.2, 15.69, 48.93, 84.14, 99.5]);
  });

  /**
   * The metric-vs-metric scatter is the one that never reaches this rule: it leaves through
   * `scatterXY` and builds its own pair of value axes. Asserted so the resolution above cannot be
   * moved up over that early return without something failing — up there it would run for a chart
   * whose axis is already settled, and `xAxis` would be named after the wrong column.
   */
  it("leaves the metric-vs-metric scatter to scatterXY, which already had two value axes", () => {
    const cols = [{ name: "region", type: "varchar" }, { name: "price", type: "double" }, { name: "rating", type: "double" }];
    const opt = build(cols, [["EU", 15.69, 3]], { kind: "scatter", xAxis: "region", xMetric: "price", yMetrics: ["rating"] });
    expect(opt.xAxis.type).toBe("value");
    expect(opt.xAxis.name).toBe("price");
  });

  /** No x column at all: nothing to read a type off, and nothing to resolve. */
  it("does not invent a scale for a scatter with no x column", () => {
    const opt = build(priceCols, priceRows, { kind: "scatter", yMetrics: ["rating"] });
    expect(opt.xAxis.type).toBe("category");
  });
});

/**
 * The round-tick claim, against real ECharts.
 *
 * WHAT THIS PROVES: that the option this builder now emits for a numeric scatter draws round,
 * evenly-spaced x labels — `0 20 40 60 80 100` for the prices above, which is the axis Metabase
 * draws for the same data — and that none of the raw price values appears as a label, because on a
 * value axis they are coordinates rather than labels.
 *
 * WHAT IT DOES NOT PROVE: that any other dataset gets those particular numbers, or that six is the
 * right count. ECharts' `IntervalScale` picks the interval from the extent and the available room;
 * this test pins the behaviour for ONE dataset and the fact that the labels are generated rather
 * than sampled from the data. There is no tick-choosing code in egret to test — selecting the axis
 * type is the whole change, and this is the assertion that the selection has the intended effect.
 * If ECharts is upgraded across a change to that scale, this file is where it surfaces.
 */
describe("what ECharts draws on the axis it was given", () => {
  /** Lay the built option out for real and report the x-axis labels ECharts settled on. */
  function xLabels(cfg: CartesianBuildConfig): string[] {
    registerEChartsModules();
    const chart = echarts.init(null, null, { renderer: "svg", ssr: true, width: 400, height: 300 });
    chart.setOption({ animation: false, ...buildCartesianOption(priceCols, priceRows, cfg) });
    const labels = (chart as unknown as {
      getModel(): { getComponent(t: string): { axis: { getViewLabels(): { formattedLabel: string }[] } } };
    }).getModel().getComponent("xAxis").axis.getViewLabels().map((l) => l.formattedLabel);
    chart.dispose();
    return labels;
  }

  it("draws round ticks across the price range, not the prices themselves", () => {
    expect(xLabels({ kind: "scatter", xAxis: "price", yMetrics: ["rating"] }))
      .toEqual(["0", "20", "40", "60", "80", "100"]);
  });

  /**
   * The same five rows on the axis they used to get, so the test above has something to be read
   * against. This is what the harness card was drawing: one label per row, which at 196 rows on a
   * one-third-width card is why PR 02's all-or-nothing rule answered "none".
   */
  it("draws one label per row on the category axis, which is what it was doing", () => {
    expect(xLabels({ kind: "scatter", xAxis: "price", yMetrics: ["rating"], xScale: "ordinal" }))
      .toEqual(["0.2", "15.69", "48.93", "84.14", "99.5"]);
  });

  /**
   * THE CLICK PAYLOAD THE DRILL IS BUILT ON, pinned against real ECharts instead of only asserted
   * about in a comment.
   *
   * WHAT THIS USED TO RECORD, and why it changed. `applyXScale` emitted bare `[x, y]` arrays, and a
   * value axis has no category to name a point after: `getDataParams` filled `name` from
   * `SeriesData.getName`, which returns `''` when the series has no name list and no `itemName`
   * dimension. So this file pinned `{name: "", data: [48.93, 7]}`, and `ChartRenderer`'s drill had
   * to read `data[0]` on a linear x — which worked there only because a linear coordinate IS the
   * category value, and left a timeseries x with nothing recoverable at all (egretdb/egret#387).
   *
   * `applyXScale` now emits ECharts ITEM OBJECTS — `{value: [x, y], name: String(category)}` — so
   * the raw category survives the re-keying and `getName` has a name list to read. That is the
   * whole fix: `p.name` is the bucket value again on every scale, and the drill has no special case
   * left. This is the assertion that the item object really does reach `getName`, rather than being
   * carried in `data` where only egret can see it.
   *
   * The drill's own test hand-writes this payload into a mocked renderer, so on its own it proves
   * the drill handles the shape, not that the shape is real. If an ECharts upgrade stopped reporting
   * a name here, that fixture would quietly stop describing anything and the drill would go back to
   * filtering on `''` with a green suite. This is the assertion that fails instead.
   */
  function clickParams(cfg: CartesianBuildConfig): { name: string; data: unknown } {
    registerEChartsModules();
    const chart = echarts.init(null, null, { renderer: "svg", ssr: true, width: 400, height: 300 });
    chart.setOption({ animation: false, ...buildCartesianOption(priceCols, priceRows, cfg) });
    // Index 2 of the sorted categories is 48.93 — a point in the middle, so nothing here can pass
    // by accidentally matching an extreme.
    const p = (chart as unknown as {
      getModel(): { getSeriesByIndex(i: number): { getDataParams(i: number): { name: string; data: unknown } } };
    }).getModel().getSeriesByIndex(0).getDataParams(2);
    chart.dispose();
    return { name: p.name, data: p.data };
  }

  it("names a value-axis point after its raw category, which is what the drill filters on", () => {
    expect(clickParams({ kind: "scatter", xAxis: "price", yMetrics: ["rating"] }))
      .toEqual({ name: "48.93", data: { value: [48.93, 7], name: "48.93" } });
  });

  it("still reports the category name and a bare value on the category axis", () => {
    expect(clickParams({ kind: "scatter", xAxis: "price", yMetrics: ["rating"], xScale: "ordinal" }))
      .toEqual({ name: "48.93", data: 7 });
  });

  /**
   * THE TIMESERIES CASE, which is the defect #387 named and the one no `linear` fix could reach.
   *
   * On a time axis `applyXScale` coordinates each point by `Date.parse`'s epoch milliseconds — a
   * number the column has never held — so `data[0]` was never a filterable value however it was
   * read. The item object's `name` is the string the ROW held, so the coordinate and the filter
   * value are now two different things that no longer have to be reconciled.
   *
   * FIVE DAYS, CLICKED IN THE MIDDLE, and the dates are deliberately nothing like their epochs:
   * `1735862400000` shares no leading digits with `2025-01-03`, so an assertion that passed by
   * reading the coordinate could not be mistaken for one reading the name. A single-point series
   * would hide an off-by-one in the datum shape entirely, which is why there are five.
   */
  const dayCols = [{ name: "d", type: "date" }, { name: "rev", type: "double" }];
  const dayRows = [
    ["2025-01-01", 10], ["2025-01-02", 20], ["2025-01-03", 30], ["2025-01-04", 40], ["2025-01-05", 50],
  ];

  function dayClickParams(cfg: CartesianBuildConfig, i: number): { name: string; data: unknown } {
    registerEChartsModules();
    const chart = echarts.init(null, null, { renderer: "svg", ssr: true, width: 400, height: 300 });
    chart.setOption({ animation: false, ...buildCartesianOption(dayCols, dayRows, cfg) });
    const p = (chart as unknown as {
      getModel(): { getSeriesByIndex(i: number): { getDataParams(i: number): { name: string; data: unknown } } };
    }).getModel().getSeriesByIndex(0).getDataParams(i);
    chart.dispose();
    return { name: p.name, data: p.data };
  }

  it("names a TIME-axis point after the date the row held, not after its epoch", () => {
    expect(dayClickParams({ kind: "line", xAxis: "d", yMetrics: ["rev"], xScale: "timeseries" }, 2))
      .toEqual({ name: "2025-01-03", data: { value: [Date.parse("2025-01-03"), 30], name: "2025-01-03" } });
  });

  /** …and the point next to it, so a fixture that reported one fixed name for every point — or an
   *  index that silently landed on the same datum twice — could not pass. */
  it("names each time-axis point after its OWN date", () => {
    expect(dayClickParams({ kind: "line", xAxis: "d", yMetrics: ["rev"], xScale: "timeseries" }, 3).name)
      .toBe("2025-01-04");
  });
});
