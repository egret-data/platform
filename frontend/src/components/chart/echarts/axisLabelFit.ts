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

import { measureText, type TextMeasurer } from "@/components/chart/measure-text";
import type { EChartsSize } from "./EChartsRenderer";
import { CHART_THEME, resolveFontFamily } from "./palette";

/**
 * WHAT THIS DELIBERATELY DOES NOT CARRY: a `bottomPad`. Metabase's `getXAxisTicksHeight` returns
 * one because Metabase turns `containLabel` OFF and computes its grid by hand, so it has to say
 * how much room the tilted labels need. egret leaves `containLabel: true` on
 * (`CHART_THEME.grid`, `palette.ts:39`), which makes reserving that room ECharts' job, not this
 * module's — and this type shipped a `bottomPad` that three builders dutifully ADDED to
 * `grid.bottom`, reserving the same band twice and crushing the plot into the top of the card.
 * Measured against real ECharts 6.1 (SSR, 193x214 canvas, four categories at 90°): `grid.bottom`
 * of 12 gives a 117px plot with all four labels drawn, and 12 + 72 gives a 45px plot with all
 * four labels still drawn — every added pixel came straight off the plot and bought nothing.
 *
 * So the answer this module returns is a DECISION (draw / turn / hide) and not a layout
 * instruction. The projection the pad was computed from still exists below, because the 70%
 * height guard genuinely needs it — but it stays internal, where it decides something this module
 * owns. If `containLabel` is ever turned off (see `estimatePlotWidth`'s note on why that is its
 * own change), reinstating a pad is two lines; leaving one here for a caller that must not use it
 * is how the double-count comes back.
 */
export type XLabelFit =
  | { kind: "show"; rotate: 0 }
  | { kind: "rotate"; rotate: 45 | 90 }
  | { kind: "hide" };

/** Metabase's own constants. Its names are shortened here, so each one gives the original — the
 *  point is that whoever next diffs this file against upstream can grep for it.
 *
 *  `HORIZONTAL_TICKS_GAP` (its name too) is the minimum ink-free space it demands between two
 *  neighbouring labels; the two factors are the multiples of the font size below which even a
 *  rotated label stops being worth drawing. */
const HORIZONTAL_TICKS_GAP = 6;
/** `X_LABEL_ROTATE_45_THRESHOLD_FACTOR` */
const ROTATE_45_MIN_FACTOR = 2.1;
/** `X_LABEL_ROTATE_90_THRESHOLD_FACTOR` */
const ROTATE_90_MIN_FACTOR = 1.2;
/** `X_LABEL_HEIGHT_RATIO_THRESHOLD`: a rotated label may not eat more than this share of the
 *  chart's height. Past it Metabase hides the axis rather than turning the bottom two-thirds of
 *  the card into a wall of vertical text. */
const MAX_LABEL_HEIGHT_RATIO = 0.7;
/**
 * Formatting and measuring thousands of labels is not free (`X_TICKS_TO_MEASURE_COUNT`).
 *
 * WHAT METABASE DOES, since this is the one place the port is not a copy — `getXTicksToMeasure`
 * (`layout/index.ts:347-373`) branches on the axis TYPE, and an ordinal axis is a category axis:
 *
 *   - numeric / time-series -> 50 at evenly spaced indices (`getEvenlySpacedIndices`);
 *   - category, `dimensionWidth <= fontSize` -> the 50 LONGEST by character count;
 *   - category otherwise -> **every** label, uncapped.
 *
 * So on the axis this module runs on, Metabase caps nothing until the ladder has already fallen
 * off its last rung. It can afford that because it computes its own layout and measures once;
 * egret keeps a bound instead, and takes the union of the two capped strategies — 50 evenly spaced
 * PLUS the longest by character count.
 *
 * Both halves earn their place. Even spacing because character count is only a PROXY for width
 * ("WWWW" outruns "iiiiiiiiiiii"), so a sample that went by length alone could miss the widest
 * label outright. The longest-by-length because the widest label is usually also the longest one,
 * and it is the label that decides the height guard and both per-label overlap
 * conditions — so leaving it to chance is what lets a 40-character category on a 60-category axis
 * sit in a sampling gap and be drawn upright into its neighbour.
 */
const TICKS_TO_MEASURE = 50;

/** Metabase pads each label with a space per side before measuring, so the estimate is pessimistic
 *  by roughly one space at each end — `getPaddedAxisLabel`, `cartesian/option/utils.ts`, whose own
 *  comment is "in some cases we render labels with extra spaces, so we have to be pessimistic". */
const pad = (label: string) => ` ${label} `;

