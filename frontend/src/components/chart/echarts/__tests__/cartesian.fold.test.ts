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

  it("a breakout chart returns before the fold is reached", () => {
    const o = opt({ ...base, series: "cat", maxCategories: 3 }) as never as { xAxis: { data: string[] } };
    expect(o.xAxis.data).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("leaves a multi-metric chart alone", () => {
    const cols = [...columns, { name: "qty", type: "double" }];
    const rs = rows.map((r, i) => [...r, i]);
    const o = buildCartesianOption(cols, rs, { ...base, yMetrics: ["rev", "qty"], maxCategories: 3 } as never) as never as { xAxis: { data: string[] } };
    expect(o.xAxis.data).toEqual(["a", "b", "c", "d", "e"]);
  });

  it("leaves a value-scaled x-axis alone — Other has no coordinate to sit at", () => {
    const cols = [{ name: "x", type: "double" }, { name: "rev", type: "double" }];
    const rs = [[1, 10], [2, 8], [3, 6], [4, 4], [5, 2]];
    const o = buildCartesianOption(cols, rs, {
      kind: "bar", xAxis: "x", yMetrics: ["rev"], xScale: "linear", maxCategories: 3,
    } as never) as never as { series: { data: unknown[] }[] };
    expect(o.series[0].data).toHaveLength(5);
  });

  it("leaves a chart with a trend line alone — the fit would describe the cutoff", () => {
    const o = opt({ ...base, maxCategories: 3, trendLine: true }) as never as { xAxis: { data: string[] } };
    expect(o.xAxis.data).toEqual(["a", "b", "c", "d", "e"]);
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
