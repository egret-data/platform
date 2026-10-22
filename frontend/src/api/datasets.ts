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
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";

import { ApiError } from "./client";
import {
  datasetsDsAddColumn,
  datasetsDsAddMetric,
  datasetsDsBookmarkAdd,
  datasetsDsBookmarkRemove,
  datasetsDsCreate,
  datasetsDsDeleteColumn,
  datasetsDsDeleteMetric,
  datasetsDsDuplicate,
  datasetsDsRefresh,
  datasetsDsSetCertification,
  datasetsDsSetMetricCertification,
  datasetsDsRemove,
  datasetsDsRestore,
  datasetsDsTrash,
  datasetsDsUpdate,
  datasetsDsUpdateColumn,
  datasetsDsUpdateMetric,
  getDatasetsDsGetQueryKey,
  getDatasetsDsGetQueryOptions,
  getDatasetsDsListQueryKey,
  useDatasetsDsGet,
  useDatasetsDsList,
} from "./generated/datasets/datasets";
import { getChartsChListQueryKey } from "./generated/charts/charts";

export type DatasetKind = "physical" | "virtual";
export interface DatasetSummary {
  id: string; name: string; description: string; database_id: string;
  kind: DatasetKind; catalog: string | null; schema: string | null; table: string | null;
  column_count: number; updated_at: string;
  folder_id: string | null;
  bookmarked?: boolean;
  deleted_at?: string | null;
  certified?: boolean;
}
export interface DatasetColumn {
  column_name: string; data_type: string; type_generic: string; is_temporal: boolean; ordinal: number;
  expression: string | null; verbose_name: string; description: string; is_calculated: boolean;
  groupby: boolean; filterable: boolean; format: string; semantic_type: string;
}
export interface DatasetMetric {
  id: string; metric_name: string; expression: string; data_type: string; type_generic: string;
  verbose_name: string; description: string; ordinal: number; format: string;
  certified: boolean; certified_by: string; certification_details: string;
}
export interface DatasetUsage { id: string; name: string; viz_type: string; trashed: boolean }
export interface DatasetDetail {
  id: string; name: string; description: string; database_id: string; kind: DatasetKind;
  catalog: string | null; schema: string | null; table: string | null; sql: string | null;
  updated_at: string; columns: DatasetColumn[]; metrics: DatasetMetric[];
  certified: boolean; certified_by: string; certification_details: string;
  /** Set while the dataset is in the trash; its charts keep running until it is deleted. */
  deleted_at?: string | null;
}
/** The answer to Refresh: the dataset now, and the physical columns the source added or dropped. */
export interface DatasetRefresh extends DatasetDetail { added_columns: string[]; removed_columns: string[] }

/** A chart named by a refused delete or rename (409 `code: "in_use"`). */
export interface InUseChart { id: string; name: string; viz_type: string; trashed: boolean }
/**
 * The charts a 409 `in_use` refusal names, or `null` for any other error. The server refuses to
 * delete a dataset or a metric that charts use unless the request carries the object's name as
 * `confirm` (D8), and refuses a metric rename it cannot rewrite the charts for.
 */
export function inUseCharts(e: unknown): InUseChart[] | null {
  if (!(e instanceof ApiError) || e.status !== 409 || e.code !== "in_use") return null;
  const charts = (e.body as { charts?: InUseChart[] } | undefined)?.charts;
  return Array.isArray(charts) ? charts : [];
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
  // No certification fields: the server dropped them from this body in 0109, because sending
  // them here is how an editor used to certify their own dataset — and name someone else as the
  // certifier. Use `useSetDatasetCertification` instead.
  return useMutation<DatasetDetail, ApiError, { name: string; description?: string; sql?: string | null }>({
    mutationFn: async (body) => (await datasetsDsUpdate(ws, id, body)).data as DatasetDetail,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: getDatasetsDsGetQueryKey(ws, id) });
      qc.invalidateQueries({ queryKey: getDatasetsDsListQueryKey(ws) });
    },
  });
}
/**
 * Certify a dataset, or one of its metrics, or withdraw either.
 *
 * SEPARATE from `useUpdateDataset` because the server guards it with the PLATFORM-scoped
 * `dataset:certify` (0109). Two consequences the caller must respect: an editor saving the
 * dataset cannot set this, and there is NO `certified_by` to send — the certifier is the
 * session, so provenance cannot be forged.
 *
 * `metricId` omitted certifies the dataset itself.
 */
export function useSetDatasetCertification(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<DatasetDetail, ApiError, { certified: boolean; details?: string; metricId?: string }>({
    mutationFn: async ({ certified, details = "", metricId }) => {
      const body = { certified, details };
      const r = metricId
        ? await datasetsDsSetMetricCertification(ws, id, metricId, body)
        : await datasetsDsSetCertification(ws, id, body);
      return r.data as DatasetDetail;
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: getDatasetsDsGetQueryKey(ws, id) });
      // The list carries a `certified` badge, so it goes stale too.
      qc.invalidateQueries({ queryKey: getDatasetsDsListQueryKey(ws) });
    },
  });
}

