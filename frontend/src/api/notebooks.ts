/**
 * Notebooks API — MIGRATED to the orval-generated client (E0·P4d-2).
 *
 * Thin adapter over `./generated/notebooks/notebooks` (+ the kernel
 * active-executions read from `./generated/kernel/kernel`), following the P4c/
 * P4d-1 pattern (see `databases.ts` / `datasets.ts`): hooks keep their old
 * names/signatures so call sites and tests don't churn; reads wrap the generated
 * query hook (unwrapping orval's `{ status, data, headers }` envelope via
 * `select`); writes run `useMutation` over the generated fetch fns and
 * invalidate the GENERATED query keys.
 *
 * The domain TS types stay FE-local: the spec models `cell_type` loosely (plain
 * `string`) and makes several notebook fields optional, so aliasing to the
 * generated DTOs would widen the `CellType` union / churn call sites. We keep
 * the hand-written types exactly as before.
 *
 * KERNEL: only the REST kernel `active-executions` GET is wired here. The
 * kernel channels WebSocket + the connect/interrupt/restart REST calls live in
 * `@/lib/kernel/KernelConnection` — the WS upgrade cannot be expressed by the
 * generated fetch client, so that connection code stays hand-written (its REST
 * calls now go through the generated kernel fetch fns; see that file).
 */
import { useMutation, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  getKernelActiveExecutionsQueryKey,
  useKernelActiveExecutions,
} from "./generated/kernel/kernel";
import {
  getNotebooksGetOneQueryKey,
  getNotebooksListQueryKey,
  getNotebooksRuntimeQueryKey,
  notebooksAddCell,
  notebooksBookmarkAdd,
  notebooksBookmarkRemove,
  notebooksCreate,
  notebooksDeleteCell,
  notebooksRemove,
  notebooksReorder,
  notebooksRestore,
  notebooksTrash,
  notebooksUpdate,
  notebooksUpdateCell,
  useNotebooksGetOne,
  useNotebooksList,
  useNotebooksRuntime,
} from "./generated/notebooks/notebooks";

export type CellType = "code" | "markdown" | "sql";

export interface Cell {
  id: string;
  cell_type: CellType;
  source: string;
  cell_order: number;
  execution_count: number | null;
  last_output: unknown | null;
  last_execution_time_ms: number | null;
  sql_target?: string | null;
  sql_into?: string | null;
}

/** Pick a collision-free default DataFrame variable name for a new SQL cell:
 *  `df`, then `df_2`, `df_3`, … skipping any `sql_into` already in use. */
export function defaultSqlInto(cells: Cell[]): string {
  const used = new Set(cells.map((c) => c.sql_into).filter(Boolean) as string[]);
  if (!used.has("df")) return "df";
  for (let i = 2; ; i++) {
    const candidate = `df_${i}`;
    if (!used.has(candidate)) return candidate;
  }
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

/** Back-compat query-key helpers; delegate to the generated key factories. */
export const qk = {
  list: (ws: string) => getNotebooksListQueryKey(ws),
  detail: (ws: string, id: string) => getNotebooksGetOneQueryKey(ws, id),
};

export function useNotebooks(ws: string, opts?: { trashed?: boolean }) {
  const trashed = !!opts?.trashed;
  return useNotebooksList<NotebookSummary[], ApiError>(
    ws,
    trashed ? { trashed: true } : undefined,
    { query: { select: (r) => r.data as NotebookSummary[] } },
  );
}

// POST /{id}/trash → 204
export function useTrashNotebook(ws: string) {
  const qc = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await notebooksTrash(ws, id);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: getNotebooksListQueryKey(ws) }),
  });
}

// POST /{id}/restore → 204
export function useRestoreNotebook(ws: string) {
  const qc = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await notebooksRestore(ws, id);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: getNotebooksListQueryKey(ws) }),
  });
}

// POST /{id}/bookmark → 204
export function useAddNotebookBookmark(ws: string) {
  const qc = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await notebooksBookmarkAdd(ws, id);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: getNotebooksListQueryKey(ws) }),
  });
}

