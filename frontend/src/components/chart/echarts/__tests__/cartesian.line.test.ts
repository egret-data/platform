import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

const columns = [{ name: "d", type: "varchar" }, { name: "rev", type: "double" }];
const rows = [["a", 1], ["b", 2], ["c", 3]];
const opt = (cfg: Record<string, unknown>) =>
  buildCartesianOption(columns, rows, {
    kind: "line", xAxis: "d", yMetrics: ["rev"], ...cfg,
  } as never) as never as { series: Record<string, unknown>[] };

describe("line shape", () => {
  it("draws a straight line by default", () => {
    const s = opt({}).series[0];
    expect(s.smooth).toBe(false);
    expect(s.step).toBeUndefined();
  });

  it("smooths when asked", () => {
    expect(opt({ lineShape: "smooth" }).series[0].smooth).toBe(true);
  });

  it("steps when asked, and does not also smooth", () => {
    const s = opt({ lineShape: "step" }).series[0];
    expect(s.step).toBe("middle");
    expect(s.smooth).toBe(false);
  });

  it("lets a series override the chart's shape", () => {
    const s = opt({ lineShape: "linear", seriesLine: { rev: { shape: "smooth" } } }).series[0];
    expect(s.smooth).toBe(true);
  });

  it("lets a series override in the other direction too", () => {
    const s = opt({ lineShape: "smooth", seriesLine: { rev: { shape: "linear" } } }).series[0];
    expect(s.smooth).toBe(false);
    expect(s.step).toBeUndefined();
  });

  it("only overrides the series it names", () => {
    const cols = [...columns, { name: "qty", type: "double" }];
    const rs = rows.map((r, i) => [...r, i]);
    const o = buildCartesianOption(cols, rs, {
      kind: "line", xAxis: "d", yMetrics: ["rev", "qty"],
      lineShape: "linear", seriesLine: { rev: { shape: "step" } },
    } as never) as never as { series: Record<string, unknown>[] };
    expect(o.series[0].step).toBe("middle");
    expect(o.series[1].step).toBeUndefined();
  });
});

const gappy = [["a", 1], ["b", null], ["c", 3]];
const optG = (cfg: Record<string, unknown>) =>
  buildCartesianOption(columns, gappy, {
    kind: "line", xAxis: "d", yMetrics: ["rev"], ...cfg,
  } as never) as never as { series: { data: unknown[]; connectNulls?: boolean }[] };

describe("missing values", () => {
  it("bridges the gap by default, as it always has", () => {
    const s = optG({}).series[0];
    expect(s.connectNulls).toBe(true);
    expect(s.data).toEqual([1, null, 3]);
  });

  it("leaves a break when asked", () => {
    const s = optG({ missing: "none" }).series[0];
    expect(s.connectNulls).toBe(false);
    expect(s.data).toEqual([1, null, 3]);
  });

  it("drops to zero when asked — ECharts has no such mode, so the data changes", () => {
    expect(optG({ missing: "zero" }).series[0].data).toEqual([1, 0, 3]);
  });

  it("never rewrites a bar's nulls — a missing bar and a zero bar say different things", () => {
    const o = buildCartesianOption(columns, gappy, {
      kind: "bar", xAxis: "d", yMetrics: ["rev"], missing: "zero",
    } as never) as never as { series: { data: unknown[] }[] };
    expect(o.series[0].data).toEqual([1, null, 3]);
  });
});
