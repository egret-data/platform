import type { ChartFilter, QuerySpec } from "@/api/charts";
import { useSpecData } from "@/api/useSpecData";
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
  const spec: QuerySpec | null = columns.length > 0
    ? { metrics: [], dimensions: [], columns: columns.map((c) => c.name), filters, orderby: [], row_limit: rowLimit, rollup_groups: [] }
    : null;
  const data = useSpecData(ws, datasetId, spec, open && !!datasetId && columns.length > 0);

  const summary = filters.length
    ? filters.map((f) => ("value" in f ? `${f.col} = ${String(f.value)}` : `${f.col} ${f.op}`)).join(", ")
    : "all rows";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-4xl">
        <DialogHeader><DialogTitle>Records where {summary}</DialogTitle></DialogHeader>
        <div className="max-h-[70vh] overflow-auto">
          {data.error && <p className="rounded bg-destructive/10 p-3 text-sm text-destructive">{data.error}</p>}
          {/* Gate on `hasResult`, not `!isLoading`: while the dialog is open but
              `columns` hasn't arrived yet (the dataset-detail race this component
              exists to handle), the query is disabled, so `isLoading` is false
              without a result ever having landed — `!isLoading` alone would flash
              "No matching records." before the real query has even fired. */}
          {!data.error && !data.hasResult && <p className="text-sm text-muted-foreground">Loading…</p>}
          {!data.error && data.hasResult && (
            data.rows.length === 0
              ? <p className="text-sm text-muted-foreground">No matching records.</p>
              : <ChartTable columns={data.columns} rows={data.rows} />
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
