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
  const g = CHART_THEME.grid;
  // The family the chart is actually DRAWN in, never the literal "inherit": a CSS-wide keyword is
  // not a legal family in the `font` shorthand, so the canvas would reject the whole declaration
  // and measure at its 10px sans-serif default. Same rule, and the same reason, as `fitXLabels`.
  const gutter = measureText(widestTickLabel(widestValue, format), {
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
