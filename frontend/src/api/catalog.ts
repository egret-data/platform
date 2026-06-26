import { useQuery } from "@tanstack/react-query";
import { ApiError, apiJson } from "./client";

export interface CatalogSummary {
  id: string;
  name: string;
  warehouse_location: string;
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
export interface TableDetail {
  name: string;
  location: string;
  current_schema_id: number;
  columns: CatalogColumn[];
  current_snapshot_id: number | null;
  snapshot_count: number;
  properties: Record<string, string>;
}

/** Dotted render of a namespace's levels, e.g. ["sales","2024"] → "sales.2024". */
export const nsDotted = (levels: string[]): string => levels.join(".");

export const catalogKeys = {
  summary: (ws: string) => ["catalog", ws, "summary"] as const,
  namespaces: (ws: string) => ["catalog", ws, "namespaces"] as const,
  tables: (ws: string, ns: string) => ["catalog", ws, "tables", ns] as const,
  table: (ws: string, ns: string, table: string) => ["catalog", ws, "table", ns, table] as const,
};

/**
 * Catalog summary. A 404 means the workspace has no catalog provisioned yet —
 * we surface that as `null` (not an error) so the UI can show an empty state.
 */
export function useCatalogSummary(ws: string) {
  return useQuery<CatalogSummary | null>({
    queryKey: catalogKeys.summary(ws),
    queryFn: async () => {
      try {
        return await apiJson<CatalogSummary>(`/ws/${ws}/catalog`);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
    retry: false,
  });
}

export function useCatalogNamespaces(ws: string, enabled = true) {
  return useQuery<NamespaceDto[]>({
    queryKey: catalogKeys.namespaces(ws),
    queryFn: () => apiJson<NamespaceDto[]>(`/ws/${ws}/catalog/namespaces`),
    enabled,
  });
}

export function useNamespaceContents(ws: string, ns: string | null) {
  return useQuery<NamespaceContents>({
    queryKey: ns ? catalogKeys.tables(ws, ns) : ["catalog", ws, "tables", "none"],
    queryFn: () => apiJson<NamespaceContents>(`/ws/${ws}/catalog/namespaces/${encodeURIComponent(ns!)}/tables`),
    enabled: !!ns,
  });
}

export function useTableDetail(ws: string, ns: string | null, table: string | null) {
  return useQuery<TableDetail>({
    queryKey: ns && table ? catalogKeys.table(ws, ns, table) : ["catalog", ws, "table", "none"],
    queryFn: () =>
      apiJson<TableDetail>(
        `/ws/${ws}/catalog/namespaces/${encodeURIComponent(ns!)}/tables/${encodeURIComponent(table!)}`,
      ),
    enabled: !!ns && !!table,
  });
}
