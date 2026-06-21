import { formatValue } from "./format";
import type { ColumnFormat } from "./format-types";
import { SERIES_PALETTE } from "./echarts/palette";

interface ResultColumn { name: string; type: string }

const MAIN = SERIES_PALETTE[0]; // #509EE3
const LIGHT = "#D9ECFB";        // track, in progress
const DARK = "#2C6FA8";         // track, goal exceeded

/**
 * Metabase-style Progress: one metric vs a goal, as a horizontal bar.
 * Clean-room of Progress/utils.ts: when below goal the track is the goal and the
 * fill is value/goal; when exceeded the track is the value and the fill is goal/value,
 * so the overshoot is visible. A pointer marks the value position.
 */
export function ProgressChart({
  columns, rows, metric, goal = 0, format = {},
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  metric?: string;
  goal?: number;
  format?: ColumnFormat;
}) {
  const mi = metric != null ? columns.findIndex((c) => c.name === metric) : -1;
  const raw = mi >= 0 && rows.length > 0 ? rows[0][mi] : undefined;
  const value = Number(raw);
  const type = mi >= 0 ? columns[mi].type : "double";
  const hasValue = raw != null && Number.isFinite(value);
  const hasGoal = Number.isFinite(goal) && goal >= 0;
  const exceeded = hasValue && hasGoal && value > goal;

  const barPercent = hasValue && hasGoal
    ? (value === goal ? 1 : Math.max(0, value < goal ? value / goal : goal / value))
    : 0;
  const arrowPercent = hasValue && hasGoal ? Math.max(0, value < goal ? value / goal : 1) : 0;

  const fmt = (v: number) => formatValue(v, type, format);

  return (
    <div className="flex h-[40vh] flex-col justify-center gap-4 px-8">
      <div className="flex items-baseline justify-between">
        <div className="text-[2rem] font-bold tabular-nums text-foreground">{hasValue ? fmt(value) : "—"}</div>
        {hasGoal && <div className="text-sm text-muted-foreground">Goal {fmt(goal)}</div>}
      </div>
      <div className="relative">
        {hasGoal && (
          <div
            data-progress-pointer
            className="absolute -top-2 h-0 w-0 -translate-x-1/2 border-x-8 border-x-transparent"
            style={{ left: `${arrowPercent * 100}%`, borderTop: `8px solid ${exceeded ? DARK : MAIN}` }}
          />
        )}
        <div className="h-4 w-full overflow-hidden rounded" style={{ backgroundColor: exceeded ? DARK : LIGHT }}>
          <div data-progress-fill className="h-full rounded" style={{ width: `${barPercent * 100}%`, backgroundColor: MAIN }} />
        </div>
      </div>
    </div>
  );
}
