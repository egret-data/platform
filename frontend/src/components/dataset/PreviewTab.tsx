import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ChartTable, type ResultColumn } from "@/components/chart/ChartTable";
import { datasetColumnDefaults } from "@/components/dataset/columnDefaults";
import type { DatasetColumn } from "@/api/datasets";
import { datasetsDsData } from "@/api/generated/datasets/datasets";

interface Props {
  ws: string;
  datasetId: string;
  columns?: DatasetColumn[];
}

const PREVIEW_LIMIT = 100;

// Data-preview tab: runs the object-scoped `POST /ws/{ws}/datasets/{id}/data`
// (bounded `SELECT * … LIMIT n`) once on mount and on demand via Refresh. A
// governed viewer without data access gets a 403/5xx here — we surface the
// message rather than crash.
export function PreviewTab({ ws, datasetId, columns = [] }: Props) {
  // A QUERY, not a mutation-fired-from-an-effect.
  //
  // This used to call the generated `useDatasetsDsData()` mutation from a mount
  // effect, guarded by a ref so a re-render could not re-fire it. Under
  // `React.StrictMode` — which is on in dev (main.tsx) — effects run twice, the
  // guard suppressed the second run, and the component was left reporting
  // `isPending` forever: permanent skeletons and a disabled Refresh button, even
  // though the POST had already come back 200.
  //
  // "Fetch on mount, refetch on demand" is what a query IS. react-query then owns
  // the deduplication the ref guard was hand-rolling, and StrictMode's double
  // effect stops mattering. `retry: false` keeps the old behaviour (mutations do
  // not retry); the endpoint is a POST only because it carries a body.
  const preview = useQuery({
    queryKey: ["dataset-preview", ws, datasetId, PREVIEW_LIMIT],
    queryFn: () => datasetsDsData(ws, datasetId, { limit: PREVIEW_LIMIT }),
    retry: false,
    refetchOnWindowFocus: false,
  });

  // Apply each column's semantic layer to the preview: its format (setting e.g. Currency on a
  // column is visible here, not just stored) and its display name as the header (KI-103). The
  // rows stay keyed by the column name.
  const columnSettings = useMemo(() => datasetColumnDefaults(columns), [columns]);

  function refresh() {
    preview.refetch();
  }

  const result = preview.data?.status === 200 ? preview.data.data : undefined;

  const header = (
    <div className="mb-2 flex items-center justify-between">
      <span className="text-sm text-muted-foreground">
        Preview — first {PREVIEW_LIMIT} rows
      </span>
      {/* `isFetching`, not `isPending`: with a query, `isPending` is only the
          FIRST load, so a Refresh of already-loaded rows would otherwise give no
          feedback and stay clickable mid-flight. */}
      <Button size="sm" variant="outline" onClick={refresh} disabled={preview.isFetching}>
        {preview.isFetching ? "Loading…" : "Refresh"}
      </Button>
    </div>
  );

  if (preview.isPending) {
    return (
      <div>
        {header}
        <div className="space-y-2">
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-8 w-full" />
          <Skeleton className="h-8 w-full" />
        </div>
      </div>
    );
  }

  if (preview.isError) {
    return (
      <div>
        {header}
        <Alert variant="destructive">
          <AlertDescription>{(preview.error as unknown as Error)?.message ?? "Failed to load preview."}</AlertDescription>
        </Alert>
      </div>
    );
  }

  if (!result || result.rows.length === 0) {
    return (
      <div>
        {header}
        <p className="py-2 text-sm text-muted-foreground">No rows.</p>
      </div>
    );
  }

  const mappedCols: ResultColumn[] = result.columns.map((c) => ({ name: c.name, type: c.type }));

  return (
    <div>
      {header}
      <div className="overflow-x-auto">
        <ChartTable columns={mappedCols} rows={result.rows} columnSettings={columnSettings} />
      </div>
    </div>
  );
}
