import type { EChartsCoreOption } from "echarts/core";
import type { CartesianViz } from "@/api/charts";
import type { ColumnFormat } from "../../format-types";
import { isNumericType, isTemporalType } from "../../format-types";
import { formatValue } from "../../format";
import { ariaDescription } from "../base";
import { CHART_THEME, COLOR_FALLBACK, lineType, resolveColor, seriesColor, SUMMARIZE_COLOR, symbolShape } from "../palette";
import { isExtractUnit, temporalLabel } from "@/components/chart/temporal-units";

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
  const categories = sortCategories(distinct(xrows.map((r) => r[xi])), xType);


  // Breakout: one ECharts series per distinct series value (first y-metric only).
  if (cfg.series != null && idx[cfg.series] != null) {
    const gi = idx[cfg.series];
    const metric = cfg.yMetrics[0];
    const mi = idx[metric];
    const seriesValues = distinct(xrows.map((r) => r[gi]));
    const cell = new Map(xrows.map((r) => [`${catKey(r[xi])} ${catKey(r[gi])}`, r[mi] as number | null]));
    // Grouped bars (>1 series, not stacked) must auto-size to sit side by side.
    const grouped = !cfg.stacked && seriesTypeFor(cfg.kind, metric, cfg.seriesTypes) === "bar" && seriesValues.length > 1;
    const series = seriesValues.map((sv, si) => {
      const data = categories.map((c) => {
        const v = cell.get(`${catKey(c)} ${catKey(sv)}`);
        return v === undefined ? null : v;
      });
      // Metabase parity: null / empty-string breakout values render as "(empty)".
      const name = sv == null || sv === "" ? "(empty)" : String(sv);
      return decorateSeries({ name, data }, metric, si, cfg, fmt, grouped);
    });
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
  const series = cfg.yMetrics.map((metric, si) => {
    const mi = idx[metric];
    const data = categories.map((c) => {
      const r = rowByCat.get(catKey(c));
      return r == null ? null : (r[mi] as number | null);
    });
    return decorateSeries({ name: metric, data }, metric, si, cfg, fmt, grouped, singleColor);
  });
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
  // A column the user renamed in Formatting should carry that name onto the axis.
  const titleOf = (col: string | undefined) =>
    col == null ? undefined : (cfg.columnSettings?.[col]?.column_title ?? col);
  const xTitle = cfg.xLabel ?? titleOf(cfg.xAxis);
  const yTitle = cfg.yLabel ?? titleOf(cfg.yMetrics[0]);
  const fmtFor = (metrics: string[]) => (v: number) => {
    if (normalized) return `${Math.round(v)}%`;
    const m = metrics[0];
    return m ? formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]) : String(v);
  };
  const categoryAxis = {
    type: "category" as const, data: categories,
    name: xTitle, nameLocation: "middle" as const, nameGap: 28,
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor },
    axisLine: { lineStyle: { color: gridLine } },
  };
  // Non-ordinal x: a value (linear) or time (timeseries) axis positioning points by
  // their real coordinate. Series carry [x, y] pairs (see applyXScale).
  const scaleAxis = {
    type: cfg.xScale === "timeseries" ? ("time" as const) : ("value" as const),
    name: xTitle, nameLocation: "middle" as const, nameGap: 28,
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor },
    axisLine: { lineStyle: { color: gridLine } },
    splitLine: { show: false as const },
  };
  const xCategory = usesXScale(cfg) ? scaleAxis : categoryAxis;
  const mkValueAxis = (metrics: string[], position?: "left" | "right") => ({
    type: cfg.logScale ? ("log" as const) : ("value" as const),
    name: position === "right" ? undefined : yTitle,
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
    xAxis = mkValueAxis(cfg.yMetrics);
    yAxis = categoryAxis;
  } else if (isDualAxis(cfg)) {
    const right = rightMetrics(cfg);
    const left = cfg.yMetrics.filter((m) => !right.includes(m));
    xAxis = xCategory;
    yAxis = [mkValueAxis(left, "left"), mkValueAxis(right, "right")];
  } else {
    xAxis = xCategory;
    yAxis = mkValueAxis(cfg.yMetrics);
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

  // Metabase puts the legend along the top; reserve room there for it, and below
  // only for the x-axis title.
  const grid = {
    ...CHART_THEME.grid,
    top: (CHART_THEME.grid.top as number) + (showLegend ? 24 : 0),
    bottom: (CHART_THEME.grid.bottom as number) + (xTitle ? 22 : 0),
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
      metric: first?.name ?? cfg.yMetrics[0],
      dimension: cfg.xAxis,
      seriesCount: described.length,
      format: (v) => fmtFor(cfg.yMetrics)(Number(v)),
      points: categories.map((c, i) => ({ name: String(c), value: first?.data?.[i] })),
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
