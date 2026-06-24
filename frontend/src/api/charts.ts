import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";
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

export interface CartesianConfig {
  xAxis?: string;            // single dimension (category / x)
  xMetric?: string;          // scatter only: a metric on the x-axis (metric-vs-metric)
  yMetrics: string[];        // one or more metrics (y)
  series?: string;           // optional 2nd dimension (breakout)
  stacked?: boolean;         // bar/area
  horizontal?: boolean;      // bar
  smooth?: boolean;          // line/area
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
  connectNulls?: boolean;        // line: bridge null gaps
  step?: boolean;                // line: step interpolation
  showStackTotal?: boolean;      // label the total atop each stacked column
  seriesColors?: Record<string, string>;  // series name -> colour override
  seriesLabels?: Record<string, string>;  // series name -> display label override
  yMin?: number;                 // manual value-axis minimum (auto when undefined)
  yMax?: number;                 // manual value-axis maximum
  showMarkers?: boolean;         // line/area: show point symbols (default true)
  xScale?: "ordinal" | "linear" | "timeseries";  // x-axis scale (default ordinal)
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

export interface Chart {
  id: string;
  dataset_id: string;
  name: string;
  description: string;
  viz_type: VizType;
  params: ChartParams;
  created_by: string | null;
  updated_at: string;
}

export const chartKeys = {
  list: (ws: string) => ["charts", ws] as const,
  one: (ws: string, id: string) => ["charts", ws, id] as const,
};

export function useCharts(ws: string) {
  return useQuery<Chart[]>({
    queryKey: chartKeys.list(ws),
    queryFn: () => apiJson<Chart[]>(`/ws/${ws}/charts`),
  });
}

export function useChart(ws: string, id: string | null) {
  return useQuery<Chart>({
    queryKey: id ? chartKeys.one(ws, id) : ["charts", ws, "none"],
    queryFn: () => apiJson<Chart>(`/ws/${ws}/charts/${id}`),
    enabled: !!id,
  });
}

export function useChartMutations(ws: string) {
  const qc = useQueryClient();
  const invalidate = () => qc.invalidateQueries({ queryKey: chartKeys.list(ws) });
  const create = useMutation({
    mutationFn: (b: { dataset_id: string; name: string; description?: string; viz_type: VizType; params: ChartParams }) =>
      apiJson<Chart>(`/ws/${ws}/charts`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
    onSuccess: invalidate,
  });
  const update = useMutation({
    mutationFn: ({ id, ...b }: { id: string; dataset_id: string; name: string; description: string; viz_type: VizType; params: ChartParams }) =>
      apiJson<Chart>(`/ws/${ws}/charts/${id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
    onSuccess: invalidate,
  });
  const remove = useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/charts/${id}`, { method: "DELETE" }),
    onSuccess: invalidate,
  });
  return { create, update, remove };
}

/** Build-only: returns the SQL + database_id; the caller submits via useSubmitQuery and polls. */
export function useBuildChartData(ws: string) {
  return useMutation({
    mutationFn: (b: { dataset_id: string; query_spec: QuerySpec }) =>
      apiJson<{ sql: string; database_id: string }>(`/ws/${ws}/charts/data`, {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b),
      }),
  });
}
