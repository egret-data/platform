import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  chartsChBookmarkAdd,
  chartsChBookmarkRemove,
  chartsChCreate,
  chartsChData,
  chartsChDuplicate,
  chartsChRemove,
  chartsChRestore,
  chartsChTrash,
  chartsChUpdate,
  getChartsChGetQueryKey,
  getChartsChListQueryKey,
  useChartsChGet,
  useChartsChList,
} from "./generated/charts/charts";
import type { ChartDataBody } from "./generated/model";
import type { ColumnFormat, Rule } from "@/components/chart/format-types";
import type { TrendComparison } from "@/components/chart/trend-comparison";

export type VizType =
  | "table" | "pivot_table"
  | "bar" | "row" | "line" | "area" | "scatter" | "combo" | "pie" | "scalar" | "progress" | "gauge" | "funnel" | "waterfall" | "smartscalar" | "boxplot" | "sankey" | "object" | "map";

export const CARTESIAN_VIZ = ["bar", "row", "line", "area", "scatter", "combo"] as const;
export type CartesianViz = (typeof CARTESIAN_VIZ)[number];
export const isCartesianViz = (v: VizType): v is CartesianViz =>
  (CARTESIAN_VIZ as readonly string[]).includes(v);
export type FilterOp =
  | "eq" | "ne" | "gt" | "ge" | "lt" | "le" | "like" | "not_like" | "in" | "not_in" | "is_null" | "is_not_null";

export type NullFilterOp = "is_null" | "is_not_null";
export type ValueFilterOp = Exclude<FilterOp, NullFilterOp>;

export type ChartFilter =
  | { col: string; op: NullFilterOp }
  | { col: string; op: ValueFilterOp; value: unknown };
export interface ChartOrderBy { field: string; desc: boolean }

/**
 * The most rows a chart query will ever return, mirroring `MAX_CHART_ROWS` in
 * `crates/egret-bi/src/chartquery.rs`, which applies it as `row_limit.clamp(1, MAX_CHART_ROWS)`.
 *
 * MIRRORED, NOT FETCHED, and the cost is stated rather than hidden: the number is served — the
 * chart-run response carries `max_rows` (`routes.rs`) — but only AFTER a query has run, and the
 * point of having it here is to tell the author the ceiling BEFORE they type past it. Two copies
 * of one number is the price; the Rust side is the authority and this comment names it so the
 * next reader knows which one to change first.
 */
export const MAX_CHART_ROWS = 10_000;

export interface QuerySpec {
  metrics: string[];
  dimensions: string[];
  columns: string[];
  filters: ChartFilter[];
  orderby: ChartOrderBy[];
  row_limit: number;
  rollup_groups: string[][];
  /** dimension column name -> temporal unit token (e.g. "month", "day_of_week"). */
  temporal_units?: Record<string, string>;
}

export interface TableState {
  columnWidths?: Record<string, number>;   // colName -> px (manual resizes only)
  hiddenColumns?: string[];                 // colName[] currently hidden
  pinnedLeft?: number;                      // freeze the first N columns (0 = none)
  /** freeze the first N rows of the SORTED result (0 = none). Those same N rows stay on screen on
   *  every page — see `ChartTable`'s freeze block for what Metabase does and why this matches it. */
  pinnedTop?: number;
  pagination?: { enabled: boolean; pageSize: number };  // pageSize in {25, 50, 100}
  columnOrder?: string[];   // table viz only: column names in display order
  rowIndex?: boolean;       // table viz only: prepend a "#" column numbering the visible rows
}

