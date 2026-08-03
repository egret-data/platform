import { describe, it, expect } from "vitest";
import {
  buildCartesianOption, cartesianSeriesNames, resolveStacked, stackModeOf, stackModePatch,
  shouldAutoSplitY, computeAxisSplit, seriesDisplayFor, bubbleDiameter,
} from "../options/cartesian";
import { CHART_THEME, SERIES_PALETTE } from "../palette";
import { colorsForKeys } from "../keyColors";

const columns = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
const rows = [["EU", 10, 3], ["US", 20, 5]];

describe("buildCartesianOption — base", () => {
  it("one series per y-metric, category axis from x values", () => {
    const opt = buildCartesianOption(columns, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"] }) as any;
    expect(opt.xAxis.type).toBe("category");
    expect(opt.xAxis.data).toEqual(["EU", "US"]);
    expect(opt.series).toHaveLength(2);
    expect(opt.series[0]).toMatchObject({ name: "rev", type: "bar", data: [10, 20] });
    expect(opt.series[1]).toMatchObject({ name: "cnt", type: "bar", data: [3, 5] });
  });
  it("line kind maps series type to line", () => {
    const opt = buildCartesianOption(columns, rows, { kind: "line", xAxis: "region", yMetrics: ["rev"] }) as any;
    expect(opt.series[0].type).toBe("line");
  });
  it("a11y: line series cycle dash style + marker shape (not colour alone), and aria is enabled", () => {
    const opt = buildCartesianOption(columns, rows, { kind: "line", xAxis: "region", yMetrics: ["rev", "cnt"] }) as any;
    // Screen-reader data description stays on; the decal patterns are off since
    // chart 05 — the dash/marker cycling below is the colour-independent affordance
    // that does not cover the fill.
    expect(opt.aria).toMatchObject({ enabled: true, decal: { show: false } });
    // First two series get DISTINCT dash types + marker shapes so colour-blind
    // users can tell them apart without relying on the palette colour.
    expect(opt.series[0].lineStyle.type).toBe("solid");
    expect(opt.series[1].lineStyle.type).toBe("dashed");
    expect(opt.series[0].symbol).not.toBe(opt.series[1].symbol);
  });
  it("returns empty series when x or metrics missing", () => {
    const opt = buildCartesianOption(columns, rows, { kind: "bar", yMetrics: [] }) as any;
    expect(opt.series).toEqual([]);
  });
});

describe("buildCartesianOption — breakout & variants", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "yr", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["EU", "2023", 1], ["EU", "2024", 2], ["US", "2023", 3]];

  it("series breakout: one series per distinct series value, null for missing cells", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "yr" }) as any;
    expect(opt.xAxis.data).toEqual(["EU", "US"]);
    const names = opt.series.map((s: any) => s.name).sort();
    expect(names).toEqual(["2023", "2024"]);
    const s2024 = opt.series.find((s: any) => s.name === "2024");
    expect(s2024.data).toEqual([2, null]); // EU=2, US missing
  });
  it("stacked sets stack:total on bar series", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "yr", stacked: true }) as any;
    expect(opt.series.every((s: any) => s.stack === "total")).toBe(true);
  });
  it("horizontal swaps category onto the y-axis", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], horizontal: true }) as any;
    expect(opt.yAxis.type).toBe("category");
    expect(opt.xAxis.type).toBe("value");
  });
  it("area kind adds areaStyle; scatter uses scatter type", () => {
    const area = buildCartesianOption(cols, rows, { kind: "area", xAxis: "region", yMetrics: ["rev"] }) as any;
    expect(area.series[0].type).toBe("line");
    expect(area.series[0].areaStyle).toBeTruthy();
    const sc = buildCartesianOption(cols, rows, { kind: "scatter", xAxis: "region", yMetrics: ["rev"] }) as any;
    expect(sc.series[0].type).toBe("scatter");
  });
  it("combo uses per-metric seriesTypes", () => {
    const cols2 = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
    const rows2 = [["EU", 1, 2]];
    const opt = buildCartesianOption(cols2, rows2, { kind: "combo", xAxis: "region", yMetrics: ["rev", "cnt"], seriesTypes: { rev: "bar", cnt: "line" } }) as any;
    expect(opt.series.map((s: any) => s.type)).toEqual(["bar", "line"]);
  });
  it("null / empty breakout values are labelled (empty) — Metabase parity", () => {
    const cols = [{ name: "region", type: "varchar" }, { name: "seg", type: "varchar" }, { name: "rev", type: "double" }];
    const rows = [["EU", null, 1], ["EU", "A", 2], ["US", "", 3]];
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "seg" }) as any;
    const names = opt.series.map((s: any) => s.name);
    expect(names).toContain("(empty)");
    expect(names).not.toContain("null");
  });
});

