import { useEffect, useState } from "react";
import type { Chart } from "@/api/charts";
import { useBuildChartData } from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";

type Col = { name: string; type: string };

export interface ChartData {
  columns: Col[];
  rows: unknown[][];
  isLoading: boolean;
  error: string | null;
}

/** Run a saved chart's query via the existing build->submit->poll pipeline. */
export function useChartData(ws: string, chart: Chart | null, nonce?: number): ChartData {
  const build = useBuildChartData(ws);
  const submit = useSubmitQuery(ws);
  const [queryId, setQueryId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const status = useQueryStatus(ws, queryId);
  const finished = status.data?.status === "finished";
  const results = useResults(ws, queryId, 0, finished);

  const specKey = chart ? `${chart.id}:${JSON.stringify(chart.params.spec)}` : "";
  useEffect(() => {
    let cancelled = false;
    if (!chart) return;
    setQueryId(null);
    setError(null);
    (async () => {
      try {
        const { sql, database_id } = await build.mutateAsync({ dataset_id: chart.dataset_id, query_spec: chart.params.spec });
        if (cancelled) return;
        const q = await submit.mutateAsync({ database_id, sql, limit: chart.params.spec.row_limit });
        if (!cancelled) setQueryId(q.id);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => { cancelled = true; };
  }, [ws, specKey, nonce]);

  const failed = status.data?.status === "failed" || status.data?.status === "cancelled";
  return {
    columns: results.data?.columns ?? [],
    rows: results.data?.rows ?? [],
    isLoading: !!chart && !error && !failed && !(finished && results.data),
    error: error ?? (failed ? (status.data?.error ?? "Query failed") : null),
  };
}
