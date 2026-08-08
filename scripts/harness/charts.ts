/**
 * The 22 charts of the "ZZ Chart Parity" dashboard, and where each sits on the grid.
 *
 * NINETEEN VIZ TYPES, then THREE BREAKOUT CARDS added afterwards and appended rather than folded
 * in. The nineteen are the baseline every parity comparison since PR 01 has been measured against,
 * so they keep their names, their configs, their geometry and their order in this list; a report
 * written against the old dashboard still lines up card for card. The three at the end are the
 * blind spot PR #436 found — every one of the nineteen leaves `cartesian.series` unset, so the
 * instrument contained no broken-out chart at all. Their block comment below is where that choice
 * is argued.
 *
 * Each entry carries a DESCRIPTOR CONFIG — the shape that viz's `defaultConfig` describes — and
 * never a settings map or a spec. Those are outputs; `harnessParams` derives them by calling the
 * app's own descriptor, which is what makes the four historical harness lies inexpressible here
 * (see `params.ts`). Adding a chart type needs an entry and nothing else.
 *
 * The configs were RECOVERED from the running `demo` workspace by feeding each stored `params`
 * back through its own `descriptor.fromParams()` — the same function the editor calls to open a
 * chart — rather than being read off a screen or retyped. 15 of the 19 round-tripped byte-exact.
 * The four that did not are annotated below; three of them are a genuine defect the harness had
 * carried since it was built, and correcting them was a deliberate decision, not a tidy-up:
 *
 *   MB · Scalar / MB · Gauge / MB · Progress stored `dimensions: ["category"], row_limit: 1000`.
 *   No descriptor can produce that: all three build their spec with `oneRow(metric)` — no
 *   dimension, `row_limit: 1`. The stored spec returned four grouped rows and the renderers read
 *   `rows[0]`, so the cards displayed ONE ARBITRARY CATEGORY's revenue: five identical runs of
 *   the stored spec returned Widget, Widget, Gizmo, Gizmo, Widget, and on one page load the
 *   gauge showed Widget's 423,667.5 while the progress bar showed Gizmo's 383,061.95.
 *
 *   Metabase's own three cards were broken the same way and worse — its scalar showed the STRING
 *   `Widget` (first column of the first row is the dimension) and its gauge refused to render at
 *   all ("Gauge visualization requires a number"). There was therefore no parity to preserve, and
 *   the harness's Metabase cards were repaired to match: a single-aggregate-row query, and the
 *   same 2,000,000 bound. Both sides now read 1,510,621.69.
 *
 *   `gauge.max` and `progress.goal` moved 500,000 → 2,000,000 with the metric. The old bounds
 *   were chosen against the arbitrary per-category figure; against the real total they would peg
 *   at 300%. 2,000,000 puts the bar at ~76%.
 *
 *   MB · Table stored `settings: {}` — trap 4, unrepaired in the data. PR 06 fixed it in the app
 *   (`table.tsx`'s `fromParams` now recovers columns from the spec for v2 params too), so it
 *   renders correctly today and regenerating it is behaviourally neutral: the generated
 *   `table.columns` is exactly what that fallback computes. It stops depending on the fallback.
 */
import type { ChartOrderBy, VizType } from "@/api/charts";
import { harnessParams, type HarnessChart } from "./params";
import { datasetByName, type MetricUse } from "./datasets";

export interface HarnessChartEntry {
  name: string;
  /** A `HarnessDataset` name from `datasets.ts`. */
  dataset: string;
  viz: VizType;
  /** The descriptor's own config. Opaque here — only the descriptor looks inside. */
  config: unknown;
  /** Defaults to 1000, matching a new chart in the studio. */
  rowLimit?: number;
  /**
   * The sort the card's author asked for, if any — the studio's own `SharedQueryState.orderby`,
   * which PR #395 put behind a control. SHARED QUERY STATE AND NOT CONFIG, for the same reason
   * `HarnessChart` keeps it beside `filters` and `rowLimit`: a sort is a question about the query,
   * not about the drawing, and the descriptors are deliberately unable to express one.
   *
   * Almost every card leaves this unset, which is a new chart in the studio and is what makes the
   * cards comparable — `buildSpec` then derives the implicit ascending breakout order Metabase's
   * query processor adds. The one card that sets it (`MB · Bar Breakout Desc`) exists precisely to
   * cover the other branch; see its entry.
   */
  orderby?: ChartOrderBy[];
  /** Grid geometry. The card sizes every PR 01–06 measurement was taken at. */
  col: number;
  row: number;
  sizeX: number;
  sizeY: number;
}

