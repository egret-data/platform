import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, apiFetch, apiJson } from "./client";

export interface Warehouse {
  id: string;
  name: string;
  storage_conn_id: string | null;
  storage_conn_name: string | null;
  created_at: string;
}
export interface NamespaceDto {
  levels: string[];
}
export interface NamespaceContents {
  tables: string[];
  views: string[];
}
export interface CatalogColumn {
  name: string;
  type: string;
  required: boolean;
}
export interface PartitionField {
  name: string;
  source_column: string;
  transform: string;
}
export interface SortField {
  column: string;
  transform: string;
  direction: string;
  null_order: string;
}
export interface Snapshot {
  snapshot_id: number;
  parent_snapshot_id: number | null;
  timestamp_ms: number;
  operation: string;
  summary: Record<string, string>;
}
export interface TableDetail {
  name: string;
  location: string;
  current_schema_id: number;
  columns: CatalogColumn[];
  current_snapshot_id: number | null;
  snapshot_count: number;
  properties: Record<string, string>;
  format_version: number;
  last_updated_ms: number;
  partition_fields: PartitionField[];
  sort_fields: SortField[];
  snapshots: Snapshot[];
}

/** Dotted render of a namespace's levels, e.g. ["sales","2024"] → "sales.2024". */
export const nsDotted = (levels: string[]): string => levels.join(".");

export const catalogKeys = {
  warehouses: (ws: string) => ["catalog", ws, "warehouses"] as const,
  warehouse: (ws: string, wid: string) => ["catalog", ws, "warehouse", wid] as const,
  namespaces: (ws: string, wid: string) => ["catalog", ws, wid, "namespaces"] as const,
  tables: (ws: string, wid: string, ns: string) => ["catalog", ws, wid, "tables", ns] as const,
  table: (ws: string, wid: string, ns: string, table: string) =>
    ["catalog", ws, wid, "table", ns, table] as const,
  tokens: (ws: string, wid: string) => ["catalog", ws, wid, "tokens"] as const,
};

// ── Workspace admin types ──────────────────────────────────────────────────

export interface CatalogTokenDto {
  id: string;
  label: string;
  permissions: string[];
  namespace_scope: string[];
  created_at: string;
}

export interface MintTokenRequest {
  label: string;
  permissions: string[];
  namespace_scope: string[];
}

export interface MintTokenResponse {
  token: CatalogTokenDto;
  raw: string;
}

// ── Warehouse CRUD ─────────────────────────────────────────────────────────

export interface CreateWarehouseRequest {
  name: string;
  storage_conn_id: string;
}

export function useWarehouses(ws: string) {
  return useQuery<Warehouse[], ApiError>({
    queryKey: catalogKeys.warehouses(ws),
    queryFn: () => apiJson<Warehouse[]>(`/ws/${ws}/catalog/warehouses`),
    // 403 (non-admin) is an expected outcome — surface it immediately instead
    // of retrying 3× before the workspace-admin notice can render.
    retry: false,
  });
}

export function useWarehouse(ws: string, wid: string) {
  return useQuery<Warehouse, ApiError>({
    queryKey: catalogKeys.warehouse(ws, wid),
    queryFn: () => apiJson<Warehouse>(`/ws/${ws}/catalog/warehouses/${wid}`),
    retry: false,
  });
}

export function useCreateWarehouse(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<Warehouse, ApiError, CreateWarehouseRequest>({
    mutationFn: (req) =>
      apiJson<Warehouse>(`/ws/${ws}/catalog/warehouses`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(req),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: catalogKeys.warehouses(ws) });
    },
  });
}

export interface RenameWarehouseArgs {
  wid: string;
  name: string;
}

