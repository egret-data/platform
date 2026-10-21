import type { EChartsCoreOption } from "echarts/core";
import type { EChartsSize } from "../EChartsRenderer";
import type { ColumnFormat } from "../../format-types";
import { formatValue, formatDimensionValue } from "../../format";
import {
  bottomNameGapClearOf, categoryLabelBand, estimatePlotWidth, fitXLabels, plotHeightLessLabels,
  verticalValueAxisFit, widestLabelWidth, type XLabelFit,
} from "../axisLabelFit";
import { ariaDescription, baseOption, tooltipSurface } from "../base";
import { sortCategories } from "./cartesian";
import {
  CHART_THEME, SERIES_PALETTE, COLOR_FALLBACK, resolveColor, resolveFontFamily, fadeColor, scaleLightness,
} from "../palette";
import { measureText } from "../../measure-text";

export interface ResultColumn { name: string; type: string }

/**
 * Whether "Unpin from zero" is OFFERED on a box plot — Metabase's `isYAxisUnpinFromZeroValid`
 * (`shared/settings/cartesian-chart.ts:245-260`) with everything that cannot vary here removed.
 *
 * Upstream that function refuses three DISPLAYS (area, bar, waterfall) and any stacked chart. A box
 * plot is none of the three and has no stacking, so both of those arms are answered by the chart
 * type itself; all that is left is the auto-range arm, which egret reads off the derived pair
 * rather than off Metabase's stored `graph.y_axis.auto_range` (see `unpinFromZeroApplies` in
 * `./cartesian.ts`, which carries the full translation and the reason).
 *
 * EXPORTED so the builder and the settings panel ask the SAME function. Restating "…and no bound is
 * set" in the panel is how a control comes to be offered on a chart that will not honour it — this
 * programme's "saves but does not paint" defect, reached from the control side.
 */
export function unpinFromZeroAppliesToBoxplot(cfg: { yMin?: number; yMax?: number }): boolean {
  return cfg.yMin == null && cfg.yMax == null;
}

/** Metabase's `boxplot.whisker_type`. */
export type WhiskerType = "tukey" | "min-max";
/** Metabase's `boxplot.points_mode`. */
export type PointsMode = "none" | "outliers" | "all";

