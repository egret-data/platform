import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [
  { name: "region", type: "varchar" },
  { name: "cat", type: "varchar" },
  { name: "rev", type: "double" },
  { name: "cnt", type: "double" },
];
const rows = [
  ["a", "x", 10, 1],
  ["b", "y", 8, 2],
];
const opt = (cfg: Record<string, unknown>) =>
  buildCartesianOption(columns, rows, cfg as never) as unknown as Record<string, never>;
const base = { kind: "bar", xAxis: "region", yMetrics: ["rev", "cnt"] };

describe("series order and visibility", () => {
  it("defaults to yMetrics order", () => {
    const o = opt(base) as never as { series: { name: string }[] };
    expect(o.series.map((s) => s.name)).toEqual(["rev", "cnt"]);
  });

  it("seriesOrder reorders the series", () => {
    const o = opt({ ...base, seriesOrder: ["cnt", "rev"] }) as never as { series: { name: string }[] };
    expect(o.series.map((s) => s.name)).toEqual(["cnt", "rev"]);
  });

  it("hiddenSeries drops a series entirely", () => {
    const o = opt({ ...base, hiddenSeries: ["rev"] }) as never as { series: { name: string }[] };
    expect(o.series.map((s) => s.name)).toEqual(["cnt"]);
  });

  it("ignores a seriesOrder name that is no longer plotted", () => {
    const o = opt({ ...base, seriesOrder: ["ghost", "cnt", "rev"] }) as never as { series: { name: string }[] };
    expect(o.series.map((s) => s.name)).toEqual(["cnt", "rev"]);
  });

  it("a partial seriesOrder puts the named series first and keeps the rest in natural order", () => {
    const cols = [...columns, { name: "amt", type: "double" }];
    const rs = rows.map((r, i) => [...r, i + 100]);
    const o = buildCartesianOption(cols, rs, {
      ...base, yMetrics: ["rev", "cnt", "amt"], seriesOrder: ["cnt"],
    } as never) as unknown as { series: { name: string }[] };
    expect(o.series.map((s) => s.name)).toEqual(["cnt", "rev", "amt"]);
  });

  it("a hidden series is not counted in the stack total", () => {
    const o = opt({
      ...base, stacked: true, showStackTotal: true, hiddenSeries: ["rev"],
    }) as never as { series: { name?: string; data: unknown[]; __total?: boolean }[] };
    const visible = o.series.find((s) => s.name === "cnt")!;
    const total = o.series[o.series.length - 1];
    expect(total.data).toEqual(visible.data);
  });

  it("hiding a series removes its trend line", () => {
    const o = opt({ ...base, trendLine: true, hiddenSeries: ["rev"] }) as never as { series: { name?: string }[] };
    expect(o.series.some((s) => s.name === "rev trend")).toBe(false);
    expect(o.series.map((s) => s.name)).not.toContain("rev trend");
  });

  it("applies to a breakout chart's series too", () => {
    const o = opt({ kind: "bar", xAxis: "region", yMetrics: ["rev"], series: "cat", seriesOrder: ["y", "x"] }) as never as {
      series: { name: string }[];
    };
    expect(o.series.map((s) => s.name)).toEqual(["y", "x"]);
  });

  it("does not describe a metric to a screen reader when every series is hidden", () => {
    const o = opt({ ...base, yMetrics: ["rev", "cnt"], hiddenSeries: ["rev", "cnt"] }) as never as {
      series: unknown[]; aria: { label: { description: string } };
    };
    expect(o.series).toHaveLength(0);
    expect(o.aria.label.description).not.toContain("NaN");
    expect(o.aria.label.description).not.toContain("rev");
  });

  it("drops the right axis when its only metric is hidden", () => {
    const o = opt({
      ...base, yMetrics: ["rev", "cnt"], rightAxisMetrics: ["cnt"], hiddenSeries: ["cnt"],
    }) as never as { yAxis: unknown };
    expect(Array.isArray(o.yAxis)).toBe(false);
  });

  it("keeps both axes when a series still sits on the right one", () => {
    const o = opt({
      ...base, yMetrics: ["rev", "cnt"], rightAxisMetrics: ["cnt"], hiddenSeries: ["rev"],
    }) as never as { yAxis: unknown[] };
    expect(Array.isArray(o.yAxis)).toBe(true);
  });

  it("still titles a breakout chart's axis", () => {
    const o = opt({ ...base, series: "cat", yMetrics: ["rev"] }) as never as { yAxis: { name?: string } };
    expect(o.yAxis.name).toBe("rev");
  });
});
