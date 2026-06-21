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
}
export interface DashboardSummary {
  id: string; name: string; description: string;
  created_by: string | null; card_count: number; updated_at: string;
}
export interface DashboardDetail {
  id: string; name: string; description: string;
  created_by: string | null; updated_at: string;
  cards: DashboardCard[];
}
export interface CardInput {
  chart_id: string | null;
  card_type: string;
  grid_col: number;
  grid_row: number;
  size_x: number;
  size_y: number;
  options?: Record<string, unknown>;
}

export const dashboardKeys = {
  list: (ws: string) => ["dashboards", ws] as const,
  one: (ws: string, id: string) => ["dashboards", ws, id] as const,
};

export function useDashboards(ws: string) {
  return useQuery<DashboardSummary[]>({
    queryKey: dashboardKeys.list(ws),
    queryFn: () => apiJson<DashboardSummary[]>(`/ws/${ws}/dashboards`),
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
    mutationFn: ({ id, ...b }: { id: string; name: string; description?: string; cards: CardInput[] }) =>
      apiJson<DashboardDetail>(`/ws/${ws}/dashboards/${id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(b) }),
    onSuccess: (d) => invalidate(d.id),
  });
  const remove = useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/dashboards/${id}`, { method: "DELETE" }),
    onSuccess: () => invalidate(),
  });
  return { create, update, remove };
}
