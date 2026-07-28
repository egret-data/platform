import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { baseOption } from "../base";
import { CHART_THEME, SERIES_PALETTE, COLOR_FALLBACK, resolveColor } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface BoxplotBuildConfig {
  category?: string;   // grouping column (raw)
  value?: string;      // numeric column whose distribution is summarised
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

// Linear-interpolation quantile over a pre-sorted array.
function quantile(sorted: number[], q: number): number {
  const pos = (sorted.length - 1) * q;
  const base = Math.floor(pos);
  const rest = pos - base;
  return sorted[base + 1] !== undefined ? sorted[base] + rest * (sorted[base + 1] - sorted[base]) : sorted[base];
}
// Tukey box: whiskers reach the furthest point within 1.5·IQR of the quartiles;
// points beyond are outliers. Also returns the mean. Matches Metabase's box plot.
function boxStats(values: number[]) {
  const s = [...values].sort((a, b) => a - b);
  const q1 = quantile(s, 0.25), med = quantile(s, 0.5), q3 = quantile(s, 0.75);
  const iqr = q3 - q1;
  const lowFence = q1 - 1.5 * iqr, highFence = q3 + 1.5 * iqr;
  const inRange = s.filter((v) => v >= lowFence && v <= highFence);
  const outliers = s.filter((v) => v < lowFence || v > highFence);
  const mean = s.reduce((t, v) => t + v, 0) / s.length;
  return { box: [inRange[0] ?? s[0], q1, med, q3, inRange[inRange.length - 1] ?? s[s.length - 1]], outliers, mean };
}

// Metabase-style Box plot: the distribution (min/Q1/median/Q3/max) of a value per
// category. Quartiles are computed client-side from the raw rows.
export function buildBoxplotOption(columns: ResultColumn[], rows: unknown[][], cfg: BoxplotBuildConfig): EChartsCoreOption {
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
  const categories = [...groups.keys()];
  const stats = categories.map((k) => boxStats(groups.get(k)!));
  const data = stats.map((s) => s.box);
  const outlierPoints = stats.flatMap((s, i) => s.outliers.map((v) => [i, v]));
  const meanPoints = stats.map((s, i) => [i, s.mean]);

  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const fmt = (v: unknown) =>
    cfg.value ? formatValue(v, cfg.metricTypes?.[cfg.value] ?? "double", cfg.columnSettings?.[cfg.value]) : String(v);

  return {
    ...baseOption(),
    grid: CHART_THEME.grid,
    tooltip: { trigger: "item" },
    xAxis: {
      type: "category", data: categories,
      axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor },
      axisLine: { lineStyle: { color: gridLine } },
    },
    yAxis: {
      type: "value",
      axisLabel: { fontSize: CHART_THEME.axisFontSize, color: axisLabelColor, formatter: (v: number) => fmt(v) },
      splitLine: { lineStyle: { color: gridLine } },
    },
    series: [
      {
        type: "boxplot",
        data,
        // Light green box with a green outline, like Metabase.
        itemStyle: { color: "#E3F0D4", borderColor: SERIES_PALETTE[1] },
      },
      // Outliers (beyond 1.5·IQR) as individual points.
      { type: "scatter", data: outlierPoints, symbolSize: 6, itemStyle: { color: SERIES_PALETTE[1] }, tooltip: { trigger: "item" } },
      // Mean as a diamond.
      { type: "scatter", data: meanPoints, symbol: "diamond", symbolSize: 9, itemStyle: { color: SERIES_PALETTE[7] }, tooltip: { trigger: "item" } },
    ],
  };
}
