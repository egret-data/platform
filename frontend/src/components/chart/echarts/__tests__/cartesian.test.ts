import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

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
