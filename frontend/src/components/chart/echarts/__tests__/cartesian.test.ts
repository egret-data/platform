import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";
import { CHART_THEME, SERIES_PALETTE } from "../palette";

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
  it("cycles the categorical palette so each series gets a distinct color", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, yMetrics: ["rev", "cnt"] }) as any;
    expect(opt.series[0].itemStyle.color).toBe(SERIES_PALETTE[0]);
    expect(opt.series[1].itemStyle.color).toBe(SERIES_PALETTE[1]);
  });
  it("bar series carry barWidth", () => {
    const opt = buildCartesianOption(cols, rows, base) as any;
    expect(opt.series[0].barWidth).toBe(CHART_THEME.barWidth);
  });
  it("line series are smooth when configured and carry a lineStyle width", () => {
    const opt = buildCartesianOption(cols, rows, { ...base, kind: "line", smooth: true }) as any;
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
  it("preserves first-seen order for a categorical (string) x-axis", () => {
    const cols = [{ name: "p", type: "varchar" }, { name: "rev", type: "double" }];
    const rows = [["Med", 2], ["High", 3], ["Low", 1]];
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "p", yMetrics: ["rev"] }) as any;
    expect(opt.xAxis.data).toEqual(["Med", "High", "Low"]);
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
  it("stays single-axis when no metric is on the right", () => {
    const opt = buildCartesianOption(cols, rows, { kind: "bar", xAxis: "x", yMetrics: ["rev", "cnt"] }) as any;
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
