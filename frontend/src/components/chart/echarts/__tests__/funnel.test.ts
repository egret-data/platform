import { describe, it, expect } from "vitest";
import { buildFunnelOption } from "../options/funnel";
import { SERIES_PALETTE } from "../palette";

const cols = [{ name: "stage", type: "varchar" }, { name: "cnt", type: "double" }];

describe("buildFunnelOption", () => {
  it("maps rows to funnel stages, sorted descending by value", () => {
    const opt = buildFunnelOption(cols, [["Cart", 40], ["Visit", 100], ["Buy", 10]], { dimension: "stage", metric: "cnt" }) as any;
    expect(opt.series[0].type).toBe("funnel");
    expect(opt.series[0].sort).toBe("descending");
    expect(opt.series[0].data).toEqual([{ name: "Visit", value: 100 }, { name: "Cart", value: 40 }, { name: "Buy", value: 10 }]);
  });
  it("drops non-positive / non-finite values", () => {
    const opt = buildFunnelOption(cols, [["A", 10], ["B", 0], ["C", -5]], { dimension: "stage", metric: "cnt" }) as any;
    expect(opt.series[0].data.map((s: any) => s.name)).toEqual(["A"]);
  });
  it("uses the categorical palette and an item tooltip with percent", () => {
    const opt = buildFunnelOption(cols, [["A", 10]], { dimension: "stage", metric: "cnt" }) as any;
    expect(opt.color).toEqual(SERIES_PALETTE);
    expect(opt.tooltip.formatter({ name: "A", value: 10, percent: 100 })).toContain("100%");
  });
  it("empty when dimension or metric missing", () => {
    expect((buildFunnelOption(cols, [["A", 10]], {}) as any).series[0].data).toEqual([]);
  });
});