/** The dashboard the 22 cards live on. `width: "full"` recovered with the rest. */
export const DASHBOARD = {
  name: "ZZ Chart Parity",
  description: "19 chart types on Metabase-matched data, three of them broken out by category",
  width: "full",
} as const;

/**
 * Row-major, matching the on-screen order. Every card is 4×5 on a 12-column grid — three per
 * row, seven rows. The uniformity is load-bearing: the 166×184 svg PRs 02–05 measured against
 * is a function of exactly this geometry, so a card resized here silently invalidates them.
 */
export const CHARTS: HarnessChartEntry[] = [
  { name: "MB · Bar", dataset: "MB By Category", viz: "bar", col: 0, row: 0, sizeX: 4, sizeY: 5,
    config: { xAxis: "category", yMetrics: ["revenue"] } },
  { name: "MB · Row", dataset: "MB By Category", viz: "row", col: 4, row: 0, sizeX: 4, sizeY: 5,
    config: { xAxis: "category", yMetrics: ["revenue"], horizontal: true } },
  { name: "MB · Line", dataset: "MB Monthly", viz: "line", col: 8, row: 0, sizeX: 4, sizeY: 5,
    config: { xAxis: "order_month", yMetrics: ["revenue"] } },

  { name: "MB · Area", dataset: "MB Monthly", viz: "area", col: 0, row: 5, sizeX: 4, sizeY: 5,
    config: { xAxis: "order_month", yMetrics: ["revenue"] } },
  { name: "MB · Combo", dataset: "MB Monthly", viz: "combo", col: 4, row: 5, sizeX: 4, sizeY: 5,
    config: {
      xAxis: "order_month", yMetrics: ["orders", "revenue"],
      seriesTypes: { revenue: "line" }, rightAxisMetrics: ["revenue"],
    } },
  { name: "MB · Scatter", dataset: "MB Scatter", viz: "scatter", col: 8, row: 5, sizeX: 4, sizeY: 5,
    config: { xAxis: "price", yMetrics: ["rating"], bubbleMetric: "orders" } },

  { name: "MB · Pie", dataset: "MB By Category", viz: "pie", col: 0, row: 10, sizeX: 4, sizeY: 5,
    config: { dimensions: ["category"], metric: "revenue" } },
  { name: "MB · Funnel", dataset: "MB Funnel", viz: "funnel", col: 4, row: 10, sizeX: 4, sizeY: 5,
    // `stepOrder` is what stops the funnel sorting itself by value; PR 05b established that the
    // Metabase card shows the pipeline order, not the ranking.
    config: {
      dimension: "step", metric: "sessions",
      stepOrder: ["Visited", "Added to cart", "Checkout", "Purchased"],
    } },
  { name: "MB · Waterfall", dataset: "MB Monthly", viz: "waterfall", col: 8, row: 10, sizeX: 4, sizeY: 5,
    config: { xAxis: "order_month", metric: "revenue" } },

  { name: "MB · Sankey", dataset: "MB Sankey", viz: "sankey", col: 0, row: 15, sizeX: 4, sizeY: 5,
    config: { source: "source", target: "category", metric: "orders" } },
  { name: "MB · Map", dataset: "MB By State", viz: "map", col: 4, row: 15, sizeX: 4, sizeY: 5,
    config: { mapKey: "us_states", region: "state", metric: "revenue" } },
  { name: "MB · Boxplot", dataset: "MB Product Prices", viz: "boxplot", col: 8, row: 15, sizeX: 4, sizeY: 5,
    // Raw columns, no metric — the box plot computes its own quartiles from every price.
    config: { category: "category", value: "price" } },

  // The three corrected in this PR. See the file header: the config is unchanged in shape, but
  // the SPEC it now produces is `oneRow` rather than a grouped query, and the bounds moved with
  // the metric they measure.
  { name: "MB · Scalar", dataset: "MB By Category", viz: "scalar", col: 0, row: 20, sizeX: 4, sizeY: 5,
    config: { metric: "revenue" } },
  { name: "MB · Trend", dataset: "MB Monthly", viz: "smartscalar", col: 4, row: 20, sizeX: 4, sizeY: 5,
    config: { dimension: "order_month", metric: "revenue" } },
  { name: "MB · Progress", dataset: "MB By Category", viz: "progress", col: 8, row: 20, sizeX: 4, sizeY: 5,
    config: { metric: "revenue", goal: 2_000_000 } },

  { name: "MB · Gauge", dataset: "MB By Category", viz: "gauge", col: 0, row: 25, sizeX: 4, sizeY: 5,
    config: { metric: "revenue", min: 0, max: 2_000_000 } },
  { name: "MB · Table", dataset: "MB Raw Orders", viz: "table", col: 4, row: 25, sizeX: 4, sizeY: 5,
    rowLimit: 60,
    config: { columns: rawOrderColumns(), metrics: [] } },
  { name: "MB · Pivot", dataset: "MB Month x Category", viz: "pivot_table", col: 8, row: 25, sizeX: 4, sizeY: 5,
    config: { rows: ["order_month"], cols: ["category"], metrics: ["revenue"] } },

  { name: "MB · Object", dataset: "MB Raw Orders", viz: "object", col: 0, row: 30, sizeX: 4, sizeY: 5,
    rowLimit: 60,
    config: { columns: rawOrderColumns() } },

  // --- THE BREAKOUT CARDS, added because the instrument had no eyes for this case -------------
  //
  // The nineteen above all leave `cartesian.series` unset. There was therefore no broken-out chart
  // anywhere in the harness, and that is why PR #436's defect — the same card drawing
  // `Doohickey / Gadget / Gizmo / Widget` on one page load and `Gizmo / Widget / Gadget / Doohickey`
  // on the next — survived thirty PRs of side-by-side comparison against Metabase. Nobody missed it;
  // there was nothing on either dashboard that could show it.
  //
  // `category` OFF `MB Month x Category` IS THE BREAKOUT COLUMN, and the choice is the whole reason
  // these cards are comparable rather than merely present:
  //
  //   - FOUR bands, so a legend fits in a 4×5 card and a wrong order is visible at a glance rather
  //     than buried in a scrolling legend. `state` (40+) and `vendor` are unusable for that reason.
  //   - The SAME four on both sides, byte for byte: egret's dataset is a `VALUES` literal
  //     materialised from Metabase's own result for this very query, and Metabase's new cards run
  //     the identical SQL that `MB · Pivot` (card 41) already runs against H2.
  //   - NOT engine-dependent. Four short ASCII strings sort and compare the same way in Trino, in
  //     H2 and in JavaScript, so a difference on screen is the renderer and never the collation.
  //   - Not `order_month`: it is already the x axis, and a chart broken out by its own category is
  //     a different question.
  //
  // WHY THREE AND NOT ONE. Band order is the legend order, the STACK order and the DRAW order, and
  // the three cards separate what a single card would confound:
  //
  //   - `MB · Bar Breakout` is the ordinary case: grouped bars, no authored sort, bands sorted by
  //     the renderer. This is the exact shape #436 was measured on.
  //   - `MB · Area Breakout` stacks, which turns band order into VERTICAL POSITION. A legend can be
  //     misread; a stack in the wrong order is a different picture of the same numbers.
  //   - `MB · Bar Breakout Desc` is the same card as the first with one thing changed — an authored
  //     `ORDER BY category DESC` — and it is what makes the first card's agreement mean anything.
  //     Metabase does NOT sort its bands: `getBreakoutDistinctValues` is
  //     `Array.from(new Set(rows.map(...)))` (`echarts/cartesian/model/series.ts:123`), pure arrival
  //     order, made deterministic by the query rather than by the renderer. So on an ascending card
  //     a renderer that ALWAYS sorted would agree with Metabase by coincidence, and the harness
  //     could not tell "egret sorts" from "egret honours the query". Only a card whose engine order
  //     is not the sorted order can, and this is that card: it is the one that exercises
  //     `preserveSeriesOrder`, which fires only when the breakout is the PRIMARY orderby key.
  //
  // LEFT OUT DELIBERATELY, both named in this PR's brief:
  //
  //   - A COMBO WITH A BREAKOUT. egret's per-series default under a breakout is positional
  //     (`seriesDisplayFor`: `index === 0 ? "line" : "bar"`) and Metabase's is not, so an unpinned
  //     combo card would compare two different defaults and teach a false lesson. Pinning the
  //     display per band on both sides would fix that and simultaneously make the card blind to
  //     band order, which was its only reason to exist. `comboBreakoutGrouping.test.ts` covers the
  //     positional reader instead.
  //   - SPLIT PANELS (#435). Its layout keys on the band's index, so it is a real reader of this
  //     order — but a panel per band inside a 4×5 card gives each of four bands roughly a fifth of
  //     ~250px, and the comparison would be about legibility rather than order. Resizing the card
  //     is not available either: the uniform 4×5 geometry is what every PR 01–06 measurement was
  //     taken at. Covered by `cartesian.scatterPanelParity.test.ts` instead.
  { name: "MB · Bar Breakout", dataset: "MB Month x Category", viz: "bar", col: 4, row: 30, sizeX: 4, sizeY: 5,
    config: { xAxis: "order_month", series: "category", yMetrics: ["revenue"] } },
  { name: "MB · Area Breakout", dataset: "MB Month x Category", viz: "area", col: 8, row: 30, sizeX: 4, sizeY: 5,
    config: { xAxis: "order_month", series: "category", yMetrics: ["revenue"], stacked: true } },
  { name: "MB · Bar Breakout Desc", dataset: "MB Month x Category", viz: "bar", col: 0, row: 35, sizeX: 4, sizeY: 5,
    // The ONLY difference from `MB · Bar Breakout`, so anything that differs on screen is
    // attributable to the sort alone. `withImplicitBreakoutOrder` appends the month behind it, so
    // the stored spec is `ORDER BY category DESC, order_month ASC` — the breakout PRIMARY, which is
    // the one condition `preserveSeriesOrder` accepts.
    orderby: [{ field: "category", desc: true }],
    config: { xAxis: "order_month", series: "category", yMetrics: ["revenue"] } },
];