export interface LineSeriesStyle {
  shape?: "linear" | "smooth" | "step";
  style?: "solid" | "dashed" | "dotted";
  size?: "S" | "M" | "L";
  markers?: "auto" | "on" | "off";
  /** Metabase's per-series `line.missing`, whose chart-wide twin it marks DEPRECATED. Unset takes
   *  `CartesianConfig.missing`, then "interpolate". The argument for keeping this chart-wide — that
   *  "zero" rewrites the data and so contaminates stack totals, normalisation and trend fits — is
   *  on `decorateSeries`, along with the measurement that retired two thirds of it. */
  missing?: "interpolate" | "zero" | "none";
}
export interface CartesianConfig {
  xAxis?: string;            // single dimension (category / x)
  xMetric?: string;          // scatter only: a metric on the x-axis (metric-vs-metric)
  yMetrics: string[];        // one or more metrics (y)
  series?: string;           // optional 2nd dimension (breakout)
  stacked?: boolean;         // bar/area
  horizontal?: boolean;      // bar
  showLabels?: boolean;      // data labels
  showLegend?: boolean;      // legend visibility
  xLabel?: string;
  yLabel?: string;
  // SERIES name -> how it is drawn. Keyed by series, not by metric, so a broken-out chart can draw
  // each band differently — and so one panel row can carry a series' colour, its rename and its
  // display together. Honoured on line, area, bar and combo, which is the set Metabase's
  // `series.display` offers itself on (`perSeriesDisplayApplies`).
  seriesTypes?: Record<string, "bar" | "line" | "area">;
  /** The SERIES plotted against a 2nd (right) Y axis — metrics on a plain chart, breakout BANDS on
   *  one with a `series`. The key name predates the second case; `rightAxisSeries` in
   *  `echarts/options/cartesian.ts` is the only reader and states the trade. */
  rightAxisMetrics?: string[];
  logScale?: boolean;            // value axis uses a log scale
  bubbleMetric?: string;         // scatter only: the metric whose value sizes each bubble
  autoSplitY?: boolean;          // Metabase "Split y-axis when necessary". Unset = on.
  /**
   * Metabase's `graph.split_panels`, which its Display tab labels "Stack series": draw each series
   * in a panel of its own, stacked down the card over one shared category axis, each with a value
   * axis scaled to its own data.
   *
   * It OVERRIDES three other stored keys rather than coexisting with them — `stacked`/`normalized`,
   * `rightAxisMetrics` and `autoSplitY` — and the panel hides all three while it is on, which is
   * what Metabase does (`lib/settings/graph.ts`, `lib/settings/series.ts`). Nothing is erased from
   * storage: turn it off and the chart stacks again, the way `horizontal` already behaves on a line.
   *
   * `splitPanelsApplies` in `echarts/options/cartesian.ts` decides where it is offered at all, and
   * the builder falls back to the ordinary chart on a card too short to draw the panels legibly.
   */
  splitPanels?: boolean;
  showXLabel?: boolean;          // Metabase "Show label" (x), separate from the label text.
  showYLabel?: boolean;          // …and the same for y.
  xAxisTicks?: boolean;          // Metabase "Show lines and tick marks" (x). Unset = shown.
  /** An explicit category-label rotation, which SUPPRESSES the measured choice (see
   *  `echarts/axisLabelFit.ts`): Metabase's auto-selection likewise applies only while the setting
   *  is untouched. Metabase folds this into `graph.x_axis.axis_enabled` as one five-value key;
   *  egret keeps SEPARATE fields because the waterfall and the box plot already store this shape
   *  (`settings/ChartAxesSection.tsx`), and one control storing two different shapes across the
   *  chart module would be worse than differing from Metabase's key. The panel select is a
   *  projection over all of them — see `xTicksChoiceOf`/`xTicksPatch`, which is where the fact that
   *  Metabase's five values are MUTUALLY EXCLUSIVE is enforced. */
  xLabelRotation?: 45 | 90;
  /** Metabase's fifth `graph.x_axis.axis_enabled` value, "Compact". A FORMATTING flag on the
   *  category label text and nothing else — see `options/cartesian.ts`'s `drawLabel`. It is the
   *  same switch as `date_abbreviate` for a date and as `compact` for a number, which is exactly
   *  how Metabase implements it (`formatting/date.tsx:717`: `options.compact || options.date_abbreviate`). */
  xLabelCompact?: boolean;
  /** Metabase's `graph.y_axis.unpin_from_zero` — let the value axis fit the data instead of
   *  reaching the origin. Unset takes the display's default, which is unpinned on a scatter and
   *  pinned everywhere else; `unpinFromZeroOn` in `options/cartesian.ts` resolves it, and
   *  `unpinFromZeroApplies` beside it decides where the control is offered at all. */
  unpinFromZero?: boolean;
  yAxisTicks?: boolean;          // …and the same for y.
  yTickCount?: number;           // Metabase "Number of tick marks" on the value axis. Unset = auto.
  normalized?: boolean;          // 100% stacking
  goalValue?: number;            // horizontal goal/reference line
  goalLabel?: string;
  trendLine?: boolean;           // overlay a least-squares trend line
  showStackTotal?: boolean;      // label the total atop each stacked column
  seriesColors?: Record<string, string>;  // series name -> colour override
  seriesLabels?: Record<string, string>;  // series name -> display label override
  yMin?: number;                 // manual value-axis minimum (auto when undefined)
  yMax?: number;                 // manual value-axis maximum
  xScale?: "ordinal" | "linear" | "timeseries";  // x-axis scale (default ordinal)
  compactValues?: boolean;       // short value-axis labels (25.5K); defaults to on once numbers are wide
  maxCategories?: number;        // fold all but the largest `maxCategories - 1` into "Other" (unset = off)
  seriesOrder?: string[];        // series names in display order; unlisted ones keep natural order after them
  hiddenSeries?: string[];       // series names to leave out of the chart entirely
  /** Data-label density: "fit" drops labels that would collide, "all" draws every one. */
  labelDensity?: "fit" | "all";
  /** Line shape. One choice, because a line cannot be both smoothed and stepped. */
  lineShape?: "linear" | "smooth" | "step";
  /** What a null becomes: bridged, dropped to zero, or left as a break in the line. */
  missing?: "interpolate" | "zero" | "none";
  /** Point markers. "auto" shows them only when there are few enough points to read. */
  markers?: "auto" | "on" | "off";
  /** Per-series overrides of the chart-wide line appearance, keyed by series name. */
  seriesLine?: Record<string, LineSeriesStyle>;
  /**
   * Dataset METRICS fetched for the tooltip alone — Metabase's "Additional tooltip columns"
   * (`graph.tooltip_columns`), ported into the one namespace where it can mean something here.
   *
   * WHY NOT COLUMNS, WHICH IS WHAT METABASE OFFERS. There, the list is the result's own columns
   * minus the ones the chart already references (`getAvailableAdditionalColumns`,
   * `shared/settings/cartesian-chart.ts:458`), and choosing one does not touch the query — measured
   * by instrumenting `fetch` before opening the sidebar (zero requests) and by the `dataset_query`
   * staying byte-identical while `visualization_settings` gained the key. That is possible because
   * Metabase's notebook and its viz settings are SEPARATE: the question may return more than the
   * chart plots. egret's `buildSpec` derives the query FROM this config, so the two can never
   * diverge and the option list would be empty on every egret chart that has a tooltip.
   *
   * Adding a RAW column instead is not the same feature wearing different clothes. Put in
   * `dimensions` it joins the `GROUP BY` and makes a different chart; put in `metrics` it does not
   * resolve at all (`find_metric` matches names against the dataset's stored metrics); wrapped in
   * `any_value()` server-side it is nondeterministic — "Revenue by Category, also show Product
   * Name" would show an arbitrary product. Metabase never meets that because its extra column is
   * itself an aggregation the question's author wrote, which is exactly what a dataset METRIC is.
   *
   * HOW IT COSTS ALMOST NOTHING: `spec.metrics` takes a list of metric names and the builder draws
   * series from `yMetrics`, never from the result columns — so a name here is appended to
   * `spec.metrics`, aggregated by the engine under the same `GROUP BY`, and drawn nowhere but the
   * tooltip. See `extraTooltipMetrics` in `echarts/options/cartesian.ts` for what is excluded and
   * why, and `cartesianDescriptor.buildSpec` for the half that asks for it.
   *
   * A QUERY INPUT, therefore, and not a display setting however firmly Metabase files it under
   * Display — which is why its control lives on the Data tab and is not `cardOverridable`.
   */
  tooltipMetrics?: string[];
}
export interface PieConfig {
  /** Up to three, drawn as nested rings from the centre out. */
  dimensions?: string[];
  metric?: string;
  donut?: boolean;
  showLegend?: boolean;
  showLabels?: boolean;
  sliceThreshold?: number;   // percent; slices below this fold into "Other" (default 2.5, 0 = off)
  sortRows?: boolean;        // sort slices by value desc (default true)
  showTotal?: boolean;       // grand total in the hole; defaults ON for a multi-ring pie, off for a one-ring donut
  // Where a slice's percent appears — Metabase's own `pie.percent_visibility` domain.
  // Unset defaults to "legend" for a multi-ring pie and "off" for a single ring; see
  // resolvePercentVisibility in echarts/options/pie.ts, which both the chart and the
  // settings panel resolve through.
  percentVisibility?: "off" | "legend" | "inside" | "both";
  sliceColors?: Record<string, string>;  // per-slice colour override keyed by dimension value
  /** Rename a slice of the FIRST dimension. */
  sliceNames?: Record<string, string>;
  /** Slices of the first dimension left out entirely. */
  hiddenSlices?: string[];
  /** Display order for the first dimension's slices. */
  sliceOrder?: string[];
  /** Decimal places on percentages. */
  decimals?: number;
}
/**
 * A conditional-colour band for a scalar. Deliberately NOT `GaugeSegment`: a gauge's arc needs
 * both ends to have a length, so its bounds are required numbers, while a scalar band is a plain
 * interval test and "≥ 1701 is High" is the natural way to write its top band. Matches Metabase's
 * `ScalarSegment`.
 *
 * `min`/`max` are NULLABLE, not optional. A bound edited down to an empty input has to round-trip
 * through JSONB as an explicit "no bound"; an optional key that simply disappears is
 * indistinguishable from one that was never set.
 */
