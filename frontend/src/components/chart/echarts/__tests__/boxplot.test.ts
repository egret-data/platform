import { describe, it, expect } from "vitest";
import { buildBoxplotOption } from "../options/boxplot";

const cols = [{ name: "g", type: "varchar" }, { name: "v", type: "double" }];
const rows = [["A", 1], ["A", 2], ["A", 3], ["A", 4], ["A", 5], ["B", 10], ["B", 20], ["B", 30]];

describe("buildBoxplotOption", () => {
  it("computes a five-number summary per category", () => {
    const opt = buildBoxplotOption(cols, rows, { category: "g", value: "v" }) as any;
    expect(opt.series[0].type).toBe("boxplot");
    expect(opt.xAxis.data).toEqual(["A", "B"]);
    expect(opt.series[0].data[0]).toEqual([1, 2, 3, 4, 5]);     // A: 1..5
    expect(opt.series[0].data[1]).toEqual([10, 15, 20, 25, 30]); // B: 10,20,30 (interpolated quartiles)
  });
  it("ignores non-finite values", () => {
    const opt = buildBoxplotOption(cols, [["A", 1], ["A", NaN], ["A", 3]], { category: "g", value: "v" }) as any;
    expect(opt.series[0].data[0]).toEqual([1, 1.5, 2, 2.5, 3]);
  });
  it("empty when category or value is missing", () => {
    expect((buildBoxplotOption(cols, rows, {}) as any).series[0].data).toEqual([]);
  });
  it("separates 1.5·IQR outliers and plots the mean (Metabase parity)", () => {
    // A: 1..5 plus a far outlier 100. Q1=2,Q3=4,IQR=2,high fence=7 → 100 is an outlier.
    const r = [["A", 1], ["A", 2], ["A", 3], ["A", 4], ["A", 5], ["A", 100]];
    const opt = buildBoxplotOption(cols, r, { category: "g", value: "v" }) as any;
    expect(opt.series[0].data[0][4]).toBe(5);          // whisker high stays at 5, not 100
    expect(opt.series[1].type).toBe("scatter");        // outlier series
    expect(opt.series[1].data).toEqual([[0, 100]]);    // the outlier point
    expect(opt.series[2].symbol).toBe("diamond");      // mean series
    expect(opt.series[2].data[0][1]).toBeCloseTo((1 + 2 + 3 + 4 + 5 + 100) / 6, 6);
  });
});
