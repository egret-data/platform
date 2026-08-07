import { describe, it, expect } from "vitest";
import { buildCartesianOption, cartesianSeriesNames, type CartesianBuildConfig } from "../options/cartesian";

/**
 * A METRIC-VS-METRIC SCATTER DREW ONE Y METRIC AND WAS BILLED FOR ALL OF THEM.
 *
 * `YAxisControl` is the shared, uncapped `FieldZone` every cartesian kind uses, and
 * `cartesianDescriptor.buildSpec` puts all of `cfg.yMetrics` into `spec.metrics` — so a scatter with
 * two Y metrics projected, aggregated and transferred both. `scatterXY` then read `cfg.yMetrics[0]`
 * and threw the rest away. MEASURED before the fix, by building the option: `yMetrics:
 * ["revenue", "orders"]` came back with `series.length === 1`, named `revenue`.
 *
 * The panel had already gone further than the builder: `cartesianSeriesNames` returns `[...yMetrics]`
 * with no breakout, so the Series list offered a colour, a rename and a Show switch for `orders` —
 * a series nothing drew. That is the panel-offers/builder-drops defect this programme keeps closing,
 * with the two halves the usual way round.
 *
 * THREE FIXES WERE POSSIBLE AND THE OTHER TWO ARE REJECTED HERE, not merely unmentioned:
 *
 *   1. CAP THE CONTROL at one Y metric on a metric-vs-metric scatter, so nothing is fetched that
 *      cannot be drawn. Rejected on parity: Metabase's scatter sets no `maxMetricsSupported`, so
 *      `getMaxMetricsSupported("scatter")` returns `Infinity` (`visualizations/index.ts:139`; only
 *      the waterfall pins it to 1), and its "Add another series" affordance is offered on a scatter
 *      whenever there is no breakout (`lib/settings/graph.ts:229-244`). Capping would also have to
 *      cap it on the DIMENSION-x scatter or vanish as the X-metric select changed, and the
 *      dimension-x flavour already draws every metric correctly through `baseOption`.
 *   2. FEED THEM TO THE TOOLTIP as additional tooltip metrics — which is, interestingly, what
 *      Metabase does with the metrics its BREAKOUT shape discards ("only the first metric is
 *      rendered, so the remaining metrics stay available as additional columns",
 *      `lib/graph/columns.ts:36-39`). Rejected because it makes one control mean two different
 *      things depending on a neighbouring select: the same chips would draw point clouds with no
 *      breakout and silently become tooltip rows with one. The tooltip metrics live in their own
 *      key, chosen deliberately, and a Y metric that cannot be drawn is a defect to fix rather than
 *      a place to put something else.
 *
 * WHAT IS DRAWN IS METABASE'S OWN SHAPE: one `type: "scatter"` series per metric, every one encoding
 * the same x key and its own y (`echarts/cartesian/scatter/option/index.ts:40-56` and `series.ts:60-69`;
 * its own e2e pins a two-metric scatter's two tooltip rows, `e2e/.../scatter.cy.spec.js:43-101`).
 *
 * AND WITH A BREAKOUT, THE FIRST METRIC ONLY — also Metabase's (`lib/graph/columns.ts:117-124`), and
 * already egret's rule everywhere else: `buildCartesianOption`'s breakout branch takes
 * `cfg.yMetrics[0]`, and `SeriesControl` prints "With a series, only the first metric is plotted."
 * under the select. This makes the scatter agree with the caption instead of applying it always.
 */

const cols = [
  { name: "cat", type: "varchar" },
  { name: "xm", type: "double" },
  { name: "revenue", type: "double" },
  { name: "orders", type: "bigint" },
  { name: "band", type: "varchar" },
];
const rows: unknown[][] = [
  ["a", 1, 10, 111, "W"],
  ["b", 2, 20, 222, "G"],
  ["c", 3, 30, 333, "W"],
];

type Series = { name: string; type: string; data: { value: number[]; name: string }[]; __raw?: string };
type Built = {
  series: Series[];
  legend?: unknown;
  yAxis: { name?: string };
  tooltip: { formatter: (p: unknown) => string };
};
const build = (extra: Partial<CartesianBuildConfig>) =>
  buildCartesianOption(cols, rows, {
    kind: "scatter", xAxis: "cat", xMetric: "xm", yMetrics: ["revenue", "orders"], ...extra,
  } as CartesianBuildConfig) as unknown as Built;

