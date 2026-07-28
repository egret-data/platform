import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { isNumericType, isTemporalType } from "../../format-types";
import { formatValue } from "../../format";
import { baseOption, axisTooltip } from "../base";
import { CHART_THEME, SERIES_PALETTE, COLOR_FALLBACK, resolveColor } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface WaterfallBuildConfig {
  xAxis?: string;
  metric?: string;
  showTotal?: boolean;
  showLabels?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

const INCREASE = SERIES_PALETTE[1]; // green
const DECREASE = SERIES_PALETTE[3]; // red
const TOTAL = "#4C5773";            // dark navy, like Metabase's total bar

// Metabase-style Waterfall: sequential deltas with a running total. Built from two
// stacked bar series — a transparent base that floats the visible delta bar.
export function buildWaterfallOption(columns: ResultColumn[], rows: unknown[][], cfg: WaterfallBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const xi = cfg.xAxis != null ? idx[cfg.xAxis] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  const gridLine = resolveColor(CHART_THEME.gridLine, COLOR_FALLBACK.gridLine);
  const axisLabelColor = resolveColor(CHART_THEME.axisLabel, COLOR_FALLBACK.axisLabel);
  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);

  const categories: string[] = [];
  const base: number[] = [];
  const bars: { value: number; itemStyle: { color: string } }[] = [];

  if (xi != null && mi != null) {
    // Order steps by the x column (temporal chronological, numeric ascending, nominal
    // kept as-is) so the running total builds in a sensible sequence — Metabase parity.
    const xType = columns[xi].type;
    const ordered = isTemporalType(xType)
      ? [...rows].sort((a, b) => Date.parse(String(a[xi])) - Date.parse(String(b[xi])))
      : isNumericType(xType)
        ? [...rows].sort((a, b) => Number(a[xi]) - Number(b[xi]))
        : rows;
    let running = 0;
    for (const r of ordered) {
      const delta = Number(r[mi]);
      if (!Number.isFinite(delta)) continue;
      const before = running;
      const after = running + delta;
      categories.push(String(r[xi] ?? ""));
      base.push(Math.min(before, after));
      bars.push({ value: Math.abs(delta), itemStyle: { color: delta >= 0 ? INCREASE : DECREASE } });
      running = after;
    }
    if (cfg.showTotal && categories.length > 0) {
      categories.push("Total");
      base.push(0);
      bars.push({ value: running, itemStyle: { color: TOTAL } });
    }
  }

  return {
    ...baseOption(),
    grid: CHART_THEME.grid,
    tooltip: axisTooltip(),
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
      { type: "bar", stack: "wf", silent: true, itemStyle: { color: "transparent" }, emphasis: { itemStyle: { color: "transparent" } }, tooltip: { show: false }, data: base },
      {
        type: "bar", stack: "wf", barWidth: CHART_THEME.barWidth, data: bars,
        label: cfg.showLabels ? { show: true, position: "top", fontSize: CHART_THEME.labelFontSize, color: axisLabelColor, formatter: (p: { value: unknown }) => fmt(p.value) } : { show: false },
      },
    ],
  };
}