// Behaviors ported from Metabase's cartesian model/option unit specs
// (visualizations/echarts/cartesian/{model,option}/*.unit.spec.ts), adapted to
// egret's single-pass buildCartesianOption. Covers the styling/axis/legend
// surface those suites assert on the produced ECharts option.
describe("buildCartesianOption — Metabase-parity styling & axes", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
  const rows = [["EU", 10, 3], ["US", 20, 5]];
  const base = { kind: "bar" as const, xAxis: "region", yMetrics: ["rev"] };

  it("dedupes x categories, preserving first-seen order", () => {
    const r = [["EU", 1, 1], ["US", 2, 2], ["EU", 3, 3]];
    const opt = buildCartesianOption(cols, r, base) as any;
    expect(opt.xAxis.data).toEqual(["EU", "US"]);
  });
  it("takes each series' colour from its NAME, not its position (Metabase parity)", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, yMetrics: ["rev", "cnt"] }) as any;
    const byName = colorsForKeys(["rev", "cnt"]);
    expect(opt.series[0].itemStyle.color).toBe(byName.rev);
    expect(opt.series[1].itemStyle.color).toBe(byName.cnt);
    // Distinct, and NOT the first two palette slots — which is what a positional rule would give.
    expect(byName.rev).not.toBe(byName.cnt);
    expect(opt.series[0].itemStyle.color).not.toBe(SERIES_PALETTE[0]);
  });
  it("gives a lone series its hashed colour too — there is no lone-series rule", () => {
    // This asserted `SUMMARIZE_COLOR` and called it Metabase parity. Measured on identical data,
    // Metabase draws a single-metric card in the colour its column NAME hashes to; the green is
    // its `success` colour, reserved for names like "profit". A chart's colours must not depend on
    // how many series happen to be on it, or the same column changes colour when another is added.
    const opt = buildCartesianOption(cols, rows, base) as any; // single metric
    expect(opt.series[0].itemStyle.color).toBe(colorsForKeys(["rev"]).rev);
    // …and it is the same ink the two-series card above gave it.
    expect(opt.series[0].itemStyle.color).toBe(colorsForKeys(["rev", "cnt"]).rev);
  });
  it("bar series carry barWidth", () => {
    const opt = buildCartesianOption(cols, rows, base) as any;
    expect(opt.series[0].barWidth).toBe(CHART_THEME.barWidth);
  });
  it("grouped (non-stacked, >1 bar) series drop the fixed barWidth so they sit side by side", () => {
    const c = [{ name: "p", type: "varchar" }, { name: "s", type: "varchar" }, { name: "rev", type: "double" }];
    const r = [["A", "X", 1], ["A", "Y", 2], ["B", "X", 3]];
    const breakout = buildCartesianOption(c, r, { kind: "bar", xAxis: "p", yMetrics: ["rev"], series: "s" }) as any;
    expect(breakout.series[0].barWidth).toBeUndefined();
    const multi = buildCartesianOption(cols, rows, { ...base, yMetrics: ["rev", "cnt"] }) as any;
    expect(multi.series[0].barWidth).toBeUndefined();
    // stacked bars stack (don't group) → keep the fixed width
    const stacked = buildCartesianOption(c, r, { kind: "bar", xAxis: "p", yMetrics: ["rev"], series: "s", stacked: true }) as any;
    expect(stacked.series[0].barWidth).toBe(CHART_THEME.barWidth);
  });
  it("line series are smooth when configured and carry a lineStyle width", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, kind: "line", lineShape: "smooth" }) as any;
    expect(opt.series[0].smooth).toBe(true);
    expect(opt.series[0].lineStyle.width).toBe(CHART_THEME.lineWidth);
  });
  it("scatter uses item tooltip + reduced opacity; other kinds use axis trigger", () => {
    const sc = buildCartesianOption(cols, rows, { ...base, kind: "scatter" }) as any;
    expect(sc.tooltip.trigger).toBe("item");
    expect(sc.series[0].itemStyle.opacity).toBe(CHART_THEME.scatterOpacity);
    expect((buildCartesianOption(cols, rows, base) as any).tooltip.trigger).toBe("axis");
  });
  it("axis titles + nameGap come from xLabel / yLabel", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, xLabel: "Region", yLabel: "Revenue" }) as any;
    expect(opt.xAxis.name).toBe("Region");
    expect(opt.yAxis.name).toBe("Revenue");
    expect(opt.xAxis.nameGap).toBe(28);
  });
  it("grid reserves room for labels (containLabel)", () => {
    expect((buildCartesianOption(cols, rows, base) as any).grid.containLabel).toBe(true);
  });
  it("reserves top space for the legend (Metabase puts it on top)", () => {
    const noLegend = (buildCartesianOption(cols, rows, base) as any).grid.top;
    const withLegend = (buildCartesianOption(cols, rows, { ...base, showLegend: true }) as any) as any;
    expect(withLegend.grid.top).toBeGreaterThan(noLegend);
    expect(withLegend.legend.top).toBe(0);
  });
  it("value axis formats labels via column type / settings", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, metricTypes: { rev: "double" }, columnSettings: { rev: { number_style: "currency", currency: "USD", decimals: 0 } } }) as any;
    expect(opt.yAxis.axisLabel.formatter(1000)).toBe("$1,000");
  });
  it("showLegend toggles the legend", () => {
    expect((buildCartesianOption(cols, rows, { ...base, showLegend: true }) as any).legend).toMatchObject({ type: "scroll" });
    expect((buildCartesianOption(cols, rows, base) as any).legend).toBeUndefined();
  });
  it("showLabels adds a data-label formatter that formats the value", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, showLabels: true, metricTypes: { rev: "double" }, columnSettings: { rev: { prefix: "$", decimals: 0 } } }) as any;
    expect(opt.series[0].label.show).toBe(true);
    expect(opt.series[0].label.formatter({ value: 10 })).toBe("$10");
  });
  it("showLabels also hides overlapping data labels, since a card's width has no room for all of them", () => {
    const withLabels = buildCartesianOption(cols, rows, { ...base, showLabels: true }) as any;
    expect(withLabels.series[0].labelLayout).toEqual({ hideOverlap: true });
    const without = buildCartesianOption(cols, rows, base) as any;
    expect(without.series[0].labelLayout).toBeUndefined();
  });
});

describe("buildCartesianOption — metric-vs-metric scatter (Metabase parity)", () => {
  const cols = [{ name: "d", type: "varchar" }, { name: "x", type: "double" }, { name: "y", type: "double" }];
  const rows = [["A", 1, 10], ["B", 2, 20]];
  it("plots (xMetric, yMetric) points on two value axes", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "scatter", xAxis: "d", xMetric: "x", yMetrics: ["y"] }) as any;
    expect(opt.xAxis.type).toBe("value");
    expect(opt.yAxis.type).toBe("value");
    expect(opt.series[0].type).toBe("scatter");
    expect(opt.series[0].data.map((d: any) => ({ value: d.value, name: d.name })))
      .toEqual([{ value: [1, 10], name: "A" }, { value: [2, 20], name: "B" }]);
  });
  it("falls back to a category scatter without xMetric", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "scatter", xAxis: "d", yMetrics: ["y"] }) as any;
    expect(opt.xAxis.type).toBe("category");
  });
});

// Phase 1c — cartesian data shaping, clean-room from Metabase's cartesian/model/dataset.ts
// (sort dimension by axis type; drop null-dimension rows).
describe("buildCartesianOption — data shaping (Metabase parity 1c)", () => {
  it("sorts a numeric x-axis ascending (not first-seen)", () => {
    const cols = [{ name: "yr", type: "int" }, { name: "rev", type: "double" }];
    const rows = [["2021", 3], ["2019", 1], ["2020", 2]];
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "yr", yMetrics: ["rev"] }) as any;
    expect(opt.xAxis.data).toEqual(["2019", "2020", "2021"]);
    expect(opt.series[0].data).toEqual([1, 2, 3]);
  });
  it("sorts a temporal x-axis chronologically", () => {
    const cols = [{ name: "d", type: "date" }, { name: "rev", type: "double" }];
    const rows = [["2024-03-01", 3], ["2024-01-01", 1], ["2024-02-01", 2]];
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "d", yMetrics: ["rev"] }) as any;
    expect(opt.xAxis.data).toEqual(["2024-01-01", "2024-02-01", "2024-03-01"]);
    expect(opt.series[0].data).toEqual([1, 2, 3]);
  });
  // This used to assert first-seen order, `["Med", "High", "Low"]`, citing Metabase's `sortDataset`
  // for it. First-seen order is the DEFECT: egret's stored specs carry `orderby: []`, so it is the
  // engine's order and it changes between calls. See the dedicated suite at the end of this file.
  it("sorts a categorical (string) x-axis ascending, not by arrival", () => {
    const cols = [{ name: "p", type: "varchar" }, { name: "rev", type: "double" }];
    const rows = [["Med", 2], ["High", 3], ["Low", 1]];
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "p", yMetrics: ["rev"] }) as any;
    expect(opt.xAxis.data).toEqual(["High", "Low", "Med"]);
    expect(opt.series[0].data).toEqual([3, 1, 2]);
  });
  it("drops rows whose x value is null", () => {
    const cols = [{ name: "yr", type: "int" }, { name: "rev", type: "double" }];
    const rows = [["2020", 2], [null, 5], ["2019", 1]];
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "yr", yMetrics: ["rev"] }) as any;
    expect(opt.xAxis.data).toEqual(["2019", "2020"]);
    expect(opt.series[0].data).toEqual([1, 2]);
  });
});

