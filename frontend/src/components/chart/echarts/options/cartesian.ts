import type { EChartsCoreOption } from "echarts/core";
import type { CartesianViz } from "@/api/charts";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { CHART_THEME, seriesColor } from "../palette";

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
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

// ECharts series type for a metric under the active kind.
function seriesTypeFor(kind: CartesianViz, metric: string, seriesTypes?: Record<string, "bar" | "line">) {
  if (kind === "combo") return seriesTypes?.[metric] ?? "bar";
  if (kind === "area") return "line";
  if (kind === "scatter") return "scatter";
  return kind; // "bar" | "line"
}

const distinct = (vals: unknown[]) => {
  const seen = new Set<string>();
  const out: unknown[] = [];
  for (const v of vals) { const k = String(v); if (!seen.has(k)) { seen.add(k); out.push(v); } }
  return out;
};

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

  const categories = distinct(rows.map((r) => r[xi]));
  const catKey = (v: unknown) => String(v);

  // Breakout: one ECharts series per distinct series value (first y-metric only).
  if (cfg.series != null && idx[cfg.series] != null) {
    const gi = idx[cfg.series];
    const metric = cfg.yMetrics[0];
    const mi = idx[metric];
    const seriesValues = distinct(rows.map((r) => r[gi]));
    const cell = new Map(rows.map((r) => [`${catKey(r[xi])} ${catKey(r[gi])}`, r[mi] as number | null]));
    const series = seriesValues.map((sv, si) => {
      const data = categories.map((c) => {
        const v = cell.get(`${catKey(c)} ${catKey(sv)}`);
        return v === undefined ? null : v;
      });
      // Metabase parity: null / empty-string breakout values render as "(empty)".
      const name = sv == null || sv === "" ? "(empty)" : String(sv);
      return decorateSeries({ name, data }, metric, si, cfg, fmt);
    });
    return baseOption(cfg, categories as (string | number)[], series);
  }

  const rowByCat = new Map(rows.map((r) => [catKey(r[xi]), r]));
  const series = cfg.yMetrics.map((metric, si) => {
    const mi = idx[metric];
    const data = categories.map((c) => {
      const r = rowByCat.get(catKey(c));
      return r == null ? null : (r[mi] as number | null);
    });
    return decorateSeries({ name: metric, data }, metric, si, cfg, fmt);
  });
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
  if (type === "bar") out.barWidth = CHART_THEME.barWidth;
  if (type === "bar" || cfg.kind === "area") {
    if (cfg.stacked) out.stack = "total";
  }
  if (cfg.kind === "area") out.areaStyle = { opacity: CHART_THEME.areaOpacity };
  if (type === "line") { out.smooth = !!cfg.smooth; out.lineStyle = { width: CHART_THEME.lineWidth }; out.symbolSize = CHART_THEME.symbolSize; }
  if (type === "scatter") { out.symbolSize = CHART_THEME.symbolSize; out.itemStyle = { color: seriesColor(colorIdx), opacity: CHART_THEME.scatterOpacity }; }
  if (cfg.showLabels) {
    out.label = {
      show: true, fontSize: CHART_THEME.labelFontSize, fontWeight: CHART_THEME.labelWeight,
      formatter: (p: { value: unknown }) => fmt(metric, p.value),
    };
  }
  return out;
}

function baseOption(cfg: CartesianBuildConfig, categories: (string | number)[], series: unknown[]): EChartsCoreOption {
  const horizontal = !!cfg.horizontal && cfg.kind !== "scatter";
  const valueFmt = (v: number) => {
    const m = cfg.yMetrics[0];
    return m ? formatValue(v, cfg.metricTypes?.[m] ?? "double", cfg.columnSettings?.[m]) : String(v);
  };
  const categoryAxis = {
    type: "category" as const, data: categories,
    name: cfg.xLabel, nameLocation: "middle" as const, nameGap: 28,
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: CHART_THEME.axisLabel },
    axisLine: { lineStyle: { color: CHART_THEME.gridLine } },
  };
  const valueAxis = {
    type: "value" as const, name: cfg.yLabel,
    axisLabel: { fontSize: CHART_THEME.axisFontSize, color: CHART_THEME.axisLabel, formatter: valueFmt },
    splitLine: { lineStyle: { color: CHART_THEME.gridLine } },
  };
  return {
    color: undefined,
    grid: CHART_THEME.grid,
    tooltip: { trigger: cfg.kind === "scatter" ? "item" : "axis" },
    legend: cfg.showLegend ? { bottom: 0, type: "scroll" } : undefined,
    xAxis: horizontal ? valueAxis : categoryAxis,
    yAxis: horizontal ? categoryAxis : valueAxis,
    series,
  };
}