/**
 * The width the categories share, estimated — the `plotWidth` every caller of `fitXLabels` below
 * passes, and the one number the whole ladder turns on.
 *
 * `CHART_THEME.grid` sets `containLabel: true` (`palette.ts`), so ECharts — not the builder —
 * decides the real plot box, and it picks the value-axis ticks that eat into it. Neither is
 * knowable here. The grid insets are ours; the gutter is approximated by formatting the widest
 * datum through the same formatter the axis uses, which is why the caller passes a STRING it has
 * already formatted rather than a number.
 *
 * WHICH WAY IT ERRS. It leans UNDER in the common case, and that is the direction to prefer: too
 * small and we call a collision that would not have happened, rotating labels that would have
 * fitted — mildly ugly, still readable. But it is known to lean OVER in three cases, and an
 * over-estimate is not harmless, so they are named rather than waved at:
 *
 *   1. ECharts extends a value axis PAST the data to a round number. A max of 9,700 gives a top
 *      tick of "10,000" — six characters where five were measured. (This is why there is no claim
 *      here that a tick cannot format wider than the extent: it can, and routinely does.)
 *   2. A dual-axis chart has a SECOND gutter on the right, and only one is subtracted. At 40-50px
 *      against a ~270px estimate that is the most material of the three. Cartesian only — the box
 *      plot and the waterfall each draw a single value axis — and `rightSide` is in scope at that
 *      call site if it is ever fixed.
 *   3. The caller's widest datum is an ABSOLUTE magnitude on all three charts, so a leading minus
 *      sign is not counted.
 *
 * WHAT AN OVER-ESTIMATE COSTS, stated exactly, because it is no longer what it was before this
 * change: once a fit exists the axis is `decided` and `interval` is `0`, so ECharts can no longer
 * drop anything. The failure is now every label drawn upright and OVERLAPPING its neighbour, not
 * labels silently missing. Different look, different diagnosis — that is what to watch for when
 * comparing against Metabase, and if it appears, this estimate is what to tighten.
 *
 * Do not "improve" this by turning `containLabel` off and computing the grid explicitly. That is
 * Metabase's design and it is defensible, but it moves the margins of every cartesian chart in the
 * product and belongs in its own change with its own before/after.
 *
 * SHARED, not copied per builder: cartesian, the box plot and the waterfall lay out the same grid
 * from the same `CHART_THEME.grid` and reserve the same 26px for a value-axis title, so three
 * copies of this arithmetic would be three places for it to drift from the grid it is meant to
 * mirror.
 */
export function estimatePlotWidth(size: EChartsSize, widestValueLabel: string, hasAxisTitle: boolean): number {
  const g = CHART_THEME.grid;
  // The family the chart is actually DRAWN in, never the literal "inherit": a CSS-wide keyword is
  // not a legal family in the `font` shorthand, so the canvas would reject the whole declaration
  // and measure at its 10px sans-serif default. Same rule, and the same reason, as `fitXLabels`.
  const gutter = measureText(widestValueLabel, {
    size: CHART_THEME.axisFontSize, family: resolveFontFamily(), weight: CHART_THEME.axisWeight,
  }).width;
  // `26` is the room each builder's `grid.left` reserves for a value-axis title, kept in step with
  // it — all three add exactly that when their value axis is named.
  return size.width - (g.left as number) - (g.right as number) - gutter - (hasAxisTitle ? 26 : 0);
}

/** At most `n` indices spread evenly over `count`, endpoints included — Metabase's
 *  `getEvenlySpacedIndices` (which builds the same set through a `Set`, so it can return fewer
 *  than `n`; the short case returns early here instead, which is the same answer without the
 *  dedupe). */
function evenIndices(count: number, n: number): number[] {
  if (count <= n) return Array.from({ length: count }, (_, i) => i);
  const step = (count - 1) / (n - 1);
  return Array.from({ length: n }, (_, i) => Math.round(i * step));
}

/** The labels actually measured: `evenIndices`, with the longest by character count folded in at
 *  its own position so the adjacent-pair test still sees them in axis order. See
 *  `TICKS_TO_MEASURE` for why it is the union of the two and not either one. */
function ticksToMeasure(labels: string[]): string[] {
  const indices = evenIndices(labels.length, TICKS_TO_MEASURE);
  const longest = labels.reduce((best, l, i) => (l.length > labels[best].length ? i : best), 0);
  if (!indices.includes(longest)) {
    indices.push(longest);
    indices.sort((a, b) => a - b);
  }
  return indices.map((i) => labels[i]);
}

