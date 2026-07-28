import type { EChartsCoreOption } from "echarts/core";
import type { CartesianViz } from "@/api/charts";
import type { ColumnFormat } from "../../format-types";
import { isNumericType, isTemporalType } from "../../format-types";
import { formatValue } from "../../format";
import { ariaDescription } from "../base";
import { CHART_THEME, COLOR_FALLBACK, lineType, resolveColor, seriesColor, SUMMARIZE_COLOR, symbolShape } from "../palette";
import { isExtractUnit, temporalLabel } from "@/components/chart/temporal-units";
import { foldTail } from "../fold";

export interface ResultColumn { name: string; type: string }
export interface CartesianBuildConfig {
  kind: CartesianViz;
  xAxis?: string;
  xMetric?: string;        // scatter only: a metric on the x-axis (metric-vs-metric)
  yMetrics: string[];
  series?: string;
  stacked?: boolean;
  horizontal?: boolean;
  smooth?: boolean;
  showLabels?: boolean;
  showLegend?: boolean;
  xLabel?: string;
  yLabel?: string;
  seriesTypes?: Record<string, "bar" | "line">;
  rightAxisMetrics?: string[];   // metrics plotted against a second (right) Y axis
  logScale?: boolean;            // value axis uses a log scale
  normalized?: boolean;          // 100% stacking — each category's values sum to 100
  goalValue?: number;            // horizontal reference (goal) line
  goalLabel?: string;
  trendLine?: boolean;           // overlay a least-squares trend line per series
  connectNulls?: boolean;        // line: bridge gaps across null points
  step?: boolean;                // line: step interpolation
  showStackTotal?: boolean;      // label the total atop each stacked column
  seriesColors?: Record<string, string>;  // series name -> colour override (Metabase series settings)
  seriesLabels?: Record<string, string>;  // series name -> display label override
  yMin?: number;                 // manual value-axis minimum (auto when undefined)
  yMax?: number;                 // manual value-axis maximum
  showMarkers?: boolean;         // line/area: show the point symbols (off by default, like Metabase)
  xScale?: "ordinal" | "linear" | "timeseries";  // x-axis scale (default ordinal/category)
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
  xAxisUnit?: string;            // temporal extraction unit for the x-axis dimension (e.g. "day_of_week")
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

interface SeriesLike { name?: string; data: (number | null)[]; yAxisIndex?: number; itemStyle?: { color?: string }; __trend?: boolean; __total?: boolean; __metric?: string }

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

const isNormalized = (cfg: CartesianBuildConfig) => !!cfg.normalized && !!cfg.stacked;

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

// Right-axis is active only when it splits the metrics (≥1 left and ≥1 right) on a
// vertical chart — clean-room of Metabase's left/right axis split.
function rightMetrics(cfg: CartesianBuildConfig): string[] {
  return (cfg.rightAxisMetrics ?? []).filter((m) => cfg.yMetrics.includes(m));
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
function seriesTypeFor(kind: CartesianViz, metric: string, seriesTypes?: Record<string, "bar" | "line">) {
  if (kind === "combo") return seriesTypes?.[metric] ?? "bar";
  if (kind === "area") return "line";
  if (kind === "scatter") return "scatter";
  if (kind === "row") return "bar";
  return kind; // "bar" | "line"
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
function scatterXY(columns: ResultColumn[], rows: unknown[][], cfg: CartesianBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const di = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
  const xi = idx[cfg.xMetric!];
  const yMetric = cfg.yMetrics[0];
  const yi = idx[yMetric];
  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const fmtM = (m: string, v: unknown) => formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]);
  const data = rows
    .map((r) => ({ value: [Number(r[xi]), Number(r[yi])], name: di != null ? String(r[di] ?? "") : "" }))
    .filter((p) => Number.isFinite(p.value[0]) && Number.isFinite(p.value[1]));
  return {
    grid: { ...CHART_THEME.grid, bottom: (CHART_THEME.grid.bottom as number) + 22 },
    tooltip: { trigger: "item", formatter: (p: { data: { name: string }; value: number[] }) => `${p.data.name}<br/>${cfg.xMetric}: ${fmtM(cfg.xMetric!, p.value[0])}<br/>${yMetric}: ${fmtM(yMetric, p.value[1])}` },
    xAxis: { type: "value", name: cfg.xMetric, nameLocation: "middle", nameGap: 28, axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: (v: number) => fmtM(cfg.xMetric!, v) }, splitLine: { lineStyle: { color: gridLine } } },
    yAxis: { type: "value", name: yMetric, axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: (v: number) => fmtM(yMetric, v) }, splitLine: { lineStyle: { color: gridLine } } },
    series: [{ type: "scatter", symbolSize: 10, itemStyle: { color: SUMMARIZE_COLOR, opacity: CHART_THEME.scatterOpacity }, data }],
  };
}