export interface ScalarSegment {
  /** Lower bound, inclusive. `null` is −∞. */
  min: number | null;
  /** Upper bound, inclusive. `null` is +∞. */
  max: number | null;
  color: string;
  label?: string;
}
export interface ScalarConfig {
  metric?: string;
  /** Conditional colour bands. Order is meaning — the FIRST match wins, so nothing sorts these. */
  segments?: ScalarSegment[];
  /** Optional caption under the value. Off by default: dashboard cards already render a header,
   * so the old always-on metric name printed it twice. */
  label?: string;
  /** Optional second caption line, below `label`. */
  sublabel?: string;
}
export interface ProgressConfig {
  metric?: string;
  goal?: number;
  /** Metabase's goal takes a number OR a column; this names the column when one is chosen. */
  goalColumn?: string;
  /** Metabase's `progress.color` — the bar colour, from which the track shades are derived. */
  color?: string;
}
export interface GaugeSegment {
  min: number;
  max: number;
  color: string;
  label?: string;
}
export interface GaugeConfig {
  metric?: string;
  /** Coloured ranges, in any order. The gauge's scale is derived from them. */
  segments?: GaugeSegment[];
}
export interface FunnelConfig {
  dimension?: string;
  metric?: string;
  /** Metabase's `funnel.rows`: the steps in display order… */
  stepOrder?: string[];
  /** …and the ones left out. */
  hiddenSteps?: string[];
  /** Metabase's `funnel.type`: one narrowing band, or a column per step. */
  type?: "funnel" | "bar";
}
export interface WaterfallConfig {
  xAxis?: string;
  metric?: string;
  /** Unset means SHOWN, as in Metabase, whose `waterfall.show_total` defaults to true. */
  showTotal?: boolean;
  /** Metabase's `waterfall.increase_color` / `decrease_color` / `total_color`. */
  increaseColor?: string;
  decreaseColor?: string;
  totalColor?: string;
  // Everything below is named after the identical `CartesianConfig` field, because the waterfall
  // reads it for the same purpose and the settings panel renders the same control for it.
  showLabels?: boolean;
  labelDensity?: "fit" | "all";
  compactValues?: boolean;
  goalValue?: number;
  goalLabel?: string;
  xLabel?: string;
  yLabel?: string;
  showXLabel?: boolean;
  showYLabel?: boolean;
  xAxisTicks?: boolean;
  yAxisTicks?: boolean;
  /** Metabase folds label rotation into its "Show lines and tick marks" select. Unset lets the
   *  chart tilt long names on its own. */
  xLabelRotation?: 45 | 90;
  /** The same select's "Compact". THE WATERFALL GETS THIS AND THE BOX PLOT DOES NOT, which
   *  overturns a decision recorded twice in this repo — see `settings/ChartAxesSection.tsx`, where
   *  the premise ("a waterfall's x is a category") and the measurement that falsifies it are set
   *  out together. `options/waterfall.ts`'s `drawLabel` names periods exactly as cartesian's does. */
  xLabelCompact?: boolean;
  yTickCount?: number;
  yMin?: number;
  yMax?: number;
  logScale?: boolean;
}
export interface SmartScalarConfig {
  dimension?: string;
  metric?: string;
  /** Metabase "Switch positive / negative colors?" — for a metric where LESS is better. */
  switchPositiveNegative?: boolean;
  /** Metabase "Compact number": 36,513 -> 36.5k. */
  compact?: boolean;
  /**
   * Metabase's `scalar.comparisons` — what the number is measured against, in authored order,
   * capped at `MAX_COMPARISONS`. Undefined means "use the default", which is a period comparison on
   * a bucketed trend and a previous-value one otherwise; see `chart/trend-comparison.ts`.
   *
   * NO CODEC ENTRY IS NEEDED, unlike the cartesian family: `fieldDescriptor` copies every stored
   * key under the viz's prefix generically, so `smartscalar.comparisons` persists with no
   * hand-maintained list to update. (`viz/cartesian-codec.ts` exists because the cartesian
   * descriptor is hand-written and does keep such a list.)
   */
  comparisons?: TrendComparison[];
}
export interface BoxplotConfig {
  category?: string;
  value?: string;
  /** The one colour the chart derives from — box fill, border, mean and points all come from it. */
  color?: string;
  /** Metabase's `boxplot.whisker_type`. Unset is Tukey (1.5 × IQR). */
  whiskers?: "tukey" | "min-max";
  /** Metabase's `boxplot.points_mode`. Unset is "outliers", and Min/Max whiskers force "none". */
  points?: "none" | "outliers" | "all";
  /** Unset means SHOWN — `boxplot.show_mean` defaults to true. */
  showMean?: boolean;
  showLabels?: boolean;
  /** Metabase's `boxplot.show_values_mode`. */
  labelMode?: "median" | "all";
  compactValues?: boolean;
  goalValue?: number;
  goalLabel?: string;
  xLabel?: string;
  yLabel?: string;
  showXLabel?: boolean;
  showYLabel?: boolean;
  xAxisTicks?: boolean;
  yAxisTicks?: boolean;
  xLabelRotation?: 45 | 90;
  /** Metabase's `graph.y_axis.unpin_from_zero`. A box plot's display is none of the three
   *  `isYAxisUnpinFromZeroValid` excludes, so it is offered here exactly as it is on a line — and
   *  it matters MORE here than anywhere, since a distribution of prices between 40 and 60 spends
   *  most of a pinned axis drawing empty space below the lowest whisker. No `xLabelCompact` twin:
   *  this chart's category axis has no formatter to compact through (`options/boxplot.ts`). */
  unpinFromZero?: boolean;
  yTickCount?: number;
  yMin?: number;
  yMax?: number;
  logScale?: boolean;
}
export interface SankeyConfig {
  source?: string;
  target?: string;
  metric?: string;
  /** Metabase's `sankey.node_align`. Unset is "left". */
  nodeAlign?: "left" | "right" | "justify";
  /** Metabase's `sankey.edge_color`. Unset is "source". */
  edgeColor?: "gray" | "source" | "target";
  showEdgeLabels?: boolean;
  /** Metabase's `sankey.label_value_formatting`, only meaningful with edge labels on. */
  labelFormatting?: "auto" | "compact" | "full";
}
/** Metabase's `map.type`. "grid" is deliberately absent — see `MapConfig.type`. */
export type MapType = "region" | "pin";

