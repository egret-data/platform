import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { ARIA_A11Y, SERIES_PALETTE } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface FunnelBuildConfig {
  dimension?: string;
  metric?: string;
  showLabels?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

// Metabase-style Funnel: one row per stage, widths by metric, sorted descending.
export function buildFunnelOption(columns: ResultColumn[], rows: unknown[][], cfg: FunnelBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const di = cfg.dimension != null ? idx[cfg.dimension] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  const data = di == null || mi == null
    ? []
    : rows
        .map((r) => ({ name: String(r[di] ?? ""), value: Number(r[mi]) }))
        .filter((s) => Number.isFinite(s.value) && s.value > 0)
        .sort((a, b) => b.value - a.value);

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);

  return {
    aria: ARIA_A11Y, // screen-reader data description (non-visual fallback)
    color: SERIES_PALETTE,
    tooltip: { trigger: "item", formatter: (p: { name: string; value: unknown; percent: number }) => `${p.name}: ${fmt(p.value)} (${p.percent}%)` },
    series: [{
      type: "funnel",
      orient: "horizontal",   // Metabase renders funnels left→right
      sort: "descending",
      funnelAlign: "center",
      gap: 2,
      left: "5%",
      right: "5%",
      top: "6%",
      bottom: "6%",
      data,
      label: { show: cfg.showLabels !== false, position: "inside", formatter: (p: { name: string; value: unknown }) => `${p.name}: ${fmt(p.value)}` },
    }],
  };
}