export function useRefreshDataset(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation<DatasetRefresh, ApiError, void>({
    mutationFn: async () => (await datasetsDsRefresh(ws, id)).data as DatasetRefresh,
    onSuccess: () => qc.invalidateQueries({ queryKey: getDatasetsDsGetQueryKey(ws, id) }),
  });
}
/**
 * Delete a dataset. `confirm` is the dataset's name, and the server requires it while charts are
 * built on the dataset (they are deleted with it); without it such a delete is a 409 `in_use`.
 */
export function useDeleteDataset(ws: string) {
  const qc = useQueryClient();
  return useMutation<void, ApiError, { id: string; confirm?: string }>({
    mutationFn: async ({ id, confirm }) => { await datasetsDsRemove(ws, id, confirm === undefined ? undefined : { confirm }); },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: getDatasetsDsListQueryKey(ws) });
      // The charts built on it went with it.
      qc.invalidateQueries({ queryKey: getChartsChListQueryKey(ws) });
    },
  });
}

/** Copy a dataset (either kind) with its column settings and metrics under a new name. */
export function useDuplicateDataset(ws: string) {
  const qc = useQueryClient();
  return useMutation<DatasetDetail, ApiError, { id: string; name: string }>({
    mutationFn: async ({ id, name }) => (await datasetsDsDuplicate(ws, id, { name })).data as DatasetDetail,
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

/** No certification fields — those go through `useSetDatasetCertification`, which the server
 *  guards with the platform-scoped `dataset:certify` (0109). */
export interface MetricInput { metric_name: string; expression: string; verbose_name?: string; description?: string; format?: string }
export interface ColumnUpdate { verbose_name?: string; description?: string; expression?: string; groupby?: boolean; filterable?: boolean; is_temporal?: boolean; format?: string; semantic_type?: string }

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
    // `confirm` is the metric's name; the server requires it while charts use the metric.
    deleteMetric: useMutation({
      mutationFn: (a: { metricId: string; confirm?: string }) =>
        datasetsDsDeleteMetric(ws, id, a.metricId, a.confirm === undefined ? undefined : { confirm: a.confirm }),
      onSuccess: inval,
    }),
    addColumn: useMutation({ mutationFn: (b: { column_name: string; expression: string; verbose_name?: string; description?: string }) => datasetsDsAddColumn(ws, id, b), onSuccess: inval }),
    updateColumn: useMutation({
      // Regenerating the client for the dashboard-revision endpoints also picked up migration
      // 0104's semantic_type allowlist, which orval now emits as a nullable enum. `ColumnUpdate`
      // keeps `string` — the FE-local domain types are deliberately looser than the spec here
      // (see this file's header) — so the widening is confined to this one boundary cast rather
      // than cascading through DatasetColumn and ColumnsTab.
      mutationFn: (a: { name: string; body: ColumnUpdate }) =>
        datasetsDsUpdateColumn(ws, id, a.name, a.body as Parameters<typeof datasetsDsUpdateColumn>[3]),
      onMutate: (a) => optimistic((d) => ({ ...d, columns: d.columns.map((c) => (c.column_name === a.name ? { ...c, ...a.body } : c)) })),
      onError: (_e, _v, ctx) => rollback(ctx as DetailEnvelope | undefined),
      onSettled: inval,
    }),
    deleteColumn: useMutation({ mutationFn: (name: string) => datasetsDsDeleteColumn(ws, id, name), onSuccess: inval }),
  };
}

/** Charts built on the dataset. `includeTrashed` adds the trashed ones: everything a delete takes. */
export function useDatasetUsage(ws: string, id: string | null, opts?: { includeTrashed?: boolean }) {
  const all = !!opts?.includeTrashed;
  return useQuery<DatasetUsage[]>({
    queryKey: ["datasets", ws, "usage", id ?? "", all],
    enabled: !!id,
    queryFn: async () => {
      const r = await fetch(`/api/ws/${encodeURIComponent(ws)}/datasets/${id}/usage${all ? "?include_trashed=true" : ""}`, { credentials: "same-origin" });
      if (!r.ok) throw new Error(`usage failed: ${r.status}`);
      return (await r.json()) as DatasetUsage[];
    },
  });
}

/**
 * For each of `datasetIds`, the names of its columns with the **Filter** switch off (KI-096). A
 * dashboard's filter-to-card mapping draws on every card at once, so this loads one dataset per
 * distinct id, and only while `enabled` (a filter's mapping is being edited). An id still loading
 * or failing has no entry, which offers every column: the switch narrows a picker, and a picker
 * that cannot load the switch must not come up empty.
 */
export function useDatasetsFilterOff(ws: string, datasetIds: string[], enabled: boolean): Map<string, Set<string>> {
  const ids = [...new Set(datasetIds)].sort();
  const results = useQueries({
    queries: ids.map((id) => getDatasetsDsGetQueryOptions(ws, id, { query: { enabled } })),
  });
  const out = new Map<string, Set<string>>();
  results.forEach((r, i) => {
    const d = (r.data as { data?: DatasetDetail } | undefined)?.data;
    if (d?.columns) out.set(ids[i], new Set(d.columns.filter((c) => c.filterable === false).map((c) => c.column_name)));
  });
  return out;
}
