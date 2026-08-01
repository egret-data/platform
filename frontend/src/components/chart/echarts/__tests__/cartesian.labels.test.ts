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

/** `opt` never passes a size, so this block describes the SIZE-BLIND path throughout. What the
 *  builder does once it can measure — draw every label, rotate every label, or draw none — is
 *  `cartesianXLabels.test.ts`. */
describe("category-axis labels, with nothing to measure against", () => {
  it("leaves label spacing to ECharts at any length, because only ECharts knows the width", () => {
    expect(opt(base, small).xAxis.axisLabel.interval).toBe("auto");
    expect(opt(base).xAxis.axisLabel.interval).toBe("auto"); // 40 categories — still auto
  });

  it("leaves spacing to ECharts in horizontal mode too, wherever the category axis sits", () => {
    expect(opt({ ...base, kind: "row" }).yAxis.axisLabel.interval).toBe("auto");
  });

  /**
   * This block used to assert the opposite — that twelve labels of twenty-odd characters came back
   * tilted, from `longest > 10 && categories.length > 6 ? 30 : 0`. That rule is gone rather than
   * kept as a fallback, and deliberately: length and count are proxies for width, and the rule got
   * the case that mattered backwards, declining to rotate four "Doohickey"-length names on a
   * one-third-width card and leaving `interval: "auto"` to drop three of the four. A guess made
   * without a width is what this workstream removes, so with no width there is no guess.
   */
  it("does not guess a rotation from label length when it cannot see the width", () => {
    const wordy = Array.from({ length: 12 }, (_, i) => [`Metropolitan region ${i}`, i * 10]);
    expect(opt(base, wordy).xAxis.axisLabel.rotate).toBe(0);
  });

  it("leaves short labels upright", () => {
    expect(opt(base).xAxis.axisLabel.rotate).toBe(0);
  });

  it("still draws the axis — hiding it is a decision only measurement can take", () => {
    const wordy = Array.from({ length: 12 }, (_, i) => [`Metropolitan region ${i}`, i * 10]);
    expect(opt(base, wordy).xAxis.axisLabel.show).toBe(true);
  });
});

describe("data label density", () => {
  it("hides colliding labels when density is unset (the default)", () => {
    const o = opt({ ...base, showLabels: true });
    expect(o.series[0].labelLayout).toEqual({ hideOverlap: true });
  });

  it("hides colliding labels when density is explicitly \"fit\"", () => {
    const o = opt({ ...base, showLabels: true, labelDensity: "fit" });
    expect(o.series[0].labelLayout).toEqual({ hideOverlap: true });
  });

  it("draws every label when density is \"all\"", () => {
    const o = opt({ ...base, showLabels: true, labelDensity: "all" });
    expect(o.series[0].labelLayout).toBeUndefined();
  });

  it("never sets labelLayout when labels are off, regardless of density", () => {
    expect(opt({ ...base, showLabels: false, labelDensity: "all" }).series[0].labelLayout).toBeUndefined();
    expect(opt({ ...base, showLabels: false, labelDensity: "fit" }).series[0].labelLayout).toBeUndefined();
    expect(opt(base).series[0].labelLayout).toBeUndefined();
  });
});
