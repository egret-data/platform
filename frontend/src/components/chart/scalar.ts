import type { ScalarSegment } from "@/api/charts";
import type { ColumnFormat } from "./format-types";
import { formatValue } from "./format";

/* ── Auto-sizing bounds ──────────────────────────────────────────────────────────────────────
 * Metabase's `getMaxFontSize` (components/ScalarValue/utils.ts). Geometry constants, so copied
 * verbatim per the parity rules. egret's GRID_COLS is 24, identical to Metabase's GRID_WIDTH, so
 * even the ratio ports unchanged.
 */
export const MIN_SIZE_REM = 1;
export const MAX_SIZE_SMALL = 2.2;
export const MAX_SIZE_LARGE = 7;
export const NARROW_CARD_WIDTH = 240;
export const NARROW_CARD_MAX_SIZE = 2.5;
export const SIZE_STEP_REM = 0.2;
/** Metabase subtracts this twice from the height and once from the width, which is why its number
 * never touches the card edge. */
export const SCALAR_PADDING = 32;

/**
 * The largest font the value is allowed to reach, in rem.
 *
 * `cardCols`/`totalCols` are the card's width and the grid's width in GRID COLUMNS — a card that
 * owns more of the grid earns a bigger ceiling. When the card is not on a grid at all (the studio
 * preview) there are no columns to read and the ceiling is `MAX_SIZE_LARGE`; the fit loop still
 * bounds it by the real box.
 */
export function maxFontSizeRem(
  cardCols: number | undefined,
  totalCols: number | undefined,
  cardWidthPx: number | undefined,
): number {
  if (cardWidthPx != null && cardWidthPx < NARROW_CARD_WIDTH) return NARROW_CARD_MAX_SIZE;
  if (cardCols == null || totalCols == null || totalCols <= 0) return MAX_SIZE_LARGE;
  return (cardCols / totalCols) * (MAX_SIZE_LARGE - MAX_SIZE_SMALL) + MAX_SIZE_SMALL;
}

/* ── Width-driven compaction ─────────────────────────────────────────────────────────────────
 * Metabase's `compactifyValue` (visualizations/lib/scalar_utils.tsx). Copied verbatim.
 */
export const COMPACT_MAX_WIDTH = 250;
export const COMPACT_WIDTH_PER_DIGIT = 25;
export const COMPACT_MIN_LENGTH = 6;

export function shouldCompact(fullValue: string, width: number): boolean {
  return (
    fullValue.length > COMPACT_MIN_LENGTH &&
    (width < COMPACT_MAX_WIDTH || width < fullValue.length * COMPACT_WIDTH_PER_DIGIT)
  );
}

/**
 * What to draw, and what the full value is.
 *
 * An explicit `format.compact` still wins — this only decides the case the user has not decided.
 * The caller shows `full` in a tooltip whenever it differs from `display`, which is what makes
 * compaction safe: the exact figure is never more than one interaction away.
 */
export function compactifyValue(
  value: unknown,
  columnType: string,
  width: number,
  format: ColumnFormat = {},
): { display: string; full: string } {
  const full = formatValue(value, columnType, { ...format, compact: false });
  const display = format.compact || shouldCompact(full, width)
    ? formatValue(value, columnType, { ...format, compact: true })
    : full;
  return { display, full };
}

/* ── Conditional colour bands ────────────────────────────────────────────────────────────────── */

/** A band is usable when it constrains at least one end. Metabase:
 * `segmentIsValid(s, { allowOpenEnded: true })` — a band open at BOTH ends matches everything and
 * says nothing, so it is dropped rather than allowed to swallow every value. */
export function isValidSegment(s: ScalarSegment | null | undefined): s is ScalarSegment {
  if (s == null) return false;
  const hasMin = typeof s.min === "number" && Number.isFinite(s.min);
  const hasMax = typeof s.max === "number" && Number.isFinite(s.max);
  return hasMin || hasMax;
}

/**
 * The first band containing `value`, or `undefined`.
 *
 * **First match wins, and nothing is sorted.** That is Metabase's `segments.find` semantics, and
 * it is why the editor must render bands in authored order: re-ordering them would change which
 * band a value lands in. (The gauge does sort by `max` — its ECharts `axisLine` stops are
 * cumulative, so it has to. A scalar is a plain interval test and must not inherit that.)
 */
export function matchSegment(value: unknown, segments?: ScalarSegment[]): ScalarSegment | undefined {
  const n = parseFloat(String(value));
  if (!Number.isFinite(n) || !segments) return undefined;
  return segments.filter(isValidSegment).find((s) => {
    const min = typeof s.min === "number" && Number.isFinite(s.min) ? s.min : -Infinity;
    const max = typeof s.max === "number" && Number.isFinite(s.max) ? s.max : Infinity;
    return min <= n && n <= max;
  });
}

/** A band's range as text: `1 - 10`, `≥ 1`, `≤ 10`. Copied from Metabase's `formatSegmentRange`. */
export function formatSegmentRange(s: ScalarSegment): string {
  const hasMin = typeof s.min === "number" && Number.isFinite(s.min);
  const hasMax = typeof s.max === "number" && Number.isFinite(s.max);
  if (hasMin && hasMax) return `${s.min} - ${s.max}`;
  if (hasMin) return `≥ ${s.min}`;
  if (hasMax) return `≤ ${s.max}`;
  return "";
}