export function useRenameWarehouse(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<Warehouse, ApiError, RenameWarehouseArgs>({
    mutationFn: ({ wid, name }) =>
      apiJson<Warehouse>(`/ws/${ws}/catalog/warehouses/${wid}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ name }),
      }),
    onSuccess: (_data, { wid }) => {
      queryClient.invalidateQueries({ queryKey: catalogKeys.warehouses(ws) });
      queryClient.invalidateQueries({ queryKey: catalogKeys.warehouse(ws, wid) });
    },
  });
}

export function useDeleteWarehouse(ws: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (wid) => {
      await apiFetch(`/ws/${ws}/catalog/warehouses/${wid}`, { method: "DELETE" });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: catalogKeys.warehouses(ws) });
    },
  });
}

// ── Per-warehouse browse ───────────────────────────────────────────────────

export function useCatalogNamespaces(ws: string, wid: string, enabled = true) {
  return useQuery<NamespaceDto[]>({
    queryKey: catalogKeys.namespaces(ws, wid),
    queryFn: () => apiJson<NamespaceDto[]>(`/ws/${ws}/catalog/warehouses/${wid}/namespaces`),
    enabled,
  });
}

export function useCreateNamespace(ws: string, wid: string) {
  const queryClient = useQueryClient();
  return useMutation<unknown, ApiError, { parent: string | null; name: string }>({
    mutationFn: (body) =>
      apiJson(`/ws/${ws}/catalog/warehouses/${wid}/namespaces`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: catalogKeys.namespaces(ws, wid) });
    },
  });
}

export function useDeleteNamespace(ws: string, wid: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (ns) => {
      await apiFetch(`/ws/${ws}/catalog/warehouses/${wid}/namespaces/${encodeURIComponent(ns)}`, {
        method: "DELETE",
      });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: catalogKeys.namespaces(ws, wid) });
    },
  });
}

export function useNamespaceContents(ws: string, wid: string, ns: string | null) {
  return useQuery<NamespaceContents>({
    queryKey: ns ? catalogKeys.tables(ws, wid, ns) : ["catalog", ws, wid, "tables", "none"],
    queryFn: () =>
      apiJson<NamespaceContents>(
        `/ws/${ws}/catalog/warehouses/${wid}/namespaces/${encodeURIComponent(ns!)}/tables`,
      ),
    enabled: !!ns,
  });
}

export function useTableDetail(ws: string, wid: string, ns: string | null, table: string | null) {
  return useQuery<TableDetail>({
    queryKey: ns && table ? catalogKeys.table(ws, wid, ns, table) : ["catalog", ws, wid, "table", "none"],
    queryFn: () =>
      apiJson<TableDetail>(
        `/ws/${ws}/catalog/warehouses/${wid}/namespaces/${encodeURIComponent(ns!)}/tables/${encodeURIComponent(table!)}`,
      ),
    enabled: !!ns && !!table,
  });
}

// ── Per-warehouse access tokens ────────────────────────────────────────────

export function useCatalogTokens(ws: string, wid: string) {
  return useQuery<CatalogTokenDto[], ApiError>({
    queryKey: catalogKeys.tokens(ws, wid),
    queryFn: () => apiJson<CatalogTokenDto[]>(`/ws/${ws}/catalog/warehouses/${wid}/tokens`),
    // 403 (non-admin) is an expected outcome — surface it immediately instead
    // of retrying 3× before the workspace-admin notice can render.
    retry: false,
  });
}

export function useMintToken(ws: string, wid: string) {
  const queryClient = useQueryClient();
  return useMutation<MintTokenResponse, ApiError, MintTokenRequest>({
    mutationFn: (req) =>
      apiJson<MintTokenResponse>(`/ws/${ws}/catalog/warehouses/${wid}/tokens`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(req),
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: catalogKeys.tokens(ws, wid) });
    },
  });
}

export function useRevokeToken(ws: string, wid: string) {
  const queryClient = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await apiFetch(`/ws/${ws}/catalog/warehouses/${wid}/tokens/${id}`, { method: "DELETE" });
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: catalogKeys.tokens(ws, wid) });
    },
  });
}
