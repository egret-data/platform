import type { EChartsCoreOption } from "echarts/core";
import type { CartesianViz, LineSeriesStyle } from "@/api/charts";
import type { EChartsSize } from "../EChartsRenderer";
import type { ColumnFormat } from "../../format-types";
import { isNumericType, isTemporalType } from "../../format-types";
import { formatValue, specifiesDateRendering } from "../../format";
import { ariaDescription, baseTextStyle, tooltipSurface } from "../base";
import { CHART_THEME, COLOR_FALLBACK, lineType, resolveColor, symbolShape } from "../palette";
import { colorsForKeys } from "../keyColors";
import { isExtractUnit, temporalLabel } from "@/components/chart/temporal-units";
import { boundarySteps, namesAPeriod, periodName } from "@/components/chart/temporal-period";
import { foldTail, OTHER_LABEL } from "../fold";
import {
  estimatePlotWidth, fitXLabels, intervalThatFits, nameGapClearOf, plotWidthLessGutter, widestLabelWidth,
  type XLabelFit,
} from "../axisLabelFit";

export interface ResultColumn { name: string; type: string }
export interface CartesianBuildConfig {
  kind: CartesianViz;
  xAxis?: string;
  xMetric?: string;        // scatter only: a metric on the x-axis (metric-vs-metric)
  yMetrics: string[];
  series?: string;
  stacked?: boolean;
  horizontal?: boolean;
  showLabels?: boolean;
  showLegend?: boolean;
  xLabel?: string;
  yLabel?: string;
  seriesTypes?: Record<string, SeriesDisplay>;   // series name -> how it is drawn (combo)
  rightAxisMetrics?: string[];   // metrics plotted against a second (right) Y axis
  /** Metabase's "Split y-axis when necessary". Unset means on, as it is there. Only consulted when
   *  `rightAxisMetrics` is empty — an explicit assignment always wins over a computed one. */
  /** Scatter only: the metric whose value sizes each bubble. */
  bubbleMetric?: string;
  autoSplitY?: boolean;
  /** Metabase's "Show label", one per axis, separate from the label TEXT. Unset means shown. Kept
   *  apart from the text so hiding a title does not make the user retype it to bring it back. */
  showXLabel?: boolean;
  showYLabel?: boolean;
  logScale?: boolean;            // value axis uses a log scale
  normalized?: boolean;          // 100% stacking — each category's values sum to 100
  goalValue?: number;            // horizontal reference (goal) line
  goalLabel?: string;
  trendLine?: boolean;           // overlay a least-squares trend line per series
  showStackTotal?: boolean;      // label the total atop each stacked column
  seriesColors?: Record<string, string>;  // series name -> colour override (Metabase series settings)
  seriesLabels?: Record<string, string>;  // series name -> display label override
  yMin?: number;                 // manual value-axis minimum (auto when undefined)
  yMax?: number;                 // manual value-axis maximum
  xScale?: "ordinal" | "linear" | "timeseries";  // x-axis scale (default ordinal/category)
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
  /**
   * The temporal unit the x-axis dimension was grouped by, from `params.spec.temporal_units`.
   * EITHER KIND, and the two are read at different places — this said "extraction unit (e.g.
   * `day_of_week`)" while that was all it drove, and has been stale since a truncation unit started
   * driving the period naming beneath it:
   *   - an EXTRACTION unit (`day_of_week`, `month_of_year`) is substituted into the categories
   *     themselves through `temporalLabel` (`displayCats`), so the axis never sees the raw integer;
   *   - a TRUNCATION unit (`month`, `quarter`, …) leaves the categories alone and reaches
   *     `drawLabel`, which names the period the bucket covers, and `boundary`, which is the only
   *     door to thinning.
   * `WaterfallBuildConfig.xAxisUnit` is the same field fed from the same line of `ChartRenderer`,
   * with one difference stated there: that builder does not substitute extraction units.
   */
  xAxisUnit?: string;
  xAxisType?: string;            // SQL type of the x-axis dimension, so its labels can be formatted
  /** Metabase's "Show lines and tick marks", per axis. Unset means shown, which is what every
   *  chart drew before the toggles existed. */
  xAxisTicks?: boolean;
  yAxisTicks?: boolean;
  /** An explicit x-label rotation, which SUPPRESSES the measured choice — Metabase's
   *  auto-selection likewise applies only while the setting is untouched. Same field name and
   *  values waterfall and box plot already store (`options/waterfall.ts`, `options/boxplot.ts`).
   *  The panel control that writes it is Task 3; this is the field the builder reads. */
  xLabelRotation?: 45 | 90;
  /** Metabase's "Number of tick marks" on the value axis. Unset lets ECharts choose. */
  yTickCount?: number;
  seriesUnit?: string;           // temporal unit of the BREAKOUT dimension, same role as xAxisUnit
  seriesType?: string;           // SQL type of the breakout dimension, so its labels can be formatted
  /** Short value-axis labels (25.5K). Defaults to on once the numbers are wide. */
  compactValues?: boolean;
  /** Fold all but the largest `maxCategories - 1` into "Other". Unset keeps every category. */
  maxCategories?: number;
  /** Series names in display order; any not listed keep their natural order after them. */
  seriesOrder?: string[];
  /** Series names to leave out of the chart entirely. */
  hiddenSeries?: string[];
  /** Data-label density: "fit" drops labels that would collide, "all" draws every one. */
  labelDensity?: "fit" | "all";
  /** Line shape. One choice, because a line cannot be both smoothed and stepped. */
  lineShape?: "linear" | "smooth" | "step";
  /** What a null becomes: bridged, dropped to zero, or left as a break in the line. */
  missing?: "interpolate" | "zero" | "none";
  /** Point markers. "auto" shows them only when there are few enough points to read. */
  markers?: "auto" | "on" | "off";
  /** Per-series overrides of the chart-wide line appearance, keyed by series name. */
  seriesLine?: Record<string, LineSeriesStyle>;
}

// The series names a cartesian chart will render — its y-metrics, or the distinct
// breakout values (null/empty folded to "(empty)", matching the renderer). Used by the
// settings panel to list series for per-series colour / rename without rebuilding the chart.
export function cartesianSeriesNames(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: { yMetrics: string[]; xAxis?: string; series?: string },
): string[] {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  if (cfg.series != null && idx[cfg.series] != null) {
    const gi = idx[cfg.series];
    const xi = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
    const valid = xi == null ? rows : rows.filter((r) => r[xi] != null);
    return distinct(valid.map((r) => r[gi])).map((sv) => (sv == null || sv === "" ? "(empty)" : String(sv)));
  }
  return [...cfg.yMetrics];
}

const BAR_MIN_HEIGHT = 1; // keep sub-pixel bars visible, like Metabase's barMinHeight
/** Above this many points on one line, markers merge into the line rather than marking it. */
const MARKER_AUTO_MAX = 30;

/** Metabase's S/M/L line sizes. M is the theme's own width, so an unset size renders unchanged.
 * S is 1.5, not ECharts' own 1px default — gridlines/axis lines already render at 1px, and a
 * data line that weighs the same as the chrome around it undercuts the point of drawing it. */
const LINE_WIDTHS = { S: 1.5, M: CHART_THEME.lineWidth, L: 4 } as const;

interface SeriesLike { name?: string; data: (number | null)[]; yAxisIndex?: number; itemStyle?: { color?: string }; __trend?: boolean; __total?: boolean; __metric?: string; __markers?: "auto" | "on" | "off" }

/**
 * Markers stop reading as points and start reading as a lumpy line once they touch. Applied after
 * folding and x-scaling, because "auto" is a question about the points that end up drawn — and it
 * counts only the ones that exist: a null draws nothing, so a sparse series is not crowded.
 */
function applyMarkers(series: SeriesLike[]): void {
  for (const s of series) {
    if (s.__markers == null) continue;
    const drawn = (s.data ?? []).filter((v) => v != null).length;
    (s as unknown as Record<string, unknown>).showSymbol =
      s.__markers === "on" || (s.__markers === "auto" && drawn <= MARKER_AUTO_MAX);
  }
}

// A transparent overlay bar whose top label shows each column's stack total —
// clean-room of Metabase's getStackTotalsSeries.
function stackTotalSeries(series: SeriesLike[], cfg: CartesianBuildConfig): SeriesLike {
  const values = series.filter((s) => !s.__trend && !s.__total);
  const n = values[0]?.data.length ?? 0;
  const totals: (number | null)[] = [];
  for (let i = 0; i < n; i++) {
    let t = 0; let any = false;
    for (const s of values) { const v = s.data[i]; if (typeof v === "number") { t += v; any = true; } }
    totals.push(any ? t : null);
  }
  const m = cfg.yMetrics[0];
  const fmt = (v: number) => (m ? formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]) : String(v));
  return {
    data: totals,
    __total: true,
    ...{ type: "bar", barGap: "-100%", barWidth: CHART_THEME.barWidth, silent: true, legendHoverLink: false, tooltip: { show: false },
      itemStyle: { color: "transparent" }, emphasis: { itemStyle: { color: "transparent" } },
      label: { show: true, position: "top", fontSize: CHART_THEME.labelFontSize, fontWeight: CHART_THEME.labelWeight,
        formatter: (p: { value: unknown }) => (p.value == null ? "" : fmt(Number(p.value))) } },
  } as SeriesLike;
}

// Least-squares fit y = m·i + b over the non-null points, evaluated at every index.
function trendData(data: (number | null)[]): (number | null)[] | null {
  const pts: [number, number][] = [];
  data.forEach((v, i) => { if (typeof v === "number") pts.push([i, v]); });
  if (pts.length < 2) return null;
  const n = pts.length;
  const sx = pts.reduce((s, p) => s + p[0], 0);
  const sy = pts.reduce((s, p) => s + p[1], 0);
  const sxy = pts.reduce((s, p) => s + p[0] * p[1], 0);
  const sxx = pts.reduce((s, p) => s + p[0] * p[0], 0);
  const denom = n * sxx - sx * sx;
  if (denom === 0) return null;
  const m = (n * sxy - sx * sy) / denom;
  const b = (sy - m * sx) / n;
  return data.map((_, i) => m * i + b);
}

