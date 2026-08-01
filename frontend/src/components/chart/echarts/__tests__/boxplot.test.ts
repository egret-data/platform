import { describe, it, expect } from "vitest";
import { buildBoxplotOption, boxStats, pointsModeFor } from "../options/boxplot";
import { SERIES_PALETTE } from "../palette";

const cols = [{ name: "g", type: "varchar" }, { name: "v", type: "double" }];
const rows = [["A", 1], ["A", 2], ["A", 3], ["A", 4], ["A", 5], ["B", 10], ["B", 20], ["B", 30]];
/** A: 1..5 plus a far outlier. Q1=2, Q3=4, IQR=2 → high fence 7, so 100 is out. */
const withOutlier = [["A", 1], ["A", 2], ["A", 3], ["A", 4], ["A", 5], ["A", 100]];

const seriesOfType = (opt: any, type: string) => opt.series.filter((s: any) => s.type === type);
const pointSeries = (opt: any) => seriesOfType(opt, "scatter");

describe("buildBoxplotOption", () => {
  it("computes a five-number summary per category", () => {
    const opt = buildBoxplotOption(cols, rows, { category: "g", value: "v" }) as any;
    expect(opt.series[0].type).toBe("boxplot");
    expect(opt.xAxis.data).toEqual(["A", "B"]);
    expect(opt.series[0].data[0]).toEqual([1, 2, 3, 4, 5]);     // A: 1..5
    expect(opt.series[0].data[1]).toEqual([10, 15, 20, 25, 30]); // B: 10,20,30 (interpolated quartiles)
  });

  it("ignores non-finite values", () => {
    const opt = buildBoxplotOption(cols, [["A", 1], ["A", NaN], ["A", 3]], { category: "g", value: "v" }) as any;
    expect(opt.series[0].data[0]).toEqual([1, 1.5, 2, 2.5, 3]);
  });

  it("empty when category or value is missing", () => {
    expect((buildBoxplotOption(cols, rows, {}) as any).series[0].data).toEqual([]);
  });

  it("separates 1.5·IQR outliers and plots the mean (Metabase parity)", () => {
    const opt = buildBoxplotOption(cols, withOutlier, { category: "g", value: "v" }) as any;
    expect(opt.series[0].data[0][4]).toBe(5);          // whisker high stays at 5, not 100
    const points = pointSeries(opt);
    expect(points[0].data).toEqual([[0, 100]]);        // the outlier point
    const mean = points.find((s: any) => s.symbol === "diamond");
    expect(mean.data[0][1]).toBeCloseTo((1 + 2 + 3 + 4 + 5 + 100) / 6, 6);
  });

  // Metabase's `boxplot.whisker_type`. With Min/Max the whiskers reach the extremes, so by
  // construction nothing is an outlier and the outlier series disappears entirely.
  it("reaches the extremes and drops the outliers on Min/Max whiskers", () => {
    const opt = buildBoxplotOption(cols, withOutlier, { category: "g", value: "v", whiskers: "min-max" }) as any;
    expect(opt.series[0].data[0][0]).toBe(1);
    expect(opt.series[0].data[0][4]).toBe(100);
    // Asserted on `boxStats` directly, not on the drawn series: `pointsModeFor` already forces
    // "none" under Min/Max, so an empty outlier SERIES would pass even if the statistics still
    // reported outliers. This is the assertion that survived its own mutation the first time.
    expect(boxStats([1, 2, 3, 4, 5, 100], "min-max").outliers).toEqual([]);
    expect(boxStats([1, 2, 3, 4, 5, 100]).outliers).toEqual([100]);
  });

  // Metabase's `boxplot.points_mode`.
  it("draws no points, the outliers, or every point", () => {
    const of = (points: "none" | "outliers" | "all") =>
      pointSeries(buildBoxplotOption(cols, withOutlier, { category: "g", value: "v", points, showMean: false }) as any);
    expect(of("none")).toHaveLength(0);
    expect(of("outliers")[0].data).toEqual([[0, 100]]);
    // "All" keeps the outliers as their own, more opaque series and adds the other five faintly.
    const all = of("all");
    expect(all.flatMap((s: any) => s.data)).toHaveLength(6);
  });

  // With Min/Max whiskers there is nothing for "Outliers only" to draw, so Metabase removes the
  // option and reads a stored "outliers" as "none". Without that the panel shows a selected choice
  // that can only ever produce an empty series.
  it("treats a stored 'outliers' as 'none' under Min/Max whiskers", () => {
    expect(pointsModeFor({ whiskers: "min-max", points: "outliers" })).toBe("none");
    expect(pointsModeFor({ whiskers: "min-max", points: "all" })).toBe("all");
    expect(pointsModeFor({})).toBe("outliers");
  });

  it("hides the mean when asked", () => {
    const opt = buildBoxplotOption(cols, rows, { category: "g", value: "v", showMean: false }) as any;
    expect(pointSeries(opt).some((s: any) => s.symbol === "diamond")).toBe(false);
  });

  /**
   * Every mark derives from ONE colour, as Metabase derives them: the fill is that colour at 15%,
   * the mean is it darkened, the outliers are it. egret hard-coded a pale green fill beside a green
   * border — two hexes that agree for exactly one colour and part company the moment it changes.
   */
  it("derives the fill, border and mean from a single colour", () => {
    const opt = buildBoxplotOption(cols, withOutlier, { category: "g", value: "v", color: "#3366CC" }) as any;
    expect(opt.series[0].itemStyle.borderColor).toBe("#3366CC");
    expect(opt.series[0].itemStyle.color).toBe("rgba(51, 102, 204, 0.15)");
    const mean = pointSeries(opt).find((s: any) => s.symbol === "diamond");
    // The literal, not a re-run of `scaleLightness` — a test that recomputes the formula it is
    // checking passes for any formula. `#3366CC` darkened by Metabase's 0.3 is this, measured once.
    // Asserting only "not the border colour" let an unrelated palette slot through.
    expect(mean.itemStyle.color).toBe("#24478f");
    const outliers = pointSeries(opt).find((s: any) => s.symbol !== "diamond");
    expect(outliers.itemStyle.color).toBe("#3366CC");
  });

  it("falls back to the first palette colour", () => {
    const opt = buildBoxplotOption(cols, rows, { category: "g", value: "v" }) as any;
    expect(opt.series[0].itemStyle.borderColor).toBe(SERIES_PALETTE[0]);
  });

  it("titles both axes from the columns, and lets the config override or hide either", () => {
    const auto = buildBoxplotOption(cols, rows, { category: "g", value: "v" }) as any;
    expect(auto.xAxis.name).toBe("g");
    expect(auto.yAxis.name).toBe("v");
    const named = buildBoxplotOption(cols, rows, { category: "g", value: "v", xLabel: "Group", showYLabel: false }) as any;
    expect(named.xAxis.name).toBe("Group");
    expect(named.yAxis.name).toBeUndefined();
  });

  it("draws a goal line when one is set", () => {
    const opt = buildBoxplotOption(cols, rows, { category: "g", value: "v", goalValue: 12, goalLabel: "Target" }) as any;
    expect(opt.series[0].markLine.data).toEqual([{ yAxis: 12 }]);
    expect(opt.series[0].markLine.label.formatter).toBe("Target");
  });

  // Metabase labels the median by default and all five statistics on "All".
  it("labels the median only, or every statistic", () => {
    const median = buildBoxplotOption(cols, rows, { category: "g", value: "v", showLabels: true, showMean: false }) as any;
    const labels = pointSeries(median).find((s: any) => s.symbolSize === 0);
    expect(labels.data).toEqual([[0, 3], [1, 20]]);
    const all = buildBoxplotOption(cols, rows, { category: "g", value: "v", showLabels: true, labelMode: "all", showMean: false }) as any;
    expect(pointSeries(all).find((s: any) => s.symbolSize === 0).data).toHaveLength(10); // 5 per box
  });

  // Metabase's `graph.label_value_formatting` defaults to COMPACT on a box plot — a median printed
  // as 87.957 beside a 47px box is why. The axis keeps the shared "compact once wide" rule.
  it("shortens the data labels by default, and stops when told to", () => {
    const labelsOf = (cfg: Record<string, unknown>) => {
      const opt = buildBoxplotOption(cols, [["A", 1234.5678], ["A", 2345.6789]],
        { category: "g", value: "v", showLabels: true, showMean: false, ...cfg }) as any;
      const s = pointSeries(opt).find((x: any) => x.symbolSize === 0);
      return s.label.formatter({ data: s.data[0] });
    };
    expect(labelsOf({})).toBe("1.8K");
    expect(labelsOf({ compactValues: false })).toContain("1,790");
  });

  it("draws no label series at all when values are off", () => {
    const opt = buildBoxplotOption(cols, rows, { category: "g", value: "v" }) as any;
    expect(pointSeries(opt).some((s: any) => s.symbolSize === 0)).toBe(false);
  });

  it("clamps the box width the way Metabase does", () => {
    const opt = buildBoxplotOption(cols, rows, { category: "g", value: "v" }) as any;
    expect(opt.series[0].boxWidth).toEqual([3, 60]);
  });
});

