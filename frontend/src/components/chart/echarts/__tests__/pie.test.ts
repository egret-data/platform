import { describe, it, expect } from "vitest";
import { buildPieOption } from "../options/pie";

const cols = [{ name: "region", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["EU", 10], ["US", 30]];

describe("buildPieOption", () => {
  it("maps rows to {name, value} slices", () => {
    const opt = buildPieOption(cols, rows, { dimension: "region", metric: "rev" }) as any;
    expect(opt.series[0].type).toBe("pie");
    expect(opt.series[0].data).toEqual([{ name: "EU", value: 10 }, { name: "US", value: 30 }]);
    expect(opt.series[0].radius).toEqual(["0%", "70%"]);
  });
  it("donut sets an inner radius", () => {
    const opt = buildPieOption(cols, rows, { dimension: "region", metric: "rev", donut: true }) as any;
    expect(opt.series[0].radius).toEqual(["45%", "70%"]);
  });
  it("empty when dimension or metric missing", () => {
    expect((buildPieOption(cols, rows, {}) as any).series[0].data).toEqual([]);
  });
});
