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
 *
 * PR 03 ADDS A FOURTH ANSWER, and it is not a hole in the third. A BUCKETED TEMPORAL axis — and only
 * one of those, see `XLabelBoundary` — may draw every Nth label upright, where the drawn ones are
 * regular period boundaries. That is not the arbitrary subset banned above: "auto" keeps whichever
 * labels happen not to collide, while a stride keeps January and July.
 *
 * The full argument is on the `thin` arm of `XLabelFit` below, and it is the ONE copy. The reader
 * who lands on `interval: 5` in a built option lands in `options/cartesian.ts` instead, so that line
 * carries a one-sentence version and a pointer back here — plus the ECharts semantics the stride
 * depends on, which are load-bearing where they are written and belong nowhere else. Restating the
 * argument in both files is the drift this module refuses a second gap constant to avoid.
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
  /**
   * Every `every`th label, upright, starting at the first — reachable ONLY through the `boundary`
   * input, i.e. only on an axis whose categories are temporal buckets.
   *
   * THIS IS NOT THE `interval: "auto"` THE HEADER ABOVE BANS, and the distinction is the whole
   * justification rather than a caveat on it. "auto" drops whichever labels happen to collide: an
   * arbitrary subset, chosen by geometry, which reads as missing data because nothing about which
   * ones survived means anything. A regular stride over BUCKETS is the opposite — the drawn labels
   * are period boundaries, every one of them the same distance apart, which is what Metabase's tick
   * interval produces (`computeTimeseriesTicksInterval`) and what a reader already expects a date
   * axis to do. Twelve months on a one-third-width card: Metabase draws "January 2026" and
   * "July 2026" and nothing between them, and a reader understands the axis at a glance.
   *
   * `rotate: 0` is part of the decision and not a default. Thinning is tried BEFORE the rotate
   * ladder because two upright labels beat twelve tilted ones; if no stride fits, the ladder below
   * runs unchanged and this arm is never returned.
   */
  | { kind: "thin"; rotate: 0; every: number }
  | { kind: "rotate"; rotate: 45 | 90 }
  | { kind: "hide" };

/**
 * What a BUCKETED temporal axis needs to be thinned, and the only door to `kind: "thin"`.
 *
 * Its one field comes from `temporal-period.ts`, keyed on the truncation unit the chart spec carries
 * (`cfg.xAxisUnit`), and it cannot be manufactured here — this module holds no temporal knowledge
 * at all, which is what makes "an ordinal axis of product names is never thinned" a fact about the
 * types rather than a rule someone has to remember. There is no such thing as every second product.
 *
 * IT CARRIED A SECOND FIELD UNTIL THIS COMMIT: `widestLabel`, the widest name the UNIT could ever
 * produce (`widestPeriodName(unit, drawLabel)`), which `strideThatFits` measured once and charged at
 * every rung. Measuring the labels a rung would ACTUALLY draw replaced it and left nothing for it to
 * decide, so it is gone rather than kept as an unused bound; `strideThatFits` below carries the
 * browser measurement that settled it and what the substitution gives up.
 */
export interface XLabelBoundary {
  /** Candidate strides in BUCKETS, cheapest first — `boundarySteps(unit)`, Metabase's
   *  `TICKS_TIMESERIES_INTERVALS` at and above the data's own unit. Empty for a unit that names no
   *  period, which is now the ONLY thing making a non-temporal stride unreachable from inside this
   *  module — the caller's `namesAPeriod` guard is the other, one level up, and the two are the same
   *  membership rule asked twice. */
  steps: number[];
}

/** Metabase's own constants. Its names are shortened here, so each one gives the original — the
 *  point is that whoever next diffs this file against upstream can grep for it.
 *
 *  `HORIZONTAL_TICKS_GAP` (its name too) is the minimum ink-free space it demands between two
 *  neighbouring labels; the two factors are the multiples of the font size below which even a
 *  rotated label stops being worth drawing.
 *
 *  EXPORTED for `options/gauge.ts`, which asks the same question about labels that are not on a
 *  cartesian axis at all — a gauge's graduations sit on an ARC, so the pair it separates can be
 *  apart in both dimensions rather than only in x. The quantity is unchanged (how much clear ink two
 *  neighbouring pieces of axis text need between them), and this module already argues that two
 *  constants for one question is how they drift; the gauge staying on this one is that argument
 *  applied across a file boundary rather than an exception to it. Nothing else about this module is
 *  reachable from there, so the export is the constant and not the ladder. */
export const HORIZONTAL_TICKS_GAP = 6;
/** ECharts' own default `axisLabel.margin`: the clear space between an axis line and its labels.
 *  Not a constant of this module's invention — it is read here because a LEFT axis's name has to be
 *  placed past labels that sit that far out (`nameGapClearOf`), and because `nameGap` is measured
 *  from the axis line rather than from the labels. */
const AXIS_LABEL_MARGIN = 8;
/** ECharts' own default `splitNumber` on a value axis. It is a HINT — the axis still rounds to a
 *  human interval — so this is a ceiling to stay under and never a count to assert. Above it there
 *  is nothing to say, which is why `intervalThatFits` answers `undefined` there rather than
 *  repeating the default back into the option. */
const DEFAULT_VALUE_SPLITS = 5;
/**
 * The plot ECharts insists on keeping, near enough — the threshold `nameGapClearOf` decides against.
 *
 * MEASURED, not documented anywhere upstream: rendered at 166px with a category label of 108.7px,
 * the un-clamped left inset (`grid.left` + label + margin) comes to 150.7 against a card whose right
 * inset starts at 150, and ECharts answers by putting the axis at 134.5 and letting the LABELS spill
 * left of `grid.left` instead. That spill is the whole problem: every gap this module computes is
 * relative to where the labels ended up, so once they move left the name follows them off the card.
 *
 * IT IS A SAFETY MARGIN AND NOT A CLIFF, which is worth saying because 24 looks like a measured
 * threshold and is not one. Setting it to 0 — declining only where the un-clamped inset would
 * overrun the card outright — still keeps the title on the canvas at every size rendered; what it
 * removes is the room to spare, leaving cards whose title clears its labels by 3px. 24 buys the
 * decision back before it gets that close. So `rowAxisLabels.test.ts` brackets the DECISION (a
 * 108.7px label falls back at 166px and clears at 250px) rather than this number, and a change to
 * it should be justified by rendering a sweep, as this one was.
 */
