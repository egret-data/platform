import { formatValue } from "./format";
import type { ColumnFormat } from "./format-types";
import { SERIES_PALETTE } from "./echarts/palette";

interface ResultColumn { name: string; type: string }

const UP = SERIES_PALETTE[1];   // green
const DOWN = SERIES_PALETTE[3]; // red

/**
 * Metabase-style Trend (SmartScalar): the latest value plus its change vs the
 * previous period. Rows are expected ordered by the dimension; the last two are
 * compared. Clean-room of Metabase's SmartScalar comparison.
 */
export function SmartScalarChart({
  columns, rows, dimension, metric, format = {},
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  dimension?: string;
  metric?: string;
  format?: ColumnFormat;
}) {
  const mi = metric != null ? columns.findIndex((c) => c.name === metric) : -1;
  const di = dimension != null ? columns.findIndex((c) => c.name === dimension) : -1;
  const type = mi >= 0 ? columns[mi].type : "double";

  const valid = mi >= 0 ? rows.filter((r) => Number.isFinite(Number(r[mi]))) : [];
  const n = valid.length;
  const curRow = n > 0 ? valid[n - 1] : undefined;
  const prevRow = n > 1 ? valid[n - 2] : undefined;
  const current = curRow ? Number(curRow[mi]) : NaN;
  const previous = prevRow ? Number(prevRow[mi]) : NaN;

  const hasCur = Number.isFinite(current);
  const hasComparison = hasCur && Number.isFinite(previous);
  const change = hasComparison ? current - previous : NaN;
  const pct = hasComparison && previous !== 0 ? change / Math.abs(previous) : NaN;
  const up = change > 0;
  const down = change < 0;

  const dimType = di >= 0 ? columns[di].type : "varchar";
  const fmt = (v: number) => formatValue(v, type, format);
  const curLabel = curRow && di >= 0 ? formatValue(curRow[di], dimType) : "";

  return (
    <div className="flex h-full min-h-[5rem] flex-col items-center justify-center gap-1">
      <div className="text-[2.5rem] font-bold leading-none tabular-nums text-foreground">{hasCur ? fmt(current) : "—"}</div>
      {curLabel && <div className="text-sm text-muted-foreground">{curLabel}</div>}
      {hasComparison && (
        <div className="flex items-center gap-1.5 text-sm">
          <span data-trend className="flex items-center gap-1 font-medium" style={{ color: up ? UP : down ? DOWN : undefined }}>
            {up ? "▲" : down ? "▼" : "•"}{" "}
            {Number.isFinite(pct) ? `${(Math.abs(pct) * 100).toFixed(2)}%` : fmt(Math.abs(change))}
          </span>
          <span className="text-muted-foreground">vs previous: {fmt(previous)}</span>
        </div>
      )}
    </div>
  );
}