// Phase 1d — dual Y-axis + log scale, clean-room from Metabase's cartesian axis model
// (leftAxisModel/rightAxisModel, yAxisIndex per series, log scale).
describe("buildCartesianOption — dual axis & scale (Metabase parity 1d)", () => {
  const cols = [{ name: "x", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
  const rows = [["A", 100, 3], ["B", 200, 5]];

  it("puts right-axis metrics on yAxisIndex 1 and builds two value axes", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "combo", xAxis: "x", yMetrics: ["rev", "cnt"], rightAxisMetrics: ["cnt"], seriesTypes: { rev: "bar", cnt: "line" } }) as any;
    expect(Array.isArray(opt.yAxis)).toBe(true);
    expect(opt.yAxis).toHaveLength(2);
    expect(opt.yAxis[1].position).toBe("right");
    expect(opt.series.find((s: any) => s.name === "cnt").yAxisIndex).toBe(1);
    expect(opt.series.find((s: any) => s.name === "rev").yAxisIndex ?? 0).toBe(0);
  });
  // `cnt` spans 2 against `rev`'s 100 — 1% of the chart's range, well under Metabase's 5% rule, so
  // sharing one axis would flatten it to a line on the floor. The split is now computed.
  it("splits the axis itself when one metric would be invisible beside the other", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev", "cnt"] }) as any;
    expect(Array.isArray(opt.yAxis)).toBe(true);
    expect(opt.series.find((s: any) => s.name === "cnt").yAxisIndex).toBe(1);
  });

  it("stays single-axis when the split is turned off", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev", "cnt"], autoSplitY: false }) as any;
    expect(Array.isArray(opt.yAxis)).toBe(false);
  });
  it("stays single-axis when every metric is on the right", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev"], rightAxisMetrics: ["rev"] }) as any;
    expect(Array.isArray(opt.yAxis)).toBe(false);
  });
  it("log scale sets the value-axis type to log", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev"], logScale: true }) as any;
    expect(opt.yAxis.type).toBe("log");
  });
  it("log scale applies to the value axis even when horizontal", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev"], horizontal: true, logScale: true }) as any;
    expect(opt.xAxis.type).toBe("log");
    expect(opt.yAxis.type).toBe("category");
  });
  it("the right axis formats labels with its own metric's settings", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "combo", xAxis: "x", yMetrics: ["rev", "cnt"], rightAxisMetrics: ["cnt"], metricTypes: { rev: "double", cnt: "bigint" }, columnSettings: { cnt: { suffix: " orders" } } }) as any;
    expect(opt.yAxis[1].axisLabel.formatter(5)).toBe("5 orders");
  });
});

// Phase 1e — overlays: goal line + 100% normalized stacking, clean-room from
// Metabase's cartesian option/goal-line.ts + model/stack.ts (normalized stack type).
describe("buildCartesianOption — overlays (Metabase parity 1e)", () => {
  const cols = [{ name: "x", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
  const rows = [["A", 30, 1], ["B", 70, 3]];

  it("goalValue adds a markLine on the first series with the goal label", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev"], goalValue: 50, goalLabel: "Target" }) as any;
    const ml = opt.series[0].markLine;
    expect(ml.data[0].yAxis).toBe(50);
    expect(ml.label.formatter).toBe("Target");
  });
  it("no markLine without a goalValue", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev"] }) as any;
    expect(opt.series[0].markLine).toBeUndefined();
  });
  it("normalized stacking turns each category's values into percentages summing to 100", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev", "cnt"], stacked: true, normalized: true }) as any;
    const a0 = opt.series[0].data[0], a1 = opt.series[1].data[0];
    expect(a0).toBeCloseTo((30 / 31) * 100, 4);
    expect(Math.round(a0 + a1)).toBe(100);
  });
  it("normalized only applies when stacked", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev", "cnt"], normalized: true }) as any;
    expect(opt.series[0].data[0]).toBe(30);
  });
  it("normalized value axis formats as percent", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev", "cnt"], stacked: true, normalized: true }) as any;
    expect(opt.yAxis.axisLabel.formatter(50)).toBe("50%");
  });
});

// Phase 2a — trend line + series polish, clean-room from Metabase's cartesian
// model/trend-line.ts (least-squares) + option/series.ts (barMinHeight, missing/lineShape).
describe("buildCartesianOption — series polish & trend (Metabase parity 2a)", () => {
  const cols = [{ name: "x", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["A", 10], ["B", 20], ["C", 30]];

  it("bar series get a minimum height so tiny bars stay visible", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev"] }) as any;
    expect(opt.series[0].barMinHeight).toBeGreaterThan(0);
  });
  it("missing and lineShape drive connectNulls and step on the series", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "x", yMetrics: ["rev"], missing: "none", lineShape: "step" }) as any;
    expect(opt.series[0].connectNulls).toBe(false);
    expect(opt.series[0].step).toBe("middle");
  });
  it("lines connect across nulls by default (Metabase parity)", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "x", yMetrics: ["rev"] }) as any;
    expect(opt.series[0].connectNulls).toBe(true);
  });
  it("trendLine appends a dashed least-squares trend series per metric", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "x", yMetrics: ["rev"], trendLine: true }) as any;
    expect(opt.series).toHaveLength(2);
    expect(opt.series[1].lineStyle.type).toBe("dashed");
    expect(opt.series[1].data).toEqual([10, 20, 30]); // perfectly linear → fit equals data
  });
  it("trend series are excluded from the legend", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "x", yMetrics: ["rev"], trendLine: true, showLegend: true }) as any;
    expect(opt.legend.data).toEqual(["rev"]);
  });
  it("no trend series without trendLine", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "x", yMetrics: ["rev"] }) as any;
    expect(opt.series).toHaveLength(1);
  });
});