export interface MapConfig {
  /** Metabase's `map.colors` — the accent the choropleth ramp derives from, and the pin colour. */
  color?: string;
  mapKey: string;      // registry key: "us_states" | "world" | "vietnam"
  /**
   * WHAT THE ROWS BECOME: shaded regions, or one marker per row at a coordinate.
   *
   * The two branches ask the backend for genuinely different things — a region map aggregates a
   * metric by a dimension, a pin map asks for raw records — so this is the field `buildSpec`
   * switches on, and it is why the panel hides the other branch's fields rather than greying them.
   *
   * OPTIONAL, AND UNDEFINED MEANS "region". Every map chart saved before pin mode existed has no
   * `map.type` at all, and all of them are region maps; reading a missing value as anything else
   * would silently re-point them at a query they cannot run. `fromParams` resolves it, so config
   * in hand always has it — only STORED params can lack it.
   *
   * Metabase's third option, "grid", is NOT here and is not an oversight. A grid map bins the
   * coordinates in SQL, which egret's `QuerySpec` has no way to express (it would need binning,
   * dual-dialect SQL for Trino and StarRocks, and a bin-width channel to carry the chosen width
   * back to the renderer). It was judged not worth building at all: the thing a grid map is for —
   * density over an area — is what egret's choropleth already does better whenever the data has a
   * region column, without binning anything.
   */
  type?: MapType;
  region?: string;     // dimension column holding the region code/name — region maps only
  metric?: string;     // region maps only
  /** Metabase's `map.latitude_column` — pin maps only. Numeric. */
  latitude?: string;
  /** Metabase's `map.longitude_column` — pin maps only. Numeric. */
  longitude?: string;
  /**
   * The column a pin is NAMED by in its tooltip. Pin maps only, and optional.
   *
   * NO METABASE COUNTERPART, because the difference is in the query rather than the display:
   * Metabase's pin map gets the whole row back and puts every column in the marker's popup, while
   * egret's raw-records mode selects exactly the columns `buildSpec` names. With only `latitude`
   * and `longitude` selected there is literally nothing else to show, so a pin could only ever
   * identify itself as its own coordinates. Naming one column adds it to the SELECT and to the
   * tooltip. A full column picker is the fuller answer, and is `tooltipColumns` below.
   */
  label?: string;
  /**
   * ADDITIONAL TOOLTIP COLUMNS — Metabase's `graph.tooltip_columns`, ported as itself rather than
   * by analogy, and this is the one chart family in egret where that is possible.
   *
   * #433 wanted this feature for the cartesian charts and could not have it: those specs GROUP BY
   * the dimension, so a raw column has no single value per drawn point, and reaching for
   * `any_value()` would print one of N rows' cells at random under a heading that claims to
   * describe the point. It substituted dataset METRICS — aggregates the dataset's author wrote —
   * because an aggregate is the thing a grouped query can honestly return.
   *
   * That objection is entirely about aggregation, and a PIN MAP HAS NONE. `buildSpec` takes
   * `chartquery.rs`'s raw-records branch (`metrics: []`, `columns` non-empty), which emits
   * `SELECT <col> AS <col> FROM … WHERE … ORDER BY … LIMIT` with no GROUP BY at all — one result
   * row per marker. A column's value on a pin is that pin's own row's cell, and there is nothing
   * for it to be ambiguous between. So Metabase's original feature is well-posed here and only
   * here.
   *
   * NOT FREE, but cheap and safe: these names are appended to `spec.columns`, so the query does
   * change. Adding a projection to a query with no GROUP BY cannot change which rows come back or
   * what any other column holds, which is the property that makes appending them uncontroversial
   * — the same change on a grouped query could alter the grouping itself.
   */
  tooltipColumns?: string[];
}