/**
 * The table and object charts show every column of `MB Raw Orders`, in the dataset's own order.
 * Taken from the dataset rather than retyped so the two charts cannot drift from each other or
 * from the SQL — the ten names appeared three times in the hand-written harness.
 */
function rawOrderColumns(): string[] {
  return datasetByName("MB Raw Orders").columns.map((c) => c.column_name);
}

/** Look one chart up by name, for the tests and for a targeted single-chart repair. */
export function chartByName(name: string): HarnessChartEntry {
  const c = CHARTS.find((e) => e.name === name);
  if (!c) throw new Error(`no harness chart named "${name}"`);
  return c;
}

/**
 * The chart's saved `params`, built the way the studio builds them. The dataset's columns come
 * from `datasets.ts` because `is_temporal` decides whether the monthly-grouping default fires —
 * the trap that made a whole PR inert.
 */
export function chartParams(entry: HarnessChartEntry) {
  const ds = datasetByName(entry.dataset);
  const chart: HarnessChart = {
    viz: entry.viz,
    config: entry.config,
    columns: ds.columns,
    rowLimit: entry.rowLimit,
    orderby: entry.orderby,
  };
  return harnessParams(chart);
}

/**
 * What each chart will actually ask its dataset for, for `assertMetricsDeclared`.
 *
 * DERIVED, not declared: the metric names come out of the same `buildSpec` that will write the
 * saved spec, so the check can never be checking a stale hand-written list. That matters — the
 * point of the invariant is that a chart naming a bare column saves with a 200 and only renders
 * `unknown metric` when somebody opens the dashboard.
 */
export function metricUses(): MetricUse[] {
  return CHARTS.map((e) => ({
    chart: e.name,
    dataset: e.dataset,
    metrics: chartParams(e).spec.metrics ?? [],
  }));
}
