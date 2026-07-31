import { formatValue } from "./format";
import type { ColumnFormat } from "./format-types";
import { SERIES_PALETTE, scaleLightness } from "./echarts/palette";

interface ResultColumn { name: string; type: string }

/** Metabase's own default (`color("accent1")`). Overridable, like its `progress.color`. */
const DEFAULT_COLOR = SERIES_PALETTE[1]; // #88BF4D

/**
 * The track shades, DERIVED from the bar colour the way Metabase derives them
 * (`getProgressColors`: `lighten(0.25)` for the in-progress track, `darken(0.3)` for the exceeded
 * one). They used to be two fixed hexes copied from a screenshot of the default green — which
 * agreed with Metabase only for that one colour, and the moment the colour became configurable
 * would have painted every other one's track green.
 */
const trackLight = (main: string) => scaleLightness(main, 1.25);
const trackDark = (main: string) => scaleLightness(main, 0.7);

/** Metabase's `getProgressMessage`, verbatim. egret said nothing at all — the reader had to
 *  compare the bar against the goal label to learn they had got there. */
function progressMessage(hasValue: boolean, hasGoal: boolean, value: number, goal: number): string {
  if (!hasValue && !hasGoal) return "No data available";
  if (!hasValue) return "No value data";
  if (!hasGoal) return "No goal set";
  if (value === goal) return "Goal met";
  if (value > goal) return "Goal exceeded";
  return "";
}

/**
 * Metabase-style Progress: one metric vs a goal, as a horizontal bar.
 * Clean-room of Progress/utils.ts: when below goal the track is the goal and the
 * fill is value/goal; when exceeded the track is the value and the fill is goal/value,
 * so the overshoot is visible. A pointer marks the value position.
 */
export function ProgressChart({
  columns, rows, metric, goal = 0, goalColumn, color, format = {},
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  metric?: string;
  goal?: number;
  /** Metabase's goal setting takes a NUMBER OR A COLUMN. Named separately here rather than
   *  overloading `goal`, so the stored shape stays typed and a stale column name cannot silently
   *  read as a number. Wins over `goal` when it names a column the result actually has. */
  goalColumn?: string;
  /** Metabase's `progress.color`. */
  color?: string;
  format?: ColumnFormat;
}) {
  const mi = metric != null ? columns.findIndex((c) => c.name === metric) : -1;
  const raw = mi >= 0 && rows.length > 0 ? rows[0][mi] : undefined;
  const value = Number(raw);
  const type = mi >= 0 ? columns[mi].type : "double";
  const hasValue = raw != null && Number.isFinite(value);

  // A goal read from a column comes from the same single row the value does.
  const gi = goalColumn != null ? columns.findIndex((c) => c.name === goalColumn) : -1;
  const goalRaw = gi >= 0 && rows.length > 0 ? rows[0][gi] : undefined;
  const goalFromColumn = gi >= 0 ? Number(goalRaw) : NaN;
  const effectiveGoal = Number.isFinite(goalFromColumn) ? goalFromColumn : goal;

  const hasGoal = Number.isFinite(effectiveGoal) && effectiveGoal >= 0;
  const exceeded = hasValue && hasGoal && value > effectiveGoal;

  const MAIN = color || DEFAULT_COLOR;
  const LIGHT = trackLight(MAIN);
  const DARK = trackDark(MAIN);
  const message = progressMessage(hasValue, hasGoal, value, effectiveGoal);

  const barPercent = hasValue && hasGoal
    ? (value === effectiveGoal ? 1 : Math.max(0, value < effectiveGoal ? value / effectiveGoal : effectiveGoal / value))
    : 0;
  const arrowPercent = hasValue && hasGoal ? Math.max(0, value < effectiveGoal ? value / effectiveGoal : 1) : 0;

  const fmt = (v: number) => formatValue(v, type, format);

  return (
    // A progress bar is exactly what ARIA's progressbar role describes; without it a screen
    // reader gets a number and two loose axis labels.
    <div className="flex h-full min-h-[5rem] flex-col justify-center px-10"
      role="progressbar"
      aria-valuenow={hasValue ? value : undefined}
      aria-valuemin={0}
      aria-valuemax={hasGoal ? effectiveGoal : undefined}
      aria-valuetext={hasValue ? `${fmt(value)}${hasGoal ? ` of ${fmt(effectiveGoal)}` : ""}${message ? `. ${message}` : ""}` : "no value"}>
      <div className="relative">
        {/* The value floats above the fill end, like Metabase. */}
        <div data-progress-value className="absolute -top-7 -translate-x-1/2 whitespace-nowrap text-sm font-semibold tabular-nums text-foreground"
          style={{ left: `${arrowPercent * 100}%` }}>
          {hasValue ? fmt(value) : "—"}
        </div>
        {/* Its own row, left-aligned, the way Metabase places it — NOT appended to the floating
            value. The value is centred on the fill end, so anything added to it hangs half its
            width past that point: measured, the pair reached 76px beyond the right end of the bar
            and within 13px of the card edge, and a longer message ("No data available") would have
            gone over. */}
        {message && (
          <div data-progress-message className="absolute -top-3 left-0 text-xs text-muted-foreground">
            {message}
          </div>
        )}
        {hasGoal && hasValue && (
          <div
            data-progress-pointer
            className="absolute -top-2 h-0 w-0 -translate-x-1/2 border-x-[6px] border-x-transparent"
            style={{ left: `${arrowPercent * 100}%`, borderTop: `7px solid ${exceeded ? DARK : MAIN}` }}
          />
        )}
        <div className="h-5 w-full overflow-hidden rounded" style={{ backgroundColor: exceeded ? DARK : LIGHT }}>
          <div data-progress-fill className="h-full rounded" style={{ width: `${barPercent * 100}%`, backgroundColor: MAIN }} />
        </div>
        {hasGoal && (
          <div className="mt-1 flex justify-between text-xs text-muted-foreground">
            <span>{fmt(0)}</span>
            <span>Goal {fmt(effectiveGoal)}</span>
          </div>
        )}
      </div>
    </div>
  );
}
