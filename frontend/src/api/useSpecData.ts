import { useQuery } from "@tanstack/react-query";
import { chartsChRunSpec } from "@/api/generated/charts/charts";
import type { ChartDataBody, ChartRunResponse } from "@/api/generated/model";
import type { QuerySpec } from "@/api/charts";

export interface SpecData {
  columns: { name: string; type: string }[];
  rows: unknown[][];
  isLoading: boolean;
  error: string | null;
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
      // shares that exact request body type). The response type also casts:
      // orval unions in the error variants (`ErrorBody`) because the spec
      // documents 4xx/5xx bodies, but `apiFetch`/the mutator throws on
      // non-2xx, so `.data` is always the success shape at runtime.
      return (await chartsChRunSpec(ws, body as unknown as ChartDataBody)).data as ChartRunResponse;
    },
  });

  return {
    columns: q.data?.columns ?? [],
    rows: q.data?.rows ?? [],
    isLoading: q.isFetching,
    error: q.error ? String(q.error) : null,
  };
}
