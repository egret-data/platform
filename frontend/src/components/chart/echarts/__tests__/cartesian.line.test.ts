import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";
import { CHART_THEME } from "../palette";

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

  it("cannot zero-fill on a log axis, so it leaves the gap instead", () => {
    const s = optG({ missing: "zero", logScale: true }).series[0];
    expect(s.data).toEqual([1, null, 3]);
    expect(s.connectNulls).toBe(false);
  });

  it("fits a trend through zero-filled points, because zero is what the user said the value was", () => {
    const s = optG({ missing: "zero", trendLine: true }) as never as { series: { name: string; data: unknown[] }[] };
    const trend = s.series.find((x) => x.name?.endsWith(" trend"));
    expect(trend).toBeDefined();
    // Least squares through the real (0,1) and (2,3) points alone — as if the gap were still
    // absent rather than a real 0 — fits exactly 1 at i=0. Through (0,1),(1,0),(2,3) it fits
    // 1/3 instead. Asserting the latter pins the decision that a zero-filled point is a real
    // data point to the fit, not a value the trend is allowed to see through.
    expect(trend!.data[0]).toBeCloseTo(1 / 3, 5);
  });
});

const manyRows = Array.from({ length: 60 }, (_, i) => [`d${i}`, i + 1]);
const optMany = (cfg: Record<string, unknown>) =>
  buildCartesianOption(columns, manyRows, {
    kind: "line", xAxis: "d", yMetrics: ["rev"], ...cfg,
  } as never) as never as { series: { showSymbol: boolean }[] };

describe("markers", () => {
  it("shows them on a short series under auto", () => {
    expect(opt({}).series[0].showSymbol).toBe(true);
  });

  it("hides them on a long series under auto — they would read as a lumpy line", () => {
    expect(optMany({}).series[0].showSymbol).toBe(false);
  });

  it("obeys an explicit choice in both directions", () => {
    expect(opt({ markers: "off" }).series[0].showSymbol).toBe(false);
    expect(optMany({ markers: "on" }).series[0].showSymbol).toBe(true);
  });

  it("lets a series override the chart", () => {
    expect(opt({ markers: "off", seriesLine: { rev: { markers: "on" } } }).series[0].showSymbol).toBe(true);
    expect(optMany({ markers: "on", seriesLine: { rev: { markers: "off" } } }).series[0].showSymbol).toBe(false);
  });

  it("counts the points actually plotted, not the rows fetched", () => {
    // A breakout splits rows across series, so each series is shorter than the result set.
    const cols = [{ name: "d", type: "varchar" }, { name: "cat", type: "varchar" }, { name: "rev", type: "double" }];
    const rs = Array.from({ length: 60 }, (_, i) => [`d${i % 20}`, i % 3 === 0 ? "x" : "y", i]);
    const o = buildCartesianOption(cols, rs, {
      kind: "line", xAxis: "d", series: "cat", yMetrics: ["rev"],
    } as never) as never as { series: { showSymbol: boolean; data: unknown[] }[] };
    // Each series has 20 points, well under the threshold, even though 60 rows came back.
    expect(o.series[0].data.length).toBeLessThanOrEqual(20);
    expect(o.series[0].showSymbol).toBe(true);
  });

  it("decides on the folded point count, not the count before folding", () => {
    const rs = Array.from({ length: 60 }, (_, i) => [`d${i}`, i + 1]);
    const o = buildCartesianOption(columns, rs, {
      kind: "line", xAxis: "d", yMetrics: ["rev"], maxCategories: 8,
    } as never) as never as { series: { showSymbol: boolean; data: unknown[] }[] };
    expect(o.series[0].data.length).toBe(8);
    expect(o.series[0].showSymbol).toBe(true);
  });

  it("does not count nulls — a sparse line is not a crowded one", () => {
    const rs = Array.from({ length: 60 }, (_, i) => [`d${i}`, i % 12 === 0 ? i + 1 : null]);
    const o = buildCartesianOption(columns, rs, {
      kind: "line", xAxis: "d", yMetrics: ["rev"],
    } as never) as never as { series: { showSymbol: boolean }[] };
    expect(o.series[0].showSymbol).toBe(true);
  });
});

