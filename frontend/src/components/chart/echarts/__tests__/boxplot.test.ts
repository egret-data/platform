import { describe, it, expect } from "vitest";
import { labelOverhangRight, buildBoxplotOption, boxStats, pointsModeFor } from "../options/boxplot";
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

/**
 * The box plot's category axis is nominal too, and it had the SAME defect as bar and row — but by
 * a different route, which is why the sort in `options/cartesian.ts` did not reach it: this file
 * builds its categories from `[...groups.keys()]`, i.e. Map insertion order, i.e. the order the
 * engine happened to return the raw rows in.
 *
 * THE FIXTURE IS THE POINT, as it was for the cartesian charts. These are the two orderings
 * measured on `POST /charts/{id}/data` — seven calls in ten returned one, three the other — and
 * NEITHER is alphabetical, so a fixture already in the target order could not detect this.
 */
describe("a box plot's nominal axis draws in a stable order", () => {
  const bcols = [{ name: "category", type: "varchar" }, { name: "price", type: "double" }];
  // 3 calls in 10, and 7 calls in 10. Each category carries a distinct distribution so a category
  // landing in the wrong slot moves a visible box, not just a label.
  const arrivalA = [
    ["Widget", 40], ["Widget", 44], ["Gadget", 30], ["Gadget", 34],
    ["Doohickey", 10], ["Doohickey", 14], ["Gizmo", 20], ["Gizmo", 24],
  ];
  const arrivalB = [
    ["Gizmo", 20], ["Gizmo", 24], ["Widget", 40], ["Widget", 44],
    ["Gadget", 30], ["Gadget", 34], ["Doohickey", 10], ["Doohickey", 14],
  ];
  const sorted = ["Doohickey", "Gadget", "Gizmo", "Widget"];

  it("sorts the categories ascending, whichever order the rows arrived in", () => {
    for (const rows of [arrivalA, arrivalB]) {
      const opt = buildBoxplotOption(bcols, rows, { category: "category", value: "price" }) as any;
      expect(opt.xAxis.data).toEqual(sorted);
      // The BOXES follow their categories — medians 12, 32, 22, 42 in sorted order. A sorted axis
      // beside arrival-order boxes would relabel every box wrongly, which is worse than either.
      expect(opt.series[0].data.map((d: number[]) => d[2])).toEqual([12, 32, 22, 42]);
    }
  });

  it("draws the two arrival orders identically", () => {
    const of = (rows: unknown[][]) => JSON.stringify(
      buildBoxplotOption(bcols, rows, { category: "category", value: "price" }),
    );
    expect(of(arrivalA)).toBe(of(arrivalB));
  });

  /**
   * Reordering the CATEGORIES cannot move a VALUE. `boxStats` sorts each group's numbers itself
   * (`[...values].sort((a, b) => a - b)`) and every group is looked up by its own key, so neither
   * the order the categories are drawn in nor the order rows arrive WITHIN a category can reach a
   * quartile. Checked rather than asserted, because if it could this would be a bigger finding
   * than the ordering.
   */
  it("computes identical quartiles however the rows are shuffled, within a category or across", () => {
    const shuffledWithin = [
      ["Widget", 44], ["Doohickey", 14], ["Gizmo", 24], ["Gadget", 34],
      ["Gadget", 30], ["Widget", 40], ["Doohickey", 10], ["Gizmo", 20],
    ];
    const boxes = (rows: unknown[][]) =>
      (buildBoxplotOption(bcols, rows, { category: "category", value: "price" }) as any).series[0].data;
    expect(boxes(shuffledWithin)).toEqual(boxes(arrivalA));
    expect(boxes(shuffledWithin)).toEqual(boxes(arrivalB));
  });

  /**
   * The category column is not always a string. Sorting is delegated to `sortCategories`, the same
   * function the bar and row charts use, so the axis TYPE still decides: "10" sorts after "9"
   * numerically and before it as text, which is what makes this discriminating.
   */
  it("sorts a numeric category column numerically, not as text", () => {
    const ncols = [{ name: "bucket", type: "int" }, { name: "price", type: "double" }];
    const rows = [["10", 5], ["10", 7], ["9", 1], ["9", 3], ["2", 2], ["2", 4]];
    const opt = buildBoxplotOption(ncols, rows, { category: "bucket", value: "price" }) as any;
    expect(opt.xAxis.data).toEqual(["2", "9", "10"]);
  });

  /**
   * The other side of the same rule. Sorting exists because a spec carrying `orderby: []` leaves
   * arrival order to the engine's whim. A spec that carries a NON-EMPTY `orderby` is the opposite
   * case: the engine has already put the rows in the order its author asked for, and re-sorting
   * them here would make the sort appear to do nothing.
   *
   * The same fixtures, deliberately: both are the measured orderings, NEITHER is alphabetical, and
   * they differ from each other — so this cannot pass by accident in either direction, and it fails
   * loudly if `preserve` is wired to the wrong polarity.
   */
  it("keeps the arrival order when the caller says the query already sorted", () => {
    for (const rows of [arrivalA, arrivalB]) {
      const opt = buildBoxplotOption(bcols, rows, {
        category: "category", value: "price", preserveCategoryOrder: true,
      }) as any;
      expect(opt.xAxis.data).toEqual(rows.filter((_, i) => i % 2 === 0).map((r) => r[0]));
    }
  });

  it("keeps each box under its own category when it preserves the order", () => {
    const opt = buildBoxplotOption(bcols, arrivalA, {
      category: "category", value: "price", preserveCategoryOrder: true,
    }) as any;
    expect(opt.xAxis.data).toEqual(["Widget", "Gadget", "Doohickey", "Gizmo"]);
    // Medians follow their categories: Widget 42, Gadget 32, Doohickey 12, Gizmo 22.
    expect(opt.series[0].data.map((d: number[]) => d[2])).toEqual([42, 32, 12, 22]);
  });

  // A temporal or numeric category axis is NOT a nominal one, and `sortCategories` orders those
  // branches whatever `preserve` says — chronology and magnitude are not opinions the query can
  // override. Pinned here because the box plot is the caller most likely to be handed a numeric
  // bucket column.
  it("still orders a numeric category column even when asked to preserve", () => {
    const ncols = [{ name: "bucket", type: "int" }, { name: "price", type: "double" }];
    const rows = [["10", 5], ["10", 7], ["9", 1], ["9", 3], ["2", 2], ["2", 4]];
    const opt = buildBoxplotOption(ncols, rows, {
      category: "bucket", value: "price", preserveCategoryOrder: true,
    }) as any;
    expect(opt.xAxis.data).toEqual(["2", "9", "10"]);
  });
});

