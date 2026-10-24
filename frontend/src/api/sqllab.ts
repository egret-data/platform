/**
 * SQL Lab API — MIGRATED to the orval-generated client (E0·P4d-1).
 *
 * These hooks route through the generated FETCH fns (the `queries`, `sqllab`
 * and `databases` tags) but KEEP their hand-written query keys, `staleTime`,
 * `refetchInterval` and `enabled` gates: the polling status hook, the paginated
 * results hook, and the hierarchical schema cache (whose `["sqllab", ws,
 * "schema", db, …]` prefix `useRefreshSchema` invalidates in one call) can't be
 * expressed through the generated hooks' fixed per-operation keys. Using the
 * generated fetch fns underneath preserves every URL/param + behavior while
 * still sourcing the wire contract from the spec.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { csrfToken } from "./client";
import { databasesList } from "./generated/databases/databases";
import {
  getQueriesResultsCsvUrl,
  queriesCancel,
  queriesHistory,
  queriesHistoryClear,
  queriesResults,
  queriesStatus,
} from "./generated/queries/queries";
import {
  sqllabCatalogs,
  sqllabColumns,
  sqllabList,
  sqllabRemove,
  sqllabSave,
  sqllabSchemas,
  sqllabTables,
  sqllabUpdate,
} from "./generated/sqllab/sqllab";

// ── types ──
export interface DatabaseListItem {
  id: string;
  name: string;
  engine: string;
  /**
   * Set when this connection was materialized by an engine→workspace assignment
   * (egret-provisioned). The SQL-Lab picker tags such entries; behaviour is
   * unchanged — both provisioned and self-service rows are queryable.
   */
  engine_id?: string | null;
}
export interface SavedQuery { id: string; database_id: string | null; name: string; sql: string; description: string; created_by: string | null; folder_id: string | null; updated_at: string }
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

export const csvUrl = (ws: string, id: string) => getQueriesResultsCsvUrl(ws, id);

// ── databases granted to the workspace ──
export function useDatabases(ws: string) {
  return useQuery<DatabaseListItem[]>({
    queryKey: qk.databases(ws),
    queryFn: async () => (await databasesList(ws)).data as unknown as DatabaseListItem[],
  });
}

// ── run / poll / results (engine API) ──
export interface QueryParamInput { name: string; type: string; value: string }
/** The text of a refused submit: the server's `{"error": "…"}` message, else its
 *  body, else the status. Never the raw JSON (KI-086). */
function refusalMessage(body: string, status: number): string {
  try {
    const parsed: unknown = JSON.parse(body);
    if (parsed && typeof parsed === "object" && typeof (parsed as { error?: unknown }).error === "string") {
      return (parsed as { error: string }).error;
    }
  } catch { /* not JSON: use the text */ }
  return body.trim() || `The query could not be started (HTTP ${status}).`;
}
export function useSubmitQuery(ws: string) {
  return useMutation({
    // Hand-written (not the generated fetch fn) so it can carry template
    // `params`; a 400 surfaces the engine/render message verbatim to the toast.
    mutationFn: async (body: { database_id: string; sql: string; limit?: number; params?: QueryParamInput[] }) => {
      const r = await fetch(`/api/ws/${encodeURIComponent(ws)}/queries`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json", ...csrfHeaders() },
        body: JSON.stringify(body),
      });
      if (!r.ok) throw new Error(refusalMessage(await r.text().catch(() => ""), r.status));
      return (await r.json()) as { id: string; status: QueryStatus };
    },
  });
}
export function useQueryStatus(ws: string, id: string | null) {
  return useQuery<QueryView>({
    queryKey: id ? qk.status(ws, id) : ["sqllab", ws, "status", "none"],
    queryFn: async () => (await queriesStatus(ws, id!)).data as unknown as QueryView,
    enabled: !!id,
    refetchInterval: (q) => {
      const s = q.state.data?.status;
      return s === "queued" || s === "running" ? 1000 : false;
    },
  });
}
export async function fetchResults(ws: string, id: string, page: number, pageSize: number) {
  return (await queriesResults(ws, id, { page, page_size: pageSize })).data as unknown as ResultsPage;
}
export function useCancelQuery(ws: string) {
  return useMutation({ mutationFn: (id: string) => queriesCancel(ws, id) });
}
export function useHistory(ws: string) {
  return useQuery<HistoryItem[]>({ queryKey: qk.history(ws), queryFn: async () => (await queriesHistory(ws)).data as unknown as HistoryItem[] });
}
/** Clear the caller's own query history in this workspace (KI-092). */
export function useClearHistory(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => queriesHistoryClear(ws),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.history(ws) }),
  });
}
/** Max rows pulled into the browser for client-side sort/filter/expand. The
 *  results endpoint clamps `page_size` to 1000; larger result sets show the
 *  first `GRID_ROW_CAP` interactively and point to CSV for the full download. */
export const GRID_ROW_CAP = 1000;
export function useResults(ws: string, queryId: string | null, enabled: boolean) {
  return useQuery<ResultsPage>({
    queryKey: ["sqllab", ws, "results", queryId],
    queryFn: () => fetchResults(ws, queryId!, 0, GRID_ROW_CAP),
    enabled: !!queryId && enabled,
  });
}

