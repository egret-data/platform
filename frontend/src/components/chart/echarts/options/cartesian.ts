import type { EChartsCoreOption } from "echarts/core";
import type { CartesianViz } from "@/api/charts";
import type { ColumnFormat } from "../../format-types";
import { isNumericType, isTemporalType } from "../../format-types";
import { formatValue } from "../../format";
import { CHART_THEME, COLOR_FALLBACK, resolveColor, seriesColor } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface CartesianBuildConfig {
  kind: CartesianViz;
  xAxis?: string;
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
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

const BAR_MIN_HEIGHT = 1; // keep sub-pixel bars visible, like Metabase's barMinHeight

interface SeriesLike { name?: string; data: (number | null)[]; yAxisIndex?: number; itemStyle?: { color?: string }; __trend?: boolean }

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

export function buildCartesianOption(
  columns: ResultColumn[],
  rows: unknown[][],
  cfg: CartesianBuildConfig,
): EChartsCoreOption {
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

  // Row chart: order categories by the first metric's value, ascending — ECharts
  // puts y-axis index 0 at the bottom, so ascending puts the largest bar on top.
  if (cfg.kind === "row" && cfg.yMetrics.length > 0) {
    const m0 = idx[cfg.yMetrics[0]];
    const totals = new Map<string, number>();
    for (const r of xrows) {
      const v = Number(r[m0]);
      if (Number.isFinite(v)) totals.set(catKey(r[xi]), (totals.get(catKey(r[xi])) ?? 0) + v);
    }
    categories = [...categories].sort((a, b) => (totals.get(catKey(a)) ?? 0) - (totals.get(catKey(b)) ?? 0));
  }

  // Breakout: one ECharts series per distinct series value (first y-metric only).
  if (cfg.series != null && idx[cfg.series] != null) {
    const gi = idx[cfg.series];
    const metric = cfg.yMetrics[0];
    const mi = idx[metric];
    const seriesValues = distinct(xrows.map((r) => r[gi]));
    const cell = new Map(xrows.map((r) => [`${catKey(r[xi])} ${catKey(r[gi])}`, r[mi] as number | null]));
    const series = seriesValues.map((sv, si) => {
      const data = categories.map((c) => {
        const v = cell.get(`${catKey(c)} ${catKey(sv)}`);
        return v === undefined ? null : v;
      });
      // Metabase parity: null / empty-string breakout values render as "(empty)".
      const name = sv == null || sv === "" ? "(empty)" : String(sv);
      return decorateSeries({ name, data }, metric, si, cfg, fmt);
    });
    if (isNormalized(cfg)) normalizeStacks(series as unknown as { data: (number | null)[] }[]);
    if (cfg.trendLine) series.push(...(trendSeries(series as unknown as SeriesLike[]) as unknown as typeof series));
    return baseOption(cfg, categories as (string | number)[], series);
  }

  const rowByCat = new Map(xrows.map((r) => [catKey(r[xi]), r]));
  const series = cfg.yMetrics.map((metric, si) => {
    const mi = idx[metric];
    const data = categories.map((c) => {
      const r = rowByCat.get(catKey(c));
      return r == null ? null : (r[mi] as number | null);
    });
    return decorateSeries({ name: metric, data }, metric, si, cfg, fmt);
  });
  if (isNormalized(cfg)) normalizeStacks(series as unknown as { data: (number | null)[] }[]);
  if (cfg.trendLine) series.push(...(trendSeries(series as unknown as SeriesLike[]) as unknown as typeof series));
  return baseOption(cfg, categories as (string | number)[], series);
}

// Shared decoration: type, color, stack, areaStyle, labels, smooth, symbol.
function decorateSeries(
  s: { name: string; data: (number | null)[] },
  metric: string,
  colorIdx: number,
  cfg: CartesianBuildConfig,
  fmt: (metric: string, v: unknown) => string,
) {
  const type = seriesTypeFor(cfg.kind, metric, cfg.seriesTypes);
  const out: Record<string, unknown> = {
    ...s,
    type,
    itemStyle: { color: seriesColor(colorIdx) },
  };
  if (type === "bar") { out.barWidth = CHART_THEME.barWidth; out.barMinHeight = BAR_MIN_HEIGHT; }
  if (type === "bar" || cfg.kind === "area") {
    if (cfg.stacked) out.stack = "total";
  }
  if (cfg.kind === "area") out.areaStyle = { opacity: CHART_THEME.areaOpacity };
  if (type === "line") {
    out.smooth = !!cfg.smooth;
    out.lineStyle = { width: CHART_THEME.lineWidth };
    out.symbolSize = CHART_THEME.symbolSize;
    out.connectNulls = !!cfg.connectNulls;
    if (cfg.step) out.step = "middle";
  }
  if (type === "scatter") { out.symbolSize = CHART_THEME.symbolSize; out.itemStyle = { color: seriesColor(colorIdx), opacity: CHART_THEME.scatterOpacity }; }
  if (isDualAxis(cfg) && rightMetrics(cfg).includes(metric)) out.yAxisIndex = 1;
  if (cfg.showLabels) {
    out.label = {
      show: true, fontSize: CHART_THEME.labelFontSize, fontWeight: CHART_THEME.labelWeight,
      position: isHorizontal(cfg) ? "right" : "top",
      formatter: (p: { value: unknown }) => fmt(metric, p.value),
    };
  }
  return out;
}

function baseOption(cfg: CartesianBuildConfig, categories: (string | number)[], series: unknown[]): EChartsCoreOption {
  const horizontal = isHorizontal(cfg);
  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const normalized = isNormalized(cfg);
  const fmtFor = (metrics: string[]) => (v: number) => {
    if (normalized) return `${Math.round(v)}%`;
    const m = metrics[0];
    return m ? formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]) : String(v);
  };
  const categoryAxis = {
    type: "category" as const, data: categories,
    name: cfg.xLabel, nameLocation: "middle" as const, nameGap: 28,
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor },
    axisLine: { lineStyle: { color: gridLine } },
  };
  const mkValueAxis = (metrics: string[], position?: "left" | "right") => ({
    type: cfg.logScale ? ("log" as const) : ("value" as const),
    name: position === "right" ? undefined : cfg.yLabel,
    position,
    ...(normalized ? { max: 100 } : {}),
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
    xAxis = categoryAxis;
    yAxis = [mkValueAxis(left, "left"), mkValueAxis(right, "right")];
  } else {
    xAxis = categoryAxis;
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

  // Reserve room below the plot so a bottom legend (and the x-axis title) don't
  // overlap the category labels.
  const grid = {
    ...CHART_THEME.grid,
    bottom: (CHART_THEME.grid.bottom as number) + (cfg.showLegend ? 28 : 0) + (cfg.xLabel ? 22 : 0),
  };

  // Trend series stay out of the legend.
  const legendNames = series.filter((s) => !(s as SeriesLike).__trend).map((s) => (s as SeriesLike).name);
  const legend = cfg.showLegend
    ? { bottom: 0, type: "scroll" as const, ...(legendNames.length !== series.length ? { data: legendNames } : {}) }
    : undefined;

  // Formatted tooltip — one row per (non-trend) series, value via its metric's format.
  const tipFmt = (name: string, v: number) => {
    if (normalized) return `${Math.round(v)}%`;
    const m = cfg.yMetrics.includes(name) ? name : cfg.yMetrics[0];
    return m ? formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]) : String(v);
  };
  type TipParam = { axisValueLabel?: string; name?: string; seriesName?: string; value?: unknown; marker?: string };
  const tooltipFormatter = (params: TipParam | TipParam[]) => {
    const arr = Array.isArray(params) ? params : [params];
    const visible = arr.filter((p) => p.value != null && !(typeof p.seriesName === "string" && p.seriesName.endsWith(" trend")));
    if (visible.length === 0) return "";
    const header = visible[0].axisValueLabel ?? visible[0].name ?? "";
    const lines = visible.map((p) => `${p.marker ?? ""} ${p.seriesName ?? ""}: <b>${tipFmt(p.seriesName ?? "", Number(p.value))}</b>`);
    return `${header}<br/>${lines.join("<br/>")}`;
  };

  return {
    color: undefined,
    grid,
    tooltip: { trigger: cfg.kind === "scatter" ? "item" : "axis", formatter: tooltipFormatter },
    legend,
    xAxis,
    yAxis,
    series,
  };
}