// Phase 3a — Row chart, clean-room from Metabase's RowChart (horizontal bars,
// categories sorted by value so the largest sits on top).
describe("buildCartesianOption — row chart (Metabase parity 3a)", () => {
  const cols = [{ name: "p", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["A", 10], ["B", 30], ["C", 20]];

  it("renders horizontal bars with categories on the y-axis", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "row", xAxis: "p", yMetrics: ["rev"] }) as any;
    expect(opt.yAxis.type).toBe("category");
    expect(opt.xAxis.type).toBe("value");
    expect(opt.series[0].type).toBe("bar");
  });
  it("keeps the dimension order (Metabase doesn't value-sort rows)", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "row", xAxis: "p", yMetrics: ["rev"] }) as any;
    expect(opt.yAxis.data).toEqual(["A", "B", "C"]);
    expect(opt.series[0].data).toEqual([10, 30, 20]);
  });
  it("places data labels at the bar end when showLabels", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "row", xAxis: "p", yMetrics: ["rev"], showLabels: true }) as any;
    expect(opt.series[0].label.position).toBe("right");
  });
});

// Phase 2b — formatted tooltip, clean-room from Metabase's echarts/tooltip
// (per-series formatted rows, trend/base series excluded).
describe("buildCartesianOption — formatted tooltip (Metabase parity 2b)", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
  const rows = [["EU", 10, 3], ["US", 20, 5]];

  it("formats each series value with its metric settings", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], metricTypes: { rev: "double" }, columnSettings: { rev: { prefix: "$", decimals: 0 } } }) as any;
    const html = opt.tooltip.formatter([{ axisValueLabel: "EU", seriesName: "rev", value: 10, marker: "" }]);
    expect(html).toContain("EU");
    expect(html).toContain("$10");
  });
  it("uses the right metric's format for each series (multi-metric)", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"], metricTypes: { rev: "double", cnt: "bigint" }, columnSettings: { cnt: { suffix: " orders" } } }) as any;
    const html = opt.tooltip.formatter([
      { axisValueLabel: "EU", seriesName: "rev", value: 10, marker: "" },
      { axisValueLabel: "EU", seriesName: "cnt", value: 3, marker: "" },
    ]);
    expect(html).toContain("3 orders");
  });
  it("excludes trend series from the tooltip", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "region", yMetrics: ["rev"], trendLine: true }) as any;
    const html = opt.tooltip.formatter([
      { axisValueLabel: "EU", seriesName: "rev", value: 10, marker: "" },
      { axisValueLabel: "EU", seriesName: "rev trend", value: 9, marker: "" },
    ]);
    expect(html).not.toContain("trend");
  });
  it("formats normalized values as percent", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"], stacked: true, normalized: true }) as any;
    const html = opt.tooltip.formatter([{ axisValueLabel: "EU", seriesName: "rev", value: 76.9, marker: "" }]);
    expect(html).toContain("77%");
  });
});

// Phase 2a-pt2 — stack-total labels, clean-room from Metabase's getStackTotalsSeries.
describe("buildCartesianOption — stack totals (Metabase parity 2a-pt2)", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
  const rows = [["EU", 10, 3], ["US", 20, 5]];

  it("showStackTotal adds a transparent total-label series summing the stack", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"], stacked: true, showStackTotal: true }) as any;
    const total = opt.series.find((s: any) => s.__total);
    expect(total).toBeTruthy();
    expect(total.itemStyle.color).toBe("transparent");
    expect(total.data).toEqual([13, 25]); // EU 10+3, US 20+5
    expect(total.label.show).toBe(true);
  });
  it("no total series unless stacked + showStackTotal", () => {
    expect((buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"], showStackTotal: true }) as any).series.some((s: any) => s.__total)).toBe(false);
    expect((buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"], stacked: true }) as any).series.some((s: any) => s.__total)).toBe(false);
  });
  it("the total series is excluded from the legend", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"], stacked: true, showStackTotal: true, showLegend: true }) as any;
    expect(opt.legend.data).toEqual(["rev", "cnt"]);
  });
});

describe("buildCartesianOption — per-series color (Metabase parity)", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "yr", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
  const rows = [["EU", "2023", 1, 9], ["US", "2024", 2, 8]];

  it("overrides a metric series colour by series name", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"], seriesColors: { cnt: "#ff0000" } }) as any;
    expect(opt.series.find((s: any) => s.name === "cnt").itemStyle.color).toBe("#ff0000");
    // The untouched series keeps the colour its own name hashes to.
    expect(opt.series.find((s: any) => s.name === "rev").itemStyle.color)
      .toBe(colorsForKeys(["rev", "cnt"], { cnt: "#ff0000" }).rev);
  });
  it("an explicit colour beats the one a lone series' name hashes to", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], seriesColors: { rev: "#123456" } }) as any;
    expect(opt.series[0].itemStyle.color).toBe("#123456");
  });
  it("overrides a breakout series colour by its value", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "yr", seriesColors: { "2024": "#00ff00" } }) as any;
    expect(opt.series.find((s: any) => s.name === "2024").itemStyle.color).toBe("#00ff00");
  });
});

describe("buildCartesianOption — manual Y-axis range (Metabase parity)", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["EU", 10], ["US", 20]];
  it("applies yMin/yMax to the value axis", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], yMin: 0, yMax: 100 }) as any;
    expect(opt.yAxis.min).toBe(0);
    expect(opt.yAxis.max).toBe(100);
  });
  it("applies the range to the value (x) axis when horizontal", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"], horizontal: true, yMin: 5, yMax: 50 }) as any;
    expect(opt.xAxis.min).toBe(5);
    expect(opt.xAxis.max).toBe(50);
  });
  it("leaves axis auto-ranged when unset", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev"] }) as any;
    expect(opt.yAxis.min).toBeUndefined();
    expect(opt.yAxis.max).toBeUndefined();
  });
});

describe("buildCartesianOption — series rename (Metabase parity)", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
  const rows = [["EU", 10, 3], ["US", 20, 5]];
  it("renames the series shown in legend while keeping per-metric format", () => {
    const opt = buildCartesianOption(cols, rows, {
      kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"], showLegend: true,
      seriesLabels: { rev: "Revenue" },
      metricTypes: { rev: "double", cnt: "bigint" },
      columnSettings: { rev: { prefix: "$", decimals: 0 } },
    }) as any;
    expect(opt.series[0].name).toBe("Revenue");
    expect(opt.series[1].name).toBe("cnt");  // un-renamed series keep their metric name
    // tooltip still formats the renamed series with rev's settings ($)
    const tip = opt.tooltip.formatter([{ axisValueLabel: "EU", seriesName: "Revenue", value: 10, marker: "" }]);
    expect(tip).toContain("$10");
  });
});

