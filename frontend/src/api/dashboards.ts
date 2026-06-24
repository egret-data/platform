import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export interface DashboardCard {
  id: string;
  chart_id: string | null;
  card_type: string;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options: Record<string, unknown>;
  tab_id: string | null;
}
export interface DashboardTab {
  id: string;
  name: string;
  position: number;
}
export interface DashboardSummary {
  id: string; name: string; description: string;
  created_by: string | null; card_count: number; updated_at: string;
  deleted_at?: string | null;
  bookmarked?: boolean;
}
export type ParamType = "text" | "number" | "date" | "id" | "category" | "boolean" | "location" | "temporal_unit";
export type ParamOperator =
  | "is" | "is-not" | "contains" | "does-not-contain" | "starts-with" | "ends-with"        // text / category / location
  | "=" | "!=" | "between" | ">=" | "<="                              // number
  | "range" | "relative" | "single" | "month-year" | "quarter-year" | "all-options";  // date
export type ParamWidget = "dropdown" | "search" | "input";
export interface ParamValuesSource {
  type: "distinct" | "static" | "card";
  static?: string[];
  cardId?: string;
  column?: string;
}
export interface DashboardParameter {
  id: string;
  name: string;
  slug: string;
  type: ParamType;
  operator: ParamOperator;
  default?: unknown;
  required?: boolean;
  widget?: ParamWidget;            // how the value is picked in view mode
  filteringParameters?: string[];  // ids of params whose value constrains THIS param's option list
  valuesSource?: ParamValuesSource;
}

export interface DashboardDetail {
  id: string; name: string; description: string;
  created_by: string | null; updated_at: string;
  parameters: DashboardParameter[];
  tabs: DashboardTab[];
  width: "fixed" | "full";
  cards: DashboardCard[];
  deleted_at?: string | null;
  bookmarked?: boolean;
}
export interface CardInput {
  chart_id: string | null;
  card_type: string;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options?: Record<string, unknown>;
  tab_index: number | null;
}

export const dashboardKeys = {
  list: (ws: string) => ["dashboards", ws] as const,
  one: (ws: string, id: string) => ["dashboards", ws, id] as const,
};

export function useDashboards(ws: string, opts?: { trashed?: boolean }) {
  const trashed = !!opts?.trashed;
  return useQuery<DashboardSummary[]>({
    queryKey: [...dashboardKeys.list(ws), { trashed }],
    queryFn: () => apiJson<DashboardSummary[]>(`/ws/${ws}/dashboards${trashed ? "?trashed=true" : ""}`),
  });
}

export function useDashboard(ws: string, id: string | null) {
  return useQuery<DashboardDetail>({
    queryKey: id ? dashboardKeys.one(ws, id) : ["dashboards", ws, "none"],
    queryFn: () => apiJson<DashboardDetail>(`/ws/${ws}/dashboards/${id}`),
    enabled: !!id,
  });
}

export function useDashboardMutations(ws: string) {
  const qc = useQueryClient();
  const invalidate = (id?: string) => {
    qc.invalidateQueries({ queryKey: dashboardKeys.list(ws) });
    if (id) qc.invalidateQueries({ queryKey: dashboardKeys.one(ws, id) });
  };
  const create = useMutation({
    mutationFn: (b: { name: string; description?: string }) =>
      apiJson<DashboardDetail>(`/ws/${ws}/dashboards`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
    onSuccess: () => invalidate(),
  });
  const update = useMutation({
    mutationFn: ({ id, ...b }: { id: string; name: string; description?: string; parameters: DashboardParameter[]; tabs: { name: string; position: number }[]; width: "fixed" | "full"; cards: CardInput[] }) =>
      apiJson<DashboardDetail>(`/ws/${ws}/dashboards/${id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
    onSuccess: (d) => invalidate(d.id),
  });
  const remove = useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/dashboards/${id}`, { method: "DELETE" }),
    onSuccess: () => invalidate(),
  });
  const duplicate = useMutation({
    mutationFn: (id: string) =>
      apiJson<DashboardDetail>(`/ws/${ws}/dashboards/${id}/duplicate`, { method: "POST" }),
    onSuccess: () => invalidate(),
  });
  const trash = useMutation({
    mutationFn: (id: string) => apiJson(`/ws/${ws}/dashboards/${id}/trash`, { method: "POST" }),
    onSuccess: () => invalidate(),
  });
  const restore = useMutation({
    mutationFn: (id: string) => apiJson(`/ws/${ws}/dashboards/${id}/restore`, { method: "POST" }),
    onSuccess: () => invalidate(),
  });
  const addBookmark = useMutation({
    mutationFn: (id: string) => apiJson(`/ws/${ws}/dashboards/${id}/bookmark`, { method: "POST" }),
    onSuccess: () => invalidate(),
  });
  const removeBookmark = useMutation({
    mutationFn: (id: string) => apiJson(`/ws/${ws}/dashboards/${id}/bookmark`, { method: "DELETE" }),
    onSuccess: () => invalidate(),
  });
  return { create, update, remove, duplicate, trash, restore, addBookmark, removeBookmark };
}
