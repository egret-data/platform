import { describe, it, expect } from "vitest";
import {
  buildCartesianOption, resolveXScale, xScaleFitsColumn, type CartesianBuildConfig,
} from "../options/cartesian";

/**
 * A SCALE THE X COLUMN CANNOT CARRY MUST NOT BLANK THE CHART.
 *
 * Measured live on the parity harness's `MB · Bar`, whose x is a text column: egret's Axes tab
 * offered Ordinal / Linear / Timeseries all enabled, and picking Timeseries drew an entirely empty
 * plot area — no bars, no y-axis values, no x tick labels. Two clicks, and the card stays broken
 * once saved.
 *
 * The mechanism is `applyXScale`: it re-keys every point onto a real x coordinate with
 * `Date.parse(String(c))` for a timeseries and `Number(c)` for a linear one, then DROPS the points
 * whose coordinate is not finite. On a column of category names every one of them is dropped, so
 * ECharts is handed a series with no data on an axis with no extent.
 *
 * The fix is in `resolveXScale` rather than in the panel, and deliberately: #407 established on
 * Decimal places that fixing the control does not reach a chart that already has the bad value
 * stored. Every reader of the scale — the axis, `applyXScale`, `foldable`, the panel's own rotation
 * rule — goes through this one function, so refusing there is the only place that reaches a saved
 * card.
 */

const textCols = [{ name: "category", type: "varchar" }, { name: "rev", type: "double" }];
const textRows = [["Doohickey", 10], ["Gadget", 7], ["Gizmo", 2]];

const dateCols = [{ name: "d", type: "date" }, { name: "rev", type: "double" }];
const dateRows = [["2024-01-01", 10], ["2024-02-01", 7], ["2024-03-01", 2]];

const numCols = [{ name: "price", type: "double" }, { name: "rev", type: "double" }];
const numRows = [[1.5, 10], [2.5, 7], [3.5, 2]];

/** The RESULT is opened up for reading; `cfg` deliberately is not — see the same helper in
 *  `scatterValueX.echarts.test.ts` for why a loose config argument silences `tsc` on the field
 *  names and builds a different chart than the test's name says. */
const build = (cols: { name: string; type: string }[], rows: unknown[][], cfg: CartesianBuildConfig): any =>
  buildCartesianOption(cols, rows, cfg);

describe("an x-scale the column cannot carry", () => {
  /** THE MEASURED DEFECT, at the layer that reaches an already-saved chart. Asserted on what
   *  ECharts is handed — categories on the axis and a value per category — because "the option has
   *  a `category` axis type" alone would pass on a build that still dropped every point. */
  it.each(["timeseries", "linear"] as const)(
    "still draws every point when a text column carries a stored %s scale",
    (xScale) => {
      const o = build(textCols, textRows, { kind: "bar", xAxis: "category", yMetrics: ["rev"], xScale });
      // The data assertion leads, because the blank plot IS the defect — an axis type assertion
      // alone would pass on a build that still handed ECharts an empty series.
      expect(o.series[0].data).toEqual([10, 7, 2]);
      expect(o.xAxis.type).toBe("category");
      expect(o.xAxis.data).toEqual(["Doohickey", "Gadget", "Gizmo"]);
    },
  );

  /** The control for the pair above: the refusal is about the COLUMN, not about the scale. A date
   *  column asked for a timeseries still gets one, points and all. */
  it("honours a timeseries scale on a temporal column", () => {
    const o = build(dateCols, dateRows, { kind: "line", xAxis: "d", yMetrics: ["rev"], xScale: "timeseries" });
    expect(o.xAxis.type).toBe("time");
    expect(o.series[0].data).toHaveLength(3);
  });

  it("honours a linear scale on a numeric column", () => {
    const o = build(numCols, numRows, { kind: "line", xAxis: "price", yMetrics: ["rev"], xScale: "linear" });
    expect(o.xAxis.type).toBe("value");
    expect(o.series[0].data).toHaveLength(3);
  });

  /** A number is not a date. `Date.parse("15.69")` is NaN, so a timeseries over a numeric column
   *  empties the plot exactly as it does over a text one — and Metabase does not offer Timeseries
   *  on a numeric column either (its scale options there are Linear / Power / Log / Ordinal). */
  it("refuses a timeseries scale on a numeric column rather than dropping every point", () => {
    const o = build(numCols, numRows, { kind: "line", xAxis: "price", yMetrics: ["rev"], xScale: "timeseries" });
    expect(o.xAxis.type).not.toBe("time");
    expect(o.series[0].data).toHaveLength(3);
  });

  /** Ordinal is always available: every column can be drawn as a sequence of categories, which is
   *  why it is the answer the refusal falls back to. */
  it("leaves an explicit ordinal scale alone on every column type", () => {
    for (const [cols, rows, x] of [[textCols, textRows, "category"], [numCols, numRows, "price"], [dateCols, dateRows, "d"]] as const) {
      const o = build(cols as never, rows as never, { kind: "bar", xAxis: x, yMetrics: ["rev"], xScale: "ordinal" });
      expect(o.xAxis.type).toBe("category");
      expect(o.series[0].data).toHaveLength(3);
    }
  });
});

