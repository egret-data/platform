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
  pagination?: { enabled: boolean; pageSize: number };  // pageSize in {25, 50, 100}
  columnOrder?: string[];   // table viz only: column names in display order
}

export interface LineSeriesStyle {
  shape?: "linear" | "smooth" | "step";
  style?: "solid" | "dashed" | "dotted";
  size?: "S" | "M" | "L";
  markers?: "auto" | "on" | "off";
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
  seriesTypes?: Record<string, "bar" | "line">;  // combo only: metric -> type
  rightAxisMetrics?: string[];   // metrics plotted against a 2nd (right) Y axis
  logScale?: boolean;            // value axis uses a log scale
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
}
export interface PieConfig {
  dimension?: string;
  metric?: string;
  donut?: boolean;
  showLegend?: boolean;
  showLabels?: boolean;
  sliceThreshold?: number;   // percent; slices below this fold into "Other" (default 2.5, 0 = off)
  sortRows?: boolean;        // sort slices by value desc (default true)
  showTotal?: boolean;       // donut only: grand total in the centre
  percentVisibility?: "off" | "inside" | "both";  // percent in slice labels (default off)
  sliceColors?: Record<string, string>;  // per-slice colour override keyed by dimension value
}
export interface ScalarConfig {
  metric?: string;
}
export interface ProgressConfig {
  metric?: string;
  goal?: number;
}
export interface GaugeConfig {
  metric?: string;
  min?: number;
  max?: number;
}
export interface FunnelConfig {
  dimension?: string;
  metric?: string;
}
export interface WaterfallConfig {
  xAxis?: string;
  metric?: string;
  showTotal?: boolean;
}
export interface SmartScalarConfig {
  dimension?: string;
  metric?: string;
}
export interface BoxplotConfig {
  category?: string;
  value?: string;
}
export interface SankeyConfig {
  source?: string;
  target?: string;
  metric?: string;
}
export interface MapConfig {
  mapKey: string;      // registry key: "us_states" | "world" | "vietnam"
  region?: string;     // dimension column holding the region code/name
  metric?: string;
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