describe("missing zero and stacking", () => {
  const cols = [...columns, { name: "qty", type: "double" }];
  const rs = [["a", 1, 4], ["b", null, 4], ["c", 3, 4]];

  it("counts a zero-filled point in a normalized stack, rather than leaving it absent", () => {
    const o = buildCartesianOption(cols, rs, {
      kind: "area", xAxis: "d", yMetrics: ["rev", "qty"],
      missing: "zero", stacked: true, normalized: true,
    } as never) as never as { series: { name?: string; data: unknown[] }[] };
    const rev = o.series.find((x) => x.name === "rev")!;
    const qty = o.series.find((x) => x.name === "qty")!;
    // Without the zero-fill, rev's gap stays null and normalizeStacks leaves it null too
    // (it only rewrites numeric values). With the fill it is 0% of the b-column total,
    // not an absent slice.
    expect(rev.data[1]).toBe(0);
    expect(qty.data[1]).toBe(100);
  });

  it("contributes 0 to the stack total, not null as if the point were absent", () => {
    const o = buildCartesianOption(columns, gappy, {
      kind: "area", xAxis: "d", yMetrics: ["rev"],
      missing: "zero", stacked: true, showStackTotal: true,
    } as never) as never as { series: { __total?: boolean; data: unknown[] }[] };
    const total = o.series.find((x) => x.__total);
    expect(total).toBeDefined();
    // With a single series, an unfilled null contributes nothing and stackTotalSeries would
    // record the column's total as null (no numeric value seen). A filled 0 is numeric, so
    // the total is 0 — present and countable, not skipped.
    expect(total!.data[1]).toBe(0);
  });
});

describe("per-series line appearance", () => {
  it("keeps the automatic dash cycle when nothing overrides it", () => {
    // The cycle lets colour-blind users tell lines apart; it is not decoration.
    const cols = [{ name: "d", type: "varchar" }, { name: "rev", type: "double" }, { name: "qty", type: "double" }];
    const rs = [["a", 1, 2], ["b", 2, 3]];
    const o = buildCartesianOption(cols, rs, {
      kind: "line", xAxis: "d", yMetrics: ["rev", "qty"],
    } as never) as never as { series: { lineStyle: { type: string } }[] };
    expect(o.series[0].lineStyle.type).not.toBe(o.series[1].lineStyle.type);
  });

  it("lets a series choose its own dash style", () => {
    const s = opt({ seriesLine: { rev: { style: "dotted" } } }).series[0] as { lineStyle: { type: string } };
    expect(s.lineStyle.type).toBe("dotted");
  });

  it("overrides only the series it names, leaving the cycle for the rest", () => {
    // rev is colorIdx 0, whose natural cycle value is already "solid" — asserting "solid"
    // here would pass even with the override deleted. Pick a value the cycle would not
    // produce, so the assertion can only pass because the override actually applied.
    const cols = [{ name: "d", type: "varchar" }, { name: "rev", type: "double" }, { name: "qty", type: "double" }];
    const rs = [["a", 1, 2], ["b", 2, 3]];
    const o = buildCartesianOption(cols, rs, {
      kind: "line", xAxis: "d", yMetrics: ["rev", "qty"], seriesLine: { rev: { style: "dotted" } },
    } as never) as never as { series: { lineStyle: { type: string } }[] };
    expect(o.series[0].lineStyle.type).toBe("dotted");
    expect(o.series[1].lineStyle.type).toBe("dashed");
  });

  it("maps size to a width, with M unchanged from the theme", () => {
    const w = (cfg: Record<string, unknown>) =>
      (opt(cfg).series[0] as { lineStyle: { width: number } }).lineStyle.width;
    expect(w({ seriesLine: { rev: { size: "M" } } })).toBe(w({}));
    expect(w({ seriesLine: { rev: { size: "L" } } })).toBeGreaterThan(w({}));
    expect(w({ seriesLine: { rev: { size: "S" } } })).toBeLessThan(w({}));
  });

  it("falls back to the theme width for an unknown size, rather than an undefined one", () => {
    // Settings JSON isn't whitelisted server-side, so a stored size the type never promised
    // (e.g. a stale "XL") must not reach zrender's merge as `undefined` — that would silently
    // overwrite ECharts' own default width.
    const s = opt({ seriesLine: { rev: { size: "XL" as never } } }).series[0] as { lineStyle: { width: number } };
    expect(s.lineStyle.width).toBe(CHART_THEME.lineWidth);
  });
});
