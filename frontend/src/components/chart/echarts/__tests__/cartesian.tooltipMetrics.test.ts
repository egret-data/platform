import { describe, it, expect } from "vitest";
import { buildCartesianOption, type CartesianBuildConfig } from "../options/cartesian";

/**
 * ADDITIONAL TOOLTIP METRICS — a metric fetched on purpose and drawn nowhere but the tooltip.
 *
 * WHY THIS IS METRICS AND NOT METABASE'S COLUMNS. Metabase's "Additional tooltip columns"
 * (`graph.tooltip_columns`, `lib/settings/graph.ts:420`) offers only columns ALREADY IN THE RESULT
 * and never touches the query — `getAvailableAdditionalColumns` filters `rawSeries[].data.cols`
 * against the columns the chart already references (`shared/settings/cartesian-chart.ts:458`). That
 * feature is possible there because Metabase's notebook and its viz settings are separate: the
 * question can return more than the chart plots. egret's `buildSpec` derives the query FROM the viz
 * config, so that divergence cannot arise and the option list would be EMPTY on every egret chart
 * that has a tooltip. Porting it verbatim would mean adding a raw column, which either changes the
 * `GROUP BY` (a different chart) or has to be wrapped in `any_value()` server-side, which is
 * nondeterministic — "Revenue by Category, also show Product Name" would show an arbitrary product.
 *
 * WHAT MAKES THE METRIC VERSION CHEAP is a property of this builder measured before it was built:
 * SERIES COME FROM `cfg.yMetrics`, NOT FROM THE RESULT COLUMNS. A result column nobody listed in
 * `yMetrics` is fetched and draws nothing — no series, no legend entry, no axis extent. So
 * `buildSpec` appends the tooltip metrics to `spec.metrics` while leaving them out of `yMetrics`,
 * the engine aggregates them under the SAME `GROUP BY` (`chartquery.rs:529` emits one
 * `<expression> AS <name>` per `spec.metrics` entry and takes the grouping from `spec.dimensions`
 * alone), and only the tooltip reads them. Nothing is nondeterministic because a metric is an
 * aggregate the dataset's author wrote, which is exactly the property Metabase's extra column has
 * and a raw column does not.
 *
 * THE LOOKUP IS BY CATEGORY STRING AND NOT BY `dataIndex`, and that is not a stylistic choice.
 * `applyXScale` FILTERS: a datum whose x will not parse is dropped, so on a value/time x-axis the
 * series' data array is no longer index-aligned with the category list. `ariaPoints` carries the
 * scar of the same mistake — a zip that "does not read as NaN, it reads as a correct-looking
 * sentence". Both shapes a datum takes carry the raw category (`{value: [x, y], name}` since #387;
 * the ordinal axis' own `xAxis.data` entry otherwise), and both surface as `params.name`, which is
 * what the formatter keys on.
 */

const cols = [
  { name: "cat", type: "varchar" },
  { name: "revenue", type: "double" },
  { name: "orders", type: "bigint" },
  { name: "avg_price", type: "double" },
  { name: "xm", type: "double" },
];
const rows: unknown[][] = [
  ["a", 10, 111, 4.5, 1],
  ["b", 20, 222, 9.25, 2],
  ["c", 30, 333, 1.75, 3],
];

type Built = {
  series: { name: string; data: unknown[] }[];
  tooltip: { formatter: (p: unknown) => string };
};
const build = (extra: Partial<CartesianBuildConfig>) =>
  buildCartesianOption(cols, rows, {
    kind: "bar", xAxis: "cat", yMetrics: ["revenue"], ...extra,
  } as CartesianBuildConfig) as unknown as Built;

/** One axis-trigger param, the shape ECharts hands the formatter for an ordinal category axis. */
const axisParam = (cat: string, seriesName: string, value: unknown, dataIndex = 0) =>
  ({ axisValueLabel: cat, name: cat, seriesName, value, marker: "●", dataIndex });

