import { useEffect, useState } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { formatValue } from "@/components/chart/format";
import type { ColumnFormat } from "@/components/chart/format-types";
import type { ResultColumn } from "@/components/chart/ChartTable";

/** Renders a single record as a vertical field/value list, paging through the rows. */
export function ObjectDetail({
  columns,
  rows,
  columnSettings = {},
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  columnSettings?: Record<string, ColumnFormat>;
}) {
  const [index, setIndex] = useState(0);
  // Reset to the first record whenever the data changes (a new query → new rows reference).
  useEffect(() => setIndex(0), [rows]);

  if (rows.length === 0) {
    return <div className="flex h-full items-center justify-center text-sm text-muted-foreground">No data</div>;
  }
  const i = Math.min(index, rows.length - 1);
  const row = rows[i];

  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <dl className="divide-y">
          {columns.map((c, ci) => (
            <div key={c.name} className="flex gap-3 px-3 py-2">
              <dt className="w-2/5 shrink-0 truncate text-xs font-medium uppercase tracking-wide text-muted-foreground">{c.name}</dt>
              <dd className="flex-1 break-words text-sm tabular-nums">{formatValue(row[ci], c.type, columnSettings[c.name])}</dd>
            </div>
          ))}
        </dl>
      </div>
      {rows.length > 1 && (
        <div className="flex items-center justify-between border-t px-3 py-1.5 text-xs text-muted-foreground">
          <button type="button" aria-label="previous record" disabled={i === 0}
            onClick={() => setIndex(i - 1)} className="disabled:opacity-40">
            <ChevronLeft className="h-4 w-4" />
          </button>
          <span>{i + 1} / {rows.length}</span>
          <button type="button" aria-label="next record" disabled={i >= rows.length - 1}
            onClick={() => setIndex(i + 1)} className="disabled:opacity-40">
            <ChevronRight className="h-4 w-4" />
          </button>
        </div>
      )}
    </div>
  );
}