export function fitXLabels({
  labels,
  plotWidth,
  outerHeight,
  fontSize = CHART_THEME.axisFontSize,
  measure = measureText,
}: {
  labels: string[];
  /** The drawable width the categories share — INNER, after the grid insets and the value-axis
   *  gutter. The caller's estimate leans UNDER in the common case and is the better direction:
   *  erring small rotates a label that would have fitted, which is mildly ugly and still readable.
   *  Erring large leaves labels upright that should have turned — and since a fit makes the axis
   *  `decided` (`interval: 0`), they are then drawn OVERLAPPING rather than dropped. See
   *  `estimatePlotWidth` above for the three cases where it does lean over. */
  plotWidth: number;
  /** The container's height, Metabase's `outerHeight` — deliberately NOT the inner counterpart of
   *  `plotWidth`, which is why it is not called `plotHeight`. The 70% guard is a share of the whole
   *  card (`checkHeight`), so passing an inset height silently tightens it. */
  outerHeight: number;
  fontSize?: number;
  /** Injectable: `measure-text.ts` returns `{width: 0, height: 0}` under jsdom by design ("Size
   *  claims belong in a browser"), so tests drive the ladder with a stub rather than assert a
   *  pixel that is always zero. */
  measure?: TextMeasurer;
}): XLabelFit {
  const upright: XLabelFit = { kind: "show", rotate: 0 };
  if (labels.length === 0 || plotWidth <= 0) return upright;

  // The family the chart is actually DRAWN in (`echarts/base.ts` sets it from the same token), not
  // the literal "inherit": a CSS-wide keyword is not a legal family in the `font` shorthand, so
  // `context.font = "400 13px inherit"` is rejected outright and the canvas keeps whatever font it
  // last had — its 10px sans-serif default on a fresh one. Every width would then come back ~30%
  // short and the fitter would call collisions "fits", which is exactly the failure it is here to
  // stop. Same rule BigNumber follows: "a fit computed against the wrong family is wrong by however
  // much the two differ in advance width."
  const style = { size: fontSize, family: resolveFontFamily(), weight: CHART_THEME.axisWeight };
  const measured = ticksToMeasure(labels);
  // PADDED for the overlap test (`areHorizontalXAxisTicksOverlapping` measures
  // `getPaddedAxisLabel(...)`, `:680`/`:684`) and UNPADDED for the height guard
  // (`getMaxXTickWidth` measures the bare formatted value, `:394-398`). The two spaces exist to
  // make the collision question pessimistic; they have no business inflating a height guard, where
  // they would fire the hide branch ~7px of label early — hiding an axis that fits, over two
  // spaces that are never drawn.
  const widths = measured.map((l) => measure(pad(l), style).width);
  const dimensionWidth = plotWidth / labels.length;

  // Adjacent PAIRS, not the widest label against the average: two wide neighbours collide even on
  // an axis whose mean label is narrow. The three conditions and the constant are Metabase's
  // `areHorizontalXAxisTicksOverlapping` verbatim; what differs is the input — it re-measures the
  // whole dataset here (twice per datum) and this runs on `measured`. The two per-label conditions
  // are unaffected in the case that matters, since the longest label is always in that set. Two
  // differences are left, in OPPOSITE directions, and past 50 labels either can occur:
  //   - a label WIDER than every sampled one while not being the longest by character count, in a
  //     sampling gap -> egret draws upright where Metabase rotates;
  //   - past 50 labels the members of `measured` are no longer each other's neighbours, so the
  //     pairwise condition can fire on a FABRICATED adjacency — two sampled labels of width 10 with
  //     `dimensionWidth` 15 overlap as a pair while neither's real neighbour of width 1 does ->
  //     egret rotates where Metabase draws upright.
  // Both are narrow and contrived, and neither can produce a partial axis, which is the property
  // this module exists for — but they are real differences, not zero.
  const overlaps = widths.some((right, i) => {
    if (i === 0) return false;
    const left = widths[i - 1];
    return left / 2 + right / 2 + HORIZONTAL_TICKS_GAP > dimensionWidth
      || right + HORIZONTAL_TICKS_GAP / 2 > dimensionWidth
      || left + HORIZONTAL_TICKS_GAP / 2 > dimensionWidth;
  });
  if (!overlaps) return upright;

  const maxWidth = Math.max(...measured.map((l) => measure(l, style).width));
  // `checkHeight`: a 45° label's vertical footprint is its width projected onto the vertical axis,
  // i.e. width/√2; a 90° one's is the width itself. This is the ONLY thing the projection is used
  // for now — it decides whether a rotation is worth taking, and no longer also gets handed to a
  // builder as a `grid.bottom` offset (see `XLabelFit`). Which means the √2 is observable only
  // through this guard, and `axisLabelFit.test.ts` brackets it there.
  const projected = (rotate: 45 | 90) => (rotate === 90 ? maxWidth : maxWidth / Math.SQRT2);
  const fitsRotated = (rotate: 45 | 90) => projected(rotate) / outerHeight < MAX_LABEL_HEIGHT_RATIO;

  if (dimensionWidth >= fontSize * ROTATE_45_MIN_FACTOR) {
    return fitsRotated(45) ? { kind: "rotate", rotate: 45 } : { kind: "hide" };
  }
  if (dimensionWidth >= fontSize * ROTATE_90_MIN_FACTOR) {
    return fitsRotated(90) ? { kind: "rotate", rotate: 90 } : { kind: "hide" };
  }
  return { kind: "hide" };
}
