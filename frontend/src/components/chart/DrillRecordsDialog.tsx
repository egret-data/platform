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

  // "Records where all rows" is not a sentence. A drill with no filters was unreachable until the
  // scalar arrived — it has no dimensions, so its drill context is always empty — which is how a
  // string that had always been wrong first became visible.
  // The operator has to be printed, not assumed to be `=`. A drill on a bucketed time dimension
  // sends a half-open range, and hard-coding `=` rendered it as the nonsense
  // "order_date = 2026-01-01, order_date = 2026-02-01" — two contradictory equalities describing
  // what is actually one month.
  const OPS: Record<string, string> = {
    eq: "=", ne: "≠", gt: ">", ge: "≥", lt: "<", le: "≤",
    like: "like", not_like: "not like", in: "in", not_in: "not in",
  };
  const title = filters.length
    ? `Records where ${filters
        .map((f) => ("value" in f ? `${f.col} ${OPS[f.op] ?? f.op} ${String(f.value)}` : `${f.col} ${f.op}`))
        .join(", ")}`
    : "All records";

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-4xl">
        <DialogHeader><DialogTitle>{title}</DialogTitle></DialogHeader>
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