const MIN_PLOT_WIDTH = 24;
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
 * The tick a value axis draws WIDEST, approximated — the string the gutter below is measured from.
 *
 * It is the one place this module still SAMPLES rather than measures what is drawn, and it has to
 * be: `containLabel` leaves the value axis's ticks to ECharts, so unlike the category labels (which
 * the caller hands over in full, and which `strideThatFits` now slices per rung) there is no list of
 * them to read.
 *
 * WHAT IT REPLACED, because the replacement only makes sense against it: the caller used to hand
 * over its widest DATUM formatted. A value axis does not draw data, it draws ROUND TICKS, and under
 * compact formatting the round one is routinely the SHORTER string — measured on the ZZ Chart Parity
 * line card, revenue peaking at 24,730 was charged `"24.7K"` at 34.6px while ECharts settled on
 * 0/5K/10K/15K/20K/"25K" and drew 24.2px. Ten pixels of a ~119px plot, reserved and never used. The
 * waterfall beside it was charged "205K" and drew "250K" — both 32.3px — so it paid nothing, which
 * is how one dataset produced two different gutters on one card.
 *
 * (An earlier version of this paragraph closed by saying those ten pixels were what stood between
 * the line chart and its "January 2026". They were not — they were one of two causes, and the
 * smaller one. The other was `strideThatFits` measuring "September 2026" for a rung that draws
 * "January 2026"; see it for the browser numbers. Recovering the gutter is still worth what it is
 * worth, which is ten pixels of plot, and no longer carries a claim about an outcome it did not
 * decide on its own.)
 *
 * IT DOES NOT PREDICT `intervalScale`, and must not start. Which ticks ECharts picks is its own
 * `nice()` over an interval drawn from {1,2,5}x10^n, and a builder that reimplemented it would be a
 * second copy to keep in step with an upgrade. What it does instead is SAMPLE a handful of values
 * the axis plausibly draws and take the widest, so the answer is a bound rather than a guess — the
 * same shape as Metabase's `getRepresentativeDates` for a date axis. The samples are the
 * fifths of a one-significant-figure ceiling — ECharts' default `splitNumber` is 5, and the ceiling
 * is at or above whatever `nice()` settles on — computed as `ceiling * n / 5` in integers so no
 * rung arrives as 18000.000000000004.
 *
 * WHY FIFTHS AND NOT MORE SAMPLES. Every extra rung can only widen the answer, and rungs ECharts
 * would never draw widen it wrongly: quarters of 30,000 include 7,500, which formats "7.5K" and
 * would re-introduce the phantom character this exists to remove — no interval in {1,2,5}x10^n
 * produces it. The fifths, by contrast, catch the case a bare ceiling misses, which is the FORMATTER
 * changing magnitude part way up the axis: a ceiling of 2,000,000 formats "2M" at two characters
 * while 0.8 and 0.2 of it give "1.6M" and "400K" at four, and four is what ECharts draws (500K, 1M,
 * 1.5M, 2M). Character count is a PROXY for width and not the same thing ("WWW" outruns "iiiiii"),
 * which is tolerable here because it only chooses BETWEEN candidates — the caller measures the
 * winner with the real measurer. It is Metabase's own proxy at the same spot (`timeseries.ts:293`).
 *
 * WHERE IT IS STILL WRONG, both directions named because "closer" is not "right":
 *
 *   - a user-set `yTickCount` becomes ECharts' `splitNumber`, so a 4-split axis lands on quarters
 *     this sample does not hold. Ticks stay round, so the miss is at most the digit that quartering
 *     adds — and the sample is not widened to cover it, for the reason above.
 *   - `yMin`/`yMax` extend the axis past the data, and this samples from the DATA's ceiling, so an
 *     explicit max far above the values is charged too little.
 *   - a LOG axis draws powers of ten, not fifths. The sample still bounds it in practice (a decade
 *     below the ceiling is never the wider string) but it bounds it by luck rather than by argument.
 *
 * All three under-charge, which is the direction that leaves x labels overlapping rather than the
 * one that hides them — see `estimatePlotWidth` below, which states both costs. That is the trade
 * this change deliberately takes: it removes an over-charge that hid whole axes and accepts a
 * smaller residual under-charge that at worst crowds them.
 */
export function widestTickLabel(widestValue: number, format: (v: number) => string): string {
  if (!Number.isFinite(widestValue) || widestValue <= 0) return format(0);
  // One significant figure, rounded UP: 24,730 -> 30,000, 9,700 -> 10,000, 205,256 -> 300,000. At or
  // above ECharts' own nice maximum in every case this samples, which is what makes the sample a
  // bound. `Math.log10` is not exact at every power of ten (`log10(1000)` is 2.9999999999999996), so
  // an off-by-one magnitude is possible — `ceil(v / magnitude) * magnitude` still lands on a value
  // at or above `v`, which is all this needs.
  const magnitude = 10 ** Math.floor(Math.log10(widestValue));
  const ceiling = Math.ceil(widestValue / magnitude) * magnitude;
  let widest = "";
  for (let n = 1; n <= 5; n++) {
    const label = format((ceiling * n) / 5);
    if (label.length > widest.length) widest = label;
  }
  return widest;
}

/** The one font every axis label and axis name in this module is measured at — the chart's own,
 *  RESOLVED rather than the literal token: a CSS-wide keyword like "inherit" is not a legal family
 *  in the `font` shorthand, so the canvas rejects the whole declaration and measures at its 10px
 *  sans-serif default, ~30% short, and a fit computed against the wrong family is wrong by however
 *  much the two differ in advance width. Hoisted so the four measurements below cannot drift apart
 *  on it. */
const axisFont = (fontSize: number = CHART_THEME.axisFontSize) =>
  ({ size: fontSize, family: resolveFontFamily(), weight: CHART_THEME.axisWeight });

/**
 * The width the categories share, estimated — the `plotWidth` every caller of `fitXLabels` below
 * passes, and the one number the whole ladder turns on.
 *
 * `CHART_THEME.grid` sets `containLabel: true` (`palette.ts`), so ECharts — not the builder —
 * decides the real plot box, and it picks the value-axis ticks that eat into it. Neither is
 * knowable here. The grid insets are ours; the gutter is approximated by `widestTickLabel` above,
 * through the same formatter the axis draws its own ticks with — which is why the caller passes the
 * widest VALUE and that formatter, rather than a string it has already made. It handed a string
 * until PR 03, and what it handed was the widest datum; see `widestTickLabel` for what that cost.
 *
 * WHICH WAY IT ERRS. It leans UNDER in the common case, and that is the direction to prefer: too
 * small and we call a collision that would not have happened, rotating labels that would have
 * fitted — mildly ugly, still readable. But it is known to lean OVER in three cases, and an
 * over-estimate is not harmless, so they are named rather than waved at:
 *
 *   1. A dual-axis chart has a SECOND gutter on the right, and only one is subtracted. At 40-50px
 *      against a ~270px estimate that is the most material of the three. Cartesian only — the box
 *      plot and the waterfall each draw a single value axis — and `rightSide` is in scope at that
 *      call site if it is ever fixed.
 *   2. The caller's widest value is an ABSOLUTE magnitude on all three charts, so a leading minus
 *      sign is not counted.
 *   3. The three cases `widestTickLabel` names, where the sampled tick can come out narrower than
 *      the drawn one.
 *
 * (A fourth used to head this list — "ECharts extends a value axis past the data to a round number,
 * so a max of 9,700 draws a tick of 10,000" — and it is gone because `widestTickLabel` now samples
 * from that round ceiling rather than from the datum. It was half of a symmetry whose other half
 * was over-charging by ten pixels on every compact axis.)
 *
 * WHAT AN OVER-ESTIMATE COSTS, and it is NOT one thing — PR 03 split this in two, so read the arm
 * that matches the axis in front of you. Neither is "labels silently missing": once a fit exists the
 * axis is `decided`, and a decided axis never leaves ECharts free to drop anything.
 *
 *   - ORDINAL axis, unchanged since PR 02: `interval` is `0`, so the failure is every label drawn
 *     upright and OVERLAPPING its neighbour.
 *   - BUCKETED TEMPORAL axis, new in PR 03: the axis may be thinned, and an over-estimate buys a
 *     stride one rung too CHEAP — every 2nd month where every 3rd was needed. The drawn labels then
 *     overlap too, but there are fewer of them, so it looks like a correctly thinned axis whose
 *     names happen to touch rather than like a crowded one. Different diagnosis, and the fix is the
 *     same: tighten this estimate, not the stride ladder.
 *
 * Either way, overlapping labels in the live comparison point HERE and not at `fitXLabels`.
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
export function estimatePlotWidth(
  size: EChartsSize,
  /** The largest magnitude the value axis has to accommodate. A NUMBER, not a formatted label:
   *  handing over a string is what let three builders charge the gutter for a datum the axis never
   *  draws, and taking the value plus the formatter makes that unrepresentable rather than merely
   *  discouraged. */
  widestValue: number,
  /** The axis's OWN tick formatter, so the candidate is a string this axis would really render —
   *  the same rule the category side keeps by measuring the labels the caller passes in. */
  format: (v: number) => string,
  hasAxisTitle: boolean,
): number {
  return plotWidthLessGutter(size, measureText(widestTickLabel(widestValue, format), axisFont()).width, hasAxisTitle);
}

