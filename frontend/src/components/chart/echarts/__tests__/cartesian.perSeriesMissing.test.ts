import { describe, it, expect } from "vitest";
import { buildCartesianOption } from "../options/cartesian";

/**
 * PER-SERIES "Replace missing values with" — Metabase's `line.missing`, which it keeps in the SERIES
 * settings and whose chart-wide twin it marks `// DEPRECATED: moved to series settings`
 * (`lib/settings/graph.ts`'s `LINE_SETTINGS`).
 *
 * This file exists because the divergence it closes had a written argument behind it, and the
 * argument was measurable. `decorateSeries` used to say that `missing` must stay chart-wide because
 * — unlike the other four `LineSeriesStyle` fields, which are appearance — `missing: "zero"`
 * REWRITES THE DATA, "and that zero then counts in stack totals, in normalisation and in a trend
 * fit", so two series disagreeing would make the chart say two things at once.
 *
 * TWO THIRDS OF THAT IS FALSE and the third is contained. Measured on `v = [1, null, 3, 4]` beside
 * `w = [10, 20, 30, 40]`, chart-wide, before anything here changed:
 *
 *   stack total   interpolate [11,20,33,44]   zero [11,20,33,44]   none [11,20,33,44]
 *   normalised w  [90.9, 100, 90.9, 90.9]     …identical…          …identical…
 *   trend of v    [1,2,3,4]                   [0.2,1.4,2.6,3.8]    [1,2,3,4]
 *
 * The stack total is byte-identical because `stackTotalSeries` sums with `typeof v === "number"`:
 * a null contributes nothing and a zero contributes zero, which is the same number. `normalizeStacks`
 * totals the same way, so neither the total nor any OTHER band's percentage moves — only the zeroed
 * band's own point, which draws at 0% where it would otherwise leave a gap. And the trend really does
 * move, but `trendSeries` fits each series independently off its own `data`, so a per-series `missing`
 * reaches that series' own trend line and nothing else.
 *
 * So there is no cross-series contamination to protect the reader from, and the four assertions below
 * are the ones that would have to fail before that conclusion could be revisited. THE FIRST THREE ARE
 * PINS on unchanged behaviour; the rest are the feature.
 */
const cols = [
  { name: "x", type: "varchar" },
  { name: "v", type: "double" },
  { name: "w", type: "double" },
];
const rows = [["a", 1, 10], ["b", null, 20], ["c", 3, 30], ["d", 4, 40]];

type Built = { series: { name?: string; data?: (number | null)[]; connectNulls?: boolean; __total?: boolean }[] };
const build = (extra: Record<string, unknown>) =>
  buildCartesianOption(cols, rows, {
    kind: "line", xAxis: "x", yMetrics: ["v", "w"], ...extra,
  } as never) as never as Built;
const dataOf = (o: Built, name: string) => o.series.find((s) => s.name === name)?.data;

describe("the measurement that retired the chart-wide-only argument", () => {
  it("a zeroed gap moves no stack total", () => {
    const totals = (missing: string) =>
      build({ kind: "area", stacked: true, showStackTotal: true, missing }).series.find((s) => s.__total)?.data;
    expect(totals("zero")).toEqual([11, 20, 33, 44]);
    expect(totals("zero")).toEqual(totals("interpolate"));
    expect(totals("zero")).toEqual(totals("none"));
  });

  it("a zeroed gap moves no OTHER series' normalised share", () => {
    const wOf = (missing: string) => dataOf(build({ kind: "area", stacked: true, normalized: true, missing }), "w");
    expect(wOf("zero")).toEqual(wOf("none"));
    // …and the zeroed band itself is the only thing that moves: a drawn 0%, not a gap.
    expect(dataOf(build({ kind: "area", stacked: true, normalized: true, missing: "zero" }), "v")?.[1]).toBe(0);
    expect(dataOf(build({ kind: "area", stacked: true, normalized: true, missing: "none" }), "v")?.[1]).toBe(null);
  });

  it("a zeroed gap moves ITS OWN trend and no other series'", () => {
    const trends = (missing: string) => ({
      v: dataOf(build({ trendLine: true, missing }), "v trend"),
      w: dataOf(build({ trendLine: true, missing }), "w trend"),
    });
    // The one real consequence, kept as the reason this is a per-series setting and not a free one.
    expect(trends("zero").v).not.toEqual(trends("none").v);
    // …and it stops at that series. `w` has no gap, so nothing about it can move.
    expect(trends("zero").w).toEqual(trends("none").w);
  });
});

describe("a series may set its own missing-value policy", () => {
  it("zeroes ONE band's gap and leaves the other's alone", () => {
    const opt = build({ missing: "none", seriesLine: { v: { missing: "zero" } } });
    expect(dataOf(opt, "v")).toEqual([1, 0, 3, 4]);
    // `w` has no gap of its own; the point is that it is still governed by the chart-wide "none".
    expect(opt.series.find((s) => s.name === "w")?.connectNulls).toBe(false);
    expect(opt.series.find((s) => s.name === "v")?.connectNulls).toBe(false);
  });

  it("connects ONE band while the chart leaves gaps", () => {
    const opt = build({ missing: "none", seriesLine: { v: { missing: "interpolate" } } });
    expect(opt.series.find((s) => s.name === "v")?.connectNulls).toBe(true);
    expect(opt.series.find((s) => s.name === "w")?.connectNulls).toBe(false);
    // Connect never rewrites data — the null stays a null and ECharts bridges it.
    expect(dataOf(opt, "v")).toEqual([1, null, 3, 4]);
  });

  it("falls back to the chart-wide value, then to interpolate", () => {
    // An entry that names OTHER fields must not be read as "this series chose a missing policy" —
    // `?? cfg.missing` is what makes the four fields independent of one another.
    const opt = build({ missing: "zero", seriesLine: { v: { size: "L" } } });
    expect(dataOf(opt, "v")).toEqual([1, 0, 3, 4]);
    // Nothing stored anywhere: Metabase's own default for `line.missing` is "interpolate".
    expect(build({}).series.find((s) => s.name === "v")?.connectNulls).toBe(true);
  });

  it("keeps the log-scale refusal per series", () => {
    // A log axis has no zero, so filling with one cannot mean what it says — the same guard the
    // chart-wide value has always carried, asked of the RESOLVED value so a per-series "zero" cannot
    // slip past it.
    const opt = build({ logScale: true, seriesLine: { v: { missing: "zero" } } });
    expect(dataOf(opt, "v")).toEqual([1, null, 3, 4]);
    expect(opt.series.find((s) => s.name === "v")?.connectNulls).toBe(false);
  });

  it("reaches a breakout band, which is the case a chart-wide setting cannot express at all", () => {
    const bcols = [{ name: "x", type: "varchar" }, { name: "g", type: "varchar" }, { name: "v", type: "double" }];
    const brows = [["a", "L", 1], ["a", "R", 5], ["b", "R", 6], ["c", "L", 3], ["c", "R", 7]];
    const opt = buildCartesianOption(bcols, brows, {
      kind: "line", xAxis: "x", series: "g", yMetrics: ["v"], missing: "none",
      seriesLine: { L: { missing: "zero" } },
    } as never) as never as Built;
    expect(opt.series.find((s) => s.name === "L")?.data).toEqual([1, 0, 3]);
    expect(opt.series.find((s) => s.name === "R")?.data).toEqual([5, 6, 7]);
  });
});
