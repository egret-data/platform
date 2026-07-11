import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { ARIA_A11Y, SERIES_PALETTE } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface SankeyBuildConfig {
  source?: string;
  target?: string;
  metric?: string;
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

// Metabase-style Sankey: weighted flows from source category to target category.
export function buildSankeyOption(columns: ResultColumn[], rows: unknown[][], cfg: SankeyBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const si = cfg.source != null ? idx[cfg.source] : undefined;
  const ti = cfg.target != null ? idx[cfg.target] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  const links: { source: string; target: string; value: number }[] = [];
  const names = new Set<string>();
  if (si != null && ti != null && mi != null) {
    for (const r of rows) {
      const value = Number(r[mi]);
      if (!Number.isFinite(value) || value <= 0) continue;
      const source = String(r[si] ?? "");
      const target = String(r[ti] ?? "");
      links.push({ source, target, value });
      names.add(source);
      names.add(target);
    }
  }
  const data = [...names].map((name) => ({ name }));

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);

  return {
    aria: ARIA_A11Y, // screen-reader data description (non-visual fallback)
    color: SERIES_PALETTE,
    tooltip: { trigger: "item", formatter: (p: { dataType: string; name: string; value: unknown }) => (p.dataType === "edge" ? `${p.name}: ${fmt(p.value)}` : p.name) },
    series: [{
      type: "sankey",
      data,
      links,
      emphasis: { focus: "adjacency" },
      lineStyle: { color: "gradient", opacity: 0.4 },
      label: { fontSize: 12 },
      left: "3%", right: "8%", top: "4%", bottom: "4%",
    }],
  };
}
