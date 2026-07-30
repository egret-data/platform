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
/** Metabase's ceiling when there is no grid to measure against: `gridSize ? getMaxFontSize(…) : 4`.
 * Not `MAX_SIZE_LARGE` — reading that as 7 put a two-digit number at 112px on a 300×160 card in the
 * measurement harness, filling the whole card. Only a browser shows that. */
export const NO_GRID_MAX_SIZE = 4;
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
  if (cardCols == null || totalCols == null || totalCols <= 0) return NO_GRID_MAX_SIZE;
  return (cardCols / totalCols) * (MAX_SIZE_LARGE - MAX_SIZE_SMALL) + MAX_SIZE_SMALL;
}

/* ── Band colours ────────────────────────────────────────────────────────────────────────────
 * A band colour lands on INK, not on a fill, which is a different contrast problem from every
 * other palette in this codebase — and `SERIES_PALETTE` is the wrong tool for it. Measured in a
 * browser on the real card surfaces, all eight series colours fail 4.5:1 as ink on the light card
 * (worst `#F9D45C` at 1.44) and six of the eight fail even the 3:1 large-text bar. They are built
 * to be filled areas with dark text on top.
 *
 * There is also a hard limit worth stating, because it bounds what any palette here can promise:
 * **no single colour can clear 4.5:1 against both themes.** Light needs relative luminance ≤ 0.183
 * and dark needs ≥ 0.274; the ranges do not overlap. Verified against 27 candidates — all 27 fail
 * one side. Since the numeral is bold and renders far above 24px in every real card, the standard
 * that applies is the large-text 3:1, and these eight clear it in BOTH themes with margin.
 *
 * Ratios below are measured (light card `#fff`, dark card `oklch(0.21 0.01 285.93)`), not guessed.
 * Metabase's own segment colours are NOT copied: its sample card 39 uses `#51528D`, which measures
 * 2.48 against egret's dark card and would ship a sub-3:1 headline number.
 */
export const SCALAR_BAND_PALETTE = [
  "#D14343", // red    light 4.57 · dark 3.87
  "#AF7A22", // amber  light 3.73 · dark 4.74
  "#5B8C3A", // green  light 3.99 · dark 4.43
  "#2E86AB", // blue   light 4.11 · dark 4.31
  "#5C6BC0", // indigo light 4.86 · dark 3.64
  "#00838F", // teal   light 4.52 · dark 3.91
  "#AD5A2B", // orange light 4.91 · dark 3.60
  "#8D6E63", // brown  light 4.62 · dark 3.83
] as const;

/** Relative luminance of the two card surfaces, measured in the browser by painting each to a 1×1
 * canvas and reading it back — `getComputedStyle` returns `oklch(...)` for egret's tokens, and a
 * naive rgb regex reads its three numbers as RGB and reports nonsense. */
const LIGHT_CARD_LUMINANCE = 1;
/** Back-solved from two independent browser measurements rather than eyeballed from the token:
 * `#7172AD` measured 3.97 and `#509EE3` measured 6.18 against the dark card, and both give
 * 0.0094. A first attempt guessed 0.0217 and put `#51528D` at 2.05 where the browser says 2.48 —
 * the tests caught it, which is the only reason the editor's warning is not quietly miscalibrated. */
const DARK_CARD_LUMINANCE = 0.0094;
/** The bar that applies to this numeral: bold, and never below 24px in a real card. */
export const LARGE_TEXT_CONTRAST = 3;

function relativeLuminance(hex: string): number | null {
  const m = /^#?([0-9a-f]{6}|[0-9a-f]{3})$/i.exec(hex.trim());
  if (!m) return null;
  const h = m[1].length === 3 ? m[1].split("").map((c) => c + c).join("") : m[1];
  const [r, g, b] = [0, 2, 4].map((i) => {
    const c = parseInt(h.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function ratio(a: number, b: number): number {
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

/** A band colour's contrast against each card surface, or `null` if it is not a hex we can read. */
export function bandContrast(hex: string): { light: number; dark: number; passes: boolean } | null {
  const l = relativeLuminance(hex);
  if (l == null) return null;
  const light = ratio(l, LIGHT_CARD_LUMINANCE);
  const dark = ratio(l, DARK_CARD_LUMINANCE);
  return {
    light: Math.round(light * 100) / 100,
    dark: Math.round(dark * 100) / 100,
    passes: light >= LARGE_TEXT_CONTRAST && dark >= LARGE_TEXT_CONTRAST,
  };
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
