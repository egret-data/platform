import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { ariaDescription, baseOption, itemTooltip } from "../base";


export interface ResultColumn { name: string; type: string }
export interface FunnelBuildConfig {
  dimension?: string;
  metric?: string;
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
    ...baseOption(),
    aria: ariaDescription({
      kind: "Funnel chart", metric: cfg.metric, dimension: cfg.dimension, format: fmt,
      points: data.map((d) => ({ name: String(d.name), value: d.value })),
    }),
    tooltip: itemTooltip(fmt, { percent: true }),
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
      // Always shown. This used to read `cfg.showLabels !== false`, but `showLabels` was on no
      // config any caller could produce — not `FunnelConfig`, not any settings panel — so the
      // `!== false` arm was unreachable and every funnel ever drawn took this branch. Now that
      // ChartRenderer SPREADS the config, an accepted-but-unsettable field stops being merely
      // dead and becomes a backdoor: a hand-edited `funnel.showLabels` in stored settings could
      // switch a chart's labels off with no control anywhere to switch them back on. Removed
      // rather than exposed — exposing it is a feature (field + panel toggle + builder), not
      // debt cleanup, and this keeps every funnel drawing exactly what it draws today.
      label: { show: true, position: "inside", formatter: (p: { name: string; value: unknown }) => `${p.name}: ${fmt(p.value)}` },
    }],
  };
}