export interface BoxplotBuildConfig {
  category?: string;   // grouping column (raw)
  value?: string;      // numeric column whose distribution is summarised
  /** The one colour the whole chart derives from — box fill, border, mean and points. */
  color?: string;
  whiskers?: WhiskerType;
  points?: PointsMode;
  /** Unset means SHOWN, as Metabase's `boxplot.show_mean` defaults to true. */
  showMean?: boolean;
  showLabels?: boolean;
  /** Metabase's `boxplot.show_values_mode`: label the median only, or every statistic. */
  labelMode?: "median" | "all";
  compactValues?: boolean;
  goalValue?: number;
  goalLabel?: string;
  xLabel?: string;
  yLabel?: string;
  showXLabel?: boolean;
  showYLabel?: boolean;
  xAxisTicks?: boolean;
  yAxisTicks?: boolean;
  xLabelRotation?: 45 | 90;
  /**
   * Metabase's `graph.y_axis.unpin_from_zero`.
   *
   * STILL NO `xLabelCompact` TWIN, and the reason has narrowed rather than gone. This axis now HAS
   * an `axisLabel.formatter` (`catLabel`, below) — the note that used to sit here said it had none,
   * and `waterfall.ts` cites this chart as the one its own Compact argument does not extend to, so
   * both were true and are now only half true. What a compact flag would act through is a
   * `date_abbreviate` / `compact` pair spread over the category's STORED format, which `catLabel`
   * consults only when the reader has already set one; with no stored format the category still
   * falls back to the raw cell and the flag has nothing to change. Offering it would therefore be a
   * control that works on some box plots and not others, decided by a setting two panels away.
   * Making it honest means giving this chart the bucketing fallback the cartesian and the waterfall
   * have (`periodName`), which is a different change and is not this one.
   */
  unpinFromZero?: boolean;
  yTickCount?: number;
  yMin?: number;
  yMax?: number;
  logScale?: boolean;
  /**
   * The rows already arrive in an order that MEANS something, so leave the nominal category axis
   * alone. Set by `ChartRenderer` for a chart whose stored spec carries a non-empty `orderby` — the
   * engine has already ordered the rows the way the author asked. Its counterpart on
   * `CartesianBuildConfig` says the same thing; both are read by the one shared `sortCategories`,
   * whose comment states the rule and the two callers.
   *
   * A spec with `orderby: []` must NOT set it: arrival order is then whatever the engine returned,
   * which is the defect PR #394 fixed.
   */
  preserveCategoryOrder?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

/**
 * Metabase's own visual derivation — every colour on the chart comes from ONE series colour, so a
 * box, its whiskers, its mean and its points stay recognisably the same series.
 *
 * egret hard-coded a pale green fill (`#E3F0D4`) beside a green border. Those two hexes agree for
 * exactly one colour; the moment the colour becomes configurable the fill stays green under a blue
 * border. Same defect as the progress chart's track shades and the waterfall's total — "copy the
 * rule, not the value".
 */
const BOX_FILL_ALPHA = 0.15;        // Metabase's BOX_FILL_OPACITY
const MEAN_DARKEN = 0.3;            // MEAN_COLOR_DARKEN_FACTOR
const NON_OUTLIER_ALPHA = 0.6;      // NON_OUTLIER_OPACITY, for "All points"
const MIN_BOX_WIDTH = 3;            // BOXPLOT_MIN_BOX_WIDTH
const MAX_BOX_WIDTH = 60;           // BOXPLOT_MAX_BOX_WIDTH
/** How far a value label sits from its box's centre line — see the labels' own note below. */
const LABEL_DISTANCE = MAX_BOX_WIDTH / 2 + 4;

/**
 * The extra right inset the last box's value labels need, in px — 0 when they already fit inside
 * the plot's half band plus the grid's own right margin.
 *
 * The last box is centred half a band (`plotWidth / categories / 2`) from the plot's right edge, and
 * its label ends `distance + labelWidth` right of that centre. Whatever runs past the band and the
 * margin is clipped. Reserving `extra` also narrows the plot by `extra`, which moves the centre left
 * by `extra / (2 * categories)`, so the inset is solved for rather than taken from the first guess:
 * `distance + labelWidth - (plotWidth - extra) / 2n <= gridRight + extra`.
 */
export function labelOverhangRight({ labelWidth, distance, plotWidth, categories, gridRight }: {
  labelWidth: number; distance: number; plotWidth: number; categories: number; gridRight: number;
}): number {
  if (categories <= 0 || labelWidth <= 0 || plotWidth <= 0) return 0;
  const over = distance + labelWidth - plotWidth / (2 * categories) - gridRight;
  return over > 0 ? Math.ceil(over / (1 - 1 / (2 * categories))) : 0;
}
const MEAN_SYMBOL_SIZE = CHART_THEME.symbolSize + 2; // MEAN_SYMBOL_SIZE_OFFSET

/** Linear-interpolation quantile over a pre-sorted array — Metabase's `getQuantile`. */
function quantile(sorted: number[], q: number): number {
  if (sorted.length === 0) return 0;
  if (sorted.length === 1) return sorted[0];
  const pos = (sorted.length - 1) * q;
  const base = Math.floor(pos);
  const rest = pos - base;
  return rest === 0 ? sorted[base] : sorted[base] + rest * (sorted[base + 1] - sorted[base]);
}

export interface BoxStats {
  box: [number, number, number, number, number]; // min, q1, median, q3, max
  outliers: number[];
  mean: number;
}

/**
 * The five numbers a box draws, plus its outliers and mean — Metabase's `computeBoxPlotDatum`.
 *
 * Two rules egret was missing, both from the same function:
 *  - **A whisker never reaches past the box.** `min(whiskerMin, q1)` / `max(whiskerMax, q3)`.
 *  - **When every point is an outlier the whisker falls back to the QUARTILE**, not to the true
 *    min/max — falling back to the extremes, as egret did, draws a whisker out to the very
 *    outlier it just excluded.
 */
export function boxStats(values: number[], whiskers: WhiskerType = "tukey"): BoxStats {
  const s = [...values].sort((a, b) => a - b);
  const q1 = quantile(s, 0.25);
  const median = quantile(s, 0.5);
  const q3 = quantile(s, 0.75);
  const mean = s.reduce((t, v) => t + v, 0) / s.length;

  if (whiskers === "min-max") {
    // The whiskers reach the extremes, so by construction nothing is an outlier.
    return { box: [s[0], q1, median, q3, s[s.length - 1]], outliers: [], mean };
  }

  const iqr = q3 - q1;
  const low = q1 - 1.5 * iqr;
  const high = q3 + 1.5 * iqr;
  const inRange = s.filter((v) => v >= low && v <= high);
  const whiskerMin = Math.min(inRange.length > 0 ? inRange[0] : q1, q1);
  const whiskerMax = Math.max(inRange.length > 0 ? inRange[inRange.length - 1] : q3, q3);
  return {
    box: [whiskerMin, q1, median, q3, whiskerMax],
    outliers: s.filter((v) => v < low || v > high),
    mean,
  };
}

/**
 * Which points a chart draws, after Metabase's own interaction between the two settings.
 *
 * With Min/Max whiskers there ARE no outliers — the whiskers already reach the extremes — so
 * Metabase removes "Outliers only" from the list and treats a stored "outliers" as "none". Without
 * that, picking Min/Max leaves a selected option that can only ever draw nothing.
 */
export function pointsModeFor(cfg: Pick<BoxplotBuildConfig, "whiskers" | "points">): PointsMode {
  const stored = cfg.points;
  if (cfg.whiskers === "min-max") return stored === "all" ? "all" : "none";
  return stored ?? "outliers";
}

// Metabase-style Box plot: the distribution of a value per category, computed client-side from the
// raw rows — its own comment says "a boxplot needs unaggregated data", and so does egret's spec.
export function buildBoxplotOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: BoxplotBuildConfig,
  /** The container's measured size, from `EChartsRenderer`. Absent when the caller has no
   *  measurement — a unit test, or anything handing over a plain option object; see the x-label fit
   *  below, which is the only thing that reads it. */
  size?: EChartsSize,
): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const ci = cfg.category != null ? idx[cfg.category] : undefined;
  const vi = cfg.value != null ? idx[cfg.value] : undefined;

  const groups = new Map<string, number[]>();
  if (ci != null && vi != null) {
    for (const r of rows) {
      const v = Number(r[vi]);
      if (!Number.isFinite(v)) continue;
      const k = String(r[ci] ?? "");
      const arr = groups.get(k);
      if (arr) arr.push(v); else groups.set(k, [v]);
    }
  }
  /**
   * Sorted, not `[...groups.keys()]`. Map insertion order is the order the ENGINE returned the raw
   * rows in, and a box plot's spec carries `orderby: []` like every other stored chart — measured
   * at `['Widget','Gadget','Doohickey','Gizmo']` on three calls in ten and
   * `['Gizmo','Widget','Gadget','Doohickey']` on the other seven, for the same chart.
   *
   * The SAME `sortCategories` the bar and row charts use, so the three cannot drift and the box
   * plot gets the temporal and numeric branches for free. See its comment for why sorting a nominal
   * axis at all is a deliberate divergence from Metabase.
   *
   * Sorted BEFORE `stats`, so every derived array below — the boxes, the mean points, the outlier
   * and all-point scatters, the tooltip's `categories[i]`, the aria labels — is indexed the same
   * way and a box cannot end up under another category's label.
   *
   * The query is deliberately left UNSORTED BY DEFAULT. The backend WOULD honour an `orderby` here
   * — `push_orderby` runs in the raw-records branch too (`chartquery.rs`) — but a box plot's spec is
   * raw rows under a `row_limit`, and `ORDER BY category LIMIT n` returns the alphabetically-first n
   * ROWS: whole categories would vanish from the chart and the ones left would have truncated
   * distributions. Sorting here fixes the axis without touching which rows the quartiles are
   * computed from, which is why no descriptor derives an order for a box plot.
   *
   * `preserveCategoryOrder` is the case where an author asked for one anyway. The rows then arrive
   * in the order the engine was told to return them, insertion order means something, and sorting
   * it away would make the sort look inert. See the field's own comment.
   */
  const catType = ci != null ? columns[ci].type : "varchar";
  const categories = sortCategories([...groups.keys()], catType, cfg.preserveCategoryOrder) as string[];
  const stats = categories.map((k) => boxStats(groups.get(k)!, cfg.whiskers ?? "tukey"));
  const data = stats.map((s) => s.box);

  const pointsMode = pointsModeFor(cfg);
  const outlierPoints = pointsMode === "none" ? [] : stats.flatMap((s, i) => s.outliers.map((v) => [i, v]));
  // "All points" draws the non-outliers too, fainter, so the outliers stay legible among them.
  const allPoints = pointsMode === "all"
    ? categories.flatMap((k, i) => {
        const out = new Set(stats[i].outliers);
        return groups.get(k)!.filter((v) => !out.has(v)).map((v) => [i, v]);
      })
    : [];
  const showMean = cfg.showMean !== false;
  const meanPoints = showMean ? stats.map((s, i) => [i, s.mean]) : [];

  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  // One colour in, every mark on the chart out.
  const color = cfg.color ?? SERIES_PALETTE[0];
  const boxFill = fadeColor(color, BOX_FILL_ALPHA);
  const meanColor = scaleLightness(color, 1 - MEAN_DARKEN);

  const widest = stats.reduce((n, s) => Math.max(n, ...s.box.map((v) => Math.abs(v))), 0);
  const compact = cfg.compactValues ?? widest >= 10_000;
  /**
   * The widest value the Y-AXIS can print, which is NOT `widest` — deliberately a second number.
   *
   * `widest` is the boxes alone, and with Tukey whiskers a box stops at `whiskerMax`, the largest
   * value INSIDE the fence (`boxStats` above). The outliers it just excluded are still drawn, on a
   * scatter series sharing this axis, and so is the mean, which an outlier drags past the box it
   * belongs to. So the axis extends past `widest` and its top tick can format far wider than the
   * boxes suggest: boxes topping at 500 with one outlier at 1,200,000 leave `compact` false (it
   * keys off `widest`, correctly — it is about the numbers a reader is asked to compare) and the
   * axis then draws "1,200,000", ~60px, where a gutter measured from "500" reserved ~21px. On a
   * ~270px card that ~40px over-estimate of `plotWidth` is larger than any case
   * `estimatePlotWidth` names, and it would show up as labels drawn upright and overlapping.
   *
   * Taken from the point arrays THEMSELVES rather than from `stats`, so it counts a series exactly
   * when that series is drawn: "no points" omits the outliers from the chart and from this, and
   * `showMean: false` does the same for the means. `allPoints` are non-outliers and so can never
   * extend the axis, but they are included anyway to keep the rule "every point series on this
   * axis" rather than a list of the ones that happen to matter.
   *
   * `compact` is deliberately left on `widest`: the shorthand a reader sees should follow the
   * distribution, not one far-flung outlier. Only the GUTTER needs the true extent.
   */
  const axisWidest = [...outlierPoints, ...allPoints, ...meanPoints]
    .reduce((n, p) => Math.max(n, Math.abs(p[1])), widest);
  const fmt = (v: unknown, useCompact = false, force?: boolean) => {
    if (cfg.value == null) return String(v);
    const col = cfg.columnSettings?.[cfg.value];
    return formatValue(v, cfg.metricTypes?.[cfg.value] ?? "double", {
      ...col,
      ...(useCompact ? { compact: col?.compact ?? force ?? compact } : {}),
    });
  };

  /**
   * A CATEGORY AS THE TICK IT IS DRAWN AS — the format this builder read for its value column and
   * never for its category one.
   *
   * `styleColumnsFor` offers the box plot's CATEGORY a Formatting accordion (it fell through to
   * `return resultColumns`), and `fmt` above resolves `columnSettings?.[cfg.value]` only, so a Date
   * style typed there was stored and drawn nowhere. Measured in the browser over `MB Monthly`: the
   * panel read `Date style: January 2024` while the axis and the spoken description both read
   * `2026-01-01`.
   *
   * Metabase's box plot formats it — `echarts/boxplot/model/index.ts`'s `createXAxisFormatter`
   * spreads `settings.column?.(dimensionColumn)` into `formatValue` for every tick — and its
   * generic cartesian axis does the same (`cartesian/model/axis.ts`), which is the rule egret's own
   * bar and line charts already follow. This chart was the one left out.
   *
   * A FORMATTER, NOT A REWRITE OF `categories`. That array is the drill's identity: `BoxplotChart`
   * reads the clicked category back off `xAxis.data[dataIndex]` and hands it to `onValueClick` as
   * the filter value, and `groups` is keyed on it. Formatting it in place is the
   * drill-on-display-strings defect four chart families have now had. So `xAxis.data` stays raw and
   * everything a reader SEES — the ticks, the tooltip heading, the aria description, and the label
   * measurements that decide rotation and gutters — goes through this.
   */
  const catFormat = cfg.category != null ? cfg.columnSettings?.[cfg.category] : undefined;
  const catLabel = (c: unknown) => formatDimensionValue(c, catType, catFormat);
  // Computed ONCE and reused by the three measurements and the aria below. `widestLabelWidth` and
  // `fitXLabels` both walk the whole list against a canvas, and a per-call `.map` would put a
  // formatter between them and the strings ECharts will really draw — which is exactly the drift
  // that makes a fitted axis disagree with the axis on screen.
  const drawnCategories = categories.map(catLabel);

  const xTitle = cfg.showXLabel === false ? undefined : (cfg.xLabel ?? cfg.category);
  const yTitle = cfg.showYLabel === false ? undefined : (cfg.yLabel ?? cfg.value);
  // Whether the labels fit is a question about WIDTH, and until `size` existed this builder could
  // not ask it: the rule here was `longest > 10 && categories.length > 6 ? 30 : 0`, and length and
  // count are only proxies for width. The same rule was deleted from the cartesian builder for the
  // same reason — four categories whose longest is "Doohickey" on a one-third-width card came out
  // as "no rotation needed", and then `interval: "auto"` let ECharts drop three of the four, an
  // axis reading as missing data. See `echarts/axisLabelFit.ts`.
  //
  // Cartesian guards this on `ordinalX` — is the category axis the BOTTOM one, and are its labels
  // the array we hold? A box plot has neither a horizontal mode nor an `xScale`, so both halves of
  // that question are settled by construction: this axis is always a bottom category axis over
  // exactly these `categories`. All that is left to ask is whether there is a size to measure
  // against, and copying a condition that is always true here would read as if there were more.
  //
  // A zero size is not a small chart, it is an UNMEASURED one: `EChartsRenderer` reads
  // `getBoundingClientRect()` before `init`, so a hidden or not-yet-laid-out container reports
  // `{0, 0}`. Measuring it would give a negative `plotWidth` — which the fitter answers "draw every
  // label upright", from a container nobody could see — and a zero height would make the rotated
  // label's projected share infinite and hide the axis. Both are decisions taken against a guessed
  // width, so decline instead.
  const measurable = size != null && size.width > 0 && size.height > 0;
  // An explicit rotation SKIPS the measurement rather than merely outranking it. Computing the fit
  // anyway and using it for `show` would honour the user's 45° and hide their axis in the same
  // breath, which is worse than either outcome alone and looks like the setting doing nothing.
  // Metabase's own auto-selection likewise applies only while the setting is untouched.
  const explicitRotate = cfg.xLabelRotation != null;
  const fit: XLabelFit | undefined = measurable && size && !explicitRotate
    ? fitXLabels({
        // The categories as DRAWN, which is now a different array from the ones the boxes are
        // keyed on: this axis DOES have an `axisLabel.formatter` (`catLabel`), like the cartesian
        // and waterfall axes. Measuring the raw keys would size the gutter and choose the rotation
        // against text ECharts is not going to draw — "2026-01-01" is ten characters where
        // "January 2026" is twelve, so the fit would have been decided a tick too narrow.
        labels: drawnCategories,
        // The gutter is the widest value the y-axis can print — `axisWidest`, which counts the
        // outlier and mean points too, NOT the boxes-only `widest`; see its comment above — through
        // the y-axis's own formatter (`fmt(v, true)` below). `yTitle` is what adds 26 to
        // `grid.left` below.
        // The VALUE and the formatter, not the formatted value: `estimatePlotWidth` rounds to a tick
        // the axis would actually draw before measuring it, which a pre-formatted string cannot be.
        plotWidth: estimatePlotWidth(size, axisWidest, (v) => fmt(v, true), Boolean(yTitle)),
        // The CONTAINER's height, not an inset one: the 70% guard is a share of the whole card
        // (Metabase's `checkHeight`), so passing a grid-adjusted height silently tightens it.
        outerHeight: size.height,
      })
    : undefined;
  const rotate = cfg.xLabelRotation ?? (fit?.kind === "rotate" ? fit.rotate : 0);
  // Either way the axis has now DECIDED, and a decided axis draws every label or none — never the
  // subset `interval: "auto"` leaves ECharts free to pick.
  const decided = fit != null || explicitRotate;

  /**
   * THE OTHER AXIS HAS THE SAME DEFECT ON THE OTHER DIMENSION, and this chart is one of the two the
   * live 166x184 dashboard showed it on: the value axis drew `20 40 60 80` on a 10.7px pitch against
   * 15px label boxes, every neighbouring pair overlapping, while `category` was drawn straight down
   * its own four rotated names.
   *
   * The cause is the ladder above rather than the numbers: four product names turned 90° reserve
   * 72px of a 184px card, leaving a 56px plot for six ticks. `echarts/axisLabelFit.ts` carries the
   * whole argument and all three measurements — `categoryLabelBand` for the band those names take,
   * `plotHeightLessLabels` for what is left, `verticalValueAxisFit` for the pitch — because
   * the cartesian builder and this one lay the same grid out and two copies of that arithmetic is
   * two places for it to drift.
   *
   * MEASURED OR DECLINED, the same rule the x-label fit above keeps: `widestLabelWidth` reports 0
   * where there is no canvas, and a band computed from that is a guess about a card nobody measured.
   */
  const measuredCategories = measurable ? widestLabelWidth(drawnCategories) : 0;
  const categoryGutter = measuredCategories > 0 ? measuredCategories : undefined;
  const categoryBand = categoryGutter != null && fit?.kind !== "hide"
    ? categoryLabelBand({ rotate, widestLabel: categoryGutter })
    : 0;
  /**
   * ROOM FOR THE LAST BOX'S VALUE LABELS (KI-165). They are drawn to the RIGHT of each box, at
   * `LABEL_DISTANCE` from its centre line, and `containLabel` reserves room for axis labels only —
   * so on the last box a label wider than the space between that box and the card edge was clipped
   * ("31.6" for "31.6K"). Measured, like the x-label fit: the strings this builder will draw there,
   * in the bold label font, against the plot width the fit above estimates. No measurement (jsdom,
   * an unlaid-out card) reserves nothing, the old behaviour.
   */
  const lastStats = stats[stats.length - 1];
  const lastLabels = cfg.showLabels && lastStats
    ? ((cfg.labelMode ?? "median") === "all" ? lastStats.box : [lastStats.box[2]])
      .map((v) => fmt(v, true, cfg.compactValues ?? true))
    : [];
  const labelFont = { size: CHART_THEME.labelFontSize, family: resolveFontFamily(), weight: CHART_THEME.labelWeight };
  const lastLabelWidth = measurable && lastLabels.length > 0
    ? widestLabelWidth(lastLabels, (t) => measureText(t, labelFont))
    : 0;
  const labelRoom = size && lastLabelWidth > 0
    ? labelOverhangRight({
        // + 4: Metabase's label padding, so the text clears the card edge rather than touching it
        // (measured live at 1024px: without it the label ended 1px past the <svg>).
        labelWidth: lastLabelWidth + 4,
        distance: LABEL_DISTANCE,
        plotWidth: estimatePlotWidth(size, axisWidest, (v) => fmt(v, true), Boolean(yTitle)),
        categories: stats.length,
        gridRight: CHART_THEME.grid.right as number,
      })
    : 0;
  // The insets this builder is about to emit, hoisted so the fit subtracts the grid it really uses.
  const grid = {
    ...CHART_THEME.grid,
    bottom: (CHART_THEME.grid.bottom as number) + (xTitle ? 22 : 0),
    left: (CHART_THEME.grid.left as number) + (yTitle ? 26 : 0),
    right: (CHART_THEME.grid.right as number) + labelRoom,
  };
  /**
   * How far the value axis runs. Every value the axis has to hold — the five box numbers, the
   * outliers, the non-outlier points and the means — and ZERO, because ECharts' `scale` defaults to
   * false and this axis reaches the origin whether or not a price does. It is the same set
   * `axisWidest` above takes its magnitude from, kept as two ends rather than one because an
   * interval divides a DISTANCE: a distribution running -40 to +60 is 100 across while its biggest
   * number is 60. `yMin`/`yMax` replace their end when the user has set one.
   */
  //
  // …UNLESS THE CHART IS UNPINNED, which is why the seed below is not the literal `0, 0` this
  // paragraph described until now. Metabase's `graph.y_axis.unpin_from_zero` is one line on the
  // axis upstream (`scale: !!settings[...]`), but this builder does not leave the extent to
  // ECharts: `down` writes an explicit `min`/`max`, and an explicit `min` outranks `scale`
  // outright. Setting the flag without moving the seed would have left the axis pinned on exactly
  // the measured cards — the ones small enough for `verticalValueAxisFit` to engage — which is the
  // "saves but does not paint" defect, silent under jsdom because nothing there is measured.
  //
  // The gate is the same rule `unpinFromZeroApplies` states for the cartesian family, reduced to
  // what can vary here: a box plot's display is none of the three Metabase excludes and it cannot
  // stack, so all that is left of `isYAxisUnpinFromZeroValid` is its auto-range arm — an explicit
  // bound already decides the extent, so the flag would be inert beside one.
  const unpin = cfg.unpinFromZero === true && unpinFromZeroAppliesToBoxplot(cfg);
  // REDUCED rather than spread into `Math.min`: "All points" draws every raw row, and a spread of
  // one argument per point is a call frame that overflows on a big enough result set.
  let axisLo = unpin ? Infinity : 0, axisHi = unpin ? -Infinity : 0;
  for (const v of [
    ...stats.flatMap((s) => s.box),
    ...outlierPoints.map((p) => p[1]), ...allPoints.map((p) => p[1]), ...meanPoints.map((p) => p[1]),
  ]) {
    if (!Number.isFinite(v)) continue;
    if (v < axisLo) axisLo = v;
    if (v > axisHi) axisHi = v;
  }
  // A chart with nothing finite to plot keeps its seed, and `±Infinity` is not an extent — see the
  // identical fallback in `options/cartesian.ts`'s `axisExtent` for why this is stated rather than
  // left to the fits' own `isFinite` guards to answer by accident.
  if (!Number.isFinite(axisLo) || !Number.isFinite(axisHi)) { axisLo = 0; axisHi = 0; }
  const down = categoryGutter != null && size && !cfg.logScale && cfg.yTickCount == null
    ? verticalValueAxisFit({
        plotHeight: plotHeightLessLabels({ size, band: categoryBand, grid }),
        lo: cfg.yMin ?? axisLo,
        hi: cfg.yMax ?? axisHi,
      })
    : undefined;
  // NO ROOM IS RESERVED BELOW THE PLOT FOR A TILTED LABEL, at any rotation, measured or explicit —
  // the 44/18 ladder this builder used to add on the explicit path is gone with it. See
  // `XLabelFit`'s doc in `../axisLabelFit.ts`: `containLabel` already expands the grid around the
  // rotated labels, and every pixel added on top came off the plot one-for-one.

  // Metabase labels the median by default and every statistic on "All"; a box's five numbers are
  // `[min, q1, median, q3, max]`, so the median is index 2.
  const labelMode = cfg.labelMode ?? "median";
  const labelPoints = cfg.showLabels
    ? stats.flatMap((s, i) => (labelMode === "all" ? s.box.map((v) => [i, v]) : [[i, s.box[2]]]))
    : [];
  const boxSeries: Record<string, unknown> = {
    type: "boxplot",
    data,
    // A box narrower than 3px is a line, and past 60px it stops reading as a distribution and
    // starts reading as a bar — Metabase clamps both ends.
    boxWidth: [MIN_BOX_WIDTH, MAX_BOX_WIDTH],
    itemStyle: { color: boxFill, borderColor: color, borderWidth: 1 },
    emphasis: { focus: "series", itemStyle: { borderColor: scaleLightness(color, 0.8), borderWidth: 2 } },
    blur: { itemStyle: { opacity: CHART_THEME.blurOpacity } },
  };
  if (cfg.goalValue != null) {
    boxSeries.markLine = {
      silent: true,
      symbol: "none",
      data: [{ yAxis: cfg.goalValue }],
      label: { formatter: cfg.goalLabel ?? "Goal", position: "insideEndTop", color: axisLabelColor },
      lineStyle: { color: axisLabelColor, type: "dashed" },
    };
  }

  return {
    ...baseOption(),
    /**
     * A BOX IS FIVE NUMBERS, AND THE DESCRIPTION USED TO READ OUT ONE OF THEM WITHOUT SAYING WHICH.
     *
     * It announced "Box plot of price by category. 4 values: Doohickey 50.06, …" — four bare
     * numbers. They are MEDIANS, which nothing in the sentence said, so a screen-reader user was
     * invited to take 50.06 for the category's value; and the spread, which is the entire reason to
     * choose a box plot over a bar chart, was not described at all. A sighted reader gets the
     * quartiles and the whiskers from the shape and all five numbers from the tooltip below.
     *
     * `noun: "boxes"` fixes the count's claim and the parenthetical carries the rest, in the box's
     * own order — the same five statistics the tooltip prints, formatted through the same `fmt`, so
     * the two readings of this chart cannot drift apart. Both fields are optional on
     * `ariaDescription` and no other caller passes either, so this is the only description in the
     * product that moves.
     */
    aria: ariaDescription({
      kind: "Box plot", metric: cfg.value, dimension: cfg.category, format: (v) => fmt(v),
      noun: "boxes",
      points: drawnCategories.map((c, i) => {
        // `[min, q1, median, q3, max]` — index 2 is the median, and the four it sits between are
        // what the parenthetical adds. A category with no box (a group that produced no rows)
        // keeps the `undefined` value the description already handled and gains no detail.
        const box = stats[i]?.box;
        return {
          name: c,
          value: box?.[2],
          detail: box
            ? `median; min ${fmt(box[0])}, Q1 ${fmt(box[1])}, Q3 ${fmt(box[3])}, max ${fmt(box[4])}`
            : undefined,
        };
      }),
    }),
    grid,
    // The series-level `tooltip: { trigger: "item" }` entries below only make the point series
    // hoverable at all; the box the tooltip is DRAWN in is this one, so the shared surface belongs
    // here and covers all of them.
    tooltip: {
      trigger: "item",
      ...tooltipSurface(),
      formatter: (p: { seriesType?: string; dataIndex?: number; data?: unknown; value?: unknown }) => {
        if (p.seriesType === "boxplot") {
          const i = p.dataIndex ?? 0;
          const s = stats[i];
          if (!s) return "";
          const row = (k: string, v: number) => `<div>${k}: ${fmt(v)}</div>`;
          return `<div style="font-weight:600">${drawnCategories[i]}</div>`
            + row("Max", s.box[4]) + row("Q3", s.box[3]) + row("Median", s.box[2])
            + row("Q1", s.box[1]) + row("Min", s.box[0])
            + (showMean ? row("Mean", s.mean) : "");
        }
        // A point series datum is `[categoryIndex, value]`.
        const d = Array.isArray(p.data) ? p.data : undefined;
        const i = d ? Number(d[0]) : 0;
        return `<div style="font-weight:600">${drawnCategories[i] ?? ""}</div><div>${fmt(d ? d[1] : p.value)}</div>`;
      },
    },
    xAxis: {
      type: "category", data: categories,
      name: xTitle, nameLocation: "middle",
      // `nameGap` is measured from the AXIS LINE, and what sits between the line and the name is the
      // column of category labels — which, once the ladder above turns them 90°, run down into
      // exactly the band 28 places the title in. Live at 166x184: `category` occupied y 106.2-119.2
      // against names running 86.2-147.0, drawn across all four. `bottomNameGapClearOf` states the
      // three terms and owns the one card it declines on; `Math.max` keeps 28 as the floor, which is
      // the gap an axis whose labels are hidden should keep.
      nameGap: Math.max(28, (categoryGutter != null && size
        ? bottomNameGapClearOf({ band: categoryBand, size, gridTop: grid.top as number })
        : undefined) ?? 28),
      nameTextStyle: { color: axisLabelColor },
      axisLabel: {
        fontSize: CHART_THEME.axisFontSize, color: axisLabelColor,
        interval: decided ? (0 as const) : ("auto" as const), rotate,
        // The category's own `column_settings`, applied to the tick and to nothing else — see
        // `catLabel`. `xAxis.data` above stays the raw key the drill filters by.
        formatter: (v: unknown) => catLabel(v),
        // The third answer the ladder can give: when even a rotated label would eat 70% of the
        // card's height, Metabase draws no axis rather than a wall of text.
        show: fit?.kind !== "hide",
      },
      axisLine: { show: cfg.xAxisTicks !== false, lineStyle: { color: gridLine } },
      axisTick: { show: cfg.xAxisTicks !== false },
    },
    yAxis: {
      type: cfg.logScale ? "log" : "value",
      // Metabase's `graph.y_axis.unpin_from_zero`, whose whole upstream implementation is ECharts'
      // own `scale` flag (`echarts/cartesian/option/axis.ts:380`). Written only when true, as on the
      // cartesian family: pinned is ECharts' default too, so an absence already says it and every
      // existing box plot's option stays byte-identical.
      ...(unpin ? { scale: true } : {}),
      name: yTitle, nameLocation: "middle", nameGap: 44,
      nameTextStyle: { color: axisLabelColor },
      // The ends the fit's interval divides, written FIRST so a range the user set outranks them —
      // see `verticalValueAxisFit` for why ECharts will not round them itself once an `interval` is
      // named, and for the short last step it leaves behind.
      ...(down != null ? { min: down.min, max: down.max } : {}),
      ...(cfg.yMin != null ? { min: cfg.yMin } : {}),
      ...(cfg.yMax != null ? { max: cfg.yMax } : {}),
      // The user's own tick count wins outright; the measured fit is the fallback behind it, and it
      // arrives ALONE — an `interval` beside a `splitNumber` would win over it in ECharts, which is
      // why the fit above declines outright once one is set rather than being masked here.
      ...(cfg.yTickCount != null ? { splitNumber: cfg.yTickCount }
        : down != null ? { interval: down.interval } : {}),
      axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: (v: number) => fmt(v, true) },
      axisLine: { show: cfg.yAxisTicks !== false, lineStyle: { color: gridLine } },
      axisTick: { show: cfg.yAxisTicks !== false },
      splitLine: { lineStyle: { color: gridLine } },
    },
    series: [
      boxSeries,
      // Non-outliers, only in "All points" — fainter than the outliers so those stay the ones the
      // eye lands on.
      ...(allPoints.length > 0 ? [{
        type: "scatter", data: allPoints, symbolSize: CHART_THEME.symbolSize,
        itemStyle: { color: fadeColor(color, NON_OUTLIER_ALPHA) }, tooltip: { trigger: "item" }, z: 2,
      }] : []),
      // Outliers, in the series' own colour at the shared scatter opacity.
      ...(outlierPoints.length > 0 ? [{
        type: "scatter", data: outlierPoints, symbolSize: CHART_THEME.symbolSize,
        itemStyle: { color, opacity: CHART_THEME.scatterOpacity }, tooltip: { trigger: "item" }, z: 3,
      }] : []),
      // The mean, as a diamond in the series colour darkened — Metabase's rule, so it reads as
      // this box's mean rather than as a separate series.
      ...(meanPoints.length > 0 ? [{
        type: "scatter", data: meanPoints, symbol: "diamond", symbolSize: MEAN_SYMBOL_SIZE,
        itemStyle: { color: meanColor }, tooltip: { trigger: "item" }, z: 4,
      }] : []),
      // Data labels. An ECharts `boxplot` series will not label its own five numbers, so they are
      // drawn by a size-0 scatter carrying only text — the same construction Metabase uses for the
      // waterfall's labels. "Median only" is one point per box; "All" is all five.
      ...(labelPoints.length > 0 ? [{
        type: "scatter", data: labelPoints, symbolSize: 0, silent: true, z: 5,
        tooltip: { show: false },
        label: {
          // The label anchors at the box's CENTRE line, so it has to clear the box's own half
          // width or it prints across the border — which is what `distance: 6` did. Half of
          // `MAX_BOX_WIDTH` plus Metabase's own 4px label padding always clears it. A narrower box
          // leaves the label further out than strictly needed; overlapping the border is the worse
          // of the two, and ECharts gives an option builder no way to know the drawn width.
          show: true, position: "right", distance: LABEL_DISTANCE,
          fontSize: CHART_THEME.labelFontSize, fontWeight: CHART_THEME.labelWeight, color: axisLabelColor,
          // Metabase's `graph.label_value_formatting` defaults to COMPACT for a box plot, and the
          // reason shows immediately: a median of 87.957 sits beside a box 47px tall. The axis
          // keeps the shared "compact once the numbers get wide" rule; only the labels differ,
          // which is how Metabase splits it too.
          formatter: (p: { data: number[] }) => fmt(p.data[1], true, cfg.compactValues ?? true),
        },
        // THE SAME BARGAIN THE CARTESIAN AND WATERFALL BUILDERS STRIKE, made against the control
        // this panel actually offers. There is no `labelDensity` here; "Values to display" is the
        // box plot's density control, because "All" is what stacks five numbers down one box's
        // height and is the only setting that can crowd. So "All" means the user has asked for
        // every statistic and accepted the crowding — hiding a third of them would be the panel
        // lying about what it draws. "Median only" is one label per box and keeps the backstop,
        // which is what stops a wide category axis printing medians through each other.
        //
        // Unconditional until the LabelManager reached the bundle, when it cost 12 of 45 texts at
        // 640x400 and 26 at a dashboard card, with no way for the user to ask for the rest.
        labelLayout: { hideOverlap: labelMode !== "all" },
      }] : []),
    ],
  };
}