/**
 * `estimatePlotWidth`'s arithmetic, with the gutter already measured — the ONE copy, and the reason
 * it is separated out rather than inlined above.
 *
 * A ROW CHART needs the same subtraction against a different gutter. Its left-hand axis is the
 * CATEGORY axis, so what eats into the plot on that side is the category names, not a column of
 * value ticks — and `estimatePlotWidth`'s signature deliberately admits no string, because handing
 * it one is what let three builders charge the gutter for a datum their axis never drew. Rather
 * than loosen that (or copy the four terms into `options/cartesian.ts`), the row chart measures its
 * own gutter with `widestLabelWidth` below — the labels ARE the drawn strings there, exactly as
 * `fitXLabels` takes them — and hands the width here.
 *
 * `26` is the room each builder's `grid.left` reserves for an axis title, kept in step with it: all
 * three add exactly that when the axis on that edge is named.
 */
export function plotWidthLessGutter(size: EChartsSize, gutter: number, hasAxisTitle: boolean): number {
  const g = CHART_THEME.grid;
  return size.width - (g.left as number) - (g.right as number) - gutter - (hasAxisTitle ? 26 : 0);
}

/** The widest of the labels an axis DRAWS, measured. The category-side counterpart of
 *  `widestTickLabel`, and unlike it nothing here is INVENTED — a category axis is handed its labels
 *  in full, so this measures the strings themselves rather than sampling values the axis might
 *  round to. `ticksToMeasure` bounds the cost the same way the overlap test above does, and with the
 *  same residual: past 50 labels the widest one can hide in a sampling gap, since character count is
 *  only a proxy for width. */
export function widestLabelWidth(
  labels: string[],
  measure: TextMeasurer = measureText,
  fontSize: number = CHART_THEME.axisFontSize,
): number {
  let widest = 0;
  for (const l of ticksToMeasure(labels)) {
    const w = measure(l, axisFont(fontSize)).width;
    if (w > widest) widest = w;
  }
  return widest;
}

/** {1,2,5}x10^n at or above `v` — ECharts' own family of tick intervals (`intervalScale`'s
 *  `nice()` draws from it), so an interval chosen here is one the axis would have been willing to
 *  choose for itself and its labels stay round. */
function niceInterval(v: number): number {
  const magnitude = 10 ** Math.floor(Math.log10(v));
  for (const m of [1, 2, 5]) if (m * magnitude >= v) return m * magnitude;
  return 10 * magnitude;
}

/**
 * The tick INTERVAL a value axis needs for its labels to clear each other across `plotWidth`, or
 * `undefined` when ECharts' own default is already loose enough — the value-axis answer to the
 * question `fitXLabels` answers for categories.
 *
 * IT IS A DIFFERENT ANSWER, and that is the whole point rather than an implementation detail. A
 * category axis that cannot fit its labels rotates them, because the labels are the data and
 * dropping any of them reads as missing data. A value axis draws ROUND TICKS that it generated
 * itself, so the same crowding has a remedy the other axis does not have: draw fewer of them. Ten
 * thousand and twenty thousand say everything nought-to-forty-thousand says. Metabase agrees from
 * both directions — it greys axis-label rotation out on a value axis, and its own row chart at
 * 311px answers this crowding by drawing a single tick — so handing a value axis to the rotate
 * ladder would be a divergence dressed as a fix.
 *
 * AN INTERVAL AND NOT A `splitNumber`, and this is the one part of this function that was MEASURED
 * rather than reasoned. `splitNumber` is a HINT: ECharts divides the extent by it and then rounds
 * the result to a nice number, and it rounds DOWN as readily as up — so a count that came from a
 * pixel budget does not survive the round trip. The first version of this fix asked a 250px row
 * card for 3 splits over a 412,000 extent and got an interval of 100,000 back: five labels, 22px
 * apart, still overlapping by 8.6px. Rendered across eighteen widths from 166 to 800, that version
 * left the labels overlapping at 220, 260 and 280 while an interval cleared every one of them.
 *
 * WHAT THAT DOES AND DOES NOT SAY, because the distinction is easy to lose and a later reader will
 * be tempted to "simplify" this back: it is not that `splitNumber` cannot express the answer. A
 * split count DERIVED from the interval below — `round(span / interval)` — round-trips through
 * ECharts' rounding to this same interval, and mutating the option that way passes every case in
 * `rowAxisLabels.test.ts`. The two encodings agree; what failed was deriving the COUNT from the
 * pitch and hoping the interval ECharts inferred from it would match. Naming the interval removes
 * the inference, and is the shorter way to say it. (`axisTick` follows the labels for free — it
 * carries no interval of its own on a value axis.)
 *
 * THE BUDGET IS A PITCH, not a packing. Labels are centred on their ticks and evenly spaced, so
 * neighbours sit `plotWidth / steps` apart whatever the count, and the axis fits when that pitch
 * clears the widest label plus one gap of ink. `HORIZONTAL_TICKS_GAP` is that gap, the same 6px
 * `fitXLabels` demands between two neighbouring category labels — one question, one constant.
 *
 * ONE LABEL'S WIDTH IS CHARGED BEFORE THE DIVISION, for the two OUTERMOST labels: each overhangs
 * its end of the plot by half its width, and in echarts 6 the grid shrinks to keep them on the
 * card, so the pitch is shared over a plot narrower than the estimate. Measured on the live card's
 * data at 250px: `plotWidthLessGutter` estimates 139 and the drawn plot is 110 — 29px, against a
 * label of 30.6. Charging it makes the estimate's residual error the small term rather than the
 * one that decides the answer.
 *
 * WHY A CEILING AND NOT A COUNT. Returning `undefined` at or above `DEFAULT_VALUE_SPLITS` leaves a
 * roomy card's option byte-identical to what it built before this existed — the fit may only ever
 * take ticks AWAY. A chart wide enough for fifteen labels does not want fifteen, and ECharts'
 * default already draws six there with 21px between them.
 *
 * WHY IT DECLINES ON A ZERO MEASUREMENT rather than treating it as "everything fits". `measureText`
 * returns 0 under jsdom by design ("Size claims belong in a browser"), and a zero-width label makes
 * any pitch clear any budget — so an unmeasured environment would silently be told the default is
 * fine. That happens to be the answer it wants, but by accident; saying so explicitly keeps this
 * function's contract "decide from a width, or decline", which is the builder's rule for `size`.
 *
 * WHAT IT DOES NOT DO IS SET THE AXIS'S MAX, and the residual is real rather than theoretical. An
 * interval that does not divide the extent ECharts chose leaves a short last step — 200,000 against
 * a 500,000 max draws 0/200K/400K/500K, and those last two sit half a pitch apart. ECharts drops
 * that label itself where it would collide (`fixMinMaxLabelShow`), which is why the rendered gap
 * stays positive at every width measured; at 300px it comes out at 1.4px, which is exactly what the
 * axis drew there BEFORE this fit existed. Taking the max over as well would even the pitch out, and
 * would also move every narrow row chart's bar scale — a visible change to charts that do not have
 * this defect, so it belongs in its own commit if the 1.4px ever reads as a defect of its own.
 */
