/**
 * Whether a category axis can draw its labels, and how — ported from Metabase's
 * `getAutoAxisEnabledSetting` (`frontend/src/metabase/visualizations/echarts/cartesian/layout/index.ts`).
 *
 * The property that matters is that the answer is never PARTIAL. egret decided rotation from label
 * count and length (`options/cartesian.ts`: `longest > 10 && categories.length > 6 ? 30 : 0`) and
 * left the rest to ECharts' `interval: "auto"`, which silently drops labels that would collide.
 * Four categories on a one-third-width dashboard card came out with ONE label drawn — observed live
 * against Metabase on byte-identical data — and an axis missing three of its four names reads as
 * missing data rather than as a layout decision. Whether labels fit is a question about width, so
 * this module answers it from a width: draw all of them, rotate all of them, or hide the axis.
 */

import { measureText as realMeasureText, type TextMeasurer } from "@/components/chart/measure-text";
import { CHART_THEME, resolveFontFamily } from "./palette";

export type XLabelFit =
  | { kind: "show"; rotate: 0; bottomPad: number }
  | { kind: "rotate"; rotate: 45 | 90; bottomPad: number }
  | { kind: "hide" };

/** Metabase's own constants, kept under its own names so the two files can be diffed by eye.
 *  `HORIZONTAL_TICKS_GAP` is the minimum ink-free space it demands between two neighbouring
 *  labels; the two factors are the multiples of the font size below which even a rotated label
 *  stops being worth drawing. */
const HORIZONTAL_TICKS_GAP = 6;
const ROTATE_45_MIN_FACTOR = 2.1;
const ROTATE_90_MIN_FACTOR = 1.2;
/** `X_LABEL_HEIGHT_RATIO_THRESHOLD`: a rotated label may not eat more than this share of the
 *  chart's height. Past it Metabase hides the axis rather than turning the bottom two-thirds of
 *  the card into a wall of vertical text. */
const MAX_LABEL_HEIGHT_RATIO = 0.7;
/** Formatting and measuring thousands of labels is not free (`X_TICKS_TO_MEASURE_COUNT`). Sampled
 *  at EVENLY SPACED indices rather than as a prefix, because on a sorted axis the widest label is
 *  as likely to be the last as the first — Metabase's `getEvenlySpacedIndices`.
 *
 *  DIVERGENCE: Metabase caps only the *max-width* measurement this way and re-measures the full
 *  dataset (twice per datum) for the overlap test; this reuses the one sampled set for both, so an
 *  axis of >50 categories could in principle hide a lone wide label between two samples. It cannot
 *  bite in practice: past 50 categories `dimensionWidth` is already down near the font size, where
 *  any label at all overlaps and the ladder has left the upright branch regardless. */
const TICKS_TO_MEASURE = 50;

/** Metabase pads each label with a space per side before measuring, so the estimate is pessimistic
 *  by roughly one space at each end — `getPaddedAxisLabel`, `cartesian/option/utils.ts`, whose own
 *  comment is "in some cases we render labels with extra spaces, so we have to be pessimistic". */
const pad = (label: string) => ` ${label} `;

/** `n` items spread across `items`, endpoints always included. Metabase's `getEvenlySpacedIndices`
 *  builds the same set (through a `Set`, so it can return fewer than `n` on short inputs; here the
 *  short case returns early instead, which is the same answer without the dedupe). */
function sample<T>(items: T[], n: number): T[] {
  if (items.length <= n) return items;
  const step = (items.length - 1) / (n - 1);
  return Array.from({ length: n }, (_, i) => items[Math.round(i * step)]);
}

export function fitXLabels({
  labels,
  plotWidth,
  plotHeight,
  fontSize = CHART_THEME.axisFontSize,
  measure = realMeasureText,
}: {
  labels: string[];
  /** The drawable width the categories share. Deliberately an UNDER-estimate at the call site:
   *  erring small rotates a label that would have fitted (mildly ugly, still readable), erring
   *  large leaves it upright and lets ECharts drop it, which is the defect this exists to remove. */
  plotWidth: number;
  /** The chart's outer height — Metabase's `outerHeight`, what the 70% guard is a share of. */
  plotHeight: number;
  fontSize?: number;
  /** Injectable: `measure-text.ts` returns `{width: 0, height: 0}` under jsdom by design ("Size
   *  claims belong in a browser"), so tests drive the ladder with a stub rather than assert a
   *  pixel that is always zero. */
  measure?: TextMeasurer;
}): XLabelFit {
  const upright: XLabelFit = { kind: "show", rotate: 0, bottomPad: 0 };
  if (labels.length === 0 || plotWidth <= 0) return upright;

  // The family the chart is actually DRAWN in (`echarts/base.ts` sets it from the same token), not
  // the literal "inherit": a CSS-wide keyword is not a legal family in the `font` shorthand, so
  // `context.font = "400 13px inherit"` is rejected outright and the canvas keeps whatever font it
  // last had — its 10px sans-serif default on a fresh one. Every width would then come back ~30%
  // short and the fitter would call collisions "fits", which is exactly the failure it is here to
  // stop. Same rule BigNumber follows: "a fit computed against the wrong family is wrong by however
  // much the two differ in advance width."
  const style = { size: fontSize, family: resolveFontFamily(), weight: CHART_THEME.axisWeight };
  const widths = sample(labels, TICKS_TO_MEASURE).map((l) => measure(pad(l), style).width);
  const dimensionWidth = plotWidth / labels.length;

  // Adjacent PAIRS, not the widest label against the average: two wide neighbours collide even on
  // an axis whose mean label is narrow. Conditions and constant are Metabase's
  // `areHorizontalXAxisTicksOverlapping` verbatim.
  const overlaps = widths.some((right, i) => {
    if (i === 0) return false;
    const left = widths[i - 1];
    return left / 2 + right / 2 + HORIZONTAL_TICKS_GAP > dimensionWidth
      || right + HORIZONTAL_TICKS_GAP / 2 > dimensionWidth
      || left + HORIZONTAL_TICKS_GAP / 2 > dimensionWidth;
  });
  if (!overlaps) return upright;

  const maxWidth = Math.max(...widths);
  // `checkHeight`: a 45° label's vertical footprint is its width projected onto the vertical axis,
  // i.e. width/√2; a 90° one's is the width itself.
  const projected = (rotate: 45 | 90) => (rotate === 90 ? maxWidth : maxWidth / Math.SQRT2);
  const fitsRotated = (rotate: 45 | 90) => projected(rotate) / plotHeight < MAX_LABEL_HEIGHT_RATIO;

  // The room the axis needs below the plot, which is that same projection — Metabase's
  // `getXAxisTicksHeight`. Whole pixels because it lands in a grid offset.
  const padFor = (rotate: 45 | 90) => Math.ceil(projected(rotate));

  if (dimensionWidth >= fontSize * ROTATE_45_MIN_FACTOR) {
    return fitsRotated(45) ? { kind: "rotate", rotate: 45, bottomPad: padFor(45) } : { kind: "hide" };
  }
  if (dimensionWidth >= fontSize * ROTATE_90_MIN_FACTOR) {
    return fitsRotated(90) ? { kind: "rotate", rotate: 90, bottomPad: padFor(90) } : { kind: "hide" };
  }
  return { kind: "hide" };
}