/**
 * THE CATEGORY IS A VALUE, AND IT HAS A FORMAT — the fifth chart family to need this said out loud.
 *
 * `styleColumnsFor` falls through to `return resultColumns` for a box plot, so the CATEGORY column
 * reaches `FormatSection` and gets a Date style select of its own; `buildBoxplotOption` read
 * `columnSettings?.[cfg.value]` and nothing for `cfg.category`, so everything typed there was
 * stored and drawn nowhere. Measured in the browser over `MB Monthly`: the panel read
 * `Date style: January 2024` while the axis and the aria description still read `2026-01-01`.
 *
 * Metabase formats it. `echarts/boxplot/model/index.ts`'s `createXAxisFormatter` spreads
 * `settings.column?.(dimensionColumn)` into `formatValue` for every tick — its box plot has a
 * formatter on this axis where egret's had none — so the rule is copied, not invented.
 *
 * `xAxis.data` STAYS RAW and is asserted so in every case below. It is the drill's identity:
 * `BoxplotChart` reads the clicked category back off `xAxis.data[dataIndex]` and hands it to
 * `onValueClick` as the filter value, so formatting that array is the drill-on-display-strings
 * defect this programme has now had in four families. The label is a `formatter`; the data is not.
 */
describe("buildBoxplotOption — the category's own format", () => {
  const dated = [{ name: "d", type: "timestamp" }, { name: "v", type: "double" }];
  const datedRows = [["2026-01-01", 10], ["2026-01-01", 20], ["2026-02-01", 5], ["2026-02-01", 7]];
  const labelOf = (opt: any, i: number) => opt.xAxis.axisLabel.formatter(opt.xAxis.data[i], i);

  it("draws each tick through the category's stored format", () => {
    const opt = buildBoxplotOption(dated, datedRows, {
      category: "d", value: "v", columnSettings: { d: { date_style: "MMMM YYYY" } },
    }) as any;
    expect([labelOf(opt, 0), labelOf(opt, 1)]).toEqual(["January 2026", "February 2026"]);
    expect(opt.xAxis.data).toEqual(["2026-01-01", "2026-02-01"]);
  });

  it("carries a text category's prefix", () => {
    const opt = buildBoxplotOption(cols, rows, {
      category: "g", value: "v", columnSettings: { g: { prefix: "Cat " } },
    }) as any;
    expect(labelOf(opt, 0)).toBe("Cat A");
    expect(opt.xAxis.data).toEqual(["A", "B"]);
  });

  // The same rename hazard `format.ts`'s `specifiesDateRendering` exists for: `{column_title}` is
  // truthy and silent about dates, and handing it to `formatValue` on a temporal column takes
  // `formatDate`'s DEFAULT style — so renaming the column would restyle every tick.
  it("is not restyled by a format that says nothing about dates", () => {
    const opt = buildBoxplotOption(dated, datedRows, {
      category: "d", value: "v", columnSettings: { d: { column_title: "Month" } },
    }) as any;
    expect(labelOf(opt, 0)).toBe("2026-01-01");
  });

  // The tooltip heading and the spoken description are the same label the axis draws. A chart that
  // formats its axis and then reads the raw key aloud has two names for one category.
  it("uses the formatted category in the tooltip heading and the aria description", () => {
    const opt = buildBoxplotOption(dated, datedRows, {
      category: "d", value: "v", columnSettings: { d: { date_style: "MMMM YYYY" } },
    }) as any;
    expect(opt.tooltip.formatter({ seriesType: "boxplot", dataIndex: 0 })).toContain("January 2026");
    expect(opt.aria.label.description).toContain("January 2026");
    expect(opt.aria.label.description).not.toContain("2026-01-01");
  });
});