export function intervalThatFits({
  plotWidth,
  span,
  widestValue,
  format,
  fontSize = CHART_THEME.axisFontSize,
  measure = measureText,
}: {
  /** The drawable width the ticks share, from `plotWidthLessGutter`. */
  plotWidth: number;
  /** The distance the axis covers — what the interval has to divide. NOT the same as
   *  `widestValue`: an axis running -400 to +400 is 800 across, and sizing its interval against 400
   *  would ask for twice the ticks that fit. */
  span: number;
  /** The largest magnitude the axis carries, and its own tick formatter — the same pair
   *  `estimatePlotWidth` takes, and for the same reason: the candidate has to be a string this axis
   *  would really draw. `widestTickLabel` turns them into one. */
  widestValue: number;
  format: (v: number) => string;
  fontSize?: number;
  measure?: TextMeasurer;
}): number | undefined {
  // Written as `!(x > 0)` rather than `x <= 0` so a NaN is DECLINED rather than carried: `<=` is
  // false for NaN, and this function is exported — the waterfall and the box plot are its obvious
  // next callers, and a NaN plot width returning a NaN interval would reach an option silently.
  if (!(plotWidth > 0) || !(span > 0) || !Number.isFinite(span)) return undefined;
  const width = measure(widestTickLabel(widestValue, format), axisFont(fontSize)).width;
  if (!(width > 0)) return undefined;
  const steps = Math.floor((plotWidth - width) / (width + HORIZONTAL_TICKS_GAP));
  if (steps >= DEFAULT_VALUE_SPLITS) return undefined;
  // One step is ECharts' floor as well as ours: a value axis draws its two ends. Where even those
  // two collide the honest remedy is a wider card.
  return niceInterval(span / Math.max(1, steps));
}

/**
 * The vertical band a value label OCCUPIES, as a multiple of the font size — the height counterpart
 * of the width `intervalThatFits` measures per label, and the one term that could not be carried
 * across from it.
 *
 * IT IS NOT MEASURED PER LABEL, AND THAT IS THE POINT. `intervalThatFits` has to ask `measureText`
 * how wide `"400K"` comes out, because the answer depends on the digits; stacked labels are all
 * exactly as tall as each other whatever they say, so the vertical fit needs no measurer for its
 * pitch at all and cannot be defeated by a jsdom that reports zero. (It still declines under jsdom,
 * for a different reason — the PLOT HEIGHT below is measured. See `plotHeightLessLabels`.)
 *
 * 15/13 IS A MEASUREMENT AND NOT A GUESS AT A LINE HEIGHT, and the distinction matters because
 * ECharts' own answer is a different number. Read off the live 166x184 dashboard card, Chrome, the
 * app's `--font-sans` at `400 13px`: every axis label's box came out 15px tall — `100K` and `20`
 * alike, digits having neither descenders nor accents to vary it. zrender lays the same axis out
 * against `getLineHeight(font)`, which is the width of `国` and comes to exactly the font size
 * (13px, confirmed in SSR). So ECharts thinks its labels are 13 tall while the browser draws 15,
 * and a fit charged 13 would leave two boxes touching by 2px while satisfying its own arithmetic.
 * What is drawn is what a reader sees, so what is drawn is what is charged.
 *
 * NO `HORIZONTAL_TICKS_GAP` IS ADDED ON TOP OF IT, which is the one place this fit declines to
 * follow the horizontal one, and it is not an oversight. That constant is the ink-free space two
 * labels running SIDE BY SIDE need, where the box being measured is the glyphs' own advance width
 * and there is no slack in it — `getPaddedAxisLabel` even adds a space at each end to be
 * pessimistic. A LINE BOX already contains its leading: 15px of box around digits whose ink is
 * ~9.4px tall leaves 5.6px of clear space between two boxes that merely touch. Charging 6 more
 * would be billing the same clearance twice, and it is the difference between leaving the harness's
 * combo card alone (its ticks sit 18.3px apart) and thinning an axis nobody reported. The criterion
 * the live audit applied was that no two label BOXES intersect, and that is the criterion here.
 */
const VALUE_LABEL_BOX_RATIO = 15 / 13;

/**
 * The vertical band the CATEGORY labels take out of the card below the plot, given the rotation the
 * ladder above settled on — the term that makes a vertical chart's plot SHORT, and the reason its
 * value axis crowds at a size where a row chart's does not.
 *
 * WHY THE BUILDER CANNOT JUST SUBTRACT ITS GRID INSETS, which is what `plotWidthLessGutter` does on
 * the other axis: `grid.bottom` is 12, and the plot on the live bar card ends 105.8px above the
 * bottom of a 184px SVG. The missing 72px is this — four 90°-rotated product names, each as tall as
 * its own width, reserved by ECharts' `outerBounds` from a grid inset that says nothing about them.
 * A vertical fit computed against `size.height` less the insets would be told it had 138px to spend
 * where it had 56.
 *
 * EACH ARM MEASURED, against real ECharts 6.1 in SSR, at two font sizes so a term that is really a
 * constant cannot masquerade as a multiple of one. The quantity read out is the room ECharts left
 * below the plot in excess of `grid.bottom`, less `AXIS_LABEL_MARGIN`; the widest of the four
 * category names measures 60.84px at 13px and 93.60px at 20px:
 *
 *              measured 13px   this returns   measured 20px   this returns
 *     0°        13.00           13.00          20.00           20.00
 *     45°       49.74           52.21          75.38           80.33
 *     90°       63.84           67.34          96.60           103.60
 *
 * SO TWO OF THE THREE ARE BOUNDS RATHER THAN EQUALITIES, deliberately, and they lean the safe way:
 * over-stating the band under-states the plot, which spends fewer ticks than strictly necessary —
 * the failure is a sparse axis, not an overlapping one. The 0° arm is exact because there is
 * nothing to project: an upright label's band is its line, and its line is the font size.
 *
 * 90° IS THE WIDTH PLUS HALF A LINE, and the half line is slack rather than geometry. A label
 * turned onto the vertical runs exactly its own width down the card, so the width is the whole of
 * the projection — but ECharts reserved 3.00px more than that at BOTH font sizes, out of the single
 * estimate pass `layOutGridByOuterBounds` makes (its own comment concedes the rect "might be
 * slightly smaller than the ideally expected result"). Charging a measured 3 would pin an
 * implementation detail of one ECharts version; charging half a line covers it at any font size and
 * says why it is there.
 *
 * 45° IS THE ROTATED BOX'S PROJECTION, `(w·sin45 + h·cos45)` with the label's height taken as the
 * font size — the same √2 the height guard in `fitXLabels` projects with, applied to both sides of
 * the box rather than to the width alone. `maxWidth / √2` alone is NOT a bound here: it comes to
 * 43.02 against a measured 49.74.
 */