describe("resolveXScale", () => {
  it("drops a stored scale the column cannot carry", () => {
    expect(resolveXScale("bar", "timeseries", "varchar")).toBeUndefined();
    expect(resolveXScale("bar", "linear", "varchar")).toBeUndefined();
    expect(resolveXScale("bar", "timeseries", "double")).toBeUndefined();
    expect(resolveXScale("bar", "linear", "date")).toBeUndefined();
  });

  it("keeps a stored scale the column can carry", () => {
    expect(resolveXScale("bar", "timeseries", "timestamp")).toBe("timeseries");
    expect(resolveXScale("bar", "linear", "bigint")).toBe("linear");
    expect(resolveXScale("bar", "ordinal", "varchar")).toBe("ordinal");
  });

  /** Before a query has run there is no result column and so no type to judge against. Refusing on
   *  a guess would silently rewrite a setting the user made; the honest answer is to keep it. */
  it("keeps a stored scale when the column type is not known", () => {
    expect(resolveXScale("bar", "timeseries", undefined)).toBe("timeseries");
    expect(resolveXScale("bar", "linear", undefined)).toBe("linear");
  });

  /** A refused scale falls through to the automatic answer rather than to `ordinal` outright — so a
   *  scatter over a numeric column that stored `timeseries` still gets the value axis #403 gave it,
   *  not the 196-category axis that predates it. */
  it("a scatter whose refused scale falls through still gets its automatic value axis", () => {
    expect(resolveXScale("scatter", "timeseries", "double")).toBe("linear");
    expect(resolveXScale("scatter", "linear", "varchar")).toBeUndefined();
  });
});

describe("xScaleFitsColumn", () => {
  it("answers the same question the select uses to grey its items out", () => {
    expect(xScaleFitsColumn("ordinal", "varchar")).toBe(true);
    expect(xScaleFitsColumn("linear", "varchar")).toBe(false);
    expect(xScaleFitsColumn("timeseries", "varchar")).toBe(false);
    expect(xScaleFitsColumn("linear", "double")).toBe(true);
    expect(xScaleFitsColumn("timeseries", "double")).toBe(false);
    expect(xScaleFitsColumn("timeseries", "timestamp")).toBe(true);
    expect(xScaleFitsColumn("linear", "timestamp")).toBe(false);
  });

  /** Nothing is greyed out while the column's type is unknown — the panel cannot honestly say a
   *  scale will not work when it does not know what it would be applied to. */
  it("permits everything when the column type is unknown", () => {
    expect(xScaleFitsColumn("linear", undefined)).toBe(true);
    expect(xScaleFitsColumn("timeseries", undefined)).toBe(true);
  });
});