describe("a metric-vs-metric scatter draws every Y metric", () => {
  it("builds one point series per metric, sharing the x metric", () => {
    const o = build({});
    expect(o.series.map((s) => s.name)).toStrictEqual(["revenue", "orders"]);
    expect(o.series.every((s) => s.type === "scatter")).toBe(true);
  });

  /** The x coordinate is the SAME metric on both clouds — that is what "sharing the x-axis" means,
   *  and the cheap wrong implementation (one series per ROW, or the second metric on x) would pass
   *  the count assertion above and fail this one. */
  it("plots each cloud at (xMetric, its own metric)", () => {
    const o = build({});
    expect(o.series[0].data.map((d) => d.value)).toStrictEqual([[1, 10], [2, 20], [3, 30]]);
    expect(o.series[1].data.map((d) => d.value)).toStrictEqual([[1, 111], [2, 222], [3, 333]]);
  });

  it("names the two clouds so the panel's Series list can address them", () => {
    const o = build({});
    // The list the settings panel builds, and the keys the builder drew — the same two strings, or
    // a colour picked in the panel lands on nothing.
    expect(cartesianSeriesNames(cols, rows, { yMetrics: ["revenue", "orders"], xAxis: "cat" }))
      .toStrictEqual(o.series.map((s) => s.__raw ?? s.name));
  });

  it("shows a legend by default once there is more than one cloud", () => {
    expect(build({}).legend).toBeDefined();
    expect(build({ yMetrics: ["revenue"] }).legend).toBeUndefined();
  });

  /** Two metrics on one axis have no single name, exactly as `baseOption`'s `metricTitle` decides
   *  for every other kind ("An axis plotting several metrics has no one name — the legend names
   *  them instead"). Titling it `revenue` would label the axis for one of the two clouds on it. */
  it("drops the value-axis title once two metrics share the axis", () => {
    expect(build({}).yAxis.name).toBeUndefined();
    expect(build({ yMetrics: ["revenue"] }).yAxis.name).toBe("revenue");
  });

  it("names the hovered cloud's own metric in the tooltip, not the first one", () => {
    const o = build({});
    const txt = o.tooltip.formatter({ data: o.series[1].data[0], value: [1, 111] });
    expect(txt).toContain("orders");
    expect(txt).not.toContain("revenue");
  });

  /**
   * THE DATA LABEL FORMATS THROUGH EACH CLOUD'S OWN METRIC, which is a second reader of the same
   * "which metric is this?" question the tooltip answers, and it has its own answer to get wrong.
   * `pointLabel` used to be one object closed over `yMetric`; a shared closure would print the
   * first metric's currency prefix beside the second metric's count — and nothing else in this file
   * would have noticed, since every other assertion reads names, colours or coordinates.
   */
  it("labels each cloud through its own metric's format", () => {
    const o = build({
      showLabels: true,
      metricTypes: { revenue: "double", orders: "bigint" },
      columnSettings: { revenue: { prefix: "$", decimals: 2 }, orders: { suffix: " ea" } },
    }) as unknown as { series: { label: { formatter: (p: { value: number[] }) => string } }[] };
    expect(o.series[0].label.formatter({ value: [1, 10] })).toBe("$10.00");
    expect(o.series[1].label.formatter({ value: [1, 111] })).toBe("111 ea");
  });

  it("colours the two clouds differently", () => {
    const o = build({}) as unknown as { series: { itemStyle: { color: string } }[] };
    expect(o.series[0].itemStyle.color).not.toBe(o.series[1].itemStyle.color);
  });

  /** A pinned colour is keyed by the series name the panel shows, which is now the metric on every
   *  cloud — the same key `groupKey` already used for the lone-series case. */
  it("honours a pinned colour on the second metric", () => {
    const o = build({ seriesColors: { orders: "#123456" } }) as unknown as
      { series: { name: string; itemStyle: { color: string } }[] };
    expect(o.series.find((s) => s.name === "orders")?.itemStyle.color).toBe("#123456");
  });

  it("hides a cloud the panel switched off", () => {
    expect(build({ hiddenSeries: ["orders"] }).series.map((s) => s.name)).toStrictEqual(["revenue"]);
  });

  /**
   * WITH A BREAKOUT, THE FIRST METRIC ONLY — the family's rule, and the caption `SeriesControl`
   * already prints under the Series select. The bands are the series here, so a cross product would
   * put two clouds on every band and the legend would name neither of them usefully.
   */
  it("falls back to the first metric alone once a breakout names the series", () => {
    const o = build({ series: "band" });
    expect(o.series.map((s) => s.name)).toStrictEqual(["W", "G"]);
    expect(o.series[0].data.map((d) => d.value)).toStrictEqual([[1, 10], [3, 30]]);
  });

  it("leaves the single-metric scatter exactly as it was", () => {
    const o = build({ yMetrics: ["revenue"] });
    expect(o.series.map((s) => s.name)).toStrictEqual(["revenue"]);
    expect(o.series[0].data.map((d) => d.value)).toStrictEqual([[1, 10], [2, 20], [3, 30]]);
  });
});
