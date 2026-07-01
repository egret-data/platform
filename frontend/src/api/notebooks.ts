import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiFetch, apiJson } from "./client";

export type CellType = "code" | "markdown" | "sql";

export interface Cell {
  id: string;
  cell_type: CellType;
  source: string;
  cell_order: number;
  execution_count: number | null;
  last_output: unknown | null;
  last_execution_time_ms: number | null;
  sql_target: string | null;
}

export interface NotebookSummary {
  id: string;
  name: string;
  description: string;
  is_public: boolean;
  owner_user_id: string;
  is_owner: boolean;
  updated_at: string;
  bookmarked?: boolean;
  folder_id?: string | null;
  deleted_at?: string | null;
}

export interface NotebookDetail extends NotebookSummary {
  cells: Cell[];
}

export const qk = {
  list: (ws: string) => ["notebooks", ws] as const,
  detail: (ws: string, id: string) => ["notebooks", ws, id] as const,
};

const base = (ws: string) => `/ws/${encodeURIComponent(ws)}/notebooks`;

function jsonBody(method: string, body?: unknown): RequestInit {
  return {
    method,
    headers: { "content-type": "application/json" },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  };
}

export function useNotebooks(ws: string, opts?: { trashed?: boolean }) {
  const trashed = !!opts?.trashed;
  return useQuery<NotebookSummary[]>({
    queryKey: [...qk.list(ws), { trashed }],
    queryFn: () => apiJson<NotebookSummary[]>(`${base(ws)}${trashed ? "?trashed=true" : ""}`),
  });
}

// POST /{id}/trash → 204
export function useTrashNotebook(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiFetch(`${base(ws)}/${id}/trash`, { method: "POST" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.list(ws) }),
  });
}

// POST /{id}/restore → 204
export function useRestoreNotebook(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiFetch(`${base(ws)}/${id}/restore`, { method: "POST" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.list(ws) }),
  });
}

// POST /{id}/bookmark → 204
export function useAddNotebookBookmark(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiFetch(`${base(ws)}/${id}/bookmark`, { method: "POST" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.list(ws) }),
  });
}

// DELETE /{id}/bookmark → 204
export function useRemoveNotebookBookmark(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiFetch(`${base(ws)}/${id}/bookmark`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.list(ws) }),
  });
}

export function useNotebook(ws: string, id: string | null) {
  return useQuery<NotebookDetail>({
    queryKey: id ? qk.detail(ws, id) : ["notebooks", ws, "none"],
    queryFn: () => apiJson<NotebookDetail>(`${base(ws)}/${id}`),
    enabled: !!id,
  });
}

export function useCreateNotebook(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { name?: string }) =>
      apiJson<NotebookDetail>(base(ws), jsonBody("POST", b)),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.list(ws) }),
  });
}

export function useUpdateNotebook(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (
      b: Partial<Pick<NotebookSummary, "name" | "description" | "is_public">>,
    ) => apiJson<NotebookDetail>(`${base(ws)}/${id}`, jsonBody("PATCH", b)),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: qk.detail(ws, id) });
      qc.invalidateQueries({ queryKey: qk.list(ws) });
    },
  });
}

// DELETE notebook → 204, use apiFetch (no body parse)
export function useDeleteNotebook(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => apiFetch(`${base(ws)}/${id}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.list(ws) }),
  });
}

// POST cells → 201 with NotebookDetail body
export function useAddCell(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { cell_type: CellType; source?: string; cell_order?: number }) =>
      apiJson<NotebookDetail>(`${base(ws)}/${id}/cells`, jsonBody("POST", b)),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.detail(ws, id) }),
  });
}

// PATCH cell → 204, use apiFetch (no body parse)
export function useUpdateCell(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (a: { cid: string; cell_type?: CellType; source?: string }) =>
      apiFetch(
        `${base(ws)}/${id}/cells/${a.cid}`,
        jsonBody("PATCH", { cell_type: a.cell_type, source: a.source }),
      ),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.detail(ws, id) }),
  });
}

// DELETE cell → 204, use apiFetch (no body parse)
export function useDeleteCell(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (cid: string) =>
      apiFetch(`${base(ws)}/${id}/cells/${cid}`, { method: "DELETE" }),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.detail(ws, id) }),
  });
}

// PUT cells/order → 204, use apiFetch (no body parse)
export function useReorderCells(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (order: string[]) =>
      apiFetch(`${base(ws)}/${id}/cells/order`, jsonBody("PUT", { order })),
    onSuccess: () => qc.invalidateQueries({ queryKey: qk.detail(ws, id) }),
  });
}