export interface ChartParams {
  spec: QuerySpec;
  // pivot-only layout (ignored for table viz)
  rows?: string[];
  cols?: string[];
  metrics?: string[];
  showRowTotals?: boolean;
  showColTotals?: boolean;
  condense?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  conditionalFormatting?: Rule[];
  tableState?: TableState;
  collapsedRows?: string[];                     // pivot only: serialized collapsed row-group paths
  pivotSort?: Record<string, "asc" | "desc">;   // pivot only: row-dimension name -> direction
  cartesian?: CartesianConfig;   // cartesian-family viz only
  pie?: PieConfig;               // pie viz only
  scalar?: ScalarConfig;         // big-number viz only
  progress?: ProgressConfig;     // progress viz only
  gauge?: GaugeConfig;           // gauge viz only
  funnel?: FunnelConfig;         // funnel viz only
  waterfall?: WaterfallConfig;   // waterfall viz only
  smartscalar?: SmartScalarConfig; // trend viz only
  boxplot?: BoxplotConfig;       // boxplot viz only
  sankey?: SankeyConfig;         // sankey viz only
  map?: MapConfig;               // map viz only
}

/** Flat, namespaced viz settings — the v2 storage shape (see the A2 spec). */
export type VizSettings = Record<string, unknown>;

/** Params as stored from A2 onwards: the query, plus flat namespaced settings. */
export interface ChartParamsV2 {
  version: 2;
  spec: QuerySpec;
  settings: VizSettings;
}

