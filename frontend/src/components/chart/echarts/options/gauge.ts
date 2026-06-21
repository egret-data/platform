import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { SERIES_PALETTE, COLOR_FALLBACK, resolveColor } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface GaugeBuildConfig {
  metric?: string;
  min?: number;
  max?: number;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

const MAIN = SERIES_PALETTE[0]; // #509EE3
const TRACK = "#D9ECFB";

// Metabase-style Gauge: one value on an arc between min and max.
export function buildGaugeOption(columns: ResultColumn[], rows: unknown[][], cfg: GaugeBuildConfig): EChartsCoreOption {
  const mi = cfg.metric != null ? columns.findIndex((c) => c.name === cfg.metric) : -1;
  const raw = mi >= 0 && rows.length > 0 ? rows[0][mi] : undefined;
  const value = raw != null && Number.isFinite(Number(raw)) ? Number(raw) : 0;
  const min = cfg.min ?? 0;
  const max = cfg.max ?? (value > min ? value * 1.25 : min + 100);

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);
  const axisLabelColor = resolveColor("var(--muted-foreground)", COLOR_FALLBACK.axisLabel);
  const textColor = resolveColor("var(--foreground)", COLOR_FALLBACK.text);

  return {
    series: [{
      type: "gauge",
      min, max,
      startAngle: 210, endAngle: -30,
      progress: { show: true, width: 16, itemStyle: { color: MAIN } },
      axisLine: { lineStyle: { width: 16, color: [[1, TRACK]] } },
      axisTick: { show: false },
      splitLine: { show: false },
      axisLabel: { formatter: (v: number) => fmt(v), distance: 16, fontSize: 11, color: axisLabelColor },
      pointer: { show: true, itemStyle: { color: MAIN } },
      anchor: { show: false },
      title: { show: false },
      detail: { valueAnimation: false, formatter: (v: number) => fmt(v), fontSize: 24, fontWeight: 700, offsetCenter: [0, "55%"], color: textColor },
      data: [{ value }],
    }],
  };
}