describe("buildCartesianOption — point markers (Metabase parity)", () => {
  const cols = [{ name: "m", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["Jan", 10], ["Feb", 20]];
  it("shows symbols by default and hides them when markers=off", () => {
    const shown = buildCartesianOption(cols, rows, { kind: "line", xAxis: "m", yMetrics: ["rev"] }) as any;
    expect(shown.series[0].showSymbol).toBe(true);
    const hidden = buildCartesianOption(cols, rows, { kind: "line", xAxis: "m", yMetrics: ["rev"], markers: "off" }) as any;
    expect(hidden.series[0].showSymbol).toBe(false);
  });
});

describe("buildCartesianOption — x-axis scale (Metabase parity)", () => {
  const cols = [{ name: "x", type: "double" }, { name: "rev", type: "double" }];
  const rows = [["1", 10], ["3", 30], ["2", 20]];
  it("ordinal (default) keeps a category axis with aligned arrays", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "x", yMetrics: ["rev"] }) as any;
    expect(opt.xAxis.type).toBe("category");
    expect(opt.series[0].data).toEqual([10, 20, 30]); // sorted numeric categories
  });
  /** The pairs are wrapped in ECharts item objects since #387, so a clicked point still knows which
   *  category it came from — `applyXScale` says why, and `scatterValueX.echarts.test.ts` pins what
   *  ECharts then reports for one. The COORDINATE is unchanged, which is what these two assert. */
  it("linear scale uses a value axis with [x,y] pairs", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "x", yMetrics: ["rev"], xScale: "linear" }) as any;
    expect(opt.xAxis.type).toBe("value");
    expect(opt.series[0].data).toEqual([
      { value: [1, 10], name: "1" }, { value: [2, 20], name: "2" }, { value: [3, 30], name: "3" },
    ]);
  });
  /** On a TIME axis the coordinate and the name are different things — the epoch positions the
   *  point, the date string is what a drill can filter on — so this is the case where reading one
   *  for the other cannot go unnoticed. */
  it("timeseries scale uses a time axis with parsed-timestamp pairs", () => {
    const tcols = [{ name: "d", type: "date" }, { name: "rev", type: "double" }];
    const trows = [["2024-01-01", 10], ["2024-01-03", 30]];
    const opt = buildCartesianOption(tcols, trows, { kind: "line", xAxis: "d", yMetrics: ["rev"], xScale: "timeseries" }) as any;
    expect(opt.xAxis.type).toBe("time");
    expect(opt.series[0].data).toEqual([
      { value: [Date.parse("2024-01-01"), 10], name: "2024-01-01" },
      { value: [Date.parse("2024-01-03"), 30], name: "2024-01-03" },
    ]);
  });
  it("tooltip reads y from [x,y] pairs on a scaled axis", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "line", xAxis: "x", yMetrics: ["rev"], xScale: "linear", metricTypes: { rev: "double" }, columnSettings: { rev: { prefix: "$" } } }) as any;
    const tip = opt.tooltip.formatter([{ axisValueLabel: "2", seriesName: "rev", value: [2, 20], marker: "" }]);
    expect(tip).toContain("$20");
  });
});

describe("cartesianSeriesNames helper", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "yr", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "bigint" }];
  const rows = [["EU", "2023", 1, 9], ["US", "", 2, 8]];
  it("returns the y-metrics when there is no breakout", () => {
    expect(cartesianSeriesNames(cols, rows, { yMetrics: ["rev", "cnt"] })).toEqual(["rev", "cnt"]);
  });
  it("returns distinct breakout values, mapping null/empty to (empty)", () => {
    expect(cartesianSeriesNames(cols, rows, { yMetrics: ["rev"], series: "yr" })).toEqual(["2023", "(empty)"]);
  });
});

describe("category axis labels are formatted", () => {
  // A chart grouped by month put the raw bucket on the axis — "2026-01-01" for what is really
  // January 2026. Only the metric (value) axis went through `formatValue`.
  const base = {
    yMetrics: ["rev"], xAxis: "d", kind: "bar" as const,
    metricTypes: { rev: "bigint" },
  };
  const axisOf = (opt: Record<string, unknown>) =>
    (Array.isArray(opt.xAxis) ? opt.xAxis[0] : opt.xAxis) as { axisLabel?: { formatter?: (v: string) => string } };

  it("applies the x column's date format to its labels", () => {
    const opt = buildCartesianOption(
      [{ name: "d", type: "date" }, { name: "rev", type: "bigint" }],
      [["2026-01-01", 10]],
      { ...base, xAxisType: "date", columnSettings: { d: { date_style: "MMMM YYYY" } } },
    ) as Record<string, unknown>;
    expect(axisOf(opt).axisLabel?.formatter?.("2026-01-01")).toBe("January 2026");
  });

  it("leaves labels alone when the column has no format", () => {
    const opt = buildCartesianOption(
      [{ name: "d", type: "date" }, { name: "rev", type: "bigint" }],
      [["2026-01-01", 10]],
      { ...base, xAxisType: "date" },
    ) as Record<string, unknown>;
    expect(axisOf(opt).axisLabel?.formatter?.("2026-01-01")).toBe("2026-01-01");
  });
});

describe("breakout series names", () => {
  const cols = [
    { name: "region", type: "varchar" },
    { name: "order_date", type: "date" },
    { name: "rev", type: "double" },
  ];
  const rows = [["EU", "2026-01-01", 10], ["EU", "2026-02-01", 4]];

  // The panel offered a date style for the breakout column and stored it; the legend drew the raw
  // bucket regardless — the same offered-stored-ignored pair the pivot and the pie each had.
  it("reads a breakout dimension through its column format", () => {
    const opt = buildCartesianOption(cols, rows, {
      kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "order_date",
      seriesType: "date", columnSettings: { order_date: { date_style: "MMMM YYYY" } },
    }) as any;
    expect(opt.series.map((s: any) => s.name)).toEqual(["January 2026", "February 2026"]);
    // …while the raw value rides along, because that is what a drill has to filter by.
    expect(opt.series.map((s: any) => s.__raw)).toEqual(["2026-01-01", "2026-02-01"]);
  });

  it("lets an explicit rename win over the column format", () => {
    const opt = buildCartesianOption(cols, rows, {
      kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "order_date",
      seriesType: "date", columnSettings: { order_date: { date_style: "MMMM YYYY" } },
      seriesLabels: { "2026-01-01": "Kickoff" },
    }) as any;
    expect(opt.series.map((s: any) => s.name)).toEqual(["Kickoff", "February 2026"]);
    expect(opt.series[0].__raw).toBe("2026-01-01");
  });

  it("leaves an extraction unit's finished word alone", () => {
    const opt = buildCartesianOption(
      [{ name: "region", type: "varchar" }, { name: "dow", type: "date" }, { name: "rev", type: "double" }],
      [["EU", "Wed", 10]],
      {
        kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "dow",
        seriesUnit: "day_of_week", seriesType: "date",
        columnSettings: { dow: { date_style: "MMMM YYYY" } },
      },
    ) as any;
    expect(opt.series[0].name).toBe("Wed");
  });
});

