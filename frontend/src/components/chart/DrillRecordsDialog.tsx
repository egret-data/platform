import { useEffect, useState } from "react";
import { useBuildChartData, type ChartFilter, type QuerySpec } from "@/api/charts";
import { useSubmitQuery, useQueryStatus, useResults } from "@/api/sqllab";
import { ChartTable, type ResultColumn } from "./ChartTable";
import { Dialog, DialogContent, DialogHeader, DialogTitle } from "@/components/ui/dialog";

export function DrillRecordsDialog({
  open, onOpenChange, ws, datasetId, columns, filters, rowLimit = 1000,
}: {
  open: boolean;
  onOpenChange: (o: boolean) => void;
  ws: string;
  datasetId: string | null;
  columns: ResultColumn[];
  filters: ChartFilter[];
  rowLimit?: number;
}) {
  const build = useBuildChartData(ws);
  const submit = useSubmitQuery(ws);
  const [queryId, setQueryId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const status = useQueryStatus(ws, queryId);
  const finished = status.data?.status === "finished";
  const results = useResults(ws, queryId, 0, finished);

  const colKey = columns.map((c) => c.name).join(",");
  useEffect(() => {
    if (!open || !datasetId || columns.length === 0) return;
    setQueryId(null);
    setError(null);
    const spec: QuerySpec = {
      metrics: [], dimensions: [], columns: columns.map((c) => c.name),
      filters, orderby: [], row_limit: rowLimit, rollup_groups: [],
    };
    let aborted = false;
    (async () => {
      try {
        const { sql, database_id } = await build.mutateAsync({ dataset_id: datasetId, query_spec: spec });
        const ack = await submit.mutateAsync({ database_id, sql, limit: rowLimit });
        if (!aborted) setQueryId(ack.id);
      } catch (e) { if (!aborted) setError(String(e)); }
    })();
    return () => { aborted = true; };
  }, [open, datasetId, colKey]);

  const summary = filters.length
    ? filters.map((f) => ("value" in f ? `${f.col} = ${String(f.value)}` : `${f.col} ${f.op}`)).join(", ")
    : "all rows";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-4xl">
        <DialogHeader><DialogTitle>Records where {summary}</DialogTitle></DialogHeader>
        <div className="max-h-[70vh] overflow-auto">
          {error && <p className="rounded bg-destructive/10 p-3 text-sm text-destructive">{error}</p>}
          {status.data?.status === "failed" && <p className="rounded bg-destructive/10 p-3 text-sm text-destructive">{status.data.error}</p>}
          {!error && !finished && status.data?.status !== "failed" && <p className="text-sm text-muted-foreground">Loading…</p>}
          {finished && results.data && (
            results.data.rows.length === 0
              ? <p className="text-sm text-muted-foreground">No matching records.</p>
              : <ChartTable columns={results.data.columns} rows={results.data.rows} />
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