describe("a tooltip metric is fetched and drawn only in the tooltip", () => {
  it("adds a row to the tooltip naming the metric and its formatted value", () => {
    const o = build({ tooltipMetrics: ["avg_price"] });
    const txt = o.tooltip.formatter([axisParam("a", "revenue", 10)]);
    expect(txt).toContain("avg_price");
    expect(txt).toContain("4.5");
  });

  /**
   * THE POINT OF THE WHOLE FEATURE, asserted rather than assumed: the extra metric must not become a
   * series. `toStrictEqual` and not `toContain` — a fourth series appearing is the failure this
   * pins, so the whole list is the assertion.
   */
  it("draws no series for it, and no legend entry", () => {
    const o = build({ tooltipMetrics: ["avg_price", "orders"] });
    expect(o.series.map((s) => s.name)).toStrictEqual(["revenue"]);
  });

  it("keeps the series rows above the extra rows, and marks only the series ones", () => {
    const o = build({ yMetrics: ["revenue", "orders"], tooltipMetrics: ["avg_price"] });
    const txt = o.tooltip.formatter([
      axisParam("a", "revenue", 10),
      axisParam("a", "orders", 111),
    ]);
    // Header, then the two series, then the extra — in that order.
    expect(txt.indexOf("revenue")).toBeLessThan(txt.indexOf("orders"));
    expect(txt.indexOf("orders")).toBeLessThan(txt.indexOf("avg_price"));
  });

  it("reads the value of the HOVERED category, not of the first row", () => {
    const o = build({ tooltipMetrics: ["avg_price"] });
    expect(o.tooltip.formatter([axisParam("b", "revenue", 20, 1)])).toContain("9.25");
    expect(o.tooltip.formatter([axisParam("c", "revenue", 30, 2)])).toContain("1.75");
  });

  /**
   * A VALUE/TIME X-AXIS, which is the case an index-aligned implementation gets wrong silently.
   * `applyXScale` rewrites each datum to `{value: [x, y], name}` and DROPS any whose x will not
   * parse, so the series' data array stops being index-aligned with the categories. The row for
   * `"3"` is the one that proves the lookup followed the datum rather than the position: with a
   * positional read it would report `9.25`, the second surviving point's value, and read as a
   * perfectly plausible tooltip.
   */
  it("survives applyXScale's filtering on a linear x-axis", () => {
    const numCols = [
      { name: "cat", type: "double" },
      { name: "revenue", type: "double" },
      { name: "avg_price", type: "double" },
    ];
    const numRows: unknown[][] = [["nope", 5, 99], [1, 10, 4.5], [3, 30, 1.75]];
    const o = buildCartesianOption(numCols, numRows, {
      kind: "line", xAxis: "cat", yMetrics: ["revenue"], xScale: "linear",
      tooltipMetrics: ["avg_price"],
    } as CartesianBuildConfig) as unknown as Built;
    // `axisValueLabel` DELIBERATELY DIFFERS FROM `name`, which is the whole point of the pair and
    // was the hole in the first version of this test. On a value axis ECharts formats the x
    // COORDINATE into `axisValueLabel` ("3.00" through the axis' own number format) while `name`
    // stays the datum's raw category — so a formatter keyed on `axisValueLabel` finds nothing in a
    // map built from raw categories. Writing both as "3" made the two orderings indistinguishable
    // and let a lookup on the wrong one pass: mutating the key to prefer `axisValueLabel` survived.
    const txt = o.tooltip.formatter([
      { axisValueLabel: "3.00", name: "3", seriesName: "revenue", value: [3, 30], marker: "●", dataIndex: 1 },
    ]);
    expect(txt).toContain("1.75");
    expect(txt).not.toContain("9.25");
  });

  /**
   * AN EXTRACTION UNIT SUBSTITUTES THE CATEGORIES THEMSELVES, so the axis says "Wed" where the row
   * holds `3` (`displayCats`, built through `temporalLabel`). On an ordinal axis ECharts reports the
   * `xAxis.data` entry as `params.name`, so the formatter is handed the DISPLAY string and a map
   * keyed only on raw categories answers nothing. Registering both spellings costs one Map entry per
   * category; dropping the display one leaves the rows missing on exactly the charts that group by
   * day-of-week or month-of-year, and a tooltip that simply omits them reads as a chart nobody
   * configured rather than as a bug.
   */
  it("finds the rows when an extraction unit renames the categories", () => {
    const dcols = [
      { name: "dow", type: "bigint" },
      { name: "revenue", type: "double" },
      { name: "avg_price", type: "double" },
    ];
    const drows: unknown[][] = [[1, 10, 4.5], [2, 20, 9.25], [3, 30, 1.75]];
    const o = buildCartesianOption(dcols, drows, {
      kind: "bar", xAxis: "dow", yMetrics: ["revenue"], xAxisUnit: "day_of_week",
      tooltipMetrics: ["avg_price"],
    } as CartesianBuildConfig) as unknown as Built;
    // "Wed" is what the axis shows for a raw 3, and therefore what the formatter receives.
    const txt = o.tooltip.formatter([axisParam("Wed", "revenue", 30, 2)]);
    expect(txt).toContain("1.75");
  });

  /** A name listed twice must not print twice. The picker can reach the state — a stored list is
   *  JSON and a hand-written dashboard-card override is not obliged to be a set. */
  it("prints one row for a tooltip metric named twice", () => {
    const o = build({ tooltipMetrics: ["avg_price", "avg_price"] });
    const txt = o.tooltip.formatter([axisParam("a", "revenue", 10)]);
    expect(txt.match(/avg_price/g)).toHaveLength(1);
  });

  it("formats through the metric's own column settings, prefix and decimals included", () => {
    const o = build({
      tooltipMetrics: ["avg_price"],
      metricTypes: { avg_price: "double" },
      columnSettings: { avg_price: { prefix: "$", decimals: 2, column_title: "Average price" } },
    });
    const txt = o.tooltip.formatter([axisParam("a", "revenue", 10)]);
    expect(txt).toContain("Average price");
    expect(txt).toContain("$4.50");
  });

  /** Metabase's `alreadyIncludedColumns` rule (`cartesian-chart.ts:469`), one namespace over: a
   *  metric the chart already plots is not an ADDITIONAL one, and a second row saying the same
   *  number twice is worse than no feature. */
  it("emits no row for a metric already plotted as a series", () => {
    const o = build({ yMetrics: ["revenue"], tooltipMetrics: ["revenue"] });
    const txt = o.tooltip.formatter([axisParam("a", "revenue", 10)]);
    expect(txt.match(/revenue/g)).toHaveLength(1);
  });

  /** A stored key can outlive the result it names — a dataset drifts, or a hand-written dashboard
   *  card override carries a setting the card's stored `spec` never fetched (`applyVizOverride`
   *  merges settings and states outright that "the query `spec` is never overridden"). Neither may
   *  put the word `undefined` in a tooltip. */
  it("says nothing about a tooltip metric the result does not carry", () => {
    const o = build({ tooltipMetrics: ["not_a_column"] });
    const txt = o.tooltip.formatter([axisParam("a", "revenue", 10)]);
    expect(txt).not.toContain("not_a_column");
    expect(txt).not.toContain("undefined");
  });

  /**
   * UNDER A BREAKOUT THE BUILDER DRAWS NO EXTRA ROW, and this is the deliberate scope line rather
   * than an oversight. With a `series` column the metric is aggregated per (x, band), so there is
   * no single value to put in one row; egret's cartesian tooltip is `trigger: "axis"` and shows
   * every band at once, so an honest rendering needs one row per band with a second name to
   * disambiguate — a different feature. Metabase reaches the per-band value only because its
   * tooltip rows are built from ONE hovered `seriesModel` and its `datum`
   * (`CartesianChart/events.ts:369`). `buildSpec` declines to fetch them in the same state, so
   * nothing is paid for and dropped.
   */
  it("draws no extra rows under a breakout", () => {
    const bcols = [
      { name: "cat", type: "varchar" },
      { name: "band", type: "varchar" },
      { name: "revenue", type: "double" },
      { name: "avg_price", type: "double" },
    ];
    const brows: unknown[][] = [["a", "W", 10, 4.5], ["a", "G", 6, 8], ["b", "W", 20, 9.25]];
    const o = buildCartesianOption(bcols, brows, {
      kind: "bar", xAxis: "cat", series: "band", yMetrics: ["revenue"],
      tooltipMetrics: ["avg_price"],
    } as CartesianBuildConfig) as unknown as Built;
    const txt = o.tooltip.formatter([axisParam("a", "W", 10)]);
    expect(txt).not.toContain("avg_price");
  });

  /** A row chart swaps the axes and a horizontal bar's tooltip is the same formatter — the rows
   *  must not be lost because the category moved to the y-axis. */
  it("works on a row chart, where the category axis is the y one", () => {
    const o = build({ kind: "row", tooltipMetrics: ["avg_price"] });
    expect(o.tooltip.formatter([axisParam("a", "revenue", 10)])).toContain("4.5");
  });

  /** Chart unchanged with the key absent — the whole family shares this formatter, so a regression
   *  here would be a tooltip change on every cartesian chart in the product. */
  it("changes nothing when no tooltip metric is chosen", () => {
    const withKey = build({ tooltipMetrics: [] });
    const without = build({});
    const p = [axisParam("a", "revenue", 10)];
    expect(withKey.tooltip.formatter(p)).toBe(without.tooltip.formatter(p));
    expect(without.tooltip.formatter(p)).toBe("a<br/>● revenue: <b>10</b>");
  });
});