describe("axis lines, ticks and tick count", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
  const rows = [["EU", 10], ["US", 20]];
  const base = { kind: "bar" as const, xAxis: "region", yMetrics: ["rev"] };

  // Unset means shown — every chart saved before these settings existed must keep both axes drawn.
  it("draws lines and ticks on both axes by default", () => {
    const opt = buildCartesianOption(cols, rows, base) as any;
    expect(opt.xAxis.axisLine.show).toBe(true);
    expect(opt.xAxis.axisTick.show).toBe(true);
    expect(opt.yAxis.axisLine.show).toBe(true);
    expect(opt.yAxis.axisTick.show).toBe(true);
    expect(opt.yAxis.splitNumber).toBeUndefined();
  });

  it("hides each axis' line and ticks independently", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, xAxisTicks: false }) as any;
    expect(opt.xAxis.axisLine.show).toBe(false);
    expect(opt.xAxis.axisTick.show).toBe(false);
    expect(opt.yAxis.axisLine.show).toBe(true);
  });

  it("passes the requested tick count to the value axis", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, yTickCount: 5 }) as any;
    expect(opt.yAxis.splitNumber).toBe(5);
  });

  // Horizontal swaps which ECharts axis is the value one, so the toggles have to follow the
  // ROLE (category vs value), not the axis key.
  it("keeps the toggles attached to their axis role when horizontal", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, horizontal: true, yTickCount: 4 }) as any;
    expect(opt.xAxis.splitNumber).toBe(4);
  });
});

describe("stacking defaults and the three-state mode", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "double" }];
  const rows = [["EU", 10, 3], ["US", 20, 5]];

  // Overlapping translucent areas at 0.3 opacity read as mud, which is why Metabase's area chart
  // stacks unless told otherwise. Bar and line do not.
  it("stacks an area by default and leaves bar and line unstacked", () => {
    expect(resolveStacked({ kind: "area" })).toBe(true);
    expect(resolveStacked({ kind: "bar" })).toBe(false);
    expect(resolveStacked({ kind: "line" })).toBe(false);
  });

  // `undefined` (control never touched) and `false` (explicitly "Don't stack") must stay distinct:
  // `cartesianToSettings` drops undefined keys, so the difference is what a saved chart carries.
  it("keeps an explicit Don't stack on an area", () => {
    expect(resolveStacked({ kind: "area", stacked: false })).toBe(false);
    expect(resolveStacked({ kind: "bar", stacked: true })).toBe(true);
  });

  it("reaches the drawn series, not just the helper", () => {
    const area = buildCartesianOption(cols, rows, { kind: "area", xAxis: "region", yMetrics: ["rev", "cnt"] }) as any;
    expect(area.series.map((s: any) => s.stack)).toEqual(["total", "total"]);
    const bar = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"] }) as any;
    expect(bar.series.every((s: any) => s.stack === undefined)).toBe(true);
    const off = buildCartesianOption(cols, rows, { kind: "area", xAxis: "region", yMetrics: ["rev", "cnt"], stacked: false }) as any;
    expect(off.series.every((s: any) => s.stack === undefined)).toBe(true);
  });

  it("reads the three states as one value", () => {
    expect(stackModeOf({ kind: "area" })).toBe("stacked");
    expect(stackModeOf({ kind: "bar" })).toBe("none");
    expect(stackModeOf({ kind: "bar", stacked: true, normalized: true })).toBe("normalized");
    // `normalized` alone is not a state: without stacking there is nothing to normalise.
    expect(stackModeOf({ kind: "bar", normalized: true })).toBe("none");
  });

  // Two booleans encode three states, so the pair must be written together — leaving `normalized`
  // behind when choosing "Don't stack" stored a combination no control could show.
  it("clears normalized when the mode leaves it behind", () => {
    expect(stackModePatch("none")).toEqual({ stacked: false, normalized: undefined });
    expect(stackModePatch("stacked")).toEqual({ stacked: true, normalized: undefined });
    expect(stackModePatch("normalized")).toEqual({ stacked: true, normalized: true });
  });
});

describe("hover emphasis and blur", () => {
  const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "double" }];
  const rows = [["EU", 10, 3], ["US", 20, 5]];

  // Without `focus`, ECharts emphasises whatever is under the pointer and leaves the rest alone —
  // measured on egret, hovering one band lightened all four, which says nothing about which one
  // you are on. Metabase focuses the series and pushes the others back.
  it("focuses the hovered series on every cartesian kind", () => {
    for (const kind of ["bar", "line", "area"] as const) {
      const opt = buildCartesianOption(cols, rows, { kind, xAxis: "region", yMetrics: ["rev", "cnt"] }) as any;
      for (const s of opt.series) expect(s.emphasis.focus).toBe("series");
    }
  });

  it("lifts an area's own fill and pushes the others back, at Metabase's measured opacities", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "area", xAxis: "region", yMetrics: ["rev", "cnt"] }) as any;
    const s = opt.series[0];
    expect(s.areaStyle.opacity).toBe(0.3);
    expect(s.emphasis.areaStyle.opacity).toBe(0.6);
    expect(s.blur.areaStyle.opacity).toBe(0.2);
  });

  // A bar has no fill opacity to raise, so only the blur applies to it.
  it("blurs a bar without inventing an emphasis fill for it", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"] }) as any;
    const s = opt.series[0];
    expect(s.blur.itemStyle.opacity).toBe(0.3);
    expect(s.emphasis.areaStyle).toBeUndefined();
  });
});

