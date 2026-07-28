import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [
  { name: "region", type: "varchar" },
  { name: "cat", type: "varchar" },
  { name: "rev", type: "double" },
];
const rows = [["a", "x", 10], ["b", "x", 8], ["c", "y", 6], ["d", "y", 4], ["e", "y", 2]];
const opt = (cfg: Record<string, unknown>) =>
  buildCartesianOption(columns, rows, cfg as never) as unknown as Record<string, never>;
const base = { kind: "bar", xAxis: "region", yMetrics: ["rev"] };

describe("max categories", () => {
  it("keeps every category when no maximum is set", () => {
    const o = opt(base) as never as { xAxis: { data: string[] } };
    expect(o.xAxis.data).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("folds the tail into Other when a maximum is set", () => {
    const o = opt({ ...base, maxCategories: 3 }) as never as {
      xAxis: { data: string[] }; series: { data: number[] }[];
    };
    expect(o.xAxis.data).toEqual(["a", "b", "Other"]);
    expect(o.series[0].data).toEqual([10, 8, 12]);
  });

  it("leaves a breakout chart alone — Other would have to merge series too", () => {
    const o = opt({ ...base, series: "cat", maxCategories: 3 }) as never as { xAxis: { data: string[] } };
    expect(o.xAxis.data).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("leaves a multi-metric chart alone", () => {
    const cols = [...columns, { name: "qty", type: "double" }];
    const rs = rows.map((r, i) => [...r, i]);
    const o = buildCartesianOption(cols, rs, { ...base, yMetrics: ["rev", "qty"], maxCategories: 3 } as never) as never as { xAxis: { data: string[] } };
    expect(o.xAxis.data).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("leaves a value-scaled x-axis alone — Other has no coordinate", () => {
    const o = opt({ ...base, maxCategories: 3, xScale: "linear" }) as never as { xAxis: unknown };
    expect(JSON.stringify(o.xAxis)).not.toContain("Other");
  });

  it("still folds when the axis title and legend defaults are in play", () => {
    const o = opt({ ...base, maxCategories: 2 }) as never as {
      xAxis: { data: string[]; name?: string }; series: { data: number[] }[];
    };
    expect(o.xAxis.data).toEqual(["a", "Other"]);
    expect(o.series[0].data).toEqual([10, 20]);
    expect(o.xAxis.name).toBe("region");
  });
});
