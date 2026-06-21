import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { SERIES_PALETTE } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface PieBuildConfig {
  dimension?: string;
  metric?: string;
  donut?: boolean;
  showLegend?: boolean;
  showLabels?: boolean;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

export function buildPieOption(columns: ResultColumn[], rows: unknown[][], cfg: PieBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const di = cfg.dimension != null ? idx[cfg.dimension] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  const data = di == null || mi == null
    ? []
    : rows.map((r) => ({ name: String(r[di] ?? ""), value: r[mi] as number }));

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);

  return {
    color: SERIES_PALETTE,
    tooltip: { trigger: "item", formatter: (p: { name: string; value: unknown; percent: number }) => `${p.name}: ${fmt(p.value)} (${p.percent}%)` },
    legend: cfg.showLegend ? { bottom: 0, type: "scroll" } : undefined,
    series: [{
      type: "pie",
      radius: cfg.donut ? ["45%", "70%"] : ["0%", "70%"],
      center: ["50%", "48%"],
      data,
      label: { show: !!cfg.showLabels, formatter: "{b}: {d}%" },
    }],
  };
}
