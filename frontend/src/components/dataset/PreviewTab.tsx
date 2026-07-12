import { useEffect, useRef } from "react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ChartTable, type ResultColumn } from "@/components/chart/ChartTable";
import { useDatasetsDsData } from "@/api/generated/datasets/datasets";

interface Props {
  ws: string;
  datasetId: string;
}

const PREVIEW_LIMIT = 100;

// Data-preview tab: runs the object-scoped `POST /ws/{ws}/datasets/{id}/data`
// (bounded `SELECT * … LIMIT n`) once on mount and on demand via Refresh. A
// governed viewer without data access gets a 403/5xx here — we surface the
// message rather than crash.
export function PreviewTab({ ws, datasetId }: Props) {
  const preview = useDatasetsDsData();
  const { mutate } = preview;

  // Run once on mount (and if the target dataset changes); guarded so a
  // re-render doesn't re-fire the request.
  const ranFor = useRef<string | null>(null);
  useEffect(() => {
    const key = `${ws}/${datasetId}`;
    if (ranFor.current === key) return;
    ranFor.current = key;
    mutate({ workspace: ws, id: datasetId, data: { limit: PREVIEW_LIMIT } });
  }, [ws, datasetId, mutate]);

  function refresh() {
    mutate({ workspace: ws, id: datasetId, data: { limit: PREVIEW_LIMIT } });
  }

  const result = preview.data?.status === 200 ? preview.data.data : undefined;

  const header = (
    <div className="mb-2 flex items-center justify-between">
      <span className="text-sm text-muted-foreground">
        Preview — first {PREVIEW_LIMIT} rows
      </span>
      <Button size="sm" variant="outline" onClick={refresh} disabled={preview.isPending}>
        {preview.isPending ? "Loading…" : "Refresh"}
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
        <ChartTable columns={mappedCols} rows={result.rows} />
      </div>
    </div>
  );
}