// ── saved queries (egret-bi) ──
export function useSavedQueries(ws: string) {
  return useQuery<SavedQuery[]>({ queryKey: qk.saved(ws), queryFn: async () => (await sqllabList(ws)).data as unknown as SavedQuery[] });
}
export function useSaveQuery(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (body: { database_id: string | null; name: string; sql: string; description?: string }) =>
      (await sqllabSave(ws, body)).data as unknown as SavedQuery,
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.saved(ws) }),
  });
}
export function useUpdateSavedQuery(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async ({ id, ...body }: { id: string; database_id: string | null; name: string; sql: string; description?: string }) =>
      (await sqllabUpdate(ws, id, body)).data as unknown as SavedQuery,
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.saved(ws) }),
  });
}
export function useDeleteSavedQuery(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => sqllabRemove(ws, id),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.saved(ws) }),
  });
}

// ── schema browse (egret-bi), lazy + cached ──
const SCHEMA_STALE = 5 * 60 * 1000; // 5 min (schema cache)
export function useCatalogs(ws: string, db: string | null) {
  return useQuery<string[]>({ queryKey: qk.schema(ws, db ?? "", "catalogs"), enabled: !!db, staleTime: SCHEMA_STALE,
    queryFn: async () => (await sqllabCatalogs(ws, db!)).data as unknown as string[] });
}
export function useSchemas(ws: string, db: string | null, catalog: string) {
  return useQuery<string[]>({ queryKey: qk.schema(ws, db ?? "", "schemas", catalog), enabled: !!db, staleTime: SCHEMA_STALE,
    queryFn: async () => (await sqllabSchemas(ws, db!, { catalog })).data as unknown as string[] });
}
export function useTables(ws: string, db: string | null, catalog: string, schema: string) {
  return useQuery<string[]>({ queryKey: qk.schema(ws, db ?? "", "tables", catalog, schema), enabled: !!db && !!schema, staleTime: SCHEMA_STALE,
    queryFn: async () => (await sqllabTables(ws, db!, { catalog, schema })).data as unknown as string[] });
}
export function useColumns(ws: string, db: string | null, catalog: string, schema: string, table: string, enabled: boolean) {
  return useQuery<SchemaColumn[]>({ queryKey: qk.schema(ws, db ?? "", "columns", catalog, schema, table), enabled: !!db && enabled, staleTime: SCHEMA_STALE,
    queryFn: async () => (await sqllabColumns(ws, db!, { catalog, schema, table })).data as unknown as SchemaColumn[] });
}
// ── global object search (egret-bi) ──
export interface SearchHit { catalog: string; schema: string; table: string }
export function fqnOf(h: SearchHit): string {
  return h.catalog ? `${h.catalog}.${h.schema}.${h.table}` : `${h.schema}.${h.table}`;
}
export function useSearchObjects(ws: string, db: string | null, query: string) {
  const q = query.trim();
  return useQuery<SearchHit[]>({
    queryKey: ["sqllab", ws, "search", db ?? "", q],
    enabled: !!db && q.length >= 2,
    staleTime: 30_000,
    queryFn: async () => {
      const r = await fetch(
        `/api/ws/${encodeURIComponent(ws)}/sqllab/databases/${db}/search?q=${encodeURIComponent(q)}`,
        { credentials: "same-origin" },
      );
      if (!r.ok) throw new Error(`search failed: ${r.status}`);
      return (await r.json()) as SearchHit[];
    },
  });
}

// ── SQL snippets (egret-bi) ──
export interface SqlSnippet { id: string; name: string; body: string; description: string; updated_at: string }
export interface SnippetBody { name: string; body: string; description?: string }
function csrfHeaders(): Record<string, string> {
  const token = csrfToken();
  return token ? { "X-Egret-CSRF": token } : {};
}
async function snippetFetch(url: string, init?: RequestInit) {
  const r = await fetch(url, {
    credentials: "same-origin",
    ...init,
    headers: { "Content-Type": "application/json", ...csrfHeaders(), ...(init?.headers ?? {}) },
  });
  if (!r.ok) throw new Error(`snippet request failed: ${r.status}`);
  return r.status === 200 ? r.json() : undefined;
}
const snippetsBase = (ws: string) => `/api/ws/${encodeURIComponent(ws)}/sqllab/snippets`;
export function useSnippets(ws: string) {
  return useQuery<SqlSnippet[]>({
    queryKey: ["sqllab", ws, "snippets"],
    queryFn: () => snippetFetch(snippetsBase(ws)) as Promise<SqlSnippet[]>,
  });
}
export function useCreateSnippet(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: SnippetBody) => snippetFetch(snippetsBase(ws), { method: "POST", body: JSON.stringify(b) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sqllab", ws, "snippets"] }),
  });
}
export function useUpdateSnippet(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...b }: SnippetBody & { id: string }) =>
      snippetFetch(`${snippetsBase(ws)}/${id}`, { method: "PUT", body: JSON.stringify(b) }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sqllab", ws, "snippets"] }),
  });
}
export function useDeleteSnippet(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => snippetFetch(`${snippetsBase(ws)}/${id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["sqllab", ws, "snippets"] }),
  });
}

export function useRefreshSchema(ws: string) {
  const qc = useQueryClient();
  return (db: string) => qc.invalidateQueries({ queryKey: ["sqllab", ws, "schema", db] });
}
