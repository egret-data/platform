import type { EChartsCoreOption } from "echarts/core";
import type { CartesianViz, LineSeriesStyle } from "@/api/charts";
import type { EChartsSize } from "../EChartsRenderer";
import type { ColumnFormat } from "../../format-types";
import { isNumericType, isTemporalType } from "../../format-types";
import { formatValue } from "../../format";
import { ariaDescription, baseTextStyle, tooltipSurface } from "../base";
import { CHART_THEME, COLOR_FALLBACK, lineType, resolveColor, resolveFontFamily, seriesColor, SUMMARIZE_COLOR, symbolShape } from "../palette";
import { isExtractUnit, temporalLabel } from "@/components/chart/temporal-units";
import { foldTail, OTHER_LABEL } from "../fold";
import { fitXLabels, type XLabelFit } from "../axisLabelFit";
import { measureText } from "@/components/chart/measure-text";

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
  xAxisUnit?: string;            // temporal extraction unit for the x-axis dimension (e.g. "day_of_week")
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
  const series = groups.map((g, si) => {
    const own = points.filter((p) => p.group === g);
    // A lone series takes the summarize green, as everywhere else in the family.
    const color = cfg.seriesColors?.[g] ?? (groups.length === 1 ? SUMMARIZE_COLOR : seriesColor(si));
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
  // Resolve the automatic axis split ONCE, here, and hand the rest of the builder a config that
  // already names the right-axis metrics. Every downstream reader — `isDualAxis`, the per-series
  // `yAxisIndex`, the axis titles — then goes on asking the same question it always did, and a
  // computed split is indistinguishable from one the user assigned by hand.
  const cfg: CartesianBuildConfig = (cfg0.rightAxisMetrics ?? []).length > 0
    ? cfg0
    : (() => {
        const extents = metricExtents(columns, rows, cfg0.yMetrics);
        if (!shouldAutoSplitY(cfg0, extents)) return cfg0;
        const right = computeAxisSplit(cfg0.yMetrics, extents);
        // A split that puts everything on one side is not a split.
        return right.length > 0 && right.length < cfg0.yMetrics.length
          ? { ...cfg0, rightAxisMetrics: right }
          : cfg0;
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
    const builtSeries = seriesValues.map((sv, si) => {
      const data = categories.map((c) => {
        const v = cell.get(`${catKey(c)} ${catKey(sv)}`);
        return v === undefined ? null : v;
      });
      // Metabase parity: null / empty-string breakout values render as "(empty)".
      const name = sv == null || sv === "" ? "(empty)" : String(sv);
      return decorateSeries({ name, data }, metric, si, cfg, fmt, grouped);
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
  // Metabase tints a lone series with its summarize green rather than accent blue.
  const singleColor = cfg.yMetrics.length === 1 ? SUMMARIZE_COLOR : undefined;
  let series = cfg.yMetrics.map((metric, si) => {
    const mi = idx[metric];
    const data = categories.map((c) => {
      const r = rowByCat.get(catKey(c));
      return r == null ? null : (r[mi] as number | null);
    });
    return decorateSeries({ name: metric, data }, metric, si, cfg, fmt, grouped, singleColor);
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
  grouped = false,
  colorOverride?: string,
) {
  const display = seriesDisplayFor(cfg.kind, s.name ?? metric, colorIdx, cfg.seriesTypes);
  const type = echartsTypeOf(display);
  // Per-series colour (keyed by the original series name) wins over the single-series
  // green and the palette slot — clean-room of Metabase's series colour settings.
  const color = cfg.seriesColors?.[s.name] ?? colorOverride ?? seriesColor(colorIdx);
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

/**
 * The width the categories share, estimated.
 *
 * `CHART_THEME.grid` sets `containLabel: true` (`palette.ts`), so ECharts — not this builder —
 * decides the real plot box, and it picks the value-axis ticks that eat into it. Neither is
 * knowable here. The grid insets are ours; the gutter is approximated by formatting the widest
 * datum through the same formatter the axis uses.
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
 *      against a ~270px estimate that is the most material of the three. `rightSide` is in scope
 *      at the call site if this is ever fixed.
 *   3. `widestOf` takes the widest ABSOLUTE datum, so a leading minus sign is not counted.
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
 */
function estimatePlotWidth(size: EChartsSize, widestValueLabel: string, hasAxisTitle: boolean): number {
  const g = CHART_THEME.grid;
  // The family the chart is actually DRAWN in, never the literal "inherit": a CSS-wide keyword is
  // not a legal family in the `font` shorthand, so the canvas would reject the whole declaration
  // and measure at its 10px sans-serif default. Same rule, and the same reason, as `axisLabelFit`.
  const gutter = measureText(widestValueLabel, {
    size: CHART_THEME.axisFontSize, family: resolveFontFamily(), weight: CHART_THEME.axisWeight,
  }).width;
  // `26` is the room `grid.left` below reserves for a value-axis title, kept in step with it.
  return size.width - (g.left as number) - (g.right as number) - gutter - (hasAxisTitle ? 26 : 0);
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
  // generating its ticks first, which is PR 03.
  const ordinalX = !horizontal && !usesXScale(cfg);

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
  const fit: XLabelFit | undefined = measurable && size && ordinalX && !explicitRotate
    ? fitXLabels({
        // The RAW category, where `axisLabel.formatter` below draws the FORMATTED one. Where the
        // two differ — only when the x column carries a `columnSettings` entry — the formatted
        // label is the wider of the two ("2026-01-01" measured, "January 2026" drawn; `1000`
        // measured, "1,000" drawn), so this under-measures. That is the same direction as an
        // over-estimated `plotWidth`, and carries the same cost: labels left upright that should
        // have been rotated.
        labels: categories.map(String),
        plotWidth: estimatePlotWidth(size, fmtFor(leftSide)(widestOf(leftSide)), Boolean(yTitle)),
        // The CONTAINER's height, not an inset one: the 70% guard is a share of the whole card
        // (Metabase's `checkHeight`), so passing a grid-adjusted height silently tightens it.
        outerHeight: size.height,
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
  // Either way the axis has now DECIDED, and a decided axis draws every label or none — never the
  // subset `interval: "auto"` leaves ECharts free to pick.
  const decided = fit != null || explicitRotate;
  // Room below the plot for the tilted labels — `bottomPad` scales with the widest label, which is
  // what the measurement knows and the flat 18px fallback does not. Gated on `ordinalX` because
  // this is the BOTTOM margin: a row chart's rotated labels want room on the left (which
  // `containLabel` gives them), and a value x-axis never applied the rotation in the first place.
  const xLabelPad = fit ? (fit.kind === "rotate" ? fit.bottomPad : 0) : (rotate && ordinalX ? 18 : 0);

  const categoryAxis = {
    type: "category" as const, data: categories,
    name: xTitle, nameLocation: "middle" as const, nameGap: 28,
    // The title matches the label colour — an axis name is not a lesser piece of chrome
    // than the values beneath it, and ECharts' default nameTextStyle colour reads paler.
    nameTextStyle: { color: axisLabelColor },
    // Category labels go through the column's own format. Extraction units are already
    // substituted above (`displayCats`), but a TRUNCATION unit left the raw bucket on the axis —
    // a chart grouped by month read "2026-01-01" where the bucket is really January 2026. The
    // formatter only changes what is drawn; `data` stays the raw value, so a drill still filters
    // on what the engine returned.
    axisLabel: {
      fontSize: CHART_THEME.axisFontSize, color: axisLabelColor,
      interval: decided ? (0 as const) : ("auto" as const), rotate,
      // The third answer the ladder can give: when even a rotated label would eat 70% of the
      // card's height, Metabase draws no axis rather than a wall of text.
      show: fit?.kind !== "hide",
      formatter: (v: string) => {
        const fmt = cfg.xAxis ? cfg.columnSettings?.[cfg.xAxis] : undefined;
        if (!fmt || v == null) return String(v ?? "");
        const out = formatValue(v, cfg.xAxisType ?? "varchar", fmt);
        return out === "" ? String(v) : out;
      },
    },
    // TWO different `show`s sit on this axis and they are not the same switch. `xAxisTicks` is the
    // axis LINE and the tick marks; the LABELS are `axisLabel.show` above, which only the measured
    // fit ever writes. Metabase's single `axis_enabled: false` hides both, so egret's "Hide" is
    // narrower than its namesake — a pre-existing, product-wide divergence (waterfall, box plot and
    // sankey ship the same select today), but worth saying here now that a second `show` on the
    // same axis could be mistaken for the one the panel writes.
    axisLine: { show: cfg.xAxisTicks !== false, lineStyle: { color: gridLine } },
    axisTick: { show: cfg.xAxisTicks !== false },
  };
  // Non-ordinal x: a value (linear) or time (timeseries) axis positioning points by
  // their real coordinate. Series carry [x, y] pairs (see applyXScale).
  //
  // No `rotate` here, deliberately: an explicit `xLabelRotation` is a NO-OP on a value x-axis. Its
  // labels are ticks ECharts generates rather than the `categories` array, so rotating them is part
  // of generating them, which is PR 03. Task 3 is about to ship the control that exposes this, so
  // if someone reports the setting doing nothing on a linear or timeseries x-axis, this is why.
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
    ...(cfg.yTickCount != null ? { splitNumber: cfg.yTickCount } : {}),
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
    bottom: (CHART_THEME.grid.bottom as number) + (bottomTitle ? 22 : 0) + xLabelPad,
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
