import { useMemo } from "react";
import type { QuerySpec, ChartFilter } from "@/api/charts";
import { useSpecData } from "@/api/useSpecData";

export interface ColumnValues { values: string[]; isLoading: boolean; error: string | null }

/** Distinct values of one column of a dataset, for filter dropdowns. */
export function useColumnValues(
  ws: string,
  datasetId: string | null,
  column: string | null,
  enabled: boolean,
  filters: ChartFilter[] = [],
): ColumnValues {
  // Select the column directly rather than as a group-by dimension: the engine
  // query builder requires at least one metric or column, so a dimensions-only
  // spec is rejected (400). Rows are deduped client-side below, so a plain
  // column select is correct.
  //
  // A new `spec` object is built on every render (its identity is not stable),
  // but `useSpecData` folds it into its query key via `JSON.stringify`, so
  // react-query dedupes on the serialized content rather than object identity
  // — a fresh identity each render does not trigger a refetch. Verified in
  // `__tests__/useColumnValues.test.tsx` ("does not refetch on an unrelated
  // rerender").
  const spec: QuerySpec | null = column
    ? { metrics: [], dimensions: [], columns: [column], filters, orderby: [], row_limit: 1000, rollup_groups: [] }
    : null;

  const data = useSpecData(ws, datasetId, spec, enabled && !!datasetId && !!column);

  const values = useMemo(
    () => Array.from(new Set(data.rows.map((r) => r[0]).filter((v) => v != null).map(String))),
    [data.rows],
  );
  return { values, isLoading: data.isLoading, error: data.error };
}
