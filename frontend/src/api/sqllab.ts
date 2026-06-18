import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

// ── types ──
export interface DatabaseListItem { id: string; name: string; engine: string }
export interface SavedQuery { id: string; database_id: string | null; name: string; sql: string; description: string; created_by: string | null; updated_at: string }
export interface SchemaColumn { name: string; type: string }
export type QueryStatus = "queued" | "running" | "finished" | "failed" | "cancelled";
export interface QueryView {
  id: string; database_id: string; status: QueryStatus;
  columns: { name: string; type: string }[] | null;
  preview: unknown[] | null; row_count: number | null; truncated: boolean; error: string | null;
}
export interface ResultsPage { columns: { name: string; type: string }[]; rows: unknown[][]; page: number; page_size: number; row_count: number; truncated: boolean }
export interface HistoryItem { id: string; database_id: string; status: QueryStatus; sql: string; row_count: number | null; submitted_at: string }

// ── query keys ──
export const qk = {
  databases: (ws: string) => ["sqllab", ws, "databases"] as const,
  saved: (ws: string) => ["sqllab", ws, "saved"] as const,
  history: (ws: string) => ["sqllab", ws, "history"] as const,
  status: (ws: string, id: string) => ["sqllab", ws, "status", id] as const,
  schema: (ws: string, db: string, level: string, ...rest: string[]) => ["sqllab", ws, "schema", db, level, ...rest] as const,
};

export const csvUrl = (ws: string, id: string) => `/api/ws/${ws}/queries/${id}/results.csv`;

// ── databases granted to the workspace ──
export function useDatabases(ws: string) {
  return useQuery<DatabaseListItem[]>({
    queryKey: qk.databases(ws),
    queryFn: () => apiJson<DatabaseListItem[]>(`/me/databases?workspace=${encodeURIComponent(ws)}`),
  });
}

// ── run / poll / results (engine API) ──
export function useSubmitQuery(ws: string) {
  return useMutation({
    mutationFn: (body: { database_id: string; sql: string; limit?: number }) =>
      apiJson<{ id: string; status: QueryStatus }>(`/ws/${ws}/queries`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
  });
}
export function useQueryStatus(ws: string, id: string | null) {
  return useQuery<QueryView>({
    queryKey: id ? qk.status(ws, id) : ["sqllab", ws, "status", "none"],
    queryFn: () => apiJson<QueryView>(`/ws/${ws}/queries/${id}`),
    enabled: !!id,
    refetchInterval: (q) => {
      const s = q.state.data?.status;
      return s === "queued" || s === "running" ? 1000 : false;
    },
  });
}
export function fetchResults(ws: string, id: string, page: number, pageSize: number) {
  return apiJson<ResultsPage>(`/ws/${ws}/queries/${id}/results?page=${page}&page_size=${pageSize}`);
}
export function useCancelQuery(ws: string) {
  return useMutation({ mutationFn: (id: string) => apiFetch(`/ws/${ws}/queries/${id}/cancel`, { method: "POST" }) });
}
export function useHistory(ws: string) {
  return useQuery<HistoryItem[]>({ queryKey: qk.history(ws), queryFn: () => apiJson<HistoryItem[]>(`/ws/${ws}/queries`) });
}
export function useResults(ws: string, queryId: string | null, page: number, enabled: boolean) {
  return useQuery<ResultsPage>({
    queryKey: ["sqllab", ws, "results", queryId, page],
    queryFn: () => fetchResults(ws, queryId!, page, 100),
    enabled: !!queryId && enabled,
  });
}

// ── saved queries (egret-bi) ──
export function useSavedQueries(ws: string) {
  return useQuery<SavedQuery[]>({ queryKey: qk.saved(ws), queryFn: () => apiJson<SavedQuery[]>(`/ws/${ws}/sqllab/queries`) });
}
export function useSaveQuery(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { database_id: string | null; name: string; sql: string; description?: string }) =>
      apiJson<SavedQuery>(`/ws/${ws}/sqllab/queries`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.saved(ws) }),
  });
}
export function useDeleteSavedQuery(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiFetch(`/ws/${ws}/sqllab/queries/${id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.saved(ws) }),
  });
}

// ── schema browse (egret-bi), lazy + cached ──
const SCHEMA_STALE = 5 * 60 * 1000; // 5 min (schema cache)
export function useCatalogs(ws: string, db: string | null) {
  return useQuery<string[]>({ queryKey: qk.schema(ws, db ?? "", "catalogs"), enabled: !!db, staleTime: SCHEMA_STALE,
    queryFn: () => apiJson<string[]>(`/ws/${ws}/sqllab/databases/${db}/catalogs`) });
}
export function useSchemas(ws: string, db: string | null, catalog: string) {
  return useQuery<string[]>({ queryKey: qk.schema(ws, db ?? "", "schemas", catalog), enabled: !!db, staleTime: SCHEMA_STALE,
    queryFn: () => apiJson<string[]>(`/ws/${ws}/sqllab/databases/${db}/schemas?catalog=${encodeURIComponent(catalog)}`) });
}
export function useTables(ws: string, db: string | null, catalog: string, schema: string) {
  return useQuery<string[]>({ queryKey: qk.schema(ws, db ?? "", "tables", catalog, schema), enabled: !!db && !!schema, staleTime: SCHEMA_STALE,
    queryFn: () => apiJson<string[]>(`/ws/${ws}/sqllab/databases/${db}/tables?catalog=${encodeURIComponent(catalog)}&schema=${encodeURIComponent(schema)}`) });
}
export function useColumns(ws: string, db: string | null, catalog: string, schema: string, table: string, enabled: boolean) {
  return useQuery<SchemaColumn[]>({ queryKey: qk.schema(ws, db ?? "", "columns", catalog, schema, table), enabled: !!db && enabled, staleTime: SCHEMA_STALE,
    queryFn: () => apiJson<SchemaColumn[]>(`/ws/${ws}/sqllab/databases/${db}/columns?catalog=${encodeURIComponent(catalog)}&schema=${encodeURIComponent(schema)}&table=${encodeURIComponent(table)}`) });
}
export function useRefreshSchema(ws: string) {
  const qc = useQueryClient();
  return (db: string) => qc.invalidateQueries({ queryKey: ["sqllab", ws, "schema", db] });
}
