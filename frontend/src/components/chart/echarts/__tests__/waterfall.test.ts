import { describe, it, expect } from "vitest";
import { buildWaterfallOption } from "../options/waterfall";

const cols = [{ name: "m", type: "varchar" }, { name: "delta", type: "double" }];
const rows = [["Jan", 100], ["Feb", -30], ["Mar", 50]];

describe("buildWaterfallOption", () => {
  it("floats each delta on a transparent base = the running total", () => {
    const opt = buildWaterfallOption(cols, rows, { xAxis: "m", metric: "delta" }) as any;
    expect(opt.series[0].data).toEqual([0, 70, 70]);                       // base = min(before, after)
    expect(opt.series[1].data.map((d: any) => d.value)).toEqual([100, 30, 50]); // |delta|
    expect(opt.xAxis.data).toEqual(["Jan", "Feb", "Mar"]);
  });
  it("colours increases and decreases differently", () => {
    const opt = buildWaterfallOption(cols, rows, { xAxis: "m", metric: "delta" }) as any;
    expect(opt.series[1].data[0].itemStyle.color).not.toBe(opt.series[1].data[1].itemStyle.color);
  });
  it("appends a Total bar when showTotal", () => {
    const opt = buildWaterfallOption(cols, rows, { xAxis: "m", metric: "delta", showTotal: true }) as any;
    expect(opt.xAxis.data).toEqual(["Jan", "Feb", "Mar", "Total"]);
    expect(opt.series[0].data[3]).toBe(0);
    expect(opt.series[1].data[3].value).toBe(120); // 100 - 30 + 50
  });
  it("empty when x or metric is missing", () => {
    expect((buildWaterfallOption(cols, rows, {}) as any).series[1].data).toEqual([]);
  });
});