export function categoryLabelBand({
  rotate,
  /** The widest label the axis DRAWS, measured — `widestLabelWidth`. Unused at 0°, where the band
   *  is the font's own line and no string can change it. */
  widestLabel,
  fontSize = CHART_THEME.axisFontSize,
}: { rotate: 0 | 45 | 90; widestLabel: number; fontSize?: number }): number {
  if (rotate === 90) return widestLabel + fontSize / 2;
  if (rotate === 45) return (widestLabel + fontSize) / Math.SQRT2;
  return fontSize;
}

/**
 * The height the value ticks share — `plotWidthLessGutter`'s counterpart, and NOT its transposition.
 *
 * Three of the four terms have no analogue on the other axis:
 *
 *   - `categoryLabelBand`, above, which is the whole reason this cannot be four insets.
 *   - `AXIS_LABEL_MARGIN`, because ECharts holds the category labels that far clear of the axis
 *     line and the band is measured from where they START.
 *   - HALF A LINE AT THE TOP. The topmost value label is centred on its tick and would overhang the
 *     top of the card, so ECharts pushes the plot down by half its height — measured at exactly
 *     `fontSize / 2` (6.50 at 13px, 10.00 at 20px, constant across every card height rendered).
 *     This is the vertical trace of the same effect `intervalThatFits` charges a whole label width
 *     for, and it is HALF of one rather than one because only the top end moves: the bottom label
 *     sits on the axis line and overhangs into the band the category names already own.
 *
 * WHICH WAY IT ERRS, and it is the same direction the width estimate takes for the same reason:
 * UNDER. Rendered at 166x184 with four 90° category names it answers 52.2 against a drawn plot of
 * 55.7, the 3.5px being `categoryLabelBand`'s deliberate over-charge. Erring small spends fewer
 * ticks than the card could carry; erring large leaves two of them touching, which is the defect.
 *
 * THE GRID INSETS ARE PASSED IN RATHER THAN READ FROM `CHART_THEME`, unlike `plotWidthLessGutter`'s
 * hard-coded 26. Each builder adds its own room on these two edges — 24 at the top for a legend, 22
 * at the bottom for a title — and both are conditional, so a copy of that arithmetic here would be
 * a second place for it to drift from the `grid` the builder actually emits. Callers pass the very
 * object they are about to hand ECharts.
 */
export function plotHeightLessLabels({
  size,
  band,
  grid,
  fontSize = CHART_THEME.axisFontSize,
}: {
  size: EChartsSize;
  /** `categoryLabelBand` for the rotation this chart settled on, or 0 where the ladder hid the
   *  axis and nothing is drawn below it. */
  band: number;
  /** The builder's OWN resolved grid insets, not `CHART_THEME.grid`'s. */
  grid: { top: number; bottom: number };
  fontSize?: number;
}): number {
  return size.height - grid.top - grid.bottom - AXIS_LABEL_MARGIN - band - fontSize / 2;
}

/**
 * The whole scale a VERTICAL value axis needs for its labels to clear each other down `plotHeight`
 * — the interval AND the two ends it divides. `intervalThatFits` asked about the other axis, and
 * the answer is a different quantity rather than the same one rotated.
 *
 * WHAT DOES CARRY OVER, unchanged and deliberately shared: the remedy (fewer ticks, never a
 * rotation — see `intervalThatFits` for why a value axis is not handed the category ladder), the
 * `{1,2,5}×10ⁿ` lattice, and the ceiling at `DEFAULT_VALUE_SPLITS` that leaves a roomy card's
 * option byte-identical.
 *
 * WHAT DOES NOT, and each was checked rather than assumed:
 *
 *   1. NO LABEL IS MEASURED. `intervalThatFits` divides by `width + gap` and has to know which
 *      string the axis draws; a stacked label's height is its font's, so `VALUE_LABEL_BOX_RATIO`
 *      answers it outright. There is no `widestValue`, no `format`, and no zero-measurement arm —
 *      the caller's plot height is the only measured input, and it declines on that. (Nor any
 *      `HORIZONTAL_TICKS_GAP`; the constant's own note says why a line box does not want one.)
 *   2. NO WHOLE LABEL IS CHARGED BEFORE THE DIVISION. The horizontal fit subtracts one full label
 *      width for the two outermost overhangs, because echarts 6 shrinks the grid sideways to keep
 *      them on the card — measured at 250px as 29px of a 139px estimate. Vertically it does not:
 *      the plot rect came out at a CONSTANT y of 22.5 and a height of `cardHeight − 128.34` across
 *      184/220/260/300/400px cards, three chart families and every interval tried, and the drawn
 *      pitch was `plotHeight / steps` to the third decimal in all of them. The top overhang is a
 *      fixed half line and belongs to `plotHeightLessLabels`; the bottom one spills into the
 *      category names' own band. Charging a label here would have thinned every card by one rung
 *      for a shrink that does not happen.
 *   3. IT NAMES THE ENDS AS WELL AS THE INTERVAL, which is the only thing this returns that the
 *      horizontal fit does not, and it is a defect of the horizontal one rather than a luxury of
 *      this one. `intervalThatFits`'s own note records the residual: an interval that does not
 *      divide the extent ECharts chose leaves a SHORT last step — 200K against a nice 500K max
 *      draws 0/200K/400K/500K, and the last two sit half a pitch apart. It relies on ECharts
 *      dropping that label itself, and ECharts does — but by ITS 13px reckoning of a label's
 *      height, not the 15px one a browser draws. Measured on a 166x200 bar card, the last two came
 *      out 14.33px apart: comfortable to ECharts, and a 0.67px overlap on screen. Rounding the two
 *      ends OUT to multiples of the interval removes the short step by construction rather than by
 *      relying on someone else's tie-break, and it cannot clip data — `floor` on the low end and
 *      `ceil` on the high one only ever widen. `axisHelper.niceScaleExtent` is why ECharts will not
 *      do it for us: it rounds the extent for the interval it DERIVED, then overwrites the interval
 *      with the option's, leaving the extent behind.
 *
 * THE PITCH IS THE WHOLE TEST, then: labels are centred on their ticks and evenly spaced, so
 * neighbours sit `plotHeight / steps` apart, and the axis fits when that clears one label box —
 * 15px at the 13px this family draws at. The live card's crowded axes were at 10.7 (bar and box
 * plot, four labels each overlapping the next); the four that read cleanly were at 21 and 18.3.
 */
