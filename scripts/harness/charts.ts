/**
 * The 19 charts of the "ZZ Chart Parity" dashboard, and where each sits on the grid.
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
import type { VizType } from "@/api/charts";
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
  /** Grid geometry. The card sizes every PR 01–06 measurement was taken at. */
  col: number;
  row: number;
  sizeX: number;
  sizeY: number;
}

/** The dashboard the 19 cards live on. `width: "full"` recovered with the rest. */
export const DASHBOARD = {
  name: "ZZ Chart Parity",
  description: "19 chart types on Metabase-matched data",
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
