import { useMemo, useRef } from "react";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { GRID_COLS } from "./dashboard-grid";
import { formatValue, specifiesDateRendering } from "./format";
import type { ColumnFormat } from "./format-types";
import { findSize, measureText } from "./measure-text";
import {
  compactifyValue, MIN_SIZE_REM, SCALAR_PADDING, SIZE_STEP_REM, maxFontSizeRem,
} from "./scalar";
import { namesAPeriod, periodName } from "./temporal-period";
import { useElementSize } from "./useElementSize";
import { SERIES_PALETTE } from "./echarts/palette";

interface ResultColumn { name: string; type: string }

const UP = SERIES_PALETTE[1];   // green
const DOWN = SERIES_PALETTE[3]; // red

/** Fallback for the first paint, before the ResizeObserver has reported a box. This is the size
 * the card was FIXED at before it could size itself, so the initial frame is never larger than
 * what it settles to — `BigNumber` picks its own fallback the same way and for the same reason. */
const INITIAL_SIZE = "2.5rem";
const VALUE_FONT_WEIGHT = 700;

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
  switchPositiveNegative = false, compact = false, onValueClick, gridCols,
}: {
  columns: ResultColumn[];
  rows: unknown[][];
  dimension?: string;
  metric?: string;
  format?: ColumnFormat;
  /** The dimension's own format, when the user set one. UNDEFINED means they did not, and that
   *  distinction is load-bearing — an empty object would be indistinguishable from a format that
   *  happens to hold only defaults, and `periodLabel` below has to know whether to defer to it.
   *  Not sufficient on its own, either: see `specifiesDateRendering` for the objects the panel
   *  mints that are present and yet say nothing. */
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
  /**
   * Present when the chart is drillable. Handed the RAW dimension value of the row the big number
   * came from — the LATEST period — which `ChartRenderer` turns into a `DrillContext`, attaching
   * the temporal unit so the filter covers the whole period rather than its first instant.
   *
   * `undefined` when there is no dimension column configured (or the latest row holds no value for
   * it): the caller then falls back to the dimension-less drill the gauge and the scalar make.
   *
   * ONLY THE VALUE IS A CLICK TARGET. Metabase wraps its primary value alone
   * (`SmartScalar.tsx:125-134`); nothing under its `PreviousValueComparison/` carries an `onClick`,
   * so the comparison period is never turned into a filter. Kept that way deliberately — the
   * comparison is a DERIVED reading (a difference and a percentage), and the period it names is
   * one the reader can already reach by drilling the trend's own chart.
   */
  onValueClick?: (latestDimensionValue: unknown, x: number, y: number) => void;
  /** The card's width in dashboard grid columns. A card that owns more of the grid earns a bigger
   * size ceiling; without it (the studio preview) the ceiling is `NO_GRID_MAX_SIZE`. Same field,
   * same meaning and the same source as `BigNumber`'s. */
  gridCols?: number;
}) {
  const hostRef = useRef<HTMLDivElement>(null);
  const captionRef = useRef<HTMLDivElement>(null);
  const size = useElementSize(hostRef);
  const captionSize = useElementSize(captionRef);
  // The period label and the comparison line sit in the same centred column as the number, so the
  // room left for the number is the box minus whatever they take. MEASURED rather than assumed —
  // `BigNumber` records what assuming cost there: a value host given `flex-1 basis-0` swallowed the
  // spare height and stranded its captions ~300px below the number, passing every numeric check.
  const captionHeight = captionSize?.height ?? 0;

  const mi = metric != null ? columns.findIndex((c) => c.name === metric) : -1;
  const di = dimension != null ? columns.findIndex((c) => c.name === dimension) : -1;
  const type = mi >= 0 ? columns[mi].type : "double";

  /**
   * The rows this chart can read a number off. `r[mi] != null && r[mi] !== ""` IS LOAD-BEARING and
   * was missing: `Number(null)` and `Number("")` are both `0`, and `0` is finite — so a trailing
   * month with no data yet passed for a real one. The card then read "0", labelled itself with that
   * month, and said "▼ 100.00% vs previous month" — a metric reported as having collapsed to
   * nothing when the truth was that nothing had been recorded. Wrong, confident, and with nothing
   * on screen to say so.
   *
   * Found by the drill this chart gained: it pointed at that empty period. `false` and `[]` coerce
   * to 0 the same way, but neither is a shape a numeric column returns; null and the empty string
   * both are.
   */
  const valid = mi >= 0
    ? rows.filter((r) => r[mi] != null && r[mi] !== "" && Number.isFinite(Number(r[mi])))
    : [];
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
  // said "vs previous month". Observed live on the ZZ Chart Parity harness. The first two answers
  // are the cartesian x-axis's (`echarts/options/cartesian.ts`, `drawLabel`), in the same order,
  // because it is the same question asked of the same column. THE THIRD IS NOT THE SAME, and that
  // difference is the whole reason `namesAPeriod` is called here at all:
  //   1. the dimension's own format when it says something about dates — `dimensionFormat` is
  //      undefined, not `{}`, when the user set nothing, which is why it carries no default, and
  //      `specifiesDateRendering` is why a stored `{column_title: "Month"}` does not count as
  //      having set one;
  //   2. otherwise the period's name, at the unit `PERIOD_NAME` below already uses for the caption;
  //   3. otherwise `formatValue` — where the AXIS's third answer is THE RAW STRING. A date column
  //      here keeps its default "March 1, 2026" rather than reverting to "2026-03-01", which is
  //      what shipped and what a card carrying a single date should read as. `periodName` is total
  //      and so serves as arm 3 on the axis for free; it cannot serve as this one, so this site
  //      has to know whether arm 2 applies BEFORE calling — and that is `namesAPeriod`'s only job.
  //
  // NOT full parity with Metabase's trend card, deliberately, and the gap is narrower than it
  // first looks: `year` and `quarter` already match exactly. Metabase formats this label through
  // `formatDateTimeRangeWithUnit(..., {compact: true})` (`SmartScalar/compute.ts:702`), whose
  // `compact` turns the month token to "MMM" (`lib/formatting/date.tsx:717`) and expands a week
  // into its span. So what differs is `month`/`day`/`hour`/`minute` abbreviating the month name,
  // and `week` reading as its first day. That is a second formatter — a condensed range — not a
  // different argument to this one, and `periodName`'s own doc says so. What is fixed here is the
  // defect; what is left is an abbreviation.
  const dateFormatWins =
    dimensionFormat != null && (namesAPeriod(unit) ? specifiesDateRendering(dimensionFormat) : true);
  const periodLabel = (v: unknown) =>
    dateFormatWins ? formatValue(v, dimType, dimensionFormat)
      : namesAPeriod(unit) ? periodName(unit, v)
        : formatValue(v, dimType);
  const curLabel = curRow && di >= 0 ? periodLabel(curRow[di]) : "";
  // Which way is good. `up`/`down` stay factual — they drive the arrow — while the COLOUR follows
  // this, so the arrow keeps saying what happened even when the reading of it is inverted.
  const good = switchPositiveNegative ? down : up;
  const bad = switchPositiveNegative ? up : down;
  const period = unit != null ? PERIOD_NAME[unit] : undefined;
  const vsLabel = period ? `vs previous ${period}:` : "vs previous:";

  /**
   * What to draw for the big number, and what it really says.
   *
   * COMPACT AGAINST THE BOX, THEN SIZE THE RESULT — `BigNumber`'s order, and it matters: sizing the
   * uncompacted string would pick a font for text that is not on screen. `format.compact` (the
   * user's own "Compact number" switch) still wins outright inside `compactifyValue`; what this
   * adds is the case they have not decided, where the alternative is the number spilling out of the
   * card, which is what it did.
   *
   * `full` is the uncompacted figure whatever the setting, so the tooltip and the accessible name
   * below can both offer it.
   */
  const { display, full } = useMemo(() => {
    if (!hasCur) return { display: "—", full: "—" };
    const width = size ? Math.max(size.width - SCALAR_PADDING, 0) : Number.POSITIVE_INFINITY;
    return compactifyValue(current, type, width, compact ? { ...format, compact: true } : format);
  }, [hasCur, current, type, size, format, compact]);

  const fontSize = useMemo(() => {
    if (!size || !hasCur) return INITIAL_SIZE;
    const host = hostRef.current;
    // The family must be the one the DOM actually renders: a fit computed against the wrong family
    // is wrong by however much the two differ in advance width (chart 05's scar, in full at
    // `BigNumber`).
    const fontFamily = host ? window.getComputedStyle(host).fontFamily : "inherit";
    return findSize({
      text: display,
      targetHeight: Math.max(size.height - captionHeight - SCALAR_PADDING, 0),
      targetWidth: Math.max(size.width - SCALAR_PADDING, 0),
      unit: "rem",
      fontFamily,
      fontWeight: VALUE_FONT_WEIGHT,
      step: SIZE_STEP_REM,
      min: MIN_SIZE_REM,
      max: maxFontSizeRem(gridCols, GRID_COLS, size.width),
      measurer: measureText,
    });
  }, [size, display, hasCur, gridCols, captionHeight]);

  const isCompacted = display !== full;

  const spoken = [
    curLabel ? `${curLabel}: ` : "",
    // THE FULL FIGURE, not what is drawn. A screen reader has no use for "24.7k" when the number is
    // 24,730.43, and compaction is now applied without the user asking for it — so the precision it
    // trades away on screen has to stay in the accessible name, where the width does not bind.
    hasCur ? full : "no value",
    hasComparison
      ? `, ${up ? "up" : down ? "down" : "unchanged"}${Number.isFinite(pct) ? ` ${(Math.abs(pct) * 100).toFixed(2)}%` : ""} versus previous ${period ? `${period} ` : ""}${fmt(previous)}`
      : "",
  ].join("");
  const clickable = onValueClick != null && hasCur;
  // THE ROW THE NUMBER ACTUALLY CAME FROM — `valid`'s last entry, not `rows`'s. A trailing row
  // whose metric is null or non-finite is filtered out above, so the period on screen is the one
  // before it; drilling `rows[rows.length - 1]` would filter a period the card is not showing.
  const curDim = curRow && di >= 0 ? curRow[di] : undefined;

  // `leading-none` and `tabular-nums` stay; the fixed `text-[2.5rem]` does not. That constant is
  // what put 203.75px of number in a 166px card — a class cannot answer to a card width, and the
  // trend's card is exactly the one that varies most, from a dashboard tile to a full-page preview.
  const valueEl = (
    <span
      className="font-bold leading-none tabular-nums text-foreground"
      style={{ fontSize, fontWeight: VALUE_FONT_WEIGHT }}
    >
      {display}
    </span>
  );

  const interactive = clickable ? (
    <button
      type="button"
      className="group rounded focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring group-hover:text-primary"
      aria-label={`${spoken}. Show records`}
      onClick={(e) => {
        const r = e.currentTarget.getBoundingClientRect();
        onValueClick!(curDim, r.left + r.width / 2, r.bottom);
      }}
    >
      {valueEl}
    </button>
  ) : valueEl;

  return (
    <div className="relative flex h-full min-h-[5rem] w-full flex-col items-center justify-center gap-1"
      // `img` collapses its subtree, which is what makes the whole card read as one sentence — and
      // what makes a control inside it unreachable. `group` keeps the same name while leaving the
      // value operable. Same trade as the funnel's step columns and `BigNumber`'s number.
      role={clickable ? "group" : "img"}
      aria-label={spoken}>
      {/* `absolute inset-0`: this measures the WHOLE box without taking part in the layout, so the
          number and its captions stay one centred group. Sizing it by `flex-1` instead made it
          swallow the spare height and stranded `BigNumber`'s captions at the bottom of the card. */}
      <div ref={hostRef} aria-hidden className="pointer-events-none absolute inset-0" />

      <div className="flex w-full min-w-0 items-center justify-center px-2">
        {isCompacted ? (
          <TooltipProvider delayDuration={200}>
            <Tooltip>
              {/* `asChild` keeps the button/span as the trigger, so the exact figure is reachable by
                  keyboard focus and not only by hover. */}
              <TooltipTrigger asChild>{interactive}</TooltipTrigger>
              <TooltipContent>{full}</TooltipContent>
            </Tooltip>
          </TooltipProvider>
        ) : (
          interactive
        )}
      </div>

      {/* ONE MEASURED BLOCK, so the space the captions take can be subtracted from the number's own.
          The inner `gap-1` restates the outer one, which now separates this block from the number
          rather than each caption from its neighbour — the spacing on screen is unchanged. */}
      <div ref={captionRef} className="flex w-full shrink-0 flex-col items-center gap-1">
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
    </div>
  );
}