export function verticalValueAxisFit({
  plotHeight,
  lo,
  hi,
  fontSize = CHART_THEME.axisFontSize,
}: {
  /** The drawable height the ticks share, from `plotHeightLessLabels`. */
  plotHeight: number;
  /** The two ends the axis has to hold — the DISTANCE between them is what an interval divides, so
   *  an axis running -400 to +400 is 800 across and not 400. Both are widened to the round numbers
   *  returned, so a caller hands over the extent its data really needs and gets back one the ticks
   *  land on. */
  lo: number;
  hi: number;
  fontSize?: number;
}): { interval: number; min: number; max: number } | undefined {
  const span = hi - lo;
  // `!(x > 0)` rather than `x <= 0` so a NaN is DECLINED rather than carried into an option — the
  // same guard `intervalThatFits` states, and for the same reason: three builders call this.
  if (!(plotHeight > 0) || !(span > 0) || !Number.isFinite(span)) return undefined;
  const steps = Math.floor(plotHeight / (fontSize * VALUE_LABEL_BOX_RATIO));
  if (steps >= DEFAULT_VALUE_SPLITS) return undefined;
  // One step is ECharts' floor as well as ours: a value axis draws its two ends.
  const interval = niceInterval(span / Math.max(1, steps));
  return {
    interval,
    min: Math.floor(lo / interval) * interval,
    max: Math.ceil(hi / interval) * interval,
  };
}

/**
 * The `nameGap` a LEFT axis's name needs to clear its own labels, given how wide they measure.
 *
 * `nameGap` is measured from the AXIS LINE, and on a left axis the labels are what sits between the
 * two — so a gap that does not cover them draws the name across them. That is the live defect: the
 * row chart's `category` title occupied x 64.8-77.8 while `Doohickey` ran 37-97.8. The three terms
 * are the axis line to the labels (`AXIS_LABEL_MARGIN`), the labels themselves, and the same ink-free
 * `HORIZONTAL_TICKS_GAP` this module demands between any two neighbouring pieces of axis text.
 *
 * IT DOES NOT NEED THE NAME'S OWN THICKNESS AS A TERM, which is what a reader will look for. A name
 * on a left axis is rotated onto the y axis, so what it occupies horizontally is its font's HEIGHT,
 * and ECharts anchors it at `nameGap` and grows it away from the plot. Where the labels sit where
 * this module expects, that thickness lands inside the 26px `grid.left` reserves for a title: at the
 * live card's size the name's ink came out at x 18-31 against labels starting at 37 — inside the
 * band with 10px to spare, and 6px of clear ink where there was -40.8.
 *
 * …WHICH IS WHY THE ANSWER CAN BE `undefined`, AND THE CALLER MUST HAVE A FALLBACK. Substitute the
 * identity above and the name's ink is `[labelsLeft - 19, labelsLeft - 6]` — always 19px to the left
 * of wherever the labels BEGAN. That is fine while they begin at `grid.left`, and it is off the card
 * the moment they do not: past a certain label width ECharts stops honouring the left inset and
 * spills the labels leftwards instead (`MIN_PLOT_WIDTH`). Rendered at 166px, a category of 108.7px
 * put the title at x -1 and one of 129px at x -6, and a 223px one at -100. Before this function
 * existed the title was drawn at a flat 28 and sat ON its labels — the defect this fixes — but it
 * was VISIBLE, and a title that has vanished off the canvas is the worse of the two failures. So
 * where the card cannot afford the clearing gap, this declines and the caller keeps the flat gap.
 *
 * The threshold is stated in the card's own terms — the insets, the labels, their margin and the
 * plot that has to survive — because that is the arithmetic ECharts is doing when it decides to
 * spill. `26` is the title band `plotWidthLessGutter` also charges, kept in step with it.
 */
export function nameGapClearOf(labelWidth: number, size: EChartsSize): number | undefined {
  const g = CHART_THEME.grid;
  const reserved = (g.left as number) + 26 + labelWidth + AXIS_LABEL_MARGIN + MIN_PLOT_WIDTH;
  if (!(size.width - (g.right as number) >= reserved)) return undefined;
  return labelWidth + AXIS_LABEL_MARGIN + HORIZONTAL_TICKS_GAP;
}

/**
 * The `nameGap` a BOTTOM axis's name needs to clear its own labels — the same defect
 * `nameGapClearOf` fixes one axis over, and the same three terms, because `nameGap` is measured
 * from the axis line on every edge and the labels are what sits between the two wherever they are.
 *
 * IT IS A SECOND FUNCTION AND NOT AN ARGUMENT TO THE FIRST, and the reason is entirely in the
 * CLAMP. What the two compute is identical (`AXIS_LABEL_MARGIN + labels + HORIZONTAL_TICKS_GAP`);
 * what they decline for is not, and folding them together would put one of the two clamps on an
 * axis it is wrong for.
 *
 * The live defect, at 166x184: `category` occupied y 106.2-119.2 while its four 90°-rotated names
 * ran 86.2-147.0 — the title drawn straight down the middle of the labels it names, intersecting
 * all four. `nameGap` was a flat 28 from an axis line at 78.2, and the names need 68.8.
 *
 * WHY THE LEFT AXIS'S CLAMP WOULD NOT TRANSFER. `nameGapClearOf` declines on a narrow card because
 * ECharts stops honouring `grid.left` past a certain label width and SPILLS the labels leftwards,
 * so a gap measured from where they were expected walks the title off the canvas — measured there
 * at x -1, -6 and -100. Downwards there is no spill: the plot simply shrinks, the axis line rises
 * with the labels, and the title rises with the axis line. Rendered at 166x184 with a category name
 * of 108.7px the plot came out 7.8px tall and the title still landed at y 153-166 on a 184px card —
 * the same place a 60.8px name put it, because both are measured from an axis line that had already
 * moved. So the unclamped gap is SAFE over the whole range where the left axis's is not.
 *
 * …UNTIL THE PLOT RUNS OUT, which is the case this does decline for. ECharts stops shrinking at a
 * zero-height plot (measured: exactly 0.00 on a 120px card with those same four names), and from
 * there the axis line cannot rise any further while the labels below it keep growing. The condition
 * below is that case stated in full — the top inset, the half line the topmost value label
 * overhangs by, the gap itself and the title's own line — and it is the only arrangement in which
 * the title leaves the card. Where it does, the caller keeps the flat 28 and the title is drawn
 * across its labels, which is this fix's own defect and deliberately the accepted outcome:
 * `nameGapClearOf` argues it at length, and a title one can read over the top of beats one that is
 * not there.
 *
 * A HIDDEN AXIS IS THE CALLER'S BUSINESS, not this function's. When the ladder answers `hide` no
 * label is drawn, there is nothing to clear, and a gap computed from labels that are not on the
 * card is how the title ends up at y 325 on a 184px SVG — measured, on the first version of this.
 * The caller passes a band of 0 there and gets `AXIS_LABEL_MARGIN + HORIZONTAL_TICKS_GAP` back,
 * which is under the flat 28 it would otherwise use; `??` picks the larger of the two by the
 * caller's own arithmetic rather than by a special case in here.
 */