// DELETE /{id}/bookmark → 204
export function useRemoveNotebookBookmark(ws: string) {
  const qc = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await notebooksBookmarkRemove(ws, id);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: getNotebooksListQueryKey(ws) }),
  });
}

export function useNotebook(ws: string, id: string | null) {
  return useNotebooksGetOne<NotebookDetail, ApiError>(ws, id ?? "", {
    query: { enabled: !!id, select: (r) => r.data as NotebookDetail },
  });
}

export function useCreateNotebook(ws: string) {
  const qc = useQueryClient();
  return useMutation<NotebookDetail, ApiError, { name?: string }>({
    mutationFn: async (b) => (await notebooksCreate(ws, b)).data as NotebookDetail,
    onSuccess: () => qc.invalidateQueries({ queryKey: getNotebooksListQueryKey(ws) }),
  });
}

export function useUpdateNotebook(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<
    NotebookDetail,
    ApiError,
    Partial<Pick<NotebookSummary, "name" | "description" | "is_public">>
  >({
    mutationFn: async (b) => (await notebooksUpdate(ws, id, b)).data as NotebookDetail,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: getNotebooksGetOneQueryKey(ws, id) });
      qc.invalidateQueries({ queryKey: getNotebooksListQueryKey(ws) });
    },
  });
}

// DELETE notebook → 204
export function useDeleteNotebook(ws: string) {
  const qc = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => {
      await notebooksRemove(ws, id);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: getNotebooksListQueryKey(ws) }),
  });
}

// POST cells → 201 with NotebookDetail body
export function useAddCell(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<
    NotebookDetail,
    ApiError,
    {
      cell_type: CellType;
      source?: string;
      cell_order?: number;
      sql_target?: string;
      sql_into?: string;
    }
  >({
    mutationFn: async (b) => (await notebooksAddCell(ws, id, b)).data as NotebookDetail,
    onSuccess: () => qc.invalidateQueries({ queryKey: getNotebooksGetOneQueryKey(ws, id) }),
  });
}

// PATCH cell → 204
export function useUpdateCell(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<
    void,
    ApiError,
    {
      cid: string;
      cell_type?: CellType;
      source?: string;
      sql_target?: string;
      sql_into?: string;
    }
  >({
    mutationFn: async (a) => {
      await notebooksUpdateCell(ws, id, a.cid, {
        cell_type: a.cell_type,
        source: a.source,
        sql_target: a.sql_target,
        sql_into: a.sql_into,
      });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: getNotebooksGetOneQueryKey(ws, id) }),
  });
}

// DELETE cell → 204
export function useDeleteCell(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (cid) => {
      await notebooksDeleteCell(ws, id, cid);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: getNotebooksGetOneQueryKey(ws, id) }),
  });
}

// PUT cells/order → 204
export function useReorderCells(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<void, ApiError, string[]>({
    mutationFn: async (order) => {
      await notebooksReorder(ws, id, { order });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: getNotebooksGetOneQueryKey(ws, id) }),
  });
}

export interface EngineOption {
  id: string;
  name: string;
  engine: string;
  enabled: boolean;
}

export interface NotebookRuntime {
  username: string;
  engines: EngineOption[];
  engine_jwt: string | null;
  engine_jwt_expires_at: string | null;
}

// GET /ws/{ws}/notebook-runtime → NotebookRuntime
export function useNotebookRuntime(ws: string) {
  return useNotebooksRuntime<NotebookRuntime, ApiError>(ws, {
    query: {
      staleTime: 60_000,
      queryKey: getNotebooksRuntimeQueryKey(ws),
      select: (r) => r.data as NotebookRuntime,
    },
  });
}

// GET /ws/{ws}/notebooks/{id}/kernel/active-executions → { running: string[] }
export function useActiveExecutions(ws: string, id: string) {
  return useKernelActiveExecutions<{ running: string[] }, ApiError>(ws, id, {
    query: {
      queryKey: getKernelActiveExecutionsQueryKey(ws, id),
      select: (r) => r.data as { running: string[] },
    },
  });
}