// One dashed trend series per source series, sharing its axis + color, kept out of the legend.
function trendSeries(series: SeriesLike[]): SeriesLike[] {
  const out: SeriesLike[] = [];
  for (const s of series) {
    const fit = trendData(s.data);
    if (!fit) continue;
    out.push({
      name: `${s.name ?? ""} trend`,
      data: fit,
      yAxisIndex: s.yAxisIndex ?? 0,
      __trend: true,
      ...{ type: "line", symbol: "none", smooth: false, silent: true, legendHoverLink: false, z: 3,
        lineStyle: { type: "dashed", width: 1.5, color: s.itemStyle?.color },
        tooltip: { show: false } },
    } as SeriesLike);
  }
  return out;
}

/**
 * Metabase's `graph.series_order`: filters out hidden series and reorders the rest.
 *
 * Called before the overlays are built, so anything it drops is never counted in a stack
 * total, never normalised against, and leaves no orphaned trend line — but that guarantee
 * only holds as far downstream as every consumer keys off the series that reach it (see the
 * axis-shape logic in baseOption, which had to be re-derived from the surviving series
 * rather than from cfg for the same reason). Order matters beyond the legend: it is also the
 * stacking order and the draw order.
 */
function applySeriesOrder(series: SeriesLike[], cfg: CartesianBuildConfig): SeriesLike[] {
  const hidden = cfg.hiddenSeries;
  const order = cfg.seriesOrder;
  if (!hidden?.length && !order?.length) return series;
  const out = hidden?.length ? series.filter((s) => !hidden.includes(s.name ?? "")) : [...series];
  if (order?.length) {
    const rank = new Map(order.map((n, i) => [n, i]));
    // A name that is no longer plotted simply ranks nowhere; anything the stored order does
    // not mention keeps its natural position after the ones it does.
    out.sort(
      (a, b) =>
        (rank.get(a.name ?? "") ?? Number.MAX_SAFE_INTEGER) -
        (rank.get(b.name ?? "") ?? Number.MAX_SAFE_INTEGER),
    );
  }
  return out;
}

/**
 * Whether this chart stacks, resolving the default its KIND carries.
 *
 * An area chart stacks unless told otherwise — Metabase's own default, and for a reason its
 * measurement shows: overlapping translucent areas at 0.3 opacity read as mud, so the shape only
 * works stacked. Bar and line default to unstacked, as they do there.
 *
 * `undefined` is genuinely distinct from `false` here: `cartesianToSettings` drops undefined keys,
 * so a chart whose Stacking control was never touched stores nothing and takes the kind's default,
 * while one explicitly set to "Don't stack" stores `false` and keeps it.
 */
export function resolveStacked(cfg: Pick<CartesianBuildConfig, "kind" | "stacked">): boolean {
  return cfg.stacked ?? cfg.kind === "area";
}

/** The three mutually exclusive stacking states Metabase offers, as one value. */
export type StackMode = "none" | "stacked" | "normalized";

export function stackModeOf(cfg: Pick<CartesianBuildConfig, "kind" | "stacked" | "normalized">): StackMode {
  if (!resolveStacked(cfg)) return "none";
  return cfg.normalized ? "normalized" : "stacked";
}

/** The config patch a chosen mode writes. Two booleans encode three states, so the pair has to be
 *  set together — leaving `normalized` behind when switching to "Don't stack" would store a
 *  combination no control can show. */
export function stackModePatch(mode: StackMode): { stacked: boolean; normalized: boolean | undefined } {
  return {
    stacked: mode !== "none",
    normalized: mode === "normalized" ? true : undefined,
  };
}

const isNormalized = (cfg: CartesianBuildConfig) => !!cfg.normalized && resolveStacked(cfg);

// 100% stacking — rewrite each category column so its series values sum to 100.
function normalizeStacks(series: { data: (number | null)[] }[]) {
  const n = series[0]?.data.length ?? 0;
  for (let i = 0; i < n; i++) {
    let total = 0;
    for (const s of series) { const v = s.data[i]; if (typeof v === "number") total += v; }
    for (const s of series) {
      const v = s.data[i];
      s.data[i] = typeof v === "number" ? (total > 0 ? (v / total) * 100 : 0) : null;
    }
  }
}

/** A metric's [min, max] over the drawn rows. */
type Extent = [number, number];

// Right-axis is active only when it splits the metrics (≥1 left and ≥1 right) on a
// vertical chart — clean-room of Metabase's left/right axis split.
function rightMetrics(cfg: CartesianBuildConfig): string[] {
  return (cfg.rightAxisMetrics ?? []).filter((m) => cfg.yMetrics.includes(m));
}

/** Per-metric [min, max] over the drawn rows. The split decision needs one range per metric — the
 *  granularity egret's own right-axis control uses, so a computed split is a value that control
 *  could equally have produced by hand. */
function metricExtents(columns: ResultColumn[], rows: unknown[][], metrics: string[]): Record<string, Extent> {
  const out: Record<string, Extent> = {};
  for (const m of metrics) {
    const i = columns.findIndex((c) => c.name === m);
    if (i < 0) continue;
    let lo = Infinity;
    let hi = -Infinity;
    for (const r of rows) {
      const v = Number(r[i]);
      if (!Number.isFinite(v)) continue;
      if (v < lo) lo = v;
      if (v > hi) hi = v;
    }
    if (Number.isFinite(lo) && Number.isFinite(hi)) out[m] = [lo, hi];
  }
  return out;
}

/** Metabase's `KEYS_TO_COMPARE` verbatim — the format fields that make two metrics incomparable on
 *  one scale. Every one of them exists on egret's ColumnFormat under the same name. */
const KEYS_TO_COMPARE = [
  "number_style", "currency", "currency_style", "number_separators", "decimals", "scale",
  "prefix", "suffix",
] as const;

function comparisonSettings(fmt: ColumnFormat | undefined): string {
  if (!fmt) return "";
  const picked: Record<string, unknown> = {};
  for (const k of KEYS_TO_COMPARE) {
    const v = fmt[k];
    if (v === undefined) continue;
    // Metabase drops an EMPTY prefix/suffix before comparing, so "" and unset are the same thing.
    if ((k === "prefix" || k === "suffix") && v === "") continue;
    picked[k] = v;
  }
  return JSON.stringify(picked);
}

/**
 * Whether the y-axis should split itself — Metabase's `shouldAutoSplitYAxis`, ported.
 *
 * Its own note on the last rule, kept because it is the reason for the number: "When the series
 * with the smallest range is less than 5% of the chart's total range, we split the y-axis so it
 * doesn't look too small."
 *
 * The `semantic_type` and multi-card clauses have no egret equivalent — egret plots one dataset and
 * carries no semantic types — so they are dropped rather than approximated. The format clause does
 * translate exactly: two metrics formatted as different currencies, or one scaled and one not, do
 * not belong on one scale however close their ranges are.
 */
export function shouldAutoSplitY(cfg: CartesianBuildConfig, extents: Record<string, Extent>): boolean {
  if (cfg.autoSplitY === false) return false;
  // A stack answers a different question — the parts of one total — so a second scale would make
  // the stack meaningless. Metabase bails on `stackable.stack_type` for the same reason.
  if (resolveStacked(cfg)) return false;
  if (isHorizontal(cfg)) return false;
  const metrics = cfg.yMetrics.filter((m) => extents[m] != null);
  if (metrics.length < 2) return false;

  const fmts = metrics.map((m) => comparisonSettings(cfg.columnSettings?.[m]));
  if (new Set(fmts).size > 1) return true;

  const ranges = metrics.map((m) => extents[m][1] - extents[m][0]);
  const minRange = Math.min(...ranges);
  const chartRange = Math.max(...metrics.map((m) => extents[m][1])) - Math.min(...metrics.map((m) => extents[m][0]));
  if (chartRange === 0) return false;
  return minRange / chartRange <= 0.05;
}

// Metabase's own constants, verbatim.
const SPLIT_AXIS_UNSPLIT_COST = -100;
const SPLIT_AXIS_COST_FACTOR = 2;
const SPLIT_AXIS_MAX_DEPTH = 8;

/** Every partition of `list` into two sides. Past the depth cap the remainder goes to the smaller
 *  side in one piece, so the search stays bounded without dropping a metric. */
function generateSplits(list: string[], left: string[] = [], right: string[] = [], depth = 0): [string[], string[]][] {
  if (list.length === 0) return [[left, right]];
  if (depth > SPLIT_AXIS_MAX_DEPTH) {
    return left.length < right.length ? [[left.concat(list), right]] : [[left, right.concat(list)]];
  }
  return [
    ...generateSplits(list.slice(1), left.concat([list[0]]), right, depth + 1),
    ...generateSplits(list.slice(1), left, right.concat([list[0]]), depth + 1),
  ];
}

/** How badly one axis serves the metrics on it: the square of how much of the axis each metric
 *  fails to use. An empty side scores -100 while unsplit is still favoured, which is what keeps a
 *  chart on one axis unless splitting genuinely pays. */
function axisCost(extents: Extent[], favorUnsplit = true): number {
  if (favorUnsplit && extents.length === 0) return SPLIT_AXIS_UNSPLIT_COST;
  const lo = Math.min(...extents.map((e) => e[0]));
  const hi = Math.max(...extents.map((e) => e[1]));
  const axisRange = hi - lo;
  if (axisRange === 0) return 0;
  return extents.reduce((sum, e) => {
    const r = e[1] - e[0];
    return sum + (r === 0 ? 0 : Math.pow(axisRange / r, SPLIT_AXIS_COST_FACTOR));
  }, 0);
}

/** The cheapest partition. Metabase's `computeSplit`, ported. Returns the RIGHT side, which is what
 *  egret's `rightAxisMetrics` names. */
export function computeAxisSplit(metrics: string[], extents: Record<string, Extent>): string[] {
  const usable = metrics.filter((m) => extents[m] != null);
  let best: [string[], string[]] | null = null;
  let bestCost = Infinity;
  for (const split of generateSplits(usable)) {
    const c = axisCost(split[0].map((m) => extents[m])) + axisCost(split[1].map((m) => extents[m]));
    if (!best || c < bestCost) { best = split; bestCost = c; }
  }
  return best?.[1] ?? [];
}
function isDualAxis(cfg: CartesianBuildConfig): boolean {
  const right = rightMetrics(cfg);
  return !isHorizontal(cfg) && right.length > 0 && right.length < cfg.yMetrics.length;
}

// A row chart is a horizontal bar chart.
const isHorizontal = (cfg: CartesianBuildConfig) =>
  cfg.kind === "row" || (!!cfg.horizontal && cfg.kind !== "scatter");