/** What a stored chart may hold: pre-A2 rows are v1, new writes are v2. */
export type AnyChartParams = ChartParams | ChartParamsV2;

export const isParamsV2 = (p: AnyChartParams | undefined | null): p is ChartParamsV2 =>
  !!p && (p as ChartParamsV2).version === 2;

/** A spec that selects nothing — used as the starting point for a new chart. */
export const EMPTY_SPEC: QuerySpec = {
  metrics: [], dimensions: [], columns: [], filters: [], orderby: [],
  row_limit: 1000, rollup_groups: [],
};

export interface Chart {
  id: string;
  dataset_id: string;
  name: string;
  description: string;
  viz_type: VizType;
  params: AnyChartParams;
  created_by: string | null;
  updated_at: string;
  folder_id: string | null;
  bookmarked?: boolean;
  deleted_at?: string | null;
}

/** Back-compat query-key helpers; delegate to the generated key factories. */
export const chartKeys = {
  list: (ws: string) => getChartsChListQueryKey(ws),
  one: (ws: string, id: string) => getChartsChGetQueryKey(ws, id),
};

export function useCharts(ws: string, opts?: { trashed?: boolean }) {
  const trashed = !!opts?.trashed;
  return useChartsChList<Chart[], ApiError>(ws, trashed ? { trashed: true } : undefined, {
    query: { select: (r) => r.data as Chart[] },
  });
}

