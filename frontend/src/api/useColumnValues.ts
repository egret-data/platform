import { useEffect, useMemo, useState } from "react";
import { useBuildChartData } from "@/api/charts";
import type { QuerySpec } from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";

export interface ColumnValues { values: string[]; isLoading: boolean; error: string | null }

/** Distinct values of one column of a dataset, for filter dropdowns. */
export function useColumnValues(ws: string, datasetId: string | null, column: string | null, enabled: boolean): ColumnValues {
  const build = useBuildChartData(ws);
  const submit = useSubmitQuery(ws);
  const [queryId, setQueryId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const status = useQueryStatus(ws, queryId);
  const finished = status.data?.status === "finished";
  const results = useResults(ws, queryId, 0, finished);

  const key = enabled && datasetId && column ? `${datasetId}:${column}` : "";
  useEffect(() => {
    let cancelled = false;
    if (!key || !datasetId || !column) return;
    setQueryId(null); setError(null);
    (async () => {
      try {
        const spec: QuerySpec = { metrics: [], dimensions: [column], columns: [], filters: [], orderby: [], row_limit: 1000, rollup_groups: [] };
        const { sql, database_id } = await build.mutateAsync({ dataset_id: datasetId, query_spec: spec });
        if (cancelled) return;
        const q = await submit.mutateAsync({ database_id, sql, limit: 1000 });
        if (!cancelled) setQueryId(q.id);
      } catch (e) { if (!cancelled) setError(String(e)); }
    })();
    return () => { cancelled = true; };
  }, [ws, key]);

  const values = useMemo(() => {
    const raw = results.data?.rows ?? [];
    return Array.from(new Set(raw.map((r) => r[0]).filter((v) => v != null).map(String)));
  }, [results.data]);
  return { values, isLoading: enabled && !error && !(finished && results.data), error };
}
