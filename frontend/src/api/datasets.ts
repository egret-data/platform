import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export type DatasetKind = "physical" | "virtual";
export interface DatasetSummary {
  id: string; name: string; description: string; database_id: string;
  kind: DatasetKind; column_count: number; updated_at: string;
}
export interface DatasetColumn {
  column_name: string; data_type: string; type_generic: string; is_temporal: boolean; ordinal: number;
}
export interface DatasetDetail {
  id: string; name: string; description: string; database_id: string; kind: DatasetKind;
  catalog: string | null; schema: string | null; table: string | null; sql: string | null;
  updated_at: string; columns: DatasetColumn[];
}
export type CreateSource =
  | { kind: "physical"; catalog?: string | null; schema: string; table: string }
  | { kind: "virtual"; sql: string };
export interface CreateDatasetBody {
  name: string; database_id: string; description?: string; source: CreateSource;
}

export const dsKeys = {
  list: (ws: string) => ["datasets", ws, "list"] as const,
  detail: (ws: string, id: string) => ["datasets", ws, "detail", id] as const,
};

export function useDatasets(ws: string) {
  return useQuery<DatasetSummary[]>({
    queryKey: dsKeys.list(ws),
    queryFn: () => apiJson<DatasetSummary[]>(`/ws/${ws}/datasets`),
  });
}
export function useDataset(ws: string, id: string | null) {
  return useQuery<DatasetDetail>({
    queryKey: id ? dsKeys.detail(ws, id) : ["datasets", ws, "detail", "none"],
    queryFn: () => apiJson<DatasetDetail>(`/ws/${ws}/datasets/${id}`),
    enabled: !!id,
  });
}
export function useCreateDataset(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: CreateDatasetBody) =>
      apiJson<DatasetDetail>(`/ws/${ws}/datasets`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: dsKeys.list(ws) }),
  });
}
export function useUpdateDataset(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { name: string; description?: string; sql?: string | null }) =>
      apiJson<DatasetDetail>(`/ws/${ws}/datasets/${id}`, { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
    onSuccess: () => { qc.invalidateQueries({ queryKey: dsKeys.detail(ws, id) }); qc.invalidateQueries({ queryKey: dsKeys.list(ws) }); },
  });
}
export function useRefreshDataset(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => apiJson<DatasetDetail>(`/ws/${ws}/datasets/${id}/refresh`, { method: "POST" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: dsKeys.detail(ws, id) }),
  });
}
export function useDeleteDataset(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/datasets/${id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: dsKeys.list(ws) }),
  });
}
