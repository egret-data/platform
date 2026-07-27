import { useQuery, keepPreviousData } from "@tanstack/react-query";
import { chartsChRunSpec } from "@/api/generated/charts/charts";
import type { ChartDataBody, ChartRunResponse } from "@/api/generated/model";
import type { QuerySpec } from "@/api/charts";

export interface SpecData {
  columns: { name: string; type: string }[];
  rows: unknown[][];
  isLoading: boolean;
  error: string | null;
  /**
   * True once a run has actually completed successfully at least once for the
   * current query key — as distinct from `!isLoading`, which is also true
   * while the query is simply disabled (e.g. `enabled: false`, or a
   * precondition like "columns haven't arrived yet" hasn't been met). A
   * caller that wants to tell "no rows came back" apart from "we haven't
   * asked yet" should gate on this, not on `!isLoading && !error`.
   */
  hasResult: boolean;
}

/**
 * Run a `QuerySpec` against a dataset and return the rows.
 *
 * Replaces the old build -> submit -> poll pipeline that went through the
 * free-form SQL endpoint (`POST /queries`). That endpoint is now gated on the
 * SQL Lab entitlement, and chart work must keep working in a workspace where
 * SQL Lab is switched off — so it goes through `POST /charts/run`, which
 * compiles and executes the spec server-side in one request.
 */
export function useSpecData(
  ws: string,
  datasetId: string | null,
  spec: QuerySpec | null,
  enabled: boolean,
  nonce?: number,
): SpecData {
  const key = datasetId && spec ? JSON.stringify(spec) : "";
  const q = useQuery({
    queryKey: ["chartsChRunSpec", ws, datasetId, key, nonce],
    enabled: enabled && !!datasetId && !!spec,
    queryFn: async () => {
      const body: { dataset_id: string; query_spec: QuerySpec } = { dataset_id: datasetId!, query_spec: spec! };
      // The FE `QuerySpec` (discriminated `ChartFilter[]`) is stricter than the
      // generated `ChartDataBody.query_spec`; cast the body to the generated
      // shape, same as `useBuildChartData` in `@/api/charts` (this endpoint
      // shares that exact request body type). A direct cast compiles cleanly
      // (no `unknown` hop needed), so the rest of the shape stays checked —
      // only the known-safe `ChartFilter.value: unknown` vs `Filter.value:
      // FilterValue` mismatch is being bridged. The response type also casts:
      // orval unions in the error variants (`ErrorBody`) because the spec
      // documents 4xx/5xx bodies, but `apiFetch`/the mutator throws on
      // non-2xx, so `.data` is always the success shape at runtime.
      return (await chartsChRunSpec(ws, body as ChartDataBody)).data as ChartRunResponse;
    },
    // Keep the previous result on screen across auto-run refetches (only the
    // very first load shows the loading skeleton) instead of flashing back to
    // it on every debounced edit.
    placeholderData: keepPreviousData,
  });

  return {
    columns: q.data?.columns ?? [],
    rows: q.data?.rows ?? [],
    // `q.isLoading` (TanStack's `isPending && isFetching`) rather than
    // `q.isFetching`: with `placeholderData: keepPreviousData` the previous
    // key's data is treated as present during a refetch, so `isPending` (and
    // thus `isLoading`) goes false immediately — callers keep rendering the
    // prior chart instead of unmounting it for a loading skeleton on every
    // debounced auto-run. `isFetching` alone would stay true across every
    // refetch and defeat that. First-ever load (no data yet) is unaffected:
    // isPending is true until the first response lands.
    isLoading: q.isLoading,
    error: q.error ? String(q.error) : null,
    hasResult: q.data !== undefined,
  };
}
