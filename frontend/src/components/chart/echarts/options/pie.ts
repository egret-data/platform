import type { EChartsCoreOption } from "echarts/core";
import type { ColumnFormat } from "../../format-types";
import { formatValue } from "../../format";
import { SERIES_PALETTE, OTHER_SLICE_COLOR, COLOR_FALLBACK, resolveColor } from "../palette";

export interface ResultColumn { name: string; type: string }
export interface PieBuildConfig {
  dimension?: string;
  metric?: string;
  donut?: boolean;
  showLegend?: boolean;
  showLabels?: boolean;
  /** Slices below this percent of the total are grouped into one "Other" slice. Default 2.5; 0 disables. */
  sliceThreshold?: number;
  /** Sort slices by value descending. Default true. */
  sortRows?: boolean;
  /** Donut only: show the grand total in the centre. */
  showTotal?: boolean;
  /** Whether the slice's percent appears in its label. Default "off". */
  percentVisibility?: "off" | "inside" | "both";
  columnSettings?: Record<string, ColumnFormat>;
  metricTypes?: Record<string, string>;
}

const OTHER_NAME = "Other";
const DEFAULT_THRESHOLD = 2.5; // percent — Metabase SLICE_THRESHOLD (0.025)

interface Slice { name: string; value: number; itemStyle?: { color: string } }

// Clean-room of Metabase's pie data prep (pie/model/index.ts): drop zeros, handle
// negatives (all-negative → abs, mixed → drop negatives), sort by value desc, then
// fold sub-threshold slices into a single muted "Other".
function buildSlices(rows: unknown[][], di: number, mi: number, cfg: PieBuildConfig): Slice[] {
  let slices: Slice[] = rows
    .map((r) => ({ name: String(r[di] ?? ""), value: Number(r[mi]) }))
    .filter((s) => Number.isFinite(s.value));

  const allNegative = slices.length > 0 && slices.every((s) => s.value < 0);
  slices = allNegative
    ? slices.map((s) => ({ ...s, value: Math.abs(s.value) }))
    : slices.filter((s) => s.value > 0); // drops zeros and (mixed-sign) negatives

  if (cfg.sortRows !== false) {
    slices = [...slices].sort((a, b) => b.value - a.value);
  }

  const threshold = (cfg.sliceThreshold ?? DEFAULT_THRESHOLD) / 100;
  const total = slices.reduce((t, s) => t + s.value, 0);
  if (threshold > 0 && total > 0) {
    const small = slices.filter((s) => s.value / total < threshold);
    // Only worth an "Other" bucket when it merges 2+ slices (matches Metabase).
    if (small.length > 1) {
      const big = slices.filter((s) => s.value / total >= threshold);
      const otherTotal = small.reduce((t, s) => t + s.value, 0);
      const otherColor = resolveColor(OTHER_SLICE_COLOR, COLOR_FALLBACK.axisLabel);
      slices = [...big, { name: OTHER_NAME, value: otherTotal, itemStyle: { color: otherColor } }];
    }
  }

  return slices;
}

export function buildPieOption(columns: ResultColumn[], rows: unknown[][], cfg: PieBuildConfig): EChartsCoreOption {
  const idx = Object.fromEntries(columns.map((c, i) => [c.name, i]));
  const di = cfg.dimension != null ? idx[cfg.dimension] : undefined;
  const mi = cfg.metric != null ? idx[cfg.metric] : undefined;

  const data = di == null || mi == null ? [] : buildSlices(rows, di, mi, cfg);

  const fmt = (v: unknown) =>
    cfg.metric ? formatValue(v, cfg.metricTypes?.[cfg.metric] ?? "double", cfg.columnSettings?.[cfg.metric]) : String(v);

  // Label: name (showLabels) and/or percent (percentVisibility) — Metabase getSliceLabel.
  const showName = !!cfg.showLabels;
  const showPercent = cfg.percentVisibility === "inside" || cfg.percentVisibility === "both";
  const pct = (p: number) => `${Math.round(p * 10) / 10}%`;
  const labelFormatter = (p: { name: string; percent: number }) => {
    const name = showName ? p.name : undefined;
    const percent = showPercent ? pct(p.percent) : undefined;
    if (name != null && percent != null) return `${name}: ${percent}`;
    return name ?? percent ?? "";
  };

  // Donut centre total — Metabase getTotalGraphicOption (static total; no hover swap yet).
  const total = data.reduce((t, s) => t + s.value, 0);
  const graphic = cfg.donut && cfg.showTotal && data.length > 0
    ? [
        { type: "text", left: "center", top: "43%", style: { text: fmt(total), fontSize: 22, fontWeight: 700, fill: resolveColor("var(--foreground)", COLOR_FALLBACK.text), textAlign: "center" } },
        { type: "text", left: "center", top: "53%", style: { text: "Total", fontSize: 13, fill: resolveColor(OTHER_SLICE_COLOR, COLOR_FALLBACK.axisLabel), textAlign: "center" } },
      ]
    : undefined;

  return {
    color: SERIES_PALETTE,
    tooltip: { trigger: "item", formatter: (p: { name: string; value: unknown; percent: number }) => `${p.name}: ${fmt(p.value)} (${p.percent}%)` },
    legend: cfg.showLegend ? { bottom: 0, type: "scroll" } : undefined,
    graphic,
    series: [{
      type: "pie",
      radius: cfg.donut ? ["45%", "70%"] : ["0%", "70%"],
      center: ["50%", "48%"],
      data,
      label: { show: showName || showPercent, formatter: labelFormatter },
    }],
  };
}