// A non-ordinal x-scale (linear/timeseries) positions points by their real x value
// instead of evenly-spaced categories — only on a vertical chart.
const usesXScale = (cfg: CartesianBuildConfig) =>
  !isHorizontal(cfg) && (cfg.xScale === "linear" || cfg.xScale === "timeseries");

/**
 * The x-scale a chart is DRAWN at, which is the stored one whenever there is one and otherwise
 * `linear` for a scatter whose x column holds numbers.
 *
 * Why: a scatter over `price` plotted 196 distinct prices as 196 CATEGORIES, evenly spaced and each
 * carrying its own label. ECharts kept whichever of those labels did not collide — three arbitrary
 * prices, `15.69 48.93 84.14` — and once PR 02 made the axis all-or-nothing it drew none at all.
 * Metabase draws `0 20 40 60 80 100` there, and not because it thins better: its x is a VALUE axis,
 * so the labels are round ticks ECharts generates from the extent rather than a subset of the data.
 * egret has had that axis all along (`usesXScale` -> `scaleAxis`); nothing ever selected it.
 *
 * SCATTER ONLY, and that is a rule rather than a starting point. A bar chart's categories are
 * discrete even when they are numbers — bar width, grouping and stacking are all computed off the
 * axis type — so putting bars on a value axis is a much larger change than this defect asks for.
 * A scatter has no such geometry: a point sits where its coordinates say.
 *
 * RESOLVED AT RENDER TIME, never written into the stored config. That is this programme's standing
 * rule and it is what makes a scatter saved before this change pick the new axis up. It also means
 * the answer moves with the data: re-point the chart at a varchar column and the category axis
 * comes back on its own.
 *
 * An explicit `xScale` — including `"ordinal"` — always wins, so a user who has said what they want
 * is never overridden. The panel offers that choice on a dimension-x scatter for exactly this
 * reason (`settings/CartesianDisplaySection.tsx`, `AxesOptions`): an automatic answer with no lever
 * beside it is not a default, it is a rule.
 *
 * ONE KNOWN EDGE, PRE-EXISTING BUT NEWLY REACHABLE. `isNumericType` matches `bigint` and `long`,
 * and a value axis coerces with `Number()` (`applyXScale`), which is a float64 — so a scatter over
 * an id column with values above 2^53 plots and drills on a rounded id. That has been true of a
 * hand-set Linear scale since it existed; what is new is that such a column reaches it without
 * anyone choosing. Narrowing the numeric test to exclude 64-bit integer types would also exclude
 * the ordinary bigint counts that make perfectly good scatter axes, so it is left alone and named
 * here rather than silently traded away.
 */
export function resolveXScale(
  kind: CartesianViz,
  xScale: CartesianBuildConfig["xScale"],
  xColumnType: string | undefined,
): CartesianBuildConfig["xScale"] {
  if (xScale != null) return xScale;
  if (kind !== "scatter") return undefined;
  return xColumnType != null && isNumericType(xColumnType) ? "linear" : undefined;
}

// Re-key category-aligned series onto real x coordinates: each value becomes an
// [x, y] pair, dropping points whose x can't be parsed. Mutates the series in place.
function applyXScale(categories: unknown[], series: unknown[], cfg: CartesianBuildConfig) {
  const coord = (c: unknown) => (cfg.xScale === "timeseries" ? Date.parse(String(c)) : Number(c));
  const xs = categories.map(coord);
  for (const s of series as { data: (number | null)[] }[]) {
    s.data = s.data
      .map((y, i) => [xs[i], y] as unknown as number | null)
      .filter((p) => Number.isFinite((p as unknown as [number, unknown])[0]));
  }
}

// ECharts series type for a metric under the active kind.
/** A breakout series' value as it should READ — its column's format, if the column has one and the
 *  value is a real column value rather than an extraction unit's finished word ("Wed"). */
function seriesDisplayName(cfg: CartesianBuildConfig, raw: string): string {
  const col = cfg.series;
  if (col == null || isExtractUnit(cfg.seriesUnit)) return raw;
  const fmt = cfg.columnSettings?.[col];
  if (!fmt) return raw;
  return formatValue(raw, cfg.seriesType ?? "varchar", fmt) || raw;
}

/** What a series is DRAWN AS. "area" is not an ECharts series type — it is a line with a fill — so
 *  it stays separate from the `type` the option carries (see `echartsTypeOf`). */
export type SeriesDisplay = "bar" | "line" | "area";

/**
 * A combo series' display, keyed by the SERIES name — the same key space as its colour and its
 * rename, so one row of the panel can carry all of them. It used to be keyed by metric, which a
 * breakout has only one of: every band of a broken-out combo was forced to share one display.
 *
 * The default is Metabase's, verbatim (`getSeriesDefaultDisplay`): `index === 0 ? "line" : "bar"`.
 * egret defaulted every series to "bar", so a fresh combo was indistinguishable from a bar chart
 * until the user changed something — the one thing the chart type exists to show was off by
 * default.
 */
export function seriesDisplayFor(
  kind: CartesianViz, seriesName: string, index: number, seriesTypes?: Record<string, SeriesDisplay>,
): SeriesDisplay | "scatter" {
  if (kind === "combo") return seriesTypes?.[seriesName] ?? (index === 0 ? "line" : "bar");
  if (kind === "area") return "area";
  if (kind === "scatter") return "scatter";
  if (kind === "row") return "bar";
  return kind; // "bar" | "line"
}

/** The ECharts series type behind a display. An area is a line that happens to be filled. */
function echartsTypeOf(display: SeriesDisplay | "scatter"): "bar" | "line" | "scatter" {
  return display === "area" ? "line" : display;
}

const distinct = (vals: unknown[]) => {
  const seen = new Set<string>();
  const out: unknown[] = [];
  for (const v of vals) { const k = String(v); if (!seen.has(k)) { seen.add(k); out.push(v); } }
  return out;
};

// Order the x categories by axis type — temporal chronologically, numeric ascending —
// while leaving a nominal (string) axis in first-seen order. (Metabase model/dataset.ts.)
function sortCategories(cats: unknown[], type: string): unknown[] {
  if (isTemporalType(type)) return [...cats].sort((a, b) => Date.parse(String(a)) - Date.parse(String(b)));
  if (isNumericType(type)) return [...cats].sort((a, b) => Number(a) - Number(b));
  return cats;
}

// Metric-vs-metric scatter (Metabase's true scatter): each dimension row is a point
// at (xMetric, yMetric) on two value axes.
// Metabase's bubble scale (scatter/option/series.ts), verbatim including its rationale: the size
// interpolates by AREA, not by radius, because a radius-linear scale exaggerates large values —
// doubling the radius quadruples the ink. `d = 2·√(area/π)`.
const MIN_BUBBLE_DIAMETER = 15;
const MAX_BUBBLE_DIAMETER = 75;

/** Diameter for a bubble value, given the metric's extent. Returns the floor when there is nothing
 *  to scale against — a single point, or an extent of width zero. */
export function bubbleDiameter(value: number, lo: number, hi: number): number {
  if (!Number.isFinite(value) || !(hi > lo)) return MIN_BUBBLE_DIAMETER;
  const t = Math.min(1, Math.max(0, (value - lo) / (hi - lo)));
  const areaLo = Math.PI * (MIN_BUBBLE_DIAMETER / 2) ** 2;
  const areaHi = Math.PI * (MAX_BUBBLE_DIAMETER / 2) ** 2;
  return 2 * Math.sqrt((areaLo + t * (areaHi - areaLo)) / Math.PI);
}

// Metric-vs-metric scatter (Metabase's true scatter): each dimension row is a point
// at (xMetric, yMetric) on two value axes.
function scatterXY(columns: ResultColumn[], rows: unknown[][], cfg: CartesianBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const di = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
  const xi = idx[cfg.xMetric!];
  const yMetric = cfg.yMetrics[0];
  const yi = idx[yMetric];
  const gi = cfg.series != null ? idx[cfg.series] : undefined;
  const bi = cfg.bubbleMetric != null ? idx[cfg.bubbleMetric] : undefined;
  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const fmtM = (m: string, v: unknown) => formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]);

  interface Pt { value: number[]; name: string; bubble?: number }
  const points: (Pt & { group: string })[] = [];
  for (const r of rows) {
    const x = Number(r[xi]);
    const y = Number(r[yi]);
    if (!Number.isFinite(x) || !Number.isFinite(y)) continue;
    const b = bi != null ? Number(r[bi]) : undefined;
    points.push({
      value: [x, y],
      name: di != null ? String(r[di] ?? "") : "",
      ...(b != null && Number.isFinite(b) ? { bubble: b } : {}),
      group: gi != null ? String(r[gi] ?? "") : "",
    });
  }

  // The bubble extent spans EVERY point, not each series' own — otherwise the same value would
  // draw at two sizes depending on which breakout band it landed in.
  const bubbles = points.map((p) => p.bubble).filter((v): v is number => v != null);
  const bLo = bubbles.length ? Math.min(...bubbles) : 0;
  const bHi = bubbles.length ? Math.max(...bubbles) : 0;
  // Total on purpose: ECharts calls this per symbol during layout, and anything thrown in there
  // takes the whole series down silently — the chart keeps its axes and simply draws no points.
  const sizeOf = (p: Pt | undefined) => {
    const v = p?.bubble;
    return bi == null || v == null ? CHART_THEME.scatterSymbolSize : bubbleDiameter(v, bLo, bHi);
  };

  const groups = cfg.series != null ? distinct(points.map((p) => p.group)).map(String) : [""];
  // Keyed by GROUP NAME so a breakout value keeps its colour across every card that breaks out by
  // it — and so an explicit `seriesColors` entry consumes its colour rather than shadowing it.
  //
  // An un-broken-out scatter has one group whose name is the empty string, and hashing THAT would
  // give every such card the same colour whatever it plots. It is keyed by the y-metric instead —
  // which is also the name the series is labelled with two lines below, and the key
  // `cartesianSeriesNames` hands the settings panel, so a pinned colour now actually finds its
  // series. Keying the lookup on "" meant `cfg.seriesColors` could never match on this path.
  const groupKey = (g: string) => (g === "" ? yMetric : g);
  const groupColors = colorsForKeys(groups.map(groupKey), cfg.seriesColors);
  const series = groups.map((g) => {
    const own = points.filter((p) => p.group === g);
    const color = groupColors[groupKey(g)];
    return {
      type: "scatter" as const,
      name: cfg.seriesLabels?.[g] ?? (g === "" ? yMetric : g),
      __raw: g,
      itemStyle: { color, opacity: CHART_THEME.scatterOpacity },
      symbolSize: (_v: unknown, params?: { data?: Pt }) => sizeOf(params?.data),
      // The same lift-and-recede the rest of the family got; a scatter with several bands needs it
      // most, since its points overlap by nature.
      emphasis: { focus: "series" as const },
      blur: { itemStyle: { opacity: CHART_THEME.blurOpacity } },
      data: own,
    };
  });

  return {
    grid: { ...CHART_THEME.grid, bottom: (CHART_THEME.grid.bottom as number) + 22 },
    tooltip: {
      trigger: "item", ...tooltipSurface(),
      formatter: (p: { data: Pt; value: number[] }) => {
        const head = p.data.name ? `${p.data.name}<br/>` : "";
        const bubble = cfg.bubbleMetric != null && p.data.bubble != null
          ? `<br/>${cfg.bubbleMetric}: ${fmtM(cfg.bubbleMetric, p.data.bubble)}`
          : "";
        return `${head}${cfg.xMetric}: ${fmtM(cfg.xMetric!, p.value[0])}<br/>${yMetric}: ${fmtM(yMetric, p.value[1])}${bubble}`;
      },
    },
    ...(groups.length > 1 ? { legend: { type: "scroll", bottom: 0, textStyle: { color: axisLabelColor } } } : {}),
    xAxis: { type: "value", name: cfg.xMetric, nameLocation: "middle", nameGap: 28, axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: (v: number) => fmtM(cfg.xMetric!, v) }, splitLine: { lineStyle: { color: gridLine } } },
    yAxis: { type: "value", name: yMetric, axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: (v: number) => fmtM(yMetric, v) }, splitLine: { lineStyle: { color: gridLine } } },
    series,
  };
}