describe("automatic y-axis split (Metabase's shouldAutoSplitYAxis / computeSplit)", () => {
  const ext = (o: Record<string, [number, number]>) => o;
  const base = { kind: "bar" as const, xAxis: "x", yMetrics: ["a", "b"] };

  // Its own note, kept because it is the reason for the number: "When the series with the smallest
  // range is less than 5% of the chart's total range, we split the y-axis so it doesn't look too
  // small."
  it("splits at the 5% boundary and not above it", () => {
    expect(shouldAutoSplitY(base, ext({ a: [0, 100], b: [0, 5] }))).toBe(true);
    expect(shouldAutoSplitY(base, ext({ a: [0, 100], b: [0, 6] }))).toBe(false);
  });

  it("never splits a stack — a second scale would make the parts stop summing to the whole", () => {
    expect(shouldAutoSplitY({ ...base, kind: "area" }, ext({ a: [0, 100], b: [0, 1] }))).toBe(false);
    expect(shouldAutoSplitY({ ...base, stacked: true }, ext({ a: [0, 100], b: [0, 1] }))).toBe(false);
  });

  it("needs two metrics, and respects the opt-out", () => {
    expect(shouldAutoSplitY({ ...base, yMetrics: ["a"] }, ext({ a: [0, 100] }))).toBe(false);
    expect(shouldAutoSplitY({ ...base, autoSplitY: false }, ext({ a: [0, 100], b: [0, 1] }))).toBe(false);
  });

  // Metabase's KEYS_TO_COMPARE clause: two metrics in different currencies do not belong on one
  // scale however close their ranges are.
  it("splits metrics whose formats are not comparable, whatever their ranges", () => {
    const cfg = { ...base, columnSettings: { a: { currency: "USD" }, b: { currency: "EUR" } } };
    expect(shouldAutoSplitY(cfg, ext({ a: [0, 100], b: [0, 100] }))).toBe(true);
    // …and an empty prefix is the same as no prefix, as it is there.
    const same = { ...base, columnSettings: { a: { prefix: "" }, b: {} } };
    expect(shouldAutoSplitY(same, ext({ a: [0, 100], b: [0, 100] }))).toBe(false);
  });

  it("puts the odd metric out on its own side", () => {
    expect(computeAxisSplit(["a", "b"], ext({ a: [0, 100], b: [0, 2] }))).toEqual(["b"]);
  });

  // `axisCost` scores an empty side at -100 while unsplit is favoured, so metrics that share a
  // scale comfortably stay together.
  it("keeps comparable metrics on one axis", () => {
    expect(computeAxisSplit(["a", "b"], ext({ a: [0, 100], b: [0, 90] }))).toEqual([]);
  });
});

describe("combo — how each series is drawn", () => {
  const cols = [{ name: "x", type: "varchar" }, { name: "rev", type: "double" }, { name: "cnt", type: "double" }];
  const rows = [["A", 100, 3], ["B", 200, 5]];

  // Metabase's `getSeriesDefaultDisplay`, verbatim: `index === 0 ? "line" : "bar"`. egret defaulted
  // every series to "bar", so a fresh combo was indistinguishable from a bar chart until the user
  // changed something — the one thing the chart type exists to show was off by default.
  it("draws the first series as a line and the rest as bars", () => {
    expect(seriesDisplayFor("combo", "rev", 0)).toBe("line");
    expect(seriesDisplayFor("combo", "cnt", 1)).toBe("bar");
    expect(seriesDisplayFor("combo", "third", 2)).toBe("bar");
  });

  it("lets an explicit choice win, keyed by SERIES name", () => {
    expect(seriesDisplayFor("combo", "rev", 0, { rev: "bar" })).toBe("bar");
    expect(seriesDisplayFor("combo", "cnt", 1, { cnt: "area" })).toBe("area");
    // A key for a different series does not leak onto this one.
    expect(seriesDisplayFor("combo", "cnt", 1, { rev: "area" })).toBe("bar");
  });

  it("leaves the fixed kinds alone", () => {
    expect(seriesDisplayFor("bar", "rev", 0)).toBe("bar");
    expect(seriesDisplayFor("line", "rev", 1)).toBe("line");
    expect(seriesDisplayFor("area", "rev", 1)).toBe("area");
  });

  it("reaches the drawn series", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "combo", xAxis: "x", yMetrics: ["rev", "cnt"] }) as any;
    expect(opt.series.map((s: any) => s.type)).toEqual(["line", "bar"]);
    expect(opt.series[0].areaStyle).toBeUndefined();
  });

  // "area" is not an ECharts series type — it is a line that happens to be filled — so a combo can
  // hold a filled band and a bare line at once.
  it("draws an area-displayed series as a filled line", () => {
    const opt = buildCartesianOption(cols, rows, {
      kind: "combo", xAxis: "x", yMetrics: ["rev", "cnt"], seriesTypes: { rev: "area" },
    }) as any;
    expect(opt.series[0].type).toBe("line");
    expect(opt.series[0].areaStyle.opacity).toBe(0.3);
    expect(opt.series[0].emphasis.areaStyle.opacity).toBe(0.6);
    expect(opt.series[1].type).toBe("bar");
    expect(opt.series[1].areaStyle).toBeUndefined();
  });
});

describe("scatter — bubbles, breakout and focus", () => {
  const cols = [
    { name: "d", type: "varchar" }, { name: "g", type: "varchar" },
    { name: "x", type: "double" }, { name: "y", type: "double" }, { name: "b", type: "double" },
  ];
  const rows = [
    ["A", "EU", 1, 10, 0], ["B", "EU", 2, 20, 50], ["C", "US", 3, 30, 100],
  ];
  const base = { kind: "scatter" as const, xAxis: "d", xMetric: "x", yMetrics: ["y"] };

  // Metabase's own scale, and its reason: interpolating by AREA rather than by radius, because a
  // radius-linear scale exaggerates large values — doubling the radius quadruples the ink.
  it("sizes a bubble by area between 15 and 75", () => {
    expect(bubbleDiameter(0, 0, 100)).toBeCloseTo(15, 6);
    expect(bubbleDiameter(100, 0, 100)).toBeCloseTo(75, 6);
    // Halfway is NOT 45 — that is the whole point of an area scale.
    expect(bubbleDiameter(50, 0, 100)).toBeCloseTo(2 * Math.sqrt((Math.PI * 7.5 ** 2 + Math.PI * 37.5 ** 2) / 2 / Math.PI), 6);
    expect(bubbleDiameter(50, 0, 100)).toBeGreaterThan(50);
  });

  it("falls back to the floor when there is nothing to scale against", () => {
    expect(bubbleDiameter(5, 5, 5)).toBe(15);
    expect(bubbleDiameter(NaN, 0, 10)).toBe(15);
  });

  it("gives every point its own size from the bubble metric", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, bubbleMetric: "b" }) as any;
    const sizeOf = opt.series[0].symbolSize;
    const data = opt.series[0].data;
    expect(sizeOf(null, { data: data[0] })).toBeCloseTo(15, 6);
    expect(sizeOf(null, { data: data[2] })).toBeCloseTo(75, 6);
  });

  // The extent spans every point, not each band's own — otherwise one value would draw at two
  // sizes depending on which breakout band it landed in.
  it("scales bubbles against the whole chart, not each series", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, series: "g", bubbleMetric: "b" }) as any;
    const us = opt.series.find((s: any) => s.__raw === "US");
    expect(us.symbolSize(null, { data: us.data[0] })).toBeCloseTo(75, 6);
  });

  it("draws one series per breakout value, each its own colour", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, series: "g" }) as any;
    expect(opt.series.map((s: any) => s.__raw)).toEqual(["EU", "US"]);
    expect(opt.series[0].itemStyle.color).not.toBe(opt.series[1].itemStyle.color);
    expect(opt.legend).toBeDefined();
  });

  // scatterXY bypasses decorateSeries, so it missed the emphasis work the rest of the family got —
  // and a scatter needs it most, its points overlapping by nature.
  it("focuses the hovered series", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, series: "g" }) as any;
    for (const s of opt.series) {
      expect(s.emphasis.focus).toBe("series");
      expect(s.blur.itemStyle.opacity).toBe(0.3);
    }
  });
});