export function bottomNameGapClearOf({
  band,
  size,
  gridTop,
  fontSize = CHART_THEME.axisFontSize,
}: {
  /** The vertical band the labels occupy — `categoryLabelBand`, the same number
   *  `plotHeightLessLabels` is given, so the gap and the plot cannot disagree about where the
   *  labels end. */
  band: number;
  size: EChartsSize;
  /** The builder's own top inset, which is where a squeezed plot bottoms out. */
  gridTop: number;
  fontSize?: number;
}): number | undefined {
  const gap = AXIS_LABEL_MARGIN + band + HORIZONTAL_TICKS_GAP;
  if (!(size.height >= gridTop + fontSize / 2 + gap + fontSize)) return undefined;
  return gap;
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

/**
 * The cheapest stride from `boundary.steps` whose drawn labels fit, or `undefined` if none does —
 * Metabase's `computeTimeseriesTicksInterval` (`timeseries.ts:216-263`) walking its ladder upwards
 * from the data's own unit and stopping at the first rung under the width budget.
 *
 * A label drawn every `every`th bucket owns `every * dimensionWidth` of the axis, and the rung fits
 * when the widest label THAT RUNG WOULD DRAW, plus one gap of clear ink, does.
 * `HORIZONTAL_TICKS_GAP` rather than Metabase's own `TICK_BUFFER_PIXELS` of 10: this module already
 * answers "how much ink-free space between two neighbouring labels" with 6, above, and two constants
 * for one question in one file is how they drift apart. Padding matches upstream, which measures
 * `getPaddedAxisLabel(formatter(date))` at `timeseries.ts:290-291` — the same pessimism the overlap
 * test uses.
 *
 * WHICH WAY THE 4px CUTS, since it is a real parity difference and not a rounding detail: Metabase
 * fits `floor(width / (widest + 10))` ticks and egret `floor(width / (widest + 6))`, so egret's
 * budget is the looser one and egret thins LESS aggressively. At a boundary width egret draws every
 * 2nd month where Metabase draws every 3rd — more labels, more tightly spaced. That is the
 * difference a side-by-side will photograph; if it reads as crowded rather than as generous, this
 * constant is the one to reconcile.
 *
 * THE DRAWN SET, NOT THE UNIT'S WORST CASE. This is where the port stops following Metabase, and
 * where it stops following its own first version, so it is argued rather than stated.
 *
 * The candidate used to be `boundary.widestLabel` — `widestPeriodName(unit, drawLabel)`, the widest
 * name the unit could EVER produce — measured once and charged at every rung. At `month` that string
 * is "September 2026". But a rung of `every` draws indices 0, `every`, `2 * every`, … and nothing
 * else, so at the rungs that decide a narrow card it was measuring a string the axis does not draw:
 * twelve monthly buckets at rung 12 draw "January 2026", alone.
 *
 * MEASURED IN THE BROWSER, not simulated — the ZZ Chart Parity dashboard, a 166x184 card, Chrome at
 * `400 13px` in the app's own `--font-sans` stack, read out of this function's own inputs on the
 * four charts that carry `xAxisUnit: "month"` (line, area, combo, waterfall):
 *
 *     plotWidth 93.8 over 12 buckets  ->  dimensionWidth 7.82
 *     " September 2026 "   109.3px        what the old candidate measured
 *     " January 2026 "      83.5px        what rung 12 actually draws
 *
 * Rung 12 is `alone`, so its budget is the plot exactly: 93.8px. The drawn label CLEARS it by
 * 10.3px; the hypothetical one loses by 15.5. And every rung past 12 has that same budget — the
 * `Math.min` below clamps them all to the plot — so once the unit's worst case loses, the whole
 * remaining ladder loses with it, the axis falls through to the rotate ladder, and at 7.82px per
 * category that ladder hides it. Line, area and combo drew NO x-axis label at all. The waterfall's
 * 13 categories reach rung 12 with TWO drawn labels rather than one, and rung 24 alone against the
 * same plot with the same 109.3px candidate, so it lost its "January 2026" too — in the commit whose
 * own note recorded it as unchanged.
 *
 * WHAT THAT GIVES UP, because it was a real property and not an accident. `widestPeriodName` existed
 * so a chart's stride would not move as its data's range moved, which is Metabase's rule too: it
 * sizes ticks against `getRepresentativeDates` rather than against its data (`maxTicksForChartWidth`,
 * `timeseries.ts:280-299`), on the grounds that a month axis whose visible range happens to be
 * May–July would size itself on "May 2026" and start clipping the moment the range slid to September.
 *
 * THAT REASON DOES NOT TRANSFER, and the difference is structural rather than a preference. Metabase
 * strides a TIME axis under a data zoom: the visible range moves while the layout that chose the
 * stride does not re-run, so a stride chosen from the visible slice is stale as soon as the slice
 * changes. egret strides a CATEGORY axis over the whole result set, with no zoom, and
 * `EChartsRenderer` re-runs the builder — and therefore this function — on every resize and every
 * new result. There is no state in which the labels change and the stride does not. The range cannot
 * slide out from under a stride here, so what the stability bought was a stride sized against a
 * string the chart never renders, at a cost of four blank axes.
 *
 * What is genuinely conceded is that two charts at the same unit and the same width can now land on
 * DIFFERENT rungs when their months differ in length — a February-to-April axis may thin one rung
 * less than a July-to-September one. Both draw the labels they measured, which is the property that
 * decides whether a reader can read the axis; the rung is not.
 *
 * THREE OF METABASE'S RULES ARE DELIBERATELY NOT PORTED, since each would be wrong here:
 *
 *   - `minTickCount = 2`, which steps BACK a rung when the chosen one would draw fewer than two
 *     ticks and accepts the overflow. On a category axis that draws two labels overlapping; the
 *     honest answer at that width is one label, or none and a rotation. So the last rung stands.
 *   - the `interval` FALLBACK to the largest rung when nothing fits. Returning `undefined` hands the
 *     axis back to the rotate ladder, which is PR 02's behaviour and the right one — Metabase has no
 *     rotate ladder to hand back to.
 *   - stride 1, skipped outright. "Every label" was already asked and answered above, by `overlaps`,
 *     over every adjacent PAIR rather than over one rung's drawn set — a strictly stronger test than
 *     anything this loop could re-run, so re-asking it here could only overrule it.
 *
 * Rungs at or past the label count all draw the same single label at index 0, so the first of them
 * is the last one worth testing — and it IS worth testing, because "one label or none" is a real
 * rung of the ladder (twelve months at every 12th is "January 2026" alone) and not a degenerate
 * case to skip.
 *
 * THAT LAST RUNG IS BUDGETED DIFFERENTLY FROM THE ONES BELOW IT, and getting that wrong is what
 * made two builders written to mirror each other disagree on the same axis. Both halves of the
 * interior budget — `every * dimensionWidth` of owned axis, minus a gap of clear ink — describe a
 * label with NEIGHBOURS. The single label this rung draws has none:
 *
 *   - IT CANNOT OWN MORE THAN THE PLOT. `every * dimensionWidth` runs off the end of the axis as
 *     soon as `every` passes the bucket count, so a rung of 24 over 13 categories "offers" nearly
 *     twice the width that exists. Hence `Math.min(every, count)`, which at this rung is the whole
 *     plot and not a pixel more.
 *   - IT IS NOT CHARGED THE GAP. `HORIZONTAL_TICKS_GAP` is the ink-free space demanded BETWEEN two
 *     neighbouring labels (see the constant, and the overlap test that is its other user). Charging
 *     it here bills a label for clearance from a neighbour that is not drawn.
 *
 * …AND IT IS NOT CHARGED THE PADDING EITHER, which is the same argument as the gap and was left
 * half-applied. `pad` exists because Metabase measures `getPaddedAxisLabel(formatter(date))` and its
 * own comment is "we have to be pessimistic" — pessimism about two labels touching. This module
 * already draws that line once: the overlap test above measures PADDED and the height guard measures
 * BARE, because the height guard asks "how much room does this one label need" rather than "do these
 * two collide". The rung that draws one label asks the height guard's question, so it gets the
 * height guard's answer. Two spaces are 7.2px at `400 13px`, and on the live waterfall they were the
 * whole margin: " January 2026 " is 90.6px against an 85.7px plot and "January 2026" is 83.5px.
 *
 * BOTH ARMS ARE STILL LOAD-BEARING once the drawn set is measured, and the waterfall is the case
 * that shows it. Twelve monthly buckets plus a "Total" is 13 categories: rung 12 draws indices 0 and
 * 12 — "January 2026" and "Total", two labels, so it is NOT `alone` and is rightly charged the gap —
 * and rung 24 is its first `alone` rung, offering `24 * dimensionWidth`, roughly twice the axis that
 * exists, if the min did not clamp it. That is how the waterfall once thinned on a live card at a
 * width where the line chart beside it hid: same unit, same twelve months, opposite answers, decided
 * by whether a chart appends a Total. The two now measure real drawn labels against the same clamped
 * budget, so what is left between them is only WHICH rung they land on — and every rung at or past
 * the count draws index 0 and nothing else, so they draw the same axis.
 *
 * The gap arm is no longer what decides the live card (at 93.8px of plot, "January 2026" clears with
 * or without the 6px), but it is still the correct rule and the window where it bites is real, so
 * `axisLabelFit.test.ts` brackets it from both sides rather than leaving it to the next card that
 * lands inside it.
 */
function strideThatFits(
  boundary: XLabelBoundary,
  /** The labels the axis would draw at stride 1 — the same array `fitXLabels` measured for overlap,
   *  in axis order. Every rung slices its own drawn set out of this, which is what makes the budget
   *  a statement about the labels on the card rather than about the unit. */
  labels: string[],
  dimensionWidth: number,
  /** Measures a string VERBATIM — this applies `pad` itself, because whether a rung is charged the
   *  pessimistic two spaces depends on whether it draws a neighbour. */
  widthOf: (text: string) => number,
): number | undefined {
  const { steps } = boundary;
  // Empty for a unit that names no period. Belt and braces against the caller's own `namesAPeriod`
  // guard: without it an ordinal axis could reach the ladder below and be thinned into "every second
  // product", which the type is shaped to make unrepresentable.
  const count = labels.length;
  if (steps.length === 0 || count === 0) return undefined;
  for (const every of steps) {
    if (every < 2) continue;
    const drawn: string[] = [];
    for (let i = 0; i < count; i += every) drawn.push(labels[i]);
    // At or past the count this rung draws index 0 and nothing else — see the two arms above for
    // what that changes, and why it is also the last rung worth testing. Derived from the drawn set
    // rather than re-deriving `every >= count`, so the two cannot disagree.
    const alone = drawn.length === 1;
    const budget = Math.min(every, count) * dimensionWidth;
    // `ticksToMeasure` for the same reason the overlap test above uses it, and reusing it rather
    // than repeating its rule: a rung of 7 over ten years of daily buckets draws 521 labels, and
    // measuring every one of them at every rung is the cost `TICKS_TO_MEASURE` exists to bound. It
    // is exact where it matters — 50 or fewer drawn labels are all measured — and past that it keeps
    // the longest by character count, which is the one that decides this comparison.
    const width = Math.max(...ticksToMeasure(drawn).map((l) => widthOf(alone ? l : pad(l))));
    if (width + (alone ? 0 : HORIZONTAL_TICKS_GAP) <= budget) return every;
    if (alone) break;
  }
  return undefined;
}

export function fitXLabels({
  labels,
  plotWidth,
  outerHeight,
  boundary,
  fontSize = CHART_THEME.axisFontSize,
  measure = measureText,
}: {
  labels: string[];
  /** The drawable width the categories share — INNER, after the grid insets and the value-axis
   *  gutter. The caller's estimate leans UNDER in the common case and is the better direction:
   *  erring small rotates a label that would have fitted, which is mildly ugly and still readable.
   *  Erring large leaves labels upright that should have turned — or, on a bucketed axis, thinned
   *  one rung less than they needed — and since a fit makes the axis `decided`, they are then drawn
   *  OVERLAPPING rather than dropped. See `estimatePlotWidth` above, which states both failures. */
  plotWidth: number;
  /** The container's height, Metabase's `outerHeight` — deliberately NOT the inner counterpart of
   *  `plotWidth`, which is why it is not called `plotHeight`. The 70% guard is a share of the whole
   *  card (`checkHeight`), so passing an inset height silently tightens it. */
  outerHeight: number;
  /** Present only when the categories are temporal BUCKETS; see `XLabelBoundary`. Absent is the
   *  normal case and means PR 02's all-or-nothing ladder, unchanged. */
  boundary?: XLabelBoundary;
  fontSize?: number;
  /** Injectable: `measure-text.ts` returns `{width: 0, height: 0}` under jsdom by design ("Size
   *  claims belong in a browser"), so tests drive the ladder with a stub rather than assert a
   *  pixel that is always zero. */
  measure?: TextMeasurer;
}): XLabelFit {
  const upright: XLabelFit = { kind: "show", rotate: 0 };
  if (labels.length === 0 || plotWidth <= 0) return upright;

  // The family the chart is actually DRAWN in (`echarts/base.ts` sets it from the same token), not
  // the literal "inherit" — `axisFont` above carries the argument, and holding it in one place is
  // what keeps this ladder measuring at the same font as the gutter it is budgeted against.
  const style = axisFont(fontSize);
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

  // BEFORE the rotate ladder, never instead of it: on a bucketed date axis two upright period names
  // beat twelve tilted ones, which is what Metabase draws — but a stride that does not fit falls
  // straight through to the rungs below with nothing changed.
  const every = boundary && strideThatFits(boundary, labels, dimensionWidth,
    (t) => measure(t, style).width);
  if (every != null) return { kind: "thin", rotate: 0, every };

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