describe("boxStats", () => {
  /**
   * The invariant a box plot must never break: a whisker cannot end inside its own box, and cannot
   * reach out to a point the fences just excluded.
   *
   * HONEST NOTE: Metabase's `Math.min(rawWhiskerMin, q1)` / `Math.max(rawWhiskerMax, q3)` clamp and
   * its fall back to the quartile are copied, but **I could not construct an input where either
   * one changes the answer** — the smallest in-range point is provably ≤ Q1 for every case I tried,
   * and `inRange` is never empty because Q1 and Q3 always lie between the fences. They are kept
   * because they are Metabase's, cost nothing, and cannot be wrong; this test states the invariant
   * rather than pretending to guard a branch I cannot reach. A mutation removing the clamp does NOT
   * fail here, and that is reported rather than hidden.
   */
  it("keeps every whisker outside its box and inside the fences", () => {
    for (const values of [[5, 5, 5, 5, 100], [1, 2, 3, 4, 5, 100], [-1000, 10, 11, 12, 13], [0, 90, 100, 100, 100]]) {
      const s = boxStats(values);
      expect(s.box[0]).toBeLessThanOrEqual(s.box[1]);    // whisker low ≤ Q1
      expect(s.box[4]).toBeGreaterThanOrEqual(s.box[3]); // whisker high ≥ Q3
      for (const o of s.outliers) {
        expect(o < s.box[0] || o > s.box[4]).toBe(true); // no whisker reaches an outlier
      }
    }
  });

  it("interpolates quartiles linearly", () => {
    expect(boxStats([10, 20, 30]).box).toEqual([10, 15, 20, 25, 30]);
  });

  it("survives a single value", () => {
    const s = boxStats([7]);
    expect(s.box).toEqual([7, 7, 7, 7, 7]);
    expect(s.mean).toBe(7);
  });
});