/**
 * A nominal axis draws in a stable order, whatever the query returned.
 *
 * THE FIXTURE IS THE POINT. These are the two orderings actually measured on
 * `POST /charts/{id}/data` across ten calls to the same chart — seven returned one, three the
 * other — and NEITHER is alphabetical. A fixture whose categories already arrive in the target
 * order cannot detect this defect at all, so both real arrival orders are asserted here, and the
 * expected output matches neither of them.
 */
describe("a nominal axis draws in a stable order", () => {
  const cols = [{ name: "category", type: "varchar" }, { name: "revenue", type: "double" }];
  // 3 calls in 10.
  const arrivalA = [["Widget", 4], ["Gadget", 3], ["Doohickey", 1], ["Gizmo", 2]];
  // 7 calls in 10 — the one that made three consecutive page loads look deterministic.
  const arrivalB = [["Gizmo", 2], ["Widget", 4], ["Gadget", 3], ["Doohickey", 1]];
  const sorted = ["Doohickey", "Gadget", "Gizmo", "Widget"];

  it("sorts a bar's categories ascending, whichever order the rows arrived in", () => {
    for (const rows of [arrivalA, arrivalB]) {
      const opt = buildCartesianOption(cols, rows, {
        kind: "bar", xAxis: "category", yMetrics: ["revenue"],
      }) as any;
      expect(opt.xAxis.data).toEqual(sorted);
      // The bars must follow their categories, not keep the arrival order beside a sorted axis.
      expect(opt.series[0].data).toEqual([1, 3, 2, 4]);
    }
  });

  it("draws the two arrival orders identically — that is the whole defect", () => {
    const of = (rows: unknown[][]) => JSON.stringify(
      buildCartesianOption(cols, rows, { kind: "bar", xAxis: "category", yMetrics: ["revenue"] }),
    );
    expect(of(arrivalA)).toBe(of(arrivalB));
  });

  it("sorts a row chart's category axis the same way", () => {
    const opt = buildCartesianOption(cols, arrivalB, {
      kind: "row", xAxis: "category", yMetrics: ["revenue"],
    }) as any;
    expect(opt.yAxis.type).toBe("category");
    expect(opt.yAxis.data).toEqual(sorted);
    expect(opt.series[0].data).toEqual([1, 3, 2, 4]);
  });

  it("sorts a breakout chart's categories too, and keeps each cell with its own category", () => {
    const bcols = [
      { name: "category", type: "varchar" }, { name: "state", type: "varchar" },
      { name: "revenue", type: "double" },
    ];
    const rows = [["Widget", "CA", 4], ["Doohickey", "CA", 1], ["Gizmo", "TX", 2]];
    const opt = buildCartesianOption(bcols, rows, {
      kind: "bar", xAxis: "category", yMetrics: ["revenue"], series: "state",
    }) as any;
    expect(opt.xAxis.data).toEqual(["Doohickey", "Gizmo", "Widget"]);
    expect(opt.series.find((s: any) => s.name === "CA").data).toEqual([1, null, 4]);
    expect(opt.series.find((s: any) => s.name === "TX").data).toEqual([null, 2, null]);
  });

  /**
   * The one exemption. Arbitrary order is what is being fixed; a PIPELINE is not arbitrary, and
   * the funnel's bar mode hands its steps over already ranked by `funnel.stepOrder`. The second
   * half is what makes this test worth having — it shows the sort really would have scrambled it.
   */
  it("leaves a caller's meaningful order alone when it asks", () => {
    const scols = [{ name: "step", type: "varchar" }, { name: "users", type: "bigint" }];
    const rows = [["Visited", 4], ["Added to cart", 3], ["Checkout", 2], ["Purchased", 1]];
    const base = { kind: "bar" as const, xAxis: "step", yMetrics: ["users"] };
    const kept = buildCartesianOption(scols, rows, { ...base, preserveCategoryOrder: true }) as any;
    expect(kept.xAxis.data).toEqual(["Visited", "Added to cart", "Checkout", "Purchased"]);
    const scrambled = buildCartesianOption(scols, rows, base) as any;
    expect(scrambled.xAxis.data).toEqual(["Added to cart", "Checkout", "Purchased", "Visited"]);
  });

  // The two branches this does NOT change. They are Metabase's own (`sortDataset`) and were never
  // the defect: a temporal axis already drew chronologically and a numeric one already ascending.
  it("still draws a temporal axis chronologically", () => {
    const tcols = [{ name: "m", type: "timestamp" }, { name: "revenue", type: "double" }];
    const rows = [["2026-03-01", 3], ["2026-01-01", 1], ["2026-02-01", 2]];
    const opt = buildCartesianOption(tcols, rows, { kind: "line", xAxis: "m", yMetrics: ["revenue"] }) as any;
    expect(opt.series[0].data).toEqual([1, 2, 3]);
  });

  it("still draws a numeric axis ascending, not as text", () => {
    const ncols = [{ name: "n", type: "bigint" }, { name: "revenue", type: "double" }];
    // 2 < 10 numerically but "10" < "2" as text, so this discriminates the numeric branch from a
    // string sort landing on it by mistake.
    const rows = [["10", 10], ["2", 2], ["1", 1]];
    const opt = buildCartesianOption(ncols, rows, { kind: "bar", xAxis: "n", yMetrics: ["revenue"] }) as any;
    expect(opt.series[0].data).toEqual([1, 2, 10]);
  });
});
