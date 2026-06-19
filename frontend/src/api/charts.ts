import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export type VizType = "table" | "pivot_table";
export type FilterOp =
  | "eq" | "ne" | "gt" | "ge" | "lt" | "le" | "like" | "in" | "not_in" | "is_null" | "is_not_null";

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
  rollup: boolean;
}

export interface ChartParams {
  spec: QuerySpec;
  // pivot-only layout (ignored for table viz)
  rows?: string[];
  cols?: string[];
  metric?: string;
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
    mutationFn: ({ id, ...b }: { id: string; name: string; description: string; viz_type: VizType; params: ChartParams }) =>
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
