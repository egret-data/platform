/**
 * Datasets API — MIGRATED to the orval-generated client (E0·P4d-1).
 *
 * Thin adapter over `./generated/datasets/datasets`, following the P4c pattern
 * (see `databases.ts`): hooks keep their old names/signatures so call sites and
 * tests don't churn; reads wrap the generated query hook (unwrapping orval's
 * `{ status, data, headers }` envelope via `select`); writes run `useMutation`
 * over the generated fetch fns and invalidate the GENERATED query keys.
 *
 * The domain TS types stay FE-local: the spec models `kind`/`source` more
 * loosely (plain `string`) than the FE unions (`DatasetKind`, `CreateSource`),
 * so aliasing to the generated DTOs would widen them and churn call sites.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  datasetsDsAddColumn,
  datasetsDsAddMetric,
  datasetsDsBookmarkAdd,
  datasetsDsBookmarkRemove,
  datasetsDsCreate,
  datasetsDsDeleteColumn,
  datasetsDsDeleteMetric,
  datasetsDsRefresh,
  datasetsDsRemove,
  datasetsDsRestore,
  datasetsDsTrash,
  datasetsDsUpdate,
  datasetsDsUpdateColumn,
  datasetsDsUpdateMetric,
  getDatasetsDsGetQueryKey,
  getDatasetsDsListQueryKey,
  useDatasetsDsGet,
  useDatasetsDsList,
} from "./generated/datasets/datasets";

export type DatasetKind = "physical" | "virtual";
export interface DatasetSummary {
  id: string; name: string; description: string; database_id: string;
  kind: DatasetKind; catalog: string | null; schema: string | null; table: string | null;
  column_count: number; updated_at: string;
  folder_id: string | null;
  bookmarked?: boolean;
  deleted_at?: string | null;
}
export interface DatasetColumn {
  column_name: string; data_type: string; type_generic: string; is_temporal: boolean; ordinal: number;
  expression: string | null; verbose_name: string; description: string; is_calculated: boolean;
  groupby: boolean; filterable: boolean; format: string;
}
export interface DatasetMetric {
  id: string; metric_name: string; expression: string; data_type: string; type_generic: string;
  verbose_name: string; description: string; ordinal: number; format: string; certified: boolean;
}
export interface DatasetUsage { id: string; name: string }
export interface DatasetDetail {
  id: string; name: string; description: string; database_id: string; kind: DatasetKind;
  catalog: string | null; schema: string | null; table: string | null; sql: string | null;
  updated_at: string; columns: DatasetColumn[]; metrics: DatasetMetric[];
}
export type CreateSource =
  | { kind: "physical"; catalog?: string | null; schema: string; table: string }
  | { kind: "virtual"; sql: string };
export interface CreateDatasetBody {
  name: string; database_id: string; description?: string; source: CreateSource;
}

/** Back-compat query-key helpers; delegate to the generated key factories. */
export const dsKeys = {
  list: (ws: string) => getDatasetsDsListQueryKey(ws),
  detail: (ws: string, id: string) => getDatasetsDsGetQueryKey(ws, id),
};

export function useDatasets(ws: string, opts?: { trashed?: boolean }) {
  const trashed = !!opts?.trashed;
  return useDatasetsDsList<DatasetSummary[], ApiError>(ws, trashed ? { trashed: true } : undefined, {
    query: { select: (r) => r.data as DatasetSummary[] },
  });
}
export function useDataset(ws: string, id: string | null) {
  return useDatasetsDsGet<DatasetDetail, ApiError>(ws, id ?? "", {
    query: { enabled: !!id, select: (r) => r.data as DatasetDetail },
  });
}
export function useCreateDataset(ws: string) {
  const qc = useQueryClient();
  return useMutation<DatasetDetail, ApiError, CreateDatasetBody>({
    mutationFn: async (body) => (await datasetsDsCreate(ws, body)).data as DatasetDetail,
    onSuccess: () => qc.invalidateQueries({ queryKey: getDatasetsDsListQueryKey(ws) }),
  });
}
export function useUpdateDataset(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<DatasetDetail, ApiError, { name: string; description?: string; sql?: string | null }>({
    mutationFn: async (body) => (await datasetsDsUpdate(ws, id, body)).data as DatasetDetail,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: getDatasetsDsGetQueryKey(ws, id) });
      qc.invalidateQueries({ queryKey: getDatasetsDsListQueryKey(ws) });
    },
  });
}
export function useRefreshDataset(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<DatasetDetail, ApiError, void>({
    mutationFn: async () => (await datasetsDsRefresh(ws, id)).data as DatasetDetail,
    onSuccess: () => qc.invalidateQueries({ queryKey: getDatasetsDsGetQueryKey(ws, id) }),
  });
}
export function useDeleteDataset(ws: string) {
  const qc = useQueryClient();
  return useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await datasetsDsRemove(ws, id); },
    onSuccess: () => qc.invalidateQueries({ queryKey: getDatasetsDsListQueryKey(ws) }),
  });
}

