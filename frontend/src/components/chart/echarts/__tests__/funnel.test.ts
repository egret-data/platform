import { describe, it, expect } from "vitest";
import { buildFunnelOption } from "../options/funnel";
import { SERIES_PALETTE } from "../palette";

const cols = [{ name: "stage", type: "varchar" }, { name: "cnt", type: "double" }];

describe("buildFunnelOption", () => {
  // Row order, NOT value order — see the sequence-vs-ranking block below for why that changed.
  it("maps rows to funnel stages in the order they arrived", () => {
    const opt = buildFunnelOption(cols, [["Cart", 40], ["Visit", 100], ["Buy", 10]], { dimension: "stage", metric: "cnt" }) as any;
    expect(opt.series[0].type).toBe("funnel");
    expect(opt.series[0].sort).toBe("none");
    expect(opt.series[0].data).toEqual([{ name: "Cart", value: 40 }, { name: "Visit", value: 100 }, { name: "Buy", value: 10 }]);
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

describe("buildFunnelOption — steps are a sequence, not a ranking", () => {
  const cols = [{ name: "step", type: "varchar" }, { name: "n", type: "double" }];
  // Deliberately NOT descending: the third step is larger than the second.
  const rows = [["Visited", 100], ["Signed up", 40], ["Verified", 60], ["Paid", 10]];

  // Measured against Metabase on four categories whose values do not descend: it keeps them in row
  // order. egret sorted by value, which rearranges a funnel's sequence into a story the data never
  // told — "Verified" appearing before "Signed up" reads as a different process.
  it("keeps the rows in the order they arrived", () => {
    const opt = buildFunnelOption(cols, rows, { dimension: "step", metric: "n" }) as any;
    expect(opt.series[0].data.map((d: any) => d.name)).toEqual(["Visited", "Signed up", "Verified", "Paid"]);
  });

  // The other half: ECharts' own default is descending and would undo the above.
  it("tells ECharts not to re-sort either", () => {
    const opt = buildFunnelOption(cols, rows, { dimension: "step", metric: "n" }) as any;
    expect(opt.series[0].sort).toBe("none");
  });

  it("honours a stored order, keeping unlisted steps after it", () => {
    const opt = buildFunnelOption(cols, rows, {
      dimension: "step", metric: "n", stepOrder: ["Paid", "Visited"],
    }) as any;
    expect(opt.series[0].data.map((d: any) => d.name)).toEqual(["Paid", "Visited", "Signed up", "Verified"]);
  });

  it("drops a hidden step", () => {
    const opt = buildFunnelOption(cols, rows, {
      dimension: "step", metric: "n", hiddenSteps: ["Signed up"],
    }) as any;
    expect(opt.series[0].data.map((d: any) => d.name)).toEqual(["Visited", "Verified", "Paid"]);
  });

  // Metabase prints each step's share of the FIRST one ("124.22 %"); egret printed only the raw
  // value, leaving the reader to divide in their head.
  it("labels every step after the first with its share of the first", () => {
    const opt = buildFunnelOption(cols, rows, { dimension: "step", metric: "n" }) as any;
    const label = opt.series[0].label.formatter;
    expect(label({ name: "Visited", value: 100, dataIndex: 0 })).toBe("Visited: 100");
    expect(label({ name: "Signed up", value: 40, dataIndex: 1 })).toContain("40.00 %");
    expect(label({ name: "Paid", value: 10, dataIndex: 3 })).toContain("10.00 %");
  });
});
