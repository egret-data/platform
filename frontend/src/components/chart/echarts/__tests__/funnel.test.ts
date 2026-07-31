import { describe, it, expect } from "vitest";
import { funnelSteps } from "../options/funnel";

const cols = [{ name: "stage", type: "varchar" }, { name: "cnt", type: "double" }];

describe("funnelSteps — a sequence, not a ranking", () => {
  // Deliberately NOT descending: the third step is larger than the second.
  const rows = [["Visited", 100], ["Signed up", 40], ["Verified", 60], ["Paid", 10]];

  // Measured against Metabase on four categories whose values do not descend: it keeps them in row
  // order. egret sorted by value, which rearranges a funnel's sequence into a story the data never
  // told — "Verified" appearing before "Signed up" reads as a different process.
  it("keeps the rows in the order they arrived", () => {
    const steps = funnelSteps(cols, rows, { dimension: "stage", metric: "cnt" });
    expect(steps.map((s) => s.name)).toEqual(["Visited", "Signed up", "Verified", "Paid"]);
  });

  it("honours a stored order, keeping unlisted steps after it", () => {
    const steps = funnelSteps(cols, rows, { dimension: "stage", metric: "cnt", stepOrder: ["Paid", "Visited"] });
    expect(steps.map((s) => s.name)).toEqual(["Paid", "Visited", "Signed up", "Verified"]);
  });

  it("drops a hidden step", () => {
    const steps = funnelSteps(cols, rows, { dimension: "stage", metric: "cnt", hiddenSteps: ["Signed up"] });
    expect(steps.map((s) => s.name)).toEqual(["Visited", "Verified", "Paid"]);
  });

  it("drops non-positive / non-finite values", () => {
    const steps = funnelSteps(cols, [["A", 10], ["B", 0], ["C", -5]], { dimension: "stage", metric: "cnt" });
    expect(steps.map((s) => s.name)).toEqual(["A"]);
  });

  // Metabase prints each step's share of the FIRST one ("124.22 %"); egret printed only the raw
  // value, leaving the reader to divide in their head.
  it("gives every step after the first its share of the first", () => {
    const steps = funnelSteps(cols, rows, { dimension: "stage", metric: "cnt" });
    expect(steps[0].share).toBe("");
    expect(steps[1].share).toBe("40.00 %");
    expect(steps[3].share).toBe("10.00 %");
  });

  // The edge height Metabase draws: `value / max`. On its own chart `3976/5061 = 0.78562` was
  // exactly the polygon edge, which is where this rule comes from.
  it("sizes each edge as a share of the tallest step", () => {
    const steps = funnelSteps(cols, rows, { dimension: "stage", metric: "cnt" });
    expect(steps[0].fraction).toBeCloseTo(1, 10);
    expect(steps[1].fraction).toBeCloseTo(0.4, 10);
    expect(steps[3].fraction).toBeCloseTo(0.1, 10);
  });

  it("formats the value through the metric's own column settings", () => {
    const steps = funnelSteps(cols, [["A", 1234.5]], {
      dimension: "stage", metric: "cnt", columnSettings: { cnt: { prefix: "$", decimals: 0 } },
    });
    expect(steps[0].label).toContain("$");
  });

  it("returns nothing without a dimension or a metric", () => {
    expect(funnelSteps(cols, rows, { dimension: "stage" })).toEqual([]);
    expect(funnelSteps(cols, rows, { metric: "cnt" })).toEqual([]);
  });
});
