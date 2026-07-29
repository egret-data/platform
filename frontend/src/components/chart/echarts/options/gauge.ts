import type { EChartsCoreOption } from "echarts/core";
import type { GaugeSegment } from "@/api/charts";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { baseOption, itemTooltip } from "../base";
import { ARIA_A11Y } from "../palette";
import { SERIES_PALETTE, COLOR_FALLBACK, resolveColor } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface GaugeBuildConfig {
  metric?: string;
  /** Coloured ranges, in any order. The gauge's scale is derived from them. */
  segments?: GaugeSegment[];
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

const ZONE_LOW = SERIES_PALETTE[3];  // red
const ZONE_MID = SERIES_PALETTE[4];  // yellow
const ZONE_HIGH = SERIES_PALETTE[1]; // green

// Metabase-style Gauge: one value on an arc between min and max.
export function buildGaugeOption(columns: ResultColumn[], rows: unknown[][], cfg: GaugeBuildConfig): EChartsCoreOption {
  const mi = cfg.metric != null ? columns.findIndex((c) => c.name === cfg.metric) : -1;
  const raw = mi >= 0 && rows.length > 0 ? rows[0][mi] : undefined;
  const value = raw != null && Number.isFinite(Number(raw)) ? Number(raw) : 0;
  // Task 3 derives these from cfg.segments. Until then, today's auto-range verbatim — which is
  // identical to the old expression ONLY when no min/max was configured. A gauge that had them
  // set renders on the auto-range from this commit onward; migration 0102 (task 2) is what
  // converts those into segments, and it ships in the same PR, so no release sees one without
  // the other.
  const min = 0;
  const max = value > 0 ? value * 2 : 100;

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);
  const axisLabelColor = resolveColor("var(--muted-foreground)", COLOR_FALLBACK.axisLabel);
  const textColor = resolveColor("var(--foreground)", COLOR_FALLBACK.text);

  return {
    ...baseOption(),
    // One reading on an arc; what matters is the value and the range it sits in.
    aria: {
      ...ARIA_A11Y,
      label: { description: `Gauge${cfg.metric ? ` of ${cfg.metric}` : ""}. ${fmt(value)}, on a scale from ${fmt(min)} to ${fmt(max)}.` },
    },
    tooltip: itemTooltip(fmt),
    series: [{
      type: "gauge",
      min, max,
      startAngle: 210, endAngle: -30,
      // Three coloured zones (red / yellow / green), like Metabase's default gauge.
      axisLine: { lineStyle: { width: 18, color: [[1 / 3, ZONE_LOW], [2 / 3, ZONE_MID], [1, ZONE_HIGH]] } },
      axisTick: { show: false },
      splitLine: { show: false },
      axisLabel: { formatter: (v: number) => fmt(v), distance: 18, fontSize: 11, color: axisLabelColor },
      pointer: { show: true, itemStyle: { color: textColor } },
      anchor: { show: false },
      title: { show: false },
      detail: { valueAnimation: false, formatter: (v: number) => fmt(v), fontSize: 24, fontWeight: 700, offsetCenter: [0, "55%"], color: textColor },
      data: [{ value }],
    }],
  };
}