export function useDatasetListMutations(ws: string) {
  const qc = useQueryClient();
  const invalidate = () => qc.invalidateQueries({ queryKey: getDatasetsDsListQueryKey(ws) });
  const trash = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await datasetsDsTrash(ws, id); },
    onSuccess: invalidate,
  });
  const restore = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await datasetsDsRestore(ws, id); },
    onSuccess: invalidate,
  });
  const addBookmark = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await datasetsDsBookmarkAdd(ws, id); },
    onSuccess: invalidate,
  });
  const removeBookmark = useMutation<void, ApiError, string>({
    mutationFn: async (id) => { await datasetsDsBookmarkRemove(ws, id); },
    onSuccess: invalidate,
  });
  return { trash, restore, addBookmark, removeBookmark };
}

export interface MetricInput { metric_name: string; expression: string; verbose_name?: string; description?: string; format?: string; certified?: boolean }
export interface ColumnUpdate { verbose_name?: string; description?: string; expression?: string; groupby?: boolean; filterable?: boolean; is_temporal?: boolean; format?: string }

// The detail query caches orval's envelope (`{ status, data, headers }`); the
// `select` in useDataset only transforms the hook's return, not the cache.
type DetailEnvelope = { data: DatasetDetail } & Record<string, unknown>;

export function useDatasetMutations(ws: string, id: string) {
  const qc = useQueryClient();
  const key = getDatasetsDsGetQueryKey(ws, id);
  const inval = () => qc.invalidateQueries({ queryKey: key });

  // Optimistically patch the cached detail so semantic-layer toggles/inputs
  // respond instantly instead of waiting for the round-trip + refetch (the
  // Switch is server-controlled, so without this the thumb wouldn't move until
  // the mutation settled). Returns the prior envelope for rollback on error.
  const optimistic = async (mut: (d: DatasetDetail) => DatasetDetail) => {
    await qc.cancelQueries({ queryKey: key });
    const prev = qc.getQueryData<DetailEnvelope>(key);
    if (prev?.data) qc.setQueryData<DetailEnvelope>(key, { ...prev, data: mut(prev.data) });
    return prev;
  };
  const rollback = (ctx: DetailEnvelope | undefined) => { if (ctx) qc.setQueryData(key, ctx); };

  return {
    // Bodies carry the semantic-layer fields (format/certified, groupby/filterable/
    // is_temporal); the generated client forwards them as JSON.
    addMetric: useMutation({ mutationFn: (b: MetricInput) => datasetsDsAddMetric(ws, id, b), onSuccess: inval }),
    updateMetric: useMutation({
      mutationFn: (a: { metricId: string; body: MetricInput }) => datasetsDsUpdateMetric(ws, id, a.metricId, a.body),
      onMutate: (a) => optimistic((d) => ({ ...d, metrics: d.metrics.map((m) => (m.id === a.metricId ? { ...m, ...a.body } : m)) })),
      onError: (_e, _v, ctx) => rollback(ctx as DetailEnvelope | undefined),
      onSettled: inval,
    }),
    deleteMetric: useMutation({ mutationFn: (metricId: string) => datasetsDsDeleteMetric(ws, id, metricId), onSuccess: inval }),
    addColumn: useMutation({ mutationFn: (b: { column_name: string; expression: string; verbose_name?: string; description?: string }) => datasetsDsAddColumn(ws, id, b), onSuccess: inval }),
    updateColumn: useMutation({
      mutationFn: (a: { name: string; body: ColumnUpdate }) => datasetsDsUpdateColumn(ws, id, a.name, a.body),
      onMutate: (a) => optimistic((d) => ({ ...d, columns: d.columns.map((c) => (c.column_name === a.name ? { ...c, ...a.body } : c)) })),
      onError: (_e, _v, ctx) => rollback(ctx as DetailEnvelope | undefined),
      onSettled: inval,
    }),
    deleteColumn: useMutation({ mutationFn: (name: string) => datasetsDsDeleteColumn(ws, id, name), onSuccess: inval }),
  };
}

export function useDatasetUsage(ws: string, id: string | null) {
  return useQuery<DatasetUsage[]>({
    queryKey: ["datasets", ws, "usage", id ?? ""],
    enabled: !!id,
    queryFn: async () => {
      const r = await fetch(`/api/ws/${encodeURIComponent(ws)}/datasets/${id}/usage`, { credentials: "same-origin" });
      if (!r.ok) throw new Error(`usage failed: ${r.status}`);
      return (await r.json()) as DatasetUsage[];
    },
  });
}