describe("the metric-vs-metric scatter gets the same rows", () => {
  const sbuild = (extra: Partial<CartesianBuildConfig>) =>
    buildCartesianOption(cols, rows, {
      kind: "scatter", xAxis: "cat", xMetric: "xm", yMetrics: ["revenue"], ...extra,
    } as CartesianBuildConfig) as unknown as Built;

  /**
   * `scatterXY` builds its own option from nothing rather than spreading `baseOption`, and that is
   * precisely why it "kept arriving at the same defect from a new direction: every setting
   * `baseOption` learns has to be taught to it separately or it is silently dropped on one chart
   * type" (its own doc comment). Taught in the same breath, not left for the next sweep.
   *
   * THE DATUM COMES OUT OF THE BUILT OPTION and is not written by hand here. `p.data` on an item
   * tooltip IS the object the builder put in `series[].data` — ECharts hands it back untouched —
   * so a hand-made `{name, value}` stand-in is not a faithful fixture: it can only ever exercise
   * what the formatter does with a datum the builder never produced. This path carries its extra
   * values ON the datum (see the `extra` field), so reading the real one is also the only way this
   * assertion covers the half that attaches them.
   */
  it("adds the row to a scatter's item tooltip", () => {
    const o = sbuild({ tooltipMetrics: ["avg_price"] });
    const d = o.series[0].data[1] as { value: number[] };
    const txt = o.tooltip.formatter({ data: d, value: d.value });
    expect(txt).toContain("avg_price");
    expect(txt).toContain("9.25");
  });

  it("emits no row for the scatter's x metric or its bubble metric", () => {
    const o = sbuild({ bubbleMetric: "orders", tooltipMetrics: ["xm", "orders"] });
    const d = o.series[0].data[0] as { value: number[] };
    const txt = o.tooltip.formatter({ data: d, value: d.value });
    expect(txt.match(/xm/g)).toHaveLength(1);
    expect(txt.match(/orders/g)).toHaveLength(1);
  });

  it("draws no extra point series for a tooltip metric", () => {
    const o = sbuild({ tooltipMetrics: ["avg_price"] });
    expect(o.series.map((s) => s.name)).toStrictEqual(["revenue"]);
  });

  /**
   * THE ONLY PATH ON WHICH `extraTooltipMetrics`' BREAKOUT GUARD IS LOAD-BEARING, and it went
   * uncovered because the ordinary chart's test of the same rule passes for a different reason.
   * `buildCartesianOption`'s breakout branch returns `baseOption(cfg, displayCats, series, size)`
   * with no extras argument at all, so "draws no extra rows under a breakout" up in the first
   * describe would hold even if the guard were deleted — mutating it away survived. `scatterXY`
   * calls `extraTooltipMetrics` directly and is the one caller the guard actually governs.
   *
   * A scatter CAN be broken out (`gi`, and the bands become the series), and the value is then per
   * (point, band) with no single row to describe the category — the same reason the ordinary chart
   * declines, reached through the one code path that needs the guard to say so.
   */
  it("draws no extra rows on a scatter that is broken out", () => {
    const bcols = [
      { name: "cat", type: "varchar" },
      { name: "band", type: "varchar" },
      { name: "xm", type: "double" },
      { name: "revenue", type: "double" },
      { name: "avg_price", type: "double" },
    ];
    const brows: unknown[][] = [["a", "W", 1, 10, 4.5], ["b", "G", 2, 20, 9.25]];
    const o = buildCartesianOption(bcols, brows, {
      kind: "scatter", xAxis: "cat", series: "band", xMetric: "xm", yMetrics: ["revenue"],
      tooltipMetrics: ["avg_price"],
    } as CartesianBuildConfig) as unknown as Built;
    const d = o.series[0].data[0] as { value: number[] };
    expect(o.tooltip.formatter({ data: d, value: d.value })).not.toContain("avg_price");
  });
});