export function buildCartesianOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg0: CartesianBuildConfig,
  /** The chart's measured pixel size, when the caller has one — this builder is pure data with no
   * DOM of its own, so EChartsRenderer measures the container and threads it through (see
   * `buildPieOption`, the first to take one).
   *
   * Read by the x-axis label fit (`baseOption` -> `fitXLabels`): whether the category labels fit
   * is a question about WIDTH, which this builder could not see until this parameter existed —
   * the rule it replaced guessed from label count and length (`longest > 10 &&
   * categories.length > 6`), which are proxies for width, not width.
   *
   * Undefined means the caller has no measurement (every unit test, `cartesianMeta`, and any
   * caller passing a plain option object rather than a size function); that keeps the size-blind
   * behaviour, since a decision made against a guessed width is worse than one that declines to
   * guess. `__tests__/cartesianXLabels.test.ts` pins both halves. */
  size?: EChartsSize,
): EChartsCoreOption {
  if (cfg0.kind === "scatter" && cfg0.xMetric && cfg0.yMetrics.length > 0) {
    return scatterXY(columns, rows, cfg0);
  }
  // Resolve the x-scale ONCE, here, on the same principle as the axis split below it: every
  // downstream reader — `usesXScale`, `applyXScale`, `foldable`, `ordinalX`, `scaleAxis` — goes on
  // asking `cfg.xScale` exactly as it always did, and a computed scale is indistinguishable from
  // one the user chose. Read off the RESULT column rather than `cfg.xAxisType`, which is the same
  // type by a longer route (`ChartRenderer` looks it up in this very list) but is optional and
  // absent from every caller that builds a config by hand.
  //
  // Below the `scatterXY` early return, because above it would be wasted: a scatter with an
  // `xMetric` AND at least one y-metric leaves through that return and builds its own pair of value
  // axes, so the common metric-vs-metric case never reads this. Not "unreachable" — an `xMetric`
  // scatter with NO y-metric fails that return's second condition and arrives here, where it has no
  // series to draw anyway and leaves through the empty-option branch a few lines down. What this
  // line is really FOR is the scatter whose x is a DIMENSION: the one that was drawing 196
  // categories.
  const xColumn = cfg0.xAxis != null ? columns.find((c) => c.name === cfg0.xAxis) : undefined;
  const xScale = resolveXScale(cfg0.kind, cfg0.xScale, xColumn?.type);
  const cfgX: CartesianBuildConfig = xScale === cfg0.xScale ? cfg0 : { ...cfg0, xScale };
  // Resolve the automatic axis split ONCE, here, and hand the rest of the builder a config that
  // already names the right-axis metrics. Every downstream reader — `isDualAxis`, the per-series
  // `yAxisIndex`, the axis titles — then goes on asking the same question it always did, and a
  // computed split is indistinguishable from one the user assigned by hand.
  const cfg: CartesianBuildConfig = (cfgX.rightAxisMetrics ?? []).length > 0
    ? cfgX
    : (() => {
        const extents = metricExtents(columns, rows, cfgX.yMetrics);
        if (!shouldAutoSplitY(cfgX, extents)) return cfgX;
        const right = computeAxisSplit(cfgX.yMetrics, extents);
        // A split that puts everything on one side is not a split.
        return right.length > 0 && right.length < cfgX.yMetrics.length
          ? { ...cfgX, rightAxisMetrics: right }
          : cfgX;
      })();
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const xi = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
  const fmt = (metric: string, v: unknown) =>
    formatValue(v, cfg.metricTypes?.[metric] ?? "double", cfg.columnSettings?.[metric]);

  if (xi == null || cfg.yMetrics.length === 0) {
    return baseOption(cfg, [], [], size);
  }

  // Drop rows with a null x value, then order categories by the x column's type.
  const xType = columns[xi].type;
  const xrows = rows.filter((r) => r[xi] != null);
  const catKey = (v: unknown) => String(v);
  let categories = sortCategories(distinct(xrows.map((r) => r[xi])), xType);


  // Breakout: one ECharts series per distinct series value (first y-metric only).
  if (cfg.series != null && idx[cfg.series] != null) {
    const gi = idx[cfg.series];
    const metric = cfg.yMetrics[0];
    const mi = idx[metric];
    const seriesValues = distinct(xrows.map((r) => r[gi]));
    const cell = new Map(xrows.map((r) => [`${catKey(r[xi])} ${catKey(r[gi])}`, r[mi] as number | null]));
    // Grouped bars (>1 series, not stacked) must auto-size to sit side by side.
    // How many of these will be drawn as bars decides whether bars must sit side by side. With a
    // per-series display the answer is no longer one lookup — a combo can mix them.
    const barCount = seriesValues.filter((_, si) => seriesDisplayFor(cfg.kind, "", si, cfg.seriesTypes) === "bar").length;
    const grouped = !resolveStacked(cfg) && barCount > 1;
    // Metabase parity: null / empty-string breakout values render as "(empty)".
    const groupName = (sv: unknown) => (sv == null || sv === "" ? "(empty)" : String(sv));
    // Keyed by GROUP NAME so a breakout value keeps its colour across every card that breaks out by
    // it — a `Gadget` band is the same yellow whether the card also plots `Widget` or not, and
    // whether it is drawn second or fourth. An explicit `seriesColors` entry is folded in rather
    // than layered on top, so it CONSUMES its colour and the series that would have hashed onto it
    // moves along instead of being drawn identically.
    const groupColors = colorsForKeys(seriesValues.map(groupName), cfg.seriesColors);
    const builtSeries = seriesValues.map((sv, si) => {
      const data = categories.map((c) => {
        const v = cell.get(`${catKey(c)} ${catKey(sv)}`);
        return v === undefined ? null : v;
      });
      return decorateSeries({ name: groupName(sv), data }, metric, si, cfg, fmt, groupColors, grouped);
    });
    const series = applySeriesOrder(builtSeries as unknown as SeriesLike[], cfg) as unknown as typeof builtSeries;
    if (isNormalized(cfg)) normalizeStacks(series as unknown as { data: (number | null)[] }[]);
    if (cfg.trendLine) series.push(...(trendSeries(series as unknown as SeriesLike[]) as unknown as typeof series));
    if (resolveStacked(cfg) && cfg.showStackTotal && !isNormalized(cfg)) series.push(stackTotalSeries(series as unknown as SeriesLike[], cfg) as unknown as (typeof series)[number]);
    if (usesXScale(cfg)) applyXScale(categories, series, cfg);
    applyMarkers(series as unknown as SeriesLike[]);
    const displayCats = isExtractUnit(cfg.xAxisUnit) ? categories.map((v) => temporalLabel(cfg.xAxisUnit, v)) : categories;
    return baseOption(cfg, displayCats as (string | number)[], series, size);
  }

  const rowByCat = new Map(xrows.map((r) => [catKey(r[xi]), r]));
  const barMetrics = cfg.yMetrics.filter((m, si) => seriesDisplayFor(cfg.kind, m, si, cfg.seriesTypes) === "bar").length;
  const grouped = !resolveStacked(cfg) && barMetrics > 1;
  // The comment this replaces said Metabase tints a lone series with its summarize green. Measured
  // on identical data it does not: a single-metric bar card draws #f9d45c, the colour "revenue"
  // hashes to. The green is Metabase's `success`, which `colorsForKeys` still returns — for a series
  // actually NAMED "profit" or "completed", which is the rule that was mistaken for a lone-series
  // one.
  //
  // A DUPLICATE metric would now draw both series in one colour, where the positional scheme gave
  // them separate slots — `colorsForKeys` returns a Record, so one name is one entry. Not reachable
  // from the picker (`FieldZone` offers only metrics not already selected, and its chips are keyed
  // by name), but chart settings are stored JSON and are not whitelisted server-side, so a
  // hand-written config can hold one. Left as is rather than guarded: two series that ARE the same
  // metric being drawn the same is a fair reading of that config, and the old scheme already
  // collapsed them the moment either had a `seriesColors` entry, which is keyed by name too.
  const metricColors = colorsForKeys(cfg.yMetrics, cfg.seriesColors);
  let series = cfg.yMetrics.map((metric, si) => {
    const mi = idx[metric];
    const data = categories.map((c) => {
      const r = rowByCat.get(catKey(c));
      return r == null ? null : (r[mi] as number | null);
    });
    return decorateSeries({ name: metric, data }, metric, si, cfg, fmt, metricColors, grouped);
  });

  // Folding is offered only where "Other" means one clear thing: a single metric, no
  // breakout, and a plain category axis. With a breakout the bucket would have to merge
  // series as well, which changes what the chart says; on a value or time axis "Other" has
  // no coordinate to sit at; and a temporal extract axis is already a fixed small domain
  // (twelve months, seven weekdays) that nobody needs folded. And a trend line fitted
  // through an "Other" bucket would describe where the user set the cutoff rather than the
  // data, since the bucket's size moves with it. Those are separate decisions, taken when
  // the charts that need them are redesigned.
  let foldedCategory: string | undefined;
  const foldable =
    cfg.maxCategories != null &&
    !cfg.series &&
    cfg.yMetrics.length === 1 &&
    !usesXScale(cfg) &&
    !isExtractUnit(cfg.xAxisUnit) &&
    !cfg.trendLine;
  if (foldable) {
    const s = series[0] as unknown as SeriesLike;
    const folded = foldTail(
      categories.map((c, i) => ({ name: String(c), value: s.data?.[i] })),
      cfg.maxCategories,
    );
    // A fold actually happened only if the list got shorter. Recorded on the option so the drill
    // can refuse that one bar: "Other" here is a bucket the chart invented, and `= 'Other'` either
    // matches nothing or — because foldTail merges into a REAL category of that name when one
    // exists — matches a fraction of the bar and understates it. Both are wrong silently.
    if (folded.length < categories.length) foldedCategory = OTHER_LABEL;
    categories = folded.map((p) => p.name);
    s.data = folded.map((p) => (typeof p.value === "number" ? p.value : null));
  }

  series = applySeriesOrder(series as unknown as SeriesLike[], cfg) as unknown as typeof series;

  if (isNormalized(cfg)) normalizeStacks(series as unknown as { data: (number | null)[] }[]);
  if (cfg.trendLine) series.push(...(trendSeries(series as unknown as SeriesLike[]) as unknown as typeof series));
  if (resolveStacked(cfg) && cfg.showStackTotal && !isNormalized(cfg)) series.push(stackTotalSeries(series as unknown as SeriesLike[], cfg) as unknown as (typeof series)[number]);
  if (usesXScale(cfg)) applyXScale(categories, series, cfg);
  applyMarkers(series as unknown as SeriesLike[]);
  const displayCats = isExtractUnit(cfg.xAxisUnit) ? categories.map((v) => temporalLabel(cfg.xAxisUnit, v)) : categories;
  // `__foldedCategory` is not an ECharts key — ECharts ignores what it doesn't know. It rides on
  // the option because the drill needs to recognise the one bar it must refuse, and the option is
  // the only thing both the builder and the click handler hold.
  return { ...baseOption(cfg, displayCats as (string | number)[], series, size), __foldedCategory: foldedCategory };
}

