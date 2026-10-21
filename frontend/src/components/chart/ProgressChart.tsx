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
  columns, rows, metric, goal, goalColumn, color, format = {}, onValueClick,
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
  /** Present when the chart is drillable. A progress bar shows ONE aggregate against a goal, so its
   *  drill context is empty — "show me the records behind this number", the same one the scalar and
   *  the gauge make. Metabase's `Progress.tsx:72-77` builds `{ value, column, settings, event }`
   *  with no `dimensions`, and hangs it on the WHOLE bar: there is no "clicked at 60% of the track"
   *  behaviour to copy, and the goal is a setting rather than a column of anything. */
  onValueClick?: (x: number, y: number) => void;
}) {
  const mi = metric != null ? columns.findIndex((c) => c.name === metric) : -1;
  const raw = mi >= 0 && rows.length > 0 ? rows[0][mi] : undefined;
  const value = Number(raw);
  const type = mi >= 0 ? columns[mi].type : "double";
  const hasValue = raw != null && Number.isFinite(value);

  // A goal read from a column comes from the same single row the value does.
  const gi = goalColumn != null ? columns.findIndex((c) => c.name === goalColumn) : -1;
  const goalRaw = gi >= 0 && rows.length > 0 ? rows[0][gi] : undefined;
  // `Number(null)` is 0, so an empty goal cell is ruled out before converting — as the typed goal
  // is: an empty Goal box stores nothing, and nothing is "no goal set", never a goal of 0 (KI-044).
  const goalFromColumn = gi >= 0 && goalRaw != null && goalRaw !== "" ? Number(goalRaw) : NaN;
  const effectiveGoal = Number.isFinite(goalFromColumn) ? goalFromColumn : (goal ?? NaN);

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
  /** The whole reading, as one sentence — the progressbar's `aria-valuetext` and, when the chart is
   *  clickable, the button's accessible name. One expression so the two can never disagree. */
  const reading = hasValue
    ? `${fmt(value)}${hasGoal ? ` of ${fmt(effectiveGoal)}` : ""}${message ? `. ${message}` : ""}`
    : "no value";
  const clickable = onValueClick != null && hasValue;

  const OUTER = "flex h-full min-h-[5rem] w-full flex-col justify-center px-10";

  const bar = (
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
            gone over.
            `bottom-full mb-1` rather than a fixed negative offset: a -12px top against a ~15px
            line box left the descenders sitting 3px INSIDE the bar. Anchoring the message's BOTTOM
            to the bar's top keeps the gap correct whatever the font metrics turn out to be. */}
        {message && (
          <div data-progress-message className="absolute bottom-full left-0 mb-1 text-xs text-muted-foreground">
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
  );

  /**
   * THE TWO SHAPES THIS RENDERS IN, and why it is a choice rather than an oversight.
   *
   * ARIA's `progressbar` is exactly what this chart is, and without it a screen reader gets a
   * number and two loose axis labels. But `progressbar` is CHILDREN-PRESENTATIONAL: a control
   * nested inside one is not reachable at all, so "keep the role and put a button in it" is not an
   * option that works — it is the option that looks like it works.
   *
   * So the clickable form drops the role and moves the whole reading onto the button's accessible
   * name, which is the same trade `BigNumber` makes at the same seam (it drops its own `role="img"`
   * the moment it becomes interactive). Nothing spoken is lost — `reading` is the identical string
   * `aria-valuetext` carried — and what is gained is a drill a keyboard can reach. The unclickable
   * form, which is what the studio preview and a dashboard in edit mode render, is untouched.
   */
  if (!clickable) {
    return (
      <div className={OUTER}
        role="progressbar"
        aria-valuenow={hasValue ? value : undefined}
        aria-valuemin={0}
        aria-valuemax={hasGoal ? effectiveGoal : undefined}
        aria-valuetext={reading}>
        {bar}
      </div>
    );
  }
  return (
    <button
      type="button"
      className={`${OUTER} text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring`}
      aria-label={`${metric ? `${metric}: ` : ""}${reading}. Show records`}
      onClick={(e) => onValueClick!(e.clientX, e.clientY)}
    >
      {bar}
    </button>
  );
}
