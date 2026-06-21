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
});
