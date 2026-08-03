import type { EChartsCoreOption } from "echarts/core";
import type { EChartsSize } from "../EChartsRenderer";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { estimatePlotWidth, fitXLabels, type XLabelFit } from "../axisLabelFit";
import { ariaDescription, baseOption, tooltipSurface } from "../base";
import { sortCategories } from "./cartesian";
import {
  CHART_THEME, SERIES_PALETTE, COLOR_FALLBACK, resolveColor, fadeColor, scaleLightness,
} from "../palette";

export interface ResultColumn { name: string; type: string }

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
  yTickCount?: number;
  yMin?: number;
  yMax?: number;
  logScale?: boolean;
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
   * The query is deliberately left alone. The backend WOULD honour an `orderby` here — `push_orderby`
   * runs in the raw-records branch too (`chartquery.rs`) — but a box plot's spec is raw rows under a
   * `row_limit`, and `ORDER BY category LIMIT n` returns the alphabetically-first n ROWS: whole
   * categories would vanish from the chart and the ones left would have truncated distributions.
   * Sorting here fixes the axis without touching which rows the quartiles are computed from.
   */
  const catType = ci != null ? columns[ci].type : "varchar";
  const categories = sortCategories([...groups.keys()], catType) as string[];
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
        // The categories as DRAWN: this axis has no `axisLabel.formatter`, so the raw group key is
        // the label — unlike the cartesian and waterfall axes, which format theirs.
        labels: categories,
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
    // A box is five numbers; the median is the one worth reading aloud per category.
    aria: ariaDescription({
      kind: "Box plot", metric: cfg.value, dimension: cfg.category, format: (v) => fmt(v),
      points: categories.map((c, i) => ({ name: String(c), value: stats[i]?.box?.[2] })),
    }),
    grid: {
      ...CHART_THEME.grid,
      bottom: (CHART_THEME.grid.bottom as number) + (xTitle ? 22 : 0),
      left: (CHART_THEME.grid.left as number) + (yTitle ? 26 : 0),
    },
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
          return `<div style="font-weight:600">${categories[i]}</div>`
            + row("Max", s.box[4]) + row("Q3", s.box[3]) + row("Median", s.box[2])
            + row("Q1", s.box[1]) + row("Min", s.box[0])
            + (showMean ? row("Mean", s.mean) : "");
        }
        // A point series datum is `[categoryIndex, value]`.
        const d = Array.isArray(p.data) ? p.data : undefined;
        const i = d ? Number(d[0]) : 0;
        return `<div style="font-weight:600">${categories[i] ?? ""}</div><div>${fmt(d ? d[1] : p.value)}</div>`;
      },
    },
    xAxis: {
      type: "category", data: categories,
      name: xTitle, nameLocation: "middle", nameGap: 28,
      nameTextStyle: { color: axisLabelColor },
      axisLabel: {
        fontSize: CHART_THEME.axisFontSize, color: axisLabelColor,
        interval: decided ? (0 as const) : ("auto" as const), rotate,
        // The third answer the ladder can give: when even a rotated label would eat 70% of the
        // card's height, Metabase draws no axis rather than a wall of text.
        show: fit?.kind !== "hide",
      },
      axisLine: { show: cfg.xAxisTicks !== false, lineStyle: { color: gridLine } },
      axisTick: { show: cfg.xAxisTicks !== false },
    },
    yAxis: {
      type: cfg.logScale ? "log" : "value",
      name: yTitle, nameLocation: "middle", nameGap: 44,
      nameTextStyle: { color: axisLabelColor },
      ...(cfg.yMin != null ? { min: cfg.yMin } : {}),
      ...(cfg.yMax != null ? { max: cfg.yMax } : {}),
      ...(cfg.yTickCount != null ? { splitNumber: cfg.yTickCount } : {}),
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
          show: true, position: "right", distance: MAX_BOX_WIDTH / 2 + 4,
          fontSize: CHART_THEME.labelFontSize, fontWeight: CHART_THEME.labelWeight, color: axisLabelColor,
          // Metabase's `graph.label_value_formatting` defaults to COMPACT for a box plot, and the
          // reason shows immediately: a median of 87.957 sits beside a box 47px tall. The axis
          // keeps the shared "compact once the numbers get wide" rule; only the labels differ,
          // which is how Metabase splits it too.
          formatter: (p: { data: number[] }) => fmt(p.data[1], true, cfg.compactValues ?? true),
        },
        labelLayout: { hideOverlap: true },
      }] : []),
    ],
  };
}
