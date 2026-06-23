import { formatValue } from "./format";
import type { ColumnFormat } from "./format-types";

interface ResultColumn { name: string; type: string }

/** Metabase-style "Number" viz: one aggregated value, large and formatted. */
export function BigNumber({
  columns, rows, metric, format = {},
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  metric?: string;
  format?: ColumnFormat;
}) {
  const mi = metric != null ? columns.findIndex((c) => c.name === metric) : -1;
  const raw = mi >= 0 && rows.length > 0 ? rows[0][mi] : undefined;
  const type = mi >= 0 ? columns[mi].type : "double";
  const text = raw == null ? "—" : formatValue(raw, type, format);

  return (
    <div className="flex h-full min-h-[5rem] flex-col items-center justify-center">
      <div className="text-[2.5rem] font-bold leading-none tabular-nums text-foreground">{text}</div>
      {metric && <div className="mt-2 text-sm text-muted-foreground">{metric}</div>}
    </div>
  );
}
