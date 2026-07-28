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