export function buildCartesianOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: CartesianBuildConfig,
): EChartsCoreOption {
  if (cfg.kind === "scatter" && cfg.xMetric && cfg.yMetrics.length > 0) {
    return scatterXY(columns, rows, cfg);
  }
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const xi = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
  const fmt = (metric: string, v: unknown) =>
    formatValue(v, cfg.metricTypes?.[metric] ?? "double", cfg.columnSettings?.[metric]);

  if (xi == null || cfg.yMetrics.length === 0) {
    return baseOption(cfg, [], []);
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
    const grouped = !cfg.stacked && seriesTypeFor(cfg.kind, metric, cfg.seriesTypes) === "bar" && seriesValues.length > 1;
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
    if (cfg.stacked && cfg.showStackTotal && !isNormalized(cfg)) series.push(stackTotalSeries(series as unknown as SeriesLike[], cfg) as unknown as (typeof series)[number]);
    if (usesXScale(cfg)) applyXScale(categories, series, cfg);
    const displayCats = isExtractUnit(cfg.xAxisUnit) ? categories.map((v) => temporalLabel(cfg.xAxisUnit, v)) : categories;
    return baseOption(cfg, displayCats as (string | number)[], series);
  }

  const rowByCat = new Map(xrows.map((r) => [catKey(r[xi]), r]));
  const barMetrics = cfg.yMetrics.filter((m) => seriesTypeFor(cfg.kind, m, cfg.seriesTypes) === "bar").length;
  const grouped = !cfg.stacked && barMetrics > 1;
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
    categories = folded.map((p) => p.name);
    s.data = folded.map((p) => (typeof p.value === "number" ? p.value : null));
  }

  series = applySeriesOrder(series as unknown as SeriesLike[], cfg) as unknown as typeof series;

  if (isNormalized(cfg)) normalizeStacks(series as unknown as { data: (number | null)[] }[]);
  if (cfg.trendLine) series.push(...(trendSeries(series as unknown as SeriesLike[]) as unknown as typeof series));
  if (cfg.stacked && cfg.showStackTotal && !isNormalized(cfg)) series.push(stackTotalSeries(series as unknown as SeriesLike[], cfg) as unknown as (typeof series)[number]);
  if (usesXScale(cfg)) applyXScale(categories, series, cfg);
  const displayCats = isExtractUnit(cfg.xAxisUnit) ? categories.map((v) => temporalLabel(cfg.xAxisUnit, v)) : categories;
  return baseOption(cfg, displayCats as (string | number)[], series);
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
  const type = seriesTypeFor(cfg.kind, metric, cfg.seriesTypes);
  // Per-series colour (keyed by the original series name) wins over the single-series
  // green and the palette slot — clean-room of Metabase's series colour settings.
  const color = cfg.seriesColors?.[s.name] ?? colorOverride ?? seriesColor(colorIdx);
  const out: Record<string, unknown> = {
    ...s,
    name: cfg.seriesLabels?.[s.name] ?? s.name,  // display label override (legend/tooltip)
    __metric: metric,                            // keep the source metric for tooltip formatting
    type,
    itemStyle: { color },
  };
  // Only pin the bar width for a single bar per category; grouped bars must let
  // ECharts auto-size and place them side by side (otherwise they overlap).
  if (type === "bar") { if (!grouped) out.barWidth = CHART_THEME.barWidth; out.barMinHeight = BAR_MIN_HEIGHT; }
  if (type === "bar" || cfg.kind === "area") {
    if (cfg.stacked) out.stack = "total";
  }
  if (cfg.kind === "area") out.areaStyle = { opacity: CHART_THEME.areaOpacity };
  if (type === "line") {
    out.smooth = !!cfg.smooth;
    // Cycle the dash style + marker shape per series so lines are distinguishable
    // without relying on colour alone (a11y: colour-blind users).
    out.lineStyle = { width: CHART_THEME.lineWidth, type: lineType(colorIdx) };
    out.symbol = symbolShape(colorIdx);
    out.symbolSize = CHART_THEME.symbolSize;
    out.showSymbol = cfg.showMarkers !== false;  // markers shown unless explicitly turned off
    out.connectNulls = cfg.connectNulls ?? true; // Metabase connects across nulls by default
    if (cfg.step) out.step = "middle";
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

function baseOption(cfg: CartesianBuildConfig, categories: (string | number)[], series: unknown[]): EChartsCoreOption {
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
  const xTitle = cfg.xLabel ?? titleOf(cfg.xAxis);

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
  const yTitle = cfg.yLabel ?? metricTitle(leftSide);
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

  // Always ECharts' own `'auto'`: it measures the rendered labels and drops the ones that
  // would collide. Any rule of ours is a function of the category COUNT, and the thing that
  // decides whether labels fit is the axis WIDTH — which the option builder never sees. An
  // evenly-spaced every-Nth rule looks tidier on a wide builder canvas and is illegible on a
  // dashboard card.
  const longest = categories.reduce((n: number, c) => Math.max(n, String(c).length), 0);
  // Long names are unreadable upright regardless of how many survive thinning (there is no
  // thinning any more) — only on a bottom axis; a horizontal chart stacks categories down
  // the left, where length costs nothing.
  const rotate = !horizontal && longest > 10 && categories.length > 6 ? 30 : 0;

  const categoryAxis = {
    type: "category" as const, data: categories,
    name: xTitle, nameLocation: "middle" as const, nameGap: 28,
    // The title matches the label colour — an axis name is not a lesser piece of chrome
    // than the values beneath it, and ECharts' default nameTextStyle colour reads paler.
    nameTextStyle: { color: axisLabelColor },
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, interval: "auto" as const, rotate },
    axisLine: { lineStyle: { color: gridLine } },
  };
  // Non-ordinal x: a value (linear) or time (timeseries) axis positioning points by
  // their real coordinate. Series carry [x, y] pairs (see applyXScale).
  const scaleAxis = {
    type: cfg.xScale === "timeseries" ? ("time" as const) : ("value" as const),
    name: xTitle, nameLocation: "middle" as const, nameGap: 28,
    nameTextStyle: { color: axisLabelColor },
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor },
    axisLine: { lineStyle: { color: gridLine } },
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
    bottom: (CHART_THEME.grid.bottom as number) + (bottomTitle ? 22 : 0) + (rotate ? 18 : 0),
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
    grid,
    tooltip: { trigger: cfg.kind === "scatter" ? "item" : "axis", formatter: tooltipFormatter },
    legend,
    xAxis,
    yAxis,
    series,
  };
}