export function useChart(ws: string, id: string | null) {
  return useChartsChGet<Chart, ApiError>(ws, id ?? "", {
    query: { enabled: !!id, select: (r) => r.data as Chart },
  });
}

export function useChartMutations(ws: string) {
  const qc = useQueryClient();
  const invalidate = () => qc.invalidateQueries({ queryKey: getChartsChListQueryKey(ws) });
  const create = useMutation<Chart, ApiError, { dataset_id: string; name: string; description?: string; viz_type: VizType; params: ChartParams }>({
    mutationFn: async (b) => (await chartsChCreate(ws, b)).data as Chart,
    onSuccess: invalidate,
  });
  const update = useMutation<Chart, ApiError, { id: string; dataset_id: string; name: string; description: string; viz_type: VizType; params: ChartParams }>({
    mutationFn: async ({ id, ...b }) => (await chartsChUpdate(ws, id, b)).data as Chart,
    onSuccess: invalidate,
  });
  const remove = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await chartsChRemove(ws, id); },
    onSuccess: invalidate,
  });
  const duplicate = useMutation<Chart, ApiError, string>({
    mutationFn: async (id) => (await chartsChDuplicate(ws, id)).data as Chart,
    onSuccess: invalidate,
  });
  const trash = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await chartsChTrash(ws, id); },
    onSuccess: invalidate,
  });
  const restore = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await chartsChRestore(ws, id); },
    onSuccess: invalidate,
  });
  const addBookmark = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await chartsChBookmarkAdd(ws, id); },
    onSuccess: invalidate,
  });
  const removeBookmark = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await chartsChBookmarkRemove(ws, id); },
    onSuccess: invalidate,
  });
  return { create, update, remove, duplicate, trash, restore, addBookmark, removeBookmark };
}

/**
 * The SQL a `QuerySpec` compiles to, for on-demand display — e.g. the chart
 * builder's "View SQL" disclosure. Build-only (`ch_data`): it never submits
 * or runs the query, so it stays available even when a workspace has SQL Lab
 * switched off. Gated by the `charts` entitlement, not `sql_lab` — a chart
 * author can always see what SQL their own chart compiles to; what's gated
 * is the free-form SQL *tool*, not the ability to read compiled SQL.
 *
 * Lazy via `enabled`: scope it to whichever UI affordance reveals the SQL
 * (e.g. the disclosure being open), not to the chart's run lifecycle, so a
 * workspace that never opens it never fetches.
 */
export function useBuildChartData(ws: string, datasetId: string | null, spec: QuerySpec | null, enabled: boolean) {
  const key = datasetId && spec ? JSON.stringify(spec) : "";
  return useQuery<{ sql: string; database_id: string }, ApiError>({
    queryKey: ["chartsChData", ws, datasetId, key],
    enabled: enabled && !!datasetId && !!spec,
    // The FE `QuerySpec` (discriminated `ChartFilter[]`) is stricter than the spec's
    // loosely-typed `query_spec`; cast the body to the generated shape.
    queryFn: async () =>
      (await chartsChData(ws, { dataset_id: datasetId!, query_spec: spec! } as unknown as ChartDataBody)).data as { sql: string; database_id: string },
  });
}