// Shared decoration: type, color, stack, areaStyle, labels, smooth, symbol.
function decorateSeries(
  s: { name: string; data: (number | null)[] },
  metric: string,
  colorIdx: number,
  cfg: CartesianBuildConfig,
  fmt: (metric: string, v: unknown) => string,
  /** The colour of every series this chart will draw, keyed by its RAW name — built once by the
   *  caller, because the assignment is a decision about the whole set (a hash collision moves one
   *  key along, and a pinned colour takes its slot out of circulation) and cannot be made one
   *  series at a time. Both call paths pass a complete mapping, so there is no `??` fallback here:
   *  a positional one would be a second, silent colour rule that no test could see. */
  colors: Record<string, string>,
  grouped = false,
) {
  const display = seriesDisplayFor(cfg.kind, s.name ?? metric, colorIdx, cfg.seriesTypes);
  const type = echartsTypeOf(display);
  // `cfg.seriesColors` is NOT consulted again here. The `?? cfg.seriesColors?.[s.name]` this
  // replaces was redundant rather than wrong — `colorsForKeys` has already folded the user's pinned
  // colours in, so re-reading them changes no pixel today (measured: restoring the chain leaves the
  // whole chart suite green). It goes because it reads as a SECOND authority on colour, and the next
  // person to reach for it — to add a case the mapping does not cover — gets a series that is pinned
  // here but still in circulation there, and two series drawn in the same ink.
  const color = colors[s.name];
  const out: Record<string, unknown> = {
    ...s,
    // Display label, in falling order of authority: the user's explicit rename, then the series
    // dimension's own column format, then the raw value. The FORMAT step is what made a breakout
    // by month read "2026-01-01" in the legend while the panel offered a date style for that very
    // column — offered, stored, and ignored.
    name: cfg.seriesLabels?.[s.name] ?? seriesDisplayName(cfg, s.name),
    // The RAW series value, kept because `name` above is what the reader sees and the drill needs
    // what the column holds. Filtering by the display label found nothing the moment a series was
    // renamed — the rename silently broke the click.
    __raw: s.name,
    __metric: metric,                            // keep the source metric for tooltip formatting
    type,
    itemStyle: { color },
  };
  // Only pin the bar width for a single bar per category; grouped bars must let
  // ECharts auto-size and place them side by side (otherwise they overlap).
  if (type === "bar") { if (!grouped) out.barWidth = CHART_THEME.barWidth; out.barMinHeight = BAR_MIN_HEIGHT; }
  // Filled either because the whole chart is an area, or because this one series is displayed as
  // one — a combo can hold both a filled band and a bare line.
  const filled = cfg.kind === "area" || display === "area";
  if (type === "bar" || filled) {
    if (resolveStacked(cfg)) out.stack = "total";
  }
  if (filled) out.areaStyle = { opacity: CHART_THEME.areaOpacity };

  // Pointing at a series lifts it and pushes every other one back — the effect that makes a
  // multi-series chart readable, and the one thing egret's cartesian charts had no answer for.
  // Without `focus`, ECharts applies its default emphasis to whatever is under the pointer and
  // leaves the rest alone: measured on egret, hovering one band lightened ALL FOUR, which says
  // nothing about which one you are on.
  //
  // The numbers are Metabase's, measured on a live instance rather than read off its source:
  //   bar  — hovered stays opaque, the other series drop to 0.3
  //   area — hovered fill goes 0.3 -> 0.6, the other fills 0.3 -> 0.2
  // The trend and stack-total helper series are excluded by their callers, which never reach here.
  out.emphasis = {
    focus: "series",
    ...(filled ? { areaStyle: { opacity: CHART_THEME.areaOpacityEmphasis } } : {}),
  };
  out.blur = {
    itemStyle: { opacity: CHART_THEME.blurOpacity },
    lineStyle: { opacity: CHART_THEME.blurOpacity },
    ...(filled ? { areaStyle: { opacity: CHART_THEME.areaOpacityBlur } } : {}),
  };
  if (type === "line") {
    // A series may depart from the chart-wide line appearance. Metabase keeps these settings
    // per-series only; egret keeps a chart-wide default and lets a series override it, so the
    // common case stays one control instead of one per line.
    const ln = cfg.seriesLine?.[s.name] ?? {};
    const shape = ln.shape ?? cfg.lineShape ?? "linear";
    out.smooth = shape === "smooth";
    if (shape === "step") out.step = "middle";   // egret's existing step mode, not Metabase's step-after
    // Resolved here, applied at the end of buildCartesianOption: folding and x-scaling both
    // rewrite the data afterwards, and "auto" is a question about the points that actually get
    // drawn.
    out.__markers = ln.markers ?? cfg.markers ?? "auto";
    // Chart-wide only: `LineSeriesStyle` has no `missing`, because how a gap is read is a
    // statement about the data, and two series disagreeing would make the chart say two
    // things at once.
    // A log axis has no zero, so filling with one cannot mean what it says — ECharts would drop
    // the point and break the line at exactly the place the user asked to have filled. Leaving
    // the gap is the honest version of "this value cannot be shown here".
    const missing = cfg.logScale && (cfg.missing ?? "interpolate") === "zero"
      ? "none"
      : (cfg.missing ?? "interpolate");
    out.connectNulls = missing === "interpolate";
    if (missing === "zero") {
      // ECharts can bridge a null or break at it, but it has no notion of treating one as
      // zero — that has to be the data, not an option. Safe to rewrite here as plain numbers:
      // applyXScale (which re-keys onto [x, y] pairs) runs after decorateSeries, never before.
      // This is not a rendering nicety: the zero becomes a real data point downstream too — it
      // counts in stack totals, in normalisation, and in a trend fit — because `missing: "zero"`
      // means the user is asserting what the absent value WAS, not just how to draw it.
      out.data = (s.data ?? []).map((v) => (v == null ? 0 : v));
    }
    // Cycle the dash style + marker shape per series so lines are distinguishable
    // without relying on colour alone (a11y: colour-blind users). The dash cycle is an
    // accessibility feature — it lets lines be told apart without colour — so a per-series
    // style REPLACES it for that one series only and leaves the rest cycling.
    // Settings JSON is not whitelisted server-side, so `ln.size` may be a value the type
    // never promised (e.g. a stored "XL"). `in` guards the lookup so an unknown size falls
    // back to the theme width instead of `LINE_WIDTHS[unknown]` silently overwriting
    // ECharts' default width with `undefined`.
    out.lineStyle = {
      width: ln.size && ln.size in LINE_WIDTHS ? LINE_WIDTHS[ln.size] : CHART_THEME.lineWidth,
      type: ln.style ?? lineType(colorIdx),
    };
    out.symbol = symbolShape(colorIdx);
    out.symbolSize = CHART_THEME.symbolSize;
  }
  // Scatter series get a distinct marker shape per series (shape, not colour).
  if (type === "scatter") { out.symbol = symbolShape(colorIdx); out.symbolSize = CHART_THEME.symbolSize; out.itemStyle = { color, opacity: CHART_THEME.scatterOpacity }; }
  if (isDualAxis(cfg) && rightMetrics(cfg).includes(metric)) out.yAxisIndex = 1;
  if (cfg.showLabels) {
    out.label = {
      show: true, fontSize: CHART_THEME.labelFontSize, fontWeight: CHART_THEME.labelWeight,
      position: isHorizontal(cfg) ? "right" : "top",
      // On a value/time x-axis the datum is an [x, y] pair — label the y.
      formatter: (p: { value: unknown }) => fmt(metric, Array.isArray(p.value) ? p.value[1] : p.value),
    };
    // "fit" is the default: at a dashboard card's width there is no room for a label per
    // bar, and a pile of overlapping numbers is worse than showing fewer. "all" is for
    // someone who would rather have every number and accept the crowding.
    if (cfg.labelDensity !== "all") out.labelLayout = { hideOverlap: true };
  }
  return out;
}

