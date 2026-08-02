import { formatValue } from "./format";
import type { ColumnFormat } from "./format-types";
import { namesAPeriod, periodName } from "./temporal-period";
import { SERIES_PALETTE } from "./echarts/palette";

interface ResultColumn { name: string; type: string }

const UP = SERIES_PALETTE[1];   // green
const DOWN = SERIES_PALETTE[3]; // red

/**
 * Metabase-style Trend (SmartScalar): the latest value plus its change vs the
 * previous period. Rows are expected ordered by the dimension; the last two are
 * compared. Clean-room of Metabase's SmartScalar comparison.
 */
/** The period a temporal unit names, for the comparison caption. Metabase writes "vs. previous
 *  month" rather than a bare "vs. previous" — the reader should not have to remember which
 *  granularity the chart was grouped by to know what it is being compared against. */
const PERIOD_NAME: Record<string, string> = {
  minute: "minute", hour: "hour", day: "day", week: "week",
  month: "month", quarter: "quarter", year: "year",
};

export function SmartScalarChart({
  columns, rows, dimension, metric, format = {}, dimensionFormat, unit,
  switchPositiveNegative = false, compact = false,
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  dimension?: string;
  metric?: string;
  format?: ColumnFormat;
  /** The dimension's own format, when the user set one. UNDEFINED means they did not, and that
   *  distinction is load-bearing — an empty object would be indistinguishable from a format that
   *  happens to hold only defaults, and `periodLabel` below has to know whether to defer to it. */
  dimensionFormat?: ColumnFormat;
  /** The dimension's temporal unit, when it has one. Names the period being compared against, in
   *  the caption, AND the period the label above the number covers. */
  unit?: string;
  /** Metabase's "Switch positive / negative colors?". A metric where LESS is better — cost, churn,
   *  time-to-resolve — reads backwards otherwise: green for the number going up. The same defect
   *  the gauge's fixed low-red/high-green zones had before chart 04. */
  switchPositiveNegative?: boolean;
  /** Metabase's "Compact number": 36,513 -> 36.5k, for a card too small to hold the full figure. */
  compact?: boolean;
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
  const fmt = (v: number) => formatValue(v, type, compact ? { ...format, compact: true } : format);
  // The label above the number is the PERIOD the latest row covers, and it used to print that
  // period's first DAY — "December 1, 2026" for a monthly comparison whose caption right beneath it
  // said "vs previous month". Observed live on the ZZ Chart Parity harness. Same three answers, in
  // the same order, as the cartesian x-axis (`echarts/options/cartesian.ts`), because it is the same
  // question asked of the same column:
  //   1. the dimension's own format when the user set one — `dimensionFormat` is undefined, not
  //      `{}`, when they have not, which is the whole reason it carries no default;
  //   2. otherwise the period's name, at the unit `PERIOD_NAME` below already uses for the caption;
  //   3. otherwise `formatValue`, which is where a non-bucketed date column keeps its default
  //      "March 1, 2026" and a non-temporal dimension keeps its plain string.
  //
  // NOT full parity with Metabase's trend card, deliberately, and the gap is narrow and recorded:
  // Metabase formats this one through `formatDateTimeRangeWithUnit(..., {compact: true})`
  // (`SmartScalar/compute.ts:702`), which abbreviates the month at EVERY unit ("Sep 2026") and
  // expands a week into its span ("Jan 5–11, 2026"). That is a second formatter — a condensed
  // range — not a different argument to this one, and `periodName`'s own doc says so. What is fixed
  // here is the defect; what is left is an abbreviation.
  const periodLabel = (v: unknown) =>
    dimensionFormat != null ? formatValue(v, dimType, dimensionFormat)
      : namesAPeriod(unit) ? periodName(unit, v)
        : formatValue(v, dimType);
  const curLabel = curRow && di >= 0 ? periodLabel(curRow[di]) : "";
  // Which way is good. `up`/`down` stay factual — they drive the arrow — while the COLOUR follows
  // this, so the arrow keeps saying what happened even when the reading of it is inverted.
  const good = switchPositiveNegative ? down : up;
  const bad = switchPositiveNegative ? up : down;
  const period = unit != null ? PERIOD_NAME[unit] : undefined;
  const vsLabel = period ? `vs previous ${period}:` : "vs previous:";

  return (
    <div className="flex h-full min-h-[5rem] flex-col items-center justify-center gap-1"
      role="img"
      aria-label={[
        curLabel ? `${curLabel}: ` : "",
        hasCur ? fmt(current) : "no value",
        hasComparison
          ? `, ${up ? "up" : down ? "down" : "unchanged"}${Number.isFinite(pct) ? ` ${(Math.abs(pct) * 100).toFixed(2)}%` : ""} versus previous ${period ? `${period} ` : ""}${fmt(previous)}`
          : "",
      ].join("")}>
      <div className="text-[2.5rem] font-bold leading-none tabular-nums text-foreground">{hasCur ? fmt(current) : "—"}</div>
      {curLabel && <div className="text-sm text-muted-foreground">{curLabel}</div>}
      {hasComparison && (
        <div className="flex items-center gap-1.5 text-sm">
          <span data-trend className="flex items-center gap-1 font-medium" style={{ color: good ? UP : bad ? DOWN : undefined }}>
            {up ? "▲" : down ? "▼" : "•"}{" "}
            {Number.isFinite(pct) ? `${(Math.abs(pct) * 100).toFixed(2)}%` : fmt(Math.abs(change))}
          </span>
          <span className="text-muted-foreground">{vsLabel} {fmt(previous)}</span>
        </div>
      )}
    </div>
  );
}
