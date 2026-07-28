import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
const big = Array.from({ length: 40 }, (_, i) => [`r${i}`, (i + 1) * 25_468]);
const small = [["a", 12], ["b", 34]];
const opt = (cfg: any, rows: unknown[][] = big) => buildCartesianOption(columns, rows, cfg) as any;
const base = { kind: "bar", xAxis: "region", yMetrics: ["rev"] };

describe("value-axis labels", () => {
  it("shortens wide numbers by default", () => {
    expect(opt(base).yAxis.axisLabel.formatter(25468)).toBe("25.5K");
  });

  it("leaves small numbers alone", () => {
    expect(opt(base, small).yAxis.axisLabel.formatter(25468)).toBe("25,468");
  });

  it("obeys an explicit choice in both directions", () => {
    expect(opt({ ...base, compactValues: false }).yAxis.axisLabel.formatter(25468)).toBe("25,468");
    expect(opt({ ...base, compactValues: true }, small).yAxis.axisLabel.formatter(25468)).toBe("25.5K");
  });

  it("keeps the column's own formatting while shortening", () => {
    const o = opt({ ...base, columnSettings: { rev: { prefix: "$", number_separators: ", " } } });
    expect(o.yAxis.axisLabel.formatter(25468)).toBe("$25,5K");
  });

  it("lets a column that asks for full numbers keep them", () => {
    const o = opt({ ...base, columnSettings: { rev: { compact: false } } });
    expect(o.yAxis.axisLabel.formatter(25468)).toBe("25,468");
  });

  it("shows percentages unchanged when the chart is 100% stacked", () => {
    const o = opt({ ...base, normalized: true, stacked: true });
    expect(o.yAxis.axisLabel.formatter(42)).toBe("42%");
  });

  it("decides per axis, so a small left axis is not shortened by a large right one", () => {
    const cols = [
      { name: "region", type: "varchar" },
      { name: "orders", type: "double" },
      { name: "revenue", type: "double" },
    ];
    const rows = [["N", 300, 2_000_000], ["S", 1200, 9_000_000], ["E", 2400, 16_000_000]];
    const o = buildCartesianOption(cols, rows, {
      kind: "combo", xAxis: "region",
      yMetrics: ["orders", "revenue"],
      rightAxisMetrics: ["revenue"],
    } as any) as any;
    expect(o.yAxis[0].axisLabel.formatter(2100)).toBe("2,100");
    expect(o.yAxis[1].axisLabel.formatter(2_000_000)).toBe("2M");
  });
});

describe("category-axis labels", () => {
  it("leaves label spacing to ECharts at any length, because only ECharts knows the width", () => {
    expect(opt(base, small).xAxis.axisLabel.interval).toBe("auto");
    expect(opt(base).xAxis.axisLabel.interval).toBe("auto"); // 40 categories — still auto
  });

  it("leaves spacing to ECharts in horizontal mode too, wherever the category axis sits", () => {
    expect(opt({ ...base, kind: "row" }).yAxis.axisLabel.interval).toBe("auto");
  });

  it("rotates long labels rather than only dropping them", () => {
    const wordy = Array.from({ length: 12 }, (_, i) => [`Metropolitan region ${i}`, i * 10]);
    expect(opt(base, wordy).xAxis.axisLabel.rotate).toBeGreaterThan(0);
  });

  it("leaves short labels upright", () => {
    expect(opt(base).xAxis.axisLabel.rotate).toBe(0);
  });
});