/**
 * KI-165: the last box's value label ran past the card's right edge and was clipped ("31.6" for
 * "31.6K"). The reserve is solved for, so after reserving it the label ends exactly inside.
 */
describe("labelOverhangRight", () => {
  const ends = (o: { labelWidth: number; distance: number; plotWidth: number; categories: number; gridRight: number }, extra: number) =>
    o.distance + o.labelWidth - (o.plotWidth - extra) / (2 * o.categories) - (o.gridRight + extra);

  it("reserves nothing when the label fits in the last half band and the margin", () => {
    expect(labelOverhangRight({ labelWidth: 30, distance: 34, plotWidth: 700, categories: 4, gridRight: 16 })).toBe(0);
  });

  it("reserves enough that the label no longer runs past the edge, after the plot narrows", () => {
    const o = { labelWidth: 40, distance: 34, plotWidth: 260, categories: 4, gridRight: 16 };
    const extra = labelOverhangRight(o);
    expect(extra).toBeGreaterThan(0);
    expect(ends(o, extra)).toBeLessThanOrEqual(0);
    expect(ends(o, extra - 1)).toBeGreaterThan(0);
  });

  it("reserves nothing it cannot measure", () => {
    expect(labelOverhangRight({ labelWidth: 0, distance: 34, plotWidth: 260, categories: 4, gridRight: 16 })).toBe(0);
    expect(labelOverhangRight({ labelWidth: 40, distance: 34, plotWidth: 260, categories: 0, gridRight: 16 })).toBe(0);
  });
});