function baseOption(
  cfg: CartesianBuildConfig,
  categories: (string | number)[],
  series: unknown[],
  /** Absent when the caller has no measurement; see the x-label fit below. */
  size?: EChartsSize,
): EChartsCoreOption {
  const horizontal = isHorizontal(cfg);
  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const normalized = isNormalized(cfg);
  // Defaults are decided here and never stored, so a chart saved before this change picks
  // them up too. `??` keeps an explicit choice winning — including `false` and `""`.
  const hasBreakout = cfg.series != null && cfg.series !== "";
  const showLegend = cfg.showLegend ?? (hasBreakout || cfg.yMetrics.length > 1);
  // A renamed series is what the legend and tooltip already show, so the axis follows it
  // first; `column_title` is the per-column rename the Formatting panel will write.
  const titleOf = (col: string | undefined) =>
    col == null ? undefined : (cfg.seriesLabels?.[col] ?? cfg.columnSettings?.[col]?.column_title ?? col);
  // An axis plotting several metrics has no one name — the legend names them instead.
  const metricTitle = (metrics: string[]) => (metrics.length === 1 ? titleOf(metrics[0]) : undefined);
  const xTitle = cfg.showXLabel === false ? undefined : (cfg.xLabel ?? titleOf(cfg.xAxis));

  // Compact labels are for wide numbers, not for many bars — three bars of revenue in the
  // millions crowd an axis just as badly as forty do. Measured per axis: a dual-axis chart
  // has two scales on purpose, and the small one should not inherit the big one's shorthand.
  const plotted = (series as SeriesLike[]).filter((s) => !s.__trend && !s.__total);

  // Axis shape follows the series that survived hiding, not the config: hiding the only
  // right-axis metric should take its axis with it. Keyed on yAxisIndex rather than on the
  // metric names because that index was baked into each series before hiding ran — an axis
  // a surviving series still points at has to exist. The Set matters: a breakout splits one
  // metric across many series, so without deduping, leftSide would list it once per series
  // and metricTitle (single-metric only) would stop titling breakout charts' axis.
  const dual = plotted.some((s) => s.yAxisIndex === 1);
  const rightSide = dual ? rightMetrics(cfg) : [];
  const leftSide = [...new Set(plotted.filter((s) => s.yAxisIndex !== 1).map((s) => s.__metric ?? s.name ?? ""))]
    .filter(Boolean);
  const yTitle = cfg.showYLabel === false ? undefined : (cfg.yLabel ?? metricTitle(leftSide));
  const widestOf = (metrics: string[]) => {
    let widest = 0;
    for (const s of plotted) {
      // A breakout splits one metric into many series, each still tagged with __metric.
      // An untagged series predates that and is counted against whichever axis asks.
      if (s.__metric != null && !metrics.includes(s.__metric)) continue;
      for (const d of (s.data ?? []) as unknown[]) {
        // On a value/time x-axis the datum is an [x, y] pair — the y is what the axis shows.
        const n = Math.abs(Number(Array.isArray(d) ? d[1] : d));
        if (Number.isFinite(n) && n > widest) widest = n;
      }
    }
    return widest;
  };

  const fmtFor = (metrics: string[]) => {
    // 10,000 is where a grouped number reaches six characters and "10K" starts winning.
    const compact = normalized ? false : (cfg.compactValues ?? widestOf(metrics) >= 10_000);
    return (v: number) => {
      if (normalized) return `${Math.round(v)}%`;
      const m = metrics[0];
      if (!m) return String(v);
      const col = cfg.columnSettings?.[m];
      // A column that states its own `compact` preference outranks the chart-wide default.
      return formatValue(v, cfg.metricTypes?.[m] ?? "double", { ...col, compact: col?.compact ?? compact });
    };
  };

  // Whether the labels fit is a question about WIDTH, and until `size` existed this builder could
  // not ask it: the rule here was `longest > 10 && categories.length > 6 ? 30 : 0`, and length and
  // count are only proxies for width. Four categories whose longest is "Doohickey" on a
  // one-third-width card came out under that rule as "no rotation needed", and then
  // `interval: "auto"` let ECharts drop three of the four — an axis reading as missing data.
  //
  // THE MEASUREMENT applies to a bottom ORDINAL axis and nothing else. A horizontal chart stacks
  // its categories down the left, where their length costs nothing, so there is no collision to
  // measure; and a linear/timeseries x is a value axis whose labels are ticks ECharts generates,
  // not this array — deciding a fit from `categories` there would reserve room below a rotation
  // that never gets applied, since `scaleAxis` carries no `rotate`. Fitting a value axis means
  // generating its ticks first, and NOTHING OWNS THAT: this said "which is PR 03", and PR 03 turned
  // out to be about which axis a scatter gets (`resolveXScale`), not about laying out the ticks on
  // it. The deferral stands; the owner named for it never existed.
  const ordinalX = !horizontal && !usesXScale(cfg);

  /**
   * What a category is DRAWN AS. Hoisted above the fit below rather than written inline on
   * `axisLabel.formatter`, because the fitter has to measure THIS STRING and nothing else — see
   * the `labels:` line at the call site for what happened when it measured the other one.
   *
   * THREE answers in priority order, and the middle one is what PR 03 added:
   *   1. the column's own format, when it says something the formatter for this column would
   *      consult — a stored-then-ignored setting is the defect the pivot, the pie and the breakout
   *      legend each had in turn. "Says something" rather than "exists": on a BUCKETED axis, where
   *      arm 2 is in play, a format silent about dates would otherwise beat the period name with
   *      `formatDate`'s DEFAULTS, and the panel mints exactly such objects — renaming the column to
   *      "Month" stores `{column_title: "Month"}` and reverted this axis from "January 2026" to
   *      "January 1, 2026" (see `specifiesDateRendering`). Off a bucketed axis there is nothing for
   *      the format to outrank, so it still wins on existence there, as it always did.
   *   2. otherwise the NAME OF THE PERIOD the bucket covers, when the axis is bucketed by a
   *      truncation unit — "January 2026" for a month, not its first day;
   *   3. otherwise the raw value, which is every non-temporal axis and every temporal column that
   *      was not bucketed.
   * `periodName` is total and falls through to the raw string itself, so it IS answer 3 as well —
   * `namesAPeriod` is asked only whether arm 2 EXISTS to be outranked, never to guard the call.
   *
   * The trend card answers the same question about the same column and has to stay in step:
   * `SmartScalarChart.tsx`, where arms 1 and 2 are these and ARM 3 IS NOT, and says so.
   */
  const bucketedX = namesAPeriod(cfg.xAxisUnit);
  const drawLabel = (v: string) => {
    const fmt = cfg.xAxis ? cfg.columnSettings?.[cfg.xAxis] : undefined;
    const formatWins = fmt != null && (bucketedX ? specifiesDateRendering(fmt) : true);
    if (formatWins && v != null) {
      const out = formatValue(v, cfg.xAxisType ?? "varchar", fmt);
      if (out !== "") return out;
    }
    return periodName(cfg.xAxisUnit, v);
  };

  // An explicit rotation SKIPS the measurement rather than merely outranking it. Computing the fit
  // anyway and using it for `show` would honour the user's 45° and hide their axis in the same
  // breath, which is worse than either outcome alone and looks like the setting doing nothing.
  // Metabase's own auto-selection likewise applies only while the setting is untouched.
  const explicitRotate = cfg.xLabelRotation != null;
  // A zero size is not a small chart, it is an UNMEASURED one: `EChartsRenderer` reads
  // `getBoundingClientRect()` before `init`, so a hidden or not-yet-laid-out container reports
  // `{0, 0}`. Measuring it would give a negative `plotWidth` — which the fitter answers "draw every
  // label upright", from a container nobody could see — and a zero height would make the rotated
  // label's projected share infinite and hide the axis. Both are decisions taken against a guessed
  // width, which is the one thing `size`'s own doc says never to do, so decline instead.
  const measurable = size != null && size.width > 0 && size.height > 0;
  /** What the category axis draws, wherever that axis sits — measured by the ladder below on a
   *  vertical chart and by the row chart's two fits further down, so the two cannot disagree about
   *  which strings are on the card. */
  const drawnCategories = categories.map((c) => drawLabel(String(c)));
  const fit: XLabelFit | undefined = measurable && size && ordinalX && !explicitRotate
    ? fitXLabels({
        // THE STRING THE AXIS ACTUALLY DRAWS, by construction — same closure, same argument.
        //
        // This measured `categories.map(String)`, the RAW value, and justified the gap as "the
        // formatted label is the wider of the two, so this under-measures". PR 03 falsified both
        // halves at once. The two now differ whenever `xAxisUnit` is a truncation unit — the
        // common case and this PR's whole subject, not the rare `columnSettings` one that comment
        // named — and the period name is often NARROWER, not wider: a year measured "2026-01-01"
        // at ten characters and drew "2026" at four, a quarter ten against "Q3 2026" at seven. So
        // it had begun to OVER-measure, which rotates labels that would have sat upright, in the
        // PR whose entire motivation is labels not fitting. Formatting here retires the
        // `columnSettings` skew too, which that comment conceded and left standing.
        labels: drawnCategories,
        // The widest VALUE the left axis carries and the axis's own formatter — not the formatted
        // datum, which is what this passed until the gutter was found reserving 10px for "24.7K" on
        // an axis whose top tick is "25K". `estimatePlotWidth` rounds to a tick it would draw.
        plotWidth: estimatePlotWidth(size, widestOf(leftSide), fmtFor(leftSide), Boolean(yTitle)),
        // The CONTAINER's height, not an inset one: the 70% guard is a share of the whole card
        // (Metabase's `checkHeight`), so passing a grid-adjusted height silently tightens it.
        outerHeight: size.height,
        // THE PRECONDITION FOR THINNING, enforced here rather than assumed downstream: only a
        // BUCKETED temporal axis gets one. `bucketedX` is `namesAPeriod(cfg.xAxisUnit)`, the same
        // single membership rule `drawLabel` above uses, so an axis that is thinned is by
        // construction an axis whose labels are period names. On an ordinal axis of product names
        // this is `undefined` and the fitter cannot reach its stride arm at all — there is no such
        // thing as every second product. (Both fields are also empty for a non-period unit, so the
        // guard holds a second time even if this line is ever loosened.)
        //
        // ONLY THE LADDER, no sizing candidate. This used to pass `widestLabel:
        // widestPeriodName(cfg.xAxisUnit, drawLabel)` as well — the widest name the unit could
        // produce, threaded through this builder's own `drawLabel` so the stride was sized against a
        // string the axis would really render. It was still the wrong string: a rung draws indices
        // 0, every, 2*every, … and the fitter now measures exactly those out of `labels:` above,
        // which is the same closure's output for the buckets this chart actually has. See
        // `strideThatFits` for the browser measurement and for what dropping the unit's worst case
        // concedes.
        boundary: bucketedX ? { steps: boundarySteps(cfg.xAxisUnit) } : undefined,
      })
    : undefined;
  // …but an explicit rotation is honoured WHEREVER the category axis sits, `ordinalX` or not. On a
  // row chart it is the left-hand axis, and ECharts rotates that one too: `buildAxisLabel` reads
  // `axisLabel.rotate` for every axis kind and `innerTextLayout` composes it with the axis's own
  // rotation (`PI/2` for a y-axis, `cartesianAxisHelper.js`), so 90° there turns the names to run
  // vertically alongside the axis. Dropping the setting because the axis is not at the bottom
  // would be a silent no-op in the panel — and the panel offers this control on row charts, where
  // it labels the vertical axis as the category one.
  const rotate = cfg.xLabelRotation ?? (fit?.kind === "rotate" ? fit.rotate : 0);

  /**
   * A ROW CHART'S TWO AXES ARE THE OTHER WAY UP, and both of the fits above are about the bottom
   * one, so neither has ever run for it. What it needs instead is measured here.
   *
   * THE LADDER ABOVE IS NOT SIMPLY WIDENED TO REACH IT, which is the tempting one-line version.
   * `ordinalX` excludes a horizontal chart for the right reason: the ladder's answers are ROTATE and
   * HIDE, and a row chart's bottom axis is a VALUE axis. Its labels are round ticks ECharts
   * generated, so tilting them is a remedy Metabase does not offer there (it greys the control out
   * on a value axis) and hiding them is not what its row chart does either — at 311px it draws one
   * tick and keeps drawing it. Fewer ticks is the value axis's answer; see `fitValueTicks`.
   *
   * WHAT THE LEFT SIDE COSTS IS THE CATEGORY NAMES, not a column of value ticks — the row chart is
   * the one cartesian where those two swap edges — so the gutter is measured from the labels
   * themselves rather than sampled from `widestTickLabel`. Measured ONCE, because the same width
   * decides both answers: how much plot is left for the ticks, and how far out the category axis's
   * own name has to sit to clear its labels.
   *
   * Live, at the 166x184 card this was found on: `Doohickey` measures 60.8 of the 166, leaving ~55
   * for ticks that measure 30.6 apiece — one label's worth, where four were being drawn on an 8px
   * pitch, each overlapping its neighbour by 23px.
   */
  const measuredCategories = horizontal && measurable && size ? widestLabelWidth(drawnCategories) : 0;
  // A ZERO IS NOT A ZERO-WIDTH LABEL, it is no measurement at all: `measure-text.ts` reports one
  // deliberately where there is no canvas ("Size claims belong in a browser"), which is every jsdom
  // test in the repo. Declining there is the same rule `fitValueTicks` keeps for its own candidate
  // and the same one `measurable` keeps for `size` — a name placed against a width nobody observed
  // is the guess this builder does not make. It also keeps every unmeasured row chart's option
  // exactly as it was before this fit existed.
  const categoryGutter = measuredCategories > 0 ? measuredCategories : undefined;
  /**
   * How far the value axis runs, which is what an interval has to divide.
   *
   * A BAR AXIS INCLUDES ZERO whether or not the data does — ECharts' `scale` defaults to false and
   * a bar measured from a floating baseline lies — so the extent is the data's own, widened to the
   * origin. `yMin`/`yMax` replace their end when the user has set one, and a normalised chart is
   * 0-100 by construction (the `max: 100` below is the same statement).
   */
  const axisSpan = (metrics: string[]) => {
    if (normalized) return 100;
    let lo = 0, hi = 0;
    for (const s of plotted) {
      if (s.__metric != null && !metrics.includes(s.__metric)) continue;
      for (const d of (s.data ?? []) as unknown[]) {
        const n = Number(Array.isArray(d) ? d[1] : d);
        if (!Number.isFinite(n)) continue;
        if (n < lo) lo = n;
        if (n > hi) hi = n;
      }
    }
    return (cfg.yMax ?? hi) - (cfg.yMin ?? lo);
  };
  // A LOG axis is excluded: its ticks are powers, so a linear interval means something else
  // entirely there and the crowding it suffers is not the crowding this measures.
  const valueInterval = categoryGutter != null && size && !cfg.logScale
    ? intervalThatFits({
        plotWidth: plotWidthLessGutter(size, categoryGutter, Boolean(xTitle)),
        span: axisSpan(leftSide),
        // The same pair the ladder charges its gutter for, and for the same reason: a value axis
        // draws round ticks, not data, so the candidate is `widestTickLabel`'s and not a datum's.
        // On a horizontal chart `leftSide` is the metric side whatever the name says — the split is
        // by series, and `isDualAxis` is false here, so every plotted series is in it.
        widestValue: widestOf(leftSide),
        format: fmtFor(leftSide),
      })
    : undefined;
  // Either way the axis has now DECIDED, and a decided axis never leaves ECharts the subset
  // `interval: "auto"` would let it pick. What it draws is every label, none, or — on a bucketed
  // temporal axis only — every Nth, which the `interval` line below argues is not the same thing.
  const decided = fit != null || explicitRotate;
  // NO ROOM IS RESERVED BELOW THE PLOT FOR A TILTED LABEL, at any rotation, measured or explicit.
  // See `XLabelFit`'s doc in `../axisLabelFit.ts` for the measurement that settled it: `containLabel`
  // already expands the grid around the rotated labels, and everything this builder added on top
  // came off the plot one-for-one. Deleted rather than set to zero — a `+ 0` in the grid below is
  // an invitation to put a number back in it.

  const categoryAxis = {
    type: "category" as const, data: categories,
    name: xTitle, nameLocation: "middle" as const,
    // `nameGap` is measured from the AXIS LINE, so what it has to clear is whatever sits between the
    // line and the name — nothing at the bottom of a vertical chart, where 28 has always been right,
    // but the whole column of category names on a row chart, where the title was drawn straight
    // across them. `nameGapClearOf` states the three terms. Unmeasured, 28 stands: a gap guessed
    // from no width is the one thing this builder's `size` doc says never to do, and the row chart
    // it mis-places is the row chart nobody has measured.
    nameGap: categoryGutter != null ? nameGapClearOf(categoryGutter) : 28,
    // The title matches the label colour — an axis name is not a lesser piece of chrome
    // than the values beneath it, and ECharts' default nameTextStyle colour reads paler.
    nameTextStyle: { color: axisLabelColor },
    // Category labels go through the column's own format. Extraction units are already
    // substituted above (`displayCats`); a TRUNCATION unit is named by the formatter below —
    // a chart grouped by month read "2026-01-01" where the bucket is really January 2026. The
    // formatter only changes what is drawn; `data` stays the raw value, so a drill still filters
    // on what the engine returned.
    axisLabel: {
      fontSize: CHART_THEME.axisFontSize, color: axisLabelColor,
      // `fit.every - 1`, NOT `fit.every`: ECharts' category `interval` counts the labels SKIPPED
      // between two drawn ones, not the stride. `ordinalScaleCreateTicks`
      // (`echarts/lib/scale/helper.js:235`) reads it as `step = max((categoryInterval || 0) + 1, 1)`
      // and walks `tickValue += step` from the extent's start — so `0` is every label, which is what
      // the `decided` arm below has always meant, and every 6th is `5`. Off by one here is invisible
      // in a screenshot: it draws Jan/Jun/Nov instead of Jan/Jul, which looks like a date axis.
      //
      // Two more things checked in that source rather than assumed, because both would undo the
      // "every drawn label is a period boundary" property this rests on. The stride starts at INDEX
      // ZERO — `startTick` is only re-aligned when the extent starts past 0, i.e. under a data zoom
      // this axis does not have (`:239-241`) — so the first bucket is always one of the drawn ones.
      // And the last index is appended `offInterval` (`:251-253`) but then IGNORED by `AxisBuilder`
      // while `showMinLabel`/`showMaxLabel` are unset, which they are (`AxisBuilder.js:675`); so a
      // December that is not on the stride is not quietly drawn beside July. The tick MARKS follow
      // the labels for free: `axisTick` carries no `interval`, which defaults to "auto", and "auto"
      // on a tick model means "whatever the labels decided" (`axisTickLabelBuilder.js:182-187`).
      //
      // AND THIS IS NOT PR 02's RULE BEING UNDONE, which is the reading to head off: "auto" drops
      // whichever labels happen to collide, an arbitrary subset, while a stride over buckets draws
      // period boundaries — the full argument, and why restoring a flat `interval: 0` here would
      // NOT restore PR 02, is on the `thin` arm of `XLabelFit` in `../axisLabelFit.ts`, which is
      // its one copy.
      interval: fit?.kind === "thin" ? fit.every - 1 : decided ? (0 as const) : ("auto" as const),
      // THE FIRST BUCKET IS THE ONE LABEL A STRIDE PROMISES, so on a thinned axis it is declared
      // required rather than left to ECharts' min/max heuristic — which discards it. Read in
      // `AxisBuilder.fixMinMaxLabelShow` (`:648-726`) against echarts 6.1, because a `thin` decision
      // that renders nothing is exactly what this looked like on the live dashboard:
      //
      //   - `ordinalScaleCreateTicks` (`scale/helper.js:229-253`) walks the stride from index 0 and
      //     then APPENDS the extent's last index with `offInterval: true`. Twelve monthly buckets at
      //     stride 12 therefore hand `AxisBuilder` two labels, "January 2026" and "December 2026",
      //     not one.
      //   - `fixMinMaxLabelShow` runs `deal(showMinLabel, 0, 1)` FIRST. With the option unset it
      //     tests the two for intersection and, finding one, ignores the OUTMOST — January — before
      //     `deal(showMaxLabel, 1, 0)` ever gets to drop December for being `offInterval`. Two
      //     labels ~84 and ~91px wide on a 94px plot overlap by about a pixel, so the axis lost the
      //     only label it had chosen to draw, to a label it was about to throw away.
      //
      // `true` flips that one branch: the inner label is dropped instead, which is the same December
      // the next `deal` would have dropped anyway. Set ONLY on the thin arm — index 0 is on-stride
      // there by construction (`ordinalScaleCreateTicks` starts at 0), and it is the label the whole
      // `thin` argument in `../axisLabelFit.ts` rests on. On every other arm the axis draws all its
      // labels or none, and this heuristic has nothing to decide.
      showMinLabel: fit?.kind === "thin" ? true : undefined,
      rotate,
      // The last answer the ladder can give: when even a rotated label would eat 70% of the
      // card's height, Metabase draws no axis rather than a wall of text.
      show: fit?.kind !== "hide",
      // `drawLabel`, defined above so the FITTER measures this same string. Do not inline it back:
      // the two drifting apart is what made the fit rotate labels that would have fitted upright.
      formatter: drawLabel,
    },
    // TWO different `show`s sit on this axis and they are not the same switch. `xAxisTicks` is the
    // axis LINE and the tick marks; the LABELS are `axisLabel.show` above, which only the measured
    // fit ever writes. Metabase's single `axis_enabled: false` hides both, so egret's "Hide" is
    // narrower than its namesake — a pre-existing divergence shared with the other two charts that
    // ship this select (the waterfall and the box plot, via `settings/ChartAxesSection.tsx`; a
    // sankey has no axes and stores neither field), but worth saying here now that a second `show`
    // on the same axis could be mistaken for the one the panel writes.
    axisLine: { show: cfg.xAxisTicks !== false, lineStyle: { color: gridLine } },
    axisTick: { show: cfg.xAxisTicks !== false },
  };
  // Non-ordinal x: a value (linear) or time (timeseries) axis positioning points by
  // their real coordinate. Series carry [x, y] pairs (see applyXScale).
  //
  // No `rotate` here, and PR 03 did NOT change that — this note used to say rotating these labels
  // "is PR 03", and PR 03 turned out to be about which axis a scatter gets, not about laying out the
  // ticks on it. An explicit `xLabelRotation` is still a NO-OP on a value x-axis.
  //
  // NOT BECAUSE IT COULD NOT BE APPLIED. ECharts would honour it: `AxisBuilder.buildAxisLabel` reads
  // `axisLabel.rotate` with no branch on the axis kind (`AxisBuilder.js:875`), so adding one key
  // here would rotate these generated ticks today. The reason not to is PARITY — Metabase greys the
  // rotation out on a value axis too, so honouring it would be a divergence dressed up as a fix, in
  // the workstream whose whole point is to stop diverging. A second reason, real but thinner in a
  // product this young: a chart saved as Linear can still CARRY a rotation from before the switch,
  // and applying it would silently turn labels nobody has touched in months. Either way the setting
  // is one the panel must not OFFER where it would land here, which is what `valueX` is for.
  //
  // WHAT PR 03 ADDED TO THIS AXIS'S CATCHMENT, and the reason the panel rule had to move with it: a
  // scatter over a numeric dimension now arrives here WITHOUT anyone having chosen a scale
  // (`resolveXScale`). Before, `usesXScale` was true only where the user had picked Linear or
  // Timeseries and the panel could grey the rotations out by reading `config.xScale` alone. It now
  // has to resolve the scale the same way this builder does, from the x column's type —
  // `settings/CartesianDisplaySection.tsx`, `valueX`, which calls `resolveXScale` rather than
  // restating it. A config can still CARRY a rotation here, and the panel still shows it rather
  // than pretending it is gone; what it will not do is let one be newly chosen.
  const scaleAxis = {
    type: cfg.xScale === "timeseries" ? ("time" as const) : ("value" as const),
    name: xTitle, nameLocation: "middle" as const, nameGap: 28,
    nameTextStyle: { color: axisLabelColor },
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor },
    axisLine: { show: cfg.xAxisTicks !== false, lineStyle: { color: gridLine } },
    axisTick: { show: cfg.xAxisTicks !== false },
    splitLine: { show: false as const },
  };
  const xCategory = usesXScale(cfg) ? scaleAxis : categoryAxis;
  const mkValueAxis = (metrics: string[], position?: "left" | "right") => ({
    type: cfg.logScale ? ("log" as const) : ("value" as const),
    name: position === "right" ? undefined : yTitle,
    nameLocation: "middle" as const,
    nameTextStyle: { color: axisLabelColor },
    // Clear of the tick labels: a vertical axis's numbers are far wider than a horizontal
    // axis's are tall.
    nameGap: horizontal ? 28 : 44,
    position,
    ...(normalized ? { max: 100 } : {}),
    // Manual axis range (Metabase y-axis min/max) overrides the auto extent.
    ...(cfg.yMin != null ? { min: cfg.yMin } : {}),
    ...(cfg.yMax != null ? { max: cfg.yMax } : {}),
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: fmtFor(metrics) },
    axisLine: { show: cfg.yAxisTicks !== false, lineStyle: { color: gridLine } },
    axisTick: { show: cfg.yAxisTicks !== false },
    // `splitNumber` is a HINT — ECharts still rounds to human intervals, so asking for 5 can
    // yield 4 or 6. Metabase's own control behaves the same way, being the same underlying knob.
    //
    // The user's own tick count wins outright, and takes the hint's looseness with it. `interval` is
    // the measured fallback behind it, and it is `undefined` on every axis this fit has nothing to
    // say about — a vertical chart's value axis, whose ticks stack downwards where their width costs
    // nothing, and any row chart wide enough for ECharts' own default. So these two lines change
    // exactly one option: a narrow row chart's bottom axis. `intervalThatFits` argues why it is an
    // interval rather than a `splitNumber` of our own, and has the measurements.
    ...(cfg.yTickCount != null ? { splitNumber: cfg.yTickCount }
      : valueInterval != null ? { interval: valueInterval } : {}),
    // A second (right) axis omits gridlines to avoid doubling them up.
    splitLine: position === "right" ? { show: false } : { lineStyle: { color: gridLine } },
  });

  let xAxis: unknown;
  let yAxis: unknown;
  if (horizontal) {
    xAxis = mkValueAxis(leftSide);
    yAxis = categoryAxis;
  } else if (dual) {
    xAxis = xCategory;
    yAxis = [mkValueAxis(leftSide, "left"), mkValueAxis(rightSide, "right")];
  } else {
    xAxis = xCategory;
    yAxis = mkValueAxis(leftSide);
  }

  // Goal line — a horizontal reference at goalValue, drawn via the first series' markLine.
  if (cfg.goalValue != null && series.length > 0) {
    (series[0] as Record<string, unknown>).markLine = {
      silent: true,
      symbol: "none",
      data: [{ yAxis: cfg.goalValue }],
      label: { formatter: cfg.goalLabel ?? "Goal", position: "insideEndTop", color: axisLabelColor },
      lineStyle: { color: axisLabelColor, type: "dashed" },
    };
  }

  // Metabase puts the legend along the top; reserve room there for it. The category axis
  // sits at the bottom normally and on the left when horizontal — the value axis is the
  // other one — so reserve room on whichever edge each title actually renders on.
  const bottomTitle = horizontal ? yTitle : xTitle;
  const leftTitle = horizontal ? xTitle : yTitle;
  const grid = {
    ...CHART_THEME.grid,
    top: (CHART_THEME.grid.top as number) + (showLegend ? 24 : 0),
    bottom: (CHART_THEME.grid.bottom as number) + (bottomTitle ? 22 : 0),
    left: (CHART_THEME.grid.left as number) + (leftTitle ? 26 : 0),
  };

  // Trend series stay out of the legend.
  const legendNames = series.filter((s) => !(s as SeriesLike).__trend && !(s as SeriesLike).__total).map((s) => (s as SeriesLike).name);
  const legend = showLegend
    ? { top: 0, left: "center" as const, type: "scroll" as const, ...(legendNames.length !== series.length ? { data: legendNames } : {}) }
    : undefined;

  // Formatted tooltip — one row per (non-trend) series, value via its metric's format.
  // Series may be renamed, so resolve the source metric from each series' __metric.
  const metricByName = new Map<string, string>();
  for (const s of series as SeriesLike[]) {
    if (s.name != null && s.__metric != null) metricByName.set(s.name, s.__metric);
  }
  const tipFmt = (name: string, v: number) => {
    if (normalized) return `${Math.round(v)}%`;
    const m = metricByName.get(name) ?? (cfg.yMetrics.includes(name) ? name : cfg.yMetrics[0]);
    return m ? formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]) : String(v);
  };
  type TipParam = { axisValueLabel?: string; name?: string; seriesName?: string; value?: unknown; marker?: string };
  const tooltipFormatter = (params: TipParam | TipParam[]) => {
    const arr = Array.isArray(params) ? params : [params];
    const visible = arr.filter((p) => p.value != null && p.seriesName && !p.seriesName.endsWith(" trend"));
    if (visible.length === 0) return "";
    const header = visible[0].axisValueLabel ?? visible[0].name ?? "";
    // On a value/time x-axis the datum is an [x, y] pair — take y.
    const yOf = (v: unknown) => (Array.isArray(v) ? Number(v[1]) : Number(v));
    const lines = visible.map((p) => `${p.marker ?? ""} ${p.seriesName ?? ""}: <b>${tipFmt(p.seriesName ?? "", yOf(p.value))}</b>`);
    return `${header}<br/>${lines.join("<br/>")}`;
  };

  // The first plotted series carries the values a screen reader hears; the rest are named
  // in the count, since reading every series of a multi-series chart is worse than useless.
  const described = (series as SeriesLike[]).filter((s) => !s.__trend && !s.__total);
  const first = described[0];
  return {
    aria: ariaDescription({
      kind: `${cfg.kind === "line" ? "Line" : cfg.kind === "area" ? "Area" : cfg.kind === "scatter" ? "Scatter" : cfg.kind === "combo" ? "Combo" : "Bar"} chart`,
      // No fallback to cfg.yMetrics[0]: with every series hidden there is no metric being
      // described, and naming one would tell a screen reader about a chart nobody can see.
      metric: first?.name,
      dimension: cfg.xAxis,
      seriesCount: described.length,
      format: (v) => fmtFor(cfg.yMetrics)(Number(v)),
      points: first ? categories.map((c, i) => ({ name: String(c), value: first.data?.[i] })) : [],
    }),
    color: undefined,
    // The product's own typeface, at the option root — this is the one builder that composes its
    // whole option itself rather than spreading base.ts's `baseOption()`, so it has to say so.
    textStyle: baseTextStyle(),
    grid,
    tooltip: { trigger: cfg.kind === "scatter" ? "item" : "axis", ...tooltipSurface(), formatter: tooltipFormatter },
    legend,
    xAxis,
    yAxis,
    series,
  };
}
