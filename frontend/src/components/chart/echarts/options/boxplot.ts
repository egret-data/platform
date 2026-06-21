import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
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
function fiveNumber(values: number[]): [number, number, number, number, number] {
  const s = [...values].sort((a, b) => a - b);
  return [s[0], quantile(s, 0.25), quantile(s, 0.5), quantile(s, 0.75), s[s.length - 1]];
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
  const data = categories.map((k) => fiveNumber(groups.get(k)!));

  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const fmt = (v: unknown) =>
    cfg.value ? formatValue(v, cfg.metricTypes?.[cfg.value] ?? "double", cfg.columnSettings?.[cfg.value]) : String(v);

  return {
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
    series: [{
      type: "boxplot",
      data,
      itemStyle: { color: SERIES_PALETTE[0], borderColor: SERIES_PALETTE[7] },
    }],
  };
}
